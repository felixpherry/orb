//! The mouse in orb's own UI: a click on the sidebar or the right side moves
//! the keys there, a click on a row selects it and a double-click attaches,
//! a click on the sidebar's input box starts a search, and a click on a
//! dashboard item highlights it. The wheel moves the sidebar's selection
//! while it has the keys, and otherwise scrolls its view. While attached, the
//! pane gets its own mouse events, as the attached program expects. In a picker a click
//! selects a row, a double-click picks it and the wheel over its list moves
//! the selection;
//! a click outside a picker or the rename box closes it like `Esc`. A click
//! in the text of the sidebar search, a picker's input or the rename box
//! moves its cursor there.
//!
//! Each frame records where it drew what a click can land on, and a mouse
//! event is mapped back through that record.

use std::time::{Duration, Instant};

use orb_domain::feat::sessions::state::SidebarItem;
use orb_domain::{Focus, Intent};
use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::picker::grapheme_at;

/// Two clicks on the same target closer than this are a double-click
/// (neovim's `mousetime`).
const DOUBLE_CLICK: Duration = Duration::from_millis(500);
/// How many lines a wheel notch scrolls the sidebar's view.
const WHEEL_LINES: i16 = 3;

/// Where the last frame drew what a click can land on. Rebuilt every frame.
#[derive(Debug, Default)]
pub(crate) struct HitMap {
    /// The sidebar; empty while it's hidden.
    sidebar: Rect,
    /// The right side: the attached pane or the dashboard.
    right: Rect,
    /// The sidebar's input box.
    sidebar_input: Option<Rect>,
    /// Each on-screen sidebar row's visible lines.
    sidebar_rows: Vec<(Rect, SidebarItem)>,
    /// Each dashboard menu item's line and index; empty while a pane is shown.
    dashboard_items: Vec<(Rect, usize)>,
    /// The open picker's or rename box's popup.
    overlay: Option<Rect>,
    /// Where the wheel moves the open picker's selection: its popup, or the
    /// session or worktree picker's list box.
    selector: Option<Rect>,
    /// Each selectable picker row's line and its index into the picker's
    /// shown rows.
    picker_rows: Vec<(Rect, usize)>,
    /// The text line of the input that has the keys: the sidebar search, a
    /// picker's input or the rename box.
    text: Option<TextHit>,
}

/// The text line of the input that has the keys, and what `visible` drew
/// its text from.
#[derive(Debug)]
struct TextHit {
    /// The input's line, its prompt included.
    line: Rect,
    /// How many columns the prompt takes before the text.
    prompt: usize,
    /// The typed text.
    text: String,
    /// The cursor, as a grapheme index into `text`.
    cursor: usize,
    /// The columns `visible` was given for the text.
    room: usize,
}

impl HitMap {
    /// A frame's empty hit map, with the sidebar and the right side where
    /// the layout put them.
    pub(crate) fn new(sidebar: Rect, right: Rect) -> Self {
        Self {
            sidebar,
            right,
            ..Self::default()
        }
    }

    /// Records where the sidebar's input box was drawn.
    pub(crate) fn record_sidebar_input(&mut self, area: Rect) {
        self.sidebar_input = Some(area);
    }

    /// Records the visible lines of a sidebar row.
    pub(crate) fn record_row(&mut self, area: Rect, item: SidebarItem) {
        self.sidebar_rows.push((area, item));
    }

    /// Records the line of the dashboard's menu item at `index`.
    pub(crate) fn record_menu_item(&mut self, area: Rect, index: usize) {
        self.dashboard_items.push((area, index));
    }

    /// The sidebar row drawn at `at`, if any.
    pub(crate) fn row_at(&self, at: Position) -> Option<SidebarItem> {
        self.sidebar_rows
            .iter()
            .find(|(area, _)| area.contains(at))
            .map(|&(_, item)| item)
    }

    /// Whether `at` is on the sidebar's input box.
    pub(crate) fn on_sidebar_input(&self, at: Position) -> bool {
        self.sidebar_input.is_some_and(|area| area.contains(at))
    }

