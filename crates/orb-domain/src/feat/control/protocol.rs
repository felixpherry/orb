//! What the `orb` commands and a running orb say to each other over the socket:
//! one request line, one reply line, both JSON.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

use crate::feat::layout::tree::Split;
use crate::feat::plate::Verb;
use crate::feat::sessions::state::AgentStatus;
use crate::ids::{AgentId, PaneId, PlateItemId, ProjectId, SessionId};

/// The socket's file name inside orb's folder (`~/.orb`).
pub const SOCKET_FILE: &str = "orb.sock";

/// Where a running orb listens, given orb's folder.
pub fn socket_path(orb_root: &Path) -> PathBuf {
    orb_root.join(SOCKET_FILE)
}

/// How many graphemes of a name go on the wire.
const NAME_LIMIT: usize = 60;

/// `name` cut to [`NAME_LIMIT`] graphemes, ending in `…` when cut.
pub(crate) fn short_name(name: &str) -> String {
    cut(name, NAME_LIMIT)
}

/// `text` cut to `limit` graphemes, ending in `…` when cut.
pub(crate) fn cut(text: &str, limit: usize) -> String {
    let graphemes: Vec<&str> = text.graphemes(true).collect();
    if graphemes.len() <= limit {
        return text.to_owned();
    }
    let mut kept: String = graphemes
        .into_iter()
        .take(limit.saturating_sub(1))
        .collect();
    kept.push('…');
    kept
}

/// One request: who asks (the pane the command runs in, if any) and what for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// The pane the command was run from, when it runs inside orb.
    pub caller: Option<PaneId>,
    /// What is asked.
    #[serde(flatten)]
    pub op: Op,
}

