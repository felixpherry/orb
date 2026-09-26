//! The draft form: what the right side shows while a draft is selected.
//!
//! It names the project and lists the four settings the draft's session will
//! start with — workspace, base branch, model and permission — each with the
//! leader key that picks it, then how to start. An existing worktree shows its
//! path, cut from the left when it's too long; its branch is its own and can't
//! be picked, so that row is dimmed.

use std::path::Path;

use orb_domain::feat::picker::list::setting_label;
use orb_domain::feat::sessions::state::{Draft, DraftWorkspace, Project};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::picker::{cut_left, tilde};
use crate::sidebar::{DARK_GRAY, GRAY, badge, render_split, workspace_label};

/// Draws `project`'s `draft` into `area`; paths under `home` show as `~/`.
pub(crate) fn render(project: &Project, draft: &Draft, home: &Path, area: Rect, buf: &mut Buffer) {
    let [header, workspace, branch, model, permission, _, start] =
        Layout::vertical([Constraint::Length(1); 7]).areas(area);
    Line::from(vec![
        Span::raw("New thread · "),
        badge(&project.title, true),
        Span::raw(" "),
        Span::raw(project.title.as_str()),
    ])
    .render(header, buf);
    let place = match &draft.workspace {
        DraftWorkspace::Existing(path) => tilde(path, home),
        DraftWorkspace::Local | DraftWorkspace::NewWorktree => {
            workspace_label(&draft.workspace).to_owned()
        }
    };
    let read_only = matches!(draft.workspace, DraftWorkspace::Existing(_));
    render_field("Workspace", &place, "␣w", false, workspace, buf);
    render_field(
        "Base branch",
        draft.branch.as_deref().unwrap_or("unknown"),
        "␣b",
        read_only,
        branch,
        buf,
    );
    render_field(
        "Model",
        setting_label(draft.model.as_deref()),
        "␣m",
        false,
        model,
        buf,
    );
    render_field(
        "Permission",
        setting_label(draft.permission.as_deref()),
        "␣a",
        false,
        permission,
        buf,
    );
    Line::raw("⏎ start").render(start, buf);
}

/// The column a field's value starts at.
const VALUE_X: usize = 13;

