//! Checks whether the user's git actions on the selected thread can proceed:
//! changing its workspace, and switching its branch.

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

/// What the mode line says when a branch switch is refused as
/// [`SwitchBranchError::Busy`].
pub const BUSY_DIRECTORY: &str = "Claude is working in this directory";

/// Why switching the selected thread's branch can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum SwitchBranchError {
    /// The cursor isn't on a thread.
    NoThread,
    /// A session is already being started.
    Starting,
    /// A thread in the same directory is working or waiting.
    Busy,
}

/// Allow switching the selected thread's branch unless a start is in flight or
/// a turn is underway in its directory, which a checkout would change under it.
///
/// # Errors
///
/// Returns [`SwitchBranchError::NoThread`] without a selected thread,
/// [`SwitchBranchError::Starting`] while a start is in flight, and
/// [`SwitchBranchError::Busy`] while any thread in the same directory is in
/// progress.
pub fn validate_switch_branch(state: &AppState) -> Result<(), SwitchBranchError> {
    let sessions = &state.sessions;
    match sessions.selected_thread() {
        None => Err(SwitchBranchError::NoThread),
        Some(_) if sessions.starting => Err(SwitchBranchError::Starting),
        Some(selected)
            if sessions
                .threads()
                .any(|thread| thread.cwd == selected.cwd && thread.status.in_progress()) =>
        {
            Err(SwitchBranchError::Busy)
        }
        Some(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::{
        ChangeWorkspaceError, SwitchBranchError, validate_change_workspace, validate_switch_branch,
    };
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
                    draft: None,
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

    /// [`selected`] plus thread 2, working in `cwd`.
    fn with_working_thread(cwd: &str) -> AppState {
        let mut state = selected(None);
        if let Some(project) = state.sessions.projects.first_mut() {
            let working = project.threads.first().map(|thread| Thread {
                id: ThreadId(2),
                cwd: cwd.into(),
                status: ThreadStatus::Working,
                ..thread.clone()
            });
            project.threads.extend(working);
        }
        state
    }

    #[rstest::rstest]
    fn switch_branch_rejected_while_a_same_cwd_thread_works() {
        // Given another thread in the selected thread's directory, working.
        let state = with_working_thread("/work");

        // When validating a branch switch.
        let result = validate_switch_branch(&state);

        // Then validation fails with Busy.
        assert_eq!(
            result,
            Err(SwitchBranchError::Busy),
            "a checkout would change files under the running turn"
        );
    }

    #[rstest::rstest]
    fn switch_branch_allowed_while_a_thread_elsewhere_works() {
        // Given another thread working in a different directory.
        let state = with_working_thread("/work-tree");

        // When validating a branch switch.
        let result = validate_switch_branch(&state);

        // Then it is allowed.
        assert_eq!(result, Ok(()), "a turn elsewhere doesn't block a checkout");
    }
}
