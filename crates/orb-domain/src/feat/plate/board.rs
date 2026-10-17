//! The plate laid out for the screen.
//!
//! Four groups in a fixed order — Your move, Waiting on others, Parked and
//! Done today — each sorted its own way. orb adds its own Your move row for
//! every agent waiting for an approval or an answer, built from the agent's
//! status each time the board is laid out and never stored. Building a board
//! only reads the app's state.

use std::cmp::Reverse;
use std::time::SystemTime;

use super::state::{ItemState, PlateItem, Verb};
use crate::AppState;
use crate::feat::harness::HarnessId;
use crate::feat::sessions::state::{Agent, AgentStatus, Session};
use crate::ids::{PaneId, PlateItemId, SessionId};

/// Which group a row sits in, in the order the plate shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupKind {
    YourMove,
    Waiting,
    Parked,
    DoneToday,
}

impl GroupKind {
    /// Every group, in the order the plate shows them.
    pub const ALL: [Self; 4] = [Self::YourMove, Self::Waiting, Self::Parked, Self::DoneToday];
}

/// What the selection keys on: an item, or orb's row for the agent in a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RowKey {
    Item(PlateItemId),
    Agent(PaneId),
}

impl RowKey {
    /// The item's or the pane's number, which breaks ties in a sort.
    fn number(self) -> i64 {
        match self {
            Self::Item(id) => id.0,
            Self::Agent(pane) => pane.0,
        }
    }
}

/// What an agent orb speaks for is waiting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrbAsk {
    Approval,
    Answer,
}

/// Where a row comes from.
#[derive(Debug, Clone, Copy)]
pub enum RowSource<'a> {
    /// An item an agent added.
    Item(&'a PlateItem),
    /// orb's own row for an agent waiting on the user; built from its
    /// status, never stored.
    Orb(OrbAsk),
}

/// One row of the plate, with everything the screen shows for it.
#[derive(Debug, Clone)]
pub struct BoardRow<'a> {
    pub key: RowKey,
    pub source: RowSource<'a>,
    pub verb: Verb,
    /// The item's title, or `Approve <name>` / `Answer <name>`.
    pub title: String,
    pub session: SessionId,
    /// The session's title now while it exists, else the one the item saved.
    pub session_title: String,
    /// The session's project's title; `None` once the session is gone.
    pub project: Option<String>,
    pub harness: Option<HarnessId>,
    pub urgent: bool,
    /// Where the row's age counts from: the last state change, the close
    /// for a done item, the wait's start for orb's row (`None` if unknown).
    pub since: Option<SystemTime>,
}

impl BoardRow<'_> {
    /// Where the row sorts in Your move after urgency: orb's approval rows,
    /// then its answer rows, then items.
    fn rank(&self) -> u8 {
        match self.source {
            RowSource::Orb(OrbAsk::Approval) => 0,
            RowSource::Orb(OrbAsk::Answer) => 1,
            RowSource::Item(_) => 2,
        }
    }
}

/// One group's rows, sorted, and whether they show.
#[derive(Debug, Clone)]
pub struct Group<'a> {
    pub kind: GroupKind,
    pub rows: Vec<BoardRow<'a>>,
    pub open: bool,
}

/// The plate laid out for the screen: the four groups in order and the
/// selected row.
#[derive(Debug, Clone)]
pub struct Board<'a> {
    pub groups: [Group<'a>; 4],
    /// The row the view's cursor stands on among the open groups' rows
    /// (see [`PlateCursor::on`](super::view::PlateCursor::on)); `None` when they hold none.
    pub selected: Option<RowKey>,
}

