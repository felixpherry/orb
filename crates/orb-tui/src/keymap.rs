//! Key routing: where a key goes depends on the focus. Only keys the user has
//! defined are bound.

use orb_domain::{Focus, Intent};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Where a key goes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// The key is bound to an intent.
    Intent(Intent),
    /// The key goes to the terminal pane's child.
    Forward,
    /// The key does nothing.
    Ignore,
}

/// Where `key` goes while `focus` holds.
pub(crate) fn route(key: KeyEvent, focus: Focus) -> Route {
    match focus {
        Focus::Attached => match (key.code, key.modifiers) {
            // `<C-\>` as the kitty protocol reports it, and as crossterm parses
            // its legacy byte 0x1C.
            (KeyCode::Char('\\' | '4'), KeyModifiers::CONTROL) => Route::Intent(Intent::Detach),
            _ => Route::Forward,
        },
        Focus::Normal => match (key.code, key.modifiers) {
            (KeyCode::Char('q'), KeyModifiers::NONE) => Route::Intent(Intent::Quit),
            (KeyCode::Enter, KeyModifiers::NONE) => Route::Intent(Intent::Attach),
            _ => Route::Ignore,
        },
    }
}

#[cfg(test)]
mod tests {
    use orb_domain::{Focus, Intent};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{Route, route};

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
        let routed = route(key, Focus::Attached);

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
        let routed = route(key, Focus::Attached);

        // Then it goes to the child.
        assert_eq!(
            routed,
            Route::Forward,
            "{code} with {modifiers} should be forwarded"
        );
    }

    #[rstest::rstest]
    fn enter_attaches_in_normal_focus() {
        // Given a plain Enter press.
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        // When routing it in Normal focus.
        let routed = route(key, Focus::Normal);

        // Then it attaches.
        assert_eq!(routed, Route::Intent(Intent::Attach), "Enter should attach");
    }

    #[rstest::rstest]
    fn plain_q_maps_to_quit() {
        // Given a plain `q` press.
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);

        // When routing it in Normal focus.
        let routed = route(key, Focus::Normal);

        // Then it quits.
        assert_eq!(routed, Route::Intent(Intent::Quit), "plain q should quit");
    }

    #[rstest::rstest]
    #[case(KeyCode::Char('Q'), KeyModifiers::SHIFT)]
    #[case(KeyCode::Char('q'), KeyModifiers::CONTROL)]
    #[case(KeyCode::Char('x'), KeyModifiers::NONE)]
    #[case(KeyCode::Esc, KeyModifiers::NONE)]
    #[case(KeyCode::Char('\\'), KeyModifiers::CONTROL)]
    fn other_keys_are_ignored_in_normal_focus(
        #[case] code: KeyCode,
        #[case] modifiers: KeyModifiers,
    ) {
        // Given a key that isn't bound in Normal focus.
        let key = KeyEvent::new(code, modifiers);

        // When routing it in Normal focus.
        let routed = route(key, Focus::Normal);

        // Then nothing happens.
        assert_eq!(
            routed,
            Route::Ignore,
            "{code} with {modifiers} should be ignored"
        );
    }
}
