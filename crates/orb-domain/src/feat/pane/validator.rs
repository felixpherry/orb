//! Checks whether the user can attach to the selected thread's session.

use wherror::Error;

use crate::AppState;
use crate::feat::sessions::state::ThreadStatus;

/// Why attaching can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum AttachError {
    /// No thread is selected.
    NoSelection,
    /// Claude no longer knows the selected thread's session.
    Gone,
}

/// Allow attaching only to a selected thread whose session still exists.
///
/// # Errors
///
/// Returns [`AttachError::NoSelection`] without a selected thread, and
/// [`AttachError::Gone`] if the selected thread's session is gone.
pub fn validate_attach(state: &AppState) -> Result<(), AttachError> {
    match state.sessions.selected_thread() {
        None => Err(AttachError::NoSelection),
        Some(thread) if thread.status == ThreadStatus::Gone => Err(AttachError::Gone),
        Some(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{AttachError, validate_attach};
    use crate::AppState;

    #[rstest::rstest]
    fn attach_rejected_without_selection() {
        // Given no selected thread.
        let state = AppState::default();

        // When validating attach.
        let result = validate_attach(&state);

        // Then validation fails with NoSelection.
        assert_eq!(
            result,
            Err(AttachError::NoSelection),
            "attach needs a selected thread"
        );
    }
}
