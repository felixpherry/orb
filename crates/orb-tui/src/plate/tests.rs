use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jiff::tz::TimeZone;
use orb_domain::AppState;
use orb_domain::feat::harness::{HarnessId, claude, pi};
use orb_domain::feat::plate::{ItemState, Plate, PlateItem, Verb};
use orb_domain::feat::sessions::state::{
    Agent, AgentStatus, PaneLaunch, Project, ProjectKind, Sessions,
};
use orb_domain::ids::{AgentId, PaneId, PlateItemId, ProjectId, SessionId};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Rect};
use ratatui::style::Modifier;

use super::{DONE_ICON, IDEA_ICON, PARKED_ICON, URGENT_ICON, render, render_float};
use crate::sidebar::{APPROVAL_ICON, BLUE, DARK3, DARK5, MAGENTA, RED, VISUAL, YELLOW};
use crate::test_support::sessions_for;

/// The clock while drawing: ten days past the epoch, at 01:00 UTC.
fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_hours(10 * 24 + 1)
}

/// `secs` seconds before [`now`].
fn ago(secs: u64) -> SystemTime {
    now() - Duration::from_secs(secs)
}

/// Agent `id` in session `id` and pane `id`, titled `Fix the bug`, `status`.
fn agent(id: i64, status: AgentStatus) -> Agent {
    Agent {
        last_session: None,
        harness: HarnessId::new("claude"),
        id: AgentId(id),
        title: Some("Fix the bug".to_owned()),
        cwd: "/Users/me/dev/orb".into(),
        transcript: None,
        status,
        turn_started_at: None,
        waiting_since: Some(ago(120)),
        pane: Some(PaneLaunch {
            pane: PaneId(id),
            session: SessionId(id),
        }),
        branch: Some("main".to_owned()),
        created_at: UNIX_EPOCH,
        last_activity_at: UNIX_EPOCH,
        unseen: false,
        model: None,
    }
}

/// Item `id` in `state`: a review of session 1 from claude, changed 3
/// minutes ago, done a minute ago when done.
fn item(id: i64, state: ItemState) -> PlateItem {
    PlateItem {
        id: PlateItemId(id),
        verb: Verb::Review,
        state,
        title: format!("item {id}"),
        what: None,
        links: Vec::new(),
        urgent: false,
        session: SessionId(1),
        session_title: "Old session".to_owned(),
        pane: PaneId(1),
        harness: Some(HarnessId::new("claude")),
        changed_at: ago(180),
        done_at: (state == ItemState::Done).then(|| ago(60)),
        created_at: ago(600),
    }
}

/// orb with project `orb` running `agents` and `items` on the plate, claude
/// and pi registered.
fn state(agents: Vec<Agent>, items: Vec<PlateItem>) -> AppState {
    let projects = vec![Project {
        id: ProjectId(1),
        title: "orb".to_owned(),
        root: "/Users/me/dev/orb".into(),
        created_at: UNIX_EPOCH,
        removed: false,
        repo: true,
        agents,
        kind: ProjectKind::Normal,
    }];
    AppState {
        sessions: Sessions {
            sessions: sessions_for(&projects),
            projects,
            ..Sessions::default()
        },
        harnesses: vec![claude::info(), pi::info()],
        plate: Plate::from_saved(items),
        home: "/Users/me".into(),
        ..AppState::default()
    }
}

/// orb with an idle agent in session 1 and `items` on the plate.
fn with_items(items: Vec<PlateItem>) -> AppState {
    state(vec![agent(1, AgentStatus::Idle)], items)
}

/// Draws `state`'s start screen on a `width`×`height` buffer.
fn draw(state: &AppState, width: u16, height: u16) -> Buffer {
    let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
    render(state, now(), &TimeZone::UTC, buf.area, &mut buf);
    buf
}

fn lines(buf: &Buffer) -> Vec<String> {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .filter_map(|x| buf.cell((x, y)).map(Cell::symbol))
                .collect()
        })
        .collect()
}

