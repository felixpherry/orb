//! Where a harness's sessions run: starting one, listing what each is doing,
//! stopping or deleting one, and the command a pane runs for one.

use std::ffi::OsString;
use std::path::Path;

use async_trait::async_trait;
use error_stack::Report;
use wherror::Error;

use super::state::ThreadStatus;
use crate::feat::zmx::zmx_service::ZmxSession;

/// A session host call failed. Every report carries a one-line reason as its
/// latest `String` attachment, fit for the mode line.
#[derive(Debug, Error)]
#[error(debug)]
pub struct SessionHostError;

/// Marks a [`SessionHostError`] as the harness refusing a directory it hasn't
/// been trusted in yet.
#[derive(Debug, Clone, Copy)]
pub struct WorkspaceUntrusted;

/// A session that was just started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedSession {
    /// The id the host knows the session by.
    pub short_id: String,
}

/// How to start a session; `None` leaves the setting to the harness's own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionOptions {
    /// The model alias or name.
    pub model: Option<String>,
    /// The permission mode.
    pub permission_mode: Option<String>,
}

/// What a thread's attach needs to start its session when nothing runs it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttachStart<'a> {
    /// The model the thread was started with, if one was picked.
    pub model: Option<&'a str>,
    /// Whether the harness has written the session's transcript yet.
    pub has_transcript: bool,
}

/// One session the host knows about: one it runs itself, by its id, or one
/// the user started in a terminal, by its process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    /// The id the host runs the session by; `None` for one the user started.
    pub short_id: Option<String>,
    /// The harness's session id, which names the transcript file; `None` for
    /// one the user started, whose identity comes from orb's hook.
    pub session_id: Option<String>,
    pub status: ThreadStatus,
    /// For one the user started: its process, then each parent up to the
    /// first process. Empty otherwise.
    pub ancestry: Vec<u32>,
}

/// Runs sessions in the background, independent of orb.
#[async_trait]
pub trait SessionHost: Send + Sync {
    fn name(&self) -> &'static str;

    /// Starts an idle session in `cwd` with `options`.
    ///
    /// # Errors
    ///
    /// Returns an error if the host refuses or fails to start the session.
    async fn create(
        &self,
        cwd: &Path,
        options: &SessionOptions,
    ) -> Result<CreatedSession, Report<SessionHostError>>;

    /// The sessions among `short_ids` the host knows about, and any others
    /// it reports, including sessions users started in a terminal.
    ///
    /// # Errors
    ///
    /// Returns an error if the host can't be asked or its answer can't be read.
    async fn list(
        &self,
        short_ids: &[String],
    ) -> Result<Vec<SessionRecord>, Report<SessionHostError>>;

    /// Stops the session and keeps its conversation.
    ///
    /// # Errors
    ///
    /// Returns an error if the host refuses or fails to stop the session.
    async fn stop(&self, short_id: &str) -> Result<(), Report<SessionHostError>>;

    /// Deletes the session.
    ///
    /// # Errors
    ///
    /// Returns an error if the host refuses or fails to delete the session.
    async fn remove(&self, short_id: &str) -> Result<(), Report<SessionHostError>>;

    /// The command a pane runs for the session. A harness whose command can
    /// also start the session uses `start` for that.
    fn attach_argv(&self, short_id: &str, start: &AttachStart<'_>) -> Vec<OsString>;

    /// The zmx session the host already runs the session in, which a pane
    /// joins instead of making its own; `None` (the default) when the host
    /// doesn't use zmx.
    fn zmx_session(&self, _short_id: &str) -> Option<ZmxSession> {
        None
    }
}
