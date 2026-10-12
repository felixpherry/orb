//! Kitty images a pane's child sends, kept per pane so two panes never touch
//! each other's images in the one outer kitty they share.
//!
//! Each image the child sends gets an id of its own across all of orb and is
//! sent on to kitty untouched apart from its control keys: quiet, unplaced,
//! and kept in memory only when kitty can. Where the child placed it is
//! remembered here, to be shown inside the pane later. Deletes, screen
//! switches, screen clears and resets free the pane's own images in kitty.
//! The child's questions about image support are answered here; nothing kitty
//! says ever reaches the child. A pane can stop answering them, which also
//! drops everything else its child sends about images. Each frame, the placements on the screen the
//! child shows are worked out inside the pane's content.

mod command;
mod place;

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use command::{KittyCommand, Value};
use ratatui::layout::{Position, Rect};

use crate::pane::{CellSize, GraphicsSupport};

/// Outer image ids, unique across orb, starting at 1 and skipping 0 on wrap.
static NEXT_OUTER_ID: AtomicU32 = AtomicU32::new(1);
/// One per pane model, so a respawned pane never reuses its predecessor's
/// images.
static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);
/// The first id given to an image or placement the child sent without one.
const FIRST_ANON: u32 = 0x8000_0000;
/// The keys that place an image, dropped from every forwarded transmit.
const PLACEMENT_KEYS: &[u8] = b"pICXYxywhcrzUPQHV";

/// What a pane's images need this frame: bytes for kitty first, then where
/// its placements show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphicsFrame {
    /// Bytes for kitty in the child's order: transmits, frame edits, frees.
    pub out: Vec<u8>,
    /// The placements that show, cut to the pane.
    pub visible: Vec<Visible>,
    /// The outer cells those placements cover.
    pub holes: Vec<Rect>,
}

/// One placement as it shows on the outer screen, cut to its pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Visible {
    /// The image's id in kitty.
    pub image: u32,
    /// The placement's id within the image.
    pub placement: u32,
    /// Moves on each time the child sends the image again, which takes its
    /// placements off in kitty.
    pub generation: u32,
    /// The outer cell its top left corner is in.
    pub at: Position,
    /// Pixels right and down within that cell (`X`, `Y`).
    pub offset: (u32, u32),
    /// The part of the image shown (`x`, `y`, `w`, `h`); a zero width or
    /// height is the rest of the image.
    pub src: [u32; 4],
    /// The columns and rows it is scaled to (`c`, `r`), when the child
    /// asked; kitty works out a missing one from the image's shape.
    pub cells: (Option<u32>, Option<u32>),
    /// The child's stacking order for it.
    pub z: i32,
}

/// One pane's images and placements, and the bytes waiting for kitty.
#[derive(Debug)]
pub(crate) struct PaneGraphics {
    instance: u64,
    support: GraphicsSupport,
    /// Whether the child hears that images work; off, its images never
    /// reach kitty.
    answering: bool,
    /// The child's image ids and what they became.
    images: HashMap<u32, Image>,
    /// Where the child placed its images, by child image id and placement id.
    placements: HashMap<(u32, u32), Placement>,
    /// The next id for an image or placement the child sent without one.
    next_anon: u32,
    /// A chunked command still being collected.
    upload: Option<Upload>,
    /// Bytes for kitty, in the child's order: transmits, frame edits, frees.
    out: Vec<u8>,
}

/// One of the child's images, as kitty knows it.
#[derive(Debug)]
struct Image {
    /// Its id in kitty.
    outer: u32,
    /// Moves on each time the child sends the image again, which deletes its
    /// placements in kitty.
    generation: u32,
    /// Its size in pixels, when the child said.
    px: Option<(u32, u32)>,
    /// Whether it was sent on the alternate screen.
    alt: bool,
}

/// Where the child placed an image: the cell its cursor was on, and the
/// placement's own keys.
#[derive(Debug)]
struct Placement {
    line: i32,
    col: u16,
    /// Pixels right and down within that cell (`X`, `Y`).
    offset: (u32, u32),
    /// The part of the image shown (`x`, `y`, `w`, `h`), when given; a
    /// missing key is 0.
    src: Option<[u32; 4]>,
    /// The columns and rows it is scaled to (`c`, `r`), when given.
    cells: (Option<u32>, Option<u32>),
    z: i32,
    /// Whether it was placed on the alternate screen.
    alt: bool,
}

