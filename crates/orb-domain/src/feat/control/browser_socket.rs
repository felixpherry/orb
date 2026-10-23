//! terminal-browser's split socket: answers what terminal-browser asks the
//! socket its `TERMINAL_BROWSER_TERMINAL_SOCKET` names, so `terminal-browser
//! open --split` opens its browser in a new orb pane.
//!
//! Each connection carries one JSON line, `{"id","method","params","caller"}`,
//! answered by one line, `{"id","result"}` or `{"id","error":{"message"}}`.
//! orb offers three methods: which pane a process runs in, a split that types
//! a command into the new pane, and moving the keys into a pane. Splits and
//! focus go through the control socket's own requests, so they're checked and
//! logged the same way, and a split leaves the keys where they were.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use error_stack::Report;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::State;
use crate::common::{Finished, run_within};
use crate::feat::control::agent::quoted;
use crate::feat::control::protocol::{Direction, Op, PaneResult, Reply, Request};
use crate::feat::control::server::{self, ControlDeps, ControlServerError};
use crate::feat::control::target::{self, Target};
use crate::feat::sessions::child_env::ORB_PANE_ID;
use crate::ids::PaneId;

/// The socket's file name inside orb's folder (`~/.orb`).
pub const SOCKET_FILE: &str = "terminal-browser.sock";

/// Where a running orb listens for terminal-browser, given orb's folder.
pub fn socket_path(orb_root: &Path) -> PathBuf {
    orb_root.join(SOCKET_FILE)
}

/// The methods `hello` offers; terminal-browser turns off every other one.
const METHODS: [&str; 3] = ["getCurrentPane", "split", "focusPane"];

/// How long reading a process's environment may take.
const ENV_LIMIT: Duration = Duration::from_secs(2);

/// Finds the orb pane a process runs in.
pub trait PaneFinder: Send + Sync {
    fn name(&self) -> &'static str;

    /// The pane process `pid` was started in, from its `ORB_PANE_ID`; `None`
    /// when it has none, has exited, or its environment can't be read.
    fn pane_of(&self, pid: u32) -> Option<PaneId>;

    fn panes_on(&self, tty: &str) -> Vec<PaneId>;
}

/// Reads a process's environment as it was when it started: `ps -E` on
/// macOS, `/proc/<pid>/environ` on Linux. macOS shows it only for the user's
/// own processes that aren't Apple's.
#[derive(Debug)]
pub struct LaunchEnv;

impl PaneFinder for LaunchEnv {
    fn name(&self) -> &'static str {
        "launch-env"
    }

    fn pane_of(&self, pid: u32) -> Option<PaneId> {
        // ponytail: pane from the caller's env via ps; switch to sysctl KERN_PROCARGS2 if ps proves slow.
        let env = if cfg!(target_os = "linux") {
            std::fs::read(format!("/proc/{pid}/environ")).ok()?
        } else {
            let mut command = Command::new("ps");
            command.args(["-E", "-o", "command=", "-p", &pid.to_string()]);
            match run_within(command, ENV_LIMIT) {
                Ok(Finished::Exited(output)) => output.stdout,
                _ => return None,
            }
        };
        pane_in(&String::from_utf8_lossy(&env))
    }

    fn panes_on(&self, tty: &str) -> Vec<PaneId> {
        if terminal_name(tty).is_none() {
            return Vec::new();
        }
        let mut command = Command::new("ps");
        command.args(["-t", tty, "-o", "pid="]);
        let output = match run_within(command, ENV_LIMIT) {
            Ok(Finished::Exited(output)) if output.status.success() => output.stdout,
            _ => return Vec::new(),
        };
        String::from_utf8_lossy(&output)
            .split_whitespace()
            .filter_map(|pid| pid.parse().ok())
            .filter_map(|pid| self.pane_of(pid))
            .collect()
    }
}

