//! The sessions actor — the owner of orb's threads, their statuses, and titles.
//!
//! At start it restores the saved threads into the shared state. Then it polls
//! the session hosts: every second while a turn is underway in any agent
//! pane, every five seconds otherwise, and right away when asked. Each poll
//! reads what each thread's agent is doing: a session a harness runs itself
//! from that harness's record; otherwise from the thread's pane, Stopped when
//! the pane's zmx session isn't running, else the latest report a harness
//! like pi wrote to the pane file, or the record of the Claude whose process
//! runs under the pane (by process ancestry from the pane's zmx process). A
//! thread with no pane is Stopped. Then it stamps when a turn starts, reads
//! new transcript lines for titles and branches, and saves what changed.
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
//! ends in a worktree still on orb's `orb/<hex>` branch, and the harness or
//! the user has titled the thread, the branch is renamed after that title.
//!
//! When the frontend finds a thread's orb worktree gone as it attaches, the
//! actor recreates it at the same path, so the session resumes the conversation
//! there. It prunes git's record of the old worktree, then checks out the
//! branch the worktree was on: the Feature group's branch for a grouped
//! thread, else the thread's own, else orb's `orb/<hex>` for the directory.
//! When that branch is gone it makes it again from the project's default
//! branch, fetched first from origin. Then it asks the frontend to attach, or
//! shows git's reason.
//!
//! It checks branches out in a thread's directory or in a draft's (the
//! project's root, or the draft's existing worktree), unless a turn is
//! underway there. Before the first prompt, picking the default branch from a
//! worktree checks it out in the project's root and moves the thread, or the
//! draft, there. It makes a draft's project a git repository when asked.
//!
//! At start it shows every harness by its name alone, then asks each one, in
//! parallel, for its models and permission modes and shows what it says.
//!
//! Every session start, list, stop and removal goes through the thread's (or
//! draft's, or group's) harness. A thread whose harness orb doesn't know shows
//! as Gone, with nothing to attach.
//!
//! When a harness refuses to start in a directory it hasn't been trusted in,
//! the start waits while the actor asks the frontend to have the user trust
//! the folder the harness names for it (the git root, the main repository
//! for a worktree, or the directory itself outside git). On yes it
//! marks the folder trusted in the harness's config and tries the start once
//! more; a refusal then fails the start. On no, the start fails as
//! `Workspace not trusted`. Either way a worktree made for a failed start is
//! removed.
//!
//! It keeps each thread's place in the sidebar: pinning, settling onto the
//! Settled shelf (which stops a session its harness runs itself), un-settling,
//! and deleting (which removes only such a session). A turn un-settles its
//! thread. A turn that ends while the user is on another thread shows as
//! unseen until they select it. A thread being deleted is hidden from the
//! sidebar until its session is removed; if the removal fails, it shows again.
//!
//! It follows each session's settle lifecycle on its store row: a turn ending
//! in any of its agent panes is its latest activity, a turn underway
//! un-settles it, and a session idle for three days settles itself unless it
//! is pinned, was just un-settled, or is the one the sidebar's cursor is on.
//! Settling a session stops nothing.
//!
//! It creates groups. A Feature group is refused while its branch exists; it
//! gets its worktree when its draft starts. A Research or Learn group lives in
//! orb's own project for its kind, added when first needed, in a new folder
//! copied from the user's template for the kind; orb writes that template
//! from its built-in default when it's missing, and never overwrites a
//! folder that already exists. A new group's default harness, model and
//! permission are the project's last-used ones; the group's draft is
//! selected, and its defaults and draft are saved as the user edits them.
//! Starting the draft starts a thread: a Feature group's first in a new
//! worktree on the branch named after it, which becomes the group's
//! directory, any other in the group's directory. `n` opens a group's draft,
//! making one at the top of the group when it has none. A group is pinned,
//! settled and deleted as a whole: pinning a settled group un-settles it, settling it stops its idle sessions that a harness runs itself, and
//! deleting it deletes each thread, then the group and its directory: a
//! Research or Learn folder, or a Feature worktree (forced, or only pruned
//! from git's records when it's already gone) and its branch. A Feature group
//! whose branch orb would delete isn't merged is kept whole, with the reason.
//! A turn in any of its threads un-settles it.
//!
//! It adds projects and removes them: a removed project leaves `␣n` and the
//! project filter and loses its draft, its threads stay, and adding it again
//! restores it.
//!
//! It restores the sidebar's saved width and project filter at start, and
//! saves them when asked. It does the same for the jump list, dropping saved
//! rows that no longer exist, and drops a deleted group's rows from it.
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
use super::session_host::{
    AttachStart, SessionHostError, SessionOptions, SessionRecord, WorkspaceUntrusted,
};
use super::state::{
    Draft, DraftWorkspace, Group, GroupDefaults, GroupDraft, GroupId, GroupKind, NEW_THREAD,
    Notice, NoticeKind, PaneId, PaneLaunch, Project, ProjectId, ProjectKind, Session, SessionId,
    SessionKind, Sessions, SidebarItem, Thread, ThreadId, ThreadStatus,
};
use super::store::{
    DraftRow, GroupRow, LastUsed, LastWorkspace, NewGroup, NewPaneThread, NewThread, PaneRow,
    SessionRow, SettledOverride, Store, StoreError, TabRow, ThreadRow, Ui,
};
use super::template;
use super::validator::{group_exists, on_disk};
use crate::command::Workspace;
use crate::common::{Services, State, Wake};
use crate::feat::git::git_service::{GitError, GitRef, GitService, git_reason};
use crate::feat::git::validator::BUSY_DIRECTORY;
use crate::feat::git::worktree::{
    hex, hex_branch, is_orb_worktree, new_worktree_path, previous_worktree, slug,
};
use crate::feat::harness::{Harness, HarnessId, HarnessInfo, Scan, TranscriptFormat, claude};
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
/// The error shown when a group's or the Incognito folder can't be made.
const FOLDER_UNMADE: &str = "couldn't make the folder";
/// The error shown when a deleted session's folder can't be removed.
const FOLDER_UNREMOVED: &str = "couldn't remove the folder";
/// The error shown when a started session can't be saved or shown.
const NEW_SESSION_UNSAVED: &str = "couldn't save the new session";
/// The error shown when the harness's config can't be updated to trust a folder.
const TRUST_UNSAVED: &str = "couldn't trust the folder";
/// The error shown when the user declines to trust a start's folder.
const TRUST_DECLINED: &str = "Workspace not trusted";
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
/// shows (`AppState::harnesses`): the projects, their drafts and groups, the
/// sessions with their names, pins and settles, the threads' statuses and
/// titles, the latest host error, the origin ref a start is fetching, the
/// folder a harness asks to trust before a start, and the started or restored
/// session the frontend should attach to. It adds orb's Incognito project at
/// start. It also restores the sidebar's width and project filter, and
/// selects a new group's draft. It loads every session's layout at start,
/// adds the panes splits and new tabs make (it saves their store rows, so it
/// gives them their ids), lays out a new thread's session, drops a deleted
/// session's layout, and saves a layout when asked. The intent handler also
/// moves the cursor, opens and closes the shelf, marks a start as starting,
/// edits a draft's fields before asking for them to be saved, and resizes or
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
    /// The saved groups, as last written to the store.
    groups: Vec<GroupRow>,
    /// The saved panes, as last written to the store.
    panes: HashMap<PaneId, PaneRow>,
    /// Each saved session: its directory, where its new panes start, and
    /// its settle lifecycle.
    sessions: HashMap<SessionId, SessionRow>,
    /// The start waiting for the user to trust the folder its harness names,
    /// and that path.
    pending: Option<(PendingStart, PathBuf)>,
    /// The agents' reports of what runs in each pane.
    pane_files: PaneFiles,
    /// Each pane's latest agent report.
    pane_reports: HashMap<PaneId, PaneReport>,
    /// The threads a harness runs itself, as the last poll found: the only
    /// ones settling stops and deleting removes.
    hosted: HashSet<ThreadId>,
}

/// Poll the session host now.
#[derive(Debug)]
pub struct Poll;

/// Split `session`'s focused pane `split` with a new shell pane, and save
/// the layout.
#[derive(Debug)]
pub struct SplitPane {
    pub session: SessionId,
    pub split: Split,
}

/// Open a tab of one new shell pane in a session, and save the layout.
#[derive(Debug)]
pub struct NewTab(pub SessionId);

/// Save a session's tabs, splits, focus and pane names as the app state has
/// them.
#[derive(Debug)]
pub struct SaveLayout(pub SessionId);

/// Stop the Claude `--bg` sessions the store's migration to sessions turned
/// into panes, so they don't fight the `claude --resume` those panes run.
#[derive(Debug)]
pub struct StopMigrated;

/// Kill the panes zmx still runs for sessions that are already settled.
#[derive(Debug)]
pub struct KillSettled;

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

/// Check a branch out in a group's worktree, moving every thread in it.
#[derive(Debug)]
pub struct CheckoutGroup {
    pub group: GroupId,
    pub git_ref: GitRef,
}

/// Mark the waiting start's project path trusted in the harness's config, as the
/// user said yes, and try the start once more.
#[derive(Debug)]
pub struct TrustWorkspace;

/// End the waiting start as a failed one, as the user declined to trust its
/// project path.
#[derive(Debug)]
pub struct DeclineTrust;

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

/// Remove a project from `␣n` and the project filter and discard its draft.
#[derive(Debug)]
pub struct RemoveProject(pub ProjectId);

/// Create a `kind` group named `name` (a slug) with a draft: in `project`
/// for a Feature group, else in orb's own project for the kind.
#[derive(Debug)]
pub struct CreateGroup {
    pub kind: GroupKind,
    pub project: Option<ProjectId>,
    pub name: String,
}

/// Start group `.0`'s first thread from its draft.
#[derive(Debug)]
pub struct StartGroupDraft(pub GroupId);

/// Save group `.0`'s default model and permission as the app state has them.
#[derive(Debug)]
pub struct SaveGroupDraft(pub GroupId);

/// Recreate session `.0`'s orb worktree, gone from disk, then attach to it.
#[derive(Debug)]
pub struct RestoreWorktree(pub SessionId);

/// What a harness's probe found: `.0` replaces the entry with its id.
#[derive(Debug)]
pub struct Probed(pub HarnessInfo);

/// A session start in flight: what it is for, where it runs, the branch
/// checked out there when known, the worktree orb made for it, if any, and
/// the options the session starts with.
struct PendingStart {
    kind: StartKind,
    cwd: PathBuf,
    branch: Option<String>,
    made: Option<MadeWorktree>,
    options: SessionOptions,
    /// The harness the session starts in.
    harness: HarnessId,
}

/// What a session start is for.
enum StartKind {
    /// A new thread from the project's draft, whose workspace was of kind
    /// `workspace`.
    Draft {
        project: ProjectId,
        workspace: LastWorkspace,
    },
    /// A thread of group `group` in `project`, from its draft. The cursor
    /// follows the thread if it's still on `from`.
    Group {
        project: ProjectId,
        group: GroupId,
        from: Option<SidebarItem>,
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
        for harness in actor.services.harnesses.all() {
            let harness = Arc::clone(harness);
            let actor_ref = actor_ref.clone();
            tokio::spawn(async move {
                let info = harness.probe().await;
                let _ = actor_ref.tell(Probed(info)).await;
            });
        }
        {
            let actor_ref = actor_ref.clone();
            tokio::spawn(async move {
                let _ = actor_ref.tell(StopMigrated).await;
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
        _msg: StopMigrated,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.stop_migrated().await;
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

impl Message<Probed> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        Probed(mut info): Probed,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        {
            let notice = info.notice.take();
            let mut app = self.state.write();
            if let Some(entry) = app.harnesses.iter_mut().find(|entry| entry.id == info.id) {
                *entry = info;
            }
            if let Some(notice) = notice {
                app.sessions.error = Some(notice);
            }
        }
        (self.wake)();
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

impl Message<CheckoutGroup> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        CheckoutGroup { group, git_ref }: CheckoutGroup,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.check_out_group(group, &git_ref);
    }
}

impl Message<TrustWorkspace> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        _msg: TrustWorkspace,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.trust_workspace().await;
    }
}

impl Message<DeclineTrust> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        _msg: DeclineTrust,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.decline_trust();
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
        self.settle_session(id).await;
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
        self.delete_session(id).await;
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

impl Message<CreateGroup> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        CreateGroup {
            kind,
            project,
            name,
        }: CreateGroup,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.create_group(kind, project, name);
    }
}

impl Message<StartGroupDraft> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        StartGroupDraft(id): StartGroupDraft,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.start_group_draft(id).await;
    }
}

