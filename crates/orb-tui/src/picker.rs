//! The picker popup, in T3 Code's command-palette look: a rounded box over
//! the screen holding the filter input, a section label, the rows, and a
//! footer of keys. The box is only as tall as its rows need, and its top
//! stays put while the filter narrows them.
//!
//! The project picker shows each project's badge and name over its path; the
//! directory picker shows one folder per row; the workspace picker shows where
//! a thread's session could run, each with its glyph; the branch picker shows
//! each branch with its badge, dimming the ones checked out where the thread
//! can't follow and saying where; the model picker shows each model's name,
//! with its legacy models under their own label; a draft whose project isn't
//! a git repository gets one row, `Initialize Git`. Where the filter matched is
//! bold and underlined, and the rows scroll to keep the selection in view.

use std::path::Path;

use orb_domain::feat::git::git_service::GitRef;
use orb_domain::feat::picker::list::{
    BranchRow, INIT_GIT, Matches, PickerItem, WorkspaceChoice, setting_label,
};
use orb_domain::feat::picker::state::{PickerKind, PickerState, split_path};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Margin, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Widget};
use unicode_segmentation::UnicodeSegmentation;

use crate::sidebar::{DARK_GRAY, GRAY, OUTLINE, SELECTED, badge};

/// Nerd Font's search glyph, before every picker's input but the directory
/// picker's.
const SEARCH: &str = "\u{f002}";
/// Nerd Font's folder glyph, before the directory picker's input and rows, and
/// the current checkout's workspace row.
const FOLDER: &str = "\u{f07b}";
/// Nerd Font's code-fork glyph, before the worktree workspace rows.
const WORKTREE: &str = "\u{f126}";
/// Nerd Font's history glyph, before the previous worktree's workspace row.
const HISTORY: &str = "\u{f1da}";
/// The widest the popup gets, in columns.
const MAX_WIDTH: u16 = 90;
/// The popup's lines besides its rows: the borders, a blank line inside each,
/// the input, the gap and label above the rows, and the gap and footer below.
const CHROME: u16 = 9;

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
    let directories = matches!(picker.kind(), PickerKind::Directories { .. });
    let row_height: u16 = match picker.kind() {
        PickerKind::Projects => 2,
        _ => 1,
    };
    let popup = {
        let rows = u16::try_from(picker.shown().count())
            .unwrap_or(u16::MAX)
            .saturating_mul(row_height)
            .max(1);
        popup_rect(area, rows)
    };
    Clear.render(popup, buf);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(OUTLINE));
    let inner = block.inner(popup).inner(Margin::new(2, 1));
    block.render(popup, buf);
    let [input, _, label, rows, _, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let cursor = render_input(picker, directories, pad(input), buf);
    Line::styled(section_label(picker.kind()), Style::new().fg(GRAY)).render(pad(label), buf);
    let page = render_rows(picker, home, row_height, rows, buf, scroll);
    render_footer(directories, pad(footer), buf);
    (page, cursor)
}

/// The label above the rows.
fn section_label(kind: &PickerKind) -> &'static str {
    match kind {
        PickerKind::Projects => "Projects",
        PickerKind::Directories { .. } => "Directories",
        PickerKind::Workspace { .. } => "Workspace",
        PickerKind::Branches { .. } => "Branches",
        PickerKind::Model { .. } => "Models",
        PickerKind::Permission { .. } => "Permission modes",
        PickerKind::InitGit { .. } => "Not a git repository",
    }
}

/// jinn's popup, 80% of the width (at least 30 columns) and 75% of the
/// height plus 4 rows, narrowed to `MAX_WIDTH` and shortened to fit `rows`
/// lines of results. It's centred across, and its top stays where the
/// full-height popup's would be, a third of the way down, so filtering
/// doesn't move the input.
fn popup_rect(area: Rect, rows: u16) -> Rect {
    let width = (area.width - area.width / 5)
        .clamp(30, MAX_WIDTH)
        .min(area.width);
    let full = (area.height - area.height.div_ceil(4))
        .saturating_add(4)
        .min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - full) / 3,
        width,
        rows.saturating_add(CHROME).min(full),
    )
}

