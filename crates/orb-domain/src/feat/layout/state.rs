//! Each attached session's tabs, which tab is shown, and the tab body's size.

use std::collections::{HashMap, HashSet};

use ratatui::layout::Rect;

use super::tree::{NavDirection, Split, TileLayout, find_in_direction};
use crate::feat::sessions::state::{PaneId, ThreadId};
use crate::feat::sidebar::state::STEP;

/// One tab: its name, its panes' tree, and whether it shows only the focused pane.
#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
    /// Set by renaming the tab; unnamed tabs show only their number.
    name: Option<String>,
    tree: TileLayout,
    zoomed: bool,
}

impl Tab {
    fn new(pane: PaneId) -> Self {
        Self {
            name: None,
            tree: TileLayout::new(pane),
            zoomed: false,
        }
    }

    /// What the tab bar shows for the tab at 0-based `index`: `"1"` or `"1 name"`.
    pub fn label(&self, index: usize) -> String {
        match &self.name {
            Some(name) => format!("{} {name}", index + 1),
            None => (index + 1).to_string(),
        }
    }

    /// The name the user gave the tab, if any.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Whether the tab shows only its focused pane.
    pub fn zoomed(&self) -> bool {
        self.zoomed
    }

    /// Whether pane `pane` is one of this tab's.
    pub fn holds(&self, pane: PaneId) -> bool {
        self.tree.pane_ids().contains(&pane)
    }
}

/// A session's tabs, in order, and the one shown.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionLayout {
    tabs: Vec<Tab>,
    active: usize,
}

impl SessionLayout {
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// The shown tab's 0-based index.
    pub fn active(&self) -> usize {
        self.active
    }

    /// The shown tab.
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    /// The shown tab's focused pane.
    pub fn focused(&self) -> Option<PaneId> {
        self.active_tab().map(|tab| tab.tree.focused())
    }

    /// Where the shown tab's panes are drawn in `body`. A zoomed tab places
    /// only its focused pane, over all of `body`; otherwise each pane gives
    /// up its left column and top row to the border when one runs there.
    pub fn placed(&self, body: Rect) -> Vec<Placed> {
        let Some(tab) = self.active_tab() else {
            return vec![];
        };
        if tab.zoomed {
            return vec![Placed {
                pane: tab.tree.focused(),
                area: body,
                focused: true,
            }];
        }
        tab.tree
            .panes(body)
            .into_iter()
            .map(|info| {
                let left = u16::from(info.rect.x > body.x);
                let top = u16::from(info.rect.y > body.y);
                Placed {
                    pane: info.id,
                    area: Rect::new(
                        info.rect.x + left,
                        info.rect.y + top,
                        info.rect.width.saturating_sub(left),
                        info.rect.height.saturating_sub(top),
                    ),
                    focused: info.is_focused,
                }
            })
            .collect()
    }

    fn tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    /// Removes tab `index`, showing the tab before it when it was shown.
    fn remove_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.tabs.remove(index);
            if index <= self.active {
                self.active = self.active.saturating_sub(1);
            }
        }
    }
}

/// Where a pane is drawn: its content area (borders already taken off) and
/// whether it has the layout's focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub pane: PaneId,
    pub area: Rect,
    pub focused: bool,
}

/// What a focus move did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusMove {
    /// Another pane, or the next tab's focused pane, has the focus.
    Moved,
    /// Nothing is left of the focused pane: the keys go to the sidebar.
    Sidebar,
    /// Nothing that way; nothing changed.
    Stuck,
}

/// Every attached thread's layout. Written by the intent handler and by the
/// frontend, which opens a layout for each pane it reattaches at start,
/// drops the layout of a thread that left `attached`, closes panes whose
/// program ended, and records the tab body's size after each draw.
#[derive(Debug, Default)]
pub struct Layouts {
    sessions: HashMap<ThreadId, SessionLayout>,
    /// The id the next split-off pane gets, once decremented; counts down
    /// from -1.
    next_pane: i64,
    /// The tab body's area in the last frame; focus moves and resizes
    /// measure against it.
    body: Rect,
}

