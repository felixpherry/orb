//! Dashboard — what the right-hand area shows while no Claude pane is up.
//!
//! LazyVim's start screen, for orb: the word ORB in shadowed block letters
//! fading from blue to violet, with a trail of moons and a few stars; a line
//! naming the selection (the shelf hint on the Settled header); the menu of
//! what the selection can do, each item with its icon, its current value
//! where it has one and its key against the right edge; and a footer counting
//! working threads, threads and projects. Why a session couldn't start shows
//! under the footer. Everything is centred, and the blank lines between items
//! go when the area is too short for them.

use std::borrow::Cow;
use std::path::Path;

use orb_domain::feat::dashboard::{DashboardItem, items};
use orb_domain::feat::picker::list::setting_label;
use orb_domain::feat::sessions::state::{
    Draft, DraftWorkspace, GroupKind, NEW_THREAD, SidebarItem, SidebarRow,
};
use orb_domain::{AppState, tilde};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use unicode_segmentation::UnicodeSegmentation;

use crate::sidebar::{
    self, BLUE, BLUE1, BRANCH, CLAUDE, CLAUDE_LOGO, COMMENT, CYAN, DARK3, DARK5, FG, FG_DARK,
    FOLDER, MAGENTA, ORANGE, YELLOW,
};

/// tokyonight-moon's `red`.
const RED: Color = Color::Rgb(0xff, 0x75, 0x7f);

// Nerd Font glyphs.
const PLUS: &str = "\u{f067}";
const FUNNEL: &str = "\u{f0b0}";
const FORK: &str = "\u{f126}";
const TERMINAL: &str = "\u{f120}";
const GIT: &str = "\u{f1d3}";
const VIM: &str = "\u{e62b}";
const CHIP: &str = "\u{f2db}";
const SHIELD: &str = "\u{f132}";
const EXIT: &str = "\u{f08b}";
const ROCKET: &str = "\u{f135}";

/// ORB in ANSI Shadow, as LazyVim's header.
const SHADOW: [&str; 6] = [
    " ██████╗ ██████╗ ██████╗ ",
    "██╔═══██╗██╔══██╗██╔══██╗",
    "██║   ██║██████╔╝██████╔╝",
    "██║   ██║██╔══██╗██╔══██╗",
    "╚██████╔╝██║  ██║██████╔╝",
    " ╚═════╝ ╚═╝  ╚═╝╚═════╝ ",
];
const SHADOW_WIDTH: u16 = 25;

/// Moons rising off the B like LazyVim's z's, then the stars around the
/// banner, as `(x, y, glyph, colour)` from the banner's top-left cell.
const SKY: [(i16, i16, &str, Color); 10] = [
    (27, 4, "·", DARK5),
    (30, 3, "∘", COMMENT),
    (33, 2, "○", BLUE),
    (37, 0, "●", YELLOW),
    (-6, 1, "✦", DARK5),
    (-10, 4, "·", DARK3),
    (-3, 5, "⋆", COMMENT),
    (42, 4, "✧", DARK5),
    (22, -1, "·", DARK3),
    (4, -1, "⋆", DARK3),
];

/// Rows besides the menu: the banner, a blank, the context line and a blank
/// above it; a blank, the footer and a blank below it.
const CHROME_HEIGHT: u16 = 12;

