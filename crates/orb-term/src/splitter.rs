//! Picks out of a pane's raw output the few sequences orb handles itself,
//! before the rest reaches the terminal emulator.
//!
//! The child's output arrives in reads that can cut any sequence in two, so
//! the start of a cut sequence is carried into the next read. What comes out
//! keeps the child's byte order: runs of text for the emulator, kitty graphics
//! commands, and markers for the synchronized-update brackets, the cell-size,
//! version and pixel-mouse queries, pixel-mouse switches, screen clears,
//! resets and shell prompt marks (OSC 133), which are left in the text and
//! marked. Only the exact forms below are picked out; everything else stays
//! text.

use std::borrow::Cow;
use std::mem;

const ESC: u8 = 0x1b;
/// The longest unfinished escape sequence carried into the next read; a
/// longer one is let through as text, except a prompt mark, which may run to
/// [`MAX_MARK`].
const MAX_ESCAPE: usize = 32;
/// The longest prompt mark carried into the next read; a longer one is marked
/// from what arrived, and the rest of it is text.
const MAX_MARK: usize = 4 * 1024;
/// The bytes a prompt mark starts with, after its `ESC ]`.
const MARK_START: &[u8] = b"133;";
/// The parameter of a command start that holds its percent-encoded command
/// line.
const CMDLINE_URL: &[u8] = b"cmdline_url=";
const BEL: u8 = 0x07;
/// The largest graphics command kept; a bigger one is dropped whole.
const MAX_GRAPHICS: usize = 16 * 1024 * 1024;
/// The length of `ESC _ G`, which starts a graphics command.
const GRAPHICS_START: usize = 3;

/// One piece of a pane's output, in the order the child wrote it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Piece<'a> {
    /// Bytes for the terminal, unchanged: borrowed from the read, or owned
    /// when part of them came from the previous read.
    Text(Cow<'a, [u8]>),
    /// A kitty graphics command: the bytes between `ESC _ G` and `ESC \`.
    Graphics(Vec<u8>),
    /// The start of a synchronized update (`CSI ? 2026 h`), taken out of the
    /// text.
    SyncStart,
    /// The end of a synchronized update (`CSI ? 2026 l`), taken out of the
    /// text.
    SyncEnd,
    /// A question for the cell size in pixels (`CSI 16 t`), taken out of the
    /// text.
    CellSizeQuery,
    /// A question for the terminal's name and version (`CSI > q` or
    /// `CSI > 0 q`), taken out of the text.
    VersionQuery,
    /// Follows a private mode set or reset whose parameters include 1016
    /// (pixel mouse reports), left in the text before it; `true` for set.
    PixelMouse(bool),
    /// A question for the pixel mouse mode (`CSI ? 1016 $ p`), taken out of
    /// the text.
    PixelMouseQuery,
    /// Follows a screen clear (`CSI 2 J` or `CSI 3 J`) left in the text before
    /// it.
    ClearedScreen,
    /// Follows a full reset (`ESC c`) left in the text before it.
    Reset,
    /// Follows a shell's prompt mark (OSC 133) left in the text before it.
    PromptMark(Mark),
}

/// One shell prompt mark (OSC 133).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Mark {
    /// `A`: the prompt starts.
    PromptStart,
    /// `B`: the prompt ends; typing starts here.
    InputStart,
    /// `C`: a command starts; `program` is the first word of its
    /// percent-decoded `cmdline_url`.
    CommandStart { program: Option<String> },
    /// `D`: the command ended, with its exit status when given.
    CommandEnd { exit: Option<i32> },
}

