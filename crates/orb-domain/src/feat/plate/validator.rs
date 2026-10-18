//! Checks a plate request must pass before orb asks for it.

use crate::AppState;
use crate::feat::control::protocol::{ApiError, ErrorCode};
use crate::feat::control::target::{self, Resolved};
use crate::feat::control::validator::not_settled;
use crate::feat::plate::state::{ITEM_DONE, no_item};
use crate::feat::plate::{PlateItem, RowKey, Verb};
use crate::ids::{PaneId, PlateItemId, SessionId};

/// The hint for an item ID orb doesn't know: done items are left out of a
/// plain list.
pub(crate) const UNKNOWN_ITEM_HINT: &str =
    "orb plate list shows the open items; orb plate list --all shows the done ones too";

/// The hint for a change to an item that is done.
pub(crate) const DONE_ITEM_HINT: &str =
    "the item is done; orb plate list --all shows it, orb plate list the open items";

/// What a colonist is told when it tries to add a plate item.
pub(crate) const COLONIST_REFUSED: &str = "colonists report to their chief";

/// The caller's session and pane, which an added item comes from:
/// `no_target` outside orb, `not_found` when the pane is gone, `colonist`
/// in a colonist, `settled` in a settled session.
pub(crate) fn adding_session(app: &AppState, caller: Option<PaneId>) -> Result<Resolved, ApiError> {
    let resolved = target::resolve(app, None, caller)?;
    if app
        .sessions
        .session(resolved.session)
        .is_some_and(|session| session.parent.is_some())
    {
        return Err(ApiError::new(
            ErrorCode::Colonist,
            COLONIST_REFUSED,
            "put it in your report to your chief",
        ));
    }
    not_settled(app, resolved.session)?;
    Ok(resolved)
}

/// The verb `text` names; `bad_request` naming every verb when it names
/// none.
pub(crate) fn verb(text: &str) -> Result<Verb, ApiError> {
    Verb::parse(text).ok_or_else(|| {
        let names: Vec<&str> = Verb::ALL.iter().map(|verb| verb.name()).collect();
        ApiError::new(
            ErrorCode::BadRequest,
            format!("unknown verb {text}; use one of {}", names.join(", ")),
            "orb plate add --help lists the verbs",
        )
    })
}

/// Item `id`, still open: `not_found` when there's none, `bad_request` when
/// it's done.
pub(crate) fn open_item(app: &AppState, id: PlateItemId) -> Result<&PlateItem, ApiError> {
    let item = known_item(app, id)?;
    if item.state.is_open() {
        Ok(item)
    } else {
        Err(ApiError::new(
            ErrorCode::BadRequest,
            ITEM_DONE,
            DONE_ITEM_HINT,
        ))
    }
}

/// Item `id`, open or done; `not_found` when there's none.
pub(crate) fn known_item(app: &AppState, id: PlateItemId) -> Result<&PlateItem, ApiError> {
    app.plate
        .item(id)
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, no_item(id), UNKNOWN_ITEM_HINT))
}

/// `links`, each free of control characters (C0, DEL and C1);
/// `bad_request` naming the first that holds one. A link can be typed into
/// a shell, where a control character such as Ctrl-C would act on the
/// prompt instead of staying in the link.
pub(crate) fn links(links: &[String]) -> Result<(), ApiError> {
    match links
        .iter()
        .position(|link| link.chars().any(char::is_control))
    {
        Some(at) => Err(ApiError::new(
            ErrorCode::BadRequest,
            format!("link {} holds a control character", at + 1),
            "take the control characters out of the link",
        )),
        None => Ok(()),
    }
}

/// Where ⏎ on a plate row takes the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GoTarget {
    /// An agent's pane of a live session.
    Agent { session: SessionId, pane: PaneId },
    /// A shell pane of a live session.
    Shell { session: SessionId, pane: PaneId },
    /// The session itself: its pane is gone, or it's settled.
    Session(SessionId),
}

/// Why ⏎ can't go anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GoError {
    /// The row's session was deleted or is being deleted.
    SessionGone,
}

/// The message box's words for [`GoError::SessionGone`].
pub(crate) const SESSION_GONE: &str = "the session is gone";

/// Where ⏎ on the row with `key`, from `session`, goes. orb's row goes to
/// its agent's pane. An item goes to its pane while that pane is still in
/// its live session, and to the session itself once the pane is gone or the
/// session is settled, so entering it resumes it.
pub(crate) fn go_target(
    app: &AppState,
    key: RowKey,
    session: SessionId,
) -> Result<GoTarget, GoError> {
    let found = app.sessions.live(session).ok_or(GoError::SessionGone)?;
    let item = match key {
        RowKey::Agent(pane) => return Ok(GoTarget::Agent { session, pane }),
        RowKey::Item(id) => app.plate.item(id),
    };
    let pane = item
        .map(|item| item.pane)
        .filter(|&pane| found.settled_at.is_none() && app.layouts.owner_of(pane) == Some(session));
    Ok(match pane {
        Some(pane) if app.sessions.runs_agent(session, pane, &app.layouts) => {
            GoTarget::Agent { session, pane }
        }
        Some(pane) => GoTarget::Shell { session, pane },
        None => GoTarget::Session(session),
    })
}

