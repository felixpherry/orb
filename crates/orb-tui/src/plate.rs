//! The plate: the start screen, what the right-hand area shows while no
//! session is shown, and a float over the shown session while it has the
//! keys.
//!
//! As the start screen the plate fills the right-hand area inside a small
//! margin; as a float it covers most of the shown session's tab body,
//! centred, the tab bar and the sidebar still drawn around it. Either way it
//! is a list box
//! titled Plate beside a preview of the selected row, or the preview under
//! the list when the area is narrow. The list holds four groups — Your move,
//! Waiting on others, Parked and Done today — each under a header with its
//! count, Done today folded until it's opened; its bottom border counts what
//! is on the user's plate and how many agents are working. Each row shows
//! its glyph, its verb's tag, its title, and on a wide list its session's
//! title, then its age. The preview shows the selected row's state, title,
//! description, numbered links, where it came from and its keys. Clicks on
//! the plate do nothing; while it has the keys, a click off it closes it.

use std::time::{Duration, SystemTime};

use jiff::tz::TimeZone;
use orb_domain::AppState;
use orb_domain::feat::harness::{HarnessId, HarnessInfo};
use orb_domain::feat::plate::{
    Board, BoardRow, Group, GroupKind, ItemState, LinkKind, OrbAsk, RowKey, RowSource, Verb,
    local_midnight,
};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Widget};

use crate::session_picker::{boxed, cut_right, wrap_words};
use crate::sidebar::{
    APPROVAL_ICON, BG_DARK, BLACK, BLUE, BLUE1, COMMENT, CYAN, DARK3, DARK5, FG, FG_DARK, GREEN,
    GREEN1, GUTTER, INPUT_ICON, MAGENTA, ORANGE, RED, VISUAL, YELLOW, ago_label, render_split,
};
use crate::tabs;

/// The list box's title icon, also before the mode line's Your move count.
pub(crate) const PLATE_ICON: &str = "\u{f0ae}";
/// An open group's header.
const FOLD_OPEN: &str = "\u{f47c}";
/// A folded group's header.
const FOLD_CLOSED: &str = "\u{f460}";
/// An urgent Your move item.
const URGENT_ICON: &str = "\u{f06a}";
/// A Your move item.
const MOVE_ICON: &str = "●";
/// An item waiting on others.
const WAIT_ICON: &str = "\u{f252}";
/// An idea an agent offered.
const IDEA_ICON: &str = "\u{f0eb}";
/// An item the user parked.
const PARKED_ICON: &str = "\u{f04c}";
/// A done item.
const DONE_ICON: &str = "\u{f00c}";

/// Below this many columns the preview goes under the list.
const WIDE: u16 = 100;
/// From this many columns inside the list box, rows show their session.
const SESSION_COLUMNS: usize = 70;
/// The room a row's session title takes, its gap included.
const SESSION_ROOM: usize = 22;

/// Draws the plate as the start screen in `area` (the right-hand side),
/// inside a 2-column, 1-row margin, with Done today counted from local
/// midnight of `now` in `tz`. Returns `area`, all of which is the plate's,
/// and how many rows the list shows.
pub(crate) fn render(
    state: &AppState,
    now: SystemTime,
    tz: &TimeZone,
    area: Rect,
    buf: &mut Buffer,
) -> (Rect, usize) {
    let board = Board::new(state, local_midnight(now, tz));
    let outer = Rect {
        x: area.x.saturating_add(2),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(2),
    };
    (area, render_boxes(&board, state, now, outer, buf))
}

/// Draws the plate floating over the shown session: a cleared rect 92% ×
/// 86% of the tab body inside `right`, centred, holding the list and the
/// preview, with Done today counted from local midnight of `now` in `tz`.
/// Returns that rect and how many rows the list shows.
pub(crate) fn render_float(
    state: &AppState,
    now: SystemTime,
    tz: &TimeZone,
    right: Rect,
    buf: &mut Buffer,
) -> (Rect, usize) {
    let [_, body] = tabs::areas(right);
    let rect = float_rect(body);
    Clear.render(rect, buf);
    let board = Board::new(state, local_midnight(now, tz));
    (rect, render_boxes(&board, state, now, rect, buf))
}

/// The float's rect in the tab `body`: 92% of its width and 86% of its
/// height, centred.
fn float_rect(body: Rect) -> Rect {
    let share =
        |size: u16, percent: u32| u16::try_from(u32::from(size) * percent / 100).unwrap_or(size);
    let width = share(body.width, 92);
    let height = share(body.height, 86);
    Rect {
        x: body.x + (body.width - width) / 2,
        y: body.y + (body.height - height) / 2,
        width,
        height,
    }
}

