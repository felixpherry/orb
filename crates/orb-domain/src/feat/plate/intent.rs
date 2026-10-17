//! What the user's keys do to the plate: `<C-g> o` gives it the keys, and
//! every key inside acts on its rows.

use std::time::SystemTime;

use super::board::{Board, RowKey};
use super::links;
use super::state::PlateItem;
use super::validator::{GoError, GoTarget, SESSION_GONE, go_target};
use super::view::{PlatePrefix, PlateUndo};
use crate::command::SessionsCommand;
use crate::feat::picker::intent::{pick, pick_agent, pick_session};
use crate::ids::{PaneId, PlateItemId, SessionId};
use crate::{AppState, Command, Focus};

/// A key the plate takes while it has the keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlateKey {
    /// `j` or `↓`.
    Next,
    /// `k` or `↑`.
    Prev,
    /// `G`.
    Last,
    /// `<C-d>`.
    HalfPageDown,
    /// `<C-u>`.
    HalfPageUp,
    /// `g`: the first of `gg`.
    G,
    /// `⏎`.
    Go,
    /// `x`.
    Done,
    /// `p`.
    Park,
    /// `u`.
    Undo,
    /// `z`: open or fold Done today.
    ToggleDone,
    /// `Esc`.
    Close,
    /// `<C-g>`: the first of `<C-g> o`.
    Leader,
    /// `o`, which acts only after `<C-g>`.
    O,
    /// `1`–`9`: open that link of the selected item.
    Link(usize),
    /// Any other key: it ends a sequence and does nothing.
    Other,
}

/// `<C-g> o` from the sidebar or a pane: the plate takes the keys,
/// remembering where they were, and floats over the shown session when
/// there is one. From a pane, the pane loses the keys (`Detach`). Inside
/// the plate it closes it, as Esc does.
pub(crate) fn open(state: &mut AppState) -> Vec<Command> {
    let from = state.focus;
    let commands = match from {
        Focus::Sidebar => vec![],
        Focus::Pane => vec![Command::Detach],
        Focus::Plate => return close(state),
        Focus::Picker | Focus::Rename | Focus::Search => return vec![],
    };
    let view = state.plate.view_mut();
    view.back = from;
    view.pending = None;
    state.focus = Focus::Plate;
    commands
}

/// A key inside the plate at `now`. A pending `g` or `<C-g>` is taken
/// first: `g` then `g` selects the first row, `g` then `x` opens the
/// selected item's first link, `<C-g>` then `o` closes, and anything else
/// after either only ends it. Without one: the moves, `⏎`, `x`, `p`, `u`,
/// `z`, `Esc`, a digit opening that link, and `g` / `<C-g>` starting a
/// sequence. Nothing happens unless the plate has the keys.
pub(crate) fn key(state: &mut AppState, key: PlateKey, now: SystemTime) -> Vec<Command> {
    if state.focus != Focus::Plate {
        return vec![];
    }
    match (state.plate.view_mut().pending.take(), key) {
        (Some(PlatePrefix::G), PlateKey::G) => select(state, now, Pick::First),
        (Some(PlatePrefix::G), PlateKey::Done) => return links::open_link(state, now, 1),
        (Some(PlatePrefix::Leader), PlateKey::O) | (None, PlateKey::Close) => return close(state),
        (None, PlateKey::Next) => select(state, now, Pick::Down(1)),
        (None, PlateKey::Prev) => select(state, now, Pick::Up(1)),
        (None, PlateKey::Last) => select(state, now, Pick::Last),
        (None, PlateKey::HalfPageDown) => select(state, now, Pick::Down(half_page(state))),
        (None, PlateKey::HalfPageUp) => select(state, now, Pick::Up(half_page(state))),
        (None, PlateKey::G) => state.plate.view_mut().pending = Some(PlatePrefix::G),
        (None, PlateKey::Leader) => state.plate.view_mut().pending = Some(PlatePrefix::Leader),
        (None, PlateKey::ToggleDone) => toggle_done(state),
        (None, PlateKey::Go) => return go(state, now),
        (None, PlateKey::Done) => return change(state, now, PlateItem::finished),
        (None, PlateKey::Park) => return change(state, now, PlateItem::parked_or_back),
        (None, PlateKey::Undo) => return undo(state, now),
        (None, PlateKey::Link(n)) => return links::open_link(state, now, n),
        (Some(_), _) | (None, PlateKey::O | PlateKey::Other) => {}
    }
    vec![]
}

/// Which row a move selects.
#[derive(Debug, Clone, Copy)]
enum Pick {
    First,
    Last,
    /// This many rows further down, stopping at the last.
    Down(usize),
    /// This many rows further up, stopping at the first.
    Up(usize),
}

/// The rows the cursor can stand on at `now`, top to bottom, and the
/// selected one.
fn rows(state: &AppState, now: SystemTime) -> (Vec<RowKey>, Option<RowKey>) {
    let board = Board::new(state, state.zone.midnight(now));
    (board.keys(), board.selected)
}

/// Half the rows the list last showed, at least one.
fn half_page(state: &AppState) -> usize {
    (state.plate.view().page / 2).max(1)
}

