//! The search actor — owns `AppState.search` and the search index's only
//! connection.
//!
//! At startup it indexes every live thread's transcript, newest chat first,
//! one transcript per message so queries run in between. Before each query it
//! reads what each transcript gained since the last read, and it answers
//! previews from the index.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::PathBuf;

use error_stack::Report;
use kameo::mailbox;
use kameo::prelude::{Actor, ActorRef, Context, Message, Spawn};

use super::index::{Hit, SearchIndex, SearchIndexError, ThreadKey, snippet};
use super::state::SearchProgress;
use crate::AppState;
use crate::common::{State, Wake};
use crate::feat::harness::{HarnessId, Harnesses};
use crate::feat::picker::list::PickerItem;
use crate::feat::picker::state::{PickerState, thread_label};
use crate::feat::sessions::sessions_actor::to_ms;
use crate::feat::sessions::state::{Project, Sessions, Thread};

/// What the search actor needs to start.
pub struct SearchActorDeps {
    pub state: State,
    /// Every harness, to read each thread's transcript in its own format.
    pub harnesses: Harnesses,
    /// `~/.orb/userdata/search.sqlite`.
    pub index_path: PathBuf,
    /// Tells the frontend to redraw.
    pub wake: Wake,
}

/// Owns [`SearchProgress`] and the search index. Writes the search picker's
/// rows and preview through their guarded setters.
pub struct SearchActor {
    state: State,
    harnesses: Harnesses,
    /// `None` when the index couldn't be opened; then every message is a no-op.
    index: Option<SearchIndex>,
    wake: Wake,
    /// Transcripts the startup indexing hasn't read yet, newest chat first,
    /// with the harness each thread runs in.
    queue: VecDeque<(ThreadKey, HarnessId, PathBuf)>,
}

/// Index the next queued transcript. The actor sends it to itself.
#[derive(Debug)]
pub struct IndexNext;

/// Catch up, then run the query text in the index.
#[derive(Debug)]
pub struct SearchTranscripts(pub String);

/// Load the exchange of hit `hit` into the search picker's preview.
#[derive(Debug)]
pub struct LoadSearchPreview {
    pub hit: i64,
    pub path: PathBuf,
    pub prompt_offset: u64,
}

/// Spawns the search actor with a mailbox that never refuses a message.
/// Must be called inside a tokio runtime, once `AppState` holds the threads.
pub fn spawn_search_actor(deps: SearchActorDeps) -> ActorRef<SearchActor> {
    SearchActor::spawn_with_mailbox(deps, mailbox::unbounded())
}

impl Actor for SearchActor {
    type Args = SearchActorDeps;
    type Error = kameo::error::Infallible;

    fn on_start(
        args: Self::Args,
        actor_ref: ActorRef<Self>,
    ) -> impl Future<Output = Result<Self, Self::Error>> + Send {
        let actor = Self::start(args);
        if !actor.queue.is_empty() {
            let _ = actor_ref.tell(IndexNext).try_send();
        }
        std::future::ready(Ok(actor))
    }
}

impl Message<IndexNext> for SearchActor {
    type Reply = ();

    async fn handle(
        &mut self,
        _msg: IndexNext,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.index_next(ctx.actor_ref());
    }
}

impl Message<SearchTranscripts> for SearchActor {
    type Reply = ();

    async fn handle(
        &mut self,
        SearchTranscripts(query): SearchTranscripts,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.search(&query);
    }
}

impl Message<LoadSearchPreview> for SearchActor {
    type Reply = ();

    async fn handle(
        &mut self,
        msg: LoadSearchPreview,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.preview(msg);
    }
}

