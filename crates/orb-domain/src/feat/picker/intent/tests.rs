use crate::command::{SearchCommand, SessionsCommand, Workspace, WorktreesCommand};
use crate::feat::git::git_service::{GitRef, WorktreeFacts};
use crate::feat::layout::state::{Layouts, SessionLayout, test_entry};
use crate::feat::layout::tree::Split;
use crate::feat::picker::list::{PickerItem, WorkspaceChoice};
use crate::feat::picker::state::{PickTarget, PickerKind, PickerState, SessionFilter};
use crate::feat::sessions::state::{
    FolderKind, PaneLaunch, ProjectKind, Session, SessionKind, Sessions, SidebarItem, SidebarRow,
    Thread, ThreadStatus, layouts_for, sessions_for,
};
use crate::feat::sessions::validator::SETTLE_IN_PROGRESS;
use crate::feat::sidebar::state::{Rename, RenameTarget};
use crate::ids::{PaneId, ProjectId, SessionId, ThreadId};
use crate::intent_handler::test_support::{
    ORPHAN, USED, agent, answer_yes, at, branch, choosing_workspace, close_thread_1s_pane,
    highlight, in_root, jumping, left_pane, left_pane_showing_1, managing, named,
    on_agent_in_another_tab, on_thread, open_sessions, picked_worktree, picking_thread_1,
    project_row, removing_alpha, rename_text, renaming, searching, settle, shown_tab_name,
    state_at, state_with, stopped, thread, two_projects, with_layout, with_projects,
    worktree_picker_after, worktree_state,
};
use crate::{AppState, Command, Focus, Intent, IntentHandler, TextInput};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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

/// The `New session` box for beta, outside git, holding `name`.
fn naming_non_git(name: &str) -> AppState {
    let mut state = picking_non_git();
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);
    type_name(&mut state, name);
    state
}

/// Types `name` into the open box.
fn type_name(state: &mut AppState, name: &str) {
    for ch in name.chars() {
        IntentHandler::handle(&Intent::PickerInput(ch), state);
    }
}

/// Alpha's new-session workspace picker, alpha holding a session in
/// `/wt/alpha-1`, named `name` in the `New session` box, with `choice`
/// highlighted.
fn choosing_named_workspace(choice: &WorkspaceChoice, name: &str) -> AppState {
    let mut state = picking(Focus::Sidebar);
    state.sessions.sessions = vec![session_in(5, 1, "/wt/alpha-1")];
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);
    type_name(&mut state, name);
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);
    highlight(&mut state, &PickerItem::Workspace(choice.clone()));
    state
}

/// Alpha's new-session workspace picker, alpha holding a session in
/// `/wt/alpha-1`, unnamed, with `choice` highlighted.
fn choosing_new_workspace(choice: &WorkspaceChoice) -> AppState {
    choosing_named_workspace(choice, "")
}

/// Alpha's base branch picker for a new worktree of a session named
/// `name`, listing `main`.
fn choosing_named_base(name: &str) -> AppState {
    let mut state = choosing_named_workspace(&WorkspaceChoice::NewWorktree, name);
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);
    if let Some(picker) = &mut state.picker {
        picker.show_branches(
            Path::new("/alpha"),
            vec![branch("main", true, Some("/alpha"))],
        );
    }
    state
}

/// Alpha's base branch picker for a new worktree, unnamed, listing
/// `main`.
fn choosing_base() -> AppState {
    choosing_named_base("")
}

#[rstest::rstest]
fn picking_a_project_opens_the_empty_new_session_box() {
    // Given the project picker opened from the sidebar with alpha
    // highlighted.
    let mut state = picking(Focus::Sidebar);

    // When handling PickerConfirm.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then the empty New session box for alpha takes the keys, and no
    // picker is open.
    assert_eq!(
        (
            state.focus,
            state.picker.is_none(),
            state
                .rename
                .map(|rename| (rename.target, rename.input.text().to_owned())),
        ),
        (
            Focus::Rename,
            true,
            Some((
                RenameTarget::NewSession {
                    project: ProjectId(1),
                    return_to: Focus::Sidebar,
                },
                String::new(),
            )),
        ),
        "picking a project asks for the session's name first"
    );
}

#[rstest::rstest]
fn naming_a_git_project_session_opens_the_workspace_picker() {
    // Given the New session box for alpha, a git project, holding `api`.
    let mut state = picking(Focus::Sidebar);
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);
    type_name(&mut state, "api");

    // When handling PickerConfirm.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then alpha's new-session workspace picker is open, carrying the
    // name.
    assert_eq!(
        state.picker.as_ref().map(PickerState::kind),
        Some(&PickerKind::Workspace {
            target: PickTarget::New {
                project: ProjectId(1),
                name: Some("api".to_owned()),
            }
        }),
        "a git project goes on to the workspace picker"
    );
}

#[rstest::rstest]
fn naming_a_session_outside_git_returns_new_session_with_the_name() {
    // Given the New session box for beta, outside git, holding ` api `.
    let mut state = naming_non_git(" api ");

    // When handling PickerConfirm.
    let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then a session named `api` in beta's checkout is asked for at once.
    assert_eq!(
        commands,
        vec![
            Command::Sessions(SessionsCommand::NewSession {
                project: ProjectId(2),
                workspace: Workspace::Checkout,
                name: Some("api".to_owned()),
                if_active: None,
            }),
            Command::Sessions(SessionsCommand::SaveJumps),
        ],
        "a project outside git gets its named session in its checkout"
    );
}

#[rstest::rstest]
fn blank_session_name_returns_unnamed_new_session() {
    // Given the New session box for beta, outside git, holding spaces.
    let mut state = naming_non_git("  ");

    // When handling PickerConfirm.
    let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then an unnamed session in beta's checkout is asked for.
    assert_eq!(
        commands,
        vec![
            Command::Sessions(SessionsCommand::NewSession {
                project: ProjectId(2),
                workspace: Workspace::Checkout,
                name: None,
                if_active: None,
            }),
            Command::Sessions(SessionsCommand::SaveJumps),
        ],
        "a blank name leaves the session unnamed"
    );
}

#[rstest::rstest]
fn cancelling_the_new_session_box_makes_no_session() {
    // Given the New session box for beta, outside git, holding `api`.
    let mut state = naming_non_git("api");

    // When handling PickerCancel.
    let commands = IntentHandler::handle(&Intent::PickerCancel, &mut state);

    // Then nothing is asked for, nothing is starting and no box or
    // picker is open.
    assert_eq!(
        (
            commands,
            state.sessions.starting,
            state.rename.is_none(),
            state.picker.is_none()
        ),
        (vec![], false, true, true),
        "Esc in the New session box cancels the new session"
    );
}

#[rstest::rstest]
#[case(Focus::Sidebar)]
#[case(Focus::Pane)]
fn cancelling_the_new_session_box_gives_the_keys_back(#[case] from: Focus) {
    // Given the New session box, the project picker opened from `from`.
    let mut state = picking(from);
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // When handling PickerCancel.
    IntentHandler::handle(&Intent::PickerCancel, &mut state);

    // Then the keys are back where `<C-g> n` was pressed.
    assert_eq!(state.focus, from, "Esc gives the keys back");
}

