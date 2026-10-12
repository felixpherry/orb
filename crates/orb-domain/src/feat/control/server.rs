//! The running orb's side of the control socket: reads each request, answers
//! it from orb's state, and writes the reply.

use std::fs::{self, Permissions};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::thread;

use error_stack::{Report, ResultExt};
use wherror::Error;

use crate::feat::control::protocol::{
    AgentRow, AgentsResult, ApiError, ErrorCode, Op, PaneRow, PanesResult, Reply, Request,
    SessionRow, SessionsResult, TabRow, TabsResult,
};
use crate::feat::control::target::{self, Resolved, Target};
use crate::ids::{PaneId, SessionId};
use crate::{AppState, Log, State};

/// The control socket couldn't be opened.
#[derive(Debug, Error)]
#[error(debug)]
pub struct ControlServerError;

/// What the control server reads and where it logs.
#[derive(Debug, Clone)]
pub struct ControlDeps {
    /// orb's shared state, read to answer requests.
    pub state: State,
    /// Where failures go.
    pub log: Log,
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
    remove_socket(path, &deps.log);
    let listener = UnixListener::bind(path)
        .change_context(ControlServerError)
        .attach_with(|| format!("couldn't bind {}", path.display()))?;
    fs::set_permissions(path, Permissions::from_mode(0o600))
        .change_context(ControlServerError)
        .attach("couldn't make the socket private")?;
    thread::Builder::new()
        .name("orb-control".into())
        .spawn(move || accept(&listener, &deps))
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
fn accept(listener: &UnixListener, deps: &ControlDeps) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let state = deps.state.clone();
                let spawned = thread::Builder::new()
                    .name("orb-control-request".into())
                    .spawn(move || serve(stream, &state));
                if let Err(error) = spawned {
                    deps.log
                        .error_on_change("control-spawn", "couldn't answer a request", &error);
                }
            }
            Err(error) => {
                deps.log
                    .error_on_change("control-accept", "couldn't accept a request", &error);
            }
        }
    }
}

/// Reads one request line, answers it, and writes one reply line. A client
/// that hangs up early gets nothing.
fn serve(stream: UnixStream, state: &State) {
    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).is_err() {
        return;
    }
    let reply = match serde_json::from_str::<Request>(&line) {
        Ok(request) => route(&state.read(), &request),
        Err(error) => Reply::err(ApiError::new(ErrorCode::BadRequest, error.to_string())),
    };
    if let Ok(text) = serde_json::to_string(&reply) {
        let mut stream = stream;
        let _ = writeln!(stream, "{text}");
    }
}

/// The reply to `request`, read from `app`.
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

    use super::{ControlDeps, ControlServerError, listen};
    use crate::feat::control::client;
    use crate::feat::sessions::state::AgentStatus;
    use crate::ids::PaneId;
    use crate::intent_handler::test_support::{agent, state_with};
    use crate::{AppState, Log, State};

    /// Listens on a socket in a fresh temp dir, answering from `app`.
    fn listening(app: AppState) -> Result<(TempDir, PathBuf), Report<ControlServerError>> {
        let dir = tempfile::tempdir().change_context(ControlServerError)?;
        let socket = dir.path().join("orb.sock");
        let deps = ControlDeps {
            state: State::new(app),
            log: Log::default(),
        };
        listen(&socket, deps)?;
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
}
