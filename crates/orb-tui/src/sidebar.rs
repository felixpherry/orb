//! The sidebar: one list of orb's sessions across projects, drawn
//! like LazyVim's file explorer (snacks.nvim) in tokyonight-moon.
//!
//! An input box heads it: `Sessions`, with an `i` badge lit while the
//! search has the keys, the filtered project after the `>` prompt, and how
//! many sessions are listed out of all of them. Below it, pinned sessions
//! come first, then active ones, each a card: its most urgent agent's
//! status icon and its title, then the project and status, then the branch
//! (a Research or Learn session's folder as a `~/…` path), then one row per
//! agent pane under the card, with its status, name or title and harness
//! mark. A card with agent rows ends its last line with `⌄`; folded, it
//! hides them and shows `›` after one status icon per agent. The selected
//! row's first line is highlighted. Settled sessions fold into a shelf at
//! the bottom, drawn as one-line rows while it's open. The sidebar scrolls to keep the whole selected row in view,
//! unless the wheel scrolled it while the keys are elsewhere.
//!
//! While the user searches, the typed text follows the prompt, and only
//! the sessions whose title or an agent's title matches it are
//! listed, settled ones included and folded cards open, with the matched
//! characters highlighted as in the pickers.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

use orb_domain::feat::harness::HarnessInfo;
use orb_domain::feat::layout::state::Layouts;
use orb_domain::feat::sessions::state::{
    FolderKind, NEW_THREAD, Project, Session, SessionId, SessionKind, Sessions, SidebarItem,
    SidebarRow, Thread, ThreadStatus, most_urgent,
};
use orb_domain::feat::sidebar::state::SidebarLayout;
use orb_domain::tilde;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Widget};
use unicode_segmentation::UnicodeSegmentation;

use crate::mouse::HitMap;
use crate::picker::{highlight, visible};

/// Whether `thread`'s session is among the `attached` ones.
pub(crate) fn is_attached(attached: &HashSet<SessionId>, thread: &Thread) -> bool {
    thread
        .session()
        .is_some_and(|session| attached.contains(&session))
}

/// A harness's mark in orange, else the model chip in blue.
pub(crate) fn mark(icon: Option<&str>) -> (&str, Color) {
    match icon {
        Some(icon) => (icon, LOGO),
        None => (CHIP, BLUE1),
    }
}

/// How far the sidebar is scrolled, kept between frames.
#[derive(Debug, Default)]
pub(crate) struct SidebarScroll {
    /// The list line drawn on the sidebar's top row.
    offset: u16,
    /// The selection the wheel scrolled the view on, while the view is
    /// free of it.
    #[expect(
        clippy::option_option,
        reason = "free on no selection differs from not free"
    )]
    free: Option<Option<SidebarItem>>,
}

impl SidebarScroll {
    /// Scrolls the view `lines` down (up when negative) without following
    /// the selection, `cursor`, until it moves or `release` is called.
    pub(crate) fn scroll_free(&mut self, lines: i16, cursor: Option<SidebarItem>) {
        self.offset = self.offset.saturating_add_signed(lines);
        self.free = Some(cursor);
    }

    /// Brings the view back to the selection on the next draw.
    pub(crate) fn release(&mut self) {
        self.free = None;
    }

    /// Whether this draw leaves the view where the wheel put it: only while
    /// the selection is still `cursor`, the one it was scrolled on.
    fn stays_free(&mut self, cursor: Option<SidebarItem>) -> bool {
        let free = self.free == Some(cursor);
        if !free {
            self.free = None;
        }
        free
    }
}

/// Draws the sidebar into `area`, a blank column short of its right edge: the
/// input box, then the list, scrolled so the cursor's row is in view. Returns
/// the y of the selected row's first line when it's on screen, the list's
/// layout, and the search text's cursor while there is a search. An idle
/// agent of a session in `attached` shows a filled circle; an agent row reads
/// its pane's name from `layouts` and ends with its harness's mark from
/// `harnesses`.
#[expect(
    clippy::too_many_arguments,
    reason = "the sidebar's inputs plus the scroll and hit map it updates"
)]
pub(crate) fn render(
    sessions: &Sessions,
    attached: &HashSet<SessionId>,
    harnesses: &[HarnessInfo],
    layouts: &Layouts,
    home: &Path,
    now: SystemTime,
    area: Rect,
    buf: &mut Buffer,
    scroll: &mut SidebarScroll,
    hits: &mut HitMap,
) -> (Option<u16>, SidebarLayout, Option<Position>) {
    buf.set_style(area, Style::new().bg(BG_DARK).fg(FG));
    let area = Rect {
        width: area.width.saturating_sub(1),
        ..area
    };
    let [input, list] = Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(area);
    hits.record_sidebar_input(input);
    let rows = sessions.sidebar();
    let search_cursor = render_input(sessions, &rows, input, buf, hits);
    let layout = SidebarLayout {
        rows: list.height,
        heights: rows.iter().map(height).collect(),
    };
    let selected_y = render_list(
        sessions, attached, harnesses, layouts, home, rows, now, list, buf, scroll, hits,
    );
    (selected_y, layout, search_cursor)
}