    /// The index of the dashboard menu item drawn at `at`, if any.
    pub(crate) fn menu_item_at(&self, at: Position) -> Option<usize> {
        self.dashboard_items
            .iter()
            .find(|(area, _)| area.contains(at))
            .map(|&(_, index)| index)
    }

    /// Records where the open picker or rename box was drawn.
    pub(crate) fn record_overlay(&mut self, area: Rect) {
        self.overlay = Some(area);
    }

    /// Records where the wheel moves the open picker's selection.
    pub(crate) fn record_selector(&mut self, area: Rect) {
        self.selector = Some(area);
    }

    /// Records the line of the picker row shown at `index`.
    pub(crate) fn record_picker_row(&mut self, area: Rect, index: usize) {
        self.picker_rows.push((area, index));
    }

    /// Whether `at` is on the open picker or rename box.
    pub(crate) fn on_overlay(&self, at: Position) -> bool {
        self.overlay.is_some_and(|area| area.contains(at))
    }

    /// Whether the wheel at `at` moves the open picker's selection.
    pub(crate) fn on_selector(&self, at: Position) -> bool {
        self.selector.is_some_and(|area| area.contains(at))
    }

    /// The shown index of the picker row drawn at `at`, if any.
    pub(crate) fn picker_row_at(&self, at: Position) -> Option<usize> {
        self.picker_rows
            .iter()
            .find(|(area, _)| area.contains(at))
            .map(|&(_, index)| index)
    }

    /// Records the input line `line`, whose text follows `prompt` columns
    /// and was drawn by `visible(text, cursor, room)`.
    pub(crate) fn record_text(
        &mut self,
        line: Rect,
        prompt: usize,
        text: &str,
        cursor: usize,
        room: usize,
    ) {
        self.text = Some(TextHit {
            line,
            prompt,
            text: text.to_owned(),
            cursor,
            room,
        });
    }

    /// The grapheme of the input's text drawn at `at`: the first shown one
    /// when `at` is on the prompt, the text's length past its end. `None` off
    /// the input's line.
    pub(crate) fn text_at(&self, at: Position) -> Option<usize> {
        let hit = self.text.as_ref().filter(|hit| hit.line.contains(at))?;
        let column = usize::from(at.x - hit.line.x).saturating_sub(hit.prompt);
        Some(grapheme_at(&hit.text, hit.cursor, hit.room, column))
    }
}

/// What a click can double on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClickTarget {
    /// A sidebar row.
    Row(SidebarItem),
    /// A picker row, by its index into the picker's shown rows.
    PickerRow(usize),
}

/// Whether a click is the first or the second of a double-click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Click {
    Single,
    Double,
}

/// The last click, for telling a double-click.
#[derive(Debug, Default)]
pub(crate) struct Clicks {
    last: Option<(Instant, ClickTarget)>,
}

impl Clicks {
    /// A left click on `target` (`None`: on anything else) at `now`: `Double`
    /// when the last click hit the same target less than 500 ms ago, which
    /// then starts over; else `Single`, remembered when it hit a target.
    pub(crate) fn click(&mut self, target: Option<ClickTarget>, now: Instant) -> Click {
        let double = matches!(
            (self.last, target),
            (Some((then, last)), Some(target))
                if last == target && now.duration_since(then) < DOUBLE_CLICK
        );
        if double {
            self.last = None;
            Click::Double
        } else {
            self.last = target.map(|target| (now, target));
            Click::Single
        }
    }
}

/// What the frontend loop does with a mouse event.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MouseRoute {
    /// Pass it to the attached pane.
    Forward,
    /// Handle these intents in order, as if keys asked for them.
    Intents(Vec<Intent>),
    /// Scroll the sidebar's view this many lines (up when negative) without
    /// moving its selection.
    ScrollSidebar(i16),
    /// Ignore it.
    Nothing,
}

/// The mouse event acts as a left click or a wheel notch (`1` down, `-1` up).
enum Action {
    Click,
    Wheel(i16),
}

