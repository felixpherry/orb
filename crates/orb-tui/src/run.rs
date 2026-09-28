//! The frontend loop: wait for input or pane activity, apply it, draw once.
//!
//! Terminal input, the pane's background threads, and the actors' state
//! changes all feed one channel. The loop sleeps until something arrives,
//! handles everything pending, and then draws a single frame, so bursts of
//! output cost one redraw. The only tick is every spinner frame (100 ms)
//! while a thread is working or a session is starting, so the spinners turn
//! and a working thread's elapsed time counts up.
//!
//! Each attached thread keeps its own `claude attach` pane while other
//! threads are selected; the right side shows the selected thread's pane
//! when it's attached, else the dashboard. While attached, input goes
//! straight to Claude; otherwise keys go through the resize keys, then the
//! [`keymap`]. The loop itself reads the directory picker's listings and the
//! branch picker's refs, and hands tools to zellij, since each takes
//! milliseconds.
//!
//! `<C-h>` moves the keys from the pane to the sidebar and leaves it drawn,
//! so `<C-l>` goes back into it; `<C-\>`, in the pane or on the thread in the
//! sidebar, detaches it, and settling or deleting the thread or its Claude
//! exiting ends the pane too.
//!
//! When a thread finishes a turn or starts needing an approval or an answer
//! while orb's pane isn't focused, the loop announces it as a desktop
//! notification; notices that arrive while it is focused are dropped. zellij
//! sends no focus-out to the tab the user leaves, so while orb seems focused
//! a thread of its own asks zellij whether any client is on orb's pane, and
//! announces the notices only if none is. That thread also asks zellij which
//! tab orb's pane is on, where a click on the notification goes back to.
//!
//! When a session start waits for the user to trust a directory, a pane of
//! its own, over any thread's, runs an interactive `claude` there. Leaving
//! it, by its exit, `<C-\>` or `<C-h>`, asks the sessions actor to try the
//! start again. When a started draft's thread comes up still selected, the
//! loop attaches to it, unless the user is typing in a picker, the rename
//! box or the sidebar search, or is already attached.
//!
//! After each frame the outer terminal's cursor takes the shape of where the
//! keys are: a block in the sidebar, a bar in a text input, Claude's own
//! shape while attached.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Instant, SystemTime};

use error_stack::{Report, ResultExt};
use jiff::tz::TimeZone;
use kameo::prelude::ActorRef;
use orb_domain::feat::git::git_service::{GitService, git_reason};
use orb_domain::feat::notify::notifier::NotifierService;
use orb_domain::feat::sessions::sessions_actor::{self, SessionsActor};
use orb_domain::feat::sessions::state::ThreadId;
use orb_domain::feat::zellij::zellij_service::{
    NOT_IN_ZELLIJ, ZellijError, ZellijService, zellij_reason,
};
use orb_domain::{Command, Focus, Intent, IntentHandler, State, Wake};
use orb_term::{Pane, PaneCommand, PaneEvent, PaneSize};
use ratatui::DefaultTerminal;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::layout::Rect;
use wherror::Error;

use crate::keymap::{self, Keys, Route, Scope, Selection};
use crate::picker::PickerScroll;
use crate::sidebar::{SPINNER_FRAME, SidebarScroll};
use crate::{outer_terminal, render};

/// The frontend loop failed to draw a frame or read a terminal event.
#[derive(Debug, Error)]
#[error(debug)]
pub struct TuiRunError;

/// orb's TUI. Created before the actors so they can wake its loop.
pub struct Frontend {
    tx: Sender<LoopEvent>,
    rx: Receiver<LoopEvent>,
    /// The zone the mode line's clock shows.
    tz: TimeZone,
}

