//! The sidebar: orb's threads grouped by project, each with its status icon,
//! title, and a short label (elapsed time while working, what it's waiting
//! for, or that it's gone). The selected thread is highlighted.

use std::iter;
use std::time::{Duration, SystemTime};

use orb_domain::feat::sessions::state::{Project, Sessions, Thread, ThreadStatus};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Widget};

/// One sidebar row.
enum Row<'a> {
    Project(&'a Project),
    Thread(&'a Thread),
}

/// Draws the sidebar into `area`, with a border on its right edge.
pub(crate) fn render(sessions: &Sessions, now: SystemTime, area: Rect, buf: &mut Buffer) {
    let block = Block::new().borders(Borders::RIGHT);
    let inner = block.inner(area);
    block.render(area, buf);
    let rows = sessions.projects.iter().flat_map(|project| {
        iter::once(Row::Project(project)).chain(project.threads.iter().map(Row::Thread))
    });
    // ponytail: no scrolling, rows past the bottom are cut; scroll to the
    // selection once thread lists outgrow the screen (M4's sections).
    for (row, y) in rows.zip(inner.top()..inner.bottom()) {
        let area = Rect {
            y,
            height: 1,
            ..inner
        };
        match row {
            Row::Project(project) => render_project(project, area, buf),
            Row::Thread(thread) => {
                render_thread(thread, now, area, buf);
                if sessions.selected_id() == Some(thread.id) {
                    buf.set_style(area, Style::new().add_modifier(Modifier::REVERSED));
                }
            }
        }
    }
}

/// The project's title, then its root on the right.
fn render_project(project: &Project, area: Rect, buf: &mut Buffer) {
    let title = Line::raw(project.title.as_str());
    let [title_area, root_area] = Layout::horizontal([
        Constraint::Length(width(&title).saturating_add(1)),
        Constraint::Fill(1),
    ])
    .areas(area);
    title.render(title_area, buf);
    Line::raw(project.root.to_string_lossy())
        .right_aligned()
        .render(root_area, buf);
}

/// An indented `icon title … label` row.
fn render_thread(thread: &Thread, now: SystemTime, area: Rect, buf: &mut Buffer) {
    let label = Line::raw(label(thread, now)).right_aligned();
    let [_, icon_area, title_area, label_area] = Layout::horizontal([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(width(&label).saturating_add(1)),
    ])
    .areas(area);
    Line::raw(icon(thread.status)).render(icon_area, buf);
    Line::raw(thread.title.as_deref().unwrap_or("New thread")).render(title_area, buf);
    label.render(label_area, buf);
}

fn icon(status: ThreadStatus) -> &'static str {
    match status {
        ThreadStatus::Working => "●",
        ThreadStatus::NeedsApproval => "◐",
        ThreadStatus::NeedsInput => "?",
        ThreadStatus::Failed | ThreadStatus::Gone => "✗",
        ThreadStatus::Stopped => "■",
        ThreadStatus::Idle | ThreadStatus::Unknown => "",
    }
}

fn label(thread: &Thread, now: SystemTime) -> String {
    match thread.status {
        ThreadStatus::Working => elapsed_label(thread, now),
        ThreadStatus::NeedsApproval => "approve".to_owned(),
        ThreadStatus::NeedsInput => "input".to_owned(),
        ThreadStatus::Gone => "gone".to_owned(),
        ThreadStatus::Idle
        | ThreadStatus::Failed
        | ThreadStatus::Stopped
        | ThreadStatus::Unknown => String::new(),
    }
}

/// How long the thread's turn has been running at `now`; 0 without a stamp
/// or when the clock went backwards.
fn elapsed_label(thread: &Thread, now: SystemTime) -> String {
    let elapsed = thread
        .turn_started_at
        .and_then(|started| now.duration_since(started).ok())
        .unwrap_or_default();
    format_elapsed(elapsed)
}

/// `45s`, `2m14s`, or `1h5m`.
pub(crate) fn format_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m{}s", secs / 60, secs % 60),
        _ => format!("{}h{}m", secs / 3600, secs / 60 % 60),
    }
}

fn width(line: &Line<'_>) -> u16 {
    u16::try_from(line.width()).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use orb_domain::feat::sessions::state::{
        Project, ProjectId, Sessions, Thread, ThreadId, ThreadStatus,
    };
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::Rect;

    use super::{format_elapsed, render};

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn thread(status: ThreadStatus, title: Option<&str>) -> Thread {
        Thread {
            id: ThreadId(1),
            title: title.map(str::to_owned),
            cwd: "/Users/me/dev/orb".into(),
            transcript: None,
            status,
            turn_started_at: Some(at(866)),
            attach_argv: vec![],
            branch: None,
            pinned_at: None,
            settled_at: None,
            active_since: SystemTime::UNIX_EPOCH,
            last_activity_at: SystemTime::UNIX_EPOCH,
            unseen: false,
        }
    }

    fn sessions(thread: Thread) -> Sessions {
        Sessions {
            projects: vec![Project {
                id: ProjectId(1),
                title: "orb".to_owned(),
                root: "/Users/me/dev/orb".into(),
                threads: vec![thread],
            }],
            ..Sessions::default()
        }
    }

    /// The thread's row (the one under its project header) at `now`.
    fn thread_row(sessions: &Sessions, now: SystemTime) -> String {
        let mut buf = Buffer::empty(Rect::new(0, 0, 32, 3));
        render(sessions, now, buf.area, &mut buf);
        (0..buf.area.width)
            .filter_map(|x| buf.cell((x, 1)).map(Cell::symbol))
            .collect()
    }

    #[rstest::rstest]
    #[case(0, "0s")]
    #[case(45, "45s")]
    #[case(134, "2m14s")]
    #[case(3900, "1h5m")]
    fn format_elapsed_is_compact(#[case] secs: u64, #[case] expected: &str) {
        // Given / When formatting the elapsed seconds.
        let formatted = format_elapsed(Duration::from_secs(secs));

        // Then it uses the largest two units.
        assert_eq!(formatted, expected, "{secs} s");
    }

    #[rstest::rstest]
    fn working_thread_shows_its_elapsed_time() {
        // Given a Working thread whose turn started 134 s before now.
        let sessions = sessions(thread(ThreadStatus::Working, Some("Fix the bug")));

        // When rendering the sidebar.
        let row = thread_row(&sessions, at(1000));

        // Then its row shows the working icon and the elapsed time.
        assert!(
            row.contains('●') && row.contains("2m14s"),
            "row was '{row}'"
        );
    }

    #[rstest::rstest]
    fn thread_needing_approval_says_approve() {
        // Given a thread waiting on a permission prompt.
        let sessions = sessions(thread(ThreadStatus::NeedsApproval, Some("Fix the bug")));

        // When rendering the sidebar.
        let row = thread_row(&sessions, at(1000));

        // Then its row shows the approval icon and label.
        assert!(
            row.contains('◐') && row.contains("approve"),
            "row was '{row}'"
        );
    }

    #[rstest::rstest]
    fn untitled_thread_is_a_new_thread() {
        // Given a thread without a title yet.
        let sessions = sessions(thread(ThreadStatus::Idle, None));

        // When rendering the sidebar.
        let row = thread_row(&sessions, at(1000));

        // Then its row says "New thread".
        assert!(row.contains("New thread"), "row was '{row}'");
    }
}
