//! zmx as the host of pi sessions.
//!
//! A start only picks the session's id. The host names the zmx session (the
//! session id, on its socket dir) and gives pi's command, and the thread's
//! pane runs `zmx attach <id> pi --session-id <id>` from the two. That creates
//! the session with the pane as its first client, so pi's startup modes reach
//! orb live, and joins it when it already runs, getting zmx's snapshot of the
//! screen and modes. `zmx kill` ends it. A socket that answers `connect` means the session is
//! running; its status then comes from the tail of pi's session file.

use std::ffi::OsString;
use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use error_stack::{Report, ResultExt};
use serde_json::Value;

use super::runner::{RunOutput, Runner};
use super::session_file;
use crate::feat::git::worktree::hex;
use crate::feat::sessions::session_host::{
    AttachStart, CreatedSession, SessionHost, SessionHostError, SessionOptions, SessionRecord,
};
use crate::feat::sessions::state::ThreadStatus;
use crate::feat::sessions::transcript::read_new_lines;
use crate::feat::zmx::zmx_service::{ZmxSession, zmx_argv};

/// How long `zmx kill` gets to run.
const KILL_TIMEOUT: Duration = Duration::from_secs(5);
/// How much of a session file's end the status reads.
const TAIL: u64 = 512 * 1024;

/// Hosts pi sessions under zmx, keeping their sockets in `socket_dir`.
pub struct ZmxHost {
    runner: Arc<dyn Runner>,
    socket_dir: PathBuf,
    sessions_dir: PathBuf,
}

