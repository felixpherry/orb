//! The sessions actor — the owner of orb's threads, their statuses, and titles.
//!
//! At start it restores the saved threads into the shared state. Then it polls
//! the session host: every second while a turn is underway or orb is attached,
//! every five seconds otherwise, and right away when asked. Each poll maps the
//! host's records onto the threads, stamps when a turn starts, reads new
//! transcript lines for titles and branches, and saves what changed.
//!
//! It keeps each project's draft: created prefilled from the project's
//! last-used workspace, model and permission (else the latest used project's
//! model and permission in a local checkout), saved as the user edits it, and
//! following checkouts in the project's root while it is a local draft.
//! Starting a draft makes its worktree if it needs a new one, starts the
//! session with its model and permission, and replaces the draft with the new
//! thread; orb attaches to it only if the draft was still selected.
//!
//! Before a thread's first prompt it can move the thread to another workspace:
//! a new worktree of the project, or a directory that already exists. The new
//! session starts first, and only then is the old one removed; a worktree orb
//! made for a start that failed is removed again, and an orb worktree no
//! thread uses after a move is removed unless it has changes. When a turn
//! ends in a worktree still on orb's `orb/<hex>` branch, and Claude or the
//! user has titled the thread, the branch is renamed after that title.
//!
//! It checks branches out in a thread's directory or in a draft's (the
//! project's root, or the draft's existing worktree), unless a turn is
//! underway there. Before the first prompt, picking the default branch from a
//! worktree checks it out in the project's root and moves the thread, or the
//! draft, there. It makes a draft's project a git repository when asked.
//!
//! When Claude refuses to start in a directory it hasn't been trusted in, the
//! start waits: the actor asks the frontend for an interactive `claude` there,
//! and tries the start once more when asked. A second refusal fails the start.
//!
//! It keeps each thread's place in the sidebar: pinning, settling onto the
//! Settled shelf (which stops the session), un-settling, and deleting. A turn
//! un-settles its thread, and a thread idle for three days settles itself
//! unless it is pinned, was just un-settled, or orb is attached to it. A turn
//! that ends while the user is on another thread shows as unseen until they
//! select it. A thread being deleted is hidden from the sidebar until its
//! session is removed; if the removal fails, it shows again.
//!
//! It adds projects and removes them: a removed project leaves `␣n` and the
//! project filter and loses its draft, its threads stay, and adding it again
//! restores it.
//!
//! It restores the sidebar's saved width and project filter at start, and
//! saves them when asked.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use error_stack::Report;
use kameo::mailbox;
use kameo::prelude::{Actor, ActorRef, Context, Message, Reply, Spawn};
use tokio::sync::Notify;

use super::session_host::{
    SessionHostError, SessionHostService, SessionOptions, SessionRecord, WorkspaceUntrusted,
};
use super::state::{
    Draft, DraftWorkspace, Project, ProjectId, Sessions, SidebarItem, Thread, ThreadId,
    ThreadStatus,
};
use super::store::{
    DraftRow, LastUsed, LastWorkspace, NewThread, SettledOverride, Store, ThreadRow, Ui,
};
use super::transcript::{locate, scan_title};
use crate::Focus;
use crate::command::Workspace;
use crate::common::{Services, State, Wake};
use crate::feat::git::git_service::{GitError, GitRef, GitService, git_reason};
use crate::feat::git::validator::BUSY_DIRECTORY;
use crate::feat::git::worktree::{
    hex, hex_branch, is_orb_worktree, new_worktree_path, previous_worktree, slug,
};
use crate::feat::sidebar::state::{DEFAULT_WIDTH, clamp_width};

/// How long to wait between polls while a turn is underway or orb is attached.
const FAST_POLL: Duration = Duration::from_secs(1);
/// How long to wait between polls otherwise.
const SLOW_POLL: Duration = Duration::from_secs(5);
/// How long an unpinned thread stays idle before it settles itself, in ms.
const AUTO_SETTLE_AFTER: i64 = 3 * 24 * 60 * 60 * 1000;
/// The error shown when orb's store can't be written.
const SAVE_FAILED: &str = "couldn't save orb's state";
/// The error shown when a started session can't be saved or shown.
const NEW_SESSION_UNSAVED: &str = "couldn't save the new session";
/// How many random worktree names to try before giving up.
const WORKTREE_NAME_ATTEMPTS: u32 = 8;

/// What the sessions actor needs to start.
pub struct SessionsActorDeps {
    pub services: Services,
    pub state: State,
    pub store: Store,
    /// Claude's config directory, where transcripts live.
    pub claude_dir: PathBuf,
    /// Where orb makes new worktrees.
    pub worktrees_root: PathBuf,
    /// Tells the frontend to redraw.
    pub wake: Wake,
}

/// Owns [`Sessions`](super::state::Sessions): the projects and their drafts,
/// the threads' statuses, titles, pins and settles, the latest `claude` error,
/// the directory a start waits to be trusted in, and the started thread the
/// frontend should attach to. It also restores the sidebar's width and
/// project filter. The intent handler also moves the cursor, opens and closes
/// the shelf, marks a start as starting, edits a draft's fields before asking
/// for them to be saved, and resizes or filters the sidebar before asking for
/// that to be saved.
pub struct SessionsActor {
    services: Services,
    state: State,
    store: Store,
    claude_dir: PathBuf,
    worktrees_root: PathBuf,
    wake: Wake,
    /// Cuts the ticker's wait short so it polls now.
    poke: Arc<Notify>,
    /// The saved threads, as last written to the store.
    rows: Vec<ThreadRow>,
    /// The start waiting for the user to trust its directory.
    pending: Option<PendingStart>,
}

/// Poll the session host now.
#[derive(Debug)]
pub struct Poll;

/// How long to wait before the next [`Poll`].
#[derive(Debug, Reply)]
pub struct NextPoll(pub Duration);

/// Give a project a draft, prefilled from its last-used settings, unless it
/// has one.
#[derive(Debug)]
pub struct CreateDraft(pub ProjectId);

/// Save a project's draft as the app state has it, filling in its branch if
/// it has none.
#[derive(Debug)]
pub struct SaveDraft(pub ProjectId);

/// Check a branch out for a project's draft in `cwd`: the draft's directory,
/// or the project's root, which the draft then moves to.
#[derive(Debug)]
pub struct CheckoutDraft {
    pub project: ProjectId,
    pub git_ref: GitRef,
    pub cwd: PathBuf,
}

/// Make a project's root a git repository for its draft.
#[derive(Debug)]
pub struct InitGit(pub ProjectId);

/// Start a session from a project's draft, which becomes a thread.
#[derive(Debug)]
pub struct StartDraft(pub ProjectId);

/// Throw a project's draft away.
#[derive(Debug)]
pub struct DiscardDraft(pub ProjectId);

/// Start a thread's session over in another workspace, before its first
/// prompt.
#[derive(Debug)]
pub struct MoveThread {
    pub thread: ThreadId,
    pub to: Workspace,
}

/// Check a branch out for a thread: in its directory, or with `to_root` in
/// its project's root, moving the prompt-less thread there.
#[derive(Debug)]
pub struct SwitchBranch {
    pub thread: ThreadId,
    pub git_ref: GitRef,
    pub to_root: bool,
}

/// Try the start that waits for trust once more, now that the user had the
/// chance to trust its directory.
#[derive(Debug)]
pub struct RetryStart;

/// Add a directory as a project.
#[derive(Debug)]
pub struct AddProject(pub PathBuf);

/// Poll now instead of waiting for the next tick.
#[derive(Debug)]
pub struct RefreshSessions;

/// Pin a thread to the top of the sidebar, un-settling it if needed.
#[derive(Debug)]
pub struct Pin(pub ThreadId);

/// Unpin a thread.
#[derive(Debug)]
pub struct Unpin(pub ThreadId);

/// Settle a thread onto the Settled shelf and stop its session.
#[derive(Debug)]
pub struct Settle(pub ThreadId);

/// Take a thread off the Settled shelf and keep it active until its next turn.
#[derive(Debug)]
pub struct Unsettle(pub ThreadId);

/// Delete a thread's session and forget the thread.
#[derive(Debug)]
pub struct Delete(pub ThreadId);

/// The user selected a thread, so its latest turn is seen.
#[derive(Debug)]
pub struct Visit(pub ThreadId);

/// Save the sidebar's width and project filter as they now are in the app
/// state.
#[derive(Debug)]
pub struct SaveUi;

/// Remove a project from `␣n` and the project filter and discard its draft.
#[derive(Debug)]
pub struct RemoveProject(pub ProjectId);

/// A session start in flight: what it is for, where it runs, the branch
/// checked out there when known, the worktree orb made for it, if any, and
/// the options the session starts with.
struct PendingStart {
    kind: StartKind,
    cwd: PathBuf,
    branch: Option<String>,
    made: Option<MadeWorktree>,
    options: SessionOptions,
}

/// What a session start is for.
enum StartKind {
    /// A new thread from the project's draft, whose workspace was of kind
    /// `workspace`.
    Draft {
        project: ProjectId,
        workspace: LastWorkspace,
    },
    /// An existing, prompt-less thread moving out of `old_cwd`.
    Move {
        thread: ThreadId,
        old_short_id: String,
        old_cwd: PathBuf,
    },
}

/// A worktree orb made, on its new branch.
struct MadeWorktree {
    repo: PathBuf,
    path: PathBuf,
    branch: String,
}

/// Spawns the sessions actor with a mailbox that never refuses a message.
/// Must be called inside a tokio runtime.
pub fn spawn_sessions_actor(deps: SessionsActorDeps) -> ActorRef<SessionsActor> {
    SessionsActor::spawn_with_mailbox(deps, mailbox::unbounded())
}

impl Actor for SessionsActor {
    type Args = SessionsActorDeps;
    type Error = kameo::error::Infallible;

    fn on_start(
        args: Self::Args,
        actor_ref: ActorRef<Self>,
    ) -> impl Future<Output = Result<Self, Self::Error>> + Send {
        let actor = Self::restore(args);
        let poke = actor.poke.clone();
        tokio::spawn(async move {
            while let Ok(NextPoll(next)) = actor_ref.ask(Poll).await {
                tokio::select! {
                    () = tokio::time::sleep(next) => {}
                    () = poke.notified() => {}
                }
            }
        });
        std::future::ready(Ok(actor))
    }
}

impl Message<Poll> for SessionsActor {
    type Reply = NextPoll;

    async fn handle(&mut self, _msg: Poll, _ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        NextPoll(self.poll().await)
    }
}

impl Message<CreateDraft> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        CreateDraft(id): CreateDraft,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.create_draft(id);
    }
}

impl Message<SaveDraft> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        SaveDraft(id): SaveDraft,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.save_draft(id);
    }
}

impl Message<CheckoutDraft> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        CheckoutDraft {
            project,
            git_ref,
            cwd,
        }: CheckoutDraft,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.checkout_draft(project, &git_ref, &cwd);
    }
}

impl Message<InitGit> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        InitGit(id): InitGit,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.init_git(id);
    }
}

impl Message<StartDraft> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        StartDraft(id): StartDraft,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.start_draft(id).await;
    }
}

impl Message<DiscardDraft> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        DiscardDraft(id): DiscardDraft,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.discard_draft(id);
    }
}

impl Message<MoveThread> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        MoveThread { thread, to }: MoveThread,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.move_thread(thread, to).await;
    }
}

impl Message<SwitchBranch> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        SwitchBranch {
            thread,
            git_ref,
            to_root,
        }: SwitchBranch,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if to_root {
            self.switch_to_root(thread, &git_ref).await;
        } else {
            self.check_out(thread, &git_ref);
        }
    }
}

impl Message<RetryStart> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        _msg: RetryStart,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.retry_start().await;
    }
}

impl Message<AddProject> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        AddProject(root): AddProject,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.add_project(root);
    }
}

impl Message<RefreshSessions> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        _msg: RefreshSessions,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.poke.notify_one();
    }
}

impl Message<Pin> for SessionsActor {
    type Reply = ();

    async fn handle(&mut self, Pin(id): Pin, _ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.pin(id);
    }
}

impl Message<Unpin> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        Unpin(id): Unpin,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.unpin(id);
    }
}

impl Message<Settle> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        Settle(id): Settle,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.settle(id).await;
    }
}

impl Message<Unsettle> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        Unsettle(id): Unsettle,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.unsettle(id);
    }
}

impl Message<Delete> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        Delete(id): Delete,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.delete(id).await;
    }
}

impl Message<Visit> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        Visit(id): Visit,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.visit(id);
    }
}

impl Message<SaveUi> for SessionsActor {
    type Reply = ();

    async fn handle(&mut self, _msg: SaveUi, _ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.save_ui();
    }
}

impl Message<RemoveProject> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        RemoveProject(id): RemoveProject,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.remove_project(id);
    }
}

impl SessionsActor {
    /// Shows the saved projects, threads and drafts, sizes the sidebar as it
    /// was saved, kept within its bounds, filters it to the saved project if
    /// that's still shown and not removed, and selects its first row. Each
    /// draft learns what git says about it.
    fn restore(deps: SessionsActorDeps) -> Self {
        let SessionsActorDeps {
            services,
            state,
            store,
            claude_dir,
            worktrees_root,
            wake,
        } = deps;
        let (projects, rows, drafts, error) = match store.load() {
            Ok((projects, rows, drafts)) => (projects, rows, drafts, None),
            Err(_) => (
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Some("couldn't load orb's saved sessions".to_owned()),
            ),
        };
        let projects: Vec<Project> = projects
            .into_iter()
            .map(|project| Project {
                threads: rows
                    .iter()
                    .filter(|row| row.project_id == project.id)
                    .map(|row| unpolled(&services.session_host, row))
                    .collect(),
                draft: drafts
                    .iter()
                    .find(|draft| draft.project_id == project.id)
                    .map(|row| with_git(&services.git, &project.root, draft_of(row))),
                id: project.id,
                title: project.title,
                root: project.root,
                created_at: from_ms(project.created_at),
                removed: project.removed_at.is_some(),
            })
            .collect();
        let ui = store.ui().unwrap_or_default();
        let filter = ui.project_filter.filter(|&id| {
            projects
                .iter()
                .any(|project| project.id == id && !project.removed)
        });
        {
            let mut app = state.write();
            app.sidebar.width = ui.sidebar_width.map_or(DEFAULT_WIDTH, clamp_width);
            let sessions = &mut app.sessions;
            sessions.projects = projects;
            sessions.cursor = None;
            sessions.filter_to(filter);
            sessions.error = error;
        }
        wake();
        Self {
            services,
            state,
            store,
            claude_dir,
            worktrees_root,
            wake,
            poke: Arc::default(),
            rows,
            pending: None,
        }
    }

    /// Asks the host what every session is doing and shows it. Returns how
    /// long to wait before the next poll.
    async fn poll(&mut self) -> Duration {
        if let Err(report) = self.sync().await {
            self.fail(&report);
        }
        let app = self.state.read();
        if app.sessions.any_in_progress() || app.focus == Focus::Attached {
            FAST_POLL
        } else {
            SLOW_POLL
        }
    }

    /// Asks the host what every session is doing, shows it, and stops the
    /// sessions that auto-settled.
    async fn sync(&mut self) -> Result<(), Report<SessionHostError>> {
        let records = self.services.session_host.list().await?;
        // ponytail: stops run in order (~0.7 s each); only a first launch
        // that auto-settles many threads waits long.
        for short_id in self.apply(&records) {
            self.stop(&short_id).await;
        }
        Ok(())
    }

    /// Updates every saved thread from its record and transcript, follows its
    /// settle lifecycle, saves the ones that changed, then shows them all in
    /// one write. Returns the sessions that auto-settled and should stop.
    fn apply(&mut self, records: &[SessionRecord]) -> Vec<String> {
        let now = now_ms();
        let (cursor, attached) = {
            let app = self.state.read();
            (app.sessions.cursor, app.focus == Focus::Attached)
        };
        let mut statuses = Vec::with_capacity(self.rows.len());
        let mut to_stop = Vec::new();
        let mut error = None;
        for row in &mut self.rows {
            // `--all` can list a stale stopped record next to the live one.
            let record = records
                .iter()
                .filter(|record| record.short_id == row.short_id)
                .min_by_key(|record| {
                    matches!(record.status, ThreadStatus::Stopped | ThreadStatus::Failed)
                });
            let status = record.map_or(ThreadStatus::Gone, |record| record.status);
            let before = row.clone();
            let was_in_progress = row.turn_started_at.is_some();
            update_row(row, record, status, now, &self.claude_dir);
            if was_in_progress && !status.in_progress() {
                rename_hex_branch(&self.services.git, &self.worktrees_root, row);
            }
            let selected = cursor == Some(SidebarItem::Thread(row.id));
            if follow_activity(row, status, was_in_progress, selected, attached, now) {
                to_stop.push(row.short_id.clone());
            }
            if *row != before && self.store.save_thread(row).is_err() {
                error = Some(SAVE_FAILED.to_owned());
            }
            statuses.push(status);
        }
        let changed = {
            let mut app = self.state.write();
            let sessions = &mut app.sessions;
            let mut changed = false;
            if let Some(error) = error {
                changed = sessions.error.as_ref() != Some(&error);
                sessions.error = Some(error);
            }
            for (row, status) in self.rows.iter().zip(statuses) {
                if let Some(thread) = thread_mut(sessions, row.id) {
                    changed |= show(thread, row, status);
                }
            }
            changed
        };
        if changed {
            (self.wake)();
        }
        to_stop
    }

    /// Gives project `id` a draft unless it has one: the project's last-used
    /// workspace, model and permission, else the latest used project's model
    /// and permission in a local checkout. A remembered previous worktree the
    /// project no longer has falls back to a local checkout. The branch is the
    /// one checked out in the draft's directory, or for a new worktree the
    /// default branch; see [`with_git`].
    fn create_draft(&mut self, id: ProjectId) {
        let seeds = self
            .state
            .read()
            .sessions
            .projects
            .iter()
            .find(|project| project.id == id && project.draft.is_none())
            .map(|project| {
                let previous = previous_worktree(project, &project.root, None);
                (project.root.clone(), previous)
            });
        let Some((root, previous)) = seeds else {
            return (self.wake)();
        };
        let last = self.store.last_used(id).ok().flatten();
        let (workspace, branch) = match (last.as_ref().map(|last| last.workspace), previous) {
            (Some(LastWorkspace::NewWorktree), _) => (DraftWorkspace::NewWorktree, None),
            (Some(LastWorkspace::Previous), Some((path, branch))) => {
                (DraftWorkspace::Existing(path), branch)
            }
            _ => (DraftWorkspace::Local, None),
        };
        let settings = last.or_else(|| self.store.latest_last_used().ok().flatten());
        let draft = with_git(
            &self.services.git,
            &root,
            Draft {
                branch,
                workspace,
                model: settings.as_ref().and_then(|used| used.model.clone()),
                permission: settings.and_then(|used| used.permission_mode),
                created_at: from_ms(now_ms()),
                repo: true,
                from: None,
            },
        );
        let saved = self.store.save_draft(&draft_row(id, &draft));
        {
            let mut app = self.state.write();
            let sessions = &mut app.sessions;
            if let Some(project) = sessions.projects.iter_mut().find(|p| p.id == id) {
                project.draft = Some(draft);
            }
            if saved.is_err() {
                sessions.error = Some(SAVE_FAILED.to_owned());
            }
        }
        (self.wake)();
    }

    /// Saves project `id`'s draft as the app state has it, first bringing
    /// what git says about it up to date; see [`with_git`].
    fn save_draft(&mut self, id: ProjectId) {
        let Some((root, draft)) = self.draft(id) else {
            return;
        };
        let seen = with_git(&self.services.git, &root, draft.clone());
        let row = {
            let mut app = self.state.write();
            let Some(shown) = app.sessions.draft_mut(id) else {
                return;
            };
            shown.repo = seen.repo;
            // The user may have changed it meanwhile; a later save follows.
            if shown.workspace == draft.workspace && shown.branch == draft.branch {
                shown.branch = seen.branch;
                shown.from = seen.from;
            }
            draft_row(id, shown)
        };
        if self.store.save_draft(&row).is_err() {
            self.state.write().sessions.error = Some(SAVE_FAILED.to_owned());
        }
        (self.wake)();
    }

    /// Checks `git_ref` out in `cwd` for project `id`'s draft. A checkout in
    /// the project's root moves a draft in a worktree back to the root.
    fn checkout_draft(&mut self, id: ProjectId, git_ref: &GitRef, cwd: &Path) {
        let Some(root) = self.project_root(id) else {
            return;
        };
        if let Some(local) = self.check_out_in(cwd, git_ref)
            && cwd == root
        {
            self.draft_to_root(id, local);
        }
    }

    /// Moves project `id`'s draft from its worktree to the project's root,
    /// on `branch`, and saves it.
    fn draft_to_root(&mut self, id: ProjectId, branch: String) {
        let row = {
            let mut app = self.state.write();
            let Some(draft) = app
                .sessions
                .draft_mut(id)
                .filter(|draft| matches!(draft.workspace, DraftWorkspace::Existing(_)))
            else {
                return;
            };
            draft.workspace = DraftWorkspace::Local;
            draft.branch = Some(branch);
            draft.from = None;
            draft_row(id, draft)
        };
        if self.store.save_draft(&row).is_err() {
            self.state.write().sessions.error = Some(SAVE_FAILED.to_owned());
        }
        (self.wake)();
    }

