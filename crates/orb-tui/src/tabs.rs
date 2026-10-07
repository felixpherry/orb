//! The right-hand area while a session is shown: a one-row tab bar, then the
//! shown tab's panes laid out by its tree, each in a rounded frame titled
//! with its name. The frame shows where the focus is: blue while the pane
//! has the keys, grey while it has the focus and the keys are elsewhere,
//! dim otherwise. A zoomed tab shows only its focused pane. A stack is a
//! list naming its panes beside the shown one.

use std::collections::HashMap;

use orb_domain::AppState;
use orb_domain::feat::layout::state::{Placement, SessionLayout};
use orb_domain::feat::sessions::state::PaneId;
use orb_term::Pane;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Widget};

use crate::mouse::HitMap;
use crate::sidebar::{BG_DARK, BLUE, COMMENT, DARK3, FG_DARK, GUTTER};

/// The widest a stack's list gets, frame included.
const LIST_MAX: u16 = 24;

/// `[tab bar, tab body]` of the right-hand area.
pub(crate) fn areas(right: Rect) -> [Rect; 2] {
    Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(right)
}

/// Where `layout`'s shown tab draws its panes in `body`, its stack's list
/// as wide as the longest pane title needs, up to `LIST_MAX`. Drawing,
/// resizing and spawning all ask here, so they agree on every pane's size.
pub(crate) fn placement(state: &AppState, layout: &SessionLayout, body: Rect) -> Placement {
    let longest = layout
        .active_tab()
        .map(|tab| tab.stacked())
        .unwrap_or_default()
        .into_iter()
        .map(|pane| Span::raw(pane_title(state, pane)).width())
        .max();
    let list_width = longest.map_or(0, |longest| {
        // Frame, `> ` and a space after the name.
        u16::try_from(longest + 5).unwrap_or(LIST_MAX).min(LIST_MAX)
    });
    layout.placed(body, list_width)
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
    tab_bar(layout, bar, buf);
    let placement = placement(state, layout, body);
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
        Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(GUTTER))
            .render(stack.area, buf);
        for (pane, row) in &stack.rows {
            buf.set_stringn(
                row.x,
                row.y,
                pane_title(state, *pane),
                usize::from(row.width),
                Style::new().fg(DARK3),
            );
            if *pane != stack.shown {
                hits.record_pane(*row, *pane, true);
            }
        }
    }
    cursor
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
