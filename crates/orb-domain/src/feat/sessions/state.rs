//! The sidebar's contents: projects, their sessions and threads, the
//! sidebar's order, and its cursor.

use std::cmp::Reverse;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use fuzzy_matcher::skim::SkimMatcherV2;

use crate::TextInput;
use crate::feat::harness::HarnessId;
use crate::feat::layout::state::Layouts;
use crate::feat::picker::list::fuzzy_match;
use crate::feat::sidebar::state::SidebarLayout;
use crate::ids::{PaneId, ProjectId, SessionId, ThreadId};

/// What a thread is called before its transcript names it.
pub const NEW_THREAD: &str = "New thread";

/// The pane a thread runs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneLaunch {
    pub pane: PaneId,
    /// The session the pane belongs to.
    pub session: SessionId,
}

/// What a session is for, which decides its look and keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    /// Work in a project's checkout or worktree.
    Plain,
    /// A research folder of orb's own.
    Research,
    /// A learning folder of orb's own.
    Learn,
    /// A chat outside any project, in orb's Incognito folder.
    Incognito,
}

/// One directory holding tabs of panes, some of them running agents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: SessionId,
    pub project: ProjectId,
    pub kind: SessionKind,
    /// Where its panes start.
    pub dir: PathBuf,
    /// The name the user gave with `r`; `None` = not renamed.
    pub name: Option<String>,
    /// The branch the store last recorded.
    pub branch: Option<String>,
    pub created_at: SystemTime,
    /// When the session was pinned; `None` = not pinned.
    pub pinned_at: Option<SystemTime>,
    /// When the session was settled; `None` = not settled.
    pub settled_at: Option<SystemTime>,
    /// Sorts Active: the later of its creation and its latest un-settle.
    pub active_since: SystemTime,
    /// When a turn last ended in one of its agent panes.
    pub last_activity_at: SystemTime,
}

/// Which of orb's own folders a session lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderKind {
    /// `~/.orb/research/<name>/`.
    Research,
    /// `~/.orb/learn/<name>/`.
    Learn,
}

/// A folder's slug: `name`'s words joined by `-`, case kept (`GT-514 login`
/// → `GT-514-login`).
#[must_use]
pub fn folder_slug(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join("-")
}

/// Whether a project is one the user added, or one of orb's own folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectKind {
    /// A directory the user added.
    Normal,
    /// orb's folder that holds Research sessions' folders.
    Research,
    /// orb's folder that holds Learn sessions' folders.
    Learn,
    /// orb's folder for chats outside any project (`/tmp/orb-incognito`).
    Incognito,
}

/// What a thread's session is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThreadStatus {
    /// Not polled yet since orb started.
    Unknown,
    /// The session is running a turn.
    Working,
    /// The session waits for the user to approve something.
    NeedsApproval,
    /// The session waits for the user to answer.
    NeedsInput,
    /// The session is idle, ready for a prompt.
    Idle,
    /// The session failed.
    Failed,
    /// The session stopped.
    Stopped,
    /// Its harness no longer knows the session.
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
    /// The session started waiting for an approval.
    NeedsApproval,
    /// The session started waiting for an answer.
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

/// One session orb started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    pub id: ThreadId,
    /// `None` until the transcript names the thread.
    pub title: Option<String>,
    /// Where the session runs.
    pub cwd: PathBuf,
    /// The session's transcript, once located.
    pub transcript: Option<PathBuf>,
    pub status: ThreadStatus,
    /// When orb first saw the current turn running; `None` between turns.
    pub turn_started_at: Option<SystemTime>,
    /// How its pane runs; `None` for a harness orb doesn't know.
    pub pane: Option<PaneLaunch>,
    /// The session it runs in, or ran in before its pane ended; `None` for
    /// a thread that never ran in one.
    pub last_session: Option<SessionId>,
    /// The git branch the transcript last named.
    pub branch: Option<String>,
    /// When the thread was created (its store row's `created_at`).
    pub created_at: SystemTime,
    /// When orb last saw a turn end, else when the thread was created.
    pub last_activity_at: SystemTime,
    /// A turn ended after the user last selected the thread.
    pub unseen: bool,
    /// The `--model` a session from before the overhaul started with;
    /// `None` = unknown.
    pub model: Option<String>,
    /// The harness its session runs in.
    pub harness: HarnessId,
}

