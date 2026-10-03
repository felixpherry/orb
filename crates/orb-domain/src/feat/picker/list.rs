//! The list inside a picker: the typed filter text, the items it narrows, and
//! the selection among the ones shown.

use std::path::PathBuf;

use fuzzy_matcher::FuzzyMatcher as _;
use fuzzy_matcher::skim::SkimMatcherV2;

use crate::TextInput;
use crate::feat::git::git_service::GitRef;
use crate::feat::sessions::state::{ProjectId, ProjectKind, ThreadId};

/// One row a picker can show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerItem {
    /// A project, matched on its name and path; one of orb's own (Research,
    /// Learn) only on its name.
    Project {
        id: ProjectId,
        title: String,
        root: PathBuf,
        kind: ProjectKind,
    },
    /// A subdirectory, matched on its name.
    Directory { name: String },
    /// Where a thread's session could run, matched on its label.
    Workspace(WorkspaceChoice),
    /// A branch to switch to, matched on its name.
    Branch(BranchRow),
    /// A model or permission mode for a draft; `None` is Claude's default.
    /// Matched on its label.
    Setting(Option<&'static str>),
    /// A section label between rows. It can't be selected, and it's hidden
    /// while a filter is typed.
    Heading(&'static str),
    /// Make the draft's project a git repository. Matched on its label.
    InitGit,
    /// The project filter's row for no filter. Matched on its label.
    AllProjects,
    /// A confirm picker's answer: `Yes` or `No`. Matched on its label.
    Confirm(bool),
    /// A thread in the session picker, labelled `<project|group>/title` and
    /// matched on the whole label. `split` is the byte offset where the title
    /// starts. `settled` is whether it, or its group, is settled.
    Thread {
        id: ThreadId,
        label: String,
        split: usize,
        settled: bool,
    },
}

/// The text of the [`PickerItem::InitGit`] row, as in T3 Code.
pub const INIT_GIT: &str = "Initialize Git";

/// The text of the [`PickerItem::AllProjects`] row.
pub const ALL_PROJECTS: &str = "All projects";

/// The text of a [`PickerItem::Confirm`] row.
pub fn confirm_label(yes: bool) -> &'static str {
    if yes { "Yes" } else { "No" }
}

/// A Claude model a draft can pick: the full ID passed as `--model`, the
/// name shown for it, and the other values T3 Code's manifest maps to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    pub id: &'static str,
    pub name: &'static str,
    /// Other names for the model, such as `opus`, that a draft or thread may
    /// have stored.
    pub aliases: &'static [&'static str],
}

/// The current models, in T3 Code's order.
pub const MODELS: [Model; 4] = [
    Model {
        id: "claude-opus-5-5",
        name: "Claude Opus 5.5",
        aliases: &["opus-5.5", "claude-opus-5.5"],
    },
    Model {
        id: "claude-fable-5-1",
        name: "Claude Fable 5.1",
        aliases: &["fable", "fable-5.1", "claude-fable-5.1"],
    },
    Model {
        id: "claude-opus-5",
        name: "Claude Opus 5",
        aliases: &["opus", "opus-5", "claude-opus-5.0", "claude-opus-5-0"],
    },
    Model {
        id: "claude-sonnet-5",
        name: "Claude Sonnet 5",
        aliases: &[
            "sonnet",
            "sonnet-5",
            "claude-sonnet-5.0",
            "claude-sonnet-5-0",
        ],
    },
];

