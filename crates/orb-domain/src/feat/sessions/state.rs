//! The sidebar's contents: projects, their threads, the sidebar's order, and
//! its cursor.

use std::cmp::Reverse;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::SystemTime;

/// Identifies a thread across launches (its row in orb's store).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThreadId(pub i64);

/// Identifies a project across launches (its row in orb's store).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectId(pub i64);

/// What a thread's Claude session is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThreadStatus {
    /// Not polled yet since orb started.
    Unknown,
    /// Claude is running a turn.
    Working,
    /// Claude waits for the user to approve something.
    NeedsApproval,
    /// Claude waits for the user to answer.
    NeedsInput,
    /// The session is idle, ready for a prompt.
    Idle,
    /// The session failed.
    Failed,
    /// The session stopped.
    Stopped,
    /// Claude no longer knows the session.
    Gone,
}

impl ThreadStatus {
    /// Whether a turn is underway: running, or paused waiting on the user.
    pub fn in_progress(self) -> bool {
        matches!(self, Self::Working | Self::NeedsApproval | Self::NeedsInput)
    }
}

/// One Claude session orb started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    pub id: ThreadId,
    /// `None` until the transcript names the thread.
    pub title: Option<String>,
    /// Where the session runs.
    pub cwd: PathBuf,
    /// The session's Claude transcript, once located.
    pub transcript: Option<PathBuf>,
    pub status: ThreadStatus,
    /// When orb first saw the current turn running; `None` between turns.
    pub turn_started_at: Option<SystemTime>,
    /// The command that attaches to the session.
    pub attach_argv: Vec<OsString>,
    /// The git branch the transcript last named.
    pub branch: Option<String>,
    /// When the thread was pinned; `None` = not pinned.
    pub pinned_at: Option<SystemTime>,
    /// When the thread was settled; `None` = not settled.
    pub settled_at: Option<SystemTime>,
    /// Sorts Active: the later of its creation and its latest un-settle.
    pub active_since: SystemTime,
    /// When orb last saw a turn end, else when the thread was created.
    pub last_activity_at: SystemTime,
    /// A turn ended after the user last selected the thread.
    pub unseen: bool,
}

/// A directory the user starts sessions in.
#[derive(Debug, Clone)]
pub struct Project {
    pub id: ProjectId,
    pub title: String,
    pub root: PathBuf,
    /// When the project was added.
    pub created_at: SystemTime,
    /// Newest first.
    pub threads: Vec<Thread>,
}

/// A row the sidebar's cursor can rest on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarItem {
    Thread(ThreadId),
    /// The Settled shelf's header.
    SettledShelf,
}

/// One sidebar row, in display order.
#[derive(Debug, Clone, Copy)]
pub enum SidebarRow<'a> {
    /// A pinned or active thread, drawn as a card.
    Card {
        project: &'a Project,
        thread: &'a Thread,
    },
    /// The Settled shelf's header.
    ShelfHeader {
        /// How many threads are settled.
        count: usize,
        open: bool,
    },
    /// A settled thread, drawn as a one-line row.
    Settled {
        project: &'a Project,
        thread: &'a Thread,
    },
}

impl SidebarRow<'_> {
    /// What the cursor rests on when it is on this row.
    pub fn item(&self) -> SidebarItem {
        match self {
            Self::Card { thread, .. } | Self::Settled { thread, .. } => {
                SidebarItem::Thread(thread.id)
            }
            Self::ShelfHeader { .. } => SidebarItem::SettledShelf,
        }
    }
}

/// orb's projects and threads, and where the sidebar's cursor is.
///
/// Written by the sessions actor (projects, `error`, `starting` when a create
/// ends, the cursor after a restore or a create) and by the intent handler
/// (the cursor on navigation, settle and delete, `shelf_open`, `starting` when
/// a create begins).
#[derive(Debug, Clone, Default)]
pub struct Sessions {
    /// In the order orb first used them.
    pub projects: Vec<Project>,
    /// What the sidebar's cursor is on.
    pub cursor: Option<SidebarItem>,
    /// The Settled shelf shows its threads. Written only by the intent handler.
    pub shelf_open: bool,
    /// A new session is being created.
    pub starting: bool,
    /// The latest `claude` failure; cleared by the next success.
    pub error: Option<String>,
}

