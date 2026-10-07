//! Key routing: which keys do what in each focus. Only keys the user has
//! defined are bound.
//!
//! In the sidebar, keys go through a which-key keymap whose scope is the
//! focus and what the sidebar cursor is on; `<Space>` is the leader and shows
//! a popup. A key that does nothing for the selection isn't bound there
//! (`␣h`/`␣m`/`␣a` off a draft, `␣a` where the selection's harness has no
//! permission modes, `p`/`s`/`r` off a session, `␣w`/`␣b` and the tool keys
//! `␣t`/`␣gg`/`␣v` with nothing selected), so the popups don't offer it.
//! `<C-Right>`/`<C-Left>` resize the focused side outside which-key, which
//! can't name them, and `<C-o>`/`<C-i>` move back and forward through the
//! jump list, also outside which-key. In the sidebar, `<C-\>` detaches the
//! selected attached session, outside which-key too. While attached, every
//! key goes to the attached program except `<C-\>`, `<C-h>`, `<C-b>`,
//! `<C-Space>`, which opens the session picker, the jump keys, and the resize
//! keys, which resize the pane. An open picker takes typed characters as
//! filter text and has its own fixed keys, `<C-x>` among them for removing a
//! project from the project filter and `<C-s>` for showing or hiding settled
//! sessions in the session picker. The rename box (`r`) and the sidebar
//! search (`/` or `i`) use the picker's keys. `␣␣` opens the session picker
//! in every scope. `␣i` opens orb's Incognito draft in every scope. `l`/`h`
//! open and close the Settled shelf. On a group's draft, `␣h`/`␣m`/`␣a` pick
//! the draft's own harness, model and permission. `␣gf`/`␣gr`/`␣gl` add a
//! Feature, Research or Learn group in every scope. The Cmd keys (focus
//! moves, split, close, grow and shrink, tab moves) work in the sidebar and
//! in panes, outside which-key, which has no Super modifier. On orb's
//! Incognito draft and on Incognito, Research and Learn sessions, `␣w`/`␣b`
//! aren't bound: every other key is the same as on any draft or session.

use std::fmt;

use orb_domain::feat::layout::tree::{NavDirection, Split};
use orb_domain::feat::sessions::state::{GroupKind, ProjectKind, SessionKind, Sessions};
use orb_domain::feat::zellij::zellij_service::Tool;
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
    Tools,
}

impl fmt::Display for KeyCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::General => "general",
            Self::Navigation => "navigation",
            Self::Sessions => "sessions",
            Self::Threads => "threads",
            Self::Tools => "tools",
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
    Draft,
    /// orb's Incognito project's draft.
    IncognitoDraft,
    /// A group's draft.
    GroupDraft,
    /// No row, or the settled shelf's header.
    Nothing,
}

impl Selection {
    /// What `sessions`' cursor is on.
    pub(crate) fn of(sessions: &Sessions) -> Self {
        match (
            sessions.selected_draft(),
            sessions.selected_group_draft(),
            sessions.selected_session(),
        ) {
            (Some((project, _)), _, _) if project.kind == ProjectKind::Incognito => {
                Self::IncognitoDraft
            }
            (Some(_), _, _) => Self::Draft,
            (None, Some(_), _) => Self::GroupDraft,
            (None, None, Some(session)) if session.kind != SessionKind::Plain => {
                Self::OwnFolderSession
            }
            (None, None, Some(_)) => Self::Session,
            (None, None, None) => Self::Nothing,
        }
    }
}

/// Which bindings apply: the focused side's, less the keys that do nothing
/// for the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Scope {
    /// The sidebar on a session.
    Sidebar,
    /// The sidebar on a draft: no pin or settle, but its setting pickers.
    SidebarDraft,
    /// The sidebar with no session or draft selected.
    SidebarEmpty,
    /// The sidebar on a group's draft: its setting pickers, and `d`, which
    /// discards it.
    SidebarGroupDraft,
    /// The sidebar on an Incognito, Research or Learn session:
    /// [`Scope::Sidebar`]'s keys but `␣w`/`␣b`.
    SidebarIncognito,
    /// The sidebar on the Incognito draft: [`Scope::SidebarDraft`]'s keys but
    /// `␣w`/`␣b`.
    SidebarIncognitoDraft,
    /// The dashboard on a session.
    Dashboard,
    /// The dashboard on a draft: its setting pickers too.
    DashboardDraft,
    /// The dashboard with no session or draft selected.
    DashboardEmpty,
    /// The dashboard on a group's draft: its setting pickers too.
    DashboardGroupDraft,
    /// The dashboard on an Incognito, Research or Learn session:
    /// [`Scope::Dashboard`]'s keys but `␣w`/`␣b`.
    DashboardIncognito,
    /// The dashboard on the Incognito draft: [`Scope::DashboardDraft`]'s keys
    /// but `␣w`/`␣b`.
    DashboardIncognitoDraft,
}

impl Scope {
    /// The scope for keys in `focus` with `selection`.
    pub(crate) fn new(focus: Focus, selection: Selection) -> Self {
        match (focus, selection) {
            (Focus::Dashboard, Selection::Session) => Self::Dashboard,
            (Focus::Dashboard, Selection::Draft) => Self::DashboardDraft,
            (Focus::Dashboard, Selection::Nothing) => Self::DashboardEmpty,
            (Focus::Dashboard, Selection::GroupDraft) => Self::DashboardGroupDraft,
            (Focus::Dashboard, Selection::OwnFolderSession) => Self::DashboardIncognito,
            (Focus::Dashboard, Selection::IncognitoDraft) => Self::DashboardIncognitoDraft,
            (
                Focus::Sidebar | Focus::Attached | Focus::Picker | Focus::Rename | Focus::Search,
                Selection::Session,
            ) => Self::Sidebar,
            (
                Focus::Sidebar | Focus::Attached | Focus::Picker | Focus::Rename | Focus::Search,
                Selection::Draft,
            ) => Self::SidebarDraft,
            (
                Focus::Sidebar | Focus::Attached | Focus::Picker | Focus::Rename | Focus::Search,
                Selection::Nothing,
            ) => Self::SidebarEmpty,
            (
                Focus::Sidebar | Focus::Attached | Focus::Picker | Focus::Rename | Focus::Search,
                Selection::GroupDraft,
            ) => Self::SidebarGroupDraft,
            (
                Focus::Sidebar | Focus::Attached | Focus::Picker | Focus::Rename | Focus::Search,
                Selection::OwnFolderSession,
            ) => Self::SidebarIncognito,
            (
                Focus::Sidebar | Focus::Attached | Focus::Picker | Focus::Rename | Focus::Search,
                Selection::IncognitoDraft,
            ) => Self::SidebarIncognitoDraft,
        }
    }
}

/// Which bindings apply: [`Scope`]'s, with `␣a` only while the selection's
/// harness lists permission modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct KeyScope {
    scope: Scope,
    permissions: bool,
}

impl KeyScope {
    /// The scope for keys in `focus` over `state`'s selection.
    pub(crate) fn new(focus: Focus, state: &AppState) -> Self {
        Self {
            scope: Scope::new(focus, Selection::of(&state.sessions)),
            permissions: state.offers_permissions(),
        }
    }
}

/// `scope` where the selection's harness lists permission modes.
impl From<Scope> for KeyScope {
    fn from(scope: Scope) -> Self {
        Self {
            scope,
            permissions: true,
        }
    }
}

/// The keymap under construction: `bind` registers a key for both values
/// of the permissions flag, `bind_permissions` only where it is set.
struct Binder(Keymap<KeyEvent, KeyScope, Intent, KeyCategory>);

impl Binder {
    fn bind(
        &mut self,
        sequence: &str,
        intent: Intent,
        category: KeyCategory,
        scope: Scope,
    ) -> &mut Self {
        let without = KeyScope {
            scope,
            permissions: false,
        };
        self.0.bind(sequence, intent.clone(), category, without);
        self.bind_permissions(sequence, intent, category, scope)
    }

    fn bind_permissions(
        &mut self,
        sequence: &str,
        intent: Intent,
        category: KeyCategory,
        scope: Scope,
    ) -> &mut Self {
        self.0.bind(sequence, intent, category, scope.into());
        self
    }
}

/// The keymap with its current scope and pending key sequence.
pub(crate) type Keys = WhichKeyState<KeyEvent, KeyScope, Intent, KeyCategory>;