impl Thread {
    /// When the user last chatted in the thread: the later of the current
    /// turn's start and the last turn's end.
    pub fn last_chat(&self) -> SystemTime {
        self.turn_started_at
            .unwrap_or(UNIX_EPOCH)
            .max(self.last_activity_at)
    }

    /// The session whose pane it runs in; `None` once it has ended.
    pub fn session(&self) -> Option<SessionId> {
        self.pane.as_ref().map(|pane| pane.session)
    }

    /// The session it runs in, else the one it last ran in.
    pub fn home(&self) -> Option<SessionId> {
        self.session().or(self.last_session)
    }

    /// How urgently the thread needs the user, lowest first: an approval,
    /// an answer, a failure or lost session, a running turn, a turn done
    /// since the user last looked, then idle, then stopped.
    pub fn urgency(&self) -> u8 {
        match self.status {
            ThreadStatus::NeedsApproval => 0,
            ThreadStatus::NeedsInput => 1,
            ThreadStatus::Failed | ThreadStatus::Gone => 2,
            ThreadStatus::Working => 3,
            ThreadStatus::Idle if self.unseen => 4,
            ThreadStatus::Idle | ThreadStatus::Unknown => 5,
            ThreadStatus::Stopped => 6,
        }
    }
}

/// The thread among `threads` that most needs the user (see
/// [`Thread::urgency`]); a session's status is its.
pub fn most_urgent<'a, I>(threads: I) -> Option<&'a Thread>
where
    I: IntoIterator<Item = &'a Thread>,
{
    threads.into_iter().min_by_key(|thread| thread.urgency())
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
    /// Whether its root is in a git repository, as the sessions actor last
    /// found; never for orb's own projects. Not saved.
    pub repo: bool,
    /// Removed from `<C-g> n` and the project filter; its sessions stay.
    pub removed: bool,
    pub kind: ProjectKind,
}

/// A row the sidebar's cursor can rest on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarItem {
    /// A session's card, or its line on the Settled shelf.
    Session(SessionId),
    /// The Settled shelf's header.
    SettledShelf,
    /// An agent's line under its session's card, keyed by the agent's pane.
    Agent { session: SessionId, pane: PaneId },
}

/// One sidebar row, in display order.
#[derive(Debug, Clone, Copy)]
pub enum SidebarRow<'a> {
    /// A pinned or active session's three-line card.
    Card {
        project: &'a Project,
        session: &'a Session,
    },
    /// The Settled shelf's header.
    ShelfHeader {
        /// How many sessions are settled.
        count: usize,
        /// Whether it reads as open: the shelf is, or a search lists its
        /// matches.
        open: bool,
    },
    /// A settled session, drawn as a one-line row.
    Settled {
        project: &'a Project,
        session: &'a Session,
    },
    /// An agent pane's line under its session's card; `last` is the card's
    /// last agent.
    Agent {
        session: &'a Session,
        thread: &'a Thread,
        pane: PaneId,
        last: bool,
    },
}

