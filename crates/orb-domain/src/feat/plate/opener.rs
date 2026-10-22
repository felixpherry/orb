//! Opening a plate link outside orb's panes: as a new tab of the
//! terminal-browser in an orb pane, or with the desktop's own opener.
//!
//! The browser in a pane is found by its terminal: orb reads which terminals
//! the pane's processes run on, then asks terminal-browser which of its
//! browsers runs on one of them, and opens the tab in that browser by key.

use std::fmt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use error_stack::{Report, ResultExt};
use serde::Deserialize;
use wherror::Error;

use crate::common::{Finished, run_within, wait_within};
use crate::feat::control::browser_socket::pane_in;
use crate::ids::PaneId;

/// How long each outside program a link runs may take.
const LINK_LIMIT: Duration = Duration::from_secs(5);

/// A link couldn't be opened. Every report carries a one-line reason.
#[derive(Debug, Error)]
#[error(debug)]
pub struct LinkError;

/// Opens links.
pub trait LinkOpener: Send + Sync {
    fn name(&self) -> &'static str;

    /// Opens `url` as a new tab of the terminal-browser running in orb pane `pane`.
    ///
    /// # Errors
    ///
    /// Returns an error if no terminal-browser runs in `pane` or it didn't take the tab.
    fn new_tab(&self, url: &str, pane: PaneId) -> Result<(), Report<LinkError>>;

    /// Opens `address`, a URL or a file, with the desktop's opener.
    ///
    /// # Errors
    ///
    /// Returns an error if there's no opener or it failed.
    fn system_open(&self, address: &str) -> Result<(), Report<LinkError>>;
}

/// Opens links with the programs found at start-up: terminal-browser for a
/// pane's browser, and the desktop's opener (`open` on macOS, `xdg-open` on
/// Linux) for the rest. Either is `None` when it isn't installed.
#[derive(Debug)]
pub struct DesktopLinks {
    /// The `terminal-browser` program.
    pub terminal_browser: Option<PathBuf>,
    /// The desktop's opener.
    pub system: Option<PathBuf>,
}

impl LinkOpener for DesktopLinks {
    fn name(&self) -> &'static str {
        "desktop"
    }

    fn new_tab(&self, url: &str, pane: PaneId) -> Result<(), Report<LinkError>> {
        let Some(browser) = &self.terminal_browser else {
            return Err(Report::new(LinkError).attach("terminal-browser isn't installed"));
        };
        let ttys = pane_ttys(pane)?;
        let listing = {
            let mut command = Command::new(browser);
            command.args(["ls", "--all", "--json"]);
            run(command, "terminal-browser ls")?
        };
        let key = browser_on(&String::from_utf8_lossy(&listing), &ttys)
            .change_context(LinkError)
            .attach("terminal-browser ls gave unreadable JSON")?
            .ok_or_else(|| {
                Report::new(LinkError)
                    .attach(format!("no terminal-browser runs in pane {}", pane.0))
            })?;
        let mut command = Command::new(browser);
        command.args(["new-tab", "--browser", &key, url]);
        run(command, "new-tab").map(drop)
    }

    fn system_open(&self, address: &str) -> Result<(), Report<LinkError>> {
        let Some(program) = &self.system else {
            return Err(Report::new(LinkError).attach("nothing to open links with"));
        };
        // The opener may leave the app it starts holding its output, so it
        // gets none and only its exit is waited on.
        let mut child = Command::new(program)
            .arg(address)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .change_context(LinkError)
            .attach_with(|| format!("couldn't run {}", program.display()))?;
        match wait_within(&mut child, LINK_LIMIT) {
            Ok(Some(status)) if status.success() => Ok(()),
            Ok(Some(status)) => Err(Report::new(LinkError)
                .attach(format!("{} exited with {status}", program.display()))),
            Ok(None) => Err(Report::new(LinkError).attach(format!(
                "{} still ran after {} s",
                program.display(),
                LINK_LIMIT.as_secs()
            ))),
            Err(error) => Err(Report::new(error)
                .change_context(LinkError)
                .attach(format!("couldn't wait for {}", program.display()))),
        }
    }
}

/// Runs `command` within [`LINK_LIMIT`] and returns its output, or an error
/// naming `what` and why it failed.
fn run(command: Command, what: &str) -> Result<Vec<u8>, Report<LinkError>> {
    match run_within(command, LINK_LIMIT) {
        Ok(Finished::Exited(out)) if out.status.success() => Ok(out.stdout),
        Ok(Finished::Exited(out)) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let reason = stderr.lines().next().unwrap_or_default().trim().to_owned();
            Err(Report::new(LinkError).attach(match reason.as_str() {
                "" => format!("{what} exited with {}", out.status),
                _ => format!("{what} exited with {}: {reason}", out.status),
            }))
        }
        Ok(Finished::TimedOut) => Err(Report::new(LinkError)
            .attach(format!("{what} still ran after {} s", LINK_LIMIT.as_secs()))),
        Err(error) => Err(Report::new(error)
            .change_context(LinkError)
            .attach(format!("couldn't run {what}"))),
    }
}