/// Selects the row `pick` names among those shown at `now`; nothing with no
/// rows.
fn select(state: &mut AppState, now: SystemTime, pick: Pick) {
    let (keys, selected) = rows(state, now);
    let Some(last) = keys.len().checked_sub(1) else {
        return;
    };
    let at = selected
        .and_then(|key| keys.iter().position(|&row| row == key))
        .unwrap_or(0);
    let to = match pick {
        Pick::First => 0,
        Pick::Last => last,
        Pick::Down(rows) => at.saturating_add(rows).min(last),
        Pick::Up(rows) => at.saturating_sub(rows),
    };
    if let Some(&key) = keys.get(to) {
        state.plate.view_mut().cursor.select(key, keys);
    }
}

/// Gives the keys back where they came from: to the shown session's pane
/// (`Attach`, which gives the pane its keys again) when they came from a
/// pane and a session is still shown, else to the sidebar.
fn close(state: &mut AppState) -> Vec<Command> {
    let view = state.plate.view_mut();
    view.pending = None;
    let back = view.back;
    match (back, state.shown_session()) {
        (Focus::Pane, Some(shown)) => {
            state.focus = Focus::Pane;
            vec![Command::Attach(shown)]
        }
        _ => {
            state.focus = Focus::Sidebar;
            vec![]
        }
    }
}

/// `⏎`: gives the keys back where they came from, then shows the selected
/// row's pane with the keys in it, or enters its session when the pane is
/// gone or the session settled (which resumes it). When the session is
/// gone, the message box says so and the plate keeps the keys.
fn go(state: &mut AppState, now: SystemTime) -> Vec<Command> {
    let picked = {
        let board = Board::new(state, state.zone.midnight(now));
        board.selected_row().map(|row| (row.key, row.session))
    };
    let Some((key, session)) = picked else {
        return vec![];
    };
    let target = match go_target(state, key, session) {
        Ok(target) => target,
        Err(GoError::SessionGone) => {
            state.sessions.error = Some(SESSION_GONE.to_owned());
            return vec![];
        }
    };
    give_back(state);
    match target {
        GoTarget::Agent { session, pane } => pick_agent(state, session, pane),
        GoTarget::Shell { session, pane } => enter_pane(state, session, pane),
        GoTarget::Session(session) => pick_session(state, session),
    }
}

/// Hands the keys back to where they came into the plate from: the pane
/// while a session is shown, else the sidebar.
pub(super) fn give_back(state: &mut AppState) {
    let view = state.plate.view_mut();
    view.pending = None;
    state.focus = match (view.back, state.shown_session()) {
        (Focus::Pane, Some(_)) => Focus::Pane,
        _ => Focus::Sidebar,
    };
}

/// Shows pane `pane` of `session` in its tab and enters the session with
/// the keys in it, as ⏎ on its card does with that pane focused.
pub(super) fn enter_pane(state: &mut AppState, session: SessionId, pane: PaneId) -> Vec<Command> {
    let from = state.jump_from();
    state.layouts.reveal(session, pane);
    let row = state.entry_row(session);
    let mut commands = pick(state, from, row, row);
    commands.push(Command::Sessions(SessionsCommand::SaveLayout(session)));
    commands
}

/// `x` or `p`: makes `change` at `now` to the selected item, keeping how it
/// was for `u`, and asks for the item to be saved. The row is chosen again
/// among the rows shown now first, so the cursor moves on to its neighbour
/// in today's order if the item leaves them. orb's rows, and items `change`
/// leaves as they are, do nothing.
fn change(
    state: &mut AppState,
    now: SystemTime,
    change: fn(&PlateItem, SystemTime) -> Option<PlateItem>,
) -> Vec<Command> {
    let (keys, selected) = rows(state, now);
    let Some(key @ RowKey::Item(id)) = selected else {
        return vec![];
    };
    let Some(item) = state.plate.item(id) else {
        return vec![];
    };
    let before = PlateUndo {
        item: id,
        state: item.state,
        changed_at: item.changed_at,
    };
    let Some(changed) = change(item, now) else {
        return vec![];
    };
    let view = state.plate.view_mut();
    view.undo = Some(before);
    view.cursor.select(key, keys);
    state.plate.replace(changed);
    vec![save(id)]
}

/// `u`: puts the item of the last `x` or `p` back as it was, selects it, and
/// asks for it to be saved; then there's nothing left to undo. Nothing
/// happens when there's nothing to undo or the item is gone.
fn undo(state: &mut AppState, now: SystemTime) -> Vec<Command> {
    let Some(before) = state.plate.view_mut().undo.take() else {
        return vec![];
    };
    let Some(restored) = state
        .plate
        .item(before.item)
        .map(|item| item.restored(before.state, before.changed_at))
    else {
        return vec![];
    };
    state.plate.replace(restored);
    let key = RowKey::Item(before.item);
    let (keys, _) = rows(state, now);
    if keys.contains(&key) {
        state.plate.view_mut().cursor.select(key, keys);
    }
    vec![save(before.item)]
}

/// The sessions actor's save of item `id` as the state has it.
fn save(id: PlateItemId) -> Command {
    Command::Sessions(SessionsCommand::SavePlateItem(id))
}

/// Opens Done today, or folds it.
fn toggle_done(state: &mut AppState) {
    let view = state.plate.view_mut();
    view.done_open = !view.done_open;
}

#[cfg(test)]
mod tests {
    use jiff::tz::{self, TimeZone};

