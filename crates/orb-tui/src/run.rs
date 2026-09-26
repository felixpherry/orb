//! The frontend loop: wait for input or pane activity, apply it, draw once.
//!
//! Terminal input, the pane's background threads, and the actors' state
//! changes all feed one channel. The loop sleeps until something arrives,
//! handles everything pending, and then draws a single frame, so bursts of
//! output cost one redraw. The only tick is once a second while a thread is
//! working, so its elapsed time counts up.
//!
//! Each thread gets its own `claude attach` pane; selecting another thread
//! drops it (the session keeps running). While attached, input goes straight
//! to Claude; otherwise keys go through the [`keymap`]. The loop reads the
//! directory picker's listings itself, since a listing takes about a
//! millisecond.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use error_stack::{Report, ResultExt};
use kameo::prelude::ActorRef;
use orb_domain::feat::preview::preview_actor::{self, PreviewActor};
use orb_domain::feat::sessions::sessions_actor::{self, SessionsActor};
use orb_domain::feat::sessions::state::ThreadId;
use orb_domain::{Command, Focus, IntentHandler, State, Wake};
use orb_term::{Pane, PaneCommand, PaneEvent, PaneSize};
use ratatui::DefaultTerminal;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::layout::Rect;
use wherror::Error;

use crate::keymap::{self, Keys, Route};
use crate::picker::PickerScroll;
use crate::preview::PreviewCache;
use crate::sidebar::SidebarScroll;
use crate::{outer_terminal, render};

/// The frontend loop failed to draw a frame or read a terminal event.
#[derive(Debug, Error)]
#[error(debug)]
pub struct TuiRunError;

/// orb's TUI. Created before the actors so they can wake its loop.
pub struct Frontend {
    tx: Sender<LoopEvent>,
    rx: Receiver<LoopEvent>,
}

impl Default for Frontend {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx }
    }
}

impl Frontend {
    /// Wakes the loop to redraw after an actor changed the state.
    pub fn waker(&self) -> Wake {
        let tx = self.tx.clone();
        Arc::new(move || {
            let _ = tx.send(LoopEvent::StateChanged);
        })
    }

    /// Runs orb's TUI until the user quits. Session commands go to
    /// `sessions` and preview commands to `preview`; attached sessions run
    /// with `claude_env`. The terminal is restored on exit and on panic.
    ///
    /// # Errors
    ///
    /// Returns [`TuiRunError`] if drawing a frame or reading a terminal event
    /// fails.
    pub fn run(
        self,
        state: State,
        sessions: ActorRef<SessionsActor>,
        preview: ActorRef<PreviewActor>,
        claude_env: Vec<(OsString, OsString)>,
    ) -> Result<(), Report<TuiRunError>> {
        let Self { tx, rx } = self;
        ratatui::run(|terminal| -> io::Result<()> {
            outer_terminal::enable(terminal.backend_mut())?;
            outer_terminal::install_panic_hook();
            let result = App::new(state, sessions, preview, claude_env, tx).run(terminal, &rx);
            let restored = outer_terminal::disable(terminal.backend_mut());
            result.and(restored)
        })
        .change_context(TuiRunError)
    }
}

/// Something that wakes the loop.
enum LoopEvent {
    Input(Event),
    InputFailed(io::Error),
    Pane(PaneEvent),
    /// An actor changed the state.
    StateChanged,
}

/// A running `claude attach` and the thread it belongs to.
struct AttachedPane {
    thread: ThreadId,
    pane: Pane,
}

struct App {
    state: State,
    sessions: ActorRef<SessionsActor>,
    preview: ActorRef<PreviewActor>,
    keys: Keys,
    pane: Option<AttachedPane>,
    /// Shown under the preview header when `claude attach` couldn't start.
    pane_error: Option<String>,
    /// The environment attached sessions run with.
    claude_env: Vec<(OsString, OsString)>,
    tx: Sender<LoopEvent>,
    pane_area: Rect,
    /// The cursor style last sent to the outer terminal.
    cursor_style: SetCursorStyle,
    preview_cache: PreviewCache,
    sidebar_scroll: SidebarScroll,
    picker_scroll: PickerScroll,
}

impl App {
    fn new(
        state: State,
        sessions: ActorRef<SessionsActor>,
        preview: ActorRef<PreviewActor>,
        claude_env: Vec<(OsString, OsString)>,
        tx: Sender<LoopEvent>,
    ) -> Self {
        let focus = state.read().focus;
        Self {
            state,
            sessions,
            preview,
            keys: Keys::new(keymap::keymap(), focus),
            pane: None,
            pane_error: None,
            claude_env,
            tx,
            pane_area: Rect::default(),
            cursor_style: SetCursorStyle::DefaultUserShape,
            preview_cache: PreviewCache::default(),
            sidebar_scroll: SidebarScroll::default(),
            picker_scroll: PickerScroll::default(),
        }
    }

