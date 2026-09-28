//! The jump list: the sidebar rows the user jumped between, and where
//! `<C-o>`/`<C-i>` last landed among them.

use crate::feat::sessions::state::SidebarItem;

/// How many rows the jump list keeps.
pub const JUMP_LIMIT: usize = 20;

/// The rows the user jumped between, oldest first, each at most once, and
/// the one the last `<C-o>`/`<C-i>` landed on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JumpList {
    /// Oldest first; at most [`JUMP_LIMIT`], never the Settled header.
    entries: Vec<SidebarItem>,
    /// The entry the last `<C-o>`/`<C-i>` landed on; `None` while the user
    /// isn't moving through the list.
    at: Option<usize>,
}

impl JumpList {
    /// A list of `items`, oldest first, as saved: the newest
    /// [`JUMP_LIMIT`], each once, without the Settled header.
    #[must_use]
    pub fn from_saved(items: Vec<SidebarItem>) -> Self {
        let mut list = Self::default();
        for item in items {
            list.record(item);
        }
        list
    }

    /// The rows, oldest first.
    #[must_use]
    pub fn entries(&self) -> &[SidebarItem] {
        &self.entries
    }

    /// Adds `item` as the newest row, moving it there if it's already
    /// listed and dropping the oldest past [`JUMP_LIMIT`]. The Settled header
    /// is never added.
    pub fn record(&mut self, item: SidebarItem) {
        if item == SidebarItem::SettledShelf {
            return;
        }
        self.entries.retain(|&entry| entry != item);
        self.entries.push(item);
        let over = self.entries.len().saturating_sub(JUMP_LIMIT);
        self.entries.drain(..over);
        self.at = None;
    }

    /// Records a jump: the row it left, then the row it landed on.
    pub fn jump(&mut self, from: Option<SidebarItem>, to: Option<SidebarItem>) {
        for item in [from, to].into_iter().flatten() {
            self.record(item);
        }
    }

    /// Where [`back`](Self::back) would land from `current`: the nearest
    /// older row that's `reachable` and isn't `current`.
    pub fn peek_back<F>(&self, current: Option<SidebarItem>, reachable: F) -> Option<SidebarItem>
    where
        F: Fn(SidebarItem) -> bool,
    {
        let list = self.seen_from(current);
        let end = list.at.unwrap_or(list.entries.len());
        list.entries
            .get(..end)?
            .iter()
            .rev()
            .copied()
            .find(|&entry| Some(entry) != current && reachable(entry))
    }

    /// Where [`forward`](Self::forward) would land from `current`: the
    /// nearest newer row that's `reachable` and isn't `current`; nothing
    /// unless a `<C-o>` went back first.
    pub fn peek_forward<F>(&self, current: Option<SidebarItem>, reachable: F) -> Option<SidebarItem>
    where
        F: Fn(SidebarItem) -> bool,
    {
        self.entries
            .get(self.at? + 1..)?
            .iter()
            .copied()
            .find(|&entry| Some(entry) != current && reachable(entry))
    }

    /// `<C-o>`: the nearest older `reachable` row other than `current`. The
    /// first one records `current`, so `<C-i>` can come back to it. With no
    /// such row, nothing changes.
    pub fn back<F>(&mut self, current: Option<SidebarItem>, reachable: F) -> Option<SidebarItem>
    where
        F: Fn(SidebarItem) -> bool,
    {
        let target = self.peek_back(current, reachable)?;
        *self = self.seen_from(current);
        self.land(target);
        Some(target)
    }

    /// `<C-i>`: the nearest newer `reachable` row other than `current`,
    /// after a `<C-o>`. With no such row, nothing changes.
    pub fn forward<F>(&mut self, current: Option<SidebarItem>, reachable: F) -> Option<SidebarItem>
    where
        F: Fn(SidebarItem) -> bool,
    {
        let target = self.peek_forward(current, reachable)?;
        self.land(target);
        Some(target)
    }

    /// Drops `item`, a row that no longer exists. Removing the row the last
    /// jump landed on ends the move through the list.
    pub fn remove(&mut self, item: SidebarItem) {
        let Some(gone) = self.entries.iter().position(|&entry| entry == item) else {
            return;
        };
        self.entries.remove(gone);
        self.at = match self.at {
            Some(at) if gone < at => Some(at - 1),
            Some(at) if gone == at => None,
            at => at,
        };
    }

    /// The list as a `<C-o>` from `current` sees it: with `current`
    /// recorded as the newest row unless a `<C-o>` already went back.
    fn seen_from(&self, current: Option<SidebarItem>) -> Self {
        let mut list = self.clone();
        if let (None, Some(current)) = (self.at, current) {
            list.record(current);
        }
        list
    }

    /// Marks `target`, a listed row, as the one the last jump landed on.
    fn land(&mut self, target: SidebarItem) {
        self.at = self.entries.iter().position(|&entry| entry == target);
    }
}

#[cfg(test)]
mod tests {
    use super::{JUMP_LIMIT, JumpList};
    use crate::feat::sessions::state::{SidebarItem, ThreadId};

    fn on(id: i64) -> SidebarItem {
        SidebarItem::Thread(ThreadId(id))
    }

    /// A list holding threads `ids`, oldest first.
    fn list_of(ids: &[i64]) -> JumpList {
        JumpList::from_saved(ids.iter().copied().map(on).collect())
    }

