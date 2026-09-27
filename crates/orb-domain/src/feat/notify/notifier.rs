//! What orb asks of the desktop: show a notification with a title and a body.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;

use error_stack::{Report, ResultExt};
use wherror::Error;

use super::osascript::OsascriptNotifier;
use super::terminal_notifier::{ClickTarget, TerminalNotifierNotifier, on_path};
use crate::feat::sessions::state::{Notice, ThreadId};

/// A notification couldn't be sent.
#[derive(Debug, Error)]
#[error(debug)]
pub struct NotifyError;

/// Shows desktop notifications.
pub trait Notifier: Send + Sync {
    fn name(&self) -> &'static str;

    /// Shows a notification titled `title` saying `body` about `thread`,
    /// without waiting for it to be shown. A notifier that can replaces an
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

    /// The desktop's notifier: `terminal-notifier` if `path` (a `PATH`
    /// value) has it, whose click goes to `click`; else `osascript`.
    pub fn desktop(path: &OsStr, click: ClickTarget) -> Self {
        match on_path("terminal-notifier", path) {
            Some(program) => Self::new(Arc::new(TerminalNotifierNotifier::new(program, click))),
            None => Self::new(Arc::new(OsascriptNotifier)),
        }
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
        self.notifier
            .notify(&title, notice.kind.label(), notice.thread, tab)
    }
}

/// Runs `program` with `args` and no stdin, stdout or stderr, so it never
/// writes into orb's screen, without waiting for it: a thread of its own
/// waits for it to exit.
///
/// # Errors
///
/// Returns an error if `program` can't start.
pub fn spawn_detached<P>(program: P, args: Vec<OsString>) -> Result<(), Report<NotifyError>>
where
    P: AsRef<OsStr>,
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
            let _ = child.wait();
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

    use super::{Notifier, NotifierService, NotifyError};
    use crate::feat::notify::terminal_notifier::ClickTarget;
    use crate::feat::sessions::state::{Notice, NoticeKind, ThreadId};

    /// What the fake notifier was asked to show.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Shown {
        title: String,
        body: String,
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
            thread: ThreadId,
            tab: Option<u64>,
        ) -> Result<(), Report<NotifyError>> {
            self.shown
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Shown {
                    title: title.to_owned(),
                    body: body.to_owned(),
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
}
