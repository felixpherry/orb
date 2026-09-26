//! The [`IntentHandler`]: the single decision point for all user input.

use std::path::{Path, PathBuf};

use crate::command::Workspace;
use crate::feat::git::validator::{
    BUSY_DIRECTORY, ChangeWorkspaceError, SwitchBranchError, validate_change_workspace,
    validate_switch_branch,
};
use crate::feat::git::worktree::previous_worktree;
use crate::feat::pane::validator::validate_attach;
use crate::feat::picker::list::{BranchRow, PickerItem, WorkspaceChoice};
use crate::feat::picker::state::{PickTarget, PickerKind, PickerState};
use crate::feat::picker::validator::{
    validate_add_directory, validate_open_directory, validate_pick_project,
};
use crate::feat::preview::validator::{validate_toggle_fold, validate_yank};
use crate::feat::sessions::state::{
    AttachTarget, Draft, DraftWorkspace, Project, ProjectId, SidebarItem, ThreadId,
};
use crate::feat::sessions::validator::{
    validate_close_shelf, validate_delete, validate_open_shelf, validate_pick_setting,
    validate_start_draft, validate_toggle_pin, validate_toggle_settle,
};
use crate::feat::sidebar::validator::{validate_focus_sidebar, validate_resize};
use crate::feat::zellij::validator::validate_open_tool;
use crate::{AppState, Command, Focus, Intent};

/// Applies each [`Intent`] to [`AppState`] in one match block.
pub struct IntentHandler;

impl IntentHandler {
    /// Apply `intent` to `state` and return the commands that must follow.
    /// Every intent first clears the mode line's error, which the user has now
    /// seen; otherwise an intent that fails validation changes nothing.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per intent keeps every input decision in one match"
    )]
    pub fn handle(intent: &Intent, state: &mut AppState) -> Vec<Command> {
        state.sessions.error = None;
        match intent {
            Intent::Quit => {
                state.should_quit = true;
                vec![]
            }
            Intent::SelectNext => {
                state.sessions.select_next();
                with_visit(state, vec![Command::ShowPreview])
            }
            Intent::SelectPrev => {
                state.sessions.select_prev();
                with_visit(state, vec![Command::ShowPreview])
            }
            Intent::SelectFirst => {
                state.sessions.select_first();
                with_visit(state, vec![Command::ShowPreview])
            }
            Intent::SelectLast => {
                state.sessions.select_last();
                with_visit(state, vec![Command::ShowPreview])
            }
            Intent::SelectHalfPageDown => {
                state.sessions.half_page_down(&state.sidebar.layout);
                with_visit(state, vec![Command::ShowPreview])
            }
            Intent::SelectHalfPageUp => {
                state.sessions.half_page_up(&state.sidebar.layout);
                with_visit(state, vec![Command::ShowPreview])
            }
            Intent::FocusPreview => {
                state.focus = Focus::Preview;
                vec![]
            }
            Intent::FocusSidebar => match validate_focus_sidebar(state) {
                Ok(()) => {
                    state.focus = Focus::Sidebar;
                    vec![]
                }
                Err(_) => vec![],
            },
            Intent::ToggleSidebar => {
                if state.sidebar.hidden {
                    state.sidebar.hidden = false;
                    state.focus = Focus::Sidebar;
                } else {
                    state.sidebar.hidden = true;
                    if state.focus == Focus::Sidebar {
                        state.focus = Focus::Preview;
                    }
                }
                vec![]
            }
            Intent::WidenFocused | Intent::NarrowFocused => {
                // Widening the right side narrows the sidebar.
                let changed = match (validate_resize(state), state.focus, intent) {
                    (Ok(()), Focus::Sidebar, Intent::WidenFocused)
                    | (Ok(()), Focus::Preview, Intent::NarrowFocused) => state.sidebar.widen(),
                    (Ok(()), Focus::Sidebar | Focus::Preview, _) => state.sidebar.narrow(),
                    _ => false,
                };
                changed.then_some(Command::SaveUi).into_iter().collect()
            }
            Intent::Attach if state.sessions.cursor == Some(SidebarItem::SettledShelf) => {
                if state.sessions.shelf_open {
                    state.sessions.close_shelf();
                } else {
                    state.sessions.open_shelf();
                }
                vec![]
            }
            Intent::Attach if matches!(state.sessions.cursor, Some(SidebarItem::Draft(_))) => {
                match (validate_start_draft(state), state.sessions.cursor) {
                    (Ok(()), Some(SidebarItem::Draft(project))) => {
                        state.sessions.starting = true;
                        vec![Command::StartDraft(project)]
                    }
                    _ => vec![],
                }
            }
            Intent::Attach => match (validate_attach(state), state.sessions.selected_thread()) {
                (Ok(()), Some(thread)) => {
                    let target = AttachTarget {
                        thread: thread.id,
                        argv: thread.attach_argv.clone(),
                        cwd: thread.cwd.clone(),
                    };
                    state.focus = Focus::Attached;
                    vec![Command::Attach(target), Command::RefreshSessions]
                }
                _ => vec![],
            },
            Intent::Detach => {
                state.focus = Focus::Preview;
                vec![Command::Detach, Command::RefreshSessions]
            }
            Intent::NewSession => {
                let items = state
                    .sessions
                    .projects_by_recency()
                    .into_iter()
                    .map(|project| PickerItem::Project {
                        id: project.id,
                        title: project.title.clone(),
                        root: project.root.clone(),
                    })
                    .collect();
                open_picker(state, PickerState::projects(items, state.focus));
                vec![]
            }
            Intent::AddProject => {
                let (picker, dir) = PickerState::directories(state.home.clone(), state.focus);
                state.picker = Some(picker);
                state.focus = Focus::Picker;
                vec![Command::ListDirectories(dir)]
            }
            Intent::ChangeWorkspace => match (
                validate_change_workspace(state),
                state.sessions.selected_draft(),
                state.sessions.selected_project(),
                state.sessions.selected_thread(),
            ) {
                (Ok(()), Some((project, draft)), _, _) if !draft.repo => {
                    open_picker(state, PickerState::init_git(project.id, state.focus));
                    vec![]
                }
                (Ok(()), Some((project, draft)), _, _) => {
                    let (cwd, worktree) = match &draft.workspace {
                        DraftWorkspace::Existing(path) => (path, true),
                        DraftWorkspace::Local | DraftWorkspace::NewWorktree => {
                            (&project.root, false)
                        }
                    };
                    let items = workspace_items(project, cwd, worktree, None);
                    let picker = {
                        let target = PickTarget::Draft(project.id);
                        let mut picker = PickerState::workspace(target, items, state.focus);
                        if draft.workspace == DraftWorkspace::NewWorktree {
                            picker.select(&PickerItem::Workspace(WorkspaceChoice::NewWorktree));
                        }
                        picker
                    };
                    open_picker(state, picker);
                    vec![]
                }
                (Ok(()), None, Some(project), Some(thread)) => {
                    let worktree = thread.cwd != project.root;
                    let items = workspace_items(project, &thread.cwd, worktree, Some(thread.id));
                    let target = PickTarget::Thread(thread.id);
                    open_picker(state, PickerState::workspace(target, items, state.focus));
                    vec![]
                }
                (Err(ChangeWorkspaceError::Locked { worktree }), ..) => {
                    let workspace = if worktree {
                        "Worktree"
                    } else {
                        "Local checkout"
                    };
                    state.sessions.error = Some(format!("Workspace locked · {workspace}"));
                    vec![]
                }
                _ => vec![],
            },
            Intent::PickerInput(ch) => list(state.picker.as_mut().and_then(|p| p.insert(*ch))),
            Intent::PickerBackspace => list(state.picker.as_mut().and_then(PickerState::backspace)),
            Intent::PickerDeleteWord => {
                list(state.picker.as_mut().and_then(PickerState::delete_word))
            }
            Intent::PickerCursorLeft => {
                if let Some(picker) = &mut state.picker {
                    picker.cursor_left();
                }
                vec![]
            }
            Intent::PickerCursorRight => {
                if let Some(picker) = &mut state.picker {
                    picker.cursor_right();
                }
                vec![]
            }
            Intent::PickerNext => {
                if let Some(picker) = &mut state.picker {
                    picker.next();
                }
                vec![]
            }
            Intent::PickerPrev => {
                if let Some(picker) = &mut state.picker {
                    picker.prev();
                }
                vec![]
            }
            Intent::PickerHalfPageDown => {
                if let Some(picker) = &mut state.picker {
                    picker.half_page_down();
                }
                vec![]
            }
            Intent::PickerHalfPageUp => {
                if let Some(picker) = &mut state.picker {
                    picker.half_page_up();
                }
                vec![]
            }
            Intent::SwitchBranch => match (
                validate_switch_branch(state),
                state.sessions.selected_draft(),
                state.sessions.selected_thread(),
            ) {
                (Ok(()), Some((project, draft)), _) if !draft.repo => {
                    open_picker(state, PickerState::init_git(project.id, state.focus));
                    vec![]
                }
                (Ok(()), Some((project, draft)), _) => {
                    let cwd = draft_dir(project, draft);
                    let base = match draft.workspace {
                        DraftWorkspace::NewWorktree => draft.branch.clone(),
                        DraftWorkspace::Local | DraftWorkspace::Existing(_) => None,
                    };
                    let target = PickTarget::Draft(project.id);
                    let picker =
                        PickerState::branches(target, cwd.clone(), true, base, state.focus);
                    open_picker(state, picker);
                    vec![Command::ListBranches(cwd)]
                }
                (Ok(()), None, Some(thread)) => {
                    let cwd = thread.cwd.clone();
                    let unstarted = thread.transcript.is_none() && !thread.status.in_progress();
                    let target = PickTarget::Thread(thread.id);
                    let picker =
                        PickerState::branches(target, cwd.clone(), unstarted, None, state.focus);
                    open_picker(state, picker);
                    vec![Command::ListBranches(cwd)]
                }
                (Err(SwitchBranchError::Busy), ..) => {
                    state.sessions.error = Some(BUSY_DIRECTORY.to_owned());
                    vec![]
                }
                _ => vec![],
            },
            Intent::OpenTool(tool) => match (
                validate_open_tool(state),
                state.sessions.selected_draft(),
                state.sessions.selected_thread(),
            ) {
                (Ok(()), Some((project, draft)), _) => vec![Command::OpenTool {
                    tool: *tool,
                    cwd: draft_dir(project, draft),
                }],
                (Ok(()), None, Some(thread)) => vec![Command::OpenTool {
                    tool: *tool,
                    cwd: thread.cwd.clone(),
                }],
                _ => vec![],
            },
            Intent::PickModel => {
                match (
                    validate_pick_setting(state),
                    state.sessions.selected_draft(),
                ) {
                    (Ok(()), Some((project, draft))) => {
                        let current = draft.model.as_deref();
                        let picker = PickerState::models(project.id, current, state.focus);
                        open_picker(state, picker);
                        vec![]
                    }
                    _ => vec![],
                }
            }
            Intent::PickPermission => {
                match (
                    validate_pick_setting(state),
                    state.sessions.selected_draft(),
                ) {
                    (Ok(()), Some((project, draft))) => {
                        let current = draft.permission.as_deref();
                        let picker = PickerState::permissions(project.id, current, state.focus);
                        open_picker(state, picker);
                        vec![]
                    }
                    _ => vec![],
                }
            }
            Intent::PickerOpen => match validate_open_directory(state) {
                Ok(()) => list(state.picker.as_mut().and_then(PickerState::open_directory)),
                Err(_) => vec![],
            },
            Intent::PickerConfirm => match state.picker.as_ref().map(PickerState::kind) {
                Some(&PickerKind::Workspace {
                    target: PickTarget::Thread(thread),
                }) => {
                    let to = match close_picker(state).as_ref().and_then(PickerState::selected) {
                        Some(PickerItem::Workspace(WorkspaceChoice::NewWorktree)) => {
                            Some(Workspace::NewWorktree)
                        }
                        Some(PickerItem::Workspace(WorkspaceChoice::Previous { path, .. })) => {
                            Some(Workspace::Existing(path.clone()))
                        }
                        _ => None,
                    };
                    match to {
                        Some(to) => {
                            state.sessions.starting = true;
                            vec![Command::MoveThread { thread, to }]
                        }
                        None => vec![],
                    }
                }
                Some(&PickerKind::Workspace {
                    target: PickTarget::Draft(project),
                }) => match close_picker(state).as_ref().and_then(PickerState::selected) {
                    Some(PickerItem::Workspace(choice)) => {
                        pick_draft_workspace(state, project, choice)
                    }
                    _ => vec![],
                },
                Some(&PickerKind::InitGit { project }) => {
                    match close_picker(state).as_ref().and_then(PickerState::selected) {
                        Some(PickerItem::InitGit) => vec![Command::InitGit(project)],
                        _ => vec![],
                    }
                }
                Some(&PickerKind::Model { project }) => {
                    match close_picker(state).as_ref().and_then(PickerState::selected) {
                        Some(&PickerItem::Setting(value)) => edit_draft(state, project, |draft| {
                            draft.model = value.map(str::to_owned);
                        }),
                        _ => vec![],
                    }
                }
                Some(&PickerKind::Permission { project }) => {
                    match close_picker(state).as_ref().and_then(PickerState::selected) {
                        Some(&PickerItem::Setting(value)) => edit_draft(state, project, |draft| {
                            draft.permission = value.map(str::to_owned);
                        }),
                        _ => vec![],
                    }
                }
                Some(PickerKind::Branches {
                    target: PickTarget::Thread(_),
                    ..
                }) => close_picker(state)
                    .map(|picker| pick_branch(state, &picker))
                    .unwrap_or_default(),
                Some(PickerKind::Branches {
                    target: PickTarget::Draft(_),
                    ..
                }) => close_picker(state)
                    .map(|picker| pick_draft_branch(state, &picker))
                    .unwrap_or_default(),
                _ => match (validate_pick_project(state), validate_add_directory(state)) {
                    (Ok(()), _) => {
                        match close_picker(state).as_ref().and_then(PickerState::selected) {
                            Some(&PickerItem::Project { id, .. }) => open_draft(state, id),
                            _ => vec![],
                        }
                    }
                    (_, Ok(())) => close_picker(state)
                        .and_then(|picker| picker.directory_to_add())
                        .map(Command::AddProject)
                        .into_iter()
                        .collect(),
                    _ => vec![],
                },
            },
            Intent::PickerCancel => {
                close_picker(state);
                vec![]
            }
            Intent::NextBlock => {
                state.preview.next_block();
                vec![]
            }
            Intent::PrevBlock => {
                state.preview.prev_block();
                vec![]
            }
            Intent::HalfPageDown => {
                state.preview.half_page_down();
                vec![]
            }
            Intent::HalfPageUp => {
                state.preview.half_page_up();
                vec![]
            }
            Intent::Top => {
                state.preview.top();
                vec![]
            }
            Intent::Bottom => {
                state.preview.bottom();
                vec![]
            }
            Intent::ToggleFold => match validate_toggle_fold(state) {
                Ok(()) => {
                    state.preview.toggle_fold();
                    vec![]
                }
                Err(_) => vec![],
            },
            Intent::Yank => match (validate_yank(state), state.preview.cursor_block()) {
                (Ok(()), Some(block)) => vec![Command::Yank(block.raw_text())],
                _ => vec![],
            },
            Intent::OpenShelf => match validate_open_shelf(state) {
                Ok(()) => {
                    state.sessions.open_shelf();
                    vec![]
                }
                Err(_) => vec![],
            },
            Intent::CloseShelf => match validate_close_shelf(state) {
                Ok(()) => {
                    state.sessions.close_shelf();
                    vec![Command::ShowPreview]
                }
                Err(_) => vec![],
            },
            Intent::TogglePin => {
                match (validate_toggle_pin(state), state.sessions.selected_thread()) {
                    (Ok(()), Some(thread)) => match thread.pinned_at {
                        Some(_) => vec![Command::Unpin(thread.id)],
                        None => vec![Command::Pin(thread.id)],
                    },
                    _ => vec![],
                }
            }
            Intent::ToggleSettle => {
                match (
                    validate_toggle_settle(state),
                    state.sessions.selected_thread(),
                ) {
                    (Ok(()), Some(thread)) if thread.settled_at.is_some() => {
                        vec![Command::Unsettle(thread.id)]
                    }
                    (Ok(()), Some(thread)) => {
                        let id = thread.id;
                        state.sessions.cursor = state.sessions.card_neighbour(id);
                        with_visit(state, vec![Command::Settle(id), Command::ShowPreview])
                    }
                    _ => vec![],
                }
            }
            Intent::DeleteThread => match (validate_delete(state), state.sessions.cursor) {
                (Ok(()), Some(item @ SidebarItem::Draft(project))) => {
                    state.sessions.cursor = state.sessions.row_neighbour(item);
                    with_visit(
                        state,
                        vec![Command::DiscardDraft(project), Command::ShowPreview],
                    )
                }
                (Ok(()), Some(item @ SidebarItem::Thread(id))) => {
                    let neighbour = state.sessions.row_neighbour(item);
                    state.sessions.deleting.insert(id);
                    state.sessions.cursor = neighbour;
                    with_visit(state, vec![Command::Delete(id), Command::ShowPreview])
                }
                _ => vec![],
            },
        }
    }
}

