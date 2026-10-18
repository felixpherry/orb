//! The running orb's side of the control socket: reads each request, works out
//! from orb's state what it comes to, has `agent_ops` and `pane_ops` carry out
//! agent and pane requests, asks the sessions actor for what it adds or the
//! screen loop for what it changes, and writes the reply.

use std::fs::{self, Permissions};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime};

use error_stack::{Report, ResultExt};
use kameo::actor::ActorRef;
use kameo::error::SendError;
use wherror::Error;

use crate::feat::control::agent::Timing;
use crate::feat::control::agent_ops::PromptAsk;
use crate::feat::control::change::Change;
use crate::feat::control::lists::{ended, route, settled, tab_panes};
use crate::feat::control::pane_control::{PaneControl, PaneRequest};
use crate::feat::control::pane_ops::{PaneTiming, RunAsk, WaitAsk};
use crate::feat::control::protocol::{
    AgentDetail, ApiError, ClosedResult, ErrorCode, Op, ReadResult, Reply, Request, SettledResult,
    StartResult,
};
use crate::feat::control::screen_text;
use crate::feat::control::target::{self, Resolved, Target};
use crate::feat::control::turns::{Prompts, Turns};
use crate::feat::control::validator;
use crate::feat::control::{agent_ops, pane_ops};
use crate::feat::harness::Harnesses;
use crate::feat::plate::api::{self as plate, PlateAsk};
use crate::feat::sessions::sessions_actor::{ApiAdd, ApiStart, ApiStartError, SessionsActor};
use crate::feat::sessions::state::SessionKind;
use crate::ids::{PaneId, ProjectId, SessionId};
use crate::{AppState, Log, State};

/// How long a connection may take to send its request line.
const REQUEST_LIMIT: Duration = Duration::from_secs(5);

/// The control socket couldn't be opened.
#[derive(Debug, Error)]
#[error(debug)]
pub struct ControlServerError;

/// What the control server reads, who it asks, and where it logs.
#[derive(Debug, Clone)]
pub struct ControlDeps {
    /// orb's shared state, read to answer requests.
    pub state: State,
    /// Where failures go.
    pub log: Log,
    /// Makes panes and sessions.
    pub sessions: ActorRef<SessionsActor>,
    /// Runs the sessions actor's asks from the request threads.
    pub runtime: tokio::runtime::Handle,
    /// The screen loop, which applies changes to what it owns.
    pub panes: PaneControl,
    /// Read agents' transcripts for their replies.
    pub harnesses: Harnesses,
    /// What orb last pasted into each pane's agent; only the control server
    /// reads or writes it.
    pub prompts: Prompts,
}

impl ControlDeps {
    /// The read side of agents' turns over these deps.
    pub(super) const fn turns(&self) -> Turns<'_> {
        Turns {
            state: &self.state,
            harnesses: &self.harnesses,
            prompts: &self.prompts,
        }
    }
}

/// Opens the control socket at `path`, only for the user, and answers each
/// connection on its own thread. A socket left by an orb that crashed is
/// replaced.
///
/// # Errors
///
/// Returns an error if the socket can't be bound, made private, or its
/// thread can't start.
pub fn listen(path: &Path, deps: ControlDeps) -> Result<(), Report<ControlServerError>> {
    open(path, deps.log.clone(), move |line, gone| {
        reply_line(line, |request| answer(&deps, request, gone))
    })
}

/// Opens a socket at `path` as [`listen`] does, answering each request line
/// with the reply line `answer` gives; `answer`'s second argument says
/// whether the caller has hung up meanwhile.
pub(super) fn open<F>(path: &Path, log: Log, answer: F) -> Result<(), Report<ControlServerError>>
where
    F: Fn(&str, &dyn Fn() -> bool) -> String + Send + Sync + 'static,
{
    remove_socket(path, &log);
    let listener = UnixListener::bind(path)
        .change_context(ControlServerError)
        .attach_with(|| format!("couldn't bind {}", path.display()))?;
    fs::set_permissions(path, Permissions::from_mode(0o600))
        .change_context(ControlServerError)
        .attach("couldn't make the socket private")?;
    thread::Builder::new()
        .name("orb-control".into())
        .spawn(move || accept(&listener, &log, &Arc::new(answer)))
        .map(drop)
        .change_context(ControlServerError)
        .attach("couldn't start the control thread")
}

/// Removes the socket file; a missing one is fine, any other failure is logged.
pub fn remove_socket(path: &Path, log: &Log) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => log.line(
            "control",
            &format!("couldn't remove {}: {error}", path.display()),
        ),
    }
}

/// Hands each connection to its own thread, for as long as orb runs.
fn accept<F>(listener: &UnixListener, log: &Log, answer: &Arc<F>)
where
    F: Fn(&str, &dyn Fn() -> bool) -> String + Send + Sync + 'static,
{
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let answer = Arc::clone(answer);
                let spawned = thread::Builder::new()
                    .name("orb-control-request".into())
                    .spawn(move || serve(stream, REQUEST_LIMIT, &*answer));
                if let Err(error) = spawned {
                    log.error_on_change("control-spawn", "couldn't answer a request", &error);
                }
            }
            Err(error) => {
                log.error_on_change("control-accept", "couldn't accept a request", &error);
            }
        }
    }
}

/// Reads one request line, answers it, and writes one reply line. A client
/// that hangs up early, or sends no line within `read_limit`, gets nothing.
fn serve<F>(stream: UnixStream, read_limit: Duration, answer: &F)
where
    F: Fn(&str, &dyn Fn() -> bool) -> String,
{
    let mut line = String::new();
    let read = stream
        .set_read_timeout(Some(read_limit))
        .and_then(|()| BufReader::new(&stream).read_line(&mut line));
    if read.is_err() {
        return;
    }
    let probing = watch_for_hang_up(&stream);
    let gone = || probing && hung_up(&stream);
    let reply = answer(&line, &gone);
    let mut stream = stream;
    let _ = stream.set_nonblocking(false);
    let _ = writeln!(stream, "{reply}");
}

/// Makes reads on `stream` return at once, so [`hung_up`] can check it
/// without waiting. Unlike a read timeout, which macOS refuses to set once the
/// caller has hung up, this works either way.
fn watch_for_hang_up(stream: &UnixStream) -> bool {
    stream.set_nonblocking(true).is_ok()
}

/// Whether the caller on `stream` hung up. It never sends more than its one
/// line, so a later read either finds nothing yet (still there) or finds the
/// end of the stream (gone).
fn hung_up(mut stream: &UnixStream) -> bool {
    match stream.read(&mut [0; 1]) {
        Ok(0) => true,
        Ok(_) => false,
        Err(error) => !matches!(
            error.kind(),
            ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
        ),
    }
}

/// The control socket's reply line to `line`: `answer`'s reply to the
/// request it holds, or `bad_request` when it holds none.
fn reply_line<F>(line: &str, answer: F) -> String
where
    F: FnOnce(&Request) -> Reply,
{
    let reply = match serde_json::from_str::<Request>(line) {
        Ok(request) => answer(&request),
        Err(error) => Reply::err(ApiError::new(
            ErrorCode::BadRequest,
            error.to_string(),
            "send one JSON request per line, as the orb command does",
        )),
    };
    serde_json::to_string(&reply).unwrap_or_default()
}

/// What a request comes to once read against the state.
#[derive(Debug)]
pub(super) enum Action {
    /// Answered from the state alone.
    Answered(Reply),
    /// A new shell the sessions actor adds, then named and given its command
    /// on the screen loop.
    Add {
        add: ApiAdd,
        name: Option<String>,
        run: Option<String>,
    },
    /// A new session the sessions actor starts.
    Start(ApiStart),
    /// A change the screen loop applies.
    Change(Change),
    /// A close or settle the screen loop applies, replying with what it ended.
    Ending { change: Change, ended: Ended },
    /// Typing the screen loop does into a pane: a send, keys or a type.
    Pane(PaneRequest),
    /// A command line typed into a shell pane, then maybe waited on.
    Run(RunAsk),
    /// A wait until a pane shows a matching line.
    PaneWait(WaitAsk),
    /// A pane's text the screen loop reads, then cut to its first `head` and
    /// last `tail` lines.
    Read {
        pane: PaneId,
        lines: usize,
        head: Option<usize>,
        tail: Option<usize>,
    },
    /// An agent the screen loop types into a pane (an existing one, or one
    /// added first), then waits for.
    StartAgent {
        at: StartIn,
        line: String,
        name: Option<String>,
    },
    /// A prompt pasted into an agent, then maybe waited on.
    Prompt(PromptAsk),
    /// A wait until the pane's agent is idle or blocked.
    Wait {
        pane: PaneId,
        timeout: Option<Duration>,
    },
    /// What a pane's agent wrote since orb's prompt, else its last reply.
    Reply { pane: PaneId },
    /// An agent's full row, its status read through a just-sent prompt.
    Get { pane: PaneId, detail: AgentDetail },
    /// A plate add, update or close the sessions actor answers.
    Plate(PlateAsk),
}

