//! Commands: work the [`IntentHandler`](crate::IntentHandler) asks for after it
//! has updated [`AppState`](crate::AppState). Pane commands,
//! `ListDirectories`, `ListBranches`, `LoadPreview` and `OpenTool` are carried
//! out by the frontend loop; session commands (drafts, add or remove a project, move to
//! another workspace, switch a thread's or a group's branch, refresh, pin, rename, settle, delete,
//! visit, save the sidebar's width and filter, save the jump list, create and start groups, start a
//! group's sibling, pin, settle and delete groups, save a group draft, answer a trust confirm) go
//! to the sessions actor; worktree commands (refresh, delete) go to the worktrees actor; search
//! commands (query, preview) go to the search actor.

use std::path::PathBuf;

use crate::feat::git::git_service::GitRef;
use crate::feat::sessions::state::{
    AttachTarget, GroupId, GroupKind, ProjectId, SidebarItem, ThreadId,
};
use crate::feat::zellij::zellij_service::Tool;

/// Something that must happen in response to an intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Show the target's session in the terminal pane and send it input. If
    /// its orb worktree is gone, the sessions actor recreates it first and the
    /// frontend attaches once it's back.
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
    /// Start a session from the project's draft, which becomes a
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
    /// Check the branch out in the group's worktree, which moves every thread
    /// in it.
    CheckoutGroup { group: GroupId, git_ref: GitRef },
    /// List the refs of the directory's repository into the open branch picker.
    ListBranches(PathBuf),
    /// Focus the tool's zellij pane for the directory, else open one.
    OpenTool { tool: Tool, cwd: PathBuf },
    /// Add the directory as a project.
    AddProject(PathBuf),
    /// List the directory's subdirectories into the open directory picker.
    ListDirectories(PathBuf),
    /// Read the thread's transcript into the open session picker's preview.
    LoadPreview {
        thread: ThreadId,
        transcript: PathBuf,
    },
    /// Poll the sessions' statuses now instead of waiting for the next tick.
    RefreshSessions,
    /// Pin the thread to the top of the sidebar.
    Pin(ThreadId),
    /// Unpin the thread.
    Unpin(ThreadId),
    /// Give the thread orb's own name, or with `None` go back to the
    /// harness's title.
    RenameThread {
        thread: ThreadId,
        title: Option<String>,
    },
    /// Move the thread to the Settled shelf and stop its session.
    Settle(ThreadId),
    /// Bring the thread back from the Settled shelf and keep it active.
    Unsettle(ThreadId),
    /// Delete the thread and its session.
    Delete(ThreadId),
    /// The user is looking at the thread now.
    Visit(ThreadId),
    /// Save the sidebar's width and project filter as they now are in the
    /// app state.
    SaveUi,
    /// Save the jump list as it now is in the app state.
    SaveJumps,
    /// Remove the project from `␣n` and the project filter and discard its
    /// draft; its threads stay.
    RemoveProject(ProjectId),
    /// Create a `kind` group named `name` (a slug) in `project`, or for
    /// Research/Learn in orb's own project for the kind, with a draft.
    CreateGroup {
        kind: GroupKind,
        project: Option<ProjectId>,
        name: String,
    },
    /// Start the group's first thread from its draft.
    StartGroupDraft(GroupId),
    /// Save the group's default model and permission (its draft's and each
    /// new sibling's) as they now are in the app state.
    SaveGroupDraft(GroupId),
    /// Start a thread at the top of `group`, in its directory, with `model`
    /// and `permission_mode` (the group's defaults); select and attach it if
    /// the cursor is still on `from`.
    StartSibling {
        group: GroupId,
        model: Option<String>,
        permission_mode: Option<String>,
        from: Option<SidebarItem>,
    },
    /// Pin the group to the top of the sidebar; a settled group un-settles.
    PinGroup(GroupId),
    /// Unpin the group.
    UnpinGroup(GroupId),
    /// Move the group to the Settled shelf and stop its idle sessions.
    SettleGroup(GroupId),
    /// Bring the group back from the Settled shelf and keep it active.
    UnsettleGroup(GroupId),
    /// Delete every thread of the group and its session, then the
    /// group and its folder or worktree; a Feature group whose slug branch
    /// isn't merged is kept whole.
    DeleteGroup(GroupId),
    /// Mark the folder the waiting session start asks about trusted in
    /// its harness's config, and try the start again.
    TrustWorkspace,
    /// End the waiting session start as a failed one: the user didn't trust
    /// its folder.
    DeclineTrust,
    /// Rescan orb's worktrees and re-read their facts, then their sizes.
    RefreshWorktrees,
    /// Force-remove the worktree at `path`, keeping its branch.
    DeleteWorktree { path: PathBuf },
    /// Bring the search index up to date and run `query` in it, listing the
    /// rows in the open search picker while `query` is still its typed text.
    SearchTranscripts { query: String },
    /// Load the exchange of search hit `hit`, the prompt at `prompt_offset` in
    /// the transcript at `path` and its replies, into the search picker's
    /// preview.
    LoadSearchPreview {
        hit: i64,
        path: PathBuf,
        prompt_offset: u64,
    },
}

/// Where a thread's session runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Workspace {
    /// A new worktree of the project, on a new branch.
    NewWorktree,
    /// A directory that already exists: a worktree or the project's root.
    Existing(PathBuf),
}
