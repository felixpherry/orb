//! Draws a frame: the sidebar on the left, the attached session or the
//! selected thread's preview on the right, the mode line at the bottom, and the
//! which-key popup on top while a key sequence is pending. While `s` or `x`
//! waits for its repeat, a banner above the selected thread says what the
//! repeat will do instead of the popup.

use std::time::SystemTime;

use orb_domain::feat::preview::state::PreviewLayout;
use orb_domain::feat::sessions::state::SidebarItem;
use orb_domain::feat::sessions::validator::{ToggleSettleError, validate_toggle_settle};
use orb_domain::{AppState, Focus};
use orb_term::Pane;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::Widget;
use ratatui_which_key::WhichKey;

use crate::keymap::{self, Keys};
use crate::preview::{self, PreviewCache};
use crate::sidebar::{self, SidebarScroll};

/// Splits the screen into `[sidebar, right side, mode line]`.
pub(crate) fn layout(area: Rect) -> [Rect; 3] {
    let [body, mode_line] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
    let [sidebar, right] =
        Layout::horizontal([Constraint::Length(32), Constraint::Fill(1)]).areas(body);
    [sidebar, right, mode_line]
}

/// Draws the whole frame. `pane` is the selected thread's session, if orb has
/// one running; it's drawn only while attached, and otherwise the right side
/// shows the selected thread's preview, with `pane_error` saying why the
/// session couldn't start. Returns the preview's layout when it was drawn.
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
    cache: &mut PreviewCache,
    scroll: &mut SidebarScroll,
) -> Option<PreviewLayout> {
    let [sidebar_area, right, mode_line] = layout(frame.area());
    let selected_y = sidebar::render(
        &state.sessions,
        now,
        sidebar_area,
        frame.buffer_mut(),
        scroll,
    );
    let preview_layout = match (pane, state.focus) {
        (Some(pane), Focus::Attached) => {
            if let Some(cursor) = pane.render(right, frame.buffer_mut()) {
                frame.set_cursor_position(cursor);
            }
            None
        }
        _ => match state.sessions.selected_thread() {
            Some(thread) => Some(preview::render(
                state,
                thread,
                pane_error,
                right,
                frame.buffer_mut(),
                cache,
            )),
            None => {
                let hint = match state.sessions.cursor {
                    Some(SidebarItem::SettledShelf) => shelf_hint(state),
                    _ => "␣n new session".to_owned(),
                };
                Line::raw(hint).render(right, frame.buffer_mut());
                None
            }
        },
    };
    render_mode_line(state, mode_line, frame.buffer_mut());
    match keymap::pending_confirm(keys) {
        Some(confirm) => {
            if let Some(y) = selected_y {
                render_banner(state, confirm, y, frame.buffer_mut());
            }
        }
        // ratatui-which-key divides by the height inside the popup's borders.
        None if frame.area().height > 2 => WhichKey::new().render(frame.buffer_mut(), keys),
        None => {}
    }
    preview_layout
}

/// What `⏎` does on the Settled header: `▸ Settled (N) · ⏎ open`, or
/// `▾ Settled · ⏎ close` while the shelf is open.
fn shelf_hint(state: &AppState) -> String {
    let sessions = &state.sessions;
    let count = sessions
        .threads()
        .filter(|thread| thread.settled_at.is_some())
        .count();
    let action = if sessions.shelf_open { "close" } else { "open" };
    format!(
        "{} · ⏎ {action}",
        sidebar::shelf_label(count, sessions.shelf_open)
    )
}

/// jinn's confirm banner, on the line above the selected row (the row itself
/// at the top), against the frame's right edge: yellow when repeating
/// `confirm` will act, red when it would be refused.
fn render_banner(state: &AppState, confirm: char, selected_y: u16, buf: &mut Buffer) {
    let Some(thread) = state.sessions.selected_thread() else {
        return;
    };
    let (text, colour) = match (confirm, validate_toggle_settle(state)) {
        ('x', _) => (" Press x again to delete ", Color::Yellow),
        (_, Ok(())) if thread.settled_at.is_some() => {
            (" Press s again to un-settle ", Color::Yellow)
        }
        (_, Ok(())) => (" Press s again to settle ", Color::Yellow),
        (_, Err(ToggleSettleError::InProgress)) => {
            (" Can't settle while Claude is working ", Color::Red)
        }
        (_, Err(ToggleSettleError::NoThread)) => return,
    };
    let banner = Line::styled(text, Style::new().fg(Color::Black).bg(colour));
    let area = buf.area;
    let width = u16::try_from(banner.width())
        .unwrap_or(u16::MAX)
        .min(area.width);
    banner.render(
        Rect::new(area.right() - width, selected_y.saturating_sub(1), width, 1),
        buf,
    );
}