/// Draws `board`'s list box and preview box in `outer`: side by side from
/// [`WIDE`] columns, the list taking 52% (at least 40 columns), else the
/// list over the preview, 60/40. Returns how many rows the list shows.
fn render_boxes(
    board: &Board<'_>,
    state: &AppState,
    now: SystemTime,
    outer: Rect,
    buf: &mut Buffer,
) -> usize {
    if outer.is_empty() {
        return 0;
    }
    let [list_area, preview_area] = if outer.width >= WIDE {
        let share = u16::try_from(u32::from(outer.width) * 52 / 100).unwrap_or(outer.width);
        let list_w = share.clamp(40.min(outer.width), outer.width);
        Layout::horizontal([Constraint::Length(list_w), Constraint::Fill(1)]).areas(outer)
    } else {
        Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(outer)
    };
    let page = render_list(board, state, now, list_area, buf);
    match board.selected_row() {
        Some(row) => render_preview(row, group_of(board, row.key), state, now, preview_area, buf),
        None => render_nothing(preview_area, buf),
    }
    page
}

/// The group the row with `key` sits in.
fn group_of(board: &Board<'_>, key: RowKey) -> GroupKind {
    board
        .groups
        .iter()
        .find(|group| group.rows.iter().any(|row| row.key == key))
        .map_or(GroupKind::YourMove, |group| group.kind)
}

/// One line of the list: its left and right halves, and the row it shows
/// when it shows one.
struct ListLine {
    left: Line<'static>,
    right: Line<'static>,
    key: Option<RowKey>,
}

impl ListLine {
    fn plain(left: Line<'static>) -> Self {
        Self {
            left,
            right: Line::default(),
            key: None,
        }
    }
}

/// Draws the list box: the title, the groups and their rows, the counts in
/// the bottom border. Returns how many rows it shows.
fn render_list(
    board: &Board<'_>,
    state: &AppState,
    now: SystemTime,
    area: Rect,
    buf: &mut Buffer,
) -> usize {
    let title = Line::from(vec![
        span(format!(" {PLATE_ICON} "), ORANGE),
        Span::styled("Plate ", bold(FG)),
    ]);
    let footer = format!(
        " {} your move · {} waiting · {} parked · {} working ",
        board.count(GroupKind::YourMove),
        board.count(GroupKind::Waiting),
        board.count(GroupKind::Parked),
        state.sessions.working_count(),
    );
    let block = boxed(Some(title)).title_bottom(Line::from(span(footer, COMMENT)).right_aligned());
    let inner = block.inner(area);
    block.render(area, buf);
    let inner = Rect {
        x: inner.x.saturating_add(1),
        y: inner.y.saturating_add(1),
        width: inner.width.saturating_sub(2),
        height: inner.height.saturating_sub(1),
    };
    let width = usize::from(inner.width);
    let session_room = if width >= SESSION_COLUMNS {
        SESSION_ROOM
    } else {
        0
    };
    let title_room = width.saturating_sub(4 + 9 + session_room + 5);
    let mut lines = Vec::new();
    for (n, group) in board.groups.iter().enumerate() {
        if n > 0 {
            lines.push(ListLine::plain(Line::default()));
        }
        lines.push(ListLine::plain(group_head(group)));
        if !group.open {
            continue;
        }
        for row in &group.rows {
            lines.push(list_row(row, group.kind, now, title_room, session_room));
        }
    }
    draw_lines(&lines, board.selected, inner, buf);
    usize::from(inner.height)
}