/// A row's text area: two cells in from each side of the selection fill.
fn pad(area: Rect) -> Rect {
    area.inner(Margin::new(2, 0))
}

/// The glyph and the typed text, or the project picker's placeholder.
/// Returns the cursor's position.
fn render_input(picker: &PickerState, directories: bool, area: Rect, buf: &mut Buffer) -> Position {
    let glyph = Span::styled(
        format!("{} ", if directories { FOLDER } else { SEARCH }),
        Style::new().fg(DARK_GRAY),
    );
    let text = match picker.input() {
        "" if matches!(picker.kind(), PickerKind::Projects) => {
            Span::styled("Search projects...", Style::new().fg(DARK_GRAY))
        }
        input => Span::raw(input),
    };
    let before: String = picker
        .input()
        .graphemes(true)
        .take(picker.cursor())
        .collect();
    let column = glyph.width() + Line::raw(before).width();
    Line::from(vec![glyph, text]).render(area, buf);
    let x = u16::try_from(column)
        .unwrap_or(u16::MAX)
        .min(area.width.saturating_sub(1));
    Position::new(area.x + x, area.y)
}

/// The rows from the scroll offset down, or a hint when there are none.
/// Returns how many rows fit.
fn render_rows(
    picker: &PickerState,
    home: &Path,
    row_height: u16,
    area: Rect,
    buf: &mut Buffer,
    scroll: &mut PickerScroll,
) -> usize {
    let page = usize::from(area.height / row_height).max(1);
    let shown: Vec<(&PickerItem, &Matches)> = picker.shown().collect();
    if shown.is_empty() {
        Line::styled(empty_hint(picker), Style::new().fg(DARK_GRAY)).render(pad(area), buf);
        return page;
    }
    let selection = picker.selection();
    scroll.offset = scroll
        .offset
        .min(selection)
        .max((selection + 1).saturating_sub(page))
        .min(shown.len().saturating_sub(page));
    let tops = (area.top()..area.bottom()).step_by(usize::from(row_height));
    for ((index, (item, matches)), top) in
        shown.into_iter().enumerate().skip(scroll.offset).zip(tops)
    {
        let row = Rect::new(area.x, top, area.width, row_height).intersection(area);
        if index == selection && picker.selected().is_some() {
            buf.set_style(row, Style::new().bg(SELECTED));
        }
        render_item(item, matches, picker.kind(), home, pad(row), buf);
    }
    page
}

/// A project's badge and name over its path, a directory's folder and name,
/// or a workspace's glyph and label.
fn render_item(
    item: &PickerItem,
    matches: &Matches,
    kind: &PickerKind,
    home: &Path,
    area: Rect,
    buf: &mut Buffer,
) {
    match item {
        PickerItem::Project { title, root, .. } => {
            let [name_area, path_area] = Layout::vertical([Constraint::Length(1); 2]).areas(area);
            let name = [badge(title, true), Span::raw(" ")]
                .into_iter()
                .chain(highlight(
                    title,
                    &matches.name,
                    Style::new().fg(Color::White),
                ));
            Line::from_iter(name).render(name_area, buf);
            let path = std::iter::once(Span::raw("   ")).chain(highlight(
                &root.display().to_string(),
                &matches.path,
                Style::new().fg(DARK_GRAY),
            ));
            Line::from_iter(path).render(path_area, buf);
        }
        PickerItem::Directory { name } => {
            let row = [Span::styled(FOLDER, Style::new().fg(GRAY)), Span::raw(" ")]
                .into_iter()
                .chain(highlight(name, &matches.name, Style::new()));
            Line::from_iter(row).render(area, buf);
        }
        PickerItem::Workspace(choice) => {
            let row = [
                Span::styled(workspace_glyph(choice), Style::new().fg(GRAY)),
                Span::raw(" "),
            ]
            .into_iter()
            .chain(highlight(&choice.label(), &matches.name, Style::new()));
            Line::from_iter(row).render(area, buf);
        }
        PickerItem::Branch(row) => {
            let cwd = match kind {
                PickerKind::Branches { cwd, .. } => cwd.as_path(),
                _ => Path::new(""),
            };
            render_branch(row, matches, cwd, home, area, buf);
        }
        PickerItem::Setting(value) => {
            Line::from_iter(highlight(
                setting_label(*value),
                &matches.name,
                Style::new(),
            ))
            .render(area, buf);
        }
        PickerItem::Heading(text) => {
            Line::styled(*text, Style::new().fg(GRAY)).render(area, buf);
        }
        PickerItem::InitGit => {
            Line::from_iter(highlight(INIT_GIT, &matches.name, Style::new())).render(area, buf);
        }
    }
}

