//! Commands: work the [`IntentHandler`](crate::IntentHandler) asks for after it
//! has updated [`AppState`](crate::AppState). Pane commands, `Yank`,
//! `ListDirectories`, `ListBranches` and `OpenTool` are carried out by the
//! frontend loop; session commands (drafts, add project, move to another
//! workspace, switch branch, refresh, pin, settle, delete, visit) go to the
//! sessions actor; `ShowPreview` goes to the preview actor.

use std::path::PathBuf;

use crate::feat::git::git_service::GitRef;
use crate::feat::sessions::state::{AttachTarget, ProjectId, ThreadId};
use crate::feat::zellij::zellij_service::Tool;

/// Something that must happen in response to an intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Show the target's session in the terminal pane and send it input.
    Attach(AttachTarget),
    /// Stop sending input to the terminal pane.
    Detach,
    /// Give the project a draft, prefilled from its last-used settings,
    /// unless it has one.
    CreateDraft(ProjectId),
    /// Save the project's draft as it now is in the app state.
    SaveDraft(ProjectId),
    /// Check the branch out in `cwd` for the project's draft: the draft's
    /// directory, or the project's root, which the draft then moves to.
    CheckoutDraft {
        project: ProjectId,
        git_ref: GitRef,
        cwd: PathBuf,
    },
    /// Make the project's root a git repository (`git init`) for its draft.
    InitGit(ProjectId),
    /// Start a Claude session from the project's draft, which becomes a
    /// thread.
    StartDraft(ProjectId),
    /// Throw the project's draft away.
    DiscardDraft(ProjectId),
    /// Start the thread's session over in another workspace, before its
    /// first prompt.
    MoveThread { thread: ThreadId, to: Workspace },
    /// Check the branch out in the thread's directory. With `to_root`, check
    /// it out in the project's root instead and move the prompt-less thread
    /// there.
    SwitchBranch {
        thread: ThreadId,
        git_ref: GitRef,
        to_root: bool,
    },
    /// List the refs of the directory's repository into the open branch picker.
    ListBranches(PathBuf),
    /// Focus the tool's zellij pane for the directory, else open one.
    OpenTool { tool: Tool, cwd: PathBuf },
    /// Add the directory as a project.
    AddProject(PathBuf),
    /// List the directory's subdirectories into the open directory picker.
    ListDirectories(PathBuf),
    /// Poll the sessions' statuses now instead of waiting for the next tick.
    RefreshSessions,
    /// Show the selected thread's transcript now instead of waiting for the
    /// next check.
    ShowPreview,
    /// Copy text to the clipboard.
    Yank(String),
    /// Pin the thread to the top of the sidebar.
    Pin(ThreadId),
    /// Unpin the thread.
    Unpin(ThreadId),
    /// Move the thread to the Settled shelf and stop its session.
    Settle(ThreadId),
    /// Bring the thread back from the Settled shelf and keep it active.
    Unsettle(ThreadId),
    /// Delete the thread and its Claude session.
    Delete(ThreadId),
    /// The user is looking at the thread now.
    Visit(ThreadId),
}

/// Where a thread's session runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Workspace {
    /// A new worktree of the project, on a new branch.
    NewWorktree,
    /// A directory that already exists: a worktree or the project's root.
    Existing(PathBuf),
}
