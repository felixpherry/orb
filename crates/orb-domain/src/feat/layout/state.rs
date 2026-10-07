//! Every session's tabs, which tab is shown, where each pane's program runs,
//! and the tab body's size.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ratatui::layout::Rect;

use super::tree::{NavDirection, Split, TileLayout, find_in_direction};
use crate::feat::sessions::state::{PaneId, SessionId};
use crate::feat::sidebar::state::STEP;
use crate::feat::zmx::zmx_service::ZmxSession;

/// Where a pane's program runs: its zmx session and directory, the name the
/// user gave it, and the command that brings its conversation back in a
/// fresh shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneEntry {
    pub id: PaneId,
    pub zmx: ZmxSession,
    pub cwd: PathBuf,
    pub name: Option<String>,
    /// Typed into the pane's shell when zmx has to make its session anew.
    pub resume: Option<String>,
}

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
        Self::restore(None, TileLayout::new(pane))
    }

    /// A saved tab: its name and tree, not zoomed.
    pub fn restore(name: Option<String>, tree: TileLayout) -> Self {
        Self {
            name,
            tree,
            zoomed: false,
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

    /// The tab's focused pane.
    pub fn focused(&self) -> PaneId {
        self.tree.focused()
    }

    /// The tab's tree, as the store saves it.
    pub fn layout_json(&self) -> String {
        self.tree.to_json()
    }
}

/// A session's tabs, in order, the one shown, and where each of their panes
/// runs.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionLayout {
    tabs: Vec<Tab>,
    active: usize,
    panes: HashMap<PaneId, PaneEntry>,
}

impl SessionLayout {
    /// One tab of the one pane `pane`.
    pub fn of(pane: PaneEntry) -> Self {
        Self {
            tabs: vec![Tab::new(pane.id)],
            active: 0,
            panes: HashMap::from([(pane.id, pane)]),
        }
    }

