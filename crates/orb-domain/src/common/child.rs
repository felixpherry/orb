//! Running a child process for at most a time limit, killed past it, so a
//! command that hangs can't hang orb.

use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How often a running child is checked for having exited.
const POLL: Duration = Duration::from_millis(10);

/// How a child run by [`run_within`] ended.
#[derive(Debug)]
pub enum Finished {
    /// It exited within the limit, with this status and output.
    Exited(Output),
    /// It ran past the limit and was killed.
    TimedOut,
}

/// Runs `command` with no stdin and piped stdout/stderr for at most `limit`,
/// killing (and reaping) it if it runs longer.
///
/// # Errors
///
/// Returns an error if the command can't be started or waited on.
pub fn run_within(mut command: Command, limit: Duration) -> std::io::Result<Finished> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if Instant::now() < deadline => thread::sleep(POLL),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(Finished::TimedOut);
            }
        }
    };
    Ok(Finished::Exited(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    }))
}

/// Reads all of `pipe` on a thread of its own, so a child that prints more
/// than a pipe holds isn't stalled waiting for orb to read it.
fn drain<R>(pipe: Option<R>) -> JoinHandle<Vec<u8>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    })
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate setup failures with `?` and assert on the outcome"
)]
mod tests {
    use std::fs;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use tempfile::TempDir;

    use super::{Finished, run_within};

    /// `program` with `args`.
    fn command(program: &str, args: &[&str]) -> Command {
        let mut command = Command::new(program);
        command.args(args);
        command
    }

    #[rstest::rstest]
    fn run_within_says_a_command_that_outlives_the_limit_timed_out() {
        // Given a command that runs longer than the limit.
        let sleep = command("sleep", &["10"]);

        // When running it with a 100 ms limit.
        let result = run_within(sleep, Duration::from_millis(100));

        // Then it timed out.
        assert!(
            matches!(result, Ok(Finished::TimedOut)),
            "a command past its limit should time out"
        );
    }

    #[rstest::rstest]
    fn run_within_kills_a_command_that_outlives_the_limit() -> std::io::Result<()> {
        // Given a command that writes its pid to a file, then runs longer than the limit.
        let dir = TempDir::new()?;
        let pid_file = dir.path().join("pid");
        let sleep = command(
            "sh",
            &[
                "-c",
                r#"echo $$ > "$0"; exec sleep 10"#,
                &pid_file.to_string_lossy(),
            ],
        );

        // When running it with a 500 ms limit.
        let _timed_out = run_within(sleep, Duration::from_millis(500));

        // Then that process is gone.
        let pid = fs::read_to_string(&pid_file)?;
        let alive = Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()?
            .success();
        assert!(!alive, "a command past its limit should be killed");
        Ok(())
    }

    #[rstest::rstest]
    fn run_within_returns_all_of_an_output_longer_than_a_pipe_holds() -> std::io::Result<()> {
        // Given a command that prints 1 MB, far more than a pipe holds.
        let head = command("head", &["-c", "1000000", "/dev/zero"]);

        // When running it with a 5 s limit.
        let stdout_len = match run_within(head, Duration::from_secs(5))? {
            Finished::Exited(output) => Some(output.stdout.len()),
            Finished::TimedOut => None,
        };

        // Then all of its output comes back.
        assert_eq!(stdout_len, Some(1_000_000), "the whole stdout");
        Ok(())
    }
}
