//! An open picker: what it picks, its list, and where the keys go back to.
//!
//! The directory picker reads its input as a path. The part through the last
//! `/` names the directory being browsed, and the rest filters its
//! subdirectories. Editing the directory part asks the frontend to list the
//! new directory.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::feat::git::git_service::GitRef;
use crate::feat::picker::list::{BranchRow, Matches, PickerItem, PickerList};
use crate::feat::sessions::state::{
    NEW_THREAD, Project, ProjectId, Session, SessionId, Sessions, Thread, ThreadId,
};
use crate::feat::sessions::transcript::{Exchange, Role};
use crate::feat::worktrees::state::{User, order, users};
use crate::{AppState, Focus};

/// What a workspace or base picker sets up: a new session in a project, or
/// an existing session's move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickTarget {
    /// A new session of the project.
    New(ProjectId),
    /// An existing session's new workspace.
    Move(SessionId),
}

/// What an open picker picks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerKind {
    /// `<C-g> n`: pick the project of a new session.
    Projects,
    /// `<C-g> a`: pick a directory to add as a project. `listed` is the directory
    /// text (e.g. `~/dev/`) the items were read from; `None` while the input
    /// isn't a path.
    Directories { listed: Option<String> },
    /// `<C-g> w`: pick where `target`'s session runs.
    Workspace { target: PickTarget },
    /// A new worktree's base for `target`: the refs of the project's `root`,
    /// the default branch first, none disabled.
    Base { target: PickTarget, root: PathBuf },
    /// `<C-g> b`: pick a branch for `session`, whose refs are listed in `cwd`,
    /// its directory. `unstarted` is whether it has had no agent turn yet, so
    /// it can still follow a branch into another worktree.
    Branches {
        session: SessionId,
        cwd: PathBuf,
        unstarted: bool,
    },
    /// `<C-g> w` or `<C-g> b` where the project isn't a git repository: make it one.
    InitGit { project: ProjectId },
    /// `<C-g> f`: pick the project the sidebar is filtered to, or all of them.
    ProjectFilter,
    /// `<C-x>` in the project filter: confirm removing `project`.
    RemoveProject { project: ProjectId },
    /// `s` on a session: confirm settling it.
    SettleSession { session: SessionId },
    /// `d` on a session: confirm deleting it. `folder` says its Research or
    /// Learn folder goes too.
    DeleteSession { session: SessionId, folder: bool },
    /// `<C-g> Space`: pick a thread to jump into. `settled` is whether
    /// settled threads are listed; `<C-s>` flips it.
    Sessions { settled: bool },
    /// `<C-g> W`: orb's worktrees, to look over and delete.
    Worktrees,
    /// `<C-x>` in the worktree picker: confirm force-removing the worktree at
    /// `path`. `dirty` is whether it has uncommitted changes.
    DeleteWorktree { path: PathBuf, dirty: bool },
    /// `<C-g> /`: messages across every thread's transcripts that match the typed
    /// text. `overflow` is whether more matched than are listed.
    Search { overflow: bool },
}

/// What the session picker's preview shows for a thread: its transcript's
/// last exchanges, and the transcript's length when they were read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPreview {
    pub thread: ThreadId,
    pub len: u64,
    pub exchanges: Vec<Exchange>,
}

/// What the search picker's preview shows for a hit: every indexed message of
/// the exchange it's in, as `(id, role, text)` in transcript order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPreview {
    pub hit: i64,
    pub messages: Vec<(i64, Role, String)>,
}

/// The open picker.
#[derive(Debug)]
pub struct PickerState {
    kind: PickerKind,
    list: PickerList,
    /// Where `Esc` or a pick returns the keys.
    return_to: Focus,
    /// What `~/` expands to.
    home: PathBuf,
    /// How many items fit on screen, as the frontend last drew it.
    page: usize,
    /// The branch to select when the refs are listed with nothing typed.
    wanted: Option<String>,
    /// The session picker's preview, as last read.
    preview: Option<SessionPreview>,
    /// The search picker's preview, as last loaded.
    search_preview: Option<SearchPreview>,
    /// The worktree list a delete confirm was opened over; the confirm
    /// returns to it.
    under: Option<Box<PickerState>>,
}

impl PickerState {
    /// A project picker over `items`, in the order given.
    pub fn projects(items: Vec<PickerItem>, return_to: Focus) -> Self {
        Self {
            kind: PickerKind::Projects,
            list: PickerList::new(items),
            return_to,
            home: PathBuf::new(),
            page: 0,
            wanted: None,
            preview: None,
            search_preview: None,
            under: None,
        }
    }

    /// The session picker over `items`, in the order given, with settled
    /// threads hidden.
    pub fn sessions(items: Vec<PickerItem>, return_to: Focus) -> Self {
        Self {
            kind: PickerKind::Sessions { settled: false },
            list: PickerList::new(items),
            return_to,
            home: PathBuf::new(),
            page: 0,
            wanted: None,
            preview: None,
            search_preview: None,
            under: None,
        }
    }

    /// The worktree picker over `items`, in the order given.
    pub fn worktrees(items: Vec<PickerItem>, return_to: Focus) -> Self {
        Self {
            kind: PickerKind::Worktrees,
            ..Self::projects(items, return_to)
        }
    }

    /// The search picker, with nothing typed and no rows yet.
    pub fn search(return_to: Focus) -> Self {
        Self {
            kind: PickerKind::Search { overflow: false },
            ..Self::projects(Vec::new(), return_to)
        }
    }

    /// A project filter picker over `items`, in the order given, with
    /// `current`'s project selected, else the first item.
    pub fn project_filter(
        items: Vec<PickerItem>,
        current: Option<ProjectId>,
        return_to: Focus,
    ) -> Self {
        let list = {
            let selected = items
                .iter()
                .find(|item| matches!(item, PickerItem::Project { id, .. } if Some(*id) == current))
                .cloned();
            let mut list = PickerList::new(items);
            if let Some(selected) = selected {
                list.select(&selected);
            }
            list
        };
        Self {
            kind: PickerKind::ProjectFilter,
            list,
            return_to,
            home: PathBuf::new(),
            page: 0,
            wanted: None,
            preview: None,
            search_preview: None,
            under: None,
        }
    }

    /// The `No`/`Yes` confirm for removing `project`, with `No` selected.
    pub fn remove_project(project: ProjectId, return_to: Focus) -> Self {
        Self::confirm(PickerKind::RemoveProject { project }, return_to)
    }