/// Draws the dashboard into `area` with `pane_error` under the footer.
/// Returns where the cursor goes: the first cell of the highlighted item's
/// label.
pub(crate) fn render(
    state: &AppState,
    pane_error: Option<&str>,
    area: Rect,
    buf: &mut Buffer,
) -> Position {
    let items = items(&state.sessions);
    let cursor = state.dashboard.index(&state.sessions, items.len());
    let count = u16::try_from(items.len()).unwrap_or(u16::MAX);
    let menu_height = |gap: u16| (count * (gap + 1)).saturating_sub(gap);
    let gap = u16::from(menu_height(1) + CHROME_HEIGHT <= area.height);
    let top = area.y + area.height.saturating_sub(menu_height(gap) + CHROME_HEIGHT) / 2;
    banner(
        area.x + area.width.saturating_sub(SHADOW_WIDTH) / 2,
        top,
        area,
        buf,
    );
    centre(context(state, usize::from(area.width)), top + 7, area, buf);
    let menu_width = 60.min(area.width.saturating_sub(6));
    let menu_x = area.x + area.width.saturating_sub(menu_width) / 2;
    let menu_y = top + 9;
    let mut at = Position::new(menu_x + 3, menu_y);
    for (index, item) in items.iter().enumerate() {
        let y = menu_y + index as u16 * (gap + 1);
        entry(
            state,
            *item,
            Position::new(menu_x, y),
            menu_width,
            area,
            buf,
        );
        if index == cursor {
            at.y = y;
        }
    }
    let footer = menu_y + menu_height(gap) + 2;
    centre(stats(state), footer, area, buf);
    if let Some(error) = pane_error {
        centre(
            Line::from(span(error.to_owned(), RED)),
            footer + 1,
            area,
            buf,
        );
    }
    at
}

/// The ORB letters with its moons and stars, the letters' top-left at
/// `(x, top)`: solid blocks from blue to violet row by row, their shadow in
/// darker tones of the same.
fn banner(x: u16, top: u16, area: Rect, buf: &mut Buffer) {
    for (row, text) in (0u16..).zip(SHADOW) {
        let t = f64::from(row) / 5.0;
        let solid = lerp(BLUE, MAGENTA, t);
        let edge = lerp(
            Color::Rgb(0x3b, 0x4f, 0x8c),
            Color::Rgb(0x5a, 0x45, 0x8c),
            t,
        );
        let spans: Vec<Span<'static>> = text
            .graphemes(true)
            .map(|cell| span(cell, if cell == "█" { solid } else { edge }))
            .collect();
        put(Line::from(spans), x, top + row, area, buf);
    }
    for (dx, dy, glyph, fg) in SKY {
        if let (Some(x), Some(y)) = (x.checked_add_signed(dx), top.checked_add_signed(dy)) {
            put(Line::from(span(glyph, fg)), x, y, area, buf);
        }
    }
}

/// What the selection is: the thread's project, title and branch, a draft's
/// project and branch, a group's kind, `<project>/<slug>` and branch (the
/// title is the kind's name on the card, `New thread` on its draft), the
/// shelf hint on the Settled header, or that nothing is selected. A long
/// title is cut to fit `max` columns.
fn context(state: &AppState, max: usize) -> Line<'static> {
    let sessions = &state.sessions;
    let (project, title, branch) = match (
        sessions.selected_thread(),
        sessions.selected_draft(),
        sessions.selected_group(),
    ) {
        (Some(thread), _, _) => (
            sessions
                .selected_project()
                .map(|project| (FOLDER, BLUE, project.title.clone())),
            thread.title.as_deref().unwrap_or(NEW_THREAD),
            thread.branch.as_deref(),
        ),
        (None, Some((project, draft)), _) => (
            Some((FOLDER, BLUE, project.title.clone())),
            NEW_THREAD,
            draft.branch.as_deref(),
        ),
        (None, None, Some((project, group))) => {
            let (icon, colour) = sidebar::kind_look(group.kind);
            let title = match sessions.cursor {
                Some(SidebarItem::GroupDraft(_)) => NEW_THREAD,
                _ => kind_title(group.kind),
            };
            (
                Some((icon, colour, format!("{}/{}", project.title, group.name))),
                title,
                group.branch.as_deref(),
            )
        }
        (None, None, None) => {
            return match sessions.cursor {
                Some(SidebarItem::SettledShelf) => Line::from(span(shelf_hint(state), FG_DARK)),
                _ => Line::from(span("no session selected", COMMENT)),
            };
        }
    };
    let mut spans = Vec::new();
    if let Some((icon, colour, text)) = project {
        spans.extend([
            span(format!("{icon} "), colour),
            span(text, FG_DARK),
            span("  ·  ", DARK3),
        ]);
    }
    spans.push(span(fit(title, max.saturating_sub(40).max(12)), FG));
    if let Some(branch) = branch {
        spans.extend([
            span("  ·  ", DARK3),
            span(format!("{BRANCH} "), MAGENTA),
            span(branch.to_owned(), FG_DARK),
        ]);
    }
    Line::from(spans)
}