impl SidebarRow<'_> {
    /// What the cursor rests on when it is on this row.
    pub fn item(&self) -> SidebarItem {
        match self {
            Self::Card { session, .. } | Self::Settled { session, .. } => {
                SidebarItem::Session(session.id)
            }
            Self::ShelfHeader { .. } => SidebarItem::SettledShelf,
            Self::Agent { session, pane, .. } => SidebarItem::Agent {
                session: session.id,
                pane: *pane,
            },
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

/// orb's projects, sessions and threads, and where the sidebar's cursor is.
///
/// Written by the sessions actor (projects, `sessions`, `starting` when a
/// start ends, `fetching`, the cursor and `filter` after a restore, the
/// cursor on a new session, removing a session from `deleting`) and by the
/// intent handler (the cursor on navigation, settle and delete, `active`,
/// `shelf_open`, `folded`, `starting` when a start begins, adding a session
/// to `deleting`, `filter`, `error`). The frontend loop moves `error` into
/// the message box.
#[derive(Debug, Clone, Default)]
pub struct Sessions {
    /// In the order orb first used them.
    pub projects: Vec<Project>,
    /// Every session, in the order the store made them. Written only by the
    /// sessions actor.
    pub sessions: Vec<Session>,
    /// What the sidebar's cursor is on.
    pub cursor: Option<SidebarItem>,
    /// The session the right-hand area shows, set by entering a row; `None`
    /// before any is entered and after the active session is settled or
    /// deleted. Written only by the intent handler; read it through
    /// [`Sessions::active`].
    pub active: Option<SessionId>,
    /// The Settled shelf shows its sessions. Written only by the intent
    /// handler.
    pub shelf_open: bool,
    /// A new session is being created, or a session's worktree recreated.
    pub starting: bool,
    /// Some saved layouts couldn't be read at start. For the rest of the run
    /// orb kills no pane program, deletes no pane and prunes no worktree it
    /// can't account for. Written only by the sessions actor, at start.
    pub partial_load: bool,
    /// The origin ref a start is fetching, like `origin/main`; none when no
    /// fetch is running. Written only by the sessions actor.
    pub fetching: Option<String>,
    /// Why the intent just handled was refused, for the message box.
    /// Written only by the intent handler (it clears it first on every
    /// intent) and the frontend loop, which moves it into the message box
    /// after each turn. Workers report their errors instead.
    pub error: Option<String>,
    /// Sessions hidden while they are removed. The intent handler inserts,
    /// the sessions actor removes.
    pub deleting: HashSet<SessionId>,
    /// The project the sidebar is filtered to; `None` = all projects.
    pub filter: Option<ProjectId>,
    /// The sidebar search while the user types one. Written only by the
    /// intent handler; the frontend clears it when it gives the keys to a
    /// pane.
    pub search: Option<Search>,
    /// Sessions whose cards hide their agent rows. Written only by the
    /// intent handler; empty at start and never saved.
    pub folded: HashSet<SessionId>,
}

impl Sessions {
    /// The projects the user added and didn't remove, most recently active
    /// first: by their threads' latest activity, else when they were added.
    /// Ties go by title, then id. orb's own projects (Research, Learn,
    /// Incognito) aren't listed.
    pub fn projects_by_recency(&self) -> Vec<&Project> {
        let mut projects: Vec<&Project> = self
            .projects
            .iter()
            .filter(|project| !project.removed && project.kind == ProjectKind::Normal)
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

    /// orb's own project of `kind` (Research, Learn or Incognito), unless it
    /// was removed.
    pub fn own_project(&self, kind: ProjectKind) -> Option<&Project> {
        self.projects
            .iter()
            .find(|project| project.kind == kind && !project.removed)
    }

    /// Session `id`, if it still exists.
    pub fn session(&self, id: SessionId) -> Option<&Session> {
        self.sessions.iter().find(|session| session.id == id)
    }

    /// Project `id`, if it still exists.
    pub fn project(&self, id: ProjectId) -> Option<&Project> {
        self.projects.iter().find(|project| project.id == id)
    }

    /// The threads running in session `id`'s panes (its agent panes'
    /// conversations), oldest first; none while it is being deleted.
    pub fn agents(&self, id: SessionId) -> Vec<&Thread> {
        if self.deleting.contains(&id) {
            return Vec::new();
        }
        let mut agents: Vec<&Thread> = self
            .threads()
            .filter(|thread| thread.session() == Some(id))
            .collect();
        agents.sort_by_key(|thread| (thread.created_at, thread.id.0));
        agents
    }

    /// What `session` is called: its `r` name, else the first of its agents
    /// a transcript named, else its directory's name.
    pub fn title(&self, session: &Session) -> String {
        session
            .name
            .clone()
            .or_else(|| {
                self.agents(session.id)
                    .into_iter()
                    .find_map(|thread| thread.title.clone())
            })
            .unwrap_or_else(|| {
                session.dir.file_name().map_or_else(
                    || session.dir.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                )
            })
    }

    /// Whether an agent turn has run in session `id`: one of its threads,
    /// ended ones included, has a title (its first prompt or an agent's
    /// title) or a turn underway.
    pub fn turned(&self, id: SessionId) -> bool {
        self.threads().any(|thread| {
            thread.home() == Some(id) && (thread.title.is_some() || thread.status.in_progress())
        })
    }

    /// Session `id`'s agent rows: its agents whose pane a tab or float of
    /// its layout holds, with the pane.
    pub fn agent_rows(&self, id: SessionId, layouts: &Layouts) -> Vec<(&Thread, PaneId)> {
        self.agents(id)
            .into_iter()
            .filter_map(|thread| Some((thread, thread.pane.as_ref()?.pane)))
            .filter(|&(_, pane)| layouts.owner_of(pane) == Some(id))
            .collect()
    }

    /// Whether pane `pane` still runs one of session `id`'s agents (never
    /// while the session is being deleted).
    pub fn runs_agent(&self, id: SessionId, pane: PaneId, layouts: &Layouts) -> bool {
        self.agent_rows(id, layouts)
            .iter()
            .any(|&(_, shown)| shown == pane)
    }

    /// Whether session `id`'s card hides its agent rows: it's folded and no
    /// search has text, since a search lists a folded card as if open.
    pub fn hides_agents(&self, id: SessionId) -> bool {
        self.folded.contains(&id) && !self.searching()
    }

    /// Whether `thread` runs in a session being deleted.
    pub fn is_deleting(&self, thread: &Thread) -> bool {
        thread
            .session()
            .is_some_and(|session| self.deleting.contains(&session))
    }

    /// Every thread in sidebar order: project by project, newest first.
    pub fn threads(&self) -> impl Iterator<Item = &Thread> {
        self.projects
            .iter()
            .flat_map(|project| project.threads.iter())
    }

    /// The sidebar's rows: pinned sessions (newest pin first), active sessions (newest created or
    /// un-settled first), each card followed by one row per agent pane unless
    /// it's folded, then, if any is settled, the shelf header and the
    /// settled sessions (newest settle first). A collapsed shelf still lists
    /// the cursor's settled session. Ties go to the higher id. Sessions being
    /// deleted aren't listed. While a project filter is set, only that
    /// project's rows are. While a search has text, only the sessions whose
    /// title, or one of whose agent rows' names, matches it are,
    /// settled ones even while the shelf is closed, and the shelf header only
    /// when a settled session matches, reading as open; a folded card lists
    /// its agent rows then too.
    pub fn sidebar(&self, layouts: &Layouts) -> Vec<SidebarRow<'_>> {
        let (mut settled, live): (Vec<_>, Vec<_>) = self
            .listed_sessions(layouts)
            .into_iter()
            .partition(|(_, session)| session.settled_at.is_some());
        let (mut pinned, mut active): (Vec<_>, Vec<_>) = live
            .into_iter()
            .partition(|(_, session)| session.pinned_at.is_some());
        pinned.sort_by_key(|(_, session)| Reverse((session.pinned_at, session.id.0)));
        active.sort_by_key(|(_, session)| Reverse((session.active_since, session.id.0)));
        settled.sort_by_key(|(_, session)| Reverse((session.settled_at, session.id.0)));
        let mut rows: Vec<SidebarRow<'_>> = Vec::new();
        for (project, session) in pinned.into_iter().chain(active) {
            rows.push(SidebarRow::Card { project, session });
            if self.hides_agents(session.id) {
                continue;
            }
            let agents = self.agent_rows(session.id, layouts);
            let count = agents.len();
            rows.extend(agents.into_iter().enumerate().map(|(at, (thread, pane))| {
                SidebarRow::Agent {
                    session,
                    thread,
                    pane,
                    last: at + 1 == count,
                }
            }));
        }
        if !settled.is_empty() {
            let open = self.shelf_open || self.searching();
            rows.push(SidebarRow::ShelfHeader {
                count: settled.len(),
                open,
            });
            rows.extend(
                settled
                    .into_iter()
                    .filter(|(_, session)| {
                        open || self.cursor == Some(SidebarItem::Session(session.id))
                    })
                    .map(|(project, session)| SidebarRow::Settled { project, session }),
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

    /// Move the cursor to the first row the search lists, or to
    /// nothing when none matches. Blank text leaves the cursor where it is.
    pub fn select_first_match(&mut self, layouts: &Layouts) {
        if self.searching() {
            self.cursor = self.matches(layouts).first().copied();
        }
    }

    /// Move the cursor to the next row the search lists, agent rows
    /// included, past the shelf header; wraps from the last one to the first. Without a
    /// cursor, or with one on a row that's gone, selects the first.
    pub fn select_next_match(&mut self, layouts: &Layouts) {
        let matches = self.matches(layouts);
        let next = match self.position(&matches, layouts) {
            None => matches.first(),
            Some(at) => matches.get((at + 1) % matches.len()),
        };
        if let Some(&next) = next {
            self.cursor = Some(next);
        }
    }

    /// Move the cursor to the previous row the search lists, agent
    /// rows included, past the shelf header; wraps from the first one to the last. Without
    /// a cursor, or with one on a row that's gone, selects the first.
    pub fn select_prev_match(&mut self, layouts: &Layouts) {
        let matches = self.matches(layouts);
        let prev = match self.position(&matches, layouts) {
            None => matches.first(),
            Some(at) => matches.get((at + matches.len() - 1) % matches.len()),
        };
        if let Some(&prev) = prev {
            self.cursor = Some(prev);
        }
    }

    /// End the search and put the cursor back where it was before it, or on
    /// the first row when that row is no longer listed.
    pub fn cancel_search(&mut self, layouts: &Layouts) {
        let Some(search) = self.search.take() else {
            return;
        };
        self.cursor = search.return_to;
        let items = self.items(layouts);
        if self.position(&items, layouts).is_none() {
            self.cursor = items.first().copied();
        }
    }

    /// Makes session `id` the one the right-hand area shows.
    pub fn enter(&mut self, id: SessionId) {
        self.active = Some(id);
    }

    /// Leaves session `id`: nothing is active any more if it was.
    pub fn leave(&mut self, id: SessionId) {
        if self.active == Some(id) {
            self.active = None;
        }
    }

    /// The active session, while it still exists and isn't being deleted.
    pub fn active(&self) -> Option<SessionId> {
        self.active
            .filter(|id| !self.deleting.contains(id) && self.session(*id).is_some())
    }

    /// The session under the cursor, or whose agent row it's on, if it still
    /// exists and isn't being deleted.
    pub fn selected_session(&self) -> Option<&Session> {
        match self.cursor? {
            SidebarItem::Session(id) | SidebarItem::Agent { session: id, .. }
                if !self.deleting.contains(&id) =>
            {
                self.session(id)
            }
            _ => None,
        }
    }

    /// The agent row under the cursor, as its session and pane, while that
    /// pane still runs one of the session's agents.
    pub fn selected_agent(&self, layouts: &Layouts) -> Option<(SessionId, PaneId)> {
        match self.cursor? {
            SidebarItem::Agent { session, pane } if self.runs_agent(session, pane, layouts) => {
                Some((session, pane))
            }
            _ => None,
        }
    }

    /// The row the cursor reads as: its own, or its session's card once the
    /// agent row it was on is gone.
    pub fn cursor_row(&self, layouts: &Layouts) -> Option<SidebarItem> {
        match self.cursor? {
            SidebarItem::Agent { session, .. } if self.selected_agent(layouts).is_none() => {
                Some(SidebarItem::Session(session))
            }
            item => Some(item),
        }
    }

    /// The thread running in pane `pane`, if any.
    pub fn agent_in(&self, pane: PaneId) -> Option<&Thread> {
        self.threads().find(|thread| {
            thread
                .pane
                .as_ref()
                .is_some_and(|launch| launch.pane == pane)
        })
    }

    /// The selected session's lead thread: its oldest agent pane's.
    pub fn selected_thread(&self) -> Option<&Thread> {
        self.agents(self.selected_session()?.id).into_iter().next()
    }

    /// The project of the session under the cursor.
    pub fn selected_project(&self) -> Option<&Project> {
        self.project(self.selected_session()?.project)
    }

    /// Move the cursor to the row below; wraps from the last row to the first.
    /// Without a cursor, or with one on a row that's gone, selects the first
    /// row.
    pub fn select_next(&mut self, layouts: &Layouts) {
        let items = self.items(layouts);
        let next = match self.position(&items, layouts) {
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
    pub fn select_prev(&mut self, layouts: &Layouts) {
        let items = self.items(layouts);
        let prev = match self.position(&items, layouts) {
            None => items.first(),
            Some(at) => items.get((at + items.len() - 1) % items.len()),
        };
        if let Some(&prev) = prev {
            self.cursor = Some(prev);
        }
    }

    /// Move the cursor to `item` when it's one of the sidebar's rows;
    /// otherwise leave it.
    pub fn select_row(&mut self, item: SidebarItem, layouts: &Layouts) {
        if self.items(layouts).contains(&item) {
            self.cursor = Some(item);
        }
    }

    /// Move the cursor to the first row.
    pub fn select_first(&mut self, layouts: &Layouts) {
        if let Some(&first) = self.items(layouts).first() {
            self.cursor = Some(first);
        }
    }

    /// Move the cursor to the last row.
    pub fn select_last(&mut self, layouts: &Layouts) {
        if let Some(&last) = self.items(layouts).last() {
            self.cursor = Some(last);
        }
    }

    /// Move the cursor down by as many rows as fit in half the sidebar's
    /// visible height, at least one; stays put on the last row. Without a
    /// cursor, or with one on a row that's gone, selects the first row.
    pub fn half_page_down(&mut self, layout: &SidebarLayout, layouts: &Layouts) {
        let items = self.items(layouts);
        let target = match self.position(&items, layouts) {
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
    pub fn half_page_up(&mut self, layout: &SidebarLayout, layouts: &Layouts) {
        let items = self.items(layouts);
        let target = match self.position(&items, layouts) {
            None => items.first(),
            Some(at) => items.get(at.saturating_sub(half_page(layout, (0..at).rev()))),
        };
        if let Some(&target) = target {
            self.cursor = Some(target);
        }
    }

    /// Where the cursor goes when session `item` is settled: the next card
    /// below, else the nearest one above, else the shelf header the settle
    /// creates.
    pub fn card_neighbour(&self, item: SidebarItem, layouts: &Layouts) -> Option<SidebarItem> {
        self.neighbour(item, layouts, |row| matches!(row, SidebarRow::Card { .. }))
            .or(Some(SidebarItem::SettledShelf))
    }

    /// Where the cursor goes when `item` is deleted: the next row below,
    /// else the nearest one above, skipping shelf headers and agent rows;
    /// else the shelf header if another session is settled.
    pub fn row_neighbour(&self, item: SidebarItem, layouts: &Layouts) -> Option<SidebarItem> {
        self.neighbour(item, layouts, |row| {
            !matches!(
                row,
                SidebarRow::ShelfHeader { .. } | SidebarRow::Agent { .. }
            )
        })
        .or_else(|| {
            self.listed_sessions(layouts)
                .into_iter()
                .any(|(_, session)| {
                    SidebarItem::Session(session.id) != item && session.settled_at.is_some()
                })
                .then_some(SidebarItem::SettledShelf)
        })
    }

    /// Filters the sidebar to `filter`'s project, or to all projects; a
    /// cursor on a row no longer listed moves to the first row.
    pub fn filter_to(&mut self, filter: Option<ProjectId>, layouts: &Layouts) {
        self.filter = filter;
        let items = self.items(layouts);
        if self.position(&items, layouts).is_none() {
            self.cursor = items.first().copied();
        }
    }

    /// Show the Settled shelf's sessions.
    pub fn open_shelf(&mut self) {
        self.shelf_open = true;
    }

    /// Hide the Settled shelf's sessions and put the cursor on its header.
    pub fn close_shelf(&mut self) {
        self.shelf_open = false;
        self.cursor = Some(SidebarItem::SettledShelf);
    }

    /// Hide session `id`'s agent rows and put the cursor on its card.
    pub fn fold(&mut self, id: SessionId) {
        self.folded.insert(id);
        self.cursor = Some(SidebarItem::Session(id));
    }

    /// Show session `id`'s agent rows again.
    pub fn unfold(&mut self, id: SessionId) {
        self.folded.remove(&id);
    }

    /// Whether a jump can land on `item`: it still exists, isn't being
    /// deleted, and the project filter lists it. Never the Settled header; an
    /// agent row only while its pane still runs an agent.
    pub fn jumpable(&self, item: SidebarItem, layouts: &Layouts) -> bool {
        let mut projects = self.listed_projects();
        match item {
            SidebarItem::Session(id) => {
                !self.deleting.contains(&id)
                    && self.session(id).is_some_and(|session| {
                        projects.any(|project| project.id == session.project)
                    })
            }
            SidebarItem::Agent { session, pane } => {
                self.jumpable(SidebarItem::Session(session), layouts)
                    && self.runs_agent(session, pane, layouts)
            }
            SidebarItem::SettledShelf => false,
        }
    }

    /// How many threads have `status`, not counting those being deleted.
    pub fn status_count(&self, status: ThreadStatus) -> usize {
        self.shown_threads()
            .filter(|thread| thread.status == status)
            .count()
    }

    /// How many threads are running a turn right now, not counting those
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

    /// The sessions the sidebar may list, with their projects: in a listed
    /// project, not being deleted, and matching the search by their title or
    /// the name one of their agent rows shows.
    fn listed_sessions(&self, layouts: &Layouts) -> Vec<(&Project, &Session)> {
        self.sessions
            .iter()
            .filter(|session| !self.deleting.contains(&session.id))
            .filter_map(|session| {
                let project = self
                    .listed_projects()
                    .find(|project| project.id == session.project)?;
                Some((project, session))
            })
            .filter(|(_, session)| {
                !self.searching()
                    || self.title_matches(&self.title(session)).is_some()
                    || self
                        .agent_rows(session.id, layouts)
                        .into_iter()
                        .any(|(thread, pane)| {
                            self.title_matches(layouts.agent_name(pane, Some(thread)))
                                .is_some()
                        })
            })
            .collect()
    }

    /// Every thread but those being deleted.
    fn shown_threads(&self) -> impl Iterator<Item = &Thread> {
        self.threads().filter(|thread| !self.is_deleting(thread))
    }

    /// Whether a search with text is filtering the sidebar.
    pub fn searching(&self) -> bool {
        self.search
            .as_ref()
            .is_some_and(|search| !search.input.text().trim().is_empty())
    }

    /// The rows the search steps through, in display order: every row but
    /// the shelf header, so agent rows are stops too.
    fn matches(&self, layouts: &Layouts) -> Vec<SidebarItem> {
        self.items(layouts)
            .into_iter()
            .filter(|&item| item != SidebarItem::SettledShelf)
            .collect()
    }

    /// What each sidebar row's cursor rests on, in display order.
    fn items(&self, layouts: &Layouts) -> Vec<SidebarItem> {
        self.sidebar(layouts).iter().map(SidebarRow::item).collect()
    }

    /// Where the cursor is in `items`, if it's on one of them; a cursor on
    /// a vanished agent row is on its session's card.
    fn position(&self, items: &[SidebarItem], layouts: &Layouts) -> Option<usize> {
        let cursor = self.cursor_row(layouts)?;
        items.iter().position(|&item| item == cursor)
    }

    /// The first row after `item`'s that `fits`, else the nearest one before
    /// it.
    fn neighbour<F>(&self, item: SidebarItem, layouts: &Layouts, fits: F) -> Option<SidebarItem>
    where
        F: Fn(&SidebarRow<'_>) -> bool,
    {
        let rows = self.sidebar(layouts);
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

/// The sessions `projects`' threads run in, one per pane session, as the
/// sessions actor would show them: each in its first thread's directory,
/// neither pinned nor settled, active since the thread's creation.
#[cfg(test)]
pub(crate) fn sessions_for(projects: &[Project]) -> Vec<Session> {
    let mut shown: Vec<Session> = Vec::new();
    for project in projects {
        for thread in &project.threads {
            let Some(id) = thread.session() else {
                continue;
            };
            if shown.iter().any(|session| session.id == id) {
                continue;
            }
            shown.push(Session {
                id,
                project: project.id,
                kind: match project.kind {
                    ProjectKind::Normal => SessionKind::Plain,
                    ProjectKind::Research => SessionKind::Research,
                    ProjectKind::Learn => SessionKind::Learn,
                    ProjectKind::Incognito => SessionKind::Incognito,
                },
                dir: thread.cwd.clone(),
                name: None,
                branch: None,
                created_at: thread.created_at,
                pinned_at: None,
                settled_at: None,
                active_since: thread.created_at,
                last_activity_at: thread.last_activity_at,
            });
        }
    }
    shown
}

/// Layouts holding every pane `projects`' threads run in: one tab per pane,
/// in its thread's session.
#[cfg(test)]
pub(crate) fn layouts_for(projects: &[Project]) -> Layouts {
    use crate::feat::layout::state::{SessionLayout, test_entry};
    let mut layouts = Layouts::default();
    for launch in projects
        .iter()
        .flat_map(|project| &project.threads)
        .filter_map(|thread| thread.pane.as_ref())
    {
        let entry = test_entry(launch.pane.0);
        match layouts.get(launch.session) {
            Some(_) => layouts.new_tab(launch.session, entry),
            None => layouts.insert(launch.session, SessionLayout::of(entry)),
        }
    }
    layouts
}

#[cfg(test)]
mod tests;
