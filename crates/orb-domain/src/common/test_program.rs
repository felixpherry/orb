//! Small scripts that tests write and then run.

use std::io;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use super::{Finished, run_within};

/// How long the shell that writes a test program may take.
const WRITE_LIMIT: Duration = Duration::from_secs(5);

/// Writes `script` to `path` as an executable. A shell writes it, so this
/// process never holds the file open for writing: Linux refuses to run a file
/// any process holds open that way, and a test running alongside can fork
/// while this process holds it, keeping a copy of that handle until its child
/// starts.
///
/// # Errors
///
/// Returns an error if the shell can't be run or can't write the file.
pub fn write_program(path: &Path, script: &str) -> io::Result<()> {
    let mut shell = Command::new("/bin/sh");
    shell
        .args(["-c", r#"printf '%s' "$2" > "$1" && chmod 755 "$1""#, "sh"])
        .arg(path)
        .arg(script);
    match run_within(shell, WRITE_LIMIT)? {
        Finished::Exited(output) if output.status.success() => Ok(()),
        Finished::Exited(output) => Err(io::Error::other(format!(
            "couldn't write {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))),
        Finished::TimedOut => Err(io::Error::other(format!(
            "writing {} took over {WRITE_LIMIT:?}",
            path.display()
        ))),
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate setup failures with `?` and assert on the outcome"
)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use tempfile::TempDir;

    use super::write_program;

    #[rstest::rstest]
    fn written_program_holds_the_script_and_runs() -> std::io::Result<()> {
        // Given a script that prints a line with a quote and a dollar sign.
        let dir = TempDir::new()?;
        let path = dir.path().join("it's $prog");
        let script = "#!/bin/sh\necho \"it's \\$1: $1\"\n";

        // When writing it as a program.
        write_program(&path, script)?;

        // Then the file holds the script exactly and anyone can run it.
        assert_eq!(fs::read_to_string(&path)?, script, "the file's contents");
        assert_eq!(
            fs::metadata(&path)?.permissions().mode() & 0o777,
            0o755,
            "the file's mode"
        );
        Ok(())
    }
}
