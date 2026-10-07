//! Checks whether the user can open a tool for the sidebar's selection.

use wherror::Error;

use crate::AppState;

/// Why opening a tool for the selection can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum OpenToolError {
    /// The cursor isn't on a session.
    NoSelection,
}

/// Allow opening a tool when a session is selected.
///
/// # Errors
///
/// Returns [`OpenToolError::NoSelection`] without a selected session.
pub fn validate_open_tool(state: &AppState) -> Result<(), OpenToolError> {
    match state.sessions.selected_session() {
        Some(_) => Ok(()),
        None => Err(OpenToolError::NoSelection),
    }
}

#[cfg(test)]
mod tests {
    use crate::feat::harness::HarnessId;
    use std::time::SystemTime;

    use super::{OpenToolError, validate_open_tool};
    use crate::AppState;
    use crate::feat::sessions::state::{
        PaneId, PaneLaunch, Project, ProjectId, ProjectKind, SessionId, Sessions, SidebarItem,
        Thread, ThreadId, ThreadStatus, sessions_for,
    };

    /// One project at `/work` with a thread, the cursor on `cursor`.
    fn state_at(cursor: Option<SidebarItem>) -> AppState {
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
                cursor,
                ..Sessions::default()
            },
            ..AppState::default()
        };
        state.sessions.sessions = sessions_for(&state.sessions.projects);
        state
    }

    #[rstest::rstest]
    #[case(None)]
    #[case(Some(SidebarItem::SettledShelf))]
    fn open_tool_rejected_without_a_session(#[case] cursor: Option<SidebarItem>) {
        // Given a cursor on no session.
        let state = state_at(cursor);

        // When validating opening a tool.
        let result = validate_open_tool(&state);

        // Then validation fails with NoSelection.
        assert_eq!(
            result,
            Err(OpenToolError::NoSelection),
            "a tool needs a session's directory"
        );
    }

    #[rstest::rstest]
    fn open_tool_allowed_on_a_session() {
        // Given a selected session.
        let state = state_at(Some(SidebarItem::Session(SessionId(1))));

        // When validating opening a tool.
        let result = validate_open_tool(&state);

        // Then it is allowed.
        assert_eq!(result, Ok(()), "a session has a directory");
    }
}