fn terminal_name(tty: &str) -> Option<&str> {
    let name = tty.strip_prefix("/dev/")?;
    name.split('/')
        .all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
        .then_some(name)
}

/// The pane the last `ORB_PANE_ID=<id>` word in `env` names. Words split at
/// whitespace and NULs, so this reads `ps -E` (the command, then the
/// environment) and `/proc/<pid>/environ` alike.
pub(crate) fn pane_in(env: &str) -> Option<PaneId> {
    env.split(|c: char| c == '\0' || c.is_whitespace())
        .rev()
        .find_map(|word| word.strip_prefix(ORB_PANE_ID)?.strip_prefix('='))
        .and_then(|id| id.parse().ok())
        .map(PaneId)
}

/// Opens terminal-browser's socket at `path`, only for the user, and answers
/// each request through the control server's own requests with `deps`. A
/// socket left by an orb that crashed is replaced.
///
/// # Errors
///
/// Returns an error if the socket can't be bound, made private, or its
/// thread can't start.
pub fn listen(path: &Path, deps: ControlDeps) -> Result<(), Report<ControlServerError>> {
    let state = deps.state.clone();
    server::open(path, deps.log.clone(), move |line, _gone| {
        reply(line, &state, &LaunchEnv, &|request: &Request| {
            server::answer(&deps, request, &|| false)
        })
    })
}

/// One request from terminal-browser.
#[derive(Debug, Deserialize)]
struct Call {
    /// Echoed in the reply, which terminal-browser checks.
    id: String,
    /// What is asked: `hello`, `getCurrentPane`, `split`, `focusPane`, …
    method: String,
    /// The method's arguments.
    #[serde(default)]
    params: Value,
    /// The process that asks, when it says.
    #[serde(default)]
    caller: Option<Caller>,
}

/// The process that sent a request (its `cwd` is ignored).
#[derive(Debug, Deserialize)]
struct Caller {
    /// Its process id.
    pid: u32,
}

/// A pane as terminal-browser names it: orb's pane id and its session's id,
/// as text.
#[derive(Debug, Serialize, Deserialize)]
struct Pane {
    /// orb's pane id.
    id: String,
    /// The id of the session holding it.
    tab: String,
}

/// What `split` is given; its `size` and `tty` are ignored.
#[derive(Debug, Deserialize)]
struct SplitParams {
    /// The pane to split.
    from: Pane,
    /// Which side the new pane goes; orb has right and down.
    direction: Direction,
    /// The program and its arguments to run in the new pane.
    command: Vec<String>,
}

/// What `focusPane` is given.
#[derive(Debug, Deserialize)]
struct FocusParams {
    /// The pane to move the keys into.
    pane: String,
}

/// The reply line to request `line`. The caller's pane comes from
/// `finder` and `state`; splits and focus are control requests `orb`
/// carries out.
fn reply<A>(line: &str, state: &State, finder: &dyn PaneFinder, orb: &A) -> String
where
    A: Fn(&Request) -> Reply,
{
    let Ok(call) = serde_json::from_str::<Call>(line) else {
        return reply_text("", Err("orb couldn't read the request".to_owned()));
    };
    let result = match call.method.as_str() {
        "hello" => Ok(json!({ "name": "orb", "methods": METHODS })),
        "getCurrentPane" => Ok(json!({
            "pane": current_pane(state, finder, &call.params, call.caller.as_ref())
        })),
        "split" => split(orb, call.params),
        "focusPane" => focus(orb, call.params),
        other => Err(format!("orb does not support {other}")),
    };
    reply_text(&call.id, result)
}

/// `{"id","result"}`, or `{"id","error":{"message"}}`, as one line of text.
fn reply_text(id: &str, result: Result<Value, String>) -> String {
    match result {
        Ok(result) => json!({ "id": id, "result": result }),
        Err(message) => json!({ "id": id, "error": { "message": message } }),
    }
    .to_string()
}