impl Piece<'_> {
    /// The same piece, owning its bytes.
    pub(crate) fn into_owned(self) -> Piece<'static> {
        match self {
            Piece::Text(text) => Piece::Text(Cow::Owned(text.into_owned())),
            Piece::Graphics(body) => Piece::Graphics(body),
            Piece::SyncStart => Piece::SyncStart,
            Piece::SyncEnd => Piece::SyncEnd,
            Piece::CellSizeQuery => Piece::CellSizeQuery,
            Piece::VersionQuery => Piece::VersionQuery,
            Piece::PixelMouse(on) => Piece::PixelMouse(on),
            Piece::PixelMouseQuery => Piece::PixelMouseQuery,
            Piece::ClearedScreen => Piece::ClearedScreen,
            Piece::Reset => Piece::Reset,
            Piece::PromptMark(mark) => Piece::PromptMark(mark),
        }
    }
}

/// Splits a pane's output into [`Piece`]s, read after read.
#[derive(Debug, Default)]
pub(crate) struct Splitter {
    /// The bytes of a sequence the last read cut off.
    carry: Vec<u8>,
    /// What the carried bytes are.
    open: Open,
}

/// What the previous read left unfinished.
#[derive(Debug, Clone, Copy, Default)]
enum Open {
    /// Nothing: the next read starts fresh.
    #[default]
    Nothing,
    /// The start of an escape sequence that may still be one orb picks out.
    Escape,
    /// A graphics command's bytes so far.
    Graphics,
    /// A graphics command over the size limit, skipped to its end; `esc` when
    /// the last read ended in the first byte of `ESC \`.
    Skipping { esc: bool },
}

/// What the bytes at an `ESC` turned out to be.
enum Seq {
    /// The start of a sequence orb picks out, cut short.
    Partial,
    /// Text: none of orb's sequences starts here. Scanning goes on from this
    /// index, the first byte that rules them all out.
    Text(usize),
    /// A sequence of this length that orb takes out of the text.
    TakenOut(Piece<'static>, usize),
    /// A sequence of this length left in the text, followed by a marker.
    Marked(Piece<'static>, usize),
    /// `ESC _ G`, the start of a graphics command.
    Graphics,
}

impl Splitter {
    /// Splits one read. A sequence cut at its end is carried into the next
    /// call.
    pub(crate) fn split<'a>(&mut self, bytes: &'a [u8]) -> Vec<Piece<'a>> {
        let mut pieces = Vec::new();
        let mut rest = bytes;
        while !rest.is_empty() {
            rest = match self.open {
                Open::Nothing => self.text(rest, &mut pieces),
                Open::Escape => self.resume_escape(rest, &mut pieces),
                Open::Graphics | Open::Skipping { .. } => self.graphics(rest, &mut pieces),
            };
        }
        pieces
    }

    /// Scans `bytes` from a fresh start, keeping each run of text whole up to
    /// the next sequence orb picks out; returns what follows that sequence.
    fn text<'a>(&mut self, bytes: &'a [u8], pieces: &mut Vec<Piece<'a>>) -> &'a [u8] {
        let mut scanned = 0;
        loop {
            let Some(at) = bytes
                .get(scanned..)
                .and_then(|unscanned| unscanned.iter().position(|&byte| byte == ESC))
                .map(|at| scanned + at)
            else {
                push_text(pieces, Cow::Borrowed(bytes));
                return &[];
            };
            let (before, seq) = bytes.split_at_checked(at).unwrap_or((bytes, &[]));
            match classify(seq) {
                Seq::Text(skip) => scanned = at + skip,
                Seq::Partial if seq.len() >= cap(seq) => {
                    push_text(pieces, Cow::Borrowed(bytes));
                    return &[];
                }
                Seq::Partial => {
                    push_text(pieces, Cow::Borrowed(before));
                    self.carry.extend_from_slice(seq);
                    self.open = Open::Escape;
                    return &[];
                }
                Seq::TakenOut(piece, len) => {
                    push_text(pieces, Cow::Borrowed(before));
                    pieces.push(piece);
                    return seq.get(len..).unwrap_or_default();
                }
                Seq::Marked(piece, len) => {
                    let (text, after) = bytes.split_at_checked(at + len).unwrap_or((bytes, &[]));
                    push_text(pieces, Cow::Borrowed(text));
                    pieces.push(piece);
                    return after;
                }
                Seq::Graphics => {
                    push_text(pieces, Cow::Borrowed(before));
                    self.open = Open::Graphics;
                    return seq.get(GRAPHICS_START..).unwrap_or_default();
                }
            }
        }
    }

