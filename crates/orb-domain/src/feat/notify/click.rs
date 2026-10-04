//! Where a click on a notification takes the user: the kitty window, zellij
//! tab and pane orb runs in, and on Linux the niri window showing it. On
//! Linux orb runs the click's steps itself.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;

use crate::common::{Finished, run_within};

/// Where a click on a notification takes the user: what orb found at startup.
#[derive(Debug, Clone, Default)]
pub struct ClickTarget {
    /// orb's kitty; `None` without `kitten` or kitty's socket.
    pub kitty: Option<Kitty>,
    /// orb's zellij pane; `None` outside zellij.
    pub zellij: Option<ZellijTarget>,
    /// niri, the compositor orb's window is on; `None` without `NIRI_SOCKET` or `niri`.
    pub niri: Option<NiriTarget>,
}

/// The kitty orb runs in, driven by its remote control.
#[derive(Debug, Clone)]
pub struct Kitty {
    /// The `kitten` program.
    pub kitten: PathBuf,
    /// kitty's remote control socket, as `KITTY_LISTEN_ON` gives it.
    pub socket: String,
    /// orb's kitty window, `KITTY_WINDOW_ID`, used outside zellij only.
    pub window: Option<u64>,
}

/// orb's pane in its zellij session.
#[derive(Debug, Clone)]
pub struct ZellijTarget {
    /// The `zellij` program.
    pub zellij: PathBuf,
    /// orb's session.
    pub session: String,
    /// orb's terminal pane, `terminal_<pane>`.
    pub pane: u32,
}

/// The niri compositor, driven by `niri msg`.
#[derive(Debug, Clone)]
pub struct NiriTarget {
    /// The `niri` program.
    pub niri: PathBuf,
}

/// How long one click step may run before it's killed.
const CLICK_LIMIT: Duration = Duration::from_secs(2);

/// Runs a click step's program.
pub trait ClickRunner: Send + Sync {
    fn name(&self) -> &'static str;

    /// Runs `program` with `args`; its stdout if it exits 0, else `None`
    /// (it couldn't start, failed, or ran too long).
    fn run(&self, program: &Path, args: &[OsString]) -> Option<String>;
}

/// Runs click steps as child processes, each killed after [`CLICK_LIMIT`].
#[derive(Debug, Clone, Copy, Default)]
pub struct CommandClickRunner;

impl ClickRunner for CommandClickRunner {
    fn name(&self) -> &'static str {
        "command"
    }

    fn run(&self, program: &Path, args: &[OsString]) -> Option<String> {
        let command = {
            let mut command = Command::new(program);
            command.args(args);
            command
        };
        match run_within(command, CLICK_LIMIT) {
            Ok(Finished::Exited(output)) if output.status.success() => {
                Some(String::from_utf8_lossy(&output.stdout).into_owned())
            }
            _ => None,
        }
    }
}

/// Takes the user back to orb: raises orb's niri window, then goes to orb's
/// zellij tab (when `tab` is known) and focuses orb's pane. A step orb
/// can't do is skipped, and a failed step doesn't stop the next.
pub fn focus_orb(click: &ClickTarget, tab: Option<u64>, runner: &dyn ClickRunner) {
    if let (Some(niri), Some(zellij)) = (&click.niri, &click.zellij) {
        let window = runner
            .run(&niri.niri, &os_args(&["msg", "--json", "windows"]))
            .and_then(|json| niri_window(&json, &zellij.session));
        if let Some(id) = window {
            let id = id.to_string();
            runner.run(
                &niri.niri,
                &os_args(&["msg", "action", "focus-window", "--id", &id]),
            );
        }
    }
    if let Some(zellij) = &click.zellij {
        let session = zellij.session.as_str();
        if let Some(tab) = tab {
            let tab = tab.to_string();
            runner.run(
                &zellij.zellij,
                &os_args(&["--session", session, "action", "go-to-tab-by-id", &tab]),
            );
        }
        let pane = format!("terminal_{}", zellij.pane);
        runner.run(
            &zellij.zellij,
            &os_args(&["--session", session, "action", "focus-pane-id", &pane]),
        );
    }
}

/// `words` as a program's arguments.
fn os_args(words: &[&str]) -> Vec<OsString> {
    words.iter().map(OsString::from).collect()
}

/// One niri window, as `niri msg --json windows` lists it.
#[derive(Deserialize)]
struct NiriWindow {
    id: u64,
    title: Option<String>,
}

/// The first niri window in `json` showing zellij session `session`, whose
/// title zellij sets to `<session> | <pane title>`; `None` if there's none
/// or `json` isn't niri's window list.
fn niri_window(json: &str, session: &str) -> Option<u64> {
    let prefix = format!("{session} |");
    serde_json::from_str::<Vec<NiriWindow>>(json)
        .ok()?
        .into_iter()
        .find(|window| {
            window
                .title
                .as_deref()
                .is_some_and(|title| title.starts_with(&prefix))
        })
        .map(|window| window.id)
}

