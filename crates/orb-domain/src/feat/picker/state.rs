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
use crate::feat::picker::list::{
    BranchRow, LEGACY_MODELS, MODELS, Matches, Model, PERMISSION_MODES, PickerItem, PickerList,
    model,
};
use crate::feat::sessions::state::{
    GroupId, GroupKind, NEW_THREAD, ProjectId, Sessions, ThreadId, ThreadStatus,
};
use crate::feat::sessions::transcript::Exchange;
use crate::feat::worktrees::state::{User, order, users};
use crate::{AppState, Focus};

/// What a workspace or branch picker sets up: a thread, a project's draft,
/// or (branch only) a Feature group's worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickTarget {
    Thread(ThreadId),
    Draft(ProjectId),
    Group(GroupId),
}

/// What a model or permission picker sets: a project's draft, or a group's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftTarget {
    Project(ProjectId),
    Group(GroupId),
}

/// What an open picker picks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerKind {
    /// `␣n`: pick a project to start a session in.
    Projects,
    /// `␣gf`: pick the project a new Feature group is for.
    GroupProject,
    /// `␣p`: pick a directory to add as a project. `listed` is the directory
    /// text (e.g. `~/dev/`) the items were read from; `None` while the input
    /// isn't a path.
    Directories { listed: Option<String> },
    /// `␣w`: pick where `target`'s session runs.
    Workspace { target: PickTarget },
    /// `␣b`: pick a branch for `target`, whose refs are listed in `cwd`.
    /// `unstarted` is whether it has had no prompt yet, so it can still
    /// follow a branch into another worktree.
    Branches {
        target: PickTarget,
        cwd: PathBuf,
        unstarted: bool,
    },
    /// `␣m`: pick the model of `target`'s draft.
    Model { target: DraftTarget },
    /// `␣w` or `␣b` on a draft whose project isn't a git repository: make it
    /// one.
    InitGit { project: ProjectId },
    /// `␣a`: pick the permission mode of `target`'s draft.
    Permission { target: DraftTarget },
    /// `␣f`: pick the project the sidebar is filtered to, or all of them.
    ProjectFilter,
    /// `<C-x>` in the project filter: confirm removing `project`.
    RemoveProject { project: ProjectId },
    /// `s` on a thread: confirm settling it.
    SettleThread { thread: ThreadId },
    /// `d` on a thread: confirm deleting it.
    DeleteThread { thread: ThreadId },
    /// `d` on a draft: confirm discarding it.
    DiscardDraft { project: ProjectId },
    /// `s` on a group's card: confirm settling it.
    SettleGroup { group: GroupId },
    /// `d` on a group's card: confirm deleting it, its threads and its
    /// directory. `dir` is the group's kind when it has a directory on disk
    /// (a Feature group's worktree once started, a Research/Learn folder),
    /// else `None`.
    DeleteGroup {
        group: GroupId,
        dir: Option<GroupKind>,
    },
    /// Claude refused a session start in `dir`, Claude's project path for it
    /// (the git root, the main repository for a worktree, else the folder):
    /// confirm trusting it.
    TrustWorkspace { dir: PathBuf },
    /// `␣␣`/`<C-Space>`: pick a thread to jump into. `settled` is whether
    /// settled threads are listed; `<C-s>` flips it.
    Sessions { settled: bool },
    /// `␣sw`: orb's worktrees, to look over and delete.
    Worktrees,
    /// `<C-x>` in the worktree picker: confirm force-removing the worktree at
    /// `path`. `dirty` is whether it has uncommitted changes.
    DeleteWorktree { path: PathBuf, dirty: bool },
}

