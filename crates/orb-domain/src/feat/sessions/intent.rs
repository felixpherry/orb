//! What the session keys do: making, folding, pinning, settling and deleting
//! sessions, and what the confirms and name boxes for them ask for.

use crate::command::{SessionsCommand, Workspace};
use crate::feat::git::intent::workspace_items;
use crate::feat::picker::intent::open_picker;
use crate::feat::picker::state::{PickTarget, PickerState};
use crate::feat::sessions::state::{
    FolderKind, ProjectKind, SessionKind, SidebarItem, folder_slug,
};
use crate::feat::sessions::validator::{
    FoldTarget, NewFolderError, SETTLE_IN_PROGRESS, ToggleSettleError, validate_delete,
    validate_fold, validate_new_folder, validate_new_incognito, validate_new_session,
    validate_toggle_pin, validate_toggle_settle, validate_unfold,
};
use crate::feat::sidebar::state::{Rename, RenameTarget};
use crate::ids::{ProjectId, SessionId};
use crate::{AppState, Command, Focus, TextInput};

/// Asks for a new Incognito session in its checkout, clearing a filter to
/// another project.
pub(crate) fn new_incognito(state: &mut AppState) -> Vec<Command> {
    match (
        validate_new_incognito(state),
        validate_new_session(state),
        state
            .sessions
            .own_project(ProjectKind::Incognito)
            .map(|project| project.id),
    ) {
        (Ok(()), Ok(()), Some(project)) => {
            let mut commands = unfilter(state, project);
            commands.extend(new_session(state, project, Workspace::Checkout, None));
            commands
        }
        _ => vec![],
    }
}

/// Opens the Settled shelf or the card under the cursor.
pub(crate) fn unfold(state: &mut AppState) -> Vec<Command> {
    match validate_unfold(state) {
        Ok(FoldTarget::Shelf) => {
            state.sessions.open_shelf();
            vec![]
        }
        Ok(FoldTarget::Card(id)) => {
            state.sessions.unfold(id);
            vec![]
        }
        Err(_) => vec![],
    }
}

/// Closes the Settled shelf or the card under the cursor.
pub(crate) fn fold(state: &mut AppState) -> Vec<Command> {
    match validate_fold(state) {
        Ok(FoldTarget::Shelf) => {
            state.sessions.close_shelf();
            vec![]
        }
        Ok(FoldTarget::Card(id)) => {
            state.sessions.fold(id);
            vec![]
        }
        Err(_) => vec![],
    }
}

/// Pins the selected session to the top, or unpins it.
pub(crate) fn toggle_pin(state: &mut AppState) -> Vec<Command> {
    match (
        validate_toggle_pin(state),
        state.sessions.selected_session(),
    ) {
        (Ok(()), Some(session)) => match session.pinned_at {
            Some(_) => {
                vec![Command::Sessions(SessionsCommand::UnpinSession(session.id))]
            }
            None => vec![Command::Sessions(SessionsCommand::PinSession(session.id))],
        },
        _ => vec![],
    }
}

