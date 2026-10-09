//! Key routing: which keys do what in each focus. Only keys the user has
//! defined are bound.
//!
//! The Cmd keys (focus moves, split, close, grow and shrink, tab moves),
//! `<C-S-h>`/`<C-S-l>` (the keys to the sidebar and into the panes) and
//! `<C-[>`/`<C-]>` (back and forward through the jump list) come first, in
//! the sidebar and in panes, outside which-key: it has no Super modifier,
//! Ctrl+Shift+h must never match a plain `<C-h>`, and Ctrl+[ must never
//! match a plain Esc. `<C-g>` is the which-key leader in both; its popup
//! lists only the keys that do something for the selection (`<C-g> w`/`<C-g>
//! b` not on Incognito, Research and Learn sessions, `<C-g> p` and `<C-g>
//! <C-g>` only in a pane, `<C-g> t` only with a session selected and its
//! `b` only in a pane, `<C-g> f` only in the sidebar); `<C-g> e` hides or
//! shows the sidebar and `<C-g> q` quits. The sidebar's own keys (`j`/`k`, `gg`/`G`, `<C-d>`/`<C-u>`, `⏎`,
//! `/` and `i`, `l`/`h`, `r`/`p`/`s`/`d` on a session, `r`/`d` on an agent
//! row) go through the same keymap. In a pane every other key goes to its
//! program. An open picker, the rename box and
//! the sidebar search take typed characters and have their own fixed keys,
//! `<C-x>` among them for removing a project from the project filter and
//! `<C-s>` for showing or hiding settled sessions in the session picker.

use std::fmt;

use orb_domain::feat::layout::tree::{NavDirection, Split};
use orb_domain::feat::sessions::state::{FolderKind, SessionKind, Sessions};
use orb_domain::{AppState, Focus, Intent};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui_which_key::{Keymap, WhichKeyState};

/// The which-key popup's section for a binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyCategory {
    General,
    Navigation,
    Sessions,
    Threads,
}

impl fmt::Display for KeyCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::General => "general",
            Self::Navigation => "navigation",
            Self::Sessions => "sessions",
            Self::Threads => "threads",
        })
    }
}

/// What the sidebar cursor is on, which decides the keys that do something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Selection {
    /// A session in a project's checkout or worktree.
    Session,
    /// An Incognito, Research or Learn session, whose folder is orb's own.
    OwnFolderSession,
    /// An agent row of a session in a project's checkout or worktree.
    Agent,
    /// An agent row of an Incognito, Research or Learn session.
    OwnFolderAgent,
    /// No row, or the settled shelf's header.
    Nothing,
}

impl Selection {
    /// What `sessions`' cursor is on.
    pub(crate) fn of(sessions: &Sessions) -> Self {
        let agent = sessions.selected_agent().is_some();
        match (sessions.selected_session(), agent) {
            (Some(session), true) if session.kind != SessionKind::Plain => Self::OwnFolderAgent,
            (Some(_), true) => Self::Agent,
            (Some(session), false) if session.kind != SessionKind::Plain => Self::OwnFolderSession,
            (Some(_), false) => Self::Session,
            (None, _) => Self::Nothing,
        }
    }
}

/// Which bindings apply: the focused side's, less the keys that do nothing
/// for the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Scope {
    /// The sidebar on a session.
    Sidebar,
    /// The sidebar with no session selected.
    SidebarEmpty,
    /// The sidebar on an Incognito, Research or Learn session:
    /// [`Scope::Sidebar`]'s keys but `<C-g> w`/`<C-g> b`.
    SidebarIncognito,
    /// A pane of the shown session.
    Pane,
    /// A pane of an Incognito, Research or Learn session: [`Scope::Pane`]'s
    /// keys but `<C-g> w`/`<C-g> b`.
    PaneIncognito,
    /// The sidebar on an agent row: [`Scope::Sidebar`]'s keys but `p`/`s`.
    SidebarAgent,
    /// The sidebar on an agent row of an Incognito, Research or Learn
    /// session: [`Scope::SidebarAgent`]'s keys but `<C-g> w`/`<C-g> b`.
    SidebarAgentIncognito,
}

impl Scope {
    /// The scope for keys in `focus` with `selection`. A pane focus with
    /// nothing selected doesn't last (the keys leave once no layout is
    /// shown), so it reads as [`Scope::Pane`].
    pub(crate) fn new(focus: Focus, selection: Selection) -> Self {
        match (focus, selection) {
            (Focus::Pane, Selection::OwnFolderSession | Selection::OwnFolderAgent) => {
                Self::PaneIncognito
            }
            (Focus::Pane, Selection::Session | Selection::Agent | Selection::Nothing) => Self::Pane,
            (
                Focus::Sidebar | Focus::Picker | Focus::Rename | Focus::Search,
                Selection::Session,
            ) => Self::Sidebar,
            (
                Focus::Sidebar | Focus::Picker | Focus::Rename | Focus::Search,
                Selection::Nothing,
            ) => Self::SidebarEmpty,
            (
                Focus::Sidebar | Focus::Picker | Focus::Rename | Focus::Search,
                Selection::OwnFolderSession,
            ) => Self::SidebarIncognito,
            (Focus::Sidebar | Focus::Picker | Focus::Rename | Focus::Search, Selection::Agent) => {
                Self::SidebarAgent
            }
            (
                Focus::Sidebar | Focus::Picker | Focus::Rename | Focus::Search,
                Selection::OwnFolderAgent,
            ) => Self::SidebarAgentIncognito,
        }
    }

    /// The scope for keys in `focus` over `state`'s selection.
    pub(crate) fn of(focus: Focus, state: &AppState) -> Self {
        Self::new(focus, Selection::of(&state.sessions))
    }
}

/// The keymap with its current scope and pending key sequence.
pub(crate) type Keys = WhichKeyState<KeyEvent, Scope, Intent, KeyCategory>;

/// orb's which-key leader in the sidebar and in panes.
pub(crate) const LEADER: KeyEvent = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL);

