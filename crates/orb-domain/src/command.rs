//! Commands: work the [`IntentHandler`](crate::IntentHandler) asks for after it
//! has updated [`AppState`](crate::AppState). Pane commands,
//! `ListDirectories`, `ListBranches`, `LoadPreview` and `OpenTool` are carried
//! out by the frontend loop; session commands (drafts, add or remove a project, move to
//! another workspace, switch a thread's or a group's branch, refresh, pin, rename, settle,
//! delete and visit a session, save the sidebar's width and filter, save the jump list,
//! create and start groups, save a group draft, answer a trust confirm,
//! split a pane, open a tab, save a layout) go to the sessions actor; worktree commands (refresh, delete) go to the worktrees actor; search
//! commands (query, preview) go to the search actor.

use std::path::PathBuf;

use crate::feat::git::git_service::GitRef;
use crate::feat::layout::tree::Split;
use crate::feat::sessions::state::{GroupId, GroupKind, ProjectId, SessionId, ThreadId};
use crate::feat::zellij::zellij_service::Tool;

/// Something that must happen in response to an intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Give every pane of the session a client and show it, with the keys in
    /// its focused pane. If its orb worktree is gone, the sessions actor
    /// recreates it first and the frontend attaches once it's back.
    Attach(SessionId),
    /// Stop sending input to the terminal pane.
    Detach,
    /// Split `session`'s focused pane `split`; the sessions actor makes the pane.
    SplitPane { session: SessionId, split: Split },
    /// Open a tab of one new pane in `session`.
    NewTab(SessionId),
    /// Save `session`'s tabs, splits, focus and pane names as the app state
    /// has them.
    SaveLayout(SessionId),
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
    /// Pin the session to the top of the sidebar; a settled session
    /// un-settles.
    PinSession(SessionId),
    /// Unpin the session.
    UnpinSession(SessionId),
    /// Give the session a name, or with `None` go back to its agents' or
    /// directory's.
    RenameSession {
        session: SessionId,
        name: Option<String>,
    },
    /// Move the session to the Settled shelf.
    SettleSession(SessionId),
    /// Bring the session back from the Settled shelf and keep it active.
    UnsettleSession(SessionId),
    /// Delete the session: its panes, tabs and threads.
    DeleteSession(SessionId),
    /// The user is looking at the session now.
    Visit(SessionId),
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
    /// Save the group's default setup and draft as they now are in the app state.
    SaveGroupDraft(GroupId),
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
