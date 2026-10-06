//! The sidebar: one list of orb's drafts and threads across projects, drawn
//! like LazyVim's file explorer (snacks.nvim) in tokyonight-moon.
//!
//! An input box heads it: `Sessions`, with an `i` badge lit while the
//! search has the keys, the filtered project after the `>` prompt, and how
//! many drafts and threads are listed out of all of them. Below it, drafts
//! come first, then pinned threads, then active ones, each a three-line tree
//! node: the status icon and title, then the project and status, then the
//! branch (a draft's workspace). A group is a three-line node too: its most
//! urgent child's status and its slug, then its kind and project, then its
//! branch or folder with an icon per child and a fold chevron; its children
//! hang below it as one-line rows while it's open. The selected row's first
//! line is highlighted. Settled threads and groups fold into a shelf at the
//! bottom, drawn as one-line rows while it's open. The sidebar scrolls to
//! keep the whole selected row in view, unless the wheel scrolled it while
//! the keys are elsewhere.
//!
//! While the user searches, the typed text follows the prompt, and only
//! drafts and threads whose title matches it are listed, settled ones
//! included, with the matched characters highlighted as in the pickers.

use std::borrow::Cow;
use std::collections::HashSet;
use std::time::{Duration, SystemTime};

use orb_domain::feat::harness::HarnessInfo;
use orb_domain::feat::sessions::state::{
    Draft, DraftWorkspace, Group, GroupKind, NEW_THREAD, Project, Sessions, SidebarItem,
    SidebarRow, Thread, ThreadId, ThreadStatus,
};
use orb_domain::feat::sidebar::state::SidebarLayout;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Widget};
use unicode_segmentation::UnicodeSegmentation;

use crate::mouse::HitMap;
use crate::picker::{highlight, visible};

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
/// thread in `attached` shows a filled circle; a thread's node shows its
/// harness's tag from `harnesses`.
#[expect(
    clippy::too_many_arguments,
    reason = "the sidebar's inputs plus the scroll and hit map it updates"
)]
pub(crate) fn render(
    sessions: &Sessions,
    attached: &HashSet<ThreadId>,
    harnesses: &[HarnessInfo],
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
        sessions, attached, harnesses, rows, now, list, buf, scroll, hits,
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

/// `shown/total`, like snacks' match count: the drafts and threads listed
/// (not group cards), out of every draft, group draft and thread not being
/// deleted.
fn count(sessions: &Sessions, rows: &[SidebarRow<'_>]) -> String {
    let shown = rows
        .iter()
        .filter(|row| {
            !matches!(
                row,
                SidebarRow::ShelfHeader { .. }
                    | SidebarRow::GroupCard { .. }
                    | SidebarRow::SettledGroup { .. }
            )
        })
        .count();
    let drafts = sessions
        .projects
        .iter()
        .map(|project| {
            usize::from(project.draft.is_some())
                + project
                    .groups
                    .iter()
                    .filter(|group| group.draft.is_some())
                    .count()
        })
        .sum::<usize>();
    let threads = sessions
        .threads()
        .filter(|thread| !sessions.deleting.contains(&thread.id))
        .count();
    format!("{shown}/{}", drafts + threads)
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
    attached: &HashSet<ThreadId>,
    harnesses: &[HarnessInfo],
    rows: Vec<SidebarRow<'_>>,
    now: SystemTime,
    area: Rect,
    buf: &mut Buffer,
    scroll: &mut SidebarScroll,
    hits: &mut HitMap,
) -> Option<u16> {
    let (placed, total) = place(rows, area.height);
    let selected = placed
        .iter()
        .find(|(row, _)| Some(row.item()) == sessions.cursor)
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
            if Some(row.item()) == sessions.cursor {
                list.set_style(
                    Rect {
                        height: 1,
                        ..row_area
                    },
                    Style::new().bg(VISUAL),
                );
            }
            let ends_shelf = !placed
                .iter()
                .skip(index + 1)
                .map(|(row, _)| row)
                .find(|row| {
                    !matches!(
                        row,
                        SidebarRow::GroupThread { .. } | SidebarRow::GroupDraftRow { .. }
                    )
                })
                .is_some_and(|row| {
                    matches!(
                        row,
                        SidebarRow::Settled { .. } | SidebarRow::SettledGroup { .. }
                    )
                });
            render_row(
                sessions, attached, harnesses, row, ends_shelf, now, row_area, &mut list,
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

/// How many lines a row takes: a draft's, thread's or group's node 3, else 1.
fn height(row: &SidebarRow<'_>) -> u16 {
    match row {
        SidebarRow::Draft { .. } | SidebarRow::Card { .. } | SidebarRow::GroupCard { .. } => 3,
        SidebarRow::ShelfHeader { .. }
        | SidebarRow::Settled { .. }
        | SidebarRow::GroupThread { .. }
        | SidebarRow::GroupDraftRow { .. }
        | SidebarRow::SettledGroup { .. } => 1,
    }
}

/// One row; `ends_shelf` says no settled entry follows it. Titles show where
/// the search matched them.
#[expect(
    clippy::too_many_arguments,
    reason = "the row plus the frame inputs every node takes"
)]
fn render_row(
    sessions: &Sessions,
    attached: &HashSet<ThreadId>,
    harnesses: &[HarnessInfo],
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
            render_card(
                project,
                thread,
                harnesses.iter().find(|info| info.id == thread.harness),
                attached.contains(&thread.id),
                &matched(title(thread)),
                now,
                area,
                buf,
            );
        }
        SidebarRow::ShelfHeader { count, open } => render_shelf_header(*count, *open, area, buf),
        SidebarRow::Settled { thread, .. } => {
            let matched = matched(title(thread));
            render_settled(thread, &matched, ends_shelf, now, area, buf);
        }
        SidebarRow::GroupCard {
            project,
            group,
            open,
        } => render_group_card(
            sessions,
            attached,
            project,
            group,
            *open,
            &matched(&group.name),
            now,
            area,
            buf,
        ),
        SidebarRow::GroupThread { thread, last, .. } => render_group_thread(
            thread,
            attached.contains(&thread.id),
            *last,
            &matched(title(thread)),
            now,
            area,
            buf,
        ),
        SidebarRow::GroupDraftRow { last, .. } => render_split(
            Line::from(
                [
                    span(if *last { LAST_CHILD_GUIDE } else { CHILD_GUIDE }, GUTTER),
                    span(format!("{PENCIL} "), YELLOW),
                ]
                .into_iter()
                .chain(highlight(NEW_THREAD, &matched(NEW_THREAD), |_| FG))
                .collect::<Vec<_>>(),
            ),
            Line::from(span("draft", DARK3)),
            area,
            buf,
        ),
        SidebarRow::SettledGroup { project, group, .. } => {
            let guide = if ends_shelf { LAST_GUIDE } else { GUIDE };
            let (icon, colour) = kind_look(group.kind);
            let ago = ago_label(since(now, group.settled_at.unwrap_or(group.created_at)));
            render_split(
                Line::from(
                    [span(guide, GUTTER), span(format!("{icon} "), colour)]
                        .into_iter()
                        .chain(highlight(&group.name, &matched(&group.name), |_| COMMENT))
                        .collect::<Vec<_>>(),
                ),
                Line::from(span(
                    format!("{} · {ago}", members(sessions, project, group).len()),
                    DARK3,
                )),
                area,
                buf,
            );
        }
    }
}

