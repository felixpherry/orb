//! Shared application state: written by the [`IntentHandler`](crate::IntentHandler)
//! and the actors, read by the renderer.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::feat::harness::{HarnessId, HarnessInfo};
use crate::feat::jumps::state::JumpList;
use crate::feat::layout::state::{Layouts, SessionLayout};
use crate::feat::picker::state::PickerState;
use crate::feat::search::state::SearchProgress;
use crate::feat::sessions::state::{Session, Sessions, SidebarItem};
use crate::feat::sidebar::state::{Rename, SidebarView};
use crate::feat::worktrees::state::Worktrees;
use crate::ids::SessionId;

/// Which part of orb receives the user's keys. Ordered so it can key the
/// which-key scopes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Focus {
    /// Keys move through the sidebar's sessions.
    #[default]
    Sidebar,
    /// Keys go to the shown layout's focused pane.
    Pane,
    /// Keys edit the open picker's filter and move its selection.
    Picker,
    /// Keys edit the name in the rename box.
    Rename,
    /// Keys edit the sidebar's search and move between its matches.
    Search,
}

/// A sticky mode the keys can be in while they are in a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneMode {
    /// Keys push and pull the focused pane's borders.
    Resize,
    /// Keys swap the focused pane with another.
    Move,
}

/// Everything the frontend needs to draw a frame and decide whether to exit.
#[derive(Debug, Default)]
pub struct AppState {
    /// Set when the user or a signal asked orb to quit; the frontend loop exits when true.
    pub should_quit: bool,
    /// Where the user's keys currently go.
    pub focus: Focus,
    /// The pane mode the user turned on, if any; it only counts while the keys
    /// are in a pane (see [`AppState::pane_mode`]). Written by the intent
    /// handler.
    pub pane_mode: Option<PaneMode>,
    /// orb's projects and their sessions.
    pub sessions: Sessions,
    /// The sidebar's width, visibility and last layout.
    pub sidebar: SidebarView,
    /// The sessions orb holds pane clients for: added at start and on `⏎`,
    /// removed by settling, deleting, a workspace change, a failed spawn and
    /// the layout emptying.
    /// Every pane of a session in here has a client, and the right-hand area
    /// shows the active session while it is in here. Written by the intent
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
    /// The rows `<C-[>`/`<C-]>` move between. Written by the intent handler
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
    /// The pane mode in force: the one turned on, while the keys are in a pane.
    pub fn pane_mode(&self) -> Option<PaneMode> {
        self.pane_mode.filter(|_| self.focus == Focus::Pane)
    }

    /// The session the right-hand area shows: the active one, while it is
    /// attached and has a layout.
    pub fn shown_session(&self) -> Option<SessionId> {
        let id = self.sessions.active()?;
        (self.attached.contains(&id) && self.layouts.get(id).is_some()).then_some(id)
    }

    /// The row entering session `id` records: the agent row of its focused
    /// pane while that pane runs an agent, else its card.
    pub fn entry_row(&self, id: SessionId) -> SidebarItem {
        match self.layouts.get(id).and_then(SessionLayout::focused) {
            Some(pane) if self.sessions.runs_agent(id, pane, &self.layouts) => {
                SidebarItem::Agent { session: id, pane }
            }
            _ => SidebarItem::Session(id),
        }
    }

    /// The active marker's row: the active session's card while it's folded,
    /// else the agent row of its focused pane, or its card when that pane is
    /// a shell. Follows every focus change inside the session.
    pub fn active_row(&self) -> Option<SidebarItem> {
        let id = self.sessions.active()?;
        Some(if self.sessions.hides_agents(id) {
            SidebarItem::Session(id)
        } else {
            self.entry_row(id)
        })
    }

    /// The session `␣w` and `␣b` act on: the shown one while the keys are in
    /// a pane, else the one under the sidebar cursor.
    pub fn keyed_session(&self) -> Option<&Session> {
        match self.focus {
            Focus::Pane => self.sessions.session(self.shown_session()?),
            _ => self.sessions.selected_session(),
        }
    }

    /// The agent row of the shown session's focused pane, while it runs an
    /// agent.
    pub fn focused_agent(&self) -> Option<SidebarItem> {
        Some(self.entry_row(self.shown_session()?))
            .filter(|row| matches!(row, SidebarItem::Agent { .. }))
    }

    /// Where a jump starts: the focused agent while the keys are in a pane,
    /// else the sidebar cursor.
    pub fn jump_from(&self) -> Option<SidebarItem> {
        match self.focus {
            Focus::Pane => self.focused_agent().or(self.sessions.cursor),
            _ => self.sessions.cursor,
        }
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

    use super::{AppState, Focus, PaneMode};
    use crate::feat::harness::HarnessId;
    use crate::feat::layout::state::{PaneEntry, SessionLayout};
    use crate::feat::sessions::state::{
        PaneLaunch, Project, ProjectKind, Sessions, SidebarItem, Thread, ThreadStatus, sessions_for,
    };
    use crate::feat::zmx::zmx_service::ZmxSession;
    use crate::ids::{PaneId, ProjectId, SessionId, ThreadId};

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
            created_at: SystemTime::UNIX_EPOCH,
            last_activity_at: SystemTime::UNIX_EPOCH,
            unseen: false,
            model: None,
        }
    }

    /// Thread 1 selected, running in pane 10 of session 1; attached and
    /// active when `attached`.
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
                resume: None,
            }),
        );
        if attached {
            state.attached.insert(SessionId(1));
            state.sessions.enter(SessionId(1));
        }
        state
    }

    #[rstest::rstest]
    fn shown_layout_is_the_active_attached_threads_session() {
        // Given thread 1 attached and active, running in session 1.
        let state = selecting_thread(true);

        // When asking for the shown layout.
        let shown = state.shown_layout();

        // Then it is session 1's.
        assert!(
            shown.is_some() && shown == state.layouts.get(SessionId(1)),
            "the active attached thread's session is shown"
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
    fn pane_mode_is_none_outside_a_pane() {
        // Given resize mode turned on while the keys are in the sidebar.
        let state = AppState {
            focus: Focus::Sidebar,
            pane_mode: Some(PaneMode::Resize),
            ..AppState::default()
        };

        // When asking for the pane mode in force.
        let mode = state.pane_mode();

        // Then there is none.
        assert_eq!(mode, None, "a pane mode only counts in a pane");
    }
}