/// The first line holding `text`.
fn line_with(buf: &Buffer, text: &str) -> String {
    lines(buf)
        .into_iter()
        .find(|line| line.contains(text))
        .unwrap_or_default()
}

/// The row of the first line holding `text`.
fn row_of(buf: &Buffer, text: &str) -> Option<u16> {
    let y = lines(buf).iter().position(|line| line.contains(text))?;
    u16::try_from(y).ok()
}

/// Where `text` starts on the first line holding `marker`.
fn find_on(buf: &Buffer, marker: &str, text: &str) -> Option<Position> {
    let y = row_of(buf, marker)?;
    let mut line = String::new();
    let mut starts = Vec::new();
    for x in 0..buf.area.width {
        let symbol = buf.cell((x, y)).map_or("", Cell::symbol);
        if !symbol.is_empty() {
            starts.push((line.len(), x));
        }
        line.push_str(symbol);
    }
    let at = line.find(text)?;
    starts
        .iter()
        .find(|(start, _)| *start == at)
        .map(|&(_, x)| Position::new(x, y))
}

/// The cell where `text` starts on the first line holding `marker`.
fn cell_on<'a>(buf: &'a Buffer, marker: &str, text: &str) -> Option<&'a Cell> {
    let at = find_on(buf, marker, text)?;
    buf.cell(at)
}

#[rstest::rstest]
fn start_screen_draws_the_plate_title() {
    // Given an empty plate.
    let state = with_items(Vec::new());

    // When drawing the start screen 80×40.
    let buf = draw(&state, 80, 40);

    // Then the list box is titled Plate.
    assert!(
        line_with(&buf, "Plate").contains("\u{f0ae} Plate"),
        "screen was\n{}",
        lines(&buf).join("\n")
    );
}

#[rstest::rstest]
fn start_screen_draws_the_four_group_headers_in_order() {
    // Given an empty plate.
    let state = with_items(Vec::new());

    // When drawing the start screen 80×40.
    let buf = draw(&state, 80, 40);

    // Then the four groups' headers are drawn top to bottom.
    let rows: Vec<Option<u16>> = ["Your move", "Waiting on others", "Parked", "Done today"]
        .iter()
        .map(|name| row_of(&buf, name))
        .collect();
    assert_eq!(
        rows,
        [Some(3), Some(5), Some(7), Some(9)],
        "each header a blank line after the last"
    );
}

#[rstest::rstest]
fn folded_done_today_says_z_opens_and_hides_its_rows() {
    // Given an item done a minute ago, Done today folded.
    let state = with_items(vec![item(1, ItemState::Done)]);

    // When drawing the start screen 80×40.
    let buf = draw(&state, 80, 40);

    // Then its header says z opens it, and its row isn't drawn.
    let screen = lines(&buf).join("\n");
    assert!(
        line_with(&buf, "Done today").contains("Done today  1  z opens")
            && !screen.contains("item 1"),
        "screen was\n{screen}"
    );
}

#[rstest::rstest]
fn footer_counts_your_move_waiting_parked_and_working() {
    // Given an agent waiting for approval, one working, and an item in
    // each of Your move, Waiting and Parked.
    let state = state(
        vec![
            agent(1, AgentStatus::NeedsApproval),
            agent(2, AgentStatus::Working),
        ],
        vec![
            item(1, ItemState::YourMove),
            item(2, ItemState::Waiting),
            item(3, ItemState::Parked { by_user: true }),
        ],
    );

    // When drawing the start screen 80×40.
    let buf = draw(&state, 80, 40);

    // Then the list's bottom border counts orb's row in Your move.
    assert!(
        line_with(&buf, "your move ·").contains(" 2 your move · 1 waiting · 1 parked · 1 working "),
        "footer was '{}'",
        line_with(&buf, "your move ·")
    );
}

#[rstest::rstest]
fn empty_your_move_says_nothing_needs_you() {
    // Given an empty plate.
    let state = with_items(Vec::new());

    // When drawing the start screen 80×40.
    let buf = draw(&state, 80, 40);

    // Then Your move's header says nothing needs the user.
    assert!(
        line_with(&buf, "Your move")
            .contains("Your move  0  nothing needs you, a good time for a parked idea"),
        "header was '{}'",
        line_with(&buf, "Your move")
    );
}

