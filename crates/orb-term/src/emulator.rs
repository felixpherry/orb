//! The pane's terminal emulator: the child's screen, its terminal modes, and
//! the replies it expects.
//!
//! Output from the child updates the screen. Queries the child sends (device
//! attributes, kitty keyboard flags, mode reports) are answered straight back
//! to the child, and clipboard copies are reported to the caller. A
//! synchronized update stays hidden until the child ends it or its deadline
//! passes and the caller flushes it. User input is encoded for the modes the
//! child currently has on.
//!
//! The screen keeps a history the wheel scrolls back through, and a text
//! selection the mouse drags out and copies. Keys and pastes bring the view
//! back to the live screen and drop the selection.

use std::io::Write;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::{ClipboardType, Config, Term, TermMode, viewport_to_point};
use alacritty_terminal::vte::ansi::Processor;
use ratatui::buffer::Buffer;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::encode::{encode_focus, encode_key, encode_mouse, encode_paste};
use crate::pane::{PaneEvent, PaneSize};
use crate::render::{cursor_style, render_term};

/// Lines of output a pane keeps above its screen.
const HISTORY: usize = 10_000;
/// Lines one wheel notch scrolls, or arrow keys it sends.
const WHEEL_LINES: i32 = 3;
/// Presses on one cell closer together than this are a double or triple
/// click (the sidebar's double-click window).
const MULTI_CLICK: Duration = Duration::from_millis(500);
/// How often dragging past an edge may scroll the view by a line.
const EDGE_SCROLL: Duration = Duration::from_millis(10);
/// What ends a double-click's word: whitespace and brackets.
const WORD_ENDS: &str = " \t[]{}<>()";

/// Called with everything the pane reports to its owner.
pub(crate) type Notify = Arc<dyn Fn(PaneEvent) + Send + Sync>;

/// The child's input, shared by the emulator (user input) and its listener
/// (query replies).
type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// Receives what the terminal emits while it processes the child's output.
struct Listener {
    writer: SharedWriter,
    notify: Notify,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(reply) => write_to(&self.writer, reply.as_bytes()),
            Event::ClipboardStore(ClipboardType::Clipboard, text) => {
                (self.notify)(PaneEvent::Clipboard(text));
            }
            _ => {}
        }
    }
}

/// The child's screen and modes, plus the writer that reaches its input.
pub(crate) struct Emulator {
    term: Term<Listener>,
    parser: Processor,
    writer: SharedWriter,
    /// The last press that started a selection, for counting double and
    /// triple clicks.
    press: Option<Press>,
    /// When dragging past an edge last scrolled the view.
    edge_scrolled: Option<Instant>,
}

/// A press that started a selection: its cell, when, and how many quick
/// presses on that cell it ends (1 to 3).
#[derive(Clone, Copy)]
struct Press {
    cell: Point,
    at: Instant,
    count: u8,
}

impl Emulator {
    /// A blank screen of `size` whose replies and input go to `writer`.
    pub(crate) fn new(size: PaneSize, writer: Box<dyn Write + Send>, notify: Notify) -> Self {
        let writer: SharedWriter = Arc::new(Mutex::new(writer));
        let config = Config {
            scrolling_history: HISTORY,
            semantic_escape_chars: WORD_ENDS.to_owned(),
            kitty_keyboard: true,
            ..Config::default()
        };
        let listener = Listener {
            writer: writer.clone(),
            notify,
        };
        Self {
            term: Term::new(config, &size, listener),
            parser: Processor::new(),
            writer,
            press: None,
            edge_scrolled: None,
        }
    }

    /// Processes output from the child.
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// Reflows the screen to `size`.
    pub(crate) fn resize(&mut self, size: PaneSize) {
        self.term.resize(size);
    }

    /// When the pending synchronized update must be shown even if the child
    /// never ends it; `None` when no update is pending.
    pub(crate) fn sync_deadline(&self) -> Option<Instant> {
        self.parser.sync_timeout().sync_timeout()
    }