/// What a request asks for, named on the wire as `<thing>.<verb>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum Op {
    /// Sessions: open ones, or every one with `all`.
    #[serde(rename = "session.list")]
    SessionList {
        /// Settled sessions too.
        #[serde(default)]
        all: bool,
        /// Only this project's.
        #[serde(default)]
        project: Option<ProjectId>,
    },
    /// A session's tabs and the panes in each.
    #[serde(rename = "tab.list")]
    TabList {
        /// The session, else the caller's.
        #[serde(default)]
        session: Option<SessionId>,
    },
    /// A session's panes, with the caller's marked.
    #[serde(rename = "pane.list")]
    PaneList {
        /// The session, else the caller's.
        #[serde(default)]
        session: Option<SessionId>,
    },
    /// Agents in panes: live ones, every one with `all`, or those with
    /// `status`.
    #[serde(rename = "agent.list")]
    AgentList {
        /// Stopped and gone agents too.
        #[serde(default)]
        all: bool,
        /// Only this session's.
        #[serde(default)]
        session: Option<SessionId>,
        /// Only these statuses; empty for the default.
        #[serde(default)]
        status: Vec<AgentStatus>,
    },
    /// Projects that aren't removed.
    #[serde(rename = "project.list")]
    ProjectList,
    /// Where the caller is: its pane, tab and session.
    #[serde(rename = "orb.here")]
    Here,
    /// A new shell beside a pane (or its whole stack).
    #[serde(rename = "pane.split")]
    PaneSplit {
        /// The pane, else the caller's.
        #[serde(default)]
        pane: Option<PaneId>,
        /// Which side the new pane goes.
        direction: Direction,
        /// A name for the new pane.
        #[serde(default)]
        name: Option<String>,
        /// A command line typed into the new shell, then Enter.
        #[serde(default)]
        run: Option<String>,
        /// The folder the new shell starts in, else the session's.
        #[serde(default)]
        cwd: Option<PathBuf>,
    },
    /// A new shell stacked right after a pane.
    #[serde(rename = "pane.stack")]
    PaneStack {
        /// The pane, else the caller's.
        #[serde(default)]
        pane: Option<PaneId>,
        /// A name for the new pane.
        #[serde(default)]
        name: Option<String>,
        /// A command line typed into the new shell, then Enter.
        #[serde(default)]
        run: Option<String>,
        /// The folder the new shell starts in, else the session's.
        #[serde(default)]
        cwd: Option<PathBuf>,
    },
    /// A new floating shell in a session.
    #[serde(rename = "pane.float")]
    PaneFloat {
        /// The session, else the caller's.
        #[serde(default)]
        session: Option<SessionId>,
        /// A name for the new pane.
        #[serde(default)]
        name: Option<String>,
        /// A command line typed into the new shell, then Enter.
        #[serde(default)]
        run: Option<String>,
        /// The folder the new shell starts in, else the session's.
        #[serde(default)]
        cwd: Option<PathBuf>,
    },
    /// A new tab with one shell at the end of a session's tabs.
    #[serde(rename = "tab.create")]
    TabCreate {
        /// The session, else the caller's.
        #[serde(default)]
        session: Option<SessionId>,
        /// A name for the new pane.
        #[serde(default)]
        name: Option<String>,
        /// A command line typed into the new shell, then Enter.
        #[serde(default)]
        run: Option<String>,
        /// The folder the new shell starts in, else the session's.
        #[serde(default)]
        cwd: Option<PathBuf>,
    },
    /// A new session.
    #[serde(rename = "session.create")]
    SessionCreate {
        /// The project, else the caller's session's.
        #[serde(default)]
        project: Option<ProjectId>,
        /// In a new worktree from the project's default branch, else in its
        /// checkout.
        #[serde(default)]
        worktree: bool,
        /// Its name, else the usual one.
        #[serde(default)]
        name: Option<String>,
    },
    /// Closes a pane.
    #[serde(rename = "pane.close")]
    PaneClose { pane: PaneId },
    /// Names a pane; an empty name goes back to the usual one.
    #[serde(rename = "pane.rename")]
    PaneRename { pane: PaneId, name: String },
    /// Shows a pane and moves the keys into it.
    #[serde(rename = "pane.focus")]
    PaneFocus { pane: PaneId },
    /// Closes the tab holding a pane.
    #[serde(rename = "tab.close")]
    TabClose {
        /// The pane, else the caller's.
        #[serde(default)]
        pane: Option<PaneId>,
    },
    /// Names a session; an empty name goes back to the usual one.
    #[serde(rename = "session.rename")]
    SessionRename {
        /// The session, else the caller's.
        #[serde(default)]
        session: Option<SessionId>,
        name: String,
    },
    /// Moves a session to the Settled shelf.
    #[serde(rename = "session.settle")]
    SessionSettle {
        /// The session, else the caller's.
        #[serde(default)]
        session: Option<SessionId>,
    },
    /// Shows a session's focused pane and moves the keys into it.
    #[serde(rename = "session.focus")]
    SessionFocus { session: SessionId },
    /// Pastes text into a pane.
    #[serde(rename = "pane.send")]
    PaneSend { pane: PaneId, text: String },
    /// Presses named keys in a pane, in order.
    #[serde(rename = "pane.keys")]
    PaneKeys { pane: PaneId, keys: Vec<String> },
    /// Types text into a pane: no paste, no Enter.
    #[serde(rename = "pane.type")]
    PaneType { pane: PaneId, text: String },
    /// Types a command and Enter into a pane's shell.
    #[serde(rename = "pane.run")]
    PaneRun {
        pane: PaneId,
        command: String,
        /// Wait for the command to end and reply with its output.
        #[serde(default)]
        wait: bool,
        /// With `wait`, give up after this many milliseconds.
        #[serde(default)]
        timeout: Option<u64>,
        /// Type even while a program runs in the pane.
        #[serde(default)]
        force: bool,
        /// With `wait`, keep the output's last `tail` lines (200 when left out).
        #[serde(default)]
        tail: Option<usize>,
    },
    /// A pane's screen, plus up to `lines` lines of history, as trimmed text.
    #[serde(rename = "pane.read")]
    PaneRead {
        /// The pane, else the caller's.
        #[serde(default)]
        pane: Option<PaneId>,
        /// How many lines of history to add above the screen.
        #[serde(default)]
        lines: usize,
        /// Keep the first `head` lines of the trimmed text.
        #[serde(default)]
        head: Option<usize>,
        /// Then keep the last `tail` lines.
        #[serde(default)]
        tail: Option<usize>,
    },
    /// Waits until a line of a pane's text holds `text` or matches `regex`.
    #[serde(rename = "pane.wait")]
    PaneWait {
        pane: PaneId,
        /// Text a line must hold.
        #[serde(default)]
        text: Option<String>,
        /// A pattern a line must match.
        #[serde(default)]
        regex: Option<String>,
        /// Give up after this many milliseconds.
        #[serde(default)]
        timeout: Option<u64>,
        /// Lines of history searched above the screen (200 when left out).
        #[serde(default)]
        lines: Option<usize>,
    },
    /// Starts an agent in a shell pane, or in a new one, and waits until orb
    /// sees it.
    #[serde(rename = "agent.start")]
    AgentStart {
        /// The pane, else the caller's.
        #[serde(default)]
        pane: Option<PaneId>,
        kind: AgentKind,
        /// A name for the pane.
        #[serde(default)]
        name: Option<String>,
        /// Typed after the program, each quoted for the shell.
        #[serde(default)]
        args: Vec<String>,
        /// Start in a new pane split off the caller's toward this side.
        #[serde(default)]
        split: Option<Direction>,
        /// Start in a new tab of the caller's session.
        #[serde(default)]
        tab: bool,
        /// Type the program into an existing pane even while another program
        /// runs there.
        #[serde(default)]
        force: bool,
    },
    /// Pastes a prompt into an agent and presses Enter.
    #[serde(rename = "agent.prompt")]
    AgentPrompt {
        pane: PaneId,
        text: String,
        /// Wait for the turn to end or the agent to be blocked.
        #[serde(default)]
        wait: bool,
        /// Milliseconds the whole call may take; none waits as long as work
        /// goes on.
        #[serde(default)]
        timeout: Option<u64>,
    },
    /// Waits until a pane's agent is idle or waits on the user.
    #[serde(rename = "agent.wait")]
    AgentWait {
        pane: PaneId,
        /// Milliseconds to wait; none waits as long as work goes on.
        #[serde(default)]
        timeout: Option<u64>,
    },
    /// A pane's agent in full.
    #[serde(rename = "agent.get")]
    AgentGet { pane: PaneId },
    /// The last reply in a pane's agent's transcript.
    #[serde(rename = "agent.reply")]
    AgentReply { pane: PaneId },
    /// A new item on the user's plate.
    #[serde(rename = "plate.add")]
    PlateAdd {
        /// The user's next step.
        title: String,
        /// One of the 11 verbs, lowercase; checked by orb.
        verb: String,
        /// What happened, a sentence or two.
        #[serde(default)]
        what: Option<String>,
        /// URLs or file paths, in order.
        #[serde(default)]
        links: Vec<String>,
        #[serde(default)]
        urgent: bool,
        /// Waiting on someone other than the user.
        #[serde(default)]
        waiting: bool,
        /// Where ⏎ goes, else the caller's pane.
        #[serde(default)]
        pane: Option<PaneId>,
    },
    /// Changes an open item; a field left out stays.
    #[serde(rename = "plate.update")]
    PlateUpdate {
        item: PlateItemId,
        #[serde(default)]
        title: Option<String>,
        /// One of the 11 verbs, lowercase; checked by orb.
        #[serde(default)]
        verb: Option<String>,
        #[serde(default)]
        what: Option<String>,
        /// Replaces every link when given.
        #[serde(default)]
        links: Option<Vec<String>>,
        #[serde(default)]
        urgent: Option<bool>,
        /// Waiting on others (true) or the user's move (false).
        #[serde(default)]
        waiting: Option<bool>,
    },
    /// Closes an item.
    #[serde(rename = "plate.done")]
    PlateDone { item: PlateItemId },
    /// Open items and those done today.
    #[serde(rename = "plate.list")]
    PlateList,
}