impl Sessions {
    /// The projects, most recently active first: by their threads' latest
    /// activity, else when they were added. Ties go by title, then id.
    pub fn projects_by_recency(&self) -> Vec<&Project> {
        let mut projects: Vec<&Project> = self.projects.iter().collect();
        projects.sort_by_key(|&project| {
            let latest = project
                .threads
                .iter()
                .map(|thread| thread.last_activity_at)
                .max()
                .unwrap_or(project.created_at);
            (Reverse(latest), &project.title, project.id.0)
        });
        projects
    }

    /// Every thread in sidebar order: project by project, newest first.
    pub fn threads(&self) -> impl Iterator<Item = &Thread> {
        self.projects
            .iter()
            .flat_map(|project| project.threads.iter())
    }

    /// The sidebar's rows: pinned cards (newest pin first), active cards
    /// (newest created or un-settled first), then, if anything is settled, the
    /// shelf header and the settled rows (newest settle first). A collapsed
    /// shelf still lists the cursor's settled thread. Ties go to the higher id.
    pub fn sidebar(&self) -> Vec<SidebarRow<'_>> {
        let (mut settled, live): (Vec<_>, Vec<_>) = self
            .projects
            .iter()
            .flat_map(|project| project.threads.iter().map(move |thread| (project, thread)))
            .partition(|(_, thread)| thread.settled_at.is_some());
        let (mut pinned, mut active): (Vec<_>, Vec<_>) = live
            .into_iter()
            .partition(|(_, thread)| thread.pinned_at.is_some());
        pinned.sort_by_key(|(_, thread)| Reverse((thread.pinned_at, thread.id.0)));
        active.sort_by_key(|(_, thread)| Reverse((thread.active_since, thread.id.0)));
        settled.sort_by_key(|(_, thread)| Reverse((thread.settled_at, thread.id.0)));
        let mut rows: Vec<SidebarRow<'_>> = pinned
            .into_iter()
            .chain(active)
            .map(|(project, thread)| SidebarRow::Card { project, thread })
            .collect();
        if !settled.is_empty() {
            rows.push(SidebarRow::ShelfHeader {
                count: settled.len(),
                open: self.shelf_open,
            });
            rows.extend(
                settled
                    .into_iter()
                    .filter(|(_, thread)| {
                        self.shelf_open || self.cursor == Some(SidebarItem::Thread(thread.id))
                    })
                    .map(|(project, thread)| SidebarRow::Settled { project, thread }),
            );
        }
        rows
    }

    /// The thread under the cursor, if it still exists.
    pub fn selected_thread(&self) -> Option<&Thread> {
        match self.cursor {
            Some(SidebarItem::Thread(id)) => self.threads().find(|thread| thread.id == id),
            _ => None,
        }
    }

    /// The id of the thread under the cursor, if it still exists.
    pub fn selected_id(&self) -> Option<ThreadId> {
        self.selected_thread().map(|thread| thread.id)
    }

    /// Move the cursor to the row below; stays put on the last row. Without a
    /// cursor, or with one on a row that's gone, selects the first row.
    pub fn select_next(&mut self) {
        let items = self.items();
        let next = match self.position(&items) {
            None => items.first(),
            Some(at) => items.get(at + 1),
        };
        if let Some(&next) = next {
            self.cursor = Some(next);
        }
    }

    /// Move the cursor to the row above; stays put on the first row. Without a
    /// cursor, or with one on a row that's gone, selects the first row.
    pub fn select_prev(&mut self) {
        let items = self.items();
        let prev = match self.position(&items) {
            None => items.first(),
            Some(at) => at.checked_sub(1).and_then(|at| items.get(at)),
        };
        if let Some(&prev) = prev {
            self.cursor = Some(prev);
        }
    }

    /// Where the cursor goes when thread `id` is settled: the next card below,
    /// else the nearest card above, else the shelf header the settle creates.
    pub fn card_neighbour(&self, id: ThreadId) -> Option<SidebarItem> {
        self.neighbour(id, |row| matches!(row, SidebarRow::Card { .. }))
            .or(Some(SidebarItem::SettledShelf))
    }

    /// Where the cursor goes when thread `id` is deleted: the next thread row
    /// below, else the nearest one above, else the shelf header if another
    /// thread is settled.
    pub fn row_neighbour(&self, id: ThreadId) -> Option<SidebarItem> {
        self.neighbour(id, |row| !matches!(row, SidebarRow::ShelfHeader { .. }))
            .or_else(|| {
                self.threads()
                    .any(|thread| thread.id != id && thread.settled_at.is_some())
                    .then_some(SidebarItem::SettledShelf)
            })
    }

    /// Show the Settled shelf's threads.
    pub fn open_shelf(&mut self) {
        self.shelf_open = true;
    }

    /// Hide the Settled shelf's threads and put the cursor on its header.
    pub fn close_shelf(&mut self) {
        self.shelf_open = false;
        self.cursor = Some(SidebarItem::SettledShelf);
    }

    /// How many threads are running a turn right now.
    pub fn working_count(&self) -> usize {
        self.threads()
            .filter(|thread| thread.status == ThreadStatus::Working)
            .count()
    }

    /// Whether any thread has a turn underway.
    pub fn any_in_progress(&self) -> bool {
        self.threads().any(|thread| thread.status.in_progress())
    }

    /// What each sidebar row's cursor rests on, in display order.
    fn items(&self) -> Vec<SidebarItem> {
        self.sidebar().iter().map(SidebarRow::item).collect()
    }

    /// Where the cursor is in `items`, if it's on one of them.
    fn position(&self, items: &[SidebarItem]) -> Option<usize> {
        let cursor = self.cursor?;
        items.iter().position(|&item| item == cursor)
    }

    /// The first row after thread `id` that `fits`, else the nearest one
    /// before it.
    fn neighbour<F>(&self, id: ThreadId, fits: F) -> Option<SidebarItem>
    where
        F: Fn(&SidebarRow<'_>) -> bool,
    {
        let rows = self.sidebar();
        let at = rows
            .iter()
            .position(|row| row.item() == SidebarItem::Thread(id))
            .unwrap_or(rows.len());
        let (before, after) = rows.split_at(at);
        after
            .iter()
            .skip(1)
            .find(|row| fits(row))
            .or_else(|| before.iter().rev().find(|row| fits(row)))
            .map(SidebarRow::item)
    }
}

