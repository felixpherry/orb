//! Checks whether the user can open a tool for the sidebar's selection.

use wherror::Error;

use crate::AppState;

/// Why opening a tool for the selection can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum OpenToolError {
    /// The cursor isn't on a thread or a draft.
    NoSelection,
}

/// Allow opening a tool when a thread or a draft is selected.
///
/// # Errors
///
/// Returns [`OpenToolError::NoSelection`] without a selected thread or draft.
pub fn validate_open_tool(state: &AppState) -> Result<(), OpenToolError> {
    match (
        state.sessions.selected_draft(),
        state.sessions.selected_thread(),
    ) {
        (None, None) => Err(OpenToolError::NoSelection),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::{OpenToolError, validate_open_tool};
    use crate::AppState;
    use crate::feat::sessions::state::{
        Draft, DraftWorkspace, Project, ProjectId, Sessions, SidebarItem, Thread, ThreadId,
        ThreadStatus,
    };

    /// One project at `/work` with a thread and a draft, the cursor on
    /// `cursor`.
    fn state_at(cursor: Option<SidebarItem>) -> AppState {
        AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "work".into(),
                    root: "/work".into(),
                    created_at: SystemTime::UNIX_EPOCH,
                    draft: Some(Draft {
                        workspace: DraftWorkspace::Local,
                        branch: None,
                        model: None,
                        permission: None,
                        created_at: SystemTime::UNIX_EPOCH,
                        repo: true,
                        from: None,
                    }),
                    threads: vec![Thread {
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
                        last_activity_at: SystemTime::UNIX_EPOCH,
                        unseen: false,
                    }],
                }],
                cursor,
                ..Sessions::default()
            },
            ..AppState::default()
        }
    }

    #[rstest::rstest]
    #[case(None)]
    #[case(Some(SidebarItem::SettledShelf))]
    fn open_tool_rejected_without_a_thread_or_draft(#[case] cursor: Option<SidebarItem>) {
        // Given a cursor on neither a thread nor a draft.
        let state = state_at(cursor);

        // When validating opening a tool.
        let result = validate_open_tool(&state);

        // Then validation fails with NoSelection.
        assert_eq!(
            result,
            Err(OpenToolError::NoSelection),
            "a tool needs a thread's or draft's directory"
        );
    }

    #[rstest::rstest]
    #[case(SidebarItem::Thread(ThreadId(1)))]
    #[case(SidebarItem::Draft(ProjectId(1)))]
    fn open_tool_allowed_on_a_thread_or_draft(#[case] cursor: SidebarItem) {
        // Given a selected thread or draft.
        let state = state_at(Some(cursor));

        // When validating opening a tool.
        let result = validate_open_tool(&state);

        // Then it is allowed.
        assert_eq!(result, Ok(()), "a thread or draft has a directory");
    }
}
