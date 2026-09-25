//! The preview: the selected thread's transcript as blocks under a one-row
//! header, read from disk without starting Claude.
//!
//! Prompts and replies carry a "you" / "claude" label beside their text; tool
//! calls, thinking, and compaction show as a header row. Tool output and
//! thinking stay folded until opened. Built blocks are cached, only the ones
//! in view are drawn, and the frame's block heights go back to the preview
//! state so navigation scrolls by them.

use std::collections::HashMap;

use orb_domain::feat::preview::block::{
    Block, BlockId, BlockKind, SystemLevel, ToolCall, ToolStatus,
};
use orb_domain::feat::preview::state::{Preview, PreviewLayout};
use orb_domain::feat::sessions::state::{Thread, ThreadId};
use orb_domain::{AppState, Focus};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Widget, Wrap};

/// Where labels and header rows start, from the viewport's left edge; the
/// columns before it are the cursor's gutter.
const LABEL_X: u16 = 2;
/// Where bodies start.
const CONTENT_X: u16 = 10;

/// Blocks already built for the preview's thread, so a frame builds only new
/// or changed ones.
#[derive(Debug, Default)]
pub(crate) struct PreviewCache {
    thread: Option<ThreadId>,
    blocks: HashMap<BlockId, Drawn>,
}

/// A block built for one content width and fold state.
#[derive(Debug)]
struct Drawn {
    width: u16,
    open: bool,
    /// The block as it was built; once it changes it's built again.
    block: Block,
    /// The tool, thinking, or compaction row, at the label column.
    header: Option<Line<'static>>,
    /// "you" or "claude", beside the body's first row.
    label: Option<&'static str>,
    /// Wrapped at the content column.
    body: Option<Text<'static>>,
    rows: usize,
}

impl Drawn {
    /// Builds `block` to wrap at `width` columns, with its body shown when it
    /// doesn't fold or is `open`.
    fn new(block: &Block, width: u16, open: bool) -> Self {
        let dim = Style::new().dim();
        let (header, label, body) = match &block.kind {
            BlockKind::You(text) => (None, Some("you"), Some(Text::raw(text.clone()))),
            BlockKind::Claude(text) => (None, Some("claude"), Some(markdown(text))),
            BlockKind::Thinking(text) => (
                Some(Line::styled(
                    if open { "▾ Thinking" } else { "▸ Thinking" },
                    dim,
                )),
                None,
                open.then(|| Text::styled(text.clone(), dim.italic())),
            ),
            BlockKind::Tool(call) => (
                Some(Line::raw(tool_header(call, block.foldable(), open))),
                None,
                call.output
                    .clone()
                    .filter(|_| open)
                    .map(|output| Text::styled(output, dim)),
            ),
            BlockKind::System {
                level: SystemLevel::Divider,
                text,
            } => (Some(Line::styled(format!("── {text} ──"), dim)), None, None),
            BlockKind::System {
                level: SystemLevel::Info,
                text,
            } => (None, None, Some(Text::styled(text.clone(), dim))),
            BlockKind::System {
                level: SystemLevel::Error,
                text,
            } => (
                None,
                None,
                Some(Text::styled(text.clone(), Style::new().red())),
            ),
        };
        let body_rows = body.as_ref().map_or(0, |body| {
            Paragraph::new(body.clone())
                .wrap(Wrap { trim: false })
                .line_count(width)
        });
        Self {
            width,
            open,
            block: block.clone(),
            rows: (usize::from(header.is_some()) + body_rows).max(1),
            header,
            label,
            body,
        }
    }
}

impl PreviewCache {
    /// `block` built for a content `width` and fold state, reusing the last
    /// build while none of them changed.
    fn get(&mut self, block: &Block, width: u16, open: bool) -> &Drawn {
        let drawn = self
            .blocks
            .entry(block.id)
            .or_insert_with(|| Drawn::new(block, width, open));
        if drawn.width != width || drawn.open != open || drawn.block != *block {
            *drawn = Drawn::new(block, width, open);
        }
        drawn
    }
}

