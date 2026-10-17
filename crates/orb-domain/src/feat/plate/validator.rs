//! Checks a plate request must pass before orb asks for it.

use crate::AppState;
use crate::feat::control::protocol::{ApiError, ErrorCode};
use crate::feat::control::target::{self, Resolved};
use crate::feat::control::validator::not_settled;
use crate::feat::plate::{PlateItem, Verb};
use crate::ids::{PaneId, PlateItemId};

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
        return Err(ApiError::new(ErrorCode::Colonist, COLONIST_REFUSED));
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
        Err(ApiError::new(ErrorCode::BadRequest, "item is done"))
    }
}

/// Item `id`, open or done; `not_found` when there's none.
pub(crate) fn known_item(app: &AppState, id: PlateItemId) -> Result<&PlateItem, ApiError> {
    app.plate
        .item(id)
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, format!("no item {}", id.0)))
}

#[cfg(test)]
mod tests {
    use super::{known_item, verb};
    use crate::feat::control::protocol::ErrorCode;
    use crate::feat::plate::state::test_item;
    use crate::feat::plate::{ItemState, Plate};
    use crate::feat::sessions::state::AgentStatus;
    use crate::ids::PlateItemId;
    use crate::intent_handler::test_support::{agent, state_with};

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