/// A group kind's name, as the dashboard titles its card.
fn kind_title(kind: GroupKind) -> &'static str {
    match kind {
        GroupKind::Feature => "Feature group",
        GroupKind::Research => "Research group",
        GroupKind::Learn => "Learn group",
    }
}

/// What `⏎` does on the Settled header: `▸ Settled (N) · ⏎ open`, or
/// `▾ Settled · ⏎ close` while the shelf is open.
fn shelf_hint(state: &AppState) -> String {
    let sessions = &state.sessions;
    let count = sessions
        .sidebar()
        .iter()
        .find_map(|row| match row {
            SidebarRow::ShelfHeader { count, .. } => Some(*count),
            _ => None,
        })
        .unwrap_or_default();
    let action = if sessions.shelf_open { "close" } else { "open" };
    format!(
        "{} · ⏎ {action}",
        sidebar::shelf_label(count, sessions.shelf_open)
    )
}

/// One menu row `width` columns wide from `at`: icon, label, the item's value
/// dimmed and cut to the room left, and its key against the right edge.
fn entry(
    state: &AppState,
    item: DashboardItem,
    at: Position,
    width: u16,
    area: Rect,
    buf: &mut Buffer,
) {
    let (icon, label) = look(item);
    let icon_fg = match item {
        DashboardItem::Open | DashboardItem::Start => CLAUDE,
        _ => BLUE1,
    };
    let room = usize::from(width).saturating_sub(label.len() + 9);
    let mut spans = vec![span(icon, icon_fg), span("  ", FG), span(label, CYAN)];
    if let Some(value) = value(state, item).filter(|_| room > 4) {
        spans.extend([span("  ", FG), span(fit(&value, room), COMMENT)]);
    }
    put(Line::from(spans), at.x, at.y, area, buf);
    put(
        Line::from(span(item.key().to_string(), ORANGE)),
        at.x + width.saturating_sub(1),
        at.y,
        area,
        buf,
    );
}

/// An item's icon and label.
fn look(item: DashboardItem) -> (&'static str, &'static str) {
    match item {
        DashboardItem::Open => (CLAUDE_LOGO, "Open session"),
        DashboardItem::Start => (ROCKET, "Start session"),
        DashboardItem::Workspace => (FORK, "Workspace"),
        DashboardItem::Branch => (BRANCH, "Branch"),
        DashboardItem::Model => (CHIP, "Model"),
        DashboardItem::Permission => (SHIELD, "Permission"),
        DashboardItem::NewSession => (PLUS, "New session"),
        DashboardItem::AddProject => (FOLDER, "Add project"),
        DashboardItem::FilterProjects => (FUNNEL, "Filter projects"),
        DashboardItem::Shell => (TERMINAL, "Shell"),
        DashboardItem::Lazygit => (GIT, "Lazygit"),
        DashboardItem::Neovim => (VIM, "Neovim"),
        DashboardItem::Quit => (EXIT, "Quit"),
    }
}

