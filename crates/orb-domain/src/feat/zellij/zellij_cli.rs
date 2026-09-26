//! The `zellij` command line as orb's [`Zellij`].
//!
//! Every call is a `zellij action` with no stdin that inherits orb's own
//! environment, not orb's child environment: inside zellij that environment
//! carries `ZELLIJ_SESSION_NAME` and `ZELLIJ_PANE_ID`, so each action targets
//! orb's session and client. When zellij fails, the first line it printed to
//! stderr becomes the reason. Against a session that's gone, such as one
//! renamed since orb started, zellij waits forever, so a call that outlives
//! [`TIMEOUT`] is killed and fails.
//!
//! zellij runs a new pane's command under its server's environment, not
//! orb's, so the command goes through `env` to carry orb's own `NO_COLOR`:
//! a `NO_COLOR` the server picked up from the terminal that started it would
//! otherwise turn lazygit monochrome.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use error_stack::Report;
use serde::Deserialize;

use super::zellij_service::{Zellij, ZellijError, ZellijPane};

/// How long one `zellij action` may take.
const TIMEOUT: Duration = Duration::from_secs(2);
/// How often a running `zellij action` is checked for having exited.
const POLL: Duration = Duration::from_millis(10);

/// Runs the `zellij` on `PATH`.
#[derive(Debug, Clone)]
pub struct ZellijCli {
    /// orb's own `NO_COLOR`, which a new pane's command gets in place of the
    /// zellij server's.
    no_color: Option<OsString>,
}

impl ZellijCli {
    pub fn new(no_color: Option<OsString>) -> Self {
        Self { no_color }
    }
}

impl Zellij for ZellijCli {
    fn name(&self) -> &'static str {
        "zellij"
    }

    fn panes(&self) -> Result<Vec<ZellijPane>, Report<ZellijError>> {
        let listed = run(["list-panes", "--json"])?;
        parse_panes(&listed).map_err(|error| {
            Report::new(error)
                .change_context(ZellijError)
                .attach("couldn't read zellij's panes".to_owned())
        })
    }

    fn focus(&self, pane: &ZellijPane) -> Result<(), Report<ZellijError>> {
        run(["go-to-tab-by-id".to_owned(), pane.tab.to_string()])?;
        run(["focus-pane-id".to_owned(), format!("terminal_{}", pane.id)])?;
        Ok(())
    }

    fn open(&self, name: &str, cwd: &Path, argv: &[OsString]) -> Result<(), Report<ZellijError>> {
        run(new_pane_args(name, cwd, self.no_color.as_deref(), argv))?;
        Ok(())
    }
}

/// Runs `zellij action <args>`; returns its stdout.
fn run<I, S>(args: I) -> Result<String, Report<ZellijError>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("zellij");
    command.arg("action").args(args);
    run_within(command, TIMEOUT)
}

/// Runs `command` with no stdin for at most `limit`, killing it if it runs
/// longer; returns its stdout.
fn run_within(mut command: Command, limit: Duration) -> Result<String, Report<ZellijError>> {
    let couldnt_run = |error: std::io::Error| {
        Report::new(error)
            .change_context(ZellijError)
            .attach("couldn't run zellij".to_owned())
    };
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(couldnt_run)?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let reason = "zellij timed out (session renamed? restart orb)".to_owned();
                return Err(Report::new(ZellijError).attach(reason));
            }
            Err(error) => return Err(couldnt_run(error)),
        }
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    if status.success() {
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    } else {
        let reason = String::from_utf8_lossy(&stderr)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map_or_else(|| "zellij failed".to_owned(), str::to_owned);
        Err(Report::new(ZellijError).attach(reason))
    }
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

/// One entry of `zellij action list-panes --json`, only the fields orb reads:
/// an exited pane lacks others, such as `pane_command`.
#[derive(Deserialize)]
struct RawPane {
    id: u32,
    is_plugin: bool,
    title: String,
    tab_id: u64,
}

/// The terminal panes in `list-panes --json`'s output; plugin panes, whose
/// ids overlap the terminals', are left out.
///
/// # Errors
///
/// Returns an error if `json` isn't a list of panes.
pub fn parse_panes(json: &str) -> Result<Vec<ZellijPane>, serde_json::Error> {
    let panes: Vec<RawPane> = serde_json::from_str(json)?;
    Ok(panes
        .into_iter()
        .filter(|pane| !pane.is_plugin)
        .map(|pane| ZellijPane {
            id: pane.id,
            name: pane.title,
            tab: pane.tab_id,
        })
        .collect())
}