impl<'a> Board<'a> {
    /// `app`'s plate as the screen shows it at `today` (local midnight):
    /// orb's rows mixed into Your move, items done before `today` left out,
    /// Done today open only while the view has it open. The project
    /// filter doesn't apply.
    ///
    /// Your move puts urgent rows first, then orb's approval rows, then its
    /// answer rows, then items, each oldest first (orb's rows with no wait
    /// start last); Waiting sorts oldest first, Parked and Done today
    /// newest first.
    #[must_use]
    pub fn new(app: &'a AppState, today: SystemTime) -> Self {
        let mut your_move = orb_rows(app);
        let mut waiting = Vec::new();
        let mut parked = Vec::new();
        let mut done = Vec::new();
        for item in app.plate.items() {
            let list = match item.state {
                ItemState::YourMove => &mut your_move,
                ItemState::Waiting => &mut waiting,
                ItemState::Parked { .. } => &mut parked,
                ItemState::Done if item_since(item) >= today => &mut done,
                ItemState::Done => continue,
            };
            list.push(item_row(app, item));
        }
        your_move.sort_by_key(|row| {
            (
                !row.urgent,
                row.rank(),
                row.since.is_none(),
                row.since,
                row.key.number(),
            )
        });
        waiting.sort_by_key(|row| (row.since, row.key.number()));
        parked.sort_by_key(|row| (Reverse(row.since), row.key.number()));
        done.sort_by_key(|row| (Reverse(row.since), row.key.number()));
        let group = |kind, rows, open| Group { kind, rows, open };
        let mut board = Self {
            groups: [
                group(GroupKind::YourMove, your_move, true),
                group(GroupKind::Waiting, waiting, true),
                group(GroupKind::Parked, parked, true),
                group(GroupKind::DoneToday, done, app.plate.view().done_open),
            ],
            selected: None,
        };
        board.selected = app.plate.view().cursor.on(&board.keys());
        board
    }

    /// The group `kind`.
    #[must_use]
    pub fn group(&self, kind: GroupKind) -> &Group<'a> {
        let [your_move, waiting, parked, done] = &self.groups;
        match kind {
            GroupKind::YourMove => your_move,
            GroupKind::Waiting => waiting,
            GroupKind::Parked => parked,
            GroupKind::DoneToday => done,
        }
    }

    /// The rows the cursor can stand on, top to bottom: those of open groups.
    #[must_use]
    pub fn keys(&self) -> Vec<RowKey> {
        self.groups
            .iter()
            .filter(|group| group.open)
            .flat_map(|group| group.rows.iter().map(|row| row.key))
            .collect()
    }

    /// The row with `key`, in any group.
    #[must_use]
    pub fn row(&self, key: RowKey) -> Option<&BoardRow<'a>> {
        self.groups
            .iter()
            .flat_map(|group| &group.rows)
            .find(|row| row.key == key)
    }

    /// The selected row.
    #[must_use]
    pub fn selected_row(&self) -> Option<&BoardRow<'a>> {
        self.row(self.selected?)
    }

    /// How many rows `kind` holds, folded or not.
    #[must_use]
    pub fn count(&self, kind: GroupKind) -> usize {
        self.group(kind).rows.len()
    }
}

/// orb's rows: every agent waiting for an approval or an answer, with a pane,
/// in a session that exists, isn't settled and isn't being deleted,
/// colonists included and the project filter ignored. Each is named after
/// the agent's pane, else the agent's title, else its session's title.
#[must_use]
fn orb_rows(app: &AppState) -> Vec<BoardRow<'_>> {
    let sessions = &app.sessions;
    sessions
        .all_agents()
        .filter_map(|agent| {
            let (ask, pane, session) = orb_ask(app, agent)?;
            let session_title = sessions.title(session);
            let name = app
                .layouts
                .entry(pane)
                .and_then(|entry| entry.name.clone())
                .or_else(|| agent.title.clone())
                .unwrap_or_else(|| session_title.clone());
            let (verb, title) = match ask {
                OrbAsk::Approval => (Verb::Approve, format!("Approve {name}")),
                OrbAsk::Answer => (Verb::Answer, format!("Answer {name}")),
            };
            Some(BoardRow {
                key: RowKey::Agent(pane),
                source: RowSource::Orb(ask),
                verb,
                title,
                session: session.id,
                session_title,
                project: sessions
                    .project(session.project)
                    .map(|project| project.title.clone()),
                harness: Some(agent.harness.clone()),
                urgent: false,
                since: agent.waiting_since,
            })
        })
        .collect()
}

/// How many rows Your move holds: the open items there plus orb's rows,
/// counted without laying out the board.
#[must_use]
pub fn your_move_count(app: &AppState) -> usize {
    let items = app
        .plate
        .items()
        .iter()
        .filter(|item| item.state == ItemState::YourMove)
        .count();
    let agents = app
        .sessions
        .all_agents()
        .filter(|agent| orb_ask(app, agent).is_some())
        .count();
    items + agents
}

