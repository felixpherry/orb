//! The [`IntentHandler`]: the single decision point for all user input.

use crate::feat::pane::validator::validate_attach;
use crate::{AppState, Command, Focus, Intent};

/// Applies each [`Intent`] to [`AppState`] in one match block.
pub struct IntentHandler;

impl IntentHandler {
    /// Apply `intent` to `state` and return the commands the frontend loop
    /// must carry out. An intent that fails validation changes nothing.
    pub fn handle(intent: &Intent, state: &mut AppState) -> Vec<Command> {
        match intent {
            Intent::Quit => {
                state.should_quit = true;
                vec![]
            }
            Intent::Attach => match validate_attach(state) {
                Ok(()) => {
                    state.focus = Focus::Attached;
                    vec![Command::Attach]
                }
                Err(_) => vec![],
            },
            Intent::Detach => {
                state.focus = Focus::Normal;
                vec![Command::Detach]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{AppState, Command, Focus, Intent, IntentHandler};

    fn state_with_pane_command() -> AppState {
        AppState {
            pane_argv: vec!["cat".into()],
            ..AppState::default()
        }
    }

    #[rstest::rstest]
    fn quit_sets_should_quit_in_state() {
        // Given a fresh AppState.
        let mut state = AppState::default();

        // When handling Quit.
        IntentHandler::handle(&Intent::Quit, &mut state);

        // Then the frontend is told to exit.
        assert!(state.should_quit, "Quit should set should_quit");
    }

    #[rstest::rstest]
    fn attach_sets_focus_attached() {
        // Given orb started with a pane command.
        let mut state = state_with_pane_command();

        // When handling Attach.
        IntentHandler::handle(&Intent::Attach, &mut state);

        // Then keys go to the pane.
        assert_eq!(state.focus, Focus::Attached, "Attach should focus the pane");
    }

    #[rstest::rstest]
    fn attach_returns_attach_command() {
        // Given orb started with a pane command.
        let mut state = state_with_pane_command();

        // When handling Attach.
        let commands = IntentHandler::handle(&Intent::Attach, &mut state);

        // Then the loop is told to attach.
        assert_eq!(
            commands,
            vec![Command::Attach],
            "Attach should return Command::Attach"
        );
    }

    #[rstest::rstest]
    fn attach_without_pane_command_leaves_focus_normal() {
        // Given orb started without a pane command.
        let mut state = AppState::default();

        // When handling Attach.
        IntentHandler::handle(&Intent::Attach, &mut state);

        // Then keys still drive orb.
        assert_eq!(
            state.focus,
            Focus::Normal,
            "Attach without a pane command should not change focus"
        );
    }

    #[rstest::rstest]
    fn attach_without_pane_command_returns_no_commands() {
        // Given orb started without a pane command.
        let mut state = AppState::default();

        // When handling Attach.
        let commands = IntentHandler::handle(&Intent::Attach, &mut state);

        // Then the loop has nothing to do.
        assert!(
            commands.is_empty(),
            "Attach without a pane command should return no commands"
        );
    }

    #[rstest::rstest]
    fn detach_sets_focus_normal() {
        // Given keys going to the pane.
        let mut state = AppState {
            focus: Focus::Attached,
            ..state_with_pane_command()
        };

        // When handling Detach.
        IntentHandler::handle(&Intent::Detach, &mut state);

        // Then keys drive orb again.
        assert_eq!(
            state.focus,
            Focus::Normal,
            "Detach should return focus to orb"
        );
    }

    #[rstest::rstest]
    fn detach_returns_detach_command() {
        // Given keys going to the pane.
        let mut state = AppState {
            focus: Focus::Attached,
            ..state_with_pane_command()
        };

        // When handling Detach.
        let commands = IntentHandler::handle(&Intent::Detach, &mut state);

        // Then the loop is told to detach.
        assert_eq!(
            commands,
            vec![Command::Detach],
            "Detach should return Command::Detach"
        );
    }
}