#[rstest::rstest]
fn picking_a_base_carries_the_session_name() {
    // Given alpha's base picker for a session named `api`, `main`
    // highlighted.
    let mut state = choosing_named_base("api");

    // When confirming.
    let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then a session named `api` in a new worktree from `main` is asked
    // for.
    assert!(
        commands.contains(&Command::Sessions(SessionsCommand::NewSession {
            project: ProjectId(1),
            workspace: Workspace::NewWorktree {
                base: "main".to_owned()
            },
            name: Some("api".to_owned()),
            if_active: None,
        })),
        "the name rides the workspace and base pickers, got {commands:?}"
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
        commands.contains(&Command::Sessions(SessionsCommand::NewSession {
            project: ProjectId(1),
            workspace: Workspace::Checkout,
            name: None,
            if_active: None,
        })),
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
        commands.contains(&Command::Sessions(SessionsCommand::NewSession {
            project: ProjectId(1),
            workspace: Workspace::Existing("/wt/alpha-1".into()),
            name: None,
            if_active: None,
        })),
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
            target: PickTarget::New {
                project: ProjectId(1),
                name: None,
            },
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
        commands.contains(&Command::Sessions(SessionsCommand::NewSession {
            project: ProjectId(1),
            workspace: Workspace::NewWorktree {
                base: "main".to_owned()
            },
            name: None,
            if_active: None,
        })),
        "a picked base should make a session in a new worktree, got {commands:?}"
    );
}

#[rstest::rstest]
fn new_session_pick_marks_starting() {
    // Given the New session box for beta, outside git.
    let mut state = naming_non_git("");

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
    // Given the New session box for beta, outside git, while a start is
    // in flight.
    let mut state = naming_non_git("");
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
        vec![Command::Sessions(SessionsCommand::AddProject(
            Path::new(HOME).join("dev")
        ))],
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
    // Given the project picker opened from a pane.
    let mut state = picking(Focus::Pane);

    // When handling PickerCancel.
    IntentHandler::handle(&Intent::PickerCancel, &mut state);

    // Then the picker is closed and the keys are back in the pane.
    assert_eq!(
        (state.focus, state.picker.is_none()),
        (Focus::Pane, true),
        "PickerCancel should close the picker and return to the pane"
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
        focus: Focus::Pane,
        ..state_with(vec![in_root(1), thread(2, ThreadStatus::Idle)], 1)
    };
    state.sessions.enter(SessionId(1));
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
        [Command::Sessions(SessionsCommand::ChangeWorkspace {
            session: SessionId(1),
            to: Workspace::Existing("/work/2".into()),
            if_active: Some(SessionId(1)),
        })],
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
        [Command::Sessions(SessionsCommand::ChangeWorkspace {
            session: SessionId(1),
            to: Workspace::NewWorktree {
                base: "main".into()
            },
            if_active: None,
        })],
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

/// The branch picker opened on session 1 of `threads`, showing `refs`.
fn choosing_branch(threads: Vec<Thread>, refs: Vec<GitRef>) -> AppState {
    let mut state = AppState {
        focus: Focus::Sidebar,
        ..state_with(threads, 1)
    };
    IntentHandler::handle(&Intent::SwitchBranch, &mut state);
    if let Some(picker) = &mut state.picker {
        picker.show_branches(Path::new("/work"), refs);
    }
    state
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
        [Command::Sessions(SessionsCommand::SwitchBranch {
            session: SessionId(1),
            git_ref: branch("feat", false, None),
            to_root: false,
            if_active: None,
        })],
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
        [Command::Sessions(SessionsCommand::ChangeWorkspace {
            session: SessionId(1),
            to: Workspace::Existing("/wt/feat".into()),
            if_active: None,
        })],
        "a session with no turn follows the branch into its worktree"
    );
}

/// The session active when each start in `commands` was asked for.
fn if_active_of(commands: &[Command]) -> Vec<Option<SessionId>> {
    commands
        .iter()
        .filter_map(|command| match command {
            Command::Sessions(
                SessionsCommand::NewSession { if_active, .. }
                | SessionsCommand::ChangeWorkspace { if_active, .. }
                | SessionsCommand::SwitchBranch { if_active, .. }
                | SessionsCommand::NewFolderSession { if_active, .. },
            ) => Some(*if_active),
            _ => None,
        })
        .collect()
}

#[rstest::rstest]
fn new_session_carries_the_session_active_when_asked() {
    // Given alpha's workspace picker on the current checkout, with
    // session 5 active.
    let mut state = choosing_new_workspace(&WorkspaceChoice::Current { worktree: false });
    state.sessions.enter(SessionId(5));

    // When confirming.
    let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then the new session opens only while session 5 is still active.
    assert_eq!(
        if_active_of(&commands),
        [Some(SessionId(5))],
        "a new session should carry the active session, got {commands:?}"
    );
}

#[rstest::rstest]
fn change_workspace_carries_the_session_active_when_asked() {
    // Given session 1's workspace picker on the previous worktree, with
    // session 2 active.
    let mut state = choosing_previous_worktree();
    state.sessions.enter(SessionId(2));

    // When confirming.
    let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then the move opens only while session 2 is still active.
    assert_eq!(
        if_active_of(&commands),
        [Some(SessionId(2))],
        "a move should carry the active session, got {commands:?}"
    );
}

#[rstest::rstest]
fn switch_branch_to_the_root_carries_the_session_active_when_asked() {
    // Given the branch picker of session 1, with no turn and outside the
    // checkout, on the default branch checked out nowhere, and session 2
    // active.
    let mut state = AppState {
        focus: Focus::Sidebar,
        ..state_with(
            vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
            1,
        )
    };
    IntentHandler::handle(&Intent::SwitchBranch, &mut state);
    if let Some(picker) = &mut state.picker {
        let main = GitRef {
            default: true,
            ..branch("main", false, None)
        };
        picker.show_branches(Path::new("/work/1"), vec![main]);
    }
    state.sessions.enter(SessionId(2));

    // When confirming.
    let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then the switch to the root opens only while session 2 is still
    // active.
    assert_eq!(
        if_active_of(&commands),
        [Some(SessionId(2))],
        "a switch to the root should carry the active session, got {commands:?}"
    );
}

#[rstest::rstest]
fn folder_session_carries_the_session_active_when_asked() {
    // Given the name box for a new Research session holding `notes`,
    // with session 1 active.
    let mut state = AppState {
        focus: Focus::Rename,
        rename: Some(Rename {
            target: RenameTarget::NewFolder(FolderKind::Research),
            input: TextInput::new("notes"),
            creating: false,
        }),
        ..state_with(vec![in_root(1)], 1)
    };
    state.sessions.enter(SessionId(1));

    // When confirming.
    let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then the folder session opens only while session 1 is still
    // active.
    assert_eq!(
        if_active_of(&commands),
        [Some(SessionId(1))],
        "a folder session should carry the active session, got {commands:?}"
    );
}