    /// Saved tabs showing tab `active` (the last when past it), with where
    /// their panes run. Entries for panes no tab holds are left out.
    pub fn restore(tabs: Vec<Tab>, active: usize, panes: Vec<PaneEntry>) -> Self {
        let panes = panes
            .into_iter()
            .filter(|entry| tabs.iter().any(|tab| tab.holds(entry.id)))
            .map(|entry| (entry.id, entry))
            .collect();
        Self {
            active: active.min(tabs.len().saturating_sub(1)),
            tabs,
            panes,
        }
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// Where each of the session's panes runs, in no order.
    pub fn panes(&self) -> impl Iterator<Item = &PaneEntry> {
        self.panes.values()
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
        self.active_tab().map(Tab::focused)
    }

    /// What the tab bar shows for the tab at 0-based `index`: its number,
    /// then its name, else its focused pane's name, as `"1"` or `"1 name"`.
    pub fn tab_label(&self, index: usize) -> String {
        let name = self.tabs.get(index).and_then(|tab| {
            tab.name.as_deref().or_else(|| {
                self.panes
                    .get(&tab.focused())
                    .and_then(|entry| entry.name.as_deref())
            })
        });
        match name {
            Some(name) => format!("{} {name}", index + 1),
            None => (index + 1).to_string(),
        }
    }

    /// Where the shown tab's panes are drawn in `body`. A zoomed tab places
    /// only its focused pane, over all of `body`; otherwise each pane gives
    /// up its left column and top row to the border when one runs there; a
    /// stack's title rows keep their row and give up only the left column.
    pub fn placed(&self, body: Rect) -> Vec<Placed> {
        let Some(tab) = self.active_tab() else {
            return vec![];
        };
        if tab.zoomed {
            return vec![Placed {
                pane: tab.tree.focused(),
                area: body,
                focused: true,
                collapsed: false,
            }];
        }
        tab.tree
            .panes(body)
            .into_iter()
            .map(|info| {
                let left = u16::from(info.rect.x > body.x);
                let top = u16::from(!info.collapsed && info.rect.y > body.y);
                Placed {
                    pane: info.id,
                    area: Rect::new(
                        info.rect.x + left,
                        info.rect.y + top,
                        info.rect.width.saturating_sub(left),
                        info.rect.height.saturating_sub(top),
                    ),
                    focused: info.is_focused,
                    collapsed: info.collapsed,
                }
            })
            .collect()
    }

    fn holds(&self, pane: PaneId) -> bool {
        self.tabs.iter().any(|tab| tab.holds(pane))
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

    /// Forgets where panes no tab holds any more run.
    fn drop_loose_entries(&mut self) {
        let held: HashSet<PaneId> = self
            .tabs
            .iter()
            .flat_map(|tab| tab.tree.pane_ids())
            .collect();
        self.panes.retain(|id, _| held.contains(id));
    }
}

/// Where a pane is drawn: its content area (borders already taken off) and
/// whether it has the layout's focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub pane: PaneId,
    pub area: Rect,
    pub focused: bool,
    /// A stack's one-row title, drawn in place of the pane's screen.
    pub collapsed: bool,
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

/// Every session's tabs and splits. Written by the sessions actor (loading
/// them at start, adding a pane on add, split, new tab or a new session,
/// pointing a moved session's panes at its new directory, dropping a deleted
/// session), the intent handler (focus, resize, zoom, tab moves,
/// renames, closes) and the frontend (closing panes whose program ended, the
/// tab body's size).
#[derive(Debug, Default)]
pub struct Layouts {
    sessions: HashMap<SessionId, SessionLayout>,
    /// The tab body's area in the last frame; focus moves and resizes
    /// measure against it.
    body: Rect,
}

impl Layouts {
    /// Puts `layout` in as `session`'s, replacing the one it had.
    pub fn insert(&mut self, session: SessionId, layout: SessionLayout) {
        self.sessions.insert(session, layout);
    }

    /// Drops `session`'s layout.
    pub fn remove(&mut self, session: SessionId) {
        self.sessions.remove(&session);
    }

    pub fn get(&self, owner: SessionId) -> Option<&SessionLayout> {
        self.sessions.get(&owner)
    }

    /// The session whose layout holds `pane`.
    pub fn owner_of(&self, pane: PaneId) -> Option<SessionId> {
        self.sessions
            .iter()
            .find(|(_, layout)| layout.holds(pane))
            .map(|(owner, _)| *owner)
    }

    /// Where pane `pane` runs.
    pub fn entry(&self, pane: PaneId) -> Option<&PaneEntry> {
        self.sessions
            .values()
            .find_map(|layout| layout.panes.get(&pane))
    }

    /// Every pane of every tab of `owner`'s layout.
    pub fn session_panes(&self, owner: SessionId) -> Vec<PaneId> {
        self.sessions
            .get(&owner)
            .map(|layout| {
                layout
                    .tabs
                    .iter()
                    .flat_map(|tab| tab.tree.pane_ids())
                    .collect()
            })
            .unwrap_or_default()
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

    /// Splits the shown tab's focused pane `split` with `pane`, which takes
    /// the focus, sharing the space evenly along that direction; a tab holding
    /// a stack re-tiles with `pane` added instead. The tab shows every pane
    /// again.
    pub fn split(&mut self, owner: SessionId, split: Split, pane: PaneEntry) {
        let Some(layout) = self.sessions.get_mut(&owner) else {
            return;
        };
        if let Some(tab) = layout.tab_mut() {
            tab.tree.split_focused(split, pane.id);
            tab.zoomed = false;
            layout.panes.insert(pane.id, pane);
        }
    }

    /// Adds `pane` to the shown tab and re-tiles the tab by pane count;
    /// `pane` takes the focus and the tab shows every pane again.
    pub fn add_tiled(&mut self, owner: SessionId, pane: PaneEntry) {
        let Some(layout) = self.sessions.get_mut(&owner) else {
            return;
        };
        if let Some(tab) = layout.tab_mut() {
            tab.tree.add_tiled(pane.id);
            tab.zoomed = false;
            layout.panes.insert(pane.id, pane);
        }
    }

    /// Appends a tab of the one pane `pane` and shows it.
    pub fn new_tab(&mut self, owner: SessionId, pane: PaneEntry) {
        if let Some(layout) = self.sessions.get_mut(&owner) {
            layout.tabs.push(Tab::new(pane.id));
            layout.active = layout.tabs.len() - 1;
            layout.panes.insert(pane.id, pane);
        }
    }

    /// Closes `pane` wherever it is. The tab it leaves is re-tiled by pane
    /// count; a tab left without panes goes, and the tab before it is shown;
    /// a layout left without tabs goes.
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
            let closed = tab.tree.close_pane(pane);
            if closed {
                tab.tree.tile();
            }
            !closed
        });
        if emptied {
            layout.remove_tab(index);
        }
        layout.drop_loose_entries();
        self.drop_if_empty(owner);
    }

