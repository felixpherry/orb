//! The preview's view: which thread's blocks it shows, where the cursor is,
//! how far it's scrolled, and which blocks are unfolded.
//!
//! Until the user moves, the preview follows the tail: the cursor sits on the
//! last block and the view stays scrolled to the bottom as blocks arrive.
//! Moves scroll just enough to keep the cursor's block in view, and landing
//! back on the last block at the bottom resumes following.

use std::collections::HashSet;
use std::mem;
use std::sync::Arc;

use crate::feat::preview::block::{Block, BlockId};
use crate::feat::sessions::state::ThreadId;

/// The selected thread's transcript as blocks, and where the user is in it.
///
/// The preview actor writes `thread`, `blocks`, `branch`, `model`, `skipped`,
/// and resets the view when it switches thread. The intent handler writes
/// `cursor`, `offset`, `expanded`. The frontend loop writes `layout` after
/// each draw.
#[derive(Debug, Clone, Default)]
pub struct Preview {
    /// Whose transcript `blocks` shows.
    pub thread: Option<ThreadId>,
    pub blocks: Arc<[Block]>,
    /// The git branch the session last ran on.
    pub branch: Option<String>,
    /// The model that last replied, without its `claude-` prefix.
    pub model: Option<String>,
    /// Transcript lines that couldn't be read.
    pub skipped: usize,
    /// The block under the cursor; `None` follows the tail (the last block,
    /// scrolled to the bottom).
    pub cursor: Option<BlockId>,
    /// Rows scrolled past the top, while not following.
    pub offset: usize,
    /// Foldable blocks the user opened.
    pub expanded: HashSet<BlockId>,
    /// The last frame's viewport rows and block heights.
    pub layout: PreviewLayout,
}

/// How the last frame laid the preview out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreviewLayout {
    /// Rows the blocks are drawn in.
    pub rows: usize,
    /// One per block, in order.
    pub heights: Vec<usize>,
}

impl Preview {
    /// The block under the cursor: the last one while following.
    pub fn cursor_block(&self) -> Option<&Block> {
        self.blocks.get(self.index())
    }

    /// Move the cursor to the next block, scrolling it into view.
    pub fn next_block(&mut self) {
        let next = self.index() + 1;
        if next < self.blocks.len() {
            self.move_to(next);
        }
    }

    /// Move the cursor to the previous block, scrolling it into view.
    pub fn prev_block(&mut self) {
        if let Some(prev) = self.index().checked_sub(1) {
            self.move_to(prev);
        }
    }

    /// Jump to the first block.
    pub fn top(&mut self) {
        if let Some(first) = self.blocks.first() {
            self.cursor = Some(first.id);
            self.offset = 0;
            self.settle();
        }
    }

    /// Jump to the bottom and follow the tail.
    pub fn bottom(&mut self) {
        self.cursor = None;
    }

    /// Scroll half a viewport down, following the tail once it reaches the
    /// bottom.
    pub fn half_page_down(&mut self) {
        let offset = self.scroll_offset() + self.half_page();
        if offset < self.max_offset() {
            self.offset = offset;
            self.cursor = self.anchor(offset);
        } else {
            self.cursor = None;
        }
    }

    /// Scroll half a viewport up.
    pub fn half_page_up(&mut self) {
        let offset = self.scroll_offset().saturating_sub(self.half_page());
        self.offset = offset;
        self.cursor = self.anchor(offset);
        self.settle();
    }

    /// Open the cursor's block if it's folded, or fold it if it's open.
    pub fn toggle_fold(&mut self) {
        let Some(id) = self.cursor_block().map(|block| block.id) else {
            return;
        };
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
    }

    /// Show `thread`'s latest content. Switching from another thread starts
    /// its view over: following, scrolled to the top, and every block folded.
    pub fn show(
        &mut self,
        thread: ThreadId,
        blocks: Arc<[Block]>,
        branch: Option<String>,
        model: Option<String>,
        skipped: usize,
        switched: bool,
    ) {
        self.thread = Some(thread);
        self.blocks = blocks;
        self.branch = branch;
        self.model = model;
        self.skipped = skipped;
        if switched {
            self.cursor = None;
            self.offset = 0;
            self.expanded.clear();
        }
    }

