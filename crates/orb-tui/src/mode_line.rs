//! The mode line: orb's bottom row, drawn like LazyVim's lualine in
//! tokyonight-moon.
//!
//! On the left, the mode in a block of its colour, an arrow into the
//! selected session's or draft's branch, then its place: `<project>/<title>`
//! for a session, `<project>/<slug>` for a group draft, the project for a
//! draft. Then the latest error in red and the worktree notice. On the
//! right, `N running`, `fetching origin/<base>…` while Start fetches, or
//! `starting session…` with a spinner, how many agents need an approval or
//! an answer, the selected row's place among the listed rows, and the local
//! time in the mode's colour. When the line is too narrow, the right side
//! stays whole and the left side is cut at its end.

use std::borrow::Cow;
use std::time::SystemTime;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use orb_domain::feat::sessions::state::{Sessions, SidebarItem, SidebarRow, ThreadStatus};
use orb_domain::{AppState, Focus};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::picker::WORKTREE;
use crate::sidebar::{
    APPROVAL_ICON, BG_DARK, BLACK, BLUE, BRANCH, CYAN, FAILED_ICON, FG_DARK, FOLDER, GREEN, GREEN1,
    GUTTER, INPUT_ICON, MAGENTA, ORANGE, RED, SPINNER, SPINNER_FRAME, YELLOW,
};

/// Closes a left block into the next (Nerd Font `nf-pl-left_hard_divider`).
const ARROW_RIGHT: &str = "\u{e0b0}";
/// Opens a right block (`nf-pl-right_hard_divider`).
const ARROW_LEFT: &str = "\u{e0b2}";
/// Before the time (`nf-fa-clock_o`).
const CLOCK: &str = "\u{f017}";

/// Draws the mode line into `area`: the mode, the selected row's branch and
/// project, then the latest error in red and the worktree notice on the
/// left; the activity, the approval and input counts, the selected row's
/// position and the time `now` in `tz` on the right. The right side keeps
/// its width, and the left is cut at its end.
pub(crate) fn render(
    state: &AppState,
    now: SystemTime,
    tz: &TimeZone,
    area: Rect,
    buf: &mut Buffer,
) {
    let (mode, colour) = mode(state);
    let right = right(&state.sessions, colour, now, tz);
    let right_width = u16::try_from(right.width())
        .unwrap_or(u16::MAX)
        .min(area.width);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_width)]).areas(area);
    buf.set_style(area, Style::new().bg(BG_DARK));
    left(
        &state.sessions,
        state.worktrees.notice.as_deref(),
        mode,
        colour,
    )
    .render(left_area, buf);
    right.render(right_area, buf);
}

/// The mode's name and colour.
fn mode(state: &AppState) -> (&'static str, Color) {
    let drafting = state.sessions.selected_draft().is_some()
        || state.sessions.selected_group_draft().is_some();
    match (state.focus, drafting) {
        (Focus::Attached, _) => ("ATTACHED", GREEN1),
        (Focus::Sidebar | Focus::Dashboard, true) => ("DRAFT", MAGENTA),
        (Focus::Sidebar | Focus::Dashboard, false) => ("NORMAL", BLUE),
        (Focus::Picker, _) => ("PICKER", YELLOW),
        (Focus::Rename | Focus::Search, _) => ("INSERT", GREEN),
    }
}