    use super::{PlateKey, key, open};
    use crate::command::SessionsCommand;
    use crate::feat::layout::state::test_entry;
    use crate::feat::layout::tree::Split;
    use crate::feat::picker::intent::pick_agent;
    use crate::feat::plate::board::{Board, GroupKind, RowKey};
    use crate::feat::plate::state::{ItemState, Plate, PlateItem, test_item};
    use crate::feat::sessions::state::AgentStatus;
    use crate::ids::{PaneId, PlateItemId, SessionId};
    use crate::intent_handler::test_support::{
        agent, at, settle, shown_focus, state_with, with_layout,
    };
    use crate::{AppState, Command, Focus, Intent, IntentHandler, LocalZone};

    /// The clock the tests press keys at.
    const NOW: u64 = 1000;

    /// Presses `keys` in order at [`NOW`], returning every command.
    fn pressed(state: &mut AppState, keys: &[PlateKey]) -> Vec<Command> {
        keys.iter()
            .flat_map(|&pressed| key(state, pressed, at(NOW)))
            .collect()
    }

    fn item_key(id: i64) -> RowKey {
        RowKey::Item(PlateItemId(id))
    }

    /// The selected row on `state`'s board at [`NOW`].
    fn selected(state: &AppState) -> Option<RowKey> {
        Board::new(state, state.zone.midnight(at(NOW))).selected
    }

    /// The start screen with session 1's idle agent and `items`, the plate
    /// given the keys from the sidebar.
    fn plate_of(items: Vec<PlateItem>) -> AppState {
        let mut state = state_with(vec![agent(1, AgentStatus::Idle)], 1);
        state.plate = Plate::from_saved(items);
        open(&mut state);
        state
    }

    /// [`plate_of`] items 1 to `last`, all in Your move.
    fn your_move(last: i64) -> AppState {
        plate_of(
            (1..=last)
                .map(|id| test_item(id, ItemState::YourMove))
                .collect(),
        )
    }

    /// Session 1 shown, the plate given the keys from `from`.
    fn opened_over_a_session(from: Focus) -> AppState {
        let mut state = with_layout(from);
        open(&mut state);
        state
    }

    #[rstest::rstest]
    fn open_plate_over_a_shown_session_floats_it() {
        // Given session 1 shown, the sidebar with the keys.
        let mut state = with_layout(Focus::Sidebar);

        // When pressing `<C-g> o`.
        IntentHandler::handle(&Intent::OpenPlate, &mut state);

        // Then the plate has the keys and floats over the session.
        assert_eq!(
            (state.focus, state.plate_floats()),
            (Focus::Plate, true),
            "the plate should float over the shown session with the keys"
        );
    }

    #[rstest::rstest]
    fn open_plate_on_the_start_screen_only_moves_the_keys() {
        // Given no session shown, the sidebar with the keys.
        let mut state = state_with(vec![agent(1, AgentStatus::Idle)], 1);

        // When pressing `<C-g> o`.
        IntentHandler::handle(&Intent::OpenPlate, &mut state);

        // Then the plate has the keys and doesn't float.
        assert_eq!(
            (state.focus, state.plate_floats()),
            (Focus::Plate, false),
            "the start-screen plate should take the keys where it is"
        );
    }

    #[rstest::rstest]
    fn open_plate_from_a_pane_returns_detach() {
        // Given the keys in session 1's pane.
        let mut state = with_layout(Focus::Pane);

        // When pressing `<C-g> o`.
        let commands = IntentHandler::handle(&Intent::OpenPlate, &mut state);

        // Then the pane loses the keys.
        assert_eq!(
            commands,
            vec![Command::Detach],
            "opening the plate from a pane should detach the pane"
        );
    }

    #[rstest::rstest]
    #[case::sidebar(Focus::Sidebar)]
    #[case::pane(Focus::Pane)]
    fn open_plate_remembers_where_the_keys_were(#[case] from: Focus) {
        // Given session 1 shown, the keys at `from`.
        let mut state = with_layout(from);

        // When pressing `<C-g> o`.
        IntentHandler::handle(&Intent::OpenPlate, &mut state);

        // Then the plate gives the keys back there when it closes.
        assert_eq!(
            state.plate.view().back,
            from,
            "the plate should remember where the keys came from"
        );
    }

    #[rstest::rstest]
    fn open_plate_inside_the_plate_closes_it() {
        // Given the plate over session 1, opened from the sidebar.
        let mut state = opened_over_a_session(Focus::Sidebar);

        // When `<C-g> o` arrives again.
        IntentHandler::handle(&Intent::OpenPlate, &mut state);

        // Then the sidebar has the keys back.
        assert_eq!(
            state.focus,
            Focus::Sidebar,
            "<C-g> o inside the plate should close it"
        );
    }

    #[rstest::rstest]
    fn esc_gives_the_keys_back_to_the_pane() {
        // Given the plate over session 1, opened from its pane.
        let mut state = opened_over_a_session(Focus::Pane);

        // When pressing Esc.
        pressed(&mut state, &[PlateKey::Close]);

        // Then the pane has the keys back.
        assert_eq!(
            state.focus,
            Focus::Pane,
            "Esc should give the pane its keys"
        );
    }

    #[rstest::rstest]
    fn esc_gives_the_keys_back_to_the_sidebar() {
        // Given the plate over session 1, opened from the sidebar.
        let mut state = opened_over_a_session(Focus::Sidebar);

        // When pressing Esc.
        pressed(&mut state, &[PlateKey::Close]);

        // Then the sidebar has the keys back.
        assert_eq!(
            state.focus,
            Focus::Sidebar,
            "Esc should give the sidebar its keys"
        );
    }

