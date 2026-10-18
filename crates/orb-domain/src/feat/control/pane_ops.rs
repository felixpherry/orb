//! Pane requests: what a split, stack, float or new tab adds and what a send,
//! keys, type, run, read or wait comes to, and carrying out an add (naming the
//! new pane and starting its command), typing on the screen loop, a command
//! line typed into a shell and waited on, or a wait for a line to show.

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::AppState;
use crate::feat::control::agent;
use crate::feat::control::change::Change;
use crate::feat::control::pane_control::{
    PaneControl, PaneRequest, ShellPhase, ShellView, key_event,
};
use crate::feat::control::protocol::{
    ApiError, Done, ErrorCode, MatchResult, Op, PaneResult, Reply, RunResult, short_name,
};
use crate::feat::control::screen_text;
use crate::feat::control::server::{
    Action, ControlDeps, SPLIT_FLOAT, added, changing, changing_session, tiled,
};
use crate::feat::control::target::{Resolved, Target};
use crate::feat::sessions::sessions_actor::{ApiAdd, ApiPlace};
use crate::ids::{PaneId, SessionId};

/// What a split, stack, float or new tab comes to, read from `app`: the
/// pane or session it adds to, settled before a bad `cwd`.
pub(super) fn add_io(app: &AppState, op: &Op, caller: Option<PaneId>) -> Result<Action, ApiError> {
    match op {
        Op::PaneSplit {
            pane,
            direction,
            name,
            run,
            cwd,
        } => {
            let (session, at) = tiled(app, *pane, caller, SPLIT_FLOAT)?;
            let place = ApiPlace::Split {
                at,
                split: (*direction).into(),
            };
            adding(session, place, name.clone(), run.clone(), cwd.as_deref())
        }
        Op::PaneStack {
            pane,
            name,
            run,
            cwd,
        } => {
            let (session, at) = tiled(app, *pane, caller, SPLIT_FLOAT)?;
            adding(
                session,
                ApiPlace::Stack { at },
                name.clone(),
                run.clone(),
                cwd.as_deref(),
            )
        }
        Op::PaneFloat {
            session,
            name,
            run,
            cwd,
        } => {
            let session = changing_session(app, *session, caller)?;
            adding(
                session,
                ApiPlace::Float,
                name.clone(),
                run.clone(),
                cwd.as_deref(),
            )
        }
        Op::TabCreate {
            session,
            name,
            run,
            cwd,
        } => {
            let session = changing_session(app, *session, caller)?;
            adding(
                session,
                ApiPlace::Tab,
                name.clone(),
                run.clone(),
                cwd.as_deref(),
            )
        }
        _ => Err(ApiError::new(
            ErrorCode::BadRequest,
            "not an add",
            "orb help lists the commands",
        )),
    }
}

/// The add of a shell at `place` in `session`, in folder `cwd` when given:
/// `bad_request` before anything is added when `cwd` isn't an absolute path
/// to a folder.
fn adding(
    session: SessionId,
    place: ApiPlace,
    name: Option<String>,
    run: Option<String>,
    cwd: Option<&Path>,
) -> Result<Action, ApiError> {
    if let Some(dir) = cwd
        && !(dir.is_absolute() && dir.is_dir())
    {
        return Err(ApiError::new(
            ErrorCode::BadRequest,
            format!("no folder {}", dir.display()),
            "give --cwd an existing folder; orb pane list shows each pane's cwd",
        ));
    }
    Ok(Action::Add {
        add: ApiAdd {
            session,
            place,
            dir: cwd.map(Path::to_path_buf),
        },
        name,
        run,
    })
}

/// What a send, keys, type, run, read or wait comes to, read from `app`: settled
/// before an unknown key name or a bad pattern, so a refused list of keys sends
/// nothing.
pub(super) fn pane_io(app: &AppState, op: &Op, caller: Option<PaneId>) -> Result<Action, ApiError> {
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
                        ApiError::new(
                            ErrorCode::BadRequest,
                            format!("unknown key {name}"),
                            key_names(*pane),
                        )
                    })
                })
                .collect::<Result<_, _>>()?;
            Ok(Action::Pane(PaneRequest::Keys { pane: *pane, keys }))
        }
        Op::PaneType { pane, text } => {
            changing(app, Some(Target::Pane(*pane)), caller)?;
            Ok(Action::Pane(PaneRequest::Type {
                pane: *pane,
                text: text.clone(),
            }))
        }
        Op::PaneRun {
            pane,
            command,
            wait,
            timeout,
            force,
            tail,
        } => {
            changing(app, Some(Target::Pane(*pane)), caller)?;
            Ok(Action::Run(RunAsk {
                pane: *pane,
                command: command.clone(),
                wait: *wait,
                force: *force,
                timeout: timeout.map(Duration::from_millis),
                tail: tail.unwrap_or(TAIL),
            }))
        }
        Op::PaneRead {
            pane,
            lines,
            head,
            tail,
        } => match changing(app, pane.map(Target::Pane), caller)? {
            Resolved {
                pane: Some(pane), ..
            } => Ok(Action::Read {
                pane,
                lines: *lines,
                head: *head,
                tail: *tail,
            }),
            Resolved { pane: None, .. } => Err(ApiError::new(
                ErrorCode::BadRequest,
                "no pane to read",
                "name the pane: orb pane read <pane>",
            )),
        },
        Op::PaneWait {
            pane,
            text,
            regex,
            timeout,
            lines,
        } => {
            changing(app, Some(Target::Pane(*pane)), caller)?;
            Ok(Action::PaneWait(WaitAsk {
                pane: *pane,
                matcher: matcher(*pane, text.as_deref(), regex.as_deref())?,
                timeout: timeout.map(Duration::from_millis),
                lines: lines.unwrap_or(TAIL),
            }))
        }
        _ => Err(ApiError::new(
            ErrorCode::BadRequest,
            "not pane input or output",
            "orb help lists the commands",
        )),
    }
}