#[rstest::rstest]
fn first_row_is_selected_on_the_start_screen() {
    // Given two Your move items.
    let state = with_items(vec![
        item(1, ItemState::YourMove),
        item(2, ItemState::YourMove),
    ]);

    // When drawing the start screen 80×40.
    let buf = draw(&state, 80, 40);

    // Then the first row's cells are filled with the selection colour.
    assert_eq!(
        cell_on(&buf, "item 1", "item 1").map(|cell| cell.bg),
        Some(VISUAL),
        "the oldest Your move row is selected"
    );
}

#[rstest::rstest]
fn your_move_row_draws_its_tag_bold_in_the_verbs_colour() {
    // Given a review in Your move.
    let state = with_items(vec![item(1, ItemState::YourMove)]);

    // When drawing the start screen 80×40.
    let buf = draw(&state, 80, 40);

    // Then its tag is bold blue.
    let tag = cell_on(&buf, "item 1", "REVIEW");
    assert_eq!(
        tag.map(|cell| (cell.fg, cell.modifier.contains(Modifier::BOLD))),
        Some((BLUE, true)),
        "a review's tag in Your move"
    );
}

#[rstest::rstest]
fn urgent_item_draws_the_urgent_glyph() {
    // Given an urgent item in Your move.
    let state = with_items(vec![PlateItem {
        urgent: true,
        ..item(1, ItemState::YourMove)
    }]);

    // When drawing the start screen 80×40.
    let buf = draw(&state, 80, 40);

    // Then its row starts with the red urgent glyph.
    assert_eq!(
        cell_on(&buf, "item 1", URGENT_ICON).map(|cell| cell.fg),
        Some(RED),
        "an urgent item's glyph"
    );
}

#[rstest::rstest]
fn orb_approval_row_draws_the_approval_glyph() {
    // Given an agent waiting for approval.
    let state = state(vec![agent(1, AgentStatus::NeedsApproval)], Vec::new());

    // When drawing the start screen 80×40.
    let buf = draw(&state, 80, 40);

    // Then orb's row has the yellow approval glyph.
    assert_eq!(
        cell_on(&buf, "Approve Fix the bug", APPROVAL_ICON).map(|cell| cell.fg),
        Some(YELLOW),
        "orb's approval row"
    );
}

#[rstest::rstest]
fn waiting_row_draws_no_tag() {
    // Given a review waiting on others.
    let state = with_items(vec![item(1, ItemState::Waiting)]);

    // When drawing the start screen 80×40.
    let buf = draw(&state, 80, 40);

    // Then its row has no tag.
    let row = line_with(&buf, "item 1");
    assert!(!row.contains("REVIEW"), "row was '{row}'");
}

#[rstest::rstest]
fn offered_idea_draws_the_idea_glyph() {
    // Given an idea an agent offered.
    let state = with_items(vec![PlateItem {
        verb: Verb::Idea,
        ..item(1, ItemState::Parked { by_user: false })
    }]);

    // When drawing the start screen 80×40.
    let buf = draw(&state, 80, 40);

    // Then its row has the magenta idea glyph.
    assert_eq!(
        cell_on(&buf, "item 1", IDEA_ICON).map(|cell| cell.fg),
        Some(MAGENTA),
        "an offered idea's glyph"
    );
}

#[rstest::rstest]
fn user_parked_item_draws_the_pause_glyph() {
    // Given an item the user parked.
    let state = with_items(vec![item(1, ItemState::Parked { by_user: true })]);

    // When drawing the start screen 80×40.
    let buf = draw(&state, 80, 40);

    // Then its row has the dim pause glyph.
    assert_eq!(
        cell_on(&buf, "item 1", PARKED_ICON).map(|cell| cell.fg),
        Some(DARK5),
        "a parked item's glyph"
    );
}