#[rstest::rstest]
fn picking_the_current_branch_returns_no_command() {
    // Given the branch picker with the current branch highlighted.
    let mut state = choosing_branch(vec![in_root(1)], vec![branch("main", true, Some("/work"))]);

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
        commands.contains(&Command::Sessions(SessionsCommand::SettleSession(
            SessionId(2)
        ))),
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
fn delete_confirm_returns_delete_session() {
    // Given one thread, selected, and Yes highlighted in its delete confirm.
    let mut state = state_with(vec![thread(1, ThreadStatus::Idle)], 1);
    answer_yes(&Intent::Delete, &mut state);

    // When confirming.
    let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then the sessions actor is asked to delete it.
    assert!(
        commands.contains(&Command::Sessions(SessionsCommand::DeleteSession(
            SessionId(1)
        ))),
        "Yes on the delete confirm should return Delete"
    );
}

#[rstest::rstest]
fn delete_selects_the_next_row_below() {
    // Given cards 2 and 1, then the open shelf holding thread 3, with card 1 selected, and Yes
    // highlighted in its delete confirm.
    let mut state = settle(
        state_with(
            vec![
                thread(1, ThreadStatus::Idle),
                thread(2, ThreadStatus::Idle),
                stopped(3),
            ],
            1,
        ),
        &[3],
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
        .sidebar(&state.layouts)
        .iter()
        .map(SidebarRow::item)
        .collect();
    assert_eq!(
        listed,
        vec![
            SidebarItem::Session(SessionId(2)),
            SidebarItem::Agent {
                session: SessionId(2),
                pane: PaneId(2),
            }
        ],
        "a deleted thread should leave the sidebar at once"
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
        vec![Command::Sessions(SessionsCommand::InitGit(ProjectId(1)))],
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
    poll(&mut state, &[stopped(1)]);
    let mut state = settle(state, &[1]);

    // When confirming.
    let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then it is not toggled back.
    assert!(
        commands.is_empty(),
        "Yes on an already settled thread should not un-settle it"
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

/// [`two_projects`] with the project filter opened from the sidebar.
fn filtering(filter: Option<i64>, cursor: SidebarItem) -> AppState {
    let mut state = two_projects(filter, cursor);
    IntentHandler::handle(&Intent::FilterProjects, &mut state);
    state
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
        vec![
            Command::Sessions(SessionsCommand::SaveUi),
            Command::Sessions(SessionsCommand::Visit(SessionId(21)))
        ],
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
    // Given the projects screen with alpha highlighted.
    // When handling PickerRemove.
    let state = removing_alpha(None, on_thread(21));

    // Then the remove confirm for alpha is open.
    assert_eq!(
        state.picker.as_ref().map(PickerState::kind),
        Some(&PickerKind::RemoveProject {
            project: ProjectId(1)
        }),
        "d on a project should ask to confirm its removal"
    );
}

#[rstest::rstest]
fn picker_remove_on_a_project_draws_the_confirm_over_the_list() {
    // Given the projects screen with alpha highlighted.
    // When handling PickerRemove.
    let state = removing_alpha(None, on_thread(21));

    // Then the confirm is drawn over the projects screen.
    assert_eq!(
        state
            .picker
            .as_ref()
            .and_then(PickerState::under)
            .map(PickerState::kind),
        Some(&PickerKind::ManageProjects),
        "the remove confirm should sit over the projects screen"
    );
}

#[rstest::rstest]
fn picker_remove_in_the_project_filter_does_nothing() {
    // Given the project filter with alpha highlighted.
    let mut state = filtering(None, on_thread(21));
    highlight(&mut state, &project_row(1, "alpha"));

    // When handling PickerRemove.
    IntentHandler::handle(&Intent::PickerRemove, &mut state);

    // Then the project filter stays open.
    assert_eq!(
        state.picker.as_ref().map(PickerState::kind),
        Some(&PickerKind::ProjectFilter),
        "the project filter only filters"
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
            Command::Sessions(SessionsCommand::RemoveProject(ProjectId(1))),
            Command::Sessions(SessionsCommand::SaveUi),
            Command::Sessions(SessionsCommand::Visit(SessionId(21))),
        ],
        "Yes should remove the project"
    );
}

#[rstest::rstest]
fn confirming_yes_drops_the_project_row() {
    // Given the remove confirm for alpha with Yes highlighted.
    let mut state = removing_alpha(None, on_thread(21));
    highlight(&mut state, &PickerItem::Confirm(true));

    // When handling PickerConfirm.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then the projects screen is back, without alpha.
    let picker = state.picker.as_ref();
    assert_eq!(
        (
            picker.map(PickerState::kind),
            picker.is_some_and(|picker| picker
                .shown()
                .any(|(item, _)| *item == project_row(1, "alpha"))),
        ),
        (Some(&PickerKind::ManageProjects), false),
        "Yes should go back to the projects screen without alpha"
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
fn confirming_no_returns_to_the_projects_screen() {
    // Given the remove confirm for alpha with No highlighted.
    let mut state = removing_alpha(None, on_thread(21));

    // When handling PickerConfirm.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then the projects screen is back, still listing alpha.
    let picker = state.picker.as_ref();
    assert_eq!(
        (
            picker.map(PickerState::kind),
            picker.is_some_and(|picker| picker
                .shown()
                .any(|(item, _)| *item == project_row(1, "alpha"))),
        ),
        (Some(&PickerKind::ManageProjects), true),
        "No should go back to the projects screen with alpha still listed"
    );
}

#[rstest::rstest]
fn cancelling_the_remove_confirm_returns_to_the_projects_screen() {
    // Given the remove confirm opened from the projects screen.
    let mut state = removing_alpha(None, on_thread(21));

    // When handling PickerCancel.
    IntentHandler::handle(&Intent::PickerCancel, &mut state);

    // Then the projects screen is back with the keys.
    assert_eq!(
        (state.picker.as_ref().map(PickerState::kind), state.focus),
        (Some(&PickerKind::ManageProjects), Focus::Picker),
        "Esc should go back to the projects screen"
    );
}

#[rstest::rstest]
fn open_projects_opens_the_projects_screen() {
    // Given two projects and the keys in the sidebar.
    let mut state = two_projects(None, on_thread(21));

    // When handling OpenProjects.
    IntentHandler::handle(&Intent::OpenProjects, &mut state);

    // Then the projects screen takes the keys.
    assert_eq!(
        (state.focus, state.picker.as_ref().map(PickerState::kind)),
        (Focus::Picker, Some(&PickerKind::ManageProjects)),
        "<C-g> P should open the projects screen"
    );
}

#[rstest::rstest]
fn open_projects_lists_projects_in_new_session_order() {
    // Given two projects, and the rows the new-session picker lists.
    let mut state = two_projects(None, on_thread(21));
    let rows = |state: &AppState| -> Vec<PickerItem> {
        state
            .picker
            .iter()
            .flat_map(PickerState::shown)
            .map(|(item, _)| item.clone())
            .collect()
    };
    let new_session_rows = {
        let mut picking = two_projects(None, on_thread(21));
        IntentHandler::handle(&Intent::NewSession, &mut picking);
        rows(&picking)
    };

    // When handling OpenProjects.
    IntentHandler::handle(&Intent::OpenProjects, &mut state);

    // Then it lists the same projects in the same order.
    assert_eq!(
        rows(&state),
        new_session_rows,
        "the projects screen should list projects as <C-g> n does"
    );
}

/// [`managing`] at home `~/`, then `a`, with the directory picker listing
/// `dev`.
fn adding_from_projects() -> AppState {
    let mut state = AppState {
        home: HOME.into(),
        ..managing(None, on_thread(21))
    };
    IntentHandler::handle(&Intent::AddProject, &mut state);
    if let Some(picker) = &mut state.picker {
        picker.show_directories(Path::new(HOME), vec!["dev".to_owned()]);
    }
    state
}

#[rstest::rstest]
fn add_project_in_the_projects_screen_opens_the_directory_picker_over_it() {
    // Given the projects screen.
    let mut state = managing(None, on_thread(21));

    // When handling AddProject.
    IntentHandler::handle(&Intent::AddProject, &mut state);

    // Then the directory picker is drawn over the projects screen.
    let picker = state.picker.as_ref();
    assert!(
        matches!(
            (
                picker.map(PickerState::kind),
                picker.and_then(PickerState::under).map(PickerState::kind),
            ),
            (
                Some(PickerKind::Directories { .. }),
                Some(PickerKind::ManageProjects)
            )
        ),
        "a should open the directory picker over the projects screen"
    );
}

#[rstest::rstest]
fn confirming_a_directory_over_the_projects_screen_returns_add_project() {
    // Given the directory picker over the projects screen, `dev`
    // highlighted.
    let mut state = adding_from_projects();

    // When handling PickerConfirm.
    let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then the sessions actor is asked to add `~/dev`.
    assert_eq!(
        commands,
        vec![Command::Sessions(SessionsCommand::AddProject(
            Path::new(HOME).join("dev")
        ))],
        "confirming a directory should add it as a project"
    );
}

#[rstest::rstest]
fn confirming_a_directory_over_the_projects_screen_returns_to_the_list() {
    // Given the directory picker over the projects screen, `dev`
    // highlighted.
    let mut state = adding_from_projects();

    // When handling PickerConfirm.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then the projects screen is back with the keys.
    assert_eq!(
        (state.picker.as_ref().map(PickerState::kind), state.focus),
        (Some(&PickerKind::ManageProjects), Focus::Picker),
        "adding a directory should go back to the projects screen"
    );
}

#[rstest::rstest]
fn confirming_a_project_in_the_projects_screen_opens_the_new_session_box() {
    // Given the projects screen with alpha highlighted.
    let mut state = managing(None, on_thread(21));
    highlight(&mut state, &project_row(1, "alpha"));

    // When handling PickerConfirm.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then the New session box for alpha takes the keys.
    assert!(
        state.focus == Focus::Rename
            && matches!(
                state.rename.as_ref().map(|rename| &rename.target),
                Some(RenameTarget::NewSession {
                    project: ProjectId(1),
                    ..
                })
            ),
        "⏎ in the projects screen should ask for a new session's name"
    );
}

#[rstest::rstest]
fn cancelling_the_directory_picker_over_the_projects_screen_returns_to_the_list() {
    // Given the directory picker over the projects screen.
    let mut state = adding_from_projects();

    // When handling PickerCancel.
    IntentHandler::handle(&Intent::PickerCancel, &mut state);

    // Then the projects screen is back with the keys.
    assert_eq!(
        (state.picker.as_ref().map(PickerState::kind), state.focus),
        (Some(&PickerKind::ManageProjects), Focus::Picker),
        "Esc should go back to the projects screen"
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
        vec![Command::Sessions(SessionsCommand::SaveUi)],
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
        vec![Command::Sessions(SessionsCommand::RenameSession {
            session: SessionId(1),
            name: Some("Sidebar search".to_owned()),
        })],
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
        vec![Command::Sessions(SessionsCommand::RenameSession {
            session: SessionId(1),
            name: None,
        })],
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
        vec![Command::Sessions(SessionsCommand::Visit(SessionId(3)))],
        "typing should visit the first match"
    );
}

#[rstest::rstest]
fn search_next_moves_to_the_next_match() {
    // Given "fix" matching threads 3 and 1, with the cursor on thread 3's
    // agent row.
    let mut state = searching("fix", Some(3));
    state.sessions.cursor = Some(SidebarItem::Agent {
        session: SessionId(3),
        pane: PaneId(3),
    });

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

    // Then the cursor is on thread 3's agent row, past the unmatched
    // thread 2.
    assert_eq!(
        state.sessions.cursor,
        Some(SidebarItem::Agent {
            session: SessionId(3),
            pane: PaneId(3),
        }),
        "<C-k> should move to the previous match"
    );
}

#[rstest::rstest]
fn search_next_moves_from_a_card_to_its_agent_row() {
    // Given "fix" matching threads 3 and 1, with the cursor on thread 3's
    // card.
    let mut state = searching("fix", Some(3));

    // When handling PickerNext (`<C-j>`).
    IntentHandler::handle(&Intent::PickerNext, &mut state);

    // Then the cursor is on thread 3's agent row.
    assert_eq!(
        state.sessions.cursor,
        Some(SidebarItem::Agent {
            session: SessionId(3),
            pane: PaneId(3),
        }),
        "<C-j> should stop on the card's agent row"
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
        commands.contains(&Command::Sessions(SessionsCommand::NewFolderSession {
            kind: FolderKind::Research,
            name: "GT-514-login".into(),
            if_active: None,
        })),
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
fn confirming_a_folder_name_while_another_session_starts_asks_for_nothing() {
    // Given the name box holding a fresh name, and another session
    // being made.
    let mut state = naming_research("tokio select");
    state.sessions.starting = true;

    // When handling PickerConfirm.
    let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then nothing is asked for, as for any other start meanwhile.
    assert_eq!(
        commands,
        vec![],
        "a folder session should wait for the start in flight"
    );
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
    // Given the New session box for beta, outside git, opened from
    // alpha's thread 11.
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
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // When confirming the name.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then thread 11 is recorded; the new session's row follows when the
    // frontend attaches it.
    assert_eq!(
        state.jumps.entries(),
        [on_thread(11)],
        "a <C-g> n pick should record where it left"
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
    state.layouts = layouts_for(&state.sessions.projects);
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
    let state = settle(
        state_at(
            vec![stopped(1), thread(2, ThreadStatus::Idle)],
            SidebarItem::SettledShelf,
        ),
        &[1],
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
fn filter_settled_lists_a_settled_thread() {
    // Given the session picker over settled thread 1 and idle thread 2.
    let mut state = open_sessions(settle(
        state_at(
            vec![stopped(1), thread(2, ThreadStatus::Idle)],
            SidebarItem::SettledShelf,
        ),
        &[1],
    ));

    // When handling PickerFilter(Settled).
    IntentHandler::handle(&Intent::PickerFilter(SessionFilter::Settled), &mut state);

    // Then thread 1 is listed.
    assert!(
        session_ids(&state).contains(&SessionId(1)),
        "the settled filter should list settled threads"
    );
}

/// The kind of the open picker.
fn picker_kind(state: &AppState) -> Option<PickerKind> {
    state.picker.as_ref().map(|picker| picker.kind().clone())
}

#[rstest::rstest]
fn pressing_the_active_filter_again_lists_all() {
    // Given the session picker filtered to blocked threads.
    let mut state = open_sessions(state_at(
        vec![thread(1, ThreadStatus::Idle)],
        SidebarItem::SettledShelf,
    ));
    IntentHandler::handle(&Intent::PickerFilter(SessionFilter::Blocked), &mut state);

    // When handling PickerFilter(Blocked) again.
    IntentHandler::handle(&Intent::PickerFilter(SessionFilter::Blocked), &mut state);

    // Then the picker is back on all.
    assert_eq!(
        picker_kind(&state),
        Some(PickerKind::Sessions {
            filter: SessionFilter::All
        }),
        "the same filter twice should go back to all"
    );
}

#[rstest::rstest]
fn reopening_the_session_picker_starts_on_all() {
    // Given the session picker filtered to settled threads, then closed.
    let mut state = open_sessions(state_at(
        vec![thread(1, ThreadStatus::Idle)],
        SidebarItem::SettledShelf,
    ));
    IntentHandler::handle(&Intent::PickerFilter(SessionFilter::Settled), &mut state);
    IntentHandler::handle(&Intent::PickerCancel, &mut state);
    IntentHandler::handle(&Intent::PickerCancel, &mut state);

    // When opening the session picker again.
    let state = open_sessions(state);

    // Then it starts on all.
    assert_eq!(
        picker_kind(&state),
        Some(PickerKind::Sessions {
            filter: SessionFilter::All
        }),
        "every opening should start on all"
    );
}

#[rstest::rstest]
fn lone_thread_row_is_labelled_project_session_and_title() {
    // Given thread 1, titled `fix login`, in project `work`.
    let state = state_at(
        vec![named(thread(1, ThreadStatus::Idle), "fix login")],
        SidebarItem::SettledShelf,
    );

    // When opening the session picker.
    let state = open_sessions(state);

    // Then its row reads `work/fix login/fix login`: the session is
    // named by its agent too.
    assert_eq!(
        session_labels(&state),
        ["work/fix login/fix login"],
        "a lone thread is labelled by its project"
    );
}

#[rstest::rstest]
fn incognito_thread_row_is_labelled_incognito_session_and_title() {
    // Given thread 1, titled `scratch`, in orb's Incognito project.
    let mut state = with_projects(&["Incognito"]);
    if let Some(project) = state.sessions.projects.first_mut() {
        project.kind = ProjectKind::Incognito;
        project.threads = vec![named(thread(1, ThreadStatus::Idle), "scratch")];
    }
    state.sessions.sessions = sessions_for(&state.sessions.projects);
    state.layouts = layouts_for(&state.sessions.projects);

    // When opening the session picker.
    let state = open_sessions(state);

    // Then its row reads `Incognito/scratch/scratch`.
    assert_eq!(
        session_labels(&state),
        ["Incognito/scratch/scratch"],
        "an Incognito thread is labelled Incognito"
    );
}

#[rstest::rstest]
fn untitled_session_row_ends_in_its_folder_and_new_thread() {
    // Given untitled thread 1 in project `work`, running in /work/1.
    let state = state_at(
        vec![thread(1, ThreadStatus::Idle)],
        SidebarItem::SettledShelf,
    );

    // When opening the session picker.
    let state = open_sessions(state);

    // Then its row reads `work/1/New thread`.
    assert_eq!(
        session_labels(&state),
        ["work/1/New thread"],
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
    let state = jumping(Focus::Pane, &[2], &[]);

    // When opening the session picker.
    let state = open_sessions(state);

    // Then closing it would return to the pane.
    assert_eq!(
        state.picker.as_ref().map(PickerState::return_to),
        Some(Focus::Pane),
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
fn filter_with_text_typed_lists_the_matching_settled_thread() {
    // Given the session picker over settled `alpha` (1) and `beta` (2),
    // with `alpha` typed, which shows nothing.
    let state = settle(
        state_at(
            vec![
                named(stopped(1), "alpha"),
                named(thread(2, ThreadStatus::Idle), "beta"),
            ],
            SidebarItem::SettledShelf,
        ),
        &[1],
    );
    let mut state = typed(open_sessions(state), &['a', 'l', 'p', 'h', 'a']);

    // When handling PickerFilter(Settled).
    IntentHandler::handle(&Intent::PickerFilter(SessionFilter::Settled), &mut state);

    // Then only `alpha` is shown, still filtered by the typed text.
    assert_eq!(
        session_ids(&state),
        [SessionId(1)],
        "filtering should keep the typed text"
    );
}

#[rstest::rstest]
fn filter_in_another_picker_changes_nothing() {
    // Given the project picker over alpha and beta.
    let mut state = picking(Focus::Sidebar);

    // When handling PickerFilter(Settled).
    IntentHandler::handle(&Intent::PickerFilter(SessionFilter::Settled), &mut state);

    // Then it's still the project picker over both projects.
    assert_eq!(
        state
            .picker
            .as_ref()
            .map(|picker| (picker.kind().clone(), picker.total())),
        Some((PickerKind::Projects, 2)),
        "other pickers should ignore the filter"
    );
}

/// The kinds of the open picker and the list under it.
fn picker_and_under(state: &AppState) -> Option<(PickerKind, Option<PickerKind>)> {
    state.picker.as_ref().map(|picker| {
        (
            picker.kind().clone(),
            picker.under().map(|under| under.kind().clone()),
        )
    })
}

/// The session picker's kind on all threads.
fn all_sessions() -> PickerKind {
    PickerKind::Sessions {
        filter: SessionFilter::All,
    }
}

#[rstest::rstest]
fn d_in_the_session_picker_opens_close_pane_over_the_list() {
    // Given the session picker on thread 1.
    let mut state = picking_thread_1();

    // When handling PickerRemove.
    IntentHandler::handle(&Intent::PickerRemove, &mut state);

    // Then thread 1's close confirm is open over the session picker.
    assert_eq!(
        picker_and_under(&state),
        Some((
            PickerKind::ClosePane {
                pane: PaneId(1),
                session: SessionId(1),
            },
            Some(all_sessions()),
        )),
        "d should ask to close the agent over the list"
    );
}

#[rstest::rstest]
fn d_on_a_gone_session_in_the_session_picker_does_nothing() {
    // Given the session picker on thread 1, whose session is being
    // deleted.
    let mut state = picking_thread_1();
    state.sessions.deleting.insert(SessionId(1));

    // When handling PickerRemove.
    IntentHandler::handle(&Intent::PickerRemove, &mut state);

    // Then the session picker stays open with nothing over it.
    assert_eq!(
        picker_and_under(&state),
        Some((all_sessions(), None)),
        "d on a gone session should do nothing"
    );
}

#[rstest::rstest]
fn yes_over_the_session_picker_returns_save_layout() {
    // Given Yes highlighted in thread 1's close confirm over the list.
    let mut state = picking_thread_1();
    answer_yes(&Intent::PickerRemove, &mut state);

    // When confirming.
    let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then session 1's layout, without the pane, is saved.
    assert!(
        commands.contains(&Command::Sessions(SessionsCommand::SaveLayout(SessionId(
            1
        )))),
        "Yes over the list should save the layout it closed a pane of"
    );
}

#[rstest::rstest]
fn yes_over_the_session_picker_drops_the_row() {
    // Given Yes highlighted in thread 1's close confirm over the list.
    let mut state = picking_thread_1();
    answer_yes(&Intent::PickerRemove, &mut state);

    // When confirming.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then the session list is back without thread 1's row.
    assert_eq!(
        (picker_kind(&state), session_ids(&state)),
        (Some(all_sessions()), vec![SessionId(2)]),
        "the closed agent's row should go"
    );
}

#[rstest::rstest]
fn yes_over_the_session_picker_leaves_the_sidebar_cursor_elsewhere() {
    // Given the sidebar cursor on thread 2, and Yes highlighted in
    // thread 1's close confirm over the list.
    let mut state = picking_thread_1();
    answer_yes(&Intent::PickerRemove, &mut state);

    // When confirming.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then the sidebar cursor stays on thread 2.
    assert_eq!(
        state.sessions.cursor,
        Some(SidebarItem::Session(SessionId(2))),
        "a cursor on another session should stay put"
    );
}

#[rstest::rstest]
fn yes_over_the_session_picker_after_the_pane_closed_returns_to_the_list() {
    // Given Yes highlighted in thread 1's close confirm over the list,
    // and then thread 1's pane closes.
    let mut state = picking_thread_1();
    answer_yes(&Intent::PickerRemove, &mut state);
    close_thread_1s_pane(&mut state);

    // When confirming.
    let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then nothing is asked, and the list is back with its row.
    assert_eq!(
        (commands, picker_kind(&state), session_ids(&state)),
        (
            vec![],
            Some(all_sessions()),
            vec![SessionId(2), SessionId(1)]
        ),
        "Yes on a closed pane should only go back to the list"
    );
}

#[rstest::rstest]
fn yes_over_the_session_picker_closes_only_that_agent() {
    // Given session 1 running thread 1 in pane 1 and thread 2 in pane 2,
    // and Yes highlighted in thread 1's close confirm over the list.
    let mut state = state_with(
        vec![
            thread(1, ThreadStatus::Idle),
            Thread {
                pane: Some(PaneLaunch {
                    pane: PaneId(2),
                    session: SessionId(1),
                }),
                ..thread(2, ThreadStatus::Idle)
            },
        ],
        1,
    );
    state.layouts = {
        let mut layouts = Layouts::default();
        layouts.insert(SessionId(1), SessionLayout::of(test_entry(1)));
        layouts.split(SessionId(1), Split::Right, test_entry(2));
        layouts
    };
    let mut state = open_sessions(state);
    highlight(
        &mut state,
        &PickerItem::Session {
            id: SessionId(1),
            thread: ThreadId(1),
            pane: PaneId(1),
            label: "work/1/New thread".into(),
            split: 5,
            settled: false,
        },
    );
    answer_yes(&Intent::PickerRemove, &mut state);

    // When confirming.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then pane 1 is gone and its sibling pane 2 stays.
    assert_eq!(
        (
            state.layouts.entry(PaneId(1)).is_some(),
            state.layouts.entry(PaneId(2)).is_some()
        ),
        (false, true),
        "Yes should close only the listed agent's pane"
    );
}

#[rstest::rstest]
fn no_over_the_session_picker_returns_to_the_list() {
    // Given No highlighted in thread 1's close confirm over the list.
    let mut state = picking_thread_1();
    IntentHandler::handle(&Intent::PickerRemove, &mut state);
    highlight(&mut state, &PickerItem::Confirm(false));

    // When confirming.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then the session list is back with both rows.
    assert_eq!(
        (picker_kind(&state), session_ids(&state)),
        (Some(all_sessions()), vec![SessionId(2), SessionId(1)]),
        "No should go back to the list as it was"
    );
}

#[rstest::rstest]
fn picking_an_agent_moves_the_cursor_to_its_row() {
    // Given the session picker on thread 1, opened on thread 2.
    let mut state = picking_thread_1();

    // When confirming.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then the cursor is on thread 1's agent row.
    assert_eq!(
        state.sessions.cursor,
        Some(agent_row_1()),
        "picking should select the agent's row"
    );
}

/// Thread 1's agent row: pane 1 of session 1.
fn agent_row_1() -> SidebarItem {
    SidebarItem::Agent {
        session: SessionId(1),
        pane: PaneId(1),
    }
}

#[rstest::rstest]
fn picking_an_agent_reveals_its_pane() {
    // Given the session picker on thread 1, opened on thread 2.
    let mut state = picking_thread_1();

    // When confirming.
    let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then session 1's layout, showing the pane, is saved.
    assert!(
        commands.contains(&Command::Sessions(SessionsCommand::SaveLayout(SessionId(
            1
        )))),
        "picking should save the layout it revealed the pane in"
    );
}

#[rstest::rstest]
fn picking_an_agent_whose_pane_closed_closes_the_picker() {
    // Given thread 1 highlighted, its pane closed since the picker opened.
    let mut state = picking_thread_1();
    close_thread_1s_pane(&mut state);

    // When confirming.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then the picker is closed.
    assert!(state.picker.is_none(), "the picker should close");
}

#[rstest::rstest]
fn picking_an_agent_whose_pane_closed_returns_no_commands() {
    // Given thread 1 highlighted, its pane closed since the picker opened.
    let mut state = picking_thread_1();
    close_thread_1s_pane(&mut state);

    // When confirming.
    let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then nothing happens.
    assert!(commands.is_empty(), "a closed pane can't be jumped into");
}

#[rstest::rstest]
fn picking_an_agent_attaches_its_session() {
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
fn picking_an_agent_focuses_the_pane() {
    // Given the session picker on thread 1, opened on thread 2.
    let mut state = picking_thread_1();

    // When confirming.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then the pane takes the keys.
    assert_eq!(state.focus, Focus::Pane, "picking should focus the pane");
}

#[rstest::rstest]
fn picking_an_agent_records_the_row_left_then_the_agent() {
    // Given the session picker on thread 1, opened on thread 2.
    let mut state = picking_thread_1();

    // When confirming.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then thread 2, then thread 1's agent, are recorded.
    assert_eq!(
        state.jumps.entries(),
        [on_thread(2), agent(1, 1)],
        "picking should record where it left and the agent it landed in"
    );
}

#[rstest::rstest]
fn picking_the_selected_thread_records_no_jump() {
    // Given the session picker on thread 2, opened on thread 2's agent
    // row.
    let mut state = jumping(Focus::Sidebar, &[], &[]);
    state.sessions.cursor = Some(SidebarItem::Agent {
        session: SessionId(2),
        pane: PaneId(2),
    });
    let mut state = open_sessions(state);

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
#[case::last(Intent::PickerLast, 3)]
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
    open_sessions(settle(
        state_at(
            vec![
                with_transcript(Thread {
                    last_activity_at: at(secs),
                    ..stopped(1)
                }),
                with_transcript(ended_at(2, 10)),
            ],
            SidebarItem::SettledShelf,
        ),
        &[1],
    ))
}

#[rstest::rstest]
fn filter_settled_loads_the_newly_selected_threads_preview() {
    // Given settled thread 1, last active at 30 s, hidden, and thread 2
    // selected.
    let mut state = settled_last_active_at(30);

    // When handling PickerFilter(Settled).
    let commands = IntentHandler::handle(&Intent::PickerFilter(SessionFilter::Settled), &mut state);

    // Then thread 1's preview is loaded.
    assert_eq!(commands, [load(1)], "the new selection should be loaded");
}

#[rstest::rstest]
fn filter_that_keeps_the_selection_loads_no_preview() {
    // Given settled thread 1 hidden, and idle thread 2 selected.
    let mut state = settled_last_active_at(5);

    // When handling PickerFilter(Idle), which still lists thread 2.
    let commands = IntentHandler::handle(&Intent::PickerFilter(SessionFilter::Idle), &mut state);

    // Then nothing is loaded.
    assert!(commands.is_empty(), "the same selection needs no new read");
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
        "<C-g> W should open the worktree picker"
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
        [Command::Worktrees(WorktreesCommand::RefreshWorktrees)],
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
/// `d` pressed on it.
fn delete_row(state: &mut AppState, row: usize) {
    IntentHandler::handle(&Intent::OpenWorktreePicker, state);
    IntentHandler::handle(&Intent::PickerSelectRow(row), state);
    IntentHandler::handle(&Intent::PickerRemove, state);
}

#[rstest::rstest]
fn remove_on_an_attached_row_sets_the_refusal() {
    // Given thread 1, in `USED`, attached.
    let mut state = AppState {
        attached: [SessionId(1)].into(),
        ..worktree_state()
    };

    // When pressing d on `USED`'s row.
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

    // When pressing d on `USED`'s row.
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

    // When pressing d on the orphan's row.
    delete_row(&mut state, 1);

    // Then the clean delete confirm is open for it.
    assert_eq!(
        state.picker.as_ref().map(PickerState::kind),
        Some(&PickerKind::DeleteWorktree {
            path: ORPHAN.into(),
            dirty: false,
        }),
        "d should ask to confirm the delete"
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

    // When pressing d on the orphan's row.
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
        [Command::Worktrees(WorktreesCommand::DeleteWorktree {
            path: USED.into()
        })],
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

#[rstest::rstest]
fn picker_start_search_moves_the_keys_to_the_input() {
    // Given the worktree picker open in list mode.
    let mut state = worktree_picker_after(&[]);

    // When handling PickerStartSearch.
    IntentHandler::handle(&Intent::PickerStartSearch, &mut state);

    // Then the picker is in search mode.
    assert!(
        state.picker.as_ref().is_some_and(PickerState::searching),
        "/ should enter search mode"
    );
}

#[rstest::rstest]
fn picker_cancel_in_search_mode_returns_to_the_list() {
    // Given the worktree picker in search mode with `o` typed.
    let mut state = worktree_picker_after(&[Intent::PickerStartSearch, Intent::PickerInput('o')]);

    // When handling PickerCancel.
    IntentHandler::handle(&Intent::PickerCancel, &mut state);

    // Then the picker is open in list mode with the keys.
    assert!(
        state.picker.as_ref().is_some_and(PickerState::listing) && state.focus == Focus::Picker,
        "Esc in search mode should go back to the list"
    );
}

#[rstest::rstest]
fn picker_cancel_in_search_mode_keeps_the_typed_text() {
    // Given the worktree picker in search mode with `o` typed.
    let mut state = worktree_picker_after(&[Intent::PickerStartSearch, Intent::PickerInput('o')]);

    // When handling PickerCancel.
    IntentHandler::handle(&Intent::PickerCancel, &mut state);

    // Then the typed text is still there.
    assert_eq!(
        state.picker.as_ref().map(PickerState::input),
        Some("o"),
        "Esc in search mode should keep the filter"
    );
}

#[rstest::rstest]
fn picker_cancel_in_search_mode_with_nothing_typed_returns_to_the_list() {
    // Given the worktree picker in search mode with nothing typed.
    let mut state = worktree_picker_after(&[Intent::PickerStartSearch]);

    // When handling PickerCancel.
    IntentHandler::handle(&Intent::PickerCancel, &mut state);

    // Then the picker is open in list mode.
    assert!(
        state.picker.as_ref().is_some_and(PickerState::listing),
        "Esc in an empty search should go back to the list"
    );
}

#[rstest::rstest]
fn picker_cancel_in_list_mode_closes_the_picker() {
    // Given the worktree picker open in list mode from the sidebar.
    let mut state = worktree_picker_after(&[]);

    // When handling PickerCancel.
    IntentHandler::handle(&Intent::PickerCancel, &mut state);

    // Then the picker is closed and the sidebar has the keys back.
    assert!(
        state.picker.is_none() && state.focus == Focus::Sidebar,
        "Esc in list mode should close the picker"
    );
}

#[rstest::rstest]
fn g_twice_selects_the_first_row() {
    // Given the worktree picker on its second row.
    let mut state = worktree_picker_after(&[Intent::PickerNext, Intent::PickerG]);

    // When handling the second PickerG.
    IntentHandler::handle(&Intent::PickerG, &mut state);

    // Then the first row is selected.
    assert_eq!(
        picked_worktree(&state),
        Some(PathBuf::from(USED)),
        "gg should select the first row"
    );
}

#[rstest::rstest]
fn picker_last_selects_the_last_row() {
    // Given the worktree picker on its first row.
    let mut state = worktree_picker_after(&[]);

    // When handling PickerLast.
    IntentHandler::handle(&Intent::PickerLast, &mut state);

    // Then the last row is selected.
    assert_eq!(
        picked_worktree(&state),
        Some(PathBuf::from(ORPHAN)),
        "G should select the last row"
    );
}

#[rstest::rstest]
fn click_on_a_list_pickers_input_enters_search_mode() {
    // Given the worktree picker open in list mode.
    let mut state = worktree_picker_after(&[]);

    // When clicking its input box.
    IntentHandler::handle(&Intent::PickerCursorTo(0), &mut state);

    // Then the picker is in search mode.
    assert!(
        state.picker.as_ref().is_some_and(PickerState::searching),
        "a click on the input should enter search mode"
    );
}

#[rstest::rstest]
fn click_on_a_filter_pickers_input_keeps_it_filtering() {
    // Given the project picker open.
    let mut state = picking(Focus::Sidebar);

    // When clicking its input box.
    IntentHandler::handle(&Intent::PickerCursorTo(0), &mut state);

    // Then the picker neither searches nor lists.
    assert!(
        state
            .picker
            .as_ref()
            .is_some_and(|picker| !picker.searching() && !picker.listing()),
        "a click should leave a type-to-filter picker as it was"
    );
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
        [Command::Search(SearchCommand::LoadSearchPreview {
            hit: 2,
            path: format!("/t/{thread}.jsonl").into(),
            prompt_offset: 0,
        })],
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
        "<C-g> / should open the search picker"
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
        [Command::Search(SearchCommand::SearchTranscripts {
            query: String::new()
        })],
        "opening should search for nothing"
    );
}

#[rstest::rstest]
#[case::input(Intent::PickerInput('x'), "fix")]
#[case::backspace(Intent::PickerBackspace, "f")]
#[case::delete_word(Intent::PickerDeleteWord, "")]
fn editing_the_search_input_returns_a_search_for_it(#[case] intent: Intent, #[case] query: &str) {
    // Given the search picker with `fi` typed.
    let mut state = jumping(Focus::Sidebar, &[], &[]);
    IntentHandler::handle(&Intent::OpenSearch, &mut state);
    let mut state = typed(state, &['f', 'i']);

    // When editing the typed text.
    let commands = IntentHandler::handle(&intent, &mut state);

    // Then the new text is searched for.
    assert_eq!(
        commands,
        [Command::Search(SearchCommand::SearchTranscripts {
            query: query.to_owned()
        })],
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
        !commands.iter().any(|command| matches!(
            command,
            Command::Search(SearchCommand::SearchTranscripts { .. })
        )),
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
    assert_eq!(state.focus, Focus::Pane, "the panes get the keys back");
}

#[rstest::rstest]
fn rename_tab_cancel_puts_the_keys_back_in_the_panes() {
    // Given the rename box on the first tab holding "logs".
    let mut state = renaming_tab("logs");

    // When cancelling.
    IntentHandler::handle(&Intent::PickerCancel, &mut state);

    // Then the keys are back in the pane.
    assert_eq!(state.focus, Focus::Pane, "the panes get the keys back");
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
        commands.contains(&Command::Sessions(SessionsCommand::SaveLayout(SessionId(
            1
        )))),
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
    assert_eq!(state.focus, Focus::Pane, "the panes get the keys back");
}

/// The rename box opened from the agent row of pane 1, holding `name`.
fn renaming_agent(name: &str) -> AppState {
    let mut state = on_agent_in_another_tab();
    state.focus = Focus::Rename;
    state.rename = Some(Rename {
        target: RenameTarget::Agent(PaneId(1)),
        input: TextInput::new(name),
        creating: false,
    });
    state
}

#[rstest::rstest]
fn rename_agent_confirm_names_the_pane() {
    // Given the agent's rename box holding "api".
    let mut state = renaming_agent("api");

    // When confirming.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then pane 1 is named api.
    assert_eq!(
        state
            .layouts
            .entry(PaneId(1))
            .and_then(|entry| entry.name.clone()),
        Some("api".to_owned()),
        "the agent's pane takes the name"
    );
}

#[rstest::rstest]
fn rename_agent_confirm_gives_the_sidebar_the_keys() {
    // Given the agent's rename box holding "api".
    let mut state = renaming_agent("api");

    // When confirming.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then the sidebar has the keys.
    assert_eq!(state.focus, Focus::Sidebar, "the keys stay in the sidebar");
}

#[rstest::rstest]
fn rename_agent_confirm_saves_the_layout() {
    // Given the agent's rename box holding "api".
    let mut state = renaming_agent("api");

    // When confirming.
    let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then session 1's layout is saved.
    assert_eq!(
        commands,
        vec![Command::Sessions(SessionsCommand::SaveLayout(SessionId(1)))],
        "an agent's new name should be saved"
    );
}

#[rstest::rstest]
fn rename_agent_cancel_gives_the_sidebar_the_keys() {
    // Given the agent's rename box holding "api".
    let mut state = renaming_agent("api");

    // When cancelling.
    IntentHandler::handle(&Intent::PickerCancel, &mut state);

    // Then the sidebar has the keys.
    assert_eq!(state.focus, Focus::Sidebar, "the keys stay in the sidebar");
}

/// The agent row of pane 1 (in tab 1, beside tab 2's pane 60), with Yes
/// highlighted in its close confirm.
fn closing_agent() -> AppState {
    let mut state = on_agent_in_another_tab();
    answer_yes(&Intent::Delete, &mut state);
    state
}

#[rstest::rstest]
fn close_pane_confirm_yes_closes_the_pane() {
    // Given Yes highlighted in pane 1's close confirm.
    let mut state = closing_agent();

    // When confirming.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then pane 1 is gone from the layout.
    assert_eq!(
        state.layouts.owner_of(PaneId(1)),
        None,
        "Yes should close the agent's pane"
    );
}

#[rstest::rstest]
fn closing_an_agent_pane_removes_its_row_at_once() {
    // Given Yes highlighted in pane 1's close confirm, with no poll to
    // follow.
    let mut state = closing_agent();

    // When confirming.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then the sidebar has no agent row for pane 1.
    let rows: Vec<PaneId> = state
        .sessions
        .sidebar(&state.layouts)
        .iter()
        .filter_map(|row| match row {
            SidebarRow::Agent { pane, .. } if *pane == PaneId(1) => Some(*pane),
            _ => None,
        })
        .collect();
    assert_eq!(
        rows,
        Vec::<PaneId>::new(),
        "a closed agent pane's row should go before the next poll"
    );
}

#[rstest::rstest]
fn close_pane_confirm_yes_saves_the_layout() {
    // Given Yes highlighted in pane 1's close confirm.
    let mut state = closing_agent();

    // When confirming.
    let commands = IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then session 1's layout is saved.
    assert_eq!(
        commands,
        vec![Command::Sessions(SessionsCommand::SaveLayout(SessionId(1)))],
        "a closed agent pane should be saved"
    );
}

#[rstest::rstest]
fn close_pane_confirm_yes_moves_the_cursor_to_the_card() {
    // Given Yes highlighted in pane 1's close confirm.
    let mut state = closing_agent();

    // When confirming.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then the cursor is on session 1's card.
    assert_eq!(
        state.sessions.cursor,
        Some(SidebarItem::Session(SessionId(1))),
        "the cursor should leave the closed agent's row for its card"
    );
}

#[rstest::rstest]
fn close_pane_confirm_no_keeps_the_pane() {
    // Given pane 1's close confirm, with No selected.
    let mut state = on_agent_in_another_tab();
    IntentHandler::handle(&Intent::Delete, &mut state);

    // When confirming.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then pane 1 is still in session 1's layout.
    assert_eq!(
        state.layouts.owner_of(PaneId(1)),
        Some(SessionId(1)),
        "No should keep the agent's pane"
    );
}

#[rstest::rstest]
fn search_confirm_on_a_folded_cards_agent_row_unfolds_it() {
    // Given "logout" matching thread 3, its card folded and the cursor on
    // its agent row.
    let mut state = searching("logout", Some(3));
    state.sessions.folded.insert(SessionId(3));
    state.sessions.cursor = Some(SidebarItem::Agent {
        session: SessionId(3),
        pane: PaneId(3),
    });

    // When ending the search with `⏎`.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then thread 3's card is open.
    assert!(
        !state.sessions.folded.contains(&SessionId(3)),
        "a cursor left on a hidden row unfolds its card"
    );
}

#[rstest::rstest]
fn deleting_the_active_session_leaves_none_active() {
    // Given active session 1 hovered, its delete confirm on Yes.
    let mut state = left_pane_showing_1(&[1], 1);
    answer_yes(&Intent::Delete, &mut state);

    // When confirming.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then nothing is active.
    assert_eq!(
        state.sessions.active(),
        None,
        "a deleted session should stop being active"
    );
}

#[rstest::rstest]
fn deleting_the_active_session_shows_the_dashboard() {
    // Given active session 1 hovered, its delete confirm on Yes.
    let mut state = left_pane_showing_1(&[1], 1);
    answer_yes(&Intent::Delete, &mut state);

    // When confirming.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then no layout is shown, so the right side draws the dashboard.
    assert!(
        state.shown_layout().is_none(),
        "deleting the active session should leave the dashboard on the right"
    );
}

#[rstest::rstest]
fn settling_the_active_session_leaves_none_active() {
    // Given active session 1 hovered, its settle confirm on Yes.
    let mut state = left_pane_showing_1(&[1], 1);
    answer_yes(&Intent::ToggleSettle, &mut state);

    // When confirming.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then nothing is active.
    assert_eq!(
        state.sessions.active(),
        None,
        "a settled session should stop being active"
    );
}

#[rstest::rstest]
fn picking_an_agent_makes_its_session_active() {
    // Given the session picker on thread 1, opened on thread 2.
    let mut state = picking_thread_1();

    // When confirming.
    IntentHandler::handle(&Intent::PickerConfirm, &mut state);

    // Then session 1 is active.
    assert_eq!(
        state.sessions.active(),
        Some(SessionId(1)),
        "picking an agent should make its session active"
    );
}