    /// Finishes the escape sequence the last read cut off with the start of
    /// `bytes`; returns the bytes after it.
    fn resume_escape<'a>(&mut self, bytes: &'a [u8], pieces: &mut Vec<Piece<'a>>) -> &'a [u8] {
        let carried = self.carry.len();
        let (head, _) = bytes
            .split_at_checked(MAX_MARK.saturating_sub(carried))
            .unwrap_or((bytes, &[]));
        self.carry.extend_from_slice(head);
        // How much of the joined bytes is text, the marker after it, how much
        // of them the sequence used, and what comes next.
        let (text, marker, used, open) = match classify(&self.carry) {
            Seq::Partial if self.carry.len() < cap(&self.carry) => return &[],
            Seq::Partial => (self.carry.len(), None, self.carry.len(), Open::Nothing),
            Seq::Text(len) => (len, None, len, Open::Nothing),
            Seq::TakenOut(piece, len) => (0, Some(piece), len, Open::Nothing),
            Seq::Marked(piece, len) => (len, Some(piece), len, Open::Nothing),
            Seq::Graphics => (0, None, GRAPHICS_START, Open::Graphics),
        };
        // A prompt mark ended by an escape that isn't its `ESC \` stops
        // before that escape, which may sit in the carried bytes; it starts
        // the next sequence.
        let unused = self.carry.get(used..carried).map(<[u8]>::to_vec);
        self.carry.truncate(text);
        push_text(pieces, Cow::Owned(mem::take(&mut self.carry)));
        pieces.extend(marker);
        self.open = open;
        if let Some(unused) = unused.filter(|unused| !unused.is_empty()) {
            self.carry = unused;
            self.open = Open::Escape;
            return bytes;
        }
        bytes
            .get(used.saturating_sub(carried)..)
            .unwrap_or_default()
    }

    /// Collects a graphics command's bytes up to its `ESC \`; returns the
    /// bytes after it.
    fn graphics<'a>(&mut self, bytes: &'a [u8], pieces: &mut Vec<Piece<'a>>) -> &'a [u8] {
        let esc_before = match self.open {
            Open::Skipping { esc } => esc,
            _ => self.carry.last() == Some(&ESC),
        };
        // Where the command's bytes in this read stop, and where the bytes
        // after its `ESC \` start; `None` when it goes on into the next read.
        let end = match bytes.first() {
            Some(b'\\') if esc_before => {
                self.carry.pop();
                Some((0, 1))
            }
            _ => bytes
                .windows(2)
                .position(|pair| pair == b"\x1b\\")
                .map(|at| (at, at + 2)),
        };
        let (body, after) = match end {
            Some((stop, after)) => (
                bytes.get(..stop).unwrap_or_default(),
                bytes.get(after..).unwrap_or_default(),
            ),
            None => (bytes, &[][..]),
        };
        let skipping = matches!(self.open, Open::Skipping { .. })
            || self.carry.len() + body.len() > MAX_GRAPHICS;
        match (skipping, end) {
            (false, Some(_)) => {
                self.carry.extend_from_slice(body);
                pieces.push(Piece::Graphics(mem::take(&mut self.carry)));
                self.open = Open::Nothing;
            }
            (false, None) => self.carry.extend_from_slice(body),
            (true, Some(_)) => {
                self.carry = Vec::new();
                self.open = Open::Nothing;
            }
            (true, None) => {
                self.carry = Vec::new();
                self.open = Open::Skipping {
                    esc: bytes.last() == Some(&ESC),
                };
            }
        }
        after
    }
}

/// Adds `text` as a piece unless it is empty.
fn push_text<'a>(pieces: &mut Vec<Piece<'a>>, text: Cow<'a, [u8]>) {
    if !text.is_empty() {
        pieces.push(Piece::Text(text));
    }
}