/// The terminals the processes of orb pane `pane` run on, from each process's
/// start-up environment: `ps -E` on macOS, `/proc/<pid>/environ` on Linux.
pub(crate) fn pane_ttys(pane: PaneId) -> Result<Vec<String>, Report<LinkError>> {
    let text = if cfg!(target_os = "linux") {
        let mut command = Command::new("ps");
        command.args(["-e", "-o", "pid=,tty="]);
        let listing = run(command, "ps")?;
        String::from_utf8_lossy(&listing)
            .lines()
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                let (pid, tty) = (words.next()?, words.next()?);
                let env = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
                let env = String::from_utf8_lossy(&env).replace('\n', "\0");
                Some(format!("{tty} {env}\n"))
            })
            .collect()
    } else {
        let mut command = Command::new("ps");
        command.args(["-A", "-E", "-o", "tty=,command="]);
        String::from_utf8_lossy(&run(command, "ps")?).into_owned()
    };
    Ok(ttys_in(&text, pane))
}

/// The terminals in `text`, one process a line as its terminal then its
/// command and environment, whose process runs in orb pane `pane`. Each comes
/// once, as a `/dev/` path; processes without a terminal are skipped.
pub(crate) fn ttys_in(text: &str, pane: PaneId) -> Vec<String> {
    let mut ttys: Vec<String> = Vec::new();
    for line in text.lines() {
        let Some((tty, rest)) = line.trim_start().split_once(char::is_whitespace) else {
            continue;
        };
        if matches!(tty, "??" | "?" | "-") || pane_in(rest) != Some(pane) {
            continue;
        }
        let tty = match tty.strip_prefix("/dev/") {
            Some(_) => tty.to_owned(),
            None => format!("/dev/{tty}"),
        };
        if !ttys.contains(&tty) {
            ttys.push(tty);
        }
    }
    ttys
}

/// What `terminal-browser ls --all --json` prints, as far as orb reads it.
#[derive(Debug, Deserialize)]
struct Listing {
    /// Every running browser.
    #[serde(default)]
    browsers: Vec<Listed>,
}

/// One running browser.
#[derive(Debug, Deserialize)]
struct Listed {
    /// What `--browser` takes to name it.
    key: String,
    /// The terminal it draws on, when it has one.
    #[serde(default)]
    tty: Option<String>,
}

/// The key of the first browser in `json`, a `terminal-browser ls --all
/// --json` listing, that runs on one of `ttys`; `None` when none does.
///
/// # Errors
///
/// Returns an error if `json` isn't such a listing.
pub(crate) fn browser_on(json: &str, ttys: &[String]) -> Result<Option<String>, serde_json::Error> {
    let listing: Listing = serde_json::from_str(json)?;
    Ok(listing
        .browsers
        .into_iter()
        .find(|browser| browser.tty.as_ref().is_some_and(|tty| ttys.contains(tty)))
        .map(|browser| browser.key))
}

/// Opens nothing: every link is refused.
#[derive(Debug)]
pub struct NoLinks;

impl LinkOpener for NoLinks {
    fn name(&self) -> &'static str {
        "none"
    }

    fn new_tab(&self, _url: &str, _pane: PaneId) -> Result<(), Report<LinkError>> {
        Err(Report::new(LinkError).attach("links can't open here"))
    }

    fn system_open(&self, _address: &str) -> Result<(), Report<LinkError>> {
        Err(Report::new(LinkError).attach("links can't open here"))
    }
}

/// Shared handle to the [`LinkOpener`] in use.
#[derive(Clone)]
pub struct LinkService {
    opener: Arc<dyn LinkOpener>,
}

impl LinkService {
    pub fn new(opener: Arc<dyn LinkOpener>) -> Self {
        Self { opener }
    }

    /// A service that opens nothing.
    pub fn none() -> Self {
        Self::new(Arc::new(NoLinks))
    }

    /// Opens `url` as a new tab of the terminal-browser running in orb pane `pane`.
    ///
    /// # Errors
    ///
    /// Returns an error if no terminal-browser runs in `pane` or it didn't take the tab.
    pub fn new_tab(&self, url: &str, pane: PaneId) -> Result<(), Report<LinkError>> {
        self.opener.new_tab(url, pane)
    }

    /// Opens `address`, a URL or a file, with the desktop's opener.
    ///
    /// # Errors
    ///
    /// Returns an error if there's no opener or it failed.
    pub fn system_open(&self, address: &str) -> Result<(), Report<LinkError>> {
        self.opener.system_open(address)
    }
}

