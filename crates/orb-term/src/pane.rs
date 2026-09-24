//! A terminal pane: one child process running in a PTY, with its screen kept
//! up to date in the background.

use std::ffi::OsString;
use std::fmt::Display;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use alacritty_terminal::grid::Dimensions;
use error_stack::{Report, ResultExt};
use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use ratatui::buffer::Buffer;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{KeyEvent, MouseEvent};
use ratatui::layout::{Position, Rect};
use wherror::Error;

use crate::emulator::{Emulator, Notify, lock};

/// The pane's size in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneSize {
    /// Width in columns.
    pub cols: u16,
    /// Height in rows.
    pub rows: u16,
}

impl From<Rect> for PaneSize {
    /// The size of `area`, at least 2 columns by 1 row so the child always has
    /// a usable screen.
    fn from(area: Rect) -> Self {
        Self {
            cols: area.width.max(2),
            rows: area.height.max(1),
        }
    }
}

impl Dimensions for PaneSize {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn screen_lines(&self) -> usize {
        usize::from(self.rows)
    }

    fn columns(&self) -> usize {
        usize::from(self.cols)
    }
}

/// What the pane runs, where, and with which environment.
#[derive(Debug, Clone)]
pub struct PaneCommand {
    /// The program followed by its arguments.
    pub argv: Vec<OsString>,
    /// The child's working directory.
    pub cwd: PathBuf,
    /// The child's complete environment; nothing is inherited from orb.
    pub env: Vec<(OsString, OsString)>,
}

/// Something the pane reports to its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneEvent {
    /// The child wrote output; the screen may have changed.
    Output,
    /// The child copied text to the clipboard.
    Clipboard(String),
    /// The child exited.
    Exited,
}

/// The pane's child could not be started.
#[derive(Debug, Error)]
#[error(debug)]
pub struct PaneError;

/// A running child in a PTY. Its output is parsed on a background thread, so
/// the screen is current whenever the owner draws; the owner is notified of
/// output, clipboard copies, and exit.
///
/// Dropping the pane kills the child.
pub struct Pane {
    emulator: Arc<Mutex<Emulator>>,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    exited: Arc<AtomicBool>,
    size: PaneSize,
}

impl Pane {
    /// Starts `command` in a new PTY of `size`. `notify` is called from
    /// background threads with everything the pane reports.
    ///
    /// # Errors
    ///
    /// Returns [`PaneError`] if the command is empty, the PTY can't be opened,
    /// or the command can't be started.
    pub fn spawn<F>(
        command: &PaneCommand,
        size: PaneSize,
        notify: F,
    ) -> Result<Self, Report<PaneError>>
    where
        F: Fn(PaneEvent) + Send + Sync + 'static,
    {
        let Some((program, args)) = command.argv.split_first() else {
            return Err(Report::new(PaneError).attach("the pane command is empty"));
        };
        let pair = native_pty_system()
            .openpty(pty_size(size))
            .map_err(pty_error)?;
        let reader = pair.master.try_clone_reader().map_err(pty_error)?;
        let writer = spawn_writer(pair.master.take_writer().map_err(pty_error)?)?;
        let builder = {
            let mut builder = CommandBuilder::new(program);
            builder.args(args);
            builder.env_clear();
            for (key, value) in &command.env {
                builder.env(key, value);
            }
            builder.cwd(&command.cwd);
            builder
        };
        let child = pair.slave.spawn_command(builder).map_err(pty_error)?;
        // Our copy of the slave would keep the PTY open after the child exits.
        drop(pair.slave);
        let notify: Notify = Arc::new(notify);
        let pane = Self {
            emulator: Arc::new(Mutex::new(Emulator::new(size, writer, notify.clone()))),
            master: pair.master,
            killer: child.clone_killer(),
            exited: Arc::new(AtomicBool::new(false)),
            size,
        };
        // From here on, an early return drops `pane`, which kills the child.
        spawn_reader(reader, pane.emulator.clone(), notify.clone())?;
        spawn_reaper(child, pane.exited.clone(), notify)?;
        Ok(pane)
    }

    /// Resizes the screen and tells the child. Does nothing if the size is
    /// unchanged, so it's cheap to call on every frame.
    pub fn resize(&mut self, size: PaneSize) {
        if size == self.size {
            return;
        }
        self.size = size;
        lock(&self.emulator).resize(size);
        let _ = self.master.resize(pty_size(size));
    }

    /// Whether the child has exited.
    pub fn has_exited(&self) -> bool {
        self.exited.load(Ordering::SeqCst)
    }

    /// Sends `key` to the child, encoded for its current keyboard modes.
    pub fn key(&self, key: &KeyEvent) {
        lock(&self.emulator).key(key);
    }

    /// Sends pasted `text` to the child.
    pub fn paste(&self, text: &str) {
        lock(&self.emulator).paste(text);
    }

    /// Sends a mouse event to the child if it asked for mouse reports and the
    /// event happened over the pane drawn at `area`.
    pub fn mouse(&self, event: MouseEvent, area: Rect) {
        lock(&self.emulator).mouse(event, area);
    }