#[rstest::rstest]
fn done_row_draws_a_dim_check() {
    // Given an item done a minute ago, Done today open.
    let mut state = with_items(vec![item(1, ItemState::Done)]);
    state.plate.view_mut().done_open = true;

    // When drawing the start screen 80×40.
    let buf = draw(&state, 80, 40);

    // Then its row has the dim check.
    assert_eq!(
        cell_on(&buf, "item 1", DONE_ICON).map(|cell| cell.fg),
        Some(DARK3),
        "a done item's glyph"
    );
}

#[rstest::rstest]
fn wide_list_shows_the_session_title_and_age() {
    // Given an item changed 3 minutes ago in session 1.
    let state = with_items(vec![item(1, ItemState::YourMove)]);

    // When drawing the start screen 160×40.
    let buf = draw(&state, 160, 40);

    // Then its row ends with the session's title and the age.
    let row = line_with(&buf, "item 1");
    assert!(row.contains("Fix the bug  3m"), "row was '{row}'");
}

#[rstest::rstest]
fn narrow_list_leaves_out_the_session_title() {
    // Given an item in session 1.
    let state = with_items(vec![item(1, ItemState::YourMove)]);

    // When drawing the start screen 120×40, the list under 70 columns.
    let buf = draw(&state, 120, 40);

    // Then its row shows only the age on the right.
    let row = line_with(&buf, "item 1");
    let list: String = row.chars().take(60).collect();
    assert!(
        !list.contains("Fix the bug") && list.contains("3m"),
        "row was '{row}'"
    );
}

#[rstest::rstest]
fn deleted_sessions_item_shows_its_saved_title() {
    // Given an item whose session is gone.
    let state = with_items(vec![PlateItem {
        session: SessionId(9),
        ..item(1, ItemState::YourMove)
    }]);

    // When drawing the start screen 160×40.
    let buf = draw(&state, 160, 40);

    // Then its row shows the title the session had.
    let row = line_with(&buf, "item 1");
    assert!(row.contains("Old session"), "row was '{row}'");
}

#[rstest::rstest]
fn preview_shows_the_title_and_description() {
    // Given an item with a description.
    let state = with_items(vec![PlateItem {
        what: Some("The fix is on the branch".to_owned()),
        ..item(1, ItemState::YourMove)
    }]);

    // When drawing the start screen 80×40.
    let buf = draw(&state, 80, 40);

    // Then the preview shows the title, then a blank line and the description.
    let lines = lines(&buf);
    let top = row_of(&buf, "Fix the bug · orb").map(usize::from);
    let at = |below: usize| {
        top.and_then(|top| lines.get(top + below)).map(|line| {
            line.trim_matches(|c: char| c == '│' || c.is_whitespace())
                .to_owned()
        })
    };
    assert_eq!(
        [at(4), at(5), at(6)],
        [
            Some("item 1".to_owned()),
            Some(String::new()),
            Some("The fix is on the branch".to_owned()),
        ],
        "screen was\n{}",
        lines.join("\n")
    );
}

#[rstest::rstest]
fn preview_numbers_the_links_with_their_labels() {
    // Given an item with a Jira link and a pull request.
    let state = with_items(vec![PlateItem {
        links: vec![
            "https://acme.atlassian.net/browse/GT-501".to_owned(),
            "https://github.com/acme/orb/pull/12".to_owned(),
        ],
        ..item(1, ItemState::YourMove)
    }]);

    // When drawing the start screen 80×40.
    let buf = draw(&state, 80, 40);

    // Then each link has its number, label and address.
    let first = line_with(&buf, "GT-501");
    let second = line_with(&buf, "#12");
    assert!(
        first.contains("1  \u{e75c} GT-501          https://acme.atlassian.net/browse/GT-501")
            && second.contains("2  \u{f407} #12             https://github.com/acme/orb/pull/12"),
        "links were\n{first}\n{second}"
    );
}

#[rstest::rstest]
fn preview_without_links_says_no_links() {
    // Given an item with no links.
    let state = with_items(vec![item(1, ItemState::YourMove)]);

    // When drawing the start screen 80×40.
    let buf = draw(&state, 80, 40);

    // Then the preview says it has none.
    assert!(
        row_of(&buf, "no links").is_some(),
        "screen was\n{}",
        lines(&buf).join("\n")
    );
}

