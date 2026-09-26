//! The `zellij` command line as orb's [`Zellij`].
//!
//! Every call is a `zellij action` with no stdin that inherits orb's own
//! environment, not orb's child environment: inside zellij that environment
//! carries `ZELLIJ_SESSION_NAME` and `ZELLIJ_PANE_ID`, so each action targets
//! orb's session and client. When zellij fails, the first line it printed to
//! stderr becomes the reason.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::{Command, Stdio};

use error_stack::Report;
use serde::Deserialize;

use super::zellij_service::{Zellij, ZellijError, ZellijPane};

/// Runs the `zellij` on `PATH`.
#[derive(Debug, Clone, Copy)]
pub struct ZellijCli;

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
        run(new_pane_args(name, cwd, argv))?;
        Ok(())
    }
}

/// Runs `zellij action <args>`; returns its stdout.
fn run<I, S>(args: I) -> Result<String, Report<ZellijError>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("zellij")
        .arg("action")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            Report::new(error)
                .change_context(ZellijError)
                .attach("couldn't run zellij".to_owned())
        })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let reason = String::from_utf8_lossy(&output.stderr)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map_or_else(|| "zellij failed".to_owned(), str::to_owned);
        Err(Report::new(ZellijError).attach(reason))
    }
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
pub fn new_pane_args(name: &str, cwd: &Path, argv: &[OsString]) -> Vec<OsString> {
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
        .chain(argv.iter().cloned())
        .collect()
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate parse failures with `?` and assert on the outcome"
)]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    use super::{new_pane_args, parse_panes};
    use crate::feat::zellij::zellij_service::ZellijPane;

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

    #[rstest::rstest]
    fn new_pane_args_open_a_named_full_screen_floating_pane_running_the_command() {
        // Given / When building the arguments for lazygit in ~/dev/orb.
        let args = new_pane_args(
            "orb:~/dev/orb:lazygit",
            Path::new("/Users/me/dev/orb"),
            &["lazygit".into()],
        );

        // Then they float a pane over the whole tab that closes on exit, with the command after `--`.
        let expected: Vec<OsString> = [
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
            "lazygit",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        assert_eq!(args, expected, "new-pane arguments");
    }
}