/// Maps a mouse event to what the loop does with it, given where the last
/// frame drew things (`hits`), who has the keys, and whether the right side
/// shows a pane. Every event over the pane while attached goes to it; other
/// than that, only left clicks and the vertical wheel act.
pub(crate) fn route(
    event: MouseEvent,
    hits: &HitMap,
    focus: Focus,
    pane_shown: bool,
    clicks: &mut Clicks,
    now: Instant,
) -> MouseRoute {
    let at = Position::new(event.column, event.row);
    if focus == Focus::Attached && hits.right.contains(at) {
        return MouseRoute::Forward;
    }
    let action = match event.kind {
        MouseEventKind::Down(MouseButton::Left) => Action::Click,
        MouseEventKind::ScrollDown => Action::Wheel(1),
        MouseEventKind::ScrollUp => Action::Wheel(-1),
        _ => return MouseRoute::Nothing,
    };
    match (focus, action) {
        (Focus::Picker | Focus::Rename, action) => {
            route_overlay(at, hits, focus, action, clicks, now)
        }
        (_, Action::Wheel(_)) if !hits.sidebar.contains(at) => MouseRoute::Nothing,
        (Focus::Sidebar, Action::Wheel(1)) => MouseRoute::Intents(vec![Intent::SelectWheelNext]),
        (Focus::Sidebar, Action::Wheel(_)) => MouseRoute::Intents(vec![Intent::SelectWheelPrev]),
        (_, Action::Wheel(notch)) => MouseRoute::ScrollSidebar(notch * WHEEL_LINES),
        (_, Action::Click) => {
            let row = hits.row_at(at);
            let click = clicks.click(row.map(ClickTarget::Row), now);
            route_click(at, hits, focus, pane_shown, row, click)
        }
    }
}

/// Where a click or wheel notch at `at` goes while a picker or the rename
/// box has the keys: a click outside the popup cancels it, a click on a
/// picker row selects it and a double-click picks it, a click in the
/// input's text moves its cursor there, and the wheel over a picker's list
/// moves its selection. Before the popup is drawn, nothing.
fn route_overlay(
    at: Position,
    hits: &HitMap,
    focus: Focus,
    action: Action,
    clicks: &mut Clicks,
    now: Instant,
) -> MouseRoute {
    if hits.overlay.is_none() {
        return MouseRoute::Nothing;
    }
    match (action, hits.on_overlay(at), focus) {
        (Action::Wheel(notch), _, Focus::Picker) if hits.on_selector(at) => match notch {
            1 => MouseRoute::Intents(vec![Intent::PickerWheelNext]),
            _ => MouseRoute::Intents(vec![Intent::PickerWheelPrev]),
        },
        (Action::Wheel(_), ..) => MouseRoute::Nothing,
        (Action::Click, false, _) => {
            clicks.click(None, now);
            MouseRoute::Intents(vec![Intent::PickerCancel])
        }
        (Action::Click, true, _) => {
            let row = hits.picker_row_at(at);
            let click = clicks.click(row.map(ClickTarget::PickerRow), now);
            match (row, hits.text_at(at)) {
                (Some(index), _) => intents([
                    Some(Intent::PickerSelectRow(index)),
                    (click == Click::Double).then_some(Intent::PickerConfirm),
                ]),
                (None, Some(grapheme)) => {
                    MouseRoute::Intents(vec![Intent::PickerCursorTo(grapheme)])
                }
                (None, None) => MouseRoute::Nothing,
            }
        }
    }
}

/// Where a left click at `at` sends the keys and what it selects; `row` is
/// the sidebar row under it.
fn route_click(
    at: Position,
    hits: &HitMap,
    focus: Focus,
    pane_shown: bool,
    row: Option<SidebarItem>,
    click: Click,
) -> MouseRoute {
    let attach = (click == Click::Double).then_some(Intent::Attach);
    let lead = match focus {
        Focus::Search => {
            return match (row, hits.text_at(at), hits.right.contains(at)) {
                (Some(item), ..) => intents([Some(Intent::SelectRow(item)), attach]),
                (None, Some(grapheme), _) => {
                    MouseRoute::Intents(vec![Intent::PickerCursorTo(grapheme)])
                }
                (None, None, true) => {
                    MouseRoute::Intents(vec![Intent::PickerConfirm, Intent::FocusRight])
                }
                (None, None, false) => MouseRoute::Nothing,
            };
        }
        Focus::Attached => Some(Intent::LeavePane),
        Focus::Dashboard => Some(Intent::FocusSidebar),
        Focus::Sidebar | Focus::Picker | Focus::Rename => None,
    };
    if hits.on_sidebar_input(at) {
        return intents([lead, Some(Intent::Search)]);
    }
    if let Some(item) = row {
        return intents([lead, Some(Intent::SelectRow(item)), attach]);
    }
    match (focus, hits.right.contains(at), hits.menu_item_at(at)) {
        (Focus::Sidebar, true, item) => intents([
            Some(Intent::FocusRight),
            item.filter(|_| !pane_shown).map(Intent::DashboardHighlight),
        ]),
        (Focus::Dashboard, _, Some(index)) => {
            MouseRoute::Intents(vec![Intent::DashboardHighlight(index)])
        }
        _ => MouseRoute::Nothing,
    }
}

