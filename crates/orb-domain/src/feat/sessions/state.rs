//! The sidebar's contents: projects, their threads, and the selection.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::SystemTime;

/// Identifies a thread across launches (its row in orb's store).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThreadId(pub i64);

/// Identifies a project across launches (its row in orb's store).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectId(pub i64);

/// What a thread's Claude session is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThreadStatus {
    /// Not polled yet since orb started.
    Unknown,
    /// Claude is running a turn.
    Working,
    /// Claude waits for the user to approve something.
    NeedsApproval,
    /// Claude waits for the user to answer.
    NeedsInput,
    /// The session is idle, ready for a prompt.
    Idle,
    /// The session failed.
    Failed,
    /// The session stopped.
    Stopped,
    /// Claude no longer knows the session.
    Gone,
}

impl ThreadStatus {
    /// Whether a turn is underway: running, or paused waiting on the user.
    pub fn in_progress(self) -> bool {
        matches!(self, Self::Working | Self::NeedsApproval | Self::NeedsInput)
    }
}

/// One Claude session orb started.
#[derive(Debug, Clone)]
pub struct Thread {
    pub id: ThreadId,
    /// `None` until the transcript names the thread.
    pub title: Option<String>,
    /// Where the session runs.
    pub cwd: PathBuf,
    pub status: ThreadStatus,
    /// When orb first saw the current turn running; `None` between turns.
    pub turn_started_at: Option<SystemTime>,
    /// The command that attaches to the session.
    pub attach_argv: Vec<OsString>,
}

/// A directory orb started sessions in.
#[derive(Debug, Clone)]
pub struct Project {
    pub id: ProjectId,
    pub title: String,
    pub root: PathBuf,
    /// Newest first.
    pub threads: Vec<Thread>,
}

/// orb's projects and threads, and which thread is selected.
///
/// Written by the sessions actor (projects, `error`, `starting` when a create
/// ends, the selection after a create) and by the intent handler (the
/// selection on `j`/`k`, `starting` when a create begins).
#[derive(Debug, Clone, Default)]
pub struct Sessions {
    /// In the order orb first used them.
    pub projects: Vec<Project>,
    pub selected: Option<ThreadId>,
    /// A new session is being created.
    pub starting: bool,
    /// The latest `claude` failure; cleared by the next success.
    pub error: Option<String>,
}

impl Sessions {
    /// Every thread in sidebar order: project by project, newest first.
    pub fn threads(&self) -> impl Iterator<Item = &Thread> {
        self.projects
            .iter()
            .flat_map(|project| project.threads.iter())
    }

    /// The selected thread, if it still exists.
    pub fn selected_thread(&self) -> Option<&Thread> {
        self.threads()
            .find(|thread| Some(thread.id) == self.selected)
    }

    /// Select the thread below the selected one; stays put on the last thread.
    /// Without a selection, selects the first thread.
    pub fn select_next(&mut self) {
        let next = match self.selected_thread() {
            None => self.threads().next(),
            Some(current) => self
                .threads()
                .skip_while(|thread| thread.id != current.id)
                .nth(1),
        }
        .map(|thread| thread.id);
        if next.is_some() {
            self.selected = next;
        }
    }

    /// Select the thread above the selected one; stays put on the first thread.
    /// Without a selection, selects the first thread.
    pub fn select_prev(&mut self) {
        let prev = match self.selected_thread() {
            None => self.threads().next(),
            Some(current) => self
                .threads()
                .take_while(|thread| thread.id != current.id)
                .last(),
        }
        .map(|thread| thread.id);
        if prev.is_some() {
            self.selected = prev;
        }
    }

    /// How many threads are running a turn right now.
    pub fn working_count(&self) -> usize {
        self.threads()
            .filter(|thread| thread.status == ThreadStatus::Working)
            .count()
    }

    /// Whether any thread has a turn underway.
    pub fn any_in_progress(&self) -> bool {
        self.threads().any(|thread| thread.status.in_progress())
    }
}

/// What the frontend needs to attach to a thread's session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachTarget {
    pub thread: ThreadId,
    pub argv: Vec<OsString>,
    pub cwd: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::{Project, ProjectId, Sessions, Thread, ThreadId, ThreadStatus};

    fn thread(id: i64) -> Thread {
        Thread {
            id: ThreadId(id),
            title: None,
            cwd: "/tmp".into(),
            status: ThreadStatus::Idle,
            turn_started_at: None,
            attach_argv: vec![],
        }
    }

    fn project(id: i64, threads: Vec<Thread>) -> Project {
        Project {
            id: ProjectId(id),
            title: format!("project-{id}"),
            root: "/tmp".into(),
            threads,
        }
    }

    #[rstest::rstest]
    fn select_next_from_last_thread_of_a_project_selects_first_of_the_next() {
        // Given project A with threads 1, 2 and project B with thread 3, and thread 2 selected.
        let mut sessions = Sessions {
            projects: vec![
                project(1, vec![thread(1), thread(2)]),
                project(2, vec![thread(3)]),
            ],
            selected: Some(ThreadId(2)),
            ..Sessions::default()
        };

        // When selecting the next thread.
        sessions.select_next();

        // Then project B's first thread is selected.
        assert_eq!(
            sessions.selected,
            Some(ThreadId(3)),
            "next after A's last thread should be B's first"
        );
    }
}
