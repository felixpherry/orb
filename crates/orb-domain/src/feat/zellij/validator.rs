//! Checks whether the user can open a tool for the sidebar's selection.

use wherror::Error;

use crate::AppState;

/// Why opening a tool for the selection can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum OpenToolError {
    /// The cursor isn't on a session, a draft or a group's draft.
    NoSelection,
}

/// Allow opening a tool when a session, a draft or a group draft is
/// selected.
///
/// # Errors
///
/// Returns [`OpenToolError::NoSelection`] without a selected session, draft
/// or group draft.
pub fn validate_open_tool(state: &AppState) -> Result<(), OpenToolError> {
    match (
        state.sessions.selected_draft(),
        state.sessions.selected_session(),
        state.sessions.selected_group(),
    ) {
        (None, None, None) => Err(OpenToolError::NoSelection),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use crate::feat::harness::HarnessId;
    use std::time::SystemTime;

    use super::{OpenToolError, validate_open_tool};
    use crate::AppState;
    use crate::feat::sessions::state::{
        Draft, DraftWorkspace, Group, GroupDefaults, GroupDraft, GroupId, GroupKind, PaneId,
        PaneLaunch, Project, ProjectId, ProjectKind, SessionId, Sessions, SidebarItem, Thread,
        ThreadId, ThreadStatus, sessions_for,
    };

    /// One project at `/work` with a thread, a draft and group 9 (still a
    /// draft), the cursor on `cursor`.
    fn state_at(cursor: Option<SidebarItem>) -> AppState {
        let mut state = AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "work".into(),
                    root: "/work".into(),
                    created_at: SystemTime::UNIX_EPOCH,
                    removed: false,
                    draft: Some(Draft {
                        harness: HarnessId::new("claude"),
                        workspace: DraftWorkspace::Local,
                        branch: None,
                        model: None,
                        permission: None,
                        created_at: SystemTime::UNIX_EPOCH,
                        repo: true,
                        from: None,
                    }),
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
                            command: vec![],
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
                    groups: vec![Group {
                        id: GroupId(9),
                        kind: GroupKind::Research,
                        name: "tokio-cancel".into(),
                        dir: Some("/orb/research/tokio-cancel".into()),
                        branch: None,
                        created_at: SystemTime::UNIX_EPOCH,
                        pinned_at: None,
                        settled_at: None,
                        active_since: SystemTime::UNIX_EPOCH,
                        draft: Some(GroupDraft::default()),
                        defaults: GroupDefaults {
                            harness: HarnessId::new("claude"),
                            model: None,
                            permission: None,
                        },
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
    #[case(SidebarItem::Session(SessionId(1)))]
    #[case(SidebarItem::Draft(ProjectId(1)))]
    fn open_tool_allowed_on_a_session_or_draft(#[case] cursor: SidebarItem) {
        // Given a selected thread or draft.
        let state = state_at(Some(cursor));

        // When validating opening a tool.
        let result = validate_open_tool(&state);

        // Then it is allowed.
        assert_eq!(result, Ok(()), "a session or draft has a directory");
    }

    #[rstest::rstest]
    fn open_tool_allowed_on_a_group_draft() {
        // Given a selected group draft.
        let cursor = SidebarItem::GroupDraft(GroupId(9));
        let state = state_at(Some(cursor));

        // When validating opening a tool.
        let result = validate_open_tool(&state);

        // Then it is allowed.
        assert_eq!(result, Ok(()), "a group draft has a directory");
    }
}
