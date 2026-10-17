//! How the user is looking at the plate.

use super::board::RowKey;

/// How the user is looking at the plate. Written by the intent handler.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlateView {
    /// The selected row.
    pub cursor: PlateCursor,
    /// Done today shows its rows; folded at start.
    pub done_open: bool,
}

/// The plate's selection, kept by row key so it stays on its row as rows
/// come, go and change group.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlateCursor {
    /// The chosen row; `None` until one is chosen, which selects the first row.
    key: Option<RowKey>,
    /// The rows in order when `key` was chosen, to find its neighbour once it leaves.
    order: Vec<RowKey>,
}

impl PlateCursor {
    /// Chooses `key` among `order`, the rows shown now.
    pub fn select(&mut self, key: RowKey, order: Vec<RowKey>) {
        self.key = Some(key);
        self.order = order;
    }

    /// The selected row among `keys`, the rows shown now: the chosen one
    /// while it's there; once it's gone, the first row after it in the
    /// remembered order that's still there, else the nearest before it;
    /// the first row when nothing was chosen or none of those remain;
    /// `None` when `keys` is empty.
    #[must_use]
    pub fn on(&self, keys: &[RowKey]) -> Option<RowKey> {
        let first = keys.first().copied();
        let Some(chosen) = self.key else {
            return first;
        };
        if keys.contains(&chosen) {
            return Some(chosen);
        }
        let Some(at) = self.order.iter().position(|&key| key == chosen) else {
            return first;
        };
        let (before, after) = self.order.split_at(at);
        after
            .iter()
            .skip(1)
            .chain(before.iter().rev())
            .find(|key| keys.contains(key))
            .copied()
            .or(first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use crate::feat::plate::board::Board;
    use crate::feat::plate::state::{ItemState, Plate, test_item};
    use crate::feat::sessions::state::AgentStatus;
    use crate::ids::PlateItemId;
    use crate::intent_handler::test_support::{agent, at, state_with};

    fn item_key(id: i64) -> RowKey {
        RowKey::Item(PlateItemId(id))
    }

    /// `keys` of items with these ids, in order.
    fn items(ids: &[i64]) -> Vec<RowKey> {
        ids.iter().copied().map(item_key).collect()
    }

    /// A cursor on item `id`, chosen among the items `order`.
    fn on_item(id: i64, order: &[i64]) -> PlateCursor {
        let mut cursor = PlateCursor::default();
        cursor.select(item_key(id), items(order));
        cursor
    }

    /// Session 1 with an idle agent, holding items 1 to 3 in Your move.
    fn three_items() -> AppState {
        let mut state = state_with(vec![agent(1, AgentStatus::Idle)], 1);
        state.plate = Plate::from_saved(
            (1..=3)
                .map(|id| test_item(id, ItemState::YourMove))
                .collect(),
        );
        state
    }

    #[rstest::rstest]
    fn cursor_starts_on_the_first_row() {
        // Given a plate nobody has moved the cursor on.
        let state = three_items();

        // When laying out the board.
        let board = Board::new(&state, at(0));

        // Then its first row is selected.
        assert_eq!(
            board.selected,
            board.keys().first().copied(),
            "an untouched cursor selects the first row"
        );
    }

    #[rstest::rstest]
    fn cursor_stays_on_its_row_when_rows_are_added_before_it() {
        // Given the cursor on item 2 of items 1 and 2.
        let cursor = on_item(2, &[1, 2]);

        // When items 3 and 4 arrive ahead of it.
        let selected = cursor.on(&items(&[3, 4, 1, 2]));

        // Then item 2 is still selected.
        assert_eq!(
            selected,
            Some(item_key(2)),
            "new rows above don't move the selection off its row"
        );
    }

    #[rstest::rstest]
    fn cursor_stays_on_its_item_when_it_changes_group() {
        // Given the cursor on item 2, in Your move with items 1 and 3.
        let mut state = three_items();
        let keys = Board::new(&state, at(0)).keys();
        state.plate.view_mut().cursor.select(item_key(2), keys);

        // When item 2 moves to Waiting and the board is laid out again.
        let view = state.plate.view().clone();
        state.plate = Plate::from_saved(vec![
            test_item(1, ItemState::YourMove),
            test_item(2, ItemState::Waiting),
            test_item(3, ItemState::YourMove),
        ]);
        *state.plate.view_mut() = view;
        let selected = Board::new(&state, at(0)).selected;

        // Then item 2 is still selected, in its new group.
        assert_eq!(
            selected,
            Some(item_key(2)),
            "the selection follows its item into another group"
        );
    }

    #[rstest::rstest]
    fn cursor_moves_to_the_next_row_when_its_row_leaves() {
        // Given the cursor on item 2 of items 1 to 3.
        let cursor = on_item(2, &[1, 2, 3]);

        // When item 2 leaves.
        let selected = cursor.on(&items(&[1, 3]));

        // Then the row that came after it is selected.
        assert_eq!(
            selected,
            Some(item_key(3)),
            "the selection moves on to the next row"
        );
    }

    #[rstest::rstest]
    fn cursor_moves_to_the_previous_row_when_the_last_row_leaves() {
        // Given the cursor on item 3, the last of items 1 to 3.
        let cursor = on_item(3, &[1, 2, 3]);

        // When item 3 leaves.
        let selected = cursor.on(&items(&[1, 2]));

        // Then the row before it is selected.
        assert_eq!(
            selected,
            Some(item_key(2)),
            "with nothing after it, the selection moves back one row"
        );
    }

    #[rstest::rstest]
    fn cursor_goes_to_the_first_row_when_every_remembered_row_left() {
        // Given the cursor on item 2 of items 1 and 2.
        let cursor = on_item(2, &[1, 2]);

        // When both leave and items 5 and 6 arrive.
        let selected = cursor.on(&items(&[5, 6]));

        // Then the first row is selected.
        assert_eq!(
            selected,
            Some(item_key(5)),
            "with no remembered row left, the selection goes to the top"
        );
    }

    #[rstest::rstest]
    fn cursor_on_an_empty_plate_selects_nothing() {
        // Given the cursor on item 1.
        let cursor = on_item(1, &[1]);

        // When the plate empties.
        let selected = cursor.on(&[]);

        // Then nothing is selected.
        assert_eq!(selected, None, "an empty plate has no selected row");
    }
}
