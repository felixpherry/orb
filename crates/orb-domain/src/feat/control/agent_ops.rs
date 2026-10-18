//! Agent requests: where `agent start` runs its agent, what a prompt, wait,
//! get or reply comes to, and carrying out a start or a prompt on the screen
//! loop, waiting on the agent after.

use std::fs;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::AppState;
use crate::command::SessionsCommand;
use crate::feat::control::agent::{self, Sent, Timing};
use crate::feat::control::change::Change;
use crate::feat::control::lists::agent_detail;
use crate::feat::control::pane_control::{PaneControl, PaneRequest};
use crate::feat::control::pane_ops;
use crate::feat::control::protocol::{
    AgentDetail, AgentKind, AnswerResult, ApiError, Direction, ErrorCode, Op, PromptResult, Reply,
    ReplyResult, WaitEnd, WaitResult,
};
use crate::feat::control::server::{
    Action, ControlDeps, StartIn, added, changing, changing_session, tiled,
};
use crate::feat::control::target::{self, Target};
use crate::feat::control::turns::{Prompted, Turns};
use crate::feat::control::validator;
use crate::feat::sessions::sessions_actor::{ApiAdd, ApiPlace};
use crate::feat::sessions::state::AgentStatus;
use crate::ids::PaneId;

/// What an agent request comes to, read from `app`. A wait is refused on a
/// settled session; a get and a reply are answered there.
pub(super) fn agent_io(
    app: &AppState,
    op: &Op,
    caller: Option<PaneId>,
) -> Result<Action, ApiError> {
    match op {
        Op::AgentStart {
            pane,
            kind,
            name,
            args,
            split,
            tab,
            force,
        } => Ok(Action::StartAgent {
            at: agent_start(app, *pane, *split, *tab, *kind, caller, *force)?,
            kind: *kind,
            line: agent::shell_line(*kind, args),
            name: name.clone(),
        }),
        Op::AgentPrompt {
            pane,
            text,
            wait,
            timeout,
        } => {
            changing(app, Some(Target::Pane(*pane)), caller).map_err(agent_pane)?;
            validator::agent_ready(app, *pane)?;
            Ok(Action::Prompt(PromptAsk {
                pane: *pane,
                text: text.clone(),
                wait: *wait,
                timeout: timeout.map(Duration::from_millis),
            }))
        }
        Op::AgentWait { pane, timeout } => {
            changing(app, Some(Target::Pane(*pane)), caller).map_err(agent_pane)?;
            Ok(Action::Wait {
                pane: *pane,
                timeout: timeout.map(Duration::from_millis),
            })
        }
        Op::AgentGet { pane } => {
            let session = target::resolve(app, Some(Target::Pane(*pane)), caller)
                .map_err(agent_pane)?
                .session;
            let agent = agent::pane_agent(app, *pane).ok_or_else(|| agent::no_agent(*pane))?;
            Ok(Action::Get {
                pane: *pane,
                detail: agent_detail(app, session, agent, *pane),
            })
        }
        Op::AgentReply { pane } => {
            target::resolve(app, Some(Target::Pane(*pane)), caller).map_err(agent_pane)?;
            let agent = agent::pane_agent(app, *pane).ok_or_else(|| agent::no_agent(*pane))?;
            if agent.transcript.is_none() {
                return Err(agent::no_reply(*pane, "no transcript yet"));
            }
            Ok(Action::Reply { pane: *pane })
        }
        _ => Err(ApiError::wrong_kind("not an agent request")),
    }
}

