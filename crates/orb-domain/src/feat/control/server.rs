//! The running orb's side of the control socket: reads each request, answers
//! it from orb's state, asks the sessions actor for what it adds or the screen
//! loop for what it changes and for what a pane shows, and writes the reply.

use std::fs::{self, Permissions};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::thread;

use error_stack::{Report, ResultExt};
use kameo::actor::ActorRef;
use wherror::Error;

use crate::feat::control::change::Change;
use crate::feat::control::pane_control::{PaneControl, PaneRequest, key_event};
use crate::feat::control::protocol::{
    AgentRow, AgentsResult, ApiError, Done, ErrorCode, Op, PaneResult, PaneRow, PanesResult,
    ReadResult, Reply, Request, SessionRow, SessionsResult, StartResult, TabRow, TabsResult,
};
use crate::feat::control::target::{self, Resolved, Target};
use crate::feat::control::validator;
use crate::feat::sessions::sessions_actor::{ApiAdd, ApiPlace, ApiStart, SessionsActor};
use crate::ids::{PaneId, ProjectId, SessionId};
use crate::{AppState, Log, State};

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
    open(path, deps.log.clone(), move |request| {
        answer(&deps, request)
    })
}

/// Opens the socket at `path` as [`listen`] does, answering each request with
/// `answer`.
fn open<F>(path: &Path, log: Log, answer: F) -> Result<(), Report<ControlServerError>>
where
    F: Fn(&Request) -> Reply + Send + Sync + 'static,
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
    F: Fn(&Request) -> Reply + Send + Sync + 'static,
{
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let answer = Arc::clone(answer);
                let spawned = thread::Builder::new()
                    .name("orb-control-request".into())
                    .spawn(move || serve(stream, &*answer));
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
/// that hangs up early gets nothing.
fn serve<F>(stream: UnixStream, answer: &F)
where
    F: Fn(&Request) -> Reply,
{
    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).is_err() {
        return;
    }
    let reply = match serde_json::from_str::<Request>(&line) {
        Ok(request) => answer(&request),
        Err(error) => Reply::err(ApiError::new(ErrorCode::BadRequest, error.to_string())),
    };
    if let Ok(text) = serde_json::to_string(&reply) {
        let mut stream = stream;
        let _ = writeln!(stream, "{text}");
    }
}

/// What a request comes to once read against the state.
#[derive(Debug)]
enum Action {
    /// Answered from the state alone.
    Answered(Reply),
    /// A new shell the sessions actor adds.
    Add(ApiAdd),
    /// A new session the sessions actor starts.
    Start(ApiStart),
    /// A change the screen loop applies.
    Change(Change),
    /// Typing the screen loop does into a pane: a send, keys or a run.
    Pane(PaneRequest),
    /// A pane's text the screen loop reads.
    Read(PaneRequest),
}

/// The reply to `request`: worked out from the state (with no lock held
/// after), then carried out. Each change writes one `api` log line; lists and
/// reads write none.
fn answer(deps: &ControlDeps, request: &Request) -> Reply {
    let planned = plan(&deps.state.read(), request);
    let reply = match planned {
        Ok(Action::Answered(reply)) => return reply,
        Ok(Action::Read(read)) => {
            return match deps.panes.ask(read) {
                Ok(text) => Reply::ok(&ReadResult {
                    text: text.unwrap_or_default(),
                }),
                Err(error) => Reply::err(error),
            };
        }
        Ok(Action::Add(add)) => add_pane(deps, add),
        Ok(Action::Start(start)) => start_session(deps, start),
        Ok(Action::Change(change)) => typed(deps, PaneRequest::Change(change)),
        Ok(Action::Pane(request)) => typed(deps, request),
        Err(error) => Reply::err(error),
    };
    deps.log.line("api", &log_text(request, &reply));
    reply
}

/// `<op> caller=<id> pane=<id> session=<id> project=<id> -> ok|<code>`, with
/// only the IDs `request` carries: never a name, a prompt or pasted text.
fn log_text(request: &Request, reply: &Reply) -> String {
    let fields = serde_json::to_value(request).unwrap_or_default();
    let text = |key| match fields.get(key) {
        Some(serde_json::Value::String(op)) if key == "op" => Some(op.clone()),
        Some(serde_json::Value::Number(id)) => Some(format!("{key}={id}")),
        _ => None,
    };
    let result = match &reply.error {
        None => "ok".to_owned(),
        Some(error) => serde_json::to_value(error.code)
            .ok()
            .and_then(|code| code.as_str().map(str::to_owned))
            .unwrap_or_default(),
    };
    ["op", "caller", "pane", "session", "project"]
        .into_iter()
        .filter_map(text)
        .chain([format!("-> {result}")])
        .collect::<Vec<_>>()
        .join(" ")
}

