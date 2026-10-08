//! Every session's tabs, which tab is shown, where each pane's program runs,
//! and the tab body's size.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ratatui::layout::Rect;

use super::tree::{Grow, NavDirection, PaneInfo, Split, TileLayout, find_in_direction};
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

/// The trees `Cmd +` and `Cmd -` can go back to. It holds only while the
/// tab's tree is still the one the last of them left.
#[derive(Debug, Clone, PartialEq)]
struct ResizeHistory {
    /// The tree the last `Cmd +` or `Cmd -` left.
    after: TileLayout,
    /// The trees from before each `Cmd +`, newest last; `Cmd -` goes back to them.
    before_grow: Vec<TileLayout>,
    /// The trees from before each `Cmd -`, newest last; `Cmd +` goes back to them.
    before_shrink: Vec<TileLayout>,
}

/// One tab: its name, its panes' tree, whether it shows only the focused
/// pane, and what `Cmd +` and `Cmd -` can undo.
#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
    /// Set by renaming the tab; unnamed tabs show only their number.
    name: Option<String>,
    tree: TileLayout,
    zoomed: bool,
    /// Never saved: a restored tab starts without one.
    history: Option<ResizeHistory>,
}

impl Tab {
    fn new(pane: PaneId) -> Self {
        Self::restore(None, TileLayout::new(pane))
    }