/// What the frontend needs to attach to a thread's session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachTarget {
    pub thread: ThreadId,
    pub argv: Vec<OsString>,
    pub cwd: PathBuf,
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::{
        Project, ProjectId, Sessions, SidebarItem, SidebarRow, Thread, ThreadId, ThreadStatus,
    };

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn thread(id: i64) -> Thread {
        Thread {
            id: ThreadId(id),
            title: None,
            cwd: "/tmp".into(),
            transcript: None,
            status: ThreadStatus::Idle,
            turn_started_at: None,
            attach_argv: vec![],
            branch: None,
            pinned_at: None,
            settled_at: None,
            active_since: SystemTime::UNIX_EPOCH,
            last_activity_at: SystemTime::UNIX_EPOCH,
            unseen: false,
        }
    }

    /// Thread `id`, active since second `secs`.
    fn active(id: i64, secs: u64) -> Thread {
        Thread {
            active_since: at(secs),
            ..thread(id)
        }
    }

    /// Thread `id`, pinned at second `secs`.
    fn pinned(id: i64, secs: u64) -> Thread {
        Thread {
            pinned_at: Some(at(secs)),
            ..thread(id)
        }
    }

    /// Thread `id`, settled at second `secs`.
    fn settled(id: i64, secs: u64) -> Thread {
        Thread {
            settled_at: Some(at(secs)),
            ..thread(id)
        }
    }

    /// Thread `id`, last active at second `secs`.
    fn last_active(id: i64, secs: u64) -> Thread {
        Thread {
            last_activity_at: at(secs),
            ..thread(id)
        }
    }

    /// The ids of `sessions`' projects, most recently active first.
    fn by_recency(sessions: &Sessions) -> Vec<i64> {
        sessions
            .projects_by_recency()
            .iter()
            .map(|project| project.id.0)
            .collect()
    }

    fn project(id: i64, threads: Vec<Thread>) -> Project {
        Project {
            id: ProjectId(id),
            title: format!("project-{id}"),
            root: "/tmp".into(),
            created_at: SystemTime::UNIX_EPOCH,
            threads,
        }
    }

    /// One project holding `threads`, with the cursor on `cursor`.
    fn sessions(threads: Vec<Thread>, cursor: Option<SidebarItem>) -> Sessions {
        Sessions {
            projects: vec![project(1, threads)],
            cursor,
            ..Sessions::default()
        }
    }

    /// What each sidebar row's cursor rests on, in display order.
    fn items(sessions: &Sessions) -> Vec<SidebarItem> {
        sessions.sidebar().iter().map(SidebarRow::item).collect()
    }

    fn on(id: i64) -> SidebarItem {
        SidebarItem::Thread(ThreadId(id))
    }

    #[rstest::rstest]
    fn projects_by_recency_puts_newer_thread_activity_first() {
        // Given project 1 last active at 10 s and project 2 at 20 s.
        let sessions = Sessions {
            projects: vec![
                project(1, vec![last_active(1, 10)]),
                project(2, vec![last_active(2, 20)]),
            ],
            ..Sessions::default()
        };

        // When ordering the projects by recency.
        let ids = by_recency(&sessions);

        // Then the more recently active project comes first.
        assert_eq!(ids, vec![2, 1], "newer thread activity should come first");
    }

    #[rstest::rstest]
    fn projects_by_recency_uses_created_at_without_threads() {
        // Given project 1 last active at 10 s, and project 2 with no threads,
        // added at 20 s.
        let sessions = Sessions {
            projects: vec![
                project(1, vec![last_active(1, 10)]),
                Project {
                    created_at: at(20),
                    ..project(2, vec![])
                },
            ],
            ..Sessions::default()
        };

        // When ordering the projects by recency.
        let ids = by_recency(&sessions);

        // Then the project added later comes first.
        assert_eq!(
            ids,
            vec![2, 1],
            "a project without threads should rank by when it was added"
        );
    }

    #[rstest::rstest]
    fn projects_by_recency_breaks_ties_by_title() {
        // Given projects zeta (1) and alpha (2), equally recent.
        let sessions = Sessions {
            projects: vec![
                Project {
                    title: "zeta".into(),
                    ..project(1, vec![])
                },
                Project {
                    title: "alpha".into(),
                    ..project(2, vec![])
                },
            ],
            ..Sessions::default()
        };

        // When ordering the projects by recency.
        let ids = by_recency(&sessions);

        // Then alpha comes first.
        assert_eq!(
            ids,
            vec![2, 1],
            "equally recent projects should sort by title"
        );
    }

    #[rstest::rstest]
    fn sidebar_orders_pinned_by_newest_pin() {
        // Given threads 1 and 2 pinned at 10 s and 20 s, and thread 3 active
        // since 30 s.
        let sessions = sessions(vec![pinned(1, 10), pinned(2, 20), active(3, 30)], None);

        // When listing the sidebar.
        let items = items(&sessions);

        // Then the newest pin comes first, and the pins come before the active
        // thread.
        assert_eq!(
            items,
            vec![on(2), on(1), on(3)],
            "pinned cards should lead, newest pin first"
        );
    }

    #[rstest::rstest]
    fn sidebar_orders_active_by_newest_created_or_unsettled() {
        // Given old thread 1 un-settled at 50 s, and newer thread 2 created at
        // 20 s.
        let sessions = sessions(vec![active(1, 50), active(2, 20)], None);

        // When listing the sidebar.
        let items = items(&sessions);

        // Then the un-settled thread comes first.
        assert_eq!(
            items,
            vec![on(1), on(2)],
            "active cards should list the newest created or un-settled first"
        );
    }

    #[rstest::rstest]
    fn sidebar_orders_settled_by_newest_settle() {
        // Given threads settled at 10 s, 30 s and 20 s, with the shelf open.
        let sessions = Sessions {
            shelf_open: true,
            ..sessions(vec![settled(1, 10), settled(2, 30), settled(3, 20)], None)
        };

        // When listing the sidebar.
        let items = items(&sessions);

        // Then the header leads the settled rows, newest settle first.
        assert_eq!(
            items,
            vec![SidebarItem::SettledShelf, on(2), on(3), on(1)],
            "settled rows should list the newest settle first"
        );
    }

    #[rstest::rstest]
    fn collapsed_shelf_hides_settled_threads() {
        // Given two settled threads and the shelf collapsed.
        let sessions = sessions(vec![settled(1, 10), settled(2, 20)], None);

        // When listing the sidebar.
        let items = items(&sessions);

        // Then only the header shows.
        assert_eq!(
            items,
            vec![SidebarItem::SettledShelf],
            "a collapsed shelf should show only its header"
        );
    }

    #[rstest::rstest]
    fn collapsed_shelf_keeps_the_selected_settled_thread() {
        // Given two settled threads, the shelf collapsed, and the cursor on
        // thread 1.
        let sessions = sessions(vec![settled(1, 10), settled(2, 20)], Some(on(1)));

        // When listing the sidebar.
        let items = items(&sessions);

        // Then the header and the selected thread show.
        assert_eq!(
            items,
            vec![SidebarItem::SettledShelf, on(1)],
            "a collapsed shelf should keep the cursor's settled thread"
        );
    }

    #[rstest::rstest]
    fn sidebar_has_no_header_without_settled_threads() {
        // Given two active threads.
        let sessions = sessions(vec![active(1, 10), active(2, 20)], None);

        // When listing the sidebar.
        let items = items(&sessions);

        // Then only their cards show.
        assert_eq!(
            items,
            vec![on(2), on(1)],
            "without settled threads there should be no shelf header"
        );
    }

    #[rstest::rstest]
    fn select_next_from_the_last_card_selects_the_shelf() {
        // Given an active thread 1, selected, and a settled thread 2.
        let mut sessions = sessions(vec![active(1, 10), settled(2, 20)], Some(on(1)));

        // When selecting the next row.
        sessions.select_next();

        // Then the shelf header is selected.
        assert_eq!(
            sessions.cursor,
            Some(SidebarItem::SettledShelf),
            "next after the last card should be the shelf header"
        );
    }

    #[rstest::rstest]
    fn select_next_on_a_collapsed_shelf_stays() {
        // Given an active thread, a settled thread, and the cursor on the
        // collapsed shelf's header.
        let mut sessions = sessions(
            vec![active(1, 10), settled(2, 20)],
            Some(SidebarItem::SettledShelf),
        );

        // When selecting the next row.
        sessions.select_next();

        // Then the header stays selected.
        assert_eq!(
            sessions.cursor,
            Some(SidebarItem::SettledShelf),
            "the collapsed shelf's header is the last row"
        );
    }

    #[rstest::rstest]
    fn select_next_lands_on_a_thread_across_projects() {
        // Given project 1 with threads active since 30 s and 10 s, project 2
        // with one active since 20 s, and the newest selected.
        let mut sessions = Sessions {
            projects: vec![
                project(1, vec![active(3, 30), active(1, 10)]),
                project(2, vec![active(2, 20)]),
            ],
            cursor: Some(on(3)),
            ..Sessions::default()
        };

        // When selecting the next row.
        sessions.select_next();

        // Then project 2's thread is selected, not project 1's older one.
        assert_eq!(
            sessions.cursor,
            Some(on(2)),
            "the sidebar should be one list across projects"
        );
    }
}