/// Draws `thread`'s preview into `area`: the header, `pane_error` if
/// `claude attach` couldn't start, then the blocks. Returns the viewport's
/// rows and every block's height.
pub(crate) fn render(
    state: &AppState,
    thread: &Thread,
    pane_error: Option<&str>,
    area: Rect,
    buf: &mut Buffer,
    cache: &mut PreviewCache,
) -> PreviewLayout {
    let preview = &state.preview;
    let [header_area, error_area, viewport] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(u16::from(pane_error.is_some())),
        Constraint::Fill(1),
    ])
    .areas(area);
    Line::raw(header(preview, thread)).render(header_area, buf);
    if let Some(pane_error) = pane_error {
        Line::raw(pane_error).render(error_area, buf);
    }
    let rows = usize::from(viewport.height);
    // Still loading: the blocks belong to another thread.
    if preview.thread != Some(thread.id) {
        return PreviewLayout {
            rows,
            heights: vec![],
        };
    }
    if preview.blocks.is_empty() {
        Line::raw("No messages yet · ⏎ attach to send the first prompt").render(viewport, buf);
        return PreviewLayout {
            rows,
            heights: vec![],
        };
    }
    if cache.thread != preview.thread {
        cache.thread = preview.thread;
        cache.blocks.clear();
    }
    let width = viewport.width.saturating_sub(CONTENT_X);
    let heights: Vec<usize> = preview
        .blocks
        .iter()
        .map(|block| {
            cache
                .get(block, width, preview.expanded.contains(&block.id))
                .rows
        })
        .collect();
    // Following keeps the last row on the viewport's bottom row.
    let max_offset = heights.iter().sum::<usize>().saturating_sub(rows);
    let offset = match preview.cursor {
        None => max_offset,
        Some(_) => preview.offset.min(max_offset),
    };
    let cursor = preview
        .cursor_block()
        .map(|block| block.id)
        .filter(|_| state.focus == Focus::Preview);
    let mut start = 0;
    for (block, height) in preview.blocks.iter().zip(&heights) {
        if start >= offset + rows {
            break;
        }
        if start + height > offset
            && let Some(drawn) = cache.blocks.get(&block.id)
        {
            let y = viewport.y + start.saturating_sub(offset) as u16;
            let skip = offset.saturating_sub(start);
            draw_block(drawn, skip, y, cursor == Some(block.id), viewport, buf);
        }
        start += height;
    }
    PreviewLayout { rows, heights }
}

/// `title · branch · model`, and how many transcript lines couldn't be read.
/// Only the title shows until the thread's transcript is loaded.
fn header(preview: &Preview, thread: &Thread) -> String {
    let mut parts = vec![
        thread
            .title
            .clone()
            .unwrap_or_else(|| "New thread".to_owned()),
    ];
    if preview.thread == Some(thread.id) {
        parts.extend(preview.branch.clone());
        parts.extend(preview.model.clone());
        if preview.skipped > 0 {
            parts.push(format!("⚠ {} unreadable lines", preview.skipped));
        }
    }
    parts.join(" · ")
}

/// Draws the rows of `drawn` that are in `viewport`: `skip` rows of it are
/// scrolled above the viewport, and its first visible row is `y`. `cursor`
/// marks its rows in the gutter.
fn draw_block(drawn: &Drawn, skip: usize, y: u16, cursor: bool, viewport: Rect, buf: &mut Buffer) {
    let end = y.saturating_add(
        u16::try_from(drawn.rows - skip)
            .unwrap_or(u16::MAX)
            .min(viewport.bottom() - y),
    );
    if cursor {
        for row in y..end {
            if let Some(cell) = buf.cell_mut((viewport.x, row)) {
                cell.set_symbol("┃");
            }
        }
    }
    let row = |x: u16, y: u16, width: u16, height: u16| {
        Rect::new(viewport.x.saturating_add(x), y, width, height).intersection(viewport)
    };
    let header_rows = usize::from(drawn.header.is_some());
    if let Some(header) = &drawn.header
        && skip == 0
    {
        header.render(row(LABEL_X, y, viewport.width, 1), buf);
    }
    let Some(body) = &drawn.body else {
        return;
    };
    let body_skip = skip.saturating_sub(header_rows);
    let body_y = y + u16::from(skip < header_rows);
    if body_y >= end {
        return;
    }
    if let Some(label) = drawn.label
        && body_skip == 0
    {
        Line::from(label.bold()).render(row(LABEL_X, body_y, CONTENT_X - LABEL_X, 1), buf);
    }
    Paragraph::new(body.clone())
        .wrap(Wrap { trim: false })
        .scroll((u16::try_from(body_skip).unwrap_or(u16::MAX), 0))
        .render(row(CONTENT_X, body_y, drawn.width, end - body_y), buf);
}

