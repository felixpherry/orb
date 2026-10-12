//! The panes' kitty images on the outer screen, kept in step with each frame.
//!
//! Each frame, what the panes' programs sent for kitty (images, frame edits,
//! frees) goes out first, a drawn pane's with its cells and a hidden pane's
//! as it arrives. Then each drawn pane's images are placed where its content
//! shows them: new or moved ones placed, ones no longer shown taken off. A
//! pane that isn't drawn has its images taken off the screen but kept in
//! kitty, so showing it again brings them straight back; a pane that is gone
//! has its images freed. Images sit in kitty's deepest layer, under every
//! cell with a background of its own, so everything orb draws covers them; a
//! float's images sit above those of the panes under it. All of it is
//! written in the frame's own screen update, with the cursor saved before
//! and restored after.

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};

use orb_domain::ids::PaneId;
use orb_term::{GraphicsFrame, Pane, Visible};

/// The z of the lowest-ranked pane's images: deep under kitty's limit for
/// images drawn below cells with a background of their own
/// (−1,073,741,824).
const DEEPEST: i32 = -2_000_000_000;
/// The z values one pane's images span above its rank's base.
const RANK_SPAN: i32 = 1 << 24;
/// The highest rank with a band of its own; higher ranks share it, which
/// keeps every band under kitty's limit.
const TOP_RANK: usize = 54;

/// One drawn pane's images this frame: its image model's id, its rank among
/// the placed panes (tiled first, then floats bottom to top), and its
/// images.
pub(crate) type DrawnImages = (u64, usize, GraphicsFrame);

/// What kitty shows of the panes' images, kept in step with each frame.
#[derive(Default)]
pub(crate) struct Compositor {
    /// Per image model: the placements kitty shows, as last placed.
    shown: HashMap<u64, HashMap<(u32, u32), Visible>>,
    /// Per image model: the kitty ids of the images it last held.
    images: HashMap<u64, Vec<u32>>,
}

impl Compositor {
    /// Writes this frame's image bytes to `out` for the panes `drawn`, after
    /// taking the waiting bytes of the panes in `panes` that weren't drawn
    /// and reading which images every pane still holds.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to `out` fails.
    pub(crate) fn compose<W>(
        &mut self,
        out: &mut W,
        drawn: &[DrawnImages],
        panes: &HashMap<PaneId, Pane>,
    ) -> io::Result<()>
    where
        W: Write,
    {
        let drawn_models: HashSet<u64> = drawn.iter().map(|(model, _, _)| *model).collect();
        let hidden: Vec<u8> = panes
            .values()
            .filter(|pane| !drawn_models.contains(&pane.graphics_instance()))
            .flat_map(Pane::take_graphics_output)
            .collect();
        let live = panes
            .values()
            .map(|pane| (pane.graphics_instance(), pane.live_images()))
            .collect();
        self.write(out, drawn, &hidden, live)
    }

    /// Writes, inside `ESC 7 … ESC 8`, the drawn panes' waiting bytes, then
    /// `hidden` (the other panes' waiting bytes), then what places, moves
    /// and takes off placements, then frees for the images of models no
    /// longer in `live`. Writes nothing when none of that has anything.
    fn write<W>(
        &mut self,
        out: &mut W,
        drawn: &[DrawnImages],
        hidden: &[u8],
        live: HashMap<u64, Vec<u32>>,
    ) -> io::Result<()>
    where
        W: Write,
    {
        let mut body: Vec<u8> = drawn
            .iter()
            .flat_map(|(_, _, frame)| frame.out.iter().copied())
            .chain(hidden.iter().copied())
            .collect();
        let mut before = std::mem::take(&mut self.shown);
        for (model, rank, frame) in drawn {
            let shown = before.remove(model).unwrap_or_default();
            let wanted: HashMap<(u32, u32), Visible> = frame
                .visible
                .iter()
                .map(|visible| {
                    let z = band(*rank, visible.z);
                    (
                        (visible.image, visible.placement),
                        Visible { z, ..*visible },
                    )
                })
                .collect();
            for key in shown.keys().filter(|key| !wanted.contains_key(key)) {
                take_off(&mut body, *key);
            }
            for (key, visible) in &wanted {
                if shown.get(key) != Some(visible) {
                    place(&mut body, visible);
                }
            }
            self.shown.insert(*model, wanted);
        }
        for key in before.values().flat_map(HashMap::keys) {
            take_off(&mut body, *key);
        }
        for (_, images) in self
            .images
            .iter()
            .filter(|(model, _)| !live.contains_key(model))
        {
            for image in images {
                body.extend_from_slice(format!("\x1b_Ga=d,d=I,i={image},q=2\x1b\\").as_bytes());
            }
        }
        self.images = live;
        if body.is_empty() {
            return Ok(());
        }
        out.write_all(b"\x1b7")?;
        out.write_all(&body)?;
        out.write_all(b"\x1b8")
    }
}

