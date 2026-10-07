// Adapted from herdr (https://github.com/herdrdev/herdr), src/layout.rs at
// commit 3d9d2b18, licensed under the Apache License, Version 2.0. Modified
// for orb: pane ids come from the caller instead of a global counter; splits
// are `Split::{Right, Down}`; resizing moves the focused pane's own split by
// whole cells; the tree saves to and loads from JSON; herdr's borders,
// scrollbar lanes, swap, insert and drag-resize code is left out.

//! A tab's panes as a binary tree of right and down splits: where each pane
//! lands in an area, and which pane is the neighbour in a direction.

use std::cmp::Reverse;

use error_stack::{Report, ResultExt};
use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};
use wherror::Error;

use crate::feat::sessions::state::PaneId;

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

/// Where a pane lands in an area, and whether it has the focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneInfo {
    pub id: PaneId,
    pub rect: Rect,
    pub is_focused: bool,
}

/// A node of the tree: one pane, or a split holding two subtrees.
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

    /// A layout of the tree `root`, focused on `focus`.
    pub fn from_saved(root: Node, focus: PaneId) -> Self {
        Self {
            root,
            focus,
            prev_focus: None,
        }
    }

    /// Move focus, recording the pane being left. No-op when focus is unchanged.
    fn set_focus(&mut self, id: PaneId) {
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

    /// Every pane id, in tree order.
    pub fn pane_ids(&self) -> Vec<PaneId> {
        let mut ids = Vec::new();
        collect_ids(&self.root, &mut ids);
        ids
    }

    /// Splits the focused pane `split`, the new pane `id` going right after it
    /// and taking the focus. The run of `split` splits the two panes join is
    /// re-divided into equal shares.
    pub fn split_focused(&mut self, split: Split, id: PaneId) {
        if let Some(node) = find_pane_mut(&mut self.root, self.focus) {
            *node = split_node(self.focus, split, id, 0.5);
            even_run(&mut self.root, split, id);
            self.set_focus(id);
        }
    }

    /// Rebuilds the tree from the template for its pane count, panes in their
    /// current tree order. Focus and its history stay.
    pub fn tile(&mut self) {
        if let Some(root) = template(&self.pane_ids()) {
            self.root = root;
        }
    }

    /// Adds pane `id` last in reading order, re-tiles, and focuses it.
    pub fn add_tiled(&mut self, id: PaneId) {
        let ids = {
            let mut ids = self.pane_ids();
            ids.push(id);
            ids
        };
        if let Some(root) = template(&ids) {
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

    /// Moves the focused pane's own split border (its parent split) by
    /// `cells` columns, or rows for a `Down` split: towards the sibling when
    /// `grow`, away from it otherwise. The ratio stays between 0.1 and 0.9. A
    /// lone pane has no split, and nothing changes. Returns whether it changed.
    pub fn resize_focused(&mut self, grow: bool, cells: u16, area: Rect) -> bool {
        let Some((ratio, focused_first, extent)) = parent_split(&mut self.root, area, self.focus)
        else {
            return false;
        };
        if extent == 0 {
            return false;
        }
        let delta = f32::from(cells) / f32::from(extent);
        let resized = if grow == focused_first {
            *ratio + delta
        } else {
            *ratio - delta
        }
        .clamp(0.1, 0.9);
        let changed = (resized - *ratio).abs() > f32::EPSILON;
        *ratio = resized;
        changed
    }

    /// The tree as JSON: `{"pane":7}` or
    /// `{"split":"right","ratio":0.5,"first":…,"second":…}`.
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
        let mut layout = Self::from_saved(Node::from(saved), focus);
        if !layout.pane_ids().contains(&focus)
            && let Some(first) = layout.pane_ids().first()
        {
            layout.focus = *first;
        }
        Ok(layout)
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
        }
    }
}