/// Where `agent start` runs its agent.
#[derive(Debug)]
pub(super) enum StartIn {
    /// A shell pane that's already there.
    Pane(PaneId),
    /// A new shell the sessions actor adds first.
    New(ApiAdd),
}

/// What a close or settle replies once applied.
#[derive(Debug)]
pub(super) enum Ended {
    /// The panes a pane or tab close ended.
    Closed(ClosedResult),
    /// The session a settle ended, with its panes.
    Settled(SettledResult),
}

impl Ended {
    /// The reply carrying what was ended.
    fn reply(&self) -> Reply {
        match self {
            Self::Closed(closed) => Reply::ok(closed),
            Self::Settled(settled) => Reply::ok(settled),
        }
    }
}

/// The reply to `request`: worked out from the state (with no lock held
/// after), then carried out. Each change, and any request refused before it
/// runs, writes one `api` log line; lists, reads, waits, gets and replies that
/// are answered write none. A wait stops once `gone` says the caller hung up.
pub(super) fn answer(deps: &ControlDeps, request: &Request, gone: &dyn Fn() -> bool) -> Reply {
    let planned = plan(&deps.state.read(), request);
    let reply = match planned {
        Ok(Action::Answered(reply)) => return reply,
        Ok(Action::Read {
            pane,
            lines,
            head,
            tail,
        }) => {
            return Reply::from_result(deps.panes.text(pane, lines).map(|text| ReadResult {
                text: screen_text::cut(&screen_text::trim(&text), head, tail),
            }));
        }
        Ok(Action::PaneWait(ask)) => {
            return pane_ops::wait_for(&deps.panes, &ask, gone, PaneTiming::REAL);
        }
        Ok(Action::Get { pane, detail }) => return agent_ops::get(deps.turns(), pane, detail),
        Ok(Action::Wait { pane, timeout }) => {
            let poke = || agent_ops::poke(deps);
            return agent_ops::wait(deps.turns(), pane, timeout, &poke, gone, Timing::REAL);
        }
        Ok(Action::Reply { pane }) => return Reply::from_result(deps.turns().reply(pane)),
        Ok(Action::Add { add, name, run }) => pane_ops::add_pane(deps, add, name, run),
        Ok(Action::Start(start)) => start_session(deps, start),
        Ok(Action::Change(change)) => pane_ops::typed(deps, PaneRequest::Change(change)),
        Ok(Action::Ending { change, ended }) => match deps.panes.ask(PaneRequest::Change(change)) {
            Ok(_) => ended.reply(),
            Err(error) => Reply::err(error),
        },
        Ok(Action::Pane(request)) => pane_ops::typed(deps, request),
        Ok(Action::Run(ask)) => pane_ops::run(&deps.panes, &ask, gone, PaneTiming::REAL),
        Ok(Action::StartAgent { at, line, name }) => agent_ops::start_in(deps, at, line, name),
        Ok(Action::Prompt(ask)) => {
            let poke = || agent_ops::poke(deps);
            agent_ops::prompt_agent(&deps.panes, deps.turns(), ask, &poke, gone, Timing::REAL)
        }
        Ok(Action::Plate(ask)) => plate::ask(deps, ask),
        Err(error) => Reply::err(error),
    };
    deps.log.line("api", &log_text(request, &reply));
    reply
}

/// `<op> caller=<id> item=<id> pane=<id> session=<id> project=<id> ->
/// ok|<code>`, with only the IDs `request` carries, and the new item's an add
/// replies with: never a name, a title, a prompt or pasted text. A `failed`
/// reply adds its reason, which may quote git but never a prompt or pasted
/// text.
fn log_text(request: &Request, reply: &Reply) -> String {
    let mut fields = serde_json::to_value(request).unwrap_or_default();
    let added = reply.result.as_ref().and_then(|result| result.get("item"));
    if let (Some(fields), Some(item)) = (fields.as_object_mut(), added) {
        fields.entry("item").or_insert_with(|| item.clone());
    }
    let text = |key| match fields.get(key) {
        Some(serde_json::Value::String(op)) if key == "op" => Some(op.clone()),
        Some(serde_json::Value::Number(id)) => Some(format!("{key}={id}")),
        _ => None,
    };
    let result = match &reply.error {
        None => "ok".to_owned(),
        Some(error) if error.code == ErrorCode::Failed => format!("failed: {}", error.message),
        Some(error) => serde_json::to_value(error.code)
            .ok()
            .and_then(|code| code.as_str().map(str::to_owned))
            .unwrap_or_default(),
    };
    ["op", "caller", "item", "pane", "session", "project"]
        .into_iter()
        .filter_map(text)
        .chain([format!("-> {result}")])
        .collect::<Vec<_>>()
        .join(" ")
}

/// What `request` comes to, read from `app`, or why it's refused.
fn plan(app: &AppState, request: &Request) -> Result<Action, ApiError> {
    let caller = request.caller;
    match &request.op {
        Op::SessionList { .. }
        | Op::TabList { .. }
        | Op::PaneList { .. }
        | Op::AgentList { .. }
        | Op::ProjectList
        | Op::Here => Ok(Action::Answered(route(app, request))),
        Op::PaneSplit { .. }
        | Op::PaneStack { .. }
        | Op::PaneFloat { .. }
        | Op::TabCreate { .. } => pane_ops::add_io(app, &request.op, caller),
        Op::SessionCreate {
            project,
            worktree,
            name,
        } => session_create(app, *project, *worktree, name.clone(), caller),
        Op::PaneClose { pane } => {
            changing(app, Some(Target::Pane(*pane)), caller)?;
            Ok(Action::Ending {
                change: Change::ClosePane(*pane),
                ended: Ended::Closed(ClosedResult {
                    ended: ended(app, &[*pane]),
                }),
            })
        }
        Op::PaneRename { pane, name } => {
            changing(app, Some(Target::Pane(*pane)), caller)?;
            Ok(Action::Change(Change::RenamePane {
                pane: *pane,
                name: name.clone(),
            }))
        }
        Op::PaneFocus { pane } => {
            let session = changing(app, Some(Target::Pane(*pane)), caller)?.session;
            Ok(Action::Change(Change::Focus {
                session,
                pane: Some(*pane),
            }))
        }
        Op::TabClose { pane } => {
            let (_, pane) = tiled(app, *pane, caller, "a float isn't in a tab")?;
            Ok(Action::Ending {
                change: Change::CloseTab(pane),
                ended: Ended::Closed(ClosedResult {
                    ended: ended(app, &tab_panes(app, pane)),
                }),
            })
        }
        Op::SessionRename { session, name } => Ok(Action::Change(Change::RenameSession {
            session: changing_session(app, *session, caller)?,
            name: name.clone(),
        })),
        Op::SessionSettle { session } => {
            let session = changing_session(app, *session, caller)?;
            Ok(Action::Ending {
                change: Change::SettleSession(session),
                ended: Ended::Settled(settled(app, session)),
            })
        }
        Op::SessionFocus { session } => Ok(Action::Change(Change::Focus {
            session: changing_session(app, Some(*session), caller)?,
            pane: None,
        })),
        Op::PaneSend { .. }
        | Op::PaneKeys { .. }
        | Op::PaneType { .. }
        | Op::PaneRun { .. }
        | Op::PaneRead { .. }
        | Op::PaneWait { .. } => pane_ops::pane_io(app, &request.op, caller),
        Op::AgentStart { .. }
        | Op::AgentPrompt { .. }
        | Op::AgentWait { .. }
        | Op::AgentGet { .. }
        | Op::AgentReply { .. } => agent_ops::agent_io(app, &request.op, caller),
        Op::PlateList => {
            let now = SystemTime::now();
            Ok(Action::Answered(plate::list(
                app,
                now,
                app.zone.midnight(now),
            )))
        }
        Op::PlateAdd { .. } | Op::PlateUpdate { .. } | Op::PlateDone { .. } => {
            plate::plan(app, &request.op, caller).map(Action::Plate)
        }
    }
}