/// A chunked command being collected; nothing reaches kitty until its last
/// chunk arrives.
#[derive(Debug)]
struct Upload {
    /// The first chunk, as the child sent it.
    first: Vec<u8>,
    /// The later chunks, ready for kitty.
    rest: Vec<u8>,
    /// The quietness a later chunk asked for, which replaces the first one's.
    quiet: Option<i64>,
}

/// Where the child's cursor was, and on which screen, when a command arrived.
#[derive(Debug, Clone, Copy)]
struct Cursor {
    line: i32,
    col: u16,
    alt: bool,
}

impl PaneGraphics {
    /// An empty model for a pane whose images `support` says can reach the
    /// screen.
    pub(crate) fn new(support: GraphicsSupport) -> Self {
        Self {
            instance: NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed),
            support,
            answering: true,
            images: HashMap::new(),
            placements: HashMap::new(),
            next_anon: FIRST_ANON,
            upload: None,
            out: Vec::new(),
        }
    }

    /// This model's id, different for every pane ever made.
    pub(crate) fn instance(&self) -> u64 {
        self.instance
    }

    /// Starts or stops telling the child that images work; stopping frees
    /// the images it already sent.
    pub(crate) fn answer(&mut self, on: bool) {
        if self.answering && !on {
            self.reset();
        }
        self.answering = on;
    }

    /// Whether the child is told that images work: orb is in kitty and the
    /// pane answers image questions. Pixel mouse is offered on the same terms.
    pub(crate) fn answers(&self) -> bool {
        matches!(self.support, GraphicsSupport::Kitty { .. }) && self.answering
    }

    /// Applies one command (the bytes between `ESC _ G` and `ESC \`) sent
    /// with the child's cursor at `line`, `col` on the alternate screen when
    /// `alt`; what the child should hear back goes to `replies`.
    pub(crate) fn command(
        &mut self,
        body: &[u8],
        line: i32,
        col: u16,
        alt: bool,
        replies: &mut Vec<u8>,
    ) {
        let Some(mut command) = KittyCommand::parse(body) else {
            return;
        };
        let (GraphicsSupport::Kitty { transient }, true) = (self.support, self.answering) else {
            if action(&command) == b'q' {
                reply(replies, &command, "EINVAL:no graphics");
            }
            return;
        };
        let at = Cursor { line, col, alt };
        if let Some(upload) = &mut self.upload
            && continues(&command)
        {
            upload.quiet = command.int(b'q').or(upload.quiet);
            command.set(b'q', Value::Int(2));
            command.write_to(&mut upload.rest);
            if command.int(b'm') != Some(1) {
                self.finish(at, transient, replies);
            }
            return;
        }
        // Any other command ends a partial upload, as it does in kitty.
        self.upload = None;
        if command.int(b'm') == Some(1) && matches!(action(&command), b't' | b'T' | b'q' | b'f') {
            self.upload = Some(Upload {
                first: body.to_vec(),
                rest: Vec::new(),
                quiet: None,
            });
            return;
        }
        self.run(command, &[], at, transient, replies);
    }

    /// Runs the collected upload as one command, its chunks reaching kitty
    /// together.
    fn finish(&mut self, at: Cursor, transient: bool, replies: &mut Vec<u8>) {
        let Some(upload) = self.upload.take() else {
            return;
        };
        let Some(mut command) = KittyCommand::parse(&upload.first) else {
            return;
        };
        if let Some(quiet) = upload.quiet {
            command.set(b'q', Value::Int(quiet));
        }
        self.run(command, &upload.rest, at, transient, replies);
    }

    /// Acts on one whole command; `rest` holds its later chunks, ready for
    /// kitty.
    fn run(
        &mut self,
        command: KittyCommand<'_>,
        rest: &[u8],
        at: Cursor,
        transient: bool,
        replies: &mut Vec<u8>,
    ) {
        match action(&command) {
            b'q' => {
                let direct = matches!(command.char(b't'), None | Some(b'd'));
                let answer = if direct {
                    "OK"
                } else {
                    "EINVAL:orb answers only direct queries"
                };
                reply(replies, &command, answer);
            }
            b't' | b'T' => self.transmit(command, rest, at, transient, replies),
            b'p' => self.place(&command, at, replies),
            b'd' => self.delete(&command, at.alt),
            b'f' | b'a' | b'c' => self.edit(command, rest),
            _ => {}
        }
    }

    /// Sends an image on to kitty under its outer id, unplaced and quiet, and
    /// remembers where `a=T` placed it.
    fn transmit(
        &mut self,
        mut command: KittyCommand<'_>,
        rest: &[u8],
        at: Cursor,
        transient: bool,
        replies: &mut Vec<u8>,
    ) {
        let child = match id(&command, b'i') {
            Some(child) => child,
            None => self.anon(),
        };
        let px = match (
            command.int(b'f'),
            uint(&command, b's'),
            uint(&command, b'v'),
        ) {
            (Some(100), _, _) => None,
            (_, Some(width), Some(height)) => Some((width, height)),
            _ => None,
        };
        let outer = match self.images.entry(child) {
            Entry::Occupied(entry) => {
                // Sending an id again deletes its placements in kitty.
                self.placements.retain(|&(image, _), _| image != child);
                let image = entry.into_mut();
                image.generation = image.generation.wrapping_add(1);
                image.px = px;
                image.alt = at.alt;
                image.outer
            }
            Entry::Vacant(entry) => {
                let outer = next_outer_id();
                entry.insert(Image {
                    outer,
                    generation: 0,
                    px,
                    alt: at.alt,
                });
                outer
            }
        };
        if action(&command) == b'T' && command.int(b'U') != Some(1) {
            let placement = match id(&command, b'p') {
                Some(placement) => placement,
                None => self.anon(),
            };
            self.placements
                .insert((child, placement), placement_at(&command, at));
        }
        reply(replies, &command, "OK");
        command.set(b'a', Value::Char(b't'));
        command.set(b'i', Value::Int(outer.into()));
        command.remove(PLACEMENT_KEYS);
        command.set(b'q', Value::Int(2));
        if transient && command.int(b'N').is_none() {
            command.set(b'N', Value::Int(1));
        }
        command.write_to(&mut self.out);
        self.out.extend_from_slice(rest);
    }

    /// Places an image the child already sent, replacing the placement with
    /// the same id.
    fn place(&mut self, command: &KittyCommand<'_>, at: Cursor, replies: &mut Vec<u8>) {
        let Some(child) = id(command, b'i') else {
            return;
        };
        if !self.images.contains_key(&child) {
            reply(replies, command, "ENOENT:no such image");
            return;
        }
        if command.int(b'U') != Some(1) {
            let placement = match id(command, b'p') {
                Some(placement) => placement,
                None => self.anon(),
            };
            self.placements
                .insert((child, placement), placement_at(command, at));
        }
        reply(replies, command, "OK");
    }

    /// Deletes placements on the screen the child is on (`d=a`, `d=A`) or of
    /// one image (`d=i`, `d=I`); the uppercase forms also free the images
    /// left unplaced. Other forms are ignored.
    fn delete(&mut self, command: &KittyCommand<'_>, alt: bool) {
        match command.char(b'd').unwrap_or(b'a') {
            b'a' => self.placements.retain(|_, placement| placement.alt != alt),
            b'A' => self.cleared_screen(alt),
            which @ (b'i' | b'I') => {
                let Some(child) = id(command, b'i') else {
                    return;
                };
                match id(command, b'p') {
                    Some(placement) => {
                        self.placements.remove(&(child, placement));
                    }
                    None => self.placements.retain(|&(image, _), _| image != child),
                }
                if which == b'I' {
                    self.free_where(|image, _, placed| image == child && !placed);
                }
            }
            _ => {}
        }
    }

    /// Sends an animation command (`a=f`, `a=a`, `a=c`) on to kitty under
    /// the image's outer id, quiet; dropped when the image is unknown.
    fn edit(&mut self, mut command: KittyCommand<'_>, rest: &[u8]) {
        let Some(image) = id(&command, b'i').and_then(|child| self.images.get(&child)) else {
            return;
        };
        command.set(b'i', Value::Int(image.outer.into()));
        command.set(b'q', Value::Int(2));
        command.write_to(&mut self.out);
        self.out.extend_from_slice(rest);
    }

    /// The child switched screens: frees every image of the alternate
    /// screen, which kitty clears on entry.
    pub(crate) fn switched_screen(&mut self) {
        self.free_where(|_, image, _| image.alt);
    }

    /// The child cleared a screen (the alternate one when `alt`): drops its
    /// placements and frees its images left unplaced, as kitty does.
    pub(crate) fn cleared_screen(&mut self, alt: bool) {
        self.placements.retain(|_, placement| placement.alt != alt);
        self.free_where(|_, image, placed| image.alt == alt && !placed);
    }

    /// The child reset the terminal: frees every image.
    pub(crate) fn reset(&mut self) {
        self.upload = None;
        self.free_where(|_, _, _| true);
    }

    /// Takes the bytes waiting for kitty.
    pub(crate) fn take_out(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    /// Takes the bytes waiting for kitty, and works out where the placements
    /// show in `content`, whose cells are `cell` pixels, while the child is
    /// on the alternate screen when `alt_now`. Nothing shows while the view
    /// is `display_offset` lines back in the history.
    pub(crate) fn frame(
        &mut self,
        content: Rect,
        cell: CellSize,
        display_offset: usize,
        alt_now: bool,
    ) -> GraphicsFrame {
        // The alternate screen has no history, so only the main screen is
        // ever scrolled back.
        let (visible, holes) = self
            .placements
            .iter()
            .filter(|(_, placement)| placement.alt == alt_now && display_offset == 0)
            .filter_map(|(&(child, id), placement)| {
                place::visible(placement, self.images.get(&child)?, id, content, cell)
            })
            .unzip();
        GraphicsFrame {
            out: self.take_out(),
            visible,
            holes,
        }
    }

    /// The outer id of every image the pane holds in kitty.
    pub(crate) fn live_images(&self) -> Vec<u32> {
        self.images.values().map(|image| image.outer).collect()
    }

    /// Frees in kitty, and forgets with their placements, the images
    /// `doomed` picks from their child id, the image, and whether it has a
    /// placement.
    fn free_where<F>(&mut self, doomed: F)
    where
        F: Fn(u32, &Image, bool) -> bool,
    {
        self.images.retain(|&child, image| {
            let placed = self.placements.keys().any(|&(placed, _)| placed == child);
            if !doomed(child, image, placed) {
                return true;
            }
            let free = format!("\x1b_Ga=d,d=I,i={},q=2\x1b\\", image.outer);
            self.out.extend_from_slice(free.as_bytes());
            false
        });
        self.placements
            .retain(|(child, _), _| self.images.contains_key(child));
    }

    /// An id for an image or placement the child sent without one.
    fn anon(&mut self) -> u32 {
        let id = self.next_anon;
        self.next_anon = self.next_anon.wrapping_add(1).max(FIRST_ANON);
        id
    }
}

