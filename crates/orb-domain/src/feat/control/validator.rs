//! Checks a request must pass before it changes anything.

use crate::AppState;
use crate::feat::control::protocol::{ApiError, ErrorCode};
use crate::feat::sessions::validator::{NewSessionError, validate_new_session};
use crate::ids::SessionId;

/// Refuses a change to settled session `session` with `settled`.
pub(crate) fn not_settled(app: &AppState, session: SessionId) -> Result<(), ApiError> {
    match app.sessions.session(session) {
        Some(found) if found.settled_at.is_some() => {
            Err(ApiError::new(ErrorCode::Settled, "session is settled"))
        }
        _ => Ok(()),
    }
}

/// Refuses a new session while one is starting, with `starting`.
pub(crate) fn not_starting(app: &AppState) -> Result<(), ApiError> {
    match validate_new_session(app) {
        Ok(()) => Ok(()),
        Err(NewSessionError::Starting) => Err(ApiError::new(
            ErrorCode::Starting,
            "a session is already starting",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{not_settled, not_starting};
    use crate::feat::control::protocol::ErrorCode;
    use crate::ids::SessionId;
    use crate::intent_handler::test_support::{settle, state_with, stopped};

    #[rstest::rstest]
    fn settled_session_is_refused() {
        // Given session 1, settled.
        let app = settle(state_with(vec![stopped(1)], 1), &[1]);

        // When checking a change to session 1.
        let checked = not_settled(&app, SessionId(1));

        // Then it is refused with settled.
        assert_eq!(
            checked.map_err(|error| error.code),
            Err(ErrorCode::Settled),
            "a settled session can't be changed"
        );
    }

    #[rstest::rstest]
    fn new_session_is_refused_while_one_starts() {
        // Given a session already starting.
        let mut app = state_with(vec![stopped(1)], 1);
        app.sessions.starting = true;

        // When checking a new session.
        let checked = not_starting(&app);

        // Then it is refused with starting.
        assert_eq!(
            checked.map_err(|error| error.code),
            Err(ErrorCode::Starting),
            "only one session starts at a time"
        );
    }
}
