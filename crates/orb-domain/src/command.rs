//! Commands: work the [`IntentHandler`](crate::IntentHandler) asks the frontend
//! loop to carry out after it has updated [`AppState`](crate::AppState).

/// Something the frontend loop must do in response to an intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Start (or refocus) the terminal pane and send it input.
    Attach,
    /// Stop sending input to the terminal pane.
    Detach,
}
