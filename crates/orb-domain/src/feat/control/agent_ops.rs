//! Agent requests: where `agent start` runs its agent, what a prompt, wait,
//! get or reply comes to, and carrying out a start or a prompt on the screen
//! loop, waiting on the agent after.

use std::time::{Duration, Instant, SystemTime};

use crate::AppState;
use crate::command::SessionsCommand;
use crate::feat::control::agent::{self, Timing};
use crate::feat::control::change::Change;
use crate::feat::control::lists::agent_detail;
use crate::feat::control::pane_control::PaneRequest;
use crate::feat::control::protocol::{
    AgentKind, ApiError, Direction, ErrorCode, Op, PromptResult, Reply,
};
use crate::feat::control::server::{
    Action, ControlDeps, StartIn, added, changing, changing_session, tiled,
};
use crate::feat::control::target::{self, Target};
use crate::feat::control::validator;
use crate::feat::sessions::sessions_actor::{ApiAdd, ApiPlace};
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
        } => Ok(Action::StartAgent {
            at: agent_start(app, *pane, *split, *tab, *kind, caller)?,
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
            Ok(Action::Prompt {
                pane: *pane,
                text: text.clone(),
                wait: *wait,
                timeout: timeout.map(Duration::from_millis),
            })
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
            Ok(Action::Answered(Reply::ok(&agent_detail(
                app, session, agent, *pane,
            ))))
        }
        Op::AgentReply { pane } => {
            target::resolve(app, Some(Target::Pane(*pane)), caller).map_err(agent_pane)?;
            let agent = agent::pane_agent(app, *pane).ok_or_else(|| agent::no_agent(*pane))?;
            let path = agent.transcript.clone().ok_or_else(|| {
                ApiError::new(
                    ErrorCode::NoReply,
                    "no transcript yet",
                    format!("orb agent wait {0}, then orb agent reply {0}", pane.0),
                )
            })?;
            Ok(Action::Transcript {
                harness: agent.harness.clone(),
                path,
                pane: *pane,
            })
        }
        _ => Err(ApiError::new(
            ErrorCode::BadRequest,
            "not an agent request",
            "orb help lists the commands",
        )),
    }
}

/// Where `agent start` runs a `kind` agent: pane `pane` (else the caller's,
/// `pane_busy` while an agent is live there), a new pane split off the
/// caller's toward `split`, or a new tab of the caller's session with `tab`.
/// Naming more than one place is a `bad_request`, as is splitting a float.
fn agent_start(
    app: &AppState,
    pane: Option<PaneId>,
    split: Option<Direction>,
    tab: bool,
    kind: AgentKind,
    caller: Option<PaneId>,
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
            Ok(StartIn::Pane(pane))
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

/// Has the screen loop paste `text` into `pane`'s agent and press Enter;
/// with `wait`, waits for the turn to end or the agent to be blocked, with
/// `timeout` over the whole call, until `gone` says the caller hung up.
pub(super) fn prompt_agent(
    deps: &ControlDeps,
    pane: PaneId,
    text: String,
    wait: bool,
    timeout: Option<Duration>,
    gone: &dyn Fn() -> bool,
) -> Reply {
    let deadline = timeout.map(|limit| Instant::now() + limit);
    let pasted = SystemTime::now();
    if let Err(error) = deps.panes.ask(PaneRequest::Prompt { pane, text }) {
        return Reply::err(error);
    }
    if !wait {
        return Reply::ok(&PromptResult { submitted: true });
    }
    let poke = || poke(deps);
    match agent::wait_prompted(
        &deps.state,
        pane,
        &poke,
        gone,
        Timing::REAL,
        deadline,
        pasted,
    ) {
        Ok(done) => Reply::ok(&done),
        Err(error) => Reply::err(error),
    }
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
    match agent::wait_started(&deps.state, pane, &|| poke(deps), Timing::REAL) {
        Ok(started) => Reply::ok(&started),
        Err(error) => Reply::err(error),
    }
}

/// Adds the pane `at` asks for, if any, then starts the agent there (see
/// [`start_agent`]).
pub(super) fn start_in(
    deps: &ControlDeps,
    at: StartIn,
    line: String,
    name: Option<String>,
) -> Reply {
    let pane = match at {
        StartIn::Pane(pane) => pane,
        StartIn::New(add) => match added(deps, add) {
            Ok(pane) => pane,
            Err(error) => return Reply::err(error),
        },
    };
    start_agent(deps, pane, line, name)
}
