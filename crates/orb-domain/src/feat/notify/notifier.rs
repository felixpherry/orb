//! What orb asks of the desktop: show a notification with a title and a body.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;

use error_stack::{Report, ResultExt};
use wherror::Error;

use super::click::{ClickTarget, on_path};
use super::osascript::OsascriptNotifier;
use super::terminal_notifier::TerminalNotifierNotifier;
use crate::feat::sessions::state::{Notice, NoticeKind, ThreadId};

/// A notification couldn't be sent.
#[derive(Debug, Error)]
#[error(debug)]
pub struct NotifyError;

/// How insistently a notification asks for attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    /// Shown and let go, like any other notification.
    Normal,
    /// Stays on screen until the user deals with it, where the desktop can.
    Critical,
}

/// Shows desktop notifications.
pub trait Notifier: Send + Sync {
    fn name(&self) -> &'static str;

    /// Shows a notification titled `title` saying `body` about `thread`,
    /// without waiting for it to be shown, asking at `urgency` where the
    /// notifier can say how insistently. A notifier that can replaces an
    /// earlier notification about `thread`, and takes a click back to orb,
    /// on zellij tab `tab` if known.
    ///
    /// # Errors
    ///
    /// Returns an error if the notification couldn't be sent.
    fn notify(
        &self,
        title: &str,
        body: &str,
        urgency: Urgency,
        thread: ThreadId,
        tab: Option<u64>,
    ) -> Result<(), Report<NotifyError>>;
}

/// Shared handle to the [`Notifier`] in use.
#[derive(Clone)]
pub struct NotifierService {
    notifier: Arc<dyn Notifier>,
}

impl NotifierService {
    pub fn new(notifier: Arc<dyn Notifier>) -> Self {
        Self { notifier }
    }

    /// The desktop's notifier, whose click goes to `click`. On macOS it is
    /// `terminal-notifier` if `path` (a `PATH` value) has it, falling back to
    /// `osascript` when it fails, else `osascript`. On Linux it is the
    /// session bus's notification server. Anywhere else it shows nothing.
    pub fn desktop(path: &OsStr, click: ClickTarget) -> Self {
        let notifier = if cfg!(target_os = "macos") {
            macos_notifier(path, click)
        } else {
            linux_notifier(click)
        };
        Self::new(notifier)
    }

    /// Announces `notice`: titled `<project> · <thread title>`, saying why
    /// the thread needs the user. A click goes back to orb, on zellij tab
    /// `tab` if known.
    ///
    /// # Errors
    ///
    /// Returns an error if the notification couldn't be sent.
    pub fn announce(&self, notice: &Notice, tab: Option<u64>) -> Result<(), Report<NotifyError>> {
        let title = format!("{} · {}", notice.project, notice.title);
        self.notifier.notify(
            &title,
            notice.kind.label(),
            urgency(notice.kind),
            notice.thread,
            tab,
        )
    }
}

/// How insistently a notice of `kind` asks: a thread that is blocked on the
/// user is critical, a finished turn is not.
fn urgency(kind: NoticeKind) -> Urgency {
    match kind {
        NoticeKind::Finished => Urgency::Normal,
        NoticeKind::NeedsApproval | NoticeKind::NeedsInput => Urgency::Critical,
    }
}

/// macOS's notifier: `terminal-notifier` if `path` has it, whose click goes
/// to `click` and which falls back to `osascript`; else `osascript`.
fn macos_notifier(path: &OsStr, click: ClickTarget) -> Arc<dyn Notifier> {
    match on_path("terminal-notifier", path) {
        Some(program) => Arc::new(TerminalNotifierNotifier::new(
            program,
            click,
            Arc::new(OsascriptNotifier),
        )),
        None => Arc::new(OsascriptNotifier),
    }
}

/// Linux's notifier: the session bus's notification server, whose click
/// runs `click`'s steps.
#[cfg(target_os = "linux")]
fn linux_notifier(click: ClickTarget) -> Arc<dyn Notifier> {
    use super::click::CommandClickRunner;
    use super::xdg::{XdgNotifier, ZbusDesktop};

    Arc::new(XdgNotifier::new(
        Arc::new(ZbusDesktop),
        click,
        Arc::new(CommandClickRunner),
    ))
}

