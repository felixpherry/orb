//! The control server's line to the screen loop: hands it a request and waits
//! for the loop's answer, so changes to what the screen owns happen on the
//! loop's thread.

use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::feat::control::change::Change;
use crate::feat::control::protocol::{ApiError, ErrorCode, SEE_LOG};
use crate::ids::PaneId;

/// What an agent asks the screen loop to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneRequest {
    Change(Change),
    /// Paste `text` into the pane (bracketed when its program turned that on).
    Send {
        pane: PaneId,
        text: String,
    },
    /// Press `keys` in the pane, in order.
    Keys {
        pane: PaneId,
        keys: Vec<KeyEvent>,
    },
    /// Type `text` into the pane as raw input: no bracketed paste, no Enter,
    /// control characters left out.
    Type {
        pane: PaneId,
        text: String,
    },
    /// Type `line` and Enter into the pane as raw input, so a shell runs it.
    Run {
        pane: PaneId,
        line: String,
    },
    /// Type `line` and Enter as `Run` does, unless a line already waits to be
    /// typed in the pane (`pane_busy`).
    Launch {
        pane: PaneId,
        line: String,
    },
    /// Paste `text`, then press Enter, with nothing in between.
    Prompt {
        pane: PaneId,
        text: String,
    },
    /// The pane's screen, after up to `lines` lines of history, as text.
    Read {
        pane: PaneId,
        lines: usize,
    },
    /// What the pane's shell is doing.
    Shell {
        pane: PaneId,
    },
    /// The text of the pane's absolute lines `start..end`.
    Between {
        pane: PaneId,
        start: i64,
        end: i64,
    },
}

/// What a pane's shell is doing, as the screen loop saw it from the prompt
/// marks the shell sends.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShellView {
    /// Whether the shell sent any prompt mark.
    pub marks_seen: bool,
    pub phase: ShellPhase,
    /// The last command that ended.
    pub last: Option<Finished>,
    /// Goes up by one on every mark.
    pub seq: u64,
}

/// Where a pane's shell is, between marks.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ShellPhase {
    /// No mark yet.
    #[default]
    Unknown,
    /// At its prompt; `leftover` when text sits on the command line (the
    /// cursor moved since the prompt ended).
    AtPrompt { leftover: bool },
    /// Running `program` (its first word), started by the mark with seq
    /// `started`.
    Running {
        program: Option<String>,
        started: u64,
    },
}

/// A command that ended: its exit status, its output's absolute lines
/// (`end_line` not included), and the seq of the mark that started it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Finished {
    pub exit: Option<i32>,
    pub start_line: i64,
    pub end_line: i64,
    pub started: u64,
}

/// Lines a `Between` read, and whether their start had already scrolled out
/// of the pane's history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneLines {
    pub text: String,
    pub truncated: bool,
}

/// The loop's answer to one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneAnswer {
    /// The request was carried out.
    Done,
    /// The screen text a `Read` asked for.
    Text(String),
    /// The shell a `Shell` asked about.
    Shell(ShellView),
    /// The lines a `Between` asked for.
    Lines(PaneLines),
}

/// The loop's answer, or its refusal.
pub type PaneReply = Result<PaneAnswer, ApiError>;

