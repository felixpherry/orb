//! The sidebar: one list of orb's threads across projects, in T3 Code's style.
//!
//! Pinned threads come first, then active ones, each a three-line card: the
//! project's badge and name with the thread's status (or the time since its
//! last turn), the title, and the branch. Settled threads fold into a shelf at
//! the bottom, drawn as one-line rows while it's open. The sidebar scrolls to
//! keep the selection in view.

use std::time::{Duration, SystemTime};

use orb_domain::feat::sessions::state::{Project, Sessions, SidebarRow, Thread, ThreadStatus};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};
use unicode_segmentation::UnicodeSegmentation;

/// How far the sidebar is scrolled, kept between frames.
#[derive(Debug, Default)]
pub(crate) struct SidebarScroll {
    /// The list line drawn on the sidebar's top row.
    offset: u16,
}

/// Draws the sidebar into `area`, with a border on its right edge, scrolled
/// so the cursor's row is in view. Returns the y of the selected row's first
/// line when it's on screen.
pub(crate) fn render(
    sessions: &Sessions,
    now: SystemTime,
    area: Rect,
    buf: &mut Buffer,
    scroll: &mut SidebarScroll,
) -> Option<u16> {
    let block = Block::new().borders(Borders::RIGHT);
    let inner = block.inner(area);
    block.render(area, buf);
    let rows = sessions.sidebar();
    // Each row with its top line in the list. Blank lines above the shelf
    // header keep the shelf at the bottom while the list is short.
    let (placed, total) = {
        let content = rows.iter().map(height).fold(0, u16::saturating_add);
        let gap = inner.height.saturating_sub(content);
        let mut top = 0_u16;
        let placed: Vec<(SidebarRow<'_>, u16)> = rows
            .into_iter()
            .map(|row| {
                if matches!(row, SidebarRow::ShelfHeader { .. }) {
                    top = top.saturating_add(gap);
                }
                let row_top = top;
                top = top.saturating_add(height(&row));
                (row, row_top)
            })
            .collect();
        (placed, top)
    };
    let selected = placed
        .iter()
        .find(|(row, _)| Some(row.item()) == sessions.cursor)
        .map(|(row, top)| (*top, height(row)));
    if let Some((top, rows)) = selected {
        scroll.offset = scroll
            .offset
            .min(top)
            .max(top.saturating_add(rows).saturating_sub(inner.height));
    }
    scroll.offset = scroll.offset.min(total.saturating_sub(inner.height));
    // The whole list, then the lines in view.
    let list = {
        let mut list = Buffer::empty(Rect::new(inner.x, 0, inner.width, total));
        for (row, top) in &placed {
            let row_area = Rect::new(inner.x, *top, inner.width, height(row));
            let is_selected = Some(row.item()) == sessions.cursor;
            render_row(row, is_selected, now, row_area, &mut list);
        }
        list
    };
    for (y, line) in (inner.top()..inner.bottom()).zip(scroll.offset..total) {
        for x in inner.left()..inner.right() {
            if let (Some(cell), Some(shown)) = (list.cell((x, line)), buf.cell_mut((x, y))) {
                *shown = cell.clone();
            }
        }
    }
    selected
        .map(|(top, _)| top)
        .filter(|top| (scroll.offset..scroll.offset.saturating_add(inner.height)).contains(top))
        .map(|top| inner.y + top - scroll.offset)
}

/// How many lines a row takes: 3 for a card, 1 otherwise.
fn height(row: &SidebarRow<'_>) -> u16 {
    match row {
        SidebarRow::Card { .. } => 3,
        SidebarRow::ShelfHeader { .. } | SidebarRow::Settled { .. } => 1,
    }
}

/// One row, with the selection background behind it and a cell of padding on
/// each side.
fn render_row(row: &SidebarRow<'_>, selected: bool, now: SystemTime, area: Rect, buf: &mut Buffer) {
    if selected {
        buf.set_style(area, Style::new().bg(SELECTED));
    }
    let area = area.inner(Margin::new(1, 0));
    match row {
        SidebarRow::Card { project, thread } => render_card(project, thread, now, area, buf),
        SidebarRow::ShelfHeader { count, open } => {
            Line::styled(shelf_label(*count, *open), Style::new().fg(GRAY)).render(area, buf);
        }
        SidebarRow::Settled { project, thread } => {
            render_settled(project, thread, selected, now, area, buf);
        }
    }
}

/// The badge, project, pin marker and status; the title; the branch and ✳.
fn render_card(project: &Project, thread: &Thread, now: SystemTime, area: Rect, buf: &mut Buffer) {
    let [heading, title, footer] = Layout::vertical([Constraint::Length(1); 3]).areas(area);
    let pin = if thread.pinned_at.is_some() {
        " ⚑"
    } else {
        ""
    };
    let project_line = Line::from(vec![
        badge(&project.title, true),
        Span::raw(" "),
        Span::styled(format!("{}{pin}", project.title), Style::new().fg(GRAY)),
    ]);
    render_split(project_line, Line::from(status(thread, now)), heading, buf);
    Line::raw(thread.title.as_deref().unwrap_or("New thread")).render(title, buf);
    render_split(
        Line::styled(
            thread.branch.as_deref().unwrap_or_default(),
            Style::new().fg(DARK_GRAY),
        ),
        Line::styled("✳", Style::new().fg(CLAUDE)),
        footer,
        buf,
    );
}

/// A settled thread's row: its badge (lit only while selected), title, and
/// the time since it settled, after a `✗` if it failed or is gone.
fn render_settled(
    project: &Project,
    thread: &Thread,
    selected: bool,
    now: SystemTime,
    area: Rect,
    buf: &mut Buffer,
) {
    let title = Line::from(vec![
        badge(&project.title, selected),
        Span::raw(" "),
        Span::styled(
            thread.title.as_deref().unwrap_or("New thread"),
            Style::new().fg(GRAY),
        ),
    ]);
    let mark = match thread.status {
        ThreadStatus::Failed | ThreadStatus::Gone => "✗ ",
        _ => "",
    };
    let settled = thread
        .settled_at
        .map(|at| ago_label(since(now, at)))
        .unwrap_or_default();
    let when = Line::from(vec![
        Span::styled(mark, Style::new().fg(RED)),
        Span::styled(settled, Style::new().fg(DARK_GRAY)),
    ]);
    render_split(title, when, area, buf);
}

/// The Settled shelf header's text: `▸ Settled (N)` closed, `▾ Settled` open.
pub(crate) fn shelf_label(count: usize, open: bool) -> String {
    if open {
        "▾ Settled".to_owned()
    } else {
        format!("▸ Settled ({count})")
    }
}

/// Draws `left`, and `right` against the right edge with a cell between them.
fn render_split(left: Line<'_>, right: Line<'_>, area: Rect, buf: &mut Buffer) {
    let [left_area, right_area] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(width(&right).saturating_add(1)),
    ])
    .areas(area);
    left.render(left_area, buf);
    right.right_aligned().render(right_area, buf);
}