/// The sidebar and pane bindings, scoped by focus and selection.
#[expect(
    clippy::too_many_lines,
    reason = "one binding per key keeps the whole keymap in one place"
)]
pub(crate) fn keymap() -> Keymap<KeyEvent, Scope, Intent, KeyCategory> {
    const SIDEBAR: [Scope; 5] = [
        Scope::Sidebar,
        Scope::SidebarEmpty,
        Scope::SidebarIncognito,
        Scope::SidebarAgent,
        Scope::SidebarAgentIncognito,
    ];
    const PANE: [Scope; 2] = [Scope::Pane, Scope::PaneIncognito];
    const SESSION: [Scope; 6] = [
        Scope::Sidebar,
        Scope::SidebarIncognito,
        Scope::SidebarAgent,
        Scope::SidebarAgentIncognito,
        Scope::Pane,
        Scope::PaneIncognito,
    ];
    let mut keymap = Keymap::new().with_leader(LEADER);
    keymap.describe_group("<leader>", "leader");
    keymap.describe_group("<leader>p", "pane");
    keymap.describe_group("<leader>t", "tab");
    keymap.describe_group("<leader>g", "new");
    for scope in SIDEBAR {
        keymap
            .bind("j", Intent::SelectNext, KeyCategory::Navigation, scope)
            .bind("k", Intent::SelectPrev, KeyCategory::Navigation, scope)
            .bind("gg", Intent::SelectFirst, KeyCategory::Navigation, scope)
            .bind("G", Intent::SelectLast, KeyCategory::Navigation, scope)
            .bind(
                "<c-d>",
                Intent::SelectHalfPageDown,
                KeyCategory::Navigation,
                scope,
            )
            .bind(
                "<c-u>",
                Intent::SelectHalfPageUp,
                KeyCategory::Navigation,
                scope,
            )
            .bind("<enter>", Intent::Attach, KeyCategory::Sessions, scope)
            .bind("/", Intent::Search, KeyCategory::Navigation, scope)
            .bind("i", Intent::Search, KeyCategory::Navigation, scope)
            .bind("l", Intent::Unfold, KeyCategory::Navigation, scope)
            .bind("h", Intent::Fold, KeyCategory::Navigation, scope)
            .bind(
                "<leader>f",
                Intent::FilterProjects,
                KeyCategory::Sessions,
                scope,
            );
    }
    for scope in [Scope::Sidebar, Scope::SidebarIncognito] {
        keymap
            .bind("r", Intent::Rename, KeyCategory::Threads, scope)
            .bind("p", Intent::TogglePin, KeyCategory::Threads, scope)
            .bind("s", Intent::ToggleSettle, KeyCategory::Threads, scope)
            .bind("d", Intent::Delete, KeyCategory::Threads, scope);
    }
    for scope in [Scope::SidebarAgent, Scope::SidebarAgentIncognito] {
        keymap
            .bind("r", Intent::Rename, KeyCategory::Threads, scope)
            .bind("d", Intent::Delete, KeyCategory::Threads, scope);
    }
    for scope in SIDEBAR.into_iter().chain(PANE) {
        keymap
            .bind("<leader>q", Intent::Quit, KeyCategory::General, scope)
            .bind(
                "<leader>e",
                Intent::ToggleSidebar,
                KeyCategory::Navigation,
                scope,
            )
            .bind(
                "<leader><space>",
                Intent::OpenSessionPicker,
                KeyCategory::Navigation,
                scope,
            )
            .bind(
                "<leader>n",
                Intent::NewSession,
                KeyCategory::Sessions,
                scope,
            )
            .bind(
                "<leader>a",
                Intent::AddProject,
                KeyCategory::Sessions,
                scope,
            )
            .bind(
                "<leader>i",
                Intent::NewIncognito,
                KeyCategory::Sessions,
                scope,
            )
            .bind(
                "<leader>gr",
                Intent::NewFolder(FolderKind::Research),
                KeyCategory::Sessions,
                scope,
            )
            .bind(
                "<leader>gl",
                Intent::NewFolder(FolderKind::Learn),
                KeyCategory::Sessions,
                scope,
            )
            .bind(
                "<leader>/",
                Intent::OpenSearch,
                KeyCategory::Navigation,
                scope,
            )
            .bind(
                "<leader>W",
                Intent::OpenWorktreePicker,
                KeyCategory::Navigation,
                scope,
            );
    }
    for scope in [Scope::Sidebar, Scope::SidebarAgent, Scope::Pane] {
        keymap
            .bind(
                "<leader>w",
                Intent::ChangeWorkspace,
                KeyCategory::Sessions,
                scope,
            )
            .bind(
                "<leader>b",
                Intent::SwitchBranch,
                KeyCategory::Sessions,
                scope,
            );
    }
    for scope in SESSION {
        keymap
            .bind("<leader>tn", Intent::NewTab, KeyCategory::Navigation, scope)
            .bind(
                "<leader>tx",
                Intent::CloseTab,
                KeyCategory::Navigation,
                scope,
            )
            .bind(
                "<leader>tr",
                Intent::RenameTab,
                KeyCategory::Navigation,
                scope,
            );
        for n in 1..=9 {
            keymap.bind(
                &format!("<leader>t{n}"),
                Intent::GoToTab(n),
                KeyCategory::Navigation,
                scope,
            );
        }
    }
    for scope in PANE {
        keymap
            .bind(
                "<leader>pd",
                Intent::SplitPane(Split::Down),
                KeyCategory::Navigation,
                scope,
            )
            .bind(
                "<leader>pr",
                Intent::SplitPane(Split::Right),
                KeyCategory::Navigation,
                scope,
            )
            .bind(
                "<leader>ps",
                Intent::StackPane,
                KeyCategory::Navigation,
                scope,
            )
            .bind(
                "<leader>pf",
                Intent::ToggleZoom,
                KeyCategory::Navigation,
                scope,
            )
            .bind(
                "<leader>tb",
                Intent::BreakPane,
                KeyCategory::Navigation,
                scope,
            )
            .bind(
                "<leader>px",
                Intent::ClosePane,
                KeyCategory::Navigation,
                scope,
            )
            .bind(
                "<leader>pc",
                Intent::RenamePane,
                KeyCategory::Navigation,
                scope,
            )
            .bind(
                "<leader><c-g>",
                Intent::SendCtrlG,
                KeyCategory::Navigation,
                scope,
            );
    }
    keymap
}

/// Feeds `key` to the keymap; returns the intent once a binding completes.
pub(crate) fn press(keys: &mut Keys, key: KeyEvent) -> Option<Intent> {
    // Bindings compare the whole event: a held key (`Repeat`) or one carrying
    // lock-state bits would never match, so keep only the code and modifiers.
    // A shifted character already carries its case (`G`), and the bindings
    // name it without SHIFT.
    let modifiers = match key.code {
        KeyCode::Char(_) => key.modifiers - KeyModifiers::SHIFT,
        _ => key.modifiers,
    };
    keys.handle_key(KeyEvent::new(key.code, modifiers))
}

/// Where a key goes in the sidebar or a pane.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// A Cmd key or a jump key: run this intent.
    Intent(Intent),
    /// Feed it to the which-key keymap.
    Keymap,
    /// Write it to the focused pane's program.
    Forward,
}

/// Where `key` goes in the sidebar, or in a pane when `in_pane`, with a
/// which-key sequence `pending` or not: the Cmd keys, then `<C-S-h>` and
/// `<C-S-l>`, then the jump keys, then the keymap. In a pane only `<C-g>` and the rest of the sequence it
/// starts go to the keymap; every other key goes to its program.
pub(crate) fn route(key: KeyEvent, in_pane: bool, pending: bool) -> Route {
    match cmd_route(key)
        .or_else(|| ctrl_shift_route(key))
        .or_else(|| jump_route(key))
    {
        Some(intent) => Route::Intent(intent),
        None if !in_pane || pending || is_leader(key) => Route::Keymap,
        None => Route::Forward,
    }
}

/// Whether `key` is `<C-g>`, whatever its kind or lock state.
fn is_leader(key: KeyEvent) -> bool {
    (key.code, key.modifiers) == (LEADER.code, LEADER.modifiers)
}

