//! The sessions actor — the owner of orb's threads, their statuses, and titles.
//!
//! At start it restores the saved threads into the shared state. Then it polls
//! the harnesses: every second while a turn is underway in any agent pane,
//! every five seconds otherwise, and right away when asked. Each poll reads
//! what each thread's agent is doing from the thread's pane: Stopped when the
//! pane's zmx session isn't running, else the latest report a harness like pi
//! wrote to the pane file, or the status of the Claude whose process runs
//! under the pane (by process ancestry from the pane's zmx process). A thread
//! with no pane is Stopped. Then it stamps when a turn starts, reads
//! new transcript lines for titles and branches, and saves what changed.
//!
//! It makes new sessions: a session row with one tab running a shell, in a
//! project's checkout, a worktree it already has, a new worktree made from a
//! base branch, or orb's Incognito folder (made first when missing). The new
//! session is selected and the frontend asked to attach it. A failure shows
//! the reason and removes the worktree and branch orb made for it. Whether
//! each project is a git repository is found at start, on add and after
//! Initialize Git.
//!
//! Before a session's first agent turn it can move the session to another
//! workspace: the project's checkout, a worktree that already exists, or a
//! new one made from a base branch. Every pane is killed, the threads in
//! them unbound, and the session and its panes point at the new directory;
//! the frontend then attaches it again, starting a fresh shell in each pane.
//! A worktree orb made for a move that failed is removed again, and an orb
//! worktree no session uses after a move is removed unless it has changes.
//! When a turn
//! ends in a worktree still on orb's `orb/<hex>` branch, and the harness or
//! the user has titled the thread, the branch is renamed after that title.
//!
//! When the frontend finds a thread's orb worktree gone as it attaches, the
//! actor recreates it at the same path, so the session resumes the conversation
//! there. It prunes git's record of the old worktree, then checks out the
//! branch the worktree was on: the session's, else one of its threads', else
//! orb's `orb/<hex>` for the directory.
//! When that branch is gone it makes it again from the project's default
//! branch, fetched first from origin. Then it asks the frontend to attach, or
//! shows git's reason.
//!
//! It checks branches out in a session's directory, unless a turn is
//! underway there. Before the first agent turn, picking the default branch
//! from a worktree checks it out in the project's checkout and moves the
//! session there. It makes a project a git repository when asked.
//!
//! At start it shows every harness by its name and mark.
//!
//! A thread whose harness orb doesn't know shows as Gone.
//!
//! A turn that ends while the user is on another session shows as unseen
//! until they select it. A session being deleted is hidden from the sidebar
//! until it is removed.
//!
//! It follows each session's settle lifecycle on its store row: a turn ending
//! in any of its agent panes is its latest activity, a turn underway
//! un-settles it, and a session idle for three days settles itself unless it
//! is pinned, was just un-settled, or is the one the sidebar's cursor is on.
//! Settling a session stops nothing.
//!
//! It makes Research and Learn sessions: each in a new folder under orb's
//! own project for its kind, added when first needed, copied from the user's
//! template for the kind; orb writes that template from its built-in default
//! when it's missing, and never overwrites a folder that already exists.
//!
//! It adds projects and removes them: a removed project leaves `<C-g> n` and the
//! project filter, its sessions stay, and adding it again
//! restores it.
//!
//! It restores the sidebar's saved width and project filter at start, and
//! saves them when asked. It does the same for the jump list, dropping saved
//! rows that no longer exist.
//!
//! It queues a notice for the frontend to announce when a thread it has
//! polled since orb started finishes a turn, or starts needing an approval or
//! an answer, in any project, unless the thread is being deleted.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use error_stack::Report;
use kameo::mailbox;
use kameo::prelude::{Actor, ActorRef, Context, Message, Reply, Spawn};
use tokio::sync::Notify;

use super::pane_status::{PaneReport, match_records, pane_status};
use super::state::{
    FolderKind, NEW_THREAD, Notice, NoticeKind, PaneId, PaneLaunch, Project, ProjectId,
    ProjectKind, Session, SessionId, SessionKind, Sessions, SidebarItem, Thread, ThreadId,
    ThreadStatus,
};
use super::store::{
    NewPaneThread, PaneRow, SessionRow, SettledOverride, Store, StoreError, TabRow, ThreadRow, Ui,
};
use super::template;
use super::validator::folder_exists;
use crate::command::Workspace;
use crate::common::{Services, State, Wake};
use crate::feat::git::git_service::{GitError, GitRef, GitService, git_reason};
use crate::feat::git::validator::BUSY_DIRECTORY;
use crate::feat::git::worktree::{hex, hex_branch, is_orb_worktree, new_worktree_path, slug};
use crate::feat::harness::{
    Harness, HarnessError, HarnessId, RunningAgent, Scan, TranscriptFormat, claude,
};
use crate::feat::integration::pane_file::{AgentEvent, AgentReport, PaneFiles};
use crate::feat::integration::{NUDGE, panes_dir};
use crate::feat::jumps::state::JumpList;
use crate::feat::layout::state::{PaneEntry, SessionLayout, Tab};
use crate::feat::layout::tree::{Split, TileLayout};
use crate::feat::sidebar::state::{DEFAULT_WIDTH, clamp_width};
use crate::feat::zmx::zmx_service::{ZmxEntry, ZmxSession};
use crate::{AppState, Focus};

/// How long to wait between polls while a turn is underway in any agent pane.
const FAST_POLL: Duration = Duration::from_secs(1);
/// How long to wait between polls otherwise.
const SLOW_POLL: Duration = Duration::from_secs(5);
/// How long an unpinned session stays idle before it settles itself, in ms.
const AUTO_SETTLE_AFTER: i64 = 3 * 24 * 60 * 60 * 1000;
/// The error shown when orb's store can't be written.
const SAVE_FAILED: &str = "couldn't save orb's state";
/// The error shown when a session's folder or the Incognito folder can't be
/// made.
const FOLDER_UNMADE: &str = "couldn't make the folder";
/// The error shown when a deleted session's folder can't be removed.
const FOLDER_UNREMOVED: &str = "couldn't remove the folder";
/// The error shown when a started session can't be saved or shown.
const NEW_SESSION_UNSAVED: &str = "couldn't save the new session";
/// How many random worktree names to try before giving up.
const WORKTREE_NAME_ATTEMPTS: u32 = 8;

/// What the sessions actor needs to start.
pub struct SessionsActorDeps {
    pub services: Services,
    pub state: State,
    pub store: Store,
    /// Where orb makes new worktrees.
    pub worktrees_root: PathBuf,
    /// orb's own folder (`~/.orb`): the Research/Learn roots and templates.
    pub orb_root: PathBuf,
    /// orb's Incognito project's root (`/tmp/orb-incognito`), made at start
    /// and before each Incognito start.
    pub incognito_root: PathBuf,
    /// Tells the frontend to redraw.
    pub wake: Wake,
    /// Neither orb's Claude hook nor its pi extension is installed; the mode
    /// line says how to install them.
    pub integration_missing: bool,
}

/// Owns [`Sessions`](super::state::Sessions) and the harnesses the frontend
/// shows (`AppState::harnesses`): the projects, the
/// sessions with their names, pins and settles, the threads' statuses and
/// titles, the latest error, the origin ref a start is fetching, and the
/// new, moved or restored session the frontend should attach to. It adds orb's Incognito project at
/// start. It also restores the sidebar's width and project filter, and
/// selects a new session. It loads every session's
/// layout at start, adds the panes splits and new tabs make (it saves their
/// store rows, so it gives them their ids), lays out a new session, drops a
/// deleted
/// session's layout, and saves a layout when asked. The intent handler also
/// moves the cursor, opens and closes the shelf, marks a start as starting,
/// and resizes or
/// filters the sidebar before asking for that to be saved. It binds threads
/// to panes from the agents' pane files, keeps each pane's latest report and
/// resume command current, and owns each session's name, pin, settle and
/// activity state on its store row.
pub struct SessionsActor {
    services: Services,
    state: State,
    store: Store,
    worktrees_root: PathBuf,
    orb_root: PathBuf,
    wake: Wake,
    /// Cuts the ticker's wait short so it polls now.
    poke: Arc<Notify>,
    /// The saved threads, as last written to the store.
    rows: Vec<ThreadRow>,
    /// The saved panes, as last written to the store.
    panes: HashMap<PaneId, PaneRow>,
    /// Each saved session: its directory, where its new panes start, and
    /// its settle lifecycle.
    sessions: HashMap<SessionId, SessionRow>,
    /// The agents' reports of what runs in each pane.
    pane_files: PaneFiles,
    /// Each pane's latest agent report.
    pane_reports: HashMap<PaneId, PaneReport>,
}

/// Poll the harnesses now.
#[derive(Debug)]
pub struct Poll;

/// Split `session`'s focused pane `split` with a new shell pane, and save
/// the layout.
#[derive(Debug)]
pub struct SplitPane {
    pub session: SessionId,
    pub split: Split,
}

/// Open a tab of one new shell pane in a session, its first when it has no
/// layout, and save the layout.
#[derive(Debug)]
pub struct NewTab(pub SessionId);

/// Save a session's tabs, splits, focus and pane names as the app state has
/// them.
#[derive(Debug)]
pub struct SaveLayout(pub SessionId);

/// Stop the Claude `--bg` sessions the store's migration to sessions turned
/// into panes, so they don't fight the `claude --resume` those panes run.
/// orb waits for it before the frontend starts, so no pane types
/// `claude --resume` while its `--bg` session still runs. The function is
/// told how many sessions are about to be stopped, once, when any are.
#[derive(Debug)]
pub struct StopMigrated(pub fn(usize));

/// Kill the panes zmx still runs for sessions that are already settled.
#[derive(Debug)]
pub struct KillSettled;

/// How long to wait before the next [`Poll`].
#[derive(Debug, Reply)]
pub struct NextPoll(pub Duration);

/// Make a session of `project` in `workspace`, with one shell pane.
#[derive(Debug)]
pub struct NewSession {
    pub project: ProjectId,
    pub workspace: Workspace,
}

/// Make a project's root a git repository.
#[derive(Debug)]
pub struct InitGit(pub ProjectId);

/// Move a session, before its first agent turn, to another workspace.
#[derive(Debug)]
pub struct ChangeWorkspace {
    pub session: SessionId,
    pub to: Workspace,
}

/// Check a branch out for a session: in its directory, or with `to_root` in
/// its project's checkout, moving the session there.
#[derive(Debug)]
pub struct SwitchBranch {
    pub session: SessionId,
    pub git_ref: GitRef,
    pub to_root: bool,
}

/// Add a directory as a project.
#[derive(Debug)]
pub struct AddProject(pub PathBuf);

/// Poll now instead of waiting for the next tick.
#[derive(Debug)]
pub struct RefreshSessions;

/// Pin a session to the top of the sidebar, un-settling it if needed.
#[derive(Debug)]
pub struct PinSession(pub SessionId);

/// Unpin a session.
#[derive(Debug)]
pub struct UnpinSession(pub SessionId);

/// Give a session a name, or with `None` go back to its agents' or
/// directory's.
#[derive(Debug)]
pub struct RenameSession {
    pub session: SessionId,
    pub name: Option<String>,
}

/// Settle a session onto the Settled shelf.
#[derive(Debug)]
pub struct SettleSession(pub SessionId);

/// Take a session off the Settled shelf and keep it active until its next
/// turn.
#[derive(Debug)]
pub struct UnsettleSession(pub SessionId);

/// Delete a session: its panes, tabs and threads.
#[derive(Debug)]
pub struct DeleteSession(pub SessionId);

/// The user selected a session, so its agents' latest turns are seen.
#[derive(Debug)]
pub struct Visit(pub SessionId);

/// Save the sidebar's width and project filter as they now are in the app
/// state.
#[derive(Debug)]
pub struct SaveUi;

/// Save the jump list as it now is in the app state.
#[derive(Debug)]
pub struct SaveJumps;

/// Remove a project from `<C-g> n` and the project filter.
#[derive(Debug)]
pub struct RemoveProject(pub ProjectId);

/// Make a `kind` session with one shell in orb's own folder `name` (a slug).
#[derive(Debug)]
pub struct NewFolderSession {
    pub kind: FolderKind,
    pub name: String,
}

/// Recreate session `.0`'s orb worktree, gone from disk, then attach to it.
#[derive(Debug)]
pub struct RestoreWorktree(pub SessionId);

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
        {
            let actor_ref = actor_ref.clone();
            tokio::spawn(async move {
                let _ = actor_ref.tell(KillSettled).await;
            });
        }
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

impl Message<SplitPane> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        msg: SplitPane,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.add_pane(msg.session, Some(msg.split));
    }
}

impl Message<NewTab> for SessionsActor {
    type Reply = ();

    async fn handle(&mut self, msg: NewTab, _ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.add_pane(msg.0, None);
    }
}

impl Message<SaveLayout> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        msg: SaveLayout,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.save_layout(msg.0);
    }
}

impl Message<StopMigrated> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        msg: StopMigrated,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.stop_migrated(msg.0).await;
    }
}

impl Message<KillSettled> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        _msg: KillSettled,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.kill_settled();
    }
}

impl Message<Poll> for SessionsActor {
    type Reply = NextPoll;

    async fn handle(&mut self, _msg: Poll, _ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        NextPoll(self.poll().await)
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

impl Message<NewSession> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        NewSession { project, workspace }: NewSession,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.new_session(project, workspace);
    }
}

impl Message<ChangeWorkspace> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        ChangeWorkspace { session, to }: ChangeWorkspace,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.change_workspace(session, to);
    }
}

impl Message<SwitchBranch> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        SwitchBranch {
            session,
            git_ref,
            to_root,
        }: SwitchBranch,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.switch_branch(session, &git_ref, to_root);
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

impl Message<PinSession> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        PinSession(id): PinSession,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.pin_session(id);
    }
}

impl Message<UnpinSession> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        UnpinSession(id): UnpinSession,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.unpin_session(id);
    }
}

impl Message<RenameSession> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        RenameSession { session, name }: RenameSession,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.rename_session(session, name);
    }
}

impl Message<SettleSession> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        SettleSession(id): SettleSession,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.settle_session(id);
    }
}

impl Message<UnsettleSession> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        UnsettleSession(id): UnsettleSession,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.unsettle_session(id);
    }
}

impl Message<DeleteSession> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        DeleteSession(id): DeleteSession,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.delete_session(id);
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

impl Message<SaveJumps> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        _msg: SaveJumps,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.save_jumps();
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

impl Message<NewFolderSession> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        NewFolderSession { kind, name }: NewFolderSession,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.new_folder_session(kind, &name);
    }
}

impl Message<RestoreWorktree> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        RestoreWorktree(id): RestoreWorktree,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.restore_worktree(id);
    }
}