/// `s`: un-settles a settled session at once, or opens the settle confirm
/// for an unsettled one.
pub(crate) fn toggle_settle(state: &mut AppState) -> Vec<Command> {
    match (
        validate_toggle_settle(state),
        state.sessions.selected_session(),
    ) {
        (Ok(()), Some(session)) if session.settled_at.is_some() => {
            vec![Command::Sessions(SessionsCommand::UnsettleSession(
                session.id,
            ))]
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

/// `d`: opens the close confirm for an agent row, or the delete confirm for
/// a session row.
pub(crate) fn delete(state: &mut AppState) -> Vec<Command> {
    match (
        validate_delete(state),
        state.sessions.cursor_row(&state.layouts),
    ) {
        (Ok(()), Some(SidebarItem::Agent { session, pane })) => {
            open_picker(state, PickerState::close_pane(pane, session, state.focus));
            vec![]
        }
        (Ok(()), Some(SidebarItem::Session(id))) => {
            let folder = deletes_folder(state, id);
            open_picker(state, PickerState::delete_session(id, folder, state.focus));
            vec![]
        }
        _ => vec![],
    }
}

/// Filters the sidebar to `filter`'s project, or to all projects, and asks
/// for the filter to be saved.
pub(crate) fn filter_to(state: &mut AppState, filter: Option<ProjectId>) -> Vec<Command> {
    state.sessions.filter_to(filter, &state.layouts);
    vec![Command::Sessions(SessionsCommand::SaveUi)]
}

/// Asks for `project` to be removed. A filter to it goes back to all
/// projects.
pub(crate) fn remove_project(state: &mut AppState, project: ProjectId) -> Vec<Command> {
    if state.sessions.filter == Some(project) {
        state.sessions.filter = None;
    }
    vec![
        Command::Sessions(SessionsCommand::RemoveProject(project)),
        Command::Sessions(SessionsCommand::SaveUi),
    ]
}

/// Settles `session`, answered `Yes` in its confirm, if the cursor is still
/// on it, it is still unsettled and no agent pane has a turn underway: it
/// leaves `attached` and the cursor moves to the neighbouring card. A turn
/// that started meanwhile shows the refusal on the mode line.
pub(crate) fn settle_session(state: &mut AppState, session: SessionId) -> Vec<Command> {
    match (
        validate_toggle_settle(state),
        state.sessions.selected_session(),
    ) {
        (Ok(()), Some(selected)) if selected.id == session && selected.settled_at.is_none() => {
            state.attached.remove(&session);
            state.sessions.leave(session);
            state.sessions.cursor = state
                .sessions
                .card_neighbour(SidebarItem::Session(session), &state.layouts);
            vec![Command::Sessions(SessionsCommand::SettleSession(session))]
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
pub(crate) fn still_deletable(state: &AppState, item: SidebarItem) -> bool {
    state.sessions.cursor_row(&state.layouts) == Some(item) && validate_delete(state).is_ok()
}

/// Hides `session` at once, detaches it, drops it from the jump list, moves
/// the sidebar cursor to the neighbouring row when it was on `session`, and
/// asks for the delete.
pub(crate) fn delete_session(state: &mut AppState, session: SessionId) -> Vec<Command> {
    let on_it = state
        .sessions
        .selected_session()
        .map(|selected| selected.id)
        == Some(session);
    let neighbour = state
        .sessions
        .row_neighbour(SidebarItem::Session(session), &state.layouts);
    state.sessions.deleting.insert(session);
    state.attached.remove(&session);
    state.sessions.leave(session);
    state.jumps.remove(SidebarItem::Session(session));
    if on_it {
        state.sessions.cursor = neighbour;
    }
    vec![
        Command::Sessions(SessionsCommand::DeleteSession(session)),
        Command::Sessions(SessionsCommand::SaveJumps),
    ]
}

/// Picks `project` in the new-session picker: a filter to another project
/// goes back to all projects, and the empty `New session` name box opens,
/// giving the keys back to where the picker was opened from when it closes.
pub(crate) fn pick_project(state: &mut AppState, project: ProjectId) -> Vec<Command> {
    if state.sessions.project(project).is_none() {
        return vec![];
    }
    let return_to = state.focus;
    let commands = unfilter(state, project);
    state.rename = Some(Rename {
        target: RenameTarget::NewSession { project, return_to },
        input: TextInput::default(),
        creating: false,
    });
    state.focus = Focus::Rename;
    commands
}

/// `⏎` in the `New session` box: the keys go back to `return_to`; a git
/// project goes on to the workspace picker, any other gets its session in
/// its checkout at once. A blank name leaves the session unnamed.
pub(crate) fn confirm_session_name(
    state: &mut AppState,
    project: ProjectId,
    return_to: Focus,
) -> Vec<Command> {
    let name = state.rename.take().and_then(|rename| {
        let name = rename.input.text().trim();
        (!name.is_empty()).then(|| name.to_owned())
    });
    state.focus = return_to;
    let Some(found) = state.sessions.project(project) else {
        return vec![];
    };
    if !found.repo {
        return new_session(state, project, Workspace::Checkout, name);
    }
    let items = workspace_items(&state.sessions, found, &found.root, false);
    let picker = PickerState::workspace(PickTarget::New { project, name }, items, return_to);
    open_picker(state, picker);
    vec![]
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
    outside
        .then_some(Command::Sessions(SessionsCommand::SaveUi))
        .into_iter()
        .collect()
}

/// Opens the empty name box for a new session in orb's own `kind` folder.
pub(crate) fn open_folder_name(state: &mut AppState, kind: FolderKind) -> Vec<Command> {
    state.rename = Some(Rename {
        target: RenameTarget::NewFolder(kind),
        input: TextInput::default(),
        creating: false,
    });
    state.focus = Focus::Rename;
    vec![]
}

/// Whether deleting `session` also deletes its folder: a colony in Research or Learn.
fn deletes_folder(state: &AppState, session: SessionId) -> bool {
    state.sessions.session(session).is_some_and(|session| {
        session.kind == SessionKind::Colony
            && state
                .sessions
                .project(session.project)
                .is_some_and(|project| {
                    matches!(project.kind, ProjectKind::Research | ProjectKind::Learn)
                })
    })
}

/// `⏎` in the name box for a new Research or Learn session: an empty name,
/// one already asked for, or one while another session is being made does
/// nothing; an invalid one shows why on the mode line, keeping the box open;
/// a valid one asks for the session, and the sessions actor closes the box
/// once it's made.
pub(crate) fn confirm_folder_name(state: &mut AppState) -> Vec<Command> {
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
    let command = Command::Sessions(SessionsCommand::NewFolderSession {
        kind: *kind,
        name: folder_slug(input.text()),
        if_active: state.sessions.active(),
    });
    state.sessions.starting = true;
    state.jumps.jump(state.sessions.cursor, None);
    vec![command, Command::Sessions(SessionsCommand::SaveJumps)]
}

/// Asks for a new session of `project` in `workspace`, named `name`, one at
/// a time: marks the start and records the row it leaves as a jump
/// (`attach_session` records the new session's row when the frontend
/// attaches it).
pub(crate) fn new_session(
    state: &mut AppState,
    project: ProjectId,
    workspace: Workspace,
    name: Option<String>,
) -> Vec<Command> {
    if validate_new_session(state).is_err() {
        return vec![];
    }
    state.sessions.starting = true;
    state.jumps.jump(state.sessions.cursor, None);
    let if_active = state.sessions.active();
    vec![
        Command::Sessions(SessionsCommand::NewSession {
            project,
            workspace,
            name,
            if_active,
        }),
        Command::Sessions(SessionsCommand::SaveJumps),
    ]
}

#[cfg(test)]
mod tests {
    use crate::command::{SessionsCommand, Workspace};
    use crate::feat::picker::list::PickerItem;
    use crate::feat::picker::state::PickerKind;
    use crate::feat::sessions::state::{
        AgentStatus, FolderKind, ProjectKind, SessionKind, SidebarItem,
    };
    use crate::feat::sessions::validator::SETTLE_IN_PROGRESS;
    use crate::feat::sidebar::state::RenameTarget;
    use crate::ids::{PaneId, ProjectId, SessionId};
    use crate::intent_handler::test_support::{
        agent, at, incognito, on_agent_in_another_tab, rename_text, settle, state_at, state_with,
        stopped,
    };
    use crate::{AppState, Command, Focus, Intent, IntentHandler};

    #[rstest::rstest]
    fn l_on_the_shelf_opens_it() {
        // Given the cursor on the collapsed Settled header.
        let mut state = settle(state_at(vec![stopped(1)], SidebarItem::SettledShelf), &[1]);

        // When handling Unfold on the header.
        IntentHandler::handle(&Intent::Unfold, &mut state);

        // Then the shelf is open.
        assert!(
            state.sessions.shelf_open,
            "l on the header should open the shelf"
        );
    }

    #[rstest::rstest]
    fn h_on_a_settled_agent_closes_the_shelf_and_selects_it() {
        // Given the shelf open and the cursor on settled agent 1.
        let mut state = settle(state_with(vec![stopped(1)], 1), &[1]);
        state.sessions.shelf_open = true;

        // When handling Fold.
        IntentHandler::handle(&Intent::Fold, &mut state);

        // Then the shelf is closed with its header selected.
        assert_eq!(
            (state.sessions.shelf_open, state.sessions.cursor),
            (false, Some(SidebarItem::SettledShelf)),
            "h in the shelf should close it onto its header"
        );
    }

    #[rstest::rstest]
    fn h_on_an_active_card_keeps_the_shelf_open() {
        // Given the shelf open and the cursor on active agent 1.
        let mut state = settle(
            state_with(vec![agent(1, AgentStatus::Idle), stopped(2)], 1),
            &[2],
        );
        state.sessions.shelf_open = true;

        // When handling Fold.
        IntentHandler::handle(&Intent::Fold, &mut state);

        // Then the shelf and cursor are unchanged.
        assert_eq!(
            (state.sessions.shelf_open, state.sessions.cursor),
            (true, Some(SidebarItem::Session(SessionId(1)))),
            "h on a card leaves the shelf alone"
        );
    }

    #[rstest::rstest]
    fn settle_on_a_settled_agent_returns_unsettle() {
        // Given a selected settled agent.
        let mut state = settle(state_with(vec![stopped(1)], 1), &[1]);

        // When handling ToggleSettle.
        let commands = IntentHandler::handle(&Intent::ToggleSettle, &mut state);

        // Then the sessions actor is asked to un-settle it.
        assert_eq!(
            commands,
            vec![Command::Sessions(SessionsCommand::UnsettleSession(
                SessionId(1)
            ))],
            "ToggleSettle on a settled agent should return Unsettle"
        );
    }

    #[rstest::rstest]
    fn settle_on_a_settled_agent_keeps_the_cursor() {
        // Given a selected settled agent.
        let mut state = settle(state_with(vec![stopped(1)], 1), &[1]);

        // When handling ToggleSettle.
        IntentHandler::handle(&Intent::ToggleSettle, &mut state);

        // Then the cursor stays on it.
        assert_eq!(
            state.sessions.cursor,
            Some(SidebarItem::Session(SessionId(1))),
            "un-settle should keep the cursor on the agent"
        );
    }

    #[rstest::rstest]
    fn toggle_pin_on_a_session_returns_pin_session() {
        // Given a selected unpinned session.
        let mut state = state_with(vec![agent(1, AgentStatus::Idle)], 1);

        // When handling TogglePin.
        let commands = IntentHandler::handle(&Intent::TogglePin, &mut state);

        // Then the sessions actor is asked to pin it.
        assert_eq!(
            commands,
            vec![Command::Sessions(SessionsCommand::PinSession(SessionId(1)))],
            "p on an unpinned session pins it"
        );
    }

    #[rstest::rstest]
    fn toggle_pin_on_a_pinned_agent_returns_unpin() {
        // Given a selected pinned agent.
        let mut state = state_with(vec![agent(1, AgentStatus::Idle)], 1);
        if let Some(session) = state.sessions.sessions.get_mut(0) {
            session.pinned_at = Some(at(1));
        }

        // When handling TogglePin.
        let commands = IntentHandler::handle(&Intent::TogglePin, &mut state);

        // Then the sessions actor is asked to unpin it.
        assert_eq!(
            commands,
            vec![Command::Sessions(SessionsCommand::UnpinSession(SessionId(
                1
            )))],
            "TogglePin on a pinned agent should return Unpin"
        );
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
        // Given a selected idle agent.
        let mut state = state_with(vec![agent(1, AgentStatus::Idle)], 1);

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
            "s should ask to settle the agent"
        );
    }

    #[rstest::rstest]
    fn settle_on_a_settled_agent_opens_no_confirm() {
        // Given a selected settled agent.
        let mut state = settle(state_with(vec![stopped(1)], 1), &[1]);

        // When handling ToggleSettle.
        IntentHandler::handle(&Intent::ToggleSettle, &mut state);

        // Then no confirm opens.
        assert!(state.picker.is_none(), "un-settling needs no confirm");
    }

    #[rstest::rstest]
    fn settle_is_refused_while_an_agent_works() {
        // Given a selected agent Claude is working in.
        let mut state = state_with(vec![agent(1, AgentStatus::Working)], 1);

        // When handling ToggleSettle.
        IntentHandler::handle(&Intent::ToggleSettle, &mut state);

        // Then the mode line says it can't settle.
        assert_eq!(
            state.sessions.error.as_deref(),
            Some(SETTLE_IN_PROGRESS),
            "s on a working agent should say why it can't settle"
        );
    }

    #[rstest::rstest]
    fn settle_on_a_working_agent_opens_no_confirm() {
        // Given a selected agent Claude is working in.
        let mut state = state_with(vec![agent(1, AgentStatus::Working)], 1);

        // When handling ToggleSettle.
        IntentHandler::handle(&Intent::ToggleSettle, &mut state);

        // Then no confirm opens.
        assert!(
            state.picker.is_none(),
            "an agent that can't be settled should not ask to settle"
        );
    }

    #[rstest::rstest]
    fn delete_opens_the_delete_confirm_with_no_selected() {
        // Given a selected agent.
        let mut state = state_with(vec![agent(1, AgentStatus::Idle)], 1);

        // When handling DeleteAgent.
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
            "d should ask to delete the agent"
        );
    }

    #[rstest::rstest]
    fn delete_on_a_research_colony_asks_to_delete_its_folder() {
        // Given a selected colony in the Research project.
        let mut state = state_with(vec![agent(1, AgentStatus::Idle)], 1);
        if let Some(project) = state.sessions.projects.first_mut() {
            project.kind = ProjectKind::Research;
        }
        if let Some(session) = state.sessions.sessions.first_mut() {
            session.kind = SessionKind::Colony;
        }

        // When handling Delete.
        IntentHandler::handle(&Intent::Delete, &mut state);

        // Then its delete confirm says it takes the folder.
        assert_eq!(
            open_confirm(&state).map(|(kind, _)| kind),
            Some(&PickerKind::DeleteSession {
                session: SessionId(1),
                folder: true
            }),
            "d on a Research colony should ask to delete its folder too"
        );
    }

    #[rstest::rstest]
    fn new_incognito_opens_no_name_box() {
        // Given the Incognito project.
        let mut state = incognito();

        // When handling NewIncognito.
        IntentHandler::handle(&Intent::NewIncognito, &mut state);

        // Then no name box is open.
        assert!(state.rename.is_none(), "␣i asks no name");
    }

    #[rstest::rstest]
    fn new_incognito_returns_new_session_in_the_incognito_checkout() {
        // Given the Incognito project.
        let mut state = incognito();

        // When handling NewIncognito.
        let commands = IntentHandler::handle(&Intent::NewIncognito, &mut state);

        // Then a session in the Incognito folder is asked for.
        assert!(
            commands.contains(&Command::Sessions(SessionsCommand::NewSession {
                project: ProjectId(2),
                workspace: Workspace::Checkout,
                name: None,
                if_active: None,
            })),
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
            "<C-g> gr should open the name box"
        );
    }

    #[rstest::rstest]
    fn delete_on_an_agent_row_opens_the_close_pane_confirm() {
        // Given the cursor on the agent row of pane 1.
        let mut state = on_agent_in_another_tab();

        // When handling Delete.
        IntentHandler::handle(&Intent::Delete, &mut state);

        // Then the confirm for closing pane 1 is open with No selected.
        assert_eq!(
            open_confirm(&state),
            Some((
                &PickerKind::ClosePane {
                    pane: PaneId(1),
                    session: SessionId(1)
                },
                Some(&PickerItem::Confirm(false))
            )),
            "d on an agent row should ask to close its pane"
        );
    }

    /// Session 1's card with its agent row, the cursor on `cursor`, folded
    /// or not.
    fn card_at(cursor: SidebarItem, folded: bool) -> AppState {
        let mut state = state_at(vec![agent(1, AgentStatus::Idle)], cursor);
        if folded {
            state.sessions.folded.insert(SessionId(1));
        }
        state
    }

    const AGENT_1: SidebarItem = SidebarItem::Agent {
        session: SessionId(1),
        pane: PaneId(1),
    };

    #[rstest::rstest]
    fn unfold_on_a_folded_card_unfolds_it() {
        // Given session 1's card folded, with the cursor on it.
        let mut state = card_at(SidebarItem::Session(SessionId(1)), true);

        // When handling Unfold (`l`).
        IntentHandler::handle(&Intent::Unfold, &mut state);

        // Then the card is open.
        assert!(
            !state.sessions.folded.contains(&SessionId(1)),
            "l should open the card"
        );
    }

    #[rstest::rstest]
    fn fold_on_a_card_folds_it() {
        // Given session 1's open card, with the cursor on it.
        let mut state = card_at(SidebarItem::Session(SessionId(1)), false);

        // When handling Fold (`h`).
        IntentHandler::handle(&Intent::Fold, &mut state);

        // Then the card is folded.
        assert!(
            state.sessions.folded.contains(&SessionId(1)),
            "h should fold the card"
        );
    }

    #[rstest::rstest]
    fn fold_on_an_agent_row_folds_its_card() {
        // Given the cursor on session 1's agent row.
        let mut state = card_at(AGENT_1, false);

        // When handling Fold (`h`).
        IntentHandler::handle(&Intent::Fold, &mut state);

        // Then session 1's card is folded.
        assert!(
            state.sessions.folded.contains(&SessionId(1)),
            "h on an agent row should fold its card"
        );
    }

    #[rstest::rstest]
    fn fold_on_an_agent_row_moves_the_cursor_to_its_card() {
        // Given the cursor on session 1's agent row.
        let mut state = card_at(AGENT_1, false);

        // When handling Fold (`h`).
        IntentHandler::handle(&Intent::Fold, &mut state);

        // Then the cursor is on session 1's card.
        assert_eq!(
            state.sessions.cursor,
            Some(SidebarItem::Session(SessionId(1))),
            "the folded row's cursor moves to its card"
        );
    }

    #[rstest::rstest]
    fn unfold_on_an_agent_row_does_nothing() {
        // Given the cursor on session 1's agent row, the card open.
        let mut state = card_at(AGENT_1, false);

        // When handling Unfold (`l`).
        IntentHandler::handle(&Intent::Unfold, &mut state);

        // Then the cursor stays and nothing is folded.
        assert_eq!(
            (state.sessions.cursor, state.sessions.folded.is_empty()),
            (Some(AGENT_1), true),
            "l on an agent row should do nothing"
        );
    }

    #[rstest::rstest]
    fn fold_and_unfold_on_a_card_without_agents_do_nothing(
        #[values(Intent::Fold, Intent::Unfold)] intent: Intent,
    ) {
        // Given session 1's card whose agent pane has closed.
        let mut state = card_at(SidebarItem::Session(SessionId(1)), false);
        for project in &mut state.sessions.projects {
            for agent in &mut project.agents {
                agent.pane = None;
            }
        }

        // When handling `intent`.
        IntentHandler::handle(&intent, &mut state);

        // Then nothing is folded.
        assert!(
            state.sessions.folded.is_empty(),
            "{intent:?} has no agent rows to act on"
        );
    }

    #[rstest::rstest]
    fn h_on_the_open_shelf_header_closes_it() {
        // Given the cursor on the open Settled header.
        let mut state = settle(state_at(vec![stopped(1)], SidebarItem::SettledShelf), &[1]);
        state.sessions.shelf_open = true;

        // When handling Fold (`h`).
        IntentHandler::handle(&Intent::Fold, &mut state);

        // Then the shelf is closed.
        assert!(
            !state.sessions.shelf_open,
            "h on the header should close the shelf"
        );
    }
}