/// The intents that are there, in order.
fn intents<I>(list: I) -> MouseRoute
where
    I: IntoIterator<Item = Option<Intent>>,
{
    MouseRoute::Intents(list.into_iter().flatten().collect())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use orb_domain::feat::sessions::state::{SidebarItem, ThreadId};
    use orb_domain::{Focus, Intent};
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;

    use super::{Click, ClickTarget, Clicks, HitMap, MouseRoute, route};

    const THREAD_1: SidebarItem = SidebarItem::Thread(ThreadId(1));

    /// A 30-column sidebar and a 50-column right side, 20 lines tall over the
    /// mode line: the input box on lines 0 to 2, thread 1 on lines 3 to 5,
    /// and the dashboard's third menu item on line 10.
    fn hits() -> HitMap {
        let mut hits = HitMap::new(Rect::new(0, 0, 30, 20), Rect::new(30, 0, 50, 20));
        hits.record_sidebar_input(Rect::new(0, 0, 29, 3));
        hits.record_row(Rect::new(0, 3, 29, 3), THREAD_1);
        hits.record_menu_item(Rect::new(40, 10, 30, 1), 2);
        hits
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn left_click(column: u16, row: u16) -> MouseEvent {
        mouse(MouseEventKind::Down(MouseButton::Left), column, row)
    }

    /// Routes `event` with `focus`, no pane shown and no earlier click.
    fn route_once(event: MouseEvent, focus: Focus) -> MouseRoute {
        route(
            event,
            &hits(),
            focus,
            false,
            &mut Clicks::default(),
            Instant::now(),
        )
    }

    /// The intents a route runs; empty for any other route.
    fn intents_of(route: MouseRoute) -> Vec<Intent> {
        match route {
            MouseRoute::Intents(intents) => intents,
            MouseRoute::Forward | MouseRoute::ScrollSidebar(_) | MouseRoute::Nothing => vec![],
        }
    }

    #[rstest::rstest]
    fn click_on_a_row_with_the_sidebar_focused_selects_it() {
        // Given the sidebar has the keys.
        // When clicking thread 1's row.
        let routed = route_once(left_click(5, 4), Focus::Sidebar);

        // Then the row is selected.
        assert_eq!(
            routed,
            MouseRoute::Intents(vec![Intent::SelectRow(THREAD_1)]),
            "a click on a row should select it"
        );
    }

    #[rstest::rstest]
    fn click_on_a_row_from_the_pane_leaves_it_first() {
        // Given the attached pane has the keys.
        // When clicking thread 1's row.
        let routed = route_once(left_click(5, 4), Focus::Attached);

        // Then the pane is left, then the row is selected.
        assert_eq!(
            routed,
            MouseRoute::Intents(vec![Intent::LeavePane, Intent::SelectRow(THREAD_1)]),
            "a click on a row should leave the pane first"
        );
    }

    #[rstest::rstest]
    fn double_click_on_a_row_attaches() {
        // Given the sidebar has the keys and thread 1's row was just clicked.
        let (hits, mut clicks, now) = (hits(), Clicks::default(), Instant::now());
        route(
            left_click(5, 4),
            &hits,
            Focus::Sidebar,
            false,
            &mut clicks,
            now,
        );

        // When clicking it again 100 ms later.
        let routed = route(
            left_click(5, 4),
            &hits,
            Focus::Sidebar,
            false,
            &mut clicks,
            now + Duration::from_millis(100),
        );

        // Then the last intent attaches.
        assert_eq!(
            intents_of(routed).last(),
            Some(&Intent::Attach),
            "a double-click on a row should attach"
        );
    }

    #[rstest::rstest]
    fn click_on_the_pane_while_attached_is_forwarded() {
        // Given the attached pane has the keys.
        // When clicking inside it.
        let routed = route_once(left_click(50, 5), Focus::Attached);

        // Then the click goes to the pane.
        assert_eq!(
            routed,
            MouseRoute::Forward,
            "a click on the attached pane should go to it"
        );
    }

    #[rstest::rstest]
    fn click_on_the_right_side_from_the_sidebar_focuses_right() {
        // Given the sidebar has the keys.
        // When clicking the right side away from the menu.
        let routed = route_once(left_click(35, 5), Focus::Sidebar);

        // Then the keys move right.
        assert_eq!(
            intents_of(routed).first(),
            Some(&Intent::FocusRight),
            "a click on the right side should focus it"
        );
    }

    #[rstest::rstest]
    fn click_on_a_menu_item_with_the_dashboard_focused_highlights_it() {
        // Given the dashboard has the keys.
        // When clicking its third menu item.
        let routed = route_once(left_click(50, 10), Focus::Dashboard);

        // Then that item is highlighted.
        assert_eq!(
            routed,
            MouseRoute::Intents(vec![Intent::DashboardHighlight(2)]),
            "a click on a menu item should highlight it"
        );
    }

    #[rstest::rstest]
    fn wheel_over_the_sidebar_with_its_keys_moves_the_selection() {
        // Given the sidebar has the keys.
        // When wheeling down over it.
        let routed = route_once(mouse(MouseEventKind::ScrollDown, 5, 10), Focus::Sidebar);

        // Then the selection moves down.
        assert_eq!(
            routed,
            MouseRoute::Intents(vec![Intent::SelectWheelNext]),
            "the wheel should move the sidebar's selection"
        );
    }

    #[rstest::rstest]
    fn wheel_over_the_sidebar_from_the_pane_scrolls_its_view() {
        // Given the attached pane has the keys.
        // When wheeling down over the sidebar.
        let routed = route_once(mouse(MouseEventKind::ScrollDown, 5, 10), Focus::Attached);

        // Then the sidebar's view scrolls three lines.
        assert_eq!(
            routed,
            MouseRoute::ScrollSidebar(3),
            "the wheel should scroll the sidebar's view"
        );
    }

    #[rstest::rstest]
    #[case::sidebar(Focus::Sidebar)]
    #[case::attached(Focus::Attached)]
    #[case::dashboard(Focus::Dashboard)]
    fn click_on_the_input_box_starts_a_search(#[case] focus: Focus) {
        // Given `focus` has the keys.
        // When clicking the sidebar's input box.
        let routed = route_once(left_click(5, 1), focus);

        // Then the last intent starts a search.
        assert_eq!(
            intents_of(routed).last(),
            Some(&Intent::Search),
            "a click on the input box from {focus:?} should search"
        );
    }

    #[rstest::rstest]
    fn click_on_a_row_during_a_search_selects_it() {
        // Given a search has the keys.
        // When clicking thread 1's row.
        let routed = route_once(left_click(5, 4), Focus::Search);

        // Then the row is selected.
        assert_eq!(
            routed,
            MouseRoute::Intents(vec![Intent::SelectRow(THREAD_1)]),
            "a click on a row during a search should select it"
        );
    }

    #[rstest::rstest]
    fn click_on_the_right_side_during_a_search_confirms_and_focuses_right() {
        // Given a search has the keys.
        // When clicking the right side.
        let routed = route_once(left_click(50, 5), Focus::Search);

        // Then the search ends on its match and the keys move right.
        assert_eq!(
            routed,
            MouseRoute::Intents(vec![Intent::PickerConfirm, Intent::FocusRight]),
            "a click on the right side should end the search there"
        );
    }

    #[rstest::rstest]
    fn click_on_the_mode_line_does_nothing() {
        // Given the sidebar has the keys.
        // When clicking the mode line below both sides.
        let routed = route_once(left_click(5, 20), Focus::Sidebar);

        // Then nothing happens.
        assert_eq!(
            routed,
            MouseRoute::Nothing,
            "a click on the mode line should do nothing"
        );
    }

    /// `hits()` under a picker popup over columns 20 to 59, lines 4 to 13
    /// (it covers part of thread 1's row), with shown row 1 on line 8.
    fn picker_hits() -> HitMap {
        let mut hits = hits();
        hits.record_overlay(Rect::new(20, 4, 40, 10));
        hits.record_selector(Rect::new(20, 4, 40, 10));
        hits.record_picker_row(Rect::new(21, 8, 38, 1), 1);
        hits
    }

    /// `hits()` under a session picker popup over columns 20 to 59, lines 4
    /// to 13: the list box on columns 20 to 37, the preview on 39 to 59.
    fn session_picker_hits() -> HitMap {
        let mut hits = hits();
        hits.record_overlay(Rect::new(20, 4, 40, 10));
        hits.record_selector(Rect::new(20, 4, 18, 10));
        hits
    }

    /// Routes `event` over `hits` with `focus`, no pane shown and no earlier
    /// click.
    fn route_over(event: MouseEvent, hits: &HitMap, focus: Focus) -> MouseRoute {
        route(
            event,
            hits,
            focus,
            false,
            &mut Clicks::default(),
            Instant::now(),
        )
    }

    #[rstest::rstest]
    fn click_outside_a_picker_cancels_it() {
        // Given a picker has the keys.
        // When clicking thread 1's row, outside the popup.
        let routed = route_over(left_click(5, 4), &picker_hits(), Focus::Picker);

        // Then the picker is cancelled and the row isn't selected.
        assert_eq!(
            routed,
            MouseRoute::Intents(vec![Intent::PickerCancel]),
            "a click outside a picker should only cancel it"
        );
    }

    #[rstest::rstest]
    fn click_on_a_picker_row_selects_it() {
        // Given a picker has the keys.
        // When clicking its shown row 1.
        let routed = route_over(left_click(30, 8), &picker_hits(), Focus::Picker);

        // Then that row is selected.
        assert_eq!(
            routed,
            MouseRoute::Intents(vec![Intent::PickerSelectRow(1)]),
            "a click on a picker row should select it"
        );
    }

    #[rstest::rstest]
    fn double_click_on_a_picker_row_picks_it() {
        // Given a picker has the keys and its row 1 was just clicked.
        let (hits, mut clicks, now) = (picker_hits(), Clicks::default(), Instant::now());
        route(
            left_click(30, 8),
            &hits,
            Focus::Picker,
            false,
            &mut clicks,
            now,
        );

        // When clicking it again 100 ms later.
        let routed = route(
            left_click(30, 8),
            &hits,
            Focus::Picker,
            false,
            &mut clicks,
            now + Duration::from_millis(100),
        );

        // Then the row is selected and picked.
        assert_eq!(
            routed,
            MouseRoute::Intents(vec![Intent::PickerSelectRow(1), Intent::PickerConfirm]),
            "a double-click on a picker row should pick it"
        );
    }

    #[rstest::rstest]
    #[case::down(MouseEventKind::ScrollDown, Intent::PickerWheelNext)]
    #[case::up(MouseEventKind::ScrollUp, Intent::PickerWheelPrev)]
    fn wheel_over_a_picker_moves_its_selection(
        #[case] kind: MouseEventKind,
        #[case] expected: Intent,
    ) {
        // Given a picker has the keys.
        // When wheeling over its popup.
        let routed = route_over(mouse(kind, 30, 6), &picker_hits(), Focus::Picker);

        // Then the picker's selection moves.
        assert_eq!(
            routed,
            MouseRoute::Intents(vec![expected]),
            "the wheel over a picker should move its selection"
        );
    }

    #[rstest::rstest]
    fn wheel_outside_a_picker_does_nothing() {
        // Given a picker has the keys.
        // When wheeling down over the sidebar, outside the popup.
        let routed = route_over(
            mouse(MouseEventKind::ScrollDown, 5, 15),
            &picker_hits(),
            Focus::Picker,
        );

        // Then nothing happens.
        assert_eq!(
            routed,
            MouseRoute::Nothing,
            "the wheel outside a picker should do nothing"
        );
    }

    #[rstest::rstest]
    fn wheel_over_the_session_pickers_list_moves_its_selection() {
        // Given the session picker has the keys.
        // When wheeling down over its list box.
        let routed = route_over(
            mouse(MouseEventKind::ScrollDown, 25, 8),
            &session_picker_hits(),
            Focus::Picker,
        );

        // Then the picker's selection moves down.
        assert_eq!(
            routed,
            MouseRoute::Intents(vec![Intent::PickerWheelNext]),
            "the wheel over the session picker's list should move its selection"
        );
    }

    #[rstest::rstest]
    fn wheel_over_the_session_pickers_preview_does_nothing() {
        // Given the session picker has the keys.
        // When wheeling down over its preview.
        let routed = route_over(
            mouse(MouseEventKind::ScrollDown, 50, 8),
            &session_picker_hits(),
            Focus::Picker,
        );

        // Then nothing happens.
        assert_eq!(
            routed,
            MouseRoute::Nothing,
            "the wheel over the session picker's preview should do nothing"
        );
    }

    #[rstest::rstest]
    fn click_inside_a_picker_off_its_rows_does_nothing() {
        // Given a picker has the keys.
        // When clicking inside the popup on no row.
        let routed = route_over(left_click(30, 5), &picker_hits(), Focus::Picker);

        // Then nothing happens: the picker stays open.
        assert_eq!(
            routed,
            MouseRoute::Nothing,
            "a click inside a picker off its rows should do nothing"
        );
    }

    #[rstest::rstest]
    fn double_click_inside_a_picker_off_its_rows_does_nothing() {
        // Given a picker has the keys and a spot on no row was just clicked.
        let (hits, mut clicks, now) = (picker_hits(), Clicks::default(), Instant::now());
        route(
            left_click(30, 5),
            &hits,
            Focus::Picker,
            false,
            &mut clicks,
            now,
        );

        // When clicking it again 100 ms later.
        let routed = route(
            left_click(30, 5),
            &hits,
            Focus::Picker,
            false,
            &mut clicks,
            now + Duration::from_millis(100),
        );

        // Then nothing happens: the selected row isn't picked.
        assert_eq!(
            routed,
            MouseRoute::Nothing,
            "a double-click inside a picker off its rows should pick nothing"
        );
    }

    /// `hits()` while searching for "thread": the box's inner line is
    /// columns 1 to 27 of line 1, the text after the two-column `> `.
    fn search_hits() -> HitMap {
        let mut hits = hits();
        hits.record_text(Rect::new(1, 1, 27, 1), 2, "thread", 6, 24);
        hits
    }

    /// `picker_hits()` with "alpha" typed on the input line (line 5), after
    /// the three-column ` > `.
    fn input_hits() -> HitMap {
        let mut hits = picker_hits();
        hits.record_text(Rect::new(21, 5, 38, 1), 3, "alpha", 5, 34);
        hits
    }

    #[rstest::rstest]
    fn click_on_the_search_text_moves_its_cursor() {
        // Given the sidebar search has the keys, "thread" typed.
        // When clicking the "r" of "thread".
        let routed = route_over(left_click(5, 1), &search_hits(), Focus::Search);

        // Then the search cursor moves before the "r".
        assert_eq!(
            routed,
            MouseRoute::Intents(vec![Intent::PickerCursorTo(2)]),
            "a click on the search text should move its cursor there"
        );
    }

    #[rstest::rstest]
    fn click_on_the_search_prompt_moves_the_cursor_to_the_start() {
        // Given the sidebar search has the keys, "thread" typed.
        // When clicking the `>` of the prompt.
        let routed = route_over(left_click(1, 1), &search_hits(), Focus::Search);

        // Then the search cursor moves to the first shown grapheme.
        assert_eq!(
            routed,
            MouseRoute::Intents(vec![Intent::PickerCursorTo(0)]),
            "a click on the prompt should move the cursor to the start"
        );
    }

    #[rstest::rstest]
    fn click_on_the_search_box_border_does_nothing() {
        // Given the sidebar search has the keys, "thread" typed.
        // When clicking the box's top border, off the text line.
        let routed = route_over(left_click(5, 0), &search_hits(), Focus::Search);

        // Then nothing happens.
        assert_eq!(
            routed,
            MouseRoute::Nothing,
            "a click on the search box's border should do nothing"
        );
    }

    #[rstest::rstest]
    #[case::picker(Focus::Picker)]
    #[case::rename(Focus::Rename)]
    fn click_on_an_inputs_text_moves_its_cursor(#[case] focus: Focus) {
        // Given a picker or the rename box has the keys, "alpha" typed.
        // When clicking the "p" of "alpha".
        let routed = route_over(left_click(26, 5), &input_hits(), focus);

        // Then its cursor moves before the "p".
        assert_eq!(
            routed,
            MouseRoute::Intents(vec![Intent::PickerCursorTo(2)]),
            "a click on the input's text should move its cursor there with {focus:?}"
        );
    }

    #[rstest::rstest]
    fn click_before_the_picker_is_drawn_does_nothing() {
        // Given a picker has the keys but no frame has drawn it yet.
        // When clicking thread 1's row.
        let routed = route_over(left_click(5, 4), &hits(), Focus::Picker);

        // Then nothing happens.
        assert_eq!(
            routed,
            MouseRoute::Nothing,
            "a click before the picker is drawn should do nothing"
        );
    }

    #[rstest::rstest]
    fn click_outside_the_rename_box_cancels_it() {
        // Given the rename box has the keys, drawn over columns 20 to 59,
        // lines 2 to 4.
        let hits = {
            let mut hits = hits();
            hits.record_overlay(Rect::new(20, 2, 40, 3));
            hits
        };

        // When clicking outside it.
        let routed = route_over(left_click(5, 10), &hits, Focus::Rename);

        // Then the rename box is cancelled.
        assert_eq!(
            routed,
            MouseRoute::Intents(vec![Intent::PickerCancel]),
            "a click outside the rename box should cancel it"
        );
    }

    const ROW_1: Option<ClickTarget> = Some(ClickTarget::Row(THREAD_1));

    #[rstest::rstest]
    fn two_quick_clicks_on_the_same_row_are_a_double_click() {
        // Given a click on thread 1's row.
        let (mut clicks, now) = (Clicks::default(), Instant::now());
        clicks.click(ROW_1, now);

        // When clicking it again 100 ms later.
        let click = clicks.click(ROW_1, now + Duration::from_millis(100));

        // Then it's a double-click.
        assert_eq!(click, Click::Double, "the second quick click");
    }

    #[rstest::rstest]
    fn clicks_600ms_apart_are_two_singles() {
        // Given a click on thread 1's row.
        let (mut clicks, now) = (Clicks::default(), Instant::now());
        clicks.click(ROW_1, now);

        // When clicking it again 600 ms later.
        let click = clicks.click(ROW_1, now + Duration::from_millis(600));

        // Then it's a single click.
        assert_eq!(click, Click::Single, "the second slow click");
    }

    #[rstest::rstest]
    fn clicks_on_different_rows_are_two_singles() {
        // Given a click on thread 1's row.
        let (mut clicks, now) = (Clicks::default(), Instant::now());
        clicks.click(ROW_1, now);

        // When clicking thread 2's row 100 ms later.
        let click = clicks.click(
            Some(ClickTarget::Row(SidebarItem::Thread(ThreadId(2)))),
            now + Duration::from_millis(100),
        );

        // Then it's a single click.
        assert_eq!(click, Click::Single, "a quick click on another row");
    }

    #[rstest::rstest]
    fn third_quick_click_is_a_single() {
        // Given a double-click on thread 1's row.
        let (mut clicks, now) = (Clicks::default(), Instant::now());
        clicks.click(ROW_1, now);
        clicks.click(ROW_1, now + Duration::from_millis(100));

        // When clicking it a third time 100 ms later.
        let click = clicks.click(ROW_1, now + Duration::from_millis(200));

        // Then it's a single click.
        assert_eq!(click, Click::Single, "the third quick click");
    }
}