/// The executable `program` in the first absolute directory of `path` (a
/// `PATH` value) that has one.
pub fn on_path(program: &str, path: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, PoisonError};

    use super::{ClickRunner, ClickTarget, NiriTarget, ZellijTarget, focus_orb, niri_window};

    /// One program run: the program and its arguments.
    type Call = (PathBuf, Vec<String>);

    /// niri's window list with a browser whose title holds ` | ` and orb's kitty.
    const WINDOWS: &str = r#"[
        {"id": 2, "title": "API Keys | Settings | Firefox", "app_id": "firefox"},
        {"id": 5, "title": null, "app_id": "mpv"},
        {"id": 6, "title": "tremendous-panda | orb ~", "app_id": "kitty"}
    ]"#;

    /// A click runner that records every run and answers niri's window list
    /// with `windows`, everything else with empty output.
    struct FakeRunner {
        calls: Mutex<Vec<Call>>,
        windows: Option<String>,
    }

    impl FakeRunner {
        fn new(windows: &str) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                windows: Some(windows.to_owned()),
            }
        }

        fn calls(&self) -> Vec<Call> {
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl ClickRunner for FakeRunner {
        fn name(&self) -> &'static str {
            "fake"
        }

        fn run(&self, program: &Path, args: &[OsString]) -> Option<String> {
            let args: Vec<String> = args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            let lists_windows = args == ["msg", "--json", "windows"];
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((program.to_owned(), args));
            if lists_windows {
                self.windows.clone()
            } else {
                Some(String::new())
            }
        }
    }

    fn click(niri: bool) -> ClickTarget {
        ClickTarget {
            kitty: None,
            zellij: Some(ZellijTarget {
                zellij: PathBuf::from("/z/zellij"),
                session: "tremendous-panda".to_owned(),
                pane: 4,
            }),
            niri: niri.then(|| NiriTarget {
                niri: PathBuf::from("/n/niri"),
            }),
        }
    }

    fn call(program: &str, args: &[&str]) -> Call {
        (
            PathBuf::from(program),
            args.iter().map(|arg| (*arg).to_owned()).collect(),
        )
    }

    fn list_windows() -> Call {
        call("/n/niri", &["msg", "--json", "windows"])
    }

    fn focus_window() -> Call {
        call("/n/niri", &["msg", "action", "focus-window", "--id", "6"])
    }

    fn go_to_tab() -> Call {
        call(
            "/z/zellij",
            &[
                "--session",
                "tremendous-panda",
                "action",
                "go-to-tab-by-id",
                "3",
            ],
        )
    }

    fn focus_pane() -> Call {
        call(
            "/z/zellij",
            &[
                "--session",
                "tremendous-panda",
                "action",
                "focus-pane-id",
                "terminal_4",
            ],
        )
    }

    #[rstest::rstest]
    fn click_focuses_niri_window_then_zellij_pane() {
        // Given orb under niri and zellij, with orb's kitty in niri's window list.
        let runner = FakeRunner::new(WINDOWS);

        // When the click takes the user back to orb on tab 3.
        focus_orb(&click(true), Some(3), &runner);

        // Then niri raises orb's window, then zellij goes to the tab and focuses the pane.
        assert_eq!(
            runner.calls(),
            vec![list_windows(), focus_window(), go_to_tab(), focus_pane()],
            "the click's steps, in order"
        );
    }

    #[rstest::rstest]
    fn click_without_niri_runs_only_zellij_steps() {
        // Given orb in zellij without niri.
        let runner = FakeRunner::new(WINDOWS);

        // When the click takes the user back to orb on tab 3.
        focus_orb(&click(false), Some(3), &runner);

        // Then only zellij's steps run.
        assert_eq!(
            runner.calls(),
            vec![go_to_tab(), focus_pane()],
            "the click's steps without niri"
        );
    }

    #[rstest::rstest]
    fn click_without_tab_skips_go_to_tab() {
        // Given orb under niri and zellij.
        let runner = FakeRunner::new(WINDOWS);

        // When the click takes the user back to orb on an unknown tab.
        focus_orb(&click(true), None, &runner);

        // Then zellij focuses the pane without going to a tab.
        assert_eq!(
            runner.calls(),
            vec![list_windows(), focus_window(), focus_pane()],
            "the click's steps without a tab"
        );
    }

    #[rstest::rstest]
    fn niri_window_is_picked_by_zellij_session_title() {
        // Given niri's window list with a browser titled with " | " first.
        // When looking for the window showing zellij session tremendous-panda.
        let window = niri_window(WINDOWS, "tremendous-panda");

        // Then it's orb's kitty window.
        assert_eq!(window, Some(6), "the window showing the session");
    }

    #[rstest::rstest]
    fn niri_lookup_with_no_matching_window_skips_focus() {
        // Given niri's window list without a window showing orb's session.
        let runner = FakeRunner::new(r#"[{"id": 2, "title": "other | orb ~"}]"#);

        // When the click takes the user back to orb on tab 3.
        focus_orb(&click(true), Some(3), &runner);

        // Then no window is focused, and zellij's steps still run.
        assert_eq!(
            runner.calls(),
            vec![list_windows(), go_to_tab(), focus_pane()],
            "the click's steps without a matching window"
        );
    }
}
