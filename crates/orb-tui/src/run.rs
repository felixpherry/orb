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
//! the inside of its frame; panes of hidden tabs and other sessions keep their size. A
//! pane that leaves every layout (closed, or its session deleted) is killed
//! with `zmx kill`. A pane of an attached session whose program ended closes
//! in its layout. At start every `orb-p*` session on orb's pane dir that no
//! pane names is killed (left by earlier versions, or by panes closed while
//! orb was down), then every unsettled session is attached again (one whose
//! directory is gone only if a pane still runs). A pane whose zmx session is
//! gone gets a fresh shell, and orb types its resume command
//! (`claude --resume <id>`, `pi --session-id <id>`) once the client first
//! prints. A client that dies while its session is still shown and zmx still
//! lists it is attached again once; a second death closes the pane with an
//! error in the message box. One whose session isn't shown any more is
//! dropped, keeping its program. While a pane
//! has the keys, input goes straight to its program except orb's Cmd keys,
//! `<C-[>`/`<C-]>` and a `<C-g>` sequence (see [`keymap::route`]). The loop
//! itself reads the directory picker's listings, the branch picker's refs
//! and the session picker's preview, re-reading the preview whenever the
//! selected thread's transcript changes length, since each takes
//! milliseconds.
//!
//! A click on the sidebar, `<C-S-h>` or `<C-g> e` from a pane moves the keys
//! to the sidebar and leaves the panes drawn, so `<C-S-l>` or `<C-g> e` goes
//! back in. Settling
//! or deleting a session ends its panes' clients; `⏎` on a settled session
//! brings its panes back the way start-up does. A pane whose program exits
//! within a second of attaching leaves `session <title> exited at start` on
//! the start screen.
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
//! frame's hit map (see [`mouse`]). The wheel
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
use orb_domain::feat::picker::state::{PickerKind, worktree_items};
use orb_domain::feat::search::search_actor::SearchActor;
use orb_domain::feat::sessions::child_env::pane_env;
use orb_domain::feat::sessions::sessions_actor::SessionsActor;
use orb_domain::feat::sessions::state::{Notice, PaneId, SessionId, ThreadId};
use orb_domain::feat::sessions::validator::validate_new_session;
use orb_domain::feat::sidebar::state::SidebarLayout;
use orb_domain::feat::worktrees::worktrees_actor::WorktreesActor;
use orb_domain::feat::zmx::zmx_service::{ZmxService, ZmxSession, attach_argv};
use orb_domain::{
    AppState, Command, Focus, Intent, IntentHandler, Log, Outcome, Reporter, SessionsCommand, State,
};
use orb_term::{Pane, PaneCommand, PaneEvent, PaneSize};
use ratatui::DefaultTerminal;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent,
};
use ratatui::crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use ratatui::layout::{Margin, Position, Rect};
use wherror::Error;

use crate::keymap::{self, Keys, Route, Scope};
use crate::mouse::{self, Clicks, HitMap, MouseRoute};
use crate::picker::PickerScroll;
use crate::sidebar::{SPINNER_FRAME, SidebarScroll};
use crate::toast::Toasts;
use crate::{outer_terminal, render, tabs};

/// How soon after its pane starts an attached program's exit counts as a
/// failed start.
const EARLY_EXIT: Duration = Duration::from_secs(1);

/// How often the session picker re-reads the same thread's growing transcript.
const PREVIEW_EVERY: Duration = Duration::from_secs(1);

/// What the start screen says after an attached program exits within
/// [`EARLY_EXIT`] of starting.
const EXITED_AT_START: &str = "session exited at start";

/// The frontend loop failed to draw a frame or read a terminal event.
#[derive(Debug, Error)]
#[error(debug)]
pub struct TuiRunError;

