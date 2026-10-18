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
pub(crate) const NAME_LIMIT: usize = 60;

/// `name` cut to [`NAME_LIMIT`] graphemes, ending in `…` when cut.
pub(crate) fn short_name(name: &str) -> String {
    let graphemes: Vec<&str> = name.graphemes(true).collect();
    if graphemes.len() <= NAME_LIMIT {
        return name.to_owned();
    }
    let mut cut: String = graphemes.into_iter().take(NAME_LIMIT - 1).collect();
    cut.push('…');
    cut
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
    /// Every session, settled ones included.
    #[serde(rename = "session.list")]
    SessionList,
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
    /// Every live agent in every session.
    #[serde(rename = "agent.list")]
    AgentList,
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
    },
    /// A new shell stacked right after a pane.
    #[serde(rename = "pane.stack")]
    PaneStack {
        /// The pane, else the caller's.
        #[serde(default)]
        pane: Option<PaneId>,
    },
    /// A new floating shell in a session.
    #[serde(rename = "pane.float")]
    PaneFloat {
        /// The session, else the caller's.
        #[serde(default)]
        session: Option<SessionId>,
    },
    /// A new tab with one shell at the end of a session's tabs.
    #[serde(rename = "tab.create")]
    TabCreate {
        /// The session, else the caller's.
        #[serde(default)]
        session: Option<SessionId>,
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
    /// Types a command and Enter into a pane.
    #[serde(rename = "pane.run")]
    PaneRun { pane: PaneId, command: String },
    /// A pane's screen, plus up to `lines` lines of history, as text.
    #[serde(rename = "pane.read")]
    PaneRead {
        /// The pane, else the caller's.
        #[serde(default)]
        pane: Option<PaneId>,
        /// How many lines of history to add above the screen.
        #[serde(default)]
        lines: usize,
    },
    /// Starts an agent in a shell pane and waits until orb sees it.
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
    /// A pane's agent, as `agent list` shows it.
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
        }
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
            Err(error) => Self::err(ApiError::new(ErrorCode::Failed, error.to_string(), SEE_LOG)),
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
}

/// One session in `session list`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionRow {
    pub id: SessionId,
    pub project: ProjectId,
    /// The name the sidebar shows.
    pub name: String,
    pub dir: String,
    pub branch: Option<String>,
    pub settled: bool,
}

/// `session list`'s result.
#[derive(Debug, Clone, Serialize)]
pub struct SessionsResult {
    pub sessions: Vec<SessionRow>,
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

/// One agent in `agent list`.
#[derive(Debug, Clone, Serialize)]
pub struct AgentRow {
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

/// `pane read`'s result: the screen as text, empty when it shows nothing.
#[derive(Debug, Clone, Serialize)]
pub struct ReadResult {
    pub text: String,
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

/// `agent wait`'s (and `agent prompt --wait`'s) result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WaitResult {
    pub status: WaitEnd,
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
