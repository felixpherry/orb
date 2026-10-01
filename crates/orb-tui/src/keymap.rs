//! Key routing: which keys do what in each focus. Only keys the user has
//! defined are bound.
//!
//! In the sidebar and the dashboard, keys go through a which-key keymap whose
//! scope is the focus and what the sidebar cursor is on; `<Space>` is the
//! leader and shows a popup. A key that does nothing for the selection isn't
//! bound there (`␣m`/`␣a` on a thread, `p`/`s` off a lone thread or a
//! group's card, `r` off a thread, `␣w`/`␣b`
//! and the tool keys `␣t`/`␣gg`/`␣v` with nothing selected, and the
//! dashboard's `m`/`a` off a draft or a group's card and
//! `o`/`w`/`b`/`t`/`g`/`v` with nothing selected), so the popups don't offer
//! it. On the dashboard each menu item's
//! letter runs it, `j`/`k` or `↓`/`↑` move the menu cursor, and `⏎` runs the
//! highlighted item. `<C-Right>`/`<C-Left>` resize the focused side
//! outside which-key, which can't name them, and `<C-o>`/`<C-i>` move back
//! and forward through the jump list, also outside which-key. In the
//! sidebar, `<C-\>` detaches the selected attached thread, outside which-key
//! too. While attached, every key goes to Claude except `<C-\>`, `<C-h>`,
//! the jump keys, and the resize keys, which resize the pane as they do the
//! dashboard. An open picker takes
//! typed characters as filter text and has its own fixed keys, `<C-x>` among
//! them for removing a project from the project filter. The rename box (`r`)
//! and the sidebar search (`/` or `i`) use the picker's keys. `␣i` starts an
//! incognito session in every scope; on the dashboard, `i` does too. On a
//! group's card, draft or threads, `l`/`h` open and close the group. `␣w` isn't bound
//! on them, and `␣b` (the dashboard's `b` too) only on the card of a Feature
//! group whose worktree exists, where it switches that worktree's branch.
//! `␣m`/`␣a` on a card pick the group's default model and permission (its
//! draft's and each sibling's). `n` on a group's card or thread starts a
//! sibling; `d` on a card deletes the group.
//! `␣gf`/`␣gr`/`␣gl` add a Feature, Research or Learn group in every scope.

use std::fmt;

use orb_domain::feat::dashboard::DashboardItem::{
    AddProject, Branch, FilterProjects, Incognito, Lazygit, Model, Neovim, NewSession, Open,
    Permission, Quit, Shell, Start, Workspace,
};
use orb_domain::feat::sessions::state::{GroupKind, Sessions};
use orb_domain::feat::zellij::zellij_service::Tool;
use orb_domain::{Focus, Intent};
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
    Thread,
    Draft,
    /// A group's card with no worktree: a Research or Learn group, or a
    /// Feature group before its draft starts.
    GroupCard,
    /// A Feature group's card once its worktree exists.
    WorktreeCard,
    /// A thread in a group.
    GroupThread,
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
            sessions.selected_thread(),
            sessions.selected_group(),
        ) {
            (Some(_), _, _, _) => Self::Draft,
            (None, Some(_), _, _) => Self::GroupDraft,
            (None, None, Some(_), Some(_)) => Self::GroupThread,
            (None, None, Some(_), None) => Self::Thread,
            (None, None, None, Some((_, group)))
                if group.kind == GroupKind::Feature && group.dir.is_some() =>
            {
                Self::WorktreeCard
            }
            (None, None, None, Some(_)) => Self::GroupCard,
            (None, None, None, None) => Self::Nothing,
        }
    }
}

/// Which bindings apply: the focused side's, less the keys that do nothing
/// for the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Scope {
    /// The sidebar on a thread.
    Sidebar,
    /// The sidebar on a draft: no pin or settle, but its setting pickers.
    SidebarDraft,
    /// The sidebar with no thread or draft selected.
    SidebarEmpty,
    /// The sidebar on a group's card: fold keys, pin, settle, delete and a
    /// sibling, but no rename.
    SidebarGroup,
    /// The sidebar on a started Feature group's card: [`Scope::SidebarGroup`]'s
    /// keys and its worktree's branch.
    SidebarWorktreeGroup,
    /// The sidebar on a thread in a group: fold keys, rename, delete and a
    /// sibling; no pin or settle.
    SidebarGroupThread,
    /// The sidebar on a group's draft: fold keys, its setting pickers, and
    /// `d`, which is refused.
    SidebarGroupDraft,
    /// The dashboard on a thread.
    Dashboard,
    /// The dashboard on a draft: its setting pickers too.
    DashboardDraft,
    /// The dashboard with no thread or draft selected.
    DashboardEmpty,
    /// The dashboard on a group's card: the tools only.
    DashboardGroup,
    /// The dashboard on a started Feature group's card: its worktree's branch
    /// and the tools.
    DashboardWorktreeGroup,
    /// The dashboard on a thread in a group: no workspace or branch.
    DashboardGroupThread,
    /// The dashboard on a group's draft: its setting pickers too.
    DashboardGroupDraft,
}

