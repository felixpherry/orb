//! The [`IntentHandler`]: the single decision point for all user input.

use crate::feat::pane::validator::validate_attach;
use crate::feat::preview::validator::{validate_toggle_fold, validate_yank};
use crate::feat::sessions::state::{AttachTarget, SidebarItem};
use crate::feat::sessions::validator::{
    validate_close_shelf, validate_delete, validate_new_session, validate_open_shelf,
    validate_toggle_pin, validate_toggle_settle,
};
use crate::{AppState, Command, Focus, Intent};

/// Applies each [`Intent`] to [`AppState`] in one match block.
pub struct IntentHandler;

impl IntentHandler {
    /// Apply `intent` to `state` and return the commands that must follow.
    /// An intent that fails validation changes nothing.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per intent keeps every input decision in one match"
    )]
    pub fn handle(intent: &Intent, state: &mut AppState) -> Vec<Command> {
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
            Intent::FocusPreview => {
                state.focus = Focus::Preview;
                vec![]
            }
            Intent::FocusSidebar => {
                state.focus = Focus::Sidebar;
                vec![]
            }
            Intent::Attach if state.sessions.cursor == Some(SidebarItem::SettledShelf) => {
                if state.sessions.shelf_open {
                    state.sessions.close_shelf();
                } else {
                    state.sessions.open_shelf();
                }
                vec![]
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
            Intent::NewSession => match validate_new_session(state) {
                Ok(()) => {
                    state.sessions.starting = true;
                    vec![Command::CreateSession]
                }
                Err(_) => vec![],
            },
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
            Intent::DeleteThread => match (validate_delete(state), state.sessions.selected_id()) {
                (Ok(()), Some(id)) => {
                    state.sessions.cursor = state.sessions.row_neighbour(id);
                    with_visit(state, vec![Command::Delete(id), Command::ShowPreview])
                }
                _ => vec![],
            },
        }
    }
}

/// `commands`, then a visit to the thread under the cursor, if any.
fn with_visit(state: &AppState, mut commands: Vec<Command>) -> Vec<Command> {
    commands.extend(state.sessions.selected_id().map(Command::Visit));
    commands
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use crate::feat::preview::block::{Block, BlockId, BlockKind, ToolCall, ToolStatus};
    use crate::feat::preview::state::{Preview, PreviewLayout};
    use crate::feat::sessions::state::{
        AttachTarget, Project, ProjectId, Sessions, SidebarItem, Thread, ThreadId, ThreadStatus,
    };
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
                    threads,
                }],
                cursor: Some(cursor),
                ..Sessions::default()
            },
            ..AppState::default()
        }
    }

    /// One project holding `threads`, with thread `selected` selected.
    fn state_with(threads: Vec<Thread>, selected: i64) -> AppState {
        state_at(threads, SidebarItem::Thread(ThreadId(selected)))
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
    fn new_session_sets_starting() {
        // Given no create in flight.
        let mut state = AppState::default();

        // When handling NewSession.
        IntentHandler::handle(&Intent::NewSession, &mut state);

        // Then a create is in flight.
        assert!(state.sessions.starting, "NewSession should set starting");
    }

    #[rstest::rstest]
    fn new_session_returns_create_session() {
        // Given no create in flight.
        let mut state = AppState::default();

        // When handling NewSession.
        let commands = IntentHandler::handle(&Intent::NewSession, &mut state);

        // Then the sessions actor is asked to create one.
        assert_eq!(
            commands,
            vec![Command::CreateSession],
            "NewSession should return CreateSession"
        );
    }

    #[rstest::rstest]
    fn new_session_while_starting_returns_no_commands() {
        // Given a create already in flight.
        let mut state = AppState {
            sessions: Sessions {
                starting: true,
                ..Sessions::default()
            },
            ..AppState::default()
        };

        // When handling NewSession again.
        let commands = IntentHandler::handle(&Intent::NewSession, &mut state);

        // Then no second create is requested.
        assert!(
            commands.is_empty(),
            "NewSession while starting should return no commands"
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
}