    #[rstest::rstest]
    fn esc_back_into_a_pane_returns_attach() {
        // Given the plate over session 1, opened from its pane.
        let mut state = opened_over_a_session(Focus::Pane);

        // When pressing Esc.
        let commands = pressed(&mut state, &[PlateKey::Close]);

        // Then the loop is asked to give session 1's pane its keys.
        assert_eq!(
            commands,
            vec![Command::Attach(SessionId(1))],
            "going back into the pane should attach the shown session"
        );
    }

    #[rstest::rstest]
    fn esc_with_the_session_no_longer_shown_goes_to_the_sidebar() {
        // Given the plate opened from session 1's pane, the session no
        // longer attached.
        let mut state = opened_over_a_session(Focus::Pane);
        state.attached.clear();

        // When pressing Esc.
        pressed(&mut state, &[PlateKey::Close]);

        // Then the sidebar has the keys.
        assert_eq!(
            state.focus,
            Focus::Sidebar,
            "with no pane shown, the keys should go to the sidebar"
        );
    }

    #[rstest::rstest]
    fn leader_then_o_closes_the_plate() {
        // Given the plate over session 1, opened from the sidebar.
        let mut state = opened_over_a_session(Focus::Sidebar);

        // When pressing `<C-g>` then `o`.
        pressed(&mut state, &[PlateKey::Leader, PlateKey::O]);

        // Then the sidebar has the keys back.
        assert_eq!(
            state.focus,
            Focus::Sidebar,
            "<C-g> o inside the plate should close it"
        );
    }

    #[rstest::rstest]
    fn leader_then_another_key_only_ends_the_sequence() {
        // Given items 1 and 2, the cursor on item 1.
        let mut state = your_move(2);

        // When pressing `<C-g>` then `j`.
        pressed(&mut state, &[PlateKey::Leader, PlateKey::Next]);

        // Then the cursor didn't move and no sequence is in progress.
        assert_eq!(
            (selected(&state), state.plate.view().pending),
            (Some(item_key(1)), None),
            "a key after <C-g> should only end the sequence"
        );
    }

    #[rstest::rstest]
    fn o_alone_does_nothing() {
        // Given the plate over session 1.
        let mut state = opened_over_a_session(Focus::Sidebar);

        // When pressing `o`.
        pressed(&mut state, &[PlateKey::O]);

        // Then the plate keeps the keys.
        assert_eq!(
            state.focus,
            Focus::Plate,
            "o without <C-g> before it should do nothing"
        );
    }

    #[rstest::rstest]
    fn keys_do_nothing_unless_the_plate_has_them() {
        // Given items 1 and 2, the sidebar with the keys.
        let mut state = your_move(2);
        state.focus = Focus::Sidebar;
        let before = state.plate.view().clone();

        // When a plate key arrives.
        pressed(
            &mut state,
            &[PlateKey::Next, PlateKey::G, PlateKey::ToggleDone],
        );

        // Then the plate's view is as it was.
        assert_eq!(
            state.plate.view(),
            &before,
            "the plate should ignore keys it doesn't have"
        );
    }

    #[rstest::rstest]
    fn j_selects_the_next_row() {
        // Given items 1 to 3, the cursor on item 1.
        let mut state = your_move(3);

        // When pressing `j`.
        pressed(&mut state, &[PlateKey::Next]);

        // Then item 2 is selected.
        assert_eq!(selected(&state), Some(item_key(2)), "j moves down one row");
    }

    #[rstest::rstest]
    fn j_on_the_last_row_stays() {
        // Given items 1 to 3, the cursor on item 3.
        let mut state = your_move(3);
        pressed(&mut state, &[PlateKey::Last]);

        // When pressing `j`.
        pressed(&mut state, &[PlateKey::Next]);

        // Then item 3 is still selected.
        assert_eq!(
            selected(&state),
            Some(item_key(3)),
            "j on the last row stays there"
        );
    }

    #[rstest::rstest]
    fn k_selects_the_previous_row() {
        // Given items 1 to 3, the cursor on item 3.
        let mut state = your_move(3);
        pressed(&mut state, &[PlateKey::Last]);

        // When pressing `k`.
        pressed(&mut state, &[PlateKey::Prev]);

        // Then item 2 is selected.
        assert_eq!(selected(&state), Some(item_key(2)), "k moves up one row");
    }

    #[rstest::rstest]
    fn k_on_the_first_row_stays() {
        // Given items 1 to 3, the cursor on item 1.
        let mut state = your_move(3);

        // When pressing `k`.
        pressed(&mut state, &[PlateKey::Prev]);

        // Then item 1 is still selected.
        assert_eq!(
            selected(&state),
            Some(item_key(1)),
            "k on the first row stays there"
        );
    }

    #[rstest::rstest]
    fn g_g_selects_the_first_row() {
        // Given items 1 to 3, the cursor on item 3.
        let mut state = your_move(3);
        pressed(&mut state, &[PlateKey::Last]);

        // When pressing `g` twice.
        pressed(&mut state, &[PlateKey::G, PlateKey::G]);

        // Then item 1 is selected.
        assert_eq!(selected(&state), Some(item_key(1)), "gg goes to the top");
    }

