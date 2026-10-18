//! What orb last pasted into each pane's agent, and what that agent's turn
//! came to: everything it wrote since, whether the prompt still counts as
//! work underway, and the text of a failed turn.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use crate::State;
use crate::feat::control::agent::{self, Sent};
use crate::feat::control::protocol::{ApiError, ErrorCode, ReplyResult};
use crate::feat::harness::{HarnessId, Harnesses};
use crate::feat::sessions::state::AgentStatus;
use crate::feat::sessions::transcript::Role;
use crate::ids::{AgentId, PaneId};

/// What orb last pasted into a pane's agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Prompted {
    /// The agent that took it; an entry for another agent is stale.
    pub agent: AgentId,
    /// When orb pasted it.
    pub at: SystemTime,
    /// The agent's transcript then; `None` before its first prompt.
    pub transcript: Option<PathBuf>,
    /// The transcript's length then, in bytes.
    pub offset: u64,
}

impl Prompted {
    /// Where the answers to this prompt are read from, given the agent's
    /// transcript `now`: the same file from `offset`; a transcript first
    /// found since the prompt from its start; `None` once the transcript
    /// changed (a `/clear`).
    pub(crate) fn since(&self, now: Option<&Path>) -> Option<(PathBuf, u64)> {
        let now = now?;
        match &self.transcript {
            Some(then) if then == now => Some((now.to_path_buf(), self.offset)),
            Some(_) => None,
            None => Some((now.to_path_buf(), 0)),
        }
    }
}

/// What orb last pasted into each pane's agent. Clones share one map.
#[derive(Debug, Clone, Default)]
pub struct Prompts(Arc<Mutex<HashMap<PaneId, Prompted>>>);

impl Prompts {
    /// Remembers `prompted` as `pane`'s latest prompt.
    pub(crate) fn remember(&self, pane: PaneId, prompted: Prompted) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(pane, prompted);
    }

    /// `pane`'s latest prompt if `agent` took it; an entry left by another
    /// agent is dropped.
    pub(crate) fn for_agent(&self, pane: PaneId, agent: AgentId) -> Option<Prompted> {
        let mut map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if map
            .get(&pane)
            .is_some_and(|prompted| prompted.agent != agent)
        {
            map.remove(&pane);
        }
        map.get(&pane).cloned()
    }
}

/// A pane's agent as the state showed it, copied out so no lock is held
/// while its transcript is read.
#[derive(Debug, Clone)]
pub(crate) struct AgentView {
    pub id: AgentId,
    pub status: AgentStatus,
    pub harness: HarnessId,
    pub transcript: Option<PathBuf>,
}

/// The read side of agents' turns: orb's state, the harnesses that read
/// transcripts, and orb's prompts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Turns<'a> {
    pub state: &'a State,
    pub harnesses: &'a Harnesses,
    pub prompts: &'a Prompts,
}