/// Where `agent start` runs a `kind` agent: pane `pane` (else the caller's,
/// `pane_busy` while an agent is live there; `force` types into a program
/// running there), a new pane split off the caller's toward `split`, or a
/// new tab of the caller's session with `tab`. Naming more than one place is
/// a `bad_request`, as are `force` with a new pane and splitting a float.
fn agent_start(
    app: &AppState,
    pane: Option<PaneId>,
    split: Option<Direction>,
    tab: bool,
    kind: AgentKind,
    caller: Option<PaneId>,
    force: bool,
) -> Result<StartIn, ApiError> {
    match (pane, split, tab) {
        (Some(_), Some(_), _) | (Some(_), _, true) | (None, Some(_), true) => Err(ApiError::new(
            ErrorCode::BadRequest,
            "--pane, --split and --tab exclude each other",
            format!(
                "name one: orb agent start --kind {} --split right",
                kind.program()
            ),
        )),
        (None, Some(_), false) | (None, None, true) if force => Err(ApiError::new(
            ErrorCode::BadRequest,
            "--force goes with --pane or your own pane; a new pane runs nothing yet",
            format!(
                "leave --force out: orb agent start --kind {} --split right",
                kind.program()
            ),
        )),
        (None, Some(direction), false) => {
            let (session, at) = tiled(app, None, caller, START_FLOAT)?;
            Ok(StartIn::New(ApiAdd {
                session,
                place: ApiPlace::Split {
                    at,
                    split: direction.into(),
                },
                dir: None,
            }))
        }
        (None, None, true) => Ok(StartIn::New(ApiAdd {
            session: changing_session(app, None, caller)?,
            place: ApiPlace::Tab,
            dir: None,
        })),
        (pane, None, false) => {
            let Some(pane) = changing(app, pane.map(Target::Pane), caller)?.pane else {
                return Err(ApiError::new(
                    ErrorCode::BadRequest,
                    "no pane to start in",
                    format!(
                        "name the pane: orb agent start --kind {} --pane <id>",
                        kind.program()
                    ),
                ));
            };
            validator::pane_free(app, pane, kind)?;
            Ok(StartIn::Pane {
                pane,
                force,
                own: caller == Some(pane),
            })
        }
    }
}

/// Why `agent start --split` can't run from a float.
pub(super) const START_FLOAT: &str = "a float can't be split; start the agent with --tab instead";

/// `error`, pointing a missing pane at the agent list, which shows where
/// each agent runs.
fn agent_pane(error: ApiError) -> ApiError {
    if error.code == ErrorCode::NotFound {
        error.with_hint("orb agent list shows each agent's pane")
    } else {
        error
    }
}

/// Pokes the sessions poll so it reads the panes now.
pub(super) fn poke(deps: &ControlDeps) {
    let _ = deps
        .sessions
        .tell(SessionsCommand::RefreshSessions)
        .try_send();
}

/// A prompt for a pane's agent, and whether and how long to wait on it.
#[derive(Debug)]
pub(super) struct PromptAsk {
    pub pane: PaneId,
    pub text: String,
    pub wait: bool,
    pub timeout: Option<Duration>,
}

/// Has the screen loop paste the prompt into its pane's agent and press
/// Enter, remembering when and how long the agent's transcript was then;
/// with `wait`, waits for the turn to end, with the timeout over the whole
/// call, until `gone` says the caller hung up, and replies with everything
/// the agent wrote since the paste, reading the transcript again for up to
/// `timing.reply_grace` when the turn ended with nothing written yet. An agent that stops to
/// ask the user is `agent_blocked`; a turn that ends in a failed model call
/// is `turn_failed`, with the transcript's error text when it holds one.
pub(super) fn prompt_agent(
    panes: &PaneControl,
    turns: Turns<'_>,
    ask: PromptAsk,
    poke: &dyn Fn(),
    gone: &dyn Fn() -> bool,
    timing: Timing,
) -> Reply {
    let deadline = ask.timeout.map(|limit| Instant::now() + limit);
    let agent = turns.agent(ask.pane);
    let pasted = SystemTime::now();
    let transcript = agent.as_ref().and_then(|view| view.transcript.clone());
    let offset = transcript
        .as_deref()
        .and_then(|path| fs::metadata(path).ok())
        .map_or(0, |meta| meta.len());
    if let Err(error) = panes.ask(PaneRequest::Prompt {
        pane: ask.pane,
        text: ask.text,
    }) {
        return Reply::err(error);
    }
    if let Some(view) = agent {
        turns.prompts.remember(
            ask.pane,
            Prompted {
                agent: view.id,
                at: pasted,
                transcript,
                offset,
            },
        );
    }
    if !ask.wait {
        return Reply::ok(&PromptResult { submitted: true });
    }
    let sent = Sent {
        at: pasted,
        gate: timing.stall_gate,
    };
    match agent::wait_prompted(turns.state, ask.pane, poke, gone, timing, deadline, sent) {
        Ok(WaitResult {
            status: WaitEnd::Idle,
        }) => match answer_after_idle(turns, ask.pane, gone, timing, deadline) {
            Ok(reply) => Reply::ok(&AnswerResult {
                status: WaitEnd::Idle,
                text: reply.text,
                at: reply.at,
            }),
            Err(error) => Reply::err(error),
        },
        Ok(WaitResult {
            status: WaitEnd::Blocked,
        }) => Reply::err(agent::asked_user(ask.pane)),
        Err(error) => Reply::err(turns.failure(ask.pane, error)),
    }
}