    #[rstest::rstest]
    fn g_then_another_key_only_ends_the_g() {
        // Given items 1 to 3, the cursor on item 3.
        let mut state = your_move(3);
        pressed(&mut state, &[PlateKey::Last]);

        // When pressing `g` then `k`.
        pressed(&mut state, &[PlateKey::G, PlateKey::Prev]);

        // Then the cursor didn't move and no sequence is in progress.
        assert_eq!(
            (selected(&state), state.plate.view().pending),
            (Some(item_key(3)), None),
            "a key after g should only end the g"
        );
    }

    #[rstest::rstest]
    fn capital_g_selects_the_last_row() {
        // Given items 1 to 3, the cursor on item 1.
        let mut state = your_move(3);

        // When pressing `G`.
        pressed(&mut state, &[PlateKey::Last]);

        // Then item 3 is selected.
        assert_eq!(selected(&state), Some(item_key(3)), "G goes to the bottom");
    }

    #[rstest::rstest]
    fn ctrl_d_moves_half_a_page_down() {
        // Given items 1 to 10, a 6-row list, the cursor on item 1.
        let mut state = your_move(10);
        state.plate.view_mut().page = 6;

        // When pressing `<C-d>`.
        pressed(&mut state, &[PlateKey::HalfPageDown]);

        // Then item 4 is selected, 3 rows down.
        assert_eq!(
            selected(&state),
            Some(item_key(4)),
            "<C-d> moves half the list's rows down"
        );
    }

    #[rstest::rstest]
    fn ctrl_u_moves_half_a_page_up() {
        // Given items 1 to 10, a 6-row list, the cursor on item 10.
        let mut state = your_move(10);
        state.plate.view_mut().page = 6;
        pressed(&mut state, &[PlateKey::Last]);

        // When pressing `<C-u>`.
        pressed(&mut state, &[PlateKey::HalfPageUp]);

        // Then item 7 is selected, 3 rows up.
        assert_eq!(
            selected(&state),
            Some(item_key(7)),
            "<C-u> moves half the list's rows up"
        );
    }

    #[rstest::rstest]
    fn moves_skip_rows_of_a_folded_done_today() {
        // Given item 1 in Your move and item 2 done today, folded.
        let mut state = plate_of(vec![
            test_item(1, ItemState::YourMove),
            test_item(2, ItemState::Done),
        ]);

        // When pressing `G`.
        pressed(&mut state, &[PlateKey::Last]);

        // Then item 1 is selected.
        assert_eq!(
            selected(&state),
            Some(item_key(1)),
            "a folded Done today's rows can't be selected"
        );
    }

    /// Whether Done today shows its rows on `state`'s board at [`NOW`].
    fn done_today_open(state: &AppState) -> bool {
        Board::new(state, state.zone.midnight(at(NOW)))
            .group(GroupKind::DoneToday)
            .open
    }

    #[rstest::rstest]
    fn z_opens_done_today() {
        // Given item 1 done today, folded.
        let mut state = plate_of(vec![test_item(1, ItemState::Done)]);

        // When pressing `z`.
        pressed(&mut state, &[PlateKey::ToggleDone]);

        // Then Done today is open.
        assert!(done_today_open(&state), "z should open Done today");
    }

    #[rstest::rstest]
    fn z_again_folds_done_today() {
        // Given Done today opened with `z`.
        let mut state = plate_of(vec![test_item(1, ItemState::Done)]);
        pressed(&mut state, &[PlateKey::ToggleDone]);

        // When pressing `z` again.
        pressed(&mut state, &[PlateKey::ToggleDone]);

        // Then Done today is folded.
        assert!(!done_today_open(&state), "z again should fold Done today");
    }

    #[rstest::rstest]
    fn cursor_stays_on_its_item_when_an_item_is_added_above() {
        // Given items 1 to 3, the cursor moved onto item 2.
        let mut state = your_move(3);
        pressed(&mut state, &[PlateKey::Next]);

        // When item 4 arrives, older than the rest, so it sorts first.
        state.plate.add(PlateItem {
            changed_at: at(5),
            ..test_item(4, ItemState::YourMove)
        });

        // Then item 2 is still selected.
        assert_eq!(
            selected(&state),
            Some(item_key(2)),
            "a new row above shouldn't move the selection"
        );
    }

    #[rstest::rstest]
    fn cursor_moves_on_when_its_orb_row_leaves() {
        // Given agent 1 waiting for approval and item 1, the cursor moved
        // onto orb's row for the agent.
        let mut state = state_with(vec![agent(1, AgentStatus::NeedsApproval)], 1);
        state.plate = Plate::from_saved(vec![test_item(1, ItemState::YourMove)]);
        open(&mut state);
        pressed(&mut state, &[PlateKey::Next, PlateKey::Prev]);

        // When the agent goes back to work.
        for agent in state
            .sessions
            .projects
            .iter_mut()
            .flat_map(|project| &mut project.agents)
        {
            agent.status = AgentStatus::Working;
        }

        // Then item 1 is selected.
        assert_eq!(
            selected(&state),
            Some(item_key(1)),
            "the selection should move on once orb's row leaves"
        );
    }