/// What `request` comes to, read from `app`, or why it's refused.
fn plan(app: &AppState, request: &Request) -> Result<Action, ApiError> {
    let caller = request.caller;
    let add = |session, place| Ok(Action::Add(ApiAdd { session, place }));
    match &request.op {
        Op::SessionList | Op::TabList { .. } | Op::PaneList { .. } | Op::AgentList => {
            Ok(Action::Answered(route(app, request)))
        }
        Op::PaneSplit { pane, direction } => {
            let (session, at) = tiled(app, *pane, caller, SPLIT_FLOAT)?;
            add(
                session,
                ApiPlace::Split {
                    at,
                    split: (*direction).into(),
                },
            )
        }
        Op::PaneStack { pane } => {
            let (session, at) = tiled(app, *pane, caller, SPLIT_FLOAT)?;
            add(session, ApiPlace::Stack { at })
        }
        Op::PaneFloat { session } => add(changing_session(app, *session, caller)?, ApiPlace::Float),
        Op::TabCreate { session } => add(changing_session(app, *session, caller)?, ApiPlace::Tab),
        Op::SessionCreate {
            project,
            worktree,
            name,
        } => Ok(Action::Start(ApiStart {
            project: project_for(app, *project, caller)?,
            worktree: *worktree,
            name: name.clone(),
        })),
        Op::PaneClose { pane } => {
            changing(app, Some(Target::Pane(*pane)), caller)?;
            Ok(Action::Change(Change::ClosePane(*pane)))
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
            Ok(Action::Change(Change::CloseTab(pane)))
        }
        Op::SessionRename { session, name } => Ok(Action::Change(Change::RenameSession {
            session: changing_session(app, *session, caller)?,
            name: name.clone(),
        })),
        Op::SessionSettle { session } => Ok(Action::Change(Change::SettleSession(
            changing_session(app, *session, caller)?,
        ))),
        Op::SessionFocus { session } => Ok(Action::Change(Change::Focus {
            session: changing_session(app, Some(*session), caller)?,
            pane: None,
        })),
        Op::PaneSend { .. } | Op::PaneKeys { .. } | Op::PaneRun { .. } | Op::PaneRead { .. } => {
            pane_io(app, &request.op, caller)
        }
    }
}

/// What a send, keys, run or read comes to, read from `app`: settled before
/// an unknown key name, so a refused list of keys sends nothing.
fn pane_io(app: &AppState, op: &Op, caller: Option<PaneId>) -> Result<Action, ApiError> {
    match op {
        Op::PaneSend { pane, text } => {
            changing(app, Some(Target::Pane(*pane)), caller)?;
            Ok(Action::Pane(PaneRequest::Send {
                pane: *pane,
                text: text.clone(),
            }))
        }
        Op::PaneKeys { pane, keys } => {
            changing(app, Some(Target::Pane(*pane)), caller)?;
            let keys = keys
                .iter()
                .map(|name| {
                    key_event(name).ok_or_else(|| {
                        ApiError::new(ErrorCode::BadRequest, format!("unknown key {name}"))
                    })
                })
                .collect::<Result<_, _>>()?;
            Ok(Action::Pane(PaneRequest::Keys { pane: *pane, keys }))
        }
        Op::PaneRun { pane, command } => {
            changing(app, Some(Target::Pane(*pane)), caller)?;
            Ok(Action::Pane(PaneRequest::Run {
                pane: *pane,
                line: command.clone(),
            }))
        }
        Op::PaneRead { pane, lines } => match changing(app, pane.map(Target::Pane), caller)? {
            Resolved {
                pane: Some(pane), ..
            } => Ok(Action::Read(PaneRequest::Read {
                pane,
                lines: *lines,
            })),
            Resolved { pane: None, .. } => {
                Err(ApiError::new(ErrorCode::BadRequest, "no pane to read"))
            }
        },
        _ => Err(ApiError::new(
            ErrorCode::BadRequest,
            "not pane input or output",
        )),
    }
}

