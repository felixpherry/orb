//! The sidebar: one list of orb's drafts and threads across projects, drawn
//! like LazyVim's file explorer (snacks.nvim) in tokyonight-moon.
//!
//! An input box heads it: `Sessions`, with an `i` badge lit while the
//! Settled shelf is open, the filtered project after the `>` prompt, and how
//! many drafts and threads are listed out of all of them. Below it, drafts
//! come first, then pinned threads, then active ones, each a three-line tree
//! node: the status icon and title, then the project and status, then the
//! branch (a draft's workspace). The selected row's first line is
//! highlighted. Settled threads fold into a shelf at the bottom, drawn as
//! one-line rows while it's open. The sidebar scrolls to keep the whole
//! selected row in view.
//!
//! While the user searches, the typed text follows the prompt, and only
//! drafts and threads whose title matches it are listed, settled ones
//! included, with the matched characters highlighted as in the pickers.

use std::borrow::Cow;
use std::time::{Duration, SystemTime};

use orb_domain::feat::sessions::state::{
    Draft, DraftWorkspace, NEW_THREAD, Project, Sessions, SidebarRow, Thread, ThreadStatus,
};
use orb_domain::feat::sidebar::state::SidebarLayout;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Widget};
use unicode_segmentation::UnicodeSegmentation;

use crate::picker::highlight;

/// How far the sidebar is scrolled, kept between frames.
#[derive(Debug, Default)]
pub(crate) struct SidebarScroll {
    /// The list line drawn on the sidebar's top row.
    offset: u16,
}

/// Draws the sidebar into `area`, a blank column short of its right edge: the
/// input box, then the list, scrolled so the cursor's row is in view. Returns
/// the y of the selected row's first line when it's on screen, the list's
/// layout, and the search text's cursor while there is a search.
pub(crate) fn render(
    sessions: &Sessions,
    now: SystemTime,
    area: Rect,
    buf: &mut Buffer,
    scroll: &mut SidebarScroll,
) -> (Option<u16>, SidebarLayout, Option<Position>) {
    buf.set_style(area, Style::new().bg(BG_DARK).fg(FG));
    let area = Rect {
        width: area.width.saturating_sub(1),
        ..area
    };
    let [input, list] = Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(area);
    let rows = sessions.sidebar();
    let search_cursor = render_input(sessions, &rows, input, buf);
    let layout = SidebarLayout {
        rows: list.height,
        heights: rows.iter().map(height).collect(),
    };
    let selected_y = render_list(sessions, rows, now, list, buf, scroll);
    (selected_y, layout, search_cursor)
}

/// The rounded box at the top: its title and shelf badge, the prompt, and the
/// count against its right edge. Returns the search text's cursor while
/// there is a search.
fn render_input(
    sessions: &Sessions,
    rows: &[SidebarRow<'_>],
    area: Rect,
    buf: &mut Buffer,
) -> Option<Position> {
    let badge = if sessions.shelf_open {
        Style::new().fg(BLUE).bg(GUTTER)
    } else {
        Style::new().fg(DARK3)
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ORANGE))
        .title(Line::from(vec![
            span(" Sessions ", ORANGE),
            Span::styled(" i ", badge),
            Span::raw(" "),
        ]));
    let inner = block.inner(area);
    block.render(area, buf);
    let (prompt, cursor) = prompt(sessions);
    render_split(
        prompt,
        Line::from(span(count(sessions, rows), COMMENT)),
        inner,
        buf,
    );
    cursor.map(|column| {
        Position::new(
            inner
                .x
                .saturating_add(column)
                .min(inner.right().saturating_sub(1)),
            inner.y,
        )
    })
}

/// `>`, followed by the filtered project's folder and name while there is
/// one, then the search text while there is a search, where snacks shows the
/// query. Also returns the column of the search text's cursor.
fn prompt(sessions: &Sessions) -> (Line<'_>, Option<u16>) {
    let filtered = sessions
        .filter
        .and_then(|id| sessions.projects.iter().find(|project| project.id == id));
    let mut spans = match filtered {
        Some(project) => vec![
            span("> ", CYAN),
            span(format!("{FOLDER} "), project_colour(&project.title)),
            span(project.title.as_str(), FG),
        ],
        None => vec![span(">", CYAN)],
    };
    let Some(search) = &sessions.search else {
        return (Line::from(spans), None);
    };
    spans.push(Span::raw(" "));
    let typed: String = search
        .input
        .text()
        .graphemes(true)
        .take(search.input.cursor())
        .collect();
    let column = spans
        .iter()
        .map(Span::width)
        .sum::<usize>()
        .saturating_add(Span::raw(typed).width());
    spans.push(span(search.input.text(), FG));
    (
        Line::from(spans),
        Some(u16::try_from(column).unwrap_or(u16::MAX)),
    )
}

/// `shown/total`, like snacks' match count: the drafts and threads listed,
/// out of every draft and thread not being deleted.
fn count(sessions: &Sessions, rows: &[SidebarRow<'_>]) -> String {
    let shown = rows
        .iter()
        .filter(|row| !matches!(row, SidebarRow::ShelfHeader { .. }))
        .count();
    let drafts = sessions
        .projects
        .iter()
        .filter(|project| project.draft.is_some())
        .count();
    let threads = sessions
        .threads()
        .filter(|thread| !sessions.deleting.contains(&thread.id))
        .count();
    format!("{shown}/{}", drafts + threads)
}

/// Draws the rows into `area`, scrolled so the selected one is whole on
/// screen, and returns the y of its first line when it's there.
fn render_list(
    sessions: &Sessions,
    rows: Vec<SidebarRow<'_>>,
    now: SystemTime,
    area: Rect,
    buf: &mut Buffer,
    scroll: &mut SidebarScroll,
) -> Option<u16> {
    let (placed, total) = place(rows, area.height);
    let selected = placed
        .iter()
        .find(|(row, _)| Some(row.item()) == sessions.cursor)
        .map(|(row, top)| (*top, height(row)));
    if let Some((top, rows)) = selected {
        scroll.offset = scroll
            .offset
            .min(top)
            .max(top.saturating_add(rows).saturating_sub(area.height));
    }
    scroll.offset = scroll.offset.min(total.saturating_sub(area.height));
    // The whole list, then the lines in view.
    let list = {
        let mut list = Buffer::empty(Rect::new(area.x, 0, area.width, total));
        list.set_style(list.area, Style::new().bg(BG_DARK).fg(FG));
        for (index, (row, top)) in placed.iter().enumerate() {
            let row_area = Rect::new(area.x, *top, area.width, height(row));
            if Some(row.item()) == sessions.cursor {
                list.set_style(
                    Rect {
                        height: 1,
                        ..row_area
                    },
                    Style::new().bg(VISUAL),
                );
            }
            let ends_shelf =
                !matches!(placed.get(index + 1), Some((SidebarRow::Settled { .. }, _)));
            render_row(sessions, row, ends_shelf, now, row_area, &mut list);
        }
        list
    };
    for (y, line) in (area.top()..area.bottom()).zip(scroll.offset..total) {
        for x in area.left()..area.right() {
            if let (Some(cell), Some(shown)) = (list.cell((x, line)), buf.cell_mut((x, y))) {
                *shown = cell.clone();
            }
        }
    }
    selected
        .map(|(top, _)| top)
        .filter(|top| (scroll.offset..scroll.offset.saturating_add(area.height)).contains(top))
        .map(|top| area.y + top - scroll.offset)
}

/// Each row with its top line in the list, and the list's height. Blank lines
/// above the shelf header keep the shelf at the bottom of `lines` while the
/// list is short.
fn place(rows: Vec<SidebarRow<'_>>, lines: u16) -> (Vec<(SidebarRow<'_>, u16)>, u16) {
    let content = rows.iter().map(height).fold(0, u16::saturating_add);
    let gap = lines.saturating_sub(content);
    let mut top = 0_u16;
    let placed = rows
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
}