/// The directory a draft works in: its worktree, else the project's root.
fn draft_dir(project: &Project, draft: &Draft) -> PathBuf {
    match &draft.workspace {
        DraftWorkspace::Existing(path) => path.clone(),
        DraftWorkspace::Local | DraftWorkspace::NewWorktree => project.root.clone(),
    }
}

/// Asks the frontend to list `dir` into the directory picker, if there is one.
fn list(dir: Option<PathBuf>) -> Vec<Command> {
    dir.map(Command::ListDirectories).into_iter().collect()
}

/// What picking the selected branch of `picker`, a closed branch picker, asks
/// for:
/// - the current branch: nothing;
/// - before the first prompt, a branch checked out in another worktree: move
///   the thread there;
/// - before the first prompt, the local default branch checked out nowhere,
///   from outside the root: check it out in the root and move the thread
///   there;
/// - otherwise: check it out in the thread's directory.
fn pick_branch(state: &mut AppState, picker: &PickerState) -> Vec<Command> {
    let (
        &PickerKind::Branches {
            target: PickTarget::Thread(thread),
            ref cwd,
            unstarted,
        },
        Some(PickerItem::Branch(BranchRow { git_ref, .. })),
    ) = (picker.kind(), picker.selected())
    else {
        return vec![];
    };
    let elsewhere = git_ref.worktree.as_ref().filter(|path| *path != cwd);
    match (git_ref.current, unstarted, elsewhere) {
        (true, _, _) => vec![],
        (false, true, Some(path)) => {
            state.sessions.starting = true;
            vec![Command::MoveThread {
                thread,
                to: Workspace::Existing(path.clone()),
            }]
        }
        _ => {
            let outside_root = state.sessions.projects.iter().any(|project| {
                project.root != *cwd && project.threads.iter().any(|t| t.id == thread)
            });
            let to_root = unstarted
                && git_ref.default
                && !git_ref.remote
                && git_ref.worktree.is_none()
                && outside_root;
            state.sessions.starting |= to_root;
            vec![Command::SwitchBranch {
                thread,
                git_ref: git_ref.clone(),
                to_root,
            }]
        }
    }
}

/// What picking the selected branch of `picker`, a draft's closed branch
/// picker listing the draft's directory (the project's root, or its
/// worktree), asks for, as T3 Code's `resolveBranchSelectionTarget` decides:
/// - a new-worktree draft: record the branch as the base, nothing else;
/// - the branch checked out there already: nothing;
/// - a branch checked out in another worktree or the root: move the draft
///   there;
/// - from a worktree, the default branch: check it out in the root and move
///   the draft there;
/// - otherwise: check the branch out in the draft's directory.
fn pick_draft_branch(state: &mut AppState, picker: &PickerState) -> Vec<Command> {
    let (
        &PickerKind::Branches {
            target: PickTarget::Draft(project),
            ref cwd,
            ..
        },
        Some(PickerItem::Branch(BranchRow { git_ref, .. })),
    ) = (picker.kind(), picker.selected())
    else {
        return vec![];
    };
    let Some(root) = state
        .sessions
        .projects
        .iter()
        .find(|p| p.id == project)
        .map(|p| p.root.clone())
    else {
        return vec![];
    };
    let Some(draft) = state.sessions.draft_mut(project) else {
        return vec![];
    };
    let checkout = |cwd: &Path| {
        vec![Command::CheckoutDraft {
            project,
            git_ref: git_ref.clone(),
            cwd: cwd.to_owned(),
        }]
    };
    match (&draft.workspace, &git_ref.worktree) {
        (DraftWorkspace::NewWorktree, _) => {
            draft.branch = Some(git_ref.name.clone());
            draft.from = None;
            vec![Command::SaveDraft(project)]
        }
        _ if git_ref.current => vec![],
        (_, Some(path)) if path == cwd => vec![],
        (_, Some(path)) => {
            draft.workspace = if *path == root {
                DraftWorkspace::Local
            } else {
                DraftWorkspace::Existing(path.clone())
            };
            draft.branch = Some(git_ref.name.clone());
            vec![Command::SaveDraft(project)]
        }
        (DraftWorkspace::Existing(_), None) if git_ref.default => checkout(&root),
        (DraftWorkspace::Local | DraftWorkspace::Existing(_), None) => checkout(cwd),
    }
}

/// What picking `choice` in `project`'s draft's workspace picker asks for.
/// The draft's own workspace (`Current worktree` for a draft in a worktree)
/// changes nothing, as re-picking a value does in T3 Code's selector; another
/// one is set on the draft, with its branch to be filled in unless it's a
/// previous worktree's, and saved.
fn pick_draft_workspace(
    state: &mut AppState,
    project: ProjectId,
    choice: &WorkspaceChoice,
) -> Vec<Command> {
    let Some(draft) = state.sessions.draft_mut(project) else {
        return vec![];
    };
    let (workspace, branch) = match choice {
        WorkspaceChoice::Current { worktree: true } => return vec![],
        WorkspaceChoice::Current { worktree: false } => (DraftWorkspace::Local, None),
        WorkspaceChoice::NewWorktree => (DraftWorkspace::NewWorktree, None),
        WorkspaceChoice::Previous { path, branch } => {
            (DraftWorkspace::Existing(path.clone()), branch.clone())
        }
    };
    if draft.workspace == workspace {
        return vec![];
    }
    draft.workspace = workspace;
    draft.branch = branch;
    draft.from = None;
    vec![Command::SaveDraft(project)]
}

/// The workspace picker's rows for a thread or draft running in `cwd`: stay
/// there (`worktree` is whether that's a worktree), a new worktree, then the
/// project's previous worktree other than `cwd`, ignoring the thread `except`.
fn workspace_items(
    project: &Project,
    cwd: &Path,
    worktree: bool,
    except: Option<ThreadId>,
) -> Vec<PickerItem> {
    [
        WorkspaceChoice::Current { worktree },
        WorkspaceChoice::NewWorktree,
    ]
    .into_iter()
    .chain(
        previous_worktree(project, cwd, except)
            .map(|(path, branch)| WorkspaceChoice::Previous { path, branch }),
    )
    .map(PickerItem::Workspace)
    .collect()
}

/// Selects `project`'s draft and gives the keys to its form, asking the
/// sessions actor to create the draft when the project has none.
fn open_draft(state: &mut AppState, project: ProjectId) -> Vec<Command> {
    let exists = state
        .sessions
        .projects
        .iter()
        .any(|p| p.id == project && p.draft.is_some());
    state.sessions.cursor = Some(SidebarItem::Draft(project));
    state.focus = Focus::Preview;
    (!exists)
        .then_some(Command::CreateDraft(project))
        .into_iter()
        .chain([Command::ShowPreview])
        .collect()
}

/// Applies `edit` to `project`'s draft and asks the sessions actor to save it;
/// nothing when the draft is gone.
fn edit_draft<F>(state: &mut AppState, project: ProjectId, edit: F) -> Vec<Command>
where
    F: FnOnce(&mut Draft),
{
    match state.sessions.draft_mut(project) {
        Some(draft) => {
            edit(draft);
            vec![Command::SaveDraft(project)]
        }
        None => vec![],
    }
}

/// Opens `picker` and gives it the keys.
fn open_picker(state: &mut AppState, picker: PickerState) {
    state.picker = Some(picker);
    state.focus = Focus::Picker;
}

/// Closes the picker and gives the keys back to where it was opened from.
fn close_picker(state: &mut AppState) -> Option<PickerState> {
    let picker = state.picker.take()?;
    state.focus = picker.return_to();
    Some(picker)
}

