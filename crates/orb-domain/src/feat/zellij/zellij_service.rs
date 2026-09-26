//! What orb asks of zellij: open a tool in a directory as its own pane, or
//! focus that pane if it's already open.
//!
//! A tool's pane is found again by its name, `orb:<directory>:<tool>`, so
//! every thread in one checkout shares one pane per tool, and orb keeps no
//! pane ids.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use error_stack::Report;
use wherror::Error;

use crate::tilde;

/// A zellij call failed. Every report carries a one-line reason as its latest
/// `String` attachment, fit for the mode line.
#[derive(Debug, Error)]
#[error(debug)]
pub struct ZellijError;

/// What the mode line says when orb isn't running inside zellij.
pub const NOT_IN_ZELLIJ: &str = "Not running inside zellij";

/// The one-line reason a zellij failure carries.
pub fn zellij_reason(report: &Report<ZellijError>) -> String {
    report
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| "zellij failed".to_owned())
}

/// A program orb opens in a thread's directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Shell,
    Lazygit,
    Nvim,
}

impl Tool {
    /// The which-key label, and the last part of the pane's name.
    pub fn label(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Lazygit => "lazygit",
            Self::Nvim => "nvim",
        }
    }

    /// The command the pane runs: the user's `shell`, `lazygit`, or `nvim .`.
    pub fn argv(self, shell: &OsStr) -> Vec<OsString> {
        match self {
            Self::Shell => vec![shell.to_owned()],
            Self::Lazygit => vec!["lazygit".into()],
            Self::Nvim => vec!["nvim".into(), ".".into()],
        }
    }
}

/// A terminal pane in orb's zellij session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZellijPane {
    /// The terminal id; zellij addresses it as `terminal_<id>`.
    pub id: u32,
    /// Its name if set, else the title its program set.
    pub name: String,
    /// Its tab's stable id.
    pub tab: u64,
}

/// Runs zellij actions against the session orb runs in.
pub trait Zellij: Send + Sync {
    fn name(&self) -> &'static str;

    /// Every terminal pane in the session, on every tab; plugin panes are
    /// left out.
    ///
    /// # Errors
    ///
    /// Returns an error if zellij can't list them.
    fn panes(&self) -> Result<Vec<ZellijPane>, Report<ZellijError>>;

    /// Switches to the pane's tab and focuses it; a hidden floating pane
    /// shows again.
    ///
    /// # Errors
    ///
    /// Returns an error if zellij refuses.
    fn focus(&self, pane: &ZellijPane) -> Result<(), Report<ZellijError>>;

    /// Opens a full-screen floating pane named `name` in `cwd` running
    /// `argv`, closed when `argv` exits.
    ///
    /// # Errors
    ///
    /// Returns an error if zellij refuses.
    fn open(&self, name: &str, cwd: &Path, argv: &[OsString]) -> Result<(), Report<ZellijError>>;
}

/// The name of `tool`'s pane for `cwd`: `orb:<cwd, with home as ~>:<tool>`,
/// e.g. `orb:~/dev/orb:lazygit`.
pub fn pane_name(tool: Tool, cwd: &Path, home: &Path) -> String {
    format!("orb:{}:{}", tilde(cwd, home), tool.label())
}

/// Shared handle to the [`Zellij`] in use, with the user's shell and home.
#[derive(Clone)]
pub struct ZellijService {
    zellij: Arc<dyn Zellij>,
    /// What [`Tool::Shell`] runs.
    shell: OsString,
    /// Shown as `~` in pane names and reasons.
    home: PathBuf,
}

impl ZellijService {
    pub fn new(zellij: Arc<dyn Zellij>, shell: OsString, home: PathBuf) -> Self {
        Self {
            zellij,
            shell,
            home,
        }
    }