/// A group's header: its fold icon, its name bold in its colour and its
/// count; `z opens` after a folded one, and the empty Your move's nudge.
fn group_head(group: &Group<'_>) -> Line<'static> {
    let (name, colour) = match group.kind {
        GroupKind::YourMove => ("Your move", ORANGE),
        GroupKind::Waiting => ("Waiting on others", CYAN),
        GroupKind::Parked => ("Parked", MAGENTA),
        GroupKind::DoneToday => ("Done today", GREEN),
    };
    let mut head = vec![
        span(if group.open { FOLD_OPEN } else { FOLD_CLOSED }, DARK3),
        span(" ", FG),
        Span::styled(name, bold(colour)),
        span(format!("  {}", group.rows.len()), COMMENT),
    ];
    if !group.open {
        head.push(span("  z opens", DARK3));
    }
    if group.kind == GroupKind::YourMove && group.rows.is_empty() {
        head.push(span(
            "  nothing needs you, a good time for a parked idea",
            GREEN,
        ));
    }
    Line::from(head)
}

/// `row`'s line in `kind`: glyph, tag and title on the left; its session's
/// title (when `session_room` isn't 0) and its age on the right.
fn list_row(
    row: &BoardRow<'_>,
    kind: GroupKind,
    now: SystemTime,
    title_room: usize,
    session_room: usize,
) -> ListLine {
    let tag = format!("{:<8}", row.verb.to_string());
    let tag = match kind {
        GroupKind::YourMove => Span::styled(tag, bold(verb_colour(row.verb))),
        GroupKind::Parked => span(tag, verb_colour(row.verb)),
        GroupKind::Waiting | GroupKind::DoneToday => span(" ".repeat(8), DARK3),
    };
    let title_style = match kind {
        GroupKind::YourMove => bold(FG),
        GroupKind::Waiting | GroupKind::Parked => Style::new().fg(FG_DARK),
        GroupKind::DoneToday => Style::new().fg(COMMENT),
    };
    let mut right = Vec::new();
    if session_room > 0 {
        right.push(span(cut_right(&row.session_title, session_room - 2), DARK5));
    }
    if let Some(since) = row.since {
        right.push(span(
            format!("{:>4}", ago_label(elapsed(now, since))),
            COMMENT,
        ));
    }
    ListLine {
        left: Line::from(vec![
            span("  ", FG),
            glyph(row, kind),
            span(" ", FG),
            tag,
            span(" ", FG),
            Span::styled(cut_right(&row.title, title_room), title_style),
        ]),
        right: Line::from(right),
        key: Some(row.key),
    }
}

/// Draws `lines` in `area` from the offset that keeps the `selected` row in
/// view, its cells filled with the selection colour. While lines are cut
/// off below, the bottom line says how many.
fn draw_lines(lines: &[ListLine], selected: Option<RowKey>, area: Rect, buf: &mut Buffer) {
    let height = usize::from(area.height);
    if height == 0 {
        return;
    }
    let at = lines
        .iter()
        .position(|line| line.key.is_some() && line.key == selected)
        .unwrap_or(0);
    let offset = if lines.len() <= height {
        0
    } else {
        let room = height.saturating_sub(1).max(1);
        (at + 1).saturating_sub(room).min(lines.len() - height)
    };
    for (y, line) in (area.y..area.bottom()).zip(lines.iter().skip(offset)) {
        let rect = Rect::new(area.x, y, area.width, 1);
        if line.key.is_some() && line.key == selected {
            buf.set_style(rect, Style::new().bg(VISUAL));
        }
        render_split(line.left.clone(), line.right.clone(), rect, buf);
    }
    let below = lines.len().saturating_sub(offset + height);
    if below > 0 && height > 1 {
        let rect = Rect::new(area.x, area.bottom() - 1, area.width, 1);
        Clear.render(rect, buf);
        buf.set_style(rect, Style::new().bg(BG_DARK));
        Line::from(span(format!("  ⌄ {} more lines below", below + 1), DARK3)).render(rect, buf);
    }
}

/// Draws the preview of `row`, in group `kind`, in `area`: titled with its
/// session and project, then its state and age, its title, its description,
/// its links and where it came from, with its keys on the second-to-last row.
fn render_preview(
    row: &BoardRow<'_>,
    kind: GroupKind,
    state: &AppState,
    now: SystemTime,
    area: Rect,
    buf: &mut Buffer,
) {
    let mut title = vec![
        span(" ", FG),
        Span::styled(row.session_title.clone(), bold(FG)),
    ];
    match &row.project {
        Some(project) => title.push(span(format!(" · {project} "), COMMENT)),
        None => title.push(span(" ", FG)),
    }
    let block = boxed(Some(Line::from(title))).padding(Padding::new(2, 2, 1, 0));
    let inner = block.inner(area);
    block.render(area, buf);
    let width = usize::from(inner.width);
    let mut text = vec![state_line(row, kind, now), Line::default()];
    text.extend(wrapped(&row.title, width, bold(FG)));
    if let RowSource::Item(item) = row.source
        && let Some(what) = item.what.as_deref().filter(|what| !what.is_empty())
    {
        text.push(Line::default());
        text.extend(wrapped(what, width, Style::new().fg(FG_DARK)));
    }
    text.push(Line::default());
    text.extend(link_lines(row, width));
    text.push(Line::default());
    text.push(source_line(row, &state.harnesses, now));
    let text_area = Rect {
        height: inner.height.saturating_sub(2),
        ..inner
    };
    Paragraph::new(text).render(text_area, buf);
    put(
        chips(row, kind),
        inner.x,
        inner.bottom().saturating_sub(2),
        inner,
        buf,
    );
}

/// The preview's first line: the glyph, the bold tag, the state and the age.
fn state_line(row: &BoardRow<'_>, kind: GroupKind, now: SystemTime) -> Line<'static> {
    let words = match kind {
        GroupKind::YourMove => "your move",
        GroupKind::Waiting => "waiting on others",
        GroupKind::Parked => "parked",
        GroupKind::DoneToday => "done",
    };
    let tail = [Some(words.to_owned()), age(row, now)];
    Line::from(vec![
        glyph(row, kind),
        Span::styled(format!(" {}", row.verb), bold(verb_colour(row.verb))),
        span(joined(tail), COMMENT),
    ])
}

/// `text` wrapped to `width` in `style`.
fn wrapped(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let words = text
        .split(' ')
        .filter(|word| !word.is_empty())
        .map(|word| vec![Span::styled(word.to_owned(), style)])
        .collect();
    wrap_words(words, width, &[], &[])
}

/// One numbered line per link of `row`'s item: its number, its kind's icon,
/// its label and its address; `no links` when there are none.
fn link_lines(row: &BoardRow<'_>, width: usize) -> Vec<Line<'static>> {
    let links = match row.source {
        RowSource::Item(item) => item.links(),
        RowSource::Orb { .. } => Vec::new(),
    };
    if links.is_empty() {
        return vec![Line::from(span("no links", DARK3))];
    }
    links
        .iter()
        .enumerate()
        .map(|(n, link)| {
            let (icon, colour) = link_icon(link.kind);
            Line::from(vec![
                span(format!("{:<3}", n + 1), ORANGE),
                span(format!("{icon} "), colour),
                span(padded(&cut_right(&link.label, 15), 16), FG),
                Span::styled(
                    cut_right(&link.address, width.saturating_sub(22)),
                    Style::new().fg(COMMENT).add_modifier(Modifier::UNDERLINED),
                ),
            ])
        })
        .collect()
}

/// Where `row` came from, then its project and age.
fn source_line(row: &BoardRow<'_>, harnesses: &[HarnessInfo], now: SystemTime) -> Line<'static> {
    let agent = agent_name(row.harness.as_ref(), harnesses);
    let session = &row.session_title;
    let text = match row.source {
        RowSource::Orb { .. } => format!("orb saw {agent} waiting in {session}"),
        RowSource::Item(item) => match item.state {
            ItemState::Parked { by_user: false } => format!("offered by {agent} in {session}"),
            ItemState::Parked { by_user: true } => {
                format!("you parked it · from {agent} in {session}")
            }
            _ if row.harness.is_none() => format!("added from a shell in {session}"),
            _ => format!("added by {agent} in {session}"),
        },
    };
    let tail = [row.project.clone(), age(row, now)];
    Line::from(vec![span(text, DARK5), span(joined(tail), DARK3)])
}

/// The harness's id and mark (`claude ✳`), or `a shell` with none.
fn agent_name(harness: Option<&HarnessId>, harnesses: &[HarnessInfo]) -> String {
    let Some(id) = harness else {
        return "a shell".to_owned();
    };
    match harnesses
        .iter()
        .find(|info| &info.id == id)
        .and_then(|info| info.icon.as_deref())
    {
        Some(icon) => format!("{id} {icon}"),
        None => id.to_string(),
    }
}

/// The keys the preview offers for `row` in `kind`.
fn chips(row: &BoardRow<'_>, kind: GroupKind) -> Line<'static> {
    let mut spans = vec![key_chip("⏎"), span(" go there", FG_DARK)];
    if let RowSource::Orb { .. } = row.source {
        return Line::from(spans);
    }
    spans.extend([span("   ", FG_DARK), key_chip("gx"), span(" link", FG_DARK)]);
    if kind == GroupKind::DoneToday {
        return Line::from(spans);
    }
    let park = match kind {
        GroupKind::Parked => " unpark",
        _ => " park",
    };
    spans.extend([
        span("   ", FG_DARK),
        key_chip("x"),
        span(" done   ", FG_DARK),
        key_chip("p"),
        span(park, FG_DARK),
    ]);
    Line::from(spans)
}

/// The preview with no row to show.
fn render_nothing(area: Rect, buf: &mut Buffer) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(GUTTER).bg(BG_DARK))
        .style(Style::new().bg(BG_DARK));
    let inner = block.inner(area);
    block.render(area, buf);
    put(
        Line::from(span("Nothing to show", DARK3)),
        inner.x.saturating_add(2),
        inner.y.saturating_add(1),
        inner,
        buf,
    );
}

/// `row`'s glyph in `kind`.
fn glyph(row: &BoardRow<'_>, kind: GroupKind) -> Span<'static> {
    let (icon, colour) = match row.source {
        RowSource::Orb {
            ask: OrbAsk::Approval,
            ..
        } => (APPROVAL_ICON, YELLOW),
        RowSource::Orb {
            ask: OrbAsk::Answer,
            ..
        } => (INPUT_ICON, MAGENTA),
        RowSource::Item(item) => match kind {
            GroupKind::YourMove if item.urgent => (URGENT_ICON, RED),
            GroupKind::YourMove => (MOVE_ICON, ORANGE),
            GroupKind::Waiting => (WAIT_ICON, COMMENT),
            GroupKind::Parked if item.state == (ItemState::Parked { by_user: false }) => {
                (IDEA_ICON, MAGENTA)
            }
            GroupKind::Parked => (PARKED_ICON, DARK5),
            GroupKind::DoneToday => (DONE_ICON, DARK3),
        },
    };
    span(icon, colour)
}

/// The colour of `verb`'s tag.
fn verb_colour(verb: Verb) -> Color {
    match verb {
        Verb::Decide => RED,
        Verb::Review => BLUE,
        Verb::Walk => GREEN1,
        Verb::Pick | Verb::Answer | Verb::Idea => MAGENTA,
        Verb::Read => CYAN,
        Verb::Approve => YELLOW,
        Verb::Reply => BLUE1,
        Verb::Merge => GREEN,
        Verb::Note => COMMENT,
    }
}

/// The icon and colour of a link of `kind`.
fn link_icon(kind: LinkKind) -> (&'static str, Color) {
    match kind {
        LinkKind::Jira => ("\u{e75c}", BLUE1),
        LinkKind::PullRequest => ("\u{f407}", MAGENTA),
        LinkKind::Sentry => ("\u{f188}", ORANGE),
        LinkKind::Grafana => ("\u{f201}", GREEN1),
        LinkKind::File => ("\u{f15c}", FG_DARK),
        LinkKind::Web => ("\u{f0ac}", BLUE),
    }
}

/// A key's chip: ` k `, bold black on blue.
fn key_chip(key: &str) -> Span<'static> {
    Span::styled(
        format!(" {key} "),
        Style::new().fg(BLACK).bg(BLUE).add_modifier(Modifier::BOLD),
    )
}

/// How long before `now` `at` was; zero if the clock went backwards.
fn elapsed(now: SystemTime, at: SystemTime) -> Duration {
    now.duration_since(at).unwrap_or_default()
}

/// How long ago `row`'s age started (`3m ago`); `None` when unknown.
fn age(row: &BoardRow<'_>, now: SystemTime) -> Option<String> {
    row.since.map(|since| long_ago(elapsed(now, since)))
}

/// Each of `parts` that's there, after ` · `.
fn joined<const N: usize>(parts: [Option<String>; N]) -> String {
    let mut text = String::new();
    for part in parts.into_iter().flatten() {
        text.push_str(" · ");
        text.push_str(&part);
    }
    text
}

/// `just now` under a minute, else `3m ago`.
fn long_ago(elapsed: Duration) -> String {
    match elapsed.as_secs() {
        0..60 => "just now".to_owned(),
        _ => format!("{} ago", ago_label(elapsed)),
    }
}

/// `text` followed by spaces to fill `width` columns.
fn padded(text: &str, width: usize) -> String {
    let used = Line::raw(text).width();
    format!("{text}{}", " ".repeat(width.saturating_sub(used)))
}

fn span<T>(text: T, fg: Color) -> Span<'static>
where
    T: Into<String>,
{
    Span::styled(text.into(), Style::new().fg(fg))
}

fn bold(fg: Color) -> Style {
    Style::new().fg(fg).add_modifier(Modifier::BOLD)
}

/// Draws `line` from `(x, y)` to `area`'s right edge; nothing when that cell
/// is outside `area`.
fn put(line: Line<'_>, x: u16, y: u16, area: Rect, buf: &mut Buffer) {
    if area.contains(Position::new(x, y)) {
        line.render(Rect::new(x, y, area.right() - x, 1), buf);
    }
}

#[cfg(test)]
mod tests;