/// What `command` does; kitty's default is a transmit.
fn action(command: &KittyCommand<'_>) -> u8 {
    command.char(b'a').unwrap_or(b't')
}

/// Whether `command` is a later chunk of an upload: only `m` and `q`, plus
/// `a=f` for an animation frame.
fn continues(command: &KittyCommand<'_>) -> bool {
    command.only(b"mq") || (command.only(b"mqa") && command.char(b'a') == Some(b'f'))
}

/// The non-negative number `key` holds, if it fits.
fn uint(command: &KittyCommand<'_>, key: u8) -> Option<u32> {
    u32::try_from(command.int(key)?).ok()
}

/// The id `key` holds; 0 means none.
fn id(command: &KittyCommand<'_>, key: u8) -> Option<u32> {
    uint(command, key).filter(|&id| id != 0)
}

/// The placement `command` asks for with the cursor `at`.
fn placement_at(command: &KittyCommand<'_>, at: Cursor) -> Placement {
    let src = b"xywh".map(|key| uint(command, key));
    Placement {
        line: at.line,
        col: at.col,
        offset: (
            uint(command, b'X').unwrap_or(0),
            uint(command, b'Y').unwrap_or(0),
        ),
        src: src
            .iter()
            .any(Option::is_some)
            .then(|| src.map(|value| value.unwrap_or(0))),
        cells: (uint(command, b'c'), uint(command, b'r')),
        z: command
            .int(b'z')
            .and_then(|z| i32::try_from(z).ok())
            .unwrap_or(0),
        alt: at.alt,
    }
}