/// A group's thread under its card: its guide (`last` ends the group), status
/// icon, title (`matched` at those byte offsets) and time.
fn render_group_thread(
    thread: &Thread,
    attached: bool,
    last: bool,
    matched: &[usize],
    now: SystemTime,
    area: Rect,
    buf: &mut Buffer,
) {
    let guide = if last { LAST_CHILD_GUIDE } else { CHILD_GUIDE };
    let (glyph, _, colour) = status(thread, attached, now);
    render_split(
        Line::from(
            [span(guide, GUTTER), span(format!("{glyph} "), colour)]
                .into_iter()
                .chain(highlight(title(thread), matched, |_| FG))
                .collect::<Vec<_>>(),
        ),
        Line::from(span(when(thread, now), COMMENT)),
        area,
        buf,
    );
}

/// A thread's node: its status icon, title (`matched` at those byte
/// offsets), pin and time; the project, its harness's tag (`info`) and
/// status word; the branch and the harness's mark.
#[expect(
    clippy::too_many_arguments,
    reason = "the node's parts plus the frame inputs every node takes"
)]
fn render_card(
    project: &Project,
    thread: &Thread,
    info: Option<&HarnessInfo>,
    attached: bool,
    matched: &[usize],
    now: SystemTime,
    area: Rect,
    buf: &mut Buffer,
) {
    let [heading, place, footer] = Layout::vertical([Constraint::Length(1); 3]).areas(area);
    let (glyph, word, colour) = status(thread, attached, now);
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
    let tag = info.and_then(|info| info.tag.as_deref());
    render_split(
        project_line(project),
        Line::from(match (tag, word) {
            (Some(tag), Some(word)) => vec![span(format!("{tag} "), DARK3), span(word, colour)],
            (Some(tag), None) => vec![span(tag, DARK3)],
            (None, Some(word)) => vec![span(word, colour)],
            (None, None) => vec![],
        }),
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
        Line::from(
            info.and_then(|info| info.icon.clone())
                .map(|icon| span(icon, LOGO))
                .unwrap_or_default(),
        ),
        footer,
        buf,
    );
}