    /// Runs `git init` in project `id`'s root, then shows its draft as a git
    /// project's. A failure shows why.
    fn init_git(&mut self, id: ProjectId) {
        let Some(root) = self.project_root(id) else {
            return;
        };
        match self.services.git.init(&root) {
            Ok(()) => self.save_draft(id),
            Err(report) => {
                self.state.write().sessions.error = Some(format!(
                    "Git initialization failed: {}",
                    git_reason(&report)
                ));
                (self.wake)();
            }
        }
    }

    /// Forgets project `id`'s draft.
    fn discard_draft(&mut self, id: ProjectId) {
        let deleted = self.store.delete_draft(id);
        {
            let mut app = self.state.write();
            let sessions = &mut app.sessions;
            if let Some(project) = sessions.projects.iter_mut().find(|p| p.id == id) {
                project.draft = None;
            }
            if deleted.is_err() {
                sessions.error = Some(SAVE_FAILED.to_owned());
            }
        }
        (self.wake)();
    }

    /// Starts a session from project `id`'s draft, with its model and
    /// permission: in the project's root, in its existing worktree, or in a
    /// new worktree made now from the draft's base branch. A draft of a
    /// project that isn't a git repository always starts in the root.
    async fn start_draft(&mut self, id: ProjectId) {
        let Some((root, draft)) = self.draft(id) else {
            return self.end_start(Err("the draft is gone".to_owned()));
        };
        let options = SessionOptions {
            model: draft.model,
            permission_mode: draft.permission,
        };
        let workspace = if draft.repo {
            draft.workspace
        } else {
            DraftWorkspace::Local
        };
        let (workspace, cwd, made) = match workspace {
            DraftWorkspace::Local => (LastWorkspace::Local, root, None),
            DraftWorkspace::Existing(path) if path.is_dir() => {
                (LastWorkspace::Previous, path, None)
            }
            DraftWorkspace::Existing(path) => {
                return self.end_start(Err(format!(
                    "worktree no longer exists: {}",
                    path.display()
                )));
            }
            DraftWorkspace::NewWorktree => {
                match self.add_worktree(&root, draft.branch.as_deref()) {
                    Ok(made) => (LastWorkspace::NewWorktree, made.path.clone(), Some(made)),
                    Err(report) => return self.end_start(Err(git_reason(&report))),
                }
            }
        };
        // The draft's branch can be stale (a checkout outside orb, a restart).
        let branch = match &made {
            Some(made) => Some(made.branch.clone()),
            None => current_branch(&self.services.git, &cwd),
        };
        let pending = PendingStart {
            kind: StartKind::Draft {
                project: id,
                workspace,
            },
            cwd,
            branch,
            made,
            options,
        };
        self.start(pending, true).await;
    }

    /// Project `id`'s root and draft, if it has one.
    fn draft(&self, id: ProjectId) -> Option<(PathBuf, Draft)> {
        self.state
            .read()
            .sessions
            .projects
            .iter()
            .find(|project| project.id == id)
            .and_then(|project| Some((project.root.clone(), project.draft.clone()?)))
    }

    /// Starts a session for `pending` and finishes what it was for. If Claude
    /// hasn't been trusted in its directory and `allow_trust`, the start waits
    /// for the user to trust it. If it fails, the reason shows and a worktree
    /// made for it is removed.
    async fn start(&mut self, pending: PendingStart, allow_trust: bool) {
        let created = self
            .services
            .session_host
            .create(&pending.cwd, &pending.options)
            .await;
        if let Err(report) = &created
            && allow_trust
            && report.contains::<WorkspaceUntrusted>()
        {
            self.state.write().sessions.trust = Some(pending.cwd.clone());
            self.pending = Some(pending);
            return (self.wake)();
        }
        let PendingStart {
            kind,
            cwd,
            branch,
            made,
            options,
        } = pending;
        match (created, kind) {
            (Ok(created), StartKind::Draft { project, workspace }) => {
                let thread = self.save_new(project, &cwd, &created.short_id, branch, &options);
                let used = LastUsed {
                    workspace,
                    model: options.model,
                    permission_mode: options.permission_mode,
                };
                self.finish_draft(project, &used, thread);
            }
            (
                Ok(created),
                StartKind::Move {
                    thread,
                    old_short_id,
                    old_cwd,
                },
            ) => {
                let moved = Moved {
                    thread,
                    short_id: created.short_id,
                    cwd,
                    branch,
                };
                self.finish_move(moved, &old_short_id, &old_cwd).await;
            }
            (Err(report), _) => {
                if let Some(made) = made {
                    self.remove_made(&made);
                }
                self.end_start(Err(reason(&report)));
            }
        }
    }

    /// Tries the start waiting for trust once more; a second refusal fails it.
    async fn retry_start(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        self.state.write().sessions.trust = None;
        self.start(pending, false).await;
    }

    /// Replaces project `project_id`'s draft with the thread `created` from
    /// it, first in the project, and records `used` as the project's last-used
    /// settings. If the draft was still selected, selects the thread and asks
    /// the frontend to attach to it.
    fn finish_draft(
        &mut self,
        project_id: ProjectId,
        used: &LastUsed,
        created: Result<Thread, String>,
    ) {
        let result = created.and_then(|thread| {
            let saved = self
                .store
                .delete_draft(project_id)
                .and_then(|()| self.store.record_last_used(project_id, used, now_ms()));
            let mut app = self.state.write();
            let sessions = &mut app.sessions;
            let project = sessions
                .projects
                .iter_mut()
                .find(|project| project.id == project_id)
                .ok_or_else(|| NEW_SESSION_UNSAVED.to_owned())?;
            let id = thread.id;
            project.draft = None;
            project.threads.insert(0, thread);
            if sessions.cursor == Some(SidebarItem::Draft(project_id)) {
                sessions.cursor = Some(SidebarItem::Thread(id));
                sessions.attach = Some(id);
            }
            saved.map_err(|_report| SAVE_FAILED.to_owned())
        });
        self.end_start(result);
    }

    /// Ends a session start: stops showing it as starting and shows `result`'s
    /// error, or clears the error and polls the new session now.
    fn end_start(&self, result: Result<(), String>) {
        let succeeded = result.is_ok();
        {
            let mut app = self.state.write();
            let sessions = &mut app.sessions;
            sessions.starting = false;
            sessions.error = result.err();
        }
        (self.wake)();
        if succeeded {
            self.poke.notify_one();
        }
    }

    /// Moves a prompt-less thread to another workspace: prepares it, then
    /// starts the thread's session there.
    async fn move_thread(&mut self, id: ThreadId, to: Workspace) {
        match self.prepare_move(id, to).await {
            Ok(pending) => self.start(pending, true).await,
            Err(error) => self.end_start(Err(error)),
        }
    }

    /// Checks again that thread `id` has had no prompt, then gets the
    /// workspace ready: an existing directory, or a new worktree of the
    /// project, made from its default branch (fetched first from `origin` when
    /// there is one). The session starts over with the thread's model and
    /// permission.
    async fn prepare_move(&mut self, id: ThreadId, to: Workspace) -> Result<PendingStart, String> {
        self.sync().await.map_err(|report| reason(&report))?;
        let row = self
            .rows
            .iter()
            .find(|row| row.id == id)
            .ok_or_else(|| "the thread is gone".to_owned())?;
        let root = self
            .project_root(row.project_id)
            .ok_or_else(|| "the thread's project is gone".to_owned())?;
        let busy = self.status(id).is_some_and(ThreadStatus::in_progress);
        if row.transcript_path.is_some() || busy {
            let workspace = if row.cwd == root {
                "Local checkout"
            } else {
                "Worktree"
            };
            return Err(format!("Workspace locked · {workspace}"));
        }
        let kind = StartKind::Move {
            thread: id,
            old_short_id: row.short_id.clone(),
            old_cwd: row.cwd.clone(),
        };
        let options = SessionOptions {
            model: row.model.clone(),
            permission_mode: row.permission_mode.clone(),
        };
        let (cwd, made) = match to {
            Workspace::Existing(path) if path.is_dir() => (path, None),
            Workspace::Existing(path) => {
                return Err(format!("worktree no longer exists: {}", path.display()));
            }
            Workspace::NewWorktree => {
                let made = self
                    .add_worktree(&root, None)
                    .map_err(|report| git_reason(&report))?;
                (made.path.clone(), Some(made))
            }
        };
        Ok(PendingStart {
            kind,
            cwd,
            branch: made.as_ref().map(|made| made.branch.clone()),
            made,
            options,
        })
    }

    /// Makes a new worktree of the repository at `repo` on a new `orb/<hex>`
    /// branch, from `base` (else the default branch) as origin has it, else as
    /// it is locally. A ref of another remote, like `upstream/x`, is used as it
    /// is.
    fn add_worktree(
        &self,
        repo: &Path,
        base: Option<&str>,
    ) -> Result<MadeWorktree, Report<GitError>> {
        let git = &self.services.git;
        let base = match base {
            Some(base) => base.to_owned(),
            None => git.default_branch(repo)?,
        };
        let base = start_point(git, repo, &base, |branch| git.fetch(repo, branch))?;
        let (path, branch) = (0..WORKTREE_NAME_ATTEMPTS)
            .map(|attempt| {
                let hex = hex(attempt);
                (
                    new_worktree_path(&self.worktrees_root, repo, &hex),
                    format!("orb/{hex}"),
                )
            })
            .find(|(path, branch)| !path.exists() && !git.branch_exists(repo, branch))
            .ok_or_else(|| Report::new(GitError).attach("couldn't pick a new worktree name"))?;
        git.add_worktree(repo, &path, &branch, &base)?;
        Ok(MadeWorktree {
            repo: repo.to_owned(),
            path,
            branch,
        })
    }

    /// Removes a worktree orb just made, and its branch, discarding anything
    /// in them.
    fn remove_made(&self, made: &MadeWorktree) {
        let git = &self.services.git;
        if git.remove_worktree(&made.repo, &made.path, true).is_ok() {
            let _ = git.delete_branch(&made.repo, &made.branch, true);
        }
    }

    /// Finishes a move once the new session runs: removes the old session,
    /// saves and shows the thread in its new workspace, and removes the orb
    /// worktree it left if no thread uses it and it has no changes.
    async fn finish_move(&mut self, moved: Moved, old_short_id: &str, old_cwd: &Path) {
        let removed = self.services.session_host.remove(old_short_id).await;
        let branch = moved.branch.or_else(|| {
            self.rows
                .iter()
                .find(|row| row.id != moved.thread && row.cwd == moved.cwd)
                .and_then(|row| row.branch.clone())
        });
        let Some(row) = self.rows.iter_mut().find(|row| row.id == moved.thread) else {
            return self.end_start(Err(NEW_SESSION_UNSAVED.to_owned()));
        };
        let old_branch = row.branch.take();
        *row = ThreadRow {
            short_id: moved.short_id,
            cwd: moved.cwd,
            session_id: None,
            transcript_path: None,
            transcript_offset: 0,
            title: None,
            custom_title: None,
            ai_titled: false,
            branch,
            ..row.clone()
        };
        let saved = self.store.save_thread(row);
        let shown = unpolled(&self.services.session_host, row);
        let project_id = row.project_id;
        if let Some(thread) = thread_mut(&mut self.state.write().sessions, moved.thread) {
            *thread = shown;
        }
        if is_orb_worktree(&self.worktrees_root, old_cwd)
            && !self.rows.iter().any(|row| row.cwd == old_cwd)
            && let Some(root) = self.project_root(project_id)
        {
            let git = &self.services.git;
            if git.remove_worktree(&root, old_cwd, false).is_ok()
                && let Some(old_branch) = old_branch
            {
                let _ = git.delete_branch(&root, &old_branch, false);
            }
        }
        let result = match (saved, removed) {
            (Err(_), _) => Err(SAVE_FAILED.to_owned()),
            (Ok(()), Err(report)) => Err(reason(&report)),
            (Ok(()), Ok(())) => Ok(()),
        };
        self.end_start(result);
    }

    /// Checks `git_ref` out in thread `id`'s directory.
    fn check_out(&mut self, id: ThreadId, git_ref: &GitRef) {
        let cwd = self
            .rows
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.cwd.clone());
        if let Some(cwd) = cwd {
            let _ = self.check_out_in(&cwd, git_ref);
        }
    }

    /// Checks `git_ref` out in `cwd` and shows the branch there. Refused while
    /// a turn is underway in it. Returns the branch checked out, if it was.
    fn check_out_in(&mut self, cwd: &Path, git_ref: &GitRef) -> Option<String> {
        let result = if self.busy_in(cwd) {
            Err(BUSY_DIRECTORY.to_owned())
        } else {
            match self.services.git.checkout(cwd, git_ref) {
                Ok(local) => self.show_branch(cwd, &local).map(|()| local),
                Err(report) => Err(git_reason(&report)),
            }
        };
        let (checked_out, error) = match result {
            Ok(local) => (Some(local), None),
            Err(error) => (None, Some(error)),
        };
        self.state.write().sessions.error = error;
        (self.wake)();
        checked_out
    }

    /// Checks the default branch out in the project's root and moves the
    /// prompt-less thread `id` there from its worktree. Refused while a turn
    /// is underway in the root.
    async fn switch_to_root(&mut self, id: ThreadId, git_ref: &GitRef) {
        let root = self
            .rows
            .iter()
            .find(|row| row.id == id)
            .and_then(|row| self.project_root(row.project_id));
        let Some(root) = root else {
            return self.end_start(Err("the thread is gone".to_owned()));
        };
        let pending = match self
            .prepare_move(id, Workspace::Existing(root.clone()))
            .await
        {
            Ok(pending) => pending,
            Err(error) => return self.end_start(Err(error)),
        };
        if self.busy_in(&root) {
            return self.end_start(Err(BUSY_DIRECTORY.to_owned()));
        }
        match self.services.git.checkout(&root, git_ref) {
            Ok(local) => {
                let _ = self.show_branch(&root, &local);
                let pending = PendingStart {
                    branch: Some(local),
                    ..pending
                };
                self.start(pending, true).await;
            }
            Err(report) => self.end_start(Err(git_reason(&report))),
        }
    }

    /// Saves and shows `branch` on every thread in `cwd`, and on the draft
    /// there: a local draft of a project rooted there, or one in that
    /// worktree.
    fn show_branch(&mut self, cwd: &Path, branch: &str) -> Result<(), String> {
        let mut saved = Ok(());
        let mut app = self.state.write();
        for row in self.rows.iter_mut().filter(|row| row.cwd == cwd) {
            row.branch = Some(branch.to_owned());
            if self.store.save_thread(row).is_err() {
                saved = Err(SAVE_FAILED.to_owned());
            }
            if let Some(thread) = thread_mut(&mut app.sessions, row.id) {
                let status = thread.status;
                show(thread, row, status);
            }
        }
        for project in &mut app.sessions.projects {
            let root = project.root.as_path();
            if let Some(draft) = project
                .draft
                .as_mut()
                .filter(|draft| match &draft.workspace {
                    DraftWorkspace::Local => root == cwd,
                    DraftWorkspace::Existing(path) => path == cwd,
                    DraftWorkspace::NewWorktree => false,
                })
            {
                draft.branch = Some(branch.to_owned());
                if self
                    .store
                    .save_draft(&draft_row(project.id, draft))
                    .is_err()
                {
                    saved = Err(SAVE_FAILED.to_owned());
                }
            }
        }
        saved
    }

    /// Whether a turn is underway in any thread in `cwd`.
    fn busy_in(&self, cwd: &Path) -> bool {
        self.state
            .read()
            .sessions
            .threads()
            .any(|thread| thread.cwd == cwd && thread.status.in_progress())
    }

    /// Saves a session just started in `cwd` on `branch` with `options`
    /// under the project.
    fn save_new(
        &mut self,
        project_id: ProjectId,
        cwd: &Path,
        short_id: &str,
        branch: Option<String>,
        options: &SessionOptions,
    ) -> Result<Thread, String> {
        let now = now_ms();
        let new = NewThread {
            project_id,
            short_id: short_id.to_owned(),
            cwd: cwd.to_owned(),
            created_at: now,
            model: options.model.clone(),
            permission_mode: options.permission_mode.clone(),
        };
        let Ok(id) = self.store.insert_thread(&new) else {
            return Err(NEW_SESSION_UNSAVED.to_owned());
        };
        let row = ThreadRow {
            id,
            project_id,
            short_id: new.short_id,
            session_id: None,
            title: None,
            custom_title: None,
            cwd: new.cwd,
            transcript_path: None,
            transcript_offset: 0,
            created_at: now,
            turn_started_at: None,
            branch,
            pinned_at: None,
            settled_override: None,
            settled_at: None,
            unsettled_at: None,
            last_activity_at: now,
            last_visited_at: now,
            ai_titled: false,
            model: new.model,
            permission_mode: new.permission_mode,
        };
        if row.branch.is_some() && self.store.save_thread(&row).is_err() {
            return Err(NEW_SESSION_UNSAVED.to_owned());
        }
        let thread = unpolled(&self.services.session_host, &row);
        self.rows.push(row);
        Ok(thread)
    }

    /// Saves `root` as a project, unless it already is one, and shows it; a
    /// removed project is restored.
    fn add_project(&mut self, root: PathBuf) {
        let now = now_ms();
        let title = project_title(&root);
        let added = if root.is_dir() {
            self.store
                .add_project(&root, &title, now)
                .map_err(|_report| SAVE_FAILED.to_owned())
        } else {
            Err(format!("not a directory: {}", root.display()))
        };
        {
            let mut app = self.state.write();
            let sessions = &mut app.sessions;
            match added {
                Ok(id) => {
                    sessions.error = None;
                    match sessions
                        .projects
                        .iter_mut()
                        .find(|project| project.id == id)
                    {
                        Some(project) => project.removed = false,
                        None => sessions.projects.push(Project {
                            id,
                            title,
                            root,
                            created_at: from_ms(now),
                            threads: Vec::new(),
                            draft: None,
                            removed: false,
                        }),
                    }
                }
                Err(error) => sessions.error = Some(error),
            }
        }
        (self.wake)();
    }

    /// Removes project `id` from `␣n` and the project filter and discards its
    /// draft, once the store has; its threads stay.
    fn remove_project(&mut self, id: ProjectId) {
        let removed = self.store.remove_project(id, now_ms());
        {
            let mut app = self.state.write();
            let sessions = &mut app.sessions;
            match (removed, sessions.projects.iter_mut().find(|p| p.id == id)) {
                (Ok(()), Some(project)) => {
                    project.removed = true;
                    project.draft = None;
                }
                (Ok(()), None) => {}
                (Err(_), _) => sessions.error = Some(SAVE_FAILED.to_owned()),
            }
        }
        (self.wake)();
    }

    /// Pins a thread; pinning a settled thread un-settles it.
    fn pin(&mut self, id: ThreadId) {
        self.edit(id, |row, now| {
            row.pinned_at = row.pinned_at.or(Some(now));
            if row.settled_override == Some(SettledOverride::Settled) {
                unsettle_row(row, now);
            }
        });
    }

    fn unpin(&mut self, id: ThreadId) {
        self.edit(id, |row, _| row.pinned_at = None);
    }

    /// Settles a thread that isn't mid-turn, then stops its session if it's
    /// idle. A turn that started after the key press wins.
    async fn settle(&mut self, id: ThreadId) {
        let (Some(short_id), Some(status)) = (self.short_id(id), self.status(id)) else {
            return;
        };
        if status.in_progress() {
            return;
        }
        self.edit(id, settle_row);
        if status == ThreadStatus::Idle {
            // ponytail: stop runs on the actor (~0.7 s); spawn it if triage feels laggy.
            self.stop(&short_id).await;
        }
    }

    fn unsettle(&mut self, id: ThreadId) {
        self.edit(id, unsettle_row);
    }

    /// Marks a thread's latest turn seen.
    fn visit(&mut self, id: ThreadId) {
        self.edit(id, |row, now| row.last_visited_at = now);
    }

    /// Removes a thread's session, then forgets the thread. A session Claude
    /// no longer knows isn't removed first. If the removal fails, the thread
    /// stays, shown again, and the reason shows. However it ends, the thread
    /// is no longer hidden as being deleted.
    async fn delete(&mut self, id: ThreadId) {
        let (Some(short_id), Some(status)) = (self.short_id(id), self.status(id)) else {
            self.state.write().sessions.deleting.remove(&id);
            (self.wake)();
            return;
        };
        if status != ThreadStatus::Gone
            && let Err(report) = self.services.session_host.remove(&short_id).await
        {
            {
                let mut app = self.state.write();
                app.sessions.deleting.remove(&id);
                app.sessions.error = Some(reason(&report));
            }
            (self.wake)();
            return;
        }
        let deleted = self.store.delete_thread(id);
        self.rows.retain(|row| row.id != id);
        {
            let mut app = self.state.write();
            let sessions = &mut app.sessions;
            for project in &mut sessions.projects {
                project.threads.retain(|thread| thread.id != id);
            }
            sessions.deleting.remove(&id);
            if deleted.is_err() {
                sessions.error = Some(SAVE_FAILED.to_owned());
            }
        }
        (self.wake)();
    }

    /// Saves the sidebar's width and project filter, showing why if it can't.
    fn save_ui(&self) {
        let ui = {
            let app = self.state.read();
            Ui {
                sidebar_width: Some(app.sidebar.width),
                project_filter: app.sessions.filter,
            }
        };
        if self.store.save_ui(&ui).is_err() {
            self.state.write().sessions.error = Some(SAVE_FAILED.to_owned());
            (self.wake)();
        }
    }

    /// Changes thread `id`'s saved row with `change` (given the time now),
    /// then saves and shows it. Does nothing if the thread is gone.
    fn edit<F>(&mut self, id: ThreadId, change: F)
    where
        F: FnOnce(&mut ThreadRow, i64),
    {
        let Some(row) = self.rows.iter_mut().find(|row| row.id == id) else {
            return;
        };
        change(row, now_ms());
        let saved = self.store.save_thread(row);
        {
            let mut app = self.state.write();
            let sessions = &mut app.sessions;
            if saved.is_err() {
                sessions.error = Some(SAVE_FAILED.to_owned());
            }
            if let Some(thread) = thread_mut(sessions, id) {
                let status = thread.status;
                show(thread, row, status);
            }
        }
        (self.wake)();
    }

    /// Stops a session, showing why if it can't.
    async fn stop(&mut self, short_id: &str) {
        if let Err(report) = self.services.session_host.stop(short_id).await {
            self.fail(&report);
        }
    }

    /// Shows a `claude` failure in the mode line.
    fn fail(&self, report: &Report<SessionHostError>) {
        self.state.write().sessions.error = Some(reason(report));
        (self.wake)();
    }

    /// The session id `claude --bg` gave thread `id`.
    fn short_id(&self, id: ThreadId) -> Option<String> {
        self.rows
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.short_id.clone())
    }

    /// The directory project `id`'s sessions start in.
    fn project_root(&self, id: ProjectId) -> Option<PathBuf> {
        self.state
            .read()
            .sessions
            .projects
            .iter()
            .find(|project| project.id == id)
            .map(|project| project.root.clone())
    }

    /// What thread `id`'s session was doing at the last poll.
    fn status(&self, id: ThreadId) -> Option<ThreadStatus> {
        self.state
            .read()
            .sessions
            .threads()
            .find(|thread| thread.id == id)
            .map(|thread| thread.status)
    }
}

