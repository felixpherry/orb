//! Commands: work the [`IntentHandler`](crate::IntentHandler) asks for after it
//! has updated [`AppState`](crate::AppState). Pane commands (`Attach`,
//! `Detach`, `SendCtrlG`),
//! `ListDirectories`, `ListBranches`, `LoadPreview` and `OpenTool` are carried
//! out by the frontend loop; session commands (new sessions, add or remove a
//! project, move a session to another workspace, switch a session's branch,
//! refresh, pin, rename, settle, delete and visit a session, save the
//! sidebar's width and filter, save the jump list, split a pane, open a tab,
//! save a layout) go to the sessions actor; worktree commands (refresh,
//! delete) go to the worktrees actor; search commands (query, preview) go to
//! the search actor.

use std::path::PathBuf;

use crate::feat::git::git_service::GitRef;
use crate::feat::layout::tree::Split;
use crate::feat::sessions::state::{FolderKind, ProjectId, SessionId, ThreadId};
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
    /// Write Ctrl g to the focused pane.
    SendCtrlG,
    /// Split `session`'s focused pane `split`; the sessions actor makes the pane.
    SplitPane { session: SessionId, split: Split },
    /// Open a tab of one new pane in `session`.
    NewTab(SessionId),
    /// Save `session`'s tabs, splits, focus and pane names as the app state
    /// has them.
    SaveLayout(SessionId),
    /// Make a session of `project` in `workspace`, with one shell.
    NewSession {
        project: ProjectId,
        workspace: Workspace,
    },
    /// Make the project's root a git repository (`git init`).
    InitGit(ProjectId),
    /// Move `session`, before its first agent turn, to `to`, restarting its
    /// panes there.
    ChangeWorkspace { session: SessionId, to: Workspace },
    /// Check `git_ref` out in `session`'s directory; with `to_root`, check it
    /// out in the project's checkout and move the session there.
    SwitchBranch {
        session: SessionId,
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
    /// Remove the project from `<C-g> n` and the project filter; its sessions
    /// stay.
    RemoveProject(ProjectId),
    /// Make a `kind` session in orb's own folder `name` (a slug), with one
    /// shell.
    NewFolderSession { kind: FolderKind, name: String },
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

/// Where a new session runs, or where a session moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Workspace {
    /// The project's checkout (its root).
    Checkout,
    /// A new worktree orb makes from `base`, a branch or remote ref name.
    NewWorktree { base: String },
    /// A worktree that already exists.
    Existing(PathBuf),
}
