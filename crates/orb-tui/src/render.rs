//! Draws a frame: the sidebar on the left, the attached session or the
//! dashboard on the right, the mode line at the bottom, and the which-key
//! popup on top while a key sequence is pending.
//! While the sidebar is hidden, the right side takes the full width.
//! An open picker is drawn over everything but the mode line, without the
//! popup, and so is the rename box while it has the keys.
//! The terminal's cursor is shown only where the keys are: on the sidebar's
//! selected row, at the text cursor of the picker, the rename box or the
//! sidebar search, on the dashboard's highlighted item, or in the attached
//! pane.
//! Whatever is left on the terminal's default background gets orb's navy, so
//! a transparent terminal doesn't show through.

use std::time::SystemTime;

use jiff::tz::TimeZone;
use orb_domain::feat::sidebar::state::{SidebarLayout, SidebarView};
use orb_domain::{AppState, Focus};
use orb_term::Pane;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Color;

use crate::dashboard;
use crate::keymap::Keys;
use crate::mode_line;
use crate::picker::{self, PickerScroll};
use crate::rename;
use crate::sidebar::{self, SidebarScroll};
use crate::which_key;

/// Behind everything that doesn't set its own background (tokyonight-moon's
/// `bg`).
const BACKGROUND: Color = Color::Rgb(0x22, 0x24, 0x36);

/// Splits the screen into `[sidebar, right side, mode line]`, the sidebar
/// `sidebar.width` columns wide, or none while it's hidden.
pub(crate) fn layout(area: Rect, sidebar: &SidebarView) -> [Rect; 3] {
    let [body, mode_line] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
    let width = if sidebar.hidden { 0 } else { sidebar.width };
    let [sidebar, right] =
        Layout::horizontal([Constraint::Length(width), Constraint::Fill(1)]).areas(body);
    [sidebar, right, mode_line]
}