    /// The `No`/`Yes` confirm for settling `session`, with `No` selected.
    pub fn settle_session(session: SessionId, return_to: Focus) -> Self {
        Self::confirm(PickerKind::SettleSession { session }, return_to)
    }

    /// The `No`/`Yes` confirm for deleting `session` (and its folder when
    /// `folder`), with `No` selected.
    pub fn delete_session(session: SessionId, folder: bool, return_to: Focus) -> Self {
        Self::confirm(PickerKind::DeleteSession { session, folder }, return_to)
    }

    /// The `No`/`Yes` confirm for deleting the worktree at `path`, with `No`
    /// selected, over `list`, which it returns to.
    pub fn delete_worktree(list: PickerState, path: PathBuf, dirty: bool) -> Self {
        let return_to = list.return_to;
        Self {
            under: Some(Box::new(list)),
            ..Self::confirm(PickerKind::DeleteWorktree { path, dirty }, return_to)
        }
    }

    /// The picker this confirm was opened over, if any.
    pub fn back(self) -> Option<PickerState> {
        self.under.map(|list| *list)
    }

    /// The picker this confirm is drawn over, if any.
    pub fn under(&self) -> Option<&PickerState> {
        self.under.as_deref()
    }

    /// A `No`/`Yes` confirm of `kind`, with `No` selected.
    fn confirm(kind: PickerKind, return_to: Focus) -> Self {
        Self {
            kind,
            list: PickerList::new(vec![PickerItem::Confirm(false), PickerItem::Confirm(true)]),
            return_to,
            home: PathBuf::new(),
            page: 0,
            wanted: None,
            preview: None,
            search_preview: None,
            under: None,
        }
    }

    /// A workspace picker for `target` over `items`, in the order given.
    pub fn workspace(target: PickTarget, items: Vec<PickerItem>, return_to: Focus) -> Self {
        Self {
            kind: PickerKind::Workspace { target },
            list: PickerList::new(items),
            return_to,
            home: PathBuf::new(),
            page: 0,
            wanted: None,
            preview: None,
            search_preview: None,
            under: None,
        }
    }

    /// A branch picker for `session` in `cwd`, empty until its refs are
    /// listed; then `wanted` is selected if listed, else the first branch.
    pub fn branches(
        session: SessionId,
        cwd: PathBuf,
        unstarted: bool,
        wanted: Option<String>,
        return_to: Focus,
    ) -> Self {
        Self {
            kind: PickerKind::Branches {
                session,
                cwd,
                unstarted,
            },
            list: PickerList::default(),
            return_to,
            home: PathBuf::new(),
            page: 0,
            wanted,
            preview: None,
            search_preview: None,
            under: None,
        }
    }

    /// A base branch picker for `target`'s new worktree, listing the refs of
    /// `root`; empty until they are listed.
    pub fn base(target: PickTarget, root: PathBuf, return_to: Focus) -> Self {
        Self {
            kind: PickerKind::Base { target, root },
            list: PickerList::default(),
            return_to,
            home: PathBuf::new(),
            page: 0,
            wanted: None,
            preview: None,
            search_preview: None,
            under: None,
        }
    }

    /// The one-row picker that offers to make `project` a git repository.
    pub fn init_git(project: ProjectId, return_to: Focus) -> Self {
        Self {
            kind: PickerKind::InitGit { project },
            list: PickerList::new(vec![PickerItem::InitGit]),
            return_to,
            home: PathBuf::new(),
            page: 0,
            wanted: None,
            preview: None,
            search_preview: None,
            under: None,
        }
    }

    /// A directory picker with `~/` typed, and `home`, the directory to list.
    pub fn directories(home: PathBuf, return_to: Focus) -> (Self, PathBuf) {
        let list = {
            let mut list = PickerList::default();
            list.replace_input("~/");
            list
        };
        let picker = Self {
            kind: PickerKind::Directories {
                listed: Some("~/".to_owned()),
            },
            list,
            return_to,
            home: home.clone(),
            page: 0,
            wanted: None,
            preview: None,
            search_preview: None,
            under: None,
        };
        (picker, home)
    }

    /// Types `ch`. Returns a directory to list when the directory part changed.
    pub fn insert(&mut self, ch: char) -> Option<PathBuf> {
        self.list.insert(ch);
        self.edited()
    }

    /// Deletes the grapheme before the cursor. Returns a directory to list
    /// when the directory part changed.
    pub fn backspace(&mut self) -> Option<PathBuf> {
        self.list.backspace();
        self.edited()
    }

    /// Deletes the word before the cursor. Returns a directory to list when
    /// the directory part changed.
    pub fn delete_word(&mut self) -> Option<PathBuf> {
        self.list.delete_word();
        self.edited()
    }

    /// Shows `names`, the subdirectories of `dir`, alphabetically ignoring
    /// case. Ignored unless `dir` is the directory being browsed.
    pub fn show_directories(&mut self, dir: &Path, names: Vec<String>) {
        let PickerKind::Directories {
            listed: Some(listed),
        } = &self.kind
        else {
            return;
        };
        if expand(listed, &self.home) != dir {
            return;
        }
        let items = {
            let mut names = names;
            names.sort_by_cached_key(|name| (name.to_lowercase(), name.clone()));
            names
                .into_iter()
                .map(|name| PickerItem::Directory { name })
                .collect()
        };
        let leaf =
            split_path(self.list.input()).map_or_else(String::new, |(_, leaf)| leaf.to_owned());
        self.list.set_items(items, &leaf);
    }

    /// Shows `refs`, listed in `cwd`, filtered by the typed text. Ignored
    /// unless this is `cwd`'s branch or base picker. A branch picker selects
    /// the wanted branch when nothing is typed and, once the thread has had a
    /// prompt, disables a branch checked out in another worktree. A base
    /// picker lists the default branch first and disables none.
    pub fn show_branches(&mut self, cwd: &Path, refs: Vec<GitRef>) {
        let unstarted = match &self.kind {
            PickerKind::Branches {
                cwd: picker_cwd,
                unstarted,
                ..
            } if picker_cwd == cwd => *unstarted,
            PickerKind::Base { root, .. } if root == cwd => {
                let mut refs = refs;
                refs.sort_by_key(|git_ref| !git_ref.default);
                let items = refs
                    .into_iter()
                    .map(|git_ref| {
                        PickerItem::Branch(BranchRow {
                            disabled: false,
                            git_ref,
                        })
                    })
                    .collect();
                let pattern = self.list.input().to_owned();
                self.list.set_items(items, &pattern);
                return;
            }
            _ => return,
        };
        let items = refs
            .into_iter()
            .map(|git_ref| {
                let elsewhere = git_ref.worktree.as_ref().is_some_and(|path| path != cwd);
                PickerItem::Branch(BranchRow {
                    disabled: elsewhere && !unstarted,
                    git_ref,
                })
            })
            .collect::<Vec<_>>();
        let wanted = items
            .iter()
            .find(|item| {
                matches!(item, PickerItem::Branch(row) if Some(&row.git_ref.name) == self.wanted.as_ref())
            })
            .cloned();
        let pattern = self.list.input().to_owned();
        self.list.set_items(items, &pattern);
        if let Some(wanted) = wanted.filter(|_| pattern.is_empty()) {
            self.list.select(&wanted);
        }
    }