/// A card's status slot: what the thread is doing or waiting for, else how
/// long ago orb saw its last turn end.
fn status(thread: &Thread, now: SystemTime) -> Span<'static> {
    let (text, colour) = match thread.status {
        ThreadStatus::NeedsApproval => ("◐ Pending Approval".to_owned(), AMBER),
        ThreadStatus::NeedsInput => ("? Awaiting Input".to_owned(), INDIGO),
        ThreadStatus::Working => {
            let elapsed = thread
                .turn_started_at
                .map(|started| since(now, started))
                .unwrap_or_default();
            (format!("● Working {}", working_label(elapsed)), SKY)
        }
        ThreadStatus::Failed => ("✗ Failed".to_owned(), RED),
        ThreadStatus::Gone => ("✗ Gone".to_owned(), RED),
        ThreadStatus::Idle if thread.unseen => ("✓ Completed".to_owned(), EMERALD),
        ThreadStatus::Stopped => ("■ Stopped".to_owned(), GRAY),
        ThreadStatus::Idle | ThreadStatus::Unknown => {
            (ago_label(since(now, thread.last_activity_at)), GRAY)
        }
    };
    Span::styled(text, Style::new().fg(colour))
}

/// How long before `now` `at` was; zero if the clock went backwards.
fn since(now: SystemTime, at: SystemTime) -> Duration {
    now.duration_since(at).unwrap_or_default()
}

/// How long a turn has run, as T3 shows it: `45s`, `2m`, or `1h 5m`.
pub(crate) fn working_label(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        _ => format!("{}h {}m", secs / 3600, secs / 60 % 60),
    }
}

