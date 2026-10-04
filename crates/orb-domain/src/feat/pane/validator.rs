//! Checks whether the user can attach to the selected thread's session, or
//! detach it from its pane.

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

/// Why detaching can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum DetachError {
    /// No thread is selected.
    NoSelection,
    /// The selected thread has no pane.
    NotAttached,
}

/// Allow detaching only a selected thread that is attached.
///
/// # Errors
///
/// Returns [`DetachError::NoSelection`] without a selected thread, and
/// [`DetachError::NotAttached`] if the selected thread isn't attached.
pub fn validate_detach(state: &AppState) -> Result<(), DetachError> {
    match state.sessions.selected_id() {
        None => Err(DetachError::NoSelection),
        Some(id) if !state.attached.contains(&id) => Err(DetachError::NotAttached),
        Some(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use crate::feat::harness::HarnessId;
    use std::time::SystemTime;

    use super::{AttachError, DetachError, validate_attach, validate_detach};
    use crate::AppState;
    use crate::feat::sessions::state::{
        Project, ProjectId, ProjectKind, Sessions, SidebarItem, Thread, ThreadId, ThreadStatus,
    };

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

    #[rstest::rstest]
    fn detach_rejected_without_selection() {
        // Given no selected thread.
        let state = AppState::default();

        // When validating detach.
        let result = validate_detach(&state);

        // Then validation fails with NoSelection.
        assert_eq!(
            result,
            Err(DetachError::NoSelection),
            "detach needs a selected thread"
        );
    }

    #[rstest::rstest]
    fn detach_rejected_when_not_attached() {
        // Given thread 1 selected but not attached.
        let state = AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "work".into(),
                    root: "/work".into(),
                    created_at: SystemTime::UNIX_EPOCH,
                    removed: false,
                    draft: None,
                    threads: vec![Thread {
                        harness: HarnessId::new("claude"),
                        id: ThreadId(1),
                        title: None,
                        cwd: "/work".into(),
                        transcript: None,
                        status: ThreadStatus::Idle,
                        turn_started_at: None,
                        attach_argv: vec![],
                        branch: None,
                        pinned_at: None,
                        settled_at: None,
                        active_since: SystemTime::UNIX_EPOCH,
                        created_at: SystemTime::UNIX_EPOCH,
                        last_activity_at: SystemTime::UNIX_EPOCH,
                        unseen: false,
                        group: None,
                        model: None,
                        permission: None,
                    }],
                    groups: vec![],
                    kind: ProjectKind::Normal,
                }],
                cursor: Some(SidebarItem::Thread(ThreadId(1))),
                ..Sessions::default()
            },
            ..AppState::default()
        };

        // When validating detach.
        let result = validate_detach(&state);

        // Then validation fails with NotAttached.
        assert_eq!(
            result,
            Err(DetachError::NotAttached),
            "detach needs the selected thread attached"
        );
    }
}
