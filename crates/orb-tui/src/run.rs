//! The frontend loop: wait for input or pane activity, apply it, draw once.
//!
//! Terminal input, the pane's background threads, and the actors' state
//! changes all feed one channel. The loop sleeps until something arrives,
//! handles everything pending, and then draws a single frame, so bursts of
//! output cost one redraw. The only tick is every spinner frame (100 ms)
//! while a thread is working or a session is starting, so the spinners turn
//! and a working thread's elapsed time counts up.
//!
//! Each pane is a `zmx attach` client: its program runs in a zmx session, so
//! dropping the pane (a detach of its session) leaves the program running.
//! Every session's layout (tabs of split panes) is kept and saved; while a
//! session is attached, each of its panes has a client. The right side shows
//! the selected session, a tab bar over its shown tab's panes, while it is
//! attached, else the start screen. Each frame resizes every visible pane to
//! its own rect; panes of hidden tabs and other sessions keep their size. A
//! pane that leaves every layout (closed, or its session deleted) is killed
//! with `zmx kill`. A pane of an attached session whose program ended closes
//! in its layout. At start every `orb-p*` session on orb's pane dir that no
//! pane names is killed (left by earlier versions, or by panes closed while
//! orb was down), then every unsettled session is attached again (one whose
//! directory is gone only if a pane still runs). A pane whose zmx session is
//! gone gets a fresh shell, and orb types its resume command
//! (`claude --resume <id>`, `pi --session-id <id>`) once the client first
//! prints. A client
//! that dies while zmx still lists its session is attached again once; a
//! second death closes the pane with an error on the mode line. While a pane
//! has the keys, input goes straight to its program except orb's Cmd keys,
//! `<C-[>`/`<C-]>` and a `<C-g>` sequence (see [`keymap::route`]). The loop
//! itself reads the directory picker's listings, the branch picker's refs
//! and the session picker's preview, re-reading the preview whenever the
//! selected thread's transcript changes length, since each takes
//! milliseconds.
//!
//! A click on the sidebar or `Cmd h` from the leftmost pane moves the keys to
//! the sidebar and leaves the panes drawn, so `Cmd l` goes back in. Settling
//! or deleting a session ends its panes' clients; `⏎` on a settled session
//! brings its panes back the way start-up does. A pane whose program exits
//! within a second of attaching leaves `session exited at start` on the start screen.
//!
//! When a thread finishes a turn or starts needing an approval or an answer
//! while orb's terminal isn't focused, the loop sends it as a desktop
//! notification; notices that arrive while it is focused are dropped, since
//! the sidebar already shows them.
//!
//! When a new session, or one moved to another workspace, comes up still
//! selected, the loop attaches to it. If the user is typing in a picker, the rename box or the
//! sidebar search, or is in a pane, it waits, and attaches once the keys are
//! back in the sidebar, leaving them there with the pane drawn, as long as
//! the session is still selected. Attaching to a session whose orb worktree
//! is gone (none of its panes running) asks the sessions actor to recreate
//! it instead, leaving the keys in the sidebar, and attaches the same way
//! once it's back.
//!
//! orb captures the mouse throughout and maps each event through the last
//! frame's hit map, following zellij's rules (see [`mouse`]). The wheel
//! scrolls a pane's history unless its program reads the mouse or is on the
//! alternate screen. A drag in a pane whose program doesn't read the mouse
//! selects its text, and the release copies it to the clipboard through
//! OSC 52.
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
use orb_domain::feat::layout::state::{Layouts, SessionLayout};
use orb_domain::feat::notify::notifier::NotifierService;
use orb_domain::feat::picker::state::PickerKind;
use orb_domain::feat::search::search_actor::{self, SearchActor};
use orb_domain::feat::sessions::child_env::pane_env;
use orb_domain::feat::sessions::sessions_actor::{self, SessionsActor};
use orb_domain::feat::sessions::state::{PaneId, SessionId, ThreadId};
use orb_domain::feat::worktrees::worktrees_actor::{self, WorktreesActor};
use orb_domain::feat::zmx::zmx_service::{ZmxService, ZmxSession, attach_argv};
use orb_domain::{AppState, Command, Focus, Intent, IntentHandler, State, Wake};
use orb_term::{Pane, PaneCommand, PaneEvent, PaneSize};
use ratatui::DefaultTerminal;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent,
};
use ratatui::layout::{Position, Rect};
use wherror::Error;

use crate::keymap::{self, Keys, Route, Scope};
use crate::mouse::{self, Clicks, HitMap, MouseRoute};
use crate::picker::PickerScroll;
use crate::sidebar::{SPINNER_FRAME, SidebarScroll};
use crate::{outer_terminal, render, tabs};

/// How soon after its pane starts an attached program's exit counts as a
/// failed start.
const EARLY_EXIT: Duration = Duration::from_secs(1);

/// What the start screen says after an attached program exits within
/// [`EARLY_EXIT`] of starting.
const EXITED_AT_START: &str = "session exited at start";

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
    /// `git`; panes run with `env` and attach through `zmx`; notices are
    /// announced through `notifier` while orb's terminal isn't focused. The
    /// terminal is restored on exit and on panic.
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
        zmx: ZmxService,
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
                zmx,
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
    /// Pane `PaneId` reported `PaneEvent`.
    Pane(PaneId, PaneEvent),
    /// An actor changed the state.
    StateChanged,
}

/// What the loop does with a new session's request to attach to it.
#[derive(Debug, PartialEq, Eq)]
enum StartedAttach {
    /// There's no request, or its session is no longer selected: drop it.
    Drop,
    /// The user is typing (a picker, the rename box, the search) or in a
    /// pane: keep the request until the keys are back.
    Wait,
    /// Attach now. `keep_keys` leaves the keys where they are, for a request
    /// that waited.
    Attach { keep_keys: bool },
}