    /// Shows or hides settled threads in the session picker, re-listing
    /// `sessions`' threads under the typed text and selecting the best match.
    /// Other pickers ignore it.
    pub fn toggle_settled(&mut self, sessions: &Sessions) {
        let PickerKind::Sessions { settled } = &mut self.kind else {
            return;
        };
        *settled = !*settled;
        let items = session_items(sessions, *settled);
        let pattern = self.list.input().to_owned();
        self.list.set_items(items, &pattern);
    }

    /// The selected row's thread: a session row's previewed agent's, or a
    /// search hit's. `None` for any other row.
    pub fn selected_thread(&self) -> Option<ThreadId> {
        match self.list.selected() {
            Some(&PickerItem::Session { thread, .. }) => thread,
            Some(&PickerItem::Hit { thread, .. }) => Some(thread),
            _ => None,
        }
    }

    /// The session the selected row opens: a session row's, or the one a
    /// hit's thread runs or last ran in. `None` for any other row.
    pub fn picked_session(&self, sessions: &Sessions) -> Option<SessionId> {
        match *self.list.selected()? {
            PickerItem::Session { id, .. } => Some(id),
            PickerItem::Hit { thread, .. } => sessions
                .threads()
                .find(|shown| shown.id == thread)
                .and_then(Thread::home),
            _ => None,
        }
    }

    /// The selected worktree row's path; `None` unless one is selected.
    pub fn selected_worktree(&self) -> Option<&Path> {
        match self.list.selected() {
            Some(PickerItem::Worktree { path, .. }) => Some(path),
            _ => None,
        }
    }

    /// Drops the selected worktree row, keeping the typed text, and selects
    /// the next row, else the previous one.
    pub fn remove_worktree_row(&mut self) {
        self.list.remove_selected();
    }

    /// Keeps `exchanges`, read from `thread`'s transcript at `len` bytes, as
    /// the preview. Dropped unless `thread` is still the selected row, so a
    /// late read for an old selection never shows.
    pub fn show_preview(&mut self, thread: ThreadId, len: u64, exchanges: Vec<Exchange>) {
        if self.selected_thread() == Some(thread) {
            self.preview = Some(SessionPreview {
                thread,
                len,
                exchanges,
            });
        }
    }

    /// The selected thread's preview; `None` until it's read, or with no
    /// thread selected.
    pub fn preview(&self) -> Option<&SessionPreview> {
        self.preview
            .as_ref()
            .filter(|preview| Some(preview.thread) == self.selected_thread())
    }

    /// Forgets both the session and the search picker's preview.
    pub fn clear_preview(&mut self) {
        self.preview = None;
        self.search_preview = None;
    }

    /// The selected hit's message id, transcript and prompt offset; `None`
    /// unless a hit row is selected.
    pub fn selected_hit(&self) -> Option<(i64, &Path, u64)> {
        match self.list.selected() {
            Some(PickerItem::Hit {
                id,
                path,
                prompt_offset,
                ..
            }) => Some((*id, path, *prompt_offset)),
            _ => None,
        }
    }

    /// Keeps `messages` as hit `hit`'s preview. Dropped unless `hit` is still
    /// the selected row, so a late load for an old selection never shows.
    pub fn show_search_preview(&mut self, hit: i64, messages: Vec<(i64, Role, String)>) {
        if self.selected_hit().map(|(id, ..)| id) == Some(hit) {
            self.search_preview = Some(SearchPreview { hit, messages });
        }
    }

    /// The selected hit's preview; `None` until it's loaded, or with no hit
    /// selected.
    pub fn search_preview(&self) -> Option<&SearchPreview> {
        self.search_preview
            .as_ref()
            .filter(|preview| self.selected_hit().map(|(id, ..)| id) == Some(preview.hit))
    }

    /// The typed text while this is the search picker; `None` otherwise.
    pub fn search_query(&self) -> Option<&str> {
        matches!(self.kind, PickerKind::Search { .. }).then(|| self.list.input())
    }

    /// Lists `items`, the rows `query` found, in the order given, selecting
    /// the first. Ignored unless this is the search picker and `query` is
    /// still its typed text, so a late result never replaces newer ones.
    /// `overflow` is whether more matched than `items` holds.
    pub fn show_hits(&mut self, query: &str, items: Vec<PickerItem>, overflow: bool) {
        let PickerKind::Search { overflow: shown } = &mut self.kind else {
            return;
        };
        if self.list.input() != query {
            return;
        }
        *shown = overflow;
        self.list.set_items(items, "");
    }

    /// Whether the selected thread's transcript, now `len` bytes long, needs
    /// reading for the preview: nothing was read for it yet, or it was read
    /// at another length.
    pub fn wants_preview(&self, len: u64) -> bool {
        self.selected_thread().is_some() && self.preview().is_none_or(|preview| preview.len != len)
    }

    /// Selects the shown row holding `item`; stays put when none does.
    pub fn select(&mut self, item: &PickerItem) {
        self.list.select(item);
    }

    /// Browses into the selected directory. Returns it to list.
    pub fn open_directory(&mut self) -> Option<PathBuf> {
        let (dir_text, _) = split_path(self.list.input())?;
        let Some(PickerItem::Directory { name }) = self.list.selected() else {
            return None;
        };
        let input = format!("{dir_text}{name}/");
        self.list.replace_input(&input);
        self.edited()
    }

