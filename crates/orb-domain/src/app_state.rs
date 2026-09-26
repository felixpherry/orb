//! Shared application state: written by the [`IntentHandler`](crate::IntentHandler)
//! and the actors, read by the renderer.

use std::path::PathBuf;

use crate::feat::picker::state::PickerState;
use crate::feat::preview::state::Preview;
use crate::feat::sessions::state::Sessions;

/// Which part of orb receives the user's keys. Ordered so it can key the
/// which-key scopes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Focus {
    /// Keys move through the sidebar's threads.
    #[default]
    Sidebar,
    /// Keys act on the selected thread's preview on the right.
    Preview,
    /// Keys go to the attached Claude session.
    Attached,
    /// Keys edit the open picker's filter and move its selection.
    Picker,
}

/// Everything the frontend needs to draw a frame and decide whether to exit.
#[derive(Debug, Default)]
pub struct AppState {
    /// Set when the user asked to quit; the frontend loop exits when true.
    pub should_quit: bool,
    /// Where the user's keys currently go.
    pub focus: Focus,
    /// orb's projects and their Claude sessions.
    pub sessions: Sessions,
    /// The selected thread's transcript preview.
    pub preview: Preview,
    /// The open picker, if any.
    pub picker: Option<PickerState>,
    /// The user's home directory; what the directory picker's `~/` means.
    pub home: PathBuf,
}