/// The hint for an unknown key name in `pane`: the names `pane keys` knows,
/// and the command that types text there.
fn key_names(pane: PaneId) -> String {
    format!(
        "key names: enter esc tab backspace space delete insert up down left right home end \
         pageup pagedown f1-f12 shift+tab ctrl+<a-z> alt+<char>, or one character; \
         for letters and text use orb pane type {} <text>",
        pane.0
    )
}

/// Asks the screen loop for `request` and replies `{}` once it's done.
pub(super) fn typed(deps: &ControlDeps, request: PaneRequest) -> Reply {
    match deps.panes.ask(request) {
        Ok(_) => Reply::ok(&Done {}),
        Err(error) => Reply::err(error),
    }
}

/// Adds the shell, then names it and types `run` into it on the screen loop,
/// replying with the new pane; a rename or run the loop refuses is replied
/// as that refusal, naming the new pane.
pub(super) fn add_pane(
    deps: &ControlDeps,
    add: ApiAdd,
    name: Option<String>,
    run: Option<String>,
) -> Reply {
    let pane = match added(deps, add) {
        Ok(pane) => pane,
        Err(error) => return Reply::err(error),
    };
    for request in follow_ups(pane, name, run) {
        if let Err(error) = deps.panes.ask(request) {
            return Reply::err(ApiError {
                message: format!("pane {} was added, but {}", pane.0, error.message),
                hint: Some(format!("orb pane read {} shows it", pane.0)),
                ..error
            });
        }
    }
    Reply::ok(&PaneResult { pane })
}

/// The screen-loop requests that name a new pane and start its command: the
/// rename first, then the run.
pub(super) fn follow_ups(
    pane: PaneId,
    name: Option<String>,
    run: Option<String>,
) -> Vec<PaneRequest> {
    let rename = name.map(|name| PaneRequest::Change(Change::RenamePane { pane, name }));
    let run = run.map(|line| PaneRequest::Run { pane, line });
    rename.into_iter().chain(run).collect()
}

/// How pane waits pace themselves; tests shrink it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PaneTiming {
    /// How often the pane is read.
    pub tick: Duration,
    /// How long `run --wait` waits to see the command start.
    pub start_gate: Duration,
    /// How long a screen must stay unchanged to count as quiet.
    pub quiet: Duration,
}

impl PaneTiming {
    /// The pace a running orb uses.
    pub(crate) const REAL: Self = Self {
        tick: Duration::from_millis(100),
        start_gate: Duration::from_secs(2),
        quiet: Duration::from_secs(1),
    };
}

/// The default number of lines `run --wait` keeps, and `pane wait` searches
/// above the screen.
const TAIL: usize = 200;

/// How many of the screen's last lines a `not_run` refusal quotes.
const NOT_RUN_LINES: usize = 10;

/// How many of the pane's last lines a `pane wait` timeout quotes.
const WAIT_LINES: usize = 5;

/// A command line to type into a shell pane, and how to wait on it.
#[derive(Debug)]
pub(super) struct RunAsk {
    pub pane: PaneId,
    pub command: String,
    /// Wait for the command to end and reply with its output.
    pub wait: bool,
    /// Type even while a program runs in the pane.
    pub force: bool,
    /// With `wait`, how long the whole call may take.
    pub timeout: Option<Duration>,
    /// With `wait`, how many of the output's last lines to keep.
    pub tail: usize,
}

/// Types `ask`'s command line into its pane and replies `{}`, or with
/// `wait`, the command's exit status and output once it ends. In a shell that
/// sends prompt marks, a running program refuses it as `busy` unless `force`,
/// and text left on the command line is cleared first; there the wait follows
/// the marks. Without marks, or forced into a running program, the wait ends
/// once the screen stays the same for a while. A wait stops once `gone` says
/// the caller hung up.
pub(super) fn run(
    panes: &PaneControl,
    ask: &RunAsk,
    gone: &dyn Fn() -> bool,
    timing: PaneTiming,
) -> Reply {
    let deadline = ask.timeout.map(|limit| Instant::now() + limit);
    let outcome = type_command(panes, ask).and_then(|before| {
        if !ask.wait {
            return Ok(None);
        }
        let marked = before.marks_seen && !matches!(before.phase, ShellPhase::Running { .. });
        if marked {
            marked_wait(panes, ask, before.seq, deadline, gone, timing).map(Some)
        } else {
            quiet_wait(panes, ask, deadline, gone, timing).map(Some)
        }
    });
    match outcome {
        Ok(None) => Reply::ok(&Done {}),
        Ok(Some(result)) => Reply::ok(&result),
        Err(error) => Reply::err(error),
    }
}

