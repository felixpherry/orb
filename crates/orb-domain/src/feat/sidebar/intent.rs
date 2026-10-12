//! What the sidebar keys do: moving the cursor, showing or hiding the
//! sidebar, and the rename box and search that take the picker's keys.

use crate::command::SessionsCommand;
use crate::feat::sessions::intent::{confirm_folder_name, confirm_session_name};
use crate::feat::sessions::state::{Search, SidebarItem};
use crate::feat::sidebar::state::{Rename, RenameTarget};
use crate::feat::sidebar::validator::validate_rename;
use crate::intent_handler::{focus_right, show_pane, with_visit};
use crate::{AppState, Command, Focus, Intent, TextInput};

/// Moves the cursor to the next row.
pub(crate) fn select_next(state: &mut AppState) -> Vec<Command> {
    state.sessions.select_next(&state.layouts);
    with_visit(state, vec![])
}

/// Moves the cursor to the previous row.
pub(crate) fn select_prev(state: &mut AppState) -> Vec<Command> {
    state.sessions.select_prev(&state.layouts);
    with_visit(state, vec![])
}

/// `gg`: moves the cursor to the first row, recording the jump.
pub(crate) fn select_first(state: &mut AppState) -> Vec<Command> {
    let from = state.sessions.cursor;
    state.sessions.select_first(&state.layouts);
    let commands = record_jump(state, from);
    with_visit(state, commands)
}

/// `G`: moves the cursor to the last row, recording the jump.
pub(crate) fn select_last(state: &mut AppState) -> Vec<Command> {
    let from = state.sessions.cursor;
    state.sessions.select_last(&state.layouts);
    let commands = record_jump(state, from);
    with_visit(state, commands)
}

/// Moves the cursor half a page down.
pub(crate) fn half_page_down(state: &mut AppState) -> Vec<Command> {
    state
        .sessions
        .half_page_down(&state.sidebar.layout, &state.layouts);
    with_visit(state, vec![])
}

/// Moves the cursor half a page up.
pub(crate) fn half_page_up(state: &mut AppState) -> Vec<Command> {
    state
        .sessions
        .half_page_up(&state.sidebar.layout, &state.layouts);
    with_visit(state, vec![])
}

/// A click on row `item`: moves the cursor there; during a search it also
/// acts as the search's `⏎`.
pub(crate) fn select_row(state: &mut AppState, item: SidebarItem) -> Vec<Command> {
    if state.focus == Focus::Search {
        state.sessions.select_row(item, &state.layouts);
        return search_key(&Intent::PickerConfirm, state);
    }
    state.sessions.select_row(item, &state.layouts);
    with_visit(state, vec![])
}