/// The pane `caller` runs in and its session; `None` outside orb, or for a
/// pane orb no longer has. The process is read before the state is locked.
fn current_pane(
    state: &State,
    finder: &dyn PaneFinder,
    params: &Value,
    caller: Option<&Caller>,
) -> Option<Pane> {
    let panes = match params.get("tty") {
        None | Some(Value::Null) => vec![finder.pane_of(caller?.pid)?],
        Some(tty) => {
            let tty = tty.as_str()?;
            terminal_name(tty)?;
            finder.panes_on(tty)
        }
    };
    let app = state.read();
    let mut owners = panes.into_iter().filter_map(|pane| {
        target::resolve(&app, Some(Target::Pane(pane)), None)
            .ok()
            .map(|owner| (pane, owner.session))
    });
    let (pane, session) = owners.next()?;
    if owners.any(|(other, _)| other != pane) {
        return None;
    }
    Some(Pane {
        id: pane.0.to_string(),
        tab: session.0.to_string(),
    })
}

/// Splits `from` toward `direction`, then types `command`, each word quoted,
/// into the new pane; `{}`, or orb's refusal.
fn split<A>(orb: &A, params: Value) -> Result<Value, String>
where
    A: Fn(&Request) -> Reply,
{
    let SplitParams {
        from,
        direction,
        command,
    } = serde_json::from_value(params).map_err(|error| error.to_string())?;
    let at = pane_id(&from.id)?;
    let made = result_of(orb(&ask(Op::PaneSplit {
        pane: Some(at),
        direction,
        name: None,
        run: None,
        cwd: None,
    })))?;
    let PaneResult { pane } = serde_json::from_value(made).map_err(|error| error.to_string())?;
    let line = command
        .iter()
        .map(String::as_str)
        .map(quoted)
        .collect::<Vec<_>>()
        .join(" ");
    result_of(orb(&ask(Op::PaneRun {
        pane,
        command: line,
        wait: false,
        timeout: None,
        force: false,
        tail: None,
    })))
    .map(|_| json!({}))
}

/// Shows `pane` and moves the keys into it; `{}`, or orb's refusal.
fn focus<A>(orb: &A, params: Value) -> Result<Value, String>
where
    A: Fn(&Request) -> Reply,
{
    let FocusParams { pane } = serde_json::from_value(params).map_err(|error| error.to_string())?;
    result_of(orb(&ask(Op::PaneFocus {
        pane: pane_id(&pane)?,
    })))
    .map(|_| json!({}))
}

/// A control request for `op`, naming its pane rather than a caller.
fn ask(op: Op) -> Request {
    Request { caller: None, op }
}

/// The pane terminal-browser names `id`.
fn pane_id(id: &str) -> Result<PaneId, String> {
    id.parse()
        .ok()
        .map(PaneId)
        .ok_or_else(|| format!("no pane {id}"))
}

/// `reply`'s result, or its error's message.
fn result_of(reply: Reply) -> Result<Value, String> {
    match reply.error {
        Some(error) => Err(error.message),
        None => Ok(reply.result.unwrap_or_default()),
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate setup failures with `?` and assert on the outcome"
)]
mod tests {
    use std::cell::RefCell;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    use error_stack::{Report, ResultExt};
    use serde_json::{Value, json};

    use super::{LaunchEnv, PaneFinder, SOCKET_FILE, pane_in, reply};
    use crate::common::wait_within;
    use crate::feat::control::protocol::{
        ApiError, Direction, Done, ErrorCode, Op, PaneResult, Reply, Request,
    };
    use crate::feat::control::server::{ControlServerError, open};
    use crate::feat::layout::state::test_entry;
    use crate::feat::layout::tree::Split;
    use crate::feat::sessions::state::AgentStatus;
    use crate::ids::{PaneId, SessionId};
    use crate::intent_handler::test_support::{agent, state_with};
    use crate::{AppState, Log, State};

    /// Puts every process in one pane, or in none.
    struct Fixed {
        process: Option<PaneId>,
        terminals: Vec<(&'static str, Vec<PaneId>)>,
    }

