//! The preview actor — the owner of the preview's content.
//!
//! Every half second, and right away when asked, it checks the selected
//! thread's transcript, reads any new lines, rebuilds the conversation when it
//! changed, and shows it. The most recently shown threads stay loaded, so
//! switching back to one shows it at once.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use kameo::mailbox;
use kameo::prelude::{Actor, ActorRef, Context, Message, Spawn};
use tokio::sync::Notify;

use super::block::Block;
use super::conversation::Conversation;
use crate::common::{State, Wake};
use crate::feat::sessions::state::ThreadId;
use crate::feat::sessions::transcript::read_new_lines;

/// How long to wait between transcript checks.
const CHECK_EVERY: Duration = Duration::from_millis(500);
/// How many threads stay loaded.
const CACHED: usize = 4;

/// What the preview actor needs to start.
pub struct PreviewActorDeps {
    pub state: State,
    /// Tells the frontend to redraw.
    pub wake: Wake,
}

/// Owns the preview's content: which thread it shows and that thread's blocks.
pub struct PreviewActor {
    state: State,
    wake: Wake,
    /// Cuts the ticker's wait short so it checks now.
    poke: Arc<Notify>,
    /// Most recent last, at most [`CACHED`].
    recent: Vec<Loaded>,
}

/// A thread's transcript as read so far.
struct Loaded {
    thread: ThreadId,
    path: Option<PathBuf>,
    /// Bytes of the transcript read so far.
    offset: u64,
    conversation: Conversation,
    blocks: Arc<[Block]>,
}

impl Loaded {
    fn new(thread: ThreadId, path: Option<PathBuf>) -> Self {
        Self {
            thread,
            path,
            offset: 0,
            conversation: Conversation::default(),
            blocks: Arc::default(),
        }
    }
}

/// Check the selected thread's transcript now.
#[derive(Debug)]
pub struct Tick;

/// Show the selected thread's transcript now instead of waiting for the next
/// check.
#[derive(Debug)]
pub struct ShowPreview;

/// Spawns the preview actor with a mailbox that never refuses a message.
/// Must be called inside a tokio runtime.
pub fn spawn_preview_actor(deps: PreviewActorDeps) -> ActorRef<PreviewActor> {
    PreviewActor::spawn_with_mailbox(deps, mailbox::unbounded())
}

impl Actor for PreviewActor {
    type Args = PreviewActorDeps;
    type Error = kameo::error::Infallible;

    fn on_start(
        args: Self::Args,
        actor_ref: ActorRef<Self>,
    ) -> impl Future<Output = Result<Self, Self::Error>> + Send {
        let actor = Self::new(args);
        let poke = actor.poke.clone();
        tokio::spawn(async move {
            while actor_ref.ask(Tick).await.is_ok() {
                tokio::select! {
                    () = tokio::time::sleep(CHECK_EVERY) => {}
                    () = poke.notified() => {}
                }
            }
        });
        std::future::ready(Ok(actor))
    }
}

impl Message<Tick> for PreviewActor {
    type Reply = ();

    async fn handle(&mut self, _msg: Tick, _ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.refresh();
    }
}

impl Message<ShowPreview> for PreviewActor {
    type Reply = ();

    async fn handle(
        &mut self,
        _msg: ShowPreview,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.poke.notify_one();
    }
}

impl PreviewActor {
    fn new(deps: PreviewActorDeps) -> Self {
        let PreviewActorDeps { state, wake } = deps;
        Self {
            state,
            wake,
            poke: Arc::default(),
            recent: Vec::new(),
        }
    }