impl Turns<'_> {
    /// `pane`'s agent: a live one before a dead one.
    pub(crate) fn agent(&self, pane: PaneId) -> Option<AgentView> {
        agent::pane_agent(&self.state.read(), pane).map(|found| AgentView {
            id: found.id,
            status: found.status,
            harness: found.harness.clone(),
            transcript: found.transcript.clone(),
        })
    }

    /// `agent reply` for `pane`: every assistant message since orb's prompt
    /// while the transcript is the one orb prompted, else the last
    /// exchange's reply.
    pub(crate) fn reply(&self, pane: PaneId) -> Result<ReplyResult, ApiError> {
        let view = self.agent(pane).ok_or_else(|| agent::no_agent(pane))?;
        let path = view
            .transcript
            .ok_or_else(|| agent::no_reply(pane, "no transcript yet"))?;
        let reader = self.harnesses.get(&view.harness).ok_or_else(|| {
            agent::no_reply(
                pane,
                &format!("orb can't read {} transcripts", view.harness),
            )
        })?;
        if let Some(prompted) = self.prompts.for_agent(pane, view.id)
            && let Some((since, offset)) = prompted.since(Some(&path))
        {
            let read = reader
                .messages(&since, offset, 0)
                .map_err(|error| agent::unreadable(&since, &error, pane))?;
            if !read.restarted {
                let zone = self.state.read().zone.0.clone();
                return agent::since_prompt(&read.messages, prompted.at, &zone, pane);
            }
        }
        agent::last_reply(reader.as_ref(), &path, pane)
    }

    /// How long orb's prompt to `pane`'s agent still counts as work underway
    /// at `now`: pasted under `gate` ago, the agent `Idle`, `Unknown` (its
    /// status not yet polled since) or still `Failed` from its last turn, and
    /// no answer in its transcript since the prompt. `None` otherwise.
    pub(crate) fn just_sent(&self, pane: PaneId, gate: Duration, now: SystemTime) -> Option<Sent> {
        let view = self.agent(pane)?;
        if !matches!(
            view.status,
            AgentStatus::Idle | AgentStatus::Unknown | AgentStatus::Failed
        ) {
            return None;
        }
        let prompted = self.prompts.for_agent(pane, view.id)?;
        let left = gate.checked_sub(now.duration_since(prompted.at).ok()?)?;
        if left.is_zero() {
            return None;
        }
        let answered = prompted
            .since(view.transcript.as_deref())
            .zip(self.harnesses.get(&view.harness))
            .and_then(|((path, offset), reader)| reader.messages(&path, offset, 0).ok())
            .is_some_and(|read| {
                read.messages
                    .iter()
                    .any(|message| matches!(message.role, Role::Assistant | Role::Failure))
            });
        (!answered).then_some(Sent {
            at: prompted.at,
            gate: left,
        })
    }

    /// `error`, with the failed turn's text from `pane`'s transcript when it
    /// is `turn_failed` and the transcript says why.
    pub(crate) fn failure(&self, pane: PaneId, error: ApiError) -> ApiError {
        if error.code != ErrorCode::TurnFailed {
            return error;
        }
        match self.reply(pane) {
            Err(found) if found.code == ErrorCode::TurnFailed => found,
            _ => error,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::{Prompted, Prompts, Turns};
    use crate::State;
    use crate::feat::control::agent::Sent;
    use crate::feat::control::protocol::{ErrorCode, ReplyResult};
    use crate::feat::harness::Harnesses;
    use crate::feat::harness::fake::FakeHarness;
    use crate::feat::sessions::state::{Agent, AgentStatus};
    use crate::feat::sessions::transcript::{Exchange, Message, Role};
    use crate::ids::{AgentId, PaneId};
    use crate::intent_handler::test_support::{agent, state_with};

    /// The transcript agent 1 writes in the tests.
    const TRANSCRIPT: &str = "/t/1.jsonl";

    /// 14:02:09 UTC on the first day of the epoch.
    fn prompt_time() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(14 * 3600 + 2 * 60 + 9)
    }

    /// Orb's prompt to agent `id`, pasted into `transcript` when it was
    /// `offset` bytes long.
    fn prompted(id: i64, transcript: Option<&str>, offset: u64) -> Prompted {
        Prompted {
            agent: AgentId(id),
            at: prompt_time(),
            transcript: transcript.map(PathBuf::from),
            offset,
        }
    }

    /// A message from `role` saying `text` at byte `offset`, written at
    /// unix ms `at`.
    fn message(role: Role, text: &str, offset: u64, at: i64) -> Message {
        Message {
            role,
            text: text.to_owned(),
            offset,
            prompt_offset: 0,
            at,
        }
    }

    /// An exchange whose reply is `reply`.
    fn exchange(reply: &str) -> Exchange {
        Exchange {
            reply: Some((reply.to_owned(), None)),
            ..Exchange::default()
        }
    }

    /// Pane 1 holding idle agent 1, writing [`TRANSCRIPT`].
    fn idle_pane() -> State {
        State::new(state_with(
            vec![Agent {
                transcript: Some(PathBuf::from(TRANSCRIPT)),
                ..agent(1, AgentStatus::Idle)
            }],
            1,
        ))
    }

    /// A claude harness whose transcripts hold `messages` and `exchanges`.
    fn claude(messages: Vec<Message>, exchanges: Vec<Exchange>) -> Harnesses {
        Harnesses::new(vec![Arc::new(
            FakeHarness::new("claude", Vec::new())
                .with_messages(messages)
                .with_exchanges(exchanges),
        )])
    }

    /// `agent reply` for pane 1 of `state`, read through `harnesses` and
    /// `prompts`.
    fn reply(
        state: &State,
        harnesses: &Harnesses,
        prompts: &Prompts,
    ) -> Result<ReplyResult, ErrorCode> {
        Turns {
            state,
            harnesses,
            prompts,
        }
        .reply(PaneId(1))
        .map_err(|error| error.code)
    }

    /// How long a prompt counts as work in the tests.
    const GATE: Duration = Duration::from_secs(5);

    /// Whether orb's prompt to idle agent 1 in pane 1, at byte 100 of its
    /// transcript, counts as work `after` the paste, the transcript holding
    /// `messages` since.
    fn just_sent(messages: Vec<Message>, after: Duration) -> Option<Sent> {
        let state = idle_pane();
        let harnesses = claude(messages, Vec::new());
        let prompts = Prompts::default();
        prompts.remember(PaneId(1), prompted(1, Some(TRANSCRIPT), 100));
        Turns {
            state: &state,
            harnesses: &harnesses,
            prompts: &prompts,
        }
        .just_sent(PaneId(1), GATE, prompt_time() + after)
    }

    #[rstest::rstest]
    fn just_sent_holds_inside_the_gate() {
        // Given orb's prompt to idle agent 1, its transcript holding only the
        // prompt's own line since.
        let since = vec![message(Role::User, "fix it", 100, 1_000)];

        // When asking a second after the paste.
        let sent = just_sent(since, Duration::from_secs(1));

        // Then the prompt counts as work for the rest of the gate.
        assert_eq!(
            sent,
            Some(Sent {
                at: prompt_time(),
                gate: Duration::from_secs(4),
            }),
            "a prompt pasted a second ago should count as work for 4 s more"
        );
    }

    #[rstest::rstest]
    fn just_sent_ends_after_the_gate() {
        // Given orb's prompt to idle agent 1, nothing written since.
        let since = Vec::new();

        // When asking six seconds after the paste.
        let sent = just_sent(since, Duration::from_secs(6));

        // Then the prompt no longer counts as work.
        assert_eq!(
            sent, None,
            "a prompt pasted past the gate should not count as work"
        );
    }

    #[rstest::rstest]
    fn just_sent_ends_once_the_transcript_holds_an_answer() {
        // Given orb's prompt to idle agent 1, answered in its transcript.
        let since = vec![
            message(Role::User, "fix it", 100, 1_000),
            message(Role::Assistant, "Fixed.", 200, 2_000),
        ];

        // When asking a second after the paste.
        let sent = just_sent(since, Duration::from_secs(1));

        // Then the turn already ran, so the idle status is true.
        assert_eq!(
            sent, None,
            "an answer since the prompt should end the just-sent rule"
        );
    }

    #[rstest::rstest]
    fn since_reads_the_prompted_transcript_from_its_offset() {
        // Given a prompt pasted into a transcript 120 bytes long.
        let prompt = prompted(1, Some(TRANSCRIPT), 120);

        // When asking where its answers are, the transcript unchanged.
        let since = prompt.since(Some(Path::new(TRANSCRIPT)));

        // Then they are read from that transcript at byte 120.
        assert_eq!(
            since,
            Some((PathBuf::from(TRANSCRIPT), 120)),
            "answers should be read past the length at the paste"
        );
    }

    #[rstest::rstest]
    fn since_reads_a_transcript_first_found_after_the_prompt_from_its_start() {
        // Given a prompt pasted into an agent with no transcript yet.
        let prompt = prompted(1, None, 0);

        // When asking where its answers are, a transcript found since.
        let since = prompt.since(Some(Path::new(TRANSCRIPT)));

        // Then the new transcript is read from its start.
        assert_eq!(
            since,
            Some((PathBuf::from(TRANSCRIPT), 0)),
            "a transcript first found after the prompt holds only its answers"
        );
    }

    #[rstest::rstest]
    fn since_is_none_once_the_transcript_changed() {
        // Given a prompt pasted into one transcript.
        let prompt = prompted(1, Some(TRANSCRIPT), 120);

        // When asking where its answers are, the agent now writing another.
        let since = prompt.since(Some(Path::new("/t/2.jsonl")));

        // Then there is nowhere to read them.
        assert_eq!(
            since, None,
            "a changed transcript no longer holds the prompt's answers"
        );
    }

    #[rstest::rstest]
    fn entry_left_by_another_agent_is_dropped() {
        // Given pane 1's prompt taken by agent 1.
        let prompts = Prompts::default();
        prompts.remember(PaneId(1), prompted(1, Some(TRANSCRIPT), 120));

        // When agent 2, now in pane 1, asks for it.
        let stale = prompts.for_agent(PaneId(1), AgentId(2));

        // Then it is gone, even for agent 1 after.
        assert_eq!(
            (stale, prompts.for_agent(PaneId(1), AgentId(1))),
            (None, None),
            "a prompt another agent took should be dropped"
        );
    }

    #[rstest::rstest]
    fn reply_joins_every_answer_since_orbs_prompt() {
        // Given orb's prompt at byte 100, then the prompt line and two answers.
        let state = idle_pane();
        let harnesses = claude(
            vec![
                message(Role::User, "fix it", 100, 1_000),
                message(Role::Assistant, "Looking.", 200, 2_000),
                message(Role::Assistant, "Fixed.", 300, 3_000),
            ],
            Vec::new(),
        );
        let prompts = Prompts::default();
        prompts.remember(PaneId(1), prompted(1, Some(TRANSCRIPT), 100));

        // When reading the reply.
        let replied = reply(&state, &harnesses, &prompts);

        // Then both answers come back, at the second's time.
        assert_eq!(
            replied,
            Ok(ReplyResult {
                text: "Looking.\n\nFixed.".to_owned(),
                at: Some(3_000),
            }),
            "every answer since the prompt should be joined by a blank line"
        );
    }

    #[rstest::rstest]
    fn reply_with_no_answer_since_the_prompt_is_no_reply() {
        // Given orb's prompt at 14:02:09 UTC, followed only by its own line.
        let state = idle_pane();
        let harnesses = claude(
            vec![message(Role::User, "fix it", 100, 1_000)],
            vec![exchange("an older answer")],
        );
        let prompts = Prompts::default();
        prompts.remember(PaneId(1), prompted(1, Some(TRANSCRIPT), 100));

        // When reading the reply.
        let replied = Turns {
            state: &state,
            harnesses: &harnesses,
            prompts: &prompts,
        }
        .reply(PaneId(1));

        // Then there is none yet, naming when orb prompted.
        assert!(
            replied.as_ref().is_err_and(
                |error| error.code == ErrorCode::NoReply && error.message.contains("14:02:09")
            ),
            "no answer since the prompt should be no_reply naming its time, got {replied:?}"
        );
    }

    #[rstest::rstest]
    fn reply_without_a_prompt_falls_back_to_the_last_exchange() {
        // Given an agent orb never prompted, whose last exchange replied.
        let state = idle_pane();
        let harnesses = claude(
            vec![message(Role::Assistant, "a message", 300, 3_000)],
            vec![exchange("the last answer")],
        );

        // When reading the reply.
        let replied = reply(&state, &harnesses, &Prompts::default());

        // Then it is the last exchange's reply.
        assert_eq!(
            replied.map(|reply| reply.text),
            Ok("the last answer".to_owned()),
            "with no prompt from orb the last exchange's reply should come back"
        );
    }

    #[rstest::rstest]
    fn reply_after_the_transcript_changed_falls_back_to_the_last_exchange() {
        // Given orb's prompt into another transcript than the agent's now.
        let state = idle_pane();
        let harnesses = claude(
            vec![message(Role::Assistant, "a message", 300, 3_000)],
            vec![exchange("the last answer")],
        );
        let prompts = Prompts::default();
        prompts.remember(PaneId(1), prompted(1, Some("/t/old.jsonl"), 100));

        // When reading the reply.
        let replied = reply(&state, &harnesses, &prompts);

        // Then it is the last exchange's reply.
        assert_eq!(
            replied.map(|reply| reply.text),
            Ok("the last answer".to_owned()),
            "a changed transcript should fall back to the last exchange's reply"
        );
    }

    #[rstest::rstest]
    fn just_sent_ends_once_the_transcript_holds_a_failure() {
        // Given orb's prompt to idle agent 1, its model call failed since.
        let since = vec![
            message(Role::User, "fix it", 100, 1_000),
            message(Role::Failure, "API Error: 500", 200, 2_000),
        ];

        // When asking a second after the paste.
        let sent = just_sent(since, Duration::from_secs(1));

        // Then the turn already ended, so the idle status is true.
        assert_eq!(
            sent, None,
            "a failure since the prompt should end the just-sent rule"
        );
    }

    #[rstest::rstest]
    fn since_prompt_ending_in_a_failure_is_turn_failed() {
        // Given orb's prompt at byte 100, an answer, then a failed model call.
        let state = idle_pane();
        let harnesses = claude(
            vec![
                message(Role::User, "fix it", 100, 1_000),
                message(Role::Assistant, "Looking.", 200, 2_000),
                message(Role::Failure, "API Error: Connection lost", 300, 3_000),
            ],
            Vec::new(),
        );
        let prompts = Prompts::default();
        prompts.remember(PaneId(1), prompted(1, Some(TRANSCRIPT), 100));

        // When reading the reply.
        let replied = Turns {
            state: &state,
            harnesses: &harnesses,
            prompts: &prompts,
        }
        .reply(PaneId(1));

        // Then the turn failed with the error's text, pointing at the screen.
        let error = replied.err();
        assert!(
            error.as_ref().is_some_and(|error| {
                error.code == ErrorCode::TurnFailed
                    && error.message == "API Error: Connection lost"
                    && error.hint.as_deref().is_some_and(|hint| {
                        hint.contains("the agent's model call failed")
                            && hint.contains("orb pane read 1")
                    })
            }),
            "a turn ending in a failure should be turn_failed with its text, got {error:?}"
        );
    }
}
