//! The sessions actor — the owner of orb's threads, their statuses, and titles.
//!
//! At start it restores the saved threads into the shared state. Then it polls
//! the session host: every second while a turn is underway or orb is attached,
//! every five seconds otherwise, and right away when asked. Each poll maps the
//! host's records onto the threads, stamps when a turn starts, reads new
//! transcript lines for titles, and saves what changed. It also starts new
//! sessions in orb's launch directory.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use error_stack::Report;
use kameo::mailbox;
use kameo::prelude::{Actor, ActorRef, Context, Message, Reply, Spawn};
use tokio::sync::Notify;

use super::session_host::{SessionHostError, SessionHostService, SessionRecord};
use super::state::{Project, ProjectId, SidebarItem, Thread, ThreadStatus};
use super::store::{NewThread, SettledOverride, Store, ThreadRow};
use super::transcript::{locate, scan_title};
use crate::Focus;
use crate::common::{Services, State, Wake};

/// How long to wait between polls while a turn is underway or orb is attached.
const FAST_POLL: Duration = Duration::from_secs(1);
/// How long to wait between polls otherwise.
const SLOW_POLL: Duration = Duration::from_secs(5);

/// What the sessions actor needs to start.
pub struct SessionsActorDeps {
    pub services: Services,
    pub state: State,
    pub store: Store,
    /// Claude's config directory, where transcripts live.
    pub claude_dir: PathBuf,
    /// Where orb was launched; new sessions start here.
    pub launch_dir: PathBuf,
    /// Tells the frontend to redraw.
    pub wake: Wake,
}

/// Owns [`Sessions`](super::state::Sessions): the projects, the threads'
/// statuses and titles, and the latest `claude` error. The intent handler also
/// moves the selection and marks a create as starting.
pub struct SessionsActor {
    services: Services,
    state: State,
    store: Store,
    claude_dir: PathBuf,
    launch_dir: PathBuf,
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

/// Start a new session in orb's launch directory.
#[derive(Debug)]
pub struct CreateSession;

/// Poll now instead of waiting for the next tick.
#[derive(Debug)]
pub struct RefreshSessions;

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
        _msg: CreateSession,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.create().await;
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

impl SessionsActor {
    /// Shows the saved projects and threads, newest thread first, and selects
    /// the first thread.
    fn restore(deps: SessionsActorDeps) -> Self {
        let SessionsActorDeps {
            services,
            state,
            store,
            claude_dir,
            launch_dir,
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
                    .map(|row| thread(&services.session_host, row))
                    .collect(),
                id: project.id,
                title: project.title,
                root: project.root,
            })
            .collect();
        {
            let mut app = state.write();
            let sessions = &mut app.sessions;
            sessions.projects = projects;
            let first = sessions
                .threads()
                .next()
                .map(|thread| SidebarItem::Thread(thread.id));
            sessions.cursor = first;
            sessions.error = error;
        }
        wake();
        Self {
            services,
            state,
            store,
            claude_dir,
            launch_dir,
            wake,
            poke: Arc::default(),
            rows,
        }
    }

    /// Asks the host what every session is doing and shows it. Returns how
    /// long to wait before the next poll.
    async fn poll(&mut self) -> Duration {
        match self.services.session_host.list().await {
            Ok(records) => self.apply(&records),
            Err(report) => {
                self.state.write().sessions.error = Some(reason(&report));
                (self.wake)();
            }
        }
        let app = self.state.read();
        if app.sessions.any_in_progress() || app.focus == Focus::Attached {
            FAST_POLL
        } else {
            SLOW_POLL
        }
    }

