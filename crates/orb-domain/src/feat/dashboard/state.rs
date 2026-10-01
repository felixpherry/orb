//! The dashboard's menu cursor.

use super::items;
use crate::feat::sessions::state::{Sessions, SidebarItem};

/// Which dashboard item is highlighted, and for which selection. It reads as
/// the first item once the selection isn't the one it last moved on, so any
/// selection change resets it, whoever makes it. Written only by the intent
/// handler.
#[derive(Debug, Default)]
pub struct DashboardCursor {
    /// The selection the cursor last moved on.
    on: Option<SidebarItem>,
    index: usize,
}

impl DashboardCursor {
    /// The highlighted index among the selection's `len` items: 0 when the
    /// selection changed since the last move.
    pub fn index(&self, sessions: &Sessions, len: usize) -> usize {
        if self.on == sessions.cursor {
            self.index.min(len.saturating_sub(1))
        } else {
            0
        }
    }

    /// Highlight the next item, wrapping from the last to the first.
    pub fn next(&mut self, sessions: &Sessions) {
        self.step(sessions, true);
    }

    /// Highlight the previous item, wrapping from the first to the last.
    pub fn prev(&mut self, sessions: &Sessions) {
        self.step(sessions, false);
    }

    fn step(&mut self, sessions: &Sessions, forward: bool) {
        let len = items(sessions).len().max(1);
        let index = self.index(sessions, len);
        self.index = if forward {
            (index + 1) % len
        } else {
            (index + len - 1) % len
        };
        self.on = sessions.cursor;
    }
}

#[cfg(test)]
mod tests {
    use super::DashboardCursor;
    use crate::feat::dashboard::items;
    use crate::feat::dashboard::tests::sessions;
    use crate::feat::sessions::state::{ProjectId, SidebarItem, ThreadId};

    #[rstest::rstest]
    fn cursor_reads_the_first_item_after_the_selection_changes() {
        // Given the cursor moved down on thread 1.
        let mut sessions = sessions(Some(true), Some(SidebarItem::Thread(ThreadId(1))));
        let mut cursor = DashboardCursor::default();
        cursor.next(&sessions);

        // When the selection moves to the draft without an intent.
        sessions.cursor = Some(SidebarItem::Draft(ProjectId(1)));

        // Then the cursor reads as the first item.
        assert_eq!(
            cursor.index(&sessions, items(&sessions).len()),
            0,
            "a new selection should start on the first item"
        );
    }

    #[rstest::rstest]
    fn cursor_stays_within_a_shrunken_item_list() {
        // Given the cursor on the last of a git draft's thirteen items.
        let mut sessions = sessions(Some(true), Some(SidebarItem::Draft(ProjectId(1))));
        let mut cursor = DashboardCursor::default();
        cursor.prev(&sessions);

        // When the draft turns out not to be in a git repository.
        if let Some(draft) = sessions.draft_mut(ProjectId(1)) {
            draft.repo = false;
        }

        // Then the cursor reads as the new last item.
        let len = items(&sessions).len();
        assert_eq!(
            cursor.index(&sessions, len),
            len - 1,
            "the cursor should clamp to the last item"
        );
    }
}