    #[rstest::rstest]
    fn done_today_counts_from_midnight_in_the_users_zone() {
        // Given a zone 7 hours ahead of UTC, item 1 in Your move, and item 2
        // done after local midnight but before UTC midnight.
        const DAY: u64 = 86_400;
        let now = at(10 * DAY + 2 * 3600);
        let done = at(10 * DAY - 4 * 3600);
        let mut state = plate_of(vec![
            test_item(1, ItemState::YourMove),
            PlateItem {
                changed_at: done,
                done_at: Some(done),
                ..test_item(2, ItemState::Done)
            },
        ]);
        state.zone = LocalZone(TimeZone::fixed(tz::offset(7)));
        key(&mut state, PlateKey::ToggleDone, now);

        // When pressing `G`.
        key(&mut state, PlateKey::Last, now);

        // Then item 2 is selected: it was done today here.
        let today = state.zone.midnight(now);
        assert_eq!(
            Board::new(&state, today).selected,
            Some(item_key(2)),
            "today should start at the user's midnight, not UTC's"
        );
    }

    /// The start screen with session 1's idle agent in pane 1 and item 1
    /// from session `session`'s pane `pane`, before the plate opens.
    fn with_item(session: i64, pane: i64) -> AppState {
        let mut state = state_with(vec![agent(1, AgentStatus::Idle)], 1);
        state.plate = Plate::from_saved(vec![PlateItem {
            session: SessionId(session),
            pane: PaneId(pane),
            ..test_item(1, ItemState::YourMove)
        }]);
        state
    }

    /// Opens the plate on `state` from the sidebar and presses ⏎, returning
    /// the commands.
    fn enter(state: &mut AppState) -> Vec<Command> {
        open(state);
        pressed(state, &[PlateKey::Go])
    }

    #[rstest::rstest]
    fn enter_on_an_agent_item_shows_its_pane_with_the_keys() {
        // Given item 1 from agent 1's pane, nothing shown.
        let mut state = with_item(1, 1);

        // When pressing ⏎ on it in the plate.
        enter(&mut state);

        // Then session 1 shows pane 1 with the keys in it.
        assert_eq!(
            (state.focus, state.shown_session(), shown_focus(&state)),
            (Focus::Pane, Some(SessionId(1)), Some(PaneId(1))),
            "⏎ on an agent's item should show its pane with the keys"
        );
    }

    #[rstest::rstest]
    fn enter_on_an_agent_item_returns_what_picking_the_agent_returns() {
        // Given item 1 from agent 1's pane, and a second such state with
        // the keys in the sidebar, as ⏎ leaves them before it picks.
        let mut state = with_item(1, 1);
        let mut picked = with_item(1, 1);
        picked.focus = Focus::Sidebar;
        let expected = pick_agent(&mut picked, SessionId(1), PaneId(1));

        // When pressing ⏎ on it in the plate.
        let commands = enter(&mut state);

        // Then the commands are the session picker's for that agent.
        assert_eq!(
            commands, expected,
            "⏎ on an agent's item should jump as picking the agent does"
        );
    }

    #[rstest::rstest]
    fn enter_on_a_shell_pane_item_shows_that_pane() {
        // Given item 1 from shell pane 50, split off agent 1's pane, with
        // the agent's pane focused.
        let mut state = with_item(1, 50);
        state
            .layouts
            .split(SessionId(1), Split::Right, test_entry(50));
        state.layouts.reveal(SessionId(1), PaneId(1));

        // When pressing ⏎ on it in the plate.
        enter(&mut state);

        // Then session 1 shows the shell pane with the keys in it.
        assert_eq!(
            (state.focus, shown_focus(&state)),
            (Focus::Pane, Some(PaneId(50))),
            "⏎ on a shell pane's item should show that pane"
        );
    }

    #[rstest::rstest]
    fn enter_on_an_item_whose_pane_is_gone_enters_its_session() {
        // Given item 1 from pane 99, no longer in session 1.
        let mut state = with_item(1, 99);

        // When pressing ⏎ on it in the plate.
        enter(&mut state);

        // Then session 1 is shown with the keys in its pane.
        assert_eq!(
            (state.focus, state.shown_session()),
            (Focus::Pane, Some(SessionId(1))),
            "⏎ on an item whose pane is gone should enter its session"
        );
    }

    #[rstest::rstest]
    fn enter_on_a_settled_sessions_item_resumes_it() {
        // Given item 1 from agent 1's pane, session 1 settled.
        let mut state = settle(with_item(1, 1), &[1]);

        // When pressing ⏎ on it in the plate.
        let commands = enter(&mut state);

        // Then session 1 is un-settled.
        assert!(
            commands.contains(&Command::Sessions(SessionsCommand::UnsettleSession(
                SessionId(1)
            ))),
            "⏎ on a settled session's item should resume it, got {commands:?}"
        );
    }

    #[rstest::rstest]
    fn enter_on_an_item_whose_session_is_gone_says_so() {
        // Given item 1 from session 2, which is gone.
        let mut state = with_item(2, 2);

        // When pressing ⏎ on it in the plate.
        enter(&mut state);

        // Then the message box says the session is gone.
        assert_eq!(
            state.sessions.error.as_deref(),
            Some("the session is gone"),
            "⏎ on an item whose session is gone should say so"
        );
    }

    #[rstest::rstest]
    fn enter_on_an_item_whose_session_is_gone_keeps_the_keys() {
        // Given item 1 from session 2, which is gone.
        let mut state = with_item(2, 2);

        // When pressing ⏎ on it in the plate.
        enter(&mut state);

        // Then the plate still has the keys.
        assert_eq!(
            state.focus,
            Focus::Plate,
            "the plate should keep the keys when ⏎ can't go anywhere"
        );
    }

