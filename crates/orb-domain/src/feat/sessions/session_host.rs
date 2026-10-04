//! Where Claude sessions run: starting one, listing what each is doing,
//! stopping or deleting one, and the command that attaches to one.

use std::ffi::OsString;
use std::path::Path;

use async_trait::async_trait;
use error_stack::Report;
use wherror::Error;

use super::state::ThreadStatus;

/// A session host call failed. Every report carries a one-line reason as its
/// latest `String` attachment, fit for the mode line.
#[derive(Debug, Error)]
#[error(debug)]
pub struct SessionHostError;

/// Marks a [`SessionHostError`] as Claude refusing a directory it hasn't been
/// trusted in yet.
#[derive(Debug, Clone, Copy)]
pub struct WorkspaceUntrusted;

/// A session that was just started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedSession {
    /// The id the host knows the session by.
    pub short_id: String,
}

/// How to start a session; `None` leaves the setting to Claude's own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionOptions {
    /// The model alias or name.
    pub model: Option<String>,
    /// The permission mode.
    pub permission_mode: Option<String>,
}

/// One session the host knows about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub short_id: String,
    /// The Claude session id, which names the transcript file.
    pub session_id: Option<String>,
    pub status: ThreadStatus,
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
    /// it reports.
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

    /// The command that attaches a terminal to the session.
    fn attach_argv(&self, short_id: &str) -> Vec<OsString>;
}