    /// Focuses `tool`'s pane for `cwd`, else opens it.
    ///
    /// # Errors
    ///
    /// Returns an error if `cwd` isn't a directory, before any zellij call,
    /// or if a zellij call fails.
    pub fn open_tool(&self, tool: Tool, cwd: &Path) -> Result<(), Report<ZellijError>> {
        if !cwd.is_dir() {
            let reason = format!("{} doesn't exist", tilde(cwd, &self.home));
            return Err(Report::new(ZellijError).attach(reason));
        }
        let name = pane_name(tool, cwd, &self.home);
        let panes = self.zellij.panes()?;
        match panes.iter().find(|pane| pane.name == name) {
            Some(pane) => self.zellij.focus(pane),
            None => self.zellij.open(&name, cwd, &tool.argv(&self.shell)),
        }
    }
}

impl fmt::Debug for ZellijService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Zellij<{}>", self.zellij.name())
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate setup failures with `?` and assert on the outcome"
)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, PoisonError};

    use error_stack::{Report, ResultExt};
    use tempfile::TempDir;

    use super::{Tool, Zellij, ZellijError, ZellijPane, ZellijService, pane_name, zellij_reason};

    /// A zellij call the fake saw.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Panes,
        Focus(ZellijPane),
        Open {
            name: String,
            cwd: PathBuf,
            argv: Vec<OsString>,
        },
    }

    /// A zellij session holding `panes` that records every call.
    struct FakeZellij {
        panes: Vec<ZellijPane>,
        calls: Mutex<Vec<Call>>,
    }

    impl FakeZellij {
        fn holding(panes: Vec<ZellijPane>) -> Arc<Self> {
            Arc::new(Self {
                panes,
                calls: Mutex::default(),
            })
        }

        fn calls(&self) -> Vec<Call> {
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        /// The calls that focus or open a pane.
        fn changes(&self) -> Vec<Call> {
            self.calls()
                .into_iter()
                .filter(|call| *call != Call::Panes)
                .collect()
        }

        fn record(&self, call: Call) {
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(call);
        }
    }

    impl Zellij for FakeZellij {
        fn name(&self) -> &'static str {
            "fake"
        }

        fn panes(&self) -> Result<Vec<ZellijPane>, Report<ZellijError>> {
            self.record(Call::Panes);
            Ok(self.panes.clone())
        }

        fn focus(&self, pane: &ZellijPane) -> Result<(), Report<ZellijError>> {
            self.record(Call::Focus(pane.clone()));
            Ok(())
        }

        fn open(
            &self,
            name: &str,
            cwd: &Path,
            argv: &[OsString],
        ) -> Result<(), Report<ZellijError>> {
            self.record(Call::Open {
                name: name.to_owned(),
                cwd: cwd.to_owned(),
                argv: argv.to_vec(),
            });
            Ok(())
        }
    }

    /// A home directory holding the checkout `orb`.
    fn home_with_checkout() -> Result<(TempDir, PathBuf), Report<ZellijError>> {
        let home = TempDir::new().change_context(ZellijError)?;
        let checkout = home.path().join("orb");
        fs::create_dir_all(&checkout).change_context(ZellijError)?;
        Ok((home, checkout))
    }

    fn service(zellij: &Arc<FakeZellij>, home: &Path) -> ZellijService {
        ZellijService::new(zellij.clone(), "fish".into(), home.to_owned())
    }

    #[rstest::rstest]
    fn open_tool_opens_a_pane_when_none_has_its_name() -> Result<(), Report<ZellijError>> {
        // Given a session with no panes, and a checkout.
        let (home, checkout) = home_with_checkout()?;
        let zellij = FakeZellij::holding(Vec::new());

        // When opening lazygit in the checkout.
        service(&zellij, home.path()).open_tool(Tool::Lazygit, &checkout)?;

        // Then zellij opens one pane, named for the checkout and tool, running lazygit there.
        assert_eq!(
            zellij.changes(),
            vec![Call::Open {
                name: "orb:~/orb:lazygit".to_owned(),
                cwd: checkout,
                argv: vec!["lazygit".into()],
            }],
            "a missing pane should be opened once"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn open_tool_focuses_the_pane_with_its_name() -> Result<(), Report<ZellijError>> {
        // Given a session with the checkout's lazygit pane on another tab.
        let (home, checkout) = home_with_checkout()?;
        let pane = ZellijPane {
            id: 4,
            name: pane_name(Tool::Lazygit, &checkout, home.path()),
            tab: 1,
        };
        let zellij = FakeZellij::holding(vec![pane.clone()]);

        // When opening lazygit in the checkout.
        service(&zellij, home.path()).open_tool(Tool::Lazygit, &checkout)?;

        // Then zellij focuses that pane and opens nothing.
        assert_eq!(
            zellij.changes(),
            vec![Call::Focus(pane)],
            "an open pane should be focused, not opened again"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn open_tool_opens_a_pane_when_only_another_tools_is_open() -> Result<(), Report<ZellijError>> {
        // Given a session with the checkout's shell pane.
        let (home, checkout) = home_with_checkout()?;
        let zellij = FakeZellij::holding(vec![ZellijPane {
            id: 2,
            name: pane_name(Tool::Shell, &checkout, home.path()),
            tab: 0,
        }]);

        // When opening nvim in the checkout.
        service(&zellij, home.path()).open_tool(Tool::Nvim, &checkout)?;

        // Then zellij opens nvim's own pane.
        assert_eq!(
            zellij.changes(),
            vec![Call::Open {
                name: "orb:~/orb:nvim".to_owned(),
                cwd: checkout,
                argv: vec!["nvim".into(), ".".into()],
            }],
            "another tool's pane should not stand in for this tool's"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn open_tool_says_a_missing_directory_doesnt_exist() -> Result<(), Report<ZellijError>> {
        // Given a directory under home that isn't on disk.
        let home = TempDir::new().change_context(ZellijError)?;
        let zellij = FakeZellij::holding(Vec::new());

        // When opening lazygit there.
        let result =
            service(&zellij, home.path()).open_tool(Tool::Lazygit, &home.path().join("gone"));

        // Then the reason names the directory from home.
        assert_eq!(
            result.err().as_ref().map(zellij_reason),
            Some("~/gone doesn't exist".to_owned()),
            "a missing directory should be refused by its ~ path"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn open_tool_for_a_missing_directory_calls_no_zellij() -> Result<(), Report<ZellijError>> {
        // Given a directory under home that isn't on disk.
        let home = TempDir::new().change_context(ZellijError)?;
        let zellij = FakeZellij::holding(Vec::new());

        // When opening lazygit there.
        let _refused =
            service(&zellij, home.path()).open_tool(Tool::Lazygit, &home.path().join("gone"));

        // Then zellij was never called.
        assert!(
            zellij.calls().is_empty(),
            "a missing directory should not reach zellij"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case("/Users/me/dev/orb", "orb:~/dev/orb:lazygit")]
    #[case("/tmp/orb", "orb:/tmp/orb:lazygit")]
    fn pane_name_shows_the_directory_from_home(#[case] cwd: &str, #[case] expected: &str) {
        // Given / When / Then: lazygit's pane for `cwd` is named `expected`.
        assert_eq!(
            pane_name(Tool::Lazygit, Path::new(cwd), Path::new("/Users/me")),
            expected,
            "pane name for {cwd}"
        );
    }

    #[rstest::rstest]
    #[case(Tool::Shell, &["fish"])]
    #[case(Tool::Lazygit, &["lazygit"])]
    #[case(Tool::Nvim, &["nvim", "."])]
    fn tool_runs_its_command(#[case] tool: Tool, #[case] expected: &[&str]) {
        // Given / When / Then: `tool` runs `expected`, with fish as the shell.
        let expected: Vec<OsString> = expected.iter().map(OsString::from).collect();
        assert_eq!(tool.argv("fish".as_ref()), expected, "{tool:?}'s command");
    }
}