    /// Show nothing, keeping the last frame's layout.
    pub fn clear(&mut self) {
        *self = Self {
            layout: mem::take(&mut self.layout),
            ..Self::default()
        };
    }

    /// Put the cursor on block `i`, scrolling as little as possible to show
    /// it whole, or its top when it's taller than the viewport.
    fn move_to(&mut self, i: usize) {
        let rows = self.layout.rows;
        let offset = self.scroll_offset();
        let start = self.start(i);
        let height = self.height(i);
        self.offset = if start < offset {
            start
        } else if start + height <= offset + rows {
            offset
        } else if height > rows {
            start
        } else {
            start + height - rows
        };
        self.cursor = self.blocks.get(i).map(|block| block.id);
        self.settle();
    }

    /// Resume following once the cursor is on the last block at the bottom.
    fn settle(&mut self) {
        let max = self.max_offset();
        if self.index() + 1 == self.blocks.len() && self.offset.min(max) == max {
            self.cursor = None;
        }
    }

    /// The first block that starts in a view scrolled to `offset`, else the
    /// block spanning its top row.
    fn anchor(&self, offset: usize) -> Option<BlockId> {
        let view = offset..offset + self.layout.rows;
        let mut start = 0;
        let mut spanning = self.blocks.last();
        for (i, block) in self.blocks.iter().enumerate() {
            if view.contains(&start) {
                return Some(block.id);
            }
            let end = start + self.height(i);
            if (start..end).contains(&offset) {
                spanning = Some(block);
            }
            start = end;
        }
        spanning.map(|block| block.id)
    }

    /// The offset the view is actually drawn at.
    fn scroll_offset(&self) -> usize {
        match self.cursor {
            None => self.max_offset(),
            Some(_) => self.offset.min(self.max_offset()),
        }
    }

    fn max_offset(&self) -> usize {
        self.start(self.blocks.len())
            .saturating_sub(self.layout.rows)
    }

    /// The cursor's position in `blocks`; the last block while following.
    fn index(&self) -> usize {
        self.cursor
            .and_then(|id| self.blocks.iter().position(|block| block.id == id))
            .unwrap_or_else(|| self.blocks.len().saturating_sub(1))
    }

    /// The row block `i` starts at.
    fn start(&self, i: usize) -> usize {
        (0..i).map(|j| self.height(j)).sum()
    }

    /// Block `i`'s rows in the last frame; 1 until it's been drawn.
    fn height(&self, i: usize) -> usize {
        self.layout.heights.get(i).copied().unwrap_or(1)
    }