impl Message<SaveGroupDraft> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        SaveGroupDraft(id): SaveGroupDraft,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.save_group_draft(id);
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
    /// Shows the saved projects, threads and drafts, sizes the sidebar as it
    /// was saved, kept within its bounds, filters it to the saved project if
    /// that's still shown and not removed, and selects its first row. Each
    /// draft learns what git says about it. The saved jump list comes back
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
        let (projects, rows, drafts, groups, error) = match store.load() {
            Ok((projects, rows, drafts, groups)) => (
                projects,
                rows,
                drafts,
                groups,
                added.err().map(|_report| SAVE_FAILED.to_owned()),
            ),
            Err(_) => (
                Vec::new(),
                Vec::new(),
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
                draft: drafts
                    .iter()
                    .find(|draft| draft.project_id == project.id)
                    .map(|row| with_git(&services.git, &project.root, draft_of(row))),
                id: project.id,
                title: project.title,
                root: project.root,
                created_at: from_ms(project.created_at),
                removed: project.removed_at.is_some(),
                kind: project.kind,
                groups: groups
                    .iter()
                    .filter(|row| row.project_id == project.id)
                    .map(|row| group(row, &rows))
                    .collect(),
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
                    .filter(|&item| exists(&projects, &session_rows, item))
                    .collect(),
            )
        };
        {
            let mut app = state.write();
            app.sidebar.width = ui.sidebar_width.map_or(DEFAULT_WIDTH, clamp_width);
            app.jumps = jumps;
            app.harnesses = services.harnesses.placeholders();
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
            groups,
            panes,
            sessions: session_rows,
            pending: None,
            pane_reports: HashMap::new(),
            hosted: HashSet::new(),
        }
    }

    /// Asks the host what every session is doing and shows it. Returns how
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
            pinned_at: None,
            settled_override: None,
            settled_at: None,
            unsettled_at: None,
            last_activity_at: now,
            last_visited_at: now,
            ai_titled: false,
            model: None,
            permission_mode: None,
            renamed_title: None,
            group_id: None,
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

    /// Saves pane `pane`'s resume command, unless it already has it.
    fn set_resume(&mut self, pane: PaneId, resume: Option<&str>) -> Result<(), Report<StoreError>> {
        let Some(row) = self.panes.get_mut(&pane) else {
            return Ok(());
        };
        if row.resume.as_deref() != resume {
            self.store.set_pane_resume(pane, resume)?;
            row.resume = resume.map(str::to_owned);
        }
        Ok(())
    }

    /// Asks the host what every session is doing and shows it.
    /// A harness whose list fails leaves its threads as they were; the
    /// others still update, and the failure is returned after.
    async fn sync(&mut self) -> Result<(), Report<SessionHostError>> {
        let mut listed: HashMap<HarnessId, Vec<SessionRecord>> = HashMap::new();
        let mut failure = None;
        for harness in self.services.harnesses.all() {
            let id = harness.id();
            let short_ids: Vec<String> = self
                .rows
                .iter()
                .filter(|row| row.harness == id)
                .map(|row| row.short_id.clone())
                .collect();
            match harness.list(&short_ids).await {
                Ok(records) => {
                    listed.insert(id, records);
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
    /// out) and the records matched to the panes their processes run under.
    fn live(&self, listed: &HashMap<HarnessId, Vec<SessionRecord>>) -> Live {
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
        let records: Vec<SessionRecord> = listed.values().flatten().cloned().collect();
        Live {
            matched: match_records(&records, &roots),
            sessions,
        }
    }

    /// Updates every saved thread from its record and transcript, follows its
    /// activity and its group's and session's settle lifecycle, saves the
    /// ones that changed, then shows them all in one write. A thread whose
    /// harness wasn't `listed` (its list failed) is left as it was; one whose
    /// harness orb doesn't know is Gone.
    fn apply(&mut self, listed: &HashMap<HarnessId, Vec<SessionRecord>>, live: &Live) {
        let now = now_ms();
        let selected = match self.state.read().sessions.cursor {
            Some(SidebarItem::Session(id)) => Some(id),
            _ => None,
        };
        let mut statuses = Vec::with_capacity(self.rows.len());
        let mut error = None;
        let mut hosted = HashSet::new();
        for row in &mut self.rows {
            let harness = self.services.harnesses.get(&row.harness);
            let records = match (harness, listed.get(&row.harness)) {
                (Some(_), None) => {
                    statuses.push(None);
                    continue;
                }
                (_, records) => records.map_or(&[][..], Vec::as_slice),
            };
            let record = hosted_record(records, &row.short_id);
            let Some(status) = status_now(row, harness, record, live, &self.pane_reports) else {
                statuses.push(None);
                continue;
            };
            if record.is_some() {
                hosted.insert(row.id);
            }
            let before = row.clone();
            let was_in_progress = row.turn_started_at.is_some();
            let format = harness.map(|harness| harness.as_ref() as &dyn TranscriptFormat);
            let asks_git = update_row(row, record, status, now, format);
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
        self.hosted = hosted;
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
                    changed |= show(
                        thread,
                        row,
                        status,
                        pane_launch(&self.services, row, &self.panes),
                    );
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

    /// Gives project `id` a draft unless it has one: the project's last-used
    /// workspace, harness, model and permission, else the latest used
    /// project's harness, model and permission in a local checkout, else the
    /// first registered harness. A last-used harness orb no longer knows gives
    /// the first registered one with its default model and permission. A
    /// remembered previous worktree the project no longer has falls back to a
    /// local checkout. The branch is the
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
        let (harness, model, permission) = self.last_settings(last);
        let draft = with_git(
            &self.services.git,
            &root,
            Draft {
                branch,
                workspace,
                model,
                permission,
                created_at: from_ms(now_ms()),
                repo: true,
                from: None,
                harness,
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

    /// The harness, model and permission a new draft or group starts with:
    /// `last`, else the latest last-used record from any project. A harness
    /// orb no longer knows gives the first registered one with its defaults;
    /// a record from before harnesses keeps its model and permission on the
    /// first one.
    fn last_settings(&self, last: Option<LastUsed>) -> (HarnessId, Option<String>, Option<String>) {
        let default = self.services.harnesses.default_id();
        match last.or_else(|| self.store.latest_last_used().ok().flatten()) {
            Some(LastUsed {
                harness: Some(id), ..
            }) if self.services.harnesses.get(&id).is_none() => (default, None, None),
            Some(used) => (
                used.harness.unwrap_or(default),
                used.model,
                used.permission_mode,
            ),
            None => (default, None, None),
        }
    }

    /// Saves project `id`'s draft as the app state has it, first bringing
    /// what git says about it up to date; see [`with_git`].
    fn save_draft(&mut self, id: ProjectId) {
        let Some((root, _, draft)) = self.draft(id) else {
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

    /// Saves group `id`'s default setup and draft as the app state has them,
    /// on its kept row, so nothing else of the group changes.
    fn save_group_draft(&mut self, id: GroupId) {
        let shown = self
            .state
            .read()
            .sessions
            .group(id)
            .map(|(_, group)| (group.defaults.clone(), group.draft.clone()));
        let Some((defaults, draft)) = shown else {
            return;
        };
        self.edit_group(id, |row, _now| {
            row.harness = defaults.harness;
            row.draft_model = defaults.model;
            row.draft_permission_mode = defaults.permission;
            row.draft = draft;
        });
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
    /// project that isn't a git repository always starts in the root. An
    /// Incognito draft's folder is made first when missing; any other missing
    /// folder fails the start with its path.
    async fn start_draft(&mut self, id: ProjectId) {
        let Some((root, kind, draft)) = self.draft(id) else {
            return self.end_start(Err("the draft is gone".to_owned()));
        };
        if kind == ProjectKind::Incognito && fs::create_dir_all(&root).is_err() {
            return self.end_start(Err(FOLDER_UNMADE.to_owned()));
        }
        let options = SessionOptions {
            model: draft.model,
            permission_mode: draft.permission,
        };
        let harness = draft.harness;
        let workspace = if draft.repo {
            draft.workspace
        } else {
            DraftWorkspace::Local
        };
        let (workspace, cwd, made) = match workspace {
            DraftWorkspace::Local if root.is_dir() => (LastWorkspace::Local, root, None),
            DraftWorkspace::Local => {
                return self.end_start(Err(format!(
                    "project folder no longer exists: {}",
                    root.display()
                )));
            }
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
                match self.add_worktree(&root, draft.branch.as_deref(), None) {
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
            harness,
        };
        self.start(pending, true).await;
    }

    /// Project `id`'s root, kind and draft, if it has one.
    fn draft(&self, id: ProjectId) -> Option<(PathBuf, ProjectKind, Draft)> {
        self.state
            .read()
            .sessions
            .projects
            .iter()
            .find(|project| project.id == id)
            .and_then(|project| Some((project.root.clone(), project.kind, project.draft.clone()?)))
    }

    /// Starts a session for `pending` in its harness and finishes what it was
    /// for. If the harness hasn't been trusted in its directory and
    /// `allow_trust`, the start waits for the user to trust the harness's
    /// folder for it (asking the frontend once). If it fails, the reason
    /// shows and a worktree made for it is removed.
    async fn start(&mut self, pending: PendingStart, allow_trust: bool) {
        let harness = match self.harness(&pending.harness) {
            Ok(harness) => harness,
            Err(error) => {
                if let Some(made) = &pending.made {
                    self.remove_made(made);
                }
                return self.end_start(Err(error));
            }
        };
        let created = harness.create(&pending.cwd, &pending.options).await;
        if let Err(report) = &created
            && allow_trust
            && report.contains::<WorkspaceUntrusted>()
        {
            let dir = harness.trust_dir(&pending.cwd);
            let asked = {
                let mut app = self.state.write();
                app.sessions.trust.replace(dir.clone()).as_ref() == Some(&dir)
            };
            self.pending = Some((pending, dir));
            if !asked {
                (self.wake)();
            }
            return;
        }
        let PendingStart {
            kind,
            cwd,
            branch,
            made,
            options,
            harness,
        } = pending;
        match (created, kind) {
            (Ok(created), StartKind::Draft { project, workspace }) => {
                let thread = self.save_new(
                    project,
                    &cwd,
                    &created.short_id,
                    branch,
                    &options,
                    None,
                    &harness,
                );
                let used = LastUsed {
                    harness: Some(harness),
                    workspace,
                    model: options.model,
                    permission_mode: options.permission_mode,
                };
                self.finish_draft(project, &used, thread);
            }
            (
                Ok(created),
                StartKind::Group {
                    project,
                    group,
                    from,
                },
            ) => {
                let thread = self.save_new(
                    project,
                    &cwd,
                    &created.short_id,
                    branch,
                    &options,
                    Some(group),
                    &harness,
                );
                self.finish_group_start(project, group, from, &cwd, thread);
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
                self.finish_move(moved, &harness, &old_short_id, &old_cwd)
                    .await;
            }
            (Err(report), _) => {
                if let Some(made) = made {
                    self.remove_made(&made);
                }
                self.end_start(Err(reason(&report)));
            }
        }
    }

    /// Marks the waiting start's project path trusted in the harness's config and
    /// tries the start once more; a second refusal fails it. If the path
    /// can't be marked, the start fails and a worktree made for it is removed.
    async fn trust_workspace(&mut self) {
        let Some((pending, dir)) = self.pending.take() else {
            return;
        };
        self.state.write().sessions.trust = None;
        let trusted = self.harness(&pending.harness).and_then(|harness| {
            harness
                .trust(&dir)
                .map_err(|_report| TRUST_UNSAVED.to_owned())
        });
        match trusted {
            Ok(()) => self.start(pending, false).await,
            Err(error) => {
                if let Some(made) = &pending.made {
                    self.remove_made(made);
                }
                self.end_start(Err(error));
            }
        }
    }

    /// Ends the waiting start as a failed one, as the user declined to trust
    /// its project path: the draft stays, a worktree made for it is removed,
    /// and `Workspace not trusted` shows.
    fn decline_trust(&mut self) {
        let Some((pending, _)) = self.pending.take() else {
            return;
        };
        self.state.write().sessions.trust = None;
        if let Some(made) = &pending.made {
            self.remove_made(made);
        }
        self.end_start(Err(TRUST_DECLINED.to_owned()));
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
            let session = thread.session();
            project.draft = None;
            project.threads.insert(0, thread);
            if sessions.cursor == Some(SidebarItem::Draft(project_id)) {
                sessions.cursor = session.map(SidebarItem::Session);
                sessions.attach = session;
            }
            saved.map_err(|_report| SAVE_FAILED.to_owned())
        });
        self.end_start(result);
    }

    /// Starts a thread from group `id`'s draft with its resolved settings: a
    /// Feature group not yet started in a new worktree of its project on the
    /// branch named after it, refused while that branch exists; any other
    /// group in its directory, a Feature group on the branch checked out there.
    async fn start_group_draft(&mut self, id: GroupId) {
        let found = self
            .state
            .read()
            .sessions
            .projects
            .iter()
            .find_map(|project| {
                let group = project.groups.iter().find(|group| group.id == id)?;
                let (harness, model, permission) = group.draft_settings()?;
                Some((
                    project.id,
                    project.root.clone(),
                    project.title.clone(),
                    group.clone(),
                    harness.clone(),
                    SessionOptions {
                        model: model.map(str::to_owned),
                        permission_mode: permission.map(str::to_owned),
                    },
                ))
            });
        let Some((project, root, title, group, harness, options)) = found else {
            return self.end_start(Err("the draft is gone".to_owned()));
        };
        let (cwd, branch, made) = match (group.kind, group.dir) {
            (GroupKind::Feature, None) => {
                let branch = group.branch.unwrap_or(group.name);
                if self.services.git.branch_exists(&root, &branch) {
                    return self.end_start(Err(on_disk(GroupKind::Feature, &branch, &title)));
                }
                match self.add_worktree(&root, None, Some(&branch)) {
                    Ok(made) => (made.path.clone(), Some(branch), Some(made)),
                    Err(report) => return self.end_start(Err(git_reason(&report))),
                }
            }
            (kind, Some(dir)) if dir.is_dir() => {
                let branch = match kind {
                    GroupKind::Feature => current_branch(&self.services.git, &dir),
                    GroupKind::Research | GroupKind::Learn => None,
                };
                (dir, branch, None)
            }
            (_, dir) => {
                let dir = dir.unwrap_or_default();
                return self.end_start(Err(format!("folder no longer exists: {}", dir.display())));
            }
        };
        let pending = PendingStart {
            kind: StartKind::Group {
                project,
                group: id,
                from: Some(SidebarItem::GroupDraft(id)),
            },
            cwd,
            branch,
            made,
            options,
            harness,
        };
        self.start(pending, true).await;
    }

    /// Adds group `group_id`'s thread `created`, first in its project: its
    /// first thread replaces the draft, in `cwd` (a Feature group's directory
    /// from now on). If the cursor is still on `from`, selects the thread and
    /// asks the frontend to attach, in the same state write. Doesn't record
    /// last-used settings.
    fn finish_group_start(
        &mut self,
        project_id: ProjectId,
        group_id: GroupId,
        from: Option<SidebarItem>,
        cwd: &Path,
        created: Result<Thread, String>,
    ) {
        let result = created.and_then(|thread| {
            let saved = match self.groups.iter_mut().find(|row| row.id == group_id) {
                Some(row) => {
                    row.dir.get_or_insert_with(|| cwd.to_owned());
                    row.draft = None;
                    self.store
                        .save_group(row)
                        .map_err(|_report| SAVE_FAILED.to_owned())
                }
                None => Err(NEW_SESSION_UNSAVED.to_owned()),
            };
            let mut app = self.state.write();
            let sessions = &mut app.sessions;
            let project = sessions
                .projects
                .iter_mut()
                .find(|project| project.id == project_id)
                .ok_or_else(|| NEW_SESSION_UNSAVED.to_owned())?;
            if let Some(group) = project.groups.iter_mut().find(|group| group.id == group_id) {
                group.dir.get_or_insert_with(|| cwd.to_owned());
                group.draft = None;
            }
            let session = thread.session();
            project.threads.insert(0, thread);
            if sessions.cursor == from {
                sessions.cursor = session.map(SidebarItem::Session);
                sessions.attach = session;
            }
            saved
        });
        self.end_start(result);
    }

    /// Ends a session start: stops showing it as starting, unless another
    /// start still waits for trust, and shows `result`'s error, or clears the
    /// error and polls the new session now.
    fn end_start(&self, result: Result<(), String>) {
        let succeeded = result.is_ok();
        {
            let mut app = self.state.write();
            let sessions = &mut app.sessions;
            sessions.starting = self.pending.is_some();
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
                    .add_worktree(&root, None, None)
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
            harness: row.harness.clone(),
        })
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
    /// threads' (its Feature group's branch, else its own), else orb's
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
            .find_map(|row| {
                row.group_id
                    .and_then(|group| self.groups.iter().find(|g| g.id == group))
                    .filter(|group| group.kind == GroupKind::Feature)
                    .and_then(|group| group.branch.clone())
                    .or_else(|| row.branch.clone())
            });
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

    /// Finishes a move once the new session runs: removes the old session,
    /// saves and shows the thread in its new workspace, and removes the orb
    /// worktree it left if no thread uses it and it has no changes.
    async fn finish_move(
        &mut self,
        moved: Moved,
        harness: &HarnessId,
        old_short_id: &str,
        old_cwd: &Path,
    ) {
        let removed = match self.harness(harness) {
            Ok(harness) => harness.remove(old_short_id).await.map_err(|r| reason(&r)),
            Err(error) => Err(error),
        };
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
        let shown = unpolled(&self.services, row, &self.panes);
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
            (Ok(()), Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        };
        self.end_start(result);
    }

    /// Checks `git_ref` out in group `id`'s worktree, which moves every
    /// thread in it.
    fn check_out_group(&mut self, id: GroupId, git_ref: &GitRef) {
        let dir = self
            .groups
            .iter()
            .find(|row| row.id == id)
            .and_then(|row| row.dir.clone());
        if let Some(dir) = dir {
            let _ = self.check_out_in(&dir, git_ref);
        }
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

    /// Saves and shows `branch` on every thread in `cwd`, on the group whose
    /// worktree it is, and on the draft there: a local draft of a project
    /// rooted there, or one in that worktree.
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
                show(
                    thread,
                    row,
                    status,
                    pane_launch(&self.services, row, &self.panes),
                );
            }
        }
        for row in self
            .groups
            .iter_mut()
            .filter(|row| row.dir.as_deref() == Some(cwd))
        {
            row.branch = Some(branch.to_owned());
            if self.store.save_group(row).is_err() {
                saved = Err(SAVE_FAILED.to_owned());
            }
            if let Some(shown) = app.sessions.group_mut(row.id) {
                *shown = group(row, &self.rows);
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

    /// Saves a session just started in `harness` in `cwd` on `branch` with
    /// `options` under the project, in `group` if any.
    #[expect(
        clippy::too_many_arguments,
        reason = "each is one field of the new thread's row"
    )]
    fn save_new(
        &mut self,
        project_id: ProjectId,
        cwd: &Path,
        short_id: &str,
        branch: Option<String>,
        options: &SessionOptions,
        group: Option<GroupId>,
        harness: &HarnessId,
    ) -> Result<Thread, String> {
        let now = now_ms();
        let new = NewThread {
            project_id,
            short_id: short_id.to_owned(),
            cwd: cwd.to_owned(),
            created_at: now,
            model: options.model.clone(),
            permission_mode: options.permission_mode.clone(),
            group_id: group,
            harness: harness.clone(),
            zmx: self
                .services
                .harnesses
                .get(harness)
                .and_then(|harness| harness.zmx_session(short_id)),
        };
        let Ok(inserted) = self.store.insert_thread(&new) else {
            return Err(NEW_SESSION_UNSAVED.to_owned());
        };
        let pane = PaneRow {
            id: inserted.pane,
            session_id: inserted.session,
            cwd: new.cwd.clone(),
            zmx_name: new.zmx.as_ref().map(|zmx| zmx.name.clone()),
            zmx_dir: new.zmx.as_ref().map(|zmx| zmx.dir.clone()),
            resume: None,
            migrated_bg: None,
            name: None,
        };
        let entry = pane_entry(&self.services, &self.orb_root, &pane);
        {
            let mut app = self.state.write();
            let kind = app
                .sessions
                .projects
                .iter()
                .find(|project| project.id == project_id)
                .map_or(SessionKind::Plain, |project| session_kind(project.kind));
            let session = SessionRow {
                id: inserted.session,
                project_id,
                kind,
                name: None,
                branch: None,
                created_at: now,
                dir: new.cwd.clone(),
                active_tab: 0,
                pinned_at: None,
                settled_override: None,
                settled_at: None,
                unsettled_at: None,
                last_activity_at: now,
            };
            app.layouts
                .insert(inserted.session, SessionLayout::of(entry));
            app.sessions.sessions.push(show_session(&session));
            self.sessions.insert(inserted.session, session);
        }
        self.panes.insert(pane.id, pane);
        let row = ThreadRow {
            id: inserted.thread,
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
            renamed_title: None,
            group_id: new.group_id,
            harness: new.harness,
            pane_id: Some(inserted.pane),
            orb_session: Some(inserted.session),
        };
        if row.branch.is_some() && self.store.save_thread(&row).is_err() {
            return Err(NEW_SESSION_UNSAVED.to_owned());
        }
        let thread = unpolled(&self.services, &row, &self.panes);
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
                    show_project(sessions, id, title, root, ProjectKind::Normal, now);
                }
                Err(error) => sessions.error = Some(error),
            }
        }
        (self.wake)();
    }

    /// Creates a `kind` group named `name` with a draft, and selects the
    /// draft. Research/Learn: orb's project for the kind (added or restored),
    /// then the folder `<orb_root>/<kind>/<name>` copied from
    /// `<orb_root>/templates/<kind>`, seeded first when missing. Feature:
    /// refused while the branch `name` exists. Any refusal or failure shows on
    /// the mode line and leaves no group; a folder this call made is removed
    /// again. The name box that asked closes once the group is made, and
    /// otherwise stays open with its text. A made group outside the project
    /// filter clears the filter, and saves that.
    fn create_group(&mut self, kind: GroupKind, project: Option<ProjectId>, name: String) {
        let now = now_ms();
        let own = own_folder(kind);
        let resolved = match (own, project) {
            (Some((project_kind, dir, title)), _) => {
                let root = self.orb_root.join(dir);
                fs::create_dir_all(&root)
                    .map_err(|_error| FOLDER_UNMADE.to_owned())
                    .and_then(|()| {
                        self.store
                            .add_project(&root, title, project_kind, now)
                            .map_err(|_report| SAVE_FAILED.to_owned())
                    })
                    .map(|id| (id, root, title.to_owned()))
            }
            (None, Some(id)) => self
                .state
                .read()
                .sessions
                .projects
                .iter()
                .find(|p| p.id == id)
                .map(|p| (id, p.root.clone(), p.title.clone()))
                .ok_or_else(|| "the project is gone".to_owned()),
            (None, None) => Err("the project is gone".to_owned()),
        };
        let created = resolved
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|(id, root, title)| self.new_group(kind, *id, root, title, name, now));
        let unfiltered = {
            let mut app = self.state.write();
            let app = &mut *app;
            let sessions = &mut app.sessions;
            if let (Some((project_kind, ..)), Ok((id, root, title))) = (own, &resolved) {
                show_project(
                    sessions,
                    *id,
                    title.clone(),
                    root.clone(),
                    project_kind,
                    now,
                );
            }
            let made = created.is_ok();
            let unfiltered = match (created, resolved) {
                (Ok(group), Ok((id, ..))) => {
                    sessions.cursor = Some(SidebarItem::GroupDraft(group.id));
                    sessions.error = None;
                    if let Some(p) = sessions.projects.iter_mut().find(|p| p.id == id) {
                        p.groups.push(group);
                    }
                    let outside = sessions.filter.is_some_and(|filter| filter != id);
                    if outside {
                        sessions.filter = None;
                    }
                    outside
                }
                (Ok(_), Err(_)) => false,
                (Err(error), _) => {
                    sessions.error = Some(error);
                    false
                }
            };
            answer_name_box(app, made);
            unfiltered
        };
        if unfiltered {
            self.save_ui();
        }
        (self.wake)();
    }

    /// Saves a `kind` group `name` in `project` (rooted at `root`, titled
    /// `title`) with its folder, if it has one, and returns how it shows.
    /// The error is the mode-line text.
    fn new_group(
        &mut self,
        kind: GroupKind,
        project: ProjectId,
        root: &Path,
        title: &str,
        name: String,
        now: i64,
    ) -> Result<Group, String> {
        let taken = self
            .groups
            .iter()
            .any(|row| row.project_id == project && row.kind == kind && row.name == name);
        if taken {
            return Err(group_exists(&name));
        }
        let (dir, branch) = match own_folder(kind) {
            None if self.services.git.branch_exists(root, &name) => {
                return Err(on_disk(kind, &name, title));
            }
            None => (None, Some(name.clone())),
            Some((_, kind_dir, _)) => {
                let dir = root.join(&name);
                if dir.exists() {
                    return Err(on_disk(kind, &name, title));
                }
                let template = self.orb_root.join("templates").join(kind_dir);
                let seeded = if template.exists() {
                    Ok(())
                } else {
                    template::seed(&template, kind)
                };
                let copied = seeded.and_then(|()| template::copy(&template, &dir));
                if copied.is_err() {
                    let _ = fs::remove_dir_all(&dir);
                    return Err(FOLDER_UNMADE.to_owned());
                }
                (Some(dir), None)
            }
        };
        let (harness, draft_model, draft_permission_mode) =
            self.last_settings(self.store.last_used(project).ok().flatten());
        let inserted = self.store.insert_group(&NewGroup {
            project_id: project,
            kind,
            name: name.clone(),
            dir: dir.clone(),
            branch: branch.clone(),
            created_at: now,
            draft_model: draft_model.clone(),
            draft_permission_mode: draft_permission_mode.clone(),
            harness: harness.clone(),
        });
        let Ok(id) = inserted else {
            if let Some(dir) = &dir {
                let _ = fs::remove_dir_all(dir);
            }
            return Err(SAVE_FAILED.to_owned());
        };
        let row = GroupRow {
            id,
            project_id: project,
            kind,
            name,
            dir,
            branch,
            created_at: now,
            pinned_at: None,
            settled_override: None,
            settled_at: None,
            unsettled_at: None,
            draft_model,
            draft_permission_mode,
            harness,
            draft: None,
        };
        let shown = group(&row, &self.rows);
        self.groups.push(row);
        Ok(shown)
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
    /// underway (a turn that started after the key press wins), then stops
    /// its idle threads a harness runs itself and kills every pane, keeping
    /// its tabs, layout and each pane's resume command.
    async fn settle_session(&mut self, id: SessionId) {
        let agents: Vec<(HarnessId, String, Option<ThreadStatus>, bool)> = self
            .session_rows(id)
            .map(|row| {
                (
                    row.harness.clone(),
                    row.short_id.clone(),
                    self.status(row.id),
                    self.hosted.contains(&row.id),
                )
            })
            .collect();
        if agents
            .iter()
            .any(|(_, _, status, _)| status.is_some_and(ThreadStatus::in_progress))
        {
            return;
        }
        self.edit_session(id, settle_session_row);
        for (harness, short_id, status, hosted) in agents {
            if hosted && status == Some(ThreadStatus::Idle) {
                // ponytail: stop runs on the actor (~0.7 s); spawn it if triage feels laggy.
                self.stop(&harness, &short_id).await;
            }
        }
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

    /// Deletes session `id`: removes its threads a harness runs itself first
    /// (a failure keeps the session, shown again with the reason), kills
    /// every pane, deletes a Research or Learn folder that is orb's own and
    /// no other session uses, then its tabs, panes, threads (ended ones
    /// included) and row, and
    /// forgets them. Claude transcripts, pi session files and worktrees
    /// stay. However it ends, the session is no longer hidden as being
    /// deleted.
    async fn delete_session(&mut self, id: SessionId) {
        let hosted: Vec<(HarnessId, String)> = self
            .session_rows(id)
            .filter(|row| self.hosted.contains(&row.id))
            .map(|row| (row.harness.clone(), row.short_id.clone()))
            .collect();
        for (harness, short_id) in hosted {
            let removed = match self.harness(&harness) {
                Ok(harness) => harness.remove(&short_id).await.map_err(|r| reason(&r)),
                Err(_) => Ok(()),
            };
            if let Err(error) = removed {
                {
                    let mut app = self.state.write();
                    app.sessions.deleting.remove(&id);
                    app.sessions.error = Some(error);
                }
                return (self.wake)();
            }
        }
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
    /// `split`, or in a new tab when `None`. Then saves the layout. Nothing
    /// happens once the session's layout is gone; a pane that can't be saved
    /// shows why.
    fn add_pane(&mut self, session: SessionId, split: Option<Split>) {
        let Some(dir) = self.sessions.get(&session).map(|row| row.dir.clone()) else {
            return;
        };
        if self.state.read().layouts.get(session).is_none() {
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
                show(
                    thread,
                    row,
                    status,
                    pane_launch(&self.services, row, &self.panes),
                );
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

    /// Changes group `id`'s saved row with `change` (given the time now),
    /// then saves and shows it. Does nothing if the group is gone.
    fn edit_group<F>(&mut self, id: GroupId, change: F)
    where
        F: FnOnce(&mut GroupRow, i64),
    {
        let Some(row) = self.groups.iter_mut().find(|row| row.id == id) else {
            return;
        };
        change(row, now_ms());
        let saved = self.store.save_group(row);
        {
            let mut app = self.state.write();
            let sessions = &mut app.sessions;
            if saved.is_err() {
                sessions.error = Some(SAVE_FAILED.to_owned());
            }
            if let Some(shown) = sessions.group_mut(id) {
                *shown = group(row, &self.rows);
            }
        }
        (self.wake)();
    }

    /// Stops each `--bg` session a migrated pane still names, through Claude,
    /// in pane order, ignoring failures (a session already gone is fine),
    /// then forgets them all. Without Claude registered nothing happens and
    /// the markers stay.
    async fn stop_migrated(&mut self) {
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
        // ponytail: stops run in order on the actor (~0.7 s each), once after
        // the migration; run them concurrently if many threads ever migrate.
        for (_, short_id) in &marked {
            let _ = claude.stop(short_id).await;
        }
        let panes: Vec<PaneId> = marked.iter().map(|(pane, _)| *pane).collect();
        match self.store.clear_migrated_bg(&panes) {
            Ok(()) => {
                for pane in &panes {
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

    /// Stops a session in `harness`, showing why if it can't.
    async fn stop(&mut self, harness: &HarnessId, short_id: &str) {
        match self.harness(harness) {
            Ok(harness) => {
                if let Err(report) = harness.stop(short_id).await {
                    self.fail(&report);
                }
            }
            Err(error) => {
                self.state.write().sessions.error = Some(error);
                (self.wake)();
            }
        }
    }

    /// The harness `id` names, or the mode-line text for one orb doesn't know.
    fn harness(&self, id: &HarnessId) -> Result<Arc<dyn Harness>, String> {
        self.services
            .harnesses
            .get(id)
            .cloned()
            .ok_or_else(|| format!("unknown harness {id}"))
    }

    /// Shows a host failure in the mode line.
    fn fail(&self, report: &Report<SessionHostError>) {
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
    /// The status of each pane a record's process runs under.
    matched: HashMap<PaneId, ThreadStatus>,
}

/// The hosted record of session `short_id`: the live one over a stale
/// stopped one (`--all` can list both).
fn hosted_record<'a>(records: &'a [SessionRecord], short_id: &str) -> Option<&'a SessionRecord> {
    records
        .iter()
        .filter(|record| record.short_id.as_deref() == Some(short_id))
        .min_by_key(|record| matches!(record.status, ThreadStatus::Stopped | ThreadStatus::Failed))
}

/// What a saved thread is doing now, if this poll can tell: Gone for a
/// harness orb doesn't know; its hosted `record`'s status; Stopped with no
/// pane; else its pane's status (see [`pane_status`]). `None` (left as it
/// was) when its pane's socket dir couldn't be listed.
fn status_now(
    row: &ThreadRow,
    harness: Option<&Arc<dyn Harness>>,
    record: Option<&SessionRecord>,
    live: &Live,
    reports: &HashMap<PaneId, PaneReport>,
) -> Option<ThreadStatus> {
    let Some(harness) = harness else {
        return Some(ThreadStatus::Gone);
    };
    if let Some(record) = record {
        return Some(record.status);
    }
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

/// Answers the name box that asked for a group, if it's still waiting: closes
/// it once the group is `made`, else leaves it open with its text for another
/// try.
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

/// Brings a saved thread up to date with its record: the session id, the
/// turn stamp, and the title and branch from any new transcript lines, read
/// in its harness's `format`. Returns whether a scan ran and named no
/// branch, which leaves the branch to git.
fn update_row(
    row: &mut ThreadRow,
    record: Option<&SessionRecord>,
    status: ThreadStatus,
    now_ms: i64,
    format: Option<&dyn TranscriptFormat>,
) -> bool {
    if let Some(session_id) = record.and_then(|record| record.session_id.as_ref())
        && row.session_id.as_ref() != Some(session_id)
    {
        // A new session (e.g. after `/clear`) writes a new transcript.
        if row.session_id.is_some() {
            row.transcript_path = None;
            row.transcript_offset = 0;
        }
        row.session_id = Some(session_id.clone());
    }
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

/// The one-line reason a session host failure carries.
fn reason(report: &Report<SessionHostError>) -> String {
    report
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| "session command failed".to_owned())
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
        pinned_at: row.pinned_at.map(from_ms),
        settled_at: row
            .settled_at
            .filter(|_| row.settled_override == Some(SettledOverride::Settled))
            .map(from_ms),
        active_since: from_ms(row.created_at.max(row.unsettled_at.unwrap_or(0))),
        created_at: from_ms(row.created_at),
        last_activity_at: from_ms(row.last_activity_at),
        unseen: row.last_activity_at > row.last_visited_at,
        group: row.group_id,
        model: row.model.clone(),
        permission: row.permission_mode.clone(),
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

/// Whether saved jump-list row `item` still names something, shown or not:
/// one of `sessions`, or in `projects` a project (for a draft) or a group.
fn exists(
    projects: &[Project],
    sessions: &HashMap<SessionId, SessionRow>,
    item: SidebarItem,
) -> bool {
    match item {
        SidebarItem::Session(id) => sessions.contains_key(&id),
        SidebarItem::Draft(id) => projects.iter().any(|project| project.id == id),
        SidebarItem::GroupDraft(id) => projects
            .iter()
            .any(|project| project.groups.iter().any(|group| group.id == id)),
        SidebarItem::SettledShelf => false,
    }
}

/// How a saved group looks: its saved draft, and a draft whenever no saved
/// thread is in it.
fn group(row: &GroupRow, threads: &[ThreadRow]) -> Group {
    Group {
        id: row.id,
        kind: row.kind,
        name: row.name.clone(),
        dir: row.dir.clone(),
        branch: row.branch.clone(),
        created_at: from_ms(row.created_at),
        pinned_at: row.pinned_at.map(from_ms),
        settled_at: row
            .settled_at
            .filter(|_| row.settled_override == Some(SettledOverride::Settled))
            .map(from_ms),
        active_since: from_ms(row.created_at.max(row.unsettled_at.unwrap_or(0))),
        defaults: GroupDefaults {
            harness: row.harness.clone(),
            model: row.draft_model.clone(),
            permission: row.draft_permission_mode.clone(),
        },
        draft: row.draft.clone().or_else(|| {
            (!threads.iter().any(|thread| thread.group_id == Some(row.id)))
                .then(GroupDraft::default)
        }),
    }
}

/// How a saved thread looks before its first poll; one whose harness orb
/// doesn't know is Gone, with nothing to attach.
fn unpolled(services: &Services, row: &ThreadRow, panes: &HashMap<PaneId, PaneRow>) -> Thread {
    let status = match services.harnesses.get(&row.harness) {
        Some(_) => ThreadStatus::Unknown,
        None => ThreadStatus::Gone,
    };
    thread(row, status, pane_launch(services, row, panes))
}

/// `row`'s pane and what runs in it: nothing for a pane with a resume
/// command (it starts as a shell to type that into), else its harness's
/// command, built from the row's model and whether its transcript is known,
/// so a pane that starts the session again starts it right. `None` for a
/// thread with no pane or a harness orb doesn't know.
fn pane_launch(
    services: &Services,
    row: &ThreadRow,
    panes: &HashMap<PaneId, PaneRow>,
) -> Option<PaneLaunch> {
    let harness = services.harnesses.get(&row.harness)?;
    let pane = panes.get(&row.pane_id?)?;
    Some(PaneLaunch {
        pane: pane.id,
        session: pane.session_id,
        command: match pane.resume {
            Some(_) => vec![],
            None => harness.attach_argv(
                &row.short_id,
                &AttachStart {
                    model: row.model.as_deref(),
                    has_transcript: row.transcript_path.is_some(),
                },
            ),
        },
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

/// How project `project_id`'s draft is saved.
fn draft_row(project_id: ProjectId, draft: &Draft) -> DraftRow {
    DraftRow {
        project_id,
        workspace: draft.workspace.clone(),
        branch: draft.branch.clone(),
        model: draft.model.clone(),
        permission_mode: draft.permission.clone(),
        created_at: to_ms(draft.created_at),
        harness: draft.harness.clone(),
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
/// Start, a look at the last fetch for the dashboard), else `base` as is.
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
        harness: row.harness.clone(),
    }
}

/// Shows project `id` in `sessions`: restores it if removed, taking `kind`
/// unless that's [`ProjectKind::Normal`] (as the store keeps a saved kind),
/// or adds it.
fn show_project(
    sessions: &mut Sessions,
    id: ProjectId,
    title: String,
    root: PathBuf,
    kind: ProjectKind,
    now: i64,
) {
    match sessions
        .projects
        .iter_mut()
        .find(|project| project.id == id)
    {
        Some(project) => {
            project.removed = false;
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
            draft: None,
            removed: false,
            kind,
            groups: Vec::new(),
        }),
    }
}

/// orb's own folder for a `kind` group: its project's kind, its folder's
/// name under orb's directory, and its project's title. `None` for a Feature
/// group, which lives in the user's project.
fn own_folder(kind: GroupKind) -> Option<(ProjectKind, &'static str, &'static str)> {
    match kind {
        GroupKind::Feature => None,
        GroupKind::Research => Some((ProjectKind::Research, "research", "Research")),
        GroupKind::Learn => Some((ProjectKind::Learn, "learn", "Learn")),
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
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, OnceLock, PoisonError};
    use std::time::{Duration, SystemTime};

    use async_trait::async_trait;
    use error_stack::{Report, ResultExt};

    use super::{
        FAST_POLL, Probed, SLOW_POLL, SessionsActor, SessionsActorDeps, notice_kind, now_ms,
        settle_session_row, spawn_sessions_actor,
    };
    use crate::Focus;
    use crate::TextInput;
    use crate::command::Workspace;
    use crate::common::{Services, State};
    use crate::feat::git::git_service::{Git, GitError, GitRef, GitService, WorktreeFacts};
    use crate::feat::git::validator::BUSY_DIRECTORY;
    use crate::feat::git::worktree::hex_branch;
    use crate::feat::harness::claude::ClaudeCode;
    use crate::feat::harness::claude::models;
    use crate::feat::harness::claude::transcript::transcript_path;
    use crate::feat::harness::claude::trust::{WorkspaceTrust, WorkspaceTrustError};
    use crate::feat::harness::fake::FakeHarness;
    use crate::feat::harness::{Harness, HarnessInfo, Harnesses};
    use crate::feat::jumps::state::JumpList;
    use crate::feat::layout::tree::Split;
    use crate::feat::sessions::session_host::{
        AttachStart, CreatedSession, SessionHost, SessionHostError, SessionOptions, SessionRecord,
        WorkspaceUntrusted,
    };
    use crate::feat::sessions::state::{
        Draft, DraftWorkspace, Group, GroupDefaults, GroupDraft, GroupId, GroupKind, Notice,
        NoticeKind, Own, PaneId, ProjectId, ProjectKind, SessionId, SidebarItem, Thread, ThreadId,
        ThreadStatus,
    };
    use crate::feat::sessions::store::{
        DraftRow, GroupRow, InsertedThread, LastUsed, LastWorkspace, NewGroup, NewPaneThread,
        NewThread, SessionRow, SettledOverride, Store, StoreError, TabRow, ThreadRow, Ui,
    };
    use crate::feat::sidebar::state::{Rename, RenameTarget};
    use crate::feat::zmx::zmx_service::fake::FakeZmx;
    use crate::feat::zmx::zmx_service::{ZmxOutput, ZmxService, ZmxSession};

    /// The orb project's root, a folder that exists so its drafts can start.
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
    const UNTRUSTED: &str = concat!(
        "Workspace not trusted. Run `claude` in ",
        env!("CARGO_MANIFEST_DIR"),
        " once and accept the trust prompt"
    );
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
        /// Whether each `create`'s directory existed when it was called, in
        /// order.
        cwd_existed: Mutex<Vec<bool>>,
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
                cwd_existed: Mutex::default(),
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

        fn cwd_existed(&self) -> Vec<bool> {
            self.cwd_existed
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
            self.cwd_existed
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(cwd.is_dir());
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

        async fn list(
            &self,
            _short_ids: &[String],
        ) -> Result<Vec<SessionRecord>, Report<SessionHostError>> {
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

        /// `[<short id>]`, plus `--model <m>` until the thread's transcript
        /// is known, as pi's host builds it.
        fn attach_argv(&self, short_id: &str, start: &AttachStart<'_>) -> Vec<OsString> {
            let mut argv = vec![OsString::from(short_id)];
            if let (Some(model), false) = (start.model, start.has_transcript) {
                argv.extend(["--model", model].map(OsString::from));
            }
            argv
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

        /// A repository without an `origin` whose project path is `project`.
        fn in_project(project: &str) -> Arc<Self> {
            Arc::new(Self {
                project: Some(PathBuf::from(project)),
                ..Self::answering()
            })
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

    /// A trust service that records the folders it trusts, and may refuse.
    struct FakeTrust {
        fails: bool,
        /// The folders `trust` was called on, in order.
        trusted: Mutex<Vec<PathBuf>>,
    }

    impl FakeTrust {
        fn accepting() -> Arc<Self> {
            Arc::new(Self {
                fails: false,
                trusted: Mutex::default(),
            })
        }

        fn failing() -> Arc<Self> {
            Arc::new(Self {
                fails: true,
                trusted: Mutex::default(),
            })
        }

        fn trusted(&self) -> Vec<PathBuf> {
            self.trusted
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl WorkspaceTrust for FakeTrust {
        fn name(&self) -> &'static str {
            "fake"
        }

        fn trust(&self, dir: &Path) -> Result<(), Report<WorkspaceTrustError>> {
            self.trusted
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(dir.to_owned());
            if self.fails {
                Err(Report::new(WorkspaceTrustError).attach("config unwritable"))
            } else {
                Ok(())
            }
        }
    }

    /// A zmx with no sessions, on the pane socket dir /zmx.
    fn fake_zmx() -> ZmxService {
        zmx_listing("")
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

    fn record(short_id: &str, status: ThreadStatus) -> SessionRecord {
        SessionRecord {
            short_id: Some(short_id.to_owned()),
            session_id: None,
            status,
            ancestry: Vec::new(),
        }
    }

    /// A store holding one thread in the orb project, created an hour ago.
    fn store_with_thread(short_id: &str) -> Result<(Store, ThreadId), Report<StoreError>> {
        let store = Store::open_in_memory()?;
        let id = add_thread(&store, short_id, now_ms() - HOUR_MS)?;
        Ok((store, id))
    }

    /// A store holding one thread in the orb project started on `model`.
    fn store_with_model_thread(
        short_id: &str,
        model: &str,
    ) -> Result<(Store, ThreadId), Report<StoreError>> {
        let store = Store::open_in_memory()?;
        let project_id = orb_project(&store)?;
        let id = store
            .insert_thread(&NewThread {
                harness: HarnessId::new("claude"),
                project_id,
                short_id: short_id.to_owned(),
                cwd: PathBuf::from(PROJECT_ROOT),
                created_at: now_ms() - HOUR_MS,
                model: Some(model.to_owned()),
                permission_mode: None,
                group_id: None,
                zmx: None,
            })
            .map(|inserted| inserted.thread)?;
        Ok((store, id))
    }

    /// Saves the orb project rooted at [`PROJECT_ROOT`].
    fn orb_project(store: &Store) -> Result<ProjectId, Report<StoreError>> {
        store.add_project(Path::new(PROJECT_ROOT), "orb", ProjectKind::Normal, 0)
    }

    /// Saves a thread created at `created_at` in the orb project.
    fn add_thread(
        store: &Store,
        short_id: &str,
        created_at: i64,
    ) -> Result<ThreadId, Report<StoreError>> {
        let project_id = orb_project(store)?;
        store
            .insert_thread(&NewThread {
                harness: HarnessId::new("claude"),
                project_id,
                short_id: short_id.to_owned(),
                cwd: PathBuf::from(PROJECT_ROOT),
                created_at,
                model: None,
                permission_mode: None,
                group_id: None,
                zmx: None,
            })
            .map(|inserted| inserted.thread)
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
        start_in(
            store,
            host,
            git,
            &FakeTrust::accepting(),
            claude_dir,
            Path::new(ORB_ROOT),
            Path::new(INCOGNITO_ROOT),
        )
    }

    /// Starts the actor on `store` with `git` and `trust`, making worktrees
    /// under [`WORKTREES_ROOT`] and groups' folders under `orb_root`.
    fn start_in(
        store: Store,
        host: &Arc<FakeHost>,
        git: &Arc<FakeGit>,
        trust: &Arc<FakeTrust>,
        claude_dir: &Path,
        orb_root: &Path,
        incognito_root: &Path,
    ) -> (SessionsActor, State) {
        let state = State::default();
        let actor = SessionsActor::restore(SessionsActorDeps {
            services: Services {
                harnesses: Harnesses::new(vec![Arc::new(ClaudeCode::new(
                    host.clone(),
                    trust.clone(),
                    claude_dir.to_owned(),
                    GitService::new(git.clone()),
                ))]),
                git: GitService::new(git.clone()),
                zmx: fake_zmx(),
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

    /// What the actor needs to run Claude over `claude` and [`OTHER`] over
    /// `other`, sharing `state`.
    fn deps_beside(
        store: Store,
        claude: &Arc<FakeHost>,
        other: &Arc<FakeHost>,
        state: &State,
    ) -> SessionsActorDeps {
        let git = FakeGit::local();
        SessionsActorDeps {
            services: Services {
                harnesses: Harnesses::new(vec![
                    Arc::new(ClaudeCode::new(
                        claude.clone(),
                        FakeTrust::accepting(),
                        PathBuf::from(NO_CLAUDE_DIR),
                        GitService::new(git.clone()),
                    )),
                    Arc::new(FakeHarness::new(OTHER, other.clone())),
                ]),
                git: GitService::new(git),
                zmx: fake_zmx(),
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

    /// Starts the actor on `store` with Claude over `claude` and [`OTHER`]
    /// over `other`, the way `on_start` does, without the ticker or probes.
    fn start_beside(
        store: Store,
        claude: &Arc<FakeHost>,
        other: &Arc<FakeHost>,
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
        let project_id = orb_project(store)?;
        store
            .insert_thread(&NewThread {
                harness: HarnessId::new(harness),
                project_id,
                short_id: short_id.to_owned(),
                cwd: PathBuf::from(PROJECT_ROOT),
                created_at: now_ms() - HOUR_MS,
                model: None,
                permission_mode: None,
                group_id: None,
                zmx: None,
            })
            .map(|inserted| inserted.thread)
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn starting_a_draft_creates_its_session_through_the_drafts_harness()
    -> Result<(), Report<StoreError>> {
        // Given a local draft of the other harness.
        let (store, id) = store_with_draft(|id| DraftRow {
            harness: HarnessId::new(OTHER),
            ..draft_row(id, DraftWorkspace::Local)
        })?;
        let claude = FakeHost::creating(Ok("aa"));
        let other = FakeHost::creating(Ok("bb"));
        let (mut actor, _state) = start_beside(store, &claude, &other);

        // When starting the draft.
        actor.start_draft(id).await;

        // Then only the other harness creates a session, in the project's root.
        assert_eq!(
            (claude.created_in(), other.created_in()),
            (Vec::new(), vec![PathBuf::from(PROJECT_ROOT)]),
            "a draft should start in the harness it was drafted for"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn other_harness_thread_shows_the_status_its_host_lists() -> Result<(), Report<StoreError>>
    {
        // Given a thread of the other harness, whose host lists it working.
        let store = Store::open_in_memory()?;
        let id = add_thread_in(&store, OTHER, "bb")?;
        let claude = FakeHost::listing(Vec::new());
        let other = FakeHost::listing(vec![record("bb", ThreadStatus::Working)]);
        let (mut actor, state) = start_beside(store, &claude, &other);

        // When polling.
        actor.poll().await;

        // Then the thread shows working.
        assert_eq!(
            status_of(&state, id),
            Some(ThreadStatus::Working),
            "a thread should show the status its own harness lists"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn other_harness_thread_attaches_with_its_harness_command() -> Result<(), Report<StoreError>> {
        // Given a saved thread `bb` of the other harness.
        let store = Store::open_in_memory()?;
        let id = add_thread_in(&store, OTHER, "bb")?;
        let host = FakeHost::listing(Vec::new());

        // When restoring.
        let (_actor, state) = start_beside(store, &host, &host);

        // Then it attaches with the command its harness builds.
        assert_eq!(
            shown(&state, id)
                .and_then(|thread| thread.pane)
                .map(|launch| launch.command),
            Some(vec![OsString::from(OTHER), OsString::from("bb")]),
            "a thread should attach through its own harness"
        );
        Ok(())
    }

    /// Saves a Claude thread `short_id` in the orb project, created an hour
    /// ago, running in the zmx session `zmx` of its harness's own, if any.
    fn insert_thread(
        store: &Store,
        short_id: &str,
        zmx: Option<ZmxSession>,
    ) -> Result<InsertedThread, Report<StoreError>> {
        let project_id = orb_project(store)?;
        store.insert_thread(&NewThread {
            harness: HarnessId::new("claude"),
            project_id,
            short_id: short_id.to_owned(),
            cwd: PathBuf::from(PROJECT_ROOT),
            created_at: now_ms() - HOUR_MS,
            model: None,
            permission_mode: None,
            group_id: None,
            zmx,
        })
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
        let inserted = insert_thread(&store, "aa", None)?;
        let host = FakeHost::listing(Vec::new());

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
        // Given a thread whose pane keeps zmx session `orb-abc` on dir `pi`.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(
            &store,
            "aa",
            Some(ZmxSession {
                name: "orb-abc".into(),
                dir: PathBuf::from("pi"),
            }),
        )?;
        let host = FakeHost::listing(Vec::new());

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
    fn pane_with_a_resume_command_starts_a_shell() -> Result<(), Report<StoreError>> {
        // Given a thread whose pane has a resume command to type.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let path = dir.path().join("state.sqlite");
        let store = Store::open(&path)?;
        let inserted = insert_thread(&store, "aa", None)?;
        rusqlite::Connection::open(&path)
            .and_then(|conn| {
                conn.execute(
                    "UPDATE panes SET resume = 'claude --resume s-aa' WHERE id = ?1",
                    [inserted.pane.0],
                )
            })
            .change_context(StoreError)?;
        let host = FakeHost::listing(Vec::new());

        // When restoring.
        let (_actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // Then its pane runs no command, so zmx starts a shell.
        assert_eq!(
            shown(&state, inserted.thread)
                .and_then(|thread| thread.pane)
                .map(|launch| launch.command),
            Some(vec![]),
            "a pane orb will type a resume command into starts as a shell"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn restore_loads_each_sessions_tabs_into_layouts() -> Result<(), Report<StoreError>> {
        // Given a thread's session saved with two tabs, the second shown.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa", None)?;
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
        let host = FakeHost::listing(Vec::new());

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
        let inserted = insert_thread(&store, "aa", None)?;
        let host = FakeHost::listing(Vec::new());

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
        let inserted = insert_thread(&store, "aa", None)?;
        let host = FakeHost::listing(Vec::new());
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
        let inserted = insert_thread(&store, "aa", None)?;
        let host = FakeHost::listing(Vec::new());
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
        let inserted = insert_thread(&store, "aa", None)?;
        let host = FakeHost::listing(Vec::new());
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
        let inserted = insert_thread(&store, "aa", None)?;
        let host = FakeHost::listing(Vec::new());
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
        let inserted = insert_thread(&store, "aa", None)?;
        let host = FakeHost::listing(Vec::new());
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
        let inserted = insert_thread(&store, "aa", None)?;
        let host = FakeHost::listing(Vec::new());
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
        let inserted = insert_thread(&store, "aa", None)?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When settling it.
        actor.settle_session(inserted.session).await;

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
        let inserted = insert_thread(&store, "aa", None)?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When settling it.
        actor.settle_session(inserted.session).await;

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
    async fn settle_session_stops_an_idle_hosted_agent() -> Result<(), Report<StoreError>> {
        // Given thread aa's session, its agent idle and hosted by Claude.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa", None)?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When settling it.
        actor.settle_session(inserted.session).await;

        // Then Claude stops aa.
        assert_eq!(
            host.stopped(),
            vec!["aa".to_owned()],
            "an idle hosted agent should be stopped"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn settle_session_kills_every_pane() -> Result<(), Report<StoreError>> {
        // Given thread aa's idle session with a second pane, and thread bb's
        // session.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa", None)?;
        let second = store.insert_pane(inserted.session, Path::new(PROJECT_ROOT))?;
        insert_thread(&store, "bb", None)?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        let (zmx, service) = recording_zmx();
        actor.services.zmx = service;

        // When settling aa's session.
        actor.settle_session(inserted.session).await;

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
        let inserted = insert_thread(&store, "aa", None)?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;
        actor.pin_session(inserted.session);

        // When settling it.
        actor.settle_session(inserted.session).await;

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
    async fn settling_a_pane_agents_session_stops_nothing() -> Result<(), Report<StoreError>> {
        // Given an idle Claude in aa's pane, which no harness runs itself.
        let mut fx = PaneStatus::new(
            false,
            vec![interactive(&[100], ThreadStatus::Idle)],
            running,
        )?;
        fx.actor.poll().await;

        // When settling aa's session.
        fx.actor.settle_session(fx.thread.session).await;

        // Then nothing was asked to stop it.
        assert!(
            fx.host.stopped().is_empty(),
            "a pane agent has no hosted session to stop"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn unsettle_session_keeps_it_active() -> Result<(), Report<StoreError>> {
        // Given thread aa's session, settled.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa", None)?;
        let host = FakeHost::listing(Vec::new());
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
        let inserted = insert_thread(&store, "aa", None)?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, state) = start_unselected(store, &host);
        actor.poll().await;
        host.set_list(Ok(vec![record("aa", ThreadStatus::Idle)]));
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
        let inserted = insert_thread(&store, "aa", None)?;
        let second = store.insert_pane(inserted.session, Path::new(PROJECT_ROOT))?;
        insert_thread(&store, "bb", None)?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        let (zmx, service) = recording_zmx();
        actor.services.zmx = service;

        // When deleting aa's session.
        actor.delete_session(inserted.session).await;

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
        let inserted = insert_thread(&store, "aa", None)?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When deleting it.
        actor.delete_session(inserted.session).await;

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
        let inserted = insert_thread(&store, "aa", None)?;
        resave(&store, "aa", |row| ThreadRow {
            pane_id: None,
            ..row
        })?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When deleting it.
        actor.delete_session(inserted.session).await;

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
        let inserted = insert_thread(&store, "aa", None)?;
        resave(&store, "aa", |row| ThreadRow {
            pane_id: None,
            ..row
        })?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When deleting the session.
        actor.delete_session(inserted.session).await;

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
    ) -> Result<(Store, InsertedThread), Report<StoreError>> {
        let store = Store::open_in_memory()?;
        let project_id = store.add_project(
            &orb_root.join("research"),
            "Research",
            ProjectKind::Research,
            0,
        )?;
        let insert = |short_id: &str| {
            store.insert_thread(&NewThread {
                harness: HarnessId::new("claude"),
                project_id,
                short_id: short_id.to_owned(),
                cwd: dir.to_owned(),
                created_at: now_ms() - HOUR_MS,
                model: None,
                permission_mode: None,
                group_id: None,
                zmx: None,
            })
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
            &FakeHost::listing(Vec::new()),
            &FakeGit::local(),
            &FakeTrust::accepting(),
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
        actor.delete_session(inserted.session).await;

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
        actor.delete_session(inserted.session).await;

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
        actor.delete_session(inserted.session).await;

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
        actor.delete_session(inserted.session).await;

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
        fx.actor.delete_session(fx.thread.session).await;

        // Then the pane's file is gone.
        assert!(
            !fx.file(fx.thread.pane).exists(),
            "a deleted session's pane files should go"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn deleting_a_pane_agents_session_skips_remove() -> Result<(), Report<StoreError>> {
        // Given an idle Claude in aa's pane, which no harness runs itself.
        let mut fx = PaneStatus::new(
            false,
            vec![interactive(&[100], ThreadStatus::Idle)],
            running,
        )?;
        fx.actor.poll().await;

        // When deleting aa's session.
        fx.actor.delete_session(fx.thread.session).await;

        // Then nothing was asked to remove it.
        assert!(
            fx.host.removed().is_empty(),
            "a pane agent has no hosted session to remove"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_remove_keeps_the_session() -> Result<(), Report<StoreError>> {
        // Given thread aa's session, whose hosted agent can't be removed.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa", None)?;
        let host = FakeHost::refusing_remove(vec![record("aa", ThreadStatus::Idle)], "rm: busy");
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;
        state.write().sessions.deleting.insert(inserted.session);

        // When deleting it.
        actor.delete_session(inserted.session).await;

        // Then the sidebar lists it again.
        let listed = state
            .read()
            .sessions
            .sidebar()
            .iter()
            .any(|row| row.item() == SidebarItem::Session(inserted.session));
        assert!(listed, "a session whose delete failed should come back");
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_remove_shows_the_reason() -> Result<(), Report<StoreError>> {
        // Given thread aa's session, whose hosted agent can't be removed.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa", None)?;
        let host = FakeHost::refusing_remove(vec![record("aa", ThreadStatus::Idle)], "rm: busy");
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When deleting it.
        actor.delete_session(inserted.session).await;

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
    async fn delete_session_unhides_it() -> Result<(), Report<StoreError>> {
        // Given thread aa's session, hidden as being deleted.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa", None)?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        state.write().sessions.deleting.insert(inserted.session);

        // When deleting it.
        actor.delete_session(inserted.session).await;

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
        let inserted = insert_thread(&store, "aa", None)?;
        store.save_jumps(&[SidebarItem::Session(inserted.session)])?;

        // When the actor starts.
        let (_actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

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
        let host = FakeHost::listing(Vec::new());
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
        let inserted = insert_thread(&store, "aa", None)?;
        let host = FakeHost::listing(Vec::new());
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
        let inserted = insert_thread(&store, "aa", None)?;
        let host = FakeHost::listing(Vec::new());
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

    #[rstest::rstest]
    fn save_layout_writes_the_sessions_tabs() -> Result<(), Report<StoreError>> {
        // Given a thread's session whose tab was renamed in the app state.
        let store = Store::open_in_memory()?;
        let inserted = insert_thread(&store, "aa", None)?;
        let host = FakeHost::listing(Vec::new());
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
            let pane = insert_thread(&store, short_id, None)?.pane;
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

    #[rstest::rstest]
    #[tokio::test]
    async fn stop_migrated_stops_each_marked_bg_session() -> Result<(), Report<StoreError>> {
        // Given panes still naming --bg sessions aa and bb.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let store = migrated_store(&dir.path().join("state.sqlite"))?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When stopping the migrated sessions.
        actor.stop_migrated().await;

        // Then Claude stopped aa and then bb.
        assert_eq!(
            host.stopped(),
            vec!["aa".to_owned(), "bb".to_owned()],
            "every marked --bg session should be stopped in pane order"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn stop_migrated_clears_the_markers() -> Result<(), Report<StoreError>> {
        // Given panes still naming --bg sessions aa and bb.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let store = migrated_store(&dir.path().join("state.sqlite"))?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When stopping the migrated sessions.
        actor.stop_migrated().await;

        // Then no pane names a --bg session any more.
        let marked: Vec<String> = actor
            .store
            .layouts()?
            .panes
            .into_iter()
            .filter_map(|pane| pane.migrated_bg)
            .collect();
        assert!(
            marked.is_empty(),
            "stopped sessions should be forgotten: {marked:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn thread_of_an_unknown_harness_shows_gone_with_no_attach_command()
    -> Result<(), Report<StoreError>> {
        // Given a saved thread of a harness orb doesn't know.
        let store = Store::open_in_memory()?;
        let id = add_thread_in(&store, "gone-harness", "aa")?;
        let host = FakeHost::listing(Vec::new());

        // When restoring.
        let (_actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // Then it shows gone and can't be attached.
        assert_eq!(
            shown(&state, id).map(|thread| (thread.status, thread.pane.is_none())),
            Some((ThreadStatus::Gone, true)),
            "a thread of an unknown harness should be gone with no attach command"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn thread_attach_argv_passes_the_model_before_its_transcript_is_found()
    -> Result<(), Report<StoreError>> {
        // Given a saved thread `aa` started on sonnet, with no transcript found.
        let (store, id) = store_with_model_thread("aa", "sonnet")?;
        let host = FakeHost::listing(Vec::new());

        // When restoring.
        let (_actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // Then its attach command carries the model.
        assert_eq!(
            shown(&state, id)
                .and_then(|thread| thread.pane)
                .map(|launch| launch.command),
            Some(vec![
                OsString::from("aa"),
                OsString::from("--model"),
                OsString::from("sonnet"),
            ]),
            "an attach before the transcript is found should start on the thread's model"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn thread_attach_argv_drops_the_model_once_its_transcript_is_found()
    -> Result<(), Report<StoreError>> {
        // Given a thread `aa` started on sonnet whose session has a transcript.
        let claude_dir = tempfile::tempdir().change_context(StoreError)?;
        let path = transcript_path(claude_dir.path(), Path::new(PROJECT_ROOT), "s1");
        fs::create_dir_all(path.parent().unwrap_or(claude_dir.path()))
            .change_context(StoreError)?;
        fs::write(&path, "").change_context(StoreError)?;
        let (store, id) = store_with_model_thread("aa", "sonnet")?;
        let host = FakeHost::listing(vec![SessionRecord {
            session_id: Some("s1".to_owned()),
            ..record("aa", ThreadStatus::Idle)
        }]);
        let (mut actor, state) = start(store, &host, claude_dir.path());

        // When polling finds the transcript.
        actor.poll().await;

        // Then its attach command no longer carries the model.
        assert_eq!(
            shown(&state, id)
                .and_then(|thread| thread.pane)
                .map(|launch| launch.command),
            Some(vec![OsString::from("aa")]),
            "an attach once the transcript is found should resume without the model"
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
        let claude = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let other = FakeHost::listing(vec![record("bb", ThreadStatus::Idle)]);
        let (mut actor, state) = start_beside(store, &claude, &other);
        actor.poll().await;
        other.set_list(Err("supervisor down".to_owned()));
        claude.set_list(Ok(vec![record("aa", ThreadStatus::Working)]));
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
    fn restore_publishes_a_placeholder_per_harness() -> Result<(), Report<StoreError>> {
        // Given Claude and the other harness.
        let claude = FakeHost::listing(Vec::new());
        let other = FakeHost::listing(Vec::new());

        // When restoring.
        let (_actor, state) = start_beside(Store::open_in_memory()?, &claude, &other);

        // Then each harness shows a placeholder, in registration order.
        assert_eq!(
            state.read().harnesses,
            vec![
                HarnessInfo::placeholder(HarnessId::new("claude"), "Claude Code"),
                HarnessInfo::placeholder(HarnessId::new(OTHER), OTHER),
            ],
            "restore should publish a placeholder per harness before any probe answers"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn restore_without_integrations_shows_the_install_nudge() -> Result<(), Report<StoreError>> {
        // Given neither orb's Claude hook nor its pi extension is installed.
        let claude = FakeHost::listing(Vec::new());
        let other = FakeHost::listing(Vec::new());
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

    #[rstest::rstest]
    #[tokio::test]
    async fn probed_info_replaces_its_placeholder() -> Result<(), Report<StoreError>> {
        // Given a running actor with Claude and the other harness.
        let state = State::default();
        let claude = FakeHost::listing(Vec::new());
        let other = FakeHost::listing(Vec::new());
        let actor_ref = spawn_sessions_actor(deps_beside(
            Store::open_in_memory()?,
            &claude,
            &other,
            &state,
        ));

        // When it handles Claude's probe.
        actor_ref
            .ask(Probed(models::info()))
            .await
            .map_err(|error| Report::new(StoreError).attach(error.to_string()))?;

        // Then Claude's entry is what the probe found.
        assert_eq!(
            state
                .read()
                .harness_info(&HarnessId::new("claude"))
                .cloned(),
            Some(models::info()),
            "a probe's info should replace its harness's placeholder"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn probe_notice_shows_on_the_mode_line() -> Result<(), Report<StoreError>> {
        // Given a running actor with Claude and the other harness.
        let state = State::default();
        let claude = FakeHost::listing(Vec::new());
        let other = FakeHost::listing(Vec::new());
        let actor_ref = spawn_sessions_actor(deps_beside(
            Store::open_in_memory()?,
            &claude,
            &other,
            &state,
        ));

        // When it handles a probe that came back with a notice.
        let notice = "pi --list-models failed: exit code 1";
        actor_ref
            .ask(Probed(HarnessInfo {
                notice: Some(notice.to_owned()),
                ..models::info()
            }))
            .await
            .map_err(|error| Report::new(StoreError).attach(error.to_string()))?;

        // Then the notice is on the mode line.
        assert_eq!(
            error_of(&state).as_deref(),
            Some(notice),
            "a probe's notice should show on the mode line"
        );
        Ok(())
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
    async fn poll_marks_a_claude_thread_without_a_record_stopped() -> Result<(), Report<StoreError>>
    {
        // Given a saved thread the host doesn't list, its pane's zmx session gone.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("bb", ThreadStatus::Idle)]);
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
    async fn poll_waits_five_seconds_while_no_turn_is_underway() -> Result<(), Report<StoreError>> {
        // Given an idle thread and orb attached.
        let (store, id) = store_with_thread("aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
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
            harness: HarnessId::new("claude"),
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
            harness: Some(HarnessId::new("claude")),
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
        let web = store.add_project(Path::new(WEB_ROOT), "web", ProjectKind::Normal, 0)?;
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

    /// Settings used last in the local checkout with `harness`'s default
    /// model and permission.
    fn used_in(harness: &str) -> LastUsed {
        LastUsed {
            harness: Some(HarnessId::new(harness)),
            workspace: LastWorkspace::Local,
            model: None,
            permission_mode: None,
        }
    }

    /// The harness of project `id`'s draft as the sidebar shows it.
    fn draft_harness(state: &State, id: ProjectId) -> Option<HarnessId> {
        shown_draft(state, id).map(|draft| draft.harness)
    }

    #[rstest::rstest]
    fn new_draft_takes_the_projects_last_used_harness() -> Result<(), Report<StoreError>> {
        // Given orb last started in the other harness, and web started in
        // Claude after that.
        let store = Store::open_in_memory()?;
        let orb = orb_project(&store)?;
        store.record_last_used(orb, &used_in(OTHER), 10)?;
        let web = store.add_project(Path::new(WEB_ROOT), "web", ProjectKind::Normal, 0)?;
        store.record_last_used(web, &used_in("claude"), 20)?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, state) = start_beside(store, &host, &host);

        // When creating orb's draft.
        actor.create_draft(orb);

        // Then it runs the other harness.
        assert_eq!(
            draft_harness(&state, orb),
            Some(HarnessId::new(OTHER)),
            "a draft should take its project's last-used harness"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_draft_takes_the_latest_used_harness_without_its_own() -> Result<(), Report<StoreError>> {
        // Given orb last started in the other harness, and web never started.
        let store = Store::open_in_memory()?;
        let orb = orb_project(&store)?;
        store.record_last_used(orb, &used_in(OTHER), 10)?;
        let web = store.add_project(Path::new(WEB_ROOT), "web", ProjectKind::Normal, 0)?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, state) = start_beside(store, &host, &host);

        // When creating web's draft.
        actor.create_draft(web);

        // Then it runs the other harness too.
        assert_eq!(
            draft_harness(&state, web),
            Some(HarnessId::new(OTHER)),
            "a project never started should take the latest used harness"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_draft_takes_the_default_harness_when_nothing_was_used() -> Result<(), Report<StoreError>>
    {
        // Given a project nothing was ever started in, with the other
        // harness registered after Claude.
        let store = Store::open_in_memory()?;
        let orb = orb_project(&store)?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, state) = start_beside(store, &host, &host);

        // When creating its draft.
        actor.create_draft(orb);

        // Then it runs the first registered harness.
        assert_eq!(
            draft_harness(&state, orb),
            Some(HarnessId::new("claude")),
            "with nothing used, a draft should take the default harness"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn unregistered_last_used_harness_gives_the_default_harness_and_model()
    -> Result<(), Report<StoreError>> {
        // Given a project last started on sonnet in a harness orb no longer
        // has.
        let store = Store::open_in_memory()?;
        let orb = orb_project(&store)?;
        store.record_last_used(
            orb,
            &LastUsed {
                model: Some("sonnet".to_owned()),
                ..used_in("gone-harness")
            },
            10,
        )?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, state) = start_beside(store, &host, &host);

        // When creating its draft.
        actor.create_draft(orb);

        // Then it runs the default harness on its default model.
        assert_eq!(
            shown_draft(&state, orb).map(|draft| (draft.harness, draft.model)),
            Some((HarnessId::new("claude"), None)),
            "a gone harness's draft should fall back to the default harness and model"
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
        let web = store.add_project(Path::new(WEB_ROOT), "web", ProjectKind::Normal, 0)?;
        store.save_draft(&draft_row(web, DraftWorkspace::Local))?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When starting web's draft.
        actor.start_draft(web).await;

        // Then the host starts it in web's root.
        assert_eq!(
            host.created_in(),
            vec![PathBuf::from(WEB_ROOT)],
            "a local draft should start in its project's root"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn local_start_in_a_missing_project_folder_shows_why() -> Result<(), Report<StoreError>> {
        // Given a local draft of a project whose folder no longer exists.
        let (store, id) = {
            let store = Store::open_in_memory()?;
            let id = store.add_project(
                Path::new("/nonexistent/gone"),
                "gone",
                ProjectKind::Normal,
                0,
            )?;
            store.save_draft(&draft_row(id, DraftWorkspace::Local))?;
            (store, id)
        };
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When starting the draft.
        actor.start_draft(id).await;

        // Then the mode line says the project folder is gone.
        assert_eq!(
            error_of(&state).as_deref(),
            Some("project folder no longer exists: /nonexistent/gone"),
            "a missing project folder should fail the start with its path"
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
        let web = store.add_project(Path::new(WEB_ROOT), "web", ProjectKind::Normal, 0)?;
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
            state
                .read()
                .sessions
                .selected_thread()
                .map(|thread| thread.id),
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

        // Then the frontend is asked to attach to the new thread's session.
        assert_eq!(
            state.read().sessions.attach,
            Some(session_of(&actor.store, saved(&actor.store, "bb")?.id)?),
            "a still-selected draft's thread should be attached to"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn started_draft_thread_gets_its_own_session() -> Result<(), Report<StoreError>> {
        // Given a local draft, selected.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        state.write().sessions.cursor = Some(SidebarItem::Draft(id));

        // When starting it.
        actor.start_draft(id).await;

        // Then the new thread's pane is laid out in a session of its own.
        let thread = saved(&actor.store, "bb")?;
        let laid_out = thread
            .pane_id
            .and_then(|pane| state.read().layouts.owner_of(pane))
            .map(|session| state.read().layouts.session_panes(session));
        assert_eq!(
            laid_out,
            Some(thread.pane_id.into_iter().collect()),
            "a started thread should get a one-pane session"
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
        state.write().sessions.cursor = Some(SidebarItem::Session(SessionId(thread.0)));

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
        state.write().sessions.cursor = Some(SidebarItem::Session(SessionId(thread.0)));

        // When starting the draft.
        actor.start_draft(id).await;

        // Then the old thread stays selected.
        assert_eq!(
            state.read().sessions.cursor,
            Some(SidebarItem::Session(SessionId(thread.0))),
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

    /// The directory of Claude's transcripts, holding session `s1` of thread
    /// `aa` with `lines`, and a store where `aa` was given `custom_title`
    /// by `/rename` and `renamed` with `r`.
    fn renamed_thread(
        lines: &str,
        custom_title: &str,
        renamed: &str,
    ) -> Result<(tempfile::TempDir, Store, ThreadId), Report<StoreError>> {
        let claude_dir = tempfile::tempdir().change_context(StoreError)?;
        let path = transcript_path(claude_dir.path(), Path::new(PROJECT_ROOT), "s1");
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
        let host = FakeHost::listing(vec![in_session(ThreadStatus::Idle)]);
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
        let host = FakeHost::listing(vec![in_session(ThreadStatus::Idle)]);
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
        let host = FakeHost::listing(Vec::new());

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
        let host = FakeHost::listing(Vec::new());
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
        let b = store.add_project(Path::new("/b"), "b", ProjectKind::Normal, 2)?;
        let a = store.add_project(Path::new("/a"), "a", ProjectKind::Normal, 1)?;
        let insert = |project_id, short_id: &str, created_at| {
            store
                .insert_thread(&NewThread {
                    harness: HarnessId::new("claude"),
                    project_id,
                    short_id: short_id.to_owned(),
                    cwd: PathBuf::from("/a"),
                    created_at,
                    model: None,
                    permission_mode: None,
                    group_id: None,
                    zmx: None,
                })
                .map(|inserted| inserted.thread)
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
        // Given an old pinned session and a newer unpinned one.
        let store = Store::open_in_memory()?;
        let pinned = add_thread(&store, "old", 10)?;
        add_thread(&store, "new", 20)?;
        store.save_session(&SessionRow {
            pinned_at: Some(30),
            ..saved_session(&store, "old")?
        })?;

        // When the actor starts.
        let (_actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

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
        let (_actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

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
        let (actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );
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

    /// Starts the actor on `store` with `incognito_root` as orb's Incognito
    /// folder.
    fn start_incognito(
        store: Store,
        host: &Arc<FakeHost>,
        incognito_root: &Path,
    ) -> (SessionsActor, State) {
        start_in(
            store,
            host,
            &FakeGit::local(),
            &FakeTrust::accepting(),
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
        let (_actor, state) = start_incognito(
            Store::open_in_memory()?,
            &FakeHost::listing(Vec::new()),
            &root,
        );

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
        let (_actor, state) = start_incognito(store, &FakeHost::listing(Vec::new()), &root);

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
        let (_actor, _state) = start_incognito(
            Store::open_in_memory()?,
            &FakeHost::listing(Vec::new()),
            &root,
        );

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
        let (_actor, state) = start_incognito(store, &FakeHost::listing(Vec::new()), &root);

        // Then it isn't removed.
        assert_eq!(
            removed_of(&state, id),
            Some(false),
            "start should bring back a removed Incognito project"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn incognito_start_makes_its_folder_first() -> Result<(), Report<StoreError>> {
        // Given the Incognito project with a local draft, its folder deleted
        // after start.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let root = dir.path().join("incognito");
        let store = Store::open_in_memory()?;
        let id = store.add_project(&root, "Incognito", ProjectKind::Incognito, 0)?;
        store.save_draft(&draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, _state) = start_incognito(store, &host, &root);
        fs::remove_dir_all(&root).change_context(StoreError)?;

        // When starting the draft.
        actor.start_draft(id).await;

        // Then the folder existed when the host started the session.
        assert_eq!(
            host.cwd_existed(),
            vec![true],
            "an Incognito start should make its folder before starting"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn incognito_start_fails_when_its_folder_cant_be_made() -> Result<(), Report<StoreError>>
    {
        // Given the Incognito project with a local draft, rooted under a
        // regular file.
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let file = dir.path().join("file");
        fs::write(&file, "").change_context(StoreError)?;
        let root = file.join("incognito");
        let store = Store::open_in_memory()?;
        let id = store.add_project(&root, "Incognito", ProjectKind::Incognito, 0)?;
        store.save_draft(&draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, state) = start_incognito(store, &host, &root);

        // When starting the draft.
        actor.start_draft(id).await;

        // Then the mode line says the folder couldn't be made.
        assert_eq!(
            error_of(&state).as_deref(),
            Some("couldn't make the folder"),
            "an Incognito start should fail when its folder can't be made"
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
        let web = store.add_project(Path::new(WEB_ROOT), "web", ProjectKind::Normal, 0)?;
        let thread = store
            .insert_thread(&NewThread {
                harness: HarnessId::new("claude"),
                project_id: web,
                short_id: "bb".to_owned(),
                cwd: PathBuf::from(WEB_ROOT),
                created_at: 500,
                model: None,
                permission_mode: None,
                group_id: None,
                zmx: None,
            })
            .map(|inserted| inserted.thread)?;
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
        store.add_project(Path::new(PROJECT_ROOT), "orb", ProjectKind::Normal, 1_500)?;

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
        store
            .insert_thread(&NewThread {
                harness: HarnessId::new("claude"),
                project_id,
                short_id: "t1".to_owned(),
                cwd: PathBuf::from(PROJECT_ROOT),
                created_at: 1_500,
                model: None,
                permission_mode: None,
                group_id: None,
                zmx: None,
            })
            .map(|inserted| inserted.thread)?;

        // When the actor starts.
        let (_actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

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

    #[rstest::rstest]
    fn restore_gives_a_group_without_threads_a_draft() -> Result<(), Report<StoreError>> {
        // Given a project with a saved Feature group in opus and no threads.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new(PROJECT_ROOT), "orb", ProjectKind::Normal, 1_500)?;
        store.insert_group(&NewGroup {
            harness: HarnessId::new("claude"),
            project_id,
            kind: GroupKind::Feature,
            name: "GT-514-login".to_owned(),
            dir: None,
            branch: Some("GT-514-login".to_owned()),
            created_at: 2_000,
            draft_model: Some("opus".to_owned()),
            draft_permission_mode: None,
        })?;

        // When the actor starts.
        let (_actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // Then the group shows a draft with its saved settings.
        let drafts: Vec<(bool, GroupDefaults)> = state
            .read()
            .sessions
            .projects
            .iter()
            .flat_map(|project| {
                project
                    .groups
                    .iter()
                    .map(|group| (group.draft.is_some(), group.defaults.clone()))
            })
            .collect();
        assert_eq!(
            drafts,
            vec![(
                true,
                GroupDefaults {
                    harness: HarnessId::new("claude"),
                    model: Some("opus".to_owned()),
                    permission: None,
                }
            )],
            "a group without threads should be restored with its draft"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn restore_puts_a_saved_draft_on_its_project() -> Result<(), Report<StoreError>> {
        // Given a project with a saved new-worktree draft on main, in opus,
        // created at 2 s.
        let store = Store::open_in_memory()?;
        let project_id =
            store.add_project(Path::new(PROJECT_ROOT), "orb", ProjectKind::Normal, 1_500)?;
        store.save_draft(&DraftRow {
            harness: HarnessId::new("claude"),
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
            .filter(|project| project.kind == ProjectKind::Normal)
            .map(|project| project.draft.clone())
            .collect();
        assert_eq!(
            drafts,
            vec![Some(Draft {
                harness: HarnessId::new("claude"),
                workspace: DraftWorkspace::NewWorktree,
                branch: Some("main".to_owned()),
                model: Some("opus".to_owned()),
                permission: None,
                created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(2),
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
        state.write().sessions.cursor = Some(SidebarItem::Session(SessionId(other.0)));
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
        state.write().sessions.cursor = Some(SidebarItem::Session(SessionId(id.0)));
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
    async fn turn_end_without_a_scanned_branch_takes_the_branch_from_git()
    -> Result<(), Report<StoreError>> {
        // Given a branchless thread a poll saw working, whose transcript names no branch.
        let claude_dir = tempfile::tempdir().change_context(StoreError)?;
        let path = transcript_path(claude_dir.path(), Path::new(PROJECT_ROOT), "s1");
        fs::create_dir_all(path.parent().unwrap_or(claude_dir.path()))
            .change_context(StoreError)?;
        fs::write(&path, PROMPT_LINE).change_context(StoreError)?;
        let (store, _) = store_with_thread("aa")?;
        let working = SessionRecord {
            session_id: Some("s1".to_owned()),
            ..record("aa", ThreadStatus::Working)
        };
        let host = FakeHost::listing(vec![working.clone()]);
        let (mut actor, _state) = start(store, &host, claude_dir.path());
        actor.poll().await;

        // When a poll sees its turn end.
        host.set_list(Ok(vec![SessionRecord {
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
        let orb_worktrees =
            Path::new(WORKTREES_ROOT).join(Path::new(PROJECT_ROOT).file_name().unwrap_or_default());
        let in_worktree = created.first().is_some_and(|cwd| {
            cwd.parent() == Some(orb_worktrees.as_path())
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
    async fn new_worktree_start_shows_the_fetch_while_it_runs() -> Result<(), Report<StoreError>> {
        // Given a new-worktree draft on main, an origin, and git watching the app state.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::NewWorktree))?;
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::with_origin(Ok(true)));
        let (mut actor, state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));
        git.watch(&state);

        // When starting the draft.
        actor.start_draft(id).await;

        // Then, while git fetched, the app state showed origin/main being fetched.
        assert_eq!(
            git.fetching_seen(),
            vec![Some("origin/main".to_owned())],
            "the fetch should show while it runs"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn start_clears_the_fetch_once_it_finishes() -> Result<(), Report<StoreError>> {
        // Given a new-worktree draft on main and an origin that answers.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::NewWorktree))?;
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::with_origin(Ok(true)));
        let (mut actor, state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When starting the draft.
        actor.start_draft(id).await;

        // Then no fetch shows any more.
        assert_eq!(
            state.read().sessions.fetching,
            None,
            "a finished fetch shouldn't keep showing"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_fetch_clears_the_fetch() -> Result<(), Report<StoreError>> {
        // Given a new-worktree draft on main and an origin that times out.
        let (store, id) = store_with_draft(|id| draft_row(id, DraftWorkspace::NewWorktree))?;
        let host = FakeHost::creating(Ok("bb"));
        let git = FakeGit::with_origin(Err("git fetch origin main timed out after 15 s"));
        let (mut actor, state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When starting the draft.
        actor.start_draft(id).await;

        // Then no fetch shows any more.
        assert_eq!(
            state.read().sessions.fetching,
            None,
            "a failed fetch shouldn't keep showing"
        );
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
        let id = store
            .insert_thread(&NewThread {
                harness: HarnessId::new("claude"),
                project_id: orb_project(&store)?,
                short_id: "aa".to_owned(),
                cwd: PathBuf::from(PROJECT_ROOT),
                created_at: now_ms() - HOUR_MS,
                model: Some("sonnet".to_owned()),
                permission_mode: Some("plan".to_owned()),
                group_id: None,
                zmx: None,
            })
            .map(|inserted| inserted.thread)?;
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
    async fn untrusted_draft_start_asks_for_trust_in_the_project_path()
    -> Result<(), Report<StoreError>> {
        // Given a local draft whose project path is `/tmp/main`, and claude
        // refusing it as untrusted.
        let (store, project) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let (host, git) = (
            FakeHost::untrusted(1, Ok("bb")),
            FakeGit::in_project("/tmp/main"),
        );
        let (mut actor, state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When starting the draft.
        actor.start_draft(project).await;

        // Then the start waits for the project path to be trusted.
        assert_eq!(
            trust_of(&state),
            Some(PathBuf::from("/tmp/main")),
            "claude's project path should be offered for trust"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn untrusted_start_without_a_project_path_asks_for_trust_in_its_directory()
    -> Result<(), Report<StoreError>> {
        // Given a local draft with no project path, and claude refusing its
        // untrusted root.
        let (store, project) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let (host, git) = (FakeHost::untrusted(1, Ok("bb")), FakeGit::local());
        let (mut actor, state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When starting the draft.
        actor.start_draft(project).await;

        // Then the start waits for its own directory to be trusted.
        assert_eq!(
            trust_of(&state),
            Some(PathBuf::from(PROJECT_ROOT)),
            "the start's directory should be offered for trust"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn untrusted_worktree_asks_for_trust_in_the_main_repo() -> Result<(), Report<StoreError>>
    {
        // Given claude refusing a new, untrusted worktree of the project.
        let (store, id) = store_with_thread("aa")?;
        let (host, git) = (
            FakeHost::untrusted(1, Ok("bb")),
            FakeGit::in_project(PROJECT_ROOT),
        );
        let (mut actor, state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));

        // When moving a thread to a new worktree.
        actor.move_thread(id, Workspace::NewWorktree).await;

        // Then the start waits for the main repository to be trusted.
        assert_eq!(
            trust_of(&state),
            Some(PathBuf::from(PROJECT_ROOT)),
            "the worktree's main repository should be offered for trust"
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
    async fn poll_does_not_retry_a_start_waiting_for_trust() -> Result<(), Report<StoreError>> {
        // Given a draft start waiting for trust.
        let (store, project) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::untrusted(1, Ok("bb"));
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.start_draft(project).await;

        // When the next poll runs.
        actor.poll().await;

        // Then no second session start is tried.
        assert_eq!(
            host.created_in().len(),
            1,
            "only the user's answer should retry the start"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn poll_waits_five_seconds_while_a_start_waits_for_trust()
    -> Result<(), Report<StoreError>> {
        // Given a draft start waiting for trust.
        let (store, project) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::untrusted(1, Ok("bb"));
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.start_draft(project).await;

        // When polling.
        let next = actor.poll().await;

        // Then the next poll comes at the idle pace.
        assert_eq!(
            next, SLOW_POLL,
            "a pending trust shouldn't speed up polling"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn trusting_the_workspace_saves_trust_for_the_project_path()
    -> Result<(), Report<StoreError>> {
        // Given a local draft start waiting for `/tmp/main` to be trusted.
        let (store, project) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let (host, git, trust) = (
            FakeHost::untrusted(1, Ok("bb")),
            FakeGit::in_project("/tmp/main"),
            FakeTrust::accepting(),
        );
        let (mut actor, _state) = start_in(
            store,
            &host,
            &git,
            &trust,
            Path::new(NO_CLAUDE_DIR),
            Path::new(ORB_ROOT),
            Path::new(INCOGNITO_ROOT),
        );
        actor.start_draft(project).await;

        // When the user trusts the workspace.
        actor.trust_workspace().await;

        // Then the project path is marked trusted.
        assert_eq!(
            trust.trusted(),
            vec![PathBuf::from("/tmp/main")],
            "claude's project path should be saved as trusted"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn trusting_the_workspace_starts_the_thread() -> Result<(), Report<StoreError>> {
        // Given a draft start waiting for trust, which claude accepts next.
        let (store, project) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::untrusted(1, Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.start_draft(project).await;

        // When the user trusts the workspace.
        actor.trust_workspace().await;

        // Then the new thread shows.
        assert_eq!(
            state.read().sessions.threads().count(),
            1,
            "a trusted start should add the thread"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn trusted_start_refused_again_shows_the_error() -> Result<(), Report<StoreError>> {
        // Given a draft start waiting for trust, which claude refuses again.
        let (store, project) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::untrusted(2, Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.start_draft(project).await;

        // When the user trusts the workspace.
        actor.trust_workspace().await;

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
    async fn trusted_start_refused_again_removes_the_new_worktree() -> Result<(), Report<StoreError>>
    {
        // Given a move to a new worktree waiting for trust, which claude
        // refuses again.
        let (store, id) = store_with_thread("aa")?;
        let (host, git) = (FakeHost::untrusted(2, Ok("bb")), FakeGit::local());
        let (mut actor, _state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));
        actor.move_thread(id, Workspace::NewWorktree).await;

        // When the user trusts the workspace.
        actor.trust_workspace().await;

        // Then the worktree orb made is force-removed.
        let (path, _branch) = git
            .added()
            .ok_or_else(|| Report::new(StoreError).attach("no worktree was added"))?;
        assert!(
            git.calls()
                .contains(&GitCall::RemoveWorktree { path, force: true }),
            "a refused retry should remove the worktree made for it"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn trusted_start_refused_again_does_not_ask_for_trust() -> Result<(), Report<StoreError>>
    {
        // Given a draft start waiting for trust, which claude refuses again.
        let (store, project) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::untrusted(2, Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.start_draft(project).await;

        // When the user trusts the workspace.
        actor.trust_workspace().await;

        // Then no trust is asked for again.
        assert_eq!(
            trust_of(&state),
            None,
            "a retry should never ask for trust again"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_trust_save_shows_why() -> Result<(), Report<StoreError>> {
        // Given a draft start waiting for trust, and Claude's config unwritable.
        let (store, project) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::untrusted(1, Ok("bb"));
        let (mut actor, state) = start_in(
            store,
            &host,
            &FakeGit::local(),
            &FakeTrust::failing(),
            Path::new(NO_CLAUDE_DIR),
            Path::new(ORB_ROOT),
            Path::new(INCOGNITO_ROOT),
        );
        actor.start_draft(project).await;

        // When the user trusts the workspace.
        actor.trust_workspace().await;

        // Then the start fails, saying the folder couldn't be trusted.
        assert_eq!(
            error_of(&state).as_deref(),
            Some("couldn't trust the folder"),
            "an unsaved trust should fail the start"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_trust_save_removes_the_new_worktree() -> Result<(), Report<StoreError>> {
        // Given a move to a new worktree waiting for trust, and Claude's
        // config unwritable.
        let (store, id) = store_with_thread("aa")?;
        let (host, git) = (FakeHost::untrusted(1, Ok("bb")), FakeGit::local());
        let (mut actor, _state) = start_in(
            store,
            &host,
            &git,
            &FakeTrust::failing(),
            Path::new(NO_CLAUDE_DIR),
            Path::new(ORB_ROOT),
            Path::new(INCOGNITO_ROOT),
        );
        actor.move_thread(id, Workspace::NewWorktree).await;

        // When the user trusts the workspace.
        actor.trust_workspace().await;

        // Then the worktree orb made is force-removed.
        let (path, _branch) = git
            .added()
            .ok_or_else(|| Report::new(StoreError).attach("no worktree was added"))?;
        assert!(
            git.calls()
                .contains(&GitCall::RemoveWorktree { path, force: true }),
            "an unsaved trust should remove the worktree made for the start"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn trusting_the_workspace_without_a_waiting_start_saves_nothing()
    -> Result<(), Report<StoreError>> {
        // Given no start waiting for trust.
        let (store, _project) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let trust = FakeTrust::accepting();
        let (mut actor, _state) = start_in(
            store,
            &FakeHost::listing(Vec::new()),
            &FakeGit::local(),
            &trust,
            Path::new(NO_CLAUDE_DIR),
            Path::new(ORB_ROOT),
            Path::new(INCOGNITO_ROOT),
        );

        // When the user trusts the workspace.
        actor.trust_workspace().await;

        // Then nothing is marked trusted.
        assert!(
            trust.trusted().is_empty(),
            "trust with nothing waiting should save nothing"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn declining_trust_removes_the_new_worktree() -> Result<(), Report<StoreError>> {
        // Given a move to a new worktree waiting for trust.
        let (store, id) = store_with_thread("aa")?;
        let (host, git) = (FakeHost::untrusted(1, Ok("bb")), FakeGit::local());
        let (mut actor, _state) = start_with(store, &host, &git, Path::new(NO_CLAUDE_DIR));
        actor.move_thread(id, Workspace::NewWorktree).await;

        // When the user declines to trust the workspace.
        actor.decline_trust();

        // Then the worktree orb made is force-removed.
        let (path, _branch) = git
            .added()
            .ok_or_else(|| Report::new(StoreError).attach("no worktree was added"))?;
        assert!(
            git.calls()
                .contains(&GitCall::RemoveWorktree { path, force: true }),
            "a declined start should remove the worktree made for it"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn declining_trust_shows_workspace_not_trusted() -> Result<(), Report<StoreError>> {
        // Given a draft start waiting for trust.
        let (store, project) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::untrusted(1, Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.start_draft(project).await;

        // When the user declines to trust the workspace.
        actor.decline_trust();

        // Then the start fails as not trusted.
        assert_eq!(
            error_of(&state).as_deref(),
            Some("Workspace not trusted"),
            "a declined start should say the workspace isn't trusted"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn declining_trust_ends_the_trust_request() -> Result<(), Report<StoreError>> {
        // Given a draft start waiting for trust.
        let (store, project) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::untrusted(1, Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.start_draft(project).await;

        // When the user declines to trust the workspace.
        actor.decline_trust();

        // Then no trust is asked for any more.
        assert_eq!(
            trust_of(&state),
            None,
            "a declined start should stop asking for trust"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn declining_trust_keeps_the_draft() -> Result<(), Report<StoreError>> {
        // Given a draft start waiting for trust.
        let (store, project) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::untrusted(1, Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.start_draft(project).await;

        // When the user declines to trust the workspace.
        actor.decline_trust();

        // Then the draft still shows.
        assert!(
            shown_draft(&state, project).is_some(),
            "a declined start should keep the draft"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn declining_trust_without_a_waiting_start_shows_nothing()
    -> Result<(), Report<StoreError>> {
        // Given no start waiting for trust.
        let (store, _project) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When the user declines to trust the workspace.
        actor.decline_trust();

        // Then no error shows.
        assert_eq!(
            error_of(&state),
            None,
            "declining with nothing waiting should show nothing"
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
            Some("A session is working in this directory"),
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
            let project_id = store.add_project(root.path(), "orb", ProjectKind::Normal, 0)?;
            store
                .insert_thread(&NewThread {
                    harness: HarnessId::new("claude"),
                    project_id,
                    short_id: "aa".to_owned(),
                    cwd: Path::new(WORKTREES_ROOT).join("orb/orb-0123abcd"),
                    created_at: now_ms() - HOUR_MS,
                    model: None,
                    permission_mode: None,
                    group_id: None,
                    zmx: None,
                })
                .map(|inserted| inserted.thread)?
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
    async fn turn_end_renames_the_hex_branch_to_the_orb_name_slug() -> Result<(), Report<StoreError>>
    {
        // Given a thread on its hex branch titled only by its first prompt,
        // then renamed with `r`.
        let (claude_dir, store, id) = worktree_thread(PROMPT_LINE, HEX_BRANCH)?;
        resave(&store, "aa", |row| ThreadRow {
            renamed_title: Some("Sidebar search".to_owned()),
            ..row
        })?;
        let host = FakeHost::listing(Vec::new());
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
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::NeedsApproval)]);
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
        let host = FakeHost::listing(Vec::new());
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
        let host = FakeHost::listing(vec![in_session(ThreadStatus::Working)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When two polls in a row see it waiting for an approval.
        host.set_list(Ok(vec![in_session(ThreadStatus::NeedsApproval)]));
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
        let host = FakeHost::listing(vec![in_session(ThreadStatus::Working)]);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;
        state.write().sessions.deleting.insert(SessionId(id.0));

        // When a poll sees its turn end.
        host.set_list(Ok(vec![in_session(ThreadStatus::Idle)]));
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
        let host = FakeHost::listing(Vec::new());
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

    /// The orb project, last started with sonnet in plan mode, with the actor
    /// started on `git` and a temp folder as orb's own directory.
    fn creating(
        git: &Arc<FakeGit>,
    ) -> Result<(tempfile::TempDir, ProjectId, SessionsActor, State), Report<StoreError>> {
        let orb_root = tempfile::tempdir().change_context(StoreError)?;
        let store = Store::open_in_memory()?;
        let orb = orb_project(&store)?;
        store.record_last_used(orb, &used(LastWorkspace::NewWorktree), 10)?;
        let (actor, state) = start_in(
            store,
            &FakeHost::listing(Vec::new()),
            git,
            &FakeTrust::accepting(),
            Path::new(NO_CLAUDE_DIR),
            orb_root.path(),
            Path::new(INCOGNITO_ROOT),
        );
        Ok((orb_root, orb, actor, state))
    }

    /// Every group the sidebar has, in project order.
    fn groups_of(state: &State) -> Vec<Group> {
        state
            .read()
            .sessions
            .projects
            .iter()
            .flat_map(|project| project.groups.clone())
            .collect()
    }

    #[rstest::rstest]
    fn creating_a_research_group_seeds_a_missing_template() -> Result<(), Report<StoreError>> {
        // Given no Research template.
        let (orb_root, _, mut actor, _state) = creating(&FakeGit::local())?;

        // When creating Research group `tokio-cancel`.
        actor.create_group(GroupKind::Research, None, "tokio-cancel".into());

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
    fn creating_a_research_group_copies_the_template() -> Result<(), Report<StoreError>> {
        // Given no Research template.
        let (orb_root, _, mut actor, _state) = creating(&FakeGit::local())?;

        // When creating Research group `tokio-cancel`.
        actor.create_group(GroupKind::Research, None, "tokio-cancel".into());

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
    fn creating_a_research_group_copies_the_kit() -> Result<(), Report<StoreError>> {
        // Given no Research template.
        let (orb_root, _, mut actor, _state) = creating(&FakeGit::local())?;

        // When creating Research group `tokio-cancel`.
        actor.create_group(GroupKind::Research, None, "tokio-cancel".into());

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
    fn creating_a_research_group_adds_the_research_project() -> Result<(), Report<StoreError>> {
        // Given no Research project.
        let (orb_root, _, mut actor, state) = creating(&FakeGit::local())?;

        // When creating Research group `tokio-cancel`.
        actor.create_group(GroupKind::Research, None, "tokio-cancel".into());

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
    fn creating_a_learn_group_uses_the_learn_folder() -> Result<(), Report<StoreError>> {
        // Given no Learn project.
        let (orb_root, _, mut actor, _state) = creating(&FakeGit::local())?;

        // When creating Learn group `rust-async`.
        actor.create_group(GroupKind::Learn, None, "rust-async".into());

        // Then its folder is under orb's learn folder.
        assert!(
            orb_root.path().join("learn/rust-async").is_dir(),
            "a Learn group's folder should be under learn/"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn creating_a_group_keeps_a_curated_template() -> Result<(), Report<StoreError>> {
        // Given a Research template the user wrote.
        let (orb_root, _, mut actor, _state) = creating(&FakeGit::local())?;
        let template = orb_root.path().join("templates/research");
        fs::create_dir_all(&template).change_context(StoreError)?;
        fs::write(template.join("AGENTS.md"), "mine").change_context(StoreError)?;

        // When creating Research group `tokio-cancel`.
        actor.create_group(GroupKind::Research, None, "tokio-cancel".into());

        // Then the folder gets the user's AGENTS.md.
        let text = fs::read_to_string(orb_root.path().join("research/tokio-cancel/AGENTS.md"))
            .change_context(StoreError)?;
        assert_eq!(text, "mine", "a curated template should be copied as is");
        Ok(())
    }

    #[rstest::rstest]
    fn creating_a_group_selects_its_draft() -> Result<(), Report<StoreError>> {
        // Given the orb project.
        let (_orb_root, orb, mut actor, state) = creating(&FakeGit::local())?;

        // When creating Feature group `GT-514-login` in it.
        actor.create_group(GroupKind::Feature, Some(orb), "GT-514-login".into());

        // Then the cursor is on the new group's draft.
        let ids: Vec<SidebarItem> = groups_of(&state)
            .iter()
            .map(|group| SidebarItem::GroupDraft(group.id))
            .collect();
        assert_eq!(
            (state.read().sessions.cursor, ids.len()),
            (ids.first().copied(), 1),
            "the new group's draft should be selected"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn creating_a_group_saves_it() -> Result<(), Report<StoreError>> {
        // Given the orb project, last started with sonnet.
        let (_orb_root, orb, mut actor, _state) = creating(&FakeGit::local())?;

        // When creating Feature group `GT-514-login` in it.
        actor.create_group(GroupKind::Feature, Some(orb), "GT-514-login".into());

        // Then the store has it, with sonnet for its draft.
        let saved: Vec<(String, Option<String>)> = actor
            .store
            .load()?
            .3
            .into_iter()
            .map(|row| (row.name, row.draft_model))
            .collect();
        assert_eq!(
            saved,
            vec![("GT-514-login".to_owned(), Some("sonnet".to_owned()))],
            "the group should be saved with the last-used model"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn creating_a_group_over_an_existing_folder_is_refused() -> Result<(), Report<StoreError>> {
        // Given a folder `x` already under orb's research folder.
        let (orb_root, _, mut actor, state) = creating(&FakeGit::local())?;
        fs::create_dir_all(orb_root.path().join("research/x")).change_context(StoreError)?;

        // When creating Research group `x`.
        actor.create_group(GroupKind::Research, None, "x".into());

        // Then the mode line says the folder exists.
        assert_eq!(
            error_of(&state).as_deref(),
            Some("~/.orb/research/x already exists"),
            "an existing folder should be refused"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn refused_group_over_an_existing_folder_adds_no_group() -> Result<(), Report<StoreError>> {
        // Given a folder `x` already under orb's research folder.
        let (orb_root, _, mut actor, state) = creating(&FakeGit::local())?;
        fs::create_dir_all(orb_root.path().join("research/x")).change_context(StoreError)?;

        // When creating Research group `x`.
        actor.create_group(GroupKind::Research, None, "x".into());

        // Then no group shows or is saved.
        assert_eq!(
            (groups_of(&state).len(), actor.store.load()?.3.len()),
            (0, 0),
            "a refused group should leave nothing"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn creating_a_feature_group_on_an_existing_branch_is_refused() -> Result<(), Report<StoreError>>
    {
        // Given the orb project, where branch `GT-514-login` exists.
        let (_orb_root, orb, mut actor, state) = creating(&FakeGit::having("GT-514-login"))?;

        // When creating Feature group `GT-514-login` in it.
        actor.create_group(GroupKind::Feature, Some(orb), "GT-514-login".into());

        // Then the mode line says the branch exists, and no group shows.
        assert_eq!(
            (error_of(&state), groups_of(&state).len()),
            (
                Some("branch GT-514-login already exists in orb".to_owned()),
                0
            ),
            "an existing branch should refuse the group"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn creating_a_feature_group_makes_no_worktree() -> Result<(), Report<StoreError>> {
        // Given the orb project.
        let git = FakeGit::local();
        let (_orb_root, orb, mut actor, state) = creating(&git)?;

        // When creating Feature group `GT-514-login` in it.
        actor.create_group(GroupKind::Feature, Some(orb), "GT-514-login".into());

        // Then no worktree is added and the group has no directory yet.
        let dirs: Vec<Option<PathBuf>> = groups_of(&state).into_iter().map(|g| g.dir).collect();
        assert_eq!(
            (git.added(), dirs),
            (None, vec![None]),
            "a Feature group's worktree waits for its start"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn creating_a_taken_group_twice_is_refused() -> Result<(), Report<StoreError>> {
        // Given Research group `tokio-cancel` just created.
        let (_orb_root, _, mut actor, state) = creating(&FakeGit::local())?;
        actor.create_group(GroupKind::Research, None, "tokio-cancel".into());

        // When creating it again.
        actor.create_group(GroupKind::Research, None, "tokio-cancel".into());

        // Then the second is refused as taken, and one group shows.
        assert_eq!(
            (error_of(&state), groups_of(&state).len()),
            (Some("Group tokio-cancel already exists".to_owned()), 1),
            "a second create of the same group should be refused"
        );
        Ok(())
    }

    /// Opens the name box for a new `kind` group in `project` holding `text`,
    /// as `⏎` leaves it: waiting for the actor, with the keys.
    fn waiting_name_box(state: &State, kind: GroupKind, project: Option<ProjectId>, text: &str) {
        let mut app = state.write();
        app.rename = Some(Rename {
            target: RenameTarget::NewGroup { kind, project },
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
    fn made_group_closes_its_name_box() -> Result<(), Report<StoreError>> {
        // Given the name box waiting for Research group `tokio-cancel`.
        let (_orb_root, _, mut actor, state) = creating(&FakeGit::local())?;
        waiting_name_box(&state, GroupKind::Research, None, "tokio cancel");

        // When creating it.
        actor.create_group(GroupKind::Research, None, "tokio-cancel".into());

        // Then the box is closed and the sidebar has the keys.
        assert_eq!(
            name_box_of(&state),
            (None, Focus::Sidebar),
            "a made group should close its name box"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn group_refused_on_an_existing_branch_keeps_its_name_box_open()
    -> Result<(), Report<StoreError>> {
        // Given the name box waiting for Feature group `GT-514-login` in orb,
        // where that branch exists.
        let (_orb_root, orb, mut actor, state) = creating(&FakeGit::having("GT-514-login"))?;
        waiting_name_box(&state, GroupKind::Feature, Some(orb), "GT-514 login");

        // When creating it.
        actor.create_group(GroupKind::Feature, Some(orb), "GT-514-login".into());

        // Then the box stays open with the name, no longer waiting.
        assert_eq!(
            name_box_of(&state),
            (Some(("GT-514 login".to_owned(), false)), Focus::Rename),
            "an existing branch should keep the name box open"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn group_refused_on_an_existing_folder_keeps_its_name_box_open()
    -> Result<(), Report<StoreError>> {
        // Given the name box waiting for Research group `x`, whose folder
        // exists.
        let (orb_root, _, mut actor, state) = creating(&FakeGit::local())?;
        fs::create_dir_all(orb_root.path().join("research/x")).change_context(StoreError)?;
        waiting_name_box(&state, GroupKind::Research, None, "x");

        // When creating it.
        actor.create_group(GroupKind::Research, None, "x".into());

        // Then the box stays open with the name, no longer waiting.
        assert_eq!(
            name_box_of(&state),
            (Some(("x".to_owned(), false)), Focus::Rename),
            "an existing folder should keep the name box open"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn made_group_outside_the_filter_clears_it() -> Result<(), Report<StoreError>> {
        // Given the sidebar filtered to orb.
        let (_orb_root, orb, mut actor, state) = creating(&FakeGit::local())?;
        state.write().sessions.filter = Some(orb);

        // When creating Research group `tokio-cancel`, outside orb.
        actor.create_group(GroupKind::Research, None, "tokio-cancel".into());

        // Then the filter goes back to all projects.
        assert_eq!(
            state.read().sessions.filter,
            None,
            "a group outside the filter should clear it"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn made_group_outside_the_filter_saves_the_cleared_filter() -> Result<(), Report<StoreError>> {
        // Given the sidebar filtered to orb, and that saved.
        let (_orb_root, orb, mut actor, state) = creating(&FakeGit::local())?;
        state.write().sessions.filter = Some(orb);
        actor.save_ui();

        // When creating Research group `tokio-cancel`, outside orb.
        actor.create_group(GroupKind::Research, None, "tokio-cancel".into());

        // Then the store holds no filter.
        assert_eq!(
            actor.store.ui()?.project_filter,
            None,
            "the cleared filter should be saved"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn made_group_inside_the_filter_keeps_it() -> Result<(), Report<StoreError>> {
        // Given the sidebar filtered to orb.
        let (_orb_root, orb, mut actor, state) = creating(&FakeGit::local())?;
        state.write().sessions.filter = Some(orb);

        // When creating Feature group `GT-514-login` in orb.
        actor.create_group(GroupKind::Feature, Some(orb), "GT-514-login".into());

        // Then the filter stays on orb.
        assert_eq!(
            state.read().sessions.filter,
            Some(orb),
            "a group inside the filter should keep it"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn refused_group_keeps_the_filter() -> Result<(), Report<StoreError>> {
        // Given the sidebar filtered to orb, and Research folder `x` on disk.
        let (orb_root, orb, mut actor, state) = creating(&FakeGit::local())?;
        fs::create_dir_all(orb_root.path().join("research/x")).change_context(StoreError)?;
        state.write().sessions.filter = Some(orb);

        // When creating Research group `x`.
        actor.create_group(GroupKind::Research, None, "x".into());

        // Then the filter stays on orb.
        assert_eq!(
            state.read().sessions.filter,
            Some(orb),
            "a refused group should leave the filter alone"
        );
        Ok(())
    }

    const SLUG_BRANCH: &str = "GT-514-login";

    /// A store whose orb project has a `kind` group `GT-514-login` with a
    /// draft on opus in auto mode, in `dir` when given, and the actor started
    /// on `host` and `git` with the group draft selected.
    fn group_draft(
        kind: GroupKind,
        dir: Option<PathBuf>,
        host: &Arc<FakeHost>,
        git: &Arc<FakeGit>,
    ) -> Result<(GroupId, SessionsActor, State), Report<StoreError>> {
        let store = Store::open_in_memory()?;
        let project_id = orb_project(&store)?;
        let id = store.insert_group(&NewGroup {
            harness: HarnessId::new("claude"),
            project_id,
            kind,
            name: SLUG_BRANCH.to_owned(),
            dir,
            branch: (kind == GroupKind::Feature).then(|| SLUG_BRANCH.to_owned()),
            created_at: 1,
            draft_model: Some("opus".to_owned()),
            draft_permission_mode: Some("auto".to_owned()),
        })?;
        let (actor, state) = start_with(store, host, git, Path::new(NO_CLAUDE_DIR));
        state.write().sessions.cursor = Some(SidebarItem::GroupDraft(id));
        Ok((id, actor, state))
    }

    /// Group `id` as the sidebar shows it.
    fn shown_group(state: &State, id: GroupId) -> Option<Group> {
        groups_of(state).into_iter().find(|group| group.id == id)
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn starting_a_feature_group_draft_adds_the_worktree_on_its_slug_branch()
    -> Result<(), Report<StoreError>> {
        // Given a Feature group draft.
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::local());
        let (id, mut actor, _state) = group_draft(GroupKind::Feature, None, &host, &git)?;

        // When starting it.
        actor.start_group_draft(id).await;

        // Then git adds an orb worktree on the group's slug branch.
        let added = git
            .added()
            .map(|(path, branch)| (path.starts_with(WORKTREES_ROOT), branch));
        assert_eq!(
            added,
            Some((true, SLUG_BRANCH.to_owned())),
            "the worktree should be on the slug branch"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn starting_a_feature_group_draft_starts_the_thread_in_the_worktree()
    -> Result<(), Report<StoreError>> {
        // Given a Feature group draft.
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::local());
        let (id, mut actor, _state) = group_draft(GroupKind::Feature, None, &host, &git)?;

        // When starting it.
        actor.start_group_draft(id).await;

        // Then the session starts in the worktree git added.
        assert_eq!(
            host.created_in(),
            git.added()
                .map(|(path, _)| path)
                .into_iter()
                .collect::<Vec<_>>(),
            "the first thread should start in the new worktree"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn started_feature_group_keeps_the_worktree_as_its_dir() -> Result<(), Report<StoreError>>
    {
        // Given a Feature group draft.
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::local());
        let (id, mut actor, state) = group_draft(GroupKind::Feature, None, &host, &git)?;

        // When starting it.
        actor.start_group_draft(id).await;

        // Then the group's directory is the worktree, shown and saved.
        let worktree = git.added().map(|(path, _)| path);
        let saved = actor.store.load()?.3.into_iter().find_map(|row| row.dir);
        assert_eq!(
            (shown_group(&state, id).and_then(|group| group.dir), saved),
            (worktree.clone(), worktree),
            "the worktree should be the group's directory from now on"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn starting_a_group_draft_puts_its_thread_in_the_group() -> Result<(), Report<StoreError>>
    {
        // Given a Feature group draft.
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::local());
        let (id, mut actor, state) = group_draft(GroupKind::Feature, None, &host, &git)?;

        // When starting it.
        actor.start_group_draft(id).await;

        // Then the new thread is in the group, which has no draft any more.
        let grouped: Vec<Option<GroupId>> = state
            .read()
            .sessions
            .threads()
            .map(|thread| thread.group)
            .collect();
        let draft = shown_group(&state, id).map(|group| group.draft.is_some());
        assert_eq!(
            (grouped, draft),
            (vec![Some(id)], Some(false)),
            "the group's first thread should replace its draft"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn starting_a_selected_group_draft_selects_and_attaches_the_thread()
    -> Result<(), Report<StoreError>> {
        // Given a selected Feature group draft.
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::local());
        let (id, mut actor, state) = group_draft(GroupKind::Feature, None, &host, &git)?;

        // When starting it.
        actor.start_group_draft(id).await;

        // Then the new thread is selected and attached.
        let sessions = &state.read().sessions;
        let thread = sessions.threads().next();
        assert_eq!(
            (sessions.cursor, sessions.attach),
            (
                thread.and_then(Thread::session).map(SidebarItem::Session),
                thread.and_then(Thread::session)
            ),
            "the started thread should be selected and attached"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn trusted_group_draft_start_selects_and_attaches_the_thread()
    -> Result<(), Report<StoreError>> {
        // Given a selected Research group draft whose start waits for its
        // folder to be trusted.
        let folder = tempfile::tempdir().change_context(StoreError)?;
        let (host, git) = (FakeHost::untrusted(1, Ok("bb")), FakeGit::local());
        let (id, mut actor, state) = group_draft(
            GroupKind::Research,
            Some(folder.path().to_owned()),
            &host,
            &git,
        )?;
        actor.start_group_draft(id).await;

        // When the user trusts the folder.
        actor.trust_workspace().await;

        // Then its thread is selected and to be attached.
        let sessions = &state.read().sessions;
        let thread = sessions.threads().next();
        assert_eq!(
            (sessions.cursor, sessions.attach),
            (
                thread.and_then(Thread::session).map(SidebarItem::Session),
                thread.and_then(Thread::session)
            ),
            "the trusted start's thread should replace the draft and attach"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn starting_a_research_group_draft_runs_in_its_folder() -> Result<(), Report<StoreError>>
    {
        // Given a Research group draft with its folder.
        let folder = tempfile::tempdir().change_context(StoreError)?;
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::local());
        let (id, mut actor, _state) = group_draft(
            GroupKind::Research,
            Some(folder.path().to_owned()),
            &host,
            &git,
        )?;

        // When starting it.
        actor.start_group_draft(id).await;

        // Then the session starts in the folder, and git adds no worktree.
        assert_eq!(
            (host.created_in(), git.added()),
            (vec![folder.path().to_owned()], None),
            "a Research group should start in its folder"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn starting_a_group_draft_uses_its_model_and_permission() -> Result<(), Report<StoreError>>
    {
        // Given a Feature group draft on opus in auto mode.
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::local());
        let (id, mut actor, _state) = group_draft(GroupKind::Feature, None, &host, &git)?;

        // When starting it.
        actor.start_group_draft(id).await;

        // Then the session starts with them.
        assert_eq!(
            host.created_with(),
            vec![SessionOptions {
                model: Some("opus".to_owned()),
                permission_mode: Some("auto".to_owned()),
            }],
            "the group draft's settings should start the session"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn feature_group_start_fails_when_the_branch_appeared() -> Result<(), Report<StoreError>>
    {
        // Given a Feature group draft whose branch was made outside orb.
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::having(SLUG_BRANCH));
        let (id, mut actor, state) = group_draft(GroupKind::Feature, None, &host, &git)?;

        // When starting it.
        actor.start_group_draft(id).await;

        // Then the mode line says the branch exists, and the draft stays.
        assert_eq!(
            (
                error_of(&state),
                shown_group(&state, id).is_some_and(|group| group.draft.is_some())
            ),
            (
                Some("branch GT-514-login already exists in orb".to_owned()),
                true
            ),
            "an existing branch should refuse the start"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_group_draft_start_keeps_the_draft() -> Result<(), Report<StoreError>> {
        // Given a Feature group draft and a host that refuses to start a session.
        let (host, git) = (FakeHost::creating(Err("claude failed")), FakeGit::local());
        let (id, mut actor, state) = group_draft(GroupKind::Feature, None, &host, &git)?;

        // When starting it.
        actor.start_group_draft(id).await;

        // Then the group keeps its draft, and the worktree made for it is removed.
        let removed = git.added().is_some_and(|(path, _)| {
            git.calls()
                .contains(&GitCall::RemoveWorktree { path, force: true })
        });
        assert_eq!(
            (
                shown_group(&state, id).is_some_and(|group| group.draft.is_some()),
                removed
            ),
            (true, true),
            "a failed start should leave the group as it was"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn turn_end_keeps_a_feature_groups_slug_branch() -> Result<(), Report<StoreError>> {
        // Given a Claude-titled thread in an orb worktree on a group's slug branch.
        let (claude_dir, store, _) =
            worktree_thread(&format!("{PROMPT_LINE}{AI_TITLE_LINE}"), SLUG_BRANCH)?;
        let host = FakeHost::listing(Vec::new());
        let git = FakeGit::local();
        let (mut actor, _state) = start_with(store, &host, &git, claude_dir.path());

        // When a poll sees its turn end.
        end_turn(&mut actor, &host).await;

        // Then no branch was renamed.
        assert_eq!(git.renamed(), None, "a slug branch is never renamed");
        Ok(())
    }

    /// Gives group `id` `draft` in the app state, with the cursor on it.
    fn give_draft(state: &State, id: GroupId, draft: GroupDraft) {
        let sessions = &mut state.write().sessions;
        if let Some(group) = sessions.group_mut(id) {
            group.draft = Some(draft);
        }
        sessions.cursor = Some(SidebarItem::GroupDraft(id));
    }

    /// A `kind` group in a temp folder holding thread aa, whose saved draft
    /// is selected, with the actor started on `host` and `git`.
    fn started_group_draft(
        kind: GroupKind,
        host: &Arc<FakeHost>,
        git: &Arc<FakeGit>,
    ) -> Result<(tempfile::TempDir, GroupId, SessionsActor, State), Report<StoreError>> {
        let dir = tempfile::tempdir().change_context(StoreError)?;
        let (store, group, _) = store_with_group(kind, Some(dir.path()), &["aa"])?;
        let (mut actor, state) = start_with(store, host, git, Path::new(NO_CLAUDE_DIR));
        give_draft(&state, group, GroupDraft::default());
        actor.save_group_draft(group);
        Ok((dir, group, actor, state))
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn starting_a_group_draft_in_a_started_feature_group_uses_its_worktree()
    -> Result<(), Report<StoreError>> {
        // Given a started Feature group in its worktree, with a draft.
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::local());
        let (dir, id, mut actor, _state) = started_group_draft(GroupKind::Feature, &host, &git)?;

        // When starting the draft.
        actor.start_group_draft(id).await;

        // Then the session starts in the group's worktree, and git adds none.
        assert_eq!(
            (host.created_in(), git.added()),
            (vec![dir.path().to_owned()], None),
            "a started Feature group's draft should reuse its worktree"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn starting_a_group_draft_in_a_started_feature_group_takes_its_checked_out_branch()
    -> Result<(), Report<StoreError>> {
        // Given a started Feature group whose worktree has dev checked out,
        // with a draft.
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::local());
        let (_dir, id, mut actor, _state) = started_group_draft(GroupKind::Feature, &host, &git)?;

        // When starting the draft as thread bb.
        actor.start_group_draft(id).await;

        // Then bb is saved on dev.
        assert_eq!(
            saved(&actor.store, "bb")?.branch.as_deref(),
            Some(CURRENT_BRANCH),
            "the new thread should be on the worktree's checked out branch"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn starting_a_group_draft_in_a_started_group_puts_its_thread_first()
    -> Result<(), Report<StoreError>> {
        // Given a started Research group holding thread aa, with a draft.
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::local());
        let (_dir, id, mut actor, state) = started_group_draft(GroupKind::Research, &host, &git)?;

        // When starting the draft as thread bb.
        actor.start_group_draft(id).await;

        // Then bb is listed first in the group.
        let first = state
            .read()
            .sessions
            .threads()
            .find(|thread| thread.group == Some(id))
            .and_then(|thread| thread.pane.clone())
            .map(|launch| launch.command);
        assert_eq!(
            first,
            Some(vec![OsString::from("bb")]),
            "a new thread should go to the top of its group"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn starting_a_group_draft_after_the_cursor_moved_keeps_the_cursor()
    -> Result<(), Report<StoreError>> {
        // Given a started Research group's draft, with the cursor since moved
        // to the shelf's header.
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::local());
        let (_dir, id, mut actor, state) = started_group_draft(GroupKind::Research, &host, &git)?;
        state.write().sessions.cursor = Some(SidebarItem::SettledShelf);

        // When the draft starts.
        actor.start_group_draft(id).await;

        // Then the cursor stays where it is, and nothing is attached.
        let sessions = &state.read().sessions;
        assert_eq!(
            (sessions.cursor, sessions.attach),
            (Some(SidebarItem::SettledShelf), None),
            "a moved cursor should stay put"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case(GroupKind::Feature)]
    #[case(GroupKind::Research)]
    #[case(GroupKind::Learn)]
    #[tokio::test]
    async fn starting_a_group_draft_in_a_missing_directory_fails(
        #[case] kind: GroupKind,
    ) -> Result<(), Report<StoreError>> {
        // Given a started `kind` group whose directory is gone, its draft
        // selected and a start in flight.
        let (store, id, _) =
            store_with_group(kind, Some(Path::new("/nonexistent/GT-514-login")), &["aa"])?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        give_draft(&state, id, GroupDraft::default());
        state.write().sessions.starting = true;

        // When starting the draft.
        actor.start_group_draft(id).await;

        // Then the start ends with the missing folder on the mode line.
        let sessions = &state.read().sessions;
        assert_eq!(
            (
                sessions
                    .error
                    .as_deref()
                    .is_some_and(|error| error.starts_with("folder no longer exists")),
                sessions.starting
            ),
            (true, false),
            "a missing directory should fail the start"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn starting_a_group_draft_uses_its_overrides() -> Result<(), Report<StoreError>> {
        // Given a Feature group on opus in auto mode whose draft picked
        // sonnet.
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::local());
        let (id, mut actor, state) = group_draft(GroupKind::Feature, None, &host, &git)?;
        give_draft(
            &state,
            id,
            GroupDraft {
                model: Own::Set(Some("sonnet".to_owned())),
                ..GroupDraft::default()
            },
        );

        // When starting it.
        actor.start_group_draft(id).await;

        // Then the session starts on sonnet in the group's auto mode.
        assert_eq!(
            host.created_with(),
            vec![SessionOptions {
                model: Some("sonnet".to_owned()),
                permission_mode: Some("auto".to_owned()),
            }],
            "the draft's own pick should win over the group's default"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn starting_a_group_draft_clears_it() -> Result<(), Report<StoreError>> {
        // Given a started Research group with a draft.
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::local());
        let (_dir, id, mut actor, state) = started_group_draft(GroupKind::Research, &host, &git)?;

        // When starting the draft.
        actor.start_group_draft(id).await;

        // Then the group shows no draft.
        assert_eq!(
            shown_group(&state, id).map(|group| group.draft),
            Some(None),
            "a started draft should leave the group"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn starting_a_group_draft_saves_the_cleared_draft() -> Result<(), Report<StoreError>> {
        // Given a started Research group with a saved draft.
        let (host, git) = (FakeHost::creating(Ok("bb")), FakeGit::local());
        let (_dir, id, mut actor, _state) = started_group_draft(GroupKind::Research, &host, &git)?;

        // When starting the draft.
        actor.start_group_draft(id).await;

        // Then the saved group has no draft.
        assert_eq!(
            saved_group(&actor.store, id)?.draft,
            None,
            "the cleared draft should be saved"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn group_draft_overrides_are_saved() -> Result<(), Report<StoreError>> {
        // Given a group holding thread aa whose draft picked pi on its
        // default model.
        let (store, id, _) = store_with_group(GroupKind::Research, None, &["aa"])?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        let draft = GroupDraft {
            harness: Own::Set(HarnessId::new("pi")),
            model: Own::Set(None),
            permission: Own::Group,
        };
        give_draft(&state, id, draft.clone());

        // When saving it.
        actor.save_group_draft(id);

        // Then the saved row has the draft.
        assert_eq!(
            saved_group(&actor.store, id)?.draft,
            Some(draft),
            "the draft's overrides should be saved"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn threadless_group_saved_without_a_draft_still_shows_one() -> Result<(), Report<StoreError>> {
        // Given a group with no thread, saved without a draft.
        let (store, id, _) = store_with_group(GroupKind::Research, None, &[])?;
        let host = FakeHost::listing(Vec::new());

        // When restoring it.
        let (_actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // Then it shows a draft that follows the group.
        assert_eq!(
            shown_group(&state, id).map(|group| group.draft),
            Some(Some(GroupDraft::default())),
            "a threadless group should always show a draft"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn new_group_takes_the_last_used_harness_with_its_model() -> Result<(), Report<StoreError>> {
        // Given orb last started in the other harness on other-model.
        let store = Store::open_in_memory()?;
        let orb = orb_project(&store)?;
        store.record_last_used(
            orb,
            &LastUsed {
                model: Some("other-model".to_owned()),
                ..used_in(OTHER)
            },
            10,
        )?;
        let host = FakeHost::listing(Vec::new());
        let (mut actor, state) = start_beside(store, &host, &host);

        // When creating Feature group `GT-514-login` in it.
        actor.create_group(GroupKind::Feature, Some(orb), SLUG_BRANCH.into());

        // Then the group's defaults are the other harness on other-model.
        let defaults: Vec<(HarnessId, Option<String>)> = groups_of(&state)
            .into_iter()
            .map(|group| (group.defaults.harness, group.defaults.model))
            .collect();
        assert_eq!(
            defaults,
            vec![(HarnessId::new(OTHER), Some("other-model".to_owned()))],
            "a new group should take the last-used harness with its model"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn saving_a_started_groups_defaults_keeps_the_rest_of_its_row()
    -> Result<(), Report<StoreError>> {
        // Given a started Feature group, switched to `main`, whose defaults
        // were changed on its card to sonnet in plan mode.
        let (group, mut actor, state) = started_feature_group(&FakeGit::local()).await?;
        actor.check_out_group(group, &git_ref("main", false));
        if let Some(defaults) = state
            .write()
            .sessions
            .group_mut(group)
            .map(|group| &mut group.defaults)
        {
            defaults.model = Some("sonnet".to_owned());
            defaults.permission = Some("plan".to_owned());
        }

        // When saving them.
        actor.save_group_draft(group);

        // Then the saved row has the defaults and still its directory and
        // branch.
        let row = saved_group(&actor.store, group)?;
        assert_eq!(
            (
                row.draft_model.as_deref(),
                row.draft_permission_mode.as_deref(),
                row.dir.as_deref(),
                row.branch.as_deref()
            ),
            (
                Some("sonnet"),
                Some("plan"),
                Some(Path::new(HEX_WORKTREE)),
                Some("main")
            ),
            "the card's defaults should save onto the group's kept row"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn saving_a_started_groups_defaults_leaves_its_threads_alone()
    -> Result<(), Report<StoreError>> {
        // Given a started Feature group whose thread aa runs Claude's default,
        // and whose defaults were changed to sonnet.
        let (group, mut actor, state) = started_feature_group(&FakeGit::local()).await?;
        if let Some(defaults) = state
            .write()
            .sessions
            .group_mut(group)
            .map(|group| &mut group.defaults)
        {
            defaults.model = Some("sonnet".to_owned());
        }

        // When saving them.
        actor.save_group_draft(group);

        // Then thread aa keeps its saved model.
        assert_eq!(
            saved(&actor.store, "aa")?.model,
            None,
            "a running thread keeps its model"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn group_default_harness_is_saved() -> Result<(), Report<StoreError>> {
        // Given a group draft whose default harness was changed to the other
        // harness.
        let (host, git) = (FakeHost::listing(Vec::new()), FakeGit::local());
        let (id, mut actor, state) = group_draft(GroupKind::Feature, None, &host, &git)?;
        if let Some(defaults) = state
            .write()
            .sessions
            .group_mut(id)
            .map(|group| &mut group.defaults)
        {
            defaults.harness = HarnessId::new(OTHER);
        }

        // When saving it.
        actor.save_group_draft(id);

        // Then the group's stored row reads back with the other harness.
        let saved: Vec<HarnessId> = actor
            .store
            .load()?
            .3
            .into_iter()
            .map(|row| row.harness)
            .collect();
        assert_eq!(
            saved,
            vec![HarnessId::new(OTHER)],
            "the group's default harness should be saved"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn saving_a_group_draft_persists_its_model() -> Result<(), Report<StoreError>> {
        // Given a group draft whose model was changed to sonnet.
        let (host, git) = (FakeHost::listing(Vec::new()), FakeGit::local());
        let (id, mut actor, state) = group_draft(GroupKind::Feature, None, &host, &git)?;
        if let Some(defaults) = state
            .write()
            .sessions
            .group_mut(id)
            .map(|group| &mut group.defaults)
        {
            defaults.model = Some("sonnet".to_owned());
        }

        // When saving it.
        actor.save_group_draft(id);

        // Then the store has sonnet for the group's draft.
        let saved: Vec<Option<String>> = actor
            .store
            .load()?
            .3
            .into_iter()
            .map(|row| row.draft_model)
            .collect();
        assert_eq!(
            saved,
            vec![Some("sonnet".to_owned())],
            "the group draft's model should be saved"
        );
        Ok(())
    }

    /// A store whose orb project has a `kind` group `GT-514-login`, in `dir`
    /// when given, holding one thread per short id, each created an hour ago.
    fn store_with_group(
        kind: GroupKind,
        dir: Option<&Path>,
        short_ids: &[&str],
    ) -> Result<(Store, GroupId, Vec<ThreadId>), Report<StoreError>> {
        let store = Store::open_in_memory()?;
        let project_id = orb_project(&store)?;
        let group = store.insert_group(&NewGroup {
            harness: HarnessId::new("claude"),
            project_id,
            kind,
            name: SLUG_BRANCH.to_owned(),
            dir: dir.map(Path::to_owned),
            branch: (kind == GroupKind::Feature).then(|| SLUG_BRANCH.to_owned()),
            created_at: 1,
            draft_model: None,
            draft_permission_mode: None,
        })?;
        let threads = short_ids
            .iter()
            .map(|short_id| {
                store
                    .insert_thread(&NewThread {
                        harness: HarnessId::new("claude"),
                        project_id,
                        short_id: (*short_id).to_owned(),
                        cwd: dir.unwrap_or(Path::new(PROJECT_ROOT)).to_owned(),
                        created_at: now_ms() - HOUR_MS,
                        model: None,
                        permission_mode: None,
                        group_id: Some(group),
                        zmx: None,
                    })
                    .map(|inserted| inserted.thread)
            })
            .collect::<Result<_, _>>()?;
        Ok((store, group, threads))
    }

    /// The saved row of group `id`.
    fn saved_group(store: &Store, id: GroupId) -> Result<GroupRow, Report<StoreError>> {
        store
            .load()?
            .3
            .into_iter()
            .find(|row| row.id == id)
            .ok_or_else(|| Report::new(StoreError).attach(format!("group {id:?} isn't saved")))
    }

    /// A started Feature group in the orb worktree [`HEX_WORKTREE`] on
    /// [`SLUG_BRANCH`], holding idle thread aa, polled once, on `git`.
    async fn started_feature_group(
        git: &Arc<FakeGit>,
    ) -> Result<(GroupId, SessionsActor, State), Report<StoreError>> {
        let (store, group, _) =
            store_with_group(GroupKind::Feature, Some(Path::new(HEX_WORKTREE)), &["aa"])?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, state) = start_with(store, &host, git, Path::new(NO_CLAUDE_DIR));
        actor.poll().await;
        Ok((group, actor, state))
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn switching_a_groups_branch_shows_it_on_the_group() -> Result<(), Report<StoreError>> {
        // Given a started Feature group on its slug branch.
        let (group, mut actor, state) = started_feature_group(&FakeGit::local()).await?;

        // When switching its worktree to `main`.
        actor.check_out_group(group, &git_ref("main", false));

        // Then the card shows `main`.
        assert_eq!(
            shown_group(&state, group).and_then(|group| group.branch),
            Some("main".to_owned()),
            "the group should show its new branch"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn switching_a_groups_branch_saves_it() -> Result<(), Report<StoreError>> {
        // Given a started Feature group on its slug branch.
        let (group, mut actor, _state) = started_feature_group(&FakeGit::local()).await?;

        // When switching its worktree to `main`.
        actor.check_out_group(group, &git_ref("main", false));

        // Then the saved group is on `main`.
        assert_eq!(
            saved_group(&actor.store, group)?.branch.as_deref(),
            Some("main"),
            "the group's new branch should be saved"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn switching_a_groups_branch_moves_every_thread_in_it() -> Result<(), Report<StoreError>>
    {
        // Given a started Feature group holding idle threads aa and bb.
        let (store, group, threads) = store_with_group(
            GroupKind::Feature,
            Some(Path::new(HEX_WORKTREE)),
            &["aa", "bb"],
        )?;
        let host = FakeHost::listing(vec![
            record("aa", ThreadStatus::Idle),
            record("bb", ThreadStatus::Idle),
        ]);
        let (mut actor, state) =
            start_with(store, &host, &FakeGit::local(), Path::new(NO_CLAUDE_DIR));
        actor.poll().await;

        // When switching its worktree to `main`.
        actor.check_out_group(group, &git_ref("main", false));

        // Then both threads show `main`.
        let branches: Vec<Option<String>> =
            threads.iter().map(|id| branch_of(&state, *id)).collect();
        assert_eq!(
            branches,
            vec![Some("main".to_owned()); 2],
            "every thread in the worktree should show the new branch"
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
    ) -> Result<InsertedThread, Report<StoreError>> {
        let project_id = orb_project(store)?;
        let inserted = store.insert_thread(&NewThread {
            harness: HarnessId::new("claude"),
            project_id,
            short_id: short_id.to_owned(),
            cwd: cwd.to_owned(),
            created_at: now_ms() - HOUR_MS,
            model: None,
            permission_mode: None,
            group_id: None,
            zmx: None,
        })?;
        resave(store, short_id, |row| ThreadRow {
            branch: branch.map(str::to_owned),
            ..row
        })?;
        Ok(inserted)
    }

    /// The session thread `id` runs in, as `store` has it.
    fn session_of(store: &Store, id: ThreadId) -> Result<SessionId, Report<StoreError>> {
        let pane = store
            .load()?
            .1
            .into_iter()
            .find(|row| row.id == id)
            .and_then(|row| row.pane_id);
        store
            .layouts()?
            .panes
            .into_iter()
            .find(|row| Some(row.id) == pane)
            .map(|row| row.session_id)
            .ok_or_else(|| Report::new(StoreError).attach("the thread has no session"))
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
        let (mut actor, _state) = start_with(
            store,
            &FakeHost::listing(vec![]),
            git,
            Path::new(NO_CLAUDE_DIR),
        );
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
            &FakeHost::listing(vec![]),
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
            &FakeHost::listing(vec![]),
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
            &FakeHost::listing(vec![]),
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
    async fn restore_keeps_starting_while_another_start_waits_for_trust()
    -> Result<(), Report<StoreError>> {
        // Given a draft start waiting for its folder to be trusted, and thread
        // aa's pruned worktree.
        let (store, project) = store_with_draft(|id| draft_row(id, DraftWorkspace::Local))?;
        let id = insert_in(&store, "aa", Path::new(HEX_WORKTREE), Some("fix-parser"))?.session;
        let host = FakeHost::untrusted(1, Ok("bb"));
        let (mut actor, state) =
            start_with(store, &host, &FakeGit::local(), Path::new(NO_CLAUDE_DIR));
        state.write().sessions.starting = true;
        actor.start_draft(project).await;

        // When restoring aa's worktree.
        actor.restore_worktree(id);

        // Then the waiting start still shows as starting.
        assert!(
            state.read().sessions.starting,
            "a restore should not end a start that still waits for trust"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn restore_in_a_feature_group_uses_the_group_branch() -> Result<(), Report<StoreError>> {
        // Given thread aa of a Feature group whose worktree is gone, aa saved
        // on another branch, and the group's branch still there.
        let (store, _, threads) =
            store_with_group(GroupKind::Feature, Some(Path::new(HEX_WORKTREE)), &["aa"])?;
        resave(&store, "aa", |row| ThreadRow {
            branch: Some(HEX_BRANCH.to_owned()),
            ..row
        })?;
        let id = threads
            .first()
            .copied()
            .ok_or_else(|| Report::new(StoreError).attach("aa wasn't saved"))?;
        let id = session_of(&store, id)?;
        let git = FakeGit::having(SLUG_BRANCH);

        // When restoring its worktree.
        let calls = restore_calls(store, &git, id);

        // Then the worktree comes back on the group's branch.
        assert!(
            calls.contains(&GitCall::AddWorktreeOn {
                path: PathBuf::from(HEX_WORKTREE),
                branch: SLUG_BRANCH.to_owned(),
            }),
            "a Feature group's thread should get the group's branch back"
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
        thread: InsertedThread,
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
            F: FnOnce(&Store, &InsertedThread) -> Result<(), Report<StoreError>>,
        {
            let dir = tempfile::tempdir().change_context(StoreError)?;
            let store = Store::open_in_memory()?;
            let thread = insert_thread(&store, "aa", None)?;
            let shell = store.insert_pane(thread.session, Path::new(PROJECT_ROOT))?;
            setup(&store, &thread)?;
            let host = FakeHost::listing(Vec::new());
            let (actor, state) = start_in(
                store,
                &host,
                &FakeGit::local(),
                &FakeTrust::accepting(),
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
        fx.actor.settle_session(fx.thread.session).await;
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
            other = Some(insert_thread(store, "bb", None)?);
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
    /// records) or `pi` (status from pane reports), polled through a host
    /// listing `records` and a zmx whose listing `zmx` makes from the pane,
    /// the actor reading pane files under a scratch orb folder.
    struct PaneStatus {
        dir: tempfile::TempDir,
        thread: InsertedThread,
        host: Arc<FakeHost>,
        actor: SessionsActor,
        state: State,
    }

    impl PaneStatus {
        fn new<F>(
            reports: bool,
            records: Vec<SessionRecord>,
            zmx: F,
        ) -> Result<Self, Report<StoreError>>
        where
            F: FnOnce(PaneId) -> ZmxService,
        {
            let dir = tempfile::tempdir().change_context(StoreError)?;
            let store = Store::open_in_memory()?;
            let agent = if reports { "pi" } else { "claude" };
            let thread = store.insert_thread(&NewThread {
                harness: HarnessId::new(agent),
                project_id: orb_project(&store)?,
                short_id: "aa".to_owned(),
                cwd: PathBuf::from(PROJECT_ROOT),
                created_at: now_ms() - HOUR_MS,
                model: None,
                permission_mode: None,
                group_id: None,
                zmx: None,
            })?;
            let host = FakeHost::listing(records);
            let harness: Arc<dyn Harness> = if reports {
                Arc::new(FakeHarness::reporting(agent, host.clone()))
            } else {
                Arc::new(FakeHarness::new(agent, host.clone()))
            };
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
    fn interactive(ancestry: &[u32], status: ThreadStatus) -> SessionRecord {
        SessionRecord {
            short_id: None,
            session_id: None,
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
    fn start_unselected(store: Store, host: &Arc<FakeHost>) -> (SessionsActor, State) {
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
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
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
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
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
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
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
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
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
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
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
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
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
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
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
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Working)]);
        let (mut actor, _state) = start_unselected(store, &host);
        actor.poll().await;

        // When polling after its turn ended.
        host.set_list(Ok(vec![record("aa", ThreadStatus::Idle)]));
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
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
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
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
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
        let settled = insert_thread(&store, "aa", None)?;
        let active = insert_thread(&store, "bb", None)?;
        let host = FakeHost::listing(Vec::new());
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
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
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

    #[rstest::rstest]
    #[tokio::test]
    async fn long_idle_thread_no_longer_settles_itself() -> Result<(), Report<StoreError>> {
        // Given thread aa idle for four days.
        let (store, _) = store_with_thread("aa")?;
        idle_for_four_days(&store, "aa")?;
        let host = FakeHost::listing(vec![record("aa", ThreadStatus::Idle)]);
        let (mut actor, _state) = start_unselected(store, &host);

        // When polling.
        actor.poll().await;

        // Then the thread's own settle field stays unused.
        assert_eq!(
            saved(&actor.store, "aa")?.settled_override,
            None,
            "a thread settles only through its session"
        );
        Ok(())
    }
}
