//! Checks whether the user's git actions on the selected thread or draft can
//! proceed: changing its workspace, and switching its branch (also on a
//! Feature group's card, for the group's worktree).

use std::path::Path;

use wherror::Error;

use crate::AppState;
use crate::feat::sessions::state::{DraftWorkspace, Sessions};

/// Why changing the selected thread's or draft's workspace can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum ChangeWorkspaceError {
    /// The cursor isn't on a thread or a draft.
    NoSelection,
    /// The thread is in a group, whose directory is fixed.
    Grouped,
    /// A session is already being started.
    Starting,
    /// The thread has had its first prompt. `worktree` is whether it runs in
    /// a worktree rather than the project's root checkout.
    Locked { worktree: bool },
}

/// Allow changing the selected draft's workspace, or the selected thread's
/// before its first prompt, one start at a time.
///
/// # Errors
///
/// Returns [`ChangeWorkspaceError::NoSelection`] without a selected thread or
/// draft, [`ChangeWorkspaceError::Grouped`] on a thread in a group,
/// [`ChangeWorkspaceError::Starting`] while a start is in flight, and
/// [`ChangeWorkspaceError::Locked`] once the thread has a transcript or a turn
/// underway.
pub fn validate_change_workspace(state: &AppState) -> Result<(), ChangeWorkspaceError> {
    let sessions = &state.sessions;
    match (
        sessions.selected_draft(),
        sessions.selected_thread(),
        sessions.selected_project(),
    ) {
        (None, None, _) | (None, Some(_), None) => Err(ChangeWorkspaceError::NoSelection),
        (None, Some(thread), _) if thread.group.is_some() => Err(ChangeWorkspaceError::Grouped),
        _ if sessions.starting => Err(ChangeWorkspaceError::Starting),
        (None, Some(thread), Some(project))
            if thread.transcript.is_some() || thread.status.in_progress() =>
        {
            Err(ChangeWorkspaceError::Locked {
                worktree: thread.cwd != project.root,
            })
        }
        _ => Ok(()),
    }
}

/// What the mode line says when a branch switch is refused as
/// [`SwitchBranchError::Busy`].
pub const BUSY_DIRECTORY: &str = "A session is working in this directory";

/// Why switching the selected session's or draft's branch can't proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum SwitchBranchError {
    /// The cursor isn't on a draft or a session with an agent.
    NoSelection,
    /// The session's lead thread is in a group.
    Grouped,
    /// A session is already being started.
    Starting,
    /// A thread in the directory the checkout would change is working or
    /// waiting.
    Busy,
}

/// Allow switching the selected draft's branch, or that of the selected
/// session's lead thread, unless a start is in flight or a turn is underway
/// in the directory a checkout would change. A local draft checks out in the
/// project's root and an existing-worktree draft in its worktree; a
/// new-worktree draft only records its base, and a draft whose project isn't
/// a git repository is offered `git init` instead.
///
/// # Errors
///
/// Returns [`SwitchBranchError::NoSelection`] without a selected draft or a
/// session with an agent, [`SwitchBranchError::Grouped`] when the lead
/// thread is in a group, [`SwitchBranchError::Starting`] while a start is in
/// flight, and [`SwitchBranchError::Busy`] while any thread in the same
/// directory is in progress.
pub fn validate_switch_branch(state: &AppState) -> Result<(), SwitchBranchError> {
    let sessions = &state.sessions;
    match (sessions.selected_draft(), sessions.selected_thread()) {
        (None, None) => Err(SwitchBranchError::NoSelection),
        (None, Some(thread)) if thread.group.is_some() => Err(SwitchBranchError::Grouped),
        _ if sessions.starting => Err(SwitchBranchError::Starting),
        (Some((_, draft)), _) if !draft.repo => Ok(()),
        (Some((project, draft)), _) => match &draft.workspace {
            DraftWorkspace::Existing(path) => not_busy(sessions, path),
            DraftWorkspace::NewWorktree => Ok(()),
            DraftWorkspace::Local => not_busy(sessions, &project.root),
        },
        (None, Some(thread)) => not_busy(sessions, &thread.cwd),
    }
}

