//! The frontend loop: wait for input or pane activity, apply it, draw once.
//!
//! Terminal input, the pane's background threads, and the actors' state
//! changes all feed one channel. The loop sleeps until something arrives,
//! handles everything pending, and then draws a single frame, so bursts of
//! output cost one redraw. The only tick is every spinner frame (100 ms)
//! while a thread is working or a session is starting, so the spinners turn
//! and a working thread's elapsed time counts up.
//!
//! Each attached thread keeps its own attach command's pane while other
//! threads are selected; the right side shows the selected thread's pane
//! when it's attached, else the dashboard. While attached, input goes
//! straight to the attached program, except the resize keys; otherwise keys go through the resize keys, then the
//! [`keymap`]. The loop itself reads the directory picker's listings, the
//! branch picker's refs and the session picker's preview, re-reading the
//! preview whenever the selected thread's transcript changes length, and
//! hands tools to zellij, since each takes milliseconds.
//!
//! `<C-h>` moves the keys from the pane to the sidebar and leaves it drawn,
//! so `<C-l>` goes back into it; `<C-\>`, in the pane or on the thread in the
//! sidebar, detaches it, and settling or deleting the thread or its
//! attached program exiting ends the pane too.
//!
//! When a thread finishes a turn or starts needing an approval or an answer
//! while orb's pane isn't focused, the loop announces it as a desktop
//! notification; notices that arrive while it is focused are dropped. zellij
//! sends no focus-out to the tab the user leaves, so while orb seems focused
//! a thread of its own asks zellij whether any client is on orb's pane, and
//! announces the notices only if none is. That thread also asks zellij which
//! tab orb's pane is on, where a click on the notification goes back to.
//!
//! When a session start waits for the user to trust a folder, the loop asks
//! with a `No`/`Yes` confirm naming it, in place of any picker, the rename box
//! or the sidebar search; a thread's pane that had the keys loses them, as on
//! `<C-h>`. When a started draft's thread comes up still selected, the loop
//! attaches to it. If the user is typing in a picker, the rename box or the
//! sidebar search, or is in a pane, it waits, and attaches once the keys are
//! back in the sidebar or the dashboard, leaving them there with the pane
//! drawn, as long as the thread is still selected. Attaching to a thread whose
//! orb worktree is gone asks the sessions actor to recreate it instead,
//! leaving the keys on the dashboard, and attaches the same way once it's
//! back.
//!
//! orb captures the mouse throughout. Mouse events over the attached pane go
//! to the attached program; the rest are mapped through the last frame's hit map to intents
//! or a scroll of the sidebar's view (see [`mouse`]).
//!
//! After each frame the outer terminal's cursor takes the shape of where the
//! keys are: a block in the sidebar, a bar in a text input, the attached
//! program's own shape while attached.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use error_stack::{Report, ResultExt};
use jiff::tz::TimeZone;
use kameo::prelude::ActorRef;
use orb_domain::feat::git::git_service::{GitService, git_reason};
use orb_domain::feat::git::worktree::is_orb_worktree;
use orb_domain::feat::harness::Harnesses;
use orb_domain::feat::notify::notifier::NotifierService;
use orb_domain::feat::picker::state::{PickerKind, PickerState};
use orb_domain::feat::search::search_actor::{self, SearchActor};
use orb_domain::feat::sessions::sessions_actor::{self, SessionsActor};
use orb_domain::feat::sessions::state::ThreadId;
use orb_domain::feat::worktrees::worktrees_actor::{self, WorktreesActor};
use orb_domain::feat::zellij::zellij_service::{
    NOT_IN_ZELLIJ, ZellijError, ZellijService, zellij_reason,
};
use orb_domain::{AppState, Command, Focus, Intent, IntentHandler, State, Wake};
use orb_term::{Pane, PaneCommand, PaneEvent, PaneSize};
use ratatui::DefaultTerminal;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{self, Event, KeyEventKind, MouseEvent};
use ratatui::layout::Rect;
use wherror::Error;

use crate::keymap::{self, KeyScope, Keys, Route};
use crate::mouse::{self, Clicks, HitMap, MouseRoute};
use crate::picker::PickerScroll;
use crate::sidebar::{SPINNER_FRAME, SidebarScroll};
use crate::{outer_terminal, render};

