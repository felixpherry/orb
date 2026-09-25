//! Commands: work the [`IntentHandler`](crate::IntentHandler) asks for after it
//! has updated [`AppState`](crate::AppState). Pane commands and `Yank` are
//! carried out by the frontend loop; session commands go to the sessions
//! actor; `ShowPreview` goes to the preview actor.

use crate::feat::sessions::state::AttachTarget;

/// Something that must happen in response to an intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Show the target's session in the terminal pane and send it input.
    Attach(AttachTarget),
    /// Stop sending input to the terminal pane.
    Detach,
    /// Start a new Claude session.
    CreateSession,
    /// Poll the sessions' statuses now instead of waiting for the next tick.
    RefreshSessions,
    /// Show the selected thread's transcript now instead of waiting for the
    /// next check.
    ShowPreview,
    /// Copy text to the clipboard.
    Yank(String),
}