/// [`SwitchBranchError::Busy`] while any thread in `dir` is in progress.
fn not_busy(sessions: &Sessions, dir: &Path) -> Result<(), SwitchBranchError> {
    if sessions
        .threads()
        .any(|thread| thread.cwd == dir && thread.status.in_progress())
    {
        Err(SwitchBranchError::Busy)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::feat::harness::HarnessId;
    use std::time::SystemTime;

    use super::{
        ChangeWorkspaceError, SwitchBranchError, validate_change_workspace, validate_switch_branch,
    };
    use crate::AppState;
    use crate::feat::sessions::state::{
        Draft, DraftWorkspace, GroupId, PaneId, PaneLaunch, Project, ProjectId, ProjectKind,
        SessionId, Sessions, SidebarItem, Thread, ThreadId, ThreadStatus, sessions_for,
    };

    /// One project at `/work` whose only thread, selected, runs in the root
    /// with `transcript`.
    fn selected(transcript: Option<&str>) -> AppState {
        let mut state = AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "work".into(),
                    root: "/work".into(),
                    created_at: SystemTime::UNIX_EPOCH,
                    removed: false,
                    draft: None,
                    threads: vec![Thread {
                        last_session: None,
                        harness: HarnessId::new("claude"),
                        id: ThreadId(1),
                        title: None,
                        cwd: "/work".into(),
                        transcript: transcript.map(Into::into),
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
                    groups: vec![],
                    kind: ProjectKind::Normal,
                }],
                cursor: Some(SidebarItem::Session(SessionId(1))),
                ..Sessions::default()
            },
            ..AppState::default()
        };
        state.sessions.sessions = sessions_for(&state.sessions.projects);
        state
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

    /// [`with_working_thread`] in the root, plus the project's draft in
    /// `workspace`, selected.
    fn draft_beside_busy_root(workspace: DraftWorkspace) -> AppState {
        let mut state = with_working_thread("/work");
        if let Some(project) = state.sessions.projects.first_mut() {
            project.draft = Some(Draft {
                harness: HarnessId::new("claude"),
                workspace,
                branch: None,
                model: None,
                permission: None,
                created_at: SystemTime::UNIX_EPOCH,
                repo: true,
                from: None,
            });
        }
        state.sessions.cursor = Some(SidebarItem::Draft(ProjectId(1)));
        state
    }

    #[rstest::rstest]
    fn change_workspace_allowed_on_a_draft() {
        // Given a selected local draft.
        let state = draft_beside_busy_root(DraftWorkspace::Local);

        // When validating a workspace change.
        let result = validate_change_workspace(&state);

        // Then it is allowed.
        assert_eq!(result, Ok(()), "a draft can always change workspace");
    }

    #[rstest::rstest]
    fn change_workspace_on_a_draft_rejected_while_starting() {
        // Given a selected draft while a start is in flight.
        let state = {
            let mut state = draft_beside_busy_root(DraftWorkspace::Local);
            state.sessions.starting = true;
            state
        };

        // When validating a workspace change.
        let result = validate_change_workspace(&state);

        // Then validation fails with Starting.
        assert_eq!(
            result,
            Err(ChangeWorkspaceError::Starting),
            "a draft can't change while a start may be reading it"
        );
    }

    #[rstest::rstest]
    fn switch_branch_allowed_on_an_existing_worktree_draft_while_the_root_is_busy() {
        // Given a selected draft in an existing worktree and a thread working
        // in the root.
        let state = draft_beside_busy_root(DraftWorkspace::Existing("/wt/feat".into()));

        // When validating a branch switch.
        let result = validate_switch_branch(&state);

        // Then it is allowed.
        assert_eq!(
            result,
            Ok(()),
            "the draft's checkout happens in its worktree, not the busy root"
        );
    }

    #[rstest::rstest]
    fn switch_branch_rejected_on_an_existing_worktree_draft_while_that_worktree_is_busy() {
        // Given a selected draft in `/work-tree`, where a thread is working.
        let state = {
            let mut state = with_working_thread("/work-tree");
            if let Some(project) = state.sessions.projects.first_mut() {
                project.draft = Some(Draft {
                    harness: HarnessId::new("claude"),
                    workspace: DraftWorkspace::Existing("/work-tree".into()),
                    branch: None,
                    model: None,
                    permission: None,
                    created_at: SystemTime::UNIX_EPOCH,
                    repo: true,
                    from: None,
                });
            }
            state.sessions.cursor = Some(SidebarItem::Draft(ProjectId(1)));
            state
        };

        // When validating a branch switch.
        let result = validate_switch_branch(&state);

        // Then validation fails with Busy.
        assert_eq!(
            result,
            Err(SwitchBranchError::Busy),
            "a checkout would change files under the worktree's running turn"
        );
    }

    #[rstest::rstest]
    fn switch_branch_allowed_on_a_non_git_draft_while_the_root_is_busy() {
        // Given a selected local draft of a project that isn't a git
        // repository, and a thread working in the root.
        let state = {
            let mut state = draft_beside_busy_root(DraftWorkspace::Local);
            if let Some(draft) = state.sessions.draft_mut(ProjectId(1)) {
                draft.repo = false;
            }
            state
        };

        // When validating a branch switch.
        let result = validate_switch_branch(&state);

        // Then it is allowed: it only offers `git init`.
        assert_eq!(result, Ok(()), "a non-git draft is offered git init");
    }

    #[rstest::rstest]
    fn switch_branch_rejected_on_a_local_draft_while_the_root_is_busy() {
        // Given a selected local draft and a thread working in the root.
        let state = draft_beside_busy_root(DraftWorkspace::Local);

        // When validating a branch switch.
        let result = validate_switch_branch(&state);

        // Then validation fails with Busy.
        assert_eq!(
            result,
            Err(SwitchBranchError::Busy),
            "a local draft's checkout would change files under the running turn"
        );
    }

    #[rstest::rstest]
    fn switch_branch_allowed_on_a_new_worktree_draft_while_the_root_is_busy() {
        // Given a selected new-worktree draft and a thread working in the root.
        let state = draft_beside_busy_root(DraftWorkspace::NewWorktree);

        // When validating a branch switch.
        let result = validate_switch_branch(&state);

        // Then it is allowed.
        assert_eq!(
            result,
            Ok(()),
            "a new-worktree draft only records its base branch"
        );
    }

    /// [`selected`] with the thread, prompt-less, in group 9.
    fn grouped() -> AppState {
        let mut state = selected(None);
        if let Some(thread) = state
            .sessions
            .projects
            .first_mut()
            .and_then(|project| project.threads.first_mut())
        {
            thread.group = Some(GroupId(9));
        }
        state
    }

    #[rstest::rstest]
    fn change_workspace_rejected_on_a_grouped_thread() {
        // Given a prompt-less selected thread in a group.
        let state = grouped();

        // When validating a workspace change.
        let result = validate_change_workspace(&state);

        // Then validation fails with Grouped.
        assert_eq!(
            result,
            Err(ChangeWorkspaceError::Grouped),
            "a group's directory is fixed"
        );
    }

    #[rstest::rstest]
    fn switch_branch_rejected_on_a_grouped_thread() {
        // Given a selected thread in a group.
        let state = grouped();

        // When validating a branch switch.
        let result = validate_switch_branch(&state);

        // Then validation fails with Grouped.
        assert_eq!(
            result,
            Err(SwitchBranchError::Grouped),
            "a group's directory is fixed"
        );
    }
}