    /// The directory `⏎` adds: the selected one, else the typed directory
    /// when nothing follows its last `/`.
    pub fn directory_to_add(&self) -> Option<PathBuf> {
        if !matches!(self.kind, PickerKind::Directories { .. }) {
            return None;
        }
        let (dir_text, leaf) = split_path(self.list.input())?;
        let dir = expand(dir_text, &self.home);
        match self.list.selected() {
            Some(PickerItem::Directory { name }) => Some(dir.join(name)),
            Some(
                PickerItem::Project { .. }
                | PickerItem::Workspace(_)
                | PickerItem::Branch(_)
                | PickerItem::InitGit
                | PickerItem::AllProjects
                | PickerItem::Confirm(_)
                | PickerItem::Session { .. }
                | PickerItem::Worktree { .. }
                | PickerItem::Hit { .. },
            ) => None,
            None => leaf.is_empty().then_some(dir),
        }
    }

    /// Records how many items fit on screen, for half-page moves.
    pub fn resize(&mut self, page: usize) {
        self.page = page;
    }

    /// Selects the next item.
    pub fn next(&mut self) {
        self.list.next();
    }

    /// Selects the previous item.
    pub fn prev(&mut self) {
        self.list.prev();
    }

    /// Moves the selection down half a page.
    pub fn half_page_down(&mut self) {
        self.list.half_page_down(self.page);
    }

    /// Moves the selection up half a page.
    pub fn half_page_up(&mut self) {
        self.list.half_page_up(self.page);
    }

    /// Selects shown row `index` unless it's a heading, disabled or past the end.
    pub fn select_row(&mut self, index: usize) {
        self.list.select_row(index);
    }

    /// Selects the next item, stopping on the last.
    pub fn select_below(&mut self) {
        self.list.select_below();
    }

    /// Selects the previous item, stopping on the first.
    pub fn select_above(&mut self) {
        self.list.select_above();
    }

    /// Moves the cursor one grapheme left.
    pub fn cursor_left(&mut self) {
        self.list.cursor_left();
    }

    /// Moves the cursor one grapheme right.
    pub fn cursor_right(&mut self) {
        self.list.cursor_right();
    }

    /// Moves the cursor to grapheme `index`, or to the end when the input is
    /// shorter.
    pub fn cursor_to(&mut self, index: usize) {
        self.list.cursor_to(index);
    }

    /// What the picker picks.
    pub fn kind(&self) -> &PickerKind {
        &self.kind
    }

    /// Where the keys go when the picker closes.
    pub fn return_to(&self) -> Focus {
        self.return_to
    }

    /// How many items fit on screen, as last drawn.
    pub fn page(&self) -> usize {
        self.page
    }

    /// The typed text.
    pub fn input(&self) -> &str {
        self.list.input()
    }

    /// The cursor, as a grapheme index into [`input`](Self::input).
    pub fn cursor(&self) -> usize {
        self.list.cursor()
    }

    /// The selected row, as an index into [`shown`](Self::shown).
    pub fn selection(&self) -> usize {
        self.list.selection()
    }

    /// The selected item; `None` when nothing is shown.
    pub fn selected(&self) -> Option<&PickerItem> {
        self.list.selected()
    }

    /// The shown items and where they matched, in display order.
    pub fn shown(&self) -> impl Iterator<Item = (&PickerItem, &Matches)> {
        self.list.shown()
    }

    /// How many items the picker has, shown or hidden by the filter.
    pub fn total(&self) -> usize {
        self.list.total()
    }

    /// After an edit: re-filter, or start browsing the new directory. The
    /// search picker's rows come from the search index, so it keeps them.
    fn edited(&mut self) -> Option<PathBuf> {
        let input = self.list.input().to_owned();
        let PickerKind::Directories { listed } = &mut self.kind else {
            if !matches!(self.kind, PickerKind::Search { .. }) {
                self.list.refilter(&input);
            }
            return None;
        };
        match split_path(&input) {
            None => {
                *listed = None;
                self.list.set_items(Vec::new(), "");
                None
            }
            Some((dir_text, _)) if listed.as_deref() != Some(dir_text) => {
                *listed = Some(dir_text.to_owned());
                self.list.set_items(Vec::new(), "");
                Some(expand(dir_text, &self.home))
            }
            Some((_, leaf)) => {
                self.list.refilter(leaf);
                None
            }
        }
    }
}

/// Splits a path the user typed into the directory part, through the last
/// `/`, and the rest. `None` unless it starts with `/` or `~/`.
pub fn split_path(input: &str) -> Option<(&str, &str)> {
    if !(input.starts_with('/') || input.starts_with("~/")) {
        return None;
    }
    input.split_at_checked(input.rfind('/')? + 1)
}

/// The directory `dir_text` names, with `~/` meaning `home` and no trailing `/`.
pub fn expand(dir_text: &str, home: &Path) -> PathBuf {
    let path = match dir_text.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(dir_text),
    };
    path.components().collect()
}

/// The session picker's rows: the sessions of every project inside the
/// project filter (removed projects too), newest chat first by the latest
/// chat of its agents, else its own last activity, ties to the higher id. The
/// selected session is listed like any other. Sessions being deleted, and
/// settled sessions unless `settled`, are left out. Each row previews its
/// most recently active agent.
pub fn session_items(sessions: &Sessions, settled: bool) -> Vec<PickerItem> {
    let mut rows: Vec<(SystemTime, i64, PickerItem)> = sessions
        .sessions
        .iter()
        .filter(|session| {
            sessions
                .filter
                .is_none_or(|filter| filter == session.project)
        })
        .filter(|session| !sessions.deleting.contains(&session.id))
        .filter(|session| settled || session.settled_at.is_none())
        .filter_map(|session| {
            let project = sessions.project(session.project)?;
            let lead = sessions
                .agents(session.id)
                .into_iter()
                .max_by_key(|thread| (thread.last_chat(), thread.id.0));
            let (label, split) = session_label(sessions, project, session);
            Some((
                lead.map_or(session.last_activity_at, Thread::last_chat),
                session.id.0,
                PickerItem::Session {
                    id: session.id,
                    thread: lead.map(|thread| thread.id),
                    label,
                    split,
                    settled: session.settled_at.is_some(),
                },
            ))
        })
        .collect();
    rows.sort_by_key(|(last_chat, id, _)| Reverse((*last_chat, *id)));
    rows.into_iter().map(|(_, _, item)| item).collect()
}

/// A session's picker label, `<project>/title`, and the byte offset where
/// the title starts.
pub fn session_label(sessions: &Sessions, project: &Project, session: &Session) -> (String, usize) {
    (
        format!("{}/{}", project.title, sessions.title(session)),
        project.title.len() + 1,
    )
}

