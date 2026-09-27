//! `terminal-notifier` as orb's [`Notifier`], whose notifications take the
//! user back to orb when clicked.
//!
//! A click makes macOS relaunch terminal-notifier, which runs the
//! notification's `-execute` line with `/bin/sh`, outside zellij and with a
//! bare `PATH`. So the line names every program by its full path and orb's
//! session by name: kitty focuses the window showing orb's zellij session,
//! then zellij goes to orb's tab and focuses orb's pane. Each step orb
//! couldn't find at startup is left out, and without any there is no
//! `-execute`. Every word of the line is quoted for `sh`, so no session name
//! or path can change what it runs.
//!
//! terminal-notifier reads its options through `NSUserDefaults`, which parses
//! a value starting with `(`, `{` or `"` as a property list and takes one
//! starting with `-` for the next option. terminal-notifier strips one leading
//! backslash from every value to allow for that, so orb prefixes every value
//! with one. A later notification about the same thread replaces the earlier
//! one. Like `osascript`, each one runs with no stdin, stdout or stderr, and a
//! thread of its own waits for it to exit.

use std::ffi::{OsStr, OsString};
use std::fmt::Write;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use error_stack::Report;

use super::notifier::{Notifier, NotifyError, spawn_detached};
use crate::feat::sessions::state::ThreadId;

/// Where a click on a notification takes the user: what orb found at startup.
#[derive(Debug, Clone, Default)]
pub struct ClickTarget {
    /// orb's kitty; `None` without `kitten` or kitty's socket.
    pub kitty: Option<Kitty>,
    /// orb's zellij pane; `None` outside zellij.
    pub zellij: Option<ZellijTarget>,
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

/// Shows notifications with `terminal-notifier`, whose click runs a command.
#[derive(Debug, Clone)]
pub struct TerminalNotifierNotifier {
    program: PathBuf,
    click: ClickTarget,
}

impl TerminalNotifierNotifier {
    pub fn new(program: PathBuf, click: ClickTarget) -> Self {
        Self { program, click }
    }

    /// terminal-notifier's arguments to show `title` and `body` about
    /// `thread`, replacing an earlier one about it, whose click goes back to
    /// orb, on zellij tab `tab` if known.
    fn args(&self, title: &str, body: &str, thread: ThreadId, tab: Option<u64>) -> Vec<OsString> {
        let group = format!("orb-{}", thread.0);
        let execute = self.click_command(tab);
        [
            ("-title", Some(title)),
            ("-message", Some(body)),
            ("-group", Some(group.as_str())),
            ("-execute", execute.as_deref()),
        ]
        .into_iter()
        .filter_map(|(option, value)| value.map(|value| [option.into(), format!("\\{value}")]))
        .flatten()
        .map(OsString::from)
        .collect()
    }

    /// The `sh` line a click runs; `None` if orb knows nothing to go back to.
    fn click_command(&self, tab: Option<u64>) -> Option<String> {
        let ClickTarget { kitty, zellij } = &self.click;
        let window = match (kitty, zellij) {
            (Some(kitty), Some(zellij)) => Some((kitty, window_title_match(&zellij.session))),
            (Some(kitty), None) => kitty.window.map(|id| (kitty, format!("id:{id}"))),
            (None, _) => None,
        };
        let steps = {
            let mut steps = Vec::new();
            if let Some((kitty, matching)) = window {
                let kitten = kitty.kitten.to_string_lossy();
                let to = kitty.socket.as_str();
                steps.push(sh_words(&[
                    &kitten,
                    "@",
                    "--to",
                    to,
                    "focus-window",
                    "--match",
                    &matching,
                ]));
            }
            if let Some(zellij) = zellij {
                let program = zellij.zellij.to_string_lossy();
                let (program, session) = (program.as_ref(), zellij.session.as_str());
                if let Some(tab) = tab {
                    let tab = tab.to_string();
                    let go = [
                        program,
                        "--session",
                        session,
                        "action",
                        "go-to-tab-by-id",
                        &tab,
                    ];
                    steps.push(sh_words(&go));
                }
                let pane = format!("terminal_{}", zellij.pane);
                let focus = [
                    program,
                    "--session",
                    session,
                    "action",
                    "focus-pane-id",
                    &pane,
                ];
                steps.push(sh_words(&focus));
            }
            steps
        };
        (!steps.is_empty()).then(|| steps.join("; "))
    }
}

impl Notifier for TerminalNotifierNotifier {
    fn name(&self) -> &'static str {
        "terminal-notifier"
    }