/// The move of the keys `key` asks for: `<C-S-h>` to the sidebar and
/// `<C-S-l>` into the panes. Kitty's alternate key report makes crossterm
/// read Ctrl+Shift+h as `H` with only Ctrl; without it the key is `h` with
/// Ctrl and Shift. Plain `<C-h>` and `<C-l>` never match: in a pane they
/// belong to its program.
fn ctrl_shift_route(key: KeyEvent) -> Option<Intent> {
    let ctrl_shift = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
    match (key.code, key.modifiers) {
        (KeyCode::Char('H'), KeyModifiers::CONTROL) => Some(Intent::LeavePane),
        (KeyCode::Char('h'), modifiers) if modifiers == ctrl_shift => Some(Intent::LeavePane),
        (KeyCode::Char('L'), KeyModifiers::CONTROL) => Some(Intent::FocusPanes),
        (KeyCode::Char('l'), modifiers) if modifiers == ctrl_shift => Some(Intent::FocusPanes),
        _ => None,
    }
}

/// The jump `key` asks for: `<C-[>` goes back through the jump list and
/// `<C-]>` forward, as kitty reports them under the disambiguate flag
/// (`CSI 91;5u`, `CSI 93;5u`). A plain Esc, which is what Ctrl+[ sends
/// without that flag, never matches; nor does the legacy `<C-]>` byte,
/// which crossterm reads as Ctrl+5.
fn jump_route(key: KeyEvent) -> Option<Intent> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('['), KeyModifiers::CONTROL) => Some(Intent::JumpBack),
        (KeyCode::Char(']'), KeyModifiers::CONTROL) => Some(Intent::JumpForward),
        _ => None,
    }
}

/// The Cmd key `key` asks for, in the sidebar or a pane. Cmd arrives as
/// Super through kitty's `map cmd+<key> send_key super+<key>`. Shift is
/// ignored so `Cmd +` matches however kitty reports it. `None` for any other
/// key, Cmd or not.
fn cmd_route(key: KeyEvent) -> Option<Intent> {
    if key.modifiers - KeyModifiers::SHIFT != KeyModifiers::SUPER {
        return None;
    }
    match key.code {
        KeyCode::Char('h') | KeyCode::Left => Some(Intent::MoveFocus(NavDirection::Left)),
        KeyCode::Char('j') | KeyCode::Down => Some(Intent::MoveFocus(NavDirection::Down)),
        KeyCode::Char('k') | KeyCode::Up => Some(Intent::MoveFocus(NavDirection::Up)),
        KeyCode::Char('l') | KeyCode::Right => Some(Intent::MoveFocus(NavDirection::Right)),
        KeyCode::Char('n') => Some(Intent::AddPane),
        KeyCode::Char('x') => Some(Intent::ClosePane),
        KeyCode::Char('+' | '=') => Some(Intent::GrowFocused),
        KeyCode::Char('-') => Some(Intent::ShrinkFocused),
        KeyCode::Char(digit @ '1'..='9') => digit.to_digit(10).map(|n| Intent::GoToTab(n as usize)),
        KeyCode::Char('[') => Some(Intent::PreviousSwapLayout),
        KeyCode::Char(']') => Some(Intent::NextSwapLayout),
        KeyCode::Char('i') => Some(Intent::MoveTabLeft),
        KeyCode::Char('o') => Some(Intent::MoveTabRight),
        _ => None,
    }
}

