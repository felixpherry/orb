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
use crate::feat::sessions::state::{SessionId, Sessions, Thread, ThreadId};
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
    /// The threads orb holds live panes for: added on attach, removed by
    /// `<C-\>`, settling, deleting, the pane's exit and a failed spawn. While
    /// one of a session's threads is in here, every pane of that session has
    /// a client, and the right-hand area shows the selected thread's session.
    /// Written by the intent handler, the frontend, and the sessions actor
    /// (which detaches a group it goes ahead deleting).
    pub attached: HashSet<ThreadId>,
    /// Every session's tabs and splits, saved in the store as they change
    /// (see [`Layouts`]).
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
    /// The session the right-hand area shows: the selected thread's, while
    /// that thread is attached.
    pub fn shown_session(&self) -> Option<SessionId> {
        let thread = self.sessions.selected_thread()?;
        if !self.attached.contains(&thread.id) {
            return None;
        }
        self.layouts.owner_of(thread.pane.as_ref()?.pane)
    }

    /// The layout the right-hand area shows: the shown session's. `None`
    /// means the start screen.
    pub fn shown_layout(&self) -> Option<&SessionLayout> {
        self.layouts.get(self.shown_session()?)
    }

    /// The threads running in session `session`'s panes (its agent panes'
    /// conversations), leaving out those being deleted.
    pub fn session_threads(&self, session: SessionId) -> Vec<&Thread> {
        self.sessions
            .threads()
            .filter(|thread| !self.sessions.deleting.contains(&thread.id))
            .filter(|thread| {
                thread
                    .pane
                    .as_ref()
                    .and_then(|pane| self.layouts.owner_of(pane.pane))
                    == Some(session)
            })
            .collect()
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
    use crate::feat::layout::state::{PaneEntry, SessionLayout};
    use crate::feat::sessions::state::{
        Draft, DraftWorkspace, Group, GroupDefaults, GroupDraft, GroupId, GroupKind, Own, PaneId,
        PaneLaunch, Project, ProjectId, ProjectKind, SessionId, Sessions, SidebarItem, Thread,
        ThreadId, ThreadStatus,
    };
    use crate::feat::zmx::zmx_service::ZmxSession;

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

    /// Idle thread `id`, running in pane `pane` when it has one.
    fn thread_in(id: i64, pane: Option<i64>) -> Thread {
        Thread {
            harness: HarnessId::new("claude"),
            id: ThreadId(id),
            title: None,
            cwd: "/tmp".into(),
            transcript: None,
            status: ThreadStatus::Idle,
            turn_started_at: None,
            pane: pane.map(|pane| PaneLaunch {
                pane: PaneId(pane),
                command: vec![],
            }),
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
        }
    }

    /// Thread 1 selected, running in pane 10 of session 1; attached when
    /// `attached`.
    fn selecting_thread(attached: bool) -> AppState {
        let mut state = drafting("claude");
        if let Some(project) = state.sessions.projects.first_mut() {
            project.threads = vec![thread_in(1, Some(10))];
        }
        state.sessions.cursor = Some(SidebarItem::Thread(ThreadId(1)));
        state.layouts.insert(
            SessionId(1),
            SessionLayout::of(PaneEntry {
                id: PaneId(10),
                zmx: ZmxSession {
                    name: "orb-p10".into(),
                    dir: "/tmp/zmx".into(),
                },
                cwd: "/tmp".into(),
                name: None,
            }),
        );
        if attached {
            state.attached.insert(ThreadId(1));
        }
        state
    }

    #[rstest::rstest]
    fn shown_layout_is_the_selected_attached_threads_session() {
        // Given selected thread 1 attached, running in session 1.
        let state = selecting_thread(true);

        // When asking for the shown layout.
        let shown = state.shown_layout();

        // Then it is session 1's.
        assert!(
            shown.is_some() && shown == state.layouts.get(SessionId(1)),
            "the selected attached thread's session is shown"
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

    #[rstest::rstest]
    fn session_threads_are_the_threads_in_its_panes() {
        // Given thread 1 in session 1's pane 10, thread 2 in session 2's
        // pane 20, and thread 3 with no pane.
        let mut state = selecting_thread(false);
        if let Some(project) = state.sessions.projects.first_mut() {
            project
                .threads
                .extend([thread_in(2, Some(20)), thread_in(3, None)]);
        }
        state.layouts.insert(
            SessionId(2),
            SessionLayout::of(PaneEntry {
                id: PaneId(20),
                zmx: ZmxSession {
                    name: "orb-p20".into(),
                    dir: "/tmp/zmx".into(),
                },
                cwd: "/tmp".into(),
                name: None,
            }),
        );

        // When listing session 1's threads.
        let threads: Vec<ThreadId> = state
            .session_threads(SessionId(1))
            .into_iter()
            .map(|thread| thread.id)
            .collect();

        // Then only thread 1 runs there.
        assert_eq!(
            threads,
            [ThreadId(1)],
            "a session's threads are those in its panes"
        );
    }
}