    #[rstest::rstest]
    fn enter_on_an_orb_row_shows_the_agents_pane() {
        // Given agent 1 waiting for approval, so orb's row for it is the
        // only one, and nothing shown.
        let mut state = state_with(vec![agent(1, AgentStatus::NeedsApproval)], 1);

        // When pressing ⏎ on it in the plate.
        enter(&mut state);

        // Then session 1 shows the agent's pane with the keys in it.
        assert_eq!(
            (state.focus, state.shown_session(), shown_focus(&state)),
            (Focus::Pane, Some(SessionId(1)), Some(PaneId(1))),
            "⏎ on orb's row should show the waiting agent's pane"
        );
    }

    /// Item `id` on `state`'s plate.
    fn item(state: &AppState, id: i64) -> Option<&PlateItem> {
        state.plate.item(PlateItemId(id))
    }

    /// Item `id`'s state on `state`'s plate.
    fn state_of(state: &AppState, id: i64) -> Option<ItemState> {
        item(state, id).map(|item| item.state)
    }

    /// The sessions actor's save of item `id`.
    fn saves(id: i64) -> Vec<Command> {
        vec![Command::Sessions(SessionsCommand::SavePlateItem(
            PlateItemId(id),
        ))]
    }

    /// Agent 1 waiting for approval, so orb's row for it comes first and is
    /// selected, above item 1 in Your move; the plate has the keys.
    fn orb_row_selected() -> AppState {
        let mut state = state_with(vec![agent(1, AgentStatus::NeedsApproval)], 1);
        state.plate = Plate::from_saved(vec![test_item(1, ItemState::YourMove)]);
        open(&mut state);
        state
    }

    /// Item 1 done today, Done today opened and the item selected.
    fn done_item_selected() -> AppState {
        let mut state = plate_of(vec![test_item(1, ItemState::Done)]);
        pressed(&mut state, &[PlateKey::ToggleDone, PlateKey::Last]);
        state
    }

    #[rstest::rstest]
    fn x_moves_an_item_to_done() {
        // Given item 1 in Your move, selected.
        let mut state = your_move(1);

        // When pressing `x`.
        pressed(&mut state, &[PlateKey::Done]);

        // Then it's done, closed now.
        assert_eq!(
            item(&state, 1).map(|item| (item.state, item.done_at)),
            Some((ItemState::Done, Some(at(NOW)))),
            "x should close the item now"
        );
    }

    #[rstest::rstest]
    fn x_returns_a_save_of_the_item() {
        // Given item 1 in Your move, selected.
        let mut state = your_move(1);

        // When pressing `x`.
        let commands = pressed(&mut state, &[PlateKey::Done]);

        // Then the sessions actor is asked to save it.
        assert_eq!(commands, saves(1), "x should save the item");
    }

    #[rstest::rstest]
    fn x_on_an_orb_row_does_nothing() {
        // Given orb's row for a waiting agent, selected.
        let mut state = orb_row_selected();

        // When pressing `x`.
        let commands = pressed(&mut state, &[PlateKey::Done]);

        // Then nothing is saved, the item stays open and the row stays.
        assert_eq!(
            (commands, state_of(&state, 1), selected(&state)),
            (
                vec![],
                Some(ItemState::YourMove),
                Some(RowKey::Agent(PaneId(1)))
            ),
            "x on orb's row should do nothing"
        );
    }

    #[rstest::rstest]
    fn x_on_a_done_item_does_nothing() {
        // Given item 1 done today, selected.
        let mut state = done_item_selected();
        let before = item(&state, 1).cloned();

        // When pressing `x`.
        let commands = pressed(&mut state, &[PlateKey::Done]);

        // Then nothing is saved and the item is as it was.
        assert_eq!(
            (commands, item(&state, 1).cloned()),
            (vec![], before),
            "x on a done item should do nothing"
        );
    }

    #[rstest::rstest]
    fn gx_never_closes_the_item() {
        // Given item 1 in Your move, selected.
        let mut state = your_move(1);

        // When pressing `g` then `x`.
        let commands = pressed(&mut state, &[PlateKey::G, PlateKey::Done]);

        // Then the item is still open and nothing is saved.
        assert_eq!(
            (commands, state_of(&state, 1)),
            (vec![], Some(ItemState::YourMove)),
            "gx opens a link; it never marks the item done"
        );
    }

    #[rstest::rstest]
    #[case::your_move(ItemState::YourMove)]
    #[case::waiting(ItemState::Waiting)]
    fn p_parks_an_item(#[case] open_state: ItemState) {
        // Given item 1, open and not parked, selected.
        let mut state = plate_of(vec![test_item(1, open_state)]);

        // When pressing `p`.
        pressed(&mut state, &[PlateKey::Park]);

        // Then the user parked it now.
        assert_eq!(
            item(&state, 1).map(|item| (item.state, item.changed_at)),
            Some((ItemState::Parked { by_user: true }, at(NOW))),
            "p should park the item"
        );
    }

    #[rstest::rstest]
    fn p_brings_a_parked_item_back() {
        // Given item 1 parked by the user, selected.
        let mut state = plate_of(vec![test_item(1, ItemState::Parked { by_user: true })]);

        // When pressing `p`.
        pressed(&mut state, &[PlateKey::Park]);

        // Then it's the user's move again.
        assert_eq!(
            state_of(&state, 1),
            Some(ItemState::YourMove),
            "p on a parked item should bring it back"
        );
    }