impl Layouts {
    /// Makes a one-tab layout of `owner`'s own pane when it has none.
    pub fn open(&mut self, owner: ThreadId) {
        self.sessions.entry(owner).or_insert_with(|| SessionLayout {
            tabs: vec![Tab::new(PaneId::from(owner))],
            active: 0,
        });
    }

    pub fn get(&self, owner: ThreadId) -> Option<&SessionLayout> {
        self.sessions.get(&owner)
    }

    /// The thread whose layout holds `pane`.
    pub fn owner_of(&self, pane: PaneId) -> Option<ThreadId> {
        self.sessions
            .iter()
            .find(|(_, layout)| layout.tabs.iter().any(|tab| tab.holds(pane)))
            .map(|(owner, _)| *owner)
    }

    /// Every pane of every tab of every layout.
    pub fn pane_ids(&self) -> HashSet<PaneId> {
        self.sessions
            .values()
            .flat_map(|layout| &layout.tabs)
            .flat_map(|tab| tab.tree.pane_ids())
            .collect()
    }

    /// The tab body's area in the last frame.
    pub fn body(&self) -> Rect {
        self.body
    }

    /// The tab body now measures `body`.
    pub fn fit_to(&mut self, body: Rect) {
        self.body = body;
    }

    /// Drops the layout of every thread not in `attached`.
    pub fn retain_attached(&mut self, attached: &HashSet<ThreadId>) {
        self.sessions.retain(|owner, _| attached.contains(owner));
    }

    /// Splits the shown tab's focused pane `split`; the new pane takes the
    /// focus and the tab shows every pane again.
    pub fn split(&mut self, owner: ThreadId, split: Split) {
        if !self.sessions.contains_key(&owner) {
            return;
        }
        let id = self.next_id();
        if let Some(tab) = self.tab_mut(owner) {
            tab.tree.split_focused(split, id);
            tab.zoomed = false;
        }
    }

    /// Appends a tab of one new pane and shows it.
    pub fn new_tab(&mut self, owner: ThreadId) {
        if !self.sessions.contains_key(&owner) {
            return;
        }
        let id = self.next_id();
        if let Some(layout) = self.sessions.get_mut(&owner) {
            layout.tabs.push(Tab::new(id));
            layout.active = layout.tabs.len() - 1;
        }
    }

    /// Closes `pane` wherever it is. A tab left without panes goes, and the
    /// tab before it is shown; a layout left without tabs goes.
    pub fn close_pane(&mut self, pane: PaneId) {
        let Some(owner) = self.owner_of(pane) else {
            return;
        };
        let Some(layout) = self.sessions.get_mut(&owner) else {
            return;
        };
        let Some(index) = layout.tabs.iter().position(|tab| tab.holds(pane)) else {
            return;
        };
        let emptied = layout.tabs.get_mut(index).is_some_and(|tab| {
            tab.zoomed = false;
            !tab.tree.close_pane(pane)
        });
        if emptied {
            layout.remove_tab(index);
        }
        self.drop_if_empty(owner);
    }

    /// Closes the shown tab; the tab before it is shown. A layout left
    /// without tabs goes.
    pub fn close_tab(&mut self, owner: ThreadId) {
        if let Some(layout) = self.sessions.get_mut(&owner) {
            layout.remove_tab(layout.active);
        }
        self.drop_if_empty(owner);
    }

    /// Moves the focus to the pane `nav` of the focused one. With none to the
    /// left the keys go to the sidebar; with none to the right the next tab
    /// is shown.
    pub fn move_focus(&mut self, owner: ThreadId, nav: NavDirection) -> FocusMove {
        let body = self.body;
        let Some(layout) = self.sessions.get_mut(&owner) else {
            return FocusMove::Stuck;
        };
        let tab_count = layout.tabs.len();
        let active = layout.active;
        let Some(tab) = layout.tab_mut() else {
            return FocusMove::Stuck;
        };
        let panes = tab.tree.panes(body);
        let neighbour = panes
            .iter()
            .find(|info| info.is_focused)
            .and_then(|focused| find_in_direction(focused, nav, &panes));
        match (neighbour, nav) {
            (Some(id), _) => {
                tab.tree.focus_pane(id);
                FocusMove::Moved
            }
            (None, NavDirection::Left) => FocusMove::Sidebar,
            (None, NavDirection::Right) if active + 1 < tab_count => {
                layout.active = active + 1;
                FocusMove::Moved
            }
            (None, _) => FocusMove::Stuck,
        }
    }