/// The older models T3 Code files under "Legacy models", in its order.
pub const LEGACY_MODELS: [Model; 7] = [
    Model {
        id: "claude-fable-5",
        name: "Claude Fable 5",
        aliases: &[],
    },
    Model {
        id: "claude-opus-4-8",
        name: "Claude Opus 4.8",
        aliases: &["opus-4.8", "claude-opus-4.8"],
    },
    Model {
        id: "claude-opus-4-7",
        name: "Claude Opus 4.7",
        aliases: &["opus-4.7", "claude-opus-4.7"],
    },
    Model {
        id: "claude-opus-4-6",
        name: "Claude Opus 4.6",
        aliases: &["opus-4.6", "claude-opus-4.6", "claude-opus-4-6-20251117"],
    },
    Model {
        id: "claude-opus-4-5",
        name: "Claude Opus 4.5",
        aliases: &[],
    },
    Model {
        id: "claude-sonnet-4-6",
        name: "Claude Sonnet 4.6",
        aliases: &[
            "sonnet-4.6",
            "claude-sonnet-4.6",
            "claude-sonnet-4-6-20251117",
        ],
    },
    Model {
        id: "claude-haiku-4-5",
        name: "Claude Haiku 4.5",
        aliases: &[
            "haiku",
            "haiku-4.5",
            "claude-haiku-4.5",
            "claude-haiku-4-5-20251001",
        ],
    },
];

/// The `--permission-mode` values a draft can pick, besides Claude's default.
pub const PERMISSION_MODES: [&str; 6] = [
    "acceptEdits",
    "auto",
    "bypassPermissions",
    "manual",
    "dontAsk",
    "plan",
];

/// The model `value` names, by its ID or one of its aliases.
pub fn model(value: &str) -> Option<&'static Model> {
    MODELS
        .iter()
        .chain(&LEGACY_MODELS)
        .find(|model| model.id == value || model.aliases.contains(&value))
}

/// A model's or permission mode's text: a known model's name (also for an
/// alias), else the value as stored, or `Default` for none.
pub fn setting_label(value: Option<&str>) -> &str {
    match value {
        None => "Default",
        Some(value) => model(value).map_or(value, |model| model.name),
    }
}

impl PickerItem {
    /// Whether the row is shown but can't be selected.
    pub fn disabled(&self) -> bool {
        matches!(
            self,
            Self::Branch(BranchRow { disabled: true, .. }) | Self::Heading(_)
        )
    }
}

/// A row of the branch picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchRow {
    pub git_ref: GitRef,
    /// The branch is checked out in another worktree and the thread can no
    /// longer follow it there.
    pub disabled: bool,
}

/// A row of the workspace picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceChoice {
    /// Stay where the thread is: the root checkout, or a worktree.
    Current { worktree: bool },
    /// A new worktree of the project.
    NewWorktree,
    /// The project's previous worktree, and its branch if known.
    Previous {
        path: PathBuf,
        branch: Option<String>,
    },
}

impl WorkspaceChoice {
    /// The row's text.
    pub fn label(&self) -> String {
        match self {
            Self::Current { worktree: false } => "Current checkout".to_owned(),
            Self::Current { worktree: true } => "Current worktree".to_owned(),
            Self::NewWorktree => "New worktree".to_owned(),
            Self::Previous {
                branch: Some(branch),
                ..
            } => format!("Previous worktree ({branch})"),
            Self::Previous { branch: None, .. } => "Previous worktree".to_owned(),
        }
    }
}

/// Where the filter matched a shown item: byte offsets into its title (or
/// directory name) and into its path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Matches {
    pub name: Vec<usize>,
    pub path: Vec<usize>,
}

/// The filter text with its cursor, the items, and which of them are shown.
#[derive(Debug, Default)]
pub struct PickerList {
    /// The typed filter text and its cursor.
    input: TextInput,
    /// An index into `shown`.
    selection: usize,
    items: Vec<PickerItem>,
    /// Item indices and where they matched, in display order.
    shown: Vec<(usize, Matches)>,
}

impl PickerList {
    /// A list with nothing typed, showing every item in order.
    pub fn new(items: Vec<PickerItem>) -> Self {
        let mut list = Self::default();
        list.set_items(items, "");
        list
    }

    /// Replaces the items and filters them with `pattern`.
    pub fn set_items(&mut self, items: Vec<PickerItem>, pattern: &str) {
        self.items = items;
        self.refilter(pattern);
    }

