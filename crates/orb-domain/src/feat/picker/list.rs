//! The list inside a picker: the typed filter text, the items it narrows, and
//! the selection among the ones shown.

use std::path::PathBuf;

use fuzzy_matcher::FuzzyMatcher as _;
use fuzzy_matcher::skim::SkimMatcherV2;
use unicode_segmentation::UnicodeSegmentation;

use crate::feat::sessions::state::ProjectId;

/// One row a picker can show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerItem {
    /// A project, matched on its name and path.
    Project {
        id: ProjectId,
        title: String,
        root: PathBuf,
    },
    /// A subdirectory, matched on its name.
    Directory { name: String },
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
    /// The typed filter text.
    input: String,
    /// A grapheme index into `input`.
    cursor: usize,
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
    /// best score first, list order breaking ties, and selects the first.
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
        self.selection = 0;
    }

    /// Replaces the filter text, leaving the cursor at its end.
    pub fn replace_input(&mut self, text: &str) {
        text.clone_into(&mut self.input);
        self.cursor = self.input.graphemes(true).count();
    }

    /// Types `ch` at the cursor. Line breaks are dropped.
    pub fn insert(&mut self, ch: char) {
        if matches!(ch, '\n' | '\r') {
            return;
        }
        let at = self.byte_at(self.cursor);
        self.input.insert(at, ch);
        self.cursor = self
            .input
            .get(..at + ch.len_utf8())
            .map_or(0, |typed| typed.graphemes(true).count());
    }

    /// Deletes the grapheme before the cursor.
    pub fn backspace(&mut self) {
        let Some(before) = self.cursor.checked_sub(1) else {
            return;
        };
        let start = self.byte_at(before);
        self.input
            .replace_range(start..self.byte_at(self.cursor), "");
        self.cursor = before;
    }

    /// Deletes back from the cursor to the previous `/` or space. A `/` or
    /// space right before the cursor goes with it: `~/dev/` becomes `~/`.
    pub fn delete_word(&mut self) {
        let before: Vec<&str> = self.input.graphemes(true).take(self.cursor).collect();
        let separator = |g: &&str| *g == "/" || g.chars().all(char::is_whitespace);
        let mut from = before.len();
        if before.last().is_some_and(separator) {
            from -= 1;
        }
        from -= before.get(..from).map_or(0, |word| {
            word.iter().rev().take_while(|&g| !separator(g)).count()
        });
        let (start, end) = (self.byte_at(from), self.byte_at(self.cursor));
        self.input.replace_range(start..end, "");
        self.cursor = from;
    }

    /// Moves the cursor one grapheme left.
    pub fn cursor_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Moves the cursor one grapheme right.
    pub fn cursor_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.input.graphemes(true).count());
    }

    /// Selects the next shown item, stopping at the last.
    pub fn next(&mut self) {
        self.move_down(1);
    }

    /// Selects the previous shown item, stopping at the first.
    pub fn prev(&mut self) {
        self.selection = self.selection.saturating_sub(1);
    }

    /// Moves the selection down half of the `page` items that fit on screen.
    pub fn half_page_down(&mut self, page: usize) {
        self.move_down((page / 2).max(1));
    }

    /// Moves the selection up half of the `page` items that fit on screen.
    pub fn half_page_up(&mut self, page: usize) {
        self.selection = self.selection.saturating_sub((page / 2).max(1));
    }

    /// The typed filter text.
    pub fn input(&self) -> &str {
        &self.input
    }

    /// The cursor, as a grapheme index into [`input`](Self::input).
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The selected row, as an index into [`shown`](Self::shown).
    pub fn selection(&self) -> usize {
        self.selection
    }

    /// The selected item; `None` when nothing is shown.
    pub fn selected(&self) -> Option<&PickerItem> {
        let (index, _) = self.shown.get(self.selection)?;
        self.items.get(*index)
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

    /// Whether no item is shown.
    pub fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }

    fn move_down(&mut self, step: usize) {
        self.selection = (self.selection + step).min(self.shown.len().saturating_sub(1));
    }

    /// The byte offset of grapheme `index` in the input, or its end.
    fn byte_at(&self, index: usize) -> usize {
        self.input
            .grapheme_indices(true)
            .nth(index)
            .map_or(self.input.len(), |(at, _)| at)
    }
}

fn hidden(item: &PickerItem, pattern: &str) -> bool {
    matches!(item, PickerItem::Directory { name } if name.starts_with('.'))
        && !pattern.starts_with('.')
}

/// The summed score and match offsets of `item` when every term matches.
fn score(matcher: &SkimMatcherV2, item: &PickerItem, terms: &[&str]) -> Option<(i64, Matches)> {
    // A project is matched on "title\nroot". Typed text never holds a line
    // break, so every offset falls on one side of it.
    let (label, title_len) = match item {
        PickerItem::Project { title, root, .. } => {
            (format!("{title}\n{}", root.display()), Some(title.len()))
        }
        PickerItem::Directory { name } => (name.clone(), None),
    };
    let bytes: Vec<usize> = label.char_indices().map(|(at, _)| at).collect();
    let mut total = 0;
    let mut offsets = Vec::new();
    for term in terms {
        let (score, chars) = matcher.fuzzy_indices(&label, term)?;
        total += score;
        offsets.extend(chars.iter().filter_map(|&index| bytes.get(index).copied()));
    }
    offsets.sort_unstable();
    offsets.dedup();
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

    use super::{Matches, PickerItem, PickerList};
    use crate::feat::sessions::state::ProjectId;

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
        }
    }

    fn shown_names(list: &PickerList) -> Vec<String> {
        list.shown()
            .map(|(item, _)| match item {
                PickerItem::Directory { name } => name.clone(),
                PickerItem::Project { title, .. } => title.clone(),
            })
            .collect()
    }

    fn typed(text: &str) -> PickerList {
        let mut list = PickerList::default();
        for ch in text.chars() {
            list.insert(ch);
        }
        list
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
                PickerItem::Directory { .. } => None,
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
    fn insert_types_at_the_cursor() {
        // Given "ab" typed, with the cursor between the two.
        let mut list = typed("ab");
        list.cursor_left();

        // When typing "x".
        list.insert('x');

        // Then it lands at the cursor.
        assert_eq!(list.input(), "axb", "insert should type at the cursor");
    }

    #[rstest::rstest]
    fn backspace_deletes_the_grapheme_before_the_cursor() {
        // Given "abc" typed, with the cursor before "c".
        let mut list = typed("abc");
        list.cursor_left();

        // When deleting.
        list.backspace();

        // Then "b" is gone.
        assert_eq!(
            list.input(),
            "ac",
            "backspace should delete before the cursor"
        );
    }

    #[rstest::rstest]
    #[case("~/dev/orb", "~/dev/")]
    #[case("~/dev/", "~/")]
    #[case("~/", "")]
    #[case("foo bar", "foo ")]
    #[case("foo ", "")]
    fn delete_word_deletes_back_to_a_separator(#[case] text: &str, #[case] expected: &str) {
        // Given text typed with the cursor at its end.
        let mut list = typed(text);

        // When deleting a word.
        list.delete_word();

        // Then the text up to the previous separator remains.
        assert_eq!(list.input(), expected, "delete_word on {text:?}");
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
    fn next_stops_at_the_last_item() {
        // Given two items with the last selected.
        let mut list = PickerList::new(directories(&["a", "b"]));
        list.next();

        // When selecting the next item.
        list.next();

        // Then the last item stays selected.
        assert_eq!(list.selection(), 1, "next should stop at the last item");
    }
}