    /// A saved tab: its name and tree, not zoomed, with no resize history.
    pub fn restore(name: Option<String>, tree: TileLayout) -> Self {
        Self {
            name,
            tree,
            zoomed: false,
            history: None,
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

    /// Runs `step` with the tab's resize history, emptied first when the tree
    /// is no longer the one the last `Cmd +` or `Cmd -` left, then keeps the
    /// history for the tree `step` leaves. Whether `step` changed the tree.
    fn with_history<F>(&mut self, step: F) -> bool
    where
        F: FnOnce(&mut Self, &mut ResizeHistory) -> bool,
    {
        let mut history = self
            .history
            .take()
            .filter(|history| history.after == self.tree)
            .unwrap_or_else(|| ResizeHistory {
                after: self.tree.clone(),
                before_grow: Vec::new(),
                before_shrink: Vec::new(),
            });
        let changed = step(self, &mut history);
        history.after = self.tree.clone();
        self.history = Some(history);
        changed
    }

    /// Goes back to the tree from before the last `Cmd -`, or grows the
    /// focused pane a step, or stacks its split when the step doesn't fit.
    /// With no split left to grow or stack, the tab zooms unless the pane is
    /// its only one. A zoomed tab doesn't grow, and neither does one with no
    /// body yet. Whether the tree changed.
    fn grow(&mut self, body: Rect) -> bool {
        if self.zoomed || body.is_empty() {
            return false;
        }
        self.with_history(|tab, history| match history.before_shrink.pop() {
            Some(tree) => {
                tab.tree = tree;
                true
            }
            None => {
                let before = tab.tree.clone();
                match tab.tree.grow_focused(STEP, body) {
                    Grow::Grew | Grow::Stacked => {
                        history.before_grow.push(before);
                        true
                    }
                    Grow::Stuck => {
                        tab.zoomed = tab.tree.pane_count() > 1;
                        false
                    }
                }
            }
        })
    }

    /// Leaves a zoom, or goes back to the tree from before the last `Cmd +`,
    /// or shrinks the focused pane a step, its ratio held between 0.1 and 0.9.
    /// Whether the tree changed; leaving a zoom isn't a change.
    fn shrink(&mut self, body: Rect) -> bool {
        if self.zoomed {
            self.zoomed = false;
            return false;
        }
        self.with_history(|tab, history| match history.before_grow.pop() {
            Some(tree) => {
                tab.tree = tree;
                true
            }
            None => {
                let before = tab.tree.clone();
                let shrank = tab.tree.resize_focused(false, STEP, body);
                if shrank {
                    history.before_shrink.push(before);
                }
                shrank
            }
        })
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

    /// Where the shown tab's panes are drawn in `body`: each pane's frame on
    /// its whole tree cell. A zoomed tab places only its focused pane, over
    /// all of `body`. Each stack becomes a list of its panes, one row each,
    /// at the top of its area and at most half its height, and its expanded
    /// pane below it at the stack's full width. `list_width` gives a stack's
    /// list width, bars included, from its panes; the list is centred,
    /// leaving at least two columns on each side for the shown row's `>` (at
    /// least 3 wide, at most the stack's width).
    pub fn placed<F>(&self, body: Rect, list_width: F) -> Placement
    where
        F: Fn(&[PaneId]) -> u16,
    {
        let Some(tab) = self.active_tab() else {
            return Placement::default();
        };
        if tab.zoomed {
            return Placement {
                panes: vec![Placed {
                    pane: tab.tree.focused(),
                    area: body,
                    focused: true,
                }],
                stacks: vec![],
            };
        }
        let stacks = tab.tree.stacks(body);
        let mut panes: Vec<Placed> = tab
            .tree
            .panes(body)
            .into_iter()
            .filter(|info| !stacks.iter().any(|stack| stack.panes.contains(&info.id)))
            .map(|info| Placed {
                pane: info.id,
                area: info.rect,
                focused: info.is_focused,
            })
            .collect();
        let stacks = stacks
            .into_iter()
            .map(|stack| {
                let fits = stack
                    .panes
                    .len()
                    .min(usize::from(stack.area.height / 2))
                    .max(1);
                let rows = u16::try_from(fits).unwrap_or(u16::MAX);
                let area = {
                    let width = list_width(&stack.panes)
                        .min(stack.area.width.saturating_sub(4))
                        .max(3)
                        .min(stack.area.width);
                    let x = stack.area.x + (stack.area.width - width) / 2;
                    Rect::new(x, stack.area.y, width, rows)
                };
                panes.push(Placed {
                    pane: stack.expanded,
                    area: Rect::new(
                        stack.area.x,
                        stack.area.y.saturating_add(rows),
                        stack.area.width,
                        stack.area.height.saturating_sub(rows),
                    ),
                    focused: tab.tree.focused() == stack.expanded,
                });
                let start = if stack.panes.len() > fits {
                    stack
                        .panes
                        .iter()
                        .position(|id| *id == stack.expanded)
                        .unwrap_or_default()
                        .saturating_sub(fits - 1)
                } else {
                    0
                };
                let rows = stack
                    .panes
                    .iter()
                    .skip(start)
                    .zip(inside_bars(area).rows())
                    .map(|(id, row)| (*id, row))
                    .collect();
                StackList {
                    area,
                    rows,
                    shown: stack.expanded,
                    panes: stack.panes,
                }
            })
            .collect();
        Placement { panes, stacks }
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

/// Where the shown tab draws its panes, and a list per stack.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Placement {
    pub panes: Vec<Placed>,
    pub stacks: Vec<StackList>,
}

/// A pane with a frame: the frame's outer rect and whether it has the focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub pane: PaneId,
    pub area: Rect,
    pub focused: bool,
}

impl Placed {
    /// Where the pane's screen goes: `area` less one cell on every side.
    pub fn content(&self) -> Rect {
        inside(self.area)
    }
}

/// A stack's list of panes: its rect (bars included), every stacked pane,
/// one row per pane that fits (between the bars), and the pane shown below
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackList {
    pub area: Rect,
    pub rows: Vec<(PaneId, Rect)>,
    pub shown: PaneId,
    /// Every stacked pane, in stack order: the order the wheel steps through.
    pub panes: Vec<PaneId>,
}

/// A stack list's `area` less its bar column on each side.
fn inside_bars(area: Rect) -> Rect {
    Rect::new(
        area.x.saturating_add(1),
        area.y,
        area.width.saturating_sub(2),
        area.height,
    )
}

/// `area` less one cell on every side.
fn inside(area: Rect) -> Rect {
    Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    )
}

/// The pane a move `nav` into a tab focuses: one on the edge it enters
/// from (the rightmost column moving left, the leftmost moving right). The
/// tab's own focus is kept when it's on that edge; else the topmost
/// pane there that isn't collapsed in a stack.
fn entry_pane(panes: &[PaneInfo], nav: NavDirection) -> Option<PaneId> {
    let edge: Vec<&PaneInfo> = match nav {
        NavDirection::Left => {
            let right = panes.iter().map(|info| info.rect.right()).max()?;
            panes
                .iter()
                .filter(|info| info.rect.right() == right)
                .collect()
        }
        NavDirection::Right => {
            let left = panes.iter().map(|info| info.rect.x).min()?;
            panes.iter().filter(|info| info.rect.x == left).collect()
        }
        NavDirection::Up | NavDirection::Down => return None,
    };
    match edge.iter().find(|info| info.is_focused) {
        Some(info) => Some(info.id),
        None => edge
            .iter()
            .filter(|info| !info.collapsed)
            .min_by_key(|info| info.rect.y)
            .or_else(|| edge.iter().min_by_key(|info| info.rect.y))
            .map(|info| info.id),
    }
}

/// What a focus move did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusMove {
    /// Another pane, in this tab or the one beside it, has the focus.
    Moved,
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

    /// Stacks `pane` with the shown tab's focused pane, joining the focused
    /// pane's stack when it has one; `pane` is shown and takes the focus, and
    /// the tab shows every pane again.
    pub fn stack(&mut self, owner: SessionId, pane: PaneEntry) {
        let Some(layout) = self.sessions.get_mut(&owner) else {
            return;
        };
        if let Some(tab) = layout.tab_mut() {
            tab.tree.stack_focused(pane.id);
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

    /// Moves the shown tab's focused pane to a new tab after the last one and
    /// shows it, as zellij's break pane does; the tab it leaves is re-tiled
    /// by pane count. A tab of one pane stays as it is.
    pub fn break_pane(&mut self, owner: SessionId) {
        let Some(layout) = self.sessions.get_mut(&owner) else {
            return;
        };
        let Some(tab) = layout.tab_mut() else {
            return;
        };
        let pane = tab.tree.focused();
        if tab.tree.pane_ids().len() < 2 || !tab.tree.close_pane(pane) {
            return;
        }
        tab.tree.tile();
        tab.zoomed = false;
        layout.tabs.push(Tab::new(pane));
        layout.active = layout.tabs.len() - 1;
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
    /// left or right, the previous or next tab is shown, wrapping, and the
    /// pane on the edge it's entered from gets the focus. With one tab
    /// nothing changes.
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
        let target = match (neighbour, nav) {
            (Some(id), _) => {
                tab.tree.focus_pane(id);
                return FocusMove::Moved;
            }
            (None, NavDirection::Left) if tab_count > 1 => (active + tab_count - 1) % tab_count,
            (None, NavDirection::Right) if tab_count > 1 => (active + 1) % tab_count,
            (None, _) => return FocusMove::Stuck,
        };
        layout.active = target;
        if let Some(tab) = layout.tab_mut()
            && !tab.zoomed
            && let Some(id) = entry_pane(&tab.tree.panes(body), nav)
        {
            tab.tree.focus_pane(id);
        }
        FocusMove::Moved
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

    /// `Cmd +` (`grow`) or `Cmd -` on the shown tab's focused pane: see
    /// [`Tab::grow`] and [`Tab::shrink`]. Each undoes the other first. Whether
    /// the tree changed; a zoom or unzoom alone isn't a change.
    pub fn resize_focused(&mut self, owner: SessionId, grow: bool) -> bool {
        let body = self.body;
        self.tab_mut(owner).is_some_and(|tab| {
            if grow {
                tab.grow(body)
            } else {
                tab.shrink(body)
            }
        })
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

    use super::{FocusMove, Layouts, Placed, Placement, SessionLayout, Tab, test_entry as entry};
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

    /// The width the tests' stack lists ask for.
    const LIST_WIDTH: u16 = 20;

    fn placement(layouts: &Layouts) -> Placement {
        placement_with(layouts, LIST_WIDTH)
    }

    fn placement_with(layouts: &Layouts, list_width: u16) -> Placement {
        layouts
            .get(OWNER)
            .map(|layout| layout.placed(BODY, |_| list_width))
            .unwrap_or_default()
    }

    fn placed(layouts: &Layouts) -> Vec<Placed> {
        placement(layouts).panes
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

    /// How many panes each column holds, left to right, a stack counting
    /// every pane in it.
    fn shape(layouts: &Layouts) -> Vec<usize> {
        let placement = placement(layouts);
        let mut columns: Vec<(u16, usize)> = Vec::new();
        for place in &placement.panes {
            let count = placement
                .stacks
                .iter()
                .find(|stack| stack.shown == place.pane)
                .map_or(1, |stack| stack.panes.len());
            match columns.last_mut() {
                Some((x, n)) if *x == place.area.x => *n += count,
                _ => columns.push((place.area.x, count)),
            }
        }
        columns.into_iter().map(|(_, n)| n).collect()
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

    #[rstest::rstest]
    #[case(8, vec![1])]
    #[case(9, vec![1, 1])]
    #[case(12, vec![1, 4])]
    #[case(13, vec![2, 4])]
    #[case(14, vec![1, 4, 2])]
    #[case(16, vec![1, 4, 4])]
    #[case(17, vec![2, 4, 4])]
    #[case(18, vec![11])]
    #[case(19, vec![12])]
    fn closing_any_pane_re_tiles_to_the_template_for_one_fewer(
        #[case] last: i64,
        #[case] expected: Vec<usize>,
    ) {
        // Given panes 7 to `last` tiled.
        let tab = || tiled(last);

        // When closing each pane in turn, on its own copy of the tab.
        let shapes: Vec<Vec<usize>> = (7..=last)
            .map(|closed| {
                let mut layouts = tab();
                layouts.close_pane(PaneId(closed));
                shape(&layouts)
            })
            .collect();

        // Then every close leaves the template shape for the new count.
        assert!(
            shapes.iter().all(|shape| *shape == expected),
            "closing any of panes 7..={last} should give {expected:?}, got {shapes:?}"
        );
    }

    /// The stacked pane shown below the stack's list.
    fn expanded(layouts: &Layouts) -> Option<PaneId> {
        placement(layouts)
            .stacks
            .into_iter()
            .next()
            .map(|stack| stack.shown)
    }

    /// How many panes the stack's list names besides the shown one.
    fn listed_others(layouts: &Layouts) -> usize {
        placement(layouts)
            .stacks
            .into_iter()
            .next()
            .map_or(0, |stack| stack.rows.len().saturating_sub(1))
    }

    #[rstest::rstest]
    fn split_in_a_stacked_tab_re_tiles_with_the_new_pane() {
        // Given eleven tiled panes, one stack.
        let mut layouts = tiled(17);

        // When splitting the focused pane right with pane 18.
        layouts.split(OWNER, Split::Right, entry(18));

        // Then the tab is one stack of twelve, eleven listed besides the shown one.
        assert_eq!(
            (shape(&layouts), listed_others(&layouts)),
            (vec![12], 11),
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

        // Then pane 12 is the stack's shown pane.
        assert_eq!(
            expanded(&layouts),
            Some(PaneId(12)),
            "reveal should expand a stacked pane"
        );
    }

    #[rstest::rstest]
    fn stack_list_is_centred_at_the_top_of_the_stack() {
        // Given eleven tiled panes, one stack over the whole body.
        let layouts = tiled(17);

        // When placing them with a 20-column list.
        let list = placement(&layouts)
            .stacks
            .into_iter()
            .next()
            .map(|stack| stack.area);

        // Then the list takes the body's top eleven rows, 20 columns wide and centred.
        assert_eq!(
            list,
            Some(Rect::new(30, 0, 20, 11)),
            "the list is a centred row per pane on top"
        );
    }

    #[rstest::rstest]
    #[case(0, 3)]
    #[case(1, 3)]
    #[case(200, 76)]
    fn stack_list_width_stays_between_three_and_the_stack(
        #[case] list_width: u16,
        #[case] expected: u16,
    ) {
        // Given eleven tiled panes, one stack 80 columns wide.
        let layouts = tiled(17);

        // When placing them with a list `list_width` wide.
        let width = placement_with(&layouts, list_width)
            .stacks
            .into_iter()
            .next()
            .map(|stack| stack.area.width);

        // Then the list is clamped to fit two bars and a column, and the
        // stack with room for the `>` beside it.
        assert_eq!(width, Some(expected), "list width for {list_width}");
    }

    #[rstest::rstest]
    fn stack_list_rows_sit_between_the_bars() {
        // Given eleven tiled panes, one stack over the whole body.
        let layouts = tiled(17);

        // When placing them with a 20-column list.
        let first = placement(&layouts)
            .stacks
            .into_iter()
            .next()
            .and_then(|stack| stack.rows.first().map(|(_, row)| *row));

        // Then the first name's row leaves a column for each bar.
        assert_eq!(
            first,
            Some(Rect::new(31, 0, 18, 1)),
            "names go inside the bars"
        );
    }

    #[rstest::rstest]
    fn stack_shown_pane_fills_the_rows_below_the_list() {
        // Given eleven tiled panes, one stack over the whole body.
        let layouts = tiled(17);

        // When placing them.
        let area = placement(&layouts)
            .panes
            .iter()
            .find(|place| Some(place.pane) == expanded(&layouts))
            .map(|place| place.area);

        // Then the shown pane takes every row under the list.
        assert_eq!(
            area,
            Some(Rect::new(0, 11, 80, 13)),
            "the shown pane is pinned below the list"
        );
    }

    #[rstest::rstest]
    fn stack_list_is_at_most_half_the_stack() {
        // Given fourteen tiled panes in a 24-row body.
        let layouts = tiled(20);

        // When placing them.
        let height = placement(&layouts)
            .stacks
            .into_iter()
            .next()
            .map(|stack| stack.area.height);

        // Then the list is half the body's height.
        assert_eq!(height, Some(12), "the list stops at half the stack");
    }

    #[rstest::rstest]
    fn stack_list_scrolls_to_keep_the_shown_row() {
        // Given eleven tiled panes, the last stacked pane 17 shown.
        let layouts = tiled(17);

        // When placing them in an 8-row body, whose list fits four rows.
        let rows: Vec<PaneId> = layouts
            .get(OWNER)
            .and_then(|layout| {
                layout
                    .placed(Rect::new(0, 0, 80, 8), |_| LIST_WIDTH)
                    .stacks
                    .into_iter()
                    .next()
            })
            .map(|stack| stack.rows.into_iter().map(|(pane, _)| pane).collect())
            .unwrap_or_default();

        // Then the list shows the last four stacked panes, pane 17 among them.
        assert_eq!(
            rows,
            vec![PaneId(14), PaneId(15), PaneId(16), PaneId(17)],
            "the list scrolls to the shown pane"
        );
    }

    /// The panes each tab holds, in tab order.
    fn tab_panes(layouts: &Layouts) -> Vec<Vec<PaneId>> {
        layouts.get(OWNER).map_or_else(Vec::new, |layout| {
            layout
                .tabs()
                .iter()
                .map(|tab| tab.tree.pane_ids())
                .collect()
        })
    }

    #[rstest::rstest]
    fn break_pane_moves_the_focused_pane_to_its_own_tab() {
        // Given pane 7 split right with pane 8, which has the focus.
        let mut layouts = split_right();

        // When breaking the focused pane out.
        layouts.break_pane(OWNER);

        // Then pane 7 keeps the first tab and pane 8 has a second tab.
        assert_eq!(
            tab_panes(&layouts),
            vec![vec![PaneId(7)], vec![PaneId(8)]],
            "the focused pane leaves for a new tab"
        );
    }

    #[rstest::rstest]
    fn break_pane_shows_the_new_tab() {
        // Given pane 7 split right with pane 8, which has the focus.
        let mut layouts = split_right();

        // When breaking the focused pane out.
        layouts.break_pane(OWNER);

        // Then the new tab is shown.
        assert_eq!(
            layouts.get(OWNER).map(SessionLayout::active),
            Some(1),
            "the broken-out pane's tab is shown"
        );
    }

    #[rstest::rstest]
    fn break_pane_on_a_lone_pane_does_nothing() {
        // Given a tab of pane 7 alone.
        let mut layouts = opened();

        // When breaking the focused pane out.
        layouts.break_pane(OWNER);

        // Then there is still one tab of pane 7.
        assert_eq!(
            tab_panes(&layouts),
            vec![vec![PaneId(7)]],
            "a lone pane is already its own tab"
        );
    }

    #[rstest::rstest]
    fn stack_on_a_lone_pane_stacks_the_new_pane_shown() {
        // Given a lone pane 7.
        let mut layouts = opened();

        // When stacking pane 8 with it.
        layouts.stack(OWNER, entry(8));

        // Then panes 7 and 8 are a stack showing pane 8, which has the focus.
        assert_eq!(
            (
                placement(&layouts)
                    .stacks
                    .into_iter()
                    .next()
                    .map(|stack| stack.rows.len()),
                expanded(&layouts),
                focused(&layouts)
            ),
            (Some(2), Some(PaneId(8)), Some(PaneId(8))),
            "a stack of two shows the new pane"
        );
    }

    /// Session 1's layout with two stacks: panes 7 and 10 on the left, panes
    /// 8 and 9 on the right, pane 10 focused.
    fn two_stacks() -> Layouts {
        let mut layouts = split_right();
        layouts.stack(OWNER, entry(9));
        layouts.focus_pane(OWNER, PaneId(7));
        layouts.stack(OWNER, entry(10));
        layouts
    }

    #[rstest::rstest]
    fn stack_outside_every_stack_starts_a_second_list() {
        // Given pane 7 beside a stack of panes 8 and 9, pane 7 focused.
        let mut layouts = split_right();
        layouts.stack(OWNER, entry(9));
        layouts.focus_pane(OWNER, PaneId(7));

        // When stacking pane 10.
        layouts.stack(OWNER, entry(10));

        // Then the tab draws two stack lists.
        assert_eq!(
            placement(&layouts).stacks.len(),
            2,
            "a pane outside every stack starts a second stack with its own list"
        );
    }

    #[rstest::rstest]
    fn each_stack_list_takes_its_own_width() {
        // Given two stacks, the left one holding pane 7.
        let layouts = two_stacks();

        // When placing them with a 10-column list for the left stack and a
        // 20-column one for the right.
        let widths: Vec<u16> = layouts
            .get(OWNER)
            .map(|layout| {
                layout.placed(
                    BODY,
                    |panes| {
                        if panes.contains(&PaneId(7)) { 10 } else { 20 }
                    },
                )
            })
            .unwrap_or_default()
            .stacks
            .iter()
            .map(|stack| stack.area.width)
            .collect();

        // Then each list is as wide as its own stack asked.
        assert_eq!(widths, vec![10, 20], "each stack list has its own width");
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
    #[case::left_from_the_second(2, NavDirection::Left, 0)]
    #[case::left_from_the_first_wraps(1, NavDirection::Left, 2)]
    #[case::right_from_the_first(1, NavDirection::Right, 1)]
    #[case::right_from_the_last_wraps(3, NavDirection::Right, 0)]
    fn move_focus_off_the_tab_edge_shows_the_tab_beside_it(
        #[case] from: usize,
        #[case] nav: NavDirection,
        #[case] shown: usize,
    ) {
        // Given three one-pane tabs, tab `from` shown.
        let mut layouts = three_tabs();
        layouts.go_to_tab(OWNER, from);

        // When moving the focus off the tab's edge.
        let moved = layouts.move_focus(OWNER, nav);

        // Then the tab beside it is shown, wrapping past the ends.
        assert_eq!(
            (moved, active(&layouts)),
            (FocusMove::Moved, Some(shown)),
            "the edge leads to the tab beside it"
        );
    }

    #[rstest::rstest]
    fn move_focus_left_into_a_tab_focuses_its_rightmost_pane() {
        // Given tab 1 with panes 7 and 8 side by side, 7 focused, and tab 2
        // of pane 9 shown.
        let mut layouts = split_right();
        layouts.focus_pane(OWNER, PaneId(7));
        layouts.new_tab(OWNER, entry(9));

        // When moving the focus left.
        layouts.move_focus(OWNER, NavDirection::Left);

        // Then tab 1's right pane has the focus.
        assert_eq!(
            focused(&layouts),
            Some(PaneId(8)),
            "entering from the right focuses the rightmost pane"
        );
    }

    #[rstest::rstest]
    fn move_focus_right_into_a_tab_focuses_its_leftmost_pane() {
        // Given tab 1 of pane 7 and tab 2 with panes 9 and 10 side by side,
        // 10 focused, tab 1 shown.
        let mut layouts = opened();
        layouts.new_tab(OWNER, entry(9));
        layouts.split(OWNER, Split::Right, entry(10));
        layouts.go_to_tab(OWNER, 1);

        // When moving the focus right.
        layouts.move_focus(OWNER, NavDirection::Right);

        // Then tab 2's left pane has the focus.
        assert_eq!(
            focused(&layouts),
            Some(PaneId(9)),
            "entering from the left focuses the leftmost pane"
        );
    }

    #[rstest::rstest]
    fn move_focus_into_a_tab_keeps_its_focus_on_the_entry_edge() {
        // Given tab 1 of pane 7 and tab 2 with pane 9 above pane 10, 10
        // focused, tab 1 shown.
        let mut layouts = opened();
        layouts.new_tab(OWNER, entry(9));
        layouts.split(OWNER, Split::Down, entry(10));
        layouts.go_to_tab(OWNER, 1);

        // When moving the focus right.
        layouts.move_focus(OWNER, NavDirection::Right);

        // Then tab 2 keeps the focus on the lower pane.
        assert_eq!(
            focused(&layouts),
            Some(PaneId(10)),
            "a focused pane on the entry edge keeps the focus"
        );
    }

    #[rstest::rstest]
    #[case::left(NavDirection::Left)]
    #[case::right(NavDirection::Right)]
    fn move_focus_off_the_edge_of_the_only_tab_is_stuck(#[case] nav: NavDirection) {
        // Given a lone pane in the only tab.
        let mut layouts = opened();

        // When moving the focus off its edge.
        let moved = layouts.move_focus(OWNER, nav);

        // Then nothing moves.
        assert_eq!(moved, FocusMove::Stuck, "one tab has nothing beside it");
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
            placement(&layouts),
            Placement {
                panes: vec![Placed {
                    pane: PaneId(8),
                    area: BODY,
                    focused: true,
                }],
                stacks: vec![],
            },
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
    fn split_right_places_each_pane_on_its_whole_tree_cell() {
        // Given two panes side by side over an 80-column body.
        let layouts = split_right();

        // When placing them.
        let areas: Vec<Rect> = placed(&layouts).iter().map(|place| place.area).collect();

        // Then each pane's frame takes its half.
        assert_eq!(
            areas,
            vec![Rect::new(0, 0, 40, 24), Rect::new(40, 0, 40, 24)],
            "the frames touch at column 40"
        );
    }

    #[rstest::rstest]
    fn split_down_places_each_pane_on_its_whole_tree_cell() {
        // Given two stacked panes over a 24-row body.
        let mut layouts = opened();
        layouts.split(OWNER, Split::Down, entry(8));

        // When placing them.
        let areas: Vec<Rect> = placed(&layouts).iter().map(|place| place.area).collect();

        // Then each pane's frame takes its half.
        assert_eq!(
            areas,
            vec![Rect::new(0, 0, 80, 12), Rect::new(0, 12, 80, 12)],
            "the frames touch at row 12"
        );
    }

    #[rstest::rstest]
    fn content_is_the_cell_less_one_cell_on_each_side() {
        // Given a pane placed on a 40×24 cell.
        let place = Placed {
            pane: PaneId(7),
            area: Rect::new(0, 0, 40, 24),
            focused: false,
        };

        // When asking where its screen goes.
        let content = place.content();

        // Then the frame takes one cell on every side.
        assert_eq!(
            content,
            Rect::new(1, 1, 38, 22),
            "the frame is one cell thick"
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

    /// A tall body for the `Cmd +` climb: in 80×24 the first step on a [1][4]
    /// column already stacks.
    const TALL: Rect = Rect::new(0, 0, 80, 40);

    /// Whether the shown tab is zoomed.
    fn zoomed(layouts: &Layouts) -> bool {
        layouts
            .get(OWNER)
            .and_then(SessionLayout::active_tab)
            .is_some_and(Tab::zoomed)
    }

    /// The shown tab's tree as saved.
    fn tree_json(layouts: &Layouts) -> Option<String> {
        layouts
            .get(OWNER)
            .and_then(SessionLayout::active_tab)
            .map(Tab::layout_json)
    }

    /// The shown tab placed over `TALL`.
    fn tall_placement(layouts: &Layouts) -> Placement {
        layouts
            .get(OWNER)
            .map(|layout| layout.placed(TALL, |_| LIST_WIDTH))
            .unwrap_or_default()
    }

    /// Each stack's panes, in tree order.
    fn stacked(layouts: &Layouts) -> Vec<Vec<PaneId>> {
        tall_placement(layouts)
            .stacks
            .into_iter()
            .map(|stack| stack.panes)
            .collect()
    }

    /// A [1][4] tab (pane 7 | panes 8 to 11) over `TALL`, focused on pane 9.
    fn climb() -> Layouts {
        let mut layouts = tiled(11);
        layouts.fit_to(TALL);
        layouts.focus_pane(OWNER, PaneId(9));
        layouts
    }

    /// Presses `Cmd +` until the stacks change, at most 30 times.
    fn grow_until_the_stacks_change(layouts: &mut Layouts) {
        let before = stacked(layouts);
        for _ in 0..30 {
            layouts.resize_focused(OWNER, true);
            if stacked(layouts) != before {
                return;
            }
        }
    }

    #[rstest::rstest]
    fn cmd_plus_in_a_column_first_grows_the_pane() {
        // Given a [1][4] tab focused on pane 9, 10 rows tall.
        let mut layouts = climb();

        // When pressing Cmd + once.
        layouts.resize_focused(OWNER, true);

        // Then pane 9 is 14 rows tall.
        let height = tall_placement(&layouts)
            .panes
            .iter()
            .find(|place| place.pane == PaneId(9))
            .map(|place| place.area.height);
        assert_eq!(height, Some(14), "the first step should grow pane 9 4 rows");
    }

    #[rstest::rstest]
    fn cmd_plus_in_a_column_stacks_part_of_the_column() {
        // Given a [1][4] tab focused on pane 9.
        let mut layouts = climb();

        // When pressing Cmd + until the stacks change.
        grow_until_the_stacks_change(&mut layouts);

        // Then pane 9 and the panes below it are one stack.
        assert_eq!(
            stacked(&layouts),
            vec![vec![PaneId(9), PaneId(10), PaneId(11)]],
            "the first stack should hold pane 9 and the panes below it"
        );
    }

    #[rstest::rstest]
    fn cmd_plus_on_a_stacked_part_stacks_the_whole_column() {
        // Given a [1][4] tab whose panes 9 to 11 were stacked by Cmd +.
        let mut layouts = climb();
        grow_until_the_stacks_change(&mut layouts);

        // When pressing Cmd + until the stacks change again.
        grow_until_the_stacks_change(&mut layouts);

        // Then the whole right column is one stack.
        assert_eq!(
            stacked(&layouts),
            vec![vec![PaneId(8), PaneId(9), PaneId(10), PaneId(11)]],
            "the second stack should hold the whole column"
        );
    }

    #[rstest::rstest]
    fn cmd_plus_on_a_stacked_column_stacks_the_whole_tab() {
        // Given a [1][4] tab whose right column was stacked by Cmd +.
        let mut layouts = climb();
        grow_until_the_stacks_change(&mut layouts);
        grow_until_the_stacks_change(&mut layouts);

        // When pressing Cmd + until the stacks change again.
        grow_until_the_stacks_change(&mut layouts);

        // Then every pane in the tab is one stack.
        assert_eq!(
            stacked(&layouts),
            vec![vec![
                PaneId(7),
                PaneId(8),
                PaneId(9),
                PaneId(10),
                PaneId(11)
            ]],
            "the third stack should hold the whole tab"
        );
    }

    #[rstest::rstest]
    fn cmd_plus_on_a_whole_tab_stack_zooms_it() {
        // Given a [1][4] tab that Cmd + stacked whole.
        let mut layouts = climb();
        grow_until_the_stacks_change(&mut layouts);
        grow_until_the_stacks_change(&mut layouts);
        grow_until_the_stacks_change(&mut layouts);

        // When pressing Cmd + once more.
        layouts.resize_focused(OWNER, true);

        // Then the tab is zoomed.
        assert!(
            zoomed(&layouts),
            "Cmd + on a whole-tab stack should zoom it"
        );
    }

    #[rstest::rstest]
    fn grow_on_a_stacked_tab_zooms_it() {
        // Given a tab that is one stack of panes 7 and 8.
        let mut layouts = opened();
        layouts.stack(OWNER, entry(8));

        // When growing the focused pane.
        layouts.resize_focused(OWNER, true);

        // Then the tab is zoomed.
        assert!(zoomed(&layouts), "growing a one-stack tab should zoom it");
    }

    #[rstest::rstest]
    fn grow_on_a_lone_pane_changes_nothing() {
        // Given a tab of pane 7 alone.
        let mut layouts = opened();

        // When growing it.
        let changed = layouts.resize_focused(OWNER, true);

        // Then neither the tree nor the zoom changed.
        assert_eq!(
            (changed, zoomed(&layouts)),
            (false, false),
            "a lone pane has nothing to grow into and nothing to zoom"
        );
    }

    #[rstest::rstest]
    fn grow_on_a_zoomed_tab_keeps_the_tree() {
        // Given panes 7 and 8 side by side, zoomed.
        let mut layouts = split_right();
        layouts.toggle_zoom(OWNER);
        let before = tree_json(&layouts);

        // When growing the focused pane.
        layouts.resize_focused(OWNER, true);

        // Then the tree is as it was.
        assert_eq!(
            tree_json(&layouts),
            before,
            "growing a zoomed tab should leave its tree alone"
        );
    }

    /// A 14-column body: panes 7 and 8 side by side get 7 columns each, so a
    /// `Cmd +` on pane 8 would leave pane 7 3 columns wide and stacks them instead.
    const NARROW: Rect = Rect::new(0, 0, 14, 24);

    /// `split_right()` over `NARROW`.
    fn narrow() -> Layouts {
        let mut layouts = split_right();
        layouts.fit_to(NARROW);
        layouts
    }

    /// Pane `pane`'s width when placed over `BODY`.
    fn width(layouts: &Layouts, pane: i64) -> Option<u16> {
        placed(layouts)
            .into_iter()
            .find(|place| place.pane == PaneId(pane))
            .map(|place| place.area.width)
    }

    #[rstest::rstest]
    fn cmd_minus_on_a_zoomed_tab_only_unzooms_it() {
        // Given panes 7 and 8 side by side, zoomed.
        let mut layouts = split_right();
        layouts.toggle_zoom(OWNER);
        let before = tree_json(&layouts);

        // When pressing Cmd -.
        layouts.resize_focused(OWNER, false);

        // Then the tab is unzoomed and its tree is as it was.
        assert_eq!(
            (zoomed(&layouts), tree_json(&layouts)),
            (false, before),
            "Cmd - on a zoomed tab should only leave the zoom"
        );
    }

    #[rstest::rstest]
    fn cmd_minus_after_a_stacking_cmd_plus_restores_the_tree() {
        // Given panes 7 and 8 over a narrow body, stacked by one Cmd +.
        let mut layouts = narrow();
        let before = tree_json(&layouts);
        layouts.resize_focused(OWNER, true);

        // When pressing Cmd -.
        layouts.resize_focused(OWNER, false);

        // Then the tree is the one from before the Cmd +.
        assert_eq!(
            tree_json(&layouts),
            before,
            "Cmd - should undo the Cmd + that stacked the split"
        );
    }

    #[rstest::rstest]
    fn cmd_plus_after_a_cmd_minus_restores_the_tree() {
        // Given panes 7 and 8 over a narrow body, shrunk once, then again
        // up to the ratio bound.
        let mut layouts = narrow();
        layouts.resize_focused(OWNER, false);
        let before = tree_json(&layouts);
        layouts.resize_focused(OWNER, false);

        // When pressing Cmd +.
        layouts.resize_focused(OWNER, true);

        // Then the tree is the one from before the second Cmd -.
        assert_eq!(
            tree_json(&layouts),
            before,
            "Cmd + should undo the last Cmd -"
        );
    }

    #[rstest::rstest]
    fn cmd_minus_replays_the_climb_back_to_the_original_tree() {
        // Given a [1][4] tab that Cmd + climbed until it zoomed.
        let mut layouts = climb();
        let original = tree_json(&layouts);
        let mut changes = 0;
        for _ in 0..30 {
            if zoomed(&layouts) {
                break;
            }
            if layouts.resize_focused(OWNER, true) {
                changes += 1;
            }
        }

        // When pressing Cmd - once per change, plus once for the zoom.
        for _ in 0..=changes {
            layouts.resize_focused(OWNER, false);
        }

        // Then the tab is unzoomed with its original tree.
        assert_eq!(
            (zoomed(&layouts), tree_json(&layouts)),
            (false, original),
            "Cmd - should replay every Cmd + back to the original tree"
        );
    }

    #[rstest::rstest]
    fn cmd_minus_after_a_focus_move_shrinks_the_newly_focused_pane() {
        // Given pane 8 grown once, leaving pane 7 36 columns wide, and the
        // focus moved to pane 7.
        let mut layouts = split_right();
        layouts.resize_focused(OWNER, true);
        layouts.focus_pane(OWNER, PaneId(7));

        // When pressing Cmd -.
        layouts.resize_focused(OWNER, false);

        // Then pane 7 shrank a step instead of the grow being undone.
        assert_eq!(
            width(&layouts, 7),
            Some(32),
            "after a focus move Cmd - should shrink the focused pane"
        );
    }

    #[rstest::rstest]
    fn cmd_minus_after_an_added_pane_keeps_it() {
        // Given pane 8 grown once, then pane 9 added.
        let mut layouts = split_right();
        layouts.resize_focused(OWNER, true);
        layouts.add_tiled(OWNER, entry(9));

        // When pressing Cmd -.
        layouts.resize_focused(OWNER, false);

        // Then pane 9 is still placed.
        assert!(
            placed(&layouts).iter().any(|place| place.pane == PaneId(9)),
            "Cmd - after an add should not bring back the tree without pane 9"
        );
    }

    #[rstest::rstest]
    fn cmd_minus_without_history_shrinks_the_pane_a_step() {
        // Given panes 7 and 8 side by side, pane 8 40 columns wide.
        let mut layouts = split_right();

        // When pressing Cmd -.
        layouts.resize_focused(OWNER, false);

        // Then pane 8 is 4 columns narrower.
        assert_eq!(
            width(&layouts, 8),
            Some(36),
            "Cmd - with no history should shrink the focused pane a step"
        );
    }
}