/// A group's node: its most urgent child's icon, the slug (`matched` at
/// those byte offsets), pin and time; the kind icon, project and status
/// word; the branch or folder, one icon per child, and the fold chevron.
#[expect(
    clippy::too_many_arguments,
    reason = "the group row's parts plus the frame inputs every node takes"
)]
fn render_group_card(
    sessions: &Sessions,
    attached: &HashSet<ThreadId>,
    project: &Project,
    group: &Group,
    open: bool,
    matched: &[usize],
    now: SystemTime,
    area: Rect,
    buf: &mut Buffer,
) {
    let [heading, place_area, footer] = Layout::vertical([Constraint::Length(1); 3]).areas(area);
    let threads = members(sessions, project, group);
    let urgent = threads.iter().copied().min_by_key(|thread| rank(thread));
    let (glyph, word, colour) = urgent.map_or((PENCIL, Some("draft"), YELLOW), |thread| {
        status(thread, attached.contains(&thread.id), now)
    });
    let pin = if group.pinned_at.is_some() { PIN } else { "" };
    let time = match urgent {
        Some(thread) if thread.status == ThreadStatus::Working => when(thread, now),
        _ => ago_label(since(
            now,
            threads
                .iter()
                .map(|thread| thread.last_activity_at)
                .max()
                .unwrap_or(group.created_at),
        )),
    };
    render_split(
        Line::from(
            [span(format!(" {glyph} "), colour)]
                .into_iter()
                .chain(highlight(&group.name, matched, |_| FG))
                .collect::<Vec<_>>(),
        ),
        Line::from(vec![span(format!("{pin} "), ORANGE), span(time, COMMENT)]),
        heading,
        buf,
    );
    let (icon, kind_colour) = kind_look(group.kind);
    render_split(
        Line::from(vec![
            span(GUIDE, GUTTER),
            span(format!("{icon} "), kind_colour),
            span(project.title.as_str(), FG_DARK),
        ]),
        word.map(|word| Line::from(span(word, colour)))
            .unwrap_or_default(),
        place_area,
        buf,
    );
    let icons = group
        .draft
        .is_some()
        .then_some((PENCIL, YELLOW))
        .into_iter()
        .chain(threads.iter().map(|thread| {
            let (glyph, _, colour) = status(thread, attached.contains(&thread.id), now);
            (glyph, colour)
        }))
        .collect::<Vec<_>>();
    let rest = icons.len().saturating_sub(CHILD_ICONS);
    render_split(
        Line::from(vec![
            span(LAST_GUIDE, GUTTER),
            span(group_place(group), COMMENT),
        ]),
        Line::from(
            icons
                .into_iter()
                .take(CHILD_ICONS)
                .map(|(glyph, colour)| span(format!("{glyph} "), colour))
                .chain((rest > 0).then(|| span(format!("+{rest} "), COMMENT)))
                .chain([span(if open { FOLD_OPEN } else { FOLD_CLOSED }, DARK3)])
                .collect::<Vec<_>>(),
        ),
        footer,
        buf,
    );
}

/// A group's threads not being deleted, newest first: every one, not only
/// those a search lists.
fn members<'a>(sessions: &Sessions, project: &'a Project, group: &Group) -> Vec<&'a Thread> {
    project
        .threads
        .iter()
        .filter(|thread| thread.group == Some(group.id) && !sessions.deleting.contains(&thread.id))
        .collect()
}

/// How urgent a child's status is for its group's card; the lowest wins.
fn rank(thread: &Thread) -> u8 {
    match thread.status {
        ThreadStatus::NeedsApproval => 0,
        ThreadStatus::NeedsInput => 1,
        ThreadStatus::Failed | ThreadStatus::Gone => 2,
        ThreadStatus::Working => 3,
        ThreadStatus::Idle if thread.unseen => 4,
        ThreadStatus::Idle | ThreadStatus::Unknown => 5,
        ThreadStatus::Stopped => 6,
    }
}

/// Where a group's sessions run: a Feature's branch, else its folder under
/// `~/.orb`.
fn group_place(group: &Group) -> String {
    match group.kind {
        GroupKind::Feature => format!("{BRANCH} {}", group.branch.as_deref().unwrap_or("—")),
        GroupKind::Research => format!("{FOLDER} ~/.orb/research/{}", group.name),
        GroupKind::Learn => format!("{FOLDER} ~/.orb/learn/{}", group.name),
    }
}