/// A branch's name, and on the right its badge, or where it's checked out
/// when the row is disabled, the whole row dimmed.
fn render_branch(
    row: &BranchRow,
    matches: &Matches,
    cwd: &Path,
    home: &Path,
    area: Rect,
    buf: &mut Buffer,
) {
    let git_ref = &row.git_ref;
    let (name_colour, prefix, right) = match (row.disabled, &git_ref.worktree) {
        (true, Some(path)) => (DARK_GRAY, "in ", tilde(path, home)),
        _ => (
            Color::White,
            "",
            branch_badge(git_ref, cwd).unwrap_or_default().to_owned(),
        ),
    };
    let name = Line::from_iter(highlight(
        &git_ref.name,
        &matches.name,
        Style::new().fg(name_colour),
    ));
    let room = usize::from(area.width).saturating_sub(name.width() + 2 + prefix.len());
    let right = Line::styled(
        format!("{prefix}{}", cut_left(&right, room)),
        Style::new().fg(DARK_GRAY),
    );
    let right_width = u16::try_from(right.width()).unwrap_or(u16::MAX);
    name.render(area, buf);
    right.render(
        Rect {
            x: area.right().saturating_sub(right_width),
            width: right_width.min(area.width),
            ..area
        },
        buf,
    );
}

/// The badge on a branch row, by T3's priority: the branch this directory is
/// on, one checked out in another worktree, a remote ref, the default branch.
fn branch_badge(git_ref: &GitRef, cwd: &Path) -> Option<&'static str> {
    let elsewhere = git_ref.worktree.as_deref().is_some_and(|path| path != cwd);
    match git_ref {
        GitRef { current: true, .. } => Some("current"),
        _ if elsewhere => Some("worktree"),
        GitRef { remote: true, .. } => Some("remote"),
        GitRef { default: true, .. } => Some("default"),
        _ => None,
    }
}

/// `path` with `home` shown as `~`.
pub(crate) fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if !home.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
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

/// The glyph before a workspace row: a folder for the root checkout, a fork
/// for a worktree, history for the previous worktree.
fn workspace_glyph(choice: &WorkspaceChoice) -> &'static str {
    match choice {
        WorkspaceChoice::Current { worktree: false } => FOLDER,
        WorkspaceChoice::Current { worktree: true } | WorkspaceChoice::NewWorktree => WORKTREE,
        WorkspaceChoice::Previous { .. } => HISTORY,
    }
}

/// `text` in `style`, with each grapheme holding one of `offsets` (byte
/// offsets into `text`) bold and underlined.
fn highlight(text: &str, offsets: &[usize], style: Style) -> Vec<Span<'static>> {
    let matched = style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (start, grapheme) in text.grapheme_indices(true) {
        let bytes = start..start + grapheme.len();
        let style = if offsets.iter().any(|offset| bytes.contains(offset)) {
            matched
        } else {
            style
        };
        match spans.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push_str(grapheme),
            _ => spans.push(Span::styled(grapheme.to_owned(), style)),
        }
    }
    spans
}

/// Why no rows are shown.
fn empty_hint(picker: &PickerState) -> &'static str {
    match (picker.kind(), split_path(picker.input())) {
        (PickerKind::Projects, _) if picker.input().trim().is_empty() => {
            "No projects — ␣p adds one"
        }
        (PickerKind::Directories { .. }, None) => "Type a path starting with / or ~/",
        (PickerKind::Directories { .. }, Some((_, ""))) => "No directories",
        (PickerKind::Branches { .. }, _) if picker.input().is_empty() => "Loading branches…",
        (PickerKind::Branches { .. }, _) => "No matching branches",
        _ => "No matches",
    }
}

