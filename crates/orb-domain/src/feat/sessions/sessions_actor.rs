//! The sessions actor — the owner of orb's threads, their statuses, and titles.
//!
//! At start it restores the saved threads into the shared state. Then it polls
//! the session host: every second while a turn is underway or orb is attached,
//! every five seconds otherwise, and right away when asked. Each poll maps the
//! host's records onto the threads, stamps when a turn starts, reads new
//! transcript lines for titles and branches, and saves what changed. It also
//! starts new sessions in a picked project's directory and adds projects.
//!
//! It keeps each thread's place in the sidebar: pinning, settling onto the
//! Settled shelf (which stops the session), un-settling, and deleting. A turn
//! un-settles its thread, and a thread idle for three days settles itself
//! unless it is pinned, was just un-settled, or orb is attached to it. A turn
//! that ends while the user is on another thread shows as unseen until they
//! select it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use error_stack::Report;
use kameo::mailbox;
use kameo::prelude::{Actor, ActorRef, Context, Message, Reply, Spawn};
use tokio::sync::Notify;

use super::session_host::{SessionHostError, SessionHostService, SessionRecord};
use super::state::{
    Project, ProjectId, Sessions, SidebarItem, SidebarRow, Thread, ThreadId, ThreadStatus,
};
use super::store::{NewThread, SettledOverride, Store, ThreadRow};
use super::transcript::{locate, scan_title};
use crate::Focus;
use crate::common::{Services, State, Wake};

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

/// What the sessions actor needs to start.
pub struct SessionsActorDeps {
    pub services: Services,
    pub state: State,
    pub store: Store,
    /// Claude's config directory, where transcripts live.
    pub claude_dir: PathBuf,
    /// Tells the frontend to redraw.
    pub wake: Wake,
}

/// Owns [`Sessions`](super::state::Sessions): the projects, the threads'
/// statuses, titles, pins and settles, and the latest `claude` error. The
/// intent handler also moves the cursor, opens and closes the shelf, and marks
/// a create as starting.
pub struct SessionsActor {
    services: Services,
    state: State,
    store: Store,
    claude_dir: PathBuf,
    wake: Wake,
    /// Cuts the ticker's wait short so it polls now.
    poke: Arc<Notify>,
    /// The saved threads, as last written to the store.
    rows: Vec<ThreadRow>,
}

/// Poll the session host now.
#[derive(Debug)]
pub struct Poll;

/// How long to wait before the next [`Poll`].
#[derive(Debug, Reply)]
pub struct NextPoll(pub Duration);

/// Start a new session in a project's directory.
#[derive(Debug)]
pub struct CreateSession {
    pub project: ProjectId,
    pub root: PathBuf,
}

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

impl Message<CreateSession> for SessionsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        CreateSession { project, root }: CreateSession,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.create(project, &root).await;
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

impl SessionsActor {
    /// Shows the saved projects and threads, and selects the sidebar's first
    /// row.
    fn restore(deps: SessionsActorDeps) -> Self {
        let SessionsActorDeps {
            services,
            state,
            store,
            claude_dir,
            wake,
        } = deps;
        let (projects, rows, error) = match store.load() {
            Ok((projects, rows)) => (projects, rows, None),
            Err(_) => (
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
                id: project.id,
                title: project.title,
                root: project.root,
                created_at: from_ms(project.created_at),
            })
            .collect();
        {
            let mut app = state.write();
            let sessions = &mut app.sessions;
            sessions.projects = projects;
            sessions.cursor = sessions.sidebar().first().map(SidebarRow::item);
            sessions.error = error;
        }
        wake();
        Self {
            services,
            state,
            store,
            claude_dir,
            wake,
            poke: Arc::default(),
            rows,
        }
    }

