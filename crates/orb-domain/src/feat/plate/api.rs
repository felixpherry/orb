//! The plate's side of the control API: what each `orb plate` request comes
//! to, the sessions actor's answer to an add, an update or a close, and
//! `plate list`'s reply.

use std::time::SystemTime;

use error_stack::Report;
use kameo::error::SendError;

use crate::AppState;
use crate::feat::control::agent;
use crate::feat::control::protocol::{
    ApiError, Done, ErrorCode, ItemResult, Op, PlateItemRow, PlateResult, Reply, SEE_LOG,
};
use crate::feat::control::server::ControlDeps;
use crate::feat::control::target::{self, Target};
use crate::feat::plate::validator::{self, DONE_ITEM_HINT, UNKNOWN_ITEM_HINT};
use crate::feat::plate::{ItemState, NewPlateItem, PlateChange, PlateItem, Verb};
use crate::feat::sessions::sessions_actor::{
    ApiPlateAdd, ApiPlateDone, ApiPlateUpdate, PlateApiError,
};
use crate::ids::PaneId;

/// What a plate request comes to, read from the state.
#[derive(Debug)]
pub(crate) enum PlateAsk {
    Add(ApiPlateAdd),
    Update(ApiPlateUpdate),
    Done(ApiPlateDone),
}

/// What plate `op` from `caller` comes to, or why it's refused.
pub(crate) fn plan(app: &AppState, op: &Op, caller: Option<PaneId>) -> Result<PlateAsk, ApiError> {
    match op {
        Op::PlateAdd { .. } => add(app, op, caller),
        Op::PlateUpdate {
            item,
            title,
            verb,
            what,
            links,
            urgent,
            waiting,
        } => {
            validator::open_item(app, *item)?;
            if let Some(links) = links {
                validator::links(links)?;
            }
            let verb = verb.as_deref().map(validator::verb).transpose()?;
            Ok(PlateAsk::Update(ApiPlateUpdate {
                item: *item,
                change: PlateChange {
                    title: title.clone(),
                    verb,
                    what: what.clone(),
                    links: links.clone(),
                    urgent: *urgent,
                    waiting: *waiting,
                },
            }))
        }
        Op::PlateDone { item } => {
            validator::known_item(app, *item)?;
            Ok(PlateAsk::Done(ApiPlateDone { item: *item }))
        }
        _ => Err(ApiError::wrong_kind("not a plate request")),
    }
}

/// The item `plate add` asks for. It comes from the caller's agent, and ⏎
/// goes to `--pane` when given, else to the caller's pane; the item belongs
/// to that pane's session. An idea lands parked as the agent's offer, so it
/// can't be waiting.
fn add(app: &AppState, op: &Op, caller: Option<PaneId>) -> Result<PlateAsk, ApiError> {
    let Op::PlateAdd {
        title,
        verb,
        what,
        links,
        urgent,
        waiting,
        pane,
    } = op
    else {
        return Err(ApiError::wrong_kind("not a plate add"));
    };
    let from = validator::adding_session(app, caller)?;
    let verb = validator::verb(verb)?;
    validator::links(links)?;
    if verb == Verb::Idea && *waiting {
        return Err(ApiError::new(
            ErrorCode::BadRequest,
            "an idea can't be waiting",
            "drop --waiting, or pick another --verb",
        ));
    }
    let caller_pane = from.pane.ok_or_else(|| {
        ApiError::new(
            ErrorCode::NoTarget,
            "not inside an orb pane",
            "run orb plate add from an orb pane",
        )
    })?;
    let (session, pane) = match pane {
        Some(pane) => {
            let target = target::resolve(app, Some(Target::Pane(*pane)), caller)?;
            (target.session, *pane)
        }
        None => (from.session, caller_pane),
    };
    let state = match (verb, waiting) {
        (Verb::Idea, _) => ItemState::Parked { by_user: false },
        (_, true) => ItemState::Waiting,
        (_, false) => ItemState::YourMove,
    };
    Ok(PlateAsk::Add(ApiPlateAdd {
        item: NewPlateItem {
            verb,
            state,
            title: title.clone(),
            what: what.clone(),
            links: links.clone(),
            urgent: *urgent,
            session,
            session_title: app
                .sessions
                .session(session)
                .map(|session| app.sessions.title(session))
                .unwrap_or_default(),
            pane,
            harness: agent::pane_agent(app, caller_pane)
                .filter(|found| agent::live(found.status))
                .map(|found| found.harness.clone()),
        },
    }))
}

/// Asks the sessions actor for `ask` and replies with its answer: the new
/// item for an add, `{}` for an update or a close.
pub(crate) fn ask(deps: &ControlDeps, ask: PlateAsk) -> Reply {
    match ask {
        PlateAsk::Add(add) => match deps
            .runtime
            .block_on(async { deps.sessions.ask(add).await })
        {
            Ok(item) => Reply::ok(&ItemResult { item }),
            Err(error) => Reply::err(refused(error)),
        },
        PlateAsk::Update(update) => changed(
            deps.runtime
                .block_on(async { deps.sessions.ask(update).await }),
        ),
        PlateAsk::Done(done) => changed(
            deps.runtime
                .block_on(async { deps.sessions.ask(done).await }),
        ),
    }
}

/// The reply to the actor's `answer` to an update or a close.
fn changed<M>(answer: Result<(), SendError<M, Report<PlateApiError>>>) -> Reply {
    match answer {
        Ok(()) => Reply::ok(&Done {}),
        Err(error) => Reply::err(refused(error)),
    }
}

/// The API error for the actor's failed answer `error`: its code from why
/// it failed, and the report's latest reason as the message.
fn refused<M>(error: SendError<M, Report<PlateApiError>>) -> ApiError {
    let SendError::HandlerError(report) = error else {
        return ApiError::failed(error.to_string());
    };
    let (code, hint) = match report.current_context() {
        PlateApiError::NotFound => (ErrorCode::NotFound, UNKNOWN_ITEM_HINT),
        PlateApiError::Done => (ErrorCode::BadRequest, DONE_ITEM_HINT),
        PlateApiError::Unsaved => (ErrorCode::Failed, SEE_LOG),
    };
    let reason = report
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| format!("{:?}", report.current_context()));
    ApiError::new(code, reason, hint)
}

/// `plate list`'s reply: the items that aren't done, or every item with
/// `all`, with each one's age at `now`. `hidden` counts the done items the
/// default left out, and the hint says how to see them.
pub(crate) fn list(app: &AppState, now: SystemTime, all: bool) -> Reply {
    let every = app.plate.items();
    let items: Vec<PlateItemRow> = every
        .iter()
        .filter(|item| all || item.state.is_open())
        .map(|item| row(item, now))
        .collect();
    let hidden = every.len() - items.len();
    let plural = if hidden == 1 { "" } else { "s" };
    Reply::ok(&PlateResult {
        items,
        hidden,
        hint: (hidden > 0)
            .then(|| format!("orb plate list --all shows {hidden} done item{plural}")),
    })
}

/// `item`'s row in `plate list`, aged at `now`.
fn row(item: &PlateItem, now: SystemTime) -> PlateItemRow {
    PlateItemRow {
        id: item.id,
        verb: item.verb,
        state: item.state.name(),
        title: item.title.clone(),
        what: item.what.clone(),
        links: item.links.clone(),
        urgent: item.urgent,
        session: item.session,
        pane: item.pane,
        age_s: now
            .duration_since(item.changed_at)
            .map_or(0, |age| age.as_secs()),
    }
}

#[cfg(test)]
mod tests;