/// Checks the pane's shell, clears text left on its command line, and types
/// the command line, giving back the shell as it was before; `busy` while a
/// program runs there, unless `force`.
fn type_command(panes: &PaneControl, ask: &RunAsk) -> Result<ShellView, ApiError> {
    let before = panes.shell(ask.pane)?;
    match &before.phase {
        ShellPhase::Running { program, .. } if !ask.force => {
            return Err(busy(ask.pane, program.as_deref()));
        }
        ShellPhase::AtPrompt { leftover: true } => {
            panes.ask(PaneRequest::Keys {
                pane: ask.pane,
                keys: clear_line(),
            })?;
        }
        _ => {}
    }
    panes.ask(PaneRequest::Run {
        pane: ask.pane,
        line: ask.command.clone(),
    })?;
    Ok(before)
}

/// The keys that empty a shell's command line: to its end, then cut back
/// to its start.
fn clear_line() -> Vec<KeyEvent> {
    ['e', 'u']
        .into_iter()
        .map(|letter| KeyEvent::new(KeyCode::Char(letter), KeyModifiers::CONTROL))
        .collect()
}

/// Waits on the prompt marks for the command typed after mark `seq`: its
/// start within the start gate, then its end by `deadline`, and replies with
/// its exit status and output. `not_run` when it didn't start, `timeout` when
/// `deadline` came first.
fn marked_wait(
    panes: &PaneControl,
    ask: &RunAsk,
    seq: u64,
    deadline: Option<Instant>,
    gone: &dyn Fn() -> bool,
    timing: PaneTiming,
) -> Result<RunResult, ApiError> {
    let pane = ask.pane;
    let gate = Instant::now() + timing.start_gate;
    let start_by = deadline.map_or(gate, |deadline| deadline.min(gate));
    let started = poll(timing.tick, Some(start_by), gone, || {
        match panes.shell(pane) {
            Ok(view) => ours_started(&view, seq).then_some(Ok(())),
            Err(error) => Some(Err(error)),
        }
    });
    match started {
        Some(outcome) => outcome?,
        None if deadline.is_some_and(|deadline| deadline < gate) => {
            return Err(ApiError::new(
                ErrorCode::Timeout,
                format!(
                    "the shell in pane {} hadn't started the command at the timeout",
                    pane.0
                ),
                format!("orb pane read {} shows the pane", pane.0),
            ));
        }
        None => return Err(not_run(pane, &panes.text(pane, 0)?)),
    }
    let finished = poll(timing.tick, deadline, gone, || match panes.shell(pane) {
        Ok(view) => view.last.filter(|finished| finished.started > seq).map(Ok),
        Err(error) => Some(Err(error)),
    })
    .ok_or_else(|| {
        ApiError::new(
            ErrorCode::Timeout,
            format!(
                "the command in pane {} was still running at the timeout",
                pane.0
            ),
            format!(
                "orb pane read {0} shows where it got to; orb pane wait {0} --text <text> \
                 waits for a line",
                pane.0
            ),
        )
    })??;
    let lines = panes.between(pane, finished.start_line, finished.end_line)?;
    Ok(run_result(
        finished.exit,
        &lines.text,
        ask.tail,
        lines.truncated,
    ))
}

/// Whether the command typed after mark `seq` has started: it runs now, or
/// it already ended.
fn ours_started(view: &ShellView, seq: u64) -> bool {
    let running = matches!(view.phase, ShellPhase::Running { started, .. } if started > seq);
    running || view.last.is_some_and(|finished| finished.started > seq)
}

/// Reads the pane's screen and `tail` lines of history every tick until it
/// stays the same for `timing.quiet`, and replies with it, exit unknown. At
/// `deadline` it replies with the screen as it is then.
fn quiet_wait(
    panes: &PaneControl,
    ask: &RunAsk,
    deadline: Option<Instant>,
    gone: &dyn Fn() -> bool,
    timing: PaneTiming,
) -> Result<RunResult, ApiError> {
    let mut seen: Option<(String, Instant)> = None;
    let quiet = poll(timing.tick, deadline, gone, || {
        let text = match panes.text(ask.pane, ask.tail) {
            Ok(text) => text,
            Err(error) => return Some(Err(error)),
        };
        let now = Instant::now();
        match &seen {
            Some((last, since)) if *last == text => {
                (now.duration_since(*since) >= timing.quiet).then_some(Ok(text))
            }
            _ => {
                seen = Some((text, now));
                None
            }
        }
    });
    let text = match quiet {
        Some(outcome) => outcome?,
        None => seen.map(|(text, _)| text).unwrap_or_default(),
    };
    Ok(run_result(None, &text, ask.tail, false))
}