    /// Updates every saved thread from its record and transcript, saves the
    /// ones that changed, then shows them all in one write.
    fn apply(&mut self, records: &[SessionRecord]) {
        let now = now_ms();
        let mut statuses = Vec::with_capacity(self.rows.len());
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
            update_row(row, record, status, now, &self.claude_dir);
            if *row != before && self.store.save_thread(row).is_err() {
                error = Some("couldn't save orb's state".to_owned());
            }
            statuses.push(status);
        }
        let changed = {
            let mut app = self.state.write();
            let sessions = &mut app.sessions;
            let mut changed = sessions.error != error;
            sessions.error = error;
            for (row, status) in self.rows.iter().zip(statuses) {
                if let Some(thread) = sessions
                    .projects
                    .iter_mut()
                    .flat_map(|project| &mut project.threads)
                    .find(|thread| thread.id == row.id)
                {
                    changed |= show(thread, row, status);
                }
            }
            changed
        };
        if changed {
            (self.wake)();
        }
    }

    /// Starts a session in the launch directory, saves it, and shows it
    /// selected at the top of its project.
    async fn create(&mut self) {
        let created = match self.services.session_host.create(&self.launch_dir).await {
            Ok(created) => self.save_new(&created.short_id),
            Err(report) => Err(reason(&report)),
        };
        let succeeded = created.is_ok();
        {
            let mut app = self.state.write();
            let sessions = &mut app.sessions;
            sessions.starting = false;
            match created {
                Ok((project_id, thread)) => {
                    sessions.cursor = Some(SidebarItem::Thread(thread.id));
                    sessions.error = None;
                    match sessions
                        .projects
                        .iter_mut()
                        .find(|project| project.id == project_id)
                    {
                        Some(project) => project.threads.insert(0, thread),
                        None => sessions.projects.push(Project {
                            id: project_id,
                            title: project_title(&self.launch_dir),
                            root: self.launch_dir.clone(),
                            threads: vec![thread],
                        }),
                    }
                }
                Err(error) => sessions.error = Some(error),
            }
        }
        (self.wake)();
        if succeeded {
            self.poke.notify_one();
        }
    }

    /// Saves a just-started session under the launch directory's project.
    fn save_new(&mut self, short_id: &str) -> Result<(ProjectId, Thread), String> {
        let now = now_ms();
        let saved = self
            .store
            .upsert_project(&self.launch_dir, &project_title(&self.launch_dir), now)
            .and_then(|project_id| {
                let row = NewThread {
                    project_id,
                    short_id: short_id.to_owned(),
                    cwd: self.launch_dir.clone(),
                    created_at: now,
                };
                self.store
                    .insert_thread(&row)
                    .map(|thread_id| (project_id, thread_id))
            });
        let Ok((project_id, id)) = saved else {
            return Err("couldn't save the new session".to_owned());
        };
        let row = ThreadRow {
            id,
            project_id,
            short_id: short_id.to_owned(),
            session_id: None,
            title: None,
            custom_title: None,
            cwd: self.launch_dir.clone(),
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
        let thread = thread(&self.services.session_host, &row);
        self.rows.push(row);
        Ok((project_id, thread))
    }
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

/// Shows a saved thread's title, turn stamp, and transcript, and `status`,
/// on `thread`. Returns whether anything visible changed.
fn show(thread: &mut Thread, row: &ThreadRow, status: ThreadStatus) -> bool {
    let turn_started_at = row.turn_started_at.map(from_ms);
    let title = display_title(row);
    let changed = thread.status != status
        || thread.title != title
        || thread.turn_started_at != turn_started_at
        || thread.transcript != row.transcript_path;
    thread.status = status;
    thread.title = title;
    thread.turn_started_at = turn_started_at;
    thread.transcript.clone_from(&row.transcript_path);
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

/// How a saved thread looks before its first poll.
fn thread(host: &SessionHostService, row: &ThreadRow) -> Thread {
    Thread {
        id: row.id,
        title: display_title(row),
        cwd: row.cwd.clone(),
        transcript: row.transcript_path.clone(),
        status: ThreadStatus::Unknown,
        turn_started_at: row.turn_started_at.map(from_ms),
        attach_argv: host.attach_argv(&row.short_id),
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

    use super::{FAST_POLL, SLOW_POLL, SessionsActor, SessionsActorDeps};
    use crate::Focus;
    use crate::common::{Services, State};
    use crate::feat::sessions::session_host::{
        CreatedSession, SessionHost, SessionHostError, SessionHostService, SessionRecord,
    };
    use crate::feat::sessions::state::{ThreadId, ThreadStatus};
    use crate::feat::sessions::store::{NewThread, Store, StoreError, ThreadRow};
    use crate::feat::sessions::transcript::transcript_path;

    const LAUNCH_DIR: &str = "/tmp/orb";
    const NO_CLAUDE_DIR: &str = "/nonexistent/claude";

    /// A session host whose answers the test scripts.
    struct FakeHost {
        list: Mutex<Result<Vec<SessionRecord>, String>>,
        create: Result<String, String>,
        remove: Result<(), String>,
        /// The sessions `stop` was called on, in order.
        stopped: Mutex<Vec<String>>,
        /// The sessions `remove` was called on, in order.
        removed: Mutex<Vec<String>>,
    }

    impl FakeHost {
        fn listing(records: Vec<SessionRecord>) -> Arc<Self> {
            Arc::new(Self {
                list: Mutex::new(Ok(records)),
                create: Err("no create scripted".to_owned()),
                remove: Ok(()),
                stopped: Mutex::default(),
                removed: Mutex::default(),
            })
        }

        fn creating(create: Result<&str, &str>) -> Arc<Self> {
            Arc::new(Self {
                list: Mutex::new(Ok(Vec::new())),
                create: create.map(str::to_owned).map_err(str::to_owned),
                remove: Ok(()),
                stopped: Mutex::default(),
                removed: Mutex::default(),
            })
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

        async fn create(&self, _cwd: &Path) -> Result<CreatedSession, Report<SessionHostError>> {
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

    /// A store holding one thread in the launch directory's project.
    fn store_with_thread(short_id: &str) -> Result<(Store, ThreadId), Report<StoreError>> {
        let store = Store::open_in_memory()?;
        let project_id = store.upsert_project(Path::new(LAUNCH_DIR), "orb", 0)?;
        let id = store.insert_thread(&NewThread {
            project_id,
            short_id: short_id.to_owned(),
            cwd: PathBuf::from(LAUNCH_DIR),
            created_at: 0,
        })?;
        Ok((store, id))
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
            launch_dir: PathBuf::from(LAUNCH_DIR),
            wake: Arc::new(|| {}),
        });
        (actor, state)
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
        // Given a saved thread in the launch directory's project.
        let (store, _) = store_with_thread("aa")?;
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When creating a session.
        actor.create().await;

        // Then the new thread is the project's first.
        let first = state
            .read()
            .sessions
            .projects
            .first()
            .and_then(|project| project.threads.first())
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
        let host = FakeHost::creating(Ok("bb"));
        let (mut actor, state) = start(store, &host, Path::new(NO_CLAUDE_DIR));

        // When creating a session.
        actor.create().await;

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
        // Given a create in flight.
        let host = FakeHost::creating(outcome);
        let (mut actor, state) = start(Store::open_in_memory()?, &host, Path::new(NO_CLAUDE_DIR));
        state.write().sessions.starting = true;

        // When the create finishes.
        actor.create().await;

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
        let host = FakeHost::creating(Err("Workspace not trusted"));
        let (mut actor, state) = start(Store::open_in_memory()?, &host, Path::new(NO_CLAUDE_DIR));

        // When creating a session.
        actor.create().await;

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
        let host = FakeHost::creating(Err("Workspace not trusted"));
        let (mut actor, state) = start(Store::open_in_memory()?, &host, Path::new(NO_CLAUDE_DIR));

        // When creating a session.
        actor.create().await;

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
    async fn poll_titles_a_thread_with_its_transcripts_prompt() -> Result<(), Report<StoreError>> {
        // Given a thread whose session's transcript has a prompt.
        let claude_dir = tempfile::tempdir().change_context(StoreError)?;
        let path = transcript_path(claude_dir.path(), Path::new(LAUNCH_DIR), "s1");
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
        let path = transcript_path(claude_dir.path(), Path::new(LAUNCH_DIR), "s1");
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
        let path = transcript_path(claude_dir.path(), Path::new(LAUNCH_DIR), "s1");
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
        let b = store.upsert_project(Path::new("/b"), "b", 2)?;
        let a = store.upsert_project(Path::new("/a"), "a", 1)?;
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
    fn restore_selects_the_first_thread() -> Result<(), Report<StoreError>> {
        // Given two saved threads, created at 10 and 20.
        let store = Store::open_in_memory()?;
        let project_id = store.upsert_project(Path::new(LAUNCH_DIR), "orb", 0)?;
        let insert = |short_id: &str, created_at| {
            store.insert_thread(&NewThread {
                project_id,
                short_id: short_id.to_owned(),
                cwd: PathBuf::from(LAUNCH_DIR),
                created_at,
            })
        };
        insert("old", 10)?;
        let newest = insert("new", 20)?;

        // When the actor starts.
        let (_actor, state) = start(
            store,
            &FakeHost::listing(Vec::new()),
            Path::new(NO_CLAUDE_DIR),
        );

        // Then the newest thread is selected.
        assert_eq!(
            state.read().sessions.selected_id(),
            Some(newest),
            "the first thread in the sidebar should be selected"
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
}
