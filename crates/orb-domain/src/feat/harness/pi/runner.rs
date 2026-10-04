//! The programs the pi harness runs: zmx to end a session, and pi itself to
//! list its models.

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use error_stack::{Report, ResultExt};
use tokio::process::Command;
use tokio::time::timeout;
use wherror::Error;

/// What a finished command left behind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOutput {
    /// The exit code; `None` when a signal ended it.
    pub code: Option<i32>,
    /// What it printed to stdout (lossy UTF-8).
    pub stdout: String,
    /// What it printed to stderr (lossy UTF-8).
    pub stderr: String,
}

impl RunOutput {
    /// The first non-blank line it printed, stdout first.
    pub fn first_line(&self) -> Option<&str> {
        self.stdout
            .lines()
            .chain(self.stderr.lines())
            .map(str::trim)
            .find(|line| !line.is_empty())
    }
}

/// A command couldn't run, or outlived its time limit.
#[derive(Debug, Error)]
#[error(debug)]
pub struct RunError;

/// Runs the commands the pi harness needs.
#[async_trait]
pub trait Runner: Send + Sync {
    fn name(&self) -> &'static str;

    /// Runs `argv` in `cwd`, killed after `limit`.
    ///
    /// # Errors
    ///
    /// Returns an error, with the reason attached as a `String`, if the
    /// command can't start or outlives `limit`.
    async fn run(
        &self,
        argv: &[OsString],
        cwd: &Path,
        limit: Duration,
    ) -> Result<RunOutput, Report<RunError>>;
}

/// Runs commands as child processes with exactly `env` and no stdin.
#[derive(Debug, Clone)]
pub struct ProcessRunner {
    env: Vec<(OsString, OsString)>,
}

impl ProcessRunner {
    /// A runner whose commands run with exactly `env`.
    pub fn new(env: Vec<(OsString, OsString)>) -> Self {
        Self { env }
    }
}

#[async_trait]
impl Runner for ProcessRunner {
    fn name(&self) -> &'static str {
        "process"
    }

    async fn run(
        &self,
        argv: &[OsString],
        cwd: &Path,
        limit: Duration,
    ) -> Result<RunOutput, Report<RunError>> {
        let Some((program, rest)) = argv.split_first() else {
            return Err(Report::new(RunError).attach(String::from("no command to run")));
        };
        let what = program.to_string_lossy().into_owned();
        let mut command = Command::new(program);
        command
            .args(rest)
            .current_dir(cwd)
            .env_clear()
            .envs(self.env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::null())
            .kill_on_drop(true);
        let output = timeout(limit, command.output())
            .await
            .change_context(RunError)
            .attach(format!("{what} timed out after {} s", limit.as_secs_f64()))?
            .map_err(|error| {
                let reason = format!("couldn't run {what}: {error}");
                Report::new(error).change_context(RunError).attach(reason)
            })?;
        Ok(RunOutput {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, PoisonError};
    use std::time::Duration;

    use async_trait::async_trait;
    use error_stack::Report;

    use super::{RunError, RunOutput, Runner};

    /// A runner that records each command and answers every one with the
    /// same output.
    pub(crate) struct FakeRunner {
        output: RunOutput,
        calls: Mutex<Vec<(Vec<OsString>, PathBuf)>>,
    }

    impl FakeRunner {
        /// A runner whose commands all end with `output`.
        pub(crate) fn new(output: RunOutput) -> Self {
            Self {
                output,
                calls: Mutex::default(),
            }
        }

        /// The commands run so far, each with the directory it ran in.
        pub(crate) fn calls(&self) -> Vec<(Vec<OsString>, PathBuf)> {
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    #[async_trait]
    impl Runner for FakeRunner {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn run(
            &self,
            argv: &[OsString],
            cwd: &Path,
            _limit: Duration,
        ) -> Result<RunOutput, Report<RunError>> {
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((argv.to_vec(), cwd.to_path_buf()));
            Ok(self.output.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;
    use std::time::Duration;

    use error_stack::Report;

    use super::{ProcessRunner, RunError, RunOutput, Runner};

    fn argv(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn process_runner_returns_output_and_code() -> Result<(), Report<RunError>> {
        // Given a command that prints a line and exits 3.
        let runner = ProcessRunner::new(Vec::new());
        let command = argv(&["/bin/sh", "-c", "echo hi; exit 3"]);

        // When running it.
        let output = runner
            .run(&command, Path::new("/"), Duration::from_secs(5))
            .await?;

        // Then its exit code and output come back.
        assert_eq!(
            output,
            RunOutput {
                code: Some(3),
                stdout: "hi\n".to_owned(),
                stderr: String::new(),
            },
            "the command's code and output should be returned"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn process_runner_times_out() {
        // Given a command that runs for 5 s.
        let runner = ProcessRunner::new(Vec::new());
        let command = argv(&["/bin/sleep", "5"]);

        // When running it with a 50 ms limit.
        let result = runner
            .run(&command, Path::new("/"), Duration::from_millis(50))
            .await;

        // Then it fails with a reason that names the limit.
        let reason = result
            .err()
            .and_then(|report| report.downcast_ref::<String>().cloned());
        assert_eq!(
            reason.as_deref(),
            Some("/bin/sleep timed out after 0.05 s"),
            "the timeout should be the reason"
        );
    }
}