impl fmt::Debug for LinkService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Links<{}>", self.opener.name())
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use std::sync::{Mutex, PoisonError};

    use error_stack::Report;

    use super::{LinkError, LinkOpener};
    use crate::ids::PaneId;

    /// One call a [`FakeLinks`] was asked.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum LinkCall {
        /// `new_tab(url, pane)`.
        NewTab { url: String, pane: PaneId },
        /// `system_open(address)`.
        System(String),
    }

    /// An opener that records each call and answers every one the same way.
    #[derive(Debug, Default)]
    pub(crate) struct FakeLinks {
        fails: bool,
        calls: Mutex<Vec<LinkCall>>,
    }

    impl FakeLinks {
        /// An opener whose every call fails.
        pub(crate) fn failing() -> Self {
            Self {
                fails: true,
                calls: Mutex::default(),
            }
        }

        /// The calls made so far, in order.
        pub(crate) fn calls(&self) -> Vec<LinkCall> {
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn answer(&self, call: LinkCall) -> Result<(), Report<LinkError>> {
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(call);
            if self.fails {
                Err(Report::new(LinkError).attach("the fake opener fails"))
            } else {
                Ok(())
            }
        }
    }

    impl LinkOpener for FakeLinks {
        fn name(&self) -> &'static str {
            "fake"
        }

        fn new_tab(&self, url: &str, pane: PaneId) -> Result<(), Report<LinkError>> {
            self.answer(LinkCall::NewTab {
                url: url.to_owned(),
                pane,
            })
        }

        fn system_open(&self, address: &str) -> Result<(), Report<LinkError>> {
            self.answer(LinkCall::System(address.to_owned()))
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate failures with `?` and assert on the outcome"
)]
mod tests {
    use super::{browser_on, ttys_in};
    use crate::ids::PaneId;

    /// `ps -A -E -o tty=,command=` with pane 1933's zmx client and browser on
    /// two terminals, and pane 1934's shell on a third.
    const PS: &str = "\
ttys018  zmx attach orb-p1933 HOME=/h ORB_PANE_ID=1933
ttys019  terminal-browser open https://x.test HOME=/h ORB_PANE_ID=1933
ttys020  -fish HOME=/h ORB_PANE_ID=1934
ttys019  node browser.js HOME=/h ORB_PANE_ID=1933
";

    /// `terminal-browser ls --all --json` with a browser on `ttys007` and one
    /// on `ttys019`.
    const LS: &str = r#"{
  "self": null,
  "browsers": [
    {"key": "14553-7", "pid": 14553, "tty": "/dev/ttys007", "pane": {"tab": "277", "pane": "1837"}},
    {"key": "14553-9", "pid": 14553, "tty": "/dev/ttys019", "pane": {"tab": "277", "pane": "1837"}}
  ]
}"#;

    #[rstest::rstest]
    fn ttys_in_finds_the_ttys_of_a_panes_processes() {
        // Given a process list with pane 1933 on ttys018 and ttys019, and
        // pane 1934 on ttys020.
        // When finding pane 1933's terminals.
        let ttys = ttys_in(PS, PaneId(1933));

        // Then they are its two, once each, as /dev/ paths.
        assert_eq!(
            ttys,
            ["/dev/ttys018", "/dev/ttys019"],
            "pane 1933's processes run on ttys018 and ttys019"
        );
    }

    #[rstest::rstest]
    fn ttys_in_skips_processes_without_a_tty() {
        // Given a pane whose only process has no terminal.
        let text = "??  zmx serve orb-p5 ORB_PANE_ID=5\n";

        // When finding its terminals.
        let ttys = ttys_in(text, PaneId(5));

        // Then there are none.
        assert!(
            ttys.is_empty(),
            "a process without a terminal gives none: {ttys:?}"
        );
    }

    #[rstest::rstest]
    fn browser_on_picks_the_browser_on_the_panes_tty() -> Result<(), serde_json::Error> {
        // Given two browsers, one on the pane's terminal ttys019.
        let ttys = ["/dev/ttys018".to_owned(), "/dev/ttys019".to_owned()];

        // When finding the browser on the pane.
        let key = browser_on(LS, &ttys)?;

        // Then it is the one on ttys019.
        assert_eq!(
            key.as_deref(),
            Some("14553-9"),
            "the browser drawing on the pane's terminal"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn browser_on_finds_none_when_no_browser_runs_there() -> Result<(), serde_json::Error> {
        // Given two browsers on other terminals.
        let ttys = ["/dev/ttys030".to_owned()];

        // When finding the browser on the pane.
        let key = browser_on(LS, &ttys)?;

        // Then there is none.
        assert_eq!(key, None, "no browser draws on the pane's terminal");
        Ok(())
    }
}