impl Scope {
    /// The scope for keys in `focus` with `selection`.
    pub(crate) fn new(focus: Focus, selection: Selection) -> Self {
        match (focus, selection) {
            (Focus::Dashboard, Selection::Thread) => Self::Dashboard,
            (Focus::Dashboard, Selection::Draft) => Self::DashboardDraft,
            (Focus::Dashboard, Selection::Nothing) => Self::DashboardEmpty,
            (Focus::Dashboard, Selection::GroupCard) => Self::DashboardGroup,
            (Focus::Dashboard, Selection::WorktreeCard) => Self::DashboardWorktreeGroup,
            (Focus::Dashboard, Selection::GroupThread) => Self::DashboardGroupThread,
            (Focus::Dashboard, Selection::GroupDraft) => Self::DashboardGroupDraft,
            (
                Focus::Sidebar | Focus::Attached | Focus::Picker | Focus::Rename | Focus::Search,
                Selection::Thread,
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
                Selection::GroupCard,
            ) => Self::SidebarGroup,
            (
                Focus::Sidebar | Focus::Attached | Focus::Picker | Focus::Rename | Focus::Search,
                Selection::WorktreeCard,
            ) => Self::SidebarWorktreeGroup,
            (
                Focus::Sidebar | Focus::Attached | Focus::Picker | Focus::Rename | Focus::Search,
                Selection::GroupThread,
            ) => Self::SidebarGroupThread,
            (
                Focus::Sidebar | Focus::Attached | Focus::Picker | Focus::Rename | Focus::Search,
                Selection::GroupDraft,
            ) => Self::SidebarGroupDraft,
        }
    }
}

/// The keymap with its current scope and pending key sequence.
pub(crate) type Keys = WhichKeyState<KeyEvent, Scope, Intent, KeyCategory>;

