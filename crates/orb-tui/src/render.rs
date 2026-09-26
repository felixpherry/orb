//! Draws a frame: the sidebar on the left, the attached session, the selected
//! thread's preview or the selected draft's form on the right, the mode line at
//! the bottom, and the which-key popup on top while a key sequence is pending.
//! While the sidebar is hidden, the right side takes the full width.
//! While `s` or `x` waits for its repeat, a banner above the selected row says
//! what the repeat will do instead of the popup. An open picker is drawn over
//! everything but the mode line, with neither the popup nor the banner.
//! Whatever is left on the terminal's default background gets orb's navy, so
//! a transparent terminal doesn't show through.

use std::time::SystemTime;

use orb_domain::feat::preview::state::PreviewLayout;
use orb_domain::feat::sessions::state::SidebarItem;
use orb_domain::feat::sessions::validator::{ToggleSettleError, validate_toggle_settle};
use orb_domain::feat::sidebar::state::{SidebarLayout, SidebarView};
use orb_domain::{AppState, Focus};
use orb_term::Pane;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::Widget;
use ratatui_which_key::WhichKey;

use crate::draft;
use crate::keymap::{self, Keys};
use crate::picker::{self, PickerScroll};
use crate::preview::{self, PreviewCache};
use crate::sidebar::{self, SidebarScroll};

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

