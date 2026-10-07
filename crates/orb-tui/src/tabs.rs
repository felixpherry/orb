//! The right-hand area while a session is shown: a one-row tab bar, then the
//! shown tab's panes laid out by its tree, each in a rounded frame titled
//! with its name. The frame shows where the focus is: blue while the pane
//! has the keys, grey while it has the focus and the keys are elsewhere,
//! dim otherwise. A zoomed tab shows only its focused pane. A stack is a
//! list naming its panes, one plain row each, above the shown one.

use std::collections::HashMap;
use std::ops::Range;

use orb_domain::AppState;
use orb_domain::feat::layout::state::{SessionLayout, StackList};
use orb_domain::feat::sessions::state::PaneId;
use orb_term::Pane;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Widget};

use crate::mouse::HitMap;
use crate::session_picker::cut_right;
use crate::sidebar::{BLACK, BLUE, COMMENT, DARK3, FG, FG_DARK, GUTTER};

/// The trailing chevron's text.
const PLUS: &str = " + ";
/// The powerline arrow each chevron starts and ends with.
const ARROW: &str = "\u{e0b0}";

/// `[tab bar, tab body]` of the right-hand area.
pub(crate) fn areas(right: Rect) -> [Rect; 2] {
    Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(right)
}

/// Draws `layout` into `right`: the tab bar, then each placed pane that has
/// a client in `panes` inside its frame, and the stack's list. Records each
/// pane's screen, and whether its program reads the mouse, in `hits`.
/// Returns the focused pane's cursor while `keys_in_pane`.
pub(crate) fn render(
    state: &AppState,
    layout: &SessionLayout,
    panes: &HashMap<PaneId, Pane>,
    keys_in_pane: bool,
    right: Rect,
    buf: &mut Buffer,
    hits: &mut HitMap,
) -> Option<Position> {
    let [bar, body] = areas(right);
    tab_bar(state, layout, bar, buf);
    let placement = layout.placed(body);
    let mut cursor = None;
    for place in &placement.panes {
        let lit = place.focused && keys_in_pane;
        let content = place.content();
        let reads_mouse = panes.get(&place.pane).is_some_and(Pane::reads_mouse);
        hits.record_pane(content, place.pane, reads_mouse);
        let drawn = panes
            .get(&place.pane)
            .filter(|_| !content.is_empty())
            .and_then(|pane| pane.render(content, buf));
        if lit {
            cursor = drawn;
        }
        let (border, title) = match (lit, place.focused) {
            (true, _) => (BLUE, Style::new().fg(BLUE).add_modifier(Modifier::BOLD)),
            (false, true) => (COMMENT, Style::new().fg(FG_DARK)),
            (false, false) => (GUTTER, Style::new().fg(DARK3)),
        };
        Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(border))
            .title(Line::styled(
                format!(" {} ", pane_title(state, place.pane)),
                title,
            ))
            .render(place.area, buf);
    }
    if let Some(stack) = &placement.stack {
        let lit = keys_in_pane
            && placement
                .panes
                .iter()
                .any(|place| place.pane == stack.shown && place.focused);
        stack_list(state, stack, lit, buf, hits);
    }
    cursor
}

/// Draws `stack`'s list: a plain row per pane that fits, the shown pane's
/// as `> name`, bold blue while `lit`. Names too long for the
/// row end in `…`. Records every other row in `hits`, reading the mouse so
/// a click focuses its pane without starting a selection.
fn stack_list(state: &AppState, stack: &StackList, lit: bool, buf: &mut Buffer, hits: &mut HitMap) {
    for (pane, row) in &stack.rows {
        let (mark, style) = match (*pane == stack.shown, lit) {
            (true, true) => ("> ", Style::new().fg(BLUE).add_modifier(Modifier::BOLD)),
            (true, false) => ("> ", Style::new().fg(FG_DARK)),
            (false, _) => ("  ", Style::new().fg(DARK3)),
        };
        let text = cut_right(
            &format!("{mark}{}", pane_title(state, *pane)),
            usize::from(row.width),
        );
        buf.set_stringn(row.x, row.y, text, usize::from(row.width), style);
        if *pane != stack.shown {
            hits.record_pane(*row, *pane, true);
        }
    }
}

/// What orb calls pane `pane` in its frame and the stack list: its name,
/// else the title of the agent running in it, else `shell`.
pub(crate) fn pane_title(state: &AppState, pane: PaneId) -> String {
    state
        .layouts
        .entry(pane)
        .and_then(|entry| entry.name.clone())
        .or_else(|| {
            state
                .sessions
                .threads()
                .find(|thread| {
                    thread
                        .pane
                        .as_ref()
                        .is_some_and(|launch| launch.pane == pane)
                })
                .and_then(|thread| thread.title.clone())
        })
        .unwrap_or_else(|| "shell".to_owned())
}

/// The tab bar, like zellij's: the shown session's name, then a chevron per
/// tab with the shown one bold on blue, then a ` + ` chevron. Tabs that
/// don't fit are counted in `← +N` / `+N →` chips, keeping the shown tab.
fn tab_bar(state: &AppState, layout: &SessionLayout, bar: Rect, buf: &mut Buffer) {
    buf.set_style(bar, Style::new().bg(BLACK));
    let session = Span::styled(
        format!(" {} ", session_label(state)),
        Style::new().fg(FG).bg(BLACK).add_modifier(Modifier::BOLD),
    );
    let shown = Style::new().fg(BLACK).bg(BLUE).add_modifier(Modifier::BOLD);
    let rest = Style::new().fg(COMMENT).bg(GUTTER);
    let labels: Vec<String> = (0..layout.tabs().len())
        .map(|index| format!(" {} ", layout.tab_label(index)))
        .collect();
    let widths: Vec<u16> = labels.iter().map(|label| chevron_width(label)).collect();
    let room = bar
        .width
        .saturating_sub(span_width(&session))
        .saturating_sub(chevron_width(PLUS));
    let range = visible_tabs(&widths, layout.active(), room);
    let mut spans = vec![session];
    if range.start > 0 {
        chevron(&mut spans, left_chip(range.start), rest);
    }
    for (index, label) in labels.iter().enumerate().take(range.end).skip(range.start) {
        let style = if index == layout.active() {
            shown
        } else {
            rest
        };
        chevron(&mut spans, label.clone(), style);
    }
    if range.end < labels.len() {
        chevron(&mut spans, right_chip(labels.len() - range.end), rest);
    }
    chevron(&mut spans, PLUS.to_owned(), rest);
    Line::from(spans).render(bar, buf);
}