/// The sidebar and dashboard bindings, scoped by focus and selection.
#[expect(
    clippy::too_many_lines,
    reason = "one binding per key keeps the whole keymap in one place"
)]
pub(crate) fn keymap() -> Keymap<KeyEvent, Scope, Intent, KeyCategory> {
    const SIDEBAR_GROUPS: [Scope; 4] = [
        Scope::SidebarGroup,
        Scope::SidebarWorktreeGroup,
        Scope::SidebarGroupThread,
        Scope::SidebarGroupDraft,
    ];
    const DASHBOARD_GROUPS: [Scope; 4] = [
        Scope::DashboardGroup,
        Scope::DashboardWorktreeGroup,
        Scope::DashboardGroupThread,
        Scope::DashboardGroupDraft,
    ];
    const SIDEBAR: [Scope; 7] = [
        Scope::Sidebar,
        Scope::SidebarDraft,
        Scope::SidebarEmpty,
        Scope::SidebarGroup,
        Scope::SidebarWorktreeGroup,
        Scope::SidebarGroupThread,
        Scope::SidebarGroupDraft,
    ];
    const DASHBOARD: [Scope; 7] = [
        Scope::Dashboard,
        Scope::DashboardDraft,
        Scope::DashboardEmpty,
        Scope::DashboardGroup,
        Scope::DashboardWorktreeGroup,
        Scope::DashboardGroupThread,
        Scope::DashboardGroupDraft,
    ];
    let mut keymap = Keymap::new();
    keymap.describe_group("<leader>", "leader");
    keymap.describe_group("<leader>g", "group");
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
    for scope in [Scope::Sidebar, Scope::SidebarDraft, Scope::SidebarEmpty] {
        keymap
            .bind("l", Intent::OpenShelf, KeyCategory::Navigation, scope)
            .bind("h", Intent::CloseShelf, KeyCategory::Navigation, scope);
    }
    for scope in SIDEBAR_GROUPS {
        keymap
            .bind("l", Intent::OpenGroup, KeyCategory::Navigation, scope)
            .bind("h", Intent::CloseGroup, KeyCategory::Navigation, scope);
    }
    for scope in [Scope::Sidebar, Scope::SidebarGroupThread] {
        keymap.bind("r", Intent::Rename, KeyCategory::Threads, scope);
    }
    for scope in [
        Scope::Sidebar,
        Scope::SidebarGroup,
        Scope::SidebarWorktreeGroup,
    ] {
        keymap
            .bind("p", Intent::TogglePin, KeyCategory::Threads, scope)
            .bind("s", Intent::ToggleSettle, KeyCategory::Threads, scope);
    }
    for scope in [
        Scope::SidebarGroup,
        Scope::SidebarWorktreeGroup,
        Scope::SidebarGroupThread,
    ] {
        keymap.bind("n", Intent::NewSibling, KeyCategory::Sessions, scope);
    }
    for scope in [Scope::Sidebar, Scope::SidebarDraft]
        .into_iter()
        .chain(SIDEBAR_GROUPS)
    {
        keymap.bind("d", Intent::DeleteThread, KeyCategory::Threads, scope);
    }
    for scope in DASHBOARD {
        keymap
            .bind(
                "<c-h>",
                Intent::FocusSidebar,
                KeyCategory::Navigation,
                scope,
            )
            .bind("j", Intent::DashboardNext, KeyCategory::Navigation, scope)
            .bind(
                "<down>",
                Intent::DashboardNext,
                KeyCategory::Navigation,
                scope,
            )
            .bind("k", Intent::DashboardPrev, KeyCategory::Navigation, scope)
            .bind(
                "<up>",
                Intent::DashboardPrev,
                KeyCategory::Navigation,
                scope,
            )
            .bind(
                "<enter>",
                Intent::DashboardRun,
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
    for (scope, items) in [
        (
            Scope::Dashboard,
            &[Open, Workspace, Branch, Shell, Lazygit, Neovim][..],
        ),
        (
            Scope::DashboardDraft,
            &[
                Start, Workspace, Branch, Model, Permission, Shell, Lazygit, Neovim,
            ],
        ),
        (Scope::DashboardEmpty, &[]),
        (
            Scope::DashboardGroup,
            &[Model, Permission, Shell, Lazygit, Neovim],
        ),
        (
            Scope::DashboardWorktreeGroup,
            &[Branch, Model, Permission, Shell, Lazygit, Neovim],
        ),
        (Scope::DashboardGroupThread, &[Open, Shell, Lazygit, Neovim]),
        (
            Scope::DashboardGroupDraft,
            &[Start, Model, Permission, Shell, Lazygit, Neovim],
        ),
    ] {
        for item in items
            .iter()
            .chain(&[NewSession, Incognito, AddProject, FilterProjects, Quit])
        {
            let category = match item {
                Shell | Lazygit | Neovim => KeyCategory::Tools,
                Quit => KeyCategory::General,
                _ => KeyCategory::Sessions,
            };
            keymap.bind(&item.key().to_string(), item.intent(), category, scope);
        }
    }
    for scope in SIDEBAR.into_iter().chain(DASHBOARD) {
        keymap
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
        keymap.bind(
            "<leader>w",
            Intent::ChangeWorkspace,
            KeyCategory::Sessions,
            scope,
        );
    }
    for scope in [
        Scope::Sidebar,
        Scope::SidebarDraft,
        Scope::SidebarWorktreeGroup,
        Scope::Dashboard,
        Scope::DashboardDraft,
        Scope::DashboardWorktreeGroup,
    ] {
        keymap.bind(
            "<leader>b",
            Intent::SwitchBranch,
            KeyCategory::Sessions,
            scope,
        );
    }
    for scope in [
        Scope::Sidebar,
        Scope::SidebarDraft,
        Scope::Dashboard,
        Scope::DashboardDraft,
    ]
    .into_iter()
    .chain(SIDEBAR_GROUPS)
    .chain(DASHBOARD_GROUPS)
    {
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
        Scope::SidebarGroup,
        Scope::SidebarWorktreeGroup,
        Scope::DashboardGroup,
        Scope::DashboardWorktreeGroup,
    ] {
        keymap
            .bind("<leader>m", Intent::PickModel, KeyCategory::Sessions, scope)
            .bind(
                "<leader>a",
                Intent::PickPermission,
                KeyCategory::Sessions,
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

/// Where a key goes while attached.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// The key is bound to an intent.
    Intent(Intent),
    /// The key goes to Claude.
    Forward,
}

/// Where `key` goes while attached.
pub(crate) fn attached_route(key: KeyEvent) -> Route {
    match (key.code, key.modifiers) {
        // `<C-\>` as the kitty protocol reports it, and as crossterm parses
        // its legacy byte 0x1C.
        (KeyCode::Char('\\' | '4'), KeyModifiers::CONTROL) => Route::Intent(Intent::Detach),
        // Claude can't bind `<C-h>`: it's Backspace in a legacy terminal.
        (KeyCode::Char('h'), KeyModifiers::CONTROL) => Route::Intent(Intent::LeavePane),
        // Claude still backgrounds a task with `Ctrl+X Ctrl+B`.
        (KeyCode::Char('b'), KeyModifiers::CONTROL) => Route::Intent(Intent::ToggleSidebar),
        _ => jump_route(key)
            .or_else(|| layout_route(key))
            .map_or(Route::Forward, Route::Intent),
    }
}

/// The jump `key` asks for in the sidebar, the dashboard or the attached
/// pane: `<C-o>` goes back through the jump list and `<C-i>` forward. Only
/// bare Ctrl matches, so `ctrl+shift+o` and Tab still reach Claude. `None`
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

/// What `key` does in the sidebar outside which-key: `<C-\>` (the kitty
/// `Char('\\')` and the legacy `Char('4')` forms) detaches the selected
/// thread. `None` for any other key.
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
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use orb_domain::feat::dashboard::DashboardItem::{
        self, AddProject, Branch, FilterProjects, Incognito, Lazygit, Model, Neovim, NewSession,
        Open, Permission, Quit, Shell, Start, Workspace,
    };
    use std::time::SystemTime;

    use orb_domain::feat::sessions::state::{
        Group, GroupDefaults, GroupId, GroupKind, Project, ProjectId, ProjectKind, Sessions,
        SidebarItem, Thread, ThreadId, ThreadStatus,
    };
    use orb_domain::feat::zellij::zellij_service::Tool;
    use orb_domain::{Focus, Intent};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    use super::{
        Keys, Route, Scope, Selection, attached_route, jump_route, keymap, layout_route,
        picker_route, press, sidebar_route,
    };

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[rstest::rstest]
    fn space_opens_the_leader_popup_in_the_sidebar() {
        // Given the keymap in Sidebar focus.
        let mut keys = Keys::new(keymap(), Scope::Sidebar);

        // When pressing Space.
        press(&mut keys, key(KeyCode::Char(' ')));

        // Then the which-key popup waits for the next key.
        assert!(keys.is_pending(), "Space should open the leader popup");
    }

    #[rstest::rstest]
    fn space_then_n_starts_a_session_in_the_sidebar() {
        // Given Space already pressed in Sidebar focus.
        let mut keys = Keys::new(keymap(), Scope::Sidebar);
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
        let mut keys = Keys::new(keymap(), scope);
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
        let mut keys = Keys::new(keymap(), Scope::Dashboard);
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
        let mut keys = Keys::new(keymap(), Scope::Dashboard);
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
    #[case(Scope::SidebarGroup, 'm', Intent::PickModel)]
    #[case(Scope::SidebarGroup, 'a', Intent::PickPermission)]
    #[case(Scope::SidebarWorktreeGroup, 'm', Intent::PickModel)]
    #[case(Scope::SidebarWorktreeGroup, 'a', Intent::PickPermission)]
    fn leader_keys_open_the_session_setup_pickers(
        #[case] scope: Scope,
        #[case] pressed: char,
        #[case] expected: Intent,
    ) {
        // Given Space already pressed.
        let mut keys = Keys::new(keymap(), scope);
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
        let mut keys = Keys::new(keymap(), scope);
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
    fn q_quits_on_the_dashboard(
        #[values(Scope::Dashboard, Scope::DashboardDraft, Scope::DashboardEmpty)] scope: Scope,
    ) {
        // Given the keymap in a dashboard scope.
        let mut keys = Keys::new(keymap(), scope);

        // When pressing `q`.
        let intent = press(&mut keys, key(KeyCode::Char('q')));

        // Then orb quits.
        assert_eq!(intent, Some(Intent::Quit), "q should quit in {scope:?}");
    }

    #[rstest::rstest]
    fn dashboard_movement_keys_move_the_menu_cursor(
        #[values(Scope::Dashboard, Scope::DashboardDraft, Scope::DashboardEmpty)] scope: Scope,
        #[values(
            (key(KeyCode::Char('j')), Intent::DashboardNext),
            (key(KeyCode::Down), Intent::DashboardNext),
            (key(KeyCode::Char('k')), Intent::DashboardPrev),
            (key(KeyCode::Up), Intent::DashboardPrev),
            (key(KeyCode::Enter), Intent::DashboardRun),
        )]
        binding: (KeyEvent, Intent),
    ) {
        // Given the keymap in a dashboard scope.
        let (pressed, expected) = binding;
        let mut keys = Keys::new(keymap(), scope);

        // When pressing the key.
        let intent = press(&mut keys, pressed);

        // Then it moves the cursor or runs the highlighted item.
        assert_eq!(intent.as_ref(), Some(&expected), "{pressed:?} in {scope:?}");
    }

    #[rstest::rstest]
    #[case::general_on_a_thread(Scope::Dashboard, &[NewSession, Incognito, AddProject, FilterProjects, Quit])]
    #[case::general_on_a_draft(Scope::DashboardDraft, &[NewSession, Incognito, AddProject, FilterProjects, Quit])]
    #[case::general_on_nothing(Scope::DashboardEmpty, &[NewSession, Incognito, AddProject, FilterProjects, Quit])]
    #[case::thread(Scope::Dashboard, &[Open, Workspace, Branch, Shell, Lazygit, Neovim])]
    #[case::draft(
        Scope::DashboardDraft,
        &[Start, Workspace, Branch, Model, Permission, Shell, Lazygit, Neovim]
    )]
    #[case::group(Scope::DashboardGroup, &[Model, Permission, Shell, Lazygit, Neovim])]
    #[case::worktree_group(
        Scope::DashboardWorktreeGroup,
        &[Branch, Model, Permission, Shell, Lazygit, Neovim]
    )]
    #[case::group_thread(Scope::DashboardGroupThread, &[Open, Shell, Lazygit, Neovim])]
    #[case::group_draft(
        Scope::DashboardGroupDraft,
        &[Start, Model, Permission, Shell, Lazygit, Neovim]
    )]
    fn dashboard_item_keys_run_their_items(#[case] scope: Scope, #[case] items: &[DashboardItem]) {
        // Given the keymap in a dashboard scope.
        let mut keys = Keys::new(keymap(), scope);

        // When pressing each item's key.
        let intents: Vec<Option<Intent>> = items
            .iter()
            .map(|item| press(&mut keys, key(KeyCode::Char(item.key()))))
            .collect();

        // Then each yields its item's intent.
        let expected: Vec<Option<Intent>> = items.iter().map(|item| Some(item.intent())).collect();
        assert_eq!(intents, expected, "the item keys in {scope:?}");
    }

    #[rstest::rstest]
    #[case(Scope::DashboardDraft, Some(Intent::PickModel))]
    #[case(Scope::DashboardGroup, Some(Intent::PickModel))]
    #[case(Scope::Dashboard, None)]
    #[case(Scope::DashboardGroupThread, None)]
    fn m_picks_the_model_only_on_a_drafts_dashboard(
        #[case] scope: Scope,
        #[case] expected: Option<Intent>,
    ) {
        // Given the keymap in a dashboard scope.
        let mut keys = Keys::new(keymap(), scope);

        // When pressing `m`.
        let intent = press(&mut keys, key(KeyCode::Char('m')));

        // Then it opens the model picker only on a draft.
        assert_eq!(intent, expected, "m in {scope:?}");
    }

    #[rstest::rstest]
    fn o_is_unbound_on_the_dashboard_with_nothing_selected() {
        // Given the keymap on the dashboard with no thread or draft selected.
        let mut keys = Keys::new(keymap(), Scope::DashboardEmpty);

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
        #[values(
            Scope::Sidebar,
            Scope::SidebarDraft,
            Scope::SidebarEmpty,
            Scope::SidebarGroup,
            Scope::SidebarWorktreeGroup,
            Scope::SidebarGroupThread,
            Scope::SidebarGroupDraft
        )]
        scope: Scope,
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
    fn leader_i_starts_incognito_in_every_scope(
        #[values(
            Scope::Sidebar,
            Scope::SidebarDraft,
            Scope::SidebarEmpty,
            Scope::SidebarGroup,
            Scope::SidebarWorktreeGroup,
            Scope::SidebarGroupThread,
            Scope::SidebarGroupDraft,
            Scope::Dashboard,
            Scope::DashboardDraft,
            Scope::DashboardEmpty,
            Scope::DashboardGroup,
            Scope::DashboardWorktreeGroup,
            Scope::DashboardGroupThread,
            Scope::DashboardGroupDraft
        )]
        scope: Scope,
    ) {
        // Given Space already pressed in `scope`.
        let mut keys = Keys::new(keymap(), scope);
        press(&mut keys, key(KeyCode::Char(' ')));

        // When pressing `i`.
        let intent = press(&mut keys, key(KeyCode::Char('i')));

        // Then it starts an incognito session.
        assert_eq!(
            intent,
            Some(Intent::NewIncognito),
            "␣i should start incognito in {scope:?}"
        );
    }

    #[rstest::rstest]
    fn i_starts_incognito_on_the_dashboard(
        #[values(
            Scope::Dashboard,
            Scope::DashboardDraft,
            Scope::DashboardEmpty,
            Scope::DashboardGroup,
            Scope::DashboardWorktreeGroup,
            Scope::DashboardGroupThread,
            Scope::DashboardGroupDraft
        )]
        scope: Scope,
    ) {
        // Given the keymap in a dashboard scope.
        let mut keys = Keys::new(keymap(), scope);

        // When pressing `i`.
        let intent = press(&mut keys, key(KeyCode::Char('i')));

        // Then it starts an incognito session.
        assert_eq!(
            intent,
            Some(Intent::NewIncognito),
            "i should start incognito in {scope:?}"
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
        let mut keys = Keys::new(keymap(), scope);
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
    #[case(vec![key(KeyCode::Enter)], Intent::DashboardRun)]
    fn dashboard_keys_map_to_their_intents(
        #[case] pressed: Vec<KeyEvent>,
        #[case] expected: Intent,
    ) {
        // Given the keymap on a thread's dashboard.
        let mut keys = Keys::new(keymap(), Scope::Dashboard);

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
    #[case(Focus::Sidebar, Selection::Thread, Scope::Sidebar)]
    #[case(Focus::Sidebar, Selection::Draft, Scope::SidebarDraft)]
    #[case(Focus::Sidebar, Selection::Nothing, Scope::SidebarEmpty)]
    #[case(Focus::Dashboard, Selection::Thread, Scope::Dashboard)]
    #[case(Focus::Dashboard, Selection::Draft, Scope::DashboardDraft)]
    #[case(Focus::Dashboard, Selection::Nothing, Scope::DashboardEmpty)]
    #[case(Focus::Sidebar, Selection::GroupCard, Scope::SidebarGroup)]
    #[case(Focus::Sidebar, Selection::WorktreeCard, Scope::SidebarWorktreeGroup)]
    #[case(Focus::Sidebar, Selection::GroupThread, Scope::SidebarGroupThread)]
    #[case(Focus::Sidebar, Selection::GroupDraft, Scope::SidebarGroupDraft)]
    #[case(Focus::Dashboard, Selection::GroupCard, Scope::DashboardGroup)]
    #[case(
        Focus::Dashboard,
        Selection::WorktreeCard,
        Scope::DashboardWorktreeGroup
    )]
    #[case(Focus::Dashboard, Selection::GroupThread, Scope::DashboardGroupThread)]
    #[case(Focus::Dashboard, Selection::GroupDraft, Scope::DashboardGroupDraft)]
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
            draft,
            defaults: GroupDefaults {
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
                    id: ThreadId(1),
                    title: None,
                    cwd: "/orb".into(),
                    transcript: None,
                    status: ThreadStatus::Idle,
                    turn_started_at: None,
                    attach_argv: vec![],
                    branch: None,
                    pinned_at: None,
                    settled_at: None,
                    active_since: SystemTime::UNIX_EPOCH,
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

    #[rstest::rstest]
    #[case::card(SidebarItem::Group(GroupId(9)), Selection::GroupCard)]
    #[case::worktree_card(SidebarItem::Group(GroupId(11)), Selection::WorktreeCard)]
    #[case::grouped_thread(SidebarItem::Thread(ThreadId(1)), Selection::GroupThread)]
    #[case::group_draft(SidebarItem::GroupDraft(GroupId(10)), Selection::GroupDraft)]
    fn selection_follows_the_cursors_group_row(
        #[case] cursor: SidebarItem,
        #[case] expected: Selection,
    ) {
        // Given the cursor on a group's row.
        let sessions = grouped(cursor);

        // When reading the selection.
        let selection = Selection::of(&sessions);

        // Then it names the kind of group row.
        assert_eq!(selection, expected, "the selection on {cursor:?}");
    }

    #[rstest::rstest]
    #[case(Scope::SidebarGroup, KeyCode::Char('j'), Intent::SelectNext)]
    #[case(Scope::SidebarGroup, KeyCode::Enter, Intent::Attach)]
    #[case(Scope::SidebarGroup, KeyCode::Char('l'), Intent::OpenGroup)]
    #[case(Scope::SidebarGroup, KeyCode::Char('h'), Intent::CloseGroup)]
    #[case(Scope::SidebarGroup, KeyCode::Char('p'), Intent::TogglePin)]
    #[case(Scope::SidebarGroup, KeyCode::Char('s'), Intent::ToggleSettle)]
    #[case(Scope::SidebarGroup, KeyCode::Char('d'), Intent::DeleteThread)]
    #[case(Scope::SidebarGroupThread, KeyCode::Char('d'), Intent::DeleteThread)]
    #[case(Scope::SidebarGroupDraft, KeyCode::Char('d'), Intent::DeleteThread)]
    #[case(Scope::SidebarGroup, KeyCode::Char('n'), Intent::NewSibling)]
    #[case(Scope::SidebarWorktreeGroup, KeyCode::Char('l'), Intent::OpenGroup)]
    #[case(Scope::SidebarWorktreeGroup, KeyCode::Char('p'), Intent::TogglePin)]
    #[case(Scope::SidebarWorktreeGroup, KeyCode::Char('d'), Intent::DeleteThread)]
    #[case(Scope::SidebarWorktreeGroup, KeyCode::Char('n'), Intent::NewSibling)]
    #[case(Scope::SidebarGroupThread, KeyCode::Char('n'), Intent::NewSibling)]
    #[case(Scope::SidebarGroupThread, KeyCode::Char('j'), Intent::SelectNext)]
    #[case(Scope::SidebarGroupThread, KeyCode::Enter, Intent::Attach)]
    #[case(Scope::SidebarGroupThread, KeyCode::Char('l'), Intent::OpenGroup)]
    #[case(Scope::SidebarGroupThread, KeyCode::Char('h'), Intent::CloseGroup)]
    #[case(Scope::SidebarGroupThread, KeyCode::Char('r'), Intent::Rename)]
    #[case(Scope::SidebarGroupDraft, KeyCode::Char('j'), Intent::SelectNext)]
    #[case(Scope::SidebarGroupDraft, KeyCode::Enter, Intent::Attach)]
    #[case(Scope::SidebarGroupDraft, KeyCode::Char('l'), Intent::OpenGroup)]
    #[case(Scope::SidebarGroupDraft, KeyCode::Char('h'), Intent::CloseGroup)]
    fn group_scope_keys_map_to_their_intents(
        #[case] scope: Scope,
        #[case] code: KeyCode,
        #[case] expected: Intent,
    ) {
        // Given the keymap on a group's row in the sidebar.
        let mut keys = Keys::new(keymap(), scope);

        // When pressing the key.
        let intent = press(&mut keys, key(code));

        // Then it yields its intent.
        assert_eq!(intent.as_ref(), Some(&expected), "{code:?} in {scope:?}");
    }

    #[rstest::rstest]
    #[case('p')]
    #[case('s')]
    fn pin_and_settle_are_unbound_on_a_grouped_thread(#[case] pressed: char) {
        // Given the keymap on a thread in a group.
        let mut keys = Keys::new(keymap(), Scope::SidebarGroupThread);

        // When pressing `p` or `s`.
        let intent = press(&mut keys, key(KeyCode::Char(pressed)));

        // Then nothing happens: its group is pinned and settled instead.
        assert_eq!(intent, None, "{pressed} on a grouped thread");
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
        let mut keys = Keys::new(keymap(), scope);

        // When pressing `n`.
        let intent = press(&mut keys, key(KeyCode::Char('n')));

        // Then nothing happens.
        assert_eq!(intent, None, "n in {scope:?}");
    }

    #[rstest::rstest]
    fn leader_w_is_unbound_in_group_scopes(
        #[values(
            Scope::SidebarGroup,
            Scope::SidebarWorktreeGroup,
            Scope::SidebarGroupThread,
            Scope::SidebarGroupDraft,
            Scope::DashboardGroup,
            Scope::DashboardWorktreeGroup,
            Scope::DashboardGroupThread,
            Scope::DashboardGroupDraft
        )]
        scope: Scope,
    ) {
        // Given the leader popup's keys in a group scope.
        let keys = leader_popup(scope);

        // When looking for `w`.
        let found = keys.contains(&key(KeyCode::Char('w')));

        // Then it isn't listed.
        assert!(!found, "w in the {scope:?} leader popup");
    }

    #[rstest::rstest]
    fn leader_b_is_unbound_on_group_rows_without_a_worktree(
        #[values(
            Scope::SidebarGroup,
            Scope::SidebarGroupThread,
            Scope::SidebarGroupDraft,
            Scope::DashboardGroup,
            Scope::DashboardGroupThread,
            Scope::DashboardGroupDraft
        )]
        scope: Scope,
    ) {
        // Given the leader popup's keys on a group's thread, draft, or a card
        // with no worktree.
        let keys = leader_popup(scope);

        // When looking for `b`.
        let found = keys.contains(&key(KeyCode::Char('b')));

        // Then it isn't listed.
        assert!(!found, "b in the {scope:?} leader popup");
    }

    #[rstest::rstest]
    fn leader_b_switches_branch_on_a_worktree_card(
        #[values(Scope::SidebarWorktreeGroup, Scope::DashboardWorktreeGroup)] scope: Scope,
    ) {
        // Given Space already pressed on a started Feature group's card.
        let mut keys = Keys::new(keymap(), scope);
        press(&mut keys, key(KeyCode::Char(' ')));

        // When pressing `b`.
        let intent = press(&mut keys, key(KeyCode::Char('b')));

        // Then it switches the worktree's branch.
        assert_eq!(intent, Some(Intent::SwitchBranch), "Space b in {scope:?}");
    }

    /// The keys the leader popup lists in `scope`.
    fn leader_popup(scope: Scope) -> Vec<KeyEvent> {
        keymap()
            .get_children_at_path(&[key(KeyCode::Char(' '))], &scope)
            .unwrap_or_default()
            .into_iter()
            .map(|(key, _)| key)
            .collect()
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
        let mut keys = Keys::new(keymap(), scope);
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
            Scope::SidebarGroup,
            Scope::SidebarWorktreeGroup,
            Scope::SidebarGroupThread,
            Scope::SidebarGroupDraft,
            Scope::DashboardGroup,
            Scope::DashboardWorktreeGroup,
            Scope::DashboardGroupThread,
            Scope::DashboardGroupDraft
        )]
        scope: Scope,
    ) {
        // Given Space and `g` already pressed.
        let mut keys = Keys::new(keymap(), scope);
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
        #[values(Scope::SidebarEmpty, Scope::DashboardGroup)] scope: Scope,
        #[values(
            ('f', GroupKind::Feature),
            ('r', GroupKind::Research),
            ('l', GroupKind::Learn)
        )]
        binding: (char, GroupKind),
    ) {
        // Given Space and `g` already pressed.
        let (pressed, kind) = binding;
        let mut keys = Keys::new(keymap(), scope);
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
            .get_children_at_path(&[key(KeyCode::Char(' ')), key(KeyCode::Char('g'))], &scope)
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
    #[case(Scope::Sidebar, "befginptvw")]
    #[case(Scope::SidebarDraft, "abefgimnptvw")]
    #[case(Scope::SidebarEmpty, "efginp")]
    #[case(Scope::SidebarGroup, "aefgimnptv")]
    #[case(Scope::SidebarWorktreeGroup, "abefgimnptv")]
    #[case(Scope::SidebarGroupThread, "efginptv")]
    #[case(Scope::SidebarGroupDraft, "aefgimnptv")]
    #[case(Scope::Dashboard, "beginptvw")]
    #[case(Scope::DashboardDraft, "abegimnptvw")]
    #[case(Scope::DashboardEmpty, "eginp")]
    #[case(Scope::DashboardGroup, "aegimnptv")]
    #[case(Scope::DashboardWorktreeGroup, "abegimnptv")]
    #[case(Scope::DashboardGroupThread, "eginptv")]
    #[case(Scope::DashboardGroupDraft, "aegimnptv")]
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
        let mut keys = Keys::new(keymap(), Scope::SidebarDraft);

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
        let mut keys = Keys::new(keymap(), Scope::SidebarEmpty);

        // When pressing `r`.
        let intent = press(&mut keys, key(KeyCode::Char('r')));

        // Then nothing happens.
        assert_eq!(intent, None, "only a thread can be renamed");
    }

    #[rstest::rstest]
    fn enter_on_a_draft_in_the_sidebar_attaches() {
        // Given the keymap in the sidebar on a draft.
        let mut keys = Keys::new(keymap(), Scope::SidebarDraft);

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
    #[case(vec![key(KeyCode::Enter)], Intent::DashboardRun)]
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
        let mut keys = Keys::new(keymap(), Scope::DashboardDraft);

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
    #[case('d', Intent::DeleteThread)]
    fn sidebar_thread_key_yields_its_intent(#[case] c: char, #[case] expected: Intent) {
        // Given the keymap in Sidebar focus.
        let mut keys = Keys::new(keymap(), Scope::Sidebar);

        // When pressing the key once.
        let intent = press(&mut keys, key(KeyCode::Char(c)));

        // Then it yields its intent.
        assert_eq!(intent, Some(expected), "{c} in the sidebar");
    }

    #[rstest::rstest]
    fn d_on_a_draft_in_the_sidebar_deletes() {
        // Given the keymap in the sidebar on a draft.
        let mut keys = Keys::new(keymap(), Scope::SidebarDraft);

        // When pressing `d`.
        let intent = press(&mut keys, key(KeyCode::Char('d')));

        // Then it asks to discard the draft.
        assert_eq!(
            intent,
            Some(Intent::DeleteThread),
            "d on a draft should ask to discard it"
        );
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
}
