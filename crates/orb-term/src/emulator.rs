//! The pane's terminal emulator: the child's screen, its terminal modes, and
//! the replies it expects.
//!
//! Output from the child updates the screen. Queries the child sends (device
//! attributes, kitty keyboard flags, mode reports) are answered straight back
//! to the child, and clipboard copies are reported to the caller. A
//! synchronized update stays hidden until the child ends it or its deadline
//! passes and the caller flushes it. User input is encoded for the modes the
//! child currently has on.

use std::io::Write;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::term::{ClipboardType, Config, Term};
use alacritty_terminal::vte::ansi::Processor;
use ratatui::buffer::Buffer;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{KeyEvent, MouseEvent};
use ratatui::layout::{Position, Rect};

use crate::encode::{encode_focus, encode_key, encode_mouse, encode_paste};
use crate::pane::{PaneEvent, PaneSize};
use crate::render::{cursor_style, render_term};

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
}

impl Emulator {
    /// A blank screen of `size` whose replies and input go to `writer`.
    pub(crate) fn new(size: PaneSize, writer: Box<dyn Write + Send>, notify: Notify) -> Self {
        let writer: SharedWriter = Arc::new(Mutex::new(writer));
        let config = Config {
            scrolling_history: 0,
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

    /// Sends `key` to the child.
    pub(crate) fn key(&self, key: &KeyEvent) {
        self.write(&encode_key(key, *self.term.mode()));
    }

    /// Sends pasted `text` to the child.
    pub(crate) fn paste(&self, text: &str) {
        self.write(&encode_paste(text, *self.term.mode()));
    }

    /// Sends a mouse event over the pane drawn at `area` to the child.
    pub(crate) fn mouse(&self, event: MouseEvent, area: Rect) {
        self.write(&encode_mouse(event, area, *self.term.mode()));
    }

    /// Tells the child the pane gained or lost focus.
    pub(crate) fn focus(&self, focused: bool) {
        self.write(&encode_focus(focused, *self.term.mode()));
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
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Rect;

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

    /// The symbols of the emulator's first row, concatenated.
    fn first_row(emulator: &Emulator) -> String {
        let area = Rect::new(0, 0, 20, 5);
        let mut buf = Buffer::empty(area);
        emulator.render(area, &mut buf);
        (0..area.width)
            .filter_map(|x| buf.cell((x, 0)))
            .map(Cell::symbol)
            .collect()
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
}