/// What `pane`'s agent wrote since orb's prompt, once its turn ended.
/// Claude can write its last text a moment after its status reads idle, so
/// a `no_reply` is read again every tick for up to `timing.reply_grace`,
/// stopping early at `deadline` or once `gone` says the caller hung up.
fn answer_after_idle(
    turns: Turns<'_>,
    pane: PaneId,
    gone: &dyn Fn() -> bool,
    timing: Timing,
    deadline: Option<Instant>,
) -> Result<ReplyResult, ApiError> {
    let grace = Instant::now() + timing.reply_grace;
    let until = deadline.map_or(grace, |deadline| deadline.min(grace));
    loop {
        match turns.reply(pane) {
            Err(error) if error.code == ErrorCode::NoReply && Instant::now() < until && !gone() => {
                thread::sleep(timing.tick);
            }
            read => return read,
        }
    }
}

/// `agent wait` on `pane`: waits for the turn to end, with the timeout over
/// the whole call, until `gone` says the caller hung up. Inside the stall
/// gate of orb's prompt, while the agent still reads idle, the prompt counts
/// as work: the wait sees the turn start first. A failed agent is
/// `turn_failed`, with the transcript's error text when it holds one.
pub(super) fn wait(
    turns: Turns<'_>,
    pane: PaneId,
    timeout: Option<Duration>,
    poke: &dyn Fn(),
    gone: &dyn Fn() -> bool,
    timing: Timing,
) -> Reply {
    let deadline = timeout.map(|limit| Instant::now() + limit);
    let waited = match turns.just_sent(pane, timing.stall_gate, SystemTime::now()) {
        Some(sent) => agent::wait_prompted(turns.state, pane, poke, gone, timing, deadline, sent),
        None => agent::wait_done(turns.state, pane, poke, gone, timing, deadline),
    };
    match waited {
        Ok(done) => Reply::ok(&done),
        Err(error) => Reply::err(turns.failure(pane, error)),
    }
}

/// `agent get` on `pane`: `detail`, reading `working` while orb's prompt
/// still counts as work (inside the stall gate, the agent not yet seen
/// working).
pub(super) fn get(turns: Turns<'_>, pane: PaneId, mut detail: AgentDetail) -> Reply {
    if turns
        .just_sent(pane, Timing::REAL.stall_gate, SystemTime::now())
        .is_some()
    {
        detail.status = AgentStatus::Working;
    }
    Reply::ok(&detail)
}

/// Has the screen loop type `line` into `pane`, names the pane `name` if
/// given (a failed rename is logged, not replied), then waits for the agent
/// to start.
fn start_agent(deps: &ControlDeps, pane: PaneId, line: String, name: Option<String>) -> Reply {
    if let Err(error) = deps.panes.ask(PaneRequest::Launch { pane, line }) {
        return Reply::err(error);
    }
    if let Some(name) = name
        && let Err(error) = deps
            .panes
            .ask(PaneRequest::Change(Change::RenamePane { pane, name }))
    {
        deps.log.line(
            "control",
            &format!("couldn't name pane {}: {}", pane.0, error.message),
        );
    }
    Reply::from_result(agent::wait_started(
        &deps.state,
        pane,
        &|| poke(deps),
        Timing::REAL,
    ))
}

