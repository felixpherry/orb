//! Checks whether the user can start a new session.

use wherror::Error;

use crate::AppState;

/// Why starting a new session can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum NewSessionError {
    /// A new session is already being created.
    AlreadyStarting,
}

/// Allow one new session at a time.
///
/// # Errors
///
/// Returns [`NewSessionError::AlreadyStarting`] while a create is in flight.
pub fn validate_new_session(state: &AppState) -> Result<(), NewSessionError> {
    if state.sessions.starting {
        return Err(NewSessionError::AlreadyStarting);
    }
    Ok(())
}
