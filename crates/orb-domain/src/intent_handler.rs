//! The [`IntentHandler`]: the single decision point for all user input.

use crate::feat::pane::validator::validate_attach;
use crate::feat::preview::validator::{validate_toggle_fold, validate_yank};
use crate::feat::sessions::state::AttachTarget;
use crate::feat::sessions::validator::validate_new_session;
use crate::{AppState, Command, Focus, Intent};

/// Applies each [`Intent`] to [`AppState`] in one match block.
pub struct IntentHandler;

impl IntentHandler {
    /// Apply `intent` to `state` and return the commands that must follow.
    /// An intent that fails validation changes nothing.
    pub fn handle(intent: &Intent, state: &mut AppState) -> Vec<Command> {
        match intent {
            Intent::Quit => {
                state.should_quit = true;
                vec![]
            }
            Intent::SelectNext => {
                state.sessions.select_next();
                vec![Command::ShowPreview]
            }
            Intent::SelectPrev => {
                state.sessions.select_prev();
                vec![Command::ShowPreview]
            }
            Intent::FocusPreview => {
                state.focus = Focus::Preview;
                vec![]
            }
            Intent::FocusSidebar => {
                state.focus = Focus::Sidebar;
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
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::feat::preview::block::{Block, BlockId, BlockKind, ToolCall, ToolStatus};
    use crate::feat::preview::state::{Preview, PreviewLayout};
    use crate::feat::sessions::state::{
        AttachTarget, Project, ProjectId, Sessions, Thread, ThreadId, ThreadStatus,
    };
    use crate::{AppState, Command, Focus, Intent, IntentHandler};

    fn thread(id: i64, status: ThreadStatus) -> Thread {
        Thread {
            id: ThreadId(id),
            title: None,
            cwd: format!("/work/{id}").into(),
            status,
            turn_started_at: None,
            attach_argv: vec!["claude".into(), "attach".into(), format!("t{id}").into()],
        }
    }

    /// One project holding `threads`, with thread `selected` selected.
    fn state_with(threads: Vec<Thread>, selected: i64) -> AppState {
        AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "work".into(),
                    root: "/work".into(),
                    threads,
                }],
                selected: Some(ThreadId(selected)),
                ..Sessions::default()
            },
            ..AppState::default()
        }
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
        // Given threads 1 and 2 with the last one selected.
        let mut state = state_with(
            vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
            2,
        );

        // When handling SelectNext.
        IntentHandler::handle(&Intent::SelectNext, &mut state);

        // Then the selection stays on the last thread.
        assert_eq!(
            state.sessions.selected,
            Some(ThreadId(2)),
            "SelectNext should not wrap past the last thread"
        );
    }

    #[rstest::rstest]
    fn select_prev_on_first_thread_keeps_selection() {
        // Given threads 1 and 2 with the first one selected.
        let mut state = state_with(
            vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)],
            1,
        );

        // When handling SelectPrev.
        IntentHandler::handle(&Intent::SelectPrev, &mut state);

        // Then the selection stays on the first thread.
        assert_eq!(
            state.sessions.selected,
            Some(ThreadId(1)),
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
        assert_eq!(
            commands,
            vec![Command::ShowPreview],
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
}