/// Readies `pane`'s shell for a `kind` agent's line, as `pane run` does:
/// `busy` while a program runs there, unless `force`, with a hint that
/// also starts the agent in a new pane; text left on the command line is
/// cleared. In the caller's `own` pane the program is the caller itself, so
/// the hint offers only a new pane.
fn shell_free(
    panes: &PaneControl,
    pane: PaneId,
    kind: AgentKind,
    force: bool,
    own: bool,
) -> Result<(), ApiError> {
    let program = kind.program();
    let elsewhere = format!("orb agent start --kind {program} --split right");
    match pane_ops::ready_shell(panes, pane, force, Some(&elsewhere)) {
        Ok(_) => Ok(()),
        Err(error) if own && error.code == ErrorCode::Busy => Err(ApiError::new(
            ErrorCode::Busy,
            format!("{}, your own pane", error.message),
            format!(
                "the agent can't start there while this call runs; start it in a new pane: \
                 {elsewhere}, or orb agent start --kind {program} --tab"
            ),
        )),
        Err(error) => Err(error),
    }
}

/// Adds the pane `at` asks for, if any, then starts the `kind` agent there
/// (see [`start_agent`]), an existing pane's shell readied first (see
/// [`shell_free`]).
pub(super) fn start_in(
    deps: &ControlDeps,
    at: StartIn,
    kind: AgentKind,
    line: String,
    name: Option<String>,
) -> Reply {
    let pane = match at {
        StartIn::Pane { pane, force, own } => {
            match shell_free(&deps.panes, pane, kind, force, own) {
                Ok(()) => pane,
                Err(error) => return Reply::err(error),
            }
        }
        StartIn::New(add) => match added(deps, add) {
            Ok(pane) => pane,
            Err(error) => return Reply::err(error),
        },
    };
    start_agent(deps, pane, line, name)
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate setup failures with `?` and assert on the outcome"
)]
mod tests {
    use std::fs;
    use std::io;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use tempfile::tempdir;

    use super::{PromptAsk, agent_io, get, prompt_agent, shell_free, wait};
    use crate::State;
    use crate::feat::control::agent::Timing;
    use crate::feat::control::pane_control::{
        PaneAnswer, PaneControl, PaneRequest, ShellPhase, ShellView,
    };
    use crate::feat::control::protocol::{AgentKind, ErrorCode, Op, Reply};
    use crate::feat::control::server::Action;
    use crate::feat::control::turns::{Prompts, Turns};
    use crate::feat::harness::Harnesses;
    use crate::feat::harness::fake::FakeHarness;
    use crate::feat::sessions::state::{Agent, AgentStatus};
    use crate::feat::sessions::transcript::{Exchange, Message, Role};
    use crate::ids::PaneId;
    use crate::intent_handler::test_support::{agent, state_with};

    /// Waits paced for tests.
    const FAST: Timing = Timing {
        tick: Duration::from_millis(1),
        poke_every: Duration::from_millis(1),
        start_limit: Duration::from_millis(50),
        stall_gate: Duration::from_millis(50),
        reply_grace: Duration::from_millis(50),
    };

    /// Waits paced for tests, with a stall gate long enough that a wait
    /// started right after a prompt still falls inside it.
    const GATED: Timing = Timing {
        stall_gate: Duration::from_millis(300),
        ..FAST
    };

    /// `agent prompt 1 "go on"` through `turns`, not waiting.
    fn prompt(turns: Turns<'_>) -> Reply {
        let ask = PromptAsk {
            pane: PaneId(1),
            text: "go on".to_owned(),
            wait: false,
            timeout: None,
        };
        prompt_agent(&obliging(), turns, ask, &|| {}, &|| false, FAST)
    }

    /// Pane 1 holding idle agent 1, writing `transcript`.
    fn idle_pane(transcript: PathBuf) -> State {
        State::new(state_with(
            vec![Agent {
                transcript: Some(transcript),
                ..agent(1, AgentStatus::Idle)
            }],
            1,
        ))
    }

    /// A claude harness whose transcripts hold `messages`.
    fn claude(messages: Vec<Message>) -> Harnesses {
        Harnesses::new(vec![Arc::new(
            FakeHarness::new("claude", Vec::new()).with_messages(messages),
        )])
    }

    /// Agent 1 in `state` now reads `status`.
    fn set_status(state: &State, status: AgentStatus) {
        for project in &mut state.write().sessions.projects {
            for found in project.agents.iter_mut().filter(|found| found.id.0 == 1) {
                found.status = status;
            }
        }
    }

