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
    /// Move the preview's cursor to the next block.
    NextBlock,
    /// Move the preview's cursor to the previous block.
    PrevBlock,
    /// Scroll the preview half a page down.
    HalfPageDown,
    /// Scroll the preview half a page up.
    HalfPageUp,
    /// Jump to the preview's first block.
    Top,
    /// Jump to the preview's bottom and follow new blocks.
    Bottom,
    /// Fold or unfold the preview's cursor block.
    ToggleFold,
    /// Copy the preview's cursor block to the clipboard.
    Yank,
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
            Self::NextBlock => "next block",
            Self::PrevBlock => "previous block",
            Self::HalfPageDown => "half page down",
            Self::HalfPageUp => "half page up",
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::ToggleFold => "fold",
            Self::Yank => "yank",
        })
    }
}