impl From<Saved> for Node {
    fn from(saved: Saved) -> Self {
        match saved {
            Saved::Pane { pane } => Self::Pane(pane),
            Saved::Split {
                split,
                ratio,
                first,
                second,
            } => Self::Split {
                split,
                ratio: valid_split_ratio(ratio),
                first: Box::new(Self::from(*first)),
                second: Box::new(Self::from(*second)),
            },
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

/// Column sizes for `n` panes, first column first; the flag says the last
/// column is a stack (`n > 10`).
fn columns(n: usize) -> (Vec<usize>, bool) {
    match n {
        0 => (vec![], false),
        1 => (vec![1], false),
        11.. => (vec![1, n - 1], true),
        _ => {
            let cols = {
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
            };
            (cols, false)
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

/// The template tree for `ids` in reading order: columns from [`columns`],
/// filled top to bottom, first column first. `None` for no ids.
fn template(ids: &[PaneId]) -> Option<Node> {
    let (cols, _stacked) = columns(ids.len());
    let mut rest = ids.iter().copied().map(Node::Pane);
    let columns = cols
        .iter()
        .filter_map(|&k| even_chain(rest.by_ref().take(k).collect(), Split::Down))
        .collect();
    even_chain(columns, Split::Right)
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
    }
}

fn collect_panes(node: &Node, area: Rect, focus: PaneId, result: &mut Vec<PaneInfo>) {
    match node {
        Node::Pane(id) => result.push(PaneInfo {
            id: *id,
            rect: area,
            is_focused: *id == focus,
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
    }
}

fn collect_ids(node: &Node, ids: &mut Vec<PaneId>) {
    match node {
        Node::Pane(id) => ids.push(*id),
        Node::Split { first, second, .. } => {
            collect_ids(first, ids);
            collect_ids(second, ids);
        }
    }
}

fn find_pane_mut(node: &mut Node, target: PaneId) -> Option<&mut Node> {
    match node {
        Node::Pane(id) if *id == target => Some(node),
        Node::Pane(_) => None,
        Node::Split { first, second, .. } => {
            find_pane_mut(first, target).or_else(|| find_pane_mut(second, target))
        }
    }
}

/// The split directly holding pane `target`, laid over `area`: its ratio,
/// whether `target` is its first subtree, and the cells it divides (width
/// for a right split, height for a down split).
fn parent_split(node: &mut Node, area: Rect, target: PaneId) -> Option<(&mut f32, bool, u16)> {
    let Node::Split {
        split,
        ratio,
        first,
        second,
    } = node
    else {
        return None;
    };
    let extent = match split {
        Split::Right => area.width,
        Split::Down => area.height,
    };
    if matches!(**first, Node::Pane(id) if id == target) {
        return Some((ratio, true, extent));
    }
    if matches!(**second, Node::Pane(id) if id == target) {
        return Some((ratio, false, extent));
    }
    let (a, b) = split_rect(area, *split, *ratio);
    parent_split(first, a, target).or_else(|| parent_split(second, b, target))
}

fn split_node(target: PaneId, split: Split, new_id: PaneId, ratio: f32) -> Node {
    Node::Split {
        split,
        ratio,
        first: Box::new(Node::Pane(target)),
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

    use super::{NavDirection, Node, PaneInfo, Split, TileLayout, columns, find_in_direction};
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
            Node::Pane(_) => None,
        }
    }

    /// A layout of panes 1 to `n`, each added with `add_tiled`.
    fn tiled(n: i64) -> TileLayout {
        let mut layout = TileLayout::new(pane(1));
        for id in 2..=n {
            layout.add_tiled(pane(id));
        }
        layout
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
    fn resize_grows_the_focused_first_pane_by_whole_cells() {
        // Given two halves of a 100-column area, the left one focused.
        let mut layout = halves(1);

        // When growing it by 4 cells.
        layout.resize_focused(true, 4, AREA);

        // Then the split moves 4 columns right.
        assert!(
            ratio(&layout).is_some_and(|ratio| (ratio - 0.54).abs() < 1e-6),
            "ratio should be 0.54, got {:?}",
            ratio(&layout)
        );
    }

    #[rstest::rstest]
    fn resize_grows_the_focused_second_pane_towards_its_sibling() {
        // Given two halves of a 100-column area, the right one focused.
        let mut layout = halves(2);

        // When growing it by 4 cells.
        layout.resize_focused(true, 4, AREA);

        // Then the split moves 4 columns left.
        assert!(
            ratio(&layout).is_some_and(|ratio| (ratio - 0.46).abs() < 1e-6),
            "ratio should be 0.46, got {:?}",
            ratio(&layout)
        );
    }

    #[rstest::rstest]
    #[case(0.9, true)]
    #[case(0.1, false)]
    fn resize_stops_at_the_ratio_bounds(#[case] start: f32, #[case] grow: bool) {
        // Given a split at its bound, the left pane focused.
        let mut layout = TileLayout::from_saved(
            split(
                Split::Right,
                start,
                Node::Pane(pane(1)),
                Node::Pane(pane(2)),
            ),
            pane(1),
        );

        // When resizing past the bound.
        let changed = layout.resize_focused(grow, 4, AREA);

        // Then the ratio stays at the bound.
        assert_eq!(
            (changed, ratio(&layout)),
            (false, Some(start)),
            "the ratio should stay at its bound"
        );
    }

    #[rstest::rstest]
    fn resize_of_a_lone_pane_changes_nothing() {
        // Given a layout of one pane.
        let mut layout = TileLayout::new(pane(1));

        // When growing it.
        let changed = layout.resize_focused(true, 4, AREA);

        // Then nothing changed.
        assert!(!changed, "a lone pane has no split to move");
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
        assert_eq!(columns(n), (expected, false), "columns for {n} panes");
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
        layout.add_tiled(pane(6));

        // Then two panes sit in the first column and four in the second.
        assert_eq!(shape(&layout, AREA), vec![2, 4], "six panes tile [2][4]");
    }

    #[rstest::rstest]
    fn add_tiled_focuses_the_new_pane() {
        // Given five tiled panes.
        let mut layout = tiled(5);

        // When adding pane 6.
        layout.add_tiled(pane(6));

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
        layout.resize_focused(true, 10, AREA);

        // When adding pane 4.
        layout.add_tiled(pane(4));

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
}