    fn half_page(&self) -> usize {
        (self.layout.rows / 2).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::{Preview, PreviewLayout};
    use crate::feat::preview::block::{Block, BlockId, BlockKind};

    /// Five blocks of 4 rows: 20 rows, so a 10-row viewport scrolls up to 10.
    const FIVE: [usize; 5] = [4; 5];

    /// Blocks of `heights` in a 10-row viewport, with the cursor on block
    /// `cursor` (`None` follows) and the view scrolled to `offset`.
    fn preview(heights: &[usize], cursor: Option<u32>, offset: usize) -> Preview {
        Preview {
            blocks: (0..heights.len() as u32)
                .map(|i| Block {
                    id: BlockId(i),
                    parts: 1,
                    kind: BlockKind::You(format!("prompt {i}")),
                })
                .collect(),
            cursor: cursor.map(BlockId),
            offset,
            layout: PreviewLayout {
                rows: 10,
                heights: heights.to_vec(),
            },
            ..Preview::default()
        }
    }

    #[rstest::rstest]
    fn next_block_on_last_block_changes_nothing() {
        // Given a preview following the tail.
        let mut preview = preview(&FIVE, None, 0);

        // When moving to the next block.
        preview.next_block();

        // Then it still follows from the same offset.
        assert_eq!(
            (preview.cursor, preview.offset),
            (None, 0),
            "there is no block after the last"
        );
    }

    #[rstest::rstest]
    fn prev_block_on_first_block_changes_nothing() {
        // Given the cursor on the first block at the top.
        let mut preview = preview(&FIVE, Some(0), 0);

        // When moving to the previous block.
        preview.prev_block();

        // Then the cursor and view stay put.
        assert_eq!(
            (preview.cursor, preview.offset),
            (Some(BlockId(0)), 0),
            "there is no block before the first"
        );
    }

    #[rstest::rstest]
    fn prev_block_while_following_moves_to_second_to_last() {
        // Given a preview following the tail.
        let mut preview = preview(&FIVE, None, 0);

        // When moving to the previous block.
        preview.prev_block();

        // Then the cursor is on the block before the last.
        assert_eq!(
            preview.cursor,
            Some(BlockId(3)),
            "the previous block of the tail is the second-to-last"
        );
    }

    #[rstest::rstest]
    fn next_block_below_viewport_scrolls_it_fully_into_view() {
        // Given the cursor on block 2 with rows 2..12 in view.
        let mut preview = preview(&FIVE, Some(2), 2);

        // When moving to block 3 (rows 12..16).
        preview.next_block();

        // Then the view scrolls until block 3 ends on the bottom row.
        assert_eq!(preview.offset, 6, "rows 6..16 show block 3 whole");
    }

    #[rstest::rstest]
    fn next_block_onto_last_block_at_bottom_follows() {
        // Given the cursor on block 3 with rows 6..16 in view.
        let mut preview = preview(&FIVE, Some(3), 6);

        // When moving onto the last block, which scrolls to the bottom.
        preview.next_block();

        // Then the preview follows the tail again.
        assert_eq!(
            preview.cursor, None,
            "the last block at the bottom is the tail"
        );
    }

    #[rstest::rstest]
    fn top_moves_cursor_to_first_block() {
        // Given a preview following the tail.
        let mut preview = preview(&FIVE, None, 0);

        // When jumping to the top.
        preview.top();

        // Then the cursor is on the first block, scrolled to the top.
        assert_eq!(
            (preview.cursor, preview.offset),
            (Some(BlockId(0)), 0),
            "top should show the first block from row 0"
        );
    }

    #[rstest::rstest]
    fn bottom_follows_the_tail() {
        // Given the cursor on block 1.
        let mut preview = preview(&FIVE, Some(1), 2);

        // When jumping to the bottom.
        preview.bottom();

        // Then the preview follows the tail.
        assert_eq!(preview.cursor, None, "bottom should follow the tail");
    }

    #[rstest::rstest]
    fn half_page_down_mid_transcript_scrolls_half_a_viewport() {
        // Given the cursor on the first block at the top.
        let mut preview = preview(&FIVE, Some(0), 0);

        // When scrolling half a page down.
        preview.half_page_down();

        // Then the view moved 5 rows and the cursor is on block 2, the first
        // to start in rows 5..15.
        assert_eq!(
            (preview.cursor, preview.offset),
            (Some(BlockId(2)), 5),
            "half a page is 5 rows; block 2 starts at row 8"
        );
    }

    #[rstest::rstest]
    fn half_page_down_reaching_bottom_follows() {
        // Given the cursor on block 2 with rows 6..16 in view.
        let mut preview = preview(&FIVE, Some(2), 6);

        // When scrolling half a page down past the bottom (offset 10).
        preview.half_page_down();

        // Then the preview follows the tail.
        assert_eq!(
            preview.cursor, None,
            "reaching the bottom should follow the tail"
        );
    }

    #[rstest::rstest]
    fn half_page_up_while_following_scrolls_up_from_the_bottom() {
        // Given a preview following the tail (drawn at offset 10).
        let mut preview = preview(&FIVE, None, 0);

        // When scrolling half a page up.
        preview.half_page_up();

        // Then the view is half a page above the bottom.
        assert_eq!(preview.offset, 5, "10 − 5 rows");
    }

    #[rstest::rstest]
    fn half_page_up_inside_tall_last_block_keeps_cursor_on_it() {
        // Given a 30-row last block after a 4-row one, followed (drawn at
        // offset 24).
        let mut preview = preview(&[4, 30], None, 0);

        // When scrolling half a page up, still inside the last block.
        preview.half_page_up();

        // Then the view moved up 5 rows and the cursor stays on that block.
        assert_eq!(
            (preview.cursor, preview.offset),
            (Some(BlockId(1)), 19),
            "the tall block should stay under the cursor"
        );
    }
}