/// What `key` does in an open picker; `None` when it does nothing.
pub(crate) fn picker_route(key: KeyEvent) -> Option<Intent> {
    match (key.code, key.modifiers) {
        (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
            Some(Intent::PickerInput(c))
        }
        (KeyCode::Backspace, KeyModifiers::NONE) => Some(Intent::PickerBackspace),
        (KeyCode::Char('w'), KeyModifiers::CONTROL) => Some(Intent::PickerDeleteWord),
        (KeyCode::Left, KeyModifiers::NONE) => Some(Intent::PickerCursorLeft),
        (KeyCode::Right, KeyModifiers::NONE) => Some(Intent::PickerCursorRight),
        (KeyCode::Down, KeyModifiers::NONE) | (KeyCode::Char('j'), KeyModifiers::CONTROL) => {
            Some(Intent::PickerNext)
        }
        (KeyCode::Up, KeyModifiers::NONE) | (KeyCode::Char('k'), KeyModifiers::CONTROL) => {
            Some(Intent::PickerPrev)
        }
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => Some(Intent::PickerHalfPageDown),
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => Some(Intent::PickerHalfPageUp),
        (KeyCode::Enter, KeyModifiers::NONE) => Some(Intent::PickerConfirm),
        (KeyCode::Tab, KeyModifiers::NONE) => Some(Intent::PickerOpen),
        (KeyCode::Esc, KeyModifiers::NONE) => Some(Intent::PickerCancel),
        (KeyCode::Char('x'), KeyModifiers::CONTROL) => Some(Intent::PickerRemove),
        (KeyCode::Char('s'), KeyModifiers::CONTROL) => Some(Intent::PickerToggleSettled),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use orb_domain::feat::harness::HarnessId;
    use std::time::SystemTime;

    use crate::test_support::sessions_for;
    use orb_domain::feat::sessions::state::{
        FolderKind, PaneId, PaneLaunch, Project, ProjectId, ProjectKind, SessionId, Sessions,
        SidebarItem, Thread, ThreadId, ThreadStatus,
    };
    use orb_domain::{Focus, Intent};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use ratatui_which_key::{Key, NodeResult};

    use super::{
        Keys, LEADER, Route, Scope, Selection, cmd_route, keymap, picker_route, press, route,
    };
    use orb_domain::feat::layout::tree::{NavDirection, Split};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[rstest::rstest]
    #[case(key(KeyCode::Char('q')), Intent::PickerInput('q'))]
    #[case(key(KeyCode::Char('j')), Intent::PickerInput('j'))]
    #[case(key(KeyCode::Char(' ')), Intent::PickerInput(' '))]
    #[case(
        KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
        Intent::PickerInput('G')
    )]
    #[case(ctrl('j'), Intent::PickerNext)]
    #[case(ctrl('k'), Intent::PickerPrev)]
    #[case(key(KeyCode::Down), Intent::PickerNext)]
    #[case(key(KeyCode::Up), Intent::PickerPrev)]
    #[case(ctrl('d'), Intent::PickerHalfPageDown)]
    #[case(ctrl('u'), Intent::PickerHalfPageUp)]
    #[case(ctrl('w'), Intent::PickerDeleteWord)]
    #[case(key(KeyCode::Backspace), Intent::PickerBackspace)]
    #[case(key(KeyCode::Left), Intent::PickerCursorLeft)]
    #[case(key(KeyCode::Right), Intent::PickerCursorRight)]
    #[case(key(KeyCode::Enter), Intent::PickerConfirm)]
    #[case(key(KeyCode::Tab), Intent::PickerOpen)]
    #[case(key(KeyCode::Esc), Intent::PickerCancel)]
    #[case(ctrl('s'), Intent::PickerToggleSettled)]
    fn picker_keys_map_to_their_intents(#[case] pressed: KeyEvent, #[case] expected: Intent) {
        // Given / When routing the key in an open picker.
        let intent = picker_route(pressed);

        // Then it yields its intent.
        assert_eq!(
            intent.as_ref(),
            Some(&expected),
            "the key for {expected} in the picker"
        );
    }

    #[rstest::rstest]
    fn ctrl_x_asks_to_remove_in_the_picker() {
        // Given / When routing `<C-x>` in an open picker.
        let intent = picker_route(ctrl('x'));

        // Then it asks to remove the highlighted project.
        assert_eq!(
            intent,
            Some(Intent::PickerRemove),
            "<C-x> should remove in the picker"
        );
    }

    #[rstest::rstest]
    #[case(key(KeyCode::Char('j')), Intent::SelectNext)]
    #[case(key(KeyCode::Char('k')), Intent::SelectPrev)]
    #[case(key(KeyCode::Enter), Intent::Attach)]
    #[case(key(KeyCode::Char('p')), Intent::TogglePin)]
    #[case(key(KeyCode::Char('r')), Intent::Rename)]
    #[case(key(KeyCode::Char('l')), Intent::Unfold)]
    #[case(key(KeyCode::Char('h')), Intent::Fold)]
    fn sidebar_keys_map_to_their_intents(#[case] pressed: KeyEvent, #[case] expected: Intent) {
        // Given the keymap in Sidebar focus.
        let mut keys = Keys::new(keymap(), Scope::Sidebar);

        // When pressing the key.
        let intent = press(&mut keys, pressed);

        // Then it yields its intent.
        assert_eq!(
            intent.as_ref(),
            Some(&expected),
            "the key for {expected} in the sidebar"
        );
    }

    #[rstest::rstest]
    fn slash_and_i_search_in_every_sidebar_scope(
        #[values(Scope::Sidebar, Scope::SidebarEmpty, Scope::SidebarIncognito)] scope: Scope,
        #[values('/', 'i')] pressed: char,
    ) {
        // Given the keymap in a sidebar scope.
        let mut keys = Keys::new(keymap(), scope);

        // When pressing the key.
        let intent = press(&mut keys, key(KeyCode::Char(pressed)));

        // Then it starts a search.
        assert_eq!(
            intent,
            Some(Intent::Search),
            "`{pressed}` should search in {scope:?}"
        );
    }

    #[rstest::rstest]
    fn sidebar_jump_keys_map_to_their_intents(
        #[values(Scope::Sidebar, Scope::SidebarEmpty)] scope: Scope,
        #[values(
            (vec![key(KeyCode::Char('g')), key(KeyCode::Char('g'))], Intent::SelectFirst),
            (vec![KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT)], Intent::SelectLast),
            (vec![ctrl('d')], Intent::SelectHalfPageDown),
            (vec![ctrl('u')], Intent::SelectHalfPageUp)
        )]
        binding: (Vec<KeyEvent>, Intent),
    ) {
        // Given the keymap in a sidebar scope.
        let (pressed, expected) = binding;
        let mut keys = Keys::new(keymap(), scope);

        // When pressing the keys in order (Shift+G as kitty reports it).
        let intent = pressed
            .into_iter()
            .map(|pressed| press(&mut keys, pressed))
            .last()
            .flatten();

        // Then it yields its jump.
        assert_eq!(
            intent.as_ref(),
            Some(&expected),
            "the key for {expected} in {scope:?}"
        );
    }

    #[rstest::rstest]
    #[case(Focus::Sidebar, Selection::Session, Scope::Sidebar)]
    #[case(Focus::Sidebar, Selection::Nothing, Scope::SidebarEmpty)]
    #[case(Focus::Sidebar, Selection::OwnFolderSession, Scope::SidebarIncognito)]
    #[case(Focus::Pane, Selection::Session, Scope::Pane)]
    #[case(Focus::Pane, Selection::OwnFolderSession, Scope::PaneIncognito)]
    #[case(Focus::Sidebar, Selection::Agent, Scope::SidebarAgent)]
    #[case(
        Focus::Sidebar,
        Selection::OwnFolderAgent,
        Scope::SidebarAgentIncognito
    )]
    #[case(Focus::Pane, Selection::Agent, Scope::Pane)]
    #[case(Focus::Pane, Selection::OwnFolderAgent, Scope::PaneIncognito)]
    fn scope_follows_focus_and_the_selection(
        #[case] focus: Focus,
        #[case] selection: Selection,
        #[case] expected: Scope,
    ) {
        // Given / When / Then the scope matches the focused side and selection.
        assert_eq!(
            Scope::new(focus, selection),
            expected,
            "the scope in {focus:?} on {selection:?}"
        );
    }

    /// One project of `kind` holding thread 1 in session 1, with the cursor
    /// on `cursor`.
    fn lone(kind: ProjectKind, cursor: SidebarItem) -> Sessions {
        let projects = vec![Project {
            id: ProjectId(1),
            title: "orb".into(),
            root: "/orb".into(),
            created_at: SystemTime::UNIX_EPOCH,
            removed: false,
            repo: true,
            threads: vec![Thread {
                last_session: None,
                harness: HarnessId::new("claude"),
                id: ThreadId(1),
                title: None,
                cwd: "/orb".into(),
                transcript: None,
                status: ThreadStatus::Idle,
                turn_started_at: None,
                pane: Some(PaneLaunch {
                    pane: PaneId(1),
                    session: SessionId(1),
                }),
                branch: None,
                created_at: SystemTime::UNIX_EPOCH,
                last_activity_at: SystemTime::UNIX_EPOCH,
                unseen: false,
                model: None,
            }],
            kind,
        }];
        Sessions {
            sessions: sessions_for(&projects),
            projects,
            cursor: Some(cursor),
            ..Sessions::default()
        }
    }

    #[rstest::rstest]
    #[case::normal_thread(
        ProjectKind::Normal,
        SidebarItem::Session(SessionId(1)),
        Selection::Session
    )]
    #[case::incognito_thread(
        ProjectKind::Incognito,
        SidebarItem::Session(SessionId(1)),
        Selection::OwnFolderSession
    )]
    #[case::normal_agent(
        ProjectKind::Normal,
        SidebarItem::Agent { session: SessionId(1), pane: PaneId(1) },
        Selection::Agent
    )]
    #[case::incognito_agent(
        ProjectKind::Incognito,
        SidebarItem::Agent { session: SessionId(1), pane: PaneId(1) },
        Selection::OwnFolderAgent
    )]
    fn selection_follows_the_lone_rows_project_kind(
        #[case] kind: ProjectKind,
        #[case] cursor: SidebarItem,
        #[case] expected: Selection,
    ) {
        // Given the cursor on a session.
        let sessions = lone(kind, cursor);

        // When reading the selection.
        let selection = Selection::of(&sessions);

        // Then it follows the project's kind.
        assert_eq!(
            selection, expected,
            "the selection on {cursor:?} in a {kind:?} project"
        );
    }

    #[rstest::rstest]
    fn agent_rows_leave_p_and_s_unbound(
        #[values(Scope::SidebarAgent, Scope::SidebarAgentIncognito)] scope: Scope,
        #[values('p', 's')] pressed: char,
    ) {
        // Given the keymap on an agent row.
        let mut keys = Keys::new(keymap(), scope);

        // When pressing the key.
        let intent = press(&mut keys, key(KeyCode::Char(pressed)));

        // Then nothing happens and no sequence starts.
        assert_eq!(
            (intent, keys.is_pending()),
            (None, false),
            "`{pressed}` should do nothing in {scope:?}"
        );
    }

    #[rstest::rstest]
    fn agent_row_keys_map_to_their_intents(
        #[values(Scope::SidebarAgent, Scope::SidebarAgentIncognito)] scope: Scope,
        #[values(
            (KeyCode::Char('r'), Intent::Rename),
            (KeyCode::Char('d'), Intent::Delete),
            (KeyCode::Char('j'), Intent::SelectNext),
            (KeyCode::Enter, Intent::Attach)
        )]
        binding: (KeyCode, Intent),
    ) {
        // Given the keymap on an agent row.
        let (pressed, expected) = binding;
        let mut keys = Keys::new(keymap(), scope);

        // When pressing the key.
        let intent = press(&mut keys, key(pressed));

        // Then it yields its intent.
        assert_eq!(
            intent.as_ref(),
            Some(&expected),
            "the key for {expected} in {scope:?}"
        );
    }

    #[rstest::rstest]
    fn n_is_unbound_in_the_sidebar(#[values(Scope::Sidebar, Scope::SidebarEmpty)] scope: Scope) {
        // Given the keymap in the sidebar.
        let mut keys = Keys::new(keymap(), scope);

        // When pressing `n`.
        let intent = press(&mut keys, key(KeyCode::Char('n')));

        // Then nothing happens.
        assert_eq!(intent, None, "n in {scope:?}");
    }

    /// The keys the popup lists in `scope` after `<C-g>` and `then`, as
    /// which-key names them, sorted.
    fn leader_popup(scope: Scope, then: &[KeyEvent]) -> Vec<String> {
        let mut keys: Vec<String> = keymap()
            .get_children_at_path(&[&[LEADER], then].concat(), &scope)
            .unwrap_or_default()
            .into_iter()
            .map(|(key, _)| key.display())
            .collect();
        keys.sort_unstable();
        keys
    }

    /// Every key sequence bound in `scope` with its intent, sorted by sequence.
    fn bindings(scope: Scope) -> Vec<(Vec<KeyEvent>, Intent)> {
        let keymap = keymap();
        let mut paths: Vec<Vec<KeyEvent>> = keymap
            .get_children_at_path(&[], &scope)
            .unwrap_or_default()
            .into_iter()
            .map(|(key, _)| vec![key])
            .collect();
        let mut found = vec![];
        while let Some(path) = paths.pop() {
            match keymap.navigate(&path, &scope) {
                Some(NodeResult::Leaf { action }) => found.push((path, action)),
                Some(NodeResult::Branch { children }) => paths.extend(
                    children
                        .into_iter()
                        .map(|child| [path.clone(), vec![child.key]].concat()),
                ),
                None => {}
            }
        }
        found.sort_by_key(|(path, _)| format!("{path:?}"));
        found
    }

    #[rstest::rstest]
    #[case(vec![key(KeyCode::Char('p'))])]
    #[case(vec![key(KeyCode::Char('s'))])]
    #[case(vec![key(KeyCode::Char('r'))])]
    fn session_keys_are_unbound_with_nothing_selected(#[case] pressed: Vec<KeyEvent>) {
        // Given the keymap in the sidebar with no session selected.
        let mut keys = Keys::new(keymap(), Scope::SidebarEmpty);

        // When pressing `p`, `s` or `r`.
        let intents: Vec<Option<Intent>> = pressed
            .into_iter()
            .map(|pressed| press(&mut keys, pressed))
            .collect();

        // Then nothing happens.
        assert!(
            intents.iter().all(Option::is_none),
            "a session key did something with nothing selected: {intents:?}"
        );
    }

    #[rstest::rstest]
    fn r_is_unbound_on_the_settled_header_or_no_row() {
        // Given the keymap in the sidebar with no session selected.
        let mut keys = Keys::new(keymap(), Scope::SidebarEmpty);

        // When pressing `r`.
        let intent = press(&mut keys, key(KeyCode::Char('r')));

        // Then nothing happens.
        assert_eq!(intent, None, "only a thread can be renamed");
    }

    #[rstest::rstest]
    #[case('s', Intent::ToggleSettle)]
    #[case('d', Intent::Delete)]
    fn sidebar_thread_key_yields_its_intent(#[case] c: char, #[case] expected: Intent) {
        // Given the keymap in Sidebar focus.
        let mut keys = Keys::new(keymap(), Scope::Sidebar);

        // When pressing the key once.
        let intent = press(&mut keys, key(KeyCode::Char(c)));

        // Then it yields its intent.
        assert_eq!(intent, Some(expected), "{c} in the sidebar");
    }

    #[rstest::rstest]
    fn held_j_selects_the_next_thread() {
        // Given a held `j` as the kitty protocol reports it.
        let mut keys = Keys::new(keymap(), Scope::Sidebar);
        let held = KeyEvent::new_with_kind_and_state(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
            KeyEventKind::Repeat,
            KeyEventState::NUM_LOCK,
        );

        // When pressing it.
        let intent = press(&mut keys, held);

        // Then it still selects the next thread.
        assert_eq!(
            intent,
            Some(Intent::SelectNext),
            "a repeated j should select the next thread"
        );
    }

    fn cmd(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SUPER)
    }

    #[rstest::rstest]
    #[case(KeyCode::Char('h'), Intent::MoveFocus(NavDirection::Left))]
    #[case(KeyCode::Char('j'), Intent::MoveFocus(NavDirection::Down))]
    #[case(KeyCode::Char('k'), Intent::MoveFocus(NavDirection::Up))]
    #[case(KeyCode::Char('l'), Intent::MoveFocus(NavDirection::Right))]
    #[case(KeyCode::Left, Intent::MoveFocus(NavDirection::Left))]
    #[case(KeyCode::Down, Intent::MoveFocus(NavDirection::Down))]
    #[case(KeyCode::Up, Intent::MoveFocus(NavDirection::Up))]
    #[case(KeyCode::Right, Intent::MoveFocus(NavDirection::Right))]
    #[case(KeyCode::Char('n'), Intent::AddPane)]
    #[case(KeyCode::Char('x'), Intent::ClosePane)]
    #[case(KeyCode::Char('+'), Intent::GrowFocused)]
    #[case(KeyCode::Char('='), Intent::GrowFocused)]
    #[case(KeyCode::Char('-'), Intent::ShrinkFocused)]
    #[case(KeyCode::Char('1'), Intent::GoToTab(1))]
    #[case(KeyCode::Char('2'), Intent::GoToTab(2))]
    #[case(KeyCode::Char('3'), Intent::GoToTab(3))]
    #[case(KeyCode::Char('4'), Intent::GoToTab(4))]
    #[case(KeyCode::Char('5'), Intent::GoToTab(5))]
    #[case(KeyCode::Char('6'), Intent::GoToTab(6))]
    #[case(KeyCode::Char('7'), Intent::GoToTab(7))]
    #[case(KeyCode::Char('8'), Intent::GoToTab(8))]
    #[case(KeyCode::Char('9'), Intent::GoToTab(9))]
    #[case(KeyCode::Char('['), Intent::PreviousSwapLayout)]
    #[case(KeyCode::Char(']'), Intent::NextSwapLayout)]
    #[case(KeyCode::Char('i'), Intent::MoveTabLeft)]
    #[case(KeyCode::Char('o'), Intent::MoveTabRight)]
    fn cmd_keys_route_to_their_intents(
        #[case] code: KeyCode,
        #[case] expected: Intent,
        #[values(false, true)] in_pane: bool,
    ) {
        // Given / When routing Cmd with `code` in the sidebar or a pane.
        let routed = route(cmd(code), in_pane, false);

        // Then it asks for its intent.
        assert_eq!(
            routed,
            Route::Intent(expected),
            "Cmd {code} should route (in a pane: {in_pane})"
        );
    }

    #[rstest::rstest]
    fn cmd_plus_with_shift_grows() {
        // Given Cmd + as kitty reports it with Shift.
        let pressed = KeyEvent::new(
            KeyCode::Char('+'),
            KeyModifiers::SUPER | KeyModifiers::SHIFT,
        );

        // When routing it.
        let intent = cmd_route(pressed);

        // Then it grows.
        assert_eq!(intent, Some(Intent::GrowFocused), "Cmd Shift + should grow");
    }

    #[rstest::rstest]
    fn ctrl_letters_are_not_cmd_keys() {
        // Given Ctrl h.
        let pressed = ctrl('h');

        // When routing it as a Cmd key.
        let intent = cmd_route(pressed);

        // Then it isn't one.
        assert_eq!(intent, None, "<C-h> is not a Cmd key");
    }

    #[rstest::rstest]
    #[case::kitty_alternate_h(KeyCode::Char('H'), KeyModifiers::CONTROL, Intent::LeavePane)]
    #[case::shifted_h(
        KeyCode::Char('h'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        Intent::LeavePane
    )]
    #[case::kitty_alternate_l(KeyCode::Char('L'), KeyModifiers::CONTROL, Intent::FocusPanes)]
    #[case::shifted_l(
        KeyCode::Char('l'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        Intent::FocusPanes
    )]
    fn ctrl_shift_h_and_l_move_the_keys(
        #[case] code: KeyCode,
        #[case] modifiers: KeyModifiers,
        #[case] expected: Intent,
        #[values(false, true)] in_pane: bool,
    ) {
        // Given / When routing Ctrl+Shift with `code` in the sidebar or a pane.
        let routed = route(KeyEvent::new(code, modifiers), in_pane, false);

        // Then it asks for its intent.
        assert_eq!(
            routed,
            Route::Intent(expected),
            "{code} with {modifiers:?} should route (in a pane: {in_pane})"
        );
    }

    #[rstest::rstest]
    #[case('h')]
    #[case('l')]
    fn plain_ctrl_h_and_l_go_to_the_pane(#[case] c: char) {
        // Given plain Ctrl with `c`.
        let pressed = ctrl(c);

        // When routing it in a pane.
        let routed = route(pressed, true, false);

        // Then the pane's program gets it.
        assert_eq!(routed, Route::Forward, "<C-{c}> belongs to the pane");
    }

    #[rstest::rstest]
    fn plain_q_in_the_sidebar_does_nothing() {
        // Given the keymap in Sidebar focus.
        let mut keys = Keys::new(keymap(), Scope::Sidebar);

        // When pressing `q`.
        let intent = press(&mut keys, key(KeyCode::Char('q')));

        // Then nothing happens.
        assert_eq!(intent, None, "<C-g> q quits, not q");
    }

    #[rstest::rstest]
    #[case(false, Route::Keymap)]
    #[case(true, Route::Forward)]
    fn cmd_f_is_unbound(#[case] in_pane: bool, #[case] expected: Route) {
        // Given Cmd f, reserved for floating panes.
        let pressed = cmd(KeyCode::Char('f'));

        // When routing it in the sidebar or a pane.
        let routed = route(pressed, in_pane, false);

        // Then orb asks for nothing: a pane gets it, the sidebar's keymap
        // has no binding for it.
        assert_eq!(
            routed, expected,
            "Cmd f stays unbound (in a pane: {in_pane})"
        );
    }

    #[rstest::rstest]
    #[case('[', Intent::JumpBack)]
    #[case(']', Intent::JumpForward)]
    fn ctrl_brackets_jump_in_the_sidebar_and_in_a_pane(
        #[case] c: char,
        #[case] expected: Intent,
        #[values(false, true)] in_pane: bool,
    ) {
        // Given `<C-[>` or `<C-]>` as kitty reports it under the disambiguate
        // flag.
        let pressed = ctrl(c);

        // When routing it in the sidebar or a pane.
        let routed = route(pressed, in_pane, false);

        // Then it moves through the jump list.
        assert_eq!(
            routed,
            Route::Intent(expected),
            "<C-{c}> should jump (in a pane: {in_pane})"
        );
    }

    #[rstest::rstest]
    #[case(KeyEvent::new(KeyCode::Char('['), KeyModifiers::CONTROL | KeyModifiers::SHIFT))]
    #[case(key(KeyCode::Char('[')))]
    #[case(key(KeyCode::Char(']')))]
    fn jump_keys_need_bare_ctrl(#[case] pressed: KeyEvent) {
        // Given a bracket without bare Ctrl.

        // When routing it in a pane.
        let routed = route(pressed, true, false);

        // Then the pane's program gets it.
        assert_eq!(routed, Route::Forward, "{pressed:?} shouldn't jump");
    }

    #[rstest::rstest]
    fn esc_is_forwarded_in_a_pane() {
        // Given a plain Esc, which is also what Ctrl+[ sends without the
        // kitty protocol.
        let pressed = key(KeyCode::Esc);

        // When routing it in a pane.
        let routed = route(pressed, true, false);

        // Then the pane's program gets it.
        assert_eq!(routed, Route::Forward, "Esc should reach the pane");
    }

    #[rstest::rstest]
    fn esc_never_jumps_in_the_sidebar() {
        // Given a plain Esc.
        let pressed = key(KeyCode::Esc);

        // When routing it in the sidebar.
        let routed = route(pressed, false, false);

        // Then it goes to the keymap, not the jump list.
        assert_eq!(routed, Route::Keymap, "Esc should never jump");
    }

    #[rstest::rstest]
    fn legacy_ctrl_right_bracket_is_not_a_jump() {
        // Given Ctrl+5, which is also how crossterm reads the legacy `<C-]>`
        // byte.
        let pressed = ctrl('5');

        // When routing it in a pane.
        let routed = route(pressed, true, false);

        // Then the pane's program gets it.
        assert_eq!(routed, Route::Forward, "Ctrl+5 shouldn't jump forward");
    }

    #[rstest::rstest]
    #[case(ctrl('\\'))]
    #[case(ctrl('4'))]
    #[case(ctrl('h'))]
    #[case(ctrl('b'))]
    #[case(ctrl(' '))]
    #[case(ctrl('o'))]
    #[case(ctrl('i'))]
    #[case(ctrl('l'))]
    #[case(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL))]
    #[case(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL))]
    fn old_direct_keys_are_forwarded_in_a_pane(#[case] pressed: KeyEvent) {
        // Given a Ctrl key orb took from the pane before.

        // When routing it in a pane.
        let routed = route(pressed, true, false);

        // Then the pane's program gets it.
        assert_eq!(routed, Route::Forward, "{pressed:?} should reach the pane");
    }

    #[rstest::rstest]
    #[case(key(KeyCode::Char('q')))]
    #[case(key(KeyCode::Enter))]
    #[case(key(KeyCode::Char('a')))]
    #[case(key(KeyCode::Char(' ')))]
    #[case(key(KeyCode::Backspace))]
    #[case(key(KeyCode::Tab))]
    fn plain_keys_are_forwarded_in_a_pane(#[case] pressed: KeyEvent) {
        // Given a plain key.

        // When routing it in a pane.
        let routed = route(pressed, true, false);

        // Then the pane's program gets it.
        assert_eq!(routed, Route::Forward, "{pressed:?} should reach the pane");
    }

    #[rstest::rstest]
    fn old_direct_keys_are_unbound_in_the_sidebar(
        #[values(Scope::Sidebar, Scope::SidebarEmpty)] scope: Scope,
        #[values(
            ctrl('\\'),
            ctrl('4'),
            ctrl('l'),
            ctrl('o'),
            ctrl('i'),
            KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL)
        )]
        pressed: KeyEvent,
    ) {
        // Given the keymap in a sidebar scope.
        let mut keys = Keys::new(keymap(), scope);

        // When pressing a Ctrl key orb bound before.
        let intent = press(&mut keys, pressed);

        // Then nothing happens and no sequence starts.
        assert_eq!(
            (intent, keys.is_pending()),
            (None, false),
            "{pressed:?} should do nothing in {scope:?}"
        );
    }

    /// The key that types `c`: a capital with Shift, as kitty reports it,
    /// and BEL (`\x07`) as `<C-g>`.
    fn typed(c: char) -> KeyEvent {
        match c {
            '\x07' => LEADER,
            c if c.is_uppercase() => KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT),
            c => key(KeyCode::Char(c)),
        }
    }

    #[rstest::rstest]
    fn ctrl_g_opens_the_leader_popup(
        #[values(
            Scope::Sidebar,
            Scope::SidebarEmpty,
            Scope::SidebarIncognito,
            Scope::Pane,
            Scope::PaneIncognito
        )]
        scope: Scope,
    ) {
        // Given the keymap in `scope`.
        let mut keys = Keys::new(keymap(), scope);

        // When pressing `<C-g>`.
        press(&mut keys, LEADER);

        // Then the which-key popup waits for the next key.
        assert!(
            keys.is_pending(),
            "<C-g> should open the popup in {scope:?}"
        );
    }

    #[rstest::rstest]
    fn leader_s_is_unbound(#[values(Scope::Sidebar, Scope::Pane)] scope: Scope) {
        // Given `<C-g>` pressed in `scope`.
        let mut keys = Keys::new(keymap(), scope);
        press(&mut keys, LEADER);

        // When pressing `s`.
        let intent = press(&mut keys, key(KeyCode::Char('s')));

        // Then nothing happens and no sequence is left waiting.
        assert_eq!(
            (intent, keys.is_pending()),
            (None, false),
            "<C-g> s should do nothing in {scope:?}"
        );
    }

    #[rstest::rstest]
    fn space_is_unbound_in_the_sidebar() {
        // Given the keymap in the sidebar on a session.
        let mut keys = Keys::new(keymap(), Scope::Sidebar);

        // When pressing Space.
        let intent = press(&mut keys, key(KeyCode::Char(' ')));

        // Then nothing happens and no sequence starts.
        assert_eq!(
            (intent, keys.is_pending()),
            (None, false),
            "Space is no longer a leader"
        );
    }

    #[rstest::rstest]
    #[case(Scope::Pane, "e", Intent::ToggleSidebar)]
    #[case(Scope::Sidebar, "e", Intent::ToggleSidebar)]
    #[case(Scope::Pane, " ", Intent::OpenSessionPicker)]
    #[case(Scope::SidebarEmpty, " ", Intent::OpenSessionPicker)]
    #[case(Scope::Sidebar, "n", Intent::NewSession)]
    #[case(Scope::Pane, "n", Intent::NewSession)]
    #[case(Scope::SidebarEmpty, "a", Intent::AddProject)]
    #[case(Scope::Pane, "a", Intent::AddProject)]
    #[case(Scope::Sidebar, "f", Intent::FilterProjects)]
    #[case(Scope::SidebarEmpty, "f", Intent::FilterProjects)]
    #[case(Scope::PaneIncognito, "i", Intent::NewIncognito)]
    #[case(Scope::SidebarEmpty, "i", Intent::NewIncognito)]
    #[case(Scope::Sidebar, "w", Intent::ChangeWorkspace)]
    #[case(Scope::Pane, "w", Intent::ChangeWorkspace)]
    #[case(Scope::Sidebar, "b", Intent::SwitchBranch)]
    #[case(Scope::Pane, "b", Intent::SwitchBranch)]
    #[case(Scope::SidebarEmpty, "gr", Intent::NewFolder(FolderKind::Research))]
    #[case(Scope::Pane, "gl", Intent::NewFolder(FolderKind::Learn))]
    #[case(Scope::Sidebar, "/", Intent::OpenSearch)]
    #[case(Scope::Pane, "/", Intent::OpenSearch)]
    #[case(Scope::SidebarEmpty, "W", Intent::OpenWorktreePicker)]
    #[case(Scope::Pane, "W", Intent::OpenWorktreePicker)]
    #[case(Scope::Pane, "pd", Intent::SplitPane(Split::Down))]
    #[case(Scope::Pane, "pr", Intent::SplitPane(Split::Right))]
    #[case(Scope::Pane, "pf", Intent::ToggleZoom)]
    #[case(Scope::Pane, "px", Intent::ClosePane)]
    #[case(Scope::Pane, "pc", Intent::RenamePane)]
    #[case(Scope::Pane, "ps", Intent::StackPane)]
    #[case(Scope::Pane, "tb", Intent::BreakPane)]
    #[case(Scope::Pane, "tn", Intent::NewTab)]
    #[case(Scope::Sidebar, "tn", Intent::NewTab)]
    #[case(Scope::Pane, "tx", Intent::CloseTab)]
    #[case(Scope::Sidebar, "tx", Intent::CloseTab)]
    #[case(Scope::Pane, "tr", Intent::RenameTab)]
    #[case(Scope::Sidebar, "tr", Intent::RenameTab)]
    #[case(Scope::Pane, "t1", Intent::GoToTab(1))]
    #[case(Scope::Sidebar, "t6", Intent::GoToTab(6))]
    #[case(Scope::Pane, "t9", Intent::GoToTab(9))]
    #[case(Scope::Pane, "\x07", Intent::SendCtrlG)]
    #[case(Scope::Pane, "q", Intent::Quit)]
    #[case(Scope::Sidebar, "q", Intent::Quit)]
    #[case(Scope::SidebarEmpty, "q", Intent::Quit)]
    fn leader_keys_yield_their_intents(
        #[case] scope: Scope,
        #[case] then: &str,
        #[case] expected: Intent,
    ) {
        // Given `<C-g>` pressed in `scope`.
        let mut keys = Keys::new(keymap(), scope);
        press(&mut keys, LEADER);

        // When typing the rest of the sequence.
        let intent = then
            .chars()
            .map(|c| press(&mut keys, typed(c)))
            .last()
            .flatten();

        // Then it yields its intent.
        assert_eq!(
            intent.as_ref(),
            Some(&expected),
            "<C-g> {then:?} in {scope:?}"
        );
    }

    #[rstest::rstest]
    #[case(Scope::Sidebar, "/ Space W a b e f g i n q t w")]
    #[case(Scope::SidebarEmpty, "/ Space W a e f g i n q")]
    #[case(Scope::SidebarIncognito, "/ Space W a e f g i n q t")]
    #[case(Scope::Pane, "/ <C-g> Space W a b e g i n p q t w")]
    #[case(Scope::PaneIncognito, "/ <C-g> Space W a e g i n p q t")]
    #[case(Scope::SidebarAgent, "/ Space W a b e f g i n q t w")]
    #[case(Scope::SidebarAgentIncognito, "/ Space W a e f g i n q t")]
    fn leader_popup_matches_the_scope_table(#[case] scope: Scope, #[case] expected: &str) {
        // Given orb's keymap in the scope.

        // When listing the keys after `<C-g>`, sorted.
        let found = leader_popup(scope, &[]).join(" ");

        // Then they are the scope's row of the key table.
        assert_eq!(found, expected, "<C-g> keys in {scope:?}");
    }

    #[rstest::rstest]
    fn leader_p_popup_lists_the_pane_keys() {
        // Given orb's keymap in a pane.

        // When listing the keys after `<C-g> p`.
        let found = leader_popup(Scope::Pane, &[key(KeyCode::Char('p'))]).join(" ");

        // Then they are the pane keys.
        assert_eq!(found, "c d f r s x", "<C-g> p keys in a pane");
    }

    #[rstest::rstest]
    #[case::sidebar(Scope::Sidebar, "1 2 3 4 5 6 7 8 9 n r x")]
    #[case::pane_adds_break(Scope::Pane, "1 2 3 4 5 6 7 8 9 b n r x")]
    fn leader_t_popup_lists_the_tab_keys(#[case] scope: Scope, #[case] expected: &str) {
        // Given orb's keymap with a session selected.

        // When listing the keys after `<C-g> t`.
        let found = leader_popup(scope, &[key(KeyCode::Char('t'))]).join(" ");

        // Then they are the tab keys, `b` only where there is a focused pane to break out.
        assert_eq!(found, expected, "<C-g> t keys in {scope:?}");
    }

    #[rstest::rstest]
    fn leader_g_popup_lists_research_and_learn(
        #[values(
            Scope::Sidebar,
            Scope::SidebarEmpty,
            Scope::SidebarIncognito,
            Scope::Pane,
            Scope::PaneIncognito
        )]
        scope: Scope,
    ) {
        // Given orb's keymap in `scope`.

        // When listing the keys after `<C-g> g`.
        let found = leader_popup(scope, &[key(KeyCode::Char('g'))]).join(" ");

        // Then they are the Research and Learn keys.
        assert_eq!(found, "l r", "<C-g> g keys in {scope:?}");
    }

    #[rstest::rstest]
    #[case(Scope::Sidebar, Scope::SidebarIncognito)]
    #[case(Scope::Pane, Scope::PaneIncognito)]
    fn incognito_scope_binds_its_base_scopes_keys_but_w_and_b(
        #[case] base: Scope,
        #[case] incognito: Scope,
    ) {
        // Given the bindings of the base scope, less `<C-g> w` and `<C-g> b`.
        let expected: Vec<(Vec<KeyEvent>, Intent)> = {
            let unbound = ['w', 'b'].map(|c| vec![LEADER, key(KeyCode::Char(c))]);
            bindings(base)
                .into_iter()
                .filter(|(path, _)| !unbound.contains(path))
                .collect()
        };

        // When listing the incognito scope's bindings.
        let found = bindings(incognito);

        // Then they are the same, key for key and intent for intent.
        assert_eq!(found, expected, "{incognito:?} against {base:?}");
    }

    #[rstest::rstest]
    fn ctrl_g_goes_to_the_keymap_in_a_pane() {
        // Given `<C-g>`.

        // When routing it in a pane with no sequence pending.
        let routed = route(LEADER, true, false);

        // Then it goes to the keymap instead of the pane.
        assert_eq!(
            routed,
            Route::Keymap,
            "<C-g> should open which-key in a pane"
        );
    }

    #[rstest::rstest]
    #[case(key(KeyCode::Char('j')))]
    #[case(key(KeyCode::Esc))]
    #[case(key(KeyCode::Char(' ')))]
    fn keys_go_to_the_keymap_while_a_sequence_is_pending_in_a_pane(#[case] pressed: KeyEvent) {
        // Given a `<C-g>` sequence pending in a pane.

        // When routing the next key.
        let routed = route(pressed, true, true);

        // Then the keymap gets it, not the pane.
        assert_eq!(
            routed,
            Route::Keymap,
            "{pressed:?} should continue the sequence"
        );
    }

    #[rstest::rstest]
    fn cmd_keys_still_win_while_a_sequence_is_pending() {
        // Given a `<C-g>` sequence pending in a pane.

        // When routing Cmd h.
        let routed = route(cmd(KeyCode::Char('h')), true, true);

        // Then it moves the focus.
        assert_eq!(
            routed,
            Route::Intent(Intent::MoveFocus(NavDirection::Left)),
            "Cmd h should win over a pending sequence"
        );
    }

    #[rstest::rstest]
    fn ctrl_g_ctrl_g_sends_ctrl_g_in_a_pane() {
        // Given `<C-g>` pressed in a pane.
        let mut keys = Keys::new(keymap(), Scope::Pane);
        press(&mut keys, LEADER);

        // When pressing `<C-g>` again.
        let intent = press(&mut keys, LEADER);

        // Then it sends Ctrl g to the pane.
        assert_eq!(
            intent,
            Some(Intent::SendCtrlG),
            "<C-g> <C-g> should send Ctrl g"
        );
    }
}
