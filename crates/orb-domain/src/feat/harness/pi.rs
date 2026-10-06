//! pi, a coding agent orb hosts under zmx: one zmx session per thread, its
//! socket under orb's own zmx directory, status and titles read from pi's own
//! session files, models from `pi --list-models`.

pub mod host;
pub mod models;
pub mod runner;
pub mod session_file;

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use error_stack::Report;

use self::host::ZmxHost;
use self::runner::Runner;
use super::{Harness, HarnessId, HarnessInfo, Scan, TranscriptFormat};
use crate::feat::notify::click::on_path;
use crate::feat::sessions::session_host::{
    AttachStart, CreatedSession, SessionHost, SessionHostError, SessionOptions, SessionRecord,
};
use crate::feat::sessions::transcript::{Exchange, MessageRead};
use crate::feat::zmx::zmx_service::ZmxSession;

/// The id pi threads are stored under.
pub const ID: &str = "pi";

/// The name the user sees for pi, also its sidebar tag.
const LABEL: &str = "pi";

/// pi: sessions under zmx, session files under `sessions_dir`, models from
/// `pi --list-models`.
pub struct Pi {
    host: ZmxHost,
    runner: Arc<dyn Runner>,
    sessions_dir: PathBuf,
    /// The first of `pi` and `zmx` missing from `PATH` when orb started.
    missing: Option<&'static str>,
}

impl Pi {
    /// `path` is the `PATH` orb started with; `socket_dir` is zmx's directory
    /// for orb's sessions (`ZMX_DIR`) and `sessions_dir` is where pi writes
    /// its session files.
    pub fn new(
        runner: Arc<dyn Runner>,
        path: &OsStr,
        socket_dir: PathBuf,
        sessions_dir: PathBuf,
    ) -> Self {
        let missing = ["pi", "zmx"]
            .into_iter()
            .find(|program| on_path(program, path).is_none());
        Self {
            host: ZmxHost::new(runner.clone(), socket_dir, sessions_dir.clone()),
            runner,
            sessions_dir,
            missing,
        }
    }
}

#[async_trait]
impl SessionHost for Pi {
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

    fn zmx_session(&self, short_id: &str) -> Option<ZmxSession> {
        self.host.zmx_session(short_id)
    }
}

impl TranscriptFormat for Pi {
    fn locate(&self, cwd: &Path, session_id: &str) -> Option<PathBuf> {
        session_file::find(&self.sessions_dir, Some(cwd), session_id)
    }

    fn scan(&self, path: &Path, offset: u64, previous: &Scan) -> io::Result<Scan> {
        session_file::scan(path, offset, previous)
    }

    fn exchanges(&self, path: &Path) -> io::Result<(u64, Vec<Exchange>)> {
        session_file::exchanges(path)
    }

    fn messages(&self, path: &Path, offset: u64, prompt_offset: u64) -> io::Result<MessageRead> {
        session_file::messages(path, offset, prompt_offset)
    }
}

#[async_trait]
impl Harness for Pi {
    fn id(&self) -> HarnessId {
        HarnessId::new(ID)
    }

