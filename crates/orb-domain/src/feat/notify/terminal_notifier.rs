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
//! thread of its own waits for it to exit. If it fails, for example because
//! it isn't allowed to notify, that thread sends the same notification through
//! a fallback notifier instead.

use std::ffi::OsString;
use std::fmt::Write;
use std::path::PathBuf;
use std::sync::Arc;

use error_stack::Report;

use super::click::ClickTarget;
use super::notifier::{Notifier, NotifyError, Urgency, spawn_detached};
use crate::feat::sessions::state::ThreadId;

/// Shows notifications with `terminal-notifier`, whose click runs a command,
/// and through `fallback` when terminal-notifier exits unsuccessfully.
#[derive(Clone)]
pub struct TerminalNotifierNotifier {
    program: PathBuf,
    click: ClickTarget,
    fallback: Arc<dyn Notifier>,
}

impl TerminalNotifierNotifier {
    pub fn new(program: PathBuf, click: ClickTarget, fallback: Arc<dyn Notifier>) -> Self {
        Self {
            program,
            click,
            fallback,
        }
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
        let ClickTarget { kitty, zellij, .. } = &self.click;
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
        urgency: Urgency,
        thread: ThreadId,
        tab: Option<u64>,
    ) -> Result<(), Report<NotifyError>> {
        let resend = {
            let fallback = Arc::clone(&self.fallback);
            let (title, body) = (title.to_owned(), body.to_owned());
            move || {
                let _ = fallback.notify(&title, &body, urgency, thread, tab);
            }
        };
        spawn_detached(&self.program, self.args(title, body, thread, tab), resend)
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
    use std::sync::Arc;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::time::Duration;

    use error_stack::{Report, ResultExt};
    use tempfile::TempDir;

    use super::{TerminalNotifierNotifier, window_title_match};
    use crate::feat::notify::click::{ClickTarget, Kitty, ZellijTarget};
    use crate::feat::notify::notifier::{Notifier, NotifyError, Urgency};
    use crate::feat::sessions::state::ThreadId;

    /// What a fallback notifier was asked to show: title, body, thread and tab.
    type Resent = (String, String, ThreadId, Option<u64>);

    /// A fallback notifier that sends each notification it's asked to show
    /// down a channel.
    struct Fallback(Sender<Resent>);

    impl Notifier for Fallback {
        fn name(&self) -> &'static str {
            "fallback"
        }

        fn notify(
            &self,
            title: &str,
            body: &str,
            _urgency: Urgency,
            thread: ThreadId,
            tab: Option<u64>,
        ) -> Result<(), Report<NotifyError>> {
            let _ = self
                .0
                .send((title.to_owned(), body.to_owned(), thread, tab));
            Ok(())
        }
    }

    /// A fallback notifier, and what it gets asked to show.
    fn fallback() -> (Arc<Fallback>, Receiver<Resent>) {
        let (sender, resent) = mpsc::channel();
        (Arc::new(Fallback(sender)), resent)
    }

    /// Writes a `terminal-notifier` into `dir` that shows nothing and exits `code`.
    fn exiting(dir: &Path, code: i32) -> Result<PathBuf, Report<NotifyError>> {
        let path = dir.join("terminal-notifier");
        fs::write(&path, format!("#!/bin/sh\nexit {code}\n")).change_context(NotifyError)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .change_context(NotifyError)?;
        Ok(path)
    }

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
        TerminalNotifierNotifier::new(PathBuf::from("/t/terminal-notifier"), click, fallback().0)
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
            niri: None,
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
        let notifier = notifier(ClickTarget {
            kitty,
            zellij,
            niri: None,
        });

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
            niri: None,
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
            niri: None,
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

    #[rstest::rstest]
    #[case::not_allowed_to_notify(3)]
    #[case::refused_by_the_notification_service(5)]
    fn failing_terminal_notifier_resends_the_notice_through_the_fallback(
        #[case] code: i32,
    ) -> Result<(), Report<NotifyError>> {
        // Given a terminal-notifier that exits `code`, and a fallback notifier.
        let dir = TempDir::new().change_context(NotifyError)?;
        let (fallback, resent) = fallback();
        let notifier = TerminalNotifierNotifier::new(
            exiting(dir.path(), code)?,
            ClickTarget::default(),
            fallback,
        );

        // When notifying that thread 7 finished, with orb on tab 3.
        notifier.notify("orb · x", "Finished", Urgency::Normal, ThreadId(7), Some(3))?;

        // Then the fallback is asked to show the same notification.
        assert_eq!(
            resent.recv_timeout(Duration::from_secs(5)).ok(),
            Some((
                "orb · x".to_owned(),
                "Finished".to_owned(),
                ThreadId(7),
                Some(3)
            )),
            "what the fallback showed after terminal-notifier exited {code}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn succeeding_terminal_notifier_sends_nothing_through_the_fallback()
    -> Result<(), Report<NotifyError>> {
        // Given a terminal-notifier that exits 0, and a fallback notifier.
        let dir = TempDir::new().change_context(NotifyError)?;
        let (fallback, resent) = fallback();
        let notifier = TerminalNotifierNotifier::new(
            exiting(dir.path(), 0)?,
            ClickTarget::default(),
            fallback,
        );

        // When notifying that thread 7 finished.
        notifier.notify("orb · x", "Finished", Urgency::Normal, ThreadId(7), None)?;

        // Then the fallback is asked to show nothing.
        assert!(
            resent.recv_timeout(Duration::from_secs(1)).is_err(),
            "the fallback should stay quiet when terminal-notifier succeeds"
        );
        Ok(())
    }
}
