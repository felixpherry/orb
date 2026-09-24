//! Key → [`Intent`] mapping. Only keys the user has defined are bound.

use orb_domain::Intent;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The intent bound to `key`, if any.
pub(crate) fn intent_for(key: KeyEvent) -> Option<Intent> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), KeyModifiers::NONE) => Some(Intent::Quit),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use orb_domain::Intent;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::intent_for;

    #[rstest::rstest]
    fn plain_q_maps_to_quit() {
        // Given a plain `q` press.
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);

        // When mapping it.
        let intent = intent_for(key);

        // Then it quits.
        assert_eq!(intent, Some(Intent::Quit), "plain q should quit");
    }

    #[rstest::rstest]
    #[case(KeyCode::Char('Q'), KeyModifiers::SHIFT)]
    #[case(KeyCode::Char('q'), KeyModifiers::CONTROL)]
    #[case(KeyCode::Char('x'), KeyModifiers::NONE)]
    #[case(KeyCode::Esc, KeyModifiers::NONE)]
    fn other_keys_map_to_nothing(#[case] code: KeyCode, #[case] modifiers: KeyModifiers) {
        // Given a key that isn't bound.
        let key = KeyEvent::new(code, modifiers);

        // When mapping it.
        let intent = intent_for(key);

        // Then nothing happens.
        assert_eq!(intent, None, "{code} with {modifiers} should be unbound");
    }
}
