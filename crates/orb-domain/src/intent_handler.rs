//! The [`IntentHandler`]: the single decision point for all user input.

use crate::{AppState, Intent};

/// Applies each [`Intent`] to [`AppState`] in one match block.
pub struct IntentHandler;

impl IntentHandler {
    /// Apply `intent` to `state`.
    pub fn handle(intent: &Intent, state: &mut AppState) {
        match intent {
            Intent::Quit => state.should_quit = true,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{AppState, Intent, IntentHandler};

    #[rstest::rstest]
    fn quit_sets_should_quit_in_state() {
        // Given a fresh AppState.
        let mut state = AppState::default();

        // When handling Quit.
        IntentHandler::handle(&Intent::Quit, &mut state);

        // Then the frontend is told to exit.
        assert!(state.should_quit, "Quit should set should_quit");
    }
}