/// Asks the screen loop for `request` and replies `{}` once it's done.
fn typed(deps: &ControlDeps, request: PaneRequest) -> Reply {
    match deps.panes.ask(request) {
        Ok(_) => Reply::ok(&Done {}),
        Err(error) => Reply::err(error),
    }
}

/// What a change is about, as [`target::resolve`] finds it; `settled` when
/// its session is settled.
fn changing(
    app: &AppState,
    target: Option<Target>,
    caller: Option<PaneId>,
) -> Result<Resolved, ApiError> {
    let resolved = target::resolve(app, target, caller)?;
    validator::not_settled(app, resolved.session)?;
    Ok(resolved)
}

/// The session a change names, else the caller's; `settled` when it is.
fn changing_session(
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
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such project"))
}

/// Pane `pane` (else the caller's) and its session; `bad_request` saying
/// `why` when it's a float.
fn tiled(
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
        _ => Err(ApiError::new(ErrorCode::BadRequest, why)),
    }
}

/// Why a float can't be split or stacked.
const SPLIT_FLOAT: &str = "floats can't be split or stacked";

/// Asks the sessions actor for the shell `add` describes and replies with it.
fn add_pane(deps: &ControlDeps, add: ApiAdd) -> Reply {
    match deps
        .runtime
        .block_on(async { deps.sessions.ask(add).await })
    {
        Ok(pane) => Reply::ok(&PaneResult { pane }),
        Err(error) => Reply::err(ApiError::new(ErrorCode::Failed, error.to_string())),
    }
}

/// Marks a session as starting (refused with `starting` while one already
/// is), then asks the sessions actor to start `start` and replies with the new
/// session and its first pane. The actor clears the mark however it ends.
fn start_session(deps: &ControlDeps, start: ApiStart) -> Reply {
    let claimed = {
        let mut app = deps.state.write();
        validator::not_starting(&app).map(|()| app.sessions.starting = true)
    };
    if let Err(error) = claimed {
        return Reply::err(error);
    }
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
        Err(error) => Reply::err(ApiError::new(ErrorCode::Failed, error.to_string())),
    }
}

/// The reply to a list `request`, read from `app`.
fn route(app: &AppState, request: &Request) -> Reply {
    match request.op {
        Op::SessionList => Reply::ok(&sessions(app)),
        Op::TabList { session } => match in_session(app, session, request.caller) {
            Ok(resolved) => Reply::ok(&tabs(app, resolved.session)),
            Err(error) => Reply::err(error),
        },
        Op::PaneList { session } => match in_session(app, session, request.caller) {
            Ok(resolved) => Reply::ok(&panes(app, resolved.session, request.caller)),
            Err(error) => Reply::err(error),
        },
        Op::AgentList => Reply::ok(&agents(app)),
        _ => Reply::err(ApiError::new(ErrorCode::BadRequest, "not a list")),
    }
}

/// The session named, else the caller's.
fn in_session(
    app: &AppState,
    session: Option<SessionId>,
    caller: Option<PaneId>,
) -> Result<Resolved, ApiError> {
    target::resolve(app, session.map(Target::Session), caller)
}

/// Every session not being deleted, settled ones included.
fn sessions(app: &AppState) -> SessionsResult {
    let sessions = app
        .sessions
        .sessions
        .iter()
        .filter(|session| !app.sessions.deleting.contains(&session.id))
        .map(|session| SessionRow {
            id: session.id,
            project: session.project,
            name: app.sessions.title(session),
            dir: session.dir.to_string_lossy().into_owned(),
            branch: session.branch.clone(),
            settled: session.settled_at.is_some(),
        })
        .collect();
    SessionsResult { sessions }
}

/// Session `session`'s tabs, numbered from 1 as the tab bar shows them, with
/// their panes; none without a layout.
fn tabs(app: &AppState, session: SessionId) -> TabsResult {
    let tabs = app
        .layouts
        .get(session)
        .map(|layout| {
            layout
                .tabs()
                .iter()
                .enumerate()
                .map(|(index, tab)| TabRow {
                    index: index + 1,
                    panes: tab.pane_ids(),
                })
                .collect()
        })
        .unwrap_or_default();
    TabsResult { session, tabs }
}