    /// `agent prompt 1 "go on" --wait` through `turns`.
    fn prompt_wait(turns: Turns<'_>, poke: &dyn Fn()) -> Reply {
        let ask = PromptAsk {
            pane: PaneId(1),
            text: "go on".to_owned(),
            wait: true,
            timeout: None,
        };
        prompt_agent(&obliging(), turns, ask, poke, &|| false, FAST)
    }

    /// A screen loop that carries out every request.
    fn obliging() -> PaneControl {
        PaneControl::new(|_, reply| {
            let _ = reply.send(Ok(PaneAnswer::Done));
        })
    }

    /// A screen loop whose pane's shell runs `program`, counting the
    /// requests that would type into the pane.
    fn running(program: &'static str, typed: Arc<AtomicUsize>) -> PaneControl {
        PaneControl::new(move |request, reply| {
            let answer = match request {
                PaneRequest::Shell { .. } => PaneAnswer::Shell(ShellView {
                    marks_seen: true,
                    phase: ShellPhase::Running {
                        program: Some(program.to_owned()),
                        started: 3,
                    },
                    last: None,
                    seq: 3,
                    held: false,
                }),
                _ => {
                    typed.fetch_add(1, Ordering::SeqCst);
                    PaneAnswer::Done
                }
            };
            let _ = reply.send(Ok(answer));
        })
    }

    #[rstest::rstest]
    fn start_in_a_pane_running_a_program_is_refused_as_busy() {
        // Given pane 1's shell running vim.
        let typed = Arc::new(AtomicUsize::new(0));
        let panes = running("vim", Arc::clone(&typed));

        // When readying pane 1 for a claude start.
        let checked = shell_free(&panes, PaneId(1), AgentKind::Claude, false, false);

        // Then it is refused as busy, naming vim and a new pane, and
        // nothing is typed.
        let error = checked.err();
        assert_eq!(
            (
                error
                    .as_ref()
                    .map(|error| (error.code, error.message.as_str())),
                error
                    .as_ref()
                    .and_then(|error| error.hint.as_deref())
                    .is_some_and(
                        |hint| hint.contains("orb agent start --kind claude --split right")
                    ),
                typed.load(Ordering::SeqCst),
            ),
            (Some((ErrorCode::Busy, "vim is running in pane 1")), true, 0),
            "a running program refuses the agent's line, got {error:?}"
        );
    }

    #[rstest::rstest]
    fn start_in_your_own_busy_pane_points_only_at_a_new_pane() {
        // Given pane 1, the caller's own, where fish runs orb itself.
        let typed = Arc::new(AtomicUsize::new(0));
        let panes = running("orb", Arc::clone(&typed));

        // When readying pane 1 for a claude start from pane 1.
        let checked = shell_free(&panes, PaneId(1), AgentKind::Claude, false, true);

        // Then it is refused as busy, the hint naming a new pane or tab and
        // neither stopping the program nor forcing.
        let error = checked.err();
        let hint = error
            .as_ref()
            .and_then(|error| error.hint.clone())
            .unwrap_or_default();
        assert_eq!(
            (
                error.as_ref().map(|error| error.code),
                hint.contains("orb agent start --kind claude --split right")
                    && hint.contains("--tab"),
                hint.contains("--force") || hint.contains("ctrl+c"),
                typed.load(Ordering::SeqCst),
            ),
            (Some(ErrorCode::Busy), true, false, 0),
            "the caller can't quit itself, got {error:?}"
        );
    }

    #[rstest::rstest]
    fn forced_start_in_a_pane_running_a_program_goes_ahead() {
        // Given pane 1's shell running a nested shell.
        let typed = Arc::new(AtomicUsize::new(0));
        let panes = running("bash", typed);

        // When readying pane 1 for a claude start with --force.
        let checked = shell_free(&panes, PaneId(1), AgentKind::Claude, true, false);

        // Then the start may go ahead.
        assert_eq!(
            checked.map_err(|error| error.code),
            Ok(()),
            "--force types into the running program"
        );
    }

    /// An assistant message saying `text` at byte `offset`.
    fn answer(text: &str, offset: u64) -> Message {
        Message {
            role: Role::Assistant,
            text: text.to_owned(),
            offset,
            prompt_offset: 0,
            at: 1_000,
        }
    }