impl SessionsActor {
    /// Shows the saved projects and threads, sizes the sidebar as it
    /// was saved, kept within its bounds, filters it to the saved project if
    /// that's still shown and not removed, and selects its first row. Each
    /// project learns whether it is a git repository. The saved jump list comes back
    /// without the rows that no longer exist, and every session's layout comes
    /// back as it was saved. orb's Incognito project is added
    /// (or un-removed) and its folder made first.
    #[expect(
        clippy::too_many_lines,
        reason = "brings back every saved piece of the sidebar and layouts in one place"
    )]
    fn restore(deps: SessionsActorDeps) -> Self {
        let SessionsActorDeps {
            services,
            state,
            store,
            worktrees_root,
            orb_root,
            incognito_root,
            wake,
            integration_missing,
        } = deps;
        let _ = fs::create_dir_all(&incognito_root);
        let added = store.add_project(
            &incognito_root,
            "Incognito",
            ProjectKind::Incognito,
            now_ms(),
        );
        let RestoredLayouts {
            panes,
            sessions: session_rows,
            layouts,
            error: layouts_error,
        } = restore_layouts(&store, &services, &orb_root);
        let (projects, rows, error) = match store.load() {
            Ok((projects, rows)) => (
                projects,
                rows,
                added.err().map(|_report| SAVE_FAILED.to_owned()),
            ),
            Err(_) => (
                Vec::new(),
                Vec::new(),
                Some("couldn't load orb's saved sessions".to_owned()),
            ),
        };
        let error = error.or_else(|| layouts_error.map(str::to_owned));
        let projects: Vec<Project> = projects
            .into_iter()
            .map(|project| Project {
                threads: rows
                    .iter()
                    .filter(|row| row.project_id == project.id)
                    .map(|row| unpolled(&services, row, &panes))
                    .collect(),
                // ponytail: one git call per project at start; cache it if
                // many projects make start-up slow.
                repo: project.kind == ProjectKind::Normal
                    && services.git.project_path(&project.root).is_some(),
                id: project.id,
                title: project.title,
                root: project.root,
                created_at: from_ms(project.created_at),
                removed: project.removed_at.is_some(),
                kind: project.kind,
            })
            .collect();
        let ui = store.ui().unwrap_or_default();
        let filter = ui.project_filter.filter(|&id| {
            projects
                .iter()
                .any(|project| project.id == id && !project.removed)
        });
        let jumps = {
            let saved = store.jumps().unwrap_or_default();
            JumpList::from_saved(
                saved
                    .into_iter()
                    .filter(|&item| exists(&session_rows, item))
                    .collect(),
            )
        };
        {
            let mut app = state.write();
            app.sidebar.width = ui.sidebar_width.map_or(DEFAULT_WIDTH, clamp_width);
            app.jumps = jumps;
            app.harnesses = services.harnesses.infos();
            for (session, layout) in layouts {
                app.layouts.insert(session, layout);
            }
            let sessions = &mut app.sessions;
            sessions.projects = projects;
            sessions.sessions = {
                let mut shown: Vec<Session> = session_rows.values().map(show_session).collect();
                shown.sort_by_key(|session| session.id.0);
                shown
            };
            sessions.cursor = None;
            sessions.filter_to(filter);
            sessions.error = error.or_else(|| integration_missing.then(|| NUDGE.to_owned()));
        }
        wake();
        Self {
            services,
            state,
            store,
            worktrees_root,
            pane_files: PaneFiles::new(panes_dir(&orb_root)),
            orb_root,
            wake,
            poke: Arc::default(),
            rows,
            panes,
            sessions: session_rows,
            pane_reports: HashMap::new(),
        }
    }

    /// Asks the harnesses what every agent is doing and shows it. Returns how
    /// long to wait before the next poll: a second while any agent pane has a
    /// turn underway.
    async fn poll(&mut self) -> Duration {
        self.follow_pane_files();
        if let Err(report) = self.sync().await {
            self.fail(&report);
        }
        let app = self.state.read();
        if app.sessions.any_in_progress() {
            FAST_POLL
        } else {
            SLOW_POLL
        }
    }

    /// Applies the agents' reports written since the last poll, oldest first.
    /// A failed save shows on the mode line.
    fn follow_pane_files(&mut self) {
        let mut failed = false;
        for (pane, report) in self.pane_files.changed() {
            self.pane_reports.insert(
                pane,
                PaneReport {
                    event: report.event,
                    at: report.at,
                },
            );
            failed |= self.follow_report(pane, &report).is_err();
        }
        if failed {
            self.state.write().sessions.error = Some(SAVE_FAILED.to_owned());
            (self.wake)();
        }
    }

    /// What `report` from `pane` means for the threads:
    /// - `end` of the pane's current conversation: the thread leaves the pane,
    ///   and the pane has nothing to resume;
    /// - anything else names the conversation now in the pane: its thread
    ///   keeps the pane (transcript refreshed), or the current thread takes
    ///   the new id after Claude's `/clear` or pi's `/new`, or the thread
    ///   holding that id moves in, or a new thread starts; the pane's resume
    ///   command then names it.
    ///
    /// Reports from an agent orb has no harness for, or a pane orb has no row
    /// for, are ignored, and so is an `end` from a settled session's pane:
    /// settling ended it, and its thread and resume command stay for the
    /// session to come back.
    fn follow_report(
        &mut self,
        pane: PaneId,
        report: &AgentReport,
    ) -> Result<(), Report<StoreError>> {
        let Some(cwd) = self.panes.get(&pane).map(|row| row.cwd.clone()) else {
            return Ok(());
        };
        let id = HarnessId::new(report.agent.clone());
        let Some(harness) = self.services.harnesses.get(&id).cloned() else {
            return Ok(());
        };
        let sid = report.session_id.as_str();
        let holds = |row: &ThreadRow| row.short_id == sid || row.session_id.as_deref() == Some(sid);
        let current = self.rows.iter().position(|row| row.pane_id == Some(pane));
        let settled = self
            .panes
            .get(&pane)
            .and_then(|row| self.sessions.get(&row.session_id))
            .is_some_and(|session| session.settled_override == Some(SettledOverride::Settled));
        if report.event == AgentEvent::End && settled {
            return Ok(());
        }
        if report.event == AgentEvent::End {
            let ended = match current.and_then(|i| self.rows.get_mut(i)) {
                Some(row) if holds(row) => {
                    row.pane_id = None;
                    self.store.save_thread(row)?;
                    true
                }
                _ => false,
            };
            return if ended {
                self.set_resume(pane, None)
            } else {
                Ok(())
            };
        }
        self.set_resume(pane, Some(&harness.resume_command(sid)))?;
        let holder = self.rows.iter().position(holds);
        let swaps = current
            .and_then(|i| self.rows.get(i))
            .is_some_and(|row| report.replaces_conversation() && row.harness == id);
        match (current, holder) {
            (Some(i), Some(j)) if i == j => {
                if let Some(row) = self.rows.get_mut(i) {
                    let before = row.clone();
                    take_transcript(row, report.transcript.as_ref());
                    if *row != before {
                        self.store.save_thread(row)?;
                    }
                }
            }
            (Some(i), _) if swaps => {
                if let Some(row) = self.rows.get_mut(i) {
                    take_session(row, sid);
                    row.transcript_path.clone_from(&report.transcript);
                    row.transcript_offset = 0;
                    self.store.save_thread(row)?;
                }
            }
            (current, Some(j)) => {
                let old_pane = self
                    .rows
                    .get(j)
                    .and_then(|row| row.pane_id)
                    .filter(|&old| old != pane);
                self.unbind(current)?;
                let session = self.panes.get(&pane).map(|row| row.session_id);
                if let Some(row) = self.rows.get_mut(j) {
                    row.pane_id = Some(pane);
                    row.orb_session = session;
                    take_session(row, sid);
                    take_transcript(row, report.transcript.as_ref());
                    self.store.save_thread(row)?;
                }
                if let Some(old) = old_pane {
                    self.set_resume(old, None)?;
                }
            }
            (current, None) => {
                self.unbind(current)?;
                self.start_pane_thread(pane, cwd, id, report)?;
            }
        }
        Ok(())
    }

    /// Takes the thread at `index` in the rows, if any, out of its pane.
    fn unbind(&mut self, index: Option<usize>) -> Result<(), Report<StoreError>> {
        if let Some(row) = index.and_then(|i| self.rows.get_mut(i)) {
            row.pane_id = None;
            self.store.save_thread(row)?;
        }
        Ok(())
    }

    /// Saves and shows a thread `report` started in `pane`, in the project of
    /// the pane's session, at the top of its threads.
    fn start_pane_thread(
        &mut self,
        pane: PaneId,
        cwd: PathBuf,
        harness: HarnessId,
        report: &AgentReport,
    ) -> Result<(), Report<StoreError>> {
        let now = now_ms();
        let (id, project_id) = self.store.insert_pane_thread(&NewPaneThread {
            pane,
            session_id: report.session_id.clone(),
            transcript_path: report.transcript.clone(),
            harness: harness.clone(),
            cwd: cwd.clone(),
            created_at: now,
        })?;
        let row = ThreadRow {
            id,
            project_id,
            short_id: report.session_id.clone(),
            session_id: Some(report.session_id.clone()),
            title: None,
            custom_title: None,
            cwd,
            transcript_path: report.transcript.clone(),
            transcript_offset: 0,
            created_at: now,
            turn_started_at: None,
            branch: None,
            last_activity_at: now,
            last_visited_at: now,
            ai_titled: false,
            model: None,
            renamed_title: None,
            harness,
            pane_id: Some(pane),
            orb_session: self.panes.get(&pane).map(|row| row.session_id),
        };
        let thread = unpolled(&self.services, &row, &self.panes);
        self.rows.push(row);
        if let Some(project) = self
            .state
            .write()
            .sessions
            .projects
            .iter_mut()
            .find(|project| project.id == project_id)
        {
            project.threads.insert(0, thread);
        }
        (self.wake)();
        Ok(())
    }

    /// Saves pane `pane`'s resume command, unless it already has it, and
    /// shows it in the pane's layout entry.
    fn set_resume(&mut self, pane: PaneId, resume: Option<&str>) -> Result<(), Report<StoreError>> {
        let Some(row) = self.panes.get_mut(&pane) else {
            return Ok(());
        };
        if row.resume.as_deref() != resume {
            self.store.set_pane_resume(pane, resume)?;
            row.resume = resume.map(str::to_owned);
            self.state
                .write()
                .layouts
                .remember_resume(pane, row.resume.clone());
        }
        Ok(())
    }

    /// Asks the harnesses what every agent is doing and shows it.
    /// A harness that can't be asked leaves its threads as they were; the
    /// others still update, and the failure is returned after.
    async fn sync(&mut self) -> Result<(), Report<HarnessError>> {
        let mut listed: HashMap<HarnessId, Vec<RunningAgent>> = HashMap::new();
        let mut failure = None;
        for harness in self.services.harnesses.all() {
            match harness.running().await {
                Ok(agents) => {
                    listed.insert(harness.id(), agents);
                }
                Err(report) => failure = Some(report),
            }
        }
        let live = self.live(&listed);
        self.apply(&listed, &live);
        failure.map_or(Ok(()), Err)
    }

    /// What the poll sees of the panes threads run in: each one's zmx session
    /// (one `zmx list` per socket dir; a dir zmx can't list leaves its panes
    /// out) and the agents matched to the panes their processes run under.
    fn live(&self, listed: &HashMap<HarnessId, Vec<RunningAgent>>) -> Live {
        let bound: Vec<(PaneId, ZmxSession)> = self
            .rows
            .iter()
            .filter_map(|row| self.panes.get(&row.pane_id?))
            .map(|pane| {
                (
                    pane.id,
                    pane_entry(&self.services, &self.orb_root, pane).zmx,
                )
            })
            .collect();
        let dirs: HashMap<&Path, Option<Vec<ZmxEntry>>> = bound
            .iter()
            .map(|(_, zmx)| zmx.dir.as_path())
            .collect::<HashSet<&Path>>()
            .into_iter()
            .map(|dir| (dir, self.services.zmx.list(dir).ok()))
            .collect();
        let sessions: HashMap<PaneId, Option<ZmxEntry>> = bound
            .iter()
            .filter_map(|(pane, zmx)| {
                let entries = dirs.get(zmx.dir.as_path())?.as_ref()?;
                Some((
                    *pane,
                    entries.iter().find(|entry| entry.name == zmx.name).cloned(),
                ))
            })
            .collect();
        let roots: HashMap<u32, PaneId> = sessions
            .iter()
            .filter_map(|(pane, entry)| Some((entry.as_ref()?.pid?, *pane)))
            .collect();
        let agents: Vec<RunningAgent> = listed.values().flatten().cloned().collect();
        Live {
            matched: match_records(&agents, &roots),
            sessions,
        }
    }

    /// Updates every saved thread from its pane and transcript, follows its
    /// activity and its session's settle lifecycle, saves the ones that
    /// changed, then shows them all in one write. A thread whose harness
    /// wasn't `listed` (it couldn't be asked) is left as it was; one whose
    /// harness orb doesn't know is Gone.
    fn apply(&mut self, listed: &HashMap<HarnessId, Vec<RunningAgent>>, live: &Live) {
        let now = now_ms();
        let selected = match self.state.read().sessions.cursor {
            Some(SidebarItem::Session(id)) => Some(id),
            _ => None,
        };
        let mut statuses = Vec::with_capacity(self.rows.len());
        let mut error = None;
        for row in &mut self.rows {
            let harness = self.services.harnesses.get(&row.harness);
            if harness.is_some() && !listed.contains_key(&row.harness) {
                statuses.push(None);
                continue;
            }
            let Some(status) = status_now(row, harness, live, &self.pane_reports) else {
                statuses.push(None);
                continue;
            };
            let before = row.clone();
            let was_in_progress = row.turn_started_at.is_some();
            let format = harness.map(|harness| harness.as_ref() as &dyn TranscriptFormat);
            let asks_git = update_row(row, status, now, format);
            if was_in_progress && !status.in_progress() {
                if asks_git && let Some(branch) = current_branch(&self.services.git, &row.cwd) {
                    row.branch = Some(branch);
                }
                rename_hex_branch(&self.services.git, &self.worktrees_root, row);
            }
            let in_selected = selected.is_some()
                && row
                    .pane_id
                    .and_then(|pane| self.panes.get(&pane))
                    .map(|pane| pane.session_id)
                    == selected;
            follow_activity(row, status, was_in_progress, in_selected, now);
            if *row != before && self.store.save_thread(row).is_err() {
                error = Some(SAVE_FAILED.to_owned());
            }
            statuses.push(Some(status));
        }
        let changed_sessions = self.follow_sessions(&statuses, selected, now, &mut error);
        let changed = {
            let mut app = self.state.write();
            let sessions = &mut app.sessions;
            let mut changed = false;
            if let Some(error) = error {
                changed = sessions.error.as_ref() != Some(&error);
                sessions.error = Some(error);
            }
            for (row, status) in self
                .rows
                .iter()
                .zip(statuses)
                .filter_map(|(row, status)| Some((row, status?)))
            {
                if let Some(notice) = notice(sessions, row, status) {
                    sessions.notices.push(notice);
                    changed = true;
                }
                if let Some(thread) = thread_mut(sessions, row.id) {
                    changed |= show(thread, row, status, pane_launch(row, &self.panes));
                }
            }
            for id in changed_sessions {
                if let (Some(row), Some(shown)) = (
                    self.sessions.get(&id),
                    sessions.sessions.iter_mut().find(|shown| shown.id == id),
                ) {
                    *shown = show_session(row);
                    changed = true;
                }
            }
            changed
        };
        if changed {
            (self.wake)();
        }
    }

    /// Follows each session's settle lifecycle (see [`follow_session`]) from
    /// its agent threads' polled rows and `statuses` (index-aligned with the
    /// rows; `None` where a thread wasn't polled, which leaves its session as
    /// it was), saving the ones that changed. `selected` never auto-settles.
    /// A session that auto-settles is detached and its panes killed. Sets
    /// `error` when a save fails. Returns the sessions that changed.
    fn follow_sessions(
        &mut self,
        statuses: &[Option<ThreadStatus>],
        selected: Option<SessionId>,
        now: i64,
        error: &mut Option<String>,
    ) -> Vec<SessionId> {
        let mut changed = Vec::new();
        let mut auto_settled = Vec::new();
        for session in self.sessions.values_mut() {
            let Some(children) = self
                .rows
                .iter()
                .zip(statuses)
                .filter(|(row, _)| {
                    row.pane_id
                        .and_then(|pane| self.panes.get(&pane))
                        .is_some_and(|pane| pane.session_id == session.id)
                })
                .map(|(row, status)| Some((row, (*status)?)))
                .collect::<Option<Vec<(&ThreadRow, ThreadStatus)>>>()
            else {
                continue;
            };
            let latest = children
                .iter()
                .map(|(row, _)| row.last_activity_at)
                .fold(session.last_activity_at, i64::max);
            let in_progress = children.iter().any(|(_, status)| status.in_progress());
            let before = session.clone();
            let settled = follow_session(
                session,
                Activity {
                    latest,
                    in_progress,
                    agents: !children.is_empty(),
                },
                selected == Some(session.id),
                now,
            );
            if *session != before {
                if self.store.save_session(session).is_err() {
                    *error = Some(SAVE_FAILED.to_owned());
                }
                changed.push(session.id);
            }
            if settled {
                auto_settled.push(session.id);
            }
        }
        if !auto_settled.is_empty() {
            {
                let mut app = self.state.write();
                for id in &auto_settled {
                    app.attached.remove(id);
                }
            }
            for &id in &auto_settled {
                self.kill_panes(id);
            }
        }
        changed
    }

    /// Runs `git init` in project `id`'s root, then shows the project as a
    /// git repository. A failure shows why.
    fn init_git(&mut self, id: ProjectId) {
        let Some(root) = self.project_root(id) else {
            return;
        };
        let inited = self.services.git.init(&root);
        {
            let mut app = self.state.write();
            let sessions = &mut app.sessions;
            match inited {
                Ok(()) => {
                    if let Some(project) = sessions.projects.iter_mut().find(|p| p.id == id) {
                        project.repo = true;
                    }
                }
                Err(report) => {
                    sessions.error = Some(format!(
                        "Git initialization failed: {}",
                        git_reason(&report)
                    ));
                }
            }
        }
        (self.wake)();
    }

    /// Ends a session start: stops showing it as starting, and shows
    /// `result`'s error, or clears the error and polls the new session now.
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

    /// Makes a session of project `id` in `workspace` with one tab running a
    /// shell, selects it and asks the frontend to attach it. The checkout or
    /// an existing worktree must still be a folder; a new worktree is made
    /// from its base (fetched from origin first, see `add_worktree`); the
    /// Incognito project's folder is made first. On failure the reason
    /// shows, and a worktree orb made for it is removed with its branch.
    fn new_session(&mut self, id: ProjectId, workspace: Workspace) {
        let Some((root, kind)) = self.project_of(id) else {
            return self.end_start(Err("the project is gone".to_owned()));
        };
        if kind == ProjectKind::Incognito && fs::create_dir_all(&root).is_err() {
            return self.end_start(Err(FOLDER_UNMADE.to_owned()));
        }
        let (dir, made) = match self.reach(&root, workspace) {
            Ok(reached) => reached,
            Err(error) => return self.end_start(Err(error)),
        };
        let branch = self.branch_of(&dir, made.as_ref());
        let result = self.save_session(id, session_kind(kind), &dir, branch);
        if result.is_err()
            && let Some(made) = &made
        {
            self.remove_made(made);
        }
        self.end_start(result);
    }

    /// Saves a new `kind` session of project `project` in `dir` on `branch`
    /// with one tab of one shell pane, shows it, selects it and asks the
    /// frontend to attach it. The error is the mode-line text.
    fn save_session(
        &mut self,
        project: ProjectId,
        kind: SessionKind,
        dir: &Path,
        branch: Option<String>,
    ) -> Result<(), String> {
        let now = now_ms();
        let (id, pane) = self
            .store
            .insert_session(project, kind, dir, branch.as_deref(), now)
            .map_err(|_report| NEW_SESSION_UNSAVED.to_owned())?;
        let row = SessionRow {
            id,
            project_id: project,
            kind,
            name: None,
            branch,
            created_at: now,
            dir: dir.to_owned(),
            active_tab: 0,
            pinned_at: None,
            settled_override: None,
            settled_at: None,
            unsettled_at: None,
            last_activity_at: now,
        };
        let pane = PaneRow {
            id: pane,
            session_id: id,
            cwd: dir.to_owned(),
            zmx_name: None,
            zmx_dir: None,
            resume: None,
            migrated_bg: None,
            name: None,
        };
        let entry = pane_entry(&self.services, &self.orb_root, &pane);
        {
            let mut app = self.state.write();
            app.layouts.insert(id, SessionLayout::of(entry));
            let sessions = &mut app.sessions;
            sessions.sessions.push(show_session(&row));
            sessions.cursor = Some(SidebarItem::Session(id));
            sessions.attach = Some(id);
        }
        self.panes.insert(pane.id, pane);
        self.sessions.insert(id, row);
        Ok(())
    }

    /// Project `id`'s root and kind, if it still exists.
    fn project_of(&self, id: ProjectId) -> Option<(PathBuf, ProjectKind)> {
        self.state
            .read()
            .sessions
            .project(id)
            .map(|project| (project.root.clone(), project.kind))
    }

    /// Where `to` is for a session of the project rooted at `root`: the
    /// checkout or an existing worktree, which must still be a folder, or a
    /// new worktree made from its base (fetched from origin first, see
    /// `add_worktree`), with the worktree orb made. The error is the
    /// mode-line text.
    fn reach(&self, root: &Path, to: Workspace) -> Result<(PathBuf, Option<MadeWorktree>), String> {
        match to {
            Workspace::Checkout if root.is_dir() => Ok((root.to_owned(), None)),
            Workspace::Checkout => Err(format!(
                "project folder no longer exists: {}",
                root.display()
            )),
            Workspace::Existing(path) if path.is_dir() => Ok((path, None)),
            Workspace::Existing(path) => {
                Err(format!("worktree no longer exists: {}", path.display()))
            }
            Workspace::NewWorktree { base } => self
                .add_worktree(root, Some(&base), None)
                .map(|made| (made.path.clone(), Some(made)))
                .map_err(|report| git_reason(&report)),
        }
    }

    /// The branch a session in `dir` is on: the one orb `made` there, else
    /// the one checked out, if git can tell.
    fn branch_of(&self, dir: &Path, made: Option<&MadeWorktree>) -> Option<String> {
        made.map_or_else(
            || current_branch(&self.services.git, dir),
            |made| Some(made.branch.clone()),
        )
    }

    /// Moves session `id`, which has had no agent turn, to `to`: the
    /// checkout, an existing worktree, or a new one made from its base. Saves
    /// the session and its panes at the new directory, kills every pane,
    /// unbinds the threads in them and clears their resume commands, and
    /// points the layout at the new directory. The worktree it left is
    /// removed with its branch when orb made it and no other session uses it
    /// (git refuses one with changes). However it ends, the frontend is
    /// asked to attach the session again, which starts a fresh shell in each
    /// pane; a session that has had a turn, or a target that can't be
    /// reached, stays where it was with the reason shown.
    fn change_workspace(&mut self, id: SessionId, to: Workspace) {
        let result = self.move_session(id, to);
        self.state.write().sessions.attach = Some(id);
        self.end_start(result);
    }

    /// The work of [`Self::change_workspace`], up to asking for the attach.
    fn move_session(&mut self, id: SessionId, to: Workspace) -> Result<(), String> {
        let (project, old_dir, old_branch) = self
            .sessions
            .get(&id)
            .map(|row| (row.project_id, row.dir.clone(), row.branch.clone()))
            .ok_or_else(|| "the session is gone".to_owned())?;
        let (root, _) = self
            .project_of(project)
            .ok_or_else(|| "the project is gone".to_owned())?;
        if self.state.read().sessions.turned(id) {
            let workspace = if old_dir == root {
                "Local checkout"
            } else {
                "Worktree"
            };
            return Err(format!("Workspace locked · {workspace}"));
        }
        let (dir, made) = self.reach(&root, to)?;
        let branch = self.branch_of(&dir, made.as_ref());
        if self
            .store
            .move_session(id, &dir, branch.as_deref())
            .is_err()
        {
            if let Some(made) = &made {
                self.remove_made(made);
            }
            return Err(SAVE_FAILED.to_owned());
        }
        self.kill_panes(id);
        let panes: Vec<PaneId> = self
            .panes
            .values()
            .filter(|pane| pane.session_id == id)
            .map(|pane| pane.id)
            .collect();
        let mut saved = Ok(());
        let mut unbound = Vec::new();
        for &pane in &panes {
            for row in self.rows.iter_mut().filter(|row| row.pane_id == Some(pane)) {
                row.pane_id = None;
                unbound.push(row.id);
                if self.store.save_thread(row).is_err() {
                    saved = Err(SAVE_FAILED.to_owned());
                }
            }
            if self.set_resume(pane, None).is_err() {
                saved = Err(SAVE_FAILED.to_owned());
            }
            if let Some(row) = self.panes.get_mut(&pane) {
                row.cwd.clone_from(&dir);
            }
        }
        let shown = self.sessions.get_mut(&id).map(|row| {
            row.dir.clone_from(&dir);
            row.branch.clone_from(&branch);
            show_session(row)
        });
        {
            let mut app = self.state.write();
            app.layouts.move_session(id, &dir);
            let sessions = &mut app.sessions;
            if let (Some(shown), Some(session)) = (
                shown,
                sessions
                    .sessions
                    .iter_mut()
                    .find(|session| session.id == id),
            ) {
                *session = shown;
            }
            for row in self.rows.iter().filter(|row| unbound.contains(&row.id)) {
                if let Some(thread) = thread_mut(sessions, row.id) {
                    let status = thread.status;
                    show(thread, row, status, pane_launch(row, &self.panes));
                }
            }
        }
        self.remove_left(&root, &old_dir, old_branch);
        saved
    }

    /// Removes the orb worktree at `old_dir`, a session just left, and its
    /// branch (the session's `old_branch`, else orb's `orb/<hex>` for it),
    /// unless another session still uses it; git refuses one with changes,
    /// or a branch that isn't merged.
    fn remove_left(&self, root: &Path, old_dir: &Path, old_branch: Option<String>) {
        if !is_orb_worktree(&self.worktrees_root, old_dir)
            || self.sessions.values().any(|row| row.dir == old_dir)
        {
            return;
        }
        let git = &self.services.git;
        if git.remove_worktree(root, old_dir, false).is_ok()
            && let Some(branch) = old_branch.or_else(|| hex_branch(&self.worktrees_root, old_dir))
        {
            let _ = git.delete_branch(root, &branch, false);
        }
    }

    /// Makes a new worktree of the repository at `repo` on the new branch
    /// `branch` (else a new `orb/<hex>` one), from `base` (else the default
    /// branch) as origin has it, else as it is locally. A ref of another
    /// remote, like `upstream/x`, is used as it is.
    fn add_worktree(
        &self,
        repo: &Path,
        base: Option<&str>,
        branch: Option<&str>,
    ) -> Result<MadeWorktree, Report<GitError>> {
        let git = &self.services.git;
        let base = match base {
            Some(base) => base.to_owned(),
            None => git.default_branch(repo)?,
        };
        let base = start_point(git, repo, &base, |branch| self.fetch(repo, branch))?;
        let (path, branch) = (0..WORKTREE_NAME_ATTEMPTS)
            .map(|attempt| {
                let hex = hex(attempt);
                (
                    new_worktree_path(&self.worktrees_root, repo, &hex),
                    branch.map_or_else(|| format!("orb/{hex}"), str::to_owned),
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

    /// Fetches `branch` from origin, showing `fetching origin/<branch>…`
    /// while it runs.
    fn fetch(&self, repo: &Path, branch: &str) -> Result<bool, Report<GitError>> {
        self.state.write().sessions.fetching = Some(format!("origin/{branch}"));
        (self.wake)();
        let fetched = self.services.git.fetch(repo, branch);
        self.state.write().sessions.fetching = None;
        (self.wake)();
        fetched
    }

    /// Removes a worktree orb just made, and its branch, discarding anything
    /// in them.
    fn remove_made(&self, made: &MadeWorktree) {
        let git = &self.services.git;
        if git.remove_worktree(&made.repo, &made.path, true).is_ok() {
            let _ = git.delete_branch(&made.repo, &made.branch, true);
        }
    }

    /// Recreates session `id`'s worktree at its path and asks the frontend to
    /// attach to the session, ending the start the frontend marked. On
    /// failure, git's reason shows and nothing attaches.
    fn restore_worktree(&mut self, id: SessionId) {
        let result = self.recreate_worktree(id).map(|()| {
            self.state.write().sessions.attach = Some(id);
        });
        self.end_start(result);
    }

    /// Prunes git's record of session `id`'s missing worktree, then adds it
    /// back on its branch, or on that branch made anew from the default branch
    /// (as origin has it) when it's gone. A directory that is back already
    /// needs nothing.
    fn recreate_worktree(&self, id: SessionId) -> Result<(), String> {
        let (root, cwd, branch) = self
            .restore_target(id)
            .ok_or_else(|| "the session is gone".to_owned())?;
        if cwd.is_dir() {
            return Ok(());
        }
        let branch = branch.ok_or_else(|| "couldn't tell the worktree's branch".to_owned())?;
        let git = &self.services.git;
        git.prune_worktrees(&root)
            .and_then(|()| {
                if git.branch_exists(&root, &branch) {
                    return git.add_worktree_on(&root, &cwd, &branch);
                }
                let base = git.default_branch(&root)?;
                let base = start_point(git, &root, &base, |b| self.fetch(&root, b))?;
                git.add_worktree(&root, &cwd, &branch, &base)
            })
            .map_err(|report| git_reason(&report))
    }

    /// Session `id`'s project root, its directory, and the branch its
    /// worktree was on: the session's recorded branch, else one of its
    /// threads', else orb's
    /// `orb/<hex>` for the directory.
    fn restore_target(&self, id: SessionId) -> Option<(PathBuf, PathBuf, Option<String>)> {
        let session = self.sessions.get(&id)?;
        let thread_branch = self
            .rows
            .iter()
            .filter(|row| {
                row.pane_id
                    .and_then(|pane| self.panes.get(&pane))
                    .is_some_and(|pane| pane.session_id == id)
            })
            .find_map(|row| row.branch.clone());
        let branch = session
            .branch
            .clone()
            .or(thread_branch)
            .or_else(|| hex_branch(&self.worktrees_root, &session.dir));
        Some((
            self.project_root(session.project_id)?,
            session.dir.clone(),
            branch,
        ))
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

    /// Checks `git_ref` out for session `id`: in its directory, or with
    /// `to_root` in its project's checkout, moving the session there.
    fn switch_branch(&mut self, id: SessionId, git_ref: &GitRef, to_root: bool) {
        match self.sessions.get(&id).map(|row| row.dir.clone()) {
            _ if to_root => self.switch_to_root(id, git_ref),
            Some(dir) => {
                let _ = self.check_out_in(&dir, git_ref);
            }
            None => {}
        }
    }

    /// Checks `git_ref` out in the project's checkout and moves session
    /// `id`, which has had no agent turn, there from its worktree. Refused
    /// while a turn is underway in the checkout.
    fn switch_to_root(&mut self, id: SessionId, git_ref: &GitRef) {
        let root = self
            .sessions
            .get(&id)
            .and_then(|row| self.project_root(row.project_id));
        let Some(root) = root else {
            return self.end_start(Err("the session is gone".to_owned()));
        };
        if self.state.read().sessions.turned(id) {
            return self.change_workspace(id, Workspace::Checkout);
        }
        if self.busy_in(&root) {
            self.state.write().sessions.attach = Some(id);
            return self.end_start(Err(BUSY_DIRECTORY.to_owned()));
        }
        match self.services.git.checkout(&root, git_ref) {
            Ok(local) => {
                let _ = self.show_branch(&root, &local);
                self.change_workspace(id, Workspace::Checkout);
            }
            Err(report) => {
                self.state.write().sessions.attach = Some(id);
                self.end_start(Err(git_reason(&report)));
            }
        }
    }

    /// Saves and shows `branch` on every thread and every session in `cwd`.
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
                show(thread, row, status, pane_launch(row, &self.panes));
            }
        }
        for row in self.sessions.values_mut().filter(|row| row.dir == cwd) {
            row.branch = Some(branch.to_owned());
            if self.store.save_session(row).is_err() {
                saved = Err(SAVE_FAILED.to_owned());
            }
            if let Some(shown) = app.sessions.sessions.iter_mut().find(|s| s.id == row.id) {
                *shown = show_session(row);
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

    /// Saves `root` as a project, unless it already is one, and shows it; a
    /// removed project is restored.
    fn add_project(&mut self, root: PathBuf) {
        let now = now_ms();
        let title = project_title(&root);
        let added = if root.is_dir() {
            self.store
                .add_project(&root, &title, ProjectKind::Normal, now)
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
                    let repo = self.services.git.project_path(&root).is_some();
                    show_project(sessions, id, title, root, ProjectKind::Normal, repo, now);
                }
                Err(error) => sessions.error = Some(error),
            }
        }
        (self.wake)();
    }

    /// Makes a `kind` session named `name` in orb's own folder for the kind:
    /// adds or restores its project, copies `<orb_root>/templates/<kind>`
    /// (seeded when missing) to `<orb_root>/<kind>/<name>`, refused when that
    /// folder exists, and saves a session of that kind there with one shell,
    /// selected and attached. A made session outside the project filter
    /// clears it. The name box closes once the session is made and otherwise
    /// stays open with the reason on the mode line; a folder this call made
    /// is removed again.
    fn new_folder_session(&mut self, kind: FolderKind, name: &str) {
        let now = now_ms();
        let (project_kind, kind_dir, title) = own_folder(kind);
        let root = self.orb_root.join(kind_dir);
        let project = fs::create_dir_all(&root)
            .map_err(|_error| FOLDER_UNMADE.to_owned())
            .and_then(|()| {
                self.store
                    .add_project(&root, title, project_kind, now)
                    .map_err(|_report| SAVE_FAILED.to_owned())
            });
        if let Ok(id) = project {
            let mut app = self.state.write();
            show_project(
                &mut app.sessions,
                id,
                title.to_owned(),
                root.clone(),
                project_kind,
                false,
                now,
            );
        }
        let made = project.and_then(|id| {
            let dir = self.make_folder(kind, kind_dir, &root, name)?;
            let saved = self.save_session(id, session_kind(project_kind), &dir, None);
            if saved.is_err() {
                let _ = fs::remove_dir_all(&dir);
            }
            saved.map(|()| id)
        });
        let unfiltered = {
            let mut app = self.state.write();
            let app = &mut *app;
            answer_name_box(app, made.is_ok());
            let sessions = &mut app.sessions;
            let outside = made
                .as_ref()
                .is_ok_and(|&id| sessions.filter.is_some_and(|filter| filter != id));
            if outside {
                sessions.filter = None;
            }
            outside
        };
        if unfiltered {
            self.save_ui();
        }
        self.end_start(made.map(|_| ()));
    }

    /// Copies `kind`'s template (`<orb_root>/templates/<kind_dir>`, seeded
    /// when missing) to `root/name` and returns that folder; refused when it
    /// exists. A folder this call made is removed again on failure. The
    /// error is the mode-line text.
    fn make_folder(
        &self,
        kind: FolderKind,
        kind_dir: &str,
        root: &Path,
        name: &str,
    ) -> Result<PathBuf, String> {
        let dir = root.join(name);
        if dir.exists() {
            return Err(folder_exists(kind, name));
        }
        let template = self.orb_root.join("templates").join(kind_dir);
        let seeded = if template.exists() {
            Ok(())
        } else {
            template::seed(&template, kind)
        };
        match seeded.and_then(|()| template::copy(&template, &dir)) {
            Ok(()) => Ok(dir),
            Err(_) => {
                let _ = fs::remove_dir_all(&dir);
                Err(FOLDER_UNMADE.to_owned())
            }
        }
    }

    /// Removes project `id` from `<C-g> n` and the project filter once the store
    /// has; its sessions stay.
    fn remove_project(&mut self, id: ProjectId) {
        let removed = self.store.remove_project(id, now_ms());
        {
            let mut app = self.state.write();
            let sessions = &mut app.sessions;
            match (removed, sessions.projects.iter_mut().find(|p| p.id == id)) {
                (Ok(()), Some(project)) => project.removed = true,
                (Ok(()), None) => {}
                (Err(_), _) => sessions.error = Some(SAVE_FAILED.to_owned()),
            }
        }
        (self.wake)();
    }

    /// Pins session `id`; pinning a settled session un-settles it.
    fn pin_session(&mut self, id: SessionId) {
        self.edit_session(id, |row, now| {
            row.pinned_at = row.pinned_at.or(Some(now));
            if row.settled_override == Some(SettledOverride::Settled) {
                unsettle_session_row(row, now);
            }
        });
    }

    fn unpin_session(&mut self, id: SessionId) {
        self.edit_session(id, |row, _| row.pinned_at = None);
    }

    /// Gives session `id` the user's name; `None` goes back to its agents'
    /// or directory's.
    fn rename_session(&mut self, id: SessionId, name: Option<String>) {
        self.edit_session(id, |row, _| row.name = name);
    }

    /// The saved threads running in session `id`'s panes.
    fn session_rows(&self, id: SessionId) -> impl Iterator<Item = &ThreadRow> {
        self.rows.iter().filter(move |row| {
            row.pane_id
                .and_then(|pane| self.panes.get(&pane))
                .is_some_and(|pane| pane.session_id == id)
        })
    }

    /// Settles session `id` unless one of its agent panes has a turn
    /// underway (a turn that started after the key press wins), then kills
    /// every pane, keeping its tabs, layout and each pane's resume command.
    fn settle_session(&mut self, id: SessionId) {
        if self
            .session_rows(id)
            .any(|row| self.status(row.id).is_some_and(ThreadStatus::in_progress))
        {
            return;
        }
        self.edit_session(id, settle_session_row);
        self.kill_panes(id);
    }

    fn unsettle_session(&mut self, id: SessionId) {
        self.edit_session(id, unsettle_session_row);
    }

    /// Marks the latest turn of each of session `id`'s agents seen.
    fn visit(&mut self, id: SessionId) {
        let unseen: Vec<ThreadId> = self
            .session_rows(id)
            .filter(|row| row.last_activity_at > row.last_visited_at)
            .map(|row| row.id)
            .collect();
        for thread in unseen {
            self.edit(thread, |row, now| row.last_visited_at = now);
        }
    }

    /// Deletes session `id`: kills every pane, deletes a Research or Learn
    /// folder that is orb's own and no other session uses, then its tabs,
    /// panes, threads (ended ones included) and row, and forgets them.
    /// Claude transcripts, pi session files and worktrees stay. However it
    /// ends, the session is no longer hidden as being deleted.
    fn delete_session(&mut self, id: SessionId) {
        self.kill_panes(id);
        let cleared = self.clear_folder(id);
        let threads: Vec<ThreadId> = self
            .session_rows(id)
            .chain(self.rows.iter().filter(|row| row.orb_session == Some(id)))
            .map(|row| row.id)
            .collect();
        let panes: Vec<PaneId> = self
            .panes
            .values()
            .filter(|pane| pane.session_id == id)
            .map(|pane| pane.id)
            .collect();
        let deleted = self.store.delete_session(id);
        self.rows.retain(|row| !threads.contains(&row.id));
        for pane in &panes {
            let _ = fs::remove_file(self.pane_files.file(*pane));
            self.panes.remove(pane);
            self.pane_reports.remove(pane);
        }
        self.sessions.remove(&id);
        {
            let mut app = self.state.write();
            app.layouts.remove(id);
            app.attached.remove(&id);
            let sessions = &mut app.sessions;
            sessions.sessions.retain(|session| session.id != id);
            for project in &mut sessions.projects {
                project
                    .threads
                    .retain(|thread| !threads.contains(&thread.id));
            }
            sessions.deleting.remove(&id);
            match (deleted, cleared) {
                (Err(_), _) => sessions.error = Some(SAVE_FAILED.to_owned()),
                (Ok(()), Err(error)) => sessions.error = Some(error),
                (Ok(()), Ok(())) => {}
            }
        }
        (self.wake)();
    }

    /// Kills the panes zmx still lists for every settled session, listing
    /// each socket dir once, in pane order. Failures are ignored.
    fn kill_settled(&self) {
        let mut rows: Vec<&PaneRow> = self
            .panes
            .values()
            .filter(|row| {
                self.sessions.get(&row.session_id).is_some_and(|session| {
                    session.settled_override == Some(SettledOverride::Settled)
                })
            })
            .collect();
        rows.sort_by_key(|row| row.id.0);
        let zmx: Vec<ZmxSession> = rows
            .into_iter()
            .map(|row| pane_entry(&self.services, &self.orb_root, row).zmx)
            .collect();
        let listed: HashMap<&Path, Vec<ZmxEntry>> = zmx
            .iter()
            .map(|session| session.dir.as_path())
            .collect::<HashSet<&Path>>()
            .into_iter()
            .map(|dir| (dir, self.services.zmx.list(dir).unwrap_or_default()))
            .collect();
        for session in &zmx {
            let running = listed
                .get(session.dir.as_path())
                .is_some_and(|entries| entries.iter().any(|entry| entry.name == session.name));
            if running {
                let _ = self.services.zmx.kill(session);
            }
        }
    }

    /// Runs `zmx kill` on each of session `id`'s panes in pane order,
    /// ignoring failures.
    fn kill_panes(&self, id: SessionId) {
        let mut rows: Vec<&PaneRow> = self
            .panes
            .values()
            .filter(|row| row.session_id == id)
            .collect();
        rows.sort_by_key(|row| row.id.0);
        for row in rows {
            let _ = self
                .services
                .zmx
                .kill(&pane_entry(&self.services, &self.orb_root, row).zmx);
        }
    }

    /// Removes session `id`'s folder when it is a Research or Learn session
    /// in orb's own folder for its kind and no other session uses it
    /// (already gone counts as done). The error is the mode-line text.
    fn clear_folder(&self, id: SessionId) -> Result<(), String> {
        let Some(row) = self.sessions.get(&id) else {
            return Ok(());
        };
        let kind_dir = match row.kind {
            SessionKind::Research => "research",
            SessionKind::Learn => "learn",
            SessionKind::Plain | SessionKind::Incognito => return Ok(()),
        };
        let shared = self
            .sessions
            .values()
            .any(|other| other.id != id && other.dir == row.dir);
        if shared || row.dir.parent() != Some(self.orb_root.join(kind_dir).as_path()) {
            return Ok(());
        }
        match fs::remove_dir_all(&row.dir) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(FOLDER_UNREMOVED.to_owned()),
        }
    }

    /// Adds a shell pane in `session`'s directory: splitting its focused pane
    /// `split`, or in a new tab when `None`; a session with no layout (its
    /// last pane closed) gets a first tab of that shell and is attached once
    /// it's saved. Then saves the layout. A split of a session with no layout
    /// does nothing; a pane that can't be saved shows why.
    fn add_pane(&mut self, session: SessionId, split: Option<Split>) {
        let Some(dir) = self.sessions.get(&session).map(|row| row.dir.clone()) else {
            return;
        };
        let first = self.state.read().layouts.get(session).is_none();
        if first && split.is_some() {
            return;
        }
        let Ok(id) = self.store.insert_pane(session, &dir) else {
            self.state.write().sessions.error = Some(SAVE_FAILED.to_owned());
            return (self.wake)();
        };
        let row = PaneRow {
            id,
            session_id: session,
            cwd: dir,
            zmx_name: None,
            zmx_dir: None,
            resume: None,
            migrated_bg: None,
            name: None,
        };
        let entry = pane_entry(&self.services, &self.orb_root, &row);
        self.panes.insert(id, row);
        {
            let mut app = self.state.write();
            match split {
                Some(split) => app.layouts.split(session, split, entry),
                None if first => {
                    app.layouts.insert(session, SessionLayout::of(entry));
                    app.sessions.attach = Some(session);
                }
                None => app.layouts.new_tab(session, entry),
            }
        }
        self.save_layout(session);
        (self.wake)();
    }

    /// Saves `session`'s tabs, active tab and pane names as the app state
    /// has them; panes no tab holds any more are deleted from the store. A
    /// session whose layout emptied keeps no tab or pane and settles now;
    /// nothing happens for one that is gone.
    fn save_layout(&mut self, session: SessionId) {
        let saved = {
            let app = self.state.read();
            app.layouts.get(session).map(|layout| {
                let tabs: Vec<TabRow> = layout
                    .tabs()
                    .iter()
                    .enumerate()
                    .map(|(position, tab)| TabRow {
                        session_id: session,
                        position,
                        name: tab.name().map(str::to_owned),
                        layout: tab.layout_json(),
                        focus_pane: Some(tab.focused()),
                    })
                    .collect();
                let names: Vec<(PaneId, Option<String>)> = layout
                    .panes()
                    .map(|entry| (entry.id, entry.name.clone()))
                    .collect();
                (tabs, layout.active(), names)
            })
        };
        let (tabs, active, names) = match saved {
            Some(saved) => saved,
            None if self.sessions.contains_key(&session) => {
                self.edit_session(session, settle_session_row);
                (Vec::new(), 0, Vec::new())
            }
            None => return,
        };
        match self.store.save_layout(session, active, &tabs, &names) {
            Ok(dropped) => {
                for pane in dropped {
                    self.panes.remove(&pane);
                    for row in self.rows.iter_mut().filter(|row| row.pane_id == Some(pane)) {
                        row.pane_id = None;
                    }
                }
            }
            Err(_) => {
                self.state.write().sessions.error = Some(SAVE_FAILED.to_owned());
                (self.wake)();
            }
        }
    }

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

    /// Saves the jump list, showing why if it can't.
    fn save_jumps(&self) {
        let jumps = self.state.read().jumps.entries().to_vec();
        if self.store.save_jumps(&jumps).is_err() {
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
                show(thread, row, status, pane_launch(row, &self.panes));
            }
        }
        (self.wake)();
    }

    /// Changes session `id`'s saved row with `change` (given the time now),
    /// then saves and shows it. Does nothing if the session is gone.
    fn edit_session<F>(&mut self, id: SessionId, change: F)
    where
        F: FnOnce(&mut SessionRow, i64),
    {
        let Some(row) = self.sessions.get_mut(&id) else {
            return;
        };
        change(row, now_ms());
        let saved = self.store.save_session(row);
        {
            let mut app = self.state.write();
            let sessions = &mut app.sessions;
            if saved.is_err() {
                sessions.error = Some(SAVE_FAILED.to_owned());
            }
            if let Some(shown) = sessions.sessions.iter_mut().find(|shown| shown.id == id) {
                *shown = show_session(row);
            }
        }
        (self.wake)();
    }

    /// Stops each `--bg` session a migrated pane still names that Claude
    /// lists as live, through Claude, in pane order, ignoring failures (a
    /// session already gone is fine), and forgets each one as soon as its
    /// stop returns, so an interrupted start keeps its progress. The rest are
    /// forgotten without a stop; if Claude can't list them, every one is
    /// stopped. `announce` hears how many stops will run, when any will.
    /// Without Claude registered nothing happens and the markers stay.
    async fn stop_migrated(&mut self, announce: fn(usize)) {
        let Some(claude) = self
            .services
            .harnesses
            .get(&HarnessId::new(claude::ID))
            .cloned()
        else {
            return;
        };
        let marked: Vec<(PaneId, String)> = {
            let mut marked: Vec<(PaneId, String)> = self
                .panes
                .values()
                .filter_map(|pane| Some((pane.id, pane.migrated_bg.clone()?)))
                .collect();
            marked.sort_by_key(|(pane, _)| pane.0);
            marked
        };
        if marked.is_empty() {
            return;
        }
        let (live, ended): (Vec<_>, Vec<_>) = match claude.live_background().await {
            Ok(live) => marked
                .into_iter()
                .partition(|(_, short_id)| live.contains(short_id)),
            Err(_) => (marked, Vec::new()),
        };
        self.forget_migrated(&ended.iter().map(|(pane, _)| *pane).collect::<Vec<_>>());
        if !live.is_empty() {
            announce(live.len());
        }
        for (pane, short_id) in &live {
            let _ = claude.stop_migrated(short_id).await;
            self.forget_migrated(&[*pane]);
        }
    }

    /// Forgets the `--bg` sessions `panes` named, in the store and here.
    fn forget_migrated(&mut self, panes: &[PaneId]) {
        if panes.is_empty() {
            return;
        }
        match self.store.clear_migrated_bg(panes) {
            Ok(()) => {
                for pane in panes {
                    if let Some(row) = self.panes.get_mut(pane) {
                        row.migrated_bg = None;
                    }
                }
            }
            Err(_) => {
                self.state.write().sessions.error = Some(SAVE_FAILED.to_owned());
                (self.wake)();
            }
        }
    }

    /// Shows a harness failure in the mode line.
    fn fail(&self, report: &Report<HarnessError>) {
        self.state.write().sessions.error = Some(reason(report));
        (self.wake)();
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

/// What one poll saw of the panes threads run in.
struct Live {
    /// Each bound pane's zmx session, `None` when it isn't running. A pane
    /// whose socket dir couldn't be listed is missing.
    sessions: HashMap<PaneId, Option<ZmxEntry>>,
    /// The status of each pane an agent's process runs under.
    matched: HashMap<PaneId, ThreadStatus>,
}

/// What a saved thread is doing now, if this poll can tell: Gone for a
/// harness orb doesn't know; Stopped with no pane; else its pane's status
/// (see [`pane_status`]). `None` (left as it was) when its pane's socket dir
/// couldn't be listed.
fn status_now(
    row: &ThreadRow,
    harness: Option<&Arc<dyn Harness>>,
    live: &Live,
    reports: &HashMap<PaneId, PaneReport>,
) -> Option<ThreadStatus> {
    let Some(harness) = harness else {
        return Some(ThreadStatus::Gone);
    };
    let Some(pane) = row.pane_id else {
        return Some(ThreadStatus::Stopped);
    };
    let running = live.sessions.get(&pane)?;
    Some(pane_status(
        running.as_ref(),
        harness.reports_status(),
        reports.get(&pane).copied(),
        live.matched.get(&pane).copied(),
    ))
}

/// Follows a polled row's activity, given its `status` now and whether a
/// turn was underway before this poll:
/// - a turn that ended is the thread's latest activity;
/// - the latest turn of a thread in the selected session is seen.
///
/// Settling and un-settling are its session's (see [`follow_session`]).
fn follow_activity(
    row: &mut ThreadRow,
    status: ThreadStatus,
    was_in_progress: bool,
    selected: bool,
    now: i64,
) {
    if was_in_progress && !status.in_progress() {
        row.last_activity_at = now;
    }
    if selected && row.last_activity_at > row.last_visited_at {
        row.last_visited_at = now;
    }
}

/// What a poll saw of a session's agent panes.
#[derive(Debug, Clone, Copy)]
struct Activity {
    /// The latest turn activity in it, its own or any agent pane's.
    latest: i64,
    /// Whether any agent pane has a turn underway.
    in_progress: bool,
    /// Whether it has an agent pane at all.
    agents: bool,
}

/// Follows a session's settle lifecycle from `activity`:
/// - the latest activity becomes the session's;
/// - a turn underway un-settles it and ends its kept-active mark;
/// - an unpinned session with an agent pane, idle for
///   [`AUTO_SETTLE_AFTER`], settles as of that activity, unless it is kept
///   active, in progress or `selected`. A session with no agent pane never
///   settles by itself, since settling kills its shells and editors.
///
/// Returns whether it settled the session just now.
fn follow_session(row: &mut SessionRow, activity: Activity, selected: bool, now: i64) -> bool {
    row.last_activity_at = activity.latest;
    if activity.in_progress
        && let Some(settled_override) = row.settled_override
    {
        if settled_override == SettledOverride::Settled {
            row.unsettled_at = Some(now);
        }
        row.settled_override = None;
        row.settled_at = None;
    }
    if row.settled_override.is_none()
        && row.pinned_at.is_none()
        && activity.agents
        && !activity.in_progress
        && !selected
        && now.saturating_sub(activity.latest) >= AUTO_SETTLE_AFTER
    {
        settle_session_row(row, activity.latest);
        return true;
    }
    false
}

/// Settles session `row` onto the shelf as of `at`, unpinning it.
fn settle_session_row(row: &mut SessionRow, at: i64) {
    row.settled_override = Some(SettledOverride::Settled);
    row.settled_at = Some(at);
    row.unsettled_at = None;
    row.pinned_at = None;
}

/// Un-settles session `row` and keeps it active until its next turn
/// activity. It re-enters Active at `now` unless it was already kept active.
fn unsettle_session_row(row: &mut SessionRow, now: i64) {
    if row.settled_override != Some(SettledOverride::Active) {
        row.unsettled_at = Some(now);
    }
    row.settled_override = Some(SettledOverride::Active);
    row.settled_at = None;
}

/// Answers the name box that asked for a new session, if it's still waiting:
/// closes it once the session is `made`, else leaves it open with its text
/// for another try.
fn answer_name_box(app: &mut AppState, made: bool) {
    match (&mut app.rename, made) {
        (Some(rename), false) if rename.creating => rename.creating = false,
        (Some(rename), true) if rename.creating => {
            app.rename = None;
            if app.focus == Focus::Rename {
                app.focus = Focus::Sidebar;
            }
        }
        _ => {}
    }
}

/// The shown thread with id `id`.
fn thread_mut(sessions: &mut Sessions, id: ThreadId) -> Option<&mut Thread> {
    sessions
        .projects
        .iter_mut()
        .flat_map(|project| &mut project.threads)
        .find(|thread| thread.id == id)
}

/// Brings a saved thread up to date with `status`: the turn stamp, and the
/// title and branch from any new transcript lines, read in its harness's
/// `format`. Returns whether a scan ran and named no branch, which leaves
/// the branch to git.
fn update_row(
    row: &mut ThreadRow,
    status: ThreadStatus,
    now_ms: i64,
    format: Option<&dyn TranscriptFormat>,
) -> bool {
    row.turn_started_at = status
        .in_progress()
        .then(|| row.turn_started_at.unwrap_or(now_ms));
    let (Some(session_id), Some(format)) = (&row.session_id, format) else {
        return false;
    };
    if row.transcript_path.is_none() {
        row.transcript_path = format.locate(&row.cwd, session_id);
    }
    let previous = Scan {
        title: row.title.clone(),
        custom_title: row.custom_title.clone(),
        branch: row.branch.clone(),
        ai_titled: row.ai_titled,
        offset: row.transcript_offset,
    };
    let Some(scan) = row
        .transcript_path
        .as_deref()
        .and_then(|path| format.scan(path, row.transcript_offset, &previous).ok())
    else {
        return false;
    };
    // A new `/rename` replaces the orb name; the harness re-writing its old
    // name doesn't.
    if scan.custom_title != row.custom_title {
        row.renamed_title = None;
    }
    row.title = scan.title;
    row.custom_title = scan.custom_title;
    row.ai_titled |= scan.ai_titled;
    row.transcript_offset = scan.offset;
    match scan.branch {
        Some(branch) => {
            row.branch = Some(branch);
            false
        }
        None => true,
    }
}

/// Makes `row` the thread of conversation `session_id`.
fn take_session(row: &mut ThreadRow, session_id: &str) {
    session_id.clone_into(&mut row.short_id);
    row.session_id = Some(session_id.to_owned());
}

/// Points `row` at `transcript` when the report names a different one, read
/// from its start.
fn take_transcript(row: &mut ThreadRow, transcript: Option<&PathBuf>) {
    if let Some(path) = transcript
        && row.transcript_path.as_ref() != Some(path)
    {
        row.transcript_path = Some(path.clone());
        row.transcript_offset = 0;
    }
}

/// Renames the `orb/<hex>` branch of the orb worktree `row` is in to
/// `orb/<slug>` of its title, once the harness or the user titled it. The old name
/// stays when that branch already exists or git refuses.
fn rename_hex_branch(git: &GitService, worktrees_root: &Path, row: &mut ThreadRow) {
    if let Some(old) = hex_branch(worktrees_root, &row.cwd)
        && row.branch.as_deref() == Some(old.as_str())
        && (row.renamed_title.is_some() || row.custom_title.is_some() || row.ai_titled)
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

/// Shows a saved thread and `status` on `thread`, its pane run as `pane`.
/// Returns whether anything visible changed.
fn show(
    thread: &mut Thread,
    row: &ThreadRow,
    status: ThreadStatus,
    pane: Option<PaneLaunch>,
) -> bool {
    let shown = self::thread(row, status, pane);
    let changed = *thread != shown;
    *thread = shown;
    changed
}

/// Why a thread that was `old` and is now `new` needs the user, if it does:
/// a turn ended, or the session started waiting for an approval or an answer. A
/// thread not polled since orb started never does, so states that existed at
/// launch don't notify.
fn notice_kind(old: ThreadStatus, new: ThreadStatus) -> Option<NoticeKind> {
    match (old, new) {
        (ThreadStatus::Unknown, _) => None,
        (old, ThreadStatus::Idle) if old.in_progress() => Some(NoticeKind::Finished),
        (old, ThreadStatus::NeedsApproval) if old != ThreadStatus::NeedsApproval => {
            Some(NoticeKind::NeedsApproval)
        }
        (old, ThreadStatus::NeedsInput) if old != ThreadStatus::NeedsInput => {
            Some(NoticeKind::NeedsInput)
        }
        _ => None,
    }
}

/// The notice for the saved thread `row` now being `status`, if its shown
/// status changes in a way that needs the user; never for a thread being
/// deleted. The title is the row's, already updated by this poll.
fn notice(sessions: &Sessions, row: &ThreadRow, status: ThreadStatus) -> Option<Notice> {
    let (project, thread) = sessions.projects.iter().find_map(|project| {
        project
            .threads
            .iter()
            .find(|thread| thread.id == row.id)
            .map(|thread| (project, thread))
    })?;
    if sessions.is_deleting(thread) {
        return None;
    }
    Some(Notice {
        thread: row.id,
        kind: notice_kind(thread.status, status)?,
        project: project.title.clone(),
        title: display_title(row).unwrap_or_else(|| NEW_THREAD.to_owned()),
    })
}

/// The title the sidebar shows: the user's `r` name, else their `/rename`,
/// else the transcript's.
fn display_title(row: &ThreadRow) -> Option<String> {
    row.renamed_title
        .clone()
        .or_else(|| row.custom_title.clone())
        .or_else(|| row.title.clone())
}

/// The one-line reason a harness failure carries.
fn reason(report: &Report<HarnessError>) -> String {
    report
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| "harness command failed".to_owned())
}

/// How a saved thread looks with `status`, its pane run as `pane`.
fn thread(row: &ThreadRow, status: ThreadStatus, pane: Option<PaneLaunch>) -> Thread {
    Thread {
        id: row.id,
        title: display_title(row),
        cwd: row.cwd.clone(),
        transcript: row.transcript_path.clone(),
        status,
        turn_started_at: row.turn_started_at.map(from_ms),
        pane,
        last_session: row.orb_session,
        branch: row.branch.clone(),
        created_at: from_ms(row.created_at),
        last_activity_at: from_ms(row.last_activity_at),
        unseen: row.last_activity_at > row.last_visited_at,
        model: row.model.clone(),
        harness: row.harness.clone(),
    }
}

/// The kind of session a project's new thread gets.
fn session_kind(kind: ProjectKind) -> SessionKind {
    match kind {
        ProjectKind::Normal => SessionKind::Plain,
        ProjectKind::Research => SessionKind::Research,
        ProjectKind::Learn => SessionKind::Learn,
        ProjectKind::Incognito => SessionKind::Incognito,
    }
}

/// How a saved session looks: settled only while settled by hand or by
/// idleness, active since the later of its creation and its latest
/// un-settle.
fn show_session(row: &SessionRow) -> Session {
    Session {
        id: row.id,
        project: row.project_id,
        kind: row.kind,
        dir: row.dir.clone(),
        name: row.name.clone(),
        branch: row.branch.clone(),
        created_at: from_ms(row.created_at),
        pinned_at: row.pinned_at.map(from_ms),
        settled_at: row
            .settled_at
            .filter(|_| row.settled_override == Some(SettledOverride::Settled))
            .map(from_ms),
        active_since: from_ms(row.created_at.max(row.unsettled_at.unwrap_or(0))),
        last_activity_at: from_ms(row.last_activity_at),
    }
}

/// Whether saved jump-list row `item` still names one of `sessions`, shown
/// or not.
fn exists(sessions: &HashMap<SessionId, SessionRow>, item: SidebarItem) -> bool {
    match item {
        SidebarItem::Session(id) => sessions.contains_key(&id),
        SidebarItem::SettledShelf => false,
    }
}

/// How a saved thread looks before its first poll; one whose harness orb
/// doesn't know is Gone.
fn unpolled(services: &Services, row: &ThreadRow, panes: &HashMap<PaneId, PaneRow>) -> Thread {
    let status = match services.harnesses.get(&row.harness) {
        Some(_) => ThreadStatus::Unknown,
        None => ThreadStatus::Gone,
    };
    thread(row, status, pane_launch(row, panes))
}

/// `row`'s pane and its session; `None` for a thread with no pane.
fn pane_launch(row: &ThreadRow, panes: &HashMap<PaneId, PaneRow>) -> Option<PaneLaunch> {
    let pane = panes.get(&row.pane_id?)?;
    Some(PaneLaunch {
        pane: pane.id,
        session: pane.session_id,
    })
}

/// Where pane `row`'s program runs: its kept zmx session, on a socket dir
/// under orb's folder unless the dir is absolute (orb's pane dir when none),
/// else `orb-p<id>` on orb's pane dir.
fn pane_entry(services: &Services, orb_root: &Path, row: &PaneRow) -> PaneEntry {
    let zmx = match &row.zmx_name {
        Some(name) => ZmxSession {
            name: name.clone(),
            dir: row
                .zmx_dir
                .as_ref()
                .map_or_else(|| services.zmx.dir().to_owned(), |dir| orb_root.join(dir)),
        },
        None => services.zmx.session(row.id.zmx_name()),
    };
    PaneEntry {
        id: row.id,
        zmx,
        cwd: row.cwd.clone(),
        name: row.name.clone(),
        resume: row.resume.clone(),
    }
}

/// What the store's saved layouts restore to.
struct RestoredLayouts {
    /// Every saved pane.
    panes: HashMap<PaneId, PaneRow>,
    /// Every saved session.
    sessions: HashMap<SessionId, SessionRow>,
    /// Each session's layout.
    layouts: Vec<(SessionId, SessionLayout)>,
    /// Why some or all of them couldn't be read.
    error: Option<&'static str>,
}

/// Every saved session's layout: its tabs by position, each focused on its
/// saved pane (else its first), showing its saved tab, with where each pane
/// runs. A tab whose tree doesn't read is left out, and so is a session left
/// with no tab. Nothing comes back when the store can't be read.
fn restore_layouts(store: &Store, services: &Services, orb_root: &Path) -> RestoredLayouts {
    let Ok(saved) = store.layouts() else {
        return RestoredLayouts {
            panes: HashMap::new(),
            sessions: HashMap::new(),
            layouts: Vec::new(),
            error: Some("couldn't load orb's saved layouts"),
        };
    };
    let mut unread = false;
    let layouts = saved
        .sessions
        .iter()
        .filter_map(|session| {
            let tabs: Vec<Tab> = saved
                .tabs
                .iter()
                .filter(|tab| tab.session_id == session.id)
                .filter_map(|tab| {
                    let focus = tab.focus_pane.unwrap_or(PaneId(0));
                    match TileLayout::from_json(&tab.layout, focus) {
                        Ok(tree) => Some(Tab::restore(tab.name.clone(), tree)),
                        Err(_) => {
                            unread = true;
                            None
                        }
                    }
                })
                .collect();
            if tabs.is_empty() {
                return None;
            }
            let panes = saved
                .panes
                .iter()
                .filter(|pane| pane.session_id == session.id)
                .map(|pane| pane_entry(services, orb_root, pane))
                .collect();
            Some((
                session.id,
                SessionLayout::restore(tabs, session.active_tab, panes),
            ))
        })
        .collect();
    RestoredLayouts {
        sessions: saved
            .sessions
            .into_iter()
            .map(|row| (row.id, row))
            .collect(),
        panes: saved.panes.into_iter().map(|row| (row.id, row)).collect(),
        layouts,
        error: unread.then_some("couldn't read a saved tab"),
    }
}

/// The branch checked out in `cwd`, if git can tell.
fn current_branch(git: &GitService, cwd: &Path) -> Option<String> {
    git.refs(cwd)
        .ok()?
        .into_iter()
        .find(|git_ref| git_ref.current)
        .map(|git_ref| git_ref.name)
}

/// The ref a new worktree of `repo` starts from for `base`: `origin/<b>` when
/// the repository has an origin and `origin_has` says it has `b` (a fetch of
/// `b` from origin), else `base` as is.
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

/// Shows project `id` in `sessions`: restores it if removed, taking `kind`
/// unless that's [`ProjectKind::Normal`] (as the store keeps a saved kind),
/// or adds it; either way `repo` says whether it is a git repository.
fn show_project(
    sessions: &mut Sessions,
    id: ProjectId,
    title: String,
    root: PathBuf,
    kind: ProjectKind,
    repo: bool,
    now: i64,
) {
    match sessions
        .projects
        .iter_mut()
        .find(|project| project.id == id)
    {
        Some(project) => {
            project.removed = false;
            project.repo = repo;
            if kind != ProjectKind::Normal {
                project.kind = kind;
            }
        }
        None => sessions.projects.push(Project {
            id,
            title,
            root,
            created_at: from_ms(now),
            threads: Vec::new(),
            repo,
            removed: false,
            kind,
        }),
    }
}

/// orb's own folder for a `kind` session: its project's kind, its folder's
/// name under orb's directory, and its project's title.
fn own_folder(kind: FolderKind) -> (ProjectKind, &'static str, &'static str) {
    match kind {
        FolderKind::Research => (ProjectKind::Research, "research", "Research"),
        FolderKind::Learn => (ProjectKind::Learn, "learn", "Learn"),
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
pub(crate) fn to_ms(time: SystemTime) -> i64 {
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
    use crate::feat::harness::HarnessId;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, OnceLock, PoisonError};
    use std::time::{Duration, SystemTime};

    use error_stack::{Report, ResultExt};

    use super::{
        FAST_POLL, SLOW_POLL, SessionsActor, SessionsActorDeps, notice_kind, now_ms,
        settle_session_row,
    };
    use crate::Focus;
    use crate::TextInput;
    use crate::command::Workspace;
    use crate::common::{Services, State};
    use crate::feat::git::git_service::{Git, GitError, GitRef, GitService, WorktreeFacts};
    use crate::feat::harness::claude::ClaudeCode;
    use crate::feat::harness::claude::fake::FakeClaude;
    use crate::feat::harness::claude::transcript::transcript_path;
    use crate::feat::harness::fake::FakeHarness;
    use crate::feat::harness::{Harness, Harnesses, RunningAgent};
    use crate::feat::jumps::state::JumpList;
    use crate::feat::layout::state::test_entry;
    use crate::feat::layout::tree::Split;
    use crate::feat::sessions::state::{
        FolderKind, Notice, NoticeKind, PaneId, PaneLaunch, Project, ProjectId, ProjectKind,
        Session, SessionId, SessionKind, SidebarItem, Thread, ThreadId, ThreadStatus,
    };
    use crate::feat::sessions::store::{
        NewPaneThread, SessionRow, SettledOverride, Store, StoreError, TabRow, ThreadRow, Ui,
    };
    use crate::feat::sidebar::state::{Rename, RenameTarget};
    use crate::feat::zmx::zmx_service::fake::FakeZmx;
    use crate::feat::zmx::zmx_service::{ZmxOutput, ZmxService, ZmxSession};

    /// The orb project's root, a folder that exists so its sessions can start.
    const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");
    /// The web project's root, another folder that exists.
    const WEB_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    /// The branch checked out wherever the fake git lists refs.
    const CURRENT_BRANCH: &str = "dev";
    const NO_CLAUDE_DIR: &str = "/nonexistent/claude";
    const WORKTREES_ROOT: &str = "/nonexistent/worktrees";
    const ORB_ROOT: &str = "/nonexistent/orb";
    const INCOGNITO_ROOT: &str = "/nonexistent/orb-incognito";
    /// Why git refuses outside a repository.
    const NOT_A_REPO: &str = "fatal: not a git repository";
    const HOUR_MS: i64 = 60 * 60 * 1000;
    const DAY_MS: i64 = 24 * HOUR_MS;

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
        AddWorktreeOn {
            path: PathBuf,
            branch: String,
        },
        PruneWorktrees(PathBuf),
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
        /// Whether `is_merged` and `delete_branch` without force find the
        /// branch merged.
        merged: bool,
        /// What `project_path` answers.
        project: Option<PathBuf>,
        calls: Mutex<Vec<GitCall>>,
        /// The app state `fetch` looks at, once a test watches one.
        watched: OnceLock<State>,
        /// What `sessions.fetching` was each time `fetch` ran, while watched.
        fetching_seen: Mutex<Vec<Option<String>>>,
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
                merged: true,
                project: None,
                calls: Mutex::default(),
                watched: OnceLock::new(),
                fetching_seen: Mutex::default(),
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

        /// Looks at `state` on every later fetch.
        fn watch(&self, state: &State) {
            let _ = self.watched.set(state.clone());
        }

        /// What `sessions.fetching` read while each watched fetch ran.
        fn fetching_seen(&self) -> Vec<Option<String>> {
            self.fetching_seen
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
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
            if let Some(state) = self.watched.get() {
                let fetching = state.read().sessions.fetching.clone();
                self.fetching_seen
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(fetching);
            }
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

        fn add_worktree_on(
            &self,
            _repo: &Path,
            path: &Path,
            branch: &str,
        ) -> Result<(), Report<GitError>> {
            self.record(GitCall::AddWorktreeOn {
                path: path.to_owned(),
                branch: branch.to_owned(),
            });
            Ok(())
        }

        fn prune_worktrees(&self, repo: &Path) -> Result<(), Report<GitError>> {
            self.record(GitCall::PruneWorktrees(repo.to_owned()));
            Ok(())
        }

        fn worktree_facts(&self, _path: &Path) -> Result<WorktreeFacts, Report<GitError>> {
            Err(Report::new(GitError)
                .attach("the sessions actor never reads worktree facts".to_owned()))
        }

        fn disk_usage(&self, _path: &Path) -> Option<u64> {
            None
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
            if !force && !self.merged {
                return Err(Report::new(GitError)
                    .attach(format!("error: the branch '{branch}' is not fully merged")));
            }
            Ok(())
        }

        fn branch_exists(&self, _repo: &Path, branch: &str) -> bool {
            self.existing.as_deref() == Some(branch)
        }

        fn is_merged(&self, _repo: &Path, _branch: &str) -> bool {
            self.merged
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

        fn project_path(&self, _cwd: &Path) -> Option<PathBuf> {
            self.project.clone()
        }
    }

    /// A zmx whose `list` prints `stdout` on every socket dir, on the pane
    /// socket dir /zmx.
    fn zmx_listing(stdout: &str) -> ZmxService {
        ZmxService::new(
            Arc::new(FakeZmx::new(ZmxOutput {
                success: true,
                stdout: stdout.to_owned(),
                stderr: String::new(),
            })),
            PathBuf::from("/zmx"),
        )
    }

    /// A thread and the one-pane session it runs in.
    #[derive(Debug, Clone, Copy)]
    struct Seeded {
        thread: ThreadId,
        session: SessionId,
        pane: PaneId,
    }

    /// The process the fake zmx runs the pane of thread `short_id` as.
    fn pid_of(short_id: &str) -> u32 {
        short_id.bytes().fold(7_u32, |pid, byte| {
            pid.wrapping_mul(31).wrapping_add(u32::from(byte)) % 1_000_000
        }) + 1_000
    }

    /// The Claude running in thread `short_id`'s pane, `status`.
    fn record(short_id: &str, status: ThreadStatus) -> RunningAgent {
        RunningAgent {
            status,
            ancestry: vec![pid_of(short_id)],
        }
    }

    /// A `claude` running `agents`.
    fn listing(agents: Vec<RunningAgent>) -> Arc<FakeClaude> {
        Arc::new(FakeClaude::running(agents))
    }

    /// A zmx running the pane of every thread `store` has bound to one, as
    /// process [`pid_of`] its short id, on the pane socket dir /zmx.
    fn zmx_running(store: &Store) -> ZmxService {
        let panes = store.layouts().map(|saved| saved.panes).unwrap_or_default();
        let rows = store.load().map(|(_, rows)| rows).unwrap_or_default();
        let listed: String = rows
            .iter()
            .filter_map(|row| {
                let pane = panes.iter().find(|pane| Some(pane.id) == row.pane_id)?;
                let name = pane.zmx_name.clone().unwrap_or_else(|| pane.id.zmx_name());
                Some(format!(
                    "  name={name}\tpid={}\tclients=1\tcreated=1\n",
                    pid_of(&row.short_id)
                ))
            })
            .collect();
        zmx_listing(&listed)
    }

    /// A store holding one thread in the orb project, created an hour ago.
    fn store_with_thread(short_id: &str) -> Result<(Store, ThreadId), Report<StoreError>> {
        let store = Store::open_in_memory()?;
        let id = add_thread(&store, short_id, now_ms() - HOUR_MS)?;
        Ok((store, id))
    }

    /// Saves the orb project rooted at [`PROJECT_ROOT`].
    fn orb_project(store: &Store) -> Result<ProjectId, Report<StoreError>> {
        store.add_project(Path::new(PROJECT_ROOT), "orb", ProjectKind::Normal, 0)
    }

    /// Saves thread `short_id` of `harness`, created at `created_at`, in a
    /// new `kind` session of project `project` in `dir`, one tab of one pane.
    fn seed_with(
        store: &Store,
        harness: &str,
        project: ProjectId,
        kind: SessionKind,
        dir: &Path,
        short_id: &str,
        created_at: i64,
    ) -> Result<Seeded, Report<StoreError>> {
        let (session, pane) = store.insert_session(project, kind, dir, None, created_at)?;
        let (thread, _) = store.insert_pane_thread(&NewPaneThread {
            pane,
            session_id: short_id.to_owned(),
            transcript_path: None,
            harness: HarnessId::new(harness),
            cwd: dir.to_owned(),
            created_at,
        })?;
        Ok(Seeded {
            thread,
            session,
            pane,
        })
    }

    /// Saves thread `short_id` of `harness`, created at `created_at`, in a
    /// new session of the orb project of its own, one tab of one pane.
    fn seed(
        store: &Store,
        harness: &str,
        short_id: &str,
        created_at: i64,
    ) -> Result<Seeded, Report<StoreError>> {
        let project = orb_project(store)?;
        seed_with(
            store,
            harness,
            project,
            SessionKind::Plain,
            Path::new(PROJECT_ROOT),
            short_id,
            created_at,
        )
    }

    /// Saves a Claude thread created at `created_at` in the orb project.
    fn add_thread(
        store: &Store,
        short_id: &str,
        created_at: i64,
    ) -> Result<ThreadId, Report<StoreError>> {
        seed(store, "claude", short_id, created_at).map(|seeded| seeded.thread)
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
    fn start(store: Store, claude: &Arc<FakeClaude>, claude_dir: &Path) -> (SessionsActor, State) {
        start_with(store, claude, &FakeGit::local(), claude_dir)
    }

    /// Starts the actor on `store` with `git`, making worktrees under
    /// [`WORKTREES_ROOT`].
    fn start_with(
        store: Store,
        claude: &Arc<FakeClaude>,
        git: &Arc<FakeGit>,
        claude_dir: &Path,
    ) -> (SessionsActor, State) {
        start_in(
            store,
            claude,
            git,
            claude_dir,
            Path::new(ORB_ROOT),
            Path::new(INCOGNITO_ROOT),
        )
    }

    /// Starts the actor on `store` with `git`, making worktrees under
    /// [`WORKTREES_ROOT`] and Research and Learn folders under `orb_root`.
    /// zmx runs every bound thread's pane (see [`zmx_running`]).
    fn start_in(
        store: Store,
        claude: &Arc<FakeClaude>,
        git: &Arc<FakeGit>,
        claude_dir: &Path,
        orb_root: &Path,
        incognito_root: &Path,
    ) -> (SessionsActor, State) {
        let state = State::default();
        let zmx = zmx_running(&store);
        let actor = SessionsActor::restore(SessionsActorDeps {
            services: Services {
                harnesses: Harnesses::new(vec![Arc::new(ClaudeCode::new(
                    claude.clone(),
                    claude_dir.to_owned(),
                ))]),
                git: GitService::new(git.clone()),
                zmx,
            },
            state: state.clone(),
            store,
            worktrees_root: PathBuf::from(WORKTREES_ROOT),
            orb_root: orb_root.to_owned(),
            incognito_root: incognito_root.to_owned(),
            wake: Arc::new(|| {}),
            integration_missing: false,
        });
        (actor, state)
    }

    /// The harness the routing tests run beside Claude.
    const OTHER: &str = "other";

    /// What the actor needs to run Claude over `claude` and `other` beside
    /// it, sharing `state`.
    fn deps_beside(
        store: Store,
        claude: &Arc<FakeClaude>,
        other: &Arc<FakeHarness>,
        state: &State,
    ) -> SessionsActorDeps {
        let git = FakeGit::local();
        let zmx = zmx_running(&store);
        SessionsActorDeps {
            services: Services {
                harnesses: Harnesses::new(vec![
                    Arc::new(ClaudeCode::new(
                        claude.clone(),
                        PathBuf::from(NO_CLAUDE_DIR),
                    )),
                    other.clone(),
                ]),
                git: GitService::new(git),
                zmx,
            },
            state: state.clone(),
            store,
            worktrees_root: PathBuf::from(WORKTREES_ROOT),
            orb_root: PathBuf::from(ORB_ROOT),
            incognito_root: PathBuf::from(INCOGNITO_ROOT),
            wake: Arc::new(|| {}),
            integration_missing: false,
        }
    }

    /// Starts the actor on `store` with Claude over `claude` and `other`
    /// beside it, the way `on_start` does, without the ticker.
    fn start_beside(
        store: Store,
        claude: &Arc<FakeClaude>,
        other: &Arc<FakeHarness>,
    ) -> (SessionsActor, State) {
        let state = State::default();
        let actor = SessionsActor::restore(deps_beside(store, claude, other, &state));
        (actor, state)
    }

    /// Saves a thread `short_id` of `harness` in the orb project, created an
    /// hour ago.
    fn add_thread_in(
        store: &Store,
        harness: &str,
        short_id: &str,
    ) -> Result<ThreadId, Report<StoreError>> {
        seed(store, harness, short_id, now_ms() - HOUR_MS).map(|seeded| seeded.thread)
    }

    /// Saves a Claude thread `short_id` in the orb project, created an hour
    /// ago, in a session of its own.
    fn insert_thread(store: &Store, short_id: &str) -> Result<Seeded, Report<StoreError>> {
        seed(store, "claude", short_id, now_ms() - HOUR_MS)
    }

    /// Where pane `pane` runs, as the restored layouts say.
    fn zmx_of(state: &State, pane: PaneId) -> Option<ZmxSession> {
        state
            .read()
            .layouts
            .entry(pane)
            .map(|entry| entry.zmx.clone())
    }

    #[rstest::rstest]
    fn thread_pane_runs_in_its_pane_rows_zmx_session() -> Result<(), Report<StoreError>> {
        // Given a saved thread whose harness runs no zmx session of its own.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let host = listing(Vec::new());

        // When restoring.
        let (_actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // Then its pane runs in orb-p<pane id> on orb's pane socket dir.
        assert_eq!(
            zmx_of(&state, inserted.pane),
            Some(ZmxSession {
                name: format!("orb-p{}", inserted.pane.0),
                dir: PathBuf::from("/zmx"),
            }),
            "a pane without a kept session gets its own on orb's dir"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn kept_pi_session_resolves_under_orbs_folder() -> Result<(), Report<StoreError>> {
        // Given a thread whose pane keeps zmx session `orb-abc` on dir `pi`,
        // as the migration leaves a pi thread's.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        let store = Store::open(&path)?;
        let inserted = insert_thread(&store, "aa")?;
        rusqlite::Connection::open(&path)
            .and_then(|conn| {
                conn.execute(
                    "UPDATE panes SET zmx_name = 'orb-abc', zmx_dir = 'pi' WHERE id = ?1",
                    [inserted.pane.0],
                )
            })
            .change_context(StoreError)?;
        let host = listing(Vec::new());

        // When restoring.
        let (_actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // Then it runs in orb-abc under orb's folder.
        assert_eq!(
            zmx_of(&state, inserted.pane),
            Some(ZmxSession {
                name: "orb-abc".into(),
                dir: Path::new(ORB_ROOT).join("pi"),
            }),
            "a relative socket dir is under orb's folder"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn thread_pane_starts_a_shell() -> Result<(), Report<StoreError>> {
        // Given a saved thread in a pane.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let host = listing(Vec::new());

        // When restoring.
        let (_actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // Then it shows only its pane and session: nothing runs but the
        // shell zmx starts, where the user types the agent.
        assert_eq!(
            shown(&state, inserted.thread).and_then(|thread| thread.pane),
            Some(PaneLaunch {
                pane: inserted.pane,
                session: inserted.session,
            }),
            "a thread's pane should carry no command of its harness"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn restore_loads_each_sessions_tabs_into_layouts() -> Result<(), Report<StoreError>> {
        // Given a thread's session saved with two tabs, the second shown.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let second = store.insert_pane(inserted.session, Path::new(PROJECT_ROOT))?;
        let tab = |position, pane: PaneId| TabRow {
            session_id: inserted.session,
            position,
            name: None,
            layout: format!("{{\"pane\":{}}}", pane.0),
            focus_pane: Some(pane),
        };
        store.save_layout(
            inserted.session,
            1,
            &[tab(0, inserted.pane), tab(1, second)],
            &[(inserted.pane, None), (second, None)],
        )?;
        let host = listing(Vec::new());

        // When restoring.
        let (_actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // Then its layout has both tabs, the second shown and focused on its pane.
        let restored = state
            .read()
            .layouts
            .get(inserted.session)
            .map(|layout| (layout.tabs().len(), layout.active(), layout.focused()));
        assert_eq!(
            restored,
            Some((2, 1, Some(second))),
            "the saved tabs should come back"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn restore_shows_each_saved_session() -> Result<(), Report<StoreError>> {
        // Given a thread's session in the orb project.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let host = listing(Vec::new());

        // When restoring.
        let (_actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // Then the session is shown in its directory.
        let shown: Vec<(SessionId, PathBuf)> = state
            .read()
            .sessions
            .sessions
            .iter()
            .map(|session| (session.id, session.dir.clone()))
            .collect();
        assert_eq!(
            shown,
            [(inserted.session, PathBuf::from(PROJECT_ROOT))],
            "every saved session should be shown"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn emptied_layout_settles_its_session() -> Result<(), Report<StoreError>> {
        // Given a thread's one-pane session whose last pane was closed.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let host = listing(Vec::new());
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        state.write().layouts.close_pane(inserted.pane);

        // When saving its layout.
        actor.save_layout(inserted.session);

        // Then the store keeps no tab for it and has it settled.
        let saved = actor.store.layouts()?;
        let settled = saved
            .sessions
            .iter()
            .find(|row| row.id == inserted.session)
            .and_then(|row| row.settled_override);
        assert_eq!(
            (saved.tabs.len(), settled),
            (0, Some(SettledOverride::Settled)),
            "a session whose last pane closed is saved empty and settled"
        );
        Ok(())
    }

    /// A zmx that lists no sessions and records each command, on the pane
    /// socket dir /zmx.
    fn recording_zmx() -> (Arc<FakeZmx>, ZmxService) {
        let zmx = Arc::new(FakeZmx::new(ZmxOutput {
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        }));
        (zmx.clone(), ZmxService::new(zmx, PathBuf::from("/zmx")))
    }

    /// The zmx session names `zmx` was asked to kill, in order.
    fn killed(zmx: &FakeZmx) -> Vec<String> {
        zmx.calls()
            .into_iter()
            .filter_map(|argv| {
                let at = argv.iter().position(|word| word == "kill")?;
                argv.get(at + 1)
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .collect()
    }

    /// The saved row of session `id`.
    fn saved_session_row(store: &Store, id: SessionId) -> Option<SessionRow> {
        store
            .layouts()
            .ok()?
            .sessions
            .into_iter()
            .find(|row| row.id == id)
    }

    #[rstest::rstest]
    fn pin_session_saves_its_pin() -> Result<(), Report<StoreError>> {
        // Given thread aa's session.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let host = listing(Vec::new());
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When pinning it.
        actor.pin_session(inserted.session);

        // Then its row is saved pinned.
        assert!(
            saved_session_row(&actor.store, inserted.session)
                .is_some_and(|row| row.pinned_at.is_some()),
            "a pinned session should be saved pinned"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn pin_on_a_settled_session_unsettles_it() -> Result<(), Report<StoreError>> {
        // Given thread aa's session, settled.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let host = listing(Vec::new());
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.edit_session(inserted.session, settle_session_row);

        // When pinning it.
        actor.pin_session(inserted.session);

        // Then it shows active again.
        assert_eq!(
            state
                .read()
                .sessions
                .session(inserted.session)
                .map(|session| session.settled_at),
            Some(None),
            "pinning a settled session should bring it back"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn unpin_session_clears_its_pin() -> Result<(), Report<StoreError>> {
        // Given thread aa's session, pinned.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let host = listing(Vec::new());
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.pin_session(inserted.session);

        // When unpinning it.
        actor.unpin_session(inserted.session);

        // Then its row is saved unpinned.
        assert!(
            saved_session_row(&actor.store, inserted.session)
                .is_some_and(|row| row.pinned_at.is_none()),
            "an unpinned session should be saved unpinned"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn rename_session_saves_the_name() -> Result<(), Report<StoreError>> {
        // Given thread aa's session.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let host = listing(Vec::new());
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When naming it "auth".
        actor.rename_session(inserted.session, Some("auth".to_owned()));

        // Then the store returns the name.
        assert_eq!(
            saved_session_row(&actor.store, inserted.session).and_then(|row| row.name),
            Some("auth".to_owned()),
            "a session's r name should be saved"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn rename_session_shows_the_name_as_its_title() -> Result<(), Report<StoreError>> {
        // Given thread aa's session.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let host = listing(Vec::new());
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When naming it "auth".
        actor.rename_session(inserted.session, Some("auth".to_owned()));

        // Then its title is the name.
        let app = state.read();
        let title = app
            .sessions
            .session(inserted.session)
            .map(|session| app.sessions.title(session));
        assert_eq!(title.as_deref(), Some("auth"), "the r name is the title");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn settle_session_marks_it_settled() -> Result<(), Report<StoreError>> {
        // Given thread aa's session, idle.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let host = listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When settling it.
        actor.settle_session(inserted.session);

        // Then it shows and is saved settled.
        let settled = (
            state
                .read()
                .sessions
                .session(inserted.session)
                .is_some_and(|session| session.settled_at.is_some()),
            saved_session_row(&actor.store, inserted.session).and_then(|row| row.settled_override),
        );
        assert_eq!(
            settled,
            (true, Some(SettledOverride::Settled)),
            "a settled session should show and be saved settled"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn settle_session_with_a_working_agent_is_ignored() -> Result<(), Report<StoreError>> {
        // Given thread aa's session, its agent working.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let host = listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When settling it.
        actor.settle_session(inserted.session);

        // Then it stays unsettled.
        assert_eq!(
            saved_session_row(&actor.store, inserted.session).and_then(|row| row.settled_override),
            None,
            "a turn that started after the key press wins"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn settle_session_kills_every_pane() -> Result<(), Report<StoreError>> {
        // Given thread aa's idle session with a second pane, and thread bb's
        // session.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let second = store.insert_pane(inserted.session, Path::new(PROJECT_ROOT))?;
        insert_thread(&store, "bb")?;
        let host = listing(Vec::new());
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        let (zmx, service) = recording_zmx();
        actor.services.zmx = service;

        // When settling aa's session.
        actor.settle_session(inserted.session);

        // Then both its panes are killed, and nothing of bb's.
        assert_eq!(
            killed(&zmx),
            vec![inserted.pane.zmx_name(), second.zmx_name()],
            "settling a session kills each of its panes"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn settle_session_unpins_it() -> Result<(), Report<StoreError>> {
        // Given thread aa's pinned idle session.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let host = listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;
        actor.pin_session(inserted.session);

        // When settling it.
        actor.settle_session(inserted.session);

        // Then it is saved without its pin.
        assert_eq!(
            saved_session_row(&actor.store, inserted.session).map(|row| row.pinned_at),
            Some(None),
            "a settle should remove the pin"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn settle_session_stops_nothing() -> Result<(), Report<StoreError>> {
        // Given an idle Claude in aa's pane.
        let mut fx = PaneStatus::new(
            false,
            vec![interactive(&[100], ThreadStatus::Idle)],
            running,
        )?;
        fx.actor.poll().await;

        // When settling aa's session.
        fx.actor.settle_session(fx.thread.session);

        // Then nothing was asked to stop it.
        assert!(
            fx.host.stops().is_empty(),
            "settling kills panes and stops nothing through a harness"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn unsettle_session_keeps_it_active() -> Result<(), Report<StoreError>> {
        // Given thread aa's session, settled.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let host = listing(Vec::new());
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.edit_session(inserted.session, settle_session_row);

        // When un-settling it.
        actor.unsettle_session(inserted.session);

        // Then it is kept active.
        assert_eq!(
            saved_session_row(&actor.store, inserted.session).and_then(|row| row.settled_override),
            Some(SettledOverride::Active),
            "an un-settled session should be kept active"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn visit_marks_the_sessions_agents_seen() -> Result<(), Report<StoreError>> {
        // Given thread aa whose turn ended unseen.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let host = listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, state) = start_unselected(store, &host);
        actor.poll().await;
        host.set_running(Ok(vec![record("aa", ThreadStatus::Idle)]));
        actor.poll().await;

        // When visiting its session.
        actor.visit(inserted.session);

        // Then aa is seen.
        assert_eq!(
            shown(&state, inserted.thread).map(|thread| thread.unseen),
            Some(false),
            "visiting a session should see its agents' turns"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn delete_session_kills_every_pane() -> Result<(), Report<StoreError>> {
        // Given thread aa's session with a second pane, and thread bb's
        // session.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let second = store.insert_pane(inserted.session, Path::new(PROJECT_ROOT))?;
        insert_thread(&store, "bb")?;
        let host = listing(Vec::new());
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        let (zmx, service) = recording_zmx();
        actor.services.zmx = service;

        // When deleting aa's session.
        actor.delete_session(inserted.session);

        // Then both its panes are killed, and nothing of bb's.
        assert_eq!(
            killed(&zmx),
            vec![inserted.pane.zmx_name(), second.zmx_name()],
            "deleting a session kills each of its panes"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn delete_session_removes_its_threads_tabs_panes_and_row()
    -> Result<(), Report<StoreError>> {
        // Given thread aa's session.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let host = listing(Vec::new());
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When deleting it.
        actor.delete_session(inserted.session);

        // Then the store keeps no session, tab, pane or thread of it.
        let layouts = actor.store.layouts()?;
        let left = (
            layouts.sessions.len(),
            layouts.tabs.len(),
            layouts.panes.len(),
            actor.store.load()?.1.len(),
        );
        assert_eq!(left, (0, 0, 0, 0), "a deleted session leaves nothing");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn delete_threadless_session_removes_it() -> Result<(), Report<StoreError>> {
        // Given a session whose agent has ended, so no thread runs in it.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        resave(&store, "aa", |row| ThreadRow {
            pane_id: None,
            ..row
        })?;
        let host = listing(Vec::new());
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When deleting it.
        actor.delete_session(inserted.session);

        // Then it is neither shown nor saved.
        let kept = (
            state.read().sessions.session(inserted.session).is_some(),
            saved_session_row(&actor.store, inserted.session).is_some(),
        );
        assert_eq!(kept, (false, false), "a session without threads still goes");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn delete_session_removes_its_ended_threads() -> Result<(), Report<StoreError>> {
        // Given thread aa's session, aa having ended in it.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        resave(&store, "aa", |row| ThreadRow {
            pane_id: None,
            ..row
        })?;
        let host = listing(Vec::new());
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When deleting the session.
        actor.delete_session(inserted.session);

        // Then aa is neither shown nor saved.
        let kept = (
            shown(&state, inserted.thread).is_some(),
            actor.store.load()?.1.len(),
        );
        assert_eq!(
            kept,
            (false, 0),
            "a deleted session takes its ended threads"
        );
        Ok(())
    }

    /// A store holding two sessions of orb's Research project in `dir`, made
    /// for threads aa and, when `shared`, bb.
    fn research_sessions(
        orb_root: &Path,
        dir: &Path,
        shared: bool,
    ) -> Result<(Store, Seeded), Report<StoreError>> {
        let store = Store::open_in_memory()?;
        let project_id = store.add_project(
            &orb_root.join("research"),
            "Research",
            ProjectKind::Research,
            0,
        )?;
        let insert = |short_id: &str| {
            seed_with(
                &store,
                "claude",
                project_id,
                SessionKind::Research,
                dir,
                short_id,
                now_ms() - HOUR_MS,
            )
        };
        let inserted = insert("aa")?;
        if shared {
            insert("bb")?;
        }
        Ok((store, inserted))
    }

    /// An actor on `store` with orb's folder at `orb_root`.
    fn start_at(store: Store, orb_root: &Path) -> (SessionsActor, State) {
        start_in(
            store,
            &listing(Vec::new()),
            &FakeGit::local(),
            Path::new(NO_CLAUDE_DIR),
            orb_root,
            Path::new(INCOGNITO_ROOT),
        )
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn delete_research_session_removes_its_folder() -> Result<(), Report<StoreError>> {
        // Given a Research session in its own folder under orb's.
        let orb = tempfile::tempdir().change_context(StoreError)?;
        let dir = orb.path().join("research").join("tokio-cancel");
        fs::create_dir_all(&dir).change_context(StoreError)?;
        let (store, inserted) = research_sessions(orb.path(), &dir, false)?;
        let (mut actor, _state) = start_at(store, orb.path());

        // When deleting it.
        actor.delete_session(inserted.session);

        // Then its folder is gone.
        assert!(!dir.exists(), "a Research session takes its folder");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn delete_research_session_keeps_a_folder_another_session_uses()
    -> Result<(), Report<StoreError>> {
        // Given two Research sessions in the same folder under orb's.
        let orb = tempfile::tempdir().change_context(StoreError)?;
        let dir = orb.path().join("research").join("tokio-cancel");
        fs::create_dir_all(&dir).change_context(StoreError)?;
        let (store, inserted) = research_sessions(orb.path(), &dir, true)?;
        let (mut actor, _state) = start_at(store, orb.path());

        // When deleting one of them.
        actor.delete_session(inserted.session);

        // Then the folder stays.
        assert!(dir.exists(), "a folder another session uses stays");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn delete_research_session_leaves_a_folder_outside_orbs_own()
    -> Result<(), Report<StoreError>> {
        // Given a Research session whose folder isn't under orb's research
        // folder.
        let orb = tempfile::tempdir().change_context(StoreError)?;
        let outside = tempfile::tempdir().change_context(StoreError)?;
        let (store, inserted) = research_sessions(orb.path(), outside.path(), false)?;
        let (mut actor, _state) = start_at(store, orb.path());

        // When deleting it.
        actor.delete_session(inserted.session);

        // Then that folder is still there.
        assert!(
            outside.path().is_dir(),
            "only a session's own folder under orb's is removed"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn delete_research_session_whose_folder_is_gone_shows_no_error()
    -> Result<(), Report<StoreError>> {
        // Given a Research session whose own folder was already removed by
        // hand.
        let orb = tempfile::tempdir().change_context(StoreError)?;
        let dir = orb.path().join("research").join("tokio-cancel");
        let (store, inserted) = research_sessions(orb.path(), &dir, false)?;
        let (mut actor, state) = start_at(store, orb.path());

        // When deleting it.
        actor.delete_session(inserted.session);

        // Then no error shows.
        assert_eq!(
            error_of(&state),
            None,
            "an already-removed folder counts as removed"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn delete_session_removes_its_pane_files() -> Result<(), Report<StoreError>> {
        // Given thread aa's pane has a report.
        let mut fx = PaneReports::new()?;
        fx.report(fx.thread.pane, "start", "aa", Some("startup"))?;
        fx.actor.poll().await;

        // When deleting aa's session.
        fx.actor.delete_session(fx.thread.session);

        // Then the pane's file is gone.
        assert!(
            !fx.file(fx.thread.pane).exists(),
            "a deleted session's pane files should go"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn delete_session_removes_nothing_through_claude() -> Result<(), Report<StoreError>> {
        // Given an idle Claude in aa's pane.
        let mut fx = PaneStatus::new(
            false,
            vec![interactive(&[100], ThreadStatus::Idle)],
            running,
        )?;
        fx.actor.poll().await;

        // When deleting aa's session.
        fx.actor.delete_session(fx.thread.session);

        // Then nothing was asked of the harness.
        assert!(
            fx.host.stops().is_empty(),
            "deleting kills panes and asks nothing of a harness"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn delete_session_unhides_it() -> Result<(), Report<StoreError>> {
        // Given thread aa's session, hidden as being deleted.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let host = listing(Vec::new());
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        state.write().sessions.deleting.insert(inserted.session);

        // When deleting it.
        actor.delete_session(inserted.session);

        // Then nothing is left hidden.
        assert!(
            state.read().sessions.deleting.is_empty(),
            "a deleted session shouldn't stay marked as being deleted"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn restore_keeps_a_migrated_session_jump() -> Result<(), Report<StoreError>> {
        // Given a saved jump to thread aa's session.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        store.save_jumps(&[SidebarItem::Session(inserted.session)])?;

        // When the actor starts.
        let (_actor, state) = start(store, &listing(Vec::new()), Path::new(NO_CLAUDE_DIR));

        // Then the jump is in the list.
        assert_eq!(
            state.read().jumps.entries(),
            [SidebarItem::Session(inserted.session)],
            "a saved session jump should come back"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn session_without_agents_never_auto_settles() -> Result<(), Report<StoreError>> {
        // Given a session idle for four days whose agent has ended.
        let (store, _) = store_with_thread("aa")?;
        let session = saved_session(&store, "aa")?.id;
        session_idle_for_four_days(&store, "aa")?;
        resave(&store, "aa", |row| ThreadRow {
            pane_id: None,
            ..row
        })?;
        let host = listing(Vec::new());
        let (mut actor, _state) = start_unselected(store, &host);

        // When polling.
        actor.poll().await;

        // Then the session stays active.
        assert_eq!(
            saved_session_row(&actor.store, session).and_then(|row| row.settled_override),
            None,
            "a session of shells only never settles by itself"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn split_pane_saves_a_new_focused_pane() -> Result<(), Report<StoreError>> {
        // Given a thread's one-pane session.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let host = listing(Vec::new());
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When splitting it right.
        actor.add_pane(inserted.session, Some(Split::Right));

        // Then the store has a second pane, which the tab focuses.
        let saved = actor.store.layouts()?;
        let new_pane = saved
            .panes
            .iter()
            .map(|pane| pane.id)
            .find(|id| *id != inserted.pane);
        let focus: Vec<Option<PaneId>> = saved.tabs.iter().map(|tab| tab.focus_pane).collect();
        assert_eq!(
            (new_pane.is_some(), focus),
            (true, vec![new_pane]),
            "a split should save a new focused pane"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_tab_saves_a_new_tab() -> Result<(), Report<StoreError>> {
        // Given a thread's one-pane session.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let host = listing(Vec::new());
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When opening a tab.
        actor.add_pane(inserted.session, None);

        // Then the store has two tabs, the second shown.
        let saved = actor.store.layouts()?;
        let active: Vec<usize> = saved.sessions.iter().map(|row| row.active_tab).collect();
        assert_eq!(
            (saved.tabs.len(), active),
            (2, vec![1]),
            "a new tab should be saved and shown"
        );
        Ok(())
    }

    /// A thread's session whose last pane closed and was saved, so it has no
    /// layout.
    fn emptied_session() -> Result<(SessionsActor, State, Seeded), Report<StoreError>> {
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let host = listing(Vec::new());
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        state.write().layouts.close_pane(inserted.pane);
        actor.save_layout(inserted.session);
        Ok((actor, state, inserted))
    }

    #[rstest::rstest]
    fn new_tab_on_a_session_without_a_layout_opens_one_shell_tab() -> Result<(), Report<StoreError>>
    {
        // Given a session whose last pane closed.
        let (mut actor, state, inserted) = emptied_session()?;

        // When opening a tab.
        actor.add_pane(inserted.session, None);

        // Then the session's layout holds one pane.
        assert_eq!(
            state.read().layouts.session_panes(inserted.session).len(),
            1,
            "an emptied session should get one shell"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_tab_on_a_session_without_a_layout_asks_to_attach_it() -> Result<(), Report<StoreError>> {
        // Given a session whose last pane closed.
        let (mut actor, state, inserted) = emptied_session()?;

        // When opening a tab.
        actor.add_pane(inserted.session, None);

        // Then the frontend is asked to attach it.
        assert_eq!(
            state.read().sessions.attach,
            Some(inserted.session),
            "the new shell should be attached"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn split_on_a_session_without_a_layout_does_nothing() -> Result<(), Report<StoreError>> {
        // Given a session whose last pane closed.
        let (mut actor, state, inserted) = emptied_session()?;

        // When splitting its pane.
        actor.add_pane(inserted.session, Some(Split::Right));

        // Then it still has no layout and no saved pane.
        assert_eq!(
            (
                state.read().layouts.get(inserted.session).is_none(),
                actor.store.layouts()?.panes.len()
            ),
            (true, 0),
            "there is no pane to split"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn save_layout_writes_the_sessions_tabs() -> Result<(), Report<StoreError>> {
        // Given a thread's session whose tab was renamed in the app state.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        let host = listing(Vec::new());
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        state
            .write()
            .layouts
            .rename_tab(inserted.session, 0, Some("agent".into()));

        // When saving its layout.
        actor.save_layout(inserted.session);

        // Then the store has the name.
        let names: Vec<Option<String>> = actor
            .store
            .layouts()?
            .tabs
            .into_iter()
            .map(|tab| tab.name)
            .collect();
        assert_eq!(
            names,
            vec![Some("agent".to_owned())],
            "the tab's name should be saved"
        );
        Ok(())
    }

    /// A store at `path` holding Claude threads `aa` and `bb`, whose panes
    /// still name their `--bg` sessions, as the migration leaves them.
    fn migrated_store(path: &Path) -> Result<Store, Report<StoreError>> {
        let store = Store::open(path)?;
        for short_id in ["aa", "bb"] {
            let pane = insert_thread(&store, short_id)?.pane;
            rusqlite::Connection::open(path)
                .and_then(|conn| {
                    conn.execute(
                        "UPDATE panes SET migrated_bg = ?2 WHERE id = ?1",
                        rusqlite::params![pane.0, short_id],
                    )
                })
                .change_context(StoreError)?;
        }
        Store::open(path)
    }

    /// The `--bg` sessions panes in `actor`'s store still name.
    fn still_marked(actor: &SessionsActor) -> Result<Vec<String>, Report<StoreError>> {
        Ok(actor
            .store
            .layouts()?
            .panes
            .into_iter()
            .filter_map(|pane| pane.migrated_bg)
            .collect())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn stop_migrated_stops_only_live_sessions() -> Result<(), Report<StoreError>> {
        // Given panes still naming --bg sessions aa and bb, of which Claude
        // lists only bb as live.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let store = migrated_store(&dir.path().join("state.sqlite"))?;
        let host = listing(Vec::new());
        host.set_live(Ok(vec!["bb".to_owned()]));
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When stopping the migrated sessions.
        actor.stop_migrated(|_| {}).await;

        // Then Claude stopped bb alone.
        assert_eq!(
            host.stopped(),
            vec!["bb".to_owned()],
            "only a live --bg session should be stopped"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn stop_migrated_forgets_sessions_that_are_not_live() -> Result<(), Report<StoreError>> {
        // Given panes still naming --bg sessions aa and bb, neither of which
        // Claude lists as live (done, stopped, failed or gone).
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let store = migrated_store(&dir.path().join("state.sqlite"))?;
        let host = listing(Vec::new());
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When stopping the migrated sessions.
        actor.stop_migrated(|_| {}).await;

        // Then no pane names a --bg session any more.
        let marked = still_marked(&actor)?;
        assert!(
            marked.is_empty(),
            "sessions with nothing to stop should be forgotten: {marked:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn stop_migrated_forgets_each_session_as_its_stop_returns()
    -> Result<(), Report<StoreError>> {
        // Given live --bg sessions aa and bb, where stopping bb never ends.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let store = migrated_store(&dir.path().join("state.sqlite"))?;
        let host = listing(Vec::new());
        host.set_live(Ok(vec!["aa".to_owned(), "bb".to_owned()]));
        host.hang_on("bb");
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When the stops are interrupted while bb's runs.
        let _ = tokio::time::timeout(Duration::from_millis(100), actor.stop_migrated(|_| {})).await;

        // Then only bb is still named.
        assert_eq!(
            still_marked(&actor)?,
            vec!["bb".to_owned()],
            "aa should be forgotten once its stop returned"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn stop_migrated_stops_every_session_when_claude_cant_list()
    -> Result<(), Report<StoreError>> {
        // Given panes still naming --bg sessions aa and bb, and a Claude that
        // can't list its sessions.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let store = migrated_store(&dir.path().join("state.sqlite"))?;
        let host = listing(Vec::new());
        host.set_live(Err("claude agents failed".to_owned()));
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When stopping the migrated sessions.
        actor.stop_migrated(|_| {}).await;

        // Then Claude stopped aa and then bb.
        assert_eq!(
            host.stopped(),
            vec!["aa".to_owned(), "bb".to_owned()],
            "every marked --bg session should be stopped in pane order"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn thread_of_an_unknown_harness_shows_gone() -> Result<(), Report<StoreError>> {
        // Given a saved thread of a harness orb doesn't know.
        let store = Store::open_in_memory()?;
        let id = add_thread_in(&store, "gone-harness", "aa")?;
        let host = listing(Vec::new());

        // When restoring.
        let (_actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // Then it shows gone.
        assert_eq!(
            status_of(&state, id),
            Some(ThreadStatus::Gone),
            "a thread of an unknown harness should be gone"
        );
        Ok(())
    }

    /// Claude's thread `aa` and the other harness's thread `bb`, both shown
    /// idle by one poll, after which the other harness's list fails while
    /// Claude lists `aa` working and polls again.
    async fn poll_with_a_failed_list() -> Result<(State, ThreadId, ThreadId), Report<StoreError>> {
        let store = Store::open_in_memory()?;
        let claude_thread = add_thread_in(&store, "claude", "aa")?;
        let other_thread = add_thread_in(&store, OTHER, "bb")?;
        let claude = listing(vec![record("aa", ThreadStatus::Idle)]);
        let other = Arc::new(FakeHarness::new(
            OTHER,
            vec![record("bb", ThreadStatus::Idle)],
        ));
        let (mut actor, state) = start_beside(store, &claude, &other);
        actor.poll().await;
        other.set_running(Err("supervisor down".to_owned()));
        claude.set_running(Ok(vec![record("aa", ThreadStatus::Working)]));
        actor.poll().await;
        Ok((state, claude_thread, other_thread))
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_list_keeps_its_threads_statuses() -> Result<(), Report<StoreError>> {
        // Given threads of two harnesses a poll showed idle.
        // When the other harness's list fails on the next poll.
        let (state, _, other_thread) = poll_with_a_failed_list().await?;

        // Then its thread keeps the status it had.
        assert_eq!(
            status_of(&state, other_thread),
            Some(ThreadStatus::Idle),
            "a failed list should leave its harness's threads as they were"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_list_of_one_harness_still_updates_the_others_threads()
    -> Result<(), Report<StoreError>> {
        // Given threads of two harnesses a poll showed idle.
        // When the other harness's list fails while Claude lists its thread working.
        let (state, claude_thread, _) = poll_with_a_failed_list().await?;

        // Then Claude's thread shows working.
        assert_eq!(
            status_of(&state, claude_thread),
            Some(ThreadStatus::Working),
            "one harness's failed list shouldn't hold back the others' threads"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn restore_shows_each_harness_mark() -> Result<(), Report<StoreError>> {
        // Given Claude and the other harness.
        let claude = listing(Vec::new());
        let other = Arc::new(FakeHarness::new(OTHER, Vec::new()));

        // When restoring.
        let (_actor, state) = start_beside(Store::open_in_memory()?, &claude, &other);

        // Then each harness shows with its mark, in registration order.
        let marks: Vec<(HarnessId, Option<String>)> = state
            .read()
            .harnesses
            .iter()
            .map(|info| (info.id.clone(), info.icon.clone()))
            .collect();
        assert_eq!(
            marks,
            vec![
                (HarnessId::new("claude"), Some("✳".to_owned())),
                (HarnessId::new(OTHER), None),
            ],
            "restore should publish each harness's mark"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn restore_ignores_saved_groups() -> Result<(), Report<StoreError>> {
        // Given a store holding a group row and no session.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        let project = orb_project(&Store::open(&path)?)?;
        rusqlite::Connection::open(&path)
            .and_then(|conn| {
                conn.execute(
                    "INSERT INTO groups (project_id, kind, name, created_at)
                     VALUES (?1, 'research', 'tokio-cancel', 1000)",
                    [project.0],
                )
            })
            .change_context(StoreError)?;

        // When the actor starts.
        let (_actor, state) = start(
            Store::open(&path)?,
            &listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // Then the sidebar has no row for it.
        assert_eq!(
            state.read().sessions.sidebar().len(),
            0,
            "a saved group should not show"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn restore_without_integrations_shows_the_install_nudge() -> Result<(), Report<StoreError>> {
        // Given neither orb's Claude hook nor its pi extension is installed.
        let claude = listing(Vec::new());
        let other = Arc::new(FakeHarness::new(OTHER, Vec::new()));
        let state = State::default();
        let deps = SessionsActorDeps {
            integration_missing: true,
            ..deps_beside(Store::open_in_memory()?, &claude, &other, &state)
        };

        // When restoring.
        let _actor = SessionsActor::restore(deps);

        // Then the mode line says how to install them.
        assert_eq!(
            state.read().sessions.error.as_deref(),
            Some(crate::feat::integration::NUDGE),
            "a missing integration should show the install nudge"
        );
        Ok(())
    }

    fn error_of(state: &State) -> Option<String> {
        state.read().sessions.error.clone()
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

    /// A Claude directory holding `lines` as the transcript of conversation
    /// `aa` in [`HEX_WORKTREE`], and a store whose thread `aa` is there on
    /// `branch`.
    fn worktree_thread(
        lines: &str,
        branch: &str,
    ) -> Result<(tempfile::TempDir, Store, ThreadId), Report<StoreError>> {
        let claude_dir = tempfile::tempdir().change_context(StoreError)?;
        let path = transcript_path(claude_dir.path(), Path::new(HEX_WORKTREE), "aa");
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

    /// The Claude in thread `aa`'s pane, `status`.
    fn in_session(status: ThreadStatus) -> RunningAgent {
        record("aa", status)
    }

    /// Polls once while thread `aa` works, then once after its turn ended.
    async fn end_turn(actor: &mut SessionsActor, host: &FakeClaude) {
        host.set_running(Ok(vec![in_session(ThreadStatus::Working)]));
        actor.poll().await;
        host.set_running(Ok(vec![in_session(ThreadStatus::Idle)]));
        actor.poll().await;
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_shows_the_status_of_each_threads_record() -> Result<(), Report<StoreError>> {
        // Given a saved thread and a host reporting it waiting for an approval.
        let (store, id) = store_with_thread("aa")?;
        let host = listing(vec![record("aa", ThreadStatus::NeedsApproval)]);
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
    async fn poll_marks_a_claude_thread_without_a_record_stopped() -> Result<(), Report<StoreError>>
    {
        // Given a saved thread the host doesn't list, its pane's zmx session gone.
        let (store, id) = store_with_thread("aa")?;
        let host = listing(vec![record("bb", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When polling.
        actor.poll().await;

        // Then the thread is stopped.
        assert_eq!(
            status_of(&state, id),
            Some(ThreadStatus::Stopped),
            "a thread with nothing running should be stopped"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_stamps_a_turn_when_it_starts() -> Result<(), Report<StoreError>> {
        // Given an unstamped thread the host now reports busy.
        let (store, id) = store_with_thread("aa")?;
        let host = listing(vec![record("aa", ThreadStatus::Working)]);
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
        let host = listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;
        let stamp = stamp_of(&state, id)
            .ok_or_else(|| Report::new(StoreError).attach("the busy poll didn't stamp the turn"))?;

        // When a poll sees it waiting for input.
        host.set_running(Ok(vec![record("aa", ThreadStatus::NeedsInput)]));
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
        let host = listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When a poll sees it idle.
        host.set_running(Ok(vec![record("aa", ThreadStatus::Idle)]));
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
        let host = listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        state.write().sessions.error = Some("A session is working in this directory".to_owned());

        // When a poll succeeds.
        actor.poll().await;

        // Then the failure still shows.
        assert_eq!(
            error_of(&state).as_deref(),
            Some("A session is working in this directory"),
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
        let host = listing(vec![record("aa", ThreadStatus::Working)]);
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
        let host = listing(vec![record("aa", ThreadStatus::Working)]);
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
        let host = listing(vec![record("aa", ThreadStatus::Idle)]);
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
        let host = listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When polling.
        let next = actor.poll().await;

        // Then the next poll is in 1 s.
        assert_eq!(next, FAST_POLL, "a running turn is watched closely");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_waits_five_seconds_while_no_turn_is_underway() -> Result<(), Report<StoreError>> {
        // Given an idle thread and orb attached.
        let (store, id) = store_with_thread("aa")?;
        let host = listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        {
            let mut app = state.write();
            app.focus = Focus::Sidebar;
            let session = app
                .sessions
                .threads()
                .find(|thread| thread.id == id)
                .and_then(Thread::session);
            app.attached.extend(session);
        }

        // When polling.
        let next = actor.poll().await;

        // Then the next poll is in 5 s.
        assert_eq!(
            next, SLOW_POLL,
            "being attached alone isn't watched closely"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_poll_keeps_the_last_statuses() -> Result<(), Report<StoreError>> {
        // Given a thread a poll saw idle.
        let (store, id) = store_with_thread("aa")?;
        let host = listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When the next poll fails.
        host.set_running(Err("claude: command not found".to_owned()));
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
        let host = listing(Vec::new());
        host.set_running(Err("claude: command not found".to_owned()));
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

    /// The actor on a store holding the orb project alone, with `git`, after
    /// making a session of it in `workspace`.
    fn made_session(
        git: &Arc<FakeGit>,
        workspace: Workspace,
    ) -> Result<(SessionsActor, State), Report<StoreError>> {
        let (store, id) = store_with_project()?;
        let (mut actor, state) =
            start_with(store, &listing(Vec::new()), git, Path::new(NO_CLAUDE_DIR));
        actor.new_session(id, workspace);
        Ok((actor, state))
    }

    /// The sessions the sidebar has, in store order.
    fn sessions_of(state: &State) -> Vec<Session> {
        state.read().sessions.sessions.clone()
    }

    /// The only session the sidebar has.
    fn only_session(state: &State) -> Result<Session, Report<StoreError>> {
        match sessions_of(state).as_slice() {
            [session] => Ok(session.clone()),
            other => Err(Report::new(StoreError).attach(format!("{} sessions", other.len()))),
        }
    }

    #[rstest::rstest]
    fn new_session_in_the_checkout_runs_in_the_project_root() -> Result<(), Report<StoreError>> {
        // Given / When making a session of the orb project in its checkout.
        let (_actor, state) = made_session(&FakeGit::local(), Workspace::Checkout)?;

        // Then it runs in the project's root.
        assert_eq!(
            only_session(&state)?.dir,
            PathBuf::from(PROJECT_ROOT),
            "a checkout session runs in the root"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_session_has_one_tab_with_one_pane() -> Result<(), Report<StoreError>> {
        // Given / When making a session of the orb project in its checkout.
        let (_actor, state) = made_session(&FakeGit::local(), Workspace::Checkout)?;

        // Then its layout holds one tab of one pane in the root.
        let id = only_session(&state)?.id;
        let app = state.read();
        let shape = app.layouts.get(id).map(|layout| {
            (
                layout.tabs().len(),
                layout
                    .panes()
                    .map(|pane| pane.cwd.clone())
                    .collect::<Vec<_>>(),
            )
        });
        assert_eq!(
            shape,
            Some((1, vec![PathBuf::from(PROJECT_ROOT)])),
            "a new session holds one tab of one shell pane"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_session_pane_runs_in_orbs_own_zmx_session() -> Result<(), Report<StoreError>> {
        // Given / When making a session of the orb project in its checkout.
        let (_actor, state) = made_session(&FakeGit::local(), Workspace::Checkout)?;

        // Then its pane runs in `orb-p<id>` on orb's pane socket dir.
        let id = only_session(&state)?.id;
        let app = state.read();
        let runs: Vec<(String, ZmxSession)> = app
            .layouts
            .get(id)
            .map(|layout| {
                layout
                    .panes()
                    .map(|pane| (pane.id.zmx_name(), pane.zmx.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let expected: Vec<(String, ZmxSession)> = runs
            .iter()
            .map(|(name, _)| {
                (
                    name.clone(),
                    ZmxSession {
                        name: name.clone(),
                        dir: PathBuf::from("/zmx"),
                    },
                )
            })
            .collect();
        assert_eq!(
            (runs.len(), &runs),
            (1, &expected),
            "a new pane runs in orb's own zmx session"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_session_is_selected() -> Result<(), Report<StoreError>> {
        // Given / When making a session of the orb project.
        let (_actor, state) = made_session(&FakeGit::local(), Workspace::Checkout)?;

        // Then the cursor is on it.
        let id = only_session(&state)?.id;
        assert_eq!(
            state.read().sessions.cursor,
            Some(SidebarItem::Session(id)),
            "a new session is selected"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_session_asks_the_frontend_to_attach() -> Result<(), Report<StoreError>> {
        // Given / When making a session of the orb project.
        let (_actor, state) = made_session(&FakeGit::local(), Workspace::Checkout)?;

        // Then the frontend is asked to attach it.
        let id = only_session(&state)?.id;
        assert_eq!(
            state.read().sessions.attach,
            Some(id),
            "a new session is attached"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_session_ends_the_start() -> Result<(), Report<StoreError>> {
        // Given a start in flight.
        let (store, id) = store_with_project()?;
        let (mut actor, state) = start(store, &listing(Vec::new()), Path::new(NO_CLAUDE_DIR));
        state.write().sessions.starting = true;

        // When making a session.
        actor.new_session(id, Workspace::Checkout);

        // Then nothing is starting any more.
        assert!(
            !state.read().sessions.starting,
            "a made session ends the start"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_session_in_a_new_worktree_runs_in_the_worktree_orb_made()
    -> Result<(), Report<StoreError>> {
        // Given / When making a session in a new worktree from `main`.
        let git = FakeGit::local();
        let (_actor, state) = made_session(
            &git,
            Workspace::NewWorktree {
                base: "main".to_owned(),
            },
        )?;

        // Then it runs in the worktree orb added, on its branch.
        let session = only_session(&state)?;
        assert_eq!(
            Some((session.dir, session.branch.unwrap_or_default())),
            git.added(),
            "the session runs in the new worktree"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_session_in_a_new_worktree_starts_from_its_base() -> Result<(), Report<StoreError>> {
        // Given / When making a session in a new worktree from `feat`, which
        // origin has.
        let git = FakeGit::with_origin(Ok(true));
        made_session(
            &git,
            Workspace::NewWorktree {
                base: "feat".to_owned(),
            },
        )?;

        // Then `feat` is fetched and the worktree starts from origin's.
        assert_eq!(
            fetch_and_add_steps(&git),
            ["fetch feat", "add from origin/feat"],
            "a new worktree starts from its base as origin has it"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_worktree_whose_fetch_fails_makes_no_session() -> Result<(), Report<StoreError>> {
        // Given / When making a session in a new worktree whose fetch fails.
        let git = FakeGit::with_origin(Err("fatal: unable to access origin"));
        let (_actor, state) = made_session(
            &git,
            Workspace::NewWorktree {
                base: "main".to_owned(),
            },
        )?;

        // Then no session is made.
        assert!(
            sessions_of(&state).is_empty(),
            "a failed fetch makes nothing"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_worktree_whose_fetch_fails_shows_the_reason() -> Result<(), Report<StoreError>> {
        // Given / When making a session in a new worktree whose fetch fails.
        let git = FakeGit::with_origin(Err("fatal: unable to access origin"));
        let (_actor, state) = made_session(
            &git,
            Workspace::NewWorktree {
                base: "main".to_owned(),
            },
        )?;

        // Then git's reason shows.
        assert_eq!(
            error_of(&state).as_deref(),
            Some("fatal: unable to access origin"),
            "a failed fetch shows why"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn failed_save_removes_the_new_worktree_and_branch() -> Result<(), Report<StoreError>> {
        // Given a project the sidebar shows but the store doesn't have.
        let git = FakeGit::local();
        let (mut actor, state) = start_with(
            Store::open_in_memory()?,
            &listing(Vec::new()),
            &git,
            Path::new(NO_CLAUDE_DIR),
        );
        state.write().sessions.projects.push(Project {
            id: ProjectId(99),
            title: "ghost".into(),
            root: PROJECT_ROOT.into(),
            created_at: SystemTime::UNIX_EPOCH,
            threads: vec![],
            repo: true,
            removed: false,
            kind: ProjectKind::Normal,
        });

        // When making a session of it in a new worktree.
        actor.new_session(
            ProjectId(99),
            Workspace::NewWorktree {
                base: "main".to_owned(),
            },
        );

        // Then the worktree and branch orb made are force-removed.
        let (path, branch) = git
            .added()
            .ok_or_else(|| Report::new(StoreError).attach("no worktree was added"))?;
        let calls = git.calls();
        assert!(
            calls.contains(&GitCall::RemoveWorktree { path, force: true })
                && calls.contains(&GitCall::DeleteBranch {
                    branch,
                    force: true
                }),
            "an unsaved session leaves no worktree or branch, got {calls:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_session_in_a_missing_checkout_fails_with_its_path() -> Result<(), Report<StoreError>> {
        // Given a project whose root is gone.
        let store = Store::open_in_memory()?;
        let id = store.add_project(
            Path::new("/nonexistent/gone"),
            "gone",
            ProjectKind::Normal,
            0,
        )?;
        let (mut actor, state) = start(store, &listing(Vec::new()), Path::new(NO_CLAUDE_DIR));

        // When making a session in its checkout.
        actor.new_session(id, Workspace::Checkout);

        // Then the mode line names the missing folder.
        assert_eq!(
            error_of(&state).as_deref(),
            Some("project folder no longer exists: /nonexistent/gone"),
            "a missing checkout shows its path"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_incognito_session_makes_its_folder() -> Result<(), Report<StoreError>> {
        // Given orb's Incognito folder gone after the actor started.
        let tmp = tempfile::tempdir().change_context(StoreError)?;
        let incognito = tmp.path().join("orb-incognito");
        let (mut actor, state) = start_in(
            Store::open_in_memory()?,
            &listing(Vec::new()),
            &FakeGit::local(),
            Path::new(NO_CLAUDE_DIR),
            Path::new(ORB_ROOT),
            &incognito,
        );
        fs::remove_dir_all(&incognito).change_context(StoreError)?;
        let id = state
            .read()
            .sessions
            .own_project(ProjectKind::Incognito)
            .map(|project| project.id)
            .ok_or_else(|| Report::new(StoreError).attach("no Incognito project"))?;

        // When making an Incognito session.
        actor.new_session(id, Workspace::Checkout);

        // Then its folder is back and the session runs there.
        assert_eq!(
            (incognito.is_dir(), only_session(&state)?.dir),
            (true, incognito.clone()),
            "an Incognito session makes its folder first"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_session_in_the_checkout_is_on_its_checked_out_branch() -> Result<(), Report<StoreError>>
    {
        // Given / When making a session of the orb project in its checkout.
        let (_actor, state) = made_session(&FakeGit::local(), Workspace::Checkout)?;

        // Then it shows the branch checked out there.
        assert_eq!(
            only_session(&state)?.branch.as_deref(),
            Some(CURRENT_BRANCH),
            "a checkout session is on the checkout's branch"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_session_in_a_missing_worktree_fails_with_its_path() -> Result<(), Report<StoreError>> {
        // Given / When making a session in a worktree that is gone.
        let (_actor, state) = made_session(
            &FakeGit::local(),
            Workspace::Existing(PathBuf::from("/nonexistent/worktree")),
        )?;

        // Then the mode line names the missing worktree.
        assert_eq!(
            error_of(&state).as_deref(),
            Some("worktree no longer exists: /nonexistent/worktree"),
            "a missing worktree shows its path"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_session_in_a_new_worktree_without_origin_starts_from_the_local_base()
    -> Result<(), Report<StoreError>> {
        // Given / When making a session in a new worktree from `feat` in a
        // repository without an origin.
        let git = FakeGit::local();
        made_session(
            &git,
            Workspace::NewWorktree {
                base: "feat".to_owned(),
            },
        )?;

        // Then nothing is fetched and the worktree starts from local `feat`.
        assert_eq!(
            fetch_and_add_steps(&git),
            ["add from feat"],
            "without an origin the base is used as it is locally"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_session_from_another_remotes_ref_uses_it_as_it_is() -> Result<(), Report<StoreError>> {
        // Given / When making a session in a new worktree from `upstream/x`.
        let git = FakeGit::with_origin(Ok(true));
        made_session(
            &git,
            Workspace::NewWorktree {
                base: "upstream/x".to_owned(),
            },
        )?;

        // Then origin isn't asked and the worktree starts from `upstream/x`.
        assert_eq!(
            fetch_and_add_steps(&git),
            ["add from upstream/x"],
            "another remote's ref is used as it is"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_worktree_session_shows_the_fetch_while_it_runs() -> Result<(), Report<StoreError>> {
        // Given an origin, and git watching the app state.
        let (store, id) = store_with_project()?;
        let git = FakeGit::with_origin(Ok(true));
        let (mut actor, state) =
            start_with(store, &listing(Vec::new()), &git, Path::new(NO_CLAUDE_DIR));
        git.watch(&state);

        // When making a session in a new worktree from `main`.
        actor.new_session(
            id,
            Workspace::NewWorktree {
                base: "main".to_owned(),
            },
        );

        // Then, while git fetched, origin/main showed as being fetched.
        assert_eq!(
            git.fetching_seen(),
            vec![Some("origin/main".to_owned())],
            "the fetch shows while it runs"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_worktree_whose_fetch_fails_clears_the_fetch() -> Result<(), Report<StoreError>> {
        // Given / When making a session in a new worktree whose fetch fails.
        let (_actor, state) = made_session(
            &FakeGit::with_origin(Err("fatal: unable to access origin")),
            Workspace::NewWorktree {
                base: "main".to_owned(),
            },
        )?;

        // Then no fetch shows any more.
        assert_eq!(
            state.read().sessions.fetching,
            None,
            "a failed fetch stops showing"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn failed_save_in_an_existing_worktree_removes_no_worktree() -> Result<(), Report<StoreError>> {
        // Given a project the sidebar shows but the store doesn't have, and a
        // worktree that already exists.
        let worktree = tempfile::tempdir().change_context(StoreError)?;
        let git = FakeGit::local();
        let (mut actor, state) = start_with(
            Store::open_in_memory()?,
            &listing(Vec::new()),
            &git,
            Path::new(NO_CLAUDE_DIR),
        );
        state.write().sessions.projects.push(Project {
            id: ProjectId(99),
            title: "ghost".into(),
            root: PROJECT_ROOT.into(),
            created_at: SystemTime::UNIX_EPOCH,
            threads: vec![],
            repo: true,
            removed: false,
            kind: ProjectKind::Normal,
        });

        // When making a session of it in that worktree.
        actor.new_session(
            ProjectId(99),
            Workspace::Existing(worktree.path().to_owned()),
        );

        // Then git is asked to remove no worktree and delete no branch.
        let calls = git.calls();
        assert!(
            !calls.iter().any(|call| matches!(
                call,
                GitCall::RemoveWorktree { .. } | GitCall::DeleteBranch { .. }
            )),
            "a worktree orb didn't make stays, got {calls:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_incognito_session_whose_folder_cant_be_made_shows_why() -> Result<(), Report<StoreError>>
    {
        // Given orb's Incognito folder under a regular file.
        let tmp = tempfile::tempdir().change_context(StoreError)?;
        let file = tmp.path().join("file");
        fs::write(&file, "").change_context(StoreError)?;
        let (mut actor, state) = start_in(
            Store::open_in_memory()?,
            &listing(Vec::new()),
            &FakeGit::local(),
            Path::new(NO_CLAUDE_DIR),
            Path::new(ORB_ROOT),
            &file.join("orb-incognito"),
        );
        let id = state
            .read()
            .sessions
            .own_project(ProjectKind::Incognito)
            .map(|project| project.id)
            .ok_or_else(|| Report::new(StoreError).attach("no Incognito project"))?;

        // When making an Incognito session.
        actor.new_session(id, Workspace::Checkout);

        // Then the mode line says the folder couldn't be made.
        assert_eq!(
            error_of(&state).as_deref(),
            Some("couldn't make the folder"),
            "an Incognito session needs its folder"
        );
        Ok(())
    }

    /// A store holding the orb project alone.
    fn store_with_project() -> Result<(Store, ProjectId), Report<StoreError>> {
        let store = Store::open_in_memory()?;
        let id = orb_project(&store)?;
        Ok((store, id))
    }

    /// Whether project `id` shows as a git repository.
    fn repo_of(state: &State, id: ProjectId) -> Option<bool> {
        state
            .read()
            .sessions
            .project(id)
            .map(|project| project.repo)
    }

    #[rstest::rstest]
    fn restore_marks_a_project_outside_git_not_a_repo() -> Result<(), Report<StoreError>> {
        // Given a saved project whose root isn't a git repository.
        let (store, id) = store_with_project()?;

        // When the actor starts.
        let (_actor, state) = start_with(
            store,
            &listing(Vec::new()),
            &FakeGit::plain(Ok(())),
            Path::new(NO_CLAUDE_DIR),
        );

        // Then the project shows as outside git.
        assert_eq!(
            repo_of(&state, id),
            Some(false),
            "a restored project should learn it isn't a repository"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn init_git_runs_git_init_in_the_root() -> Result<(), Report<StoreError>> {
        // Given a project that isn't a git repository.
        let (store, id) = store_with_project()?;
        let git = FakeGit::plain(Ok(()));
        let (mut actor, _state) =
            start_with(store, &listing(Vec::new()), &git, Path::new(NO_CLAUDE_DIR));

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
    fn init_git_marks_the_project_a_repo() -> Result<(), Report<StoreError>> {
        // Given a project that isn't a git repository.
        let (store, id) = store_with_project()?;
        let (mut actor, state) = start_with(
            store,
            &listing(Vec::new()),
            &FakeGit::plain(Ok(())),
            Path::new(NO_CLAUDE_DIR),
        );

        // When initializing git for it.
        actor.init_git(id);

        // Then the project shows as a git repository.
        assert_eq!(
            repo_of(&state, id),
            Some(true),
            "an initialized project should be a repository"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn failed_git_init_shows_why() -> Result<(), Report<StoreError>> {
        // Given a non-git project where git init fails.
        let (store, id) = store_with_project()?;
        let (mut actor, state) = start_with(
            store,
            &listing(Vec::new()),
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
    fn adding_a_directory_shows_it_as_a_project() -> Result<(), Report<StoreError>> {
        // Given no projects and an existing directory.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let (mut actor, state) = start(
            Store::open_in_memory()?,
            &listing(Vec::new()),
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
            .filter(|project| project.kind == ProjectKind::Normal)
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
        store.add_project(dir.path(), "web", ProjectKind::Normal, 0)?;
        let (mut actor, state) = start(store, &listing(Vec::new()), Path::new(NO_CLAUDE_DIR));

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
            &listing(Vec::new()),
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
            &listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // When adding a path that doesn't exist.
        actor.add_project(PathBuf::from("/nonexistent/project"));

        // Then no project is saved.
        assert!(
            !actor
                .store
                .load()?
                .0
                .iter()
                .any(|project| project.root == Path::new("/nonexistent/project")),
            "a missing directory shouldn't be saved"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_titles_a_thread_with_its_transcripts_prompt() -> Result<(), Report<StoreError>> {
        // Given a thread whose session's transcript has a prompt.
        let claude_dir = tempfile::tempdir().change_context(StoreError)?;
        let path = transcript_path(claude_dir.path(), Path::new(PROJECT_ROOT), "aa");
        fs::create_dir_all(path.parent().unwrap_or(claude_dir.path()))
            .change_context(StoreError)?;
        fs::write(
            &path,
            "{\"type\":\"user\",\"message\":{\"content\":\"Fix the sidebar\"}}\n",
        )
        .change_context(StoreError)?;
        let (store, id) = store_with_thread("aa")?;
        let host = listing(vec![record("aa", ThreadStatus::Idle)]);
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
        let path = transcript_path(claude_dir.path(), Path::new(PROJECT_ROOT), "aa");
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
        let host = listing(vec![record("aa", ThreadStatus::Idle)]);
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

    /// The directory of Claude's transcripts, holding session `s1` of thread
    /// `aa` with `lines`, and a store where `aa` was given `custom_title`
    /// by `/rename` and `renamed` with `r`.
    fn renamed_thread(
        lines: &str,
        custom_title: &str,
        renamed: &str,
    ) -> Result<(tempfile::TempDir, Store, ThreadId), Report<StoreError>> {
        let claude_dir = tempfile::tempdir().change_context(StoreError)?;
        let path = transcript_path(claude_dir.path(), Path::new(PROJECT_ROOT), "aa");
        fs::create_dir_all(path.parent().unwrap_or(claude_dir.path()))
            .change_context(StoreError)?;
        fs::write(&path, lines).change_context(StoreError)?;
        let (store, id) = store_with_thread("aa")?;
        resave(&store, "aa", |row| ThreadRow {
            custom_title: Some(custom_title.to_owned()),
            renamed_title: Some(renamed.to_owned()),
            ..row
        })?;
        Ok((claude_dir, store, id))
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn new_custom_title_replaces_the_orb_name() -> Result<(), Report<StoreError>> {
        // Given a thread renamed with `r` after a `/rename` to `orb-m1`,
        // whose transcript now has a `/rename` to `orb-m2`.
        let (claude_dir, store, id) = renamed_thread(
            "{\"type\":\"custom-title\",\"customTitle\":\"orb-m2\"}\n",
            "orb-m1",
            "Sidebar search",
        )?;
        let host = listing(vec![in_session(ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, claude_dir.path());

        // When polling.
        actor.poll().await;

        // Then the thread shows the new `/rename`.
        assert_eq!(
            shown(&state, id).and_then(|thread| thread.title).as_deref(),
            Some("orb-m2"),
            "a different `/rename` should replace the orb name"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn repeated_custom_title_keeps_the_orb_name() -> Result<(), Report<StoreError>> {
        // Given a thread renamed with `r` after a `/rename` to `orb-m1`,
        // whose transcript has Claude writing `orb-m1` again.
        let (claude_dir, store, id) = renamed_thread(
            "{\"type\":\"custom-title\",\"customTitle\":\"orb-m1\"}\n",
            "orb-m1",
            "Sidebar search",
        )?;
        let host = listing(vec![in_session(ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, claude_dir.path());

        // When polling.
        actor.poll().await;

        // Then the thread keeps the orb name.
        assert_eq!(
            shown(&state, id).and_then(|thread| thread.title).as_deref(),
            Some("Sidebar search"),
            "Claude re-writing its old name shouldn't replace the orb name"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn saved_orb_name_is_the_title_after_a_restart() -> Result<(), Report<StoreError>> {
        // Given a store holding a thread Claude titled and the user renamed.
        let (store, id) = store_with_thread("aa")?;
        resave(&store, "aa", |row| ThreadRow {
            title: Some("Fix the sidebar".to_owned()),
            renamed_title: Some("Sidebar search".to_owned()),
            ..row
        })?;
        let host = listing(Vec::new());

        // When orb starts on that store.
        let (_actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // Then the sidebar shows the orb name.
        assert_eq!(
            shown(&state, id).and_then(|thread| thread.title).as_deref(),
            Some("Sidebar search"),
            "the orb name should survive a restart"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn turn_end_notice_names_the_thread_by_its_orb_name() -> Result<(), Report<StoreError>> {
        // Given a thread `/rename`d "Parser fix" and renamed "Lexer" with `r`.
        let (store, _) = store_with_thread("aa")?;
        resave(&store, "aa", |row| ThreadRow {
            custom_title: Some("Parser fix".to_owned()),
            renamed_title: Some("Lexer".to_owned()),
            ..row
        })?;
        let host = listing(Vec::new());
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When polls see its turn run and then end.
        end_turn(&mut actor, &host).await;

        // Then the notice names the thread by its orb name.
        let titles: Vec<_> = notices_of(&state)
            .into_iter()
            .map(|notice| notice.title)
            .collect();
        assert_eq!(titles, ["Lexer"], "the notice's thread title");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_shows_the_located_transcript_on_the_thread() -> Result<(), Report<StoreError>> {
        // Given a thread whose session has a transcript.
        let claude_dir = tempfile::tempdir().change_context(StoreError)?;
        let path = transcript_path(claude_dir.path(), Path::new(PROJECT_ROOT), "aa");
        fs::create_dir_all(path.parent().unwrap_or(claude_dir.path()))
            .change_context(StoreError)?;
        fs::write(&path, "").change_context(StoreError)?;
        let (store, id) = store_with_thread("aa")?;
        let host = listing(vec![record("aa", ThreadStatus::Idle)]);
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
        let b = store.add_project(Path::new("/b"), "b", ProjectKind::Normal, 2)?;
        let a = store.add_project(Path::new("/a"), "a", ProjectKind::Normal, 1)?;
        let insert = |project_id, short_id: &str, created_at| {
            seed_with(
                &store,
                "claude",
                project_id,
                SessionKind::Plain,
                Path::new("/a"),
                short_id,
                created_at,
            )
            .map(|seeded| seeded.thread)
        };
        let a_old = insert(a, "a1", 10)?;
        let b_only = insert(b, "b1", 20)?;
        let a_new = insert(a, "a2", 30)?;

        // When the actor starts.
        let (_actor, state) = start(store, &listing(Vec::new()), Path::new(NO_CLAUDE_DIR));

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
        // Given an old pinned session and a newer unpinned one.
        let store = Store::open_in_memory()?;
        let pinned = add_thread(&store, "old", 10)?;
        add_thread(&store, "new", 20)?;
        store.save_session(&SessionRow {
            pinned_at: Some(30),
            ..saved_session(&store, "old")?
        })?;

        // When the actor starts.
        let (_actor, state) = start(store, &listing(Vec::new()), Path::new(NO_CLAUDE_DIR));

        // Then the pinned session, first in the sidebar, is selected.
        assert_eq!(
            state.read().sessions.cursor,
            Some(SidebarItem::Session(SessionId(pinned.0))),
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
    fn restore_drops_a_saved_jump_to_a_missing_thread() -> Result<(), Report<StoreError>> {
        // Given a saved jump list naming thread aa and a thread that's gone.
        let (store, id) = store_with_thread("aa")?;
        let gone = SidebarItem::Session(SessionId(id.0 + 1));
        store.save_jumps(&[SidebarItem::Session(SessionId(id.0)), gone])?;

        // When the actor starts.
        let (_actor, state) = start(store, &listing(Vec::new()), Path::new(NO_CLAUDE_DIR));

        // Then only thread aa is in the jump list.
        assert_eq!(
            state.read().jumps.entries(),
            [SidebarItem::Session(SessionId(id.0))],
            "a saved jump to a missing thread should be dropped"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn saving_jumps_writes_the_jump_list() -> Result<(), Report<StoreError>> {
        // Given a started actor whose jump list holds thread aa.
        let (store, id) = store_with_thread("aa")?;
        let (actor, state) = start(store, &listing(Vec::new()), Path::new(NO_CLAUDE_DIR));
        state.write().jumps = JumpList::from_saved(vec![SidebarItem::Session(SessionId(id.0))]);

        // When saving the jump list.
        actor.save_jumps();

        // Then the store holds it.
        assert_eq!(
            actor.store.jumps()?,
            vec![SidebarItem::Session(SessionId(id.0))],
            "the jump list should be saved"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn restore_applies_the_saved_sidebar_width() -> Result<(), Report<StoreError>> {
        // Given a sidebar saved 40 columns wide.
        let store = store_with_width(40)?;

        // When the actor starts.
        let (_actor, state) = start(store, &listing(Vec::new()), Path::new(NO_CLAUDE_DIR));

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
        let (_actor, state) = start(store, &listing(Vec::new()), Path::new(NO_CLAUDE_DIR));

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
            &listing(Vec::new()),
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
        // Given a project.
        let (store, id) = store_with_project()?;
        let (mut actor, state) = start(store, &listing(Vec::new()), Path::new(NO_CLAUDE_DIR));

        // When removing it.
        actor.remove_project(id);

        // Then it shows as removed.
        assert_eq!(
            removed_of(&state, id),
            Some(true),
            "a removed project should leave <C-g> n and the filter"
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
        let (_actor, state) = start(store, &listing(Vec::new()), Path::new(NO_CLAUDE_DIR));

        // Then the project shows as removed.
        assert_eq!(
            removed_of(&state, id),
            Some(true),
            "the removal should survive a restart"
        );
        Ok(())
    }

    /// Starts the actor on `store` with `incognito_root` as orb's Incognito
    /// folder.
    fn start_incognito(
        store: Store,
        host: &Arc<FakeClaude>,
        incognito_root: &Path,
    ) -> (SessionsActor, State) {
        start_in(
            store,
            host,
            &FakeGit::local(),
            Path::new(NO_CLAUDE_DIR),
            Path::new(ORB_ROOT),
            incognito_root,
        )
    }

    /// The titles of the Incognito projects the sidebar has.
    fn incognito_titles(state: &State) -> Vec<String> {
        state
            .read()
            .sessions
            .projects
            .iter()
            .filter(|project| project.kind == ProjectKind::Incognito)
            .map(|project| project.title.clone())
            .collect()
    }

    #[rstest::rstest]
    fn restore_adds_the_incognito_project() -> Result<(), Report<StoreError>> {
        // Given an empty store.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let root = dir.path().join("incognito");

        // When the actor starts.
        let (_actor, state) =
            start_incognito(Store::open_in_memory()?, &listing(Vec::new()), &root);

        // Then it has an Incognito project titled Incognito.
        assert_eq!(
            incognito_titles(&state),
            vec!["Incognito".to_owned()],
            "start should add orb's Incognito project"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn restore_keeps_one_incognito_project() -> Result<(), Report<StoreError>> {
        // Given a store that already holds the Incognito project.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let root = dir.path().join("incognito");
        let store = Store::open_in_memory()?;
        store.add_project(&root, "Incognito", ProjectKind::Incognito, 0)?;

        // When the actor starts.
        let (_actor, state) = start_incognito(store, &listing(Vec::new()), &root);

        // Then there's still one Incognito project.
        assert_eq!(
            incognito_titles(&state).len(),
            1,
            "start shouldn't add the Incognito project twice"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn restore_makes_the_incognito_folder() -> Result<(), Report<StoreError>> {
        // Given a missing Incognito folder.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let root = dir.path().join("incognito");

        // When the actor starts.
        let (_actor, _state) =
            start_incognito(Store::open_in_memory()?, &listing(Vec::new()), &root);

        // Then the folder exists.
        assert!(root.is_dir(), "start should make the Incognito folder");
        Ok(())
    }

    #[rstest::rstest]
    fn restore_un_removes_the_incognito_project() -> Result<(), Report<StoreError>> {
        // Given a removed Incognito project.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let root = dir.path().join("incognito");
        let store = Store::open_in_memory()?;
        let id = store.add_project(&root, "Incognito", ProjectKind::Incognito, 0)?;
        store.remove_project(id, 1)?;

        // When the actor starts.
        let (_actor, state) = start_incognito(store, &listing(Vec::new()), &root);

        // Then it isn't removed.
        assert_eq!(
            removed_of(&state, id),
            Some(false),
            "start should bring back a removed Incognito project"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn adding_a_removed_project_restores_it() -> Result<(), Report<StoreError>> {
        // Given a directory that's a removed project.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let store = Store::open_in_memory()?;
        let id = store.add_project(dir.path(), "web", ProjectKind::Normal, 0)?;
        store.remove_project(id, 1_000)?;
        let (mut actor, state) = start(store, &listing(Vec::new()), Path::new(NO_CLAUDE_DIR));

        // When adding it again.
        actor.add_project(dir.path().to_owned());

        // Then it's no longer removed.
        assert_eq!(
            removed_of(&state, id),
            Some(false),
            "<C-g> a on a removed project's directory should restore it"
        );
        Ok(())
    }

    /// A store with projects orb and web, each holding one thread, and its
    /// layout saved filtered to web. Returns web's id and thread.
    fn store_filtered_to_web() -> Result<(Store, ProjectId, ThreadId), Report<StoreError>> {
        let store = Store::open_in_memory()?;
        add_thread(&store, "aa", 1_000)?;
        let web = store.add_project(Path::new(WEB_ROOT), "web", ProjectKind::Normal, 0)?;
        let thread = seed_with(
            &store,
            "claude",
            web,
            SessionKind::Plain,
            Path::new(WEB_ROOT),
            "bb",
            500,
        )
        .map(|seeded| seeded.thread)?;
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
        let (_actor, state) = start(store, &listing(Vec::new()), Path::new(NO_CLAUDE_DIR));

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
        let (_actor, state) = start(store, &listing(Vec::new()), Path::new(NO_CLAUDE_DIR));

        // Then the cursor is on web's thread.
        assert_eq!(
            state.read().sessions.cursor,
            Some(SidebarItem::Session(SessionId(thread.0))),
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
        let (_actor, state) = start(store, &listing(Vec::new()), Path::new(NO_CLAUDE_DIR));

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
        let (actor, state) = start(store, &listing(Vec::new()), Path::new(NO_CLAUDE_DIR));
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
        store.add_project(Path::new(PROJECT_ROOT), "orb", ProjectKind::Normal, 1_500)?;

        // When the actor starts.
        let (_actor, state) = start(store, &listing(Vec::new()), Path::new(NO_CLAUDE_DIR));

        // Then the project shows it was added at 1.5 s.
        let added: Vec<SystemTime> = state
            .read()
            .sessions
            .projects
            .iter()
            .filter(|project| project.kind == ProjectKind::Normal)
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
    fn restore_keeps_when_each_thread_was_created() -> Result<(), Report<StoreError>> {
        // Given a thread created at 1.5 s.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new(PROJECT_ROOT), "orb", ProjectKind::Normal, 1)?;
        seed_with(
            &store,
            "claude",
            project_id,
            SessionKind::Plain,
            Path::new(PROJECT_ROOT),
            "t1",
            1_500,
        )
        .map(|seeded| seeded.thread)?;

        // When the actor starts.
        let (_actor, state) = start(store, &listing(Vec::new()), Path::new(NO_CLAUDE_DIR));

        // Then the thread shows it was created at 1.5 s.
        let created: Vec<SystemTime> = state
            .read()
            .sessions
            .threads()
            .map(|thread| thread.created_at)
            .collect();
        assert_eq!(
            created,
            vec![SystemTime::UNIX_EPOCH + Duration::from_millis(1_500)],
            "a restored thread should keep when it was created"
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

    #[rstest::rstest]
    #[tokio::test]
    async fn turn_end_on_an_unselected_thread_is_unseen() -> Result<(), Report<StoreError>> {
        // Given a working thread while another thread is selected.
        let (store, id) = store_with_thread("aa")?;
        let other = add_thread(&store, "bb", now_ms() - HOUR_MS)?;
        let host = listing(vec![
            record("aa", ThreadStatus::Working),
            record("bb", ThreadStatus::Idle),
        ]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        state.write().sessions.cursor = Some(SidebarItem::Session(SessionId(other.0)));
        actor.poll().await;

        // When a poll sees its turn end.
        host.set_running(Ok(vec![
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
        let host = listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        state.write().sessions.cursor = Some(SidebarItem::Session(SessionId(id.0)));
        actor.poll().await;

        // When a poll sees its turn end.
        host.set_running(Ok(vec![record("aa", ThreadStatus::Idle)]));
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
        let host = listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;
        let before = now_ms();

        // When a poll sees its turn end.
        host.set_running(Ok(vec![record("aa", ThreadStatus::Idle)]));
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
    async fn turn_end_without_a_scanned_branch_takes_the_branch_from_git()
    -> Result<(), Report<StoreError>> {
        // Given a branchless thread a poll saw working, whose transcript names no branch.
        let claude_dir = tempfile::tempdir().change_context(StoreError)?;
        let path = transcript_path(claude_dir.path(), Path::new(PROJECT_ROOT), "aa");
        fs::create_dir_all(path.parent().unwrap_or(claude_dir.path()))
            .change_context(StoreError)?;
        fs::write(&path, PROMPT_LINE).change_context(StoreError)?;
        let (store, _) = store_with_thread("aa")?;
        let working = record("aa", ThreadStatus::Working);
        let host = listing(vec![working.clone()]);
        let (mut actor, _state) = start(store, &host, claude_dir.path());
        actor.poll().await;

        // When a poll sees its turn end.
        host.set_running(Ok(vec![RunningAgent {
            status: ThreadStatus::Idle,
            ..working
        }]));
        actor.poll().await;

        // Then the thread takes the branch git has checked out there.
        assert_eq!(
            saved(&actor.store, "aa")?.branch.as_deref(),
            Some(CURRENT_BRANCH),
            "a turn end with no scanned branch should ask git"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_shows_the_transcript_branch() -> Result<(), Report<StoreError>> {
        // Given a thread whose transcript's prompt names a branch.
        let claude_dir = tempfile::tempdir().change_context(StoreError)?;
        let path = transcript_path(claude_dir.path(), Path::new(PROJECT_ROOT), "aa");
        fs::create_dir_all(path.parent().unwrap_or(claude_dir.path()))
            .change_context(StoreError)?;
        fs::write(
            &path,
            "{\"type\":\"user\",\"gitBranch\":\"main\",\"message\":{\"content\":\"Fix it\"}}\n",
        )
        .change_context(StoreError)?;
        let (store, id) = store_with_thread("aa")?;
        let host = listing(vec![record("aa", ThreadStatus::Idle)]);
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

    /// A store whose session holds untitled thread `aa` in the orb project's
    /// checkout, with a second pane.
    fn movable() -> Result<(Store, Seeded, PaneId), Report<StoreError>> {
        let store = Store::open_in_memory()?;
        let seeded = insert_thread(&store, "aa")?;
        let second = store.insert_pane(seeded.session, Path::new(PROJECT_ROOT))?;
        Ok((store, seeded, second))
    }

    /// What [`moved_to`] leaves: the actor, its state, the session's thread
    /// and first pane, its second pane, and the zmx it called after start.
    type Moved = (SessionsActor, State, Seeded, PaneId, Arc<FakeZmx>);

    /// An actor on [`movable`]'s store whose session moved to `to`.
    fn moved_to(to: Workspace, git: &Arc<FakeGit>) -> Result<Moved, Report<StoreError>> {
        let (store, seeded, second) = movable()?;
        let (mut actor, state) =
            start_with(store, &listing(Vec::new()), git, Path::new(NO_CLAUDE_DIR));
        let (zmx, service) = recording_zmx();
        actor.services.zmx = service;
        actor.change_workspace(seeded.session, to);
        Ok((actor, state, seeded, second, zmx))
    }

    /// The directory session `id` shows.
    fn dir_of(state: &State, id: SessionId) -> Option<PathBuf> {
        state
            .read()
            .sessions
            .session(id)
            .map(|session| session.dir.clone())
    }

    #[rstest::rstest]
    fn change_workspace_kills_every_pane() -> Result<(), Report<StoreError>> {
        // Given a session with two panes, before any agent turn.
        let to = tempfile::tempdir().change_context(StoreError)?;

        // When moving it to another existing directory.
        let (_actor, _state, seeded, second, zmx) =
            moved_to(Workspace::Existing(to.path().to_owned()), &FakeGit::local())?;

        // Then both its panes are killed.
        assert_eq!(
            killed(&zmx),
            vec![seeded.pane.zmx_name(), second.zmx_name()],
            "a moved session's panes start over"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn change_workspace_moves_the_sessions_dir() -> Result<(), Report<StoreError>> {
        // Given a session in the checkout, before any agent turn.
        let to = tempfile::tempdir().change_context(StoreError)?;

        // When moving it to another existing directory.
        let (_actor, state, seeded, _, _) =
            moved_to(Workspace::Existing(to.path().to_owned()), &FakeGit::local())?;

        // Then it shows in that directory.
        assert_eq!(
            dir_of(&state, seeded.session),
            Some(to.path().to_owned()),
            "the session should run in its new workspace"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn change_workspace_saves_every_panes_cwd() -> Result<(), Report<StoreError>> {
        // Given a session with two panes, before any agent turn.
        let to = tempfile::tempdir().change_context(StoreError)?;

        // When moving it to another existing directory.
        let (actor, _state, _, _, _) =
            moved_to(Workspace::Existing(to.path().to_owned()), &FakeGit::local())?;

        // Then both panes are saved there.
        let cwds: Vec<PathBuf> = actor
            .store
            .layouts()?
            .panes
            .into_iter()
            .map(|pane| pane.cwd)
            .collect();
        assert_eq!(
            cwds,
            vec![to.path().to_owned(); 2],
            "every pane should start in the new workspace"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn change_workspace_unbinds_the_threads_in_its_panes() -> Result<(), Report<StoreError>> {
        // Given thread aa in a session's pane, before any agent turn.
        let to = tempfile::tempdir().change_context(StoreError)?;

        // When moving the session.
        let (actor, _state, _, _, _) =
            moved_to(Workspace::Existing(to.path().to_owned()), &FakeGit::local())?;

        // Then aa runs in no pane.
        assert_eq!(
            saved(&actor.store, "aa")?.pane_id,
            None,
            "a killed agent's thread leaves its pane"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn change_workspace_asks_the_frontend_to_attach() -> Result<(), Report<StoreError>> {
        // Given a session before any agent turn.
        let to = tempfile::tempdir().change_context(StoreError)?;

        // When moving it.
        let (_actor, state, seeded, _, _) =
            moved_to(Workspace::Existing(to.path().to_owned()), &FakeGit::local())?;

        // Then the frontend is asked to attach it again.
        assert_eq!(
            state.read().sessions.attach,
            Some(seeded.session),
            "fresh shells start when the frontend attaches"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn change_workspace_after_a_turn_is_refused() -> Result<(), Report<StoreError>> {
        // Given a session whose thread is titled, so a turn has run.
        let (store, seeded, _) = movable()?;
        resave(&store, "aa", |row| ThreadRow {
            title: Some("Fix the parser".to_owned()),
            ..row
        })?;
        let (mut actor, state) = start(store, &listing(Vec::new()), Path::new(NO_CLAUDE_DIR));
        let to = tempfile::tempdir().change_context(StoreError)?;

        // When moving it.
        actor.change_workspace(seeded.session, Workspace::Existing(to.path().to_owned()));

        // Then it stays, locked.
        assert_eq!(
            (dir_of(&state, seeded.session), error_of(&state)),
            (
                Some(PathBuf::from(PROJECT_ROOT)),
                Some("Workspace locked · Local checkout".to_owned())
            ),
            "a session that had a turn keeps its workspace"
        );
        Ok(())
    }

    /// A store holding sessions of the orb project in the orb worktree
    /// `old`, for threads aa and, when `shared`, bb.
    fn in_orb_worktree(old: &Path, shared: bool) -> Result<(Store, Seeded), Report<StoreError>> {
        let store = Store::open_in_memory()?;
        let project = orb_project(&store)?;
        let insert = |short_id: &str| {
            seed_with(
                &store,
                "claude",
                project,
                SessionKind::Plain,
                old,
                short_id,
                now_ms() - HOUR_MS,
            )
        };
        let seeded = insert("aa")?;
        if shared {
            insert("bb")?;
        }
        Ok((store, seeded))
    }

    #[rstest::rstest]
    fn change_workspace_removes_the_old_orb_worktree() -> Result<(), Report<StoreError>> {
        // Given a session alone in an orb worktree.
        let old = PathBuf::from("/nonexistent/worktrees/orb/orb-0a1b2c3d");
        let (store, seeded) = in_orb_worktree(&old, false)?;
        let git = FakeGit::local();
        let (mut actor, _state) =
            start_with(store, &listing(Vec::new()), &git, Path::new(NO_CLAUDE_DIR));

        // When moving it to the checkout.
        actor.change_workspace(seeded.session, Workspace::Checkout);

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
                },
            ],
            "an orb worktree no session uses should go"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn change_workspace_keeps_an_old_worktree_another_session_uses()
    -> Result<(), Report<StoreError>> {
        // Given two sessions in the same orb worktree.
        let old = PathBuf::from("/nonexistent/worktrees/orb/orb-0a1b2c3d");
        let (store, seeded) = in_orb_worktree(&old, true)?;
        let git = FakeGit::local();
        let (mut actor, _state) =
            start_with(store, &listing(Vec::new()), &git, Path::new(NO_CLAUDE_DIR));

        // When moving one of them to the checkout.
        actor.change_workspace(seeded.session, Workspace::Checkout);

        // Then git removes nothing.
        assert!(
            git.calls().is_empty(),
            "a worktree another session uses stays"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn failed_change_workspace_removes_the_new_worktree() -> Result<(), Report<StoreError>> {
        // Given a session whose store refuses to move it.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        let store = Store::open(&path)?;
        let seeded = insert_thread(&store, "aa")?;
        rusqlite::Connection::open(&path)
            .and_then(|conn| {
                conn.execute_batch(
                    "CREATE TRIGGER refuse BEFORE UPDATE OF dir ON sessions
                     BEGIN SELECT RAISE(ABORT, 'refused'); END;",
                )
            })
            .change_context(StoreError)?;
        let git = FakeGit::local();
        let (mut actor, _state) =
            start_with(store, &listing(Vec::new()), &git, Path::new(NO_CLAUDE_DIR));

        // When moving it to a new worktree.
        actor.change_workspace(
            seeded.session,
            Workspace::NewWorktree {
                base: "main".to_owned(),
            },
        );

        // Then the worktree and branch orb made are force-removed.
        let (path, branch) = git
            .added()
            .ok_or_else(|| Report::new(StoreError).attach("no worktree was added"))?;
        let calls = git.calls();
        assert!(
            calls.contains(&GitCall::RemoveWorktree { path, force: true })
                && calls.contains(&GitCall::DeleteBranch {
                    branch,
                    force: true
                }),
            "a failed move leaves no worktree or branch, got {calls:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn failed_change_workspace_to_an_existing_worktree_removes_no_worktree()
    -> Result<(), Report<StoreError>> {
        // Given a session whose store refuses to move it, and a worktree that
        // already exists.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let worktree = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        let store = Store::open(&path)?;
        let seeded = insert_thread(&store, "aa")?;
        rusqlite::Connection::open(&path)
            .and_then(|conn| {
                conn.execute_batch(
                    "CREATE TRIGGER refuse BEFORE UPDATE OF dir ON sessions
                     BEGIN SELECT RAISE(ABORT, 'refused'); END;",
                )
            })
            .change_context(StoreError)?;
        let git = FakeGit::local();
        let (mut actor, _state) =
            start_with(store, &listing(Vec::new()), &git, Path::new(NO_CLAUDE_DIR));

        // When moving it to that worktree.
        actor.change_workspace(
            seeded.session,
            Workspace::Existing(worktree.path().to_owned()),
        );

        // Then git is asked to remove no worktree and delete no branch.
        let calls = git.calls();
        assert!(
            !calls.iter().any(|call| matches!(
                call,
                GitCall::RemoveWorktree { .. } | GitCall::DeleteBranch { .. }
            )),
            "a worktree orb didn't make stays, got {calls:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn change_workspace_whose_fetch_fails_keeps_the_session_where_it_was()
    -> Result<(), Report<StoreError>> {
        // Given a session in the checkout of a project whose fetch fails.
        let git = FakeGit::with_origin(Err("fatal: unable to access origin"));

        // When moving it to a new worktree from main.
        let (_actor, state, seeded, _, _) = moved_to(
            Workspace::NewWorktree {
                base: "main".to_owned(),
            },
            &git,
        )?;

        // Then it stays in the checkout, with git's reason shown.
        assert_eq!(
            (dir_of(&state, seeded.session), error_of(&state)),
            (
                Some(PathBuf::from(PROJECT_ROOT)),
                Some("fatal: unable to access origin".to_owned())
            ),
            "a move that can't reach its workspace keeps the session"
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

    /// The branch session `id` shows.
    fn branch_of(state: &State, id: SessionId) -> Option<String> {
        state.read().sessions.session(id)?.branch.clone()
    }

    #[rstest::rstest]
    fn switch_branch_checks_out_in_the_sessions_dir() -> Result<(), Report<StoreError>> {
        // Given a session in the orb project's checkout.
        let store = Store::open_in_memory()?;
        let seeded = insert_thread(&store, "aa")?;
        let host = listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When switching it to `feat`.
        actor.switch_branch(seeded.session, &git_ref("feat", false), false);

        // Then the sidebar shows it on feat.
        assert_eq!(
            branch_of(&state, seeded.session).as_deref(),
            Some("feat"),
            "the session should show its new branch"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn switch_branch_refused_after_the_directory_became_busy()
    -> Result<(), Report<StoreError>> {
        // Given a session whose agent's turn started after the picker opened.
        let store = Store::open_in_memory()?;
        let seeded = insert_thread(&store, "aa")?;
        let host = listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When switching it to `feat`.
        actor.switch_branch(seeded.session, &git_ref("feat", false), false);

        // Then the mode line says an agent is working there.
        assert_eq!(
            error_of(&state).as_deref(),
            Some("A session is working in this directory"),
            "a checkout under a running turn should be refused"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn switch_to_a_remote_ref_shows_the_local_branch_name() -> Result<(), Report<StoreError>> {
        // Given a session in the orb project's checkout.
        let store = Store::open_in_memory()?;
        let seeded = insert_thread(&store, "aa")?;
        let host = listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When switching it to `origin/feat`.
        actor.switch_branch(seeded.session, &git_ref("origin/feat", true), false);

        // Then the sidebar shows the tracking branch feat.
        assert_eq!(
            branch_of(&state, seeded.session).as_deref(),
            Some("feat"),
            "a remote ref checks out as its local branch"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn switch_branch_to_root_moves_the_session_to_the_checkout() -> Result<(), Report<StoreError>> {
        // Given a session with no turn in an orb worktree of a project on disk.
        let root = tempfile::tempdir().change_context(StoreError)?;
        let store = Store::open_in_memory()?;
        let project = store.add_project(root.path(), "orb", ProjectKind::Normal, 0)?;
        let seeded = seed_with(
            &store,
            "claude",
            project,
            SessionKind::Plain,
            &Path::new(WORKTREES_ROOT).join("orb/orb-0123abcd"),
            "aa",
            now_ms() - HOUR_MS,
        )?;
        let (mut actor, state) = start(store, &listing(Vec::new()), Path::new(NO_CLAUDE_DIR));

        // When picking the default branch, checked out nowhere.
        actor.switch_branch(seeded.session, &git_ref("main", false), true);

        // Then the session runs in the checkout.
        assert_eq!(
            dir_of(&state, seeded.session),
            Some(root.path().to_owned()),
            "the session should move back to the checkout"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn turn_end_renames_the_hex_branch_to_the_title_slug() -> Result<(), Report<StoreError>> {
        // Given a thread in an orb worktree on its hex branch, titled by Claude.
        let (claude_dir, store, id) =
            worktree_thread(&format!("{PROMPT_LINE}{AI_TITLE_LINE}"), HEX_BRANCH)?;
        let host = listing(Vec::new());
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
    async fn turn_end_renames_the_hex_branch_to_the_orb_name_slug() -> Result<(), Report<StoreError>>
    {
        // Given a thread on its hex branch titled only by its first prompt,
        // then renamed with `r`.
        let (claude_dir, store, id) = worktree_thread(PROMPT_LINE, HEX_BRANCH)?;
        resave(&store, "aa", |row| ThreadRow {
            renamed_title: Some("Sidebar search".to_owned()),
            ..row
        })?;
        let host = listing(Vec::new());
        let (mut actor, state) = start_with(store, &host, &FakeGit::local(), claude_dir.path());

        // When a poll sees its turn end.
        end_turn(&mut actor, &host).await;

        // Then the thread shows the branch named after the orb name.
        assert_eq!(
            shown(&state, id)
                .and_then(|thread| thread.branch)
                .as_deref(),
            Some("orb/sidebar-search"),
            "the hex branch should be renamed to the orb name's slug"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn no_rename_when_the_branch_is_not_the_hex_branch() -> Result<(), Report<StoreError>> {
        // Given a Claude-titled thread in an orb worktree on another branch.
        let (claude_dir, store, _) =
            worktree_thread(&format!("{PROMPT_LINE}{AI_TITLE_LINE}"), "main")?;
        let host = listing(Vec::new());
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
        let host = listing(Vec::new());
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
        let host = listing(Vec::new());
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
        let host = listing(vec![in_session(ThreadStatus::Working)]);
        let git = FakeGit::local();
        let (mut actor, _state) = start_with(store, &host, &git, claude_dir.path());

        // When polls see its turn still underway.
        actor.poll().await;
        actor.poll().await;

        // Then no branch was renamed.
        assert_eq!(git.renamed(), None, "the rename waits for the turn to end");
        Ok(())
    }

    fn notices_of(state: &State) -> Vec<Notice> {
        state.read().sessions.notices.clone()
    }

    #[rstest::rstest]
    #[case(ThreadStatus::Unknown, ThreadStatus::Idle, None)]
    #[case(ThreadStatus::Unknown, ThreadStatus::NeedsApproval, None)]
    #[case(ThreadStatus::Working, ThreadStatus::Idle, Some(NoticeKind::Finished))]
    #[case(
        ThreadStatus::NeedsApproval,
        ThreadStatus::Idle,
        Some(NoticeKind::Finished)
    )]
    #[case(
        ThreadStatus::Working,
        ThreadStatus::NeedsApproval,
        Some(NoticeKind::NeedsApproval)
    )]
    #[case(
        ThreadStatus::Working,
        ThreadStatus::NeedsInput,
        Some(NoticeKind::NeedsInput)
    )]
    #[case(ThreadStatus::NeedsApproval, ThreadStatus::NeedsApproval, None)]
    #[case(
        ThreadStatus::NeedsApproval,
        ThreadStatus::NeedsInput,
        Some(NoticeKind::NeedsInput)
    )]
    #[case(ThreadStatus::Idle, ThreadStatus::Idle, None)]
    #[case(ThreadStatus::Working, ThreadStatus::Failed, None)]
    #[case(ThreadStatus::Working, ThreadStatus::Stopped, None)]
    fn notice_kind_names_why_a_status_change_needs_the_user(
        #[case] old: ThreadStatus,
        #[case] new: ThreadStatus,
        #[case] expected: Option<NoticeKind>,
    ) {
        assert_eq!(notice_kind(old, new), expected, "{old:?} → {new:?}");
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn first_poll_after_restore_queues_no_notice() -> Result<(), Report<StoreError>> {
        // Given a saved thread that was already waiting for an approval at launch.
        let (store, _) = store_with_thread("aa")?;
        let host = listing(vec![record("aa", ThreadStatus::NeedsApproval)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When polling for the first time.
        actor.poll().await;

        // Then no notice is queued.
        assert_eq!(
            notices_of(&state),
            [],
            "a state that existed at launch shouldn't notify"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn turn_end_queues_a_finished_notice_naming_project_and_thread()
    -> Result<(), Report<StoreError>> {
        // Given orb's thread titled "Parser fix".
        let (store, id) = store_with_thread("aa")?;
        resave(&store, "aa", |row| ThreadRow {
            custom_title: Some("Parser fix".to_owned()),
            ..row
        })?;
        let host = listing(Vec::new());
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When polls see its turn run and then end.
        end_turn(&mut actor, &host).await;

        // Then one Finished notice names the project and the thread.
        assert_eq!(
            notices_of(&state),
            [Notice {
                thread: id,
                kind: NoticeKind::Finished,
                project: "orb".to_owned(),
                title: "Parser fix".to_owned(),
            }],
            "the ended turn's notice"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn a_wait_seen_by_two_polls_queues_one_notice() -> Result<(), Report<StoreError>> {
        // Given a thread whose turn is running.
        let (store, _) = store_with_thread("aa")?;
        let host = listing(vec![in_session(ThreadStatus::Working)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When two polls in a row see it waiting for an approval.
        host.set_running(Ok(vec![in_session(ThreadStatus::NeedsApproval)]));
        actor.poll().await;
        actor.poll().await;

        // Then one NeedsApproval notice is queued.
        let kinds: Vec<NoticeKind> = notices_of(&state)
            .into_iter()
            .map(|notice| notice.kind)
            .collect();
        assert_eq!(
            kinds,
            [NoticeKind::NeedsApproval],
            "the same wait should notify once"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn turn_end_of_a_thread_being_deleted_queues_no_notice() -> Result<(), Report<StoreError>>
    {
        // Given a thread whose turn is running, then marked for deletion.
        let (store, id) = store_with_thread("aa")?;
        let host = listing(vec![in_session(ThreadStatus::Working)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;
        state.write().sessions.deleting.insert(SessionId(id.0));

        // When a poll sees its turn end.
        host.set_running(Ok(vec![in_session(ThreadStatus::Idle)]));
        actor.poll().await;

        // Then no notice is queued.
        assert_eq!(
            notices_of(&state),
            [],
            "a thread being deleted shouldn't notify"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn turn_end_outside_the_project_filter_still_queues_a_notice()
    -> Result<(), Report<StoreError>> {
        // Given orb's thread, with the sidebar filtered to another project.
        let (store, id) = store_with_thread("aa")?;
        let web = store.add_project(Path::new(WEB_ROOT), "web", ProjectKind::Normal, 0)?;
        let host = listing(Vec::new());
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        state.write().sessions.filter = Some(web);

        // When polls see its turn run and then end.
        end_turn(&mut actor, &host).await;

        // Then its notice is queued anyway.
        let notified: Vec<ThreadId> = notices_of(&state)
            .into_iter()
            .map(|notice| notice.thread)
            .collect();
        assert_eq!(notified, [id], "the filter shouldn't hide notices");
        Ok(())
    }

    /// The orb project, with the actor started on `git` and a temp folder as
    /// orb's own directory.
    fn creating(
        git: &Arc<FakeGit>,
    ) -> Result<(tempfile::TempDir, ProjectId, SessionsActor, State), Report<StoreError>> {
        let orb_root = tempfile::tempdir().change_context(StoreError)?;
        let store = Store::open_in_memory()?;
        let orb = orb_project(&store)?;
        let (actor, state) = start_in(
            store,
            &listing(Vec::new()),
            git,
            Path::new(NO_CLAUDE_DIR),
            orb_root.path(),
            Path::new(INCOGNITO_ROOT),
        );
        Ok((orb_root, orb, actor, state))
    }

    #[rstest::rstest]
    fn new_research_session_seeds_a_missing_template() -> Result<(), Report<StoreError>> {
        // Given no Research template.
        let (orb_root, _, mut actor, _state) = creating(&FakeGit::local())?;

        // When making Research session `tokio-cancel`.
        actor.new_folder_session(FolderKind::Research, "tokio-cancel");

        // Then the template is written.
        assert!(
            orb_root
                .path()
                .join("templates/research/AGENTS.md")
                .is_file(),
            "a missing template should be seeded"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_research_session_copies_the_template_into_its_folder() -> Result<(), Report<StoreError>>
    {
        // Given no Research template.
        let (orb_root, _, mut actor, _state) = creating(&FakeGit::local())?;

        // When making Research session `tokio-cancel`.
        actor.new_folder_session(FolderKind::Research, "tokio-cancel");

        // Then its folder has AGENTS.md and a CLAUDE.md link to it.
        let dir = orb_root.path().join("research/tokio-cancel");
        let link = fs::read_link(dir.join("CLAUDE.md")).change_context(StoreError)?;
        assert_eq!(
            (dir.join("AGENTS.md").is_file(), link),
            (true, PathBuf::from("AGENTS.md")),
            "the folder should be the template's copy"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_research_session_copies_the_kit() -> Result<(), Report<StoreError>> {
        // Given no Research template.
        let (orb_root, _, mut actor, _state) = creating(&FakeGit::local())?;

        // When making Research session `tokio-cancel`.
        actor.new_folder_session(FolderKind::Research, "tokio-cancel");

        // Then its folder has the kit's investigator subagent.
        assert!(
            orb_root
                .path()
                .join("research/tokio-cancel/.claude/agents/investigator.md")
                .is_file(),
            "the folder should get the research kit"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_research_session_adds_the_research_project() -> Result<(), Report<StoreError>> {
        // Given no Research project.
        let (orb_root, _, mut actor, state) = creating(&FakeGit::local())?;

        // When making Research session `tokio-cancel`.
        actor.new_folder_session(FolderKind::Research, "tokio-cancel");

        // Then orb's Research project shows, rooted in orb's research folder.
        let research: Vec<(String, PathBuf)> = state
            .read()
            .sessions
            .projects
            .iter()
            .filter(|project| project.kind == ProjectKind::Research)
            .map(|project| (project.title.clone(), project.root.clone()))
            .collect();
        assert_eq!(
            research,
            vec![("Research".to_owned(), orb_root.path().join("research"))],
            "the Research project should be added"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_learn_session_uses_the_learn_folder() -> Result<(), Report<StoreError>> {
        // Given no Learn project.
        let (orb_root, _, mut actor, _state) = creating(&FakeGit::local())?;

        // When making Learn session `rust-async`.
        actor.new_folder_session(FolderKind::Learn, "rust-async");

        // Then its folder is under orb's learn folder.
        assert!(
            orb_root.path().join("learn/rust-async").is_dir(),
            "a Learn session's folder should be under learn/"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_research_session_is_a_research_session() -> Result<(), Report<StoreError>> {
        // Given no Research project.
        let (orb_root, _, mut actor, state) = creating(&FakeGit::local())?;

        // When making Research session `tokio-cancel`.
        actor.new_folder_session(FolderKind::Research, "tokio-cancel");

        // Then a Research session runs in its folder.
        let session = only_session(&state)?;
        assert_eq!(
            (session.kind, session.dir),
            (
                SessionKind::Research,
                orb_root.path().join("research/tokio-cancel")
            ),
            "the session should be a Research one in its folder"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_learn_session_has_one_tab_with_one_shell_pane() -> Result<(), Report<StoreError>> {
        // Given no Learn project.
        let (orb_root, _, mut actor, state) = creating(&FakeGit::local())?;

        // When making Learn session `rust-async`.
        actor.new_folder_session(FolderKind::Learn, "rust-async");

        // Then its layout holds one tab of one pane in its folder.
        let id = only_session(&state)?.id;
        let app = state.read();
        let shape = app.layouts.get(id).map(|layout| {
            (
                layout.tabs().len(),
                layout
                    .panes()
                    .map(|pane| pane.cwd.clone())
                    .collect::<Vec<_>>(),
            )
        });
        assert_eq!(
            shape,
            Some((1, vec![orb_root.path().join("learn/rust-async")])),
            "a Learn session holds one tab of one shell pane"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_folder_session_keeps_a_curated_template() -> Result<(), Report<StoreError>> {
        // Given a Research template the user wrote.
        let (orb_root, _, mut actor, _state) = creating(&FakeGit::local())?;
        let template = orb_root.path().join("templates/research");
        fs::create_dir_all(&template).change_context(StoreError)?;
        fs::write(template.join("AGENTS.md"), "mine").change_context(StoreError)?;

        // When making Research session `tokio-cancel`.
        actor.new_folder_session(FolderKind::Research, "tokio-cancel");

        // Then the folder gets the user's AGENTS.md.
        let text = fs::read_to_string(orb_root.path().join("research/tokio-cancel/AGENTS.md"))
            .change_context(StoreError)?;
        assert_eq!(text, "mine", "a curated template should be copied as is");
        Ok(())
    }

    #[rstest::rstest]
    fn new_folder_session_refused_when_its_folder_exists() -> Result<(), Report<StoreError>> {
        // Given a folder `x` already under orb's research folder.
        let (orb_root, _, mut actor, state) = creating(&FakeGit::local())?;
        fs::create_dir_all(orb_root.path().join("research/x")).change_context(StoreError)?;

        // When making Research session `x`.
        actor.new_folder_session(FolderKind::Research, "x");

        // Then the mode line says the folder exists.
        assert_eq!(
            error_of(&state).as_deref(),
            Some("~/.orb/research/x already exists"),
            "an existing folder should be refused"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn refused_folder_session_adds_no_session() -> Result<(), Report<StoreError>> {
        // Given a folder `x` already under orb's research folder.
        let (orb_root, _, mut actor, state) = creating(&FakeGit::local())?;
        fs::create_dir_all(orb_root.path().join("research/x")).change_context(StoreError)?;

        // When making Research session `x`.
        actor.new_folder_session(FolderKind::Research, "x");

        // Then no session shows.
        assert!(
            state.read().sessions.sessions.is_empty(),
            "a refused session should leave nothing"
        );
        Ok(())
    }

    /// Opens the name box for a new `kind` session holding `text`, as `⏎`
    /// leaves it: waiting for the actor, with the keys.
    fn waiting_name_box(state: &State, kind: FolderKind, text: &str) {
        let mut app = state.write();
        app.rename = Some(Rename {
            target: RenameTarget::NewFolder(kind),
            input: TextInput::new(text),
            creating: true,
        });
        app.focus = Focus::Rename;
    }

    /// The name box's text and whether it waits, and the focus.
    fn name_box_of(state: &State) -> (Option<(String, bool)>, Focus) {
        let app = state.read();
        let shown = app
            .rename
            .as_ref()
            .map(|rename| (rename.input.text().to_owned(), rename.creating));
        (shown, app.focus)
    }

    #[rstest::rstest]
    fn made_folder_session_closes_the_name_box() -> Result<(), Report<StoreError>> {
        // Given the name box waiting for Research session `tokio-cancel`.
        let (_orb_root, _, mut actor, state) = creating(&FakeGit::local())?;
        waiting_name_box(&state, FolderKind::Research, "tokio cancel");

        // When making it.
        actor.new_folder_session(FolderKind::Research, "tokio-cancel");

        // Then the box is closed and the sidebar has the keys.
        assert_eq!(
            name_box_of(&state),
            (None, Focus::Sidebar),
            "a made session should close its name box"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn refused_folder_name_keeps_the_name_box_open() -> Result<(), Report<StoreError>> {
        // Given the name box waiting for Research session `x`, whose folder
        // exists.
        let (orb_root, _, mut actor, state) = creating(&FakeGit::local())?;
        fs::create_dir_all(orb_root.path().join("research/x")).change_context(StoreError)?;
        waiting_name_box(&state, FolderKind::Research, "x");

        // When making it.
        actor.new_folder_session(FolderKind::Research, "x");

        // Then the box stays open with the name, no longer waiting.
        assert_eq!(
            name_box_of(&state),
            (Some(("x".to_owned(), false)), Focus::Rename),
            "an existing folder should keep the name box open"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_folder_session_outside_the_filter_clears_it() -> Result<(), Report<StoreError>> {
        // Given the sidebar filtered to orb.
        let (_orb_root, orb, mut actor, state) = creating(&FakeGit::local())?;
        state.write().sessions.filter = Some(orb);

        // When making Research session `tokio-cancel`, outside orb.
        actor.new_folder_session(FolderKind::Research, "tokio-cancel");

        // Then the filter goes back to all projects.
        assert_eq!(
            state.read().sessions.filter,
            None,
            "a session outside the filter should clear it"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_folder_session_outside_the_filter_saves_the_cleared_filter()
    -> Result<(), Report<StoreError>> {
        // Given the sidebar filtered to orb, and that saved.
        let (_orb_root, orb, mut actor, state) = creating(&FakeGit::local())?;
        state.write().sessions.filter = Some(orb);
        actor.save_ui();

        // When making Research session `tokio-cancel`, outside orb.
        actor.new_folder_session(FolderKind::Research, "tokio-cancel");

        // Then the store holds no filter.
        assert_eq!(
            actor.store.ui()?.project_filter,
            None,
            "the cleared filter should be saved"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn refused_folder_session_keeps_the_filter() -> Result<(), Report<StoreError>> {
        // Given the sidebar filtered to orb, and Research folder `x` on disk.
        let (orb_root, orb, mut actor, state) = creating(&FakeGit::local())?;
        fs::create_dir_all(orb_root.path().join("research/x")).change_context(StoreError)?;
        state.write().sessions.filter = Some(orb);

        // When making Research session `x`.
        actor.new_folder_session(FolderKind::Research, "x");

        // Then the filter stays on orb.
        assert_eq!(
            state.read().sessions.filter,
            Some(orb),
            "a refused session should leave the filter alone"
        );
        Ok(())
    }

    /// Saves thread `short_id` in the orb project, created an hour ago, in a
    /// session of its own in `cwd`, on `branch`.
    fn insert_in(
        store: &Store,
        short_id: &str,
        cwd: &Path,
        branch: Option<&str>,
    ) -> Result<Seeded, Report<StoreError>> {
        let project_id = orb_project(store)?;
        let inserted = seed_with(
            store,
            "claude",
            project_id,
            SessionKind::Plain,
            cwd,
            short_id,
            now_ms() - HOUR_MS,
        )?;
        resave(store, short_id, |row| ThreadRow {
            branch: branch.map(str::to_owned),
            ..row
        })?;
        Ok(inserted)
    }

    /// A store whose thread `aa` has a session in [`HEX_WORKTREE`], now
    /// gone, on `branch`.
    fn pruned_thread(branch: Option<&str>) -> Result<(Store, SessionId), Report<StoreError>> {
        let store = Store::open_in_memory()?;
        let inserted = insert_in(&store, "aa", Path::new(HEX_WORKTREE), branch)?;
        Ok((store, inserted.session))
    }

    /// The git calls an actor on `store` and `git` makes restoring `id`.
    fn restore_calls(store: Store, git: &Arc<FakeGit>, id: SessionId) -> Vec<GitCall> {
        let (mut actor, _state) =
            start_with(store, &listing(vec![]), git, Path::new(NO_CLAUDE_DIR));
        actor.restore_worktree(id);
        git.calls()
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn restore_on_an_existing_branch_adds_the_worktree_on_it()
    -> Result<(), Report<StoreError>> {
        // Given thread aa's pruned worktree, whose branch still exists.
        let (store, id) = pruned_thread(Some("fix-parser"))?;
        let git = FakeGit::having("fix-parser");

        // When restoring its worktree.
        let calls = restore_calls(store, &git, id);

        // Then the worktree is added back on that branch.
        assert!(
            calls.contains(&GitCall::AddWorktreeOn {
                path: PathBuf::from(HEX_WORKTREE),
                branch: "fix-parser".to_owned(),
            }),
            "an existing branch should be checked out in the restored worktree"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn restore_on_a_missing_branch_creates_it_from_the_default_branch()
    -> Result<(), Report<StoreError>> {
        // Given thread aa's pruned worktree, whose branch is gone.
        let (store, id) = pruned_thread(Some("fix-parser"))?;
        let git = FakeGit::local();

        // When restoring its worktree.
        let calls = restore_calls(store, &git, id);

        // Then the branch is made again from the default branch.
        assert!(
            calls.contains(&GitCall::AddWorktree {
                path: PathBuf::from(HEX_WORKTREE),
                branch: "fix-parser".to_owned(),
                base: "main".to_owned(),
            }),
            "a gone branch should be made again from main"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn restore_on_a_missing_branch_starts_from_origin() -> Result<(), Report<StoreError>> {
        // Given thread aa's pruned worktree, whose branch is gone, in a
        // project with an origin.
        let (store, id) = pruned_thread(Some("fix-parser"))?;
        let git = FakeGit::with_origin(Ok(true));

        // When restoring its worktree.
        let calls = restore_calls(store, &git, id);

        // Then the branch is made again from origin's default branch.
        assert!(
            calls.contains(&GitCall::AddWorktree {
                path: PathBuf::from(HEX_WORKTREE),
                branch: "fix-parser".to_owned(),
                base: "origin/main".to_owned(),
            }),
            "a gone branch should start from the fetched origin/main"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn restore_without_a_recorded_branch_uses_the_hex_branch()
    -> Result<(), Report<StoreError>> {
        // Given thread aa's pruned worktree, with no branch recorded.
        let (store, id) = pruned_thread(None)?;
        let git = FakeGit::local();

        // When restoring its worktree.
        let calls = restore_calls(store, &git, id);

        // Then the worktree comes back on orb's branch for its directory.
        assert!(
            calls.contains(&GitCall::AddWorktree {
                path: PathBuf::from(HEX_WORKTREE),
                branch: HEX_BRANCH.to_owned(),
                base: "main".to_owned(),
            }),
            "a thread with no branch should get orb/<hex> back"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn restore_prunes_stale_worktree_metadata_first() -> Result<(), Report<StoreError>> {
        // Given thread aa's pruned worktree.
        let (store, id) = pruned_thread(Some("fix-parser"))?;
        let git = FakeGit::local();

        // When restoring its worktree.
        let calls = restore_calls(store, &git, id);

        // Then git's record of the old worktree is pruned before anything
        // else.
        assert_eq!(
            calls.first(),
            Some(&GitCall::PruneWorktrees(PathBuf::from(PROJECT_ROOT))),
            "stale worktree metadata should be pruned first"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn restore_success_sets_attach() -> Result<(), Report<StoreError>> {
        // Given thread aa's pruned worktree.
        let (store, id) = pruned_thread(Some("fix-parser"))?;
        let (mut actor, state) = start_with(
            store,
            &listing(vec![]),
            &FakeGit::local(),
            Path::new(NO_CLAUDE_DIR),
        );

        // When restoring its worktree.
        actor.restore_worktree(id);

        // Then the frontend is asked to attach to the session.
        assert_eq!(
            state.read().sessions.attach,
            Some(id),
            "a restored session should be attached"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn restore_failure_sets_the_error_and_no_attach() -> Result<(), Report<StoreError>> {
        // Given thread aa's pruned worktree, in a project git can't read.
        let (store, id) = pruned_thread(Some("fix-parser"))?;
        let (mut actor, state) = start_with(
            store,
            &listing(vec![]),
            &FakeGit::plain(Ok(())),
            Path::new(NO_CLAUDE_DIR),
        );

        // When restoring its worktree.
        actor.restore_worktree(id);

        // Then git's reason shows and nothing attaches.
        let sessions = &state.read().sessions;
        assert_eq!(
            (sessions.error.as_deref(), sessions.attach),
            (Some(NOT_A_REPO), None),
            "a failed restore should show why and not attach"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn restore_ends_the_start() -> Result<(), Report<StoreError>> {
        // Given thread aa's pruned worktree, its restore marked as starting.
        let (store, id) = pruned_thread(Some("fix-parser"))?;
        let (mut actor, state) = start_with(
            store,
            &listing(vec![]),
            &FakeGit::local(),
            Path::new(NO_CLAUDE_DIR),
        );
        state.write().sessions.starting = true;

        // When restoring its worktree.
        actor.restore_worktree(id);

        // Then nothing is starting any more.
        assert!(
            !state.read().sessions.starting,
            "a finished restore should stop showing as starting"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn restore_of_a_directory_that_exists_runs_no_git() -> Result<(), Report<StoreError>> {
        // Given thread aa in a directory that is on disk.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let store = Store::open_in_memory()?;
        let id = insert_in(&store, "aa", dir.path(), Some("fix-parser"))?.session;
        let git = FakeGit::local();

        // When restoring its worktree.
        let calls = restore_calls(store, &git, id);

        // Then git is never asked to do anything.
        assert!(
            calls.is_empty(),
            "a worktree already back should need no git"
        );
        Ok(())
    }

    /// Claude thread `aa` in a pane of its own and a shell pane beside it in
    /// the same session, the actor reading pane files under a scratch orb
    /// folder.
    struct PaneReports {
        dir: tempfile::TempDir,
        thread: Seeded,
        shell: PaneId,
        actor: SessionsActor,
        state: State,
    }

    impl PaneReports {
        fn new() -> Result<Self, Report<StoreError>> {
            Self::with(|_, _| Ok(()))
        }

        /// The fixture, with `setup` run on its store and `aa`'s pane first.
        fn with<F>(setup: F) -> Result<Self, Report<StoreError>>
        where
            F: FnOnce(&Store, &Seeded) -> Result<(), Report<StoreError>>,
        {
            let dir = tempfile::tempdir().change_context(StoreError)?;
            let store = Store::open_in_memory()?;
            let thread = insert_thread(&store, "aa")?;
            let shell = store.insert_pane(thread.session, Path::new(PROJECT_ROOT))?;
            setup(&store, &thread)?;
            let host = listing(Vec::new());
            let (actor, state) = start_in(
                store,
                &host,
                &FakeGit::local(),
                Path::new(NO_CLAUDE_DIR),
                dir.path(),
                Path::new(INCOGNITO_ROOT),
            );
            Ok(Self {
                dir,
                thread,
                shell,
                actor,
                state,
            })
        }

        /// Pane `pane`'s file.
        fn file(&self, pane: PaneId) -> PathBuf {
            self.dir
                .path()
                .join("panes")
                .join(format!("{}.json", pane.0))
        }

        /// Writes Claude's `event` report of conversation `session_id` from
        /// `pane`, started for `source`, its transcript `/t/<id>.jsonl`.
        fn report(
            &self,
            pane: PaneId,
            event: &str,
            session_id: &str,
            source: Option<&str>,
        ) -> Result<(), Report<StoreError>> {
            fs::create_dir_all(self.dir.path().join("panes")).change_context(StoreError)?;
            let json = serde_json::json!({
                "agent": "claude",
                "event": event,
                "session_id": session_id,
                "transcript": format!("/t/{session_id}.jsonl"),
                "source": source,
                "at": now_ms(),
            });
            fs::write(self.file(pane), json.to_string()).change_context(StoreError)
        }

        /// The saved threads.
        fn rows(&self) -> Result<Vec<ThreadRow>, Report<StoreError>> {
            Ok(self.actor.store.load()?.1)
        }

        /// The saved thread `id`.
        fn row(&self, id: ThreadId) -> Result<ThreadRow, Report<StoreError>> {
            self.rows()?
                .into_iter()
                .find(|row| row.id == id)
                .ok_or_else(|| Report::new(StoreError).attach("the thread isn't saved"))
        }

        /// Pane `pane`'s saved resume command.
        fn resume(&self, pane: PaneId) -> Result<Option<String>, Report<StoreError>> {
            Ok(self
                .actor
                .store
                .layouts()?
                .panes
                .into_iter()
                .find(|row| row.id == pane)
                .and_then(|row| row.resume))
        }
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn pane_report_start_creates_a_thread_in_that_pane() -> Result<(), Report<StoreError>> {
        // Given Claude started conversation s-new in the shell pane.
        let mut fx = PaneReports::new()?;
        fx.report(fx.shell, "start", "s-new", Some("startup"))?;

        // When polling.
        fx.actor.poll().await;

        // Then a Claude thread for s-new runs in that pane, with its transcript.
        let found: Vec<_> = fx
            .rows()?
            .into_iter()
            .filter(|row| row.short_id == "s-new")
            .map(|row| {
                (
                    row.pane_id,
                    row.session_id,
                    row.harness,
                    row.transcript_path,
                )
            })
            .collect();
        let expected = vec![(
            Some(fx.shell),
            Some("s-new".to_owned()),
            HarnessId::new("claude"),
            Some(PathBuf::from("/t/s-new.jsonl")),
        )];
        assert_eq!(
            found, expected,
            "the report should start a thread in the pane"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn pane_report_start_shows_the_new_thread() -> Result<(), Report<StoreError>> {
        // Given Claude started conversation s-new in the shell pane.
        let mut fx = PaneReports::new()?;
        fx.report(fx.shell, "start", "s-new", Some("startup"))?;

        // When polling.
        fx.actor.poll().await;

        // Then the new thread is shown.
        let id = fx
            .rows()?
            .into_iter()
            .find(|row| row.short_id == "s-new")
            .map(|row| row.id)
            .ok_or_else(|| Report::new(StoreError).attach("no thread for s-new"))?;
        assert!(
            shown(&fx.state, id).is_some(),
            "the new thread should be shown in its project"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn pane_report_resume_binds_the_thread_holding_that_session()
    -> Result<(), Report<StoreError>> {
        // Given Claude resumed thread aa's conversation in the shell pane.
        let mut fx = PaneReports::new()?;
        fx.report(fx.shell, "start", "aa", Some("resume"))?;

        // When polling.
        fx.actor.poll().await;

        // Then aa, still the only thread, runs in the shell pane.
        let found: Vec<_> = fx
            .rows()?
            .into_iter()
            .map(|row| (row.id, row.pane_id))
            .collect();
        assert_eq!(
            found,
            vec![(fx.thread.thread, Some(fx.shell))],
            "the thread holding the session should move into the pane"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn pane_report_resume_takes_the_pane_from_its_previous_thread()
    -> Result<(), Report<StoreError>> {
        // Given thread s-old runs in the shell pane, and Claude then resumed
        // aa's conversation there.
        let mut old = None;
        let mut fx = PaneReports::with(|store, thread| {
            let shell = store
                .layouts()?
                .panes
                .into_iter()
                .map(|row| row.id)
                .find(|&pane| pane != thread.pane)
                .ok_or_else(|| Report::new(StoreError).attach("no shell pane"))?;
            old = Some(
                store
                    .insert_pane_thread(&NewPaneThread {
                        pane: shell,
                        session_id: "s-old".to_owned(),
                        transcript_path: None,
                        harness: HarnessId::new("claude"),
                        cwd: PathBuf::from(PROJECT_ROOT),
                        created_at: now_ms(),
                    })?
                    .0,
            );
            Ok(())
        })?;
        let old = old.ok_or_else(|| Report::new(StoreError))?;
        fx.report(fx.shell, "start", "aa", Some("resume"))?;

        // When polling.
        fx.actor.poll().await;

        // Then s-old's thread no longer has a pane.
        assert_eq!(
            fx.row(old)?.pane_id,
            None,
            "the pane's earlier thread should leave it"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case::claude_clear("clear")]
    #[case::pi_new("new")]
    #[tokio::test]
    async fn pane_report_after_clear_keeps_the_same_thread(
        #[case] source: &str,
    ) -> Result<(), Report<StoreError>> {
        // Given aa's pane started a fresh conversation s-2 in its place.
        let mut fx = PaneReports::new()?;
        fx.report(fx.thread.pane, "start", "s-2", Some(source))?;

        // When polling.
        fx.actor.poll().await;

        // Then thread aa holds s-2.
        assert_eq!(
            fx.row(fx.thread.thread)?.session_id.as_deref(),
            Some("s-2"),
            "the same thread should take the new conversation"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn pane_report_after_clear_swaps_the_transcript() -> Result<(), Report<StoreError>> {
        // Given aa's pane ran /clear into conversation s-2.
        let mut fx = PaneReports::new()?;
        fx.report(fx.thread.pane, "start", "s-2", Some("clear"))?;

        // When polling.
        fx.actor.poll().await;

        // Then thread aa reads s-2's transcript.
        assert_eq!(
            fx.row(fx.thread.thread)?.transcript_path,
            Some(PathBuf::from("/t/s-2.jsonl")),
            "the thread should read the new conversation's transcript"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn pane_report_end_unbinds_the_thread() -> Result<(), Report<StoreError>> {
        // Given aa's conversation ended in its pane.
        let mut fx = PaneReports::new()?;
        fx.report(fx.thread.pane, "end", "aa", None)?;

        // When polling.
        fx.actor.poll().await;

        // Then aa no longer has a pane.
        assert_eq!(
            fx.row(fx.thread.thread)?.pane_id,
            None,
            "an ended thread should leave its pane"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn end_report_leaves_the_session_listed() -> Result<(), Report<StoreError>> {
        // Given aa's conversation ended in its session's only agent pane.
        let mut fx = PaneReports::new()?;
        fx.report(fx.thread.pane, "end", "aa", None)?;

        // When polling.
        fx.actor.poll().await;

        // Then the session is still in the sidebar.
        let listed = fx
            .state
            .read()
            .sessions
            .sidebar()
            .iter()
            .any(|row| row.item() == SidebarItem::Session(fx.thread.session));
        assert!(listed, "a session whose agents all ended keeps its card");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn settle_session_keeps_each_panes_resume_command() -> Result<(), Report<StoreError>> {
        // Given aa running in its pane, then its session settled, which ends
        // aa's conversation in the pane.
        let mut fx = PaneReports::new()?;
        fx.report(fx.thread.pane, "start", "aa", Some("resume"))?;
        fx.actor.poll().await;
        fx.actor.settle_session(fx.thread.session);
        fx.report(fx.thread.pane, "end", "aa", None)?;

        // When polling.
        fx.actor.poll().await;

        // Then the pane still resumes aa.
        assert_eq!(
            fx.resume(fx.thread.pane)?.as_deref(),
            Some("claude --resume aa"),
            "a settled pane should keep its resume command"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn thread_resumed_in_another_session_takes_that_session() -> Result<(), Report<StoreError>>
    {
        // Given thread bb's session, and aa's conversation resumed in bb's
        // pane.
        let mut other = None;
        let mut fx = PaneReports::with(|store, _| {
            other = Some(insert_thread(store, "bb")?);
            Ok(())
        })?;
        let other = other.ok_or_else(|| Report::new(StoreError).attach("bb wasn't made"))?;
        fx.report(other.pane, "start", "aa", Some("resume"))?;

        // When polling.
        fx.actor.poll().await;

        // Then aa belongs to bb's session.
        assert_eq!(
            fx.row(fx.thread.thread)?.orb_session,
            Some(other.session),
            "a thread should take the session of the pane it is bound to"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn end_report_keeps_the_threads_session() -> Result<(), Report<StoreError>> {
        // Given aa's conversation ended in its pane.
        let mut fx = PaneReports::new()?;
        fx.report(fx.thread.pane, "end", "aa", None)?;

        // When polling.
        fx.actor.poll().await;

        // Then aa still shows in its session.
        assert_eq!(
            shown(&fx.state, fx.thread.thread).and_then(|thread| thread.home()),
            Some(fx.thread.session),
            "an ended thread should keep the session it ran in"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn end_report_in_a_settled_session_keeps_the_thread_bound()
    -> Result<(), Report<StoreError>> {
        // Given aa's session settled, then aa's conversation ending in its
        // pane.
        let mut fx = PaneReports::new()?;
        fx.actor.edit_session(fx.thread.session, settle_session_row);
        fx.report(fx.thread.pane, "end", "aa", None)?;

        // When polling.
        fx.actor.poll().await;

        // Then aa keeps its pane.
        assert_eq!(
            fx.row(fx.thread.thread)?.pane_id,
            Some(fx.thread.pane),
            "settling ended it, so the thread waits to come back"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn pane_report_end_of_another_session_changes_nothing() -> Result<(), Report<StoreError>>
    {
        // Given aa's pane reports the end of some other conversation.
        let mut fx = PaneReports::new()?;
        fx.report(fx.thread.pane, "end", "zz", None)?;

        // When polling.
        fx.actor.poll().await;

        // Then aa keeps its pane.
        assert_eq!(
            fx.row(fx.thread.thread)?.pane_id,
            Some(fx.thread.pane),
            "a stale end should change nothing"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn pane_report_start_sets_the_panes_resume_command() -> Result<(), Report<StoreError>> {
        // Given Claude started conversation s-new in the shell pane.
        let mut fx = PaneReports::new()?;
        fx.report(fx.shell, "start", "s-new", Some("startup"))?;

        // When polling.
        fx.actor.poll().await;

        // Then the pane resumes s-new.
        assert_eq!(
            fx.resume(fx.shell)?.as_deref(),
            Some("claude --resume s-new"),
            "the pane should resume its conversation"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn pane_report_end_clears_the_panes_resume_command() -> Result<(), Report<StoreError>> {
        // Given aa's pane resumes aa, and aa's conversation ended.
        let mut fx = PaneReports::with(|store, thread| {
            store.set_pane_resume(thread.pane, Some("claude --resume aa"))
        })?;
        fx.report(fx.thread.pane, "end", "aa", None)?;

        // When polling.
        fx.actor.poll().await;

        // Then the pane has nothing to resume.
        assert_eq!(
            fx.resume(fx.thread.pane)?,
            None,
            "an ended conversation shouldn't be resumed"
        );
        Ok(())
    }

    /// Pane `pane`'s resume command, as its layout entry shows it.
    fn layout_resume(state: &State, pane: PaneId) -> Option<String> {
        state
            .read()
            .layouts
            .entry(pane)
            .and_then(|entry| entry.resume.clone())
    }

    #[rstest::rstest]
    fn restored_pane_entry_carries_its_resume_command() -> Result<(), Report<StoreError>> {
        // Given a saved thread whose pane resumes aa.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa")?;
        store.set_pane_resume(inserted.pane, Some("claude --resume aa"))?;
        let host = listing(Vec::new());

        // When restoring.
        let (_actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // Then the pane's layout entry carries the resume command.
        assert_eq!(
            layout_resume(&state, inserted.pane).as_deref(),
            Some("claude --resume aa"),
            "the restored entry should know how to resume its pane"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn pane_report_start_shows_the_resume_command_in_the_layout()
    -> Result<(), Report<StoreError>> {
        // Given the shell pane open in a tab of its own, and Claude started
        // conversation s-new in it.
        let mut fx = PaneReports::new()?;
        fx.state
            .write()
            .layouts
            .new_tab(fx.thread.session, test_entry(fx.shell.0));
        fx.report(fx.shell, "start", "s-new", Some("startup"))?;

        // When polling.
        fx.actor.poll().await;

        // Then the shell pane's layout entry resumes s-new.
        assert_eq!(
            layout_resume(&fx.state, fx.shell).as_deref(),
            Some("claude --resume s-new"),
            "the layout should carry the pane's new resume command"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn pane_report_end_clears_the_resume_command_in_the_layout()
    -> Result<(), Report<StoreError>> {
        // Given aa's pane resumes aa, and aa's conversation ended.
        let mut fx = PaneReports::with(|store, thread| {
            store.set_pane_resume(thread.pane, Some("claude --resume aa"))
        })?;
        fx.report(fx.thread.pane, "end", "aa", None)?;

        // When polling.
        fx.actor.poll().await;

        // Then the pane's layout entry has nothing to resume.
        assert_eq!(
            layout_resume(&fx.state, fx.thread.pane),
            None,
            "an ended conversation shouldn't be resumed from the layout"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn pane_report_from_an_unknown_pane_is_ignored() -> Result<(), Report<StoreError>> {
        // Given a report from a pane orb has no row for.
        let mut fx = PaneReports::new()?;
        fx.report(PaneId(999), "start", "s-new", Some("startup"))?;

        // When polling.
        fx.actor.poll().await;

        // Then no thread is added.
        assert_eq!(
            fx.rows()?.len(),
            1,
            "an unknown pane's report should be ignored"
        );
        Ok(())
    }

    /// Thread `aa` in a pane of its own, its harness `claude` (status from
    /// the agents it sees running) or `pi` (status from pane reports),
    /// polled through a harness seeing `records` and a zmx whose listing `zmx`
    /// makes from the pane, the actor reading pane files under a scratch orb
    /// folder.
    struct PaneStatus {
        dir: tempfile::TempDir,
        thread: Seeded,
        host: Arc<FakeHarness>,
        actor: SessionsActor,
        state: State,
    }

    impl PaneStatus {
        fn new<F>(
            reports: bool,
            records: Vec<RunningAgent>,
            zmx: F,
        ) -> Result<Self, Report<StoreError>>
        where
            F: FnOnce(PaneId) -> ZmxService,
        {
            let dir = tempfile::tempdir().change_context(StoreError)?;
            let store = Store::open_in_memory()?;
            let agent = if reports { "pi" } else { "claude" };
            let thread = seed_with(
                &store,
                agent,
                orb_project(&store)?,
                SessionKind::Plain,
                Path::new(PROJECT_ROOT),
                "aa",
                now_ms() - HOUR_MS,
            )?;
            let host = Arc::new(if reports {
                FakeHarness::reporting(agent)
            } else {
                FakeHarness::new(agent, records)
            });
            let harness: Arc<dyn Harness> = host.clone();
            let state = State::default();
            let git = FakeGit::local();
            let actor = SessionsActor::restore(SessionsActorDeps {
                services: Services {
                    harnesses: Harnesses::new(vec![harness]),
                    git: GitService::new(git),
                    zmx: zmx(thread.pane),
                },
                state: state.clone(),
                store,
                worktrees_root: PathBuf::from(WORKTREES_ROOT),
                orb_root: dir.path().to_owned(),
                incognito_root: PathBuf::from(INCOGNITO_ROOT),
                wake: Arc::new(|| {}),
                integration_missing: false,
            });
            Ok(Self {
                dir,
                thread,
                host,
                actor,
                state,
            })
        }

        /// Writes pi's `event` report of conversation `aa` from the thread's pane.
        fn report(&self, event: &str) -> Result<(), Report<StoreError>> {
            let panes = self.dir.path().join("panes");
            fs::create_dir_all(&panes).change_context(StoreError)?;
            let json = serde_json::json!({
                "agent": "pi",
                "event": event,
                "session_id": "aa",
                "transcript": null,
                "source": "startup",
                "at": now_ms(),
            });
            fs::write(
                panes.join(format!("{}.json", self.thread.pane.0)),
                json.to_string(),
            )
            .change_context(StoreError)
        }

        fn status(&self) -> Option<ThreadStatus> {
            status_of(&self.state, self.thread.thread)
        }
    }

    /// A zmx running `pane`'s session as process 100, made at unix second 1.
    fn running(pane: PaneId) -> ZmxService {
        zmx_listing(&format!(
            "  name={}\tpid=100\tclients=1\tcreated=1\n",
            pane.zmx_name()
        ))
    }

    /// A zmx that can't list any socket dir.
    fn unlistable(_pane: PaneId) -> ZmxService {
        ZmxService::new(
            Arc::new(FakeZmx::new(ZmxOutput::default())),
            PathBuf::from("/zmx"),
        )
    }

    /// A Claude the user started, whose process and parents are `ancestry`.
    fn interactive(ancestry: &[u32], status: ThreadStatus) -> RunningAgent {
        RunningAgent {
            status,
            ancestry: ancestry.to_vec(),
        }
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_shows_a_claude_panes_interactive_status() -> Result<(), Report<StoreError>> {
        // Given a Claude working under the shell of aa's running pane.
        let mut fx = PaneStatus::new(
            false,
            vec![interactive(&[300, 100, 1], ThreadStatus::Working)],
            running,
        )?;

        // When polling.
        fx.actor.poll().await;

        // Then aa shows the Claude's status.
        assert_eq!(
            fx.status(),
            Some(ThreadStatus::Working),
            "the record under the pane should give the status"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_shows_a_claude_pane_without_a_record_stopped() -> Result<(), Report<StoreError>> {
        // Given aa's pane runs, but no Claude runs under it.
        let mut fx = PaneStatus::new(
            false,
            vec![interactive(&[400, 1], ThreadStatus::Working)],
            running,
        )?;

        // When polling.
        fx.actor.poll().await;

        // Then aa is stopped.
        assert_eq!(
            fx.status(),
            Some(ThreadStatus::Stopped),
            "a pane with no Claude under it should be stopped"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_shows_a_pi_panes_reported_status() -> Result<(), Report<StoreError>> {
        // Given pi in aa's running pane reported a turn underway.
        let mut fx = PaneStatus::new(true, Vec::new(), running)?;
        fx.report("working")?;

        // When polling.
        fx.actor.poll().await;

        // Then aa is working.
        assert_eq!(
            fx.status(),
            Some(ThreadStatus::Working),
            "pi's latest report should give the status"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_leaves_a_pane_thread_as_it_was_when_zmx_cant_list()
    -> Result<(), Report<StoreError>> {
        // Given zmx can't list aa's pane's socket dir.
        let mut fx = PaneStatus::new(false, Vec::new(), unlistable)?;

        // When polling.
        fx.actor.poll().await;

        // Then aa keeps its status from before the poll.
        assert_eq!(
            fx.status(),
            Some(ThreadStatus::Unknown),
            "an unlistable pane should leave its thread as it was"
        );
        Ok(())
    }

    /// Leaves the saved thread `short_id` and every saved session idle for
    /// four days, and returns since when.
    fn session_idle_for_four_days(
        store: &Store,
        short_id: &str,
    ) -> Result<i64, Report<StoreError>> {
        let since = idle_for_four_days(store, short_id)?;
        for row in store.layouts()?.sessions {
            store.save_session(&SessionRow {
                last_activity_at: since,
                ..row
            })?;
        }
        Ok(since)
    }

    /// Saves `change` over every saved session row.
    fn resave_sessions<F>(store: &Store, change: F) -> Result<(), Report<StoreError>>
    where
        F: Fn(SessionRow) -> SessionRow,
    {
        for row in store.layouts()?.sessions {
            store.save_session(&change(row))?;
        }
        Ok(())
    }

    /// The saved session thread `short_id` runs in.
    fn saved_session(store: &Store, short_id: &str) -> Result<SessionRow, Report<StoreError>> {
        let pane = saved(store, short_id)?.pane_id;
        let layouts = store.layouts()?;
        let session = layouts
            .panes
            .iter()
            .find(|row| Some(row.id) == pane)
            .map(|row| row.session_id);
        layouts
            .sessions
            .into_iter()
            .find(|row| Some(row.id) == session)
            .ok_or_else(|| Report::new(StoreError).attach(format!("{short_id} has no session")))
    }

    /// Starts the actor on `store` over `host` with nothing selected.
    fn start_unselected(store: Store, host: &Arc<FakeClaude>) -> (SessionsActor, State) {
        let (actor, state) = start(store, host, Path::new(NO_CLAUDE_DIR));
        state.write().sessions.cursor = None;
        (actor, state)
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn session_idle_for_three_days_auto_settles() -> Result<(), Report<StoreError>> {
        // Given thread aa's session idle for four days.
        let (store, _) = store_with_thread("aa")?;
        let since = session_idle_for_four_days(&store, "aa")?;
        let host = listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, _state) = start_unselected(store, &host);

        // When polling.
        actor.poll().await;

        // Then the session is settled as of its latest activity.
        let row = saved_session(&actor.store, "aa")?;
        assert_eq!(
            (row.settled_override, row.settled_at),
            (Some(SettledOverride::Settled), Some(since)),
            "a long-idle session should settle as of its latest activity"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn selected_session_never_auto_settles() -> Result<(), Report<StoreError>> {
        // Given thread aa's session idle for four days, with aa selected.
        let (store, id) = store_with_thread("aa")?;
        session_idle_for_four_days(&store, "aa")?;
        let host = listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start_unselected(store, &host);
        state.write().sessions.cursor = Some(SidebarItem::Session(SessionId(id.0)));

        // When polling.
        actor.poll().await;

        // Then the session stays active.
        assert_eq!(
            saved_session(&actor.store, "aa")?.settled_override,
            None,
            "the selected session should never auto-settle"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn pinned_session_never_auto_settles() -> Result<(), Report<StoreError>> {
        // Given thread aa's pinned session idle for four days.
        let (store, _) = store_with_thread("aa")?;
        session_idle_for_four_days(&store, "aa")?;
        resave_sessions(&store, |row| SessionRow {
            pinned_at: Some(now_ms() - HOUR_MS),
            ..row
        })?;
        let host = listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, _state) = start_unselected(store, &host);

        // When polling.
        actor.poll().await;

        // Then the session stays active.
        assert_eq!(
            saved_session(&actor.store, "aa")?.settled_override,
            None,
            "a pin keeps the session in view"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn kept_active_session_never_auto_settles() -> Result<(), Report<StoreError>> {
        // Given thread aa's kept-active session idle for four days.
        let (store, _) = store_with_thread("aa")?;
        session_idle_for_four_days(&store, "aa")?;
        resave_sessions(&store, |row| SessionRow {
            settled_override: Some(SettledOverride::Active),
            ..row
        })?;
        let host = listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, _state) = start_unselected(store, &host);

        // When polling.
        actor.poll().await;

        // Then the session stays kept active.
        assert_eq!(
            saved_session(&actor.store, "aa")?.settled_override,
            Some(SettledOverride::Active),
            "an un-settled session stays active until its next turn"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn session_with_a_turn_underway_never_auto_settles() -> Result<(), Report<StoreError>> {
        // Given thread aa's session idle for four days, and aa now working.
        let (store, _) = store_with_thread("aa")?;
        session_idle_for_four_days(&store, "aa")?;
        let host = listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, _state) = start_unselected(store, &host);

        // When polling.
        actor.poll().await;

        // Then the session stays active.
        assert_eq!(
            saved_session(&actor.store, "aa")?.settled_override,
            None,
            "a session with a turn underway should never auto-settle"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn turn_in_a_pane_unsettles_its_session() -> Result<(), Report<StoreError>> {
        // Given thread aa's settled session, and aa now working.
        let (store, _) = store_with_thread("aa")?;
        resave_sessions(&store, |row| SessionRow {
            settled_override: Some(SettledOverride::Settled),
            settled_at: Some(now_ms() - HOUR_MS),
            ..row
        })?;
        let host = listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, _state) = start_unselected(store, &host);

        // When polling.
        actor.poll().await;

        // Then the session is no longer settled.
        let row = saved_session(&actor.store, "aa")?;
        assert_eq!(
            (row.settled_override, row.settled_at),
            (None, None),
            "a turn in one of its panes should un-settle the session"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn turn_in_a_pane_ends_its_sessions_kept_active_mark() -> Result<(), Report<StoreError>> {
        // Given thread aa's kept-active session, and aa now working.
        let (store, _) = store_with_thread("aa")?;
        resave_sessions(&store, |row| SessionRow {
            settled_override: Some(SettledOverride::Active),
            ..row
        })?;
        let host = listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, _state) = start_unselected(store, &host);

        // When polling.
        actor.poll().await;

        // Then the mark is cleared.
        assert_eq!(
            saved_session(&actor.store, "aa")?.settled_override,
            None,
            "turn activity should let auto-settle apply again"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn turn_end_is_its_sessions_latest_activity() -> Result<(), Report<StoreError>> {
        // Given thread aa polled while working.
        let (store, _) = store_with_thread("aa")?;
        let host = listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, _state) = start_unselected(store, &host);
        actor.poll().await;

        // When polling after its turn ended.
        host.set_running(Ok(vec![record("aa", ThreadStatus::Idle)]));
        actor.poll().await;

        // Then the turn's end is the session's latest activity.
        assert_eq!(
            saved_session(&actor.store, "aa")?.last_activity_at,
            saved(&actor.store, "aa")?.last_activity_at,
            "the session's activity should follow its thread's turn end"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn auto_settle_kills_the_sessions_panes() -> Result<(), Report<StoreError>> {
        // Given thread aa's session idle for four days.
        let (store, _) = store_with_thread("aa")?;
        session_idle_for_four_days(&store, "aa")?;
        let pane = saved(&store, "aa")?
            .pane_id
            .ok_or_else(|| Report::new(StoreError).attach("aa has no pane"))?;
        let host = listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, _state) = start_unselected(store, &host);
        let (zmx, service) = recording_zmx();
        actor.services.zmx = service;

        // When polling, which auto-settles the session.
        actor.poll().await;

        // Then its pane is killed.
        assert_eq!(
            killed(&zmx),
            vec![pane.zmx_name()],
            "an auto-settle should kill the session's panes"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn auto_settle_detaches_the_session() -> Result<(), Report<StoreError>> {
        // Given thread aa's attached session idle for four days.
        let (store, _) = store_with_thread("aa")?;
        session_idle_for_four_days(&store, "aa")?;
        let session = saved_session(&store, "aa")?.id;
        let host = listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start_unselected(store, &host);
        state.write().attached.insert(session);

        // When polling, which auto-settles the session.
        actor.poll().await;

        // Then it is no longer attached.
        assert!(
            !state.read().attached.contains(&session),
            "an auto-settled session should be detached"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn kill_settled_kills_listed_panes_of_settled_sessions() -> Result<(), Report<StoreError>> {
        // Given settled session aa and unsettled session bb, zmx listing both
        // their panes.
        let store = Store::open_in_memory()?;
        let settled = insert_thread(&store, "aa")?;
        let active = insert_thread(&store, "bb")?;
        let host = listing(Vec::new());
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.edit_session(settled.session, settle_session_row);
        let zmx = Arc::new(FakeZmx::new(ZmxOutput {
            success: true,
            stdout: format!(
                "  name={}\tpid=100\tclients=0\tcreated=1\n  name={}\tpid=101\tclients=0\tcreated=1\n",
                settled.pane.zmx_name(),
                active.pane.zmx_name()
            ),
            stderr: String::new(),
        }));
        actor.services.zmx = ZmxService::new(zmx.clone(), PathBuf::from("/zmx"));

        // When killing the settled sessions' panes.
        actor.kill_settled();

        // Then only aa's pane is killed.
        assert_eq!(
            killed(&zmx),
            vec![settled.pane.zmx_name()],
            "only a settled session's listed pane should be killed"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn session_auto_settle_stops_nothing() -> Result<(), Report<StoreError>> {
        // Given thread aa's session idle for four days.
        let (store, _) = store_with_thread("aa")?;
        session_idle_for_four_days(&store, "aa")?;
        let host = listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, _state) = start_unselected(store, &host);

        // When polling, which auto-settles the session.
        actor.poll().await;

        // Then nothing is stopped.
        assert!(
            host.stopped().is_empty(),
            "an auto-settle should leave the session's agents running"
        );
        Ok(())
    }
}