/// A thread whose session started over in a new workspace.
struct Moved {
    thread: ThreadId,
    /// The new session's id.
    short_id: String,
    /// The new workspace.
    cwd: PathBuf,
    /// The new workspace's branch, when orb made or checked it out.
    branch: Option<String>,
}

/// Follows a polled row's settle lifecycle, given its `status` now and
/// whether a turn was underway before this poll:
/// - a turn that ended is the thread's latest activity;
/// - a turn underway un-settles the thread and ends its kept-active mark;
/// - an unpinned thread idle for [`AUTO_SETTLE_AFTER`] settles, unless it is
///   kept active or orb is attached to it;
/// - the selected thread's latest turn is seen.
///
/// Returns whether it auto-settled with an idle session to stop.
fn follow_activity(
    row: &mut ThreadRow,
    status: ThreadStatus,
    was_in_progress: bool,
    selected: bool,
    attached: bool,
    now: i64,
) -> bool {
    if was_in_progress && !status.in_progress() {
        row.last_activity_at = now;
    }
    if status.in_progress()
        && let Some(settled_override) = row.settled_override
    {
        if settled_override == SettledOverride::Settled {
            row.unsettled_at = Some(now);
        }
        row.settled_override = None;
        row.settled_at = None;
    }
    let auto_settle = row.settled_override.is_none()
        && row.pinned_at.is_none()
        && !status.in_progress()
        && !(selected && attached)
        && now.saturating_sub(row.last_activity_at) >= AUTO_SETTLE_AFTER;
    if auto_settle {
        settle_row(row, row.last_activity_at);
    }
    if selected && row.last_activity_at > row.last_visited_at {
        row.last_visited_at = now;
    }
    auto_settle && status == ThreadStatus::Idle
}

/// Settles `row` onto the shelf as of `at`, unpinning it.
fn settle_row(row: &mut ThreadRow, at: i64) {
    row.settled_override = Some(SettledOverride::Settled);
    row.settled_at = Some(at);
    row.unsettled_at = None;
    row.pinned_at = None;
}

/// Un-settles `row` and keeps it active until its next turn activity. It
/// re-enters Active at `now` unless it was already kept active.
fn unsettle_row(row: &mut ThreadRow, now: i64) {
    if row.settled_override != Some(SettledOverride::Active) {
        row.unsettled_at = Some(now);
    }
    row.settled_override = Some(SettledOverride::Active);
    row.settled_at = None;
}

/// The shown thread with id `id`.
fn thread_mut(sessions: &mut Sessions, id: ThreadId) -> Option<&mut Thread> {
    sessions
        .projects
        .iter_mut()
        .flat_map(|project| &mut project.threads)
        .find(|thread| thread.id == id)
}

/// Brings a saved thread up to date with its record: the session id, the
/// turn stamp, and the title and branch from any new transcript lines.
fn update_row(
    row: &mut ThreadRow,
    record: Option<&SessionRecord>,
    status: ThreadStatus,
    now_ms: i64,
    claude_dir: &Path,
) {
    if let Some(session_id) = record.and_then(|record| record.session_id.as_ref())
        && row.session_id.as_ref() != Some(session_id)
    {
        // A new Claude session (e.g. after `/clear`) writes a new transcript.
        if row.session_id.is_some() {
            row.transcript_path = None;
            row.transcript_offset = 0;
        }
        row.session_id = Some(session_id.clone());
    }
    row.turn_started_at = status
        .in_progress()
        .then(|| row.turn_started_at.unwrap_or(now_ms));
    let Some(session_id) = &row.session_id else {
        return;
    };
    if row.transcript_path.is_none() {
        row.transcript_path = locate(claude_dir, &row.cwd, session_id);
    }
    if let Some(path) = &row.transcript_path
        && let Ok(scan) = scan_title(
            path,
            row.transcript_offset,
            row.title.clone(),
            row.custom_title.clone(),
            row.branch.clone(),
        )
    {
        row.title = scan.title;
        row.custom_title = scan.custom_title;
        row.branch = scan.branch;
        row.ai_titled |= scan.ai_titled;
        row.transcript_offset = scan.offset;
    }
}

/// Renames the `orb/<hex>` branch of the orb worktree `row` is in to
/// `orb/<slug>` of its title, once Claude or the user titled it. The old name
/// stays when that branch already exists or git refuses.
fn rename_hex_branch(git: &GitService, worktrees_root: &Path, row: &mut ThreadRow) {
    if let Some(old) = hex_branch(worktrees_root, &row.cwd)
        && row.branch.as_deref() == Some(old.as_str())
        && (row.custom_title.is_some() || row.ai_titled)
        && let Some(new) = display_title(row)
            .as_deref()
            .and_then(slug)
            .map(|slug| format!("orb/{slug}"))
        && !git.branch_exists(&row.cwd, &new)
        && git.rename_branch(&row.cwd, &old, &new).is_ok()
    {
        row.branch = Some(new);
    }
}

/// Shows a saved thread and `status` on `thread`. Returns whether anything
/// visible changed.
fn show(thread: &mut Thread, row: &ThreadRow, status: ThreadStatus) -> bool {
    let shown = self::thread(row, status, thread.attach_argv.clone());
    let changed = *thread != shown;
    *thread = shown;
    changed
}

/// The title the sidebar shows: the user's `/rename`, else the transcript's.
fn display_title(row: &ThreadRow) -> Option<String> {
    row.custom_title.clone().or_else(|| row.title.clone())
}

/// The one-line reason a session host failure carries.
fn reason(report: &Report<SessionHostError>) -> String {
    report
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| "claude failed".to_owned())
}

/// How a saved thread looks with `status`, attached to with `attach_argv`.
fn thread(row: &ThreadRow, status: ThreadStatus, attach_argv: Vec<OsString>) -> Thread {
    Thread {
        id: row.id,
        title: display_title(row),
        cwd: row.cwd.clone(),
        transcript: row.transcript_path.clone(),
        status,
        turn_started_at: row.turn_started_at.map(from_ms),
        attach_argv,
        branch: row.branch.clone(),
        pinned_at: row.pinned_at.map(from_ms),
        settled_at: row
            .settled_at
            .filter(|_| row.settled_override == Some(SettledOverride::Settled))
            .map(from_ms),
        active_since: from_ms(row.created_at.max(row.unsettled_at.unwrap_or(0))),
        last_activity_at: from_ms(row.last_activity_at),
        unseen: row.last_activity_at > row.last_visited_at,
    }
}

/// How a saved thread looks before its first poll.
fn unpolled(host: &SessionHostService, row: &ThreadRow) -> Thread {
    thread(row, ThreadStatus::Unknown, host.attach_argv(&row.short_id))
}

/// The branch checked out in `cwd`, if git can tell.
fn current_branch(git: &GitService, cwd: &Path) -> Option<String> {
    git.refs(cwd)
        .ok()?
        .into_iter()
        .find(|git_ref| git_ref.current)
        .map(|git_ref| git_ref.name)
}

/// How project `project_id`'s draft is saved.
fn draft_row(project_id: ProjectId, draft: &Draft) -> DraftRow {
    DraftRow {
        project_id,
        workspace: draft.workspace.clone(),
        branch: draft.branch.clone(),
        model: draft.model.clone(),
        permission_mode: draft.permission.clone(),
        created_at: to_ms(draft.created_at),
    }
}

/// The branch a draft in `workspace` of the project at `root` shows: the one
/// checked out in its directory, or for a new worktree the default branch.
/// `None` when git can't tell.
fn draft_branch(git: &GitService, root: &Path, workspace: &DraftWorkspace) -> Option<String> {
    match workspace {
        DraftWorkspace::Local => current_branch(git, root),
        DraftWorkspace::Existing(path) => current_branch(git, path),
        DraftWorkspace::NewWorktree => git.default_branch(root).ok(),
    }
}

/// `draft` of the project at `root` with what git now says about it: whether
/// the root is in a repository, its branch when it has none (see
/// [`draft_branch`]), and for a new worktree the ref it would start from
/// (see [`start_point`]), judged by what origin had at the last fetch.
fn with_git(git: &GitService, root: &Path, draft: Draft) -> Draft {
    let repo = git.refs(root).is_ok();
    let branch = match draft.branch {
        Some(branch) => Some(branch),
        None if repo => draft_branch(git, root, &draft.workspace),
        None => None,
    };
    let from = match (&draft.workspace, &branch) {
        (DraftWorkspace::NewWorktree, Some(base)) if repo => {
            start_point(git, root, base, |b| Ok(git.has_remote_branch(root, b))).ok()
        }
        _ => None,
    };
    Draft {
        branch,
        repo,
        from,
        ..draft
    }
}

/// The ref a new worktree of `repo` starts from for `base`: `origin/<b>` when
/// the repository has an origin and `origin_has` says it has `b` (a fetch at
/// Start, a look at the last fetch for the draft form), else `base` as is.
/// Only `origin/<b>`, a name without `/`, or an existing local branch is
/// looked for on origin; another remote's ref, like `upstream/x`, is used as
/// is.
fn start_point<F>(
    git: &GitService,
    repo: &Path,
    base: &str,
    origin_has: F,
) -> Result<String, Report<GitError>>
where
    F: FnOnce(&str) -> Result<bool, Report<GitError>>,
{
    let on_origin = match base.strip_prefix("origin/") {
        Some(branch) => Some(branch),
        None if !base.contains('/') || git.branch_exists(repo, base) => Some(base),
        None => None,
    };
    Ok(match on_origin {
        Some(branch) if git.has_origin(repo) && origin_has(branch)? => format!("origin/{branch}"),
        _ => base.to_owned(),
    })
}

/// How a saved draft looks, before git is asked about it.
fn draft_of(row: &DraftRow) -> Draft {
    Draft {
        workspace: row.workspace.clone(),
        branch: row.branch.clone(),
        model: row.model.clone(),
        permission: row.permission_mode.clone(),
        created_at: from_ms(row.created_at),
        repo: true,
        from: None,
    }
}

/// A project's title: its directory's name, else the whole path.
fn project_title(root: &Path) -> String {
    root.file_name().map_or_else(
        || root.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Milliseconds since the Unix epoch, now.
fn now_ms() -> i64 {
    to_ms(SystemTime::now())
}

/// Milliseconds since the Unix epoch at `time`; before the epoch clamps to 0.
fn to_ms(time: SystemTime) -> i64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        })
}

