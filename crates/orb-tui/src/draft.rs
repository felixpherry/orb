//! The draft form: what the right side shows while a draft is selected.
//!
//! It names the project and lists the four settings the draft's session will
//! start with — workspace, base branch, model (by name) and permission — then
//! how to start. An existing worktree shows its path, cut from the left when
//! it's too long; its branch is its own and can't be picked, so that row is
//! dimmed.

use std::path::Path;

use orb_domain::feat::picker::list::setting_label;
use orb_domain::feat::sessions::state::{Draft, DraftWorkspace, Project};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::picker::{cut_left, tilde};
use crate::sidebar::{DARK_GRAY, GRAY, badge};

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
    render_field("Workspace", &place, false, workspace, buf);
    render_field(
        "Base branch",
        draft.branch.as_deref().unwrap_or("unknown"),
        read_only,
        branch,
        buf,
    );
    render_field(
        "Model",
        setting_label(draft.model.as_deref()),
        false,
        model,
        buf,
    );
    render_field(
        "Permission",
        setting_label(draft.permission.as_deref()),
        false,
        permission,
        buf,
    );
    Line::raw("⏎ start").render(start, buf);
}

/// Where a draft's session will run, in a word or two.
fn workspace_label(workspace: &DraftWorkspace) -> &'static str {
    match workspace {
        DraftWorkspace::Local => "Local checkout",
        DraftWorkspace::NewWorktree => "New worktree",
        DraftWorkspace::Existing(_) => "Worktree",
    }
}

/// The column a field's value starts at.
const VALUE_X: usize = 13;

/// A setting's label and value, both dimmed when it can't be picked. A value
/// too long for the row is cut from the left.
fn render_field(label: &str, value: &str, dim: bool, area: Rect, buf: &mut Buffer) {
    let (label_colour, value_colour) = if dim {
        (DARK_GRAY, DARK_GRAY)
    } else {
        (GRAY, Color::White)
    };
    let room = usize::from(area.width).saturating_sub(VALUE_X);
    Line::from(vec![
        Span::styled(format!("{label:<VALUE_X$}"), Style::new().fg(label_colour)),
        Span::styled(cut_left(value, room), Style::new().fg(value_colour)),
    ])
    .render(area, buf);
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
    #[case(1, "Workspace    Local checkout")]
    #[case(2, "Base branch  dev")]
    #[case(3, "Model        Default")]
    #[case(4, "Permission   plan")]
    fn field_row_shows_only_its_label_and_value(#[case] y: usize, #[case] field: &str) {
        // Given a local draft on dev with the default model in plan mode.
        let draft = draft(DraftWorkspace::Local);

        // When drawing its form.
        let row = line(&draw(&draft), y);

        // Then the row is the field's label and value, with no key after it.
        assert_eq!(row.trim_end(), field, "the field row");
    }

    #[rstest::rstest]
    #[case("claude-opus-5-5", "Model        Claude Opus 5.5")]
    #[case("opus", "Model        opus")]
    fn model_row_names_a_known_model_and_shows_others_as_stored(
        #[case] model: &str,
        #[case] expected: &str,
    ) {
        // Given a draft on `model`.
        let draft = Draft {
            model: Some(model.to_owned()),
            ..draft(DraftWorkspace::Local)
        };

        // When drawing its form.
        let row = line(&draw(&draft), 3);

        // Then the model row shows the model's name, or the stored value.
        assert_eq!(row.trim_end(), expected, "the model row");
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

        // Then the path's start gives way to `…`, keeping its end.
        assert!(
            row.starts_with("Workspace    …") && row.ends_with("/orb-1a2b3c4d"),
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
