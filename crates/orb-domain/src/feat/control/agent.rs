//! Agents in panes, for the control socket: which agent a pane holds, the line
//! that starts one, waiting on orb's state until an agent starts, works or
//! stops, and what it replied: everything since orb's prompt, else its last
//! reply.

use std::fs;
use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::feat::control::protocol::{
    AgentKind, ApiError, ErrorCode, ReplyResult, StartedResult, WaitEnd, WaitResult, cut,
};
use crate::feat::control::target::{self, Target};
use crate::feat::control::validator;
use crate::feat::harness::Harness;
use crate::feat::sessions::state::{Agent, AgentStatus};
use crate::feat::sessions::transcript::{Message, Role};
use crate::ids::PaneId;
use crate::{AppState, State};

/// How the waits pace themselves; tests shrink it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    /// How often the state is read.
    pub tick: Duration,
    /// How often the sessions poll is poked.
    pub poke_every: Duration,
    /// How long `agent start` waits for the agent.
    pub start_limit: Duration,
    /// How long `agent prompt --wait` waits to see work start.
    pub stall_gate: Duration,
    /// How long `agent prompt --wait` keeps reading the transcript for an
    /// answer after the turn ended with none yet.
    pub reply_grace: Duration,
}

impl Timing {
    /// The pace a running orb uses.
    pub(crate) const REAL: Self = Self {
        tick: Duration::from_millis(100),
        poke_every: Duration::from_secs(1),
        start_limit: Duration::from_secs(30),
        stall_gate: Duration::from_secs(5),
        reply_grace: Duration::from_secs(2),
    };
}

/// Whether an agent with `status` may still be running. A failed agent
/// still runs: only its last turn failed, and it takes the next prompt.
pub(crate) fn live(status: AgentStatus) -> bool {
    !matches!(status, AgentStatus::Stopped | AgentStatus::Gone)
}

/// The agent bound to `pane`: a live one before a dead one.
pub(crate) fn pane_agent(app: &AppState, pane: PaneId) -> Option<&Agent> {
    app.sessions
        .all_agents()
        .filter(|agent| agent.pane.as_ref().is_some_and(|at| at.pane == pane))
        .min_by_key(|agent| !live(agent.status))
}

/// `kind`'s program and `args` as one shell line, each arg quoted by
/// [`quoted`].
pub(crate) fn shell_line(kind: AgentKind, args: &[String]) -> String {
    args.iter().fold(kind.program().to_owned(), |line, arg| {
        format!("{line} {}", quoted(arg))
    })
}

/// `word` in single quotes, each `'` written `'\''` and each `\` written
/// `'\\'`: the quote is closed, the character is escaped outside it, and the
/// quote reopens. POSIX shells and fish both parse the result as one word
/// holding `word` as is; fish, which reads `\'` and `\\` as escapes inside
/// single quotes, never sees a bare `\` there. Quoting covers parsing only:
/// a control character typed into a live prompt acts on the line editor
/// before the shell parses anything, so a typed line drops control
/// characters (`Pane::type_line`) and `plate add` and `plate update` refuse
/// links holding them.
pub fn quoted(word: &str) -> String {
    let inner: String = word
        .chars()
        .map(|character| match character {
            '\'' => "'\\''".to_owned(),
            '\\' => "'\\\\'".to_owned(),
            other => other.to_string(),
        })
        .collect();
    format!("'{inner}'")
}

/// Reads `state` every tick until `check` gives an answer, poking the poll
/// after the first read and then every `poke_every`; `None` once `deadline`
/// passes, `timeout` once `gone` says the caller hung up. No lock is held
/// across the sleep or the poke.
fn poll_until<T, F>(
    state: &State,
    poke: &dyn Fn(),
    gone: &dyn Fn() -> bool,
    timing: Timing,
    deadline: Option<Instant>,
    mut check: F,
) -> Option<Result<T, ApiError>>
where
    F: FnMut(&AppState) -> Option<Result<T, ApiError>>,
{
    let mut poked: Option<Instant> = None;
    loop {
        let outcome = check(&state.read());
        if outcome.is_some() {
            return outcome;
        }
        if gone() {
            return Some(Err(caller_left()));
        }
        let now = Instant::now();
        if deadline.is_some_and(|deadline| now >= deadline) {
            return None;
        }
        if poked.is_none_or(|at| now.duration_since(at) >= timing.poke_every) {
            poke();
            poked = Some(now);
        }
        thread::sleep(timing.tick);
    }
}