/// The `zellij action` arguments that open a floating pane named `name` in
/// `cwd` running `argv`, filling the tab and closing when `argv` exits.
/// `argv` runs with `NO_COLOR` set to `no_color`, or unset without one.
pub fn new_pane_args(
    name: &str,
    cwd: &Path,
    no_color: Option<&OsStr>,
    argv: &[OsString],
) -> Vec<OsString> {
    let no_color = match no_color {
        Some(value) => vec![OsString::from_iter([OsStr::new("NO_COLOR="), value])],
        None => vec!["-u".into(), "NO_COLOR".into()],
    };
    ["new-pane", "--floating", "--close-on-exit", "--name", name]
        .into_iter()
        .map(OsString::from)
        .chain([OsString::from("--cwd"), cwd.as_os_str().to_owned()])
        .chain(
            [
                "-x", "0", "-y", "0", "--width", "100%", "--height", "100%", "--",
            ]
            .map(OsString::from),
        )
        .chain([OsString::from("env")])
        .chain(no_color)
        .chain(argv.iter().cloned())
        .collect()
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate parse failures with `?` and assert on the outcome"
)]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use error_stack::Report;
    use tempfile::TempDir;

    use super::{new_pane_args, parse_panes, run_within};
    use crate::feat::zellij::zellij_service::{ZellijError, ZellijPane, zellij_reason};

    fn pane(id: u32, name: &str, tab: u64) -> ZellijPane {
        ZellijPane {
            id,
            name: name.to_owned(),
            tab,
        }
    }

    #[rstest::rstest]
    fn parse_panes_keeps_only_terminal_panes() -> Result<(), serde_json::Error> {
        // Given list-panes output with a plugin pane, a held pane and a second tab.
        let json = include_str!("fixtures/list-panes.json");

        // When parsing it.
        let panes = parse_panes(json)?;

        // Then every terminal pane is kept, with its id, name and tab, and the plugin is dropped.
        assert_eq!(
            panes,
            vec![
                pane(0, "/t/orb-m8-probe", 0),
                pane(4, "orb:~/dev/orb:lazygit", 0),
                pane(5, "orb:1:held", 0),
                pane(3, "~/d/orb", 1),
            ],
            "terminal panes from every tab"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn parse_panes_rejects_output_that_isnt_json() {
        // Given / When parsing something that isn't JSON.
        let parsed = parse_panes("There is no active session!");

        // Then parsing fails.
        assert!(parsed.is_err(), "non-JSON output should not parse");
    }

    /// The `new-pane` arguments for lazygit in ~/dev/orb, split at `--` into
    /// the pane's options and its command.
    fn lazygit_pane_args(no_color: Option<&str>) -> (Vec<OsString>, Vec<OsString>) {
        let args = new_pane_args(
            "orb:~/dev/orb:lazygit",
            Path::new("/Users/me/dev/orb"),
            no_color.map(OsStr::new),
            &["lazygit".into()],
        );
        let start = args.iter().position(|arg| arg == "--").map_or(0, |i| i + 1);
        let options = args.iter().take(start).cloned().collect();
        let command = args.into_iter().skip(start).collect();
        (options, command)
    }

    fn os_strings(strs: &[&str]) -> Vec<OsString> {
        strs.iter().map(OsString::from).collect()
    }

    #[rstest::rstest]
    fn new_pane_args_open_a_named_full_screen_floating_pane() {
        // Given / When building the arguments for lazygit in ~/dev/orb.
        let (options, _) = lazygit_pane_args(None);

        // Then they float a pane over the whole tab that closes on exit, ending at `--`.
        let expected = os_strings(&[
            "new-pane",
            "--floating",
            "--close-on-exit",
            "--name",
            "orb:~/dev/orb:lazygit",
            "--cwd",
            "/Users/me/dev/orb",
            "-x",
            "0",
            "-y",
            "0",
            "--width",
            "100%",
            "--height",
            "100%",
            "--",
        ]);
        assert_eq!(options, expected, "new-pane options");
    }

    #[rstest::rstest]
    #[case(None, &["env", "-u", "NO_COLOR", "lazygit"])]
    #[case(Some("1"), &["env", "NO_COLOR=1", "lazygit"])]
    fn new_pane_command_runs_with_orbs_own_no_color(
        #[case] no_color: Option<&str>,
        #[case] expected: &[&str],
    ) {
        // Given / When building the arguments for lazygit with orb's NO_COLOR.
        let (_, command) = lazygit_pane_args(no_color);

        // Then the command runs through `env` with that NO_COLOR, whatever the server's is.
        assert_eq!(command, os_strings(expected), "the pane's command");
    }

    /// `program` with `args`, as a stand-in for `zellij action`.
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

        // Then the reason says zellij timed out and how to recover.
        assert_eq!(
            result.err().as_ref().map(zellij_reason),
            Some("zellij timed out (session renamed? restart orb)".to_owned()),
            "a command past its limit should fail as timed out"
        );
    }

    #[rstest::rstest]
    #[case(
        command("sh", &["-c", "echo >&2; echo '  no session  ' >&2; echo more >&2; exit 1"]),
        "no session"
    )]
    #[case(command("sh", &["-c", "exit 1"]), "zellij failed")]
    #[case(command("/nonexistent/zellij", &[]), "couldn't run zellij")]
    fn run_within_gives_the_reason_a_failed_command_left(
        #[case] failing: Command,
        #[case] expected: &str,
    ) {
        // Given a command that fails, with or without saying why, or can't start.
        // When running it.
        let result = run_within(failing, Duration::from_secs(5));

        // Then the reason is stderr's first non-empty line, else a stock one.
        assert_eq!(
            result.err().as_ref().map(zellij_reason),
            Some(expected.to_owned()),
            "the reason for the mode line"
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
    fn run_within_returns_all_of_an_output_longer_than_a_pipe_holds()
    -> Result<(), Report<ZellijError>> {
        // Given a command that prints 1 MB, far more than a pipe holds.
        let head = command("head", &["-c", "1000000", "/dev/zero"]);

        // When running it with a 5 s limit.
        let output = run_within(head, Duration::from_secs(5))?;

        // Then all of its output comes back.
        assert_eq!(output.len(), 1_000_000, "the whole stdout");
        Ok(())
    }
}