/// The sidebar and dashboard bindings, scoped by focus and selection.
#[expect(
    clippy::too_many_lines,
    reason = "one binding per key keeps the whole keymap in one place"
)]
pub(crate) fn keymap() -> Keymap<KeyEvent, KeyScope, Intent, KeyCategory> {
    const SIDEBAR: [Scope; 6] = [
        Scope::Sidebar,
        Scope::SidebarDraft,
        Scope::SidebarEmpty,
        Scope::SidebarGroupDraft,
        Scope::SidebarIncognito,
        Scope::SidebarIncognitoDraft,
    ];
    const DASHBOARD: [Scope; 6] = [
        Scope::Dashboard,
        Scope::DashboardDraft,
        Scope::DashboardEmpty,
        Scope::DashboardGroupDraft,
        Scope::DashboardIncognito,
        Scope::DashboardIncognitoDraft,
    ];
    let mut keymap = Binder(Keymap::new());
    keymap.0.describe_group("<leader>", "leader");
    keymap.0.describe_group("<leader>g", "group");
    keymap.0.describe_group("<leader>s", "search");
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
            .bind("<c-l>", Intent::FocusRight, KeyCategory::Navigation, scope)
            .bind("<enter>", Intent::Attach, KeyCategory::Sessions, scope)
            .bind("q", Intent::Quit, KeyCategory::General, scope)
            .bind("/", Intent::Search, KeyCategory::Navigation, scope)
            .bind("i", Intent::Search, KeyCategory::Navigation, scope)
            .bind(
                "<leader>n",
                Intent::NewSession,
                KeyCategory::Sessions,
                scope,
            )
            .bind(
                "<leader>p",
                Intent::AddProject,
                KeyCategory::Sessions,
                scope,
            )
            .bind(
                "<leader>f",
                Intent::FilterProjects,
                KeyCategory::Sessions,
                scope,
            );
    }
    for scope in [
        Scope::Sidebar,
        Scope::SidebarDraft,
        Scope::SidebarEmpty,
        Scope::SidebarIncognito,
        Scope::SidebarIncognitoDraft,
    ] {
        keymap
            .bind("l", Intent::OpenShelf, KeyCategory::Navigation, scope)
            .bind("h", Intent::CloseShelf, KeyCategory::Navigation, scope);
    }
    for scope in [Scope::Sidebar, Scope::SidebarIncognito] {
        keymap
            .bind("r", Intent::Rename, KeyCategory::Threads, scope)
            .bind("p", Intent::TogglePin, KeyCategory::Threads, scope)
            .bind("s", Intent::ToggleSettle, KeyCategory::Threads, scope);
    }
    for scope in [
        Scope::Sidebar,
        Scope::SidebarDraft,
        Scope::SidebarGroupDraft,
        Scope::SidebarIncognito,
        Scope::SidebarIncognitoDraft,
    ] {
        keymap.bind("d", Intent::Delete, KeyCategory::Threads, scope);
    }
    for scope in DASHBOARD {
        keymap
            .bind(
                "<c-h>",
                Intent::FocusSidebar,
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
                "<leader>p",
                Intent::AddProject,
                KeyCategory::Sessions,
                scope,
            );
    }
    for scope in SIDEBAR.into_iter().chain(DASHBOARD) {
        keymap
            .bind(
                "<leader><leader>",
                Intent::OpenSessionPicker,
                KeyCategory::Navigation,
                scope,
            )
            .bind(
                "<leader>sw",
                Intent::OpenWorktreePicker,
                KeyCategory::Navigation,
                scope,
            )
            .bind(
                "<leader>sg",
                Intent::OpenSearch,
                KeyCategory::Navigation,
                scope,
            )
            .bind(
                "<leader>e",
                Intent::ToggleSidebar,
                KeyCategory::Navigation,
                scope,
            )
            .bind(
                "<leader>i",
                Intent::NewIncognito,
                KeyCategory::Sessions,
                scope,
            )
            .bind(
                "<leader>gf",
                Intent::NewGroup(GroupKind::Feature),
                KeyCategory::Sessions,
                scope,
            )
            .bind(
                "<leader>gr",
                Intent::NewGroup(GroupKind::Research),
                KeyCategory::Sessions,
                scope,
            )
            .bind(
                "<leader>gl",
                Intent::NewGroup(GroupKind::Learn),
                KeyCategory::Sessions,
                scope,
            );
    }
    for scope in [
        Scope::Sidebar,
        Scope::SidebarDraft,
        Scope::Dashboard,
        Scope::DashboardDraft,
    ] {
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
    for scope in [
        Scope::Sidebar,
        Scope::SidebarDraft,
        Scope::SidebarGroupDraft,
        Scope::SidebarIncognito,
        Scope::SidebarIncognitoDraft,
        Scope::Dashboard,
        Scope::DashboardDraft,
        Scope::DashboardGroupDraft,
        Scope::DashboardIncognito,
        Scope::DashboardIncognitoDraft,
    ] {
        keymap
            .bind(
                "<leader>t",
                Intent::OpenTool(Tool::Shell),
                KeyCategory::Tools,
                scope,
            )
            .bind(
                "<leader>gg",
                Intent::OpenTool(Tool::Lazygit),
                KeyCategory::Tools,
                scope,
            )
            .bind(
                "<leader>v",
                Intent::OpenTool(Tool::Nvim),
                KeyCategory::Tools,
                scope,
            );
    }
    for scope in [
        Scope::SidebarDraft,
        Scope::DashboardDraft,
        Scope::SidebarGroupDraft,
        Scope::DashboardGroupDraft,
        Scope::SidebarIncognitoDraft,
        Scope::DashboardIncognitoDraft,
    ] {
        keymap
            .bind(
                "<leader>h",
                Intent::PickHarness,
                KeyCategory::Sessions,
                scope,
            )
            .bind("<leader>m", Intent::PickModel, KeyCategory::Sessions, scope)
            .bind_permissions(
                "<leader>a",
                Intent::PickPermission,
                KeyCategory::Sessions,
                scope,
            );
    }
    keymap.0
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

/// Where a key goes while attached.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// The key is bound to an intent.
    Intent(Intent),
    /// The key goes to the attached program.
    Forward,
}

/// Where `key` goes while attached.
pub(crate) fn attached_route(key: KeyEvent) -> Route {
    match (key.code, key.modifiers) {
        // `<C-\>` as the kitty protocol reports it, and as crossterm parses
        // its legacy byte 0x1C.
        (KeyCode::Char('\\' | '4'), KeyModifiers::CONTROL) => Route::Intent(Intent::Detach),
        // The attached program can't bind `<C-h>`: it's Backspace in a legacy terminal.
        (KeyCode::Char('h'), KeyModifiers::CONTROL) => Route::Intent(Intent::LeavePane),
        // The attached program backgrounds a task with `Ctrl+X Ctrl+B`, not `<C-b>`.
        (KeyCode::Char('b'), KeyModifiers::CONTROL) => Route::Intent(Intent::ToggleSidebar),
        // `<C-Space>` as the kitty protocol reports it, and as crossterm
        // parses its legacy NUL byte.
        (KeyCode::Char(' '), KeyModifiers::CONTROL) => Route::Intent(Intent::OpenSessionPicker),
        _ => jump_route(key)
            .or_else(|| layout_route(key))
            .map_or(Route::Forward, Route::Intent),
    }
}

/// The jump `key` asks for in the sidebar, the dashboard or the attached
/// pane: `<C-o>` goes back through the jump list and `<C-i>` forward. Only
/// bare Ctrl matches, so `ctrl+shift+o` and Tab still reach the attached program. `None`
/// for any other key.
pub(crate) fn jump_route(key: KeyEvent) -> Option<Intent> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('o'), KeyModifiers::CONTROL) => Some(Intent::JumpBack),
        (KeyCode::Char('i'), KeyModifiers::CONTROL) => Some(Intent::JumpForward),
        _ => None,
    }
}

/// The resize `key` asks for in the sidebar, the dashboard or the attached
/// pane: `<C-Right>` widens the focused side and `<C-Left>` narrows it.
/// `None` for any other key.
pub(crate) fn layout_route(key: KeyEvent) -> Option<Intent> {
    match (key.code, key.modifiers) {
        (KeyCode::Right, KeyModifiers::CONTROL) => Some(Intent::WidenFocused),
        (KeyCode::Left, KeyModifiers::CONTROL) => Some(Intent::NarrowFocused),
        _ => None,
    }
}

