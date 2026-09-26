//! Checks whether the user's sidebar actions can proceed: starting a session,
//! pinning, settling or deleting the selected thread, and opening or closing
//! the Settled shelf.

use wherror::Error;

use crate::AppState;
use crate::feat::sessions::state::SidebarItem;

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

/// Why settling or un-settling can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum ToggleSettleError {
    /// The cursor isn't on a thread.
    NoThread,
    /// Claude is running a turn or waiting on the user.
    InProgress,
}

/// Allow settling a thread only between turns; un-settling is always allowed.
///
/// # Errors
///
/// Returns [`ToggleSettleError::NoThread`] without a selected thread, and
/// [`ToggleSettleError::InProgress`] if the selected thread isn't settled and
/// has a turn underway.
pub fn validate_toggle_settle(state: &AppState) -> Result<(), ToggleSettleError> {
    match state.sessions.selected_thread() {
        None => Err(ToggleSettleError::NoThread),
        Some(thread) if thread.settled_at.is_none() && thread.status.in_progress() => {
            Err(ToggleSettleError::InProgress)
        }
        Some(_) => Ok(()),
    }
}

/// Why pinning or unpinning can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum TogglePinError {
    /// The cursor isn't on a thread.
    NoThread,
}

/// Allow pinning or unpinning the selected thread.
///
/// # Errors
///
/// Returns [`TogglePinError::NoThread`] without a selected thread.
pub fn validate_toggle_pin(state: &AppState) -> Result<(), TogglePinError> {
    match state.sessions.selected_thread() {
        None => Err(TogglePinError::NoThread),
        Some(_) => Ok(()),
    }
}

/// Why deleting can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum DeleteError {
    /// The cursor isn't on a thread.
    NoThread,
}

/// Allow deleting the selected thread, whatever it is doing.
///
/// # Errors
///
/// Returns [`DeleteError::NoThread`] without a selected thread.
pub fn validate_delete(state: &AppState) -> Result<(), DeleteError> {
    match state.sessions.selected_thread() {
        None => Err(DeleteError::NoThread),
        Some(_) => Ok(()),
    }
}

/// Why opening the Settled shelf can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum OpenShelfError {
    /// The cursor isn't on the shelf's header.
    NotOnShelf,
}

/// Allow opening the Settled shelf from its header.
///
/// # Errors
///
/// Returns [`OpenShelfError::NotOnShelf`] unless the cursor is on the header.
pub fn validate_open_shelf(state: &AppState) -> Result<(), OpenShelfError> {
    match state.sessions.cursor {
        Some(SidebarItem::SettledShelf) => Ok(()),
        _ => Err(OpenShelfError::NotOnShelf),
    }
}

/// Why closing the Settled shelf can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum CloseShelfError {
    /// The cursor is neither on the shelf's header nor in the open shelf.
    NotInShelf,
}

/// Allow closing the Settled shelf from its header or from a settled thread
/// in the open shelf.
///
/// # Errors
///
/// Returns [`CloseShelfError::NotInShelf`] when the cursor is anywhere else.
pub fn validate_close_shelf(state: &AppState) -> Result<(), CloseShelfError> {
    let sessions = &state.sessions;
    match sessions.cursor {
        Some(SidebarItem::SettledShelf) => Ok(()),
        Some(SidebarItem::Thread(_))
            if sessions.shelf_open
                && sessions
                    .selected_thread()
                    .is_some_and(|thread| thread.settled_at.is_some()) =>
        {
            Ok(())
        }
        _ => Err(CloseShelfError::NotInShelf),
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::{ToggleSettleError, validate_toggle_settle};
    use crate::AppState;
    use crate::feat::sessions::state::{
        Project, ProjectId, Sessions, SidebarItem, Thread, ThreadId, ThreadStatus,
    };

    #[rstest::rstest]
    fn settle_is_refused_while_working() {
        // Given a selected thread running a turn.
        let state = AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "work".into(),
                    root: "/work".into(),
                    created_at: SystemTime::UNIX_EPOCH,
                    threads: vec![Thread {
                        id: ThreadId(1),
                        title: None,
                        cwd: "/work".into(),
                        transcript: None,
                        status: ThreadStatus::Working,
                        turn_started_at: None,
                        attach_argv: vec![],
                        branch: None,
                        pinned_at: None,
                        settled_at: None,
                        active_since: SystemTime::UNIX_EPOCH,
                        last_activity_at: SystemTime::UNIX_EPOCH,
                        unseen: false,
                    }],
                }],
                cursor: Some(SidebarItem::Thread(ThreadId(1))),
                ..Sessions::default()
            },
            ..AppState::default()
        };

        // When validating a settle.
        let result = validate_toggle_settle(&state);

        // Then validation fails with InProgress.
        assert_eq!(
            result,
            Err(ToggleSettleError::InProgress),
            "a working thread can't be settled"
        );
    }
}