/// The program `agent start` types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Pi,
}

impl AgentKind {
    /// The program it runs, as `--kind` names it.
    pub fn program(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Pi => "pi",
        }
    }
}

/// Which side of a pane `pane split` puts the new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Right,
    Down,
}

impl From<Direction> for Split {
    fn from(direction: Direction) -> Self {
        match direction {
            Direction::Right => Self::Right,
            Direction::Down => Self::Down,
        }
    }
}

/// Why a request failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Nothing listens on the socket (only the client says this).
    NotRunning,
    /// The request couldn't be read or makes no sense.
    BadRequest,
    /// No target was given and the command doesn't run inside an orb pane.
    NoTarget,
    /// The pane, session or agent doesn't exist (or is being deleted).
    NotFound,
    /// The session is settled, so nothing in it may change.
    Settled,
    /// A colonist asked to add a plate item; colonists report to their chief.
    Colonist,
    /// A session is already starting.
    Starting,
    /// The agent waits for an approval or an answer.
    AgentBlocked,
    /// An agent is already live in that pane.
    PaneBusy,
    /// No live agent in that pane.
    NoAgent,
    /// The agent has no reply yet.
    NoReply,
    /// The wait ran out of time.
    Timeout,
    /// The agent didn't start working after the prompt.
    PromptStalled,
    /// A program runs in the pane's shell.
    Busy,
    /// The shell didn't run the typed command.
    NotRun,
    /// The agent's turn ended in a failed model call.
    TurnFailed,
    /// Something else went wrong; the message says what.
    Failed,
}