impl SearchActor {
    /// Opens the index, drops the threads that are gone, queues every live
    /// transcript newest chat first and reports the indexing as not started.
    /// When the index can't be opened, says why in `AppState.search` and
    /// leaves the queue empty.
    fn start(deps: SearchActorDeps) -> Self {
        let SearchActorDeps {
            state,
            harnesses,
            index_path,
            wake,
        } = deps;
        let mut index = match SearchIndex::open(&index_path) {
            Ok(index) => index,
            Err(report) => {
                state.write().search.error = Some(reason(&report));
                wake();
                return Self {
                    state,
                    harnesses,
                    index: None,
                    wake,
                    queue: VecDeque::new(),
                };
            }
        };
        let (keys, queue) = {
            let app = state.read();
            let mut threads: Vec<&Thread> = live(&app.sessions).map(|(_, thread)| thread).collect();
            threads.sort_by_key(|thread| Reverse((thread.last_chat(), thread.id.0)));
            let keys: HashSet<ThreadKey> = threads.iter().map(|thread| key(thread)).collect();
            let queue: VecDeque<(ThreadKey, HarnessId, PathBuf)> = threads
                .iter()
                .filter_map(|thread| {
                    Some((
                        key(thread),
                        thread.harness.clone(),
                        thread.transcript.clone()?,
                    ))
                })
                .collect();
            (keys, queue)
        };
        // A failure leaves the rows; the next catch-up retries.
        let _ = index.retain_threads(&keys);
        state.write().search = SearchProgress {
            indexed: 0,
            total: queue.len(),
            error: None,
        };
        wake();
        Self {
            state,
            harnesses,
            index: Some(index),
            wake,
            queue,
        }
    }

    /// Indexes the next queued transcript and counts it, then asks for the
    /// one after while any are left. A failure leaves that transcript's saved
    /// offset alone, so the next catch-up retries it. A thread whose harness
    /// orb doesn't know is counted but not read.
    fn index_next(&mut self, me: &ActorRef<Self>) {
        let Some((key, harness, path)) = self.queue.pop_front() else {
            return;
        };
        if let Some(index) = &mut self.index
            && let Some(harness) = self.harnesses.get(&harness)
        {
            let _ = index.index_transcript(key, &path, |path, offset, prompt_offset| {
                harness.messages(path, offset, prompt_offset)
            });
        }
        self.state.write().search.indexed += 1;
        (self.wake)();
        if !self.queue.is_empty() {
            let _ = me.tell(IndexNext).try_send();
        }
    }

    /// Brings the index up to date and lists what `query` finds in the open
    /// search picker. Skipped when `query` is no longer the picker's typed
    /// text, which also collapses keystrokes queued behind a slow query. A
    /// failed query writes no rows. Then shows the first row's preview, since
    /// no keystroke selects it.
    fn search(&mut self, query: &str) {
        let Some(index) = &mut self.index else {
            return;
        };
        if self
            .state
            .read()
            .picker
            .as_ref()
            .and_then(PickerState::search_query)
            != Some(query)
        {
            return;
        }
        catch_up(index, &self.state, &self.harnesses, &self.queue);
        let Ok(found) = index.query(query) else {
            return;
        };
        let items = rows(&self.state.read(), found.hits);
        let first = {
            let mut app = self.state.write();
            let Some(picker) = &mut app.picker else {
                return;
            };
            picker.show_hits(query, items, found.overflow);
            picker
                .selected_hit()
                .map(|(hit, path, prompt_offset)| LoadSearchPreview {
                    hit,
                    path: path.to_owned(),
                    prompt_offset,
                })
        };
        if let Some(request) = first {
            self.preview(request);
        }
        (self.wake)();
    }

    /// Shows the exchange of hit `hit` in the search picker's preview,
    /// unless another row is selected by now. A failed read writes nothing.
    fn preview(&self, request: LoadSearchPreview) {
        let LoadSearchPreview {
            hit,
            path,
            prompt_offset,
        } = request;
        let Some(index) = &self.index else {
            return;
        };
        let Ok(messages) = index.exchange(&path, prompt_offset) else {
            return;
        };
        if let Some(picker) = &mut self.state.write().picker {
            picker.show_search_preview(hit, messages);
        }
        (self.wake)();
    }
}

