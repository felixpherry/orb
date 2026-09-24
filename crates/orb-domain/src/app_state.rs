//! Shared application state: written by the [`IntentHandler`](crate::IntentHandler),
//! read by the renderer.

/// Everything the frontend needs to draw a frame and decide whether to exit.
#[derive(Debug, Default)]
pub struct AppState {
    /// Set when the user asked to quit; the frontend loop exits when true.
    pub should_quit: bool,
}