/// Off Linux there is no notification server to use, so nothing is shown.
#[cfg(not(target_os = "linux"))]
fn linux_notifier(_click: ClickTarget) -> Arc<dyn Notifier> {
    Arc::new(super::none::NoneNotifier)
}

/// Runs `program` with `args` and no stdin, stdout or stderr, so it never
/// writes into orb's screen, without waiting for it: a thread of its own
/// waits for it to exit, and runs `on_failure` if it doesn't exit 0.
///
/// # Errors
///
/// Returns an error if `program` can't start.
pub fn spawn_detached<P, F>(
    program: P,
    args: Vec<OsString>,
    on_failure: F,
) -> Result<(), Report<NotifyError>>
where
    P: AsRef<OsStr>,
    F: FnOnce() + Send + 'static,
{
    let program = program.as_ref();
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .change_context(NotifyError)
        .attach_with(|| format!("couldn't run {}", program.display()))?;
    thread::Builder::new()
        .name("orb-notify".into())
        .spawn(move || {
            if !child.wait().is_ok_and(|status| status.success()) {
                on_failure();
            }
        })
        .map(drop)
        .change_context(NotifyError)
        .attach("couldn't wait for the notifier")
}

impl fmt::Debug for NotifierService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Notifier<{}>", self.notifier.name())
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate failures with `?` and assert on the outcome"
)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex, PoisonError};

    use error_stack::Report;
    use tempfile::TempDir;

    use super::{Notifier, NotifierService, NotifyError, Urgency};
    use crate::feat::notify::click::ClickTarget;
    use crate::feat::sessions::state::{Notice, NoticeKind, ThreadId};

    /// What the fake notifier was asked to show.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Shown {
        title: String,
        body: String,
        urgency: Urgency,
        thread: ThreadId,
        tab: Option<u64>,
    }

    /// A notifier that records each notification it was asked to show.
    #[derive(Default)]
    struct FakeNotifier {
        shown: Mutex<Vec<Shown>>,
    }

    impl FakeNotifier {
        fn shown(&self) -> Vec<Shown> {
            self.shown
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl Notifier for FakeNotifier {
        fn name(&self) -> &'static str {
            "fake"
        }

        fn notify(
            &self,
            title: &str,
            body: &str,
            urgency: Urgency,
            thread: ThreadId,
            tab: Option<u64>,
        ) -> Result<(), Report<NotifyError>> {
            self.shown
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Shown {
                    title: title.to_owned(),
                    body: body.to_owned(),
                    urgency,
                    thread,
                    tab,
                });
            Ok(())
        }
    }

    fn notice(kind: NoticeKind) -> Notice {
        Notice {
            thread: ThreadId(1),
            kind,
            project: "orb".to_owned(),
            title: "Parser fix".to_owned(),
        }
    }

    #[rstest::rstest]
    fn announce_titles_the_notification_by_project_and_thread() -> Result<(), Report<NotifyError>> {
        // Given a notifier.
        let notifier = Arc::new(FakeNotifier::default());

        // When announcing that orb's "Parser fix" thread finished.
        NotifierService::new(notifier.clone()).announce(&notice(NoticeKind::Finished), None)?;

        // Then one notification is titled by the project and the thread.
        let titles: Vec<String> = notifier
            .shown()
            .into_iter()
            .map(|shown| shown.title)
            .collect();
        assert_eq!(titles, ["orb · Parser fix"], "the notification's title");
        Ok(())
    }

    #[rstest::rstest]
    #[case(NoticeKind::Finished, "Finished")]
    #[case(NoticeKind::NeedsApproval, "Needs approval")]
    #[case(NoticeKind::NeedsInput, "Needs input")]
    fn announce_says_why_in_the_body(
        #[case] kind: NoticeKind,
        #[case] expected: &str,
    ) -> Result<(), Report<NotifyError>> {
        // Given a notifier.
        let notifier = Arc::new(FakeNotifier::default());

        // When announcing a notice of `kind`.
        NotifierService::new(notifier.clone()).announce(&notice(kind), None)?;

        // Then the notification's body says `expected`.
        let bodies: Vec<String> = notifier
            .shown()
            .into_iter()
            .map(|shown| shown.body)
            .collect();
        assert_eq!(bodies, [expected], "the body for {kind:?}");
        Ok(())
    }

    #[rstest::rstest]
    #[case(NoticeKind::Finished, Urgency::Normal)]
    #[case(NoticeKind::NeedsApproval, Urgency::Critical)]
    #[case(NoticeKind::NeedsInput, Urgency::Critical)]
    fn announce_urgency_follows_notice_kind(
        #[case] kind: NoticeKind,
        #[case] expected: Urgency,
    ) -> Result<(), Report<NotifyError>> {
        // Given a notifier.
        let notifier = Arc::new(FakeNotifier::default());

        // When announcing a notice of `kind`.
        NotifierService::new(notifier.clone()).announce(&notice(kind), None)?;

        // Then the notifier is asked at `expected` urgency.
        let urgencies: Vec<Urgency> = notifier
            .shown()
            .into_iter()
            .map(|shown| shown.urgency)
            .collect();
        assert_eq!(urgencies, [expected], "the urgency for {kind:?}");
        Ok(())
    }

    #[rstest::rstest]
    fn announce_tells_the_notifier_the_thread_and_orbs_tab() -> Result<(), Report<NotifyError>> {
        // Given a notifier.
        let notifier = Arc::new(FakeNotifier::default());

        // When announcing thread 1's notice with orb on tab 3.
        NotifierService::new(notifier.clone()).announce(&notice(NoticeKind::Finished), Some(3))?;

        // Then the notifier hears of thread 1 and tab 3.
        let heard: Vec<(ThreadId, Option<u64>)> = notifier
            .shown()
            .into_iter()
            .map(|shown| (shown.thread, shown.tab))
            .collect();
        assert_eq!(heard, [(ThreadId(1), Some(3))], "the thread and tab");
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[rstest::rstest]
    #[case::terminal_notifier_is_on_path(Some(0o755), "Notifier<terminal-notifier>")]
    #[case::terminal_notifier_isnt_executable(Some(0o644), "Notifier<osascript>")]
    #[case::terminal_notifier_is_missing(None, "Notifier<osascript>")]
    fn desktop_uses_terminal_notifier_when_path_has_it(
        #[case] mode: Option<u32>,
        #[case] expected: &str,
    ) -> std::io::Result<()> {
        // Given a PATH directory holding terminal-notifier with `mode`, or not at all.
        let dir = TempDir::new()?;
        if let Some(mode) = mode {
            let program = dir.path().join("terminal-notifier");
            fs::write(&program, "#!/bin/sh\n")?;
            fs::set_permissions(&program, fs::Permissions::from_mode(mode))?;
        }

        // When picking the desktop's notifier.
        let service = NotifierService::desktop(&OsString::from(dir.path()), ClickTarget::default());

        // Then it is terminal-notifier only if it's there to run.
        assert_eq!(format!("{service:?}"), expected, "the notifier picked");
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[rstest::rstest]
    fn desktop_notifier_on_linux_is_xdg() -> std::io::Result<()> {
        // Given a PATH directory holding an executable terminal-notifier.
        let dir = TempDir::new()?;
        let program = dir.path().join("terminal-notifier");
        fs::write(&program, "#!/bin/sh\n")?;
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755))?;

        // When picking the desktop's notifier on Linux.
        let service = NotifierService::desktop(&OsString::from(dir.path()), ClickTarget::default());

        // Then it is the freedesktop notifier, not terminal-notifier.
        assert_eq!(
            format!("{service:?}"),
            "Notifier<xdg>",
            "the notifier picked on Linux"
        );
        Ok(())
    }
}
