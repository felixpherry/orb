//! A session's floating panes, drawn over all of its tabs.
//!
//! Floats keep their places as shares of the tab body, so they grow and
//! shrink with the window. A new float goes to the staggered spot: half the
//! body, centred, moved two cells down and right past any float already
//! there. The top float has the focus while the floats are shown.

use ratatui::layout::Rect;

use super::tree::NavDirection;
use crate::feat::sessions::state::PaneId;

/// Smallest float width, in cells (capped at the body).
const MIN_WIDTH: u16 = 10;
/// Smallest float height, in cells (capped at the body).
const MIN_HEIGHT: u16 = 4;

/// A float's place as shares (0..=1) of the tab body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloatRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl FloatRect {
    /// `cells` as shares of `body`; the whole body when `body` is empty.
    pub fn from_cells(cells: Rect, body: Rect) -> Self {
        if body.is_empty() {
            Self {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            }
        } else {
            let (bw, bh) = (f32::from(body.width), f32::from(body.height));
            Self {
                x: f32::from(cells.x.saturating_sub(body.x)) / bw,
                y: f32::from(cells.y.saturating_sub(body.y)) / bh,
                width: f32::from(cells.width) / bw,
                height: f32::from(cells.height) / bh,
            }
        }
    }

    /// Back to cells in `body`: rounded, at least the smallest float size (or
    /// the body when smaller), at most the body, slid left and up to stay inside.
    pub fn to_cells(self, body: Rect) -> Rect {
        let (bw, bh) = (f32::from(body.width), f32::from(body.height));
        let width = ((self.width * bw).round().max(0.0) as u16)
            .clamp(MIN_WIDTH.min(body.width), body.width);
        let height = ((self.height * bh).round().max(0.0) as u16)
            .clamp(MIN_HEIGHT.min(body.height), body.height);
        let x = body
            .x
            .saturating_add((self.x * bw).round().max(0.0) as u16)
            .min(body.right() - width);
        let y = body
            .y
            .saturating_add((self.y * bh).round().max(0.0) as u16)
            .min(body.bottom() - height);
        Rect::new(x, y, width, height)
    }
}

/// One float: its pane and place.
#[derive(Debug, Clone, PartialEq)]
pub struct Float {
    pane: PaneId,
    rect: FloatRect,
}

/// A session's floats, bottom first (the last is on top and has the focus),
/// and whether they are shown.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Floats {
    panes: Vec<Float>,
    shown: bool,
}

impl Floats {
    /// Whether the session has no floats.
    pub fn is_empty(&self) -> bool {
        self.panes.is_empty()
    }

    /// The top float while the floats are shown.
    pub fn focused(&self) -> Option<PaneId> {
        self.panes
            .last()
            .filter(|_| self.shown)
            .map(|float| float.pane)
    }

    /// Whether `pane` is one of the floats.
    pub fn holds(&self, pane: PaneId) -> bool {
        self.panes.iter().any(|float| float.pane == pane)
    }

    /// Every float's pane, shown or not, bottom first.
    pub fn pane_ids(&self) -> impl Iterator<Item = PaneId> + '_ {
        self.panes.iter().map(|float| float.pane)
    }

    /// Each shown float's pane and frame in `body`, bottom first; none while hidden.
    pub fn visible(&self, body: Rect) -> impl Iterator<Item = (PaneId, Rect)> + '_ {
        let panes: &[Float] = if self.shown { &self.panes } else { &[] };
        panes
            .iter()
            .map(move |float| (float.pane, float.rect.to_cells(body)))
    }

    /// Puts `pane` on top in the staggered spot for `body` and shows the floats.
    pub fn add(&mut self, pane: PaneId, body: Rect) {
        let spot = {
            let taken: Vec<Rect> = self
                .panes
                .iter()
                .map(|float| float.rect.to_cells(body))
                .collect();
            staggered_spot(body, &taken)
        };
        self.panes.push(Float {
            pane,
            rect: FloatRect::from_cells(spot, body),
        });
        self.shown = true;
    }

    /// Shows or hides the floats; nothing with no floats. Whether it changed.
    pub fn toggle(&mut self) -> bool {
        if self.panes.is_empty() {
            return false;
        }
        self.shown = !self.shown;
        true
    }

    /// Takes `pane` out; hides the floats when none are left. Whether it was a float.
    pub fn remove(&mut self, pane: PaneId) -> bool {
        let Some(index) = self.panes.iter().position(|float| float.pane == pane) else {
            return false;
        };
        self.panes.remove(index);
        if self.panes.is_empty() {
            self.shown = false;
        }
        true
    }

    /// Moves `pane` to the top.
    pub fn raise(&mut self, pane: PaneId) {
        if let Some(index) = self.panes.iter().position(|float| float.pane == pane) {
            let float = self.panes.remove(index);
            self.panes.push(float);
        }
    }

    /// The float `nav` of the top one in `body`: among floats whose top-left
    /// corner lies that way and whose rows (left/right) or columns (up/down)
    /// overlap the top float's, the nearest one.
    pub fn neighbour(&self, nav: NavDirection, body: Rect) -> Option<PaneId> {
        let (top, rest) = self.panes.split_last()?;
        let from = top.rect.to_cells(body);
        let rows = |r: Rect| r.y < from.bottom() && from.y < r.bottom();
        let cols = |r: Rect| r.x < from.right() && from.x < r.right();
        let candidates = rest
            .iter()
            .map(|float| (float.pane, float.rect.to_cells(body)));
        match nav {
            NavDirection::Left => candidates
                .filter(|(_, r)| r.x < from.x && rows(*r))
                .max_by_key(|(_, r)| (r.x, r.y)),
            NavDirection::Right => candidates
                .filter(|(_, r)| r.x > from.x && rows(*r))
                .min_by_key(|(_, r)| (r.x, r.y)),
            NavDirection::Up => candidates
                .filter(|(_, r)| r.y < from.y && cols(*r))
                .max_by_key(|(_, r)| (r.y, r.x)),
            NavDirection::Down => candidates
                .filter(|(_, r)| r.y > from.y && cols(*r))
                .min_by_key(|(_, r)| (r.y, r.x)),
        }
        .map(|(pane, _)| pane)
    }
}