    /// Asks the host what every session is doing and shows it. Returns how
    /// long to wait before the next poll.
    async fn poll(&mut self) -> Duration {
        match self.services.session_host.list().await {
            Ok(records) => {
                // ponytail: stops run in order (~0.7 s each); only a first
                // launch that auto-settles many threads waits long.
                for short_id in self.apply(&records) {
                    self.stop(&short_id).await;
                }
            }
            Err(report) => self.fail(&report),
        }
        let app = self.state.read();
        if app.sessions.any_in_progress() || app.focus == Focus::Attached {
            FAST_POLL
        } else {
            SLOW_POLL
        }
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
            let mut changed = sessions.error != error;
            sessions.error = error;
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

    /// Starts a session in the project's directory, saves it, and shows it
    /// selected at the top of the project.
    async fn create(&mut self, project_id: ProjectId, root: &Path) {
        let created = match self.services.session_host.create(root).await {
            Ok(created) => self.save_new(project_id, root, &created.short_id),
            Err(report) => Err(reason(&report)),
        };
        let succeeded = created.is_ok();
        {
            let mut app = self.state.write();
            let sessions = &mut app.sessions;
            sessions.starting = false;
            match created {
                Ok(thread) => match sessions
                    .projects
                    .iter_mut()
                    .find(|project| project.id == project_id)
                {
                    Some(project) => {
                        sessions.cursor = Some(SidebarItem::Thread(thread.id));
                        sessions.error = None;
                        project.threads.insert(0, thread);
                    }
                    None => sessions.error = Some(NEW_SESSION_UNSAVED.to_owned()),
                },
                Err(error) => sessions.error = Some(error),
            }
        }
        (self.wake)();
        if succeeded {
            self.poke.notify_one();
        }
    }

    /// Saves a just-started session in `root` under the project.
    fn save_new(
        &mut self,
        project_id: ProjectId,
        root: &Path,
        short_id: &str,
    ) -> Result<Thread, String> {
        let now = now_ms();
        let new = NewThread {
            project_id,
            short_id: short_id.to_owned(),
            cwd: root.to_owned(),
            created_at: now,
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
            branch: None,
            pinned_at: None,
            settled_override: None,
            settled_at: None,
            unsettled_at: None,
            last_activity_at: now,
            last_visited_at: now,
        };
        let thread = unpolled(&self.services.session_host, &row);
        self.rows.push(row);
        Ok(thread)
    }

    /// Saves `root` as a project, unless it already is one, and shows it.
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
                    if !sessions.projects.iter().any(|project| project.id == id) {
                        sessions.projects.push(Project {
                            id,
                            title,
                            root,
                            created_at: from_ms(now),
                            threads: Vec::new(),
                        });
                    }
                }
                Err(error) => sessions.error = Some(error),
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
    /// stays and the reason shows.
    async fn delete(&mut self, id: ThreadId) {
        let (Some(short_id), Some(status)) = (self.short_id(id), self.status(id)) else {
            return;
        };
        if status != ThreadStatus::Gone
            && let Err(report) = self.services.session_host.remove(&short_id).await
        {
            self.fail(&report);
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
            if deleted.is_err() {
                sessions.error = Some(SAVE_FAILED.to_owned());
            }
        }
        (self.wake)();
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
        row.transcript_offset = scan.offset;
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

/// A project's title: its directory's name, else the whole path.
fn project_title(root: &Path) -> String {
    root.file_name().map_or_else(
        || root.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Milliseconds since the Unix epoch, now.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
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
    use crate::common::{Services, State};
    use crate::feat::sessions::session_host::{
        CreatedSession, SessionHost, SessionHostError, SessionHostService, SessionRecord,
    };
    use crate::feat::sessions::state::{
        ProjectId, SidebarItem, SidebarRow, Thread, ThreadId, ThreadStatus,
    };
    use crate::feat::sessions::store::{NewThread, SettledOverride, Store, StoreError, ThreadRow};
    use crate::feat::sessions::transcript::transcript_path;

    const PROJECT_ROOT: &str = "/tmp/orb";
    const NO_CLAUDE_DIR: &str = "/nonexistent/claude";
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

        fn set_list(&self, list: Result<Vec<SessionRecord>, String>) {
            *self.list.lock().unwrap_or_else(PoisonError::into_inner) = list;
        }
    }

    #[async_trait]
    impl SessionHost for FakeHost {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn create(&self, cwd: &Path) -> Result<CreatedSession, Report<SessionHostError>> {
            self.created_in
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(cwd.to_owned());
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
        let state = State::default();
        let actor = SessionsActor::restore(SessionsActorDeps {
            services: Services {
                session_host: SessionHostService::new(host.clone()),
            },
            state: state.clone(),
            store,
            claude_dir: claude_dir.to_owned(),
            wake: Arc::new(|| {}),
        });
        (actor, state)
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

    #[rstest::rstest]
    #[tokio::test]
    async fn created_thread_is_first_in_its_project() -> Result<(), Report<StoreError>> {
        // Given a saved thread in the orb project.
        let (store, _) = store_with_thread("aa")?;
        let project = orb_project(&store)?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When creating a session in it.
        actor.create(project, Path::new(PROJECT_ROOT)).await;

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
    async fn created_thread_is_selected() -> Result<(), Report<StoreError>> {
        // Given a saved thread, selected.
        let (store, _) = store_with_thread("aa")?;
        let project = orb_project(&store)?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When creating a session in its project.
        actor.create(project, Path::new(PROJECT_ROOT)).await;

        // Then the new thread is selected.
        assert_eq!(
            state.read().sessions.selected_id(),
            Some(saved(&actor.store, "bb")?.id),
            "the new thread should be selected"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case::succeeded(Ok("bb"))]
    #[case::failed(Err("Workspace not trusted"))]
    #[tokio::test]
    async fn create_ends_starting(
        #[case] outcome: Result<&str, &str>,
    ) -> Result<(), Report<StoreError>> {
        // Given a create in flight in the orb project.
        let store = Store::open_in_memory()?;
        let project = orb_project(&store)?;
        let host = FakeHost::creating(outcome);
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));
        state.write().sessions.starting = true;

        // When the create finishes.
        actor.create(project, Path::new(PROJECT_ROOT)).await;

        // Then nothing is starting any more.
        assert!(
            !state.read().sessions.starting,
            "a finished create should stop showing as starting"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_create_shows_the_reason() -> Result<(), Report<StoreError>> {
        // Given a host that refuses to start a session.
        let store = Store::open_in_memory()?;
        let project = orb_project(&store)?;
        let host = FakeHost::creating(Err("Workspace not trusted"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When creating a session.
        actor.create(project, Path::new(PROJECT_ROOT)).await;

        // Then the reason is the error.
        assert_eq!(
            state.read().sessions.error.as_deref(),
            Some("Workspace not trusted"),
            "the mode line should show why the create failed"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn failed_create_adds_no_thread() -> Result<(), Report<StoreError>> {
        // Given no threads and a host that refuses to start a session.
        let store = Store::open_in_memory()?;
        let project = orb_project(&store)?;
        let host = FakeHost::creating(Err("Workspace not trusted"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When creating a session.
        actor.create(project, Path::new(PROJECT_ROOT)).await;

        // Then there are still no threads.
        assert_eq!(
            state.read().sessions.threads().count(),
            0,
            "a failed create shouldn't add a thread"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn create_starts_the_session_in_the_projects_root() -> Result<(), Report<StoreError>> {
        // Given saved orb and web projects.
        let store = Store::open_in_memory()?;
        orb_project(&store)?;
        let web = store.add_project(Path::new("/tmp/web"), "web", 0)?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When creating a session in web.
        actor.create(web, Path::new("/tmp/web")).await;

        // Then the host starts it in web's root.
        assert_eq!(
            host.created_in(),
            vec![PathBuf::from("/tmp/web")],
            "a session should start in its project's root"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn created_thread_is_saved_under_its_project() -> Result<(), Report<StoreError>> {
        // Given saved orb and web projects.
        let store = Store::open_in_memory()?;
        orb_project(&store)?;
        let web = store.add_project(Path::new("/tmp/web"), "web", 0)?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, _state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When creating a session in web.
        actor.create(web, Path::new("/tmp/web")).await;

        // Then the saved thread belongs to web.
        assert_eq!(
            saved(&actor.store, "bb")?.project_id,
            web,
            "the new thread should be saved under the picked project"
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
}
