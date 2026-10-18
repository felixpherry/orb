//! The `orb session|tab|pane|agent|plate|project …` commands: turn the
//! words typed into one request, send it to the running orb, print its
//! reply (JSON for data, plain text for a pane's screen, a command's
//! output, a line waited for and an agent's answer) and pick the exit code
//! (0 done, 1 refused or orb not running, 2 typed wrong). `orb help` and
//! `--help` print help instead, with no orb needed. Bare `orb` in an orb
//! pane says which pane, tab and session it is.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use serde::Deserialize;
use wherror::Error;

use crate::feat::control::agent::Timing;
use crate::feat::control::help::{self, CommandHelp};
use crate::feat::control::pane_ops::last_line;
use crate::feat::control::protocol::{
    AgentKind, ApiError, Direction, ErrorCode, HereResult, Op, Reply, Request, RunResult,
};
use crate::feat::sessions::state::AgentStatus;
use crate::ids::{PaneId, PlateItemId, ProjectId, SessionId};

/// What `orb` does with the words it was started with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// `--version` or `-V` first: print the version.
    Version,
    /// Start orb: no words outside an orb pane, `--upgrade-data` alone, or `integration …`.
    App,
    /// No words inside an orb pane: say where the caller is.
    Home,
    /// Anything else: help, a command, or a usage error.
    Client,
}

/// What `args` ask `orb` to do, `in_pane` when it runs inside an orb pane.
pub fn entry(args: &[OsString], in_pane: bool) -> Entry {
    let Some(first) = args.first() else {
        return if in_pane { Entry::Home } else { Entry::App };
    };
    if first == "--version" || first == "-V" {
        Entry::Version
    } else if first == "integration" || (first == "--upgrade-data" && args.len() == 1) {
        Entry::App
    } else {
        Entry::Client
    }
}

/// Runs `orb <noun> <verb> …` against the orb listening on `socket`, as the
/// pane `caller` when it runs inside orb. The reply's result (plain text for a
/// pane's screen, a waited-on command's output, an agent's reply and its
/// answer to a waited-on prompt unless `--json` is given, else JSON) or the
/// help asked for goes to `out`, an error or a usage error to `err`; returns
/// the exit code.
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
    let words: Option<Vec<&str>> = args.iter().map(|arg| arg.to_str()).collect();
    match words.as_deref().and_then(asked_help) {
        Some(Ok(text)) => {
            let _ = writeln!(out, "{text}");
            return 0;
        }
        Some(Err(error)) => {
            let _ = write!(err, "{}", error.text());
            return 2;
        }
        None => {}
    }
    let op = match parse(args) {
        Ok(op) => op,
        Err(error) => {
            let _ = write!(err, "{}", error.text());
            return 2;
        }
    };
    let limit = reply_limit(&op);
    let as_text = !words.as_deref().is_some_and(asked_json);
    let request = Request { caller, op };
    match ask(socket, &request, limit).and_then(Reply::into_result) {
        Ok(result) => {
            let text = text_of(&request.op, &result)
                .filter(|_| as_text)
                .unwrap_or_else(|| result.to_string());
            let _ = writeln!(out, "{text}");
            0
        }
        Err(error) => {
            print_error(err, &error);
            1
        }
    }
}

/// `op`'s reply `result` as plain text, printed unless `--json` is given: a
/// screen's or an answer's text, a waited-on command's output and exit, the
/// line a wait matched; `None` for any other reply, or one not of its shape.
fn text_of(op: &Op, result: &serde_json::Value) -> Option<String> {
    match op {
        Op::PaneRead { .. } | Op::AgentReply { .. } | Op::AgentPrompt { wait: true, .. } => result
            .get("text")?
            .as_str()
            .map(|text| text.trim_end_matches('\n').to_owned()),
        Op::PaneWait { .. } => result.get("line")?.as_str().map(str::to_owned),
        Op::PaneRun {
            pane, wait: true, ..
        } => RunResult::deserialize(result)
            .ok()
            .map(|run| run_text(*pane, &run)),
        _ => None,
    }
}

/// A waited-on command's output in pane `pane` as lines: a note when its
/// start was gone from the history, a note naming how many earlier lines were
/// cut and how to read them, the output, then `[exit n]` (`[exit ?]` when
/// the shell sends no prompt marks).
fn run_text(pane: PaneId, run: &RunResult) -> String {
    let shown = run.output.lines().count();
    let mut lines = Vec::new();
    if run.truncated {
        lines.push("[… earlier output is gone from the pane's history]".to_owned());
    }
    if run.lines > shown {
        lines.push(format!(
            "[… {} earlier lines; orb pane read {} --lines {}]",
            run.lines - shown,
            pane.0,
            run.lines
        ));
    }
    if !run.output.is_empty() {
        lines.push(run.output.clone());
    }
    lines.push(
        run.exit
            .map_or_else(|| "[exit ?]".to_owned(), |exit| format!("[exit {exit}]")),
    );
    lines.join("\n")
}

/// Whether `--json` is among `words` before `--`.
fn asked_json(words: &[&str]) -> bool {
    before_dash(words).contains(&"--json")
}

