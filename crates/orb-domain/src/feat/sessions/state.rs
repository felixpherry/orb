//! The sidebar's contents: projects, their threads and drafts, the sidebar's
//! order, and its cursor.

use std::cmp::Reverse;
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::SystemTime;

use fuzzy_matcher::skim::SkimMatcherV2;

use crate::TextInput;
use crate::feat::picker::list::fuzzy_match;
use crate::feat::sidebar::state::SidebarLayout;

/// What a draft is called, and a thread before its transcript names it.
pub const NEW_THREAD: &str = "New thread";

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

/// Why a thread needs the user, as a notification says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    /// A turn ended.
    Finished,
    /// Claude started waiting for an approval.
    NeedsApproval,
    /// Claude started waiting for an answer.
    NeedsInput,
}

impl NoticeKind {
    /// What the notification's body says.
    pub fn label(self) -> &'static str {
        match self {
            Self::Finished => "Finished",
            Self::NeedsApproval => "Needs approval",
            Self::NeedsInput => "Needs input",
        }
    }
}

/// A status change the frontend announces as a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub thread: ThreadId,
    pub kind: NoticeKind,
    /// The thread's project's title.
    pub project: String,
    /// The thread's title, else "New thread".
    pub title: String,
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

/// Where a draft's session will run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftWorkspace {
    /// The project's root.
    Local,
    /// A worktree orb creates when the draft starts.
    NewWorktree,
    /// A worktree that already exists.
    Existing(PathBuf),
}

/// The session setup picked for a project's next thread, before it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub workspace: DraftWorkspace,
    /// Local: the root's checked-out branch as last known. New worktree: the
    /// base it starts from (a ref name like `main` or `origin/x`). Existing:
    /// that worktree's branch.
    pub branch: Option<String>,
    /// The `--model`; `None` = Claude's default.
    pub model: Option<String>,
    /// The `--permission-mode`; `None` = Claude's default.
    pub permission: Option<String>,
    pub created_at: SystemTime,
    /// Whether the project's root is in a git repository, as the sessions
    /// actor last found. Not saved.
    pub repo: bool,
    /// New worktree: the ref it would start from as git last knew it,
    /// `origin/<base>` when origin has the base, else the base as is. `None`
    /// until the sessions actor has looked. Not saved.
    pub from: Option<String>,
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
    /// The project's draft; a project has at most one.
    pub draft: Option<Draft>,
    /// Removed from `␣n` and the project filter; its threads stay.
    pub removed: bool,
}

/// A row the sidebar's cursor can rest on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarItem {
    /// A project's draft.
    Draft(ProjectId),
    Thread(ThreadId),
    /// The Settled shelf's header.
    SettledShelf,
}

/// One sidebar row, in display order.
#[derive(Debug, Clone, Copy)]
pub enum SidebarRow<'a> {
    /// A project's draft, drawn as a card.
    Draft {
        project: &'a Project,
        draft: &'a Draft,
    },
    /// A pinned or active thread, drawn as a card.
    Card {
        project: &'a Project,
        thread: &'a Thread,
    },
    /// The Settled shelf's header.
    ShelfHeader {
        /// How many threads are settled.
        count: usize,
        /// Whether it reads as open: the shelf is, or a search lists its
        /// matches.
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
            Self::Draft { project, .. } => SidebarItem::Draft(project.id),
            Self::Card { thread, .. } | Self::Settled { thread, .. } => {
                SidebarItem::Thread(thread.id)
            }
            Self::ShelfHeader { .. } => SidebarItem::SettledShelf,
        }
    }
}

/// The sidebar search: the typed text, and where the cursor was before it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Search {
    pub input: TextInput,
    /// Where `Esc` puts the cursor back.
    pub return_to: Option<SidebarItem>,
}

/// orb's projects, threads and drafts, and where the sidebar's cursor is.
///
/// Written by the sessions actor (projects and their drafts, `error`,
/// `starting` when a start ends, `trust`, `attach`, the cursor and `filter`
/// after a restore, the cursor when a still-selected draft becomes a thread,
/// removing a thread from `deleting`, pushing `notices`) and by the intent
/// handler (the cursor on navigation, settle and delete, `shelf_open`,
/// `starting` when a start begins, a draft's fields when the user picks them,
/// adding a thread to `deleting`, `filter`). The frontend loop takes `attach`
/// and `notices`.
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
    /// The latest failure; shown until the next intent or a later success.
    pub error: Option<String>,
    /// A session start waits for the user to trust this directory in an
    /// interactive `claude`.
    pub trust: Option<PathBuf>,
    /// A started draft's thread for the frontend to attach to.
    pub attach: Option<ThreadId>,
    /// Threads hidden while their `claude rm` runs. The intent handler
    /// inserts; the sessions actor removes.
    pub deleting: HashSet<ThreadId>,
    /// The project the sidebar is filtered to; `None` = all projects.
    pub filter: Option<ProjectId>,
    /// Status changes for the frontend to announce; the sessions actor
    /// pushes, the frontend takes.
    pub notices: Vec<Notice>,
    /// The sidebar search while the user types one. Written only by the
    /// intent handler; the frontend clears it when it gives the keys to a
    /// pane.
    pub search: Option<Search>,
}

