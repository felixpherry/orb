//! Checks whether the user can attach to the selected session, or detach it
//! from its panes.

use wherror::Error;

use crate::AppState;

/// Why attaching can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum AttachError {
    /// No session is selected, or it is being deleted.
    NoSelection,
}

/// Allow attaching only to a selected session that isn't being deleted.
///
/// # Errors
///
/// Returns [`AttachError::NoSelection`] without such a session.
pub fn validate_attach(state: &AppState) -> Result<(), AttachError> {
    match state.sessions.selected_session() {
        None => Err(AttachError::NoSelection),
        Some(_) => Ok(()),
    }
}

/// Why detaching can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum DetachError {
    /// No session is selected.
    NoSelection,
    /// The selected session isn't attached.
    NotAttached,
}

/// Allow detaching only a selected session that is attached.
///
/// # Errors
///
/// Returns [`DetachError::NoSelection`] without a selected session, and
/// [`DetachError::NotAttached`] if the selected session isn't attached.
pub fn validate_detach(state: &AppState) -> Result<(), DetachError> {
    match state.sessions.selected_session().map(|session| session.id) {
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
        PaneId, PaneLaunch, Project, ProjectId, ProjectKind, SessionId, Sessions, SidebarItem,
        Thread, ThreadId, ThreadStatus, sessions_for,
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
        let mut state = AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "work".into(),
                    root: "/work".into(),
                    created_at: SystemTime::UNIX_EPOCH,
                    removed: false,
                    repo: true,
                    threads: vec![Thread {
                        last_session: None,
                        harness: HarnessId::new("claude"),
                        id: ThreadId(1),
                        title: None,
                        cwd: "/work".into(),
                        transcript: None,
                        status: ThreadStatus::Idle,
                        turn_started_at: None,
                        pane: Some(PaneLaunch {
                            pane: PaneId(1),
                            session: SessionId(1),
                        }),
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
                    kind: ProjectKind::Normal,
                }],
                cursor: Some(SidebarItem::Session(SessionId(1))),
                ..Sessions::default()
            },
            ..AppState::default()
        };

        state.sessions.sessions = sessions_for(&state.sessions.projects);

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
