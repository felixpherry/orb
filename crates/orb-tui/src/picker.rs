//! The picker popup, drawn like LazyVim's `vim.ui.select` in tokyonight-moon:
//! a small rounded float with the picker's name centred in its top border, a
//! `>` prompt over an orange rule, numbered one-line rows with the selected
//! one filled, and the picker's keys dim in its bottom border. The float is
//! only as tall as its rows need, and its top stays put while the filter
//! narrows them.
//!
//! The project picker shows each project as a folder in its badge colour and
//! its path, the parent dimmed and the name bright, with the name on the
//! right when the folder is named differently; so does the project filter,
//! under an `All projects` row. Confirming a project's removal offers `No`
//! and `Yes`; a session start waiting on trust asks `Trust ~/path?` the same
//! way. The directory picker shows one folder per row; the workspace
//! picker shows where a thread's session could run, each with its glyph; the
//! branch picker shows each branch with its badge on the right, dimming the
//! ones checked out where the thread can't follow and saying where; the model
//! picker shows each model's name after Claude's mark, with its legacy models
//! under their own heading, and the permission picker each mode after a
//! shield; a draft whose project isn't a git repository gets one row,
//! `Initialize Git`. Where the filter matched is blue and bold, and the rows
//! scroll to keep the selection in view.

use std::borrow::Cow;
use std::path::Path;

use orb_domain::feat::git::git_service::GitRef;
use orb_domain::feat::picker::list::{
    ALL_PROJECTS, INIT_GIT, Matches, PickerItem, WorkspaceChoice, confirm_label, setting_label,
};
use orb_domain::feat::picker::state::{PickerKind, PickerState, split_path};
use orb_domain::feat::sessions::state::{GroupKind, ProjectKind};
use orb_domain::tilde;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Widget};
use unicode_segmentation::UnicodeSegmentation;

use crate::sidebar::{
    BG_DARK, BLUE, BLUE1, BORDER, BRANCH, CLAUDE, CLAUDE_LOGO, COMMENT, CYAN, DARK3, DARK5, FG,
    FG_DARK, FOLDER, FOLDER_OPEN, GREEN, MAGENTA, ORANGE, VISUAL, YELLOW, badge, render_split,
};

/// Nerd Font's code-fork glyph, before the worktree workspace rows.
const WORKTREE: &str = "\u{f126}";
/// Nerd Font's history glyph, before the previous worktree's workspace row.
const HISTORY: &str = "\u{f1da}";
/// Before a permission mode (`nf-fa-shield`).
const SHIELD: &str = "\u{f132}";
/// Before Initialize Git (`nf-fa-git`).
const GIT: &str = "\u{f1d3}";

/// How far the picker's rows are scrolled, kept between frames.
#[derive(Debug, Default)]
pub(crate) struct PickerScroll {
    /// The row drawn at the top.
    offset: usize,
}

/// Draws `picker` in a popup over `area`; paths under `home` show as `~/`.
/// Returns how many rows fit, and where the terminal cursor goes in the input.
pub(crate) fn render(
    picker: &PickerState,
    home: &Path,
    area: Rect,
    buf: &mut Buffer,
    scroll: &mut PickerScroll,
) -> (usize, Position) {
    let popup = {
        let lines = |count: usize| u16::try_from(count).unwrap_or(u16::MAX);
        popup_rect(area, lines(picker.shown().count()), lines(picker.total()))
    };
    Clear.render(popup, buf);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(BORDER).bg(BG_DARK))
        .style(Style::new().bg(BG_DARK))
        .title(Line::from(span(format!(" {} ", title(picker.kind(), home)), BLUE)).centered())
        .title_bottom(hints(picker.kind()));
    let inner = block.inner(popup);
    block.render(popup, buf);
    let [input, rule, rows] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);
    let cursor = render_input(picker, input, buf);
    Line::from(span("─".repeat(usize::from(rule.width)), ORANGE)).render(rule, buf);
    let page = render_rows(picker, home, rows, buf, scroll);
    (page, cursor)
}

/// The popup: half the width, kept to 44–72 columns, and as tall as `shown`
/// rows plus its border, input and rule, at most 60% of the height. It's
/// centred across, and its top stays where the popup for all `total` rows
/// would be centred, so filtering doesn't move the input.
fn popup_rect(area: Rect, shown: u16, total: u16) -> Rect {
    let width = (area.width / 2).clamp(44, 72).min(area.width);
    let tallest = (area.height.saturating_mul(3) / 5).max(8).min(area.height);
    let full = total.max(1).saturating_add(4).min(tallest);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - full) / 2,
        width,
        shown.max(1).saturating_add(4).min(tallest),
    )
}

/// The picker's name, centred in its top border; paths under `home` show as `~/`.
fn title(kind: &PickerKind, home: &Path) -> Cow<'static, str> {
    let name = match kind {
        PickerKind::TrustWorkspace { dir } => {
            return Cow::Owned(format!("Trust {}?", tilde(dir, home)));
        }
        PickerKind::Projects | PickerKind::GroupProject => "Projects",
        PickerKind::Sessions { .. } => "Sessions",
        PickerKind::ProjectFilter => "Filter projects",
        PickerKind::Directories { .. } => "Add project",
        PickerKind::Workspace { .. } => "Workspace",
        PickerKind::Branches { .. } => "Branches",
        PickerKind::Model { .. } => "Model",
        PickerKind::Permission { .. } => "Permission mode",
        PickerKind::InitGit { .. } => "Not a git repository",
        PickerKind::RemoveProject { .. } => "Remove project?",
        PickerKind::SettleThread { .. } => "Settle thread?",
        PickerKind::DeleteThread { .. } => "Delete thread?",
        PickerKind::DiscardDraft { .. } => "Discard draft?",
        PickerKind::SettleGroup { .. } => "Settle group?",
        PickerKind::DeleteGroup { dir: None, .. } => "Delete group?",
        PickerKind::DeleteGroup {
            dir: Some(GroupKind::Feature),
            ..
        } => "Delete group and its worktree?",
        PickerKind::DeleteGroup { dir: Some(_), .. } => "Delete group and its folder?",
    };
    Cow::Borrowed(name)
}