    /// Shows the items matching every whitespace-separated term of `pattern`,
    /// best score first, list order breaking ties, and selects the first that
    /// isn't disabled.
    ///
    /// Directories whose name starts with `.` stay hidden unless `pattern`
    /// does too.
    pub fn refilter(&mut self, pattern: &str) {
        let terms: Vec<&str> = pattern.split_whitespace().collect();
        let matcher = SkimMatcherV2::default();
        let mut scored: Vec<(i64, usize, Matches)> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| !hidden(item, pattern))
            .filter_map(|(index, item)| {
                score(&matcher, item, &terms).map(|(score, matches)| (score, index, matches))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        self.shown = scored
            .into_iter()
            .map(|(_, index, matches)| (index, matches))
            .collect();
        self.selection = (0..self.shown.len())
            .find(|&index| self.enabled(index))
            .unwrap_or(0);
    }

    /// Replaces the filter text, leaving the cursor at its end.
    pub fn replace_input(&mut self, text: &str) {
        self.input = TextInput::new(text);
    }

    /// Types `ch` at the cursor. Line breaks are dropped.
    pub fn insert(&mut self, ch: char) {
        self.input.insert(ch);
    }

    /// Deletes the grapheme before the cursor.
    pub fn backspace(&mut self) {
        self.input.backspace();
    }

    /// Deletes back from the cursor to the previous `/` or space. A `/` or
    /// space right before the cursor goes with it: `~/dev/` becomes `~/`.
    pub fn delete_word(&mut self) {
        self.input.delete_word();
    }

    /// Moves the cursor one grapheme left.
    pub fn cursor_left(&mut self) {
        self.input.cursor_left();
    }

    /// Moves the cursor one grapheme right.
    pub fn cursor_right(&mut self) {
        self.input.cursor_right();
    }

    /// Selects the next shown item, wrapping from the last to the first.
    /// Disabled items are skipped here and by every other move.
    pub fn next(&mut self) {
        self.cycle(false);
    }

    /// Selects the previous shown item, wrapping from the first to the last.
    pub fn prev(&mut self) {
        self.cycle(true);
    }

    /// Moves the selection down half of the `page` items that fit on screen.
    pub fn half_page_down(&mut self, page: usize) {
        self.move_down((page / 2).max(1));
    }

    /// Moves the selection up half of the `page` items that fit on screen.
    pub fn half_page_up(&mut self, page: usize) {
        self.move_up((page / 2).max(1));
    }

    /// Selects shown row `index` when it's there and can be selected;
    /// otherwise stays.
    pub fn select_row(&mut self, index: usize) {
        if self.enabled(index) {
            self.selection = index;
        }
    }

    /// Selects the nearest enabled item below the selection, staying on
    /// the last one.
    pub fn select_below(&mut self) {
        self.move_down(1);
    }

    /// Selects the nearest enabled item above the selection, staying on
    /// the first one.
    pub fn select_above(&mut self) {
        self.move_up(1);
    }

    /// The typed filter text.
    pub fn input(&self) -> &str {
        self.input.text()
    }

    /// The cursor, as a grapheme index into [`input`](Self::input).
    pub fn cursor(&self) -> usize {
        self.input.cursor()
    }

    /// The selected row, as an index into [`shown`](Self::shown).
    pub fn selection(&self) -> usize {
        self.selection
    }

    /// The selected item; `None` when nothing selectable is shown.
    pub fn selected(&self) -> Option<&PickerItem> {
        let (index, _) = self.shown.get(self.selection)?;
        self.items.get(*index).filter(|item| !item.disabled())
    }

    /// Selects the shown row holding `item`; stays put when none does.
    pub fn select(&mut self, item: &PickerItem) {
        if let Some(found) = (0..self.shown.len())
            .find(|&index| self.enabled(index) && self.item_at(index) == Some(item))
        {
            self.selection = found;
        }
    }

    /// The shown items and where they matched, in display order.
    pub fn shown(&self) -> impl Iterator<Item = (&PickerItem, &Matches)> {
        self.shown
            .iter()
            .filter_map(|(index, matches)| self.items.get(*index).map(|item| (item, matches)))
    }

    /// How many items are shown.
    pub fn len(&self) -> usize {
        self.shown.len()
    }

    /// How many items there are, shown or not.
    pub fn total(&self) -> usize {
        self.items.len()
    }

    /// Whether no item is shown.
    pub fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }

