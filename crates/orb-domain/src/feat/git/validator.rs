//! Checks whether the user's git actions on the selected thread can proceed:
//! changing its workspace.

use wherror::Error;

use crate::AppState;

/// Why changing the selected thread's workspace can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum ChangeWorkspaceError {
    /// The cursor isn't on a thread.
    NoThread,
    /// A session is already being started.
    Starting,
    /// The thread has had its first prompt. `worktree` is whether it runs in
    /// a worktree rather than the project's root checkout.
    Locked { worktree: bool },
}

/// Allow changing the selected thread's workspace only before its first
/// prompt, one start at a time.
///
/// # Errors
///
/// Returns [`ChangeWorkspaceError::NoThread`] without a selected thread,
/// [`ChangeWorkspaceError::Starting`] while a start is in flight, and
/// [`ChangeWorkspaceError::Locked`] once the thread has a transcript or a turn
/// underway.
pub fn validate_change_workspace(state: &AppState) -> Result<(), ChangeWorkspaceError> {
    let sessions = &state.sessions;
    match (sessions.selected_thread(), sessions.selected_project()) {
        (Some(_), Some(_)) if sessions.starting => Err(ChangeWorkspaceError::Starting),
        (Some(thread), Some(project))
            if thread.transcript.is_some() || thread.status.in_progress() =>
        {
            Err(ChangeWorkspaceError::Locked {
                worktree: thread.cwd != project.root,
            })
        }
        (Some(_), Some(_)) => Ok(()),
        _ => Err(ChangeWorkspaceError::NoThread),
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::{ChangeWorkspaceError, validate_change_workspace};
    use crate::AppState;
    use crate::feat::sessions::state::{
        Project, ProjectId, Sessions, SidebarItem, Thread, ThreadId, ThreadStatus,
    };

    /// One project at `/work` whose only thread, selected, runs in the root
    /// with `transcript`.
    fn selected(transcript: Option<&str>) -> AppState {
        AppState {
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
                        transcript: transcript.map(Into::into),
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
                cursor: Some(SidebarItem::Thread(ThreadId(1))),
                ..Sessions::default()
            },
            ..AppState::default()
        }
    }

    #[rstest::rstest]
    fn change_workspace_rejected_when_the_thread_has_a_transcript() {
        // Given a selected thread in the root checkout that has a transcript.
        let state = selected(Some("/claude/t1.jsonl"));

        // When validating a workspace change.
        let result = validate_change_workspace(&state);

        // Then it is locked to the local checkout.
        assert_eq!(
            result,
            Err(ChangeWorkspaceError::Locked { worktree: false }),
            "a prompted thread's workspace should be locked"
        );
    }

    #[rstest::rstest]
    fn change_workspace_rejected_while_starting() {
        // Given a prompt-less selected thread while a start is in flight.
        let state = {
            let mut state = selected(None);
            state.sessions.starting = true;
            state
        };

        // When validating a workspace change.
        let result = validate_change_workspace(&state);

        // Then validation fails with Starting.
        assert_eq!(
            result,
            Err(ChangeWorkspaceError::Starting),
            "one start at a time"
        );
    }

    #[rstest::rstest]
    fn change_workspace_allowed_before_the_first_prompt() {
        // Given a prompt-less selected thread.
        let state = selected(None);

        // When validating a workspace change.
        let result = validate_change_workspace(&state);

        // Then it is allowed.
        assert_eq!(result, Ok(()), "a prompt-less thread can change workspace");
    }
}
