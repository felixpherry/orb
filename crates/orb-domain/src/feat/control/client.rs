//! The `orb session|tab|pane|agent …` commands: turn the words typed into one
//! request, send it to the running orb, print its reply and pick the exit code
//! (0 done, 1 refused or orb not running, 2 typed wrong).

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use crate::feat::control::protocol::{ApiError, Direction, ErrorCode, Op, Reply, Request};
use crate::ids::{PaneId, ProjectId, SessionId};

/// The first arguments that make `orb` a client.
pub const NOUNS: [&str; 4] = ["session", "tab", "pane", "agent"];

/// What the commands accept, shown when they're typed wrong.
const USAGE: &str = "usage:
  orb session list
  orb tab list [--session <id>]
  orb pane list [--session <id>]
  orb agent list
  orb session create [--project <id>] [--worktree] [--name <text>]
  orb tab create [--session <id>]
  orb pane split [<pane>] --direction right|down
  orb pane stack [<pane>]
  orb pane float [--session <id>]
  orb pane close <pane>
  orb pane rename <pane> <name>
  orb pane focus <pane>
  orb tab close [--pane <id>]
  orb session rename [<session>] <name>
  orb session settle [<session>]
  orb session focus <session>
  orb pane send <pane> <text>
  orb pane keys <pane> <key>...
  orb pane run <pane> <command>
  orb pane read [<pane>] [--lines <n>]";

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
            session: session_only(rest).ok()?,
        }),
        ("pane", "list", rest) => Some(Op::PaneList {
            session: session_only(rest).ok()?,
        }),
        ("pane", "float", rest) => Some(Op::PaneFloat {
            session: session_only(rest).ok()?,
        }),
        ("tab", "create", rest) => Some(Op::TabCreate {
            session: session_only(rest).ok()?,
        }),
        ("pane", "split", rest) => {
            let words = Words::split(rest, &[("--direction", true)])?;
            let direction = match words.value("--direction")? {
                "right" => Direction::Right,
                "down" => Direction::Down,
                _ => return None,
            };
            Some(Op::PaneSplit {
                pane: words.target().ok()?.map(PaneId),
                direction,
            })
        }
        ("pane", "stack", rest) => Some(Op::PaneStack {
            pane: Words::split(rest, &[])?.target().ok()?.map(PaneId),
        }),
        ("session", "create", rest) => {
            let words = Words::split(
                rest,
                &[("--project", true), ("--worktree", false), ("--name", true)],
            )?;
            if !words.positional.is_empty() {
                return None;
            }
            Some(Op::SessionCreate {
                project: words.id("--project").ok()?.map(ProjectId),
                worktree: words.flags.contains_key("--worktree"),
                name: words.value("--name").map(str::to_owned),
            })
        }
        ("pane", "close", rest) => Some(Op::PaneClose {
            pane: PaneId(one_id(rest)?),
        }),
        ("pane", "focus", rest) => Some(Op::PaneFocus {
            pane: PaneId(one_id(rest)?),
        }),
        ("pane", "rename", [pane, name]) => Some(Op::PaneRename {
            pane: PaneId(number(pane).ok()?),
            name: (*name).to_owned(),
        }),
        ("tab", "close", rest) => {
            let words = Words::split(rest, &[("--pane", true)])?;
            if !words.positional.is_empty() {
                return None;
            }
            Some(Op::TabClose {
                pane: words.id("--pane").ok()?.map(PaneId),
            })
        }
        ("session", "rename", [name]) => Some(Op::SessionRename {
            session: None,
            name: (*name).to_owned(),
        }),
        ("session", "rename", [session, name]) => Some(Op::SessionRename {
            session: Some(SessionId(number(session).ok()?)),
            name: (*name).to_owned(),
        }),
        ("session", "settle", rest) => Some(Op::SessionSettle {
            session: Words::split(rest, &[])?.target().ok()?.map(SessionId),
        }),
        ("session", "focus", rest) => Some(Op::SessionFocus {
            session: SessionId(one_id(rest)?),
        }),
        ("pane", "send" | "keys" | "run" | "read", rest) => pane_io(verb, rest),
        _ => None,
    }
}

/// The pane input or output `verb` and its words ask for.
fn pane_io(verb: &str, rest: &[&str]) -> Option<Op> {
    match (verb, rest) {
        ("send", [pane, text]) => Some(Op::PaneSend {
            pane: PaneId(number(pane).ok()?),
            text: (*text).to_owned(),
        }),
        ("run", [pane, command]) => Some(Op::PaneRun {
            pane: PaneId(number(pane).ok()?),
            command: (*command).to_owned(),
        }),
        ("keys", [pane, keys @ ..]) if !keys.is_empty() => Some(Op::PaneKeys {
            pane: PaneId(number(pane).ok()?),
            keys: keys.iter().map(|key| (*key).to_owned()).collect(),
        }),
        ("read", rest) => {
            let words = Words::split(rest, &[("--lines", true)])?;
            Some(Op::PaneRead {
                pane: words.target().ok()?.map(PaneId),
                lines: words
                    .value("--lines")
                    .map(str::parse)
                    .transpose()
                    .ok()?
                    .unwrap_or(0),
            })
        }
        _ => None,
    }
}