/// Draws the whole frame. `pane` is the selected thread's session, if orb has
/// one running; it's drawn only while attached, and otherwise the right side
/// shows the selected thread's preview, with `pane_error` saying why the
/// session couldn't start. Returns the preview's layout when it was drawn,
/// the sidebar's layout unless it's hidden, and how many rows the picker fits
/// when it's open.
#[expect(
    clippy::too_many_arguments,
    reason = "the frame's inputs and the three frontend view states it updates"
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
    picker_scroll: &mut PickerScroll,
) -> (Option<PreviewLayout>, Option<SidebarLayout>, Option<usize>) {
    let [sidebar_area, right, mode_line] = layout(frame.area(), &state.sidebar);
    let (selected_y, sidebar_layout) = if state.sidebar.hidden {
        (None, None)
    } else {
        let (selected_y, sidebar_layout) = sidebar::render(
            &state.sessions,
            now,
            sidebar_area,
            frame.buffer_mut(),
            scroll,
        );
        (selected_y, Some(sidebar_layout))
    };
    let preview_layout = match (pane, state.focus) {
        (Some(pane), Focus::Attached) => {
            if let Some(cursor) = pane.render(right, frame.buffer_mut()) {
                frame.set_cursor_position(cursor);
            }
            None
        }
        _ => match (
            state.sessions.selected_thread(),
            state.sessions.selected_draft(),
        ) {
            (Some(thread), _) => Some(preview::render(
                state,
                thread,
                pane_error,
                right,
                frame.buffer_mut(),
                cache,
            )),
            (None, Some((project, draft))) => {
                draft::render(project, draft, &state.home, right, frame.buffer_mut());
                None
            }
            (None, None) => {
                let hint = match state.sessions.cursor {
                    Some(SidebarItem::SettledShelf) => shelf_hint(state),
                    _ => "␣n new session · ␣p add project".to_owned(),
                };
                Line::raw(hint).render(right, frame.buffer_mut());
                None
            }
        },
    };
    render_mode_line(state, mode_line, frame.buffer_mut());
    let picker_page = match (&state.picker, keymap::pending_confirm(keys)) {
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
        (None, Some(confirm)) => {
            if let Some(y) = selected_y {
                render_banner(state, confirm, y, frame.buffer_mut());
            }
            None
        }
        // ratatui-which-key divides by the height inside the popup's borders.
        (None, None) if frame.area().height > 2 => {
            WhichKey::new().render(frame.buffer_mut(), keys);
            None
        }
        (None, None) => None,
    };
    for cell in &mut frame.buffer_mut().content {
        if cell.bg == Color::Reset {
            cell.bg = BACKGROUND;
        }
    }
    (preview_layout, sidebar_layout, picker_page)
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
/// `confirm` will act, red when it would be refused. On a draft only `x`
/// has one.
fn render_banner(state: &AppState, confirm: char, selected_y: u16, buf: &mut Buffer) {
    let sessions = &state.sessions;
    let (text, colour) = match (
        confirm,
        validate_toggle_settle(state),
        sessions.selected_thread(),
    ) {
        ('x', _, None) if sessions.selected_draft().is_some() => {
            (" Press x again to discard ", Color::Yellow)
        }
        (_, _, None) | (_, Err(ToggleSettleError::NoThread), _) => return,
        ('x', _, Some(_)) => (" Press x again to delete ", Color::Yellow),
        (_, Ok(()), Some(thread)) if thread.settled_at.is_some() => {
            (" Press s again to un-settle ", Color::Yellow)
        }
        (_, Ok(()), Some(_)) => (" Press s again to settle ", Color::Yellow),
        (_, Err(ToggleSettleError::InProgress), Some(_)) => {
            (" Can't settle while Claude is working ", Color::Red)
        }
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

/// The mode's name on the left (`DRAFT` while a draft is selected); on the
/// right, a session being started, else the latest `claude` failure, else how
/// many threads are working. A right side too long for the line is cut at its
/// end, two cells after the name.
fn render_mode_line(state: &AppState, area: Rect, buf: &mut Buffer) {
    let mode = match (state.focus, state.sessions.selected_draft()) {
        (Focus::Attached, _) => "ATTACHED",
        (Focus::Sidebar | Focus::Preview, Some(_)) => "DRAFT",
        (Focus::Sidebar | Focus::Preview, None) => "NORMAL",
        (Focus::Picker, _) => "PICKER",
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
    let mode = Line::raw(mode);
    let status_width = {
        let room = usize::from(area.width).saturating_sub(mode.width() + 2);
        u16::try_from(status.width().min(room)).unwrap_or(u16::MAX)
    };
    let [mode_area, status_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(status_width)]).areas(area);
    mode.render(mode_area, buf);
    status.render(status_area, buf);
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::{Duration, Instant, SystemTime};

    use orb_domain::feat::picker::state::PickerState;
    use orb_domain::feat::preview::state::Preview;
    use orb_domain::feat::sessions::state::{
        Draft, DraftWorkspace, Project, ProjectId, Sessions, SidebarItem, Thread, ThreadId,
        ThreadStatus,
    };
    use orb_domain::feat::sidebar::state::SidebarView;
    use orb_domain::{AppState, Focus};
    use orb_term::{Pane, PaneCommand, PaneSize};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::Rect;

    use super::{BACKGROUND, layout, render};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::keymap::{Keys, Scope, keymap, press};
    use crate::picker::PickerScroll;
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
                created_at: SystemTime::UNIX_EPOCH,
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
                &mut PickerScroll::default(),
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
                &mut PreviewCache::default(),
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
        let [_, right, _] = layout(buffer.area, &SidebarView::default());
        text(buffer, right)
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
        let mut keys = Keys::new(keymap(), Scope::Sidebar);
        press(
            &mut keys,
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
        );
        keys
    }

    fn mode_line(buffer: &Buffer) -> String {
        let [_, _, mode_line] = layout(buffer.area, &SidebarView::default());
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
        let [_, right, _] = layout(buffer.area, &SidebarView::default());
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
    fn long_claude_error_is_cut_after_the_mode_name() {
        // Given a `claude` error wider than the 80-column mode line.
        let state = AppState {
            sessions: Sessions {
                error: Some(
                    "Workspace not trusted. Run `claude` in /Users/me/dev/a-long-project \
                     once and accept the trust prompt, then retry."
                        .to_owned(),
                ),
                ..Sessions::default()
            },
            ..AppState::default()
        };

        // When drawing a frame.
        let buffer = draw(&state);

        // Then the mode's name stays whole, and the error starts two cells on.
        let mode_line = mode_line(&buffer);
        assert!(
            mode_line.starts_with("NORMAL  Workspace not trusted"),
            "mode line was '{mode_line}'"
        );
    }

    #[rstest::rstest]
    fn mode_line_shows_normal_outside_a_session() {
        // Given orb in Sidebar focus.
        let state = AppState::default();

        // When drawing a frame.
        let buffer = draw(&state);

        // Then the mode line shows only the mode's name.
        let mode_line = mode_line(&buffer);
        assert_eq!(mode_line.trim_end(), "NORMAL", "the mode line");
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

        // Then the mode line shows only the mode's name.
        let mode_line = mode_line(&buffer);
        assert_eq!(mode_line.trim_end(), "ATTACHED", "the mode line");
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

    #[rstest::rstest]
    fn mode_line_shows_picker_while_the_picker_is_open() {
        // Given an open project picker.
        let state = AppState {
            focus: Focus::Picker,
            picker: Some(PickerState::projects(vec![], Focus::Sidebar)),
            ..AppState::default()
        };

        // When drawing a frame.
        let buffer = draw(&state);

        // Then the mode line shows only the mode's name.
        let mode_line = mode_line(&buffer);
        assert_eq!(mode_line.trim_end(), "PICKER", "the mode line");
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
                &mut PreviewCache::default(),
                &mut SidebarScroll::default(),
                &mut PickerScroll::default(),
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
        let [_, right, _] = layout(buffer.area, &SidebarView::default());
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

    #[rstest::rstest]
    fn selected_draft_draws_its_form() {
        // Given orb's draft selected.
        let state = drafted(Focus::Sidebar);

        // When drawing a frame.
        let right = right_side(&draw(&state));

        // Then the right side is the draft's form.
        assert!(
            right.starts_with("New thread · OB orb") && right.contains("⏎ start"),
            "right side was '{right}'"
        );
    }

    #[rstest::rstest]
    fn draft_cursor_before_its_draft_exists_shows_the_hint() {
        // Given the cursor on orb's draft before the actor made it.
        let state = AppState {
            sessions: Sessions {
                cursor: Some(SidebarItem::Draft(ProjectId(1))),
                ..sessions(vec![])
            },
            ..AppState::default()
        };

        // When drawing a frame.
        let right = right_side(&draw(&state));

        // Then the right side shows the no-selection hint.
        assert!(
            right.starts_with("␣n new session · ␣p add project"),
            "right side was '{right}'"
        );
    }

    #[rstest::rstest]
    #[case(Focus::Sidebar)]
    #[case(Focus::Preview)]
    fn mode_line_shows_draft_on_a_draft(#[case] focus: Focus) {
        // Given orb's draft selected.
        let state = drafted(focus);

        // When drawing a frame.
        let buffer = draw(&state);

        // Then the mode line shows only the mode's name.
        let mode_line = mode_line(&buffer);
        assert_eq!(mode_line.trim_end(), "DRAFT", "the mode line on a draft");
    }

    #[rstest::rstest]
    fn pending_x_on_a_draft_shows_the_discard_banner() {
        // Given orb's draft selected and `x` pressed once.
        let state = drafted(Focus::Sidebar);

        // When drawing a frame.
        let buffer = draw_with(&state, &pending('x'));

        // Then the banner asks for the second `x` to discard it.
        let screen = text(&buffer, buffer.area);
        assert!(
            screen.contains(" Press x again to discard "),
            "screen was '{screen}'"
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
    fn hidden_sidebar_draws_the_right_side_from_the_left_edge() {
        // Given orb's draft selected with the sidebar hidden.
        let state = hidden(drafted(Focus::Preview));

        // When drawing a frame.
        let buffer = draw(&state);

        // Then the draft's form starts in the first column.
        let screen = text(&buffer, buffer.area);
        assert!(
            screen.starts_with("New thread · OB orb"),
            "screen was '{screen}'"
        );
    }

    #[rstest::rstest]
    fn hidden_sidebar_returns_no_layout() {
        // Given a selected thread with the sidebar hidden.
        let state = hidden(selected(Focus::Preview));

        // When drawing a frame.
        let Ok(mut terminal) = Terminal::new(TestBackend::new(80, 8));
        let mut sidebar_layout = None;
        let Ok(_) = terminal.draw(|frame| {
            (_, sidebar_layout, _) = render(
                frame,
                &state,
                None,
                None,
                &Keys::new(keymap(), Scope::Preview),
                SystemTime::UNIX_EPOCH,
                &mut PreviewCache::default(),
                &mut SidebarScroll::default(),
                &mut PickerScroll::default(),
            );
        });

        // Then there's no sidebar layout to replace the last drawn one.
        assert_eq!(sidebar_layout, None, "a hidden sidebar isn't laid out");
    }

    #[rstest::rstest]
    fn pending_s_on_a_draft_shows_no_banner() {
        // Given orb's draft selected and `s` pressed once.
        let state = drafted(Focus::Sidebar);

        // When drawing a frame.
        let buffer = draw_with(&state, &pending('s'));

        // Then no banner is drawn.
        let screen = text(&buffer, buffer.area);
        assert!(!screen.contains("Press s again"), "screen was '{screen}'");
    }
}