/// What `seq`, which starts with `ESC`, is.
fn classify(seq: &[u8]) -> Seq {
    match seq {
        [ESC] | [ESC, b'_'] => Seq::Partial,
        [ESC, b'c', ..] => Seq::Marked(Piece::Reset, 2),
        [ESC, b'_', b'G', ..] => Seq::Graphics,
        [ESC, b'_', ..] => Seq::Text(2),
        [ESC, b'[', body @ ..] => csi(body),
        [ESC, b']', body @ ..] => osc(body),
        _ => Seq::Text(1),
    }
}

/// How long the unfinished sequence `seq` may grow while carried into the
/// next read.
fn cap(seq: &[u8]) -> usize {
    match seq {
        [ESC, b']', body @ ..] if body.starts_with(MARK_START) => MAX_MARK,
        _ => MAX_ESCAPE,
    }
}

/// What an OSC is, from the bytes after its `ESC ]`: a prompt mark (`133;`
/// then `A`, `B`, `C` or `D`) ending in BEL or `ESC \`, or text. An escape
/// other than `ESC \` ends a mark before it.
fn osc(body: &[u8]) -> Seq {
    if body.len() < MARK_START.len() && MARK_START.starts_with(body) {
        return Seq::Partial;
    }
    let Some(after) = body.strip_prefix(MARK_START) else {
        return Seq::Text(2);
    };
    let letter = match after {
        [] | [b'A'..=b'D'] => return Seq::Partial,
        [letter @ b'A'..=b'D', b';' | BEL | ESC, ..] => *letter,
        _ => return Seq::Text(2),
    };
    // The most of `body` a mark may take, and where its parameters start.
    let limit = MAX_MARK - 2;
    let params_at = MARK_START.len() + 1;
    let terminator = body
        .get(params_at..body.len().min(limit))
        .and_then(|params| params.iter().position(|&byte| byte == BEL || byte == ESC))
        .map(|at| params_at + at);
    // The mark's length in `body`, and where its parameters stop.
    let (len, params_end) = match terminator {
        Some(at) => match (body.get(at), body.get(at + 1)) {
            (Some(&BEL), _) => (at + 1, at),
            (_, Some(b'\\')) => (at + 2, at),
            (_, None) if body.len() < limit => return Seq::Partial,
            _ => (at, at),
        },
        None if body.len() < limit => return Seq::Partial,
        None => (limit, limit),
    };
    let params = body.get(params_at..params_end).unwrap_or_default();
    Seq::Marked(Piece::PromptMark(mark(letter, params)), 2 + len)
}

/// The prompt mark `letter` names, read from its `params`: the bytes after
/// the letter, up to its terminator.
fn mark(letter: u8, params: &[u8]) -> Mark {
    let mut params = params
        .strip_prefix(b";")
        .unwrap_or(params)
        .split(|&byte| byte == b';');
    match letter {
        b'A' => Mark::PromptStart,
        b'B' => Mark::InputStart,
        b'C' => Mark::CommandStart {
            program: params
                .find_map(|param| param.strip_prefix(CMDLINE_URL))
                .and_then(|url| {
                    String::from_utf8_lossy(&percent_decoded(url))
                        .split_whitespace()
                        .next()
                        .map(str::to_owned)
                }),
        },
        _ => Mark::CommandEnd {
            exit: params
                .next()
                .and_then(|status| std::str::from_utf8(status).ok())
                .and_then(|status| status.parse().ok()),
        },
    }
}

/// `bytes` with each `%XX` (two hex digits) turned into the byte it names; a
/// `%` not followed by two hex digits stays as it is.
fn percent_decoded(bytes: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while let Some((&byte, after)) = rest.split_first() {
        let escaped = match (byte, after) {
            (b'%', [high, low, ..]) => hex(*high).zip(hex(*low)).map(|(high, low)| high << 4 | low),
            _ => None,
        };
        match escaped {
            Some(escaped) => {
                decoded.push(escaped);
                rest = after.get(2..).unwrap_or_default();
            }
            None => {
                decoded.push(byte);
                rest = after;
            }
        }
    }
    decoded
}