/// A failure as it goes on the wire: a code, a message for people and the
/// step that fixes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    /// Why it failed.
    pub code: ErrorCode,
    /// What happened, in words.
    pub message: String,
    /// The command or step that fixes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// The message as orb's log writes it, for a message that names panes,
    /// agents or sessions by title; never sent to the client.
    #[serde(skip)]
    pub logged: Option<String>,
}

impl ApiError {
    /// A refusal with this code, message and hint.
    pub fn new<M, H>(code: ErrorCode, message: M, hint: H) -> Self
    where
        M: Into<String>,
        H: Into<String>,
    {
        Self {
            code,
            message: message.into(),
            hint: Some(hint.into()),
            logged: None,
        }
    }

    /// A `failed` refusal for a failure inside orb, pointing at its log.
    pub fn failed<M>(message: M) -> Self
    where
        M: Into<String>,
    {
        Self::new(ErrorCode::Failed, message, SEE_LOG)
    }

    /// A `bad_request` refusal for a request handed to code that serves
    /// other kinds; `message` says which kind it expected.
    pub fn wrong_kind<M>(message: M) -> Self
    where
        M: Into<String>,
    {
        Self::new(
            ErrorCode::BadRequest,
            message,
            "orb help lists the commands",
        )
    }

    /// The same refusal, written to orb's log as `logged` in place of its
    /// message.
    #[must_use]
    pub fn logged_as<L>(self, logged: L) -> Self
    where
        L: Into<String>,
    {
        Self {
            logged: Some(logged.into()),
            ..self
        }
    }

    /// The message as orb's log writes it.
    pub fn log_message(&self) -> &str {
        self.logged.as_deref().unwrap_or(&self.message)
    }

    /// The same refusal with `hint` in place of its own.
    #[must_use]
    pub fn with_hint<H>(self, hint: H) -> Self
    where
        H: Into<String>,
    {
        Self {
            hint: Some(hint.into()),
            ..self
        }
    }
}

/// The hint for a failure inside orb.
pub const SEE_LOG: &str = "try again; orb's log at ~/.orb/orb.log may say why";

/// One reply: `ok` with a result, or not `ok` with an error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    /// Whether the request succeeded.
    pub ok: bool,
    /// What a successful request gives back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Why a request failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ApiError>,
}

impl Reply {
    /// A successful reply carrying `result`; a result that can't be written
    /// as JSON becomes a `failed` reply.
    pub fn ok<T>(result: &T) -> Self
    where
        T: Serialize,
    {
        match serde_json::to_value(result) {
            Ok(value) => Self {
                ok: true,
                result: Some(value),
                error: None,
            },
            Err(error) => Self::err(ApiError::failed(error.to_string())),
        }
    }