    /// Shows the selected thread's latest blocks, reading only what its
    /// transcript gained since the last check. Wakes the frontend only when
    /// the preview changed.
    fn refresh(&mut self) {
        let target = self
            .state
            .read()
            .sessions
            .selected_thread()
            .map(|thread| (thread.id, thread.transcript.clone(), thread.cwd.clone()));
        let Some((id, path, cwd)) = target else {
            let cleared = {
                let mut app = self.state.write();
                let shown = app.preview.thread.is_some();
                if shown {
                    app.preview.clear();
                }
                shown
            };
            if cleared {
                (self.wake)();
            }
            return;
        };
        let mut loaded = match self.recent.iter().position(|loaded| loaded.thread == id) {
            Some(i) => self.recent.remove(i),
            None => Loaded::new(id, None),
        };
        let mut changed = loaded.path != path;
        if changed {
            loaded = Loaded::new(id, path);
        }
        // ponytail: reads and parses on the actor's tokio worker (~20 ms for a
        // 20 MB transcript); move it to `spawn_blocking` if that ever stalls.
        if let Some(path) = &loaded.path
            && fs::metadata(path).is_ok_and(|meta| meta.len() != loaded.offset)
            && let Ok(new) = read_new_lines(path, loaded.offset)
        {
            if new.restarted {
                loaded.conversation = Conversation::default();
            }
            loaded.conversation.push_lines(&new.text);
            loaded.offset = new.offset;
            changed |= new.restarted || !new.text.is_empty();
        }
        if changed {
            loaded.blocks = loaded.conversation.blocks(&cwd).into();
        }
        let switched = self.state.read().preview.thread != Some(id);
        if switched || changed {
            self.state.write().preview.show(
                id,
                loaded.blocks.clone(),
                loaded.conversation.branch().map(str::to_owned),
                loaded.conversation.model().map(str::to_owned),
                loaded.conversation.skipped(),
                switched,
            );
            (self.wake)();
        }
        self.recent.push(loaded);
        if self.recent.len() > CACHED {
            self.recent.remove(0);
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate file failures with `?` and assert on the outcome"
)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::SystemTime;

    use serde_json::json;
    use tempfile::tempdir;

    use super::{PreviewActor, PreviewActorDeps};
    use crate::AppState;
    use crate::common::State;
    use crate::feat::preview::block::{Block, BlockId};
    use crate::feat::sessions::state::{
        Project, ProjectId, Sessions, SidebarItem, Thread, ThreadId, ThreadStatus,
    };

    /// A transcript line holding one prompt.
    fn prompt(uuid: &str, parent: Option<&str>, text: &str) -> String {
        let line = json!({
            "type": "user", "uuid": uuid, "parentUuid": parent,
            "message": {"role": "user", "content": text},
        });
        format!("{line}\n")
    }

    fn thread(id: i64, transcript: Option<PathBuf>) -> Thread {
        Thread {
            id: ThreadId(id),
            title: None,
            cwd: "/work/demo".into(),
            transcript,
            status: ThreadStatus::Idle,
            turn_started_at: None,
            attach_argv: vec![],
            branch: None,
            pinned_at: None,
            settled_at: None,
            active_since: SystemTime::UNIX_EPOCH,
            last_activity_at: SystemTime::UNIX_EPOCH,
            unseen: false,
        }
    }

    /// One project holding `threads`, with thread `selected` selected.
    fn state_with(threads: Vec<Thread>, selected: i64) -> State {
        State::new(AppState {
            sessions: Sessions {
                projects: vec![Project {
                    id: ProjectId(1),
                    title: "demo".into(),
                    root: "/work/demo".into(),
                    created_at: SystemTime::UNIX_EPOCH,
                    draft: None,
                    threads,
                }],
                cursor: Some(SidebarItem::Thread(ThreadId(selected))),
                ..Sessions::default()
            },
            ..AppState::default()
        })
    }

    /// Starts the actor on `state` the way `on_start` does, without the ticker.
    fn start(state: &State) -> PreviewActor {
        PreviewActor::new(PreviewActorDeps {
            state: state.clone(),
            wake: Arc::new(|| {}),
        })
    }

    /// Whose blocks the preview shows, and their text.
    fn shown(state: &State) -> (Option<ThreadId>, Vec<String>) {
        let app = state.read();
        let texts = app.preview.blocks.iter().map(Block::raw_text).collect();
        (app.preview.thread, texts)
    }

    fn append(path: &Path, text: &str) -> io::Result<()> {
        OpenOptions::new()
            .append(true)
            .open(path)?
            .write_all(text.as_bytes())
    }

    #[rstest::rstest]
    fn first_refresh_shows_the_selected_threads_blocks() -> io::Result<()> {
        // Given a selected thread whose transcript has one prompt.
        let dir = tempdir()?;
        let path = dir.path().join("a.jsonl");
        fs::write(&path, prompt("u1", None, "Fix the bug"))?;
        let state = state_with(vec![thread(1, Some(path))], 1);
        let mut actor = start(&state);

        // When refreshing.
        actor.refresh();

        // Then the preview shows that thread's prompt.
        assert_eq!(
            shown(&state),
            (Some(ThreadId(1)), vec!["Fix the bug".to_owned()]),
            "the selected thread's transcript should be shown"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn grown_transcript_shows_the_new_block_while_following() -> io::Result<()> {
        // Given a shown thread, followed to the tail.
        let dir = tempdir()?;
        let path = dir.path().join("a.jsonl");
        fs::write(&path, prompt("u1", None, "Fix the bug"))?;
        let state = state_with(vec![thread(1, Some(path.clone()))], 1);
        let mut actor = start(&state);
        actor.refresh();

        // When its transcript gains a prompt and the actor refreshes.
        append(&path, &prompt("u2", Some("u1"), "Run the tests"))?;
        actor.refresh();

        // Then the new block is shown and the preview still follows.
        let texts = shown(&state).1;
        assert_eq!(
            (texts, state.read().preview.cursor),
            (
                vec!["Fix the bug".to_owned(), "Run the tests".to_owned()],
                None
            ),
            "a growing transcript should show its new block at the followed tail"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn switching_thread_starts_the_view_over() {
        // Given thread 1 shown, scrolled, with a block unfolded.
        let state = state_with(vec![thread(1, None), thread(2, None)], 1);
        let mut actor = start(&state);
        actor.refresh();
        {
            let preview = &mut state.write().preview;
            preview.cursor = Some(BlockId(0));
            preview.offset = 3;
            preview.expanded.insert(BlockId(0));
        }

        // When the selection moves to thread 2 and the actor refreshes.
        state.write().sessions.cursor = Some(SidebarItem::Thread(ThreadId(2)));
        actor.refresh();

        // Then the view follows from the top with every block folded.
        let app = state.read();
        assert_eq!(
            (
                app.preview.cursor,
                app.preview.offset,
                app.preview.expanded.is_empty()
            ),
            (None, 0, true),
            "another thread's preview should start over"
        );
    }

    #[rstest::rstest]
    fn switching_thread_shows_the_new_threads_blocks() -> io::Result<()> {
        // Given thread 1 shown, and thread 2 with its own transcript.
        let dir = tempdir()?;
        let (a, b) = (dir.path().join("a.jsonl"), dir.path().join("b.jsonl"));
        fs::write(&a, prompt("a1", None, "Fix the bug"))?;
        fs::write(&b, prompt("b1", None, "Write the docs"))?;
        let state = state_with(vec![thread(1, Some(a)), thread(2, Some(b))], 1);
        let mut actor = start(&state);
        actor.refresh();

        // When the selection moves to thread 2 and the actor refreshes.
        state.write().sessions.cursor = Some(SidebarItem::Thread(ThreadId(2)));
        actor.refresh();

        // Then thread 2's blocks are shown.
        assert_eq!(
            shown(&state),
            (Some(ThreadId(2)), vec!["Write the docs".to_owned()]),
            "the newly selected thread's transcript should be shown"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn cached_thread_stays_shown_after_its_transcript_is_deleted() -> io::Result<()> {
        // Given thread 1 shown, then thread 2, then thread 1's transcript deleted.
        let dir = tempdir()?;
        let (a, b) = (dir.path().join("a.jsonl"), dir.path().join("b.jsonl"));
        fs::write(&a, prompt("a1", None, "Fix the bug"))?;
        fs::write(&b, prompt("b1", None, "Write the docs"))?;
        let state = state_with(vec![thread(1, Some(a.clone())), thread(2, Some(b))], 1);
        let mut actor = start(&state);
        actor.refresh();
        state.write().sessions.cursor = Some(SidebarItem::Thread(ThreadId(2)));
        actor.refresh();
        fs::remove_file(&a)?;

        // When the selection moves back to thread 1 and the actor refreshes.
        state.write().sessions.cursor = Some(SidebarItem::Thread(ThreadId(1)));
        actor.refresh();

        // Then thread 1's cached blocks are shown.
        assert_eq!(
            shown(&state),
            (Some(ThreadId(1)), vec!["Fix the bug".to_owned()]),
            "a cached thread should show what was read before"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn selected_thread_without_a_transcript_shows_no_blocks() {
        // Given a selected thread whose transcript isn't located yet.
        let state = state_with(vec![thread(1, None)], 1);
        let mut actor = start(&state);

        // When refreshing.
        actor.refresh();

        // Then the preview is that thread's, with no blocks.
        assert_eq!(
            shown(&state),
            (Some(ThreadId(1)), Vec::new()),
            "a thread without a transcript should show no blocks"
        );
    }

    #[rstest::rstest]
    fn nothing_selected_clears_the_preview() -> io::Result<()> {
        // Given a shown thread.
        let dir = tempdir()?;
        let path = dir.path().join("a.jsonl");
        fs::write(&path, prompt("u1", None, "Fix the bug"))?;
        let state = state_with(vec![thread(1, Some(path))], 1);
        let mut actor = start(&state);
        actor.refresh();

        // When nothing is selected and the actor refreshes.
        state.write().sessions.cursor = None;
        actor.refresh();

        // Then the preview shows nothing.
        assert_eq!(
            shown(&state),
            (None, Vec::new()),
            "without a selection the preview should be empty"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn replaced_transcript_is_rebuilt_from_its_new_content() -> io::Result<()> {
        // Given a shown thread with two prompts.
        let dir = tempdir()?;
        let path = dir.path().join("a.jsonl");
        fs::write(
            &path,
            [
                prompt("u1", None, "Fix the bug"),
                prompt("u2", Some("u1"), "Run the tests"),
            ]
            .concat(),
        )?;
        let state = state_with(vec![thread(1, Some(path.clone()))], 1);
        let mut actor = start(&state);
        actor.refresh();

        // When the transcript is replaced by a shorter one reusing a uuid and
        // the actor refreshes.
        fs::write(&path, prompt("u1", None, "Hi"))?;
        actor.refresh();

        // Then only the new content is shown.
        assert_eq!(
            shown(&state).1,
            vec!["Hi".to_owned()],
            "a replaced transcript should be read again from the start"
        );
        Ok(())
    }
}