    #[rstest::rstest]
    fn reply_after_a_prompt_leaves_out_earlier_answers() -> io::Result<()> {
        // Given idle agent 1 in pane 1 whose 100-byte transcript already holds
        // an answer, and which answers again past those bytes.
        let dir = tempdir()?;
        let transcript = dir.path().join("t.jsonl");
        fs::write(&transcript, "x".repeat(100))?;
        let state = idle_pane(transcript);
        let harnesses = claude(vec![
            answer("the earlier answer", 40),
            answer("the new answer", 150),
        ]);
        let prompts = Prompts::default();
        let turns = Turns {
            state: &state,
            harnesses: &harnesses,
            prompts: &prompts,
        };
        let ask = PromptAsk {
            pane: PaneId(1),
            text: "go on".to_owned(),
            wait: false,
            timeout: None,
        };
        prompt_agent(&obliging(), turns, ask, &|| {}, &|| false, Timing::REAL);

        // When reading pane 1's reply.
        let replied = turns.reply(PaneId(1));

        // Then only the answer written past the prompt comes back.
        assert_eq!(
            replied.map(|reply| reply.text).map_err(|error| error.code),
            Ok("the new answer".to_owned()),
            "the answer from before orb's prompt should be left out"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn prompt_wait_replies_with_the_answer_since_the_prompt() -> io::Result<()> {
        // Given idle agent 1 in pane 1 whose 100-byte transcript already holds
        // an answer, which works on the prompt, then goes idle having
        // answered past those bytes.
        let dir = tempdir()?;
        let transcript = dir.path().join("t.jsonl");
        fs::write(&transcript, "x".repeat(100))?;
        let state = idle_pane(transcript);
        let harnesses = claude(vec![
            answer("the earlier answer", 40),
            answer("the new answer", 150),
        ]);
        let prompts = Prompts::default();
        let polls = AtomicUsize::new(0);
        let poke = || match polls.fetch_add(1, Ordering::SeqCst) + 1 {
            1 => set_status(&state, AgentStatus::Working),
            3 => set_status(&state, AgentStatus::Idle),
            _ => {}
        };

        // When prompting it and waiting.
        let replied = prompt_wait(
            Turns {
                state: &state,
                harnesses: &harnesses,
                prompts: &prompts,
            },
            &poke,
        );

        // Then the reply is the turn's end and the answer since the prompt.
        assert_eq!(
            replied.result,
            Some(serde_json::json!({
                "status": "idle",
                "text": "the new answer",
                "at": 1_000,
            })),
            "prompt --wait should reply with the answer, got {replied:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn prompt_wait_reads_an_answer_written_just_after_idle() -> io::Result<()> {
        // Given idle agent 1 in pane 1 over a 100-byte transcript, which works
        // on the prompt and goes idle before its answer reaches the
        // transcript.
        let dir = tempdir()?;
        let transcript = dir.path().join("t.jsonl");
        fs::write(&transcript, "x".repeat(100))?;
        let state = idle_pane(transcript);
        let harnesses = Harnesses::new(vec![Arc::new(
            FakeHarness::new("claude", Vec::new())
                .with_messages(vec![answer("the late answer", 150)])
                .answering_after(2),
        )]);
        let prompts = Prompts::default();
        let polls = AtomicUsize::new(0);
        let poke = || match polls.fetch_add(1, Ordering::SeqCst) + 1 {
            1 => set_status(&state, AgentStatus::Working),
            3 => set_status(&state, AgentStatus::Idle),
            _ => {}
        };

        // When prompting it and waiting.
        let replied = prompt_wait(
            Turns {
                state: &state,
                harnesses: &harnesses,
                prompts: &prompts,
            },
            &poke,
        );

        // Then the reply is the answer written just after the turn ended.
        assert_eq!(
            replied.result,
            Some(serde_json::json!({
                "status": "idle",
                "text": "the late answer",
                "at": 1_000,
            })),
            "prompt --wait should read the answer written after idle, got {replied:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn prompt_wait_with_nothing_written_is_no_reply() -> io::Result<()> {
        // Given idle agent 1 in pane 1 over a 100-byte transcript, which works
        // on the prompt and goes idle without writing an answer.
        let dir = tempdir()?;
        let transcript = dir.path().join("t.jsonl");
        fs::write(&transcript, "x".repeat(100))?;
        let state = idle_pane(transcript);
        let harnesses = claude(Vec::new());
        let prompts = Prompts::default();
        let polls = AtomicUsize::new(0);
        let poke = || match polls.fetch_add(1, Ordering::SeqCst) + 1 {
            1 => set_status(&state, AgentStatus::Working),
            3 => set_status(&state, AgentStatus::Idle),
            _ => {}
        };

        // When prompting it and waiting.
        let replied = prompt_wait(
            Turns {
                state: &state,
                harnesses: &harnesses,
                prompts: &prompts,
            },
            &poke,
        );

        // Then the call is refused as no_reply once the grace runs out.
        assert_eq!(
            replied.error.map(|error| error.code),
            Some(ErrorCode::NoReply),
            "prompt --wait with nothing written should be no_reply"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn prompt_wait_on_an_agent_that_asks_the_user_is_agent_blocked() -> io::Result<()> {
        // Given idle agent 1 in pane 1, which stops to ask for an approval
        // once prompted.
        let dir = tempdir()?;
        let transcript = dir.path().join("t.jsonl");
        fs::write(&transcript, "x".repeat(100))?;
        let state = idle_pane(transcript);
        let harnesses = claude(Vec::new());
        let prompts = Prompts::default();
        let poke = || set_status(&state, AgentStatus::NeedsApproval);

        // When prompting it and waiting.
        let replied = prompt_wait(
            Turns {
                state: &state,
                harnesses: &harnesses,
                prompts: &prompts,
            },
            &poke,
        );

        // Then the call is refused as blocked, pointing at the agent's screen.
        let error = replied.error;
        assert!(
            error.as_ref().is_some_and(|error| {
                error.code == ErrorCode::AgentBlocked
                    && error
                        .message
                        .contains("stopped to ask for an approval or an answer")
                    && error
                        .hint
                        .as_deref()
                        .is_some_and(|hint| hint.contains("orb pane read 1"))
            }),
            "an agent asking the user should be agent_blocked naming pane read, got {error:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn get_right_after_a_prompt_reads_working() -> io::Result<()> {
        // Given idle agent 1 in pane 1, its status not yet polled since orb's
        // prompt, nothing answered.
        let dir = tempdir()?;
        let transcript = dir.path().join("t.jsonl");
        fs::write(&transcript, "x".repeat(100))?;
        let state = idle_pane(transcript);
        let harnesses = claude(Vec::new());
        let prompts = Prompts::default();
        let turns = Turns {
            state: &state,
            harnesses: &harnesses,
            prompts: &prompts,
        };
        prompt(turns);
        let planned = agent_io(&state.read(), &Op::AgentGet { pane: PaneId(1) }, None);
        let Ok(Action::Get { pane, detail }) = planned else {
            return Err(io::Error::other(format!(
                "agent get wasn't planned: {planned:?}"
            )));
        };

        // When reading the agent.
        let got = get(turns, pane, detail);

        // Then it reads working.
        let status = got
            .result
            .as_ref()
            .and_then(|row| row.get("status").cloned());
        assert_eq!(
            status,
            Some(serde_json::json!("working")),
            "a prompt orb just sent should read as work, got {got:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn wait_right_after_a_prompt_with_no_turn_is_stalled() -> io::Result<()> {
        // Given idle agent 1 in pane 1 that orb just prompted, which never
        // starts working.
        let dir = tempdir()?;
        let transcript = dir.path().join("t.jsonl");
        fs::write(&transcript, "x".repeat(100))?;
        let state = idle_pane(transcript);
        let harnesses = claude(Vec::new());
        let prompts = Prompts::default();
        let turns = Turns {
            state: &state,
            harnesses: &harnesses,
            prompts: &prompts,
        };
        prompt(turns);

        // When waiting on pane 1.
        let waited = wait(turns, PaneId(1), None, &|| {}, &|| false, GATED);

        // Then the stale idle doesn't end the wait: the prompt stalled.
        assert_eq!(
            waited.error.map(|error| error.code),
            Some(ErrorCode::PromptStalled),
            "a wait right after a prompt should look for the turn first"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn wait_right_after_a_prompt_returns_once_the_turn_ends() -> io::Result<()> {
        // Given idle agent 1 in pane 1 that orb just prompted, which the first
        // poll sees working and the third idle again.
        let dir = tempdir()?;
        let transcript = dir.path().join("t.jsonl");
        fs::write(&transcript, "x".repeat(100))?;
        let state = idle_pane(transcript);
        let harnesses = claude(Vec::new());
        let prompts = Prompts::default();
        let turns = Turns {
            state: &state,
            harnesses: &harnesses,
            prompts: &prompts,
        };
        prompt(turns);
        let polls = AtomicUsize::new(0);
        let poke = || match polls.fetch_add(1, Ordering::SeqCst) + 1 {
            1 => set_status(&state, AgentStatus::Working),
            3 => set_status(&state, AgentStatus::Idle),
            _ => {}
        };

        // When waiting on pane 1.
        let waited = wait(turns, PaneId(1), None, &poke, &|| false, GATED);

        // Then it ends idle once the turn is over, not on the stale idle.
        assert_eq!(
            (waited.result, polls.load(Ordering::SeqCst) >= 3),
            (Some(serde_json::json!({ "status": "idle" })), true),
            "the wait should end idle only after the turn ran"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn prompt_wait_on_a_failed_turn_is_turn_failed() -> io::Result<()> {
        // Given idle agent 1 in pane 1, which works on the prompt, then goes
        // idle with its model call failed past the transcript's 100 bytes.
        let dir = tempdir()?;
        let transcript = dir.path().join("t.jsonl");
        fs::write(&transcript, "x".repeat(100))?;
        let state = idle_pane(transcript);
        let harnesses = claude(vec![Message {
            role: Role::Failure,
            ..answer("API Error: Connection lost", 150)
        }]);
        let prompts = Prompts::default();
        let polls = AtomicUsize::new(0);
        let poke = || match polls.fetch_add(1, Ordering::SeqCst) + 1 {
            1 => set_status(&state, AgentStatus::Working),
            3 => set_status(&state, AgentStatus::Idle),
            _ => {}
        };

        // When prompting it and waiting.
        let replied = prompt_wait(
            Turns {
                state: &state,
                harnesses: &harnesses,
                prompts: &prompts,
            },
            &poke,
        );

        // Then the call fails as turn_failed with the error's text.
        let error = replied.error.map(|error| serde_json::to_value(&error));
        assert_eq!(
            error
                .as_ref()
                .and_then(|error| error.as_ref().ok())
                .map(|error| (error.get("code").cloned(), error.get("message").cloned())),
            Some((
                Some(serde_json::json!("turn_failed")),
                Some(serde_json::json!("API Error: Connection lost"))
            )),
            "a failed turn should be turn_failed with its text, got {error:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn agent_wait_on_a_failed_agent_carries_the_transcripts_failure() {
        // Given failed agent 1 in pane 1, whose last exchange ended in an
        // API error.
        let state = State::new(state_with(
            vec![Agent {
                transcript: Some(PathBuf::from("/t/1.jsonl")),
                ..agent(1, AgentStatus::Failed)
            }],
            1,
        ));
        let harnesses = Harnesses::new(vec![Arc::new(
            FakeHarness::new("claude", Vec::new()).with_exchanges(vec![Exchange {
                failure: Some("API Error: 529 Overloaded".to_owned()),
                ..Exchange::default()
            }]),
        )]);
        let prompts = Prompts::default();

        // When waiting on pane 1.
        let waited = wait(
            Turns {
                state: &state,
                harnesses: &harnesses,
                prompts: &prompts,
            },
            PaneId(1),
            None,
            &|| {},
            &|| false,
            FAST,
        );

        // Then the wait fails as turn_failed with the transcript's error.
        let error = waited.error;
        assert_eq!(
            error
                .as_ref()
                .map(|error| (error.code, error.message.as_str())),
            Some((ErrorCode::TurnFailed, "API Error: 529 Overloaded")),
            "a failed agent's wait should carry its error text, got {error:?}"
        );
    }
}
