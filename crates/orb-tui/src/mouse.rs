//! The mouse in orb's own UI: a click on the sidebar or the right side moves
//! the keys there, a click on a row selects it and a double-click attaches,
//! a click on the sidebar's input box starts a search, and a click on a
//! dashboard item highlights it. The wheel moves the sidebar's selection
//! while it has the keys, and otherwise scrolls its view. While attached, the
//! pane gets its own mouse events, as Claude expects.
//!
//! Each frame records where it drew what a click can land on, and a mouse
//! event is mapped back through that record.

use std::time::{Duration, Instant};

use orb_domain::feat::sessions::state::SidebarItem;
use orb_domain::{Focus, Intent};
use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

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
}

/// What a click can double on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClickTarget {
    /// A sidebar row.
    Row(SidebarItem),
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
        (Focus::Picker | Focus::Rename, _) => MouseRoute::Nothing,
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
            return match (row, hits.right.contains(at)) {
                (Some(item), _) => intents([Some(Intent::SelectRow(item)), attach]),
                (None, true) => {
                    MouseRoute::Intents(vec![Intent::PickerConfirm, Intent::FocusRight])
                }
                (None, false) => MouseRoute::Nothing,
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
