//! zmx as the host of pi sessions.
//!
//! A start only picks the session's id. The host names the zmx session (the
//! session id, on its socket dir) and gives pi's command, and the thread's
//! pane runs `zmx attach <id> pi --session-id <id>` from the two. That creates
//! the session with the pane as its first client, so pi's startup modes reach
//! orb live, and joins it when it already runs, getting zmx's snapshot of the
//! screen and modes. `zmx kill` ends it. pi's status comes from the reports
//! orb's extension writes to the pane file, so the host lists nothing.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use error_stack::{Report, ResultExt};

use super::runner::{RunOutput, Runner};
use crate::feat::git::worktree::hex;
use crate::feat::sessions::session_host::{
    AttachStart, CreatedSession, SessionHost, SessionHostError, SessionOptions, SessionRecord,
};
use crate::feat::zmx::zmx_service::{ZmxSession, zmx_argv};

/// How long `zmx kill` gets to run.
const KILL_TIMEOUT: Duration = Duration::from_secs(5);

/// Hosts pi sessions under zmx, keeping their sockets in `socket_dir`.
pub struct ZmxHost {
    runner: Arc<dyn Runner>,
    socket_dir: PathBuf,
}

impl ZmxHost {
    /// A host keeping its sockets in `socket_dir`.
    pub fn new(runner: Arc<dyn Runner>, socket_dir: PathBuf) -> Self {
        Self { runner, socket_dir }
    }

    /// The socket of session `id`; zmx names it after the session.
    fn socket(&self, id: &str) -> PathBuf {
        self.socket_dir.join(id)
    }

    /// `zmx <args>` on the host's socket directory (see [`zmx_argv`]).
    fn zmx(&self, args: &[&str]) -> Vec<OsString> {
        zmx_argv(&self.socket_dir, args.iter().copied())
    }
}

/// A new session id: `orb-` and 32 hex characters.
fn new_id() -> String {
    format!("orb-{}{}{}{}", hex(0), hex(1), hex(2), hex(3))
}

/// An error whose reason is the first line `output` printed, else `fallback`.
fn failure(output: &RunOutput, fallback: &str) -> Report<SessionHostError> {
    let reason = output.first_line().unwrap_or(fallback).to_owned();
    Report::new(SessionHostError).attach(reason)
}