/// a · b · c: the mode block, the branch block when there's a branch, then the
/// project, the error and the worktree notice. On a session, the project
/// reads `<project>/<title>` and the branch is its lead agent's, else the
/// one the session recorded; on a group draft, `<project>/<slug>` and the
/// group's branch.
fn left(sessions: &Sessions, notice: Option<&str>, mode: &str, colour: Color) -> Line<'static> {
    let (branch, project): (Option<&str>, Option<String>) = match (
        sessions.selected_session(),
        sessions.selected_group_draft(),
        sessions.selected_draft(),
    ) {
        (Some(session), _, _) => (
            sessions
                .selected_thread()
                .and_then(|thread| thread.branch.as_deref())
                .or(session.branch.as_deref()),
            sessions
                .project(session.project)
                .map(|project| format!("{}/{}", project.title, sessions.title(session))),
        ),
        (None, Some((project, group)), _) => (
            group.branch.as_deref(),
            Some(format!("{}/{}", project.title, group.name)),
        ),
        (None, None, Some((project, draft))) => {
            (draft.branch.as_deref(), Some(project.title.clone()))
        }
        (None, None, None) => (None, None),
    };
    let mut spans = vec![on(format!(" {mode} "), BLACK, colour).bold()];
    match branch {
        Some(branch) => spans.extend([
            on(ARROW_RIGHT, colour, GUTTER),
            on(format!(" {BRANCH} {branch} "), colour, GUTTER),
            on(ARROW_RIGHT, GUTTER, BG_DARK),
        ]),
        None => spans.push(on(ARROW_RIGHT, colour, BG_DARK)),
    }
    spans.push(on(" ", FG_DARK, BG_DARK));
    spans.extend(project.map(|project| on(format!("{FOLDER} {project} "), FG_DARK, BG_DARK)));
    spans.extend(
        sessions
            .error
            .as_ref()
            .map(|error| on(format!("{FAILED_ICON} {error} "), RED, BG_DARK)),
    );
    spans.extend(notice.map(|notice| on(format!("{WORKTREE} {notice} "), FG_DARK, BG_DARK)));
    Line::from(spans)
}

/// x · y · z: the activity and the counts, the position block when the cursor
/// is on a listed row, then the clock block.
fn right(sessions: &Sessions, colour: Color, now: SystemTime, tz: &TimeZone) -> Line<'static> {
    let counts = [
        (APPROVAL_ICON, ThreadStatus::NeedsApproval, YELLOW),
        (INPUT_ICON, ThreadStatus::NeedsInput, CYAN),
    ];
    let mut spans: Vec<Span<'static>> = activity(sessions, now)
        .map(|activity| on(format!("{activity}  "), ORANGE, BG_DARK))
        .into_iter()
        .chain(counts.into_iter().filter_map(|(icon, status, fg)| {
            match sessions.status_count(status) {
                0 => None,
                count => Some(on(format!("{icon} {count}  "), fg, BG_DARK)),
            }
        }))
        .collect();
    match position(sessions) {
        Some((at, shown)) => spans.extend([
            on(ARROW_LEFT, GUTTER, BG_DARK),
            on(format!(" {at}/{shown} "), colour, GUTTER),
            on(ARROW_LEFT, colour, GUTTER),
        ]),
        None => spans.push(on(ARROW_LEFT, colour, BG_DARK)),
    }
    spans.push(on(format!(" {CLOCK} {} ", clock(now, tz)), BLACK, colour).bold());
    Line::from(spans)
}

/// `<spinner> fetching origin/<b>…` while a start fetches, `<spinner> starting
/// session…`, `<spinner> N running`, or nothing.
fn activity(sessions: &Sessions, now: SystemTime) -> Option<String> {
    match (
        sessions.starting,
        &sessions.fetching,
        sessions.working_count(),
    ) {
        (true, Some(fetching), _) => Some(format!("{} fetching {fetching}…", spinner(now))),
        (true, None, _) => Some(format!("{} starting session…", spinner(now))),
        (false, _, 0) => None,
        (false, _, working) => Some(format!("{} {working} running", spinner(now))),
    }
}

/// The cursor's row, 1-based, among the listed drafts and sessions (not the
/// Settled header), and how many are listed.
fn position(sessions: &Sessions) -> Option<(usize, usize)> {
    let items: Vec<SidebarItem> = sessions
        .sidebar()
        .iter()
        .filter(|row| !matches!(row, SidebarRow::ShelfHeader { .. }))
        .map(SidebarRow::item)
        .collect();
    let at = items
        .iter()
        .position(|&item| Some(item) == sessions.cursor)?;
    Some((at + 1, items.len()))
}

/// `now` in `tz` as `HH:MM`, or `--:--` when jiff can't represent it.
fn clock(now: SystemTime, tz: &TimeZone) -> String {
    match Timestamp::try_from(now) {
        Ok(timestamp) => {
            let time = tz.to_datetime(timestamp);
            format!("{:02}:{:02}", time.hour(), time.minute())
        }
        Err(_) => "--:--".to_owned(),
    }
}

