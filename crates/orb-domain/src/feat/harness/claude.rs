//! Claude Code, the harness orb started with: sessions run by its own
//! background supervisor, transcripts under its config dir, trust written to
//! its config file.

pub mod models;
pub mod supervisor;
pub mod transcript;
pub mod trust;

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use error_stack::{Report, ResultExt};

use self::trust::WorkspaceTrust;
use super::{Harness, HarnessError, HarnessId, HarnessInfo, Scan, TranscriptFormat};
use crate::feat::git::git_service::GitService;
use crate::feat::sessions::session_host::{
    AttachStart, CreatedSession, SessionHost, SessionHostError, SessionOptions, SessionRecord,
};
use crate::feat::sessions::transcript::{Exchange, MessageRead};

/// The id Claude Code threads are stored under.
pub const ID: &str = "claude";

/// The name the user sees for Claude Code.
const LABEL: &str = "Claude Code";

/// Claude Code: sessions through `host`, transcripts under `claude_dir`,
/// trust through `trust` at the repository's project folder.
pub struct ClaudeCode {
    host: Arc<dyn SessionHost>,
    trust: Arc<dyn WorkspaceTrust>,
    claude_dir: PathBuf,
    git: GitService,
}

impl ClaudeCode {
    pub fn new(
        host: Arc<dyn SessionHost>,
        trust: Arc<dyn WorkspaceTrust>,
        claude_dir: PathBuf,
        git: GitService,
    ) -> Self {
        Self {
            host,
            trust,
            claude_dir,
            git,
        }
    }
}

#[async_trait]
impl SessionHost for ClaudeCode {
    fn name(&self) -> &'static str {
        ID
    }

    async fn create(
        &self,
        cwd: &Path,
        options: &SessionOptions,
    ) -> Result<CreatedSession, Report<SessionHostError>> {
        self.host.create(cwd, options).await
    }

    async fn list(
        &self,
        short_ids: &[String],
    ) -> Result<Vec<SessionRecord>, Report<SessionHostError>> {
        self.host.list(short_ids).await
    }

    async fn stop(&self, short_id: &str) -> Result<(), Report<SessionHostError>> {
        self.host.stop(short_id).await
    }

    async fn remove(&self, short_id: &str) -> Result<(), Report<SessionHostError>> {
        self.host.remove(short_id).await
    }

    fn attach_argv(&self, short_id: &str, start: &AttachStart<'_>) -> Vec<OsString> {
        self.host.attach_argv(short_id, start)
    }
}

impl TranscriptFormat for ClaudeCode {
    fn locate(&self, cwd: &Path, session_id: &str) -> Option<PathBuf> {
        transcript::locate(&self.claude_dir, cwd, session_id)
    }

    fn scan(&self, path: &Path, offset: u64, previous: &Scan) -> io::Result<Scan> {
        transcript::scan_title(
            path,
            offset,
            previous.title.clone(),
            previous.custom_title.clone(),
            previous.branch.clone(),
        )
    }

    fn exchanges(&self, path: &Path) -> io::Result<(u64, Vec<Exchange>)> {
        transcript::read_exchanges(path)
    }

    fn messages(&self, path: &Path, offset: u64, prompt_offset: u64) -> io::Result<MessageRead> {
        transcript::read_messages(path, offset, prompt_offset)
    }
}

#[async_trait]
impl Harness for ClaudeCode {
    fn id(&self) -> HarnessId {
        HarnessId::new(ID)
    }

    fn label(&self) -> &'static str {
        LABEL
    }

    async fn probe(&self) -> HarnessInfo {
        models::info()
    }

    /// Claude trusts a repository at its main checkout, which covers every
    /// worktree of it.
    fn trust_dir(&self, cwd: &Path) -> PathBuf {
        self.git.project_path(cwd).unwrap_or_else(|| cwd.to_owned())
    }

    fn trust(&self, dir: &Path) -> Result<(), Report<HarnessError>> {
        self.trust.trust(dir).change_context(HarnessError)
    }

    fn resume_command(&self, session_id: &str) -> String {
        format!("claude --resume {session_id}")
    }
}
