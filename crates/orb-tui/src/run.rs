//! The frontend loop: wait for input or pane activity, apply it, draw once.
//!
//! Terminal input and the pane's background threads all feed one channel. The
//! loop sleeps until something arrives, handles everything pending, and then
//! draws a single frame, so there is no fixed tick and bursts of output cost
//! one redraw. While attached, input goes straight to the pane's child; only
//! the keys bound in [`keymap`] become intents.

use std::ffi::OsString;
use std::io::{self, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Instant;

use error_stack::{Report, ResultExt};
use orb_domain::{AppState, Command, Focus, IntentHandler};
use orb_term::{Pane, PaneCommand, PaneEvent, PaneSize};
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::{DefaultTerminal, Frame};
use wherror::Error;

use crate::child_env::child_env;
use crate::keymap::{self, Route};
use crate::outer_terminal;

/// The frontend loop failed to draw a frame or read a terminal event.
#[derive(Debug, Error)]
#[error(debug)]
pub struct TuiRunError;

/// Run orb's TUI until the user quits. Attached sessions run with an
/// environment derived from `parent_env`. The terminal is restored on exit and
/// on panic.
///
/// # Errors
///
/// Returns [`TuiRunError`] if drawing a frame or reading a terminal event fails.
pub fn run(parent_env: Vec<(OsString, OsString)>) -> Result<(), Report<TuiRunError>> {
    let env = child_env(parent_env);
    ratatui::run(|terminal| -> io::Result<()> {
        outer_terminal::enable(terminal.backend_mut())?;
        outer_terminal::install_panic_hook();
        let (tx, rx) = mpsc::channel();
        let result = App::new(env, tx).run(terminal, &rx);
        let restored = outer_terminal::disable(terminal.backend_mut());
        result.and(restored)
    })
    .change_context(TuiRunError)
}

/// Something that wakes the loop.
enum LoopEvent {
    Input(Event),
    InputFailed(io::Error),
    Pane(PaneEvent),
}

struct App {
    state: AppState,
    pane: Option<Pane>,
    /// Shown in place of the pane when it couldn't start.
    status: Option<String>,
    /// The environment attached sessions run with.
    env: Vec<(OsString, OsString)>,
    tx: Sender<LoopEvent>,
    pane_area: Rect,
    /// The cursor style last sent to the outer terminal.
    cursor_style: SetCursorStyle,
}

impl App {
    fn new(env: Vec<(OsString, OsString)>, tx: Sender<LoopEvent>) -> Self {
        Self {
            state: AppState::default(),
            pane: None,
            status: None,
            env,
            tx,
            pane_area: Rect::default(),
            cursor_style: SetCursorStyle::DefaultUserShape,
        }
    }

    fn run(mut self, terminal: &mut DefaultTerminal, rx: &Receiver<LoopEvent>) -> io::Result<()> {
        spawn_input_thread(self.tx.clone())?;
        loop {
            let [pane_area, _] = layout(terminal.size()?.into());
            self.pane_area = pane_area;
            if let Some(pane) = &mut self.pane {
                pane.resize(PaneSize::from(pane_area));
            }
            terminal.draw(|frame| {
                render(
                    frame,
                    &self.state,
                    self.pane.as_ref(),
                    self.status.as_deref(),
                );
            })?;
            self.mirror_cursor_style(terminal.backend_mut())?;
            if self.state.should_quit {
                return Ok(());
            }
            let first = match self.pane.as_ref().and_then(Pane::sync_deadline) {
                Some(deadline) => {
                    match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                        Ok(event) => Some(event),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => return Ok(()),
                    }
                }
                None => match rx.recv() {
                    Ok(event) => Some(event),
                    Err(_) => return Ok(()),
                },
            };
            for event in first.into_iter().chain(rx.try_iter()) {
                self.handle(event, terminal.backend_mut())?;
            }
            if let Some(pane) = &self.pane {
                pane.flush_expired_sync(Instant::now());
            }
        }
    }

    /// The pane that receives input: present, and the user is attached to it.
    fn attached_pane(&self) -> Option<&Pane> {
        self.pane
            .as_ref()
            .filter(|_| self.state.focus == Focus::Attached)
    }

    fn handle<W>(&mut self, event: LoopEvent, out: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        match event {
            LoopEvent::Input(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                match keymap::route(key, self.state.focus) {
                    Route::Intent(intent) => {
                        for command in IntentHandler::handle(&intent, &mut self.state) {
                            self.execute(&command, out)?;
                        }
                    }
                    Route::Forward => {
                        if let Some(pane) = self.attached_pane() {
                            pane.key(&key);
                        }
                    }
                    Route::Ignore => {}
                }
            }
            LoopEvent::Input(Event::Paste(text)) => {
                if let Some(pane) = self.attached_pane() {
                    pane.paste(&text);
                }
            }
            LoopEvent::Input(Event::Mouse(mouse)) => {
                if let Some(pane) = self.attached_pane() {
                    pane.mouse(mouse, self.pane_area);
                }
            }
            LoopEvent::Input(Event::FocusGained) => {
                if let Some(pane) = self.attached_pane() {
                    pane.focus(true);
                }
            }
            LoopEvent::Input(Event::FocusLost) => {
                if let Some(pane) = self.attached_pane() {
                    pane.focus(false);
                }
            }
            // Resize and the rest: the next iteration lays out and redraws.
            LoopEvent::Input(_) | LoopEvent::Pane(PaneEvent::Output) => {}
            LoopEvent::InputFailed(error) => return Err(error),
            LoopEvent::Pane(PaneEvent::Clipboard(text)) => {
                outer_terminal::copy_to_clipboard(out, &text)?;
            }
            LoopEvent::Pane(PaneEvent::Exited) => {
                self.state.focus = Focus::Preview;
                self.leave_pane(out)?;
            }
        }
        Ok(())
    }

    fn execute<W>(&mut self, command: &Command, out: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        match command {
            Command::Attach(target) => {
                if self.pane.as_ref().is_none_or(Pane::has_exited) {
                    let tx = self.tx.clone();
                    let command = PaneCommand {
                        argv: target.argv.clone(),
                        cwd: target.cwd.clone(),
                        env: self.env.clone(),
                    };
                    let spawned =
                        Pane::spawn(&command, PaneSize::from(self.pane_area), move |event| {
                            let _ = tx.send(LoopEvent::Pane(event));
                        });
                    match spawned {
                        Ok(pane) => {
                            self.pane = Some(pane);
                            self.status = None;
                        }
                        Err(_) => {
                            self.status = Some("couldn't start claude attach".to_owned());
                            self.state.focus = Focus::Preview;
                            return Ok(());
                        }
                    }
                }
                if let Some(pane) = &self.pane {
                    pane.focus(true);
                }
                outer_terminal::set_mouse_capture(out, true)
            }
            Command::Detach => {
                if let Some(pane) = &self.pane {
                    pane.focus(false);
                }
                self.leave_pane(out)
            }
            Command::CreateSession | Command::RefreshSessions => Ok(()),
        }
    }

    /// Gives the mouse and the cursor shape back to orb.
    fn leave_pane<W>(&mut self, out: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        outer_terminal::set_mouse_capture(out, false)?;
        self.cursor_style = SetCursorStyle::DefaultUserShape;
        outer_terminal::set_cursor_style(out, self.cursor_style)
    }

    /// Shows the child's cursor shape in the outer terminal while attached.
    fn mirror_cursor_style<W>(&mut self, out: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        let Some(style) = self.attached_pane().map(Pane::cursor_style) else {
            return Ok(());
        };
        if style == self.cursor_style {
            return Ok(());
        }
        self.cursor_style = style;
        outer_terminal::set_cursor_style(out, style)
    }
}

