//! Draws a frame: the sidebar on the left, the attached session or the
//! selected thread's preview on the right, the mode line at the bottom, and the
//! which-key popup on top while a key sequence is pending.

use std::time::SystemTime;

use orb_domain::feat::sessions::state::{Sessions, Thread, ThreadStatus};
use orb_domain::{AppState, Focus};
use orb_term::Pane;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::Widget;
use ratatui_which_key::WhichKey;

use crate::keymap::Keys;
use crate::sidebar;

/// Splits the screen into `[sidebar, right side, mode line]`.
pub(crate) fn layout(area: Rect) -> [Rect; 3] {
    let [body, mode_line] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
    let [sidebar, right] =
        Layout::horizontal([Constraint::Length(32), Constraint::Fill(1)]).areas(body);
    [sidebar, right, mode_line]
}

/// Draws the whole frame. `pane` is the selected thread's session, if orb has
/// one running; `pane_error` says why it couldn't start.
pub(crate) fn render(
    frame: &mut Frame,
    state: &AppState,
    pane: Option<&Pane>,
    pane_error: Option<&str>,
    keys: &Keys,
    now: SystemTime,
) {
    let [sidebar_area, right, mode_line] = layout(frame.area());
    sidebar::render(&state.sessions, now, sidebar_area, frame.buffer_mut());
    match pane {
        Some(pane) => {
            if let Some(cursor) = pane.render(right, frame.buffer_mut())
                && state.focus == Focus::Attached
            {
                frame.set_cursor_position(cursor);
            }
        }
        None => render_preview(&state.sessions, pane_error, now, right, frame.buffer_mut()),
    }
    render_mode_line(state, mode_line, frame.buffer_mut());
    // ratatui-which-key divides by the height inside the popup's borders.
    if frame.area().height > 2 {
        WhichKey::new().render(frame.buffer_mut(), keys);
    }
}

/// The selected thread's header and how to attach, or how to start a session
/// when nothing is selected.
fn render_preview(
    sessions: &Sessions,
    pane_error: Option<&str>,
    now: SystemTime,
    area: Rect,
    buf: &mut Buffer,
) {
    let Some(thread) = sessions.selected_thread() else {
        Line::raw("␣n new session").render(area, buf);
        return;
    };
    let [header, error, hint] = Layout::vertical([Constraint::Length(1); 3]).areas(area);
    Line::raw(format!(
        "{} · {} · {}",
        thread.title.as_deref().unwrap_or("New thread"),
        status_label(thread, now),
        thread.cwd.display(),
    ))
    .render(header, buf);
    if let Some(pane_error) = pane_error {
        Line::raw(pane_error).render(error, buf);
    }
    Line::raw("⏎ attach").render(hint, buf);
}

fn status_label(thread: &Thread, now: SystemTime) -> String {
    match thread.status {
        ThreadStatus::Working => format!("working {}", sidebar::elapsed_label(thread, now)),
        ThreadStatus::NeedsApproval => "needs approval".to_owned(),
        ThreadStatus::NeedsInput => "needs input".to_owned(),
        ThreadStatus::Idle => "idle".to_owned(),
        ThreadStatus::Failed => "failed".to_owned(),
        ThreadStatus::Stopped => "stopped".to_owned(),
        ThreadStatus::Gone => "gone".to_owned(),
        ThreadStatus::Unknown => "…".to_owned(),
    }
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
    use std::time::SystemTime;

    use orb_domain::feat::sessions::state::{
        Project, ProjectId, Sessions, Thread, ThreadId, ThreadStatus,
    };
    use orb_domain::{AppState, Focus};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::Rect;

    use super::{layout, render};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::keymap::{Keys, keymap, press};

    fn thread(id: i64, status: ThreadStatus) -> Thread {
        Thread {
            id: ThreadId(id),
            title: Some("Fix the bug".to_owned()),
            cwd: "/Users/me/dev/orb".into(),
            transcript: None,
            status,
            turn_started_at: None,
            attach_argv: vec![],
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
        let Ok(mut terminal) = Terminal::new(TestBackend::new(80, 8));
        let keys = Keys::new(keymap(), Focus::Sidebar);
        let Ok(_) = terminal.draw(|frame| {
            render(frame, state, None, None, &keys, SystemTime::UNIX_EPOCH);
        });
        terminal.backend().buffer().clone()
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
        let [_, _, mode_line] = layout(buffer.area);
        text(buffer, mode_line)
    }

    #[rstest::rstest]
    fn detached_selected_thread_shows_how_to_attach() {
        // Given a selected thread and no pane.
        let state = AppState {
            sessions: Sessions {
                selected: Some(ThreadId(1)),
                ..sessions(vec![thread(1, ThreadStatus::Idle)])
            },
            ..AppState::default()
        };

        // When drawing a frame.
        let buffer = draw(&state);

        // Then the right side offers to attach.
        let [_, right, _] = layout(buffer.area);
        let right = text(&buffer, right);
        assert!(right.contains("⏎ attach"), "right side was '{right}'");
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
            render(frame, &state, None, None, &keys, SystemTime::UNIX_EPOCH);
        });

        // Then the frame is drawn without the popup.
        let mode_line = mode_line(terminal.backend().buffer());
        assert!(
            mode_line.starts_with("NORMAL"),
            "mode line was '{mode_line}'"
        );
    }
}