/// How many lines a row takes: a draft's or thread's node 3, else 1.
fn height(row: &SidebarRow<'_>) -> u16 {
    match row {
        SidebarRow::Draft { .. } | SidebarRow::Card { .. } => 3,
        SidebarRow::ShelfHeader { .. } | SidebarRow::Settled { .. } => 1,
    }
}

/// One row; `ends_shelf` says no settled row follows it. Titles show where
/// the search matched them.
fn render_row(
    sessions: &Sessions,
    row: &SidebarRow<'_>,
    ends_shelf: bool,
    now: SystemTime,
    area: Rect,
    buf: &mut Buffer,
) {
    let matched = |title: &str| sessions.title_matches(title).unwrap_or_default();
    match row {
        SidebarRow::Draft { project, draft } => {
            render_draft(project, draft, &matched(NEW_THREAD), area, buf);
        }
        SidebarRow::Card { project, thread } => {
            render_card(project, thread, &matched(title(thread)), now, area, buf);
        }
        SidebarRow::ShelfHeader { count, open } => render_shelf_header(*count, *open, area, buf),
        SidebarRow::Settled { thread, .. } => {
            let matched = matched(title(thread));
            render_settled(thread, &matched, ends_shelf, now, area, buf);
        }
    }
}

/// A thread's node: its status icon, title (`matched` at those byte
/// offsets), pin and time; the project and status word; the branch and ✳.
fn render_card(
    project: &Project,
    thread: &Thread,
    matched: &[usize],
    now: SystemTime,
    area: Rect,
    buf: &mut Buffer,
) {
    let [heading, place, footer] = Layout::vertical([Constraint::Length(1); 3]).areas(area);
    let (glyph, word, colour) = status(thread, now);
    let pin = if thread.pinned_at.is_some() { PIN } else { "" };
    render_split(
        Line::from(
            [span(format!(" {glyph} "), colour)]
                .into_iter()
                .chain(highlight(title(thread), matched, |_| FG))
                .collect::<Vec<_>>(),
        ),
        Line::from(vec![
            span(format!("{pin} "), ORANGE),
            span(when(thread, now), COMMENT),
        ]),
        heading,
        buf,
    );
    render_split(
        project_line(project),
        word.map(|word| Line::from(span(word, colour)))
            .unwrap_or_default(),
        place,
        buf,
    );
    render_split(
        Line::from(vec![
            span(LAST_GUIDE, GUTTER),
            span(
                format!("{BRANCH} {}", thread.branch.as_deref().unwrap_or("—")),
                COMMENT,
            ),
        ]),
        Line::from(span(CLAUDE_LOGO, CLAUDE)),
        footer,
        buf,
    );
}

/// A draft's node: the pencil and `New thread` (`matched` at those byte
/// offsets); the project; the workspace and branch it will start on.
fn render_draft(project: &Project, draft: &Draft, matched: &[usize], area: Rect, buf: &mut Buffer) {
    let [heading, place, footer] = Layout::vertical([Constraint::Length(1); 3]).areas(area);
    render_split(
        Line::from(
            [span(format!(" {PENCIL} "), YELLOW)]
                .into_iter()
                .chain(highlight(NEW_THREAD, matched, |_| FG))
                .collect::<Vec<_>>(),
        ),
        Line::from(span("draft", DARK3)),
        heading,
        buf,
    );
    render_split(project_line(project), Line::default(), place, buf);
    render_split(
        Line::from(vec![
            span(LAST_GUIDE, GUTTER),
            span(format!("{BRANCH} {}", workspace(draft)), COMMENT),
        ]),
        Line::default(),
        footer,
        buf,
    );
}

/// A node's middle line: the project's folder in its badge colour, and its
/// name.
fn project_line(project: &Project) -> Line<'_> {
    Line::from(vec![
        span(GUIDE, GUTTER),
        span(format!("{FOLDER} "), project_colour(&project.title)),
        span(project.title.as_str(), FG_DARK),
    ])
}

/// Where a draft will start: `local`, `new worktree` or `worktree`, then its
/// branch when it has one.
fn workspace(draft: &Draft) -> String {
    let place = match draft.workspace {
        DraftWorkspace::Local => "local",
        DraftWorkspace::NewWorktree => "new worktree",
        DraftWorkspace::Existing(_) => "worktree",
    };
    match &draft.branch {
        Some(branch) => format!("{place} · {branch}"),
        None => place.to_owned(),
    }
}

/// The Settled shelf's folder, open or closed, and how many threads it holds.
fn render_shelf_header(count: usize, open: bool, area: Rect, buf: &mut Buffer) {
    let folder = if open { FOLDER_OPEN } else { FOLDER };
    render_split(
        Line::from(vec![
            span(format!(" {folder} "), BLUE),
            span("Settled", BLUE),
        ]),
        Line::from(span(count.to_string(), COMMENT)),
        area,
        buf,
    );
}

/// A settled thread's row under the shelf's folder: its icon (a dim check
/// unless it failed or is gone), title (`matched` at those byte offsets), and
/// the time since it settled.
fn render_settled(
    thread: &Thread,
    matched: &[usize],
    ends_shelf: bool,
    now: SystemTime,
    area: Rect,
    buf: &mut Buffer,
) {
    let guide = if ends_shelf { LAST_GUIDE } else { GUIDE };
    let (glyph, colour) = match thread.status {
        ThreadStatus::Failed | ThreadStatus::Gone => {
            let (glyph, _, colour) = status(thread, now);
            (glyph, colour)
        }
        _ => (COMPLETED_ICON, DARK3),
    };
    render_split(
        Line::from(
            [span(guide, GUTTER), span(format!("{glyph} "), colour)]
                .into_iter()
                .chain(highlight(title(thread), matched, |_| COMMENT))
                .collect::<Vec<_>>(),
        ),
        Line::from(span(when(thread, now), DARK3)),
        area,
        buf,
    );
}