/// What to do with `started`, a new session's request, given the selected
/// session, where the keys are, and whether the request has `waited` already.
fn started_attach(
    started: Option<SessionId>,
    selected: Option<SessionId>,
    focus: Focus,
    waited: bool,
) -> StartedAttach {
    match focus {
        _ if started.is_none() || started != selected => StartedAttach::Drop,
        Focus::Picker | Focus::Rename | Focus::Search | Focus::Pane => StartedAttach::Wait,
        Focus::Sidebar => StartedAttach::Attach { keep_keys: waited },
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

/// Each pane of an unsettled session, with its owner and zmx session: what
/// start-up attaches again when zmx still runs it.
fn reattachable(state: &AppState) -> Vec<(SessionId, ZmxSession)> {
    state
        .layouts
        .pane_ids()
        .into_iter()
        .filter_map(|id| {
            let owner = state.layouts.owner_of(id)?;
            let unsettled = state
                .sessions
                .session(owner)
                .is_some_and(|session| session.settled_at.is_none());
            unsettled.then(|| Some((owner, state.layouts.entry(id)?.zmx.clone())))?
        })
        .collect()
}

/// Closes each `ended` pane of an `attached` session in its layout; returns
/// the sessions whose layouts changed. Panes of other sessions ended because
/// orb killed them, so their layouts keep them.
fn closed_in_layouts(
    layouts: &mut Layouts,
    attached: &HashSet<SessionId>,
    ended: &HashSet<PaneId>,
) -> HashSet<SessionId> {
    let mut edited = HashSet::new();
    for &id in ended {
        match layouts.owner_of(id) {
            Some(owner) if attached.contains(&owner) => {
                layouts.close_pane(id);
                edited.insert(owner);
            }
            _ => {}
        }
    }
    edited
}

/// The panes to drop, given each as (pane, ended): the ended ones, and
/// those no layout holds any more (`keep`).
fn to_drop<I>(panes: I, keep: &HashSet<PaneId>) -> Vec<PaneId>
where
    I: IntoIterator<Item = (PaneId, bool)>,
{
    panes
        .into_iter()
        .filter(|&(id, exited)| exited || !keep.contains(&id))
        .map(|(id, _)| id)
        .collect()
}

/// Dropped panes no layout holds any more (`all`) and that didn't end:
/// closed or deleted while running, so their zmx session is killed. A pane
/// still in a layout keeps running for its session's next attach.
fn to_kill(gone: &[PaneId], ended: &HashSet<PaneId>, all: &HashSet<PaneId>) -> Vec<PaneId> {
    gone.iter()
        .copied()
        .filter(|id| !ended.contains(id) && !all.contains(id))
        .collect()
}

/// Every kept pane without a client in `live`.
fn to_spawn<P>(keep: &HashSet<PaneId>, live: &HashMap<PaneId, P>) -> Vec<PaneId> {
    keep.iter()
        .copied()
        .filter(|id| !live.contains_key(id))
        .collect()
}

/// What to type into each pane about to get a client, given as (pane, its
/// zmx session, its resume command): the resume command, for each pane whose
/// session zmx didn't list, since the attach then makes a fresh shell.
/// `listed` maps each socket dir to the names zmx listed there, or to `None`
/// when it couldn't list it; nothing is typed there, since a live agent
/// would take the text as a prompt. A command with a control character is
/// never typed.
fn resume_plan(
    spawning: &[(PaneId, ZmxSession, Option<String>)],
    listed: &HashMap<PathBuf, Option<HashSet<String>>>,
) -> HashMap<PaneId, String> {
    spawning
        .iter()
        .filter_map(|(id, zmx, resume)| {
            let resume = resume.as_ref()?;
            let names = listed.get(&zmx.dir)?.as_ref()?;
            (!names.contains(&zmx.name) && !resume.chars().any(char::is_control))
                .then(|| (*id, resume.clone()))
        })
        .collect()
}

/// The sessions start-up attaches, from `panes` (each pane of an unsettled
/// session, see [`reattachable`]): each with a pane zmx still runs
/// (`running`), so its program shows again, and each whose directory is
/// there (`present`), so panes whose zmx session died while orb was down
/// come back as fresh shells and resume. A session whose directory is gone
/// and that runs nothing waits for `⏎`, which recreates an orb worktree
/// first.
fn attach_at_start(
    panes: &[(SessionId, ZmxSession)],
    running: &HashSet<ZmxSession>,
    present: &HashSet<SessionId>,
) -> HashSet<SessionId> {
    panes
        .iter()
        .filter(|(owner, zmx)| running.contains(zmx) || present.contains(owner))
        .map(|(owner, _)| *owner)
        .collect()
}

/// The sessions on orb's pane dir to kill at start: every `orb-p*` name no
/// pane names (`owned`). Other names are left alone.
fn stale_sessions(listed: Vec<String>, owned: &HashSet<String>) -> Vec<String> {
    listed
        .into_iter()
        .filter(|name| name.starts_with("orb-p") && !owned.contains(name))
        .collect()
}

/// Why a pane's `zmx attach` client exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaneExit {
    /// zmx no longer lists the session: its program ended, so the pane closes.
    Ended,
    /// zmx still lists it, so only the client died: attach again.
    Respawn,
    /// The client died again after one respawn: close the pane and say so.
    Failed,
}

/// How to treat a pane whose client exited, given whether zmx still `listed`
/// its session and whether it was `respawned` once already.
fn classify_exit(listed: bool, respawned: bool) -> PaneExit {
    match (listed, respawned) {
        (false, _) => PaneExit::Ended,
        (true, false) => PaneExit::Respawn,
        (true, true) => PaneExit::Failed,
    }
}

/// The start screen's pane error once panes are dropped, given whether each
/// dropped pane's program exited within [`EARLY_EXIT`] of starting:
/// `session exited at start` when one did, else `error` as it was.
fn pane_error_after<I>(error: Option<String>, early_exits: I) -> Option<String>
where
    I: IntoIterator<Item = bool>,
{
    if early_exits.into_iter().any(|early| early) {
        Some(EXITED_AT_START.to_owned())
    } else {
        error
    }
}

/// Where the keys go once the pane is gone: from the pane to the sidebar;
/// anywhere else (the sidebar after a click on it, a text input) they stay.
fn after_pane(focus: Focus) -> Focus {
    match focus {
        Focus::Pane => Focus::Sidebar,
        Focus::Sidebar | Focus::Picker | Focus::Rename | Focus::Search => focus,
    }
}

/// The outer terminal's cursor shape with the keys in `focus`: a steady
/// block on the sidebar's selected row, a steady bar in a text input (a
/// picker's filter, the rename box, the sidebar search), the child's own shape (`pane`) while attached, and the user's
/// default while attached without a pane.
fn cursor_style(focus: Focus, pane: Option<SetCursorStyle>) -> SetCursorStyle {
    match (focus, pane) {
        (Focus::Sidebar, _) => SetCursorStyle::SteadyBlock,
        (Focus::Picker | Focus::Rename | Focus::Search, _) => SetCursorStyle::SteadyBar,
        (Focus::Pane, Some(style)) => style,
        (Focus::Pane, None) => SetCursorStyle::DefaultUserShape,
    }
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
    /// Every pane orb holds a `zmx attach` client for, kept while other
    /// threads are selected.
    panes: HashMap<PaneId, Pane>,
    /// The zmx session each held pane's client attached to, for killing it
    /// once its layout no longer says.
    pane_sessions: HashMap<PaneId, ZmxSession>,
    /// The pane that last got a focus-in, so it gets a focus-out when the
    /// keys move on (see [`App::sync_focus`]).
    focused_pane: Option<PaneId>,
    /// Panes whose client exit the loop hasn't classified yet.
    exited: Vec<PaneId>,
    /// Panes already attached again once after their client died.
    respawned: HashSet<PaneId>,
    /// Resume commands waiting for their fresh pane's first output.
    typing: HashMap<PaneId, String>,
    /// Shown on the start screen when a pane's client couldn't start.
    pane_error: Option<String>,
    /// A new session's attach request is waiting for the keys to come back
    /// to the sidebar.
    attach_waited: bool,
    /// The environment panes run with.
    env: Vec<(OsString, OsString)>,
    /// Lists the zmx sessions panes attach to.
    zmx: ZmxService,
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
        zmx: ZmxService,
        notifier: NotifierService,
        tx: Sender<LoopEvent>,
        tz: TimeZone,
    ) -> Self {
        let scope = {
            let state = state.read();
            Scope::of(state.focus, &state)
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
            pane_sessions: HashMap::new(),
            focused_pane: None,
            exited: Vec::new(),
            respawned: HashSet::new(),
            typing: HashMap::new(),
            pane_error: None,
            attach_waited: false,
            env,
            zmx,
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
        self.measure(terminal.size()?.into());
        self.reattach_live();
        loop {
            self.measure(terminal.size()?.into());
            let placed = self
                .state
                .read()
                .shown_layout()
                .map(|layout| layout.placed(self.pane_area))
                .unwrap_or_default();
            for place in placed.iter().filter(|place| !place.collapsed) {
                if let Some(pane) = self.panes.get_mut(&place.pane) {
                    pane.resize(PaneSize::from(place.area));
                }
            }
            let now = SystemTime::now();
            let mut drawn = (None, None);
            if self.state.read().focus == Focus::Sidebar {
                self.sidebar_scroll.release();
            }
            terminal.draw(|frame| {
                let state = self.state.read();
                drawn = render::render(
                    frame,
                    &state,
                    &self.panes,
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
            self.open_started();
            self.sync_focus();
            let now = Instant::now();
            for pane in self.panes.values() {
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
        let sync = self.panes.values().filter_map(Pane::sync_deadline);
        tick.into_iter().chain(sync).min()
    }

    /// Lays the screen of `size` out: the tab body becomes the pane area,
    /// and the layouts measure their focus moves and resizes against it.
    fn measure(&mut self, size: Rect) {
        let [_, right, _] = render::layout(size, &self.state.read().sidebar);
        let [_, body] = tabs::areas(right);
        self.pane_area = body;
        if self.state.read().layouts.body() != body {
            self.state.write().layouts.fit_to(body);
        }
    }

    /// The shown layout's focused pane while the keys are in a pane.
    fn attached_id(&self) -> Option<PaneId> {
        let state = self.state.read();
        match state.focus {
            Focus::Pane => state.shown_layout()?.focused(),
            Focus::Sidebar | Focus::Picker | Focus::Rename | Focus::Search => None,
        }
    }

    /// The pane that receives input: the shown layout's focused pane while
    /// the keys are in a pane.
    fn attached_pane(&self) -> Option<&Pane> {
        self.panes.get(&self.attached_id()?)
    }

    /// Gives a focus-in to the pane that has the keys, the shown layout's
    /// focused pane, and a focus-out to the one that had them.
    fn sync_focus(&mut self) {
        let want = self.attached_id();
        if want == self.focused_pane {
            return;
        }
        if let Some(pane) = self.focused_pane.and_then(|id| self.panes.get(&id)) {
            pane.focus(false);
        }
        if let Some(pane) = want.and_then(|id| self.panes.get(&id)) {
            pane.focus(true);
        }
        self.focused_pane = want;
    }

    /// Acts on a mouse event: forwards it to the focused pane, runs the
    /// intents it maps to (ending any key sequence in progress), scrolls the
    /// sidebar's view, sends a wheel notch to a pane, or starts, extends or
    /// copies a pane's selection, writing the copy to the clipboard through
    /// `out`.
    fn mouse<W>(&mut self, mouse: MouseEvent, out: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        let (focus, focused, cursor) = {
            let state = self.state.read();
            (
                state.focus,
                state.shown_layout().and_then(SessionLayout::focused),
                state.sessions.cursor,
            )
        };
        let now = Instant::now();
        let route = mouse::route(mouse, &self.hits, focus, focused, &mut self.clicks, now);
        let at = Position::new(mouse.column, mouse.row);
        match route {
            MouseRoute::Forward => {
                if let Some((pane, area)) = self.attached_id().and_then(|id| self.pane_and_area(id))
                {
                    pane.mouse(mouse, area);
                }
            }
            MouseRoute::Intents(intents) => self.run_mouse_intents(&intents),
            MouseRoute::Select(intents, id) => {
                self.run_mouse_intents(&intents);
                if let Some((pane, area)) = self.pane_and_area(id) {
                    pane.select_start(at, area, now);
                }
            }
            MouseRoute::Extend(id) => {
                if let Some((pane, area)) = self.pane_and_area(id) {
                    pane.select_update(at, area, now);
                }
            }
            MouseRoute::Copy(id) => {
                if let Some(text) = self.panes.get(&id).and_then(Pane::select_finish) {
                    outer_terminal::copy_to_clipboard(out, &text)?;
                }
            }
            MouseRoute::Wheel(id) => {
                if let Some((pane, area)) = self.pane_and_area(id) {
                    pane.wheel(mouse, area);
                }
            }
            MouseRoute::ScrollSidebar(lines) => {
                self.sidebar_scroll.scroll_free(lines, cursor);
            }
            MouseRoute::Nothing => {}
        }
        Ok(())
    }

    /// Pane `id` and where the last frame drew it.
    fn pane_and_area(&self, id: PaneId) -> Option<(&Pane, Rect)> {
        Some((self.panes.get(&id)?, self.hits.pane_area(id)?))
    }

    /// Runs a mouse event's intents, ending any key sequence in progress.
    fn run_mouse_intents(&mut self, intents: &[Intent]) {
        if intents.is_empty() {
            return;
        }
        self.keys.dismiss();
        for intent in intents {
            let commands = IntentHandler::handle(intent, &mut self.state.write());
            for command in &commands {
                self.execute(command);
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
                    // The rename box and the search take the picker's keys.
                    Focus::Picker | Focus::Rename | Focus::Search => keymap::picker_route(key),
                    Focus::Sidebar | Focus::Pane => {
                        match keymap::route(key, focus == Focus::Pane, self.keys.is_pending()) {
                            Route::Intent(intent) => {
                                // A Cmd key or a jump ends any key sequence in progress.
                                self.keys.dismiss();
                                Some(intent)
                            }
                            Route::Keymap => {
                                // Focus and the selection also change outside
                                // intents (the pane exits, a new session attaches).
                                let scope = Scope::of(focus, &self.state.read());
                                if *self.keys.scope() != scope {
                                    self.keys.set_scope(scope);
                                }
                                keymap::press(&mut self.keys, key)
                            }
                            Route::Forward => {
                                if let Some(pane) = self.attached_pane() {
                                    pane.key(&key);
                                }
                                None
                            }
                        }
                    }
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
            LoopEvent::Input(Event::Mouse(mouse)) => self.mouse(mouse, out)?,
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
            LoopEvent::Pane(id, PaneEvent::Output) => self.type_resume(id),
            LoopEvent::Input(_) | LoopEvent::StateChanged => {}
            LoopEvent::Pane(id, PaneEvent::Exited) => self.exited.push(id),
            LoopEvent::InputFailed(error) => return Err(error),
            LoopEvent::Pane(_, PaneEvent::Clipboard(text)) => {
                outer_terminal::copy_to_clipboard(out, &text)?;
            }
        }
        Ok(())
    }

    #[expect(clippy::too_many_lines, reason = "one arm per command")]
    fn execute(&mut self, command: &Command) {
        match command {
            Command::Attach(session) => {
                let (dir, panes) = {
                    let app = self.state.read();
                    (
                        app.sessions
                            .session(*session)
                            .map(|shown| shown.dir.clone()),
                        app.layouts.session_panes(*session),
                    )
                };
                let live = panes
                    .iter()
                    .any(|id| self.panes.get(id).is_some_and(|pane| !pane.has_exited()));
                if !live
                    && dir.is_some_and(|dir| {
                        attach_or_restore(&dir, &self.worktrees_root) == AttachPlan::Restore
                    })
                {
                    let _ = self
                        .sessions
                        .tell(sessions_actor::RestoreWorktree(*session))
                        .try_send();
                    let mut app = self.state.write();
                    app.focus = Focus::Sidebar;
                    app.attached.remove(session);
                    app.sessions.starting = true;
                    return;
                }
                self.pane_error = None;
                self.reconcile();
                self.sync_focus();
            }
            Command::SplitPane { session, split } => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::SplitPane {
                        session: *session,
                        split: *split,
                    })
                    .try_send();
            }
            Command::AddPane(session) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::AddPane(*session))
                    .try_send();
            }
            Command::NewTab(session) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::NewTab(*session))
                    .try_send();
            }
            Command::SaveLayout(session) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::SaveLayout(*session))
                    .try_send();
            }
            Command::Detach => self.sync_focus(),
            Command::SendCtrlG => {
                if let Some(pane) = self.attached_pane() {
                    pane.key(&KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
                }
            }
            Command::NewSession { project, workspace } => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::NewSession {
                        project: *project,
                        workspace: workspace.clone(),
                    })
                    .try_send();
            }
            Command::InitGit(project) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::InitGit(*project))
                    .try_send();
            }
            Command::ChangeWorkspace { session, to } => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::ChangeWorkspace {
                        session: *session,
                        to: to.clone(),
                    })
                    .try_send();
            }
            Command::SwitchBranch {
                session,
                git_ref,
                to_root,
            } => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::SwitchBranch {
                        session: *session,
                        git_ref: git_ref.clone(),
                        to_root: *to_root,
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
            Command::PinSession(id) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::PinSession(*id))
                    .try_send();
            }
            Command::UnpinSession(id) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::UnpinSession(*id))
                    .try_send();
            }
            Command::RenameSession { session, name } => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::RenameSession {
                        session: *session,
                        name: name.clone(),
                    })
                    .try_send();
            }
            Command::SettleSession(id) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::SettleSession(*id))
                    .try_send();
            }
            Command::UnsettleSession(id) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::UnsettleSession(*id))
                    .try_send();
            }
            Command::DeleteSession(id) => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::DeleteSession(*id))
                    .try_send();
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
            Command::NewFolderSession { kind, name } => {
                let _ = self
                    .sessions
                    .tell(sessions_actor::NewFolderSession {
                        kind: *kind,
                        name: name.clone(),
                    })
                    .try_send();
            }
        }
    }

    /// Classifies each pane whose client exit arrived (see [`classify_exit`]):
    /// attaches a dead client again once, and on a second death puts
    /// `couldn't reattach to zmx session <name>` on the mode line. Returns the
    /// panes to close. An exit from a pane already replaced or dropped is
    /// ignored.
    fn classify_exits(&mut self) -> HashSet<PaneId> {
        let mut ended = HashSet::new();
        for id in mem::take(&mut self.exited) {
            if !self.panes.get(&id).is_some_and(Pane::has_exited) {
                continue;
            }
            let Some((session, cwd)) = self.pane_launch(id) else {
                ended.insert(id);
                continue;
            };
            let listed = self
                .zmx
                .list(&session.dir)
                .is_ok_and(|entries| entries.iter().any(|entry| entry.name == session.name));
            let exit = classify_exit(listed, self.respawned.contains(&id));
            let respawned = match exit {
                PaneExit::Respawn => self.spawn_pane(id, &session, &cwd),
                PaneExit::Ended | PaneExit::Failed => None,
            };
            match (exit, respawned) {
                (_, Some(pane)) => {
                    if self.focused_pane == Some(id) {
                        pane.focus(true);
                    }
                    self.panes.insert(id, pane);
                    self.pane_sessions.insert(id, session.clone());
                    self.respawned.insert(id);
                }
                (PaneExit::Ended, None) => {
                    ended.insert(id);
                }
                (PaneExit::Respawn | PaneExit::Failed, None) => {
                    ended.insert(id);
                    self.state.write().sessions.error =
                        Some(format!("couldn't reattach to zmx session {}", session.name));
                }
            }
        }
        ended
    }

    /// Drops the panes whose program ended or whose client died for good
    /// (see [`Self::classify_exits`]) and those of sessions no longer
    /// attached, which ends their `zmx attach` client. A pane of an attached
    /// session that ended closes in its layout, which is saved; a session
    /// whose layout emptied leaves `attached`. A dropped pane no layout holds
    /// any more that didn't end is killed with `zmx kill`. A pane that ended
    /// within [`EARLY_EXIT`] of starting leaves `session exited at start` on
    /// the start screen. Every pane of an attached session gets a client;
    /// when one can't start, its session leaves `attached` and `couldn't
    /// start zmx attach` shows on the start screen. If the keys were in a
    /// pane and no layout is shown any more, they go to the sidebar.
    fn reconcile(&mut self) {
        let ended = self.classify_exits();
        let (keep, all, edited) = {
            let mut app = self.state.write();
            let app = &mut *app;
            let edited = closed_in_layouts(&mut app.layouts, &app.attached, &ended);
            app.attached
                .retain(|owner| app.layouts.get(*owner).is_some());
            let keep: HashSet<PaneId> = app
                .attached
                .iter()
                .flat_map(|owner| app.layouts.session_panes(*owner))
                .collect();
            (keep, app.layouts.pane_ids(), edited)
        };
        for owner in edited {
            let _ = self
                .sessions
                .tell(sessions_actor::SaveLayout(owner))
                .try_send();
        }
        let gone = to_drop(self.panes.keys().map(|id| (*id, ended.contains(id))), &keep);
        self.pane_error = pane_error_after(
            self.pane_error.take(),
            gone.iter()
                .filter_map(|id| self.panes.get(id))
                .map(|pane| pane.exited_early(EARLY_EXIT)),
        );
        for id in to_kill(&gone, &ended, &all) {
            if let Some(session) = self.pane_sessions.get(&id) {
                let _ = self.zmx.kill(session);
            }
        }
        for id in &gone {
            self.panes.remove(id);
            self.pane_sessions.remove(id);
            self.respawned.remove(id);
            self.typing.remove(id);
        }
        let spawning = to_spawn(&keep, &self.panes);
        let plan = self.plan_resumes(&spawning);
        self.typing.extend(plan);
        for id in spawning {
            let spawned = self.pane_launch(id).and_then(|(session, cwd)| {
                self.spawn_pane(id, &session, &cwd)
                    .map(|pane| (pane, session))
            });
            match spawned {
                Some((pane, session)) => {
                    if self.focused_pane == Some(id) {
                        pane.focus(true);
                    }
                    self.panes.insert(id, pane);
                    self.pane_sessions.insert(id, session);
                }
                None => {
                    self.typing.remove(&id);
                    self.pane_error = Some("couldn't start zmx attach".to_owned());
                    let mut app = self.state.write();
                    if let Some(owner) = app.layouts.owner_of(id) {
                        app.attached.remove(&owner);
                    }
                }
            }
        }
        let mut app = self.state.write();
        if app.focus == Focus::Pane && app.shown_layout().is_none() {
            app.focus = after_pane(app.focus);
        }
    }

    /// [`resume_plan`] for `spawning`, listing once each socket dir that
    /// holds a pane with a resume command.
    fn plan_resumes(&self, spawning: &[PaneId]) -> HashMap<PaneId, String> {
        let spawning: Vec<(PaneId, ZmxSession, Option<String>)> = {
            let state = self.state.read();
            spawning
                .iter()
                .filter_map(|&id| {
                    let entry = state.layouts.entry(id)?;
                    Some((id, entry.zmx.clone(), entry.resume.clone()))
                })
                .collect()
        };
        let listed = spawning
            .iter()
            .filter(|(_, _, resume)| resume.is_some())
            .map(|(_, zmx, _)| zmx.dir.clone())
            .collect::<HashSet<PathBuf>>()
            .into_iter()
            .map(|dir| {
                let names = self
                    .zmx
                    .list(&dir)
                    .ok()
                    .map(|entries| entries.into_iter().map(|entry| entry.name).collect());
                (dir, names)
            })
            .collect();
        resume_plan(&spawning, &listed)
    }

    /// Types pane `id`'s waiting resume command, once, now that its client
    /// printed: zmx's `session "…" created` banner comes before the shell
    /// starts, and the shell reads the line at its first prompt.
    fn type_resume(&mut self, id: PaneId) {
        if let Some(line) = self.typing.remove(&id)
            && let Some(pane) = self.panes.get(&id)
        {
            pane.type_line(&line);
        }
    }

    /// Attaches to a new session's request while it's still selected, as
    /// [`started_attach`] decides: at once, or, if the user was typing or in
    /// a pane when it came up, once the keys are back in the sidebar, leaving
    /// them there with the pane drawn (as a click on the sidebar does). A
    /// request whose session is no longer selected is dropped for good. A
    /// failure the start still reported (saving the store) stays on the mode
    /// line.
    fn open_started(&mut self) {
        let (commands, keep_keys) = {
            let mut state = self.state.write();
            let decision = started_attach(
                state.sessions.attach,
                state.sessions.selected_session().map(|session| session.id),
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
            // The pane loses the keys, as on a click on the sidebar.
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

    /// Takes the sessions actor's notices and, while orb's terminal isn't
    /// focused, sends them on a thread of their own (the desktop's
    /// notification server can be slow to answer); while it is, the sidebar
    /// already shows them and they're dropped. A notice that can't be sent
    /// is dropped.
    fn announce(&self) {
        let notices = mem::take(&mut self.state.write().sessions.notices);
        if notices.is_empty() || self.focused {
            return;
        }
        let notifier = self.notifier.clone();
        let _ = thread::Builder::new()
            .name("orb-notice".into())
            .spawn(move || {
                for notice in &notices {
                    let _ = notifier.announce(notice);
                }
            });
    }

    /// Kills every `orb-p*` session on orb's pane dir that no pane names
    /// (see [`stale_sessions`]), then attaches every unsettled session whose
    /// directory is there or that has a pane zmx still runs (see
    /// [`attach_at_start`]) and gives its panes their clients, so programs
    /// left running when orb quit show again and panes whose zmx session died
    /// while orb was down come back with their agents resumed (see
    /// [`resume_plan`]). Each socket dir the panes use is listed once; a dir
    /// zmx can't list brings no running program back.
    fn reattach_live(&mut self) {
        let owned: HashSet<String> = {
            let state = self.state.read();
            state
                .layouts
                .pane_ids()
                .into_iter()
                .filter_map(|id| state.layouts.entry(id))
                .filter(|entry| entry.zmx.dir == self.zmx.dir())
                .map(|entry| entry.zmx.name.clone())
                .collect()
        };
        let listed = self
            .zmx
            .list(self.zmx.dir())
            .unwrap_or_default()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        for name in stale_sessions(listed, &owned) {
            let _ = self.zmx.kill(&self.zmx.session(name));
        }
        let panes = reattachable(&self.state.read());
        let running: HashSet<ZmxSession> = {
            let dirs: HashSet<&Path> = panes.iter().map(|(_, zmx)| zmx.dir.as_path()).collect();
            dirs.into_iter()
                .flat_map(|dir| {
                    self.zmx
                        .list(dir)
                        .unwrap_or_default()
                        .into_iter()
                        .map(move |entry| ZmxSession {
                            name: entry.name,
                            dir: dir.to_owned(),
                        })
                })
                .collect()
        };
        let present: HashSet<SessionId> = {
            let state = self.state.read();
            panes
                .iter()
                .map(|(owner, _)| *owner)
                .filter(|owner| {
                    state
                        .sessions
                        .session(*owner)
                        .is_some_and(|session| session.dir.is_dir())
                })
                .collect()
        };
        let owners = attach_at_start(&panes, &running, &present);
        self.state.write().attached.extend(owners);
        self.reconcile();
    }

    /// Where pane `id` runs, as its layout says: its zmx session and its
    /// directory. `None` once no layout holds it.
    fn pane_launch(&self, id: PaneId) -> Option<(ZmxSession, PathBuf)> {
        let state = self.state.read();
        let entry = state.layouts.entry(id)?;
        Some((entry.zmx.clone(), entry.cwd.clone()))
    }

    /// Runs `zmx attach` on `session` for pane `id`, in `cwd`, which starts
    /// the user's shell when zmx makes the session,
    /// sized to its place in the shown layout (else, or when it's a stack's
    /// title, the whole pane area),
    /// with orb's child environment and the pane's `ORB_PANE_ID`; `None` if
    /// it can't start.
    fn spawn_pane(&self, id: PaneId, session: &ZmxSession, cwd: &Path) -> Option<Pane> {
        let tx = self.tx.clone();
        let command = PaneCommand {
            argv: attach_argv(session),
            cwd: cwd.to_owned(),
            env: pane_env(&self.env, id),
        };
        let area = self
            .state
            .read()
            .shown_layout()
            .and_then(|layout| {
                layout
                    .placed(self.pane_area)
                    .into_iter()
                    .find(|place| place.pane == id && !place.collapsed)
            })
            .map_or(self.pane_area, |place| place.area);
        Pane::spawn(&command, PaneSize::from(area), move |event| {
            let _ = tx.send(LoopEvent::Pane(id, event));
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
    use std::collections::{HashMap, HashSet};
    use std::fs;
    use std::io;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;
    use std::time::UNIX_EPOCH;

    use orb_domain::feat::layout::state::{Layouts, PaneEntry, SessionLayout};
    use orb_domain::feat::layout::tree::Split;
    use orb_domain::feat::picker::list::PickerItem;
    use orb_domain::feat::picker::state::PickerState;
    use orb_domain::feat::sessions::state::{
        PaneId, Project, ProjectId, ProjectKind, Session, SessionId, SessionKind, Sessions, Thread,
        ThreadId, ThreadStatus,
    };
    use orb_domain::feat::zmx::zmx_service::ZmxSession;
    use orb_domain::{AppState, Focus};
    use ratatui::crossterm::cursor::SetCursorStyle;

    use super::{
        AttachPlan, PaneExit, StartedAttach, after_pane, attach_at_start, attach_or_restore,
        classify_exit, closed_in_layouts, cursor_style, list_directories, pane_error_after,
        reattachable, resume_plan, stale_preview, stale_sessions, started_attach, to_drop, to_kill,
        to_spawn,
    };

    /// Pane `id`, a shell in `orb-p<id>`.
    fn entry(id: i64) -> PaneEntry {
        PaneEntry {
            id: PaneId(id),
            zmx: ZmxSession {
                name: format!("orb-p{id}"),
                dir: "/zmx".into(),
            },
            cwd: "/tmp".into(),
            name: None,
            resume: None,
        }
    }

    /// Session 1 holding panes 1 and 2 side by side.
    fn split_session() -> Layouts {
        let mut layouts = Layouts::default();
        layouts.insert(SessionId(1), SessionLayout::of(entry(1)));
        layouts.split(SessionId(1), Split::Right, entry(2));
        layouts
    }

    #[rstest::rstest]
    fn ended_pane_of_an_attached_session_closes_in_its_layout() {
        // Given attached session 1 with panes 1 and 2.
        let mut layouts = split_session();
        let attached = HashSet::from([SessionId(1)]);

        // When pane 2 ends.
        closed_in_layouts(&mut layouts, &attached, &HashSet::from([PaneId(2)]));

        // Then only pane 1 is left in its layout.
        assert_eq!(
            layouts.session_panes(SessionId(1)),
            [PaneId(1)],
            "an attached session's ended pane closes"
        );
    }

    #[rstest::rstest]
    fn ended_pane_of_an_unattached_session_stays_in_its_layout() {
        // Given session 1 with panes 1 and 2, not attached.
        let mut layouts = split_session();

        // When pane 2 ends.
        closed_in_layouts(&mut layouts, &HashSet::new(), &HashSet::from([PaneId(2)]));

        // Then both panes are still in its layout.
        assert_eq!(
            layouts.session_panes(SessionId(1)).len(),
            2,
            "orb killed it, so the layout keeps it"
        );
    }

    /// Session `id` of project 1, settled at `settled`.
    fn session(id: i64, settled: bool) -> Session {
        Session {
            id: SessionId(id),
            project: ProjectId(1),
            kind: SessionKind::Plain,
            dir: "/tmp".into(),
            name: None,
            branch: None,
            created_at: UNIX_EPOCH,
            pinned_at: None,
            settled_at: settled.then_some(UNIX_EPOCH),
            active_since: UNIX_EPOCH,
            last_activity_at: UNIX_EPOCH,
        }
    }

    #[rstest::rstest]
    fn reattach_skips_settled_sessions() {
        // Given unsettled session 1 with pane 1 and settled session 2 with
        // pane 2.
        let mut state = AppState::default();
        state.sessions.sessions = vec![session(1, false), session(2, true)];
        state
            .layouts
            .insert(SessionId(1), SessionLayout::of(entry(1)));
        state
            .layouts
            .insert(SessionId(2), SessionLayout::of(entry(2)));

        // When picking what start-up attaches again.
        let owners: Vec<SessionId> = reattachable(&state)
            .into_iter()
            .map(|(owner, _)| owner)
            .collect();

        // Then only session 1 is.
        assert_eq!(
            owners,
            [SessionId(1)],
            "a settled session's panes stay down"
        );
    }

    #[rstest::rstest]
    #[case::sidebar_block(Focus::Sidebar, None, SetCursorStyle::SteadyBlock)]
    #[case::picker_bar(Focus::Picker, None, SetCursorStyle::SteadyBar)]
    #[case::rename_bar(Focus::Rename, None, SetCursorStyle::SteadyBar)]
    #[case::search_bar(Focus::Search, None, SetCursorStyle::SteadyBar)]
    #[case::pane_follows_its_program(
        Focus::Pane,
        Some(SetCursorStyle::BlinkingUnderScore),
        SetCursorStyle::BlinkingUnderScore
    )]
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
                        last_session: None,
                        harness: HarnessId::new("claude"),
                        id: ThreadId(1),
                        title: None,
                        cwd: "/tmp".into(),
                        transcript: Some(transcript.clone()),
                        status: ThreadStatus::Idle,
                        turn_started_at: None,
                        pane: None,
                        branch: None,
                        created_at: UNIX_EPOCH,
                        last_activity_at: UNIX_EPOCH,
                        unseen: false,
                        model: None,
                    }],
                    repo: true,
                    removed: false,
                    kind: ProjectKind::Normal,
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
    #[case(Focus::Pane, Focus::Sidebar)]
    #[case(Focus::Sidebar, Focus::Sidebar)]
    #[case(Focus::Picker, Focus::Picker)]
    fn keys_leave_a_gone_pane_for_the_sidebar_only_from_the_pane(
        #[case] focus: Focus,
        #[case] expected: Focus,
    ) {
        // Given / When / Then only a pane focus moves to the sidebar once the
        // pane is gone.
        assert_eq!(
            after_pane(focus),
            expected,
            "focus after the pane goes from {focus:?}"
        );
    }

    #[rstest::rstest]
    fn started_session_still_selected_is_attached() {
        // Given new session 1 still selected.
        let started = Some(SessionId(1));

        // When deciding what to do in the sidebar.
        let decision = started_attach(started, started, Focus::Sidebar, false);

        // Then orb attaches, taking the keys into the pane.
        assert_eq!(
            decision,
            StartedAttach::Attach { keep_keys: false },
            "a still-selected started thread should attach"
        );
    }

    #[rstest::rstest]
    fn started_session_is_dropped_after_the_selection_moved() {
        // Given new session 1 while session 2 is selected.
        let started = Some(SessionId(1));

        // When deciding what to do.
        let decision = started_attach(started, Some(SessionId(2)), Focus::Sidebar, false);

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
    #[case(Focus::Pane)]
    fn started_session_waits_while_typing_or_in_a_pane(#[case] focus: Focus) {
        // Given new session 1 still selected. e.g. while
        // a picker has the keys.
        let started = Some(SessionId(1));

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
    fn waited_started_thread_attaches_once_the_keys_are_back() {
        // Given thread 1's attach request waited while the user was in a
        // picker, and the thread is still selected.
        let started = Some(SessionId(1));

        // When deciding what to do once the keys are back in the sidebar.
        let decision = started_attach(started, started, Focus::Sidebar, true);

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
        // Given pane 1 exited and pane 2 alive, both in a layout.
        let keep = HashSet::from([PaneId(1), PaneId(2)]);

        // When choosing the panes to drop.
        let dropped = to_drop([(PaneId(1), true), (PaneId(2), false)], &keep);

        // Then only the exited pane goes.
        assert_eq!(dropped, vec![PaneId(1)], "the exited pane is dropped");
    }

    #[rstest::rstest]
    fn to_drop_takes_a_pane_in_no_layout() {
        // Given live pane 1 that no layout holds.
        let keep = HashSet::new();

        // When choosing the panes to drop.
        let dropped = to_drop([(PaneId(1), false)], &keep);

        // Then the pane goes.
        assert_eq!(dropped, vec![PaneId(1)], "a pane in no layout is dropped");
    }

    #[rstest::rstest]
    fn to_drop_keeps_a_live_pane_in_a_layout() {
        // Given live pane 1 held by a layout.
        let keep = HashSet::from([PaneId(1)]);

        // When choosing the panes to drop.
        let dropped = to_drop([(PaneId(1), false)], &keep);

        // Then nothing goes.
        assert!(
            dropped.is_empty(),
            "a live pane in a layout stays, got {dropped:?}"
        );
    }

    #[rstest::rstest]
    fn to_kill_takes_a_dropped_pane_no_layout_holds() {
        // Given pane 7 dropped while running, in no layout any more.
        let gone = [PaneId(7)];

        // When choosing the zmx sessions to kill.
        let killed = to_kill(&gone, &HashSet::new(), &HashSet::new());

        // Then its session is killed.
        assert_eq!(killed, vec![PaneId(7)], "a closed pane's program is killed");
    }

    #[rstest::rstest]
    fn to_kill_leaves_a_dropped_pane_still_in_a_layout() {
        // Given pane 7 dropped by a detach, still in its session's layout.
        let gone = [PaneId(7)];

        // When choosing the zmx sessions to kill.
        let killed = to_kill(&gone, &HashSet::new(), &HashSet::from([PaneId(7)]));

        // Then its program keeps running.
        assert!(killed.is_empty(), "a laid-out pane outlives its detach");
    }

    #[rstest::rstest]
    fn to_kill_leaves_a_pane_that_ended() {
        // Given pane 7 dropped because its program ended.
        let gone = [PaneId(7)];

        // When choosing the zmx sessions to kill.
        let killed = to_kill(&gone, &HashSet::from([PaneId(7)]), &HashSet::new());

        // Then nothing is killed.
        assert!(killed.is_empty(), "an ended pane has nothing to kill");
    }

    #[rstest::rstest]
    fn to_spawn_takes_every_kept_pane_without_a_client() {
        // Given kept panes 7 and 8, pane 8 with a client.
        let keep = HashSet::from([PaneId(7), PaneId(8)]);
        let live: HashMap<PaneId, &str> = HashMap::from([(PaneId(8), "client")]);

        // When choosing the panes to start.
        let spawned = to_spawn(&keep, &live);

        // Then pane 7 gets a client.
        assert_eq!(
            spawned,
            vec![PaneId(7)],
            "a kept pane without a client starts"
        );
    }

    /// Pane `id` about to get a client, in `orb-p<id>` on `/zmx`, with
    /// `resume` as its resume command.
    fn spawning(id: i64, resume: Option<&str>) -> (PaneId, ZmxSession, Option<String>) {
        (
            PaneId(id),
            ZmxSession {
                name: format!("orb-p{id}"),
                dir: "/zmx".into(),
            },
            resume.map(str::to_owned),
        )
    }

    /// zmx listing `names` on `/zmx`.
    fn zmx_lists(names: &[&str]) -> HashMap<PathBuf, Option<HashSet<String>>> {
        HashMap::from([(
            PathBuf::from("/zmx"),
            Some(names.iter().map(|name| (*name).to_owned()).collect()),
        )])
    }

    #[rstest::rstest]
    fn resume_plan_types_a_panes_resume_when_zmx_no_longer_runs_it() {
        // Given pane 1 resuming aa, and zmx running nothing on its dir.
        let panes = [spawning(1, Some("claude --resume aa"))];

        // When planning what to type.
        let plan = resume_plan(&panes, &zmx_lists(&[]));

        // Then pane 1's fresh shell gets its resume command.
        assert_eq!(
            plan,
            HashMap::from([(PaneId(1), "claude --resume aa".to_owned())]),
            "a fresh shell resumes its pane's conversation"
        );
    }

    #[rstest::rstest]
    fn resume_plan_types_nothing_into_a_running_session() {
        // Given pane 1 resuming aa, and zmx still running orb-p1.
        let panes = [spawning(1, Some("claude --resume aa"))];

        // When planning what to type.
        let plan = resume_plan(&panes, &zmx_lists(&["orb-p1"]));

        // Then nothing is typed.
        assert!(
            plan.is_empty(),
            "a running program must not get the command"
        );
    }

    #[rstest::rstest]
    fn resume_plan_types_nothing_into_a_pane_without_a_resume_command() {
        // Given pane 1 with nothing to resume, and zmx running nothing.
        let panes = [spawning(1, None)];

        // When planning what to type.
        let plan = resume_plan(&panes, &zmx_lists(&[]));

        // Then nothing is typed.
        assert!(plan.is_empty(), "a plain shell stays plain");
    }

    #[rstest::rstest]
    fn resume_plan_types_nothing_where_zmx_couldnt_list() {
        // Given pane 1 resuming aa, and zmx failing to list its dir.
        let panes = [spawning(1, Some("claude --resume aa"))];
        let listed = HashMap::from([(PathBuf::from("/zmx"), None)]);

        // When planning what to type.
        let plan = resume_plan(&panes, &listed);

        // Then nothing is typed.
        assert!(plan.is_empty(), "an unknown session might be running");
    }

    #[rstest::rstest]
    fn resume_plan_never_types_a_command_with_a_control_character() {
        // Given pane 1 whose resume command holds a newline.
        let panes = [spawning(1, Some("claude --resume aa\nrm -rf ~"))];

        // When planning what to type.
        let plan = resume_plan(&panes, &zmx_lists(&[]));

        // Then nothing is typed.
        assert!(
            plan.is_empty(),
            "a control character could run a second command"
        );
    }

    /// Session 1's pane 1, in `orb-p1` on `/zmx`.
    fn start_pane() -> [(SessionId, ZmxSession); 1] {
        [(
            SessionId(1),
            ZmxSession {
                name: "orb-p1".into(),
                dir: "/zmx".into(),
            },
        )]
    }

    #[rstest::rstest]
    fn start_attaches_a_session_whose_panes_all_died() {
        // Given session 1's directory there and its pane no longer running.
        let panes = start_pane();

        // When choosing what to attach at start.
        let owners = attach_at_start(&panes, &HashSet::new(), &HashSet::from([SessionId(1)]));

        // Then session 1 is attached.
        assert_eq!(
            owners,
            HashSet::from([SessionId(1)]),
            "dead panes come back as fresh shells"
        );
    }

    #[rstest::rstest]
    fn start_attaches_a_session_with_a_running_pane_whose_directory_is_gone() {
        // Given session 1's pane still running and its directory gone.
        let panes = start_pane();
        let running = HashSet::from([panes[0].1.clone()]);

        // When choosing what to attach at start.
        let owners = attach_at_start(&panes, &running, &HashSet::new());

        // Then session 1 is attached.
        assert_eq!(
            owners,
            HashSet::from([SessionId(1)]),
            "a running program shows again"
        );
    }

    #[rstest::rstest]
    fn start_leaves_a_session_with_its_directory_gone_and_nothing_running() {
        // Given session 1's directory gone and nothing of it running.
        let panes = start_pane();

        // When choosing what to attach at start.
        let owners = attach_at_start(&panes, &HashSet::new(), &HashSet::new());

        // Then session 1 waits for ⏎.
        assert!(owners.is_empty(), "⏎ recreates its worktree first");
    }

    #[rstest::rstest]
    fn stale_sessions_takes_old_thread_and_split_names() {
        // Given zmx listing phase-era names no pane owns.
        let listed = vec!["orb-p3".to_owned(), "orb-p-1".to_owned()];

        // When choosing what to kill.
        let stale = stale_sessions(listed, &HashSet::new());

        // Then both are killed.
        assert_eq!(
            stale,
            vec!["orb-p3".to_owned(), "orb-p-1".to_owned()],
            "leftover thread and split sessions go"
        );
    }

    #[rstest::rstest]
    fn stale_sessions_spares_a_pane_rows_session() {
        // Given zmx listing orb-p12, which pane 12 owns.
        let listed = vec!["orb-p12".to_owned()];

        // When choosing what to kill.
        let stale = stale_sessions(listed, &HashSet::from(["orb-p12".to_owned()]));

        // Then nothing is killed.
        assert!(stale.is_empty(), "a pane's own session stays");
    }

    #[rstest::rstest]
    fn stale_sessions_spares_names_without_the_orb_p_prefix() {
        // Given zmx listing a session orb didn't name.
        let listed = vec!["work".to_owned()];

        // When choosing what to kill.
        let stale = stale_sessions(listed, &HashSet::new());

        // Then it is left alone.
        assert!(stale.is_empty(), "only orb-p* sessions are orb's");
    }

    #[rstest::rstest]
    #[case::ended(false, false, PaneExit::Ended)]
    #[case::ended_after_a_respawn(false, true, PaneExit::Ended)]
    #[case::client_died(true, false, PaneExit::Respawn)]
    #[case::client_died_again(true, true, PaneExit::Failed)]
    fn pane_exit_is_classified_by_zmx_listing_and_an_earlier_respawn(
        #[case] listed: bool,
        #[case] respawned: bool,
        #[case] expected: PaneExit,
    ) {
        // Given / When a pane's client exits, zmx `listed` its session or
        // not, and the pane was `respawned` once or not.
        let exit = classify_exit(listed, respawned);

        // Then the exit is `expected`.
        assert_eq!(exit, expected, "listed {listed}, respawned {respawned}");
    }

    #[rstest::rstest]
    fn early_exit_sets_session_exited_at_start() {
        // Given no pane error.
        let error = None;

        // When one of the dropped panes exited early.
        let error = pane_error_after(error, [false, true]);

        // Then the start screen says the session exited at start.
        assert_eq!(
            error.as_deref(),
            Some("session exited at start"),
            "an early exit sets the pane error"
        );
    }

    #[rstest::rstest]
    fn later_exit_keeps_the_pane_error() {
        // Given an existing pane error.
        let error = Some("couldn't start the attach command".to_owned());

        // When the only dropped pane exited later.
        let error = pane_error_after(error, [false]);

        // Then the pane error is unchanged.
        assert_eq!(
            error.as_deref(),
            Some("couldn't start the attach command"),
            "a later exit leaves the pane error as is"
        );
    }

    #[rstest::rstest]
    fn attach_spawns_in_an_existing_worktree() -> io::Result<()> {
        // Given an orb worktree that is on disk.
        let root = tempfile::tempdir()?;
        let cwd = root.path().join("orb/orb-1a2b3c4d");
        fs::create_dir_all(&cwd)?;

        // When deciding how to attach to it.
        let plan = attach_or_restore(&cwd, root.path());

        // Then its panes start in it.
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

        // Then its panes start as they always have.
        assert_eq!(
            plan,
            AttachPlan::Spawn,
            "a directory orb didn't make spawns"
        );
        Ok(())
    }
}