/// What a change is about, as [`target::resolve`] finds it; `settled` when
/// its session is settled.
pub(super) fn changing(
    app: &AppState,
    target: Option<Target>,
    caller: Option<PaneId>,
) -> Result<Resolved, ApiError> {
    let resolved = target::resolve(app, target, caller)?;
    validator::not_settled(app, resolved.session)?;
    Ok(resolved)
}

/// The session a change names, else the caller's; `settled` when it is.
pub(super) fn changing_session(
    app: &AppState,
    session: Option<SessionId>,
    caller: Option<PaneId>,
) -> Result<SessionId, ApiError> {
    changing(app, session.map(Target::Session), caller).map(|resolved| resolved.session)
}

/// Project `project`, else the caller's session's; `not_found` when it's gone
/// or removed.
fn project_for(
    app: &AppState,
    project: Option<ProjectId>,
    caller: Option<PaneId>,
) -> Result<ProjectId, ApiError> {
    let id = match project {
        Some(id) => Some(id),
        None => {
            let session = target::resolve(app, None, caller)?.session;
            app.sessions.session(session).map(|session| session.project)
        }
    };
    id.and_then(|id| app.sessions.project(id))
        .filter(|project| !project.removed)
        .map(|project| project.id)
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::NotFound,
                "no such project",
                "orb project list shows the projects",
            )
        })
}

/// A new session of `project` (else the caller's session's). Without
/// `project`, a caller in a fresh world gets one in its own folder, no
/// worktree; `--worktree` there, or `project` naming a fresh world, is a
/// `bad_request`. A caller in a colony or a colonist makes it that
/// session's colonist.
fn session_create(
    app: &AppState,
    project: Option<ProjectId>,
    worktree: bool,
    name: Option<String>,
    caller: Option<PaneId>,
) -> Result<Action, ApiError> {
    let named = project.is_some();
    let project = project_for(app, project, caller)?;
    let asking = target::resolve(app, None, caller)
        .ok()
        .and_then(|resolved| app.sessions.session(resolved.session));
    let dir = match app
        .sessions
        .project(project)
        .filter(|project| project.kind.is_fresh_world())
    {
        None => None,
        Some(world) if named => {
            return Err(ApiError::new(
                ErrorCode::BadRequest,
                format!(
                    "can't start a session in fresh world {}; make its colonies with <C-g> c",
                    world.title
                ),
                "run orb session create without --project from a pane of the world's colony",
            ));
        }
        Some(world) if worktree => {
            return Err(ApiError::new(
                ErrorCode::BadRequest,
                format!(
                    "fresh world {}'s folders aren't git repositories; for a worktree, name a project with --project",
                    world.title
                ),
                "name a git project with --project <id>; orb project list shows the projects",
            ));
        }
        Some(_) => Some(asking.map(|session| session.dir.clone()).ok_or_else(|| {
            ApiError::new(
                ErrorCode::NotFound,
                "no such session",
                "run orb session create from inside an orb pane",
            )
        })?),
    };
    let parent = asking
        .filter(|session| session.kind == SessionKind::Colony || session.parent.is_some())
        .map(|session| session.id);
    Ok(Action::Start(ApiStart {
        project,
        worktree,
        name,
        parent,
        dir,
    }))
}

/// Pane `pane` (else the caller's) and its session; `bad_request` saying
/// `why` when it's a float.
pub(super) fn tiled(
    app: &AppState,
    pane: Option<PaneId>,
    caller: Option<PaneId>,
    why: &str,
) -> Result<(SessionId, PaneId), ApiError> {
    match changing(app, pane.map(Target::Pane), caller)? {
        Resolved {
            session,
            pane: Some(at),
        } if !app
            .layouts
            .get(session)
            .is_some_and(|layout| layout.floats().holds(at)) =>
        {
            Ok((session, at))
        }
        _ => Err(ApiError::new(
            ErrorCode::BadRequest,
            why,
            r#"orb pane list marks floating panes with "float": true"#,
        )),
    }
}

/// Why a float can't be split or stacked.
pub(super) const SPLIT_FLOAT: &str = "floats can't be split or stacked";

/// Asks the sessions actor for the shell `add` describes; `failed` when it can't.
pub(super) fn added(deps: &ControlDeps, add: ApiAdd) -> Result<PaneId, ApiError> {
    deps.runtime
        .block_on(async { deps.sessions.ask(add).await })
        .map_err(|error| ApiError::failed(error.to_string()))
}

/// Asks the sessions actor to start `start` and replies with the new session
/// and its first pane, or `starting` when the actor says another session is
/// starting.
fn start_session(deps: &ControlDeps, start: ApiStart) -> Reply {
    match deps
        .runtime
        .block_on(async { deps.sessions.ask(start).await })
    {
        Ok((session, pane)) => {
            if let Err(error) = deps.panes.ask(PaneRequest::Change(Change::Attach(session))) {
                deps.log.line(
                    "control",
                    &format!("couldn't run session {}: {}", session.0, error.message),
                );
            }
            Reply::ok(&StartResult { session, pane })
        }
        Err(SendError::HandlerError(ApiStartError::Starting)) => Reply::err(ApiError::new(
            ErrorCode::Starting,
            ApiStartError::Starting.to_string(),
            "try again in a few seconds",
        )),
        Err(error) => Reply::err(ApiError::failed(error.to_string())),
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate setup failures with `?` and assert on the outcome"
)]
mod tests {
    use std::ffi::OsString;
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::thread;
    use std::time::Duration;

    use error_stack::{Report, ResultExt};
    use ratatui::crossterm::event::KeyCode;
    use tempfile::TempDir;

    use super::{
        Action, ControlServerError, Ended, StartIn, hung_up, log_text, open, plan, reply_line,
        serve, watch_for_hang_up,
    };
    use crate::feat::control::agent_ops::START_FLOAT;
    use crate::feat::control::change::Change;
    use crate::feat::control::client;
    use crate::feat::control::lists::route;
    use crate::feat::control::pane_control::PaneRequest;
    use crate::feat::control::pane_ops::follow_ups;
    use crate::feat::control::protocol::{
        AgentKind, ApiError, ClosedResult, Direction, Done, EndedRow, ErrorCode, ItemResult, Op,
        Reply, Request, SettledResult,
    };
    use crate::feat::layout::state::test_entry;
    use crate::feat::layout::tree::Split;
    use crate::feat::sessions::sessions_actor::{ApiAdd, ApiPlace};
    use crate::feat::sessions::state::{AgentStatus, ProjectKind, SessionKind};
    use crate::ids::{PaneId, PlateItemId, SessionId};
    use crate::intent_handler::test_support::{agent, named, settle, state_with, stopped};
    use crate::{AppState, Log, State};

    /// Listens on a socket in a fresh temp dir, answering from `app`.
    fn listening(app: AppState) -> Result<(TempDir, PathBuf), Report<ControlServerError>> {
        let dir = tempfile::tempdir().change_context(ControlServerError)?;
        let socket = dir.path().join("orb.sock");
        let state = State::new(app);
        open(&socket, Log::default(), move |line, _gone| {
            reply_line(line, |request| route(&state.read(), request))
        })?;
        Ok((dir, socket))
    }