/// What `agent` waits on the user for, its pane and its session, when orb
/// gives it a row: it waits for an approval or an answer, has a pane, and
/// its session exists, isn't settled and isn't being deleted.
fn orb_ask<'a>(app: &'a AppState, agent: &Agent) -> Option<(OrbAsk, PaneId, &'a Session)> {
    let sessions = &app.sessions;
    let ask = match agent.status {
        AgentStatus::NeedsApproval => OrbAsk::Approval,
        AgentStatus::NeedsInput => OrbAsk::Answer,
        _ => return None,
    };
    let launch = agent.pane.as_ref()?;
    let session = sessions
        .live(launch.session)
        .filter(|session| session.settled_at.is_none())?;
    Some((ask, launch.pane, session))
}

/// Where `item`'s age counts from: its close for a done item, else its last
/// state change.
fn item_since(item: &PlateItem) -> SystemTime {
    match item.state {
        ItemState::Done => item.done_at.unwrap_or(item.changed_at),
        _ => item.changed_at,
    }
}

/// `item`'s row, titled with its session as that session is called now, or
/// as it was called when the item was added once it's gone.
fn item_row<'a>(app: &'a AppState, item: &'a PlateItem) -> BoardRow<'a> {
    let sessions = &app.sessions;
    let session = sessions.session(item.session);
    BoardRow {
        key: RowKey::Item(item.id),
        source: RowSource::Item(item),
        verb: item.verb,
        title: item.title.clone(),
        session: item.session,
        session_title: session.map_or_else(
            || item.session_title.clone(),
            |session| sessions.title(session),
        ),
        project: session
            .and_then(|session| sessions.project(session.project))
            .map(|project| project.title.clone()),
        harness: item.harness.clone(),
        urgent: item.urgent,
        since: Some(item_since(item)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feat::plate::state::{Plate, test_item};
    use crate::ids::ProjectId;
    use crate::intent_handler::test_support::{
        agent, at, colony_with_colonists, named, settle, state_with,
    };

    /// Item `id` in `state`, last changed at second `changed`.
    fn item(id: i64, state: ItemState, changed: u64) -> PlateItem {
        PlateItem {
            changed_at: at(changed),
            done_at: (state == ItemState::Done).then(|| at(changed)),
            ..test_item(id, state)
        }
    }

    /// `agent` waiting since second `since`.
    fn waiting(agent: Agent, since: u64) -> Agent {
        Agent {
            waiting_since: Some(at(since)),
            ..agent
        }
    }

    /// Session 1 with an idle agent, holding `items`.
    fn plate_of(items: Vec<PlateItem>) -> AppState {
        let mut state = state_with(vec![agent(1, AgentStatus::Idle)], 1);
        state.plate = Plate::from_saved(items);
        state
    }

    /// The keys of `kind`'s rows on `state`'s board at the epoch.
    fn keys_in(state: &AppState, kind: GroupKind) -> Vec<RowKey> {
        Board::new(state, at(0))
            .group(kind)
            .rows
            .iter()
            .map(|row| row.key)
            .collect()
    }

    /// The group holding `key` on `state`'s board at the epoch.
    fn group_of(state: &AppState, key: RowKey) -> Option<GroupKind> {
        let board = Board::new(state, at(0));
        GroupKind::ALL
            .into_iter()
            .find(|&kind| board.group(kind).rows.iter().any(|row| row.key == key))
    }

    fn item_key(id: i64) -> RowKey {
        RowKey::Item(PlateItemId(id))
    }

    fn agent_key(pane: i64) -> RowKey {
        RowKey::Agent(PaneId(pane))
    }

    #[rstest::rstest]
    #[case(ItemState::YourMove, GroupKind::YourMove)]
    #[case(ItemState::Waiting, GroupKind::Waiting)]
    #[case(ItemState::Parked { by_user: true }, GroupKind::Parked)]
    #[case(ItemState::Parked { by_user: false }, GroupKind::Parked)]
    #[case(ItemState::Done, GroupKind::DoneToday)]
    fn items_land_in_the_group_of_their_state(#[case] state: ItemState, #[case] kind: GroupKind) {
        // Given an item in `state`, done today when done.
        let app = plate_of(vec![item(1, state, 10)]);

        // When laying out the board.
        let group = group_of(&app, item_key(1));

        // Then it sits in its state's group.
        assert_eq!(group, Some(kind), "an item in {state:?} lands in {kind:?}");
    }

    #[rstest::rstest]
    fn your_move_puts_urgent_items_first() {
        // Given an old item, an agent waiting for approval and a newer urgent item.
        let mut app = state_with(vec![waiting(agent(1, AgentStatus::NeedsApproval), 5)], 1);
        app.plate = Plate::from_saved(vec![
            item(1, ItemState::YourMove, 1),
            PlateItem {
                urgent: true,
                ..item(2, ItemState::YourMove, 50)
            },
        ]);

        // When laying out the board.
        let keys = keys_in(&app, GroupKind::YourMove);

        // Then the urgent item leads, ahead of orb's row too.
        assert_eq!(
            keys,
            vec![item_key(2), agent_key(1), item_key(1)],
            "urgent items sort before everything else in Your move"
        );
    }

    #[rstest::rstest]
    fn your_move_puts_approval_rows_then_answer_rows_before_items() {
        // Given an old item, an agent waiting for an answer and a later one
        // waiting for approval.
        let mut app = state_with(
            vec![
                waiting(agent(1, AgentStatus::NeedsInput), 2),
                waiting(agent(2, AgentStatus::NeedsApproval), 5),
            ],
            1,
        );
        app.plate = Plate::from_saved(vec![item(1, ItemState::YourMove, 0)]);

        // When laying out the board.
        let keys = keys_in(&app, GroupKind::YourMove);

        // Then approvals come first, then answers, then items.
        assert_eq!(
            keys,
            vec![agent_key(2), agent_key(1), item_key(1)],
            "orb's approval rows, then its answer rows, then items"
        );
    }

    #[rstest::rstest]
    fn your_move_items_sort_oldest_first() {
        // Given a newer item added before an older one.
        let app = plate_of(vec![
            item(1, ItemState::YourMove, 30),
            item(2, ItemState::YourMove, 10),
        ]);

        // When laying out the board.
        let keys = keys_in(&app, GroupKind::YourMove);

        // Then the older one leads.
        assert_eq!(
            keys,
            vec![item_key(2), item_key(1)],
            "Your move items sort oldest first"
        );
    }

    #[rstest::rstest]
    fn orb_rows_without_a_wait_start_sort_last() {
        // Given two agents waiting for approval, one with no wait start yet.
        let app = state_with(
            vec![
                agent(1, AgentStatus::NeedsApproval),
                waiting(agent(2, AgentStatus::NeedsApproval), 50),
            ],
            1,
        );

        // When laying out the board.
        let keys = keys_in(&app, GroupKind::YourMove);

        // Then the one with a wait start leads.
        assert_eq!(
            keys,
            vec![agent_key(2), agent_key(1)],
            "an orb row with no wait start sorts after those with one"
        );
    }

    #[rstest::rstest]
    fn waiting_sorts_oldest_first() {
        // Given a newer waiting item added before an older one.
        let app = plate_of(vec![
            item(1, ItemState::Waiting, 30),
            item(2, ItemState::Waiting, 10),
        ]);

        // When laying out the board.
        let keys = keys_in(&app, GroupKind::Waiting);

        // Then the older one leads.
        assert_eq!(
            keys,
            vec![item_key(2), item_key(1)],
            "Waiting sorts oldest first"
        );
    }

    #[rstest::rstest]
    fn parked_sorts_newest_first() {
        // Given an older parked item added before a newer one.
        let app = plate_of(vec![
            item(1, ItemState::Parked { by_user: true }, 10),
            item(2, ItemState::Parked { by_user: false }, 30),
        ]);

        // When laying out the board.
        let keys = keys_in(&app, GroupKind::Parked);

        // Then the newer one leads.
        assert_eq!(
            keys,
            vec![item_key(2), item_key(1)],
            "Parked sorts newest first"
        );
    }

    #[rstest::rstest]
    fn done_today_sorts_newest_first() {
        // Given an item done earlier added before one done later.
        let app = plate_of(vec![
            item(1, ItemState::Done, 10),
            item(2, ItemState::Done, 30),
        ]);

        // When laying out the board.
        let keys = keys_in(&app, GroupKind::DoneToday);

        // Then the later one leads.
        assert_eq!(
            keys,
            vec![item_key(2), item_key(1)],
            "Done today sorts newest first"
        );
    }

    #[rstest::rstest]
    fn done_today_leaves_out_items_done_before_midnight() {
        // Given an item done before midnight and one done after.
        let app = plate_of(vec![
            item(1, ItemState::Done, 50),
            item(2, ItemState::Done, 150),
        ]);

        // When laying out the board with midnight at 100 s.
        let board = Board::new(&app, at(100));

        // Then only today's shows.
        let keys: Vec<RowKey> = board
            .group(GroupKind::DoneToday)
            .rows
            .iter()
            .map(|row| row.key)
            .collect();
        assert_eq!(
            keys,
            vec![item_key(2)],
            "Done today holds only items done since midnight"
        );
    }

    #[rstest::rstest]
    #[case(false, false)]
    #[case(true, true)]
    fn done_today_rows_are_selectable_only_while_open(
        #[case] done_open: bool,
        #[case] selectable: bool,
    ) {
        // Given an item done today, with Done today open or folded.
        let mut app = plate_of(vec![item(1, ItemState::Done, 10)]);
        app.plate.view_mut().done_open = done_open;

        // When listing the rows the cursor can stand on.
        let keys = Board::new(&app, at(0)).keys();

        // Then the done item is among them only while Done today is open.
        assert_eq!(
            keys.contains(&item_key(1)),
            selectable,
            "a done row is selectable only while Done today is open ({done_open})"
        );
    }

    #[rstest::rstest]
    fn agent_waiting_for_approval_gets_an_approve_row() {
        // Given an agent waiting for approval.
        let app = state_with(
            vec![named(agent(1, AgentStatus::NeedsApproval), "payout retry")],
            1,
        );

        // When laying out the board.
        let board = Board::new(&app, at(0));

        // Then Your move holds orb's approve row for it.
        let row = board.group(GroupKind::YourMove).rows.first();
        assert_eq!(
            row.map(|row| (row.key, row.verb, row.title.as_str())),
            Some((agent_key(1), Verb::Approve, "Approve payout retry")),
            "an agent waiting for approval gets an approve row"
        );
    }

    #[rstest::rstest]
    fn agent_waiting_for_an_answer_gets_an_answer_row() {
        // Given an agent waiting for an answer.
        let app = state_with(
            vec![named(agent(1, AgentStatus::NeedsInput), "pick a region")],
            1,
        );

        // When laying out the board.
        let board = Board::new(&app, at(0));

        // Then Your move holds orb's answer row for it.
        let row = board.group(GroupKind::YourMove).rows.first();
        assert_eq!(
            row.map(|row| (row.key, row.verb, row.title.as_str())),
            Some((agent_key(1), Verb::Answer, "Answer pick a region")),
            "an agent waiting for an answer gets an answer row"
        );
    }

    #[rstest::rstest]
    #[case(Some("tests"), Some("fix"), "Approve tests")]
    #[case(None, Some("fix"), "Approve fix")]
    #[case(None, None, "Approve payments")]
    fn orb_row_is_named_after_the_pane_then_the_agent_then_the_session(
        #[case] pane_name: Option<&str>,
        #[case] agent_title: Option<&str>,
        #[case] title: &str,
    ) {
        // Given an agent waiting for approval in session "payments", with
        // its pane and the agent maybe named.
        let mut app = state_with(
            vec![Agent {
                title: agent_title.map(str::to_owned),
                ..agent(1, AgentStatus::NeedsApproval)
            }],
            1,
        );
        for session in &mut app.sessions.sessions {
            session.name = Some("payments".to_owned());
        }
        app.layouts
            .rename_pane(PaneId(1), pane_name.map(str::to_owned));

        // When building orb's rows.
        let rows = orb_rows(&app);

        // Then the row takes the first name there is.
        assert_eq!(
            rows.iter()
                .map(|row| row.title.as_str())
                .collect::<Vec<_>>(),
            vec![title],
            "orb's row is named after the pane, else the agent, else the session"
        );
    }

    #[rstest::rstest]
    fn orb_rows_include_colonists() {
        // Given a colony whose colonist waits for approval.
        let app = colony_with_colonists([
            AgentStatus::Idle,
            AgentStatus::NeedsApproval,
            AgentStatus::Idle,
        ]);

        // When building orb's rows.
        let keys: Vec<RowKey> = orb_rows(&app).iter().map(|row| row.key).collect();

        // Then the colonist has one.
        assert_eq!(
            keys,
            vec![agent_key(2)],
            "a colonist waiting for approval gets orb's row"
        );
    }

    #[rstest::rstest]
    fn orb_rows_leave_out_settled_sessions() {
        // Given an agent waiting for approval in a settled session.
        let app = settle(
            state_with(vec![agent(1, AgentStatus::NeedsApproval)], 1),
            &[1],
        );

        // When building orb's rows.
        let rows = orb_rows(&app);

        // Then there are none.
        assert!(rows.is_empty(), "a settled session's agents get no orb row");
    }

    #[rstest::rstest]
    fn orb_rows_leave_out_sessions_being_deleted() {
        // Given an agent waiting for approval in a session being deleted.
        let mut app = state_with(vec![agent(1, AgentStatus::NeedsApproval)], 1);
        app.sessions.deleting.insert(SessionId(1));

        // When building orb's rows.
        let rows = orb_rows(&app);

        // Then there are none.
        assert!(
            rows.is_empty(),
            "a session being deleted has its agents get no orb row"
        );
    }

    #[rstest::rstest]
    fn agent_that_stops_waiting_loses_its_row() {
        // Given an agent that waited for approval.
        let mut app = state_with(vec![agent(1, AgentStatus::NeedsApproval)], 1);

        // When it goes back to work.
        for agent in app.sessions.projects.iter_mut().flat_map(|p| &mut p.agents) {
            agent.status = AgentStatus::Working;
        }

        // Then Your move is empty.
        assert_eq!(
            keys_in(&app, GroupKind::YourMove),
            Vec::<RowKey>::new(),
            "an agent that stops waiting loses orb's row"
        );
    }

    #[rstest::rstest]
    #[case(AgentStatus::Working, false)]
    #[case(AgentStatus::Idle, true)]
    #[case(AgentStatus::Failed, false)]
    fn working_or_unread_agents_get_no_row(#[case] status: AgentStatus, #[case] unseen: bool) {
        // Given an agent that isn't waiting on the user.
        let app = state_with(
            vec![Agent {
                unseen,
                ..agent(1, status)
            }],
            1,
        );

        // When building orb's rows.
        let rows = orb_rows(&app);

        // Then there are none.
        assert!(
            rows.is_empty(),
            "an agent {status:?} (unread: {unseen}) gets no orb row"
        );
    }

    #[rstest::rstest]
    fn plate_ignores_the_project_filter() {
        // Given an agent waiting for approval and an item, with the sidebar
        // filtered to another project.
        let mut app = state_with(vec![agent(1, AgentStatus::NeedsApproval)], 1);
        app.plate = Plate::from_saved(vec![item(1, ItemState::YourMove, 10)]);
        app.sessions.filter = Some(ProjectId(9));

        // When laying out the board.
        let keys = keys_in(&app, GroupKind::YourMove);

        // Then both still show.
        assert_eq!(
            keys,
            vec![agent_key(1), item_key(1)],
            "the plate shows every project's rows whatever the filter"
        );
    }

    #[rstest::rstest]
    fn item_shows_its_sessions_current_title() {
        // Given an item whose session was renamed after the add.
        let mut app = plate_of(vec![item(1, ItemState::YourMove, 10)]);
        for session in &mut app.sessions.sessions {
            session.name = Some("renamed".to_owned());
        }

        // When laying out the board.
        let board = Board::new(&app, at(0));

        // Then the row shows the session's title now.
        assert_eq!(
            board.row(item_key(1)).map(|row| row.session_title.as_str()),
            Some("renamed"),
            "an item shows its session's current title"
        );
    }

    #[rstest::rstest]
    fn item_of_a_deleted_session_shows_its_saved_title() {
        // Given an item whose session is gone.
        let app = plate_of(vec![PlateItem {
            session: SessionId(9),
            ..item(1, ItemState::YourMove, 10)
        }]);

        // When laying out the board.
        let board = Board::new(&app, at(0));

        // Then the row shows the title the item saved, and no project.
        assert_eq!(
            board
                .row(item_key(1))
                .map(|row| (row.session_title.as_str(), row.project.clone())),
            Some(("session 1", None)),
            "a gone session's item shows its saved title"
        );
    }
}