/// The picker's keys, dim and right-aligned in its bottom border: each key,
/// then what it does.
fn hints(kind: &PickerKind) -> Line<'static> {
    let keys: &[(&str, &str)] = match kind {
        PickerKind::Directories { .. } => &[("⏎", "add"), ("Tab", "open"), ("Esc", "close")],
        PickerKind::ProjectFilter => &[("⏎", "filter"), ("<C-x>", "remove"), ("Esc", "close")],
        PickerKind::RemoveProject { .. }
        | PickerKind::SettleThread { .. }
        | PickerKind::DeleteThread { .. }
        | PickerKind::DiscardDraft { .. }
        | PickerKind::SettleGroup { .. }
        | PickerKind::DeleteGroup { .. }
        | PickerKind::TrustWorkspace { .. }
        | PickerKind::InitGit { .. } => &[("⏎", "confirm"), ("Esc", "cancel")],
        _ => &[("⏎", "select"), ("Esc", "close")],
    };
    let mut spans = vec![Span::raw(" ")];
    for (index, (key, label)) in keys.iter().enumerate() {
        if index > 0 {
            spans.push(span(" · ", DARK3));
        }
        spans.push(span(*key, FG_DARK));
        spans.push(span(format!(" {label}"), COMMENT));
    }
    spans.push(Span::raw(" "));
    Line::from(spans).right_aligned()
}

/// The ` > ` prompt and the typed text, its end while it is too long to fit.
/// Returns the cursor's position.
fn render_input(picker: &PickerState, area: Rect, buf: &mut Buffer) -> Position {
    let prompt = span(" > ", CYAN);
    let prompt_width = prompt.width();
    let (shown, before) = visible(
        picker.input(),
        picker.cursor(),
        usize::from(area.width).saturating_sub(prompt_width + 1),
    );
    Line::from(vec![prompt, span(shown, FG)]).render(area, buf);
    let x = u16::try_from(prompt_width + before)
        .unwrap_or(u16::MAX)
        .min(area.width.saturating_sub(1));
    Position::new(area.x + x, area.y)
}

/// The numbered rows from the scroll offset down, the selected one filled, or
/// a hint when there are none. Returns how many rows fit.
fn render_rows(
    picker: &PickerState,
    home: &Path,
    area: Rect,
    buf: &mut Buffer,
    scroll: &mut PickerScroll,
) -> usize {
    let page = usize::from(area.height).max(1);
    let shown: Vec<(&PickerItem, &Matches)> = picker.shown().collect();
    if shown.is_empty() {
        Line::from(span(format!("  {}", empty_hint(picker)), COMMENT)).render(area, buf);
        return page;
    }
    let selection = picker.selection();
    scroll.offset = scroll
        .offset
        .min(selection)
        .max((selection + 1).saturating_sub(page))
        .min(shown.len().saturating_sub(page));
    let mut number = shown
        .iter()
        .take(scroll.offset)
        .filter(|(item, _)| !matches!(item, PickerItem::Heading(_)))
        .count();
    for ((index, (item, matches)), y) in shown
        .into_iter()
        .enumerate()
        .skip(scroll.offset)
        .zip(area.top()..area.bottom())
    {
        let row = Rect::new(area.x, y, area.width, 1);
        if index == selection && picker.selected().is_some() {
            buf.set_style(row, Style::new().bg(VISUAL));
        }
        let label = match item {
            PickerItem::Heading(_) => Span::raw("    "),
            _ => {
                number += 1;
                span(format!("{number:>2}. "), DARK5)
            }
        };
        let (content, right) = row_content(item, matches, picker.kind(), home);
        let left = Line::from_iter([Span::raw(" "), label].into_iter().chain(content));
        // The row's own text wins; the right column is cut from its left.
        let room = usize::from(row.width).saturating_sub(left.width() + 3);
        let right = right.map_or_else(Line::default, |right| right.cut(room));
        render_split(
            left,
            right,
            Rect {
                width: row.width.saturating_sub(1),
                ..row
            },
            buf,
        );
    }
    page
}

/// What a row shows against its right edge: `prefix`, then `text`, cut from
/// its left to fit.
struct RightColumn {
    prefix: &'static str,
    text: String,
    fg: Color,
}

impl RightColumn {
    /// `text` in `fg`, with no prefix.
    fn new(text: String, fg: Color) -> Self {
        Self {
            prefix: "",
            text,
            fg,
        }
    }

    /// The column in at most `room` cells, keeping the prefix whole.
    fn cut(self, room: usize) -> Line<'static> {
        let text = cut_left(&self.text, room.saturating_sub(self.prefix.len()));
        Line::from(span(format!("{}{text}", self.prefix), self.fg))
    }
}

