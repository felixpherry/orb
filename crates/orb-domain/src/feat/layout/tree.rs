// Adapted from herdr (https://github.com/herdrdev/herdr), src/layout.rs at
// commit 3d9d2b18, licensed under the Apache License, Version 2.0. Modified
// for orb: pane ids come from the caller instead of a global counter; splits
// are `Split::{Right, Down}`; resizing grows, stacks and zooms panes; the
// tree saves to and loads from JSON; herdr's borders, scrollbar lanes, swap,
// insert and drag-resize code is left out.

//! A tab's panes as a tree of right and down splits and stacks of panes
//! sharing one area, any number of them: where each pane
//! lands in an area, and which pane is the neighbour in a direction.

use std::cmp::{Ordering, Reverse};

use error_stack::{Report, ResultExt};
use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};
use wherror::Error;

use crate::feat::sessions::state::PaneId;

/// The fewest rows a pane outside a stack keeps after a `Cmd +` step, its
/// frame included. A stack keeps twice this, since its list takes up to
/// half its rows.
const MIN_PANE_ROWS: u16 = 5;
/// The fewest columns any pane or stack keeps after a `Cmd +` step, its
/// frame included.
const MIN_PANE_COLS: u16 = 5;

/// The `Cmd +`/`Cmd -` step, in percent of the tab, when the stacked one has
/// nowhere to go, and the least share of the tab a moved pane keeps.
const RESIZE_PERCENT: f32 = 5.0;
/// The share of the tab `Cmd +`/`Cmd -` try first.
const STACKED_RESIZE_PERCENT: f32 = 30.0;

/// `percent` of `extent` cells, rounded, at least 1.
fn cells(extent: u16, percent: f32) -> u16 {
    ((f32::from(extent) * percent / 100.0).round() as u16).max(1)
}

/// Which way a split puts its second pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Split {
    /// Side by side; the new pane on the right.
    Right,
    /// Stacked; the new pane below.
    Down,
}

/// Cardinal direction for focus moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavDirection {
    Left,
    Right,
    Up,
    Down,
}

impl NavDirection {
    /// The direction facing the other way.
    fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
            Self::Up => Self::Down,
            Self::Down => Self::Up,
        }
    }
}

/// Where a pane lands in an area, and whether it has the focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneInfo {
    pub id: PaneId,
    pub rect: Rect,
    pub is_focused: bool,
    /// A stack's one-row title: the pane is in the stack but not expanded.
    pub collapsed: bool,
}

/// A stack in an area: the rect it takes, its panes in order and the one
/// expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackInfo {
    pub area: Rect,
    pub panes: Vec<PaneId>,
    pub expanded: PaneId,
}

/// What a [`TileLayout::grow_focused`] step did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grow {
    /// The focused pane's border moved.
    Grew,
    /// The focused pane's parent split became one stack showing it.
    Stacked,
    /// No split holds the focused pane or its stack: a lone pane, or a tree
    /// that is one stack.
    Stuck,
}

/// A tab's place in the list of swap layouts, in list order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapLayout {
    /// The tab's starting shape: one pane.
    Base,
    /// Columns of panes (orb's grid).
    Vertical,
    /// Rows of panes: vertical turned on its side.
    Horizontal,
    /// One stack of every pane.
    Stacked,
    /// The first pane on the left half, the rest stacked on the right.
    HalfStacked,
}

impl SwapLayout {
    const ALL: [Self; 5] = [
        Self::Base,
        Self::Vertical,
        Self::Horizontal,
        Self::Stacked,
        Self::HalfStacked,
    ];

    /// Whether the layout has a template for `n` panes.
    fn fits(self, n: usize) -> bool {
        match self {
            Self::Base => n == 1,
            Self::Vertical | Self::Horizontal => (1..=10).contains(&n),
            Self::Stacked => n >= 2,
            Self::HalfStacked => n >= 3,
        }
    }

    /// The first layout that fits `n` panes, starting at this one (at the
    /// one after it when `step`), going backwards when `back`, wrapping;
    /// `None` when none fits after a full turn.
    pub fn fitting(self, n: usize, step: bool, back: bool) -> Option<Self> {
        let len = Self::ALL.len();
        let at = self as usize;
        (usize::from(step)..usize::from(step) + len)
            .map(|offset| {
                if back {
                    (at + 2 * len - offset) % len
                } else {
                    (at + offset) % len
                }
            })
            .filter_map(|index| Self::ALL.get(index).copied())
            .find(|layout| layout.fits(n))
    }
}

/// A node of the tree: one pane, a split holding two subtrees, or a stack
/// of panes sharing one area.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Pane(PaneId),
    Split {
        split: Split,
        /// The share of the area the first subtree takes.
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
    /// Panes in one area: `expanded` gets the rows, every other pane a
    /// one-row title. Holds at least two panes, `expanded` among them.
    Stack {
        panes: Vec<PaneId>,
        expanded: PaneId,
    },
}

/// A tab's tiling: the tree of splits and the focused pane.
#[derive(Debug, Clone, PartialEq)]
pub struct TileLayout {
    root: Node,
    focus: PaneId,
    /// Pane focused before `focus`, where `close_focused` returns to. Only a
    /// real focus move writes it.
    prev_focus: Option<PaneId>,
}

/// A saved tree couldn't be read.
#[derive(Debug, Error)]
#[error(debug)]
pub struct LayoutJsonError;

impl TileLayout {
    /// A layout of the one pane `root`, focused.
    pub fn new(root: PaneId) -> Self {
        Self::from_saved(Node::Pane(root), root)
    }

    /// A layout of the tree `root`, focused on `focus`, which is expanded
    /// if it's stacked.
    pub fn from_saved(mut root: Node, focus: PaneId) -> Self {
        expand(&mut root, focus);
        Self {
            root,
            focus,
            prev_focus: None,
        }
    }

    /// Move focus, recording the pane being left, and expand it if it's
    /// stacked. Focus and its history stay when focus is unchanged.
    fn set_focus(&mut self, id: PaneId) {
        expand(&mut self.root, id);
        if id != self.focus {
            self.prev_focus = Some(self.focus);
            self.focus = id;
        }
    }

    pub fn focused(&self) -> PaneId {
        self.focus
    }

    pub fn pane_count(&self) -> usize {
        count_panes(&self.root)
    }

    /// Where each pane lands in `area`, in tree order.
    pub fn panes(&self, area: Rect) -> Vec<PaneInfo> {
        let mut result = Vec::new();
        collect_panes(&self.root, area, self.focus, &mut result);
        result
    }

    /// Every stack in the tree and the area it takes in `area`, in tree order.
    pub fn stacks(&self, area: Rect) -> Vec<StackInfo> {
        let mut stacks = Vec::new();
        stacks_in(&self.root, area, &mut stacks);
        stacks
    }

    /// Every pane id, in tree order.
    pub fn pane_ids(&self) -> Vec<PaneId> {
        let mut ids = Vec::new();
        collect_ids(&self.root, &mut ids);
        ids
    }

    /// Splits the focused pane, or its whole stack when it's stacked, `split`:
    /// the new pane `id` goes right after it and takes the focus, and the run
    /// of `split` splits the two join is re-divided into equal shares.
    pub fn split_focused(&mut self, split: Split, id: PaneId) {
        if let Some(node) = find_holding_mut(&mut self.root, self.focus) {
            let held = std::mem::replace(node, Node::Pane(id));
            *node = split_node(held, split, id, 0.5);
            even_run(&mut self.root, split, id);
            self.set_focus(id);
        }
    }

    /// Stacks the new pane `id` right after the focused pane and focuses it:
    /// in the focused pane's stack when it has one, else in a new stack of
    /// the two.
    pub fn stack_focused(&mut self, id: PaneId) {
        let focus = self.focus;
        match find_stack_holding_mut(&mut self.root, focus) {
            Some(panes) => {
                let at = panes
                    .iter()
                    .position(|pane| *pane == focus)
                    .map_or(panes.len(), |index| index + 1);
                panes.insert(at, id);
            }
            None => {
                if let Some(node) = find_holding_mut(&mut self.root, focus) {
                    *node = Node::Stack {
                        panes: vec![focus, id],
                        expanded: id,
                    };
                }
            }
        }
        self.set_focus(id);
    }

    /// Adds pane `id`, focused, where a new pane goes in a tab changed by
    /// hand: into the focused pane's stack when it has one; else it halves
    /// the focused pane's cell in `area`, top and bottom when it's tall (rows
    /// × [`CELL_RATIO`] over its columns, and over 20 rows), left and right
    /// when it's over 60 columns; else the two are stacked. No other pane
    /// moves.
    pub fn add_beside_focus(&mut self, id: PaneId, area: Rect) {
        let focus = self.focus;
        let stacked = self
            .stacks(area)
            .iter()
            .any(|stack| stack.panes.contains(&focus));
        let split = self
            .panes(area)
            .into_iter()
            .find(|info| info.id == focus && !stacked)
            .and_then(|info| {
                let (rows, cols) = (info.rect.height, info.rect.width);
                if rows.saturating_mul(CELL_RATIO) > cols && rows > 20 {
                    Some(Split::Down)
                } else if cols > 60 {
                    Some(Split::Right)
                } else {
                    None
                }
            });
        match split.zip(find_holding_mut(&mut self.root, focus)) {
            Some((split, node)) => {
                *node = split_node(Node::Pane(focus), split, id, 0.5);
                self.set_focus(id);
            }
            None => self.stack_focused(id),
        }
    }

    /// Rebuilds the tree from `layout`'s template, panes in their current
    /// tree order. Focus, its history and each stack's expanded pane stay.
    pub fn tile(&mut self, layout: SwapLayout) {
        let prefer: Vec<PaneId> = std::iter::once(self.focus)
            .chain(
                self.stacks(Rect::default())
                    .into_iter()
                    .map(|stack| stack.expanded),
            )
            .collect();
        if let Some(root) = template_for(layout, &self.pane_ids(), &prefer) {
            self.root = root;
        }
    }

    /// Adds pane `id` last in reading order, re-tiles in `layout`, and
    /// focuses it.
    pub fn add_tiled(&mut self, id: PaneId, layout: SwapLayout) {
        let ids = {
            let mut ids = self.pane_ids();
            ids.push(id);
            ids
        };
        if let Some(root) = template_for(layout, &ids, &[id]) {
            self.root = root;
            self.set_focus(id);
        }
    }

    /// Close the focused pane, returning focus to the pane it came from when
    /// that pane is still open. Returns false if it's the last pane.
    pub fn close_focused(&mut self) -> bool {
        if self.pane_count() <= 1 {
            return false;
        }
        let target = self.focus;
        let ids = self.pane_ids();
        let Some(pos) = ids.iter().position(|id| *id == target) else {
            return false;
        };
        let ordered = ids
            .get(pos + 1)
            .or_else(|| ids.get(pos.wrapping_sub(1)))
            .copied()
            .unwrap_or(target);
        let new_focus = match self.prev_focus {
            Some(prev) if prev != target && ids.contains(&prev) => prev,
            _ => ordered,
        };
        let old = std::mem::replace(&mut self.root, Node::Pane(PaneId(0)));
        match remove_pane(old, target) {
            Some(new_root) => {
                self.root = new_root;
                self.focus = new_focus;
                expand(&mut self.root, new_focus);
                self.prev_focus = None;
                true
            }
            None => false,
        }
    }

    /// Close any pane. Focus and its history are left alone unless the closed
    /// pane is the focused one. Returns false for the last pane or one not here.
    pub fn close_pane(&mut self, id: PaneId) -> bool {
        if self.focus == id {
            return self.close_focused();
        }
        if self.pane_count() <= 1 || !self.pane_ids().contains(&id) {
            return false;
        }
        let old = std::mem::replace(&mut self.root, Node::Pane(PaneId(0)));
        let Some(new_root) = remove_pane(old, id) else {
            return false;
        };
        self.root = new_root;
        if self.prev_focus == Some(id) {
            self.prev_focus = None;
        }
        true
    }

    /// Focus `id` when it is one of this layout's panes.
    pub fn focus_pane(&mut self, id: PaneId) {
        if self.pane_ids().contains(&id) {
            self.set_focus(id);
        }
    }

    /// `Cmd +`: towards the first neighbour above, below,
    /// left or right that lines up with the focused pane (or its stack) along
    /// its whole edge, grows it [`STACKED_RESIZE_PERCENT`] of the tab, or
    /// when that leaves a pane there too small, stacks it with that
    /// neighbour. With no such neighbour it grows [`RESIZE_PERCENT`] any way
    /// that fits; `Stuck` when nothing fits.
    pub fn grow_focused(&mut self, body: Rect) -> Grow {
        for dir in [
            NavDirection::Up,
            NavDirection::Down,
            NavDirection::Left,
            NavDirection::Right,
        ] {
            if !self.lined_up(body, dir) {
                continue;
            }
            if self.resize_toward(body, dir, true, STACKED_RESIZE_PERCENT) {
                return Grow::Grew;
            }
            if self.stack_toward(body, dir) {
                return Grow::Stacked;
            }
        }
        if self.resize_any(body, true, RESIZE_PERCENT) {
            Grow::Grew
        } else {
            Grow::Stuck
        }
    }