/// Answers `command` with `message` (`OK` or an error) unless its `q`
/// silences it (`q=1` silences OK, `q=2` everything); only a command with an
/// image id gets an answer.
fn reply(replies: &mut Vec<u8>, command: &KittyCommand<'_>, message: &str) {
    let Some(image) = id(command, b'i') else {
        return;
    };
    let quiet = command.int(b'q').unwrap_or(0);
    if quiet >= 2 || (quiet == 1 && message == "OK") {
        return;
    }
    let placement = id(command, b'p')
        .map(|placement| format!(",p={placement}"))
        .unwrap_or_default();
    let answer = format!("\x1b_Gi={image}{placement};{message}\x1b\\");
    replies.extend_from_slice(answer.as_bytes());
}

/// A new outer image id.
fn next_outer_id() -> u32 {
    loop {
        let id = NEXT_OUTER_ID.fetch_add(1, Ordering::Relaxed);
        if id != 0 {
            return id;
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;

    use super::command::KittyCommand;
    use super::{GraphicsFrame, PaneGraphics};
    use crate::pane::{CellSize, GraphicsSupport};

    /// What kitty 0.48 or later gets.
    const KITTY: GraphicsSupport = GraphicsSupport::Kitty { transient: true };

    /// An image of 10×20 pixels sent and placed at the child's cursor.
    const PLACED: &[u8] = b"a=T,f=32,s=10,v=20,t=s,i=1,p=1,C=1,q=2;bmFtZQ==";

    /// The frame for a 20×5 pane with cells of 10×20 pixels, at the outer
    /// screen's corner.
    fn frame(graphics: &mut PaneGraphics, display_offset: usize, alt_now: bool) -> GraphicsFrame {
        let cell = CellSize {
            width: 10,
            height: 20,
        };
        graphics.frame(Rect::new(0, 0, 20, 5), cell, display_offset, alt_now)
    }

    /// Sends `body` from a child whose cursor is at the top left of the main
    /// screen; returns what the child hears back.
    fn send(graphics: &mut PaneGraphics, body: &[u8]) -> Vec<u8> {
        let mut replies = Vec::new();
        graphics.command(body, 0, 0, false, &mut replies);
        replies
    }

    /// The bodies of the commands in `out`, between `ESC _ G` and `ESC \`.
    fn bodies(out: &[u8]) -> Vec<Vec<u8>> {
        out.split(|&byte| byte == b'\\')
            .filter_map(|command| command.strip_prefix(b"\x1b_G"))
            .map(|command| command.strip_suffix(b"\x1b").unwrap_or(command).to_vec())
            .collect()
    }

    /// The payloads of the commands in `out`.
    fn payloads(out: &[u8]) -> Vec<Vec<u8>> {
        bodies(out)
            .iter()
            .filter_map(|body| body.splitn(2, |&byte| byte == b';').nth(1))
            .map(<[u8]>::to_vec)
            .collect()
    }

    /// The free command kitty gets for outer image `outer`.
    fn free(outer: u32) -> Vec<u8> {
        format!("\x1b_Ga=d,d=I,i={outer},q=2\x1b\\").into_bytes()
    }

    #[rstest::rstest]
    fn forwarded_transmit_is_quiet_and_unplaced() {
        // Given a pane in kitty.
        let mut graphics = PaneGraphics::new(KITTY);

        // When the child transmits and places an image as terminal-browser does.
        send(
            &mut graphics,
            b"a=T,f=32,s=1440,v=960,t=s,i=1,p=1,C=1,z=0,X=3,Y=21,q=2;L3B4LTM2MzE0LTAtMQ==",
        );

        // Then kitty gets a quiet transmit without placement keys and with
        // the same payload.
        let out = graphics.take_out();
        let bodies = bodies(&out);
        let forwarded: Vec<_> = bodies
            .iter()
            .filter_map(|body| KittyCommand::parse(body))
            .map(|command| (command.int(b'q'), b"pCXYz".map(|key| command.int(key))))
            .collect();
        assert_eq!(
            (forwarded, payloads(&out)),
            (
                vec![(Some(2), [None; 5])],
                vec![b"L3B4LTM2MzE0LTAtMQ==".to_vec()]
            ),
            "forwarded transmit {:?}",
            String::from_utf8_lossy(&out)
        );
    }

    #[rstest::rstest]
    #[case::from_kitty_0_48(true, Some(1))]
    #[case::before_kitty_0_48(false, None)]
    fn transient_hint_follows_the_outer_kitty(
        #[case] transient: bool,
        #[case] expected: Option<i64>,
    ) {
        // Given a pane in a kitty that does or doesn't take the hint.
        let mut graphics = PaneGraphics::new(GraphicsSupport::Kitty { transient });

        // When the child transmits an image without one.
        send(&mut graphics, b"a=t,f=32,s=1,v=1,t=s,i=1,q=2;bmFtZQ==");

        // Then the forwarded transmit carries `N=1` only for the newer kitty.
        let out = graphics.take_out();
        let hints: Vec<_> = bodies(&out)
            .iter()
            .filter_map(|body| KittyCommand::parse(body))
            .map(|command| command.int(b'N'))
            .collect();
        assert_eq!(hints, vec![expected], "transient hint");
    }

    #[rstest::rstest]
    fn same_child_id_in_two_panes_gets_two_outer_ids() {
        // Given two panes in kitty.
        let mut left = PaneGraphics::new(KITTY);
        let mut right = PaneGraphics::new(KITTY);

        // When each child transmits its image 1.
        send(&mut left, b"a=t,f=32,s=1,v=1,t=s,i=1,q=2;bGVmdA==");
        send(&mut right, b"a=t,f=32,s=1,v=1,t=s,i=1,q=2;cmlnaHQ=");

        // Then the two images have different ids in kitty.
        assert_ne!(
            left.live_images(),
            right.live_images(),
            "each pane's image 1 is its own in kitty"
        );
    }

    #[rstest::rstest]
    fn delete_all_frees_only_the_panes_own_images() {
        // Given two panes in kitty that each sent their image 1.
        let mut left = PaneGraphics::new(KITTY);
        let mut right = PaneGraphics::new(KITTY);
        send(&mut left, b"a=t,f=32,s=1,v=1,t=s,i=1,q=2;bGVmdA==");
        send(&mut right, b"a=t,f=32,s=1,v=1,t=s,i=1,q=2;cmlnaHQ=");
        let own = left.live_images();
        left.take_out();

        // When the left child deletes every image.
        send(&mut left, b"a=d,d=A,q=2");

        // Then kitty is told to free only the left pane's image.
        let expected: Vec<u8> = own.into_iter().flat_map(free).collect();
        assert_eq!(
            String::from_utf8_lossy(&left.take_out()),
            String::from_utf8_lossy(&expected),
            "frees for the left pane"
        );
    }

    #[rstest::rstest]
    fn chunked_upload_queues_nothing_before_its_last_chunk() {
        // Given a pane in kitty.
        let mut graphics = PaneGraphics::new(KITTY);

        // When the child sends the first two chunks of an image.
        send(&mut graphics, b"a=T,f=24,s=2,v=2,i=3,q=2,m=1;AAAA");
        send(&mut graphics, b"m=1;BBBB");

        // Then nothing is waiting for kitty yet.
        assert!(
            graphics.take_out().is_empty(),
            "an unfinished upload stays with orb"
        );
    }

    #[rstest::rstest]
    fn completed_chunked_upload_is_queued_in_one_run() {
        // Given a pane in kitty that got the first two chunks of an image.
        let mut graphics = PaneGraphics::new(KITTY);
        send(&mut graphics, b"a=T,f=24,s=2,v=2,i=3,q=2,m=1;AAAA");
        send(&mut graphics, b"m=1;BBBB");

        // When the last chunk arrives.
        send(&mut graphics, b"m=0;CCCC");

        // Then kitty gets the three chunks back to back, in order.
        assert_eq!(
            payloads(&graphics.take_out()),
            vec![b"AAAA".to_vec(), b"BBBB".to_vec(), b"CCCC".to_vec()],
            "chunks queued together"
        );
    }

    #[rstest::rstest]
    fn query_is_never_forwarded() {
        // Given a pane in kitty.
        let mut graphics = PaneGraphics::new(KITTY);

        // When the child asks whether images work.
        send(&mut graphics, b"i=4207,a=q,t=d,f=24,s=1,v=1;AAAA");

        // Then nothing is waiting for kitty.
        assert!(graphics.take_out().is_empty(), "queries stay with orb");
    }

    #[rstest::rstest]
    fn transmit_without_kitty_queues_nothing() {
        // Given a pane whose outer terminal shows no images.
        let mut graphics = PaneGraphics::new(GraphicsSupport::None);

        // When the child transmits an image.
        send(&mut graphics, b"a=T,f=32,s=1,v=1,t=s,i=1,q=2;bmFtZQ==");

        // Then nothing is waiting for the outer terminal.
        assert!(
            graphics.take_out().is_empty(),
            "images are dropped without kitty"
        );
    }

    #[rstest::rstest]
    fn placing_an_unknown_image_is_an_error() {
        // Given a pane in kitty with no images.
        let mut graphics = PaneGraphics::new(KITTY);

        // When the child places image 9.
        let replies = send(&mut graphics, b"a=p,i=9,p=1");

        // Then it hears that there is no such image.
        assert_eq!(
            String::from_utf8_lossy(&replies),
            "\x1b_Gi=9,p=1;ENOENT:no such image\x1b\\",
            "reply to a placement of a missing image"
        );
    }

    #[rstest::rstest]
    fn frame_edit_reaches_kitty_under_the_outer_id() {
        // Given a pane in kitty whose child sent image 1.
        let mut graphics = PaneGraphics::new(KITTY);
        send(&mut graphics, b"a=t,f=32,s=4,v=4,t=s,i=1,q=2;bmFtZQ==");
        let outer = graphics.live_images();
        graphics.take_out();

        // When the child edits that image's root frame.
        send(&mut graphics, b"a=f,i=1,r=1,x=1,y=1,s=2,v=2,t=s;cGF0Y2g=");

        // Then kitty gets the edit for the image's outer id, quiet.
        let ids: Vec<_> = bodies(&graphics.take_out())
            .iter()
            .filter_map(|body| KittyCommand::parse(body))
            .map(|command| (command.int(b'i'), command.int(b'q')))
            .collect();
        let expected: Vec<_> = outer
            .into_iter()
            .map(|outer| (Some(i64::from(outer)), Some(2)))
            .collect();
        assert_eq!(ids, expected, "frame edit's id and quietness");
    }

    #[rstest::rstest]
    fn placement_on_the_other_screen_is_hidden() {
        // Given an image the child placed on the main screen.
        let mut graphics = PaneGraphics::new(KITTY);
        send(&mut graphics, PLACED);

        // When drawing the pane while it shows the alternate screen.
        let frame = frame(&mut graphics, 0, true);

        // Then the image doesn't show.
        assert!(
            frame.visible.is_empty(),
            "the main screen's image is hidden"
        );
    }

    #[rstest::rstest]
    fn scrolled_back_main_screen_hides_placements() {
        // Given an image the child placed on the main screen.
        let mut graphics = PaneGraphics::new(KITTY);
        send(&mut graphics, PLACED);

        // When drawing the pane scrolled three lines back into its history.
        let frame = frame(&mut graphics, 3, false);

        // Then the image doesn't show.
        assert!(frame.visible.is_empty(), "images don't follow the history");
    }

    #[rstest::rstest]
    fn retransmit_moves_the_generation_on() {
        // Given an image the child placed and that was drawn once.
        let mut graphics = PaneGraphics::new(KITTY);
        send(&mut graphics, PLACED);
        let before: Vec<_> = frame(&mut graphics, 0, false)
            .visible
            .iter()
            .map(|visible| visible.generation)
            .collect();

        // When the child sends and places the same image again.
        send(&mut graphics, PLACED);

        // Then the placement shows with a new generation.
        let after: Vec<_> = frame(&mut graphics, 0, false)
            .visible
            .iter()
            .map(|visible| visible.generation)
            .collect();
        assert!(
            before.len() == 1 && after.len() == 1 && before != after,
            "generation before {before:?}, after {after:?}"
        );
    }

    #[rstest::rstest]
    fn frame_drains_the_queued_output() {
        // Given a transmit that one frame already took.
        let mut graphics = PaneGraphics::new(KITTY);
        send(&mut graphics, PLACED);
        frame(&mut graphics, 0, false);

        // When drawing the next frame.
        let frame = frame(&mut graphics, 0, false);

        // Then it has nothing more for kitty.
        assert!(frame.out.is_empty(), "the transmit went out once");
    }
}