/// Drops the rows of threads gone from `state`, then reads what each live
/// thread's current transcript gained since the last read. Transcripts still
/// in the startup `queue` are left to it, and a missing file, or one whose
/// thread's harness orb doesn't know, is skipped.
fn catch_up(
    index: &mut SearchIndex,
    state: &State,
    harnesses: &Harnesses,
    queue: &VecDeque<(ThreadKey, HarnessId, PathBuf)>,
) {
    let (keys, current) = {
        let app = state.read();
        let keys: HashSet<ThreadKey> = live(&app.sessions).map(|(_, thread)| key(thread)).collect();
        let current: Vec<(ThreadKey, HarnessId, PathBuf)> = live(&app.sessions)
            .filter_map(|(_, thread)| {
                Some((
                    key(thread),
                    thread.harness.clone(),
                    thread.transcript.clone()?,
                ))
            })
            .collect();
        (keys, current)
    };
    let _ = index.retain_threads(&keys);
    for (key, harness, path) in current {
        if queue.iter().any(|(_, _, queued)| *queued == path) {
            continue;
        }
        let Some(harness) = harnesses.get(&harness) else {
            continue;
        };
        let Ok(len) = fs::metadata(&path).map(|meta| meta.len()) else {
            continue;
        };
        // An unreadable saved offset counts as changed.
        if index.transcript_offset(&path).ok().flatten() != Some(len) {
            let _ = index.index_transcript(key, &path, |path, offset, prompt_offset| {
                harness.messages(path, offset, prompt_offset)
            });
        }
    }
}

/// `hits` as picker rows, labelled from `app`. A hit whose thread (by id and
/// creation time) isn't live any more is left out.
fn rows(app: &AppState, hits: Vec<Hit>) -> Vec<PickerItem> {
    let labels: HashMap<ThreadKey, (String, usize)> = live(&app.sessions)
        .map(|(project, thread)| (key(thread), thread_label(project, thread)))
        .collect();
    hits.into_iter()
        .filter_map(|hit| {
            let (label, split) = labels.get(&(hit.thread, hit.born))?.clone();
            let (snippet, lit) = snippet(&hit.text, &hit.lit);
            Some(PickerItem::Hit {
                id: hit.id,
                thread: hit.thread,
                label,
                split,
                snippet,
                lit,
                text_lit: hit.lit,
                path: hit.path,
                prompt_offset: hit.prompt_offset,
            })
        })
        .collect()
}

/// How the index names `thread`.
fn key(thread: &Thread) -> ThreadKey {
    (thread.id, to_ms(thread.created_at))
}

/// Every thread with its project, minus those being deleted. Settled and
/// `Gone` threads count: their messages stay searchable.
fn live(sessions: &Sessions) -> impl Iterator<Item = (&Project, &Thread)> {
    sessions
        .projects
        .iter()
        .flat_map(|project| project.threads.iter().map(move |thread| (project, thread)))
        .filter(|(_, thread)| !sessions.deleting.contains(&thread.id))
}