impl Sessions {
    /// The projects not removed, most recently active first: by their
    /// threads' latest activity, else when they were added. Ties go by title,
    /// then id.
    pub fn projects_by_recency(&self) -> Vec<&Project> {
        let mut projects: Vec<&Project> = self
            .projects
            .iter()
            .filter(|project| !project.removed)
            .collect();
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

    /// The sidebar's rows: drafts (newest first), pinned cards (newest pin
    /// first), active cards (newest created or un-settled first), then, if
    /// anything is settled, the shelf header and the settled rows (newest
    /// settle first). A collapsed shelf still lists the cursor's settled
    /// thread. Ties go to the higher id. Threads being deleted aren't listed.
    /// While a project filter is set, only that project's rows are. While a
    /// search has text, only the drafts and threads whose title matches it
    /// are, settled ones even while the shelf is closed, and the shelf header
    /// only when a settled thread matches, reading as open.
    pub fn sidebar(&self) -> Vec<SidebarRow<'_>> {
        let searching = self.searching();
        let mut drafts: Vec<_> = self
            .listed_projects()
            .filter_map(|project| project.draft.as_ref().map(|draft| (project, draft)))
            .filter(|_| self.title_matches(NEW_THREAD).is_some())
            .collect();
        drafts.sort_by_key(|(project, draft)| Reverse((draft.created_at, project.id.0)));
        let (mut settled, live): (Vec<_>, Vec<_>) = self
            .listed_projects()
            .flat_map(|project| project.threads.iter().map(move |thread| (project, thread)))
            .filter(|(_, thread)| !self.deleting.contains(&thread.id))
            .filter(|(_, thread)| {
                self.title_matches(thread.title.as_deref().unwrap_or(NEW_THREAD))
                    .is_some()
            })
            .partition(|(_, thread)| thread.settled_at.is_some());
        let (mut pinned, mut active): (Vec<_>, Vec<_>) = live
            .into_iter()
            .partition(|(_, thread)| thread.pinned_at.is_some());
        pinned.sort_by_key(|(_, thread)| Reverse((thread.pinned_at, thread.id.0)));
        active.sort_by_key(|(_, thread)| Reverse((thread.active_since, thread.id.0)));
        settled.sort_by_key(|(_, thread)| Reverse((thread.settled_at, thread.id.0)));
        let mut rows: Vec<SidebarRow<'_>> = drafts
            .into_iter()
            .map(|(project, draft)| SidebarRow::Draft { project, draft })
            .chain(
                pinned
                    .into_iter()
                    .chain(active)
                    .map(|(project, thread)| SidebarRow::Card { project, thread }),
            )
            .collect();
        if !settled.is_empty() {
            rows.push(SidebarRow::ShelfHeader {
                count: settled.len(),
                open: self.shelf_open || searching,
            });
            rows.extend(
                settled
                    .into_iter()
                    .filter(|(_, thread)| {
                        searching
                            || self.shelf_open
                            || self.cursor == Some(SidebarItem::Thread(thread.id))
                    })
                    .map(|(project, thread)| SidebarRow::Settled { project, thread }),
            );
        }
        rows
    }

    /// Where the search's terms matched `title`, as sorted byte offsets:
    /// empty without a search or with blank text, `None` when a term
    /// doesn't match.
    pub fn title_matches(&self, title: &str) -> Option<Vec<usize>> {
        let terms: Vec<&str> = self
            .search
            .as_ref()
            .map(|search| search.input.text().split_whitespace().collect())
            .unwrap_or_default();
        match terms.as_slice() {
            [] => Some(Vec::new()),
            terms => {
                fuzzy_match(&SkimMatcherV2::default(), title, terms).map(|(_, offsets)| offsets)
            }
        }
    }

    /// Move the cursor to the first draft or thread the search lists, or to
    /// nothing when none matches. Blank text leaves the cursor where it is.
    pub fn select_first_match(&mut self) {
        if self.searching() {
            self.cursor = self.matches().first().copied();
        }
    }

    /// Move the cursor to the next draft or thread the search lists, past
    /// the shelf header; wraps from the last one to the first. Without a
    /// cursor, or with one on a row that's gone, selects the first.
    pub fn select_next_match(&mut self) {
        let matches = self.matches();
        let next = match self.position(&matches) {
            None => matches.first(),
            Some(at) => matches.get((at + 1) % matches.len()),
        };
        if let Some(&next) = next {
            self.cursor = Some(next);
        }
    }

    /// Move the cursor to the previous draft or thread the search lists,
    /// past the shelf header; wraps from the first one to the last. Without a
    /// cursor, or with one on a row that's gone, selects the first.
    pub fn select_prev_match(&mut self) {
        let matches = self.matches();
        let prev = match self.position(&matches) {
            None => matches.first(),
            Some(at) => matches.get((at + matches.len() - 1) % matches.len()),
        };
        if let Some(&prev) = prev {
            self.cursor = Some(prev);
        }
    }

    /// End the search and put the cursor back where it was before it, or on
    /// the first row when that row is no longer listed.
    pub fn cancel_search(&mut self) {
        let Some(search) = self.search.take() else {
            return;
        };
        self.cursor = search.return_to;
        let items = self.items();
        if self.position(&items).is_none() {
            self.cursor = items.first().copied();
        }
    }

    /// The thread under the cursor, if it still exists.
    pub fn selected_thread(&self) -> Option<&Thread> {
        match self.cursor {
            Some(SidebarItem::Thread(id)) => self.threads().find(|thread| thread.id == id),
            _ => None,
        }
    }

    /// The draft under the cursor and its project, if it still exists.
    pub fn selected_draft(&self) -> Option<(&Project, &Draft)> {
        match self.cursor {
            Some(SidebarItem::Draft(id)) => self
                .projects
                .iter()
                .find(|project| project.id == id)
                .and_then(|project| project.draft.as_ref().map(|draft| (project, draft))),
            _ => None,
        }
    }

    /// Project `id`'s draft, if it has one.
    pub fn draft_mut(&mut self, id: ProjectId) -> Option<&mut Draft> {
        self.projects
            .iter_mut()
            .find(|project| project.id == id)
            .and_then(|project| project.draft.as_mut())
    }

    /// The project holding the thread under the cursor.
    pub fn selected_project(&self) -> Option<&Project> {
        let id = self.selected_id()?;
        self.projects
            .iter()
            .find(|project| project.threads.iter().any(|thread| thread.id == id))
    }