    /// Focuses `pane` in `owner`'s shown tab.
    pub fn focus_pane(&mut self, owner: ThreadId, pane: PaneId) {
        if let Some(tab) = self.tab_mut(owner) {
            tab.tree.focus_pane(pane);
        }
    }

    /// Shows only the focused pane over the shown tab, or every pane again.
    pub fn toggle_zoom(&mut self, owner: ThreadId) {
        if let Some(tab) = self.tab_mut(owner) {
            tab.zoomed = !tab.zoomed;
        }
    }

    /// Grows (or shrinks) the focused pane a step; whether it changed.
    pub fn resize_focused(&mut self, owner: ThreadId, grow: bool) -> bool {
        let body = self.body;
        self.tab_mut(owner)
            .is_some_and(|tab| tab.tree.resize_focused(grow, STEP, body))
    }

    /// Shows tab `n`, counting from 1; past the last changes nothing.
    pub fn go_to_tab(&mut self, owner: ThreadId, n: usize) {
        if let Some(layout) = self.sessions.get_mut(&owner)
            && (1..=layout.tabs.len()).contains(&n)
        {
            layout.active = n - 1;
        }
    }

    /// Shows the next tab, wrapping to the first.
    pub fn next_tab(&mut self, owner: ThreadId) {
        if let Some(layout) = self.sessions.get_mut(&owner) {
            layout.active = (layout.active + 1) % layout.tabs.len().max(1);
        }
    }

    /// Shows the previous tab, wrapping to the last.
    pub fn previous_tab(&mut self, owner: ThreadId) {
        if let Some(layout) = self.sessions.get_mut(&owner) {
            let count = layout.tabs.len().max(1);
            layout.active = (layout.active + count - 1) % count;
        }
    }

    /// Swaps the shown tab with the one before it; the first stays put.
    pub fn move_tab_left(&mut self, owner: ThreadId) {
        if let Some(layout) = self.sessions.get_mut(&owner)
            && layout.active > 0
        {
            layout.tabs.swap(layout.active, layout.active - 1);
            layout.active -= 1;
        }
    }

    /// Swaps the shown tab with the one after it; the last stays put.
    pub fn move_tab_right(&mut self, owner: ThreadId) {
        if let Some(layout) = self.sessions.get_mut(&owner)
            && layout.active + 1 < layout.tabs.len()
        {
            layout.tabs.swap(layout.active, layout.active + 1);
            layout.active += 1;
        }
    }

    /// Names tab `tab` (0-based) of `owner`'s layout; `None` clears the name.
    pub fn rename_tab(&mut self, owner: ThreadId, tab: usize, name: Option<String>) {
        if let Some(tab) = self
            .sessions
            .get_mut(&owner)
            .and_then(|layout| layout.tabs.get_mut(tab))
        {
            tab.name = name;
        }
    }

    fn next_id(&mut self) -> PaneId {
        self.next_pane -= 1;
        PaneId(self.next_pane)
    }

    fn tab_mut(&mut self, owner: ThreadId) -> Option<&mut Tab> {
        self.sessions.get_mut(&owner)?.tab_mut()
    }

