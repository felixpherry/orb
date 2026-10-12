//! What the `orb` commands and a running orb say to each other over the socket:
//! one request line, one reply line, both JSON.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::feat::sessions::state::AgentStatus;
use crate::ids::{AgentId, PaneId, ProjectId, SessionId};

/// The socket's file name inside orb's folder (`~/.orb`).
pub const SOCKET_FILE: &str = "orb.sock";

/// Where a running orb listens, given orb's folder.
pub fn socket_path(orb_root: &Path) -> PathBuf {
    orb_root.join(SOCKET_FILE)
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

/// A failure as it goes on the wire: a code and a message for people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    /// Why it failed.
    pub code: ErrorCode,
    /// What happened, in words.
    pub message: String,
}

impl ApiError {
    /// A failure with this code and message.
    pub fn new<M>(code: ErrorCode, message: M) -> Self
    where
        M: Into<String>,
    {
        Self {
            code,
            message: message.into(),
        }
    }
}

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
            Err(error) => Self::err(ApiError::new(ErrorCode::Failed, error.to_string())),
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
    pub status: AgentStatus,
    pub cwd: String,
    pub branch: Option<String>,
}

/// `agent list`'s result.
#[derive(Debug, Clone, Serialize)]
pub struct AgentsResult {
    pub agents: Vec<AgentRow>,
}