/// A group kind's icon and colour: Feature `nf-fa-code_fork` green1,
/// Research `nf-fa-flask` blue2, Learn `nf-fa-book` purple.
pub(crate) fn kind_look(kind: GroupKind) -> (&'static str, Color) {
    match kind {
        GroupKind::Feature => ("\u{f126}", GREEN1),
        GroupKind::Research => ("\u{f0c3}", BLUE2),
        GroupKind::Learn => ("\u{f02d}", PURPLE),
    }
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
            let (glyph, _, colour) = status(thread, false, now);
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

/// The Settled shelf's label in the dashboard's shelf hint: `▸ Settled (N)`
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
/// The mode line's ATTACHED (`green1`, lualine's terminal mode), and a
/// Feature group's kind icon.
pub(crate) const GREEN1: Color = Color::Rgb(0x4f, 0xd6, 0xbe);
/// A Research group's kind icon (`blue2`).
pub(crate) const BLUE2: Color = Color::Rgb(0x0d, 0xb9, 0xd7);
/// A Learn group's kind icon (`purple`).
pub(crate) const PURPLE: Color = Color::Rgb(0xfc, 0xa7, 0xea);
/// Needing approval, and a draft's pencil; the picker's permission shield
/// and `worktree` branch badge; the rename box (`yellow`).
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
/// A harness's mark, and the dashboard's Start item (orange).
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
/// Before a harness in the picker and the dashboard (`nf-fa-plug`).
pub(crate) const PLUG: &str = "\u{f1e6}";
/// A draft (`nf-fa-pencil`).
const PENCIL: &str = "\u{f040}";
/// An open group, at the end of its card (`nf-oct-chevron_down`).
const FOLD_OPEN: &str = "\u{f47c}";
/// A closed group (`nf-oct-chevron_right`).
const FOLD_CLOSED: &str = "\u{f460}";
/// How many of a group's children its card shows an icon for.
const CHILD_ICONS: usize = 8;
/// A model, before a harness's models when it has no mark
/// (`nf-fa-microchip`).
pub(crate) const CHIP: &str = "\u{f2db}";
/// The tree guide before a node's middle line and a settled row.
const GUIDE: &str = " ├╴";
/// The tree guide before a node's last line and the last settled row.
const LAST_GUIDE: &str = " └╴";
/// The guides before a group's child rows, under the card's tree.
const CHILD_GUIDE: &str = "    ├╴";
/// The guide before a group's last child row.
const LAST_CHILD_GUIDE: &str = "    └╴";

#[cfg(test)]
mod tests {
    use orb_domain::feat::harness::claude::models::info;
    use orb_domain::feat::harness::{HarnessId, HarnessInfo};
    use std::collections::HashSet;
    use std::time::{Duration, SystemTime};

    use orb_domain::TextInput;
    use orb_domain::feat::sessions::state::{
        Draft, DraftWorkspace, Group, GroupDefaults, GroupDraft, GroupId, GroupKind, Project,
        ProjectId, ProjectKind, Search, Sessions, SidebarItem, Thread, ThreadId, ThreadStatus,
    };
    use orb_domain::feat::sidebar::state::SidebarLayout;
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::{Position, Rect};
    use ratatui::style::{Color, Modifier};

    use super::{
        APPROVAL_ICON, ATTACHED_ICON, BG_DARK, BLUE, BLUE1, BLUE2, BRANCH, CHILD_GUIDE, COMMENT,
        COMPLETED_ICON, CYAN, DARK3, FAILED_ICON, FG, FOLD_CLOSED, FOLD_OPEN, FOLDER, FOLDER_OPEN,
        GONE_ICON, GREEN, GREEN1, GUIDE, GUTTER, IDLE_ICON, INPUT_ICON, LAST_CHILD_GUIDE,
        LAST_GUIDE, LOGO, MAGENTA, ORANGE, PENCIL, PIN, PURPLE, RED, STOPPED_ICON, SidebarScroll,
        VISUAL, YELLOW, ago_label, badge_colour, monogram, render, working_label,
    };
    use crate::mouse::HitMap;

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    /// A thread titled "Thread <id>" whose turn started at 866 s.
    fn thread(id: i64, status: ThreadStatus) -> Thread {
        Thread {
            harness: HarnessId::new("claude"),
            id: ThreadId(id),
            title: Some(format!("Thread {id}")),
            cwd: "/Users/me/dev/orb".into(),
            transcript: None,
            status,
            turn_started_at: Some(at(866)),
            pane: None,
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
            groups: vec![],
            kind: ProjectKind::Normal,
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
                harness: HarnessId::new("claude"),
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
            &HashSet::new(),
            &[info()],
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
            .map(ThreadId)
            .collect::<HashSet<_>>();
        render(
            sessions,
            &attached,
            &[info()],
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
    #[case::shelf_open(Sessions { shelf_open: true, ..sessions(vec![settled(1, 10)]) }, (DARK3, BG_DARK))]
    #[case::searching(searching(sessions(vec![settled(1, 10)]), ""), (BLUE, GUTTER))]
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
            &HashSet::new(),
            &[info()],
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

    /// Draws a 32-column sidebar 8 lines tall at 1000 s with `harnesses`
    /// registered.
    fn draw_with(sessions: &Sessions, harnesses: &[HarnessInfo]) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, 32, 8));
        render(
            sessions,
            &HashSet::new(),
            harnesses,
            at(1000),
            buf.area,
            &mut buf,
            &mut SidebarScroll::default(),
            &mut HitMap::default(),
        );
        buf
    }

    /// A pi-like harness: tagged `pi`, with no mark.
    fn pi_like() -> HarnessInfo {
        HarnessInfo {
            tag: Some("pi".to_owned()),
            unavailable: None,
            ..HarnessInfo::placeholder(HarnessId::new("pi"), "pi")
        }
    }

    #[rstest::rstest]
    fn tagged_harness_shows_its_tag_before_the_status_word() {
        // Given a Working thread of the pi-like harness.
        let sessions = sessions(vec![Thread {
            harness: HarnessId::new("pi"),
            ..thread(1, ThreadStatus::Working)
        }]);

        // When rendering the sidebar.
        let buf = draw_with(&sessions, &[info(), pi_like()]);

        // Then its second line ends `pi working`, with `pi` dim.
        let place = line(&buf, 4);
        let tag_colour = (0..buf.area.width)
            .find(|&x| {
                ["p", "i", " ", "w"]
                    .iter()
                    .zip(x..)
                    .all(|(symbol, x)| buf.cell((x, 4)).map(Cell::symbol) == Some(*symbol))
            })
            .and_then(|x| buf.cell((x, 4)))
            .map(|cell| cell.fg);
        assert_eq!(
            (place.trim_end().ends_with(" pi working"), tag_colour),
            (true, Some(DARK3)),
            "line was '{place}'"
        );
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

        // Then its third line is the last guide and the branch.
        assert!(
            footer.starts_with(&format!(" └╴{BRANCH} main ")),
            "line was '{footer}'"
        );
    }

    #[rstest::rstest]
    fn node_third_line_ends_with_the_harness_mark() {
        // Given a thread.
        let sessions = sessions(vec![thread(1, ThreadStatus::Idle)]);

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 8);

        // Then its third line ends with ✳ in Claude orange, before the blank
        // column.
        assert_eq!(
            glyph(&buf, 30, 5),
            Some(("✳".to_owned(), LOGO)),
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

    /// `active` idle threads, 1 to `active`, and the threads `settled_ids`
    /// settled at 10 s, with `selected`'s thread selected.
    fn overflowing(active: i64, settled_ids: &[i64], selected: i64) -> Sessions {
        let threads = (1..=active)
            .map(|id| thread(id, ThreadStatus::Idle))
            .chain(settled_ids.iter().map(|&id| settled(id, 10)))
            .collect();
        select(sessions(threads), selected)
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

        // Then the node fills lines 6 to 8, right above the header's line.
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

    /// orb holding group "login-flow": with thread 1 unless `draft`, settled
    /// on an open shelf if `settled`.
    fn group_sessions(settled: bool, draft: bool) -> Sessions {
        let group = Group {
            id: GroupId(9),
            kind: GroupKind::Feature,
            name: "login-flow".to_owned(),
            dir: None,
            branch: None,
            created_at: SystemTime::UNIX_EPOCH,
            pinned_at: None,
            settled_at: settled.then(|| at(20)),
            active_since: SystemTime::UNIX_EPOCH,
            draft: draft.then(GroupDraft::default),
            defaults: GroupDefaults {
                harness: HarnessId::new("claude"),
                model: None,
                permission: None,
            },
        };
        let threads = if draft {
            vec![]
        } else {
            vec![Thread {
                group: Some(GroupId(9)),
                ..thread(1, ThreadStatus::Idle)
            }]
        };
        Sessions {
            projects: vec![Project {
                groups: vec![group],
                ..project(1, "orb", threads)
            }],
            shelf_open: settled,
            ..Sessions::default()
        }
    }

    #[rstest::rstest]
    #[case::group_card(group_sessions(false, false), "login-flow")]
    #[case::group_thread(group_sessions(false, false), "Thread 1")]
    #[case::group_draft_row(group_sessions(false, true), "New thread")]
    #[case::settled_group(group_sessions(true, false), "login-flow")]
    fn minimal_group_rows_draw_their_names(#[case] sessions: Sessions, #[case] expected: &str) {
        // Given a sidebar listing a group row.

        // When drawing the sidebar.
        let buf = draw(&sessions, at(900), 20);

        // Then the row's text appears.
        assert!(
            lines(&buf).iter().any(|line| line.contains(expected)),
            "the sidebar should show {expected:?}: {:#?}",
            lines(&buf)
        );
    }

    /// orb holding group 9 with `threads` as its children and a draft if
    /// `draft`: `GT-514-login` on branch `GT-514-login` for a Feature, else
    /// `tokio-cancel`.
    fn grouped(kind: GroupKind, threads: Vec<Thread>, draft: bool) -> Sessions {
        let name = match kind {
            GroupKind::Feature => "GT-514-login",
            GroupKind::Research | GroupKind::Learn => "tokio-cancel",
        };
        let group = Group {
            id: GroupId(9),
            kind,
            name: name.to_owned(),
            dir: None,
            branch: (kind == GroupKind::Feature).then(|| name.to_owned()),
            created_at: SystemTime::UNIX_EPOCH,
            pinned_at: None,
            settled_at: None,
            active_since: SystemTime::UNIX_EPOCH,
            draft: draft.then(GroupDraft::default),
            defaults: GroupDefaults {
                harness: HarnessId::new("claude"),
                model: None,
                permission: None,
            },
        };
        let threads = threads
            .into_iter()
            .map(|thread| Thread {
                group: Some(GroupId(9)),
                ..thread
            })
            .collect();
        Sessions {
            projects: vec![Project {
                groups: vec![group],
                ..project(1, "orb", threads)
            }],
            ..Sessions::default()
        }
    }

    /// orb's Feature group `GT-514-login` holding `threads`.
    fn feature(threads: Vec<Thread>) -> Sessions {
        grouped(GroupKind::Feature, threads, false)
    }

    /// The only group in `sessions`.
    fn group_mut(sessions: &mut Sessions) -> Option<&mut Group> {
        sessions.projects.first_mut()?.groups.first_mut()
    }

    #[rstest::rstest]
    #[case::feature(GroupKind::Feature, "\u{f126}", GREEN1)]
    #[case::research(GroupKind::Research, "\u{f0c3}", BLUE2)]
    #[case::learn(GroupKind::Learn, "\u{f02d}", PURPLE)]
    fn group_card_draws_the_kind_icon_in_its_colour(
        #[case] kind: GroupKind,
        #[case] icon: &str,
        #[case] colour: Color,
    ) {
        // Given a `kind` group with one thread.
        let sessions = grouped(kind, vec![thread(1, ThreadStatus::Idle)], false);

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 12);

        // Then the card's second line starts with the kind's icon in its colour.
        assert_eq!(
            glyph(&buf, 3, 4),
            Some((icon.to_owned(), colour)),
            "the kind icon for {kind:?}"
        );
    }

    #[rstest::rstest]
    #[case::approval_over_working(
        thread(1, ThreadStatus::Working),
        thread(2, ThreadStatus::NeedsApproval),
        APPROVAL_ICON,
        YELLOW
    )]
    #[case::input_over_failed(
        thread(1, ThreadStatus::Failed),
        thread(2, ThreadStatus::NeedsInput),
        INPUT_ICON,
        MAGENTA
    )]
    #[case::failed_over_working(
        thread(1, ThreadStatus::Working),
        thread(2, ThreadStatus::Failed),
        FAILED_ICON,
        RED
    )]
    #[case::working_over_done(
        Thread { unseen: true, ..thread(1, ThreadStatus::Idle) },
        thread(2, ThreadStatus::Working),
        "⠋",
        BLUE
    )]
    #[case::done_over_idle(
        thread(1, ThreadStatus::Idle),
        Thread { unseen: true, ..thread(2, ThreadStatus::Idle) },
        COMPLETED_ICON,
        GREEN
    )]
    #[case::idle_over_stopped(
        thread(1, ThreadStatus::Stopped),
        thread(2, ThreadStatus::Idle),
        IDLE_ICON,
        DARK3
    )]
    fn group_card_rolls_up_the_most_urgent_status(
        #[case] first: Thread,
        #[case] second: Thread,
        #[case] icon: &str,
        #[case] colour: Color,
    ) {
        // Given a group with two children, the more urgent one second.
        let sessions = feature(vec![first, second]);

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 12);

        // Then the card's first line shows the more urgent child's icon.
        assert_eq!(
            glyph(&buf, 1, 3),
            Some((icon.to_owned(), colour)),
            "the rolled-up icon"
        );
    }

    #[rstest::rstest]
    fn group_card_shows_the_rolled_up_status_word() {
        // Given a group with a working child and one needing approval.
        let sessions = feature(vec![
            thread(1, ThreadStatus::Working),
            thread(2, ThreadStatus::NeedsApproval),
        ]);

        // When rendering the sidebar.
        let place = line(&draw(&sessions, at(1000), 12), 4);

        // Then the card's second line ends with `approval`.
        assert!(place.trim_end().ends_with("approval"), "line was '{place}'");
    }

    #[rstest::rstest]
    fn draft_only_group_card_shows_the_pencil_and_draft() {
        // Given a group holding only its draft.
        let sessions = grouped(GroupKind::Feature, vec![], true);

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 12);

        // Then the card's first line has the yellow pencil, and its second
        // line reads `draft`.
        assert_eq!(
            (
                glyph(&buf, 1, 3),
                line(&buf, 4).trim_end().ends_with("draft")
            ),
            (Some((PENCIL.to_owned(), YELLOW)), true),
            "the draft card was {:#?}",
            lines(&buf)
        );
    }

    #[rstest::rstest]
    #[case::feature(GroupKind::Feature, format!("{BRANCH} GT-514-login"))]
    #[case::research(GroupKind::Research, format!("{FOLDER} ~/.orb/research/tokio-cancel"))]
    fn group_card_last_line_shows_the_branch_or_folder(
        #[case] kind: GroupKind,
        #[case] expected: String,
    ) {
        // Given a `kind` group with one thread.
        let sessions = grouped(kind, vec![thread(1, ThreadStatus::Idle)], false);

        // When rendering a wide sidebar.
        let footer = line(&render_sized(&sessions, at(1000), 48, 12).0, 5);

        // Then the card's last line shows where it runs.
        assert!(
            footer.starts_with(&format!("{LAST_GUIDE}{expected} ")),
            "line was '{footer}'"
        );
    }

    #[rstest::rstest]
    fn group_card_draws_one_icon_per_child() {
        // Given a group with an idle child and a stopped one.
        let sessions = feature(vec![
            thread(1, ThreadStatus::Idle),
            thread(2, ThreadStatus::Stopped),
        ]);

        // When rendering the sidebar.
        let footer = line(&draw(&sessions, at(1000), 12), 5);

        // Then the card's last line ends with their icons, then the chevron.
        assert!(
            footer
                .trim_end()
                .ends_with(&format!(" {IDLE_ICON} {STOPPED_ICON} {FOLD_OPEN}")),
            "line was '{footer}'"
        );
    }

    #[rstest::rstest]
    fn group_card_caps_child_icons_at_eight() {
        // Given a group with ten idle children.
        let sessions = feature((1..=10).map(|id| thread(id, ThreadStatus::Idle)).collect());

        // When rendering the sidebar.
        let footer = line(&draw(&sessions, at(1000), 20), 5);

        // Then the card's last line shows eight icons, then `+2`.
        assert!(
            footer
                .trim_end()
                .ends_with(&format!("{} +2 {FOLD_OPEN}", [IDLE_ICON; 8].join(" "))),
            "line was '{footer}'"
        );
    }

    #[rstest::rstest]
    #[case::open(false, FOLD_OPEN)]
    #[case::folded(true, FOLD_CLOSED)]
    fn group_card_chevron_follows_the_fold(#[case] folded: bool, #[case] chevron: &str) {
        // Given a group with one thread, folded if `folded`.
        let sessions = Sessions {
            folded: if folded {
                HashSet::from([GroupId(9)])
            } else {
                HashSet::new()
            },
            ..feature(vec![thread(1, ThreadStatus::Idle)])
        };

        // When rendering the sidebar.
        let footer = line(&draw(&sessions, at(1000), 12), 5);

        // Then the card's last line ends with the chevron.
        assert!(footer.trim_end().ends_with(chevron), "line was '{footer}'");
    }

    #[rstest::rstest]
    fn group_card_time_is_the_working_childs_turn() {
        // Given a group with a child working since 866 s and one idle since
        // 990 s.
        let sessions = feature(vec![
            thread(1, ThreadStatus::Working),
            Thread {
                last_activity_at: at(990),
                ..thread(2, ThreadStatus::Idle)
            },
        ]);

        // When rendering the sidebar at 1000 s.
        let heading = line(&draw(&sessions, at(1000), 12), 3);

        // Then the card's first line ends with the working turn's time.
        assert!(heading.trim_end().ends_with(" 2m"), "line was '{heading}'");
    }

    #[rstest::rstest]
    fn group_card_time_is_the_latest_childs_activity() {
        // Given a group with idle children last active at 400 s and 700 s.
        let sessions = feature(vec![
            Thread {
                last_activity_at: at(400),
                ..thread(1, ThreadStatus::Idle)
            },
            Thread {
                last_activity_at: at(700),
                ..thread(2, ThreadStatus::Idle)
            },
        ]);

        // When rendering the sidebar at 1000 s.
        let heading = line(&draw(&sessions, at(1000), 12), 3);

        // Then the card's first line ends with the time since the latest.
        assert!(heading.trim_end().ends_with(" 5m"), "line was '{heading}'");
    }

    #[rstest::rstest]
    fn pinned_group_card_shows_the_pin() {
        // Given a pinned group.
        let sessions = {
            let mut sessions = feature(vec![thread(1, ThreadStatus::Idle)]);
            if let Some(group) = group_mut(&mut sessions) {
                group.pinned_at = Some(at(5));
            }
            sessions
        };

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 12);

        // Then its card's first line carries the pin, in orange.
        let pin = (0..32)
            .filter_map(|x| glyph(&buf, x, 3))
            .find(|(symbol, _)| symbol == PIN);
        assert_eq!(pin, Some((PIN.to_owned(), ORANGE)), "the pin");
    }

    #[rstest::rstest]
    fn searched_group_card_highlights_the_matched_slug() {
        // Given a group `GT-514-login`, searching for "514".
        let sessions = searching(feature(vec![thread(1, ThreadStatus::Idle)]), "514");

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 12);

        // Then the slug's `5` is blue and bold.
        let five = (0..buf.area.width)
            .filter_map(|x| buf.cell((x, 3)))
            .find(|cell| cell.symbol() == "5")
            .map(|cell| (cell.fg, cell.modifier.contains(Modifier::BOLD)));
        assert_eq!(five, Some((BLUE1, true)), "the matched `5`");
    }

    #[rstest::rstest]
    fn group_thread_rows_end_with_the_last_guide() {
        // Given a group with two children.
        let sessions = feature(vec![
            thread(1, ThreadStatus::Idle),
            thread(2, ThreadStatus::Idle),
        ]);

        // When rendering the sidebar.
        let buf = draw(&sessions, at(1000), 12);

        // Then the first child's guide branches and the last one's ends.
        let guides: Vec<_> = [6, 7]
            .into_iter()
            .map(|y| {
                [CHILD_GUIDE, LAST_CHILD_GUIDE]
                    .into_iter()
                    .find(|guide| line(&buf, y).starts_with(guide))
            })
            .collect();
        assert_eq!(
            guides,
            [Some(CHILD_GUIDE), Some(LAST_CHILD_GUIDE)],
            "lines were {:#?}",
            lines(&buf)
        );
    }

    #[rstest::rstest]
    fn group_thread_row_shows_its_status_icon_and_title() {
        // Given a group with a failed child.
        let sessions = feature(vec![thread(1, ThreadStatus::Failed)]);

        // When rendering the sidebar.
        let child = line(&draw(&sessions, at(1000), 12), 6);

        // Then its row is the guide, its icon, then its title.
        assert!(
            child.starts_with(&format!("{LAST_CHILD_GUIDE}{FAILED_ICON} Thread 1 ")),
            "line was '{child}'"
        );
    }

    #[rstest::rstest]
    fn group_draft_row_shows_the_pencil_and_new_thread() {
        // Given a group holding only its draft.
        let sessions = grouped(GroupKind::Feature, vec![], true);

        // When rendering the sidebar.
        let child = line(&draw(&sessions, at(1000), 12), 6);

        // Then its draft row is the last guide, the pencil and `New thread`.
        assert!(
            child.starts_with(&format!("{LAST_CHILD_GUIDE}{PENCIL} New thread ")),
            "line was '{child}'"
        );
    }

    #[rstest::rstest]
    fn group_draft_row_above_a_thread_draws_the_child_guide() {
        // Given a group holding its draft and thread 1.
        let sessions = grouped(
            GroupKind::Feature,
            vec![thread(1, ThreadStatus::Idle)],
            true,
        );

        // When rendering the sidebar.
        let child = line(&draw(&sessions, at(1000), 12), 6);

        // Then the draft row, first under the card, branches on.
        assert!(
            child.starts_with(&format!("{CHILD_GUIDE}{PENCIL} New thread ")),
            "line was '{child}'"
        );
    }

    /// orb's Feature group holding threads 1 and 2, settled at `at_secs` on
    /// an open shelf.
    fn settled_group(at_secs: u64) -> Sessions {
        let mut sessions = Sessions {
            shelf_open: true,
            ..feature(vec![
                thread(1, ThreadStatus::Idle),
                thread(2, ThreadStatus::Idle),
            ])
        };
        if let Some(group) = group_mut(&mut sessions) {
            group.settled_at = Some(at(at_secs));
        }
        sessions
    }

    #[rstest::rstest]
    fn settled_group_row_shows_its_kind_icon_and_slug() {
        // Given a settled Feature group, with the shelf open.
        let sessions = settled_group(700);

        // When rendering the sidebar.
        let settled = settled_lines(&draw(&sessions, at(1000), 8));

        // Then its row is the guide, the kind icon, then the slug.
        assert!(
            settled
                .first()
                .is_some_and(|row| row.starts_with(&format!("{LAST_GUIDE}\u{f126} GT-514-login "))),
            "lines were {settled:#?}"
        );
    }

    #[rstest::rstest]
    fn settled_group_row_shows_its_thread_count_and_age() {
        // Given a two-thread group settled 5 minutes before now, with the
        // shelf open.
        let sessions = settled_group(700);

        // When rendering the sidebar.
        let settled = settled_lines(&draw(&sessions, at(1000), 8));

        // Then its row ends with the thread count and the time since.
        assert!(
            settled
                .first()
                .is_some_and(|row| row.trim_end().ends_with(" 2 · 5m")),
            "lines were {settled:#?}"
        );
    }

    #[rstest::rstest]
    #[case::thread_then_group(false)]
    #[case::open_group_then_thread(true)]
    fn settled_entry_followed_by_another_draws_the_middle_guide(#[case] group_first: bool) {
        // Given a settled group and a settled thread on an open shelf, the
        // group first (and open) if `group_first`.
        let sessions = {
            let (group_at, thread_at) = if group_first { (30, 20) } else { (20, 30) };
            let mut sessions = settled_group(group_at);
            if let Some(project) = sessions.projects.first_mut() {
                project.threads.push(settled(5, thread_at));
            }
            if group_first {
                sessions.opened.insert(GroupId(9));
            }
            sessions
        };

        // When rendering the sidebar.
        let settled = settled_lines(&draw(&sessions, at(1000), 14));

        // Then the first settled entry draws the middle guide.
        assert!(
            settled
                .first()
                .is_some_and(|row| row.starts_with(GUIDE) && !row.starts_with(LAST_GUIDE)),
            "lines were {settled:#?}"
        );
    }

    #[rstest::rstest]
    #[case::two_threads(feature(vec![thread(1, ThreadStatus::Idle), thread(2, ThreadStatus::Idle)]), "2/2")]
    #[case::draft_only(grouped(GroupKind::Feature, vec![], true), "1/1")]
    fn group_count_leaves_cards_out(#[case] sessions: Sessions, #[case] expected: &str) {
        // Given a sidebar listing one group.

        // When rendering the sidebar.
        let prompt = line(&draw(&sessions, at(1000), 12), 1);

        // Then the count covers the group's children, not its card.
        assert!(
            prompt.trim_end().ends_with(&format!("{expected}│")),
            "line was '{prompt}'"
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
    #[case::second_line(3)]
    #[case::third_line(4)]
    fn hit_map_maps_a_scrolled_rows_lower_lines_to_it(#[case] y: u16) {
        // Given three threads on an 8-line sidebar scrolled to the last one
        // (thread 1), so thread 2's lower two lines top the list.
        let sessions = three_threads(1);

        // When rendering the sidebar.
        let (_, hits) = render_with(&sessions, &mut SidebarScroll::default(), 8);

        // Then a click on either of those lines lands on thread 2.
        assert_eq!(
            hits.row_at(Position::new(1, y)),
            Some(SidebarItem::Thread(ThreadId(2))),
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

        // Then the last line shows the list's last row, thread 1.
        assert_eq!(
            hits.row_at(Position::new(1, 7)),
            Some(SidebarItem::Thread(ThreadId(1))),
            "the row on the bottom line"
        );
    }

    #[rstest::rstest]
    fn free_scroll_snaps_back_once_the_selection_moved() {
        // Given the view wheeled down while thread 2 was selected, and
        // thread 3 selected since.
        let sessions = three_threads(3);
        let mut scroll = SidebarScroll::default();
        scroll.scroll_free(3, Some(SidebarItem::Thread(ThreadId(2))));

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
}