/// An item's current value: the selected thread's directory and branch, the
/// selected draft's workspace, branch, model and permission, or a group
/// draft's model and permission.
fn value(state: &AppState, item: DashboardItem) -> Option<String> {
    let sessions = &state.sessions;
    let home = &state.home;
    match (item, sessions.selected_draft(), sessions.selected_thread()) {
        (DashboardItem::Workspace, Some((_, draft)), _) => {
            Some(workspace_label(&draft.workspace, home))
        }
        (DashboardItem::Workspace, None, Some(thread)) => Some(tilde(&thread.cwd, home)),
        (DashboardItem::Branch, Some((_, draft)), _) => Some(branch_label(draft)),
        (DashboardItem::Branch, None, Some(thread)) => thread.branch.clone(),
        (DashboardItem::Branch, None, None) => sessions.selected_group()?.1.branch.clone(),
        (DashboardItem::Model, Some((_, draft)), _) => {
            Some(setting_label(draft.model.as_deref()).to_owned())
        }
        (DashboardItem::Permission, Some((_, draft)), _) => {
            Some(setting_label(draft.permission.as_deref()).to_owned())
        }
        (DashboardItem::Model, None, None) => sessions
            .selected_group_draft()
            .map(|(_, _, draft)| setting_label(draft.model.as_deref()).to_owned()),
        (DashboardItem::Permission, None, None) => sessions
            .selected_group_draft()
            .map(|(_, _, draft)| setting_label(draft.permission.as_deref()).to_owned()),
        _ => None,
    }
}

/// Where a draft's session will run: a word or two, or an existing
/// worktree's path with `home` as `~`.
fn workspace_label(workspace: &DraftWorkspace, home: &Path) -> String {
    match workspace {
        DraftWorkspace::Local => "Local checkout".to_owned(),
        DraftWorkspace::NewWorktree => "New worktree".to_owned(),
        DraftWorkspace::Existing(path) => tilde(path, home),
    }
}

/// T3 Code's branch label: the checked-out branch, or for a new worktree
/// `From <ref>`; `Select ref` when git couldn't tell.
fn branch_label(draft: &Draft) -> String {
    match (&draft.workspace, &draft.branch) {
        (_, None) => "Select ref".to_owned(),
        (DraftWorkspace::NewWorktree, Some(base)) => {
            format!("From {}", draft.from.as_ref().unwrap_or(base))
        }
        (DraftWorkspace::Local | DraftWorkspace::Existing(_), Some(branch)) => branch.clone(),
    }
}

/// `✳ 2 working · 14 threads · 4 projects`, numbers in magenta.
fn stats(state: &AppState) -> Line<'static> {
    let sessions = &state.sessions;
    let threads = sessions.threads().count();
    let projects = sessions
        .projects
        .iter()
        .filter(|project| !project.removed)
        .count();
    Line::from(vec![
        span(format!("{CLAUDE_LOGO} "), CLAUDE),
        span(sessions.working_count().to_string(), MAGENTA),
        span(" working · ", BLUE),
        span(threads.to_string(), MAGENTA),
        span(format!(" {} · ", plural(threads, "thread")), BLUE),
        span(projects.to_string(), MAGENTA),
        span(format!(" {}", plural(projects, "project")), BLUE),
    ])
}

fn plural(count: usize, word: &str) -> String {
    match count {
        1 => word.to_owned(),
        _ => format!("{word}s"),
    }
}

fn span<T>(text: T, fg: Color) -> Span<'static>
where
    T: Into<Cow<'static, str>>,
{
    Span::styled(text, Style::new().fg(fg))
}

