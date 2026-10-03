//! The search picker, drawn in the session picker's snacks frame: a float
//! over the sidebar and the right side holding a list box and a preview box,
//! side by side from 120 columns and stacked below that.
//!
//! The list box is titled `Search`, with `indexing n/total` beside it while
//! orb is still indexing transcripts at startup. Its input row holds the
//! typed text and how many messages are listed, or `200+` when more matched,
//! over an orange rule. Each row is one matching message: its thread's dim
//! `<project|group>/` and bright title, then a dim one-line snippet of the
//! message with the matches lit. When the search index can't be opened, the
//! list says why instead. There are no key hints.

use orb_domain::AppState;
use orb_domain::feat::picker::list::PickerItem;
use orb_domain::feat::picker::state::{PickerKind, PickerState};
use orb_domain::feat::search::state::SearchProgress;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Widget};

use crate::mouse::HitMap;
use crate::picker::{PickerScroll, highlight, span};
use crate::session_picker::{big, boxed, boxes, render_input};
use crate::sidebar::{BG_DARK, BLUE, COMMENT, DARK3, DARK5, FG, ORANGE, RED, VISUAL};

/// Draws the search picker over `area`: `picker`'s hit rows, with the
/// indexing progress and any index error from `state`, beside an empty
/// preview box. Returns how many rows the list fits and where the terminal
/// cursor goes in the input. Records the popup, the list box as the wheel's
/// area and the list's rows in `hits`.
pub(crate) fn render(
    picker: &PickerState,
    state: &AppState,
    area: Rect,
    buf: &mut Buffer,
    scroll: &mut PickerScroll,
    hits: &mut HitMap,
) -> (usize, Position) {
    let popup = big(area);
    Clear.render(popup, buf);
    buf.set_style(popup, Style::new().bg(BG_DARK));
    hits.record_overlay(popup);
    let [list_box, preview_box] = boxes(popup, area.width >= 120);
    hits.record_selector(list_box);
    let drawn = render_list(picker, &state.search, list_box, buf, scroll, hits);
    boxed(None).render(preview_box, buf);
    drawn
}

/// ` Search `, then a dim `indexing n/total ` while indexing runs.
fn title(search: &SearchProgress) -> Line<'static> {
    let mut spans = vec![span(" Search ", BLUE)];
    if search.indexed < search.total {
        spans.push(span(
            format!("indexing {}/{} ", search.indexed, search.total),
            DARK3,
        ));
    }
    Line::from(spans)
}

/// The list box: title, input row with the count, orange rule, then the
/// rows, or `search unavailable: <error>` in red when the index can't be
/// opened. Records each row in `hits`. Returns the rows' height and the
/// cursor.
fn render_list(
    picker: &PickerState,
    search: &SearchProgress,
    area: Rect,
    buf: &mut Buffer,
    scroll: &mut PickerScroll,
    hits: &mut HitMap,
) -> (usize, Position) {
    let block = boxed(Some(title(search)));
    let inner = block.inner(area);
    block.render(area, buf);
    let [input, rule, rows] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);
    let shown: Vec<&PickerItem> = picker.shown().map(|(item, _)| item).collect();
    let count = match picker.kind() {
        PickerKind::Search { overflow: true } => format!("{}+", picker.total()),
        _ => format!("{}/{}", shown.len(), picker.total()),
    };
    let cursor = render_input(picker, &count, input, buf, hits);
    Line::from(span("─".repeat(usize::from(rule.width)), ORANGE)).render(rule, buf);
    let page = usize::from(rows.height).max(1);
    if let Some(error) = &search.error {
        Line::from(span(format!("  search unavailable: {error}"), RED)).render(rows, buf);
        return (page, cursor);
    }
    let offset = scroll.follow(picker.selection(), shown.len(), page);
    for ((index, item), y) in shown
        .into_iter()
        .enumerate()
        .skip(offset)
        .zip(rows.top()..rows.bottom())
    {
        let row = Rect::new(rows.x, y, rows.width, 1);
        hits.record_picker_row(row, index);
        if index == picker.selection() {
            buf.set_style(row, Style::new().bg(VISUAL));
        }
        render_row(item, row, buf);
    }
    (page, cursor)
}