    /// Tells the child the pane gained or lost focus, if it asked to know.
    pub fn focus(&self, focused: bool) {
        lock(&self.emulator).focus(focused);
    }

    /// When a synchronized update the child hasn't finished must be shown
    /// anyway; `None` when there's nothing pending. The owner should call
    /// [`Pane::flush_expired_sync`] once this passes.
    pub fn sync_deadline(&self) -> Option<Instant> {
        lock(&self.emulator).sync_deadline()
    }

    /// Shows a pending synchronized update whose deadline is at or before
    /// `now`.
    pub fn flush_expired_sync(&self, now: Instant) {
        lock(&self.emulator).flush_expired_sync(now);
    }

    /// Draws the child's screen into `area` of `buf`; returns where the cursor
    /// is when the child shows it.
    pub fn render(&self, area: Rect, buf: &mut Buffer) -> Option<Position> {
        lock(&self.emulator).render(area, buf)
    }

    /// The cursor style the child asked for.
    pub fn cursor_style(&self) -> SetCursorStyle {
        lock(&self.emulator).cursor_style()
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        // Kill first: dropping the PTY writer sends the child a newline and EOF.
        let _ = self.killer.kill();
    }
}

/// Starts a thread that owns the child's input and writes everything sent to
/// the returned writer into it, in order. Writing to the returned writer never
/// blocks, so a child that stops reading can't stall orb or the thread feeding
/// the child's output.
fn spawn_writer(
    mut input: Box<dyn Write + Send>,
) -> Result<Box<dyn Write + Send>, Report<PaneError>> {
    let (sender, receiver) = mpsc::channel::<Vec<u8>>();
    thread::Builder::new()
        .name("orb-pane-writer".to_owned())
        .spawn(move || {
            for bytes in receiver {
                if input
                    .write_all(&bytes)
                    .and_then(|()| input.flush())
                    .is_err()
                {
                    break;
                }
            }
        })
        .change_context(PaneError)
        .attach("failed to start the pane writer thread")?;
    Ok(Box::new(QueuedWriter(sender)))
}

/// Hands writes to the writer thread.
struct QueuedWriter(Sender<Vec<u8>>);

impl Write for QueuedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.send(buf.to_vec()).map_err(io::Error::other)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Feeds the child's output into `emulator` until the PTY closes.
fn spawn_reader(
    mut reader: Box<dyn Read + Send>,
    emulator: Arc<Mutex<Emulator>>,
    notify: Notify,
) -> Result<(), Report<PaneError>> {
    thread::Builder::new()
        .name("orb-pane-reader".to_owned())
        .spawn(move || {
            let mut buf = vec![0; 64 * 1024];
            while let Ok(n) = reader.read(&mut buf) {
                let Some(chunk) = buf.get(..n).filter(|chunk| !chunk.is_empty()) else {
                    break;
                };
                lock(&emulator).feed(chunk);
                notify(PaneEvent::Output);
            }
        })
        .change_context(PaneError)
        .attach("failed to start the pane reader thread")?;
    Ok(())
}

/// Waits for the child to exit, then marks the pane exited. This, not the
/// reader's EOF, is the exit signal: the child's own children can keep the PTY
/// open after it exits.
fn spawn_reaper(
    mut child: Box<dyn Child + Send + Sync>,
    exited: Arc<AtomicBool>,
    notify: Notify,
) -> Result<(), Report<PaneError>> {
    thread::Builder::new()
        .name("orb-pane-reaper".to_owned())
        .spawn(move || {
            let _ = child.wait();
            exited.store(true, Ordering::SeqCst);
            notify(PaneEvent::Exited);
        })
        .change_context(PaneError)
        .attach("failed to start the pane reaper thread")?;
    Ok(())
}

