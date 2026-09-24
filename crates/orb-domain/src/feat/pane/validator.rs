//! Checks whether the user can attach to the terminal pane.

use wherror::Error;

use crate::AppState;

/// Why attaching to the terminal pane can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum AttachError {
    /// orb was started without `-- <cmd…>`.
    NoPaneCommand,
}

/// Allow attaching only when orb has a command for the pane to run.
///
/// # Errors
///
/// Returns [`AttachError::NoPaneCommand`] if orb was started without a pane command.
pub fn validate_attach(state: &AppState) -> Result<(), AttachError> {
    if state.pane_argv.is_empty() {
        return Err(AttachError::NoPaneCommand);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{AttachError, validate_attach};
    use crate::AppState;

    #[rstest::rstest]
    fn attach_rejected_without_pane_command() {
        // Given orb started without a pane command.
        let state = AppState::default();

        // When validating attach.
        let result = validate_attach(&state);

        // Then validation fails with NoPaneCommand.
        assert_eq!(
            result,
            Err(AttachError::NoPaneCommand),
            "attach needs a pane command"
        );
    }

    #[rstest::rstest]
    fn attach_allowed_with_pane_command() {
        // Given orb started with `-- cat`.
        let state = AppState {
            pane_argv: vec!["cat".into()],
            ..AppState::default()
        };

        // When validating attach.
        let result = validate_attach(&state);

        // Then validation passes.
        assert_eq!(result, Ok(()), "a pane command allows attach");
    }
}