/// A row's icon and text, and what goes against its right edge.
fn row_content(
    item: &PickerItem,
    matches: &Matches,
    kind: &PickerKind,
    home: &Path,
) -> (Vec<Span<'static>>, Option<RightColumn>) {
    match item {
        PickerItem::Project {
            title,
            kind: ProjectKind::Research | ProjectKind::Learn | ProjectKind::Incognito,
            ..
        } => {
            let folder = icon(FOLDER, badge(title, true).style.fg.unwrap_or(BLUE));
            (labelled(folder, title, &matches.name), None)
        }
        PickerItem::Project { title, root, .. } => {
            let shown = tilde(root, home);
            let named = shown.ends_with(title.as_str());
            let offsets: Vec<usize> = {
                let shift = root.display().to_string().len().saturating_sub(shown.len());
                let base = shown.len().saturating_sub(title.len());
                matches
                    .path
                    .iter()
                    .filter_map(|offset| offset.checked_sub(shift))
                    .chain(
                        matches
                            .name
                            .iter()
                            .filter(|_| named)
                            .map(|offset| base + offset),
                    )
                    .collect()
            };
            let split = shown.rfind('/').map_or(0, |at| at + 1);
            let folder = icon(FOLDER, badge(title, true).style.fg.unwrap_or(BLUE));
            let left = std::iter::once(folder)
                .chain(highlight(&shown, &offsets, |at| {
                    if at < split { DARK5 } else { FG }
                }))
                .collect();
            let right = (!named).then(|| RightColumn::new(title.clone(), DARK5));
            (left, right)
        }
        PickerItem::Directory { name } => {
            let mut left = labelled(icon(FOLDER, BLUE), name, &matches.name);
            left.push(span("/", DARK5));
            (left, None)
        }
        PickerItem::Workspace(choice) => {
            let (glyph, fg) = match choice {
                WorkspaceChoice::Current { worktree: false } => (FOLDER, BLUE),
                WorkspaceChoice::Current { worktree: true } => (WORKTREE, BLUE),
                WorkspaceChoice::NewWorktree => (WORKTREE, GREEN),
                WorkspaceChoice::Previous { .. } => (HISTORY, MAGENTA),
            };
            (
                labelled(icon(glyph, fg), &choice.label(), &matches.name),
                None,
            )
        }
        PickerItem::Branch(row) => {
            let cwd = match kind {
                PickerKind::Branches { cwd, .. } => cwd.as_path(),
                _ => Path::new(""),
            };
            let git_ref = &row.git_ref;
            let (icon_fg, name_fg) = match (row.disabled, git_ref) {
                (true, _) => (DARK3, DARK3),
                (_, GitRef { current: true, .. }) => (GREEN, FG),
                (_, GitRef { remote: true, .. }) => (MAGENTA, FG),
                _ => (BLUE, FG),
            };
            let left = std::iter::once(icon(BRANCH, icon_fg))
                .chain(highlight(&git_ref.name, &matches.name, |_| name_fg))
                .collect();
            let right = match (row.disabled, &git_ref.worktree) {
                (true, Some(path)) => Some(RightColumn {
                    prefix: "in ",
                    text: tilde(path, home),
                    fg: DARK3,
                }),
                _ => branch_badge(git_ref, cwd)
                    .map(|(badge, fg)| RightColumn::new(badge.to_owned(), fg)),
            };
            (left, right)
        }
        PickerItem::Setting(value) => {
            let mark = match kind {
                PickerKind::Permission { .. } => icon(SHIELD, YELLOW),
                _ => icon(CLAUDE_LOGO, CLAUDE),
            };
            (labelled(mark, setting_label(*value), &matches.name), None)
        }
        PickerItem::Heading(text) => (vec![span(format!("── {text} ──"), COMMENT)], None),
        PickerItem::InitGit => (labelled(icon(GIT, ORANGE), INIT_GIT, &matches.name), None),
        PickerItem::AllProjects => (
            labelled(icon(FOLDER_OPEN, BLUE), ALL_PROJECTS, &matches.name),
            None,
        ),
        PickerItem::Confirm(yes) => (highlight(confirm_label(*yes), &matches.name, |_| FG), None),
        PickerItem::Thread { label, .. } => (highlight(label, &matches.name, |_| FG), None),
    }
}

/// `glyph` and a space, in `fg`: the icon before a row's text.
fn icon(glyph: &str, fg: Color) -> Span<'static> {
    span(format!("{glyph} "), fg)
}

/// `icon`, then `text` highlighted where the filter matched.
fn labelled(icon: Span<'static>, text: &str, offsets: &[usize]) -> Vec<Span<'static>> {
    std::iter::once(icon)
        .chain(highlight(text, offsets, |_| FG))
        .collect()
}

/// The badge on a branch row and its colour, by T3's priority: the branch
/// this directory is on, one checked out in another worktree, a remote ref,
/// the default branch.
fn branch_badge(git_ref: &GitRef, cwd: &Path) -> Option<(&'static str, Color)> {
    let elsewhere = git_ref.worktree.as_deref().is_some_and(|path| path != cwd);
    match git_ref {
        GitRef { current: true, .. } => Some(("current", GREEN)),
        _ if elsewhere => Some(("worktree", YELLOW)),
        GitRef { remote: true, .. } => Some(("remote", MAGENTA)),
        GitRef { default: true, .. } => Some(("default", CYAN)),
        _ => None,
    }
}

/// `text` if it fits in `width` columns, else `…` and as much of its end as
/// fits.
pub(crate) fn cut_left(text: &str, width: usize) -> String {
    if Line::raw(text).width() <= width {
        return text.to_owned();
    }
    let mut used = 1;
    let tail: Vec<&str> = text
        .graphemes(true)
        .rev()
        .take_while(|grapheme| {
            used += Line::raw(*grapheme).width();
            used <= width
        })
        .collect();
    std::iter::once("…").chain(tail.into_iter().rev()).collect()
}

/// The part of `text` to show when `room` columns fit before the cursor
/// (at grapheme `cursor`): everything, or a tail that keeps the cursor in
/// view. Returns it and its width before the cursor.
pub(crate) fn visible(text: &str, cursor: usize, room: usize) -> (String, usize) {
    let graphemes: Vec<&str> = text.graphemes(true).collect();
    let widths: Vec<usize> = graphemes
        .iter()
        .map(|grapheme| Span::raw(*grapheme).width())
        .collect();
    let mut before: usize = widths.iter().take(cursor).sum();
    let mut start = 0;
    while before > room && start < cursor {
        before -= widths.get(start).copied().unwrap_or_default();
        start += 1;
    }
    (graphemes.iter().skip(start).copied().collect(), before)
}

/// `text` with each grapheme holding one of `offsets` (byte offsets into
/// `text`) blue and bold, and the rest in `fg` of the grapheme's offset.
pub(crate) fn highlight<F>(text: &str, offsets: &[usize], fg: F) -> Vec<Span<'static>>
where
    F: Fn(usize) -> Color,
{
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (start, grapheme) in text.grapheme_indices(true) {
        let bytes = start..start + grapheme.len();
        let style = if offsets.iter().any(|offset| bytes.contains(offset)) {
            Style::new().fg(BLUE1).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(fg(start))
        };
        match spans.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push_str(grapheme),
            _ => spans.push(Span::styled(grapheme.to_owned(), style)),
        }
    }
    spans
}