    /// Selects the nearest enabled item after the selection, or before it
    /// when `backwards`, wrapping around the ends; stays when there's none.
    fn cycle(&mut self, backwards: bool) {
        let count = self.shown.len();
        let index = |offset: usize| {
            if backwards {
                (self.selection + count - offset) % count
            } else {
                (self.selection + offset) % count
            }
        };
        if let Some(found) = (1..count).map(index).find(|&index| self.enabled(index)) {
            self.selection = found;
        }
    }

    /// Selects the first enabled item from `step` rows down, else the nearest
    /// enabled one before it, else stays.
    fn move_down(&mut self, step: usize) {
        let last = self.shown.len().saturating_sub(1);
        let target = (self.selection + step).min(last);
        if let Some(found) = (target..=last)
            .chain((self.selection + 1..target).rev())
            .find(|&index| self.enabled(index))
        {
            self.selection = found;
        }
    }

    /// Selects the first enabled item from `step` rows up, else the nearest
    /// enabled one after it, else stays.
    fn move_up(&mut self, step: usize) {
        let target = self.selection.saturating_sub(step);
        if let Some(found) = (0..=target)
            .rev()
            .chain(target + 1..self.selection)
            .find(|&index| self.enabled(index))
        {
            self.selection = found;
        }
    }

    /// Whether shown row `index` exists and isn't disabled.
    fn enabled(&self, index: usize) -> bool {
        self.item_at(index).is_some_and(|item| !item.disabled())
    }

    /// The item on shown row `index`.
    fn item_at(&self, index: usize) -> Option<&PickerItem> {
        let (item, _) = self.shown.get(index)?;
        self.items.get(*item)
    }
}

fn hidden(item: &PickerItem, pattern: &str) -> bool {
    match item {
        PickerItem::Directory { name } => name.starts_with('.') && !pattern.starts_with('.'),
        PickerItem::Heading(_) => !pattern.trim().is_empty(),
        PickerItem::Project { .. }
        | PickerItem::Workspace(_)
        | PickerItem::Branch(_)
        | PickerItem::Setting(_)
        | PickerItem::InitGit
        | PickerItem::AllProjects
        | PickerItem::Confirm(_)
        | PickerItem::Thread { .. } => false,
    }
}

/// The summed score of every term in `label`, and the sorted byte offsets of
/// the characters they matched; `None` when a term doesn't match.
pub fn fuzzy_match(
    matcher: &SkimMatcherV2,
    label: &str,
    terms: &[&str],
) -> Option<(i64, Vec<usize>)> {
    let bytes: Vec<usize> = label.char_indices().map(|(at, _)| at).collect();
    let mut total = 0;
    let mut offsets = Vec::new();
    for term in terms {
        let (score, chars) = matcher.fuzzy_indices(label, term)?;
        total += score;
        offsets.extend(chars.iter().filter_map(|&index| bytes.get(index).copied()));
    }
    offsets.sort_unstable();
    offsets.dedup();
    Some((total, offsets))
}

