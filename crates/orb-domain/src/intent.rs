//! User intents: what a key press asks orb to do.

/// A user action produced by the keymap and applied by the
/// [`IntentHandler`](crate::IntentHandler).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// Quit orb.
    Quit,
    /// Attach to the terminal pane.
    Attach,
    /// Return from the terminal pane to orb.
    Detach,
}