/// Where a new float goes among the `taken` frames: half size and centred,
/// walking two cells down and right; then third size in each corner (top-left,
/// top-right, bottom-left, bottom-right) with the same walk. The first spot
/// inside `body`, at least the smallest float size and not identical to a
/// taken frame wins; the whole body when none does.
fn staggered_spot(body: Rect, taken: &[Rect]) -> Rect {
    let (left, top) = (i32::from(body.x), i32::from(body.y));
    let (right, bottom) = (i32::from(body.right()), i32::from(body.bottom()));
    let (w, h) = (i32::from(body.width), i32::from(body.height));
    let (half_w, half_h) = (w / 2, h / 2);
    let (third_w, third_h) = (w / 3, h / 3);
    // Each walk: top-left corner and size of the spot at offset `o`.
    let walks: [&dyn Fn(i32) -> (i32, i32, i32, i32); 5] = [
        &|o| {
            (
                left + (w + 2) / 4 + o,
                top + (h + 2) / 4 + o,
                half_w,
                half_h,
            )
        },
        &|o| (left + 2 + o, top + 2 + o, third_w, third_h),
        &|o| (right - third_w - 2 - o, top + 2 + o, third_w, third_h),
        &|o| (left + 2 + o, bottom - third_h - 2 - o, third_w, third_h),
        &|o| {
            (
                right - third_w - 2 - o,
                bottom - third_h - 2 - o,
                third_w,
                third_h,
            )
        },
    ];
    let inside = |(x, y, width, height): (i32, i32, i32, i32)| {
        let fits = width >= i32::from(MIN_WIDTH)
            && height >= i32::from(MIN_HEIGHT)
            && x >= left
            && y >= top
            && x + width <= right
            && y + height <= bottom;
        fits.then(|| Rect::new(x as u16, y as u16, width as u16, height as u16))
    };
    walks
        .iter()
        .find_map(|walk| {
            (0..)
                .step_by(2)
                .map_while(|o| inside(walk(o)))
                .find(|spot| !taken.contains(spot))
        })
        .unwrap_or(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: Rect = Rect::new(0, 0, 80, 24);

    #[rstest::rstest]
    fn first_float_takes_the_centred_half_of_the_body() {
        // Given no floats.
        let mut floats = Floats::default();

        // When adding a float on an 80×24 body.
        floats.add(PaneId(1), BODY);

        // Then its frame is the centred half of the body.
        let frames: Vec<Rect> = floats.visible(BODY).map(|(_, r)| r).collect();
        assert_eq!(
            frames,
            vec![Rect::new(20, 6, 40, 12)],
            "first float should be centred at half size"
        );
    }

    #[rstest::rstest]
    fn second_float_sits_two_cells_down_and_right() {
        // Given one float in the centred spot.
        let mut floats = Floats::default();
        floats.add(PaneId(1), BODY);

        // When adding another float.
        floats.add(PaneId(2), BODY);

        // Then the new top frame is two cells down and right.
        let top = floats.visible(BODY).last().map(|(_, r)| r);
        assert_eq!(
            top,
            Some(Rect::new(22, 8, 40, 12)),
            "second float should be staggered by two cells"
        );
    }

    #[rstest::rstest]
    fn float_frame_doubles_when_the_body_doubles() {
        // Given a float added on an 80×24 body.
        let mut floats = Floats::default();
        floats.add(PaneId(1), BODY);

        // When placing it on a 160×48 body.
        let frame = floats
            .visible(Rect::new(0, 0, 160, 48))
            .next()
            .map(|(_, r)| r);

        // Then its frame doubles.
        assert_eq!(
            frame,
            Some(Rect::new(40, 12, 80, 24)),
            "float frame should scale with the body"
        );
    }

    #[rstest::rstest]
    fn float_on_a_tiny_body_fits_inside_it() {
        // Given a body smaller than the smallest float.
        let body = Rect::new(3, 2, 6, 3);
        let mut floats = Floats::default();

        // When adding a float.
        floats.add(PaneId(1), body);

        // Then its frame is the whole body.
        let frame = floats.visible(body).next().map(|(_, r)| r);
        assert_eq!(
            frame,
            Some(body),
            "float on a tiny body should be clamped to the body"
        );
    }

    #[rstest::rstest]
    fn neighbour_up_is_the_float_above_the_top_one() {
        // Given two staggered floats, 2 on top and two cells below 1.
        let mut floats = Floats::default();
        floats.add(PaneId(1), BODY);
        floats.add(PaneId(2), BODY);

        // When looking for the float above the top one.
        let above = floats.neighbour(NavDirection::Up, BODY);

        // Then it is float 1.
        assert_eq!(above, Some(PaneId(1)), "the float above should be found");
    }
}