/// Reads terminal events on a background thread and sends them to the loop.
fn spawn_input_thread(tx: Sender<LoopEvent>) -> io::Result<()> {
    thread::Builder::new()
        .name("orb-input".into())
        .spawn(move || {
            loop {
                match event::read() {
                    Ok(event) => {
                        if tx.send(LoopEvent::Input(event)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = tx.send(LoopEvent::InputFailed(error));
                        break;
                    }
                }
            }
        })
        .map(drop)
}

/// Splits the screen into the pane and the mode line on the bottom row.
fn layout(area: Rect) -> [Rect; 2] {
    Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area)
}

/// Draws the pane (or why it couldn't start) and the mode line.
fn render(frame: &mut Frame, state: &AppState, pane: Option<&Pane>, status: Option<&str>) {
    let [pane_area, mode_line] = layout(frame.area());
    match (pane, status) {
        (Some(pane), _) => {
            if let Some(cursor) = pane.render(pane_area, frame.buffer_mut())
                && state.focus == Focus::Attached
            {
                frame.set_cursor_position(cursor);
            }
        }
        (None, Some(status)) => frame.render_widget(status, pane_area),
        (None, None) => {}
    }
    let mode = match state.focus {
        Focus::Attached => "ATTACHED   <C-\\> back",
        Focus::Sidebar | Focus::Preview => "NORMAL   ⏎ attach · q quit",
    };
    frame.render_widget(mode, mode_line);
}

#[cfg(test)]
mod tests {
    use orb_domain::{AppState, Focus};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Cell;

    use super::render;

    #[rstest::rstest]
    fn mode_line_shows_normal_on_last_row() {
        // Given a 40x5 test terminal and orb in Sidebar focus.
        let Ok(mut terminal) = Terminal::new(TestBackend::new(40, 5));
        let state = AppState::default();

        // When drawing a frame.
        let Ok(_) = terminal.draw(|frame| render(frame, &state, None, None));

        // Then the bottom row is the mode line.
        let buffer = terminal.backend().buffer();
        let last_row: String = (0..buffer.area.width)
            .filter_map(|x| buffer.cell((x, buffer.area.height - 1)).map(Cell::symbol))
            .collect();
        assert!(last_row.starts_with("NORMAL"), "last row was '{last_row}'");
    }

    #[rstest::rstest]
    fn mode_line_shows_attached_while_attached() {
        // Given a 40x5 test terminal and orb attached to a session.
        let Ok(mut terminal) = Terminal::new(TestBackend::new(40, 5));
        let state = AppState {
            focus: Focus::Attached,
            ..AppState::default()
        };

        // When drawing a frame.
        let Ok(_) = terminal.draw(|frame| render(frame, &state, None, None));

        // Then the mode line says so.
        let buffer = terminal.backend().buffer();
        let last_row: String = (0..buffer.area.width)
            .filter_map(|x| buffer.cell((x, buffer.area.height - 1)).map(Cell::symbol))
            .collect();
        assert!(
            last_row.starts_with("ATTACHED"),
            "last row was '{last_row}'"
        );
    }
}
