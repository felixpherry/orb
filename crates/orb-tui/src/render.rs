//! Draws a frame: the sidebar on the left, the shown session's tabs or the
//! start screen on the right, the mode line at the bottom, and the which-key
//! popup on top while a key sequence is pending.
//! While the sidebar is hidden, the right side takes the full width.
//! An open picker is drawn over everything but the mode line, without the
//! popup (the session and worktree pickers in their own snacks layout), and
//! so is the rename box while it has the keys. A confirm opened over a
//! picker draws on top of it, and only the confirm takes clicks.
//! The terminal's cursor is shown only where the keys are: on the sidebar's
//! selected row, at the text cursor of the picker, the rename box or the
//! sidebar search, or in the focused pane.
//! Each frame records where it drew the sidebar's rows and input box, the
//! shown panes, the pickers' rows and the rename box, so a click maps back
//! to them.
//! Whatever is left on the terminal's default background gets orb's navy, so
//! a transparent terminal doesn't show through.

use std::collections::HashMap;
use std::time::SystemTime;

use jiff::tz::TimeZone;
use orb_domain::feat::picker::state::{PickerKind, PickerState};
use orb_domain::feat::sessions::state::PaneId;
use orb_domain::feat::sidebar::state::{SidebarLayout, SidebarView};
use orb_domain::{AppState, Focus};
use orb_term::Pane;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Color;

use crate::dashboard;
use crate::keymap::Keys;
use crate::mode_line;
use crate::mouse::HitMap;
use crate::picker::{self, PickerScroll};
use crate::rename;
use crate::search_picker;
use crate::session_picker;
use crate::sidebar::{self, SidebarScroll};
use crate::tabs;
use crate::which_key;
use crate::worktree_picker;

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