/// `text` in `fg`.
fn span<'a, T>(text: T, fg: Color) -> Span<'a>
where
    T: Into<Cow<'a, str>>,
{
    Span::styled(text, Style::new().fg(fg))
}

/// Why no rows are shown.
fn empty_hint(picker: &PickerState) -> &'static str {
    match picker.kind() {
        PickerKind::Branches { .. } if picker.input().is_empty() => "Loading branches…",
        PickerKind::Directories { .. } if split_path(picker.input()).is_none() => {
            "Type a path starting with / or ~/"
        }
        _ => "No results",
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use orb_domain::Focus;
    use orb_domain::feat::git::git_service::GitRef;
    use orb_domain::feat::picker::list::{PickerItem, WorkspaceChoice};
    use orb_domain::feat::picker::state::{DraftTarget, PickTarget, PickerState};
    use orb_domain::feat::sessions::state::{GroupId, GroupKind, ProjectId, ProjectKind, ThreadId};
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;
    use unicode_segmentation::UnicodeSegmentation;

    use super::{FOLDER, GIT, HISTORY, PickerScroll, SHIELD, render};
    use crate::sidebar::{BLUE1, CLAUDE_LOGO, DARK3, DARK5, FG, ORANGE, VISUAL, badge};

    /// The home directory the pickers are drawn with.
    const HOME: &str = "/Users/me";

    fn project(id: i64, title: &str, root: &str) -> PickerItem {
        PickerItem::Project {
            id: ProjectId(id),
            title: title.to_owned(),
            root: root.into(),
            kind: ProjectKind::Normal,
        }
    }

    /// Draws `picker` over a `width`×`height` screen.
    fn draw(picker: &PickerState, width: u16, height: u16) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
        render(
            picker,
            Path::new(HOME),
            buf.area,
            &mut buf,
            &mut PickerScroll::default(),
        );
        buf
    }

    /// The screen's lines, top to bottom.
    fn lines(buf: &Buffer) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .filter_map(|x| buf.cell((x, y)).map(Cell::symbol))
                    .collect()
            })
            .collect()
    }

    /// The cell where `text` first starts, row by row.
    fn find(buf: &Buffer, text: &str) -> Option<(u16, u16)> {
        let len = text.graphemes(true).count();
        (0..buf.area.height).find_map(|y| {
            let row: Vec<&str> = (0..buf.area.width)
                .filter_map(|x| buf.cell((x, y)).map(Cell::symbol))
                .collect();
            (0..row.len())
                .find(|&x| {
                    row.get(x..x + len)
                        .is_some_and(|cells| cells.concat() == text)
                })
                .map(|x| (x as u16, y))
        })
    }

    /// The rows of the popup's top and bottom borders.
    fn border_rows(buf: &Buffer) -> Option<(u16, u16)> {
        find(buf, "╭")
            .zip(find(buf, "╰"))
            .map(|((_, top), (_, bottom))| (top, bottom))
    }

    /// The columns of the popup's left and right borders.
    fn border_columns(buf: &Buffer) -> Option<(u16, u16)> {
        find(buf, "╭")
            .zip(find(buf, "╮"))
            .map(|((left, _), (right, _))| (left, right))
    }

    /// The text inside the borders on the popup's `n`th line, trimmed.
    fn inner_line(buf: &Buffer, n: u16) -> Option<String> {
        let (top, _) = border_rows(buf)?;
        lines(buf)
            .get(usize::from(top + n))
            .map(|line| line.trim_matches(['│', ' ']).to_owned())
    }

    /// The line holding `text`.
    fn line_with(buf: &Buffer, text: &str) -> Option<String> {
        lines(buf).into_iter().find(|line| line.contains(text))
    }

    fn orb() -> PickerState {
        PickerState::projects(
            vec![
                project(1, "orb", "/Users/me/dev/orb"),
                project(2, "jinn", "/Users/me/dev/jinn"),
            ],
            Focus::Sidebar,
        )
    }

    /// Ten projects, none named like its folder.
    fn ten_projects() -> Vec<PickerItem> {
        (0..10)
            .map(|i| project(i, &format!("proj{i}"), &format!("/tmp/{i}")))
            .collect()
    }

    #[rstest::rstest]
    fn popup_is_as_tall_as_its_rows_plus_four() {
        // Given a project picker with two projects on a tall screen.
        let picker = orb();

        // When drawing it.
        let buf = draw(&picker, 100, 40);

        // Then the popup is 6 lines: its two rows and 4 of chrome.
        let height = border_rows(&buf).map(|(top, bottom)| bottom - top + 1);
        assert_eq!(height, Some(6), "the popup's height");
    }

    #[rstest::rstest]
    fn popup_is_at_most_60_percent_of_the_screen() {
        // Given a project picker with fifty projects.
        let picker = {
            let items = (0..50)
                .map(|i| project(i, &format!("proj{i}"), &format!("/tmp/{i}")))
                .collect();
            PickerState::projects(items, Focus::Sidebar)
        };

        // When drawing it on a 40-line screen.
        let buf = draw(&picker, 100, 40);

        // Then the popup is 24 lines tall.
        let height = border_rows(&buf).map(|(top, bottom)| bottom - top + 1);
        assert_eq!(height, Some(24), "the popup's height");
    }

    #[rstest::rstest]
    #[case(60, 44)]
    #[case(200, 72)]
    fn popup_width(#[case] screen: u16, #[case] expected: u16) {
        // Given a project picker.
        let picker = orb();

        // When drawing it on a `screen`-column screen.
        let buf = draw(&picker, screen, 40);

        // Then the popup's top border spans `expected` columns.
        let width = border_columns(&buf).map(|(left, right)| right - left + 1);
        assert_eq!(
            width,
            Some(expected),
            "the popup's width on {screen} columns"
        );
    }

    #[rstest::rstest]
    fn popup_top_stays_when_the_filter_hides_rows() {
        // Given ten projects, unfiltered and filtered down to none.
        let unfiltered = PickerState::projects(ten_projects(), Focus::Sidebar);
        let filtered = {
            let mut picker = PickerState::projects(ten_projects(), Focus::Sidebar);
            picker.insert('z');
            picker
        };

        // When drawing each.
        let tops = [unfiltered, filtered]
            .map(|picker| border_rows(&draw(&picker, 100, 40)).map(|(top, _)| top));

        // Then both popups start on the same row.
        assert_eq!(tops[0], tops[1], "the popup's top row");
    }

    #[rstest::rstest]
    fn title_is_centred_in_the_top_border() {
        // Given a project picker.
        let picker = orb();

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then " Projects " sits on the top border with as much border on
        // each side, give or take a cell.
        let title = " Projects ";
        let gaps = find(&buf, title)
            .zip(border_columns(&buf))
            .zip(border_rows(&buf))
            .filter(|(((_, y), _), (top, _))| y == top)
            .map(|(((x, _), (left, right)), _)| (x - left, right - (x + title.len() as u16 - 1)));
        assert!(
            gaps.is_some_and(|(left, right)| left.abs_diff(right) <= 1),
            "the gaps beside the title were {gaps:?}"
        );
    }

    #[rstest::rstest]
    #[case(
        PickerState::directories(PathBuf::from(HOME), Focus::Sidebar).0,
        "Add project"
    )]
    #[case(PickerState::project_filter(vec![PickerItem::AllProjects], None, Focus::Sidebar), "Filter projects")]
    #[case(PickerState::group_project(vec![], Focus::Sidebar), "Projects")]
    #[case(workspace(), "Workspace")]
    #[case(branches(vec![branch("main", None)]), "Branches")]
    #[case(
        PickerState::models(DraftTarget::Project(ProjectId(1)), None, Focus::Dashboard),
        "Model"
    )]
    #[case(
        PickerState::permissions(DraftTarget::Project(ProjectId(1)), None, Focus::Dashboard),
        "Permission mode"
    )]
    #[case(
        PickerState::init_git(ProjectId(1), Focus::Dashboard),
        "Not a git repository"
    )]
    #[case(
        PickerState::remove_project(ProjectId(1), Focus::Sidebar),
        "Remove project?"
    )]
    #[case(
        PickerState::settle_thread(ThreadId(1), Focus::Sidebar),
        "Settle thread?"
    )]
    #[case(
        PickerState::delete_thread(ThreadId(1), Focus::Sidebar),
        "Delete thread?"
    )]
    #[case(
        PickerState::discard_draft(ProjectId(1), Focus::Sidebar),
        "Discard draft?"
    )]
    #[case(PickerState::settle_group(GroupId(9), Focus::Sidebar), "Settle group?")]
    #[case(
        PickerState::delete_group(GroupId(9), None, Focus::Sidebar),
        "Delete group?"
    )]
    #[case(
        PickerState::delete_group(GroupId(9), Some(GroupKind::Feature), Focus::Sidebar),
        "Delete group and its worktree?"
    )]
    #[case(
        PickerState::delete_group(GroupId(9), Some(GroupKind::Research), Focus::Sidebar),
        "Delete group and its folder?"
    )]
    #[case::trust(
        PickerState::trust_workspace(PathBuf::from("/Users/me/dev/orb"), Focus::Sidebar),
        "Trust ~/dev/orb?"
    )]
    fn picker_is_titled_by_its_kind(#[case] picker: PickerState, #[case] title: &str) {
        // Given a picker of some kind.

        // When drawing it.
        let buf = draw(&picker, 60, 40);

        // Then its top border holds its title.
        let top = border_rows(&buf).map(|(top, _)| top);
        let at = find(&buf, &format!(" {title} ")).map(|(_, y)| y);
        assert_eq!(at, top, "the row of the title {title:?}");
    }

    #[rstest::rstest]
    fn input_starts_with_a_prompt() {
        // Given a project picker.
        let picker = orb();

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then the line under the top border starts with ` > `.
        let input = border_rows(&buf)
            .zip(border_columns(&buf))
            .map(|((top, _), (left, _))| {
                (1..4)
                    .filter_map(|dx| buf.cell((left + dx, top + 1)).map(Cell::symbol))
                    .collect::<String>()
            });
        assert_eq!(input.as_deref(), Some(" > "), "the input's first cells");
    }

    #[rstest::rstest]
    fn rule_sits_under_the_input() {
        // Given a project picker.
        let picker = orb();

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then the line under the input is an orange `─`.
        let cell = border_rows(&buf)
            .zip(border_columns(&buf))
            .and_then(|((top, _), (left, _))| buf.cell((left + 1, top + 2)));
        assert_eq!(
            cell.map(|cell| (cell.symbol(), cell.fg)),
            Some(("─", ORANGE)),
            "the rule's first cell"
        );
    }

    #[rstest::rstest]
    fn rows_are_numbered() {
        // Given a project picker with two projects.
        let picker = orb();

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then its first two rows start with 1. and 2.
        let rows = [3, 4].map(|n| inner_line(&buf, n).unwrap_or_default());
        assert!(
            rows[0].starts_with("1. ") && rows[1].starts_with("2. "),
            "the first two rows were {rows:?}"
        );
    }

    #[rstest::rstest]
    fn heading_is_not_numbered() {
        // Given a model picker, whose five current models come before the
        // Legacy models heading.
        let picker =
            PickerState::models(DraftTarget::Project(ProjectId(1)), None, Focus::Dashboard);

        // When drawing it.
        let buf = draw(&picker, 60, 40);

        // Then the first legacy model is number 6.
        let lines = lines(&buf);
        let legacy = lines
            .iter()
            .skip_while(|line| !line.contains("Legacy models"))
            .nth(1)
            .map(|line| line.trim_matches(['│', ' ']));
        assert!(
            legacy.is_some_and(|line| line.starts_with("6. ")),
            "the first legacy model's row was {legacy:?}"
        );
    }

    #[rstest::rstest]
    fn selection_fill_spans_the_inner_width() {
        // Given a project picker with orb, the first project, selected.
        let picker = orb();

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then every cell between the borders on orb's row is filled.
        let filled =
            border_rows(&buf)
                .zip(border_columns(&buf))
                .map(|((top, _), (left, right))| {
                    (left + 1..right)
                        .all(|x| buf.cell((x, top + 3)).is_some_and(|cell| cell.bg == VISUAL))
                });
        assert_eq!(filled, Some(true), "the selected row's fill");
    }

    #[rstest::rstest]
    fn project_row_dims_the_parent_and_brightens_the_name() {
        // Given a project picker listing orb at ~/dev/orb.
        let picker = orb();

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then `~/dev/` is dim and `orb` is bright.
        let colours = find(&buf, "~/dev/orb")
            .map(|(x, y)| [x, x + 6].map(|x| buf.cell((x, y)).map(|cell| cell.fg)));
        assert_eq!(
            colours,
            Some([Some(DARK5), Some(FG)]),
            "the parent's and the name's colours"
        );
    }

    #[rstest::rstest]
    fn project_folder_takes_the_badge_colour() {
        // Given a project picker listing orb.
        let picker = orb();

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then the folder before orb's path is in orb's badge colour.
        let fg = find(&buf, &format!("{FOLDER} ~/dev/orb"))
            .and_then(|at| buf.cell(at))
            .map(|cell| cell.fg);
        assert_eq!(fg, badge("orb", true).style.fg, "the folder's colour");
    }

    #[rstest::rstest]
    fn matched_graphemes_are_blue_and_bold() {
        // Given a project picker filtered by "rb".
        let picker = {
            let mut picker =
                PickerState::projects(vec![project(1, "orb", "/Users/me/dev/orb")], Focus::Sidebar);
            picker.insert('r');
            picker.insert('b');
            picker
        };

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then the name's `r` is blue1 and bold.
        let r = find(&buf, "~/dev/orb").and_then(|(x, y)| buf.cell((x + 7, y)));
        assert!(
            r.is_some_and(|cell| cell.fg == BLUE1 && cell.modifier.contains(Modifier::BOLD)),
            "the matched r was {r:?}"
        );
    }

    #[rstest::rstest]
    #[case(
        PickerState::directories(PathBuf::from(HOME), Focus::Sidebar).0,
        "⏎ add · Tab open · Esc close"
    )]
    #[case(
        PickerState::project_filter(vec![PickerItem::AllProjects], None, Focus::Sidebar),
        "⏎ filter · <C-x> remove · Esc close"
    )]
    #[case(
        PickerState::remove_project(ProjectId(1), Focus::Sidebar),
        "⏎ confirm · Esc cancel"
    )]
    #[case(
        PickerState::init_git(ProjectId(1), Focus::Dashboard),
        "⏎ confirm · Esc cancel"
    )]
    #[case(
        PickerState::settle_thread(ThreadId(1), Focus::Sidebar),
        "⏎ confirm · Esc cancel"
    )]
    #[case(
        PickerState::delete_thread(ThreadId(1), Focus::Sidebar),
        "⏎ confirm · Esc cancel"
    )]
    #[case(
        PickerState::discard_draft(ProjectId(1), Focus::Sidebar),
        "⏎ confirm · Esc cancel"
    )]
    #[case::trust(
        PickerState::trust_workspace(PathBuf::from("/Users/me/dev/orb"), Focus::Sidebar),
        "⏎ confirm · Esc cancel"
    )]
    #[case(workspace(), "⏎ select · Esc close")]
    fn hints(#[case] picker: PickerState, #[case] expected: &str) {
        // Given a picker of some kind.

        // When drawing it on a narrow screen.
        let buf = draw(&picker, 60, 16);

        // Then its bottom border holds its keys.
        let bottom = border_rows(&buf).map(|(_, bottom)| bottom);
        let at = find(&buf, expected).map(|(_, y)| y);
        assert_eq!(at, bottom, "the row of the hints {expected:?}");
    }

    #[rstest::rstest]
    fn empty_picker_says_no_results() {
        // Given a project picker filtered down to nothing.
        let picker = {
            let mut picker = orb();
            picker.insert('z');
            picker
        };

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then the rows say there are no results.
        assert_eq!(
            inner_line(&buf, 3).as_deref(),
            Some("No results"),
            "the empty list's row"
        );
    }

    #[rstest::rstest]
    fn empty_directory_picker_asks_for_a_path() {
        // Given a directory picker with its `~/` erased.
        let picker = {
            let (mut picker, _) = PickerState::directories(PathBuf::from(HOME), Focus::Sidebar);
            let _ = picker.backspace();
            let _ = picker.backspace();
            picker
        };

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then the rows say to type a path.
        assert_eq!(
            inner_line(&buf, 3).as_deref(),
            Some("Type a path starting with / or ~/"),
            "the empty list's row"
        );
    }

    #[rstest::rstest]
    fn model_row_starts_with_the_claude_mark() {
        // Given a model picker.
        let picker =
            PickerState::models(DraftTarget::Project(ProjectId(1)), None, Focus::Dashboard);

        // When drawing it.
        let buf = draw(&picker, 60, 40);

        // Then Default follows the ✳ mark.
        let lines = lines(&buf);
        assert!(
            lines
                .iter()
                .any(|line| line.contains(&format!("{CLAUDE_LOGO} Default"))),
            "screen was {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn permission_row_starts_with_a_shield() {
        // Given a permission picker.
        let picker =
            PickerState::permissions(DraftTarget::Project(ProjectId(1)), None, Focus::Dashboard);

        // When drawing it.
        let buf = draw(&picker, 60, 40);

        // Then Default follows the shield.
        let lines = lines(&buf);
        assert!(
            lines
                .iter()
                .any(|line| line.contains(&format!("{SHIELD} Default"))),
            "screen was {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn directory_row_shows_a_folder_and_the_name() {
        // Given a directory picker listing `~/dev`.
        let picker = {
            let home = PathBuf::from(HOME);
            let (mut picker, _) = PickerState::directories(home.clone(), Focus::Sidebar);
            picker.show_directories(&home, vec!["dev".to_owned()]);
            picker
        };

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then the row is a folder glyph and the name.
        let lines = lines(&buf);
        assert!(
            lines
                .iter()
                .any(|line| line.contains(&format!("{FOLDER} dev"))),
            "screen was {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn selection_below_the_fold_scrolls_into_view() {
        // Given ten projects on a screen that fits four, the sixth selected.
        let picker = {
            let mut picker = PickerState::projects(ten_projects(), Focus::Sidebar);
            for _ in 0..5 {
                picker.next();
            }
            picker
        };

        // When drawing it.
        let buf = draw(&picker, 40, 12);

        // Then the sixth project is on screen.
        let lines = lines(&buf);
        assert!(
            lines.iter().any(|line| line.contains("/tmp/5")),
            "screen was {lines:#?}"
        );
    }

    fn workspace() -> PickerState {
        PickerState::workspace(
            PickTarget::Thread(ThreadId(1)),
            vec![
                PickerItem::Workspace(WorkspaceChoice::Current { worktree: false }),
                PickerItem::Workspace(WorkspaceChoice::NewWorktree),
                PickerItem::Workspace(WorkspaceChoice::Previous {
                    path: "/Users/me/.orb/worktrees/orb/orb-1a2b3c4d".into(),
                    branch: Some("orb/fix-login".to_owned()),
                }),
            ],
            Focus::Dashboard,
        )
    }

    #[rstest::rstest]
    fn previous_worktree_row_shows_its_branch() {
        // Given a workspace picker offering the previous worktree.
        let picker = workspace();

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then its row is the history glyph and the label with the branch.
        let lines = lines(&buf);
        assert!(
            lines
                .iter()
                .any(|line| line.contains(&format!("{HISTORY} Previous worktree (orb/fix-login)"))),
            "screen was {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn branch_picker_while_listing_shows_loading() {
        // Given a branch picker whose refs aren't listed yet.
        let picker = PickerState::branches(
            PickTarget::Thread(ThreadId(1)),
            "/tmp/repo".into(),
            false,
            None,
            Focus::Dashboard,
        );

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then the rows say the branches are loading.
        let lines = lines(&buf);
        assert!(
            lines.iter().any(|line| line.contains("Loading branches…")),
            "screen was {lines:#?}"
        );
    }

    /// The repository the branch pickers list, and the thread's directory.
    const REPO: &str = "/Users/me/dev/orb";

    fn branch(name: &str, worktree: Option<&str>) -> GitRef {
        GitRef {
            name: name.to_owned(),
            remote: false,
            current: false,
            default: false,
            worktree: worktree.map(PathBuf::from),
        }
    }

    /// A branch picker in `REPO` listing `refs`, after the first prompt.
    fn branches(refs: Vec<GitRef>) -> PickerState {
        let mut picker = PickerState::branches(
            PickTarget::Thread(ThreadId(1)),
            REPO.into(),
            false,
            None,
            Focus::Dashboard,
        );
        picker.show_branches(Path::new(REPO), refs);
        picker
    }

    #[rstest::rstest]
    fn branch_row_shows_its_badge() {
        // Given a branch picker listing the default branch.
        let picker = branches(vec![GitRef {
            default: true,
            ..branch("main", None)
        }]);

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then main's row ends with the default badge.
        let line = line_with(&buf, "main");
        assert!(
            line.as_deref().is_some_and(|line| line
                .trim_end()
                .trim_end_matches('│')
                .trim_end()
                .ends_with("default")),
            "main's row was {line:?}"
        );
    }

    /// A branch picker with `feature` checked out in another worktree,
    /// disabled, under `main`.
    fn with_disabled() -> PickerState {
        branches(vec![
            GitRef {
                current: true,
                ..branch("main", Some(REPO))
            },
            branch("feature", Some("/Users/me/.orb/worktrees/orb/orb-1a2b3c4d")),
        ])
    }

    #[rstest::rstest]
    fn disabled_branch_row_is_dimmed() {
        // Given a branch picker with feature disabled.
        let picker = with_disabled();

        // When drawing it.
        let buf = draw(&picker, 80, 16);

        // Then feature's name is dark3.
        let fg = find(&buf, "feature")
            .and_then(|at| buf.cell(at))
            .map(|cell| cell.fg);
        assert_eq!(fg, Some(DARK3), "the disabled name's colour");
    }

    #[rstest::rstest]
    fn disabled_branch_row_shows_where_it_is_checked_out() {
        // Given a branch picker with feature disabled.
        let picker = with_disabled();

        // When drawing it on a screen wide enough for the whole path.
        let buf = draw(&picker, 160, 16);

        // Then feature's row says where it's checked out, under `~`.
        let line = line_with(&buf, "feature");
        assert!(
            line.as_deref()
                .is_some_and(|line| line.contains("in ~/.orb/worktrees/orb/orb-1a2b3c4d")),
            "feature's row was {line:?}"
        );
    }

    #[rstest::rstest]
    fn long_checkout_path_is_cut_from_the_left() {
        // Given a branch picker with feature disabled.
        let picker = with_disabled();

        // When drawing it too narrow for the whole path.
        let buf = draw(&picker, 40, 16);

        // Then feature's row keeps the path's end after an ellipsis.
        let line = line_with(&buf, "feature");
        assert!(
            line.as_deref()
                .is_some_and(|line| line.contains("…") && line.contains("1a2b3c4d")),
            "feature's row was {line:?}"
        );
    }

    #[rstest::rstest]
    fn cut_checkout_path_keeps_the_in_prefix() {
        // Given a branch picker with feature disabled.
        let picker = with_disabled();

        // When drawing it too narrow for the whole path.
        let buf = draw(&picker, 40, 16);

        // Then feature's row still says where, before the cut path.
        let line = line_with(&buf, "feature");
        assert!(
            line.as_deref().is_some_and(|line| line.contains("in …")),
            "feature's row was {line:?}"
        );
    }

    #[rstest::rstest]
    fn all_disabled_branch_picker_draws_no_selection_fill() {
        // Given a branch picker whose only row is disabled.
        let picker = branches(vec![branch(
            "feature",
            Some("/Users/me/.orb/worktrees/orb/orb-1a2b3c4d"),
        )]);

        // When drawing it.
        let buf = draw(&picker, 80, 16);

        // Then the cell left of feature isn't filled.
        let bg = find(&buf, "feature")
            .and_then(|(x, y)| buf.cell((x - 1, y)))
            .map(|cell| cell.bg);
        assert_ne!(bg, Some(VISUAL), "the disabled row's background");
    }

    #[rstest::rstest]
    fn model_picker_lists_default_first() {
        // Given a model picker.
        let picker = PickerState::models(
            DraftTarget::Project(ProjectId(1)),
            Some("sonnet"),
            Focus::Dashboard,
        );

        // When drawing it.
        let buf = draw(&picker, 60, 40);

        // Then the first row, under the rule, is Default.
        assert_eq!(
            inner_line(&buf, 3),
            Some(format!("1. {CLAUDE_LOGO} Default")),
            "the first row"
        );
    }

    #[rstest::rstest]
    fn model_picker_names_the_models() {
        // Given a model picker.
        let picker =
            PickerState::models(DraftTarget::Project(ProjectId(1)), None, Focus::Dashboard);

        // When drawing it.
        let buf = draw(&picker, 60, 40);

        // Then the row after Default is Claude Opus 5.5, not its ID.
        assert_eq!(
            inner_line(&buf, 4),
            Some(format!("2. {CLAUDE_LOGO} Claude Opus 5.5")),
            "the second row"
        );
    }

    #[rstest::rstest]
    fn model_picker_labels_the_legacy_models() {
        // Given a model picker.
        let picker =
            PickerState::models(DraftTarget::Project(ProjectId(1)), None, Focus::Dashboard);

        // When drawing it.
        let buf = draw(&picker, 60, 40);

        // Then a Legacy models heading follows Claude Sonnet 5.
        let lines = lines(&buf);
        let after = lines
            .iter()
            .skip_while(|line| !line.contains("Claude Sonnet 5"))
            .nth(1);
        assert!(
            after.is_some_and(|line| line.trim_matches(['│', ' ']) == "── Legacy models ──"),
            "screen was {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn init_git_picker_offers_initialize_git() {
        // Given the picker a non-git draft's ␣w or ␣b opens.
        let picker = PickerState::init_git(ProjectId(1), Focus::Dashboard);

        // When drawing it.
        let buf = draw(&picker, 60, 20);

        // Then its one row is Initialize Git.
        assert_eq!(
            inner_line(&buf, 3),
            Some(format!("1. {GIT} Initialize Git")),
            "the only row"
        );
    }

    #[rstest::rstest]
    fn project_filter_lists_all_projects_above_the_projects() {
        // Given the project filter over All projects and orb.
        let picker = PickerState::project_filter(
            vec![
                PickerItem::AllProjects,
                project(1, "orb", "/Users/me/dev/orb"),
            ],
            None,
            Focus::Sidebar,
        );

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then All projects is drawn above orb.
        let rows = (find(&buf, "All projects"), find(&buf, "~/dev/orb"));
        assert!(
            matches!(rows, (Some((_, all)), Some((_, orb))) if all < orb),
            "rows were at {rows:?}"
        );
    }

    #[rstest::rstest]
    #[case::research(ProjectKind::Research, "Research", "/Users/me/.orb/research")]
    #[case::incognito(ProjectKind::Incognito, "Incognito", "/tmp/orb-incognito")]
    fn orbs_own_project_row_reads_just_its_name(
        #[case] kind: ProjectKind,
        #[case] title: &str,
        #[case] root: &str,
    ) {
        // Given the project filter over orb and one of orb's own projects.
        let own = PickerItem::Project {
            id: ProjectId(2),
            title: title.to_owned(),
            root: root.into(),
            kind,
        };
        let picker = PickerState::project_filter(
            vec![project(1, "orb", "/Users/me/dev/orb"), own],
            None,
            Focus::Sidebar,
        );

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then its row, the second, is its folder and name, with no path or
        // title on the right.
        let row = lines(&buf)
            .into_iter()
            .find(|line| line.contains(title))
            .map(|line| line.trim_matches(['│', ' ']).to_owned());
        assert_eq!(row, Some(format!("2. {FOLDER} {title}")), "the {title} row");
    }

    #[rstest::rstest]
    fn remove_confirm_asks_to_remove_the_project() {
        // Given the confirm for removing project 1.
        let picker = PickerState::remove_project(ProjectId(1), Focus::Sidebar);

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then its title asks.
        let lines = lines(&buf);
        assert!(
            lines.iter().any(|line| line.contains("Remove project?")),
            "screen was {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn remove_confirm_lists_no_above_yes() {
        // Given the confirm for removing project 1.
        let picker = PickerState::remove_project(ProjectId(1), Focus::Sidebar);

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then No is drawn on the line above Yes.
        let rows = (find(&buf, "No"), find(&buf, "Yes"));
        assert!(
            matches!(rows, (Some((_, no)), Some((_, yes))) if no + 1 == yes),
            "rows were at {rows:?}"
        );
    }

    #[rstest::rstest]
    fn long_filter_shows_its_end_before_the_cursor() {
        // Given a filter wider than the popup, ending in `q`.
        let picker = {
            let mut picker = orb();
            for _ in 0..50 {
                picker.insert('z');
            }
            picker.insert('q');
            picker
        };

        // When drawing the picker on an 80-column screen.
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 20));
        let (_, cursor) = render(
            &picker,
            Path::new(HOME),
            buf.area,
            &mut buf,
            &mut PickerScroll::default(),
        );

        // Then the cell before the cursor holds the `q`, inside the right border.
        let before = buf.cell((cursor.x - 1, cursor.y)).map(Cell::symbol);
        assert_eq!(
            (
                before,
                border_columns(&buf).map(|(_, right)| cursor.x < right)
            ),
            (Some("q"), Some(true)),
            "the filter's end before the cursor at {cursor:?}"
        );
    }
}
