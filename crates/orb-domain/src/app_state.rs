//! Shared application state: written by the [`IntentHandler`](crate::IntentHandler)
//! and the actors, read by the renderer.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::feat::dashboard::state::DashboardCursor;
use crate::feat::harness::{HarnessId, HarnessInfo};
use crate::feat::jumps::state::JumpList;
use crate::feat::picker::state::PickerState;
use crate::feat::search::state::SearchProgress;
use crate::feat::sessions::state::{Sessions, ThreadId};
use crate::feat::sidebar::state::{Rename, SidebarView};
use crate::feat::worktrees::state::Worktrees;

/// Which part of orb receives the user's keys. Ordered so it can key the
/// which-key scopes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Focus {
    /// Keys move through the sidebar's threads.
    #[default]
    Sidebar,
    /// Keys act on the dashboard on the right.
    Dashboard,
    /// Keys go to the attached session.
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
    /// orb's projects and their sessions.
    pub sessions: Sessions,
    /// The dashboard's highlighted menu item.
    pub dashboard: DashboardCursor,
    /// The sidebar's width, visibility and last layout.
    pub sidebar: SidebarView,
    /// The threads orb holds a live pane for: added on attach, removed by
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
    /// The rows `<C-o>`/`<C-i>` move between. Written by the intent handler
    /// (recording jumps, moving through the list, dropping a deleted thread
    /// or discarded draft) and by the sessions actor, its owner (restoring the
    /// saved list, and dropping a deleted group's rows).
    pub jumps: JumpList,
    /// orb's worktrees and the sweep's notice. Written by the worktrees actor,
    /// its owner; the intent handler also clears `notice` on the next intent.
    pub worktrees: Worktrees,
    /// The search index's startup progress and failure. Written by the search
    /// actor, its owner.
    pub search: SearchProgress,
    /// Every registered harness as the pickers and keymap see it, in
    /// registration order. Written by the sessions actor, its owner.
    pub harnesses: Vec<HarnessInfo>,
}

impl AppState {
    /// What the frontend knows about harness `id`.
    pub fn harness_info(&self, id: &HarnessId) -> Option<&HarnessInfo> {
        self.harnesses.iter().find(|info| info.id == *id)
    }

    /// Whether the selected draft's or group's harness lists permission
    /// modes, so `␣a` has something to pick.
    pub fn offers_permissions(&self) -> bool {
        self.sessions
            .setting_harness()
            .and_then(|id| self.harness_info(id))
            .is_some_and(|info| !info.permission_modes.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::AppState;
    use crate::feat::harness::HarnessId;
    use crate::feat::harness::claude::models::info;
    use crate::feat::harness::fake::pi_like;
    use crate::feat::sessions::state::{
        Draft, DraftWorkspace, Project, ProjectId, ProjectKind, Sessions, SidebarItem,
    };

    /// One project whose draft runs `harness`, selected, with Claude and a
    /// pi-like harness registered.
    fn drafting(harness: &str) -> AppState {
        let draft = Draft {
            workspace: DraftWorkspace::Local,
            branch: None,
            model: None,
            permission: None,
            created_at: SystemTime::UNIX_EPOCH,
            repo: true,
            from: None,
            harness: HarnessId::new(harness),
        };
        AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "work".into(),
                    root: "/work".into(),
                    created_at: SystemTime::UNIX_EPOCH,
                    removed: false,
                    draft: Some(draft),
                    threads: vec![],
                    groups: vec![],
                    kind: ProjectKind::Normal,
                }],
                cursor: Some(SidebarItem::Draft(ProjectId(1))),
                ..Sessions::default()
            },
            harnesses: vec![info(), pi_like()],
            ..AppState::default()
        }
    }

    #[rstest::rstest]
    fn draft_of_a_harness_without_permission_modes_offers_none() {
        // Given a selected pi draft.
        let state = drafting("pi");

        // When asking whether the selection offers permission modes.
        let offers = state.offers_permissions();

        // Then it doesn't.
        assert!(!offers, "pi lists no permission modes");
    }

    #[rstest::rstest]
    fn draft_of_a_harness_with_permission_modes_offers_them() {
        // Given a selected Claude draft.
        let state = drafting("claude");

        // When asking whether the selection offers permission modes.
        let offers = state.offers_permissions();

        // Then it does.
        assert!(offers, "Claude lists permission modes");
    }
}