/// What the session picker's preview shows for a thread: its transcript's
/// last exchanges, and the transcript's length when they were read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPreview {
    pub thread: ThreadId,
    pub len: u64,
    pub exchanges: Vec<Exchange>,
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

    /// A project picker for a new Feature group over `items`, in the order
    /// given.
    pub fn group_project(items: Vec<PickerItem>, return_to: Focus) -> Self {
        Self {
            kind: PickerKind::GroupProject,
            ..Self::projects(items, return_to)
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
            under: None,
        }
    }

    /// The `No`/`Yes` confirm for removing `project`, with `No` selected.
    pub fn remove_project(project: ProjectId, return_to: Focus) -> Self {
        Self::confirm(PickerKind::RemoveProject { project }, return_to)
    }

    /// The `No`/`Yes` confirm for settling `thread`, with `No` selected.
    pub fn settle_thread(thread: ThreadId, return_to: Focus) -> Self {
        Self::confirm(PickerKind::SettleThread { thread }, return_to)
    }

    /// The `No`/`Yes` confirm for deleting `thread`, with `No` selected.
    pub fn delete_thread(thread: ThreadId, return_to: Focus) -> Self {
        Self::confirm(PickerKind::DeleteThread { thread }, return_to)
    }

    /// The `No`/`Yes` confirm for discarding `project`'s draft, with `No`
    /// selected.
    pub fn discard_draft(project: ProjectId, return_to: Focus) -> Self {
        Self::confirm(PickerKind::DiscardDraft { project }, return_to)
    }

    /// The `No`/`Yes` confirm for settling `group`, with `No` selected.
    pub fn settle_group(group: GroupId, return_to: Focus) -> Self {
        Self::confirm(PickerKind::SettleGroup { group }, return_to)
    }

    /// The `No`/`Yes` confirm for deleting `group` and, when `dir` is some,
    /// its directory of that kind, with `No` selected.
    pub fn delete_group(group: GroupId, dir: Option<GroupKind>, return_to: Focus) -> Self {
        Self::confirm(PickerKind::DeleteGroup { group, dir }, return_to)
    }

    /// The `No`/`Yes` confirm for trusting `dir`, with `No` selected.
    pub fn trust_workspace(dir: PathBuf, return_to: Focus) -> Self {
        Self::confirm(PickerKind::TrustWorkspace { dir }, return_to)
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
            under: None,
        }
    }

    /// A branch picker for `target` in `cwd`, empty until its refs are
    /// listed; then `wanted` is selected if listed, else the first branch.
    pub fn branches(
        target: PickTarget,
        cwd: PathBuf,
        unstarted: bool,
        wanted: Option<String>,
        return_to: Focus,
    ) -> Self {
        Self {
            kind: PickerKind::Branches {
                target,
                cwd,
                unstarted,
            },
            list: PickerList::default(),
            return_to,
            home: PathBuf::new(),
            page: 0,
            wanted,
            preview: None,
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
            under: None,
        }
    }

    /// A model picker for `target`'s draft: `Default`, then [`MODELS`], then
    /// a `Legacy models` heading over [`LEGACY_MODELS`], with `current`
    /// selected, also when it's one of a model's aliases.
    pub fn models(target: DraftTarget, current: Option<&str>, return_to: Focus) -> Self {
        let ids = |models: &[Model]| {
            models
                .iter()
                .map(|model| PickerItem::Setting(Some(model.id)))
                .collect::<Vec<_>>()
        };
        let items = std::iter::once(PickerItem::Setting(None))
            .chain(ids(&MODELS))
            .chain(std::iter::once(PickerItem::Heading("Legacy models")))
            .chain(ids(&LEGACY_MODELS))
            .collect();
        let current = current.map(|value| model(value).map_or(value, |model| model.id));
        Self::settings(PickerKind::Model { target }, items, current, return_to)
    }

    /// A permission-mode picker for `target`'s draft: `Default`, then
    /// [`PERMISSION_MODES`], with `current` selected.
    pub fn permissions(target: DraftTarget, current: Option<&str>, return_to: Focus) -> Self {
        let items = std::iter::once(None)
            .chain(PERMISSION_MODES.map(Some))
            .map(PickerItem::Setting)
            .collect();
        Self::settings(PickerKind::Permission { target }, items, current, return_to)
    }

    /// A `kind` picker over `items`, with the setting `current` selected, else
    /// the first.
    fn settings(
        kind: PickerKind,
        items: Vec<PickerItem>,
        current: Option<&str>,
        return_to: Focus,
    ) -> Self {
        let list = {
            let selected = items
                .iter()
                .find(|item| matches!(item, PickerItem::Setting(value) if *value == current))
                .cloned();
            let mut list = PickerList::new(items);
            if let Some(selected) = selected {
                list.select(&selected);
            }
            list
        };
        Self {
            kind,
            list,
            return_to,
            home: PathBuf::new(),
            page: 0,
            wanted: None,
            preview: None,
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

    /// Shows `refs`, listed in `cwd`, filtered by the typed text, selecting
    /// the wanted branch when nothing is typed. Ignored unless this is `cwd`'s
    /// branch picker. Once the thread has had a prompt, a branch checked out
    /// in another worktree is disabled.
    pub fn show_branches(&mut self, cwd: &Path, refs: Vec<GitRef>) {
        let PickerKind::Branches {
            cwd: picker_cwd,
            unstarted,
            ..
        } = &self.kind
        else {
            return;
        };
        if picker_cwd != cwd {
            return;
        }
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

    /// The selected row's thread; `None` unless a thread row is selected.
    pub fn selected_thread(&self) -> Option<ThreadId> {
        match self.list.selected() {
            Some(&PickerItem::Thread { id, .. }) => Some(id),
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

    /// Forgets the preview.
    pub fn clear_preview(&mut self) {
        self.preview = None;
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
                | PickerItem::Setting(_)
                | PickerItem::Heading(_)
                | PickerItem::InitGit
                | PickerItem::AllProjects
                | PickerItem::Confirm(_)
                | PickerItem::Thread { .. }
                | PickerItem::Worktree { .. },
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

    /// After an edit: re-filter, or start browsing the new directory.
    fn edited(&mut self) -> Option<PathBuf> {
        let input = self.list.input().to_owned();
        let PickerKind::Directories { listed } = &mut self.kind else {
            self.list.refilter(&input);
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

/// The session picker's rows: the threads of every project inside the
/// project filter (removed projects too), newest chat first by the later of
/// the current turn's start and the last turn's end, ties to the higher id.
/// The selected thread, threads being deleted, `Gone` threads, and settled
/// threads (or threads of a settled group) unless `settled`, are left out.
pub fn session_items(sessions: &Sessions, settled: bool) -> Vec<PickerItem> {
    let selected = sessions.selected_id();
    let mut rows: Vec<(SystemTime, i64, PickerItem)> = sessions
        .projects
        .iter()
        .filter(|project| sessions.filter.is_none_or(|filter| filter == project.id))
        .flat_map(|project| project.threads.iter().map(move |thread| (project, thread)))
        .filter_map(|(project, thread)| {
            let group = thread
                .group
                .and_then(|id| project.groups.iter().find(|group| group.id == id));
            let is_settled = thread.settled_at.is_some()
                || group.is_some_and(|group| group.settled_at.is_some());
            let left_out = Some(thread.id) == selected
                || sessions.deleting.contains(&thread.id)
                || thread.status == ThreadStatus::Gone
                || (is_settled && !settled);
            if left_out {
                return None;
            }
            let prefix = group.map_or(project.title.as_str(), |group| group.name.as_str());
            let title = thread.title.as_deref().unwrap_or(NEW_THREAD);
            Some((
                thread.last_chat(),
                thread.id.0,
                PickerItem::Thread {
                    id: thread.id,
                    label: format!("{prefix}/{title}"),
                    split: prefix.len() + 1,
                    settled: is_settled,
                },
            ))
        })
        .collect();
    rows.sort_by_key(|(last_chat, id, _)| Reverse((*last_chat, *id)));
    rows.into_iter().map(|(_, _, item)| item).collect()
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
            let titles = users.iter().filter_map(|user| match user {
                User::Thread(_, thread) => thread.title.as_deref(),
                User::Group(_, group, _) => Some(group.name.as_str()),
                User::Draft(_) => None,
            });
            PickerItem::Worktree {
                path: worktree.path.clone(),
                label: format!("{repo}/{name}"),
                split: repo.len() + 1,
                extra: branch
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
    use std::path::{Path, PathBuf};

    use super::{DraftTarget, PickTarget, PickerState, expand, split_path};
    use crate::Focus;
    use crate::feat::git::git_service::GitRef;
    use crate::feat::picker::list::{LEGACY_MODELS, PickerItem};
    use crate::feat::sessions::state::{GroupId, ProjectId, ProjectKind, ThreadId};
    use crate::feat::sessions::transcript::Exchange;

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

    /// Thread 1's branch picker in [`CWD`], showing `refs`.
    fn branches_listing(unstarted: bool, refs: Vec<GitRef>) -> PickerState {
        let mut picker = PickerState::branches(
            PickTarget::Thread(ThreadId(1)),
            CWD.into(),
            unstarted,
            None,
            Focus::Dashboard,
        );
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

    /// The labels of the shown setting rows.
    fn setting_labels(picker: &PickerState) -> Vec<Option<&'static str>> {
        picker
            .shown()
            .filter_map(|(item, _)| match item {
                PickerItem::Setting(value) => Some(*value),
                _ => None,
            })
            .collect()
    }

    #[rstest::rstest]
    fn model_picker_lists_default_then_the_models() {
        // Given / When opening a model picker for a draft with no model.
        let picker =
            PickerState::models(DraftTarget::Project(ProjectId(1)), None, Focus::Dashboard);

        // Then Default comes first, then every model ID, current then legacy.
        assert_eq!(
            setting_labels(&picker),
            [
                None,
                Some("claude-opus-5-5"),
                Some("claude-fable-5-1"),
                Some("claude-opus-5"),
                Some("claude-sonnet-5"),
                Some("claude-fable-5"),
                Some("claude-opus-4-8"),
                Some("claude-opus-4-7"),
                Some("claude-opus-4-6"),
                Some("claude-opus-4-5"),
                Some("claude-sonnet-4-6"),
                Some("claude-haiku-4-5"),
            ],
            "the model picker lists Default then the model IDs"
        );
    }

    #[rstest::rstest]
    fn model_picker_heads_the_legacy_models() {
        // Given / When opening a model picker.
        let picker =
            PickerState::models(DraftTarget::Project(ProjectId(1)), None, Focus::Dashboard);

        // Then the Legacy models heading sits between Sonnet 5 and Fable 5.
        let rows: Vec<&PickerItem> = picker.shown().map(|(item, _)| item).collect();
        assert_eq!(
            rows.get(4..7),
            Some(
                [
                    &PickerItem::Setting(Some("claude-sonnet-5")),
                    &PickerItem::Heading("Legacy models"),
                    &PickerItem::Setting(Some("claude-fable-5")),
                ]
                .as_slice()
            ),
            "the legacy models should follow their heading"
        );
    }

    #[rstest::rstest]
    fn moving_down_skips_the_legacy_heading() {
        // Given a model picker on Claude Sonnet 5, the last current model.
        let mut picker = PickerState::models(
            DraftTarget::Project(ProjectId(1)),
            Some("claude-sonnet-5"),
            Focus::Dashboard,
        );

        // When moving down.
        picker.next();

        // Then the first legacy model is selected, not the heading.
        assert_eq!(
            picker.selected(),
            Some(&PickerItem::Setting(Some("claude-fable-5"))),
            "the heading can't be selected"
        );
    }

    #[rstest::rstest]
    fn moving_down_from_the_last_model_wraps_to_default() {
        // Given a model picker on the last legacy model.
        let last = LEGACY_MODELS.last().map(|model| model.id);
        let mut picker =
            PickerState::models(DraftTarget::Project(ProjectId(1)), last, Focus::Dashboard);

        // When moving down.
        picker.next();

        // Then Default, the first row, is selected.
        assert_eq!(
            picker.selected(),
            Some(&PickerItem::Setting(None)),
            "moving down from the last model should wrap to Default"
        );
    }

    #[rstest::rstest]
    fn model_picker_selects_the_current_legacy_model() {
        // Given / When opening a model picker for a draft on Claude Haiku 4.5.
        let picker = PickerState::models(
            DraftTarget::Project(ProjectId(1)),
            Some("claude-haiku-4-5"),
            Focus::Dashboard,
        );

        // Then Claude Haiku 4.5 is selected.
        assert_eq!(
            picker.selected(),
            Some(&PickerItem::Setting(Some("claude-haiku-4-5"))),
            "the draft's legacy model should be selected"
        );
    }

    #[rstest::rstest]
    #[case("opus", "claude-opus-5")]
    #[case("sonnet", "claude-sonnet-5")]
    #[case("haiku", "claude-haiku-4-5")]
    #[case("fable", "claude-fable-5-1")]
    fn model_picker_selects_the_model_an_alias_names(
        #[case] alias: &str,
        #[case] id: &'static str,
    ) {
        // Given / When opening a model picker for a draft on an alias.
        let picker = PickerState::models(
            DraftTarget::Project(ProjectId(1)),
            Some(alias),
            Focus::Dashboard,
        );

        // Then that model is selected.
        assert_eq!(
            picker.selected(),
            Some(&PickerItem::Setting(Some(id))),
            "{alias} should select {id}"
        );
    }

    #[rstest::rstest]
    fn model_picker_selects_default_for_an_unknown_model() {
        // Given / When opening a model picker for a draft on a value no model has.
        let picker = PickerState::models(
            DraftTarget::Project(ProjectId(1)),
            Some("opus[1m]"),
            Focus::Dashboard,
        );

        // Then Default is selected.
        assert_eq!(
            picker.selected(),
            Some(&PickerItem::Setting(None)),
            "a model not in the list should select Default"
        );
    }

    #[rstest::rstest]
    fn permission_picker_selects_the_current_mode() {
        // Given / When opening a permission picker for a draft in plan mode.
        let picker = PickerState::permissions(
            DraftTarget::Project(ProjectId(1)),
            Some("plan"),
            Focus::Dashboard,
        );

        // Then plan is selected.
        assert_eq!(
            picker.selected(),
            Some(&PickerItem::Setting(Some("plan"))),
            "the draft's current mode should be selected"
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
    fn settle_group_confirm_starts_on_no() {
        // Given / When opening the confirm for settling group 9.
        let picker = PickerState::settle_group(GroupId(9), Focus::Sidebar);

        // Then No is highlighted, so ⏎ alone settles nothing.
        assert_eq!(
            picker.selected(),
            Some(&PickerItem::Confirm(false)),
            "No should be the default"
        );
    }

    #[rstest::rstest]
    fn trust_workspace_confirm_starts_on_no() {
        // Given / When opening the confirm for trusting /work.
        let picker = PickerState::trust_workspace(PathBuf::from("/work"), Focus::Sidebar);

        // Then No is highlighted, so ⏎ alone trusts nothing.
        assert_eq!(
            picker.selected(),
            Some(&PickerItem::Confirm(false)),
            "No should be the default"
        );
    }

    /// The session picker over threads 1 (`work/alpha`) and 2 (`work/zulu`),
    /// with thread 1 selected.
    fn two_threads() -> PickerState {
        let row = |id, title: &str| PickerItem::Thread {
            id: ThreadId(id),
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
}