/// The rounded box at the top: its title and search badge, the prompt, and the
/// count against its right edge. Returns the search text's cursor while
/// there is a search. Records the search text's line in `hits` while there
/// is a search.
fn render_input(
    sessions: &Sessions,
    rows: &[SidebarRow<'_>],
    area: Rect,
    buf: &mut Buffer,
    hits: &mut HitMap,
) -> Option<Position> {
    let badge = if sessions.search.is_some() {
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
    let count = Line::from(span(count(sessions, rows), COMMENT));
    let (prompt, cursor) = prompt(
        sessions,
        usize::from(inner.width).saturating_sub(usize::from(width(&count)) + 1),
        inner,
        hits,
    );
    render_split(prompt, count, inner, buf);
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
/// query. A search text too long for the `room` columns shows its end. Also
/// returns the column of the search text's cursor. Records the search text's
/// line `line` in `hits`.
fn prompt<'a>(
    sessions: &'a Sessions,
    room: usize,
    line: Rect,
    hits: &mut HitMap,
) -> (Line<'a>, Option<u16>) {
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
    let prefix_width: usize = spans.iter().map(Span::width).sum();
    let room = room.saturating_sub(prefix_width + 1);
    let (shown, before) = visible(search.input.text(), search.input.cursor(), room);
    hits.record_text(
        line,
        prefix_width,
        search.input.text(),
        search.input.cursor(),
        room,
    );
    spans.push(span(shown, FG));
    (
        Line::from(spans),
        Some(u16::try_from(prefix_width + before).unwrap_or(u16::MAX)),
    )
}

/// `shown/total`, like snacks' match count: the sessions listed, out of
/// every session not being deleted. Agent rows aren't counted.
fn count(sessions: &Sessions, rows: &[SidebarRow<'_>]) -> String {
    let shown = rows
        .iter()
        .filter(|row| {
            !matches!(
                row,
                SidebarRow::ShelfHeader { .. } | SidebarRow::Agent { .. }
            )
        })
        .count();
    let live = sessions
        .sessions
        .iter()
        .filter(|session| !sessions.deleting.contains(&session.id))
        .count();
    format!("{shown}/{live}")
}

/// Draws the rows into `area`, scrolled so the selected one is whole on
/// screen, with the shelf header held on the bottom row while its real line
/// is below the view, and records each row's visible lines in `hits`.
/// Returns the y of the selected row's first line when it's there.
#[expect(
    clippy::too_many_arguments,
    reason = "the list's inputs plus the scroll and hit map it updates"
)]
fn render_list(
    sessions: &Sessions,
    attached: &HashSet<SessionId>,
    harnesses: &[HarnessInfo],
    layouts: &Layouts,
    home: &Path,
    rows: Vec<SidebarRow<'_>>,
    now: SystemTime,
    area: Rect,
    buf: &mut Buffer,
    scroll: &mut SidebarScroll,
    hits: &mut HitMap,
) -> Option<u16> {
    let (placed, total) = place(rows, area.height);
    let cursor = sessions.cursor_row();
    let selected = placed
        .iter()
        .find(|(row, _)| Some(row.item()) == cursor)
        .map(|(row, top)| (*top, height(row)));
    let free = scroll.stays_free(sessions.cursor);
    if let (false, Some((top, rows))) = (free, selected) {
        scroll.offset = scroll
            .offset
            .min(top)
            .max(top.saturating_add(rows).saturating_sub(area.height));
    }
    scroll.offset = scroll.offset.min(total.saturating_sub(area.height));
    // The shelf header while its real line is below the view: it then takes
    // the bottom row, which the selected row must stay clear of.
    let below = |offset: u16| {
        placed.iter().find_map(|(row, top)| match row {
            SidebarRow::ShelfHeader { count, open }
                if area.height >= 2 && *top >= offset.saturating_add(area.height) =>
            {
                Some((*count, *open))
            }
            _ => None,
        })
    };
    if let (false, Some(_), Some((top, rows))) = (free, below(scroll.offset), selected) {
        scroll.offset = scroll
            .offset
            .max(top.saturating_add(rows).saturating_sub(area.height - 1));
    }
    let sticky = below(scroll.offset);
    record_rows(&placed, scroll.offset, sticky.is_some(), area, hits);
    // The whole list, then the lines in view.
    let list = {
        let mut list = Buffer::empty(Rect::new(area.x, 0, area.width, total));
        list.set_style(list.area, Style::new().bg(BG_DARK).fg(FG));
        for (index, (row, top)) in placed.iter().enumerate() {
            let row_area = Rect::new(area.x, *top, area.width, height(row));
            if Some(row.item()) == cursor {
                list.set_style(
                    Rect {
                        height: 1,
                        ..row_area
                    },
                    Style::new().bg(VISUAL),
                );
            }
            let ends_shelf = !placed
                .get(index + 1)
                .is_some_and(|(row, _)| matches!(row, SidebarRow::Settled { .. }));
            render_row(
                sessions, attached, harnesses, layouts, home, row, ends_shelf, now, row_area,
                &mut list,
            );
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
    if let Some((count, open)) = sticky {
        let row = Rect {
            y: area.bottom() - 1,
            height: 1,
            ..area
        };
        Clear.render(row, buf);
        buf.set_style(row, Style::new().bg(BG_DARK).fg(FG));
        render_shelf_header(count, open, row, buf);
    }
    selected
        .map(|(top, _)| top)
        .filter(|top| (scroll.offset..scroll.offset.saturating_add(area.height)).contains(top))
        .map(|top| area.y + top - scroll.offset)
}

/// Records in `hits` the lines of `area` each placed row shows from list
/// line `offset` down, and the bottom line as the shelf while the header is
/// `sticky` there. Blank gap lines map to nothing.
fn record_rows(
    placed: &[(SidebarRow<'_>, u16)],
    offset: u16,
    sticky: bool,
    area: Rect,
    hits: &mut HitMap,
) {
    let shown = area.height - u16::from(sticky);
    for (row, top) in placed {
        let start = (*top).max(offset);
        let end = top
            .saturating_add(height(row))
            .min(offset.saturating_add(shown));
        if start < end {
            hits.record_row(
                Rect::new(area.x, area.y + start - offset, area.width, end - start),
                row.item(),
            );
        }
    }
    if sticky {
        hits.record_row(
            Rect {
                y: area.bottom() - 1,
                height: 1,
                ..area
            },
            SidebarItem::SettledShelf,
        );
    }
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

/// How many lines a row takes: a session's card 3, any other row 1.
fn height(row: &SidebarRow<'_>) -> u16 {
    match row {
        SidebarRow::Card { .. } => 3,
        SidebarRow::ShelfHeader { .. } | SidebarRow::Settled { .. } | SidebarRow::Agent { .. } => 1,
    }
}

/// One row; `ends_shelf` says no settled session follows it. Titles show
/// where the search matched them.
#[expect(
    clippy::too_many_arguments,
    reason = "the row plus the frame inputs every node takes"
)]
fn render_row(
    sessions: &Sessions,
    attached: &HashSet<SessionId>,
    harnesses: &[HarnessInfo],
    layouts: &Layouts,
    home: &Path,
    row: &SidebarRow<'_>,
    ends_shelf: bool,
    now: SystemTime,
    area: Rect,
    buf: &mut Buffer,
) {
    match row {
        SidebarRow::Card { project, session } => render_card(
            sessions,
            project,
            session,
            attached.contains(&session.id),
            home,
            now,
            area,
            buf,
        ),
        SidebarRow::ShelfHeader { count, open } => render_shelf_header(*count, *open, area, buf),
        SidebarRow::Settled { session, .. } => {
            render_settled(sessions, session, ends_shelf, now, area, buf);
        }
        SidebarRow::Agent {
            session,
            thread,
            pane,
            last,
        } => {
            let label = layouts
                .entry(*pane)
                .and_then(|entry| entry.name.as_deref())
                .unwrap_or_else(|| title_of(thread));
            render_agent(
                sessions,
                harnesses,
                thread,
                label,
                *last,
                attached.contains(&session.id),
                now,
                area,
                buf,
            );
        }
    }
}

/// A session's card: its most urgent agent's status icon (the idle circle
/// without one, filled while `attached`), its title, pin and time; the
/// project and the status word; the branch, or for a Research or Learn
/// session its folder as a `~/…` path and its kind icon. With agent rows,
/// the last line ends with `⌄` while they show and `›` while folded, after
/// one status icon per agent (at most 8, then `+N`).
#[expect(
    clippy::too_many_arguments,
    reason = "the card's parts plus the frame inputs every node takes"
)]
fn render_card(
    sessions: &Sessions,
    project: &Project,
    session: &Session,
    attached: bool,
    home: &Path,
    now: SystemTime,
    area: Rect,
    buf: &mut Buffer,
) {
    let agents = sessions.agents(session.id);
    let lines = Layout::vertical(vec![Constraint::Length(1); 3]).split(area);
    let line = |index: usize| lines.get(index).copied().unwrap_or_default();
    let urgent = most_urgent(agents.iter().copied());
    let (glyph, word, colour) = session_status(&agents, attached, now);
    let pin = if session.pinned_at.is_some() { PIN } else { "" };
    let time = match urgent {
        Some(thread) if thread.status == ThreadStatus::Working => when(thread, now),
        _ => ago_label(since(now, session.last_activity_at)),
    };
    let title = sessions.title(session);
    let matched = sessions.title_matches(&title).unwrap_or_default();
    render_split(
        Line::from(
            [span(format!(" {glyph} "), colour)]
                .into_iter()
                .chain(highlight(&title, &matched, |_| FG))
                .collect::<Vec<_>>(),
        ),
        Line::from(vec![span(format!("{pin} "), ORANGE), span(time, COMMENT)]),
        line(0),
        buf,
    );
    render_split(
        project_line(project),
        word.map(|word| Line::from(span(word, colour)))
            .unwrap_or_default(),
        line(1),
        buf,
    );
    let rows = sessions.agent_rows(session.id);
    let folded = sessions.hides_agents(session.id);
    let guide = if rows.is_empty() || folded {
        LAST_GUIDE
    } else {
        GUIDE
    };
    let (place, kind) = match session_look(session.kind) {
        Some(look) => (
            format!("{FOLDER} {}", tilde(&session.dir, home)),
            Some(look),
        ),
        None => {
            let branch = agents
                .first()
                .and_then(|thread| thread.branch.as_deref())
                .or(session.branch.as_deref())
                .unwrap_or("—");
            (format!("{BRANCH} {branch}"), None)
        }
    };
    let icons: Vec<_> = rows
        .iter()
        .filter(|_| folded)
        .map(|(thread, _)| status(thread, attached, now))
        .collect();
    let rest = icons.len().saturating_sub(AGENT_ICONS);
    let chevron = match (rows.is_empty(), folded) {
        (true, _) => None,
        (false, true) => Some(FOLD_CLOSED),
        (false, false) => Some(FOLD_OPEN),
    };
    render_split(
        Line::from(vec![span(guide, GUTTER), span(place, COMMENT)]),
        Line::from(
            kind.map(|(icon, colour)| span(format!("{icon} "), colour))
                .into_iter()
                .chain(
                    icons
                        .into_iter()
                        .take(AGENT_ICONS)
                        .map(|(glyph, _, colour)| span(format!("{glyph} "), colour)),
                )
                .chain((rest > 0).then(|| span(format!("+{rest} "), COMMENT)))
                .chain(chevron.map(|chevron| span(chevron, DARK3)))
                .collect::<Vec<_>>(),
        ),
        line(2),
        buf,
    );
}

/// An agent pane's row under its card: the tree guide (`└╴` on the last),
/// its status icon (the idle circle filled while `attached`), `label` with
/// where the search matched it, and its harness mark on the right.
#[expect(
    clippy::too_many_arguments,
    reason = "the row's parts plus the frame inputs every node takes"
)]
fn render_agent(
    sessions: &Sessions,
    harnesses: &[HarnessInfo],
    thread: &Thread,
    label: &str,
    last: bool,
    attached: bool,
    now: SystemTime,
    area: Rect,
    buf: &mut Buffer,
) {
    let guide = if last { LAST_GUIDE } else { GUIDE };
    let (glyph, _, colour) = status(thread, attached, now);
    let matched = sessions.title_matches(label).unwrap_or_default();
    let mark = harnesses
        .iter()
        .find(|info| info.id == thread.harness)
        .and_then(|info| info.icon.clone());
    render_split(
        Line::from(
            [span(guide, GUTTER), span(format!("{glyph} "), colour)]
                .into_iter()
                .chain(highlight(label, &matched, |_| FG))
                .collect::<Vec<_>>(),
        ),
        Line::from(mark.map(|mark| span(mark, LOGO)).unwrap_or_default()),
        area,
        buf,
    );
}