/// orb's TUI. Created before the actors so they can report to its loop.
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

    /// Sends an actor's outcomes to the loop, which applies each one.
    pub fn reporter(&self) -> Reporter {
        let tx = self.tx.clone();
        Reporter::new(move |outcome| {
            let _ = tx.send(LoopEvent::Reported(outcome));
        })
    }

    /// Quits orb the way `<C-g> q` does, from outside the loop. A quit sent
    /// before the loop starts is handled on its first turn.
    pub fn quitter(&self) -> impl Fn() + Send + Sync + 'static {
        let tx = self.tx.clone();
        move || {
            let _ = tx.send(LoopEvent::Quit);
        }
    }

    /// Runs orb's TUI until the user quits or [`Frontend::quitter`] is called.
    /// Session commands go to `sessions`; worktree commands go to
    /// `worktrees`; search commands go to `search`; a thread whose worktree
    /// under `worktrees_root` is gone has it recreated before attaching; the
    /// branch picker's refs come from `git`; panes run with `env` and attach
    /// through `zmx`; notices are
    /// announced through `notifier` while orb's terminal isn't focused;
    /// problems are written to `log`. The first frame is drawn before the
    /// panes of open sessions reconnect. The terminal is restored on exit and
    /// on panic.
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
        log: Log,
    ) -> Result<(), Report<TuiRunError>> {
        let Self { tx, rx, tz } = self;
        ratatui::run(|terminal| -> io::Result<()> {
            outer_terminal::enable(terminal.backend_mut())?;
            outer_terminal::install_panic_hook(log.clone());
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
                attach_argv,
                notifier,
                log,
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
    /// A worker reported an outcome.
    Reported(Outcome),
    /// orb was asked to quit from outside (a signal).
    Quit,
}

/// A session a worker made ready to open, waiting in the loop until it can.
#[derive(Debug, Clone, Copy)]
struct Ready {
    session: SessionId,
    /// The session that was active when the user asked.
    if_active: Option<SessionId>,
    /// It waited for the keys to come back to the sidebar.
    waited: bool,
}

/// What the loop does with a session a worker made ready to open.
#[derive(Debug, PartialEq, Eq)]
enum StartedAttach {
    /// The user opened another session meanwhile: drop it.
    Drop,
    /// The user is typing (a picker, the rename box, the search) or in a
    /// pane: keep the request until the keys are back.
    Wait,
    /// Attach now. `keep_keys` leaves the keys where they are, for a request
    /// that waited.
    Attach { keep_keys: bool },
}

/// What to do with a ready session, asked for while `if_active` was the
/// active session, given the `active` session now, where the keys are, and
/// whether it has `waited` already. Moving the cursor around doesn't cancel
/// it; opening another session does.
fn started_attach(
    if_active: Option<SessionId>,
    active: Option<SessionId>,
    focus: Focus,
    waited: bool,
) -> StartedAttach {
    match focus {
        _ if if_active != active => StartedAttach::Drop,
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

/// Every kept pane without a client in `live`, except the `broken` ones.
fn to_spawn<P>(
    keep: &HashSet<PaneId>,
    live: &HashMap<PaneId, P>,
    broken: &HashSet<PaneId>,
) -> Vec<PaneId> {
    keep.iter()
        .copied()
        .filter(|id| !live.contains_key(id) && !broken.contains(id))
        .collect()
}

/// Forgets the `broken` panes no attached session holds (`keep`), so they
/// get a client again the next time their session is attached.
fn clear_detached(broken: &mut HashSet<PaneId>, keep: &HashSet<PaneId>) {
    broken.retain(|id| keep.contains(id));
}

/// The message box text after pane `name` stopped showing.
fn broken_pane_message(name: &str) -> String {
    format!("stopped showing pane {name} after an error; it keeps running")
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

/// The owners in `panes` whose session's directory is there.
fn present_at_start(state: &AppState, panes: &[(SessionId, ZmxSession)]) -> HashSet<SessionId> {
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
}

/// The sessions on orb's pane dir to kill at start: every `orb-p*` name no
/// pane names (`owned`). Other names are left alone, and so is every name
/// after a `partial_load`, since a pane orb couldn't read may own it.
fn stale_sessions(listed: Vec<String>, owned: &HashSet<String>, partial_load: bool) -> Vec<String> {
    if partial_load {
        return Vec::new();
    }
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
    /// zmx still lists it, or couldn't say, so only the client died: attach
    /// again.
    Respawn,
    /// The client died again after one respawn: its session detaches, keeping
    /// the pane, and orb says so.
    Failed,
    /// Its session isn't shown: only the client is dropped, keeping the pane
    /// and its program for the session's next attach.
    Drop,
}

/// How to treat a pane whose client exited, given whether zmx still `listed`
/// its session (`None` when zmx couldn't be asked), whether it was
/// `respawned` once already, and whether its session is `shown` (on the
/// right side). A pane whose session isn't shown is only dropped, whatever
/// zmx says. Otherwise only a session zmx positively no longer lists has
/// ended.
fn classify_exit(listed: Option<bool>, respawned: bool, shown: bool) -> PaneExit {
    match (shown, listed, respawned) {
        (false, _, _) => PaneExit::Drop,
        (true, Some(false), _) => PaneExit::Ended,
        (true, Some(true) | None, false) => PaneExit::Respawn,
        (true, Some(true) | None, true) => PaneExit::Failed,
    }
}

/// What happens to a pane after its client's exit is classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitOutcome {
    /// A new client attached: the pane stays, marked respawned.
    Reattached,
    /// Its program ended: the pane closes.
    Closed,
    /// No client could attach: its session detaches, keeping the pane, and
    /// the mode line says so.
    Detached,
    /// Its session isn't shown: the client is dropped, and the pane and its
    /// program stay for the session's next attach.
    Dropped,
}

/// The outcome of `exit` once a new client did (`reattached`) or didn't start.
fn exit_outcome(exit: PaneExit, reattached: bool) -> ExitOutcome {
    match (exit, reattached) {
        (_, true) => ExitOutcome::Reattached,
        (PaneExit::Ended, false) => ExitOutcome::Closed,
        (PaneExit::Respawn | PaneExit::Failed, false) => ExitOutcome::Detached,
        (PaneExit::Drop, false) => ExitOutcome::Dropped,
    }
}

/// The start screen's pane error once panes are dropped, given for each
/// dropped pane whether its program exited within [`EARLY_EXIT`] of
/// starting and its session's title: `session <title> exited at start`
/// for the first that did (`session exited at start` with no title), else
/// `error` as it was.
fn pane_error_after<I>(error: Option<String>, dropped: I) -> Option<String>
where
    I: IntoIterator<Item = (bool, Option<String>)>,
{
    match dropped.into_iter().find(|(early, _)| *early) {
        Some((_, Some(title))) => Some(format!("session {title} exited at start")),
        Some((_, None)) => Some(EXITED_AT_START.to_owned()),
        None => error,
    }
}

/// Whether the projects screen is open, or under the open picker.
fn projects_open(state: &AppState) -> bool {
    state.picker.as_ref().is_some_and(|picker| {
        [Some(picker), picker.under()]
            .into_iter()
            .flatten()
            .any(|shown| *shown.kind() == PickerKind::ManageProjects)
    })
}

/// Shows a worker's `outcome` in `state` and the message box at `now`: an
/// error or notice pops, a success hides the error, changed worktrees
/// re-list an open worktree picker, and search rows and previews fill the
/// search picker unless they're stale (see [`PickerState::show_hits`] and
/// [`PickerState::show_search_preview`]). Then an open projects screen
/// re-lists, so it keeps its recency order as sessions get busier.
///
/// [`PickerState::show_hits`]: orb_domain::feat::picker::state::PickerState::show_hits
/// [`PickerState::show_search_preview`]: orb_domain::feat::picker::state::PickerState::show_search_preview
fn show(outcome: Outcome, state: &mut AppState, toasts: &mut Toasts, now: Instant) {
    match outcome {
        Outcome::Changed
        | Outcome::Ready { .. }
        | Outcome::Announce(_)
        | Outcome::ProjectsChanged => {}
        Outcome::Failed(error) => toasts.error(error, now),
        Outcome::Recovered => toasts.recovered(),
        Outcome::Notice(text) => toasts.notice(text, now),
        Outcome::WorktreesChanged => {
            let items = worktree_items(state);
            if let Some(picker) = &mut state.picker {
                picker.show_worktrees(items);
            }
        }
        Outcome::Hits {
            query,
            items,
            overflow,
        } => {
            if let Some(picker) = &mut state.picker {
                picker.show_hits(&query, items, overflow);
            }
        }
        Outcome::HitPreview { hit, messages } => {
            if let Some(picker) = &mut state.picker {
                picker.show_search_preview(hit, messages);
            }
        }
    }
    if projects_open(state) {
        IntentHandler::relist_projects(state);
    }
}

/// Handles a mouse event's intents in order after ending any pane mode;
/// `execute` runs each intent's commands before the next intent is handled.
fn handle_mouse_intents<F>(state: &State, intents: &[Intent], mut execute: F)
where
    F: FnMut(&Command),
{
    IntentHandler::handle(&Intent::ExitPaneMode, &mut state.write());
    for intent in intents {
        let commands = IntentHandler::handle(intent, &mut state.write());
        for command in &commands {
            execute(command);
        }
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
    /// once its layout no longer says, also for a broken pane.
    pane_sessions: HashMap<PaneId, ZmxSession>,
    /// The pane that last got a focus-in, so it gets a focus-out when the
    /// keys move on (see [`App::sync_focus`]).
    focused_pane: Option<PaneId>,
    /// Panes whose client exit the loop hasn't classified yet.
    exited: Vec<PaneId>,
    /// Panes already attached again once after their client died.
    respawned: HashSet<PaneId>,
    /// Panes whose output handling crashed; they get no client until their
    /// session leaves `attached`.
    broken: HashSet<PaneId>,
    /// Resume commands waiting for their fresh pane's first output.
    typing: HashMap<PaneId, String>,
    /// Shown on the start screen when a pane's client couldn't start.
    pane_error: Option<String>,
    /// The message box: the latest error or notice, until it times out.
    toasts: Toasts,
    /// The session a worker last made ready, until it opens or is dropped.
    ready: Option<Ready>,
    /// The environment panes run with.
    env: Vec<(OsString, OsString)>,
    /// Lists the zmx sessions panes attach to.
    zmx: ZmxService,
    /// The command a pane's client runs to attach to its zmx session.
    attach: fn(&ZmxSession) -> Vec<OsString>,
    /// Announces the sessions actor's status changes.
    notifier: NotifierService,
    /// orb's log file.
    log: Log,
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
    /// The thread the session picker's preview was last read for, and when.
    last_preview: Option<(ThreadId, Instant)>,
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
        attach: fn(&ZmxSession) -> Vec<OsString>,
        notifier: NotifierService,
        log: Log,
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
            broken: HashSet::new(),
            typing: HashMap::new(),
            pane_error: None,
            toasts: Toasts::default(),
            ready: None,
            env,
            zmx,
            attach,
            notifier,
            log,
            tz,
            tx,
            pane_area: Rect::default(),
            cursor_style: SetCursorStyle::DefaultUserShape,
            sidebar_scroll: SidebarScroll::default(),
            picker_scroll: PickerScroll::default(),
            hits: HitMap::default(),
            clicks: Clicks::default(),
            focused: true,
            last_preview: None,
        }
    }

    fn run(mut self, terminal: &mut DefaultTerminal, rx: &Receiver<LoopEvent>) -> io::Result<()> {
        spawn_input_thread(self.tx.clone())?;
        self.measure(terminal.size()?.into());
        let mut last_cursor = None;
        // The loop's own draw stores the layout this first frame returns.
        self.draw(terminal, &mut last_cursor)?;
        self.reattach_live();
        loop {
            self.measure(terminal.size()?.into());
            let placed = {
                let state = self.state.read();
                state
                    .shown_layout()
                    .map(|layout| tabs::placement(&state, layout, self.pane_area).panes)
                    .unwrap_or_default()
            };
            for place in &placed {
                if let Some(pane) = self.panes.get_mut(&place.pane) {
                    pane.resize(PaneSize::from(place.content()));
                }
            }
            let (sidebar_layout, picker_page) = self.draw(terminal, &mut last_cursor)?;
            // Navigation scrolls by what was just drawn.
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
            self.toasts.begin_turn();
            for event in first.into_iter().chain(rx.try_iter()) {
                self.handle(event, terminal.backend_mut())?;
            }
            self.refresh_preview();
            self.reconcile();
            self.open_ready();
            self.take_error();
            self.sync_focus();
            let now = Instant::now();
            for pane in self.panes.values() {
                pane.flush_expired_sync(now);
            }
        }
    }

    /// When the loop must wake without an event: a pane's synchronized
    /// update times out, a spinner frame passes while a thread is working
    /// or a session is starting, so the spinners and elapsed time tick, or
    /// the message box runs out, so it hides, or the session picker's preview
    /// waits to be read again, so a transcript that stopped growing still
    /// catches up.
    fn deadline(&self) -> Option<Instant> {
        let tick = self
            .state
            .read()
            .sessions
            .spinning()
            .then(|| Instant::now() + SPINNER_FRAME);
        let sync = self.panes.values().filter_map(Pane::sync_deadline);
        let toast = self.toasts.deadline(Instant::now());
        let preview = stale_preview(&self.state.read())
            .map(|(thread, _)| preview_due(self.last_preview, thread, Instant::now()));
        tick.into_iter()
            .chain(sync)
            .chain(toast)
            .chain(preview)
            .min()
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
                    self.toasts.copied(now);
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

    /// Runs a mouse event's intents, ending any key sequence in progress and
    /// any pane mode.
    fn run_mouse_intents(&mut self, intents: &[Intent]) {
        if intents.is_empty() {
            return;
        }
        self.keys.dismiss();
        let state = self.state.clone();
        handle_mouse_intents(&state, intents, |command| self.execute(command));
        self.toasts.dismiss();
    }

    fn handle<W>(&mut self, event: LoopEvent, out: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        match event {
            LoopEvent::Input(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                let (focus, mode, list_intent) = {
                    let state = self.state.read();
                    let list_intent = state
                        .picker
                        .as_ref()
                        .filter(|picker| picker.listing())
                        .map(|picker| keymap::picker_list_route(picker.kind(), key));
                    (state.focus, state.pane_mode(), list_intent)
                };
                let intent = match (focus, mode) {
                    // A list-mode picker takes letters as commands until `/`.
                    (Focus::Picker, _) if list_intent.is_some() => list_intent.flatten(),
                    // The rename box and the search take the picker's keys.
                    (Focus::Picker | Focus::Rename | Focus::Search, _) => keymap::picker_route(key),
                    (Focus::Pane, Some(mode)) => {
                        // A pane mode takes every key; none reaches the pane's program.
                        self.keys.dismiss();
                        keymap::mode_route(mode, key)
                    }
                    (Focus::Sidebar | Focus::Pane, _) => {
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
                    self.toasts.dismiss();
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
            LoopEvent::Input(_) => {}
            LoopEvent::Pane(id, PaneEvent::Broken) => self.drop_broken(id),
            LoopEvent::Reported(outcome) => self.apply(outcome),
            LoopEvent::Quit => {
                IntentHandler::handle(&Intent::Quit, &mut self.state.write());
            }
            LoopEvent::Pane(id, PaneEvent::Exited) => self.exited.push(id),
            LoopEvent::InputFailed(error) => return Err(error),
            LoopEvent::Pane(_, PaneEvent::Clipboard(text)) => {
                outer_terminal::copy_to_clipboard(out, &text)?;
            }
        }
        Ok(())
    }

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
                    {
                        let mut app = self.state.write();
                        app.focus = Focus::Sidebar;
                        app.attached.remove(session);
                        // One start at a time, as for a new session.
                        if validate_new_session(&app).is_err() {
                            return;
                        }
                        app.sessions.starting = true;
                    }
                    let _ = self
                        .sessions
                        .tell(SessionsCommand::RestoreWorktree(*session))
                        .try_send();
                    return;
                }
                self.pane_error = None;
                self.reconcile();
                self.sync_focus();
            }
            Command::Detach => self.sync_focus(),
            Command::SendCtrlG => {
                if let Some(pane) = self.attached_pane() {
                    pane.key(&KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
                }
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
            Command::ListDirectories(dir) => {
                let names = list_directories(dir);
                if let Some(picker) = &mut self.state.write().picker {
                    picker.show_directories(dir, names);
                }
            }
            Command::LoadPreview { thread, transcript } => {
                self.load_preview(*thread, transcript);
            }
            Command::Sessions(command) => {
                let _ = self.sessions.tell(command.clone()).try_send();
            }
            Command::Worktrees(command) => {
                let _ = self.worktrees.tell(command.clone()).try_send();
            }
            Command::Search(command) => {
                let _ = self.search.tell(command.clone()).try_send();
            }
        }
    }

    /// Drops broken pane `id`'s client (its program keeps running in zmx),
    /// keeps it from getting another while its session stays attached, and
    /// says so in the message box and the log. It keeps its zmx session so
    /// closing it in its layout still ends its program.
    fn drop_broken(&mut self, id: PaneId) {
        if self.panes.remove(&id).is_none() {
            return;
        }
        self.respawned.remove(&id);
        self.typing.remove(&id);
        self.broken.insert(id);
        let text = {
            let mut app = self.state.write();
            let app = &mut *app;
            let text = broken_pane_message(app.layouts.agent_name(id, app.sessions.agent_in(id)));
            app.sessions.error = Some(text.clone());
            text
        };
        self.log.line("error", &text);
    }

    /// Classifies each pane whose client exit arrived (see [`classify_exit`]):
    /// attaches a dead client again once, and on a second death detaches its
    /// session, keeping the pane and its program, and puts `couldn't reattach
    /// to zmx session <name>` on the mode line. A pane whose session isn't
    /// shown any more isn't listed or attached again; reconcile drops its
    /// client and keeps the pane and its program. Returns the panes to close.
    /// An exit from a pane already replaced or dropped is ignored.
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
            let shown = {
                let app = self.state.read();
                app.layouts
                    .owner_of(id)
                    .is_some_and(|owner| app.attached.contains(&owner))
            };
            let listed = if shown {
                self.zmx
                    .list(&session.dir)
                    .inspect_err(|report| {
                        self.log.error(
                            &format!("couldn't list zmx sessions on {}", session.dir.display()),
                            report,
                        );
                    })
                    .ok()
                    .map(|entries| entries.iter().any(|entry| entry.name == session.name))
            } else {
                None
            };
            let exit = classify_exit(listed, self.respawned.contains(&id), shown);
            let respawned = match exit {
                PaneExit::Respawn => self.spawn_pane(id, &session, &cwd),
                PaneExit::Ended | PaneExit::Failed | PaneExit::Drop => None,
            };
            match (exit_outcome(exit, respawned.is_some()), respawned) {
                (ExitOutcome::Reattached, Some(pane)) => {
                    if self.focused_pane == Some(id) {
                        pane.focus(true);
                    }
                    self.panes.insert(id, pane);
                    self.pane_sessions.insert(id, session.clone());
                    self.respawned.insert(id);
                }
                (ExitOutcome::Closed, _) => {
                    ended.insert(id);
                }
                (ExitOutcome::Dropped, _) => {}
                (ExitOutcome::Detached | ExitOutcome::Reattached, _) => {
                    let mut app = self.state.write();
                    if let Some(owner) = app.layouts.owner_of(id) {
                        app.attached.remove(&owner);
                    }
                    app.sessions.error =
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
    /// any more that didn't end is killed with `zmx kill`; a broken pane no
    /// layout holds any more is killed too. A pane that ended
    /// within [`EARLY_EXIT`] of starting leaves `session <title> exited at
    /// start` on the start screen, and that line in the log. Every pane of an attached session gets a client;
    /// when one can't start, its session leaves `attached` and `couldn't
    /// start zmx attach` shows on the start screen. If the keys were in a
    /// pane and no layout is shown any more, they go to the sidebar.
    fn reconcile(&mut self) {
        let ended = self.classify_exits();
        let (keep, all, edited, titles) = {
            let mut app = self.state.write();
            let app = &mut *app;
            let titles: HashMap<PaneId, String> = self
                .panes
                .iter()
                .filter(|(_, pane)| pane.exited_early(EARLY_EXIT))
                .filter_map(|(id, _)| {
                    app.layouts
                        .owner_of(*id)
                        .and_then(|owner| app.sessions.session(owner))
                        .map(|session| (*id, app.sessions.title(session)))
                })
                .collect();
            let edited = closed_in_layouts(&mut app.layouts, &app.attached, &ended);
            app.attached
                .retain(|owner| app.layouts.get(*owner).is_some());
            let keep: HashSet<PaneId> = app
                .attached
                .iter()
                .flat_map(|owner| app.layouts.session_panes(*owner))
                .collect();
            (keep, app.layouts.pane_ids(), edited, titles)
        };
        for owner in edited {
            self.execute(&Command::Sessions(SessionsCommand::SaveLayout(owner)));
        }
        let gone = to_drop(self.panes.keys().map(|id| (*id, ended.contains(id))), &keep);
        let early_exit = pane_error_after(
            None,
            gone.iter().filter_map(|id| {
                self.panes
                    .get(id)
                    .map(|pane| (pane.exited_early(EARLY_EXIT), titles.get(id).cloned()))
            }),
        );
        if let Some(text) = &early_exit {
            self.log.line("error", text);
        }
        self.pane_error = early_exit.or_else(|| self.pane_error.take());
        for id in to_kill(&gone, &ended, &all) {
            if let Some(session) = self.pane_sessions.get(&id) {
                self.zmx.kill(session, "closed in its layout");
            }
        }
        for id in &gone {
            self.panes.remove(id);
            self.pane_sessions.remove(id);
            self.respawned.remove(id);
            self.typing.remove(id);
        }
        let unbroken: Vec<PaneId> = self
            .broken
            .iter()
            .copied()
            .filter(|id| !keep.contains(id))
            .collect();
        for id in to_kill(&unbroken, &HashSet::new(), &all) {
            if let Some(session) = self.pane_sessions.get(&id) {
                self.zmx.kill(session, "closed in its layout");
            }
        }
        for id in &unbroken {
            self.pane_sessions.remove(id);
        }
        clear_detached(&mut self.broken, &keep);
        let spawning = to_spawn(&keep, &self.panes, &self.broken);
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
                    .inspect_err(|report| {
                        self.log.error(
                            &format!("couldn't list zmx sessions on {}", dir.display()),
                            report,
                        );
                    })
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

    /// Applies a worker's `outcome`: a ready session waits to open (see
    /// [`Self::open_ready`]), a status change is announced, and every
    /// outcome shows on the screen (see [`show`]).
    fn apply(&mut self, outcome: Outcome) {
        match &outcome {
            Outcome::Ready { session, if_active } => {
                self.ready = Some(Ready {
                    session: *session,
                    if_active: *if_active,
                    waited: false,
                });
            }
            Outcome::Announce(notice) => self.announce(notice.clone()),
            _ => {}
        }
        show(
            outcome,
            &mut self.state.write(),
            &mut self.toasts,
            Instant::now(),
        );
    }

    /// Opens the ready session as [`started_attach`] decides: at once, or,
    /// if the user was typing or in a pane when it came up, once the keys
    /// are back in the sidebar, leaving them there with the pane drawn (as a
    /// click on the sidebar does). One the user moved on from is dropped.
    fn open_ready(&mut self) {
        let Some(ready) = self.ready.take() else {
            return;
        };
        let (commands, keep_keys) = {
            let mut state = self.state.write();
            let decision = started_attach(
                ready.if_active,
                state.sessions.active(),
                state.focus,
                ready.waited,
            );
            let keep_keys = match decision {
                StartedAttach::Drop => return,
                StartedAttach::Wait => {
                    self.ready = Some(Ready {
                        waited: true,
                        ..ready
                    });
                    return;
                }
                StartedAttach::Attach { keep_keys } => keep_keys,
            };
            let focus = state.focus;
            let commands = IntentHandler::open_ready(&mut state, ready.session);
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

    /// Moves the refusal the last intents or the loop's own work left in
    /// the state into the message box.
    fn take_error(&mut self) {
        if self.state.read().sessions.error.is_none() {
            return;
        }
        if let Some(error) = self.state.write().sessions.error.take() {
            self.toasts.error(error, Instant::now());
        }
    }

    /// Reads `thread`'s transcript, in its harness's format, into the open
    /// picker's preview. An unreadable transcript, or one whose harness orb
    /// doesn't know, gives an empty preview, which shows as no transcript.
    fn load_preview(&mut self, thread: ThreadId, transcript: &Path) {
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
        self.last_preview = Some((thread, Instant::now()));
    }

    /// Reads the session picker's preview again when the selected thread's
    /// transcript isn't the length it was read at: it grew while the thread
    /// works, or it was found or replaced (`/clear`) since. The same thread
    /// is read at most once every [`PREVIEW_EVERY`]; the loop's deadline
    /// wakes it for the next read.
    fn refresh_preview(&mut self) {
        let now = Instant::now();
        let stale = stale_preview(&self.state.read());
        if let Some((thread, transcript)) = stale
            && preview_due(self.last_preview, thread, now) <= now
        {
            self.load_preview(thread, &transcript);
        }
    }

    /// While orb's terminal isn't focused, sends `notice` on a thread of
    /// its own (the desktop's notification server can be slow to answer);
    /// while it is, the sidebar already shows it and it's dropped. A notice
    /// that can't be sent is dropped.
    fn announce(&self, notice: Notice) {
        if self.focused {
            return;
        }
        let notifier = self.notifier.clone();
        let _ = thread::Builder::new()
            .name("orb-notice".into())
            .spawn(move || {
                let _ = notifier.announce(&notice);
            });
    }

    /// Kills every `orb-p*` session on orb's pane dir that no pane names
    /// (see [`stale_sessions`]), then attaches every unsettled session whose
    /// directory is there or that has a pane zmx still runs (see
    /// [`attach_at_start`]) and gives its panes their clients, so programs
    /// left running when orb quit show again and panes whose zmx session died
    /// while orb was down come back with their agents resumed (see
    /// [`resume_plan`]). Each socket dir the panes use is listed once; a dir
    /// zmx can't list brings no running program back. orb calls this after
    /// its first frame, so the screen shows while panes reconnect.
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
            .inspect_err(|report| {
                self.log.error(
                    &format!("couldn't list zmx sessions on {}", self.zmx.dir().display()),
                    report,
                );
            })
            .unwrap_or_default()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        let partial_load = self.state.read().sessions.partial_load;
        for name in stale_sessions(listed, &owned, partial_load) {
            self.zmx
                .kill(&self.zmx.session(name), "no pane names it at start");
        }
        let panes = reattachable(&self.state.read());
        let running: HashSet<ZmxSession> = {
            let dirs: HashSet<&Path> = panes.iter().map(|(_, zmx)| zmx.dir.as_path()).collect();
            dirs.into_iter()
                .flat_map(|dir| {
                    self.zmx
                        .list(dir)
                        .inspect_err(|report| {
                            self.log.error(
                                &format!("couldn't list zmx sessions on {}", dir.display()),
                                report,
                            );
                        })
                        .unwrap_or_default()
                        .into_iter()
                        .map(move |entry| ZmxSession {
                            name: entry.name,
                            dir: dir.to_owned(),
                        })
                })
                .collect()
        };
        let present = present_at_start(&self.state.read(), &panes);
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

    /// Runs the attach command on `session` for pane `id`, in `cwd`, which starts
    /// the user's shell when zmx makes the session,
    /// sized to the inside of its frame in the shown layout (else to the
    /// inside of a frame over the whole pane area),
    /// with orb's child environment and the pane's `ORB_PANE_ID`; `None` if
    /// it can't start.
    fn spawn_pane(&self, id: PaneId, session: &ZmxSession, cwd: &Path) -> Option<Pane> {
        let tx = self.tx.clone();
        let command = PaneCommand {
            argv: (self.attach)(session),
            cwd: cwd.to_owned(),
            env: pane_env(&self.env, id),
        };
        let area = {
            let state = self.state.read();
            state
                .shown_layout()
                .and_then(|layout| {
                    tabs::placement(&state, layout, self.pane_area)
                        .panes
                        .into_iter()
                        .find(|place| place.pane == id)
                })
                .map_or(self.pane_area.inner(Margin::new(1, 1)), |place| {
                    place.content()
                })
        };
        Pane::spawn(&command, PaneSize::from(area), move |event| {
            let _ = tx.send(LoopEvent::Pane(id, event));
        })
        .inspect_err(|report| {
            self.log.error(
                &format!("couldn't start zmx attach for {}", session.name),
                report,
            );
        })
        .ok()
    }

    /// Draws one frame. The terminal shows only the finished frame, and a
    /// hidden cursor goes back to `last_cursor`, where it last showed, so it
    /// never visibly moves. Returns the sidebar's layout and the picker's page.
    fn draw(
        &mut self,
        terminal: &mut DefaultTerminal,
        last_cursor: &mut Option<Position>,
    ) -> io::Result<(Option<SidebarLayout>, Option<usize>)> {
        let now = SystemTime::now();
        let mut drawn = (None, None, None);
        self.sidebar_scroll
            .follow_keys(self.state.read().focus == Focus::Sidebar);
        ratatui::crossterm::queue!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
        terminal.draw(|frame| {
            let state = self.state.read();
            drawn = render::render(
                frame,
                &state,
                &self.panes,
                self.pane_error.as_deref(),
                self.toasts.shown(Instant::now()),
                &self.keys,
                now,
                &self.tz,
                &mut self.sidebar_scroll,
                &mut self.picker_scroll,
                &mut self.hits,
            );
        })?;
        let (sidebar_layout, picker_page, cursor) = drawn;
        match (cursor, *last_cursor) {
            (Some(_), _) => *last_cursor = cursor,
            (None, Some(parked)) => terminal.set_cursor_position(parked)?,
            (None, None) => {}
        }
        ratatui::crossterm::execute!(terminal.backend_mut(), EndSynchronizedUpdate)?;
        Ok((sidebar_layout, picker_page))
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

/// When the preview of `thread` may be read: at once for a thread other than
/// the one `last` read, else [`PREVIEW_EVERY`] after that read.
fn preview_due(last: Option<(ThreadId, Instant)>, thread: ThreadId, now: Instant) -> Instant {
    match last {
        Some((read, at)) if read == thread => at + PREVIEW_EVERY,
        _ => now,
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
    use std::path::{Path, PathBuf};
    use std::time::UNIX_EPOCH;

    use orb_domain::feat::layout::state::{Layouts, PaneEntry, SessionLayout};
    use orb_domain::feat::layout::tree::Split;
    use orb_domain::feat::picker::list::PickerItem;
    use orb_domain::feat::picker::state::{
        PickerState, SessionFilter, session_items, worktree_items,
    };
    use orb_domain::feat::sessions::state::{
        PaneId, PaneLaunch, Project, ProjectId, ProjectKind, Session, SessionId, SessionKind,
        Sessions, Thread, ThreadId, ThreadStatus,
    };
    use std::ffi::OsString;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver};
    use std::time::{Duration, Instant};

    use jiff::tz::TimeZone;
    use orb_domain::feat::git::git_service::GitService;
    use orb_domain::feat::harness::Harnesses;
    use orb_domain::feat::notify::none::NoneNotifier;
    use orb_domain::feat::notify::notifier::NotifierService;
    use orb_domain::feat::search::search_actor::{SearchActorDeps, spawn_search_actor};
    use orb_domain::feat::sessions::sessions_actor::{SessionsActorDeps, spawn_sessions_actor};
    use orb_domain::feat::sessions::store::Store;
    use orb_domain::feat::sessions::transcript::Role;
    use orb_domain::feat::worktrees::state::Worktree;
    use orb_domain::feat::worktrees::worktrees_actor::{WorktreesActorDeps, spawn_worktrees_actor};
    use orb_domain::feat::zmx::zmx_service::{ZmxService, ZmxSession};
    use orb_domain::{
        AppState, Command, Focus, Intent, IntentHandler, Log, Outcome, PaneMode, Reporter,
        Services, SessionsCommand, State,
    };
    use orb_term::PaneEvent;
    use ratatui::crossterm::cursor::SetCursorStyle;
    use ratatui::layout::Rect;
    use tempfile::TempDir;
    use tokio::runtime::Runtime;

    use crate::test_support::{
        FakeZmx, NoGit, layouts_for, pane, session as session_id, sessions_for,
    };

    use super::{
        App, AttachPlan, ExitOutcome, LoopEvent, PREVIEW_EVERY, PaneExit, StartedAttach, Toasts,
        after_pane, attach_at_start, attach_or_restore, broken_pane_message, classify_exit,
        clear_detached, closed_in_layouts, cursor_style, exit_outcome, handle_mouse_intents,
        list_directories, pane_error_after, present_at_start, preview_due, reattachable,
        resume_plan, show, stale_preview, stale_sessions, started_attach, to_drop, to_kill,
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
    #[case::reattached(PaneExit::Respawn, true, ExitOutcome::Reattached)]
    #[case::respawn_failed_to_attach(PaneExit::Respawn, false, ExitOutcome::Detached)]
    #[case::failed(PaneExit::Failed, false, ExitOutcome::Detached)]
    #[case::ended(PaneExit::Ended, false, ExitOutcome::Closed)]
    #[case::dropped(PaneExit::Drop, false, ExitOutcome::Dropped)]
    fn exit_outcome_after_a_classified_exit(
        #[case] exit: PaneExit,
        #[case] reattached: bool,
        #[case] expected: ExitOutcome,
    ) {
        assert_eq!(
            exit_outcome(exit, reattached),
            expected,
            "{exit:?} with a new client {reattached}"
        );
    }

    /// Unsettled session 1 in `dir`, owning pane 1, as start-up sees them.
    fn session_in(dir: PathBuf) -> (AppState, Vec<(SessionId, ZmxSession)>) {
        let mut state = AppState::default();
        state.sessions.sessions = vec![Session {
            dir,
            ..session(1, false)
        }];
        (state, vec![(SessionId(1), entry(1).zmx)])
    }

    #[rstest::rstest]
    fn start_counts_a_session_whose_directory_is_there() -> io::Result<()> {
        // Given session 1 whose directory is on disk.
        let dir = tempfile::tempdir()?;
        let (state, panes) = session_in(dir.path().to_owned());

        // When start-up checks which directories are there.
        let present = present_at_start(&state, &panes);

        // Then session 1 counts.
        assert_eq!(
            present,
            HashSet::from([SessionId(1)]),
            "a session whose directory is there counts as present"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn start_leaves_out_a_session_whose_directory_is_gone() -> io::Result<()> {
        // Given session 1 whose directory is gone.
        let dir = tempfile::tempdir()?;
        let (state, panes) = session_in(dir.path().join("gone"));

        // When start-up checks which directories are there.
        let present = present_at_start(&state, &panes);

        // Then session 1 is left out.
        assert!(
            present.is_empty(),
            "a session whose directory is gone isn't present"
        );
        Ok(())
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
    fn preview_of_a_newly_selected_thread_is_due_at_once() {
        // Given thread 1's preview was just read.
        let now = Instant::now();
        let last = Some((ThreadId(1), now));

        // When asking when thread 2's preview is due.
        let due = preview_due(last, ThreadId(2), now);

        // Then it is due at once.
        assert_eq!(due, now, "another thread's preview should be due at once");
    }

    #[rstest::rstest]
    fn preview_of_the_same_thread_waits_a_second_after_its_last_read() {
        // Given thread 1's preview was read at `read_at`.
        let read_at = Instant::now();
        let last = Some((ThreadId(1), read_at));

        // When asking 300 ms later when thread 1's preview is due.
        let asked = read_at + Duration::from_millis(300);
        let due = preview_due(last, ThreadId(1), asked);

        // Then it is due a second after the last read, later than the ask.
        assert!(
            due == read_at + PREVIEW_EVERY && due > asked,
            "the same thread should wait a second after its last read, due {due:?}"
        );
    }

    #[rstest::rstest]
    fn preview_of_the_same_thread_is_due_once_a_second_has_passed() {
        // Given thread 1's preview was read at `read_at`.
        let read_at = Instant::now();
        let last = Some((ThreadId(1), read_at));

        // When asking a second later when thread 1's preview is due.
        let asked = read_at + PREVIEW_EVERY;
        let due = preview_due(last, ThreadId(1), asked);

        // Then it is due by then.
        assert!(
            due <= asked,
            "the same thread should be due once a second has passed"
        );
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
    fn ready_session_opens_while_the_same_session_is_active() {
        // Given session 1 restored while it was active, the cursor having
        // moved to another row meanwhile, and it still active.
        let active = Some(SessionId(1));

        // When deciding what to do in the sidebar.
        let decision = started_attach(active, active, Focus::Sidebar, false);

        // Then orb opens it, taking the keys into the pane.
        assert_eq!(
            decision,
            StartedAttach::Attach { keep_keys: false },
            "glancing around the sidebar shouldn't cancel the open"
        );
    }

    #[rstest::rstest]
    fn ready_session_is_dropped_after_another_session_opened() {
        // Given a session asked for while session 1 was active, and session 2
        // opened since.

        // When deciding what to do.
        let decision = started_attach(
            Some(SessionId(1)),
            Some(SessionId(2)),
            Focus::Sidebar,
            false,
        );

        // Then it is dropped.
        assert_eq!(
            decision,
            StartedAttach::Drop,
            "opening another session meanwhile cancels the open"
        );
    }

    #[rstest::rstest]
    fn new_session_opens_when_nothing_was_active_and_still_isnt() {
        // Given a new session asked for with nothing active, and still nothing.

        // When deciding what to do.
        let decision = started_attach(None, None, Focus::Sidebar, false);

        // Then it opens.
        assert_eq!(
            decision,
            StartedAttach::Attach { keep_keys: false },
            "a new session opens when the user opened nothing else"
        );
    }

    #[rstest::rstest]
    #[case(Focus::Picker)]
    #[case(Focus::Rename)]
    #[case(Focus::Search)]
    #[case(Focus::Pane)]
    fn ready_session_waits_while_typing_or_in_a_pane(#[case] focus: Focus) {
        // Given session 1 ready while it is still active, e.g. while a picker
        // has the keys.
        let active = Some(SessionId(1));

        // When deciding what to do in `focus`.
        let decision = started_attach(active, active, focus, false);

        // Then it waits instead of being dropped.
        assert_eq!(
            decision,
            StartedAttach::Wait,
            "the open should wait in {focus:?}"
        );
    }

    #[rstest::rstest]
    fn waited_ready_session_opens_once_the_keys_are_back() {
        // Given session 1 waited while the user was in a picker, and it is
        // still active.
        let active = Some(SessionId(1));

        // When deciding what to do once the keys are back in the sidebar.
        let decision = started_attach(active, active, Focus::Sidebar, true);

        // Then orb opens it and leaves the keys there.
        assert_eq!(
            decision,
            StartedAttach::Attach { keep_keys: true },
            "a waited open should keep the keys where they came back to"
        );
    }

    /// Shows `outcome` in `state` with an empty message box, now.
    fn shown(outcome: Outcome, state: &mut AppState) {
        show(outcome, state, &mut Toasts::default(), Instant::now());
    }

    /// The search picker with "ab" typed, listing nothing yet.
    fn search_typed_ab() -> AppState {
        let mut picker = PickerState::search(Focus::Sidebar);
        let _ = picker.insert('a');
        let _ = picker.insert('b');
        AppState {
            picker: Some(picker),
            ..AppState::default()
        }
    }

    /// A hit row in thread 1.
    fn hit(id: i64) -> PickerItem {
        PickerItem::Hit {
            id,
            thread: ThreadId(1),
            label: "orb/fix".to_owned(),
            split: 4,
            snippet: "the parser".to_owned(),
            lit: vec![],
            text_lit: vec![],
            path: PathBuf::from("/t/1.jsonl"),
            prompt_offset: 0,
        }
    }

    /// How many rows the open picker lists.
    fn listed(state: &AppState) -> Option<usize> {
        state.picker.as_ref().map(|picker| picker.shown().count())
    }

    #[rstest::rstest]
    fn stale_search_rows_are_dropped() {
        // Given the search picker with "ab" typed.
        let mut state = search_typed_ab();

        // When rows for the older query "a" arrive.
        shown(
            Outcome::Hits {
                query: "a".to_owned(),
                items: vec![hit(1)],
                overflow: false,
            },
            &mut state,
        );

        // Then the picker lists nothing.
        assert_eq!(
            listed(&state),
            Some(0),
            "rows for a query no longer typed shouldn't show"
        );
    }

    #[rstest::rstest]
    fn current_search_rows_are_shown() {
        // Given the search picker with "ab" typed.
        let mut state = search_typed_ab();

        // When rows for "ab" arrive.
        shown(
            Outcome::Hits {
                query: "ab".to_owned(),
                items: vec![hit(1)],
                overflow: false,
            },
            &mut state,
        );

        // Then the picker lists them.
        assert_eq!(
            listed(&state),
            Some(1),
            "rows for the typed query should show"
        );
    }

    /// The search picker listing hits 1 and 2, with hit 1 selected.
    fn search_listing_two_hits() -> AppState {
        let mut picker = PickerState::search(Focus::Sidebar);
        picker.show_hits("", vec![hit(1), hit(2)], false);
        AppState {
            picker: Some(picker),
            ..AppState::default()
        }
    }

    /// A one-message exchange.
    fn exchange() -> Vec<(i64, Role, String)> {
        vec![(1, Role::User, "fix the parser".to_owned())]
    }

    /// The hit the open picker previews.
    fn previewed(state: &AppState) -> Option<i64> {
        state
            .picker
            .as_ref()
            .and_then(PickerState::search_preview)
            .map(|preview| preview.hit)
    }

    #[rstest::rstest]
    fn stale_search_preview_is_dropped() {
        // Given the search picker with hit 1 selected.
        let mut state = search_listing_two_hits();

        // When the preview of hit 2, selected earlier, arrives.
        shown(
            Outcome::HitPreview {
                hit: 2,
                messages: exchange(),
            },
            &mut state,
        );

        // Then nothing is previewed.
        assert_eq!(
            previewed(&state),
            None,
            "a preview for a row no longer selected shouldn't show"
        );
    }

    #[rstest::rstest]
    fn current_search_preview_is_shown() {
        // Given the search picker with hit 1 selected.
        let mut state = search_listing_two_hits();

        // When the preview of hit 1 arrives.
        shown(
            Outcome::HitPreview {
                hit: 1,
                messages: exchange(),
            },
            &mut state,
        );

        // Then hit 1 is previewed.
        assert_eq!(
            previewed(&state),
            Some(1),
            "the selected row's preview should show"
        );
    }

    /// An unused, not yet read worktree at `path`.
    fn worktree_at(path: &str) -> Worktree {
        Worktree {
            path: PathBuf::from(path),
            repo: None,
            facts: None,
            size_kb: None,
        }
    }

    #[rstest::rstest]
    fn worktrees_changed_relists_the_open_worktree_picker() {
        // Given the worktree picker over two worktrees, "orb" typed and the
        // second row selected, then a third worktree listed.
        let mut state = AppState::default();
        state.worktrees.list = vec![worktree_at("/w/orb/a1"), worktree_at("/w/orb/b2")];
        state.picker = Some({
            let mut picker = PickerState::worktrees(worktree_items(&state), Focus::Sidebar);
            for ch in ['o', 'r', 'b'] {
                let _ = picker.insert(ch);
            }
            picker.next();
            picker
        });
        state.worktrees.list.push(worktree_at("/w/orb/c3"));

        // When the worktrees actor reports the new list.
        shown(Outcome::WorktreesChanged, &mut state);

        // Then the picker lists all three, keeping the typed text and the
        // selected worktree.
        let picker = state.picker.as_ref();
        assert_eq!(
            (
                picker.map(|picker| picker.shown().count()),
                picker.map(PickerState::input),
                picker.and_then(PickerState::selected_worktree),
            ),
            (Some(3), Some("orb"), Some(Path::new("/w/orb/b2"))),
            "a rescan should re-list the open worktree picker as the user left it"
        );
    }

    /// Project `id` titled `title`, with one thread last active `secs`
    /// seconds after the epoch.
    fn project_active_at(id: i64, title: &str, secs: u64) -> Project {
        Project {
            id: ProjectId(id),
            title: title.to_owned(),
            root: PathBuf::from(format!("/{title}")),
            created_at: UNIX_EPOCH,
            threads: vec![Thread {
                last_session: None,
                harness: HarnessId::new("claude"),
                id: ThreadId(id),
                title: None,
                cwd: PathBuf::from(format!("/{title}")),
                transcript: None,
                status: ThreadStatus::Idle,
                turn_started_at: None,
                pane: None,
                branch: None,
                created_at: UNIX_EPOCH,
                last_activity_at: UNIX_EPOCH + Duration::from_secs(secs),
                unseen: false,
                model: None,
            }],
            repo: true,
            removed: false,
            kind: ProjectKind::Normal,
        }
    }

    /// Alpha (active at 20 s) and beta (at 10 s), with `opened` handled.
    fn alpha_and_beta_after(opened: &Intent) -> AppState {
        let mut state = AppState {
            sessions: Sessions {
                projects: vec![
                    project_active_at(1, "alpha", 20),
                    project_active_at(2, "beta", 10),
                ],
                ..Sessions::default()
            },
            ..AppState::default()
        };
        IntentHandler::handle(opened, &mut state);
        state
    }

    /// Beta's thread gets busier than alpha's.
    fn make_beta_busier(state: &mut AppState) {
        if let Some(beta) = state.sessions.projects.get_mut(1) {
            for thread in &mut beta.threads {
                thread.last_activity_at = UNIX_EPOCH + Duration::from_secs(30);
            }
        }
    }

    /// The project titles the open picker lists, in order.
    fn titles(state: &AppState) -> Vec<String> {
        state
            .picker
            .iter()
            .flat_map(PickerState::shown)
            .filter_map(|(item, _)| match item {
                PickerItem::Project { title, .. } => Some(title.clone()),
                _ => None,
            })
            .collect()
    }

    #[rstest::rstest]
    fn any_report_resorts_the_open_projects_screen() {
        // Given the projects screen listing alpha then beta, and beta now
        // the busier.
        let mut state = alpha_and_beta_after(&Intent::OpenProjects);
        make_beta_busier(&mut state);

        // When a plain change is reported.
        shown(Outcome::Changed, &mut state);

        // Then beta is listed first.
        assert_eq!(
            titles(&state),
            vec!["beta".to_owned(), "alpha".to_owned()],
            "the projects screen should keep its recency order live"
        );
    }

    #[rstest::rstest]
    fn report_leaves_projects_alone_when_the_screen_is_closed() {
        // Given the new-session project picker (not the projects screen)
        // listing alpha then beta, and beta now the busier.
        let mut state = alpha_and_beta_after(&Intent::NewSession);
        make_beta_busier(&mut state);

        // When a plain change is reported.
        shown(Outcome::Changed, &mut state);

        // Then the picker still lists alpha first.
        assert_eq!(
            titles(&state),
            vec!["alpha".to_owned(), "beta".to_owned()],
            "a report shouldn't re-list projects while the projects screen is closed"
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
        let spawned = to_spawn(&keep, &live, &HashSet::new());

        // Then pane 7 gets a client.
        assert_eq!(
            spawned,
            vec![PaneId(7)],
            "a kept pane without a client starts"
        );
    }

    #[rstest::rstest]
    fn broken_pane_is_not_reconnected() {
        // Given kept panes 7 and 8, neither with a client, pane 8 broken.
        let keep = HashSet::from([PaneId(7), PaneId(8)]);
        let live: HashMap<PaneId, &str> = HashMap::new();
        let broken = HashSet::from([PaneId(8)]);

        // When choosing the panes to start.
        let spawned = to_spawn(&keep, &live, &broken);

        // Then only pane 7 gets a client.
        assert_eq!(spawned, vec![PaneId(7)], "a broken pane gets no new client");
    }

    #[rstest::rstest]
    fn broken_mark_clears_when_its_session_detaches() {
        // Given broken pane 7 whose session is no longer attached.
        let mut broken = HashSet::from([PaneId(7)]);

        // When clearing marks against the kept panes.
        clear_detached(&mut broken, &HashSet::new());

        // Then pane 7 is no longer marked broken.
        assert!(broken.is_empty(), "a detached session's broken mark clears");
    }

    #[rstest::rstest]
    fn broken_pane_message_names_the_pane() {
        // Given a pane named claude.
        // When building the broken-pane message.
        let text = broken_pane_message("claude");

        // Then it names the pane and says it keeps running.
        assert_eq!(
            text, "stopped showing pane claude after an error; it keeps running",
            "the message names the broken pane"
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
        let stale = stale_sessions(listed, &HashSet::new(), false);

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
        let stale = stale_sessions(listed, &HashSet::from(["orb-p12".to_owned()]), false);

        // Then nothing is killed.
        assert!(stale.is_empty(), "a pane's own session stays");
    }

    #[rstest::rstest]
    fn stale_sessions_kills_nothing_after_a_partial_load() {
        // Given zmx listing an orb-p* name no loaded pane owns, after saved
        // layouts orb couldn't all read.
        let listed = vec!["orb-p7".to_owned()];

        // When choosing what to kill.
        let stale = stale_sessions(listed, &HashSet::new(), true);

        // Then nothing is killed.
        assert!(stale.is_empty(), "a pane orb couldn't read may own it");
    }

    #[rstest::rstest]
    fn stale_sessions_kills_nothing_when_zmx_lists_nothing() {
        // Given zmx listing nothing, as when its list fails at start.
        let listed = Vec::new();

        // When choosing what to kill.
        let stale = stale_sessions(listed, &HashSet::new(), false);

        // Then nothing is killed.
        assert!(stale.is_empty(), "a failed list kills nothing");
    }

    #[rstest::rstest]
    fn stale_sessions_spares_names_without_the_orb_p_prefix() {
        // Given zmx listing a session orb didn't name.
        let listed = vec!["work".to_owned()];

        // When choosing what to kill.
        let stale = stale_sessions(listed, &HashSet::new(), false);

        // Then it is left alone.
        assert!(stale.is_empty(), "only orb-p* sessions are orb's");
    }

    #[rstest::rstest]
    #[case::ended(Some(false), false, PaneExit::Ended)]
    #[case::ended_after_a_respawn(Some(false), true, PaneExit::Ended)]
    #[case::client_died(Some(true), false, PaneExit::Respawn)]
    #[case::client_died_again(Some(true), true, PaneExit::Failed)]
    #[case::zmx_unknown(None, false, PaneExit::Respawn)]
    #[case::zmx_unknown_again(None, true, PaneExit::Failed)]
    fn pane_exit_is_classified_by_zmx_listing_and_an_earlier_respawn(
        #[case] listed: Option<bool>,
        #[case] respawned: bool,
        #[case] expected: PaneExit,
    ) {
        // Given / When a pane of a shown session's client exits, zmx
        // `listed` its session or not, and the pane was `respawned` once or
        // not.
        let exit = classify_exit(listed, respawned, true);

        // Then the exit is `expected`.
        assert_eq!(exit, expected, "listed {listed:?}, respawned {respawned}");
    }

    #[rstest::rstest]
    #[case::ended(Some(false), false)]
    #[case::ended_after_a_respawn(Some(false), true)]
    #[case::client_died(Some(true), false)]
    #[case::client_died_again(Some(true), true)]
    #[case::zmx_unknown(None, false)]
    #[case::zmx_unknown_again(None, true)]
    fn pane_exit_of_a_session_not_shown_is_dropped(
        #[case] listed: Option<bool>,
        #[case] respawned: bool,
    ) {
        // Given / When a pane of a session that isn't shown has its client
        // exit, zmx `listed` its session or not, and the pane was
        // `respawned` once or not.
        let exit = classify_exit(listed, respawned, false);

        // Then only its client is dropped.
        assert_eq!(
            exit,
            PaneExit::Drop,
            "listed {listed:?}, respawned {respawned}"
        );
    }

    #[rstest::rstest]
    fn early_exit_names_the_session() {
        // Given no pane error.
        let error = None;

        // When one of the dropped panes, of session `orb-tui`, exited early.
        let error = pane_error_after(error, [(false, None), (true, Some("orb-tui".to_owned()))]);

        // Then the start screen names the session that exited at start.
        assert_eq!(
            error.as_deref(),
            Some("session orb-tui exited at start"),
            "an early exit names its session"
        );
    }

    #[rstest::rstest]
    fn early_exit_of_an_untitled_session_says_session_exited_at_start() {
        // Given no pane error.
        let error = None;

        // When a dropped pane with no session title exited early.
        let error = pane_error_after(error, [(true, None)]);

        // Then the start screen says the session exited at start.
        assert_eq!(
            error.as_deref(),
            Some("session exited at start"),
            "an untitled early exit keeps the plain text"
        );
    }

    #[rstest::rstest]
    fn later_exit_keeps_the_pane_error() {
        // Given an existing pane error.
        let error = Some("couldn't start the attach command".to_owned());

        // When the only dropped pane exited later.
        let error = pane_error_after(error, [(false, None)]);

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

    #[rstest::rstest]
    fn mouse_intents_end_the_pane_mode() {
        // Given move mode on in a pane.
        let state = State::new(AppState {
            focus: Focus::Pane,
            pane_mode: Some(PaneMode::Move),
            ..AppState::default()
        });

        // When a click runs its intents.
        handle_mouse_intents(&state, &[Intent::GoToTab(1)], |_| {});

        // Then no mode is on.
        assert_eq!(state.read().pane_mode(), None, "a click ends the mode");
    }

    /// An `App` over fakes, for loop tests: its actors run on their own
    /// runtime over an in-memory store and a state of their own, reporting
    /// on the loop's channel; panes attach with the given command in a temp
    /// dir; zmx is `zmx`.
    struct LoopTest {
        app: App,
        state: State,
        zmx: Arc<FakeZmx>,
        rx: Receiver<LoopEvent>,
        /// The panes' cwd, the sessions' dir and the actors' roots.
        dir: TempDir,
        /// Dropped after the app.
        runtime: Runtime,
    }

    fn loop_app(zmx: FakeZmx, attach: fn(&ZmxSession) -> Vec<OsString>) -> io::Result<LoopTest> {
        let dir = tempfile::tempdir()?;
        let runtime = Runtime::new()?;
        let store =
            Store::open_in_memory().map_err(|report| io::Error::other(report.to_string()))?;
        let git = GitService::new(Arc::new(NoGit), Log::default());
        let (tx, rx) = mpsc::channel();
        let (sessions, worktrees, search) = {
            let _context = runtime.enter();
            let report = {
                let tx = tx.clone();
                Reporter::new(move |outcome| {
                    let _ = tx.send(LoopEvent::Reported(outcome));
                })
            };
            let sessions = spawn_sessions_actor(SessionsActorDeps {
                services: Services {
                    harnesses: Harnesses::new(vec![]),
                    git: git.clone(),
                    zmx: ZmxService::new(
                        Arc::new(FakeZmx::listing("")),
                        dir.path().join("zmx"),
                        Log::default(),
                    ),
                    log: Log::default(),
                },
                state: State::default(),
                store,
                worktrees_root: dir.path().join("worktrees"),
                orb_root: dir.path().join("orb"),
                incognito_root: dir.path().join("incognito"),
                report: report.clone(),
                integration_missing: false,
            });
            let worktrees = spawn_worktrees_actor(WorktreesActorDeps {
                git: git.clone(),
                state: State::default(),
                worktrees_root: dir.path().join("worktrees"),
                report: report.clone(),
                sweep_every: Duration::from_secs(3600),
            });
            let search = spawn_search_actor(SearchActorDeps {
                state: State::default(),
                harnesses: Harnesses::new(vec![]),
                index_path: dir.path().join("search.sqlite"),
                report,
                log: Log::default(),
            });
            (sessions, worktrees, search)
        };
        runtime.block_on(sessions.wait_for_startup());
        let zmx = Arc::new(zmx);
        let state = State::default();
        let mut app = App::new(
            state.clone(),
            sessions,
            worktrees,
            dir.path().join("worktrees"),
            search,
            git,
            Harnesses::new(vec![]),
            Vec::new(),
            ZmxService::new(zmx.clone(), dir.path().join("zmx"), Log::default()),
            attach,
            NotifierService::new(Arc::new(NoneNotifier)),
            Log::default(),
            tx,
            TimeZone::UTC,
        );
        app.measure(Rect::new(0, 0, 120, 40));
        Ok(LoopTest {
            app,
            state,
            zmx,
            rx,
            dir,
            runtime,
        })
    }

    /// A client that exits at once.
    fn exits_at_once(_: &ZmxSession) -> Vec<OsString> {
        ["/bin/sh", "-c", "exit 0"].map(OsString::from).to_vec()
    }

    /// A client that keeps running.
    fn keeps_running(_: &ZmxSession) -> Vec<OsString> {
        ["/bin/sh", "-c", "/bin/sleep 30"]
            .map(OsString::from)
            .to_vec()
    }

    /// How many clients [`counted_exit_at_once`] started.
    static COUNTED_ATTACHES: AtomicUsize = AtomicUsize::new(0);

    /// A client that exits at once, counted in [`COUNTED_ATTACHES`].
    fn counted_exit_at_once(session: &ZmxSession) -> Vec<OsString> {
        COUNTED_ATTACHES.fetch_add(1, Ordering::SeqCst);
        exits_at_once(session)
    }

    impl LoopTest {
        /// Pane `id` in the test's dir, attaching to its own zmx session.
        fn entry(&self, id: PaneId) -> PaneEntry {
            PaneEntry {
                id,
                zmx: ZmxSession {
                    name: id.zmx_name(),
                    dir: "/zmx".into(),
                },
                cwd: self.dir.path().to_owned(),
                name: None,
                resume: None,
            }
        }

        /// Attached session 1 in the test's dir, holding `panes` side by
        /// side; each gets its client.
        fn attach(&mut self, panes: &[PaneId]) {
            {
                let mut state = self.state.write();
                state.sessions.sessions = vec![Session {
                    id: session_id(1),
                    dir: self.dir.path().to_owned(),
                    ..session(1, false)
                }];
                for (at, id) in panes.iter().enumerate() {
                    let entry = self.entry(*id);
                    match at {
                        0 => state
                            .layouts
                            .insert(session_id(1), SessionLayout::of(entry)),
                        _ => state.layouts.split(session_id(1), Split::Right, entry),
                    }
                }
                state.attached.insert(session_id(1));
            }
            self.app.reconcile();
        }

        /// Waits up to five seconds for pane `id`'s client to exit.
        fn exit_of(&self, id: PaneId) -> Option<LoopEvent> {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match self
                    .rx
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                {
                    Ok(LoopEvent::Pane(pane, PaneEvent::Exited)) if pane == id => {
                        return Some(LoopEvent::Pane(pane, PaneEvent::Exited));
                    }
                    Ok(_) => {}
                    Err(_) => return None,
                }
            }
        }
    }

    #[rstest::rstest]
    fn entering_a_gone_worktree_while_a_start_runs_asks_for_no_restore() -> io::Result<()> {
        // Given a session start in flight, and session 1 whose orb worktree
        // is gone.
        let mut test = loop_app(FakeZmx::listing(""), exits_at_once)?;
        {
            let mut state = test.state.write();
            state.sessions.starting = true;
            state.sessions.sessions = vec![Session {
                id: session_id(1),
                dir: test.dir.path().join("worktrees/orb/orb-1a2b3c4d"),
                ..session(1, false)
            }];
        }

        // When the loop attaches session 1, as `⏎` on its row asks, and the
        // sessions worker has handled what it was sent.
        test.app.execute(&Command::Attach(session_id(1)));
        let sessions = test.app.sessions.clone();
        test.runtime
            .block_on(async move { sessions.ask(SessionsCommand::SaveUi).await })
            .map_err(|error| io::Error::other(format!("{error:?}")))?;

        // Then the worker reports no failure: no second restore ran.
        let failures: Vec<String> = test
            .rx
            .try_iter()
            .filter_map(|event| match event {
                LoopEvent::Reported(Outcome::Failed(error)) => Some(error),
                _ => None,
            })
            .collect();
        assert_eq!(
            failures,
            Vec::<String>::new(),
            "a restore should wait for the start in flight"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn exited_client_with_unlistable_zmx_keeps_its_pane() -> io::Result<()> {
        // Given zmx that can't list, and pane 1 whose client exits at once.
        let mut test = loop_app(FakeZmx::failing("boom"), exits_at_once)?;
        test.attach(&[pane(1)]);

        // When its exit arrives and the loop goes round.
        let event = test
            .exit_of(pane(1))
            .ok_or_else(|| io::Error::other("pane 1's client didn't exit within 5 s"))?;
        test.app.handle(event, &mut io::sink())?;
        test.app.reconcile();

        // Then the pane is still in its layout.
        assert!(
            test.state.read().layouts.pane_ids().contains(&pane(1)),
            "an exit zmx can't explain keeps the pane"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn dead_pane_of_a_session_not_shown_is_not_attached_again() -> io::Result<()> {
        // Given zmx that can't list, pane 1 whose client exits at once, and
        // its session taken off the right side.
        let mut test = loop_app(FakeZmx::failing("boom"), counted_exit_at_once)?;
        test.attach(&[pane(1)]);
        test.state.write().attached.remove(&session_id(1));

        // When its exit arrives and the loop goes round.
        let event = test
            .exit_of(pane(1))
            .ok_or_else(|| io::Error::other("pane 1's client didn't exit within 5 s"))?;
        test.app.handle(event, &mut io::sink())?;
        test.app.reconcile();

        // Then no second client was started.
        assert_eq!(
            COUNTED_ATTACHES.load(Ordering::SeqCst),
            1,
            "a dead client of a session not shown is not attached again"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn pane_closed_in_its_layout_is_killed_once() -> io::Result<()> {
        // Given zmx listing nothing, and running panes 1 and 2.
        let mut test = loop_app(FakeZmx::listing(""), keeps_running)?;
        test.attach(&[pane(1), pane(2)]);

        // When pane 2 closes in its layout and the loop goes round twice.
        test.state.write().layouts.close_pane(pane(2));
        test.app.reconcile();
        test.app.reconcile();

        // Then its zmx session is killed once.
        assert_eq!(
            test.zmx.killed(),
            [pane(2).zmx_name()],
            "a closed pane's session is killed once"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn closed_broken_pane_is_killed() -> io::Result<()> {
        // Given running panes 1 and 2, and pane 2 stopped showing after an error.
        let mut test = loop_app(FakeZmx::listing(""), keeps_running)?;
        test.attach(&[pane(1), pane(2)]);
        test.app
            .handle(LoopEvent::Pane(pane(2), PaneEvent::Broken), &mut io::sink())?;

        // When pane 2 closes in its layout and the loop goes round.
        test.state.write().layouts.close_pane(pane(2));
        test.app.reconcile();

        // Then its zmx session is killed.
        assert_eq!(
            test.zmx.killed(),
            [pane(2).zmx_name()],
            "a closed broken pane's session is killed"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn a_quit_event_ends_the_loop() -> io::Result<()> {
        // Given a loop with nothing open.
        let mut test = loop_app(FakeZmx::listing(""), keeps_running)?;

        // When a quit event arrives, as a signal sends it.
        test.app.handle(LoopEvent::Quit, &mut io::sink())?;

        // Then the loop's quit flag is set, so it ends after its next draw.
        assert!(
            test.state.read().should_quit,
            "a quit event should end the loop"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn deadline_wakes_the_loop_for_a_pending_preview_read() -> io::Result<()> {
        // Given the session picker on thread 1, its preview just read, and
        // its transcript grown since.
        let mut test = loop_app(FakeZmx::listing(""), keeps_running)?;
        let transcript = test.dir.path().join("1.jsonl");
        fs::write(&transcript, "{}\n")?;
        {
            let mut state = test.state.write();
            let projects = vec![Project {
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
                    pane: Some(PaneLaunch {
                        pane: PaneId(1),
                        session: SessionId(1),
                    }),
                    branch: None,
                    created_at: UNIX_EPOCH,
                    last_activity_at: UNIX_EPOCH,
                    unseen: false,
                    model: None,
                }],
                repo: true,
                removed: false,
                kind: ProjectKind::Normal,
            }];
            state.sessions = Sessions {
                sessions: sessions_for(&projects),
                projects,
                ..Sessions::default()
            };
            state.picker = Some(PickerState::sessions(
                session_items(
                    &state.sessions,
                    &layouts_for(&state.sessions.projects),
                    SessionFilter::All,
                ),
                Focus::Sidebar,
            ));
        }
        test.app.load_preview(ThreadId(1), &transcript);
        let read_at = Instant::now();
        fs::write(&transcript, "{}\n{}\n")?;

        // When asking when the loop must wake.
        let deadline = test.app.deadline();

        // Then it wakes within a second of the read, for the next one.
        assert!(
            deadline.is_some_and(|due| read_at < due && due <= read_at + PREVIEW_EVERY),
            "a pending preview read should wake the loop within a second, got {deadline:?}"
        );
        Ok(())
    }
}
