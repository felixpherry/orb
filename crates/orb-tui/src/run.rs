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
//! to Claude; otherwise keys go through the resize keys, then the [`keymap`].
//! The loop itself reads the directory picker's listings and the branch
//! picker's refs, and hands tools to zellij, since each takes milliseconds.
//!
//! `<C-h>` moves the keys from the pane to the sidebar and leaves the pane
//! drawn on the right, so `<C-l>` goes back into it; `<C-\>` shows the
//! preview instead.
//!
//! When a session start waits for the user to trust a directory, the pane
//! runs an interactive `claude` there instead. Leaving it, by its exit,
//! `<C-\>` or `<C-h>`, asks the sessions actor to try the start again. When
//! a started draft's thread comes up still selected, the loop attaches to it,
//! unless the user is in a picker or already attached.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use error_stack::{Report, ResultExt};
use kameo::prelude::ActorRef;
use orb_domain::feat::git::git_service::{GitService, git_reason};
use orb_domain::feat::preview::preview_actor::{self, PreviewActor};
use orb_domain::feat::sessions::sessions_actor::{self, SessionsActor};
use orb_domain::feat::sessions::state::ThreadId;
use orb_domain::feat::zellij::zellij_service::{NOT_IN_ZELLIJ, ZellijService, zellij_reason};
use orb_domain::{Command, Focus, Intent, IntentHandler, State, Wake};
use orb_term::{Pane, PaneCommand, PaneEvent, PaneSize};
use ratatui::DefaultTerminal;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::layout::Rect;
use wherror::Error;