/// Words that must be exactly one ID.
fn one_id(rest: &[&str]) -> Option<i64> {
    Words::split(rest, &[])?.target().ok().flatten()
}

/// Words that may only hold `--session <id>`: the ID, or `Ok(None)` when
/// it's absent; `Err` when they hold anything else.
fn session_only(rest: &[&str]) -> Result<Option<SessionId>, ()> {
    let words = Words::split(rest, &[("--session", true)]).ok_or(())?;
    if !words.positional.is_empty() {
        return Err(());
    }
    Ok(words.id("--session")?.map(SessionId))
}

/// The words after a command's verb: its positionals and its `--flag`s.
#[derive(Debug, Default)]
struct Words<'a> {
    positional: Vec<&'a str>,
    /// Each flag given, with its value when it takes one.
    flags: HashMap<&'a str, Option<&'a str>>,
}

impl<'a> Words<'a> {
    /// Splits `rest` given the `known` flags, each marked as taking a value or
    /// not; `None` for an unknown flag, a repeat or a missing value.
    fn split(rest: &[&'a str], known: &[(&str, bool)]) -> Option<Self> {
        let mut words = Self::default();
        let mut rest = rest.iter().copied();
        while let Some(word) = rest.next() {
            if !word.starts_with("--") {
                words.positional.push(word);
                continue;
            }
            let &(_, takes_value) = known.iter().find(|&&(flag, _)| flag == word)?;
            let value = if takes_value {
                Some(rest.next()?)
            } else {
                None
            };
            if words.flags.insert(word, value).is_some() {
                return None;
            }
        }
        Some(words)
    }

    /// Flag `flag`'s value, if it was given one.
    fn value(&self, flag: &str) -> Option<&'a str> {
        self.flags.get(flag).copied().flatten()
    }

    /// Flag `flag`'s value as an ID: `Ok(None)` when absent, `Err` when it
    /// isn't a number.
    fn id(&self, flag: &str) -> Result<Option<i64>, ()> {
        self.value(flag).map(number).transpose()
    }

    /// The one optional positional, as an ID: `Ok(None)` when there is none,
    /// `Err` when it isn't a number or there are more.
    fn target(&self) -> Result<Option<i64>, ()> {
        match self.positional.as_slice() {
            [] => Ok(None),
            [id] => number(id).map(Some),
            _ => Err(()),
        }
    }
}

/// `word` as an ID number.
fn number(word: &str) -> Result<i64, ()> {
    word.parse().map_err(drop)
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

    use super::{parse, run};
    use crate::feat::control::protocol::{Direction, Op};
    use crate::ids::PaneId;

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

    #[rstest::rstest]
    fn split_words_name_the_pane_and_direction() {
        // Given / When parsing `pane split 3 --direction down`.
        let op = parse(&args(&["pane", "split", "3", "--direction", "down"]));

        // Then it asks to split pane 3 down.
        assert_eq!(
            op,
            Some(Op::PaneSplit {
                pane: Some(PaneId(3)),
                direction: Direction::Down,
            }),
            "the pane and direction come from the words"
        );
    }

    #[rstest::rstest]
    #[case::send(
        &["pane", "send", "3", "--x y"],
        Op::PaneSend { pane: PaneId(3), text: "--x y".into() },
    )]
    #[case::keys(
        &["pane", "keys", "3", "esc", "enter"],
        Op::PaneKeys { pane: PaneId(3), keys: vec!["esc".into(), "enter".into()] },
    )]
    #[case::run(
        &["pane", "run", "3", "just test"],
        Op::PaneRun { pane: PaneId(3), command: "just test".into() },
    )]
    #[case::read_own(&["pane", "read", "--lines", "5"], Op::PaneRead { pane: None, lines: 5 })]
    #[case::read_named(&["pane", "read", "3"], Op::PaneRead { pane: Some(PaneId(3)), lines: 0 })]
    fn pane_io_words_make_their_request(#[case] words: &[&str], #[case] expected: Op) {
        // Given / When parsing the words.
        let op = parse(&args(words));

        // Then they make the request they spell.
        assert_eq!(op, Some(expected), "{words:?} parse to their request");
    }

    #[rstest::rstest]
    fn pane_keys_without_a_key_is_a_usage_error() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given a socket path nothing listens on.
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("orb.sock");
        let (mut out, mut err) = (Vec::new(), Vec::new());

        // When running `pane keys 3` with no key.
        let code = run(
            &args(&["pane", "keys", "3"]),
            None,
            &socket,
            &mut out,
            &mut err,
        );

        // Then it exits 2 with the usage.
        assert_eq!(code, 2, "keys need at least one key name");
        Ok(())
    }
}