/// `pane` still there, in a session that isn't settled.
fn pane_open(app: &AppState, pane: PaneId) -> Result<(), ApiError> {
    let resolved = target::resolve(app, Some(Target::Pane(pane)), None)?;
    validator::not_settled(app, resolved.session)
}

/// Why a prompt can't go to `pane`'s agent, which waits on the user.
pub(crate) fn blocked(pane: PaneId) -> ApiError {
    ApiError::new(
        ErrorCode::AgentBlocked,
        "the agent waits for approval or input",
        format!(
            "orb pane read {} to see the question, then tell the user",
            pane.0
        ),
    )
}

/// Why `agent prompt --wait` ended without an answer: the agent stopped
/// to ask the user.
pub(crate) fn asked_user(pane: PaneId) -> ApiError {
    ApiError::new(
        ErrorCode::AgentBlocked,
        format!(
            "the agent in pane {} stopped to ask for an approval or an answer",
            pane.0
        ),
        format!(
            "orb pane read {} to see the question, then tell the user",
            pane.0
        ),
    )
}

/// Why there is nothing to prompt or wait on in `pane`.
pub(crate) fn no_agent(pane: PaneId) -> ApiError {
    ApiError::new(
        ErrorCode::NoAgent,
        format!("no live agent in pane {}", pane.0),
        format!("orb agent start --kind claude --pane {}", pane.0),
    )
}

/// Why a wait ended without an answer: nobody is listening any more.
pub(crate) fn caller_left() -> ApiError {
    ApiError::new(
        ErrorCode::Timeout,
        "the caller hung up",
        "keep the command running until orb replies",
    )
}

/// Why a wait on `pane`'s agent gave up.
fn ran_out(pane: PaneId) -> ApiError {
    ApiError::new(
        ErrorCode::Timeout,
        format!(
            "the agent in pane {} was still working at the timeout",
            pane.0
        ),
        format!(
            "orb agent get {0} shows its status; orb agent wait {0} waits again",
            pane.0
        ),
    )
}

/// `pane`'s live agent, once the pane is checked still open; `turn_failed`
/// when its last turn failed, `no_agent` when none is live.
fn live_agent(app: &AppState, pane: PaneId) -> Result<&Agent, ApiError> {
    let agent = running_agent(app, pane)?;
    if agent.status == AgentStatus::Failed {
        return Err(turn_failed(pane, None));
    }
    Ok(agent)
}

/// `pane`'s live agent, failed last turn or not, once the pane is checked
/// still open; `no_agent` when none is live.
fn running_agent(app: &AppState, pane: PaneId) -> Result<&Agent, ApiError> {
    pane_open(app, pane)?;
    pane_agent(app, pane)
        .filter(|agent| live(agent.status))
        .ok_or_else(|| no_agent(pane))
}

/// Whether `pane`'s agent wrote its transcript after `pasted`: a turn that
/// starts and ends between two polls still leaves that trace.
fn wrote_since(state: &State, pane: PaneId, pasted: SystemTime) -> bool {
    let transcript = pane_agent(&state.read(), pane).and_then(|agent| agent.transcript.clone());
    transcript
        .and_then(|path| fs::metadata(path).and_then(|meta| meta.modified()).ok())
        .is_some_and(|written| written > pasted)
}