    #[rstest::rstest]
    fn socket_is_created_with_mode_0600() -> Result<(), Report<ControlServerError>> {
        // Given / When orb listens on a socket.
        let (_dir, socket) = listening(AppState::default())?;

        // Then only the user can open it.
        let mode = std::fs::metadata(&socket)
            .change_context(ControlServerError)?
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "the socket is private to the user");
        Ok(())
    }

    #[rstest::rstest]
    fn pane_list_over_the_socket_marks_the_callers_pane() -> Result<(), Report<ControlServerError>>
    {
        // Given orb listening with session 1, pane 1, agent 1.
        let (_dir, socket) = listening(state_with(vec![agent(1, AgentStatus::Idle)], 1))?;
        let (mut out, mut err) = (Vec::new(), Vec::new());

        // When pane 1 runs `pane list`.
        let words: Vec<OsString> = ["pane", "list"].iter().map(OsString::from).collect();
        client::run(&words, Some(PaneId(1)), &socket, &mut out, &mut err);

        // Then it gets one pane, its own, marked as the caller, with agent 1.
        let reply: serde_json::Value =
            serde_json::from_slice(&out).change_context(ControlServerError)?;
        let field = |pane: &serde_json::Value, key: &str| pane.get(key).cloned();
        let summary: Vec<_> = reply
            .get("panes")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .map(|pane| {
                (
                    field(pane, "id").and_then(|v| v.as_i64()),
                    field(pane, "caller").and_then(|v| v.as_bool()),
                    field(pane, "agent").and_then(|v| v.as_i64()),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![(Some(1), Some(true), Some(1))],
            "the caller's pane is listed and marked, err: {}",
            String::from_utf8_lossy(&err)
        );
        Ok(())
    }

    #[rstest::rstest]
    fn split_of_a_float_is_a_bad_request() {
        // Given session 1 with float pane 2.
        let mut app = state_with(vec![agent(1, AgentStatus::Idle)], 1);
        app.layouts.add_float_hidden(SessionId(1), test_entry(2));

        // When planning a split of pane 2.
        let request = Request {
            caller: None,
            op: Op::PaneSplit {
                pane: Some(PaneId(2)),
                direction: Direction::Right,
                name: None,
                run: None,
                cwd: None,
            },
        };
        let planned = plan(&app, &request);

        // Then it is refused with bad_request.
        assert_eq!(
            planned.map(drop).map_err(|error| error.code),
            Err(ErrorCode::BadRequest),
            "a float can't be split"
        );
    }

    #[rstest::rstest]
    #[case::pane_split(Op::PaneSplit { pane: None, direction: Direction::Right, name: None, run: None, cwd: None })]
    #[case::pane_stack(Op::PaneStack { pane: None, name: None, run: None, cwd: None })]
    #[case::pane_float(Op::PaneFloat { session: None, name: None, run: None, cwd: None })]
    #[case::tab_create(Op::TabCreate { session: None, name: None, run: None, cwd: None })]
    #[case::tab_close(Op::TabClose { pane: None })]
    #[case::pane_close(Op::PaneClose { pane: PaneId(1) })]
    #[case::pane_rename(Op::PaneRename { pane: PaneId(1), name: "x".into() })]
    #[case::pane_focus(Op::PaneFocus { pane: PaneId(1) })]
    #[case::session_rename(Op::SessionRename { session: None, name: "x".into() })]
    #[case::session_settle(Op::SessionSettle { session: None })]
    #[case::session_focus(Op::SessionFocus { session: SessionId(1) })]
    #[case::pane_send(Op::PaneSend { pane: PaneId(1), text: "x".into() })]
    #[case::pane_keys(Op::PaneKeys { pane: PaneId(1), keys: vec!["enter".into()] })]
    #[case::pane_type(Op::PaneType { pane: PaneId(1), text: "x".into() })]
    #[case::pane_run(Op::PaneRun { pane: PaneId(1), command: "x".into(), wait: false, timeout: None, force: false, tail: None })]
    #[case::pane_run_wait(Op::PaneRun { pane: PaneId(1), command: "x".into(), wait: true, timeout: Some(5), force: false, tail: None })]
    #[case::pane_read(Op::PaneRead {
        pane: None,
        lines: 0,
        head: None,
        tail: None,
    })]
    #[case::pane_wait(Op::PaneWait { pane: PaneId(1), text: Some("x".into()), regex: None, timeout: None, lines: None })]
    #[case::agent_start(Op::AgentStart { pane: None, kind: AgentKind::Claude, name: None, args: vec![], split: None, tab: false })]
    #[case::agent_start_split(Op::AgentStart { pane: None, kind: AgentKind::Claude, name: None, args: vec![], split: Some(Direction::Right), tab: false })]
    #[case::agent_start_tab(Op::AgentStart { pane: None, kind: AgentKind::Claude, name: None, args: vec![], split: None, tab: true })]
    #[case::agent_prompt(Op::AgentPrompt { pane: PaneId(1), text: "x".into(), wait: false, timeout: None })]
    #[case::agent_wait(Op::AgentWait { pane: PaneId(1), timeout: None })]
    fn request_on_a_settled_session_is_refused(#[case] op: Op) {
        // Given session 1, settled, with pane 1.
        let app = settle(state_with(vec![stopped(1)], 1), &[1]);

        // When planning the request from pane 1.
        let planned = plan(
            &app,
            &Request {
                caller: Some(PaneId(1)),
                op,
            },
        );

        // Then it is refused with settled.
        assert_eq!(
            planned.map(drop).map_err(|error| error.code),
            Err(ErrorCode::Settled),
            "nothing in a settled session changes"
        );
    }

    #[rstest::rstest]
    fn log_line_names_ids_and_never_the_name() {
        // Given pane 4 renaming session 2 to a private name, refused.
        let request = Request {
            caller: Some(PaneId(4)),
            op: Op::SessionRename {
                session: Some(SessionId(2)),
                name: "secret plan".into(),
            },
        };
        let reply = Reply::err(ApiError::new(
            ErrorCode::Settled,
            "session is settled",
            "orb session list shows open ones",
        ));

        // When writing its log line.
        let text = log_text(&request, &reply);

        // Then it has the op, the IDs and the code, and not the name.
        assert_eq!(
            text, "session.rename caller=4 session=2 -> settled",
            "the log line carries IDs only"
        );
    }

    #[rstest::rstest]
    fn plate_update_log_line_carries_the_item_and_no_title() {
        // Given pane 4 retitling plate item 7.
        let request = Request {
            caller: Some(PaneId(4)),
            op: Op::PlateUpdate {
                item: PlateItemId(7),
                title: Some("secret title".into()),
                verb: Some("review".into()),
                what: Some("secret text".into()),
                links: None,
                urgent: None,
                waiting: None,
            },
        };

        // When writing its log line.
        let text = log_text(&request, &Reply::ok(&Done {}));

        // Then it has the op, the caller and the item, and no title or text.
        assert_eq!(
            text, "plate.update caller=4 item=7 -> ok",
            "a plate change logs its item, never its words"
        );
    }

    #[rstest::rstest]
    fn plate_add_log_line_carries_the_new_item() {
        // Given pane 4 adding an item that orb saved as item 12.
        let request = Request {
            caller: Some(PaneId(4)),
            op: Op::PlateAdd {
                title: "secret title".into(),
                verb: "review".into(),
                what: None,
                links: vec![],
                urgent: false,
                waiting: false,
                pane: None,
            },
        };
        let reply = Reply::ok(&ItemResult {
            item: PlateItemId(12),
        });

        // When writing its log line.
        let text = log_text(&request, &reply);

        // Then it names the new item from the reply, and no title.
        assert_eq!(
            text, "plate.add caller=4 item=12 -> ok",
            "an add logs the item it made"
        );
    }

    #[rstest::rstest]
    fn plate_list_is_planned_as_a_read() {
        // Given session 1 with agent 1.
        let app = state_with(vec![agent(1, AgentStatus::Idle)], 1);

        // When planning `plate list` from outside orb.
        let planned = plan(
            &app,
            &Request {
                caller: None,
                op: Op::PlateList,
            },
        );

        // Then it's answered from the state, with no ask.
        assert!(
            matches!(planned, Ok(Action::Answered(_))),
            "plate list is a read, got {planned:?}"
        );
    }

    #[rstest::rstest]
    fn log_line_of_a_failure_carries_its_reason() {
        // Given a pane close that failed because the screen didn't answer.
        let request = Request {
            caller: None,
            op: Op::PaneClose { pane: PaneId(3) },
        };
        let reply = Reply::err(ApiError::failed("orb's screen didn't answer"));

        // When writing its log line.
        let text = log_text(&request, &reply);

        // Then the reason follows the code.
        assert_eq!(
            text, "pane.close pane=3 -> failed: orb's screen didn't answer",
            "a failure logs why"
        );
    }

    #[rstest::rstest]
    fn log_line_of_a_success_ends_in_ok() {
        // Given a pane close from outside orb that worked.
        let request = Request {
            caller: None,
            op: Op::PaneClose { pane: PaneId(3) },
        };

        // When writing its log line.
        let text = log_text(&request, &Reply::ok(&Done {}));

        // Then it ends in ok.
        assert_eq!(text, "pane.close pane=3 -> ok", "a success logs ok");
    }

    #[rstest::rstest]
    fn unknown_key_name_is_a_bad_request() {
        // Given live session 1 with pane 1.
        let app = state_with(vec![agent(1, AgentStatus::Idle)], 1);

        // When planning keys `enter` then an unknown `nope` for pane 1.
        let request = Request {
            caller: None,
            op: Op::PaneKeys {
                pane: PaneId(1),
                keys: vec!["enter".into(), "nope".into()],
            },
        };
        let planned = plan(&app, &request);

        // Then it is refused with bad_request before anything reaches the
        // screen loop.
        assert_eq!(
            planned.map(drop).map_err(|error| error.code),
            Err(ErrorCode::BadRequest),
            "an unknown key name sends nothing"
        );
    }

    #[rstest::rstest]
    fn pane_wait_with_a_bad_regex_is_a_bad_request() {
        // Given live session 1 with pane 1.
        let app = state_with(vec![agent(1, AgentStatus::Idle)], 1);

        // When planning a wait in pane 1 for the pattern `(ok`, a group
        // left open.
        let request = Request {
            caller: None,
            op: Op::PaneWait {
                pane: PaneId(1),
                text: None,
                regex: Some("(ok".into()),
                timeout: None,
                lines: None,
            },
        };
        let planned = plan(&app, &request).map(drop);

        // Then it is refused with bad_request naming the pattern.
        assert_eq!(
            planned.map_err(|error| (error.code, error.message)),
            Err((
                ErrorCode::BadRequest,
                "`(ok` isn't a pattern: error: unclosed group".to_owned()
            )),
            "a pattern that doesn't compile waits on nothing"
        );
    }

    #[rstest::rstest]
    #[case::neither(None, None)]
    #[case::both(Some("a"), Some("b"))]
    fn pane_wait_needs_text_or_regex(#[case] text: Option<&str>, #[case] regex: Option<&str>) {
        // Given live session 1 with pane 1.
        let app = state_with(vec![agent(1, AgentStatus::Idle)], 1);

        // When planning a wait in pane 1 without exactly one of text and
        // regex.
        let request = Request {
            caller: None,
            op: Op::PaneWait {
                pane: PaneId(1),
                text: text.map(str::to_owned),
                regex: regex.map(str::to_owned),
                timeout: None,
                lines: None,
            },
        };
        let planned = plan(&app, &request).map(drop);

        // Then it is refused with bad_request.
        assert_eq!(
            planned.map_err(|error| error.code),
            Err(ErrorCode::BadRequest),
            "a wait needs one thing to look for"
        );
    }

    #[rstest::rstest]
    fn keys_of_single_characters_are_planned() {
        // Given live session 1 with pane 1.
        let app = state_with(vec![agent(1, AgentStatus::Idle)], 1);

        // When planning `esc : w q enter` for pane 1.
        let request = Request {
            caller: None,
            op: Op::PaneKeys {
                pane: PaneId(1),
                keys: ["esc", ":", "w", "q", "enter"].map(Into::into).to_vec(),
            },
        };
        let planned = plan(&app, &request);

        // Then the five keys reach the pane in order, `:` as its character.
        let codes = match planned {
            Ok(Action::Pane(PaneRequest::Keys { pane, keys })) => {
                Some((pane, keys.iter().map(|key| key.code).collect::<Vec<_>>()))
            }
            _ => None,
        };
        assert_eq!(
            codes,
            Some((
                PaneId(1),
                vec![
                    KeyCode::Esc,
                    KeyCode::Char(':'),
                    KeyCode::Char('w'),
                    KeyCode::Char('q'),
                    KeyCode::Enter,
                ]
            )),
            "single characters are key presses"
        );
    }

    #[rstest::rstest]
    fn unknown_key_hint_points_at_pane_type() {
        // Given live session 1 with pane 1.
        let app = state_with(vec![agent(1, AgentStatus::Idle)], 1);

        // When planning the unknown key `nope` for pane 1.
        let request = Request {
            caller: None,
            op: Op::PaneKeys {
                pane: PaneId(1),
                keys: vec!["nope".into()],
            },
        };
        let planned = plan(&app, &request);

        // Then the hint names the command that types text into pane 1.
        let hint = planned.map(drop).err().and_then(|error| error.hint);
        assert!(
            hint.as_deref()
                .is_some_and(|hint| hint.contains("orb pane type 1 <text>")),
            "the hint should point at pane type for pane 1, got {hint:?}"
        );
    }

    #[rstest::rstest]
    fn pane_type_is_planned_as_raw_typing() {
        // Given live session 1 with pane 1.
        let app = state_with(vec![agent(1, AgentStatus::Idle)], 1);

        // When planning `:wq` typed into pane 1.
        let request = Request {
            caller: None,
            op: Op::PaneType {
                pane: PaneId(1),
                text: ":wq".into(),
            },
        };
        let planned = plan(&app, &request);

        // Then the screen loop is asked to type the text as is.
        assert!(
            matches!(
                &planned,
                Ok(Action::Pane(PaneRequest::Type { pane, text }))
                    if *pane == PaneId(1) && text == ":wq"
            ),
            "pane type plans raw typing, got {planned:?}"
        );
    }

    /// The add `kind` names (`split`, `stack`, `float`, else a new tab) from
    /// pane 1 or session 1, named `server`, running `just dev` in `cwd`.
    fn add_op(kind: &str, cwd: PathBuf) -> Op {
        let (name, run, cwd) = (Some("server".into()), Some("just dev".into()), Some(cwd));
        match kind {
            "split" => Op::PaneSplit {
                pane: Some(PaneId(1)),
                direction: Direction::Right,
                name,
                run,
                cwd,
            },
            "stack" => Op::PaneStack {
                pane: Some(PaneId(1)),
                name,
                run,
                cwd,
            },
            "float" => Op::PaneFloat {
                session: Some(SessionId(1)),
                name,
                run,
                cwd,
            },
            _ => Op::TabCreate {
                session: Some(SessionId(1)),
                name,
                run,
                cwd,
            },
        }
    }

    #[rstest::rstest]
    #[case::pane_split("split")]
    #[case::pane_stack("stack")]
    #[case::pane_float("float")]
    #[case::tab_create("tab")]
    fn add_carries_its_name_run_and_folder(
        #[case] kind: &str,
    ) -> Result<(), Report<ControlServerError>> {
        // Given live session 1 with pane 1, and a folder.
        let app = state_with(vec![agent(1, AgentStatus::Idle)], 1);
        let folder = tempfile::tempdir().change_context(ControlServerError)?;

        // When planning the add named `server`, running `just dev` there.
        let request = Request {
            caller: None,
            op: add_op(kind, folder.path().to_path_buf()),
        };
        let planned = plan(&app, &request);

        // Then the sessions actor is asked for a shell in that folder, and
        // the name and the command come along.
        assert!(
            matches!(
                &planned,
                Ok(Action::Add { add, name, run })
                    if add.session == SessionId(1)
                        && add.dir.as_deref() == Some(folder.path())
                        && name.as_deref() == Some("server")
                        && run.as_deref() == Some("just dev")
            ),
            "the add carries its folder, name and run, got {planned:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn add_in_a_missing_folder_is_a_bad_request() -> Result<(), Report<ControlServerError>> {
        // Given live session 1 with pane 1, and a folder that doesn't exist.
        let app = state_with(vec![agent(1, AgentStatus::Idle)], 1);
        let folder = tempfile::tempdir().change_context(ControlServerError)?;

        // When planning a split there.
        let request = Request {
            caller: None,
            op: add_op("split", folder.path().join("nope")),
        };
        let planned = plan(&app, &request);

        // Then it is refused before anything is added.
        assert_eq!(
            planned.map(drop).map_err(|error| error.code),
            Err(ErrorCode::BadRequest),
            "a missing folder is refused"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn add_in_a_relative_folder_is_a_bad_request() {
        // Given live session 1 with pane 1.
        let app = state_with(vec![agent(1, AgentStatus::Idle)], 1);

        // When planning a new tab in a relative folder.
        let request = Request {
            caller: None,
            op: add_op("tab", PathBuf::from("rel")),
        };
        let planned = plan(&app, &request);

        // Then it is refused before anything is added.
        assert_eq!(
            planned.map(drop).map_err(|error| error.code),
            Err(ErrorCode::BadRequest),
            "a relative folder is refused"
        );
    }

    #[rstest::rstest]
    #[case::both(
        Some("server"),
        Some("just dev"),
        vec![
            PaneRequest::Change(Change::RenamePane { pane: PaneId(5), name: "server".into() }),
            PaneRequest::Run { pane: PaneId(5), line: "just dev".into() },
        ],
    )]
    #[case::neither(None, None, vec![])]
    fn follow_ups_rename_then_run(
        #[case] name: Option<&str>,
        #[case] run: Option<&str>,
        #[case] expected: Vec<PaneRequest>,
    ) {
        // Given / When the follow-ups of new pane 5 with this name and run.
        let requests = follow_ups(PaneId(5), name.map(str::to_owned), run.map(str::to_owned));

        // Then the rename comes first, then the run.
        assert_eq!(requests, expected, "the rename goes before the run");
    }

    #[rstest::rstest]
    fn log_line_of_a_send_leaves_out_the_text() {
        // Given pane 1 pasting private text into pane 3, done.
        let request = Request {
            caller: Some(PaneId(1)),
            op: Op::PaneSend {
                pane: PaneId(3),
                text: "secret".into(),
            },
        };

        // When writing its log line.
        let text = log_text(&request, &Reply::ok(&Done {}));

        // Then it has the op and the IDs, and not the text.
        assert_eq!(
            text, "pane.send caller=1 pane=3 -> ok",
            "typed text never reaches the log"
        );
    }

    #[rstest::rstest]
    fn agent_get_on_a_settled_session_is_answered() {
        // Given session 1, settled, with stopped agent 1 in pane 1.
        let app = settle(state_with(vec![stopped(1)], 1), &[1]);

        // When planning `agent get` for pane 1.
        let request = Request {
            caller: None,
            op: Op::AgentGet { pane: PaneId(1) },
        };
        let planned = plan(&app, &request);

        // Then it is answered with the agent's row.
        assert!(
            matches!(
                &planned,
                Ok(Action::Get {
                    pane: PaneId(1),
                    ..
                })
            ),
            "a settled session's agent can still be read, got {planned:?}"
        );
    }

    #[rstest::rstest]
    fn log_line_of_a_prompt_leaves_out_the_text() {
        // Given pane 1 prompting the agent in pane 3 with private text, done.
        let request = Request {
            caller: Some(PaneId(1)),
            op: Op::AgentPrompt {
                pane: PaneId(3),
                text: "secret".into(),
                wait: true,
                timeout: Some(9),
            },
        };

        // When writing its log line.
        let text = log_text(&request, &Reply::ok(&Done {}));

        // Then it has the op and the IDs, and not the text.
        assert_eq!(
            text, "agent.prompt caller=1 pane=3 -> ok",
            "a prompt's text never reaches the log"
        );
    }

    /// The parent `orb session create` gives the new session when pane 1 of
    /// session 1, of `kind` and `parent`, asks.
    fn parent_from(kind: SessionKind, parent: Option<SessionId>) -> Option<SessionId> {
        let mut app = state_with(vec![agent(1, AgentStatus::Idle)], 1);
        if let Some(session) = app.sessions.sessions.first_mut() {
            session.kind = kind;
            session.parent = parent;
        }
        let planned = plan(
            &app,
            &Request {
                caller: Some(PaneId(1)),
                op: Op::SessionCreate {
                    project: None,
                    worktree: false,
                    name: None,
                },
            },
        );
        match planned {
            Ok(Action::Start(start)) => start.parent,
            _ => None,
        }
    }

    #[rstest::rstest]
    fn session_create_from_a_colony_pane_makes_its_colonist() {
        // Given session 1, a colony, with pane 1.
        // When pane 1 runs `session create`.
        let parent = parent_from(SessionKind::Colony, None);

        // Then the new session's parent is session 1.
        assert_eq!(
            parent,
            Some(SessionId(1)),
            "a colony's pane makes a colonist"
        );
    }

    #[rstest::rstest]
    fn session_create_from_a_colonist_pane_makes_its_colonist() {
        // Given session 1, a plain colonist of session 9, with pane 1.
        // When pane 1 runs `session create`.
        let parent = parent_from(SessionKind::Plain, Some(SessionId(9)));

        // Then the new session's parent is session 1.
        assert_eq!(
            parent,
            Some(SessionId(1)),
            "a colonist's pane makes its own colonist"
        );
    }

    #[rstest::rstest]
    fn session_create_from_a_normal_pane_is_top_level() {
        // Given session 1, plain and top level, with pane 1.
        // When pane 1 runs `session create`.
        let parent = parent_from(SessionKind::Plain, None);

        // Then the new session has no parent.
        assert_eq!(
            parent, None,
            "a normal session's pane makes a top-level session"
        );
    }

    /// What `orb session create` plans when pane 1 of session 1, a colony
    /// in a project of `kind`, asks with `worktree` and, when `named`, the
    /// project's id; and session 1's folder.
    fn created_from_colony(
        kind: ProjectKind,
        worktree: bool,
        named: bool,
    ) -> (Result<Action, ApiError>, Option<PathBuf>) {
        let mut app = state_with(vec![agent(1, AgentStatus::Idle)], 1);
        let project = app.sessions.projects.first_mut().map(|project| {
            project.kind = kind;
            project.id
        });
        let dir = app.sessions.sessions.first_mut().map(|session| {
            session.kind = SessionKind::Colony;
            session.dir.clone()
        });
        let planned = plan(
            &app,
            &Request {
                caller: Some(PaneId(1)),
                op: Op::SessionCreate {
                    project: project.filter(|_| named),
                    worktree,
                    name: None,
                },
            },
        );
        (planned, dir)
    }

    #[rstest::rstest]
    #[case::research(ProjectKind::Research)]
    #[case::learn(ProjectKind::Learn)]
    #[case::fresh_world(ProjectKind::FreshWorld)]
    fn session_create_naming_a_fresh_world_is_a_bad_request(#[case] kind: ProjectKind) {
        // Given session 1, a colony with pane 1, in a fresh world of `kind`.
        // When pane 1 runs `session create --project <the world>`.
        let (planned, _) = created_from_colony(kind, false, true);

        // Then it is refused with bad_request.
        assert_eq!(
            planned.map(drop).map_err(|error| error.code),
            Err(ErrorCode::BadRequest),
            "a fresh world named with --project takes colonies, not API sessions"
        );
    }

    #[rstest::rstest]
    #[case::research(ProjectKind::Research)]
    #[case::learn(ProjectKind::Learn)]
    #[case::fresh_world(ProjectKind::FreshWorld)]
    fn session_create_in_a_fresh_world_starts_in_the_callers_folder(#[case] kind: ProjectKind) {
        // Given session 1, a colony with pane 1, in a fresh world of `kind`.
        // When pane 1 runs `session create` without --project.
        let (planned, dir) = created_from_colony(kind, false, false);

        // Then the new session starts in session 1's folder.
        let start_dir = match planned {
            Ok(Action::Start(start)) => start.dir,
            _ => None,
        };
        assert_eq!(start_dir, dir, "a helper starts in its colony's folder");
    }

    #[rstest::rstest]
    fn session_create_in_a_fresh_world_is_the_callers_colonist() {
        // Given session 1, a colony with pane 1, in a fresh world.
        // When pane 1 runs `session create` without --project.
        let (planned, _) = created_from_colony(ProjectKind::Research, false, false);

        // Then the new session's parent is session 1.
        let parent = match planned {
            Ok(Action::Start(start)) => start.parent,
            _ => None,
        };
        assert_eq!(parent, Some(SessionId(1)), "a helper sits under its colony");
    }

    #[rstest::rstest]
    fn session_create_with_worktree_in_a_fresh_world_is_a_bad_request() {
        // Given session 1, a colony with pane 1, in a fresh world.
        // When pane 1 runs `session create --worktree` without --project.
        let (planned, _) = created_from_colony(ProjectKind::Research, true, false);

        // Then it is refused with bad_request.
        assert_eq!(
            planned.map(drop).map_err(|error| error.code),
            Err(ErrorCode::BadRequest),
            "a fresh world's folders have no git for a worktree"
        );
    }

    #[rstest::rstest]
    fn session_create_in_a_normal_project_has_no_dir() {
        // Given session 1, a colony with pane 1, in a normal project.
        // When pane 1 runs `session create`.
        let (planned, _) = created_from_colony(ProjectKind::Normal, false, false);

        // Then the new session gets no folder of its own.
        let has_dir = match planned {
            Ok(Action::Start(start)) => start.dir.is_some(),
            _ => true,
        };
        assert!(
            !has_dir,
            "a normal project's session starts in its checkout"
        );
    }

    #[rstest::rstest]
    fn silent_connection_is_closed_after_the_read_limit() -> io::Result<()> {
        // Given a connection served with a 50 ms read limit.
        let (mut client, server) = UnixStream::pair()?;
        thread::spawn(move || {
            serve(
                server,
                Duration::from_millis(50),
                &|_: &str, _: &dyn Fn() -> bool| String::new(),
            );
        });

        // When the client sends nothing.
        let read = client.read(&mut [0; 1])?;

        // Then orb closes the connection without a reply.
        assert_eq!(read, 0, "a silent connection should be closed");
        Ok(())
    }

    #[rstest::rstest]
    fn answer_sees_a_caller_that_hung_up() -> io::Result<()> {
        // Given a connection whose answer runs until its caller hangs up.
        let (mut client, server) = UnixStream::pair()?;
        let served = thread::spawn(move || {
            serve(server, Duration::from_secs(5), &|_: &str,
                                                    gone: &dyn Fn()
                -> bool| {
                while !gone() {
                    thread::sleep(Duration::from_millis(1));
                }
                String::new()
            });
        });

        // When the client sends a line and hangs up.
        writeln!(client, "{{}}")?;
        drop(client);

        // Then the answer stops and serve returns.
        assert!(
            served.join().is_ok(),
            "serve should return once the caller left"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn hang_up_is_seen_when_the_caller_left_right_after_its_line() -> io::Result<()> {
        // Given a request line that was read, and then a caller that hung up.
        let (mut client, server) = UnixStream::pair()?;
        writeln!(client, "{{}}")?;
        let mut line = String::new();
        BufReader::new(&server).read_line(&mut line)?;
        drop(client);

        // When starting to watch for the hang-up.
        let watching = watch_for_hang_up(&server);

        // Then the watch starts and sees the caller gone.
        assert!(
            watching && hung_up(&server),
            "a caller that left before the watch started should read as gone"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn hang_up_is_not_seen_while_the_caller_waits() -> io::Result<()> {
        // Given a request line that was read from a caller still connected.
        let (mut client, server) = UnixStream::pair()?;
        writeln!(client, "{{}}")?;
        let mut line = String::new();
        BufReader::new(&server).read_line(&mut line)?;

        // When watching for the hang-up.
        let watching = watch_for_hang_up(&server);

        // Then the caller reads as still there.
        assert!(
            watching && !hung_up(&server),
            "a caller still connected should not read as gone"
        );
        drop(client);
        Ok(())
    }

    #[rstest::rstest]
    fn home_names_the_callers_pane_tab_and_session() -> Result<(), Report<ControlServerError>> {
        // Given orb listening with session 1, pane 1 in tab 1, agent 1.
        let (_dir, socket) = listening(state_with(vec![agent(1, AgentStatus::Idle)], 1))?;
        let (mut out, mut err) = (Vec::new(), Vec::new());

        // When bare orb runs in pane 1.
        let shown = client::home(PaneId(1), &socket, &mut out, &mut err);

        // Then it exits 0 naming the pane, tab and session, then where help is.
        let out = String::from_utf8_lossy(&out);
        assert!(
            shown == Some(0)
                && out.starts_with("orb: you're in pane 1 (tab 1) of session 1 ")
                && out.ends_with("help: orb help · orb pane --help · orb agent --help\n"),
            "the home view says where the caller is, got {shown:?}: {out} err: {}",
            String::from_utf8_lossy(&err)
        );
        Ok(())
    }

    #[rstest::rstest]
    fn here_outside_a_pane_is_no_target() {
        // Given session 1 with agent 1.
        let app = state_with(vec![agent(1, AgentStatus::Idle)], 1);

        // When planning `orb.here` from outside orb.
        let planned = plan(
            &app,
            &Request {
                caller: None,
                op: Op::Here,
            },
        );

        // Then the reply refuses it with no_target.
        let code = match &planned {
            Ok(Action::Answered(reply)) => reply.error.as_ref().map(|error| error.code),
            _ => None,
        };
        assert_eq!(
            code,
            Some(ErrorCode::NoTarget),
            "outside a pane there's no place to name, got {planned:?}"
        );
    }

    /// Session 1 with idle agent 1 in pane 1, and float pane 2.
    fn with_float() -> AppState {
        let mut app = state_with(vec![agent(1, AgentStatus::Idle)], 1);
        app.layouts.add_float_hidden(SessionId(1), test_entry(2));
        app
    }

    #[rstest::rstest]
    #[case::settled(
        settle(state_with(vec![stopped(1)], 1), &[1]),
        Some(PaneId(1)),
        Op::PaneClose { pane: PaneId(1) },
        ErrorCode::Settled,
    )]
    #[case::not_found(
        state_with(vec![agent(1, AgentStatus::Idle)], 1),
        None,
        Op::PaneClose { pane: PaneId(99) },
        ErrorCode::NotFound,
    )]
    #[case::no_target(
        state_with(vec![agent(1, AgentStatus::Idle)], 1),
        None,
        Op::PaneSplit { pane: None, direction: Direction::Right, name: None, run: None, cwd: None },
        ErrorCode::NoTarget,
    )]
    #[case::bad_request(
        with_float(),
        None,
        Op::PaneSplit { pane: Some(PaneId(2)), direction: Direction::Right, name: None, run: None, cwd: None },
        ErrorCode::BadRequest,
    )]
    #[case::pane_busy(
        state_with(vec![agent(1, AgentStatus::Idle)], 1),
        None,
        Op::AgentStart { pane: Some(PaneId(1)), kind: AgentKind::Claude, name: None, args: vec![], split: None, tab: false },
        ErrorCode::PaneBusy,
    )]
    #[case::no_agent(
        state_with(vec![stopped(1)], 1),
        None,
        Op::AgentPrompt { pane: PaneId(1), text: "x".into(), wait: false, timeout: None },
        ErrorCode::NoAgent,
    )]
    #[case::agent_blocked(
        state_with(vec![agent(1, AgentStatus::NeedsInput)], 1),
        None,
        Op::AgentPrompt { pane: PaneId(1), text: "x".into(), wait: false, timeout: None },
        ErrorCode::AgentBlocked,
    )]
    #[case::no_reply(
        state_with(vec![agent(1, AgentStatus::Idle)], 1),
        None,
        Op::AgentReply { pane: PaneId(1) },
        ErrorCode::NoReply,
    )]
    fn refusals_carry_a_hint_naming_a_command(
        #[case] app: AppState,
        #[case] caller: Option<PaneId>,
        #[case] op: Op,
        #[case] expected: ErrorCode,
    ) {
        // Given / When planning a request orb refuses.
        let planned = plan(&app, &Request { caller, op });

        // Then the refusal's hint names an orb command.
        let refused = planned
            .err()
            .map(|error| (error.code, error.hint.unwrap_or_default()));
        assert!(
            refused
                .as_ref()
                .is_some_and(|(code, hint)| *code == expected && hint.contains("orb ")),
            "a {expected:?} refusal says which command fixes it, got {refused:?}"
        );
    }

    /// `agent start --kind claude` from `caller`, in the place `pane`,
    /// `split` and `tab` name.
    fn agent_start(
        caller: Option<PaneId>,
        pane: Option<PaneId>,
        split: Option<Direction>,
        tab: bool,
    ) -> Request {
        Request {
            caller,
            op: Op::AgentStart {
                pane,
                kind: AgentKind::Claude,
                name: None,
                args: vec![],
                split,
                tab,
            },
        }
    }

    #[rstest::rstest]
    fn agent_start_split_adds_beside_the_caller() {
        // Given session 1 with idle agent 1 live in pane 1.
        let app = state_with(vec![agent(1, AgentStatus::Idle)], 1);

        // When pane 1 plans `agent start --kind claude --split right`.
        let planned = plan(
            &app,
            &agent_start(Some(PaneId(1)), None, Some(Direction::Right), false),
        );

        // Then a shell is added right of pane 1 for the agent, though an
        // agent is live in pane 1.
        assert!(
            matches!(
                &planned,
                Ok(Action::StartAgent {
                    at: StartIn::New(ApiAdd {
                        session: SessionId(1),
                        place: ApiPlace::Split {
                            at: PaneId(1),
                            split: Split::Right,
                        },
                        dir: None,
                    }),
                    ..
                })
            ),
            "the agent starts in a new pane beside the caller, got {planned:?}"
        );
    }

    #[rstest::rstest]
    fn agent_start_tab_adds_a_tab_in_the_callers_session() {
        // Given session 1 with idle agent 1 live in pane 1.
        let app = state_with(vec![agent(1, AgentStatus::Idle)], 1);

        // When pane 1 plans `agent start --kind claude --tab`.
        let planned = plan(&app, &agent_start(Some(PaneId(1)), None, None, true));

        // Then a new tab is added to session 1 for the agent.
        assert!(
            matches!(
                &planned,
                Ok(Action::StartAgent {
                    at: StartIn::New(ApiAdd {
                        session: SessionId(1),
                        place: ApiPlace::Tab,
                        dir: None,
                    }),
                    ..
                })
            ),
            "the agent starts in a new tab of the caller's session, got {planned:?}"
        );
    }

    #[rstest::rstest]
    #[case::pane_and_split(Some(PaneId(1)), Some(Direction::Right), false)]
    #[case::pane_and_tab(Some(PaneId(1)), None, true)]
    #[case::split_and_tab(None, Some(Direction::Down), true)]
    fn agent_start_with_two_places_is_a_bad_request(
        #[case] pane: Option<PaneId>,
        #[case] split: Option<Direction>,
        #[case] tab: bool,
    ) {
        // Given live session 1 with pane 1.
        let app = state_with(vec![stopped(1)], 1);

        // When pane 1 plans an agent start naming two places.
        let planned = plan(&app, &agent_start(Some(PaneId(1)), pane, split, tab));

        // Then it is refused with bad_request, pointing at one place.
        let refused = planned
            .err()
            .map(|error| (error.code, error.hint.unwrap_or_default()));
        assert_eq!(
            refused,
            Some((
                ErrorCode::BadRequest,
                "name one: orb agent start --kind claude --split right".to_owned()
            )),
            "an agent starts in one place"
        );
    }

    #[rstest::rstest]
    #[case::split(Some(Direction::Right), false)]
    #[case::tab(None, true)]
    fn agent_start_split_without_a_caller_is_no_target(
        #[case] split: Option<Direction>,
        #[case] tab: bool,
    ) {
        // Given live session 1 with pane 1.
        let app = state_with(vec![agent(1, AgentStatus::Idle)], 1);

        // When planning an agent start in a new pane from outside orb.
        let planned = plan(&app, &agent_start(None, None, split, tab));

        // Then it is refused with no_target.
        assert_eq!(
            planned.map(drop).map_err(|error| error.code),
            Err(ErrorCode::NoTarget),
            "a new pane is placed beside the caller, so it needs one"
        );
    }

    #[rstest::rstest]
    fn agent_start_split_from_a_float_points_at_tab() {
        // Given session 1 with float pane 2.
        let app = with_float();

        // When pane 2 plans `agent start --kind claude --split right`.
        let planned = plan(
            &app,
            &agent_start(Some(PaneId(2)), None, Some(Direction::Right), false),
        );

        // Then it is refused with bad_request, saying to use --tab.
        let refused = planned.err().map(|error| (error.code, error.message));
        assert_eq!(
            refused,
            Some((ErrorCode::BadRequest, START_FLOAT.to_owned())),
            "a float can't be split"
        );
    }

    #[rstest::rstest]
    fn agent_not_found_hint_points_at_agent_list() {
        // Given session 1 with agent 1 in pane 1.
        let app = state_with(vec![agent(1, AgentStatus::Idle)], 1);

        // When planning `agent get 99`.
        let planned = plan(
            &app,
            &Request {
                caller: None,
                op: Op::AgentGet { pane: PaneId(99) },
            },
        );

        // Then the hint points at the agent list.
        let hint = planned.err().and_then(|error| error.hint);
        assert!(
            hint.as_deref()
                .is_some_and(|hint| hint.contains("orb agent list")),
            "a missing agent pane points at orb agent list, got {hint:?}"
        );
    }

    #[rstest::rstest]
    fn agent_get_keeps_the_full_name_cwd_and_branch() {
        // Given agent 1 in pane 1 with a 700-grapheme title, on branch "fix".
        let long = "e\u{301}".repeat(700);
        let mut agent_1 = named(agent(1, AgentStatus::Idle), &long);
        agent_1.branch = Some("fix".to_owned());
        let app = state_with(vec![agent_1], 1);

        // When planning `agent get 1`.
        let planned = plan(
            &app,
            &Request {
                caller: None,
                op: Op::AgentGet { pane: PaneId(1) },
            },
        );

        // Then the answer has the whole title as name and title, its cwd and
        // its branch.
        let row = match &planned {
            Ok(Action::Get { detail, .. }) => serde_json::to_value(detail).ok(),
            _ => None,
        };
        let field = |key: &str| row.as_ref()?.get(key)?.as_str().map(str::to_owned);
        assert_eq!(
            (field("name"), field("title"), field("cwd"), field("branch")),
            (
                Some(long.clone()),
                Some(long.clone()),
                Some("/work/1".to_owned()),
                Some("fix".to_owned())
            ),
            "agent get shows the agent in full, got {planned:?}"
        );
    }

    #[rstest::rstest]
    fn pane_read_plans_its_head_and_tail() {
        // Given live session 1 with pane 1.
        let app = state_with(vec![agent(1, AgentStatus::Idle)], 1);

        // When planning `pane read 1 --lines 50 --head 10 --tail 5`.
        let planned = plan(
            &app,
            &Request {
                caller: None,
                op: Op::PaneRead {
                    pane: Some(PaneId(1)),
                    lines: 50,
                    head: Some(10),
                    tail: Some(5),
                },
            },
        );

        // Then the screen loop reads 50 lines of history, and the cut keeps
        // the head and tail asked for.
        assert!(
            matches!(
                &planned,
                Ok(Action::Read {
                    pane: PaneId(1),
                    lines: 50,
                    head: Some(10),
                    tail: Some(5),
                })
            ),
            "the read carries its lines, head and tail, got {planned:?}"
        );
    }

    /// What `op`, asked from pane 1, plans to end, or `None` when it plans
    /// no ending.
    fn ending(app: &AppState, op: Op) -> Option<Ended> {
        match plan(
            app,
            &Request {
                caller: Some(PaneId(1)),
                op,
            },
        ) {
            Ok(Action::Ending { ended, .. }) => Some(ended),
            _ => None,
        }
    }

    /// Row `pane` of an ending: its own `name` and live `agent`.
    fn row(pane: i64, name: Option<&str>, agent: Option<&str>) -> EndedRow {
        EndedRow {
            pane: PaneId(pane),
            name: name.map(str::to_owned),
            agent: agent.map(str::to_owned),
        }
    }

    #[rstest::rstest]
    fn pane_close_ends_its_pane_and_agent() {
        // Given session 1 whose pane 1 is named "server" and holds idle
        // agent 1.
        let mut app = state_with(vec![named(agent(1, AgentStatus::Idle), "fix login")], 1);
        app.layouts.rename_pane(PaneId(1), Some("server".into()));

        // When planning `pane close 1`.
        let ended = ending(&app, Op::PaneClose { pane: PaneId(1) });

        // Then it ends pane 1 and the agent, under the name agent list shows.
        assert!(
            matches!(
                &ended,
                Some(Ended::Closed(ClosedResult { ended }))
                    if *ended == vec![row(1, Some("server"), Some("server"))]
            ),
            "the close names its pane and agent, got {ended:?}"
        );
    }

    #[rstest::rstest]
    fn tab_close_ends_every_pane_of_the_tab() {
        // Given session 1 with a tab of panes 1 (idle agent 1) and 2, and
        // float pane 3.
        let mut app = state_with(vec![named(agent(1, AgentStatus::Idle), "fix login")], 1);
        app.layouts.split(SessionId(1), Split::Right, test_entry(2));
        app.layouts.add_float_hidden(SessionId(1), test_entry(3));

        // When pane 1 plans `tab close`.
        let ended = ending(&app, Op::TabClose { pane: None });

        // Then it ends both panes of the tab, and not the float.
        assert!(
            matches!(
                &ended,
                Some(Ended::Closed(ClosedResult { ended }))
                    if *ended == vec![row(1, None, Some("fix login")), row(2, None, None)]
            ),
            "the close names the tab's two panes, got {ended:?}"
        );
    }

    #[rstest::rstest]
    fn session_settle_names_the_session_and_its_panes() {
        // Given session 1 with a 70-character name, stopped agent 1 in pane
        // 1, a second tab of pane 2, and float pane 3.
        let mut app = state_with(vec![stopped(1)], 1);
        if let Some(session) = app.sessions.sessions.first_mut() {
            session.name = Some("a".repeat(70));
        }
        app.layouts.new_tab(SessionId(1), test_entry(2));
        app.layouts.add_float_hidden(SessionId(1), test_entry(3));

        // When planning `session settle`.
        let ended = ending(&app, Op::SessionSettle { session: None });

        // Then it names the session, cut to 60, and every tab and float
        // pane; the stopped agent isn't listed.
        let expected = SettledResult {
            session: SessionId(1),
            name: format!("{}…", "a".repeat(59)),
            ended: vec![row(1, None, None), row(2, None, None), row(3, None, None)],
        };
        assert!(
            matches!(&ended, Some(Ended::Settled(settled)) if *settled == expected),
            "the settle names its session and panes, got {ended:?}"
        );
    }
}