/// The Cmd key `key` asks for, in the sidebar or a pane. Cmd arrives as
/// Super through kitty's `map cmd+<key> send_key super+<key>`. Shift is
/// ignored so `Cmd +` matches however kitty reports it. `None` for any other
/// key, Cmd or not.
pub(crate) fn cmd_route(key: KeyEvent) -> Option<Intent> {
    if key.modifiers - KeyModifiers::SHIFT != KeyModifiers::SUPER {
        return None;
    }
    match key.code {
        KeyCode::Char('h') | KeyCode::Left => Some(Intent::MoveFocus(NavDirection::Left)),
        KeyCode::Char('j') | KeyCode::Down => Some(Intent::MoveFocus(NavDirection::Down)),
        KeyCode::Char('k') | KeyCode::Up => Some(Intent::MoveFocus(NavDirection::Up)),
        KeyCode::Char('l') | KeyCode::Right => Some(Intent::MoveFocus(NavDirection::Right)),
        KeyCode::Char('n') => Some(Intent::SplitPane(Split::Right)),
        KeyCode::Char('x') => Some(Intent::ClosePane),
        KeyCode::Char('+' | '=') => Some(Intent::GrowFocused),
        KeyCode::Char('-') => Some(Intent::ShrinkFocused),
        KeyCode::Char(digit @ '1'..='5') => digit.to_digit(10).map(|n| Intent::GoToTab(n as usize)),
        KeyCode::Char('[') => Some(Intent::PreviousTab),
        KeyCode::Char(']') => Some(Intent::NextTab),
        KeyCode::Char('i') => Some(Intent::MoveTabLeft),
        KeyCode::Char('o') => Some(Intent::MoveTabRight),
        _ => None,
    }
}