    fn notify(
        &self,
        title: &str,
        body: &str,
        thread: ThreadId,
        tab: Option<u64>,
    ) -> Result<(), Report<NotifyError>> {
        spawn_detached(&self.program, self.args(title, body, thread, tab))
    }
}

/// `words` as one `sh` command, each single-quoted.
fn sh_words(words: &[&str]) -> String {
    words
        .iter()
        .map(|word| format!("'{}'", word.replace('\'', r"'\''")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A kitty `--match` for the window showing zellij session `session`, which
/// zellij titles `<session> | <focused pane's title>`. kitty's title match is
/// a Python regex, and its match syntax splits on spaces, so every character
/// but a letter, digit or `_` is written as a `\U` escape.
fn window_title_match(session: &str) -> String {
    format!("{session} |")
        .chars()
        .fold("title:^".to_owned(), |mut regex, ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                regex.push(ch);
            } else {
                let _ = write!(regex, "\\U{:08x}", u32::from(ch));
            }
            regex
        })
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
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate setup failures with `?` and assert on the outcome"
)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use tempfile::TempDir;

    use super::{ClickTarget, Kitty, TerminalNotifierNotifier, ZellijTarget, window_title_match};
    use crate::feat::sessions::state::ThreadId;

    fn kitty(window: Option<u64>) -> Kitty {
        Kitty {
            kitten: PathBuf::from("/k/kitten"),
            socket: "unix:/tmp/kitty-1".to_owned(),
            window,
        }
    }

    fn zellij(session: &str) -> ZellijTarget {
        ZellijTarget {
            zellij: PathBuf::from("/z/zellij"),
            session: session.to_owned(),
            pane: 4,
        }
    }

    fn notifier(click: ClickTarget) -> TerminalNotifierNotifier {
        TerminalNotifierNotifier::new(PathBuf::from("/t/terminal-notifier"), click)
    }

    /// The `-execute` value in `args`, without its leading backslash.
    fn execute(args: &[OsString]) -> Option<String> {
        let at = args.iter().position(|arg| arg == "-execute")?;
        let value = args.get(at + 1)?.to_string_lossy();
        value.strip_prefix('\\').map(str::to_owned)
    }

    #[rstest::rstest]
    fn args_prefix_every_value_with_a_backslash() {
        // Given a notifier with nothing to go back to, and a title that looks like an option.
        let notifier = notifier(ClickTarget::default());

        // When building the arguments for thread 7.
        let args = notifier.args("-orb · (x) \"y\"", "Finished", ThreadId(7), None);

        // Then title, body and the thread's group each carry one leading backslash.
        let expected: Vec<OsString> = [
            "-title",
            "\\-orb · (x) \"y\"",
            "-message",
            "\\Finished",
            "-group",
            "\\orb-7",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert_eq!(args, expected, "terminal-notifier's arguments");
    }

    #[rstest::rstest]
    fn args_prefix_the_click_command_with_a_backslash() {
        // Given a notifier inside zellij.
        let notifier = notifier(ClickTarget {
            kitty: None,
            zellij: Some(zellij("s")),
        });

        // When building the arguments.
        let args = notifier.args("orb · x", "Finished", ThreadId(7), None);

        // Then the value after -execute starts with one backslash.
        let value = args
            .iter()
            .position(|arg| arg == "-execute")
            .and_then(|at| args.get(at + 1));
        assert!(
            value.is_some_and(|value| value.to_string_lossy().starts_with("\\'")),
            "the -execute value in {args:?}"
        );
    }

    #[rstest::rstest]
    #[case::kitty_and_zellij_on_a_known_tab(
        Some(kitty(Some(9))),
        Some(zellij("s")),
        Some(3),
        Some(
            "'/k/kitten' '@' '--to' 'unix:/tmp/kitty-1' 'focus-window' '--match' \
             'title:^s\\U00000020\\U0000007c'; \
             '/z/zellij' '--session' 's' 'action' 'go-to-tab-by-id' '3'; \
             '/z/zellij' '--session' 's' 'action' 'focus-pane-id' 'terminal_4'"
        )
    )]
    #[case::zellij_on_an_unknown_tab(
        Some(kitty(None)),
        Some(zellij("s")),
        None,
        Some(
            "'/k/kitten' '@' '--to' 'unix:/tmp/kitty-1' 'focus-window' '--match' \
             'title:^s\\U00000020\\U0000007c'; \
             '/z/zellij' '--session' 's' 'action' 'focus-pane-id' 'terminal_4'"
        )
    )]
    #[case::zellij_without_kitty(
        None,
        Some(zellij("s")),
        Some(3),
        Some(
            "'/z/zellij' '--session' 's' 'action' 'go-to-tab-by-id' '3'; \
             '/z/zellij' '--session' 's' 'action' 'focus-pane-id' 'terminal_4'"
        )
    )]
    #[case::kitty_outside_zellij(
        Some(kitty(Some(9))),
        None,
        None,
        Some("'/k/kitten' '@' '--to' 'unix:/tmp/kitty-1' 'focus-window' '--match' 'id:9'")
    )]
    #[case::kitty_outside_zellij_without_a_window(Some(kitty(None)), None, None, None)]
    #[case::nothing(None, None, None, None)]
    fn click_command_has_a_step_for_each_thing_orb_knows(
        #[case] kitty: Option<Kitty>,
        #[case] zellij: Option<ZellijTarget>,
        #[case] tab: Option<u64>,
        #[case] expected: Option<&str>,
    ) {
        // Given a notifier knowing `kitty` and `zellij`.
        let notifier = notifier(ClickTarget { kitty, zellij });

        // When building the arguments with orb on `tab`.
        let args = notifier.args("orb · x", "Finished", ThreadId(7), tab);

        // Then -execute runs one step for each, or is left out.
        assert_eq!(
            execute(&args).as_deref(),
            expected,
            "the -execute line in {args:?}"
        );
    }

    #[rstest::rstest]
    fn click_command_matches_kittys_window_by_an_escaped_session_title() {
        // Given a notifier in a zellij session whose name holds regex, quote and space characters.
        let notifier = notifier(ClickTarget {
            kitty: Some(kitty(None)),
            zellij: Some(zellij("it's a.b")),
        });

        // When building the arguments.
        let args = notifier.args("orb · x", "Finished", ThreadId(7), None);

        // Then kitty matches a title starting with the session and " |", every other character escaped.
        let expected = "'--match' \
             'title:^it\\U00000027s\\U00000020a\\U0000002eb\\U00000020\\U0000007c'";
        assert!(
            execute(&args).is_some_and(|line| line.contains(expected)),
            "the kitty match in {args:?}"
        );
    }

    /// Writes an executable `name` into `dir` that appends its arguments, one
    /// per line, then `--`, to `log`.
    fn recorder(dir: &Path, name: &str, log: &Path) -> std::io::Result<PathBuf> {
        let path = dir.join(name);
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" -- >> '{}'\n",
            log.display()
        );
        fs::write(&path, script)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        Ok(path)
    }

    #[rstest::rstest]
    #[case::a_quote("it's")]
    #[case::a_substitution("$(touch pwned)`touch pwned`")]
    #[case::spaces_and_separators("two words; a|b && c")]
    fn click_command_hands_each_program_its_arguments_verbatim(
        #[case] session: &str,
    ) -> std::io::Result<()> {
        // Given recording zellij and kitten programs in a directory whose name needs quoting.
        let root = TempDir::new()?;
        let bin = root.path().join("it's $dir");
        fs::create_dir_all(&bin)?;
        let log = root.path().join("log");
        let click = ClickTarget {
            kitty: Some(Kitty {
                kitten: recorder(&bin, "kitten", &log)?,
                socket: "unix:/tmp/kitty '1'".to_owned(),
                window: None,
            }),
            zellij: Some(ZellijTarget {
                zellij: recorder(&bin, "zellij", &log)?,
                session: session.to_owned(),
                pane: 4,
            }),
        };
        let args = notifier(click).args("orb · x", "Finished", ThreadId(7), Some(3));

        // When sh runs the -execute line in the temporary directory.
        let line = execute(&args).unwrap_or_default();
        Command::new("/bin/sh")
            .args(["-c", &line])
            .current_dir(root.path())
            .status()?;

        // Then each program got exactly its arguments, and nothing else ran.
        let recorded = fs::read_to_string(&log)?;
        let kitty_match = window_title_match(session);
        let expected = [
            "@",
            "--to",
            "unix:/tmp/kitty '1'",
            "focus-window",
            "--match",
            &kitty_match,
            "--",
            "--session",
            session,
            "action",
            "go-to-tab-by-id",
            "3",
            "--",
            "--session",
            session,
            "action",
            "focus-pane-id",
            "terminal_4",
            "--",
        ]
        .map(|line| format!("{line}\n"))
        .concat();
        let pwned = root.path().join("pwned").exists();
        assert_eq!(
            (recorded, pwned),
            (expected, false),
            "what the programs got for {session:?}, and whether anything else ran"
        );
        Ok(())
    }
}