/// Waits until `pane`'s agent is idle or blocked, through work and an
/// unknown status. An agent that fails is `turn_failed`, one that stops or
/// leaves `no_agent`, a closed pane `not_found`, a settled session
/// `settled`; `timeout` once `deadline` passes or `gone` says the caller
/// hung up.
pub(crate) fn wait_done(
    state: &State,
    pane: PaneId,
    poke: &dyn Fn(),
    gone: &dyn Fn() -> bool,
    timing: Timing,
    deadline: Option<Instant>,
) -> Result<WaitResult, ApiError> {
    let done = poll_until(state, poke, gone, timing, deadline, |app| {
        let agent = match live_agent(app, pane) {
            Ok(agent) => agent,
            Err(error) => return Some(Err(error)),
        };
        let status = match agent.status {
            AgentStatus::Idle => WaitEnd::Idle,
            AgentStatus::NeedsApproval | AgentStatus::NeedsInput => WaitEnd::Blocked,
            AgentStatus::Failed => return Some(Err(turn_failed(pane, None))),
            AgentStatus::Working
            | AgentStatus::Unknown
            | AgentStatus::Stopped
            | AgentStatus::Gone => return None,
        };
        Some(Ok(WaitResult { status }))
    });
    done.unwrap_or_else(|| Err(ran_out(pane)))
}

/// A prompt orb pasted: when, and how long the stall gate still runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Sent {
    /// When orb pasted it.
    pub at: SystemTime,
    /// How long is left to see the turn start.
    pub gate: Duration,
}

/// After the prompt `sent`: waits what is left of the stall gate to see a
/// turn underway, then [`wait_done`]; an agent whose last turn failed is
/// waited on like an idle one until then. Seeing none is `prompt_stalled`,
/// unless the agent wrote its transcript since the paste (a turn too short
/// to see). `deadline` covers both halves; when it comes before the gate,
/// running out is `timeout`, as is `gone` saying the caller hung up.
pub(crate) fn wait_prompted(
    state: &State,
    pane: PaneId,
    poke: &dyn Fn(),
    gone: &dyn Fn() -> bool,
    timing: Timing,
    deadline: Option<Instant>,
    sent: Sent,
) -> Result<WaitResult, ApiError> {
    let gate = Instant::now() + sent.gate;
    let first = deadline.map_or(gate, |deadline| deadline.min(gate));
    let seen = poll_until(
        state,
        poke,
        gone,
        timing,
        Some(first),
        |app| match running_agent(app, pane) {
            Ok(agent) => agent.status.in_progress().then_some(Ok(())),
            Err(error) => Some(Err(error)),
        },
    );
    match seen {
        Some(Err(error)) => return Err(error),
        None if first < gate => return Err(ran_out(pane)),
        None if !wrote_since(state, pane, sent.at) => {
            return Err(ApiError::new(
                ErrorCode::PromptStalled,
                format!(
                    "the agent in pane {} didn't start working within {} s",
                    pane.0,
                    timing.stall_gate.as_secs()
                ),
                format!(
                    "orb pane read {} to see why; don't send the prompt twice",
                    pane.0
                ),
            ));
        }
        Some(Ok(())) | None => {}
    }
    wait_done(state, pane, poke, gone, timing, deadline)
}

/// Waits for the agent started in `pane`: replies once it reads idle or
/// working; `agent_blocked` if it waits for approval or input; `timeout`
/// after `start_limit`, asking whether the integration is installed.
pub(crate) fn wait_started(
    state: &State,
    pane: PaneId,
    poke: &dyn Fn(),
    timing: Timing,
) -> Result<StartedResult, ApiError> {
    let deadline = Instant::now() + timing.start_limit;
    let started = poll_until(state, poke, &|| false, timing, Some(deadline), |app| {
        if let Err(error) = pane_open(app, pane) {
            return Some(Err(error));
        }
        let agent = pane_agent(app, pane)?;
        match agent.status {
            AgentStatus::Idle | AgentStatus::Working => Some(Ok(StartedResult {
                pane,
                agent: agent.id,
                harness: agent.harness.to_string(),
                status: agent.status,
            })),
            AgentStatus::NeedsApproval | AgentStatus::NeedsInput => Some(Err(blocked(pane))),
            AgentStatus::Unknown
            | AgentStatus::Stopped
            | AgentStatus::Gone
            | AgentStatus::Failed => None,
        }
    });
    started.unwrap_or_else(|| {
        Err(ApiError::new(
            ErrorCode::Timeout,
            format!(
                "no agent started in pane {} within {} s; is `orb integration install` done?",
                pane.0,
                timing.start_limit.as_secs()
            ),
            format!("orb pane read {} shows what the shell printed", pane.0),
        ))
    })
}

