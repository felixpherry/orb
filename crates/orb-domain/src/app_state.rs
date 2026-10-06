//! Shared application state: written by the [`IntentHandler`](crate::IntentHandler)
//! and the actors, read by the renderer.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::feat::dashboard::state::DashboardCursor;
use crate::feat::harness::{HarnessId, HarnessInfo};
use crate::feat::jumps::state::JumpList;
use crate::feat::layout::state::{Layouts, SessionLayout};
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
    /// Nothing sets it any more; the start screen takes no keys.
    Dashboard,
    /// Keys go to the shown layout's focused pane.
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
    /// (which detaches a group it goes ahead deleting). Each attached thread
    /// has a layout in `layouts`, dropped once it leaves.
    pub attached: HashSet<ThreadId>,
    /// Each attached thread's tabs and splits (see [`Layouts`]).
    pub layouts: Layouts,
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
    /// The layout the right-hand area shows: the selected thread's, while it
    /// is attached. `None` means the start screen.
    pub fn shown_layout(&self) -> Option<&SessionLayout> {
        self.layouts.get(self.sessions.selected_id()?)
    }

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
        Draft, DraftWorkspace, Group, GroupDefaults, GroupDraft, GroupId, GroupKind, Own, Project,
        ProjectId, ProjectKind, Sessions, SidebarItem, Thread, ThreadId, ThreadStatus,
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

    #[rstest::rstest]
    fn group_draft_overriding_a_harness_without_permission_modes_offers_none() {
        // Given a Claude group whose selected draft picked pi.
        let group = Group {
            id: GroupId(9),
            kind: GroupKind::Feature,
            name: "GT-514-login".into(),
            dir: None,
            branch: None,
            created_at: SystemTime::UNIX_EPOCH,
            pinned_at: None,
            settled_at: None,
            active_since: SystemTime::UNIX_EPOCH,
            draft: Some(GroupDraft {
                harness: Own::Set(HarnessId::new("pi")),
                ..GroupDraft::default()
            }),
            defaults: GroupDefaults {
                harness: HarnessId::new("claude"),
                model: None,
                permission: None,
            },
        };
        let mut state = drafting("claude");
        if let Some(project) = state.sessions.projects.first_mut() {
            project.groups = vec![group];
        }
        state.sessions.cursor = Some(SidebarItem::GroupDraft(GroupId(9)));

        // When asking whether the selection offers permission modes.
        let offers = state.offers_permissions();

        // Then it doesn't.
        assert!(
            !offers,
            "the draft's own pi harness lists no permission modes"
        );
    }

    /// Thread 1 selected; its layout open when `attached`.
    fn selecting_thread(attached: bool) -> AppState {
        let thread = Thread {
            harness: HarnessId::new("claude"),
            id: ThreadId(1),
            title: None,
            cwd: "/tmp".into(),
            transcript: None,
            status: ThreadStatus::Idle,
            turn_started_at: None,
            pane: None,
            branch: None,
            pinned_at: None,
            settled_at: None,
            active_since: SystemTime::UNIX_EPOCH,
            created_at: SystemTime::UNIX_EPOCH,
            last_activity_at: SystemTime::UNIX_EPOCH,
            unseen: false,
            group: None,
            model: None,
            permission: None,
        };
        let mut state = drafting("claude");
        if let Some(project) = state.sessions.projects.first_mut() {
            project.threads = vec![thread];
        }
        state.sessions.cursor = Some(SidebarItem::Thread(ThreadId(1)));
        if attached {
            state.attached.insert(ThreadId(1));
            state.layouts.open(ThreadId(1));
        }
        state
    }

    #[rstest::rstest]
    fn shown_layout_is_the_selected_attached_threads() {
        // Given selected thread 1 attached with its layout open.
        let state = selecting_thread(true);

        // When asking for the shown layout.
        let shown = state.shown_layout();

        // Then it is thread 1's.
        assert!(
            shown.is_some() && shown == state.layouts.get(ThreadId(1)),
            "the selected attached thread's layout is shown"
        );
    }

    #[rstest::rstest]
    fn shown_layout_is_none_for_an_unattached_thread() {
        // Given selected thread 1, not attached.
        let state = selecting_thread(false);

        // When asking for the shown layout.
        let shown = state.shown_layout();

        // Then nothing is shown.
        assert!(
            shown.is_none(),
            "an unattached thread shows the start screen"
        );
    }
}