/// Draws the whole frame. The right side shows the shown layout's tabs with
/// each of its panes that has a client in `panes`; the focused pane's cursor
/// shows only while the keys are in it. Without a shown layout, the right
/// side shows the start screen, with `pane_error` saying why the session
/// couldn't start. While the sidebar has the keys, the cursor sits on the
/// first cell of its selected row. The mode line's clock shows
/// `now` in `tz`. Returns the sidebar's layout unless it's hidden, and how
/// many rows the picker fits when it's open.
#[expect(
    clippy::too_many_arguments,
    reason = "the frame's inputs, the two frontend view states it updates, and the hit map it fills"
)]
pub(crate) fn render(
    frame: &mut Frame,
    state: &AppState,
    panes: &HashMap<PaneId, Pane>,
    pane_error: Option<&str>,
    keys: &Keys,
    now: SystemTime,
    tz: &TimeZone,
    scroll: &mut SidebarScroll,
    picker_scroll: &mut PickerScroll,
    hits: &mut HitMap,
) -> (Option<SidebarLayout>, Option<usize>) {
    let [sidebar_area, right, mode_area] = layout(frame.area(), &state.sidebar);
    *hits = HitMap::new(sidebar_area, right);
    let (selected_y, sidebar_layout, search_cursor) = if state.sidebar.hidden {
        (None, None, None)
    } else {
        let (selected_y, sidebar_layout, search_cursor) = sidebar::render(
            &state.sessions,
            &state.attached,
            &state.harnesses,
            now,
            sidebar_area,
            frame.buffer_mut(),
            scroll,
            hits,
        );
        (selected_y, Some(sidebar_layout), search_cursor)
    };
    match state.shown_layout() {
        Some(layout) => {
            let keys_in_pane = state.focus == Focus::Pane;
            if let Some(cursor) =
                tabs::render(layout, panes, keys_in_pane, right, frame.buffer_mut(), hits)
            {
                frame.set_cursor_position(cursor);
            }
        }
        None => dashboard::render(state, pane_error, right, frame.buffer_mut()),
    }
    mode_line::render(state, now, tz, mode_area, frame.buffer_mut());
    let renaming = state
        .rename
        .as_ref()
        .filter(|_| state.focus == Focus::Rename);
    let picker_page = match (&state.picker, renaming) {
        (Some(picker), _) => {
            let area = sidebar_area.union(right);
            let buf = frame.buffer_mut();
            let (rows, cursor) = match picker.under() {
                Some(list) => {
                    render_picker(
                        list,
                        state,
                        now,
                        area,
                        buf,
                        picker_scroll,
                        &mut HitMap::default(),
                    );
                    render_picker(
                        picker,
                        state,
                        now,
                        area,
                        buf,
                        &mut PickerScroll::default(),
                        hits,
                    )
                }
                None => render_picker(picker, state, now, area, buf, picker_scroll, hits),
            };
            frame.set_cursor_position(cursor);
            Some(rows)
        }
        (None, Some(rename)) => {
            let cursor =
                rename::render(rename, sidebar_area.union(right), frame.buffer_mut(), hits);
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

/// Draws `picker` over `area` with its own look, the session and worktree
/// pickers in their snacks layout and the rest in the select popup.
/// Returns how many rows fit and where the terminal cursor goes.
fn render_picker(
    picker: &PickerState,
    state: &AppState,
    now: SystemTime,
    area: Rect,
    buf: &mut Buffer,
    scroll: &mut PickerScroll,
    hits: &mut HitMap,
) -> (usize, Position) {
    match picker.kind() {
        PickerKind::Sessions { .. } => session_picker::render(
            picker,
            &state.sessions,
            &state.attached,
            &state.harnesses,
            now,
            area,
            buf,
            scroll,
            hits,
        ),
        PickerKind::Worktrees => {
            worktree_picker::render(picker, state, now, area, buf, scroll, hits)
        }
        PickerKind::Search { .. } => {
            search_picker::render(picker, state, now, area, buf, scroll, hits)
        }
        _ => picker::render(
            picker,
            &state.home,
            &state.sessions,
            area,
            buf,
            scroll,
            hits,
        ),
    }
}

#[cfg(test)]
mod tests {
    use orb_domain::feat::harness::HarnessId;
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;
    use std::thread;
    use std::time::{Duration, Instant, SystemTime};

    use crate::test_support::sessions_for;
    use jiff::tz::TimeZone;
    use orb_domain::Intent;
    use orb_domain::feat::layout::state::{PaneEntry, SessionLayout};
    use orb_domain::feat::layout::tree::Split;
    use orb_domain::feat::picker::list::PickerItem;
    use orb_domain::feat::picker::state::PickerState;
    use orb_domain::feat::search::state::SearchProgress;
    use orb_domain::feat::sessions::state::{
        PaneId, PaneLaunch, Project, ProjectId, ProjectKind, Search, SessionId, Sessions,
        SidebarItem, Thread, ThreadId, ThreadStatus,
    };
    use orb_domain::feat::sidebar::state::{Rename, RenameTarget, SidebarView};
    use orb_domain::feat::zmx::zmx_service::ZmxSession;
    use orb_domain::{AppState, Focus, TextInput};
    use orb_term::{Pane, PaneCommand, PaneSize};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::{Position, Rect};

    use super::{BACKGROUND, layout, render};
    use crate::sidebar::{BLUE, DARK3};
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    use crate::keymap::{Keys, LEADER, Scope, keymap, press};
    use crate::mouse::{self, Clicks, HitMap, MouseRoute};
    use crate::picker::PickerScroll;
    use crate::sidebar::SidebarScroll;

    /// `sessions` with the sessions its projects' threads run in.
    fn fill(sessions: Sessions) -> Sessions {
        Sessions {
            sessions: sessions_for(&sessions.projects),
            ..sessions
        }
    }

    fn thread(id: i64, status: ThreadStatus) -> Thread {
        Thread {
            last_session: None,
            harness: HarnessId::new("claude"),
            id: ThreadId(id),
            title: Some("Fix the bug".to_owned()),
            cwd: "/Users/me/dev/orb".into(),
            transcript: None,
            status,
            turn_started_at: None,
            pane: Some(PaneLaunch {
                pane: PaneId(id),
                session: SessionId(id),
            }),
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

    fn sessions(threads: Vec<Thread>) -> Sessions {
        fill(Sessions {
            projects: vec![Project {
                id: ProjectId(1),
                title: "orb".to_owned(),
                root: "/Users/me/dev/orb".into(),
                created_at: SystemTime::UNIX_EPOCH,
                removed: false,
                repo: true,
                threads,
                kind: ProjectKind::Normal,
            }],
            ..Sessions::default()
        })
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
        let keys = Keys::new(keymap(), Scope::Sidebar);
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
                &HashMap::new(),
                pane_error,
                keys,
                SystemTime::UNIX_EPOCH,
                &TimeZone::UTC,
                &mut SidebarScroll::default(),
                &mut PickerScroll::default(),
                &mut HitMap::default(),
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

    /// Thread 1's pane: a live pane that printed `PANE-TEXT`.
    fn thread_pane() -> Option<HashMap<PaneId, Pane>> {
        pane_with_text().map(|pane| HashMap::from([(PaneId(1), pane)]))
    }

    /// Draws `state` on an 80x8 screen with `panes` running; returns the
    /// right side's text.
    fn right_side_with_pane(state: &AppState, panes: &HashMap<PaneId, Pane>) -> String {
        let buffer = draw_with_pane(state, panes);
        let [_, right, _] = layout(buffer.area, &SidebarView::default());
        text(&buffer, right)
    }

    /// Draws `state` on an 80x8 screen with `panes` running.
    fn draw_with_pane(state: &AppState, panes: &HashMap<PaneId, Pane>) -> Buffer {
        let Ok(mut terminal) = Terminal::new(TestBackend::new(80, 8));
        let keys = Keys::new(keymap(), Scope::Sidebar);
        let Ok(_) = terminal.draw(|frame| {
            render(
                frame,
                state,
                panes,
                None,
                &keys,
                SystemTime::UNIX_EPOCH,
                &TimeZone::UTC,
                &mut SidebarScroll::default(),
                &mut PickerScroll::default(),
                &mut HitMap::default(),
            );
        });
        terminal.backend().buffer().clone()
    }

    /// Thread 1, selected, with the given focus.
    fn selected(focus: Focus) -> AppState {
        AppState {
            focus,
            sessions: Sessions {
                cursor: Some(SidebarItem::Session(SessionId(1))),
                ..sessions(vec![thread(1, ThreadStatus::Idle)])
            },
            ..AppState::default()
        }
    }

    /// Pane `id`, a shell in `orb-p<id>`.
    fn entry(id: i64) -> PaneEntry {
        PaneEntry {
            id: PaneId(id),
            zmx: ZmxSession {
                name: format!("orb-p{id}"),
                dir: "/tmp/zmx".into(),
            },
            cwd: "/tmp".into(),
            name: None,
            resume: None,
        }
    }

    /// Thread 1, selected and attached, running in pane 1, the one tab of
    /// session 1, with the given focus.
    fn shown(focus: Focus) -> AppState {
        let mut state = selected(focus);
        state.attached.insert(SessionId(1));
        if let Some(thread) = state
            .sessions
            .projects
            .first_mut()
            .and_then(|project| project.threads.first_mut())
        {
            thread.pane = Some(PaneLaunch {
                pane: PaneId(1),
                session: SessionId(1),
            });
        }
        state.sessions.sessions = sessions_for(&state.sessions.projects);
        state
            .layouts
            .insert(SessionId(1), SessionLayout::of(entry(1)));
        state
    }

    /// orb with no row selected, with the given focus.
    fn unselected(focus: Focus) -> AppState {
        AppState {
            focus,
            sessions: sessions(vec![]),
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
    fn session_picker_is_drawn_by_its_own_renderer() {
        // Given an open session picker with no threads.
        let state = AppState {
            focus: Focus::Picker,
            picker: Some(PickerState::sessions(vec![], Focus::Sidebar)),
            ..AppState::default()
        };

        // When drawing a frame.
        let buffer = draw(&state);

        // Then the input row shows the session picker's `0/0` count.
        let screen = text(&buffer, buffer.area);
        assert!(screen.contains("0/0"), "screen was\n{screen}");
    }

    #[rstest::rstest]
    fn worktree_picker_is_drawn_by_its_own_renderer() {
        // Given an open worktree picker with no worktrees.
        let state = AppState {
            focus: Focus::Picker,
            picker: Some(PickerState::worktrees(vec![], Focus::Sidebar)),
            ..AppState::default()
        };

        // When drawing a frame.
        let buffer = draw(&state);

        // Then the list box is titled ` Worktrees ` over a `0/0` count.
        let screen = text(&buffer, buffer.area);
        assert!(
            screen.contains(" Worktrees ") && screen.contains("0/0"),
            "screen was\n{screen}"
        );
    }

    #[rstest::rstest]
    fn search_picker_is_drawn_by_its_own_renderer() {
        // Given an open search picker while one of two transcripts is indexed.
        let state = AppState {
            focus: Focus::Picker,
            picker: Some(PickerState::search(Focus::Sidebar)),
            search: SearchProgress {
                indexed: 1,
                total: 2,
                error: None,
            },
            ..AppState::default()
        };

        // When drawing a frame.
        let buffer = draw(&state);

        // Then the list title shows the indexing progress.
        let screen = text(&buffer, buffer.area);
        assert!(screen.contains("indexing 1/2"), "screen was\n{screen}");
    }

    /// An open `Delete worktree?` confirm over a worktree picker listing
    /// `orb/orb-1234`.
    fn delete_worktree_confirm() -> AppState {
        let path = PathBuf::from("/Users/me/.orb/worktrees/orb/orb-1234");
        let list = PickerState::worktrees(
            vec![PickerItem::Worktree {
                path: path.clone(),
                label: "orb/orb-1234".to_owned(),
                split: 4,
                extra: String::new(),
            }],
            Focus::Sidebar,
        );
        AppState {
            focus: Focus::Picker,
            picker: Some(PickerState::delete_worktree(list, path, false)),
            ..AppState::default()
        }
    }

    /// Where `needle` first shows on `buffer`.
    fn find(buffer: &Buffer, needle: &str) -> Option<Position> {
        buffer.area.positions().find(|at| {
            let rest = Rect::new(at.x, at.y, buffer.area.right() - at.x, 1);
            text(buffer, rest).starts_with(needle)
        })
    }

    #[rstest::rstest]
    fn delete_worktree_confirm_draws_over_the_worktree_list() {
        // Given a delete confirm opened over the worktree picker.
        let state = delete_worktree_confirm();

        // When drawing a frame.
        let buffer = frame(&state, None, &Keys::new(keymap(), Scope::Sidebar), 20)
            .backend()
            .buffer()
            .clone();

        // Then the list's row and the confirm's title are both on screen.
        let screen = text(&buffer, buffer.area);
        assert!(
            screen.contains("orb/orb-1234") && screen.contains("Delete worktree?"),
            "screen was\n{screen}"
        );
    }

    #[rstest::rstest]
    fn click_on_a_list_row_behind_the_delete_confirm_cancels_the_confirm() {
        // Given a delete confirm drawn over the worktree picker.
        let state = delete_worktree_confirm();
        let mut hits = HitMap::default();
        let buffer = {
            let Ok(mut terminal) = Terminal::new(TestBackend::new(80, 20));
            let keys = Keys::new(keymap(), Scope::Sidebar);
            let Ok(_) = terminal.draw(|frame| {
                render(
                    frame,
                    &state,
                    &HashMap::new(),
                    None,
                    &keys,
                    SystemTime::UNIX_EPOCH,
                    &TimeZone::UTC,
                    &mut SidebarScroll::default(),
                    &mut PickerScroll::default(),
                    &mut hits,
                );
            });
            terminal.backend().buffer().clone()
        };
        let row = find(&buffer, "orb/orb-1234");

        // When clicking the list's row.
        let route = row.map(|at| {
            let event = MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: at.x,
                row: at.y,
                modifiers: KeyModifiers::NONE,
            };
            mouse::route(
                event,
                &hits,
                Focus::Picker,
                None,
                &mut Clicks::default(),
                Instant::now(),
            )
        });

        // Then the click cancels the confirm instead of selecting the row.
        assert_eq!(
            route,
            Some(MouseRoute::Intents(vec![Intent::PickerCancel])),
            "a click behind the confirm should close it"
        );
    }

    #[rstest::rstest]
    fn selected_thread_shows_the_start_screen() {
        // Given a selected thread that isn't attached.
        let state = selected(Focus::Sidebar);

        // When drawing a frame.
        let buffer = draw_tall(&state, None);

        // Then the right side is the start screen's banner.
        let right = text(&buffer, right_of(&buffer));
        assert!(
            right.contains("██╔═══██╗██╔══██╗██╔══██╗"),
            "right side was\n{right}"
        );
    }

    #[rstest::rstest]
    fn pane_error_shows_on_the_start_screen() {
        // Given a selected thread whose session couldn't start.
        let state = selected(Focus::Sidebar);

        // When drawing a frame with the pane's error.
        let buffer = draw_tall(&state, Some("zmx attach failed"));

        // Then the error is on the right side.
        let right = text(&buffer, right_of(&buffer));
        assert!(
            right.contains("zmx attach failed"),
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
        let panes = thread_pane();
        let state = shown(Focus::Pane);

        // When drawing a frame while attached.
        let buffer = panes.as_ref().map(|panes| draw_with_pane(&state, panes));

        // Then the pane's first cell, under the tab bar, has orb's background.
        let [_, right, _] = layout(Rect::new(0, 0, 80, 8), &SidebarView::default());
        let bg = buffer.and_then(|buffer| buffer.cell((right.x, right.y + 1)).map(|cell| cell.bg));
        assert_eq!(bg, Some(BACKGROUND), "the pane's background");
    }

    /// Thread 1 selected, with the rename box open on it holding "Fix" and
    /// the keys in `focus`.
    fn renaming(focus: Focus) -> AppState {
        AppState {
            rename: Some(Rename {
                target: RenameTarget::Session(SessionId(1)),
                input: TextInput::new("Fix"),
                creating: false,
            }),
            ..selected(focus)
        }
    }

    #[rstest::rstest]
    fn rename_box_puts_the_cursor_after_the_name() {
        // Given the rename box holding "Fix" with the keys.
        let state = renaming(Focus::Rename);

        // When drawing a frame.
        let cursor = cursor_of(&state, &HashMap::new());

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
        // Given `<C-g>` pressed on a two-row screen, too short for the popup.
        let Ok(mut terminal) = Terminal::new(TestBackend::new(80, 2));
        let mut keys = Keys::new(keymap(), Scope::Sidebar);
        press(&mut keys, LEADER);

        // When drawing a frame.
        let state = AppState::default();
        let Ok(_) = terminal.draw(|frame| {
            render(
                frame,
                &state,
                &HashMap::new(),
                None,
                &keys,
                SystemTime::UNIX_EPOCH,
                &TimeZone::UTC,
                &mut SidebarScroll::default(),
                &mut PickerScroll::default(),
                &mut HitMap::default(),
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
        // Given `<C-g>` pressed on a 20-row screen.
        let Ok(mut terminal) = Terminal::new(TestBackend::new(80, 20));
        let mut keys = Keys::new(keymap(), Scope::Sidebar);
        press(&mut keys, LEADER);

        // When drawing a frame.
        let state = AppState::default();
        let Ok(_) = terminal.draw(|frame| {
            render(
                frame,
                &state,
                &HashMap::new(),
                None,
                &keys,
                SystemTime::UNIX_EPOCH,
                &TimeZone::UTC,
                &mut SidebarScroll::default(),
                &mut PickerScroll::default(),
                &mut HitMap::default(),
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
        let panes = thread_pane();
        let state = shown(Focus::Pane);

        // When drawing a frame.
        let right = panes
            .as_ref()
            .map(|panes| right_side_with_pane(&state, panes));

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
            attached: HashSet::from([SessionId(1)]),
            ..selected(Focus::Sidebar)
        }
    }

    #[rstest::rstest]
    fn sidebar_focus_draws_the_shown_layouts_pane() {
        // Given thread 1's live pane shown while the sidebar has the keys.
        let panes = thread_pane();
        let state = shown(Focus::Sidebar);

        // When drawing a frame.
        let right = panes
            .as_ref()
            .map(|panes| right_side_with_pane(&state, panes));

        // Then the pane's output is on the right side.
        assert!(
            right
                .as_deref()
                .is_some_and(|right| right.contains("PANE-TEXT")),
            "right side was {right:?}"
        );
    }

    #[rstest::rstest]
    fn attached_thread_without_a_layout_shows_the_start_screen() {
        // Given thread 1 attached and selected in the sidebar, with no layout.
        let state = left_pane();

        // When drawing a frame.
        let buffer = draw_tall(&state, None);

        // Then the right side is the start screen's banner.
        let right = right_side(&buffer);
        assert!(
            right.contains("██╔═══██╗██╔══██╗██╔══██╗"),
            "right side was\n{right}"
        );
    }

    #[rstest::rstest]
    fn sidebar_focus_puts_the_cursor_on_the_sidebar_not_the_left_pane() {
        // Given thread 1's live pane shown, with the sidebar focused.
        let panes = thread_pane();
        let state = shown(Focus::Sidebar);

        // When drawing a frame.
        let cursor = panes.as_ref().map(|panes| cursor_of(&state, panes));

        // Then the cursor is on the selected row's first cell, not in the pane.
        assert_eq!(
            cursor,
            Some(Some(Position::new(0, 3))),
            "the cursor with the pane left shown"
        );
    }

    /// Where the cursor shows once `state` is drawn on an 80x8 screen with
    /// `panes` running; `None` while it's hidden.
    fn cursor_of(state: &AppState, panes: &HashMap<PaneId, Pane>) -> Option<Position> {
        let Ok(mut terminal) = Terminal::new(TestBackend::new(80, 8));
        let keys = Keys::new(keymap(), Scope::Sidebar);
        let Ok(_) = terminal.draw(|frame| {
            render(
                frame,
                state,
                panes,
                None,
                &keys,
                SystemTime::UNIX_EPOCH,
                &TimeZone::UTC,
                &mut SidebarScroll::default(),
                &mut PickerScroll::default(),
                &mut HitMap::default(),
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
        let cursor = cursor_of(&state, &HashMap::new());

        // Then the cursor is after "> ab" in the input box.
        assert_eq!(cursor, Some(Position::new(5, 1)), "the search cursor");
    }

    #[rstest::rstest]
    fn sidebar_focus_puts_the_cursor_on_the_selected_rows_first_cell() {
        // Given thread 1 selected with the sidebar focused.
        let state = selected(Focus::Sidebar);

        // When drawing a frame.
        let cursor = cursor_of(&state, &HashMap::new());

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
        let cursor = cursor_of(&state, &HashMap::new());

        // Then the cursor is on the first cell of the header's row.
        assert_eq!(cursor, Some(Position::new(0, 6)), "the sidebar cursor");
    }

    #[rstest::rstest]
    fn attached_focus_puts_the_cursor_in_the_focused_pane() {
        // Given a live pane for the selected thread, attached.
        let panes = thread_pane();
        let state = shown(Focus::Pane);

        // When drawing a frame.
        let cursor = panes.as_ref().and_then(|panes| cursor_of(&state, panes));

        // Then the cursor is in the pane, under the tab bar.
        let [_, right, _] = layout(Rect::new(0, 0, 80, 8), &state.sidebar);
        let body = Rect::new(right.x, right.y + 1, right.width, right.height - 1);
        assert!(
            cursor.is_some_and(|cursor| body.contains(cursor)),
            "the attached cursor was {cursor:?}, outside {body:?}"
        );
    }

    #[rstest::rstest]
    fn no_selection_shows_the_start_screen() {
        // Given no row selected.
        let state = unselected(Focus::Sidebar);

        // When drawing a frame.
        let right = right_side(&draw_tall(&state, None));

        // Then the right side is the start screen's banner.
        assert!(
            right.contains("██╔═══██╗██╔══██╗██╔══██╗"),
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
    fn hidden_sidebar_centres_the_start_screen_on_the_whole_width() {
        // Given no row selected with the sidebar hidden.
        let state = hidden(unselected(Focus::Sidebar));

        // When drawing a frame 80 columns wide.
        let buffer = draw(&state);

        // Then the 25-column banner's second row starts in column 27.
        let row = text(&buffer, Rect::new(27, 1, 25, 1));
        assert_eq!(row, "██╔═══██╗██╔══██╗██╔══██╗", "the banner's second row");
    }

    #[rstest::rstest]
    fn hidden_sidebar_draws_the_attached_pane_from_the_left_edge() {
        // Given a live pane for the selected thread, attached, with the sidebar hidden.
        let panes = thread_pane();
        let state = hidden(shown(Focus::Pane));

        // When drawing a frame.
        let row = panes.as_ref().map(|panes| {
            let buffer = draw_with_pane(&state, panes);
            text(&buffer, Rect::new(0, 1, 80, 1))
        });

        // Then the pane's output starts in the first column, under the tab bar.
        assert!(
            row.as_deref()
                .is_some_and(|row| row.starts_with("PANE-TEXT")),
            "the pane's first row was {row:?}"
        );
    }

    /// `shown(Focus::Sidebar)` after `edit` changed thread 1's layout.
    fn laid_out<F>(edit: F) -> AppState
    where
        F: FnOnce(&mut AppState),
    {
        let mut state = shown(Focus::Sidebar);
        edit(&mut state);
        state
    }

    #[rstest::rstest]
    fn tab_bar_labels_each_tab_by_number_and_name() {
        // Given a first tab named logs and a second unnamed tab.
        let state = laid_out(|state| {
            state
                .layouts
                .rename_tab(SessionId(1), 0, Some("logs".to_owned()));
            state.layouts.new_tab(SessionId(1), entry(2));
        });

        // When drawing a frame.
        let buffer = draw(&state);

        // Then the tab bar reads ` 1 logs  2 `.
        let right = right_of(&buffer);
        let bar = text(&buffer, Rect::new(right.x, right.y, right.width, 1));
        assert!(bar.starts_with(" 1 logs  2 "), "tab bar was '{bar}'");
    }

    #[rstest::rstest]
    fn unnamed_tab_shows_its_focused_panes_name() {
        // Given an unnamed tab whose focused pane is named "server".
        let state = laid_out(|state| {
            state
                .layouts
                .rename_pane(PaneId(1), Some("server".to_owned()));
        });

        // When drawing a frame.
        let buffer = draw(&state);

        // Then the tab bar reads ` 1 server `.
        let right = right_of(&buffer);
        let bar = text(&buffer, Rect::new(right.x, right.y, right.width, 1));
        assert!(bar.starts_with(" 1 server "), "tab bar was '{bar}'");
    }

    #[rstest::rstest]
    fn shown_tab_is_highlighted_in_the_tab_bar() {
        // Given two tabs, the first shown.
        let state = laid_out(|state| {
            state.layouts.new_tab(SessionId(1), entry(2));
            state.layouts.go_to_tab(SessionId(1), 1);
        });

        // When drawing a frame.
        let buffer = draw(&state);

        // Then the first tab's number sits on blue and the second's doesn't.
        let right = right_of(&buffer);
        let bg = |x| buffer.cell((x, right.y)).map(|cell| cell.bg);
        assert_eq!(
            (bg(right.x + 1), bg(right.x + 4)),
            (Some(BLUE), Some(BACKGROUND)),
            "only the shown tab is highlighted"
        );
    }

    #[rstest::rstest]
    fn split_right_draws_a_line_between_the_panes() {
        // Given thread 1's pane split right, neither pane running yet.
        let state = laid_out(|state| state.layouts.split(SessionId(1), Split::Right, entry(2)));

        // When drawing a frame on the 48-column right side.
        let buffer = draw(&state);

        // Then column 24 of the tab body is a line all the way down.
        let right = right_of(&buffer);
        let column: Vec<(Option<&str>, Option<_>)> = (right.y + 1..right.bottom())
            .map(|y| {
                let cell = buffer.cell((right.x + 24, y));
                (cell.map(Cell::symbol), cell.map(|cell| cell.fg))
            })
            .collect();
        assert_eq!(
            column,
            vec![(Some("│"), Some(DARK3)); 6],
            "the border between the panes"
        );
    }

    #[rstest::rstest]
    fn zoomed_tab_draws_no_border() {
        // Given thread 1's pane split right, then zoomed.
        let state = laid_out(|state| {
            state.layouts.split(SessionId(1), Split::Right, entry(2));
            state.layouts.toggle_zoom(SessionId(1));
        });

        // When drawing a frame.
        let buffer = draw(&state);

        // Then the tab body has no line in it.
        let right = right_of(&buffer);
        let body = text(
            &buffer,
            Rect::new(right.x, right.y + 1, right.width, right.height - 1),
        );
        assert!(!body.contains('│'), "body was\n{body}");
    }

    #[rstest::rstest]
    fn hidden_sidebar_returns_no_layout() {
        // Given a selected thread with the sidebar hidden.
        let state = hidden(selected(Focus::Sidebar));

        // When drawing a frame.
        let Ok(mut terminal) = Terminal::new(TestBackend::new(80, 8));
        let mut sidebar_layout = None;
        let Ok(_) = terminal.draw(|frame| {
            (sidebar_layout, _) = render(
                frame,
                &state,
                &HashMap::new(),
                None,
                &Keys::new(keymap(), Scope::Sidebar),
                SystemTime::UNIX_EPOCH,
                &TimeZone::UTC,
                &mut SidebarScroll::default(),
                &mut PickerScroll::default(),
                &mut HitMap::default(),
            );
        });

        // Then there's no sidebar layout to replace the last drawn one.
        assert_eq!(sidebar_layout, None, "a hidden sidebar isn't laid out");
    }
}