    fn run(mut self, terminal: &mut DefaultTerminal, rx: &Receiver<LoopEvent>) -> io::Result<()> {
        spawn_input_thread(self.tx.clone())?;
        loop {
            let [_, pane_area, _] = render::layout(terminal.size()?.into());
            self.pane_area = pane_area;
            if let Some(attached) = &mut self.pane {
                attached.pane.resize(PaneSize::from(pane_area));
            }
            let now = SystemTime::now();
            let mut drawn = (None, None);
            terminal.draw(|frame| {
                let state = self.state.read();
                let pane = self
                    .pane
                    .as_ref()
                    .filter(|attached| {
                        Some(attached.thread) == state.sessions.selected_id()
                            && !attached.pane.has_exited()
                    })
                    .map(|attached| &attached.pane);
                drawn = render::render(
                    frame,
                    &state,
                    pane,
                    self.pane_error.as_deref(),
                    &self.keys,
                    now,
                    &mut self.preview_cache,
                    &mut self.sidebar_scroll,
                    &mut self.picker_scroll,
                );
            })?;
            // Navigation scrolls by what was just drawn.
            let (preview_layout, picker_page) = drawn;
            if let Some(layout) = preview_layout
                && self.state.read().preview.layout != layout
            {
                self.state.write().preview.layout = layout;
            }
            if let Some(page) = picker_page
                && self
                    .state
                    .read()
                    .picker
                    .as_ref()
                    .is_some_and(|picker| picker.page() != page)
                && let Some(picker) = &mut self.state.write().picker
            {
                picker.resize(page);
            }
            self.mirror_cursor_style(terminal.backend_mut())?;
            if self.state.read().should_quit {
                return Ok(());
            }
            let first = match self.deadline() {
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
            self.reconcile(terminal.backend_mut())?;
            if let Some(attached) = &self.pane {
                attached.pane.flush_expired_sync(Instant::now());
            }
        }
    }

    /// When the loop must wake without an event: the pane's synchronized
    /// update times out, or a second passes while a thread is working so its
    /// elapsed time ticks.
    fn deadline(&self) -> Option<Instant> {
        let tick = (self.state.read().sessions.working_count() > 0)
            .then(|| Instant::now() + Duration::from_secs(1));
        let sync = self
            .pane
            .as_ref()
            .and_then(|attached| attached.pane.sync_deadline());
        tick.into_iter().chain(sync).min()
    }

    /// The pane that receives input: present, and the user is attached to it.
    fn attached_pane(&self) -> Option<&Pane> {
        self.pane
            .as_ref()
            .map(|attached| &attached.pane)
            .filter(|_| self.state.read().focus == Focus::Attached)
    }

    fn handle<W>(&mut self, event: LoopEvent, out: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        match event {
            LoopEvent::Input(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                let focus = self.state.read().focus;
                let intent = match focus {
                    Focus::Attached => match keymap::attached_route(key) {
                        Route::Intent(intent) => Some(intent),
                        Route::Forward => {
                            if let Some(pane) = self.attached_pane() {
                                pane.key(&key);
                            }
                            None
                        }
                    },
                    Focus::Sidebar | Focus::Preview => {
                        // Focus also changes outside intents (the pane exits).
                        if *self.keys.scope() != focus {
                            self.keys.set_scope(focus);
                        }
                        keymap::press(&mut self.keys, key)
                    }
                    Focus::Picker => keymap::picker_route(key),
                };
                if let Some(intent) = intent {
                    let commands = IntentHandler::handle(&intent, &mut self.state.write());
                    for command in &commands {
                        self.execute(command, out)?;
                    }
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
            LoopEvent::Input(_) | LoopEvent::Pane(PaneEvent::Output) | LoopEvent::StateChanged => {}
            LoopEvent::InputFailed(error) => return Err(error),
            LoopEvent::Pane(PaneEvent::Clipboard(text)) => {
                outer_terminal::copy_to_clipboard(out, &text)?;
            }
            LoopEvent::Pane(PaneEvent::Exited) => {
                // A pane dropped for another thread reports its exit too;
                // only the current pane's exit leaves the session.
                if self
                    .pane
                    .as_ref()
                    .is_some_and(|attached| attached.pane.has_exited())
                {
                    self.state.write().focus = Focus::Preview;
                    self.leave_pane(out)?;
                }
            }
        }
        Ok(())
    }

    #[expect(clippy::too_many_lines, reason = "one arm per command")]
    fn execute<W>(&mut self, command: &Command, out: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        match command {
            Command::Attach(target) => {
                let reusable = self.pane.as_ref().is_some_and(|attached| {
                    attached.thread == target.thread && !attached.pane.has_exited()
                });
                if !reusable {
                    let tx = self.tx.clone();
                    let command = PaneCommand {
                        argv: target.argv.clone(),
                        cwd: target.cwd.clone(),
                        env: self.claude_env.clone(),
                    };
                    let spawned =
                        Pane::spawn(&command, PaneSize::from(self.pane_area), move |event| {
                            let _ = tx.send(LoopEvent::Pane(event));
                        });
                    match spawned {
                        Ok(pane) => {
                            self.pane = Some(AttachedPane {
                                thread: target.thread,
                                pane,
                            });
                            self.pane_error = None;
                        }
                        Err(_) => {
                            self.pane_error = Some("couldn't start claude attach".to_owned());
                            self.state.write().focus = Focus::Preview;
                            return Ok(());
                        }
                    }
                }
                if let Some(attached) = &self.pane {
                    attached.pane.focus(true);
                }
                outer_terminal::set_mouse_capture(out, true)
            }
            Command::Detach => {
                if let Some(attached) = &self.pane {
                    attached.pane.focus(false);
                }
                self.leave_pane(out)
            }
            Command::CreateSession { project, root } => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::CreateSession {
                        project: *project,
                        root: root.clone(),
                    })
                    .try_send();
                Ok(())
            }
            Command::AddProject(root) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::AddProject(root.clone()))
                    .try_send();
                Ok(())
            }
            Command::ListDirectories(dir) => {
                let names = list_directories(dir);
                if let Some(picker) = &mut self.state.write().picker {
                    picker.show_directories(dir, names);
                }
                Ok(())
            }
            Command::RefreshSessions => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::RefreshSessions)
                    .try_send();
                Ok(())
            }
            Command::ShowPreview => {
                let _ = self.preview.tell(preview_actor::ShowPreview).try_send();
                Ok(())
            }
            Command::Yank(text) => outer_terminal::copy_to_clipboard(out, text),
            Command::Pin(id) => {
                let _ = self.sessions.tell(sessions_actor::Pin(*id)).try_send();
                Ok(())
            }
            Command::Unpin(id) => {
                let _ = self.sessions.tell(sessions_actor::Unpin(*id)).try_send();
                Ok(())
            }
            Command::Settle(id) => {
                let _ = self.sessions.tell(sessions_actor::Settle(*id)).try_send();
                Ok(())
            }
            Command::Unsettle(id) => {
                let _ = self.sessions.tell(sessions_actor::Unsettle(*id)).try_send();
                Ok(())
            }
            Command::Delete(id) => {
                let _ = self.sessions.tell(sessions_actor::Delete(*id)).try_send();
                Ok(())
            }
            Command::Visit(id) => {
                let _ = self.sessions.tell(sessions_actor::Visit(*id)).try_send();
                Ok(())
            }
        }
    }

    /// Drops the pane once another thread is selected: that kills its
    /// `claude attach`, and the session keeps running. If the selection moved
    /// while attached (a new session was selected), orb leaves the pane.
    fn reconcile<W>(&mut self, out: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        let (selected, focus) = {
            let state = self.state.read();
            (state.sessions.selected_id(), state.focus)
        };
        if self
            .pane
            .as_ref()
            .is_none_or(|attached| Some(attached.thread) == selected)
        {
            return Ok(());
        }
        self.pane = None;
        if focus == Focus::Attached {
            self.state.write().focus = Focus::Preview;
            self.leave_pane(out)?;
        }
        Ok(())
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

/// The names of `dir`'s subdirectories, symlinked ones included. Names that
/// aren't UTF-8 are skipped, and an unreadable `dir` has none.
fn list_directories(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| fs::metadata(entry.path()).is_ok_and(|metadata| metadata.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect()
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

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate file failures with `?` and assert on the outcome"
)]
mod tests {
    use std::fs;
    use std::io;
    use std::os::unix::fs::symlink;

    use super::list_directories;

    #[rstest::rstest]
    fn list_directories_keeps_directories_and_links_to_them() -> io::Result<()> {
        // Given a directory holding a file, a directory, and a link to it.
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join("notes.txt"), "")?;
        fs::create_dir_all(dir.path().join("dev"))?;
        symlink(dir.path().join("dev"), dir.path().join("linked"))?;

        // When listing its subdirectories.
        let names = {
            let mut names = list_directories(dir.path());
            names.sort();
            names
        };

        // Then the directory and the link are listed, not the file.
        assert_eq!(names, ["dev", "linked"], "the listed subdirectories");
        Ok(())
    }
}