/// orb with an agent of session 1 waiting for an answer.
fn orb_row() -> AppState {
    state(vec![agent(1, AgentStatus::NeedsInput)], Vec::new())
}

#[rstest::rstest]
#[case::claude(
    with_items(vec![item(1, ItemState::YourMove)]),
    "added by claude ✳ in Fix the bug · orb · 3m ago"
)]
#[case::pi(
    with_items(vec![PlateItem { harness: Some(HarnessId::new("pi")), ..item(1, ItemState::YourMove) }]),
    "added by pi π in Fix the bug"
)]
#[case::shell(
    with_items(vec![PlateItem { harness: None, ..item(1, ItemState::YourMove) }]),
    "added from a shell in Fix the bug"
)]
#[case::offered_idea(
    with_items(vec![item(1, ItemState::Parked { by_user: false })]),
    "offered by claude ✳ in Fix the bug"
)]
#[case::parked_by_the_user(
    with_items(vec![item(1, ItemState::Parked { by_user: true })]),
    "you parked it · from claude ✳ in Fix the bug"
)]
#[case::orb_row(orb_row(), "orb saw claude ✳ waiting in Fix the bug · orb · 2m ago")]
fn preview_names_who_added_the_item(#[case] state: AppState, #[case] expected: &str) {
    // Given the plate in `state`.

    // When drawing the start screen 80×40.
    let buf = draw(&state, 80, 40);

    // Then the preview's source line says where the row came from.
    assert!(
        row_of(&buf, expected).is_some(),
        "screen was\n{}",
        lines(&buf).join("\n")
    );
}

#[rstest::rstest]
fn parked_items_chips_say_unpark() {
    // Given an item the user parked.
    let state = with_items(vec![item(1, ItemState::Parked { by_user: true })]);

    // When drawing the start screen 80×40.
    let buf = draw(&state, 80, 40);

    // Then the preview's keys offer to unpark it.
    let keys = line_with(&buf, "go there");
    assert!(
        keys.contains(" ⏎  go there    gx  link    x  done    p  unpark"),
        "keys were '{keys}'"
    );
}

#[rstest::rstest]
fn orb_rows_preview_shows_only_go_there() {
    // Given an agent waiting for an answer.
    let state = orb_row();

    // When drawing the start screen 80×40.
    let buf = draw(&state, 80, 40);

    // Then the preview's only key is ⏎.
    let keys = line_with(&buf, "go there");
    assert!(
        keys.contains(" ⏎  go there") && !keys.contains("link"),
        "keys were '{keys}'"
    );
}

#[rstest::rstest]
fn nothing_on_the_plate_shows_nothing_to_show() {
    // Given an empty plate.
    let state = with_items(Vec::new());

    // When drawing the start screen 80×40.
    let buf = draw(&state, 80, 40);

    // Then the preview says there's nothing to show.
    assert!(
        row_of(&buf, "Nothing to show").is_some(),
        "screen was\n{}",
        lines(&buf).join("\n")
    );
}

#[rstest::rstest]
fn narrow_area_puts_the_preview_under_the_list() {
    // Given an item in Your move.
    let state = with_items(vec![item(1, ItemState::YourMove)]);

    // When drawing the start screen 80×40.
    let buf = draw(&state, 80, 40);

    // Then the preview's title is below the list's bottom border.
    let footer = row_of(&buf, "your move ·");
    let preview = row_of(&buf, "Fix the bug · orb");
    assert!(
        footer
            .zip(preview)
            .is_some_and(|(footer, preview)| preview > footer),
        "footer on row {footer:?}, preview title on row {preview:?}"
    );
}

