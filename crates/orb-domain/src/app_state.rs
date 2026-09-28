//! Shared application state: written by the [`IntentHandler`](crate::IntentHandler)
//! and the actors, read by the renderer.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::feat::dashboard::state::DashboardCursor;
use crate::feat::picker::state::PickerState;
use crate::feat::sessions::state::{Sessions, ThreadId};
use crate::feat::sidebar::state::{Rename, SidebarView};

/// Which part of orb receives the user's keys. Ordered so it can key the
/// which-key scopes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Focus {
    /// Keys move through the sidebar's threads.
    #[default]
    Sidebar,
    /// Keys act on the dashboard on the right.
    Dashboard,
    /// Keys go to the attached Claude session.
    Attached,
    /// Keys edit the open picker's filter and move its selection.
    Picker,
    /// Keys edit the name in the rename box.
    Rename,
    /// Keys edit the sidebar's search and move between its matches.
    Search,
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
    /// The dashboard's highlighted menu item.
    pub dashboard: DashboardCursor,
    /// The sidebar's width, visibility and last layout.
    pub sidebar: SidebarView,
    /// The threads orb holds a live Claude pane for: added on attach, removed by
    /// `<C-\>`, settling, deleting, the pane's exit and a failed spawn. The
    /// right-hand area shows the selected thread's pane while it's in here.
    /// Written by the intent handler, the frontend, and the sessions actor
    /// (which detaches a group it goes ahead deleting).
    pub attached: HashSet<ThreadId>,
    /// The open picker, if any.
    pub picker: Option<PickerState>,
    /// The open rename box, if any. Written by the intent handler, and
    /// cleared by the frontend when it gives the keys to a pane.
    pub rename: Option<Rename>,
    /// The user's home directory; what the directory picker's `~/` means.
    pub home: PathBuf,
}