#[cfg(test)]
mod tests {
    use super::{GoError, GoTarget, adding_session, go_target, known_item, verb};
    use crate::AppState;
    use crate::feat::control::protocol::ErrorCode;
    use crate::feat::layout::state::test_entry;
    use crate::feat::layout::tree::Split;
    use crate::feat::plate::state::test_item;
    use crate::feat::plate::{ItemState, Plate, PlateItem, RowKey};
    use crate::feat::sessions::state::AgentStatus;
    use crate::ids::{PaneId, PlateItemId, SessionId};
    use crate::intent_handler::test_support::{agent, colony_with_colonists, settle, state_with};

    /// Session 1 running agent 1 in pane 1, with item 1 from session
    /// `session`'s pane `pane`.
    fn with_item(session: i64, pane: i64) -> AppState {
        let mut app = state_with(vec![agent(1, AgentStatus::Idle)], 1);
        app.plate = Plate::from_saved(vec![PlateItem {
            session: SessionId(session),
            pane: PaneId(pane),
            ..test_item(1, ItemState::YourMove)
        }]);
        app
    }

    /// Where ⏎ on item 1 of `app`, from session `session`, goes.
    fn target_of(app: &AppState, session: i64) -> Result<GoTarget, GoError> {
        go_target(app, RowKey::Item(PlateItemId(1)), SessionId(session))
    }

    #[rstest::rstest]
    fn go_target_of_an_agent_item_is_its_pane() {
        // Given item 1 from agent 1's pane in session 1.
        let app = with_item(1, 1);

        // When working out where ⏎ goes.
        let target = target_of(&app, 1);

        // Then it's the agent's pane.
        assert_eq!(
            target,
            Ok(GoTarget::Agent {
                session: SessionId(1),
                pane: PaneId(1)
            }),
            "an item from a live agent's pane should go to that pane"
        );
    }

    #[rstest::rstest]
    fn go_target_of_a_shell_pane_item_is_that_pane() {
        // Given item 1 from shell pane 50, split off session 1's agent.
        let mut app = with_item(1, 50);
        app.layouts
            .split(SessionId(1), Split::Right, test_entry(50));

        // When working out where ⏎ goes.
        let target = target_of(&app, 1);

        // Then it's the shell pane.
        assert_eq!(
            target,
            Ok(GoTarget::Shell {
                session: SessionId(1),
                pane: PaneId(50)
            }),
            "an item from a shell pane should go to that pane"
        );
    }

    #[rstest::rstest]
    fn go_target_of_an_item_whose_pane_is_gone_is_its_session() {
        // Given item 1 from pane 99, no longer in session 1.
        let app = with_item(1, 99);

        // When working out where ⏎ goes.
        let target = target_of(&app, 1);

        // Then it's the session.
        assert_eq!(
            target,
            Ok(GoTarget::Session(SessionId(1))),
            "an item whose pane is gone should go to its session"
        );
    }

    #[rstest::rstest]
    fn go_target_of_a_settled_sessions_item_is_its_session() {
        // Given item 1 from agent 1's pane, session 1 settled with the pane
        // still in its layout.
        let app = settle(with_item(1, 1), &[1]);

        // When working out where ⏎ goes.
        let target = target_of(&app, 1);

        // Then it's the session, so entering it resumes it.
        assert_eq!(
            target,
            Ok(GoTarget::Session(SessionId(1))),
            "an item from a settled session should go to the session"
        );
    }

    #[rstest::rstest]
    #[case::deleted(2, false)]
    #[case::deleting(1, true)]
    fn go_target_of_a_deleted_or_deleting_session_is_refused(
        #[case] session: i64,
        #[case] deleting: bool,
    ) {
        // Given item 1 from session `session`, which is gone or being
        // deleted.
        let mut app = with_item(session, 1);
        if deleting {
            app.sessions.deleting.insert(SessionId(session));
        }

        // When working out where ⏎ goes.
        let target = target_of(&app, session);

        // Then it's refused.
        assert_eq!(
            target,
            Err(GoError::SessionGone),
            "an item whose session is gone should go nowhere"
        );
    }

    #[rstest::rstest]
    fn unknown_verb_names_every_verb() {
        // Given / When checking the verb `fix`.
        let refused = verb("fix").map_err(|error| (error.code, error.message));

        // Then it's a bad request listing the 11 verbs in order.
        assert_eq!(
            refused,
            Err((
                ErrorCode::BadRequest,
                "unknown verb fix; use one of decide, review, read, walk, pick, approve, \
                 answer, reply, merge, note, idea"
                    .to_owned()
            )),
            "an unknown verb should say which ones orb takes"
        );
    }

    #[rstest::rstest]
    fn colonist_refusal_tells_it_to_report_to_its_chief() {
        // Given session 2, a colonist, running agent 2 in pane 2.
        let app = colony_with_colonists([AgentStatus::Idle; 3]);

        // When pane 2 adds a plate item.
        let refused = adding_session(&app, Some(PaneId(2)))
            .err()
            .map(|error| (error.code, error.hint));

        // Then it's refused with a hint to report to the chief.
        assert_eq!(
            refused,
            Some((
                ErrorCode::Colonist,
                Some("put it in your report to your chief".to_owned())
            )),
            "a colonist is told where its item goes instead"
        );
    }

    #[rstest::rstest]
    fn done_item_is_known() {
        // Given item 1, done.
        let mut app = state_with(vec![agent(1, AgentStatus::Idle)], 1);
        app.plate = Plate::from_saved(vec![test_item(1, ItemState::Done)]);

        // When looking for item 1 as a close does.
        let found = known_item(&app, PlateItemId(1)).map(|item| item.id);

        // Then it's found.
        assert_eq!(
            found,
            Ok(PlateItemId(1)),
            "closing a done item finds it, so a second close is no error"
        );
    }
}