    /// A failed reply.
    pub fn err(error: ApiError) -> Self {
        Self {
            ok: false,
            result: None,
            error: Some(error),
        }
    }

    /// The reply's result (`null` when it carries none), or its refusal
    /// (`failed` when a refused reply names no error).
    ///
    /// # Errors
    ///
    /// Returns the reply's error when it isn't `ok`.
    pub fn into_result(self) -> Result<serde_json::Value, ApiError> {
        if self.ok {
            Ok(self.result.unwrap_or_default())
        } else {
            Err(self
                .error
                .unwrap_or_else(|| ApiError::failed("no error given")))
        }
    }

    /// `result` as a reply: its value, or its refusal.
    pub fn from_result<T>(result: Result<T, ApiError>) -> Self
    where
        T: Serialize,
    {
        result.map_or_else(Self::err, |ok| Self::ok(&ok))
    }
}

/// One session in `session list`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionRow {
    pub id: SessionId,
    pub project: ProjectId,
    /// The name the sidebar shows, cut to 60 graphemes.
    pub name: String,
    pub dir: String,
    pub branch: Option<String>,
    pub settled: bool,
}

/// `session list`'s result.
#[derive(Debug, Clone, Serialize)]
pub struct SessionsResult {
    pub sessions: Vec<SessionRow>,
    /// Settled sessions the open-only default left out.
    pub hidden: usize,
    /// How to see them, when `hidden > 0`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// One tab in `tab list`: its place and its panes.
#[derive(Debug, Clone, Serialize)]
pub struct TabRow {
    /// Its place in the tab bar, counted from 1.
    pub index: usize,
    pub panes: Vec<PaneId>,
}

/// `tab list`'s result.
#[derive(Debug, Clone, Serialize)]
pub struct TabsResult {
    pub session: SessionId,
    pub tabs: Vec<TabRow>,
}

/// One pane in `pane list`.
#[derive(Debug, Clone, Serialize)]
pub struct PaneRow {
    pub id: PaneId,
    pub session: SessionId,
    /// The tab holding it, counted from 1; none for a floating pane.
    pub tab: Option<usize>,
    pub float: bool,
    pub name: Option<String>,
    pub cwd: String,
    /// The agent live in it, if any.
    pub agent: Option<AgentId>,
    /// Whether it is the pane the command ran from.
    pub caller: bool,
}

/// `orb.here`'s result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HereResult {
    pub pane: PaneId,
    /// Its tab, counted from 1; none for a floating pane.
    pub tab: Option<usize>,
    pub session: SessionId,
    /// The session's name, cut as list names are.
    pub session_name: String,
    /// The agent live in the pane, if any.
    pub agent: Option<AgentId>,
}

/// `pane list`'s result.
#[derive(Debug, Clone, Serialize)]
pub struct PanesResult {
    pub panes: Vec<PaneRow>,
}

/// One agent in `agent list`: no cwd or branch, which its session's row
/// already has.
#[derive(Debug, Clone, Serialize)]
pub struct AgentRow {
    pub id: AgentId,
    pub pane: Option<PaneId>,
    pub session: SessionId,
    pub harness: String,
    /// The name the sidebar shows, cut to 60 graphemes.
    pub name: String,
    /// The agent's own title when it differs from `name`, cut the same way.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub status: AgentStatus,
}

/// One agent in full, as `agent get` shows it.
#[derive(Debug, Clone, Serialize)]
pub struct AgentDetail {
    pub id: AgentId,
    pub pane: Option<PaneId>,
    pub session: SessionId,
    pub harness: String,
    /// The name the sidebar shows.
    pub name: String,
    /// The agent's own title, once its transcript names it.
    pub title: Option<String>,
    pub status: AgentStatus,
    pub cwd: String,
    pub branch: Option<String>,
}

/// What an add replies: the new pane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneResult {
    pub pane: PaneId,
}