/// What `key` does in the sidebar outside which-key: `<C-\>` (the kitty
/// `Char('\\')` and the legacy `Char('4')` forms) detaches the selected
/// session. `None` for any other key.
pub(crate) fn sidebar_route(key: KeyEvent) -> Option<Intent> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('\\' | '4'), KeyModifiers::CONTROL) => Some(Intent::DetachSelected),
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
        Draft, DraftWorkspace, Group, GroupDefaults, GroupDraft, GroupId, GroupKind, PaneId,
        PaneLaunch, Project, ProjectId, ProjectKind, SessionId, Sessions, SidebarItem, Thread,
        ThreadId, ThreadStatus,
    };
    use orb_domain::feat::zellij::zellij_service::Tool;
    use orb_domain::{Focus, Intent};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use ratatui_which_key::NodeResult;

    use super::{
        KeyScope, Keys, Route, Scope, Selection, attached_route, cmd_route, jump_route, keymap,
        layout_route, picker_route, press, sidebar_route,
    };
    use orb_domain::feat::layout::tree::{NavDirection, Split};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[rstest::rstest]
    fn space_opens_the_leader_popup_in_the_sidebar() {
        // Given the keymap in Sidebar focus.
        let mut keys = Keys::new(keymap(), Scope::Sidebar.into());

        // When pressing Space.
        press(&mut keys, key(KeyCode::Char(' ')));

        // Then the which-key popup waits for the next key.
        assert!(keys.is_pending(), "Space should open the leader popup");
    }

    #[rstest::rstest]
    fn space_then_n_starts_a_session_in_the_sidebar() {
        // Given Space already pressed in Sidebar focus.
        let mut keys = Keys::new(keymap(), Scope::Sidebar.into());
        press(&mut keys, key(KeyCode::Char(' ')));

        // When pressing `n`.
        let intent = press(&mut keys, key(KeyCode::Char('n')));

        // Then it starts a session.
        assert_eq!(
            intent,
            Some(Intent::NewSession),
            "Space n should start a session"
        );
    }

    #[rstest::rstest]
    #[case(Scope::Sidebar)]
    #[case(Scope::Dashboard)]
    fn space_then_p_adds_a_project(#[case] scope: Scope) {
        // Given Space already pressed.
        let mut keys = Keys::new(keymap(), scope.into());
        press(&mut keys, key(KeyCode::Char(' ')));

        // When pressing `p`.
        let intent = press(&mut keys, key(KeyCode::Char('p')));

        // Then it opens the directory picker.
        assert_eq!(
            intent,
            Some(Intent::AddProject),
            "Space p should add a project in {scope:?}"
        );
    }

    #[rstest::rstest]
    fn leader_w_on_the_dashboard_changes_workspace() {
        // Given Space already pressed on a thread's dashboard.
        let mut keys = Keys::new(keymap(), Scope::Dashboard.into());
        press(&mut keys, key(KeyCode::Char(' ')));

        // When pressing `w`.
        let intent = press(&mut keys, key(KeyCode::Char('w')));

        // Then it opens the workspace picker.
        assert_eq!(
            intent,
            Some(Intent::ChangeWorkspace),
            "Space w should change the workspace"
        );
    }

    #[rstest::rstest]
    fn leader_b_on_the_dashboard_switches_branch() {
        // Given Space already pressed on a thread's dashboard.
        let mut keys = Keys::new(keymap(), Scope::Dashboard.into());
        press(&mut keys, key(KeyCode::Char(' ')));

        // When pressing `b`.
        let intent = press(&mut keys, key(KeyCode::Char('b')));

        // Then it opens the branch picker.
        assert_eq!(
            intent,
            Some(Intent::SwitchBranch),
            "Space b should switch the branch"
        );
    }

    #[rstest::rstest]
    #[case(Scope::Sidebar, 'w', Intent::ChangeWorkspace)]
    #[case(Scope::Sidebar, 'b', Intent::SwitchBranch)]
    #[case(Scope::SidebarDraft, 'w', Intent::ChangeWorkspace)]
    #[case(Scope::SidebarDraft, 'b', Intent::SwitchBranch)]
    #[case(Scope::SidebarDraft, 'm', Intent::PickModel)]
    #[case(Scope::SidebarDraft, 'a', Intent::PickPermission)]
    fn leader_keys_open_the_session_setup_pickers(
        #[case] scope: Scope,
        #[case] pressed: char,
        #[case] expected: Intent,
    ) {
        // Given Space already pressed.
        let mut keys = Keys::new(keymap(), scope.into());
        press(&mut keys, key(KeyCode::Char(' ')));

        // When pressing the key.
        let intent = press(&mut keys, key(KeyCode::Char(pressed)));

        // Then it yields its picker's intent.
        assert_eq!(
            intent.as_ref(),
            Some(&expected),
            "Space {pressed} in {scope:?}"
        );
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
    fn leader_f_filters_projects_in_the_sidebar(
        #[values(Scope::Sidebar, Scope::SidebarDraft, Scope::SidebarEmpty)] scope: Scope,
    ) {
        // Given Space already pressed in `scope`.
        let mut keys = Keys::new(keymap(), scope.into());
        press(&mut keys, key(KeyCode::Char(' ')));

        // When pressing `f`.
        let intent = press(&mut keys, key(KeyCode::Char('f')));

        // Then it opens the project filter.
        assert_eq!(
            intent,
            Some(Intent::FilterProjects),
            "␣f should filter projects in {scope:?}"
        );
    }

    #[rstest::rstest]
    fn leader_popup_lists_no_filter_on_the_dashboard(
        #[values(Scope::Dashboard, Scope::DashboardDraft, Scope::DashboardEmpty)] scope: Scope,
    ) {
        // Given the leader popup's keys in a dashboard scope.
        let keys = leader_popup(scope);

        // When looking for `f`.
        let found = keys.contains(&key(KeyCode::Char('f')));

        // Then it isn't listed.
        assert!(!found, "␣f is sidebar-only, not in {scope:?}");
    }

    #[rstest::rstest]
    fn o_is_unbound_on_the_dashboard_with_nothing_selected() {
        // Given the keymap on the dashboard with no thread or draft selected.
        let mut keys = Keys::new(keymap(), Scope::DashboardEmpty.into());

        // When pressing `o`.
        let intent = press(&mut keys, key(KeyCode::Char('o')));

        // Then nothing happens.
        assert_eq!(intent, None, "o has nothing to open or start");
    }

    #[rstest::rstest]
    #[case(key(KeyCode::Char('j')), Intent::SelectNext)]
    #[case(key(KeyCode::Char('k')), Intent::SelectPrev)]
    #[case(ctrl('l'), Intent::FocusRight)]
    #[case(key(KeyCode::Enter), Intent::Attach)]
    #[case(key(KeyCode::Char('q')), Intent::Quit)]
    #[case(key(KeyCode::Char('p')), Intent::TogglePin)]
    #[case(key(KeyCode::Char('r')), Intent::Rename)]
    #[case(key(KeyCode::Char('l')), Intent::OpenShelf)]
    #[case(key(KeyCode::Char('h')), Intent::CloseShelf)]
    fn sidebar_keys_map_to_their_intents(#[case] pressed: KeyEvent, #[case] expected: Intent) {
        // Given the keymap in Sidebar focus.
        let mut keys = Keys::new(keymap(), Scope::Sidebar.into());

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
        #[values(
            Scope::Sidebar,
            Scope::SidebarDraft,
            Scope::SidebarEmpty,
            Scope::SidebarGroupDraft,
            Scope::SidebarIncognito,
            Scope::SidebarIncognitoDraft
        )]
        scope: Scope,
        #[values('/', 'i')] pressed: char,
    ) {
        // Given the keymap in a sidebar scope.
        let mut keys = Keys::new(keymap(), scope.into());

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
        #[values(Scope::Sidebar, Scope::SidebarDraft, Scope::SidebarEmpty)] scope: Scope,
        #[values(
            (vec![key(KeyCode::Char('g')), key(KeyCode::Char('g'))], Intent::SelectFirst),
            (vec![KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT)], Intent::SelectLast),
            (vec![ctrl('d')], Intent::SelectHalfPageDown),
            (vec![ctrl('u')], Intent::SelectHalfPageUp),
        )]
        binding: (Vec<KeyEvent>, Intent),
    ) {
        // Given the keymap in a sidebar scope.
        let (pressed, expected) = binding;
        let mut keys = Keys::new(keymap(), scope.into());

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
    fn leader_i_opens_incognito_in_every_scope(
        #[values(
            Scope::Sidebar,
            Scope::SidebarDraft,
            Scope::SidebarEmpty,
            Scope::SidebarGroupDraft,
            Scope::SidebarIncognito,
            Scope::SidebarIncognitoDraft,
            Scope::Dashboard,
            Scope::DashboardDraft,
            Scope::DashboardEmpty,
            Scope::DashboardGroupDraft,
            Scope::DashboardIncognito,
            Scope::DashboardIncognitoDraft
        )]
        scope: Scope,
    ) {
        // Given Space already pressed in `scope`.
        let mut keys = Keys::new(keymap(), scope.into());
        press(&mut keys, key(KeyCode::Char(' ')));

        // When pressing `i`.
        let intent = press(&mut keys, key(KeyCode::Char('i')));

        // Then it opens the incognito draft.
        assert_eq!(
            intent,
            Some(Intent::NewIncognito),
            "␣i should open incognito in {scope:?}"
        );
    }

    #[rstest::rstest]
    fn leader_leader_opens_the_session_picker_in_every_scope(
        #[values(
            Scope::Sidebar,
            Scope::SidebarDraft,
            Scope::SidebarEmpty,
            Scope::SidebarGroupDraft,
            Scope::SidebarIncognito,
            Scope::SidebarIncognitoDraft,
            Scope::Dashboard,
            Scope::DashboardDraft,
            Scope::DashboardEmpty,
            Scope::DashboardGroupDraft,
            Scope::DashboardIncognito,
            Scope::DashboardIncognitoDraft
        )]
        scope: Scope,
    ) {
        // Given Space already pressed in `scope`.
        let mut keys = Keys::new(keymap(), scope.into());
        press(&mut keys, key(KeyCode::Char(' ')));

        // When pressing Space again.
        let intent = press(&mut keys, key(KeyCode::Char(' ')));

        // Then it opens the session picker.
        assert_eq!(
            intent,
            Some(Intent::OpenSessionPicker),
            "␣␣ should open the session picker in {scope:?}"
        );
    }

    #[rstest::rstest]
    fn leader_s_w_opens_the_worktree_picker(
        #[values(
            Scope::Sidebar,
            Scope::SidebarEmpty,
            Scope::Dashboard,
            Scope::DashboardEmpty
        )]
        scope: Scope,
    ) {
        // Given Space and `s` already pressed in `scope`.
        let mut keys = Keys::new(keymap(), scope.into());
        press(&mut keys, key(KeyCode::Char(' ')));
        press(&mut keys, key(KeyCode::Char('s')));

        // When pressing `w`.
        let intent = press(&mut keys, key(KeyCode::Char('w')));

        // Then it opens the worktree picker.
        assert_eq!(
            intent,
            Some(Intent::OpenWorktreePicker),
            "␣sw should open the worktree picker in {scope:?}"
        );
    }

    #[rstest::rstest]
    fn leader_s_g_opens_the_search_picker(
        #[values(
            Scope::Sidebar,
            Scope::SidebarEmpty,
            Scope::Dashboard,
            Scope::DashboardEmpty
        )]
        scope: Scope,
    ) {
        // Given Space and `s` already pressed in `scope`.
        let mut keys = Keys::new(keymap(), scope.into());
        press(&mut keys, key(KeyCode::Char(' ')));
        press(&mut keys, key(KeyCode::Char('s')));

        // When pressing `g`.
        let intent = press(&mut keys, key(KeyCode::Char('g')));

        // Then it opens the search picker.
        assert_eq!(
            intent,
            Some(Intent::OpenSearch),
            "␣sg should open the search picker in {scope:?}"
        );
    }

    #[rstest::rstest]
    fn leader_s_g_is_forwarded_while_attached(#[values(' ', 's', 'g')] c: char) {
        // Given one key of `␣sg`.
        let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);

        // When routing it while attached.
        let routed = attached_route(key);

        // Then it goes to Claude, so `␣sg` never opens the search picker there.
        assert_eq!(
            routed,
            Route::Forward,
            "{c:?} of ␣sg should be forwarded while attached"
        );
    }

    #[rstest::rstest]
    fn leader_e_toggles_the_sidebar(
        #[values(
            Scope::Sidebar,
            Scope::SidebarDraft,
            Scope::SidebarEmpty,
            Scope::Dashboard,
            Scope::DashboardDraft,
            Scope::DashboardEmpty
        )]
        scope: Scope,
    ) {
        // Given Space already pressed in `scope`.
        let mut keys = Keys::new(keymap(), scope.into());
        press(&mut keys, key(KeyCode::Char(' ')));

        // When pressing `e`.
        let intent = press(&mut keys, key(KeyCode::Char('e')));

        // Then it hides or shows the sidebar.
        assert_eq!(
            intent,
            Some(Intent::ToggleSidebar),
            "␣e should toggle the sidebar in {scope:?}"
        );
    }

    #[rstest::rstest]
    #[case(KeyCode::Right, Intent::WidenFocused)]
    #[case(KeyCode::Left, Intent::NarrowFocused)]
    fn ctrl_arrows_resize_the_focused_side(#[case] code: KeyCode, #[case] expected: Intent) {
        // Given Ctrl with an arrow.
        let pressed = KeyEvent::new(code, KeyModifiers::CONTROL);

        // When routing it in the sidebar or dashboard.
        let intent = layout_route(pressed);

        // Then it resizes.
        assert_eq!(intent, Some(expected), "<C-{code}> should resize");
    }

    #[rstest::rstest]
    #[case(key(KeyCode::Right))]
    #[case(key(KeyCode::Left))]
    #[case(KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL))]
    #[case(ctrl('h'))]
    #[case(ctrl('l'))]
    #[case(key(KeyCode::Char('j')))]
    fn other_keys_do_not_resize(#[case] pressed: KeyEvent) {
        // Given a key other than Ctrl+Left/Right.

        // When routing it in the sidebar or dashboard.
        let intent = layout_route(pressed);

        // Then it's left to the keymap.
        assert_eq!(intent, None, "{pressed:?} shouldn't resize");
    }

    #[rstest::rstest]
    #[case('\\')]
    #[case('4')]
    fn ctrl_backslash_detaches_the_selected_thread_in_the_sidebar(#[case] c: char) {
        // Given `<C-\>` in one of its two forms.
        let pressed = ctrl(c);

        // When routing it in the sidebar.
        let intent = sidebar_route(pressed);

        // Then it detaches the selected thread.
        assert_eq!(
            intent,
            Some(Intent::DetachSelected),
            "{pressed:?} should detach the selected thread"
        );
    }

    #[rstest::rstest]
    #[case(ctrl('h'))]
    #[case(key(KeyCode::Char('\\')))]
    #[case(key(KeyCode::Char('4')))]
    fn other_keys_do_not_detach_in_the_sidebar(#[case] pressed: KeyEvent) {
        // Given a key other than `<C-\>`.

        // When routing it in the sidebar.
        let intent = sidebar_route(pressed);

        // Then it's left to the keymap.
        assert_eq!(intent, None, "{pressed:?} shouldn't detach");
    }

    #[rstest::rstest]
    #[case(vec![ctrl('h')], Intent::FocusSidebar)]
    fn dashboard_keys_map_to_their_intents(
        #[case] pressed: Vec<KeyEvent>,
        #[case] expected: Intent,
    ) {
        // Given the keymap on a thread's dashboard.
        let mut keys = Keys::new(keymap(), Scope::Dashboard.into());

        // When pressing the keys in order.
        let intent = pressed
            .into_iter()
            .map(|pressed| press(&mut keys, pressed))
            .last()
            .flatten();

        // Then it yields its intent.
        assert_eq!(
            intent.as_ref(),
            Some(&expected),
            "the key for {expected} on the dashboard"
        );
    }

    #[rstest::rstest]
    #[case(Focus::Sidebar, Selection::Session, Scope::Sidebar)]
    #[case(Focus::Sidebar, Selection::Draft, Scope::SidebarDraft)]
    #[case(Focus::Sidebar, Selection::Nothing, Scope::SidebarEmpty)]
    #[case(Focus::Dashboard, Selection::Session, Scope::Dashboard)]
    #[case(Focus::Dashboard, Selection::Draft, Scope::DashboardDraft)]
    #[case(Focus::Dashboard, Selection::Nothing, Scope::DashboardEmpty)]
    #[case(Focus::Sidebar, Selection::GroupDraft, Scope::SidebarGroupDraft)]
    #[case(Focus::Dashboard, Selection::GroupDraft, Scope::DashboardGroupDraft)]
    #[case(Focus::Sidebar, Selection::OwnFolderSession, Scope::SidebarIncognito)]
    #[case(
        Focus::Sidebar,
        Selection::IncognitoDraft,
        Scope::SidebarIncognitoDraft
    )]
    #[case(
        Focus::Dashboard,
        Selection::OwnFolderSession,
        Scope::DashboardIncognito
    )]
    #[case(
        Focus::Dashboard,
        Selection::IncognitoDraft,
        Scope::DashboardIncognitoDraft
    )]
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

    /// Group `id` of project 1, drafting if `draft`.
    fn group(id: i64, draft: bool) -> Group {
        Group {
            id: GroupId(id),
            kind: GroupKind::Feature,
            name: format!("group-{id}"),
            dir: None,
            branch: None,
            created_at: SystemTime::UNIX_EPOCH,
            pinned_at: None,
            settled_at: None,
            active_since: SystemTime::UNIX_EPOCH,
            draft: draft.then(GroupDraft::default),
            defaults: GroupDefaults {
                harness: HarnessId::new("claude"),
                model: None,
                permission: None,
            },
        }
    }

    /// One project holding group 9 with thread 1, group 10 with a draft, and
    /// group 11 in its worktree, with the cursor on `cursor`.
    fn grouped(cursor: SidebarItem) -> Sessions {
        Sessions {
            projects: vec![Project {
                id: ProjectId(1),
                title: "orb".into(),
                root: "/orb".into(),
                created_at: SystemTime::UNIX_EPOCH,
                removed: false,
                draft: None,
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
                        command: vec![],
                    }),
                    branch: None,
                    pinned_at: None,
                    settled_at: None,
                    active_since: SystemTime::UNIX_EPOCH,
                    created_at: SystemTime::UNIX_EPOCH,
                    last_activity_at: SystemTime::UNIX_EPOCH,
                    unseen: false,
                    group: Some(GroupId(9)),
                    model: None,
                    permission: None,
                }],
                groups: vec![
                    group(9, false),
                    group(10, true),
                    Group {
                        dir: Some("/wt/orb-1a2b3c4d".into()),
                        ..group(11, false)
                    },
                ],
                kind: ProjectKind::Normal,
            }],
            cursor: Some(cursor),
            ..Sessions::default()
        }
    }

    /// `grouped(cursor)` as a project of `kind` with no groups: thread 1
    /// outside any group, and a non-git draft.
    fn lone(kind: ProjectKind, cursor: SidebarItem) -> Sessions {
        let mut sessions = grouped(cursor);
        sessions.projects.iter_mut().for_each(|project| {
            project.kind = kind;
            project.groups.clear();
            project
                .threads
                .iter_mut()
                .for_each(|thread| thread.group = None);
            project.draft = Some(Draft {
                harness: HarnessId::new("claude"),
                workspace: DraftWorkspace::Local,
                branch: None,
                model: None,
                permission: None,
                created_at: SystemTime::UNIX_EPOCH,
                repo: false,
                from: None,
            });
        });
        sessions.sessions = sessions_for(&sessions.projects);
        sessions
    }

    #[rstest::rstest]
    #[case::normal_thread(
        ProjectKind::Normal,
        SidebarItem::Session(SessionId(1)),
        Selection::Session
    )]
    #[case::normal_draft(
        ProjectKind::Normal,
        SidebarItem::Draft(ProjectId(1)),
        Selection::Draft
    )]
    #[case::incognito_thread(
        ProjectKind::Incognito,
        SidebarItem::Session(SessionId(1)),
        Selection::OwnFolderSession
    )]
    #[case::incognito_draft(
        ProjectKind::Incognito,
        SidebarItem::Draft(ProjectId(1)),
        Selection::IncognitoDraft
    )]
    fn selection_follows_the_lone_rows_project_kind(
        #[case] kind: ProjectKind,
        #[case] cursor: SidebarItem,
        #[case] expected: Selection,
    ) {
        // Given the cursor on a thread or draft outside any group.
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
    #[case(Scope::SidebarGroupDraft, KeyCode::Char('d'), Intent::Delete)]
    #[case(Scope::SidebarGroupDraft, KeyCode::Char('j'), Intent::SelectNext)]
    #[case(Scope::SidebarGroupDraft, KeyCode::Enter, Intent::Attach)]
    fn group_scope_keys_map_to_their_intents(
        #[case] scope: Scope,
        #[case] code: KeyCode,
        #[case] expected: Intent,
    ) {
        // Given the keymap on a group's row in the sidebar.
        let mut keys = Keys::new(keymap(), scope.into());

        // When pressing the key.
        let intent = press(&mut keys, key(code));

        // Then it yields its intent.
        assert_eq!(intent.as_ref(), Some(&expected), "{code:?} in {scope:?}");
    }

    #[rstest::rstest]
    fn n_is_unbound_off_a_group(
        #[values(
            Scope::Sidebar,
            Scope::SidebarDraft,
            Scope::SidebarEmpty,
            Scope::SidebarGroupDraft
        )]
        scope: Scope,
    ) {
        // Given the keymap off a group's card or thread.
        let mut keys = Keys::new(keymap(), scope.into());

        // When pressing `n`.
        let intent = press(&mut keys, key(KeyCode::Char('n')));

        // Then nothing happens.
        assert_eq!(intent, None, "n in {scope:?}");
    }

    /// The keys the leader popup lists in `scope`.
    fn leader_popup(scope: Scope) -> Vec<KeyEvent> {
        keymap()
            .get_children_at_path(&[key(KeyCode::Char(' '))], &scope.into())
            .unwrap_or_default()
            .into_iter()
            .map(|(key, _)| key)
            .collect()
    }

    /// Every key sequence bound in `scope` with its intent, sorted by sequence.
    fn bindings(scope: Scope) -> Vec<(Vec<KeyEvent>, Intent)> {
        let keymap = keymap();
        let mut paths: Vec<Vec<KeyEvent>> = keymap
            .get_children_at_path(&[], &scope.into())
            .unwrap_or_default()
            .into_iter()
            .map(|(key, _)| vec![key])
            .collect();
        let mut found = vec![];
        while let Some(path) = paths.pop() {
            match keymap.navigate(&path, &scope.into()) {
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
    fn leader_w_and_b_are_unbound_on_incognito_rows(
        #[values(
            Scope::SidebarIncognito,
            Scope::SidebarIncognitoDraft,
            Scope::DashboardIncognito,
            Scope::DashboardIncognitoDraft
        )]
        scope: Scope,
        #[values('w', 'b')] pressed: char,
    ) {
        // Given the leader popup's keys on an Incognito row.
        let keys = leader_popup(scope);

        // When looking for the key.
        let found = keys.contains(&key(KeyCode::Char(pressed)));

        // Then it isn't listed.
        assert!(!found, "{pressed} in the {scope:?} leader popup");
    }

    #[rstest::rstest]
    fn w_and_b_are_unbound_on_an_incognito_dashboard(
        #[values(Scope::DashboardIncognito, Scope::DashboardIncognitoDraft)] scope: Scope,
        #[values('w', 'b')] pressed: char,
    ) {
        // Given the keymap on the dashboard of an Incognito row.
        let mut keys = Keys::new(keymap(), scope.into());

        // When pressing the key.
        let intent = press(&mut keys, key(KeyCode::Char(pressed)));

        // Then nothing happens.
        assert_eq!(intent, None, "{pressed} in {scope:?}");
    }

    #[rstest::rstest]
    #[case(Scope::Sidebar, Scope::SidebarIncognito)]
    #[case(Scope::SidebarDraft, Scope::SidebarIncognitoDraft)]
    #[case(Scope::Dashboard, Scope::DashboardIncognito)]
    #[case(Scope::DashboardDraft, Scope::DashboardIncognitoDraft)]
    fn incognito_scope_binds_its_base_scopes_keys_but_w_and_b(
        #[case] base: Scope,
        #[case] incognito: Scope,
    ) {
        // Given the bindings of the base scope, less ␣w, ␣b, w and b.
        let expected: Vec<(Vec<KeyEvent>, Intent)> = {
            let unbound = [[' ', 'w'].as_slice(), &[' ', 'b'], &['w'], &['b']].map(|keys| {
                keys.iter()
                    .map(|&c| key(KeyCode::Char(c)))
                    .collect::<Vec<_>>()
            });
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
    #[case(Scope::Sidebar, false)]
    #[case(Scope::Dashboard, false)]
    #[case(Scope::SidebarEmpty, false)]
    #[case(Scope::DashboardEmpty, false)]
    #[case(Scope::SidebarDraft, true)]
    #[case(Scope::DashboardDraft, true)]
    fn leader_popup_lists_model_and_permission_only_on_a_draft(
        #[case] scope: Scope,
        #[case] listed: bool,
    ) {
        // Given the leader popup's keys in the scope.
        let keys = leader_popup(scope);

        // When looking for `m` and `a`.
        let found = [
            keys.contains(&key(KeyCode::Char('m'))),
            keys.contains(&key(KeyCode::Char('a'))),
        ];

        // Then both are listed exactly when a draft is selected.
        assert_eq!(found, [listed; 2], "m/a in the {scope:?} leader popup");
    }

    #[rstest::rstest]
    fn leader_h_picks_the_harness(
        #[values(
            Scope::SidebarDraft,
            Scope::DashboardDraft,
            Scope::SidebarGroupDraft,
            Scope::DashboardGroupDraft,
            Scope::SidebarIncognitoDraft,
            Scope::DashboardIncognitoDraft
        )]
        scope: Scope,
    ) {
        // Given Space already pressed where ␣m is bound.
        let mut keys = Keys::new(keymap(), scope.into());
        press(&mut keys, key(KeyCode::Char(' ')));

        // When pressing `h`.
        let intent = press(&mut keys, key(KeyCode::Char('h')));

        // Then it opens the harness picker.
        assert_eq!(intent, Some(Intent::PickHarness), "Space h in {scope:?}");
    }

    /// `scope` where the selection's harness lists no permission modes.
    fn without_permissions(scope: Scope) -> KeyScope {
        KeyScope {
            scope,
            permissions: false,
        }
    }

    #[rstest::rstest]
    fn leader_a_isnt_bound_without_permission_modes() {
        // Given the leader popup on a draft whose harness has no permission
        // modes.
        let keys = keymap()
            .get_children_at_path(
                &[key(KeyCode::Char(' '))],
                &without_permissions(Scope::SidebarDraft),
            )
            .unwrap_or_default();

        // When looking for `a`.
        let found = keys
            .iter()
            .any(|(key_event, _)| *key_event == key(KeyCode::Char('a')));

        // Then it isn't listed.
        assert!(!found, "␣a should be unbound without permission modes");
    }

    #[rstest::rstest]
    fn dashboard_a_isnt_bound_without_permission_modes() {
        // Given a draft's dashboard whose harness has no permission modes.
        let mut keys = Keys::new(keymap(), without_permissions(Scope::DashboardDraft));

        // When pressing `a`.
        let intent = press(&mut keys, key(KeyCode::Char('a')));

        // Then nothing happens.
        assert_eq!(intent, None, "a should be unbound without permission modes");
    }

    #[rstest::rstest]
    fn leader_h_stays_bound_without_permission_modes() {
        // Given Space already pressed on a draft whose harness has no
        // permission modes.
        let mut keys = Keys::new(keymap(), without_permissions(Scope::SidebarDraft));
        press(&mut keys, key(KeyCode::Char(' ')));

        // When pressing `h`.
        let intent = press(&mut keys, key(KeyCode::Char('h')));

        // Then it still opens the harness picker.
        assert_eq!(
            intent,
            Some(Intent::PickHarness),
            "␣h should stay bound without permission modes"
        );
    }

    #[rstest::rstest]
    #[case(Scope::SidebarEmpty, false)]
    #[case(Scope::DashboardEmpty, false)]
    #[case(Scope::Sidebar, true)]
    #[case(Scope::Dashboard, true)]
    #[case(Scope::SidebarDraft, true)]
    #[case(Scope::DashboardDraft, true)]
    fn leader_popup_lists_workspace_and_branch_only_with_a_selection(
        #[case] scope: Scope,
        #[case] listed: bool,
    ) {
        // Given the leader popup's keys in the scope.
        let keys = leader_popup(scope);

        // When looking for `w` and `b`.
        let found = [
            keys.contains(&key(KeyCode::Char('w'))),
            keys.contains(&key(KeyCode::Char('b'))),
        ];

        // Then both are listed exactly when a thread or draft is selected.
        assert_eq!(found, [listed; 2], "w/b in the {scope:?} leader popup");
    }

    #[rstest::rstest]
    fn leader_keys_open_tools(
        #[values(
            Scope::Sidebar,
            Scope::SidebarDraft,
            Scope::Dashboard,
            Scope::DashboardDraft
        )]
        scope: Scope,
        #[values(('t', Tool::Shell), ('v', Tool::Nvim))] binding: (char, Tool),
    ) {
        // Given Space already pressed.
        let (pressed, tool) = binding;
        let mut keys = Keys::new(keymap(), scope.into());
        press(&mut keys, key(KeyCode::Char(' ')));

        // When pressing the tool's key.
        let intent = press(&mut keys, key(KeyCode::Char(pressed)));

        // Then it opens that tool.
        assert_eq!(
            intent,
            Some(Intent::OpenTool(tool)),
            "Space {pressed} in {scope:?}"
        );
    }

    #[rstest::rstest]
    #[case(Scope::SidebarEmpty, false)]
    #[case(Scope::DashboardEmpty, false)]
    #[case(Scope::Sidebar, true)]
    #[case(Scope::Dashboard, true)]
    #[case(Scope::SidebarDraft, true)]
    #[case(Scope::DashboardDraft, true)]
    fn leader_popup_lists_tools_only_with_a_selection(#[case] scope: Scope, #[case] listed: bool) {
        // Given the leader popup's keys in the scope.
        let keys = leader_popup(scope);

        // When looking for `t` and `v`.
        let found = ['t', 'v'].map(|c| keys.contains(&key(KeyCode::Char(c))));

        // Then both are listed exactly when a thread or draft is selected.
        assert_eq!(found, [listed; 2], "t/v in the {scope:?} leader popup");
    }

    #[rstest::rstest]
    fn leader_gg_opens_lazygit(
        #[values(
            Scope::Sidebar,
            Scope::SidebarDraft,
            Scope::Dashboard,
            Scope::DashboardDraft,
            Scope::SidebarGroupDraft,
            Scope::DashboardGroupDraft
        )]
        scope: Scope,
    ) {
        // Given Space and `g` already pressed.
        let mut keys = Keys::new(keymap(), scope.into());
        press(&mut keys, key(KeyCode::Char(' ')));
        press(&mut keys, key(KeyCode::Char('g')));

        // When pressing `g` again.
        let intent = press(&mut keys, key(KeyCode::Char('g')));

        // Then it opens lazygit.
        assert_eq!(
            intent,
            Some(Intent::OpenTool(Tool::Lazygit)),
            "Space g g in {scope:?}"
        );
    }

    #[rstest::rstest]
    fn leader_g_new_group_keys_yield_their_kind(
        #[values(Scope::SidebarEmpty)] scope: Scope,
        #[values(
            ('f', GroupKind::Feature),
            ('r', GroupKind::Research),
            ('l', GroupKind::Learn)
        )]
        binding: (char, GroupKind),
    ) {
        // Given Space and `g` already pressed.
        let (pressed, kind) = binding;
        let mut keys = Keys::new(keymap(), scope.into());
        press(&mut keys, key(KeyCode::Char(' ')));
        press(&mut keys, key(KeyCode::Char('g')));

        // When pressing the kind's key.
        let intent = press(&mut keys, key(KeyCode::Char(pressed)));

        // Then it starts a group of that kind.
        assert_eq!(
            intent,
            Some(Intent::NewGroup(kind)),
            "Space g {pressed} in {scope:?}"
        );
    }

    #[rstest::rstest]
    #[case(Scope::SidebarEmpty, &['f', 'l', 'r'])]
    #[case(Scope::DashboardEmpty, &['f', 'l', 'r'])]
    #[case(Scope::Sidebar, &['f', 'g', 'l', 'r'])]
    #[case(Scope::SidebarGroupDraft, &['f', 'g', 'l', 'r'])]
    #[case(Scope::Dashboard, &['f', 'g', 'l', 'r'])]
    fn leader_g_popup_lists_the_scopes_group_keys(#[case] scope: Scope, #[case] expected: &[char]) {
        // Given orb's keymap in the scope.
        let keymap = keymap();

        // When listing the keys under Space g.
        let mut found: Vec<char> = keymap
            .get_children_at_path(
                &[key(KeyCode::Char(' ')), key(KeyCode::Char('g'))],
                &scope.into(),
            )
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(key, _)| match key.code {
                KeyCode::Char(c) => Some(c),
                _ => None,
            })
            .collect();
        found.sort_unstable();

        // Then they are the scope's group keys.
        assert_eq!(found, expected, "Space g keys in {scope:?}");
    }

    #[rstest::rstest]
    #[case(Scope::Sidebar, " befginpstvw")]
    #[case(Scope::SidebarDraft, " abefghimnpstvw")]
    #[case(Scope::SidebarEmpty, " efginps")]
    #[case(Scope::SidebarGroupDraft, " aefghimnpstv")]
    #[case(Scope::Dashboard, " beginpstvw")]
    #[case(Scope::DashboardDraft, " abeghimnpstvw")]
    #[case(Scope::DashboardEmpty, " eginps")]
    #[case(Scope::DashboardGroupDraft, " aeghimnpstv")]
    #[case(Scope::SidebarIncognito, " efginpstv")]
    #[case(Scope::SidebarIncognitoDraft, " aefghimnpstv")]
    #[case(Scope::DashboardIncognito, " eginpstv")]
    #[case(Scope::DashboardIncognitoDraft, " aeghimnpstv")]
    fn leader_popup_matches_the_scope_table(#[case] scope: Scope, #[case] expected: &str) {
        // Given orb's keymap in the scope.
        let popup = leader_popup(scope);

        // When listing the leader popup's keys, sorted.
        let found: String = {
            let mut chars: Vec<char> = popup
                .into_iter()
                .filter_map(|key| match key.code {
                    KeyCode::Char(c) => Some(c),
                    _ => None,
                })
                .collect();
            chars.sort_unstable();
            chars.into_iter().collect()
        };

        // Then they are the scope's row of the spec's scope table.
        assert_eq!(found, expected, "Space keys in {scope:?}");
    }

    #[rstest::rstest]
    #[case(vec![key(KeyCode::Char('p'))])]
    #[case(vec![key(KeyCode::Char('s'))])]
    #[case(vec![key(KeyCode::Char('r'))])]
    fn thread_keys_are_unbound_on_a_draft_in_the_sidebar(#[case] pressed: Vec<KeyEvent>) {
        // Given the keymap in the sidebar on a draft.
        let mut keys = Keys::new(keymap(), Scope::SidebarDraft.into());

        // When pressing `p`, `s` or `r`.
        let intents: Vec<Option<Intent>> = pressed
            .into_iter()
            .map(|pressed| press(&mut keys, pressed))
            .collect();

        // Then nothing happens.
        assert!(
            intents.iter().all(Option::is_none),
            "a thread key did something on a draft: {intents:?}"
        );
    }

    #[rstest::rstest]
    fn r_is_unbound_on_the_settled_header_or_no_row() {
        // Given the keymap in the sidebar with no thread or draft selected.
        let mut keys = Keys::new(keymap(), Scope::SidebarEmpty.into());

        // When pressing `r`.
        let intent = press(&mut keys, key(KeyCode::Char('r')));

        // Then nothing happens.
        assert_eq!(intent, None, "only a thread can be renamed");
    }

    #[rstest::rstest]
    fn enter_on_a_draft_in_the_sidebar_attaches() {
        // Given the keymap in the sidebar on a draft.
        let mut keys = Keys::new(keymap(), Scope::SidebarDraft.into());

        // When pressing Enter.
        let intent = press(&mut keys, key(KeyCode::Enter));

        // Then it yields Attach, which starts the draft.
        assert_eq!(
            intent,
            Some(Intent::Attach),
            "Enter on a draft in the sidebar should start it"
        );
    }

    #[rstest::rstest]
    #[case(vec![ctrl('h')], Intent::FocusSidebar)]
    #[case(vec![key(KeyCode::Char(' ')), key(KeyCode::Char('n'))], Intent::NewSession)]
    #[case(vec![key(KeyCode::Char(' ')), key(KeyCode::Char('p'))], Intent::AddProject)]
    #[case(vec![key(KeyCode::Char(' ')), key(KeyCode::Char('w'))], Intent::ChangeWorkspace)]
    #[case(vec![key(KeyCode::Char(' ')), key(KeyCode::Char('b'))], Intent::SwitchBranch)]
    #[case(vec![key(KeyCode::Char(' ')), key(KeyCode::Char('m'))], Intent::PickModel)]
    #[case(vec![key(KeyCode::Char(' ')), key(KeyCode::Char('a'))], Intent::PickPermission)]
    fn dashboard_draft_keys_map_to_their_intents(
        #[case] pressed: Vec<KeyEvent>,
        #[case] expected: Intent,
    ) {
        // Given the keymap on a draft's dashboard.
        let mut keys = Keys::new(keymap(), Scope::DashboardDraft.into());

        // When pressing the keys in order.
        let intent = pressed
            .into_iter()
            .map(|pressed| press(&mut keys, pressed))
            .last()
            .flatten();

        // Then it yields its intent.
        assert_eq!(
            intent.as_ref(),
            Some(&expected),
            "the key for {expected} on a draft's dashboard"
        );
    }

    #[rstest::rstest]
    #[case('s', Intent::ToggleSettle)]
    #[case('d', Intent::Delete)]
    fn sidebar_thread_key_yields_its_intent(#[case] c: char, #[case] expected: Intent) {
        // Given the keymap in Sidebar focus.
        let mut keys = Keys::new(keymap(), Scope::Sidebar.into());

        // When pressing the key once.
        let intent = press(&mut keys, key(KeyCode::Char(c)));

        // Then it yields its intent.
        assert_eq!(intent, Some(expected), "{c} in the sidebar");
    }

    #[rstest::rstest]
    fn d_on_a_draft_in_the_sidebar_deletes() {
        // Given the keymap in the sidebar on a draft.
        let mut keys = Keys::new(keymap(), Scope::SidebarDraft.into());

        // When pressing `d`.
        let intent = press(&mut keys, key(KeyCode::Char('d')));

        // Then it asks to discard the draft.
        assert_eq!(
            intent,
            Some(Intent::Delete),
            "d on a draft should ask to discard it"
        );
    }

    #[rstest::rstest]
    fn held_j_selects_the_next_thread() {
        // Given a held `j` as the kitty protocol reports it.
        let mut keys = Keys::new(keymap(), Scope::Sidebar.into());
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

    #[rstest::rstest]
    #[case(KeyCode::Char('\\'), KeyModifiers::CONTROL)]
    #[case(KeyCode::Char('4'), KeyModifiers::CONTROL)]
    fn ctrl_backslash_detaches_while_attached(
        #[case] code: KeyCode,
        #[case] modifiers: KeyModifiers,
    ) {
        // Given `<C-\>` in one of its two forms.
        let key = KeyEvent::new(code, modifiers);

        // When routing it while attached.
        let routed = attached_route(key);

        // Then it detaches.
        assert_eq!(
            routed,
            Route::Intent(Intent::Detach),
            "{code} with {modifiers} should detach"
        );
    }

    #[rstest::rstest]
    fn ctrl_h_leaves_the_pane_while_attached() {
        // Given `<C-h>`.
        let key = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL);

        // When routing it while attached.
        let routed = attached_route(key);

        // Then it leaves the pane for the sidebar instead of reaching Claude.
        assert_eq!(
            routed,
            Route::Intent(Intent::LeavePane),
            "<C-h> should leave the pane"
        );
    }

    #[rstest::rstest]
    fn ctrl_b_toggles_the_sidebar_while_attached() {
        // Given `<C-b>`.
        let key = ctrl('b');

        // When routing it while attached.
        let routed = attached_route(key);

        // Then it hides or shows the sidebar instead of reaching Claude.
        assert_eq!(
            routed,
            Route::Intent(Intent::ToggleSidebar),
            "<C-b> should toggle the sidebar while attached"
        );
    }

    #[rstest::rstest]
    fn ctrl_space_opens_the_session_picker_while_attached() {
        // Given `<C-Space>`, the form both kitty's `CSI 32;5u` and the legacy
        // NUL byte parse to.
        let key = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL);

        // When routing it while attached.
        let routed = attached_route(key);

        // Then it opens the session picker instead of reaching Claude.
        assert_eq!(
            routed,
            Route::Intent(Intent::OpenSessionPicker),
            "<C-Space> should open the session picker while attached"
        );
    }

    #[rstest::rstest]
    #[case('b')]
    #[case('B')]
    fn ctrl_shift_b_is_forwarded_while_attached(#[case] c: char) {
        // Given `ctrl+shift+b` in either of its kitty forms.
        let key = KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );

        // When routing it while attached.
        let routed = attached_route(key);

        // Then it goes to Claude.
        assert_eq!(routed, Route::Forward, "ctrl+shift+{c} should be forwarded");
    }

    #[rstest::rstest]
    #[case(KeyCode::Right, Intent::WidenFocused)]
    #[case(KeyCode::Left, Intent::NarrowFocused)]
    fn ctrl_arrows_resize_while_attached(#[case] code: KeyCode, #[case] expected: Intent) {
        // Given a Ctrl-modified arrow.
        let key = KeyEvent::new(code, KeyModifiers::CONTROL);

        // When routing it while attached.
        let routed = attached_route(key);

        // Then it resizes instead of reaching Claude.
        assert_eq!(
            routed,
            Route::Intent(expected),
            "<C-{code}> should resize while attached"
        );
    }

    #[rstest::rstest]
    #[case('o', Intent::JumpBack)]
    #[case('i', Intent::JumpForward)]
    fn ctrl_o_and_ctrl_i_jump_while_attached(#[case] c: char, #[case] expected: Intent) {
        // Given `<C-o>` or `<C-i>`.
        let key = ctrl(c);

        // When routing it while attached.
        let routed = attached_route(key);

        // Then it moves through the jump list instead of reaching Claude.
        assert_eq!(
            routed,
            Route::Intent(expected),
            "<C-{c}> should jump while attached"
        );
    }

    #[rstest::rstest]
    #[case(KeyCode::Tab, KeyModifiers::NONE)]
    #[case(KeyCode::Char('o'), KeyModifiers::CONTROL | KeyModifiers::SHIFT)]
    #[case(KeyCode::Char('O'), KeyModifiers::CONTROL | KeyModifiers::SHIFT)]
    fn tab_and_ctrl_shift_o_are_forwarded_while_attached(
        #[case] code: KeyCode,
        #[case] modifiers: KeyModifiers,
    ) {
        // Given Tab or `ctrl+shift+o` in either of its kitty forms.
        let key = KeyEvent::new(code, modifiers);

        // When routing it while attached.
        let routed = attached_route(key);

        // Then it goes to Claude.
        assert_eq!(
            routed,
            Route::Forward,
            "{code} with {modifiers} should be forwarded"
        );
    }

    #[rstest::rstest]
    #[case('o', Intent::JumpBack)]
    #[case('i', Intent::JumpForward)]
    fn ctrl_o_and_ctrl_i_jump_in_the_sidebar_and_dashboard(
        #[case] c: char,
        #[case] expected: Intent,
    ) {
        // Given `<C-o>` or `<C-i>`.
        let pressed = ctrl(c);

        // When routing it in the sidebar or dashboard.
        let intent = jump_route(pressed);

        // Then it moves through the jump list.
        assert_eq!(intent, Some(expected), "<C-{c}> should jump");
    }

    #[rstest::rstest]
    #[case(key(KeyCode::Tab))]
    #[case(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL | KeyModifiers::SHIFT))]
    #[case(KeyEvent::new(KeyCode::Char('O'), KeyModifiers::CONTROL | KeyModifiers::SHIFT))]
    #[case(key(KeyCode::Char('o')))]
    #[case(key(KeyCode::Char('i')))]
    fn other_keys_do_not_jump(#[case] pressed: KeyEvent) {
        // Given a key other than `<C-o>` and `<C-i>`.

        // When routing it in the sidebar or dashboard.
        let intent = jump_route(pressed);

        // Then it's left to the keymap.
        assert_eq!(intent, None, "{pressed:?} shouldn't jump");
    }

    #[rstest::rstest]
    #[case(KeyCode::Char('q'), KeyModifiers::NONE)]
    #[case(KeyCode::Enter, KeyModifiers::NONE)]
    #[case(KeyCode::Esc, KeyModifiers::NONE)]
    #[case(KeyCode::Char('a'), KeyModifiers::NONE)]
    #[case(KeyCode::Char(' '), KeyModifiers::NONE)]
    #[case(KeyCode::Backspace, KeyModifiers::NONE)]
    fn keys_are_forwarded_while_attached(#[case] code: KeyCode, #[case] modifiers: KeyModifiers) {
        // Given a key other than `<C-\>` and `<C-h>`.
        let key = KeyEvent::new(code, modifiers);

        // When routing it while attached.
        let routed = attached_route(key);

        // Then it goes to Claude.
        assert_eq!(
            routed,
            Route::Forward,
            "{code} with {modifiers} should be forwarded"
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
    #[case(KeyCode::Char('n'), Intent::SplitPane(Split::Right))]
    #[case(KeyCode::Char('x'), Intent::ClosePane)]
    #[case(KeyCode::Char('+'), Intent::GrowFocused)]
    #[case(KeyCode::Char('='), Intent::GrowFocused)]
    #[case(KeyCode::Char('-'), Intent::ShrinkFocused)]
    #[case(KeyCode::Char('1'), Intent::GoToTab(1))]
    #[case(KeyCode::Char('2'), Intent::GoToTab(2))]
    #[case(KeyCode::Char('3'), Intent::GoToTab(3))]
    #[case(KeyCode::Char('4'), Intent::GoToTab(4))]
    #[case(KeyCode::Char('5'), Intent::GoToTab(5))]
    #[case(KeyCode::Char('['), Intent::PreviousTab)]
    #[case(KeyCode::Char(']'), Intent::NextTab)]
    #[case(KeyCode::Char('i'), Intent::MoveTabLeft)]
    #[case(KeyCode::Char('o'), Intent::MoveTabRight)]
    fn cmd_keys_route_to_their_intents(#[case] code: KeyCode, #[case] expected: Intent) {
        // Given / When routing Cmd with `code`.
        let intent = cmd_route(cmd(code));

        // Then it asks for its intent.
        assert_eq!(intent, Some(expected), "Cmd {code} should route");
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
    fn cmd_f_is_unbound() {
        // Given Cmd f, reserved for floating panes.
        let pressed = cmd(KeyCode::Char('f'));

        // When routing it.
        let intent = cmd_route(pressed);

        // Then nothing is asked for.
        assert_eq!(intent, None, "Cmd f stays unbound");
    }
}
