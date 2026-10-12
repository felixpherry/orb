//! The `orb session|tab|pane|agent …` commands: turn the words typed into one
//! request, send it to the running orb, print its reply and pick the exit code
//! (0 done, 1 refused or orb not running, 2 typed wrong).

use std::ffi::OsString;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use crate::feat::control::protocol::{ApiError, ErrorCode, Op, Reply, Request};
use crate::ids::{PaneId, SessionId};

/// The first arguments that make `orb` a client.
pub const NOUNS: [&str; 4] = ["session", "tab", "pane", "agent"];

/// What the commands accept, shown when they're typed wrong.
const USAGE: &str = "usage:
  orb session list
  orb tab list [--session <id>]
  orb pane list [--session <id>]
  orb agent list";

/// Runs `orb <noun> <verb> …` against the orb listening on `socket`, as the
/// pane `caller` when it runs inside orb. The reply's result goes to `out`, an
/// error or the usage to `err`; returns the exit code.
pub fn run<O, E>(
    args: &[OsString],
    caller: Option<PaneId>,
    socket: &Path,
    out: &mut O,
    err: &mut E,
) -> u8
where
    O: Write,
    E: Write,
{
    let Some(op) = parse(args) else {
        let _ = writeln!(err, "{USAGE}");
        return 2;
    };
    match ask(socket, &Request { caller, op }) {
        Ok(Reply {
            ok: true, result, ..
        }) => {
            let result = result.unwrap_or(serde_json::Value::Null);
            let _ = writeln!(out, "{result}");
            0
        }
        Ok(Reply { error, .. }) => {
            let error = error.unwrap_or_else(|| ApiError::new(ErrorCode::Failed, "no error given"));
            print_error(err, &error);
            1
        }
        Err(error) => {
            print_error(err, &error);
            1
        }
    }
}

/// The request the words ask for; `None` when they don't make one.
fn parse(args: &[OsString]) -> Option<Op> {
    let words: Vec<&str> = args.iter().map(|arg| arg.to_str()).collect::<Option<_>>()?;
    let (noun, verb, rest) = match words.as_slice() {
        [noun, verb, rest @ ..] => (*noun, *verb, rest),
        _ => return None,
    };
    match (noun, verb, rest) {
        ("session", "list", []) => Some(Op::SessionList),
        ("agent", "list", []) => Some(Op::AgentList),
        ("tab", "list", rest) => Some(Op::TabList {
            session: session_flag(rest).ok()?,
        }),
        ("pane", "list", rest) => Some(Op::PaneList {
            session: session_flag(rest).ok()?,
        }),
        _ => None,
    }
}

/// The optional `--session <id>`: `Ok(None)` when absent, `Err` when the
/// words are anything else.
fn session_flag(rest: &[&str]) -> Result<Option<SessionId>, ()> {
    match rest {
        [] => Ok(None),
        ["--session", id] => id.parse().map(|id| Some(SessionId(id))).map_err(drop),
        _ => Err(()),
    }
}

/// Sends `request` over one connection and reads the one reply line.
fn ask(socket: &Path, request: &Request) -> Result<Reply, ApiError> {
    let failed =
        |error: &dyn std::fmt::Display| ApiError::new(ErrorCode::Failed, error.to_string());
    let mut stream = UnixStream::connect(socket).map_err(|error| match error.kind() {
        ErrorKind::NotFound | ErrorKind::ConnectionRefused => {
            ApiError::new(ErrorCode::NotRunning, "orb is not running")
        }
        _ => failed(&error),
    })?;
    let line = serde_json::to_string(request).map_err(|error| failed(&error))?;
    writeln!(stream, "{line}").map_err(|error| failed(&error))?;
    let mut reply = String::new();
    BufReader::new(&stream)
        .read_line(&mut reply)
        .map_err(|error| failed(&error))?;
    serde_json::from_str(&reply)
        .map_err(|error| failed(&format!("orb sent no readable reply: {error}")))
}

/// Writes `error` as one JSON line.
fn print_error<E>(err: &mut E, error: &ApiError)
where
    E: Write,
{
    if let Ok(text) = serde_json::to_string(error) {
        let _ = writeln!(err, "{text}");
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate setup failures with `?` and assert on the outcome"
)]
mod tests {
    use std::error::Error;
    use std::ffi::OsString;

    use super::run;

    /// `words` as command-line arguments.
    fn args(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    #[rstest::rstest]
    fn client_reports_not_running_when_nothing_listens() -> Result<(), Box<dyn Error + Send + Sync>>
    {
        // Given a socket path nothing listens on.
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("orb.sock");
        let (mut out, mut err) = (Vec::new(), Vec::new());

        // When running `pane list`.
        let code = run(&args(&["pane", "list"]), None, &socket, &mut out, &mut err);

        // Then it exits 1 with not_running.
        let error: serde_json::Value = serde_json::from_slice(&err)?;
        assert_eq!(
            (code, error.get("code").and_then(serde_json::Value::as_str)),
            (1, Some("not_running")),
            "no listener means not_running, exit 1"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn unknown_verb_is_a_usage_error() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given a socket path nothing listens on.
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("orb.sock");
        let (mut out, mut err) = (Vec::new(), Vec::new());

        // When running a command that doesn't exist.
        let code = run(
            &args(&["session", "delete"]),
            None,
            &socket,
            &mut out,
            &mut err,
        );

        // Then it exits 2 with the usage, without trying to connect.
        let err = String::from_utf8_lossy(&err);
        assert!(
            code == 2 && err.starts_with("usage:") && !err.contains("not_running"),
            "an unknown verb prints the usage and exits 2, got {code}: {err}"
        );
        Ok(())
    }
}