/// A search hit's label in `thread`: its session's label while that session
/// exists, else `<project>/<thread title>`.
pub fn hit_label(sessions: &Sessions, project: &Project, thread: &Thread) -> (String, usize) {
    match thread.home().and_then(|id| sessions.session(id)) {
        Some(session) => session_label(sessions, project, session),
        None => {
            let title = thread.title.as_deref().unwrap_or(NEW_THREAD);
            (
                format!("{}/{title}", project.title),
                project.title.len() + 1,
            )
        }
    }
}

/// The worktree picker's rows: `app`'s worktrees in [`order`], each labelled
/// `<repo>/<name>` and matched also on its branch and its users' titles.
pub fn worktree_items(app: &AppState) -> Vec<PickerItem> {
    order(app)
        .into_iter()
        .map(|worktree| {
            let file_name = |path: Option<&Path>| {
                path.and_then(Path::file_name)
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            };
            let repo = file_name(worktree.path.parent());
            let name = file_name(Some(&worktree.path));
            let branch = worktree
                .facts
                .as_ref()
                .and_then(|facts| facts.branch.as_deref());
            let users = users(app, &worktree.path);
            let titles = users.iter().map(|user| match user {
                User::Session(_, session, _) => app.sessions.title(session),
            });
            PickerItem::Worktree {
                path: worktree.path.clone(),
                label: format!("{repo}/{name}"),
                split: repo.len() + 1,
                extra: branch
                    .map(str::to_owned)
                    .into_iter()
                    .chain(titles)
                    .collect::<Vec<_>>()
                    .join(" "),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::feat::harness::HarnessId;
    use std::path::{Path, PathBuf};

    use std::time::UNIX_EPOCH;

    use super::{PickTarget, PickerState, expand, hit_label, session_items, split_path};
    use crate::Focus;
    use crate::feat::git::git_service::GitRef;
    use crate::feat::picker::list::PickerItem;
    use crate::feat::sessions::state::{
        PaneId, PaneLaunch, Project, ProjectId, ProjectKind, SessionId, Sessions, Thread, ThreadId,
        ThreadStatus, sessions_for,
    };
    use crate::feat::sessions::transcript::Exchange;
    use std::time::Duration;

    const HOME: &str = "/home/u";

    fn names(text: &[&str]) -> Vec<String> {
        text.iter().map(|name| (*name).to_owned()).collect()
    }

    fn shown_names(picker: &PickerState) -> Vec<String> {
        picker
            .shown()
            .filter_map(|(item, _)| match item {
                PickerItem::Directory { name } => Some(name.clone()),
                _ => None,
            })
            .collect()
    }

    /// A directory picker at `~/` listing `listing`.
    fn browsing(listing: &[&str]) -> PickerState {
        let (mut picker, home) = PickerState::directories(PathBuf::from(HOME), Focus::Sidebar);
        picker.show_directories(&home, names(listing));
        picker
    }

    fn typing(picker: &mut PickerState, text: &str) -> Option<PathBuf> {
        text.chars().fold(None, |_, ch| picker.insert(ch))
    }

    #[rstest::rstest]
    #[case("~/dev/it", Some(("~/dev/", "it")))]
    #[case("/", Some(("/", "")))]
    #[case("~", None)]
    #[case("dev", None)]
    fn split_path_splits_after_the_last_slash(
        #[case] input: &str,
        #[case] expected: Option<(&str, &str)>,
    ) {
        // Given / When / Then.
        assert_eq!(split_path(input), expected, "split_path({input:?})");
    }

    #[rstest::rstest]
    fn total_counts_items_the_filter_hides() {
        // Given a project picker over two projects, filtered to neither.
        let picker = {
            let project = |id, title: &str| PickerItem::Project {
                id: ProjectId(id),
                title: title.to_owned(),
                root: PathBuf::from("/tmp").join(title),
                kind: ProjectKind::Normal,
            };
            let mut picker =
                PickerState::projects(vec![project(1, "orb"), project(2, "jinn")], Focus::Sidebar);
            picker.insert('z');
            picker
        };

        // When counting its items.
        let total = picker.total();

        // Then both projects count.
        assert_eq!(total, 2, "total should count the hidden items");
    }

    #[rstest::rstest]
    fn expand_resolves_home_without_a_trailing_slash() {
        // Given a directory under `~/`.
        let dir_text = "~/dev/";

        // When expanding it.
        let path = expand(dir_text, Path::new(HOME));

        // Then it is under home, with no trailing slash.
        assert_eq!(
            path.as_os_str(),
            "/home/u/dev",
            "~/dev/ should expand to home/dev"
        );
    }

    #[rstest::rstest]
    fn directory_picker_opens_with_home_typed() {
        // Given / When opening a directory picker.
        let (picker, _) = PickerState::directories(PathBuf::from(HOME), Focus::Sidebar);

        // Then `~/` is typed.
        assert_eq!(picker.input(), "~/", "the picker should open at ~/");
    }

    #[rstest::rstest]
    fn directory_picker_lists_home_first() {
        // Given / When opening a directory picker.
        let (_, to_list) = PickerState::directories(PathBuf::from(HOME), Focus::Sidebar);

        // Then home is the directory to list.
        assert_eq!(to_list, Path::new(HOME), "the picker should list home");
    }

    #[rstest::rstest]
    fn directories_sort_ignoring_case() {
        // Given / When listing directories in mixed case.
        let picker = browsing(&["dev", "Documents", "Applications", "Desktop"]);

        // Then they are alphabetical, ignoring case.
        assert_eq!(
            shown_names(&picker),
            ["Applications", "Desktop", "dev", "Documents"],
            "directories should sort case-insensitively"
        );
    }

    #[rstest::rstest]
    fn hidden_directories_are_hidden_with_nothing_typed() {
        // Given / When listing a dot-directory with nothing after `~/`.
        let picker = browsing(&[".cargo", "dev"]);

        // Then the dot-directory is hidden.
        assert_eq!(
            shown_names(&picker),
            ["dev"],
            "dot-directories should be hidden"
        );
    }

    #[rstest::rstest]
    fn hidden_directories_are_hidden_before_fuzzy_matching() {
        // Given a dot-directory and a directory, both containing "c".
        let mut picker = browsing(&[".cargo", "code"]);

        // When typing "c".
        typing(&mut picker, "c");

        // Then only the visible one matches.
        assert_eq!(
            shown_names(&picker),
            ["code"],
            "`~/c` should not surface `.cargo`"
        );
    }

    #[rstest::rstest]
    fn hidden_directories_show_when_a_dot_is_typed() {
        // Given a listed dot-directory.
        let mut picker = browsing(&[".cargo", "dev"]);

        // When typing ".".
        typing(&mut picker, ".");

        // Then the dot-directory is shown.
        assert_eq!(
            shown_names(&picker),
            [".cargo"],
            "a leading dot should show dot-directories"
        );
    }

    #[rstest::rstest]
    fn edit_within_the_directory_lists_nothing() {
        // Given a directory picker at `~/`.
        let mut picker = browsing(&["dev"]);

        // When typing a filter.
        let to_list = picker.insert('d');

        // Then nothing needs listing.
        assert_eq!(to_list, None, "the directory didn't change");
    }

    #[rstest::rstest]
    fn backspace_past_a_slash_lists_the_parent() {
        // Given `~/dev/` typed.
        let mut picker = browsing(&["dev"]);
        typing(&mut picker, "dev/");

        // When deleting the `/`.
        let to_list = picker.backspace();

        // Then home is listed again.
        assert_eq!(
            to_list,
            Some(PathBuf::from(HOME)),
            "going up should list the parent"
        );
    }

    #[rstest::rstest]
    fn stale_listing_is_ignored() {
        // Given a directory picker at `~/`.
        let (mut picker, _) = PickerState::directories(PathBuf::from(HOME), Focus::Sidebar);

        // When a listing arrives for another directory.
        picker.show_directories(Path::new("/elsewhere"), names(&["x"]));

        // Then nothing is shown.
        assert!(
            shown_names(&picker).is_empty(),
            "a stale listing should be ignored"
        );
    }

    #[rstest::rstest]
    fn open_directory_types_its_path() {
        // Given `dev` selected at `~/`.
        let mut picker = browsing(&["dev"]);

        // When opening it.
        picker.open_directory();

        // Then its path is typed.
        assert_eq!(picker.input(), "~/dev/", "Tab should type the directory");
    }

    #[rstest::rstest]
    fn open_directory_lists_it() {
        // Given `dev` selected at `~/`.
        let mut picker = browsing(&["dev"]);

        // When opening it.
        let to_list = picker.open_directory();

        // Then it is the directory to list.
        assert_eq!(
            to_list,
            Some(PathBuf::from("/home/u/dev")),
            "Tab should list the directory"
        );
    }

    #[rstest::rstest]
    fn directory_to_add_is_the_selected_directory() {
        // Given `dev` selected at `~/`.
        let picker = browsing(&["dev"]);

        // When asking what `⏎` adds.
        let added = picker.directory_to_add();

        // Then it is `dev` under home.
        assert_eq!(
            added,
            Some(PathBuf::from("/home/u/dev")),
            "⏎ should add the selected directory"
        );
    }

    #[rstest::rstest]
    fn directory_to_add_is_the_typed_directory_when_empty() {
        // Given `~/dev/` typed and listed with no subdirectories.
        let mut picker = browsing(&["dev"]);
        let dev = typing(&mut picker, "dev/").unwrap_or_default();
        picker.show_directories(&dev, Vec::new());

        // When asking what `⏎` adds.
        let added = picker.directory_to_add();

        // Then it is the typed directory.
        assert_eq!(
            added,
            Some(PathBuf::from("/home/u/dev")),
            "⏎ should add the typed directory"
        );
    }

    #[rstest::rstest]
    fn directory_to_add_is_none_for_an_unmatched_filter() {
        // Given a filter that matches no directory.
        let mut picker = browsing(&["dev"]);
        typing(&mut picker, "zz");

        // When asking what `⏎` adds.
        let added = picker.directory_to_add();

        // Then there is nothing to add.
        assert_eq!(added, None, "an unmatched filter adds nothing");
    }

    const CWD: &str = "/work";

    /// Local branch `feat`, checked out in another worktree.
    fn feat_elsewhere() -> GitRef {
        GitRef {
            name: "feat".to_owned(),
            remote: false,
            current: false,
            default: false,
            worktree: Some(PathBuf::from("/wt/feat")),
        }
    }

    /// Whether each shown branch row is disabled.
    fn disabled_rows(picker: &PickerState) -> Vec<bool> {
        picker
            .shown()
            .filter_map(|(item, _)| match item {
                PickerItem::Branch(row) => Some(row.disabled),
                _ => None,
            })
            .collect()
    }

    /// Session 1's branch picker in [`CWD`], showing `refs`.
    fn branches_listing(unstarted: bool, refs: Vec<GitRef>) -> PickerState {
        let mut picker =
            PickerState::branches(SessionId(1), CWD.into(), unstarted, None, Focus::Sidebar);
        picker.show_branches(Path::new(CWD), refs);
        picker
    }

    #[rstest::rstest]
    fn branch_elsewhere_is_disabled_after_the_first_prompt() {
        // Given / When listing a branch checked out elsewhere for a prompted thread.
        let picker = branches_listing(false, vec![feat_elsewhere()]);

        // Then its row is disabled.
        assert_eq!(
            disabled_rows(&picker),
            [true],
            "a prompted thread can't follow a branch into another worktree"
        );
    }

    #[rstest::rstest]
    fn branch_elsewhere_is_enabled_before_the_first_prompt() {
        // Given / When listing a branch checked out elsewhere for a prompt-less thread.
        let picker = branches_listing(true, vec![feat_elsewhere()]);

        // Then its row is enabled.
        assert_eq!(
            disabled_rows(&picker),
            [false],
            "a prompt-less thread can move to the branch's worktree"
        );
    }

    #[rstest::rstest]
    fn stale_branch_listing_is_ignored() {
        // Given a branch picker in `/work`.
        let mut picker = branches_listing(true, vec![]);

        // When refs listed in another directory arrive.
        picker.show_branches(Path::new("/elsewhere"), vec![feat_elsewhere()]);

        // Then nothing is shown.
        assert_eq!(
            picker.shown().count(),
            0,
            "a listing for another directory should be ignored"
        );
    }

    /// Local branch `name`, the default branch when `default`.
    fn local(name: &str, default: bool) -> GitRef {
        GitRef {
            name: name.to_owned(),
            remote: false,
            current: false,
            default,
            worktree: None,
        }
    }

    /// Project 1's base picker for a new worktree of [`CWD`], showing `refs`.
    fn base_listing(refs: Vec<GitRef>) -> PickerState {
        let mut picker =
            PickerState::base(PickTarget::New(ProjectId(1)), CWD.into(), Focus::Sidebar);
        picker.show_branches(Path::new(CWD), refs);
        picker
    }

    #[rstest::rstest]
    fn base_picker_lists_the_default_branch_first() {
        // Given / When listing two branches before the default one.
        let picker = base_listing(vec![
            local("feat", false),
            local("fix", false),
            local("main", true),
        ]);

        // Then the default branch comes first, the rest in order.
        let names: Vec<&str> = picker
            .shown()
            .filter_map(|(item, _)| match item {
                PickerItem::Branch(row) => Some(row.git_ref.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            names,
            ["main", "feat", "fix"],
            "the default branch should lead the base picker"
        );
    }

    #[rstest::rstest]
    fn base_picker_disables_no_branch() {
        // Given / When listing a branch checked out in another worktree.
        let picker = base_listing(vec![local("main", true), feat_elsewhere()]);

        // Then no row is disabled.
        assert_eq!(
            disabled_rows(&picker),
            [false, false],
            "any branch can be a new worktree's base"
        );
    }

    #[rstest::rstest]
    fn remove_project_confirm_lists_no_then_yes() {
        // Given / When opening the confirm for removing project 1.
        let picker = PickerState::remove_project(ProjectId(1), Focus::Sidebar);

        // Then it lists No, then Yes.
        let rows: Vec<&PickerItem> = picker.shown().map(|(item, _)| item).collect();
        assert_eq!(
            rows,
            vec![&PickerItem::Confirm(false), &PickerItem::Confirm(true)],
            "the confirm's rows"
        );
    }

    #[rstest::rstest]
    fn remove_project_confirm_selects_no() {
        // Given / When opening the confirm for removing project 1.
        let picker = PickerState::remove_project(ProjectId(1), Focus::Sidebar);

        // Then No is highlighted, so ⏎ alone removes nothing.
        assert_eq!(
            picker.selected(),
            Some(&PickerItem::Confirm(false)),
            "No should be the default"
        );
    }

    #[rstest::rstest]
    fn settle_session_confirm_starts_on_no() {
        // Given / When opening the confirm for settling session 9.
        let picker = PickerState::settle_session(SessionId(9), Focus::Sidebar);

        // Then No is highlighted, so ⏎ alone settles nothing.
        assert_eq!(
            picker.selected(),
            Some(&PickerItem::Confirm(false)),
            "No should be the default"
        );
    }

    /// The session picker over sessions 1 (`work/alpha`) and 2
    /// (`work/zulu`), each previewing its own thread, with session 1
    /// selected.
    fn two_threads() -> PickerState {
        let row = |id, title: &str| PickerItem::Session {
            id: SessionId(id),
            thread: Some(ThreadId(id)),
            label: format!("work/{title}"),
            split: 5,
            settled: false,
        };
        PickerState::sessions(vec![row(1, "alpha"), row(2, "zulu")], Focus::Sidebar)
    }

    fn reply(text: &str) -> Vec<Exchange> {
        vec![Exchange {
            reply: Some((text.to_owned(), None)),
            ..Exchange::default()
        }]
    }

    #[rstest::rstest]
    fn show_preview_for_the_selected_thread_is_shown() {
        // Given the session picker with thread 1 selected.
        let mut picker = two_threads();

        // When showing a preview read from thread 1's transcript.
        picker.show_preview(ThreadId(1), 10, reply("Done"));

        // Then the picker's preview is that read.
        assert_eq!(
            picker.preview().map(|preview| (
                preview.thread,
                preview.len,
                preview.exchanges.clone()
            )),
            Some((ThreadId(1), 10, reply("Done"))),
            "the selected thread's preview should be shown"
        );
    }

    #[rstest::rstest]
    fn show_preview_for_a_thread_no_longer_selected_is_dropped() {
        // Given the session picker moved from thread 1 to thread 2.
        let mut picker = two_threads();
        picker.next();

        // When a late read for thread 1 arrives.
        picker.show_preview(ThreadId(1), 10, reply("Done"));

        // Then no preview is shown.
        assert!(
            picker.preview().is_none(),
            "a read for an old selection should be dropped"
        );
    }

    #[rstest::rstest]
    #[case::unread(None, true)]
    #[case::same(Some(10), false)]
    #[case::grown(Some(5), true)]
    fn wants_preview_when_the_read_length_differs(
        #[case] read_at: Option<u64>,
        #[case] expected: bool,
    ) {
        // Given the session picker on thread 1, its preview read at `read_at`.
        let mut picker = two_threads();
        if let Some(len) = read_at {
            picker.show_preview(ThreadId(1), len, vec![]);
        }

        // When asking whether a 10-byte transcript needs reading.
        let wants = picker.wants_preview(10);

        // Then it does only when the read length differs.
        assert_eq!(wants, expected, "preview read at {read_at:?}");
    }

    /// A hit row with message id `id` in thread `thread`.
    fn hit(id: i64, thread: i64) -> PickerItem {
        PickerItem::Hit {
            id,
            thread: ThreadId(thread),
            label: format!("orb/thread-{thread}"),
            split: 4,
            snippet: "the IntentHandler match".to_owned(),
            lit: Vec::new(),
            text_lit: Vec::new(),
            path: PathBuf::from(format!("/t/{thread}.jsonl")),
            prompt_offset: 0,
        }
    }

    /// The search picker, with nothing typed, listing `items`.
    fn searching(items: Vec<PickerItem>) -> PickerState {
        let mut picker = PickerState::search(Focus::Sidebar);
        picker.show_hits("", items, false);
        picker
    }

    fn hit_ids(picker: &PickerState) -> Vec<i64> {
        picker
            .shown()
            .filter_map(|(item, _)| match item {
                PickerItem::Hit { id, .. } => Some(*id),
                _ => None,
            })
            .collect()
    }

    #[rstest::rstest]
    fn selected_thread_is_the_selected_hits_thread() {
        // Given the search picker over a hit in thread 7.
        let picker = searching(vec![hit(1, 7)]);

        // When asking for the selected thread.
        let thread = picker.selected_thread();

        // Then it is the hit's thread.
        assert_eq!(thread, Some(ThreadId(7)), "a hit should select its thread");
    }

    #[rstest::rstest]
    fn typing_in_the_search_picker_keeps_its_rows_and_selection() {
        // Given the search picker over two hits, the second selected.
        let mut picker = searching(vec![hit(1, 7), hit(2, 8)]);
        picker.next();

        // When typing a character.
        picker.insert('x');

        // Then the rows and the selection are untouched.
        assert_eq!(
            (hit_ids(&picker), picker.selection()),
            (vec![1, 2], 1),
            "typing should neither refilter nor reselect hits"
        );
    }

    #[rstest::rstest]
    fn show_hits_for_a_query_no_longer_typed_is_ignored() {
        // Given the search picker with `ab` typed.
        let mut picker = searching(Vec::new());
        picker.insert('a');
        picker.insert('b');

        // When the hits for `a` arrive.
        picker.show_hits("a", vec![hit(1, 7)], false);

        // Then nothing is listed.
        assert_eq!(picker.total(), 0, "a stale result should be ignored");
    }

    /// Thread `id` titled `title`, in session `id`, its last chat at second
    /// `secs`, settled when `settled`.
    fn agent(id: i64, title: &str, secs: u64, settled: bool) -> Thread {
        let at = UNIX_EPOCH + Duration::from_secs(secs);
        Thread {
            harness: HarnessId::new("claude"),
            id: ThreadId(id),
            title: Some(title.to_owned()),
            cwd: PathBuf::from(format!("/code/{id}")),
            transcript: None,
            status: ThreadStatus::Idle,
            turn_started_at: None,
            pane: Some(PaneLaunch {
                pane: PaneId(id),
                session: SessionId(id),
            }),
            last_session: Some(SessionId(id)),
            branch: None,
            pinned_at: None,
            settled_at: settled.then_some(at),
            active_since: UNIX_EPOCH,
            created_at: UNIX_EPOCH,
            last_activity_at: at,
            unseen: false,
            group: None,
            model: None,
            permission: None,
        }
    }

    /// Project `orb` holding `threads`, each in its own session.
    fn orb_sessions(threads: Vec<Thread>) -> Sessions {
        let projects = vec![Project {
            id: ProjectId(1),
            title: "orb".to_owned(),
            root: PathBuf::from("/code/orb"),
            created_at: UNIX_EPOCH,
            threads,
            repo: true,
            removed: false,
            kind: ProjectKind::Normal,
        }];
        Sessions {
            sessions: sessions_for(&projects),
            projects,
            ..Sessions::default()
        }
    }

    /// The session ids of `items`, in order.
    fn session_ids(items: &[PickerItem]) -> Vec<SessionId> {
        items
            .iter()
            .filter_map(|item| match item {
                PickerItem::Session { id, .. } => Some(*id),
                _ => None,
            })
            .collect()
    }

    #[rstest::rstest]
    fn session_items_list_sessions_newest_chat_first() {
        // Given sessions 1 and 2, session 2's agent chatting last.
        let sessions = orb_sessions(vec![agent(1, "api", 10, false), agent(2, "ui", 20, false)]);

        // When listing the session picker's rows.
        let items = session_items(&sessions, false);

        // Then session 2 comes first.
        assert_eq!(
            session_ids(&items),
            vec![SessionId(2), SessionId(1)],
            "the newest chat should lead"
        );
    }

    #[rstest::rstest]
    #[case::hidden(false, vec![SessionId(1)])]
    #[case::shown(true, vec![SessionId(2), SessionId(1)])]
    fn session_items_leave_out_settled_sessions_until_shown(
        #[case] settled: bool,
        #[case] expected: Vec<SessionId>,
    ) {
        // Given session 1 and settled session 2.
        let sessions = orb_sessions(vec![agent(1, "api", 10, false), agent(2, "ui", 20, true)]);

        // When listing the rows with settled sessions `settled`.
        let items = session_items(&sessions, settled);

        // Then the settled session shows only when asked for.
        assert_eq!(session_ids(&items), expected, "settled shown: {settled}");
    }

    #[rstest::rstest]
    fn session_row_previews_its_agent() {
        // Given session 1 running thread 1.
        let sessions = orb_sessions(vec![agent(1, "api", 10, false)]);

        // When opening the session picker over its rows.
        let picker = PickerState::sessions(session_items(&sessions, false), Focus::Sidebar);

        // Then the selected row previews thread 1.
        assert_eq!(
            picker.selected_thread(),
            Some(ThreadId(1)),
            "a session row should preview its agent"
        );
    }

    #[rstest::rstest]
    fn hit_label_names_a_threads_session() {
        // Given thread 1 in session 1, which the user named `login`.
        let mut sessions = orb_sessions(vec![agent(1, "api", 10, false)]);
        if let Some(session) = sessions.sessions.first_mut() {
            session.name = Some("login".to_owned());
        }

        // When labelling a hit in thread 1.
        let label = sessions
            .projects
            .first()
            .zip(sessions.threads().next())
            .map(|(project, thread)| hit_label(&sessions, project, thread));

        // Then the session names it and the title starts after its slash.
        assert_eq!(
            label,
            Some(("orb/login".to_owned(), 4)),
            "a hit should be labelled by its session"
        );
    }

    #[rstest::rstest]
    fn hit_label_falls_back_to_the_threads_title() {
        // Given thread 1 that never ran in a session.
        let sessions = orb_sessions(vec![Thread {
            pane: None,
            last_session: None,
            ..agent(1, "api", 10, false)
        }]);

        // When labelling a hit in thread 1.
        let label = sessions
            .projects
            .first()
            .zip(sessions.threads().next())
            .map(|(project, thread)| hit_label(&sessions, project, thread));

        // Then the thread's title names it.
        assert_eq!(
            label,
            Some(("orb/api".to_owned(), 4)),
            "a hit without a session should be labelled by its thread"
        );
    }

    #[rstest::rstest]
    fn picked_session_of_a_hit_is_its_ended_threads_session() {
        // Given thread 1 that ended in session 1, and the search picker over a
        // hit in it.
        let sessions = {
            let mut sessions = orb_sessions(vec![agent(1, "api", 10, false)]);
            if let Some(thread) = sessions
                .projects
                .first_mut()
                .and_then(|p| p.threads.first_mut())
            {
                thread.pane = None;
            }
            sessions
        };
        let picker = searching(vec![hit(1, 1)]);

        // When asking which session the pick opens.
        let session = picker.picked_session(&sessions);

        // Then it is the session the thread ran in.
        assert_eq!(
            session,
            Some(SessionId(1)),
            "a hit on an ended thread should open its session"
        );
    }
}