#[async_trait]
impl SessionHost for ZmxHost {
    fn name(&self) -> &'static str {
        super::ID
    }

    async fn create(
        &self,
        _cwd: &Path,
        _options: &SessionOptions,
    ) -> Result<CreatedSession, Report<SessionHostError>> {
        fs::create_dir_all(&self.socket_dir).map_err(|error| {
            let reason = format!("couldn't create {}: {error}", self.socket_dir.display());
            Report::new(error)
                .change_context(SessionHostError)
                .attach(reason)
        })?;
        Ok(CreatedSession { short_id: new_id() })
    }

    /// pi's status comes from its pane reports (see
    /// `Harness::reports_status`), so the host reports nothing.
    async fn list(
        &self,
        _short_ids: &[String],
    ) -> Result<Vec<SessionRecord>, Report<SessionHostError>> {
        Ok(Vec::new())
    }

    async fn stop(&self, short_id: &str) -> Result<(), Report<SessionHostError>> {
        let output = self
            .runner
            .run(&self.zmx(&["kill", short_id]), Path::new("/"), KILL_TIMEOUT)
            .await
            .change_context(SessionHostError)?;
        // zmx exits 1 with SessionNotFound when nothing runs the session.
        match (output.code, output.stderr.contains("SessionNotFound")) {
            (Some(0), _) | (Some(1), true) => {}
            _ => return Err(failure(&output, "zmx kill exited with an error")),
        }
        let _ = fs::remove_file(self.socket(short_id));
        Ok(())
    }

    async fn remove(&self, short_id: &str) -> Result<(), Report<SessionHostError>> {
        self.stop(short_id).await
    }

    fn attach_argv(&self, short_id: &str, start: &AttachStart<'_>) -> Vec<OsString> {
        let mut argv: Vec<OsString> = ["pi", "--session-id", short_id].map(OsString::from).into();
        if let (Some(model), false) = (start.model, start.has_transcript) {
            argv.extend(["--model", model].map(OsString::from));
        }
        argv
    }

    /// pi's session runs in the zmx session named after its id, on the
    /// host's socket directory.
    fn zmx_session(&self, short_id: &str) -> Option<ZmxSession> {
        Some(ZmxSession {
            name: short_id.to_owned(),
            dir: self.socket_dir.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::ffi::OsString;
    use std::fs;
    use std::io;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    use error_stack::Report;
    use serde_json::json;
    use tempfile::{TempDir, tempdir, tempdir_in};

    use super::ZmxHost;
    use crate::feat::harness::pi::runner::RunOutput;
    use crate::feat::harness::pi::runner::fake::FakeRunner;
    use crate::feat::sessions::session_host::{
        AttachStart, SessionHost, SessionHostError, SessionOptions,
    };
    use crate::feat::zmx::zmx_service::ZmxSession;

    type TestResult = Result<(), Box<dyn Error>>;

    /// Written before any pi a test starts.
    const OLDER: &str = "2001-01-01T00:00:00.000Z";

    /// A message entry from `role`, written `at`, ended by `stop` when given.
    fn message(role: &str, stop: Option<&str>, at: &str) -> String {
        let content = json!([{"type": "text", "text": "x"}]);
        let message = match stop {
            Some(stop) => json!({"role": role, "content": content, "stopReason": stop}),
            None => json!({"role": role, "content": content}),
        };
        json!({"type": "message", "timestamp": at, "message": message}).to_string()
    }

    /// pi's first line in every session file.
    fn header() -> String {
        json!({"type": "session", "version": 3, "id": "orb-a", "timestamp": OLDER, "cwd": "/work/a"})
            .to_string()
    }

    /// Writes session `orb-a`'s file into `dir`.
    fn write_session(dir: &Path, lines: &[String]) -> io::Result<PathBuf> {
        let path = dir.join("2026-10-04T01-24-14-425Z_orb-a.jsonl");
        fs::write(&path, format!("{}\n", lines.join("\n")))?;
        Ok(path)
    }

    /// Leaves a socket file at `path` that nobody answers on, as a zmx
    /// that died leaves it. macOS marks a new socket close-on-exec only after
    /// creating it, so a child another test spawns in that moment can hold
    /// the listener open for a while; this waits until connecting is refused.
    fn stale_socket(path: &Path) -> io::Result<()> {
        drop(UnixListener::bind(path)?);
        let deadline = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(path).is_ok() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    /// A socket directory short enough for a socket path under 104 bytes.
    fn socket_dir() -> io::Result<TempDir> {
        tempdir_in("/tmp")
    }

    fn ok() -> RunOutput {
        RunOutput {
            code: Some(0),
            ..RunOutput::default()
        }
    }

    /// What `zmx kill` answers when nothing runs the session.
    fn not_found() -> RunOutput {
        RunOutput {
            code: Some(1),
            stdout: String::new(),
            stderr: "error: failed to kill session=orb-a: SessionNotFound\n".to_owned(),
        }
    }

    fn host(runner: &Arc<FakeRunner>, sockets: &Path) -> ZmxHost {
        ZmxHost::new(runner.clone(), sockets.to_path_buf())
    }

    fn reason<T>(result: Result<T, Report<SessionHostError>>) -> Option<String> {
        result.err()?.downcast_ref::<String>().cloned()
    }

    /// pi's command for orb-a, then `extra`.
    fn attach_of_orb_a(extra: &[&str]) -> Vec<OsString> {
        ["pi", "--session-id", "orb-a"]
            .iter()
            .chain(extra)
            .map(OsString::from)
            .collect()
    }

    #[rstest::rstest]
    fn attach_argv_starts_pi_on_the_session_id() {
        // Given a host keeping its sockets in /s.
        let runner = Arc::new(FakeRunner::new(ok()));
        let host = host(&runner, Path::new("/s"));

        // When building the attach command for orb-a with no model.
        let argv = host.attach_argv("orb-a", &AttachStart::default());

        // Then pi starts on that session id.
        assert_eq!(
            argv,
            attach_of_orb_a(&[]),
            "the pane's command should be pi on the session id"
        );
    }

    #[rstest::rstest]
    fn zmx_session_is_the_session_id_on_the_hosts_socket_dir() {
        // Given a host keeping its sockets in /s.
        let runner = Arc::new(FakeRunner::new(ok()));
        let host = host(&runner, Path::new("/s"));

        // When asking which zmx session orb-a runs in.
        let session = host.zmx_session("orb-a");

        // Then it is orb-a on /s.
        assert_eq!(
            session,
            Some(ZmxSession {
                name: "orb-a".to_owned(),
                dir: PathBuf::from("/s"),
            }),
            "pi's zmx session is its id on the host's socket dir"
        );
    }

    #[rstest::rstest]
    fn attach_argv_passes_the_model_before_a_session_file() {
        // Given a host keeping its sockets in /s, and a thread with a model
        // whose session file isn't known yet.
        let runner = Arc::new(FakeRunner::new(ok()));
        let host = host(&runner, Path::new("/s"));
        let start = AttachStart {
            model: Some("openai-codex/gpt-5.5"),
            has_transcript: false,
        };

        // When building the attach command for orb-a.
        let argv = host.attach_argv("orb-a", &start);

        // Then pi starts on that model.
        assert_eq!(
            argv,
            attach_of_orb_a(&["--model", "openai-codex/gpt-5.5"]),
            "a first start should pass the model"
        );
    }

    #[rstest::rstest]
    fn attach_argv_leaves_out_the_model_once_the_session_file_is_known() {
        // Given a host keeping its sockets in /s, and a thread with a model
        // whose session file is known.
        let runner = Arc::new(FakeRunner::new(ok()));
        let host = host(&runner, Path::new("/s"));
        let start = AttachStart {
            model: Some("openai-codex/gpt-5.5"),
            has_transcript: true,
        };

        // When building the attach command for orb-a.
        let argv = host.attach_argv("orb-a", &start);

        // Then pi resumes without a model, keeping the one the session uses.
        assert_eq!(
            argv,
            attach_of_orb_a(&[]),
            "a resume should leave the model to the session file"
        );
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn create_returns_an_orb_id_of_32_hex() -> TestResult {
        // Given a host.
        let sockets = socket_dir()?;
        let runner = Arc::new(FakeRunner::new(ok()));
        let host = host(&runner, sockets.path());

        // When creating a session.
        let id = host
            .create(Path::new("/work/a"), &SessionOptions::default())
            .await?
            .short_id;

        // Then its id is orb- and 32 lowercase hex characters.
        let hex = id.strip_prefix("orb-").unwrap_or_default();
        assert!(
            hex.len() == 32
                && hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "the id should be orb-<32 hex>, got {id}"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn create_runs_no_command() -> TestResult {
        // Given a host and a model to start pi on.
        let sockets = socket_dir()?;
        let runner = Arc::new(FakeRunner::new(ok()));
        let host = host(&runner, sockets.path());
        let options = SessionOptions {
            model: Some("openai-codex/gpt-5.5".to_owned()),
            permission_mode: None,
        };

        // When creating a session.
        host.create(Path::new("/work/a"), &options).await?;

        // Then nothing ran; the pane's attach starts pi.
        assert!(runner.calls().is_empty(), "create should run no command");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn stop_runs_zmx_kill() -> TestResult {
        // Given a host keeping its sockets in a socket dir.
        let sockets = socket_dir()?;
        let runner = Arc::new(FakeRunner::new(ok()));
        let host = host(&runner, sockets.path());

        // When stopping orb-a.
        host.stop("orb-a").await?;

        // Then zmx kill ran on orb's socket dir, in /.
        let expected: Vec<OsString> = {
            let mut dir = OsString::from("ZMX_DIR=");
            dir.push(sockets.path());
            [
                OsString::from("env"),
                dir,
                "ZMX_NO_DETACH_KEY=1".into(),
                "zmx".into(),
                "kill".into(),
                "orb-a".into(),
            ]
            .into()
        };
        assert_eq!(
            runner.calls(),
            [(expected, PathBuf::from("/"))],
            "stop should run zmx kill"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn stop_of_a_stopped_session_succeeds() -> TestResult {
        // Given zmx kill finding no session.
        let sockets = socket_dir()?;
        let runner = Arc::new(FakeRunner::new(not_found()));
        let host = host(&runner, sockets.path());

        // When stopping the session.
        let result = host.stop("orb-a").await;

        // Then the stop succeeds.
        assert!(result.is_ok(), "nothing running should count as stopped");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn stop_reports_zmx_kills_error() -> TestResult {
        // Given zmx kill failing for another reason.
        let sockets = socket_dir()?;
        let runner = Arc::new(FakeRunner::new(RunOutput {
            code: Some(1),
            stdout: String::new(),
            stderr: "error: failed to kill session=orb-a: ConnectionRefused\n".to_owned(),
        }));
        let host = host(&runner, sockets.path());

        // When stopping the session.
        let result = host.stop("orb-a").await;

        // Then zmx's line is the reason.
        assert_eq!(
            reason(result).as_deref(),
            Some("error: failed to kill session=orb-a: ConnectionRefused"),
            "zmx's first line should be the reason"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn stop_removes_the_socket() -> TestResult {
        // Given a socket left behind once zmx kill has run.
        let sockets = socket_dir()?;
        let socket = sockets.path().join("orb-a");
        stale_socket(&socket)?;
        let runner = Arc::new(FakeRunner::new(ok()));
        let host = host(&runner, sockets.path());

        // When stopping the session.
        host.stop("orb-a").await?;

        // Then its socket file is gone.
        assert!(!socket.exists(), "stop should remove the socket");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn remove_keeps_the_session_file() -> TestResult {
        // Given a stopped session with a session file.
        let sockets = socket_dir()?;
        let sessions = tempdir()?;
        let file = write_session(sessions.path(), &[header(), message("user", None, OLDER)])?;
        let runner = Arc::new(FakeRunner::new(not_found()));
        let host = host(&runner, sockets.path());

        // When removing the session.
        host.remove("orb-a").await?;

        // Then pi's session file is still there.
        assert!(file.exists(), "remove should keep pi's session file");
        Ok(())
    }
}