/// `session create`'s result: the new session and its first pane.
#[derive(Debug, Clone, Serialize)]
pub struct StartResult {
    pub session: SessionId,
    pub pane: PaneId,
}

/// What a change replies once done: nothing (`{}`).
#[derive(Debug, Clone, Serialize)]
#[expect(
    clippy::empty_structs_with_brackets,
    reason = "a unit struct would go on the wire as null, not {}"
)]
pub struct Done {}

/// One pane a close or settle ended: its own name, and the live agent's
/// list name (cut to 60 graphemes), each null when there is none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EndedRow {
    pub pane: PaneId,
    pub name: Option<String>,
    pub agent: Option<String>,
}

/// `pane close`'s and `tab close`'s result: the panes they ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClosedResult {
    pub ended: Vec<EndedRow>,
}

/// `session settle`'s result: the session, its name (cut to 60 graphemes),
/// and the panes it ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettledResult {
    pub session: SessionId,
    pub name: String,
    pub ended: Vec<EndedRow>,
}

/// `pane read`'s result: the screen as text, empty when it shows nothing.
#[derive(Debug, Clone, Serialize)]
pub struct ReadResult {
    pub text: String,
}

/// `pane run --wait`'s result: the exit status (null without prompt marks),
/// the output's last lines, how many lines it had, and whether its start was
/// gone from the pane's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunResult {
    pub exit: Option<i32>,
    pub output: String,
    pub lines: usize,
    pub truncated: bool,
}

/// `pane wait`'s result: the first line that matched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchResult {
    pub line: String,
}

/// `agent start`'s result: the agent orb saw start in the pane.
#[derive(Debug, Clone, Serialize)]
pub struct StartedResult {
    pub pane: PaneId,
    pub agent: AgentId,
    pub harness: String,
    pub status: AgentStatus,
}

/// `agent prompt`'s result without `--wait`.
#[derive(Debug, Clone, Serialize)]
pub struct PromptResult {
    pub submitted: bool,
}

/// How a wait on an agent ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitEnd {
    /// The turn ended.
    Idle,
    /// The agent waits for an approval or an answer.
    Blocked,
}

/// `agent wait`'s result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WaitResult {
    pub status: WaitEnd,
}

/// `agent prompt --wait`'s result: the turn ended, with everything the
/// agent wrote since the prompt and when, in unix milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnswerResult {
    pub status: WaitEnd,
    pub text: String,
    pub at: Option<i64>,
}

/// `agent reply`'s result: the last reply's text and when it was written,
/// in unix milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReplyResult {
    pub text: String,
    pub at: Option<i64>,
}

/// `agent list`'s result.
#[derive(Debug, Clone, Serialize)]
pub struct AgentsResult {
    pub agents: Vec<AgentRow>,
    /// Stopped and gone agents the live-only default left out.
    pub hidden: usize,
    /// How to see them, when `hidden > 0`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// One project in `project list`.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectRow {
    pub id: ProjectId,
    pub name: String,
    pub dir: String,
    /// repo, folder, research, learn, incognito, shared_world or fresh_world.
    pub kind: &'static str,
}

/// `project list`'s result.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectsResult {
    pub projects: Vec<ProjectRow>,
}

/// What `plate add` replies: the new item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemResult {
    pub item: PlateItemId,
}

/// One item in `plate list`.
#[derive(Debug, Clone, Serialize)]
pub struct PlateItemRow {
    pub id: PlateItemId,
    pub verb: Verb,
    /// `your_move`, `waiting`, `parked` or `done`.
    pub state: &'static str,
    pub title: String,
    pub what: Option<String>,
    pub links: Vec<String>,
    pub urgent: bool,
    /// The session it was added from; it may since be gone.
    pub session: SessionId,
    /// Where ⏎ goes.
    pub pane: PaneId,
    /// Whole seconds since the item last changed state.
    pub age_s: u64,
}

/// `plate list`'s result.
#[derive(Debug, Clone, Serialize)]
pub struct PlateResult {
    pub items: Vec<PlateItemRow>,
}