fn pty_size(size: PaneSize) -> PtySize {
    PtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// portable-pty reports `anyhow` errors, which `change_context` can't take.
fn pty_error<E>(error: E) -> Report<PaneError>
where
    E: Display,
{
    Report::new(PaneError).attach(format!("{error:#}"))
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate spawn failures with `?` and assert on the outcome"
)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use error_stack::Report;
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Rect;

    use super::{Pane, PaneCommand, PaneError, PaneEvent, PaneSize};
    use crate::emulator::lock;

    const SIZE: PaneSize = PaneSize { cols: 80, rows: 24 };

    /// Runs `argv` in `/` with exactly `env`.
    fn command(argv: &[&str], env: &[(&str, &str)]) -> PaneCommand {
        PaneCommand {
            argv: argv.iter().map(Into::into).collect(),
            cwd: "/".into(),
            env: env
                .iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
        }
    }

    /// A notify callback and the events it has received so far.
    fn recorder() -> (
        Arc<Mutex<Vec<PaneEvent>>>,
        impl Fn(PaneEvent) + Send + Sync + 'static,
    ) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        (events, move |event| lock(&sink).push(event))
    }

    /// Polls `condition` every 10 ms for up to 5 s; returns whether it held.
    fn wait_until<F>(mut condition: F) -> bool
    where
        F: FnMut() -> bool,
    {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        condition()
    }

    /// The pane's screen as text, one trimmed line per row.
    fn screen(pane: &Pane) -> String {
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        pane.render(area, &mut buf);
        (0..area.height)
            .map(|y| {
                let row: String = (0..area.width)
                    .filter_map(|x| buf.cell((x, y)))
                    .map(Cell::symbol)
                    .collect();
                row.trim_end().to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[rstest::rstest]
    fn child_output_appears_in_pane() -> Result<(), Report<PaneError>> {
        // Given a child that prints "hi".
        let command = command(&["/bin/sh", "-c", "printf hi"], &[]);

        // When it runs in a pane.
        let pane = Pane::spawn(&command, SIZE, |_| {})?;

        // Then "hi" shows up on the pane's first row.
        assert!(
            wait_until(|| screen(&pane).starts_with("hi")),
            "screen should start with the child's output, got {:?}",
            screen(&pane)
        );
        Ok(())
    }

    #[rstest::rstest]
    fn child_runs_in_the_given_directory() -> Result<(), Report<PaneError>> {
        // Given `pwd` with `/` as its directory and no environment (so no
        // `$HOME` fallback applies).
        let command = command(&["/bin/pwd"], &[]);

        // When it runs in a pane.
        let pane = Pane::spawn(&command, SIZE, |_| {})?;

        // Then it prints `/`.
        assert!(
            wait_until(|| screen(&pane).lines().next() == Some("/")),
            "pwd should print /, got {:?}",
            screen(&pane)
        );
        Ok(())
    }

    #[rstest::rstest]
    fn child_sees_only_the_given_environment() -> Result<(), Report<PaneError>> {
        // Given `env` with only ORB_TEST=1 in its environment.
        let command = command(&["/usr/bin/env"], &[("ORB_TEST", "1")]);

        // When it runs in a pane and prints its environment.
        let pane = Pane::spawn(&command, SIZE, |_| {})?;
        let printed = wait_until(|| screen(&pane).contains("ORB_TEST=1"));

        // Then it has ORB_TEST, plus the SHELL portable-pty always sets, and
        // nothing inherited from orb.
        let screen = screen(&pane);
        let inherited = screen.lines().any(|line| {
            !(line.is_empty() || line.starts_with("ORB_TEST=") || line.starts_with("SHELL="))
        });
        assert!(
            printed && !inherited,
            "env should be exactly the given one, got {screen:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn resized_pane_reports_new_size_to_child() -> Result<(), Report<PaneError>> {
        // Given an 80×24 pane whose child prints its size after a line of input.
        let command = command(&["/bin/sh", "-c", "read x; /bin/stty size"], &[]);
        let mut pane = Pane::spawn(&command, SIZE, |_| {})?;

        // When the pane is resized to 100×30 and the user presses Enter.
        pane.resize(PaneSize {
            cols: 100,
            rows: 30,
        });
        pane.key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        // Then the child sees 30 rows by 100 columns.
        assert!(
            wait_until(|| screen(&pane).contains("30 100")),
            "stty should report the new size, got {:?}",
            screen(&pane)
        );
        Ok(())
    }

    #[rstest::rstest]
    fn child_exit_is_reported() -> Result<(), Report<PaneError>> {
        // Given a child that exits immediately.
        let command = command(&["/usr/bin/true"], &[]);
        let (events, notify) = recorder();

        // When it runs in a pane.
        let _pane = Pane::spawn(&command, SIZE, notify)?;

        // Then the pane reports the exit.
        assert!(
            wait_until(|| lock(&events).contains(&PaneEvent::Exited)),
            "Exited should be reported, got {:?}",
            lock(&events)
        );
        Ok(())
    }

    #[rstest::rstest]
    fn pane_has_exited_after_child_exits() -> Result<(), Report<PaneError>> {
        // Given a child that exits immediately.
        let command = command(&["/usr/bin/true"], &[]);

        // When it runs in a pane.
        let pane = Pane::spawn(&command, SIZE, |_| {})?;

        // Then the pane says its child has exited.
        assert!(
            wait_until(|| pane.has_exited()),
            "has_exited should turn true"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn large_paste_reaches_the_child() -> Result<(), Report<PaneError>> {
        // Given a child that echoes its input back while reading it.
        let command = command(&["/bin/cat"], &[]);
        let pane = Pane::spawn(&command, SIZE, |_| {})?;

        // When the user pastes about 100 KB ending in a marker line.
        let text = format!(
            "{}END\n",
            "0123456789abcdefghijklmnopqrstuvwxyz\n".repeat(2800)
        );
        pane.paste(&text);

        // Then the whole paste arrives: the marker shows up on screen.
        assert!(
            wait_until(|| screen(&pane).contains("END")),
            "the end of the paste should be echoed, got {:?}",
            screen(&pane)
        );
        Ok(())
    }
}