/// What the tab bar calls the shown session: its name, else its branch,
/// else `session`.
fn session_label(state: &AppState) -> String {
    state
        .shown_session()
        .and_then(|id| state.sessions.session(id))
        .and_then(|session| session.name.clone().or_else(|| session.branch.clone()))
        .unwrap_or_else(|| "session".to_owned())
}

/// Pushes `text` on `style`'s background between two arrows onto `spans`.
fn chevron(spans: &mut Vec<Span<'static>>, text: String, style: Style) {
    let bg = style.bg.unwrap_or(BLACK);
    spans.push(Span::styled(ARROW, Style::new().fg(BLACK).bg(bg)));
    spans.push(Span::styled(text, style));
    spans.push(Span::styled(ARROW, Style::new().fg(bg).bg(BLACK)));
}

fn chevron_width(text: &str) -> u16 {
    span_width(&Span::raw(text)).saturating_add(2)
}

fn span_width(span: &Span) -> u16 {
    u16::try_from(span.width()).unwrap_or(u16::MAX)
}

/// Counts the `n` tabs hidden left of the bar.
fn left_chip(n: usize) -> String {
    format!(" ← +{n} ")
}

/// Counts the `n` tabs hidden right of the bar.
fn right_chip(n: usize) -> String {
    format!(" +{n} → ")
}

/// Which tabs fit in `room` columns, given each tab chevron's `widths`,
/// following zellij: start from the shown tab `active` and add neighbours,
/// alternating sides from the left, while they and the chips counting the
/// rest fit. The shown tab is always in, even when it alone overflows.
fn visible_tabs(widths: &[u16], active: usize, room: u16) -> Range<usize> {
    let chip = |hidden: usize, text: fn(usize) -> String| match hidden {
        0 => 0,
        n => u32::from(chevron_width(&text(n))),
    };
    let fits = |range: &Range<usize>| {
        let used: u32 = widths
            .get(range.clone())
            .map_or(0, |widths| widths.iter().copied().map(u32::from).sum());
        used + chip(range.start, left_chip) + chip(widths.len() - range.end, right_chip)
            <= u32::from(room)
    };
    let grow = |range: &Range<usize>, left: bool| {
        if left {
            range.start.checked_sub(1).map(|start| start..range.end)
        } else {
            (range.end < widths.len()).then(|| range.start..range.end + 1)
        }
    };
    let mut range = active.min(widths.len())..(active + 1).min(widths.len());
    let mut left = true;
    while let Some((grown, side)) = [left, !left]
        .into_iter()
        .find_map(|side| grow(&range, side).filter(fits).map(|grown| (grown, side)))
    {
        range = grown;
        left = !side;
    }
    range
}

#[cfg(test)]
mod tests {
    use super::visible_tabs;

    #[rstest::rstest]
    fn all_tabs_that_fit_are_visible() {
        // Given three 10-column tabs and 30 columns of room.
        // When choosing the visible tabs with the middle one shown.
        let range = visible_tabs(&[10; 3], 1, 30);

        // Then every tab is visible.
        assert_eq!(range, 0..3, "three tabs fit in 30 columns");
    }

    #[rstest::rstest]
    fn shown_last_tab_keeps_its_left_neighbours() {
        // Given six 10-column tabs and 40 columns of room.
        // When choosing the visible tabs with the last one shown.
        let range = visible_tabs(&[10; 6], 5, 40);

        // Then the range runs to the last tab and hides some on the left.
        assert!(
            range.end == 6 && range.start > 0,
            "the last tab and its left neighbours, got {range:?}"
        );
    }

    #[rstest::rstest]
    fn shown_first_tab_keeps_its_right_neighbours() {
        // Given six 10-column tabs and 40 columns of room.
        // When choosing the visible tabs with the first one shown.
        let range = visible_tabs(&[10; 6], 0, 40);

        // Then the range starts at the first tab and hides some on the right.
        assert!(
            range.start == 0 && range.end < 6,
            "the first tab and its right neighbours, got {range:?}"
        );
    }

    #[rstest::rstest]
    fn shown_middle_tab_hides_tabs_on_both_sides() {
        // Given eight 10-column tabs and 40 columns of room.
        // When choosing the visible tabs with tab 3 shown.
        let range = visible_tabs(&[10; 8], 3, 40);

        // Then tab 3 is visible and tabs are hidden on both sides.
        assert!(
            range.contains(&3) && range.start > 0 && range.end < 8,
            "tab 3 with both ends hidden, got {range:?}"
        );
    }

    #[rstest::rstest]
    fn shown_tab_wider_than_the_room_is_still_visible() {
        // Given a 50-column first tab and 30 columns of room.
        // When choosing the visible tabs with it shown.
        let range = visible_tabs(&[50, 10, 10], 0, 30);

        // Then only the shown tab is visible.
        assert_eq!(range, 0..1, "the shown tab alone");
    }
}