#[rstest::rstest]
fn rows_past_the_bottom_say_how_many_lines_are_below() {
    // Given twelve Your move items in a list nine lines tall.
    let state = with_items((1..=12).map(|id| item(id, ItemState::YourMove)).collect());

    // When drawing the start screen 120×14.
    let buf = draw(&state, 120, 14);

    // Then the list's last line counts the lines cut off.
    assert_eq!(
        row_of(&buf, "⌄ 11 more lines below"),
        Some(11),
        "screen was\n{}",
        lines(&buf).join("\n")
    );
}

#[rstest::rstest]
fn small_area_draws_nothing_outside_itself() {
    // Given an item, and a 4×3 area inside a 40×20 buffer.
    let state = with_items(vec![item(1, ItemState::YourMove)]);
    let area = Rect::new(10, 10, 4, 3);
    let mut buf = Buffer::empty(Rect::new(0, 0, 40, 20));

    // When drawing the start screen into the area.
    render(&state, now(), &TimeZone::UTC, area, &mut buf);

    // Then every cell outside the area is still blank.
    let drawn: Vec<Position> = buf
        .area
        .positions()
        .filter(|at| !area.contains(*at))
        .filter(|at| buf.cell(*at).is_some_and(|cell| cell.symbol() != " "))
        .collect();
    assert_eq!(drawn, [], "cells drawn outside the area");
}

/// Draws `state`'s plate floating over a session whose right-hand side is
/// all of a `width`×`height` buffer; returns the buffer and the float's rect.
fn draw_float(state: &AppState, width: u16, height: u16) -> (Buffer, Rect) {
    let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
    let (rect, _) = render_float(state, now(), &TimeZone::UTC, buf.area, &mut buf);
    (buf, rect)
}

/// The columns on `rect`'s top row that hold `symbol`.
fn top_row_columns(buf: &Buffer, rect: Rect, symbol: &str) -> Vec<u16> {
    rect.columns()
        .filter(|at| {
            buf.cell((at.x, rect.y))
                .is_some_and(|cell| cell.symbol() == symbol)
        })
        .map(|at| at.x)
        .collect()
}

#[rstest::rstest]
fn float_draws_both_boxes_inside_its_rect() {
    // Given an item in Your move.
    let state = with_items(vec![item(1, ItemState::YourMove)]);

    // When drawing the float over a 140×40 right side.
    let (buf, rect) = draw_float(&state, 140, 40);

    // Then the list and the preview start side by side on its top row, the
    // list at its left edge.
    let corners = top_row_columns(&buf, rect, "╭");
    assert!(
        corners.len() == 2 && corners.first() == Some(&rect.x),
        "box corners on the float's top row at {corners:?}, float {rect:?}, screen was\n{}",
        lines(&buf).join("\n")
    );
}

#[rstest::rstest]
fn narrow_float_puts_the_preview_under_the_list() {
    // Given an item in Your move.
    let state = with_items(vec![item(1, ItemState::YourMove)]);

    // When drawing the float over an 80×40 right side.
    let (buf, _) = draw_float(&state, 80, 40);

    // Then the preview's title is below the list's bottom border.
    let footer = row_of(&buf, "your move ·");
    let preview = row_of(&buf, "Fix the bug · orb");
    assert!(
        footer
            .zip(preview)
            .is_some_and(|(footer, preview)| preview > footer),
        "footer on row {footer:?}, preview title on row {preview:?}"
    );
}

#[rstest::rstest]
fn float_draws_nothing_outside_its_rect() {
    // Given an item, and a 20×10 right side inside a 40×20 buffer.
    let state = with_items(vec![item(1, ItemState::YourMove)]);
    let right = Rect::new(10, 5, 20, 10);
    let mut buf = Buffer::empty(Rect::new(0, 0, 40, 20));

    // When drawing the float over it.
    let (rect, _) = render_float(&state, now(), &TimeZone::UTC, right, &mut buf);

    // Then every cell outside the float is still blank.
    let drawn: Vec<Position> = buf
        .area
        .positions()
        .filter(|at| !rect.contains(*at))
        .filter(|at| buf.cell(*at).is_some_and(|cell| cell.symbol() != " "))
        .collect();
    assert_eq!(drawn, [], "cells drawn outside the float {rect:?}");
}
