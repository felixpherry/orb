//! What orb asks of the desktop: show a notification with a title and a body.

use std::fmt;
use std::sync::Arc;

use error_stack::Report;
use wherror::Error;

use crate::feat::sessions::state::Notice;

/// A notification couldn't be sent.
#[derive(Debug, Error)]
#[error(debug)]
pub struct NotifyError;

/// Shows desktop notifications.
pub trait Notifier: Send + Sync {
    fn name(&self) -> &'static str;

    /// Shows a notification titled `title` saying `body`, without waiting
    /// for it to be shown.
    ///
    /// # Errors
    ///
    /// Returns an error if the notification couldn't be sent.
    fn notify(&self, title: &str, body: &str) -> Result<(), Report<NotifyError>>;
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

    /// Announces `notice`: titled `<project> · <thread title>`, saying why
    /// the thread needs the user.
    ///
    /// # Errors
    ///
    /// Returns an error if the notification couldn't be sent.
    pub fn announce(&self, notice: &Notice) -> Result<(), Report<NotifyError>> {
        let title = format!("{} · {}", notice.project, notice.title);
        self.notifier.notify(&title, notice.kind.label())
    }
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
    use std::sync::{Arc, Mutex, PoisonError};

    use error_stack::Report;

    use super::{Notifier, NotifierService, NotifyError};
    use crate::feat::sessions::state::{Notice, NoticeKind, ThreadId};

    /// A notifier that records each title and body it was asked to show.
    #[derive(Default)]
    struct FakeNotifier {
        shown: Mutex<Vec<(String, String)>>,
    }

    impl FakeNotifier {
        fn shown(&self) -> Vec<(String, String)> {
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

        fn notify(&self, title: &str, body: &str) -> Result<(), Report<NotifyError>> {
            self.shown
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((title.to_owned(), body.to_owned()));
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
        NotifierService::new(notifier.clone()).announce(&notice(NoticeKind::Finished))?;

        // Then one notification is titled by the project and the thread.
        let titles: Vec<String> = notifier
            .shown()
            .into_iter()
            .map(|(title, _)| title)
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
        NotifierService::new(notifier.clone()).announce(&notice(kind))?;

        // Then the notification's body says `expected`.
        let bodies: Vec<String> = notifier.shown().into_iter().map(|(_, body)| body).collect();
        assert_eq!(bodies, [expected], "the body for {kind:?}");
        Ok(())
    }
}