/// `text` cut to `max` graphemes, with `…` when cut.
fn fit(text: &str, max: usize) -> String {
    if text.graphemes(true).count() <= max {
        return text.to_owned();
    }
    let kept: String = text.graphemes(true).take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// Draws `line` from `(x, y)` to `area`'s right edge; nothing when that cell
/// is outside `area`.
fn put(line: Line<'_>, x: u16, y: u16, area: Rect, buf: &mut Buffer) {
    if area.contains(Position::new(x, y)) {
        line.render(Rect::new(x, y, area.right() - x, 1), buf);
    }
}

/// Draws `line` centred on `area`'s row `y`.
fn centre(line: Line<'_>, y: u16, area: Rect, buf: &mut Buffer) {
    let width = u16::try_from(line.width()).unwrap_or(u16::MAX);
    put(
        line,
        area.x + area.width.saturating_sub(width) / 2,
        y,
        area,
        buf,
    );
}

/// The colour `t` of the way from `from` to `to`.
fn lerp(from: Color, to: Color, t: f64) -> Color {
    let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (from, to) else {
        return from;
    };
    let mix = |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * t).round() as u8;
    Color::Rgb(mix(r1, r2), mix(g1, g2), mix(b1, b2))
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use orb_domain::AppState;
    use orb_domain::feat::picker::list::setting_label;
    use orb_domain::feat::sessions::state::{
        Draft, DraftWorkspace, Group, GroupDraft, GroupId, GroupKind, NEW_THREAD, Project,
        ProjectId, ProjectKind, Sessions, SidebarItem, Thread, ThreadId, ThreadStatus,
    };
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::{Position, Rect};

    use super::{ORANGE, SHADOW, render};

    fn thread(id: i64) -> Thread {
        Thread {
            id: ThreadId(id),
            title: Some("Fix the bug".to_owned()),
            cwd: "/Users/me/dev/orb".into(),
            transcript: None,
            status: ThreadStatus::Idle,
            turn_started_at: None,
            attach_argv: vec![],
            branch: Some("main".to_owned()),
            pinned_at: None,
            settled_at: None,
            active_since: SystemTime::UNIX_EPOCH,
            last_activity_at: SystemTime::UNIX_EPOCH,
            unseen: false,
            group: None,
            model: None,
            permission: None,
        }
    }

    fn project(id: i64, title: &str, threads: Vec<Thread>, draft: Option<Draft>) -> Project {
        Project {
            id: ProjectId(id),
            title: title.to_owned(),
            root: format!("/Users/me/dev/{title}").into(),
            created_at: SystemTime::UNIX_EPOCH,
            removed: false,
            draft,
            threads,
            groups: vec![],
            kind: ProjectKind::Normal,
        }
    }

    /// `projects` with the cursor on `cursor` and `/Users/me` as home.
    fn state(projects: Vec<Project>, cursor: Option<SidebarItem>) -> AppState {
        AppState {
            sessions: Sessions {
                projects,
                cursor,
                ..Sessions::default()
            },
            home: "/Users/me".into(),
            ..AppState::default()
        }
    }

    /// orb's thread 1, selected.
    fn selected_thread() -> AppState {
        state(
            vec![project(1, "orb", vec![thread(1)], None)],
            Some(SidebarItem::Thread(ThreadId(1))),
        )
    }

    /// orb's new-worktree draft based on main, starting from origin/main, in
    /// plan mode, selected.
    fn new_worktree_draft() -> AppState {
        let draft = Draft {
            workspace: DraftWorkspace::NewWorktree,
            branch: Some("main".to_owned()),
            model: None,
            permission: Some("plan".to_owned()),
            created_at: SystemTime::UNIX_EPOCH,
            repo: true,
            from: Some("origin/main".to_owned()),
        };
        state(
            vec![project(1, "orb", vec![], Some(draft))],
            Some(SidebarItem::Draft(ProjectId(1))),
        )
    }

    /// orb with one settled thread, the Settled header selected.
    fn shelf_header() -> AppState {
        let settled = Thread {
            settled_at: Some(SystemTime::UNIX_EPOCH),
            ..thread(1)
        };
        state(
            vec![project(1, "orb", vec![settled], None)],
            Some(SidebarItem::SettledShelf),
        )
    }

    /// Draws `state`'s dashboard on a `width`×`height` buffer with
    /// `pane_error`; returns the buffer and where the cursor goes.
    fn draw(
        state: &AppState,
        pane_error: Option<&str>,
        width: u16,
        height: u16,
    ) -> (Buffer, Position) {
        let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
        let at = render(state, pane_error, buf.area, &mut buf);
        (buf, at)
    }

    fn lines(buf: &Buffer) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .filter_map(|x| buf.cell((x, y)).map(Cell::symbol))
                    .collect()
            })
            .collect()
    }

    /// The first line holding `text`.
    fn line_with(buf: &Buffer, text: &str) -> String {
        lines(buf)
            .into_iter()
            .find(|line| line.contains(text))
            .unwrap_or_default()
    }

    /// Every orange cell, row by row: `(x, y, symbol)`.
    fn orange(buf: &Buffer) -> Vec<(u16, u16, String)> {
        buf.area
            .positions()
            .filter_map(|at| buf.cell(at).map(|cell| (at, cell)))
            .filter(|(_, cell)| cell.fg == ORANGE)
            .map(|(at, cell)| (at.x, at.y, cell.symbol().to_owned()))
            .collect()
    }

    #[rstest::rstest]
    fn wide_area_draws_the_orb_banner() {
        // Given a selected thread.
        let state = selected_thread();

        // When drawing the dashboard 80×40.
        let (buf, _) = draw(&state, None, 80, 40);

        // Then every row of the ORB letters is drawn.
        let screen = lines(&buf).join("\n");
        assert!(
            SHADOW.iter().all(|row| screen.contains(row)),
            "screen was\n{screen}"
        );
    }

    #[rstest::rstest]
    fn menu_keys_are_orange_against_the_menus_right_edge() {
        // Given a selected thread.
        let state = selected_thread();

        // When drawing the dashboard 80×40, its menu 60 wide from column 10.
        let (buf, _) = draw(&state, None, 80, 40);

        // Then the thread's item keys, in order, are orange in column 69.
        let cells = orange(&buf);
        let keys: String = cells.iter().map(|(_, _, key)| key.as_str()).collect();
        let columns: Vec<u16> = cells.iter().map(|(x, _, _)| *x).collect();
        assert_eq!(
            (keys.as_str(), columns),
            ("owbnpftgvq", vec![69; 10]),
            "the menu's keys"
        );
    }

    #[rstest::rstest]
    fn wide_area_draws_the_footer_counts() {
        // Given orb with two threads, the first selected.
        let state = state(
            vec![project(1, "orb", vec![thread(1), thread(2)], None)],
            Some(SidebarItem::Thread(ThreadId(1))),
        );

        // When drawing the dashboard 80×40.
        let (buf, _) = draw(&state, None, 80, 40);

        // Then the footer counts working threads, threads and projects.
        let footer = line_with(&buf, "working");
        assert_eq!(
            footer.trim(),
            "✳ 0 working · 2 threads · 1 project",
            "the footer"
        );
    }

    #[rstest::rstest]
    fn one_thread_and_one_project_are_counted_in_the_singular() {
        // Given orb with one thread.
        let state = selected_thread();

        // When drawing the dashboard 80×40.
        let (buf, _) = draw(&state, None, 80, 40);

        // Then the footer says 1 thread and 1 project.
        let footer = line_with(&buf, "working");
        assert!(
            footer.contains(" 1 thread · 1 project"),
            "footer was '{footer}'"
        );
    }

    #[rstest::rstest]
    fn short_area_leaves_no_blank_line_between_items() {
        // Given a selected thread, whose ten items with blank lines between
        // them don't fit 20 rows.
        let state = selected_thread();

        // When drawing the dashboard 80×20.
        let (buf, _) = draw(&state, None, 80, 20);

        // Then the items' keys are on ten consecutive rows.
        let rows: Vec<u16> = orange(&buf).iter().map(|(_, y, _)| *y).collect();
        let first = rows.first().copied().unwrap_or_default();
        assert_eq!(
            rows,
            (first..first + 10).collect::<Vec<_>>(),
            "the items' rows"
        );
    }

    /// orb's thread 1 in Feature group 9 `GT-514-login`, which has a draft;
    /// the cursor on `cursor`.
    fn in_group(cursor: SidebarItem) -> AppState {
        let group = Group {
            id: GroupId(9),
            kind: GroupKind::Feature,
            name: "GT-514-login".to_owned(),
            dir: None,
            branch: Some("GT-514-login".to_owned()),
            created_at: SystemTime::UNIX_EPOCH,
            pinned_at: None,
            settled_at: None,
            active_since: SystemTime::UNIX_EPOCH,
            draft: Some(GroupDraft {
                model: None,
                permission: None,
            }),
        };
        let child = Thread {
            group: Some(GroupId(9)),
            ..thread(1)
        };
        state(
            vec![Project {
                groups: vec![group],
                ..project(1, "orb", vec![child], None)
            }],
            Some(cursor),
        )
    }

    #[rstest::rstest]
    fn group_card_context_shows_the_group_path_and_kind() {
        // Given Feature group GT-514-login's card selected.
        let state = in_group(SidebarItem::Group(GroupId(9)));

        // When drawing the dashboard 80×40.
        let (buf, _) = draw(&state, None, 80, 40);

        // Then the context line names orb/GT-514-login and Feature group.
        let context = line_with(&buf, "orb/GT-514-login");
        assert!(
            context.contains("orb/GT-514-login  ·  Feature group"),
            "context line was '{context}'"
        );
    }

    #[rstest::rstest]
    fn group_draft_context_shows_new_thread_in_the_group() {
        // Given Feature group GT-514-login's draft selected.
        let state = in_group(SidebarItem::GroupDraft(GroupId(9)));

        // When drawing the dashboard 80×40.
        let (buf, _) = draw(&state, None, 80, 40);

        // Then the context line names orb/GT-514-login and a new thread.
        let context = line_with(&buf, "orb/GT-514-login");
        assert!(
            context.contains(&format!("orb/GT-514-login  ·  {NEW_THREAD}")),
            "context line was '{context}'"
        );
    }

    #[rstest::rstest]
    fn group_draft_menu_shows_its_model() {
        // Given Feature group GT-514-login's draft on opus, selected.
        let mut state = in_group(SidebarItem::GroupDraft(GroupId(9)));
        if let Some(draft) = state.sessions.group_draft_mut(GroupId(9)) {
            draft.model = Some("opus".to_owned());
        }

        // When drawing the dashboard 80×40.
        let (buf, _) = draw(&state, None, 80, 40);

        // Then the Model row shows opus's name.
        let row = line_with(&buf, "Model");
        let expected = format!("Model  {}", setting_label(Some("opus")));
        assert!(row.contains(&expected), "Model row was '{row}'");
    }

    #[rstest::rstest]
    fn started_feature_card_menu_shows_its_branch() {
        // Given Feature group GT-514-login's card selected, its worktree on
        // `main`.
        let mut state = in_group(SidebarItem::Group(GroupId(9)));
        if let Some(group) = state
            .sessions
            .projects
            .first_mut()
            .and_then(|project| project.groups.first_mut())
        {
            group.dir = Some("/wt/orb-1a2b3c4d".into());
            group.branch = Some("main".to_owned());
            group.draft = None;
        }

        // When drawing the dashboard 80×40.
        let (buf, _) = draw(&state, None, 80, 40);

        // Then the Branch row shows the group's branch.
        let row = line_with(&buf, "Branch");
        assert!(row.contains("Branch  main"), "Branch row was '{row}'");
    }

    #[rstest::rstest]
    fn shelf_header_context_line_shows_the_shelf_hint() {
        // Given a collapsed shelf with one settled thread, its header selected.
        let state = shelf_header();

        // When drawing the dashboard 80×40.
        let (buf, _) = draw(&state, None, 80, 40);

        // Then the context line says ⏎ opens the shelf.
        let screen = lines(&buf).join("\n");
        assert!(
            screen.contains("▸ Settled (1) · ⏎ open"),
            "screen was\n{screen}"
        );
    }

    #[rstest::rstest]
    fn shelf_hint_counts_only_the_filtered_projects_threads() {
        // Given orb's settled thread 1 and web's settled thread 2, filtered to
        // orb, with the shelf header selected.
        let settled = |id| Thread {
            settled_at: Some(SystemTime::UNIX_EPOCH),
            ..thread(id)
        };
        let state = {
            let mut state = state(
                vec![
                    project(1, "orb", vec![settled(1)], None),
                    project(2, "web", vec![settled(2)], None),
                ],
                Some(SidebarItem::SettledShelf),
            );
            state.sessions.filter = Some(ProjectId(1));
            state
        };

        // When drawing the dashboard 80×40.
        let (buf, _) = draw(&state, None, 80, 40);

        // Then the hint counts orb's one settled thread.
        let screen = lines(&buf).join("\n");
        assert!(
            screen.contains("▸ Settled (1) · ⏎ open"),
            "screen was\n{screen}"
        );
    }

    #[rstest::rstest]
    fn pane_error_shows_one_line_under_the_footer() {
        // Given a selected thread whose session couldn't start.
        let state = selected_thread();

        // When drawing the dashboard 80×40 with the pane's error.
        let (buf, _) = draw(&state, Some("claude attach failed"), 80, 40);

        // Then the error is on the line under the footer.
        let lines = lines(&buf);
        let under = lines
            .iter()
            .position(|line| line.contains("working"))
            .and_then(|footer| lines.get(footer + 1));
        assert_eq!(
            under.map(|line| line.trim()),
            Some("claude attach failed"),
            "the line under the footer"
        );
    }

    #[rstest::rstest]
    fn new_worktree_draft_branch_reads_from_its_ref() {
        // Given a new-worktree draft starting from origin/main.
        let state = new_worktree_draft();

        // When drawing the dashboard 80×40.
        let (buf, _) = draw(&state, None, 80, 40);

        // Then the Branch item's value names the ref.
        let branch = line_with(&buf, "Branch");
        assert!(
            branch.contains("Branch  From origin/main"),
            "branch row was '{branch}'"
        );
    }

    #[rstest::rstest]
    #[case("Workspace", "Workspace  New worktree")]
    #[case("Model", "Model  Default")]
    #[case("Permission", "Permission  plan")]
    fn draft_item_shows_its_value(#[case] label: &str, #[case] expected: &str) {
        // Given a new-worktree draft on the default model in plan mode.
        let state = new_worktree_draft();

        // When drawing the dashboard 80×40.
        let (buf, _) = draw(&state, None, 80, 40);

        // Then the item's row shows its value after the label.
        let row = line_with(&buf, label);
        assert!(row.contains(expected), "{label} row was '{row}'");
    }

    #[rstest::rstest]
    fn cursor_goes_on_the_highlighted_labels_first_cell() {
        // Given a selected thread with the menu cursor moved to Workspace.
        let mut state = selected_thread();
        state.dashboard.next(&state.sessions);

        // When drawing the dashboard 80×40.
        let (buf, at) = draw(&state, None, 80, 40);

        // Then the cursor's cell starts the Workspace label.
        let from_cursor: String = (at.x..buf.area.width)
            .filter_map(|x| buf.cell((x, at.y)).map(Cell::symbol))
            .collect();
        assert!(
            from_cursor.starts_with("Workspace"),
            "the row from the cursor was '{from_cursor}'"
        );
    }

    #[rstest::rstest]
    fn small_area_draws_nothing_outside_itself() {
        // Given a selected thread and a 4×3 area inside a 40×20 buffer.
        let state = selected_thread();
        let area = Rect::new(10, 10, 4, 3);
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 20));

        // When drawing the dashboard into the area.
        render(&state, Some("claude attach failed"), area, &mut buf);

        // Then every cell outside the area is still blank.
        let drawn: Vec<Position> = buf
            .area
            .positions()
            .filter(|at| !area.contains(*at))
            .filter(|at| buf.cell(*at).is_some_and(|cell| cell.symbol() != " "))
            .collect();
        assert_eq!(drawn, [], "cells drawn outside the area");
    }
}