/// The key `name` stands for in `orb pane keys`: one character, sent as its
/// key (an uppercase letter as the shifted one); `enter esc tab backspace space
/// delete insert up down left right home end pageup pagedown`, `f1`–`f12`,
/// `shift+tab` and `ctrl+<a-z>` in any case; `alt+<char>`, the character as
/// typed. `None` for any other name, a control character, or more than one
/// character.
pub(crate) fn key_event(name: &str) -> Option<KeyEvent> {
    if let Some(key) = character(name) {
        return Some(key);
    }
    let lower = name.to_ascii_lowercase();
    let code = match lower.as_str() {
        "enter" => KeyCode::Enter,
        "esc" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace,
        "space" => KeyCode::Char(' '),
        "delete" => KeyCode::Delete,
        "insert" => KeyCode::Insert,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "shift+tab" => return Some(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
        other => match function_key(other) {
            Some(code) => code,
            None => return modified(name),
        },
    };
    Some(KeyEvent::new(code, KeyModifiers::NONE))
}

/// `word` as one key press when it is exactly one character that isn't a
/// control character; an uppercase letter carries SHIFT, as a typed
/// Shift+letter is reported.
fn character(word: &str) -> Option<KeyEvent> {
    let mut chars = word.chars();
    let (Some(only), None) = (chars.next(), chars.next()) else {
        return None;
    };
    if only.is_control() {
        return None;
    }
    let modifiers = if only.is_uppercase() {
        KeyModifiers::SHIFT
    } else {
        KeyModifiers::NONE
    };
    Some(KeyEvent::new(KeyCode::Char(only), modifiers))
}

/// `f1`–`f12` as their function keys; `None` for `f0`, `f13` and any other
/// name.
fn function_key(name: &str) -> Option<KeyCode> {
    let number = name.strip_prefix('f')?.parse::<u8>().ok()?;
    (1..=12).contains(&number).then_some(KeyCode::F(number))
}

/// `ctrl+<a-z>` (the letter in any case) or `alt+<char>` (`character`'s key
/// plus ALT); the prefix matches in any case.
fn modified(name: &str) -> Option<KeyEvent> {
    let (prefix, rest) = name.split_once('+')?;
    if prefix.eq_ignore_ascii_case("ctrl") {
        let letter = rest
            .to_ascii_lowercase()
            .parse::<char>()
            .ok()
            .filter(char::is_ascii_lowercase)?;
        Some(KeyEvent::new(KeyCode::Char(letter), KeyModifiers::CONTROL))
    } else if prefix.eq_ignore_ascii_case("alt") {
        let key = character(rest)?;
        Some(KeyEvent::new(key.code, key.modifiers | KeyModifiers::ALT))
    } else {
        None
    }
}

/// How long a request waits for the loop.
const LOOP_WAIT: Duration = Duration::from_secs(5);

/// Hands requests to the screen loop and waits for its answer. Cloning shares
/// the loop.
#[derive(Clone)]
pub struct PaneControl {
    send: Arc<dyn Fn(PaneRequest, Sender<PaneReply>) + Send + Sync>,
}

impl PaneControl {
    /// A handle that passes each request, with where to answer it, to `send`.
    pub fn new<F>(send: F) -> Self
    where
        F: Fn(PaneRequest, Sender<PaneReply>) + Send + Sync + 'static,
    {
        Self {
            send: Arc::new(send),
        }
    }

    /// Asks the loop to carry out `request` and waits for its answer.
    ///
    /// # Errors
    ///
    /// Returns the loop's refusal, or `failed` when the loop is gone or takes
    /// longer than five seconds.
    pub fn ask(&self, request: PaneRequest) -> PaneReply {
        let (tx, rx) = mpsc::channel();
        (self.send)(request, tx);
        rx.recv_timeout(LOOP_WAIT).unwrap_or_else(|_| {
            Err(ApiError::new(
                ErrorCode::Failed,
                "orb's screen didn't answer",
                SEE_LOG,
            ))
        })
    }
}

impl PaneControl {
    /// Pane `pane`'s screen after up to `lines` lines of history, as text.
    ///
    /// # Errors
    ///
    /// Returns what [`Self::ask`] does, or `failed` when the loop answers
    /// with something other than text.
    pub fn text(&self, pane: PaneId, lines: usize) -> Result<String, ApiError> {
        match self.ask(PaneRequest::Read { pane, lines })? {
            PaneAnswer::Text(text) => Ok(text),
            _ => Err(wrong_kind()),
        }
    }

    /// What pane `pane`'s shell is doing.
    ///
    /// # Errors
    ///
    /// Returns what [`Self::ask`] does, or `failed` when the loop answers
    /// with something other than the shell.
    pub fn shell(&self, pane: PaneId) -> Result<ShellView, ApiError> {
        match self.ask(PaneRequest::Shell { pane })? {
            PaneAnswer::Shell(shell) => Ok(shell),
            _ => Err(wrong_kind()),
        }
    }

    /// The text of pane `pane`'s absolute lines `start..end`.
    ///
    /// # Errors
    ///
    /// Returns what [`Self::ask`] does, or `failed` when the loop answers
    /// with something other than lines.
    pub fn between(&self, pane: PaneId, start: i64, end: i64) -> Result<PaneLines, ApiError> {
        match self.ask(PaneRequest::Between { pane, start, end })? {
            PaneAnswer::Lines(lines) => Ok(lines),
            _ => Err(wrong_kind()),
        }
    }
}

/// The refusal for an answer of the wrong kind.
fn wrong_kind() -> ApiError {
    ApiError::new(
        ErrorCode::Failed,
        "orb's screen answered with the wrong kind of reply",
        SEE_LOG,
    )
}

impl std::fmt::Debug for PaneControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PaneControl")
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{PaneAnswer, PaneControl, key_event};
    use crate::feat::control::protocol::ErrorCode;
    use crate::ids::PaneId;

    #[rstest::rstest]
    #[case::enter("enter", KeyCode::Enter, KeyModifiers::NONE)]
    #[case::any_case("Esc", KeyCode::Esc, KeyModifiers::NONE)]
    #[case::pageup("pageup", KeyCode::PageUp, KeyModifiers::NONE)]
    #[case::shift_tab("shift+tab", KeyCode::BackTab, KeyModifiers::SHIFT)]
    #[case::ctrl("ctrl+c", KeyCode::Char('c'), KeyModifiers::CONTROL)]
    #[case::alt("alt+b", KeyCode::Char('b'), KeyModifiers::ALT)]
    #[case::letter("q", KeyCode::Char('q'), KeyModifiers::NONE)]
    #[case::uppercase_letter("Q", KeyCode::Char('Q'), KeyModifiers::SHIFT)]
    #[case::colon(":", KeyCode::Char(':'), KeyModifiers::NONE)]
    #[case::space("space", KeyCode::Char(' '), KeyModifiers::NONE)]
    #[case::delete("delete", KeyCode::Delete, KeyModifiers::NONE)]
    #[case::insert("insert", KeyCode::Insert, KeyModifiers::NONE)]
    #[case::f5("f5", KeyCode::F(5), KeyModifiers::NONE)]
    #[case::f12("F12", KeyCode::F(12), KeyModifiers::NONE)]
    #[case::alt_symbol("alt+.", KeyCode::Char('.'), KeyModifiers::ALT)]
    #[case::alt_uppercase("alt+B", KeyCode::Char('B'), KeyModifiers::ALT | KeyModifiers::SHIFT)]
    #[case::alt_any_case("ALT+b", KeyCode::Char('b'), KeyModifiers::ALT)]
    #[case::ctrl_any_case("Ctrl+C", KeyCode::Char('c'), KeyModifiers::CONTROL)]
    fn key_names_make_their_key_events(
        #[case] name: &str,
        #[case] code: KeyCode,
        #[case] modifiers: KeyModifiers,
    ) {
        // Given / When looking up the key name.
        let key = key_event(name);

        // Then it is the key it names.
        assert_eq!(
            key,
            Some(KeyEvent::new(code, modifiers)),
            "{name} names its key"
        );
    }

    #[rstest::rstest]
    #[case::ctrl_digit("ctrl+1")]
    #[case::ctrl_two_letters("ctrl+ab")]
    #[case::unknown("f13")]
    #[case::empty("")]
    #[case::f0("f0")]
    #[case::bare_alt("alt+")]
    #[case::alt_two_letters("alt+ab")]
    #[case::two_letters("ab")]
    #[case::control_character("\u{3}")]
    #[case::alt_control_character("alt+\u{3}")]
    fn other_key_names_make_nothing(#[case] name: &str) {
        // Given / When looking up a name outside the table.
        let key = key_event(name);

        // Then there is no key.
        assert_eq!(key, None, "{name:?} names no key");
    }

    #[rstest::rstest]
    fn shell_answered_with_text_is_refused_as_failed() {
        // Given a screen loop that answers every request with text.
        let panes = PaneControl::new(|_, reply| {
            let _ = reply.send(Ok(PaneAnswer::Text("hi".into())));
        });

        // When asking it what pane 1's shell is doing.
        let shell = panes.shell(PaneId(1));

        // Then the answer is refused as failed.
        assert_eq!(
            shell.map_err(|error| error.code),
            Err(ErrorCode::Failed),
            "an answer of the wrong kind is a failure"
        );
    }
}