/// `▸ $ cargo test ✓`: folded or open (`·` when there's nothing to open), the
/// summary, and whether it succeeded.
fn tool_header(call: &ToolCall, foldable: bool, open: bool) -> String {
    let fold = match (foldable, open) {
        (false, _) => "·",
        (true, false) => "▸",
        (true, true) => "▾",
    };
    let status = match call.status {
        ToolStatus::Pending => "…",
        ToolStatus::Ok => "✓",
        ToolStatus::Failed => "✗",
    };
    format!("{fold} {} {status}", call.summary)
}

/// Claude's markdown as styled text that owns its strings, so it can be
/// cached.
fn markdown(markdown: &str) -> Text<'static> {
    let Text {
        alignment,
        style,
        lines,
    } = tui_markdown::from_str(markdown);
    Text {
        alignment,
        style,
        lines: lines
            .into_iter()
            .map(|line| Line {
                style: line.style,
                alignment: line.alignment,
                spans: line
                    .spans
                    .into_iter()
                    .map(|span| Span::styled(span.content.into_owned(), span.style))
                    .collect(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use orb_domain::feat::preview::block::{Block, BlockId, BlockKind, ToolCall, ToolStatus};
    use orb_domain::feat::preview::state::{Preview, PreviewLayout};
    use orb_domain::feat::sessions::state::{Thread, ThreadId, ThreadStatus};
    use orb_domain::{AppState, Focus};
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;

    use super::{PreviewCache, render};

    fn thread() -> Thread {
        Thread {
            id: ThreadId(1),
            title: Some("Fix the bug".to_owned()),
            cwd: "/work/demo".into(),
            transcript: None,
            status: ThreadStatus::Idle,
            turn_started_at: None,
            attach_argv: vec![],
            branch: None,
            pinned_at: None,
            settled_at: None,
            active_since: SystemTime::UNIX_EPOCH,
            last_activity_at: SystemTime::UNIX_EPOCH,
            unseen: false,
        }
    }

    /// Thread 1's preview showing `kinds` as blocks 0, 1, …, following.
    fn preview(kinds: Vec<BlockKind>) -> Preview {
        Preview {
            thread: Some(ThreadId(1)),
            blocks: kinds
                .into_iter()
                .zip(0..)
                .map(|(kind, id)| Block {
                    id: BlockId(id),
                    parts: 1,
                    kind,
                })
                .collect(),
            ..Preview::default()
        }
    }

    fn state(preview: Preview) -> AppState {
        AppState {
            preview,
            ..AppState::default()
        }
    }

    fn you(text: &str) -> BlockKind {
        BlockKind::You(text.to_owned())
    }

    fn cargo_test() -> BlockKind {
        BlockKind::Tool(ToolCall {
            summary: "$ cargo test".to_owned(),
            status: ToolStatus::Ok,
            output: Some("line one\nline two".to_owned()),
        })
    }

    /// Draws thread 1's preview on a `width`x`height` screen.
    fn draw(
        state: &AppState,
        width: u16,
        height: u16,
        cache: &mut PreviewCache,
    ) -> (Buffer, PreviewLayout) {
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        let layout = render(state, &thread(), None, area, &mut buffer, cache);
        (buffer, layout)
    }

    /// Row `y` from column `x` on, without trailing blanks.
    fn row(buffer: &Buffer, x: u16, y: u16) -> String {
        (x..buffer.area.width)
            .filter_map(|x| buffer.cell((x, y)).map(Cell::symbol))
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    /// The gutter's symbol on each row of the viewport.
    fn gutter(buffer: &Buffer) -> Vec<String> {
        (1..buffer.area.height)
            .filter_map(|y| buffer.cell((0, y)).map(|cell| cell.symbol().to_owned()))
            .collect()
    }

    #[rstest::rstest]
    fn following_puts_the_last_row_on_the_bottom_row() {
        // Given two blocks taller than a 5-row viewport, following.
        let state = state(preview(vec![
            you("a\nb\nc\nd\ne\nf"),
            you("1\n2\n3\n4\n5\n6\n7\nlast"),
        ]));

        // When drawing on a 6-row screen.
        let (buffer, _) = draw(&state, 40, 6, &mut PreviewCache::default());

        // Then the last block's last row is the bottom row.
        let bottom = row(&buffer, 10, 5);
        assert_eq!(bottom, "last", "the bottom row was '{bottom}'");
    }

    #[rstest::rstest]
    fn focused_preview_marks_the_cursor_blocks_rows() {
        // Given a one-row block and a two-row block, following, with the
        // preview focused.
        let state = AppState {
            focus: Focus::Preview,
            ..state(preview(vec![you("one"), you("two\nthree")]))
        };

        // When drawing.
        let (buffer, _) = draw(&state, 40, 5, &mut PreviewCache::default());

        // Then only the last block's rows carry the bar.
        assert_eq!(
            gutter(&buffer),
            [" ", "┃", "┃", " "],
            "the gutter should mark the cursor block"
        );
    }

    #[rstest::rstest]
    fn unfocused_preview_has_no_cursor_bar() {
        // Given blocks with the sidebar focused.
        let state = state(preview(vec![you("one"), you("two\nthree")]));

        // When drawing.
        let (buffer, _) = draw(&state, 40, 5, &mut PreviewCache::default());

        // Then the gutter is empty.
        assert_eq!(
            gutter(&buffer),
            [" "; 4],
            "the gutter should be empty without focus"
        );
    }

    #[rstest::rstest]
    fn folded_tool_block_shows_its_summary_row() {
        // Given a finished tool call with output, folded.
        let state = state(preview(vec![cargo_test()]));

        // When drawing.
        let (buffer, _) = draw(&state, 40, 5, &mut PreviewCache::default());

        // Then its row reads fold, summary, status.
        let tool = row(&buffer, 2, 1);
        assert_eq!(tool, "▸ $ cargo test ✓", "the tool row was '{tool}'");
    }

    #[rstest::rstest]
    fn expanded_tool_block_shows_its_output() {
        // Given a finished tool call the user opened.
        let state = state(Preview {
            expanded: [BlockId(0)].into(),
            ..preview(vec![cargo_test()])
        });

        // When drawing.
        let (buffer, _) = draw(&state, 40, 5, &mut PreviewCache::default());

        // Then its output is under the summary at the content column.
        let output = [row(&buffer, 10, 2), row(&buffer, 10, 3)];
        assert_eq!(
            output,
            ["line one", "line two"],
            "the output should follow the summary"
        );
    }

    #[rstest::rstest]
    fn claude_markdown_bold_is_bold() {
        // Given Claude's reply with a bold word.
        let state = state(preview(vec![BlockKind::Claude(
            "some **bold** text".to_owned(),
        )]));

        // When drawing.
        let (buffer, _) = draw(&state, 40, 5, &mut PreviewCache::default());

        // Then the word is drawn bold.
        let bold = buffer
            .cell((15, 1))
            .map(|cell| (cell.symbol(), cell.modifier));
        assert!(
            bold.is_some_and(
                |(symbol, modifier)| symbol == "b" && modifier.contains(Modifier::BOLD)
            ),
            "row 1 was '{}' with {bold:?} at the bold word",
            row(&buffer, 0, 1)
        );
    }

    #[rstest::rstest]
    fn header_shows_title_branch_and_model() {
        // Given the transcript's branch and model.
        let state = state(Preview {
            branch: Some("main".to_owned()),
            model: Some("opus-5-5".to_owned()),
            ..preview(vec![you("hi")])
        });

        // When drawing.
        let (buffer, _) = draw(&state, 60, 5, &mut PreviewCache::default());

        // Then the header joins them.
        let header = row(&buffer, 0, 0);
        assert_eq!(
            header, "Fix the bug · main · opus-5-5",
            "the header was '{header}'"
        );
    }

    #[rstest::rstest]
    fn header_counts_unreadable_lines() {
        // Given two transcript lines that couldn't be read.
        let state = state(Preview {
            skipped: 2,
            ..preview(vec![you("hi")])
        });

        // When drawing.
        let (buffer, _) = draw(&state, 60, 5, &mut PreviewCache::default());

        // Then the header warns about them.
        let header = row(&buffer, 0, 0);
        assert!(
            header.ends_with("⚠ 2 unreadable lines"),
            "the header was '{header}'"
        );
    }

    #[rstest::rstest]
    fn loading_thread_shows_only_the_header() {
        // Given the preview still showing another thread's blocks.
        let state = state(Preview {
            thread: Some(ThreadId(2)),
            ..preview(vec![you("someone else's prompt")])
        });

        // When drawing thread 1.
        let (buffer, _) = draw(&state, 40, 5, &mut PreviewCache::default());

        // Then everything under the header is blank.
        let body: Vec<String> = (1..5).map(|y| row(&buffer, 0, y)).collect();
        assert_eq!(body, [""; 4], "the viewport should be blank while loading");
    }

    #[rstest::rstest]
    fn grown_block_is_drawn_again() {
        // Given a Claude block already drawn once.
        let mut cache = PreviewCache::default();
        draw(
            &state(preview(vec![BlockKind::Claude("first".to_owned())])),
            40,
            5,
            &mut cache,
        );
        let grown = state(Preview {
            blocks: [Block {
                id: BlockId(0),
                parts: 2,
                kind: BlockKind::Claude("first\n\nsecond".to_owned()),
            }]
            .into(),
            ..preview(vec![])
        });

        // When drawing it after a second part arrived, at the same width.
        let (buffer, _) = draw(&grown, 40, 5, &mut cache);

        // Then the new text shows.
        let text: Vec<String> = (1..5).map(|y| row(&buffer, 10, y)).collect();
        assert!(
            text.iter().any(|line| line == "second"),
            "the block showed {text:?}"
        );
    }

    #[rstest::rstest]
    fn restarted_transcript_block_is_drawn_again() {
        // Given a prompt already drawn as block 0.
        let mut cache = PreviewCache::default();
        draw(&state(preview(vec![you("old prompt")])), 40, 5, &mut cache);

        // When a restarted transcript (after `/clear`) reuses block 0 with the
        // same part count.
        let (buffer, _) = draw(&state(preview(vec![you("new prompt")])), 40, 5, &mut cache);

        // Then the new prompt shows.
        let prompt = row(&buffer, 10, 1);
        assert_eq!(prompt, "new prompt", "block 0 showed '{prompt}'");
    }

    #[rstest::rstest]
    fn narrower_width_makes_wrapping_blocks_taller() {
        // Given a prompt drawn on a 40-column screen, where it fits one row.
        let mut cache = PreviewCache::default();
        let state = state(preview(vec![you("aaaa bbbb cccc dddd")]));
        draw(&state, 40, 5, &mut cache);

        // When drawing it 20 columns wide.
        let (_, layout) = draw(&state, 20, 5, &mut cache);

        // Then it wraps onto two rows.
        assert_eq!(layout.heights, [2], "the prompt should wrap");
    }

    #[rstest::rstest]
    fn render_returns_the_viewport_rows_and_block_heights() {
        // Given a one-row and a two-row block.
        let state = state(preview(vec![you("a"), you("b\nc")]));

        // When drawing on a 10-row screen.
        let (_, layout) = draw(&state, 40, 10, &mut PreviewCache::default());

        // Then the layout has the 9 rows under the header and both heights.
        assert_eq!(
            layout,
            PreviewLayout {
                rows: 9,
                heights: vec![1, 2],
            },
            "the layout should describe the frame"
        );
    }
}
