//! Dashboard — the start screen on the right while no Claude pane is shown.
//!
//! Its menu lists what the sidebar's selection can do: open a thread or start
//! a draft, change its workspace, branch and (for a draft) model and
//! permission, open a tool in its directory, and the always-available new
//! session, add project, filter projects and quit. Each item has a single key
//! that runs it; a cursor moves over the items and `⏎` runs the highlighted
//! one. The cursor goes back to the first item whenever the selection changes.

pub mod state;

use crate::Intent;
use crate::feat::sessions::state::Sessions;
use crate::feat::zellij::zellij_service::Tool;

/// One dashboard menu entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardItem {
    /// Attach to the selected thread.
    Open,
    /// Start the selected draft.
    Start,
    Workspace,
    Branch,
    Model,
    Permission,
    NewSession,
    AddProject,
    FilterProjects,
    Shell,
    Lazygit,
    Neovim,
    Quit,
}

impl DashboardItem {
    /// The key that runs the item on the dashboard.
    pub fn key(self) -> char {
        match self {
            Self::Open | Self::Start => 'o',
            Self::Workspace => 'w',
            Self::Branch => 'b',
            Self::Model => 'm',
            Self::Permission => 'a',
            Self::NewSession => 'n',
            Self::AddProject => 'p',
            Self::FilterProjects => 'f',
            Self::Shell => 't',
            Self::Lazygit => 'g',
            Self::Neovim => 'v',
            Self::Quit => 'q',
        }
    }

    /// What running the item does.
    pub fn intent(self) -> Intent {
        match self {
            Self::Open | Self::Start => Intent::Attach,
            Self::Workspace => Intent::ChangeWorkspace,
            Self::Branch => Intent::SwitchBranch,
            Self::Model => Intent::PickModel,
            Self::Permission => Intent::PickPermission,
            Self::NewSession => Intent::NewSession,
            Self::AddProject => Intent::AddProject,
            Self::FilterProjects => Intent::FilterProjects,
            Self::Shell => Intent::OpenTool(Tool::Shell),
            Self::Lazygit => Intent::OpenTool(Tool::Lazygit),
            Self::Neovim => Intent::OpenTool(Tool::Nvim),
            Self::Quit => Intent::Quit,
        }
    }
}

/// The items the selection supports, in menu order. Never empty: Quit is
/// always there.
pub fn items(sessions: &Sessions) -> Vec<DashboardItem> {
    use DashboardItem::{
        AddProject, Branch, FilterProjects, Lazygit, Model, Neovim, NewSession, Open, Permission,
        Quit, Shell, Start, Workspace,
    };
    let own: &[DashboardItem] = match (sessions.selected_draft(), sessions.selected_thread()) {
        (Some((_, draft)), _) if draft.repo => &[Start, Workspace, Branch, Model, Permission],
        (Some(_), _) => &[Start, Model, Permission],
        (None, Some(_)) => &[Open, Workspace, Branch],
        (None, None) => &[],
    };
    let tools: &[DashboardItem] = match own {
        [] => &[],
        _ => &[Shell, Lazygit, Neovim],
    };
    own.iter()
        .chain(&[NewSession, AddProject, FilterProjects])
        .chain(tools)
        .chain(&[Quit])
        .copied()
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use std::time::SystemTime;

    use super::DashboardItem::{
        AddProject, Branch, FilterProjects, Lazygit, Model, Neovim, NewSession, Open, Permission,
        Quit, Shell, Start, Workspace,
    };
    use super::items;
    use crate::feat::sessions::state::{
        Draft, DraftWorkspace, Project, ProjectId, Sessions, SidebarItem, Thread, ThreadId,
        ThreadStatus,
    };

    /// Project 1 holding thread 1 and, when `repo` is given, a local draft in
    /// a git repository or not; the cursor on `cursor`.
    pub(crate) fn sessions(repo: Option<bool>, cursor: Option<SidebarItem>) -> Sessions {
        let thread = Thread {
            id: ThreadId(1),
            title: None,
            cwd: "/work".into(),
            transcript: None,
            status: ThreadStatus::Idle,
            turn_started_at: None,
            attach_argv: vec![],
            branch: None,
            pinned_at: None,
            settled_at: None,
            active_since: SystemTime::UNIX_EPOCH,
            last_activity_at: SystemTime::UNIX_EPOCH,
            unseen: false,
        };
        let draft = repo.map(|repo| Draft {
            workspace: DraftWorkspace::Local,
            branch: None,
            model: None,
            permission: None,
            created_at: SystemTime::UNIX_EPOCH,
            repo,
            from: None,
        });
        Sessions {
            projects: vec![Project {
                id: ProjectId(1),
                title: "work".into(),
                root: "/work".into(),
                created_at: SystemTime::UNIX_EPOCH,
                removed: false,
                draft,
                threads: vec![thread],
            }],
            cursor,
            ..Sessions::default()
        }
    }

    #[rstest::rstest]
    fn thread_lists_open_its_setup_the_general_items_and_tools() {
        // Given a selected thread.
        let sessions = sessions(None, Some(SidebarItem::Thread(ThreadId(1))));

        // When listing the dashboard's items.
        let items = items(&sessions);

        // Then they are the thread's, the general ones, the tools and Quit.
        assert_eq!(
            items,
            [
                Open,
                Workspace,
                Branch,
                NewSession,
                AddProject,
                FilterProjects,
                Shell,
                Lazygit,
                Neovim,
                Quit
            ],
            "a thread's dashboard items"
        );
    }

    #[rstest::rstest]
    fn git_draft_lists_start_and_all_its_settings() {
        // Given a selected draft in a git repository.
        let sessions = sessions(Some(true), Some(SidebarItem::Draft(ProjectId(1))));

        // When listing the dashboard's items.
        let items = items(&sessions);

        // Then they start with Start, Workspace, Branch, Model and Permission.
        assert_eq!(
            items,
            [
                Start,
                Workspace,
                Branch,
                Model,
                Permission,
                NewSession,
                AddProject,
                FilterProjects,
                Shell,
                Lazygit,
                Neovim,
                Quit
            ],
            "a git draft's dashboard items"
        );
    }

    #[rstest::rstest]
    fn non_git_draft_lists_no_workspace_or_branch() {
        // Given a selected draft outside a git repository.
        let sessions = sessions(Some(false), Some(SidebarItem::Draft(ProjectId(1))));

        // When listing the dashboard's items.
        let items = items(&sessions);

        // Then Workspace and Branch are missing.
        assert_eq!(
            items,
            [
                Start,
                Model,
                Permission,
                NewSession,
                AddProject,
                FilterProjects,
                Shell,
                Lazygit,
                Neovim,
                Quit
            ],
            "a non-git draft's dashboard items"
        );
    }

    #[rstest::rstest]
    #[case::nothing(None)]
    #[case::shelf_header(Some(SidebarItem::SettledShelf))]
    fn no_thread_or_draft_lists_only_the_general_items(#[case] cursor: Option<SidebarItem>) {
        // Given no thread or draft selected.
        let sessions = sessions(None, cursor);

        // When listing the dashboard's items.
        let items = items(&sessions);

        // Then only the general items and Quit are listed.
        assert_eq!(
            items,
            [NewSession, AddProject, FilterProjects, Quit],
            "the dashboard's items with {cursor:?}"
        );
    }
}
