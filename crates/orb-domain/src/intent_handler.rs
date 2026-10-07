//! The [`IntentHandler`]: the single decision point for all user input.

use std::path::{Path, PathBuf};

use crate::command::Workspace;
use crate::feat::git::validator::{
    BUSY_DIRECTORY, ChangeWorkspaceError, SwitchBranchError, validate_change_workspace,
    validate_switch_branch,
};
use crate::feat::git::worktree::previous_worktree;
use crate::feat::jumps::validator::{validate_jump_back, validate_jump_forward};
use crate::feat::layout::state::{FocusMove, Layouts, SessionLayout, Tab};
use crate::feat::layout::tree::NavDirection;
use crate::feat::layout::validator::{
    validate_focus_pane, validate_pane_action, validate_tab_action,
};
use crate::feat::pane::validator::{validate_attach, validate_detach};
use crate::feat::picker::list::{BranchRow, PickerItem, WorkspaceChoice};
use crate::feat::picker::state::{
    PickTarget, PickerKind, PickerState, session_items, worktree_items,
};
use crate::feat::picker::validator::{
    DeleteWorktreeError, validate_add_directory, validate_delete_worktree, validate_open_directory,
    validate_pick_project, validate_pick_session, validate_remove_project,
};
use crate::feat::sessions::state::{
    FolderKind, Project, ProjectId, ProjectKind, Search, SessionId, SessionKind, Sessions,
    SidebarItem, ThreadId, folder_slug,
};
use crate::feat::sessions::validator::{
    NewFolderError, SETTLE_IN_PROGRESS, ToggleSettleError, validate_close_shelf, validate_delete,
    validate_new_folder, validate_new_incognito, validate_new_session, validate_open_shelf,
    validate_toggle_pin, validate_toggle_settle,
};
use crate::feat::sidebar::state::{Rename, RenameTarget};
use crate::feat::sidebar::validator::{validate_focus_sidebar, validate_rename, validate_resize};
use crate::feat::zellij::validator::validate_open_tool;
use crate::{AppState, Command, Focus, Intent, TextInput};

/// Applies each [`Intent`] to [`AppState`] in one match block.
pub struct IntentHandler;