/// A thread's status as its icon, its short word (none while idle), and
/// their colour.
fn status(thread: &Thread, now: SystemTime) -> (&'static str, Option<&'static str>, Color) {
    match thread.status {
        ThreadStatus::NeedsApproval => (APPROVAL_ICON, Some("approval"), YELLOW),
        ThreadStatus::NeedsInput => (INPUT_ICON, Some("input"), MAGENTA),
        ThreadStatus::Working => (spinner(thread, now), Some("working"), BLUE),
        ThreadStatus::Failed => (FAILED_ICON, Some("failed"), RED),
        ThreadStatus::Gone => (GONE_ICON, Some("gone"), RED),
        ThreadStatus::Idle if thread.unseen => (COMPLETED_ICON, Some("done"), GREEN),
        ThreadStatus::Stopped => (STOPPED_ICON, Some("stopped"), COMMENT),
        ThreadStatus::Idle | ThreadStatus::Unknown => (IDLE_ICON, None, DARK3),
    }
}

/// The working spinner's frame: one per [`SPINNER_FRAME`] since the turn
/// started, so it moves with the loop's redraw while a thread works.
fn spinner(thread: &Thread, now: SystemTime) -> &'static str {
    let frames = thread
        .turn_started_at
        .map(|at| since(now, at).as_millis() / SPINNER_FRAME.as_millis())
        .unwrap_or_default();
    SPINNER
        .get(frames as usize % SPINNER.len())
        .copied()
        .unwrap_or_default()
}

/// How long the turn has run while working, else how long ago the last turn
/// ended, or the thread settled.
fn when(thread: &Thread, now: SystemTime) -> String {
    match (thread.status, thread.turn_started_at) {
        (ThreadStatus::Working, Some(at)) => working_label(since(now, at)),
        _ => ago_label(since(
            now,
            thread.settled_at.unwrap_or(thread.last_activity_at),
        )),
    }
}

fn title(thread: &Thread) -> &str {
    thread.title.as_deref().unwrap_or(NEW_THREAD)
}

fn span<'a, T>(text: T, fg: Color) -> Span<'a>
where
    T: Into<Cow<'a, str>>,
{
    Span::styled(text, Style::new().fg(fg))
}

/// The project's badge colour, lit.
fn project_colour(name: &str) -> Color {
    let (r, g, b) = BADGE.get(badge_colour(name)).copied().unwrap_or_default();
    Color::Rgb(r, g, b)
}

/// The Settled shelf's label in the right side's hint: `▸ Settled (N)`
/// closed, `▾ Settled` open.
pub(crate) fn shelf_label(count: usize, open: bool) -> String {
    if open {
        "▾ Settled".to_owned()
    } else {
        format!("▸ Settled ({count})")
    }
}

/// Draws `left`, and `right` against the right edge with a cell between them.
pub(crate) fn render_split(left: Line<'_>, right: Line<'_>, area: Rect, buf: &mut Buffer) {
    let [left_area, right_area] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(width(&right).saturating_add(1)),
    ])
    .areas(area);
    left.render(left_area, buf);
    right.right_aligned().render(right_area, buf);
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
pub(crate) fn badge(name: &str, lit: bool) -> Span<'static> {
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
/// Neutral-400: labels in the draft form.
pub(crate) const GRAY: Color = Color::Rgb(0xa3, 0xa3, 0xa3);

/// The text on the mode line's mode and clock blocks (`black`).
pub(crate) const BLACK: Color = Color::Rgb(0x1b, 0x1d, 0x2b);
/// Behind the whole sidebar, the picker and the mode line (tokyonight-moon's
/// `bg_dark`, lualine's `bg_statusline`), darker than the right side, so the
/// sidebar needs no border.
pub(crate) const BG_DARK: Color = Color::Rgb(0x1e, 0x20, 0x30);
/// Behind the selected row's first line, and the picker's selected row
/// (`bg_visual`, the explorer's cursorline).
pub(crate) const VISUAL: Color = Color::Rgb(0x2d, 0x3f, 0x76);
/// The tree guides, behind the lit shelf badge, and behind the mode line's
/// branch and position blocks (`fg_gutter`).
pub(crate) const GUTTER: Color = Color::Rgb(0x3b, 0x42, 0x61);
/// Titles, the filtered project's name, the picker's input and rows, and
/// the rename box's text (`fg`).
pub(crate) const FG: Color = Color::Rgb(0xc8, 0xd3, 0xf5);
/// Project names, and the keys in the picker's hints (`fg_dark`).
pub(crate) const FG_DARK: Color = Color::Rgb(0x82, 0x8b, 0xb8);
/// Times, branches, the count, settled titles and the stopped icon; the
/// picker's hint labels, headings and empty-list text (`comment`).
pub(crate) const COMMENT: Color = Color::Rgb(0x63, 0x6d, 0xa6);
/// The idle icon, `draft`, the unlit shelf badge and settled marks; the
/// picker's disabled branches and hint separators (`dark3`).
pub(crate) const DARK3: Color = Color::Rgb(0x54, 0x5c, 0x7e);
/// Working, the Settled shelf and the lit shelf badge; the picker's title and
/// folders (`blue`).
pub(crate) const BLUE: Color = Color::Rgb(0x82, 0xaa, 0xff);
/// The `>` prompt, here and in the picker, and the picker's `default`
/// branch badge (`cyan`).
pub(crate) const CYAN: Color = Color::Rgb(0x86, 0xe1, 0xfc);
/// A completed turn; the picker's current branch and new worktree; the mode
/// line's INSERT (`green`).
pub(crate) const GREEN: Color = Color::Rgb(0xc3, 0xe8, 0x8d);
/// The mode line's ATTACHED (`green1`, lualine's terminal mode).
pub(crate) const GREEN1: Color = Color::Rgb(0x4f, 0xd6, 0xbe);
/// Needing approval, and a draft's pencil; the picker's permission shield
/// and `worktree` branch badge; the rename box (`yellow`).
pub(crate) const YELLOW: Color = Color::Rgb(0xff, 0xc7, 0x77);
/// The input box and the pin; the rule under the picker's input and its
/// git icon (`orange`).
pub(crate) const ORANGE: Color = Color::Rgb(0xff, 0x96, 0x6c);
/// Failed and gone, and the mode line's error (`red`).
pub(crate) const RED: Color = Color::Rgb(0xff, 0x75, 0x7f);
/// Needing input; the picker's remote branches and previous worktree
/// (`magenta`).
pub(crate) const MAGENTA: Color = Color::Rgb(0xc0, 0x99, 0xff);
/// The picker's row numbers and dimmed path parents (`dark5`).
pub(crate) const DARK5: Color = Color::Rgb(0x73, 0x7a, 0xa2);
/// The picker's matched characters, and the rename box's edit icon (`blue1`,
/// snacks' `SnacksInputIcon`).
pub(crate) const BLUE1: Color = Color::Rgb(0x65, 0xbc, 0xff);
/// The picker's border (`border_highlight`).
pub(crate) const BORDER: Color = Color::Rgb(0x58, 0x9e, 0xd7);
/// The ✳ logo, here and before the picker's models (Claude orange).
pub(crate) const CLAUDE: Color = Color::Rgb(0xd9, 0x77, 0x57);