    /// The id of the thread under the cursor, if it still exists.
    pub fn selected_id(&self) -> Option<ThreadId> {
        self.selected_thread().map(|thread| thread.id)
    }

    /// Move the cursor to the row below; wraps from the last row to the first.
    /// Without a cursor, or with one on a row that's gone, selects the first
    /// row.
    pub fn select_next(&mut self) {
        let items = self.items();
        let next = match self.position(&items) {
            None => items.first(),
            Some(at) => items.get((at + 1) % items.len()),
        };
        if let Some(&next) = next {
            self.cursor = Some(next);
        }
    }

    /// Move the cursor to the row above; wraps from the first row to the last.
    /// Without a cursor, or with one on a row that's gone, selects the first
    /// row.
    pub fn select_prev(&mut self) {
        let items = self.items();
        let prev = match self.position(&items) {
            None => items.first(),
            Some(at) => items.get((at + items.len() - 1) % items.len()),
        };
        if let Some(&prev) = prev {
            self.cursor = Some(prev);
        }
    }

    /// Move the cursor to the first row.
    pub fn select_first(&mut self) {
        if let Some(&first) = self.items().first() {
            self.cursor = Some(first);
        }
    }

    /// Move the cursor to the last row.
    pub fn select_last(&mut self) {
        if let Some(&last) = self.items().last() {
            self.cursor = Some(last);
        }
    }

    /// Move the cursor down by as many rows as fit in half the sidebar's
    /// visible height, at least one; stays put on the last row. Without a
    /// cursor, or with one on a row that's gone, selects the first row.
    pub fn half_page_down(&mut self, layout: &SidebarLayout) {
        let items = self.items();
        let target = match self.position(&items) {
            None => items.first(),
            Some(at) => {
                let steps = half_page(layout, at + 1..items.len());
                items.get((at + steps).min(items.len() - 1))
            }
        };
        if let Some(&target) = target {
            self.cursor = Some(target);
        }
    }

    /// Move the cursor up by as many rows as fit in half the sidebar's
    /// visible height, at least one; stays put on the first row. Without a
    /// cursor, or with one on a row that's gone, selects the first row.
    pub fn half_page_up(&mut self, layout: &SidebarLayout) {
        let items = self.items();
        let target = match self.position(&items) {
            None => items.first(),
            Some(at) => items.get(at.saturating_sub(half_page(layout, (0..at).rev()))),
        };
        if let Some(&target) = target {
            self.cursor = Some(target);
        }
    }

    /// Where the cursor goes when thread `id` is settled: the next card below,
    /// else the nearest card above, else the shelf header the settle creates.
    pub fn card_neighbour(&self, id: ThreadId) -> Option<SidebarItem> {
        self.neighbour(SidebarItem::Thread(id), |row| {
            matches!(row, SidebarRow::Card { .. })
        })
        .or(Some(SidebarItem::SettledShelf))
    }

    /// Where the cursor goes when the draft or thread `item` is deleted: the
    /// next draft or thread row below, else the nearest one above, else the
    /// shelf header if another thread is settled.
    pub fn row_neighbour(&self, item: SidebarItem) -> Option<SidebarItem> {
        self.neighbour(item, |row| !matches!(row, SidebarRow::ShelfHeader { .. }))
            .or_else(|| {
                self.listed_projects()
                    .flat_map(|project| project.threads.iter())
                    .filter(|thread| !self.deleting.contains(&thread.id))
                    .any(|thread| {
                        SidebarItem::Thread(thread.id) != item && thread.settled_at.is_some()
                    })
                    .then_some(SidebarItem::SettledShelf)
            })
    }