    /// `Cmd -`: breaks one pane out of the focused pane's
    /// stack, else shrinks the focused pane [`STACKED_RESIZE_PERCENT`] of the
    /// tab, else [`RESIZE_PERCENT`], any way that fits. Whether it changed.
    pub fn shrink_focused(&mut self, body: Rect) -> bool {
        self.unstack_focused()
            || self.resize_any(body, false, STACKED_RESIZE_PERCENT)
            || self.resize_any(body, false, RESIZE_PERCENT)
    }

    /// Resize mode's step: moves the focused unit's (pane's or stack's) edge
    /// facing `dir` [`RESIZE_PERCENT`] of the tab, out when `grow`, in
    /// otherwise. A grow that can't go that way (the tab's edge, or a
    /// neighbour that would get too small) pulls the opposite edge in
    /// instead; a shrink never falls back. Whether it changed.
    pub fn resize_side(&mut self, body: Rect, dir: NavDirection, grow: bool) -> bool {
        self.resize_toward(body, dir, grow, RESIZE_PERCENT)
            || (grow && self.resize_toward(body, dir.opposite(), false, RESIZE_PERCENT))
    }

    /// Move mode's step: swaps the focused pane with the pane `dir` of it,
    /// the one a directional focus move would reach. Whether it moved.
    pub fn swap_toward(&mut self, body: Rect, dir: NavDirection) -> bool {
        let panes = self.panes(body);
        let target = panes
            .iter()
            .find(|info| info.is_focused)
            .and_then(|focused| find_in_direction(focused, dir, &panes));
        target.is_some_and(|target| self.swap_with(target))
    }

    /// Move mode's step in tree order: swaps the focused pane with the next
    /// (`forward`) or previous pane, wrapping. Whether it moved; a lone pane
    /// doesn't.
    pub fn swap_in_order(&mut self, forward: bool) -> bool {
        let ids = self.pane_ids();
        let target = ids.iter().position(|id| *id == self.focus).and_then(|at| {
            let n = ids.len();
            let step = if forward { 1 } else { n - 1 };
            let next = (at + step) % n;
            ids.get(next).copied()
        });
        target.is_some_and(|target| self.swap_with(target))
    }

    /// Swaps the focused pane and `target` wherever they sit (a pane, a
    /// stack entry, a stack's shown pane); the focus stays on the moved
    /// pane, shown if it landed in a stack. The tree's shape doesn't change.
    fn swap_with(&mut self, target: PaneId) -> bool {
        if target == self.focus {
            return false;
        }
        swap_ids(&mut self.root, self.focus, target);
        self.set_focus(self.focus);
        true
    }

    /// A resize with no direction: the first of the corners (right
    /// and down, left and down, right and up, left and up), then the sides
    /// (right, down, left, up) where both moves fit.
    fn resize_any(&mut self, body: Rect, grow: bool, percent: f32) -> bool {
        use NavDirection::{Down, Left, Right, Up};
        let order = [
            (Right, Some(Down)),
            (Left, Some(Down)),
            (Right, Some(Up)),
            (Left, Some(Up)),
            (Right, None),
            (Down, None),
            (Left, None),
            (Up, None),
        ];
        for (main, sub) in order {
            let mut layout = self.clone();
            if layout.resize_toward(body, main, grow, percent)
                && sub.is_none_or(|sub| layout.resize_toward(body, sub, grow, percent))
            {
                *self = layout;
                return true;
            }
        }
        false
    }

    /// Moves the focused unit's (pane's or stack's) edge facing `dir` by
    /// `percent` of the tab that way: out when `grow`, in otherwise, with
    /// every pane along the same border. Nothing changes, and `false`, when
    /// the edge is the tab's or a moved pane would end under
    /// [`MIN_PANE_COLS`] × [`MIN_PANE_ROWS`] (a stack twice the rows) or
    /// under [`RESIZE_PERCENT`] of the tab.
    fn resize_toward(&mut self, body: Rect, dir: NavDirection, grow: bool, percent: f32) -> bool {
        let Some(edge) = edge(&self.root, body, self.focus, dir) else {
            return false;
        };
        let (region_start, region_len) = along(edge.region, edge.split);
        let cells = cells(along(body, edge.split).1, percent);
        let back = matches!(dir, NavDirection::Up | NavDirection::Left) == grow;
        let to = if back {
            edge.border.checked_sub(cells)
        } else {
            edge.border.checked_add(cells)
        };
        let Some(to) = to.filter(|to| (region_start + 1..region_start + region_len).contains(to))
        else {
            return false;
        };
        let before = leaves_of(&self.root, body, self.focus);
        let Some(after) = shifted(&before, &edge, to) else {
            return false;
        };
        let fits = after
            .iter()
            .zip(&before)
            .all(|(a, b)| a.rect == b.rect || a.roomy(body));
        if fits {
            fit(&mut self.root, &after);
        }
        fits
    }

    /// Whether the focused unit has direct neighbours towards
    /// `dir`: panes or stacks touching its edge that lie within its sides
    /// and together cover the whole edge.
    fn lined_up(&self, body: Rect, dir: NavDirection) -> bool {
        let leaves = leaves_of(&self.root, body, self.focus);
        let Some(unit) = leaves.iter().find(|leaf| leaf.focused) else {
            return false;
        };
        let split = axis(dir);
        let (start, len) = along(unit.rect, split);
        let (side, width) = across(unit.rect, split);
        let touching: Vec<(u16, u16)> = leaves
            .iter()
            .filter(|leaf| {
                let (n_start, n_len) = along(leaf.rect, split);
                let touches = match dir {
                    NavDirection::Up | NavDirection::Left => n_start + n_len == start,
                    NavDirection::Down | NavDirection::Right => n_start == start + len,
                };
                let (n_side, n_width) = across(leaf.rect, split);
                touches && ranges_overlap(side, width, n_side, n_width)
            })
            .map(|leaf| across(leaf.rect, split))
            .filter(|(n_side, n_width)| *n_side >= side && n_side + n_width <= side + width)
            .collect();
        touching.iter().any(|(n_side, _)| *n_side == side)
            && touching
                .iter()
                .any(|(n_side, n_width)| n_side + n_width == side + width)
    }

    /// Makes the focused unit and its neighbours
    /// across the edge facing `dir` one stack showing the focused pane, over
    /// both their areas. Only when the unit spans the whole border; `false`
    /// otherwise.
    // ponytail: a unit narrower than its border doesn't stack, since the tree can't hold a stack
    // across two subtrees; restructure the tree if aligned-neighbour stacks matter
    fn stack_toward(&mut self, body: Rect, dir: NavDirection) -> bool {
        let Some(edge) = edge(&self.root, body, self.focus, dir) else {
            return false;
        };
        let before = leaves_of(&self.root, body, self.focus);
        let Some(unit) = before.iter().find(|leaf| leaf.focused).copied() else {
            return false;
        };
        if across(unit.rect, edge.split) != across(edge.region, edge.split) {
            return false;
        }
        let unit_first = matches!(dir, NavDirection::Down | NavDirection::Right);
        let root = std::mem::replace(&mut self.root, Node::Pane(self.focus));
        let mut stacked = Vec::new();
        self.root = stack_at(root, body, &edge, unit_first, self.focus, &mut stacked);
        let rect = before
            .iter()
            .filter(|leaf| stacked.contains(&leaf.key))
            .fold(unit.rect, |rect, leaf| rect.union(leaf.rect));
        let mut after: Vec<Leaf> = before
            .into_iter()
            .filter(|leaf| !stacked.contains(&leaf.key))
            .collect();
        let key = match find_holding_mut(&mut self.root, self.focus) {
            Some(Node::Stack { panes, .. }) => panes.first().copied(),
            _ => None,
        };
        after.extend(key.map(|key| Leaf {
            key,
            rect,
            stack: true,
            focused: true,
        }));
        fit(&mut self.root, &after);
        true
    }

    /// The focused pane's stack gives its
    /// top pane (when the shown pane is its last) or its bottom pane a
    /// 1/n share of its rows, above or below it. `false` outside a stack.
    fn unstack_focused(&mut self) -> bool {
        let Some(node) = find_holding_mut(&mut self.root, self.focus) else {
            return false;
        };
        let Node::Stack { panes, expanded } = node else {
            return false;
        };
        let (mut panes, expanded) = (std::mem::take(panes), *expanded);
        let count = panes.len() as f32;
        let up = panes.last() == Some(&expanded);
        let out = if up {
            panes.remove(0)
        } else {
            panes.pop().unwrap_or(expanded)
        };
        let Some(rest) = stack(panes, expanded) else {
            return false;
        };
        let out = Box::new(Node::Pane(out));
        let rest = Box::new(rest);
        *node = if up {
            Node::Split {
                split: Split::Down,
                ratio: 1.0 / count,
                first: out,
                second: rest,
            }
        } else {
            Node::Split {
                split: Split::Down,
                ratio: (count - 1.0) / count,
                first: rest,
                second: out,
            }
        };
        true
    }

    /// The tree as JSON: `{"pane":7}` or
    /// `{"split":"right","ratio":0.5,"first":…,"second":…}` or
    /// `{"stack":[3,4,5],"expanded":4}`.
    pub fn to_json(&self) -> String {
        serde_json::to_string(&Saved::from(&self.root)).unwrap_or_default()
    }

    /// A layout from [`Self::to_json`]'s output, focused on `focus`, or on
    /// its first pane when `focus` isn't in it.
    ///
    /// # Errors
    ///
    /// Returns an error if `json` isn't a saved tree.
    pub fn from_json(json: &str, focus: PaneId) -> Result<Self, Report<LayoutJsonError>> {
        let saved: Saved = serde_json::from_str(json)
            .change_context(LayoutJsonError)
            .attach("failed to read a saved layout tree")?;
        let root = load(saved)
            .ok_or_else(|| Report::new(LayoutJsonError))
            .attach("the saved layout tree holds no panes")?;
        let focus = {
            let mut ids = Vec::new();
            collect_ids(&root, &mut ids);
            if ids.contains(&focus) {
                focus
            } else {
                ids.first().copied().unwrap_or(focus)
            }
        };
        Ok(Self::from_saved(root, focus))
    }
}

/// The JSON shape of a [`Node`].
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Saved {
    Pane {
        pane: PaneId,
    },
    Split {
        split: Split,
        ratio: f32,
        first: Box<Saved>,
        second: Box<Saved>,
    },
    Stack {
        stack: Vec<PaneId>,
        expanded: PaneId,
    },
}

impl From<&Node> for Saved {
    fn from(node: &Node) -> Self {
        match node {
            Node::Pane(pane) => Self::Pane { pane: *pane },
            Node::Split {
                split,
                ratio,
                first,
                second,
            } => Self::Split {
                split: *split,
                ratio: *ratio,
                first: Box::new(Self::from(first.as_ref())),
                second: Box::new(Self::from(second.as_ref())),
            },
            Node::Stack { panes, expanded } => Self::Stack {
                stack: panes.clone(),
                expanded: *expanded,
            },
        }
    }
}

/// The tree `saved` describes, ratios clamped. A stack whose `expanded`
/// isn't in it expands its first pane, a one-pane stack is that pane, and
/// an empty stack is dropped (its split's other side takes its place).
fn load(saved: Saved) -> Option<Node> {
    match saved {
        Saved::Pane { pane } => Some(Node::Pane(pane)),
        Saved::Split {
            split,
            ratio,
            first,
            second,
        } => match (load(*first), load(*second)) {
            (Some(first), Some(second)) => Some(Node::Split {
                split,
                ratio: valid_split_ratio(ratio),
                first: Box::new(first),
                second: Box::new(second),
            }),
            (first, second) => first.or(second),
        },
        Saved::Stack {
            stack: panes,
            expanded,
        } => stack(panes, expanded),
    }
}

/// Expands pane `id` in the stack holding it; other trees stay as they are.
fn expand(node: &mut Node, id: PaneId) {
    match node {
        Node::Stack { panes, expanded } if panes.contains(&id) => *expanded = id,
        Node::Split { first, second, .. } => {
            expand(first, id);
            expand(second, id);
        }
        _ => {}
    }
}

/// Swaps ids `a` and `b` everywhere in `node`.
fn swap_ids(node: &mut Node, a: PaneId, b: PaneId) {
    let swap = |id: &mut PaneId| match *id {
        x if x == a => *id = b,
        x if x == b => *id = a,
        _ => {}
    };
    match node {
        Node::Pane(id) => swap(id),
        Node::Split { first, second, .. } => {
            swap_ids(first, a, b);
            swap_ids(second, a, b);
        }
        Node::Stack { panes, expanded } => {
            panes.iter_mut().for_each(swap);
            swap(expanded);
        }
    }
}