/// kitty's z for an image the pane of `rank` asked to stack at `z`: under
/// every cell with a background of its own, and above the images of every
/// lower rank.
fn band(rank: usize, z: i32) -> i32 {
    let rank = i32::try_from(rank.min(TOP_RANK)).unwrap_or_default();
    DEEPEST + rank * RANK_SPAN + z.clamp(0, RANK_SPAN - 1)
}

/// Takes the placement `(image, placement)` off the screen, keeping the
/// image in kitty.
fn take_off(body: &mut Vec<u8>, (image, placement): (u32, u32)) {
    body.extend_from_slice(format!("\x1b_Ga=d,d=i,i={image},p={placement},q=2\x1b\\").as_bytes());
}

/// Moves the cursor to `visible`'s cell and places it there, replacing the
/// placement with the same ids; scaled only where the pane asked.
fn place(body: &mut Vec<u8>, visible: &Visible) {
    let Visible {
        image,
        placement,
        at,
        offset: (right, down),
        src: [x, y, width, height],
        cells: (columns, rows),
        z,
        ..
    } = *visible;
    let scale: String = [("c", columns), ("r", rows)]
        .into_iter()
        .filter_map(|(key, cells)| Some(format!(",{key}={}", cells?)))
        .collect();
    let command = format!(
        "\x1b[{};{}H\x1b_Ga=p,i={image},p={placement},x={x},y={y},w={width},h={height},X={right},Y={down}{scale},z={z},C=1,q=2\x1b\\",
        u32::from(at.y) + 1,
        u32::from(at.x) + 1,
    );
    body.extend_from_slice(command.as_bytes());
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use orb_term::{GraphicsFrame, Visible};
    use ratatui::layout::Position;

    use super::{Compositor, DrawnImages};

    /// Image 7's placement 1, 10×20 pixels, at outer cell (5, 3).
    fn visible() -> Visible {
        Visible {
            image: 7,
            placement: 1,
            generation: 0,
            at: Position::new(5, 3),
            offset: (0, 0),
            src: [0, 0, 10, 20],
            cells: (None, None),
            z: 0,
        }
    }

    /// Model `model` drawn at `rank`, showing `visible` with `out` waiting.
    fn drawn(model: u64, rank: usize, visible: Vec<Visible>, out: &[u8]) -> DrawnImages {
        let frame = GraphicsFrame {
            out: out.to_vec(),
            visible,
            holes: Vec::new(),
        };
        (model, rank, frame)
    }

    /// What `compositor` writes for `drawn` while models 1 and 2 hold
    /// image 7 and the models `gone` are gone.
    fn written(compositor: &mut Compositor, drawn: &[DrawnImages], gone: &[u64]) -> String {
        let live: HashMap<u64, Vec<u32>> = [1, 2]
            .into_iter()
            .filter(|model| !gone.contains(model))
            .map(|model| (model, vec![7]))
            .collect();
        let mut out = Vec::new();
        let Ok(()) = compositor.write(&mut out, drawn, &[], live) else {
            return "write failed".to_owned();
        };
        String::from_utf8_lossy(&out).into_owned()
    }

    /// kitty's placement of [`visible`] at its cell.
    const PLACED: &str =
        "\x1b[4;6H\x1b_Ga=p,i=7,p=1,x=0,y=0,w=10,h=20,X=0,Y=0,z=-2000000000,C=1,q=2\x1b\\";

    #[rstest::rstest]
    fn new_placement_is_placed_inside_cursor_save_restore() {
        // Given a compositor that placed nothing yet.
        let mut compositor = Compositor::default();

        // When a pane shows image 7 at cell (5, 3).
        let written = written(&mut compositor, &[drawn(1, 0, vec![visible()], b"")], &[]);

        // Then kitty is told to place it there, the cursor saved around it.
        assert_eq!(written, format!("\x1b7{PLACED}\x1b8"), "the placement");
    }

    #[rstest::rstest]
    fn scaled_placement_names_only_the_cells_asked_for() {
        // Given a compositor that placed nothing yet.
        let mut compositor = Compositor::default();

        // When a pane shows image 7 scaled to 4 columns, its rows left to
        // kitty.
        let scaled = Visible {
            cells: (Some(4), None),
            ..visible()
        };
        let written = written(&mut compositor, &[drawn(1, 0, vec![scaled], b"")], &[]);

        // Then the placement gives the columns and no rows.
        assert!(
            written.contains(",Y=0,c=4,z=") && !written.contains(",r="),
            "scaled placement {written:?}"
        );
    }

    #[rstest::rstest]
    fn unchanged_frame_writes_nothing() {
        // Given a pane whose image 7 is placed.
        let mut compositor = Compositor::default();
        let frame = [drawn(1, 0, vec![visible()], b"")];
        written(&mut compositor, &frame, &[]);

        // When the next frame shows it the same.
        let written = written(&mut compositor, &frame, &[]);

        // Then nothing is written.
        assert_eq!(written, "", "an unchanged frame");
    }

    #[rstest::rstest]
    fn moved_placement_is_placed_again_with_the_same_ids() {
        // Given a pane whose image 7 is placed at cell (5, 3).
        let mut compositor = Compositor::default();
        written(&mut compositor, &[drawn(1, 0, vec![visible()], b"")], &[]);

        // When the pane moves it one row down.
        let moved = Visible {
            at: Position::new(5, 4),
            ..visible()
        };
        let written = written(&mut compositor, &[drawn(1, 0, vec![moved], b"")], &[]);

        // Then the same placement is placed at the new cell, nothing taken off.
        assert_eq!(
            written,
            "\x1b7\x1b[5;6H\x1b_Ga=p,i=7,p=1,x=0,y=0,w=10,h=20,X=0,Y=0,z=-2000000000,C=1,q=2\x1b\\\x1b8",
            "the placement moved"
        );
    }

    #[rstest::rstest]
    fn queued_output_comes_before_placements() {
        // Given a compositor that placed nothing yet.
        let mut compositor = Compositor::default();

        // When a pane sends image 7 and shows it in the same frame.
        let transmit = "\x1b_Ga=t,f=32,s=10,v=20,t=s,i=7,q=2;bmFtZQ==\x1b\\";
        let frame = drawn(1, 0, vec![visible()], transmit.as_bytes());
        let written = written(&mut compositor, &[frame], &[]);

        // Then kitty gets the image before its placement.
        assert_eq!(
            written,
            format!("\x1b7{transmit}{PLACED}\x1b8"),
            "the transmit, then the placement"
        );
    }

    #[rstest::rstest]
    fn undrawn_pane_placements_are_taken_off_keeping_their_data() {
        // Given a pane whose image 7 is placed.
        let mut compositor = Compositor::default();
        written(&mut compositor, &[drawn(1, 0, vec![visible()], b"")], &[]);

        // When a frame doesn't draw that pane.
        let written = written(&mut compositor, &[], &[]);

        // Then its placement is taken off and the image kept.
        assert_eq!(
            written, "\x1b7\x1b_Ga=d,d=i,i=7,p=1,q=2\x1b\\\x1b8",
            "the placement taken off"
        );
    }

    #[rstest::rstest]
    fn vanished_instance_images_are_freed() {
        // Given a hidden pane holding image 7.
        let mut compositor = Compositor::default();
        written(&mut compositor, &[], &[]);

        // When that pane is gone.
        let written = written(&mut compositor, &[], &[1]);

        // Then kitty frees its image.
        assert_eq!(
            written, "\x1b7\x1b_Ga=d,d=I,i=7,q=2\x1b\\\x1b8",
            "the image freed"
        );
    }

    #[rstest::rstest]
    fn float_images_sit_above_tiled_images() {
        // Given a compositor that placed nothing yet.
        let mut compositor = Compositor::default();

        // When a tiled pane (rank 0) shows an image stacked high and a float
        // (rank 3) one stacked at the bottom.
        let tiled = Visible {
            z: 5000,
            ..visible()
        };
        let float = Visible {
            image: 8,
            ..visible()
        };
        let frames = [drawn(1, 0, vec![tiled], b""), drawn(2, 3, vec![float], b"")];
        let written = written(&mut compositor, &frames, &[]);

        // Then the float's image is above the tiled one's, both under the
        // cells with a background of their own.
        let zs: Vec<i64> = written
            .split(",z=")
            .skip(1)
            .filter_map(|rest| rest.split(',').next()?.parse().ok())
            .collect();
        assert!(
            matches!(zs.as_slice(), [tiled, float] if tiled < float && *float < -1_073_741_824),
            "z values {zs:?}"
        );
    }
}