/// The spinner's frame: one per [`SPINNER_FRAME`] since the epoch, so it turns
/// with the loop's tick.
fn spinner(now: SystemTime) -> &'static str {
    let frames = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        / SPINNER_FRAME.as_millis();
    SPINNER
        .get(frames as usize % SPINNER.len())
        .copied()
        .unwrap_or_default()
}

/// `text` in `fg` on `bg`.
fn on<T>(text: T, fg: Color, bg: Color) -> Span<'static>
where
    T: Into<Cow<'static, str>>,
{
    Span::styled(text, Style::new().fg(fg).bg(bg))
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate parse failures with `?` and assert on the outcome"
)]
mod tests {
    use orb_domain::feat::harness::HarnessId;
    use std::time::{Duration, SystemTime};

    use crate::test_support::sessions_for;
    use jiff::tz::{self, TimeZone};
    use orb_domain::feat::sessions::state::{
        Draft, DraftWorkspace, Group, GroupDefaults, GroupDraft, GroupId, GroupKind, PaneId,
        PaneLaunch, Project, ProjectId, ProjectKind, SessionId, Sessions, SidebarItem, Thread,
        ThreadId, ThreadStatus,
    };
    use orb_domain::feat::worktrees::state::Worktrees;
    use orb_domain::{AppState, Focus};
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::Rect;
    use ratatui::style::Color;

    use super::render;
    use crate::sidebar::{BLUE, CYAN, FG_DARK, GREEN, GREEN1, MAGENTA, RED, YELLOW};

    /// 2023-11-14 22:13:20 UTC, at spinner frame 0.
    const NOW: u64 = 1_700_000_000;

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
                command: vec![],
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

    fn project(id: i64, title: &str, threads: Vec<Thread>) -> Project {
        Project {
            id: ProjectId(id),
            title: title.to_owned(),
            root: format!("/Users/me/dev/{title}").into(),
            created_at: SystemTime::UNIX_EPOCH,
            removed: false,
            draft: None,
            threads,
            groups: vec![],
            kind: ProjectKind::Normal,
        }
    }

    fn sessions(threads: Vec<Thread>) -> Sessions {
        fill(Sessions {
            projects: vec![project(1, "orb", threads)],
            ..Sessions::default()
        })
    }

    fn with_sessions(sessions: Sessions) -> AppState {
        AppState {
            sessions,
            ..AppState::default()
        }
    }

    /// Thread 1 (Idle) selected, with the given focus.
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