/// Session `session`'s panes: each tab's, then the floats, with `caller`'s
/// marked.
fn panes(app: &AppState, session: SessionId, caller: Option<PaneId>) -> PanesResult {
    let Some(layout) = app.layouts.get(session) else {
        return PanesResult { panes: Vec::new() };
    };
    let agents = app.sessions.agent_rows(session, &app.layouts);
    let placed = layout
        .tabs()
        .iter()
        .enumerate()
        .flat_map(|(index, tab)| {
            tab.pane_ids()
                .into_iter()
                .map(move |pane| (pane, Some(index + 1)))
        })
        .chain(layout.floats().pane_ids().map(|pane| (pane, None)));
    let panes = placed
        .map(|(pane, tab)| {
            let entry = app.layouts.entry(pane);
            PaneRow {
                id: pane,
                session,
                tab,
                float: tab.is_none(),
                name: entry.and_then(|entry| entry.name.clone()),
                cwd: entry
                    .map(|entry| entry.cwd.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                agent: agents
                    .iter()
                    .find(|&&(_, shown)| shown == pane)
                    .map(|(agent, _)| agent.id),
                caller: Some(pane) == caller,
            }
        })
        .collect();
    PanesResult { panes }
}

/// Every live agent in every session not being deleted.
fn agents(app: &AppState) -> AgentsResult {
    let agents = app
        .sessions
        .sessions
        .iter()
        .flat_map(|session| {
            app.sessions
                .agent_rows(session.id, &app.layouts)
                .into_iter()
                .map(move |(agent, pane)| AgentRow {
                    id: agent.id,
                    pane: Some(pane),
                    session: session.id,
                    harness: agent.harness.to_string(),
                    name: app.layouts.agent_name(pane, Some(agent)).to_owned(),
                    status: agent.status,
                    cwd: agent.cwd.to_string_lossy().into_owned(),
                    branch: agent.branch.clone(),
                })
        })
        .collect();
    AgentsResult { agents }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate setup failures with `?` and assert on the outcome"
)]
mod tests {
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use error_stack::{Report, ResultExt};
    use tempfile::TempDir;

    use super::{ControlServerError, log_text, open, plan, route};
    use crate::feat::control::client;
    use crate::feat::control::protocol::{
        ApiError, Direction, Done, ErrorCode, Op, Reply, Request,
    };
    use crate::feat::layout::state::test_entry;
    use crate::feat::sessions::state::AgentStatus;
    use crate::ids::{PaneId, SessionId};
    use crate::intent_handler::test_support::{agent, settle, state_with, stopped};
    use crate::{AppState, Log, State};

    /// Listens on a socket in a fresh temp dir, answering from `app`.
    fn listening(app: AppState) -> Result<(TempDir, PathBuf), Report<ControlServerError>> {
        let dir = tempfile::tempdir().change_context(ControlServerError)?;
        let socket = dir.path().join("orb.sock");
        let state = State::new(app);
        open(&socket, Log::default(), move |request| {
            route(&state.read(), request)
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
    #[case::pane_split(Op::PaneSplit { pane: None, direction: Direction::Right })]
    #[case::pane_stack(Op::PaneStack { pane: None })]
    #[case::pane_float(Op::PaneFloat { session: None })]
    #[case::tab_create(Op::TabCreate { session: None })]
    #[case::tab_close(Op::TabClose { pane: None })]
    #[case::pane_close(Op::PaneClose { pane: PaneId(1) })]
    #[case::pane_rename(Op::PaneRename { pane: PaneId(1), name: "x".into() })]
    #[case::pane_focus(Op::PaneFocus { pane: PaneId(1) })]
    #[case::session_rename(Op::SessionRename { session: None, name: "x".into() })]
    #[case::session_settle(Op::SessionSettle { session: None })]
    #[case::session_focus(Op::SessionFocus { session: SessionId(1) })]
    #[case::pane_send(Op::PaneSend { pane: PaneId(1), text: "x".into() })]
    #[case::pane_keys(Op::PaneKeys { pane: PaneId(1), keys: vec!["enter".into()] })]
    #[case::pane_run(Op::PaneRun { pane: PaneId(1), command: "x".into() })]
    #[case::pane_read(Op::PaneRead { pane: None, lines: 0 })]
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
        let reply = Reply::err(ApiError::new(ErrorCode::Settled, "session is settled"));

        // When writing its log line.
        let text = log_text(&request, &reply);

        // Then it has the op, the IDs and the code, and not the name.
        assert_eq!(
            text, "session.rename caller=4 session=2 -> settled",
            "the log line carries IDs only"
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
}