/// `panes` as a stack expanding `expanded`, or its first pane when
/// `expanded` isn't one of them. One pane is just that pane; none is nothing.
fn stack(panes: Vec<PaneId>, expanded: PaneId) -> Option<Node> {
    match panes.as_slice() {
        [] => None,
        [only] => Some(Node::Pane(*only)),
        [first, ..] => {
            let expanded = if panes.contains(&expanded) {
                expanded
            } else {
                *first
            };
            Some(Node::Stack { panes, expanded })
        }
    }
}

// --- Directional pane navigation ---

/// Find the nearest pane in the given direction from `focused`.
pub fn find_in_direction(
    focused: &PaneInfo,
    direction: NavDirection,
    panes: &[PaneInfo],
) -> Option<PaneId> {
    let fr = focused.rect;

    panes
        .iter()
        .enumerate()
        .filter(|(_, p)| p.id != focused.id)
        .filter(|(_, p)| {
            let r = p.rect;
            match direction {
                NavDirection::Left => {
                    r.x + r.width <= fr.x && ranges_overlap(r.y, r.height, fr.y, fr.height)
                }
                NavDirection::Right => {
                    r.x >= fr.x + fr.width && ranges_overlap(r.y, r.height, fr.y, fr.height)
                }
                NavDirection::Up => {
                    r.y + r.height <= fr.y && ranges_overlap(r.x, r.width, fr.x, fr.width)
                }
                NavDirection::Down => {
                    r.y >= fr.y + fr.height && ranges_overlap(r.x, r.width, fr.x, fr.width)
                }
            }
        })
        .min_by_key(|(index, p)| {
            let r = p.rect;
            let edge_distance = match direction {
                NavDirection::Left => fr.x.saturating_sub(r.x + r.width),
                NavDirection::Right => r.x.saturating_sub(fr.x + fr.width),
                NavDirection::Up => fr.y.saturating_sub(r.y + r.height),
                NavDirection::Down => r.y.saturating_sub(fr.y + fr.height),
            };
            let overlap = match direction {
                NavDirection::Left | NavDirection::Right => {
                    range_overlap_amount(r.y, r.height, fr.y, fr.height)
                }
                NavDirection::Up | NavDirection::Down => {
                    range_overlap_amount(r.x, r.width, fr.x, fr.width)
                }
            };
            let center_distance = match direction {
                NavDirection::Left | NavDirection::Right => {
                    range_center_distance(r.y, r.height, fr.y, fr.height)
                }
                NavDirection::Up | NavDirection::Down => {
                    range_center_distance(r.x, r.width, fr.x, fr.width)
                }
            };
            (edge_distance, Reverse(overlap), center_distance, *index)
        })
        .map(|(_, p)| p.id)
}

fn ranges_overlap(a_start: u16, a_len: u16, b_start: u16, b_len: u16) -> bool {
    a_start < b_start + b_len && a_start + a_len > b_start
}

fn range_overlap_amount(a_start: u16, a_len: u16, b_start: u16, b_len: u16) -> u16 {
    let a_end = a_start.saturating_add(a_len);
    let b_end = b_start.saturating_add(b_len);
    a_end.min(b_end).saturating_sub(a_start.max(b_start))
}

fn range_center_distance(a_start: u16, a_len: u16, b_start: u16, b_len: u16) -> u16 {
    let a_center = a_start.saturating_mul(2).saturating_add(a_len);
    let b_center = b_start.saturating_mul(2).saturating_add(b_len);
    a_center.abs_diff(b_center)
}

// --- Tree operations ---

/// Line sizes for `n` panes, first line first (the vertical and
/// horizontal layouts, 1–10 panes).
fn columns(n: usize) -> Vec<usize> {
    match n {
        0 => vec![],
        1 => vec![1],
        _ => {
            let mut cols = vec![1];
            let mut rest = n - 1;
            while rest > 0 {
                let take = rest.min(4);
                cols.push(take);
                rest -= take;
            }
            if cols.len() > 2 && cols.last() == Some(&1) {
                cols.pop();
                if let Some(first) = cols.first_mut() {
                    *first += 1;
                }
            }
            cols
        }
    }
}

/// `nodes` side by side (`Right`) or top to bottom (`Down`) in equal
/// shares: a chain whose ratios run 1/m, 1/(m-1), …, 1/2.
fn even_chain(mut nodes: Vec<Node>, split: Split) -> Option<Node> {
    let last = nodes.pop()?;
    Some(
        nodes
            .into_iter()
            .rev()
            .enumerate()
            .fold(last, |second, (i, first)| Node::Split {
                split,
                ratio: 1.0 / (i as f32 + 2.0),
                first: Box::new(first),
                second: Box::new(second),
            }),
    )
}

/// `ids` in reading order as lines from [`columns`] chained `across` in
/// equal shares, the panes in each line chained the other way: columns for
/// `Right`, rows for `Down`.
fn grid(ids: &[PaneId], across: Split) -> Option<Node> {
    let along = match across {
        Split::Right => Split::Down,
        Split::Down => Split::Right,
    };
    let mut rest = ids.iter().copied();
    let lines = columns(ids.len())
        .into_iter()
        .filter_map(|k| even_chain(rest.by_ref().take(k).map(Node::Pane).collect(), along))
        .collect();
    even_chain(lines, across)
}

/// `layout`'s tree for `ids` in reading order. A stack expands the first of
/// `prefer` it holds, else its last pane. `None` for no ids.
fn template_for(layout: SwapLayout, ids: &[PaneId], prefer: &[PaneId]) -> Option<Node> {
    let expanded = |panes: &[PaneId]| {
        prefer
            .iter()
            .copied()
            .find(|id| panes.contains(id))
            .or_else(|| panes.last().copied())
    };
    match layout {
        SwapLayout::Base | SwapLayout::Vertical => grid(ids, Split::Right),
        SwapLayout::Horizontal => grid(ids, Split::Down),
        SwapLayout::Stacked => stack(ids.to_vec(), expanded(ids)?),
        SwapLayout::HalfStacked => {
            let (first, rest) = ids.split_first()?;
            even_chain(
                vec![Node::Pane(*first), stack(rest.to_vec(), expanded(rest)?)?],
                Split::Right,
            )
        }
    }
}

/// `node`'s run of `split` splits, flattened: every subtree under it that
/// isn't itself a `split` split, in tree order. Anything else (a pane, a
/// split the other way) is a run of one.
fn run_members(node: &Node, split: Split) -> Vec<Node> {
    match node {
        Node::Split {
            split: direction,
            first,
            second,
            ..
        } if *direction == split => {
            let mut members = run_members(first, split);
            members.extend(run_members(second, split));
            members
        }
        _ => vec![node.clone()],
    }
}

/// Shares the run of `split` splits that pane `id` sits in out evenly: the
/// topmost `split` split whose run has `id` as a member becomes an
/// [`even_chain`] of that run's members.
fn even_run(node: &mut Node, split: Split, id: PaneId) {
    let members = run_members(node, split);
    if members.len() > 1 && members.contains(&Node::Pane(id)) {
        if let Some(chain) = even_chain(members, split) {
            *node = chain;
        }
        return;
    }
    if let Node::Split { first, second, .. } = node {
        even_run(first, split, id);
        even_run(second, split, id);
    }
}

fn count_panes(node: &Node) -> usize {
    match node {
        Node::Pane(_) => 1,
        Node::Split { first, second, .. } => count_panes(first) + count_panes(second),
        Node::Stack { panes, .. } => panes.len(),
    }
}

fn collect_panes(node: &Node, area: Rect, focus: PaneId, result: &mut Vec<PaneInfo>) {
    match node {
        Node::Pane(id) => result.push(PaneInfo {
            id: *id,
            rect: area,
            is_focused: *id == focus,
            collapsed: false,
        }),
        Node::Split {
            split,
            ratio,
            first,
            second,
        } => {
            let (a, b) = split_rect(area, *split, *ratio);
            collect_panes(first, a, focus, result);
            collect_panes(second, b, focus, result);
        }
        Node::Stack { panes, expanded } => {
            let at = panes
                .iter()
                .position(|id| id == expanded)
                .unwrap_or_default();
            let titles = u16::try_from(panes.len().saturating_sub(1)).unwrap_or(u16::MAX);
            // The expanded pane's rows: what the titles leave, at least one.
            let tall = area.height.saturating_sub(titles).max(1).min(area.height);
            // Titles above it that fit beside those rows.
            let above = at.min(usize::from(area.height.saturating_sub(tall)));
            let mut y = area.y;
            for (index, id) in panes.iter().enumerate() {
                let rows = match index.cmp(&at) {
                    Ordering::Equal => tall,
                    Ordering::Less if index < above => 1,
                    Ordering::Less => 0,
                    Ordering::Greater => area.bottom().saturating_sub(y).min(1),
                };
                if rows == 0 && index != at {
                    continue;
                }
                result.push(PaneInfo {
                    id: *id,
                    rect: Rect::new(area.x, y, area.width, rows),
                    is_focused: *id == focus,
                    collapsed: index != at,
                });
                y = y.saturating_add(rows);
            }
        }
    }
}

fn stacks_in(node: &Node, area: Rect, stacks: &mut Vec<StackInfo>) {
    match node {
        Node::Pane(_) => {}
        Node::Split {
            split,
            ratio,
            first,
            second,
        } => {
            let (a, b) = split_rect(area, *split, *ratio);
            stacks_in(first, a, stacks);
            stacks_in(second, b, stacks);
        }
        Node::Stack { panes, expanded } => stacks.push(StackInfo {
            area,
            panes: panes.clone(),
            expanded: *expanded,
        }),
    }
}

fn collect_ids(node: &Node, ids: &mut Vec<PaneId>) {
    match node {
        Node::Pane(id) => ids.push(*id),
        Node::Split { first, second, .. } => {
            collect_ids(first, ids);
            collect_ids(second, ids);
        }
        Node::Stack { panes, .. } => ids.extend(panes.iter().copied()),
    }
}

/// The panes of the stack holding pane `target`, if one does.
fn find_stack_holding_mut(node: &mut Node, target: PaneId) -> Option<&mut Vec<PaneId>> {
    match node {
        Node::Stack { panes, .. } if panes.contains(&target) => Some(panes),
        Node::Pane(_) | Node::Stack { .. } => None,
        Node::Split { first, second, .. } => {
            find_stack_holding_mut(first, target).or_else(|| find_stack_holding_mut(second, target))
        }
    }
}

/// The cell height-to-width ratio: kitty-style 8×16 cells.
// ponytail: fixed cell ratio, read the terminal's pixel cell size if splits pick the wrong way
const CELL_RATIO: u16 = 2;

/// Pane `target`'s node, or the stack holding it.
fn find_holding_mut(node: &mut Node, target: PaneId) -> Option<&mut Node> {
    if holds(node, target) {
        return Some(node);
    }
    match node {
        Node::Split { first, second, .. } => {
            find_holding_mut(first, target).or_else(|| find_holding_mut(second, target))
        }
        Node::Pane(_) | Node::Stack { .. } => None,
    }
}

/// Whether `node` is pane `target` or a stack holding it.
fn holds(node: &Node, target: PaneId) -> bool {
    match node {
        Node::Pane(id) => *id == target,
        Node::Stack { panes, .. } => panes.contains(&target),
        Node::Split { .. } => false,
    }
}

/// A leaf of the tree laid over an area: a pane outside a stack, or a
/// stack, named by its first pane.
#[derive(Debug, Clone, Copy)]
struct Leaf {
    key: PaneId,
    rect: Rect,
    stack: bool,
    /// The focused pane is this pane or in this stack.
    focused: bool,
}

impl Leaf {
    /// Whether the leaf keeps [`MIN_PANE_COLS`] × [`MIN_PANE_ROWS`] (a stack
    /// twice the rows) and [`RESIZE_PERCENT`] of `body` each way.
    fn roomy(&self, body: Rect) -> bool {
        let rows = if self.stack {
            2 * MIN_PANE_ROWS
        } else {
            MIN_PANE_ROWS
        };
        let cols = MIN_PANE_COLS.max(cells(body.width, RESIZE_PERCENT));
        let rows = rows.max(cells(body.height, RESIZE_PERCENT));
        self.rect.width >= cols && self.rect.height >= rows
    }
}