/// How long after a pane starts that orb nudges its size, giving the attach
/// command time to connect and send its own size first.
const ATTACH_NUDGE: Duration = Duration::from_millis(500);

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
    /// `sessions`; worktree commands go to `worktrees`; search commands go
    /// to `search`; a thread whose worktree under `worktrees_root` is gone has
    /// it recreated before attaching; the branch picker's refs come from
    /// `git`; attached sessions run with `env`; tools open through
    /// `zellij`, `None` outside zellij; notices are announced through
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
        worktrees: ActorRef<WorktreesActor>,
        worktrees_root: PathBuf,
        search: ActorRef<SearchActor>,
        git: GitService,
        harnesses: Harnesses,
        env: Vec<(OsString, OsString)>,
        zellij: Option<ZellijService>,
        notifier: NotifierService,
    ) -> Result<(), Report<TuiRunError>> {
        let Self { tx, rx, tz } = self;
        ratatui::run(|terminal| -> io::Result<()> {
            outer_terminal::enable(terminal.backend_mut())?;
            outer_terminal::install_panic_hook();
            let result = App::new(
                state,
                sessions,
                worktrees,
                worktrees_root,
                search,
                git,
                harnesses,
                env,
                zellij,
                notifier,
                tx,
                tz,
            )
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

/// The folder to ask the user to trust: the one a start waits on, unless it
/// was asked already.
fn trust_to_open(trust: Option<&Path>, opened: Option<&Path>) -> Option<PathBuf> {
    trust.filter(|&dir| Some(dir) != opened).map(Path::to_owned)
}

/// Where the keys go once the trust confirm closes, given where they were
/// when it opened, the `return_to` of a picker it replaces, and whether the
/// sidebar is hidden: back to the sidebar or the dashboard; to where a
/// replaced picker would have returned them; to the sidebar from the rename
/// box or the search; from a pane, to the sidebar (the dashboard while it's
/// hidden), as `<C-h>` leaves it drawn.
fn trust_return_to(focus: Focus, replaced: Option<Focus>, sidebar_hidden: bool) -> Focus {
    match focus {
        Focus::Sidebar | Focus::Dashboard => focus,
        Focus::Picker => replaced.unwrap_or(Focus::Sidebar),
        Focus::Attached if sidebar_hidden => Focus::Dashboard,
        Focus::Rename | Focus::Search | Focus::Attached => Focus::Sidebar,
    }
}

/// What the loop does with a started draft's request to attach to its thread.
#[derive(Debug, PartialEq, Eq)]
enum StartedAttach {
    /// There's no request, or its thread is no longer selected: drop it.
    Drop,
    /// The user is typing (a picker, the rename box, the search) or in a
    /// pane: keep the request until the keys are back.
    Wait,
    /// Attach now. `keep_keys` leaves the keys where they are, for a request
    /// that waited.
    Attach { keep_keys: bool },
}

/// What to do with `started`, a started draft's thread, given the selection,
/// where the keys are, and whether the request has `waited` already.
fn started_attach(
    started: Option<ThreadId>,
    selected: Option<ThreadId>,
    focus: Focus,
    waited: bool,
) -> StartedAttach {
    match focus {
        _ if started.is_none() || started != selected => StartedAttach::Drop,
        Focus::Picker | Focus::Rename | Focus::Search | Focus::Attached => StartedAttach::Wait,
        Focus::Sidebar | Focus::Dashboard => StartedAttach::Attach { keep_keys: waited },
    }
}

/// What `Command::Attach` does for a thread without a live pane.
#[derive(Debug, PartialEq, Eq)]
enum AttachPlan {
    /// Start the attach command in the thread's directory.
    Spawn,
    /// The thread's orb worktree is gone: have the sessions actor recreate it.
    Restore,
}

/// Restore when `cwd` is missing and is one of orb's worktrees under
/// `worktrees_root`; otherwise spawn, so a missing directory elsewhere
/// behaves as it always has.
fn attach_or_restore(cwd: &Path, worktrees_root: &Path) -> AttachPlan {
    if cwd.is_dir() || !is_orb_worktree(worktrees_root, cwd) {
        AttachPlan::Spawn
    } else {
        AttachPlan::Restore
    }
}

/// The pane the right-hand area draws: none while the dashboard has the
/// keys, else the selected thread's.
fn shown_pane<P>(
    panes: &HashMap<ThreadId, P>,
    selected: Option<ThreadId>,
    focus: Focus,
) -> Option<&P> {
    match focus {
        Focus::Dashboard => None,
        Focus::Attached | Focus::Sidebar | Focus::Picker | Focus::Rename | Focus::Search => {
            panes.get(&selected?)
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
    /// Refreshes and deletes orb's worktrees.
    worktrees: ActorRef<WorktreesActor>,
    /// Where orb makes worktrees; a thread's missing worktree there is
    /// recreated on attach.
    worktrees_root: PathBuf,
    /// Runs transcript searches and loads their previews.
    search: ActorRef<SearchActor>,
    git: GitService,
    /// Every harness, to read a thread's transcript in its own format.
    harnesses: Harnesses,
    keys: Keys,
    /// Each attached thread's attach command, kept while other threads are
    /// selected.
    panes: HashMap<ThreadId, Pane>,
    /// The thread whose pane last got the keys, so the pane loses focus when
    /// the keys move on even though the selection already has.
    focused_pane: Option<ThreadId>,
    /// Shown on the right when a thread's attach command couldn't start.
    pane_error: Option<String>,
    /// The folder the trust confirm was opened for, while its start waits.
    opened_trust: Option<PathBuf>,
    /// A started draft's attach request is waiting for the keys to come back
    /// to the sidebar or the dashboard.
    attach_waited: bool,
    /// The environment attached sessions run with.
    env: Vec<(OsString, OsString)>,
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
    /// Where the last frame drew what a click can land on.
    hits: HitMap,
    /// The last click, for telling a double-click.
    clicks: Clicks,
    /// orb's pane has the outer terminal's focus, as its last focus event
    /// said; `true` until one arrives.
    focused: bool,
}

impl App {
    fn new(
        state: State,
        sessions: ActorRef<SessionsActor>,
        worktrees: ActorRef<WorktreesActor>,
        worktrees_root: PathBuf,
        search: ActorRef<SearchActor>,
        git: GitService,
        harnesses: Harnesses,
        env: Vec<(OsString, OsString)>,
        zellij: Option<ZellijService>,
        notifier: NotifierService,
        tx: Sender<LoopEvent>,
        tz: TimeZone,
    ) -> Self {
        let scope = {
            let state = state.read();
            KeyScope::new(state.focus, &state)
        };
        Self {
            state,
            sessions,
            worktrees,
            worktrees_root,
            search,
            git,
            harnesses,
            keys: Keys::new(keymap::keymap(), scope),
            panes: HashMap::new(),
            focused_pane: None,
            pane_error: None,
            opened_trust: None,
            attach_waited: false,
            env,
            zellij,
            notifier,
            tz,
            tx,
            pane_area: Rect::default(),
            cursor_style: SetCursorStyle::DefaultUserShape,
            sidebar_scroll: SidebarScroll::default(),
            picker_scroll: PickerScroll::default(),
            hits: HitMap::default(),
            clicks: Clicks::default(),
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
            for pane in self.panes.values_mut() {
                pane.resize(size);
            }
            let now = SystemTime::now();
            let mut drawn = (None, None);
            if self.state.read().focus == Focus::Sidebar {
                self.sidebar_scroll.release();
            }
            terminal.draw(|frame| {
                let state = self.state.read();
                let pane = shown_pane(&self.panes, state.sessions.selected_id(), state.focus)
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
                    &mut self.hits,
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
            self.refresh_preview();
            self.announce();
            self.reconcile();
            self.ask_trust();
            self.open_started();
            let now = Instant::now();
            for pane in self.panes.values_mut() {
                pane.flush_expired_sync(now);
                pane.run_nudge(now);
            }
        }
    }

    /// When the loop must wake without an event: a pane's synchronized
    /// update times out, a pane's size nudge is due, or a spinner frame
    /// passes while a thread is working or a session is starting, so the
    /// spinners and elapsed time tick.
    fn deadline(&self) -> Option<Instant> {
        let tick = self
            .state
            .read()
            .sessions
            .spinning()
            .then(|| Instant::now() + SPINNER_FRAME);
        let sync = self.panes.values().filter_map(Pane::sync_deadline);
        let nudges = self.panes.values().filter_map(Pane::nudge_deadline);
        tick.into_iter().chain(sync).chain(nudges).min()
    }

    /// The pane that receives input: the shown one, while the user is
    /// attached to it.
    fn attached_pane(&self) -> Option<&Pane> {
        let state = self.state.read();
        match state.focus {
            Focus::Attached => shown_pane(&self.panes, state.sessions.selected_id(), state.focus),
            Focus::Sidebar | Focus::Dashboard | Focus::Picker | Focus::Rename | Focus::Search => {
                None
            }
        }
    }

    /// Acts on a mouse event: forwards it to the attached pane, runs the
    /// intents it maps to (ending any key sequence in progress), or scrolls
    /// the sidebar's view.
    fn mouse(&mut self, mouse: MouseEvent) {
        let (focus, pane_shown, cursor) = {
            let state = self.state.read();
            let shown = shown_pane(&self.panes, state.sessions.selected_id(), state.focus)
                .is_some_and(|pane| !pane.has_exited());
            (state.focus, shown, state.sessions.cursor)
        };
        let route = mouse::route(
            mouse,
            &self.hits,
            focus,
            pane_shown,
            &mut self.clicks,
            Instant::now(),
        );
        match route {
            MouseRoute::Forward => {
                if let Some(pane) = self.attached_pane() {
                    pane.mouse(mouse, self.pane_area);
                }
            }
            MouseRoute::Intents(intents) => {
                self.keys.dismiss();
                for intent in &intents {
                    let commands = IntentHandler::handle(intent, &mut self.state.write());
                    for command in &commands {
                        self.execute(command);
                    }
                }
            }
            MouseRoute::ScrollSidebar(lines) => {
                self.sidebar_scroll.scroll_free(lines, cursor);
            }
            MouseRoute::Nothing => {}
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
                        match keymap::layout_route(key)
                            .or_else(|| keymap::jump_route(key))
                            .or(match focus {
                                Focus::Sidebar => keymap::sidebar_route(key),
                                _ => None,
                            }) {
                            Some(intent) => {
                                // A resize, a jump or a detach ends any key
                                // sequence in progress.
                                self.keys.dismiss();
                                Some(intent)
                            }
                            None => {
                                // Focus and the selection also change outside
                                // intents (the pane exits, a draft starts).
                                let scope = KeyScope::new(focus, &self.state.read());
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
                        self.execute(command);
                    }
                }
            }
            LoopEvent::Input(Event::Paste(text)) => {
                if let Some(pane) = self.attached_pane() {
                    pane.paste(&text);
                }
            }
            LoopEvent::Input(Event::Mouse(mouse)) => self.mouse(mouse),
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
            // Resize and the rest: the next iteration lays out and redraws;
            // `reconcile` sweeps exited thread panes.
            LoopEvent::Input(_)
            | LoopEvent::Pane(PaneEvent::Output | PaneEvent::Exited)
            | LoopEvent::StateChanged => {}
            LoopEvent::InputFailed(error) => return Err(error),
            LoopEvent::Pane(PaneEvent::Clipboard(text)) => {
                outer_terminal::copy_to_clipboard(out, &text)?;
            }
        }
        Ok(())
    }

    #[expect(clippy::too_many_lines, reason = "one arm per command")]
    fn execute(&mut self, command: &Command) {
        match command {
            Command::Attach(target) => {
                let left = self.focused_pane.take().filter(|id| *id != target.thread);
                if let Some(pane) = left.and_then(|id| self.panes.get(&id)) {
                    pane.focus(false);
                }
                if self.panes.get(&target.thread).is_none_or(Pane::has_exited) {
                    if attach_or_restore(&target.cwd, &self.worktrees_root) == AttachPlan::Restore {
                        let _ = self
                            .sessions
                            .tell(sessions_actor::RestoreWorktree(target.thread))
                            .try_send();
                        let mut app = self.state.write();
                        app.focus = Focus::Dashboard;
                        app.attached.remove(&target.thread);
                        app.sessions.starting = true;
                        return;
                    }
                    match self.spawn_pane(target.argv.clone(), target.cwd.clone()) {
                        Some(mut pane) => {
                            if target.nudge {
                                pane.nudge_after(ATTACH_NUDGE);
                            }
                            self.panes.insert(target.thread, pane);
                            self.pane_error = None;
                        }
                        None => {
                            self.pane_error = Some("couldn't start the attach command".to_owned());
                            let mut app = self.state.write();
                            app.focus = Focus::Dashboard;
                            app.attached.remove(&target.thread);
                            return;
                        }
                    }
                }
                if let Some(pane) = self.panes.get(&target.thread) {
                    pane.focus(true);
                    self.focused_pane = Some(target.thread);
                }
            }
            Command::Detach => {
                // `Intent::Detach` already gave the dashboard the keys, and a
                // jump already moved the selection, so the pane is the one
                // that last got the keys rather than the shown one.
                let focused = self
                    .focused_pane
                    .take()
                    .or_else(|| self.state.read().sessions.selected_id());
                if let Some(pane) = focused.and_then(|id| self.panes.get(&id)) {
                    pane.focus(false);
                }
            }
            Command::CreateDraft(project) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::CreateDraft(*project))
                    .try_send();
            }
            Command::SaveDraft(project) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::SaveDraft(*project))
                    .try_send();
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
            }
            Command::InitGit(project) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::InitGit(*project))
                    .try_send();
            }
            Command::StartDraft(project) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::StartDraft(*project))
                    .try_send();
            }
            Command::TrustWorkspace => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::TrustWorkspace)
                    .try_send();
            }
            Command::DeclineTrust => {
                let _ = self.sessions.tell(sessions_actor::DeclineTrust).try_send();
            }
            Command::DiscardDraft(project) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::DiscardDraft(*project))
                    .try_send();
            }
            Command::MoveThread { thread, to } => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::MoveThread {
                        thread: *thread,
                        to: to.clone(),
                    })
                    .try_send();
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
            }
            Command::CheckoutGroup { group, git_ref } => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::CheckoutGroup {
                        group: *group,
                        git_ref: git_ref.clone(),
                    })
                    .try_send();
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
            }
            Command::AddProject(root) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::AddProject(root.clone()))
                    .try_send();
            }
            Command::ListDirectories(dir) => {
                let names = list_directories(dir);
                if let Some(picker) = &mut self.state.write().picker {
                    picker.show_directories(dir, names);
                }
            }
            Command::LoadPreview { thread, transcript } => {
                self.load_preview(*thread, transcript);
            }
            Command::RefreshWorktrees => {
                let _ = self
                    .worktrees
                    .tell(worktrees_actor::RefreshWorktrees)
                    .try_send();
            }
            Command::DeleteWorktree { path } => {
                let _ = self
                    .worktrees
                    .tell(worktrees_actor::DeleteWorktree(path.clone()))
                    .try_send();
            }
            Command::SearchTranscripts { query } => {
                let _ = self
                    .search
                    .tell(search_actor::SearchTranscripts(query.clone()))
                    .try_send();
            }
            Command::LoadSearchPreview {
                hit,
                path,
                prompt_offset,
            } => {
                let _ = self
                    .search
                    .tell(search_actor::LoadSearchPreview {
                        hit: *hit,
                        path: path.clone(),
                        prompt_offset: *prompt_offset,
                    })
                    .try_send();
            }
            Command::RefreshSessions => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::RefreshSessions)
                    .try_send();
            }
            Command::Pin(id) => {
                let _ = self.sessions.tell(sessions_actor::Pin(*id)).try_send();
            }
            Command::Unpin(id) => {
                let _ = self.sessions.tell(sessions_actor::Unpin(*id)).try_send();
            }
            Command::RenameThread { thread, title } => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::RenameThread {
                        thread: *thread,
                        title: title.clone(),
                    })
                    .try_send();
            }
            Command::Settle(id) => {
                let _ = self.sessions.tell(sessions_actor::Settle(*id)).try_send();
            }
            Command::Unsettle(id) => {
                let _ = self.sessions.tell(sessions_actor::Unsettle(*id)).try_send();
            }
            Command::Delete(id) => {
                let _ = self.sessions.tell(sessions_actor::Delete(*id)).try_send();
            }
            Command::Visit(id) => {
                let _ = self.sessions.tell(sessions_actor::Visit(*id)).try_send();
            }
            Command::SaveUi => {
                let _ = self.sessions.tell(sessions_actor::SaveUi).try_send();
            }
            Command::SaveJumps => {
                let _ = self.sessions.tell(sessions_actor::SaveJumps).try_send();
            }
            Command::RemoveProject(id) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::RemoveProject(*id))
                    .try_send();
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
            }
            Command::StartGroupDraft(group) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::StartGroupDraft(*group))
                    .try_send();
            }
            Command::SaveGroupDraft(group) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::SaveGroupDraft(*group))
                    .try_send();
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
            }
            Command::PinGroup(group) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::PinGroup(*group))
                    .try_send();
            }
            Command::UnpinGroup(group) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::UnpinGroup(*group))
                    .try_send();
            }
            Command::SettleGroup(group) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::SettleGroup(*group))
                    .try_send();
            }
            Command::UnsettleGroup(group) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::UnsettleGroup(*group))
                    .try_send();
            }
            Command::DeleteGroup(group) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::DeleteGroup(*group))
                    .try_send();
            }
        }
    }

    /// Drops the panes whose attached program exited or whose thread is no
    /// longer attached, which kills their attach command, and takes them out of
    /// `attached`. If the keys were in a pane that's no longer shown, orb
    /// leaves it.
    fn reconcile(&mut self) {
        let gone = to_drop(
            self.panes.iter().map(|(id, pane)| (*id, pane.has_exited())),
            &self.state.read().attached,
        );
        for id in &gone {
            self.panes.remove(id);
        }
        let mut app = self.state.write();
        for id in &gone {
            app.attached.remove(id);
        }
        if app.focus == Focus::Attached
            && shown_pane(&self.panes, app.sessions.selected_id(), app.focus).is_none()
        {
            app.focus = after_pane(app.focus);
        }
    }

    /// Asks the user, with the `No`/`Yes` confirm, to trust the folder a
    /// session start begins waiting on, once per request (see
    /// [`trust_to_open`]). It takes the place of an open picker, the rename
    /// box or the search; a thread's pane that had the keys loses them, as on
    /// `<C-h>`. Where the keys go after is [`trust_return_to`].
    fn ask_trust(&mut self) {
        let trust = self.state.read().sessions.trust.clone();
        let ask = trust_to_open(trust.as_deref(), self.opened_trust.as_deref());
        self.opened_trust = trust;
        let Some(dir) = ask else {
            return;
        };
        self.keys.dismiss();
        let from_pane = {
            let mut app = self.state.write();
            let return_to = trust_return_to(
                app.focus,
                app.picker.as_ref().map(PickerState::return_to),
                app.sidebar.hidden,
            );
            let from_pane = app.focus == Focus::Attached;
            app.rename = None;
            app.sessions.search = None;
            app.picker = Some(PickerState::trust_workspace(dir, return_to));
            app.focus = Focus::Picker;
            from_pane
        };
        if from_pane {
            // The pane loses the keys, as on `<C-h>`.
            self.execute(&Command::Detach);
        }
    }

    /// Attaches to a started draft's thread while it's still selected, as
    /// [`started_attach`] decides: at once, or, if the user was typing or in
    /// a pane when it came up, once the keys are back in the sidebar or the
    /// dashboard, leaving them there with the pane drawn (as `<C-h>` does). A
    /// request whose thread is no longer selected is dropped for good. A
    /// failure the start still reported (saving the store) stays on the mode
    /// line.
    fn open_started(&mut self) {
        let (commands, keep_keys) = {
            let mut state = self.state.write();
            let decision = started_attach(
                state.sessions.attach,
                state.sessions.selected_id(),
                state.focus,
                self.attach_waited,
            );
            self.attach_waited = decision == StartedAttach::Wait;
            let keep_keys = match decision {
                StartedAttach::Wait => return,
                StartedAttach::Drop => {
                    state.sessions.attach = None;
                    return;
                }
                StartedAttach::Attach { keep_keys } => keep_keys,
            };
            state.sessions.attach = None;
            let focus = state.focus;
            let error = state.sessions.error.take();
            let commands = IntentHandler::handle(&Intent::Attach, &mut state);
            state.sessions.error = error;
            if keep_keys {
                state.focus = focus;
            }
            (commands, keep_keys)
        };
        for command in &commands {
            self.execute(command);
        }
        if keep_keys {
            // The pane loses the keys, as on `<C-h>`.
            self.execute(&Command::Detach);
        }
    }

    /// Reads `thread`'s transcript, in its harness's format, into the open
    /// picker's preview. An unreadable transcript, or one whose harness orb
    /// doesn't know, gives an empty preview, which shows as no transcript.
    fn load_preview(&self, thread: ThreadId, transcript: &Path) {
        let harness = self
            .state
            .read()
            .sessions
            .threads()
            .find(|candidate| candidate.id == thread)
            .and_then(|found| self.harnesses.get(&found.harness).cloned());
        let (len, exchanges) = harness
            .and_then(|harness| harness.exchanges(transcript).ok())
            .unwrap_or_default();
        if let Some(picker) = &mut self.state.write().picker {
            picker.show_preview(thread, len, exchanges);
        }
    }

    /// Reads the session picker's preview again when the selected thread's
    /// transcript isn't the length it was read at: it grew while the thread
    /// works, or it was found or replaced (`/clear`) since.
    fn refresh_preview(&self) {
        let stale = stale_preview(&self.state.read());
        if let Some((thread, transcript)) = stale {
            self.load_preview(thread, &transcript);
        }
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

    /// Runs `argv` in `cwd` in a pane the size of the pane area, with
    /// orb's child environment; `None` if it can't start.
    fn spawn_pane(&self, argv: Vec<OsString>, cwd: PathBuf) -> Option<Pane> {
        let tx = self.tx.clone();
        let command = PaneCommand {
            argv,
            cwd,
            env: self.env.clone(),
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

/// The session picker's selected thread and its transcript, when the
/// transcript's length isn't the one its preview was read at. A transcript
/// that can't be measured is left alone, and so is every other picker: the
/// search picker's previews come from the search index.
fn stale_preview(state: &AppState) -> Option<(ThreadId, PathBuf)> {
    let picker = state
        .picker
        .as_ref()
        .filter(|picker| matches!(picker.kind(), PickerKind::Sessions { .. }))?;
    let id = picker.selected_thread()?;
    let transcript = state
        .sessions
        .threads()
        .find(|thread| thread.id == id)?
        .transcript
        .as_ref()?;
    let len = fs::metadata(transcript).ok()?.len();
    picker.wants_preview(len).then(|| (id, transcript.clone()))
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
    use orb_domain::feat::harness::HarnessId;
    use std::cell::Cell;
    use std::collections::{HashMap, HashSet};
    use std::fs;
    use std::io;
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};
    use std::time::UNIX_EPOCH;

    use error_stack::Report;
    use orb_domain::feat::picker::list::PickerItem;
    use orb_domain::feat::picker::state::PickerState;
    use orb_domain::feat::sessions::state::{
        Project, ProjectId, ProjectKind, Sessions, Thread, ThreadId, ThreadStatus,
    };
    use orb_domain::feat::zellij::zellij_service::ZellijError;
    use orb_domain::{AppState, Focus};
    use ratatui::crossterm::cursor::SetCursorStyle;

    use super::{
        AttachPlan, StartedAttach, after_pane, announces, attach_or_restore, cursor_style,
        list_directories, shown_pane, stale_preview, started_attach, to_drop, trust_return_to,
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
    fn stale_preview_leaves_the_search_picker_alone() -> io::Result<()> {
        // Given thread 1 with a transcript on disk, and the search picker
        // showing a hit in it.
        let dir = tempfile::tempdir()?;
        let transcript = dir.path().join("1.jsonl");
        fs::write(&transcript, "{}\n")?;
        let state = AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "orb".to_owned(),
                    root: PathBuf::from("/repo"),
                    created_at: UNIX_EPOCH,
                    threads: vec![Thread {
                        harness: HarnessId::new("claude"),
                        id: ThreadId(1),
                        title: None,
                        cwd: "/tmp".into(),
                        transcript: Some(transcript.clone()),
                        status: ThreadStatus::Idle,
                        turn_started_at: None,
                        attach_argv: vec![],
                        branch: None,
                        pinned_at: None,
                        settled_at: None,
                        active_since: UNIX_EPOCH,
                        created_at: UNIX_EPOCH,
                        last_activity_at: UNIX_EPOCH,
                        unseen: false,
                        group: None,
                        model: None,
                        permission: None,
                    }],
                    draft: None,
                    removed: false,
                    kind: ProjectKind::Normal,
                    groups: Vec::new(),
                }],
                ..Sessions::default()
            },
            picker: Some({
                let mut picker = PickerState::search(Focus::Sidebar);
                picker.show_hits(
                    "",
                    vec![PickerItem::Hit {
                        id: 1,
                        thread: ThreadId(1),
                        label: "orb/fix".to_owned(),
                        split: 4,
                        snippet: "the parser".to_owned(),
                        lit: vec![],
                        text_lit: vec![],
                        path: transcript,
                        prompt_offset: 0,
                    }],
                    false,
                );
                picker
            }),
            ..AppState::default()
        };

        // When checking for a stale session preview.
        let stale = stale_preview(&state);

        // Then there is none to read.
        assert!(stale.is_none(), "search previews come from the index");
        Ok(())
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
    fn trust_confirm_opens_for_a_new_trust_request() {
        // Given a start waiting on /tmp/x and no trust confirm opened.
        let trust = Path::new("/tmp/x");

        // When deciding what to ask.
        let open = trust_to_open(Some(trust), None);

        // Then a trust confirm opens for /tmp/x.
        assert_eq!(
            open,
            Some(PathBuf::from("/tmp/x")),
            "a new trust request should open the confirm"
        );
    }

    #[rstest::rstest]
    fn trust_confirm_is_not_reopened_while_the_same_trust_waits() {
        // Given a start waiting on /tmp/x whose trust confirm was opened.
        let trust = Path::new("/tmp/x");

        // When deciding what to ask.
        let open = trust_to_open(Some(trust), Some(trust));

        // Then nothing opens.
        assert_eq!(open, None, "the same request shouldn't reopen the confirm");
    }

    #[rstest::rstest]
    #[case::sidebar(Focus::Sidebar, None, false, Focus::Sidebar)]
    #[case::dashboard(Focus::Dashboard, None, false, Focus::Dashboard)]
    #[case::replaced_picker(Focus::Picker, Some(Focus::Dashboard), false, Focus::Dashboard)]
    #[case::rename(Focus::Rename, None, false, Focus::Sidebar)]
    #[case::search(Focus::Search, None, false, Focus::Sidebar)]
    #[case::pane(Focus::Attached, None, false, Focus::Sidebar)]
    #[case::pane_hidden_sidebar(Focus::Attached, None, true, Focus::Dashboard)]
    fn trust_confirm_returns_the_keys_to(
        #[case] focus: Focus,
        #[case] replaced: Option<Focus>,
        #[case] sidebar_hidden: bool,
        #[case] expected: Focus,
    ) {
        // Given / When the trust confirm opens with the keys in `focus`.
        let return_to = trust_return_to(focus, replaced, sidebar_hidden);

        // Then it gives them back to `expected`, never to a picker.
        assert_eq!(
            return_to, expected,
            "where the keys go after the confirm from {focus:?}"
        );
    }

    #[rstest::rstest]
    #[case(Focus::Sidebar)]
    #[case(Focus::Dashboard)]
    fn started_thread_still_selected_is_attached(#[case] focus: Focus) {
        // Given thread 1 started from a draft and still selected.
        let started = Some(ThreadId(1));

        // When deciding what to do in `focus`.
        let decision = started_attach(started, started, focus, false);

        // Then orb attaches, taking the keys into the pane.
        assert_eq!(
            decision,
            StartedAttach::Attach { keep_keys: false },
            "a still-selected started thread should attach"
        );
    }

    #[rstest::rstest]
    fn started_thread_is_dropped_after_the_selection_moved() {
        // Given thread 1 started from a draft while thread 2 is selected.
        let started = Some(ThreadId(1));

        // When deciding what to do.
        let decision = started_attach(started, Some(ThreadId(2)), Focus::Sidebar, false);

        // Then the request is dropped.
        assert_eq!(
            decision,
            StartedAttach::Drop,
            "a moved selection shouldn't attach"
        );
    }

    #[rstest::rstest]
    #[case(Focus::Picker)]
    #[case(Focus::Rename)]
    #[case(Focus::Search)]
    #[case(Focus::Attached)]
    fn started_thread_waits_while_typing_or_in_a_pane(#[case] focus: Focus) {
        // Given thread 1 started from a draft and still selected, e.g. while
        // a picker has the keys.
        let started = Some(ThreadId(1));

        // When deciding what to do in `focus`.
        let decision = started_attach(started, started, focus, false);

        // Then the request waits instead of being dropped.
        assert_eq!(
            decision,
            StartedAttach::Wait,
            "the attach should wait in {focus:?}"
        );
    }

    #[rstest::rstest]
    #[case(Focus::Sidebar)]
    #[case(Focus::Dashboard)]
    fn waited_started_thread_attaches_once_the_keys_are_back(#[case] focus: Focus) {
        // Given thread 1's attach request waited while the user was in a
        // picker, and the thread is still selected.
        let started = Some(ThreadId(1));

        // When deciding what to do once the keys are back in `focus`.
        let decision = started_attach(started, started, focus, true);

        // Then orb attaches and leaves the keys there.
        assert_eq!(
            decision,
            StartedAttach::Attach { keep_keys: true },
            "a waited attach should keep the keys where they came back to"
        );
    }

    #[rstest::rstest]
    fn nothing_started_attaches_nothing() {
        // Given no started thread and nothing selected.

        // When deciding what to do.
        let decision = started_attach(None, None, Focus::Sidebar, false);

        // Then there's nothing to attach.
        assert_eq!(
            decision,
            StartedAttach::Drop,
            "no started thread, no attach"
        );
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
    fn shown_pane_is_the_selected_threads_pane() {
        // Given panes for threads 1 and 2, with 2 selected in the sidebar.
        let panes = HashMap::from([(ThreadId(1), "thread 1"), (ThreadId(2), "thread 2")]);

        // When choosing the pane to show.
        let shown = shown_pane(&panes, Some(ThreadId(2)), Focus::Sidebar);

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
        let shown = shown_pane(&panes, Some(ThreadId(1)), Focus::Dashboard);

        // Then no pane is shown.
        assert_eq!(shown, None, "the dashboard hides every pane");
    }

    #[rstest::rstest]
    fn attach_spawns_in_an_existing_worktree() -> io::Result<()> {
        // Given an orb worktree that is on disk.
        let root = tempfile::tempdir()?;
        let cwd = root.path().join("orb/orb-1a2b3c4d");
        fs::create_dir_all(&cwd)?;

        // When deciding how to attach to it.
        let plan = attach_or_restore(&cwd, root.path());

        // Then claude attach starts in it.
        assert_eq!(plan, AttachPlan::Spawn, "an existing worktree spawns");
        Ok(())
    }

    #[rstest::rstest]
    fn attach_restores_a_missing_orb_worktree() -> io::Result<()> {
        // Given an orb worktree whose directory is gone.
        let root = tempfile::tempdir()?;
        let cwd = root.path().join("orb/orb-1a2b3c4d");

        // When deciding how to attach to it.
        let plan = attach_or_restore(&cwd, root.path());

        // Then the sessions actor recreates it.
        assert_eq!(plan, AttachPlan::Restore, "a missing orb worktree restores");
        Ok(())
    }

    #[rstest::rstest]
    fn attach_spawns_for_a_missing_directory_outside_orbs_worktrees() -> io::Result<()> {
        // Given a missing directory outside orb's worktrees root.
        let root = tempfile::tempdir()?;
        let other = tempfile::tempdir()?;
        let cwd = other.path().join("gone");

        // When deciding how to attach to it.
        let plan = attach_or_restore(&cwd, root.path());

        // Then claude attach starts as it always has.
        assert_eq!(
            plan,
            AttachPlan::Spawn,
            "a directory orb didn't make spawns"
        );
        Ok(())
    }
}