/// `commands`, then a visit to the thread under the cursor, if any.
fn with_visit(state: &AppState, mut commands: Vec<Command>) -> Vec<Command> {
    commands.extend(state.sessions.selected_id().map(Command::Visit));
    commands
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    use crate::command::Workspace;
    use crate::feat::git::git_service::GitRef;
    use crate::feat::picker::list::{BranchRow, PERMISSION_MODES, PickerItem, WorkspaceChoice};
    use crate::feat::picker::state::{PickTarget, PickerKind, PickerState};
    use crate::feat::preview::block::{Block, BlockId, BlockKind, ToolCall, ToolStatus};
    use crate::feat::preview::state::{Preview, PreviewLayout};
    use crate::feat::sessions::state::{
        AttachTarget, Draft, DraftWorkspace, Project, ProjectId, Sessions, SidebarItem, SidebarRow,
        Thread, ThreadId, ThreadStatus,
    };
    use crate::feat::sidebar::state::SidebarView;
    use crate::feat::zellij::zellij_service::Tool;
    use crate::{AppState, Command, Focus, Intent, IntentHandler};

    fn thread(id: i64, status: ThreadStatus) -> Thread {
        Thread {
            id: ThreadId(id),
            title: None,
            cwd: format!("/work/{id}").into(),
            transcript: None,
            status,
            turn_started_at: None,
            attach_argv: vec!["claude".into(), "attach".into(), format!("t{id}").into()],
            branch: None,
            pinned_at: None,
            settled_at: None,
            active_since: SystemTime::UNIX_EPOCH,
            last_activity_at: SystemTime::UNIX_EPOCH,
            unseen: false,
        }
    }

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    /// Thread `id`, settled at second `id`.
    fn settled(id: i64) -> Thread {
        Thread {
            settled_at: Some(at(id.unsigned_abs())),
            ..thread(id, ThreadStatus::Stopped)
        }
    }

    /// One project holding `threads`, with the sidebar's cursor on `cursor`.
    /// Unsettled threads with equal times list the higher id first.
    fn state_at(threads: Vec<Thread>, cursor: SidebarItem) -> AppState {
        AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "work".into(),
                    root: "/work".into(),
                    created_at: SystemTime::UNIX_EPOCH,
                    draft: None,
                    threads,
                }],
                cursor: Some(cursor),
                ..Sessions::default()
            },
            ..AppState::default()
        }
    }

    /// One project per title, ids from 1, rooted at `/<title>`, with no
    /// threads, all added at the same time.
    fn with_projects(titles: &[&str]) -> AppState {
        AppState {
            sessions: Sessions {
                projects: (1..)
                    .zip(titles)
                    .map(|(id, title)| Project {
                        id: ProjectId(id),
                        title: (*title).to_owned(),
                        root: format!("/{title}").into(),
                        created_at: SystemTime::UNIX_EPOCH,
                        draft: None,
                        threads: vec![],
                    })
                    .collect(),
                ..Sessions::default()
            },
            ..AppState::default()
        }
    }

    const HOME: &str = "/home/me";

    /// Projects alpha (1, `/alpha`) and beta (2, `/beta`), with the project
    /// picker opened from `from`.
    fn picking(from: Focus) -> AppState {
        let mut state = AppState {
            focus: from,
            ..with_projects(&["alpha", "beta"])
        };
        IntentHandler::handle(&Intent::NewSession, &mut state);
        state
    }

    /// The directory picker opened from the sidebar at `~/`, which lists
    /// `names`.
    fn browsing(names: &[&str]) -> AppState {
        let mut state = AppState {
            home: HOME.into(),
            ..AppState::default()
        };
        IntentHandler::handle(&Intent::AddProject, &mut state);
        if let Some(picker) = &mut state.picker {
            let names = names.iter().map(|name| (*name).to_owned()).collect();
            picker.show_directories(Path::new(HOME), names);
        }
        state
    }

    /// One project holding `threads`, with thread `selected` selected.
    fn state_with(threads: Vec<Thread>, selected: i64) -> AppState {
        state_at(threads, SidebarItem::Thread(ThreadId(selected)))
    }

    /// Thread `id`, idle and prompt-less, in the project's root `/work`.
    fn in_root(id: i64) -> Thread {
        Thread {
            cwd: "/work".into(),
            ..thread(id, ThreadStatus::Idle)
        }
    }

    /// The workspace picker opened on thread 1 of `threads`.
    fn choosing_workspace(threads: Vec<Thread>) -> AppState {
        let mut state = AppState {
            focus: Focus::Preview,
            ..state_with(threads, 1)
        };
        IntentHandler::handle(&Intent::ChangeWorkspace, &mut state);
        state
    }

    fn workspace_labels(state: &AppState) -> Vec<String> {
        state
            .picker
            .iter()
            .flat_map(PickerState::shown)
            .filter_map(|(item, _)| match item {
                PickerItem::Workspace(choice) => Some(choice.label()),
                _ => None,
            })
            .collect()
    }

    /// A preview showing one block of `kind`, followed.
    fn previewing(kind: BlockKind) -> AppState {
        AppState {
            preview: Preview {
                blocks: vec![Block {
                    id: BlockId(0),
                    parts: 1,
                    kind,
                }]
                .into(),
                ..Preview::default()
            },
            ..AppState::default()
        }
    }

    fn cargo_test(output: Option<&str>) -> BlockKind {
        BlockKind::Tool(ToolCall {
            summary: "$ cargo test".into(),
            status: ToolStatus::Ok,
            output: output.map(str::to_owned),
        })
    }

    #[rstest::rstest]
    fn quit_sets_should_quit_in_state() {
        // Given a fresh AppState.
        let mut state = AppState::default();

        // When handling Quit.
        IntentHandler::handle(&Intent::Quit, &mut state);

        // Then the frontend is told to exit.
        assert!(state.should_quit, "Quit should set should_quit");
    }

    #[rstest::rstest]
    fn select_next_on_last_thread_keeps_selection() {
        // Given threads 2 and 1 in sidebar order, with the last one selected.
        let mut state = state_with(
            vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
            1,
        );

        // When handling SelectNext.
        IntentHandler::handle(&Intent::SelectNext, &mut state);

        // Then the selection stays on the last thread.
        assert_eq!(
            state.sessions.selected_id(),
            Some(ThreadId(1)),
            "SelectNext should not wrap past the last thread"
        );
    }

    #[rstest::rstest]
    fn select_prev_on_first_thread_keeps_selection() {
        // Given threads 2 and 1 in sidebar order, with the first one selected.
        let mut state = state_with(
            vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
            2,
        );

        // When handling SelectPrev.
        IntentHandler::handle(&Intent::SelectPrev, &mut state);

        // Then the selection stays on the first thread.
        assert_eq!(
            state.sessions.selected_id(),
            Some(ThreadId(2)),
            "SelectPrev should not wrap past the first thread"
        );
    }

    #[rstest::rstest]
    #[case(Intent::FocusPreview, Focus::Sidebar, Focus::Preview)]
    #[case(Intent::FocusSidebar, Focus::Preview, Focus::Sidebar)]
    fn focus_intents_move_focus(#[case] intent: Intent, #[case] from: Focus, #[case] to: Focus) {
        // Given orb focused on `from`.
        let mut state = AppState {
            focus: from,
            ..AppState::default()
        };

        // When handling the focus intent.
        IntentHandler::handle(&intent, &mut state);

        // Then the focus moved to `to`.
        assert_eq!(state.focus, to, "{intent:?} should focus {to:?}");
    }

    /// orb focused on `focus`, with the sidebar `width` columns wide and
    /// `hidden` or not.
    fn laid_out(focus: Focus, width: u16, hidden: bool) -> AppState {
        AppState {
            focus,
            sidebar: SidebarView {
                width,
                hidden,
                ..SidebarView::default()
            },
            ..AppState::default()
        }
    }

    #[rstest::rstest]
    #[case(Focus::Sidebar)]
    #[case(Focus::Preview)]
    fn toggle_sidebar_hides_a_shown_sidebar(#[case] focus: Focus) {
        // Given a shown sidebar.
        let mut state = laid_out(focus, 32, false);

        // When handling ToggleSidebar.
        IntentHandler::handle(&Intent::ToggleSidebar, &mut state);

        // Then the sidebar is hidden.
        assert!(state.sidebar.hidden, "␣e should hide the sidebar");
    }

    #[rstest::rstest]
    #[case(Focus::Sidebar)]
    #[case(Focus::Preview)]
    fn hiding_the_sidebar_focuses_the_right_side(#[case] focus: Focus) {
        // Given a shown sidebar, with `focus` focused.
        let mut state = laid_out(focus, 32, false);

        // When handling ToggleSidebar.
        IntentHandler::handle(&Intent::ToggleSidebar, &mut state);

        // Then the right side has the keys.
        assert_eq!(
            state.focus,
            Focus::Preview,
            "hiding from {focus:?} should focus the right side"
        );
    }

    #[rstest::rstest]
    fn toggle_sidebar_shows_a_hidden_sidebar() {
        // Given a hidden sidebar.
        let mut state = laid_out(Focus::Preview, 32, true);

        // When handling ToggleSidebar.
        IntentHandler::handle(&Intent::ToggleSidebar, &mut state);

        // Then the sidebar is shown.
        assert!(!state.sidebar.hidden, "␣e should show the sidebar again");
    }

    #[rstest::rstest]
    fn showing_the_sidebar_focuses_it() {
        // Given a hidden sidebar, with the preview focused.
        let mut state = laid_out(Focus::Preview, 32, true);

        // When handling ToggleSidebar.
        IntentHandler::handle(&Intent::ToggleSidebar, &mut state);

        // Then the sidebar has the keys.
        assert_eq!(
            state.focus,
            Focus::Sidebar,
            "showing the sidebar should focus it"
        );
    }

    #[rstest::rstest]
    fn focus_sidebar_while_hidden_keeps_the_preview_focused() {
        // Given a hidden sidebar, with the preview focused.
        let mut state = laid_out(Focus::Preview, 32, true);

        // When handling FocusSidebar (`<C-h>`).
        IntentHandler::handle(&Intent::FocusSidebar, &mut state);

        // Then the preview keeps the keys.
        assert_eq!(
            state.focus,
            Focus::Preview,
            "<C-h> should do nothing while the sidebar is hidden"
        );
    }

    #[rstest::rstest]
    #[case(Intent::WidenFocused, Focus::Sidebar, 36)]
    #[case(Intent::NarrowFocused, Focus::Sidebar, 28)]
    #[case(Intent::WidenFocused, Focus::Preview, 28)]
    #[case(Intent::NarrowFocused, Focus::Preview, 36)]
    fn resize_moves_the_sidebars_edge_a_step(
        #[case] intent: Intent,
        #[case] focus: Focus,
        #[case] expected: u16,
    ) {
        // Given a 32-column sidebar, with `focus` focused.
        let mut state = laid_out(focus, 32, false);

        // When handling the resize.
        IntentHandler::handle(&intent, &mut state);

        // Then the sidebar is 4 columns wider or narrower.
        assert_eq!(
            state.sidebar.width, expected,
            "{intent:?} in {focus:?} should make the sidebar {expected} wide"
        );
    }

    #[rstest::rstest]
    fn resize_returns_save_ui() {
        // Given a 32-column sidebar, focused.
        let mut state = laid_out(Focus::Sidebar, 32, false);

        // When widening it.
        let commands = IntentHandler::handle(&Intent::WidenFocused, &mut state);

        // Then the new width is saved.
        assert_eq!(
            commands,
            vec![Command::SaveUi],
            "a resize should save the width"
        );
    }

    #[rstest::rstest]
    #[case(Intent::NarrowFocused, Focus::Sidebar, 24)]
    #[case(Intent::WidenFocused, Focus::Preview, 24)]
    #[case(Intent::WidenFocused, Focus::Sidebar, 80)]
    #[case(Intent::NarrowFocused, Focus::Preview, 80)]
    fn resize_stops_at_the_sidebars_bounds(
        #[case] intent: Intent,
        #[case] focus: Focus,
        #[case] width: u16,
    ) {
        // Given a sidebar already at a bound.
        let mut state = laid_out(focus, width, false);

        // When resizing it past the bound.
        IntentHandler::handle(&intent, &mut state);

        // Then it stays at the bound.
        assert_eq!(
            state.sidebar.width, width,
            "{intent:?} in {focus:?} should stop at {width}"
        );
    }

    #[rstest::rstest]
    fn resize_at_a_bound_returns_no_commands() {
        // Given a sidebar at its 80-column maximum, focused.
        let mut state = laid_out(Focus::Sidebar, 80, false);

        // When widening it.
        let commands = IntentHandler::handle(&Intent::WidenFocused, &mut state);

        // Then nothing is saved.
        assert!(commands.is_empty(), "an unchanged width isn't saved");
    }

    #[rstest::rstest]
    #[case(Intent::WidenFocused)]
    #[case(Intent::NarrowFocused)]
    fn resize_while_hidden_keeps_the_width(#[case] intent: Intent) {
        // Given a hidden 32-column sidebar, with the preview focused.
        let mut state = laid_out(Focus::Preview, 32, true);

        // When handling the resize.
        IntentHandler::handle(&intent, &mut state);

        // Then the width is unchanged.
        assert_eq!(
            state.sidebar.width, 32,
            "{intent:?} should do nothing while the sidebar is hidden"
        );
    }

    #[rstest::rstest]
    fn attach_to_live_thread_sets_focus_attached() {
        // Given a selected idle thread.
        let mut state = state_with(vec![thread(1, ThreadStatus::Idle)], 1);

        // When handling Attach.
        IntentHandler::handle(&Intent::Attach, &mut state);

        // Then keys go to the session.
        assert_eq!(
            state.focus,
            Focus::Attached,
            "Attach should focus the session"
        );
    }

    #[rstest::rstest]
    fn attach_to_live_thread_returns_attach_and_refresh() {
        // Given a selected idle thread.
        let mut state = state_with(vec![thread(1, ThreadStatus::Idle)], 1);

        // When handling Attach.
        let commands = IntentHandler::handle(&Intent::Attach, &mut state);

        // Then the loop attaches to that thread and the statuses are refreshed.
        assert_eq!(
            commands,
            vec![
                Command::Attach(AttachTarget {
                    thread: ThreadId(1),
                    argv: vec!["claude".into(), "attach".into(), "t1".into()],
                    cwd: "/work/1".into(),
                }),
                Command::RefreshSessions,
            ],
            "Attach should target the selected thread, then refresh"
        );
    }

    #[rstest::rstest]
    fn attach_to_a_settled_thread_returns_attach() {
        // Given a selected settled thread whose session was stopped.
        let mut state = state_with(vec![settled(1)], 1);

        // When handling Attach.
        let commands = IntentHandler::handle(&Intent::Attach, &mut state);

        // Then the loop attaches to it, which resumes the stopped session.
        assert!(
            matches!(commands.first(), Some(Command::Attach(target)) if target.thread == ThreadId(1)),
            "Attach on a settled, stopped thread should attach to it"
        );
    }

    #[rstest::rstest]
    fn attach_to_gone_thread_leaves_focus_unchanged() {
        // Given a selected thread whose session is gone.
        let mut state = state_with(vec![thread(1, ThreadStatus::Gone)], 1);

        // When handling Attach.
        IntentHandler::handle(&Intent::Attach, &mut state);

        // Then keys still drive the sidebar.
        assert_eq!(
            state.focus,
            Focus::Sidebar,
            "Attach to a gone thread should not change focus"
        );
    }

    #[rstest::rstest]
    fn attach_to_gone_thread_returns_no_commands() {
        // Given a selected thread whose session is gone.
        let mut state = state_with(vec![thread(1, ThreadStatus::Gone)], 1);

        // When handling Attach.
        let commands = IntentHandler::handle(&Intent::Attach, &mut state);

        // Then nothing happens.
        assert!(
            commands.is_empty(),
            "Attach to a gone thread should return no commands"
        );
    }

    #[rstest::rstest]
    fn detach_sets_focus_preview() {
        // Given keys going to an attached session.
        let mut state = AppState {
            focus: Focus::Attached,
            ..state_with(vec![thread(1, ThreadStatus::Idle)], 1)
        };

        // When handling Detach.
        IntentHandler::handle(&Intent::Detach, &mut state);

        // Then keys drive the thread's preview.
        assert_eq!(
            state.focus,
            Focus::Preview,
            "Detach should return to the preview"
        );
    }

    #[rstest::rstest]
    fn detach_returns_detach_and_refresh() {
        // Given keys going to an attached session.
        let mut state = AppState {
            focus: Focus::Attached,
            ..state_with(vec![thread(1, ThreadStatus::Idle)], 1)
        };

        // When handling Detach.
        let commands = IntentHandler::handle(&Intent::Detach, &mut state);

        // Then the loop detaches and the statuses are refreshed.
        assert_eq!(
            commands,
            vec![Command::Detach, Command::RefreshSessions],
            "Detach should detach, then refresh"
        );
    }

    #[rstest::rstest]
    fn new_session_opens_the_project_picker() {
        // Given projects alpha and beta, and no create in flight.
        let mut state = with_projects(&["alpha", "beta"]);

        // When handling NewSession.
        IntentHandler::handle(&Intent::NewSession, &mut state);

        // Then a project picker is open and takes the keys.
        assert_eq!(
            (state.focus, state.picker.as_ref().map(PickerState::kind)),
            (Focus::Picker, Some(&PickerKind::Projects)),
            "NewSession should open the project picker"
        );
    }

    #[rstest::rstest]
    fn new_session_returns_no_commands() {
        // Given projects alpha and beta, and no create in flight.
        let mut state = with_projects(&["alpha", "beta"]);

        // When handling NewSession.
        let commands = IntentHandler::handle(&Intent::NewSession, &mut state);

        // Then nothing is created until a project is picked.
        assert!(commands.is_empty(), "NewSession should return no commands");
    }

    #[rstest::rstest]
    fn new_session_does_not_set_starting() {
        // Given projects alpha and beta, and no create in flight.
        let mut state = with_projects(&["alpha", "beta"]);

        // When handling NewSession.
        IntentHandler::handle(&Intent::NewSession, &mut state);

        // Then no create is in flight yet.
        assert!(
            !state.sessions.starting,
            "NewSession should not set starting"
        );
    }

    #[rstest::rstest]
    fn new_session_while_starting_opens_the_picker() {
        // Given a session start already in flight.
        let mut state = AppState {
            sessions: Sessions {
                starting: true,
                ..with_projects(&["alpha"]).sessions
            },
            ..AppState::default()
        };

        // When handling NewSession.
        IntentHandler::handle(&Intent::NewSession, &mut state);

        // Then the project picker opens anyway.
        assert_eq!(
            state.picker.as_ref().map(PickerState::kind),
            Some(&PickerKind::Projects),
            "a draft can be opened while another session starts"
        );
    }

    #[rstest::rstest]
    fn add_project_opens_the_directory_picker_at_home() {
        // Given no picker open.
        let mut state = AppState {
            home: HOME.into(),
            ..AppState::default()
        };

        // When handling AddProject.
        IntentHandler::handle(&Intent::AddProject, &mut state);

        // Then a directory picker at `~/` is open and takes the keys.
        assert_eq!(
            (state.focus, state.picker.as_ref().map(PickerState::kind)),
            (
                Focus::Picker,
                Some(&PickerKind::Directories {
                    listed: Some("~/".into())
                })
            ),
            "AddProject should open the directory picker at ~/"
        );
    }

    #[rstest::rstest]
    fn add_project_returns_list_directories_for_home() {
        // Given no picker open.
        let mut state = AppState {
            home: HOME.into(),
            ..AppState::default()
        };

        // When handling AddProject.
        let commands = IntentHandler::handle(&Intent::AddProject, &mut state);

        // Then the loop is asked to list the home directory.
        assert_eq!(
            commands,
            vec![Command::ListDirectories(HOME.into())],
            "AddProject should list the home directory"
        );
    }

    #[rstest::rstest]
    fn picker_confirm_on_a_project_without_a_draft_returns_create_draft() {
        // Given the project picker with alpha, which has no draft, highlighted.
        let mut state = picking(Focus::Sidebar);

        // When handling PickerConfirm.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the sessions actor is asked to create alpha's draft, and the
        // preview to show the selection.
        assert_eq!(
            commands,
            vec![Command::CreateDraft(ProjectId(1)), Command::ShowPreview],
            "picking a project without a draft should create one"
        );
    }

    #[rstest::rstest]
    fn picker_confirm_on_a_project_with_a_draft_returns_no_create_draft() {
        // Given the project picker with alpha, which has a draft, highlighted.
        let mut state = picking(Focus::Sidebar);
        if let Some(alpha) = state.sessions.projects.first_mut() {
            alpha.draft = Some(draft(DraftWorkspace::Local));
        }

        // When handling PickerConfirm.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then only the preview is refreshed.
        assert_eq!(
            commands,
            vec![Command::ShowPreview],
            "a project keeps its one draft"
        );
    }

    #[rstest::rstest]
    fn picker_confirm_on_a_project_selects_its_draft() {
        // Given the project picker with alpha highlighted.
        let mut state = picking(Focus::Sidebar);

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the cursor is on alpha's draft.
        assert_eq!(
            state.sessions.cursor,
            Some(SidebarItem::Draft(ProjectId(1))),
            "picking a project should select its draft"
        );
    }

    #[rstest::rstest]
    fn picker_confirm_on_a_project_focuses_the_draft_form() {
        // Given the project picker opened from the sidebar.
        let mut state = picking(Focus::Sidebar);

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the picker is closed and the keys are on the right side, where
        // the draft's form is.
        assert_eq!(
            (state.focus, state.picker.is_none()),
            (Focus::Preview, true),
            "picking a project should hand the keys to its draft form"
        );
    }

    #[rstest::rstest]
    fn picker_confirm_on_a_project_does_not_set_starting() {
        // Given the project picker with alpha highlighted.
        let mut state = picking(Focus::Sidebar);

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then no session is starting.
        assert!(
            !state.sessions.starting,
            "picking a project opens its draft and never starts a session"
        );
    }

    #[rstest::rstest]
    fn picker_confirm_in_the_directory_picker_returns_add_project() {
        // Given the directory picker at `~/` with `dev` highlighted.
        let mut state = browsing(&["dev"]);

        // When handling PickerConfirm.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the sessions actor is asked to add `~/dev`.
        assert_eq!(
            commands,
            vec![Command::AddProject(Path::new(HOME).join("dev"))],
            "PickerConfirm should add the highlighted directory"
        );
    }

    #[rstest::rstest]
    fn picker_open_returns_list_directories_for_the_child() {
        // Given the directory picker at `~/` with `dev` highlighted.
        let mut state = browsing(&["dev"]);

        // When handling PickerOpen.
        let commands = IntentHandler::handle(&Intent::PickerOpen, &mut state);

        // Then the loop is asked to list `~/dev`.
        assert_eq!(
            commands,
            vec![Command::ListDirectories(Path::new(HOME).join("dev"))],
            "PickerOpen should list the highlighted directory"
        );
    }

    #[rstest::rstest]
    fn picker_cancel_closes_the_picker_and_restores_its_focus() {
        // Given the project picker opened from the preview.
        let mut state = picking(Focus::Preview);

        // When handling PickerCancel.
        IntentHandler::handle(&Intent::PickerCancel, &mut state);

        // Then the picker is closed and the keys are back on the preview.
        assert_eq!(
            (state.focus, state.picker.is_none()),
            (Focus::Preview, true),
            "PickerCancel should close the picker and return to the preview"
        );
    }

    #[rstest::rstest]
    fn change_workspace_opens_the_workspace_picker() {
        // Given a selected prompt-less thread.
        let mut state = state_with(vec![in_root(1)], 1);

        // When handling ChangeWorkspace.
        IntentHandler::handle(&Intent::ChangeWorkspace, &mut state);

        // Then the thread's workspace picker is open and takes the keys.
        assert_eq!(
            (state.focus, state.picker.as_ref().map(PickerState::kind)),
            (
                Focus::Picker,
                Some(&PickerKind::Workspace {
                    target: PickTarget::Thread(ThreadId(1))
                })
            ),
            "ChangeWorkspace should open the workspace picker"
        );
    }

    #[rstest::rstest]
    fn locked_workspace_shows_the_lock_message() {
        // Given a selected thread in the root checkout that has a transcript.
        let mut state = state_with(
            vec![Thread {
                transcript: Some("/claude/t1.jsonl".into()),
                ..in_root(1)
            }],
            1,
        );

        // When handling ChangeWorkspace.
        IntentHandler::handle(&Intent::ChangeWorkspace, &mut state);

        // Then the mode line says the local checkout is locked.
        assert_eq!(
            state.sessions.error.as_deref(),
            Some("Workspace locked · Local checkout"),
            "a prompted thread should show the lock"
        );
    }

    #[rstest::rstest]
    fn workspace_picker_offers_previous_worktree_with_its_branch() {
        // Given another thread of the project in a worktree on `orb/feat`.
        let worktree = Thread {
            branch: Some("orb/feat".into()),
            ..thread(2, ThreadStatus::Idle)
        };

        // When opening the workspace picker on a root thread.
        let state = choosing_workspace(vec![in_root(1), worktree]);

        // Then the last row offers that worktree by its branch.
        assert_eq!(
            workspace_labels(&state),
            [
                "Current checkout",
                "New worktree",
                "Previous worktree (orb/feat)"
            ],
            "the seed worktree should be offered"
        );
    }

    #[rstest::rstest]
    fn workspace_picker_omits_previous_worktree_without_a_seed() {
        // Given / When opening the workspace picker on a project's only thread.
        let state = choosing_workspace(vec![in_root(1)]);

        // Then there is no previous-worktree row.
        assert_eq!(
            workspace_labels(&state),
            ["Current checkout", "New worktree"],
            "no seed means no previous worktree"
        );
    }

    #[rstest::rstest]
    fn current_row_reads_current_worktree_inside_a_worktree() {
        // Given / When opening the workspace picker on a thread in a worktree.
        let state = choosing_workspace(vec![thread(1, ThreadStatus::Idle)]);

        // Then the first row is the current worktree.
        assert_eq!(
            workspace_labels(&state).first().map(String::as_str),
            Some("Current worktree"),
            "a worktree thread's current row names the worktree"
        );
    }

    #[rstest::rstest]
    fn picking_new_worktree_returns_move_thread() {
        // Given the workspace picker with `New worktree` highlighted.
        let mut state = choosing_workspace(vec![in_root(1)]);
        IntentHandler::handle(&Intent::PickerNext, &mut state);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the thread moves to a new worktree.
        assert_eq!(
            commands,
            [Command::MoveThread {
                thread: ThreadId(1),
                to: Workspace::NewWorktree
            }],
            "New worktree should move the thread"
        );
    }

    #[rstest::rstest]
    fn picking_new_worktree_marks_starting() {
        // Given the workspace picker with `New worktree` highlighted.
        let mut state = choosing_workspace(vec![in_root(1)]);
        IntentHandler::handle(&Intent::PickerNext, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then a start is in flight.
        assert!(
            state.sessions.starting,
            "a move should mark the start in flight"
        );
    }

    #[rstest::rstest]
    fn picking_current_returns_no_command() {
        // Given the workspace picker with the current checkout highlighted.
        let mut state = choosing_workspace(vec![in_root(1)]);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then nothing happens.
        assert!(commands.is_empty(), "staying put needs no command");
    }

    /// Local branch `name`, checked out in `worktree` if any.
    fn branch(name: &str, current: bool, worktree: Option<&str>) -> GitRef {
        GitRef {
            name: name.to_owned(),
            remote: false,
            current,
            default: false,
            worktree: worktree.map(PathBuf::from),
        }
    }

    /// The branch picker opened on thread 1 of `threads`, showing `refs`.
    fn choosing_branch(threads: Vec<Thread>, refs: Vec<GitRef>) -> AppState {
        let mut state = AppState {
            focus: Focus::Preview,
            ..state_with(threads, 1)
        };
        IntentHandler::handle(&Intent::SwitchBranch, &mut state);
        if let Some(picker) = &mut state.picker {
            picker.show_branches(Path::new("/work"), refs);
        }
        state
    }

    #[rstest::rstest]
    fn switch_branch_opens_the_branch_picker() {
        // Given a selected prompt-less thread in the root.
        let mut state = state_with(vec![in_root(1)], 1);

        // When handling SwitchBranch.
        IntentHandler::handle(&Intent::SwitchBranch, &mut state);

        // Then its branch picker is open and takes the keys.
        assert_eq!(
            (state.focus, state.picker.as_ref().map(PickerState::kind)),
            (
                Focus::Picker,
                Some(&PickerKind::Branches {
                    target: PickTarget::Thread(ThreadId(1)),
                    cwd: "/work".into(),
                    unstarted: true,
                })
            ),
            "SwitchBranch should open the branch picker"
        );
    }

    #[rstest::rstest]
    fn switch_branch_returns_list_branches_for_the_cwd() {
        // Given a selected thread in the root.
        let mut state = state_with(vec![in_root(1)], 1);

        // When handling SwitchBranch.
        let commands = IntentHandler::handle(&Intent::SwitchBranch, &mut state);

        // Then the root's refs are listed.
        assert_eq!(
            commands,
            [Command::ListBranches("/work".into())],
            "the picker's refs come from the thread's directory"
        );
    }

    #[rstest::rstest]
    fn next_intent_clears_the_error() {
        // Given a failure on the mode line.
        let mut state = state_with(vec![in_root(1)], 1);
        state.sessions.error = Some("Claude is working in this directory".to_owned());

        // When handling the next intent.
        IntentHandler::handle(&Intent::SelectNext, &mut state);

        // Then the failure is gone.
        assert_eq!(
            state.sessions.error, None,
            "the user has seen the error once they press a key"
        );
    }

    #[rstest::rstest]
    fn busy_directory_shows_the_error() {
        // Given a working sibling in the selected thread's directory.
        let mut state = state_with(
            vec![
                in_root(1),
                Thread {
                    cwd: "/work".into(),
                    ..thread(2, ThreadStatus::Working)
                },
            ],
            1,
        );

        // When handling SwitchBranch.
        IntentHandler::handle(&Intent::SwitchBranch, &mut state);

        // Then the mode line says Claude is working there.
        assert_eq!(
            state.sessions.error.as_deref(),
            Some("Claude is working in this directory"),
            "a busy directory should refuse the switch"
        );
    }

    #[rstest::rstest]
    fn picking_a_free_branch_returns_switch_branch() {
        // Given the branch picker with `feat`, checked out nowhere, highlighted.
        let mut state = choosing_branch(
            vec![in_root(1)],
            vec![
                branch("main", true, Some("/work")),
                branch("feat", false, None),
            ],
        );
        IntentHandler::handle(&Intent::PickerNext, &mut state);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then feat is checked out in the thread's directory.
        assert_eq!(
            commands,
            [Command::SwitchBranch {
                thread: ThreadId(1),
                git_ref: branch("feat", false, None),
                to_root: false,
            }],
            "a free branch should be checked out"
        );
    }

    #[rstest::rstest]
    fn unstarted_pick_of_a_branch_in_another_worktree_returns_move_thread() {
        // Given a prompt-less thread's branch picker with `feat`, checked out
        // in another worktree, highlighted.
        let mut state = choosing_branch(
            vec![in_root(1)],
            vec![
                branch("main", true, Some("/work")),
                branch("feat", false, Some("/wt/feat")),
            ],
        );
        IntentHandler::handle(&Intent::PickerNext, &mut state);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the thread moves into that worktree.
        assert_eq!(
            commands,
            [Command::MoveThread {
                thread: ThreadId(1),
                to: Workspace::Existing("/wt/feat".into()),
            }],
            "a prompt-less thread follows the branch into its worktree"
        );
    }

    #[rstest::rstest]
    fn picking_the_current_branch_returns_no_command() {
        // Given the branch picker with the current branch highlighted.
        let mut state =
            choosing_branch(vec![in_root(1)], vec![branch("main", true, Some("/work"))]);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then nothing happens.
        assert!(commands.is_empty(), "the current branch needs no checkout");
    }

    #[rstest::rstest]
    fn picker_input_filters_the_project_picker() {
        // Given the project picker with alpha highlighted.
        let mut state = picking(Focus::Sidebar);

        // When typing `b`.
        IntentHandler::handle(&Intent::PickerInput('b'), &mut state);

        // Then only beta matches, so it's highlighted.
        assert!(
            matches!(
                state.picker.as_ref().and_then(PickerState::selected),
                Some(PickerItem::Project { title, .. }) if title == "beta"
            ),
            "typing b should narrow the picker to beta"
        );
    }

    #[rstest::rstest]
    #[case(Intent::SelectNext)]
    #[case(Intent::SelectPrev)]
    fn selecting_a_thread_returns_show_preview(#[case] intent: Intent) {
        // Given threads 1 and 2 with the first one selected.
        let mut state = state_with(
            vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
            1,
        );

        // When handling the selection intent.
        let commands = IntentHandler::handle(&intent, &mut state);

        // Then the preview actor is asked to show the selection.
        assert!(
            commands.contains(&Command::ShowPreview),
            "{intent:?} should return ShowPreview"
        );
    }

    #[rstest::rstest]
    #[case(Intent::NextBlock, Some(3), 6)]
    #[case(Intent::PrevBlock, Some(1), 4)]
    #[case(Intent::HalfPageDown, Some(3), 11)]
    #[case(Intent::HalfPageUp, Some(1), 1)]
    #[case(Intent::Top, Some(0), 0)]
    #[case(Intent::Bottom, None, 6)]
    fn preview_navigation_intents_move_the_cursor(
        #[case] intent: Intent,
        #[case] cursor: Option<u32>,
        #[case] offset: usize,
    ) {
        // Given eight 4-row blocks in a 10-row viewport, the cursor on block 2
        // and rows 6..16 in view.
        let mut state = AppState {
            preview: Preview {
                blocks: (0..8)
                    .map(|i| Block {
                        id: BlockId(i),
                        parts: 1,
                        kind: BlockKind::You(format!("prompt {i}")),
                    })
                    .collect(),
                cursor: Some(BlockId(2)),
                offset: 6,
                layout: PreviewLayout {
                    rows: 10,
                    heights: vec![4; 8],
                },
                ..Preview::default()
            },
            ..AppState::default()
        };

        // When handling the navigation intent.
        IntentHandler::handle(&intent, &mut state);

        // Then the cursor and view moved where that intent goes.
        assert_eq!(
            (state.preview.cursor, state.preview.offset),
            (cursor.map(BlockId), offset),
            "{intent:?} should move the preview"
        );
    }

    #[rstest::rstest]
    fn toggle_fold_on_tool_block_with_output_opens_it() {
        // Given a tool block with output under the cursor.
        let mut state = previewing(cargo_test(Some("test result: ok")));

        // When handling ToggleFold.
        IntentHandler::handle(&Intent::ToggleFold, &mut state);

        // Then the block is open.
        assert!(
            state.preview.expanded.contains(&BlockId(0)),
            "ToggleFold should open the tool block"
        );
    }

    #[rstest::rstest]
    fn toggle_fold_on_claude_block_leaves_folds_unchanged() {
        // Given a Claude block under the cursor.
        let mut state = previewing(BlockKind::Claude("Fixed **the** bug.".into()));

        // When handling ToggleFold.
        IntentHandler::handle(&Intent::ToggleFold, &mut state);

        // Then nothing is open.
        assert!(
            state.preview.expanded.is_empty(),
            "a Claude block doesn't fold"
        );
    }

    #[rstest::rstest]
    fn yank_on_tool_block_returns_summary_and_output() {
        // Given a tool block with output under the cursor.
        let mut state = previewing(cargo_test(Some("test result: ok")));

        // When handling Yank.
        let commands = IntentHandler::handle(&Intent::Yank, &mut state);

        // Then its summary line and output are copied.
        assert_eq!(
            commands,
            vec![Command::Yank("$ cargo test\ntest result: ok".into())],
            "Yank should copy the summary and output"
        );
    }

    #[rstest::rstest]
    fn yank_on_claude_block_returns_its_markdown() {
        // Given a Claude block under the cursor.
        let mut state = previewing(BlockKind::Claude("Fixed **the** bug.".into()));

        // When handling Yank.
        let commands = IntentHandler::handle(&Intent::Yank, &mut state);

        // Then its markdown source is copied.
        assert_eq!(
            commands,
            vec![Command::Yank("Fixed **the** bug.".into())],
            "Yank should copy Claude's markdown"
        );
    }

    #[rstest::rstest]
    fn yank_without_blocks_returns_no_commands() {
        // Given a preview with no blocks.
        let mut state = AppState::default();

        // When handling Yank.
        let commands = IntentHandler::handle(&Intent::Yank, &mut state);

        // Then nothing is copied.
        assert!(commands.is_empty(), "Yank needs a block to copy");
    }

    #[rstest::rstest]
    #[case(false)]
    #[case(true)]
    fn enter_on_the_shelf_toggles_it(#[case] open: bool) {
        // Given the cursor on the Settled header, with the shelf `open`.
        let mut state = state_at(vec![settled(1)], SidebarItem::SettledShelf);
        state.sessions.shelf_open = open;

        // When handling Attach.
        IntentHandler::handle(&Intent::Attach, &mut state);

        // Then the shelf flipped.
        assert_eq!(
            state.sessions.shelf_open, !open,
            "⏎ on the header should toggle the shelf"
        );
    }

    #[rstest::rstest]
    fn enter_on_the_shelf_returns_no_commands() {
        // Given the cursor on the Settled header.
        let mut state = state_at(vec![settled(1)], SidebarItem::SettledShelf);

        // When handling Attach.
        let commands = IntentHandler::handle(&Intent::Attach, &mut state);

        // Then nothing attaches.
        assert!(commands.is_empty(), "⏎ on the header shouldn't attach");
    }

    #[rstest::rstest]
    fn l_on_the_shelf_opens_it() {
        // Given the cursor on the collapsed Settled header.
        let mut state = state_at(vec![settled(1)], SidebarItem::SettledShelf);

        // When handling OpenShelf.
        IntentHandler::handle(&Intent::OpenShelf, &mut state);

        // Then the shelf is open.
        assert!(
            state.sessions.shelf_open,
            "l on the header should open the shelf"
        );
    }

    #[rstest::rstest]
    fn h_on_a_settled_thread_closes_the_shelf_and_selects_it() {
        // Given the shelf open and the cursor on settled thread 1.
        let mut state = state_with(vec![settled(1)], 1);
        state.sessions.shelf_open = true;

        // When handling CloseShelf.
        IntentHandler::handle(&Intent::CloseShelf, &mut state);

        // Then the shelf is closed with its header selected.
        assert_eq!(
            (state.sessions.shelf_open, state.sessions.cursor),
            (false, Some(SidebarItem::SettledShelf)),
            "h in the shelf should close it onto its header"
        );
    }

    #[rstest::rstest]
    fn h_on_an_active_card_does_nothing() {
        // Given the shelf open and the cursor on active thread 1.
        let mut state = state_with(vec![thread(1, ThreadStatus::Idle), settled(2)], 1);
        state.sessions.shelf_open = true;

        // When handling CloseShelf.
        IntentHandler::handle(&Intent::CloseShelf, &mut state);

        // Then the shelf and cursor are unchanged.
        assert_eq!(
            (state.sessions.shelf_open, state.sessions.cursor),
            (true, Some(SidebarItem::Thread(ThreadId(1)))),
            "h on a card should do nothing"
        );
    }

    #[rstest::rstest]
    fn settle_returns_the_settle_command() {
        // Given threads 2 and 1 in sidebar order, with thread 2 selected.
        let mut state = state_with(
            vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
            2,
        );

        // When handling ToggleSettle.
        let commands = IntentHandler::handle(&Intent::ToggleSettle, &mut state);

        // Then the sessions actor is asked to settle thread 2.
        assert!(
            commands.contains(&Command::Settle(ThreadId(2))),
            "ToggleSettle should return Settle"
        );
    }

    #[rstest::rstest]
    fn settle_selects_the_next_card_below() {
        // Given threads 3, 2 and 1 in sidebar order, with thread 2 selected.
        let mut state = state_with(
            vec![
                thread(1, ThreadStatus::Idle),
                thread(2, ThreadStatus::Idle),
                thread(3, ThreadStatus::Idle),
            ],
            2,
        );

        // When handling ToggleSettle.
        IntentHandler::handle(&Intent::ToggleSettle, &mut state);

        // Then the card below is selected.
        assert_eq!(
            state.sessions.cursor,
            Some(SidebarItem::Thread(ThreadId(1))),
            "settle should select the next card below"
        );
    }

    #[rstest::rstest]
    fn settling_the_last_card_selects_the_card_above() {
        // Given threads 2 and 1 in sidebar order, with thread 1 selected.
        let mut state = state_with(
            vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
            1,
        );

        // When handling ToggleSettle.
        IntentHandler::handle(&Intent::ToggleSettle, &mut state);

        // Then the card above is selected.
        assert_eq!(
            state.sessions.cursor,
            Some(SidebarItem::Thread(ThreadId(2))),
            "settling the last card should select the one above"
        );
    }

    #[rstest::rstest]
    fn settling_the_only_card_selects_the_shelf() {
        // Given one thread, selected.
        let mut state = state_with(vec![thread(1, ThreadStatus::Idle)], 1);

        // When handling ToggleSettle.
        IntentHandler::handle(&Intent::ToggleSettle, &mut state);

        // Then the shelf header the settle creates is selected.
        assert_eq!(
            state.sessions.cursor,
            Some(SidebarItem::SettledShelf),
            "settling the only card should select the shelf"
        );
    }

    #[rstest::rstest]
    fn settle_on_a_settled_thread_returns_unsettle() {
        // Given a selected settled thread.
        let mut state = state_with(vec![settled(1)], 1);

        // When handling ToggleSettle.
        let commands = IntentHandler::handle(&Intent::ToggleSettle, &mut state);

        // Then the sessions actor is asked to un-settle it.
        assert_eq!(
            commands,
            vec![Command::Unsettle(ThreadId(1))],
            "ToggleSettle on a settled thread should return Unsettle"
        );
    }

    #[rstest::rstest]
    fn settle_on_a_settled_thread_keeps_the_cursor() {
        // Given a selected settled thread.
        let mut state = state_with(vec![settled(1)], 1);

        // When handling ToggleSettle.
        IntentHandler::handle(&Intent::ToggleSettle, &mut state);

        // Then the cursor stays on it.
        assert_eq!(
            state.sessions.cursor,
            Some(SidebarItem::Thread(ThreadId(1))),
            "un-settle should keep the cursor on the thread"
        );
    }

    #[rstest::rstest]
    fn toggle_pin_on_a_pinned_thread_returns_unpin() {
        // Given a selected pinned thread.
        let mut state = state_with(
            vec![Thread {
                pinned_at: Some(at(1)),
                ..thread(1, ThreadStatus::Idle)
            }],
            1,
        );

        // When handling TogglePin.
        let commands = IntentHandler::handle(&Intent::TogglePin, &mut state);

        // Then the sessions actor is asked to unpin it.
        assert_eq!(
            commands,
            vec![Command::Unpin(ThreadId(1))],
            "TogglePin on a pinned thread should return Unpin"
        );
    }

    #[rstest::rstest]
    fn delete_returns_the_delete_command() {
        // Given one thread, selected.
        let mut state = state_with(vec![thread(1, ThreadStatus::Idle)], 1);

        // When handling DeleteThread.
        let commands = IntentHandler::handle(&Intent::DeleteThread, &mut state);

        // Then the sessions actor is asked to delete it.
        assert!(
            commands.contains(&Command::Delete(ThreadId(1))),
            "DeleteThread should return Delete"
        );
    }

    #[rstest::rstest]
    fn delete_selects_the_next_row_below() {
        // Given cards 2 and 1, then the open shelf holding thread 3, with card
        // 1 selected.
        let mut state = state_with(
            vec![
                thread(1, ThreadStatus::Idle),
                thread(2, ThreadStatus::Idle),
                settled(3),
            ],
            1,
        );
        state.sessions.shelf_open = true;

        // When handling DeleteThread.
        IntentHandler::handle(&Intent::DeleteThread, &mut state);

        // Then the settled thread below, past the header, is selected.
        assert_eq!(
            state.sessions.cursor,
            Some(SidebarItem::Thread(ThreadId(3))),
            "delete should select the next thread row below"
        );
    }

    #[rstest::rstest]
    #[case(Intent::SelectFirst, 3)]
    #[case(Intent::SelectLast, 1)]
    #[case(Intent::SelectHalfPageDown, 1)]
    #[case(Intent::SelectHalfPageUp, 3)]
    fn sidebar_jumps_show_and_visit_the_thread_they_land_on(
        #[case] intent: Intent,
        #[case] expected: i64,
    ) {
        // Given threads 3, 2 and 1 in sidebar order, with thread 2 selected.
        let mut state = state_with(
            vec![
                thread(1, ThreadStatus::Idle),
                thread(2, ThreadStatus::Idle),
                thread(3, ThreadStatus::Idle),
            ],
            2,
        );

        // When handling the jump.
        let commands = IntentHandler::handle(&intent, &mut state);

        // Then the preview shows the thread it lands on, which is visited.
        assert_eq!(
            commands,
            vec![Command::ShowPreview, Command::Visit(ThreadId(expected))],
            "{intent:?} should show and visit thread {expected}"
        );
    }

    #[rstest::rstest]
    fn delete_hides_the_thread_from_the_sidebar() {
        // Given threads 2 and 1, with thread 1 selected.
        let mut state = state_with(
            vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
            1,
        );

        // When handling DeleteThread.
        IntentHandler::handle(&Intent::DeleteThread, &mut state);

        // Then only thread 2 is listed, while its session is removed.
        let listed: Vec<SidebarItem> = state
            .sessions
            .sidebar()
            .iter()
            .map(SidebarRow::item)
            .collect();
        assert_eq!(
            listed,
            vec![SidebarItem::Thread(ThreadId(2))],
            "a deleted thread should leave the sidebar at once"
        );
    }

    #[rstest::rstest]
    fn select_next_returns_visit() {
        // Given threads 2 and 1 in sidebar order, with thread 2 selected.
        let mut state = state_with(
            vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
            2,
        );

        // When handling SelectNext.
        let commands = IntentHandler::handle(&Intent::SelectNext, &mut state);

        // Then the newly selected thread is visited.
        assert!(
            commands.contains(&Command::Visit(ThreadId(1))),
            "SelectNext should visit the thread it lands on"
        );
    }

    /// A draft in `workspace` with no branch, model or permission.
    fn draft(workspace: DraftWorkspace) -> Draft {
        Draft {
            workspace,
            branch: None,
            model: None,
            permission: None,
            created_at: SystemTime::UNIX_EPOCH,
            repo: true,
            from: None,
        }
    }

    /// One project at `/work` holding `threads` and `draft`, with the draft
    /// selected and the keys on its form.
    fn drafting(draft: Draft, threads: Vec<Thread>) -> AppState {
        let mut state = AppState {
            focus: Focus::Preview,
            ..state_at(threads, SidebarItem::Draft(ProjectId(1)))
        };
        if let Some(project) = state.sessions.projects.first_mut() {
            project.draft = Some(draft);
        }
        state
    }

    /// The selected project's draft.
    fn the_draft(state: &AppState) -> Option<&Draft> {
        state.sessions.selected_draft().map(|(_, draft)| draft)
    }

    #[rstest::rstest]
    fn attach_on_a_draft_returns_start_draft() {
        // Given a selected draft.
        let mut state = drafting(draft(DraftWorkspace::Local), vec![]);

        // When handling Attach.
        let commands = IntentHandler::handle(&Intent::Attach, &mut state);

        // Then the sessions actor is asked to start it.
        assert_eq!(
            commands,
            vec![Command::StartDraft(ProjectId(1))],
            "⏎ on a draft should start it"
        );
    }

    #[rstest::rstest]
    fn attach_on_a_draft_sets_starting() {
        // Given a selected draft.
        let mut state = drafting(draft(DraftWorkspace::Local), vec![]);

        // When handling Attach.
        IntentHandler::handle(&Intent::Attach, &mut state);

        // Then a start is in flight.
        assert!(
            state.sessions.starting,
            "starting a draft should mark the start in flight"
        );
    }

    #[rstest::rstest]
    fn attach_on_a_draft_while_starting_returns_no_commands() {
        // Given a selected draft while a start is in flight.
        let mut state = drafting(draft(DraftWorkspace::Local), vec![]);
        state.sessions.starting = true;

        // When handling Attach.
        let commands = IntentHandler::handle(&Intent::Attach, &mut state);

        // Then nothing starts.
        assert!(commands.is_empty(), "one start at a time");
    }

    #[rstest::rstest]
    fn change_workspace_on_a_draft_opens_its_workspace_picker() {
        // Given a selected draft.
        let mut state = drafting(draft(DraftWorkspace::Local), vec![]);

        // When handling ChangeWorkspace.
        IntentHandler::handle(&Intent::ChangeWorkspace, &mut state);

        // Then the draft's workspace picker is open.
        assert_eq!(
            state.picker.as_ref().map(PickerState::kind),
            Some(&PickerKind::Workspace {
                target: PickTarget::Draft(ProjectId(1))
            }),
            "␣w on a draft should open its workspace picker"
        );
    }

    #[rstest::rstest]
    #[case(0, DraftWorkspace::Local, Some("main"))]
    #[case(1, DraftWorkspace::NewWorktree, None)]
    #[case(2, DraftWorkspace::Existing("/work/2".into()), Some("orb/feat"))]
    fn picking_a_draft_workspace_sets_it(
        #[case] row: usize,
        #[case] workspace: DraftWorkspace,
        #[case] branch: Option<&str>,
    ) {
        // Given a local draft on `main`, whose project has a worktree thread
        // on `orb/feat`, with workspace row `row` highlighted.
        let mut state = drafting(
            Draft {
                branch: Some("main".into()),
                ..draft(DraftWorkspace::Local)
            },
            vec![Thread {
                branch: Some("orb/feat".into()),
                ..thread(2, ThreadStatus::Idle)
            }],
        );
        IntentHandler::handle(&Intent::ChangeWorkspace, &mut state);
        for _ in 0..row {
            IntentHandler::handle(&Intent::PickerNext, &mut state);
        }

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the draft has that workspace and branch; its own workspace,
        // Current checkout, keeps main.
        assert_eq!(
            the_draft(&state).map(|draft| (&draft.workspace, draft.branch.as_deref())),
            Some((&workspace, branch)),
            "workspace row {row} should set the draft's workspace"
        );
    }

    #[rstest::rstest]
    fn picking_a_draft_workspace_returns_save_draft() {
        // Given a draft's workspace picker with `New worktree` highlighted.
        let mut state = drafting(draft(DraftWorkspace::Local), vec![]);
        IntentHandler::handle(&Intent::ChangeWorkspace, &mut state);
        IntentHandler::handle(&Intent::PickerNext, &mut state);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the sessions actor is asked to save the draft.
        assert_eq!(
            commands,
            vec![Command::SaveDraft(ProjectId(1))],
            "a workspace pick should be saved"
        );
    }

    #[rstest::rstest]
    fn switch_branch_on_a_draft_opens_the_roots_branch_picker() {
        // Given a selected new-worktree draft.
        let mut state = drafting(draft(DraftWorkspace::NewWorktree), vec![]);

        // When handling SwitchBranch.
        IntentHandler::handle(&Intent::SwitchBranch, &mut state);

        // Then the draft's branch picker lists the root, nothing disabled.
        assert_eq!(
            state.picker.as_ref().map(PickerState::kind),
            Some(&PickerKind::Branches {
                target: PickTarget::Draft(ProjectId(1)),
                cwd: "/work".into(),
                unstarted: true,
            }),
            "␣b on a draft should open a branch picker over the root"
        );
    }

    #[rstest::rstest]
    fn switch_branch_on_an_existing_worktree_draft_opens_its_worktrees_branch_picker() {
        // Given a selected draft in an existing worktree.
        let mut state = drafting(draft(existing_feat()), vec![]);

        // When handling SwitchBranch.
        IntentHandler::handle(&Intent::SwitchBranch, &mut state);

        // Then the draft's branch picker lists the worktree.
        assert_eq!(
            state.picker.as_ref().map(PickerState::kind),
            Some(&PickerKind::Branches {
                target: PickTarget::Draft(ProjectId(1)),
                cwd: "/wt/feat".into(),
                unstarted: true,
            }),
            "␣b on an existing-worktree draft should list its worktree's refs"
        );
    }

    #[rstest::rstest]
    fn switch_branch_on_an_existing_worktree_draft_lists_the_worktrees_refs() {
        // Given a selected draft in an existing worktree.
        let mut state = drafting(draft(existing_feat()), vec![]);

        // When handling SwitchBranch.
        let commands = IntentHandler::handle(&Intent::SwitchBranch, &mut state);

        // Then the worktree's refs are listed.
        assert_eq!(
            commands,
            vec![Command::ListBranches("/wt/feat".into())],
            "the refs come from the draft's worktree"
        );
    }

    #[rstest::rstest]
    #[case(Intent::ChangeWorkspace)]
    #[case(Intent::SwitchBranch)]
    fn workspace_or_branch_on_a_non_git_draft_offers_git_init(#[case] intent: Intent) {
        // Given a selected draft of a project that isn't a git repository.
        let mut state = drafting(
            Draft {
                repo: false,
                ..draft(DraftWorkspace::Local)
            },
            vec![],
        );

        // When handling ␣w or ␣b.
        IntentHandler::handle(&intent, &mut state);

        // Then the picker offers only Initialize Git.
        assert_eq!(
            state.picker.as_ref().map(|picker| (
                picker.kind().clone(),
                picker
                    .shown()
                    .map(|(item, _)| item.clone())
                    .collect::<Vec<_>>()
            )),
            Some((
                PickerKind::InitGit {
                    project: ProjectId(1)
                },
                vec![PickerItem::InitGit]
            )),
            "{intent:?} on a non-git draft should offer git init"
        );
    }

    #[rstest::rstest]
    fn confirming_initialize_git_returns_init_git() {
        // Given a non-git draft's Initialize Git picker.
        let mut state = drafting(
            Draft {
                repo: false,
                ..draft(DraftWorkspace::Local)
            },
            vec![],
        );
        IntentHandler::handle(&Intent::SwitchBranch, &mut state);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the sessions actor is asked to run git init.
        assert_eq!(
            commands,
            vec![Command::InitGit(ProjectId(1))],
            "⏎ on Initialize Git should init the project"
        );
    }

    #[rstest::rstest]
    fn existing_worktree_drafts_workspace_picker_offers_its_worktree_new_and_previous() {
        // Given a draft in the worktree `/work/2`, whose project also has a
        // worktree thread 3 on `orb/other`.
        let mut state = drafting(
            draft(DraftWorkspace::Existing("/work/2".into())),
            vec![
                thread(2, ThreadStatus::Idle),
                Thread {
                    branch: Some("orb/other".into()),
                    ..thread(3, ThreadStatus::Idle)
                },
            ],
        );

        // When handling ChangeWorkspace.
        IntentHandler::handle(&Intent::ChangeWorkspace, &mut state);

        // Then the rows are Current worktree, New worktree, and the other
        // worktree, with no row for the root.
        assert_eq!(
            state.picker.as_ref().map(|picker| picker
                .shown()
                .filter_map(|(item, _)| match item {
                    PickerItem::Workspace(choice) => Some(choice.label()),
                    _ => None,
                })
                .collect::<Vec<_>>()),
            Some(vec![
                "Current worktree".to_owned(),
                "New worktree".to_owned(),
                "Previous worktree (orb/other)".to_owned(),
            ]),
            "the workspace rows of a draft in a worktree"
        );
    }

    #[rstest::rstest]
    fn confirming_current_worktree_keeps_an_existing_worktree_draft() {
        // Given a draft in `/wt/feat` on feat, its workspace picker just
        // opened.
        let mut state = drafting(
            Draft {
                branch: Some("feat".into()),
                ..draft(existing_feat())
            },
            vec![],
        );
        IntentHandler::handle(&Intent::ChangeWorkspace, &mut state);

        // When confirming straight away.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the draft still runs in its worktree, on feat.
        assert_eq!(
            the_draft(&state).map(|draft| (&draft.workspace, draft.branch.as_deref())),
            Some((&existing_feat(), Some("feat"))),
            "Current worktree should keep the draft where it is"
        );
    }

    #[rstest::rstest]
    fn confirming_new_worktree_keeps_a_new_worktree_drafts_base() {
        // Given a new-worktree draft based on feat, its workspace picker just
        // opened on New worktree.
        let mut state = drafting(
            Draft {
                branch: Some("feat".into()),
                ..draft(DraftWorkspace::NewWorktree)
            },
            vec![],
        );
        IntentHandler::handle(&Intent::ChangeWorkspace, &mut state);

        // When confirming straight away.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then nothing changes, so the base isn't reset.
        assert!(commands.is_empty(), "re-picking the draft's workspace");
    }

    #[rstest::rstest]
    fn new_worktree_drafts_workspace_picker_opens_on_new_worktree() {
        // Given a selected new-worktree draft.
        let mut state = drafting(draft(DraftWorkspace::NewWorktree), vec![]);

        // When handling ChangeWorkspace.
        IntentHandler::handle(&Intent::ChangeWorkspace, &mut state);

        // Then New worktree is selected.
        assert_eq!(
            state.picker.as_ref().and_then(PickerState::selected),
            Some(&PickerItem::Workspace(WorkspaceChoice::NewWorktree)),
            "the picker should open on the draft's workspace"
        );
    }

    #[rstest::rstest]
    fn switch_branch_on_a_local_draft_with_a_busy_root_shows_the_error() {
        // Given a selected local draft and a thread working in the root.
        let mut state = drafting(
            draft(DraftWorkspace::Local),
            vec![Thread {
                cwd: "/work".into(),
                ..thread(1, ThreadStatus::Working)
            }],
        );

        // When handling SwitchBranch.
        IntentHandler::handle(&Intent::SwitchBranch, &mut state);

        // Then the mode line says Claude is working there.
        assert_eq!(
            state.sessions.error.as_deref(),
            Some("Claude is working in this directory"),
            "a busy root should refuse a local draft's checkout"
        );
    }

    /// The branch picker of a draft in `workspace`, showing `refs` listed in
    /// the draft's directory, with the second ref highlighted when there is
    /// one.
    fn choosing_draft_branch(workspace: DraftWorkspace, refs: Vec<GitRef>) -> AppState {
        let mut state = drafting(draft(workspace), vec![]);
        IntentHandler::handle(&Intent::SwitchBranch, &mut state);
        if let Some(picker) = &mut state.picker
            && let PickerKind::Branches { cwd, .. } = picker.kind().clone()
        {
            picker.show_branches(&cwd, refs);
        }
        IntentHandler::handle(&Intent::PickerNext, &mut state);
        state
    }

    /// The worktree `/wt/feat`'s refs: its current `feat`, then `refs`.
    fn in_feat_worktree(refs: Vec<GitRef>) -> Vec<GitRef> {
        std::iter::once(branch("feat", true, Some("/wt/feat")))
            .chain(refs)
            .collect()
    }

    fn existing_feat() -> DraftWorkspace {
        DraftWorkspace::Existing("/wt/feat".into())
    }

    #[rstest::rstest]
    fn picking_the_roots_branch_moves_an_existing_worktree_draft_to_the_root() {
        // Given a draft in the worktree `/wt/feat` whose branch picker has
        // `main`, checked out in the root, highlighted.
        let mut state = choosing_draft_branch(
            existing_feat(),
            in_feat_worktree(vec![branch("main", false, Some("/work"))]),
        );

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the draft runs in the root, on main.
        assert_eq!(
            the_draft(&state).map(|draft| (&draft.workspace, draft.branch.as_deref())),
            Some((&DraftWorkspace::Local, Some("main"))),
            "a branch checked out in the root takes the draft back there"
        );
    }

    #[rstest::rstest]
    fn picking_a_branch_in_another_worktree_moves_an_existing_worktree_draft_there() {
        // Given a draft in `/wt/feat` whose branch picker has `dev`, checked
        // out in `/wt/dev`, highlighted.
        let mut state = choosing_draft_branch(
            existing_feat(),
            in_feat_worktree(vec![branch("dev", false, Some("/wt/dev"))]),
        );

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the draft runs in `/wt/dev`, on dev.
        assert_eq!(
            the_draft(&state).map(|draft| (&draft.workspace, draft.branch.as_deref())),
            Some((&DraftWorkspace::Existing("/wt/dev".into()), Some("dev"))),
            "the draft should follow the branch into its worktree"
        );
    }

    #[rstest::rstest]
    fn picking_the_free_default_branch_for_an_existing_worktree_draft_checks_it_out_in_the_root() {
        // Given a draft in `/wt/feat` whose branch picker has the default
        // branch `main`, checked out nowhere, highlighted.
        let main = GitRef {
            default: true,
            ..branch("main", false, None)
        };
        let mut state =
            choosing_draft_branch(existing_feat(), in_feat_worktree(vec![main.clone()]));

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then main is checked out in the root.
        assert_eq!(
            commands,
            vec![Command::CheckoutDraft {
                project: ProjectId(1),
                git_ref: main,
                cwd: "/work".into(),
            }],
            "the default branch takes the draft back to the root"
        );
    }

    #[rstest::rstest]
    fn picking_a_free_branch_for_an_existing_worktree_draft_checks_it_out_there() {
        // Given a draft in `/wt/feat` whose branch picker has `dev`, checked
        // out nowhere, highlighted.
        let mut state = choosing_draft_branch(
            existing_feat(),
            in_feat_worktree(vec![branch("dev", false, None)]),
        );

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then dev is checked out in the worktree.
        assert_eq!(
            commands,
            vec![Command::CheckoutDraft {
                project: ProjectId(1),
                git_ref: branch("dev", false, None),
                cwd: "/wt/feat".into(),
            }],
            "any other branch is checked out in the draft's worktree"
        );
    }

    #[rstest::rstest]
    fn picking_the_worktrees_own_branch_for_an_existing_worktree_draft_returns_no_command() {
        // Given a draft in `/wt/feat` whose branch picker has its current
        // `feat` highlighted.
        let mut state = choosing_draft_branch(existing_feat(), in_feat_worktree(vec![]));

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then nothing happens.
        assert!(commands.is_empty(), "the current branch needs no checkout");
    }

    #[rstest::rstest]
    fn new_worktree_drafts_branch_picker_selects_its_base() {
        // Given a new-worktree draft based on `feat`, its branch picker open.
        let mut state = drafting(
            Draft {
                branch: Some("feat".into()),
                ..draft(DraftWorkspace::NewWorktree)
            },
            vec![],
        );
        IntentHandler::handle(&Intent::SwitchBranch, &mut state);

        // When the root's refs arrive, its current `main` first.
        if let Some(picker) = &mut state.picker {
            picker.show_branches(
                Path::new("/work"),
                vec![
                    branch("main", true, Some("/work")),
                    branch("dev", false, None),
                    branch("feat", false, None),
                ],
            );
        }

        // Then feat, the base, is selected.
        assert_eq!(
            state.picker.as_ref().and_then(PickerState::selected),
            Some(&PickerItem::Branch(BranchRow {
                git_ref: branch("feat", false, None),
                disabled: false,
            })),
            "the picker should open on the draft's base"
        );
    }

    /// The root's current `main`, then `feat` checked out in `worktree`.
    fn main_then_feat(worktree: Option<&str>) -> Vec<GitRef> {
        vec![
            branch("main", true, Some("/work")),
            branch("feat", false, worktree),
        ]
    }

    #[rstest::rstest]
    fn picking_a_new_worktree_drafts_branch_sets_its_base() {
        // Given a new-worktree draft's branch picker with `feat` highlighted.
        let mut state = choosing_draft_branch(DraftWorkspace::NewWorktree, main_then_feat(None));

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then feat is the draft's base branch.
        assert_eq!(
            the_draft(&state).and_then(|draft| draft.branch.as_deref()),
            Some("feat"),
            "a new worktree draft records the picked base"
        );
    }

    #[rstest::rstest]
    fn picking_a_new_worktree_drafts_branch_returns_save_draft() {
        // Given a new-worktree draft's branch picker with `feat` highlighted.
        let mut state = choosing_draft_branch(DraftWorkspace::NewWorktree, main_then_feat(None));

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the draft is saved, with no checkout.
        assert_eq!(
            commands,
            vec![Command::SaveDraft(ProjectId(1))],
            "a base pick is only saved"
        );
    }

    #[rstest::rstest]
    fn picking_a_branch_in_a_worktree_moves_a_local_draft_there() {
        // Given a local draft's branch picker with `feat`, checked out in
        // another worktree, highlighted.
        let mut state =
            choosing_draft_branch(DraftWorkspace::Local, main_then_feat(Some("/wt/feat")));

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the draft runs in that worktree, on feat.
        assert_eq!(
            the_draft(&state).map(|draft| (&draft.workspace, draft.branch.as_deref())),
            Some((&DraftWorkspace::Existing("/wt/feat".into()), Some("feat"))),
            "the draft should follow the branch into its worktree"
        );
    }

    #[rstest::rstest]
    fn picking_a_branch_in_a_worktree_for_a_local_draft_returns_save_draft() {
        // Given a local draft's branch picker with `feat`, checked out in
        // another worktree, highlighted.
        let mut state =
            choosing_draft_branch(DraftWorkspace::Local, main_then_feat(Some("/wt/feat")));

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the draft is saved, with no checkout.
        assert_eq!(
            commands,
            vec![Command::SaveDraft(ProjectId(1))],
            "moving a draft to a worktree is only saved"
        );
    }

    #[rstest::rstest]
    fn picking_a_free_branch_for_a_local_draft_returns_checkout_draft() {
        // Given a local draft's branch picker with `feat`, checked out
        // nowhere, highlighted.
        let mut state = choosing_draft_branch(DraftWorkspace::Local, main_then_feat(None));

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then feat is checked out in the root.
        assert_eq!(
            commands,
            vec![Command::CheckoutDraft {
                project: ProjectId(1),
                git_ref: branch("feat", false, None),
                cwd: "/work".into(),
            }],
            "a local draft checks the branch out in the root"
        );
    }

    #[rstest::rstest]
    fn picking_the_current_branch_for_a_local_draft_returns_no_command() {
        // Given a local draft's branch picker with the root's current branch
        // highlighted.
        let mut state = choosing_draft_branch(
            DraftWorkspace::Local,
            vec![branch("main", true, Some("/work"))],
        );

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then nothing happens.
        assert!(commands.is_empty(), "the current branch needs no checkout");
    }

    #[rstest::rstest]
    fn pick_model_on_a_thread_opens_no_picker() {
        // Given a selected thread.
        let mut state = state_with(vec![in_root(1)], 1);

        // When handling PickModel.
        IntentHandler::handle(&Intent::PickModel, &mut state);

        // Then no picker opens.
        assert!(state.picker.is_none(), "only a draft has a model to pick");
    }

    #[rstest::rstest]
    fn pick_model_on_a_draft_lists_default_first() {
        // Given a selected draft.
        let mut state = drafting(draft(DraftWorkspace::Local), vec![]);

        // When handling PickModel.
        IntentHandler::handle(&Intent::PickModel, &mut state);

        // Then the draft's model picker is open with Default first.
        assert_eq!(
            state.picker.as_ref().map(|picker| (
                picker.kind().clone(),
                picker.shown().next().map(|(item, _)| item.clone())
            )),
            Some((
                PickerKind::Model {
                    project: ProjectId(1)
                },
                Some(PickerItem::Setting(None))
            )),
            "␣m on a draft should open its model picker, Default first"
        );
    }

    #[rstest::rstest]
    fn pick_model_on_a_draft_selects_its_model() {
        // Given a selected draft on Claude Sonnet 5.
        let mut state = drafting(
            Draft {
                model: Some("claude-sonnet-5".into()),
                ..draft(DraftWorkspace::Local)
            },
            vec![],
        );

        // When handling PickModel.
        IntentHandler::handle(&Intent::PickModel, &mut state);

        // Then Claude Sonnet 5 is selected.
        assert_eq!(
            state.picker.as_ref().and_then(PickerState::selected),
            Some(&PickerItem::Setting(Some("claude-sonnet-5"))),
            "the draft's model should be selected"
        );
    }

    #[rstest::rstest]
    fn picking_a_model_sets_the_drafts_model() {
        // Given a draft's model picker with Claude Opus 5.5, after Default,
        // highlighted.
        let mut state = drafting(draft(DraftWorkspace::Local), vec![]);
        IntentHandler::handle(&Intent::PickModel, &mut state);
        IntentHandler::handle(&Intent::PickerNext, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the draft runs claude-opus-5-5.
        assert_eq!(
            the_draft(&state).and_then(|draft| draft.model.as_deref()),
            Some("claude-opus-5-5"),
            "the picked model should be the draft's"
        );
    }

    #[rstest::rstest]
    fn picking_a_model_returns_save_draft() {
        // Given a draft's model picker with Claude Opus 5.5 highlighted.
        let mut state = drafting(draft(DraftWorkspace::Local), vec![]);
        IntentHandler::handle(&Intent::PickModel, &mut state);
        IntentHandler::handle(&Intent::PickerNext, &mut state);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the sessions actor is asked to save the draft.
        assert_eq!(
            commands,
            vec![Command::SaveDraft(ProjectId(1))],
            "a model pick should be saved"
        );
    }

    #[rstest::rstest]
    fn picking_default_permission_clears_the_drafts_permission() {
        // Given a draft in plan mode whose permission picker has Default
        // highlighted.
        let mut state = drafting(
            Draft {
                permission: Some("plan".into()),
                ..draft(DraftWorkspace::Local)
            },
            vec![],
        );
        IntentHandler::handle(&Intent::PickPermission, &mut state);
        for _ in PERMISSION_MODES {
            IntentHandler::handle(&Intent::PickerPrev, &mut state);
        }

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the draft has no permission flag.
        assert_eq!(
            the_draft(&state).map(|draft| draft.permission.as_deref()),
            Some(None),
            "Default should clear the draft's permission mode"
        );
    }

    #[rstest::rstest]
    fn delete_on_a_draft_returns_discard_draft() {
        // Given a selected draft.
        let mut state = drafting(draft(DraftWorkspace::Local), vec![]);

        // When handling DeleteThread.
        let commands = IntentHandler::handle(&Intent::DeleteThread, &mut state);

        // Then the sessions actor is asked to discard it.
        assert!(
            commands.contains(&Command::DiscardDraft(ProjectId(1))),
            "xx on a draft should discard it"
        );
    }

    #[rstest::rstest]
    fn delete_on_a_draft_selects_the_next_row() {
        // Given a selected draft above thread 1's card.
        let mut state = drafting(
            draft(DraftWorkspace::Local),
            vec![thread(1, ThreadStatus::Idle)],
        );

        // When handling DeleteThread.
        IntentHandler::handle(&Intent::DeleteThread, &mut state);

        // Then the card below is selected.
        assert_eq!(
            state.sessions.cursor,
            Some(SidebarItem::Thread(ThreadId(1))),
            "discarding a draft should select the row below"
        );
    }

    #[rstest::rstest]
    #[case(Intent::TogglePin)]
    #[case(Intent::ToggleSettle)]
    fn pin_and_settle_on_a_draft_return_no_commands(#[case] intent: Intent) {
        // Given a selected draft.
        let mut state = drafting(draft(DraftWorkspace::Local), vec![]);

        // When handling the intent.
        let commands = IntentHandler::handle(&intent, &mut state);

        // Then nothing happens.
        assert!(commands.is_empty(), "{intent:?} does nothing on a draft");
    }

    #[rstest::rstest]
    #[case(Tool::Shell)]
    #[case(Tool::Lazygit)]
    #[case(Tool::Nvim)]
    fn open_tool_on_a_thread_opens_it_in_the_threads_directory(#[case] tool: Tool) {
        // Given a selected thread in `/work/1`.
        let mut state = state_with(vec![thread(1, ThreadStatus::Idle)], 1);

        // When handling OpenTool.
        let commands = IntentHandler::handle(&Intent::OpenTool(tool), &mut state);

        // Then the tool opens in the thread's directory.
        assert_eq!(
            commands,
            vec![Command::OpenTool {
                tool,
                cwd: "/work/1".into(),
            }],
            "the tool should open where the thread runs"
        );
    }

    #[rstest::rstest]
    fn open_tool_on_an_existing_worktree_draft_opens_it_in_the_worktree() {
        // Given a selected draft in the existing worktree `/wt/feat`.
        let mut state = drafting(draft(existing_feat()), vec![]);

        // When handling OpenTool.
        let commands = IntentHandler::handle(&Intent::OpenTool(Tool::Lazygit), &mut state);

        // Then lazygit opens in the worktree.
        assert_eq!(
            commands,
            vec![Command::OpenTool {
                tool: Tool::Lazygit,
                cwd: "/wt/feat".into(),
            }],
            "an existing-worktree draft's tools open in its worktree"
        );
    }

    #[rstest::rstest]
    #[case(DraftWorkspace::Local)]
    #[case(DraftWorkspace::NewWorktree)]
    fn open_tool_on_a_draft_without_a_worktree_opens_it_in_the_root(
        #[case] workspace: DraftWorkspace,
    ) {
        // Given a selected local or new-worktree draft of the project at `/work`.
        let mut state = drafting(draft(workspace), vec![]);

        // When handling OpenTool.
        let commands = IntentHandler::handle(&Intent::OpenTool(Tool::Lazygit), &mut state);

        // Then lazygit opens in the project's root.
        assert_eq!(
            commands,
            vec![Command::OpenTool {
                tool: Tool::Lazygit,
                cwd: "/work".into(),
            }],
            "a draft without a worktree works in the project's root"
        );
    }

    #[rstest::rstest]
    fn open_tool_without_a_selection_returns_no_commands() {
        // Given nothing selected.
        let mut state = AppState::default();

        // When handling OpenTool.
        let commands = IntentHandler::handle(&Intent::OpenTool(Tool::Shell), &mut state);

        // Then nothing opens.
        assert!(commands.is_empty(), "a tool needs a thread or draft");
    }
}
