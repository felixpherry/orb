//! User intents: what a key press asks orb to do.

use std::fmt;

/// A user action produced by the keymap and applied by the
/// [`IntentHandler`](crate::IntentHandler).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// Quit orb; sessions keep running.
    Quit,
    /// Select the thread below.
    SelectNext,
    /// Select the thread above.
    SelectPrev,
    /// Move the keys to the selected thread's preview.
    FocusPreview,
    /// Move the keys to the sidebar.
    FocusSidebar,
    /// Attach to the selected thread's session.
    Attach,
    /// Return from the attached session to its preview.
    Detach,
    /// Start a Claude session in orb's working directory.
    NewSession,
}

/// The label the which-key popup shows for the intent.
impl fmt::Display for Intent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Quit => "quit",
            Self::SelectNext => "next thread",
            Self::SelectPrev => "previous thread",
            Self::FocusPreview => "focus preview",
            Self::FocusSidebar => "focus sidebar",
            Self::Attach => "attach",
            Self::Detach => "back to orb",
            Self::NewSession => "new session",
        })
    }
}