    /// orb's local draft on `main`, selected, with the given focus.
    fn drafted(focus: Focus) -> AppState {
        let mut sessions = Sessions {
            cursor: Some(SidebarItem::Draft(ProjectId(1))),
            ..sessions(vec![])
        };
        if let Some(project) = sessions.projects.first_mut() {
            project.draft = Some(Draft {
                harness: HarnessId::new("claude"),
                workspace: DraftWorkspace::Local,
                branch: Some("main".to_owned()),
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

    /// Draws `state`'s mode line `width` columns wide at `NOW` in `tz`.
    fn draw_in(state: &AppState, width: u16, tz: &TimeZone) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, width, 1));
        render(
            state,
            SystemTime::UNIX_EPOCH + Duration::from_secs(NOW),
            tz,
            buf.area,
            &mut buf,
        );
        buf
    }

    /// Draws `state`'s mode line 120 columns wide at `NOW` in UTC.
    fn draw(state: &AppState) -> Buffer {
        draw_in(state, 120, &TimeZone::UTC)
    }

    fn text(buffer: &Buffer) -> String {
        buffer.content.iter().map(Cell::symbol).collect()
    }

    /// The cell where `needle` starts.
    fn cell_at<'a>(buffer: &'a Buffer, needle: &str) -> Option<&'a Cell> {
        let symbols: Vec<&str> = buffer.content.iter().map(Cell::symbol).collect();
        let at = (0..symbols.len()).find(|&at| {
            symbols
                .get(at..)
                .is_some_and(|rest| rest.concat().starts_with(needle))
        })?;
        buffer.content.get(at)
    }

    #[rstest::rstest]
    fn mode_line_starts_with_the_mode_block() {
        // Given a selected thread in the sidebar.
        let state = selected(Focus::Sidebar);

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then it starts with the NORMAL block.
        let text = text(&buffer);
        assert!(text.starts_with(" NORMAL "), "mode line was '{text}'");
    }

    #[rstest::rstest]
    #[case::normal(Focus::Sidebar, BLUE)]
    #[case::attached(Focus::Attached, GREEN1)]
    #[case::picker(Focus::Picker, YELLOW)]
    #[case::search(Focus::Search, GREEN)]
    #[case::rename(Focus::Rename, GREEN)]
    fn mode_block_takes_the_mode_colour(#[case] focus: Focus, #[case] colour: Color) {
        // Given a selected thread with that focus.
        let state = selected(focus);

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then the mode name's first letter is on the mode's colour.
        assert_eq!(
            buffer.cell((1, 0)).map(|cell| cell.bg),
            Some(colour),
            "mode block background"
        );
    }

    #[rstest::rstest]
    #[case(Focus::Rename)]
    #[case(Focus::Search)]
    fn mode_line_shows_insert_while_typing(#[case] focus: Focus) {
        // Given the user typing in the rename box or the search.
        let state = AppState {
            focus,
            ..AppState::default()
        };

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then the mode is INSERT.
        let text = text(&buffer);
        assert!(text.starts_with(" INSERT "), "mode line was '{text}'");
    }

    #[rstest::rstest]
    #[case::attached(Focus::Attached, " ATTACHED ")]
    #[case::picker(Focus::Picker, " PICKER ")]
    fn mode_line_names_the_focus_mode(#[case] focus: Focus, #[case] block: &str) {
        // Given a selected thread with that focus.
        let state = selected(focus);

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then the mode block names that mode.
        let text = text(&buffer);
        assert!(text.starts_with(block), "mode line was '{text}'");
    }

    #[rstest::rstest]
    #[case(Focus::Sidebar)]
    #[case(Focus::Dashboard)]
    fn mode_line_shows_draft_on_a_draft(#[case] focus: Focus) {
        // Given a selected draft.
        let state = drafted(focus);

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then the mode is DRAFT.
        let text = text(&buffer);
        assert!(text.starts_with(" DRAFT "), "mode line was '{text}'");
        // And it is on magenta.
        assert_eq!(
            buffer.cell((1, 0)).map(|cell| cell.bg),
            Some(MAGENTA),
            "DRAFT block background"
        );
    }

    #[rstest::rstest]
    fn project_shows_the_sessions_title_on_a_session() {
        // Given session 1 selected, its agent titled `Fix the bug`, in orb.
        let state = selected(Focus::Sidebar);

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then the project reads orb/Fix the bug.
        let text = text(&buffer);
        assert!(
            text.contains("\u{f07b} orb/Fix the bug"),
            "mode line was '{text}'"
        );
    }

    #[rstest::rstest]
    fn branch_block_shows_the_sessions_branch() {
        // Given a selected session that recorded branch orb/y, its agent on
        // no branch.
        let mut state = selected(Focus::Sidebar);
        for session in &mut state.sessions.sessions {
            session.branch = Some("orb/y".to_owned());
        }

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then the branch block names orb/y.
        let text = text(&buffer);
        assert!(text.contains("\u{e0a0} orb/y"), "mode line was '{text}'");
    }

    #[rstest::rstest]
    fn branch_block_shows_the_lead_agents_branch() {
        // Given a selected session whose agent is on branch orb/x.
        let state = with_sessions(Sessions {
            cursor: Some(SidebarItem::Session(SessionId(1))),
            ..sessions(vec![Thread {
                branch: Some("orb/x".to_owned()),
                ..thread(1, ThreadStatus::Idle)
            }])
        });

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then the branch block names orb/x.
        let text = text(&buffer);
        assert!(text.contains("\u{e0a0} orb/x"), "mode line was '{text}'");
    }

    #[rstest::rstest]
    fn branch_block_shows_the_drafts_branch() {
        // Given a selected draft on main.
        let state = drafted(Focus::Sidebar);

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then the branch block names main.
        let text = text(&buffer);
        assert!(text.contains("\u{e0a0} main"), "mode line was '{text}'");
    }

    #[rstest::rstest]
    fn no_branch_block_without_a_branch() {
        // Given a selected thread with no branch.
        let state = selected(Focus::Sidebar);

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then there is no branch icon.
        let text = text(&buffer);
        assert!(!text.contains('\u{e0a0}'), "mode line was '{text}'");
    }

    #[rstest::rstest]
    fn project_follows_the_branch_in_fg_dark() {
        // Given a selected thread in project orb.
        let state = selected(Focus::Sidebar);

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then the project shows in fg_dark.
        assert_eq!(
            cell_at(&buffer, "\u{f07b} orb").map(|cell| cell.fg),
            Some(FG_DARK),
            "project foreground in '{}'",
            text(&buffer)
        );
    }

    #[rstest::rstest]
    fn nothing_selected_shows_no_branch_project_or_position() {
        // Given a thread on a branch, but no cursor.
        let state = with_sessions(sessions(vec![Thread {
            branch: Some("orb/x".to_owned()),
            ..thread(1, ThreadStatus::Idle)
        }]));

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then there is no branch, project or position.
        let text = text(&buffer);
        assert!(
            !text.contains('\u{e0a0}') && !text.contains('\u{f07b}') && !text.contains('/'),
            "mode line was '{text}'"
        );
    }

    #[rstest::rstest]
    fn error_shows_in_red_on_the_left() {
        // Given a claude failure.
        let state = with_sessions(Sessions {
            error: Some("Workspace not trusted".to_owned()),
            ..Sessions::default()
        });

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then the error shows in red.
        assert_eq!(
            cell_at(&buffer, "Workspace not trusted").map(|cell| cell.fg),
            Some(RED),
            "error foreground in '{}'",
            text(&buffer)
        );
    }

    #[rstest::rstest]
    fn notice_shows_on_the_mode_line() {
        // Given a sweep that pruned two worktrees.
        let state = AppState {
            worktrees: Worktrees {
                notice: Some("pruned 2 worktrees".to_owned()),
                ..Worktrees::default()
            },
            ..AppState::default()
        };

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then the notice shows in the dim foreground.
        assert_eq!(
            cell_at(&buffer, "pruned 2 worktrees").map(|cell| cell.fg),
            Some(FG_DARK),
            "notice foreground in '{}'",
            text(&buffer)
        );
    }

    #[rstest::rstest]
    fn activity_counts_working_threads() {
        // Given three working threads.
        let state = with_sessions(sessions(vec![
            thread(1, ThreadStatus::Working),
            thread(2, ThreadStatus::Working),
            thread(3, ThreadStatus::Working),
        ]));

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then it counts three running.
        let text = text(&buffer);
        assert!(text.contains("3 running"), "mode line was '{text}'");
    }

    #[rstest::rstest]
    fn activity_shows_starting_session() {
        // Given a session being started.
        let state = with_sessions(Sessions {
            starting: true,
            ..sessions(vec![])
        });

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then it says a session is starting.
        let text = text(&buffer);
        assert!(text.contains("starting session…"), "mode line was '{text}'");
    }

    #[rstest::rstest]
    fn activity_shows_the_fetch_in_progress() {
        // Given a session being started while origin/main is fetched.
        let state = with_sessions(Sessions {
            starting: true,
            fetching: Some("origin/main".to_owned()),
            ..sessions(vec![])
        });

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then it says origin/main is being fetched.
        let text = text(&buffer);
        assert!(
            text.contains("fetching origin/main…"),
            "mode line was '{text}'"
        );
    }

    #[rstest::rstest]
    fn activity_ignores_a_fetch_without_a_start() {
        // Given a fetch left set with no session being started and no thread working.
        let state = with_sessions(Sessions {
            fetching: Some("origin/main".to_owned()),
            ..sessions(vec![])
        });

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then it says nothing about fetching.
        let text = text(&buffer);
        assert!(!text.contains("fetching"), "mode line was '{text}'");
    }

    #[rstest::rstest]
    fn starting_spinner_turns_a_frame_later() {
        // Given a session being started and no thread working.
        let state = with_sessions(Sessions {
            starting: true,
            ..sessions(vec![])
        });

        // When drawing the mode line one spinner frame after `NOW`.
        let buffer = {
            let mut buf = Buffer::empty(Rect::new(0, 0, 120, 1));
            let now = SystemTime::UNIX_EPOCH + Duration::from_secs(NOW) + super::SPINNER_FRAME;
            render(&state, now, &TimeZone::UTC, buf.area, &mut buf);
            buf
        };

        // Then the spinner beside it shows its second frame.
        let text = text(&buffer);
        assert!(
            text.contains("⠙ starting session…"),
            "mode line was '{text}'"
        );
    }

    #[rstest::rstest]
    fn no_activity_when_idle() {
        // Given only idle threads.
        let state = with_sessions(sessions(vec![
            thread(1, ThreadStatus::Idle),
            thread(2, ThreadStatus::Idle),
        ]));

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then there is no activity.
        let text = text(&buffer);
        assert!(
            !text.contains("running") && !text.contains("starting"),
            "mode line was '{text}'"
        );
    }

    #[rstest::rstest]
    fn approval_count_follows_the_activity() {
        // Given one working thread and one waiting for an approval.
        let state = with_sessions(sessions(vec![
            thread(1, ThreadStatus::Working),
            thread(2, ThreadStatus::NeedsApproval),
        ]));

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then the approval count comes after the activity.
        let text = text(&buffer);
        assert!(
            text.find("1 running")
                .zip(text.find("\u{f071} 1"))
                .is_some_and(|(running, approvals)| running < approvals),
            "mode line was '{text}'"
        );
        // And it is yellow.
        assert_eq!(
            cell_at(&buffer, "\u{f071} 1").map(|cell| cell.fg),
            Some(YELLOW),
            "approval count foreground"
        );
    }

    #[rstest::rstest]
    fn input_count_shows_on_the_right() {
        // Given one thread waiting for an answer.
        let state = with_sessions(sessions(vec![thread(1, ThreadStatus::NeedsInput)]));

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then the input count shows in cyan.
        assert_eq!(
            cell_at(&buffer, "\u{f059} 1").map(|cell| cell.fg),
            Some(CYAN),
            "input count foreground in '{}'",
            text(&buffer)
        );
    }

    #[rstest::rstest]
    fn counts_include_threads_the_filter_hides() {
        // Given a working thread in jinn while the sidebar shows only orb.
        let state = with_sessions(fill(Sessions {
            projects: vec![
                project(1, "orb", vec![thread(1, ThreadStatus::Idle)]),
                project(2, "jinn", vec![thread(2, ThreadStatus::Working)]),
            ],
            filter: Some(ProjectId(1)),
            ..Sessions::default()
        }));

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then the hidden thread still counts.
        let text = text(&buffer);
        assert!(text.contains("1 running"), "mode line was '{text}'");
    }

    #[rstest::rstest]
    fn position_shows_the_cursors_row_of_the_shown_rows() {
        // Given seven threads with the cursor on the third row (thread 5).
        let state = with_sessions(Sessions {
            cursor: Some(SidebarItem::Session(SessionId(5))),
            ..sessions((1..=7).map(|id| thread(id, ThreadStatus::Idle)).collect())
        });

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then the position reads 3/7.
        let text = text(&buffer);
        assert!(text.contains(" 3/7 "), "mode line was '{text}'");
    }

    /// Feature group 9, `GT-514-login` on its branch, with no draft.
    fn feature_group() -> Group {
        Group {
            id: GroupId(9),
            kind: GroupKind::Feature,
            name: "GT-514-login".to_owned(),
            dir: None,
            branch: Some("GT-514-login".to_owned()),
            created_at: SystemTime::UNIX_EPOCH,
            pinned_at: None,
            settled_at: None,
            active_since: SystemTime::UNIX_EPOCH,
            draft: None,
            defaults: GroupDefaults {
                harness: HarnessId::new("claude"),
                model: None,
                permission: None,
            },
        }
    }

    /// Group 9 holding thread 1 and a draft: Feature `GT-514-login` in `orb`,
    /// or Research `tokio-cancel` in `Research`; the cursor on `cursor`.
    fn in_group(kind: GroupKind, cursor: SidebarItem) -> AppState {
        let (title, group) = match kind {
            GroupKind::Feature => ("orb", feature_group()),
            GroupKind::Research | GroupKind::Learn => (
                "Research",
                Group {
                    kind,
                    name: "tokio-cancel".to_owned(),
                    branch: None,
                    ..feature_group()
                },
            ),
        };
        let child = Thread {
            group: Some(GroupId(9)),
            ..thread(1, ThreadStatus::Idle)
        };
        with_sessions(fill(Sessions {
            projects: vec![Project {
                groups: vec![Group {
                    draft: Some(GroupDraft::default()),
                    defaults: GroupDefaults {
                        harness: HarnessId::new("claude"),
                        model: None,
                        permission: None,
                    },
                    ..group
                }],
                ..project(1, title, vec![child])
            }],
            cursor: Some(cursor),
            ..Sessions::default()
        }))
    }

    #[rstest::rstest]
    fn project_shows_the_group_path_on_a_group_draft() {
        // Given the cursor on Feature group GT-514-login's draft in orb.
        let state = in_group(GroupKind::Feature, SidebarItem::GroupDraft(GroupId(9)));

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then the project reads orb/GT-514-login.
        let text = text(&buffer);
        assert!(
            text.contains("\u{f07b} orb/GT-514-login"),
            "mode line was '{text}'"
        );
    }

    #[rstest::rstest]
    fn branch_block_shows_a_feature_groups_branch() {
        // Given the cursor on Feature group GT-514-login's draft.
        let state = in_group(GroupKind::Feature, SidebarItem::GroupDraft(GroupId(9)));

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then the branch block names the group's branch.
        let text = text(&buffer);
        assert!(
            text.contains("\u{e0a0} GT-514-login"),
            "mode line was '{text}'"
        );
    }

    #[rstest::rstest]
    fn research_group_shows_no_branch_block() {
        // Given the cursor on Research group tokio-cancel's draft.
        let state = in_group(GroupKind::Research, SidebarItem::GroupDraft(GroupId(9)));

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then the project reads Research/tokio-cancel with no branch block.
        let text = text(&buffer);
        assert!(
            text.contains("Research/tokio-cancel") && !text.contains('\u{e0a0}'),
            "mode line was '{text}'"
        );
    }

    #[rstest::rstest]
    fn mode_line_shows_draft_on_a_group_draft() {
        // Given the cursor on a group's draft.
        let state = in_group(GroupKind::Feature, SidebarItem::GroupDraft(GroupId(9)));

        // When drawing the mode line.
        let buffer = draw(&state);

        // Then the mode is DRAFT.
        let text = text(&buffer);
        assert!(text.starts_with(" DRAFT "), "mode line was '{text}'");
    }

    #[rstest::rstest]
    fn clock_shows_local_time() {
        // Given a zone at +07:00, where NOW is 05:13.
        let zone = TimeZone::fixed(tz::offset(7));

        // When drawing the mode line in that zone.
        let buffer = draw_in(&AppState::default(), 120, &zone);

        // Then the clock shows 05:13.
        let text = text(&buffer);
        assert!(text.contains("05:13"), "mode line was '{text}'");
    }

    #[rstest::rstest]
    #[case::before_the_switch("2026-03-08T06:30:00Z", "01:30")]
    #[case::after_the_switch("2026-03-08T07:30:00Z", "03:30")]
    fn clock_follows_a_dst_switch(
        #[case] instant: &str,
        #[case] expected: &str,
    ) -> Result<(), jiff::Error> {
        // Given US Eastern time, which springs forward at 07:00 UTC on 2026-03-08.
        let zone = TimeZone::posix("EST5EDT,M3.2.0,M11.1.0")?;
        let now = SystemTime::from(instant.parse::<jiff::Timestamp>()?);

        // When drawing the mode line at that instant in that zone.
        let mut buffer = Buffer::empty(Rect::new(0, 0, 120, 1));
        render(&AppState::default(), now, &zone, buffer.area, &mut buffer);

        // Then the clock shows the local time on that side of the switch.
        let text = text(&buffer);
        assert!(text.contains(expected), "mode line was '{text}'");
        Ok(())
    }

    #[rstest::rstest]
    fn narrow_line_keeps_the_right_side_whole() {
        // Given a long claude error.
        let state = with_sessions(Sessions {
            error: Some(
                "Workspace not trusted. Run `claude` in /Users/me/dev/a-long-project once \
                 and accept the trust prompt, then retry."
                    .to_owned(),
            ),
            ..Sessions::default()
        });

        // When drawing the mode line 60 columns wide.
        let buffer = draw_in(&state, 60, &TimeZone::UTC);

        // Then the clock block is whole at the end.
        let text = text(&buffer);
        assert!(text.ends_with(" \u{f017} 22:13 "), "mode line was '{text}'");
        // And the error is cut.
        assert!(!text.contains("retry."), "mode line was '{text}'");
    }

    #[rstest::rstest]
    fn narrow_line_cuts_the_error_before_the_mode_block() {
        // Given a long claude error.
        let state = with_sessions(Sessions {
            error: Some(
                "Workspace not trusted. Run `claude` in /Users/me/dev/a-long-project once \
                 and accept the trust prompt, then retry."
                    .to_owned(),
            ),
            ..Sessions::default()
        });

        // When drawing the mode line 60 columns wide.
        let buffer = draw_in(&state, 60, &TimeZone::UTC);

        // Then the line still starts with the mode block.
        let text = text(&buffer);
        assert!(text.starts_with(" NORMAL "), "mode line was '{text}'");
    }

    #[rstest::rstest]
    fn line_as_wide_as_the_right_side_shows_only_the_right_side() {
        // Given nothing selected, so the right side is the 10-column clock block.
        let state = AppState::default();

        // When drawing the mode line exactly 10 columns wide.
        let buffer = draw_in(&state, 10, &TimeZone::UTC);

        // Then the line is the whole clock block and nothing of the left.
        let text = text(&buffer);
        assert_eq!(text, "\u{e0b2} \u{f017} 22:13 ", "mode line");
    }

    #[rstest::rstest]
    fn line_narrower_than_the_right_side_cuts_the_right_side_at_its_end() {
        // Given nothing selected, so the right side is the 10-column clock block.
        let state = AppState::default();

        // When drawing the mode line 6 columns wide.
        let buffer = draw_in(&state, 6, &TimeZone::UTC);

        // Then the clock block shows from its start and is cut at its end.
        let text = text(&buffer);
        assert_eq!(text, "\u{e0b2} \u{f017} 22", "mode line");
    }

    #[rstest::rstest]
    #[case(0)]
    #[case(1)]
    fn tiny_line_draws_without_panicking(#[case] width: u16) {
        // Given a claude error and a working thread.
        let state = with_sessions(Sessions {
            error: Some("Workspace not trusted".to_owned()),
            ..sessions(vec![thread(1, ThreadStatus::Working)])
        });

        // When drawing the mode line that narrow.
        let buffer = draw_in(&state, width, &TimeZone::UTC);

        // Then it fills exactly that many cells.
        assert_eq!(buffer.content.len(), usize::from(width), "mode line cells");
    }

    #[rstest::rstest]
    fn wide_grapheme_at_the_cut_is_dropped_whole() {
        // Given an error whose wide character would straddle the cut.
        let state = with_sessions(Sessions {
            error: Some("a日".to_owned()),
            ..Sessions::default()
        });

        // When drawing the mode line so the left side ends mid-character.
        let buffer = draw_in(&state, 24, &TimeZone::UTC);

        // Then the wide character is left out and the right side stays whole.
        let text = text(&buffer);
        assert_eq!(
            text, " NORMAL \u{e0b0} \u{f057} a \u{e0b2} \u{f017} 22:13 ",
            "mode line"
        );
    }
}