/// `node`'s leaves laid over `area`, in tree order.
fn leaves_of(node: &Node, area: Rect, focus: PaneId) -> Vec<Leaf> {
    fn walk(node: &Node, area: Rect, focus: PaneId, out: &mut Vec<Leaf>) {
        match node {
            Node::Pane(id) => out.push(Leaf {
                key: *id,
                rect: area,
                stack: false,
                focused: *id == focus,
            }),
            Node::Stack { panes, .. } => out.extend(panes.first().map(|&key| Leaf {
                key,
                rect: area,
                stack: true,
                focused: panes.contains(&focus),
            })),
            Node::Split {
                split,
                ratio,
                first,
                second,
            } => {
                let (a, b) = split_rect(area, *split, *ratio);
                walk(first, a, focus, out);
                walk(second, b, focus, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(node, area, focus, &mut out);
    out
}

/// Sets every split's ratio so each leaf lands on its rect in `leaves`.
/// Returns the area `node` covers.
fn fit(node: &mut Node, leaves: &[Leaf]) -> Option<Rect> {
    let rect_of = |key: PaneId| {
        leaves
            .iter()
            .find(|leaf| leaf.key == key)
            .map(|leaf| leaf.rect)
    };
    match node {
        Node::Pane(id) => rect_of(*id),
        Node::Stack { panes, .. } => rect_of(*panes.first()?),
        Node::Split {
            split,
            ratio,
            first,
            second,
        } => {
            let a = fit(first, leaves)?;
            let b = fit(second, leaves)?;
            let (first_len, second_len) = (along(a, *split).1, along(b, *split).1);
            *ratio = f32::from(first_len) / f32::from(first_len + second_len).max(1.0);
            Some(a.union(b))
        }
    }
}

/// The split kind whose border a move towards `dir` crosses.
fn axis(dir: NavDirection) -> Split {
    match dir {
        NavDirection::Left | NavDirection::Right => Split::Right,
        NavDirection::Up | NavDirection::Down => Split::Down,
    }
}

/// `rect`'s start and length in the direction a `split` split divides.
fn along(rect: Rect, split: Split) -> (u16, u16) {
    match split {
        Split::Right => (rect.x, rect.width),
        Split::Down => (rect.y, rect.height),
    }
}

/// `rect`'s start and length across the direction a `split` split divides.
fn across(rect: Rect, split: Split) -> (u16, u16) {
    match split {
        Split::Right => (rect.y, rect.height),
        Split::Down => (rect.x, rect.width),
    }
}

/// The border a unit's edge sits on: the nearest split of kind `split`
/// with the unit on one side, the area it's laid over, and where its
/// border runs.
struct Edge {
    region: Rect,
    split: Split,
    border: u16,
}

/// The border of the focused unit's edge facing `dir`; `None` at the tab's
/// edge.
fn edge(root: &Node, body: Rect, focus: PaneId, dir: NavDirection) -> Option<Edge> {
    let split = axis(dir);
    let unit_first = matches!(dir, NavDirection::Down | NavDirection::Right);
    let (mut node, mut area, mut found) = (root, body, None);
    while let Node::Split {
        split: kind,
        ratio,
        first,
        second,
    } = node
    {
        let (a, b) = split_rect(area, *kind, *ratio);
        let in_first = contains(first, focus);
        if *kind == split && in_first == unit_first {
            found = Some(Edge {
                region: area,
                split,
                border: along(b, split).0,
            });
        }
        (node, area) = if in_first { (first, a) } else { (second, b) };
    }
    found
}

/// Whether pane `target` is in `node`.
fn contains(node: &Node, target: PaneId) -> bool {
    match node {
        Node::Split { first, second, .. } => contains(first, target) || contains(second, target),
        Node::Pane(_) | Node::Stack { .. } => holds(node, target),
    }
}

/// `leaves` with `edge`'s border moved to `to`: each leaf in its region
/// that starts on the border starts at `to`, each that ends on it ends
/// there. `None` when a leaf would have no cells left.
fn shifted(leaves: &[Leaf], edge: &Edge, to: u16) -> Option<Vec<Leaf>> {
    leaves
        .iter()
        .map(|leaf| {
            let inside = edge.region.intersection(leaf.rect) == leaf.rect;
            let (start, len) = along(leaf.rect, edge.split);
            let (start, len) = match () {
                () if inside && start == edge.border => (to, (start + len).checked_sub(to)?),
                () if inside && start + len == edge.border => (start, to.checked_sub(start)?),
                () => (start, len),
            };
            let (side, width) = across(leaf.rect, edge.split);
            let rect = match edge.split {
                Split::Right => Rect::new(start, side, len, width),
                Split::Down => Rect::new(side, start, width, len),
            };
            (len > 0).then_some(Leaf { rect, ..*leaf })
        })
        .collect()
}

/// `node` with the split at `edge` rebuilt so that the unit beside the
/// border (on the first side when `unit_first`) leaves its side and joins
/// the leaf or run of leaves across the border in one stack showing
/// `focus`. `stacked` gets the keys of every leaf merged.
fn stack_at(
    node: Node,
    area: Rect,
    edge: &Edge,
    unit_first: bool,
    focus: PaneId,
    stacked: &mut Vec<PaneId>,
) -> Node {
    let Node::Split {
        split,
        ratio,
        first,
        second,
    } = node
    else {
        return node;
    };
    if area != edge.region || split != edge.split {
        let (a, b) = split_rect(area, split, ratio);
        let (first, second) = if contains(&first, focus) {
            (
                Box::new(stack_at(*first, a, edge, unit_first, focus, stacked)),
                second,
            )
        } else {
            (
                first,
                Box::new(stack_at(*second, b, edge, unit_first, focus, stacked)),
            )
        };
        return Node::Split {
            split,
            ratio,
            first,
            second,
        };
    }
    let (unit_side, other) = if unit_first {
        (*first, *second)
    } else {
        (*second, *first)
    };
    let mut unit_ids = Vec::new();
    let rest = drop_touching(unit_side, split, !unit_first, &mut unit_ids);
    let mut merged = Vec::new();
    let other = replace_touching(other, split, unit_first, &mut |across| {
        let mut ids = Vec::new();
        collect_ids(&across, &mut ids);
        merged = across_keys(&across);
        let panes = if unit_first {
            unit_ids.iter().chain(&ids).copied().collect()
        } else {
            ids.iter().chain(&unit_ids).copied().collect()
        };
        Node::Stack {
            panes,
            expanded: focus,
        }
    });
    stacked.extend(merged);
    stacked.extend(unit_ids.first().copied());
    match rest {
        None => other,
        Some(rest) if unit_first => split_pair(split, other, rest, false),
        Some(rest) => split_pair(split, other, rest, true),
    }
}

/// The split of `a` and `b`, `a` first when `a_first`; its ratio is set by
/// [`fit`].
fn split_pair(split: Split, a: Node, b: Node, a_first: bool) -> Node {
    let (first, second) = if a_first { (a, b) } else { (b, a) };
    Node::Split {
        split,
        ratio: 0.5,
        first: Box::new(first),
        second: Box::new(second),
    }
}

/// The keys of `node`'s leaves.
fn across_keys(node: &Node) -> Vec<PaneId> {
    leaves_of(node, Rect::default(), PaneId(0))
        .into_iter()
        .map(|leaf| leaf.key)
        .collect()
}

/// `node` without its leaf on the border side (`first` when
/// `touch_first`) through `split` splits; `ids` gets that leaf's panes.
/// `None` when `node` is that leaf.
fn drop_touching(
    node: Node,
    split: Split,
    touch_first: bool,
    ids: &mut Vec<PaneId>,
) -> Option<Node> {
    match node {
        Node::Split {
            split: kind,
            ratio,
            first,
            second,
        } if kind == split => {
            let (touching, far) = if touch_first {
                (*first, *second)
            } else {
                (*second, *first)
            };
            Some(match drop_touching(touching, split, touch_first, ids) {
                None => far,
                Some(kept) => {
                    let (first, second) = if touch_first {
                        (kept, far)
                    } else {
                        (far, kept)
                    };
                    Node::Split {
                        split,
                        ratio,
                        first: Box::new(first),
                        second: Box::new(second),
                    }
                }
            })
        }
        leaf => {
            collect_ids(&leaf, ids);
            None
        }
    }
}

/// `node` with the subtree on the border side (`first` when `touch_first`)
/// through `split` splits replaced by `with` of it.
fn replace_touching(
    node: Node,
    split: Split,
    touch_first: bool,
    with: &mut dyn FnMut(Node) -> Node,
) -> Node {
    match node {
        Node::Split {
            split: kind,
            ratio,
            first,
            second,
        } if kind == split => {
            let (first, second) = if touch_first {
                (
                    Box::new(replace_touching(*first, split, touch_first, with)),
                    second,
                )
            } else {
                (
                    first,
                    Box::new(replace_touching(*second, split, touch_first, with)),
                )
            };
            Node::Split {
                split,
                ratio,
                first,
                second,
            }
        }
        other => with(other),
    }
}

fn split_node(first: Node, split: Split, new_id: PaneId, ratio: f32) -> Node {
    Node::Split {
        split,
        ratio,
        first: Box::new(first),
        second: Box::new(Node::Pane(new_id)),
    }
}

fn valid_split_ratio(ratio: f32) -> f32 {
    if ratio.is_finite() {
        ratio.clamp(0.1, 0.9)
    } else {
        0.5
    }
}

fn remove_pane(node: Node, target: PaneId) -> Option<Node> {
    match node {
        Node::Pane(id) if id == target => None,
        Node::Pane(_) => Some(node),
        Node::Split {
            split,
            ratio,
            first,
            second,
        } => match (remove_pane(*first, target), remove_pane(*second, target)) {
            (None, Some(s)) => Some(s),
            (Some(f), None) => Some(f),
            (Some(f), Some(s)) => Some(Node::Split {
                split,
                ratio,
                first: Box::new(f),
                second: Box::new(s),
            }),
            (None, None) => None,
        },
        Node::Stack {
            mut panes,
            expanded,
        } => {
            let Some(at) = panes.iter().position(|id| *id == target) else {
                return Some(Node::Stack { panes, expanded });
            };
            panes.remove(at);
            let expanded = if expanded == target {
                panes
                    .get(at)
                    .or_else(|| panes.get(at.checked_sub(1)?))
                    .copied()
                    .unwrap_or(expanded)
            } else {
                expanded
            };
            stack(panes, expanded)
        }
    }
}

fn split_rect(area: Rect, split: Split, ratio: f32) -> (Rect, Rect) {
    match split {
        Split::Right => {
            let first_w = (f32::from(area.width) * ratio).round() as u16;
            let second_w = area.width.saturating_sub(first_w);
            (
                Rect::new(area.x, area.y, first_w, area.height),
                Rect::new(area.x + first_w, area.y, second_w, area.height),
            )
        }
        Split::Down => {
            let first_h = (f32::from(area.height) * ratio).round() as u16;
            let second_h = area.height.saturating_sub(first_h);
            (
                Rect::new(area.x, area.y, area.width, first_h),
                Rect::new(area.x, area.y + first_h, area.width, second_h),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;

    use super::{
        Grow, NavDirection, Node, PaneInfo, Split, SwapLayout, TileLayout, columns,
        find_in_direction,
    };
    use crate::feat::sessions::state::PaneId;

    const AREA: Rect = Rect::new(0, 0, 100, 40);

    fn pane(id: i64) -> PaneId {
        PaneId(id)
    }

    fn split(split: Split, ratio: f32, first: Node, second: Node) -> Node {
        Node::Split {
            split,
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    /// Pane 1 on the left third; on the right, pane 2 on top and panes 3
    /// and 4 side by side below it. Pane 2 has the focus.
    fn sample_layout() -> TileLayout {
        TileLayout::from_saved(
            split(
                Split::Right,
                0.3,
                Node::Pane(pane(1)),
                split(
                    Split::Down,
                    0.6,
                    Node::Pane(pane(2)),
                    split(Split::Right, 0.4, Node::Pane(pane(3)), Node::Pane(pane(4))),
                ),
            ),
            pane(2),
        )
    }

    /// Panes 1 and 2 side by side, pane `focus` focused.
    fn halves(focus: i64) -> TileLayout {
        TileLayout::from_saved(
            split(Split::Right, 0.5, Node::Pane(pane(1)), Node::Pane(pane(2))),
            pane(focus),
        )
    }

    fn rects(layout: &TileLayout) -> Vec<(PaneId, Rect)> {
        layout
            .panes(AREA)
            .into_iter()
            .map(|info| (info.id, info.rect))
            .collect()
    }

    fn ratio(layout: &TileLayout) -> Option<f32> {
        match layout.root {
            Node::Split { ratio, .. } => Some(ratio),
            _ => None,
        }
    }

    /// A layout of panes 1 to `n`, each added with `add_tiled` in vertical.
    fn tiled(n: i64) -> TileLayout {
        tiled_as(n, SwapLayout::Vertical)
    }

    /// A layout of panes 1 to `n`, each added with `add_tiled` in `layout`.
    fn tiled_as(n: i64, layout: SwapLayout) -> TileLayout {
        let mut tree = TileLayout::new(pane(1));
        for id in 2..=n {
            tree.add_tiled(pane(id), layout);
        }
        tree
    }

    /// How many panes each row holds, top to bottom.
    fn rows_of(layout: &TileLayout, area: Rect) -> Vec<usize> {
        layout
            .panes(area)
            .chunk_by(|a, b| a.rect.y == b.rect.y)
            .map(<[PaneInfo]>::len)
            .collect()
    }

    /// Every pane's rect in `area`, in tree order.
    fn rects_in(layout: &TileLayout, area: Rect) -> Vec<(PaneId, Rect)> {
        layout
            .panes(area)
            .into_iter()
            .map(|info| (info.id, info.rect))
            .collect()
    }

    /// Every stack's panes, in tree order.
    fn stacked(layout: &TileLayout) -> Vec<Vec<PaneId>> {
        layout
            .stacks(AREA)
            .into_iter()
            .map(|stack| stack.panes)
            .collect()
    }

    /// The pane rects in `area`, one list per column, left to right.
    fn columns_of(layout: &TileLayout, area: Rect) -> Vec<Vec<Rect>> {
        layout
            .panes(area)
            .chunk_by(|a, b| a.rect.x == b.rect.x)
            .map(|column| column.iter().map(|info| info.rect).collect())
            .collect()
    }

    /// How many panes each column holds, left to right.
    fn shape(layout: &TileLayout, area: Rect) -> Vec<usize> {
        columns_of(layout, area).iter().map(Vec::len).collect()
    }

    /// The biggest gap between any two of `sizes`.
    fn spread(sizes: &[u16]) -> u16 {
        let max = sizes.iter().max().copied().unwrap_or_default();
        let min = sizes.iter().min().copied().unwrap_or_default();
        max - min
    }

    fn info(layout: &TileLayout, id: i64) -> Option<PaneInfo> {
        layout
            .panes(AREA)
            .into_iter()
            .find(|info| info.id == pane(id))
    }

    #[rstest::rstest]
    fn split_right_puts_the_new_pane_beside_the_focused_one() {
        // Given a layout of pane 1.
        let mut layout = TileLayout::new(pane(1));

        // When splitting it right with pane 2.
        layout.split_focused(Split::Right, pane(2));

        // Then pane 1 takes the left half and pane 2 the right half.
        assert_eq!(
            rects(&layout),
            vec![
                (pane(1), Rect::new(0, 0, 50, 40)),
                (pane(2), Rect::new(50, 0, 50, 40)),
            ],
            "a right split should put the panes side by side"
        );
    }

    #[rstest::rstest]
    fn split_down_puts_the_new_pane_below_the_focused_one() {
        // Given a layout of pane 1.
        let mut layout = TileLayout::new(pane(1));

        // When splitting it down with pane 2.
        layout.split_focused(Split::Down, pane(2));

        // Then pane 1 takes the top half and pane 2 the bottom half.
        assert_eq!(
            rects(&layout),
            vec![
                (pane(1), Rect::new(0, 0, 100, 20)),
                (pane(2), Rect::new(0, 20, 100, 20)),
            ],
            "a down split should stack the panes"
        );
    }

    #[rstest::rstest]
    fn split_focuses_the_new_pane() {
        // Given a layout of pane 1.
        let mut layout = TileLayout::new(pane(1));

        // When splitting it with pane 2.
        layout.split_focused(Split::Right, pane(2));

        // Then pane 2 has the focus.
        assert_eq!(layout.focused(), pane(2), "the new pane should be focused");
    }

    #[rstest::rstest]
    fn split_right_beside_two_columns_gives_three_equal_columns() {
        // Given panes 1 and 2 side by side, pane 2 focused.
        let mut layout = halves(2);

        // When splitting pane 2 right with pane 3.
        layout.split_focused(Split::Right, pane(3));

        // Then the tab is three columns of equal width.
        let widths: Vec<u16> = columns_of(&layout, AREA)
            .iter()
            .filter_map(|column| column.first().map(|rect| rect.width))
            .collect();
        assert_eq!(
            (shape(&layout, AREA), spread(&widths) <= 1),
            (vec![1, 1, 1], true),
            "a right split beside two columns should give three even columns, widths {widths:?}"
        );
    }

    #[rstest::rstest]
    fn split_down_inside_the_middle_column_keeps_the_columns_at_thirds() {
        // Given three equal columns of panes 1, 2 and 3, pane 2 focused.
        let mut layout = TileLayout::from_saved(
            split(
                Split::Right,
                1.0 / 3.0,
                Node::Pane(pane(1)),
                split(Split::Right, 0.5, Node::Pane(pane(2)), Node::Pane(pane(3))),
            ),
            pane(2),
        );

        // When splitting pane 2 down with pane 4.
        layout.split_focused(Split::Down, pane(4));

        // Then only the middle column holds two panes, and the columns stay at thirds.
        let widths: Vec<u16> = columns_of(&layout, AREA)
            .iter()
            .filter_map(|column| column.first().map(|rect| rect.width))
            .collect();
        assert_eq!(
            (shape(&layout, AREA), spread(&widths) <= 1),
            (vec![1, 2, 1], true),
            "a down split should divide only its column, widths {widths:?}"
        );
    }

    #[rstest::rstest]
    fn close_focused_returns_to_the_pane_focus_came_from() {
        // Given the sample layout with the focus moved from pane 2 to pane 4.
        let mut layout = sample_layout();
        layout.focus_pane(pane(4));

        // When closing the focused pane.
        let closed = layout.close_focused();

        // Then the focus is back on pane 2.
        assert_eq!(
            (closed, layout.focused()),
            (true, pane(2)),
            "closing should return the focus to where it came from"
        );
    }

    #[rstest::rstest]
    fn close_focused_returns_to_the_pane_that_opened_a_split() {
        // Given three panes, and pane 1 then split with pane 4.
        let mut layout = TileLayout::new(pane(1));
        layout.split_focused(Split::Right, pane(2));
        layout.split_focused(Split::Down, pane(3));
        layout.focus_pane(pane(1));
        layout.split_focused(Split::Right, pane(4));

        // When closing pane 4.
        layout.close_focused();

        // Then pane 1 has the focus again.
        assert_eq!(
            layout.focused(),
            pane(1),
            "closing a fresh split should return to the pane that opened it"
        );
    }

    #[rstest::rstest]
    fn closing_a_background_pane_keeps_the_focus() {
        // Given the sample layout focused on pane 4.
        let mut layout = sample_layout();
        layout.focus_pane(pane(4));

        // When closing pane 1, which isn't focused.
        layout.close_pane(pane(1));

        // Then pane 4 keeps the focus.
        assert_eq!(
            layout.focused(),
            pane(4),
            "closing another pane should leave the focus alone"
        );
    }

    #[rstest::rstest]
    fn close_focused_uses_tree_order_without_focus_history() {
        // Given the sample layout with no focus moves.
        let mut layout = sample_layout();

        // When closing the focused pane 2.
        layout.close_focused();

        // Then the next pane in tree order, pane 3, has the focus.
        assert_eq!(
            layout.focused(),
            pane(3),
            "without history the next pane should take the focus"
        );
    }

    #[rstest::rstest]
    fn closing_the_last_pane_is_refused() {
        // Given a layout of one pane.
        let mut layout = TileLayout::new(pane(1));

        // When closing it.
        let closed = layout.close_focused();

        // Then nothing closes.
        assert!(!closed, "the last pane should not close");
    }

    #[rstest::rstest]
    #[case(3, NavDirection::Left, 1)]
    #[case(3, NavDirection::Right, 4)]
    #[case(3, NavDirection::Up, 2)]
    #[case(2, NavDirection::Down, 4)]
    fn find_in_direction_finds_the_neighbour(
        #[case] from: i64,
        #[case] nav: NavDirection,
        #[case] expected: i64,
    ) {
        // Given the sample layout.
        let layout = sample_layout();
        let panes = layout.panes(AREA);

        // When looking from pane `from` in direction `nav`.
        let found = info(&layout, from).and_then(|from| find_in_direction(&from, nav, &panes));

        // Then the neighbour is found.
        assert_eq!(found, Some(pane(expected)), "neighbour of the pane");
    }

    #[rstest::rstest]
    fn find_in_direction_finds_nothing_past_the_edge() {
        // Given the sample layout.
        let layout = sample_layout();
        let panes = layout.panes(AREA);

        // When looking left from the leftmost pane 1.
        let found =
            info(&layout, 1).and_then(|from| find_in_direction(&from, NavDirection::Left, &panes));

        // Then nothing is there.
        assert_eq!(found, None, "nothing should be left of the leftmost pane");
    }

    #[rstest::rstest]
    fn find_in_direction_tiebreaks_by_larger_overlap() {
        // Given a pane with two panes to its left, the second overlapping it more.
        let at = |id, rect| PaneInfo {
            id: pane(id),
            rect,
            is_focused: false,
            collapsed: false,
        };
        let focused = at(1, Rect::new(10, 10, 10, 10));
        let panes = [
            focused,
            at(2, Rect::new(0, 10, 10, 2)),
            at(3, Rect::new(0, 10, 10, 8)),
        ];

        // When looking left.
        let found = find_in_direction(&focused, NavDirection::Left, &panes);

        // Then the one with the larger overlap wins.
        assert_eq!(found, Some(pane(3)), "larger overlap should win the tie");
    }

    #[rstest::rstest]
    fn layout_json_roundtrips() {
        // Given a three-pane tree focused on pane 3.
        let mut layout = TileLayout::new(pane(1));
        layout.split_focused(Split::Right, pane(2));
        layout.split_focused(Split::Down, pane(3));

        // When saving and loading it.
        let loaded = TileLayout::from_json(&layout.to_json(), pane(3)).ok();

        // Then the panes land in the same places with the same focus.
        assert_eq!(
            loaded.map(|loaded| (rects(&loaded), loaded.focused())),
            Some((rects(&layout), pane(3))),
            "the loaded tree should match the saved one"
        );
    }

    #[rstest::rstest]
    fn layout_json_names_splits_right_and_down() {
        // Given panes 1 and 2 side by side.
        let layout = halves(1);

        // When saving it.
        let json = layout.to_json();

        // Then the split is named right.
        assert_eq!(
            json, r#"{"split":"right","ratio":0.5,"first":{"pane":1},"second":{"pane":2}}"#,
            "the saved tree's shape"
        );
    }

    #[rstest::rstest]
    fn from_json_falls_back_to_the_first_pane_for_an_unknown_focus() {
        // Given a saved two-pane tree.
        let json = halves(2).to_json();

        // When loading it focused on a pane it doesn't hold.
        let loaded = TileLayout::from_json(&json, pane(9)).ok();

        // Then its first pane has the focus.
        assert_eq!(
            loaded.map(|loaded| loaded.focused()),
            Some(pane(1)),
            "an unknown focus should fall back to the first pane"
        );
    }

    #[rstest::rstest]
    fn from_json_rejects_text_that_isnt_a_tree() {
        // Given text that isn't a saved tree.
        let json = r#"{"tabs":3}"#;

        // When loading it.
        let loaded = TileLayout::from_json(json, pane(1));

        // Then it is refused.
        assert!(loaded.is_err(), "a non-tree should be rejected");
    }

    #[rstest::rstest]
    #[case(1, vec![1])]
    #[case(2, vec![1, 1])]
    #[case(3, vec![1, 2])]
    #[case(4, vec![1, 3])]
    #[case(5, vec![1, 4])]
    #[case(6, vec![2, 4])]
    #[case(7, vec![1, 4, 2])]
    #[case(8, vec![1, 4, 3])]
    #[case(9, vec![1, 4, 4])]
    #[case(10, vec![2, 4, 4])]
    fn columns_follow_the_template_table(#[case] n: usize, #[case] expected: Vec<usize>) {
        // Given / When / Then the column sizes for n panes match the table.
        assert_eq!(columns(n), expected, "columns for {n} panes");
    }

    #[rstest::rstest]
    #[case(1, vec![1])]
    #[case(2, vec![1, 1])]
    #[case(3, vec![1, 2])]
    #[case(4, vec![1, 3])]
    #[case(5, vec![1, 4])]
    #[case(6, vec![2, 4])]
    #[case(7, vec![1, 4, 2])]
    #[case(8, vec![1, 4, 3])]
    #[case(9, vec![1, 4, 4])]
    #[case(10, vec![2, 4, 4])]
    fn panes_added_one_at_a_time_follow_the_template_table(
        #[case] n: i64,
        #[case] expected: Vec<usize>,
    ) {
        // Given panes added one at a time up to n.
        let layout = tiled(n);

        // When laying them out.
        let shape = shape(&layout, AREA);

        // Then each column holds the table's pane count.
        assert_eq!(shape, expected, "shape for {n} panes");
    }

    #[rstest::rstest]
    fn panes_added_one_at_a_time_keep_even_sizes(#[values(1, 2, 3, 4, 5, 6, 7, 8, 9, 10)] n: i64) {
        // Given panes added one at a time up to n.
        let layout = tiled(n);

        // When laying them out.
        let columns = columns_of(&layout, AREA);

        // Then column widths differ by at most one cell, and so do the row
        // heights within each column.
        let widths: Vec<u16> = columns
            .iter()
            .filter_map(|column| column.first().map(|rect| rect.width))
            .collect();
        let heights: Vec<Vec<u16>> = columns
            .iter()
            .map(|column| column.iter().map(|rect| rect.height).collect())
            .collect();
        assert!(
            spread(&widths) <= 1 && heights.iter().all(|column| spread(column) <= 1),
            "uneven sizes for {n} panes, widths {widths:?}, heights {heights:?}"
        );
    }

    #[rstest::rstest]
    fn tiled_columns_share_the_width_evenly() {
        // Given seven tiled panes.
        let layout = tiled(7);

        // When laying them out in a 120×40 area.
        let widths: Vec<u16> = columns_of(&layout, Rect::new(0, 0, 120, 40))
            .iter()
            .filter_map(|column| column.first().map(|rect| rect.width))
            .collect();

        // Then the column widths differ by at most one cell.
        assert!(spread(&widths) <= 1, "uneven column widths {widths:?}");
    }

    #[rstest::rstest]
    fn tiled_column_rows_share_the_height_evenly() {
        // Given five tiled panes, one column then four.
        let layout = tiled(5);

        // When laying them out in a 40-row area.
        let heights: Vec<u16> = columns_of(&layout, AREA)
            .get(1)
            .map(|column| column.iter().map(|rect| rect.height).collect())
            .unwrap_or_default();

        // Then the second column's four heights differ by at most one row.
        assert!(
            heights.len() == 4 && spread(&heights) <= 1,
            "uneven row heights {heights:?}"
        );
    }

    #[rstest::rstest]
    fn add_tiled_lays_six_panes_out_two_and_four() {
        // Given five tiled panes.
        let mut layout = tiled(5);

        // When adding pane 6.
        layout.add_tiled(pane(6), SwapLayout::Vertical);

        // Then two panes sit in the first column and four in the second.
        assert_eq!(shape(&layout, AREA), vec![2, 4], "six panes tile [2][4]");
    }

    #[rstest::rstest]
    fn add_tiled_focuses_the_new_pane() {
        // Given five tiled panes.
        let mut layout = tiled(5);

        // When adding pane 6.
        layout.add_tiled(pane(6), SwapLayout::Vertical);

        // Then pane 6 has the focus.
        assert_eq!(
            layout.focused(),
            pane(6),
            "the added pane should be focused"
        );
    }

    #[rstest::rstest]
    fn add_tiled_evens_out_a_resized_tab() {
        // Given three tiled panes with the focused pane's border moved.
        let mut layout = tiled(3);
        layout.grow_focused(AREA);

        // When adding pane 4.
        layout.add_tiled(pane(4), SwapLayout::Vertical);

        // Then the tab is one column then three, every size even again.
        let columns = columns_of(&layout, AREA);
        let widths: Vec<u16> = columns
            .iter()
            .filter_map(|column| column.first().map(|rect| rect.width))
            .collect();
        let heights: Vec<u16> = columns
            .get(1)
            .map(|column| column.iter().map(|rect| rect.height).collect())
            .unwrap_or_default();
        assert_eq!(
            (
                shape(&layout, AREA),
                spread(&widths) <= 1,
                spread(&heights) <= 1
            ),
            (vec![1, 3], true, true),
            "the re-tiled tab should be [1][3] with even sizes, widths {widths:?}, heights {heights:?}"
        );
    }

    /// Panes `ids` stacked, pane `expanded` expanded.
    fn stack_of(ids: &[i64], expanded: i64) -> Node {
        Node::Stack {
            panes: ids.iter().copied().map(pane).collect(),
            expanded: pane(expanded),
        }
    }

    /// Pane 1 on the left half; panes 2, 3 and 4 stacked on the right half,
    /// pane `expanded` expanded. Pane `focus` has the focus.
    fn main_and_stack(expanded: i64, focus: i64) -> TileLayout {
        TileLayout::from_saved(
            split(
                Split::Right,
                0.5,
                Node::Pane(pane(1)),
                stack_of(&[2, 3, 4], expanded),
            ),
            pane(focus),
        )
    }

    /// The stacked pane that gets rows of its own.
    fn expanded(layout: &TileLayout) -> Option<PaneId> {
        layout.stacks(AREA).first().map(|stack| stack.expanded)
    }

    #[rstest::rstest]
    fn stacked_template_is_one_stack_of_every_pane() {
        // Given eleven panes tiled in stacked.
        let layout = tiled_as(11, SwapLayout::Stacked);

        // When laying them out.
        let collapsed = layout
            .panes(AREA)
            .iter()
            .filter(|info| info.collapsed)
            .count();

        // Then all eleven sit in one stack, ten of them titles.
        assert_eq!(
            (shape(&layout, AREA), collapsed),
            (vec![11], 10),
            "eleven panes tile as one stack"
        );
    }

    #[rstest::rstest]
    fn stacked_template_expands_the_focused_pane() {
        // Given four panes tiled vertical, pane 2 focused.
        let mut layout = tiled(4);
        layout.focus_pane(pane(2));

        // When re-tiling in stacked.
        layout.tile(SwapLayout::Stacked);

        // Then pane 2 is the stack's expanded pane.
        assert_eq!(
            expanded(&layout),
            Some(pane(2)),
            "the focused pane should be expanded"
        );
    }

    #[rstest::rstest]
    fn half_stacked_template_puts_the_first_pane_on_the_left_half() {
        // Given four panes tiled in half-stacked.
        let layout = tiled_as(4, SwapLayout::HalfStacked);

        // When laying them out.
        let left = info(&layout, 1).map(|info| info.rect);

        // Then pane 1 takes the left half and panes 2 to 4 are one stack.
        assert_eq!(
            (left, stacked(&layout)),
            (
                Some(Rect::new(0, 0, 50, 40)),
                vec![vec![pane(2), pane(3), pane(4)]]
            ),
            "half-stacked puts pane 1 on the left half beside one stack"
        );
    }

    #[rstest::rstest]
    #[case(1, vec![1])]
    #[case(2, vec![1, 1])]
    #[case(3, vec![1, 2])]
    #[case(4, vec![1, 3])]
    #[case(5, vec![1, 4])]
    #[case(6, vec![2, 4])]
    #[case(7, vec![1, 4, 2])]
    #[case(8, vec![1, 4, 3])]
    #[case(9, vec![1, 4, 4])]
    #[case(10, vec![2, 4, 4])]
    fn horizontal_template_lays_rows_from_the_table(#[case] n: i64, #[case] expected: Vec<usize>) {
        // Given n panes tiled in horizontal.
        let layout = tiled_as(n, SwapLayout::Horizontal);

        // When laying them out.
        let rows = rows_of(&layout, AREA);

        // Then each row holds the table's pane count.
        assert_eq!(rows, expected, "rows for {n} panes");
    }

    #[rstest::rstest]
    #[case(SwapLayout::Base, 1, true)]
    #[case(SwapLayout::Base, 2, false)]
    #[case(SwapLayout::Vertical, 10, true)]
    #[case(SwapLayout::Vertical, 11, false)]
    #[case(SwapLayout::Horizontal, 10, true)]
    #[case(SwapLayout::Horizontal, 11, false)]
    #[case(SwapLayout::Stacked, 1, false)]
    #[case(SwapLayout::Stacked, 2, true)]
    #[case(SwapLayout::HalfStacked, 2, false)]
    #[case(SwapLayout::HalfStacked, 3, true)]
    fn swap_layout_fits_from_the_table(
        #[case] layout: SwapLayout,
        #[case] n: usize,
        #[case] fits: bool,
    ) {
        // Given / When searching for a fitting layout from the layout itself.
        let found = layout.fitting(n, false, false);

        // Then it finds itself only when it fits n panes.
        assert_eq!(found == Some(layout), fits, "{layout:?} fitting {n} panes");
    }

    #[rstest::rstest]
    fn stack_focused_on_a_lone_pane_stacks_the_new_pane_shown() {
        // Given a lone pane 1.
        let mut layout = TileLayout::new(pane(1));

        // When stacking pane 2 with it.
        layout.stack_focused(pane(2));

        // Then panes 1 and 2 are a stack with pane 2 expanded and focused.
        assert_eq!(
            (
                layout
                    .stacks(AREA)
                    .into_iter()
                    .next()
                    .map(|stack| stack.panes),
                expanded(&layout),
                layout.focused()
            ),
            (Some(vec![pane(1), pane(2)]), Some(pane(2)), pane(2)),
            "the new pane is stacked after the focused one and shown"
        );
    }

    #[rstest::rstest]
    fn stack_focused_in_a_stack_inserts_after_the_focused_pane() {
        // Given pane 1 beside a stack of 2, 3 and 4, pane 2 focused.
        let mut layout = main_and_stack(2, 2);

        // When stacking pane 5.
        layout.stack_focused(pane(5));

        // Then pane 5 sits right after pane 2.
        assert_eq!(
            layout
                .stacks(AREA)
                .into_iter()
                .next()
                .map(|stack| stack.panes),
            Some(vec![pane(2), pane(5), pane(3), pane(4)]),
            "the new pane goes right after the focused one"
        );
    }

    #[rstest::rstest]
    fn stack_focused_outside_every_stack_starts_a_new_one() {
        // Given pane 1 beside a stack of 2, 3 and 4, pane 1 focused.
        let mut layout = main_and_stack(3, 1);

        // When stacking pane 5.
        layout.stack_focused(pane(5));

        // Then panes 1 and 5 are a second stack beside the first.
        assert_eq!(
            layout
                .stacks(AREA)
                .into_iter()
                .map(|stack| stack.panes)
                .collect::<Vec<_>>(),
            vec![vec![pane(1), pane(5)], vec![pane(2), pane(3), pane(4)]],
            "a pane outside every stack starts its own stack"
        );
    }

    /// A stack of panes 1 and 2 (1 expanded) on the left half beside a stack
    /// of panes 3 and 4 (3 expanded) on the right half. Pane `focus` has the
    /// focus.
    fn two_stacks(focus: i64) -> TileLayout {
        TileLayout::from_saved(
            split(
                Split::Right,
                0.5,
                stack_of(&[1, 2], 1),
                stack_of(&[3, 4], 3),
            ),
            pane(focus),
        )
    }

    #[rstest::rstest]
    fn stack_focused_joins_the_focused_panes_own_stack() {
        // Given two stacks, pane 3 in the second one focused.
        let mut layout = two_stacks(3);

        // When stacking pane 5.
        layout.stack_focused(pane(5));

        // Then pane 5 sits right after pane 3 in the second stack.
        assert_eq!(
            layout
                .stacks(AREA)
                .into_iter()
                .map(|stack| stack.panes)
                .collect::<Vec<_>>(),
            vec![vec![pane(1), pane(2)], vec![pane(3), pane(5), pane(4)]],
            "the new pane joins the focused pane's own stack"
        );
    }

    #[rstest::rstest]
    fn stacks_lists_every_stack_in_tree_order() {
        // Given two stacks side by side.
        let layout = two_stacks(1);

        // When listing the stacks.
        let stacks: Vec<(Rect, Vec<PaneId>)> = layout
            .stacks(AREA)
            .into_iter()
            .map(|stack| (stack.area, stack.panes))
            .collect();

        // Then the left stack comes first, then the right one, each on its half.
        assert_eq!(
            stacks,
            vec![
                (Rect::new(0, 0, 50, 40), vec![pane(1), pane(2)]),
                (Rect::new(50, 0, 50, 40), vec![pane(3), pane(4)]),
            ],
            "every stack is listed in tree order with its area"
        );
    }

    #[rstest::rstest]
    fn stack_titles_take_one_row_each_and_the_expanded_pane_the_rest() {
        // Given pane 1 beside a stack of panes 2, 3 and 4, pane 3 expanded.
        let layout = main_and_stack(3, 1);

        // When laying it out in a 40-row area.
        let stacked: Vec<(PaneId, Rect)> = rects(&layout).into_iter().skip(1).collect();

        // Then panes 2 and 4 take one row each and pane 3 every row between them.
        assert_eq!(
            stacked,
            vec![
                (pane(2), Rect::new(50, 0, 50, 1)),
                (pane(3), Rect::new(50, 1, 50, 38)),
                (pane(4), Rect::new(50, 39, 50, 1)),
            ],
            "titles are one row, the expanded pane takes the rest"
        );
    }

    #[rstest::rstest]
    fn stack_in_a_short_area_keeps_a_row_for_the_expanded_pane() {
        // Given a stack of panes 1 to 5, pane 5 expanded.
        let layout = TileLayout::from_saved(stack_of(&[1, 2, 3, 4, 5], 5), pane(5));

        // When laying it out in a 3-row area.
        let rect = layout
            .panes(Rect::new(0, 0, 100, 3))
            .into_iter()
            .find(|info| info.id == pane(5))
            .map(|info| info.rect);

        // Then pane 5 still gets the last row.
        assert_eq!(
            rect,
            Some(Rect::new(0, 2, 100, 1)),
            "the expanded pane should keep a row"
        );
    }

    #[rstest::rstest]
    fn focusing_a_stacked_pane_expands_it() {
        // Given pane 1 beside a stack of 2, 3 and 4 with 3 expanded, pane 1 focused.
        let mut layout = main_and_stack(3, 1);

        // When focusing pane 2.
        layout.focus_pane(pane(2));

        // Then pane 2 is expanded.
        assert_eq!(
            expanded(&layout),
            Some(pane(2)),
            "the focused stacked pane should be expanded"
        );
    }

    #[rstest::rstest]
    fn add_tiled_expands_the_new_pane() {
        // Given eleven panes tiled in stacked.
        let mut layout = tiled_as(11, SwapLayout::Stacked);

        // When adding pane 12.
        layout.add_tiled(pane(12), SwapLayout::Stacked);

        // Then pane 12 is the expanded pane of the stack.
        assert_eq!(
            expanded(&layout),
            Some(pane(12)),
            "the added pane should be expanded"
        );
    }

    #[rstest::rstest]
    fn split_in_a_stacked_layout_focuses_the_new_pane() {
        // Given eleven panes tiled in stacked, pane 5 focused.
        let mut layout = tiled_as(11, SwapLayout::Stacked);
        layout.focus_pane(pane(5));

        // When splitting it right with pane 12.
        layout.split_focused(Split::Right, pane(12));

        // Then pane 12 has the focus.
        assert_eq!(
            layout.focused(),
            pane(12),
            "the split's new pane should be focused"
        );
    }

    #[rstest::rstest]
    fn split_of_a_stacked_pane_keeps_its_stack_whole() {
        // Given pane 1 beside a stack of 2, 3 and 4, pane 3 focused.
        let mut layout = main_and_stack(3, 3);

        // When splitting it right with pane 5.
        layout.split_focused(Split::Right, pane(5));

        // Then the stack still holds panes 2, 3 and 4.
        assert_eq!(
            stacked(&layout),
            vec![vec![pane(2), pane(3), pane(4)]],
            "a split should take the whole stack, not one pane out of it"
        );
    }

    #[rstest::rstest]
    fn add_beside_focus_in_a_stack_joins_it() {
        // Given pane 1 beside a stack of 2, 3 and 4, pane 3 focused.
        let mut layout = main_and_stack(3, 3);

        // When adding pane 5 beside the focus.
        layout.add_beside_focus(pane(5), AREA);

        // Then pane 5 joins the stack right after pane 3.
        assert_eq!(
            stacked(&layout),
            vec![vec![pane(2), pane(3), pane(5), pane(4)]],
            "a new pane beside a stacked focus joins its stack"
        );
    }

    #[rstest::rstest]
    fn add_beside_focus_splits_a_tall_pane_top_and_bottom() {
        // Given a lone pane 1 in a 60×40 area.
        let area = Rect::new(0, 0, 60, 40);
        let mut layout = TileLayout::new(pane(1));

        // When adding pane 2 beside the focus.
        layout.add_beside_focus(pane(2), area);

        // Then pane 2 sits below pane 1.
        assert_eq!(
            rects_in(&layout, area),
            vec![
                (pane(1), Rect::new(0, 0, 60, 20)),
                (pane(2), Rect::new(0, 20, 60, 20)),
            ],
            "a tall pane is halved top and bottom"
        );
    }

    #[rstest::rstest]
    fn add_beside_focus_halves_only_the_focused_pane() {
        // Given panes 1 and 2 side by side in a 200×48 area, pane 1 focused.
        let area = Rect::new(0, 0, 200, 48);
        let mut layout = halves(1);

        // When adding pane 3 beside the focus.
        layout.add_beside_focus(pane(3), area);

        // Then pane 1 is halved and pane 2 keeps its width.
        let widths: Vec<u16> = layout
            .panes(area)
            .iter()
            .map(|info| info.rect.width)
            .collect();
        assert_eq!(
            widths,
            vec![50, 50, 100],
            "only the focused pane should be halved"
        );
    }

    #[rstest::rstest]
    fn add_beside_focus_splits_a_wide_pane_left_and_right() {
        // Given a lone pane 1 in a 200×24 area.
        let area = Rect::new(0, 0, 200, 24);
        let mut layout = TileLayout::new(pane(1));

        // When adding pane 2 beside the focus.
        layout.add_beside_focus(pane(2), area);

        // Then pane 2 sits beside pane 1.
        assert_eq!(
            rects_in(&layout, area),
            vec![
                (pane(1), Rect::new(0, 0, 100, 24)),
                (pane(2), Rect::new(100, 0, 100, 24)),
            ],
            "a wide pane is halved left and right"
        );
    }

    #[rstest::rstest]
    fn add_beside_focus_stacks_a_small_pane() {
        // Given a lone pane 1 in a 50×18 area.
        let area = Rect::new(0, 0, 50, 18);
        let mut layout = TileLayout::new(pane(1));

        // When adding pane 2 beside the focus.
        layout.add_beside_focus(pane(2), area);

        // Then panes 1 and 2 are one stack.
        assert_eq!(
            stacked(&layout),
            vec![vec![pane(1), pane(2)]],
            "a pane too small to split is stacked with the new one"
        );
    }

    #[rstest::rstest]
    fn stack_layout_json_roundtrips() {
        // Given twelve panes tiled in stacked with pane 5 expanded and pane 1 focused.
        let mut layout = tiled_as(12, SwapLayout::Stacked);
        layout.focus_pane(pane(5));
        layout.focus_pane(pane(1));

        // When saving and loading it.
        let loaded = TileLayout::from_json(&layout.to_json(), pane(1)).ok();

        // Then every pane lands in the same place, with the same focus and titles.
        assert_eq!(
            loaded.map(|loaded| loaded.panes(AREA)),
            Some(layout.panes(AREA)),
            "the loaded stack should match the saved one"
        );
    }

    #[rstest::rstest]
    fn layout_json_names_a_stack() {
        // Given a stack of panes 2 and 3, pane 3 expanded.
        let layout = TileLayout::from_saved(stack_of(&[2, 3], 3), pane(3));

        // When saving it.
        let json = layout.to_json();

        // Then the stack and its expanded pane are named.
        assert_eq!(
            json, r#"{"stack":[2,3],"expanded":3}"#,
            "the saved stack's shape"
        );
    }

    #[rstest::rstest]
    fn from_json_expands_the_first_pane_when_expanded_isnt_in_the_stack() {
        // Given a saved stack whose expanded pane 9 isn't in it.
        let json = r#"{"split":"right","ratio":0.5,"first":{"pane":1},"second":{"stack":[2,3,4],"expanded":9}}"#;

        // When loading it focused on pane 1.
        let loaded = TileLayout::from_json(json, pane(1)).ok();

        // Then the stack's first pane is expanded.
        assert_eq!(
            loaded.as_ref().and_then(expanded),
            Some(pane(2)),
            "an unknown expanded pane should fall back to the first"
        );
    }

    #[rstest::rstest]
    #[case(r#"{"stack":[2],"expanded":2}"#, vec![1, 2])]
    #[case(r#"{"stack":[],"expanded":2}"#, vec![1])]
    fn from_json_loads_a_short_stack_as_plain_panes(
        #[case] stack: &str,
        #[case] expected: Vec<i64>,
    ) {
        // Given pane 1 split right from a stack of fewer than two panes.
        let json =
            format!(r#"{{"split":"right","ratio":0.5,"first":{{"pane":1}},"second":{stack}}}"#);

        // When loading it.
        let loaded = TileLayout::from_json(&json, pane(1)).ok();

        // Then it holds the panes plainly, none of them a title.
        assert_eq!(
            loaded.map(|loaded| (
                loaded.pane_ids(),
                loaded.panes(AREA).iter().any(|info| info.collapsed)
            )),
            Some((expected.into_iter().map(pane).collect(), false)),
            "a short stack should load as plain panes"
        );
    }

    #[rstest::rstest]
    #[case(3, 4)]
    #[case(4, 3)]
    fn closing_the_expanded_pane_expands_its_neighbour(#[case] closed: i64, #[case] after: i64) {
        // Given pane 1 beside a stack of 2, 3 and 4 with pane `closed` expanded, pane 1 focused.
        let mut layout = main_and_stack(closed, 1);

        // When closing pane `closed`.
        layout.close_pane(pane(closed));

        // Then the pane after it, or before it when it was last, is expanded.
        assert_eq!(
            expanded(&layout),
            Some(pane(after)),
            "closing the expanded pane should expand its neighbour"
        );
    }

    #[rstest::rstest]
    #[case::left(1, 0.8)]
    #[case::right(2, 0.2)]
    fn grow_moves_a_side_by_side_border_30_percent_of_the_tab(
        #[case] focus: i64,
        #[case] expected: f32,
    ) {
        // Given two halves of a 100-column area, pane `focus` focused.
        let mut layout = halves(focus);

        // When growing it.
        let grew = layout.grow_focused(AREA);

        // Then the border moves 30 columns away from it.
        assert!(
            grew == Grow::Grew && ratio(&layout).is_some_and(|r| (r - expected).abs() < 1e-6),
            "the border should move to {expected}, got {grew:?} at {:?}",
            ratio(&layout)
        );
    }

    #[rstest::rstest]
    fn grow_tries_the_neighbours_above_first() {
        // Given panes 1 and 2 side by side above pane 3, pane 3 focused.
        let mut layout = TileLayout::from_saved(
            split(
                Split::Down,
                0.5,
                split(Split::Right, 0.5, Node::Pane(pane(1)), Node::Pane(pane(2))),
                Node::Pane(pane(3)),
            ),
            pane(3),
        );

        // When growing it.
        layout.grow_focused(AREA);

        // Then pane 3 grows 12 rows up: 30% of the 40-row tab.
        let height = layout
            .panes(AREA)
            .into_iter()
            .find(|info| info.id == pane(3))
            .map(|info| info.rect.height);
        assert_eq!(height, Some(32), "pane 3 should grow up first");
    }

    #[rstest::rstest]
    fn grow_skips_a_neighbour_wider_than_the_pane() {
        // Given panes 1 and 2 side by side above pane 3, pane 1 focused.
        let mut layout = TileLayout::from_saved(
            split(
                Split::Down,
                0.5,
                split(Split::Right, 0.5, Node::Pane(pane(1)), Node::Pane(pane(2))),
                Node::Pane(pane(3)),
            ),
            pane(1),
        );

        // When growing it.
        layout.grow_focused(AREA);

        // Then pane 1 grows right into pane 2, not down into the wider pane 3.
        let rect = layout
            .panes(AREA)
            .into_iter()
            .find(|info| info.id == pane(1))
            .map(|info| info.rect);
        assert_eq!(
            rect,
            Some(Rect::new(0, 0, 80, 20)),
            "pane 1 should skip pane 3 and grow right"
        );
    }

    #[rstest::rstest]
    fn grow_stacks_when_30_percent_does_not_fit() {
        // Given panes 1 and 2 split right at 0.8, pane 1 focused.
        let mut layout = TileLayout::from_saved(
            split(Split::Right, 0.8, Node::Pane(pane(1)), Node::Pane(pane(2))),
            pane(1),
        );

        // When growing pane 1, which would leave pane 2 no columns.
        let grew = layout.grow_focused(AREA);

        // Then the two become one stack showing pane 1.
        assert_eq!(
            (grew, layout.root),
            (Grow::Stacked, stack_of(&[1, 2], 1)),
            "a step that doesn't fit should stack the neighbour"
        );
    }

    #[rstest::rstest]
    fn grow_stacks_only_the_neighbour_touching_the_pane() {
        // Given panes 1 and 2 (15 rows each) above pane 3 (10 rows), pane 3
        // focused.
        let mut layout = TileLayout::from_saved(
            split(
                Split::Down,
                0.75,
                split(Split::Down, 0.5, Node::Pane(pane(1)), Node::Pane(pane(2))),
                Node::Pane(pane(3)),
            ),
            pane(3),
        );

        // When growing pane 3, which would leave pane 2 3 rows.
        layout.grow_focused(AREA);

        // Then panes 2 and 3 are one stack below pane 1, which keeps its 15 rows.
        assert_eq!(
            layout.root,
            split(
                Split::Down,
                0.375,
                Node::Pane(pane(1)),
                stack_of(&[2, 3], 3)
            ),
            "only the pane touching pane 3 should join its stack"
        );
    }

    #[rstest::rstest]
    fn grow_stacks_the_neighbour_below_under_the_pane() {
        // Given pane 1 (30 rows) above panes 2 and 3 (5 rows each), pane 1
        // focused.
        let mut layout = TileLayout::from_saved(
            split(
                Split::Down,
                0.75,
                Node::Pane(pane(1)),
                split(Split::Down, 0.5, Node::Pane(pane(2)), Node::Pane(pane(3))),
            ),
            pane(1),
        );

        // When growing pane 1, which would leave pane 2 no rows.
        layout.grow_focused(AREA);

        // Then panes 1 and 2 are one stack above pane 3, which keeps its 5 rows.
        assert_eq!(
            layout.root,
            split(
                Split::Down,
                0.875,
                stack_of(&[1, 2], 1),
                Node::Pane(pane(3))
            ),
            "the pane below should join pane 1's stack"
        );
    }

    #[rstest::rstest]
    fn grow_moves_a_stacks_border() {
        // Given a stack of 1 and 2 beside pane 3, pane 1 focused.
        let mut layout = TileLayout::from_saved(
            split(Split::Right, 0.5, stack_of(&[1, 2], 1), Node::Pane(pane(3))),
            pane(1),
        );

        // When growing pane 1.
        layout.grow_focused(AREA);

        // Then the border between the stack and pane 3 moves 30 columns right.
        assert!(
            ratio(&layout).is_some_and(|r| (r - 0.8).abs() < 1e-6),
            "the stack's border should move to 0.8, got {:?}",
            ratio(&layout)
        );
    }

    #[rstest::rstest]
    #[case::thirty_percent(0.5, 0.2)]
    #[case::five_percent_when_thirty_does_not_fit(0.25, 0.2)]
    fn shrink_moves_the_border_towards_the_pane(#[case] start: f32, #[case] expected: f32) {
        // Given panes 1 and 2 split right at `start`, pane 1 focused.
        let mut layout = TileLayout::from_saved(
            split(
                Split::Right,
                start,
                Node::Pane(pane(1)),
                Node::Pane(pane(2)),
            ),
            pane(1),
        );

        // When shrinking pane 1.
        layout.shrink_focused(AREA);

        // Then the border lands at `expected`.
        assert!(
            ratio(&layout).is_some_and(|r| (r - expected).abs() < 1e-6),
            "the border should move to {expected}, got {:?}",
            ratio(&layout)
        );
    }

    #[rstest::rstest]
    #[case::shown_last_breaks_out_the_top(
        3,
        split(Split::Down, 1.0 / 3.0, Node::Pane(pane(1)), stack_of(&[2, 3], 3))
    )]
    #[case::shown_first_breaks_out_the_bottom(
        1,
        split(Split::Down, 2.0 / 3.0, stack_of(&[1, 2], 1), Node::Pane(pane(3)))
    )]
    fn shrink_breaks_a_pane_out_of_the_stack(#[case] shown: i64, #[case] expected: Node) {
        // Given a stack of 1, 2 and 3 showing pane `shown`.
        let mut layout = TileLayout::from_saved(stack_of(&[1, 2, 3], shown), pane(shown));

        // When shrinking.
        layout.shrink_focused(AREA);

        // Then one pane leaves the stack with a third of its rows.
        assert_eq!(layout.root, expected, "Cmd - should break a pane out");
    }

    /// Whether the root split's ratio is `expected`, give or take a rounding.
    fn ratio_is(layout: &TileLayout, expected: f32) -> bool {
        ratio(layout).is_some_and(|ratio| (ratio - expected).abs() < 1e-3)
    }

    #[rstest::rstest]
    fn resize_side_out_moves_the_border_5_percent() {
        // Given panes 1 and 2 side by side, pane 1 focused.
        let mut layout = halves(1);

        // When pushing pane 1's right border out.
        layout.resize_side(AREA, NavDirection::Right, true);

        // Then the border sits 5% of the tab further right.
        assert!(
            ratio_is(&layout, 0.55),
            "the right border should move right 5%, got {:?}",
            ratio(&layout)
        );
    }

    #[rstest::rstest]
    fn resize_side_in_moves_the_border_back_5_percent() {
        // Given panes 1 and 2 side by side, pane 1 focused.
        let mut layout = halves(1);

        // When pulling pane 1's right border in.
        layout.resize_side(AREA, NavDirection::Right, false);

        // Then the border sits 5% of the tab further left.
        assert!(
            ratio_is(&layout, 0.45),
            "the right border should move left 5%, got {:?}",
            ratio(&layout)
        );
    }

    #[rstest::rstest]
    fn resize_side_growing_toward_the_tabs_edge_pulls_the_opposite_border_in() {
        // Given panes 1 and 2 side by side, pane 1 focused, its left edge the tab's.
        let mut layout = halves(1);

        // When pushing pane 1's left border out.
        layout.resize_side(AREA, NavDirection::Left, true);

        // Then its right border moves in 5% instead.
        assert!(
            ratio_is(&layout, 0.45),
            "a grow at the tab's edge should pull the opposite border in, got {:?}",
            ratio(&layout)
        );
    }

    #[rstest::rstest]
    fn resize_side_shrinking_toward_the_tabs_edge_changes_nothing() {
        // Given panes 1 and 2 side by side, pane 1 focused, its left edge the tab's.
        let mut layout = halves(1);
        let before = layout.clone();

        // When pulling pane 1's left border in.
        layout.resize_side(AREA, NavDirection::Left, false);

        // Then the tree is unchanged.
        assert_eq!(layout, before, "the tab's edge never moves");
    }

    #[rstest::rstest]
    fn resize_side_never_leaves_a_pane_under_the_minimum() {
        // Given panes 1 and 2 side by side in a 10-column tab, pane 1 focused.
        let mut layout = halves(1);
        let before = layout.clone();

        // When pushing pane 1's right border out (pane 2 would get 4 columns).
        layout.resize_side(Rect::new(0, 0, 10, 40), NavDirection::Right, true);

        // Then the tree is unchanged.
        assert_eq!(layout, before, "no pane goes under the minimum size");
    }

    #[rstest::rstest]
    fn swap_toward_swaps_the_two_panes_ids() {
        // Given panes 1 and 2 side by side, pane 1 focused.
        let mut layout = halves(1);
        let cells: Vec<Rect> = rects(&layout).into_iter().map(|(_, rect)| rect).collect();

        // When moving pane 1 right.
        layout.swap_toward(AREA, NavDirection::Right);

        // Then pane 2 takes the left cell and pane 1 the right, sizes unchanged.
        let expected: Vec<_> = [pane(2), pane(1)].into_iter().zip(cells).collect();
        assert_eq!(rects(&layout), expected, "the two panes should trade cells");
    }

    #[rstest::rstest]
    fn swap_toward_keeps_the_focus_on_the_moved_pane() {
        // Given panes 1 and 2 side by side, pane 1 focused.
        let mut layout = halves(1);

        // When moving pane 1 right.
        layout.swap_toward(AREA, NavDirection::Right);

        // Then pane 1 still has the focus.
        assert_eq!(
            layout.focused(),
            pane(1),
            "the focus follows the moved pane"
        );
    }

    #[rstest::rstest]
    fn swap_toward_past_the_tabs_edge_changes_nothing() {
        // Given panes 1 and 2 side by side, pane 1 focused on the left edge.
        let mut layout = halves(1);
        let before = layout.clone();

        // When moving pane 1 left.
        let moved = layout.swap_toward(AREA, NavDirection::Left);

        // Then nothing moved.
        assert!(
            !moved && layout == before,
            "a move past the tab's edge should change nothing"
        );
    }

    #[rstest::rstest]
    fn swap_in_order_forward_on_the_last_pane_swaps_with_the_first() {
        // Given panes 1 to 3 tiled, pane 3 focused.
        let mut layout = tiled(3);

        // When moving pane 3 to the next pane.
        layout.swap_in_order(true);

        // Then it wraps and swaps with pane 1.
        assert_eq!(
            layout.pane_ids(),
            vec![pane(3), pane(2), pane(1)],
            "next on the last pane should swap with the first"
        );
    }

    #[rstest::rstest]
    fn swap_in_order_backward_on_the_first_pane_swaps_with_the_last() {
        // Given panes 1 to 3 tiled, pane 1 focused.
        let mut layout = tiled(3);
        layout.focus_pane(pane(1));

        // When moving pane 1 to the previous pane.
        layout.swap_in_order(false);

        // Then it wraps and swaps with pane 3.
        assert_eq!(
            layout.pane_ids(),
            vec![pane(3), pane(2), pane(1)],
            "previous on the first pane should swap with the last"
        );
    }

    #[rstest::rstest]
    fn swap_in_order_on_a_lone_pane_changes_nothing() {
        // Given a layout of pane 1.
        let mut layout = TileLayout::new(pane(1));

        // When moving it to the next pane.
        let moved = layout.swap_in_order(true);

        // Then nothing moved.
        assert!(!moved, "a lone pane has nothing to swap with");
    }

    #[rstest::rstest]
    fn swap_toward_down_in_a_stack_swaps_with_the_next_stacked_pane() {
        // Given pane 1 beside a stack of 2, 3 and 4, pane 2 shown and focused.
        let mut layout = main_and_stack(2, 2);

        // When moving pane 2 down.
        layout.swap_toward(AREA, NavDirection::Down);

        // Then pane 2 trades places with pane 3 in the stack.
        assert_eq!(
            stacked(&layout),
            vec![vec![pane(3), pane(2), pane(4)]],
            "a move down in a stack should swap with the next stacked pane"
        );
    }

    #[rstest::rstest]
    fn swap_toward_down_in_a_stack_keeps_the_moved_pane_shown() {
        // Given pane 1 beside a stack of 2, 3 and 4, pane 2 shown and focused.
        let mut layout = main_and_stack(2, 2);

        // When moving pane 2 down.
        layout.swap_toward(AREA, NavDirection::Down);

        // Then pane 2 is still the stack's shown pane.
        assert_eq!(
            expanded(&layout),
            Some(pane(2)),
            "the moved pane should stay shown in the stack"
        );
    }

    #[rstest::rstest]
    fn swap_into_a_stack_shows_the_moved_pane_in_the_stack() {
        // Given pane 1 focused beside a stack of 2, 3 and 4, pane 2 shown.
        let mut layout = main_and_stack(2, 1);

        // When moving pane 1 right.
        layout.swap_toward(AREA, NavDirection::Right);

        // Then pane 1 takes pane 2's place in the stack and is shown.
        assert_eq!(
            (stacked(&layout), expanded(&layout)),
            (vec![vec![pane(1), pane(3), pane(4)]], Some(pane(1))),
            "the moved pane should take the stack's shown slot"
        );
    }

    #[rstest::rstest]
    fn swap_into_a_stack_puts_the_shown_pane_in_the_moved_panes_cell() {
        // Given pane 1 focused beside a stack of 2, 3 and 4, pane 2 shown.
        let mut layout = main_and_stack(2, 1);

        // When moving pane 1 right.
        layout.swap_toward(AREA, NavDirection::Right);

        // Then pane 2 sits on the left half.
        assert_eq!(
            info(&layout, 2).map(|info| info.rect),
            Some(Rect::new(0, 0, 50, 40)),
            "the stack's shown pane should land in the moved pane's cell"
        );
    }

    #[rstest::rstest]
    #[case::lone(TileLayout::new(pane(1)))]
    #[case::whole_stack(TileLayout::from_saved(stack_of(&[1, 2], 1), pane(1)))]
    fn grow_focused_without_a_neighbour_is_stuck(#[case] mut layout: TileLayout) {
        // Given a layout with no split.

        // When growing the focused pane.
        let grew = layout.grow_focused(AREA);

        // Then it's stuck.
        assert_eq!(
            grew,
            Grow::Stuck,
            "with no neighbour there is nothing to grow"
        );
    }

    #[rstest::rstest]
    fn grow_focused_keeps_unstacked_panes_at_least_five_by_five(
        #[values(2, 3, 4, 5, 6, 7, 8, 9, 10)] n: i64,
    ) {
        // Given `n` tiled panes, every one at least 33 columns by 10 rows.
        let fresh = |focus: i64| {
            let mut layout = tiled(n);
            layout.focus_pane(pane(focus));
            layout
        };

        // When growing each focused pane, on its own fresh layout, until stuck.
        let mut violations = Vec::new();
        for focus in 1..=n {
            let mut layout = fresh(focus);
            for step in 0..200 {
                if layout.grow_focused(AREA) == Grow::Stuck {
                    break;
                }
                let stacks = layout.stacks(AREA);
                violations.extend(
                    layout
                        .panes(AREA)
                        .into_iter()
                        .filter(|info| !stacks.iter().any(|stack| stack.panes.contains(&info.id)))
                        .filter(|info| info.rect.width < 5 || info.rect.height < 5)
                        .map(|info| (n, focus, step, info.id, info.rect)),
                );
            }
        }

        // Then no step left a pane outside a stack under 5×5.
        assert!(
            violations.is_empty(),
            "a grow step left panes under 5×5: {violations:?}"
        );
    }
}