/// What went wrong opening the index, for the picker's error line.
fn reason(report: &Report<SearchIndexError>) -> String {
    report
        .downcast_ref::<rusqlite::Error>()
        .map(ToString::to_string)
        .or_else(|| {
            report
                .downcast_ref::<std::io::Error>()
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| "the index can't be opened".to_owned())
}

#[cfg(test)]
mod tests {
    use crate::feat::harness::HarnessId;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant, UNIX_EPOCH};

    use serde_json::json;

    use kameo::prelude::ActorRef;

    use super::{
        LoadSearchPreview, SearchActor, SearchActorDeps, SearchTranscripts, spawn_search_actor,
    };
    use crate::common::State;
    use crate::feat::git::git_cli::GitCli;
    use crate::feat::git::git_service::GitService;
    use crate::feat::harness::Harnesses;
    use crate::feat::harness::claude::ClaudeCode;
    use crate::feat::harness::claude::supervisor::ClaudeSupervisor;
    use crate::feat::harness::claude::transcript::read_messages;
    use crate::feat::harness::claude::trust::ClaudeConfigTrust;
    use crate::feat::picker::list::PickerItem;
    use crate::feat::picker::state::PickerState;
    use crate::feat::search::index::SearchIndex;
    use crate::feat::sessions::state::{
        Project, ProjectId, ProjectKind, Sessions, Thread, ThreadId, ThreadStatus,
    };
    use crate::feat::sessions::transcript::Role;
    use crate::{AppState, Focus};

    /// A prompt line the user typed.
    fn prompt(text: &str) -> String {
        json!({"type": "user", "timestamp": "2026-01-01T00:00:00Z", "message": {"content": text}})
            .to_string()
    }

    /// A reply line holding one text block Claude wrote.
    fn reply(text: &str) -> String {
        json!({
            "type": "assistant",
            "timestamp": "2026-01-01T00:00:01Z",
            "message": {"content": [{"type": "text", "text": text}]}
        })
        .to_string()
    }

    /// Writes `lines`, each ended by a newline, to `name` in `dir`.
    fn transcript(dir: &Path, name: &str, lines: &[String]) -> io::Result<PathBuf> {
        let path = dir.join(name);
        std::fs::write(&path, format!("{}\n", lines.join("\n")))?;
        Ok(path)
    }

    /// Thread `id`, created at unix ms `born_ms`, last chatted in at second
    /// `last_chat_secs`.
    fn thread(id: i64, born_ms: u64, transcript: Option<PathBuf>, last_chat_secs: u64) -> Thread {
        Thread {
            harness: HarnessId::new("claude"),
            id: ThreadId(id),
            title: None,
            cwd: "/tmp".into(),
            transcript,
            status: ThreadStatus::Idle,
            turn_started_at: None,
            pane: None,
            branch: None,
            pinned_at: None,
            settled_at: None,
            active_since: UNIX_EPOCH,
            created_at: UNIX_EPOCH + Duration::from_millis(born_ms),
            last_activity_at: UNIX_EPOCH + Duration::from_secs(last_chat_secs),
            unseen: false,
            group: None,
            model: None,
            permission: None,
        }
    }

    /// One project `orb` holding `threads`.
    fn app(threads: Vec<Thread>) -> AppState {
        AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "orb".to_owned(),
                    root: PathBuf::from("/repo"),
                    created_at: UNIX_EPOCH,
                    threads,
                    draft: None,
                    removed: false,
                    kind: ProjectKind::Normal,
                    groups: Vec::new(),
                }],
                ..Sessions::default()
            },
            ..AppState::default()
        }
    }

    fn deps(state: &State, index_path: PathBuf) -> SearchActorDeps {
        let claude = ClaudeCode::new(
            Arc::new(ClaudeSupervisor::new(Vec::new())),
            Arc::new(ClaudeConfigTrust::new(PathBuf::new())),
            PathBuf::new(),
            GitService::new(Arc::new(GitCli::new(Vec::new()))),
        );
        SearchActorDeps {
            state: state.clone(),
            harnesses: Harnesses::new(vec![Arc::new(claude)]),
            index_path,
            wake: Arc::new(|| {}),
        }
    }

    /// The threads whose messages a second connection to the index at `path`
    /// finds for `query`, in result order.
    fn indexed(path: &Path, query: &str) -> io::Result<Vec<ThreadId>> {
        let found = SearchIndex::open(path)
            .and_then(|index| index.query(query))
            .map_err(|report| io::Error::other(format!("{report:?}")))?;
        Ok(found.hits.into_iter().map(|hit| hit.thread).collect())
    }

    /// Opens the search picker with `text` typed into it.
    fn searching(state: &State, text: &str) {
        let mut picker = PickerState::search(Focus::Sidebar);
        for ch in text.chars() {
            let _ = picker.insert(ch);
        }
        state.write().picker = Some(picker);
    }

    /// The thread and snippet of each hit row the search picker shows.
    fn hit_rows(state: &State) -> Vec<(ThreadId, String)> {
        let app = state.read();
        app.picker
            .iter()
            .flat_map(PickerState::shown)
            .filter_map(|(item, _)| match item {
                PickerItem::Hit {
                    thread, snippet, ..
                } => Some((*thread, snippet.clone())),
                _ => None,
            })
            .collect()
    }

    /// Asks the actor to run `query`, waiting until it has.
    async fn search(actor: &ActorRef<SearchActor>, query: &str) -> io::Result<()> {
        actor
            .ask(SearchTranscripts(query.to_owned()))
            .await
            .map_err(|err| io::Error::other(format!("{err:?}")))
    }

    /// The preview request for the search picker's selected hit.
    fn selected_preview(state: &State) -> Option<LoadSearchPreview> {
        let app = state.read();
        let (hit, path, prompt_offset) = app.picker.as_ref()?.selected_hit()?;
        Some(LoadSearchPreview {
            hit,
            path: path.to_path_buf(),
            prompt_offset,
        })
    }

    /// Asks the actor to load `request`, waiting until it has.
    async fn load_preview(
        actor: &ActorRef<SearchActor>,
        request: Option<LoadSearchPreview>,
    ) -> io::Result<()> {
        let request = request.ok_or_else(|| io::Error::other("no hit selected"))?;
        actor
            .ask(request)
            .await
            .map_err(|err| io::Error::other(format!("{err:?}")))
    }

    /// The role and text of each message in the search picker's preview.
    fn preview_messages(state: &State) -> Option<Vec<(Role, String)>> {
        let app = state.read();
        let preview = app.picker.as_ref()?.search_preview()?;
        Some(
            preview
                .messages
                .iter()
                .map(|(_, role, text)| (*role, text.clone()))
                .collect(),
        )
    }

    /// Waits for the startup indexing to finish.
    async fn backfilled(state: &State) -> bool {
        eventually(|| {
            let search = &state.read().search;
            search.total > 0 && search.indexed == search.total
        })
        .await
    }

    /// Polls `done` until it holds, for up to 5 s.
    async fn eventually<F>(done: F) -> bool
    where
        F: Fn() -> bool,
    {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            if Instant::now() > deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        true
    }

    #[rstest::rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn startup_indexes_every_live_threads_transcript() -> io::Result<()> {
        // Given two threads whose transcripts say `alpha` and `bravo`.
        let dir = tempfile::tempdir()?;
        let a = transcript(dir.path(), "a.jsonl", &[prompt("alpha words")])?;
        let b = transcript(dir.path(), "b.jsonl", &[prompt("bravo words")])?;
        let state = State::new(app(vec![
            thread(1, 1_000, Some(a), 20),
            thread(2, 2_000, Some(b), 10),
        ]));
        let index_path = dir.path().join("search.sqlite");

        // When the actor starts and the backfill finishes.
        let _actor = spawn_search_actor(deps(&state, index_path.clone()));
        let done = eventually(|| {
            let search = &state.read().search;
            search.total == 2 && search.indexed == 2
        })
        .await;

        // Then the index file finds both words.
        let found = (
            indexed(&index_path, "alpha")?,
            indexed(&index_path, "bravo")?,
        );
        assert!(
            done && found == (vec![ThreadId(1)], vec![ThreadId(2)]),
            "both transcripts should be indexed, saw {found:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn unopenable_index_sets_the_search_error() -> io::Result<()> {
        // Given an index path that is a directory.
        let dir = tempfile::tempdir()?;
        let state = State::new(app(Vec::new()));

        // When the actor starts.
        let actor = spawn_search_actor(deps(&state, dir.path().to_path_buf()));
        actor.wait_for_startup().await;

        // Then search reports why it is unavailable.
        assert!(
            state.read().search.error.is_some(),
            "an unopenable index should set the search error"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn query_during_backfill_lists_only_indexed_transcripts() -> io::Result<()> {
        // Given threads 1 and 2 both saying `shared`, only thread 1's
        // transcript indexed by an earlier run, and the picker searching it.
        let dir = tempfile::tempdir()?;
        let a = transcript(dir.path(), "a.jsonl", &[prompt("shared words")])?;
        let b = transcript(dir.path(), "b.jsonl", &[prompt("shared words")])?;
        let index_path = dir.path().join("search.sqlite");
        SearchIndex::open(&index_path)
            .and_then(|mut index| index.index_transcript((ThreadId(1), 1_000), &a, read_messages))
            .map_err(|report| io::Error::other(format!("{report:?}")))?;
        let state = State::new(app(vec![
            thread(1, 1_000, Some(a), 20),
            thread(2, 2_000, Some(b), 10),
        ]));
        searching(&state, "shared");

        // When the query reaches the actor ahead of its startup indexing (a
        // current-thread runtime runs the actor only once the test awaits).
        let actor = spawn_search_actor(deps(&state, index_path));
        search(&actor, "shared").await?;

        // Then only the already indexed thread is listed; thread 2 waits for
        // the backfill.
        let rows = hit_rows(&state);
        assert_eq!(
            rows,
            vec![(ThreadId(1), "shared words".to_owned())],
            "only indexed transcripts should be listed during backfill"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn query_finds_a_message_added_since_the_last_query() -> io::Result<()> {
        // Given a backfilled transcript that then gains a prompt.
        let dir = tempfile::tempdir()?;
        let a = transcript(dir.path(), "a.jsonl", &[prompt("old words")])?;
        let state = State::new(app(vec![thread(1, 1_000, Some(a.clone()), 20)]));
        let actor = spawn_search_actor(deps(&state, dir.path().join("search.sqlite")));
        let done = backfilled(&state).await;
        transcript(
            dir.path(),
            "a.jsonl",
            &[prompt("old words"), prompt("fresh words")],
        )?;
        searching(&state, "fresh");

        // When searching for the new prompt.
        search(&actor, "fresh").await?;

        // Then it is listed.
        let rows = hit_rows(&state);
        assert!(
            done && rows == vec![(ThreadId(1), "fresh words".to_owned())],
            "a prompt added since the backfill should be found, saw {rows:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn query_drops_the_rows_of_a_thread_gone_from_state() -> io::Result<()> {
        // Given threads 1 and 2 saying `shared`, backfilled, then thread 2
        // gone from the app state.
        let dir = tempfile::tempdir()?;
        let a = transcript(dir.path(), "a.jsonl", &[prompt("shared words")])?;
        let b = transcript(dir.path(), "b.jsonl", &[prompt("shared words")])?;
        let state = State::new(app(vec![
            thread(1, 1_000, Some(a), 20),
            thread(2, 2_000, Some(b), 10),
        ]));
        let index_path = dir.path().join("search.sqlite");
        let actor = spawn_search_actor(deps(&state, index_path.clone()));
        let done = backfilled(&state).await;
        for project in &mut state.write().sessions.projects {
            project.threads.retain(|thread| thread.id != ThreadId(2));
        }
        searching(&state, "shared");

        // When searching.
        search(&actor, "shared").await?;

        // Then the index holds `shared` only for thread 1.
        let found = indexed(&index_path, "shared")?;
        assert!(
            done && found == vec![ThreadId(1)],
            "a gone thread's rows should leave the index, saw {found:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn query_still_matches_the_transcript_before_clear() -> io::Result<()> {
        // Given a backfilled transcript, then `/clear` moving the thread to a
        // new one.
        let dir = tempfile::tempdir()?;
        let a = transcript(dir.path(), "a.jsonl", &[prompt("before words")])?;
        let b = transcript(dir.path(), "b.jsonl", &[prompt("after words")])?;
        let state = State::new(app(vec![thread(1, 1_000, Some(a), 20)]));
        let actor = spawn_search_actor(deps(&state, dir.path().join("search.sqlite")));
        let done = backfilled(&state).await;
        for project in &mut state.write().sessions.projects {
            for thread in &mut project.threads {
                thread.transcript = Some(b.clone());
            }
        }
        searching(&state, "before");

        // When searching for the earlier transcript's prompt.
        search(&actor, "before").await?;

        // Then it is still listed.
        let rows = hit_rows(&state);
        assert!(
            done && rows == vec![(ThreadId(1), "before words".to_owned())],
            "the transcript before /clear should still match, saw {rows:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn stale_query_leaves_the_picker_rows_unchanged() -> io::Result<()> {
        // Given the picker listing the `alpha` row for `alpha`.
        let dir = tempfile::tempdir()?;
        let a = transcript(
            dir.path(),
            "a.jsonl",
            &[prompt("alpha words"), prompt("bravo words")],
        )?;
        let state = State::new(app(vec![thread(1, 1_000, Some(a), 20)]));
        let actor = spawn_search_actor(deps(&state, dir.path().join("search.sqlite")));
        let done = backfilled(&state).await;
        searching(&state, "alpha");
        search(&actor, "alpha").await?;

        // When a query for text no longer typed arrives.
        search(&actor, "bravo").await?;

        // Then the rows are still the `alpha` ones.
        let rows = hit_rows(&state);
        assert!(
            done && rows == vec![(ThreadId(1), "alpha words".to_owned())],
            "a stale query should leave the rows alone, saw {rows:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn query_labels_a_hit_with_its_project_and_thread_title() -> io::Result<()> {
        // Given a backfilled thread titled `fix` in project `orb`.
        let dir = tempfile::tempdir()?;
        let a = transcript(dir.path(), "a.jsonl", &[prompt("some words")])?;
        let state = State::new(app(vec![Thread {
            title: Some("fix".to_owned()),
            ..thread(1, 1_000, Some(a), 20)
        }]));
        let actor = spawn_search_actor(deps(&state, dir.path().join("search.sqlite")));
        let done = backfilled(&state).await;
        searching(&state, "words");

        // When searching.
        search(&actor, "words").await?;

        // Then the row is labelled `orb/fix` with the title after `orb/`.
        let label = state
            .read()
            .picker
            .as_ref()
            .and_then(PickerState::selected)
            .and_then(|item| match item {
                PickerItem::Hit { label, split, .. } => Some((label.clone(), *split)),
                _ => None,
            });
        assert!(
            done && label == Some(("orb/fix".to_owned(), 4)),
            "the hit should be labelled with its project and title, saw {label:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn preview_for_a_hit_no_longer_selected_is_not_written() -> io::Result<()> {
        // Given two hit rows for `shared` and the selection moved off the
        // first one after its preview was asked for.
        let dir = tempfile::tempdir()?;
        let a = transcript(
            dir.path(),
            "a.jsonl",
            &[prompt("shared one"), prompt("shared two")],
        )?;
        let state = State::new(app(vec![thread(1, 1_000, Some(a), 20)]));
        let actor = spawn_search_actor(deps(&state, dir.path().join("search.sqlite")));
        let done = backfilled(&state).await;
        searching(&state, "shared");
        search(&actor, "shared").await?;
        let request = selected_preview(&state);
        if let Some(picker) = &mut state.write().picker {
            picker.next();
        }

        // When the first row's preview arrives.
        load_preview(&actor, request).await?;

        // Then no preview is shown.
        let messages = preview_messages(&state);
        assert!(
            done && messages.is_none(),
            "a preview for a hit no longer selected should not be written, saw {messages:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn query_shows_the_first_hits_preview() -> io::Result<()> {
        // Given a prompt and its reply, and the search picker open on
        // `question`.
        let dir = tempfile::tempdir()?;
        let a = transcript(
            dir.path(),
            "a.jsonl",
            &[prompt("question words"), reply("answer text")],
        )?;
        let state = State::new(app(vec![thread(1, 1_000, Some(a), 20)]));
        let actor = spawn_search_actor(deps(&state, dir.path().join("search.sqlite")));
        let done = backfilled(&state).await;
        searching(&state, "question");

        // When the query runs.
        search(&actor, "question").await?;

        // Then the first row's exchange is already in the preview.
        let messages = preview_messages(&state);
        assert!(
            done && messages
                == Some(vec![
                    (Role::User, "question words".to_owned()),
                    (Role::Assistant, "answer text".to_owned()),
                ]),
            "a query should load the first hit's preview, saw {messages:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn preview_for_the_selected_hit_shows_its_exchange() -> io::Result<()> {
        // Given a prompt and its reply, with the prompt's hit row selected.
        let dir = tempfile::tempdir()?;
        let a = transcript(
            dir.path(),
            "a.jsonl",
            &[prompt("question words"), reply("answer text")],
        )?;
        let state = State::new(app(vec![thread(1, 1_000, Some(a), 20)]));
        let actor = spawn_search_actor(deps(&state, dir.path().join("search.sqlite")));
        let done = backfilled(&state).await;
        searching(&state, "question");
        search(&actor, "question").await?;

        // When the selected hit's preview is loaded.
        load_preview(&actor, selected_preview(&state)).await?;

        // Then it shows the prompt, then the reply.
        let messages = preview_messages(&state);
        assert!(
            done && messages
                == Some(vec![
                    (Role::User, "question words".to_owned()),
                    (Role::Assistant, "answer text".to_owned()),
                ]),
            "the preview should be the hit's exchange, saw {messages:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn backfill_counts_up_one_transcript_at_a_time() -> io::Result<()> {
        // Given two threads with transcripts, and a wake that records the
        // indexing progress each time the actor wakes the frontend.
        let dir = tempfile::tempdir()?;
        let a = transcript(dir.path(), "a.jsonl", &[prompt("alpha words")])?;
        let b = transcript(dir.path(), "b.jsonl", &[prompt("bravo words")])?;
        let state = State::new(app(vec![
            thread(1, 1_000, Some(a), 20),
            thread(2, 2_000, Some(b), 10),
        ]));
        let seen: Arc<Mutex<Vec<(usize, usize)>>> = Arc::default();
        let wake = {
            let state = state.clone();
            let seen = Arc::clone(&seen);
            Arc::new(move || {
                let progress = {
                    let search = &state.read().search;
                    (search.indexed, search.total)
                };
                seen.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(progress);
            })
        };

        // When the actor starts and the backfill finishes.
        let _actor = spawn_search_actor(SearchActorDeps {
            wake,
            ..deps(&state, dir.path().join("search.sqlite"))
        });
        let done = backfilled(&state).await;

        // Then the frontend saw 0, 1, then 2 of 2 indexed.
        let seen = seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert!(
            done && seen == vec![(0, 2), (1, 2), (2, 2)],
            "progress should count each transcript as it is indexed, saw {seen:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn query_lists_a_settled_threads_messages() -> io::Result<()> {
        // Given a backfilled settled thread.
        let dir = tempfile::tempdir()?;
        let a = transcript(dir.path(), "a.jsonl", &[prompt("settled words")])?;
        let state = State::new(app(vec![Thread {
            settled_at: Some(UNIX_EPOCH + Duration::from_secs(30)),
            ..thread(1, 1_000, Some(a), 20)
        }]));
        let actor = spawn_search_actor(deps(&state, dir.path().join("search.sqlite")));
        let done = backfilled(&state).await;
        searching(&state, "settled");

        // When searching.
        search(&actor, "settled").await?;

        // Then its message is listed.
        let rows = hit_rows(&state);
        assert!(
            done && rows == vec![(ThreadId(1), "settled words".to_owned())],
            "a settled thread's messages should match, saw {rows:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn query_ignores_the_project_filter() -> io::Result<()> {
        // Given a backfilled thread in `orb` and the sidebar filtered to
        // another project.
        let dir = tempfile::tempdir()?;
        let a = transcript(dir.path(), "a.jsonl", &[prompt("global words")])?;
        let state = State::new(app(vec![thread(1, 1_000, Some(a), 20)]));
        state.write().sessions.filter = Some(ProjectId(2));
        let actor = spawn_search_actor(deps(&state, dir.path().join("search.sqlite")));
        let done = backfilled(&state).await;
        searching(&state, "global");

        // When searching.
        search(&actor, "global").await?;

        // Then the `orb` thread's message is still listed.
        let rows = hit_rows(&state);
        assert!(
            done && rows == vec![(ThreadId(1), "global words".to_owned())],
            "the project filter should not narrow search, saw {rows:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn query_drops_the_rows_of_a_thread_being_deleted() -> io::Result<()> {
        // Given threads 1 and 2 saying `shared`, backfilled, then thread 2
        // being deleted.
        let dir = tempfile::tempdir()?;
        let a = transcript(dir.path(), "a.jsonl", &[prompt("shared words")])?;
        let b = transcript(dir.path(), "b.jsonl", &[prompt("shared words")])?;
        let state = State::new(app(vec![
            thread(1, 1_000, Some(a), 20),
            thread(2, 2_000, Some(b), 10),
        ]));
        let index_path = dir.path().join("search.sqlite");
        let actor = spawn_search_actor(deps(&state, index_path.clone()));
        let done = backfilled(&state).await;
        state.write().sessions.deleting.insert(ThreadId(2));
        searching(&state, "shared");

        // When searching.
        search(&actor, "shared").await?;

        // Then the index holds `shared` only for thread 1.
        let found = indexed(&index_path, "shared")?;
        assert!(
            done && found == vec![ThreadId(1)],
            "a thread being deleted should leave the index, saw {found:?}"
        );
        Ok(())
    }
}
