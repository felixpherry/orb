//! Checks whether the user's git actions on the selected thread or draft can
//! proceed: changing its workspace, and switching its branch (also on a
//! Feature group's card, for the group's worktree).

use std::path::Path;

use wherror::Error;

use crate::AppState;
use crate::feat::sessions::state::{DraftWorkspace, GroupKind, Sessions, SidebarItem};

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
pub const BUSY_DIRECTORY: &str = "Claude is working in this directory";

/// Why switching the selected thread's, draft's or group's branch can't
/// proceed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(debug)]
pub enum SwitchBranchError {
    /// The cursor isn't on a thread, a draft or a group's card.
    NoSelection,
    /// The thread is in a group, whose branch is switched from its card.
    Grouped,
    /// The card's group has no worktree: a Research or Learn group, or a
    /// Feature group before its draft starts.
    NoWorktree,
    /// A session is already being started.
    Starting,
    /// A thread in the directory the checkout would change is working or
    /// waiting.
    Busy,
}

/// Allow switching the selected thread's, draft's or Feature group card's
/// branch unless a start is in flight or a turn is underway in the directory
/// a checkout would change. A local draft checks out in the project's root,
/// an existing-worktree draft in its worktree, and a group in its worktree; a
/// new-worktree draft only records its base, and a draft whose project isn't
/// a git repository is offered `git init` instead.
///
/// # Errors
///
/// Returns [`SwitchBranchError::NoSelection`] without a selected thread,
/// draft or group card, [`SwitchBranchError::Grouped`] on a thread in a
/// group, [`SwitchBranchError::NoWorktree`] on a card whose group has no
/// worktree, [`SwitchBranchError::Starting`] while a start is in flight, and
/// [`SwitchBranchError::Busy`] while any thread in the same directory is in
/// progress.
pub fn validate_switch_branch(state: &AppState) -> Result<(), SwitchBranchError> {
    let sessions = &state.sessions;
    let card = match sessions.cursor {
        Some(SidebarItem::Group(_)) => sessions.selected_group(),
        _ => None,
    };
    match (sessions.selected_draft(), sessions.selected_thread(), card) {
        (None, None, None) => Err(SwitchBranchError::NoSelection),
        (None, Some(thread), _) if thread.group.is_some() => Err(SwitchBranchError::Grouped),
        (None, None, Some((_, group))) if group.kind != GroupKind::Feature => {
            Err(SwitchBranchError::NoWorktree)
        }
        _ if sessions.starting => Err(SwitchBranchError::Starting),
        (Some((_, draft)), ..) if !draft.repo => Ok(()),
        (Some((project, draft)), ..) => match &draft.workspace {
            DraftWorkspace::Existing(path) => not_busy(sessions, path),
            DraftWorkspace::NewWorktree => Ok(()),
            DraftWorkspace::Local => not_busy(sessions, &project.root),
        },
        (None, Some(thread), _) => not_busy(sessions, &thread.cwd),
        (None, None, Some((_, group))) => group
            .dir
            .as_deref()
            .map_or(Err(SwitchBranchError::NoWorktree), |dir| {
                not_busy(sessions, dir)
            }),
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
    use std::time::SystemTime;

    use super::{
        ChangeWorkspaceError, SwitchBranchError, validate_change_workspace, validate_switch_branch,
    };
    use crate::AppState;
    use crate::feat::sessions::state::{
        Draft, DraftWorkspace, Group, GroupId, GroupKind, Project, ProjectId, ProjectKind,
        Sessions, SidebarItem, Thread, ThreadId, ThreadStatus,
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
                    removed: false,
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

    /// [`with_working_thread`] in the root, plus the project's draft in
    /// `workspace`, selected.
    fn draft_beside_busy_root(workspace: DraftWorkspace) -> AppState {
        let mut state = with_working_thread("/work");
        if let Some(project) = state.sessions.projects.first_mut() {
            project.draft = Some(Draft {
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

    /// [`grouped`] with the cursor on group 9's card: a `kind` group, in
    /// `dir` when given.
    fn on_card(kind: GroupKind, dir: Option<&str>) -> AppState {
        let mut state = grouped();
        if let Some(project) = state.sessions.projects.first_mut() {
            project.groups = vec![Group {
                id: GroupId(9),
                kind,
                name: "GT-514-login".into(),
                dir: dir.map(Into::into),
                branch: None,
                created_at: SystemTime::UNIX_EPOCH,
                pinned_at: None,
                settled_at: None,
                active_since: SystemTime::UNIX_EPOCH,
                draft: None,
            }];
        }
        state.sessions.cursor = Some(SidebarItem::Group(GroupId(9)));
        state
    }

    #[rstest::rstest]
    fn switch_branch_allowed_on_a_started_feature_card() {
        // Given a Feature group's card, its worktree at /wt/orb-1a2b3c4d.
        let state = on_card(GroupKind::Feature, Some("/wt/orb-1a2b3c4d"));

        // When validating a branch switch.
        let result = validate_switch_branch(&state);

        // Then it's allowed.
        assert_eq!(result, Ok(()), "a Feature card switches its worktree");
    }

    #[rstest::rstest]
    fn switch_branch_rejected_on_a_feature_card_while_its_thread_works() {
        // Given a Feature group's card, its worktree at /work, where its
        // thread is working.
        let mut state = on_card(GroupKind::Feature, Some("/work"));
        if let Some(thread) = state
            .sessions
            .projects
            .first_mut()
            .and_then(|project| project.threads.first_mut())
        {
            thread.status = ThreadStatus::Working;
        }

        // When validating a branch switch.
        let result = validate_switch_branch(&state);

        // Then validation fails with Busy.
        assert_eq!(
            result,
            Err(SwitchBranchError::Busy),
            "a checkout would change files under the group's running turn"
        );
    }

    #[rstest::rstest]
    #[case::research(GroupKind::Research, Some("/orb/research/x"))]
    #[case::learn(GroupKind::Learn, Some("/orb/learn/x"))]
    #[case::unstarted_feature(GroupKind::Feature, None)]
    fn switch_branch_rejected_on_a_card_without_a_worktree(
        #[case] kind: GroupKind,
        #[case] dir: Option<&str>,
    ) {
        // Given a card whose group has no worktree.
        let state = on_card(kind, dir);

        // When validating a branch switch.
        let result = validate_switch_branch(&state);

        // Then validation fails with NoWorktree.
        assert_eq!(
            result,
            Err(SwitchBranchError::NoWorktree),
            "a {kind:?} card at {dir:?} has no worktree"
        );
    }
}