/// `run --wait`'s result from the output's `text`: trimmed, `lines` counted,
/// then cut to its last `tail` lines.
fn run_result(exit: Option<i32>, text: &str, tail: usize, truncated: bool) -> RunResult {
    let trimmed = screen_text::trim(text);
    RunResult {
        exit,
        output: screen_text::cut(&trimmed, None, Some(tail)),
        lines: trimmed.lines().count(),
        truncated,
    }
}

/// Runs `check` every `tick` until it gives an answer; `None` once
/// `deadline` passes, `timeout` once `gone` says the caller hung up.
fn poll<T, F>(
    tick: Duration,
    deadline: Option<Instant>,
    gone: &dyn Fn() -> bool,
    mut check: F,
) -> Option<Result<T, ApiError>>
where
    F: FnMut() -> Option<Result<T, ApiError>>,
{
    loop {
        let outcome = check();
        if outcome.is_some() {
            return outcome;
        }
        if gone() {
            return Some(Err(agent::caller_left()));
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return None;
        }
        thread::sleep(tick);
    }
}

/// What `pane wait` looks for, line by line.
#[derive(Debug)]
pub(super) enum Matcher {
    /// Text a line holds.
    Text(String),
    /// A pattern a line matches.
    Pattern(regex::Regex),
}

impl Matcher {
    /// Whether `line` holds the text or matches the pattern.
    fn matches(&self, line: &str) -> bool {
        match self {
            Self::Text(text) => line.contains(text.as_str()),
            Self::Pattern(pattern) => pattern.is_match(line),
        }
    }

    /// The text or pattern, as typed.
    fn shown(&self) -> &str {
        match self {
            Self::Text(text) => text,
            Self::Pattern(pattern) => pattern.as_str(),
        }
    }
}

/// A wait until a line of a pane's text matches.
#[derive(Debug)]
pub(super) struct WaitAsk {
    pub pane: PaneId,
    pub matcher: Matcher,
    /// How long the wait may take; no limit when `None`.
    pub timeout: Option<Duration>,
    /// How many lines of history above the screen are searched.
    pub lines: usize,
}

/// The matcher for `pane wait` in `pane` from exactly one of `text` and
/// `regex`: `bad_request` for neither, both, or a pattern that doesn't
/// compile.
fn matcher(pane: PaneId, text: Option<&str>, regex: Option<&str>) -> Result<Matcher, ApiError> {
    let hint = "orb pane wait --help";
    match (text, regex) {
        (Some(text), None) => Ok(Matcher::Text(text.to_owned())),
        (None, Some(pattern)) => {
            regex::Regex::new(pattern)
                .map(Matcher::Pattern)
                .map_err(|error| {
                    ApiError::new(
                        ErrorCode::BadRequest,
                        format!(
                            "`{pattern}` isn't a pattern: {}",
                            last_line(&error.to_string())
                        ),
                        hint,
                    )
                })
        }
        _ => Err(ApiError::new(
            ErrorCode::BadRequest,
            format!("give --text or --regex to wait on pane {}", pane.0),
            hint,
        )),
    }
}

/// The last line of `text` that isn't blank, trimmed.
pub(super) fn last_line(text: &str) -> &str {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
}

/// Reads the pane's screen and `ask.lines` lines of history every tick until
/// a line matches, from the top, and replies with that line. Text already on
/// screen counts. `timeout` quoting the pane's last lines once the timeout
/// passes; a wait stops once `gone` says the caller hung up.
pub(super) fn wait_for(
    panes: &PaneControl,
    ask: &WaitAsk,
    gone: &dyn Fn() -> bool,
    timing: PaneTiming,
) -> Reply {
    let deadline = ask.timeout.map(|limit| Instant::now() + limit);
    let mut last = String::new();
    let found = poll(timing.tick, deadline, gone, || {
        let text = match panes.text(ask.pane, ask.lines) {
            Ok(text) => screen_text::trim(&text),
            Err(error) => return Some(Err(error)),
        };
        let line = text
            .lines()
            .find(|line| ask.matcher.matches(line))
            .map(str::to_owned);
        last = text;
        line.map(Ok)
    });
    match found {
        Some(Ok(line)) => Reply::ok(&MatchResult { line }),
        Some(Err(error)) => Reply::err(error),
        None => Reply::err(no_match(ask, &last)),
    }
}

/// Why `pane wait` gave up: no line of the pane's text, which was `text`
/// last, matched within the timeout.
fn no_match(ask: &WaitAsk, text: &str) -> ApiError {
    let pane = ask.pane.0;
    let within = ask.timeout.unwrap_or_default().as_secs_f64();
    let end = screen_text::cut(text, None, Some(WAIT_LINES));
    ApiError::new(
        ErrorCode::Timeout,
        format!(
            "no line in pane {pane} showed `{}` within {within} s; the pane ends with:\n{end}",
            ask.matcher.shown()
        ),
        format!(
            "orb pane read {pane} shows the screen; text already on screen counts, so wait for \
             text only the new output prints"
        ),
    )
}

/// Why a command line can't go to `pane`: `program` (its first word) runs
/// there.
fn busy(pane: PaneId, program: Option<&str>) -> ApiError {
    let name = program.map_or_else(|| "a program".to_owned(), short_name);
    ApiError::new(
        ErrorCode::Busy,
        format!("{name} is running in pane {}", pane.0),
        format!(
            "quit it with orb pane keys {} <keys> (ctrl+c stops most programs), or add --force \
             to type anyway",
            pane.0
        ),
    )
}

