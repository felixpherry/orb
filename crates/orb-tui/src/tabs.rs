//! The right-hand area while a session is shown: a one-row tab bar, then the
//! shown tab's panes laid out by its tree with plain one-cell lines between
//! them. A zoomed tab shows only its focused pane. Each collapsed pane of a
//! stack is a one-row title (` ▸ name`) in place of its screen.

use std::collections::HashMap;

use orb_domain::feat::layout::state::SessionLayout;
use orb_domain::feat::sessions::state::PaneId;
use orb_term::Pane;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::mouse::HitMap;
use crate::sidebar::{BG_DARK, BLUE, COMMENT, DARK3};

/// `[tab bar, tab body]` of the right-hand area.
pub(crate) fn areas(right: Rect) -> [Rect; 2] {
    Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(right)
}

/// Draws `layout` into `right`: the tab bar (` 1 name  2  3 …`, the shown
/// tab in reverse blue), border lines, and each placed pane that has a
/// client in `panes`. Records each pane's area, and whether its program
/// reads the mouse, in `hits`. Returns the focused pane's cursor while
/// `keys_in_pane`. A stack's collapsed pane is drawn as ` ▸ ` and its
/// `title`, recorded as reading the mouse so a click on it focuses the pane
/// without starting a selection.
pub(crate) fn render(
    layout: &SessionLayout,
    panes: &HashMap<PaneId, Pane>,
    title: &dyn Fn(PaneId) -> String,
    keys_in_pane: bool,
    right: Rect,
    buf: &mut Buffer,
    hits: &mut HitMap,
) -> Option<Position> {
    let [bar, body] = areas(right);
    tab_bar(layout, bar, buf);
    let mut cursor = None;
    for place in layout.placed(body) {
        let area = place.area;
        let line = Style::new().fg(DARK3);
        if area.x > body.x {
            for y in area.top()..area.bottom() {
                buf.set_string(area.x - 1, y, "│", line);
            }
        }
        if place.collapsed {
            buf.set_stringn(
                area.x,
                area.y,
                format!(" ▸ {}", title(place.pane)),
                usize::from(area.width),
                Style::new().fg(COMMENT),
            );
            hits.record_pane(area, place.pane, true);
            continue;
        }
        if area.y > body.y {
            let from = area.x.saturating_sub(u16::from(area.x > body.x));
            for x in from..area.right() {
                buf.set_string(x, area.y - 1, "─", line);
            }
        }
        let reads_mouse = panes.get(&place.pane).is_some_and(Pane::reads_mouse);
        hits.record_pane(area, place.pane, reads_mouse);
        let drawn = panes
            .get(&place.pane)
            .and_then(|pane| pane.render(area, buf));
        if place.focused && keys_in_pane {
            cursor = drawn;
        }
    }
    cursor
}

/// Each tab's label (its name, else its focused pane's), padded by a space
/// each side, the shown one in reverse blue.
fn tab_bar(layout: &SessionLayout, bar: Rect, buf: &mut Buffer) {
    let spans: Vec<Span<'static>> = layout
        .tabs()
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let style = if index == layout.active() {
                Style::new().fg(BG_DARK).bg(BLUE)
            } else {
                Style::new().fg(COMMENT)
            };
            Span::styled(format!(" {} ", layout.tab_label(index)), style)
        })
        .collect();
    Line::from(spans).render(bar, buf);
}