/// Milliseconds since the Unix epoch as a time; negative clamps to the epoch.
fn from_ms(ms: i64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(u64::try_from(ms).unwrap_or(0))
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate setup failures with `?` and assert on the outcome"
)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::{Duration, SystemTime};

    use async_trait::async_trait;
    use error_stack::{Report, ResultExt};

    use super::{FAST_POLL, SLOW_POLL, SessionsActor, SessionsActorDeps, now_ms};
    use crate::Focus;
    use crate::command::Workspace;
    use crate::common::{Services, State};
    use crate::feat::git::git_service::{Git, GitError, GitRef, GitService};
    use crate::feat::git::validator::BUSY_DIRECTORY;
    use crate::feat::git::worktree::hex_branch;
    use crate::feat::sessions::session_host::{
        CreatedSession, SessionHost, SessionHostError, SessionHostService, SessionOptions,
        SessionRecord, WorkspaceUntrusted,
    };
    use crate::feat::sessions::state::{
        Draft, DraftWorkspace, ProjectId, SidebarItem, SidebarRow, Thread, ThreadId, ThreadStatus,
    };
    use crate::feat::sessions::store::{
        DraftRow, LastUsed, LastWorkspace, NewThread, SettledOverride, Store, StoreError,
        ThreadRow, Ui,
    };
    use crate::feat::sessions::transcript::transcript_path;

    const PROJECT_ROOT: &str = "/tmp/orb";
    /// The branch checked out wherever the fake git lists refs.
    const CURRENT_BRANCH: &str = "dev";
    const NO_CLAUDE_DIR: &str = "/nonexistent/claude";
    const WORKTREES_ROOT: &str = "/nonexistent/worktrees";
    /// Why git refuses outside a repository.
    const NOT_A_REPO: &str = "fatal: not a git repository";
    const UNTRUSTED: &str =
        "Workspace not trusted. Run `claude` in /tmp/orb once and accept the trust prompt";
    const HOUR_MS: i64 = 60 * 60 * 1000;
    const DAY_MS: i64 = 24 * HOUR_MS;

    /// A session host whose answers the test scripts.
    struct FakeHost {
        list: Mutex<Result<Vec<SessionRecord>, String>>,
        create: Result<String, String>,
        remove: Result<(), String>,
        /// The sessions `stop` was called on, in order.
        stopped: Mutex<Vec<String>>,
        /// The sessions `remove` was called on, in order.
        removed: Mutex<Vec<String>>,
        /// The directories `create` was called in, in order.
        created_in: Mutex<Vec<PathBuf>>,
        /// The options `create` was called with, in order.
        created_with: Mutex<Vec<SessionOptions>>,
        /// How many more creates refuse an untrusted directory before
        /// `create` answers.
        untrusted: Mutex<u32>,
    }

    impl FakeHost {
        /// Lists `records`; stops and removes succeed.
        fn answering(records: Vec<SessionRecord>) -> Self {
            Self {
                list: Mutex::new(Ok(records)),
                create: Err("no create scripted".to_owned()),
                remove: Ok(()),
                stopped: Mutex::default(),
                removed: Mutex::default(),
                created_in: Mutex::default(),
                created_with: Mutex::default(),
                untrusted: Mutex::default(),
            }
        }

        fn listing(records: Vec<SessionRecord>) -> Arc<Self> {
            Arc::new(Self::answering(records))
        }

        fn creating(create: Result<&str, &str>) -> Arc<Self> {
            Arc::new(Self {
                create: create.map(str::to_owned).map_err(str::to_owned),
                ..Self::answering(Vec::new())
            })
        }

        /// Lists the thread `aa` idle and answers creates with `create`.
        fn moving(create: Result<&str, &str>) -> Arc<Self> {
            Arc::new(Self {
                create: create.map(str::to_owned).map_err(str::to_owned),
                ..Self::answering(vec![record("aa", ThreadStatus::Idle)])
            })
        }

        /// Lists the thread `aa` idle, refuses the first `times` creates as
        /// untrusted, then answers with `create`.
        fn untrusted(times: u32, create: Result<&str, &str>) -> Arc<Self> {
            Arc::new(Self {
                create: create.map(str::to_owned).map_err(str::to_owned),
                untrusted: Mutex::new(times),
                ..Self::answering(vec![record("aa", ThreadStatus::Idle)])
            })
        }

        fn refusing_remove(records: Vec<SessionRecord>, reason: &str) -> Arc<Self> {
            Arc::new(Self {
                remove: Err(reason.to_owned()),
                ..Self::answering(records)
            })
        }

        fn stopped(&self) -> Vec<String> {
            self.stopped
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn removed(&self) -> Vec<String> {
            self.removed
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn created_in(&self) -> Vec<PathBuf> {
            self.created_in
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn created_with(&self) -> Vec<SessionOptions> {
            self.created_with
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn set_list(&self, list: Result<Vec<SessionRecord>, String>) {
            *self.list.lock().unwrap_or_else(PoisonError::into_inner) = list;
        }
    }

    #[async_trait]
    impl SessionHost for FakeHost {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn create(
            &self,
            cwd: &Path,
            options: &SessionOptions,
        ) -> Result<CreatedSession, Report<SessionHostError>> {
            self.created_in
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(cwd.to_owned());
            self.created_with
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(options.clone());
            {
                let mut untrusted = self
                    .untrusted
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                if *untrusted > 0 {
                    *untrusted -= 1;
                    return Err(Report::new(SessionHostError)
                        .attach_opaque(WorkspaceUntrusted)
                        .attach(UNTRUSTED.to_owned()));
                }
            }
            self.create
                .clone()
                .map(|short_id| CreatedSession { short_id })
                .map_err(|reason| Report::new(SessionHostError).attach(reason))
        }

        async fn list(&self) -> Result<Vec<SessionRecord>, Report<SessionHostError>> {
            self.list
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
                .map_err(|reason| Report::new(SessionHostError).attach(reason))
        }

        async fn stop(&self, short_id: &str) -> Result<(), Report<SessionHostError>> {
            self.stopped
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(short_id.to_owned());
            Ok(())
        }

        async fn remove(&self, short_id: &str) -> Result<(), Report<SessionHostError>> {
            self.removed
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(short_id.to_owned());
            self.remove
                .clone()
                .map_err(|reason| Report::new(SessionHostError).attach(reason))
        }

        fn attach_argv(&self, short_id: &str) -> Vec<OsString> {
            vec![OsString::from(short_id)]
        }
    }

    /// A git call the actor made.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum GitCall {
        Fetch(String),
        AddWorktree {
            path: PathBuf,
            branch: String,
            base: String,
        },
        RemoveWorktree {
            path: PathBuf,
            force: bool,
        },
        DeleteBranch {
            branch: String,
            force: bool,
        },
        RenameBranch {
            old: String,
            new: String,
        },
        Init(PathBuf),
    }

    /// A git whose default branch is `main`, with [`CURRENT_BRANCH`] checked
    /// out everywhere, whose answers the test scripts, and that records the
    /// calls that change a repository.
    struct FakeGit {
        origin: bool,
        fetch: Result<bool, String>,
        /// The one branch that already exists.
        existing: Option<String>,
        /// Whether the project is a git repository before any `init`.
        repo: bool,
        /// How `init` answers.
        init: Result<(), String>,
        /// The branches origin had at the last fetch.
        remote: Vec<String>,
        calls: Mutex<Vec<GitCall>>,
    }

    impl FakeGit {
        /// A repository without an `origin`.
        fn answering() -> Self {
            Self {
                origin: false,
                fetch: Ok(true),
                existing: None,
                repo: true,
                init: Ok(()),
                remote: Vec::new(),
                calls: Mutex::default(),
            }
        }

        /// A repository without an `origin`.
        fn local() -> Arc<Self> {
            Arc::new(Self::answering())
        }

        /// A repository with an `origin` that answers fetches with `fetch`.
        fn with_origin(fetch: Result<bool, &str>) -> Arc<Self> {
            Arc::new(Self {
                origin: true,
                fetch: fetch.map_err(str::to_owned),
                ..Self::answering()
            })
        }

        /// A repository with an `origin` that had `branches` at the last
        /// fetch.
        fn tracking(branches: &[&str]) -> Arc<Self> {
            Arc::new(Self {
                origin: true,
                remote: branches.iter().map(|&branch| branch.to_owned()).collect(),
                ..Self::answering()
            })
        }

        /// A repository without an `origin` where `branch` already exists.
        fn having(branch: &str) -> Arc<Self> {
            Arc::new(Self {
                existing: Some(branch.to_owned()),
                ..Self::answering()
            })
        }

        /// A directory that isn't a git repository until `init` answers
        /// `init`.
        fn plain(init: Result<(), &str>) -> Arc<Self> {
            Arc::new(Self {
                repo: false,
                init: init.map_err(str::to_owned),
                ..Self::answering()
            })
        }

        /// Whether the directory is a repository now.
        fn is_repo(&self) -> bool {
            self.repo
                || (self.init.is_ok()
                    && self
                        .calls()
                        .iter()
                        .any(|call| matches!(call, GitCall::Init(_))))
        }

        fn calls(&self) -> Vec<GitCall> {
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn record(&self, call: GitCall) {
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(call);
        }

        /// The branch the actor renamed, and its new name.
        fn renamed(&self) -> Option<(String, String)> {
            self.calls().into_iter().find_map(|call| match call {
                GitCall::RenameBranch { old, new } => Some((old, new)),
                _ => None,
            })
        }

        /// The worktree the actor added, and its branch.
        fn added(&self) -> Option<(PathBuf, String)> {
            self.calls().into_iter().find_map(|call| match call {
                GitCall::AddWorktree { path, branch, .. } => Some((path, branch)),
                _ => None,
            })
        }
    }

    impl Git for FakeGit {
        fn name(&self) -> &'static str {
            "fake"
        }

        fn refs(&self, _cwd: &Path) -> Result<Vec<GitRef>, Report<GitError>> {
            if !self.is_repo() {
                return Err(Report::new(GitError).attach(NOT_A_REPO.to_owned()));
            }
            Ok(vec![GitRef {
                current: true,
                ..git_ref(CURRENT_BRANCH, false)
            }])
        }

        fn default_branch(&self, _repo: &Path) -> Result<String, Report<GitError>> {
            if !self.is_repo() {
                return Err(Report::new(GitError).attach(NOT_A_REPO.to_owned()));
            }
            Ok("main".to_owned())
        }

        fn has_origin(&self, _repo: &Path) -> bool {
            self.origin
        }

        fn fetch(&self, _repo: &Path, branch: &str) -> Result<bool, Report<GitError>> {
            self.record(GitCall::Fetch(branch.to_owned()));
            self.fetch
                .clone()
                .map_err(|reason| Report::new(GitError).attach(reason))
        }

        fn add_worktree(
            &self,
            _repo: &Path,
            path: &Path,
            branch: &str,
            base: &str,
        ) -> Result<(), Report<GitError>> {
            self.record(GitCall::AddWorktree {
                path: path.to_owned(),
                branch: branch.to_owned(),
                base: base.to_owned(),
            });
            Ok(())
        }

        fn remove_worktree(
            &self,
            _repo: &Path,
            path: &Path,
            force: bool,
        ) -> Result<(), Report<GitError>> {
            self.record(GitCall::RemoveWorktree {
                path: path.to_owned(),
                force,
            });
            Ok(())
        }

        fn delete_branch(
            &self,
            _repo: &Path,
            branch: &str,
            force: bool,
        ) -> Result<(), Report<GitError>> {
            self.record(GitCall::DeleteBranch {
                branch: branch.to_owned(),
                force,
            });
            Ok(())
        }

        fn branch_exists(&self, _repo: &Path, branch: &str) -> bool {
            self.existing.as_deref() == Some(branch)
        }

        fn has_remote_branch(&self, _repo: &Path, branch: &str) -> bool {
            self.remote.iter().any(|remote| remote == branch)
        }

        fn init(&self, dir: &Path) -> Result<(), Report<GitError>> {
            self.record(GitCall::Init(dir.to_owned()));
            self.init
                .clone()
                .map_err(|reason| Report::new(GitError).attach(reason))
        }

        fn checkout(&self, _cwd: &Path, git_ref: &GitRef) -> Result<String, Report<GitError>> {
            let local = match git_ref.name.split_once('/') {
                Some((_, branch)) if git_ref.remote => branch,
                _ => &git_ref.name,
            };
            Ok(local.to_owned())
        }

        fn rename_branch(&self, _cwd: &Path, old: &str, new: &str) -> Result<(), Report<GitError>> {
            self.record(GitCall::RenameBranch {
                old: old.to_owned(),
                new: new.to_owned(),
            });
            Ok(())
        }
    }

    fn record(short_id: &str, status: ThreadStatus) -> SessionRecord {
        SessionRecord {
            short_id: short_id.to_owned(),
            session_id: None,
            status,
        }
    }

    /// A store holding one thread in the orb project, created an hour ago.
    fn store_with_thread(short_id: &str) -> Result<(Store, ThreadId), Report<StoreError>> {
        let store = Store::open_in_memory()?;
        let id = add_thread(&store, short_id, now_ms() - HOUR_MS)?;
        Ok((store, id))
    }

    /// Saves the orb project rooted at [`PROJECT_ROOT`].
    fn orb_project(store: &Store) -> Result<ProjectId, Report<StoreError>> {
        store.add_project(Path::new(PROJECT_ROOT), "orb", 0)
    }

    /// Saves a thread created at `created_at` in the orb project.
    fn add_thread(
        store: &Store,
        short_id: &str,
        created_at: i64,
    ) -> Result<ThreadId, Report<StoreError>> {
        let project_id = orb_project(store)?;
        store.insert_thread(&NewThread {
            project_id,
            short_id: short_id.to_owned(),
            cwd: PathBuf::from(PROJECT_ROOT),
            created_at,
            model: None,
            permission_mode: None,
        })
    }

    /// Saves `change` over the saved row of the thread `short_id`.
    fn resave<F>(store: &Store, short_id: &str, change: F) -> Result<(), Report<StoreError>>
    where
        F: FnOnce(ThreadRow) -> ThreadRow,
    {
        store.save_thread(&change(saved(store, short_id)?))
    }

    /// The saved row of the thread `short_id`.
    fn saved(store: &Store, short_id: &str) -> Result<ThreadRow, Report<StoreError>> {
        store
            .load()?
            .1
            .into_iter()
            .find(|row| row.short_id == short_id)
            .ok_or_else(|| Report::new(StoreError).attach(format!("{short_id} isn't saved")))
    }

    /// Starts the actor on `store` the way `on_start` does, without the ticker.
    fn start(store: Store, host: &Arc<FakeHost>, claude_dir: &Path) -> (SessionsActor, State) {
        start_with(store, host, &FakeGit::local(), claude_dir)
    }

    /// Starts the actor on `store` with `git`, making worktrees under
    /// [`WORKTREES_ROOT`].
    fn start_with(
        store: Store,
        host: &Arc<FakeHost>,
        git: &Arc<FakeGit>,
        claude_dir: &Path,
    ) -> (SessionsActor, State) {
        let state = State::default();
        let actor = SessionsActor::restore(SessionsActorDeps {
            services: Services {
                session_host: SessionHostService::new(host.clone()),
                git: GitService::new(git.clone()),
            },
            state: state.clone(),
            store,
            claude_dir: claude_dir.to_owned(),
            worktrees_root: PathBuf::from(WORKTREES_ROOT),
            wake: Arc::new(|| {}),
        });
        (actor, state)
    }

    fn error_of(state: &State) -> Option<String> {
        state.read().sessions.error.clone()
    }

    fn trust_of(state: &State) -> Option<PathBuf> {
        state.read().sessions.trust.clone()
    }

    /// The thread `id` as the sidebar shows it.
    fn shown(state: &State, id: ThreadId) -> Option<Thread> {
        state
            .read()
            .sessions
            .threads()
            .find(|thread| thread.id == id)
            .cloned()
    }

    fn status_of(state: &State, id: ThreadId) -> Option<ThreadStatus> {
        state
            .read()
            .sessions
            .threads()
            .find(|thread| thread.id == id)
            .map(|thread| thread.status)
    }

    fn stamp_of(state: &State, id: ThreadId) -> Option<SystemTime> {
        state
            .read()
            .sessions
            .threads()
            .find(|thread| thread.id == id)
            .and_then(|thread| thread.turn_started_at)
    }

    /// The orb worktree the rename tests' thread is in, and its branch.
    const HEX_WORKTREE: &str = "/nonexistent/worktrees/orb/orb-1a2b3c4d";
    const HEX_BRANCH: &str = "orb/1a2b3c4d";
    const PROMPT_LINE: &str = "{\"type\":\"user\",\"message\":{\"content\":\"Fix the parser\"}}\n";
    const AI_TITLE_LINE: &str = "{\"type\":\"ai-title\",\"aiTitle\":\"Parser fix\"}\n";

    /// A Claude directory holding `lines` as the transcript of session `s1`
    /// in [`HEX_WORKTREE`], and a store whose thread `aa` is there on `branch`.
    fn worktree_thread(
        lines: &str,
        branch: &str,
    ) -> Result<(tempfile::TempDir, Store, ThreadId), Report<StoreError>> {
        let claude_dir = tempfile::tempdir().change_context(StoreError)?;
        let path = transcript_path(claude_dir.path(), Path::new(HEX_WORKTREE), "s1");
        fs::create_dir_all(path.parent().unwrap_or(claude_dir.path()))
            .change_context(StoreError)?;
        fs::write(&path, lines).change_context(StoreError)?;
        let (store, id) = store_with_thread("aa")?;
        resave(&store, "aa", |row| ThreadRow {
            cwd: PathBuf::from(HEX_WORKTREE),
            branch: Some(branch.to_owned()),
            ..row
        })?;
        Ok((claude_dir, store, id))
    }

    /// Thread `aa`'s record in session `s1` with `status`.
    fn in_session(status: ThreadStatus) -> SessionRecord {
        SessionRecord {
            session_id: Some("s1".to_owned()),
            ..record("aa", status)
        }
    }

    /// Polls once while thread `aa` works, then once after its turn ended.
    async fn end_turn(actor: &mut SessionsActor, host: &FakeHost) {
        host.set_list(Ok(vec![in_session(ThreadStatus::Working)]));
        actor.poll().await;
        host.set_list(Ok(vec![in_session(ThreadStatus::Idle)]));
        actor.poll().await;
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_shows_the_status_of_each_threads_record() -> Result<(), Report<StoreError>> {
        // Given a saved thread and a host reporting it waiting for an approval.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::NeedsApproval)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When polling.
        actor.poll().await;

        // Then the thread shows that status.
        assert_eq!(
            status_of(&state, id),
            Some(ThreadStatus::NeedsApproval),
            "the thread should show its record's status"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_prefers_the_live_record_over_a_stale_stopped_one()
    -> Result<(), Report<StoreError>> {
        // Given a host listing a stale stopped record and a live idle one for the same id.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![
            record("aa", ThreadStatus::Stopped),
            record("aa", ThreadStatus::Idle),
        ]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When polling.
        actor.poll().await;

        // Then the thread shows the live record's status.
        assert_eq!(
            status_of(&state, id),
            Some(ThreadStatus::Idle),
            "the live record should win over the stale one"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_marks_a_thread_without_a_record_gone() -> Result<(), Report<StoreError>> {
        // Given a saved thread the host doesn't list.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("bb", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When polling.
        actor.poll().await;

        // Then the thread is gone.
        assert_eq!(
            status_of(&state, id),
            Some(ThreadStatus::Gone),
            "a thread the host no longer lists should be gone"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_stamps_a_turn_when_it_starts() -> Result<(), Report<StoreError>> {
        // Given an unstamped thread the host now reports busy.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When polling.
        actor.poll().await;

        // Then the turn is stamped.
        assert!(
            stamp_of(&state, id).is_some(),
            "a turn seen running should be stamped"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_keeps_the_turn_stamp_while_the_turn_waits() -> Result<(), Report<StoreError>> {
        // Given a thread a poll saw running.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;
        let stamp = stamp_of(&state, id)
            .ok_or_else(|| Report::new(StoreError).attach("the busy poll didn't stamp the turn"))?;

        // When a poll sees it waiting for input.
        host.set_list(Ok(vec![record("aa", ThreadStatus::NeedsInput)]));
        actor.poll().await;

        // Then the stamp is unchanged.
        assert_eq!(
            stamp_of(&state, id),
            Some(stamp),
            "waiting is part of the same turn"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_clears_the_turn_stamp_when_the_turn_ends() -> Result<(), Report<StoreError>> {
        // Given a thread a poll saw running.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When a poll sees it idle.
        host.set_list(Ok(vec![record("aa", ThreadStatus::Idle)]));
        actor.poll().await;

        // Then the stamp is cleared.
        assert_eq!(
            stamp_of(&state, id),
            None,
            "an idle thread has no turn running"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_keeps_an_earlier_error() -> Result<(), Report<StoreError>> {
        // Given a failure on the mode line.
        let (store, _) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        state.write().sessions.error = Some("Claude is working in this directory".to_owned());

        // When a poll succeeds.
        actor.poll().await;

        // Then the failure still shows.
        assert_eq!(
            error_of(&state).as_deref(),
            Some("Claude is working in this directory"),
            "a poll shouldn't hide an error the user hasn't seen"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_keeps_a_saved_turn_stamp() -> Result<(), Report<StoreError>> {
        // Given a thread saved mid-turn, stamped 1 s after the epoch, still busy.
        let (store, id) = store_with_thread("aa")?;
        let row = ThreadRow {
            turn_started_at: Some(1_000),
            ..saved(&store, "aa")?
        };
        store.save_thread(&row)?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When polling after the relaunch.
        actor.poll().await;

        // Then the saved stamp is kept.
        assert_eq!(
            stamp_of(&state, id),
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
            "a relaunch mid-turn should keep the saved stamp"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_saves_a_new_turn_stamp() -> Result<(), Report<StoreError>> {
        // Given an unstamped thread the host now reports busy.
        let (store, _) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When polling.
        actor.poll().await;

        // Then the stamp is saved.
        assert!(
            saved(&actor.store, "aa")?.turn_started_at.is_some(),
            "the stamp should survive a relaunch"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_waits_five_seconds_when_nothing_is_underway() -> Result<(), Report<StoreError>> {
        // Given an idle thread and orb detached.
        let (store, _) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When polling.
        let next = actor.poll().await;

        // Then the next poll is in 5 s.
        assert_eq!(next, SLOW_POLL, "nothing to watch closely");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_waits_one_second_while_a_thread_is_busy() -> Result<(), Report<StoreError>> {
        // Given a thread the host reports busy.
        let (store, _) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When polling.
        let next = actor.poll().await;

        // Then the next poll is in 1 s.
        assert_eq!(next, FAST_POLL, "a running turn is watched closely");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_waits_one_second_while_attached() -> Result<(), Report<StoreError>> {
        // Given an idle thread and orb attached.
        let (store, _) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        state.write().focus = Focus::Attached;

        // When polling.
        let next = actor.poll().await;

        // Then the next poll is in 1 s.
        assert_eq!(next, FAST_POLL, "an attached user may start a turn");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_poll_keeps_the_last_statuses() -> Result<(), Report<StoreError>> {
        // Given a thread a poll saw idle.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When the next poll fails.
        host.set_list(Err("claude: command not found".to_owned()));
        actor.poll().await;

        // Then the thread still shows idle.
        assert_eq!(
            status_of(&state, id),
            Some(ThreadStatus::Idle),
            "a failed poll shouldn't change statuses"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_poll_shows_the_reason() -> Result<(), Report<StoreError>> {
        // Given a host that can't list sessions.
        let (store, _) = store_with_thread("aa")?;
        let host = FakeHost::listing(Vec::new());
        host.set_list(Err("claude: command not found".to_owned()));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When polling.
        actor.poll().await;

        // Then the reason is the error.
        assert_eq!(
            state.read().sessions.error.as_deref(),
            Some("claude: command not found"),
            "the mode line should show why the poll failed"
        );
        Ok(())
    }

    /// A draft of the project in `workspace` with Claude's defaults and no
    /// branch, created at 1 s.
    fn draft_row(project_id: ProjectId, workspace: DraftWorkspace) -> DraftRow {
        DraftRow {
            project_id,
            workspace,
            branch: None,
            model: None,
            permission_mode: None,
            created_at: 1_000,
        }
    }

    /// A store holding the orb project with `draft` saved for it.
    fn store_with_draft<F>(draft: F) -> Result<(Store, ProjectId), Report<StoreError>>
    where
        F: FnOnce(ProjectId) -> DraftRow,
    {
        let store = Store::open_in_memory()?;
        let id = orb_project(&store)?;
        store.save_draft(&draft(id))?;
        Ok((store, id))
    }

    /// Project `id`'s draft as the sidebar shows it.
    fn shown_draft(state: &State, id: ProjectId) -> Option<Draft> {
        state
            .read()
            .sessions
            .projects
            .iter()
            .find(|project| project.id == id)
            .and_then(|project| project.draft.clone())
    }

    /// Project `id`'s saved draft.
    fn saved_draft(store: &Store, id: ProjectId) -> Result<Option<DraftRow>, Report<StoreError>> {
        Ok(store
            .load()?
            .2
            .into_iter()
            .find(|draft| draft.project_id == id))
    }

    /// Settings used last with `workspace`, sonnet and plan mode.
    fn used(workspace: LastWorkspace) -> LastUsed {
        LastUsed {
            workspace,
            model: Some("sonnet".to_owned()),
            permission_mode: Some("plan".to_owned()),
        }
    }

    /// The fetches and worktree adds git saw, in order.
    fn fetch_and_add_steps(git: &FakeGit) -> Vec<String> {
        git.calls()
            .into_iter()
            .filter_map(|call| match call {
                GitCall::Fetch(branch) => Some(format!("fetch {branch}")),
                GitCall::AddWorktree { base, .. } => Some(format!("add from {base}")),
                _ => None,
            })
            .collect()
    }

    #[rstest::rstest]
    fn create_draft_prefills_the_projects_last_used_settings() -> Result<(), Report<StoreError>> {
        // Given a project last started in a new worktree with sonnet in plan mode.
        let store = Store::open_in_memory()?;
        let id = orb_project(&store)?;
        store.record_last_used(id, &used(LastWorkspace::NewWorktree), 10)?;
        let (mut actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When creating its draft.
        actor.create_draft(id);

        // Then the draft has that workspace, model and permission.
        let settings =
            shown_draft(&state, id).map(|draft| (draft.workspace, draft.model, draft.permission));
        assert_eq!(
            settings,
            Some((
                DraftWorkspace::NewWorktree,
                Some("sonnet".to_owned()),
                Some("plan".to_owned())
            )),
            "the draft should take the project's last-used settings"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn create_draft_in_a_new_project_takes_the_latest_model_and_permission()
    -> Result<(), Report<StoreError>> {
        // Given orb last started in a new worktree with sonnet in plan mode,
        // and web never started.
        let store = Store::open_in_memory()?;
        let orb = orb_project(&store)?;
        store.record_last_used(orb, &used(LastWorkspace::NewWorktree), 10)?;
        let web = store.add_project(Path::new("/tmp/web"), "web", 0)?;
        let (mut actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When creating web's draft.
        actor.create_draft(web);

        // Then it has orb's model and permission in a local checkout.
        let settings =
            shown_draft(&state, web).map(|draft| (draft.workspace, draft.model, draft.permission));
        assert_eq!(
            settings,
            Some((
                DraftWorkspace::Local,
                Some("sonnet".to_owned()),
                Some("plan".to_owned())
            )),
            "a new project should take the latest model and permission, locally"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn remembered_previous_worktree_without_one_falls_back_to_local()
    -> Result<(), Report<StoreError>> {
        // Given a project last started in a previous worktree, with no thread
        // in a worktree now.
        let (store, _) = store_with_thread("aa")?;
        let id = orb_project(&store)?;
        store.record_last_used(id, &used(LastWorkspace::Previous), 10)?;
        let (mut actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When creating its draft.
        actor.create_draft(id);

        // Then the draft is a local checkout.
        assert_eq!(
            shown_draft(&state, id).map(|draft| draft.workspace),
            Some(DraftWorkspace::Local),
            "a previous worktree the project lacks should fall back to local"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn remembered_previous_worktree_prefills_it_with_its_branch() -> Result<(), Report<StoreError>>
    {
        // Given a project last started in a previous worktree, and a thread
        // in the worktree on feat.
        let (store, _) = store_with_thread("aa")?;
        resave(&store, "aa", |row| ThreadRow {
            cwd: PathBuf::from(HEX_WORKTREE),
            branch: Some("feat".to_owned()),
            ..row
        })?;
        let id = orb_project(&store)?;
        store.record_last_used(id, &used(LastWorkspace::Previous), 10)?;
        let (mut actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When creating its draft.
        actor.create_draft(id);

        // Then the draft is in that worktree, on feat.
        let workspace = shown_draft(&state, id).map(|draft| (draft.workspace, draft.branch));
        assert_eq!(
            workspace,
            Some((
                DraftWorkspace::Existing(PathBuf::from(HEX_WORKTREE)),
                Some("feat".to_owned())
            )),
            "the draft should reopen the previous worktree on its branch"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_worktree_draft_branch_is_the_default_branch() -> Result<(), Report<StoreError>> {
        // Given a project last started in a new worktree, whose default branch is main.
        let store = Store::open_in_memory()?;
        let id = orb_project(&store)?;
        store.record_last_used(id, &used(LastWorkspace::NewWorktree), 10)?;
        let (mut actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When creating its draft.
        actor.create_draft(id);

        // Then the draft's base branch is main.
        assert_eq!(
            shown_draft(&state, id)
                .and_then(|draft| draft.branch)
                .as_deref(),
            Some("main"),
            "a new worktree should start from the default branch"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn local_draft_branch_is_the_roots_current_branch() -> Result<(), Report<StoreError>> {
        // Given a never-started project whose root is on dev.
        let store = Store::open_in_memory()?;
        let id = orb_project(&store)?;
        let (mut actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When creating its draft.
        actor.create_draft(id);

        // Then the local draft shows dev.
        assert_eq!(
            shown_draft(&state, id)
                .and_then(|draft| draft.branch)
                .as_deref(),
            Some(CURRENT_BRANCH),
            "a local draft should show the root's current branch"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn created_draft_is_saved() -> Result<(), Report<StoreError>> {
        // Given a project without a draft.
        let store = Store::open_in_memory()?;
        let id = orb_project(&store)?;
        let (mut actor, _state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When creating its draft.
        actor.create_draft(id);

        // Then the store holds a local draft on dev.
        let saved = saved_draft(&actor.store, id)?.map(|draft| (draft.workspace, draft.branch));
        assert_eq!(
            saved,
            Some((DraftWorkspace::Local, Some(CURRENT_BRANCH.to_owned()))),
            "the new draft should be saved"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn create_draft_keeps_an_existing_draft() -> Result<(), Report<StoreError>> {
        // Given a project with an opus draft, last started with sonnet.
        let (store, id) = store_with_draft(|id| DraftRow {
            model: Some("opus".to_owned()),
            ..draft_row(id, DraftWorkspace::Local)
        })?;
        store.record_last_used(id, &used(LastWorkspace::Local), 10)?;
        let (mut actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When asked to create its draft again.
        actor.create_draft(id);

        // Then the opus draft stays.
        assert_eq!(
            shown_draft(&state, id)
                .and_then(|draft| draft.model)
                .as_deref(),
            Some("opus"),
            "an existing draft should not be replaced"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn save_draft_saves_the_drafts_edits() -> Result<(), Report<StoreError>> {
        // Given a local draft the user switched to haiku.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let (mut actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );
        if let Some(draft) = state.write().sessions.draft_mut(id) {
            draft.model = Some("haiku".to_owned());
        }

        // When saving it.
        actor.save_draft(id);

        // Then the saved draft is on haiku.
        assert_eq!(
            saved_draft(&actor.store, id)?
                .and_then(|draft| draft.model)
                .as_deref(),
            Some("haiku"),
            "the draft's edit should be saved"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn save_draft_fills_in_a_missing_branch() -> Result<(), Report<StoreError>> {
        // Given a new-worktree draft without a base branch.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::NewWorktree))?;
        let (mut actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When saving it after its branch was cleared.
        if let Some(draft) = state.write().sessions.draft_mut(id) {
            draft.branch = None;
        }
        actor.save_draft(id);

        // Then its base branch is the default branch.
        assert_eq!(
            shown_draft(&state, id)
                .and_then(|draft| draft.branch)
                .as_deref(),
            Some("main"),
            "a saved new-worktree draft should get the default branch"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn picking_new_worktree_for_a_draft_adds_no_worktree() -> Result<(), Report<StoreError>> {
        // Given a local draft the user switched to a new worktree.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let git = FakeGit::local();
        let (mut actor, state) = start_with(
            store,
            &FakeHost::listing(Vec::new()),
            &git,
            Path::new(NO_CLAUDE_DIR),
        );
        if let Some(draft) = state.write().sessions.draft_mut(id) {
            draft.workspace = DraftWorkspace::NewWorktree;
            draft.branch = None;
        }

        // When saving it.
        actor.save_draft(id);

        // Then no worktree is added before Start.
        assert_eq!(
            git.added(),
            None,
            "a new-worktree draft should add its worktree only when started"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn checkout_draft_shows_the_branch_on_the_local_draft() -> Result<(), Report<StoreError>> {
        // Given a local draft.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let (mut actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When checking out origin/feat for it.
        actor.checkout_draft(id, &git_ref("origin/feat", true), Path::new(PROJECT_ROOT));

        // Then the draft shows the local branch feat.
        assert_eq!(
            shown_draft(&state, id)
                .and_then(|draft| draft.branch)
                .as_deref(),
            Some("feat"),
            "the draft should show the branch checked out in the root"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn checkout_draft_saves_the_drafts_branch() -> Result<(), Report<StoreError>> {
        // Given a local draft.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let (mut actor, _state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When checking out feat for it.
        actor.checkout_draft(id, &git_ref("feat", false), Path::new(PROJECT_ROOT));

        // Then the saved draft is on feat.
        assert_eq!(
            saved_draft(&actor.store, id)?
                .and_then(|draft| draft.branch)
                .as_deref(),
            Some("feat"),
            "the draft's new branch should be saved"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn checkout_draft_refused_while_the_root_is_busy() -> Result<(), Report<StoreError>> {
        // Given a local draft and a thread working in the root.
        let (store, _) = store_with_thread("aa")?;
        let id = orb_project(&store)?;
        store.save_draft(&draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When checking out feat for the draft.
        actor.checkout_draft(id, &git_ref("feat", false), Path::new(PROJECT_ROOT));

        // Then the mode line says Claude is working there.
        assert_eq!(
            error_of(&state).as_deref(),
            Some(BUSY_DIRECTORY),
            "a checkout under a running turn should be refused"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn refused_checkout_keeps_the_drafts_branch() -> Result<(), Report<StoreError>> {
        // Given a local draft on main and a thread working in the root.
        let (store, _) = store_with_thread("aa")?;
        let id = orb_project(&store)?;
        store.save_draft(&DraftRow {
            branch: Some("main".to_owned()),
            ..draft_row(id, DraftWorkspace::Local)
        })?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When checking out feat for the draft.
        actor.checkout_draft(id, &git_ref("feat", false), Path::new(PROJECT_ROOT));

        // Then the draft is still on main.
        assert_eq!(
            shown_draft(&state, id)
                .and_then(|draft| draft.branch)
                .as_deref(),
            Some("main"),
            "a refused checkout should leave the draft's branch"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn thread_checkout_in_the_root_updates_the_local_draft() -> Result<(), Report<StoreError>> {
        // Given a thread in the root and a local draft on main.
        let (store, thread) = store_with_thread("aa")?;
        let id = orb_project(&store)?;
        store.save_draft(&DraftRow {
            branch: Some("main".to_owned()),
            ..draft_row(id, DraftWorkspace::Local)
        })?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When switching the thread to feat.
        actor.check_out(thread, &git_ref("feat", false));

        // Then the draft follows the root onto feat.
        assert_eq!(
            shown_draft(&state, id)
                .and_then(|draft| draft.branch)
                .as_deref(),
            Some("feat"),
            "a local draft should follow checkouts in the root"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn thread_checkout_in_the_root_leaves_a_new_worktree_draft() -> Result<(), Report<StoreError>> {
        // Given a thread in the root and a new-worktree draft based on main.
        let (store, thread) = store_with_thread("aa")?;
        let id = orb_project(&store)?;
        store.save_draft(&DraftRow {
            branch: Some("main".to_owned()),
            ..draft_row(id, DraftWorkspace::NewWorktree)
        })?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When switching the thread to feat.
        actor.check_out(thread, &git_ref("feat", false));

        // Then the draft keeps its base main.
        assert_eq!(
            shown_draft(&state, id)
                .and_then(|draft| draft.branch)
                .as_deref(),
            Some("main"),
            "a new-worktree draft's base should not follow the root"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn discard_forgets_the_draft() -> Result<(), Report<StoreError>> {
        // Given a project with a draft.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let (mut actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When discarding it.
        actor.discard_draft(id);

        // Then the project has no draft.
        assert_eq!(
            shown_draft(&state, id),
            None,
            "a discarded draft should leave the sidebar"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn discard_deletes_the_saved_draft() -> Result<(), Report<StoreError>> {
        // Given a project with a draft.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let (mut actor, _state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When discarding it.
        actor.discard_draft(id);

        // Then no draft is saved.
        assert_eq!(
            saved_draft(&actor.store, id)?,
            None,
            "a discarded draft should be deleted from the store"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn discard_asks_nothing_of_the_session_host() -> Result<(), Report<StoreError>> {
        // Given a project with a draft.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When discarding it.
        actor.discard_draft(id);

        // Then no session was created, stopped or removed.
        assert!(
            host.created_in().is_empty() && host.stopped().is_empty() && host.removed().is_empty(),
            "a draft has no session to remove"
        );
        Ok(())
    }

    /// Project `id`'s draft as the sidebar shows it: its workspace and branch.
    fn draft_place(state: &State, id: ProjectId) -> Option<(DraftWorkspace, Option<String>)> {
        shown_draft(state, id).map(|draft| (draft.workspace, draft.branch))
    }

    /// The worktree the existing-worktree draft tests' draft is in.
    const DRAFT_WORKTREE: &str = "/tmp/orb-wt";

    #[rstest::rstest]
    fn checkout_draft_in_its_worktree_shows_the_branch_on_the_draft()
    -> Result<(), Report<StoreError>> {
        // Given a draft in an existing worktree on feat.
        let (store, id) = store_with_draft(|id| DraftRow {
            branch: Some("feat".to_owned()),
            ..draft_row(id, DraftWorkspace::Existing(DRAFT_WORKTREE.into()))
        })?;
        let (mut actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When checking out dev in that worktree for it.
        actor.checkout_draft(id, &git_ref("dev", false), Path::new(DRAFT_WORKTREE));

        // Then the draft stays in the worktree, on dev.
        assert_eq!(
            draft_place(&state, id),
            Some((
                DraftWorkspace::Existing(DRAFT_WORKTREE.into()),
                Some("dev".to_owned())
            )),
            "the draft should show the branch checked out in its worktree"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn checkout_draft_in_the_root_moves_an_existing_worktree_draft_there()
    -> Result<(), Report<StoreError>> {
        // Given a draft in an existing worktree on feat.
        let (store, id) = store_with_draft(|id| DraftRow {
            branch: Some("feat".to_owned()),
            ..draft_row(id, DraftWorkspace::Existing(DRAFT_WORKTREE.into()))
        })?;
        let (mut actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When checking out the default branch main in the root for it.
        actor.checkout_draft(id, &git_ref("main", false), Path::new(PROJECT_ROOT));

        // Then the draft is a local draft on main.
        assert_eq!(
            draft_place(&state, id),
            Some((DraftWorkspace::Local, Some("main".to_owned()))),
            "a checkout in the root should take the draft back there"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn checkout_draft_in_the_root_saves_the_draft_as_local() -> Result<(), Report<StoreError>> {
        // Given a draft in an existing worktree on feat.
        let (store, id) = store_with_draft(|id| DraftRow {
            branch: Some("feat".to_owned()),
            ..draft_row(id, DraftWorkspace::Existing(DRAFT_WORKTREE.into()))
        })?;
        let (mut actor, _state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When checking out main in the root for it.
        actor.checkout_draft(id, &git_ref("main", false), Path::new(PROJECT_ROOT));

        // Then the saved draft is local, on main.
        assert_eq!(
            saved_draft(&actor.store, id)?.map(|draft| (draft.workspace, draft.branch)),
            Some((DraftWorkspace::Local, Some("main".to_owned()))),
            "the draft's move to the root should be saved"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn checkout_draft_refused_while_its_worktree_is_busy() -> Result<(), Report<StoreError>> {
        // Given a draft in the worktree where thread `aa` is working.
        let (store, _) = store_with_thread("aa")?;
        resave(&store, "aa", |row| ThreadRow {
            cwd: PathBuf::from(DRAFT_WORKTREE),
            ..row
        })?;
        let id = orb_project(&store)?;
        store.save_draft(&draft_row(
            id,
            DraftWorkspace::Existing(DRAFT_WORKTREE.into()),
        ))?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When checking out dev in the worktree for the draft.
        actor.checkout_draft(id, &git_ref("dev", false), Path::new(DRAFT_WORKTREE));

        // Then the mode line says Claude is working there.
        assert_eq!(
            error_of(&state).as_deref(),
            Some(BUSY_DIRECTORY),
            "a checkout under a running turn should be refused"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn thread_checkout_in_a_worktree_updates_the_draft_there() -> Result<(), Report<StoreError>> {
        // Given thread `aa` in a worktree and a draft in the same worktree on
        // feat.
        let (store, thread) = store_with_thread("aa")?;
        resave(&store, "aa", |row| ThreadRow {
            cwd: PathBuf::from(DRAFT_WORKTREE),
            ..row
        })?;
        let id = orb_project(&store)?;
        store.save_draft(&DraftRow {
            branch: Some("feat".to_owned()),
            ..draft_row(id, DraftWorkspace::Existing(DRAFT_WORKTREE.into()))
        })?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When switching the thread to dev.
        actor.check_out(thread, &git_ref("dev", false));

        // Then the draft follows its worktree onto dev.
        assert_eq!(
            shown_draft(&state, id).and_then(|draft| draft.branch),
            Some("dev".to_owned()),
            "a draft should follow checkouts in its worktree"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case::fetched_from_origin(&["main"], "origin/main")]
    #[case::not_on_origin(&[], "main")]
    fn new_worktree_draft_says_where_it_starts_from(
        #[case] on_origin: &[&str],
        #[case] from: &str,
    ) -> Result<(), Report<StoreError>> {
        // Given a project with an origin that had `on_origin` at the last
        // fetch, last started in a new worktree.
        let store = Store::open_in_memory()?;
        let id = orb_project(&store)?;
        store.record_last_used(id, &used(LastWorkspace::NewWorktree), 10)?;
        let (mut actor, state) = start_with(
            store,
            &FakeHost::listing(Vec::new()),
            &FakeGit::tracking(on_origin),
            Path::new(NO_CLAUDE_DIR),
        );

        // When creating its draft, based on the default branch main.
        actor.create_draft(id);

        // Then it starts from `from`.
        assert_eq!(
            shown_draft(&state, id).and_then(|draft| draft.from),
            Some(from.to_owned()),
            "the ref the new worktree would start from"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_worktree_draft_without_origin_starts_from_its_base() -> Result<(), Report<StoreError>> {
        // Given a new-worktree draft based on main in a repository without an
        // origin.
        let (store, id) = store_with_draft(|id| DraftRow {
            branch: Some("main".to_owned()),
            ..draft_row(id, DraftWorkspace::NewWorktree)
        })?;
        let (mut actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When saving it.
        actor.save_draft(id);

        // Then it starts from main as it is.
        assert_eq!(
            shown_draft(&state, id).and_then(|draft| draft.from),
            Some("main".to_owned()),
            "without an origin the base is used as it is"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn draft_of_a_non_git_project_is_not_a_repository() -> Result<(), Report<StoreError>> {
        // Given a project whose root isn't a git repository.
        let store = Store::open_in_memory()?;
        let id = orb_project(&store)?;
        let (mut actor, state) = start_with(
            store,
            &FakeHost::listing(Vec::new()),
            &FakeGit::plain(Ok(())),
            Path::new(NO_CLAUDE_DIR),
        );

        // When creating its draft.
        actor.create_draft(id);

        // Then the draft knows it isn't in a repository.
        assert_eq!(
            shown_draft(&state, id).map(|draft| draft.repo),
            Some(false),
            "a plain directory isn't a git repository"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn restored_draft_of_a_non_git_project_is_not_a_repository() -> Result<(), Report<StoreError>> {
        // Given a saved draft of a project whose root isn't a git repository.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;

        // When the actor starts.
        let (_actor, state) = start_with(
            store,
            &FakeHost::listing(Vec::new()),
            &FakeGit::plain(Ok(())),
            Path::new(NO_CLAUDE_DIR),
        );

        // Then the draft knows it isn't in a repository.
        assert_eq!(
            shown_draft(&state, id).map(|draft| draft.repo),
            Some(false),
            "a restored draft should learn its project isn't a repository"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn init_git_runs_git_init_in_the_root() -> Result<(), Report<StoreError>> {
        // Given a local draft of a project that isn't a git repository.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let git = FakeGit::plain(Ok(()));
        let (mut actor, _state) = start_with(
            store,
            &FakeHost::listing(Vec::new()),
            &git,
            Path::new(NO_CLAUDE_DIR),
        );

        // When initializing git for it.
        actor.init_git(id);

        // Then git init ran in the project's root.
        assert_eq!(
            git.calls(),
            vec![GitCall::Init(PathBuf::from(PROJECT_ROOT))],
            "git init should run in the root"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn initialized_draft_is_a_repository_on_the_roots_branch() -> Result<(), Report<StoreError>> {
        // Given a local draft of a project that isn't a git repository.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let (mut actor, state) = start_with(
            store,
            &FakeHost::listing(Vec::new()),
            &FakeGit::plain(Ok(())),
            Path::new(NO_CLAUDE_DIR),
        );

        // When initializing git for it.
        actor.init_git(id);

        // Then the draft is in a repository, on the branch git reports.
        assert_eq!(
            shown_draft(&state, id).map(|draft| (draft.repo, draft.branch)),
            Some((true, Some(CURRENT_BRANCH.to_owned()))),
            "the draft should now behave as a git project's"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn failed_git_init_shows_why() -> Result<(), Report<StoreError>> {
        // Given a draft of a non-git project where git init fails.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let (mut actor, state) = start_with(
            store,
            &FakeHost::listing(Vec::new()),
            &FakeGit::plain(Err("fatal: cannot mkdir .git: Permission denied")),
            Path::new(NO_CLAUDE_DIR),
        );

        // When initializing git for it.
        actor.init_git(id);

        // Then the mode line says why.
        assert_eq!(
            error_of(&state).as_deref(),
            Some("Git initialization failed: fatal: cannot mkdir .git: Permission denied"),
            "a failed git init should show git's reason"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn non_git_new_worktree_draft_starts_in_the_root() -> Result<(), Report<StoreError>> {
        // Given a new-worktree draft of a project that isn't a git repository.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::NewWorktree))?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, _state) = start_with(
            store,
            &host,
            &FakeGit::plain(Ok(())),
            Path::new(NO_CLAUDE_DIR),
        );

        // When starting it.
        actor.start_draft(id).await;

        // Then the session runs in the root.
        assert_eq!(
            host.created_in(),
            vec![PathBuf::from(PROJECT_ROOT)],
            "a non-git project's draft always starts local"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case::local_name("feat")]
    #[case::origin_ref("origin/feat")]
    #[tokio::test]
    async fn new_worktree_start_bases_on_the_drafts_branch_from_origin(
        #[case] base: &str,
    ) -> Result<(), Report<StoreError>> {
        // Given a new-worktree draft based on `base`, and an origin that has feat.
        let (store, id) = store_with_draft(|id| DraftRow {
            branch: Some(base.to_owned()),
            ..draft_row(id, DraftWorkspace::NewWorktree)
        })?;
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::with_origin(Ok(true)));
        let (mut actor, _state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When starting the draft.
        actor.start_draft(id).await;

        // Then feat is fetched and the worktree starts from origin/feat.
        assert_eq!(
            fetch_and_add_steps(&git),
            vec!["fetch feat".to_owned(), "add from origin/feat".to_owned()],
            "the worktree should start from freshly fetched origin/feat"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn new_worktree_start_uses_another_remotes_ref_as_it_is() -> Result<(), Report<StoreError>>
    {
        // Given a new-worktree draft based on upstream/feat.
        let (store, id) = store_with_draft(|id| DraftRow {
            branch: Some("upstream/feat".to_owned()),
            ..draft_row(id, DraftWorkspace::NewWorktree)
        })?;
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::with_origin(Ok(true)));
        let (mut actor, _state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When starting the draft.
        actor.start_draft(id).await;

        // Then nothing is fetched and the worktree starts from upstream/feat.
        assert_eq!(
            fetch_and_add_steps(&git),
            vec!["add from upstream/feat".to_owned()],
            "another remote's ref should be used without a fetch"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn new_worktree_start_runs_the_session_in_the_new_worktree()
    -> Result<(), Report<StoreError>> {
        // Given a new-worktree draft.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::NewWorktree))?;
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::local());
        let (mut actor, _state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When starting the draft.
        actor.start_draft(id).await;

        // Then the session starts in the worktree git added.
        assert_eq!(
            host.created_in(),
            git.added()
                .map(|(path, _)| path)
                .into_iter()
                .collect::<Vec<_>>(),
            "the session should start in the new worktree"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn new_worktree_thread_is_saved_on_the_worktrees_branch() -> Result<(), Report<StoreError>>
    {
        // Given a new-worktree draft.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::NewWorktree))?;
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::local());
        let (mut actor, _state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When starting the draft.
        actor.start_draft(id).await;

        // Then the saved thread is on the branch orb made.
        assert_eq!(
            saved(&actor.store, "bb")?.branch,
            git.added().map(|(_, branch)| branch),
            "the new thread should be saved on its worktree's branch"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn local_thread_is_saved_on_the_roots_branch_at_start() -> Result<(), Report<StoreError>>
    {
        // Given a local draft still showing main, while the root is on dev.
        let (store, id) = store_with_draft(|id| DraftRow {
            branch: Some("main".to_owned()),
            ..draft_row(id, DraftWorkspace::Local)
        })?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When starting the draft.
        actor.start_draft(id).await;

        // Then the saved thread is on dev.
        assert_eq!(
            saved(&actor.store, "bb")?.branch.as_deref(),
            Some(CURRENT_BRANCH),
            "the new thread should be saved on the branch its root has at start"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn start_passes_the_drafts_model_and_permission() -> Result<(), Report<StoreError>> {
        // Given a local draft with sonnet in plan mode.
        let (store, id) = store_with_draft(|id| DraftRow {
            model: Some("sonnet".to_owned()),
            permission_mode: Some("plan".to_owned()),
            ..draft_row(id, DraftWorkspace::Local)
        })?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When starting the draft.
        actor.start_draft(id).await;

        // Then the session starts with sonnet in plan mode.
        assert_eq!(
            host.created_with(),
            vec![SessionOptions {
                model: Some("sonnet".to_owned()),
                permission_mode: Some("plan".to_owned()),
            }],
            "the session should start with the draft's model and permission"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn local_draft_starts_in_the_projects_root() -> Result<(), Report<StoreError>> {
        // Given saved orb and web projects, web with a local draft.
        let store = Store::open_in_memory()?;
        orb_project(&store)?;
        let web = store.add_project(Path::new("/tmp/web"), "web", 0)?;
        store.save_draft(&draft_row(web, DraftWorkspace::Local))?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When starting web's draft.
        actor.start_draft(web).await;

        // Then the host starts it in web's root.
        assert_eq!(
            host.created_in(),
            vec![PathBuf::from("/tmp/web")],
            "a local draft should start in its project's root"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn start_of_a_missing_worktree_shows_why() -> Result<(), Report<StoreError>> {
        // Given a draft in a worktree that no longer exists.
        let (store, id) = store_with_draft(|id| {
            draft_row(
                id,
                DraftWorkspace::Existing(PathBuf::from("/nonexistent/wt")),
            )
        })?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When starting the draft.
        actor.start_draft(id).await;

        // Then the mode line says the worktree is gone.
        assert_eq!(
            error_of(&state).as_deref(),
            Some("worktree no longer exists: /nonexistent/wt"),
            "a missing worktree should fail the start with its path"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn start_of_a_gone_draft_shows_why() -> Result<(), Report<StoreError>> {
        // Given a project without a draft.
        let store = Store::open_in_memory()?;
        let id = orb_project(&store)?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When starting its draft.
        actor.start_draft(id).await;

        // Then the mode line says the draft is gone.
        assert_eq!(
            error_of(&state).as_deref(),
            Some("the draft is gone"),
            "a start without a draft should fail"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case::succeeded(Ok("bb"))]
    #[case::failed(Err("claude failed"))]
    #[tokio::test]
    async fn start_ends_starting(
        #[case] outcome: Result<&str, &str>,
    ) -> Result<(), Report<StoreError>> {
        // Given a draft start in flight.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::creating(outcome);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        state.write().sessions.starting = true;

        // When the start finishes.
        actor.start_draft(id).await;

        // Then nothing is starting any more.
        assert!(
            !state.read().sessions.starting,
            "a finished start should stop showing as starting"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_start_keeps_the_draft() -> Result<(), Report<StoreError>> {
        // Given a local draft and a host that refuses to start a session.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::creating(Err("claude failed"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When starting the draft.
        actor.start_draft(id).await;

        // Then the draft is still there.
        assert!(
            shown_draft(&state, id).is_some(),
            "a failed start should keep the draft"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_start_shows_the_reason() -> Result<(), Report<StoreError>> {
        // Given a local draft and a host that refuses to start a session.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::creating(Err("claude failed"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When starting the draft.
        actor.start_draft(id).await;

        // Then the reason is the error.
        assert_eq!(
            error_of(&state).as_deref(),
            Some("claude failed"),
            "the mode line should show why the start failed"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_start_adds_no_thread() -> Result<(), Report<StoreError>> {
        // Given a local draft and a host that refuses to start a session.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::creating(Err("claude failed"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When starting the draft.
        actor.start_draft(id).await;

        // Then there are still no threads.
        assert_eq!(
            state.read().sessions.threads().count(),
            0,
            "a failed start shouldn't add a thread"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_draft_start_removes_its_new_worktree() -> Result<(), Report<StoreError>> {
        // Given a new-worktree draft and a host that refuses to start a session.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::NewWorktree))?;
        let (host, git) = (FakeHost::creating(Err("claude failed")), FakeGit::local());
        let (mut actor, _state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When starting the draft.
        actor.start_draft(id).await;

        // Then the worktree orb made is force-removed.
        let (path, _branch) = git
            .added()
            .ok_or_else(|| Report::new(StoreError).attach("no worktree was added"))?;
        assert!(
            git.calls()
                .contains(&GitCall::RemoveWorktree { path, force: true }),
            "a failed start should remove the worktree made for it"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn started_draft_leaves_the_project() -> Result<(), Report<StoreError>> {
        // Given a local draft.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When starting it.
        actor.start_draft(id).await;

        // Then the project has no draft.
        assert_eq!(
            shown_draft(&state, id),
            None,
            "a started draft should leave the sidebar"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn started_draft_is_deleted_from_the_store() -> Result<(), Report<StoreError>> {
        // Given a local draft.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When starting it.
        actor.start_draft(id).await;

        // Then no draft is saved.
        assert_eq!(
            saved_draft(&actor.store, id)?,
            None,
            "a started draft should be deleted"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn started_thread_is_first_in_its_project() -> Result<(), Report<StoreError>> {
        // Given a saved thread and a local draft in the orb project.
        let (store, _) = store_with_thread("aa")?;
        let project = orb_project(&store)?;
        store.save_draft(&draft_row(project, DraftWorkspace::Local))?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When starting the draft.
        actor.start_draft(project).await;

        // Then the new thread is the project's first.
        let first = state
            .read()
            .sessions
            .projects
            .iter()
            .find(|shown| shown.id == project)
            .and_then(|shown| shown.threads.first())
            .map(|thread| thread.id);
        assert_eq!(
            first,
            Some(saved(&actor.store, "bb")?.id),
            "the newest thread comes first"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn started_thread_is_saved_under_its_project() -> Result<(), Report<StoreError>> {
        // Given saved orb and web projects, web with a local draft.
        let store = Store::open_in_memory()?;
        orb_project(&store)?;
        let web = store.add_project(Path::new("/tmp/web"), "web", 0)?;
        store.save_draft(&draft_row(web, DraftWorkspace::Local))?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When starting web's draft.
        actor.start_draft(web).await;

        // Then the saved thread belongs to web.
        assert_eq!(
            saved(&actor.store, "bb")?.project_id,
            web,
            "the new thread should be saved under the draft's project"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn started_thread_saves_its_model_and_permission() -> Result<(), Report<StoreError>> {
        // Given a local draft with sonnet in plan mode.
        let (store, id) = store_with_draft(|id| DraftRow {
            model: Some("sonnet".to_owned()),
            permission_mode: Some("plan".to_owned()),
            ..draft_row(id, DraftWorkspace::Local)
        })?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When starting it.
        actor.start_draft(id).await;

        // Then the saved thread remembers sonnet and plan mode.
        let row = saved(&actor.store, "bb")?;
        assert_eq!(
            (row.model.as_deref(), row.permission_mode.as_deref()),
            (Some("sonnet"), Some("plan")),
            "the thread should save the flags it started with"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn start_records_the_last_used_settings() -> Result<(), Report<StoreError>> {
        // Given a new-worktree draft with sonnet in plan mode.
        let (store, id) = store_with_draft(|id| DraftRow {
            model: Some("sonnet".to_owned()),
            permission_mode: Some("plan".to_owned()),
            ..draft_row(id, DraftWorkspace::NewWorktree)
        })?;
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::local());
        let (mut actor, _state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When starting it.
        actor.start_draft(id).await;

        // Then the project's last-used settings are those.
        assert_eq!(
            actor.store.last_used(id)?,
            Some(used(LastWorkspace::NewWorktree)),
            "a start should record its workspace kind, model and permission"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn start_with_the_draft_selected_selects_the_thread() -> Result<(), Report<StoreError>> {
        // Given a local draft, selected.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        state.write().sessions.cursor = Some(SidebarItem::Draft(id));

        // When starting it.
        actor.start_draft(id).await;

        // Then the new thread is selected.
        assert_eq!(
            state.read().sessions.selected_id(),
            Some(saved(&actor.store, "bb")?.id),
            "the new thread should take the draft's place under the cursor"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn start_with_the_draft_selected_asks_to_attach() -> Result<(), Report<StoreError>> {
        // Given a local draft, selected.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        state.write().sessions.cursor = Some(SidebarItem::Draft(id));

        // When starting it.
        actor.start_draft(id).await;

        // Then the frontend is asked to attach to the new thread.
        assert_eq!(
            state.read().sessions.attach,
            Some(saved(&actor.store, "bb")?.id),
            "a still-selected draft's thread should be attached to"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn start_after_the_cursor_moved_does_not_ask_to_attach() -> Result<(), Report<StoreError>>
    {
        // Given a thread and a local draft, with the thread selected.
        let (store, thread) = store_with_thread("aa")?;
        let id = orb_project(&store)?;
        store.save_draft(&draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        state.write().sessions.cursor = Some(SidebarItem::Thread(thread));

        // When starting the draft.
        actor.start_draft(id).await;

        // Then nothing is to be attached to.
        assert_eq!(
            state.read().sessions.attach,
            None,
            "a draft the user moved away from should not steal focus"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn start_after_the_cursor_moved_keeps_the_cursor() -> Result<(), Report<StoreError>> {
        // Given a thread and a local draft, with the thread selected.
        let (store, thread) = store_with_thread("aa")?;
        let id = orb_project(&store)?;
        store.save_draft(&draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        state.write().sessions.cursor = Some(SidebarItem::Thread(thread));

        // When starting the draft.
        actor.start_draft(id).await;

        // Then the old thread stays selected.
        assert_eq!(
            state.read().sessions.cursor,
            Some(SidebarItem::Thread(thread)),
            "the cursor should stay where the user moved it"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn adding_a_directory_shows_it_as_a_project() -> Result<(), Report<StoreError>> {
        // Given no projects and an existing directory.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let (mut actor, state) = start(
            Store::open_in_memory()?,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When adding it as a project.
        actor.add_project(dir.path().to_owned());

        // Then it's the one project shown.
        let roots: Vec<PathBuf> = state
            .read()
            .sessions
            .projects
            .iter()
            .map(|project| project.root.clone())
            .collect();
        assert_eq!(
            roots,
            vec![dir.path().to_owned()],
            "the added directory should show as a project"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn adding_an_existing_project_shows_it_once() -> Result<(), Report<StoreError>> {
        // Given a directory that's already a saved project.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let store = Store::open_in_memory()?;
        store.add_project(dir.path(), "web", 0)?;
        let (mut actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When adding it again.
        actor.add_project(dir.path().to_owned());

        // Then it's shown once.
        let shown = state
            .read()
            .sessions
            .projects
            .iter()
            .filter(|project| project.root == dir.path())
            .count();
        assert_eq!(shown, 1, "an existing project shouldn't be shown twice");
        Ok(())
    }

    #[rstest::rstest]
    fn adding_a_missing_directory_shows_an_error() -> Result<(), Report<StoreError>> {
        // Given no projects.
        let (mut actor, state) = start(
            Store::open_in_memory()?,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When adding a path that doesn't exist.
        actor.add_project(PathBuf::from("/nonexistent/project"));

        // Then the error names it.
        assert_eq!(
            state.read().sessions.error.as_deref(),
            Some("not a directory: /nonexistent/project"),
            "the mode line should say the path isn't a directory"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn adding_a_missing_directory_saves_nothing() -> Result<(), Report<StoreError>> {
        // Given no projects.
        let (mut actor, _state) = start(
            Store::open_in_memory()?,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When adding a path that doesn't exist.
        actor.add_project(PathBuf::from("/nonexistent/project"));

        // Then no project is saved.
        assert!(
            actor.store.load()?.0.is_empty(),
            "a missing directory shouldn't be saved"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_titles_a_thread_with_its_transcripts_prompt() -> Result<(), Report<StoreError>> {
        // Given a thread whose session's transcript has a prompt.
        let claude_dir = tempfile::tempdir().change_context(StoreError)?;
        let path = transcript_path(claude_dir.path(), Path::new(PROJECT_ROOT), "s1");
        fs::create_dir_all(path.parent().unwrap_or(claude_dir.path()))
            .change_context(StoreError)?;
        fs::write(
            &path,
            "{\"type\":\"user\",\"message\":{\"content\":\"Fix the sidebar\"}}\n",
        )
        .change_context(StoreError)?;
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![SessionRecord {
            session_id: Some("s1".to_owned()),
            ..record("aa", ThreadStatus::Idle)
        }]);
        let (mut actor, state) = start(store, &host, claude_dir.path());

        // When polling.
        actor.poll().await;

        // Then the thread is titled with the prompt.
        let title = state
            .read()
            .sessions
            .threads()
            .find(|thread| thread.id == id)
            .and_then(|thread| thread.title.clone());
        assert_eq!(
            title.as_deref(),
            Some("Fix the sidebar"),
            "the first prompt should title the thread"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_titles_a_thread_with_its_custom_title() -> Result<(), Report<StoreError>> {
        // Given a thread whose transcript has a prompt, an ai-title, then a `/rename`.
        let claude_dir = tempfile::tempdir().change_context(StoreError)?;
        let path = transcript_path(claude_dir.path(), Path::new(PROJECT_ROOT), "s1");
        fs::create_dir_all(path.parent().unwrap_or(claude_dir.path()))
            .change_context(StoreError)?;
        fs::write(
            &path,
            concat!(
                "{\"type\":\"user\",\"message\":{\"content\":\"Fix the sidebar\"}}\n",
                "{\"type\":\"ai-title\",\"aiTitle\":\"Sidebar fix\"}\n",
                "{\"type\":\"custom-title\",\"customTitle\":\"orb-m1\"}\n",
            ),
        )
        .change_context(StoreError)?;
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![SessionRecord {
            session_id: Some("s1".to_owned()),
            ..record("aa", ThreadStatus::Idle)
        }]);
        let (mut actor, state) = start(store, &host, claude_dir.path());

        // When polling.
        actor.poll().await;

        // Then the thread is titled with the custom title.
        let title = state
            .read()
            .sessions
            .threads()
            .find(|thread| thread.id == id)
            .and_then(|thread| thread.title.clone());
        assert_eq!(
            title.as_deref(),
            Some("orb-m1"),
            "`/rename` should beat the ai-title"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_shows_the_located_transcript_on_the_thread() -> Result<(), Report<StoreError>> {
        // Given a thread whose session has a transcript.
        let claude_dir = tempfile::tempdir().change_context(StoreError)?;
        let path = transcript_path(claude_dir.path(), Path::new(PROJECT_ROOT), "s1");
        fs::create_dir_all(path.parent().unwrap_or(claude_dir.path()))
            .change_context(StoreError)?;
        fs::write(&path, "").change_context(StoreError)?;
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![SessionRecord {
            session_id: Some("s1".to_owned()),
            ..record("aa", ThreadStatus::Idle)
        }]);
        let (mut actor, state) = start(store, &host, claude_dir.path());

        // When polling.
        actor.poll().await;

        // Then the thread carries the transcript's path.
        let transcript = state
            .read()
            .sessions
            .threads()
            .find(|thread| thread.id == id)
            .and_then(|thread| thread.transcript.clone());
        assert_eq!(
            transcript,
            Some(path),
            "a located transcript should be shown on its thread"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn restore_lists_projects_by_first_use_and_threads_newest_first()
    -> Result<(), Report<StoreError>> {
        // Given project B saved before project A but first used after it,
        // with A's threads created at 10 and 30 and B's at 20.
        let store = Store::open_in_memory()?;
        let b = store.add_project(Path::new("/b"), "b", 2)?;
        let a = store.add_project(Path::new("/a"), "a", 1)?;
        let insert = |project_id, short_id: &str, created_at| {
            store.insert_thread(&NewThread {
                project_id,
                short_id: short_id.to_owned(),
                cwd: PathBuf::from("/a"),
                created_at,
                model: None,
                permission_mode: None,
            })
        };
        let a_old = insert(a, "a1", 10)?;
        let b_only = insert(b, "b1", 20)?;
        let a_new = insert(a, "a2", 30)?;

        // When the actor starts.
        let (_actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // Then the sidebar lists A's threads newest first, then B's.
        let order: Vec<ThreadId> = state
            .read()
            .sessions
            .threads()
            .map(|thread| thread.id)
            .collect();
        assert_eq!(
            order,
            vec![a_new, a_old, b_only],
            "projects by first use, threads newest first"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn restore_selects_the_first_sidebar_item() -> Result<(), Report<StoreError>> {
        // Given an old pinned thread and a newer unpinned one.
        let store = Store::open_in_memory()?;
        let pinned = add_thread(&store, "old", 10)?;
        add_thread(&store, "new", 20)?;
        resave(&store, "old", |row| ThreadRow {
            pinned_at: Some(30),
            ..row
        })?;

        // When the actor starts.
        let (_actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // Then the pinned thread, first in the sidebar, is selected.
        assert_eq!(
            state.read().sessions.cursor,
            Some(SidebarItem::Thread(pinned)),
            "the sidebar's first row should be selected"
        );
        Ok(())
    }

    /// A store whose sidebar was saved `width` columns wide.
    fn store_with_width(width: u16) -> Result<Store, Report<StoreError>> {
        let store = Store::open_in_memory()?;
        store.save_ui(&Ui {
            sidebar_width: Some(width),
            project_filter: None,
        })?;
        Ok(store)
    }

    #[rstest::rstest]
    fn restore_applies_the_saved_sidebar_width() -> Result<(), Report<StoreError>> {
        // Given a sidebar saved 40 columns wide.
        let store = store_with_width(40)?;

        // When the actor starts.
        let (_actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // Then the sidebar is 40 columns wide.
        assert_eq!(
            state.read().sidebar.width,
            40,
            "the saved width should be restored"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case(10, 24)]
    #[case(200, 80)]
    fn restore_keeps_a_saved_sidebar_width_within_its_bounds(
        #[case] saved: u16,
        #[case] expected: u16,
    ) -> Result<(), Report<StoreError>> {
        // Given a sidebar saved outside 24–80 columns (a hand-edited store).
        let store = store_with_width(saved)?;

        // When the actor starts.
        let (_actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // Then the sidebar is at the nearest bound.
        assert_eq!(
            state.read().sidebar.width,
            expected,
            "a saved width of {saved} should be clamped"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn save_ui_saves_the_sidebar_width() -> Result<(), Report<StoreError>> {
        // Given the actor on a fresh store and a sidebar resized to 44 columns.
        let (actor, state) = start(
            Store::open_in_memory()?,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );
        state.write().sidebar.width = 44;

        // When saving the layout.
        actor.save_ui();

        // Then the store holds the 44-column width and no filter.
        assert_eq!(
            actor.store.ui()?,
            Ui {
                sidebar_width: Some(44),
                project_filter: None,
            },
            "SaveUi should save the sidebar's width"
        );
        Ok(())
    }

    /// Whether project `id` shows as removed.
    fn removed_of(state: &State, id: ProjectId) -> Option<bool> {
        state
            .read()
            .sessions
            .projects
            .iter()
            .find(|project| project.id == id)
            .map(|project| project.removed)
    }

    #[rstest::rstest]
    fn remove_project_marks_it_removed() -> Result<(), Report<StoreError>> {
        // Given a project with a draft.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let (mut actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When removing it.
        actor.remove_project(id);

        // Then it shows as removed.
        assert_eq!(
            removed_of(&state, id),
            Some(true),
            "a removed project should leave ␣n and the filter"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn remove_project_drops_its_draft() -> Result<(), Report<StoreError>> {
        // Given a project with a draft.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let (mut actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When removing it.
        actor.remove_project(id);

        // Then it has no draft.
        assert_eq!(
            shown_draft(&state, id),
            None,
            "a removed project's draft should be discarded"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn restore_shows_a_removed_project_as_removed() -> Result<(), Report<StoreError>> {
        // Given a removed project.
        let store = Store::open_in_memory()?;
        let id = orb_project(&store)?;
        store.remove_project(id, 1_000)?;

        // When the actor starts.
        let (_actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // Then the project shows as removed.
        assert_eq!(
            removed_of(&state, id),
            Some(true),
            "the removal should survive a restart"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn adding_a_removed_project_restores_it() -> Result<(), Report<StoreError>> {
        // Given a directory that's a removed project.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let store = Store::open_in_memory()?;
        let id = store.add_project(dir.path(), "web", 0)?;
        store.remove_project(id, 1_000)?;
        let (mut actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When adding it again.
        actor.add_project(dir.path().to_owned());

        // Then it's no longer removed.
        assert_eq!(
            removed_of(&state, id),
            Some(false),
            "␣p on a removed project's directory should restore it"
        );
        Ok(())
    }

    /// A store with projects orb and web, each holding one thread, and its
    /// layout saved filtered to web. Returns web's id and thread.
    fn store_filtered_to_web() -> Result<(Store, ProjectId, ThreadId), Report<StoreError>> {
        let store = Store::open_in_memory()?;
        add_thread(&store, "aa", 1_000)?;
        let web = store.add_project(Path::new("/tmp/web"), "web", 0)?;
        let thread = store.insert_thread(&NewThread {
            project_id: web,
            short_id: "bb".to_owned(),
            cwd: PathBuf::from("/tmp/web"),
            created_at: 500,
            model: None,
            permission_mode: None,
        })?;
        store.save_ui(&Ui {
            sidebar_width: None,
            project_filter: Some(web),
        })?;
        Ok((store, web, thread))
    }

    #[rstest::rstest]
    fn restore_applies_the_saved_filter() -> Result<(), Report<StoreError>> {
        // Given a sidebar saved filtered to web.
        let (store, web, _) = store_filtered_to_web()?;

        // When the actor starts.
        let (_actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // Then the sidebar is filtered to web.
        assert_eq!(
            state.read().sessions.filter,
            Some(web),
            "the saved filter should be restored"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn restore_selects_the_first_filtered_row() -> Result<(), Report<StoreError>> {
        // Given a sidebar saved filtered to web, whose thread is older than
        // orb's, so it would otherwise be the second row.
        let (store, _, thread) = store_filtered_to_web()?;

        // When the actor starts.
        let (_actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // Then the cursor is on web's thread.
        assert_eq!(
            state.read().sessions.cursor,
            Some(SidebarItem::Thread(thread)),
            "the cursor should start on the filtered sidebar's first row"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn restore_ignores_a_filter_to_a_removed_project() -> Result<(), Report<StoreError>> {
        // Given a sidebar saved filtered to web, which was then removed.
        let (store, web, _) = store_filtered_to_web()?;
        store.remove_project(web, 2_000)?;

        // When the actor starts.
        let (_actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // Then the sidebar lists every project.
        assert_eq!(
            state.read().sessions.filter,
            None,
            "a filter to a removed project should be dropped"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn save_ui_saves_the_filter() -> Result<(), Report<StoreError>> {
        // Given the actor with the orb project and the sidebar filtered to it.
        let store = Store::open_in_memory()?;
        let id = orb_project(&store)?;
        let (actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );
        state.write().sessions.filter = Some(id);

        // When saving the layout.
        actor.save_ui();

        // Then the store holds the filter.
        assert_eq!(
            actor.store.ui()?.project_filter,
            Some(id),
            "SaveUi should save the project filter"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn restore_shows_when_each_project_was_added() -> Result<(), Report<StoreError>> {
        // Given a project saved at 1.5 s.
        let store = Store::open_in_memory()?;
        store.add_project(Path::new(PROJECT_ROOT), "orb", 1_500)?;

        // When the actor starts.
        let (_actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // Then the project shows it was added at 1.5 s.
        let added: Vec<SystemTime> = state
            .read()
            .sessions
            .projects
            .iter()
            .map(|project| project.created_at)
            .collect();
        assert_eq!(
            added,
            vec![SystemTime::UNIX_EPOCH + Duration::from_millis(1_500)],
            "a restored project should keep when it was added"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn restore_puts_a_saved_draft_on_its_project() -> Result<(), Report<StoreError>> {
        // Given a project with a saved new-worktree draft on main, in opus,
        // created at 2 s.
        let store = Store::open_in_memory()?;
        let project_id = store.add_project(Path::new(PROJECT_ROOT), "orb", 1_500)?;
        store.save_draft(&DraftRow {
            project_id,
            workspace: DraftWorkspace::NewWorktree,
            branch: Some("main".to_owned()),
            model: Some("opus".to_owned()),
            permission_mode: None,
            created_at: 2_000,
        })?;

        // When the actor starts.
        let (_actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // Then the project shows that draft.
        let drafts: Vec<Option<Draft>> = state
            .read()
            .sessions
            .projects
            .iter()
            .map(|project| project.draft.clone())
            .collect();
        assert_eq!(
            drafts,
            vec![Some(Draft {
                workspace: DraftWorkspace::NewWorktree,
                branch: Some("main".to_owned()),
                model: Some("opus".to_owned()),
                permission: None,
                created_at: SystemTime::UNIX_EPOCH + Duration::from_millis(2_000),
                repo: true,
                from: Some("main".to_owned()),
            })],
            "a saved draft should be restored onto its project"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn new_session_id_resets_the_transcript_cursor_and_keeps_the_title()
    -> Result<(), Report<StoreError>> {
        // Given a titled thread part-way through its transcript, and a host
        // reporting a new session id for it (e.g. after `/clear`).
        let (store, _) = store_with_thread("aa")?;
        let row = ThreadRow {
            session_id: Some("old".to_owned()),
            title: Some("Fix the sidebar".to_owned()),
            transcript_path: Some(PathBuf::from("/nonexistent/old.jsonl")),
            transcript_offset: 120,
            ..saved(&store, "aa")?
        };
        store.save_thread(&row)?;
        let host = FakeHost::listing(vec![SessionRecord {
            session_id: Some("new".to_owned()),
            ..record("aa", ThreadStatus::Idle)
        }]);
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When polling.
        actor.poll().await;

        // Then the saved cursor starts over and the title stays.
        let row = saved(&actor.store, "aa")?;
        assert_eq!(
            (row.transcript_path, row.transcript_offset, row.title),
            (None, 0, Some("Fix the sidebar".to_owned())),
            "a new session reads its own transcript from the start"
        );
        Ok(())
    }

    /// Makes the thread `short_id` idle since four days ago; returns that time.
    fn idle_for_four_days(store: &Store, short_id: &str) -> Result<i64, Report<StoreError>> {
        let since = now_ms() - 4 * DAY_MS;
        resave(store, short_id, |row| ThreadRow {
            last_activity_at: since,
            last_visited_at: since,
            ..row
        })?;
        Ok(since)
    }

    /// Settles the saved thread `short_id` an hour ago.
    fn settled_an_hour_ago(store: &Store, short_id: &str) -> Result<(), Report<StoreError>> {
        resave(store, short_id, |row| ThreadRow {
            settled_override: Some(SettledOverride::Settled),
            settled_at: Some(now_ms() - HOUR_MS),
            ..row
        })
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn settle_marks_the_thread_settled() -> Result<(), Report<StoreError>> {
        // Given an idle thread.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When settling it.
        actor.settle(id).await;

        // Then it shows settled and is saved settled.
        let settled = (
            shown(&state, id)
                .and_then(|thread| thread.settled_at)
                .is_some(),
            saved(&actor.store, "aa")?.settled_at.is_some(),
        );
        assert_eq!(
            settled,
            (true, true),
            "a settled thread should show and be saved as settled"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn settle_stops_an_idle_session() -> Result<(), Report<StoreError>> {
        // Given an idle thread.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When settling it.
        actor.settle(id).await;

        // Then its session is stopped.
        assert_eq!(
            host.stopped(),
            vec!["aa".to_owned()],
            "settling should stop the idle session"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn settle_unpins_the_thread() -> Result<(), Report<StoreError>> {
        // Given a pinned idle thread.
        let (store, id) = store_with_thread("aa")?;
        resave(&store, "aa", |row| ThreadRow {
            pinned_at: Some(now_ms() - HOUR_MS),
            ..row
        })?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When settling it.
        actor.settle(id).await;

        // Then it is no longer pinned.
        assert_eq!(
            shown(&state, id).map(|thread| thread.pinned_at),
            Some(None),
            "a settle should remove the pin"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn settle_of_a_working_thread_is_ignored() -> Result<(), Report<StoreError>> {
        // Given a thread a poll saw working.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When a settle arrives.
        actor.settle(id).await;

        // Then the thread stays active and its session keeps running.
        let ignored = (
            shown(&state, id).map(|thread| thread.settled_at),
            host.stopped(),
        );
        assert_eq!(
            ignored,
            (Some(None), Vec::<String>::new()),
            "a turn underway should win over the settle"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn unsettle_keeps_the_thread_active() -> Result<(), Report<StoreError>> {
        // Given a settled thread.
        let (store, id) = store_with_thread("aa")?;
        settled_an_hour_ago(&store, "aa")?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When un-settling it.
        actor.unsettle(id);

        // Then it is saved kept active and not settled.
        let row = saved(&actor.store, "aa")?;
        assert_eq!(
            (row.settled_override, row.settled_at),
            (Some(SettledOverride::Active), None),
            "an un-settle should keep the thread active"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn unsettle_moves_the_thread_to_the_top_of_active() -> Result<(), Report<StoreError>> {
        // Given an older settled thread and a newer active one.
        let store = Store::open_in_memory()?;
        let old = add_thread(&store, "aa", now_ms() - 2 * HOUR_MS)?;
        add_thread(&store, "bb", now_ms() - HOUR_MS)?;
        settled_an_hour_ago(&store, "aa")?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When un-settling the older one.
        actor.unsettle(old);

        // Then it is the first card.
        let first = state
            .read()
            .sessions
            .sidebar()
            .first()
            .map(SidebarRow::item);
        assert_eq!(
            first,
            Some(SidebarItem::Thread(old)),
            "an un-settled thread re-enters Active at the top"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn pin_on_a_settled_thread_unsettles_it() -> Result<(), Report<StoreError>> {
        // Given a settled thread.
        let (store, id) = store_with_thread("aa")?;
        settled_an_hour_ago(&store, "aa")?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When pinning it.
        actor.pin(id);

        // Then it is pinned and no longer settled.
        let pinned = shown(&state, id)
            .map(|thread| (thread.pinned_at.is_some(), thread.settled_at.is_none()));
        assert_eq!(
            pinned,
            Some((true, true)),
            "pinning should bring a settled thread back as a pin"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn working_poll_unsettles_a_settled_thread() -> Result<(), Report<StoreError>> {
        // Given a settled thread whose session starts a turn.
        let (store, id) = store_with_thread("aa")?;
        settled_an_hour_ago(&store, "aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When polling.
        actor.poll().await;

        // Then it is no longer settled.
        assert_eq!(
            shown(&state, id).map(|thread| thread.settled_at),
            Some(None),
            "turn activity should un-settle the thread"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn working_poll_clears_a_kept_active_mark() -> Result<(), Report<StoreError>> {
        // Given a kept-active thread whose session starts a turn.
        let (store, _) = store_with_thread("aa")?;
        resave(&store, "aa", |row| ThreadRow {
            settled_override: Some(SettledOverride::Active),
            ..row
        })?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When polling.
        actor.poll().await;

        // Then the mark is cleared.
        assert_eq!(
            saved(&actor.store, "aa")?.settled_override,
            None,
            "turn activity should let auto-settle apply again"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn idle_for_three_days_auto_settles() -> Result<(), Report<StoreError>> {
        // Given a thread idle for four days.
        let (store, _) = store_with_thread("aa")?;
        let since = idle_for_four_days(&store, "aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When polling.
        actor.poll().await;

        // Then it is settled as of its last activity.
        let row = saved(&actor.store, "aa")?;
        assert_eq!(
            (row.settled_override, row.settled_at),
            (Some(SettledOverride::Settled), Some(since)),
            "a long-idle thread should settle as of its last activity"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn auto_settle_stops_the_session() -> Result<(), Report<StoreError>> {
        // Given a thread idle for four days.
        let (store, _) = store_with_thread("aa")?;
        idle_for_four_days(&store, "aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When polling.
        actor.poll().await;

        // Then its session is stopped.
        assert_eq!(
            host.stopped(),
            vec!["aa".to_owned()],
            "an auto-settle should stop the idle session"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn pinned_thread_never_auto_settles() -> Result<(), Report<StoreError>> {
        // Given a pinned thread idle for four days.
        let (store, id) = store_with_thread("aa")?;
        idle_for_four_days(&store, "aa")?;
        resave(&store, "aa", |row| ThreadRow {
            pinned_at: Some(now_ms() - 5 * DAY_MS),
            ..row
        })?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When polling.
        actor.poll().await;

        // Then it stays active.
        assert_eq!(
            shown(&state, id).map(|thread| thread.settled_at),
            Some(None),
            "a pin keeps the thread in view"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn kept_active_thread_never_auto_settles() -> Result<(), Report<StoreError>> {
        // Given a kept-active thread idle for four days.
        let (store, id) = store_with_thread("aa")?;
        idle_for_four_days(&store, "aa")?;
        resave(&store, "aa", |row| ThreadRow {
            settled_override: Some(SettledOverride::Active),
            ..row
        })?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When polling.
        actor.poll().await;

        // Then it stays active.
        assert_eq!(
            shown(&state, id).map(|thread| thread.settled_at),
            Some(None),
            "an un-settle holds until the next turn activity"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn attached_thread_never_auto_settles() -> Result<(), Report<StoreError>> {
        // Given orb attached to a thread idle for four days.
        let (store, id) = store_with_thread("aa")?;
        idle_for_four_days(&store, "aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        {
            let mut app = state.write();
            app.sessions.cursor = Some(SidebarItem::Thread(id));
            app.focus = Focus::Attached;
        }

        // When polling.
        actor.poll().await;

        // Then it stays active.
        assert_eq!(
            shown(&state, id).map(|thread| thread.settled_at),
            Some(None),
            "stopping the attached session would kill orb's pane"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn turn_end_on_an_unselected_thread_is_unseen() -> Result<(), Report<StoreError>> {
        // Given a working thread while another thread is selected.
        let (store, id) = store_with_thread("aa")?;
        let other = add_thread(&store, "bb", now_ms() - HOUR_MS)?;
        let host = FakeHost::listing(vec![
            record("aa", ThreadStatus::Working),
            record("bb", ThreadStatus::Idle),
        ]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        state.write().sessions.cursor = Some(SidebarItem::Thread(other));
        actor.poll().await;

        // When a poll sees its turn end.
        host.set_list(Ok(vec![
            record("aa", ThreadStatus::Idle),
            record("bb", ThreadStatus::Idle),
        ]));
        actor.poll().await;

        // Then it is unseen.
        assert_eq!(
            shown(&state, id).map(|thread| thread.unseen),
            Some(true),
            "a turn that ended elsewhere should wait to be seen"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn turn_end_on_the_selected_thread_is_seen() -> Result<(), Report<StoreError>> {
        // Given the selected thread working.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        state.write().sessions.cursor = Some(SidebarItem::Thread(id));
        actor.poll().await;

        // When a poll sees its turn end.
        host.set_list(Ok(vec![record("aa", ThreadStatus::Idle)]));
        actor.poll().await;

        // Then it is seen.
        assert_eq!(
            shown(&state, id).map(|thread| thread.unseen),
            Some(false),
            "being on the thread when its turn ends counts as seeing it"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn turn_end_stamps_last_activity() -> Result<(), Report<StoreError>> {
        // Given a thread a poll saw working.
        let (store, _) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;
        let before = now_ms();

        // When a poll sees its turn end.
        host.set_list(Ok(vec![record("aa", ThreadStatus::Idle)]));
        actor.poll().await;

        // Then its last activity is the turn's end.
        assert!(
            saved(&actor.store, "aa")?.last_activity_at >= before,
            "the turn end should be the thread's last activity"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn visit_marks_the_thread_seen() -> Result<(), Report<StoreError>> {
        // Given a thread whose last turn ended after it was last selected.
        let (store, id) = store_with_thread("aa")?;
        resave(&store, "aa", |row| ThreadRow {
            last_activity_at: now_ms() - 60_000,
            ..row
        })?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When the user visits it.
        actor.visit(id);

        // Then it is seen.
        assert_eq!(
            shown(&state, id).map(|thread| thread.unseen),
            Some(false),
            "selecting a thread should see its latest turn"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn delete_removes_the_session() -> Result<(), Report<StoreError>> {
        // Given an idle thread.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When deleting it.
        actor.delete(id).await;

        // Then its session is removed.
        assert_eq!(
            host.removed(),
            vec!["aa".to_owned()],
            "deleting should remove the Claude session"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn delete_removes_the_thread() -> Result<(), Report<StoreError>> {
        // Given an idle thread.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When deleting it.
        actor.delete(id).await;

        // Then it is neither shown nor saved.
        let kept = (
            shown(&state, id).is_some(),
            actor.store.load()?.1.iter().any(|row| row.id == id),
        );
        assert_eq!(kept, (false, false), "a deleted thread should be forgotten");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_remove_keeps_the_thread() -> Result<(), Report<StoreError>> {
        // Given an idle thread whose session can't be removed.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::refusing_remove(vec![record("aa", ThreadStatus::Idle)], "rm: busy");
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When deleting it.
        actor.delete(id).await;

        // Then it is still shown.
        assert!(
            shown(&state, id).is_some(),
            "a session that wasn't removed would keep running unseen"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_remove_shows_the_reason() -> Result<(), Report<StoreError>> {
        // Given an idle thread whose session can't be removed.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::refusing_remove(vec![record("aa", ThreadStatus::Idle)], "rm: busy");
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When deleting it.
        actor.delete(id).await;

        // Then the reason is the error.
        assert_eq!(
            state.read().sessions.error.as_deref(),
            Some("rm: busy"),
            "the mode line should show why the delete failed"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn deleting_a_gone_thread_skips_remove() -> Result<(), Report<StoreError>> {
        // Given a thread Claude no longer knows.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When deleting it.
        actor.delete(id).await;

        // Then nothing is removed.
        assert!(
            host.removed().is_empty(),
            "there is no session left to remove"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn deleting_a_gone_thread_forgets_it() -> Result<(), Report<StoreError>> {
        // Given a thread Claude no longer knows.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When deleting it.
        actor.delete(id).await;

        // Then it is no longer shown.
        assert!(
            shown(&state, id).is_none(),
            "a gone thread should be deleted directly"
        );
        Ok(())
    }

    /// Hides thread `id` as being deleted, as the intent handler does.
    fn hide(state: &State, id: ThreadId) {
        state.write().sessions.deleting.insert(id);
    }

    /// Whether any thread is still hidden as being deleted.
    fn any_hidden(state: &State) -> bool {
        !state.read().sessions.deleting.is_empty()
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn delete_unhides_the_thread() -> Result<(), Report<StoreError>> {
        // Given an idle thread hidden as being deleted.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;
        hide(&state, id);

        // When deleting it.
        actor.delete(id).await;

        // Then nothing is left hidden.
        assert!(
            !any_hidden(&state),
            "a deleted thread shouldn't stay marked as being deleted"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_remove_shows_the_thread_again() -> Result<(), Report<StoreError>> {
        // Given an idle thread hidden as being deleted, whose session can't
        // be removed.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::refusing_remove(vec![record("aa", ThreadStatus::Idle)], "rm: busy");
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;
        hide(&state, id);

        // When deleting it.
        actor.delete(id).await;

        // Then the sidebar lists it again.
        let listed = state
            .read()
            .sessions
            .sidebar()
            .iter()
            .any(|row| row.item() == SidebarItem::Thread(id));
        assert!(listed, "a thread whose delete failed should come back");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn deleting_a_gone_thread_unhides_it() -> Result<(), Report<StoreError>> {
        // Given a thread Claude no longer knows, hidden as being deleted.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;
        hide(&state, id);

        // When deleting it.
        actor.delete(id).await;

        // Then nothing is left hidden.
        assert!(
            !any_hidden(&state),
            "a deleted gone thread shouldn't stay marked as being deleted"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn deleting_an_unknown_thread_unhides_it() -> Result<(), Report<StoreError>> {
        // Given a thread orb doesn't know, hidden as being deleted.
        let (store, _id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;
        let unknown = ThreadId(999);
        hide(&state, unknown);

        // When deleting it.
        actor.delete(unknown).await;

        // Then nothing is left hidden.
        assert!(
            !any_hidden(&state),
            "a delete with nothing to delete should still unhide the thread"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_shows_the_transcript_branch() -> Result<(), Report<StoreError>> {
        // Given a thread whose transcript's prompt names a branch.
        let claude_dir = tempfile::tempdir().change_context(StoreError)?;
        let path = transcript_path(claude_dir.path(), Path::new(PROJECT_ROOT), "s1");
        fs::create_dir_all(path.parent().unwrap_or(claude_dir.path()))
            .change_context(StoreError)?;
        fs::write(
            &path,
            "{\"type\":\"user\",\"gitBranch\":\"main\",\"message\":{\"content\":\"Fix it\"}}\n",
        )
        .change_context(StoreError)?;
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![SessionRecord {
            session_id: Some("s1".to_owned()),
            ..record("aa", ThreadStatus::Idle)
        }]);
        let (mut actor, state) = start(store, &host, claude_dir.path());

        // When polling.
        actor.poll().await;

        // Then the thread shows the branch.
        assert_eq!(
            shown(&state, id).and_then(|thread| thread.branch),
            Some("main".to_owned()),
            "the transcript's branch should show on the thread"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn move_to_new_worktree_starts_the_session_in_the_new_worktree()
    -> Result<(), Report<StoreError>> {
        // Given a prompt-less thread in the orb project's root.
        let (store, id) = store_with_thread("aa")?;
        let (host, git) = (FakeHost::moving(Ok("bb")), FakeGit::local());
        let (mut actor, _state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When moving it to a new worktree.
        actor.move_thread(id, Workspace::NewWorktree).await;

        // Then the new session starts in an orb-<hex> worktree of orb.
        let created = host.created_in();
        let in_worktree = created.first().is_some_and(|cwd| {
            cwd.parent() == Some(Path::new("/nonexistent/worktrees/orb"))
                && hex_branch(Path::new(WORKTREES_ROOT), cwd).is_some()
        });
        assert!(
            created.len() == 1 && in_worktree,
            "one session should start in a new orb worktree, got {created:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn move_to_new_worktree_keeps_the_threads_row() -> Result<(), Report<StoreError>> {
        // Given a prompt-less thread in the orb project's root.
        let (store, id) = store_with_thread("aa")?;
        let (host, git) = (FakeHost::moving(Ok("bb")), FakeGit::local());
        let (mut actor, state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When moving it to a new worktree.
        actor.move_thread(id, Workspace::NewWorktree).await;

        // Then the same thread, alone in its project, now runs in the worktree on its branch.
        let threads: Vec<(ThreadId, PathBuf, Option<String>)> = state
            .read()
            .sessions
            .threads()
            .map(|thread| (thread.id, thread.cwd.clone(), thread.branch.clone()))
            .collect();
        let expected = git
            .added()
            .map(|(path, branch)| vec![(id, path, Some(branch))]);
        assert_eq!(
            Some(threads),
            expected,
            "the thread should keep its row in the new worktree"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn move_removes_the_old_session() -> Result<(), Report<StoreError>> {
        // Given a prompt-less thread whose session is aa.
        let (store, id) = store_with_thread("aa")?;
        let (host, git) = (FakeHost::moving(Ok("bb")), FakeGit::local());
        let (mut actor, _state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When moving it to a new worktree.
        actor.move_thread(id, Workspace::NewWorktree).await;

        // Then aa is removed.
        assert_eq!(
            host.removed(),
            vec!["aa".to_owned()],
            "the old session should be removed"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn move_bases_on_origin_default_when_origin_exists() -> Result<(), Report<StoreError>> {
        // Given a repository with an origin that has main.
        let (store, id) = store_with_thread("aa")?;
        let (host, git) = (FakeHost::moving(Ok("bb")), FakeGit::with_origin(Ok(true)));
        let (mut actor, _state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When moving a thread to a new worktree.
        actor.move_thread(id, Workspace::NewWorktree).await;

        // Then main is fetched and the worktree starts from origin/main.
        let steps: Vec<String> = git
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                GitCall::Fetch(branch) => Some(format!("fetch {branch}")),
                GitCall::AddWorktree { base, .. } => Some(format!("add from {base}")),
                _ => None,
            })
            .collect();
        assert_eq!(
            steps,
            vec!["fetch main".to_owned(), "add from origin/main".to_owned()],
            "the worktree should start from freshly fetched origin/main"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn move_bases_on_local_default_without_origin() -> Result<(), Report<StoreError>> {
        // Given a repository without an origin.
        let (store, id) = store_with_thread("aa")?;
        let (host, git) = (FakeHost::moving(Ok("bb")), FakeGit::local());
        let (mut actor, _state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When moving a thread to a new worktree.
        actor.move_thread(id, Workspace::NewWorktree).await;

        // Then nothing is fetched and the worktree starts from the local main.
        let calls: Vec<Option<String>> = git
            .calls()
            .into_iter()
            .map(|call| match call {
                GitCall::AddWorktree { base, .. } => Some(base),
                _ => None,
            })
            .collect();
        assert_eq!(
            calls,
            vec![Some("main".to_owned())],
            "the only call should add a worktree from main"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_fetch_fails_the_move_with_the_reason() -> Result<(), Report<StoreError>> {
        // Given an origin that can't be reached.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::moving(Ok("bb"));
        let git = FakeGit::with_origin(Err("fatal: unable to access origin"));
        let (mut actor, state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When moving a thread to a new worktree.
        actor.move_thread(id, Workspace::NewWorktree).await;

        // Then git's reason shows.
        assert_eq!(
            error_of(&state).as_deref(),
            Some("fatal: unable to access origin"),
            "the fetch's reason should show"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_fetch_adds_no_worktree() -> Result<(), Report<StoreError>> {
        // Given an origin that can't be reached.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::moving(Ok("bb"));
        let git = FakeGit::with_origin(Err("fatal: unable to access origin"));
        let (mut actor, _state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When moving a thread to a new worktree.
        actor.move_thread(id, Workspace::NewWorktree).await;

        // Then no worktree is added.
        assert_eq!(git.added(), None, "a failed fetch shouldn't add a worktree");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_start_removes_the_new_worktree_and_branch() -> Result<(), Report<StoreError>> {
        // Given claude refusing to start a session.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::moving(Err("claude failed to start"));
        let git = FakeGit::local();
        let (mut actor, _state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When moving a thread to a new worktree.
        actor.move_thread(id, Workspace::NewWorktree).await;

        // Then the worktree and its branch are force-removed after being added.
        let (path, branch) = git
            .added()
            .ok_or_else(|| Report::new(StoreError).attach("no worktree was added"))?;
        let cleanup: Vec<GitCall> = git.calls().into_iter().skip(1).collect();
        assert_eq!(
            cleanup,
            vec![
                GitCall::RemoveWorktree { path, force: true },
                GitCall::DeleteBranch {
                    branch,
                    force: true
                }
            ],
            "a failed start should remove what orb made for it"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_start_keeps_the_old_session() -> Result<(), Report<StoreError>> {
        // Given claude refusing to start a session.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::moving(Err("claude failed to start"));
        let git = FakeGit::local();
        let (mut actor, _state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When moving a thread to a new worktree.
        actor.move_thread(id, Workspace::NewWorktree).await;

        // Then the old session isn't removed.
        assert!(
            host.removed().is_empty(),
            "a failed start should keep the old session"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn move_to_existing_path_adds_no_worktree() -> Result<(), Report<StoreError>> {
        // Given a prompt-less thread and an existing worktree directory.
        let existing = tempfile::tempdir().change_context(StoreError)?;
        let (store, id) = store_with_thread("aa")?;
        let (host, git) = (FakeHost::moving(Ok("bb")), FakeGit::local());
        let (mut actor, _state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When moving the thread there.
        actor
            .move_thread(id, Workspace::Existing(existing.path().to_owned()))
            .await;

        // Then the session starts there and git adds nothing.
        assert_eq!(
            (host.created_in(), git.added()),
            (vec![existing.path().to_owned()], None),
            "a move to an existing directory should only start a session there"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn move_to_missing_path_shows_an_error() -> Result<(), Report<StoreError>> {
        // Given a prompt-less thread and a worktree directory that is gone.
        let (store, id) = store_with_thread("aa")?;
        let (host, git) = (FakeHost::moving(Ok("bb")), FakeGit::local());
        let (mut actor, state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When moving the thread there.
        actor
            .move_thread(id, Workspace::Existing(PathBuf::from("/nonexistent/gone")))
            .await;

        // Then the error names the missing worktree.
        assert_eq!(
            error_of(&state).as_deref(),
            Some("worktree no longer exists: /nonexistent/gone"),
            "a missing worktree should fail the move"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn move_out_of_an_unused_orb_worktree_removes_it() -> Result<(), Report<StoreError>> {
        // Given a prompt-less thread alone in an orb worktree on its hex branch.
        let old = PathBuf::from("/nonexistent/worktrees/orb/orb-0a1b2c3d");
        let existing = tempfile::tempdir().change_context(StoreError)?;
        let (store, id) = store_with_thread("aa")?;
        resave(&store, "aa", |row| ThreadRow {
            cwd: old.clone(),
            branch: Some("orb/0a1b2c3d".to_owned()),
            ..row
        })?;
        let (host, git) = (FakeHost::moving(Ok("bb")), FakeGit::local());
        let (mut actor, _state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When moving the thread to another directory.
        actor
            .move_thread(id, Workspace::Existing(existing.path().to_owned()))
            .await;

        // Then the old worktree and its branch are removed, without force.
        assert_eq!(
            git.calls(),
            vec![
                GitCall::RemoveWorktree {
                    path: old,
                    force: false
                },
                GitCall::DeleteBranch {
                    branch: "orb/0a1b2c3d".to_owned(),
                    force: false
                }
            ],
            "an orb worktree no thread uses should be removed safely"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn move_refused_once_the_thread_has_a_transcript() -> Result<(), Report<StoreError>> {
        // Given a thread whose session wrote a transcript since the last poll.
        let claude_dir = tempfile::tempdir().change_context(StoreError)?;
        let path = transcript_path(claude_dir.path(), Path::new(PROJECT_ROOT), "s1");
        fs::create_dir_all(path.parent().unwrap_or(claude_dir.path()))
            .change_context(StoreError)?;
        fs::write(&path, "").change_context(StoreError)?;
        let (store, id) = store_with_thread("aa")?;
        let host = Arc::new(FakeHost {
            create: Ok("bb".to_owned()),
            ..FakeHost::answering(vec![SessionRecord {
                session_id: Some("s1".to_owned()),
                ..record("aa", ThreadStatus::Idle)
            }])
        });
        let git = FakeGit::local();
        let (mut actor, state) = start_with(store, &host, &git, claude_dir.path());

        // When moving it to a new worktree.
        actor.move_thread(id, Workspace::NewWorktree).await;

        // Then the move is refused with the lock message and no session starts.
        assert_eq!(
            (error_of(&state).as_deref(), host.created_in()),
            (Some("Workspace locked · Local checkout"), Vec::new()),
            "a thread with a transcript can't change workspace"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn moved_thread_saves_its_new_short_id_and_cwd() -> Result<(), Report<StoreError>> {
        // Given a prompt-less thread and an existing worktree directory.
        let existing = tempfile::tempdir().change_context(StoreError)?;
        let (store, id) = store_with_thread("aa")?;
        let (host, git) = (FakeHost::moving(Ok("bb")), FakeGit::local());
        let (mut actor, _state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When moving the thread there.
        actor
            .move_thread(id, Workspace::Existing(existing.path().to_owned()))
            .await;

        // Then the store has the thread under its new session, in the new directory.
        let row = saved(&actor.store, "bb")?;
        assert_eq!(
            (row.id, row.cwd),
            (id, existing.path().to_owned()),
            "the move should survive a relaunch"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn move_restarts_with_the_threads_model_and_permission() -> Result<(), Report<StoreError>>
    {
        // Given a prompt-less thread started with sonnet in plan mode.
        let store = Store::open_in_memory()?;
        let id = store.insert_thread(&NewThread {
            project_id: orb_project(&store)?,
            short_id: "aa".to_owned(),
            cwd: PathBuf::from(PROJECT_ROOT),
            created_at: now_ms() - HOUR_MS,
            model: Some("sonnet".to_owned()),
            permission_mode: Some("plan".to_owned()),
        })?;
        let host = FakeHost::moving(Ok("bb"));
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When moving it to a new worktree.
        actor.move_thread(id, Workspace::NewWorktree).await;

        // Then its new session starts with sonnet in plan mode.
        assert_eq!(
            host.created_with(),
            vec![SessionOptions {
                model: Some("sonnet".to_owned()),
                permission_mode: Some("plan".to_owned()),
            }],
            "a moved thread should keep its model and permission"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn untrusted_draft_start_asks_for_trust_in_the_root() -> Result<(), Report<StoreError>> {
        // Given a local draft and claude refusing the project's untrusted root.
        let (store, project) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::untrusted(1, Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When starting the draft.
        actor.start_draft(project).await;

        // Then the start waits for the root to be trusted.
        assert_eq!(
            trust_of(&state),
            Some(PathBuf::from(PROJECT_ROOT)),
            "the project's root should be offered for trust"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn untrusted_worktree_asks_for_trust_in_the_worktree() -> Result<(), Report<StoreError>> {
        // Given claude refusing a new, untrusted worktree.
        let (store, id) = store_with_thread("aa")?;
        let (host, git) = (FakeHost::untrusted(1, Ok("bb")), FakeGit::local());
        let (mut actor, state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When moving a thread to a new worktree.
        actor.move_thread(id, Workspace::NewWorktree).await;

        // Then the start waits for the refused worktree itself to be trusted.
        assert_eq!(
            trust_of(&state),
            host.created_in().last().cloned(),
            "the refused worktree should be offered for trust"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn untrusted_move_keeps_the_new_worktree() -> Result<(), Report<StoreError>> {
        // Given claude refusing a new, untrusted worktree.
        let (store, id) = store_with_thread("aa")?;
        let (host, git) = (FakeHost::untrusted(1, Ok("bb")), FakeGit::local());
        let (mut actor, _state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When moving a thread to a new worktree.
        actor.move_thread(id, Workspace::NewWorktree).await;

        // Then the worktree is only added, not removed.
        let removed = git
            .calls()
            .into_iter()
            .any(|call| matches!(call, GitCall::RemoveWorktree { .. }));
        assert!(!removed, "the worktree should wait for the retry");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn retry_after_trust_starts_the_session() -> Result<(), Report<StoreError>> {
        // Given a draft start waiting for its directory to be trusted.
        let (store, project) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::untrusted(1, Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.start_draft(project).await;

        // When retrying the start after the user trusted it.
        actor.retry_start().await;

        // Then the new thread shows.
        assert_eq!(
            state.read().sessions.threads().count(),
            1,
            "the retried start should add the thread"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn retry_refused_again_shows_the_error() -> Result<(), Report<StoreError>> {
        // Given a draft start waiting for trust, and the directory still untrusted.
        let (store, project) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::untrusted(2, Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.start_draft(project).await;

        // When retrying the start.
        actor.retry_start().await;

        // Then claude's refusal shows.
        assert_eq!(
            error_of(&state).as_deref(),
            Some(UNTRUSTED),
            "a second refusal should fail the start with its reason"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn retry_refused_again_removes_the_new_worktree() -> Result<(), Report<StoreError>> {
        // Given a move to a new worktree waiting for trust, still untrusted.
        let (store, id) = store_with_thread("aa")?;
        let (host, git) = (FakeHost::untrusted(2, Ok("bb")), FakeGit::local());
        let (mut actor, _state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));
        actor.move_thread(id, Workspace::NewWorktree).await;

        // When retrying the start.
        actor.retry_start().await;

        // Then the worktree orb made is force-removed.
        let (path, _branch) = git
            .added()
            .ok_or_else(|| Report::new(StoreError).attach("no worktree was added"))?;
        assert!(
            git.calls()
                .contains(&GitCall::RemoveWorktree { path, force: true }),
            "a failed retry should remove the worktree made for it"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn retry_refused_again_does_not_ask_for_trust() -> Result<(), Report<StoreError>> {
        // Given a draft start waiting for trust, and the directory still untrusted.
        let (store, project) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::untrusted(2, Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.start_draft(project).await;

        // When retrying the start.
        actor.retry_start().await;

        // Then no trust is asked for again.
        assert_eq!(
            trust_of(&state),
            None,
            "a retry should never reopen the trust prompt"
        );
        Ok(())
    }

    /// A ref named `name`, on a remote when `remote`, checked out nowhere.
    fn git_ref(name: &str, remote: bool) -> GitRef {
        GitRef {
            name: name.to_owned(),
            remote,
            current: false,
            default: false,
            worktree: None,
        }
    }

    fn branch_of(state: &State, id: ThreadId) -> Option<String> {
        shown(state, id)?.branch
    }

    #[rstest::rstest]
    fn switch_branch_checks_out_and_shows_the_branch() -> Result<(), Report<StoreError>> {
        // Given a thread in the orb project's root.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When switching it to `feat`.
        actor.check_out(id, &git_ref("feat", false));

        // Then the sidebar shows it on feat.
        assert_eq!(
            branch_of(&state, id).as_deref(),
            Some("feat"),
            "the thread should show its new branch"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn switch_branch_refused_after_the_directory_became_busy()
    -> Result<(), Report<StoreError>> {
        // Given a thread whose turn started after the picker opened.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When switching it to `feat`.
        actor.check_out(id, &git_ref("feat", false));

        // Then the mode line says Claude is working there.
        assert_eq!(
            error_of(&state).as_deref(),
            Some("Claude is working in this directory"),
            "a checkout under a running turn should be refused"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn switch_to_a_remote_ref_shows_the_local_branch_name() -> Result<(), Report<StoreError>> {
        // Given a thread in the orb project's root.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When switching it to `origin/feat`.
        actor.check_out(id, &git_ref("origin/feat", true));

        // Then the sidebar shows the tracking branch feat.
        assert_eq!(
            branch_of(&state, id).as_deref(),
            Some("feat"),
            "a remote ref checks out as its local branch"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn unstarted_default_pick_from_a_worktree_moves_to_the_root()
    -> Result<(), Report<StoreError>> {
        // Given a prompt-less thread in an orb worktree of a project on disk.
        let root = tempfile::tempdir().change_context(StoreError)?;
        let store = Store::open_in_memory()?;
        let id = {
            let project_id = store.add_project(root.path(), "orb", 0)?;
            store.insert_thread(&NewThread {
                project_id,
                short_id: "aa".to_owned(),
                cwd: Path::new(WORKTREES_ROOT).join("orb/orb-0123abcd"),
                created_at: now_ms() - HOUR_MS,
                model: None,
                permission_mode: None,
            })?
        };
        let host = FakeHost::moving(Ok("bb"));
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When picking the default branch, checked out nowhere.
        actor.switch_to_root(id, &git_ref("main", false)).await;

        // Then its session starts over in the root.
        assert_eq!(
            host.created_in(),
            vec![root.path().to_owned()],
            "the thread should move back to the root checkout"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn turn_end_renames_the_hex_branch_to_the_title_slug() -> Result<(), Report<StoreError>> {
        // Given a thread in an orb worktree on its hex branch, titled by Claude.
        let (claude_dir, store, id) =
            worktree_thread(&format!("{PROMPT_LINE}{AI_TITLE_LINE}"), HEX_BRANCH)?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, state) = start_with(store, &host, &FakeGit::local(), claude_dir.path());

        // When a poll sees its turn end.
        end_turn(&mut actor, &host).await;

        // Then the thread shows the branch named after its title.
        assert_eq!(
            shown(&state, id)
                .and_then(|thread| thread.branch)
                .as_deref(),
            Some("orb/parser-fix"),
            "the hex branch should be renamed to the title's slug"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn no_rename_when_the_branch_is_not_the_hex_branch() -> Result<(), Report<StoreError>> {
        // Given a Claude-titled thread in an orb worktree on another branch.
        let (claude_dir, store, _) =
            worktree_thread(&format!("{PROMPT_LINE}{AI_TITLE_LINE}"), "main")?;
        let host = FakeHost::listing(Vec::new());
        let git = FakeGit::local();
        let (mut actor, _state) = start_with(store, &host, &git, claude_dir.path());

        // When a poll sees its turn end.
        end_turn(&mut actor, &host).await;

        // Then no branch was renamed.
        assert_eq!(git.renamed(), None, "only orb's hex branch is renamed");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn no_rename_with_only_a_prompt_title() -> Result<(), Report<StoreError>> {
        // Given a thread on its hex branch titled only by its first prompt.
        let (claude_dir, store, _) = worktree_thread(PROMPT_LINE, HEX_BRANCH)?;
        let host = FakeHost::listing(Vec::new());
        let git = FakeGit::local();
        let (mut actor, _state) = start_with(store, &host, &git, claude_dir.path());

        // When a poll sees its turn end.
        end_turn(&mut actor, &host).await;

        // Then no branch was renamed.
        assert_eq!(git.renamed(), None, "a prompt title isn't enough to rename");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn no_rename_when_the_slug_branch_exists() -> Result<(), Report<StoreError>> {
        // Given a Claude-titled thread on its hex branch, and `orb/parser-fix` taken.
        let (claude_dir, store, _) =
            worktree_thread(&format!("{PROMPT_LINE}{AI_TITLE_LINE}"), HEX_BRANCH)?;
        let host = FakeHost::listing(Vec::new());
        let git = FakeGit::having("orb/parser-fix");
        let (mut actor, _state) = start_with(store, &host, &git, claude_dir.path());

        // When a poll sees its turn end.
        end_turn(&mut actor, &host).await;

        // Then no branch was renamed.
        assert_eq!(
            git.renamed(),
            None,
            "an existing slug branch keeps the old name"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn no_rename_without_a_turn_end() -> Result<(), Report<StoreError>> {
        // Given a Claude-titled thread on its hex branch.
        let (claude_dir, store, _) =
            worktree_thread(&format!("{PROMPT_LINE}{AI_TITLE_LINE}"), HEX_BRANCH)?;
        let host = FakeHost::listing(vec![in_session(ThreadStatus::Working)]);
        let git = FakeGit::local();
        let (mut actor, _state) = start_with(store, &host, &git, claude_dir.path());

        // When polls see its turn still underway.
        actor.poll().await;
        actor.poll().await;

        // Then no branch was renamed.
        assert_eq!(git.renamed(), None, "the rename waits for the turn to end");
        Ok(())
    }
}
