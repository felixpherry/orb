//! `osascript` as orb's [`Notifier`].
//!
//! Each notification runs one `osascript` whose script reads the title and
//! body from its arguments, so no thread title can change what the script
//! does. It runs with no stdin, stdout or stderr, so it never writes into
//! orb's screen, and a thread of its own waits for it to exit.

use std::ffi::OsString;
use std::process::{Command, Stdio};
use std::thread;

use error_stack::{Report, ResultExt};

use super::notifier::{Notifier, NotifyError};

/// Shows notifications with `osascript … display notification`, which macOS
/// attributes to Script Editor.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsascriptNotifier;

impl Notifier for OsascriptNotifier {
    fn name(&self) -> &'static str {
        "osascript"
    }

    fn notify(&self, title: &str, body: &str) -> Result<(), Report<NotifyError>> {
        let mut child = Command::new("osascript")
            .args(osascript_args(title, body))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .change_context(NotifyError)
            .attach("couldn't run osascript")?;
        thread::Builder::new()
            .name("orb-notify".into())
            .spawn(move || {
                let _ = child.wait();
            })
            .map(drop)
            .change_context(NotifyError)
            .attach("couldn't wait for osascript")
    }
}

/// `osascript`'s arguments to show `title` and `body`: a script that reads
/// them from `argv`, then `--`, so a title starting with `-` isn't taken for
/// an option, then the two texts.
pub fn osascript_args(title: &str, body: &str) -> Vec<OsString> {
    [
        "-e",
        "on run argv",
        "-e",
        "display notification (item 2 of argv) with title (item 1 of argv)",
        "-e",
        "end run",
        "--",
        title,
        body,
    ]
    .into_iter()
    .map(OsString::from)
    .collect()
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::osascript_args;

    #[rstest::rstest]
    fn osascript_args_pass_the_text_as_arguments_after_the_script() {
        // Given a title that would break out of an AppleScript string.
        let title = r#"-orb · "x" & (do shell script "rm -rf ~") & ""#;

        // When building osascript's arguments.
        let args = osascript_args(title, "Finished");

        // Then the script is fixed, and the title and body come last, after `--`.
        let expected: Vec<OsString> = [
            "-e",
            "on run argv",
            "-e",
            "display notification (item 2 of argv) with title (item 1 of argv)",
            "-e",
            "end run",
            "--",
            title,
            "Finished",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert_eq!(args, expected, "osascript's arguments");
    }
}