/// The words before the first `--`, or all of them without one.
fn before_dash<'a>(words: &'a [&'a str]) -> &'a [&'a str] {
    words.split(|word| *word == "--").next().unwrap_or_default()
}

/// Bare `orb` in pane `caller`: asks the orb on `socket` where the caller
/// is (`orb.here`) and prints it, returning the exit code; `None` when no
/// orb answers (`not_running`), so the caller starts orb.
pub fn home<O, E>(caller: PaneId, socket: &Path, out: &mut O, err: &mut E) -> Option<u8>
where
    O: Write,
    E: Write,
{
    let request = Request {
        caller: Some(caller),
        op: Op::Here,
    };
    let answer = match ask(socket, &request, reply_limit(&request.op)) {
        Err(error) if error.code == ErrorCode::NotRunning => return None,
        asked => asked.and_then(Reply::into_result),
    };
    let refusal = match answer.map(serde_json::from_value::<HereResult>) {
        Ok(Ok(here)) => {
            let _ = writeln!(out, "{}", home_text(&here));
            return Some(0);
        }
        Ok(Err(error)) => ApiError::failed(format!("orb sent no readable place: {error}")),
        Err(error) => error,
    };
    print_error(err, &refusal);
    Some(1)
}

/// The home view's last line: where help is.
const HOME_HELP: &str = "help: orb help · orb pane --help · orb agent --help";

/// The home view: where `here` is, then where help is.
fn home_text(here: &HereResult) -> String {
    let place = here
        .tab
        .map_or_else(|| "floating".to_owned(), |tab| format!("tab {tab}"));
    format!(
        "orb: you're in pane {} ({place}) of session {} \"{}\".\n{HOME_HELP}",
        here.pane.0, here.session.0, here.session_name
    )
}

/// Why the words typed make no request; printed with that command's usage
/// line and help line, exit 2.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error(debug)]
pub(crate) struct UsageError {
    /// The noun, once the first word named one.
    pub noun: Option<&'static str>,
    /// The verb, once the second word named one of the noun's.
    pub verb: Option<&'static str>,
    pub problem: UsageProblem,
}

/// What was wrong with the words typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UsageProblem {
    /// There were no words.
    NoCommand,
    /// A word isn't valid UTF-8.
    NotText,
    /// The first word names no noun.
    UnknownNoun {
        word: String,
        suggestion: Option<&'static str>,
    },
    /// The noun came with no verb.
    MissingVerb,
    /// The second word is none of the noun's verbs.
    UnknownVerb {
        word: String,
        suggestion: Option<&'static str>,
    },
    /// A `--` word the command doesn't take.
    UnknownFlag {
        flag: String,
        suggestion: Option<&'static str>,
    },
    /// A flag that takes a value came last.
    MissingValue { flag: &'static str },
    /// A flag's value or a positional that should be a number isn't.
    NotANumber { what: &'static str, got: String },
    /// A flag's value is none of those it allows.
    NotInSet { flag: &'static str, got: String },
    /// A flag that may come once came twice.
    Repeated { flag: &'static str },
    /// A flag the command needs was left out.
    MissingFlag { flag: &'static str },
    /// A positional the command needs was left out.
    MissingWord { what: &'static str },
    /// A word past those the command takes.
    ExtraWord { word: String },
    /// Two flags that rule each other out came together.
    Exclusive {
        one: &'static str,
        other: &'static str,
    },
    /// A flag came without the one it goes with.
    Needs {
        flag: &'static str,
        needs: &'static str,
    },
    /// `--regex`'s value isn't a pattern, `why` says what's wrong.
    BadPattern { got: String, why: String },
}

impl UsageError {
    /// The three lines printed on stderr: `error: …`, `usage: …`, `help: …`.
    pub(crate) fn text(&self) -> String {
        let (usage, help) = match (self.noun, self.verb) {
            (Some(noun), Some(verb)) => (
                help::command(noun, verb).map_or_else(
                    || format!("orb {noun} {verb}"),
                    |command| command.usage.to_owned(),
                ),
                format!("orb {noun} {verb} --help"),
            ),
            (Some(noun), None) => (format!("orb {noun} <verb> …"), format!("orb {noun} --help")),
            (None, _) => {
                let nouns = help::nouns();
                (
                    format!("orb {} <verb> …", nouns.join("|")),
                    "orb help".to_owned(),
                )
            }
        };
        format!("error: {}\nusage: {usage}\nhelp: {help}\n", self.message())
    }

    /// The `error:` line's text: what was wrong, naming the word.
    fn message(&self) -> String {
        let command = format!(
            "`{}`",
            ["orb"]
                .into_iter()
                .chain(self.noun)
                .chain(self.verb)
                .collect::<Vec<_>>()
                .join(" ")
        );
        match &self.problem {
            UsageProblem::NoCommand => format!("orb needs a command: {}", noun_list()),
            UsageProblem::NotText => "a word isn't valid UTF-8 text".to_owned(),
            UsageProblem::UnknownNoun { word, suggestion } => {
                format!("unknown command {word}{}", did_you_mean(*suggestion))
            }
            UsageProblem::MissingVerb => format!("{command} needs a verb: {}", self.verb_list()),
            UsageProblem::UnknownVerb { word, suggestion } => format!(
                "unknown verb {word} for {command}{}; its verbs are {}",
                did_you_mean(*suggestion),
                self.verb_list()
            ),
            UsageProblem::UnknownFlag { flag, suggestion } => {
                format!(
                    "unknown flag {flag} for {command}{}",
                    did_you_mean(*suggestion)
                )
            }
            UsageProblem::MissingValue { flag } => {
                format!(
                    "{flag} needs a value ({})",
                    self.shape(flag).unwrap_or("a word")
                )
            }
            UsageProblem::NotANumber { what, got } => format!("{what} takes a number, got {got}"),
            UsageProblem::NotInSet { flag, got } => {
                let allowed: Vec<&str> = self.shape(flag).unwrap_or_default().split('|').collect();
                format!("{flag} is {}, got {got}", either_of(&allowed))
            }
            UsageProblem::Repeated { flag } => format!("{flag} is given twice"),
            UsageProblem::MissingFlag { flag } => match self.shape(flag) {
                Some(shape) => format!("{command} needs {flag} {shape}"),
                None => format!("{command} needs {flag}"),
            },
            UsageProblem::MissingWord { what } => format!("{command} needs {what}"),
            UsageProblem::ExtraWord { word } => format!("unexpected word {word} for {command}"),
            UsageProblem::Exclusive { one, other } => {
                format!("{one} and {other} can't go together")
            }
            UsageProblem::Needs { flag, needs } => format!("{flag} only goes with {needs}"),
            UsageProblem::BadPattern { got, why } => format!("--regex can't take {got}: {why}"),
        }
    }

    /// The noun's verbs, comma-separated.
    fn verb_list(&self) -> String {
        help::verbs(self.noun.unwrap_or_default()).join(", ")
    }

    /// What flag `flag`'s value looks like, as the command's table row says.
    fn shape(&self, flag: &str) -> Option<&'static str> {
        help::command(self.noun?, self.verb?)?
            .params
            .iter()
            .find(|param| param.name == flag)?
            .value
    }
}

/// The nouns, comma-separated.
fn noun_list() -> String {
    help::nouns().join(", ")
}

/// ` (did you mean …?)` for a suggestion, nothing without one.
fn did_you_mean(suggestion: Option<&str>) -> String {
    suggestion
        .map(|known| format!(" (did you mean {known}?)"))
        .unwrap_or_default()
}

/// `words` as a choice: `a`, `a or b`, `a, b or c`.
fn either_of(words: &[&str]) -> String {
    match words.split_last() {
        Some((last, [])) => (*last).to_owned(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// The help the words ask for: `Some(Ok(text))` for `help …`, a first word
/// `--help`/`-h`, or `--help` among the words before `--`; `Some(Err)` when
/// they name an unknown noun or verb; `None` when they ask for no help.
fn asked_help(words: &[&str]) -> Option<Result<String, UsageError>> {
    match words {
        ["help", rest @ ..] => Some(help_word(rest)),
        ["--help" | "-h", ..] => Some(Ok(help::overview())),
        _ => {
            let before = before_dash(words);
            before.contains(&"--help").then(|| help_flag(before))
        }
    }
}

/// `orb help [<noun> [<verb>]]`: the overview, a noun's help or a command's.
/// A `--help` after `help` adds nothing, so it's left out.
fn help_word(rest: &[&str]) -> Result<String, UsageError> {
    let rest: Vec<&str> = rest
        .iter()
        .copied()
        .filter(|word| *word != "--help")
        .collect();
    match rest.as_slice() {
        [] => Ok(help::overview()),
        [noun] => Ok(help::noun_help(named_noun(noun)?)),
        [noun, verb, extra @ ..] => {
            let noun = named_noun(noun)?;
            let command = named_command(noun, verb)?;
            match extra.first() {
                Some(word) => Err(UsageError {
                    noun: Some(noun),
                    verb: Some(command.verb),
                    problem: extra_word(word),
                }),
                None => Ok(help::command_help(command)),
            }
        }
    }
}

/// `--help` among `words`: the help of the command or noun they start with;
/// every other word is ignored.
fn help_flag(words: &[&str]) -> Result<String, UsageError> {
    let Some((first, rest)) = words.split_first() else {
        return Ok(help::overview());
    };
    let noun = named_noun(first)?;
    match rest.first() {
        None | Some(&"--help") => Ok(help::noun_help(noun)),
        Some(verb) => Ok(help::command_help(named_command(noun, verb)?)),
    }
}

/// The noun `word` names, or the usage error saying it names none.
fn named_noun(word: &str) -> Result<&'static str, UsageError> {
    help::noun(word).ok_or_else(|| UsageError {
        noun: None,
        verb: None,
        problem: UsageProblem::UnknownNoun {
            word: word.to_owned(),
            suggestion: help::suggest(word, help::nouns()),
        },
    })
}

/// The command `noun verb` names, or the usage error saying `verb` is none
/// of `noun`'s.
fn named_command(noun: &'static str, verb: &str) -> Result<&'static CommandHelp, UsageError> {
    help::command(noun, verb).ok_or_else(|| UsageError {
        noun: Some(noun),
        verb: None,
        problem: UsageProblem::UnknownVerb {
            word: verb.to_owned(),
            suggestion: help::suggest(verb, help::verbs(noun)),
        },
    })
}

/// The request the words ask for, or the usage error naming what's wrong.
pub(super) fn parse(args: &[OsString]) -> Result<Op, UsageError> {
    let unnamed = |problem| UsageError {
        noun: None,
        verb: None,
        problem,
    };
    let words: Vec<&str> = args
        .iter()
        .map(|arg| arg.to_str())
        .collect::<Option<_>>()
        .ok_or_else(|| unnamed(UsageProblem::NotText))?;
    let Some((&first, rest)) = words.split_first() else {
        return Err(unnamed(UsageProblem::NoCommand));
    };
    let noun = named_noun(first)?;
    let Some((&verb, rest)) = rest.split_first() else {
        return Err(UsageError {
            noun: Some(noun),
            verb: None,
            problem: UsageProblem::MissingVerb,
        });
    };
    let command = named_command(noun, verb)?;
    build(command, rest).map_err(|problem| UsageError {
        noun: Some(noun),
        verb: Some(command.verb),
        problem,
    })
}

/// The request `command` makes from the words after its verb.
fn build(command: &'static CommandHelp, rest: &[&str]) -> Result<Op, UsageProblem> {
    let words = || Words::split(rest, command);
    Ok(match (command.noun, command.verb) {
        (noun, "list") if noun != "plate" => list_words(command, noun, rest)?,
        (noun @ "pane", verb @ ("split" | "stack" | "float")) | (noun @ "tab", verb @ "create") => {
            add_words(command, noun, verb, rest)?
        }
        ("session", "create") => {
            let words = words()?;
            words.no_positional()?;
            Op::SessionCreate {
                project: words.number("--project")?.map(ProjectId),
                worktree: words.switch("--worktree"),
                name: words.value("--name").map(str::to_owned),
            }
        }
        ("pane", "close") => Op::PaneClose {
            pane: PaneId(words()?.one("<pane>")?),
        },
        ("pane", "focus") => Op::PaneFocus {
            pane: PaneId(words()?.one("<pane>")?),
        },
        ("pane", "rename") => {
            let [pane, name] = exact(rest, ["<pane>", "<name>"])?;
            Op::PaneRename {
                pane: PaneId(number("<pane>", pane)?),
                name: name.to_owned(),
            }
        }
        ("tab", "close") => {
            let words = words()?;
            words.no_positional()?;
            Op::TabClose {
                pane: words.number("--pane")?.map(PaneId),
            }
        }
        ("session", "rename") => match rest {
            [] => return Err(UsageProblem::MissingWord { what: "<name>" }),
            [name] => Op::SessionRename {
                session: None,
                name: (*name).to_owned(),
            },
            [session, name] => Op::SessionRename {
                session: Some(SessionId(number("<session>", session)?)),
                name: (*name).to_owned(),
            },
            [_, _, extra, ..] => return Err(extra_word(extra)),
        },
        ("session", "settle") => Op::SessionSettle {
            session: words()?.target("<session>")?.map(SessionId),
        },
        ("session", "focus") => Op::SessionFocus {
            session: SessionId(words()?.one("<session>")?),
        },
        ("pane", verb) => pane_io(command, verb, rest)?,
        ("agent", verb) => agent_words(command, verb, rest)?,
        ("plate", verb) => plate_words(command, verb, rest)?,
        (_, verb) => return Err(unknown_verb(verb)),
    })
}

/// The list request of `noun` its words ask for.
fn list_words(
    command: &'static CommandHelp,
    noun: &str,
    rest: &[&str],
) -> Result<Op, UsageProblem> {
    let words = Words::split(rest, command)?;
    Ok(match noun {
        "session" => {
            words.no_positional()?;
            Op::SessionList {
                all: words.switch("--all"),
                project: words.number("--project")?.map(ProjectId),
            }
        }
        "agent" => {
            words.no_positional()?;
            Op::AgentList {
                all: words.switch("--all"),
                session: words.number("--session")?.map(SessionId),
                status: statuses(words.value("--status"))?,
            }
        }
        "project" => {
            words.no_positional()?;
            Op::ProjectList
        }
        "tab" => Op::TabList {
            session: session_only(&words)?,
        },
        "pane" => Op::PaneList {
            session: session_only(&words)?,
        },
        _ => return Err(unknown_verb("list")),
    })
}

/// `UnknownVerb` for a verb the table lists but the parser doesn't build.
fn unknown_verb(verb: &str) -> UsageProblem {
    UsageProblem::UnknownVerb {
        word: verb.to_owned(),
        suggestion: None,
    }
}

/// `ExtraWord` for `word`.
fn extra_word(word: &str) -> UsageProblem {
    UsageProblem::ExtraWord {
        word: word.to_owned(),
    }
}

/// The plate request `verb` and its words ask for.
fn plate_words(
    command: &'static CommandHelp,
    verb: &str,
    rest: &[&str],
) -> Result<Op, UsageProblem> {
    let words = || Words::split(rest, command);
    Ok(match verb {
        "add" => {
            let words = words()?;
            Op::PlateAdd {
                title: words.only("<title>")?.to_owned(),
                verb: words.required("--verb")?.to_owned(),
                what: words.value("--what").map(str::to_owned),
                links: owned(&words.values("--link")),
                urgent: words.switch("--urgent"),
                waiting: words.switch("--waiting"),
                pane: words.number("--pane")?.map(PaneId),
            }
        }
        "update" => {
            let words = words()?;
            Op::PlateUpdate {
                item: PlateItemId(words.one("<item>")?),
                title: words.value("--title").map(str::to_owned),
                verb: words.value("--verb").map(str::to_owned),
                what: words.value("--what").map(str::to_owned),
                links: words
                    .lists
                    .contains_key("--link")
                    .then(|| owned(&words.values("--link"))),
                urgent: words.either("--urgent", "--not-urgent")?,
                waiting: words.either("--waiting", "--your-move")?,
            }
        }
        "done" => Op::PlateDone {
            item: PlateItemId(words()?.one("<item>")?),
        },
        "list" => {
            words()?.no_positional()?;
            Op::PlateList
        }
        _ => return Err(unknown_verb(verb)),
    })
}

/// `words`, each as an owned string.
fn owned(words: &[&str]) -> Vec<String> {
    words.iter().map(|&word| word.to_owned()).collect()
}

/// The agent request `verb` and its words ask for.
fn agent_words(
    command: &'static CommandHelp,
    verb: &str,
    rest: &[&str],
) -> Result<Op, UsageProblem> {
    let words = || Words::split(rest, command);
    Ok(match verb {
        "start" => {
            let words = words()?;
            words.no_positional()?;
            let kind = match words.required("--kind")? {
                "claude" => AgentKind::Claude,
                "pi" => AgentKind::Pi,
                got => {
                    return Err(UsageProblem::NotInSet {
                        flag: "--kind",
                        got: got.to_owned(),
                    });
                }
            };
            words.exclusive(&["--pane", "--split", "--tab"])?;
            words.exclusive(&["--split", "--force"])?;
            words.exclusive(&["--tab", "--force"])?;
            Op::AgentStart {
                pane: words.number("--pane")?.map(PaneId),
                kind,
                name: words.value("--name").map(str::to_owned),
                args: owned(&words.after),
                split: words
                    .value("--split")
                    .map(|word| direction("--split", word))
                    .transpose()?,
                tab: words.switch("--tab"),
                force: words.switch("--force"),
            }
        }
        "prompt" => {
            let words = words()?;
            let wait = words.switch("--wait");
            let timeout = words.number("--timeout")?;
            words.needs("--wait", &["--timeout"])?;
            let given = words.with_after();
            let [pane, text] = exact(&given, ["<pane>", "<text>"])?;
            Op::AgentPrompt {
                pane: PaneId(number("<pane>", pane)?),
                text: text.to_owned(),
                wait,
                timeout,
            }
        }
        "wait" => {
            let words = words()?;
            Op::AgentWait {
                pane: PaneId(words.one("<pane>")?),
                timeout: words.number("--timeout")?,
            }
        }
        "get" => Op::AgentGet {
            pane: PaneId(words()?.one("<pane>")?),
        },
        "reply" => Op::AgentReply {
            pane: PaneId(words()?.one("<pane>")?),
        },
        _ => return Err(unknown_verb(verb)),
    })
}

/// The pane input or output `verb` and its words ask for. `send`, `type` and
/// `keys` take their words as typed, so text starting with `--` is kept; `run`,
/// `wait` and `read` read their flags, and `run` takes a command starting with
/// `--` after `--`.
fn pane_io(command: &'static CommandHelp, verb: &str, rest: &[&str]) -> Result<Op, UsageProblem> {
    Ok(match verb {
        "send" => {
            let [pane, text] = exact(rest, ["<pane>", "<text>"])?;
            Op::PaneSend {
                pane: PaneId(number("<pane>", pane)?),
                text: text.to_owned(),
            }
        }
        "type" => {
            let [pane, text] = exact(rest, ["<pane>", "<text>"])?;
            Op::PaneType {
                pane: PaneId(number("<pane>", pane)?),
                text: text.to_owned(),
            }
        }
        "run" => {
            let words = Words::split(rest, command)?;
            let wait = words.switch("--wait");
            let timeout = words.number("--timeout")?;
            let tail = words.number("--tail")?;
            words.needs("--wait", &["--timeout", "--tail"])?;
            let given = words.with_after();
            let [pane, line] = exact(&given, ["<pane>", "<command>"])?;
            Op::PaneRun {
                pane: PaneId(number("<pane>", pane)?),
                command: line.to_owned(),
                wait,
                timeout,
                force: words.switch("--force"),
                tail,
            }
        }
        "keys" => match rest {
            [] => return Err(UsageProblem::MissingWord { what: "<pane>" }),
            [_] => return Err(UsageProblem::MissingWord { what: "<key>" }),
            [pane, keys @ ..] => Op::PaneKeys {
                pane: PaneId(number("<pane>", pane)?),
                keys: owned(keys),
            },
        },
        "wait" => {
            let words = Words::split(rest, command)?;
            let pane = PaneId(words.one("<pane>")?);
            words.exclusive(&["--text", "--regex"])?;
            let text = words.value("--text").map(str::to_owned);
            let regex = words.value("--regex").map(str::to_owned);
            if text.is_none() && regex.is_none() {
                return Err(UsageProblem::MissingFlag { flag: "--text" });
            }
            if let Some(pattern) = &regex {
                regex::Regex::new(pattern).map_err(|error| UsageProblem::BadPattern {
                    got: pattern.clone(),
                    why: last_line(&error.to_string()).to_owned(),
                })?;
            }
            Op::PaneWait {
                pane,
                text,
                regex,
                timeout: words.number("--timeout")?,
                lines: words.number("--lines")?,
            }
        }
        "read" => {
            let words = Words::split(rest, command)?;
            Op::PaneRead {
                pane: words.target("<pane>")?.map(PaneId),
                lines: words.number("--lines")?.unwrap_or(0),
                head: words.number("--head")?,
                tail: words.number("--tail")?,
            }
        }
        _ => return Err(unknown_verb(verb)),
    })
}

/// The add request of `noun verb` its words ask for, with `--name`, `--run`
/// and `--cwd` (a relative folder made absolute from where orb runs).
fn add_words(
    command: &'static CommandHelp,
    noun: &str,
    verb: &str,
    rest: &[&str],
) -> Result<Op, UsageProblem> {
    let words = Words::split(rest, command)?;
    let name = words.value("--name").map(str::to_owned);
    let run = words.value("--run").map(str::to_owned);
    let cwd = words.value("--cwd").map(absolute);
    Ok(match (noun, verb) {
        ("pane", "split") => Op::PaneSplit {
            direction: direction("--direction", words.required("--direction")?)?,
            pane: words.target("<pane>")?.map(PaneId),
            name,
            run,
            cwd,
        },
        ("pane", "stack") => Op::PaneStack {
            pane: words.target("<pane>")?.map(PaneId),
            name,
            run,
            cwd,
        },
        ("pane", "float") => Op::PaneFloat {
            session: session_only(&words)?,
            name,
            run,
            cwd,
        },
        _ => Op::TabCreate {
            session: session_only(&words)?,
            name,
            run,
            cwd,
        },
    })
}

/// `flag`'s value as a side; `NotInSet` naming `flag` otherwise.
fn direction(flag: &'static str, word: &str) -> Result<Direction, UsageProblem> {
    match word {
        "right" => Ok(Direction::Right),
        "down" => Ok(Direction::Down),
        got => Err(UsageProblem::NotInSet {
            flag,
            got: got.to_owned(),
        }),
    }
}

/// `dir` as an absolute path, from the current folder when relative; as given
/// when that can't be read.
fn absolute(dir: &str) -> PathBuf {
    std::path::absolute(dir).unwrap_or_else(|_| PathBuf::from(dir))
}

/// Words that may only hold `--session <id>`: the ID, or `None` when it's
/// absent.
fn session_only(words: &Words<'_>) -> Result<Option<SessionId>, UsageProblem> {
    words.no_positional()?;
    Ok(words.number("--session")?.map(SessionId))
}

/// `rest` as exactly the words `names` names, in order: `MissingWord` for the
/// first one left out, `ExtraWord` for the first one past them.
fn exact<'w, const N: usize>(
    rest: &[&'w str],
    names: [&'static str; N],
) -> Result<[&'w str; N], UsageProblem> {
    <[&'w str; N]>::try_from(rest)
        .ok()
        .ok_or_else(|| match names.get(rest.len()) {
            Some(&what) => UsageProblem::MissingWord { what },
            None => extra_word(rest.get(N).copied().unwrap_or_default()),
        })
}

/// The words after a command's verb: its positionals, its `--flag`s and
/// whatever follows `--`.
#[derive(Debug, Default)]
struct Words<'a> {
    positional: Vec<&'a str>,
    /// Each flag given, with its value when it takes one.
    flags: HashMap<&'static str, Option<&'a str>>,
    /// Each flag that may be given more than once, with its values in order.
    lists: HashMap<&'static str, Vec<&'a str>>,
    /// Every word after `--`, kept as typed.
    after: Vec<&'a str>,
}

impl<'a> Words<'a> {
    /// Splits `rest` by the flags `command`'s table row lists: which take a
    /// value and which may repeat. Refuses a flag the row doesn't list, a
    /// repeat and a missing value. When `--` is among them, it ends the flags
    /// and every word after it goes to `after`.
    fn split(rest: &[&'a str], command: &'static CommandHelp) -> Result<Self, UsageProblem> {
        let flags = || {
            command
                .params
                .iter()
                .filter(|param| param.name.starts_with("--"))
        };
        let mut words = Self::default();
        let mut rest = rest.iter().copied();
        while let Some(word) = rest.next() {
            if !word.starts_with("--") {
                words.positional.push(word);
                continue;
            }
            let Some(param) = flags().find(|param| param.name == word) else {
                return Err(UsageProblem::UnknownFlag {
                    flag: word.to_owned(),
                    suggestion: help::suggest(word, flags().map(|param| param.name)),
                });
            };
            if param.name == "--" {
                words.after = rest.collect();
                break;
            }
            let value = match param.value {
                Some(_) => Some(
                    rest.next()
                        .ok_or(UsageProblem::MissingValue { flag: param.name })?,
                ),
                None => None,
            };
            if param.repeats {
                words.lists.entry(param.name).or_default().extend(value);
                continue;
            }
            if words.flags.insert(param.name, value).is_some() {
                return Err(UsageProblem::Repeated { flag: param.name });
            }
        }
        Ok(words)
    }

    /// Flag `flag`'s value, if it was given one.
    fn value(&self, flag: &str) -> Option<&'a str> {
        self.flags.get(flag).copied().flatten()
    }

    /// Flag `flag`'s value; `MissingFlag` when it wasn't given.
    fn required(&self, flag: &'static str) -> Result<&'a str, UsageProblem> {
        self.value(flag).ok_or(UsageProblem::MissingFlag { flag })
    }

    /// Whether switch `flag` was given.
    fn switch(&self, flag: &str) -> bool {
        self.flags.contains_key(flag)
    }

    /// Every value repeating flag `flag` was given, in order.
    fn values(&self, flag: &str) -> Vec<&'a str> {
        self.lists.get(flag).cloned().unwrap_or_default()
    }

    /// Which of the opposite flags `yes` and `no` was given: `Some(true)` or
    /// `Some(false)`, `None` for neither; `Exclusive` for both.
    fn either(&self, yes: &'static str, no: &'static str) -> Result<Option<bool>, UsageProblem> {
        match (self.switch(yes), self.switch(no)) {
            (true, true) => Err(UsageProblem::Exclusive {
                one: yes,
                other: no,
            }),
            (true, false) => Ok(Some(true)),
            (false, true) => Ok(Some(false)),
            (false, false) => Ok(None),
        }
    }

    /// `Exclusive` naming the first two of `flags` given together.
    fn exclusive(&self, flags: &[&'static str]) -> Result<(), UsageProblem> {
        let mut given = flags.iter().filter(|&&flag| self.switch(flag));
        match (given.next(), given.next()) {
            (Some(&one), Some(&other)) => Err(UsageProblem::Exclusive { one, other }),
            _ => Ok(()),
        }
    }

    /// Refuses the first of `flags` given without switch `needs`.
    fn needs(&self, needs: &'static str, flags: &[&'static str]) -> Result<(), UsageProblem> {
        if self.switch(needs) {
            return Ok(());
        }
        flags
            .iter()
            .find(|&&flag| self.switch(flag))
            .map_or(Ok(()), |&flag| Err(UsageProblem::Needs { flag, needs }))
    }

    /// The positionals, then every word after `--`.
    fn with_after(&self) -> Vec<&'a str> {
        [self.positional.as_slice(), self.after.as_slice()].concat()
    }

    /// Flag `flag`'s value as a number: `None` when absent; `NotANumber`
    /// when it isn't one.
    fn number<T>(&self, flag: &'static str) -> Result<Option<T>, UsageProblem>
    where
        T: FromStr,
    {
        self.value(flag).map(|word| number(flag, word)).transpose()
    }

    /// Refuses the first positional, for words that may hold none.
    fn no_positional(&self) -> Result<(), UsageProblem> {
        self.positional
            .first()
            .map_or(Ok(()), |word| Err(extra_word(word)))
    }

    /// The one positional `what`; `MissingWord` when there is none,
    /// `ExtraWord` past it.
    fn only(&self, what: &'static str) -> Result<&'a str, UsageProblem> {
        exact(&self.positional, [what]).map(|[word]| word)
    }

    /// The one positional `what` as an ID.
    fn one(&self, what: &'static str) -> Result<i64, UsageProblem> {
        number(what, self.only(what)?)
    }

    /// The one optional positional `what`, as an ID: `None` when there is
    /// none.
    fn target(&self, what: &'static str) -> Result<Option<i64>, UsageProblem> {
        match self.positional.as_slice() {
            [] => Ok(None),
            [id] => number(what, id).map(Some),
            [_, extra, ..] => Err(extra_word(extra)),
        }
    }
}

/// `--status`'s comma-separated names as statuses, none when it wasn't
/// given; `NotInSet` naming the first name that is no status.
fn statuses(names: Option<&str>) -> Result<Vec<AgentStatus>, UsageProblem> {
    names.map_or(Ok(Vec::new()), |names| {
        names
            .split(',')
            .map(|name| {
                serde_json::from_value(serde_json::Value::from(name))
                    .ok()
                    .ok_or_else(|| UsageProblem::NotInSet {
                        flag: "--status",
                        got: name.to_owned(),
                    })
            })
            .collect()
    })
}

/// `word`, the value of `what`, as a number; `NotANumber` when it isn't one.
fn number<T>(what: &'static str, word: &str) -> Result<T, UsageProblem>
where
    T: FromStr,
{
    word.parse().ok().ok_or_else(|| UsageProblem::NotANumber {
        what,
        got: word.to_owned(),
    })
}

/// Slack on top of a call's own limit: two screen-loop asks and some margin.
const SLACK: Duration = Duration::from_secs(15);

/// How long `session create` waits: orb's worst case for a session in a new
/// worktree (default-branch reads, a fetch, the worktree write, a queue wait),
/// with room to spare.
const CREATE_LIMIT: Duration = Duration::from_secs(180);

/// How long a call with no limit of its own waits.
const OTHER_LIMIT: Duration = Duration::from_secs(60);

/// How long to wait for orb's reply to `op`: its own limit plus slack, none
/// for a wait with no limit, three minutes for a new session, a minute for
/// the rest.
fn reply_limit(op: &Op) -> Option<Duration> {
    match op {
        Op::AgentStart { .. } => Some(Timing::REAL.start_limit + SLACK),
        Op::SessionCreate { .. } => Some(CREATE_LIMIT),
        Op::AgentPrompt {
            wait: true,
            timeout,
            ..
        }
        | Op::PaneRun {
            wait: true,
            timeout,
            ..
        }
        | Op::PaneWait { timeout, .. }
        | Op::AgentWait { timeout, .. } => {
            timeout.map(|limit| Duration::from_millis(limit) + SLACK)
        }
        _ => Some(OTHER_LIMIT),
    }
}

/// Sends `request` over one connection and reads the one reply line, giving
/// up with `timeout` once `limit` passes.
fn ask(socket: &Path, request: &Request, limit: Option<Duration>) -> Result<Reply, ApiError> {
    let failed = |error: &dyn std::fmt::Display| ApiError::failed(error.to_string());
    let mut stream = UnixStream::connect(socket).map_err(|error| match error.kind() {
        ErrorKind::NotFound | ErrorKind::ConnectionRefused => ApiError::new(
            ErrorCode::NotRunning,
            "orb is not running",
            "start orb in a terminal first",
        ),
        _ => failed(&error),
    })?;
    let line = serde_json::to_string(request).map_err(|error| failed(&error))?;
    writeln!(stream, "{line}").map_err(|error| failed(&error))?;
    stream
        .set_read_timeout(limit)
        .map_err(|error| failed(&error))?;
    let mut reply = String::new();
    BufReader::new(&stream)
        .read_line(&mut reply)
        .map_err(|error| match error.kind() {
            ErrorKind::WouldBlock | ErrorKind::TimedOut => ApiError::new(
                ErrorCode::Timeout,
                format!(
                    "orb didn't answer within {} s",
                    limit.unwrap_or_default().as_secs()
                ),
                timeout_hint(&request.op),
            ),
            _ => failed(&error),
        })?;
    serde_json::from_str(&reply)
        .map_err(|error| failed(&format!("orb sent no readable reply: {error}")))
}

/// Where to look after `op` got no answer in time: the agent or pane it
/// named, the agents for an agent started in a new pane, else the sessions.
fn timeout_hint(op: &Op) -> String {
    match *op {
        Op::AgentPrompt { pane, .. }
        | Op::AgentWait { pane, .. }
        | Op::AgentGet { pane }
        | Op::AgentReply { pane }
        | Op::AgentStart {
            pane: Some(pane), ..
        } => format!("orb agent get {} shows where it got to", pane.0),
        Op::PaneSend { pane, .. }
        | Op::PaneKeys { pane, .. }
        | Op::PaneType { pane, .. }
        | Op::PaneRun { pane, .. }
        | Op::PaneWait { pane, .. }
        | Op::PaneClose { pane }
        | Op::PaneRename { pane, .. }
        | Op::PaneFocus { pane }
        | Op::PaneRead {
            pane: Some(pane), ..
        }
        | Op::PaneSplit {
            pane: Some(pane), ..
        }
        | Op::PaneStack {
            pane: Some(pane), ..
        }
        | Op::TabClose { pane: Some(pane) } => format!("orb pane read {} shows the pane", pane.0),
        Op::AgentStart { pane: None, .. } => "orb agent list shows where it got to".to_owned(),
        _ => "orb session list".to_owned(),
    }
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
    use std::time::Duration;

    use super::{Entry, UsageProblem, entry, home, parse, reply_limit, run, timeout_hint};
    use crate::Log;
    use crate::feat::control::protocol::{AgentKind, Direction, Op};
    use crate::feat::control::server;
    use crate::feat::sessions::state::AgentStatus;
    use crate::ids::{PaneId, PlateItemId, ProjectId, SessionId};

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

        // Then it exits 2 naming the verb and pointing at the noun's help,
        // without trying to connect.
        let err = String::from_utf8_lossy(&err);
        assert!(
            code == 2
                && err.starts_with("error: unknown verb delete")
                && err.contains("help: orb session --help")
                && !err.contains("not_running"),
            "an unknown verb is named and exits 2, got {code}: {err}"
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
            Ok(Op::PaneSplit {
                pane: Some(PaneId(3)),
                direction: Direction::Down,
                name: None,
                run: None,
                cwd: None,
            }),
            "the pane and direction come from the words"
        );
    }

    #[rstest::rstest]
    #[case::tab_create(
        &["tab", "create", "--name", "x", "--run", "just dev", "--cwd", "/tmp"],
        Op::TabCreate { session: None, name: Some("x".into()), run: Some("just dev".into()), cwd: Some("/tmp".into()) },
    )]
    #[case::tab_create_bare(
        &["tab", "create"],
        Op::TabCreate { session: None, name: None, run: None, cwd: None },
    )]
    #[case::pane_split(
        &["pane", "split", "--direction", "right", "--name", "x", "--run", "just dev", "--cwd", "/tmp"],
        Op::PaneSplit {
            pane: None,
            direction: Direction::Right,
            name: Some("x".into()),
            run: Some("just dev".into()),
            cwd: Some("/tmp".into()),
        },
    )]
    #[case::pane_split_bare(
        &["pane", "split", "--direction", "right"],
        Op::PaneSplit { pane: None, direction: Direction::Right, name: None, run: None, cwd: None },
    )]
    #[case::pane_stack(
        &["pane", "stack", "--name", "x", "--run", "just dev", "--cwd", "/tmp"],
        Op::PaneStack { pane: None, name: Some("x".into()), run: Some("just dev".into()), cwd: Some("/tmp".into()) },
    )]
    #[case::pane_stack_bare(
        &["pane", "stack"],
        Op::PaneStack { pane: None, name: None, run: None, cwd: None },
    )]
    #[case::pane_float(
        &["pane", "float", "--name", "x", "--run", "just dev", "--cwd", "/tmp"],
        Op::PaneFloat { session: None, name: Some("x".into()), run: Some("just dev".into()), cwd: Some("/tmp".into()) },
    )]
    #[case::pane_float_bare(
        &["pane", "float"],
        Op::PaneFloat { session: None, name: None, run: None, cwd: None },
    )]
    fn add_words_make_their_request(#[case] words: &[&str], #[case] expected: Op) {
        // Given / When parsing an add with or without its name, run and folder.
        let op = parse(&args(words));

        // Then each flag lands on the request, and a left-out one is none.
        assert_eq!(op, Ok(expected), "the add's flags come from the words");
    }

    #[rstest::rstest]
    fn relative_cwd_is_made_absolute() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given / When parsing a new tab in folder `sub`.
        let op = parse(&args(&["tab", "create", "--cwd", "sub"]));

        // Then the folder is taken from where orb runs.
        let expected = std::env::current_dir()?.join("sub");
        assert_eq!(
            op,
            Ok(Op::TabCreate {
                session: None,
                name: None,
                run: None,
                cwd: Some(expected),
            }),
            "a relative folder is made absolute"
        );
        Ok(())
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
    #[case::type_text(
        &["pane", "type", "12", ":wq"],
        Op::PaneType { pane: PaneId(12), text: ":wq".into() },
    )]
    #[case::read_own(&["pane", "read", "--lines", "5"], Op::PaneRead {
        pane: None,
        lines: 5,
        head: None,
        tail: None,
    })]
    #[case::read_named(&["pane", "read", "3"], Op::PaneRead {
        pane: Some(PaneId(3)),
        lines: 0,
        head: None,
        tail: None,
    })]
    fn pane_io_words_make_their_request(#[case] words: &[&str], #[case] expected: Op) {
        // Given / When parsing the words.
        let op = parse(&args(words));

        // Then they make the request they spell.
        assert_eq!(op, Ok(expected), "{words:?} parse to their request");
    }

    /// `pane run 3 <command>` as asked: waiting, its timeout, forced and its tail.
    fn run_op(
        command: &str,
        wait: bool,
        timeout: Option<u64>,
        force: bool,
        tail: Option<usize>,
    ) -> Op {
        Op::PaneRun {
            pane: PaneId(3),
            command: command.into(),
            wait,
            timeout,
            force,
            tail,
        }
    }

    #[rstest::rstest]
    #[case::plain(&["pane", "run", "3", "just test"], run_op("just test", false, None, false, None))]
    #[case::wait(
        &["pane", "run", "3", "just test", "--wait"],
        run_op("just test", true, None, false, None),
    )]
    #[case::wait_timeout(
        &["pane", "run", "3", "just test", "--wait", "--timeout", "600000"],
        run_op("just test", true, Some(600_000), false, None),
    )]
    #[case::force(
        &["pane", "run", "3", "--force", "just test"],
        run_op("just test", false, None, true, None),
    )]
    #[case::wait_tail(
        &["pane", "run", "3", "just test", "--wait", "--tail", "20"],
        run_op("just test", true, None, false, Some(20)),
    )]
    #[case::json(
        &["pane", "run", "3", "git status", "--wait", "--json"],
        run_op("git status", true, None, false, None),
    )]
    #[case::double_dash(
        &["pane", "run", "3", "--wait", "--", "--version"],
        run_op("--version", true, None, false, None),
    )]
    fn run_words_make_their_request(#[case] words: &[&str], #[case] expected: Op) {
        // Given / When parsing the words.
        let op = parse(&args(words));

        // Then they make the run they spell.
        assert_eq!(op, Ok(expected), "{words:?} parse to their run");
    }

    #[rstest::rstest]
    fn run_timeout_without_wait_is_a_usage_error() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given / When running `pane run 3 x --timeout 5` without --wait.
        let (code, _, err) = ran(&["pane", "run", "3", "x", "--timeout", "5"])?;

        // Then it exits 2 saying --timeout needs --wait.
        assert_eq!(
            (code, err.lines().next()),
            (2, Some("error: --timeout only goes with --wait")),
            "a timeout only goes with --wait"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn run_tail_without_wait_is_a_usage_error() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given / When running `pane run 3 x --tail 5` without --wait.
        let (code, _, err) = ran(&["pane", "run", "3", "x", "--tail", "5"])?;

        // Then it exits 2 saying --tail needs --wait.
        assert_eq!(
            (code, err.lines().next()),
            (2, Some("error: --tail only goes with --wait")),
            "a tail only goes with --wait"
        );
        Ok(())
    }

    /// `pane wait 3` for `text` or `regex`, with `timeout` and `lines`.
    fn wait_op(
        text: Option<&str>,
        regex: Option<&str>,
        timeout: Option<u64>,
        lines: Option<usize>,
    ) -> Op {
        Op::PaneWait {
            pane: PaneId(3),
            text: text.map(str::to_owned),
            regex: regex.map(str::to_owned),
            timeout,
            lines,
        }
    }

    #[rstest::rstest]
    #[case::text(
        &["pane", "wait", "3", "--text", "Listening on"],
        wait_op(Some("Listening on"), None, None, None),
    )]
    #[case::regex(
        &["pane", "wait", "3", "--regex", "result: (ok|FAILED)"],
        wait_op(None, Some("result: (ok|FAILED)"), None, None),
    )]
    #[case::lines(
        &["pane", "wait", "3", "--text", "ready", "--lines", "50"],
        wait_op(Some("ready"), None, None, Some(50)),
    )]
    #[case::timeout(
        &["pane", "wait", "3", "--text", "ready", "--timeout", "60000"],
        wait_op(Some("ready"), None, Some(60_000), None),
    )]
    fn wait_words_make_their_request(#[case] words: &[&str], #[case] expected: Op) {
        // Given / When parsing the words.
        let op = parse(&args(words));

        // Then they make the wait they spell.
        assert_eq!(op, Ok(expected), "{words:?} parse to their wait");
    }

    #[rstest::rstest]
    fn wait_with_text_and_regex_is_a_usage_error() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given / When running `pane wait 3` with both --text and --regex.
        let (code, _, err) = ran(&["pane", "wait", "3", "--text", "a", "--regex", "b"])?;

        // Then it exits 2 saying they can't go together.
        assert_eq!(
            (code, err.lines().next()),
            (2, Some("error: --text and --regex can't go together")),
            "a wait takes one of --text and --regex"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn wait_without_text_or_regex_is_a_usage_error() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given / When running `pane wait 3` with neither --text nor --regex.
        let (code, _, err) = ran(&["pane", "wait", "3"])?;

        // Then it exits 2 asking for --text.
        assert_eq!(
            (code, err.lines().next()),
            (2, Some("error: `orb pane wait` needs --text <text>")),
            "a wait needs something to look for"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn wait_with_a_bad_regex_is_a_usage_error() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given / When running `pane wait 3 --regex "(ok"`, a group left open.
        let (code, _, err) = ran(&["pane", "wait", "3", "--regex", "(ok"])?;

        // Then it exits 2 naming the pattern and what's wrong with it.
        assert_eq!(
            (code, err.lines().next()),
            (
                2,
                Some("error: --regex can't take (ok: error: unclosed group")
            ),
            "a pattern that doesn't compile is typed wrong"
        );
        Ok(())
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

    #[rstest::rstest]
    #[case::start_with_args(
        &["agent", "start", "--kind", "pi", "--pane", "3", "--name", "x", "--", "--model", "a b"],
        Op::AgentStart {
            pane: Some(PaneId(3)),
            kind: AgentKind::Pi,
            name: Some("x".into()),
            args: vec!["--model".into(), "a b".into()],
            split: None,
            tab: false,
            force: false,
        },
    )]
    #[case::start_bare(
        &["agent", "start", "--kind", "claude"],
        Op::AgentStart { pane: None, kind: AgentKind::Claude, name: None, args: vec![], split: None, tab: false, force: false },
    )]
    #[case::start_forced(
        &["agent", "start", "--kind", "claude", "--pane", "3", "--force"],
        Op::AgentStart { pane: Some(PaneId(3)), kind: AgentKind::Claude, name: None, args: vec![], split: None, tab: false, force: true },
    )]
    #[case::prompt_wait(
        &["agent", "prompt", "3", "hi", "--wait", "--timeout", "500"],
        Op::AgentPrompt { pane: PaneId(3), text: "hi".into(), wait: true, timeout: Some(500) },
    )]
    #[case::prompt_wait_json(
        &["agent", "prompt", "3", "hi", "--wait", "--json"],
        Op::AgentPrompt { pane: PaneId(3), text: "hi".into(), wait: true, timeout: None },
    )]
    #[case::prompt_dashed(
        &["agent", "prompt", "3", "--", "--hi"],
        Op::AgentPrompt { pane: PaneId(3), text: "--hi".into(), wait: false, timeout: None },
    )]
    #[case::wait(&["agent", "wait", "3"], Op::AgentWait { pane: PaneId(3), timeout: None })]
    #[case::get(&["agent", "get", "3"], Op::AgentGet { pane: PaneId(3) })]
    #[case::reply(&["agent", "reply", "3"], Op::AgentReply { pane: PaneId(3) })]
    fn agent_words_make_their_request(#[case] words: &[&str], #[case] expected: Op) {
        // Given / When parsing the words.
        let op = parse(&args(words));

        // Then they make the request they spell.
        assert_eq!(op, Ok(expected), "{words:?} parse to their request");
    }

    #[rstest::rstest]
    #[case::split(
        &["agent", "start", "--kind", "claude", "--split", "down", "--name", "helper"],
        Op::AgentStart {
            pane: None,
            kind: AgentKind::Claude,
            name: Some("helper".into()),
            args: vec![],
            split: Some(Direction::Down),
            tab: false,
            force: false,
        },
    )]
    #[case::tab(
        &["agent", "start", "--kind", "pi", "--tab", "--", "--model", "sonnet"],
        Op::AgentStart {
            pane: None,
            kind: AgentKind::Pi,
            name: None,
            args: vec!["--model".into(), "sonnet".into()],
            split: None,
            tab: true,
            force: false,
        },
    )]
    fn agent_start_takes_split_or_tab(#[case] words: &[&str], #[case] expected: Op) {
        // Given / When parsing an agent start naming a new pane's place.
        let op = parse(&args(words));

        // Then the request carries that place and no pane.
        assert_eq!(op, Ok(expected), "{words:?} start the agent in a new pane");
    }

    #[rstest::rstest]
    #[case::pane_and_split(&["--pane", "3", "--split", "right"], "--pane", "--split")]
    #[case::pane_and_tab(&["--tab", "--pane", "3"], "--pane", "--tab")]
    #[case::split_and_tab(&["--split", "right", "--tab"], "--split", "--tab")]
    #[case::split_and_force(&["--split", "right", "--force"], "--split", "--force")]
    #[case::tab_and_force(&["--force", "--tab"], "--tab", "--force")]
    fn agent_start_places_exclude_each_other(
        #[case] places: &[&str],
        #[case] one: &'static str,
        #[case] other: &'static str,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given an agent start naming two places.
        let words = [&["agent", "start", "--kind", "claude"], places].concat();

        // When parsing and running it.
        let problem = problem(&words);
        let (code, out, _) = ran(&words)?;

        // Then both flags are named, and it exits 2 sending nothing.
        assert_eq!(
            (problem, code, out),
            (
                Some(UsageProblem::Exclusive { one, other }),
                2,
                String::new()
            ),
            "an agent starts in one place"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn split_takes_right_or_down() {
        // Given / When parsing `agent start --kind claude --split left`.
        let problem = problem(&["agent", "start", "--kind", "claude", "--split", "left"]);

        // Then --split and the value given are named.
        assert_eq!(
            problem,
            Some(UsageProblem::NotInSet {
                flag: "--split",
                got: "left".into(),
            }),
            "--split is right or down"
        );
    }

    #[rstest::rstest]
    fn prompt_timeout_without_wait_is_a_usage_error() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given a socket path nothing listens on.
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("orb.sock");
        let (mut out, mut err) = (Vec::new(), Vec::new());

        // When running `agent prompt 3 hi --timeout 5` without `--wait`.
        let code = run(
            &args(&["agent", "prompt", "3", "hi", "--timeout", "5"]),
            None,
            &socket,
            &mut out,
            &mut err,
        );

        // Then it exits 2 with the usage.
        assert_eq!(code, 2, "a timeout only goes with --wait");
        Ok(())
    }

    #[rstest::rstest]
    fn plate_add_words_carry_two_links_in_order() {
        // Given / When parsing an add with two links and --urgent.
        let op = parse(&args(&[
            "plate", "add", "t", "--verb", "review", "--link", "a", "--link", "b", "--urgent",
        ]));

        // Then it asks for one urgent review with both links, in order.
        assert_eq!(
            op,
            Ok(Op::PlateAdd {
                title: "t".into(),
                verb: "review".into(),
                what: None,
                links: vec!["a".into(), "b".into()],
                urgent: true,
                waiting: false,
                pane: None,
            }),
            "--link may repeat and keeps its order"
        );
    }

    /// The exit code of running `words` against a socket nothing listens on.
    fn exit_code(words: &[&str]) -> Result<u8, Box<dyn Error + Send + Sync>> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("orb.sock");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        Ok(run(&args(words), None, &socket, &mut out, &mut err))
    }

    #[rstest::rstest]
    fn repeated_verb_flag_is_a_usage_error() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given / When running an add that gives --verb twice.
        let code = exit_code(&["plate", "add", "t", "--verb", "review", "--verb", "read"])?;

        // Then it exits 2 with the usage.
        assert_eq!(code, 2, "only --link may repeat");
        Ok(())
    }

    #[rstest::rstest]
    fn plate_update_with_urgent_and_not_urgent_is_a_usage_error()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given / When running an update that is both urgent and not.
        let code = exit_code(&["plate", "update", "3", "--urgent", "--not-urgent"])?;

        // Then it exits 2 with the usage.
        assert_eq!(code, 2, "--urgent and --not-urgent can't go together");
        Ok(())
    }

    #[rstest::rstest]
    fn plate_add_without_a_verb_is_a_usage_error() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given / When running an add with no --verb.
        let code = exit_code(&["plate", "add", "t"])?;

        // Then it exits 2 with the usage.
        assert_eq!(code, 2, "an add needs its verb");
        Ok(())
    }

    #[rstest::rstest]
    #[case::update(
        &["plate", "update", "3", "--title", "t", "--link", "a", "--waiting", "--not-urgent"],
        Op::PlateUpdate {
            item: PlateItemId(3),
            title: Some("t".into()),
            verb: None,
            what: None,
            links: Some(vec!["a".into()]),
            urgent: Some(false),
            waiting: Some(true),
        },
    )]
    #[case::update_bare(
        &["plate", "update", "3"],
        Op::PlateUpdate {
            item: PlateItemId(3),
            title: None,
            verb: None,
            what: None,
            links: None,
            urgent: None,
            waiting: None,
        },
    )]
    #[case::done(&["plate", "done", "3"], Op::PlateDone { item: PlateItemId(3) })]
    #[case::list(&["plate", "list"], Op::PlateList)]
    fn plate_words_make_their_request(#[case] words: &[&str], #[case] expected: Op) {
        // Given / When parsing the words.
        let op = parse(&args(words));

        // Then they make the request they spell.
        assert_eq!(op, Ok(expected), "{words:?} parse to their request");
    }

    #[rstest::rstest]
    #[case::wait_with_timeout(
        Op::AgentWait { pane: PaneId(1), timeout: Some(1000) },
        Some(Duration::from_secs(16)),
    )]
    #[case::wait_without_timeout(Op::AgentWait { pane: PaneId(1), timeout: None }, None)]
    #[case::start(
        Op::AgentStart { pane: None, kind: AgentKind::Claude, name: None, args: vec![], split: None, tab: false, force: false },
        Some(Duration::from_secs(45)),
    )]
    #[case::session_create(
        Op::SessionCreate { project: None, worktree: true, name: None },
        Some(Duration::from_secs(180)),
    )]
    #[case::list(
        Op::SessionList { all: false, project: None },
        Some(Duration::from_secs(60))
    )]
    #[case::run_wait_with_timeout(
        Op::PaneRun { pane: PaneId(1), command: "x".into(), wait: true, timeout: Some(1000), force: false, tail: None },
        Some(Duration::from_secs(16)),
    )]
    #[case::run_wait_without_timeout(
        Op::PaneRun { pane: PaneId(1), command: "x".into(), wait: true, timeout: None, force: false, tail: None },
        None,
    )]
    #[case::run_without_wait(
        Op::PaneRun { pane: PaneId(1), command: "x".into(), wait: false, timeout: None, force: false, tail: None },
        Some(Duration::from_secs(60)),
    )]
    #[case::pane_wait_with_timeout(
        Op::PaneWait { pane: PaneId(1), text: Some("x".into()), regex: None, timeout: Some(1000), lines: None },
        Some(Duration::from_secs(16)),
    )]
    #[case::pane_wait_without_timeout(
        Op::PaneWait { pane: PaneId(1), text: Some("x".into()), regex: None, timeout: None, lines: None },
        None,
    )]
    fn reply_limit_outlasts_the_asked_timeout(#[case] op: Op, #[case] expected: Option<Duration>) {
        // Given / When working out how long to wait for the reply.
        let limit = reply_limit(&op);

        // Then it outlasts what the request may take.
        assert_eq!(limit, expected, "{op:?} waits for its reply that long");
    }

    /// What was wrong with `words`, or `None` when they parse.
    fn problem(words: &[&str]) -> Option<UsageProblem> {
        parse(&args(words)).err().map(|error| error.problem)
    }

    /// The exit code, stdout and stderr of running `words` against a socket
    /// nothing listens on.
    fn ran(words: &[&str]) -> Result<(u8, String, String), Box<dyn Error + Send + Sync>> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("orb.sock");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(&args(words), None, &socket, &mut out, &mut err);
        Ok((
            code,
            String::from_utf8(out)?,
            String::from_utf8_lossy(&err).into_owned(),
        ))
    }

    #[rstest::rstest]
    fn unknown_flag_names_it_and_suggests_a_known_one() {
        // Given / When parsing `pane split --dir right`.
        let problem = problem(&["pane", "split", "--dir", "right"]);

        // Then the flag is named, with the one it was likely meant to be.
        assert_eq!(
            problem,
            Some(UsageProblem::UnknownFlag {
                flag: "--dir".into(),
                suggestion: Some("--direction"),
            }),
            "--dir is unknown and close to --direction"
        );
    }

    #[rstest::rstest]
    fn flag_value_that_isnt_a_number_is_named() {
        // Given / When parsing `pane read --lines lots`.
        let problem = problem(&["pane", "read", "--lines", "lots"]);

        // Then the flag and the word given are named.
        assert_eq!(
            problem,
            Some(UsageProblem::NotANumber {
                what: "--lines",
                got: "lots".into(),
            }),
            "--lines takes a number"
        );
    }

    #[rstest::rstest]
    fn value_outside_its_set_is_named() {
        // Given / When parsing `pane split --direction left`.
        let problem = problem(&["pane", "split", "--direction", "left"]);

        // Then the flag and the value given are named.
        assert_eq!(
            problem,
            Some(UsageProblem::NotInSet {
                flag: "--direction",
                got: "left".into(),
            }),
            "--direction is right or down"
        );
    }

    #[rstest::rstest]
    fn unknown_verb_lists_the_nouns_verbs() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given / When running `session delete`.
        let (_, _, err) = ran(&["session", "delete"])?;

        // Then stderr names the verb and lists the session verbs.
        assert!(
            err.contains("delete") && err.contains("list, create, rename, settle, focus"),
            "the unknown verb and the noun's verbs are named, got {err}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn unknown_noun_is_named_with_a_suggestion() {
        // Given / When parsing `sessions list`.
        let problem = problem(&["sessions", "list"]);

        // Then the word is named, with the noun it was likely meant to be.
        assert_eq!(
            problem,
            Some(UsageProblem::UnknownNoun {
                word: "sessions".into(),
                suggestion: Some("session"),
            }),
            "sessions is no noun and close to session"
        );
    }

    #[rstest::rstest]
    fn missing_flag_value_is_named() {
        // Given / When parsing `pane read --lines` with nothing after it.
        let problem = problem(&["pane", "read", "--lines"]);

        // Then the flag missing its value is named.
        assert_eq!(
            problem,
            Some(UsageProblem::MissingValue { flag: "--lines" }),
            "--lines needs a value"
        );
    }

    #[rstest::rstest]
    fn repeated_flag_is_named() {
        // Given / When parsing a plate add giving --verb twice.
        let problem = problem(&["plate", "add", "t", "--verb", "review", "--verb", "read"]);

        // Then the repeated flag is named.
        assert_eq!(
            problem,
            Some(UsageProblem::Repeated { flag: "--verb" }),
            "--verb may come once"
        );
    }

    #[rstest::rstest]
    fn extra_positional_is_named() {
        // Given / When parsing `pane close 3 4`.
        let problem = problem(&["pane", "close", "3", "4"]);

        // Then the word past the pane is named.
        assert_eq!(
            problem,
            Some(UsageProblem::ExtraWord { word: "4".into() }),
            "pane close takes one pane"
        );
    }

    #[rstest::rstest]
    fn missing_positional_is_named() {
        // Given / When parsing `pane close` with no pane.
        let problem = problem(&["pane", "close"]);

        // Then the missing pane is named.
        assert_eq!(
            problem,
            Some(UsageProblem::MissingWord { what: "<pane>" }),
            "pane close needs its pane"
        );
    }

    #[rstest::rstest]
    fn exclusive_flags_are_named() {
        // Given / When parsing a plate update both urgent and not.
        let problem = problem(&["plate", "update", "3", "--urgent", "--not-urgent"]);

        // Then both flags are named.
        assert_eq!(
            problem,
            Some(UsageProblem::Exclusive {
                one: "--urgent",
                other: "--not-urgent",
            }),
            "--urgent and --not-urgent rule each other out"
        );
    }

    #[rstest::rstest]
    fn usage_error_prints_error_usage_and_help_lines() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given / When running `pane split --dir right`.
        let printed = ran(&["pane", "split", "--dir", "right"])?;

        // Then it exits 2 with nothing on stdout and three lines on stderr.
        assert_eq!(
            printed,
            (
                2,
                String::new(),
                "error: unknown flag --dir for `orb pane split` (did you mean --direction?)\n\
                 usage: orb pane split [<pane>] --direction right|down [--name <text>] \
                 [--run <command>] [--cwd <dir>]\n\
                 help: orb pane split --help\n"
                    .to_owned(),
            ),
            "a usage error prints what was wrong, the usage line and the help command"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn help_word_prints_the_overview() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given / When running `help`.
        let (code, out, _) = ran(&["help"])?;

        // Then it exits 0 naming every noun and how to get a noun's help.
        let nouns = ["session", "tab", "pane", "agent", "plate", "project"];
        assert!(
            code == 0
                && nouns.iter().all(|noun| out.contains(noun))
                && out.contains("orb <noun> --help"),
            "the overview names all nouns and orb <noun> --help, got {code}: {out}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn noun_help_lists_its_verbs() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given / When running `pane --help`.
        let (code, out, _) = ran(&["pane", "--help"])?;

        // Then it exits 0 naming every pane verb.
        let verbs = [
            "list", "split", "stack", "float", "close", "rename", "focus", "send", "keys", "type",
            "run", "read",
        ];
        let missing: Vec<&str> = verbs
            .into_iter()
            .filter(|verb| !out.contains(&format!("  {verb} ")))
            .collect();
        assert_eq!(
            (code, missing),
            (0, vec![]),
            "pane help lists every pane verb, got {out}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn command_help_shows_usage_flags_and_examples() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given / When running `help pane split`.
        let (code, out, _) = ran(&["help", "pane", "split"])?;

        // Then it exits 0 with split's usage line, its flag and an example.
        assert!(
            code == 0
                && out.contains("usage: orb pane split [<pane>] --direction right|down")
                && out.contains("--direction right|down  ")
                && out.contains("  orb pane split 12 --direction down"),
            "split's help shows its usage, --direction and an example, got {code}: {out}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn help_flag_wins_over_a_usage_error() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given / When running a split with a bad flag and `--help`.
        let (code, out, _) = ran(&["pane", "split", "12", "--dir", "x", "--help"])?;

        // Then it exits 0 with split's help.
        assert!(
            code == 0 && out.starts_with("orb pane split: "),
            "--help prints split's help despite the bad flag, got {code}: {out}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn words_after_double_dash_never_ask_for_help() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given / When running an agent start passing `--help` to the agent.
        let (code, out, _) = ran(&["agent", "start", "--kind", "claude", "--", "--help"])?;

        // Then it goes to orb as a request, which nothing answers.
        assert_eq!(
            (code, out),
            (1, String::new()),
            "--help after -- belongs to the agent, so the start is sent and refused"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case::bare_outside_a_pane(&[], false, Entry::App)]
    #[case::bare_in_a_pane(&[], true, Entry::Home)]
    #[case::version(&["--version"], true, Entry::Version)]
    #[case::short_version(&["-V"], false, Entry::Version)]
    #[case::upgrade_data(&["--upgrade-data"], false, Entry::App)]
    #[case::integration(&["integration", "install"], true, Entry::App)]
    #[case::unknown_word(&["sessions"], false, Entry::Client)]
    #[case::help_word(&["help"], false, Entry::Client)]
    #[case::help_flag(&["--help"], false, Entry::Client)]
    #[case::short_help(&["-h"], true, Entry::Client)]
    #[case::command(&["pane", "list"], true, Entry::Client)]
    fn entry_decides_what_orb_does(
        #[case] words: &[&str],
        #[case] in_pane: bool,
        #[case] expected: Entry,
    ) {
        // Given / When deciding what the words ask for.
        let decided = entry(&args(words), in_pane);

        // Then orb does what they ask.
        assert_eq!(
            decided, expected,
            "{words:?} (in a pane: {in_pane}) decide what orb does"
        );
    }

    #[rstest::rstest]
    fn home_with_no_orb_listening_starts_the_app() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given a socket path nothing listens on.
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("orb.sock");
        let (mut out, mut err) = (Vec::new(), Vec::new());

        // When bare orb runs in pane 1.
        let shown = home(PaneId(1), &socket, &mut out, &mut err);

        // Then nothing is shown, so orb starts.
        assert_eq!(shown, None, "with no orb to ask, bare orb starts the app");
        Ok(())
    }

    #[rstest::rstest]
    fn not_running_refusal_carries_a_hint() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given / When running `pane list` with no orb listening.
        let (_, _, err) = ran(&["pane", "list"])?;

        // Then the refusal says how to fix it.
        let error: serde_json::Value = serde_json::from_str(&err)?;
        assert_eq!(
            error.get("hint").and_then(serde_json::Value::as_str),
            Some("start orb in a terminal first"),
            "not_running says to start orb, got {err}"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case::agent(
        Op::AgentWait { pane: PaneId(3), timeout: None },
        "orb agent get 3 shows where it got to",
    )]
    #[case::pane(
        Op::PaneRun { pane: PaneId(3), command: "x".into(), wait: false, timeout: None, force: false, tail: None },
        "orb pane read 3 shows the pane",
    )]
    #[case::agent_in_a_new_pane(
        Op::AgentStart {
            pane: None,
            kind: AgentKind::Claude,
            name: None,
            args: vec![],
            split: Some(Direction::Right),
            tab: false,
            force: false,
        },
        "orb agent list shows where it got to",
    )]
    #[case::other(
        Op::SessionSettle { session: Some(SessionId(3)) },
        "orb session list",
    )]
    fn timeout_hint_points_at_what_the_op_named(#[case] op: Op, #[case] expected: &str) {
        // Given / When working out where to look after `op` timed out.
        let hint = timeout_hint(&op);

        // Then it points at the agent or pane named, the agents for one
        // started in a new pane, else the sessions.
        assert_eq!(hint, expected, "{op:?} times out with that hint");
    }

    #[rstest::rstest]
    #[case::session_list(
        &["session", "list", "--all", "--project", "4"],
        Op::SessionList { all: true, project: Some(ProjectId(4)) },
    )]
    #[case::agent_list_status(
        &["agent", "list", "--status", "needs_approval,needs_input", "--session", "7"],
        Op::AgentList {
            all: false,
            session: Some(SessionId(7)),
            status: vec![AgentStatus::NeedsApproval, AgentStatus::NeedsInput],
        },
    )]
    #[case::agent_list_all(
        &["agent", "list", "--all"],
        Op::AgentList { all: true, session: None, status: vec![] },
    )]
    #[case::project_list(&["project", "list"], Op::ProjectList)]
    #[case::pane_read_cut(
        &["pane", "read", "3", "--lines", "50", "--head", "10", "--tail", "5"],
        Op::PaneRead { pane: Some(PaneId(3)), lines: 50, head: Some(10), tail: Some(5) },
    )]
    #[case::pane_read_json(
        &["pane", "read", "3", "--json"],
        Op::PaneRead { pane: Some(PaneId(3)), lines: 0, head: None, tail: None },
    )]
    #[case::agent_reply_json(&["agent", "reply", "3", "--json"], Op::AgentReply { pane: PaneId(3) })]
    fn list_words_make_their_request(#[case] words: &[&str], #[case] expected: Op) {
        // Given / When parsing the words.
        let op = parse(&args(words));

        // Then they make the request they spell.
        assert_eq!(op, Ok(expected), "{words:?} parse to their request");
    }

    #[rstest::rstest]
    fn unknown_status_is_named() {
        // Given / When parsing `agent list --status idle,bogus`.
        let problem = problem(&["agent", "list", "--status", "idle,bogus"]);

        // Then the name that is no status is named.
        assert_eq!(
            problem,
            Some(UsageProblem::NotInSet {
                flag: "--status",
                got: "bogus".into(),
            }),
            "bogus is no agent status"
        );
    }

    #[rstest::rstest]
    fn json_flag_is_refused_where_nothing_prints_text() {
        // Given / When parsing `session list --json`.
        let problem = problem(&["session", "list", "--json"]);

        // Then --json is an unknown flag there.
        assert!(
            matches!(&problem, Some(UsageProblem::UnknownFlag { flag, .. }) if flag == "--json"),
            "session list always prints JSON, so it takes no --json, got {problem:?}"
        );
    }

    /// The exit code and stdout of running `words` against an orb that
    /// answers every request with `reply`.
    fn answered(
        words: &[&str],
        reply: &'static str,
    ) -> Result<(u8, String), Box<dyn Error + Send + Sync>> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("orb.sock");
        server::open(&socket, Log::default(), move |_, _| reply.to_owned())
            .map_err(|report| format!("{report:?}"))?;
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(&args(words), None, &socket, &mut out, &mut err);
        Ok((code, String::from_utf8(out)?))
    }

    /// An orb answer to `pane read`.
    const SCREEN: &str = r#"{"ok":true,"result":{"text":"a\nb"}}"#;

    /// An orb answer to `agent reply`.
    const ANSWER: &str = r#"{"ok":true,"result":{"text":"done","at":5}}"#;

    #[rstest::rstest]
    fn read_prints_plain_text_by_default() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given an orb whose pane 1 shows "a" and "b".
        // When running `pane read 1`.
        let printed = answered(&["pane", "read", "1"], SCREEN)?;

        // Then the screen prints as plain lines.
        assert_eq!(
            printed,
            (0, "a\nb\n".to_owned()),
            "pane read prints the screen as text"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn read_with_json_prints_the_json_shape() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given an orb whose pane 1 shows "a" and "b".
        // When running `pane read 1 --json`.
        let (code, out) = answered(&["pane", "read", "1", "--json"], SCREEN)?;

        // Then one JSON line carries the screen as its text.
        let printed: serde_json::Value = serde_json::from_str(&out)?;
        assert_eq!(
            (code, out.lines().count(), printed),
            (0, 1, serde_json::json!({ "text": "a\nb" })),
            "--json prints the reply's JSON, got {out}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn reply_prints_plain_text_by_default() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given an orb whose agent in pane 1 last answered "done".
        // When running `agent reply 1`.
        let printed = answered(&["agent", "reply", "1"], ANSWER)?;

        // Then the answer prints as plain text.
        assert_eq!(
            printed,
            (0, "done\n".to_owned()),
            "agent reply prints the answer as text"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn reply_with_json_prints_text_and_at() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given an orb whose agent in pane 1 last answered "done" at 5.
        // When running `agent reply 1 --json`.
        let (code, out) = answered(&["agent", "reply", "1", "--json"], ANSWER)?;

        // Then one JSON line carries the text and when it was written.
        let printed: serde_json::Value = serde_json::from_str(&out)?;
        assert_eq!(
            (code, out.lines().count(), printed),
            (0, 1, serde_json::json!({ "text": "done", "at": 5 })),
            "--json prints the reply's JSON, got {out}"
        );
        Ok(())
    }

    /// An orb answer to `agent prompt --wait`.
    const PROMPT_ANSWER: &str = r#"{"ok":true,"result":{"status":"idle","text":"done","at":5}}"#;

    #[rstest::rstest]
    fn prompt_wait_prints_the_answer_as_text() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given an orb whose agent in pane 1 answers the prompt "done".
        // When running `agent prompt 1 hi --wait`.
        let printed = answered(&["agent", "prompt", "1", "hi", "--wait"], PROMPT_ANSWER)?;

        // Then the answer prints as plain text.
        assert_eq!(
            printed,
            (0, "done\n".to_owned()),
            "agent prompt --wait prints the answer as text"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn prompt_wait_with_json_prints_status_text_and_at() -> Result<(), Box<dyn Error + Send + Sync>>
    {
        // Given an orb whose agent in pane 1 answers the prompt "done" at 5.
        // When running `agent prompt 1 hi --wait --json`.
        let (code, out) = answered(
            &["agent", "prompt", "1", "hi", "--wait", "--json"],
            PROMPT_ANSWER,
        )?;

        // Then one JSON line carries the status, the text and its time.
        let printed: serde_json::Value = serde_json::from_str(&out)?;
        assert_eq!(
            (code, out.lines().count(), printed),
            (
                0,
                1,
                serde_json::json!({ "status": "idle", "text": "done", "at": 5 })
            ),
            "--json prints the reply's JSON, got {out}"
        );
        Ok(())
    }

    /// An orb answer to `pane run --wait`: "a" and "b", exit 0.
    const RAN: &str =
        r#"{"ok":true,"result":{"exit":0,"output":"a\nb","lines":2,"truncated":false}}"#;

    #[rstest::rstest]
    fn run_wait_prints_output_then_exit() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given an orb where the command printed "a" and "b" and exited 0.
        // When running `pane run 1 x --wait`.
        let printed = answered(&["pane", "run", "1", "x", "--wait"], RAN)?;

        // Then the output prints as plain lines, then its exit code.
        assert_eq!(
            printed,
            (0, "a\nb\n[exit 0]\n".to_owned()),
            "pane run --wait prints the output and [exit n]"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn run_wait_with_cut_output_says_how_many_lines_came_before()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given an orb where the command printed 250 lines, the last two
        // "y" and "z", and exited 1.
        let cut =
            r#"{"ok":true,"result":{"exit":1,"output":"y\nz","lines":250,"truncated":false}}"#;

        // When running `pane run 4 x --wait --tail 2`.
        let printed = answered(&["pane", "run", "4", "x", "--wait", "--tail", "2"], cut)?;

        // Then a first line says how many came before and how to read them.
        assert_eq!(
            printed,
            (
                0,
                "[… 248 earlier lines; orb pane read 4 --lines 250]\ny\nz\n[exit 1]\n".to_owned()
            ),
            "cut output names the lines left out"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn run_wait_without_an_exit_prints_exit_unknown() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given an orb whose pane sends no prompt marks, its screen "done".
        let quiet =
            r#"{"ok":true,"result":{"exit":null,"output":"done","lines":1,"truncated":false}}"#;

        // When running `pane run 1 x --wait`.
        let printed = answered(&["pane", "run", "1", "x", "--wait"], quiet)?;

        // Then the exit prints as unknown.
        assert_eq!(
            printed,
            (0, "done\n[exit ?]\n".to_owned()),
            "no prompt marks means no exit code"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn run_wait_with_json_prints_the_json_shape() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given an orb where the command printed "a" and "b" and exited 0.
        // When running `pane run 1 x --wait --json`.
        let (code, out) = answered(&["pane", "run", "1", "x", "--wait", "--json"], RAN)?;

        // Then one JSON line carries the exit, output, lines and truncated.
        let printed: serde_json::Value = serde_json::from_str(&out)?;
        assert_eq!(
            (code, out.lines().count(), printed),
            (
                0,
                1,
                serde_json::json!({ "exit": 0, "output": "a\nb", "lines": 2, "truncated": false })
            ),
            "--json prints the reply's JSON, got {out}"
        );
        Ok(())
    }

    /// An orb answer to `pane wait`: the line "Listening on :3000".
    const MATCHED: &str = r#"{"ok":true,"result":{"line":"Listening on :3000"}}"#;

    #[rstest::rstest]
    fn wait_prints_the_matching_line() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given an orb whose pane 1 showed "Listening on :3000".
        // When running `pane wait 1 --text Listening`.
        let printed = answered(&["pane", "wait", "1", "--text", "Listening"], MATCHED)?;

        // Then the matching line prints as plain text.
        assert_eq!(
            printed,
            (0, "Listening on :3000\n".to_owned()),
            "pane wait prints the line it matched"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn wait_with_json_prints_the_json_shape() -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given an orb whose pane 1 showed "Listening on :3000".
        // When running `pane wait 1 --text Listening --json`.
        let (code, out) = answered(
            &["pane", "wait", "1", "--text", "Listening", "--json"],
            MATCHED,
        )?;

        // Then one JSON line carries the line.
        let printed: serde_json::Value = serde_json::from_str(&out)?;
        assert_eq!(
            (code, printed),
            (0, serde_json::json!({ "line": "Listening on :3000" })),
            "--json prints the reply's JSON, got {out}"
        );
        Ok(())
    }
}