    #[rstest::rstest]
    fn p_returns_a_save_of_the_item() {
        // Given item 1 in Your move, selected.
        let mut state = your_move(1);

        // When pressing `p`.
        let commands = pressed(&mut state, &[PlateKey::Park]);

        // Then the sessions actor is asked to save it.
        assert_eq!(commands, saves(1), "p should save the item");
    }

    #[rstest::rstest]
    fn p_on_an_orb_row_does_nothing() {
        // Given orb's row for a waiting agent, selected.
        let mut state = orb_row_selected();

        // When pressing `p`.
        let commands = pressed(&mut state, &[PlateKey::Park]);

        // Then nothing is saved and the item stays in Your move.
        assert_eq!(
            (commands, state_of(&state, 1)),
            (vec![], Some(ItemState::YourMove)),
            "p on orb's row should do nothing"
        );
    }

    #[rstest::rstest]
    fn p_on_a_done_item_does_nothing() {
        // Given item 1 done today, selected.
        let mut state = done_item_selected();
        let before = item(&state, 1).cloned();

        // When pressing `p`.
        let commands = pressed(&mut state, &[PlateKey::Park]);

        // Then nothing is saved and the item is as it was.
        assert_eq!(
            (commands, item(&state, 1).cloned()),
            (vec![], before),
            "p on a done item should do nothing"
        );
    }

    #[rstest::rstest]
    fn u_restores_the_item_before_the_last_x() {
        // Given item 1 in Your move, closed with `x`.
        let mut state = your_move(1);
        pressed(&mut state, &[PlateKey::Done]);

        // When pressing `u`.
        pressed(&mut state, &[PlateKey::Undo]);

        // Then it's back in Your move, as old as before, with no close time.
        assert_eq!(
            item(&state, 1).map(|item| (item.state, item.changed_at, item.done_at)),
            Some((ItemState::YourMove, at(10), None)),
            "u should undo the x"
        );
    }

    #[rstest::rstest]
    fn u_restores_the_item_before_the_last_p() {
        // Given item 1 waiting, parked with `p`.
        let mut state = plate_of(vec![test_item(1, ItemState::Waiting)]);
        pressed(&mut state, &[PlateKey::Park]);

        // When pressing `u`.
        pressed(&mut state, &[PlateKey::Undo]);

        // Then it's waiting again, as old as before.
        assert_eq!(
            item(&state, 1).map(|item| (item.state, item.changed_at)),
            Some((ItemState::Waiting, at(10))),
            "u should undo the p"
        );
    }

    #[rstest::rstest]
    fn u_returns_a_save_of_the_item() {
        // Given item 1 in Your move, closed with `x`.
        let mut state = your_move(1);
        pressed(&mut state, &[PlateKey::Done]);

        // When pressing `u`.
        let commands = pressed(&mut state, &[PlateKey::Undo]);

        // Then the sessions actor is asked to save it.
        assert_eq!(commands, saves(1), "u should save the restored item");
    }

    #[rstest::rstest]
    fn u_twice_undoes_only_once() {
        // Given items 1 and 2 in Your move, both closed with `x`, 1 first.
        let mut state = your_move(2);
        pressed(&mut state, &[PlateKey::Done, PlateKey::Done]);

        // When pressing `u` twice.
        let commands = pressed(&mut state, &[PlateKey::Undo, PlateKey::Undo]);

        // Then only item 2 came back, saved once.
        assert_eq!(
            (commands, state_of(&state, 1), state_of(&state, 2)),
            (saves(2), Some(ItemState::Done), Some(ItemState::YourMove)),
            "u should undo only the last x, and only once"
        );
    }

    #[rstest::rstest]
    fn u_with_nothing_to_undo_does_nothing() {
        // Given item 1 in Your move, untouched.
        let mut state = your_move(1);

        // When pressing `u`.
        let commands = pressed(&mut state, &[PlateKey::Undo]);

        // Then nothing is saved and the item is still open.
        assert_eq!(
            (commands, state_of(&state, 1)),
            (vec![], Some(ItemState::YourMove)),
            "u with nothing to undo should do nothing"
        );
    }

    #[rstest::rstest]
    fn u_selects_the_restored_item() {
        // Given items 1 to 3, item 1 closed with `x`, then the cursor moved
        // down to item 3.
        let mut state = your_move(3);
        pressed(&mut state, &[PlateKey::Done, PlateKey::Last]);

        // When pressing `u`.
        pressed(&mut state, &[PlateKey::Undo]);

        // Then item 1 is selected.
        assert_eq!(
            selected(&state),
            Some(item_key(1)),
            "u should put the cursor on the item it brought back"
        );
    }

    #[rstest::rstest]
    fn x_moves_the_cursor_to_the_next_row() {
        // Given items 1 to 3, the cursor on item 2.
        let mut state = your_move(3);
        pressed(&mut state, &[PlateKey::Next]);

        // When pressing `x`.
        pressed(&mut state, &[PlateKey::Done]);

        // Then item 3 is selected, item 2 having left with Done today folded.
        assert_eq!(
            selected(&state),
            Some(item_key(3)),
            "x should move the cursor on to the next row"
        );
    }
}