/// Why `pane`'s agent has nothing to reply with yet: `why`.
pub(crate) fn no_reply(pane: PaneId, why: &str) -> ApiError {
    ApiError::new(
        ErrorCode::NoReply,
        why,
        format!("orb agent wait {0}, then orb agent reply {0}", pane.0),
    )
}

/// Why the transcript at `path` of `pane`'s agent couldn't be read.
pub(crate) fn unreadable(path: &Path, error: &io::Error, pane: PaneId) -> ApiError {
    ApiError::new(
        ErrorCode::Failed,
        format!("couldn't read {}: {error}", path.display()),
        format!("orb pane read {} shows the agent's screen", pane.0),
    )
}

/// How many graphemes of a failed turn's text a refusal carries.
const FAILURE_LIMIT: usize = 300;

/// Why `pane`'s agent's turn gave no answer: its model call failed with
/// `text`, cut to 300 graphemes, else just that it failed.
pub(crate) fn turn_failed(pane: PaneId, text: Option<&str>) -> ApiError {
    ApiError::new(
        ErrorCode::TurnFailed,
        text.map_or_else(
            || format!("the agent in pane {} failed", pane.0),
            |text| cut(text, FAILURE_LIMIT),
        ),
        format!(
            "the agent's model call failed; tell the user, or retry later; orb pane read {} shows its screen",
            pane.0
        ),
    )
}

/// What `pane`'s agent wrote since orb's prompt at `prompted`, from the
/// `messages` read after it: every assistant message joined by a blank
/// line, at the last one's time; `turn_failed` when the last is a failure;
/// `no_reply` naming the prompt's time in `zone` when there is none yet.
pub(crate) fn since_prompt(
    messages: &[Message],
    prompted: SystemTime,
    zone: &TimeZone,
    pane: PaneId,
) -> Result<ReplyResult, ApiError> {
    let ended = messages
        .iter()
        .rfind(|message| matches!(message.role, Role::Assistant | Role::Failure));
    if let Some(failed) = ended.filter(|message| message.role == Role::Failure) {
        return Err(turn_failed(pane, Some(&failed.text)));
    }
    let answers: Vec<&Message> = messages
        .iter()
        .filter(|message| message.role == Role::Assistant)
        .collect();
    let Some(last) = answers.last() else {
        let time = Timestamp::try_from(prompted).map_or_else(
            |_| "an unknown time".to_owned(),
            |time| time.to_zoned(zone.clone()).strftime("%H:%M:%S").to_string(),
        );
        return Err(no_reply(
            pane,
            &format!("no answer since orb's prompt at {time}"),
        ));
    };
    Ok(ReplyResult {
        text: answers
            .iter()
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n"),
        at: (last.at > 0).then_some(last.at),
        truncated: false,
        chars: None,
    })
}