/// The value of the hex digit `digit`.
fn hex(digit: u8) -> Option<u8> {
    u8::try_from(char::from(digit).to_digit(16)?).ok()
}

/// What a CSI is, from the bytes after its `ESC [`: parameter and
/// intermediate bytes, then one final byte.
fn csi(body: &[u8]) -> Seq {
    let Some(last) = body.iter().position(|byte| !(0x20..=0x3f).contains(byte)) else {
        return Seq::Partial;
    };
    let len = 2 + last + 1;
    match body.get(..=last).unwrap_or_default() {
        b"?2026h" => Seq::TakenOut(Piece::SyncStart, len),
        b"?2026l" => Seq::TakenOut(Piece::SyncEnd, len),
        b"16t" => Seq::TakenOut(Piece::CellSizeQuery, len),
        b">q" | b">0q" => Seq::TakenOut(Piece::VersionQuery, len),
        b"2J" | b"3J" => Seq::Marked(Piece::ClearedScreen, len),
        b"?1016$p" => Seq::TakenOut(Piece::PixelMouseQuery, len),
        [b'?', params @ .., end @ (b'h' | b'l')]
            if params
                .split(|byte| *byte == b';')
                .any(|param| param == b"1016") =>
        {
            Seq::Marked(Piece::PixelMouse(*end == b'h'), len)
        }
        [.., 0x40..=0x7e] => Seq::Text(len),
        _ => Seq::Text(2 + last),
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::{MAX_GRAPHICS, MAX_MARK, Mark, Piece, Splitter};

    /// The pieces `reads` split into, one read after another, owned, with
    /// neighbouring text joined.
    fn pieces(reads: &[&[u8]]) -> Vec<Piece<'static>> {
        let mut splitter = Splitter::default();
        let mut pieces: Vec<Piece<'static>> = Vec::new();
        for read in reads {
            for piece in splitter.split(read) {
                match (pieces.last_mut(), piece) {
                    (Some(Piece::Text(joined)), Piece::Text(text)) => {
                        joined.to_mut().extend_from_slice(&text);
                    }
                    (_, piece) => pieces.push(piece.into_owned()),
                }
            }
        }
        pieces
    }

    /// A text piece holding `bytes`.
    fn text(bytes: &[u8]) -> Piece<'static> {
        Piece::Text(Cow::Owned(bytes.to_vec()))
    }

    #[rstest::rstest]
    fn graphics_command_cut_anywhere_comes_out_whole() {
        // Given a cursor move then a graphics command, as terminal-browser
        // writes them.
        let body: &[u8] = b"a=T,f=32,s=154,v=110,t=s,i=41,p=1,C=1,z=532,X=3,Y=21,N=1,q=2;\
            L3B4LTY1NzcwLTAtNjEy";
        let sample = [b"\x1b[35;101H\x1b_G", body, b"\x1b\\"].concat();

        for cut in 0..=sample.len() {
            // When it arrives in two reads cut at `cut`.
            let (first, second) = sample.split_at_checked(cut).unwrap_or_default();
            let split = pieces(&[first, second]);

            // Then the cursor move is text and the command comes out once,
            // whole.
            assert_eq!(
                split,
                [text(b"\x1b[35;101H"), Piece::Graphics(body.to_vec())],
                "pieces for the cut at byte {cut}"
            );
        }
    }

    #[rstest::rstest]
    fn apc_other_than_graphics_stays_text() {
        // Given an application command that isn't a graphics command.
        let output = b"a\x1b_Xhello\x1b\\b";

        // When splitting it.
        let split = pieces(&[output]);

        // Then all of it is text.
        assert_eq!(split, [text(output)], "a non-graphics APC is text");
    }

    #[rstest::rstest]
    #[case::start(b"h", Piece::SyncStart)]
    #[case::end(b"l", Piece::SyncEnd)]
    fn sync_markers_never_reach_the_text(#[case] last: &[u8], #[case] marker: Piece<'static>) {
        // Given text around a synchronized-update bracket.
        let output = [b"a\x1b[?2026", last, b"b"].concat();

        // When splitting it.
        let split = pieces(&[&output]);

        // Then the bracket comes out as a marker between the two texts.
        assert_eq!(
            split,
            [text(b"a"), marker, text(b"b")],
            "the bracket is taken out of the text"
        );
    }

    #[rstest::rstest]
    fn sync_start_cut_across_reads_is_recognised() {
        // Given a synchronized-update start cut in two reads.
        let reads: [&[u8]; 2] = [b"a\x1b[?20", b"26hb"];

        // When splitting them.
        let split = pieces(&reads);

        // Then the start comes out as a marker.
        assert_eq!(
            split,
            [text(b"a"), Piece::SyncStart, text(b"b")],
            "the cut start is still recognised"
        );
    }

    #[rstest::rstest]
    fn cell_size_query_is_taken_out_of_the_text() {
        // Given text around a cell-size query.
        let output = b"a\x1b[16tb";

        // When splitting it.
        let split = pieces(&[output]);

        // Then the query comes out on its own.
        assert_eq!(
            split,
            [text(b"a"), Piece::CellSizeQuery, text(b"b")],
            "the query is taken out of the text"
        );
    }

    #[rstest::rstest]
    #[case::bare(b"\x1b[>q")]
    #[case::zero(b"\x1b[>0q")]
    fn version_query_is_taken_out_of_the_text(#[case] query: &[u8]) {
        // Given text around a version query.
        let output = [b"a", query, b"b"].concat();

        // When splitting it.
        let split = pieces(&[&output]);

        // Then the query comes out on its own.
        assert_eq!(
            split,
            [text(b"a"), Piece::VersionQuery, text(b"b")],
            "the query is taken out of the text"
        );
    }

    #[rstest::rstest]
    #[case::screen(b"2J")]
    #[case::screen_and_history(b"3J")]
    fn screen_clear_stays_in_the_text_and_is_marked(#[case] clear: &[u8]) {
        // Given text around a screen clear.
        let output = [b"a\x1b[", clear, b"b"].concat();

        // When splitting it.
        let split = pieces(&[&output]);

        // Then the clear stays in the text before it and a marker follows.
        assert_eq!(
            split,
            [
                text(&[b"a\x1b[", clear].concat()),
                Piece::ClearedScreen,
                text(b"b")
            ],
            "the clear is text, then marked"
        );
    }

    #[rstest::rstest]
    fn reset_stays_in_the_text_and_is_marked() {
        // Given text around a full reset.
        let output = b"a\x1bcb";

        // When splitting it.
        let split = pieces(&[output]);

        // Then the reset stays in the text before it and a marker follows.
        assert_eq!(
            split,
            [text(b"a\x1bc"), Piece::Reset, text(b"b")],
            "the reset is text, then marked"
        );
    }

    #[rstest::rstest]
    fn pixel_mouse_query_is_taken_out_of_the_text() {
        // Given text around a pixel mouse mode query.
        let output = b"ab\x1b[?1016$pcd";

        // When splitting it.
        let split = pieces(&[output]);

        // Then the query comes out on its own.
        assert_eq!(
            split,
            [text(b"ab"), Piece::PixelMouseQuery, text(b"cd")],
            "the query is taken out of the text"
        );
    }

    #[rstest::rstest]
    #[case::set(b"\x1b[?1016h", true)]
    #[case::reset(b"\x1b[?1016l", false)]
    #[case::among_others(b"\x1b[?1003;1006;1016h", true)]
    fn pixel_mouse_switch_stays_in_the_text(#[case] switch: &[u8], #[case] on: bool) {
        // Given a mode switch naming pixel mouse reports.

        // When splitting it.
        let split = pieces(&[switch]);

        // Then it stays text and a marker follows.
        assert_eq!(
            split,
            [text(switch), Piece::PixelMouse(on)],
            "the switch is text, then marked"
        );
    }

    #[rstest::rstest]
    fn other_escapes_pass_through_unchanged() {
        // Given colours, text and an alternate-screen switch.
        let output = b"\x1b[1;31mhi\x1b[0m\x1b[?1049h";

        // When splitting it in one read.
        let split = Splitter::default().split(output);

        // Then it comes out as one unchanged text.
        assert_eq!(split, [text(output)], "one run of text");
    }

    #[rstest::rstest]
    fn long_escape_cut_across_reads_stays_text() {
        // Given a colour sequence longer than the carry limit, cut in its
        // middle.
        let output = [b"a\x1b[".as_slice(), &b"1;".repeat(20), b"31mb"].concat();
        let (first, second) = output.split_at_checked(30).unwrap_or_default();

        // When splitting the two reads.
        let split = pieces(&[first, second]);

        // Then every byte comes out as text, in order.
        assert_eq!(split, [text(&output)], "a long sequence is text");
    }

    #[rstest::rstest]
    fn graphics_command_past_the_limit_is_dropped_whole() {
        // Given a graphics command one byte over the limit, ended in a
        // second read that goes on with text.
        let first = [b"a\x1b_G".as_slice(), &vec![b'A'; MAX_GRAPHICS + 1]].concat();
        let second = b"AAA\x1b\\b";

        // When splitting the two reads.
        let split = pieces(&[&first, second]);

        // Then only the text around it comes out.
        assert_eq!(split, [text(b"ab")], "the oversized command is dropped");
    }

    /// A command start mark for `program`.
    fn command_start(program: &str) -> Piece<'static> {
        Piece::PromptMark(Mark::CommandStart {
            program: Some(program.to_owned()),
        })
    }

    #[rstest::rstest]
    #[case::prompt_start(b"\x1b]133;A;click_events=1\x1b\\", Mark::PromptStart)]
    #[case::input_start(b"\x1b]133;B\x1b\\", Mark::InputStart)]
    #[case::command_start(
        b"\x1b]133;C;cmdline_url=echo%20hi%3B%20echo%20there\x1b\\",
        Mark::CommandStart { program: Some("echo".to_owned()) }
    )]
    #[case::command_end(b"\x1b]133;D;0\x1b\\", Mark::CommandEnd { exit: Some(0) })]
    #[case::command_end_without_exit(b"\x1b]133;D\x1b\\", Mark::CommandEnd { exit: None })]
    #[case::bel_ended(b"\x1b]133;D;2\x07", Mark::CommandEnd { exit: Some(2) })]
    fn prompt_marks_stay_in_the_text_and_are_marked(#[case] seq: &[u8], #[case] mark: Mark) {
        // Given text around a shell's prompt mark.
        let output = [b"a", seq, b"b"].concat();

        // When splitting it.
        let split = pieces(&[&output]);

        // Then the mark stays in the text before it and a marker follows.
        assert_eq!(
            split,
            [
                text(&[b"a", seq].concat()),
                Piece::PromptMark(mark),
                text(b"b")
            ],
            "the mark is text, then marked"
        );
    }

    #[rstest::rstest]
    fn long_command_start_cut_anywhere_comes_out_once() {
        // Given a command start mark over 100 bytes long.
        let url = "cargo%20test%20--workspace%20".repeat(5);
        let seq = format!("\x1b]133;C;cmdline_url={url}\x1b\\");
        let sample = [b"a", seq.as_bytes(), b"b"].concat();

        for cut in 0..=sample.len() {
            // When it arrives in two reads cut at `cut`.
            let (first, second) = sample.split_at_checked(cut).unwrap_or_default();
            let split = pieces(&[first, second]);

            // Then the whole text comes out once, with one mark after the
            // sequence.
            assert_eq!(
                split,
                [
                    text(&[b"a", seq.as_bytes()].concat()),
                    command_start("cargo"),
                    text(b"b")
                ],
                "pieces for the cut at byte {cut}"
            );
        }
    }

    #[rstest::rstest]
    fn command_start_names_its_programs_first_word() {
        // Given a command start for `vim notes.md`.
        let seq = b"\x1b]133;C;cmdline_url=vim%20notes.md\x1b\\";

        // When splitting it.
        let split = pieces(&[seq]);

        // Then the mark names vim only.
        assert_eq!(
            split,
            [text(seq), command_start("vim")],
            "the program is the command line's first word"
        );
    }

    #[rstest::rstest]
    #[case::one_read(None)]
    #[case::two_reads(Some(100))]
    fn command_start_past_its_cap_is_still_marked(#[case] cut: Option<usize>) {
        // Given a command start with no terminator in its first 4 KiB.
        let start = b"\x1b]133;C;cmdline_url=";
        let output = [start.as_slice(), &[b'x'; MAX_MARK], b"\x1b\\b"].concat();
        let reads = match cut.and_then(|cut| output.split_at_checked(cut)) {
            Some((first, second)) => vec![first, second],
            None => vec![output.as_slice()],
        };

        // When splitting it.
        let split = pieces(&reads);

        // Then it is marked from what arrived, and every byte is still text.
        let (marked, rest) = output.split_at_checked(MAX_MARK).unwrap_or_default();
        let program = "x".repeat(MAX_MARK - start.len());
        assert_eq!(
            split,
            [text(marked), command_start(&program), text(rest)],
            "a mark past its cap is marked, then the rest is text"
        );
    }

    #[rstest::rstest]
    fn mark_ended_by_another_escape_cut_anywhere_keeps_that_escape() {
        // Given an input start ended by a synchronized-update start instead
        // of its own terminator.
        let sample = b"\x1b]133;B\x1b[?2026hb";

        for cut in 0..=sample.len() {
            // When it arrives in two reads cut at `cut`.
            let (first, second) = sample.split_at_checked(cut).unwrap_or_default();
            let split = pieces(&[first, second]);

            // Then the mark ends before that escape, which is still picked
            // out.
            assert_eq!(
                split,
                [
                    text(b"\x1b]133;B"),
                    Piece::PromptMark(Mark::InputStart),
                    Piece::SyncStart,
                    text(b"b")
                ],
                "pieces for the cut at byte {cut}"
            );
        }
    }

    #[rstest::rstest]
    #[case::title(b"\x1b]0;fish\x07")]
    #[case::unknown_mark(b"\x1b]133;X\x1b\\")]
    #[case::other_number(b"\x1b]1337;A\x1b\\")]
    fn other_osc_sequences_stay_text(#[case] seq: &[u8]) {
        // Given text around an OSC that isn't a prompt mark.
        let output = [b"a", seq, b"b"].concat();

        // When splitting it.
        let split = pieces(&[&output]);

        // Then all of it is text.
        assert_eq!(split, [text(&output)], "only prompt marks are marked");
    }

    #[rstest::rstest]
    #[case::other_number(b"\x1b]1337;A\x1b\\")]
    #[case::unknown_mark(b"\x1b]133;X\x1b\\")]
    #[case::sync_after_osc(b"\x1b]13\x1b[?2026h")]
    fn other_osc_sequences_cut_anywhere_stay_text(#[case] seq: &[u8]) {
        // Given text around an OSC that starts like a prompt mark but isn't
        // one.
        let output = [b"a", seq, b"b"].concat();

        for cut in 0..=output.len() {
            // When it arrives in two reads cut at `cut`.
            let (first, second) = output.split_at_checked(cut).unwrap_or_default();
            let split = pieces(&[first, second]);

            // Then every byte comes out once, in order, as text, with any
            // sequence after it still picked out.
            let expected = match seq {
                b"\x1b]13\x1b[?2026h" => vec![text(b"a\x1b]13"), Piece::SyncStart, text(b"b")],
                _ => vec![text(&output)],
            };
            assert_eq!(split, expected, "pieces for the cut at byte {cut}");
        }
    }
}
