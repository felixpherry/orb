use std::fmt::Debug;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use error_stack::{Report, ResultExt};
use serde::Deserialize;
use wherror::Error;

use crate::common::{Finished, run_within};
use crate::feat::control::protocol::{ApiError, ErrorCode};
use crate::feat::plate::opener::pane_ttys;
use crate::ids::PaneId;

const LOOKUP_LIMIT: Duration = Duration::from_secs(5);

#[derive(Debug, Error)]
#[error(debug)]
pub enum BrowserError {
    NotFound,
    Ambiguous,
    Failed,
}

pub trait BrowserLookup: Send + Sync + Debug {
    fn name(&self) -> &'static str;
    #[expect(
        clippy::missing_errors_doc,
        reason = "errors are named by BrowserError variants"
    )]
    fn key(&self, pane: PaneId) -> Result<String, Report<BrowserError>>;
}

#[derive(Debug)]
pub struct TerminalBrowsers {
    pub program: Option<PathBuf>,
}

impl BrowserLookup for TerminalBrowsers {
    fn name(&self) -> &'static str {
        "terminal-browser"
    }

    fn key(&self, pane: PaneId) -> Result<String, Report<BrowserError>> {
        let program = self.program.as_ref().ok_or_else(|| {
            Report::new(BrowserError::Failed).attach("terminal-browser isn't installed")
        })?;
        let ttys = pane_ttys(pane).change_context(BrowserError::Failed)?;
        let mut command = Command::new(program);
        command.args(["ls", "--all", "--json"]);
        let output = match run_within(command, LOOKUP_LIMIT)
            .change_context(BrowserError::Failed)
            .attach("couldn't run terminal-browser ls")?
        {
            Finished::Exited(output) if output.status.success() => output.stdout,
            Finished::Exited(output) => {
                return Err(Report::new(BrowserError::Failed).attach(format!(
                    "terminal-browser ls exited with {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }
            Finished::TimedOut => {
                return Err(Report::new(BrowserError::Failed)
                    .attach("terminal-browser ls still ran after 5 s"));
            }
        };
        unique_key(&output, &ttys, pane)
    }
}

#[derive(Deserialize)]
struct Listing {
    browsers: Vec<Browser>,
}

#[derive(Deserialize)]
struct Browser {
    key: String,
    tty: Option<String>,
}

fn unique_key(bytes: &[u8], ttys: &[String], pane: PaneId) -> Result<String, Report<BrowserError>> {
    let listing: Listing = serde_json::from_slice(bytes)
        .change_context(BrowserError::Failed)
        .attach("terminal-browser ls gave unreadable JSON")?;
    let mut keys = listing
        .browsers
        .into_iter()
        .filter(|browser| browser.tty.as_ref().is_some_and(|tty| ttys.contains(tty)))
        .map(|browser| browser.key);
    let key = keys.next().ok_or_else(|| {
        Report::new(BrowserError::NotFound)
            .attach(format!("no terminal-browser runs in pane {}", pane.0))
    })?;
    if keys.any(|other| other != key) {
        return Err(Report::new(BrowserError::Ambiguous)
            .attach(format!("several terminal browsers run in pane {}", pane.0)));
    }
    Ok(key)
}

pub(super) fn api_error(report: &Report<BrowserError>) -> ApiError {
    let message = report
        .frames()
        .find_map(|frame| {
            frame.downcast_ref::<String>().cloned().or_else(|| {
                frame
                    .downcast_ref::<&str>()
                    .map(|message| (*message).to_owned())
            })
        })
        .unwrap_or_else(|| format!("{report:?}"));
    match report.current_context() {
        BrowserError::NotFound => ApiError::new(
            ErrorCode::NotFound,
            message,
            "orb tab list and orb pane list show panes; terminal-browser ls --all lists browsers",
        ),
        BrowserError::Ambiguous => ApiError::new(
            ErrorCode::Failed,
            message,
            "terminal-browser ls --all --json lists keys; select one with --browser <key>",
        ),
        BrowserError::Failed => ApiError::failed(message),
    }
}

#[cfg(test)]
mod tests {
    use error_stack::Report;

    use super::{BrowserError, api_error, unique_key};
    use crate::feat::control::protocol::ErrorCode;
    use crate::ids::PaneId;

    const SHARED_DAEMON: &[u8] = br#"{
        "browsers": [
            {"key":"4387-1","pid":4387,"tty":"/dev/ttys001","pane":{"tab":"1","pane":"1"}},
            {"key":"4387-2","pid":4387,"tty":"/dev/ttys002","pane":{"tab":"1","pane":"1"}},
            {"key":"4387-3","pid":4387,"tty":"/dev/ttys003","pane":null}
        ]
    }"#;

    #[rstest::rstest]
    #[case("/dev/ttys001", "4387-1")]
    #[case("/dev/ttys002", "4387-2")]
    #[case("/dev/ttys003", "4387-3")]
    fn shared_daemon_browsers_are_selected_by_terminal(
        #[case] terminal: &str,
        #[case] expected: &str,
    ) {
        let result = unique_key(SHARED_DAEMON, &[terminal.to_owned()], PaneId(2));
        assert_eq!(
            result.ok().as_deref(),
            Some(expected),
            "each browser must match its own terminal, ignoring the daemon and cached ownership"
        );
    }

    #[rstest::rstest]
    fn browser_on_any_of_the_panes_terminals_matches() {
        let result = unique_key(
            SHARED_DAEMON,
            &["/dev/ttys099".to_owned(), "/dev/ttys002".to_owned()],
            PaneId(2),
        );
        assert_eq!(
            result.ok().as_deref(),
            Some("4387-2"),
            "a pane may have several terminals but only one browser"
        );
    }

    #[rstest::rstest]
    fn pane_without_a_browser_is_not_found() {
        let result = unique_key(SHARED_DAEMON, &["/dev/ttys099".to_owned()], PaneId(9));
        assert_eq!(
            result.as_ref().err().map(api_error).map(|error| error.code),
            Some(ErrorCode::NotFound),
            "a pane without a matching browser must fail instead of guessing"
        );
    }

    #[rstest::rstest]
    fn several_browsers_on_one_terminal_are_ambiguous() {
        let bytes = br#"{"browsers":[
            {"key":"4387-1","tty":"/dev/ttys001"},
            {"key":"4387-2","tty":"/dev/ttys001"}
        ]}"#;
        let result = unique_key(bytes, &["/dev/ttys001".to_owned()], PaneId(1));
        assert!(
            matches!(
                result.as_ref().err().map(Report::current_context),
                Some(BrowserError::Ambiguous)
            ),
            "multiple keys on the same terminal must fail instead of picking the first"
        );
    }

    #[rstest::rstest]
    fn several_browsers_on_different_pane_terminals_are_ambiguous() {
        let result = unique_key(
            SHARED_DAEMON,
            &["/dev/ttys001".to_owned(), "/dev/ttys002".to_owned()],
            PaneId(1),
        );
        assert!(
            matches!(
                result.as_ref().err().map(Report::current_context),
                Some(BrowserError::Ambiguous)
            ),
            "multiple browser keys across the pane's terminals must fail"
        );
    }

    #[rstest::rstest]
    fn duplicate_records_for_one_key_are_not_ambiguous() {
        let bytes = br#"{"browsers":[
            {"key":"4387-1","tty":"/dev/ttys001"},
            {"key":"4387-1","tty":"/dev/ttys001"}
        ]}"#;
        let result = unique_key(bytes, &["/dev/ttys001".to_owned()], PaneId(1));
        assert_eq!(
            result.ok().as_deref(),
            Some("4387-1"),
            "ambiguity counts distinct browser keys"
        );
    }

    #[rstest::rstest]
    #[case(b"not json")]
    #[case(br#"{}"#)]
    fn unreadable_browser_listing_is_a_failure(#[case] bytes: &[u8]) {
        let result = unique_key(bytes, &["/dev/ttys001".to_owned()], PaneId(1));
        assert!(
            matches!(
                result.as_ref().err().map(Report::current_context),
                Some(BrowserError::Failed)
            ),
            "a malformed listing must not be mistaken for a pane without a browser"
        );
    }
}
