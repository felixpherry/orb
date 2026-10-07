//! Shared application state: written by the [`IntentHandler`](crate::IntentHandler)
//! and the actors, read by the renderer.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::feat::harness::{HarnessId, HarnessInfo};
use crate::feat::jumps::state::JumpList;
use crate::feat::layout::state::{Layouts, SessionLayout};
use crate::feat::picker::state::PickerState;
use crate::feat::search::state::SearchProgress;
use crate::feat::sessions::state::{SessionId, Sessions};
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
    /// The sidebar's width, visibility and last layout.
    pub sidebar: SidebarView,
    /// The sessions orb holds pane clients for: added on `⏎`, removed by
    /// `<C-\>`, settling, deleting, a failed spawn and the layout emptying.
    /// Every pane of a session in here has a client, and the right-hand area
    /// shows the selected session while it is in here. Written by the intent
    /// handler, the frontend, and the sessions actor (which drops a session it
    /// deletes).
    pub attached: HashSet<SessionId>,
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
    /// (recording jumps, moving through the list, dropping a deleted
    /// session) and by the sessions actor, its owner (restoring the saved
    /// list).
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
    /// The session the right-hand area shows: the selected one, while it is
    /// attached and has a layout.
    pub fn shown_session(&self) -> Option<SessionId> {
        let id = self.sessions.selected_session()?.id;
        (self.attached.contains(&id) && self.layouts.get(id).is_some()).then_some(id)
    }

    /// The layout the right-hand area shows: the shown session's. `None`
    /// means the start screen.
    pub fn shown_layout(&self) -> Option<&SessionLayout> {
        self.layouts.get(self.shown_session()?)
    }

    /// What the frontend knows about harness `id`.
    pub fn harness_info(&self, id: &HarnessId) -> Option<&HarnessInfo> {
        self.harnesses.iter().find(|info| info.id == *id)
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::AppState;
    use crate::feat::harness::HarnessId;
    use crate::feat::layout::state::{PaneEntry, SessionLayout};
    use crate::feat::sessions::state::{
        PaneId, PaneLaunch, Project, ProjectId, ProjectKind, SessionId, Sessions, SidebarItem,
        Thread, ThreadId, ThreadStatus, sessions_for,
    };
    use crate::feat::zmx::zmx_service::ZmxSession;

    /// One project.
    fn one_project() -> AppState {
        AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "work".into(),
                    root: "/work".into(),
                    created_at: SystemTime::UNIX_EPOCH,
                    removed: false,
                    repo: true,
                    threads: vec![],
                    kind: ProjectKind::Normal,
                }],
                ..Sessions::default()
            },
            ..AppState::default()
        }
    }

    /// Idle thread `id`, running in pane `pane` of session 1 when it has one.
    fn thread_in(id: i64, pane: Option<i64>) -> Thread {
        Thread {
            last_session: None,
            harness: HarnessId::new("claude"),
            id: ThreadId(id),
            title: None,
            cwd: "/tmp".into(),
            transcript: None,
            status: ThreadStatus::Idle,
            turn_started_at: None,
            pane: pane.map(|pane| PaneLaunch {
                pane: PaneId(pane),
                session: SessionId(1),
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
        let mut state = one_project();
        if let Some(project) = state.sessions.projects.first_mut() {
            project.threads = vec![thread_in(1, Some(10))];
        }
        state.sessions.cursor = Some(SidebarItem::Session(SessionId(1)));
        state.sessions.sessions = sessions_for(&state.sessions.projects);
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
            state.attached.insert(SessionId(1));
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
}