/// How long ago something happened, as T3 shows it: `now`, `5m`, `3h`, or
/// `2d`.
pub(crate) fn ago_label(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    match secs {
        0..60 => "now".to_owned(),
        60..3600 => format!("{}m", secs / 60),
        3600..86_400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

/// T3's two-letter project badge: the first glyph of the first word, then the
/// first digit after it, else the first glyph of the last word, else the first
/// word's last glyph. `PR` when the name has no letters or digits.
pub(crate) fn monogram(name: &str) -> String {
    let words: Vec<&str> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let glyphs: Vec<&str> = words
        .first()
        .map(|word| word.graphemes(true).collect())
        .unwrap_or_default();
    let Some((first, rest)) = glyphs.split_first() else {
        return "PR".to_owned();
    };
    let second = rest
        .iter()
        .copied()
        .find(|glyph| glyph.chars().all(char::is_numeric))
        .or_else(|| match words.as_slice() {
            [_, .., last] => last.graphemes(true).next(),
            _ => glyphs.last().copied(),
        })
        .unwrap_or(first);
    format!("{first}{second}")
        .to_uppercase()
        .graphemes(true)
        .take(2)
        .collect()
}

/// T3's badge colour for a project: an index into [`BADGE`], hashed from the
/// lower-cased name's code points.
pub(crate) fn badge_colour(name: &str) -> usize {
    let lowered = name.trim().to_lowercase();
    let seed = match lowered.as_str() {
        "" => "project",
        seed => seed,
    };
    seed.chars()
        .fold(0, |index, c| (index * 31 + c as usize) % BADGE.len())
}

/// The project's monogram tile, in its colour, or in gray unless `lit`.
fn badge(name: &str, lit: bool) -> Span<'static> {
    let (r, g, b) = if lit {
        BADGE.get(badge_colour(name))
    } else {
        BADGE.first()
    }
    .copied()
    .unwrap_or_default();
    Span::styled(
        monogram(name),
        Style::new()
            .fg(Color::Rgb(r, g, b))
            .bg(Color::Rgb(tint(r), tint(g), tint(b))),
    )
}

/// 14% of a colour channel over black, rounded.
fn tint(channel: u8) -> u8 {
    u8::try_from((u16::from(channel) * 14 + 50) / 100).unwrap_or(u8::MAX)
}

fn width(line: &Line<'_>) -> u16 {
    u16::try_from(line.width()).unwrap_or(u16::MAX)
}

/// Tailwind's 400 shades in T3's badge order: gray, red, orange, amber,
/// yellow, lime, green, emerald, teal, cyan, sky, blue, indigo, violet, purple,
/// fuchsia, pink, rose.
const BADGE: [(u8, u8, u8); 18] = [
    (0x9c, 0xa3, 0xaf),
    (0xf8, 0x71, 0x71),
    (0xfb, 0x92, 0x3c),
    (0xfb, 0xbf, 0x24),
    (0xfa, 0xcc, 0x15),
    (0xa3, 0xe6, 0x35),
    (0x4a, 0xde, 0x80),
    (0x34, 0xd3, 0x99),
    (0x2d, 0xd4, 0xbf),
    (0x22, 0xd3, 0xee),
    (0x38, 0xbd, 0xf8),
    (0x60, 0xa5, 0xfa),
    (0x81, 0x8c, 0xf8),
    (0xa7, 0x8b, 0xfa),
    (0xc0, 0x84, 0xfc),
    (0xe8, 0x79, 0xf9),
    (0xf4, 0x72, 0xb6),
    (0xfb, 0x71, 0x85),
];
/// Pending Approval (amber-300).
const AMBER: Color = Color::Rgb(0xfc, 0xd3, 0x4d);
/// Awaiting Input (indigo-300).
const INDIGO: Color = Color::Rgb(0xa5, 0xb4, 0xfc);
/// Working (sky-300).
const SKY: Color = Color::Rgb(0x7d, 0xd3, 0xfc);
/// Completed (emerald-300).
const EMERALD: Color = Color::Rgb(0x6e, 0xe7, 0xb7);
/// Failed and gone (red-300).
const RED: Color = Color::Rgb(0xfc, 0xa5, 0xa5);
/// The project name, settled titles, the shelf header, and idle times.
const GRAY: Color = Color::Rgb(0xa3, 0xa3, 0xa3);
/// The branch and settled times.
const DARK_GRAY: Color = Color::Rgb(0x73, 0x73, 0x73);
/// The ✳ logo.
const CLAUDE: Color = Color::Rgb(0xd9, 0x77, 0x57);
/// Behind the selected row.
const SELECTED: Color = Color::Rgb(0x26, 0x26, 0x26);

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use orb_domain::feat::sessions::state::{
        Project, ProjectId, Sessions, SidebarItem, Thread, ThreadId, ThreadStatus,
    };
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::Rect;

    use super::{
        SELECTED, SKY, SidebarScroll, ago_label, badge_colour, monogram, render, working_label,
    };

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    /// An idle thread titled "Thread <id>" whose turn started at 866 s.
    fn thread(id: i64, status: ThreadStatus) -> Thread {
        Thread {
            id: ThreadId(id),
            title: Some(format!("Thread {id}")),
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

    fn settled(id: i64, at_secs: u64) -> Thread {
        Thread {
            settled_at: Some(at(at_secs)),
            ..thread(id, ThreadStatus::Stopped)
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

    /// Draws a 32-column sidebar `height` lines tall at `now`.
    fn draw(sessions: &Sessions, now: SystemTime, height: u16) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, 32, height));
        render(
            sessions,
            now,
            buf.area,
            &mut buf,
            &mut SidebarScroll::default(),
        );
        buf
    }

    /// The sidebar's lines, top to bottom.
    fn lines(buf: &Buffer) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .filter_map(|x| buf.cell((x, y)).map(Cell::symbol))
                    .collect()
            })
            .collect()
    }

    fn line(buf: &Buffer, y: usize) -> String {
        lines(buf).swap_remove(y)
    }

    #[rstest::rstest]
    #[case("orb", "OB")]
    #[case("paneru", "PU")]
    #[case("v2 app", "V2")]
    #[case("itemku-frontend-next-v2", "IV")]
    #[case("", "PR")]
    fn monogram_takes_first_and_last_glyphs(#[case] name: &str, #[case] expected: &str) {
        // Given / When deriving the project's monogram.
        let badge = monogram(name);

        // Then it follows T3's rule.
        assert_eq!(badge, expected, "monogram of '{name}'");
    }

    #[rstest::rstest]
    #[case("orb", 17)]
    #[case("paneru", 15)]
    fn badge_colour_matches_t3(#[case] name: &str, #[case] expected: usize) {
        // Given / When hashing the project's name.
        let index = badge_colour(name);

        // Then it picks T3's colour.
        assert_eq!(index, expected, "badge colour of '{name}'");
    }

    #[rstest::rstest]
    #[case(45, "45s")]
    #[case(134, "2m")]
    #[case(3900, "1h 5m")]
    fn working_label_is_t3s_duration(#[case] secs: u64, #[case] expected: &str) {
        // Given / When formatting how long a turn has run.
        let label = working_label(Duration::from_secs(secs));

        // Then it uses T3's format.
        assert_eq!(label, expected, "{secs} s");
    }

    #[rstest::rstest]
    #[case(30, "now")]
    #[case(300, "5m")]
    #[case(10_800, "3h")]
    #[case(172_800, "2d")]
    fn ago_label_is_t3s_relative_time(#[case] secs: u64, #[case] expected: &str) {
        // Given / When formatting how long ago something happened.
        let label = ago_label(Duration::from_secs(secs));

        // Then it uses T3's format.
        assert_eq!(label, expected, "{secs} s ago");
    }

    #[rstest::rstest]
    fn working_card_shows_working_and_duration_in_sky() {
        // Given a Working thread whose turn started 134 s before now.
        let sessions = sessions(vec![thread(1, ThreadStatus::Working)]);

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 3);

        // Then its card's first line says how long it has been working.
        let heading = line(&buf, 0);
        assert!(heading.contains("● Working 2m"), "line was '{heading}'");
        // And the status is sky.
        let dot = (0..32).find_map(|x| buf.cell((x, 0)).filter(|cell| cell.symbol() == "●"));
        assert_eq!(dot.map(|cell| cell.fg), Some(SKY), "the ● colour");
    }

    #[rstest::rstest]
    fn thread_needing_approval_shows_pending_approval() {
        // Given a thread waiting on a permission prompt.
        let sessions = sessions(vec![thread(1, ThreadStatus::NeedsApproval)]);

        // When rendering the sidebar.
        let heading = line(&draw(&sessions, at(1000), 3), 0);

        // Then its card says it's pending approval.
        assert!(
            heading.contains("◐ Pending Approval"),
            "line was '{heading}'"
        );
    }

    #[rstest::rstest]
    fn idle_card_shows_time_since_last_activity() {
        // Given an idle thread whose last turn ended 3 h before now.
        let sessions = sessions(vec![thread(1, ThreadStatus::Idle)]);

        // When rendering the sidebar.
        let heading = line(&draw(&sessions, at(10_800), 3), 0);

        // Then its card shows the time since.
        assert!(heading.contains(" 3h"), "line was '{heading}'");
    }

    #[rstest::rstest]
    fn unseen_idle_card_shows_completed() {
        // Given an idle thread whose turn ended while the user was elsewhere.
        let sessions = sessions(vec![Thread {
            unseen: true,
            ..thread(1, ThreadStatus::Idle)
        }]);

        // When rendering the sidebar.
        let heading = line(&draw(&sessions, at(1000), 3), 0);

        // Then its card says the turn completed.
        assert!(heading.contains("✓ Completed"), "line was '{heading}'");
    }

    #[rstest::rstest]
    fn untitled_card_is_a_new_thread() {
        // Given a thread without a title yet.
        let sessions = sessions(vec![Thread {
            title: None,
            ..thread(1, ThreadStatus::Idle)
        }]);

        // When rendering the sidebar.
        let title = line(&draw(&sessions, at(1000), 3), 1);

        // Then its card says "New thread".
        assert!(title.contains("New thread"), "line was '{title}'");
    }

    #[rstest::rstest]
    fn card_third_line_shows_branch_and_claude_logo() {
        // Given a thread on `main`.
        let sessions = sessions(vec![Thread {
            branch: Some("main".to_owned()),
            ..thread(1, ThreadStatus::Idle)
        }]);

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 3);

        // Then the card's third line starts with the branch.
        let footer = line(&buf, 2);
        assert!(footer.starts_with(" main"), "line was '{footer}'");
        // And ends with ✳ before the padding and the border.
        assert_eq!(
            buf.cell((29, 2)).map(Cell::symbol),
            Some("✳"),
            "line was '{footer}'"
        );
    }

    #[rstest::rstest]
    fn pinned_card_shows_the_pin_marker() {
        // Given a pinned thread.
        let sessions = sessions(vec![Thread {
            pinned_at: Some(at(5)),
            ..thread(1, ThreadStatus::Idle)
        }]);

        // When rendering the sidebar.
        let heading = line(&draw(&sessions, at(1000), 3), 0);

        // Then its project name carries the pin.
        assert!(heading.contains("orb ⚑"), "line was '{heading}'");
    }

    #[rstest::rstest]
    fn collapsed_shelf_header_counts_settled_threads() {
        // Given one active and two settled threads, with the shelf closed.
        let sessions = sessions(vec![
            thread(1, ThreadStatus::Idle),
            settled(2, 10),
            settled(3, 20),
        ]);

        // When rendering the sidebar.
        let lines = lines(&draw(&sessions, at(1000), 10));

        // Then the header counts both settled threads.
        assert!(
            lines.iter().any(|line| line.contains("▸ Settled (2)")),
            "lines were {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn open_shelf_draws_slim_rows() {
        // Given one active and two settled threads, with the shelf open.
        let sessions = Sessions {
            shelf_open: true,
            ..sessions(vec![
                thread(1, ThreadStatus::Idle),
                settled(2, 10),
                settled(3, 20),
            ])
        };

        // When rendering the sidebar.
        let lines = lines(&draw(&sessions, at(1000), 10));

        // Then the header is followed by one line per settled thread, newest
        // settle first.
        let after: Vec<&String> = lines
            .iter()
            .skip_while(|line| !line.contains("▾ Settled"))
            .skip(1)
            .collect();
        assert!(
            matches!(
                after.as_slice(),
                [first, second] if first.contains("Thread 3") && second.contains("Thread 2")
            ),
            "lines were {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn selected_card_has_the_selection_background() {
        // Given a selected thread.
        let sessions = Sessions {
            cursor: Some(SidebarItem::Thread(ThreadId(1))),
            ..sessions(vec![thread(1, ThreadStatus::Idle)])
        };

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 5);

        // Then all three of its lines have the selection background.
        let backgrounds: Vec<_> = (0..3)
            .map(|y| buf.cell((0, y)).map(|cell| cell.bg))
            .collect();
        assert_eq!(
            backgrounds,
            vec![Some(SELECTED); 3],
            "the card's left padding"
        );
    }

    #[rstest::rstest]
    fn sidebar_scrolls_to_show_the_selected_card() {
        // Given three cards on a five-line sidebar, with the last one selected.
        let sessions = Sessions {
            cursor: Some(SidebarItem::Thread(ThreadId(1))),
            ..sessions(vec![
                thread(1, ThreadStatus::Idle),
                thread(2, ThreadStatus::Idle),
                thread(3, ThreadStatus::Idle),
            ])
        };

        // When rendering the sidebar.
        let lines = lines(&draw(&sessions, at(1000), 5));

        // Then the selected card's title is on screen.
        assert!(
            lines.iter().any(|line| line.contains("Thread 1")),
            "lines were {lines:#?}"
        );
    }
}