    /// Filters the sidebar to `filter`'s project, or to all projects; a
    /// cursor on a row no longer listed moves to the first row.
    pub fn filter_to(&mut self, filter: Option<ProjectId>) {
        self.filter = filter;
        let items = self.items();
        if self.position(&items).is_none() {
            self.cursor = items.first().copied();
        }
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

    /// How many threads have `status`, not counting threads being deleted.
    pub fn status_count(&self, status: ThreadStatus) -> usize {
        self.shown_threads()
            .filter(|thread| thread.status == status)
            .count()
    }

    /// How many threads are running a turn right now, not counting threads
    /// being deleted.
    pub fn working_count(&self) -> usize {
        self.status_count(ThreadStatus::Working)
    }

    /// Whether something shows a spinner: a session is being started or a
    /// thread is working.
    pub fn spinning(&self) -> bool {
        self.starting || self.working_count() > 0
    }

    /// Whether any thread not being deleted has a turn underway.
    pub fn any_in_progress(&self) -> bool {
        self.shown_threads()
            .any(|thread| thread.status.in_progress())
    }

    /// The projects the filter lets the sidebar list.
    fn listed_projects(&self) -> impl Iterator<Item = &Project> {
        self.projects
            .iter()
            .filter(|project| self.filter.is_none_or(|filter| filter == project.id))
    }

    /// Every thread but those being deleted.
    fn shown_threads(&self) -> impl Iterator<Item = &Thread> {
        self.threads()
            .filter(|thread| !self.deleting.contains(&thread.id))
    }

    /// Whether a search with text is filtering the sidebar.
    fn searching(&self) -> bool {
        self.search
            .as_ref()
            .is_some_and(|search| !search.input.text().trim().is_empty())
    }

    /// The drafts and threads listed, in display order: every row but the
    /// shelf header.
    fn matches(&self) -> Vec<SidebarItem> {
        self.items()
            .into_iter()
            .filter(|&item| item != SidebarItem::SettledShelf)
            .collect()
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

    /// The first row after `item`'s that `fits`, else the nearest one before
    /// it.
    fn neighbour<F>(&self, item: SidebarItem, fits: F) -> Option<SidebarItem>
    where
        F: Fn(&SidebarRow<'_>) -> bool,
    {
        let rows = self.sidebar();
        let at = rows
            .iter()
            .position(|row| row.item() == item)
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

/// How many of the rows at `indices`, walked in order, fit in half of
/// `layout`'s visible height; at least one. A row the layout doesn't know
/// counts as one line.
fn half_page<I>(layout: &SidebarLayout, indices: I) -> usize
where
    I: Iterator<Item = usize>,
{
    let half = usize::from(layout.rows / 2).max(1);
    indices
        .scan(0, |lines, at| {
            *lines += layout
                .heights
                .get(at)
                .map_or(1, |&height| usize::from(height));
            Some(*lines)
        })
        .take_while(|&lines| lines <= half)
        .count()
        .max(1)
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
        Draft, DraftWorkspace, Project, ProjectId, Search, Sessions, SidebarItem, SidebarRow,
        Thread, ThreadId, ThreadStatus,
    };
    use crate::TextInput;
    use crate::feat::sidebar::state::SidebarLayout;

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
            removed: false,
            draft: None,
            threads,
        }
    }

    /// Project `id` with no threads and a local draft created at second `secs`.
    fn drafted(id: i64, secs: u64) -> Project {
        Project {
            draft: Some(Draft {
                workspace: DraftWorkspace::Local,
                branch: None,
                model: None,
                permission: None,
                created_at: at(secs),
                repo: true,
                from: None,
            }),
            ..project(id, vec![])
        }
    }

    fn on_draft(id: i64) -> SidebarItem {
        SidebarItem::Draft(ProjectId(id))
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

    /// Seven cards, listed 7 down to 1, with the cursor on thread `cursor`.
    fn seven_cards(cursor: i64) -> Sessions {
        sessions(
            (1..=7).map(|id| active(id, id.unsigned_abs())).collect(),
            Some(on(cursor)),
        )
    }

    /// A sidebar `rows` lines tall showing seven 4-line cards.
    fn cards_in(rows: u16) -> SidebarLayout {
        SidebarLayout {
            rows,
            heights: vec![4; 7],
        }
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
    fn select_next_on_a_collapsed_shelf_wraps_to_the_first_row() {
        // Given an active thread, a settled thread, and the cursor on the
        // collapsed shelf's header.
        let mut sessions = sessions(
            vec![active(1, 10), settled(2, 20)],
            Some(SidebarItem::SettledShelf),
        );

        // When selecting the next row.
        sessions.select_next();

        // Then the first row is selected.
        assert_eq!(
            sessions.cursor,
            Some(SidebarItem::Thread(ThreadId(1))),
            "the collapsed shelf's header is the last row, so next wraps"
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

    #[rstest::rstest]
    fn sidebar_lists_drafts_before_pinned_and_active() {
        // Given project 1 with a pinned and an active thread, and project 2
        // with a draft.
        let sessions = Sessions {
            projects: vec![
                project(1, vec![pinned(1, 10), active(2, 20)]),
                drafted(2, 5),
            ],
            ..Sessions::default()
        };

        // When listing the sidebar.
        let items = items(&sessions);

        // Then the draft comes first.
        assert_eq!(
            items,
            vec![on_draft(2), on(1), on(2)],
            "drafts should lead the sidebar"
        );
    }

    #[rstest::rstest]
    fn sidebar_orders_drafts_by_newest() {
        // Given drafts created at 10 s (project 1) and 20 s (project 2).
        let sessions = Sessions {
            projects: vec![drafted(1, 10), drafted(2, 20)],
            ..Sessions::default()
        };

        // When listing the sidebar.
        let items = items(&sessions);

        // Then the newer draft comes first.
        assert_eq!(
            items,
            vec![on_draft(2), on_draft(1)],
            "drafts should list the newest first"
        );
    }

    #[rstest::rstest]
    fn select_next_from_a_draft_reaches_the_first_pinned_card() {
        // Given a draft, selected, and pinned threads.
        let mut sessions = Sessions {
            projects: vec![
                drafted(1, 5),
                project(2, vec![pinned(1, 10), pinned(2, 20)]),
            ],
            cursor: Some(on_draft(1)),
            ..Sessions::default()
        };

        // When selecting the next row.
        sessions.select_next();

        // Then the newest pinned card is selected.
        assert_eq!(
            sessions.cursor,
            Some(on(2)),
            "next after the last draft should be the first pinned card"
        );
    }

    #[rstest::rstest]
    fn row_neighbour_of_a_draft_is_the_next_row() {
        // Given two drafts and an active thread.
        let sessions = Sessions {
            projects: vec![
                drafted(1, 20),
                drafted(2, 10),
                project(3, vec![active(1, 30)]),
            ],
            ..Sessions::default()
        };

        // When finding where the cursor goes when the newer draft is deleted.
        let neighbour = sessions.row_neighbour(on_draft(1));

        // Then it's the draft below.
        assert_eq!(
            neighbour,
            Some(on_draft(2)),
            "deleting a draft should move to the next row"
        );
    }

    #[rstest::rstest]
    fn selected_draft_is_the_cursors_draft() {
        // Given two drafts with the cursor on project 2's.
        let sessions = Sessions {
            projects: vec![drafted(1, 10), drafted(2, 20)],
            cursor: Some(on_draft(2)),
            ..Sessions::default()
        };

        // When reading the selected draft.
        let selected = sessions.selected_draft().map(|(project, _)| project.id);

        // Then it's project 2's.
        assert_eq!(
            selected,
            Some(ProjectId(2)),
            "the selected draft should be the cursor's"
        );
    }

    #[rstest::rstest]
    fn select_first_selects_the_top_row() {
        // Given a draft above two cards, with the cursor on the last card.
        let mut sessions = Sessions {
            projects: vec![
                drafted(1, 5),
                project(2, vec![active(1, 10), active(2, 20)]),
            ],
            cursor: Some(on(1)),
            ..Sessions::default()
        };

        // When selecting the first row.
        sessions.select_first();

        // Then the draft is selected.
        assert_eq!(
            sessions.cursor,
            Some(on_draft(1)),
            "the first row should be selected"
        );
    }

    #[rstest::rstest]
    fn select_last_on_a_collapsed_shelf_selects_its_header() {
        // Given an active thread, selected, and a collapsed shelf.
        let mut sessions = sessions(vec![active(1, 10), settled(2, 20)], Some(on(1)));

        // When selecting the last row.
        sessions.select_last();

        // Then the shelf's header is selected.
        assert_eq!(
            sessions.cursor,
            Some(SidebarItem::SettledShelf),
            "a collapsed shelf's header is the last row"
        );
    }

    #[rstest::rstest]
    #[case(10, 6)]
    #[case(40, 2)]
    fn half_page_down_moves_the_cards_that_fit_in_half_the_height(
        #[case] rows: u16,
        #[case] expected: i64,
    ) {
        // Given seven cards with the top one selected.
        let mut sessions = seven_cards(7);

        // When moving half a page down on a sidebar `rows` lines tall.
        sessions.half_page_down(&cards_in(rows));

        // Then the cursor moved by as many cards as fit in half of it.
        assert_eq!(sessions.cursor, Some(on(expected)), "half of {rows} lines");
    }

    #[rstest::rstest]
    #[case(10, 2)]
    #[case(40, 6)]
    fn half_page_up_moves_the_cards_that_fit_in_half_the_height(
        #[case] rows: u16,
        #[case] expected: i64,
    ) {
        // Given seven cards with the bottom one selected.
        let mut sessions = seven_cards(1);

        // When moving half a page up on a sidebar `rows` lines tall.
        sessions.half_page_up(&cards_in(rows));

        // Then the cursor moved by as many cards as fit in half of it.
        assert_eq!(sessions.cursor, Some(on(expected)), "half of {rows} lines");
    }

    #[rstest::rstest]
    fn half_page_down_on_the_last_row_stays() {
        // Given seven cards with the bottom one selected.
        let mut sessions = seven_cards(1);

        // When moving half a page down.
        sessions.half_page_down(&cards_in(40));

        // Then the cursor stays.
        assert_eq!(
            sessions.cursor,
            Some(on(1)),
            "there is nothing below the last row"
        );
    }

    #[rstest::rstest]
    fn half_page_up_on_the_first_row_stays() {
        // Given seven cards with the top one selected.
        let mut sessions = seven_cards(7);

        // When moving half a page up.
        sessions.half_page_up(&cards_in(40));

        // Then the cursor stays.
        assert_eq!(
            sessions.cursor,
            Some(on(7)),
            "there is nothing above the first row"
        );
    }

    #[rstest::rstest]
    fn half_page_down_moves_one_row_when_a_card_is_taller_than_half() {
        // Given seven cards with the top one selected, on a 4-line sidebar.
        let mut sessions = seven_cards(7);

        // When moving half a page down.
        sessions.half_page_down(&cards_in(4));

        // Then the cursor still moves one row.
        assert_eq!(
            sessions.cursor,
            Some(on(6)),
            "a half page should move at least one row"
        );
    }

    #[rstest::rstest]
    fn half_page_up_moves_one_row_when_a_card_is_taller_than_half() {
        // Given seven cards with the bottom one selected, on a 4-line sidebar.
        let mut sessions = seven_cards(1);

        // When moving half a page up.
        sessions.half_page_up(&cards_in(4));

        // Then the cursor still moves one row.
        assert_eq!(
            sessions.cursor,
            Some(on(2)),
            "a half page should move at least one row"
        );
    }

    #[rstest::rstest]
    fn half_page_down_counts_an_unknown_row_as_one_line() {
        // Given seven cards with the top one selected, and a 4-line layout
        // that knows no row heights.
        let mut sessions = seven_cards(7);
        let layout = SidebarLayout {
            rows: 4,
            heights: vec![],
        };

        // When moving half a page down.
        sessions.half_page_down(&layout);

        // Then the cursor moves two rows, one line each.
        assert_eq!(
            sessions.cursor,
            Some(on(5)),
            "a row without a height should count as one line"
        );
    }

    #[rstest::rstest]
    fn half_page_down_without_a_cursor_selects_the_first_row() {
        // Given seven cards and no cursor.
        let mut sessions = Sessions {
            cursor: None,
            ..seven_cards(7)
        };

        // When moving half a page down.
        sessions.half_page_down(&cards_in(40));

        // Then the first row is selected.
        assert_eq!(
            sessions.cursor,
            Some(on(7)),
            "without a cursor the first row should be selected"
        );
    }

    #[rstest::rstest]
    fn sidebar_skips_threads_being_deleted() {
        // Given two active threads, one being deleted.
        let sessions = Sessions {
            deleting: [ThreadId(1)].into(),
            ..sessions(vec![active(1, 10), active(2, 20)], None)
        };

        // When listing the sidebar.
        let items = items(&sessions);

        // Then only the other thread shows.
        assert_eq!(
            items,
            vec![on(2)],
            "a thread being deleted should be hidden"
        );
    }

    #[rstest::rstest]
    fn working_count_skips_threads_being_deleted() {
        // Given two working threads, one being deleted.
        let working = |id| Thread {
            status: ThreadStatus::Working,
            ..thread(id)
        };
        let sessions = Sessions {
            deleting: [ThreadId(1)].into(),
            ..sessions(vec![working(1), working(2)], None)
        };

        // When counting the working threads.
        let count = sessions.working_count();

        // Then only the other one counts.
        assert_eq!(
            count, 1,
            "a thread being deleted shouldn't count as running"
        );
    }

    #[rstest::rstest]
    fn status_count_skips_threads_being_deleted() {
        // Given two threads needing approval, one being deleted.
        let needs_approval = |id| Thread {
            status: ThreadStatus::NeedsApproval,
            ..thread(id)
        };
        let sessions = Sessions {
            deleting: [ThreadId(1)].into(),
            ..sessions(vec![needs_approval(1), needs_approval(2)], None)
        };

        // When counting the threads needing approval.
        let count = sessions.status_count(ThreadStatus::NeedsApproval);

        // Then only the other one counts.
        assert_eq!(
            count, 1,
            "a thread being deleted shouldn't count toward its status"
        );
    }

    #[rstest::rstest]
    fn spinning_while_a_session_starts() {
        // Given idle threads and a session being started.
        let sessions = Sessions {
            starting: true,
            ..sessions(vec![thread(1)], None)
        };

        // When asking whether anything spins.
        let spinning = sessions.spinning();

        // Then it spins.
        assert!(spinning, "a starting session should spin");
    }

    #[rstest::rstest]
    fn spinning_while_a_thread_works() {
        // Given a working thread and no session starting.
        let sessions = sessions(
            vec![Thread {
                status: ThreadStatus::Working,
                ..thread(1)
            }],
            None,
        );

        // When asking whether anything spins.
        let spinning = sessions.spinning();

        // Then it spins.
        assert!(spinning, "a working thread should spin");
    }

    #[rstest::rstest]
    fn not_spinning_when_idle() {
        // Given only idle threads and no session starting.
        let sessions = sessions(vec![thread(1), thread(2)], None);

        // When asking whether anything spins.
        let spinning = sessions.spinning();

        // Then nothing spins.
        assert!(!spinning, "idle threads with no start shouldn't spin");
    }

    #[rstest::rstest]
    fn any_in_progress_skips_threads_being_deleted() {
        // Given a working thread being deleted and an idle one.
        let sessions = Sessions {
            deleting: [ThreadId(1)].into(),
            ..sessions(
                vec![
                    Thread {
                        status: ThreadStatus::Working,
                        ..thread(1)
                    },
                    thread(2),
                ],
                None,
            )
        };

        // When asking whether a turn is underway.
        let busy = sessions.any_in_progress();

        // Then none is.
        assert!(!busy, "a thread being deleted shouldn't keep the poll fast");
    }

    #[rstest::rstest]
    fn projects_by_recency_skips_removed_projects() {
        // Given projects 1 and 2, with 2 removed.
        let sessions = Sessions {
            projects: vec![
                project(1, vec![]),
                Project {
                    removed: true,
                    ..project(2, vec![])
                },
            ],
            ..Sessions::default()
        };

        // When ordering the projects by recency.
        let ids = by_recency(&sessions);

        // Then only project 1 is listed.
        assert_eq!(ids, vec![1], "a removed project shouldn't be listed");
    }

    #[rstest::rstest]
    fn sidebar_lists_a_removed_projects_threads() {
        // Given project 1 removed, with an active and a settled thread, the shelf open.
        let sessions = Sessions {
            projects: vec![Project {
                removed: true,
                ..project(1, vec![active(1, 10), settled(2, 20)])
            }],
            shelf_open: true,
            ..Sessions::default()
        };

        // When listing the sidebar.
        let rows = items(&sessions);

        // Then both threads still show.
        assert_eq!(
            rows,
            vec![on(1), SidebarItem::SettledShelf, on(2)],
            "removing a project should keep its threads listed"
        );
    }

    /// Project 1 with a draft, card 1 and settled thread 2, and project 2
    /// with a draft, card 3 and settled thread 4, the shelf open, filtered to
    /// `filter`.
    fn two_projects(filter: Option<i64>) -> Sessions {
        let [first, second] = [(1, 1, 2), (2, 3, 4)].map(|(id, card, settled_id)| Project {
            threads: vec![thread(card), settled(settled_id, 10)],
            ..drafted(id, 0)
        });
        Sessions {
            projects: vec![first, second],
            shelf_open: true,
            filter: filter.map(ProjectId),
            ..Sessions::default()
        }
    }

    #[rstest::rstest]
    fn filtered_sidebar_lists_only_that_projects_rows() {
        // Given two projects, filtered to project 2.
        let sessions = two_projects(Some(2));

        // When listing the sidebar.
        let rows = items(&sessions);

        // Then only project 2's draft, card, shelf and settled thread show.
        assert_eq!(
            rows,
            vec![on_draft(2), on(3), SidebarItem::SettledShelf, on(4)],
            "the filter should keep only project 2's rows"
        );
    }

    #[rstest::rstest]
    fn unfiltered_sidebar_lists_every_project() {
        // Given two projects and no filter.
        let sessions = two_projects(None);

        // When listing the sidebar.
        let rows = items(&sessions);

        // Then both projects' rows show.
        assert_eq!(
            rows,
            vec![
                on_draft(2),
                on_draft(1),
                on(3),
                on(1),
                SidebarItem::SettledShelf,
                on(4),
                on(2),
            ],
            "no filter should list every project"
        );
    }

    #[rstest::rstest]
    fn filtered_shelf_counts_only_that_projects_settled_threads() {
        // Given two projects with a settled thread each, filtered to project 1.
        let sessions = two_projects(Some(1));

        // When listing the sidebar.
        let count = sessions.sidebar().iter().find_map(|row| match row {
            SidebarRow::ShelfHeader { count, .. } => Some(*count),
            _ => None,
        });

        // Then the shelf counts one thread.
        assert_eq!(count, Some(1), "the shelf should count project 1's only");
    }

    #[rstest::rstest]
    fn filter_to_moves_a_hidden_cursor_to_the_first_row() {
        // Given two projects and the cursor on project 1's card.
        let mut sessions = Sessions {
            cursor: Some(on(1)),
            ..two_projects(None)
        };

        // When filtering to project 2.
        sessions.filter_to(Some(ProjectId(2)));

        // Then the cursor is on project 2's draft, the first row.
        assert_eq!(
            sessions.cursor,
            Some(on_draft(2)),
            "a cursor the filter hides should move to the first row"
        );
    }

    #[rstest::rstest]
    fn filter_to_keeps_a_listed_cursor() {
        // Given two projects and the cursor on project 2's card.
        let mut sessions = Sessions {
            cursor: Some(on(3)),
            ..two_projects(None)
        };

        // When filtering to project 2.
        sessions.filter_to(Some(ProjectId(2)));

        // Then the cursor stays on the card.
        assert_eq!(
            sessions.cursor,
            Some(on(3)),
            "a cursor the filter keeps should stay"
        );
    }

    #[rstest::rstest]
    fn filter_to_an_empty_project_leaves_no_cursor() {
        // Given project 1 with a card, empty project 2, and the cursor on the
        // card.
        let mut sessions = Sessions {
            projects: vec![project(1, vec![thread(1)]), project(2, vec![])],
            cursor: Some(on(1)),
            ..Sessions::default()
        };

        // When filtering to project 2.
        sessions.filter_to(Some(ProjectId(2)));

        // Then nothing is selected.
        assert_eq!(sessions.cursor, None, "an empty sidebar has no cursor");
    }

    #[rstest::rstest]
    fn row_neighbour_skips_a_shelf_the_filter_hides() {
        // Given project 1 with only card 1, project 2 with only settled
        // thread 2, filtered to project 1.
        let sessions = Sessions {
            projects: vec![
                project(1, vec![thread(1)]),
                project(2, vec![settled(2, 10)]),
            ],
            filter: Some(ProjectId(1)),
            ..Sessions::default()
        };

        // When finding where the cursor goes after deleting card 1.
        let next = sessions.row_neighbour(on(1));

        // Then nothing is left to select.
        assert_eq!(
            next, None,
            "another project's settled thread shouldn't offer a hidden shelf"
        );
    }

    /// `thread`, titled `title`.
    fn titled(thread: Thread, title: &str) -> Thread {
        Thread {
            title: Some(title.to_owned()),
            ..thread
        }
    }

    /// `sessions` searching for `text`, from the cursor it has.
    fn searching(sessions: Sessions, text: &str) -> Sessions {
        Sessions {
            search: Some(Search {
                input: TextInput::new(text),
                return_to: sessions.cursor,
            }),
            ..sessions
        }
    }

    /// Active "fix login bug" (1), "add dark mode" (2) and "fix logout" (3),
    /// listed 3, 2, 1, and "fix lint" (4) settled, with the shelf closed.
    fn four_titles() -> Sessions {
        sessions(
            vec![
                titled(active(1, 10), "fix login bug"),
                titled(active(2, 20), "add dark mode"),
                titled(active(3, 30), "fix logout"),
                titled(settled(4, 40), "fix lint"),
            ],
            None,
        )
    }

    #[rstest::rstest]
    fn search_keeps_only_matching_titles_in_sidebar_order() {
        // Given three active threads, two titled "fix log…", and the shelf
        // empty of matches.
        let sessions = searching(
            sessions(
                vec![
                    titled(active(1, 10), "fix login bug"),
                    titled(active(2, 20), "add dark mode"),
                    titled(active(3, 30), "fix logout"),
                ],
                None,
            ),
            "fix log",
        );

        // When listing the sidebar.
        let rows = items(&sessions);

        // Then only the two matches show, newest first as usual.
        assert_eq!(
            rows,
            vec![on(3), on(1)],
            "the search should keep only matches, in sidebar order"
        );
    }

    #[rstest::rstest]
    fn search_lists_settled_matches_while_the_shelf_is_closed() {
        // Given a settled "fix lint" with the shelf closed.
        let sessions = searching(four_titles(), "lint");

        // When listing the sidebar.
        let rows = items(&sessions);

        // Then the shelf header and the settled match show.
        assert_eq!(
            rows,
            vec![SidebarItem::SettledShelf, on(4)],
            "a settled match should show with the shelf closed"
        );
    }

    #[rstest::rstest]
    fn search_counts_only_matching_settled_threads_on_the_shelf() {
        // Given two settled threads, one matching.
        let sessions = searching(
            sessions(
                vec![
                    titled(settled(1, 10), "fix lint"),
                    titled(settled(2, 20), "add dark mode"),
                ],
                None,
            ),
            "lint",
        );

        // When listing the sidebar.
        let count = sessions.sidebar().iter().find_map(|row| match row {
            SidebarRow::ShelfHeader { count, .. } => Some(*count),
            _ => None,
        });

        // Then the header counts only the match.
        assert_eq!(count, Some(1), "the header should count the matches");
    }

    #[rstest::rstest]
    #[case::searching("lint", true)]
    #[case::not_searching("", false)]
    fn shelf_header_reads_as_open_while_a_search_lists_settled_threads(
        #[case] text: &str,
        #[case] expected: bool,
    ) {
        // Given a closed shelf holding "fix lint", searching for `text`.
        let sessions = searching(
            sessions(vec![titled(settled(1, 10), "fix lint")], None),
            text,
        );

        // When listing the sidebar.
        let open = sessions.sidebar().iter().find_map(|row| match row {
            SidebarRow::ShelfHeader { open, .. } => Some(*open),
            _ => None,
        });

        // Then the header is open only while the search lists the match.
        assert_eq!(open, Some(expected), "the header searching for '{text}'");
    }

    #[rstest::rstest]
    fn search_without_a_settled_match_has_no_shelf_header() {
        // Given a settled "fix lint" and a search only an active thread
        // matches.
        let sessions = searching(four_titles(), "dark");

        // When listing the sidebar.
        let rows = items(&sessions);

        // Then only the active match shows.
        assert_eq!(rows, vec![on(2)], "no settled match, no header");
    }

    #[rstest::rstest]
    fn search_stays_inside_the_project_filter() {
        // Given two projects each with an untitled card, filtered to project
        // 2.
        let sessions = searching(two_projects(Some(2)), "new");

        // When listing the sidebar.
        let rows = items(&sessions);

        // Then only project 2's matching rows show.
        assert_eq!(
            rows,
            vec![on_draft(2), on(3), SidebarItem::SettledShelf, on(4)],
            "the search should stay inside the filter"
        );
    }

    #[rstest::rstest]
    #[case("")]
    #[case("  ")]
    fn blank_search_filters_nothing(#[case] text: &str) {
        // Given a search with blank text.
        let sessions = searching(four_titles(), text);

        // When listing the sidebar.
        let rows = items(&sessions);

        // Then every row shows as without a search.
        assert_eq!(rows, items(&four_titles()), "blank text shouldn't filter");
    }

    #[rstest::rstest]
    fn title_matches_returns_the_matched_byte_offsets() {
        // Given a search for "fl".
        let sessions = searching(Sessions::default(), "fl");

        // When matching "fix lint".
        let offsets = sessions.title_matches("fix lint");

        // Then the `f` and the `l` match.
        assert_eq!(offsets, Some(vec![0, 4]), "offsets of 'fl' in 'fix lint'");
    }

    #[rstest::rstest]
    fn select_first_match_without_a_match_clears_the_cursor() {
        // Given the cursor on thread 1 and a search nothing matches.
        let mut sessions = searching(
            Sessions {
                cursor: Some(on(1)),
                ..four_titles()
            },
            "zzz",
        );

        // When selecting the first match.
        sessions.select_first_match();

        // Then nothing is selected.
        assert_eq!(sessions.cursor, None, "no match, no cursor");
    }

    #[rstest::rstest]
    fn select_first_match_with_blank_text_keeps_the_cursor() {
        // Given the cursor on thread 1 and a blank search.
        let mut sessions = searching(
            Sessions {
                cursor: Some(on(1)),
                ..four_titles()
            },
            "",
        );

        // When selecting the first match.
        sessions.select_first_match();

        // Then the cursor stays.
        assert_eq!(sessions.cursor, Some(on(1)), "blank text shouldn't move it");
    }

    #[rstest::rstest]
    fn select_next_match_skips_the_shelf_header() {
        // Given "fix" matching cards 3 and 1 and settled 4, with the cursor on
        // card 1.
        let mut sessions = searching(
            Sessions {
                cursor: Some(on(1)),
                ..four_titles()
            },
            "fix",
        );

        // When selecting the next match.
        sessions.select_next_match();

        // Then it lands on the settled match, past the header.
        assert_eq!(sessions.cursor, Some(on(4)), "the header is skipped");
    }

    #[rstest::rstest]
    fn select_prev_match_skips_the_shelf_header() {
        // Given "fix" matching cards 3 and 1 and settled 4, with the cursor on
        // settled 4.
        let mut sessions = searching(
            Sessions {
                cursor: Some(on(4)),
                ..four_titles()
            },
            "fix",
        );

        // When selecting the previous match.
        sessions.select_prev_match();

        // Then it lands on card 1, past the header.
        assert_eq!(sessions.cursor, Some(on(1)), "the header is skipped");
    }

    #[rstest::rstest]
    fn select_next_match_wraps_from_the_last_match_to_the_first() {
        // Given "fix" matching cards 3 and 1 and settled 4, with the cursor on
        // settled 4, the last match.
        let mut sessions = searching(
            Sessions {
                cursor: Some(on(4)),
                ..four_titles()
            },
            "fix",
        );

        // When selecting the next match.
        sessions.select_next_match();

        // Then it wraps to card 3, the first match.
        assert_eq!(
            sessions.cursor,
            Some(on(3)),
            "next wraps to the first match"
        );
    }

    #[rstest::rstest]
    fn select_prev_match_wraps_from_the_first_match_to_the_last() {
        // Given "fix" matching cards 3 and 1 and settled 4, with the cursor on
        // card 3, the first match.
        let mut sessions = searching(
            Sessions {
                cursor: Some(on(3)),
                ..four_titles()
            },
            "fix",
        );

        // When selecting the previous match.
        sessions.select_prev_match();

        // Then it wraps to settled 4, the last match.
        assert_eq!(sessions.cursor, Some(on(4)), "prev wraps to the last match");
    }

    #[rstest::rstest]
    fn select_next_match_stays_on_a_single_match() {
        // Given "logout" matching only card 3, with the cursor on it.
        let mut sessions = searching(
            Sessions {
                cursor: Some(on(3)),
                ..four_titles()
            },
            "logout",
        );

        // When selecting the next match.
        sessions.select_next_match();

        // Then the cursor stays on card 3.
        assert_eq!(sessions.cursor, Some(on(3)), "a lone match stays put");
    }

    #[rstest::rstest]
    fn cancel_search_restores_the_cursor_from_before() {
        // Given a search begun on thread 2 that moved the cursor to thread 3.
        let mut sessions = Sessions {
            cursor: Some(on(3)),
            ..searching(
                Sessions {
                    cursor: Some(on(2)),
                    ..four_titles()
                },
                "logout",
            )
        };

        // When cancelling the search.
        sessions.cancel_search();

        // Then the cursor is back on thread 2.
        assert_eq!(sessions.cursor, Some(on(2)), "Esc restores the cursor");
    }

    #[rstest::rstest]
    fn cancel_search_falls_back_to_the_first_row_when_the_old_one_is_gone() {
        // Given a search begun on thread 9, which no longer exists.
        let mut sessions = searching(
            Sessions {
                cursor: Some(on(9)),
                ..four_titles()
            },
            "logout",
        );

        // When cancelling the search.
        sessions.cancel_search();

        // Then the cursor is on the first row.
        assert_eq!(sessions.cursor, Some(on(3)), "the first row");
    }
}