impl IntentHandler {
    /// Apply `intent` to `state` and return the commands that must follow.
    /// Every intent first clears the mode line's error and worktree notice,
    /// which the user has now seen; otherwise an intent that fails validation
    /// changes nothing.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per intent keeps every input decision in one match"
    )]
    pub fn handle(intent: &Intent, state: &mut AppState) -> Vec<Command> {
        state.sessions.error = None;
        state.worktrees.notice = None;
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
            Intent::SelectRow(item) if state.focus == Focus::Search => {
                state.sessions.select_row(*item);
                search_key(&Intent::PickerConfirm, state)
            }
            Intent::SelectRow(item) => {
                state.sessions.select_row(*item);
                with_visit(state, vec![])
            }
            Intent::SelectWheelNext => {
                state.sessions.select_below();
                with_visit(state, vec![])
            }
            Intent::SelectWheelPrev => {
                state.sessions.select_above();
                with_visit(state, vec![])
            }
            Intent::FocusRight => focus_right(state),
            Intent::FocusSidebar => match validate_focus_sidebar(state) {
                Ok(()) => {
                    state.focus = Focus::Sidebar;
                    vec![]
                }
                Err(_) => vec![],
            },
            Intent::ToggleSidebar => match (state.sidebar.hidden, state.focus) {
                // `<C-b>` in the attached pane: the keys stay in the pane.
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
            Intent::Attach => attach_session(state),
            Intent::Detach => {
                state.focus = Focus::Sidebar;
                if let Some(id) = state.sessions.selected_session().map(|session| session.id) {
                    state.attached.remove(&id);
                }
                vec![Command::Detach, Command::RefreshSessions]
            }
            Intent::LeavePane => leave_pane(state),
            Intent::MoveFocus(nav) => match (state.focus, nav) {
                (Focus::Sidebar, NavDirection::Right) => focus_right(state),
                (Focus::Attached, _) => move_focus(state, *nav),
                _ => vec![],
            },
            Intent::SplitPane(split) => {
                match (validate_pane_action(state), state.shown_session()) {
                    (Ok(()), Some(session)) => vec![Command::SplitPane {
                        session,
                        split: *split,
                    }],
                    _ => vec![],
                }
            }
            Intent::ClosePane => close_pane(state),
            Intent::ToggleZoom => {
                in_pane(state, Layouts::toggle_zoom);
                vec![]
            }
            Intent::GrowFocused => grow_or_shrink(state, true),
            Intent::ShrinkFocused => grow_or_shrink(state, false),
            Intent::NewTab => match (validate_tab_action(state), state.shown_session()) {
                (Ok(()), Some(session)) => vec![Command::NewTab(session)],
                _ => vec![],
            },
            Intent::CloseTab => close_tab(state),
            Intent::RenameTab => rename_tab(state),
            Intent::RenamePane => rename_pane(state),
            Intent::GoToTab(n) => on_tabs(state, |layouts, owner| layouts.go_to_tab(owner, *n)),
            Intent::NextTab => on_tabs(state, Layouts::next_tab),
            Intent::PreviousTab => on_tabs(state, Layouts::previous_tab),
            Intent::MoveTabLeft => on_tabs(state, Layouts::move_tab_left),
            Intent::MoveTabRight => on_tabs(state, Layouts::move_tab_right),
            Intent::FocusPane(pane) => {
                match (validate_focus_pane(state, *pane), state.shown_session()) {
                    (Ok(()), Some(owner)) => {
                        state.layouts.focus_pane(owner, *pane);
                        let mut commands = attach_session(state);
                        commands.push(Command::SaveLayout(owner));
                        commands
                    }
                    _ => vec![],
                }
            }
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
                match (
                    validate_detach(state),
                    state.sessions.selected_session().map(|session| session.id),
                ) {
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
                validate_new_session(state),
                state
                    .sessions
                    .own_project(ProjectKind::Incognito)
                    .map(|project| project.id),
            ) {
                (Ok(()), Ok(()), Some(project)) => {
                    let mut commands = unfilter(state, project);
                    commands.extend(new_session(state, project, Workspace::Checkout));
                    commands
                }
                _ => vec![],
            },
            Intent::OpenSessionPicker => {
                let items = session_items(&state.sessions, false);
                open_picker(state, PickerState::sessions(items, state.focus));
                preview_command(state)
            }
            Intent::OpenWorktreePicker => {
                let items = worktree_items(state);
                open_picker(state, PickerState::worktrees(items, state.focus));
                vec![Command::RefreshWorktrees]
            }
            Intent::OpenSearch => {
                open_picker(state, PickerState::search(state.focus));
                vec![Command::SearchTranscripts {
                    query: String::new(),
                }]
            }
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
                state.sessions.selected_project(),
                state.sessions.selected_session(),
            ) {
                (Ok(()), Some(project), Some(_)) if !project.repo => {
                    let picker = PickerState::init_git(project.id, state.focus);
                    open_picker(state, picker);
                    vec![]
                }
                (Ok(()), Some(project), Some(session)) => {
                    let worktree = session.dir != project.root;
                    let items = workspace_items(&state.sessions, project, &session.dir, worktree);
                    let target = PickTarget::Move(session.id);
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
            | Intent::PickerCursorTo(_)
            | Intent::PickerNext
            | Intent::PickerPrev
            | Intent::PickerHalfPageDown
            | Intent::PickerHalfPageUp
            | Intent::PickerConfirm
            | Intent::PickerOpen
            | Intent::PickerCancel
            | Intent::PickerRemove
            | Intent::PickerSelectRow(_)
            | Intent::PickerWheelNext
            | Intent::PickerWheelPrev
                if state.focus == Focus::Rename =>
            {
                rename_key(intent, state)
            }
            Intent::PickerInput(_)
            | Intent::PickerBackspace
            | Intent::PickerDeleteWord
            | Intent::PickerCursorLeft
            | Intent::PickerCursorRight
            | Intent::PickerCursorTo(_)
            | Intent::PickerNext
            | Intent::PickerPrev
            | Intent::PickerHalfPageDown
            | Intent::PickerHalfPageUp
            | Intent::PickerConfirm
            | Intent::PickerOpen
            | Intent::PickerCancel
            | Intent::PickerRemove
            | Intent::PickerSelectRow(_)
            | Intent::PickerWheelNext
            | Intent::PickerWheelPrev
                if state.focus == Focus::Search =>
            {
                search_key(intent, state)
            }
            Intent::PickerInput(ch) => {
                let before = picked(state);
                let commands = list(state.picker.as_mut().and_then(|p| p.insert(*ch)));
                let commands = with_preview(state, before, commands);
                with_search(state, commands)
            }
            Intent::PickerBackspace => {
                let before = picked(state);
                let commands = list(state.picker.as_mut().and_then(PickerState::backspace));
                let commands = with_preview(state, before, commands);
                with_search(state, commands)
            }
            Intent::PickerDeleteWord => {
                let before = picked(state);
                let commands = list(state.picker.as_mut().and_then(PickerState::delete_word));
                let commands = with_preview(state, before, commands);
                with_search(state, commands)
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
            Intent::PickerCursorTo(index) => {
                if let Some(picker) = &mut state.picker {
                    picker.cursor_to(*index);
                }
                vec![]
            }
            Intent::PickerNext => {
                let before = picked(state);
                if let Some(picker) = &mut state.picker {
                    picker.next();
                }
                with_preview(state, before, vec![])
            }
            Intent::PickerPrev => {
                let before = picked(state);
                if let Some(picker) = &mut state.picker {
                    picker.prev();
                }
                with_preview(state, before, vec![])
            }
            Intent::PickerHalfPageDown => {
                let before = picked(state);
                if let Some(picker) = &mut state.picker {
                    picker.half_page_down();
                }
                with_preview(state, before, vec![])
            }
            Intent::PickerHalfPageUp => {
                let before = picked(state);
                if let Some(picker) = &mut state.picker {
                    picker.half_page_up();
                }
                with_preview(state, before, vec![])
            }
            Intent::PickerSelectRow(index) => {
                let before = picked(state);
                if let Some(picker) = &mut state.picker {
                    picker.select_row(*index);
                }
                with_preview(state, before, vec![])
            }
            Intent::PickerWheelNext => {
                let before = picked(state);
                if let Some(picker) = &mut state.picker {
                    picker.select_below();
                }
                with_preview(state, before, vec![])
            }
            Intent::PickerWheelPrev => {
                let before = picked(state);
                if let Some(picker) = &mut state.picker {
                    picker.select_above();
                }
                with_preview(state, before, vec![])
            }
            Intent::SwitchBranch => match (
                validate_switch_branch(state),
                state.sessions.selected_project(),
                state.sessions.selected_session(),
            ) {
                (Ok(()), Some(project), Some(_)) if !project.repo => {
                    let picker = PickerState::init_git(project.id, state.focus);
                    open_picker(state, picker);
                    vec![]
                }
                (Ok(()), Some(_), Some(session)) => {
                    let (id, cwd) = (session.id, session.dir.clone());
                    let unstarted = !state.sessions.turned(id);
                    let picker =
                        PickerState::branches(id, cwd.clone(), unstarted, None, state.focus);
                    open_picker(state, picker);
                    vec![Command::ListBranches(cwd)]
                }
                (Err(SwitchBranchError::Busy), ..) => {
                    state.sessions.error = Some(BUSY_DIRECTORY.to_owned());
                    vec![]
                }
                _ => vec![],
            },
            Intent::OpenTool(tool) => {
                match (validate_open_tool(state), state.sessions.selected_session()) {
                    (Ok(()), Some(session)) => vec![Command::OpenTool {
                        tool: *tool,
                        cwd: session.dir.clone(),
                    }],
                    _ => vec![],
                }
            }
            Intent::PickerOpen => match validate_open_directory(state) {
                Ok(()) => list(state.picker.as_mut().and_then(PickerState::open_directory)),
                Err(_) => vec![],
            },
            Intent::PickerConfirm => match state.picker.as_ref().map(PickerState::kind) {
                Some(&PickerKind::Workspace {
                    target: PickTarget::Move(session),
                }) => {
                    let choice = close_picker(state)
                        .as_ref()
                        .and_then(PickerState::selected)
                        .cloned();
                    pick_move_workspace(state, session, choice)
                }
                Some(&PickerKind::Workspace {
                    target: PickTarget::New(project),
                }) => {
                    let choice = close_picker(state)
                        .as_ref()
                        .and_then(PickerState::selected)
                        .cloned();
                    pick_new_workspace(state, project, choice)
                }
                Some(&PickerKind::Base {
                    target: PickTarget::New(project),
                    ..
                }) => match close_picker(state).as_ref().and_then(PickerState::selected) {
                    Some(PickerItem::Branch(row)) => {
                        let base = row.git_ref.name.clone();
                        new_session(state, project, Workspace::NewWorktree { base })
                    }
                    _ => vec![],
                },
                Some(&PickerKind::Base {
                    target: PickTarget::Move(session),
                    ..
                }) => match close_picker(state).as_ref().and_then(PickerState::selected) {
                    Some(PickerItem::Branch(row)) => {
                        let base = row.git_ref.name.clone();
                        change_workspace(state, session, Workspace::NewWorktree { base })
                    }
                    _ => vec![],
                },
                Some(&PickerKind::InitGit { project }) => {
                    match close_picker(state).as_ref().and_then(PickerState::selected) {
                        Some(PickerItem::InitGit) => vec![Command::InitGit(project)],
                        _ => vec![],
                    }
                }
                Some(&PickerKind::RemoveProject { project }) => {
                    match close_picker(state).as_ref().and_then(PickerState::selected) {
                        Some(PickerItem::Confirm(true)) => remove_project(state, project),
                        _ => vec![],
                    }
                }
                Some(&PickerKind::SettleSession { session }) => {
                    match close_picker(state).as_ref().and_then(PickerState::selected) {
                        Some(PickerItem::Confirm(true)) => settle_session(state, session),
                        _ => vec![],
                    }
                }
                Some(&PickerKind::DeleteSession { session, .. }) => {
                    match close_picker(state).as_ref().and_then(PickerState::selected) {
                        Some(PickerItem::Confirm(true))
                            if still_deletable(state, SidebarItem::Session(session)) =>
                        {
                            delete_session(state, session)
                        }
                        _ => vec![],
                    }
                }
                Some(PickerKind::ProjectFilter) => {
                    match close_picker(state).as_ref().and_then(PickerState::selected) {
                        Some(PickerItem::AllProjects) => filter_to(state, None),
                        Some(&PickerItem::Project { id, .. }) => filter_to(state, Some(id)),
                        _ => vec![],
                    }
                }
                Some(PickerKind::Branches { .. }) => close_picker(state)
                    .map(|picker| pick_branch(state, &picker))
                    .unwrap_or_default(),
                Some(PickerKind::Sessions { .. } | PickerKind::Search { .. }) => {
                    let valid = validate_pick_session(state);
                    let session = close_picker(state)
                        .and_then(|picker| picker.picked_session(&state.sessions));
                    match (valid, session) {
                        (Ok(()), Some(session)) => pick_session(state, session),
                        _ => vec![],
                    }
                }
                Some(PickerKind::Worktrees) => vec![],
                Some(PickerKind::DeleteWorktree { .. }) => leave_delete_worktree(state),
                _ => match (validate_pick_project(state), validate_add_directory(state)) {
                    (Ok(()), _) => {
                        match close_picker(state).as_ref().and_then(PickerState::selected) {
                            Some(&PickerItem::Project { id, .. }) => pick_project(state, id),
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
            Intent::PickerCancel => match state.picker.as_ref().map(PickerState::kind) {
                Some(PickerKind::DeleteWorktree { .. }) => {
                    back_to_worktrees(state, false);
                    vec![]
                }
                _ => {
                    close_picker(state);
                    vec![]
                }
            },
            Intent::PickerRemove => match state.picker.as_ref().map(PickerState::kind) {
                Some(PickerKind::Worktrees) => ask_delete_worktree(state),
                _ => match (
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
            },
            Intent::PickerToggleSettled => {
                let before = picked(state);
                if let Some(picker) = &mut state.picker {
                    picker.toggle_settled(&state.sessions);
                }
                with_preview(state, before, vec![])
            }
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
            Intent::NewFolder(kind) => open_folder_name(state, *kind),
            Intent::TogglePin => {
                match (
                    validate_toggle_pin(state),
                    state.sessions.selected_session(),
                ) {
                    (Ok(()), Some(session)) => match session.pinned_at {
                        Some(_) => vec![Command::UnpinSession(session.id)],
                        None => vec![Command::PinSession(session.id)],
                    },
                    _ => vec![],
                }
            }
            Intent::Rename => match (validate_rename(state), state.sessions.selected_session()) {
                (Ok(()), Some(session)) => {
                    let rename = Rename {
                        target: RenameTarget::Session(session.id),
                        input: TextInput::new(&state.sessions.title(session)),
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
            Intent::ToggleSettle => {
                match (
                    validate_toggle_settle(state),
                    state.sessions.selected_session(),
                ) {
                    (Ok(()), Some(session)) if session.settled_at.is_some() => {
                        vec![Command::UnsettleSession(session.id)]
                    }
                    (Ok(()), Some(session)) => {
                        let picker = PickerState::settle_session(session.id, state.focus);
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
            Intent::Delete => match (validate_delete(state), state.sessions.cursor) {
                (Ok(()), Some(SidebarItem::Session(id))) => {
                    let folder = state.sessions.session(id).is_some_and(|session| {
                        matches!(session.kind, SessionKind::Research | SessionKind::Learn)
                    });
                    open_picker(state, PickerState::delete_session(id, folder, state.focus));
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

/// Asks the frontend to list `dir` into the directory picker, if there is one.
fn list(dir: Option<PathBuf>) -> Vec<Command> {
    dir.map(Command::ListDirectories).into_iter().collect()
}

/// What picking the selected branch of `picker`, a closed branch picker, asks
/// for:
/// - the current branch: nothing;
/// - before the first agent turn, a branch checked out in another worktree:
///   move the session there;
/// - before the first agent turn, the local default branch checked out
///   nowhere, from outside the project's checkout: check it out in the
///   checkout and move the session there;
/// - otherwise: check it out in the session's directory.
fn pick_branch(state: &mut AppState, picker: &PickerState) -> Vec<Command> {
    let (
        &PickerKind::Branches {
            session,
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
            let to = Workspace::Existing(path.clone());
            change_workspace(state, session, to)
        }
        _ => {
            let outside_root = state
                .sessions
                .session(session)
                .and_then(|found| state.sessions.project(found.project))
                .is_some_and(|project| project.root != *cwd);
            let to_root = unstarted
                && git_ref.default
                && !git_ref.remote
                && git_ref.worktree.is_none()
                && outside_root;
            let command = Command::SwitchBranch {
                session,
                git_ref: git_ref.clone(),
                to_root,
            };
            if to_root {
                detach_moving(state, session, command)
            } else {
                vec![command]
            }
        }
    }
}

/// The workspace picker's rows for something in `cwd`: stay there
/// (`worktree` is whether that's a worktree), a new worktree, then the
/// project's previous worktree other than `cwd`.
fn workspace_items(
    sessions: &Sessions,
    project: &Project,
    cwd: &Path,
    worktree: bool,
) -> Vec<PickerItem> {
    [
        WorkspaceChoice::Current { worktree },
        WorkspaceChoice::NewWorktree,
    ]
    .into_iter()
    .chain(
        previous_worktree(sessions, project, cwd)
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
/// projects.
fn remove_project(state: &mut AppState, project: ProjectId) -> Vec<Command> {
    if state.sessions.filter == Some(project) {
        state.sessions.filter = None;
    }
    with_visit(
        state,
        vec![Command::RemoveProject(project), Command::SaveUi],
    )
}

/// Settles `session`, answered `Yes` in its confirm, if the cursor is still
/// on it, it is still unsettled and no agent pane has a turn underway: it
/// leaves `attached` and the cursor moves to the neighbouring card. A turn
/// that started meanwhile shows the refusal on the mode line.
fn settle_session(state: &mut AppState, session: SessionId) -> Vec<Command> {
    match (
        validate_toggle_settle(state),
        state.sessions.selected_session(),
    ) {
        (Ok(()), Some(selected)) if selected.id == session && selected.settled_at.is_none() => {
            state.attached.remove(&session);
            state.sessions.cursor = state.sessions.card_neighbour(SidebarItem::Session(session));
            with_visit(state, vec![Command::SettleSession(session)])
        }
        (Err(ToggleSettleError::InProgress), Some(selected)) if selected.id == session => {
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

/// Hides `session` at once, detaches it, drops it from the jump list, moves
/// the cursor to the neighbouring row, and asks for the delete.
fn delete_session(state: &mut AppState, session: SessionId) -> Vec<Command> {
    let neighbour = state.sessions.row_neighbour(SidebarItem::Session(session));
    state.sessions.deleting.insert(session);
    state.attached.remove(&session);
    state.jumps.remove(SidebarItem::Session(session));
    state.sessions.cursor = neighbour;
    with_visit(
        state,
        vec![Command::DeleteSession(session), Command::SaveJumps],
    )
}

/// Picks `project` in the new-session picker: a filter to another project
/// goes back to all projects; a git project goes on to the workspace picker,
/// any other gets its session in its checkout at once.
fn pick_project(state: &mut AppState, project: ProjectId) -> Vec<Command> {
    let Some(found) = state.sessions.project(project) else {
        return vec![];
    };
    let picker = found.repo.then(|| {
        let items = workspace_items(&state.sessions, found, &found.root, false);
        PickerState::workspace(PickTarget::New(project), items, state.focus)
    });
    let mut commands = unfilter(state, project);
    match picker {
        Some(picker) => open_picker(state, picker),
        None => commands.extend(new_session(state, project, Workspace::Checkout)),
    }
    commands
}

/// Clears a sidebar filter to a project other than `project`, asking for
/// that to be saved.
fn unfilter(state: &mut AppState, project: ProjectId) -> Vec<Command> {
    let outside = state
        .sessions
        .filter
        .is_some_and(|filter| filter != project);
    if outside {
        state.sessions.filter = None;
    }
    outside.then_some(Command::SaveUi).into_iter().collect()
}

/// What picking `choice` in `project`'s new-session workspace picker asks
/// for: a session in the checkout or the previous worktree, or the base
/// branch picker for a new worktree, listing the project root's refs.
fn pick_new_workspace(
    state: &mut AppState,
    project: ProjectId,
    choice: Option<PickerItem>,
) -> Vec<Command> {
    match choice {
        Some(PickerItem::Workspace(WorkspaceChoice::Current { .. })) => {
            new_session(state, project, Workspace::Checkout)
        }
        Some(PickerItem::Workspace(WorkspaceChoice::Previous { path, .. })) => {
            new_session(state, project, Workspace::Existing(path))
        }
        Some(PickerItem::Workspace(WorkspaceChoice::NewWorktree)) => {
            let Some(root) = state.sessions.project(project).map(|p| p.root.clone()) else {
                return vec![];
            };
            let picker = PickerState::base(PickTarget::New(project), root.clone(), state.focus);
            open_picker(state, picker);
            vec![Command::ListBranches(root)]
        }
        _ => vec![],
    }
}

/// Asks for a new session of `project` in `workspace`, one at a time: marks
/// the start and records the row it leaves as a jump (`attach_session`
/// records the new session's row when the frontend attaches it).
fn new_session(state: &mut AppState, project: ProjectId, workspace: Workspace) -> Vec<Command> {
    if validate_new_session(state).is_err() {
        return vec![];
    }
    state.sessions.starting = true;
    state.jumps.jump(state.sessions.cursor, None);
    vec![
        Command::NewSession { project, workspace },
        Command::SaveJumps,
    ]
}

/// What picking `choice` in session `session`'s workspace picker asks for:
/// nothing to stay, a move to the previous worktree, or the base branch
/// picker for a new worktree, listing the project root's refs.
fn pick_move_workspace(
    state: &mut AppState,
    session: SessionId,
    choice: Option<PickerItem>,
) -> Vec<Command> {
    match choice {
        Some(PickerItem::Workspace(WorkspaceChoice::Previous { path, .. })) => {
            change_workspace(state, session, Workspace::Existing(path))
        }
        Some(PickerItem::Workspace(WorkspaceChoice::NewWorktree)) => {
            let Some(root) = state
                .sessions
                .session(session)
                .and_then(|found| state.sessions.project(found.project))
                .map(|project| project.root.clone())
            else {
                return vec![];
            };
            let picker = PickerState::base(PickTarget::Move(session), root.clone(), state.focus);
            open_picker(state, picker);
            vec![Command::ListBranches(root)]
        }
        _ => vec![],
    }
}

/// Asks for `session` to move to `to`, one start at a time (see
/// [`detach_moving`]).
fn change_workspace(state: &mut AppState, session: SessionId, to: Workspace) -> Vec<Command> {
    detach_moving(state, session, Command::ChangeWorkspace { session, to })
}

/// Asks for `command`, which moves `session` to another directory, one start
/// at a time: marks the start and detaches the session, so the frontend
/// drops its pane clients before the sessions actor kills its panes.
fn detach_moving(state: &mut AppState, session: SessionId, command: Command) -> Vec<Command> {
    if validate_new_session(state).is_err() {
        return vec![];
    }
    state.sessions.starting = true;
    state.attached.remove(&session);
    if state.focus == Focus::Attached {
        state.focus = Focus::Sidebar;
    }
    vec![command]
}

/// Jumps to `session`, picked in the session or search picker: puts the
/// cursor on it and attaches like `⏎` on its row, recording the move as a
/// jump.
fn pick_session(state: &mut AppState, session: SessionId) -> Vec<Command> {
    let from = state.sessions.cursor;
    state.sessions.cursor = Some(SidebarItem::Session(session));
    let mut commands = record_jump(state, from);
    commands.extend(show_pane(state));
    with_visit(state, commands)
}

/// Opens `picker` and gives it the keys.
fn open_picker(state: &mut AppState, picker: PickerState) {
    state.picker = Some(picker);
    state.focus = Focus::Picker;
}

/// The open picker's selected thread, and the selected hit's id when the row
/// is a search hit.
fn picked(state: &AppState) -> (Option<ThreadId>, Option<i64>) {
    let picker = state.picker.as_ref();
    (
        picker.and_then(PickerState::selected_thread),
        picker
            .and_then(PickerState::selected_hit)
            .map(|(id, ..)| id),
    )
}

/// `commands`, then [`preview_command`] when the picker's selected thread or
/// hit is no longer `before`.
fn with_preview(
    state: &mut AppState,
    before: (Option<ThreadId>, Option<i64>),
    commands: Vec<Command>,
) -> Vec<Command> {
    if picked(state) == before {
        return commands;
    }
    commands.into_iter().chain(preview_command(state)).collect()
}

/// `commands`, then a search for the typed text while the search picker is
/// open.
fn with_search(state: &AppState, commands: Vec<Command>) -> Vec<Command> {
    let query = state.picker.as_ref().and_then(PickerState::search_query);
    commands
        .into_iter()
        .chain(query.map(|query| Command::SearchTranscripts {
            query: query.to_owned(),
        }))
        .collect()
}

/// Asks for the picker's selected row to be read into its preview: a search
/// hit's exchange from the search index, or a thread's transcript. With
/// nothing selected, or a thread with no transcript yet, it clears the
/// preview instead and asks for nothing.
fn preview_command(state: &mut AppState) -> Vec<Command> {
    let Some(picker) = &mut state.picker else {
        return vec![];
    };
    if let Some((hit, path, prompt_offset)) = picker.selected_hit() {
        return vec![Command::LoadSearchPreview {
            hit,
            path: path.to_owned(),
            prompt_offset,
        }];
    }
    let load = picker.selected_thread().and_then(|id| {
        let thread = state.sessions.threads().find(|thread| thread.id == id)?;
        Some((id, thread.transcript.clone()?))
    });
    match load {
        Some((thread, transcript)) => vec![Command::LoadPreview { thread, transcript }],
        None => {
            picker.clear_preview();
            vec![]
        }
    }
}

/// Opens the empty name box for a new session in orb's own `kind` folder.
fn open_folder_name(state: &mut AppState, kind: FolderKind) -> Vec<Command> {
    state.rename = Some(Rename {
        target: RenameTarget::NewFolder(kind),
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

/// `<C-x>` in the worktree picker: the `No`/`Yes` confirm over the list, or
/// on an attached or mid-turn row, why it can't be deleted on the mode line.
fn ask_delete_worktree(state: &mut AppState) -> Vec<Command> {
    let why = match validate_delete_worktree(state) {
        Ok(()) => None,
        Err(DeleteWorktreeError::Attached) => Some("a thread in it is attached"),
        Err(DeleteWorktreeError::MidTurn) => Some("a thread in it is mid-turn"),
        Err(DeleteWorktreeError::NoPicker | DeleteWorktreeError::NoSelection) => return vec![],
    };
    let Some(path) = state
        .picker
        .as_ref()
        .and_then(PickerState::selected_worktree)
        .map(Path::to_path_buf)
    else {
        return vec![];
    };
    match why {
        Some(why) => {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            state.sessions.error = Some(format!("can't delete {name}: {why}"));
        }
        None => {
            let dirty = state
                .worktrees
                .list
                .iter()
                .find(|worktree| worktree.path == path)
                .and_then(|worktree| worktree.facts.as_ref())
                .is_some_and(|facts| facts.changes > 0);
            if let Some(list) = state.picker.take() {
                state.picker = Some(PickerState::delete_worktree(list, path, dirty));
            }
        }
    }
    vec![]
}

/// Ends the delete-worktree confirm: `Yes` returns `DeleteWorktree` for its
/// path, and either answer goes back to the list, without the row on `Yes`.
fn leave_delete_worktree(state: &mut AppState) -> Vec<Command> {
    let Some(picker) = &state.picker else {
        return vec![];
    };
    let delete = match (picker.kind(), picker.selected()) {
        (PickerKind::DeleteWorktree { path, .. }, Some(PickerItem::Confirm(true))) => {
            Some(path.clone())
        }
        _ => None,
    };
    back_to_worktrees(state, delete.is_some());
    delete
        .map(|path| Command::DeleteWorktree { path })
        .into_iter()
        .collect()
}

/// Swaps the delete-worktree confirm for the list under it, dropping the
/// selected row when `deleted`. The keys stay in the picker.
fn back_to_worktrees(state: &mut AppState, deleted: bool) {
    let Some(confirm) = state.picker.take() else {
        return;
    };
    let return_to = confirm.return_to();
    match confirm.back() {
        Some(mut list) => {
            if deleted {
                list.remove_worktree_row();
            }
            state.picker = Some(list);
        }
        None => state.focus = return_to,
    }
}

/// Attaches to the selected session and records entering it as a jump,
/// unless it can't be attached to or is the row the last `<C-o>`/`<C-i>`
/// landed on.
fn attach_session(state: &mut AppState) -> Vec<Command> {
    let mut commands = show_pane(state);
    if let (false, Some(id)) = (
        commands.is_empty(),
        state.sessions.selected_session().map(|session| session.id),
    ) && state.jumps.enter(SidebarItem::Session(id))
    {
        commands.push(Command::SaveJumps);
    }
    commands
}

/// Attaches to the selected session, adding it to the attached sessions and
/// showing its layout with the keys in its focused pane, unless it can't be
/// attached to or has no layout. A settled session is un-settled first.
fn show_pane(state: &mut AppState) -> Vec<Command> {
    let selected = state
        .sessions
        .selected_session()
        .map(|session| (session.id, session.settled_at.is_some()));
    match (validate_attach(state), selected) {
        (Ok(()), Some((id, settled))) if state.layouts.get(id).is_some() => {
            state.focus = Focus::Attached;
            state.attached.insert(id);
            settled
                .then_some(Command::UnsettleSession(id))
                .into_iter()
                .chain([Command::Attach(id), Command::RefreshSessions])
                .collect()
        }
        _ => vec![],
    }
}

/// Moves the keys into the shown layout's focused pane while the selected
/// session is attached and can still be attached to; with no layout shown
/// they stay put.
fn focus_right(state: &mut AppState) -> Vec<Command> {
    match state.sessions.selected_session().map(|session| session.id) {
        Some(id) if state.attached.contains(&id) && validate_attach(state).is_ok() => {
            attach_session(state)
        }
        _ => vec![],
    }
}

/// Moves the keys from a pane to the sidebar, unless it's hidden.
fn leave_pane(state: &mut AppState) -> Vec<Command> {
    match validate_focus_sidebar(state) {
        Ok(()) => {
            state.focus = Focus::Sidebar;
            vec![Command::Detach, Command::RefreshSessions]
        }
        Err(_) => vec![],
    }
}

/// Runs `edit` on the shown layout while the keys are in one of its panes;
/// the session it edited, if any.
fn in_pane<F>(state: &mut AppState, edit: F) -> Option<SessionId>
where
    F: FnOnce(&mut Layouts, SessionId),
{
    let (Ok(()), Some(owner)) = (validate_pane_action(state), state.shown_session()) else {
        return None;
    };
    edit(&mut state.layouts, owner);
    Some(owner)
}

/// Runs `edit` on the shown layout's tabs and saves the layout; the keys
/// stay where they are.
fn on_tabs<F>(state: &mut AppState, edit: F) -> Vec<Command>
where
    F: FnOnce(&mut Layouts, SessionId),
{
    let (Ok(()), Some(owner)) = (validate_tab_action(state), state.shown_session()) else {
        return vec![];
    };
    edit(&mut state.layouts, owner);
    vec![Command::SaveLayout(owner)]
}

/// Moves the focus to the pane `nav` of the focused one and saves the
/// layout; from the leftmost pane the keys go to the sidebar.
fn move_focus(state: &mut AppState, nav: NavDirection) -> Vec<Command> {
    let (Ok(()), Some(owner)) = (validate_pane_action(state), state.shown_session()) else {
        return vec![];
    };
    match state.layouts.move_focus(owner, nav) {
        FocusMove::Sidebar => leave_pane(state),
        FocusMove::Moved => vec![Command::SaveLayout(owner)],
        FocusMove::Stuck => vec![],
    }
}

/// Closes the focused pane and saves the layout.
fn close_pane(state: &mut AppState) -> Vec<Command> {
    let focused = state.shown_layout().and_then(SessionLayout::focused);
    let (Ok(()), Some(owner), Some(focused)) =
        (validate_pane_action(state), state.shown_session(), focused)
    else {
        return vec![];
    };
    state.layouts.close_pane(focused);
    vec![Command::SaveLayout(owner)]
}

/// Closes the shown tab and saves the layout.
fn close_tab(state: &mut AppState) -> Vec<Command> {
    let (Ok(()), Some(owner)) = (validate_tab_action(state), state.shown_session()) else {
        return vec![];
    };
    state.layouts.close_tab(owner);
    vec![Command::SaveLayout(owner)]
}

/// Grows (`grow`) or shrinks what has the keys: the sidebar, saving its new
/// width, or the focused pane.
fn grow_or_shrink(state: &mut AppState, grow: bool) -> Vec<Command> {
    match state.focus {
        Focus::Sidebar => {
            if validate_resize(state).is_err() {
                return vec![];
            }
            let changed = if grow {
                state.sidebar.widen()
            } else {
                state.sidebar.narrow()
            };
            changed.then_some(Command::SaveUi).into_iter().collect()
        }
        Focus::Attached => {
            let mut changed = false;
            let owner = in_pane(state, |layouts, owner| {
                changed = layouts.resize_focused(owner, grow);
            });
            owner
                .filter(|_| changed)
                .map(Command::SaveLayout)
                .into_iter()
                .collect()
        }
        Focus::Dashboard | Focus::Picker | Focus::Rename | Focus::Search => vec![],
    }
}

/// Opens the rename box on the shown tab, holding its name.
fn rename_tab(state: &mut AppState) -> Vec<Command> {
    let (Ok(()), Some(owner)) = (validate_tab_action(state), state.shown_session()) else {
        return vec![];
    };
    let Some((tab, name)) = state.layouts.get(owner).map(|layout| {
        let name = layout.active_tab().and_then(Tab::name).unwrap_or_default();
        (layout.active(), TextInput::new(name))
    }) else {
        return vec![];
    };
    state.rename = Some(Rename {
        target: RenameTarget::Tab { owner, tab },
        input: name,
        creating: false,
    });
    state.focus = Focus::Rename;
    vec![]
}

/// Opens the rename box on the focused pane, holding its name.
fn rename_pane(state: &mut AppState) -> Vec<Command> {
    let focused = state.shown_layout().and_then(SessionLayout::focused);
    let (Ok(()), Some(pane)) = (validate_pane_action(state), focused) else {
        return vec![];
    };
    let name = state
        .layouts
        .entry(pane)
        .and_then(|entry| entry.name.as_deref())
        .unwrap_or_default();
    state.rename = Some(Rename {
        target: RenameTarget::Pane(pane),
        input: TextInput::new(name),
        creating: false,
    });
    state.focus = Focus::Rename;
    vec![]
}

/// What a picker key does in the rename box: edit the name, save it (a blank
/// name goes back to the harness's title) or cancel, both giving the sidebar back
/// the keys. The picker's other keys do nothing.
fn rename_key(intent: &Intent, state: &mut AppState) -> Vec<Command> {
    match (intent, &mut state.rename) {
        (Intent::PickerInput(ch), Some(rename)) => rename.input.insert(*ch),
        (Intent::PickerBackspace, Some(rename)) => rename.input.backspace(),
        (Intent::PickerDeleteWord, Some(rename)) => rename.input.delete_word(),
        (Intent::PickerCursorLeft, Some(rename)) => rename.input.cursor_left(),
        (Intent::PickerCursorRight, Some(rename)) => rename.input.cursor_right(),
        (Intent::PickerCursorTo(index), Some(rename)) => rename.input.cursor_to(*index),
        (
            Intent::PickerConfirm,
            Some(Rename {
                target: RenameTarget::NewFolder(_),
                ..
            }),
        ) => return confirm_folder_name(state),
        (Intent::PickerConfirm, _) => {
            state.focus = Focus::Sidebar;
            let Some(rename) = state.rename.take() else {
                return vec![];
            };
            let name = rename.input.text().trim();
            let name = (!name.is_empty()).then(|| name.to_owned());
            return match rename.target {
                RenameTarget::Session(session) => vec![Command::RenameSession { session, name }],
                RenameTarget::NewFolder(_) => vec![],
                // The keys go back to the panes.
                RenameTarget::Tab { owner, tab } => {
                    state.layouts.rename_tab(owner, tab, name);
                    let mut commands = show_pane(state);
                    commands.push(Command::SaveLayout(owner));
                    commands
                }
                RenameTarget::Pane(pane) => {
                    state.layouts.rename_pane(pane, name);
                    let mut commands = show_pane(state);
                    commands.extend(state.layouts.owner_of(pane).map(Command::SaveLayout));
                    commands
                }
            };
        }
        (Intent::PickerCancel, _) => {
            state.focus = Focus::Sidebar;
            // A tab's or pane's rename box was opened from the panes; the keys
            // go back there.
            if let Some(Rename {
                target: RenameTarget::Tab { .. } | RenameTarget::Pane(_),
                ..
            }) = state.rename.take()
            {
                return show_pane(state);
            }
        }
        _ => {}
    }
    vec![]
}

/// `⏎` in the name box for a new Research or Learn session: an empty name,
/// one already asked for, or one while another session is being made does
/// nothing; an invalid one shows why on the mode line, keeping the box open;
/// a valid one asks for the session, and the sessions actor closes the box
/// once it's made.
fn confirm_folder_name(state: &mut AppState) -> Vec<Command> {
    match (validate_new_folder(state), validate_new_session(state)) {
        (Err(NewFolderError::Invalid(what)), _) => {
            state.sessions.error = Some(format!("Name can't use {what}"));
        }
        (Ok(()), Ok(())) => return new_folder_session(state),
        _ => {}
    }
    vec![]
}

/// Asks the sessions actor for the session the name box names, keeping the
/// box open until the actor answers: marks the start and records the row it
/// leaves as a jump, as [`new_session`] does.
fn new_folder_session(state: &mut AppState) -> Vec<Command> {
    let Some(Rename {
        target: RenameTarget::NewFolder(kind),
        input,
        creating,
    }) = &mut state.rename
    else {
        return vec![];
    };
    *creating = true;
    let command = Command::NewFolderSession {
        kind: *kind,
        name: folder_slug(input.text()),
    };
    state.sessions.starting = true;
    state.jumps.jump(state.sessions.cursor, None);
    vec![command, Command::SaveJumps]
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
        (Intent::PickerCursorTo(index), Some(search)) => {
            search.input.cursor_to(*index);
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

/// Lands a jump back or forward on `target`: the cursor moves there. From a
/// pane, the keys follow into the target's pane while
/// orb is attached to it, else go to the sidebar; elsewhere they stay put.
fn land(state: &mut AppState, target: Option<SidebarItem>) -> Vec<Command> {
    let Some(target) = target else {
        return vec![];
    };
    let from_pane = state.focus == Focus::Attached;
    state.sessions.cursor = Some(target);
    let pane = match target {
        SidebarItem::Session(id) if from_pane && state.attached.contains(&id) => show_pane(state),
        _ => vec![],
    };
    let mut commands = vec![Command::SaveJumps];
    match (from_pane, pane.is_empty()) {
        (true, true) => {
            state.focus = Focus::Sidebar;
            commands.extend([Command::Detach, Command::RefreshSessions]);
        }
        _ => commands.extend(pane),
    }
    with_visit(state, commands)
}

/// `commands`, then a visit to the session under the cursor, if any.
fn with_visit(state: &AppState, mut commands: Vec<Command>) -> Vec<Command> {
    commands.extend(
        state
            .sessions
            .selected_session()
            .map(|session| Command::Visit(session.id)),
    );
    commands
}

#[cfg(test)]
mod tests {
    use crate::feat::harness::HarnessId;
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    use crate::command::Workspace;
    use crate::feat::git::git_service::{GitRef, WorktreeFacts};
    use crate::feat::jumps::state::JumpList;
    use crate::feat::layout::state::{Layouts, SessionLayout, Tab, test_entry};
    use crate::feat::layout::tree::{NavDirection, Split};
    use crate::feat::picker::list::{PickerItem, WorkspaceChoice};
    use crate::feat::picker::state::{PickTarget, PickerKind, PickerState};
    use crate::feat::sessions::state::{
        FolderKind, PaneId, PaneLaunch, Project, ProjectId, ProjectKind, Search, Session,
        SessionId, SessionKind, Sessions, SidebarItem, SidebarRow, Thread, ThreadId, ThreadStatus,
        sessions_for,
    };
    use crate::feat::sessions::validator::SETTLE_IN_PROGRESS;
    use crate::feat::sidebar::state::{Rename, RenameTarget, SidebarView};
    use crate::feat::worktrees::state::{Worktree, Worktrees};
    use crate::feat::zellij::zellij_service::Tool;
    use crate::{AppState, Command, Focus, Intent, IntentHandler, TextInput};
    use ratatui::layout::Rect;

    /// Claude's setting row for `value`.
    /// `sessions` with the sessions its projects' threads run in.
    fn fill(sessions: Sessions) -> Sessions {
        Sessions {
            sessions: sessions_for(&sessions.projects),
            ..sessions
        }
    }

    /// Thread `id`'s pane: pane `id` of session `id`.
    fn launch(id: i64) -> PaneLaunch {
        PaneLaunch {
            pane: PaneId(id),
            session: SessionId(id),
        }
    }

    fn thread(id: i64, status: ThreadStatus) -> Thread {
        Thread {
            last_session: None,
            harness: HarnessId::new("claude"),
            id: ThreadId(id),
            title: None,
            cwd: format!("/work/{id}").into(),
            transcript: None,
            status,
            turn_started_at: None,
            pane: Some(launch(id)),
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
    /// Each thread with a pane runs in session `<id>`, a tab of that one pane.
    fn state_at(threads: Vec<Thread>, cursor: SidebarItem) -> AppState {
        let mut layouts = Layouts::default();
        for pane in threads.iter().filter_map(|thread| thread.pane.as_ref()) {
            layouts.insert(
                SessionId(pane.pane.0),
                SessionLayout::of(test_entry(pane.pane.0)),
            );
        }
        let projects = vec![Project {
            id: ProjectId(1),
            title: "work".into(),
            root: "/work".into(),
            created_at: SystemTime::UNIX_EPOCH,
            removed: false,
            repo: true,
            threads,
            kind: ProjectKind::Normal,
        }];
        AppState {
            layouts,
            sessions: Sessions {
                sessions: sessions_for(&projects),
                projects,
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
            sessions: fill(Sessions {
                projects: (1..)
                    .zip(titles)
                    .map(|(id, title)| Project {
                        id: ProjectId(id),
                        title: (*title).to_owned(),
                        root: format!("/{title}").into(),
                        created_at: SystemTime::UNIX_EPOCH,
                        removed: false,
                        repo: true,
                        threads: vec![],
                        kind: ProjectKind::Normal,
                    })
                    .collect(),
                ..Sessions::default()
            }),
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
        state_at(threads, SidebarItem::Session(SessionId(selected)))
    }

    /// Thread `id`, idle and prompt-less, in the project's root `/work`.
    fn in_root(id: i64) -> Thread {
        Thread {
            cwd: "/work".into(),
            ..thread(id, ThreadStatus::Idle)
        }
    }

    /// The workspace picker opened on session 1 of `threads`.
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
            state.sessions.selected_thread().map(|thread| thread.id),
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
            state.sessions.selected_thread().map(|thread| thread.id),
            Some(ThreadId(1)),
            "SelectPrev should wrap to the last thread"
        );
    }

    #[rstest::rstest]
    #[case(Intent::FocusRight, Focus::Sidebar, Focus::Sidebar)]
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
    fn hiding_the_sidebar_without_a_layout_keeps_the_keys(#[case] focus: Focus) {
        // Given a shown sidebar, with `focus` focused and no layout shown.
        let mut state = laid_out(focus, 32, false);

        // When handling ToggleSidebar.
        IntentHandler::handle(&Intent::ToggleSidebar, &mut state);

        // Then the keys stay where they were.
        assert_eq!(
            state.focus, focus,
            "hiding from {focus:?} with nothing shown should keep the keys"
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
                Command::Attach(SessionId(1)),
                Command::RefreshSessions,
                Command::SaveJumps,
            ],
            "Attach should target the selected thread, then refresh"
        );
    }

    #[rstest::rstest]
    fn enter_on_a_settled_session_unsettles_and_attaches_it() {
        // Given a selected settled session.
        let mut state = state_with(vec![settled(1)], 1);

        // When handling Attach.
        let commands = IntentHandler::handle(&Intent::Attach, &mut state);

        // Then it is un-settled, then attached.
        assert_eq!(
            commands.get(..2),
            Some(
                &[
                    Command::UnsettleSession(SessionId(1)),
                    Command::Attach(SessionId(1))
                ][..]
            ),
            "⏎ on a settled session should bring it back and attach"
        );
    }

    /// Keys going to thread 1's attached session.
    fn attached() -> AppState {
        AppState {
            focus: Focus::Attached,
            attached: HashSet::from([SessionId(1)]),
            ..state_with(vec![thread(1, ThreadStatus::Idle)], 1)
        }
    }

    /// Threads 1 and 2 in the sidebar with those in `attached` attached and
    /// thread `selected` selected.
    fn left_pane(attached: &[i64], selected: i64) -> AppState {
        AppState {
            focus: Focus::Sidebar,
            attached: attached.iter().copied().map(SessionId).collect(),
            ..state_with(
                vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
                selected,
            )
        }
    }

    #[rstest::rstest]
    fn attach_shows_the_threads_session() {
        // Given a selected idle thread running in session 1.
        let mut state = state_with(vec![thread(1, ThreadStatus::Idle)], 1);

        // When handling Attach.
        IntentHandler::handle(&Intent::Attach, &mut state);

        // Then session 1 is shown.
        assert_eq!(
            state.shown_session(),
            Some(SessionId(1)),
            "attaching should show the thread's session"
        );
    }

    #[rstest::rstest]
    fn attach_adds_the_thread_to_attached() {
        // Given a selected idle thread.
        let mut state = state_with(vec![thread(1, ThreadStatus::Idle)], 1);

        // When handling Attach.
        IntentHandler::handle(&Intent::Attach, &mut state);

        // Then the thread is attached.
        assert!(
            state.attached.contains(&SessionId(1)),
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
            state.attached.contains(&SessionId(1)),
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
            !state.attached.contains(&SessionId(1)),
            "Detach should remove the selected thread from attached"
        );
    }

    #[rstest::rstest]
    fn detach_focuses_the_sidebar() {
        // Given keys going to thread 1's attached session.
        let mut state = attached();

        // When handling Detach.
        IntentHandler::handle(&Intent::Detach, &mut state);

        // Then keys drive the sidebar.
        assert_eq!(
            state.focus,
            Focus::Sidebar,
            "Detach should return to the sidebar"
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
            !state.attached.contains(&SessionId(1)),
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
            state.attached.contains(&SessionId(1)),
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
                Command::Attach(SessionId(2)),
                Command::RefreshSessions,
                Command::SaveJumps,
            ],
            "FocusRight should attach to the selected thread, then refresh"
        );
    }

    #[rstest::rstest]
    #[case(&[])]
    #[case(&[1])]
    fn focus_right_on_an_unattached_thread_keeps_the_keys(#[case] attached: &[i64]) {
        // Given thread 2 selected and unattached, with no thread or thread 1
        // attached.
        let mut state = left_pane(attached, 2);
        let before = state.focus;

        // When handling FocusRight.
        IntentHandler::handle(&Intent::FocusRight, &mut state);

        // Then the keys stay put.
        assert_eq!(
            state.focus, before,
            "FocusRight with {attached:?} attached should keep the keys"
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
    fn settle_confirm_detaches_the_session() {
        // Given attached thread 1 selected in the sidebar, and Yes highlighted in its settle
        // confirm.
        let mut state = left_pane(&[1], 1);
        answer_yes(&Intent::ToggleSettle, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then thread 1 is no longer attached.
        assert!(
            !state.attached.contains(&SessionId(1)),
            "settling should remove the thread from attached"
        );
    }

    #[rstest::rstest]
    fn delete_removes_the_thread_from_attached() {
        // Given attached thread 1 selected in the sidebar, and Yes highlighted in its delete
        // confirm.
        let mut state = left_pane(&[1], 1);
        answer_yes(&Intent::Delete, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then thread 1 is no longer attached.
        assert!(
            !state.attached.contains(&SessionId(1)),
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
            "a project can be picked while another session starts"
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

    /// Session `id` of project `project` in `dir`, on no branch.
    fn session_in(id: i64, project: i64, dir: &str) -> Session {
        Session {
            id: SessionId(id),
            project: ProjectId(project),
            kind: SessionKind::Plain,
            dir: dir.into(),
            name: None,
            branch: None,
            created_at: SystemTime::UNIX_EPOCH,
            pinned_at: None,
            settled_at: None,
            active_since: SystemTime::UNIX_EPOCH,
            last_activity_at: SystemTime::UNIX_EPOCH,
        }
    }

    /// The project picker opened from the sidebar with beta, outside git,
    /// highlighted.
    fn picking_non_git() -> AppState {
        let mut state = picking(Focus::Sidebar);
        if let Some(beta) = state.sessions.projects.get_mut(1) {
            beta.repo = false;
        }
        highlight(&mut state, &project_row(2, "beta"));
        state
    }

    /// Alpha's new-session workspace picker, alpha holding a session in
    /// `/wt/alpha-1`, with `choice` highlighted.
    fn choosing_new_workspace(choice: &WorkspaceChoice) -> AppState {
        let mut state = picking(Focus::Sidebar);
        state.sessions.sessions = vec![session_in(5, 1, "/wt/alpha-1")];
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);
        highlight(&mut state, &PickerItem::Workspace(choice.clone()));
        state
    }

    /// Alpha's base branch picker for a new worktree, listing `main`.
    fn choosing_base() -> AppState {
        let mut state = choosing_new_workspace(&WorkspaceChoice::NewWorktree);
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);
        if let Some(picker) = &mut state.picker {
            picker.show_branches(
                Path::new("/alpha"),
                vec![branch("main", true, Some("/alpha"))],
            );
        }
        state
    }

    #[rstest::rstest]
    fn picking_a_git_project_opens_the_workspace_picker() {
        // Given the project picker with alpha, a git project, highlighted.
        let mut state = picking(Focus::Sidebar);

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then alpha's new-session workspace picker is open.
        assert_eq!(
            state.picker.as_ref().map(PickerState::kind),
            Some(&PickerKind::Workspace {
                target: PickTarget::New(ProjectId(1))
            }),
            "a git project goes on to the workspace picker"
        );
    }

    #[rstest::rstest]
    fn picking_a_project_outside_git_returns_new_session_in_its_checkout() {
        // Given the project picker with beta, outside git, highlighted.
        let mut state = picking_non_git();

        // When handling PickerConfirm.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then a session in beta's checkout is asked for at once.
        assert_eq!(
            commands,
            vec![
                Command::NewSession {
                    project: ProjectId(2),
                    workspace: Workspace::Checkout,
                },
                Command::SaveJumps,
            ],
            "a project outside git gets its session in its checkout"
        );
    }

    #[rstest::rstest]
    fn picking_current_checkout_returns_new_session_in_the_checkout() {
        // Given alpha's workspace picker on the current checkout.
        let mut state = choosing_new_workspace(&WorkspaceChoice::Current { worktree: false });

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then a session in alpha's checkout is asked for.
        assert!(
            commands.contains(&Command::NewSession {
                project: ProjectId(1),
                workspace: Workspace::Checkout,
            }),
            "the current checkout should make a session there, got {commands:?}"
        );
    }

    #[rstest::rstest]
    fn picking_previous_worktree_returns_new_session_in_it() {
        // Given alpha's workspace picker on its previous worktree.
        let mut state = choosing_new_workspace(&WorkspaceChoice::Previous {
            path: "/wt/alpha-1".into(),
            branch: None,
        });

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then a session in that worktree is asked for.
        assert!(
            commands.contains(&Command::NewSession {
                project: ProjectId(1),
                workspace: Workspace::Existing("/wt/alpha-1".into()),
            }),
            "the previous worktree should make a session there, got {commands:?}"
        );
    }

    #[rstest::rstest]
    fn picking_new_worktree_opens_the_base_branch_picker() {
        // Given alpha's workspace picker on New worktree.
        let mut state = choosing_new_workspace(&WorkspaceChoice::NewWorktree);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the base branch picker for alpha's root is open.
        assert_eq!(
            state.picker.as_ref().map(PickerState::kind),
            Some(&PickerKind::Base {
                target: PickTarget::New(ProjectId(1)),
                root: "/alpha".into(),
            }),
            "a new worktree asks for its base branch"
        );
    }

    #[rstest::rstest]
    fn picking_new_worktree_returns_list_branches_for_the_root() {
        // Given alpha's workspace picker on New worktree.
        let mut state = choosing_new_workspace(&WorkspaceChoice::NewWorktree);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the root's refs are listed.
        assert_eq!(
            commands,
            vec![Command::ListBranches("/alpha".into())],
            "the base picker lists the project root's refs"
        );
    }

    #[rstest::rstest]
    fn picking_a_base_returns_new_session_in_a_new_worktree() {
        // Given alpha's base picker with `main` highlighted.
        let mut state = choosing_base();

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then a session in a new worktree from `main` is asked for.
        assert!(
            commands.contains(&Command::NewSession {
                project: ProjectId(1),
                workspace: Workspace::NewWorktree {
                    base: "main".to_owned()
                },
            }),
            "a picked base should make a session in a new worktree, got {commands:?}"
        );
    }

    #[rstest::rstest]
    fn new_session_pick_marks_starting() {
        // Given the project picker with beta, outside git, highlighted.
        let mut state = picking_non_git();

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then a start is in flight.
        assert!(
            state.sessions.starting,
            "a new session should mark a start in flight"
        );
    }

    #[rstest::rstest]
    fn new_session_pick_while_starting_returns_nothing() {
        // Given the project picker with beta, outside git, highlighted, while
        // a start is in flight.
        let mut state = picking_non_git();
        state.sessions.starting = true;

        // When handling PickerConfirm.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then nothing is asked for.
        assert!(commands.is_empty(), "one new session at a time");
    }

    #[rstest::rstest]
    fn picking_a_project_outside_the_filter_clears_it() {
        // Given the sidebar filtered to alpha and the project picker with
        // beta, outside git, highlighted.
        let mut state = picking_non_git();
        state.sessions.filter = Some(ProjectId(1));

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the sidebar lists every project again.
        assert_eq!(
            state.sessions.filter, None,
            "a new session outside the filter clears it"
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
    fn change_workspace_on_a_session_opens_the_workspace_picker() {
        // Given a selected session whose thread has had no turn.
        let mut state = state_with(vec![in_root(1)], 1);

        // When handling ChangeWorkspace.
        IntentHandler::handle(&Intent::ChangeWorkspace, &mut state);

        // Then the session's workspace picker is open and takes the keys.
        assert_eq!(
            (state.focus, state.picker.as_ref().map(PickerState::kind)),
            (
                Focus::Picker,
                Some(&PickerKind::Workspace {
                    target: PickTarget::Move(SessionId(1))
                })
            ),
            "ChangeWorkspace should open the workspace picker"
        );
    }

    /// [`state_with`] thread 1 in the root, selected, its project outside
    /// git.
    fn outside_git() -> AppState {
        let mut state = state_with(vec![in_root(1)], 1);
        for project in &mut state.sessions.projects {
            project.repo = false;
        }
        state
    }

    #[rstest::rstest]
    fn change_workspace_outside_git_offers_init_git() {
        // Given a selected session of a project outside git.
        let mut state = outside_git();

        // When handling ChangeWorkspace.
        IntentHandler::handle(&Intent::ChangeWorkspace, &mut state);

        // Then the picker offers to make the project a git repository.
        assert_eq!(
            state.picker.as_ref().map(PickerState::kind),
            Some(&PickerKind::InitGit {
                project: ProjectId(1)
            }),
            "a project outside git has no worktrees to pick"
        );
    }

    #[rstest::rstest]
    fn switch_branch_outside_git_offers_init_git() {
        // Given a selected session of a project outside git.
        let mut state = outside_git();

        // When handling SwitchBranch.
        IntentHandler::handle(&Intent::SwitchBranch, &mut state);

        // Then the picker offers to make the project a git repository.
        assert_eq!(
            state.picker.as_ref().map(PickerState::kind),
            Some(&PickerKind::InitGit {
                project: ProjectId(1)
            }),
            "a project outside git has no branches to pick"
        );
    }

    #[rstest::rstest]
    fn locked_workspace_shows_the_lock_message() {
        // Given a selected session in the root checkout whose thread is
        // titled, so a turn has run.
        let mut state = state_with(
            vec![Thread {
                title: Some("fix the tests".into()),
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
            "a session that had a turn should show the lock"
        );
    }

    #[rstest::rstest]
    fn workspace_picker_offers_previous_worktree_with_its_branch() {
        // Given another session of the project in a worktree on `orb/feat`.
        let mut state = AppState {
            focus: Focus::Dashboard,
            ..state_with(vec![in_root(1), thread(2, ThreadStatus::Idle)], 1)
        };
        for session in &mut state.sessions.sessions {
            if session.id == SessionId(2) {
                session.branch = Some("orb/feat".into());
            }
        }

        // When opening the workspace picker on a root thread.
        IntentHandler::handle(&Intent::ChangeWorkspace, &mut state);

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
    fn picking_new_worktree_for_a_session_opens_the_base_picker() {
        // Given session 1's workspace picker with `New worktree` highlighted.
        let mut state = choosing_workspace(vec![in_root(1)]);
        IntentHandler::handle(&Intent::PickerNext, &mut state);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the base picker lists the project root's refs.
        assert_eq!(
            (state.picker.as_ref().map(PickerState::kind), commands),
            (
                Some(&PickerKind::Base {
                    target: PickTarget::Move(SessionId(1)),
                    root: "/work".into(),
                }),
                vec![Command::ListBranches("/work".into())],
            ),
            "a new worktree needs its base branch first"
        );
    }

    /// Session 1's workspace picker in the root, with session 2's worktree
    /// `/work/2` as the previous worktree, highlighted.
    fn choosing_previous_worktree() -> AppState {
        let mut state = AppState {
            attached: [SessionId(1)].into(),
            focus: Focus::Attached,
            ..state_with(vec![in_root(1), thread(2, ThreadStatus::Idle)], 1)
        };
        IntentHandler::handle(&Intent::ChangeWorkspace, &mut state);
        IntentHandler::handle(&Intent::PickerNext, &mut state);
        IntentHandler::handle(&Intent::PickerNext, &mut state);
        state
    }

    #[rstest::rstest]
    fn picking_a_previous_worktree_for_a_session_returns_change_workspace() {
        // Given session 1's workspace picker with the previous worktree
        // highlighted.
        let mut state = choosing_previous_worktree();

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then session 1 moves into that worktree.
        assert_eq!(
            commands,
            [Command::ChangeWorkspace {
                session: SessionId(1),
                to: Workspace::Existing("/work/2".into()),
            }],
            "the previous worktree should be the session's new workspace"
        );
    }

    #[rstest::rstest]
    fn picking_a_workspace_for_a_session_detaches_it() {
        // Given attached session 1's workspace picker with the previous
        // worktree highlighted.
        let mut state = choosing_previous_worktree();

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then it is detached, so its pane clients go before its panes are
        // killed, and the keys are in the sidebar.
        assert_eq!(
            (state.attached.contains(&SessionId(1)), state.focus),
            (false, Focus::Sidebar),
            "a moving session should be detached"
        );
    }

    #[rstest::rstest]
    fn picking_a_workspace_for_a_session_marks_starting() {
        // Given session 1's workspace picker with the previous worktree
        // highlighted.
        let mut state = choosing_previous_worktree();

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then a start is in flight.
        assert!(
            state.sessions.starting,
            "a move should mark the start in flight"
        );
    }

    #[rstest::rstest]
    fn picking_a_base_for_a_session_returns_change_workspace() {
        // Given session 1's base picker listing `main`.
        let mut state = choosing_workspace(vec![in_root(1)]);
        IntentHandler::handle(&Intent::PickerNext, &mut state);
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);
        if let Some(picker) = &mut state.picker {
            picker.show_branches(Path::new("/work"), vec![branch("main", true, None)]);
        }

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then session 1 moves into a new worktree from main.
        assert_eq!(
            commands,
            [Command::ChangeWorkspace {
                session: SessionId(1),
                to: Workspace::NewWorktree {
                    base: "main".into()
                },
            }],
            "the base should start the session's new worktree"
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

    /// The branch picker opened on session 1 of `threads`, showing `refs`.
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
                    session: SessionId(1),
                    cwd: "/work".into(),
                    unstarted: true,
                })
            ),
            "SwitchBranch should open the branch picker"
        );
    }

    #[rstest::rstest]
    fn switch_branch_on_a_session_lists_branches_in_its_dir() {
        // Given a selected session in the root.
        let mut state = state_with(vec![in_root(1)], 1);

        // When handling SwitchBranch.
        let commands = IntentHandler::handle(&Intent::SwitchBranch, &mut state);

        // Then the root's refs are listed.
        assert_eq!(
            commands,
            [Command::ListBranches("/work".into())],
            "the picker's refs come from the session's directory"
        );
    }

    #[rstest::rstest]
    fn next_intent_clears_the_error() {
        // Given a failure on the mode line.
        let mut state = state_with(vec![in_root(1)], 1);
        state.sessions.error = Some("A session is working in this directory".to_owned());

        // When handling the next intent.
        IntentHandler::handle(&Intent::SelectNext, &mut state);

        // Then the failure is gone.
        assert_eq!(
            state.sessions.error, None,
            "the user has seen the error once they press a key"
        );
    }

    #[rstest::rstest]
    fn next_intent_clears_the_notice() {
        // Given a worktree notice on the mode line.
        let mut state = state_with(vec![in_root(1)], 1);
        state.worktrees.notice = Some("pruned 2 worktrees".to_owned());

        // When handling the next intent.
        IntentHandler::handle(&Intent::SelectNext, &mut state);

        // Then the notice is gone.
        assert_eq!(
            state.worktrees.notice, None,
            "the user has seen the notice once they press a key"
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
            Some("A session is working in this directory"),
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

        // Then feat is checked out in the session's directory.
        assert_eq!(
            commands,
            [Command::SwitchBranch {
                session: SessionId(1),
                git_ref: branch("feat", false, None),
                to_root: false,
            }],
            "a free branch should be checked out"
        );
    }

    #[rstest::rstest]
    fn unstarted_pick_of_a_branch_in_another_worktree_returns_change_workspace() {
        // Given the branch picker of a session with no turn, with `feat`,
        // checked out in another worktree, highlighted.
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

        // Then the session moves into that worktree.
        assert_eq!(
            commands,
            [Command::ChangeWorkspace {
                session: SessionId(1),
                to: Workspace::Existing("/wt/feat".into()),
            }],
            "a session with no turn follows the branch into its worktree"
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
            (true, Some(SidebarItem::Session(SessionId(1)))),
            "h on a card should do nothing"
        );
    }

    #[rstest::rstest]
    fn settle_confirm_returns_settle_session() {
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
            commands.contains(&Command::SettleSession(SessionId(2))),
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
            Some(SidebarItem::Session(SessionId(1))),
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
            Some(SidebarItem::Session(SessionId(2))),
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
            vec![Command::UnsettleSession(SessionId(1))],
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
            Some(SidebarItem::Session(SessionId(1))),
            "un-settle should keep the cursor on the thread"
        );
    }

    #[rstest::rstest]
    fn toggle_pin_on_a_session_returns_pin_session() {
        // Given a selected unpinned session.
        let mut state = state_with(vec![thread(1, ThreadStatus::Idle)], 1);

        // When handling TogglePin.
        let commands = IntentHandler::handle(&Intent::TogglePin, &mut state);

        // Then the sessions actor is asked to pin it.
        assert_eq!(
            commands,
            vec![Command::PinSession(SessionId(1))],
            "p on an unpinned session pins it"
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
            vec![Command::UnpinSession(SessionId(1))],
            "TogglePin on a pinned thread should return Unpin"
        );
    }

    #[rstest::rstest]
    fn delete_confirm_returns_delete_session() {
        // Given one thread, selected, and Yes highlighted in its delete confirm.
        let mut state = state_with(vec![thread(1, ThreadStatus::Idle)], 1);
        answer_yes(&Intent::Delete, &mut state);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the sessions actor is asked to delete it.
        assert!(
            commands.contains(&Command::DeleteSession(SessionId(1))),
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
        answer_yes(&Intent::Delete, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the settled thread below, past the header, is selected.
        assert_eq!(
            state.sessions.cursor,
            Some(SidebarItem::Session(SessionId(3))),
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
            commands.contains(&Command::Visit(SessionId(expected))),
            "{intent:?} should visit thread {expected}"
        );
    }

    #[rstest::rstest]
    fn delete_confirm_hides_the_session() {
        // Given threads 2 and 1, with thread 1 selected, and Yes highlighted in its delete confirm.
        let mut state = state_with(
            vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
            1,
        );
        answer_yes(&Intent::Delete, &mut state);

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
            vec![SidebarItem::Session(SessionId(2))],
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
            commands.contains(&Command::Visit(SessionId(1))),
            "SelectNext should visit the thread it lands on"
        );
    }

    #[rstest::rstest]
    fn confirming_initialize_git_returns_init_git() {
        // Given project 1's Initialize Git picker.
        let mut state = AppState {
            picker: Some(PickerState::init_git(ProjectId(1), Focus::Sidebar)),
            focus: Focus::Picker,
            ..AppState::default()
        };

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the sessions actor is asked to run git init.
        assert_eq!(
            commands,
            vec![Command::InitGit(ProjectId(1))],
            "⏎ on Initialize Git should init the project"
        );
    }

    /// Gives every project `threads`, as a poll would.
    fn poll(state: &mut AppState, threads: &[Thread]) {
        for project in &mut state.sessions.projects {
            project.threads = threads.to_vec();
        }
        state.sessions.sessions = sessions_for(&state.sessions.projects);
    }

    /// The open picker's kind and selected item.
    fn open_confirm(state: &AppState) -> Option<(&PickerKind, Option<&PickerItem>)> {
        state
            .picker
            .as_ref()
            .map(|picker| (picker.kind(), picker.selected()))
    }

    #[rstest::rstest]
    fn toggle_settle_on_a_session_opens_its_confirm() {
        // Given a selected idle thread.
        let mut state = state_with(vec![thread(1, ThreadStatus::Idle)], 1);

        // When handling ToggleSettle.
        IntentHandler::handle(&Intent::ToggleSettle, &mut state);

        // Then its settle confirm is open with No selected.
        assert_eq!(
            open_confirm(&state),
            Some((
                &PickerKind::SettleSession {
                    session: SessionId(1)
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
    fn settle_is_refused_while_an_agent_works() {
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
        IntentHandler::handle(&Intent::Delete, &mut state);

        // Then its delete confirm is open with No selected.
        assert_eq!(
            open_confirm(&state),
            Some((
                &PickerKind::DeleteSession {
                    session: SessionId(1),
                    folder: false
                },
                Some(&PickerItem::Confirm(false))
            )),
            "d should ask to delete the thread"
        );
    }

    #[rstest::rstest]
    #[case(Intent::ToggleSettle, state_with(vec![thread(1, ThreadStatus::Idle)], 1))]
    #[case(Intent::Delete, state_with(vec![thread(1, ThreadStatus::Idle)], 1))]
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
        answer_yes(&Intent::Delete, &mut state);
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
    fn open_tool_without_a_selection_returns_no_commands() {
        // Given nothing selected.
        let mut state = AppState::default();

        // When handling OpenTool.
        let commands = IntentHandler::handle(&Intent::OpenTool(Tool::Shell), &mut state);

        // Then nothing opens.
        assert!(commands.is_empty(), "a tool needs a selected session");
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
        state.sessions.sessions = sessions_for(&state.sessions.projects);
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

    /// [`filtering`], then `<C-x>` on alpha.
    fn removing_alpha(filter: Option<i64>, cursor: SidebarItem) -> AppState {
        let mut state = filtering(filter, cursor);
        highlight(&mut state, &project_row(1, "alpha"));
        IntentHandler::handle(&Intent::PickerRemove, &mut state);
        state
    }

    fn on_thread(id: i64) -> SidebarItem {
        SidebarItem::Session(SessionId(id))
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

    /// Alpha (1) and orb's Incognito project (2), nothing selected.
    fn incognito() -> AppState {
        let mut state = with_projects(&["alpha", "incognito"]);
        for project in state.sessions.projects.iter_mut().skip(1) {
            project.kind = ProjectKind::Incognito;
            project.repo = false;
        }
        state
    }

    #[rstest::rstest]
    fn new_incognito_returns_new_session_in_the_incognito_checkout() {
        // Given the Incognito project.
        let mut state = incognito();

        // When handling NewIncognito.
        let commands = IntentHandler::handle(&Intent::NewIncognito, &mut state);

        // Then a session in the Incognito folder is asked for.
        assert!(
            commands.contains(&Command::NewSession {
                project: ProjectId(2),
                workspace: Workspace::Checkout,
            }),
            "NewIncognito should ask for a session in the Incognito folder, got {commands:?}"
        );
    }

    #[rstest::rstest]
    fn new_incognito_marks_starting() {
        // Given the Incognito project and no start in flight.
        let mut state = incognito();

        // When handling NewIncognito.
        IntentHandler::handle(&Intent::NewIncognito, &mut state);

        // Then a start is in flight.
        assert!(
            state.sessions.starting,
            "a new Incognito session should mark a start in flight"
        );
    }

    #[rstest::rstest]
    fn new_incognito_while_starting_returns_nothing() {
        // Given the Incognito project while a start is in flight.
        let mut state = incognito();
        state.sessions.starting = true;

        // When handling NewIncognito.
        let commands = IntentHandler::handle(&Intent::NewIncognito, &mut state);

        // Then nothing is asked for.
        assert!(commands.is_empty(), "one new session at a time");
    }

    #[rstest::rstest]
    fn new_incognito_clears_a_filter_to_another_project() {
        // Given the sidebar filtered to alpha.
        let mut state = incognito();
        state.sessions.filter = Some(ProjectId(1));

        // When handling NewIncognito.
        IntentHandler::handle(&Intent::NewIncognito, &mut state);

        // Then the filter is cleared.
        assert_eq!(
            state.sessions.filter, None,
            "a filter hiding the new Incognito session should be cleared"
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
            vec![Command::SaveUi, Command::Visit(SessionId(21))],
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
                Command::Visit(SessionId(21)),
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
            "picking another project should clear the filter"
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

        // Then the cleared filter is saved.
        assert_eq!(
            commands,
            vec![Command::SaveUi],
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
            (Some(ProjectId(1)), vec![]),
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
                target: RenameTarget::Session(SessionId(1)),
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
    fn rename_opens_the_box_with_the_session_title() {
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
    fn rename_on_an_untitled_session_holds_its_folder_name() {
        // Given a selected session in /work/1 whose agent has no title yet.
        let mut state = titled(None);

        // When handling Rename.
        IntentHandler::handle(&Intent::Rename, &mut state);

        // Then the rename box holds the folder's name, the session's title.
        assert_eq!(
            rename_text(&state),
            Some("1"),
            "an untitled session's rename box should hold its title"
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
    fn rename_confirm_returns_rename_session() {
        // Given the rename box holding " Sidebar search ".
        let mut state = renaming(" Sidebar search ");

        // When handling PickerConfirm.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the thread is renamed to the trimmed text.
        assert_eq!(
            commands,
            vec![Command::RenameSession {
                session: SessionId(1),
                name: Some("Sidebar search".to_owned()),
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
            vec![Command::RenameSession {
                session: SessionId(1),
                name: None,
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
            return_to: Some(SidebarItem::Session(SessionId(2))),
        });
        state.sessions.cursor = cursor.map(|id| SidebarItem::Session(SessionId(id)));
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
            Some(SidebarItem::Session(SessionId(3))),
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
            vec![Command::Visit(SessionId(3))],
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
            Some(SidebarItem::Session(SessionId(1))),
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
            Some(SidebarItem::Session(SessionId(3))),
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
            (
                true,
                Focus::Sidebar,
                Some(SidebarItem::Session(SessionId(3)))
            ),
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
            (true, Some(SidebarItem::Session(SessionId(2)))),
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
            (
                true,
                Focus::Sidebar,
                Some(SidebarItem::Session(SessionId(2)))
            ),
            "Esc should end the search where it began"
        );
    }

    #[rstest::rstest]
    fn new_folder_opens_the_empty_name_box() {
        // Given no projects.
        let mut state = AppState::default();

        // When handling NewFolder(Research).
        IntentHandler::handle(&Intent::NewFolder(FolderKind::Research), &mut state);

        // Then an empty name box names a Research session, with the keys.
        assert_eq!(
            (
                state.rename.as_ref().map(|rename| rename.target),
                rename_text(&state),
                state.focus
            ),
            (
                Some(RenameTarget::NewFolder(FolderKind::Research)),
                Some(""),
                Focus::Rename
            ),
            "␣gr should open the name box"
        );
    }

    /// The name box for a new Research session holding `text`.
    fn naming_research(text: &str) -> AppState {
        AppState {
            focus: Focus::Rename,
            rename: Some(Rename {
                target: RenameTarget::NewFolder(FolderKind::Research),
                input: TextInput::new(text),
                creating: false,
            }),
            ..AppState::default()
        }
    }

    #[rstest::rstest]
    fn confirming_a_research_name_returns_new_folder_session() {
        // Given the name box holding `GT-514 login`.
        let mut state = naming_research("GT-514 login");

        // When handling PickerConfirm.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then a Research session is asked for under the case-kept slug.
        assert!(
            commands.contains(&Command::NewFolderSession {
                kind: FolderKind::Research,
                name: "GT-514-login".into(),
            }),
            "a valid name should ask for the session, got {commands:?}"
        );
    }

    #[rstest::rstest]
    fn confirming_a_folder_name_marks_starting() {
        // Given the name box holding a fresh name.
        let mut state = naming_research("tokio select");

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then a start is in flight.
        assert!(
            state.sessions.starting,
            "a new Research session should mark a start in flight"
        );
    }

    #[rstest::rstest]
    fn confirming_a_folder_name_keeps_the_box_open_until_the_session_is_made() {
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
    fn confirming_a_folder_name_again_while_it_is_made_asks_for_nothing() {
        // Given the name box already confirmed with a fresh name.
        let mut state = naming_research("tokio select");
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // When handling PickerConfirm again.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then no second session is asked for.
        assert_eq!(commands, vec![], "a second ⏎ should send nothing");
    }

    #[rstest::rstest]
    fn confirming_an_invalid_folder_name_shows_the_char() {
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
    fn confirming_an_empty_folder_name_does_nothing() {
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

    /// Threads 1 and 2 (listed 2, 1) with the cursor on thread 2, the keys
    /// in `focus`, the threads in `attached` attached, and `jumps` listed,
    /// oldest first.
    fn jumping(focus: Focus, attached: &[i64], jumps: &[SidebarItem]) -> AppState {
        AppState {
            focus,
            attached: attached.iter().copied().map(SessionId).collect(),
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
    fn new_session_pick_records_the_row_it_leaves() {
        // Given the project picker opened from alpha's thread 11, on beta,
        // which is outside git.
        let mut state = with_projects(&["alpha", "beta"]);
        if let Some(project) = state.sessions.projects.first_mut() {
            project.threads = vec![thread(11, ThreadStatus::Idle)];
        }
        if let Some(beta) = state.sessions.projects.get_mut(1) {
            beta.repo = false;
        }
        state.sessions.cursor = Some(on_thread(11));
        IntentHandler::handle(&Intent::NewSession, &mut state);
        highlight(&mut state, &project_row(2, "beta"));

        // When picking beta.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then thread 11 is recorded; the new session's row follows when the
        // frontend attaches it.
        assert_eq!(
            state.jumps.entries(),
            [on_thread(11)],
            "a ␣n pick should record where it left"
        );
    }

    #[rstest::rstest]
    fn jump_back_lands_on_a_session() {
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
            (
                state.focus,
                state.sessions.selected_thread().map(|thread| thread.id)
            ),
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
            commands.contains(&Command::Attach(SessionId(1))),
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
            vec![Command::SaveJumps, Command::Visit(SessionId(1))],
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
    fn jump_back_from_a_pane_with_the_sidebar_hidden_focuses_the_hidden_sidebar() {
        // Given the keys in thread 2's pane with the sidebar hidden, and
        // thread 1 listed and unattached.
        let mut state = jumping(Focus::Attached, &[2], &[on_thread(1), on_thread(2)]);
        state.sidebar.hidden = true;

        // When handling JumpBack.
        IntentHandler::handle(&Intent::JumpBack, &mut state);

        // Then the hidden sidebar has the keys.
        assert_eq!(
            state.focus,
            Focus::Sidebar,
            "with the sidebar hidden, <C-o> out of a pane goes to the sidebar"
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
        state.sessions.sessions = sessions_for(&state.sessions.projects);
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
    fn jump_back_after_a_delete_skips_the_deleted_thread() {
        // Given threads 3, 2 and 1 listed, and 1, 3, 2 in the jump list, with
        // thread 2 deleted from the sidebar (the cursor moves to thread 1).
        let mut state = state_with(
            (1..=3).map(|id| thread(id, ThreadStatus::Idle)).collect(),
            2,
        );
        state.jumps = JumpList::from_saved(vec![on_thread(1), on_thread(3), on_thread(2)]);
        answer_yes(&Intent::Delete, &mut state);
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
        answer_yes(&Intent::Delete, &mut state);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then only thread 1 is left.
        assert_eq!(
            state.jumps.entries(),
            [on_thread(1)],
            "deleting should drop the thread from the jump list"
        );
    }

    /// `state` with the session picker opened from its focus.
    fn open_sessions(mut state: AppState) -> AppState {
        IntentHandler::handle(&Intent::OpenSessionPicker, &mut state);
        state
    }

    /// The session ids the open picker shows, in order.
    fn session_ids(state: &AppState) -> Vec<SessionId> {
        state
            .picker
            .iter()
            .flat_map(PickerState::shown)
            .filter_map(|(item, _)| match item {
                PickerItem::Session { id, .. } => Some(*id),
                _ => None,
            })
            .collect()
    }

    /// The labels the open picker shows, in order.
    fn session_labels(state: &AppState) -> Vec<String> {
        state
            .picker
            .iter()
            .flat_map(PickerState::shown)
            .filter_map(|(item, _)| match item {
                PickerItem::Session { label, .. } => Some(label.clone()),
                _ => None,
            })
            .collect()
    }

    /// `thread` titled `title`.
    fn named(thread: Thread, title: &str) -> Thread {
        Thread {
            title: Some(title.into()),
            ..thread
        }
    }

    /// Thread `id`, idle, whose last turn ended at second `secs`.
    fn ended_at(id: i64, secs: u64) -> Thread {
        Thread {
            last_activity_at: at(secs),
            ..thread(id, ThreadStatus::Idle)
        }
    }

    /// One project per title, holding idle thread 11, 21, … in turn, with no
    /// row selected.
    fn with_project_threads(titles: &[&str]) -> AppState {
        let mut state = with_projects(titles);
        for (project, id) in state.sessions.projects.iter_mut().zip((11..).step_by(10)) {
            project.threads = vec![thread(id, ThreadStatus::Idle)];
        }
        state.sessions.sessions = sessions_for(&state.sessions.projects);
        state
    }

    /// `state` after typing `keys` into the open picker.
    fn typed(mut state: AppState, keys: &[char]) -> AppState {
        for &ch in keys {
            IntentHandler::handle(&Intent::PickerInput(ch), &mut state);
        }
        state
    }

    #[rstest::rstest]
    fn session_picker_lists_the_latest_turn_end_first() {
        // Given thread 1 last ending at 20s and thread 2 at 10s.
        let state = state_at(
            vec![ended_at(1, 20), ended_at(2, 10)],
            SidebarItem::SettledShelf,
        );

        // When opening the session picker.
        let state = open_sessions(state);

        // Then thread 1 is listed first.
        assert_eq!(
            session_ids(&state),
            [SessionId(1), SessionId(2)],
            "the latest turn end should come first"
        );
    }

    #[rstest::rstest]
    fn session_picker_lists_a_working_thread_by_its_turn_start() {
        // Given working thread 1, last ending at 10s with a turn started at
        // 30s, and thread 2 last ending at 20s.
        let working = Thread {
            status: ThreadStatus::Working,
            turn_started_at: Some(at(30)),
            ..ended_at(1, 10)
        };
        let state = state_at(vec![working, ended_at(2, 20)], SidebarItem::SettledShelf);

        // When opening the session picker.
        let state = open_sessions(state);

        // Then the working thread is listed first.
        assert_eq!(
            session_ids(&state),
            [SessionId(1), SessionId(2)],
            "a turn started after another's end should rank first"
        );
    }

    #[rstest::rstest]
    fn session_picker_lists_only_the_filtered_projects_threads() {
        // Given alpha (thread 11) and beta (thread 21), filtered to alpha.
        let mut state = with_project_threads(&["alpha", "beta"]);
        state.sessions.filter = Some(ProjectId(1));

        // When opening the session picker.
        let state = open_sessions(state);

        // Then only alpha's thread is listed.
        assert_eq!(
            session_ids(&state),
            [SessionId(11)],
            "the project filter should apply"
        );
    }

    #[rstest::rstest]
    fn session_picker_lists_the_selected_thread() {
        // Given thread 1 last ending at 20s and thread 2 at 10s, with thread 2
        // selected.
        let state = state_with(vec![ended_at(1, 20), ended_at(2, 10)], 2);

        // When opening the session picker.
        let state = open_sessions(state);

        // Then both are listed, newest chat first.
        assert_eq!(
            session_ids(&state),
            [SessionId(1), SessionId(2)],
            "the selected thread should be listed in its place"
        );
    }

    #[rstest::rstest]
    fn session_picker_leaves_out_a_thread_being_deleted() {
        // Given threads 1 and 2, with thread 1 being deleted.
        let mut state = state_at(
            vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
            SidebarItem::SettledShelf,
        );
        state.sessions.deleting.insert(SessionId(1));

        // When opening the session picker.
        let state = open_sessions(state);

        // Then only thread 2 is listed.
        assert_eq!(
            session_ids(&state),
            [SessionId(2)],
            "a thread being deleted shouldn't be listed"
        );
    }

    #[rstest::rstest]
    fn session_picker_hides_a_settled_thread() {
        // Given settled thread 1 and idle thread 2.
        let state = state_at(
            vec![settled(1), thread(2, ThreadStatus::Idle)],
            SidebarItem::SettledShelf,
        );

        // When opening the session picker.
        let state = open_sessions(state);

        // Then only thread 2 is listed.
        assert_eq!(
            session_ids(&state),
            [SessionId(2)],
            "settled threads should start hidden"
        );
    }

    #[rstest::rstest]
    fn toggling_settled_lists_a_settled_thread() {
        // Given the session picker over settled thread 1 and idle thread 2.
        let mut state = open_sessions(state_at(
            vec![settled(1), thread(2, ThreadStatus::Idle)],
            SidebarItem::SettledShelf,
        ));

        // When handling PickerToggleSettled.
        IntentHandler::handle(&Intent::PickerToggleSettled, &mut state);

        // Then thread 1 is listed.
        assert!(
            session_ids(&state).contains(&SessionId(1)),
            "toggling settled should list settled threads"
        );
    }

    #[rstest::rstest]
    fn lone_thread_row_is_labelled_project_and_title() {
        // Given thread 1, titled `fix login`, in project `work`.
        let state = state_at(
            vec![named(thread(1, ThreadStatus::Idle), "fix login")],
            SidebarItem::SettledShelf,
        );

        // When opening the session picker.
        let state = open_sessions(state);

        // Then its row reads `work/fix login`.
        assert_eq!(
            session_labels(&state),
            ["work/fix login"],
            "a lone thread is labelled by its project"
        );
    }

    #[rstest::rstest]
    fn incognito_thread_row_is_labelled_incognito_and_title() {
        // Given thread 1, titled `scratch`, in orb's Incognito project.
        let mut state = with_projects(&["Incognito"]);
        if let Some(project) = state.sessions.projects.first_mut() {
            project.kind = ProjectKind::Incognito;
            project.threads = vec![named(thread(1, ThreadStatus::Idle), "scratch")];
        }
        state.sessions.sessions = sessions_for(&state.sessions.projects);

        // When opening the session picker.
        let state = open_sessions(state);

        // Then its row reads `Incognito/scratch`.
        assert_eq!(
            session_labels(&state),
            ["Incognito/scratch"],
            "an Incognito thread is labelled Incognito"
        );
    }

    #[rstest::rstest]
    fn untitled_session_row_ends_in_its_folder() {
        // Given untitled thread 1 in project `work`, running in /work/1.
        let state = state_at(
            vec![thread(1, ThreadStatus::Idle)],
            SidebarItem::SettledShelf,
        );

        // When opening the session picker.
        let state = open_sessions(state);

        // Then its row reads `work/1`.
        assert_eq!(
            session_labels(&state),
            ["work/1"],
            "an untitled session is labelled by its folder"
        );
    }

    #[rstest::rstest]
    fn typed_project_name_narrows_to_its_threads() {
        // Given the session picker over itemku (thread 11) and orb (thread
        // 21), with no project filter.
        let state = open_sessions(with_project_threads(&["itemku", "orb"]));

        // When typing `itemku`.
        let state = typed(state, &['i', 't', 'e', 'm', 'k', 'u']);

        // Then only itemku's thread is shown.
        assert_eq!(
            session_ids(&state),
            [SessionId(11)],
            "typed text should match the project part of the label"
        );
    }

    #[rstest::rstest]
    fn open_session_picker_from_the_pane_returns_to_the_pane() {
        // Given the keys in the attached pane.
        let state = jumping(Focus::Attached, &[2], &[]);

        // When opening the session picker.
        let state = open_sessions(state);

        // Then closing it would return to the pane.
        assert_eq!(
            state.picker.as_ref().map(PickerState::return_to),
            Some(Focus::Attached),
            "the session picker should return to the pane"
        );
    }

    #[rstest::rstest]
    fn open_session_picker_focuses_the_picker() {
        // Given the keys in the sidebar.
        let state = jumping(Focus::Sidebar, &[], &[]);

        // When opening the session picker.
        let state = open_sessions(state);

        // Then the picker takes the keys.
        assert_eq!(
            state.focus,
            Focus::Picker,
            "the picker should take the keys"
        );
    }

    #[rstest::rstest]
    fn toggle_settled_with_text_typed_lists_the_settled_match() {
        // Given the session picker over settled `alpha` (1) and `beta` (2),
        // with `alpha` typed, which shows nothing.
        let state = state_at(
            vec![
                named(settled(1), "alpha"),
                named(thread(2, ThreadStatus::Idle), "beta"),
            ],
            SidebarItem::SettledShelf,
        );
        let mut state = typed(open_sessions(state), &['a', 'l', 'p', 'h', 'a']);

        // When handling PickerToggleSettled.
        IntentHandler::handle(&Intent::PickerToggleSettled, &mut state);

        // Then only `alpha` is shown, still filtered by the typed text.
        assert_eq!(
            session_ids(&state),
            [SessionId(1)],
            "toggling should keep the typed text"
        );
    }

    #[rstest::rstest]
    fn toggle_settled_in_another_picker_changes_nothing() {
        // Given the project picker over alpha and beta.
        let mut state = picking(Focus::Sidebar);

        // When handling PickerToggleSettled.
        IntentHandler::handle(&Intent::PickerToggleSettled, &mut state);

        // Then it's still the project picker over both projects.
        assert_eq!(
            state
                .picker
                .as_ref()
                .map(|picker| (picker.kind().clone(), picker.total())),
            Some((PickerKind::Projects, 2)),
            "other pickers should ignore the settled toggle"
        );
    }

    /// The session picker opened on thread 2 of [`jumping`], listing 2, 1,
    /// with thread 1 highlighted.
    fn picking_thread_1() -> AppState {
        let mut state = open_sessions(jumping(Focus::Sidebar, &[], &[]));
        IntentHandler::handle(&Intent::PickerNext, &mut state);
        state
    }

    #[rstest::rstest]
    fn picking_a_session_moves_the_cursor_to_it() {
        // Given the session picker on thread 1, opened on thread 2.
        let mut state = picking_thread_1();

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the cursor is on thread 1.
        assert_eq!(
            state.sessions.cursor,
            Some(on_thread(1)),
            "picking should select the thread"
        );
    }

    #[rstest::rstest]
    fn picking_a_session_attaches_it() {
        // Given the session picker on thread 1, opened on thread 2.
        let mut state = picking_thread_1();

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the loop attaches to thread 1.
        assert!(
            commands.contains(&Command::Attach(SessionId(1))),
            "picking should attach to the thread"
        );
    }

    #[rstest::rstest]
    fn picking_a_session_focuses_the_pane() {
        // Given the session picker on thread 1, opened on thread 2.
        let mut state = picking_thread_1();

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the pane takes the keys.
        assert_eq!(
            state.focus,
            Focus::Attached,
            "picking should focus the pane"
        );
    }

    #[rstest::rstest]
    fn picking_a_session_records_the_row_left_then_the_thread() {
        // Given the session picker on thread 1, opened on thread 2.
        let mut state = picking_thread_1();

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then thread 2, then thread 1, are recorded.
        assert_eq!(
            state.jumps.entries(),
            [on_thread(2), on_thread(1)],
            "picking should record where it left and where it landed"
        );
    }

    #[rstest::rstest]
    fn picking_the_selected_thread_records_no_jump() {
        // Given the session picker on thread 2, opened on thread 2.
        let mut state = open_sessions(jumping(Focus::Sidebar, &[], &[]));

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the jump list is still empty.
        assert!(
            state.jumps.entries().is_empty(),
            "picking the thread you're on isn't a jump"
        );
    }

    /// The session picker on session 1, opened on session 2, after session
    /// 1 was deleted.
    fn picking_deleted() -> AppState {
        let mut state = picking_thread_1();
        state
            .sessions
            .sessions
            .retain(|session| session.id != SessionId(1));
        if let Some(project) = state.sessions.projects.first_mut() {
            project.threads.retain(|thread| thread.id != ThreadId(1));
        }
        state
    }

    #[rstest::rstest]
    fn picking_a_session_deleted_since_opening_closes_the_picker() {
        // Given thread 1 highlighted, but deleted since the picker opened.
        let mut state = picking_deleted();

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the picker is closed.
        assert!(state.picker.is_none(), "the picker should close");
    }

    #[rstest::rstest]
    fn picking_a_session_deleted_since_opening_returns_no_commands() {
        // Given thread 1 highlighted, but deleted since the picker opened.
        let mut state = picking_deleted();

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then nothing happens.
        assert!(commands.is_empty(), "a deleted thread can't be jumped into");
    }

    /// `thread` with a transcript at `/t/<id>.jsonl`.
    fn with_transcript(thread: Thread) -> Thread {
        Thread {
            transcript: Some(format!("/t/{}.jsonl", thread.id.0).into()),
            ..thread
        }
    }

    /// The `LoadPreview` for thread `id`'s transcript.
    fn load(id: i64) -> Command {
        Command::LoadPreview {
            thread: ThreadId(id),
            transcript: format!("/t/{id}.jsonl").into(),
        }
    }

    /// `alpha` (thread 1, newer) and `zulu` (thread 2), both with
    /// transcripts, in the open session picker.
    fn alpha_and_zulu() -> AppState {
        open_sessions(state_at(
            vec![
                named(with_transcript(ended_at(1, 20)), "alpha"),
                named(with_transcript(ended_at(2, 10)), "zulu"),
            ],
            SidebarItem::SettledShelf,
        ))
    }

    #[rstest::rstest]
    fn opening_the_session_picker_loads_the_selected_threads_preview() {
        // Given thread 1 with a transcript.
        let mut state = state_at(
            vec![with_transcript(thread(1, ThreadStatus::Idle))],
            SidebarItem::SettledShelf,
        );

        // When handling OpenSessionPicker.
        let commands = IntentHandler::handle(&Intent::OpenSessionPicker, &mut state);

        // Then thread 1's preview is loaded.
        assert_eq!(commands, [load(1)], "opening should load the preview");
    }

    #[rstest::rstest]
    fn opening_the_session_picker_on_a_thread_without_a_transcript_returns_no_command() {
        // Given thread 1 without a transcript.
        let mut state = state_at(
            vec![thread(1, ThreadStatus::Idle)],
            SidebarItem::SettledShelf,
        );

        // When handling OpenSessionPicker.
        let commands = IntentHandler::handle(&Intent::OpenSessionPicker, &mut state);

        // Then nothing is loaded.
        assert!(
            commands.is_empty(),
            "a thread with no transcript has nothing to read"
        );
    }

    #[rstest::rstest]
    fn selecting_a_thread_without_a_transcript_shows_no_preview() {
        // Given the session picker on thread 1 (transcript, previewed) above
        // thread 2 (no transcript).
        let mut state = open_sessions(state_at(
            vec![with_transcript(ended_at(1, 20)), ended_at(2, 10)],
            SidebarItem::SettledShelf,
        ));
        if let Some(picker) = &mut state.picker {
            picker.show_preview(ThreadId(1), 1, vec![]);
        }

        // When handling PickerNext onto thread 2.
        IntentHandler::handle(&Intent::PickerNext, &mut state);

        // Then no preview is shown.
        assert!(
            state
                .picker
                .as_ref()
                .and_then(PickerState::preview)
                .is_none(),
            "a thread with no transcript should show no preview"
        );
    }

    #[rstest::rstest]
    fn picker_select_row_selects_that_row() {
        // Given the project picker over alpha and beta, alpha selected.
        let mut state = picking(Focus::Sidebar);

        // When handling PickerSelectRow(1).
        IntentHandler::handle(&Intent::PickerSelectRow(1), &mut state);

        // Then beta's row is selected.
        assert_eq!(
            state.picker.as_ref().map(PickerState::selection),
            Some(1),
            "a clicked row should be selected"
        );
    }

    #[rstest::rstest]
    fn picker_select_row_past_the_end_changes_nothing() {
        // Given the project picker over alpha and beta, alpha selected.
        let mut state = picking(Focus::Sidebar);

        // When handling PickerSelectRow(9), past the last row.
        IntentHandler::handle(&Intent::PickerSelectRow(9), &mut state);

        // Then alpha stays selected.
        assert_eq!(
            state.picker.as_ref().map(PickerState::selection),
            Some(0),
            "an index past the end should change nothing"
        );
    }

    #[rstest::rstest]
    fn picker_wheel_next_on_the_last_row_stays_there() {
        // Given the project picker over alpha and beta, beta selected.
        let mut state = picking(Focus::Sidebar);
        IntentHandler::handle(&Intent::PickerNext, &mut state);

        // When handling PickerWheelNext.
        IntentHandler::handle(&Intent::PickerWheelNext, &mut state);

        // Then beta stays selected instead of wrapping to alpha.
        assert_eq!(
            state.picker.as_ref().map(PickerState::selection),
            Some(1),
            "the wheel should stop on the last row"
        );
    }

    #[rstest::rstest]
    fn picker_wheel_prev_on_the_first_row_stays_there() {
        // Given the project picker over alpha and beta, alpha selected.
        let mut state = picking(Focus::Sidebar);

        // When handling PickerWheelPrev.
        IntentHandler::handle(&Intent::PickerWheelPrev, &mut state);

        // Then alpha stays selected instead of wrapping to beta.
        assert_eq!(
            state.picker.as_ref().map(PickerState::selection),
            Some(0),
            "the wheel should stop on the first row"
        );
    }

    #[rstest::rstest]
    #[case::next(Intent::PickerNext, 3)]
    #[case::prev(Intent::PickerPrev, 1)]
    #[case::half_page_down(Intent::PickerHalfPageDown, 3)]
    #[case::half_page_up(Intent::PickerHalfPageUp, 1)]
    #[case::select_row(Intent::PickerSelectRow(2), 3)]
    #[case::wheel_next(Intent::PickerWheelNext, 3)]
    #[case::wheel_prev(Intent::PickerWheelPrev, 1)]
    fn moving_in_the_session_picker_loads_the_new_selections_preview(
        #[case] intent: Intent,
        #[case] expected: i64,
    ) {
        // Given the session picker on thread 2 of threads 1, 2 and 3, all with
        // transcripts.
        let mut state = open_sessions(state_at(
            vec![
                with_transcript(ended_at(1, 30)),
                with_transcript(ended_at(2, 20)),
                with_transcript(ended_at(3, 10)),
            ],
            SidebarItem::SettledShelf,
        ));
        IntentHandler::handle(&Intent::PickerNext, &mut state);

        // When handling the move.
        let commands = IntentHandler::handle(&intent, &mut state);

        // Then the newly selected thread's preview is loaded.
        assert_eq!(
            commands,
            [load(expected)],
            "{intent:?} should load the preview"
        );
    }

    #[rstest::rstest]
    fn picker_next_on_a_one_row_session_picker_loads_no_preview() {
        // Given the session picker over one thread with a transcript.
        let mut state = open_sessions(state_at(
            vec![with_transcript(thread(1, ThreadStatus::Idle))],
            SidebarItem::SettledShelf,
        ));

        // When handling PickerNext.
        let commands = IntentHandler::handle(&Intent::PickerNext, &mut state);

        // Then nothing is loaded, since the selection didn't change.
        assert!(commands.is_empty(), "the same selection needs no new read");
    }

    #[rstest::rstest]
    fn typing_that_selects_another_thread_loads_its_preview() {
        // Given the session picker over `alpha` (selected) and `zulu`.
        let mut state = alpha_and_zulu();

        // When typing `z`.
        let commands = IntentHandler::handle(&Intent::PickerInput('z'), &mut state);

        // Then `zulu`'s preview is loaded.
        assert_eq!(commands, [load(2)], "the new selection should be loaded");
    }

    #[rstest::rstest]
    #[case::backspace(Intent::PickerBackspace)]
    #[case::delete_word(Intent::PickerDeleteWord)]
    fn clearing_the_filter_loads_the_first_threads_preview(#[case] intent: Intent) {
        // Given the session picker over `alpha` and `zulu` with `z` typed.
        let mut state = typed(alpha_and_zulu(), &['z']);

        // When clearing the typed text.
        let commands = IntentHandler::handle(&intent, &mut state);

        // Then `alpha`'s preview is loaded.
        assert_eq!(
            commands,
            [load(1)],
            "{intent:?} should load the first thread"
        );
    }

    #[rstest::rstest]
    fn filtering_out_every_thread_loads_no_preview() {
        // Given the session picker over `alpha` and `zulu`.
        let mut state = alpha_and_zulu();

        // When typing `q`, which matches neither.
        let commands = IntentHandler::handle(&Intent::PickerInput('q'), &mut state);

        // Then nothing is loaded.
        assert!(commands.is_empty(), "no selection has nothing to read");
    }

    /// The session picker over settled thread 1, last active at `secs`, and
    /// thread 2, ended at 10 s, both with transcripts.
    fn settled_last_active_at(secs: u64) -> AppState {
        open_sessions(state_at(
            vec![
                with_transcript(Thread {
                    last_activity_at: at(secs),
                    ..settled(1)
                }),
                with_transcript(ended_at(2, 10)),
            ],
            SidebarItem::SettledShelf,
        ))
    }

    #[rstest::rstest]
    fn showing_settled_threads_loads_the_newly_selected_threads_preview() {
        // Given settled thread 1, last active at 30 s, hidden above thread 2.
        let mut state = settled_last_active_at(30);

        // When handling PickerToggleSettled.
        let commands = IntentHandler::handle(&Intent::PickerToggleSettled, &mut state);

        // Then thread 1's preview is loaded.
        assert_eq!(commands, [load(1)], "the new selection should be loaded");
    }

    #[rstest::rstest]
    fn showing_settled_threads_that_keeps_the_selection_loads_no_preview() {
        // Given settled thread 1, last active at 5 s, hidden below thread 2.
        let mut state = settled_last_active_at(5);

        // When handling PickerToggleSettled.
        let commands = IntentHandler::handle(&Intent::PickerToggleSettled, &mut state);

        // Then nothing is loaded.
        assert!(commands.is_empty(), "the same selection needs no new read");
    }

    #[rstest::rstest]
    fn select_row_selects_that_row() {
        // Given threads listed 3, 2, 1 with the cursor on thread 2.
        let mut state = three_titles();

        // When handling SelectRow on thread 3 (a click).
        IntentHandler::handle(&Intent::SelectRow(on_thread(3)), &mut state);

        // Then the cursor is on thread 3.
        assert_eq!(
            state.sessions.cursor,
            Some(on_thread(3)),
            "a click should select its row"
        );
    }

    #[rstest::rstest]
    fn select_row_on_an_item_not_listed_changes_nothing() {
        // Given threads listed 3, 2, 1 with the cursor on thread 2.
        let mut state = three_titles();

        // When handling SelectRow on a thread that isn't listed.
        IntentHandler::handle(&Intent::SelectRow(on_thread(99)), &mut state);

        // Then the cursor stays on thread 2.
        assert_eq!(
            state.sessions.cursor,
            Some(on_thread(2)),
            "a row that isn't listed shouldn't move the cursor"
        );
    }

    #[rstest::rstest]
    fn select_row_during_a_search_ends_the_search() {
        // Given a search for "fix" started from thread 2, on match thread 3.
        let mut state = searching("fix", Some(3));

        // When handling SelectRow on thread 1.
        IntentHandler::handle(&Intent::SelectRow(on_thread(1)), &mut state);

        // Then the search is gone and the sidebar has the keys.
        assert_eq!(
            (state.sessions.search.is_none(), state.focus),
            (true, Focus::Sidebar),
            "a click on a row should end the search as ⏎ does"
        );
    }

    #[rstest::rstest]
    fn select_row_during_a_search_records_the_jump_from_where_it_started() {
        // Given a search for "fix" started from thread 2, on match thread 3.
        let mut state = searching("fix", Some(3));

        // When handling SelectRow on thread 1.
        IntentHandler::handle(&Intent::SelectRow(on_thread(1)), &mut state);

        // Then thread 2, then thread 1, are recorded.
        assert_eq!(
            state.jumps.entries(),
            [on_thread(2), on_thread(1)],
            "a click during a search should record where it left and where it landed"
        );
    }

    #[rstest::rstest]
    fn select_wheel_next_on_the_last_row_stays_there() {
        // Given threads listed 3, 2, 1 with the cursor on the last, thread 1.
        let mut state = three_titles();
        state.sessions.cursor = Some(on_thread(1));

        // When handling SelectWheelNext (the wheel down).
        IntentHandler::handle(&Intent::SelectWheelNext, &mut state);

        // Then the cursor stays on thread 1.
        assert_eq!(
            state.sessions.cursor,
            Some(on_thread(1)),
            "the wheel shouldn't wrap past the last row"
        );
    }

    #[rstest::rstest]
    fn select_wheel_prev_on_the_first_row_stays_there() {
        // Given threads listed 3, 2, 1 with the cursor on the first, thread 3.
        let mut state = three_titles();
        state.sessions.cursor = Some(on_thread(3));

        // When handling SelectWheelPrev (the wheel up).
        IntentHandler::handle(&Intent::SelectWheelPrev, &mut state);

        // Then the cursor stays on thread 3.
        assert_eq!(
            state.sessions.cursor,
            Some(on_thread(3)),
            "the wheel shouldn't wrap past the first row"
        );
    }

    #[rstest::rstest]
    fn picker_cursor_to_moves_the_pickers_cursor() {
        // Given the project picker with "al" typed.
        let mut state = picking(Focus::Sidebar);
        IntentHandler::handle(&Intent::PickerInput('a'), &mut state);
        IntentHandler::handle(&Intent::PickerInput('l'), &mut state);

        // When handling PickerCursorTo(1).
        IntentHandler::handle(&Intent::PickerCursorTo(1), &mut state);

        // Then the picker's cursor is between "a" and "l".
        assert_eq!(
            state.picker.as_ref().map(PickerState::cursor),
            Some(1),
            "a click should move the picker's cursor"
        );
    }

    #[rstest::rstest]
    fn picker_cursor_to_moves_the_rename_boxs_cursor() {
        // Given the rename box holding "abc".
        let mut state = renaming("abc");

        // When handling PickerCursorTo(1).
        IntentHandler::handle(&Intent::PickerCursorTo(1), &mut state);

        // Then the box's cursor is between "a" and "b".
        assert_eq!(
            state.rename.as_ref().map(|rename| rename.input.cursor()),
            Some(1),
            "a click should move the rename box's cursor"
        );
    }

    #[rstest::rstest]
    fn picker_cursor_to_moves_the_search_cursor() {
        // Given a search for "thr".
        let mut state = searching("thr", Some(2));

        // When handling PickerCursorTo(1).
        IntentHandler::handle(&Intent::PickerCursorTo(1), &mut state);

        // Then the search's cursor is between "t" and "h".
        assert_eq!(
            state
                .sessions
                .search
                .as_ref()
                .map(|search| search.input.cursor()),
            Some(1),
            "a click should move the search's cursor"
        );
    }

    #[rstest::rstest]
    fn picker_cursor_to_past_the_end_stops_at_the_end() {
        // Given the rename box holding "abc".
        let mut state = renaming("abc");

        // When handling PickerCursorTo(9), past the text.
        IntentHandler::handle(&Intent::PickerCursorTo(9), &mut state);

        // Then the cursor is at the end of "abc".
        assert_eq!(
            state.rename.as_ref().map(|rename| rename.input.cursor()),
            Some(3),
            "a click past the text should put the cursor at its end"
        );
    }

    const USED: &str = "/home/u/.orb/worktrees/work/orb-ffff";
    const ORPHAN: &str = "/home/u/.orb/worktrees/work/orb-0000";

    /// Thread 1 running in worktree `USED`, and a clean orphan worktree
    /// `ORPHAN` that sorts before it by path.
    fn worktree_state() -> AppState {
        let worktree = |path: &str| Worktree {
            path: path.into(),
            repo: Some("/work".into()),
            facts: None,
            size_kb: None,
        };
        AppState {
            worktrees: Worktrees {
                list: vec![worktree(ORPHAN), worktree(USED)],
                notice: None,
            },
            ..state_at(
                vec![Thread {
                    cwd: USED.into(),
                    ..thread(1, ThreadStatus::Idle)
                }],
                SidebarItem::Session(SessionId(1)),
            )
        }
    }

    /// The paths of the open picker's shown worktree rows.
    fn worktree_rows(state: &AppState) -> Vec<PathBuf> {
        state
            .picker
            .iter()
            .flat_map(PickerState::shown)
            .filter_map(|(item, _)| match item {
                PickerItem::Worktree { path, .. } => Some(path.clone()),
                _ => None,
            })
            .collect()
    }

    #[rstest::rstest]
    fn open_worktree_picker_opens_a_worktrees_picker() {
        // Given orb's worktrees.
        let mut state = worktree_state();

        // When handling OpenWorktreePicker.
        IntentHandler::handle(&Intent::OpenWorktreePicker, &mut state);

        // Then the worktree picker is open.
        assert_eq!(
            state.picker.as_ref().map(PickerState::kind),
            Some(&PickerKind::Worktrees),
            "␣sw should open the worktree picker"
        );
    }

    #[rstest::rstest]
    fn open_worktree_picker_returns_refresh_worktrees() {
        // Given orb's worktrees.
        let mut state = worktree_state();

        // When handling OpenWorktreePicker.
        let commands = IntentHandler::handle(&Intent::OpenWorktreePicker, &mut state);

        // Then the worktrees are re-read.
        assert_eq!(
            commands,
            [Command::RefreshWorktrees],
            "opening should refresh the worktrees"
        );
    }

    #[rstest::rstest]
    fn open_worktree_picker_lists_rows_in_order() {
        // Given a used worktree and an orphan that sorts first by path.
        let mut state = worktree_state();

        // When handling OpenWorktreePicker.
        IntentHandler::handle(&Intent::OpenWorktreePicker, &mut state);

        // Then the used worktree comes first and the orphan last.
        assert_eq!(
            worktree_rows(&state),
            [PathBuf::from(USED), PathBuf::from(ORPHAN)],
            "rows should follow the worktree order"
        );
    }

    #[rstest::rstest]
    fn confirm_on_a_worktree_row_returns_no_commands() {
        // Given the worktree picker open.
        let mut state = worktree_state();
        IntentHandler::handle(&Intent::OpenWorktreePicker, &mut state);

        // When handling PickerConfirm.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then nothing follows.
        assert!(commands.is_empty(), "⏎ on a worktree row does nothing");
    }

    #[rstest::rstest]
    fn confirm_on_a_worktree_row_keeps_the_picker_open() {
        // Given the worktree picker open with its second row clicked.
        let mut state = worktree_state();
        IntentHandler::handle(&Intent::OpenWorktreePicker, &mut state);
        IntentHandler::handle(&Intent::PickerSelectRow(1), &mut state);

        // When handling PickerConfirm, as a double-click does.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the worktree picker stays open.
        assert_eq!(
            state.picker.as_ref().map(PickerState::kind),
            Some(&PickerKind::Worktrees),
            "⏎ should leave the worktree picker open"
        );
    }

    /// The worktree picker open over `state`, with row `row` selected and
    /// `<C-x>` pressed on it.
    fn delete_row(state: &mut AppState, row: usize) {
        IntentHandler::handle(&Intent::OpenWorktreePicker, state);
        IntentHandler::handle(&Intent::PickerSelectRow(row), state);
        IntentHandler::handle(&Intent::PickerRemove, state);
    }

    /// The open picker's selected worktree row.
    fn picked_worktree(state: &AppState) -> Option<PathBuf> {
        state
            .picker
            .as_ref()
            .and_then(PickerState::selected_worktree)
            .map(Path::to_path_buf)
    }

    #[rstest::rstest]
    fn remove_on_an_attached_row_sets_the_refusal() {
        // Given thread 1, in `USED`, attached.
        let mut state = AppState {
            attached: [SessionId(1)].into(),
            ..worktree_state()
        };

        // When pressing <C-x> on `USED`'s row.
        delete_row(&mut state, 0);

        // Then the mode line says why it can't be deleted.
        assert_eq!(
            state.sessions.error.as_deref(),
            Some("can't delete orb-ffff: a thread in it is attached"),
            "an attached row should be refused"
        );
    }

    #[rstest::rstest]
    fn remove_on_a_mid_turn_row_sets_the_refusal() {
        // Given thread 1, in `USED`, working.
        let mut state = worktree_state();
        for thread in state
            .sessions
            .projects
            .iter_mut()
            .flat_map(|p| &mut p.threads)
        {
            thread.status = ThreadStatus::Working;
        }

        // When pressing <C-x> on `USED`'s row.
        delete_row(&mut state, 0);

        // Then the mode line says why it can't be deleted.
        assert_eq!(
            state.sessions.error.as_deref(),
            Some("can't delete orb-ffff: a thread in it is mid-turn"),
            "a mid-turn row should be refused"
        );
    }

    #[rstest::rstest]
    fn remove_on_a_clean_row_opens_delete_worktree_confirm() {
        // Given orb's worktrees, none with changes.
        let mut state = worktree_state();

        // When pressing <C-x> on the orphan's row.
        delete_row(&mut state, 1);

        // Then the clean delete confirm is open for it.
        assert_eq!(
            state.picker.as_ref().map(PickerState::kind),
            Some(&PickerKind::DeleteWorktree {
                path: ORPHAN.into(),
                dirty: false,
            }),
            "<C-x> should ask to confirm the delete"
        );
    }

    #[rstest::rstest]
    fn remove_on_a_dirty_row_marks_the_confirm_dirty() {
        // Given the orphan with two uncommitted changes.
        let mut state = worktree_state();
        for worktree in &mut state.worktrees.list {
            if worktree.path == Path::new(ORPHAN) {
                worktree.facts = Some(WorktreeFacts {
                    branch: None,
                    changes: 2,
                    last_commit: None,
                });
            }
        }

        // When pressing <C-x> on the orphan's row.
        delete_row(&mut state, 1);

        // Then the confirm says it's dirty.
        assert_eq!(
            state.picker.as_ref().map(PickerState::kind),
            Some(&PickerKind::DeleteWorktree {
                path: ORPHAN.into(),
                dirty: true,
            }),
            "a worktree with changes should get the dirty confirm"
        );
    }

    #[rstest::rstest]
    fn yes_returns_delete_worktree_command() {
        // Given the delete confirm for `USED` with Yes highlighted.
        let mut state = worktree_state();
        delete_row(&mut state, 0);
        highlight(&mut state, &PickerItem::Confirm(true));

        // When handling PickerConfirm.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the worktree is deleted.
        assert_eq!(
            commands,
            [Command::DeleteWorktree { path: USED.into() }],
            "Yes should delete the worktree"
        );
    }

    #[rstest::rstest]
    fn yes_returns_to_the_list_without_the_row() {
        // Given the delete confirm for `USED` with Yes highlighted.
        let mut state = worktree_state();
        delete_row(&mut state, 0);
        highlight(&mut state, &PickerItem::Confirm(true));

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the list is back with only the orphan.
        assert_eq!(
            worktree_rows(&state),
            [PathBuf::from(ORPHAN)],
            "the deleted row should be gone from the list"
        );
    }

    #[rstest::rstest]
    fn yes_puts_the_cursor_on_the_neighbour() {
        // Given the delete confirm for the first row, `USED`, with Yes
        // highlighted.
        let mut state = worktree_state();
        delete_row(&mut state, 0);
        highlight(&mut state, &PickerItem::Confirm(true));

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the row after it is selected.
        assert_eq!(
            picked_worktree(&state),
            Some(PathBuf::from(ORPHAN)),
            "the next row should take the deleted row's place"
        );
    }

    #[rstest::rstest]
    fn yes_on_the_last_row_puts_the_cursor_on_the_previous_row() {
        // Given the delete confirm for the last row, the orphan, with Yes
        // highlighted.
        let mut state = worktree_state();
        delete_row(&mut state, 1);
        highlight(&mut state, &PickerItem::Confirm(true));

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the row before it is selected.
        assert_eq!(
            picked_worktree(&state),
            Some(PathBuf::from(USED)),
            "the previous row should be selected"
        );
    }

    #[rstest::rstest]
    fn no_returns_to_the_list_as_it_was() {
        // Given `orb` typed in the worktree picker and the delete confirm for
        // its second row, with No highlighted.
        let mut state = worktree_state();
        IntentHandler::handle(&Intent::OpenWorktreePicker, &mut state);
        for ch in "orb".chars() {
            IntentHandler::handle(&Intent::PickerInput(ch), &mut state);
        }
        IntentHandler::handle(&Intent::PickerSelectRow(1), &mut state);
        IntentHandler::handle(&Intent::PickerRemove, &mut state);

        // When handling PickerConfirm.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the worktree list is back with its text, rows and selection.
        let picker = state.picker.as_ref();
        assert_eq!(
            (
                picker.map(PickerState::kind),
                picker.map(PickerState::input),
                worktree_rows(&state).len(),
                picked_worktree(&state),
            ),
            (
                Some(&PickerKind::Worktrees),
                Some("orb"),
                2,
                Some(PathBuf::from(ORPHAN)),
            ),
            "No should put the list back as it was"
        );
    }

    #[rstest::rstest]
    fn esc_on_the_confirm_returns_to_the_list() {
        // Given the delete confirm for `USED`.
        let mut state = worktree_state();
        delete_row(&mut state, 0);

        // When handling PickerCancel.
        IntentHandler::handle(&Intent::PickerCancel, &mut state);

        // Then the worktree list is back, every row still there.
        assert_eq!(
            (
                state.picker.as_ref().map(PickerState::kind),
                worktree_rows(&state).len(),
            ),
            (Some(&PickerKind::Worktrees), 2),
            "Esc on the confirm should go back to the list"
        );
    }

    #[rstest::rstest]
    fn esc_on_the_worktree_list_closes_it() {
        // Given the worktree picker open.
        let mut state = worktree_state();
        IntentHandler::handle(&Intent::OpenWorktreePicker, &mut state);

        // When handling PickerCancel.
        IntentHandler::handle(&Intent::PickerCancel, &mut state);

        // Then no picker is open.
        assert!(state.picker.is_none(), "Esc on the list should close it");
    }

    /// Search hit `id` in `thread`, on `/t/<thread>.jsonl`.
    fn search_hit(id: i64, thread: i64) -> PickerItem {
        PickerItem::Hit {
            id,
            thread: ThreadId(thread),
            label: format!("work/thread {thread}"),
            split: 5,
            snippet: "fix the bug".into(),
            lit: vec![],
            text_lit: vec![],
            path: format!("/t/{thread}.jsonl").into(),
            prompt_offset: 0,
        }
    }

    /// The search picker opened over threads 1 and 2 (cursor on 2), `fix`
    /// typed, and the actor's `hits` listed, the first one selected.
    fn searching_hits(hits: Vec<PickerItem>) -> AppState {
        let mut state = jumping(Focus::Sidebar, &[], &[]);
        IntentHandler::handle(&Intent::OpenSearch, &mut state);
        let mut state = typed(state, &['f', 'i', 'x']);
        if let Some(picker) = &mut state.picker {
            picker.show_hits("fix", hits, false);
        }
        state
    }

    /// [`searching_hits`] with one hit, in `thread`.
    fn searching_a_hit_in(thread: i64) -> AppState {
        searching_hits(vec![search_hit(thread, thread)])
    }

    #[rstest::rstest]
    #[case::same_thread(1)]
    #[case::another_thread(2)]
    fn moving_to_another_hit_returns_its_search_preview(#[case] thread: i64) {
        // Given the search picker on hit 1 in thread 1, above hit 2 in `thread`.
        let mut state = searching_hits(vec![search_hit(1, 1), search_hit(2, thread)]);

        // When moving down.
        let commands = IntentHandler::handle(&Intent::PickerNext, &mut state);

        // Then hit 2's exchange is asked for.
        assert_eq!(
            commands,
            [Command::LoadSearchPreview {
                hit: 2,
                path: format!("/t/{thread}.jsonl").into(),
                prompt_offset: 0,
            }],
            "moving onto a hit should load its search preview"
        );
    }

    #[rstest::rstest]
    fn open_search_opens_the_search_picker() {
        // Given threads 1 and 2.
        let mut state = jumping(Focus::Sidebar, &[], &[]);

        // When handling OpenSearch.
        IntentHandler::handle(&Intent::OpenSearch, &mut state);

        // Then the search picker is open and takes the keys.
        assert_eq!(
            (state.picker.as_ref().map(PickerState::kind), state.focus),
            (Some(&PickerKind::Search { overflow: false }), Focus::Picker),
            "␣sg should open the search picker"
        );
    }

    #[rstest::rstest]
    fn open_search_returns_an_empty_search() {
        // Given threads 1 and 2.
        let mut state = jumping(Focus::Sidebar, &[], &[]);

        // When handling OpenSearch.
        let commands = IntentHandler::handle(&Intent::OpenSearch, &mut state);

        // Then an empty query is sent, which only catches the index up.
        assert_eq!(
            commands,
            [Command::SearchTranscripts {
                query: String::new()
            }],
            "opening should search for nothing"
        );
    }

    #[rstest::rstest]
    #[case::input(Intent::PickerInput('x'), "fix")]
    #[case::backspace(Intent::PickerBackspace, "f")]
    #[case::delete_word(Intent::PickerDeleteWord, "")]
    fn editing_the_search_input_returns_a_search_for_it(
        #[case] intent: Intent,
        #[case] query: &str,
    ) {
        // Given the search picker with `fi` typed.
        let mut state = jumping(Focus::Sidebar, &[], &[]);
        IntentHandler::handle(&Intent::OpenSearch, &mut state);
        let mut state = typed(state, &['f', 'i']);

        // When editing the typed text.
        let commands = IntentHandler::handle(&intent, &mut state);

        // Then the new text is searched for.
        assert_eq!(
            commands,
            [Command::SearchTranscripts {
                query: query.to_owned()
            }],
            "{intent:?} should search for the edited text"
        );
    }

    #[rstest::rstest]
    fn typing_in_the_session_picker_returns_no_search() {
        // Given the session picker over `alpha` and `zulu`.
        let mut state = alpha_and_zulu();

        // When typing `z`.
        let commands = IntentHandler::handle(&Intent::PickerInput('z'), &mut state);

        // Then no search is asked for.
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, Command::SearchTranscripts { .. })),
            "only the search picker searches transcripts"
        );
    }

    #[rstest::rstest]
    fn picking_a_hit_moves_the_cursor_to_its_thread() {
        // Given the search picker on a hit in thread 1, opened on thread 2.
        let mut state = searching_a_hit_in(1);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the cursor is on thread 1.
        assert_eq!(
            state.sessions.cursor,
            Some(on_thread(1)),
            "picking a hit should select its thread"
        );
    }

    #[rstest::rstest]
    fn picking_a_hit_returns_attach_for_its_thread() {
        // Given the search picker on a hit in thread 1, opened on thread 2.
        let mut state = searching_a_hit_in(1);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the loop attaches to thread 1.
        assert!(
            commands.contains(&Command::Attach(SessionId(1))),
            "picking a hit should attach to its thread"
        );
    }

    #[rstest::rstest]
    fn picking_a_hit_of_a_deleted_thread_closes_the_picker() {
        // Given the search picker on a hit in thread 9, which is gone.
        let mut state = searching_a_hit_in(9);

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the picker is closed.
        assert!(state.picker.is_none(), "⏎ should still close the picker");
    }

    #[rstest::rstest]
    fn picking_a_hit_of_a_deleted_thread_returns_no_commands() {
        // Given the search picker on a hit in thread 9, which is gone.
        let mut state = searching_a_hit_in(9);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then nothing happens.
        assert!(commands.is_empty(), "a gone thread can't be attached");
    }

    /// [`searching_a_hit_in`] thread 1, its pane ended, having last run in
    /// `last_session`.
    fn searching_an_ended_thread(last_session: Option<SessionId>) -> AppState {
        let mut state = searching_a_hit_in(1);
        for thread in state
            .sessions
            .projects
            .iter_mut()
            .flat_map(|project| project.threads.iter_mut())
            .filter(|thread| thread.id == ThreadId(1))
        {
            thread.pane = None;
            thread.last_session = last_session;
        }
        state
    }

    #[rstest::rstest]
    fn picking_a_hit_of_an_ended_thread_attaches_its_session() {
        // Given the search picker on a hit in thread 1, which ended in
        // session 1.
        let mut state = searching_an_ended_thread(Some(SessionId(1)));

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the loop attaches to session 1.
        assert!(
            commands.contains(&Command::Attach(SessionId(1))),
            "a hit on an ended thread should open its session"
        );
    }

    #[rstest::rstest]
    fn picking_a_hit_without_a_session_only_closes_the_picker() {
        // Given the search picker on a hit in thread 1, which never ran in a
        // session.
        let mut state = searching_an_ended_thread(None);

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the picker closes and nothing else happens.
        assert_eq!(
            (state.picker.is_none(), commands),
            (true, vec![]),
            "a hit with no session should only close the picker"
        );
    }

    /// Thread 1 selected and attached, shown in session 1's one-tab layout
    /// over an 80×24 body, with the keys at `focus`.
    fn with_layout(focus: Focus) -> AppState {
        let mut state = AppState {
            focus,
            attached: HashSet::from([SessionId(1)]),
            ..state_with(vec![thread(1, ThreadStatus::Idle)], 1)
        };
        state.layouts.fit_to(Rect::new(0, 0, 80, 24));
        state
    }

    /// `with_layout(Focus::Attached)` split right once: thread 1's pane on
    /// the left, shell pane 50 on the right with the focus.
    fn split_layout() -> AppState {
        let mut state = with_layout(Focus::Attached);
        state
            .layouts
            .split(SessionId(1), Split::Right, test_entry(50));
        state
    }

    /// `with_layout(Focus::Attached)` with a second tab of shell pane 60, shown.
    fn two_tabs() -> AppState {
        let mut state = with_layout(Focus::Attached);
        state.layouts.new_tab(SessionId(1), test_entry(60));
        state
    }

    fn shown_focus(state: &AppState) -> Option<PaneId> {
        state.shown_layout().and_then(SessionLayout::focused)
    }

    fn shown_tab(state: &AppState) -> Option<usize> {
        state.shown_layout().map(SessionLayout::active)
    }

    fn tab_count(state: &AppState) -> usize {
        state.shown_layout().map_or(0, |layout| layout.tabs().len())
    }

    fn pane_count(state: &AppState) -> usize {
        state
            .shown_layout()
            .map_or(0, |layout| layout.placed(Rect::new(0, 0, 80, 24)).len())
    }

    #[rstest::rstest]
    fn split_pane_asks_the_sessions_actor_for_a_pane() {
        // Given thread 1's lone pane with the keys.
        let mut state = with_layout(Focus::Attached);

        // When splitting it right.
        let commands = IntentHandler::handle(&Intent::SplitPane(Split::Right), &mut state);

        // Then the sessions actor is asked to split session 1.
        assert_eq!(
            commands,
            vec![Command::SplitPane {
                session: SessionId(1),
                split: Split::Right
            }],
            "a split goes through the sessions actor, which makes the pane"
        );
    }

    #[rstest::rstest]
    fn split_pane_outside_a_pane_does_nothing() {
        // Given thread 1's layout shown while the sidebar has the keys.
        let mut state = with_layout(Focus::Sidebar);

        // When splitting.
        let commands = IntentHandler::handle(&Intent::SplitPane(Split::Right), &mut state);

        // Then nothing is asked for.
        assert!(commands.is_empty(), "a split needs the keys in a pane");
    }

    #[rstest::rstest]
    fn move_focus_saves_the_layout() {
        // Given shell pane 50 split off and focused.
        let mut state = split_layout();

        // When moving the focus left.
        let commands = IntentHandler::handle(&Intent::MoveFocus(NavDirection::Left), &mut state);

        // Then session 1's layout is saved.
        assert_eq!(
            commands,
            vec![Command::SaveLayout(SessionId(1))],
            "a focus move should be saved"
        );
    }

    #[rstest::rstest]
    fn close_pane_saves_the_layout() {
        // Given shell pane 50 split off and focused.
        let mut state = split_layout();

        // When closing it.
        let commands = IntentHandler::handle(&Intent::ClosePane, &mut state);

        // Then session 1's layout is saved.
        assert_eq!(
            commands,
            vec![Command::SaveLayout(SessionId(1))],
            "a closed pane should be saved"
        );
    }

    #[rstest::rstest]
    fn zoom_saves_nothing() {
        // Given two panes side by side.
        let mut state = split_layout();

        // When zooming.
        let commands = IntentHandler::handle(&Intent::ToggleZoom, &mut state);

        // Then nothing is saved.
        assert!(commands.is_empty(), "zoom isn't saved");
    }

    #[rstest::rstest]
    fn close_pane_on_a_split_removes_it() {
        // Given shell pane 50 split off and focused.
        let mut state = split_layout();

        // When closing the focused pane.
        IntentHandler::handle(&Intent::ClosePane, &mut state);

        // Then only thread 1's pane is left, focused.
        assert_eq!(
            (pane_count(&state), shown_focus(&state)),
            (1, Some(PaneId(1))),
            "closing the split leaves the thread's pane"
        );
    }

    #[rstest::rstest]
    fn close_pane_on_an_agents_pane_closes_it() {
        // Given thread 1's lone pane with the keys.
        let mut state = with_layout(Focus::Attached);

        // When closing it.
        let commands = IntentHandler::handle(&Intent::ClosePane, &mut state);

        // Then its layout is gone and saved as such.
        assert_eq!(
            (state.layouts.get(SessionId(1)).is_some(), commands),
            (false, vec![Command::SaveLayout(SessionId(1))]),
            "an agent's pane closes like any other"
        );
    }

    #[rstest::rstest]
    fn move_focus_left_from_the_leftmost_pane_focuses_the_sidebar() {
        // Given thread 1's lone pane with the keys.
        let mut state = with_layout(Focus::Attached);

        // When moving the focus left.
        IntentHandler::handle(&Intent::MoveFocus(NavDirection::Left), &mut state);

        // Then the sidebar has the keys.
        assert_eq!(state.focus, Focus::Sidebar, "leftmost left is the sidebar");
    }

    #[rstest::rstest]
    fn move_focus_left_from_the_leftmost_pane_returns_detach() {
        // Given thread 1's lone pane with the keys.
        let mut state = with_layout(Focus::Attached);

        // When moving the focus left.
        let commands = IntentHandler::handle(&Intent::MoveFocus(NavDirection::Left), &mut state);

        // Then the pane loses the keys.
        assert!(
            commands.contains(&Command::Detach),
            "leaving the pane should detach its keys"
        );
    }

    #[rstest::rstest]
    fn move_focus_left_with_the_sidebar_hidden_keeps_the_keys() {
        // Given thread 1's lone pane with the keys and the sidebar hidden.
        let mut state = with_layout(Focus::Attached);
        state.sidebar.hidden = true;

        // When moving the focus left.
        IntentHandler::handle(&Intent::MoveFocus(NavDirection::Left), &mut state);

        // Then the pane keeps the keys.
        assert_eq!(
            state.focus,
            Focus::Attached,
            "a hidden sidebar can't take the keys"
        );
    }

    #[rstest::rstest]
    fn move_focus_right_from_the_sidebar_enters_the_shown_layout() {
        // Given thread 1's layout shown while the sidebar has the keys.
        let mut state = with_layout(Focus::Sidebar);

        // When moving the focus right.
        IntentHandler::handle(&Intent::MoveFocus(NavDirection::Right), &mut state);

        // Then the keys are in the pane.
        assert_eq!(state.focus, Focus::Attached, "Cmd l enters the layout");
    }

    #[rstest::rstest]
    fn move_focus_right_from_the_sidebar_without_a_layout_keeps_the_keys() {
        // Given thread 1 selected but not attached.
        let mut state = state_with(vec![thread(1, ThreadStatus::Idle)], 1);

        // When moving the focus right.
        IntentHandler::handle(&Intent::MoveFocus(NavDirection::Right), &mut state);

        // Then the sidebar keeps the keys.
        assert_eq!(state.focus, Focus::Sidebar, "nothing is shown to enter");
    }

    #[rstest::rstest]
    fn grow_with_the_sidebar_focused_widens_it() {
        // Given the sidebar 32 columns wide with the keys.
        let mut state = with_layout(Focus::Sidebar);
        state.sidebar.width = 32;

        // When growing.
        IntentHandler::handle(&Intent::GrowFocused, &mut state);

        // Then it is a step wider.
        assert_eq!(state.sidebar.width, 36, "Cmd + widens the sidebar");
    }

    #[rstest::rstest]
    fn grow_in_a_pane_grows_it() {
        // Given shell pane 50 split off on the right and focused.
        let mut state = split_layout();
        let width = |state: &AppState| {
            state
                .shown_layout()?
                .placed(Rect::new(0, 0, 80, 24))
                .into_iter()
                .find(|place| place.pane == PaneId(50))
                .map(|place| place.area.width)
        };
        let before = width(&state);

        // When growing.
        IntentHandler::handle(&Intent::GrowFocused, &mut state);

        // Then pane 50 is 4 columns wider.
        assert_eq!(
            width(&state),
            before.map(|width| width + 4),
            "Cmd + grows the focused pane"
        );
    }

    #[rstest::rstest]
    fn toggle_zoom_zooms_the_shown_tab() {
        // Given two panes side by side.
        let mut state = split_layout();

        // When zooming.
        IntentHandler::handle(&Intent::ToggleZoom, &mut state);

        // Then the tab is zoomed.
        let zoomed = state
            .shown_layout()
            .and_then(SessionLayout::active_tab)
            .is_some_and(Tab::zoomed);
        assert!(zoomed, "the shown tab should be zoomed");
    }

    #[rstest::rstest]
    fn new_tab_asks_the_sessions_actor_for_a_tab() {
        // Given thread 1's one-tab layout.
        let mut state = with_layout(Focus::Attached);

        // When opening a tab.
        let commands = IntentHandler::handle(&Intent::NewTab, &mut state);

        // Then the sessions actor is asked for a tab in session 1.
        assert_eq!(
            commands,
            vec![Command::NewTab(SessionId(1))],
            "a new tab goes through the sessions actor, which makes the pane"
        );
    }

    #[rstest::rstest]
    fn close_tab_removes_the_shown_tab() {
        // Given a second tab shown.
        let mut state = two_tabs();

        // When closing the tab.
        IntentHandler::handle(&Intent::CloseTab, &mut state);

        // Then only the first tab is left, shown.
        assert_eq!(
            (tab_count(&state), shown_tab(&state)),
            (1, Some(0)),
            "the shown tab closes"
        );
    }

    #[rstest::rstest]
    fn close_tab_holding_an_agents_pane_closes_it() {
        // Given thread 1's one-tab layout.
        let mut state = with_layout(Focus::Attached);

        // When closing the tab.
        let commands = IntentHandler::handle(&Intent::CloseTab, &mut state);

        // Then its layout is gone and saved as such.
        assert_eq!(
            (state.layouts.get(SessionId(1)).is_some(), commands),
            (false, vec![Command::SaveLayout(SessionId(1))]),
            "a tab holding an agent's pane closes like any other"
        );
    }

    #[rstest::rstest]
    fn rename_tab_opens_the_rename_box_on_the_tab() {
        // Given a second tab shown.
        let mut state = two_tabs();

        // When renaming the tab.
        IntentHandler::handle(&Intent::RenameTab, &mut state);

        // Then the rename box names thread 1's second tab and has the keys.
        assert_eq!(
            (
                state.rename.as_ref().map(|rename| rename.target),
                state.focus
            ),
            (
                Some(RenameTarget::Tab {
                    owner: SessionId(1),
                    tab: 1
                }),
                Focus::Rename
            ),
            "the rename box opens on the shown tab"
        );
    }

    /// The rename box open on thread 1's first tab, holding `name`.
    fn renaming_tab(name: &str) -> AppState {
        let mut state = with_layout(Focus::Rename);
        state.rename = Some(Rename {
            target: RenameTarget::Tab {
                owner: SessionId(1),
                tab: 0,
            },
            input: TextInput::new(name),
            creating: false,
        });
        state
    }

    fn shown_tab_name(state: &AppState) -> Option<String> {
        state
            .shown_layout()
            .and_then(SessionLayout::active_tab)
            .and_then(Tab::name)
            .map(str::to_owned)
    }

    #[rstest::rstest]
    fn rename_tab_confirm_names_the_tab() {
        // Given the rename box on the first tab holding "logs".
        let mut state = renaming_tab("logs");

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the tab is named logs.
        assert_eq!(
            shown_tab_name(&state).as_deref(),
            Some("logs"),
            "the tab takes the name"
        );
    }

    #[rstest::rstest]
    fn rename_tab_confirm_puts_the_keys_back_in_the_panes() {
        // Given the rename box on the first tab holding "logs".
        let mut state = renaming_tab("logs");

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the keys are back in the pane.
        assert_eq!(state.focus, Focus::Attached, "the panes get the keys back");
    }

    #[rstest::rstest]
    fn rename_tab_cancel_puts_the_keys_back_in_the_panes() {
        // Given the rename box on the first tab holding "logs".
        let mut state = renaming_tab("logs");

        // When cancelling.
        IntentHandler::handle(&Intent::PickerCancel, &mut state);

        // Then the keys are back in the pane.
        assert_eq!(state.focus, Focus::Attached, "the panes get the keys back");
    }

    #[rstest::rstest]
    fn rename_tab_confirm_of_an_empty_name_clears_it() {
        // Given the first tab named logs and the rename box holding spaces.
        let mut state = renaming_tab("  ");
        state
            .layouts
            .rename_tab(SessionId(1), 0, Some("logs".to_owned()));

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then the tab has no name.
        assert_eq!(shown_tab_name(&state), None, "a blank name clears it");
    }

    #[rstest::rstest]
    fn rename_pane_opens_the_rename_box_on_the_focused_pane() {
        // Given shell pane 50 split off and focused, named "server".
        let mut state = split_layout();
        state.layouts.rename_pane(PaneId(50), Some("server".into()));

        // When renaming the pane.
        IntentHandler::handle(&Intent::RenamePane, &mut state);

        // Then the rename box names pane 50, holding its name, and has the keys.
        assert_eq!(
            (
                state
                    .rename
                    .as_ref()
                    .map(|rename| (rename.target, rename.input.text().to_owned())),
                state.focus
            ),
            (
                Some((RenameTarget::Pane(PaneId(50)), "server".to_owned())),
                Focus::Rename
            ),
            "the rename box opens on the focused pane"
        );
    }

    /// The rename box open on thread 1's pane 1, holding `name`.
    fn renaming_pane(name: &str) -> AppState {
        let mut state = with_layout(Focus::Rename);
        state.rename = Some(Rename {
            target: RenameTarget::Pane(PaneId(1)),
            input: TextInput::new(name),
            creating: false,
        });
        state
    }

    #[rstest::rstest]
    fn rename_pane_confirm_names_the_pane() {
        // Given the rename box on pane 1 holding "agent".
        let mut state = renaming_pane("agent");

        // When confirming.
        IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then pane 1 is named agent.
        assert_eq!(
            state
                .layouts
                .entry(PaneId(1))
                .and_then(|entry| entry.name.clone()),
            Some("agent".to_owned()),
            "the pane takes the name"
        );
    }

    #[rstest::rstest]
    fn rename_pane_confirm_saves_the_layout() {
        // Given the rename box on pane 1 holding "agent".
        let mut state = renaming_pane("agent");

        // When confirming.
        let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

        // Then session 1's layout is saved.
        assert!(
            commands.contains(&Command::SaveLayout(SessionId(1))),
            "a pane's new name should be saved, got {commands:?}"
        );
    }

    #[rstest::rstest]
    fn rename_pane_cancel_puts_the_keys_back_in_the_panes() {
        // Given the rename box on pane 1 holding "agent".
        let mut state = renaming_pane("agent");

        // When cancelling.
        IntentHandler::handle(&Intent::PickerCancel, &mut state);

        // Then the keys are back in the pane.
        assert_eq!(state.focus, Focus::Attached, "the panes get the keys back");
    }

    #[rstest::rstest]
    fn go_to_tab_shows_it() {
        // Given a second tab shown.
        let mut state = two_tabs();

        // When going to tab 1.
        IntentHandler::handle(&Intent::GoToTab(1), &mut state);

        // Then the first tab is shown.
        assert_eq!(shown_tab(&state), Some(0), "Cmd 1 shows the first tab");
    }

    #[rstest::rstest]
    fn next_tab_shows_the_next() {
        // Given two tabs, the first shown.
        let mut state = two_tabs();
        IntentHandler::handle(&Intent::GoToTab(1), &mut state);

        // When showing the next tab.
        IntentHandler::handle(&Intent::NextTab, &mut state);

        // Then the second tab is shown.
        assert_eq!(shown_tab(&state), Some(1), "Cmd ] shows the next tab");
    }

    #[rstest::rstest]
    fn move_tab_left_swaps_it() {
        // Given a second tab shown, named logs.
        let mut state = two_tabs();
        state
            .layouts
            .rename_tab(SessionId(1), 1, Some("logs".to_owned()));

        // When moving it left.
        IntentHandler::handle(&Intent::MoveTabLeft, &mut state);

        // Then logs is the first tab, still shown.
        assert_eq!(
            (shown_tab(&state), shown_tab_name(&state).as_deref()),
            (Some(0), Some("logs")),
            "Cmd i moves the tab left"
        );
    }

    #[rstest::rstest]
    fn focus_pane_focuses_the_clicked_pane() {
        // Given shell pane 50 split off and focused, the sidebar with the keys.
        let mut state = split_layout();
        state.focus = Focus::Sidebar;

        // When clicking thread 1's pane.
        IntentHandler::handle(&Intent::FocusPane(PaneId(1)), &mut state);

        // Then thread 1's pane has the focus.
        assert_eq!(
            shown_focus(&state),
            Some(PaneId(1)),
            "the clicked pane is focused"
        );
    }

    #[rstest::rstest]
    fn focus_pane_moves_the_keys_into_the_panes() {
        // Given thread 1's layout shown while the sidebar has the keys.
        let mut state = with_layout(Focus::Sidebar);

        // When clicking thread 1's pane.
        IntentHandler::handle(&Intent::FocusPane(PaneId(1)), &mut state);

        // Then the keys are in the pane.
        assert_eq!(state.focus, Focus::Attached, "a click moves the keys in");
    }
}