/// Draws the whole frame. `pane` is the pane the frontend shows on the right
/// (the trust pane, or the selected attached thread's); its cursor shows only
/// while attached. Without one, the right side shows the dashboard, with
/// `pane_error` saying why the session couldn't start. While the sidebar or
/// the dashboard has the keys, the cursor sits on the first cell of its
/// selected row or highlighted item's label. The mode line's clock shows
/// `now` in `tz`. Returns the sidebar's layout unless it's hidden, and how
/// many rows the picker fits when it's open.
#[expect(
    clippy::too_many_arguments,
    reason = "the frame's inputs and the two frontend view states it updates"
)]
pub(crate) fn render(
    frame: &mut Frame,
    state: &AppState,
    pane: Option<&Pane>,
    pane_error: Option<&str>,
    keys: &Keys,
    now: SystemTime,
    tz: &TimeZone,
    scroll: &mut SidebarScroll,
    picker_scroll: &mut PickerScroll,
) -> (Option<SidebarLayout>, Option<usize>) {
    let [sidebar_area, right, mode_area] = layout(frame.area(), &state.sidebar);
    let (selected_y, sidebar_layout, search_cursor) = if state.sidebar.hidden {
        (None, None, None)
    } else {
        let (selected_y, sidebar_layout, search_cursor) = sidebar::render(
            &state.sessions,
            &state.attached,
            now,
            sidebar_area,
            frame.buffer_mut(),
            scroll,
        );
        (selected_y, Some(sidebar_layout), search_cursor)
    };
    let attached = state.focus == Focus::Attached;
    match pane {
        Some(pane) => {
            if let Some(cursor) = pane.render(right, frame.buffer_mut()).filter(|_| attached) {
                frame.set_cursor_position(cursor);
            }
        }
        None => {
            let at = dashboard::render(state, pane_error, right, frame.buffer_mut());
            if state.focus == Focus::Dashboard && right.contains(at) {
                frame.set_cursor_position(at);
            }
        }
    }
    mode_line::render(state, now, tz, mode_area, frame.buffer_mut());
    let renaming = state
        .rename
        .as_ref()
        .filter(|_| state.focus == Focus::Rename);
    let picker_page = match (&state.picker, renaming) {
        (Some(picker), _) => {
            let (rows, cursor) = picker::render(
                picker,
                &state.home,
                sidebar_area.union(right),
                frame.buffer_mut(),
                picker_scroll,
            );
            frame.set_cursor_position(cursor);
            Some(rows)
        }
        (None, Some(rename)) => {
            let cursor = rename::render(rename, sidebar_area.union(right), frame.buffer_mut());
            frame.set_cursor_position(cursor);
            None
        }
        (None, None) => {
            which_key::render(keys, sidebar_area.union(right), frame.buffer_mut());
            None
        }
    };
    match (state.focus, selected_y, search_cursor) {
        (Focus::Sidebar, Some(y), _) => frame.set_cursor_position((sidebar_area.x, y)),
        (Focus::Search, _, Some(cursor)) => frame.set_cursor_position(cursor),
        _ => {}
    }
    for cell in &mut frame.buffer_mut().content {
        if cell.bg == Color::Reset {
            cell.bg = BACKGROUND;
        }
    }
    (sidebar_layout, picker_page)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::thread;
    use std::time::{Duration, Instant, SystemTime};

    use jiff::tz::TimeZone;
    use orb_domain::feat::picker::state::PickerState;
    use orb_domain::feat::sessions::state::{
        Draft, DraftWorkspace, Project, ProjectId, Search, Sessions, SidebarItem, Thread, ThreadId,
        ThreadStatus,
    };
    use orb_domain::feat::sidebar::state::{Rename, SidebarView};
    use orb_domain::{AppState, Focus, TextInput};
    use orb_term::{Pane, PaneCommand, PaneSize};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::{Position, Rect};

    use super::{BACKGROUND, layout, render};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::keymap::{Keys, Scope, keymap, press};
    use crate::picker::PickerScroll;
    use crate::sidebar::SidebarScroll;

    fn thread(id: i64, status: ThreadStatus) -> Thread {
        Thread {
            id: ThreadId(id),
            title: Some("Fix the bug".to_owned()),
            cwd: "/Users/me/dev/orb".into(),
            transcript: None,
            status,
            turn_started_at: None,
            attach_argv: vec![],
            branch: None,
            pinned_at: None,
            settled_at: None,
            active_since: SystemTime::UNIX_EPOCH,
            last_activity_at: SystemTime::UNIX_EPOCH,
            unseen: false,
        }
    }

    fn sessions(threads: Vec<Thread>) -> Sessions {
        Sessions {
            projects: vec![Project {
                id: ProjectId(1),
                title: "orb".to_owned(),
                root: "/Users/me/dev/orb".into(),
                created_at: SystemTime::UNIX_EPOCH,
                removed: false,
                draft: None,
                threads,
            }],
            ..Sessions::default()
        }
    }

    /// Draws `state` on an 80x8 screen.
    fn draw(state: &AppState) -> Buffer {
        draw_with(state, &Keys::new(keymap(), Scope::Sidebar))
    }

    /// Draws `state` on an 80x8 screen with `keys` pending.
    fn draw_with(state: &AppState, keys: &Keys) -> Buffer {
        frame(state, None, keys, 8).backend().buffer().clone()
    }

    /// Draws `state` on an 80x40 screen, tall enough for the whole dashboard.
    fn draw_tall(state: &AppState, pane_error: Option<&str>) -> Buffer {
        let keys = Keys::new(keymap(), Scope::Dashboard);
        frame(state, pane_error, &keys, 40)
            .backend()
            .buffer()
            .clone()
    }

    /// The terminal after drawing `state` on an 80-column screen `height`
    /// rows tall, with `pane_error` and `keys` pending.
    fn frame(
        state: &AppState,
        pane_error: Option<&str>,
        keys: &Keys,
        height: u16,
    ) -> Terminal<TestBackend> {
        let Ok(mut terminal) = Terminal::new(TestBackend::new(80, height));
        let Ok(_) = terminal.draw(|frame| {
            render(
                frame,
                state,
                None,
                pane_error,
                keys,
                SystemTime::UNIX_EPOCH,
                &TimeZone::UTC,
                &mut SidebarScroll::default(),
                &mut PickerScroll::default(),
            );
        });
        terminal
    }

    /// A live pane that printed `PANE-TEXT`, once the text is on its screen.
    fn pane_with_text() -> Option<Pane> {
        let command = PaneCommand {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf PANE-TEXT; sleep 5".into(),
            ],
            cwd: "/".into(),
            env: vec![("PATH".into(), "/bin:/usr/bin".into())],
        };
        let pane = Pane::spawn(&command, PaneSize { cols: 48, rows: 7 }, |_| {}).ok()?;
        let area = Rect::new(0, 0, 48, 7);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let mut buffer = Buffer::empty(area);
            pane.render(area, &mut buffer);
            if text(&buffer, area).contains("PANE-TEXT") {
                return Some(pane);
            }
            thread::sleep(Duration::from_millis(10));
        }
        None
    }

    /// Draws `state` on an 80x8 screen with `pane` running for the selected
    /// thread; returns the right side's text.
    fn right_side_with_pane(state: &AppState, pane: &Pane) -> String {
        let buffer = draw_with_pane(state, pane);
        let [_, right, _] = layout(buffer.area, &SidebarView::default());
        text(&buffer, right)
    }

    /// Draws `state` on an 80x8 screen with `pane` running for the selected
    /// thread.
    fn draw_with_pane(state: &AppState, pane: &Pane) -> Buffer {
        let Ok(mut terminal) = Terminal::new(TestBackend::new(80, 8));
        let keys = Keys::new(keymap(), Scope::Sidebar);
        let Ok(_) = terminal.draw(|frame| {
            render(
                frame,
                state,
                Some(pane),
                None,
                &keys,
                SystemTime::UNIX_EPOCH,
                &TimeZone::UTC,
                &mut SidebarScroll::default(),
                &mut PickerScroll::default(),
            );
        });
        terminal.backend().buffer().clone()
    }

    /// Thread 1, selected, with the given focus.
    fn selected(focus: Focus) -> AppState {
        AppState {
            focus,
            sessions: Sessions {
                cursor: Some(SidebarItem::Thread(ThreadId(1))),
                ..sessions(vec![thread(1, ThreadStatus::Idle)])
            },
            ..AppState::default()
        }
    }

    /// orb's local draft, selected, with the given focus.
    fn drafted(focus: Focus) -> AppState {
        let mut sessions = Sessions {
            cursor: Some(SidebarItem::Draft(ProjectId(1))),
            ..sessions(vec![])
        };
        if let Some(project) = sessions.projects.first_mut() {
            project.draft = Some(Draft {
                workspace: DraftWorkspace::Local,
                branch: Some("dev".to_owned()),
                model: None,
                permission: None,
                created_at: SystemTime::UNIX_EPOCH,
                repo: true,
                from: None,
            });
        }
        AppState {
            focus,
            sessions,
            ..AppState::default()
        }
    }

    fn right_side(buffer: &Buffer) -> String {
        text(buffer, right_of(buffer))
    }

    /// The right side's area with the default sidebar.
    fn right_of(buffer: &Buffer) -> Rect {
        let [_, right, _] = layout(buffer.area, &SidebarView::default());
        right
    }

    /// The text inside `area`, row by row.
    fn text(buffer: &Buffer, area: Rect) -> String {
        area.rows()
            .map(|row| {
                row.columns()
                    .filter_map(|cell| buffer.cell(cell).map(Cell::symbol))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn mode_line(buffer: &Buffer) -> String {
        let [_, _, mode_line] = layout(buffer.area, &SidebarView::default());
        text(buffer, mode_line)
    }

    #[rstest::rstest]
    fn selected_thread_shows_the_dashboard() {
        // Given a selected thread.
        let state = selected(Focus::Sidebar);

        // When drawing a frame.
        let buffer = draw_tall(&state, None);

        // Then the right side is the dashboard's thread menu.
        let right = text(&buffer, right_of(&buffer));
        assert!(right.contains("Open session"), "right side was\n{right}");
    }

    #[rstest::rstest]
    fn pane_error_shows_on_the_dashboard() {
        // Given a selected thread whose session couldn't start.
        let state = selected(Focus::Dashboard);

        // When drawing a frame with the pane's error.
        let buffer = draw_tall(&state, Some("claude attach failed"));

        // Then the error is on the right side.
        let right = text(&buffer, right_of(&buffer));
        assert!(
            right.contains("claude attach failed"),
            "right side was\n{right}"
        );
    }

    #[rstest::rstest]
    #[case::empty_right_side(AppState::default(), (79, 0))]
    #[case::inside_the_picker(
        AppState {
            focus: Focus::Picker,
            picker: Some(PickerState::projects(vec![], Focus::Sidebar)),
            ..AppState::default()
        },
        (70, 1)
    )]
    fn cells_without_a_background_get_orbs(#[case] state: AppState, #[case] at: (u16, u16)) {
        // Given a frame's state.

        // When drawing a frame.
        let buffer = draw(&state);

        // Then a cell nothing coloured has orb's background.
        assert_eq!(
            buffer.cell(at).map(|cell| cell.bg),
            Some(BACKGROUND),
            "background at {at:?}"
        );
    }

    #[rstest::rstest]
    fn attached_pane_default_background_gets_orbs() {
        // Given a live pane that printed text with no background colour.
        let pane = pane_with_text();
        let state = selected(Focus::Attached);

        // When drawing a frame while attached.
        let buffer = pane.as_ref().map(|pane| draw_with_pane(&state, pane));

        // Then the pane's first cell has orb's background.
        let [_, right, _] = layout(Rect::new(0, 0, 80, 8), &SidebarView::default());
        let bg = buffer.and_then(|buffer| buffer.cell((right.x, right.y)).map(|cell| cell.bg));
        assert_eq!(bg, Some(BACKGROUND), "the pane's background");
    }

    /// Thread 1 selected, with the rename box open on it holding "Fix" and
    /// the keys in `focus`.
    fn renaming(focus: Focus) -> AppState {
        AppState {
            rename: Some(Rename {
                thread: ThreadId(1),
                input: TextInput::new("Fix"),
            }),
            ..selected(focus)
        }
    }

    #[rstest::rstest]
    fn rename_box_puts_the_cursor_after_the_name() {
        // Given the rename box holding "Fix" with the keys.
        let state = renaming(Focus::Rename);

        // When drawing a frame.
        let cursor = cursor_of(&state, None);

        // Then the cursor is after the name in the box, centred on the
        // 80-column screen two rows from the top.
        assert_eq!(
            cursor,
            Some(Position::new(18, 3)),
            "the cursor in the rename box"
        );
    }

    #[rstest::rstest]
    fn rename_box_is_not_drawn_without_the_keys() {
        // Given a rename box left open while the sidebar has the keys.
        let state = renaming(Focus::Sidebar);

        // When drawing a frame.
        let buffer = draw(&state);

        // Then no rename box is drawn.
        let screen: String = buffer.content.iter().map(Cell::symbol).collect();
        assert!(
            !screen.contains("Rename Session"),
            "the rename box shows only while it has the keys"
        );
    }

    #[rstest::rstest]
    fn leader_popup_on_a_two_row_screen_still_draws_the_mode_line() {
        // Given Space pressed on a two-row screen, too short for the popup.
        let Ok(mut terminal) = Terminal::new(TestBackend::new(80, 2));
        let mut keys = Keys::new(keymap(), Scope::Sidebar);
        press(
            &mut keys,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        );

        // When drawing a frame.
        let state = AppState::default();
        let Ok(_) = terminal.draw(|frame| {
            render(
                frame,
                &state,
                None,
                None,
                &keys,
                SystemTime::UNIX_EPOCH,
                &TimeZone::UTC,
                &mut SidebarScroll::default(),
                &mut PickerScroll::default(),
            );
        });

        // Then the frame is drawn without the popup.
        let mode_line = mode_line(terminal.backend().buffer());
        assert!(
            mode_line.starts_with(" NORMAL"),
            "mode line was '{mode_line}'"
        );
    }

    #[rstest::rstest]
    fn leader_popup_sits_above_the_mode_line() {
        // Given Space pressed on a 20-row screen.
        let Ok(mut terminal) = Terminal::new(TestBackend::new(80, 20));
        let mut keys = Keys::new(keymap(), Scope::Sidebar);
        press(
            &mut keys,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        );

        // When drawing a frame.
        let state = AppState::default();
        let Ok(_) = terminal.draw(|frame| {
            render(
                frame,
                &state,
                None,
                None,
                &keys,
                SystemTime::UNIX_EPOCH,
                &TimeZone::UTC,
                &mut SidebarScroll::default(),
                &mut PickerScroll::default(),
            );
        });

        // Then the popup's bottom-right corner is on the row above the mode
        // line.
        let corner = terminal.backend().buffer().cell((78, 18)).map(Cell::symbol);
        assert_eq!(corner, Some("╯"), "the popup's bottom-right corner");
    }

    #[rstest::rstest]
    fn attached_focus_draws_the_live_pane() {
        // Given a live pane for the selected thread, attached.
        let pane = pane_with_text();
        let state = selected(Focus::Attached);

        // When drawing a frame.
        let right = pane.as_ref().map(|pane| right_side_with_pane(&state, pane));

        // Then the pane's output is on the right side.
        assert!(
            right
                .as_deref()
                .is_some_and(|right| right.contains("PANE-TEXT")),
            "right side was {right:?}"
        );
    }

    /// Thread 1, selected in the sidebar and attached.
    fn left_pane() -> AppState {
        AppState {
            attached: HashSet::from([ThreadId(1)]),
            ..selected(Focus::Sidebar)
        }
    }

    #[rstest::rstest]
    fn sidebar_focus_draws_the_passed_pane() {
        // Given a live pane passed with the sidebar focused on a thread not in `attached`.
        let pane = pane_with_text();
        let state = selected(Focus::Sidebar);

        // When drawing a frame.
        let right = pane.as_ref().map(|pane| right_side_with_pane(&state, pane));

        // Then the pane's output is on the right side.
        assert!(
            right
                .as_deref()
                .is_some_and(|right| right.contains("PANE-TEXT")),
            "right side was {right:?}"
        );
    }

    #[rstest::rstest]
    fn sidebar_focus_without_a_pane_draws_the_dashboard() {
        // Given thread 1 attached and selected in the sidebar, with no pane passed.
        let state = left_pane();

        // When drawing a frame.
        let buffer = draw_tall(&state, None);

        // Then the right side is the dashboard's thread menu.
        let right = right_side(&buffer);
        assert!(right.contains("Open session"), "right side was\n{right}");
    }

    #[rstest::rstest]
    fn sidebar_focus_puts_the_cursor_on_the_sidebar_not_the_left_pane() {
        // Given a live pane passed for the selected thread, with the sidebar focused.
        let pane = pane_with_text();
        let state = selected(Focus::Sidebar);

        // When drawing a frame.
        let cursor = pane.as_ref().map(|pane| cursor_of(&state, Some(pane)));

        // Then the cursor is on the selected row's first cell, not in the pane.
        assert_eq!(
            cursor,
            Some(Some(Position::new(0, 3))),
            "the cursor with the pane left shown"
        );
    }

    /// Where the cursor shows once `state` is drawn on an 80x8 screen with
    /// `pane` running for the selected thread; `None` while it's hidden.
    fn cursor_of(state: &AppState, pane: Option<&Pane>) -> Option<Position> {
        let Ok(mut terminal) = Terminal::new(TestBackend::new(80, 8));
        let keys = Keys::new(keymap(), Scope::Sidebar);
        let Ok(_) = terminal.draw(|frame| {
            render(
                frame,
                state,
                pane,
                None,
                &keys,
                SystemTime::UNIX_EPOCH,
                &TimeZone::UTC,
                &mut SidebarScroll::default(),
                &mut PickerScroll::default(),
            );
        });
        terminal
            .backend()
            .cursor_visible()
            .then(|| terminal.get_cursor_position().ok())
            .flatten()
    }

    #[rstest::rstest]
    fn search_focus_puts_the_cursor_after_the_search_text() {
        // Given a search holding "ab" with the keys.
        let mut state = selected(Focus::Search);
        state.sessions.search = Some(Search {
            input: TextInput::new("ab"),
            return_to: None,
        });

        // When drawing a frame.
        let cursor = cursor_of(&state, None);

        // Then the cursor is after "> ab" in the input box.
        assert_eq!(cursor, Some(Position::new(5, 1)), "the search cursor");
    }

    #[rstest::rstest]
    fn sidebar_focus_puts_the_cursor_on_the_selected_rows_first_cell() {
        // Given thread 1 selected with the sidebar focused.
        let state = selected(Focus::Sidebar);

        // When drawing a frame.
        let cursor = cursor_of(&state, None);

        // Then the cursor is on the first cell of the selected row's first line.
        assert_eq!(cursor, Some(Position::new(0, 3)), "the sidebar cursor");
    }

    #[rstest::rstest]
    fn sidebar_cursor_follows_the_selection_to_a_later_row() {
        // Given thread 1 and a settled thread, with the sidebar focused on
        // the Settled header below thread 1's three-line node.
        let state = AppState {
            focus: Focus::Sidebar,
            sessions: Sessions {
                cursor: Some(SidebarItem::SettledShelf),
                ..sessions(vec![
                    thread(1, ThreadStatus::Idle),
                    Thread {
                        settled_at: Some(SystemTime::UNIX_EPOCH),
                        ..thread(2, ThreadStatus::Stopped)
                    },
                ])
            },
            ..AppState::default()
        };

        // When drawing a frame.
        let cursor = cursor_of(&state, None);

        // Then the cursor is on the first cell of the header's row.
        assert_eq!(cursor, Some(Position::new(0, 6)), "the sidebar cursor");
    }

    #[rstest::rstest]
    fn attached_focus_puts_the_cursor_in_the_pane_not_the_sidebar() {
        // Given a live pane for the selected thread, attached.
        let pane = pane_with_text();
        let state = selected(Focus::Attached);

        // When drawing a frame.
        let cursor = pane.as_ref().and_then(|pane| cursor_of(&state, Some(pane)));

        // Then the cursor is on the right side, in the pane.
        let [_, right, _] = layout(Rect::new(0, 0, 80, 8), &state.sidebar);
        assert!(
            cursor.is_some_and(|cursor| right.contains(cursor)),
            "the attached cursor was {cursor:?}, outside {right:?}"
        );
    }

    #[rstest::rstest]
    fn dashboard_focus_puts_the_cursor_on_the_highlighted_label() {
        // Given thread 1 selected with the dashboard focused.
        let state = selected(Focus::Dashboard);

        // When drawing a frame tall enough for the dashboard.
        let mut terminal = frame(&state, None, &Keys::new(keymap(), Scope::Dashboard), 40);

        // Then the cursor is shown on the first cell of Open session's label.
        let cursor = terminal
            .backend()
            .cursor_visible()
            .then(|| terminal.get_cursor_position().ok())
            .flatten();
        let label = cursor.map(|at| {
            let buffer = terminal.backend().buffer();
            text(buffer, Rect::new(at.x, at.y, buffer.area.right() - at.x, 1))
        });
        assert!(
            label
                .as_deref()
                .is_some_and(|label| label.starts_with("Open session")),
            "the cursor's row from the cursor was {label:?}"
        );
    }

    #[rstest::rstest]
    fn selected_draft_shows_the_dashboard() {
        // Given orb's draft selected.
        let state = drafted(Focus::Sidebar);

        // When drawing a frame.
        let right = right_side(&draw_tall(&state, None));

        // Then the right side is the dashboard's draft menu.
        assert!(right.contains("Start session"), "right side was\n{right}");
    }

    #[rstest::rstest]
    fn draft_cursor_before_its_draft_exists_shows_nothing_selected() {
        // Given the cursor on orb's draft before the actor made it.
        let state = AppState {
            sessions: Sessions {
                cursor: Some(SidebarItem::Draft(ProjectId(1))),
                ..sessions(vec![])
            },
            ..AppState::default()
        };

        // When drawing a frame.
        let right = right_side(&draw_tall(&state, None));

        // Then the dashboard says nothing is selected.
        assert!(
            right.contains("no session selected"),
            "right side was\n{right}"
        );
    }

    #[rstest::rstest]
    fn hidden_sidebar_gives_the_right_side_the_whole_body() {
        // Given a hidden sidebar on an 80x8 screen.
        let sidebar = SidebarView {
            hidden: true,
            ..SidebarView::default()
        };

        // When laying the screen out.
        let [_, right, _] = layout(Rect::new(0, 0, 80, 8), &sidebar);

        // Then the right side is everything above the mode line.
        assert_eq!(right, Rect::new(0, 0, 80, 7), "the right side's area");
    }

    #[rstest::rstest]
    fn sidebar_takes_its_width() {
        // Given a 40-column sidebar.
        let sidebar = SidebarView {
            width: 40,
            ..SidebarView::default()
        };

        // When laying an 80x8 screen out.
        let [sidebar_area, _, _] = layout(Rect::new(0, 0, 80, 8), &sidebar);

        // Then the sidebar is 40 columns wide.
        assert_eq!(sidebar_area.width, 40, "the sidebar's width");
    }

    /// `state` with the sidebar hidden.
    fn hidden(state: AppState) -> AppState {
        AppState {
            sidebar: SidebarView {
                hidden: true,
                ..SidebarView::default()
            },
            ..state
        }
    }

    #[rstest::rstest]
    fn hidden_sidebar_centres_the_dashboard_on_the_whole_width() {
        // Given orb's draft selected with the sidebar hidden.
        let state = hidden(drafted(Focus::Dashboard));

        // When drawing a frame 80 columns wide.
        let buffer = draw(&state);

        // Then the 25-column banner's second row starts in column 27.
        let row = text(&buffer, Rect::new(27, 1, 25, 1));
        assert_eq!(row, "██╔═══██╗██╔══██╗██╔══██╗", "the banner's second row");
    }

    #[rstest::rstest]
    fn hidden_sidebar_draws_the_attached_pane_from_the_left_edge() {
        // Given a live pane for the selected thread, attached, with the sidebar hidden.
        let pane = pane_with_text();
        let state = hidden(selected(Focus::Attached));

        // When drawing a frame.
        let screen = pane.as_ref().map(|pane| {
            let buffer = draw_with_pane(&state, pane);
            text(&buffer, buffer.area)
        });

        // Then the pane's output starts in the first column.
        assert!(
            screen
                .as_deref()
                .is_some_and(|screen| screen.starts_with("PANE-TEXT")),
            "screen was {screen:?}"
        );
    }

    #[rstest::rstest]
    fn hidden_sidebar_returns_no_layout() {
        // Given a selected thread with the sidebar hidden.
        let state = hidden(selected(Focus::Dashboard));

        // When drawing a frame.
        let Ok(mut terminal) = Terminal::new(TestBackend::new(80, 8));
        let mut sidebar_layout = None;
        let Ok(_) = terminal.draw(|frame| {
            (sidebar_layout, _) = render(
                frame,
                &state,
                None,
                None,
                &Keys::new(keymap(), Scope::Dashboard),
                SystemTime::UNIX_EPOCH,
                &TimeZone::UTC,
                &mut SidebarScroll::default(),
                &mut PickerScroll::default(),
            );
        });

        // Then there's no sidebar layout to replace the last drawn one.
        assert_eq!(sidebar_layout, None, "a hidden sidebar isn't laid out");
    }
}