impl ZmxHost {
    /// A host keeping its sockets in `socket_dir` and finding pi's session
    /// files under `sessions_dir`.
    pub fn new(runner: Arc<dyn Runner>, socket_dir: PathBuf, sessions_dir: PathBuf) -> Self {
        Self {
            runner,
            socket_dir,
            sessions_dir,
        }
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

/// Whether a zmx session answers on `socket`. A socket file nobody answers
/// on is left from a zmx that died, so it's removed.
fn answers(socket: &Path) -> bool {
    match UnixStream::connect(socket) {
        Ok(_) => true,
        Err(error) if error.kind() == ErrorKind::ConnectionRefused => {
            let _ = fs::remove_file(socket);
            false
        }
        Err(_) => false,
    }
}

/// When the session's socket was made, which is when its pi last started.
fn started(socket: &Path) -> io::Result<SystemTime> {
    fs::metadata(socket)?.modified()
}

/// Working when the file's last conversation entry is mid-turn (a prompt, a
/// tool result, or a reply asking for a tool) and was written after `since`,
/// when this pi started; Idle otherwise.
fn tail_status(path: &Path, since: SystemTime) -> io::Result<ThreadStatus> {
    let start = fs::metadata(path)?.len().saturating_sub(TAIL);
    let text = read_new_lines(path, start)?.text;
    // A tail that starts mid-file starts mid-line, so its first line is cut.
    let whole = if start > 0 {
        text.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        &text
    };
    let last = whole
        .lines()
        .rev()
        .filter(|line| line.contains(r#""type":"message""#))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|entry| matches!(role(entry), Some("user" | "toolResult" | "assistant")));
    let mid_turn = last.as_ref().is_some_and(|entry| {
        matches!(
            (role(entry), stop_reason(entry)),
            (Some("user" | "toolResult"), _) | (Some("assistant"), Some("toolUse"))
        )
    });
    let newer = {
        let since = jiff::Timestamp::try_from(since).ok();
        last.as_ref()
            .and_then(timestamp)
            .zip(since)
            .is_some_and(|(at, since)| at > since)
    };
    Ok(if mid_turn && newer {
        ThreadStatus::Working
    } else {
        ThreadStatus::Idle
    })
}

/// The role of a message entry's message.
fn role(entry: &Value) -> Option<&str> {
    entry.pointer("/message/role").and_then(Value::as_str)
}

/// Why a message entry's assistant message ended.
fn stop_reason(entry: &Value) -> Option<&str> {
    entry.pointer("/message/stopReason").and_then(Value::as_str)
}

/// When an entry was written.
fn timestamp(entry: &Value) -> Option<jiff::Timestamp> {
    entry.get("timestamp")?.as_str()?.parse().ok()
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

    async fn list(
        &self,
        short_ids: &[String],
    ) -> Result<Vec<SessionRecord>, Report<SessionHostError>> {
        Ok(short_ids
            .iter()
            .map(|id| {
                let socket = self.socket(id);
                let status = match (
                    answers(&socket),
                    session_file::find(&self.sessions_dir, None, id),
                ) {
                    (true, Some(file)) => started(&socket)
                        .and_then(|since| tail_status(&file, since))
                        .unwrap_or(ThreadStatus::Idle),
                    (true, None) => ThreadStatus::Idle,
                    (false, _) => ThreadStatus::Stopped,
                };
                SessionRecord {
                    short_id: id.clone(),
                    session_id: Some(id.clone()),
                    status,
                }
            })
            .collect())
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
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate setup failures with `?` and assert on the outcome"
)]
mod tests {
    use std::error::Error;
    use std::ffi::OsString;
    use std::fs;
    use std::io;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant, SystemTime};

    use error_stack::Report;
    use serde_json::json;
    use tempfile::{TempDir, tempdir, tempdir_in};

    use super::{ZmxHost, tail_status};
    use crate::feat::harness::pi::runner::RunOutput;
    use crate::feat::harness::pi::runner::fake::FakeRunner;
    use crate::feat::sessions::session_host::{
        AttachStart, SessionHost, SessionHostError, SessionOptions,
    };
    use crate::feat::sessions::state::ThreadStatus;
    use crate::feat::zmx::zmx_service::ZmxSession;

    type TestResult = Result<(), Box<dyn Error>>;

    /// Written after any pi a test starts.
    const NEWER: &str = "2099-01-01T00:00:00.000Z";
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

    /// A non-message entry pi writes for its extensions.
    fn custom() -> String {
        json!({"type": "custom", "customType": "plannotator", "data": {}, "timestamp": NEWER})
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

    fn host(runner: &Arc<FakeRunner>, sockets: &Path, sessions: &Path) -> ZmxHost {
        ZmxHost::new(
            runner.clone(),
            sockets.to_path_buf(),
            sessions.to_path_buf(),
        )
    }

    fn reason<T>(result: Result<T, Report<SessionHostError>>) -> Option<String> {
        result.err()?.downcast_ref::<String>().cloned()
    }

    async fn status_of(host: &ZmxHost, id: &str) -> Result<Vec<ThreadStatus>, Box<dyn Error>> {
        Ok(host
            .list(&[id.to_owned()])
            .await?
            .into_iter()
            .map(|record| record.status)
            .collect())
    }

    #[rstest::rstest]
    #[case::assistant_stop(vec![message("assistant", Some("stop"), NEWER)], ThreadStatus::Idle)]
    #[case::assistant_length(vec![message("assistant", Some("length"), NEWER)], ThreadStatus::Idle)]
    #[case::assistant_error(vec![message("assistant", Some("error"), NEWER)], ThreadStatus::Idle)]
    #[case::assistant_aborted(vec![message("assistant", Some("aborted"), NEWER)], ThreadStatus::Idle)]
    #[case::newer_user(vec![message("user", None, NEWER)], ThreadStatus::Working)]
    #[case::newer_tool_result(vec![message("toolResult", None, NEWER)], ThreadStatus::Working)]
    #[case::newer_tool_use(vec![message("assistant", Some("toolUse"), NEWER)], ThreadStatus::Working)]
    #[case::older_user(vec![message("user", None, OLDER)], ThreadStatus::Idle)]
    #[case::older_tool_use(vec![message("assistant", Some("toolUse"), OLDER)], ThreadStatus::Idle)]
    #[case::no_message(vec![header(), custom()], ThreadStatus::Idle)]
    #[case::skipped_roles(
        vec![
            message("user", None, NEWER),
            message("system", None, NEWER),
            message("bashExecution", None, NEWER),
            message("custom", None, NEWER),
        ],
        ThreadStatus::Working
    )]
    fn tail_status_follows_the_last_conversation_entry(
        #[case] tail: Vec<String>,
        #[case] expected: ThreadStatus,
    ) -> io::Result<()> {
        // Given a session file ending in `tail`.
        let dir = tempdir()?;
        let lines: Vec<String> = std::iter::once(header()).chain(tail).collect();
        let path = write_session(dir.path(), &lines)?;

        // When reading its status for a pi started now.
        let status = tail_status(&path, SystemTime::now())?;

        // Then the status follows the last user, toolResult or assistant entry.
        assert_eq!(status, expected, "status for the tail {lines:?}");
        Ok(())
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
    #[tokio::test]
    async fn stale_socket_lists_as_stopped() -> TestResult {
        // Given a socket whose zmx is gone.
        let sockets = socket_dir()?;
        let sessions = tempdir()?;
        stale_socket(&sockets.path().join("orb-a"))?;
        let host = host(
            &Arc::new(FakeRunner::new(ok())),
            sockets.path(),
            sessions.path(),
        );

        // When listing the session.
        let statuses = status_of(&host, "orb-a").await?;

        // Then it's Stopped.
        assert_eq!(
            statuses,
            [ThreadStatus::Stopped],
            "a stale socket is stopped"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn stale_socket_file_is_removed() -> TestResult {
        // Given a socket whose zmx is gone.
        let sockets = socket_dir()?;
        let sessions = tempdir()?;
        let socket = sockets.path().join("orb-a");
        stale_socket(&socket)?;
        let host = host(
            &Arc::new(FakeRunner::new(ok())),
            sockets.path(),
            sessions.path(),
        );

        // When listing the session.
        host.list(&["orb-a".to_owned()]).await?;

        // Then its socket file is gone.
        assert!(!socket.exists(), "the stale socket should be removed");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn no_socket_and_no_file_lists_as_stopped() -> TestResult {
        // Given a session with neither a socket nor a session file.
        let sockets = socket_dir()?;
        let sessions = tempdir()?;
        let host = host(
            &Arc::new(FakeRunner::new(ok())),
            sockets.path(),
            sessions.path(),
        );

        // When listing it.
        let statuses = status_of(&host, "orb-a").await?;

        // Then it's Stopped, so attaching starts it again.
        assert_eq!(statuses, [ThreadStatus::Stopped], "pi is never Gone");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn live_socket_without_a_file_lists_as_idle() -> TestResult {
        // Given a live zmx session whose pi hasn't had a prompt yet.
        let sockets = socket_dir()?;
        let sessions = tempdir()?;
        let _zmx = UnixListener::bind(sockets.path().join("orb-a"))?;
        let host = host(
            &Arc::new(FakeRunner::new(ok())),
            sockets.path(),
            sessions.path(),
        );

        // When listing it.
        let statuses = status_of(&host, "orb-a").await?;

        // Then it's Idle.
        assert_eq!(statuses, [ThreadStatus::Idle], "a fresh pi is idle");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn live_socket_with_a_newer_prompt_lists_as_working() -> TestResult {
        // Given a live zmx session whose file ends in a prompt written after it started.
        let sockets = socket_dir()?;
        let sessions = tempdir()?;
        let _zmx = UnixListener::bind(sockets.path().join("orb-a"))?;
        write_session(sessions.path(), &[header(), message("user", None, NEWER)])?;
        let host = host(
            &Arc::new(FakeRunner::new(ok())),
            sockets.path(),
            sessions.path(),
        );

        // When listing it.
        let statuses = status_of(&host, "orb-a").await?;

        // Then it's Working.
        assert_eq!(statuses, [ThreadStatus::Working], "a new prompt is work");
        Ok(())
    }

    #[rstest::rstest]
    fn attach_argv_starts_pi_on_the_session_id() {
        // Given a host keeping its sockets in /s.
        let runner = Arc::new(FakeRunner::new(ok()));
        let host = host(&runner, Path::new("/s"), Path::new("/p"));

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
        let host = host(&runner, Path::new("/s"), Path::new("/p"));

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
        let host = host(&runner, Path::new("/s"), Path::new("/p"));
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
        let host = host(&runner, Path::new("/s"), Path::new("/p"));
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
        let host = host(&runner, sockets.path(), Path::new("/p"));

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
        let host = host(&runner, sockets.path(), Path::new("/p"));
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
        let host = host(&runner, sockets.path(), Path::new("/p"));

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
        let host = host(&runner, sockets.path(), Path::new("/p"));

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
        let host = host(&runner, sockets.path(), Path::new("/p"));

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
        let host = host(&runner, sockets.path(), Path::new("/p"));

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
        let host = host(&runner, sockets.path(), sessions.path());

        // When removing the session.
        host.remove("orb-a").await?;

        // Then pi's session file is still there.
        assert!(file.exists(), "remove should keep pi's session file");
        Ok(())
    }
}