    fn label(&self) -> &'static str {
        LABEL
    }

    /// pi's models when pi and zmx are both on `PATH`; a failed model list
    /// leaves no models and its reason as the notice.
    async fn probe(&self) -> HarnessInfo {
        let (models, notice) = match self.missing {
            Some(_) => (Vec::new(), None),
            None => match models::list(self.runner.as_ref()).await {
                Ok(models) => (models, None),
                Err(report) => (Vec::new(), report.downcast_ref::<String>().cloned()),
            },
        };
        HarnessInfo {
            id: self.id(),
            label: LABEL.to_owned(),
            tag: Some(LABEL.to_owned()),
            icon: Some("π".to_owned()),
            unavailable: self.missing.map(|program| format!("{program} not found")),
            models,
            permission_modes: Vec::new(),
            notice,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::Arc;

    use tempfile::{TempDir, tempdir};

    use super::{Harness, HarnessInfo, Pi};
    use crate::feat::harness::pi::runner::RunOutput;
    use crate::feat::harness::pi::runner::fake::FakeRunner;

    const MODELS: &str = "\
provider       model                    context  max-out  thinking  images
openai-codex   gpt-5.5                  272K     128K     yes       yes
anthropic      claude-haiku-4-5         200K     64K      yes       yes
";

    /// A `PATH` directory holding an executable stub for each of `programs`.
    fn path_with(programs: &[&str]) -> io::Result<TempDir> {
        let dir = tempdir()?;
        for program in programs {
            let stub = dir.path().join(program);
            fs::write(&stub, "")?;
            fs::set_permissions(&stub, fs::Permissions::from_mode(0o755))?;
        }
        Ok(dir)
    }

    /// What pi's probe finds with `programs` on `PATH` and `runner` running
    /// its commands.
    async fn probe(programs: &[&str], runner: FakeRunner) -> io::Result<HarnessInfo> {
        let path = path_with(programs)?;
        let pi = Pi::new(
            Arc::new(runner),
            path.path().as_os_str(),
            Path::new("/s").to_path_buf(),
            Path::new("/p").to_path_buf(),
        );
        Ok(pi.probe().await)
    }

    fn listing(stdout: &str) -> FakeRunner {
        FakeRunner::new(RunOutput {
            code: Some(0),
            stdout: stdout.to_owned(),
            stderr: String::new(),
        })
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn probe_without_pi_reports_pi_not_found() -> io::Result<()> {
        // Given only zmx on PATH.
        // When probing pi.
        let info = probe(&["zmx"], listing(MODELS)).await?;

        // Then pi is unavailable because it's missing.
        assert_eq!(
            info.unavailable.as_deref(),
            Some("pi not found"),
            "a missing pi should be the reason"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn probe_without_zmx_reports_zmx_not_found() -> io::Result<()> {
        // Given only pi on PATH.
        // When probing pi.
        let info = probe(&["pi"], listing(MODELS)).await?;

        // Then pi is unavailable because zmx is missing.
        assert_eq!(
            info.unavailable.as_deref(),
            Some("zmx not found"),
            "a missing zmx should be the reason"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn probe_without_pi_leaves_no_notice() -> io::Result<()> {
        // Given only zmx on PATH.
        // When probing pi.
        let info = probe(&["zmx"], listing(MODELS)).await?;

        // Then there's nothing to tell on the mode line.
        assert_eq!(info.notice, None, "a missing program is no notice");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn probe_lists_pi_models_by_provider() -> io::Result<()> {
        // Given pi and zmx on PATH, and pi listing two providers' models.
        // When probing pi.
        let info = probe(&["pi", "zmx"], listing(MODELS)).await?;

        // Then the models are grouped under their providers.
        let groups: Vec<_> = info
            .models
            .iter()
            .map(|group| {
                (
                    group.heading.as_deref(),
                    group
                        .models
                        .iter()
                        .map(|model| model.id.as_str())
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        assert_eq!(
            groups,
            [
                (Some("openai-codex"), vec!["openai-codex/gpt-5.5"]),
                (Some("anthropic"), vec!["anthropic/claude-haiku-4-5"]),
            ],
            "pi's models should be grouped by provider"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn probe_reports_a_failed_model_list_as_its_notice() -> io::Result<()> {
        // Given pi and zmx on PATH, and pi failing to list its models.
        let runner = FakeRunner::new(RunOutput {
            code: Some(1),
            ..RunOutput::default()
        });

        // When probing pi.
        let info = probe(&["pi", "zmx"], runner).await?;

        // Then the failure is the notice.
        assert_eq!(
            info.notice.as_deref(),
            Some("pi --list-models failed: exit code 1"),
            "a failed model list should be the notice"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn probe_offers_no_permission_modes() -> io::Result<()> {
        // Given pi and zmx on PATH.
        // When probing pi.
        let info = probe(&["pi", "zmx"], listing(MODELS)).await?;

        // Then it has no permission modes to pick.
        assert!(
            info.permission_modes.is_empty(),
            "pi has no permission modes"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn probe_tags_threads_pi() -> io::Result<()> {
        // Given pi and zmx on PATH.
        // When probing pi.
        let info = probe(&["pi", "zmx"], listing(MODELS)).await?;

        // Then its threads are tagged pi.
        assert_eq!(info.tag.as_deref(), Some("pi"), "pi's tag");
        Ok(())
    }
}
