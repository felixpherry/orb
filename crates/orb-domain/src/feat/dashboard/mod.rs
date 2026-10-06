//! Dashboard — the start screen on the right while no session pane is shown.
//!
//! Its menu lists what the sidebar's selection can do: open a thread or start
//! a draft, change its workspace, branch and (for a draft) harness, model and
//! permission (only when the harness has permission modes), open a tool in
//! its directory, and the always-available new session, incognito, add
//! project, filter projects and quit. On a group's rows, the menu has no
//! workspace or branch: the group owns its directory. Nor on orb's Incognito
//! draft or its threads. A group's card and draft pick the group's default
//! harness, model and permission. Each item has a single key that runs it;
//! a cursor moves over the items and `⏎` runs the highlighted one. The cursor
//! goes back to the first item whenever the selection changes.

pub mod state;

use crate::Intent;
use crate::feat::sessions::state::{GroupKind, ProjectKind, Sessions};
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
    /// Pick the selected draft's harness.
    Harness,
    Model,
    Permission,
    NewSession,
    /// Open orb's Incognito project's draft.
    Incognito,
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
            Self::Harness => 'h',
            Self::Model => 'm',
            Self::Permission => 'a',
            Self::NewSession => 'n',
            Self::Incognito => 'i',
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
            Self::Harness => Intent::PickHarness,
            Self::Model => Intent::PickModel,
            Self::Permission => Intent::PickPermission,
            Self::NewSession => Intent::NewSession,
            Self::Incognito => Intent::NewIncognito,
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
/// always there. `permissions` is whether the selection's harness lists
/// permission modes; without them there is no Permission item.
pub fn items(sessions: &Sessions, permissions: bool) -> Vec<DashboardItem> {
    use DashboardItem::{
        AddProject, Branch, FilterProjects, Harness, Incognito, Lazygit, Model, Neovim, NewSession,
        Open, Permission, Quit, Shell, Start, Workspace,
    };
    let (own, tools): (&[DashboardItem], bool) = match (
        sessions.selected_draft(),
        sessions.selected_thread(),
        sessions.selected_group(),
    ) {
        (Some((project, draft)), _, _) if draft.repo && project.kind != ProjectKind::Incognito => (
            &[Start, Workspace, Branch, Harness, Model, Permission],
            true,
        ),
        (Some(_), _, _) => (&[Start, Harness, Model, Permission], true),
        (None, Some(_), Some(_)) => (&[Open], true),
        (None, Some(_), None)
            if sessions
                .selected_project()
                .is_some_and(|project| project.kind == ProjectKind::Incognito) =>
        {
            (&[Open], true)
        }
        (None, Some(_), None) => (&[Open, Workspace, Branch], true),
        (None, None, Some(_)) if sessions.selected_group_draft().is_some() => {
            (&[Start, Harness, Model, Permission], true)
        }
        (None, None, Some((_, group)))
            if group.kind == GroupKind::Feature && group.dir.is_some() =>
        {
            (&[Branch, Harness, Model, Permission], true)
        }
        (None, None, Some(_)) => (&[Harness, Model, Permission], true),
        (None, None, None) => (&[], false),
    };
    let tools: &[DashboardItem] = if tools {
        &[Shell, Lazygit, Neovim]
    } else {
        &[]
    };
    own.iter()
        .filter(|item| permissions || **item != Permission)
        .chain(&[NewSession, Incognito, AddProject, FilterProjects])
        .chain(tools)
        .chain(&[Quit])
        .copied()
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::feat::harness::HarnessId;
    use std::time::SystemTime;

    use super::DashboardItem::{
        AddProject, Branch, FilterProjects, Harness, Incognito, Lazygit, Model, Neovim, NewSession,
        Open, Permission, Quit, Shell, Start, Workspace,
    };
    use super::items;
    use crate::Intent;
    use crate::feat::sessions::state::{
        Draft, DraftWorkspace, Group, GroupDefaults, GroupDraft, GroupId, GroupKind, Project,
        ProjectId, ProjectKind, Sessions, SidebarItem, Thread, ThreadId, ThreadStatus,
    };

    /// Project 1 holding thread 1 and, when `repo` is given, a local draft in
    /// a git repository or not; the cursor on `cursor`.
    pub(crate) fn sessions(repo: Option<bool>, cursor: Option<SidebarItem>) -> Sessions {
        let thread = Thread {
            harness: HarnessId::new("claude"),
            id: ThreadId(1),
            title: None,
            cwd: "/work".into(),
            transcript: None,
            status: ThreadStatus::Idle,
            turn_started_at: None,
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
        };
        let draft = repo.map(|repo| Draft {
            harness: HarnessId::new("claude"),
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
                groups: vec![],
                kind: ProjectKind::Normal,
            }],
            cursor,
            ..Sessions::default()
        }
    }

    /// `sessions()` with thread 1 in Feature group 9, which has a draft when
    /// `draft`; the cursor on `cursor`.
    fn grouped(cursor: SidebarItem, draft: bool) -> Sessions {
        let mut sessions = sessions(None, Some(cursor));
        if let Some(project) = sessions.projects.first_mut() {
            project
                .threads
                .iter_mut()
                .for_each(|thread| thread.group = Some(GroupId(9)));
            project.groups = vec![Group {
                id: GroupId(9),
                kind: GroupKind::Feature,
                name: "GT-514-login".to_owned(),
                dir: None,
                branch: Some("GT-514-login".to_owned()),
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
            }];
        }
        sessions
    }

    /// `sessions(repo, Some(cursor))` with project 1 as orb's Incognito project.
    fn incognito(repo: Option<bool>, cursor: SidebarItem) -> Sessions {
        let mut sessions = sessions(repo, Some(cursor));
        sessions
            .projects
            .iter_mut()
            .for_each(|project| project.kind = ProjectKind::Incognito);
        sessions
    }

    #[rstest::rstest]
    fn incognito_thread_lists_open_without_workspace_or_branch() {
        // Given a selected thread of the Incognito project, outside a group.
        let sessions = incognito(None, SidebarItem::Thread(ThreadId(1)));

        // When listing the dashboard's items.
        let items = items(&sessions, true);

        // Then Open leads, with no Workspace or Branch.
        assert_eq!(
            items,
            [
                Open,
                NewSession,
                Incognito,
                AddProject,
                FilterProjects,
                Shell,
                Lazygit,
                Neovim,
                Quit
            ],
            "an incognito thread's dashboard items"
        );
    }

    #[rstest::rstest]
    fn incognito_draft_lists_no_workspace_or_branch_even_in_a_repo() {
        // Given the Incognito project's draft, in a git repository.
        let sessions = incognito(Some(true), SidebarItem::Draft(ProjectId(1)));

        // When listing the dashboard's items.
        let items = items(&sessions, true);

        // Then Start, Harness, Model and Permission lead, with no Workspace or Branch.
        assert_eq!(
            items,
            [
                Start,
                Harness,
                Model,
                Permission,
                NewSession,
                Incognito,
                AddProject,
                FilterProjects,
                Shell,
                Lazygit,
                Neovim,
                Quit
            ],
            "the incognito draft's dashboard items"
        );
    }

    #[rstest::rstest]
    fn grouped_thread_lists_open_without_workspace_or_branch() {
        // Given a selected thread in a group.
        let sessions = grouped(SidebarItem::Thread(ThreadId(1)), false);

        // When listing the dashboard's items.
        let items = items(&sessions, true);

        // Then Open leads, with no Workspace or Branch.
        assert_eq!(
            items,
            [
                Open,
                NewSession,
                Incognito,
                AddProject,
                FilterProjects,
                Shell,
                Lazygit,
                Neovim,
                Quit
            ],
            "a grouped thread's dashboard items"
        );
    }

    #[rstest::rstest]
    fn group_draft_lists_start_model_and_permission() {
        // Given a selected group draft.
        let sessions = grouped(SidebarItem::GroupDraft(GroupId(9)), true);

        // When listing the dashboard's items.
        let items = items(&sessions, true);

        // Then Start, Harness, Model and Permission lead.
        assert_eq!(
            items,
            [
                Start,
                Harness,
                Model,
                Permission,
                NewSession,
                Incognito,
                AddProject,
                FilterProjects,
                Shell,
                Lazygit,
                Neovim,
                Quit
            ],
            "a group draft's dashboard items"
        );
    }

    #[rstest::rstest]
    fn feature_card_with_a_worktree_lists_branch_first() {
        // Given a selected card of a Feature group whose worktree exists.
        let sessions = {
            let mut sessions = grouped(SidebarItem::Group(GroupId(9)), false);
            if let Some(group) = sessions
                .projects
                .first_mut()
                .and_then(|project| project.groups.first_mut())
            {
                group.dir = Some("/wt/orb-1a2b3c4d".into());
            }
            sessions
        };

        // When listing the dashboard's items.
        let items = items(&sessions, true);

        // Then Branch, Harness, Model and Permission lead the general items and the
        // tools.
        assert_eq!(
            items,
            [
                Branch,
                Harness,
                Model,
                Permission,
                NewSession,
                Incognito,
                AddProject,
                FilterProjects,
                Shell,
                Lazygit,
                Neovim,
                Quit
            ],
            "a started Feature card's dashboard items"
        );
    }

    #[rstest::rstest]
    fn group_card_lists_its_defaults_the_general_items_and_tools() {
        // Given a selected card of a group with no worktree yet.
        let sessions = grouped(SidebarItem::Group(GroupId(9)), false);

        // When listing the dashboard's items.
        let items = items(&sessions, true);

        // Then Harness, Model and Permission, the general items, the tools and Quit
        // are listed.
        assert_eq!(
            items,
            [
                Harness,
                Model,
                Permission,
                NewSession,
                Incognito,
                AddProject,
                FilterProjects,
                Shell,
                Lazygit,
                Neovim,
                Quit
            ],
            "a group card's dashboard items"
        );
    }

    #[rstest::rstest]
    fn thread_lists_open_its_setup_the_general_items_and_tools() {
        // Given a selected thread.
        let sessions = sessions(None, Some(SidebarItem::Thread(ThreadId(1))));

        // When listing the dashboard's items.
        let items = items(&sessions, true);

        // Then they are the thread's, the general ones, the tools and Quit.
        assert_eq!(
            items,
            [
                Open,
                Workspace,
                Branch,
                NewSession,
                Incognito,
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
        let items = items(&sessions, true);

        // Then they start with Start, Workspace, Branch, Harness, Model and Permission.
        assert_eq!(
            items,
            [
                Start,
                Workspace,
                Branch,
                Harness,
                Model,
                Permission,
                NewSession,
                Incognito,
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
        let items = items(&sessions, true);

        // Then Workspace and Branch are missing.
        assert_eq!(
            items,
            [
                Start,
                Harness,
                Model,
                Permission,
                NewSession,
                Incognito,
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
    fn harness_comes_right_before_model_on_a_draft() {
        // Given a selected draft in a git repository.
        let sessions = sessions(Some(true), Some(SidebarItem::Draft(ProjectId(1))));

        // When listing the dashboard's items.
        let items = items(&sessions, true);

        // Then Harness is the item just before Model.
        let model = items.iter().position(|item| *item == Model);
        assert_eq!(
            model
                .and_then(|at| at.checked_sub(1))
                .and_then(|at| items.get(at)),
            Some(&Harness),
            "Harness should come right before Model"
        );
    }

    #[rstest::rstest]
    fn draft_of_a_harness_without_permission_modes_lists_no_permission() {
        // Given a selected draft whose harness lists no permission modes.
        let sessions = sessions(Some(true), Some(SidebarItem::Draft(ProjectId(1))));

        // When listing the dashboard's items without permission modes.
        let items = items(&sessions, false);

        // Then the settings stop at Model, with no Permission.
        assert_eq!(
            items,
            [
                Start,
                Workspace,
                Branch,
                Harness,
                Model,
                NewSession,
                Incognito,
                AddProject,
                FilterProjects,
                Shell,
                Lazygit,
                Neovim,
                Quit
            ],
            "a draft without permission modes should have no Permission item"
        );
    }

    #[rstest::rstest]
    #[case::nothing(None)]
    #[case::shelf_header(Some(SidebarItem::SettledShelf))]
    fn no_thread_or_draft_lists_only_the_general_items(#[case] cursor: Option<SidebarItem>) {
        // Given no thread or draft selected.
        let sessions = sessions(None, cursor);

        // When listing the dashboard's items.
        let items = items(&sessions, true);

        // Then only the general items and Quit are listed.
        assert_eq!(
            items,
            [NewSession, Incognito, AddProject, FilterProjects, Quit],
            "the dashboard's items with {cursor:?}"
        );
    }

    #[rstest::rstest]
    fn incognito_item_runs_new_incognito_on_i() {
        // Given the dashboard's Incognito item.
        // When reading its key and intent.
        let bound = (Incognito.key(), Incognito.intent());

        // Then `i` opens the incognito draft.
        assert_eq!(
            bound,
            ('i', Intent::NewIncognito),
            "the Incognito item should run NewIncognito on i"
        );
    }
}