/// One hit row: a space, its label with the `<project|group>/` prefix dim
/// and the title bright, a space, then the snippet in `COMMENT` with its
/// matches lit. Clipped one column short of the row's right edge.
fn render_row(item: &PickerItem, area: Rect, buf: &mut Buffer) {
    let PickerItem::Hit {
        label,
        split,
        snippet,
        lit,
        ..
    } = item
    else {
        return;
    };
    let line = Line::from_iter(
        std::iter::once(Span::raw(" "))
            .chain(highlight(
                label,
                &[],
                |at| if at < *split { DARK5 } else { FG },
            ))
            .chain(std::iter::once(Span::raw(" ")))
            .chain(highlight(snippet, lit, |_| COMMENT)),
    );
    line.render(
        Rect {
            width: area.width.saturating_sub(1),
            ..area
        },
        buf,
    );
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use orb_domain::feat::picker::list::PickerItem;
    use orb_domain::feat::picker::state::PickerState;
    use orb_domain::feat::search::state::SearchProgress;
    use orb_domain::feat::sessions::state::ThreadId;
    use orb_domain::{AppState, Focus};
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;
    use unicode_segmentation::UnicodeSegmentation;

    use super::render;
    use crate::mouse::HitMap;
    use crate::picker::PickerScroll;
    use crate::sidebar::{BLUE1, COMMENT, DARK5, FG};

    /// A hit in thread 1 labelled `label` (title from `split`) with
    /// `snippet`, `lit` its lit byte offsets.
    fn hit(label: &str, split: usize, snippet: &str, lit: Vec<usize>) -> PickerItem {
        PickerItem::Hit {
            id: 1,
            thread: ThreadId(1),
            label: label.to_owned(),
            split,
            snippet: snippet.to_owned(),
            lit,
            text_lit: vec![],
            path: PathBuf::from("/t/1.jsonl"),
            prompt_offset: 0,
        }
    }

    /// The `orb/fix the bug` hit whose snippet `the parser drops lines`
    /// lights `parser`.
    fn parser_hit() -> PickerItem {
        hit(
            "orb/fix the bug",
            4,
            "the parser drops lines",
            (4..10).collect(),
        )
    }

    /// A search picker listing `items`, as the actor writes them.
    fn searching(items: Vec<PickerItem>, overflow: bool) -> PickerState {
        let mut picker = PickerState::search(Focus::Sidebar);
        picker.show_hits("", items, overflow);
        picker
    }

    /// App state whose search index reports `search`.
    fn app(search: SearchProgress) -> AppState {
        AppState {
            search,
            ..AppState::default()
        }
    }

    /// Draws `picker` over a 100×40 screen, stacked, with `app`'s progress.
    fn draw(picker: &PickerState, app: &AppState) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 40));
        render(
            picker,
            app,
            buf.area,
            &mut buf,
            &mut PickerScroll::default(),
            &mut HitMap::default(),
        );
        buf
    }

    /// The screen's text, one line per row.
    fn screen(buf: &Buffer) -> String {
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .filter_map(|x| buf.cell((x, y)).map(Cell::symbol))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The cell where `text` first starts, row by row.
    fn find<'a>(buf: &'a Buffer, text: &str) -> Option<&'a Cell> {
        let len = text.graphemes(true).count();
        (0..buf.area.height).find_map(|y| {
            let row: Vec<&str> = (0..buf.area.width)
                .filter_map(|x| buf.cell((x, y)).map(Cell::symbol))
                .collect();
            let x = (0..row.len()).find(|&x| {
                row.get(x..x + len)
                    .is_some_and(|cells| cells.concat() == text)
            })?;
            buf.cell((x as u16, y))
        })
    }

    #[rstest::rstest]
    fn hit_row_reads_its_label_then_its_snippet() {
        // Given a search picker over one hit.
        let picker = searching(vec![parser_hit()], false);

        // When drawing it.
        let buf = draw(&picker, &AppState::default());

        // Then the row reads the label, a space, then the snippet.
        let screen = screen(&buf);
        assert!(
            screen.contains("orb/fix the bug the parser drops lines"),
            "screen was\n{screen}"
        );
    }

    #[rstest::rstest]
    fn hit_row_dims_its_label_prefix() {
        // Given a search picker over one hit labelled `orb/fix the bug`.
        let picker = searching(vec![parser_hit()], false);

        // When drawing it.
        let buf = draw(&picker, &AppState::default());

        // Then `orb/` is dim and the title bright.
        let colors = (
            find(&buf, "orb/").map(|cell| cell.fg),
            find(&buf, "fix the bug").map(|cell| cell.fg),
        );
        assert_eq!(colors, (Some(DARK5), Some(FG)), "prefix and title colors");
    }

    #[rstest::rstest]
    fn hit_row_lights_the_snippets_matches() {
        // Given a hit whose snippet lights `parser`.
        let picker = searching(vec![parser_hit()], false);

        // When drawing it.
        let buf = draw(&picker, &AppState::default());

        // Then `parser` is lit blue and bold, and `drops` stays dim.
        let lit =
            find(&buf, "parser").map(|cell| (cell.fg, cell.modifier.contains(Modifier::BOLD)));
        let unlit = find(&buf, "drops").map(|cell| cell.fg);
        assert_eq!(
            (lit, unlit),
            (Some((BLUE1, true)), Some(COMMENT)),
            "lit and unlit snippet graphemes"
        );
    }

    #[rstest::rstest]
    fn list_title_shows_indexing_progress_while_indexing() {
        // Given one of three transcripts indexed.
        let app = app(SearchProgress {
            indexed: 1,
            total: 3,
            error: None,
        });

        // When drawing the search picker.
        let buf = draw(&searching(vec![], false), &app);

        // Then the list title shows the progress.
        let screen = screen(&buf);
        assert!(
            screen.contains(" Search indexing 1/3 "),
            "screen was\n{screen}"
        );
    }

    #[rstest::rstest]
    fn list_title_drops_the_progress_once_indexed() {
        // Given every transcript indexed.
        let app = app(SearchProgress {
            indexed: 3,
            total: 3,
            error: None,
        });

        // When drawing the search picker.
        let buf = draw(&searching(vec![], false), &app);

        // Then no indexing progress shows.
        let screen = screen(&buf);
        assert!(!screen.contains("indexing"), "screen was\n{screen}");
    }

    #[rstest::rstest]
    fn overflowing_results_count_as_200_plus() {
        // Given 200 hits with more matched.
        let hits = std::iter::repeat_with(parser_hit).take(200).collect();
        let picker = searching(hits, true);

        // When drawing it.
        let buf = draw(&picker, &AppState::default());

        // Then the count reads `200+`.
        let screen = screen(&buf);
        assert!(screen.contains("200+"), "screen was\n{screen}");
    }

    #[rstest::rstest]
    fn index_error_shows_search_unavailable_in_the_list() {
        // Given a search index that couldn't be opened.
        let app = app(SearchProgress {
            error: Some("no fts5".to_owned()),
            ..SearchProgress::default()
        });

        // When drawing the search picker.
        let buf = draw(&searching(vec![], false), &app);

        // Then the list says why search is unavailable.
        let screen = screen(&buf);
        assert!(
            screen.contains("search unavailable: no fts5"),
            "screen was\n{screen}"
        );
    }
}
