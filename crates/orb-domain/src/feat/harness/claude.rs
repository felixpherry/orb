//! Claude Code: the interactive Claudes it runs, seen by process,
//! transcripts under its config dir, and the one stop of a background
//! session the store's migration replaced.

pub mod supervisor;
pub mod transcript;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use error_stack::Report;

use self::supervisor::ClaudeAgents;
use super::{Harness, HarnessError, HarnessId, HarnessInfo, RunningAgent, Scan, TranscriptFormat};
use crate::feat::sessions::transcript::{Exchange, MessageRead};

/// The id Claude Code threads are stored under.
pub const ID: &str = "claude";

/// The name the user sees for Claude Code.
const LABEL: &str = "Claude Code";

/// How the frontend marks Claude Code: its name and `✳`.
pub fn info() -> HarnessInfo {
    HarnessInfo {
        id: HarnessId::new(ID),
        label: LABEL.to_owned(),
        icon: Some("✳".to_owned()),
    }
}

/// Claude Code: its agents through `agents`, transcripts under `claude_dir`.
pub struct ClaudeCode {
    agents: Arc<dyn ClaudeAgents>,
    claude_dir: PathBuf,
}

impl ClaudeCode {
    pub fn new(agents: Arc<dyn ClaudeAgents>, claude_dir: PathBuf) -> Self {
        Self { agents, claude_dir }
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
    fn name(&self) -> &'static str {
        self.agents.name()
    }

    fn id(&self) -> HarnessId {
        HarnessId::new(ID)
    }

    fn label(&self) -> &'static str {
        LABEL
    }

    fn info(&self) -> HarnessInfo {
        info()
    }

    fn resume_command(&self, session_id: &str) -> String {
        format!("claude --resume {session_id}")
    }

    async fn running(&self) -> Result<Vec<RunningAgent>, Report<HarnessError>> {
        self.agents.list().await
    }

    async fn stop_migrated(&self, short_id: &str) -> Result<(), Report<HarnessError>> {
        self.agents.stop(short_id).await
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use std::sync::{Mutex, PoisonError};

    use async_trait::async_trait;
    use error_stack::Report;

    use super::supervisor::ClaudeAgents;
    use crate::feat::harness::{HarnessError, RunningAgent};

    /// A `claude` that runs the agents the test scripts and logs the
    /// sessions it is asked to stop.
    pub(crate) struct FakeClaude {
        running: Mutex<Result<Vec<RunningAgent>, String>>,
        stopped: Mutex<Vec<String>>,
    }

    impl FakeClaude {
        /// A `claude` running `running`.
        pub(crate) fn running(running: Vec<RunningAgent>) -> Self {
            Self {
                running: Mutex::new(Ok(running)),
                stopped: Mutex::default(),
            }
        }

        /// From now on `list` answers `running`, or fails with the reason.
        pub(crate) fn set_running(&self, running: Result<Vec<RunningAgent>, String>) {
            *self.running.lock().unwrap_or_else(PoisonError::into_inner) = running;
        }

        /// The sessions `stop` was called on, in order.
        pub(crate) fn stopped(&self) -> Vec<String> {
            self.stopped
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    #[async_trait]
    impl ClaudeAgents for FakeClaude {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn list(&self) -> Result<Vec<RunningAgent>, Report<HarnessError>> {
            self.running
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
                .map_err(|reason| Report::new(HarnessError).attach(reason))
        }

        async fn stop(&self, short_id: &str) -> Result<(), Report<HarnessError>> {
            self.stopped
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(short_id.to_owned());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use error_stack::Report;

    use super::ClaudeCode;
    use super::fake::FakeClaude;
    use crate::feat::harness::{Harness, HarnessError};

    #[rstest::rstest]
    #[tokio::test]
    async fn claude_stop_migrated_runs_claude_stop() -> Result<(), Report<HarnessError>> {
        // Given Claude Code over a fake `claude`.
        let agents = Arc::new(FakeClaude::running(Vec::new()));
        let claude = ClaudeCode::new(agents.clone(), PathBuf::from("/nonexistent/claude"));

        // When stopping the migrated session 28bf38e2.
        claude.stop_migrated("28bf38e2").await?;

        // Then `claude stop 28bf38e2` was asked for.
        assert_eq!(
            agents.stopped(),
            ["28bf38e2"],
            "the migrated session should be stopped through claude"
        );
        Ok(())
    }
}