/// Why a wait gave up: the shell in `pane`, whose screen is `screen`, never
/// started the typed command.
fn not_run(pane: PaneId, screen: &str) -> ApiError {
    let end = screen_text::cut(&screen_text::trim(screen), None, Some(NOT_RUN_LINES));
    ApiError::new(
        ErrorCode::NotRun,
        format!(
            "the shell in pane {} didn't run the command; its screen ends with:\n{end}",
            pane.0
        ),
        format!("fix the command; orb pane read {} shows the screen", pane.0),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::Duration;

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{Matcher, PaneTiming, RunAsk, WaitAsk, run, wait_for};
    use crate::feat::control::pane_control::{
        Finished, PaneAnswer, PaneControl, PaneLines, PaneRequest, ShellPhase, ShellView,
    };
    use crate::feat::control::protocol::{
        ApiError, Done, ErrorCode, MatchResult, Reply, RunResult,
    };
    use crate::ids::PaneId;

    /// Every request a fake screen loop was asked, in order.
    type Asked = Arc<Mutex<Vec<PaneRequest>>>;

    /// Waits paced for tests: a 1 ms tick, a 50 ms start gate and a 5 ms
    /// quiet spell.
    const QUICK: PaneTiming = PaneTiming {
        tick: Duration::from_millis(1),
        start_gate: Duration::from_millis(50),
        quiet: Duration::from_millis(5),
    };

    /// `items[n]`, or the last item once past the end.
    fn nth<T: Clone>(items: &[T], n: usize) -> Option<T> {
        items.get(n).or_else(|| items.last()).cloned()
    }

    /// A screen loop whose shell is `shells[n]` at the n-th `Shell` ask and
    /// whose screen is `screens[n]` at the n-th `Read` (each the last one
    /// after), whose lines are `lines` at every `Between`, and which carries
    /// out anything else; it records every request.
    fn screen_loop(
        shells: Vec<ShellView>,
        screens: Vec<&'static str>,
        lines: &'static str,
    ) -> (PaneControl, Asked) {
        let asked: Asked = Arc::default();
        let log = Arc::clone(&asked);
        let counts = Mutex::new((0_usize, 0_usize));
        let panes = PaneControl::new(move |request, reply| {
            let mut counts = counts.lock().unwrap_or_else(PoisonError::into_inner);
            let answer = match &request {
                PaneRequest::Shell { .. } => {
                    counts.0 += 1;
                    PaneAnswer::Shell(nth(&shells, counts.0 - 1).unwrap_or_default())
                }
                PaneRequest::Read { .. } => {
                    counts.1 += 1;
                    PaneAnswer::Text(nth(&screens, counts.1 - 1).unwrap_or_default().into())
                }
                PaneRequest::Between { .. } => PaneAnswer::Lines(PaneLines {
                    text: lines.to_owned(),
                    truncated: false,
                }),
                _ => PaneAnswer::Done,
            };
            log.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(request);
            let _ = reply.send(Ok(answer));
        });
        (panes, asked)
    }

    /// The requests in `asked` that type into the pane: no shell, screen or
    /// line reads.
    fn typing(asked: &Asked) -> Vec<PaneRequest> {
        asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|request| {
                !matches!(
                    request,
                    PaneRequest::Shell { .. }
                        | PaneRequest::Read { .. }
                        | PaneRequest::Between { .. }
                )
            })
            .cloned()
            .collect()
    }

    /// A shell at its prompt after mark `seq`, with text left on its
    /// command line when `leftover`.
    fn at_prompt(seq: u64, leftover: bool) -> ShellView {
        ShellView {
            marks_seen: true,
            phase: ShellPhase::AtPrompt { leftover },
            last: None,
            seq,
        }
    }

    /// A shell running `program`, started by mark `seq`.
    fn running(program: Option<&str>, seq: u64) -> ShellView {
        ShellView {
            marks_seen: true,
            phase: ShellPhase::Running {
                program: program.map(str::to_owned),
                started: seq,
            },
            last: None,
            seq,
        }
    }

    /// A shell back at its prompt after mark `seq`, its last command
    /// started by mark `started` and ended with `exit`.
    fn ended(seq: u64, started: u64, exit: i32) -> ShellView {
        ShellView {
            last: Some(Finished {
                exit: Some(exit),
                start_line: 10,
                end_line: 12,
                started,
            }),
            ..at_prompt(seq, false)
        }
    }

    /// `pane run 12 "just test"`, waiting when `wait`, typing anyway when
    /// `force`.
    fn ask(wait: bool, force: bool) -> RunAsk {
        RunAsk {
            pane: PaneId(12),
            command: "just test".into(),
            wait,
            force,
            timeout: None,
            tail: 200,
        }
    }

    /// The `Run` request that types `just test` into pane 12.
    fn typed_line() -> PaneRequest {
        PaneRequest::Run {
            pane: PaneId(12),
            line: "just test".into(),
        }
    }

    /// The reply `run` gives in a screen loop, with no hang-up.
    fn ran(panes: &PaneControl, ask: &RunAsk) -> Reply {
        run(panes, ask, &|| false, QUICK)
    }

    /// `reply`'s result as a run result.
    fn result(reply: &Reply) -> Option<RunResult> {
        serde_json::from_value(reply.result.clone()?).ok()
    }

    #[rstest::rstest]
    fn run_while_a_program_runs_is_refused_as_busy() {
        // Given pane 12's shell running vim.
        let (panes, asked) = screen_loop(vec![running(Some("vim"), 3)], vec![], "");

        // When running a command there.
        let reply = ran(&panes, &ask(false, false));

        // Then it is refused as busy, naming vim and --force, and nothing
        // was typed.
        let error = reply
            .error
            .unwrap_or_else(|| ApiError::new(ErrorCode::Failed, "", ""));
        assert_eq!(
            (
                error.code,
                error.message.as_str(),
                error.hint.is_some_and(|hint| hint.contains("--force")),
                typing(&asked),
            ),
            (ErrorCode::Busy, "vim is running in pane 12", true, vec![]),
            "a running program refuses the line"
        );
    }

    #[rstest::rstest]
    fn forced_run_types_while_a_program_runs() {
        // Given pane 12's shell running vim.
        let (panes, asked) = screen_loop(vec![running(Some("vim"), 3)], vec![], "");

        // When running a command there with --force.
        ran(&panes, &ask(false, true));

        // Then the line is typed anyway.
        assert_eq!(
            typing(&asked),
            vec![typed_line()],
            "--force types into the running program"
        );
    }

    #[rstest::rstest]
    fn run_after_leftover_text_clears_the_line_first() {
        // Given pane 12's shell at its prompt with text left on the line.
        let (panes, asked) = screen_loop(vec![at_prompt(2, true)], vec![], "");

        // When running a command there.
        ran(&panes, &ask(false, false));

        // Then ctrl+e ctrl+u empty the line before it is typed.
        let clear = PaneRequest::Keys {
            pane: PaneId(12),
            keys: vec![
                KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
                KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            ],
        };
        assert_eq!(
            typing(&asked),
            vec![clear, typed_line()],
            "leftover text is cleared, then the line typed"
        );
    }

    #[rstest::rstest]
    fn run_at_a_clean_prompt_types_only_the_line() {
        // Given pane 12's shell at an empty prompt.
        let (panes, asked) = screen_loop(vec![at_prompt(2, false)], vec![], "");

        // When running a command there.
        ran(&panes, &ask(false, false));

        // Then only the line is typed.
        assert_eq!(
            typing(&asked),
            vec![typed_line()],
            "a clean prompt needs no clearing"
        );
    }

    #[rstest::rstest]
    fn run_in_a_pane_without_marks_types_without_checking() {
        // Given pane 12's shell sending no prompt marks.
        let (panes, asked) = screen_loop(vec![ShellView::default()], vec![], "");

        // When running a command there.
        ran(&panes, &ask(false, false));

        // Then the line is typed as it is.
        assert_eq!(
            typing(&asked),
            vec![typed_line()],
            "without marks there is nothing to check"
        );
    }

    #[rstest::rstest]
    fn run_without_wait_replies_empty() {
        // Given pane 12's shell at an empty prompt.
        let (panes, _) = screen_loop(vec![at_prompt(2, false)], vec![], "");

        // When running a command there without --wait.
        let reply = ran(&panes, &ask(false, false));

        // Then the reply is {}.
        assert_eq!(
            reply,
            Reply::ok(&Done {}),
            "a run without --wait replies {{}}"
        );
    }

    #[rstest::rstest]
    fn run_wait_replies_exit_and_output() {
        // Given pane 12's shell at its prompt, then running the command,
        // then back at its prompt after it printed "a" and "b" and exited 0.
        let shells = vec![
            at_prompt(2, false),
            running(Some("just"), 3),
            ended(4, 3, 0),
        ];
        let (panes, _) = screen_loop(shells, vec![], "a\nb");

        // When running a command there with --wait.
        let reply = ran(&panes, &ask(true, false));

        // Then the reply holds its exit code and output.
        assert_eq!(
            result(&reply),
            Some(RunResult {
                exit: Some(0),
                output: "a\nb".into(),
                lines: 2,
                truncated: false,
            }),
            "--wait replies with the exit code and output, got {reply:?}"
        );
    }

    #[rstest::rstest]
    fn run_wait_sees_a_command_that_ended_between_polls() {
        // Given pane 12's shell at its prompt, then straight back at it
        // after the command ran and exited 1, never seen running.
        let (panes, _) = screen_loop(vec![at_prompt(2, false), ended(4, 3, 1)], vec![], "boom");

        // When running a command there with --wait.
        let reply = ran(&panes, &ask(true, false));

        // Then the reply holds that command's exit code.
        assert_eq!(
            result(&reply).and_then(|result| result.exit),
            Some(1),
            "a command that started and ended between polls still counts, got {reply:?}"
        );
    }

    #[rstest::rstest]
    fn run_wait_without_a_start_mark_is_not_run() {
        // Given pane 12's shell that stays at its prompt, its screen 12
        // numbered lines.
        let screen = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\nl11\nl12";
        let (panes, _) = screen_loop(vec![at_prompt(2, false)], vec![screen], "");

        // When running a command there with --wait.
        let reply = ran(&panes, &ask(true, false));

        // Then it is refused as not run, quoting the screen's last 10 lines
        // and saying to fix the command.
        let error = reply
            .error
            .unwrap_or_else(|| ApiError::new(ErrorCode::Failed, "", ""));
        assert_eq!(
            (
                error.code,
                error
                    .message
                    .ends_with("ends with:\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\nl11\nl12"),
                error
                    .hint
                    .is_some_and(|hint| hint.starts_with("fix the command")),
            ),
            (ErrorCode::NotRun, true, true),
            "a command the shell never started is not_run, got {}",
            error.message
        );
    }

    #[rstest::rstest]
    fn run_wait_past_its_timeout_is_a_timeout() {
        // Given pane 12's shell running the command and never ending it.
        let (panes, _) = screen_loop(
            vec![at_prompt(2, false), running(Some("just"), 3)],
            vec![],
            "",
        );

        // When running a command there with --wait and a 20 ms timeout.
        let timed = RunAsk {
            timeout: Some(Duration::from_millis(20)),
            ..ask(true, false)
        };
        let reply = ran(&panes, &timed);

        // Then it is refused as a timeout.
        assert_eq!(
            reply.error.map(|error| error.code),
            Some(ErrorCode::Timeout),
            "a command still running at the timeout is a timeout"
        );
    }

    #[rstest::rstest]
    fn run_wait_stops_when_the_caller_hangs_up() {
        // Given pane 12's shell running the command and never ending it.
        let (panes, _) = screen_loop(
            vec![at_prompt(2, false), running(Some("just"), 3)],
            vec![],
            "",
        );

        // When running a command there with --wait, no timeout, from a
        // caller that has hung up.
        let reply = run(&panes, &ask(true, false), &|| true, QUICK);

        // Then the wait ends saying the caller hung up.
        assert_eq!(
            reply.error.map(|error| (error.code, error.message)),
            Some((ErrorCode::Timeout, "the caller hung up".to_owned())),
            "a caller that hung up ends the wait"
        );
    }

    #[rstest::rstest]
    fn run_wait_keeps_the_last_tail_lines() {
        // Given pane 12's shell running a command that printed 5 lines.
        let shells = vec![at_prompt(2, false), running(Some("seq"), 3), ended(4, 3, 0)];
        let (panes, _) = screen_loop(shells, vec![], "1\n2\n3\n4\n5");

        // When running it with --wait --tail 2.
        let reply = ran(
            &panes,
            &RunAsk {
                tail: 2,
                ..ask(true, false)
            },
        );

        // Then the output is its last 2 lines, out of 5.
        assert_eq!(
            result(&reply).map(|result| (result.output, result.lines)),
            Some(("4\n5".to_owned(), 5)),
            "--tail keeps the last lines and counts them all, got {reply:?}"
        );
    }

    #[rstest::rstest]
    fn run_wait_timing_out_before_the_start_gate_is_a_timeout() {
        // Given pane 12's shell that stays at its prompt.
        let (panes, _) = screen_loop(vec![at_prompt(2, false)], vec!["$"], "");

        // When running a command there with --wait and a timeout shorter
        // than the start gate.
        let timed = RunAsk {
            timeout: Some(Duration::from_millis(5)),
            ..ask(true, false)
        };
        let reply = ran(&panes, &timed);

        // Then it is a timeout, not a command the shell refused.
        assert_eq!(
            reply.error.map(|error| error.code),
            Some(ErrorCode::Timeout),
            "the timeout came before the start gate"
        );
    }

    #[rstest::rstest]
    fn run_wait_without_marks_replies_once_the_screen_is_quiet() {
        // Given pane 12's shell sending no prompt marks, its screen showing
        // the typed line, then the command's output, then staying so.
        let screens = vec!["$ ls", "$ ls\na b"];
        let (panes, _) = screen_loop(vec![ShellView::default()], screens, "");

        // When running a command there with --wait.
        let reply = ran(&panes, &ask(true, false));

        // Then the reply is the quiet screen, exit unknown.
        assert_eq!(
            result(&reply),
            Some(RunResult {
                exit: None,
                output: "$ ls\na b".into(),
                lines: 2,
                truncated: false,
            }),
            "without marks the wait ends on a quiet screen, got {reply:?}"
        );
    }

    #[rstest::rstest]
    fn forced_run_wait_into_a_running_program_replies_once_the_screen_is_quiet() {
        // Given pane 12's shell running vim, its screen not changing.
        let (panes, _) = screen_loop(vec![running(Some("vim"), 3)], vec!["~"], "");

        // When running a command there with --force --wait.
        let reply = ran(&panes, &ask(true, true));

        // Then the reply is the quiet screen, exit unknown: no mark can
        // say when a line typed into vim is done.
        assert_eq!(
            result(&reply).map(|result| (result.exit, result.output)),
            Some((None, "~".to_owned())),
            "a forced wait falls back to a quiet screen, got {reply:?}"
        );
    }

    #[rstest::rstest]
    fn run_wait_without_marks_at_its_timeout_replies_with_the_screen() {
        // Given pane 12's shell sending no prompt marks, its screen never
        // quiet for long enough.
        let (panes, _) = screen_loop(vec![ShellView::default()], vec!["building"], "");
        let slow = PaneTiming {
            quiet: Duration::from_secs(60),
            ..QUICK
        };

        // When running a command there with --wait and a 20 ms timeout.
        let timed = RunAsk {
            timeout: Some(Duration::from_millis(20)),
            ..ask(true, false)
        };
        let reply = run(&panes, &timed, &|| false, slow);

        // Then the reply is the screen as it was, not a refusal.
        assert_eq!(
            result(&reply).map(|result| result.output),
            Some("building".to_owned()),
            "the quiet wait replies with the screen at its timeout, got {reply:?}"
        );
    }

    /// `pane wait 12` for `matcher`, with no timeout.
    fn wait_ask(matcher: Matcher) -> WaitAsk {
        WaitAsk {
            pane: PaneId(12),
            matcher,
            timeout: None,
            lines: 200,
        }
    }

    /// The reply `wait_for` gives in a screen loop whose screen is
    /// `screens[n]` at the n-th read, with no hang-up.
    fn waited(screens: Vec<&'static str>, ask: &WaitAsk) -> Reply {
        let (panes, _) = screen_loop(vec![], screens, "");
        wait_for(&panes, ask, &|| false, QUICK)
    }

    /// The reply holding `line` as the line a wait matched.
    fn matched(line: &str) -> Reply {
        Reply::ok(&MatchResult { line: line.into() })
    }

    #[rstest::rstest]
    fn pane_wait_returns_once_the_text_shows() {
        // Given pane 12's screen without "Listening" for two reads, then
        // with it.
        let screens = vec![
            "building",
            "building\nstarting",
            "starting\nListening on :3000",
        ];

        // When waiting for "Listening".
        let reply = waited(screens, &wait_ask(Matcher::Text("Listening".into())));

        // Then the reply is the line that showed it.
        assert_eq!(
            reply,
            matched("Listening on :3000"),
            "the wait ends on the first read holding the text"
        );
    }

    #[rstest::rstest]
    fn pane_wait_counts_text_already_on_screen() {
        // Given pane 12's screen already showing "ready" above a later
        // line.
        let screens = vec!["ready 1\nother\nready 2"];

        // When waiting for "ready".
        let reply = waited(screens, &wait_ask(Matcher::Text("ready".into())));

        // Then the reply is the first such line from the top.
        assert_eq!(
            reply,
            matched("ready 1"),
            "text already on screen counts, first line from the top"
        );
    }

    #[rstest::rstest]
    fn pane_wait_matches_a_regex() {
        // Given pane 12's screen showing a test summary.
        let screens = vec!["running 3 tests\ntest result: FAILED. 1 passed"];
        let pattern = regex::Regex::new("test result: (ok|FAILED)").ok();

        // When waiting for the pattern `test result: (ok|FAILED)`.
        let reply = pattern.map(|pattern| waited(screens, &wait_ask(Matcher::Pattern(pattern))));

        // Then the reply is the line that matched.
        assert_eq!(
            reply,
            Some(matched("test result: FAILED. 1 passed")),
            "a pattern matches line by line"
        );
    }

    #[rstest::rstest]
    fn pane_wait_past_its_timeout_names_the_last_lines() {
        // Given pane 12's screen of 7 numbered lines that never show "done".
        let screens = vec!["l1\nl2\nl3\nl4\nl5\nl6\nl7"];

        // When waiting for "done" with a 20 ms timeout.
        let ask = WaitAsk {
            timeout: Some(Duration::from_millis(20)),
            ..wait_ask(Matcher::Text("done".into()))
        };
        let reply = waited(screens, &ask);

        // Then it is a timeout quoting the screen's last 5 lines.
        let error = reply
            .error
            .unwrap_or_else(|| ApiError::new(ErrorCode::Failed, "", ""));
        assert_eq!(
            (
                error.code,
                error.message.ends_with("ends with:\nl3\nl4\nl5\nl6\nl7"),
            ),
            (ErrorCode::Timeout, true),
            "a wait past its timeout names the pane's last lines, got {}",
            error.message
        );
    }

    #[rstest::rstest]
    fn pane_wait_stops_when_the_caller_hangs_up() {
        // Given pane 12's screen that never shows "done".
        let (panes, _) = screen_loop(vec![], vec!["building"], "");

        // When waiting for "done", no timeout, from a caller that has hung
        // up.
        let ask = wait_ask(Matcher::Text("done".into()));
        let reply = wait_for(&panes, &ask, &|| true, QUICK);

        // Then the wait ends saying the caller hung up.
        assert_eq!(
            reply.error.map(|error| (error.code, error.message)),
            Some((ErrorCode::Timeout, "the caller hung up".to_owned())),
            "a caller that hung up ends the wait"
        );
    }
}