    impl PaneFinder for Fixed {
        fn name(&self) -> &'static str {
            "fixed"
        }

        fn pane_of(&self, _pid: u32) -> Option<PaneId> {
            self.process
        }

        fn panes_on(&self, tty: &str) -> Vec<PaneId> {
            self.terminals
                .iter()
                .find(|(terminal, _)| *terminal == tty)
                .map(|(_, panes)| panes.clone())
                .unwrap_or_default()
        }
    }

    /// orb doing what it's asked: a split makes pane 9, anything else is done.
    fn obliging(request: &Request) -> Reply {
        match request.op {
            Op::PaneSplit { .. } => Reply::ok(&PaneResult { pane: PaneId(9) }),
            _ => Reply::ok(&Done {}),
        }
    }

    /// orb refusing every request because the session is settled.
    fn settled(_request: &Request) -> Reply {
        Reply::err(ApiError::new(
            ErrorCode::Settled,
            "session is settled",
            "orb session list shows open ones",
        ))
    }

    /// Session 1 holding pane 1.
    fn one_pane() -> AppState {
        state_with(vec![agent(1, AgentStatus::Idle)], 1)
    }

    /// `line` answered from `app`, with every process in pane `caller`, and
    /// the control requests that reached `orb` on the way.
    fn exchange<A>(
        line: &Value,
        app: AppState,
        caller: Option<PaneId>,
        orb: A,
    ) -> Result<(Value, Vec<Request>), serde_json::Error>
    where
        A: Fn(&Request) -> Reply,
    {
        let asked = RefCell::new(Vec::new());
        let record = |request: &Request| {
            asked.borrow_mut().push(request.clone());
            orb(request)
        };
        let finder = Fixed {
            process: caller,
            terminals: vec![("/dev/ttys001", caller.into_iter().collect())],
        };
        let text = reply(&line.to_string(), &State::new(app), &finder, &record);
        Ok((serde_json::from_str(&text)?, asked.into_inner()))
    }

    fn located(
        line: &Value,
        app: AppState,
        owners: Vec<PaneId>,
    ) -> Result<Value, serde_json::Error> {
        let finder = Fixed {
            process: Some(PaneId(1)),
            terminals: vec![("/dev/ttys002", owners)],
        };
        serde_json::from_str(&reply(
            &line.to_string(),
            &State::new(app),
            &finder,
            &obliging,
        ))
    }

    fn hello() -> Value {
        json!({"id": "abc", "method": "hello", "params": {"protocol": 1}, "caller": {"pid": 42, "cwd": "/work"}})
    }

    fn get_current_pane() -> Value {
        json!({"id": "r0", "method": "getCurrentPane", "params": {"tty": "/dev/ttys001", "cwd": "/work"}, "caller": {"pid": 42, "cwd": "/work"}})
    }

    /// terminal-browser's split request, as 0.13.4 sends it.
    fn split_line(direction: &str) -> Value {
        json!({
            "id": "r1",
            "method": "split",
            "params": {
                "from": {"id": "1", "tab": "1"},
                "direction": direction,
                "command": ["/opt/tb/bin/terminal-browser", "open", "https://x.test/?q=it's"],
                "size": null,
                "tty": "/dev/ttys001"
            },
            "caller": {"pid": 42, "cwd": "/work"}
        })
    }