/// The mode and its keys on the left; on the right, a session being started,
/// else the latest `claude` failure, else how many threads are working.
fn render_mode_line(state: &AppState, area: Rect, buf: &mut Buffer) {
    let mode = match state.focus {
        Focus::Attached => "ATTACHED   <C-\\> back",
        Focus::Sidebar | Focus::Preview => "NORMAL   ⏎ attach · ␣ leader",
    };
    let sessions = &state.sessions;
    let status = Line::raw(
        match (sessions.starting, &sessions.error, sessions.working_count()) {
            (true, _, _) => "starting session…".to_owned(),
            (false, Some(error), _) => error.clone(),
            (false, None, 0) => String::new(),
            (false, None, working) => format!("{working} running"),
        },
    );
    let status_width = u16::try_from(status.width()).unwrap_or(u16::MAX);
    let [mode_area, status_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(status_width)]).areas(area);
    Line::raw(mode).render(mode_area, buf);
    status.render(status_area, buf);
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::{Duration, Instant, SystemTime};

    use orb_domain::feat::preview::state::Preview;
    use orb_domain::feat::sessions::state::{
        Project, ProjectId, Sessions, SidebarItem, Thread, ThreadId, ThreadStatus,
    };
    use orb_domain::{AppState, Focus};
    use orb_term::{Pane, PaneCommand, PaneSize};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::Rect;

    use super::{layout, render};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::keymap::{Keys, keymap, press};
    use crate::preview::PreviewCache;
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
                threads,
            }],
            ..Sessions::default()
        }
    }

    /// Draws `state` on an 80x8 screen.
    fn draw(state: &AppState) -> Buffer {
        draw_with(state, &Keys::new(keymap(), Focus::Sidebar))
    }

    /// Draws `state` on an 80x8 screen with `keys` pending.
    fn draw_with(state: &AppState, keys: &Keys) -> Buffer {
        let Ok(mut terminal) = Terminal::new(TestBackend::new(80, 8));
        let Ok(_) = terminal.draw(|frame| {
            render(
                frame,
                state,
                None,
                None,
                keys,
                SystemTime::UNIX_EPOCH,
                &mut PreviewCache::default(),
                &mut SidebarScroll::default(),
            );
        });
        terminal.backend().buffer().clone()
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
        let Ok(mut terminal) = Terminal::new(TestBackend::new(80, 8));
        let keys = Keys::new(keymap(), Focus::Sidebar);
        let Ok(_) = terminal.draw(|frame| {
            render(
                frame,
                state,
                Some(pane),
                None,
                &keys,
                SystemTime::UNIX_EPOCH,
                &mut PreviewCache::default(),
                &mut SidebarScroll::default(),
            );
        });
        let buffer = terminal.backend().buffer();
        let [_, right, _] = layout(buffer.area);
        text(buffer, right)
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

    /// The keymap in Sidebar focus with `c` pressed once.
    fn pending(c: char) -> Keys {
        let mut keys = Keys::new(keymap(), Focus::Sidebar);
        press(
            &mut keys,
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
        );
        keys
    }

    fn mode_line(buffer: &Buffer) -> String {
        let [_, _, mode_line] = layout(buffer.area);
        text(buffer, mode_line)
    }

    #[rstest::rstest]
    fn selected_thread_without_messages_shows_how_to_attach() {
        // Given a selected thread whose transcript has no blocks yet.
        let state = AppState {
            sessions: Sessions {
                cursor: Some(SidebarItem::Thread(ThreadId(1))),
                ..sessions(vec![thread(1, ThreadStatus::Idle)])
            },
            preview: Preview {
                thread: Some(ThreadId(1)),
                ..Preview::default()
            },
            ..AppState::default()
        };

        // When drawing a frame.
        let buffer = draw(&state);

        // Then the right side offers to attach (cut at the 48-column edge).
        let [_, right, _] = layout(buffer.area);
        let right = text(&buffer, right);
        assert!(
            right.contains("No messages yet · ⏎ attach"),
            "right side was '{right}'"
        );
    }

    #[rstest::rstest]
    fn mode_line_counts_working_threads() {
        // Given three Working threads.
        let state = AppState {
            sessions: sessions(vec![
                thread(1, ThreadStatus::Working),
                thread(2, ThreadStatus::Working),
                thread(3, ThreadStatus::Working),
            ]),
            ..AppState::default()
        };

        // When drawing a frame.
        let buffer = draw(&state);

        // Then the mode line says how many are running.
        let mode_line = mode_line(&buffer);
        assert!(
            mode_line.contains("3 running"),
            "mode line was '{mode_line}'"
        );
    }

    #[rstest::rstest]
    fn mode_line_shows_the_latest_claude_error() {
        // Given a failed `claude` call.
        let state = AppState {
            sessions: Sessions {
                error: Some("Workspace not trusted".to_owned()),
                ..Sessions::default()
            },
            ..AppState::default()
        };

        // When drawing a frame.
        let buffer = draw(&state);

        // Then the mode line shows the reason.
        let mode_line = mode_line(&buffer);
        assert!(
            mode_line.contains("Workspace not trusted"),
            "mode line was '{mode_line}'"
        );
    }

    #[rstest::rstest]
    fn mode_line_shows_normal_outside_a_session() {
        // Given orb in Sidebar focus.
        let state = AppState::default();

        // When drawing a frame.
        let buffer = draw(&state);

        // Then the mode line starts with the mode.
        let mode_line = mode_line(&buffer);
        assert!(
            mode_line.starts_with("NORMAL"),
            "mode line was '{mode_line}'"
        );
    }

    #[rstest::rstest]
    fn mode_line_shows_attached_while_attached() {
        // Given orb attached to a session.
        let state = AppState {
            focus: Focus::Attached,
            ..AppState::default()
        };

        // When drawing a frame.
        let buffer = draw(&state);

        // Then the mode line says so.
        let mode_line = mode_line(&buffer);
        assert!(
            mode_line.starts_with("ATTACHED"),
            "mode line was '{mode_line}'"
        );
    }

    #[rstest::rstest]
    fn leader_popup_on_a_two_row_screen_still_draws_the_mode_line() {
        // Given Space pressed on a two-row screen, too short for the popup.
        let Ok(mut terminal) = Terminal::new(TestBackend::new(80, 2));
        let mut keys = Keys::new(keymap(), Focus::Sidebar);
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
                &mut PreviewCache::default(),
                &mut SidebarScroll::default(),
            );
        });

        // Then the frame is drawn without the popup.
        let mode_line = mode_line(terminal.backend().buffer());
        assert!(
            mode_line.starts_with("NORMAL"),
            "mode line was '{mode_line}'"
        );
    }

    #[rstest::rstest]
    fn pending_s_shows_the_settle_banner() {
        // Given an idle selected thread and `s` pressed once.
        let state = selected(Focus::Sidebar);

        // When drawing a frame.
        let buffer = draw_with(&state, &pending('s'));

        // Then the banner asks for the second `s`.
        let screen = text(&buffer, buffer.area);
        assert!(
            screen.contains(" Press s again to settle "),
            "screen was '{screen}'"
        );
    }

    #[rstest::rstest]
    fn pending_s_on_a_working_thread_shows_the_refusal() {
        // Given a Working selected thread and `s` pressed once.
        let state = AppState {
            sessions: Sessions {
                cursor: Some(SidebarItem::Thread(ThreadId(1))),
                ..sessions(vec![thread(1, ThreadStatus::Working)])
            },
            ..AppState::default()
        };

        // When drawing a frame.
        let buffer = draw_with(&state, &pending('s'));

        // Then the banner says the settle would be refused.
        let screen = text(&buffer, buffer.area);
        assert!(
            screen.contains(" Can't settle while Claude is working "),
            "screen was '{screen}'"
        );
    }

    #[rstest::rstest]
    fn pending_s_hides_the_which_key_popup() {
        // Given an idle selected thread and `s` pressed once.
        let state = selected(Focus::Sidebar);

        // When drawing a frame.
        let buffer = draw_with(&state, &pending('s'));

        // Then no popup border is drawn.
        let screen = text(&buffer, buffer.area);
        assert!(!screen.contains('┌'), "screen was '{screen}'");
    }

    #[rstest::rstest]
    fn shelf_header_selected_shows_the_open_hint() {
        // Given a collapsed shelf with one settled thread, its header selected.
        let state = AppState {
            sessions: Sessions {
                cursor: Some(SidebarItem::SettledShelf),
                ..sessions(vec![Thread {
                    settled_at: Some(SystemTime::UNIX_EPOCH),
                    ..thread(1, ThreadStatus::Stopped)
                }])
            },
            ..AppState::default()
        };

        // When drawing a frame.
        let buffer = draw(&state);

        // Then the right side says ⏎ opens the shelf.
        let [_, right, _] = layout(buffer.area);
        let right = text(&buffer, right);
        assert!(
            right.contains("▸ Settled (1) · ⏎ open"),
            "right side was '{right}'"
        );
    }

    #[rstest::rstest]
    fn preview_focus_hides_a_live_pane() {
        // Given a live pane for the selected thread, with the preview focused.
        let pane = pane_with_text();
        let state = selected(Focus::Preview);

        // When drawing a frame.
        let right = pane.as_ref().map(|pane| right_side_with_pane(&state, pane));

        // Then the right side is the preview, not the pane.
        assert!(
            right
                .as_deref()
                .is_some_and(|right| right.contains("Fix the bug") && !right.contains("PANE-TEXT")),
            "right side was {right:?}"
        );
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
}