/// The summed score and match offsets of `item` when every term matches.
fn score(matcher: &SkimMatcherV2, item: &PickerItem, terms: &[&str]) -> Option<(i64, Matches)> {
    // A project is matched on "title\nroot". Typed text never holds a line
    // break, so every offset falls on one side of it.
    let (label, title_len) = match item {
        PickerItem::Project {
            title,
            kind: ProjectKind::Research | ProjectKind::Learn | ProjectKind::Incognito,
            ..
        } => (title.clone(), None),
        PickerItem::Project { title, root, .. } => {
            (format!("{title}\n{}", root.display()), Some(title.len()))
        }
        PickerItem::Directory { name } => (name.clone(), None),
        PickerItem::Workspace(choice) => (choice.label(), None),
        PickerItem::Branch(row) => (row.git_ref.name.clone(), None),
        PickerItem::Setting(value) => (setting_label(*value).to_owned(), None),
        PickerItem::Heading(text) => ((*text).to_owned(), None),
        PickerItem::InitGit => (INIT_GIT.to_owned(), None),
        PickerItem::AllProjects => (ALL_PROJECTS.to_owned(), None),
        PickerItem::Confirm(yes) => (confirm_label(*yes).to_owned(), None),
        PickerItem::Thread { label, .. } => (label.clone(), None),
    };
    let (total, offsets) = fuzzy_match(matcher, &label, terms)?;
    let found = match title_len {
        Some(title_len) => {
            let (name, path): (Vec<usize>, Vec<usize>) =
                offsets.into_iter().partition(|&at| at < title_len);
            Matches {
                name,
                path: path
                    .into_iter()
                    .filter_map(|at| at.checked_sub(title_len + 1))
                    .collect(),
            }
        }
        None => Matches {
            name: offsets,
            path: Vec::new(),
        },
    };
    Some((total, found))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{BranchRow, Matches, PickerItem, PickerList, setting_label};
    use crate::feat::git::git_service::GitRef;
    use crate::feat::sessions::state::{ProjectId, ProjectKind};

    fn directories(names: &[&str]) -> Vec<PickerItem> {
        names
            .iter()
            .map(|name| PickerItem::Directory {
                name: (*name).to_owned(),
            })
            .collect()
    }

    fn project(id: i64, title: &str, root: &str) -> PickerItem {
        PickerItem::Project {
            id: ProjectId(id),
            title: title.to_owned(),
            root: PathBuf::from(root),
            kind: ProjectKind::Normal,
        }
    }

    fn shown_names(list: &PickerList) -> Vec<String> {
        list.shown()
            .map(|(item, _)| match item {
                PickerItem::Directory { name } => name.clone(),
                PickerItem::Project { title, .. } => title.clone(),
                PickerItem::Workspace(choice) => choice.label(),
                PickerItem::Branch(row) => row.git_ref.name.clone(),
                PickerItem::Setting(value) => setting_label(*value).to_owned(),
                PickerItem::Heading(text) => (*text).to_owned(),
                PickerItem::InitGit => super::INIT_GIT.to_owned(),
                PickerItem::AllProjects => super::ALL_PROJECTS.to_owned(),
                PickerItem::Confirm(yes) => super::confirm_label(*yes).to_owned(),
                PickerItem::Thread { label, .. } => label.clone(),
            })
            .collect()
    }

    /// Local branches named `names`; a leading `!` makes the row disabled.
    fn branches(names: &[&str]) -> Vec<PickerItem> {
        names
            .iter()
            .map(|name| {
                let disabled = name.starts_with('!');
                PickerItem::Branch(BranchRow {
                    git_ref: GitRef {
                        name: name.trim_start_matches('!').to_owned(),
                        remote: false,
                        current: false,
                        default: false,
                        worktree: None,
                    },
                    disabled,
                })
            })
            .collect()
    }

    fn selected_name(list: &PickerList) -> Option<String> {
        match list.selected() {
            Some(PickerItem::Branch(row)) => Some(row.git_ref.name.clone()),
            _ => None,
        }
    }

    #[rstest::rstest]
    fn empty_pattern_shows_every_item_in_list_order() {
        // Given a list of directories.
        let mut list = PickerList::new(directories(&["zeta", "alpha", "mid"]));

        // When filtering with an empty pattern.
        list.refilter("");

        // Then every item is shown in list order.
        assert_eq!(
            shown_names(&list),
            ["zeta", "alpha", "mid"],
            "an empty pattern should keep list order"
        );
    }

    #[rstest::rstest]
    fn pattern_shows_only_matches_best_score_first() {
        // Given a scattered match, a non-match, and an exact match.
        let mut list = PickerList::new(directories(&["o_r_b", "web", "orb"]));

        // When filtering with "orb".
        list.refilter("orb");

        // Then only the matches are shown, the exact one first.
        assert_eq!(
            shown_names(&list),
            ["orb", "o_r_b"],
            "matches should rank by score"
        );
    }

    #[rstest::rstest]
    fn equal_scores_keep_list_order() {
        // Given two projects that match "orb" equally well.
        let mut list = PickerList::new(vec![project(2, "orb", "/x/a"), project(1, "orb", "/x/b")]);

        // When filtering with "orb".
        list.refilter("orb");

        // Then the earlier item comes first.
        let ids: Vec<ProjectId> = list
            .shown()
            .filter_map(|(item, _)| match item {
                PickerItem::Project { id, .. } => Some(*id),
                PickerItem::Directory { .. }
                | PickerItem::Workspace(_)
                | PickerItem::Branch(_)
                | PickerItem::Setting(_)
                | PickerItem::Heading(_)
                | PickerItem::InitGit
                | PickerItem::AllProjects
                | PickerItem::Confirm(_)
                | PickerItem::Thread { .. } => None,
            })
            .collect();
        assert_eq!(
            ids,
            [ProjectId(2), ProjectId(1)],
            "ties should keep list order"
        );
    }

    #[rstest::rstest]
    fn every_term_must_match() {
        // Given directories matching one or both terms.
        let mut list = PickerList::new(directories(&["orb-web", "orb", "web"]));

        // When filtering with two terms.
        list.refilter("orb web");

        // Then only the item matching both is shown.
        assert_eq!(
            shown_names(&list),
            ["orb-web"],
            "an item should match every term"
        );
    }

    #[rstest::rstest]
    fn non_ascii_title_matches_at_byte_offsets() {
        // Given a project whose title has a two-byte `é`.
        let mut list = PickerList::new(vec![project(1, "café-app", "/x")]);

        // When filtering with "app".
        list.refilter("app");

        // Then the offsets are the bytes of "app", after the `é`.
        let matches: Vec<&Matches> = list.shown().map(|(_, matches)| matches).collect();
        assert_eq!(
            matches,
            [&Matches {
                name: vec![6, 7, 8],
                path: Vec::new(),
            }],
            "char indices should become byte offsets"
        );
    }

    #[rstest::rstest]
    fn project_path_match_is_offset_into_the_path() {
        // Given a project whose path, not title, holds "orb".
        let mut list = PickerList::new(vec![project(1, "web", "/src/orb")]);

        // When filtering with "orb".
        list.refilter("orb");

        // Then the offsets point into the path and none into the name.
        let matches: Vec<&Matches> = list.shown().map(|(_, matches)| matches).collect();
        assert_eq!(
            matches,
            [&Matches {
                name: Vec::new(),
                path: vec![5, 6, 7],
            }],
            "a path match should be offset into the path"
        );
    }

    #[rstest::rstest]
    fn half_page_down_moves_half_the_page() {
        // Given twenty items, the first selected.
        let names: Vec<String> = (0..20).map(|n| format!("d{n}")).collect();
        let mut list = PickerList::new(
            names
                .into_iter()
                .map(|name| PickerItem::Directory { name })
                .collect(),
        );

        // When moving half of a ten-item page down.
        list.half_page_down(10);

        // Then the sixth item is selected.
        assert_eq!(list.selection(), 5, "half a page of 10 is 5");
    }

    #[rstest::rstest]
    fn next_on_the_last_item_wraps_to_the_first() {
        // Given two items with the last selected.
        let mut list = PickerList::new(directories(&["a", "b"]));
        list.next();

        // When selecting the next item.
        list.next();

        // Then the first item is selected.
        assert_eq!(list.selection(), 0, "next should wrap to the first item");
    }

    #[rstest::rstest]
    fn prev_on_the_first_item_wraps_to_the_last() {
        // Given three items with the first selected.
        let mut list = PickerList::new(directories(&["a", "b", "c"]));

        // When selecting the previous item.
        list.prev();

        // Then the last item is selected.
        assert_eq!(list.selection(), 2, "prev should wrap to the last item");
    }

    #[rstest::rstest]
    fn next_skips_a_disabled_row() {
        // Given main selected, above a disabled row and then dev.
        let mut list = PickerList::new(branches(&["main", "!taken", "dev"]));

        // When selecting the next item.
        list.next();

        // Then dev is selected.
        assert_eq!(
            selected_name(&list).as_deref(),
            Some("dev"),
            "next should skip the disabled row"
        );
    }

    #[rstest::rstest]
    fn prev_skips_a_disabled_row() {
        // Given dev selected, below a disabled row and then main.
        let mut list = PickerList::new(branches(&["main", "!taken", "dev"]));
        list.next();

        // When selecting the previous item.
        list.prev();

        // Then main is selected.
        assert_eq!(
            selected_name(&list).as_deref(),
            Some("main"),
            "prev should skip the disabled row"
        );
    }

    #[rstest::rstest]
    fn next_wraps_past_a_disabled_first_row() {
        // Given a disabled first row, then main, then dev selected.
        let mut list = PickerList::new(branches(&["!taken", "main", "dev"]));
        list.next();

        // When selecting the next item.
        list.next();

        // Then main is selected.
        assert_eq!(
            selected_name(&list).as_deref(),
            Some("main"),
            "next should wrap past the disabled first row"
        );
    }

    #[rstest::rstest]
    fn prev_wraps_past_a_disabled_last_row() {
        // Given main selected, then dev, then a disabled last row.
        let mut list = PickerList::new(branches(&["main", "dev", "!taken"]));

        // When selecting the previous item.
        list.prev();

        // Then dev is selected.
        assert_eq!(
            selected_name(&list).as_deref(),
            Some("dev"),
            "prev should wrap past the disabled last row"
        );
    }

    #[rstest::rstest]
    fn next_with_one_item_stays() {
        // Given a single item.
        let mut list = PickerList::new(directories(&["a"]));

        // When selecting the next item.
        list.next();

        // Then it stays selected.
        assert_eq!(list.selection(), 0, "next with one item should stay");
    }

    #[rstest::rstest]
    fn half_page_down_stops_at_the_last_item() {
        // Given three items with the last selected.
        let mut list = PickerList::new(directories(&["a", "b", "c"]));
        list.prev();

        // When moving half of a ten-item page down.
        list.half_page_down(10);

        // Then the last item stays selected.
        assert_eq!(
            list.selection(),
            2,
            "half a page should stop at the last item"
        );
    }

    #[rstest::rstest]
    fn filter_matching_only_disabled_rows_selects_nothing() {
        // Given one enabled and one disabled branch.
        let mut list = PickerList::new(branches(&["main", "!taken"]));

        // When filtering down to the disabled one.
        list.refilter("taken");

        // Then nothing is selected.
        assert_eq!(selected_name(&list), None, "a disabled row can't be picked");
    }

    #[rstest::rstest]
    fn heading_is_hidden_while_filtering() {
        // Given a heading between two settings.
        let mut list = PickerList::new(vec![
            PickerItem::Setting(Some("claude-sonnet-5")),
            PickerItem::Heading("Legacy models"),
            PickerItem::Setting(Some("claude-fable-5")),
        ]);

        // When typing text the heading matches.
        list.refilter("models");

        // Then the heading isn't shown.
        assert!(
            !shown_names(&list).contains(&"Legacy models".to_owned()),
            "shown were {:?}",
            shown_names(&list)
        );
    }

    #[rstest::rstest]
    #[case(None, "Default")]
    #[case(Some("claude-opus-5-5"), "Claude Opus 5.5")]
    #[case(Some("claude-haiku-4-5"), "Claude Haiku 4.5")]
    #[case(Some("opus"), "Claude Opus 5")]
    #[case(Some("sonnet"), "Claude Sonnet 5")]
    #[case(Some("haiku"), "Claude Haiku 4.5")]
    #[case(Some("fable"), "Claude Fable 5.1")]
    #[case(Some("claude-haiku-4-5-20251001"), "Claude Haiku 4.5")]
    #[case(Some("opus[1m]"), "opus[1m]")]
    #[case(Some("plan"), "plan")]
    fn setting_label_names_known_models_and_keeps_other_values(
        #[case] value: Option<&str>,
        #[case] label: &str,
    ) {
        // Given / When / Then a model, by ID or alias, shows its name, and
        // anything else shows as is.
        assert_eq!(setting_label(value), label, "the setting's label");
    }
}