use crate::keymap::{self, Keys, Route, Scope, Selection};
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
    /// `sessions` and preview commands to `preview`; the branch picker's refs
    /// come from `git`; attached sessions run with `claude_env`; tools open
    /// through `zellij`, `None` outside zellij. The terminal is restored on
    /// exit and on panic.
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
        git: GitService,
        claude_env: Vec<(OsString, OsString)>,
        zellij: Option<ZellijService>,
    ) -> Result<(), Report<TuiRunError>> {
        let Self { tx, rx } = self;
        ratatui::run(|terminal| -> io::Result<()> {
            outer_terminal::enable(terminal.backend_mut())?;
            outer_terminal::install_panic_hook();
            let result =
                App::new(state, sessions, preview, git, claude_env, zellij, tx).run(terminal, &rx);
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

/// What the pane is running for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaneOwner {
    /// `claude attach` to the thread's session.
    Thread(ThreadId),
    /// An interactive `claude` where the user trusts a directory.
    Trust,
}

impl PaneOwner {
    /// The pane still belongs on screen while `selected` is the selected
    /// thread. A trust pane stays whatever is selected.
    fn shown_with(self, selected: Option<ThreadId>) -> bool {
        match self {
            Self::Thread(thread) => Some(thread) == selected,
            Self::Trust => true,
        }
    }

    /// Leaving the pane tries the waiting session start again.
    fn retries_start(self) -> bool {
        matches!(self, Self::Trust)
    }
}

/// The directory to open a trust pane in: the one a start waits on, unless
/// its pane was already opened.
fn trust_to_open(trust: Option<&Path>, opened: Option<&Path>) -> Option<PathBuf> {
    trust.filter(|&dir| Some(dir) != opened).map(Path::to_owned)
}

/// Whether to attach to `started`, a started draft's thread: only while it's
/// the selected thread and the user is neither in a picker nor attached.
fn attaches(started: Option<ThreadId>, selected: Option<ThreadId>, focus: Focus) -> bool {
    started.is_some() && started == selected && !matches!(focus, Focus::Picker | Focus::Attached)
}

/// Where the keys go once the pane is gone: from the pane to the preview;
/// anywhere else (the sidebar after `<C-h>`, the preview, a picker) they stay.
fn after_pane(focus: Focus) -> Focus {
    match focus {
        Focus::Attached => Focus::Preview,
        Focus::Sidebar | Focus::Preview | Focus::Picker => focus,
    }
}

/// A running pane and what it is for.
struct AttachedPane {
    owner: PaneOwner,
    pane: Pane,
}

struct App {
    state: State,
    sessions: ActorRef<SessionsActor>,
    preview: ActorRef<PreviewActor>,
    git: GitService,
    keys: Keys,
    pane: Option<AttachedPane>,
    /// Shown under the preview header when `claude attach` couldn't start.
    pane_error: Option<String>,
    /// The directory a trust pane was opened for, while its start waits.
    opened_trust: Option<PathBuf>,
    /// The environment attached sessions run with.
    claude_env: Vec<(OsString, OsString)>,
    /// Opens tools; `None` outside zellij.
    zellij: Option<ZellijService>,
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
        git: GitService,
        claude_env: Vec<(OsString, OsString)>,
        zellij: Option<ZellijService>,
        tx: Sender<LoopEvent>,
    ) -> Self {
        let scope = {
            let state = state.read();
            Scope::new(state.focus, Selection::of(&state.sessions))
        };
        Self {
            state,
            sessions,
            preview,
            git,
            keys: Keys::new(keymap::keymap(), scope),
            pane: None,
            pane_error: None,
            opened_trust: None,
            claude_env,
            zellij,
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
            let [_, pane_area, _] =
                render::layout(terminal.size()?.into(), &self.state.read().sidebar);
            self.pane_area = pane_area;
            if let Some(attached) = &mut self.pane {
                attached.pane.resize(PaneSize::from(pane_area));
            }
            let now = SystemTime::now();
            let mut drawn = (None, None, None);
            terminal.draw(|frame| {
                let state = self.state.read();
                let pane = self
                    .pane
                    .as_ref()
                    .filter(|attached| {
                        attached.owner.shown_with(state.sessions.selected_id())
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
            let (preview_layout, sidebar_layout, picker_page) = drawn;
            if let Some(layout) = preview_layout
                && self.state.read().preview.layout != layout
            {
                self.state.write().preview.layout = layout;
            }
            if let Some(layout) = sidebar_layout
                && self.state.read().sidebar.layout != layout
            {
                self.state.write().sidebar.layout = layout;
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
            self.open_trust(terminal.backend_mut())?;
            self.open_started(terminal.backend_mut())?;
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
                    Focus::Sidebar | Focus::Preview => match keymap::layout_route(key) {
                        Some(intent) => {
                            // A resize ends any key sequence in progress.
                            self.keys.dismiss();
                            Some(intent)
                        }
                        None => {
                            // Focus and the selection also change outside
                            // intents (the pane exits, a draft starts).
                            let scope =
                                Scope::new(focus, Selection::of(&self.state.read().sessions));
                            if *self.keys.scope() != scope {
                                self.keys.set_scope(scope);
                            }
                            keymap::press(&mut self.keys, key)
                        }
                    },
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
                let exited = self
                    .pane
                    .as_ref()
                    .filter(|attached| attached.pane.has_exited())
                    .map(|attached| attached.owner.retries_start());
                match exited {
                    Some(true) => self.leave_trust(out)?,
                    Some(false) => {
                        {
                            let mut app = self.state.write();
                            app.focus = after_pane(app.focus);
                            app.pane_shown = None;
                        }
                        self.leave_pane(out)?;
                    }
                    None => {}
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
                let owner = PaneOwner::Thread(target.thread);
                let reusable = self
                    .pane
                    .as_ref()
                    .is_some_and(|attached| attached.owner == owner && !attached.pane.has_exited());
                if !reusable {
                    match self.spawn_pane(target.argv.clone(), target.cwd.clone()) {
                        Some(pane) => {
                            self.pane = Some(AttachedPane { owner, pane });
                            self.pane_error = None;
                        }
                        None => {
                            self.pane_error = Some("couldn't start claude attach".to_owned());
                            let mut app = self.state.write();
                            app.focus = Focus::Preview;
                            app.pane_shown = None;
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
                    if attached.owner.retries_start() {
                        return self.leave_trust(out);
                    }
                    attached.pane.focus(false);
                }
                self.leave_pane(out)
            }
            Command::CreateDraft(project) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::CreateDraft(*project))
                    .try_send();
                Ok(())
            }
            Command::SaveDraft(project) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::SaveDraft(*project))
                    .try_send();
                Ok(())
            }
            Command::CheckoutDraft {
                project,
                git_ref,
                cwd,
            } => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::CheckoutDraft {
                        project: *project,
                        git_ref: git_ref.clone(),
                        cwd: cwd.clone(),
                    })
                    .try_send();
                Ok(())
            }
            Command::InitGit(project) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::InitGit(*project))
                    .try_send();
                Ok(())
            }
            Command::StartDraft(project) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::StartDraft(*project))
                    .try_send();
                Ok(())
            }
            Command::DiscardDraft(project) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::DiscardDraft(*project))
                    .try_send();
                Ok(())
            }
            Command::MoveThread { thread, to } => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::MoveThread {
                        thread: *thread,
                        to: to.clone(),
                    })
                    .try_send();
                Ok(())
            }
            Command::SwitchBranch {
                thread,
                git_ref,
                to_root,
            } => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::SwitchBranch {
                        thread: *thread,
                        git_ref: git_ref.clone(),
                        to_root: *to_root,
                    })
                    .try_send();
                Ok(())
            }
            Command::ListBranches(cwd) => {
                let refs = self.git.refs(cwd);
                let mut app = self.state.write();
                match refs {
                    Ok(refs) => {
                        if let Some(picker) = &mut app.picker {
                            picker.show_branches(cwd, refs);
                        }
                    }
                    Err(report) => {
                        if let Some(picker) = app.picker.take() {
                            app.focus = picker.return_to();
                        }
                        app.sessions.error = Some(git_reason(&report));
                    }
                }
                Ok(())
            }
            Command::OpenTool { tool, cwd } => {
                let opened = match &self.zellij {
                    None => Err(NOT_IN_ZELLIJ.to_owned()),
                    Some(zellij) => zellij
                        .open_tool(*tool, cwd)
                        .map_err(|report| zellij_reason(&report)),
                };
                if let Err(reason) = opened {
                    self.state.write().sessions.error = Some(reason);
                }
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
            Command::SaveUi => {
                let _ = self.sessions.tell(sessions_actor::SaveUi).try_send();
                Ok(())
            }
            Command::RemoveProject(id) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::RemoveProject(*id))
                    .try_send();
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
            .is_none_or(|attached| attached.owner.shown_with(selected))
        {
            return Ok(());
        }
        self.pane = None;
        self.state.write().pane_shown = None;
        if focus == Focus::Attached {
            self.state.write().focus = Focus::Preview;
            self.leave_pane(out)?;
        }
        Ok(())
    }

    /// Opens an interactive `claude` in the pane when a session start begins
    /// waiting for the user to trust its directory, and attaches to it.
    fn open_trust<W>(&mut self, out: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        let trust = self.state.read().sessions.trust.clone();
        let open = trust_to_open(trust.as_deref(), self.opened_trust.as_deref());
        self.opened_trust = trust;
        let Some(dir) = open else {
            return Ok(());
        };
        match self.spawn_pane(vec![OsString::from("claude")], dir) {
            Some(pane) => {
                pane.focus(true);
                self.pane = Some(AttachedPane {
                    owner: PaneOwner::Trust,
                    pane,
                });
                self.pane_error = None;
                self.state.write().focus = Focus::Attached;
                outer_terminal::set_mouse_capture(out, true)
            }
            None => {
                self.pane_error = Some("couldn't start claude".to_owned());
                self.retry_start();
                Ok(())
            }
        }
    }

    /// Attaches to a started draft's thread if it's still selected. The
    /// request is taken either way, so it never fires later. A failure the
    /// start still reported (saving the store) stays on the mode line.
    fn open_started<W>(&mut self, out: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        let commands = {
            let mut state = self.state.write();
            let started = state.sessions.attach.take();
            if !attaches(started, state.sessions.selected_id(), state.focus) {
                return Ok(());
            }
            let error = state.sessions.error.take();
            let commands = IntentHandler::handle(&Intent::Attach, &mut state);
            state.sessions.error = error;
            commands
        };
        for command in &commands {
            self.execute(command, out)?;
        }
        Ok(())
    }

    /// Closes the trust pane, returns to the preview unless `<C-h>` already
    /// moved the keys to the sidebar, and tries the waiting session start
    /// again.
    fn leave_trust<W>(&mut self, out: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        self.pane = None;
        {
            let mut app = self.state.write();
            app.focus = after_pane(app.focus);
            app.pane_shown = None;
        }
        self.retry_start();
        self.leave_pane(out)
    }

    fn retry_start(&self) {
        let _ = self.sessions.tell(sessions_actor::RetryStart).try_send();
    }

    /// Runs `argv` in `cwd` in a pane the size of the pane area, with
    /// Claude's environment; `None` if it can't start.
    fn spawn_pane(&self, argv: Vec<OsString>, cwd: PathBuf) -> Option<Pane> {
        let tx = self.tx.clone();
        let command = PaneCommand {
            argv,
            cwd,
            env: self.claude_env.clone(),
        };
        Pane::spawn(&command, PaneSize::from(self.pane_area), move |event| {
            let _ = tx.send(LoopEvent::Pane(event));
        })
        .ok()
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
    use std::path::{Path, PathBuf};

    use orb_domain::Focus;
    use orb_domain::feat::sessions::state::ThreadId;

    use super::{PaneOwner, after_pane, attaches, list_directories, trust_to_open};

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

    #[rstest::rstest]
    fn thread_pane_is_dropped_when_another_thread_is_selected() {
        // Given a pane attached to thread 1.
        let owner = PaneOwner::Thread(ThreadId(1));

        // When thread 2 is selected.
        let shown = owner.shown_with(Some(ThreadId(2)));

        // Then the pane no longer belongs on screen.
        assert!(!shown, "another thread's pane should be dropped");
    }

    #[rstest::rstest]
    fn trust_pane_survives_a_selection_change() {
        // Given a trust pane.
        let owner = PaneOwner::Trust;

        // When thread 2 is selected.
        let shown = owner.shown_with(Some(ThreadId(2)));

        // Then the pane stays.
        assert!(shown, "a selection change shouldn't close the trust pane");
    }

    #[rstest::rstest]
    #[case(PaneOwner::Trust, true)]
    #[case(PaneOwner::Thread(ThreadId(1)), false)]
    fn leaving_a_pane_retries_the_start_only_for_trust(
        #[case] owner: PaneOwner,
        #[case] expected: bool,
    ) {
        assert_eq!(
            owner.retries_start(),
            expected,
            "only a trust pane retries the waiting start"
        );
    }

    #[rstest::rstest]
    #[case(Focus::Attached, Focus::Preview)]
    #[case(Focus::Sidebar, Focus::Sidebar)]
    #[case(Focus::Preview, Focus::Preview)]
    #[case(Focus::Picker, Focus::Picker)]
    fn keys_leave_a_gone_pane_for_the_preview_only_from_the_pane(
        #[case] focus: Focus,
        #[case] expected: Focus,
    ) {
        assert_eq!(
            after_pane(focus),
            expected,
            "focus after the pane goes from {focus:?}"
        );
    }

    #[rstest::rstest]
    fn trust_pane_opens_for_a_new_trust_request() {
        // Given a start waiting on /tmp/x and no trust pane opened.
        let trust = Path::new("/tmp/x");

        // When deciding what to open.
        let open = trust_to_open(Some(trust), None);

        // Then a trust pane opens in /tmp/x.
        assert_eq!(
            open,
            Some(PathBuf::from("/tmp/x")),
            "a new trust request should open the pane"
        );
    }

    #[rstest::rstest]
    fn trust_pane_is_not_reopened_while_the_same_trust_waits() {
        // Given a start waiting on /tmp/x whose trust pane was opened.
        let trust = Path::new("/tmp/x");

        // When deciding what to open.
        let open = trust_to_open(Some(trust), Some(trust));

        // Then nothing opens.
        assert_eq!(open, None, "the same request shouldn't reopen the pane");
    }

    #[rstest::rstest]
    #[case(Focus::Sidebar)]
    #[case(Focus::Preview)]
    fn started_thread_still_selected_is_attached(#[case] focus: Focus) {
        // Given thread 1 started from a draft and still selected.
        let started = Some(ThreadId(1));

        // When deciding whether to attach in `focus`.
        let attach = attaches(started, started, focus);

        // Then orb attaches.
        assert!(attach, "a still-selected started thread should attach");
    }

    #[rstest::rstest]
    fn started_thread_is_not_attached_after_the_selection_moved() {
        // Given thread 1 started from a draft while thread 2 is selected.
        let started = Some(ThreadId(1));

        // When deciding whether to attach.
        let attach = attaches(started, Some(ThreadId(2)), Focus::Sidebar);

        // Then orb stays where the user is.
        assert!(!attach, "a moved selection shouldn't attach");
    }

    #[rstest::rstest]
    #[case(Focus::Picker)]
    #[case(Focus::Attached)]
    fn started_thread_is_not_attached_from_a_picker_or_a_pane(#[case] focus: Focus) {
        // Given thread 1 started from a draft and still selected.
        let started = Some(ThreadId(1));

        // When deciding whether to attach in `focus`.
        let attach = attaches(started, started, focus);

        // Then orb leaves the user where they are.
        assert!(!attach, "no attach while in {focus:?}");
    }

    #[rstest::rstest]
    fn nothing_started_attaches_nothing() {
        // Given no started thread and nothing selected.

        // When deciding whether to attach.
        let attach = attaches(None, None, Focus::Sidebar);

        // Then orb doesn't attach.
        assert!(!attach, "no started thread, no attach");
    }
}