    /// Closes the shown tab; the tab before it is shown. A layout left
    /// without tabs goes.
    pub fn close_tab(&mut self, owner: SessionId) {
        if let Some(layout) = self.sessions.get_mut(&owner) {
            layout.remove_tab(layout.active);
            layout.drop_loose_entries();
        }
        self.drop_if_empty(owner);
    }

    /// Moves the focus to the pane `nav` of the focused one. With none to the
    /// left the keys go to the sidebar; with none to the right the next tab
    /// is shown.
    pub fn move_focus(&mut self, owner: SessionId, nav: NavDirection) -> FocusMove {
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
    pub fn focus_pane(&mut self, owner: SessionId, pane: PaneId) {
        if let Some(tab) = self.tab_mut(owner) {
            tab.tree.focus_pane(pane);
        }
    }

    /// Shows the tab of `owner`'s layout holding `pane` and focuses `pane` in
    /// it, expanding it when it's stacked; nothing when no tab holds it. A
    /// zoomed tab stays zoomed.
    pub fn reveal(&mut self, owner: SessionId, pane: PaneId) {
        let Some(layout) = self.sessions.get_mut(&owner) else {
            return;
        };
        if let Some(index) = layout.tabs.iter().position(|tab| tab.holds(pane)) {
            layout.active = index;
            self.focus_pane(owner, pane);
        }
    }

    /// Shows only the focused pane over the shown tab, or every pane again.
    pub fn toggle_zoom(&mut self, owner: SessionId) {
        if let Some(tab) = self.tab_mut(owner) {
            tab.zoomed = !tab.zoomed;
        }
    }

    /// Grows (or shrinks) the focused pane a step; whether it changed.
    pub fn resize_focused(&mut self, owner: SessionId, grow: bool) -> bool {
        let body = self.body;
        self.tab_mut(owner)
            .is_some_and(|tab| tab.tree.resize_focused(grow, STEP, body))
    }

    /// Shows tab `n`, counting from 1; past the last changes nothing.
    pub fn go_to_tab(&mut self, owner: SessionId, n: usize) {
        if let Some(layout) = self.sessions.get_mut(&owner)
            && (1..=layout.tabs.len()).contains(&n)
        {
            layout.active = n - 1;
        }
    }

    /// Shows the next tab, wrapping to the first.
    pub fn next_tab(&mut self, owner: SessionId) {
        if let Some(layout) = self.sessions.get_mut(&owner) {
            layout.active = (layout.active + 1) % layout.tabs.len().max(1);
        }
    }

    /// Shows the previous tab, wrapping to the last.
    pub fn previous_tab(&mut self, owner: SessionId) {
        if let Some(layout) = self.sessions.get_mut(&owner) {
            let count = layout.tabs.len().max(1);
            layout.active = (layout.active + count - 1) % count;
        }
    }

    /// Swaps the shown tab with the one before it; the first stays put.
    pub fn move_tab_left(&mut self, owner: SessionId) {
        if let Some(layout) = self.sessions.get_mut(&owner)
            && layout.active > 0
        {
            layout.tabs.swap(layout.active, layout.active - 1);
            layout.active -= 1;
        }
    }

    /// Swaps the shown tab with the one after it; the last stays put.
    pub fn move_tab_right(&mut self, owner: SessionId) {
        if let Some(layout) = self.sessions.get_mut(&owner)
            && layout.active + 1 < layout.tabs.len()
        {
            layout.tabs.swap(layout.active, layout.active + 1);
            layout.active += 1;
        }
    }

    /// Names pane `pane`, wherever it is; `None` clears the name.
    pub fn rename_pane(&mut self, pane: PaneId, name: Option<String>) {
        if let Some(entry) = self
            .sessions
            .values_mut()
            .find_map(|layout| layout.panes.get_mut(&pane))
        {
            entry.name = name;
        }
    }

    /// Remembers the command that brings pane `pane`'s conversation back in
    /// a fresh shell, wherever the pane is; `None` when it has none.
    pub fn remember_resume(&mut self, pane: PaneId, resume: Option<String>) {
        if let Some(entry) = self
            .sessions
            .values_mut()
            .find_map(|layout| layout.panes.get_mut(&pane))
        {
            entry.resume = resume;
        }
    }

    /// Points every pane of `owner`'s layout at `dir`, where they start next.
    pub fn move_session(&mut self, owner: SessionId, dir: &Path) {
        if let Some(layout) = self.sessions.get_mut(&owner) {
            for entry in layout.panes.values_mut() {
                dir.clone_into(&mut entry.cwd);
            }
        }
    }

    /// Names tab `tab` (0-based) of `owner`'s layout; `None` clears the name.
    pub fn rename_tab(&mut self, owner: SessionId, tab: usize, name: Option<String>) {
        if let Some(tab) = self
            .sessions
            .get_mut(&owner)
            .and_then(|layout| layout.tabs.get_mut(tab))
        {
            tab.name = name;
        }
    }

    fn tab_mut(&mut self, owner: SessionId) -> Option<&mut Tab> {
        self.sessions.get_mut(&owner)?.tab_mut()
    }

    fn drop_if_empty(&mut self, owner: SessionId) {
        if self
            .sessions
            .get(&owner)
            .is_some_and(|layout| layout.tabs.is_empty())
        {
            self.sessions.remove(&owner);
        }
    }
}

/// Pane `id` running in `orb-p<id>` on `/tmp/zmx`, in `/work`, for tests.
#[cfg(test)]
pub(crate) fn test_entry(id: i64) -> PaneEntry {
    PaneEntry {
        id: PaneId(id),
        zmx: ZmxSession {
            name: format!("orb-p{id}"),
            dir: "/tmp/zmx".into(),
        },
        cwd: "/work".into(),
        name: None,
        resume: None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use ratatui::layout::Rect;

    use super::{FocusMove, Layouts, Placed, SessionLayout, Tab, test_entry as entry};
    use crate::feat::layout::tree::{NavDirection, Split, TileLayout};
    use crate::feat::sessions::state::{PaneId, SessionId};

    const OWNER: SessionId = SessionId(1);
    const BODY: Rect = Rect::new(0, 0, 80, 24);

    /// Session 1's layout of pane 7, measured against an 80×24 body.
    fn opened() -> Layouts {
        let mut layouts = Layouts::default();
        layouts.insert(OWNER, SessionLayout::of(entry(7)));
        layouts.fit_to(BODY);
        layouts
    }

    /// Session 1's layout split right once: pane 7 on the left, pane 8 on
    /// the right and focused.
    fn split_right() -> Layouts {
        let mut layouts = opened();
        layouts.split(OWNER, Split::Right, entry(8));
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

    /// Session 1's layout with three tabs (panes 7, 8, 9), the first shown.
    fn three_tabs() -> Layouts {
        let mut layouts = opened();
        layouts.new_tab(OWNER, entry(8));
        layouts.new_tab(OWNER, entry(9));
        layouts.go_to_tab(OWNER, 1);
        layouts
    }

    #[rstest::rstest]
    fn move_session_points_every_pane_at_the_dir() {
        // Given session 1's layout of panes 7 and 8 in /work.
        let mut layouts = split_right();

        // When moving the session to /wt.
        layouts.move_session(OWNER, std::path::Path::new("/wt"));

        // Then both panes start in /wt.
        let cwds: Vec<_> = [7, 8]
            .into_iter()
            .filter_map(|id| layouts.entry(PaneId(id)).map(|entry| entry.cwd.clone()))
            .collect();
        assert_eq!(
            cwds,
            vec![std::path::PathBuf::from("/wt"); 2],
            "every pane should start in the new directory"
        );
    }

    #[rstest::rstest]
    fn layout_of_a_pane_is_one_tab_of_it() {
        // Given no layouts.
        let mut layouts = Layouts::default();

        // When putting in session 1's layout of pane 7.
        layouts.insert(OWNER, SessionLayout::of(entry(7)));

        // Then it has one tab focused on pane 7.
        assert_eq!(
            (tab_count(&layouts), focused(&layouts)),
            (1, Some(PaneId(7))),
            "a new layout is one tab of its pane"
        );
    }

    #[rstest::rstest]
    fn split_takes_the_given_pane_and_focuses_it() {
        // Given a layout of pane 7.
        let mut layouts = opened();

        // When splitting it right with pane 8.
        layouts.split(OWNER, Split::Right, entry(8));

        // Then pane 8 is placed beside it and focused.
        let panes: Vec<(PaneId, bool)> = placed(&layouts)
            .iter()
            .map(|place| (place.pane, place.focused))
            .collect();
        assert_eq!(
            panes,
            vec![(PaneId(7), false), (PaneId(8), true)],
            "the given pane joins the tab with the focus"
        );
    }

    #[rstest::rstest]
    fn new_tab_takes_the_given_pane() {
        // Given a layout of pane 7.
        let mut layouts = opened();

        // When opening a tab of pane 8.
        layouts.new_tab(OWNER, entry(8));

        // Then pane 8's entry is there.
        assert_eq!(
            layouts.entry(PaneId(8)),
            Some(&entry(8)),
            "a new tab's pane says where it runs"
        );
    }

    #[rstest::rstest]
    fn closing_a_pane_drops_its_entry() {
        // Given two panes side by side.
        let mut layouts = split_right();

        // When closing pane 8.
        layouts.close_pane(PaneId(8));

        // Then nothing says where pane 8 runs.
        assert_eq!(layouts.entry(PaneId(8)), None, "a closed pane has no entry");
    }

    #[rstest::rstest]
    fn restored_tab_keeps_its_name_tree_and_focus() {
        // Given a saved tab named "logs" of panes 7 and 8, focused on 7.
        let tree = {
            let mut tree = TileLayout::new(PaneId(7));
            tree.split_focused(Split::Right, PaneId(8));
            tree.focus_pane(PaneId(7));
            tree
        };

        // When restoring it.
        let tab = Tab::restore(Some("logs".into()), tree.clone());

        // Then it has that name, tree and focus.
        assert_eq!(
            (tab.name(), tab.layout_json(), tab.focused()),
            (Some("logs"), tree.to_json(), PaneId(7)),
            "a restored tab is the saved one"
        );
    }

    #[rstest::rstest]
    fn split_clears_the_zoom() {
        // Given a zoomed two-pane tab.
        let mut layouts = split_right();
        layouts.toggle_zoom(OWNER);

        // When splitting again.
        layouts.split(OWNER, Split::Down, entry(9));

        // Then the tab isn't zoomed.
        assert_eq!(
            placed(&layouts).len(),
            3,
            "a split should show every pane again"
        );
    }

    /// Session 1's layout of panes 7 to `last`, each added with `add_tiled`.
    fn tiled(last: i64) -> Layouts {
        let mut layouts = opened();
        for id in 8..=last {
            layouts.add_tiled(OWNER, entry(id));
        }
        layouts
    }

    /// How many placed panes each column holds, left to right.
    fn shape(layouts: &Layouts) -> Vec<usize> {
        placed(layouts)
            .chunk_by(|a, b| a.area.x == b.area.x)
            .map(<[Placed]>::len)
            .collect()
    }

    #[rstest::rstest]
    fn closing_a_pane_re_tiles_the_tab() {
        // Given seven tiled panes, laid out [1][4][2].
        let mut layouts = tiled(13);

        // When closing pane 9.
        layouts.close_pane(PaneId(9));

        // Then the six left are laid out [2][4].
        assert_eq!(shape(&layouts), vec![2, 4], "six panes re-tile to [2][4]");
    }

    /// The stacked pane placed with rows of its own: the first placement
    /// right of the first column that isn't a title.
    fn expanded(layouts: &Layouts) -> Option<PaneId> {
        placed(layouts)
            .into_iter()
            .find(|place| place.area.x > BODY.x && !place.collapsed)
            .map(|place| place.pane)
    }

    fn collapsed_count(layouts: &Layouts) -> usize {
        placed(layouts)
            .iter()
            .filter(|place| place.collapsed)
            .count()
    }

    #[rstest::rstest]
    fn split_in_a_stacked_tab_re_tiles_with_the_new_pane() {
        // Given eleven tiled panes, a main pane and a stack of ten.
        let mut layouts = tiled(17);

        // When splitting the focused pane right with pane 18.
        layouts.split(OWNER, Split::Right, entry(18));

        // Then the tab is a main pane and a stack of eleven, ten of them titles.
        assert_eq!(
            (shape(&layouts), collapsed_count(&layouts)),
            (vec![1, 11], 10),
            "a split in a stacked tab should add the pane and re-tile"
        );
    }

    #[rstest::rstest]
    fn split_in_a_stacked_tab_focuses_the_new_pane() {
        // Given eleven tiled panes with pane 12 focused.
        let mut layouts = tiled(17);
        layouts.focus_pane(OWNER, PaneId(12));

        // When splitting the focused pane right with pane 18.
        layouts.split(OWNER, Split::Right, entry(18));

        // Then pane 18 has the focus.
        assert_eq!(
            focused(&layouts),
            Some(PaneId(18)),
            "the split's new pane should be focused"
        );
    }

    #[rstest::rstest]
    fn move_focus_down_in_a_stack_expands_the_pane_below() {
        // Given eleven tiled panes with the stacked pane 12 focused.
        let mut layouts = tiled(17);
        layouts.focus_pane(OWNER, PaneId(12));

        // When moving the focus down.
        layouts.move_focus(OWNER, NavDirection::Down);

        // Then pane 13 is focused and expanded.
        assert_eq!(
            (focused(&layouts), expanded(&layouts)),
            (Some(PaneId(13)), Some(PaneId(13))),
            "the pane below should take the focus and the rows"
        );
    }

    #[rstest::rstest]
    fn focus_moving_to_the_main_pane_keeps_the_stack_expanded() {
        // Given eleven tiled panes with the stacked pane 12 focused.
        let mut layouts = tiled(17);
        layouts.focus_pane(OWNER, PaneId(12));

        // When moving the focus left to the main pane.
        layouts.move_focus(OWNER, NavDirection::Left);

        // Then pane 12 stays expanded.
        assert_eq!(
            expanded(&layouts),
            Some(PaneId(12)),
            "leaving the stack should keep its expanded pane"
        );
    }

    #[rstest::rstest]
    fn closing_a_pane_keeps_the_stack_expanded() {
        // Given twelve tiled panes, pane 12 expanded and the main pane focused.
        let mut layouts = tiled(18);
        layouts.focus_pane(OWNER, PaneId(12));
        layouts.move_focus(OWNER, NavDirection::Left);

        // When closing the stacked pane 15.
        layouts.close_pane(PaneId(15));

        // Then the re-tiled stack still expands pane 12.
        assert_eq!(
            expanded(&layouts),
            Some(PaneId(12)),
            "re-tiling should keep the expanded pane"
        );
    }

    #[rstest::rstest]
    fn reveal_shows_and_focuses_the_pane_in_its_tab() {
        // Given panes 7 and 8 split in tab 1, pane 8 focused, and tab 2 shown.
        let mut layouts = split_right();
        layouts.new_tab(OWNER, entry(9));

        // When revealing pane 7.
        layouts.reveal(OWNER, PaneId(7));

        // Then tab 1 is shown with pane 7 focused.
        assert_eq!(
            (active(&layouts), focused(&layouts)),
            (Some(0), Some(PaneId(7))),
            "reveal should show the pane's tab and focus it"
        );
    }

    #[rstest::rstest]
    fn reveal_expands_a_stacked_pane() {
        // Given eleven tiled panes with pane 17 expanded in the stack.
        let mut layouts = tiled(17);

        // When revealing the collapsed pane 12.
        layouts.reveal(OWNER, PaneId(12));

        // Then pane 12 is placed with rows of its own.
        assert!(
            placed(&layouts)
                .iter()
                .any(|place| place.pane == PaneId(12) && !place.collapsed),
            "reveal should expand a stacked pane"
        );
    }

    #[rstest::rstest]
    fn placed_stack_titles_are_collapsed_rows_one_high() {
        // Given eleven tiled panes.
        let layouts = tiled(17);

        // When placing them.
        let titles: Vec<u16> = placed(&layouts)
            .iter()
            .filter(|place| place.collapsed)
            .map(|place| place.area.height)
            .collect();

        // Then nine are titles, each one row high.
        assert_eq!(titles, vec![1; 9], "the stack's titles are single rows");
    }

    #[rstest::rstest]
    fn add_tiled_shows_every_pane_of_a_zoomed_tab() {
        // Given a zoomed two-pane tab.
        let mut layouts = split_right();
        layouts.toggle_zoom(OWNER);

        // When adding a tiled pane.
        layouts.add_tiled(OWNER, entry(9));

        // Then the tab isn't zoomed.
        assert_eq!(
            placed(&layouts).len(),
            3,
            "adding a pane should show every pane again"
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
                pane: PaneId(8),
                area: BODY,
                focused: true,
                collapsed: false,
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
        layouts.split(OWNER, Split::Down, entry(8));

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
        layouts.new_tab(OWNER, entry(8));

        // When closing that pane.
        layouts.close_pane(PaneId(8));

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

        // When opening a tab of pane 8.
        layouts.new_tab(OWNER, entry(8));

        // Then the second tab is shown, focused on pane 8.
        assert_eq!(
            (active(&layouts), focused(&layouts)),
            (Some(1), Some(PaneId(8))),
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
    fn pane_ids_lists_every_tabs_panes() {
        // Given a split first tab and a second tab.
        let mut layouts = split_right();
        layouts.new_tab(OWNER, entry(9));

        // When listing the panes.
        let panes = layouts.pane_ids();

        // Then all three are there.
        assert_eq!(
            panes,
            HashSet::from([PaneId(7), PaneId(8), PaneId(9)]),
            "panes of every tab"
        );
    }

    #[rstest::rstest]
    fn owner_of_finds_a_panes_session() {
        // Given session 1's split layout.
        let layouts = split_right();

        // When asking whose pane 8 is.
        let owner = layouts.owner_of(PaneId(8));

        // Then it is session 1's.
        assert_eq!(owner, Some(OWNER), "pane 8 is in session 1's layout");
    }

    #[rstest::rstest]
    fn unnamed_tab_is_labelled_by_its_number() {
        // Given an unnamed first tab.
        let layouts = opened();

        // When labelling it.
        let label = layouts.get(OWNER).map(|layout| layout.tab_label(0));

        // Then the label is its number.
        assert_eq!(label.as_deref(), Some("1"), "unnamed tab label");
    }

    #[rstest::rstest]
    fn named_tab_is_labelled_by_number_and_name() {
        // Given a first tab named "logs".
        let mut layouts = opened();
        layouts.rename_tab(OWNER, 0, Some("logs".into()));

        // When labelling it.
        let label = layouts.get(OWNER).map(|layout| layout.tab_label(0));

        // Then the label is its number and name.
        assert_eq!(label.as_deref(), Some("1 logs"), "named tab label");
    }

    #[rstest::rstest]
    fn unnamed_tab_is_labelled_by_its_focused_panes_name() {
        // Given an unnamed tab whose focused pane is named "server".
        let mut layouts = opened();
        layouts.rename_pane(PaneId(7), Some("server".into()));

        // When labelling it.
        let label = layouts.get(OWNER).map(|layout| layout.tab_label(0));

        // Then the label is its number and the pane's name.
        assert_eq!(
            label.as_deref(),
            Some("1 server"),
            "pane name labels the tab"
        );
    }

    #[rstest::rstest]
    fn remembered_resume_shows_in_the_panes_entry() {
        // Given session 1's layout holding pane 7.
        let mut layouts = opened();

        // When remembering pane 7's resume command.
        layouts.remember_resume(PaneId(7), Some("claude --resume aa".into()));

        // Then pane 7's entry carries it.
        assert_eq!(
            layouts
                .entry(PaneId(7))
                .and_then(|entry| entry.resume.as_deref()),
            Some("claude --resume aa"),
            "resume command in the entry"
        );
    }
}