/// Shows or hides the sidebar.
pub(crate) fn toggle_sidebar(state: &mut AppState) -> Vec<Command> {
    match (state.sidebar.hidden, state.focus) {
        // `<C-g> e` in a pane: the keys stay in the pane.
        (hidden, Focus::Pane) => {
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
    }
}

/// `r`: opens the rename box on the selected agent or session, holding its
/// name.
pub(crate) fn rename(state: &mut AppState) -> Vec<Command> {
    match (
        validate_rename(state),
        state.sessions.selected_agent(&state.layouts),
        state.sessions.selected_session(),
    ) {
        (Ok(()), Some((_, pane)), _) => {
            let name = state
                .layouts
                .entry(pane)
                .and_then(|entry| entry.name.clone())
                .or_else(|| state.sessions.agent_in(pane)?.title.clone())
                .unwrap_or_default();
            state.rename = Some(Rename {
                target: RenameTarget::Agent(pane),
                input: TextInput::new(&name),
                creating: false,
            });
            state.focus = Focus::Rename;
            vec![]
        }
        (Ok(()), None, Some(session)) => {
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
    }
}

/// `/`: starts a sidebar search, remembering where the cursor was.
pub(crate) fn search(state: &mut AppState) -> Vec<Command> {
    state.sessions.search = Some(Search {
        input: TextInput::default(),
        return_to: state.sessions.cursor,
    });
    state.focus = Focus::Search;
    vec![]
}

/// What a picker key does in the rename box: edit the name, save it (a blank
/// name goes back to the harness's title) or cancel, both giving the sidebar back
/// the keys. The picker's other keys do nothing.
pub(crate) fn rename_key(intent: &Intent, state: &mut AppState) -> Vec<Command> {
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
        (
            Intent::PickerConfirm,
            Some(Rename {
                target: RenameTarget::NewSession { project, return_to },
                ..
            }),
        ) => {
            let (project, return_to) = (*project, *return_to);
            return confirm_session_name(state, project, return_to);
        }
        (
            Intent::PickerCancel,
            Some(Rename {
                target: RenameTarget::NewSession { return_to, .. },
                ..
            }),
        ) => {
            state.focus = *return_to;
            state.rename = None;
        }
        (Intent::PickerConfirm, _) => {
            state.focus = Focus::Sidebar;
            let Some(rename) = state.rename.take() else {
                return vec![];
            };
            let name = rename.input.text().trim();
            let name = (!name.is_empty()).then(|| name.to_owned());
            return match rename.target {
                RenameTarget::Session(session) => {
                    vec![Command::Sessions(SessionsCommand::RenameSession {
                        session,
                        name,
                    })]
                }
                RenameTarget::NewFolder(_) | RenameTarget::NewSession { .. } => vec![],
                // The keys go back to the panes.
                RenameTarget::Tab { owner, tab } => {
                    state.layouts.rename_tab(owner, tab, name);
                    let mut commands = show_pane(state);
                    commands.push(Command::Sessions(SessionsCommand::SaveLayout(owner)));
                    commands
                }
                RenameTarget::Pane(pane) => {
                    state.layouts.rename_pane(pane, name);
                    let mut commands = show_pane(state);
                    commands.extend(
                        state
                            .layouts
                            .owner_of(pane)
                            .map(SessionsCommand::SaveLayout)
                            .map(Command::Sessions),
                    );
                    commands
                }
                // The keys stay in the sidebar.
                RenameTarget::Agent(pane) => {
                    state.layouts.rename_pane(pane, name);
                    state
                        .layouts
                        .owner_of(pane)
                        .map(SessionsCommand::SaveLayout)
                        .map(Command::Sessions)
                        .into_iter()
                        .collect()
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

/// What a picker key does in the sidebar search: edit the text, putting the
/// cursor on the first match; move between matches; or end the search,
/// giving the sidebar back the keys. `⏎` keeps the cursor on its match and enters its session (with
/// no match it acts as `Esc`); `Esc` puts it back where it was. The picker's
/// other keys do nothing.
pub(crate) fn search_key(intent: &Intent, state: &mut AppState) -> Vec<Command> {
    let sessions = &mut state.sessions;
    match (intent, &mut sessions.search) {
        (Intent::PickerInput(ch), Some(search)) => {
            search.input.insert(*ch);
            sessions.select_first_match(&state.layouts);
        }
        (Intent::PickerBackspace, Some(search)) => {
            search.input.backspace();
            sessions.select_first_match(&state.layouts);
        }
        (Intent::PickerDeleteWord, Some(search)) => {
            search.input.delete_word();
            sessions.select_first_match(&state.layouts);
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
        (Intent::PickerNext, _) => sessions.select_next_match(&state.layouts),
        (Intent::PickerPrev, _) => sessions.select_prev_match(&state.layouts),
        (Intent::PickerConfirm, _) if sessions.cursor.is_some() => {
            let from = sessions.search.take().and_then(|search| search.return_to);
            if let Some(id) = sessions.selected_session().map(|session| session.id) {
                sessions.enter(id);
            }
            state.focus = Focus::Sidebar;
            let commands = record_jump(state, from);
            return with_visit(state, commands);
        }
        (Intent::PickerConfirm | Intent::PickerCancel, _) => {
            sessions.cancel_search(&state.layouts);
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
    vec![Command::Sessions(SessionsCommand::SaveJumps)]
}

#[cfg(test)]
mod tests {
    use crate::command::SessionsCommand;
    use crate::feat::sessions::state::{SidebarItem, Thread, ThreadStatus};
    use crate::feat::sidebar::state::RenameTarget;
    use crate::ids::{PaneId, SessionId, ThreadId};
    use crate::intent_handler::test_support::{
        jumping, laid_out, left_pane, left_pane_showing_1, on_agent_in_another_tab, on_thread,
        rename_text, searching, state_with, thread, three_titles, titled,
    };
    use crate::{AppState, Command, Focus, Intent, IntentHandler};

    #[rstest::rstest]
    fn select_next_on_last_thread_wraps_to_the_first() {
        // Given threads 2 and 1 in sidebar order, with the last row (thread
        // 1's agent row) selected.
        let mut state = state_with(
            vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
            1,
        );
        state.sessions.cursor = Some(SidebarItem::Agent {
            session: SessionId(1),
            pane: PaneId(1),
        });

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
    #[case(Focus::Sidebar)]
    #[case(Focus::Pane)]
    fn toggle_sidebar_hides_a_shown_sidebar(#[case] focus: Focus) {
        // Given a shown sidebar.
        let mut state = laid_out(focus, 32, false);

        // When handling ToggleSidebar.
        IntentHandler::handle(&Intent::ToggleSidebar, &mut state);

        // Then the sidebar is hidden.
        assert!(state.sidebar.hidden, "<C-g> e should hide the sidebar");
    }

    #[rstest::rstest]
    fn hiding_the_sidebar_without_a_layout_keeps_the_keys() {
        // Given a shown sidebar, with the sidebar focused and no layout shown.
        let focus = Focus::Sidebar;
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
    #[case(Focus::Sidebar)]
    #[case(Focus::Pane)]
    fn toggle_sidebar_shows_a_hidden_sidebar(#[case] focus: Focus) {
        // Given a hidden sidebar.
        let mut state = laid_out(focus, 32, true);

        // When handling ToggleSidebar.
        IntentHandler::handle(&Intent::ToggleSidebar, &mut state);

        // Then the sidebar is shown.
        assert!(
            !state.sidebar.hidden,
            "<C-g> e should show the sidebar again"
        );
    }

    #[rstest::rstest]
    fn showing_the_sidebar_focuses_it() {
        // Given a hidden sidebar that had the keys.
        let mut state = laid_out(Focus::Sidebar, 32, true);

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
        let mut state = laid_out(Focus::Pane, 32, false);

        // When handling ToggleSidebar (`<C-g> e`).
        IntentHandler::handle(&Intent::ToggleSidebar, &mut state);

        // Then the pane keeps the keys.
        assert_eq!(
            state.focus,
            Focus::Pane,
            "<C-g> e should hide the sidebar and keep the pane focused"
        );
    }

    #[rstest::rstest]
    fn showing_the_sidebar_from_the_pane_keeps_the_pane_focused() {
        // Given a hidden sidebar, with the Claude pane focused.
        let mut state = laid_out(Focus::Pane, 32, true);

        // When handling ToggleSidebar (`<C-g> e`).
        IntentHandler::handle(&Intent::ToggleSidebar, &mut state);

        // Then the pane keeps the keys.
        assert_eq!(
            state.focus,
            Focus::Pane,
            "<C-g> e should show the sidebar and keep the pane focused"
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
    fn hiding_the_sidebar_with_the_selected_thread_attached_focuses_the_pane() {
        // Given thread 1 attached and selected in the sidebar.
        let mut state = left_pane(&[1], 1);

        // When handling ToggleSidebar.
        IntentHandler::handle(&Intent::ToggleSidebar, &mut state);

        // Then keys go to the session, which takes the full width.
        assert_eq!(
            state.focus,
            Focus::Pane,
            "hiding the sidebar should focus the selected thread's pane"
        );
    }

    #[rstest::rstest]
    #[case(Intent::SelectFirst, 3)]
    #[case(Intent::SelectLast, 1)]
    #[case(Intent::SelectHalfPageDown, 2)]
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
            commands.contains(&Command::Sessions(SessionsCommand::Visit(SessionId(
                expected
            )))),
            "{intent:?} should visit thread {expected}"
        );
    }

    #[rstest::rstest]
    fn select_next_returns_visit() {
        // Given threads 2 and 1 in sidebar order, with thread 2's agent row
        // selected.
        let mut state = state_with(
            vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
            2,
        );
        state.sessions.cursor = Some(SidebarItem::Agent {
            session: SessionId(2),
            pane: PaneId(2),
        });

        // When handling SelectNext.
        let commands = IntentHandler::handle(&Intent::SelectNext, &mut state);

        // Then the newly selected thread is visited.
        assert!(
            commands.contains(&Command::Sessions(SessionsCommand::Visit(SessionId(1)))),
            "SelectNext should visit the thread it lands on"
        );
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
            commands.contains(&Command::Sessions(SessionsCommand::SaveJumps)),
            "G should save the jump list"
        );
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

    /// The rename box target and text, if the box is open.
    fn rename_box(state: &AppState) -> Option<(RenameTarget, String)> {
        state
            .rename
            .as_ref()
            .map(|rename| (rename.target, rename.input.text().to_owned()))
    }

    #[rstest::rstest]
    fn rename_on_an_agent_row_opens_the_box_on_its_pane() {
        // Given the cursor on the agent row of pane 1, named "api".
        let mut state = on_agent_in_another_tab();
        state.layouts.rename_pane(PaneId(1), Some("api".into()));

        // When handling Rename.
        IntentHandler::handle(&Intent::Rename, &mut state);

        // Then the box names the agent's pane, holding its name.
        assert_eq!(
            rename_box(&state),
            Some((RenameTarget::Agent(PaneId(1)), "api".to_owned())),
            "r on an agent row should rename its pane"
        );
    }

    #[rstest::rstest]
    fn rename_on_an_unnamed_agent_row_starts_from_its_title() {
        // Given the cursor on the agent row of unnamed pane 1, whose thread is
        // titled "fix login".
        let mut state = state_with(
            vec![Thread {
                title: Some("fix login".into()),
                ..thread(1, ThreadStatus::Idle)
            }],
            1,
        );
        state.sessions.cursor = Some(SidebarItem::Agent {
            session: SessionId(1),
            pane: PaneId(1),
        });

        // When handling Rename.
        IntentHandler::handle(&Intent::Rename, &mut state);

        // Then the box holds the agent's title.
        assert_eq!(
            rename_box(&state),
            Some((RenameTarget::Agent(PaneId(1)), "fix login".to_owned())),
            "an unnamed agent's box should start from its title"
        );
    }

    #[rstest::rstest]
    fn moving_the_hover_to_another_attached_session_keeps_the_shown_session() {
        // Given sessions 1 and 2 attached, session 1 active and hovered.
        let mut state = left_pane_showing_1(&[1, 2], 1);

        // When moving the hover up onto session 2.
        IntentHandler::handle(&Intent::SelectPrev, &mut state);

        // Then the hover moved and session 1 is still shown.
        assert_eq!(
            (
                state.sessions.cursor == Some(on_thread(1)),
                state.shown_session()
            ),
            (false, Some(SessionId(1))),
            "moving the hover should not change what's on the right"
        );
    }

    #[rstest::rstest]
    fn select_row_keeps_the_active_session() {
        // Given session 1 active, hovered in the sidebar.
        let mut state = left_pane_showing_1(&[1, 2], 1);

        // When clicking thread 2's row.
        IntentHandler::handle(&Intent::SelectRow(on_thread(2)), &mut state);

        // Then session 1 is still active.
        assert_eq!(
            state.sessions.active(),
            Some(SessionId(1)),
            "a click on a row should only move the hover"
        );
    }
}