    fn drop_if_empty(&mut self, owner: ThreadId) {
        if self
            .sessions
            .get(&owner)
            .is_some_and(|layout| layout.tabs.is_empty())
        {
            self.sessions.remove(&owner);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use ratatui::layout::Rect;

    use super::{FocusMove, Layouts, Placed, SessionLayout};
    use crate::feat::layout::tree::{NavDirection, Split};
    use crate::feat::sessions::state::{PaneId, ThreadId};

    const OWNER: ThreadId = ThreadId(7);
    const BODY: Rect = Rect::new(0, 0, 80, 24);

    /// Thread 7's layout, opened, measured against an 80×24 body.
    fn opened() -> Layouts {
        let mut layouts = Layouts::default();
        layouts.open(OWNER);
        layouts.fit_to(BODY);
        layouts
    }

    /// Thread 7's layout split right once: its own pane on the left, pane
    /// -1 on the right and focused.
    fn split_right() -> Layouts {
        let mut layouts = opened();
        layouts.split(OWNER, Split::Right);
        layouts
    }

    fn placed(layouts: &Layouts) -> Vec<Placed> {
        layouts
            .get(OWNER)
            .map(|layout| layout.placed(BODY))
            .unwrap_or_default()
    }

    fn focused(layouts: &Layouts) -> Option<PaneId> {
        layouts.get(OWNER)?.focused()
    }

    fn active(layouts: &Layouts) -> Option<usize> {
        layouts.get(OWNER).map(SessionLayout::active)
    }

    fn tab_count(layouts: &Layouts) -> usize {
        layouts.get(OWNER).map_or(0, |layout| layout.tabs().len())
    }

    /// Thread 7's layout with three tabs, the first shown.
    fn three_tabs() -> Layouts {
        let mut layouts = opened();
        layouts.new_tab(OWNER);
        layouts.new_tab(OWNER);
        layouts.go_to_tab(OWNER, 1);
        layouts
    }

    #[rstest::rstest]
    fn open_makes_one_tab_of_the_threads_pane() {
        // Given no layouts.
        let mut layouts = Layouts::default();

        // When opening thread 7's.
        layouts.open(OWNER);

        // Then it has one tab focused on the thread's own pane.
        assert_eq!(
            (tab_count(&layouts), focused(&layouts)),
            (1, Some(PaneId(7))),
            "a new layout is one tab of the thread's pane"
        );
    }

    #[rstest::rstest]
    fn split_numbers_new_panes_down_from_minus_one() {
        // Given an opened layout.
        let mut layouts = opened();

        // When splitting twice.
        layouts.split(OWNER, Split::Right);
        layouts.split(OWNER, Split::Down);

        // Then the new panes are -1 and -2.
        let panes: Vec<PaneId> = placed(&layouts).iter().map(|place| place.pane).collect();
        assert_eq!(
            panes,
            vec![PaneId(7), PaneId(-1), PaneId(-2)],
            "split-off panes count down from -1"
        );
    }

    #[rstest::rstest]
    fn split_clears_the_zoom() {
        // Given a zoomed two-pane tab.
        let mut layouts = split_right();
        layouts.toggle_zoom(OWNER);

        // When splitting again.
        layouts.split(OWNER, Split::Down);

        // Then the tab isn't zoomed.
        assert_eq!(
            placed(&layouts).len(),
            3,
            "a split should show every pane again"
        );
    }

    #[rstest::rstest]
    fn move_focus_left_from_the_leftmost_pane_asks_for_the_sidebar() {
        // Given a lone pane.
        let mut layouts = opened();

        // When moving the focus left.
        let moved = layouts.move_focus(OWNER, NavDirection::Left);

        // Then the keys should go to the sidebar.
        assert_eq!(moved, FocusMove::Sidebar, "leftmost left is the sidebar");
    }

    #[rstest::rstest]
    fn move_focus_right_from_the_rightmost_pane_goes_to_the_next_tab() {
        // Given three tabs, the first shown.
        let mut layouts = three_tabs();

        // When moving the focus right.
        let moved = layouts.move_focus(OWNER, NavDirection::Right);

        // Then the second tab is shown.
        assert_eq!(
            (moved, active(&layouts)),
            (FocusMove::Moved, Some(1)),
            "rightmost right shows the next tab"
        );
    }

    #[rstest::rstest]
    fn move_focus_right_on_the_last_tab_is_stuck() {
        // Given a lone pane in the only tab.
        let mut layouts = opened();

        // When moving the focus right.
        let moved = layouts.move_focus(OWNER, NavDirection::Right);

        // Then nothing moves.
        assert_eq!(moved, FocusMove::Stuck, "nothing is right of the last tab");
    }

    #[rstest::rstest]
    fn move_focus_moves_to_the_neighbour() {
        // Given two panes side by side, the right one focused.
        let mut layouts = split_right();

        // When moving the focus left.
        layouts.move_focus(OWNER, NavDirection::Left);

        // Then the left pane has the focus.
        assert_eq!(
            focused(&layouts),
            Some(PaneId(7)),
            "the left pane is the neighbour"
        );
    }

    #[rstest::rstest]
    fn zoomed_tab_places_only_the_focused_pane_over_the_body() {
        // Given two panes side by side.
        let mut layouts = split_right();

        // When zooming.
        layouts.toggle_zoom(OWNER);

        // Then only the focused pane is placed, over the whole body.
        assert_eq!(
            placed(&layouts),
            vec![Placed {
                pane: PaneId(-1),
                area: BODY,
                focused: true,
            }],
            "a zoomed tab shows its focused pane alone"
        );
    }

    #[rstest::rstest]
    fn zoom_twice_places_every_pane_again() {
        // Given two panes side by side.
        let mut layouts = split_right();

        // When zooming twice.
        layouts.toggle_zoom(OWNER);
        layouts.toggle_zoom(OWNER);

        // Then both panes are placed.
        assert_eq!(placed(&layouts).len(), 2, "unzoom shows every pane");
    }

    #[rstest::rstest]
    fn placed_panes_leave_a_column_for_the_border_between_them() {
        // Given two panes side by side over an 80-column body.
        let layouts = split_right();

        // When placing them.
        let areas: Vec<Rect> = placed(&layouts).iter().map(|place| place.area).collect();

        // Then the right pane starts one column after the left one ends.
        assert_eq!(
            areas,
            vec![Rect::new(0, 0, 40, 24), Rect::new(41, 0, 39, 24)],
            "column 40 is the border"
        );
    }

    #[rstest::rstest]
    fn placed_panes_leave_a_row_for_the_border_between_them() {
        // Given two stacked panes over a 24-row body.
        let mut layouts = opened();
        layouts.split(OWNER, Split::Down);

        // When placing them.
        let areas: Vec<Rect> = placed(&layouts).iter().map(|place| place.area).collect();

        // Then the bottom pane starts one row after the top one ends.
        assert_eq!(
            areas,
            vec![Rect::new(0, 0, 80, 12), Rect::new(0, 13, 80, 11)],
            "row 12 is the border"
        );
    }

    #[rstest::rstest]
    fn closing_a_tabs_last_pane_closes_the_tab() {
        // Given a second tab of one pane, shown.
        let mut layouts = opened();
        layouts.new_tab(OWNER);

        // When closing that pane.
        layouts.close_pane(PaneId(-1));

        // Then only the first tab is left, shown.
        assert_eq!(
            (tab_count(&layouts), active(&layouts)),
            (1, Some(0)),
            "the emptied tab should close"
        );
    }

    #[rstest::rstest]
    fn closing_the_last_tab_drops_the_layout() {
        // Given a layout of one tab.
        let mut layouts = opened();

        // When closing the tab.
        layouts.close_tab(OWNER);

        // Then the layout is gone.
        assert!(layouts.get(OWNER).is_none(), "no tabs means no layout");
    }

    #[rstest::rstest]
    fn new_tab_is_appended_and_shown() {
        // Given a layout of one tab.
        let mut layouts = opened();

        // When opening a tab.
        layouts.new_tab(OWNER);

        // Then the second tab is shown, focused on a new pane.
        assert_eq!(
            (active(&layouts), focused(&layouts)),
            (Some(1), Some(PaneId(-1))),
            "the new tab is shown"
        );
    }

    #[rstest::rstest]
    fn next_tab_wraps_to_the_first() {
        // Given three tabs, the last shown.
        let mut layouts = three_tabs();
        layouts.go_to_tab(OWNER, 3);

        // When showing the next tab.
        layouts.next_tab(OWNER);

        // Then the first is shown.
        assert_eq!(active(&layouts), Some(0), "next wraps to the first");
    }

    #[rstest::rstest]
    fn previous_tab_wraps_to_the_last() {
        // Given three tabs, the first shown.
        let mut layouts = three_tabs();

        // When showing the previous tab.
        layouts.previous_tab(OWNER);

        // Then the last is shown.
        assert_eq!(active(&layouts), Some(2), "previous wraps to the last");
    }

    #[rstest::rstest]
    fn go_to_tab_past_the_last_changes_nothing() {
        // Given three tabs, the first shown.
        let mut layouts = three_tabs();

        // When going to tab 5.
        layouts.go_to_tab(OWNER, 5);

        // Then the first is still shown.
        assert_eq!(active(&layouts), Some(0), "tab 5 doesn't exist");
    }

    #[rstest::rstest]
    fn move_tab_right_swaps_it_with_its_neighbour() {
        // Given three tabs, the first shown and named.
        let mut layouts = three_tabs();
        layouts.rename_tab(OWNER, 0, Some("logs".into()));

        // When moving it right.
        layouts.move_tab_right(OWNER);

        // Then it is second and still shown.
        let second = layouts
            .get(OWNER)
            .and_then(|layout| layout.tabs().get(1))
            .and_then(|tab| tab.name().map(str::to_owned));
        assert_eq!(
            (second, active(&layouts)),
            (Some("logs".to_owned()), Some(1)),
            "the moved tab is second and shown"
        );
    }

    #[rstest::rstest]
    fn move_tab_left_at_the_first_changes_nothing() {
        // Given three tabs, the first shown.
        let mut layouts = three_tabs();

        // When moving it left.
        layouts.move_tab_left(OWNER);

        // Then it is still first.
        assert_eq!(active(&layouts), Some(0), "the first tab can't move left");
    }

    #[rstest::rstest]
    fn retain_attached_drops_a_detached_threads_layout() {
        // Given thread 7's layout.
        let mut layouts = opened();

        // When keeping only attached thread 8.
        layouts.retain_attached(&HashSet::from([ThreadId(8)]));

        // Then thread 7's layout is gone.
        assert!(
            layouts.get(OWNER).is_none(),
            "detached thread loses its layout"
        );
    }

    #[rstest::rstest]
    fn pane_ids_lists_every_tabs_panes() {
        // Given a split first tab and a second tab.
        let mut layouts = split_right();
        layouts.new_tab(OWNER);

        // When listing the panes.
        let panes = layouts.pane_ids();

        // Then all three are there.
        assert_eq!(
            panes,
            HashSet::from([PaneId(7), PaneId(-1), PaneId(-2)]),
            "panes of every tab"
        );
    }

    #[rstest::rstest]
    fn owner_of_finds_a_split_panes_thread() {
        // Given thread 7's split layout.
        let layouts = split_right();

        // When asking whose pane -1 is.
        let owner = layouts.owner_of(PaneId(-1));

        // Then it is thread 7's.
        assert_eq!(owner, Some(OWNER), "pane -1 is in thread 7's layout");
    }

    #[rstest::rstest]
    fn unnamed_tab_is_labelled_by_its_number() {
        // Given an unnamed first tab.
        let layouts = opened();

        // When labelling it.
        let label = layouts
            .get(OWNER)
            .and_then(|layout| layout.tabs().first().map(|tab| tab.label(0)));

        // Then the label is its number.
        assert_eq!(label.as_deref(), Some("1"), "unnamed tab label");
    }

    #[rstest::rstest]
    fn named_tab_is_labelled_by_number_and_name() {
        // Given a first tab named "logs".
        let mut layouts = opened();
        layouts.rename_tab(OWNER, 0, Some("logs".into()));

        // When labelling it.
        let label = layouts
            .get(OWNER)
            .and_then(|layout| layout.tabs().first().map(|tab| tab.label(0)));

        // Then the label is its number and name.
        assert_eq!(label.as_deref(), Some("1 logs"), "named tab label");
    }
}