/// A folder kind's icon and colour: Research `nf-fa-flask` blue2, Learn
/// `nf-fa-book` purple.
pub(crate) fn kind_look(kind: FolderKind) -> (&'static str, Color) {
    match kind {
        FolderKind::Research => ("\u{f0c3}", BLUE2),
        FolderKind::Learn => ("\u{f02d}", PURPLE),
    }
}

/// A session kind's icon and colour, for the kinds that own their folder
/// and show it: Research `nf-fa-flask` blue2, Learn `nf-fa-book` purple.
fn session_look(kind: SessionKind) -> Option<(&'static str, Color)> {
    match kind {
        SessionKind::Research => Some(kind_look(FolderKind::Research)),
        SessionKind::Learn => Some(kind_look(FolderKind::Learn)),
        SessionKind::Plain | SessionKind::Incognito => None,
    }
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

/// The Settled shelf's folder, open or closed, and how many sessions it
/// holds.
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

/// A settled session's row under the shelf's folder: its icon (a dim check
/// unless its most urgent agent failed or is gone), title, and the time since
/// it settled.
fn render_settled(
    sessions: &Sessions,
    session: &Session,
    ends_shelf: bool,
    now: SystemTime,
    area: Rect,
    buf: &mut Buffer,
) {
    let guide = if ends_shelf { LAST_GUIDE } else { GUIDE };
    let agents = sessions.agents(session.id);
    let (glyph, colour) = match most_urgent(agents.iter().copied()) {
        Some(thread) if matches!(thread.status, ThreadStatus::Failed | ThreadStatus::Gone) => {
            let (glyph, _, colour) = status(thread, false, now);
            (glyph, colour)
        }
        _ => (COMPLETED_ICON, DARK3),
    };
    let title = sessions.title(session);
    let matched = sessions.title_matches(&title).unwrap_or_default();
    let ago = ago_label(since(
        now,
        session.settled_at.unwrap_or(session.last_activity_at),
    ));
    render_split(
        Line::from(
            [span(guide, GUTTER), span(format!("{glyph} "), colour)]
                .into_iter()
                .chain(highlight(&title, &matched, |_| COMMENT))
                .collect::<Vec<_>>(),
        ),
        Line::from(span(ago, DARK3)),
        area,
        buf,
    );
}

/// A session's status from its `agents`: its most urgent agent's, else the
/// idle circle, filled while `attached`.
pub(crate) fn session_status(
    agents: &[&Thread],
    attached: bool,
    now: SystemTime,
) -> (&'static str, Option<&'static str>, Color) {
    match most_urgent(agents.iter().copied()) {
        Some(thread) => status(thread, attached, now),
        None if attached => (ATTACHED_ICON, None, FG),
        None => (IDLE_ICON, None, DARK3),
    }
}

/// A thread's status as its icon, its short word (none while idle), and
/// their colour; `attached` fills the idle circle.
pub(crate) fn status(
    thread: &Thread,
    attached: bool,
    now: SystemTime,
) -> (&'static str, Option<&'static str>, Color) {
    match thread.status {
        ThreadStatus::NeedsApproval => (APPROVAL_ICON, Some("approval"), YELLOW),
        ThreadStatus::NeedsInput => (INPUT_ICON, Some("input"), MAGENTA),
        ThreadStatus::Working => (spinner(thread, now), Some("working"), BLUE),
        ThreadStatus::Failed => (FAILED_ICON, Some("failed"), RED),
        ThreadStatus::Gone => (GONE_ICON, Some("gone"), RED),
        ThreadStatus::Idle if thread.unseen => (COMPLETED_ICON, Some("done"), GREEN),
        ThreadStatus::Stopped => (STOPPED_ICON, Some("stopped"), COMMENT),
        ThreadStatus::Idle | ThreadStatus::Unknown if attached => (ATTACHED_ICON, None, FG),
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
/// ended.
fn when(thread: &Thread, now: SystemTime) -> String {
    match (thread.status, thread.turn_started_at) {
        (ThreadStatus::Working, Some(at)) => working_label(since(now, at)),
        _ => ago_label(since(now, thread.last_activity_at)),
    }
}

/// A thread's title, else `New thread`.
fn title_of(thread: &Thread) -> &str {
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
/// Titles, the filtered project's name and an attached idle thread's
/// circle; the picker's input and rows, and the rename box's text (`fg`).
pub(crate) const FG: Color = Color::Rgb(0xc8, 0xd3, 0xf5);
/// Project names, and the keys in the picker's hints (`fg_dark`).
pub(crate) const FG_DARK: Color = Color::Rgb(0x82, 0x8b, 0xb8);
/// Times, branches, the count, settled titles and the stopped icon; the
/// picker's hint labels, headings and empty-list text (`comment`).
pub(crate) const COMMENT: Color = Color::Rgb(0x63, 0x6d, 0xa6);
/// The idle icon, the unlit shelf badge and settled marks; the
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
/// The mode line's PANE (`green1`, lualine's terminal mode).
pub(crate) const GREEN1: Color = Color::Rgb(0x4f, 0xd6, 0xbe);
/// A Research session's kind icon (`blue2`).
pub(crate) const BLUE2: Color = Color::Rgb(0x0d, 0xb9, 0xd7);
/// A Learn session's kind icon (`purple`).
pub(crate) const PURPLE: Color = Color::Rgb(0xfc, 0xa7, 0xea);
/// Needing approval; the picker's `worktree` branch badge; the rename box
/// (`yellow`).
pub(crate) const YELLOW: Color = Color::Rgb(0xff, 0xc7, 0x77);
/// The input box and the pin; the rule under the picker's input and its
/// git icon (`orange`).
pub(crate) const ORANGE: Color = Color::Rgb(0xff, 0x96, 0x6c);
/// Failed and gone, the mode line's error, and the which-key popup's lazygit
/// icon (`red`).
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
/// A harness's mark (orange).
pub(crate) const LOGO: Color = Color::Rgb(0xd9, 0x77, 0x57);

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
pub(crate) const COMPLETED_ICON: &str = "\u{f058}";
/// Stopped (`nf-fa-stop`).
const STOPPED_ICON: &str = "\u{f04d}";
/// Idle, or a status orb doesn't know (`nf-fa-circle_o`).
const IDLE_ICON: &str = "\u{f10c}";
/// Idle while orb is attached (`nf-fa-circle`).
const ATTACHED_ICON: &str = "\u{f111}";
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
/// A model, before a harness's models when it has no mark
/// (`nf-fa-microchip`).
pub(crate) const CHIP: &str = "\u{f2db}";
/// The most status icons a folded card shows before counting the rest.
const AGENT_ICONS: usize = 8;
/// A card whose agent rows show (`nf-oct-chevron_down`).
const FOLD_OPEN: &str = "\u{f47c}";
/// A folded card (`nf-oct-chevron_right`).
const FOLD_CLOSED: &str = "\u{f460}";
/// The tree guide before a node's middle line, a card's agent row and a
/// settled row.
const GUIDE: &str = " ├╴";
/// The tree guide before a node's last line, a card's last agent row and the
/// last settled row.
const LAST_GUIDE: &str = " └╴";

#[cfg(test)]
mod tests {
    use orb_domain::feat::harness::claude::info;
    use orb_domain::feat::harness::{HarnessId, HarnessInfo};
    use orb_domain::feat::layout::state::{Layouts, PaneEntry, SessionLayout};
    use orb_domain::feat::zmx::zmx_service::ZmxSession;
    use std::collections::HashSet;
    use std::path::Path;
    use std::time::{Duration, Instant, SystemTime};

    use orb_domain::TextInput;
    use orb_domain::feat::sessions::state::{
        PaneId, PaneLaunch, Project, ProjectId, ProjectKind, Search, SessionId, SessionKind,
        Sessions, SidebarItem, Thread, ThreadId, ThreadStatus,
    };
    use orb_domain::feat::sidebar::state::SidebarLayout;
    use orb_domain::{Focus, Intent};
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::{Position, Rect};
    use ratatui::style::{Color, Modifier};

    use super::{
        APPROVAL_ICON, ATTACHED_ICON, BG_DARK, BLUE, BLUE1, BLUE2, BRANCH, COMMENT, COMPLETED_ICON,
        CYAN, DARK3, FAILED_ICON, FG, FOLD_CLOSED, FOLD_OPEN, FOLDER, FOLDER_OPEN, GONE_ICON,
        GREEN, GUIDE, GUTTER, IDLE_ICON, INPUT_ICON, LAST_GUIDE, MAGENTA, ORANGE, PIN, PURPLE, RED,
        STOPPED_ICON, SidebarScroll, VISUAL, YELLOW, ago_label, badge_colour, monogram, render,
        working_label,
    };
    use crate::mouse::{Clicks, HitMap, MouseRoute};
    use crate::test_support::sessions_for;

    /// The home directory the sidebar shortens folder paths against.
    const HOME: &str = "/Users/me";

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    /// A thread titled "Thread <id>" whose turn started at 866 s.
    fn thread(id: i64, status: ThreadStatus) -> Thread {
        Thread {
            last_session: None,
            harness: HarnessId::new("claude"),
            id: ThreadId(id),
            title: Some(format!("Thread {id}")),
            cwd: "/Users/me/dev/orb".into(),
            transcript: None,
            status,
            turn_started_at: Some(at(866)),
            pane: Some(PaneLaunch {
                pane: PaneId(id),
                session: SessionId(id),
            }),
            branch: None,
            created_at: SystemTime::UNIX_EPOCH,
            last_activity_at: SystemTime::UNIX_EPOCH,
            unseen: false,
            model: None,
        }
    }

    fn stopped(id: i64) -> Thread {
        thread(id, ThreadStatus::Stopped)
    }

    /// `sessions` with each session `(id, secs)` settled at second `secs`.
    fn settling(mut sessions: Sessions, settles: &[(i64, u64)]) -> Sessions {
        for &(id, secs) in settles {
            sessions
                .sessions
                .iter_mut()
                .filter(|session| session.id == SessionId(id))
                .for_each(|session| session.settled_at = Some(at(secs)));
        }
        sessions
    }

    fn project(id: i64, title: &str, threads: Vec<Thread>) -> Project {
        Project {
            id: ProjectId(id),
            title: title.to_owned(),
            root: format!("/Users/me/dev/{title}").into(),
            created_at: SystemTime::UNIX_EPOCH,
            removed: false,
            repo: true,
            threads,
            kind: ProjectKind::Normal,
        }
    }

    fn sessions(threads: Vec<Thread>) -> Sessions {
        let projects = vec![project(1, "orb", threads)];
        Sessions {
            sessions: sessions_for(&projects),
            projects,
            ..Sessions::default()
        }
    }

    /// orb's thread 1 and web's thread 2, filtered to orb.
    fn filtered() -> Sessions {
        let projects = vec![
            project(1, "orb", vec![thread(1, ThreadStatus::Idle)]),
            project(2, "web", vec![thread(2, ThreadStatus::Idle)]),
        ];
        Sessions {
            sessions: sessions_for(&projects),
            projects,
            filter: Some(ProjectId(1)),
            ..Sessions::default()
        }
    }

    /// `sessions` with `id`'s thread under the cursor.
    fn select(sessions: Sessions, id: i64) -> Sessions {
        Sessions {
            cursor: Some(SidebarItem::Session(SessionId(id))),
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
            &HashSet::new(),
            &[info()],
            &Layouts::default(),
            Path::new(HOME),
            now,
            buf.area,
            &mut buf,
            &mut SidebarScroll::default(),
            &mut HitMap::default(),
        );
        (buf, selected_y, layout)
    }

    /// Draws a 32-column sidebar `height` lines tall at `now`. The list's
    /// first node takes lines 3 to 5, and its right edge is column 30.
    fn draw(sessions: &Sessions, now: SystemTime, height: u16) -> Buffer {
        render_sized(sessions, now, 32, height).0
    }

    /// Draws a 32-column sidebar `height` lines tall at `now`, with `attached`'s threads attached.
    fn draw_attached(
        sessions: &Sessions,
        attached: &[i64],
        now: SystemTime,
        height: u16,
    ) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, 32, height));
        let attached = attached
            .iter()
            .copied()
            .map(SessionId)
            .collect::<HashSet<_>>();
        render(
            sessions,
            &attached,
            &[info()],
            &Layouts::default(),
            Path::new(HOME),
            now,
            buf.area,
            &mut buf,
            &mut SidebarScroll::default(),
            &mut HitMap::default(),
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

        // Then the box's top border carries the title and the search badge.
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
    #[case::shelf_open(Sessions { shelf_open: true, ..settling(sessions(vec![stopped(1)]), &[(1, 10)]) }, (DARK3, BG_DARK))]
    #[case::searching(searching(settling(sessions(vec![stopped(1)]), &[(1, 10)]), ""), (BLUE, GUTTER))]
    fn badge_lights_up_while_searching(
        #[case] sessions: Sessions,
        #[case] expected: (Color, Color),
    ) {
        // Given the Settled shelf open without a search, or a search.
        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 8);

        // Then the badge's `i` is lit only while searching.
        let badge = buf
            .cell((12, 0))
            .map(|cell| (cell.symbol().to_owned(), (cell.fg, cell.bg)));
        assert_eq!(
            badge,
            Some(("i".to_owned(), expected)),
            "the badge's colours"
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
        let sessions = settling(
            sessions(vec![thread(1, ThreadStatus::Idle), stopped(2), stopped(3)]),
            &[(2, 10), (3, 20)],
        );

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
            &HashSet::new(),
            &[info()],
            &Layouts::default(),
            Path::new(HOME),
            at(1000),
            buf.area,
            &mut buf,
            &mut SidebarScroll::default(),
            &mut HitMap::default(),
        );

        // Then the cursor is after `> `, the folder and space, `orb`, a space
        // and `thr`: 11 columns into the box.
        assert_eq!(cursor, Some(Position::new(12, 1)), "the search cursor");
    }

    #[rstest::rstest]
    fn hit_map_maps_the_search_text_to_its_graphemes() {
        // Given a filtered sidebar searching for "thr".
        let sessions = searching(filtered(), "thr");

        // When rendering the sidebar.
        let mut buf = Buffer::empty(Rect::new(0, 0, 32, 10));
        let mut hits = HitMap::default();
        let (_, _, cursor) = render(
            &sessions,
            &HashSet::new(),
            &[info()],
            &Layouts::default(),
            Path::new(HOME),
            at(1000),
            buf.area,
            &mut buf,
            &mut SidebarScroll::default(),
            &mut hits,
        );

        // Then the column before the search cursor maps to "r".
        let at = cursor.map(|cursor| Position::new(cursor.x - 1, cursor.y));
        assert_eq!(
            at.and_then(|at| hits.text_at(at)),
            Some(2),
            "the column before the cursor should be the last typed grapheme"
        );
    }

    /// The search text wider than a 32-column sidebar's input box: 40 `x`s
    /// then `end`.
    fn long_search() -> String {
        format!("{}end", "x".repeat(40))
    }

    #[rstest::rstest]
    fn long_search_text_shows_its_end_before_the_whole_count() {
        // Given one thread, searching for a text wider than the box.
        let sessions = searching(
            sessions(vec![thread(1, ThreadStatus::Idle)]),
            &long_search(),
        );

        // When rendering the sidebar.
        let prompt = line(&draw(&sessions, at(1000), 10), 1);

        // Then the line shows the text's end, then the count whole.
        assert!(
            prompt.starts_with("│> x") && prompt.trim_end().ends_with("end  0/1│"),
            "line was '{prompt}'"
        );
    }

    #[rstest::rstest]
    fn long_search_text_keeps_the_filtered_project_whole() {
        // Given the sidebar filtered to orb, searching for a text wider than
        // the box.
        let sessions = searching(filtered(), &long_search());

        // When rendering the sidebar.
        let prompt = line(&draw(&sessions, at(1000), 10), 1);

        // Then orb's folder and name start the line, and the text shows its end.
        assert!(
            prompt.starts_with(&format!("│> {FOLDER} orb x")) && prompt.contains("end "),
            "line was '{prompt}'"
        );
    }

    #[rstest::rstest]
    fn long_search_cursor_is_after_the_texts_end() {
        // Given one thread, searching for a text wider than the box.
        let sessions = searching(
            sessions(vec![thread(1, ThreadStatus::Idle)]),
            &long_search(),
        );

        // When rendering the sidebar.
        let mut buf = Buffer::empty(Rect::new(0, 0, 32, 10));
        let (_, _, cursor) = render(
            &sessions,
            &HashSet::new(),
            &[info()],
            &Layouts::default(),
            Path::new(HOME),
            at(1000),
            buf.area,
            &mut buf,
            &mut SidebarScroll::default(),
            &mut HitMap::default(),
        );

        // Then the cursor is right after the shown `end`, clear of the count.
        let before = cursor.and_then(|at| buf.cell((at.x - 1, at.y)).map(Cell::symbol));
        assert_eq!(
            (cursor, before),
            (Some(Position::new(25, 1)), Some("d")),
            "the search cursor"
        );
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
    fn search_highlights_the_matched_characters_of_a_settled_thread() {
        // Given settled "Thread 7" with the shelf closed, searching for "7".
        let sessions = searching(settling(sessions(vec![stopped(7)]), &[(7, 10)]), "7");

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
        // Given a thread in `status`, not attached.
        let sessions = sessions(vec![Thread {
            unseen,
            ..thread(1, status)
        }]);

        // When rendering the sidebar.
        let buf = draw_attached(&sessions, &[], at(1000), 8);

        // Then its icon is the status's, in the status's colour.
        assert_eq!(
            glyph(&buf, 1, 3),
            Some((icon.to_owned(), colour)),
            "the icon for {status:?}"
        );
    }

    #[rstest::rstest]
    #[case::idle(ThreadStatus::Idle)]
    #[case::unknown(ThreadStatus::Unknown)]
    fn status_icon_of_an_attached_idle_thread_is_a_filled_circle(#[case] status: ThreadStatus) {
        // Given an attached thread in `status`.
        let sessions = sessions(vec![thread(1, status)]);

        // When rendering the sidebar.
        let buf = draw_attached(&sessions, &[1], at(1000), 8);

        // Then its icon is the filled circle, in the foreground colour.
        assert_eq!(
            glyph(&buf, 1, 3),
            Some((ATTACHED_ICON.to_owned(), FG)),
            "the icon for an attached {status:?} thread"
        );
    }

    #[rstest::rstest]
    fn attached_working_thread_keeps_the_spinner() {
        // Given an attached Working thread whose turn started 134.3 s before now.
        let sessions = sessions(vec![thread(1, ThreadStatus::Working)]);

        // When rendering the sidebar.
        let buf = draw_attached(&sessions, &[1], at(1000) + Duration::from_millis(300), 8);

        // Then its icon is still the spinner's fourth frame, in blue.
        assert_eq!(
            glyph(&buf, 1, 3),
            Some(("⠸".to_owned(), BLUE)),
            "the spinner of an attached Working thread"
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
    fn untitled_agent_row_is_a_new_thread() {
        // Given a session whose agent has no title yet.
        let sessions = sessions(vec![Thread {
            title: None,
            ..thread(1, ThreadStatus::Idle)
        }]);

        // When rendering the sidebar.
        let row = line(&draw(&sessions, at(1000), 8), 6);

        // Then its agent row says "New thread".
        assert!(row.contains(" New thread"), "line was '{row}'");
    }

    #[rstest::rstest]
    fn pinned_thread_shows_an_orange_pin() {
        // Given a pinned thread.
        let mut sessions = sessions(vec![thread(1, ThreadStatus::Idle)]);
        if let Some(session) = sessions.sessions.get_mut(0) {
            session.pinned_at = Some(at(5));
        }

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

    /// Draws a 32-column sidebar 8 lines tall at 1000 s with `harnesses`
    /// registered.
    fn draw_with(sessions: &Sessions, harnesses: &[HarnessInfo]) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, 32, 8));
        render(
            sessions,
            &HashSet::new(),
            harnesses,
            &Layouts::default(),
            Path::new(HOME),
            at(1000),
            buf.area,
            &mut buf,
            &mut SidebarScroll::default(),
            &mut HitMap::default(),
        );
        buf
    }

    /// A pi-like harness with no mark.
    fn pi_like() -> HarnessInfo {
        HarnessInfo {
            id: HarnessId::new("pi"),
            label: "pi".to_owned(),
            icon: None,
        }
    }

    #[rstest::rstest]
    fn untagged_harness_shows_no_tag() {
        // Given a Working Claude thread.
        let sessions = sessions(vec![thread(1, ThreadStatus::Working)]);

        // When rendering the sidebar.
        let place = line(&draw_with(&sessions, &[info(), pi_like()]), 4);

        // Then its second line ends `working` with only spaces before it.
        assert!(
            place.trim_end().ends_with("   working"),
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

        // Then its third line is the middle guide, its agent row below, and
        // the branch.
        assert!(
            footer.starts_with(&format!(" ├╴{BRANCH} main ")),
            "line was '{footer}'"
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
            ..settling(sessions(vec![stopped(1)]), &[(1, 10)])
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
        let sessions = settling(
            sessions(vec![thread(1, ThreadStatus::Idle), stopped(2), stopped(3)]),
            &[(2, 10), (3, 20)],
        );

        // When rendering the sidebar.
        let header = shelf_line(&draw(&sessions, at(1000), 10));

        // Then the header counts both settled threads.
        assert!(header.trim_end().ends_with(" 2"), "line was '{header}'");
    }

    #[rstest::rstest]
    fn short_list_keeps_the_shelf_header_on_the_bottom_line() {
        // Given one active and one settled thread on a 10-line sidebar.
        let sessions = settling(
            sessions(vec![thread(1, ThreadStatus::Idle), stopped(2)]),
            &[(2, 10)],
        );

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
            ..settling(sessions(vec![stopped(2), stopped(3)]), &[(2, 10), (3, 20)])
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
            ..settling(sessions(vec![stopped(2)]), &[(2, 700)])
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
            ..settling(sessions(vec![thread(1, status)]), &[(1, 10)])
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
        let sessions = settling(
            sessions(vec![
                thread(1, ThreadStatus::Idle),
                thread(2, ThreadStatus::Idle),
                stopped(3),
            ]),
            &[(3, 10)],
        );

        // When rendering a 10-line sidebar.
        let (_, _, layout) = render_sized(&sessions, at(1000), 32, 10);

        // Then it reports the 7 lines under the input box and each row's
        // height.
        assert_eq!(
            layout,
            SidebarLayout {
                rows: 7,
                heights: vec![3, 1, 3, 1, 1],
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

        // Then its card's three lines fill the last three lines.
        assert_eq!(selected_y, Some(5), "the selected node's first line");
    }

    /// `active` idle threads, 1 to `active`, and the threads `settled_ids`
    /// settled at 10 s, with `selected`'s thread selected.
    fn overflowing(active: i64, settled_ids: &[i64], selected: i64) -> Sessions {
        let threads = (1..=active)
            .map(|id| thread(id, ThreadStatus::Idle))
            .chain(settled_ids.iter().map(|&id| stopped(id)))
            .collect();
        let settles: Vec<(i64, u64)> = settled_ids.iter().map(|&id| (id, 10)).collect();
        select(settling(sessions(threads), &settles), selected)
    }

    #[rstest::rstest]
    fn overflowing_list_holds_the_shelf_header_on_the_bottom_line() {
        // Given four threads and a settled one on a 10-line sidebar, with the
        // first node (thread 4) selected.
        let sessions = overflowing(4, &[5], 4);

        // When rendering the sidebar.
        let bottom = line(&draw(&sessions, at(1000), 10), 9);

        // Then the shelf header is on the last line.
        assert!(bottom.contains(" Settled "), "line was '{bottom}'");
    }

    #[rstest::rstest]
    #[case::next_to_the_header(1)]
    #[case::far_from_the_header(2)]
    fn selected_node_stays_whole_above_the_held_shelf_header(#[case] selected: i64) {
        // Given five threads and a settled one on a 10-line sidebar, with a
        // node near the list's end selected.
        let sessions = overflowing(5, &[6], selected);

        // When rendering the sidebar.
        let (buf, selected_y, _) = render_sized(&sessions, at(1000), 32, 10);

        // Then the card fills lines 6 to 8, right above the header's line.
        assert_eq!(
            (selected_y, line(&buf, 9).contains(" Settled ")),
            (Some(6), true),
            "thread {selected}'s first line, and the header on the last"
        );
    }

    #[rstest::rstest]
    fn shelf_header_in_view_scrolls_with_the_list() {
        // Given four threads and an open shelf of two on a 10-line sidebar,
        // with the last settled thread selected.
        let sessions = Sessions {
            shelf_open: true,
            ..overflowing(4, &[5, 6], 5)
        };

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 10);

        // Then the header is on its own line, the settled threads below it.
        let header = lines(&buf)
            .iter()
            .position(|line| line.contains(" Settled "));
        assert_eq!(header, Some(7), "the header's line");
    }

    #[rstest::rstest]
    #[case(24, 2)]
    #[case(2, 8)]
    #[case(80, 3)]
    #[case(0, 0)]
    fn sidebar_draws_at_any_size(#[case] width: u16, #[case] height: u16) {
        // Given a pinned thread, an active one and an open shelf, filtered to
        // orb, with the settled thread selected.
        let sessions = {
            let mut sessions = settling(
                sessions(vec![
                    thread(1, ThreadStatus::Working),
                    thread(2, ThreadStatus::NeedsApproval),
                    stopped(3),
                ]),
                &[(3, 10)],
            );
            if let Some(session) = sessions.sessions.get_mut(0) {
                session.pinned_at = Some(at(5));
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

    /// orb holding session 1 with `threads` as its agents, oldest first.
    fn one_session(threads: Vec<Thread>) -> Sessions {
        let agents: Vec<Thread> = threads
            .into_iter()
            .enumerate()
            .map(|(at_secs, thread)| Thread {
                created_at: at(u64::try_from(at_secs).unwrap_or_default()),
                pane: thread.pane.map(|launch| PaneLaunch {
                    session: SessionId(1),
                    ..launch
                }),
                ..thread
            })
            .collect();
        sessions(agents)
    }

    #[rstest::rstest]
    fn session_card_lists_one_row_per_agent_pane() {
        // Given session 1 with agent threads 1 and 2.
        let sessions = one_session(vec![
            thread(1, ThreadStatus::Idle),
            thread(2, ThreadStatus::Idle),
        ]);

        // When drawing the sidebar.
        let lines = lines(&draw(&sessions, at(1000), 12));

        // Then the card's fourth and fifth lines are the agents.
        assert!(
            lines.get(6).is_some_and(|line| line.contains("Thread 1"))
                && lines.get(7).is_some_and(|line| line.contains("Thread 2")),
            "lines were {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn agent_row_ends_with_its_harness_mark() {
        // Given session 1 with a pi agent.
        let sessions = one_session(vec![Thread {
            harness: HarnessId::new("pi"),
            ..thread(1, ThreadStatus::Idle)
        }]);

        // When drawing the sidebar with pi's mark, π.
        let pi = HarnessInfo {
            icon: Some("π".to_owned()),
            ..pi_like()
        };
        let buf = draw_with(&sessions, &[info(), pi]);

        // Then the agent row ends with π.
        assert!(
            line(&buf, 6).trim_end().ends_with('π'),
            "line was '{}'",
            line(&buf, 6)
        );
    }

    #[rstest::rstest]
    fn session_card_shows_its_most_urgent_agents_status() {
        // Given session 1 with a working agent and one needing approval.
        let sessions = one_session(vec![
            thread(1, ThreadStatus::Working),
            thread(2, ThreadStatus::NeedsApproval),
        ]);

        // When drawing the sidebar.
        let buf = draw(&sessions, at(1000), 12);

        // Then the card shows the approval icon and word.
        assert!(
            glyph(&buf, 1, 3) == Some((APPROVAL_ICON.to_owned(), YELLOW))
                && line(&buf, 4).trim_end().ends_with("approval"),
            "lines were {:#?}",
            lines(&buf)
        );
    }

    #[rstest::rstest]
    fn session_without_agents_shows_its_folder_as_title() {
        // Given session 1 whose agent has ended.
        let mut sessions = sessions(vec![thread(1, ThreadStatus::Idle)]);
        if let Some(thread) = sessions
            .projects
            .first_mut()
            .and_then(|project| project.threads.first_mut())
        {
            thread.pane = None;
        }

        // When drawing the sidebar.
        let buf = draw(&sessions, at(1000), 12);

        // Then its card is titled by its directory.
        assert!(
            line(&buf, 3).contains("orb"),
            "line was '{}'",
            line(&buf, 3)
        );
    }

    #[rstest::rstest]
    fn settled_session_is_one_line() {
        // Given settled session 1 on an open shelf.
        let sessions = Sessions {
            shelf_open: true,
            ..settling(sessions(vec![stopped(1)]), &[(1, 700)])
        };

        // When drawing the sidebar.
        let settled = settled_lines(&draw(&sessions, at(1000), 10));

        // Then its one line holds its title and the time since it settled.
        assert!(
            settled.len() == 1
                && settled
                    .first()
                    .is_some_and(|row| row.contains("Thread 1") && row.trim_end().ends_with("5m")),
            "lines were {settled:#?}"
        );
    }

    #[rstest::rstest]
    #[case::research(SessionKind::Research, BLUE2)]
    #[case::learn(SessionKind::Learn, PURPLE)]
    fn own_folder_session_shows_its_kind_icon(#[case] kind: SessionKind, #[case] colour: Color) {
        // Given a `kind` session.
        let mut sessions = sessions(vec![thread(1, ThreadStatus::Idle)]);
        if let Some(session) = sessions.sessions.first_mut() {
            session.kind = kind;
        }

        // When drawing the sidebar.
        let buf = draw(&sessions, at(1000), 12);

        // Then the third line shows the kind's icon in its colour.
        let shown = (0..buf.area.width)
            .filter_map(|x| glyph(&buf, x, 5))
            .any(|(symbol, fg)| symbol.trim() != "" && fg == colour);
        assert!(shown, "the {kind:?} icon's colour on '{}'", line(&buf, 5));
    }

    /// Session 1 holding `count` stopped agents, each in its own pane, its
    /// card `folded` or not.
    fn card_of(count: i64, folded: bool) -> Sessions {
        let threads = (1..=count)
            .map(|id| Thread {
                pane: Some(PaneLaunch {
                    pane: PaneId(id),
                    session: SessionId(1),
                }),
                ..stopped(id)
            })
            .collect();
        let mut sessions = sessions(threads);
        if folded {
            sessions.folded.insert(SessionId(1));
        }
        sessions
    }

    #[rstest::rstest]
    fn open_card_ends_its_last_line_with_the_down_chevron() {
        // Given session 1's open card over two agents.
        let sessions = card_of(2, false);

        // When drawing the sidebar.
        let buf = draw(&sessions, at(1000), 12);

        // Then its third line ends with the down chevron.
        assert!(
            line(&buf, 5).trim_end().ends_with(FOLD_OPEN),
            "line was '{}'",
            line(&buf, 5)
        );
    }

    #[rstest::rstest]
    fn folded_card_shows_a_status_icon_per_agent_then_the_right_chevron() {
        // Given session 1's card folded over two stopped agents.
        let sessions = card_of(2, true);

        // When drawing the sidebar.
        let buf = draw(&sessions, at(1000), 12);

        // Then its third line ends with two stop icons and the right chevron.
        assert!(
            line(&buf, 5)
                .trim_end()
                .ends_with(&format!("{STOPPED_ICON} {STOPPED_ICON} {FOLD_CLOSED}")),
            "line was '{}'",
            line(&buf, 5)
        );
    }

    #[rstest::rstest]
    fn folded_card_counts_agents_past_eight() {
        // Given session 1's card folded over ten agents.
        let sessions = card_of(10, true);

        // When drawing the sidebar.
        let buf = draw(&sessions, at(1000), 12);

        // Then its third line counts the two past eight.
        assert!(
            line(&buf, 5).contains(&format!("{STOPPED_ICON} +2 {FOLD_CLOSED}")),
            "line was '{}'",
            line(&buf, 5)
        );
    }

    #[rstest::rstest]
    fn folded_card_closes_its_tree() {
        // Given session 1's card folded over two agents.
        let sessions = card_of(2, true);

        // When drawing the sidebar.
        let buf = draw(&sessions, at(1000), 12);

        // Then its third line starts with the last guide.
        assert!(
            line(&buf, 5).starts_with(LAST_GUIDE),
            "line was '{}'",
            line(&buf, 5)
        );
    }

    #[rstest::rstest]
    fn card_without_agents_shows_no_chevron() {
        // Given session 1 whose agent has ended.
        let mut sessions = card_of(1, false);
        for project in &mut sessions.projects {
            for thread in &mut project.threads {
                thread.pane = None;
            }
        }

        // When drawing the sidebar.
        let buf = draw(&sessions, at(1000), 12);

        // Then its third line has no chevron.
        let third = line(&buf, 5);
        assert!(
            !third.contains(FOLD_OPEN) && !third.contains(FOLD_CLOSED),
            "line was '{third}'"
        );
    }

    /// `sessions` with session 1 a Learn session in `~/.orb/learn/<slug>`.
    fn learning(mut sessions: Sessions, slug: &str) -> Sessions {
        if let Some(session) = sessions.sessions.first_mut() {
            session.kind = SessionKind::Learn;
            session.dir = format!("{HOME}/.orb/learn/{slug}").into();
        }
        sessions
    }

    #[rstest::rstest]
    fn learn_card_shows_its_folder_as_a_tilde_path() {
        // Given a Learn session in ~/.orb/learn/rust.
        let sessions = learning(card_of(1, false), "rust");

        // When drawing the sidebar.
        let buf = draw(&sessions, at(1000), 12);

        // Then its third line shows the path from home.
        assert!(
            line(&buf, 5).contains("~/.orb/learn/rust"),
            "line was '{}'",
            line(&buf, 5)
        );
    }

    #[rstest::rstest]
    fn narrow_learn_card_cuts_its_path_before_the_chevron() {
        // Given a Learn session with a long slug, folded over one agent.
        let sessions = learning(card_of(1, true), "a-long-learning-folder");

        // When drawing a 24-column sidebar.
        let (buf, ..) = render_sized(&sessions, at(1000), 24, 12);

        // Then its third line still ends with the right chevron.
        assert!(
            line(&buf, 5).trim_end().ends_with(FOLD_CLOSED),
            "line was '{}'",
            line(&buf, 5)
        );
    }

    /// Draws a 32-column sidebar `height` lines tall at 1000 s with `scroll`;
    /// returns the selected row's first line and the hit map it filled.
    fn render_with(
        sessions: &Sessions,
        scroll: &mut SidebarScroll,
        height: u16,
    ) -> (Option<u16>, HitMap) {
        let mut buf = Buffer::empty(Rect::new(0, 0, 32, height));
        let mut hits = HitMap::default();
        let (selected_y, _, _) = render(
            sessions,
            &HashSet::new(),
            &[info()],
            &Layouts::default(),
            Path::new(HOME),
            at(1000),
            buf.area,
            &mut buf,
            scroll,
            &mut hits,
        );
        (selected_y, hits)
    }

    /// Threads 1 to 3, thread 3's node on top, with `selected`'s selected.
    fn three_threads(selected: i64) -> Sessions {
        select(
            sessions(vec![
                thread(1, ThreadStatus::Idle),
                thread(2, ThreadStatus::Idle),
                thread(3, ThreadStatus::Idle),
            ]),
            selected,
        )
    }

    #[rstest::rstest]
    fn hit_map_maps_a_scrolled_rows_last_line_to_it() {
        // Given three sessions on an 8-line sidebar scrolled to the last one
        // (session 1), so session 2's last line tops the list.
        let y = 3;
        let sessions = three_threads(1);

        // When rendering the sidebar.
        let (_, hits) = render_with(&sessions, &mut SidebarScroll::default(), 8);

        // Then a click on that line lands on session 2.
        assert_eq!(
            hits.row_at(Position::new(1, y)),
            Some(SidebarItem::Session(SessionId(2))),
            "the row at line {y}"
        );
    }

    #[rstest::rstest]
    fn hit_map_maps_the_sticky_header_row_to_the_shelf() {
        // Given four threads and a settled one on a 10-line sidebar, with
        // the shelf header held on the bottom line.
        let sessions = overflowing(4, &[5], 4);

        // When rendering the sidebar.
        let (_, hits) = render_with(&sessions, &mut SidebarScroll::default(), 10);

        // Then a click on the bottom line lands on the shelf.
        assert_eq!(
            hits.row_at(Position::new(1, 9)),
            Some(SidebarItem::SettledShelf),
            "the row on the bottom line"
        );
    }

    #[rstest::rstest]
    fn hit_map_maps_the_shelf_gap_to_nothing() {
        // Given thread 1 and a settled thread 2 on a 12-line sidebar.
        let sessions = overflowing(1, &[2], 1);

        // When rendering the sidebar.
        let (_, hits) = render_with(&sessions, &mut SidebarScroll::default(), 12);

        // Then a blank line between thread 1 and the shelf header, held at
        // the bottom, lands on no row.
        assert_eq!(
            hits.row_at(Position::new(1, 7)),
            None,
            "the row on the gap line"
        );
    }

    #[rstest::rstest]
    fn hit_map_records_the_input_box() {
        // Given an empty orb.
        let sessions = Sessions::default();

        // When rendering the sidebar.
        let (_, hits) = render_with(&sessions, &mut SidebarScroll::default(), 8);

        // Then the prompt's line is on the input box.
        assert!(
            hits.on_sidebar_input(Position::new(1, 1)),
            "the prompt line should be on the input box"
        );
    }

    #[rstest::rstest]
    fn free_scroll_leaves_the_selected_row_off_screen() {
        // Given three threads on an 8-line sidebar with the top one (thread
        // 3) selected, and the view wheeled 3 lines down.
        let sessions = three_threads(3);
        let mut scroll = SidebarScroll::default();
        scroll.scroll_free(3, sessions.cursor);

        // When rendering the sidebar.
        let (selected_y, _) = render_with(&sessions, &mut scroll, 8);

        // Then the selected row's first line is off screen.
        assert_eq!(selected_y, None, "the selected row's first line");
    }

    #[rstest::rstest]
    fn free_scroll_stops_at_the_lists_end() {
        // Given three threads on an 8-line sidebar with thread 3 selected,
        // and the view wheeled 30 lines down.
        let sessions = three_threads(3);
        let mut scroll = SidebarScroll::default();
        scroll.scroll_free(30, sessions.cursor);

        // When rendering the sidebar.
        let (_, hits) = render_with(&sessions, &mut scroll, 8);

        // Then the last line shows the list's last row, thread 1's agent
        // row.
        assert_eq!(
            hits.row_at(Position::new(1, 7)),
            Some(SidebarItem::Agent {
                session: SessionId(1),
                pane: PaneId(1),
            }),
            "the row on the bottom line"
        );
    }

    #[rstest::rstest]
    fn free_scroll_snaps_back_once_the_selection_moved() {
        // Given the view wheeled down while thread 2 was selected, and
        // thread 3 selected since.
        let sessions = three_threads(3);
        let mut scroll = SidebarScroll::default();
        scroll.scroll_free(3, Some(SidebarItem::Session(SessionId(2))));

        // When rendering the sidebar.
        let (selected_y, _) = render_with(&sessions, &mut scroll, 8);

        // Then the view follows the selection to the top of the list.
        assert_eq!(selected_y, Some(3), "the selected row's first line");
    }

    #[rstest::rstest]
    fn released_scroll_snaps_back_to_the_selection() {
        // Given the view wheeled down on thread 3, then released.
        let sessions = three_threads(3);
        let mut scroll = SidebarScroll::default();
        scroll.scroll_free(3, sessions.cursor);
        scroll.release();

        // When rendering the sidebar.
        let (selected_y, _) = render_with(&sessions, &mut scroll, 8);

        // Then the view follows the selection to the top of the list.
        assert_eq!(selected_y, Some(3), "the selected row's first line");
    }

    /// Thread 1's agent row.
    fn agent_one() -> SidebarItem {
        SidebarItem::Agent {
            session: SessionId(1),
            pane: PaneId(1),
        }
    }

    /// Draws a 32-column sidebar 10 lines tall with pane names from
    /// `layouts`. Thread 1's card takes lines 3 to 5, its agent row line 6.
    fn draw_with_layouts(sessions: &Sessions, layouts: &Layouts) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, 32, 10));
        render(
            sessions,
            &HashSet::new(),
            &[info()],
            layouts,
            Path::new(HOME),
            at(1000),
            buf.area,
            &mut buf,
            &mut SidebarScroll::default(),
            &mut HitMap::default(),
        );
        buf
    }

    #[rstest::rstest]
    fn agent_row_reads_its_panes_name() {
        // Given thread 1 running in pane 1, named "api".
        let sessions = sessions(vec![thread(1, ThreadStatus::Idle)]);
        let layouts = {
            let mut layouts = Layouts::default();
            layouts.insert(
                SessionId(1),
                SessionLayout::of(PaneEntry {
                    id: PaneId(1),
                    zmx: ZmxSession {
                        name: "orb-p1".into(),
                        dir: "/tmp/zmx".into(),
                    },
                    cwd: "/tmp".into(),
                    name: Some("api".into()),
                    resume: None,
                }),
            );
            layouts
        };

        // When rendering the sidebar.
        let agent = line(&draw_with_layouts(&sessions, &layouts), 6);

        // Then the agent row shows the pane's name.
        assert!(agent.contains("api"), "line was '{agent}'");
    }

    #[rstest::rstest]
    fn agent_row_falls_back_to_its_threads_title() {
        // Given thread 1 running in an unnamed pane.
        let sessions = sessions(vec![thread(1, ThreadStatus::Idle)]);

        // When rendering the sidebar.
        let agent = line(&draw_with_layouts(&sessions, &Layouts::default()), 6);

        // Then the agent row shows the thread's title.
        assert!(agent.contains("Thread 1"), "line was '{agent}'");
    }

    #[rstest::rstest]
    fn selected_agent_row_has_the_visual_background() {
        // Given the cursor on thread 1's agent row.
        let sessions = Sessions {
            cursor: Some(agent_one()),
            ..sessions(vec![thread(1, ThreadStatus::Idle)])
        };

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 10);

        // Then the agent row's line has the selection background.
        assert_eq!(
            buf.cell((1, 6)).map(|cell| cell.bg),
            Some(VISUAL),
            "the agent row's line"
        );
    }

    #[rstest::rstest]
    fn vanished_agent_cursor_highlights_its_card() {
        // Given the cursor on an agent row of session 1 whose pane 99 is gone.
        let sessions = Sessions {
            cursor: Some(SidebarItem::Agent {
                session: SessionId(1),
                pane: PaneId(99),
            }),
            ..sessions(vec![thread(1, ThreadStatus::Idle)])
        };

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 10);

        // Then the card's first line has the selection background.
        assert_eq!(
            buf.cell((1, 3)).map(|cell| cell.bg),
            Some(VISUAL),
            "the card's first line"
        );
    }

    #[rstest::rstest]
    fn agent_line_records_its_row_in_the_hit_map() {
        // Given thread 1 on a 10-line sidebar.
        let sessions = select(sessions(vec![thread(1, ThreadStatus::Idle)]), 1);

        // When rendering the sidebar.
        let (_, hits) = render_with(&sessions, &mut SidebarScroll::default(), 10);

        // Then a click on the agent's line lands on its agent row.
        assert_eq!(
            hits.row_at(Position::new(1, 6)),
            Some(agent_one()),
            "the row at the agent's line"
        );
    }

    #[rstest::rstest]
    fn click_on_an_agent_line_selects_its_row() {
        // Given thread 1 drawn on a 10-line sidebar that has the keys.
        let sessions = select(sessions(vec![thread(1, ThreadStatus::Idle)]), 1);
        let (_, hits) = render_with(&sessions, &mut SidebarScroll::default(), 10);

        // When clicking the agent's line.
        let routed = crate::mouse::route(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 1,
                row: 6,
                modifiers: KeyModifiers::NONE,
            },
            &hits,
            Focus::Sidebar,
            None,
            &mut Clicks::default(),
            Instant::now(),
        );

        // Then the agent row is selected.
        assert_eq!(
            routed,
            MouseRoute::Intents(vec![Intent::SelectRow(agent_one())]),
            "a click on an agent line should select its row"
        );
    }

    #[rstest::rstest]
    fn input_box_counts_sessions_not_agent_rows() {
        // Given two sessions, each running one agent.
        let sessions = sessions(vec![
            thread(1, ThreadStatus::Idle),
            thread(2, ThreadStatus::Idle),
        ]);

        // When rendering the sidebar.
        let prompt = line(&draw(&sessions, at(1000), 12), 1);

        // Then the count is two sessions out of two.
        assert!(prompt.trim_end().ends_with("2/2│"), "line was '{prompt}'");
    }
}
