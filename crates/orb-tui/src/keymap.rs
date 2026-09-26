//! Key routing: which keys do what in each focus. Only keys the user has
//! defined are bound.
//!
//! In the sidebar and the preview, keys go through a which-key keymap whose
//! scope is the focus and what the sidebar cursor is on; `<Space>` is the
//! leader and shows a popup. A key that does nothing for the selection isn't
//! bound there (the preview's block keys on a draft, `␣m`/`␣a` on a thread,
//! `␣w`/`␣b` and the tool keys `␣t`/`␣g`/`␣v` with nothing selected), so the
//! popups don't offer it. While attached, every key goes to Claude except
//! `<C-\>`. An open picker takes typed characters as filter text and has its
//! own fixed keys.

use std::fmt;

use orb_domain::feat::sessions::state::Sessions;
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
    Preview,
    Tools,
}

impl fmt::Display for KeyCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::General => "general",
            Self::Navigation => "navigation",
            Self::Sessions => "sessions",
            Self::Threads => "threads",
            Self::Preview => "preview",
            Self::Tools => "tools",
        })
    }
}

/// What the sidebar cursor is on, which decides the keys that do something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Selection {
    Thread,
    Draft,
    /// No row, or the settled shelf's header.
    Nothing,
}

impl Selection {
    /// What `sessions`' cursor is on.
    pub(crate) fn of(sessions: &Sessions) -> Self {
        match (sessions.selected_draft(), sessions.selected_thread()) {
            (Some(_), _) => Self::Draft,
            (None, Some(_)) => Self::Thread,
            (None, None) => Self::Nothing,
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
    /// The preview of a thread.
    Preview,
    /// The preview side while a draft is selected: its form has no blocks to
    /// move through, fold or yank.
    DraftForm,
    /// The preview with no thread or draft selected.
    PreviewEmpty,
}

impl Scope {
    /// The scope for keys in `focus` with `selection`.
    pub(crate) fn new(focus: Focus, selection: Selection) -> Self {
        match (focus, selection) {
            (Focus::Preview, Selection::Thread) => Self::Preview,
            (Focus::Preview, Selection::Draft) => Self::DraftForm,
            (Focus::Preview, Selection::Nothing) => Self::PreviewEmpty,
            (Focus::Sidebar | Focus::Attached | Focus::Picker, Selection::Thread) => Self::Sidebar,
            (Focus::Sidebar | Focus::Attached | Focus::Picker, Selection::Draft) => {
                Self::SidebarDraft
            }
            (Focus::Sidebar | Focus::Attached | Focus::Picker, Selection::Nothing) => {
                Self::SidebarEmpty
            }
        }
    }
}

/// The keymap with its current scope and pending key sequence.
pub(crate) type Keys = WhichKeyState<KeyEvent, Scope, Intent, KeyCategory>;

/// The sidebar and preview bindings, scoped by focus and selection.
#[expect(
    clippy::too_many_lines,
    reason = "one binding per key keeps the whole keymap in one place"
)]
pub(crate) fn keymap() -> Keymap<KeyEvent, Scope, Intent, KeyCategory> {
    let mut keymap = Keymap::new();
    keymap.describe_group("<leader>", "leader");
    for scope in [Scope::Sidebar, Scope::SidebarDraft, Scope::SidebarEmpty] {
        keymap
            .bind("j", Intent::SelectNext, KeyCategory::Navigation, scope)
            .bind("k", Intent::SelectPrev, KeyCategory::Navigation, scope)
            .bind(
                "<c-l>",
                Intent::FocusPreview,
                KeyCategory::Navigation,
                scope,
            )
            .bind("<enter>", Intent::Attach, KeyCategory::Sessions, scope)
            .bind("q", Intent::Quit, KeyCategory::General, scope)
            .bind("l", Intent::OpenShelf, KeyCategory::Navigation, scope)
            .bind("h", Intent::CloseShelf, KeyCategory::Navigation, scope)
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
    keymap
        .bind("p", Intent::TogglePin, KeyCategory::Threads, Scope::Sidebar)
        .bind(
            "ss",
            Intent::ToggleSettle,
            KeyCategory::Threads,
            Scope::Sidebar,
        );
    for scope in [Scope::Sidebar, Scope::SidebarDraft] {
        keymap.bind("xx", Intent::DeleteThread, KeyCategory::Threads, scope);
    }
    for scope in [Scope::Preview, Scope::DraftForm, Scope::PreviewEmpty] {
        keymap
            .bind(
                "<c-h>",
                Intent::FocusSidebar,
                KeyCategory::Navigation,
                scope,
            )
            .bind("<enter>", Intent::Attach, KeyCategory::Sessions, scope)
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
    keymap
        .bind(
            "j",
            Intent::NextBlock,
            KeyCategory::Navigation,
            Scope::Preview,
        )
        .bind(
            "k",
            Intent::PrevBlock,
            KeyCategory::Navigation,
            Scope::Preview,
        )
        .bind(
            "<c-d>",
            Intent::HalfPageDown,
            KeyCategory::Navigation,
            Scope::Preview,
        )
        .bind(
            "<c-u>",
            Intent::HalfPageUp,
            KeyCategory::Navigation,
            Scope::Preview,
        )
        .bind("gg", Intent::Top, KeyCategory::Navigation, Scope::Preview)
        .bind("G", Intent::Bottom, KeyCategory::Navigation, Scope::Preview)
        .bind("y", Intent::Yank, KeyCategory::Preview, Scope::Preview)
        .bind(
            "za",
            Intent::ToggleFold,
            KeyCategory::Preview,
            Scope::Preview,
        )
        .bind(
            "<tab>",
            Intent::ToggleFold,
            KeyCategory::Preview,
            Scope::Preview,
        );
    for scope in [
        Scope::Sidebar,
        Scope::SidebarDraft,
        Scope::Preview,
        Scope::DraftForm,
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
            )
            .bind(
                "<leader>t",
                Intent::OpenTool(Tool::Shell),
                KeyCategory::Tools,
                scope,
            )
            .bind(
                "<leader>g",
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
    for scope in [Scope::SidebarDraft, Scope::DraftForm] {
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

/// The sidebar key waiting for its repeat: the `s` of `ss` or the `x` of
/// `xx`.
pub(crate) fn pending_confirm(keys: &Keys) -> Option<char> {
    match (keys.scope(), keys.current_sequence.as_slice()) {
        (
            Scope::Sidebar | Scope::SidebarDraft,
            [
                KeyEvent {
                    code: KeyCode::Char(c @ ('s' | 'x')),
                    modifiers,
                    ..
                },
            ],
        ) if modifiers.is_empty() => Some(*c),
        _ => None,
    }
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
        _ => Route::Forward,
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
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use orb_domain::feat::zellij::zellij_service::Tool;
    use orb_domain::{Focus, Intent};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    use super::{
        Keys, Route, Scope, Selection, attached_route, keymap, pending_confirm, picker_route, press,
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
    #[case(Scope::Preview)]
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
    fn leader_w_in_preview_changes_workspace() {
        // Given Space already pressed in Preview focus.
        let mut keys = Keys::new(keymap(), Scope::Preview);
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
    fn leader_b_in_preview_switches_branch() {
        // Given Space already pressed in Preview focus.
        let mut keys = Keys::new(keymap(), Scope::Preview);
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
    fn ctrl_x_does_nothing_in_the_picker() {
        // Given / When routing `<C-x>` in an open picker.
        let intent = picker_route(ctrl('x'));

        // Then nothing happens.
        assert_eq!(intent, None, "<C-x> isn't bound in the picker");
    }

    #[rstest::rstest]
    fn q_is_ignored_in_the_preview() {
        // Given the keymap in Preview focus.
        let mut keys = Keys::new(keymap(), Scope::Preview);

        // When pressing `q`.
        let intent = press(&mut keys, key(KeyCode::Char('q')));

        // Then nothing happens.
        assert_eq!(intent, None, "q should only quit from the sidebar");
    }

    #[rstest::rstest]
    #[case(key(KeyCode::Char('j')), Intent::SelectNext)]
    #[case(key(KeyCode::Char('k')), Intent::SelectPrev)]
    #[case(ctrl('l'), Intent::FocusPreview)]
    #[case(key(KeyCode::Enter), Intent::Attach)]
    #[case(key(KeyCode::Char('q')), Intent::Quit)]
    #[case(key(KeyCode::Char('p')), Intent::TogglePin)]
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
    #[case(vec![ctrl('h')], Intent::FocusSidebar)]
    #[case(vec![key(KeyCode::Enter)], Intent::Attach)]
    #[case(vec![key(KeyCode::Char('j'))], Intent::NextBlock)]
    #[case(vec![key(KeyCode::Char('k'))], Intent::PrevBlock)]
    #[case(vec![ctrl('d')], Intent::HalfPageDown)]
    #[case(vec![ctrl('u')], Intent::HalfPageUp)]
    #[case(vec![key(KeyCode::Char('g')), key(KeyCode::Char('g'))], Intent::Top)]
    #[case(
        vec![KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT)],
        Intent::Bottom
    )]
    #[case(vec![key(KeyCode::Char('y'))], Intent::Yank)]
    #[case(vec![key(KeyCode::Char('z')), key(KeyCode::Char('a'))], Intent::ToggleFold)]
    #[case(vec![key(KeyCode::Tab)], Intent::ToggleFold)]
    fn preview_keys_map_to_their_intents(#[case] pressed: Vec<KeyEvent>, #[case] expected: Intent) {
        // Given the keymap in Preview focus.
        let mut keys = Keys::new(keymap(), Scope::Preview);

        // When pressing the keys in order (Shift+G as kitty reports it).
        let intent = pressed
            .into_iter()
            .map(|pressed| press(&mut keys, pressed))
            .last()
            .flatten();

        // Then it yields its intent.
        assert_eq!(
            intent.as_ref(),
            Some(&expected),
            "the key for {expected} in the preview"
        );
    }

    #[rstest::rstest]
    #[case(Focus::Sidebar, Selection::Thread, Scope::Sidebar)]
    #[case(Focus::Sidebar, Selection::Draft, Scope::SidebarDraft)]
    #[case(Focus::Sidebar, Selection::Nothing, Scope::SidebarEmpty)]
    #[case(Focus::Preview, Selection::Thread, Scope::Preview)]
    #[case(Focus::Preview, Selection::Draft, Scope::DraftForm)]
    #[case(Focus::Preview, Selection::Nothing, Scope::PreviewEmpty)]
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
    #[case(Scope::Preview, false)]
    #[case(Scope::SidebarEmpty, false)]
    #[case(Scope::PreviewEmpty, false)]
    #[case(Scope::SidebarDraft, true)]
    #[case(Scope::DraftForm, true)]
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
    #[case(Scope::PreviewEmpty, false)]
    #[case(Scope::Sidebar, true)]
    #[case(Scope::Preview, true)]
    #[case(Scope::SidebarDraft, true)]
    #[case(Scope::DraftForm, true)]
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
        #[values(Scope::Sidebar, Scope::SidebarDraft, Scope::Preview, Scope::DraftForm)]
        scope: Scope,
        #[values(('t', Tool::Shell), ('g', Tool::Lazygit), ('v', Tool::Nvim))] binding: (
            char,
            Tool,
        ),
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
    #[case(Scope::PreviewEmpty, false)]
    #[case(Scope::Sidebar, true)]
    #[case(Scope::Preview, true)]
    #[case(Scope::SidebarDraft, true)]
    #[case(Scope::DraftForm, true)]
    fn leader_popup_lists_tools_only_with_a_selection(#[case] scope: Scope, #[case] listed: bool) {
        // Given the leader popup's keys in the scope.
        let keys = leader_popup(scope);

        // When looking for `t`, `g` and `v`.
        let found = ['t', 'g', 'v'].map(|c| keys.contains(&key(KeyCode::Char(c))));

        // Then all three are listed exactly when a thread or draft is selected.
        assert_eq!(found, [listed; 3], "t/g/v in the {scope:?} leader popup");
    }

    #[rstest::rstest]
    #[case(vec![key(KeyCode::Char('p'))])]
    #[case(vec![key(KeyCode::Char('s')), key(KeyCode::Char('s'))])]
    fn pin_and_settle_are_unbound_on_a_draft_in_the_sidebar(#[case] pressed: Vec<KeyEvent>) {
        // Given the keymap in the sidebar on a draft.
        let mut keys = Keys::new(keymap(), Scope::SidebarDraft);

        // When pressing `p` or `ss`.
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
    fn x_waits_for_its_repeat_on_a_draft_in_the_sidebar() {
        // Given the keymap in the sidebar on a draft.
        let mut keys = Keys::new(keymap(), Scope::SidebarDraft);

        // When pressing `x` once.
        press(&mut keys, key(KeyCode::Char('x')));

        // Then the discard waits for the second `x`.
        assert_eq!(
            pending_confirm(&keys),
            Some('x'),
            "x on a draft should wait for xx"
        );
    }

    #[rstest::rstest]
    #[case(vec![key(KeyCode::Char('j'))])]
    #[case(vec![key(KeyCode::Char('k'))])]
    #[case(vec![ctrl('d')])]
    #[case(vec![ctrl('u')])]
    #[case(vec![key(KeyCode::Char('g')), key(KeyCode::Char('g'))])]
    #[case(vec![KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT)])]
    #[case(vec![key(KeyCode::Char('y'))])]
    #[case(vec![key(KeyCode::Char('z')), key(KeyCode::Char('a'))])]
    #[case(vec![key(KeyCode::Tab)])]
    fn block_keys_are_unbound_on_a_draft_form(#[case] pressed: Vec<KeyEvent>) {
        // Given the keymap on a draft's form.
        let mut keys = Keys::new(keymap(), Scope::DraftForm);

        // When pressing a preview block key.
        let intents: Vec<Option<Intent>> = pressed
            .into_iter()
            .map(|pressed| press(&mut keys, pressed))
            .collect();

        // Then nothing happens.
        assert!(
            intents.iter().all(Option::is_none),
            "a block key did something on a draft: {intents:?}"
        );
    }

    #[rstest::rstest]
    #[case('g')]
    #[case('z')]
    fn block_prefix_opens_no_popup_on_a_draft_form(#[case] pressed: char) {
        // Given the keymap on a draft's form.
        let mut keys = Keys::new(keymap(), Scope::DraftForm);

        // When pressing the first key of `gg` or `za`.
        press(&mut keys, key(KeyCode::Char(pressed)));

        // Then which-key waits for nothing.
        assert!(!keys.is_pending(), "{pressed} shouldn't open a popup");
    }

    #[rstest::rstest]
    #[case(vec![ctrl('h')], Intent::FocusSidebar)]
    #[case(vec![key(KeyCode::Enter)], Intent::Attach)]
    #[case(vec![key(KeyCode::Char(' ')), key(KeyCode::Char('n'))], Intent::NewSession)]
    #[case(vec![key(KeyCode::Char(' ')), key(KeyCode::Char('p'))], Intent::AddProject)]
    #[case(vec![key(KeyCode::Char(' ')), key(KeyCode::Char('w'))], Intent::ChangeWorkspace)]
    #[case(vec![key(KeyCode::Char(' ')), key(KeyCode::Char('b'))], Intent::SwitchBranch)]
    #[case(vec![key(KeyCode::Char(' ')), key(KeyCode::Char('m'))], Intent::PickModel)]
    #[case(vec![key(KeyCode::Char(' ')), key(KeyCode::Char('a'))], Intent::PickPermission)]
    fn draft_form_keys_map_to_their_intents(
        #[case] pressed: Vec<KeyEvent>,
        #[case] expected: Intent,
    ) {
        // Given the keymap on a draft's form.
        let mut keys = Keys::new(keymap(), Scope::DraftForm);

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
            "the key for {expected} on a draft's form"
        );
    }

    #[rstest::rstest]
    fn s_then_s_toggles_settle() {
        // Given `s` already pressed in Sidebar focus.
        let mut keys = Keys::new(keymap(), Scope::Sidebar);
        press(&mut keys, key(KeyCode::Char('s')));

        // When pressing `s` again.
        let intent = press(&mut keys, key(KeyCode::Char('s')));

        // Then it settles or un-settles the thread.
        assert_eq!(intent, Some(Intent::ToggleSettle), "ss should settle");
    }

    #[rstest::rstest]
    fn x_then_x_deletes() {
        // Given `x` already pressed in Sidebar focus.
        let mut keys = Keys::new(keymap(), Scope::Sidebar);
        press(&mut keys, key(KeyCode::Char('x')));

        // When pressing `x` again.
        let intent = press(&mut keys, key(KeyCode::Char('x')));

        // Then it deletes the thread.
        assert_eq!(intent, Some(Intent::DeleteThread), "xx should delete");
    }

    #[rstest::rstest]
    fn s_then_j_does_nothing() {
        // Given the keymap in Sidebar focus.
        let mut keys = Keys::new(keymap(), Scope::Sidebar);

        // When pressing `s` then `j`.
        let intents =
            [KeyCode::Char('s'), KeyCode::Char('j')].map(|code| press(&mut keys, key(code)));

        // Then neither key yields an intent.
        assert_eq!(intents, [None, None], "j should cancel the pending s");
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
    #[case(KeyCode::Char('q'), KeyModifiers::NONE)]
    #[case(KeyCode::Enter, KeyModifiers::NONE)]
    #[case(KeyCode::Esc, KeyModifiers::NONE)]
    #[case(KeyCode::Char('h'), KeyModifiers::CONTROL)]
    #[case(KeyCode::Char('a'), KeyModifiers::NONE)]
    #[case(KeyCode::Char(' '), KeyModifiers::NONE)]
    fn keys_are_forwarded_while_attached(#[case] code: KeyCode, #[case] modifiers: KeyModifiers) {
        // Given a key other than `<C-\>`.
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