/// A setting's label and value, with the key that picks it on the right; all
/// dimmed when it can't be picked. A value too long for the row is cut from
/// the left.
fn render_field(label: &str, value: &str, key: &str, dim: bool, area: Rect, buf: &mut Buffer) {
    let (label_colour, value_colour) = if dim {
        (DARK_GRAY, DARK_GRAY)
    } else {
        (GRAY, Color::White)
    };
    let key = Line::styled(key, Style::new().fg(DARK_GRAY));
    let room = usize::from(area.width).saturating_sub(VALUE_X + key.width() + 1);
    let row = Line::from(vec![
        Span::styled(format!("{label:<VALUE_X$}"), Style::new().fg(label_colour)),
        Span::styled(cut_left(value, room), Style::new().fg(value_colour)),
    ]);
    render_split(row, key, area, buf);
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use orb_domain::feat::sessions::state::{Draft, DraftWorkspace, Project, ProjectId};
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::Rect;
    use ratatui::style::Color;

    use super::render;
    use crate::sidebar::DARK_GRAY;

    fn orb() -> Project {
        Project {
            id: ProjectId(1),
            title: "orb".to_owned(),
            root: "/Users/me/dev/orb".into(),
            created_at: SystemTime::UNIX_EPOCH,
            threads: vec![],
            draft: None,
        }
    }

    /// A draft in `workspace` on `dev` with Claude's default model and a
    /// plan-mode permission.
    fn draft(workspace: DraftWorkspace) -> Draft {
        Draft {
            workspace,
            branch: Some("dev".to_owned()),
            model: None,
            permission: Some("plan".to_owned()),
            created_at: SystemTime::UNIX_EPOCH,
        }
    }

    /// Draws `draft`'s form 48 columns wide, with `/Users/me` as home.
    fn draw(draft: &Draft) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, 48, 7));
        render(&orb(), draft, "/Users/me".as_ref(), buf.area, &mut buf);
        buf
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

    fn line(buf: &Buffer, y: usize) -> String {
        lines(buf).swap_remove(y)
    }

    /// The foreground of the base branch's value.
    fn branch_colour(buf: &Buffer) -> Option<Color> {
        buf.cell((13, 2)).map(|cell| cell.fg)
    }

    #[rstest::rstest]
    fn header_names_the_project() {
        // Given a local draft in orb.
        let draft = draft(DraftWorkspace::Local);

        // When drawing its form.
        let header = line(&draw(&draft), 0);

        // Then the header says it's a new thread in orb.
        assert_eq!(header.trim_end(), "New thread · OB orb", "the header");
    }

    #[rstest::rstest]
    #[case(1, "Workspace    Local checkout", "␣w")]
    #[case(2, "Base branch  dev", "␣b")]
    #[case(3, "Model        Default", "␣m")]
    #[case(4, "Permission   plan", "␣a")]
    fn field_row_shows_its_value_and_key(#[case] y: usize, #[case] field: &str, #[case] key: &str) {
        // Given a local draft on dev with the default model in plan mode.
        let draft = draft(DraftWorkspace::Local);

        // When drawing its form.
        let row = line(&draw(&draft), y);

        // Then the row shows the field's value, and its key at the right edge.
        assert!(
            row.starts_with(field) && row.trim_end().ends_with(key),
            "row was '{row}'"
        );
    }

    #[rstest::rstest]
    fn existing_worktree_shows_its_path() {
        // Given a draft in an existing worktree under home.
        let draft = draft(DraftWorkspace::Existing(
            "/Users/me/.orb/worktrees/orb/orb-1a2b".into(),
        ));

        // When drawing its form.
        let row = line(&draw(&draft), 1);

        // Then the workspace row shows the worktree's path from home.
        assert!(
            row.starts_with("Workspace    ~/.orb/worktrees/orb/orb-1a2b "),
            "row was '{row}'"
        );
    }

    #[rstest::rstest]
    fn long_worktree_path_is_cut_from_the_left() {
        // Given a draft in a worktree whose path is wider than the row.
        let draft = draft(DraftWorkspace::Existing(
            "/Users/me/.orb/worktrees/a-very-long-repository/orb-1a2b3c4d".into(),
        ));

        // When drawing its form.
        let row = line(&draw(&draft), 1);

        // Then the path's start gives way to `…`, keeping its end and the key.
        assert!(
            row.starts_with("Workspace    …") && row.trim_end().ends_with("orb-1a2b3c4d ␣w"),
            "row was '{row}'"
        );
    }

    #[rstest::rstest]
    fn existing_worktree_dims_the_base_branch() {
        // Given a draft in an existing worktree.
        let draft = draft(DraftWorkspace::Existing(
            "/Users/me/.orb/worktrees/orb/orb-1a2b".into(),
        ));

        // When drawing its form.
        let buf = draw(&draft);

        // Then the base branch's value is dimmed.
        assert_eq!(branch_colour(&buf), Some(DARK_GRAY), "the branch colour");
    }

    #[rstest::rstest]
    fn local_draft_base_branch_is_not_dimmed() {
        // Given a local draft.
        let draft = draft(DraftWorkspace::Local);

        // When drawing its form.
        let buf = draw(&draft);

        // Then the base branch's value is drawn in white.
        assert_eq!(branch_colour(&buf), Some(Color::White), "the branch colour");
    }

    #[rstest::rstest]
    fn unknown_branch_says_so() {
        // Given a draft whose branch git couldn't tell.
        let draft = Draft {
            branch: None,
            ..draft(DraftWorkspace::Local)
        };

        // When drawing its form.
        let row = line(&draw(&draft), 2);

        // Then the base branch is unknown.
        assert!(row.starts_with("Base branch  unknown"), "row was '{row}'");
    }

    #[rstest::rstest]
    fn form_ends_with_how_to_start() {
        // Given a local draft.
        let draft = draft(DraftWorkspace::Local);

        // When drawing its form.
        let buf = draw(&draft);

        // Then a blank row, then the start key, close the form.
        assert_eq!(
            [
                line(&buf, 5).trim_end().to_owned(),
                line(&buf, 6).trim_end().to_owned()
            ],
            [String::new(), "⏎ start".to_owned()],
            "the form's last rows"
        );
    }
}