/// The reply of the last exchange that has one, in `transcript` as `reader`
/// reads it, for the agent in `pane`; `turn_failed` when that exchange ended
/// in a failed model call, `no_reply` when there is none, `failed` when the
/// file can't be read.
pub(crate) fn last_reply(
    reader: &dyn Harness,
    transcript: &Path,
    pane: PaneId,
) -> Result<ReplyResult, ApiError> {
    let (_, exchanges) = reader
        .exchanges(transcript)
        .map_err(|error| unreadable(transcript, &error, pane))?;
    let last = exchanges
        .into_iter()
        .rfind(|exchange| exchange.reply.is_some() || exchange.failure.is_some())
        .ok_or_else(|| no_reply(pane, "the agent hasn't replied yet"))?;
    if let Some(failure) = last.failure {
        return Err(turn_failed(pane, Some(&failure)));
    }
    let (text, written) = last
        .reply
        .ok_or_else(|| no_reply(pane, "the agent hasn't replied yet"))?;
    Ok(ReplyResult {
        text,
        at: written
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .and_then(|since| i64::try_from(since.as_millis()).ok()),
        truncated: false,
        chars: None,
    })
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate setup failures with `?` and assert on the outcome"
)]
mod tests {
    use std::path::Path;

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant, SystemTime};

    use super::{
        Sent, Timing, last_reply, shell_line, turn_failed, wait_done, wait_prompted, wait_started,
    };
    use crate::State;
    use crate::feat::control::protocol::{AgentKind, ErrorCode, WaitEnd, WaitResult};
    use crate::feat::harness::fake::FakeHarness;

    use crate::feat::sessions::state::{Agent, AgentStatus};
    use crate::feat::sessions::transcript::Exchange;
    use crate::ids::{AgentId, PaneId};
    use crate::intent_handler::test_support::{agent, launch, settle, state_with, stopped};

    const FAST: Timing = Timing {
        tick: Duration::from_millis(1),
        poke_every: Duration::from_millis(1),
        start_limit: Duration::from_millis(50),
        stall_gate: Duration::from_millis(50),
        reply_grace: Duration::from_millis(50),
    };

    /// A prompt pasted at `at`, the whole stall gate left.
    fn sent(at: SystemTime) -> Sent {
        Sent {
            at,
            gate: FAST.stall_gate,
        }
    }

    /// Pane 1 of session 1 holding stopped agent 1.
    fn stopped_pane() -> State {
        State::new(state_with(vec![stopped(1)], 1))
    }

    /// Agent 2 with `status`, bound to pane 1 of session 1, added to `state`
    /// as the hook's report would.
    fn bind_new_agent(state: &State, status: AgentStatus) {
        for project in &mut state.write().sessions.projects {
            project.agents.push(Agent {
                pane: Some(launch(1)),
                ..agent(2, status)
            });
        }
    }

    /// Agent `id` in `state` now reads `status`.
    fn set_status(state: &State, id: i64, status: AgentStatus) {
        for project in &mut state.write().sessions.projects {
            for found in project.agents.iter_mut().filter(|found| found.id.0 == id) {
                found.status = status;
            }
        }
    }

    /// An exchange whose reply is `reply`.
    fn exchange(reply: Option<&str>) -> Exchange {
        Exchange {
            reply: reply.map(|text| (text.to_owned(), None)),
            ..Exchange::default()
        }
    }

    /// `last_reply` from a claude transcript holding `exchanges`.
    fn reply_from(exchanges: Vec<Exchange>) -> Result<String, ErrorCode> {
        let reader = FakeHarness::new("claude", Vec::new()).with_exchanges(exchanges);
        last_reply(&reader, Path::new("/t"), PaneId(1))
            .map(|reply| reply.text)
            .map_err(|error| error.code)
    }

    #[rstest::rstest]
    fn start_wait_replies_once_a_new_agent_in_the_pane_turns_idle() {
        // Given pane 1 holding a stopped agent, and a poll that sees agent 2
        // start there idle.
        let state = stopped_pane();
        let poke = || bind_new_agent(&state, AgentStatus::Idle);

        // When waiting for an agent to start in pane 1.
        let started = wait_started(&state, PaneId(1), &poke, FAST);

        // Then the new agent is the one that started.
        assert_eq!(
            started
                .map(|started| started.agent)
                .map_err(|error| error.code),
            Ok(AgentId(2)),
            "the start should reply with the new agent"
        );
    }

    #[rstest::rstest]
    fn start_wait_times_out_when_no_agent_appears() {
        // Given pane 1 holding a stopped agent, and a poll that sees nothing.
        let state = stopped_pane();

        // When waiting for an agent to start in pane 1.
        let started = wait_started(&state, PaneId(1), &|| {}, FAST);

        // Then it times out, pointing at the integration.
        assert!(
            started
                .as_ref()
                .is_err_and(|error| error.code == ErrorCode::Timeout
                    && error.message.contains("orb integration install")),
            "no agent should time out naming the integration, got {started:?}"
        );
    }

    #[rstest::rstest]
    fn start_wait_on_an_agent_waiting_for_approval_is_agent_blocked() {
        // Given pane 1 holding a stopped agent, and a poll that sees agent 2
        // start there waiting for approval.
        let state = stopped_pane();
        let poke = || bind_new_agent(&state, AgentStatus::NeedsApproval);

        // When waiting for an agent to start in pane 1.
        let started = wait_started(&state, PaneId(1), &poke, FAST);

        // Then it is refused as blocked.
        assert_eq!(
            started
                .map(|started| started.agent)
                .map_err(|error| error.code),
            Err(ErrorCode::AgentBlocked),
            "an agent waiting for approval should be agent_blocked"
        );
    }

    #[rstest::rstest]
    fn prompt_wait_is_stalled_when_no_work_starts() {
        // Given idle agent 1 in pane 1 with no transcript, which never starts
        // working.
        let state = State::new(state_with(vec![agent(1, AgentStatus::Idle)], 1));

        // When waiting on a prompt with no deadline.
        let waited = wait_prompted(
            &state,
            PaneId(1),
            &|| {},
            &|| false,
            FAST,
            None,
            sent(SystemTime::now()),
        );

        // Then the prompt is stalled.
        assert_eq!(
            waited.map_err(|error| error.code),
            Err(ErrorCode::PromptStalled),
            "no work within the gate should be prompt_stalled"
        );
    }

    #[rstest::rstest]
    fn prompt_wait_returns_once_the_agent_goes_from_working_to_idle() {
        // Given idle agent 1 in pane 1, which the first poll sees working and
        // the third sees idle again.
        let state = State::new(state_with(vec![agent(1, AgentStatus::Idle)], 1));
        let polls = AtomicUsize::new(0);
        let poke = || match polls.fetch_add(1, Ordering::SeqCst) + 1 {
            1 => set_status(&state, 1, AgentStatus::Working),
            3 => set_status(&state, 1, AgentStatus::Idle),
            _ => {}
        };

        // When waiting on a prompt with no deadline.
        let waited = wait_prompted(
            &state,
            PaneId(1),
            &poke,
            &|| false,
            FAST,
            None,
            sent(SystemTime::now()),
        );

        // Then the turn ends idle.
        assert_eq!(
            waited.map_err(|error| error.code),
            Ok(WaitResult {
                status: WaitEnd::Idle
            }),
            "working then idle should end the wait idle"
        );
    }

    #[rstest::rstest]
    fn prompt_wait_on_an_agent_whose_last_turn_failed_waits_for_the_new_turn() {
        // Given agent 1 in pane 1 whose last turn failed, which the first
        // poll sees working on the new prompt and the third sees idle.
        let state = State::new(state_with(vec![agent(1, AgentStatus::Failed)], 1));
        let polls = AtomicUsize::new(0);
        let poke = || match polls.fetch_add(1, Ordering::SeqCst) + 1 {
            1 => set_status(&state, 1, AgentStatus::Working),
            3 => set_status(&state, 1, AgentStatus::Idle),
            _ => {}
        };

        // When waiting on a prompt with no deadline.
        let waited = wait_prompted(
            &state,
            PaneId(1),
            &poke,
            &|| false,
            FAST,
            None,
            sent(SystemTime::now()),
        );

        // Then the old failure doesn't end the wait; the new turn ends idle.
        assert_eq!(
            waited.map_err(|error| error.code),
            Ok(WaitResult {
                status: WaitEnd::Idle
            }),
            "a failed last turn should not end the wait on the next prompt"
        );
    }

    #[rstest::rstest]
    fn wait_on_a_blocked_agent_replies_blocked() {
        // Given agent 1 in pane 1 waiting for input.
        let state = State::new(state_with(vec![agent(1, AgentStatus::NeedsInput)], 1));

        // When waiting on pane 1.
        let waited = wait_done(&state, PaneId(1), &|| {}, &|| false, FAST, None);

        // Then the wait ends blocked.
        assert_eq!(
            waited.map_err(|error| error.code),
            Ok(WaitResult {
                status: WaitEnd::Blocked
            }),
            "an agent waiting for input should end the wait blocked"
        );
    }

    #[rstest::rstest]
    fn wait_ends_once_its_caller_hangs_up() {
        // Given working agent 1 in pane 1 and no deadline.
        let state = State::new(state_with(vec![agent(1, AgentStatus::Working)], 1));

        // When waiting on pane 1 for a caller that already hung up.
        let waited = wait_done(&state, PaneId(1), &|| {}, &|| true, FAST, None);

        // Then the wait gives up.
        assert_eq!(
            waited.map_err(|error| error.code),
            Err(ErrorCode::Timeout),
            "nobody is left to answer, so the wait should stop"
        );
    }

    #[rstest::rstest]
    fn wait_ends_with_no_agent_when_the_agent_stops() {
        // Given working agent 1 in pane 1, which the poll sees stop.
        let state = State::new(state_with(vec![agent(1, AgentStatus::Working)], 1));
        let poke = || set_status(&state, 1, AgentStatus::Stopped);

        // When waiting on pane 1.
        let waited = wait_done(&state, PaneId(1), &poke, &|| false, FAST, None);

        // Then there is no agent to wait on.
        assert_eq!(
            waited.map_err(|error| error.code),
            Err(ErrorCode::NoAgent),
            "a stopped agent should end the wait with no_agent"
        );
    }

    #[rstest::rstest]
    fn reply_is_the_last_exchange_with_a_reply() {
        // Given a transcript replying "first", then "second", then a prompt
        // not yet answered.
        let exchanges = vec![
            exchange(Some("first")),
            exchange(Some("second")),
            exchange(None),
        ];

        // When reading the last reply.
        let reply = reply_from(exchanges);

        // Then it is the last one written.
        assert_eq!(
            reply,
            Ok("second".to_owned()),
            "the reply should be the last exchange that has one"
        );
    }

    #[rstest::rstest]
    fn reply_without_any_reply_is_no_reply() {
        // Given a transcript whose only prompt has no reply yet.
        let exchanges = vec![exchange(None)];

        // When reading the last reply.
        let reply = reply_from(exchanges);

        // Then there is no reply.
        assert_eq!(
            reply,
            Err(ErrorCode::NoReply),
            "no reply in the transcript should be no_reply"
        );
    }

    #[rstest::rstest]
    #[case(AgentKind::Claude, &[], "claude")]
    #[case(AgentKind::Pi, &["--model", "opus"], "pi '--model' 'opus'")]
    #[case(AgentKind::Claude, &["it's"], r"claude 'it'\''s'")]
    #[case(AgentKind::Claude, &[r"a\'(cmd)"], r"claude 'a'\\''\''(cmd)'")]
    fn shell_line_quotes_each_argument(
        #[case] kind: AgentKind,
        #[case] args: &[&str],
        #[case] expected: &str,
    ) {
        // Given the agent's kind and arguments.
        let args: Vec<String> = args.iter().map(|&arg| arg.to_owned()).collect();

        // When building its shell line.
        let line = shell_line(kind, &args);

        // Then each argument is single-quoted.
        assert_eq!(line, expected, "the shell line should quote each argument");
    }

    #[rstest::rstest]
    fn prompt_wait_on_a_turn_too_short_to_see_replies_idle() -> std::io::Result<()> {
        // Given idle agent 1 in pane 1 whose transcript was written after the
        // paste, though orb never saw it working.
        let dir = tempfile::tempdir()?;
        let transcript = dir.path().join("t.jsonl");
        std::fs::write(&transcript, "{}")?;
        let state = State::new(state_with(
            vec![Agent {
                transcript: Some(transcript),
                ..agent(1, AgentStatus::Idle)
            }],
            1,
        ));

        // When waiting on a prompt pasted before that write.
        let waited = wait_prompted(
            &state,
            PaneId(1),
            &|| {},
            &|| false,
            FAST,
            None,
            sent(SystemTime::UNIX_EPOCH),
        );

        // Then the turn counts as done, not stalled.
        assert_eq!(
            waited.map_err(|error| error.code),
            Ok(WaitResult {
                status: WaitEnd::Idle
            }),
            "a transcript written since the paste means the turn ran"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn wait_timeout_hint_names_agent_get() {
        // Given working agent 1 in pane 1 that never finishes.
        let state = State::new(state_with(vec![agent(1, AgentStatus::Working)], 1));

        // When waiting on pane 1 past a deadline that has come.
        let waited = wait_done(
            &state,
            PaneId(1),
            &|| {},
            &|| false,
            FAST,
            Some(Instant::now()),
        );

        // Then the timeout points at the agent's status.
        let hint = waited.err().and_then(|error| error.hint);
        assert!(
            hint.as_deref()
                .is_some_and(|hint| hint.contains("orb agent get 1")),
            "a wait that ran out points at orb agent get, got {hint:?}"
        );
    }

    #[rstest::rstest]
    fn stalled_prompt_hint_says_to_read_the_pane() {
        // Given idle agent 1 in pane 1 with no transcript, which never starts
        // working.
        let state = State::new(state_with(vec![agent(1, AgentStatus::Idle)], 1));

        // When waiting on a prompt with no deadline.
        let waited = wait_prompted(
            &state,
            PaneId(1),
            &|| {},
            &|| false,
            FAST,
            None,
            sent(SystemTime::now()),
        );

        // Then the stall points at the pane's screen.
        let hint = waited.err().and_then(|error| error.hint);
        assert!(
            hint.as_deref()
                .is_some_and(|hint| hint.contains("orb pane read 1")),
            "a stalled prompt says to read the pane, got {hint:?}"
        );
    }

    #[rstest::rstest]
    fn failure_text_is_cut_to_300_graphemes() {
        // Given an error 400 graphemes long, each an e with a combining accent.
        let text = "e\u{301}".repeat(400);

        // When refusing with it.
        let error = turn_failed(PaneId(1), Some(&text));

        // Then the message keeps 299 of them and an ellipsis.
        assert_eq!(
            error.message,
            format!("{}…", "e\u{301}".repeat(299)),
            "the failure text should be cut to 300 graphemes"
        );
    }

    #[rstest::rstest]
    fn reply_whose_last_exchange_failed_is_turn_failed() {
        // Given a transcript replying "first", then a turn whose model call
        // failed.
        let exchanges = vec![
            exchange(Some("first")),
            Exchange {
                failure: Some("API Error: 500".to_owned()),
                ..exchange(Some("API Error: 500"))
            },
        ];

        // When reading the last reply.
        let reply = reply_from(exchanges);

        // Then the turn failed.
        assert_eq!(
            reply,
            Err(ErrorCode::TurnFailed),
            "a last exchange ending in a failure should be turn_failed"
        );
    }

    #[rstest::rstest]
    fn wait_on_a_failed_agent_is_turn_failed() {
        // Given failed agent 1 in pane 1.
        let state = State::new(state_with(vec![agent(1, AgentStatus::Failed)], 1));

        // When waiting on pane 1.
        let waited = wait_done(&state, PaneId(1), &|| {}, &|| false, FAST, None);

        // Then the wait ends at once as a failed turn.
        assert_eq!(
            waited.map_err(|error| error.code),
            Err(ErrorCode::TurnFailed),
            "a failed agent should end the wait with turn_failed"
        );
    }

    #[rstest::rstest]
    fn wait_on_a_failed_agent_in_a_settled_session_is_settled() {
        // Given failed agent 1 in pane 1, its session settled.
        let state = State::new(settle(
            state_with(vec![agent(1, AgentStatus::Failed)], 1),
            &[1],
        ));

        // When waiting on pane 1.
        let waited = wait_done(&state, PaneId(1), &|| {}, &|| false, FAST, None);

        // Then the settled session is refused first.
        assert_eq!(
            waited.map_err(|error| error.code),
            Err(ErrorCode::Settled),
            "a settled session should be refused before the failed agent"
        );
    }
}
