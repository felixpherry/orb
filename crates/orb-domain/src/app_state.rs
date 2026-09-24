//! Shared application state: written by the [`IntentHandler`](crate::IntentHandler),
//! read by the renderer.

use std::ffi::OsString;

/// Which side receives the user's keys.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Focus {
    /// Keys drive orb itself.
    #[default]
    Normal,
    /// Keys go to the terminal pane's child.
    Attached,
}

/// Everything the frontend needs to draw a frame and decide whether to exit.
#[derive(Debug, Default)]
pub struct AppState {
    /// Set when the user asked to quit; the frontend loop exits when true.
    pub should_quit: bool,
    /// Where the user's keys currently go.
    pub focus: Focus,
    /// The command the terminal pane runs; empty when orb has no pane command.
    pub pane_argv: Vec<OsString>,
}