    fn anywhere(_: SidebarItem) -> bool {
        true
    }

    #[rstest::rstest]
    fn recording_past_the_limit_drops_the_oldest() {
        // Given a full list of threads 1 to 20.
        let mut list = list_of(&(1..=20).collect::<Vec<_>>());

        // When recording a 21st row.
        list.record(on(21));

        // Then thread 1 is gone and 20 rows remain.
        assert_eq!(
            (list.entries().len(), list.entries().first().copied()),
            (JUMP_LIMIT, Some(on(2))),
            "the oldest row should drop off"
        );
    }

    #[rstest::rstest]
    fn recording_a_listed_row_moves_it_to_the_newest() {
        // Given threads 1, 2 and 3.
        let mut list = list_of(&[1, 2, 3]);

        // When recording thread 1 again.
        list.record(on(1));

        // Then it's the newest, listed once.
        assert_eq!(
            list.entries(),
            [on(2), on(3), on(1)],
            "a listed row should move to the newest slot"
        );
    }

    #[rstest::rstest]
    fn recording_the_settled_header_changes_nothing() {
        // Given threads 1 and 2.
        let mut list = list_of(&[1, 2]);

        // When recording the Settled header.
        list.record(SidebarItem::SettledShelf);

        // Then the list is unchanged.
        assert_eq!(list, list_of(&[1, 2]), "the header is never a jump");
    }

    #[rstest::rstest]
    fn back_then_forward_returns_to_the_starting_row() {
        // Given threads 1 and 2, with the cursor on thread 3.
        let mut list = list_of(&[1, 2]);
        let back = list.back(Some(on(3)), anywhere);

        // When going forward from where back landed.
        let forward = list.forward(back, anywhere);

        // Then it's back on thread 3.
        assert_eq!(forward, Some(on(3)), "<C-i> should undo <C-o>");
    }

    #[rstest::rstest]
    fn back_skips_an_unreachable_row() {
        // Given threads 1 and 2, with thread 2 unreachable.
        let mut list = list_of(&[1, 2]);

        // When going back from thread 3.
        let target = list.back(Some(on(3)), |item| item != on(2));

        // Then it lands on thread 1.
        assert_eq!(target, Some(on(1)), "unreachable rows should be skipped");
    }

    #[rstest::rstest]
    fn back_skips_the_current_row() {
        // Given threads 1 and 2, with the cursor on thread 2.
        let mut list = list_of(&[1, 2]);

        // When going back.
        let target = list.back(Some(on(2)), anywhere);

        // Then it lands on thread 1.
        assert_eq!(target, Some(on(1)), "<C-o> should never land in place");
    }

    #[rstest::rstest]
    fn forward_while_not_moving_through_the_list_finds_nothing() {
        // Given threads 1 and 2, and no <C-o> yet.
        let mut list = list_of(&[1, 2]);

        // When going forward.
        let target = list.forward(Some(on(2)), anywhere);

        // Then there's nowhere to go.
        assert_eq!(target, None, "<C-i> needs a <C-o> first");
    }

    #[rstest::rstest]
    fn back_without_an_older_row_changes_nothing() {
        // Given only thread 1, with the cursor on it.
        let mut list = list_of(&[1]);

        // When going back.
        list.back(Some(on(1)), anywhere);

        // Then the list is unchanged.
        assert_eq!(list, list_of(&[1]), "a failed <C-o> should change nothing");
    }

    #[rstest::rstest]
    fn back_from_an_unlisted_row_skips_the_row_recording_it_drops() {
        // Given a full list of threads 1 to 20, with only thread 1 reachable.
        let mut list = list_of(&(1..=20).collect::<Vec<_>>());

        // When going back from unlisted thread 21.
        let target = list.back(Some(on(21)), |item| item == on(1));

        // Then there's nowhere to go: recording thread 21 drops thread 1.
        assert_eq!(target, None, "<C-o> must not land on a dropped row");
    }

    #[rstest::rstest]
    fn remove_drops_the_row() {
        // Given threads 1, 2 and 3.
        let mut list = list_of(&[1, 2, 3]);

        // When removing thread 2.
        list.remove(on(2));

        // Then threads 1 and 3 remain.
        assert_eq!(list.entries(), [on(1), on(3)], "the deleted row should go");
    }

    #[rstest::rstest]
    fn removing_an_older_row_keeps_the_place_in_the_list() {
        // Given threads 1, 2 and 3, gone back from thread 4 to thread 3.
        let mut list = list_of(&[1, 2, 3]);
        list.back(Some(on(4)), anywhere);

        // When removing thread 1.
        list.remove(on(1));

        // Then going forward from thread 3 still lands on thread 4.
        assert_eq!(
            list.forward(Some(on(3)), anywhere),
            Some(on(4)),
            "<C-i> should still undo the <C-o>"
        );
    }

    #[rstest::rstest]
    fn from_saved_drops_the_settled_header() {
        // Given a saved list holding the Settled header between two threads.
        let saved = vec![on(1), SidebarItem::SettledShelf, on(2)];

        // When restoring it.
        let list = JumpList::from_saved(saved);

        // Then only the threads are listed.
        assert_eq!(list.entries(), [on(1), on(2)], "the header is never kept");
    }
}