/// The picker's keys as chips: the key on a filled tile, then what it does.
fn render_footer(directories: bool, area: Rect, buf: &mut Buffer) {
    let keys: &[(&str, &str)] = if directories {
        &[
            ("↑ ↓", "Navigate"),
            ("Tab", "Open"),
            ("Enter", "Add"),
            ("Esc", "Close"),
        ]
    } else {
        &[("↑ ↓", "Navigate"), ("Enter", "Select"), ("Esc", "Close")]
    };
    let chips = keys.iter().flat_map(|(key, label)| {
        [
            Span::styled(
                format!(" {key} "),
                Style::new().fg(Color::White).bg(SELECTED),
            ),
            Span::styled(format!(" {label}   "), Style::new().fg(GRAY)),
        ]
    });
    Line::from_iter(chips).render(area, buf);
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use orb_domain::Focus;
    use orb_domain::feat::git::git_service::GitRef;
    use orb_domain::feat::picker::list::{PickerItem, WorkspaceChoice};
    use orb_domain::feat::picker::state::{PickTarget, PickerState};
    use orb_domain::feat::sessions::state::{ProjectId, ThreadId};
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;
    use unicode_segmentation::UnicodeSegmentation;

    use super::{DARK_GRAY, FOLDER, HISTORY, PickerScroll, SELECTED, render};

    /// The home directory the pickers are drawn with.
    const HOME: &str = "/Users/me";

    fn project(id: i64, title: &str, root: &str) -> PickerItem {
        PickerItem::Project {
            id: ProjectId(id),
            title: title.to_owned(),
            root: root.into(),
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

    fn orb() -> PickerState {
        PickerState::projects(
            vec![
                project(1, "orb", "/Users/me/dev/orb"),
                project(2, "jinn", "/Users/me/dev/jinn"),
            ],
            Focus::Sidebar,
        )
    }

    #[rstest::rstest]
    fn project_row_shows_the_badge_and_name() {
        // Given a project picker listing orb.
        let picker = orb();

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then orb's first line is its badge and name.
        let lines = lines(&buf);
        assert!(
            lines.iter().any(|line| line.contains("OB orb")),
            "screen was {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn project_row_shows_the_path_dimmed_below_the_name() {
        // Given a project picker listing orb.
        let picker = orb();

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then the line below the name is the path, in dark gray.
        let name = find(&buf, "OB orb");
        let path = find(&buf, "/Users/me/dev/orb");
        let fg = path.and_then(|at| buf.cell(at)).map(|cell| cell.fg);
        assert_eq!(
            (path.map(|(_, y)| y), fg),
            (name.map(|(_, y)| y + 1), Some(DARK_GRAY)),
            "the path's row and colour"
        );
    }

    #[rstest::rstest]
    fn selected_project_is_filled_on_both_lines() {
        // Given a project picker with orb, the first project, selected.
        let picker = orb();

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then the cell left of the text is filled on the name and path lines.
        let fills = find(&buf, "OB orb")
            .map(|(x, y)| [y, y + 1].map(|y| buf.cell((x - 1, y)).map(|cell| cell.bg)));
        assert_eq!(
            fills,
            Some([Some(SELECTED); 2]),
            "the selected row's background"
        );
    }

    #[rstest::rstest]
    fn matched_graphemes_are_bold_and_underlined() {
        // Given a project picker filtered by "rb".
        let picker = {
            let mut picker =
                PickerState::projects(vec![project(1, "orb", "/tmp/x")], Focus::Sidebar);
            picker.insert('r');
            picker.insert('b');
            picker
        };

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then the title's `r` is bold and underlined.
        let r = find(&buf, "orb").and_then(|(x, y)| buf.cell((x + 1, y)));
        assert!(
            r.is_some_and(|cell| cell
                .modifier
                .contains(Modifier::BOLD | Modifier::UNDERLINED)),
            "the matched r was {r:?}"
        );
    }

    #[rstest::rstest]
    fn directory_picker_is_labelled_directories() {
        // Given a directory picker at `~/`.
        let (picker, _) = PickerState::directories(PathBuf::from("/Users/me"), Focus::Sidebar);

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then its section label says Directories.
        let lines = lines(&buf);
        assert!(
            lines.iter().any(|line| line.contains("Directories")),
            "screen was {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn directory_row_shows_a_folder_and_the_name() {
        // Given a directory picker listing `~/dev`.
        let picker = {
            let home = PathBuf::from("/Users/me");
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
    fn empty_project_picker_shows_the_search_placeholder() {
        // Given a project picker with no projects.
        let picker = PickerState::projects(vec![], Focus::Sidebar);

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then the input shows the placeholder.
        let lines = lines(&buf);
        assert!(
            lines.iter().any(|line| line.contains("Search projects...")),
            "screen was {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn empty_project_picker_says_how_to_add_one() {
        // Given a project picker with no projects.
        let picker = PickerState::projects(vec![], Focus::Sidebar);

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then the rows say how to add a project.
        let lines = lines(&buf);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("No projects — ␣p adds one")),
            "screen was {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn selection_below_the_fold_scrolls_into_view() {
        // Given ten projects on a screen that fits one, the sixth selected.
        let picker = {
            let items = (0..10)
                .map(|i| project(i, &format!("proj{i}"), &format!("/tmp/{i}")))
                .collect();
            let mut picker = PickerState::projects(items, Focus::Sidebar);
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
            lines.iter().any(|line| line.contains("P5 proj5")),
            "screen was {lines:#?}"
        );
    }

    /// The rows of the popup's top and bottom borders.
    fn border_rows(buf: &Buffer) -> Option<(u16, u16)> {
        find(buf, "╭")
            .zip(find(buf, "╰"))
            .map(|((_, top), (_, bottom))| (top, bottom))
    }

    #[rstest::rstest]
    fn popup_is_as_tall_as_its_rows() {
        // Given a project picker with two projects on a tall screen.
        let picker = orb();

        // When drawing it.
        let buf = draw(&picker, 100, 40);

        // Then the popup is 13 lines: its two 2-line rows and 9 of chrome.
        let height = border_rows(&buf).map(|(top, bottom)| bottom - top + 1);
        assert_eq!(height, Some(13), "the popup's height");
    }

    #[rstest::rstest]
    fn popup_is_at_most_90_columns_wide() {
        // Given a project picker.
        let picker = orb();

        // When drawing it on a 200-column screen.
        let buf = draw(&picker, 200, 40);

        // Then the popup's top border spans 90 columns.
        let width = find(&buf, "╭")
            .zip(find(&buf, "╮"))
            .map(|((left, _), (right, _))| right - left + 1);
        assert_eq!(width, Some(90), "the popup's width");
    }

    #[rstest::rstest]
    fn popup_top_stays_when_the_filter_hides_rows() {
        // Given ten projects, unfiltered and filtered down to none.
        let items: Vec<PickerItem> = (0..10)
            .map(|i| project(i, &format!("proj{i}"), &format!("/tmp/{i}")))
            .collect();
        let unfiltered = PickerState::projects(items.clone(), Focus::Sidebar);
        let filtered = {
            let mut picker = PickerState::projects(items, Focus::Sidebar);
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
    fn selection_fill_stops_short_of_the_border() {
        // Given a project picker with orb selected.
        let picker = orb();

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then the two cells inside the left border beside orb's row are
        // unfilled, and the third is filled.
        let border = find(&buf, "╭").map(|(x, _)| x);
        let fills = border.zip(find(&buf, "OB orb")).map(|(x, (_, y))| {
            [1, 2, 3].map(|dx| buf.cell((x + dx, y)).map(|cell| cell.bg == SELECTED))
        });
        assert_eq!(
            fills,
            Some([Some(false), Some(false), Some(true)]),
            "the fill beside the left border"
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
            Focus::Preview,
        )
    }

    #[rstest::rstest]
    fn workspace_picker_is_labelled_workspace() {
        // Given a workspace picker.
        let picker = workspace();

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then its section label says Workspace.
        let lines = lines(&buf);
        assert!(
            lines.iter().any(|line| line.contains("Workspace")),
            "screen was {lines:#?}"
        );
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
    fn workspace_picker_shows_the_select_footer() {
        // Given a workspace picker.
        let picker = workspace();

        // When drawing it.
        let buf = draw(&picker, 60, 16);

        // Then the footer offers Enter to select, not Tab to open.
        let lines = lines(&buf);
        assert!(
            lines
                .iter()
                .any(|line| line.contains(" Enter  Select") && !line.contains("Tab")),
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
            Focus::Preview,
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
            Focus::Preview,
        );
        picker.show_branches(Path::new(REPO), refs);
        picker
    }

    /// The line holding `text`.
    fn line_with(buf: &Buffer, text: &str) -> Option<String> {
        lines(buf).into_iter().find(|line| line.contains(text))
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

        // Then feature's name is dark gray.
        let fg = find(&buf, "feature")
            .and_then(|at| buf.cell(at))
            .map(|cell| cell.fg);
        assert_eq!(fg, Some(DARK_GRAY), "the disabled name's colour");
    }

    #[rstest::rstest]
    fn disabled_branch_row_shows_where_it_is_checked_out() {
        // Given a branch picker with feature disabled.
        let picker = with_disabled();

        // When drawing it.
        let buf = draw(&picker, 80, 16);

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
        assert_ne!(bg, Some(SELECTED), "the disabled row's background");
    }

    #[rstest::rstest]
    #[case(PickerState::models(ProjectId(1), None, Focus::Preview), "Models")]
    #[case(
        PickerState::permissions(ProjectId(1), None, Focus::Preview),
        "Permission modes"
    )]
    fn setting_picker_is_labelled_by_its_setting(#[case] picker: PickerState, #[case] label: &str) {
        // Given a model or permission picker.

        // When drawing it.
        let buf = draw(&picker, 60, 20);

        // Then its section label names the setting.
        let lines = lines(&buf);
        assert!(
            lines
                .iter()
                .any(|line| line.contains(&format!("  {label} "))),
            "screen was {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn model_picker_lists_default_first() {
        // Given a model picker.
        let picker = PickerState::models(ProjectId(1), Some("sonnet"), Focus::Preview);

        // When drawing it.
        let buf = draw(&picker, 60, 20);

        // Then the row under the label is Default.
        let lines = lines(&buf);
        let first = lines
            .iter()
            .skip_while(|line| !line.contains("Models"))
            .nth(1);
        assert!(
            first.is_some_and(|line| line.trim_matches(['│', ' ']) == "Default"),
            "screen was {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn model_picker_names_the_models() {
        // Given a model picker.
        let picker = PickerState::models(ProjectId(1), None, Focus::Preview);

        // When drawing it.
        let buf = draw(&picker, 60, 40);

        // Then the row after Default is Claude Opus 5.5, not its ID.
        let lines = lines(&buf);
        let second = lines
            .iter()
            .skip_while(|line| !line.contains("Models"))
            .nth(2);
        assert!(
            second.is_some_and(|line| line.trim_matches(['│', ' ']) == "Claude Opus 5.5"),
            "screen was {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn model_picker_labels_the_legacy_models() {
        // Given a model picker.
        let picker = PickerState::models(ProjectId(1), None, Focus::Preview);

        // When drawing it.
        let buf = draw(&picker, 60, 40);

        // Then a Legacy models label follows Claude Sonnet 5.
        let lines = lines(&buf);
        let after = lines
            .iter()
            .skip_while(|line| !line.contains("Claude Sonnet 5"))
            .nth(1);
        assert!(
            after.is_some_and(|line| line.trim_matches(['│', ' ']) == "Legacy models"),
            "screen was {lines:#?}"
        );
    }

    #[rstest::rstest]
    fn init_git_picker_offers_initialize_git() {
        // Given the picker a non-git draft's ␣w or ␣b opens.
        let picker = PickerState::init_git(ProjectId(1), Focus::Preview);

        // When drawing it.
        let buf = draw(&picker, 60, 20);

        // Then its one row, under the label, is Initialize Git.
        let lines = lines(&buf);
        let first = lines
            .iter()
            .skip_while(|line| !line.contains("Not a git repository"))
            .nth(1);
        assert!(
            first.is_some_and(|line| line.trim_matches(['│', ' ']) == "Initialize Git"),
            "screen was {lines:#?}"
        );
    }
}
