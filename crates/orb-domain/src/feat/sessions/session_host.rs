//! Where Claude sessions run: starting one, listing what each is doing,
//! stopping or deleting one, and the command that attaches to one.

use std::ffi::OsString;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

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
/// trusted in yet. Running `claude` there and accepting its prompt fixes it.
#[derive(Debug, Clone, Copy)]
pub struct WorkspaceUntrusted;

/// A session that was just started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedSession {
    /// The id the host knows the session by.
    pub short_id: String,
}

/// One session the host knows about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub short_id: String,
    /// The Claude session id, which names the transcript file.
    pub session_id: Option<String>,
    pub status: ThreadStatus,
}

/// Runs Claude sessions in the background, independent of orb.
#[async_trait]
pub trait SessionHost: Send + Sync {
    fn name(&self) -> &'static str;

    /// Starts an idle session in `cwd`.
    ///
    /// # Errors
    ///
    /// Returns an error if the host refuses or fails to start the session.
    async fn create(&self, cwd: &Path) -> Result<CreatedSession, Report<SessionHostError>>;

    /// Every session the host knows about.
    ///
    /// # Errors
    ///
    /// Returns an error if the host can't be asked or its answer can't be read.
    async fn list(&self) -> Result<Vec<SessionRecord>, Report<SessionHostError>>;

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

/// Shared handle to the [`SessionHost`] in use.
#[derive(Clone)]
pub struct SessionHostService {
    host: Arc<dyn SessionHost>,
}

impl SessionHostService {
    pub fn new(host: Arc<dyn SessionHost>) -> Self {
        Self { host }
    }

    /// Starts an idle session in `cwd`.
    ///
    /// # Errors
    ///
    /// Returns an error if the host refuses or fails to start the session.
    pub async fn create(&self, cwd: &Path) -> Result<CreatedSession, Report<SessionHostError>> {
        self.host.create(cwd).await
    }

    /// Every session the host knows about.
    ///
    /// # Errors
    ///
    /// Returns an error if the host can't be asked or its answer can't be read.
    pub async fn list(&self) -> Result<Vec<SessionRecord>, Report<SessionHostError>> {
        self.host.list().await
    }

    /// Stops the session and keeps its conversation.
    ///
    /// # Errors
    ///
    /// Returns an error if the host refuses or fails to stop the session.
    pub async fn stop(&self, short_id: &str) -> Result<(), Report<SessionHostError>> {
        self.host.stop(short_id).await
    }

    /// Deletes the session.
    ///
    /// # Errors
    ///
    /// Returns an error if the host refuses or fails to delete the session.
    pub async fn remove(&self, short_id: &str) -> Result<(), Report<SessionHostError>> {
        self.host.remove(short_id).await
    }

    /// The command that attaches a terminal to the session.
    pub fn attach_argv(&self, short_id: &str) -> Vec<OsString> {
        self.host.attach_argv(short_id)
    }
}

impl fmt::Debug for SessionHostService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SessionHost<{}>", self.host.name())
    }
}
