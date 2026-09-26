//! An open picker: what it picks, its list, and where the keys go back to.
//!
//! The directory picker reads its input as a path. The part through the last
//! `/` names the directory being browsed, and the rest filters its
//! subdirectories. Editing the directory part asks the frontend to list the
//! new directory.

use std::path::{Path, PathBuf};

use crate::Focus;
use crate::feat::picker::list::{Matches, PickerItem, PickerList};

/// What an open picker picks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerKind {
    /// `␣n`: pick a project to start a session in.
    Projects,
    /// `␣p`: pick a directory to add as a project. `listed` is the directory
    /// text (e.g. `~/dev/`) the items were read from; `None` while the input
    /// isn't a path.
    Directories { listed: Option<String> },
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
            Some(PickerItem::Project { .. }) => None,
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

    /// Moves the cursor one grapheme left.
    pub fn cursor_left(&mut self) {
        self.list.cursor_left();
    }

    /// Moves the cursor one grapheme right.
    pub fn cursor_right(&mut self) {
        self.list.cursor_right();
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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{PickerState, expand, split_path};
    use crate::Focus;
    use crate::feat::picker::list::PickerItem;

    const HOME: &str = "/home/u";

    fn names(text: &[&str]) -> Vec<String> {
        text.iter().map(|name| (*name).to_owned()).collect()
    }

    fn shown_names(picker: &PickerState) -> Vec<String> {
        picker
            .shown()
            .filter_map(|(item, _)| match item {
                PickerItem::Directory { name } => Some(name.clone()),
                PickerItem::Project { .. } => None,
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
}