/// Needing approval, here and in the mode line's count (Nerd Font
/// `nf-fa-warning`).
pub(crate) const APPROVAL_ICON: &str = "\u{f071}";
/// Needing input, here and in the mode line's count (`nf-fa-question_circle`).
pub(crate) const INPUT_ICON: &str = "\u{f059}";
/// Failed, and before the mode line's error (`nf-fa-times_circle`).
pub(crate) const FAILED_ICON: &str = "\u{f057}";
/// Gone (`nf-fa-ban`).
const GONE_ICON: &str = "\u{f05e}";
/// A completed turn, and a settled thread that didn't fail
/// (`nf-fa-check_circle`).
const COMPLETED_ICON: &str = "\u{f058}";
/// Stopped (`nf-fa-stop`).
const STOPPED_ICON: &str = "\u{f04d}";
/// Idle, or a status orb doesn't know (`nf-fa-circle_o`).
const IDLE_ICON: &str = "\u{f10c}";
/// How long each spinner frame shows; the loop redraws this often while a
/// thread works or a session starts.
pub(crate) const SPINNER_FRAME: Duration = Duration::from_millis(100);
/// The spinner's frames, here and in the mode line's activity.
pub(crate) const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// A pinned thread (`nf-fa-thumb_tack`).
const PIN: &str = "\u{f08d}";
/// Before a branch, here and in the picker (`nf-pl-branch`).
pub(crate) const BRANCH: &str = "\u{e0a0}";
/// A project, the closed Settled shelf, and the picker's folders
/// (`nf-fa-folder`).
pub(crate) const FOLDER: &str = "\u{f07b}";
/// The open Settled shelf, and the picker's `All projects` row
/// (`nf-fa-folder_open`).
pub(crate) const FOLDER_OPEN: &str = "\u{f07c}";
/// A draft (`nf-fa-pencil`).
const PENCIL: &str = "\u{f040}";
/// Claude's logo, at the end of a thread's node and before the picker's
/// models.
pub(crate) const CLAUDE_LOGO: &str = "✳";
/// The tree guide before a node's middle line and a settled row.
const GUIDE: &str = " ├╴";
/// The tree guide before a node's last line and the last settled row.
const LAST_GUIDE: &str = " └╴";

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use orb_domain::TextInput;
    use orb_domain::feat::sessions::state::{
        Draft, DraftWorkspace, Project, ProjectId, Search, Sessions, SidebarItem, Thread, ThreadId,
        ThreadStatus,
    };
    use orb_domain::feat::sidebar::state::SidebarLayout;
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::{Position, Rect};
    use ratatui::style::{Color, Modifier};

    use super::{
        APPROVAL_ICON, BG_DARK, BLUE, BLUE1, BRANCH, CLAUDE, CLAUDE_LOGO, COMMENT, COMPLETED_ICON,
        CYAN, DARK3, FAILED_ICON, FOLDER, FOLDER_OPEN, GONE_ICON, GREEN, GUIDE, GUTTER, IDLE_ICON,
        INPUT_ICON, LAST_GUIDE, MAGENTA, ORANGE, PENCIL, PIN, RED, STOPPED_ICON, SidebarScroll,
        VISUAL, YELLOW, ago_label, badge_colour, monogram, render, working_label,
    };

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    /// A thread titled "Thread <id>" whose turn started at 866 s.
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

    fn project(id: i64, title: &str, threads: Vec<Thread>) -> Project {
        Project {
            id: ProjectId(id),
            title: title.to_owned(),
            root: format!("/Users/me/dev/{title}").into(),
            created_at: SystemTime::UNIX_EPOCH,
            removed: false,
            draft: None,
            threads,
        }
    }

    fn sessions(threads: Vec<Thread>) -> Sessions {
        Sessions {
            projects: vec![project(1, "orb", threads)],
            ..Sessions::default()
        }
    }

    /// orb with only a draft in `workspace` on `branch`.
    fn draft(workspace: DraftWorkspace, branch: Option<&str>) -> Sessions {
        let mut sessions = sessions(vec![]);
        if let Some(project) = sessions.projects.first_mut() {
            project.draft = Some(Draft {
                workspace,
                branch: branch.map(str::to_owned),
                model: None,
                permission: None,
                created_at: SystemTime::UNIX_EPOCH,
                repo: true,
                from: None,
            });
        }
        sessions
    }

    /// orb's thread 1 and web's thread 2, filtered to orb.
    fn filtered() -> Sessions {
        Sessions {
            projects: vec![
                project(1, "orb", vec![thread(1, ThreadStatus::Idle)]),
                project(2, "web", vec![thread(2, ThreadStatus::Idle)]),
            ],
            filter: Some(ProjectId(1)),
            ..Sessions::default()
        }
    }

    /// `sessions` with `id`'s thread under the cursor.
    fn select(sessions: Sessions, id: i64) -> Sessions {
        Sessions {
            cursor: Some(SidebarItem::Thread(ThreadId(id))),
            ..sessions
        }
    }

    /// Draws a `width`x`height` sidebar at `now`; returns the buffer and what
    /// `render` returned.
    fn render_sized(
        sessions: &Sessions,
        now: SystemTime,
        width: u16,
        height: u16,
    ) -> (Buffer, Option<u16>, SidebarLayout) {
        let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
        let (selected_y, layout, _) = render(
            sessions,
            now,
            buf.area,
            &mut buf,
            &mut SidebarScroll::default(),
        );
        (buf, selected_y, layout)
    }

    /// Draws a 32-column sidebar `height` lines tall at `now`. The list's
    /// first node takes lines 3 to 5, and its right edge is column 30.
    fn draw(sessions: &Sessions, now: SystemTime, height: u16) -> Buffer {
        render_sized(sessions, now, 32, height).0
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

    /// The glyph at `(x, y)` and its colour.
    fn glyph(buf: &Buffer, x: u16, y: u16) -> Option<(String, Color)> {
        buf.cell((x, y))
            .map(|cell| (cell.symbol().to_owned(), cell.fg))
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
    fn sidebar_background_is_tokyonights_bg_dark() {
        // Given one thread.
        let sessions = sessions(vec![thread(1, ThreadStatus::Idle)]);

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 10);

        // Then the blank column on its right edge has the dark background.
        assert_eq!(
            buf.cell((31, 4)).map(|cell| cell.bg),
            Some(BG_DARK),
            "the right edge's background"
        );
    }

    #[rstest::rstest]
    fn input_box_is_titled_sessions() {
        // Given an empty orb.
        let sessions = Sessions::default();

        // When rendering the sidebar.
        let top = line(&draw(&sessions, at(1000), 5), 0);

        // Then the box's top border carries the title and the shelf badge.
        assert!(top.starts_with("╭ Sessions  i  ─"), "line was '{top}'");
    }

    #[rstest::rstest]
    fn input_box_border_is_orange() {
        // Given an empty orb.
        let sessions = Sessions::default();

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 5);

        // Then the box's corner is orange.
        assert_eq!(
            glyph(&buf, 0, 0),
            Some(("╭".to_owned(), ORANGE)),
            "the box's top-left corner"
        );
    }

    #[rstest::rstest]
    #[case::closed(false, (DARK3, BG_DARK))]
    #[case::open(true, (BLUE, GUTTER))]
    fn shelf_badge_lights_up_while_the_shelf_is_open(
        #[case] shelf_open: bool,
        #[case] expected: (Color, Color),
    ) {
        // Given the Settled shelf open or closed.
        let sessions = Sessions {
            shelf_open,
            ..sessions(vec![settled(1, 10)])
        };

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 8);

        // Then the badge's `i` has the shelf's colours.
        let badge = buf
            .cell((12, 0))
            .map(|cell| (cell.symbol().to_owned(), (cell.fg, cell.bg)));
        assert_eq!(
            badge,
            Some(("i".to_owned(), expected)),
            "the badge with the shelf open: {shelf_open}"
        );
    }

    #[rstest::rstest]
    fn prompt_is_a_cyan_chevron() {
        // Given an empty orb.
        let sessions = Sessions::default();

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 5);

        // Then the box's line starts with a cyan `>`.
        assert_eq!(
            glyph(&buf, 1, 1),
            Some((">".to_owned(), CYAN)),
            "the prompt"
        );
    }

    #[rstest::rstest]
    fn count_reads_listed_out_of_every_thread() {
        // Given one active and two settled threads, with the shelf closed.
        let sessions = sessions(vec![
            thread(1, ThreadStatus::Idle),
            settled(2, 10),
            settled(3, 20),
        ]);

        // When rendering the sidebar.
        let prompt = line(&draw(&sessions, at(1000), 10), 1);

        // Then the count is the one listed thread out of three.
        assert!(prompt.trim_end().ends_with("1/3│"), "line was '{prompt}'");
    }

    #[rstest::rstest]
    fn filtered_prompt_shows_the_project() {
        // Given the sidebar filtered to orb.
        let sessions = filtered();

        // When rendering the sidebar.
        let prompt = line(&draw(&sessions, at(1000), 10), 1);

        // Then the prompt is followed by orb's folder and name.
        assert!(
            prompt.starts_with(&format!("│> {FOLDER} orb ")),
            "line was '{prompt}'"
        );
    }

    #[rstest::rstest]
    fn filtered_count_is_out_of_every_projects_threads() {
        // Given orb's and web's threads, filtered to orb.
        let sessions = filtered();

        // When rendering the sidebar.
        let prompt = line(&draw(&sessions, at(1000), 10), 1);

        // Then the count is orb's thread out of both.
        assert!(prompt.trim_end().ends_with("1/2│"), "line was '{prompt}'");
    }

    /// `sessions` searching for `text`.
    fn searching(sessions: Sessions, text: &str) -> Sessions {
        Sessions {
            search: Some(Search {
                input: TextInput::new(text),
                return_to: None,
            }),
            ..sessions
        }
    }

    #[rstest::rstest]
    fn search_text_follows_the_filtered_project() {
        // Given the sidebar filtered to orb, searching for "thr".
        let sessions = searching(filtered(), "thr");

        // When rendering the sidebar.
        let prompt = line(&draw(&sessions, at(1000), 10), 1);

        // Then the search text follows orb's name.
        assert!(
            prompt.starts_with(&format!("│> {FOLDER} orb thr ")),
            "line was '{prompt}'"
        );
    }

    #[rstest::rstest]
    fn search_text_follows_the_chevron() {
        // Given no filter, searching for "thr".
        let sessions = searching(sessions(vec![thread(1, ThreadStatus::Idle)]), "thr");

        // When rendering the sidebar.
        let prompt = line(&draw(&sessions, at(1000), 10), 1);

        // Then the search text follows the `>`.
        assert!(prompt.starts_with("│> thr "), "line was '{prompt}'");
    }

    #[rstest::rstest]
    fn search_cursor_is_after_the_typed_text() {
        // Given a filtered sidebar searching for "thr".
        let sessions = searching(filtered(), "thr");

        // When rendering the sidebar.
        let mut buf = Buffer::empty(Rect::new(0, 0, 32, 10));
        let (_, _, cursor) = render(
            &sessions,
            at(1000),
            buf.area,
            &mut buf,
            &mut SidebarScroll::default(),
        );

        // Then the cursor is after `> `, the folder and space, `orb`, a space
        // and `thr`: 11 columns into the box.
        assert_eq!(cursor, Some(Position::new(12, 1)), "the search cursor");
    }

    #[rstest::rstest]
    fn search_count_reads_matches_out_of_every_thread() {
        // Given three threads, searching for "2".
        let sessions = searching(
            sessions(vec![
                thread(1, ThreadStatus::Idle),
                thread(2, ThreadStatus::Idle),
                thread(3, ThreadStatus::Idle),
            ]),
            "2",
        );

        // When rendering the sidebar.
        let prompt = line(&draw(&sessions, at(1000), 10), 1);

        // Then the count is the one match out of three.
        assert!(prompt.trim_end().ends_with("1/3│"), "line was '{prompt}'");
    }

    #[rstest::rstest]
    fn search_highlights_the_matched_title_characters() {
        // Given "Thread 7", searching for "7".
        let sessions = searching(sessions(vec![thread(7, ThreadStatus::Idle)]), "7");

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 8);

        // Then the title's `7` is blue and bold.
        let seven = (0..buf.area.width)
            .filter_map(|x| buf.cell((x, 3)))
            .find(|cell| cell.symbol() == "7")
            .map(|cell| (cell.fg, cell.modifier.contains(Modifier::BOLD)));
        assert_eq!(seven, Some((BLUE1, true)), "the matched `7`");
    }

    #[rstest::rstest]
    fn search_highlights_the_matched_characters_of_a_draft() {
        // Given a draft, searching for "N".
        let sessions = searching(draft(DraftWorkspace::Local, None), "N");

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 8);

        // Then the `N` of `New thread` is blue and bold.
        let n = (0..buf.area.width)
            .filter_map(|x| buf.cell((x, 3)))
            .find(|cell| cell.symbol() == "N")
            .map(|cell| (cell.fg, cell.modifier.contains(Modifier::BOLD)));
        assert_eq!(n, Some((BLUE1, true)), "the draft's matched `N`");
    }

    #[rstest::rstest]
    fn search_highlights_the_matched_characters_of_a_settled_thread() {
        // Given settled "Thread 7" with the shelf closed, searching for "7".
        let sessions = searching(sessions(vec![settled(7, 10)]), "7");

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 8);

        // Then the settled row's `7` is blue and bold.
        let seven = (0..buf.area.height)
            .find(|&y| line(&buf, usize::from(y)).contains("Thread 7"))
            .and_then(|y| {
                (0..buf.area.width)
                    .filter_map(|x| buf.cell((x, y)))
                    .find(|cell| cell.symbol() == "7")
            })
            .map(|cell| (cell.fg, cell.modifier.contains(Modifier::BOLD)));
        assert_eq!(seven, Some((BLUE1, true)), "the settled row's matched `7`");
    }

    #[rstest::rstest]
    fn node_first_line_is_the_status_icon_and_title() {
        // Given an idle thread.
        let sessions = sessions(vec![thread(1, ThreadStatus::Idle)]);

        // When rendering the sidebar.
        let heading = line(&draw(&sessions, at(1000), 8), 3);

        // Then its first line is the icon, then the title.
        assert!(
            heading.starts_with(&format!(" {IDLE_ICON} Thread 1 ")),
            "line was '{heading}'"
        );
    }

    #[rstest::rstest]
    #[case::approval(ThreadStatus::NeedsApproval, false, APPROVAL_ICON, YELLOW)]
    #[case::input(ThreadStatus::NeedsInput, false, INPUT_ICON, MAGENTA)]
    #[case::failed(ThreadStatus::Failed, false, FAILED_ICON, RED)]
    #[case::gone(ThreadStatus::Gone, false, GONE_ICON, RED)]
    #[case::completed(ThreadStatus::Idle, true, COMPLETED_ICON, GREEN)]
    #[case::stopped(ThreadStatus::Stopped, false, STOPPED_ICON, COMMENT)]
    #[case::idle(ThreadStatus::Idle, false, IDLE_ICON, DARK3)]
    #[case::unknown(ThreadStatus::Unknown, false, IDLE_ICON, DARK3)]
    fn status_icon_shows_the_threads_status(
        #[case] status: ThreadStatus,
        #[case] unseen: bool,
        #[case] icon: &str,
        #[case] colour: Color,
    ) {
        // Given a thread in `status`.
        let sessions = sessions(vec![Thread {
            unseen,
            ..thread(1, status)
        }]);

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 8);

        // Then its icon is the status's, in the status's colour.
        assert_eq!(
            glyph(&buf, 1, 3),
            Some((icon.to_owned(), colour)),
            "the icon for {status:?}"
        );
    }

    #[rstest::rstest]
    fn working_icon_is_the_spinner_frame_for_its_elapsed_tenths() {
        // Given a Working thread whose turn started 134.3 s before now.
        let sessions = sessions(vec![thread(1, ThreadStatus::Working)]);

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000) + Duration::from_millis(300), 8);

        // Then its icon is the spinner's fourth frame (1343 tenths), in blue.
        assert_eq!(
            glyph(&buf, 1, 3),
            Some(("⠸".to_owned(), BLUE)),
            "the spinner 134.3 s in"
        );
    }

    #[rstest::rstest]
    fn working_thread_shows_its_elapsed_time() {
        // Given a Working thread whose turn started 134 s before now.
        let sessions = sessions(vec![thread(1, ThreadStatus::Working)]);

        // When rendering the sidebar.
        let heading = line(&draw(&sessions, at(1000), 8), 3);

        // Then its first line ends with how long it has been working.
        assert!(heading.trim_end().ends_with(" 2m"), "line was '{heading}'");
    }

    #[rstest::rstest]
    fn idle_thread_shows_time_since_last_activity() {
        // Given an idle thread whose last turn ended 3 h before now.
        let sessions = sessions(vec![thread(1, ThreadStatus::Idle)]);

        // When rendering the sidebar.
        let heading = line(&draw(&sessions, at(10_800), 8), 3);

        // Then its first line ends with the time since.
        assert!(heading.trim_end().ends_with(" 3h"), "line was '{heading}'");
    }

    #[rstest::rstest]
    fn untitled_thread_is_a_new_thread() {
        // Given a thread without a title yet.
        let sessions = sessions(vec![Thread {
            title: None,
            ..thread(1, ThreadStatus::Idle)
        }]);

        // When rendering the sidebar.
        let heading = line(&draw(&sessions, at(1000), 8), 3);

        // Then it's called "New thread".
        assert!(heading.contains(" New thread "), "line was '{heading}'");
    }

    #[rstest::rstest]
    fn pinned_thread_shows_an_orange_pin() {
        // Given a pinned thread.
        let sessions = sessions(vec![Thread {
            pinned_at: Some(at(5)),
            ..thread(1, ThreadStatus::Idle)
        }]);

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 8);

        // Then its first line carries the pin, in orange.
        let pin = (0..32)
            .filter_map(|x| glyph(&buf, x, 3))
            .find(|(symbol, _)| symbol == PIN);
        assert_eq!(pin, Some((PIN.to_owned(), ORANGE)), "the pin");
    }

    #[rstest::rstest]
    fn node_second_line_shows_the_project() {
        // Given an idle thread in orb.
        let sessions = sessions(vec![thread(1, ThreadStatus::Idle)]);

        // When rendering the sidebar.
        let place = line(&draw(&sessions, at(1000), 8), 4);

        // Then its second line is a guide, orb's folder and name, and no
        // status word.
        assert_eq!(
            place.trim_end(),
            format!(" ├╴{FOLDER} orb"),
            "the node's second line"
        );
    }

    #[rstest::rstest]
    fn project_folder_takes_the_badge_colour() {
        // Given a thread in orb, whose badge is rose-400.
        let sessions = sessions(vec![thread(1, ThreadStatus::Idle)]);

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 8);

        // Then the folder on its second line is rose.
        assert_eq!(
            glyph(&buf, 3, 4),
            Some((FOLDER.to_owned(), Color::Rgb(0xfb, 0x71, 0x85))),
            "orb's folder"
        );
    }

    #[rstest::rstest]
    #[case(ThreadStatus::NeedsApproval, false, "approval")]
    #[case(ThreadStatus::NeedsInput, false, "input")]
    #[case(ThreadStatus::Working, false, "working")]
    #[case(ThreadStatus::Failed, false, "failed")]
    #[case(ThreadStatus::Gone, false, "gone")]
    #[case(ThreadStatus::Idle, true, "done")]
    #[case(ThreadStatus::Stopped, false, "stopped")]
    fn node_second_line_ends_with_the_status_word(
        #[case] status: ThreadStatus,
        #[case] unseen: bool,
        #[case] word: &str,
    ) {
        // Given a thread in `status`.
        let sessions = sessions(vec![Thread {
            unseen,
            ..thread(1, status)
        }]);

        // When rendering the sidebar.
        let place = line(&draw(&sessions, at(1000), 8), 4);

        // Then its second line ends with the status's word.
        assert!(
            place.trim_end().ends_with(&format!(" {word}")),
            "line was '{place}'"
        );
    }

    #[rstest::rstest]
    fn node_third_line_shows_the_branch() {
        // Given a thread on `main`.
        let sessions = sessions(vec![Thread {
            branch: Some("main".to_owned()),
            ..thread(1, ThreadStatus::Idle)
        }]);

        // When rendering the sidebar.
        let footer = line(&draw(&sessions, at(1000), 8), 5);

        // Then its third line is the last guide and the branch.
        assert!(
            footer.starts_with(&format!(" └╴{BRANCH} main ")),
            "line was '{footer}'"
        );
    }

    #[rstest::rstest]
    fn node_third_line_ends_with_the_claude_logo() {
        // Given a thread.
        let sessions = sessions(vec![thread(1, ThreadStatus::Idle)]);

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 8);

        // Then its third line ends with ✳ in Claude orange, before the blank
        // column.
        assert_eq!(
            glyph(&buf, 30, 5),
            Some((CLAUDE_LOGO.to_owned(), CLAUDE)),
            "line was '{}'",
            line(&buf, 5)
        );
    }

    #[rstest::rstest]
    fn draft_first_line_is_new_thread_marked_draft() {
        // Given a draft in orb.
        let sessions = draft(DraftWorkspace::Local, Some("dev"));

        // When rendering the sidebar.
        let heading = line(&draw(&sessions, at(1000), 8), 3);

        // Then its first line is the pencil and `New thread`, marked `draft`.
        assert!(
            heading.starts_with(&format!(" {PENCIL} New thread "))
                && heading.trim_end().ends_with(" draft"),
            "line was '{heading}'"
        );
    }

    #[rstest::rstest]
    fn draft_pencil_is_yellow() {
        // Given a draft in orb.
        let sessions = draft(DraftWorkspace::Local, Some("dev"));

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 8);

        // Then the pencil is yellow.
        assert_eq!(
            glyph(&buf, 1, 3),
            Some((PENCIL.to_owned(), YELLOW)),
            "the draft's pencil"
        );
    }

    #[rstest::rstest]
    fn draft_second_line_shows_the_project() {
        // Given a draft in orb.
        let sessions = draft(DraftWorkspace::Local, Some("dev"));

        // When rendering the sidebar.
        let place = line(&draw(&sessions, at(1000), 8), 4);

        // Then its second line is a guide and orb's folder and name.
        assert_eq!(
            place.trim_end(),
            format!(" ├╴{FOLDER} orb"),
            "the draft's second line"
        );
    }

    #[rstest::rstest]
    #[case(DraftWorkspace::Local, Some("dev"), "local · dev")]
    #[case(DraftWorkspace::NewWorktree, None, "new worktree")]
    #[case(
        DraftWorkspace::Existing("/Users/me/.orb/worktrees/orb/orb-1a2b".into()),
        Some("dev"),
        "worktree · dev"
    )]
    fn draft_third_line_shows_its_workspace(
        #[case] workspace: DraftWorkspace,
        #[case] branch: Option<&str>,
        #[case] expected: &str,
    ) {
        // Given a draft in `workspace` on `branch`.
        let sessions = draft(workspace, branch);

        // When rendering the sidebar.
        let footer = line(&draw(&sessions, at(1000), 8), 5);

        // Then its third line is the last guide, the branch glyph and where
        // it will start.
        assert_eq!(
            footer.trim_end(),
            format!(" └╴{BRANCH} {expected}"),
            "the draft's third line"
        );
    }

    /// The line naming the Settled shelf.
    fn shelf_line(buf: &Buffer) -> String {
        lines(buf)
            .into_iter()
            .find(|line| line.contains(" Settled"))
            .unwrap_or_default()
    }

    #[rstest::rstest]
    #[case::closed(false, FOLDER)]
    #[case::open(true, FOLDER_OPEN)]
    fn shelf_header_is_a_folder_open_while_the_shelf_is(
        #[case] shelf_open: bool,
        #[case] folder: &str,
    ) {
        // Given a settled thread, with the shelf open or closed.
        let sessions = Sessions {
            shelf_open,
            ..sessions(vec![settled(1, 10)])
        };

        // When rendering the sidebar.
        let header = shelf_line(&draw(&sessions, at(1000), 8));

        // Then the header is the folder and `Settled`.
        assert!(
            header.starts_with(&format!(" {folder} Settled ")),
            "line was '{header}'"
        );
    }

    #[rstest::rstest]
    fn shelf_header_counts_settled_threads() {
        // Given one active and two settled threads, with the shelf closed.
        let sessions = sessions(vec![
            thread(1, ThreadStatus::Idle),
            settled(2, 10),
            settled(3, 20),
        ]);

        // When rendering the sidebar.
        let header = shelf_line(&draw(&sessions, at(1000), 10));

        // Then the header counts both settled threads.
        assert!(header.trim_end().ends_with(" 2"), "line was '{header}'");
    }

    #[rstest::rstest]
    fn short_list_keeps_the_shelf_header_on_the_bottom_line() {
        // Given one active and one settled thread on a 10-line sidebar.
        let sessions = sessions(vec![thread(1, ThreadStatus::Idle), settled(2, 10)]);

        // When rendering the sidebar.
        let bottom = line(&draw(&sessions, at(1000), 10), 9);

        // Then the shelf header is on the last line.
        assert!(bottom.contains(" Settled "), "line was '{bottom}'");
    }

    /// orb's two settled threads, 2 settled at 10 s and 3 at 20 s, with the
    /// shelf open.
    fn open_shelf() -> Sessions {
        Sessions {
            shelf_open: true,
            ..sessions(vec![settled(2, 10), settled(3, 20)])
        }
    }

    /// The lines after the shelf header.
    fn settled_lines(buf: &Buffer) -> Vec<String> {
        lines(buf)
            .into_iter()
            .skip_while(|line| !line.contains(" Settled "))
            .skip(1)
            .collect()
    }

    #[rstest::rstest]
    fn open_shelf_lists_settled_threads_newest_first() {
        // Given two settled threads, with the shelf open.
        let sessions = open_shelf();

        // When rendering the sidebar.
        let settled = settled_lines(&draw(&sessions, at(1000), 6));

        // Then the header is followed by one line per settled thread, newest
        // settle first.
        assert!(
            matches!(
                settled.as_slice(),
                [first, second] if first.contains("Thread 3") && second.contains("Thread 2")
            ),
            "lines were {settled:#?}"
        );
    }

    #[rstest::rstest]
    fn settled_rows_hang_off_the_shelf_on_tree_guides() {
        // Given two settled threads, with the shelf open.
        let sessions = open_shelf();

        // When rendering the sidebar.
        let settled = settled_lines(&draw(&sessions, at(1000), 6));

        // Then the first row's guide branches and the last one's ends.
        let guides: Vec<_> = settled
            .iter()
            .map(|line| {
                [GUIDE, LAST_GUIDE]
                    .into_iter()
                    .find(|guide| line.starts_with(guide))
            })
            .collect();
        assert_eq!(
            guides,
            [Some(GUIDE), Some(LAST_GUIDE)],
            "lines were {settled:#?}"
        );
    }

    #[rstest::rstest]
    fn settled_row_ends_with_the_time_since_it_settled() {
        // Given a thread settled 5 minutes before now, with the shelf open.
        let sessions = Sessions {
            shelf_open: true,
            ..sessions(vec![settled(2, 700)])
        };

        // When rendering the sidebar.
        let settled = settled_lines(&draw(&sessions, at(1000), 5));

        // Then its row ends with the time since.
        assert!(
            settled
                .first()
                .is_some_and(|row| row.trim_end().ends_with(" 5m")),
            "lines were {settled:#?}"
        );
    }

    #[rstest::rstest]
    #[case::failed(ThreadStatus::Failed, FAILED_ICON, RED)]
    #[case::gone(ThreadStatus::Gone, GONE_ICON, RED)]
    #[case::stopped(ThreadStatus::Stopped, COMPLETED_ICON, DARK3)]
    #[case::idle(ThreadStatus::Idle, COMPLETED_ICON, DARK3)]
    fn settled_row_icon_marks_only_failures(
        #[case] status: ThreadStatus,
        #[case] icon: &str,
        #[case] colour: Color,
    ) {
        // Given a thread settled in `status`, with the shelf open.
        let sessions = Sessions {
            shelf_open: true,
            ..sessions(vec![Thread {
                settled_at: Some(at(10)),
                ..thread(1, status)
            }])
        };

        // When rendering a 5-line sidebar, its row on the last line.
        let buf = draw(&sessions, at(1000), 5);

        // Then its icon follows the guide.
        assert_eq!(
            glyph(&buf, 3, 4),
            Some((icon.to_owned(), colour)),
            "the settled icon for {status:?}"
        );
    }

    #[rstest::rstest]
    fn selected_node_first_line_has_the_cursorline() {
        // Given a selected thread.
        let sessions = select(sessions(vec![thread(1, ThreadStatus::Idle)]), 1);

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 8);

        // Then its first line has the selection background across the list.
        let backgrounds: Vec<_> = (0..31)
            .map(|x| buf.cell((x, 3)).map(|cell| cell.bg))
            .collect();
        assert_eq!(backgrounds, vec![Some(VISUAL); 31], "the node's first line");
    }

    #[rstest::rstest]
    fn selected_node_other_lines_keep_the_sidebar_background() {
        // Given a selected thread.
        let sessions = select(sessions(vec![thread(1, ThreadStatus::Idle)]), 1);

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 8);

        // Then its second and third lines keep the dark background.
        let backgrounds: Vec<_> = [4, 5]
            .into_iter()
            .map(|y| buf.cell((0, y)).map(|cell| cell.bg))
            .collect();
        assert_eq!(
            backgrounds,
            vec![Some(BG_DARK); 2],
            "the node's other lines"
        );
    }

    #[rstest::rstest]
    fn render_returns_the_selected_nodes_first_line() {
        // Given a selected thread.
        let sessions = select(sessions(vec![thread(1, ThreadStatus::Idle)]), 1);

        // When rendering a 10-line sidebar.
        let (_, selected_y, _) = render_sized(&sessions, at(1000), 32, 10);

        // Then it reports the line under the input box.
        assert_eq!(selected_y, Some(3), "the selected node's first line");
    }

    #[rstest::rstest]
    fn render_reports_the_list_height_and_each_rows_height() {
        // Given two threads and a collapsed shelf.
        let sessions = sessions(vec![
            thread(1, ThreadStatus::Idle),
            thread(2, ThreadStatus::Idle),
            settled(3, 10),
        ]);

        // When rendering a 10-line sidebar.
        let (_, _, layout) = render_sized(&sessions, at(1000), 32, 10);

        // Then it reports the 7 lines under the input box and each row's
        // height.
        assert_eq!(
            layout,
            SidebarLayout {
                rows: 7,
                heights: vec![3, 3, 1],
            },
            "the layout should be the list height and one height per row"
        );
    }

    #[rstest::rstest]
    fn sidebar_scrolls_to_show_the_whole_selected_node() {
        // Given three threads on an 8-line sidebar, with the last one (thread
        // 1) selected.
        let sessions = select(
            sessions(vec![
                thread(1, ThreadStatus::Idle),
                thread(2, ThreadStatus::Idle),
                thread(3, ThreadStatus::Idle),
            ]),
            1,
        );

        // When rendering the sidebar.
        let (_, selected_y, _) = render_sized(&sessions, at(1000), 32, 8);

        // Then its node fills the last three lines.
        assert_eq!(selected_y, Some(5), "the selected node's first line");
    }

    #[rstest::rstest]
    #[case(24, 2)]
    #[case(2, 8)]
    #[case(80, 3)]
    #[case(0, 0)]
    fn sidebar_draws_at_any_size(#[case] width: u16, #[case] height: u16) {
        // Given a draft, a pinned thread, an active one and an open shelf,
        // filtered to orb, with the settled thread selected.
        let sessions = {
            let mut sessions = draft(DraftWorkspace::NewWorktree, Some("main"));
            if let Some(project) = sessions.projects.first_mut() {
                project.threads = vec![
                    Thread {
                        pinned_at: Some(at(5)),
                        ..thread(1, ThreadStatus::Working)
                    },
                    thread(2, ThreadStatus::NeedsApproval),
                    settled(3, 10),
                ];
            }
            Sessions {
                shelf_open: true,
                filter: Some(ProjectId(1)),
                ..select(sessions, 3)
            }
        };

        // When rendering it at `width`x`height`.
        let (_, _, layout) = render_sized(&sessions, at(1000), width, height);

        // Then the list takes what the input box leaves.
        assert_eq!(
            layout.rows,
            height.saturating_sub(3),
            "the list's height at {width}x{height}"
        );
    }
}
