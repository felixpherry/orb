//! Key routing: which keys do what in each focus. Only keys the user has
//! defined are bound.
//!
//! In the sidebar and the preview, keys go through a which-key keymap whose
//! scope is the focus; `<Space>` is the leader and shows a popup. While
//! attached, every key goes to Claude except `<C-\>`.

use std::fmt;

use orb_domain::{Focus, Intent};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui_which_key::{Keymap, WhichKeyState};

/// The which-key popup's section for a binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyCategory {
    General,
    Navigation,
    Sessions,
}

impl fmt::Display for KeyCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::General => "general",
            Self::Navigation => "navigation",
            Self::Sessions => "sessions",
        })
    }
}

/// The keymap with its current scope and pending key sequence.
pub(crate) type Keys = WhichKeyState<KeyEvent, Focus, Intent, KeyCategory>;

/// The sidebar and preview bindings, scoped by focus.
pub(crate) fn keymap() -> Keymap<KeyEvent, Focus, Intent, KeyCategory> {
    let mut keymap = Keymap::new();
    keymap
        .describe_group("<leader>", "leader")
        .bind(
            "j",
            Intent::SelectNext,
            KeyCategory::Navigation,
            Focus::Sidebar,
        )
        .bind(
            "k",
            Intent::SelectPrev,
            KeyCategory::Navigation,
            Focus::Sidebar,
        )
        .bind(
            "<c-l>",
            Intent::FocusPreview,
            KeyCategory::Navigation,
            Focus::Sidebar,
        )
        .bind(
            "<enter>",
            Intent::Attach,
            KeyCategory::Sessions,
            Focus::Sidebar,
        )
        .bind("q", Intent::Quit, KeyCategory::General, Focus::Sidebar)
        .bind(
            "<leader>n",
            Intent::NewSession,
            KeyCategory::Sessions,
            Focus::Sidebar,
        )
        .bind(
            "<c-h>",
            Intent::FocusSidebar,
            KeyCategory::Navigation,
            Focus::Preview,
        )
        .bind(
            "<enter>",
            Intent::Attach,
            KeyCategory::Sessions,
            Focus::Preview,
        )
        .bind(
            "<leader>n",
            Intent::NewSession,
            KeyCategory::Sessions,
            Focus::Preview,
        );
    keymap
}

/// Feeds `key` to the keymap; returns the intent once a binding completes.
pub(crate) fn press(keys: &mut Keys, key: KeyEvent) -> Option<Intent> {
    // Bindings compare the whole event: a held key (`Repeat`) or one carrying
    // lock-state bits would never match, so keep only the code and modifiers.
    keys.handle_key(KeyEvent::new(key.code, key.modifiers))
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

#[cfg(test)]
mod tests {
    use orb_domain::{Focus, Intent};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    use super::{Keys, Route, attached_route, keymap, press};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[rstest::rstest]
    fn space_opens_the_leader_popup_in_the_sidebar() {
        // Given the keymap in Sidebar focus.
        let mut keys = Keys::new(keymap(), Focus::Sidebar);

        // When pressing Space.
        press(&mut keys, key(KeyCode::Char(' ')));

        // Then the which-key popup waits for the next key.
        assert!(keys.is_pending(), "Space should open the leader popup");
    }

    #[rstest::rstest]
    fn space_then_n_starts_a_session_in_the_sidebar() {
        // Given Space already pressed in Sidebar focus.
        let mut keys = Keys::new(keymap(), Focus::Sidebar);
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
    fn q_is_ignored_in_the_preview() {
        // Given the keymap in Preview focus.
        let mut keys = Keys::new(keymap(), Focus::Preview);

        // When pressing `q`.
        let intent = press(&mut keys, key(KeyCode::Char('q')));

        // Then nothing happens.
        assert_eq!(intent, None, "q should only quit from the sidebar");
    }

    #[rstest::rstest]
    #[case(key(KeyCode::Char('j')), Intent::SelectNext)]
    #[case(key(KeyCode::Char('k')), Intent::SelectPrev)]
    #[case(
        KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
        Intent::FocusPreview
    )]
    #[case(key(KeyCode::Enter), Intent::Attach)]
    #[case(key(KeyCode::Char('q')), Intent::Quit)]
    fn sidebar_keys_map_to_their_intents(#[case] pressed: KeyEvent, #[case] expected: Intent) {
        // Given the keymap in Sidebar focus.
        let mut keys = Keys::new(keymap(), Focus::Sidebar);

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
    #[case(
        KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL),
        Intent::FocusSidebar
    )]
    #[case(key(KeyCode::Enter), Intent::Attach)]
    fn preview_keys_map_to_their_intents(#[case] pressed: KeyEvent, #[case] expected: Intent) {
        // Given the keymap in Preview focus.
        let mut keys = Keys::new(keymap(), Focus::Preview);

        // When pressing the key.
        let intent = press(&mut keys, pressed);

        // Then it yields its intent.
        assert_eq!(
            intent.as_ref(),
            Some(&expected),
            "the key for {expected} in the preview"
        );
    }

    #[rstest::rstest]
    fn held_j_selects_the_next_thread() {
        // Given a held `j` as the kitty protocol reports it.
        let mut keys = Keys::new(keymap(), Focus::Sidebar);
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
