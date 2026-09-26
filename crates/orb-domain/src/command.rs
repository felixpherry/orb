//! Commands: work the [`IntentHandler`](crate::IntentHandler) asks for after it
//! has updated [`AppState`](crate::AppState). Pane commands, `Yank` and
//! `ListDirectories` are carried out by the frontend loop; session commands
//! (create, add project, move to another workspace, refresh, pin, settle,
//! delete, visit) go to the sessions actor; `ShowPreview` goes to the preview actor.

use std::path::PathBuf;

use crate::feat::sessions::state::{AttachTarget, ProjectId, ThreadId};

/// Something that must happen in response to an intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Show the target's session in the terminal pane and send it input.
    Attach(AttachTarget),
    /// Stop sending input to the terminal pane.
    Detach,
    /// Start a new Claude session in the project's directory.
    CreateSession { project: ProjectId, root: PathBuf },
    /// Start the thread's session over in another workspace, before its
    /// first prompt.
    MoveThread { thread: ThreadId, to: Workspace },
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