    #[rstest::rstest]
    fn hello_lists_the_three_methods() -> Result<(), serde_json::Error> {
        // Given orb's split socket.
        // When terminal-browser says hello.
        let (reply, _) = exchange(&hello(), one_pane(), None, obliging)?;

        // Then orb names itself and its three methods.
        assert_eq!(
            reply.get("result"),
            Some(&json!({"name": "orb", "methods": ["getCurrentPane", "split", "focusPane"]})),
            "hello should list the three methods"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn reply_carries_the_request_id() -> Result<(), serde_json::Error> {
        // Given a hello with id `abc`.
        // When orb answers it.
        let (reply, _) = exchange(&hello(), one_pane(), None, obliging)?;

        // Then the reply carries the same id.
        assert_eq!(
            reply.get("id"),
            Some(&json!("abc")),
            "the reply should echo the id"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn current_pane_names_the_callers_pane_and_session() -> Result<(), serde_json::Error> {
        // Given the caller runs in pane 1, which session 1 holds.
        // When terminal-browser asks which pane it's in.
        let (reply, _) = exchange(&get_current_pane(), one_pane(), Some(PaneId(1)), obliging)?;

        // Then orb names pane 1 of session 1, as text.
        assert_eq!(
            reply.get("result"),
            Some(&json!({"pane": {"id": "1", "tab": "1"}})),
            "the caller's pane and session should be named"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case::same_session(false, "1")]
    #[case::different_sessions(true, "2")]
    fn browser_terminal_overrides_the_shared_daemons_pane(
        #[case] different_sessions: bool,
        #[case] session: &str,
    ) -> Result<(), serde_json::Error> {
        let app = if different_sessions {
            state_with(
                vec![agent(1, AgentStatus::Idle), agent(2, AgentStatus::Idle)],
                1,
            )
        } else {
            let mut app = one_pane();
            app.layouts.split(SessionId(1), Split::Right, test_entry(2));
            app
        };
        let line = json!({
            "id": "browser", "method": "getCurrentPane",
            "params": {"tty": "/dev/ttys002", "cwd": "/work/2"},
            "caller": {"pid": 42, "cwd": "/work/1"}
        });
        let reply = located(&line, app, vec![PaneId(2)])?;
        assert_eq!(
            reply.get("result"),
            Some(&json!({"pane": {"id": "2", "tab": session}})),
            "the browser terminal must identify its own pane and session, not the daemon's"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case::unknown(json!("/dev/ttys999"))]
    #[case::empty(json!(""))]
    #[case::number(json!(42))]
    #[case::object(json!({"path": "/dev/ttys002"}))]
    #[case::relative_path(json!("ttys002"))]
    #[case::empty_device(json!("/dev/"))]
    #[case::parent_path(json!("/dev/../ttys002"))]
    #[case::whitespace(json!("/dev/ttys002\n"))]
    #[case::nul(json!("/dev/ttys002\0"))]
    fn unusable_browser_terminal_never_falls_back_to_the_daemon(
        #[case] tty: Value,
    ) -> Result<(), serde_json::Error> {
        let line = json!({
            "id": "browser", "method": "getCurrentPane",
            "params": {"tty": tty}, "caller": {"pid": 42}
        });
        let reply = located(&line, one_pane(), vec![PaneId(1)])?;
        assert_eq!(
            reply.get("result"),
            Some(&json!({"pane": null})),
            "an unusable supplied terminal must not resolve to the daemon's pane"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn ambiguous_browser_terminal_has_no_pane() -> Result<(), serde_json::Error> {
        let mut app = one_pane();
        app.layouts.split(SessionId(1), Split::Right, test_entry(2));
        let line = json!({
            "id": "browser", "method": "getCurrentPane",
            "params": {"tty": "/dev/ttys002"}, "caller": {"pid": 42}
        });
        let reply = located(&line, app, vec![PaneId(1), PaneId(2)])?;
        assert_eq!(
            reply.get("result"),
            Some(&json!({"pane": null})),
            "a terminal shared by distinct live panes must not select either"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn repeated_processes_in_one_pane_are_not_ambiguous() -> Result<(), serde_json::Error> {
        let mut app = one_pane();
        app.layouts.split(SessionId(1), Split::Right, test_entry(2));
        let line = json!({
            "id": "browser", "method": "getCurrentPane",
            "params": {"tty": "/dev/ttys002"}, "caller": {"pid": 42}
        });
        let reply = located(&line, app, vec![PaneId(2), PaneId(2)])?;
        assert_eq!(
            reply.get("result"),
            Some(&json!({"pane": {"id": "2", "tab": "1"}})),
            "several processes in one pane still identify one terminal owner"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case::missing(json!({}))]
    #[case::null(json!({"tty": null}))]
    fn browser_without_a_terminal_keeps_process_lookup(
        #[case] params: Value,
    ) -> Result<(), serde_json::Error> {
        let line = json!({
            "id": "browser", "method": "getCurrentPane",
            "params": params, "caller": {"pid": 42}
        });
        let reply = located(&line, one_pane(), Vec::new())?;
        assert_eq!(
            reply.get("result"),
            Some(&json!({"pane": {"id": "1", "tab": "1"}})),
            "an absent or null terminal must retain caller process compatibility"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn browser_terminal_resolves_without_a_caller() -> Result<(), serde_json::Error> {
        let line = json!({
            "id": "browser", "method": "getCurrentPane",
            "params": {"tty": "/dev/ttys002"}
        });
        let reply = located(&line, one_pane(), vec![PaneId(1)])?;
        assert_eq!(
            reply.get("result"),
            Some(&json!({"pane": {"id": "1", "tab": "1"}})),
            "a terminal owner does not require a caller process"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case::removed(false)]
    #[case::deleting(true)]
    fn browser_terminal_owner_must_still_exist(
        #[case] deleting: bool,
    ) -> Result<(), serde_json::Error> {
        let app = if deleting {
            let mut app = state_with(
                vec![agent(1, AgentStatus::Idle), agent(2, AgentStatus::Idle)],
                1,
            );
            app.sessions.deleting.insert(SessionId(2));
            app
        } else {
            one_pane()
        };
        let line = json!({
            "id": "browser", "method": "getCurrentPane",
            "params": {"tty": "/dev/ttys002"}, "caller": {"pid": 42}
        });
        let reply = located(&line, app, vec![PaneId(2)])?;
        assert_eq!(
            reply.get("result"),
            Some(&json!({"pane": null})),
            "a removed or deleting terminal owner must not resolve"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn stale_terminal_owner_does_not_make_a_live_owner_ambiguous() -> Result<(), serde_json::Error>
    {
        let line = json!({
            "id": "browser", "method": "getCurrentPane",
            "params": {"tty": "/dev/ttys002"}, "caller": {"pid": 42}
        });
        let reply = located(&line, one_pane(), vec![PaneId(2), PaneId(1)])?;
        assert_eq!(
            reply.get("result"),
            Some(&json!({"pane": {"id": "1", "tab": "1"}})),
            "only distinct live owners make terminal lookup ambiguous"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn current_pane_of_a_process_outside_orb_is_null() -> Result<(), serde_json::Error> {
        // Given the caller runs in no orb pane.
        // When terminal-browser asks which pane it's in.
        let (reply, _) = exchange(&get_current_pane(), one_pane(), None, obliging)?;

        // Then orb answers no pane.
        assert_eq!(
            reply.get("result"),
            Some(&json!({"pane": null})),
            "a process outside orb has no pane"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn current_pane_orb_no_longer_has_is_null() -> Result<(), serde_json::Error> {
        // Given the caller says pane 7, which no session holds.
        // When terminal-browser asks which pane it's in.
        let (reply, _) = exchange(&get_current_pane(), one_pane(), Some(PaneId(7)), obliging)?;

        // Then orb answers no pane.
        assert_eq!(
            reply.get("result"),
            Some(&json!({"pane": null})),
            "a pane orb no longer has is no pane"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case("right", Direction::Right)]
    #[case("down", Direction::Down)]
    fn split_asks_orb_for_a_pane_beside_the_caller(
        #[case] side: &str,
        #[case] direction: Direction,
    ) -> Result<(), serde_json::Error> {
        // Given a split from pane 1.
        // When orb answers it.
        let (_, asked) = exchange(&split_line(side), one_pane(), None, obliging)?;

        // Then the first control request splits pane 1 toward that side.
        assert_eq!(
            asked.first(),
            Some(&Request {
                caller: None,
                op: Op::PaneSplit {
                    pane: Some(PaneId(1)),
                    direction,
                    name: None,
                    run: None,
                    cwd: None,
                },
            }),
            "the split should ask for a pane beside pane 1"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn split_types_the_quoted_command_into_the_new_pane() -> Result<(), serde_json::Error> {
        // Given a split whose new pane is pane 9.
        // When orb answers it.
        let (_, asked) = exchange(&split_line("right"), one_pane(), None, obliging)?;

        // Then the second control request runs the quoted command in pane 9.
        assert_eq!(
            asked.get(1),
            Some(&Request {
                caller: None,
                op: Op::PaneRun {
                    pane: PaneId(9),
                    command: r"'/opt/tb/bin/terminal-browser' 'open' 'https://x.test/?q=it'\''s'"
                        .to_owned(),
                    wait: false,
                    timeout: None,
                    force: false,
                    tail: None,
                },
            }),
            "the command should be typed, quoted, into the new pane"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn split_asks_orb_for_nothing_else() -> Result<(), serde_json::Error> {
        // Given a split.
        // When orb answers it.
        let (_, asked) = exchange(&split_line("right"), one_pane(), None, obliging)?;

        // Then only the split and the run reached orb: the keys stay put.
        assert_eq!(asked.len(), 2, "a split should be a split and a run only");
        Ok(())
    }

    #[rstest::rstest]
    fn split_replies_with_an_empty_result() -> Result<(), serde_json::Error> {
        // Given a split orb carries out.
        // When orb answers it.
        let (reply, _) = exchange(&split_line("right"), one_pane(), None, obliging)?;

        // Then the result is `{}`.
        assert_eq!(
            reply.get("result"),
            Some(&json!({})),
            "a split should reply with an empty result"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn split_refused_by_orb_relays_its_message() -> Result<(), serde_json::Error> {
        // Given orb refuses the split: the session is settled.
        // When orb answers it.
        let (reply, _) = exchange(&split_line("right"), one_pane(), None, settled)?;

        // Then terminal-browser gets orb's message.
        assert_eq!(
            reply.pointer("/error/message"),
            Some(&json!("session is settled")),
            "orb's refusal should be relayed"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn split_refused_by_orb_types_nothing() -> Result<(), serde_json::Error> {
        // Given orb refuses the split.
        // When orb answers it.
        let (_, asked) = exchange(&split_line("right"), one_pane(), None, settled)?;

        // Then no command was typed anywhere.
        assert!(
            !asked
                .iter()
                .any(|request| matches!(request.op, Op::PaneRun { .. })),
            "a refused split should type nothing"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case("left")]
    #[case("up")]
    fn split_to_the_left_or_up_is_refused(#[case] side: &str) -> Result<(), serde_json::Error> {
        // Given a split toward a side orb doesn't split to.
        // When orb answers it.
        let (reply, _) = exchange(&split_line(side), one_pane(), None, obliging)?;

        // Then it's an error, with no result.
        assert!(
            reply.get("error").is_some() && reply.get("result").is_none(),
            "only right and down splits should be carried out"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn focus_pane_asks_orb_to_focus_it() -> Result<(), serde_json::Error> {
        // Given terminal-browser asks to focus pane 4.
        let line = json!({"id": "r2", "method": "focusPane", "params": {"pane": "4"}, "caller": {"pid": 42, "cwd": "/work"}});

        // When orb answers it.
        let (_, asked) = exchange(&line, one_pane(), None, obliging)?;

        // Then orb was asked to focus pane 4, and nothing else.
        assert_eq!(
            asked,
            vec![Request {
                caller: None,
                op: Op::PaneFocus { pane: PaneId(4) },
            }],
            "focusPane should be a pane focus"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn unknown_method_is_refused_by_name() -> Result<(), serde_json::Error> {
        // Given terminal-browser asks for a method orb doesn't offer.
        let line = json!({"id": "r3", "method": "listPanes", "params": {"tty": null}, "caller": {"pid": 42, "cwd": "/work"}});

        // When orb answers it.
        let (reply, _) = exchange(&line, one_pane(), None, obliging)?;

        // Then the error names the method.
        assert_eq!(
            reply.pointer("/error/message"),
            Some(&json!("orb does not support listPanes")),
            "an unknown method should be refused by name"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn unreadable_line_gets_an_error() -> Result<(), serde_json::Error> {
        // Given a line that isn't a request.
        let text = reply(
            "not json",
            &State::new(one_pane()),
            &Fixed {
                process: None,
                terminals: Vec::new(),
            },
            &obliging,
        );

        // When it's read back.
        let reply: Value = serde_json::from_str(&text)?;

        // Then it's an error.
        assert!(
            reply.get("error").is_some(),
            "an unreadable line should get an error"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn hello_over_the_socket_lists_the_three_methods() -> Result<(), Report<ControlServerError>> {
        // Given orb's split socket listening in a fresh temp dir.
        let dir = tempfile::tempdir().change_context(ControlServerError)?;
        let socket = dir.path().join(SOCKET_FILE);
        let state = State::new(one_pane());
        open(&socket, Log::default(), move |line, _gone| {
            reply(
                line,
                &state,
                &Fixed {
                    process: None,
                    terminals: Vec::new(),
                },
                &obliging,
            )
        })?;

        // When terminal-browser says hello over it.
        let mut stream = UnixStream::connect(&socket).change_context(ControlServerError)?;
        writeln!(stream, "{}", hello()).change_context(ControlServerError)?;
        let mut line = String::new();
        BufReader::new(&stream)
            .read_line(&mut line)
            .change_context(ControlServerError)?;

        // Then the reply lists the three methods.
        let reply: Value = serde_json::from_str(&line).change_context(ControlServerError)?;
        assert_eq!(
            reply.pointer("/result/methods"),
            Some(&json!(["getCurrentPane", "split", "focusPane"])),
            "the socket should answer hello with the three methods"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case::ps_output("tb open x HOME=/h ORB_PANE_ID=5", 5)]
    #[case::proc_environ("HOME=/h\0ORB_PANE_ID=5\0", 5)]
    #[case::env_after_command("tb ORB_PANE_ID=1 HOME=/h ORB_PANE_ID=2", 2)]
    fn pane_in_reads_the_last_orb_pane_id(#[case] env: &str, #[case] pane: i64) {
        // Given / When reading a process's environment.
        // Then the pane is the last ORB_PANE_ID's.
        assert_eq!(
            pane_in(env),
            Some(PaneId(pane)),
            "the last ORB_PANE_ID should name the pane"
        );
    }

    #[rstest::rstest]
    #[case::absent("tb open HOME=/h")]
    #[case::longer_name("XORB_PANE_ID=5")]
    #[case::not_a_number("ORB_PANE_ID=x")]
    #[case::empty("")]
    fn pane_in_finds_no_pane(#[case] env: &str) {
        // Given / When reading an environment without a usable ORB_PANE_ID.
        // Then there's no pane.
        assert_eq!(pane_in(env), None, "no usable ORB_PANE_ID means no pane");
    }

    #[rstest::rstest]
    fn process_that_has_exited_has_no_pane() -> std::io::Result<()> {
        // Given a process that has exited.
        let mut child = std::process::Command::new("/usr/bin/true").spawn()?;
        wait_within(&mut child, Duration::from_secs(5))?;

        // When looking up its pane.
        let pane = LaunchEnv.pane_of(child.id());

        // Then there's none.
        assert_eq!(pane, None, "a process that has exited has no pane");
        Ok(())
    }
}