    /// Shows the pending synchronized update if its deadline is at or before
    /// `now`.
    pub(crate) fn flush_expired_sync(&mut self, now: Instant) {
        if self.sync_deadline().is_some_and(|deadline| deadline <= now) {
            self.parser.stop_sync(&mut self.term);
        }
    }

    /// Sends `key` to the child, after returning the view to the bottom and
    /// clearing the selection.
    pub(crate) fn key(&mut self, key: &KeyEvent) {
        self.settle_view();
        self.write(&encode_key(key, *self.term.mode()));
    }

    /// Sends pasted `text` to the child, after returning the view to the
    /// bottom and clearing the selection.
    pub(crate) fn paste(&mut self, text: &str) {
        self.settle_view();
        self.write(&encode_paste(text, *self.term.mode()));
    }

    /// Input reaches the child: show its live screen without a selection.
    fn settle_view(&mut self) {
        self.term.selection = None;
        self.term.scroll_display(Scroll::Bottom);
    }

    /// Types `line` and Enter into the child as raw input, never as a
    /// bracketed paste, so a shell runs it.
    pub(crate) fn type_line(&self, line: &str) {
        self.write(format!("{line}\r").as_bytes());
    }

    /// Sends a mouse event over the pane drawn at `area` to the child. A
    /// press clears the selection.
    pub(crate) fn mouse(&mut self, event: MouseEvent, area: Rect) {
        if matches!(event.kind, MouseEventKind::Down(_)) {
            self.term.selection = None;
        }
        self.write(&encode_mouse(event, area, *self.term.mode()));
    }

    /// Tells the child the pane gained or lost focus.
    pub(crate) fn focus(&self, focused: bool) {
        self.write(&encode_focus(focused, *self.term.mode()));
    }

    /// Whether the child asked for mouse reports.
    pub(crate) fn reads_mouse(&self) -> bool {
        self.term.mode().intersects(TermMode::MOUSE_MODE)
    }

    /// A wheel notch over the pane drawn at `area`: a wheel report while
    /// the child reads the mouse, else arrow keys on the alternate screen,
    /// else a scroll of the history.
    pub(crate) fn wheel(&mut self, event: MouseEvent, area: Rect) {
        let up = match event.kind {
            MouseEventKind::ScrollUp => true,
            MouseEventKind::ScrollDown => false,
            _ => return,
        };
        if self.reads_mouse() {
            self.mouse(event, area);
        } else if self.term.mode().contains(TermMode::ALT_SCREEN) {
            let code = if up { KeyCode::Up } else { KeyCode::Down };
            let arrow = KeyEvent::new(code, KeyModifiers::NONE);
            for _ in 0..WHEEL_LINES {
                self.key(&arrow);
            }
        } else {
            self.scroll(if up { WHEEL_LINES } else { -WHEEL_LINES });
        }
    }

    /// Moves the view `lines` into the history (down toward the live screen
    /// when negative).
    fn scroll(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }

    /// Starts a selection at `at` in the pane drawn at `area`: a word on
    /// the second quick press on the same cell, its line on the third, else
    /// a plain selection that grows as the mouse drags.
    pub(crate) fn select_start(&mut self, at: Position, area: Rect, now: Instant) {
        let (cell, side) = self.cell(at, area);
        let count = match self.press {
            Some(last) if last.cell == cell && now.duration_since(last.at) < MULTI_CLICK => {
                last.count % 3 + 1
            }
            _ => 1,
        };
        self.press = Some(Press {
            cell,
            at: now,
            count,
        });
        let kind = match count {
            2 => SelectionType::Semantic,
            3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        self.term.selection = Some(Selection::new(kind, cell, side));
    }

    /// Extends the selection to `at`. Past the top or bottom edge of `area`
    /// the view first scrolls a line that way, at most every 10 ms.
    pub(crate) fn select_update(&mut self, at: Position, area: Rect, now: Instant) {
        let lines = match at.y {
            y if y < area.top() => 1,
            y if y >= area.bottom() => -1,
            _ => 0,
        };
        if lines != 0
            && self
                .edge_scrolled
                .is_none_or(|then| now.duration_since(then) >= EDGE_SCROLL)
        {
            self.scroll(lines);
            self.edge_scrolled = Some(now);
        }
        let (cell, side) = self.cell(at, area);
        if let Some(selection) = &mut self.term.selection {
            selection.update(cell, side);
        }
    }

    /// Ends the selection, which stays highlighted, and returns its text:
    /// lines joined by `\n`, no newline at the end. `None` when nothing is
    /// selected.
    pub(crate) fn select_finish(&mut self) -> Option<String> {
        self.edge_scrolled = None;
        let text = self.term.selection_to_string()?;
        let text = text.trim_end_matches('\n');
        (!text.is_empty()).then(|| text.to_owned())
    }

    /// The grid cell under `at` in the pane drawn at `area`, clamped into
    /// the pane, and the side of it a selection boundary takes: the left
    /// side, so the cell under a drag's end isn't included, except past the
    /// right edge, where the last column is.
    fn cell(&self, at: Position, area: Rect) -> (Point, Side) {
        let row =
            at.y.clamp(area.top(), area.bottom().saturating_sub(1).max(area.top())) - area.y;
        let (column, side) = match at.x {
            x if x >= area.right() => (area.width.saturating_sub(1), Side::Right),
            x => (x.saturating_sub(area.x), Side::Left),
        };
        let row = usize::from(row).min(self.term.screen_lines().saturating_sub(1));
        let column = usize::from(column).min(self.term.columns().saturating_sub(1));
        let viewport = Point::new(row, Column(column));
        (
            viewport_to_point(self.term.grid().display_offset(), viewport),
            side,
        )
    }

    /// Draws the screen into `area` of `buf`; returns the visible cursor's
    /// position.
    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) -> Option<Position> {
        render_term(&self.term, area, buf)
    }

    /// The cursor style the child asked for.
    pub(crate) fn cursor_style(&self) -> SetCursorStyle {
        cursor_style(&self.term)
    }

    fn write(&self, bytes: &[u8]) {
        write_to(&self.writer, bytes);
    }
}

