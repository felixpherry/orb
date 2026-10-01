//! The [`IntentHandler`]: the single decision point for all user input.

use std::path::{Path, PathBuf};

use crate::command::Workspace;
use crate::feat::dashboard::items;
use crate::feat::git::validator::{
    BUSY_DIRECTORY, ChangeWorkspaceError, SwitchBranchError, validate_change_workspace,
    validate_switch_branch,
};
use crate::feat::git::worktree::previous_worktree;
use crate::feat::jumps::validator::{validate_jump_back, validate_jump_forward};
use crate::feat::pane::validator::{validate_attach, validate_detach};
use crate::feat::picker::list::{BranchRow, PickerItem, WorkspaceChoice};
use crate::feat::picker::state::{DraftTarget, PickTarget, PickerKind, PickerState};
use crate::feat::picker::validator::{
    validate_add_directory, validate_open_directory, validate_pick_project, validate_remove_project,
};
use crate::feat::sessions::state::{
    AttachTarget, Draft, DraftWorkspace, Group, GroupId, GroupKind, Project, ProjectId,
    ProjectKind, Search, Sessions, SidebarItem, ThreadId, group_slug,
};
use crate::feat::sessions::validator::{
    DeleteError, LAST_IN_GROUP, NewGroupError, NewSiblingError, SETTLE_IN_PROGRESS,
    STARTS_FROM_DRAFT, ToggleSettleError, validate_close_group, validate_close_shelf,
    validate_delete, validate_new_group, validate_new_incognito, validate_new_sibling,
    validate_open_group, validate_open_shelf, validate_pick_setting, validate_start_draft,
    validate_start_group_draft, validate_toggle_pin, validate_toggle_settle,
};
use crate::feat::sidebar::state::{Rename, RenameTarget};
use crate::feat::sidebar::validator::{validate_focus_sidebar, validate_rename, validate_resize};
use crate::feat::zellij::validator::validate_open_tool;
use crate::{AppState, Command, Focus, Intent, TextInput};

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
                with_visit(state, vec![])
            }
            Intent::SelectPrev => {
                state.sessions.select_prev();
                with_visit(state, vec![])
            }
            Intent::SelectFirst => {
                let from = state.sessions.cursor;
                state.sessions.select_first();
                let commands = record_jump(state, from);
                with_visit(state, commands)
            }
            Intent::SelectLast => {
                let from = state.sessions.cursor;
                state.sessions.select_last();
                let commands = record_jump(state, from);
                with_visit(state, commands)
            }
            Intent::SelectHalfPageDown => {
                state.sessions.half_page_down(&state.sidebar.layout);
                with_visit(state, vec![])
            }
            Intent::SelectHalfPageUp => {
                state.sessions.half_page_up(&state.sidebar.layout);
                with_visit(state, vec![])
            }
            Intent::FocusRight => focus_right(state),
            Intent::DashboardNext => {
                state.dashboard.next(&state.sessions);
                vec![]
            }
            Intent::DashboardPrev => {
                state.dashboard.prev(&state.sessions);
                vec![]
            }
            Intent::DashboardRun => {
                let items = items(&state.sessions);
                match items.get(state.dashboard.index(&state.sessions, items.len())) {
                    Some(item) => Self::handle(&item.intent(), state),
                    None => vec![],
                }
            }
            Intent::FocusSidebar => match validate_focus_sidebar(state) {
                Ok(()) => {
                    state.focus = Focus::Sidebar;
                    vec![]
                }
                Err(_) => vec![],
            },
            Intent::ToggleSidebar => match (state.sidebar.hidden, state.focus) {
                // `<C-b>` in the Claude pane: the keys stay in the pane.
                (hidden, Focus::Attached) => {
                    state.sidebar.hidden = !hidden;
                    vec![]
                }
                (true, _) => {
                    state.sidebar.hidden = false;
                    state.focus = Focus::Sidebar;
                    vec![]
                }
                (false, Focus::Sidebar) => {
                    state.sidebar.hidden = true;
                    focus_right(state)
                }
                (false, _) => {
                    state.sidebar.hidden = true;
                    vec![]
                }
            },
            Intent::WidenFocused | Intent::NarrowFocused => {
                // Widening the right side narrows the sidebar.
                let changed = match (validate_resize(state), state.focus, intent) {
                    (Ok(()), Focus::Sidebar, Intent::WidenFocused)
                    | (Ok(()), Focus::Dashboard | Focus::Attached, Intent::NarrowFocused) => {
                        state.sidebar.widen()
                    }
                    (Ok(()), Focus::Sidebar | Focus::Dashboard | Focus::Attached, _) => {
                        state.sidebar.narrow()
                    }
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
            Intent::Attach if matches!(state.sessions.cursor, Some(SidebarItem::Group(_))) => {
                if let Some(SidebarItem::Group(id)) = state.sessions.cursor {
                    state.sessions.toggle_group(id);
                }
                vec![]
            }
            Intent::Attach if matches!(state.sessions.cursor, Some(SidebarItem::GroupDraft(_))) => {
                match (validate_start_group_draft(state), state.sessions.cursor) {
                    (Ok(()), Some(SidebarItem::GroupDraft(group))) => {
                        state.sessions.starting = true;
                        vec![Command::StartGroupDraft(group)]
                    }
                    _ => vec![],
                }
            }
            Intent::Attach => attach_thread(state),
            Intent::Detach => {
                state.focus = Focus::Dashboard;
                if let Some(id) = state.sessions.selected_id() {
                    state.attached.remove(&id);
                }
                vec![Command::Detach, Command::RefreshSessions]
            }
            Intent::LeavePane => match validate_focus_sidebar(state) {
                Ok(()) => {
                    state.focus = Focus::Sidebar;
                    vec![Command::Detach, Command::RefreshSessions]
                }
                Err(_) => vec![],
            },
            Intent::JumpBack => match validate_jump_back(state) {
                Ok(()) => {
                    let target = state
                        .jumps
                        .back(state.sessions.cursor, |item| state.sessions.jumpable(item));
                    land(state, target)
                }
                Err(_) => vec![],
            },
            Intent::JumpForward => match validate_jump_forward(state) {
                Ok(()) => {
                    let target = state
                        .jumps
                        .forward(state.sessions.cursor, |item| state.sessions.jumpable(item));
                    land(state, target)
                }
                Err(_) => vec![],
            },
            Intent::DetachSelected => {
                match (validate_detach(state), state.sessions.selected_id()) {
                    (Ok(()), Some(id)) => {
                        state.attached.remove(&id);
                        vec![Command::RefreshSessions]
                    }
                    _ => vec![],
                }
            }
            Intent::NewSession => {
                let items = state
                    .sessions
                    .projects_by_recency()
                    .into_iter()
                    .map(project_item)
                    .collect();
                open_picker(state, PickerState::projects(items, state.focus));
                vec![]
            }
            Intent::NewIncognito => match (
                validate_new_incognito(state),
                state
                    .sessions
                    .own_project(ProjectKind::Incognito)
                    .map(|project| project.id),
            ) {
                (Ok(()), Some(project)) => open_draft(state, project),
                _ => vec![],
            },
            Intent::FilterProjects => {
                let items = std::iter::once(PickerItem::AllProjects)
                    .chain(
                        [
                            ProjectKind::Research,
                            ProjectKind::Learn,
                            ProjectKind::Incognito,
                        ]
                        .into_iter()
                        .filter_map(|kind| state.sessions.own_project(kind))
                        .map(project_item),
                    )
                    .chain(
                        state
                            .sessions
                            .projects_by_recency()
                            .into_iter()
                            .map(project_item),
                    )
                    .collect();
                let picker = PickerState::project_filter(items, state.sessions.filter, state.focus);
                open_picker(state, picker);
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
            Intent::PickerInput(_)
            | Intent::PickerBackspace
            | Intent::PickerDeleteWord
            | Intent::PickerCursorLeft
            | Intent::PickerCursorRight
            | Intent::PickerNext
            | Intent::PickerPrev
            | Intent::PickerHalfPageDown
            | Intent::PickerHalfPageUp
            | Intent::PickerConfirm
            | Intent::PickerOpen
            | Intent::PickerCancel
            | Intent::PickerRemove
                if state.focus == Focus::Rename =>
            {
                rename_key(intent, state)
            }
            Intent::PickerInput(_)
            | Intent::PickerBackspace
            | Intent::PickerDeleteWord
            | Intent::PickerCursorLeft
            | Intent::PickerCursorRight
            | Intent::PickerNext
            | Intent::PickerPrev
            | Intent::PickerHalfPageDown
            | Intent::PickerHalfPageUp
            | Intent::PickerConfirm
            | Intent::PickerOpen
            | Intent::PickerCancel
            | Intent::PickerRemove
                if state.focus == Focus::Search =>
            {
                search_key(intent, state)
            }
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
                state.sessions.selected_group(),
            ) {
                (Ok(()), Some((project, draft)), ..) if !draft.repo => {
                    open_picker(state, PickerState::init_git(project.id, state.focus));
                    vec![]
                }
                (Ok(()), Some((project, draft)), ..) => {
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
                (Ok(()), None, Some(thread), _) => {
                    let cwd = thread.cwd.clone();
                    let unstarted = thread.transcript.is_none() && !thread.status.in_progress();
                    let target = PickTarget::Thread(thread.id);
                    let picker =
                        PickerState::branches(target, cwd.clone(), unstarted, None, state.focus);
                    open_picker(state, picker);
                    vec![Command::ListBranches(cwd)]
                }
                (Ok(()), None, None, Some((_, group))) => match &group.dir {
                    Some(dir) => {
                        let cwd = dir.clone();
                        let target = PickTarget::Group(group.id);
                        let picker =
                            PickerState::branches(target, cwd.clone(), false, None, state.focus);
                        open_picker(state, picker);
                        vec![Command::ListBranches(cwd)]
                    }
                    None => vec![],
                },
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
                state.sessions.selected_group(),
            ) {
                (Ok(()), Some((project, draft)), ..) => vec![Command::OpenTool {
                    tool: *tool,
                    cwd: draft_dir(project, draft),
                }],
                (Ok(()), None, Some(thread), _) => vec![Command::OpenTool {
                    tool: *tool,
                    cwd: thread.cwd.clone(),
                }],
                (Ok(()), None, None, Some((project, group))) => vec![Command::OpenTool {
                    tool: *tool,
                    cwd: group.dir.clone().unwrap_or_else(|| project.root.clone()),
                }],
                _ => vec![],
            },
            Intent::PickModel => {
                let picker = validate_pick_setting(state)
                    .ok()
                    .and_then(|()| setting_target(&state.sessions))
                    .map(|(target, model, _)| PickerState::models(target, model, state.focus));
                if let Some(picker) = picker {
                    open_picker(state, picker);
                }
                vec![]
            }
            Intent::PickPermission => {
                let picker = validate_pick_setting(state)
                    .ok()
                    .and_then(|()| setting_target(&state.sessions))
                    .map(|(target, _, permission)| {
                        PickerState::permissions(target, permission, state.focus)
                    });
                if let Some(picker) = picker {
                    open_picker(state, picker);
                }
                vec![]
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
                Some(&PickerKind::Model { target }) => {
                    match close_picker(state).as_ref().and_then(PickerState::selected) {
                        Some(&PickerItem::Setting(value)) => {
                            edit_setting(state, target, |model, _| {
                                *model = value.map(str::to_owned);
                            })
                        }
                        _ => vec![],
                    }
                }
                Some(&PickerKind::Permission { target }) => {
                    match close_picker(state).as_ref().and_then(PickerState::selected) {
                        Some(&PickerItem::Setting(value)) => {
                            edit_setting(state, target, |_, permission| {
                                *permission = value.map(str::to_owned);
                            })
                        }
                        _ => vec![],
                    }
                }
                Some(&PickerKind::RemoveProject { project }) => {
                    match close_picker(state).as_ref().and_then(PickerState::selected) {
                        Some(PickerItem::Confirm(true)) => remove_project(state, project),
                        _ => vec![],
                    }
                }
                Some(&PickerKind::SettleThread { thread }) => {
                    match close_picker(state).as_ref().and_then(PickerState::selected) {
                        Some(PickerItem::Confirm(true)) => settle(state, thread),
                        _ => vec![],
                    }
                }
                Some(&PickerKind::SettleGroup { group }) => {
                    match close_picker(state).as_ref().and_then(PickerState::selected) {
                        Some(PickerItem::Confirm(true)) => settle_group(state, group),
                        _ => vec![],
                    }
                }
                Some(&PickerKind::DeleteGroup { group, .. }) => {
                    match close_picker(state).as_ref().and_then(PickerState::selected) {
                        Some(PickerItem::Confirm(true))
                            if still_deletable(state, SidebarItem::Group(group)) =>
                        {
                            // Nothing is hidden yet: the sessions actor may
                            // refuse (an unmerged Feature branch), so it hides,
                            // detaches and moves the cursor once it goes ahead.
                            vec![Command::DeleteGroup(group)]
                        }
                        _ => vec![],
                    }
                }
                Some(&PickerKind::DeleteThread { thread }) => {
                    match close_picker(state).as_ref().and_then(PickerState::selected) {
                        Some(PickerItem::Confirm(true))
                            if still_deletable(state, SidebarItem::Thread(thread)) =>
                        {
                            delete_thread(state, thread)
                        }
                        _ => vec![],
                    }
                }
                Some(&PickerKind::DiscardDraft { project }) => {
                    match close_picker(state).as_ref().and_then(PickerState::selected) {
                        Some(PickerItem::Confirm(true))
                            if still_deletable(state, SidebarItem::Draft(project)) =>
                        {
                            discard_draft(state, project)
                        }
                        _ => vec![],
                    }
                }
                Some(PickerKind::TrustWorkspace { .. }) => {
                    match close_picker(state).as_ref().and_then(PickerState::selected) {
                        Some(PickerItem::Confirm(true)) => vec![Command::TrustWorkspace],
                        _ => vec![Command::DeclineTrust],
                    }
                }
                Some(PickerKind::ProjectFilter) => {
                    match close_picker(state).as_ref().and_then(PickerState::selected) {
                        Some(PickerItem::AllProjects) => filter_to(state, None),
                        Some(&PickerItem::Project { id, .. }) => filter_to(state, Some(id)),
                        _ => vec![],
                    }
                }
                Some(PickerKind::GroupProject) => {
                    match close_picker(state).as_ref().and_then(PickerState::selected) {
                        Some(&PickerItem::Project { id, .. }) => {
                            open_group_name(state, GroupKind::Feature, Some(id))
                        }
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
                Some(PickerKind::Branches {
                    target: PickTarget::Group(_),
                    ..
                }) => close_picker(state)
                    .map(|picker| pick_group_branch(&picker))
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
            Intent::PickerCancel => match close_picker(state).as_ref().map(PickerState::kind) {
                Some(PickerKind::TrustWorkspace { .. }) => vec![Command::DeclineTrust],
                _ => vec![],
            },
            Intent::PickerRemove => match (
                validate_remove_project(state),
                state.picker.as_ref().and_then(PickerState::selected),
            ) {
                (Ok(()), Some(&PickerItem::Project { id, .. })) => {
                    let return_to = state
                        .picker
                        .as_ref()
                        .map_or(Focus::Sidebar, PickerState::return_to);
                    state.picker = Some(PickerState::remove_project(id, return_to));
                    vec![]
                }
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
                    vec![]
                }
                Err(_) => vec![],
            },
            Intent::OpenGroup => match (validate_open_group(state), selected_group_id(state)) {
                (Ok(()), Some(id)) => {
                    state.sessions.open_group(id);
                    vec![]
                }
                _ => vec![],
            },
            Intent::CloseGroup => match (validate_close_group(state), selected_group_id(state)) {
                (Ok(()), Some(id)) => {
                    state.sessions.close_group(id);
                    vec![]
                }
                _ => vec![],
            },
            Intent::NewGroup(GroupKind::Feature) => {
                let items = state
                    .sessions
                    .projects_by_recency()
                    .into_iter()
                    .map(project_item)
                    .collect();
                open_picker(state, PickerState::group_project(items, state.focus));
                vec![]
            }
            Intent::NewGroup(kind) => open_group_name(state, *kind, None),
            Intent::NewSibling => match (validate_new_sibling(state), selected_group_id(state)) {
                (Ok(()), Some(group)) => {
                    let (model, permission_mode) = state
                        .sessions
                        .group(group)
                        .map(|(_, shown)| {
                            (
                                shown.defaults.model.clone(),
                                shown.defaults.permission.clone(),
                            )
                        })
                        .unwrap_or_default();
                    let from = state.sessions.cursor;
                    state.sessions.open_group(group);
                    state.sessions.starting = true;
                    vec![Command::StartSibling {
                        group,
                        model,
                        permission_mode,
                        from,
                    }]
                }
                (Err(NewSiblingError::NoThread), _) => {
                    state.sessions.error = Some(STARTS_FROM_DRAFT.to_owned());
                    vec![]
                }
                _ => vec![],
            },
            Intent::TogglePin if matches!(state.sessions.cursor, Some(SidebarItem::Group(_))) => {
                match (validate_toggle_pin(state), state.sessions.selected_group()) {
                    (Ok(()), Some((_, group))) => match group.pinned_at {
                        Some(_) => vec![Command::UnpinGroup(group.id)],
                        None => vec![Command::PinGroup(group.id)],
                    },
                    _ => vec![],
                }
            }
            Intent::TogglePin => {
                match (validate_toggle_pin(state), state.sessions.selected_thread()) {
                    (Ok(()), Some(thread)) => match thread.pinned_at {
                        Some(_) => vec![Command::Unpin(thread.id)],
                        None => vec![Command::Pin(thread.id)],
                    },
                    _ => vec![],
                }
            }
            Intent::Rename => match (validate_rename(state), state.sessions.selected_thread()) {
                (Ok(()), Some(thread)) => {
                    let rename = Rename {
                        target: RenameTarget::Thread(thread.id),
                        input: TextInput::new(thread.title.as_deref().unwrap_or_default()),
                        creating: false,
                    };
                    state.rename = Some(rename);
                    state.focus = Focus::Rename;
                    vec![]
                }
                _ => vec![],
            },
            Intent::Search => {
                state.sessions.search = Some(Search {
                    input: TextInput::default(),
                    return_to: state.sessions.cursor,
                });
                state.focus = Focus::Search;
                vec![]
            }
            Intent::ToggleSettle
                if matches!(state.sessions.cursor, Some(SidebarItem::Group(_))) =>
            {
                match (
                    validate_toggle_settle(state),
                    state.sessions.selected_group(),
                ) {
                    (Ok(()), Some((_, group))) if group.settled_at.is_some() => {
                        vec![Command::UnsettleGroup(group.id)]
                    }
                    (Ok(()), Some((_, group))) => {
                        let picker = PickerState::settle_group(group.id, state.focus);
                        open_picker(state, picker);
                        vec![]
                    }
                    (Err(ToggleSettleError::InProgress), _) => {
                        state.sessions.error = Some(SETTLE_IN_PROGRESS.to_owned());
                        vec![]
                    }
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
                        let picker = PickerState::settle_thread(thread.id, state.focus);
                        open_picker(state, picker);
                        vec![]
                    }
                    (Err(ToggleSettleError::InProgress), _) => {
                        state.sessions.error = Some(SETTLE_IN_PROGRESS.to_owned());
                        vec![]
                    }
                    _ => vec![],
                }
            }
            Intent::DeleteThread => match (validate_delete(state), state.sessions.cursor) {
                (Ok(()), Some(SidebarItem::Draft(project))) => {
                    open_picker(state, PickerState::discard_draft(project, state.focus));
                    vec![]
                }
                (Ok(()), Some(SidebarItem::Thread(id))) => {
                    open_picker(state, PickerState::delete_thread(id, state.focus));
                    vec![]
                }
                (Ok(()), Some(SidebarItem::Group(group))) => {
                    let dir = state
                        .sessions
                        .selected_group()
                        .and_then(|(_, group)| group.dir.as_ref().map(|_| group.kind));
                    open_picker(state, PickerState::delete_group(group, dir, state.focus));
                    vec![]
                }
                (Err(DeleteError::LastInGroup), _) => {
                    state.sessions.error = Some(LAST_IN_GROUP.to_owned());
                    vec![]
                }
                _ => vec![],
            },
        }
    }
}

/// The picker row for `project`.
fn project_item(project: &Project) -> PickerItem {
    PickerItem::Project {
        id: project.id,
        title: project.title.clone(),
        root: project.root.clone(),
        kind: project.kind,
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

/// What picking the selected branch of `picker`, a group's closed branch
/// picker, asks for: nothing on the branch its worktree is on, else checking
/// the branch out there. Branches checked out in another worktree can't be
/// picked.
fn pick_group_branch(picker: &PickerState) -> Vec<Command> {
    match (picker.kind(), picker.selected()) {
        (
            &PickerKind::Branches {
                target: PickTarget::Group(group),
                ..
            },
            Some(PickerItem::Branch(BranchRow { git_ref, .. })),
        ) if !git_ref.current => vec![Command::CheckoutGroup {
            group,
            git_ref: git_ref.clone(),
        }],
        _ => vec![],
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

/// Filters the sidebar to `filter`'s project, or to all projects, and asks
/// for the filter to be saved.
fn filter_to(state: &mut AppState, filter: Option<ProjectId>) -> Vec<Command> {
    state.sessions.filter_to(filter);
    with_visit(state, vec![Command::SaveUi])
}

/// Asks for `project` to be removed. A filter to it goes back to all
/// projects, and a cursor on its draft moves to the neighbouring row.
fn remove_project(state: &mut AppState, project: ProjectId) -> Vec<Command> {
    if state.sessions.filter == Some(project) {
        state.sessions.filter = None;
    }
    let draft = SidebarItem::Draft(project);
    if state.sessions.cursor == Some(draft) {
        state.sessions.cursor = state.sessions.row_neighbour(draft);
    }
    with_visit(
        state,
        vec![Command::RemoveProject(project), Command::SaveUi],
    )
}

/// Settles `thread`, answered `Yes` in its confirm, if the cursor is still on
/// it and it is still unsettled and between turns. Settling detaches it and
/// moves the cursor to the neighbouring card; a turn that started meanwhile
/// shows the refusal on the mode line.
fn settle(state: &mut AppState, thread: ThreadId) -> Vec<Command> {
    match (
        validate_toggle_settle(state),
        state.sessions.selected_thread(),
    ) {
        (Ok(()), Some(selected)) if selected.id == thread && selected.settled_at.is_none() => {
            state.attached.remove(&thread);
            state.sessions.cursor = state.sessions.card_neighbour(SidebarItem::Thread(thread));
            with_visit(state, vec![Command::Settle(thread)])
        }
        (Err(ToggleSettleError::InProgress), Some(selected)) if selected.id == thread => {
            state.sessions.error = Some(SETTLE_IN_PROGRESS.to_owned());
            vec![]
        }
        _ => vec![],
    }
}

/// Settles `group`, answered `Yes` in its confirm, if the cursor is still on
/// its card and it is still unsettled with no turn underway. Settling
/// detaches its threads and moves the cursor to the neighbouring card; a turn
/// that started meanwhile shows the refusal on the mode line.
fn settle_group(state: &mut AppState, group: GroupId) -> Vec<Command> {
    let on_card = state.sessions.cursor == Some(SidebarItem::Group(group));
    match (
        validate_toggle_settle(state),
        state.sessions.selected_group(),
    ) {
        (Ok(()), Some((_, shown))) if on_card && shown.settled_at.is_none() => {
            let threads: Vec<ThreadId> = state
                .sessions
                .group_threads(group)
                .map(|thread| thread.id)
                .collect();
            for thread in &threads {
                state.attached.remove(thread);
            }
            state.sessions.cursor = state.sessions.card_neighbour(SidebarItem::Group(group));
            with_visit(state, vec![Command::SettleGroup(group)])
        }
        (Err(ToggleSettleError::InProgress), _) if on_card => {
            state.sessions.error = Some(SETTLE_IN_PROGRESS.to_owned());
            vec![]
        }
        _ => vec![],
    }
}

/// Whether `item`, answered `Yes` in its delete or discard confirm, is still
/// under the cursor and can still be deleted.
fn still_deletable(state: &AppState, item: SidebarItem) -> bool {
    state.sessions.cursor == Some(item) && validate_delete(state).is_ok()
}

/// Asks for `thread` to be deleted, hiding it at once, detaching it, dropping
/// it from the jump list and moving the cursor to the neighbouring row.
fn delete_thread(state: &mut AppState, thread: ThreadId) -> Vec<Command> {
    let neighbour = state.sessions.row_neighbour(SidebarItem::Thread(thread));
    state.sessions.deleting.insert(thread);
    state.attached.remove(&thread);
    state.jumps.remove(SidebarItem::Thread(thread));
    state.sessions.cursor = neighbour;
    with_visit(state, vec![Command::Delete(thread), Command::SaveJumps])
}

/// Asks for `project`'s draft to be discarded, dropping it from the jump list
/// and moving the cursor to the neighbouring row.
fn discard_draft(state: &mut AppState, project: ProjectId) -> Vec<Command> {
    state.sessions.cursor = state.sessions.row_neighbour(SidebarItem::Draft(project));
    state.jumps.remove(SidebarItem::Draft(project));
    with_visit(
        state,
        vec![Command::DiscardDraft(project), Command::SaveJumps],
    )
}

/// Selects `project`'s draft and gives the keys to its form, asking the
/// sessions actor to create the draft when the project has none. A filter to
/// another project goes back to all projects. It's a jump.
fn open_draft(state: &mut AppState, project: ProjectId) -> Vec<Command> {
    let from = state.sessions.cursor;
    let exists = state
        .sessions
        .projects
        .iter()
        .any(|p| p.id == project && p.draft.is_some());
    let outside = state
        .sessions
        .filter
        .is_some_and(|filter| filter != project);
    if outside {
        state.sessions.filter = None;
    }
    state.sessions.cursor = Some(SidebarItem::Draft(project));
    state.focus = Focus::Dashboard;
    (!exists)
        .then_some(Command::CreateDraft(project))
        .into_iter()
        .chain(outside.then_some(Command::SaveUi))
        .chain(record_jump(state, from))
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

/// Applies `edit` to `target`'s model and permission and asks the sessions
/// actor to save them; nothing when the draft is gone.
fn edit_setting<F>(state: &mut AppState, target: DraftTarget, edit: F) -> Vec<Command>
where
    F: FnOnce(&mut Option<String>, &mut Option<String>),
{
    match target {
        DraftTarget::Project(project) => edit_draft(state, project, |draft| {
            edit(&mut draft.model, &mut draft.permission);
        }),
        DraftTarget::Group(group) => match state.sessions.group_defaults_mut(group) {
            Some(defaults) => {
                edit(&mut defaults.model, &mut defaults.permission);
                vec![Command::SaveGroupDraft(group)]
            }
            None => vec![],
        },
    }
}

/// What `␣m`/`␣a` set for the cursor, with its current model and
/// permission: a project's draft, or on a group's draft or card the group's
/// defaults.
fn setting_target(sessions: &Sessions) -> Option<(DraftTarget, Option<&str>, Option<&str>)> {
    match (
        sessions.cursor?,
        sessions.selected_draft(),
        sessions.selected_group(),
    ) {
        (SidebarItem::Draft(_), Some((project, draft)), _) => Some((
            DraftTarget::Project(project.id),
            draft.model.as_deref(),
            draft.permission.as_deref(),
        )),
        (SidebarItem::Group(_), _, Some((_, group))) => Some(group_setting(group)),
        (SidebarItem::GroupDraft(_), _, Some((_, group))) if group.draft => {
            Some(group_setting(group))
        }
        _ => None,
    }
}

/// `group`'s defaults as a model or permission picker's target.
fn group_setting(group: &Group) -> (DraftTarget, Option<&str>, Option<&str>) {
    (
        DraftTarget::Group(group.id),
        group.defaults.model.as_deref(),
        group.defaults.permission.as_deref(),
    )
}

/// Opens `picker` and gives it the keys.
fn open_picker(state: &mut AppState, picker: PickerState) {
    state.picker = Some(picker);
    state.focus = Focus::Picker;
}

/// Opens the empty name box for a new `kind` group in `project`.
fn open_group_name(
    state: &mut AppState,
    kind: GroupKind,
    project: Option<ProjectId>,
) -> Vec<Command> {
    state.rename = Some(Rename {
        target: RenameTarget::NewGroup { kind, project },
        input: TextInput::default(),
        creating: false,
    });
    state.focus = Focus::Rename;
    vec![]
}

/// Closes the picker and gives the keys back to where it was opened from.
fn close_picker(state: &mut AppState) -> Option<PickerState> {
    let picker = state.picker.take()?;
    state.focus = picker.return_to();
    Some(picker)
}

/// The id of the group under the cursor: its card's, its draft's, or its
/// thread's.
fn selected_group_id(state: &AppState) -> Option<GroupId> {
    state.sessions.selected_group().map(|(_, group)| group.id)
}

/// Attaches to the selected thread's session and records entering it as a
/// jump, unless the thread can't be attached to or is the row the last
/// `<C-o>`/`<C-i>` landed on.
fn attach_thread(state: &mut AppState) -> Vec<Command> {
    let mut commands = show_pane(state);
    if let (false, Some(id)) = (commands.is_empty(), state.sessions.selected_id())
        && state.jumps.enter(SidebarItem::Thread(id))
    {
        commands.push(Command::SaveJumps);
    }
    commands
}

/// Attaches to the selected thread's session, adding it to the attached
/// threads and showing its pane with the keys in it, unless the thread can't
/// be attached to.
fn show_pane(state: &mut AppState) -> Vec<Command> {
    match (validate_attach(state), state.sessions.selected_thread()) {
        (Ok(()), Some(thread)) => {
            let target = AttachTarget {
                thread: thread.id,
                argv: thread.attach_argv.clone(),
                cwd: thread.cwd.clone(),
            };
            state.focus = Focus::Attached;
            state.attached.insert(target.thread);
            vec![Command::Attach(target), Command::RefreshSessions]
        }
        _ => vec![],
    }
}

/// Moves the keys to the right-hand area: into the Claude pane while the
/// selected thread is attached and can still be attached to, else to the
/// dashboard.
fn focus_right(state: &mut AppState) -> Vec<Command> {
    match state.sessions.selected_id() {
        Some(id) if state.attached.contains(&id) && validate_attach(state).is_ok() => {
            attach_thread(state)
        }
        _ => {
            state.focus = Focus::Dashboard;
            vec![]
        }
    }
}

/// What a picker key does in the rename box: edit the name, save it (a blank
/// name goes back to Claude's title) or cancel, both giving the sidebar back
/// the keys. The picker's other keys do nothing.
fn rename_key(intent: &Intent, state: &mut AppState) -> Vec<Command> {
    match (intent, &mut state.rename) {
        (Intent::PickerInput(ch), Some(rename)) => rename.input.insert(*ch),
        (Intent::PickerBackspace, Some(rename)) => rename.input.backspace(),
        (Intent::PickerDeleteWord, Some(rename)) => rename.input.delete_word(),
        (Intent::PickerCursorLeft, Some(rename)) => rename.input.cursor_left(),
        (Intent::PickerCursorRight, Some(rename)) => rename.input.cursor_right(),
        (
            Intent::PickerConfirm,
            Some(Rename {
                target: RenameTarget::NewGroup { .. },
                ..
            }),
        ) => return confirm_group_name(state),
        (Intent::PickerConfirm, _) => {
            state.focus = Focus::Sidebar;
            return state
                .rename
                .take()
                .and_then(|rename| match rename.target {
                    RenameTarget::Thread(thread) => {
                        let title = rename.input.text().trim();
                        Some(Command::RenameThread {
                            thread,
                            title: (!title.is_empty()).then(|| title.to_owned()),
                        })
                    }
                    RenameTarget::NewGroup { .. } => None,
                })
                .into_iter()
                .collect();
        }
        (Intent::PickerCancel, _) => {
            state.rename = None;
            state.focus = Focus::Sidebar;
        }
        _ => {}
    }
    vec![]
}

/// `⏎` in the name box for a new group: an empty name, or one already asked
/// for, does nothing; an invalid or taken one shows why on the mode line, both
/// keeping the box open; a valid one asks for the group, and the sessions
/// actor closes the box once it's made.
fn confirm_group_name(state: &mut AppState) -> Vec<Command> {
    match validate_new_group(state) {
        Err(NewGroupError::Empty | NewGroupError::Creating) => {}
        Err(NewGroupError::Invalid(what)) => {
            state.sessions.error = Some(format!("Name can't use {what}"));
        }
        Err(NewGroupError::Taken(text)) => state.sessions.error = Some(text),
        Ok(()) => return create_group(state),
    }
    vec![]
}

/// Asks the sessions actor for the group the name box names, keeping the box
/// open until the actor answers.
fn create_group(state: &mut AppState) -> Vec<Command> {
    let Some(Rename {
        target: RenameTarget::NewGroup { kind, project },
        input,
        creating,
    }) = &mut state.rename
    else {
        return vec![];
    };
    *creating = true;
    vec![Command::CreateGroup {
        kind: *kind,
        project: *project,
        name: group_slug(input.text()),
    }]
}

/// What a picker key does in the sidebar search: edit the text, putting the
/// cursor on the first match; move between matches; or end the search,
/// giving the sidebar back the keys. `⏎` keeps the cursor on its match (with
/// no match it acts as `Esc`); `Esc` puts it back where it was. The picker's
/// other keys do nothing.
fn search_key(intent: &Intent, state: &mut AppState) -> Vec<Command> {
    let sessions = &mut state.sessions;
    match (intent, &mut sessions.search) {
        (Intent::PickerInput(ch), Some(search)) => {
            search.input.insert(*ch);
            sessions.select_first_match();
        }
        (Intent::PickerBackspace, Some(search)) => {
            search.input.backspace();
            sessions.select_first_match();
        }
        (Intent::PickerDeleteWord, Some(search)) => {
            search.input.delete_word();
            sessions.select_first_match();
        }
        (Intent::PickerCursorLeft, Some(search)) => {
            search.input.cursor_left();
            return vec![];
        }
        (Intent::PickerCursorRight, Some(search)) => {
            search.input.cursor_right();
            return vec![];
        }
        (Intent::PickerNext, _) => sessions.select_next_match(),
        (Intent::PickerPrev, _) => sessions.select_prev_match(),
        (Intent::PickerConfirm, _) if sessions.cursor.is_some() => {
            let from = sessions.search.take().and_then(|search| search.return_to);
            state.focus = Focus::Sidebar;
            let commands = record_jump(state, from);
            return with_visit(state, commands);
        }
        (Intent::PickerConfirm | Intent::PickerCancel, _) => {
            sessions.cancel_search();
            state.focus = Focus::Sidebar;
        }
        _ => return vec![],
    }
    with_visit(state, vec![])
}

/// Records the move from `from` to the cursor as a jump and asks for the list
/// to be saved; nothing when the cursor didn't move.
fn record_jump(state: &mut AppState, from: Option<SidebarItem>) -> Vec<Command> {
    let to = state.sessions.cursor;
    if from == to {
        return vec![];
    }
    state.jumps.jump(from, to);
    vec![Command::SaveJumps]
}

/// Lands a jump back or forward on `target`: the cursor moves there and its
/// row is revealed. From a pane, the keys follow into the target's pane while
/// orb is attached to it, else go to the sidebar (the dashboard while it's
/// hidden); elsewhere they stay put.
fn land(state: &mut AppState, target: Option<SidebarItem>) -> Vec<Command> {
    let Some(target) = target else {
        return vec![];
    };
    let from_pane = state.focus == Focus::Attached;
    state.sessions.cursor = Some(target);
    state.sessions.reveal(target);
    let pane = match target {
        SidebarItem::Thread(id) if from_pane && state.attached.contains(&id) => show_pane(state),
        _ => vec![],
    };
    let mut commands = vec![Command::SaveJumps];
    match (from_pane, pane.is_empty()) {
        (true, true) => {
            state.focus = match validate_focus_sidebar(state) {
                Ok(()) => Focus::Sidebar,
                Err(_) => Focus::Dashboard,
            };
            commands.extend([Command::Detach, Command::RefreshSessions]);
        }
        _ => commands.extend(pane),
    }
    with_visit(state, commands)
}

/// `commands`, then a visit to the thread under the cursor, if any.
fn with_visit(state: &AppState, mut commands: Vec<Command>) -> Vec<Command> {
    commands.extend(state.sessions.selected_id().map(Command::Visit));
    commands
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    use crate::command::Workspace;
    use crate::feat::git::git_service::GitRef;
    use crate::feat::jumps::state::JumpList;
    use crate::feat::picker::list::{BranchRow, PERMISSION_MODES, PickerItem, WorkspaceChoice};
    use crate::feat::picker::state::{DraftTarget, PickTarget, PickerKind, PickerState};
    use crate::feat::sessions::state::{
        AttachTarget, Draft, DraftWorkspace, Group, GroupDefaults, GroupId, GroupKind, Project,
        ProjectId, ProjectKind, Search, Sessions, SidebarItem, SidebarRow, Thread, ThreadId,
        ThreadStatus,
    };
    use crate::feat::sessions::validator::{LAST_IN_GROUP, SETTLE_IN_PROGRESS, STARTS_FROM_DRAFT};
    use crate::feat::sidebar::state::{Rename, RenameTarget, SidebarView};
    use crate::feat::zellij::zellij_service::Tool;
    use crate::{AppState, Command, Focus, Intent, IntentHandler, TextInput};

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
            group: None,
            model: None,
            permission: None,
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
                    removed: false,
                    draft: None,
                    threads,
                    groups: vec![],
                    kind: ProjectKind::Normal,
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
                        removed: false,
                        draft: None,
                        threads: vec![],
                        groups: vec![],
                        kind: ProjectKind::Normal,
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
            focus: Focus::Dashboard,
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
    fn select_next_on_last_thread_wraps_to_the_first() {
        // Given threads 2 and 1 in sidebar order, with the last one selected.
        let mut state = state_with(
            vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
            1,
        );

        // When handling SelectNext.
        IntentHandler::handle(&Intent::SelectNext, &mut state);

        // Then the first thread is selected.
        assert_eq!(
            state.sessions.selected_id(),
            Some(ThreadId(2)),
            "SelectNext should wrap to the first thread"
        );
    }

    #[rstest::rstest]
    fn select_prev_on_first_thread_wraps_to_the_last() {
        // Given threads 2 and 1 in sidebar order, with the first one selected.
        let mut state = state_with(
            vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
            2,
        );

        // When handling SelectPrev.
        IntentHandler::handle(&Intent::SelectPrev, &mut state);

        // Then the last thread is selected.
        assert_eq!(
            state.sessions.selected_id(),
            Some(ThreadId(1)),
            "SelectPrev should wrap to the last thread"
        );
    }

    #[rstest::rstest]
    #[case(Intent::FocusRight, Focus::Sidebar, Focus::Dashboard)]
    #[case(Intent::FocusSidebar, Focus::Dashboard, Focus::Sidebar)]
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
    #[case(Focus::Dashboard)]
    #[case(Focus::Attached)]
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
    #[case(Focus::Dashboard)]
    fn hiding_the_sidebar_focuses_the_right_side(#[case] focus: Focus) {
        // Given a shown sidebar, with `focus` focused.
        let mut state = laid_out(focus, 32, false);

        // When handling ToggleSidebar.
        IntentHandler::handle(&Intent::ToggleSidebar, &mut state);

        // Then the right side has the keys.
        assert_eq!(
            state.focus,
            Focus::Dashboard,
            "hiding from {focus:?} should focus the right side"
        );
    }

    #[rstest::rstest]
    #[case(Focus::Dashboard)]
    #[case(Focus::Attached)]
    fn toggle_sidebar_shows_a_hidden_sidebar(#[case] focus: Focus) {
        // Given a hidden sidebar.
        let mut state = laid_out(focus, 32, true);

        // When handling ToggleSidebar.
        IntentHandler::handle(&Intent::ToggleSidebar, &mut state);

        // Then the sidebar is shown.
        assert!(!state.sidebar.hidden, "␣e should show the sidebar again");
    }

    #[rstest::rstest]
    fn showing_the_sidebar_focuses_it() {
        // Given a hidden sidebar, with the dashboard focused.
        let mut state = laid_out(Focus::Dashboard, 32, true);

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
    fn hiding_the_sidebar_from_the_pane_keeps_the_pane_focused() {
        // Given a shown sidebar, with the Claude pane focused.
        let mut state = laid_out(Focus::Attached, 32, false);

        // When handling ToggleSidebar (`<C-b>`).
        IntentHandler::handle(&Intent::ToggleSidebar, &mut state);

        // Then the pane keeps the keys.
        assert_eq!(
            state.focus,
            Focus::Attached,
            "<C-b> should hide the sidebar and keep the pane focused"
        );
    }

    #[rstest::rstest]
    fn showing_the_sidebar_from_the_pane_keeps_the_pane_focused() {
        // Given a hidden sidebar, with the Claude pane focused.
        let mut state = laid_out(Focus::Attached, 32, true);

        // When handling ToggleSidebar (`<C-b>`).
        IntentHandler::handle(&Intent::ToggleSidebar, &mut state);

        // Then the pane keeps the keys.
        assert_eq!(
            state.focus,
            Focus::Attached,
            "<C-b> should show the sidebar and keep the pane focused"
        );
    }

    #[rstest::rstest]
    fn focus_sidebar_while_hidden_keeps_the_dashboard_focused() {
        // Given a hidden sidebar, with the dashboard focused.
        let mut state = laid_out(Focus::Dashboard, 32, true);

        // When handling FocusSidebar (`<C-h>`).
        IntentHandler::handle(&Intent::FocusSidebar, &mut state);

        // Then the dashboard keeps the keys.
        assert_eq!(
            state.focus,
            Focus::Dashboard,
            "<C-h> should do nothing while the sidebar is hidden"
        );
    }

    #[rstest::rstest]
    #[case(Intent::WidenFocused, Focus::Sidebar, 36)]
    #[case(Intent::NarrowFocused, Focus::Sidebar, 28)]
    #[case(Intent::WidenFocused, Focus::Dashboard, 28)]
    #[case(Intent::NarrowFocused, Focus::Dashboard, 36)]
    #[case(Intent::WidenFocused, Focus::Attached, 28)]
    #[case(Intent::NarrowFocused, Focus::Attached, 36)]
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
    #[case(Intent::WidenFocused, Focus::Dashboard, 24)]
    #[case(Intent::WidenFocused, Focus::Sidebar, 80)]
    #[case(Intent::NarrowFocused, Focus::Dashboard, 80)]
    #[case(Intent::WidenFocused, Focus::Attached, 24)]
    #[case(Intent::NarrowFocused, Focus::Attached, 80)]
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
        // Given a hidden 32-column sidebar, with the dashboard focused.
        let mut state = laid_out(Focus::Dashboard, 32, true);

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
                Command::SaveJumps,
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

    /// Keys going to thread 1's attached session.
    fn attached() -> AppState {
        AppState {
            focus: Focus::Attached,
            attached: HashSet::from([ThreadId(1)]),
            ..state_with(vec![thread(1, ThreadStatus::Idle)], 1)
        }
    }

    /// Threads 1 and 2 in the sidebar with those in `attached` attached and
    /// thread `selected` selected.
    fn left_pane(attached: &[i64], selected: i64) -> AppState {
        AppState {
            focus: Focus::Sidebar,
            attached: attached.iter().copied().map(ThreadId).collect(),
            ..state_with(
                vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
                selected,
            )
        }
    }

    #[rstest::rstest]
    fn attach_adds_the_thread_to_attached() {
        // Given a selected idle thread.
        let mut state = state_with(vec![thread(1, ThreadStatus::Idle)], 1);

        // When handling Attach.
        IntentHandler::handle(&Intent::Attach, &mut state);

        // Then the thread is attached.
        assert!(
            state.attached.contains(&ThreadId(1)),
            "Attach should add the thread to attached"
        );
    }

    #[rstest::rstest]
    fn selecting_another_thread_keeps_it_attached() {
        // Given thread 1 attached and selected in the sidebar.
        let mut state = left_pane(&[1], 1);

        // When handling SelectNext onto another thread.
        IntentHandler::handle(&Intent::SelectNext, &mut state);

        // Then thread 1 is still attached.
        assert!(
            state.attached.contains(&ThreadId(1)),
            "selecting another thread should keep thread 1 attached"
        );
    }

    #[rstest::rstest]
    fn detach_removes_the_selected_thread_from_attached() {
        // Given keys going to thread 1's attached session.
        let mut state = attached();

        // When handling Detach.
        IntentHandler::handle(&Intent::Detach, &mut state);

        // Then thread 1 is no longer attached.
        assert!(
            !state.attached.contains(&ThreadId(1)),
            "Detach should remove the selected thread from attached"
        );
    }

    #[rstest::rstest]
    fn detach_sets_focus_dashboard() {
        // Given keys going to thread 1's attached session.
        let mut state = attached();

        // When handling Detach.
        IntentHandler::handle(&Intent::Detach, &mut state);

        // Then keys drive the dashboard.
        assert_eq!(
            state.focus,
            Focus::Dashboard,
            "Detach should return to the dashboard"
        );
    }

    #[rstest::rstest]
    fn detach_returns_detach_and_refresh() {
        // Given keys going to thread 1's attached session.
        let mut state = attached();

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
    fn detach_selected_removes_the_thread_from_attached() {
        // Given the sidebar on attached thread 1.
        let mut state = left_pane(&[1], 1);

        // When handling DetachSelected.
        IntentHandler::handle(&Intent::DetachSelected, &mut state);

        // Then thread 1 is no longer attached.
        assert!(
            !state.attached.contains(&ThreadId(1)),
            "DetachSelected should remove the selected thread from attached"
        );
    }

    #[rstest::rstest]
    fn detach_selected_keeps_focus_in_the_sidebar() {
        // Given the sidebar on attached thread 1.
        let mut state = left_pane(&[1], 1);

        // When handling DetachSelected.
        IntentHandler::handle(&Intent::DetachSelected, &mut state);

        // Then keys still drive the sidebar.
        assert_eq!(
            state.focus,
            Focus::Sidebar,
            "DetachSelected should keep focus in the sidebar"
        );
    }

    #[rstest::rstest]
    fn detach_selected_on_an_unattached_thread_returns_no_commands() {
        // Given the sidebar on thread 1, which isn't attached.
        let mut state = left_pane(&[], 1);

        // When handling DetachSelected.
        let commands = IntentHandler::handle(&Intent::DetachSelected, &mut state);

        // Then nothing happens.
        assert_eq!(
            commands,
            vec![],
            "DetachSelected on an unattached thread should return no commands"
        );
    }

    #[rstest::rstest]
    fn leave_pane_focuses_the_sidebar() {
        // Given keys going to an attached session.
        let mut state = attached();

        // When handling LeavePane.
        IntentHandler::handle(&Intent::LeavePane, &mut state);

        // Then keys drive the sidebar.
        assert_eq!(
            state.focus,
            Focus::Sidebar,
            "LeavePane should focus the sidebar"
        );
    }

    #[rstest::rstest]
    fn leave_pane_keeps_the_thread_attached() {
        // Given keys going to thread 1's attached session.
        let mut state = attached();

        // When handling LeavePane.
        IntentHandler::handle(&Intent::LeavePane, &mut state);

        // Then thread 1 is still attached.
        assert!(
            state.attached.contains(&ThreadId(1)),
            "LeavePane should keep the thread attached"
        );
    }

    #[rstest::rstest]
    fn leave_pane_returns_detach_and_refresh() {
        // Given keys going to an attached session.
        let mut state = attached();

        // When handling LeavePane.
        let commands = IntentHandler::handle(&Intent::LeavePane, &mut state);

        // Then the loop stops sending the pane input and the statuses are
        // refreshed.
        assert_eq!(
            commands,
            vec![Command::Detach, Command::RefreshSessions],
            "LeavePane should detach, then refresh"
        );
    }

    #[rstest::rstest]
    fn leave_pane_while_the_sidebar_is_hidden_does_nothing() {
        // Given keys going to an attached session, with the sidebar hidden.
        let mut state = AppState {
            sidebar: SidebarView {
                hidden: true,
                ..SidebarView::default()
            },
            ..attached()
        };

        // When handling LeavePane.
        let commands = IntentHandler::handle(&Intent::LeavePane, &mut state);

        // Then the session stays attached and nothing else happens.
        assert_eq!(
            (state.focus, commands),
            (Focus::Attached, vec![]),
            "LeavePane should do nothing while the sidebar is hidden"
        );
    }

    #[rstest::rstest]
    fn focus_right_on_an_attached_thread_attaches() {
        // Given the sidebar on attached thread 2.
        let mut state = left_pane(&[2], 2);

        // When handling FocusRight.
        IntentHandler::handle(&Intent::FocusRight, &mut state);

        // Then keys go into thread 2's pane.
        assert_eq!(
            state.focus,
            Focus::Attached,
            "FocusRight should go into the selected thread's pane"
        );
    }

    #[rstest::rstest]
    fn focus_right_on_an_attached_thread_returns_attach_and_refresh() {
        // Given the sidebar on attached thread 2.
        let mut state = left_pane(&[2], 2);

        // When handling FocusRight.
        let commands = IntentHandler::handle(&Intent::FocusRight, &mut state);

        // Then the loop attaches to thread 2 and the statuses are refreshed.
        assert_eq!(
            commands,
            vec![
                Command::Attach(AttachTarget {
                    thread: ThreadId(2),
                    argv: vec!["claude".into(), "attach".into(), "t2".into()],
                    cwd: "/work/2".into(),
                }),
                Command::RefreshSessions,
                Command::SaveJumps,
            ],
            "FocusRight should attach to the selected thread, then refresh"
        );
    }

    #[rstest::rstest]
    #[case(&[])]
    #[case(&[1])]
    fn focus_right_on_an_unattached_thread_focuses_the_dashboard(#[case] attached: &[i64]) {
        // Given thread 2 selected and unattached, with no thread or thread 1
        // attached.
        let mut state = left_pane(attached, 2);

        // When handling FocusRight.
        IntentHandler::handle(&Intent::FocusRight, &mut state);

        // Then keys drive the dashboard.
        assert_eq!(
            state.focus,
            Focus::Dashboard,
            "FocusRight with {attached:?} attached should focus the dashboard"
        );
    }

    #[rstest::rstest]
    fn hiding_the_sidebar_with_the_selected_thread_attached_focuses_the_pane() {
        // Given thread 1 attached and selected in the sidebar.
        let mut state = left_pane(&[1], 1);

        // When handling ToggleSidebar.
        IntentHandler::handle(&Intent::ToggleSidebar, &mut state);

        // Then keys go to the session, which takes the full width.
        assert_eq!(
            state.focus,
            Focus::Attached,
            "hiding the sidebar should focus the selected thread's pane"
        );
    }

    #[rstest::rstest]
    fn settle_removes_the_thread_from_attached() {
        // Given attached thread 1 selected in the sidebar, and Yes highlighted in its settle
        // confirm.
        let mut state = left_pane(&[1], 1);
        answer_yes(&Intent::ToggleSettle, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then thread 1 is no longer attached.
        assert!(
            !state.attached.contains(&ThreadId(1)),
            "settling should remove the thread from attached"
        );
    }

    #[rstest::rstest]
    fn delete_removes_the_thread_from_attached() {
        // Given attached thread 1 selected in the sidebar, and Yes highlighted in its delete
        // confirm.
        let mut state = left_pane(&[1], 1);
        answer_yes(&Intent::DeleteThread, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then thread 1 is no longer attached.
        assert!(
            !state.attached.contains(&ThreadId(1)),
            "deleting should remove the thread from attached"
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

        // Then the sessions actor is asked to create alpha's draft.
        assert_eq!(
            commands,
            vec![Command::CreateDraft(ProjectId(1)), Command::SaveJumps],
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

        // Then no draft is created.
        assert!(
            !commands.contains(&Command::CreateDraft(ProjectId(1))),
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
    fn picker_confirm_on_a_project_focuses_the_dashboard() {
        // Given the project picker opened from the sidebar.
        let mut state = picking(Focus::Sidebar);

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the picker is closed and the keys are on the dashboard, which
        // shows the draft.
        assert_eq!(
            (state.focus, state.picker.is_none()),
            (Focus::Dashboard, true),
            "picking a project should hand the keys to the dashboard"
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
        // Given the project picker opened from the dashboard.
        let mut state = picking(Focus::Dashboard);

        // When handling PickerCancel.
        IntentHandler::handle(&Intent::PickerCancel, &mut state);

        // Then the picker is closed and the keys are back on the dashboard.
        assert_eq!(
            (state.focus, state.picker.is_none()),
            (Focus::Dashboard, true),
            "PickerCancel should close the picker and return to the dashboard"
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
            focus: Focus::Dashboard,
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

    /// One project holding Feature group 9 with threads 2 and 1, settled if
    /// `settled`, and the cursor on `cursor`.
    fn grouped_state(settled: bool, cursor: SidebarItem) -> AppState {
        let mut state = state_at(
            [2, 1]
                .map(|id| Thread {
                    group: Some(GroupId(9)),
                    ..thread(id, ThreadStatus::Idle)
                })
                .into(),
            cursor,
        );
        let group = Group {
            id: GroupId(9),
            kind: GroupKind::Feature,
            name: "GT-514-login".into(),
            dir: Some("/work/GT-514-login".into()),
            branch: Some("GT-514-login".into()),
            created_at: SystemTime::UNIX_EPOCH,
            pinned_at: None,
            settled_at: settled.then(|| at(5)),
            active_since: SystemTime::UNIX_EPOCH,
            draft: false,
            defaults: GroupDefaults::default(),
        };
        if let Some(project) = state.sessions.projects.first_mut() {
            project.groups = vec![group];
        }
        state
    }

    #[rstest::rstest]
    fn switch_branch_on_a_feature_card_opens_its_worktrees_branch_picker() {
        // Given a started Feature group's card selected.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));

        // When handling SwitchBranch.
        IntentHandler::handle(&Intent::SwitchBranch, &mut state);

        // Then the group's branch picker on its worktree takes the keys.
        assert_eq!(
            (state.focus, state.picker.as_ref().map(PickerState::kind)),
            (
                Focus::Picker,
                Some(&PickerKind::Branches {
                    target: PickTarget::Group(GroupId(9)),
                    cwd: "/work/GT-514-login".into(),
                    unstarted: false,
                })
            ),
            "␣b on a Feature card should open its worktree's branches"
        );
    }

    #[rstest::rstest]
    fn switch_branch_on_a_feature_card_lists_its_worktrees_branches() {
        // Given a started Feature group's card selected.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));

        // When handling SwitchBranch.
        let commands = IntentHandler::handle(&Intent::SwitchBranch, &mut state);

        // Then the worktree's refs are listed.
        assert_eq!(
            commands,
            [Command::ListBranches("/work/GT-514-login".into())],
            "the picker's refs come from the group's worktree"
        );
    }

    #[rstest::rstest]
    fn picking_a_branch_for_a_group_returns_checkout_group() {
        // Given the group's branch picker with `main` highlighted.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));
        IntentHandler::handle(&Intent::SwitchBranch, &mut state);
        if let Some(picker) = &mut state.picker {
            picker.show_branches(
                Path::new("/work/GT-514-login"),
                vec![
                    branch("GT-514-login", true, Some("/work/GT-514-login")),
                    branch("main", false, None),
                ],
            );
        }
        IntentHandler::handle(&Intent::PickerNext, &mut state);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then main is checked out in the group's worktree.
        assert_eq!(
            commands,
            [Command::CheckoutGroup {
                group: GroupId(9),
                git_ref: branch("main", false, None),
            }],
            "a picked branch should be checked out for the whole group"
        );
    }

    /// One project holding Feature group 9, still a draft on `model`, with
    /// the group draft selected.
    fn group_drafting(model: Option<&str>) -> AppState {
        let mut state = state_at(vec![], SidebarItem::GroupDraft(GroupId(9)));
        let group = Group {
            id: GroupId(9),
            kind: GroupKind::Feature,
            name: "GT-514-login".into(),
            dir: None,
            branch: Some("GT-514-login".into()),
            created_at: SystemTime::UNIX_EPOCH,
            pinned_at: None,
            settled_at: None,
            active_since: SystemTime::UNIX_EPOCH,
            draft: true,
            defaults: GroupDefaults {
                model: model.map(str::to_owned),
                permission: None,
            },
        };
        if let Some(project) = state.sessions.projects.first_mut() {
            project.groups = vec![group];
        }
        state
    }

    /// `state` with thread `id` running `model`.
    fn with_model(mut state: AppState, id: i64, model: &str) -> AppState {
        if let Some(thread) = state
            .sessions
            .projects
            .iter_mut()
            .flat_map(|project| &mut project.threads)
            .find(|thread| thread.id == ThreadId(id))
        {
            thread.model = Some(model.to_owned());
        }
        state
    }

    /// `state` with group 9's defaults on `model` in `permission` mode.
    fn with_defaults(mut state: AppState, model: &str, permission: &str) -> AppState {
        if let Some(defaults) = state.sessions.group_defaults_mut(GroupId(9)) {
            defaults.model = Some(model.to_owned());
            defaults.permission = Some(permission.to_owned());
        }
        state
    }

    #[rstest::rstest]
    fn n_on_a_grouped_thread_starts_a_sibling_with_the_groups_defaults() {
        // Given thread 2 of group 9 selected, running haiku, and the group's
        // defaults on opus in auto mode.
        let state = with_model(
            grouped_state(false, SidebarItem::Thread(ThreadId(2))),
            2,
            "haiku",
        );
        let mut state = with_defaults(state, "opus", "auto");

        // When handling NewSibling.
        let commands = IntentHandler::handle(&Intent::NewSibling, &mut state);

        // Then the sessions actor is asked for a sibling on the defaults.
        assert_eq!(
            commands,
            vec![Command::StartSibling {
                group: GroupId(9),
                model: Some("opus".into()),
                permission_mode: Some("auto".into()),
                from: Some(SidebarItem::Thread(ThreadId(2))),
            }],
            "n on a grouped thread should start a sibling with the group's defaults"
        );
    }

    #[rstest::rstest]
    fn n_on_a_card_takes_the_groups_default_model() {
        // Given group 9's card selected, its newest thread on haiku and its
        // defaults on opus.
        let state = with_model(
            grouped_state(false, SidebarItem::Group(GroupId(9))),
            2,
            "haiku",
        );
        let mut state = with_defaults(state, "opus", "auto");

        // When handling NewSibling.
        let commands = IntentHandler::handle(&Intent::NewSibling, &mut state);

        // Then the sibling runs opus.
        let model = commands.iter().find_map(|command| match command {
            Command::StartSibling { model, .. } => model.clone(),
            _ => None,
        });
        assert_eq!(
            model.as_deref(),
            Some("opus"),
            "n on a card should take the group's default model"
        );
    }

    #[rstest::rstest]
    fn n_on_a_folded_group_opens_it() {
        // Given the cursor on group 9's card, folded.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));
        state.sessions.folded.insert(GroupId(9));

        // When handling NewSibling.
        IntentHandler::handle(&Intent::NewSibling, &mut state);

        // Then the group is no longer folded.
        assert!(
            !state.sessions.folded.contains(&GroupId(9)),
            "n on a folded card should open the group"
        );
    }

    #[rstest::rstest]
    fn n_marks_starting() {
        // Given the cursor on group 9's card.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));

        // When handling NewSibling.
        IntentHandler::handle(&Intent::NewSibling, &mut state);

        // Then a start is in flight.
        assert!(
            state.sessions.starting,
            "starting a sibling should mark the start in flight"
        );
    }

    #[rstest::rstest]
    fn n_while_starting_does_nothing() {
        // Given the cursor on group 9's card while a start is in flight.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));
        state.sessions.starting = true;

        // When handling NewSibling.
        let commands = IntentHandler::handle(&Intent::NewSibling, &mut state);

        // Then nothing is asked of the sessions actor.
        assert!(commands.is_empty(), "one start at a time");
    }

    #[rstest::rstest]
    #[case(SidebarItem::Group(GroupId(9)))]
    #[case(SidebarItem::Thread(ThreadId(1)))]
    fn n_on_a_settled_group_starts_nothing(#[case] cursor: SidebarItem) {
        // Given the cursor on settled group 9's card or thread.
        let mut state = grouped_state(true, cursor);

        // When handling NewSibling.
        let commands = IntentHandler::handle(&Intent::NewSibling, &mut state);

        // Then nothing is asked of the sessions actor.
        assert!(commands.is_empty(), "n works only on an active group");
    }

    #[rstest::rstest]
    fn n_on_a_settled_group_opens_nothing() {
        // Given the cursor on settled group 9's card, the shelf closed.
        let mut state = grouped_state(true, SidebarItem::Group(GroupId(9)));
        state.sessions.shelf_open = false;

        // When handling NewSibling.
        IntentHandler::handle(&Intent::NewSibling, &mut state);

        // Then neither the shelf nor the group opens.
        assert_eq!(
            (
                state.sessions.shelf_open,
                state.sessions.opened.contains(&GroupId(9))
            ),
            (false, false),
            "n on a settled group should open nothing"
        );
    }

    #[rstest::rstest]
    fn n_on_a_settled_group_shows_no_error() {
        // Given the cursor on settled group 9's card.
        let mut state = grouped_state(true, SidebarItem::Group(GroupId(9)));

        // When handling NewSibling.
        IntentHandler::handle(&Intent::NewSibling, &mut state);

        // Then the mode line stays quiet.
        assert_eq!(
            state.sessions.error, None,
            "n on a settled group is a silent no-op"
        );
    }

    #[rstest::rstest]
    fn n_on_a_draft_only_group_shows_the_start_hint() {
        // Given the cursor on the card of group 9, still a draft.
        let mut state = group_drafting(None);
        state.sessions.cursor = Some(SidebarItem::Group(GroupId(9)));

        // When handling NewSibling.
        IntentHandler::handle(&Intent::NewSibling, &mut state);

        // Then the mode line points at the draft.
        assert_eq!(
            state.sessions.error.as_deref(),
            Some(STARTS_FROM_DRAFT),
            "n on a draft-only group should point at its draft"
        );
    }

    #[rstest::rstest]
    fn n_on_a_draft_only_group_starts_nothing() {
        // Given the cursor on the card of group 9, still a draft.
        let mut state = group_drafting(None);
        state.sessions.cursor = Some(SidebarItem::Group(GroupId(9)));

        // When handling NewSibling.
        let commands = IntentHandler::handle(&Intent::NewSibling, &mut state);

        // Then no start happens.
        assert!(
            commands.is_empty() && !state.sessions.starting,
            "a draft-only group starts from its draft, not n"
        );
    }

    #[rstest::rstest]
    fn p_on_a_card_pins_the_group() {
        // Given the cursor on group 9's card, unpinned.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));

        // When handling TogglePin.
        let commands = IntentHandler::handle(&Intent::TogglePin, &mut state);

        // Then the sessions actor is asked to pin the group.
        assert_eq!(
            commands,
            vec![Command::PinGroup(GroupId(9))],
            "p on a card should pin its group"
        );
    }

    #[rstest::rstest]
    fn p_on_a_pinned_card_unpins_the_group() {
        // Given the cursor on group 9's card, pinned.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));
        if let Some(group) = state
            .sessions
            .projects
            .iter_mut()
            .flat_map(|project| &mut project.groups)
            .next()
        {
            group.pinned_at = Some(at(5));
        }

        // When handling TogglePin.
        let commands = IntentHandler::handle(&Intent::TogglePin, &mut state);

        // Then the sessions actor is asked to unpin the group.
        assert_eq!(
            commands,
            vec![Command::UnpinGroup(GroupId(9))],
            "p on a pinned card should unpin its group"
        );
    }

    #[rstest::rstest]
    fn p_on_a_grouped_thread_does_nothing() {
        // Given the cursor on thread 1 of group 9.
        let mut state = grouped_state(false, SidebarItem::Thread(ThreadId(1)));

        // When handling TogglePin.
        let commands = IntentHandler::handle(&Intent::TogglePin, &mut state);

        // Then nothing is asked of the sessions actor.
        assert!(
            commands.is_empty(),
            "a grouped thread is pinned with its group"
        );
    }

    /// `state` with thread `id` in `status`.
    fn with_status(mut state: AppState, id: i64, status: ThreadStatus) -> AppState {
        if let Some(thread) = state
            .sessions
            .projects
            .iter_mut()
            .flat_map(|project| &mut project.threads)
            .find(|thread| thread.id == ThreadId(id))
        {
            thread.status = status;
        }
        state
    }

    #[rstest::rstest]
    fn s_on_a_card_opens_the_settle_group_confirm() {
        // Given the cursor on group 9's card.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));

        // When handling ToggleSettle.
        IntentHandler::handle(&Intent::ToggleSettle, &mut state);

        // Then its settle confirm is open with No selected.
        assert_eq!(
            open_confirm(&state),
            Some((
                &PickerKind::SettleGroup { group: GroupId(9) },
                Some(&PickerItem::Confirm(false))
            )),
            "s on a card should ask to settle the group"
        );
    }

    #[rstest::rstest]
    fn confirming_the_settle_group_confirm_emits_settle_group() {
        // Given Yes highlighted in group 9's settle confirm.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));
        answer_yes(&Intent::ToggleSettle, &mut state);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the sessions actor is asked to settle the group.
        assert!(
            commands.contains(&Command::SettleGroup(GroupId(9))),
            "Yes on the settle group confirm should return SettleGroup"
        );
    }

    #[rstest::rstest]
    fn settling_a_group_detaches_its_threads() {
        // Given threads 1 and 2 of group 9 attached, and Yes highlighted in
        // its settle confirm.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));
        state.attached.extend([ThreadId(1), ThreadId(2)]);
        answer_yes(&Intent::ToggleSettle, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then neither thread is attached.
        assert!(
            state.attached.is_empty(),
            "settling a group should detach its threads"
        );
    }

    #[rstest::rstest]
    fn settling_a_group_selects_the_next_card() {
        // Given group 9 beside lone thread 3, and Yes highlighted in the
        // group's settle confirm.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));
        if let Some(project) = state.sessions.projects.first_mut() {
            project.threads.push(thread(3, ThreadStatus::Idle));
        }
        answer_yes(&Intent::ToggleSettle, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the lone thread's card is selected.
        assert_eq!(
            state.sessions.cursor,
            Some(SidebarItem::Thread(ThreadId(3))),
            "settling a group should select the neighbouring card"
        );
    }

    #[rstest::rstest]
    fn s_on_a_card_with_a_working_thread_shows_the_in_progress_error() {
        // Given the cursor on group 9's card while thread 2 is working.
        let mut state = with_status(
            grouped_state(false, SidebarItem::Group(GroupId(9))),
            2,
            ThreadStatus::Working,
        );

        // When handling ToggleSettle.
        IntentHandler::handle(&Intent::ToggleSettle, &mut state);

        // Then the mode line says why.
        assert_eq!(
            state.sessions.error.as_deref(),
            Some(SETTLE_IN_PROGRESS),
            "a group with a turn underway can't settle"
        );
    }

    #[rstest::rstest]
    fn s_on_a_settled_card_unsettles_the_group() {
        // Given the cursor on settled group 9's card.
        let mut state = grouped_state(true, SidebarItem::Group(GroupId(9)));

        // When handling ToggleSettle.
        let commands = IntentHandler::handle(&Intent::ToggleSettle, &mut state);

        // Then the sessions actor is asked to un-settle the group.
        assert_eq!(
            commands,
            vec![Command::UnsettleGroup(GroupId(9))],
            "s on a settled card should un-settle its group"
        );
    }

    #[rstest::rstest]
    fn s_on_a_grouped_thread_does_nothing() {
        // Given the cursor on thread 1 of group 9.
        let mut state = grouped_state(false, SidebarItem::Thread(ThreadId(1)));

        // When handling ToggleSettle.
        let commands = IntentHandler::handle(&Intent::ToggleSettle, &mut state);

        // Then nothing is asked and no confirm opens.
        assert_eq!(
            (commands, state.picker.is_some()),
            (vec![], false),
            "a grouped thread is settled with its group"
        );
    }

    #[rstest::rstest]
    fn d_on_a_groups_only_thread_shows_the_last_in_group_error() {
        // Given group 9 holding only thread 1, which is selected.
        let mut state = grouped_state(false, SidebarItem::Thread(ThreadId(1)));
        if let Some(project) = state.sessions.projects.first_mut() {
            project.threads.retain(|thread| thread.id == ThreadId(1));
        }

        // When handling DeleteThread.
        IntentHandler::handle(&Intent::DeleteThread, &mut state);

        // Then the mode line says why.
        assert_eq!(
            state.sessions.error.as_deref(),
            Some(LAST_IN_GROUP),
            "a group keeps its last thread"
        );
    }

    #[rstest::rstest]
    fn d_on_a_group_draft_shows_the_last_in_group_error() {
        // Given the group draft selected.
        let mut state = group_drafting(None);

        // When handling DeleteThread.
        IntentHandler::handle(&Intent::DeleteThread, &mut state);

        // Then the mode line says why.
        assert_eq!(
            state.sessions.error.as_deref(),
            Some(LAST_IN_GROUP),
            "a group keeps its draft"
        );
    }

    #[rstest::rstest]
    fn d_on_a_grouped_thread_with_a_sibling_opens_the_delete_confirm() {
        // Given thread 2 of group 9 selected, beside thread 1.
        let mut state = grouped_state(false, SidebarItem::Thread(ThreadId(2)));

        // When handling DeleteThread.
        IntentHandler::handle(&Intent::DeleteThread, &mut state);

        // Then its delete confirm is open.
        assert_eq!(
            state.picker.as_ref().map(PickerState::kind),
            Some(&PickerKind::DeleteThread {
                thread: ThreadId(2)
            }),
            "d on a grouped thread with a sibling should ask to delete it"
        );
    }

    #[rstest::rstest]
    fn d_on_a_card_opens_the_delete_group_confirm() {
        // Given the cursor on group 9's card.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));

        // When handling DeleteThread.
        IntentHandler::handle(&Intent::DeleteThread, &mut state);

        // Then the group's delete confirm is open.
        assert_eq!(
            state.picker.as_ref().map(PickerState::kind),
            Some(&PickerKind::DeleteGroup {
                group: GroupId(9),
                dir: Some(GroupKind::Feature)
            }),
            "d on a card should ask to delete the group"
        );
    }

    #[rstest::rstest]
    fn d_on_a_never_started_feature_card_confirms_without_a_worktree() {
        // Given the cursor on the card of Feature group 9, still a draft.
        let mut state = group_drafting(None);
        state.sessions.cursor = Some(SidebarItem::Group(GroupId(9)));

        // When handling DeleteThread.
        IntentHandler::handle(&Intent::DeleteThread, &mut state);

        // Then the group's delete confirm names no directory.
        assert_eq!(
            state.picker.as_ref().map(PickerState::kind),
            Some(&PickerKind::DeleteGroup {
                group: GroupId(9),
                dir: None
            }),
            "a never-started Feature group has no worktree to delete"
        );
    }

    #[rstest::rstest]
    fn confirming_the_delete_group_confirm_hides_nothing_yet() {
        // Given Yes highlighted in group 9's delete confirm.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));
        answer_yes(&Intent::DeleteThread, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then no thread is hidden and the cursor stays on the card: the
        // sessions actor may still refuse the delete.
        assert_eq!(
            (state.sessions.deleting.is_empty(), state.sessions.cursor),
            (true, Some(SidebarItem::Group(GroupId(9)))),
            "a group delete should show nothing gone before the actor agrees"
        );
    }

    #[rstest::rstest]
    fn confirming_the_delete_group_confirm_emits_delete_group() {
        // Given Yes highlighted in group 9's delete confirm.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));
        answer_yes(&Intent::DeleteThread, &mut state);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the sessions actor is asked to delete the group.
        assert!(
            commands.contains(&Command::DeleteGroup(GroupId(9))),
            "Yes on the delete group confirm should return DeleteGroup"
        );
    }

    #[rstest::rstest]
    fn enter_on_a_group_draft_starts_it() {
        // Given the group draft selected.
        let mut state = group_drafting(None);

        // When handling Attach.
        let commands = IntentHandler::handle(&Intent::Attach, &mut state);

        // Then the sessions actor is asked to start it.
        assert_eq!(
            commands,
            vec![Command::StartGroupDraft(GroupId(9))],
            "⏎ on a group draft should start it"
        );
    }

    #[rstest::rstest]
    fn enter_on_a_group_draft_marks_starting() {
        // Given the group draft selected.
        let mut state = group_drafting(None);

        // When handling Attach.
        IntentHandler::handle(&Intent::Attach, &mut state);

        // Then a start is in flight.
        assert!(
            state.sessions.starting,
            "starting a group draft should mark the start in flight"
        );
    }

    #[rstest::rstest]
    fn enter_on_a_group_draft_while_starting_does_nothing() {
        // Given the group draft selected while a start is in flight.
        let mut state = group_drafting(None);
        state.sessions.starting = true;

        // When handling Attach.
        let commands = IntentHandler::handle(&Intent::Attach, &mut state);

        // Then nothing starts.
        assert!(commands.is_empty(), "one start at a time");
    }

    #[rstest::rstest]
    fn leader_m_on_a_group_draft_opens_its_model_picker() {
        // Given the group draft selected.
        let mut state = group_drafting(None);

        // When handling PickModel.
        IntentHandler::handle(&Intent::PickModel, &mut state);

        // Then the group draft's model picker is open.
        assert_eq!(
            state.picker.as_ref().map(PickerState::kind),
            Some(&PickerKind::Model {
                target: DraftTarget::Group(GroupId(9))
            }),
            "␣m on a group draft should open its model picker"
        );
    }

    #[rstest::rstest]
    #[case::model(Intent::PickModel, PickerKind::Model { target: DraftTarget::Group(GroupId(9)) })]
    #[case::permission(
        Intent::PickPermission,
        PickerKind::Permission { target: DraftTarget::Group(GroupId(9)) }
    )]
    fn leader_m_and_a_on_a_card_open_the_groups_setting_picker(
        #[case] intent: Intent,
        #[case] kind: PickerKind,
    ) {
        // Given a started group's card selected.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));

        // When handling the pick.
        IntentHandler::handle(&intent, &mut state);

        // Then the group's picker is open.
        assert_eq!(
            state.picker.as_ref().map(PickerState::kind),
            Some(&kind),
            "␣m/␣a on a card should pick the group's default"
        );
    }

    #[rstest::rstest]
    fn picking_a_model_on_a_card_sets_the_groups_default() {
        // Given a started group's card and its model picker with Claude Opus
        // 5.5, after Default, highlighted.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));
        IntentHandler::handle(&Intent::PickModel, &mut state);
        IntentHandler::handle(&Intent::PickerNext, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the group's default model is claude-opus-5-5.
        assert_eq!(
            group_draft_model(&state).as_deref(),
            Some("claude-opus-5-5"),
            "the picked model should be the group's default"
        );
    }

    #[rstest::rstest]
    fn picking_a_model_on_a_card_saves_it() {
        // Given a started group's card and its model picker with Claude Opus
        // 5.5 highlighted.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));
        IntentHandler::handle(&Intent::PickModel, &mut state);
        IntentHandler::handle(&Intent::PickerNext, &mut state);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the sessions actor is asked to save the group's defaults.
        assert_eq!(
            commands,
            vec![Command::SaveGroupDraft(GroupId(9))],
            "a card's model pick should be saved"
        );
    }

    #[rstest::rstest]
    fn picking_a_model_on_a_card_leaves_its_threads_alone() {
        // Given a started group's card, thread 2 running haiku, and the
        // model picker with Claude Opus 5.5 highlighted.
        let mut state = with_model(
            grouped_state(false, SidebarItem::Group(GroupId(9))),
            2,
            "haiku",
        );
        IntentHandler::handle(&Intent::PickModel, &mut state);
        IntentHandler::handle(&Intent::PickerNext, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then thread 2 still runs haiku.
        let model = state
            .sessions
            .threads()
            .find(|thread| thread.id == ThreadId(2))
            .and_then(|thread| thread.model.clone());
        assert_eq!(
            model.as_deref(),
            Some("haiku"),
            "a running thread keeps its model"
        );
    }

    #[rstest::rstest]
    fn leader_m_on_a_card_shows_the_groups_default_as_current() {
        // Given a started group's card whose default is Claude Opus 5.5.
        let mut state = with_defaults(
            grouped_state(false, SidebarItem::Group(GroupId(9))),
            "claude-opus-5-5",
            "auto",
        );

        // When handling PickModel.
        IntentHandler::handle(&Intent::PickModel, &mut state);

        // Then the picker highlights the group's default.
        let highlighted = state
            .picker
            .as_ref()
            .and_then(PickerState::selected)
            .cloned();
        assert_eq!(
            highlighted,
            Some(PickerItem::Setting(Some("claude-opus-5-5"))),
            "the card's picker should start on the group's default"
        );
    }

    /// Group 9's default model, as the app state has it.
    fn group_draft_model(state: &AppState) -> Option<String> {
        let (_, group) = state.sessions.selected_group()?;
        group.defaults.model.clone()
    }

    #[rstest::rstest]
    fn picking_a_model_for_a_group_draft_sets_it() {
        // Given the group draft's model picker with Claude Opus 5.5, after
        // Default, highlighted.
        let mut state = group_drafting(None);
        IntentHandler::handle(&Intent::PickModel, &mut state);
        IntentHandler::handle(&Intent::PickerNext, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the group draft runs claude-opus-5-5.
        assert_eq!(
            group_draft_model(&state).as_deref(),
            Some("claude-opus-5-5"),
            "the picked model should be the group draft's"
        );
    }

    #[rstest::rstest]
    fn picking_a_model_for_a_group_draft_saves_it() {
        // Given the group draft's model picker with Claude Opus 5.5 highlighted.
        let mut state = group_drafting(None);
        IntentHandler::handle(&Intent::PickModel, &mut state);
        IntentHandler::handle(&Intent::PickerNext, &mut state);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the sessions actor is asked to save the group draft.
        assert_eq!(
            commands,
            vec![Command::SaveGroupDraft(GroupId(9))],
            "a group draft's model pick should be saved"
        );
    }

    #[rstest::rstest]
    fn l_on_a_folded_group_opens_it() {
        // Given the cursor on group 9's card, folded.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));
        state.sessions.folded.insert(GroupId(9));

        // When handling OpenGroup.
        IntentHandler::handle(&Intent::OpenGroup, &mut state);

        // Then the group is no longer folded.
        assert!(
            !state.sessions.folded.contains(&GroupId(9)),
            "l on a folded card should open the group"
        );
    }

    #[rstest::rstest]
    fn l_on_a_closed_settled_group_opens_it() {
        // Given the cursor on settled group 9, closed.
        let mut state = grouped_state(true, SidebarItem::Group(GroupId(9)));

        // When handling OpenGroup.
        IntentHandler::handle(&Intent::OpenGroup, &mut state);

        // Then the group is opened.
        assert!(
            state.sessions.opened.contains(&GroupId(9)),
            "l on a settled group should open it"
        );
    }

    #[rstest::rstest]
    fn l_on_a_settled_group_opens_the_shelf() {
        // Given the cursor on settled group 9 with the shelf closed.
        let mut state = grouped_state(true, SidebarItem::Group(GroupId(9)));

        // When handling OpenGroup.
        IntentHandler::handle(&Intent::OpenGroup, &mut state);

        // Then the Settled shelf is open.
        assert!(
            state.sessions.shelf_open,
            "l on a settled group should open the shelf with it"
        );
    }

    #[rstest::rstest]
    fn h_on_a_grouped_thread_closes_the_group_onto_its_card() {
        // Given the cursor on thread 1 in open group 9.
        let mut state = grouped_state(false, SidebarItem::Thread(ThreadId(1)));

        // When handling CloseGroup.
        IntentHandler::handle(&Intent::CloseGroup, &mut state);

        // Then the group is folded with its card selected.
        assert_eq!(
            (
                state.sessions.folded.contains(&GroupId(9)),
                state.sessions.cursor
            ),
            (true, Some(SidebarItem::Group(GroupId(9)))),
            "h on a child should fold the group onto its card"
        );
    }

    #[rstest::rstest]
    fn h_on_an_open_settled_group_closes_it_onto_its_card() {
        // Given the shelf open, settled group 9 opened and the cursor on its thread 1.
        let mut state = grouped_state(true, SidebarItem::Thread(ThreadId(1)));
        state.sessions.shelf_open = true;
        state.sessions.opened.insert(GroupId(9));

        // When handling CloseGroup.
        IntentHandler::handle(&Intent::CloseGroup, &mut state);

        // Then the group is closed with its row selected.
        assert_eq!(
            (
                state.sessions.opened.contains(&GroupId(9)),
                state.sessions.cursor
            ),
            (false, Some(SidebarItem::Group(GroupId(9)))),
            "h in an open settled group should close it onto its row"
        );
    }

    #[rstest::rstest]
    fn h_on_a_closed_settled_group_closes_the_shelf() {
        // Given the shelf open and the cursor on settled group 9, closed.
        let mut state = grouped_state(true, SidebarItem::Group(GroupId(9)));
        state.sessions.shelf_open = true;

        // When handling CloseGroup.
        IntentHandler::handle(&Intent::CloseGroup, &mut state);

        // Then the shelf is closed with its header selected.
        assert_eq!(
            (state.sessions.shelf_open, state.sessions.cursor),
            (false, Some(SidebarItem::SettledShelf)),
            "h on a closed settled group should close the shelf onto its header"
        );
    }

    #[rstest::rstest]
    fn enter_on_an_open_card_folds_the_group() {
        // Given the cursor on open group 9's card.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));

        // When handling Attach.
        IntentHandler::handle(&Intent::Attach, &mut state);

        // Then the group is folded.
        assert!(
            state.sessions.folded.contains(&GroupId(9)),
            "⏎ on an open card should fold the group"
        );
    }

    #[rstest::rstest]
    fn enter_on_a_folded_card_opens_the_group() {
        // Given the cursor on folded group 9's card.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));
        state.sessions.folded.insert(GroupId(9));

        // When handling Attach.
        IntentHandler::handle(&Intent::Attach, &mut state);

        // Then the group is open.
        assert!(
            !state.sessions.folded.contains(&GroupId(9)),
            "⏎ on a folded card should open the group"
        );
    }

    #[rstest::rstest]
    fn enter_on_a_card_returns_no_commands() {
        // Given the cursor on group 9's card.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));

        // When handling Attach.
        let commands = IntentHandler::handle(&Intent::Attach, &mut state);

        // Then nothing attaches.
        assert!(commands.is_empty(), "⏎ on a card shouldn't attach");
    }

    #[rstest::rstest]
    fn settle_returns_the_settle_command() {
        // Given threads 2 and 1 in sidebar order, with thread 2 selected, and Yes highlighted in
        // its settle confirm.
        let mut state = state_with(
            vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
            2,
        );
        answer_yes(&Intent::ToggleSettle, &mut state);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the sessions actor is asked to settle thread 2.
        assert!(
            commands.contains(&Command::Settle(ThreadId(2))),
            "Yes on the settle confirm should return Settle"
        );
    }

    #[rstest::rstest]
    fn settle_selects_the_next_card_below() {
        // Given threads 3, 2 and 1 in sidebar order, with thread 2 selected, and Yes highlighted in
        // its settle confirm.
        let mut state = state_with(
            vec![
                thread(1, ThreadStatus::Idle),
                thread(2, ThreadStatus::Idle),
                thread(3, ThreadStatus::Idle),
            ],
            2,
        );
        answer_yes(&Intent::ToggleSettle, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the card below is selected.
        assert_eq!(
            state.sessions.cursor,
            Some(SidebarItem::Thread(ThreadId(1))),
            "settle should select the next card below"
        );
    }

    #[rstest::rstest]
    fn settling_the_last_card_selects_the_card_above() {
        // Given threads 2 and 1 in sidebar order, with thread 1 selected, and Yes highlighted in
        // its settle confirm.
        let mut state = state_with(
            vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
            1,
        );
        answer_yes(&Intent::ToggleSettle, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the card above is selected.
        assert_eq!(
            state.sessions.cursor,
            Some(SidebarItem::Thread(ThreadId(2))),
            "settling the last card should select the one above"
        );
    }

    #[rstest::rstest]
    fn settling_the_only_card_selects_the_shelf() {
        // Given one thread, selected, and Yes highlighted in its settle confirm.
        let mut state = state_with(vec![thread(1, ThreadStatus::Idle)], 1);
        answer_yes(&Intent::ToggleSettle, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

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
        // Given one thread, selected, and Yes highlighted in its delete confirm.
        let mut state = state_with(vec![thread(1, ThreadStatus::Idle)], 1);
        answer_yes(&Intent::DeleteThread, &mut state);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the sessions actor is asked to delete it.
        assert!(
            commands.contains(&Command::Delete(ThreadId(1))),
            "Yes on the delete confirm should return Delete"
        );
    }

    #[rstest::rstest]
    fn delete_selects_the_next_row_below() {
        // Given cards 2 and 1, then the open shelf holding thread 3, with card 1 selected, and Yes
        // highlighted in its delete confirm.
        let mut state = state_with(
            vec![
                thread(1, ThreadStatus::Idle),
                thread(2, ThreadStatus::Idle),
                settled(3),
            ],
            1,
        );
        state.sessions.shelf_open = true;
        answer_yes(&Intent::DeleteThread, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

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

        // Then the thread it lands on is visited.
        assert!(
            commands.contains(&Command::Visit(ThreadId(expected))),
            "{intent:?} should visit thread {expected}"
        );
    }

    #[rstest::rstest]
    fn delete_hides_the_thread_from_the_sidebar() {
        // Given threads 2 and 1, with thread 1 selected, and Yes highlighted in its delete confirm.
        let mut state = state_with(
            vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
            1,
        );
        answer_yes(&Intent::DeleteThread, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

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
            focus: Focus::Dashboard,
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
                    target: DraftTarget::Project(ProjectId(1))
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
        // Given a selected draft, and Yes highlighted in its discard confirm.
        let mut state = drafting(draft(DraftWorkspace::Local), vec![]);
        answer_yes(&Intent::DeleteThread, &mut state);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the sessions actor is asked to discard it.
        assert!(
            commands.contains(&Command::DiscardDraft(ProjectId(1))),
            "Yes on a draft's discard confirm should discard it"
        );
    }

    #[rstest::rstest]
    fn delete_on_a_draft_selects_the_next_row() {
        // Given a selected draft above thread 1's card, and Yes highlighted in its discard confirm.
        let mut state = drafting(
            draft(DraftWorkspace::Local),
            vec![thread(1, ThreadStatus::Idle)],
        );
        answer_yes(&Intent::DeleteThread, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the card below is selected.
        assert_eq!(
            state.sessions.cursor,
            Some(SidebarItem::Thread(ThreadId(1))),
            "discarding a draft should select the row below"
        );
    }

    /// Gives every project `threads`, as a poll would.
    fn poll(state: &mut AppState, threads: &[Thread]) {
        for project in &mut state.sessions.projects {
            project.threads = threads.to_vec();
        }
    }

    /// The open picker's kind and selected item.
    fn open_confirm(state: &AppState) -> Option<(&PickerKind, Option<&PickerItem>)> {
        state
            .picker
            .as_ref()
            .map(|picker| (picker.kind(), picker.selected()))
    }

    #[rstest::rstest]
    fn settle_opens_the_settle_confirm_with_no_selected() {
        // Given a selected idle thread.
        let mut state = state_with(vec![thread(1, ThreadStatus::Idle)], 1);

        // When handling ToggleSettle.
        IntentHandler::handle(&Intent::ToggleSettle, &mut state);

        // Then its settle confirm is open with No selected.
        assert_eq!(
            open_confirm(&state),
            Some((
                &PickerKind::SettleThread {
                    thread: ThreadId(1)
                },
                Some(&PickerItem::Confirm(false))
            )),
            "s should ask to settle the thread"
        );
    }

    #[rstest::rstest]
    fn settle_on_a_settled_thread_opens_no_confirm() {
        // Given a selected settled thread.
        let mut state = state_with(vec![settled(1)], 1);

        // When handling ToggleSettle.
        IntentHandler::handle(&Intent::ToggleSettle, &mut state);

        // Then no confirm opens.
        assert!(state.picker.is_none(), "un-settling needs no confirm");
    }

    #[rstest::rstest]
    fn settle_on_a_working_thread_shows_the_refusal() {
        // Given a selected thread Claude is working in.
        let mut state = state_with(vec![thread(1, ThreadStatus::Working)], 1);

        // When handling ToggleSettle.
        IntentHandler::handle(&Intent::ToggleSettle, &mut state);

        // Then the mode line says it can't settle.
        assert_eq!(
            state.sessions.error.as_deref(),
            Some(SETTLE_IN_PROGRESS),
            "s on a working thread should say why it can't settle"
        );
    }

    #[rstest::rstest]
    fn settle_on_a_working_thread_opens_no_confirm() {
        // Given a selected thread Claude is working in.
        let mut state = state_with(vec![thread(1, ThreadStatus::Working)], 1);

        // When handling ToggleSettle.
        IntentHandler::handle(&Intent::ToggleSettle, &mut state);

        // Then no confirm opens.
        assert!(
            state.picker.is_none(),
            "a thread that can't be settled should not ask to settle"
        );
    }

    #[rstest::rstest]
    fn yes_on_the_settle_confirm_after_a_turn_started_does_not_settle() {
        // Given Yes highlighted in thread 1's settle confirm, and then Claude
        // starts a turn in it.
        let mut state = state_with(vec![thread(1, ThreadStatus::Idle)], 1);
        answer_yes(&Intent::ToggleSettle, &mut state);
        poll(&mut state, &[thread(1, ThreadStatus::Working)]);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then nothing is asked of the sessions actor.
        assert!(
            commands.is_empty(),
            "a thread that started working should not be settled"
        );
    }

    #[rstest::rstest]
    fn yes_on_the_settle_confirm_after_a_turn_started_shows_the_refusal() {
        // Given Yes highlighted in thread 1's settle confirm, and then Claude
        // starts a turn in it.
        let mut state = state_with(vec![thread(1, ThreadStatus::Idle)], 1);
        answer_yes(&Intent::ToggleSettle, &mut state);
        poll(&mut state, &[thread(1, ThreadStatus::Working)]);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the mode line says it can't settle.
        assert_eq!(
            state.sessions.error.as_deref(),
            Some(SETTLE_IN_PROGRESS),
            "Yes on a thread that started working should say why it can't settle"
        );
    }

    #[rstest::rstest]
    fn yes_on_the_settle_confirm_after_it_was_settled_returns_no_commands() {
        // Given Yes highlighted in thread 1's settle confirm, and then thread 1
        // is settled.
        let mut state = state_with(vec![thread(1, ThreadStatus::Idle)], 1);
        answer_yes(&Intent::ToggleSettle, &mut state);
        poll(&mut state, &[settled(1)]);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then it is not toggled back.
        assert!(
            commands.is_empty(),
            "Yes on an already settled thread should not un-settle it"
        );
    }

    #[rstest::rstest]
    fn delete_opens_the_delete_confirm_with_no_selected() {
        // Given a selected thread.
        let mut state = state_with(vec![thread(1, ThreadStatus::Idle)], 1);

        // When handling DeleteThread.
        IntentHandler::handle(&Intent::DeleteThread, &mut state);

        // Then its delete confirm is open with No selected.
        assert_eq!(
            open_confirm(&state),
            Some((
                &PickerKind::DeleteThread {
                    thread: ThreadId(1)
                },
                Some(&PickerItem::Confirm(false))
            )),
            "d should ask to delete the thread"
        );
    }

    #[rstest::rstest]
    fn delete_on_a_draft_opens_the_discard_confirm_with_no_selected() {
        // Given a selected draft.
        let mut state = drafting(draft(DraftWorkspace::Local), vec![]);

        // When handling DeleteThread.
        IntentHandler::handle(&Intent::DeleteThread, &mut state);

        // Then its discard confirm is open with No selected.
        assert_eq!(
            open_confirm(&state),
            Some((
                &PickerKind::DiscardDraft {
                    project: ProjectId(1)
                },
                Some(&PickerItem::Confirm(false))
            )),
            "d on a draft should ask to discard it"
        );
    }

    #[rstest::rstest]
    #[case(Intent::ToggleSettle, state_with(vec![thread(1, ThreadStatus::Idle)], 1))]
    #[case(Intent::DeleteThread, state_with(vec![thread(1, ThreadStatus::Idle)], 1))]
    #[case(Intent::DeleteThread, drafting(draft(DraftWorkspace::Local), vec![]))]
    fn no_on_a_sidebar_confirm_returns_no_commands(
        #[case] intent: Intent,
        #[case] mut state: AppState,
    ) {
        // Given the confirm `intent` opened, with No selected.
        IntentHandler::handle(&intent, &mut state);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then nothing is asked of the sessions actor.
        assert!(
            commands.is_empty(),
            "No on {intent:?}'s confirm should do nothing"
        );
    }

    #[rstest::rstest]
    fn yes_on_the_delete_confirm_after_the_thread_left_returns_no_commands() {
        // Given Yes highlighted in thread 1's delete confirm, and then thread 1
        // leaves the sidebar.
        let mut state = state_with(vec![thread(1, ThreadStatus::Idle)], 1);
        answer_yes(&Intent::DeleteThread, &mut state);
        poll(&mut state, &[]);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then nothing is asked of the sessions actor.
        assert!(
            commands.is_empty(),
            "Yes on a thread that is gone should do nothing"
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

    #[rstest::rstest]
    fn open_tool_on_a_group_card_uses_the_group_dir() {
        // Given a group card selected, its group in `/work/GT-514-login`.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));

        // When handling OpenTool.
        let commands = IntentHandler::handle(&Intent::OpenTool(Tool::Lazygit), &mut state);

        // Then lazygit opens in the group's directory.
        assert_eq!(
            commands,
            vec![Command::OpenTool {
                tool: Tool::Lazygit,
                cwd: "/work/GT-514-login".into(),
            }],
            "a card's tools open in its group's directory"
        );
    }

    #[rstest::rstest]
    fn open_tool_on_an_unstarted_feature_group_uses_the_project_root() {
        // Given a Feature group draft with no directory yet, in `/work`.
        let mut state = group_drafting(None);

        // When handling OpenTool.
        let commands = IntentHandler::handle(&Intent::OpenTool(Tool::Lazygit), &mut state);

        // Then lazygit opens in the project's root.
        assert_eq!(
            commands,
            vec![Command::OpenTool {
                tool: Tool::Lazygit,
                cwd: "/work".into(),
            }],
            "an unstarted Feature group has only its project's root"
        );
    }

    #[rstest::rstest]
    fn open_tool_on_a_grouped_thread_uses_its_cwd() {
        // Given grouped thread 1 selected, running in `/work/1`.
        let mut state = grouped_state(false, SidebarItem::Thread(ThreadId(1)));

        // When handling OpenTool.
        let commands = IntentHandler::handle(&Intent::OpenTool(Tool::Shell), &mut state);

        // Then the shell opens where the thread runs.
        assert_eq!(
            commands,
            vec![Command::OpenTool {
                tool: Tool::Shell,
                cwd: "/work/1".into(),
            }],
            "a grouped thread's tools open in its own directory"
        );
    }

    /// The picker row for project `id` of [`with_projects`].
    fn project_row(id: i64, title: &str) -> PickerItem {
        PickerItem::Project {
            id: ProjectId(id),
            title: title.to_owned(),
            root: format!("/{title}").into(),
            kind: ProjectKind::Normal,
        }
    }

    /// Projects alpha (1) and beta (2), holding idle threads 11 and 21 (21
    /// listed first), the sidebar filtered to `filter` with the cursor on
    /// `cursor`, and the project filter opened from the sidebar.
    fn filtering(filter: Option<i64>, cursor: SidebarItem) -> AppState {
        let mut state = with_projects(&["alpha", "beta"]);
        for (project, id) in state.sessions.projects.iter_mut().zip([11, 21]) {
            project.threads = vec![thread(id, ThreadStatus::Idle)];
        }
        state.sessions.filter = filter.map(ProjectId);
        state.sessions.cursor = Some(cursor);
        IntentHandler::handle(&Intent::FilterProjects, &mut state);
        state
    }

    /// Highlights `item` in the open picker.
    fn highlight(state: &mut AppState, item: &PickerItem) {
        if let Some(picker) = &mut state.picker {
            picker.select(item);
        }
    }

    /// Handles `intent` to open its `No`/`Yes` confirm, then highlights `Yes`.
    fn answer_yes(intent: &Intent, state: &mut AppState) {
        IntentHandler::handle(intent, state);
        highlight(state, &PickerItem::Confirm(true));
    }

    /// The trust confirm for `/work` is open, opened from the sidebar.
    fn trusting() -> AppState {
        AppState {
            picker: Some(PickerState::trust_workspace("/work".into(), Focus::Sidebar)),
            focus: Focus::Picker,
            ..AppState::default()
        }
    }

    #[rstest::rstest]
    fn yes_on_the_trust_confirm_returns_trust_workspace() {
        // Given the trust confirm with Yes highlighted.
        let mut state = trusting();
        highlight(&mut state, &PickerItem::Confirm(true));

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the folder is trusted.
        assert_eq!(
            commands,
            vec![Command::TrustWorkspace],
            "Yes should trust the folder"
        );
    }

    #[rstest::rstest]
    fn no_on_the_trust_confirm_returns_decline_trust() {
        // Given the trust confirm with No highlighted.
        let mut state = trusting();

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the waiting start is declined.
        assert_eq!(
            commands,
            vec![Command::DeclineTrust],
            "No should decline the trust"
        );
    }

    #[rstest::rstest]
    fn cancelling_the_trust_confirm_returns_decline_trust() {
        // Given the trust confirm.
        let mut state = trusting();

        // When cancelling it.
        let commands = IntentHandler::handle(&Intent::PickerCancel, &mut state);

        // Then the waiting start is declined.
        assert_eq!(
            commands,
            vec![Command::DeclineTrust],
            "Esc should decline the trust"
        );
    }

    #[rstest::rstest]
    fn confirming_the_trust_confirm_with_both_rows_filtered_away_returns_decline_trust() {
        // Given the trust confirm with Yes highlighted, then `zz` typed so its
        // filter hides both No and Yes.
        let mut state = trusting();
        highlight(&mut state, &PickerItem::Confirm(true));
        IntentHandler::handle(&Intent::PickerInput('z'), &mut state);
        IntentHandler::handle(&Intent::PickerInput('z'), &mut state);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the waiting start is declined.
        assert_eq!(
            commands,
            vec![Command::DeclineTrust],
            "confirming with nothing shown should decline the trust"
        );
    }

    /// [`filtering`], then `<C-x>` on alpha.
    fn removing_alpha(filter: Option<i64>, cursor: SidebarItem) -> AppState {
        let mut state = filtering(filter, cursor);
        highlight(&mut state, &project_row(1, "alpha"));
        IntentHandler::handle(&Intent::PickerRemove, &mut state);
        state
    }

    fn on_thread(id: i64) -> SidebarItem {
        SidebarItem::Thread(ThreadId(id))
    }

    #[rstest::rstest]
    fn filter_projects_lists_all_projects_then_the_projects() {
        // Given projects alpha and beta.
        // When handling FilterProjects.
        let state = filtering(None, on_thread(21));

        // Then the picker lists All projects, then alpha and beta.
        let rows: Vec<PickerItem> = state
            .picker
            .iter()
            .flat_map(PickerState::shown)
            .map(|(item, _)| item.clone())
            .collect();
        assert_eq!(
            rows,
            vec![
                PickerItem::AllProjects,
                project_row(1, "alpha"),
                project_row(2, "beta"),
            ],
            "the project filter's rows"
        );
    }

    /// Projects alpha (1) and orb's Incognito project (2), which has a draft
    /// when `with_draft`; nothing selected.
    fn incognito(with_draft: bool) -> AppState {
        let mut state = with_projects(&["alpha", "incognito"]);
        for project in state.sessions.projects.iter_mut().skip(1) {
            project.kind = ProjectKind::Incognito;
            project.draft = with_draft.then(|| draft(DraftWorkspace::Local));
        }
        state
    }

    #[rstest::rstest]
    fn new_incognito_without_a_draft_creates_it_without_starting_it() {
        // Given the Incognito project without a draft.
        let mut state = incognito(false);

        // When handling NewIncognito.
        let commands = IntentHandler::handle(&Intent::NewIncognito, &mut state);

        // Then the draft is created, and not started.
        assert_eq!(
            commands,
            vec![Command::CreateDraft(ProjectId(2)), Command::SaveJumps],
            "a missing incognito draft should be created, not started"
        );
    }

    #[rstest::rstest]
    fn new_incognito_with_a_draft_neither_creates_nor_starts_one() {
        // Given the Incognito project with a draft.
        let mut state = incognito(true);

        // When handling NewIncognito.
        let commands = IntentHandler::handle(&Intent::NewIncognito, &mut state);

        // Then only the jump is saved: no CreateDraft, no StartDraft.
        assert_eq!(
            commands,
            vec![Command::SaveJumps],
            "an existing incognito draft should just be opened"
        );
    }

    #[rstest::rstest]
    fn new_incognito_selects_the_incognito_draft() {
        // Given the Incognito project and nothing selected.
        let mut state = incognito(false);

        // When handling NewIncognito.
        IntentHandler::handle(&Intent::NewIncognito, &mut state);

        // Then the cursor is on the Incognito draft.
        assert_eq!(
            state.sessions.cursor,
            Some(SidebarItem::Draft(ProjectId(2))),
            "the incognito draft should be selected"
        );
    }

    #[rstest::rstest]
    fn new_incognito_focuses_the_dashboard() {
        // Given the Incognito project and the sidebar focused.
        let mut state = incognito(true);
        state.focus = Focus::Sidebar;

        // When handling NewIncognito.
        IntentHandler::handle(&Intent::NewIncognito, &mut state);

        // Then the dashboard has the focus, ready for ⏎ to start the draft.
        assert_eq!(
            state.focus,
            Focus::Dashboard,
            "the incognito draft should open on the dashboard"
        );
    }

    #[rstest::rstest]
    fn new_incognito_doesnt_mark_a_start_in_flight() {
        // Given the Incognito project and no start in flight.
        let mut state = incognito(true);

        // When handling NewIncognito.
        IntentHandler::handle(&Intent::NewIncognito, &mut state);

        // Then no start is in flight.
        assert!(
            !state.sessions.starting,
            "opening the incognito draft shouldn't mark a start in flight"
        );
    }

    #[rstest::rstest]
    fn new_incognito_clears_a_filter_to_another_project() {
        // Given the sidebar filtered to alpha.
        let mut state = incognito(true);
        state.sessions.filter = Some(ProjectId(1));

        // When handling NewIncognito.
        IntentHandler::handle(&Intent::NewIncognito, &mut state);

        // Then the filter is cleared.
        assert_eq!(
            state.sessions.filter, None,
            "a filter hiding the incognito draft should be cleared"
        );
    }

    #[rstest::rstest]
    fn new_incognito_while_starting_still_selects_the_incognito_draft() {
        // Given the Incognito project while a start is in flight.
        let mut state = incognito(true);
        state.sessions.starting = true;

        // When handling NewIncognito.
        IntentHandler::handle(&Intent::NewIncognito, &mut state);

        // Then the cursor is on the Incognito draft.
        assert_eq!(
            state.sessions.cursor,
            Some(SidebarItem::Draft(ProjectId(2))),
            "a start in flight shouldn't stop the incognito draft opening"
        );
    }

    #[rstest::rstest]
    fn filter_projects_lists_orbs_projects_after_all_projects() {
        // Given Learn, alpha, Incognito, Research and beta, in that order.
        let mut state = with_projects(&["learn", "alpha", "incognito", "research", "beta"]);
        for (project, kind) in state.sessions.projects.iter_mut().zip([
            ProjectKind::Learn,
            ProjectKind::Normal,
            ProjectKind::Incognito,
            ProjectKind::Research,
        ]) {
            project.kind = kind;
        }

        // When handling FilterProjects.
        IntentHandler::handle(&Intent::FilterProjects, &mut state);

        // Then Research, Learn and Incognito follow All projects, once each,
        // before the projects.
        let rows: Vec<PickerItem> = state
            .picker
            .iter()
            .flat_map(PickerState::shown)
            .map(|(item, _)| item.clone())
            .collect();
        let own = |id, title: &str, kind| PickerItem::Project {
            id: ProjectId(id),
            title: title.to_owned(),
            root: format!("/{title}").into(),
            kind,
        };
        assert_eq!(
            rows,
            vec![
                PickerItem::AllProjects,
                own(4, "research", ProjectKind::Research),
                own(1, "learn", ProjectKind::Learn),
                own(3, "incognito", ProjectKind::Incognito),
                project_row(2, "alpha"),
                project_row(5, "beta"),
            ],
            "Research, Learn and Incognito should come right after All projects"
        );
    }

    #[rstest::rstest]
    fn filter_projects_selects_the_current_filter() {
        // Given the sidebar filtered to beta.
        // When handling FilterProjects.
        let state = filtering(Some(2), on_thread(21));

        // Then beta is highlighted.
        assert_eq!(
            state.picker.as_ref().and_then(PickerState::selected),
            Some(&project_row(2, "beta")),
            "the current filter should be preselected"
        );
    }

    #[rstest::rstest]
    fn filter_projects_selects_all_projects_without_a_filter() {
        // Given no filter.
        // When handling FilterProjects.
        let state = filtering(None, on_thread(21));

        // Then All projects is highlighted.
        assert_eq!(
            state.picker.as_ref().and_then(PickerState::selected),
            Some(&PickerItem::AllProjects),
            "no filter should preselect All projects"
        );
    }

    #[rstest::rstest]
    fn confirming_a_project_filter_sets_it() {
        // Given the project filter with beta highlighted.
        let mut state = filtering(None, on_thread(21));
        highlight(&mut state, &project_row(2, "beta"));

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the sidebar is filtered to beta.
        assert_eq!(
            state.sessions.filter,
            Some(ProjectId(2)),
            "picking beta should filter to it"
        );
    }

    #[rstest::rstest]
    fn confirming_a_project_filter_moves_an_outside_cursor_to_the_first_row() {
        // Given the cursor on alpha's thread 11 and beta highlighted.
        let mut state = filtering(None, on_thread(11));
        highlight(&mut state, &project_row(2, "beta"));

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the cursor is on beta's thread 21, the first row left.
        assert_eq!(
            state.sessions.cursor,
            Some(on_thread(21)),
            "a cursor the filter hides should move to the first row"
        );
    }

    #[rstest::rstest]
    fn confirming_a_project_filter_returns_save_ui() {
        // Given the cursor on beta's thread 21 and beta highlighted.
        let mut state = filtering(None, on_thread(21));
        highlight(&mut state, &project_row(2, "beta"));

        // When handling PickerConfirm.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the filter is saved and thread 21 visited.
        assert_eq!(
            commands,
            vec![Command::SaveUi, Command::Visit(ThreadId(21))],
            "filtering should save the filter"
        );
    }

    #[rstest::rstest]
    fn confirming_all_projects_clears_the_filter() {
        // Given the sidebar filtered to beta and All projects highlighted.
        let mut state = filtering(Some(2), on_thread(21));
        highlight(&mut state, &PickerItem::AllProjects);

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the sidebar lists every project again.
        assert_eq!(
            state.sessions.filter, None,
            "All projects should clear the filter"
        );
    }

    #[rstest::rstest]
    fn picker_remove_on_a_project_opens_the_remove_confirm() {
        // Given the project filter with alpha highlighted.
        // When handling PickerRemove.
        let state = removing_alpha(None, on_thread(21));

        // Then the remove confirm for alpha is open.
        assert_eq!(
            state.picker.as_ref().map(PickerState::kind),
            Some(&PickerKind::RemoveProject {
                project: ProjectId(1)
            }),
            "<C-x> on a project should ask to confirm its removal"
        );
    }

    #[rstest::rstest]
    fn picker_remove_on_all_projects_does_nothing() {
        // Given the project filter with All projects highlighted.
        let mut state = filtering(None, on_thread(21));

        // When handling PickerRemove.
        IntentHandler::handle(&Intent::PickerRemove, &mut state);

        // Then the project filter stays open.
        assert_eq!(
            state.picker.as_ref().map(PickerState::kind),
            Some(&PickerKind::ProjectFilter),
            "All projects can't be removed"
        );
    }

    #[rstest::rstest]
    fn confirming_yes_returns_remove_project() {
        // Given the remove confirm for alpha with Yes highlighted.
        let mut state = removing_alpha(None, on_thread(21));
        highlight(&mut state, &PickerItem::Confirm(true));

        // When handling PickerConfirm.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then alpha is removed and the filter saved.
        assert_eq!(
            commands,
            vec![
                Command::RemoveProject(ProjectId(1)),
                Command::SaveUi,
                Command::Visit(ThreadId(21)),
            ],
            "Yes should remove the project"
        );
    }

    #[rstest::rstest]
    fn confirming_yes_clears_a_filter_to_the_project() {
        // Given the sidebar filtered to alpha, and the remove confirm for
        // alpha with Yes highlighted.
        let mut state = removing_alpha(Some(1), on_thread(11));
        highlight(&mut state, &PickerItem::Confirm(true));

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the sidebar lists every project again.
        assert_eq!(
            state.sessions.filter, None,
            "removing the filtered project should clear the filter"
        );
    }

    #[rstest::rstest]
    fn confirming_yes_moves_the_cursor_off_the_projects_draft() {
        // Given alpha's draft selected (above threads 21 and 11), and the
        // remove confirm for alpha with Yes highlighted.
        let mut state = {
            let mut state = filtering(None, SidebarItem::Draft(ProjectId(1)));
            if let Some(alpha) = state.sessions.projects.first_mut() {
                alpha.draft = Some(draft(DraftWorkspace::Local));
            }
            highlight(&mut state, &project_row(1, "alpha"));
            IntentHandler::handle(&Intent::PickerRemove, &mut state);
            state
        };
        highlight(&mut state, &PickerItem::Confirm(true));

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the cursor is on thread 21, the row below the draft.
        assert_eq!(
            state.sessions.cursor,
            Some(on_thread(21)),
            "the cursor should leave the discarded draft"
        );
    }

    #[rstest::rstest]
    fn confirming_no_returns_nothing() {
        // Given the remove confirm for alpha with No highlighted.
        let mut state = removing_alpha(None, on_thread(21));

        // When handling PickerConfirm.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then nothing is removed.
        assert!(commands.is_empty(), "No should remove nothing");
    }

    #[rstest::rstest]
    fn cancelling_the_remove_confirm_returns_to_the_sidebar() {
        // Given the remove confirm opened from the sidebar's project filter.
        let mut state = removing_alpha(None, on_thread(21));

        // When handling PickerCancel.
        IntentHandler::handle(&Intent::PickerCancel, &mut state);

        // Then the picker is closed and the keys are back in the sidebar.
        assert_eq!(
            (state.picker.is_none(), state.focus),
            (true, Focus::Sidebar),
            "Esc should close the confirm back to the sidebar"
        );
    }

    #[rstest::rstest]
    fn new_session_outside_the_filter_clears_it() {
        // Given the sidebar filtered to beta and the project picker with alpha
        // highlighted.
        let mut state = picking(Focus::Sidebar);
        state.sessions.filter = Some(ProjectId(2));

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the sidebar lists every project again.
        assert_eq!(
            state.sessions.filter, None,
            "opening another project's draft should clear the filter"
        );
    }

    #[rstest::rstest]
    fn new_session_outside_the_filter_returns_save_ui() {
        // Given the sidebar filtered to beta and the project picker with alpha
        // highlighted.
        let mut state = picking(Focus::Sidebar);
        state.sessions.filter = Some(ProjectId(2));

        // When handling PickerConfirm.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then alpha's draft is created and the cleared filter saved.
        assert_eq!(
            commands,
            vec![
                Command::CreateDraft(ProjectId(1)),
                Command::SaveUi,
                Command::SaveJumps
            ],
            "clearing the filter should save it"
        );
    }

    #[rstest::rstest]
    fn new_session_inside_the_filter_keeps_it() {
        // Given the sidebar filtered to alpha and the project picker with
        // alpha highlighted.
        let mut state = picking(Focus::Sidebar);
        state.sessions.filter = Some(ProjectId(1));

        // When handling PickerConfirm.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the filter stays, and isn't saved.
        assert_eq!(
            (state.sessions.filter, commands),
            (
                Some(ProjectId(1)),
                vec![Command::CreateDraft(ProjectId(1)), Command::SaveJumps]
            ),
            "a project inside the filter leaves it alone"
        );
    }

    /// Thread 1, titled `title`, selected in the sidebar.
    fn titled(title: Option<&str>) -> AppState {
        state_with(
            vec![Thread {
                title: title.map(str::to_owned),
                ..thread(1, ThreadStatus::Idle)
            }],
            1,
        )
    }

    /// Thread 1 selected, with the rename box open on it holding `text`.
    fn renaming(text: &str) -> AppState {
        AppState {
            focus: Focus::Rename,
            rename: Some(Rename {
                target: RenameTarget::Thread(ThreadId(1)),
                input: TextInput::new(text),
                creating: false,
            }),
            ..titled(Some("Fix the sidebar"))
        }
    }

    /// The text in the open rename box, if it's open.
    fn rename_text(state: &AppState) -> Option<&str> {
        state.rename.as_ref().map(|rename| rename.input.text())
    }

    #[rstest::rstest]
    fn rename_opens_the_box_filled_with_the_threads_title() {
        // Given a selected thread titled "Fix the sidebar".
        let mut state = titled(Some("Fix the sidebar"));

        // When handling Rename.
        IntentHandler::handle(&Intent::Rename, &mut state);

        // Then the rename box holds its title.
        assert_eq!(
            rename_text(&state),
            Some("Fix the sidebar"),
            "the rename box should start from the thread's title"
        );
    }

    #[rstest::rstest]
    fn rename_on_an_untitled_thread_opens_an_empty_box() {
        // Given a selected thread with no title yet.
        let mut state = titled(None);

        // When handling Rename.
        IntentHandler::handle(&Intent::Rename, &mut state);

        // Then the rename box is empty, not "New thread".
        assert_eq!(
            rename_text(&state),
            Some(""),
            "an untitled thread's rename box should start empty"
        );
    }

    #[rstest::rstest]
    fn rename_moves_the_keys_to_the_rename_box() {
        // Given a selected thread with the sidebar focused.
        let mut state = titled(Some("Fix the sidebar"));

        // When handling Rename.
        IntentHandler::handle(&Intent::Rename, &mut state);

        // Then the keys go to the rename box.
        assert_eq!(state.focus, Focus::Rename, "Rename should focus the box");
    }

    #[rstest::rstest]
    fn rename_on_a_draft_opens_nothing() {
        // Given the cursor on a draft.
        let mut state = state_at(vec![], SidebarItem::Draft(ProjectId(1)));

        // When handling Rename.
        IntentHandler::handle(&Intent::Rename, &mut state);

        // Then no rename box opens.
        assert!(state.rename.is_none(), "a draft can't be renamed");
    }

    #[rstest::rstest]
    fn typing_in_the_rename_box_edits_the_name() {
        // Given the rename box holding "Fix".
        let mut state = renaming("Fix");

        // When typing `!`.
        IntentHandler::handle(&Intent::PickerInput('!'), &mut state);

        // Then the name is "Fix!".
        assert_eq!(
            rename_text(&state),
            Some("Fix!"),
            "typing should edit the rename box"
        );
    }

    #[rstest::rstest]
    fn rename_confirm_returns_rename_thread_with_the_trimmed_text() {
        // Given the rename box holding " Sidebar search ".
        let mut state = renaming(" Sidebar search ");

        // When handling PickerConfirm.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the thread is renamed to the trimmed text.
        assert_eq!(
            commands,
            vec![Command::RenameThread {
                thread: ThreadId(1),
                title: Some("Sidebar search".to_owned()),
            }],
            "⏎ should rename the thread"
        );
    }

    #[rstest::rstest]
    #[case::empty("")]
    #[case::whitespace("   ")]
    fn blank_rename_confirm_returns_rename_thread_with_no_title(#[case] text: &str) {
        // Given the rename box holding nothing but whitespace.
        let mut state = renaming(text);

        // When handling PickerConfirm.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then orb's name is cleared.
        assert_eq!(
            commands,
            vec![Command::RenameThread {
                thread: ThreadId(1),
                title: None,
            }],
            "a blank ⏎ should go back to Claude's title"
        );
    }

    #[rstest::rstest]
    fn rename_confirm_gives_the_sidebar_back_the_keys() {
        // Given the rename box open.
        let mut state = renaming("Sidebar search");

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the box is closed and the sidebar has the keys.
        assert_eq!(
            (state.focus, state.rename.is_none()),
            (Focus::Sidebar, true),
            "⏎ should close the rename box"
        );
    }

    #[rstest::rstest]
    fn rename_cancel_returns_no_command() {
        // Given the rename box open.
        let mut state = renaming("Sidebar search");

        // When handling PickerCancel.
        let commands = IntentHandler::handle(&Intent::PickerCancel, &mut state);

        // Then nothing is renamed.
        assert!(commands.is_empty(), "Esc shouldn't rename: {commands:?}");
    }

    #[rstest::rstest]
    fn rename_cancel_gives_the_sidebar_back_the_keys() {
        // Given the rename box open.
        let mut state = renaming("Sidebar search");

        // When handling PickerCancel.
        IntentHandler::handle(&Intent::PickerCancel, &mut state);

        // Then the box is closed and the sidebar has the keys.
        assert_eq!(
            (state.focus, state.rename.is_none()),
            (Focus::Sidebar, true),
            "Esc should close the rename box"
        );
    }

    /// "fix login bug" (1), "add dark mode" (2) and "fix logout" (3), listed
    /// 3, 2, 1, with the cursor on thread 2.
    fn three_titles() -> AppState {
        let threads = [
            (1, "fix login bug"),
            (2, "add dark mode"),
            (3, "fix logout"),
        ]
        .map(|(id, title)| Thread {
            title: Some(title.to_owned()),
            ..thread(id, ThreadStatus::Idle)
        })
        .into();
        state_with(threads, 2)
    }

    /// `three_titles`, searching for `text` from thread 2, with the cursor
    /// on `cursor`.
    fn searching(text: &str, cursor: Option<i64>) -> AppState {
        let mut state = three_titles();
        state.focus = Focus::Search;
        state.sessions.search = Some(Search {
            input: TextInput::new(text),
            return_to: Some(SidebarItem::Thread(ThreadId(2))),
        });
        state.sessions.cursor = cursor.map(|id| SidebarItem::Thread(ThreadId(id)));
        state
    }

    #[rstest::rstest]
    fn search_moves_the_keys_to_the_input_box() {
        // Given the sidebar focused on thread 2.
        let mut state = three_titles();

        // When handling Search.
        IntentHandler::handle(&Intent::Search, &mut state);

        // Then the keys go to an empty search.
        assert_eq!(
            (
                state.focus,
                state.sessions.search.as_ref().map(|s| s.input.text())
            ),
            (Focus::Search, Some("")),
            "Search should focus an empty search"
        );
    }

    #[rstest::rstest]
    fn typing_in_the_search_selects_the_first_match() {
        // Given an empty search begun on thread 2.
        let mut state = searching("", Some(2));

        // When typing `l`, which "fix logout" (3) and "fix login bug" (1)
        // match.
        IntentHandler::handle(&Intent::PickerInput('l'), &mut state);

        // Then the cursor is on the first match.
        assert_eq!(
            state.sessions.cursor,
            Some(SidebarItem::Thread(ThreadId(3))),
            "typing should select the first match"
        );
    }

    #[rstest::rstest]
    fn typing_in_the_search_visits_the_first_match() {
        // Given an empty search begun on thread 2.
        let mut state = searching("", Some(2));

        // When typing `l`.
        let commands = IntentHandler::handle(&Intent::PickerInput('l'), &mut state);

        // Then the match is visited.
        assert_eq!(
            commands,
            vec![Command::Visit(ThreadId(3))],
            "typing should visit the first match"
        );
    }

    #[rstest::rstest]
    fn search_next_moves_to_the_next_match() {
        // Given "fix" matching threads 3 and 1, with the cursor on 3.
        let mut state = searching("fix", Some(3));

        // When handling PickerNext (`<C-j>`).
        IntentHandler::handle(&Intent::PickerNext, &mut state);

        // Then the cursor is on thread 1, past the unmatched thread 2.
        assert_eq!(
            state.sessions.cursor,
            Some(SidebarItem::Thread(ThreadId(1))),
            "<C-j> should move to the next match"
        );
    }

    #[rstest::rstest]
    fn search_prev_moves_to_the_previous_match() {
        // Given "fix" matching threads 3 and 1, with the cursor on 1.
        let mut state = searching("fix", Some(1));

        // When handling PickerPrev (`<C-k>`).
        IntentHandler::handle(&Intent::PickerPrev, &mut state);

        // Then the cursor is on thread 3, past the unmatched thread 2.
        assert_eq!(
            state.sessions.cursor,
            Some(SidebarItem::Thread(ThreadId(3))),
            "<C-k> should move to the previous match"
        );
    }

    #[rstest::rstest]
    fn search_confirm_clears_the_search_and_keeps_the_selection() {
        // Given "logout" matching thread 3, with the cursor on it.
        let mut state = searching("logout", Some(3));

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the search is gone, the sidebar has the keys and the cursor
        // stays on the match.
        assert_eq!(
            (
                state.sessions.search.is_none(),
                state.focus,
                state.sessions.cursor
            ),
            (true, Focus::Sidebar, Some(SidebarItem::Thread(ThreadId(3)))),
            "⏎ should end the search on the match"
        );
    }

    #[rstest::rstest]
    fn search_confirm_without_a_match_restores_the_old_selection() {
        // Given a search begun on thread 2 that matches nothing.
        let mut state = searching("zzz", None);

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then it acts as Esc: the cursor is back on thread 2.
        assert_eq!(
            (state.sessions.search.is_none(), state.sessions.cursor),
            (true, Some(SidebarItem::Thread(ThreadId(2)))),
            "⏎ with no match should act as Esc"
        );
    }

    #[rstest::rstest]
    fn search_cancel_clears_the_search_and_restores_the_old_selection() {
        // Given a search begun on thread 2 that moved the cursor to thread 3.
        let mut state = searching("logout", Some(3));

        // When handling PickerCancel.
        IntentHandler::handle(&Intent::PickerCancel, &mut state);

        // Then the search is gone, the sidebar has the keys and the cursor is
        // back on thread 2.
        assert_eq!(
            (
                state.sessions.search.is_none(),
                state.focus,
                state.sessions.cursor
            ),
            (true, Focus::Sidebar, Some(SidebarItem::Thread(ThreadId(2)))),
            "Esc should end the search where it began"
        );
    }

    /// The dashboard cursor's index on `state`'s selection.
    fn dashboard_index(state: &AppState) -> usize {
        let len = crate::feat::dashboard::items(&state.sessions).len();
        state.dashboard.index(&state.sessions, len)
    }

    #[rstest::rstest]
    fn dashboard_next_on_the_last_item_wraps_to_the_first() {
        // Given a thread's dashboard with the cursor on its last item.
        let mut state = AppState {
            focus: Focus::Dashboard,
            ..state_with(vec![in_root(1)], 1)
        };
        IntentHandler::handle(&Intent::DashboardPrev, &mut state);

        // When handling DashboardNext.
        IntentHandler::handle(&Intent::DashboardNext, &mut state);

        // Then the first item is highlighted.
        assert_eq!(dashboard_index(&state), 0, "next on the last item wraps");
    }

    #[rstest::rstest]
    fn dashboard_prev_on_the_first_item_wraps_to_the_last() {
        // Given a thread's dashboard with the cursor on its first item.
        let mut state = AppState {
            focus: Focus::Dashboard,
            ..state_with(vec![in_root(1)], 1)
        };

        // When handling DashboardPrev.
        IntentHandler::handle(&Intent::DashboardPrev, &mut state);

        // Then the last of the thread's eleven items is highlighted.
        assert_eq!(
            dashboard_index(&state),
            10,
            "previous on the first item wraps"
        );
    }

    #[rstest::rstest]
    fn dashboard_run_on_model_opens_the_drafts_model_picker() {
        // Given a git draft's dashboard with the cursor on Model, its fourth
        // item.
        let mut state = drafting(draft(DraftWorkspace::Local), vec![]);
        for _ in 0..3 {
            IntentHandler::handle(&Intent::DashboardNext, &mut state);
        }

        // When handling DashboardRun.
        IntentHandler::handle(&Intent::DashboardRun, &mut state);

        // Then the draft's model picker is open, as PickModel opens it.
        assert_eq!(
            state.picker.as_ref().map(PickerState::kind),
            Some(&PickerKind::Model {
                target: DraftTarget::Project(ProjectId(1))
            }),
            "⏎ on Model should open the model picker"
        );
    }

    #[rstest::rstest]
    fn dashboard_run_with_nothing_selected_opens_the_project_picker() {
        // Given the dashboard with no thread or draft selected.
        let mut state = AppState {
            focus: Focus::Dashboard,
            ..with_projects(&["alpha"])
        };

        // When handling DashboardRun on its first item.
        IntentHandler::handle(&Intent::DashboardRun, &mut state);

        // Then New session's project picker is open.
        assert_eq!(
            state.picker.as_ref().map(PickerState::kind),
            Some(&PickerKind::Projects),
            "⏎ with nothing selected should run New session"
        );
    }

    #[rstest::rstest]
    fn dashboard_cursor_goes_back_to_the_first_item_on_a_new_selection() {
        // Given thread 2's dashboard with the cursor moved down.
        let mut state = AppState {
            focus: Focus::Dashboard,
            ..state_with(vec![in_root(1), in_root(2)], 2)
        };
        IntentHandler::handle(&Intent::DashboardNext, &mut state);

        // When selecting the next thread.
        IntentHandler::handle(&Intent::SelectNext, &mut state);

        // Then the first item is highlighted.
        assert_eq!(
            dashboard_index(&state),
            0,
            "a new selection should start on Open"
        );
    }

    #[rstest::rstest]
    fn leader_gf_opens_the_group_project_picker() {
        // Given a project.
        let mut state = with_projects(&["alpha"]);

        // When handling NewGroup(Feature).
        IntentHandler::handle(&Intent::NewGroup(GroupKind::Feature), &mut state);

        // Then the group project picker has the keys.
        assert_eq!(
            (state.focus, state.picker.as_ref().map(PickerState::kind)),
            (Focus::Picker, Some(&PickerKind::GroupProject)),
            "␣gf should open the group project picker"
        );
    }

    #[rstest::rstest]
    fn group_project_picker_lists_only_normal_projects() {
        // Given alpha and orb's Research project.
        let mut state = with_projects(&["alpha", "research"]);
        if let Some(project) = state.sessions.projects.get_mut(1) {
            project.kind = ProjectKind::Research;
        }

        // When handling NewGroup(Feature).
        IntentHandler::handle(&Intent::NewGroup(GroupKind::Feature), &mut state);

        // Then only alpha is listed.
        let rows: Vec<PickerItem> = state
            .picker
            .iter()
            .flat_map(PickerState::shown)
            .map(|(item, _)| item.clone())
            .collect();
        assert_eq!(
            rows,
            vec![project_row(1, "alpha")],
            "a Feature group can't be in Research"
        );
    }

    #[rstest::rstest]
    fn picking_a_group_project_opens_the_feature_name_box() {
        // Given the group project picker on alpha.
        let mut state = with_projects(&["alpha"]);
        IntentHandler::handle(&Intent::NewGroup(GroupKind::Feature), &mut state);

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the name box names a Feature group in alpha, with the keys.
        assert_eq!(
            (state.rename.map(|rename| rename.target), state.focus),
            (
                Some(RenameTarget::NewGroup {
                    kind: GroupKind::Feature,
                    project: Some(ProjectId(1)),
                }),
                Focus::Rename
            ),
            "picking a project should open the name box"
        );
    }

    #[rstest::rstest]
    fn leader_gr_opens_the_research_name_box() {
        // Given no projects.
        let mut state = AppState::default();

        // When handling NewGroup(Research).
        IntentHandler::handle(&Intent::NewGroup(GroupKind::Research), &mut state);

        // Then an empty name box names a Research group, with the keys.
        assert_eq!(
            (
                state.rename.as_ref().map(|rename| rename.target),
                rename_text(&state),
                state.focus
            ),
            (
                Some(RenameTarget::NewGroup {
                    kind: GroupKind::Research,
                    project: None,
                }),
                Some(""),
                Focus::Rename
            ),
            "␣gr should open the name box"
        );
    }

    /// orb's Research project holding group `tokio-cancel`, with the name box
    /// for a new Research group holding `text`.
    fn naming_research(text: &str) -> AppState {
        let mut state = with_projects(&["Research"]);
        if let Some(project) = state.sessions.projects.first_mut() {
            project.kind = ProjectKind::Research;
            project.groups = vec![Group {
                id: GroupId(9),
                kind: GroupKind::Research,
                name: "tokio-cancel".into(),
                dir: Some("/Research/tokio-cancel".into()),
                branch: None,
                created_at: SystemTime::UNIX_EPOCH,
                pinned_at: None,
                settled_at: None,
                active_since: SystemTime::UNIX_EPOCH,
                draft: false,
                defaults: GroupDefaults::default(),
            }];
        }
        AppState {
            focus: Focus::Rename,
            rename: Some(Rename {
                target: RenameTarget::NewGroup {
                    kind: GroupKind::Research,
                    project: None,
                },
                input: TextInput::new(text),
                creating: false,
            }),
            ..state
        }
    }

    #[rstest::rstest]
    fn confirming_a_group_name_keeps_the_box_open_until_the_group_is_made() {
        // Given the name box holding a fresh name.
        let mut state = naming_research("tokio select");

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the box keeps its name and the keys, waiting for the actor.
        assert_eq!(
            (
                state.rename.as_ref().map(|rename| rename.creating),
                rename_text(&state),
                state.focus
            ),
            (Some(true), Some("tokio select"), Focus::Rename),
            "a valid name should wait for the sessions actor"
        );
    }

    #[rstest::rstest]
    fn confirming_a_group_name_again_while_it_is_made_asks_for_nothing() {
        // Given the name box already confirmed with a fresh name.
        let mut state = naming_research("tokio select");
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // When handling PickerConfirm again.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then no second group is asked for.
        assert_eq!(commands, vec![], "a second ⏎ should send nothing");
    }

    #[rstest::rstest]
    fn confirming_a_taken_group_name_shows_the_error() {
        // Given the name box holding the taken name.
        let mut state = naming_research("tokio-cancel");

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the mode line says a group has the name.
        assert_eq!(
            state.sessions.error.as_deref(),
            Some("Group tokio-cancel already exists"),
            "a taken name should show why"
        );
    }

    #[rstest::rstest]
    fn confirming_a_taken_group_name_keeps_the_box_open() {
        // Given the name box holding the taken name.
        let mut state = naming_research("tokio cancel");

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the box keeps the name and the keys.
        assert_eq!(
            (rename_text(&state), state.focus),
            (Some("tokio cancel"), Focus::Rename),
            "a taken name should keep the box open"
        );
    }

    #[rstest::rstest]
    fn confirming_an_invalid_group_name_shows_the_char() {
        // Given the name box holding `a/b`.
        let mut state = naming_research("a/b");

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the mode line names the slash.
        assert_eq!(
            state.sessions.error.as_deref(),
            Some("Name can't use /"),
            "an invalid name should show the char"
        );
    }

    #[rstest::rstest]
    fn confirming_an_empty_group_name_does_nothing() {
        // Given the name box holding only spaces.
        let mut state = naming_research("   ");

        // When handling PickerConfirm.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then nothing is emitted and the box stays open with the keys.
        assert_eq!(
            (commands, rename_text(&state), state.focus),
            (vec![], Some("   "), Focus::Rename),
            "an empty name should do nothing"
        );
    }

    /// Projects alpha (1) and beta (2), with the name box for a new `kind`
    /// group in `project` holding `text`.
    fn naming_group(kind: GroupKind, project: Option<i64>, text: &str) -> AppState {
        let state = with_projects(&["alpha", "beta"]);
        AppState {
            focus: Focus::Rename,
            rename: Some(Rename {
                target: RenameTarget::NewGroup {
                    kind,
                    project: project.map(ProjectId),
                },
                input: TextInput::new(text),
                creating: false,
            }),
            ..state
        }
    }

    #[rstest::rstest]
    fn confirming_a_group_name_emits_create_group() {
        // Given the name box for a new Research group holding `tokio cancel`.
        let mut state = naming_group(GroupKind::Research, None, "tokio cancel");

        // When handling PickerConfirm.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the group is asked for under its slug.
        assert_eq!(
            commands,
            vec![Command::CreateGroup {
                kind: GroupKind::Research,
                project: None,
                name: "tokio-cancel".into(),
            }],
            "a valid name should ask for the group"
        );
    }

    #[rstest::rstest]
    fn feature_group_slug_keeps_case() {
        // Given the name box for a new Feature group in alpha holding
        // `GT-514 login`.
        let mut state = naming_group(GroupKind::Feature, Some(1), "GT-514 login");

        // When handling PickerConfirm.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the slug keeps the name's case.
        let names: Vec<&str> = commands
            .iter()
            .filter_map(|command| match command {
                Command::CreateGroup { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["GT-514-login"], "the slug should keep case");
    }

    /// Threads 1 and 2 (listed 2, 1) with the cursor on thread 2, the keys
    /// in `focus`, the threads in `attached` attached, and `jumps` listed,
    /// oldest first.
    fn jumping(focus: Focus, attached: &[i64], jumps: &[SidebarItem]) -> AppState {
        AppState {
            focus,
            attached: attached.iter().copied().map(ThreadId).collect(),
            jumps: JumpList::from_saved(jumps.to_vec()),
            ..state_with(
                vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
                2,
            )
        }
    }

    #[rstest::rstest]
    fn attach_on_b_after_a_lists_a_then_b() {
        // Given thread 1 entered, then the cursor moved to thread 2.
        let mut state = jumping(Focus::Sidebar, &[], &[]);
        state.sessions.cursor = Some(on_thread(1));
        IntentHandler::handle(&Intent::Attach, &mut state);
        state.focus = Focus::Sidebar;
        state.sessions.cursor = Some(on_thread(2));

        // When handling Attach on thread 2.
        IntentHandler::handle(&Intent::Attach, &mut state);

        // Then the jump list is thread 1, then thread 2.
        assert_eq!(
            state.jumps.entries(),
            [on_thread(1), on_thread(2)],
            "entering each pane should record it"
        );
    }

    #[rstest::rstest]
    fn attach_returns_save_jumps() {
        // Given thread 2 selected.
        let mut state = jumping(Focus::Sidebar, &[], &[]);

        // When handling Attach.
        let commands = IntentHandler::handle(&Intent::Attach, &mut state);

        // Then the jump list is saved.
        assert!(
            commands.contains(&Command::SaveJumps),
            "entering a pane should save the jump list"
        );
    }

    #[rstest::rstest]
    fn select_first_records_the_row_left_then_the_first_row() {
        // Given the cursor on thread 1, the last row.
        let mut state = jumping(Focus::Sidebar, &[], &[]);
        state.sessions.cursor = Some(on_thread(1));

        // When handling SelectFirst (`gg`).
        IntentHandler::handle(&Intent::SelectFirst, &mut state);

        // Then thread 1, then thread 2, are recorded.
        assert_eq!(
            state.jumps.entries(),
            [on_thread(1), on_thread(2)],
            "gg should record where it left and where it landed"
        );
    }

    #[rstest::rstest]
    fn select_first_on_the_first_row_records_nothing() {
        // Given the cursor on thread 2, the first row.
        let mut state = jumping(Focus::Sidebar, &[], &[]);

        // When handling SelectFirst (`gg`).
        IntentHandler::handle(&Intent::SelectFirst, &mut state);

        // Then the jump list is still empty.
        assert!(state.jumps.entries().is_empty(), "gg in place isn't a jump");
    }

    #[rstest::rstest]
    fn select_last_returns_save_jumps() {
        // Given the cursor on thread 2, the first row.
        let mut state = jumping(Focus::Sidebar, &[], &[]);

        // When handling SelectLast (`G`).
        let commands = IntentHandler::handle(&Intent::SelectLast, &mut state);

        // Then the jump list is saved.
        assert!(
            commands.contains(&Command::SaveJumps),
            "G should save the jump list"
        );
    }

    #[rstest::rstest]
    #[case(Intent::SelectNext)]
    #[case(Intent::SelectHalfPageDown)]
    fn plain_cursor_moves_record_nothing(#[case] intent: Intent) {
        // Given the cursor on thread 2, the first row.
        let mut state = jumping(Focus::Sidebar, &[], &[]);

        // When moving the cursor down.
        IntentHandler::handle(&intent, &mut state);

        // Then the jump list is still empty.
        assert!(state.jumps.entries().is_empty(), "{intent:?} isn't a jump");
    }

    #[rstest::rstest]
    fn search_confirm_on_a_match_records_the_row_before_search_then_the_match() {
        // Given a search for "fix" started from thread 2, on match thread 3.
        let mut state = searching("fix", Some(3));

        // When handling PickerConfirm (`⏎`).
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then thread 2, then thread 3, are recorded.
        assert_eq!(
            state.jumps.entries(),
            [on_thread(2), on_thread(3)],
            "a search ⏎ should record where it left and where it landed"
        );
    }

    #[rstest::rstest]
    fn new_session_pick_records_the_previous_row_then_the_draft() {
        // Given the project picker opened from alpha's thread 11, on beta.
        let mut state = with_projects(&["alpha", "beta"]);
        if let Some(project) = state.sessions.projects.first_mut() {
            project.threads = vec![thread(11, ThreadStatus::Idle)];
        }
        state.sessions.cursor = Some(on_thread(11));
        IntentHandler::handle(&Intent::NewSession, &mut state);
        highlight(&mut state, &project_row(2, "beta"));

        // When picking beta.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then thread 11, then beta's draft, are recorded.
        assert_eq!(
            state.jumps.entries(),
            [on_thread(11), SidebarItem::Draft(ProjectId(2))],
            "a ␣n pick should record where it left and the draft"
        );
    }

    #[rstest::rstest]
    fn jump_back_moves_the_cursor_to_the_older_row() {
        // Given thread 1 listed before the cursor's thread 2.
        let mut state = jumping(Focus::Sidebar, &[], &[on_thread(1), on_thread(2)]);

        // When handling JumpBack.
        IntentHandler::handle(&Intent::JumpBack, &mut state);

        // Then the cursor is on thread 1.
        assert_eq!(
            state.sessions.cursor,
            Some(on_thread(1)),
            "<C-o> should land on the older row"
        );
    }

    #[rstest::rstest]
    fn jump_forward_after_jump_back_returns_to_the_row() {
        // Given a jump back from thread 2 to thread 1.
        let mut state = jumping(Focus::Sidebar, &[], &[on_thread(1), on_thread(2)]);
        IntentHandler::handle(&Intent::JumpBack, &mut state);

        // When handling JumpForward.
        IntentHandler::handle(&Intent::JumpForward, &mut state);

        // Then the cursor is back on thread 2.
        assert_eq!(
            state.sessions.cursor,
            Some(on_thread(2)),
            "<C-i> should undo <C-o>"
        );
    }

    #[rstest::rstest]
    fn jump_back_returns_save_jumps() {
        // Given thread 1 listed before the cursor's thread 2.
        let mut state = jumping(Focus::Sidebar, &[], &[on_thread(1), on_thread(2)]);

        // When handling JumpBack.
        let commands = IntentHandler::handle(&Intent::JumpBack, &mut state);

        // Then the jump list is saved.
        assert!(
            commands.contains(&Command::SaveJumps),
            "<C-o> should save the jump list"
        );
    }

    #[rstest::rstest]
    fn jump_back_without_a_target_returns_no_commands() {
        // Given only the cursor's thread listed.
        let mut state = jumping(Focus::Sidebar, &[], &[on_thread(2)]);

        // When handling JumpBack.
        let commands = IntentHandler::handle(&Intent::JumpBack, &mut state);

        // Then nothing happens.
        assert!(commands.is_empty(), "<C-o> with nowhere to go does nothing");
    }

    #[rstest::rstest]
    fn jump_back_from_a_pane_onto_an_attached_thread_focuses_its_pane() {
        // Given the keys in thread 2's pane, with thread 1 attached and listed.
        let mut state = jumping(Focus::Attached, &[1, 2], &[on_thread(1), on_thread(2)]);

        // When handling JumpBack.
        IntentHandler::handle(&Intent::JumpBack, &mut state);

        // Then the keys are in thread 1's pane.
        assert_eq!(
            (state.focus, state.sessions.selected_id()),
            (Focus::Attached, Some(ThreadId(1))),
            "<C-o> from a pane should follow into the target's pane"
        );
    }

    #[rstest::rstest]
    fn jump_back_from_a_pane_onto_an_attached_thread_returns_attach() {
        // Given the keys in thread 2's pane, with thread 1 attached and listed.
        let mut state = jumping(Focus::Attached, &[1, 2], &[on_thread(1), on_thread(2)]);

        // When handling JumpBack.
        let commands = IntentHandler::handle(&Intent::JumpBack, &mut state);

        // Then the loop shows thread 1's pane.
        assert!(
            commands.contains(&Command::Attach(AttachTarget {
                thread: ThreadId(1),
                argv: vec!["claude".into(), "attach".into(), "t1".into()],
                cwd: "/work/1".into(),
            })),
            "<C-o> onto a live pane should show it"
        );
    }

    #[rstest::rstest]
    fn jump_back_into_a_pane_adds_no_entry() {
        // Given the keys in thread 2's pane, with thread 1 attached and listed.
        let mut state = jumping(Focus::Attached, &[1, 2], &[on_thread(1), on_thread(2)]);

        // When handling JumpBack.
        IntentHandler::handle(&Intent::JumpBack, &mut state);

        // Then the list is unchanged.
        assert_eq!(
            state.jumps.entries(),
            [on_thread(1), on_thread(2)],
            "landing in a pane isn't itself a jump"
        );
    }

    #[rstest::rstest]
    fn re_entering_the_pane_jump_back_landed_in_keeps_jump_forward() {
        // Given a jump back from thread 2's pane into thread 1's, then
        // <C-h> to the sidebar and <C-l> back into thread 1's pane.
        let mut state = jumping(Focus::Attached, &[1, 2], &[on_thread(1), on_thread(2)]);
        IntentHandler::handle(&Intent::JumpBack, &mut state);
        IntentHandler::handle(&Intent::FocusSidebar, &mut state);
        IntentHandler::handle(&Intent::FocusRight, &mut state);

        // When handling JumpForward.
        IntentHandler::handle(&Intent::JumpForward, &mut state);

        // Then the cursor is back on thread 2.
        assert_eq!(
            state.sessions.cursor,
            Some(on_thread(2)),
            "re-entering the landed pane should keep <C-i>"
        );
    }

    #[rstest::rstest]
    fn re_entering_the_pane_jump_back_landed_in_returns_no_save_jumps() {
        // Given a jump back from thread 2's pane into thread 1's, then
        // <C-h> to the sidebar.
        let mut state = jumping(Focus::Attached, &[1, 2], &[on_thread(1), on_thread(2)]);
        IntentHandler::handle(&Intent::JumpBack, &mut state);
        IntentHandler::handle(&Intent::FocusSidebar, &mut state);

        // When handling FocusRight back into thread 1's pane.
        let commands = IntentHandler::handle(&Intent::FocusRight, &mut state);

        // Then the list isn't saved: nothing was recorded.
        assert!(
            !commands.contains(&Command::SaveJumps),
            "re-entering the landed pane records nothing to save"
        );
    }

    #[rstest::rstest]
    fn jump_back_from_the_sidebar_onto_an_attached_thread_keeps_the_sidebar_focused() {
        // Given the sidebar on thread 2, with thread 1 attached and listed.
        let mut state = jumping(Focus::Sidebar, &[1], &[on_thread(1), on_thread(2)]);

        // When handling JumpBack.
        IntentHandler::handle(&Intent::JumpBack, &mut state);

        // Then the sidebar keeps the keys.
        assert_eq!(
            state.focus,
            Focus::Sidebar,
            "a sidebar <C-o> never moves the keys"
        );
    }

    #[rstest::rstest]
    fn jump_back_onto_a_thread_without_a_pane_returns_no_attach() {
        // Given the sidebar on thread 2, with thread 1 listed and unattached.
        let mut state = jumping(Focus::Sidebar, &[], &[on_thread(1), on_thread(2)]);

        // When handling JumpBack.
        let commands = IntentHandler::handle(&Intent::JumpBack, &mut state);

        // Then it only saves the list and visits thread 1.
        assert_eq!(
            commands,
            vec![Command::SaveJumps, Command::Visit(ThreadId(1))],
            "<C-o> must never attach"
        );
    }

    #[rstest::rstest]
    fn jump_back_from_a_pane_onto_a_thread_without_a_pane_focuses_the_sidebar() {
        // Given the keys in thread 2's pane, with thread 1 listed and unattached.
        let mut state = jumping(Focus::Attached, &[2], &[on_thread(1), on_thread(2)]);

        // When handling JumpBack.
        IntentHandler::handle(&Intent::JumpBack, &mut state);

        // Then the sidebar has the keys.
        assert_eq!(
            state.focus,
            Focus::Sidebar,
            "<C-o> out of a pane onto a row without one goes to the sidebar"
        );
    }

    #[rstest::rstest]
    fn jump_back_from_a_pane_onto_a_thread_without_a_pane_returns_detach() {
        // Given the keys in thread 2's pane, with thread 1 listed and unattached.
        let mut state = jumping(Focus::Attached, &[2], &[on_thread(1), on_thread(2)]);

        // When handling JumpBack.
        let commands = IntentHandler::handle(&Intent::JumpBack, &mut state);

        // Then the pane loses the keys.
        assert!(
            commands.contains(&Command::Detach),
            "leaving a pane should detach its keys"
        );
    }

    #[rstest::rstest]
    fn jump_back_from_a_pane_with_the_sidebar_hidden_focuses_the_dashboard() {
        // Given the keys in thread 2's pane with the sidebar hidden, and
        // thread 1 listed and unattached.
        let mut state = jumping(Focus::Attached, &[2], &[on_thread(1), on_thread(2)]);
        state.sidebar.hidden = true;

        // When handling JumpBack.
        IntentHandler::handle(&Intent::JumpBack, &mut state);

        // Then the dashboard has the keys.
        assert_eq!(
            state.focus,
            Focus::Dashboard,
            "with the sidebar hidden, <C-o> out of a pane goes to the dashboard"
        );
    }

    #[rstest::rstest]
    fn jump_back_onto_a_draft_returns_no_start() {
        // Given the sidebar on thread 1, with the project's draft listed.
        let mut state = drafting(
            draft(DraftWorkspace::Local),
            vec![thread(1, ThreadStatus::Idle)],
        );
        state.focus = Focus::Sidebar;
        state.sessions.cursor = Some(on_thread(1));
        state.jumps = JumpList::from_saved(vec![SidebarItem::Draft(ProjectId(1)), on_thread(1)]);

        // When handling JumpBack.
        let commands = IntentHandler::handle(&Intent::JumpBack, &mut state);

        // Then it only saves the list.
        assert_eq!(
            commands,
            vec![Command::SaveJumps],
            "<C-o> onto a draft must not start it"
        );
    }

    #[rstest::rstest]
    fn jump_back_skips_a_row_hidden_by_the_filter() {
        // Given the sidebar filtered to beta on thread 22, with beta's
        // thread 21, then alpha's thread 11, listed before it.
        let mut state = with_projects(&["alpha", "beta"]);
        if let [alpha, beta] = state.sessions.projects.as_mut_slice() {
            alpha.threads = vec![thread(11, ThreadStatus::Idle)];
            beta.threads = vec![
                thread(21, ThreadStatus::Idle),
                thread(22, ThreadStatus::Idle),
            ];
        }
        state.sessions.filter = Some(ProjectId(2));
        state.sessions.cursor = Some(on_thread(22));
        state.jumps = JumpList::from_saved(vec![on_thread(21), on_thread(11), on_thread(22)]);

        // When handling JumpBack.
        IntentHandler::handle(&Intent::JumpBack, &mut state);

        // Then it lands on thread 21.
        assert_eq!(
            state.sessions.cursor,
            Some(on_thread(21)),
            "<C-o> should skip rows the filter hides"
        );
    }

    #[rstest::rstest]
    fn jump_back_onto_a_thread_in_a_folded_group_opens_it() {
        // Given group 9 folded with the cursor on its card, and its thread 1
        // listed before the card.
        let mut state = grouped_state(false, SidebarItem::Group(GroupId(9)));
        state.sessions.folded.insert(GroupId(9));
        state.jumps = JumpList::from_saved(vec![on_thread(1), SidebarItem::Group(GroupId(9))]);

        // When handling JumpBack.
        IntentHandler::handle(&Intent::JumpBack, &mut state);

        // Then the group is open.
        assert!(
            !state.sessions.folded.contains(&GroupId(9)),
            "<C-o> into a folded group should open it"
        );
    }

    #[rstest::rstest]
    fn jump_back_onto_a_thread_in_a_settled_closed_group_opens_it_and_the_shelf() {
        // Given settled group 9 closed under a closed shelf with the cursor on
        // its card, and its thread 1 listed before the card.
        let mut state = grouped_state(true, SidebarItem::Group(GroupId(9)));
        state.jumps = JumpList::from_saved(vec![on_thread(1), SidebarItem::Group(GroupId(9))]);

        // When handling JumpBack.
        IntentHandler::handle(&Intent::JumpBack, &mut state);

        // Then the group and the shelf are open.
        assert!(
            state.sessions.opened.contains(&GroupId(9)) && state.sessions.shelf_open,
            "<C-o> into a settled group should open it and the shelf"
        );
    }

    #[rstest::rstest]
    fn jump_back_after_a_delete_skips_the_deleted_thread() {
        // Given threads 3, 2 and 1 listed, and 1, 3, 2 in the jump list, with
        // thread 2 deleted from the sidebar (the cursor moves to thread 1).
        let mut state = state_with(
            (1..=3).map(|id| thread(id, ThreadStatus::Idle)).collect(),
            2,
        );
        state.jumps = JumpList::from_saved(vec![on_thread(1), on_thread(3), on_thread(2)]);
        answer_yes(&Intent::DeleteThread, &mut state);
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // When handling JumpBack.
        IntentHandler::handle(&Intent::JumpBack, &mut state);

        // Then it lands on thread 3.
        assert_eq!(
            state.sessions.cursor,
            Some(on_thread(3)),
            "a deleted thread is never a target"
        );
    }

    #[rstest::rstest]
    fn delete_drops_the_thread_from_the_jump_list() {
        // Given threads 1 and 2 in the jump list, and Yes highlighted in
        // selected thread 2's delete confirm.
        let mut state = jumping(Focus::Sidebar, &[], &[on_thread(1), on_thread(2)]);
        answer_yes(&Intent::DeleteThread, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then only thread 1 is left.
        assert_eq!(
            state.jumps.entries(),
            [on_thread(1)],
            "deleting should drop the thread from the jump list"
        );
    }

    #[rstest::rstest]
    fn discard_drops_the_draft_from_the_jump_list() {
        // Given the selected draft in the jump list, and Yes highlighted in
        // its discard confirm.
        let mut state = drafting(draft(DraftWorkspace::Local), vec![]);
        state.jumps = JumpList::from_saved(vec![SidebarItem::Draft(ProjectId(1))]);
        answer_yes(&Intent::DeleteThread, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the jump list is empty.
        assert!(
            state.jumps.entries().is_empty(),
            "discarding should drop the draft from the jump list"
        );
    }
}