impl Frontend {
    /// A frontend whose mode line shows the time in `tz`.
    pub fn new(tz: TimeZone) -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx, tz }
    }

    /// Wakes the loop to redraw after an actor changed the state.
    pub fn waker(&self) -> Wake {
        let tx = self.tx.clone();
        Arc::new(move || {
            let _ = tx.send(LoopEvent::StateChanged);
        })
    }

    /// Runs orb's TUI until the user quits. Session commands go to
    /// `sessions`; the branch picker's refs
    /// come from `git`; attached sessions run with `claude_env`; tools open
    /// through `zellij`, `None` outside zellij; notices are announced through
    /// `notifier` while orb's pane isn't focused, or while zellij says no
    /// client is on it. The terminal is restored on exit and on panic.
    ///
    /// # Errors
    ///
    /// Returns [`TuiRunError`] if drawing a frame or reading a terminal event
    /// fails.
    pub fn run(
        self,
        state: State,
        sessions: ActorRef<SessionsActor>,
        git: GitService,
        claude_env: Vec<(OsString, OsString)>,
        zellij: Option<ZellijService>,
        notifier: NotifierService,
    ) -> Result<(), Report<TuiRunError>> {
        let Self { tx, rx, tz } = self;
        ratatui::run(|terminal| -> io::Result<()> {
            outer_terminal::enable(terminal.backend_mut())?;
            outer_terminal::install_panic_hook();
            let result = App::new(state, sessions, git, claude_env, zellij, notifier, tx, tz)
                .run(terminal, &rx);
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

/// The directory to open a trust pane in: the one a start waits on, unless
/// its pane was already opened.
fn trust_to_open(trust: Option<&Path>, opened: Option<&Path>) -> Option<PathBuf> {
    trust.filter(|&dir| Some(dir) != opened).map(Path::to_owned)
}

/// Whether to attach to `started`, a started draft's thread: only while it's
/// the selected thread and the user is neither typing (in a picker, the
/// rename box or the search) nor attached.
fn attaches(started: Option<ThreadId>, selected: Option<ThreadId>, focus: Focus) -> bool {
    started.is_some()
        && started == selected
        && !matches!(
            focus,
            Focus::Picker | Focus::Rename | Focus::Search | Focus::Attached
        )
}

/// The pane the right-hand area draws: none while the dashboard has the
/// keys, else the trust pane while it's open, else the selected thread's.
fn shown_pane<'a, P>(
    trust: Option<&'a P>,
    panes: &'a HashMap<ThreadId, P>,
    selected: Option<ThreadId>,
    focus: Focus,
) -> Option<&'a P> {
    match focus {
        Focus::Dashboard => None,
        Focus::Attached | Focus::Sidebar | Focus::Picker | Focus::Rename | Focus::Search => {
            trust.or_else(|| panes.get(&selected?))
        }
    }
}

/// The panes to drop, given each as (thread, exited): the exited ones, and
/// those whose thread left `attached`.
fn to_drop<I>(panes: I, attached: &HashSet<ThreadId>) -> Vec<ThreadId>
where
    I: IntoIterator<Item = (ThreadId, bool)>,
{
    panes
        .into_iter()
        .filter(|&(id, exited)| exited || !attached.contains(&id))
        .map(|(id, _)| id)
        .collect()
}

/// Where the keys go once the pane is gone: from the pane to the dashboard;
/// anywhere else (the sidebar after `<C-h>`, the dashboard, a text input) they
/// stay.
fn after_pane(focus: Focus) -> Focus {
    match focus {
        Focus::Attached => Focus::Dashboard,
        Focus::Sidebar | Focus::Dashboard | Focus::Picker | Focus::Rename | Focus::Search => focus,
    }
}

/// The outer terminal's cursor shape with the keys in `focus`: a steady
/// block on the sidebar's selected row or the dashboard's highlighted item, a
/// steady bar in a text input (a picker's filter, the rename box, the sidebar
/// search), the child's own shape (`pane`) while attached, and the user's
/// default while attached without a pane.
fn cursor_style(focus: Focus, pane: Option<SetCursorStyle>) -> SetCursorStyle {
    match (focus, pane) {
        (Focus::Sidebar | Focus::Dashboard, _) => SetCursorStyle::SteadyBlock,
        (Focus::Picker | Focus::Rename | Focus::Search, _) => SetCursorStyle::SteadyBar,
        (Focus::Attached, Some(style)) => style,
        (Focus::Attached, None) => SetCursorStyle::DefaultUserShape,
    }
}

/// Whether to announce notices: always while orb's pane isn't focused.
/// While it seems focused, where the sidebar already shows each status, only
/// if `watched`, asked only then, says no zellij client has orb's pane
/// focused, since a zellij tab switch sends orb no focus-out. If zellij can't
/// say, they're dropped.
fn announces<F>(focused: bool, watched: F) -> bool
where
    F: FnOnce() -> Result<bool, Report<ZellijError>>,
{
    !focused || matches!(watched(), Ok(false))
}

struct App {
    state: State,
    sessions: ActorRef<SessionsActor>,
    git: GitService,
    keys: Keys,
    /// Each attached thread's `claude attach`, kept while other threads are
    /// selected.
    panes: HashMap<ThreadId, Pane>,
    /// The interactive `claude` where the user trusts a directory, drawn over
    /// any thread's pane while it's open.
    trust: Option<Pane>,
    /// Shown on the right when `claude attach` couldn't start.
    pane_error: Option<String>,
    /// The directory a trust pane was opened for, while its start waits.
    opened_trust: Option<PathBuf>,
    /// The environment attached sessions run with.
    claude_env: Vec<(OsString, OsString)>,
    /// Opens tools; `None` outside zellij.
    zellij: Option<ZellijService>,
    /// Announces the sessions actor's notices.
    notifier: NotifierService,
    /// The zone the mode line's clock shows.
    tz: TimeZone,
    tx: Sender<LoopEvent>,
    pane_area: Rect,
    /// The cursor style last sent to the outer terminal.
    cursor_style: SetCursorStyle,
    sidebar_scroll: SidebarScroll,
    picker_scroll: PickerScroll,
    /// orb's pane has the outer terminal's focus, as its last focus event
    /// said; `true` until one arrives.
    focused: bool,
}

impl App {
    fn new(
        state: State,
        sessions: ActorRef<SessionsActor>,
        git: GitService,
        claude_env: Vec<(OsString, OsString)>,
        zellij: Option<ZellijService>,
        notifier: NotifierService,
        tx: Sender<LoopEvent>,
        tz: TimeZone,
    ) -> Self {
        let scope = {
            let state = state.read();
            Scope::new(state.focus, Selection::of(&state.sessions))
        };
        Self {
            state,
            sessions,
            git,
            keys: Keys::new(keymap::keymap(), scope),
            panes: HashMap::new(),
            trust: None,
            pane_error: None,
            opened_trust: None,
            claude_env,
            zellij,
            notifier,
            tz,
            tx,
            pane_area: Rect::default(),
            cursor_style: SetCursorStyle::DefaultUserShape,
            sidebar_scroll: SidebarScroll::default(),
            picker_scroll: PickerScroll::default(),
            focused: true,
        }
    }

    fn run(mut self, terminal: &mut DefaultTerminal, rx: &Receiver<LoopEvent>) -> io::Result<()> {
        spawn_input_thread(self.tx.clone())?;
        loop {
            let [_, pane_area, _] =
                render::layout(terminal.size()?.into(), &self.state.read().sidebar);
            self.pane_area = pane_area;
            let size = PaneSize::from(pane_area);
            for pane in self.trust.iter_mut().chain(self.panes.values_mut()) {
                pane.resize(size);
            }
            let now = SystemTime::now();
            let mut drawn = (None, None);
            terminal.draw(|frame| {
                let state = self.state.read();
                let pane = shown_pane(
                    self.trust.as_ref(),
                    &self.panes,
                    state.sessions.selected_id(),
                    state.focus,
                )
                .filter(|pane| !pane.has_exited());
                drawn = render::render(
                    frame,
                    &state,
                    pane,
                    self.pane_error.as_deref(),
                    &self.keys,
                    now,
                    &self.tz,
                    &mut self.sidebar_scroll,
                    &mut self.picker_scroll,
                );
            })?;
            // Navigation scrolls by what was just drawn.
            let (sidebar_layout, picker_page) = drawn;
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
            self.sync_cursor_style(terminal.backend_mut())?;
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
            self.announce();
            self.reconcile(terminal.backend_mut())?;
            self.open_trust(terminal.backend_mut())?;
            self.open_started(terminal.backend_mut())?;
            let now = Instant::now();
            for pane in self.trust.iter().chain(self.panes.values()) {
                pane.flush_expired_sync(now);
            }
        }
    }

    /// When the loop must wake without an event: a pane's synchronized
    /// update times out, or a spinner frame passes while a thread is working
    /// or a session is starting, so the spinners and elapsed time tick.
    fn deadline(&self) -> Option<Instant> {
        let tick = self
            .state
            .read()
            .sessions
            .spinning()
            .then(|| Instant::now() + SPINNER_FRAME);
        let sync = self
            .trust
            .iter()
            .chain(self.panes.values())
            .filter_map(Pane::sync_deadline);
        tick.into_iter().chain(sync).min()
    }

    /// The pane that receives input: the shown one, while the user is
    /// attached to it.
    fn attached_pane(&self) -> Option<&Pane> {
        let state = self.state.read();
        match state.focus {
            Focus::Attached => shown_pane(
                self.trust.as_ref(),
                &self.panes,
                state.sessions.selected_id(),
                state.focus,
            ),
            Focus::Sidebar | Focus::Dashboard | Focus::Picker | Focus::Rename | Focus::Search => {
                None
            }
        }
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
                    Focus::Sidebar | Focus::Dashboard => {
                        match keymap::layout_route(key).or(match focus {
                            Focus::Sidebar => keymap::sidebar_route(key),
                            _ => None,
                        }) {
                            Some(intent) => {
                                // A resize or a detach ends any key sequence in progress.
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
                        }
                    }
                    // The rename box and the search take the picker's keys.
                    Focus::Picker | Focus::Rename | Focus::Search => keymap::picker_route(key),
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
                self.focused = true;
                if let Some(pane) = self.attached_pane() {
                    pane.focus(true);
                }
            }
            LoopEvent::Input(Event::FocusLost) => {
                self.focused = false;
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
                // `reconcile` sweeps exited thread panes.
                if self.trust.as_ref().is_some_and(Pane::has_exited) {
                    self.leave_trust(out)?;
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
                if self.panes.get(&target.thread).is_none_or(Pane::has_exited) {
                    match self.spawn_pane(target.argv.clone(), target.cwd.clone()) {
                        Some(pane) => {
                            self.panes.insert(target.thread, pane);
                            self.pane_error = None;
                        }
                        None => {
                            self.pane_error = Some("couldn't start claude attach".to_owned());
                            let mut app = self.state.write();
                            app.focus = Focus::Dashboard;
                            app.attached.remove(&target.thread);
                            return Ok(());
                        }
                    }
                }
                if let Some(pane) = self.panes.get(&target.thread) {
                    pane.focus(true);
                }
                outer_terminal::set_mouse_capture(out, true)
            }
            Command::Detach => {
                if self.trust.is_some() {
                    return self.leave_trust(out);
                }
                // `Intent::Detach` already gave the dashboard the keys, so
                // the pane is looked up directly rather than as shown.
                let selected = self.state.read().sessions.selected_id();
                if let Some(pane) = selected.and_then(|id| self.panes.get(&id)) {
                    pane.focus(false);
                }
                outer_terminal::set_mouse_capture(out, false)
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
            Command::Pin(id) => {
                let _ = self.sessions.tell(sessions_actor::Pin(*id)).try_send();
                Ok(())
            }
            Command::Unpin(id) => {
                let _ = self.sessions.tell(sessions_actor::Unpin(*id)).try_send();
                Ok(())
            }
            Command::RenameThread { thread, title } => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::RenameThread {
                        thread: *thread,
                        title: title.clone(),
                    })
                    .try_send();
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
            Command::CreateGroup {
                kind,
                project,
                name,
            } => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::CreateGroup {
                        kind: *kind,
                        project: *project,
                        name: name.clone(),
                    })
                    .try_send();
                Ok(())
            }
            Command::StartGroupDraft(group) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::StartGroupDraft(*group))
                    .try_send();
                Ok(())
            }
            Command::SaveGroupDraft(group) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::SaveGroupDraft(*group))
                    .try_send();
                Ok(())
            }
            Command::StartSibling {
                group,
                model,
                permission_mode,
                from,
            } => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::StartSibling {
                        group: *group,
                        model: model.clone(),
                        permission_mode: permission_mode.clone(),
                        from: *from,
                    })
                    .try_send();
                Ok(())
            }
            Command::PinGroup(group) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::PinGroup(*group))
                    .try_send();
                Ok(())
            }
            Command::UnpinGroup(group) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::UnpinGroup(*group))
                    .try_send();
                Ok(())
            }
            Command::SettleGroup(group) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::SettleGroup(*group))
                    .try_send();
                Ok(())
            }
            Command::UnsettleGroup(group) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::UnsettleGroup(*group))
                    .try_send();
                Ok(())
            }
            Command::DeleteGroup(group) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::DeleteGroup(*group))
                    .try_send();
                Ok(())
            }
        }
    }

    /// Drops the panes whose Claude exited or whose thread is no longer
    /// attached, which kills their `claude attach`, and takes them out of
    /// `attached`. If the keys were in a pane that's no longer shown, orb
    /// leaves it.
    fn reconcile<W>(&mut self, out: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        let gone = to_drop(
            self.panes.iter().map(|(id, pane)| (*id, pane.has_exited())),
            &self.state.read().attached,
        );
        for id in &gone {
            self.panes.remove(id);
        }
        let left = {
            let mut app = self.state.write();
            for id in &gone {
                app.attached.remove(id);
            }
            let left = app.focus == Focus::Attached
                && shown_pane(
                    self.trust.as_ref(),
                    &self.panes,
                    app.sessions.selected_id(),
                    app.focus,
                )
                .is_none();
            if left {
                app.focus = after_pane(app.focus);
            }
            left
        };
        if left {
            outer_terminal::set_mouse_capture(out, false)?;
        }
        Ok(())
    }

    /// Opens an interactive `claude` in its own pane, over any thread's, when
    /// a session start begins waiting for the user to trust its directory, and
    /// attaches to it, closing the rename box if it was open.
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
                self.trust = Some(pane);
                self.pane_error = None;
                {
                    let mut app = self.state.write();
                    app.rename = None;
                    app.sessions.search = None;
                    app.focus = Focus::Attached;
                }
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

    /// Closes the trust pane, leaving the thread panes running, returns to
    /// the dashboard unless `<C-h>` already moved the keys to the sidebar, and
    /// tries the waiting session start again.
    fn leave_trust<W>(&mut self, out: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        self.trust = None;
        {
            let mut app = self.state.write();
            app.focus = after_pane(app.focus);
        }
        self.retry_start();
        outer_terminal::set_mouse_capture(out, false)
    }

    /// Takes the sessions actor's notices and announces them as
    /// [`announces`] decides, telling the notifier which zellij tab orb's
    /// pane is on so a click can go back there. Each zellij call can take up
    /// to its timeout, so this happens on a thread of its own. A notice that
    /// can't be sent is dropped, and one whose tab zellij can't give is sent
    /// without it.
    fn announce(&self) {
        let notices = mem::take(&mut self.state.write().sessions.notices);
        if notices.is_empty() {
            return;
        }
        let focused = self.focused;
        let zellij = self.zellij.clone();
        let notifier = self.notifier.clone();
        let _ = thread::Builder::new()
            .name("orb-notice".into())
            .spawn(move || {
                let watched = || match &zellij {
                    Some(zellij) => zellij.pane_focused(),
                    None => Err(Report::new(ZellijError).attach(NOT_IN_ZELLIJ.to_owned())),
                };
                if announces(focused, watched) {
                    let tab = zellij.as_ref().and_then(|zellij| zellij.pane_tab().ok());
                    for notice in &notices {
                        let _ = notifier.announce(notice, tab);
                    }
                }
            });
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

    /// Shows the cursor shape for where the keys are in the outer terminal.
    fn sync_cursor_style<W>(&mut self, out: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        let style = {
            let focus = self.state.read().focus;
            cursor_style(focus, self.attached_pane().map(Pane::cursor_style))
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
    use std::cell::Cell;
    use std::collections::{HashMap, HashSet};
    use std::fs;
    use std::io;
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};

    use error_stack::Report;
    use orb_domain::Focus;
    use orb_domain::feat::sessions::state::ThreadId;
    use orb_domain::feat::zellij::zellij_service::ZellijError;
    use ratatui::crossterm::cursor::SetCursorStyle;

    use super::{
        after_pane, announces, attaches, cursor_style, list_directories, shown_pane, to_drop,
        trust_to_open,
    };

    #[rstest::rstest]
    #[case::sidebar_block(Focus::Sidebar, None, SetCursorStyle::SteadyBlock)]
    #[case::picker_bar(Focus::Picker, None, SetCursorStyle::SteadyBar)]
    #[case::rename_bar(Focus::Rename, None, SetCursorStyle::SteadyBar)]
    #[case::search_bar(Focus::Search, None, SetCursorStyle::SteadyBar)]
    #[case::attached_follows_the_pane(
        Focus::Attached,
        Some(SetCursorStyle::BlinkingUnderScore),
        SetCursorStyle::BlinkingUnderScore
    )]
    #[case::dashboard_block(Focus::Dashboard, None, SetCursorStyle::SteadyBlock)]
    fn cursor_shape_follows_where_the_keys_are(
        #[case] focus: Focus,
        #[case] pane: Option<SetCursorStyle>,
        #[case] expected: SetCursorStyle,
    ) {
        // Given / When choosing the cursor shape in `focus` with the pane's `pane` shape.
        let style = cursor_style(focus, pane);

        // Then it is the shape for that focus.
        assert_eq!(style, expected, "cursor shape in {focus:?}");
    }

    #[rstest::rstest]
    #[case::no_client_is_on_orbs_pane(Ok(false), true)]
    #[case::a_client_is_on_orbs_pane(Ok(true), false)]
    #[case::zellij_cant_say(Err(Report::new(ZellijError)), false)]
    fn a_focused_orb_announces_only_when_zellij_says_no_client_is_on_its_pane(
        #[case] watched: Result<bool, Report<ZellijError>>,
        #[case] expected: bool,
    ) {
        // Given / When deciding while orb's pane seems focused and zellij answers `watched`.
        let announced = announces(true, || watched);

        // Then the notices are announced only if no client is on orb's pane.
        assert_eq!(announced, expected, "whether a focused orb announces");
    }

    #[rstest::rstest]
    fn an_unfocused_orb_announces() {
        // Given / When deciding while orb's pane isn't focused, with zellij saying it is.
        let announced = announces(false, || Ok(true));

        // Then the notices are announced.
        assert!(announced, "an unfocused orb announces every notice");
    }

    #[rstest::rstest]
    fn an_unfocused_orb_doesnt_ask_zellij() {
        // Given a zellij that counts how often it's asked.
        let asked = Cell::new(0);

        // When deciding while orb's pane isn't focused.
        let _announced = announces(false, || {
            asked.set(asked.get() + 1);
            Ok(false)
        });

        // Then zellij was never asked.
        assert_eq!(asked.get(), 0, "focus-out alone decides");
    }

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
    #[case(Focus::Attached, Focus::Dashboard)]
    #[case(Focus::Sidebar, Focus::Sidebar)]
    #[case(Focus::Dashboard, Focus::Dashboard)]
    #[case(Focus::Picker, Focus::Picker)]
    fn keys_leave_a_gone_pane_for_the_dashboard_only_from_the_pane(
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
    #[case(Focus::Dashboard)]
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
    #[case(Focus::Rename)]
    #[case(Focus::Search)]
    #[case(Focus::Attached)]
    fn started_thread_is_not_attached_while_typing_or_in_a_pane(#[case] focus: Focus) {
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

    #[rstest::rstest]
    fn to_drop_takes_an_exited_pane() {
        // Given pane 1 exited and pane 2 alive, both attached.
        let attached = HashSet::from([ThreadId(1), ThreadId(2)]);

        // When choosing the panes to drop.
        let dropped = to_drop([(ThreadId(1), true), (ThreadId(2), false)], &attached);

        // Then only the exited pane goes.
        assert_eq!(dropped, vec![ThreadId(1)], "the exited pane is dropped");
    }

    #[rstest::rstest]
    fn to_drop_takes_a_pane_whose_thread_left_attached() {
        // Given live pane 1 with nothing attached.
        let attached = HashSet::new();

        // When choosing the panes to drop.
        let dropped = to_drop([(ThreadId(1), false)], &attached);

        // Then the pane goes.
        assert_eq!(
            dropped,
            vec![ThreadId(1)],
            "the un-attached pane is dropped"
        );
    }

    #[rstest::rstest]
    fn to_drop_keeps_a_live_attached_pane() {
        // Given live pane 1 with thread 1 attached.
        let attached = HashSet::from([ThreadId(1)]);

        // When choosing the panes to drop.
        let dropped = to_drop([(ThreadId(1), false)], &attached);

        // Then nothing goes.
        assert!(
            dropped.is_empty(),
            "a live attached pane stays, got {dropped:?}"
        );
    }

    #[rstest::rstest]
    fn shown_pane_is_the_trust_pane_while_it_is_open() {
        // Given the trust pane open and thread 1's pane, with 1 selected and attached.
        let panes = HashMap::from([(ThreadId(1), "thread 1")]);

        // When choosing the pane to show.
        let shown = shown_pane(Some(&"trust"), &panes, Some(ThreadId(1)), Focus::Attached);

        // Then it is the trust pane.
        assert_eq!(
            shown,
            Some(&"trust"),
            "the trust pane is shown over any thread's"
        );
    }

    #[rstest::rstest]
    fn shown_pane_is_the_selected_threads_pane() {
        // Given panes for threads 1 and 2, with 2 selected in the sidebar.
        let panes = HashMap::from([(ThreadId(1), "thread 1"), (ThreadId(2), "thread 2")]);

        // When choosing the pane to show.
        let shown = shown_pane(None, &panes, Some(ThreadId(2)), Focus::Sidebar);

        // Then it is thread 2's pane.
        assert_eq!(
            shown,
            Some(&"thread 2"),
            "the selected thread's pane is shown"
        );
    }

    #[rstest::rstest]
    fn shown_pane_is_none_while_the_dashboard_has_the_keys() {
        // Given thread 1's pane, with 1 selected and the dashboard focused.
        let panes = HashMap::from([(ThreadId(1), "thread 1")]);

        // When choosing the pane to show.
        let shown = shown_pane(None, &panes, Some(ThreadId(1)), Focus::Dashboard);

        // Then no pane is shown.
        assert_eq!(shown, None, "the dashboard hides every pane");
    }
}