/// Locks `mutex`, recovering the data if a previous holder panicked.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Writes `bytes` to the child's input. A child that stopped reading is not
/// an error the pane can act on, so write failures are dropped.
fn write_to(writer: &SharedWriter, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let mut writer = lock(writer);
    let _ = writer.write_all(bytes).and_then(|()| writer.flush());
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use ratatui::buffer::{Buffer, Cell};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
    use ratatui::layout::{Position, Rect};
    use ratatui::style::Modifier;

    use super::{Emulator, lock};
    use crate::pane::{PaneEvent, PaneSize};

    /// An in-memory child input that tests can read back.
    #[derive(Clone, Default)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl SharedBuf {
        fn contents(&self) -> Vec<u8> {
            lock(&self.0).clone()
        }
    }

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            lock(&self.0).extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// A 20×5 emulator, what it wrote to the child, and what it reported.
    fn emulator() -> (Emulator, SharedBuf, Arc<Mutex<Vec<PaneEvent>>>) {
        let input = SharedBuf::default();
        let events = Arc::new(Mutex::new(Vec::new()));
        let notify = {
            let events = events.clone();
            Arc::new(move |event| lock(&events).push(event))
        };
        let size = PaneSize { cols: 20, rows: 5 };
        (
            Emulator::new(size, Box::new(input.clone()), notify),
            input,
            events,
        )
    }

    /// Where the 20×5 emulator is drawn: at the top left of the frame.
    const AREA: Rect = Rect::new(0, 0, 20, 5);

    /// Where edge-scroll tests draw the emulator: one row down, so there is
    /// a row above it.
    const LOWERED: Rect = Rect::new(0, 1, 20, 5);

    /// The emulator's screen drawn into [`AREA`].
    fn screen(emulator: &Emulator) -> Buffer {
        let mut buf = Buffer::empty(AREA);
        emulator.render(AREA, &mut buf);
        buf
    }

    /// The symbols of the emulator's row `y`, concatenated, without
    /// trailing blanks.
    fn row(emulator: &Emulator, y: u16) -> String {
        let buf = screen(emulator);
        let row: String = (0..AREA.width)
            .filter_map(|x| buf.cell((x, y)))
            .map(Cell::symbol)
            .collect();
        row.trim_end().to_owned()
    }

    /// The symbols of the emulator's first row, concatenated.
    fn first_row(emulator: &Emulator) -> String {
        row(emulator, 0)
    }

    /// Whether the cells `x` of row `y` are all drawn in reverse video.
    fn reversed(emulator: &Emulator, xs: std::ops::Range<u16>, y: u16) -> bool {
        let buf = screen(emulator);
        xs.into_iter().all(|x| {
            buf.cell((x, y))
                .is_some_and(|cell| cell.modifier.contains(Modifier::REVERSED))
        })
    }

    /// A child that printed `line1` to `line9`: the screen shows `line5`
    /// to `line9`, the history holds the rest.
    fn nine_lines() -> (Emulator, SharedBuf) {
        let (mut emulator, input, _) = emulator();
        let lines: Vec<String> = (1..=9).map(|n| format!("line{n}")).collect();
        emulator.feed(lines.join("\r\n").as_bytes());
        (emulator, input)
    }

    /// A wheel notch at the top left of the pane.
    fn wheel(kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// One wheel notch up over the pane drawn at [`AREA`].
    fn wheel_up(emulator: &mut Emulator) {
        emulator.wheel(wheel(MouseEventKind::ScrollUp), AREA);
    }

    /// A drag from `from` to `to` in the pane drawn at [`AREA`].
    fn drag(emulator: &mut Emulator, from: (u16, u16), to: (u16, u16)) {
        let now = Instant::now();
        emulator.select_start(Position::new(from.0, from.1), AREA, now);
        emulator.select_update(Position::new(to.0, to.1), AREA, now);
    }

    /// `count` quick presses on `at`, 100 ms apart.
    fn clicks(emulator: &mut Emulator, at: (u16, u16), count: u32) {
        let start = Instant::now();
        for n in 0..count {
            let now = start + Duration::from_millis(100) * n;
            emulator.select_start(Position::new(at.0, at.1), AREA, now);
        }
    }

    #[rstest::rstest]
    fn da1_query_is_answered() {
        // Given a fresh emulator.
        let (mut emulator, input, _) = emulator();

        // When the child asks for the primary device attributes.
        emulator.feed(b"\x1b[c");

        // Then the child receives a VT102 reply.
        assert_eq!(input.contents(), b"\x1b[?6c", "DA1 reply");
    }

    #[rstest::rstest]
    fn typed_line_reaches_the_child_with_enter() {
        // Given a fresh emulator.
        let (emulator, input, _) = emulator();

        // When typing a resume command.
        emulator.type_line("claude --resume aa");

        // Then the child receives the line and a carriage return.
        assert_eq!(input.contents(), b"claude --resume aa\r", "typed line");
    }

    #[rstest::rstest]
    fn typed_line_ignores_bracketed_paste() {
        // Given a child that turned bracketed paste on.
        let (mut emulator, input, _) = emulator();
        emulator.feed(b"\x1b[?2004h");

        // When typing a resume command.
        emulator.type_line("claude --resume aa");

        // Then the line reaches the child unwrapped.
        assert_eq!(input.contents(), b"claude --resume aa\r", "unwrapped line");
    }

    #[rstest::rstest]
    fn kitty_keyboard_query_is_answered() {
        // Given a fresh emulator.
        let (mut emulator, input, _) = emulator();

        // When the child asks for the kitty keyboard flags.
        emulator.feed(b"\x1b[?u");

        // Then the child learns kitty is supported with no flags set.
        assert_eq!(input.contents(), b"\x1b[?0u", "kitty flags reply");
    }

    #[rstest::rstest]
    fn child_kitty_push_switches_keys_to_kitty_encoding() {
        // Given a child that pushed kitty flags 5 (disambiguate + alternate keys).
        let (mut emulator, input, _) = emulator();
        emulator.feed(b"\x1b[>5u");

        // When the user presses Shift+Enter.
        emulator.key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));

        // Then the child receives the kitty encoding.
        assert!(
            input.contents().ends_with(b"\x1b[13;2u"),
            "Shift+Enter should be kitty-encoded, got {:?}",
            String::from_utf8_lossy(&input.contents())
        );
    }

    #[rstest::rstest]
    fn synchronized_text_is_hidden_until_the_update_ends() {
        // Given a fresh emulator.
        let (mut emulator, _, _) = emulator();

        // When the child starts a synchronized update and writes text.
        emulator.feed(b"\x1b[?2026hhi");

        // Then the text isn't on screen yet.
        assert!(
            !first_row(&emulator).contains("hi"),
            "text inside an open synchronized update should be hidden"
        );
    }

    #[rstest::rstest]
    fn synchronized_text_appears_once_the_deadline_passes() {
        // Given a synchronized update the child never ends.
        let (mut emulator, _, _) = emulator();
        emulator.feed(b"\x1b[?2026hhi");

        // When flushing well after its deadline.
        emulator.flush_expired_sync(Instant::now() + Duration::from_secs(1));

        // Then the text is on screen.
        assert!(
            first_row(&emulator).contains("hi"),
            "an expired synchronized update should be shown"
        );
    }

    #[rstest::rstest]
    fn clipboard_write_is_reported() {
        // Given a fresh emulator.
        let (mut emulator, _, events) = emulator();

        // When the child copies "hello" to the clipboard with OSC 52.
        emulator.feed(b"\x1b]52;c;aGVsbG8=\x07");

        // Then the copy is reported.
        assert_eq!(
            *lock(&events),
            [PaneEvent::Clipboard("hello".to_owned())],
            "OSC 52 copy should be reported"
        );
    }

    #[rstest::rstest]
    fn scrolling_up_shows_earlier_output() {
        // Given a shell that printed nine lines into a five-row pane.
        let (mut emulator, _) = nine_lines();

        // When one wheel notch scrolls up.
        wheel_up(&mut emulator);

        // Then the top row shows the line three above the screen's first.
        assert_eq!(row(&emulator, 0), "line2", "top row after one notch up");
    }

    #[rstest::rstest]
    fn history_keeps_the_last_ten_thousand_lines() {
        // Given a shell that printed 10 010 numbered lines.
        let (mut emulator, _, _) = emulator();
        let lines: Vec<String> = (1..=10_010).map(|n| n.to_string()).collect();
        emulator.feed(lines.join("\r\n").as_bytes());

        // When scrolling as far back as the history goes.
        emulator.scroll(20_000);

        // Then the oldest line kept is the one 10 000 above the screen.
        assert_eq!(row(&emulator, 0), "6", "oldest line in the history");
    }

    #[rstest::rstest]
    fn key_returns_the_view_to_the_bottom() {
        // Given a pane scrolled back into its history.
        let (mut emulator, _) = nine_lines();
        wheel_up(&mut emulator);

        // When the user types a key.
        emulator.key(&KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));

        // Then the live screen is shown again.
        assert_eq!(row(&emulator, 0), "line5", "top row of the live screen");
    }

    #[rstest::rstest]
    #[case::mouse_tracking("\x1b[?1000h\x1b[?1006h", "\x1b[<64;1;1M")]
    #[case::alternate_screen("\x1b[?1049h", "\x1b[A\x1b[A\x1b[A")]
    #[case::alternate_screen_cursor_keys("\x1b[?1049h\x1b[?1h", "\x1bOA\x1bOA\x1bOA")]
    fn wheel_reaches_the_child_by_its_modes(#[case] modes: &str, #[case] expected: &str) {
        // Given a child that switched on `modes`.
        let (mut emulator, input, _) = emulator();
        emulator.feed(modes.as_bytes());

        // When one wheel notch scrolls up over the pane.
        wheel_up(&mut emulator);

        // Then the child receives the input those modes call for.
        assert_eq!(
            String::from_utf8_lossy(&input.contents()),
            expected,
            "wheel input for {}",
            modes.escape_debug()
        );
    }

    #[rstest::rstest]
    fn wheel_on_the_main_screen_writes_nothing_to_the_child() {
        // Given a shell on the main screen that doesn't read the mouse.
        let (mut emulator, input) = nine_lines();

        // When one wheel notch scrolls up over the pane.
        wheel_up(&mut emulator);

        // Then the child receives nothing.
        assert!(input.contents().is_empty(), "the wheel should only scroll");
    }

    #[rstest::rstest]
    #[case::clicks("\x1b[?1000h", true)]
    #[case::drags("\x1b[?1002h", true)]
    #[case::motion("\x1b[?1003h", true)]
    #[case::nothing("", false)]
    fn child_reads_the_mouse_after_asking(#[case] modes: &str, #[case] expected: bool) {
        // Given a child that switched on `modes`.
        let (mut emulator, _, _) = emulator();
        emulator.feed(modes.as_bytes());

        // When asking whether it reads the mouse.
        let reads = emulator.reads_mouse();

        // Then the answer follows its mouse tracking mode.
        assert_eq!(
            reads,
            expected,
            "reads the mouse after {}",
            modes.escape_debug()
        );
    }

    #[rstest::rstest]
    fn drag_over_two_lines_copies_their_text() {
        // Given a child that printed two lines.
        let (mut emulator, _, _) = emulator();
        emulator.feed(b"one\r\ntwo");

        // When dragging from the first line's start past the second's end.
        drag(&mut emulator, (0, 0), (3, 1));
        let text = emulator.select_finish();

        // Then both lines are copied, joined by a newline.
        assert_eq!(text.as_deref(), Some("one\ntwo"), "copied text");
    }

    #[rstest::rstest]
    fn drag_while_scrolled_back_copies_the_scrolled_lines() {
        // Given a pane scrolled back so its top row shows `line2`.
        let (mut emulator, _) = nine_lines();
        wheel_up(&mut emulator);

        // When dragging over the top row.
        drag(&mut emulator, (0, 0), (5, 0));
        let text = emulator.select_finish();

        // Then the history line shown there is copied.
        assert_eq!(text.as_deref(), Some("line2"), "copied text");
    }

    #[rstest::rstest]
    fn double_click_copies_the_word() {
        // Given a child that printed three words.
        let (mut emulator, _, _) = emulator();
        emulator.feed(b"foo bar baz");

        // When double-clicking inside the middle word.
        clicks(&mut emulator, (5, 0), 2);
        let text = emulator.select_finish();

        // Then that word is copied.
        assert_eq!(text.as_deref(), Some("bar"), "copied word");
    }

    #[rstest::rstest]
    fn triple_click_copies_the_line() {
        // Given a child that printed three words.
        let (mut emulator, _, _) = emulator();
        emulator.feed(b"foo bar baz");

        // When triple-clicking inside the middle word.
        clicks(&mut emulator, (5, 0), 3);
        let text = emulator.select_finish();

        // Then the whole line is copied.
        assert_eq!(text.as_deref(), Some("foo bar baz"), "copied line");
    }

    #[rstest::rstest]
    fn fourth_quick_click_starts_a_plain_selection() {
        // Given a child that printed three words.
        let (mut emulator, _, _) = emulator();
        emulator.feed(b"foo bar baz");

        // When clicking four times quickly on one cell.
        clicks(&mut emulator, (5, 0), 4);
        let text = emulator.select_finish();

        // Then nothing is selected, as after a single click.
        assert_eq!(text, None, "a fourth click starts over");
    }

    #[rstest::rstest]
    fn click_without_drag_copies_nothing() {
        // Given a child that printed three words.
        let (mut emulator, _, _) = emulator();
        emulator.feed(b"foo bar baz");

        // When clicking once and releasing without moving.
        clicks(&mut emulator, (5, 0), 1);
        let text = emulator.select_finish();

        // Then there is nothing to copy.
        assert_eq!(text, None, "a click selects nothing");
    }

    #[rstest::rstest]
    fn dragging_above_the_top_edge_scrolls_into_history() {
        // Given a selection started on the pane's first row.
        let (mut emulator, _) = nine_lines();
        let now = Instant::now();
        emulator.select_start(Position::new(0, 1), LOWERED, now);

        // When the drag moves above the pane.
        emulator.select_update(Position::new(0, 0), LOWERED, now);

        // Then the view scrolls one line into the history.
        assert_eq!(row(&emulator, 0), "line4", "top row after the edge scroll");
    }

    #[rstest::rstest]
    fn dragging_below_the_bottom_edge_scrolls_toward_the_live_screen() {
        // Given a pane scrolled back three lines, selecting from its last row.
        let (mut emulator, _) = nine_lines();
        emulator.wheel(wheel(MouseEventKind::ScrollUp), LOWERED);
        let now = Instant::now();
        emulator.select_start(Position::new(0, 5), LOWERED, now);

        // When the drag moves below the pane.
        emulator.select_update(Position::new(0, 6), LOWERED, now);

        // Then the view scrolls one line back toward the live screen.
        assert_eq!(row(&emulator, 0), "line3", "top row after the edge scroll");
    }

    #[rstest::rstest]
    fn edge_scroll_waits_between_lines() {
        // Given a drag that just scrolled the view past the top edge.
        let (mut emulator, _) = nine_lines();
        let now = Instant::now();
        emulator.select_start(Position::new(0, 1), LOWERED, now);
        emulator.select_update(Position::new(0, 0), LOWERED, now);

        // When the drag moves above the pane again at the same instant.
        emulator.select_update(Position::new(1, 0), LOWERED, now);

        // Then the view doesn't scroll a second line.
        assert_eq!(row(&emulator, 0), "line4", "top row after two quick moves");
    }

    #[rstest::rstest]
    fn key_clears_the_selection() {
        // Given a selection over `one`.
        let (mut emulator, _, _) = emulator();
        emulator.feed(b"one");
        drag(&mut emulator, (0, 0), (3, 0));

        // When the user types a key.
        emulator.key(&KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));

        // Then `one` is no longer highlighted.
        assert!(
            !reversed(&emulator, 0..3, 0),
            "the key should clear the selection"
        );
    }

    #[rstest::rstest]
    fn new_press_clears_the_old_selection() {
        // Given a selection over `one`.
        let (mut emulator, _, _) = emulator();
        emulator.feed(b"one two");
        drag(&mut emulator, (0, 0), (3, 0));

        // When a new press lands elsewhere in the pane.
        emulator.select_start(Position::new(10, 2), AREA, Instant::now());

        // Then `one` is no longer highlighted.
        assert!(
            !reversed(&emulator, 0..3, 0),
            "the press should replace the selection"
        );
    }

    #[rstest::rstest]
    fn finished_selection_stays_highlighted() {
        // Given a selection over `one`.
        let (mut emulator, _, _) = emulator();
        emulator.feed(b"one");
        drag(&mut emulator, (0, 0), (3, 0));

        // When the selection ends.
        emulator.select_finish();

        // Then `one` is still highlighted.
        assert!(
            reversed(&emulator, 0..3, 0),
            "the selection should stay drawn"
        );
    }
}
