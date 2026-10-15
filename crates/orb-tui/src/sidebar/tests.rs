use orb_domain::feat::harness::claude::info;
use orb_domain::feat::harness::{HarnessId, HarnessInfo};
use orb_domain::feat::layout::state::{Layouts, PaneEntry, SessionLayout};
use orb_domain::feat::picker::list::PickerItem;
use orb_domain::feat::picker::state::{SessionFilter, session_items};
use orb_domain::feat::zmx::zmx_service::ZmxSession;
use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use orb_domain::TextInput;
use orb_domain::feat::sessions::state::{
    Agent, AgentStatus, PaneLaunch, Project, ProjectKind, Search, SessionKind, Sessions,
    SidebarItem,
};
use orb_domain::feat::sidebar::state::SidebarLayout;
use orb_domain::ids::{AgentId, PaneId, ProjectId, SessionId};
use orb_domain::{Focus, Intent};
use ratatui::buffer::{Buffer, Cell};
use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier};

use super::{
    APPROVAL_ICON, ATTACHED_ICON, BG_DARK, BLUE, BLUE1, BLUE2, BRANCH, CHILD_GUIDE, COMMENT,
    COMPLETED_ICON, CYAN, DARK3, FAILED_ICON, FG, FOLD_CLOSED, FOLD_OPEN, FOLDER, FOLDER_OPEN,
    GONE_ICON, GREEN, GUIDE, GUTTER, IDLE_ICON, INPUT_ICON, LAST_CHILD_GUIDE, LAST_GUIDE, MAGENTA,
    ORANGE, PIN, PURPLE, RED, STOPPED_ICON, SidebarScroll, VISUAL, YELLOW, ago_label, badge_colour,
    monogram, render, working_label,
};
use crate::mouse::{Clicks, HitMap, MouseRoute};
use crate::test_support::{layouts_for, sessions_for};

/// The home directory the sidebar shortens folder paths against.
const HOME: &str = "/Users/me";

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

/// An agent titled "Agent <id>" whose turn started at 866 s.
fn agent(id: i64, status: AgentStatus) -> Agent {
    Agent {
        last_session: None,
        harness: HarnessId::new("claude"),
        id: AgentId(id),
        title: Some(format!("Agent {id}")),
        cwd: "/Users/me/dev/orb".into(),
        transcript: None,
        status,
        turn_started_at: Some(at(866)),
        pane: Some(PaneLaunch {
            pane: PaneId(id),
            session: SessionId(id),
        }),
        branch: None,
        created_at: SystemTime::UNIX_EPOCH,
        last_activity_at: SystemTime::UNIX_EPOCH,
        unseen: false,
        model: None,
    }
}

fn stopped(id: i64) -> Agent {
    agent(id, AgentStatus::Stopped)
}

/// `sessions` with each session `(id, secs)` settled at second `secs`.
fn settling(mut sessions: Sessions, settles: &[(i64, u64)]) -> Sessions {
    for &(id, secs) in settles {
        sessions
            .sessions
            .iter_mut()
            .filter(|session| session.id == SessionId(id))
            .for_each(|session| session.settled_at = Some(at(secs)));
    }
    sessions
}

fn project(id: i64, title: &str, agents: Vec<Agent>) -> Project {
    Project {
        id: ProjectId(id),
        title: title.to_owned(),
        root: format!("/Users/me/dev/{title}").into(),
        created_at: SystemTime::UNIX_EPOCH,
        removed: false,
        repo: true,
        agents,
        kind: ProjectKind::Normal,
    }
}

fn sessions(agents: Vec<Agent>) -> Sessions {
    let projects = vec![project(1, "orb", agents)];
    Sessions {
        sessions: sessions_for(&projects),
        projects,
        ..Sessions::default()
    }
}

/// orb's agent 1 and web's agent 2, filtered to orb.
fn filtered() -> Sessions {
    let projects = vec![
        project(1, "orb", vec![agent(1, AgentStatus::Idle)]),
        project(2, "web", vec![agent(2, AgentStatus::Idle)]),
    ];
    Sessions {
        sessions: sessions_for(&projects),
        projects,
        filter: Some(ProjectId(1)),
        ..Sessions::default()
    }
}

/// `sessions` with `id`'s agent under the cursor.
fn select(sessions: Sessions, id: i64) -> Sessions {
    Sessions {
        cursor: Some(SidebarItem::Session(SessionId(id))),
        ..sessions
    }
}

/// Draws a `width`x`height` sidebar at `now`; returns the buffer and what
/// `render` returned.
fn render_sized(
    sessions: &Sessions,
    now: SystemTime,
    width: u16,
    height: u16,
) -> (Buffer, Option<u16>, SidebarLayout) {
    let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
    let (selected_y, layout, _) = render(
        sessions,
        None,
        &HashSet::new(),
        &[info()],
        &layouts_for(&sessions.projects),
        Path::new(HOME),
        now,
        buf.area,
        &mut buf,
        &mut SidebarScroll::default(),
        &mut HitMap::default(),
    );
    (buf, selected_y, layout)
}

/// Draws a 32-column sidebar `height` lines tall at `now`. The list's
/// first node takes lines 3 to 5, and its right edge is column 30.
fn draw(sessions: &Sessions, now: SystemTime, height: u16) -> Buffer {
    render_sized(sessions, now, 32, height).0
}

/// Draws a 32-column sidebar `height` lines tall at `now`, with `attached`'s agents attached.
fn draw_attached(sessions: &Sessions, attached: &[i64], now: SystemTime, height: u16) -> Buffer {
    let mut buf = Buffer::empty(Rect::new(0, 0, 32, height));
    let attached = attached
        .iter()
        .copied()
        .map(SessionId)
        .collect::<HashSet<_>>();
    render(
        sessions,
        None,
        &attached,
        &[info()],
        &layouts_for(&sessions.projects),
        Path::new(HOME),
        now,
        buf.area,
        &mut buf,
        &mut SidebarScroll::default(),
        &mut HitMap::default(),
    );
    buf
}

/// The sidebar's lines, top to bottom.
fn lines(buf: &Buffer) -> Vec<String> {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .filter_map(|x| buf.cell((x, y)).map(Cell::symbol))
                .collect()
        })
        .collect()
}

fn line(buf: &Buffer, y: usize) -> String {
    lines(buf).swap_remove(y)
}

/// The glyph at `(x, y)` and its colour.
fn glyph(buf: &Buffer, x: u16, y: u16) -> Option<(String, Color)> {
    buf.cell((x, y))
        .map(|cell| (cell.symbol().to_owned(), cell.fg))
}

#[rstest::rstest]
#[case("orb", "OB")]
#[case("paneru", "PU")]
#[case("v2 app", "V2")]
#[case("itemku-frontend-next-v2", "IV")]
#[case("", "PR")]
fn monogram_takes_first_and_last_glyphs(#[case] name: &str, #[case] expected: &str) {
    // Given / When deriving the project's monogram.
    let badge = monogram(name);

    // Then it follows T3's rule.
    assert_eq!(badge, expected, "monogram of '{name}'");
}

#[rstest::rstest]
#[case("orb", 17)]
#[case("paneru", 15)]
fn badge_colour_matches_t3(#[case] name: &str, #[case] expected: usize) {
    // Given / When hashing the project's name.
    let index = badge_colour(name);

    // Then it picks T3's colour.
    assert_eq!(index, expected, "badge colour of '{name}'");
}

#[rstest::rstest]
#[case(45, "45s")]
#[case(134, "2m")]
#[case(3900, "1h 5m")]
fn working_label_is_t3s_duration(#[case] secs: u64, #[case] expected: &str) {
    // Given / When formatting how long a turn has run.
    let label = working_label(Duration::from_secs(secs));

    // Then it uses T3's format.
    assert_eq!(label, expected, "{secs} s");
}

#[rstest::rstest]
#[case(30, "now")]
#[case(300, "5m")]
#[case(10_800, "3h")]
#[case(172_800, "2d")]
fn ago_label_is_t3s_relative_time(#[case] secs: u64, #[case] expected: &str) {
    // Given / When formatting how long ago something happened.
    let label = ago_label(Duration::from_secs(secs));

    // Then it uses T3's format.
    assert_eq!(label, expected, "{secs} s ago");
}

#[rstest::rstest]
fn sidebar_background_is_tokyonights_bg_dark() {
    // Given one agent.
    let sessions = sessions(vec![agent(1, AgentStatus::Idle)]);

    // When rendering the sidebar.
    let buf = draw(&sessions, at(1000), 10);

    // Then the blank column on its right edge has the dark background.
    assert_eq!(
        buf.cell((31, 4)).map(|cell| cell.bg),
        Some(BG_DARK),
        "the right edge's background"
    );
}

#[rstest::rstest]
fn input_box_is_titled_sessions() {
    // Given an empty orb.
    let sessions = Sessions::default();

    // When rendering the sidebar.
    let top = line(&draw(&sessions, at(1000), 5), 0);

    // Then the box's top border carries the title and the search badge.
    assert!(top.starts_with("╭ Sessions  i  ─"), "line was '{top}'");
}

#[rstest::rstest]
fn input_box_border_is_orange() {
    // Given an empty orb.
    let sessions = Sessions::default();

    // When rendering the sidebar.
    let buf = draw(&sessions, at(1000), 5);

    // Then the box's corner is orange.
    assert_eq!(
        glyph(&buf, 0, 0),
        Some(("╭".to_owned(), ORANGE)),
        "the box's top-left corner"
    );
}

#[rstest::rstest]
#[case::shelf_open(Sessions { shelf_open: true, ..settling(sessions(vec![stopped(1)]), &[(1, 10)]) }, (DARK3, BG_DARK))]
#[case::searching(searching(settling(sessions(vec![stopped(1)]), &[(1, 10)]), ""), (BLUE, GUTTER))]
fn badge_lights_up_while_searching(#[case] sessions: Sessions, #[case] expected: (Color, Color)) {
    // Given the Settled shelf open without a search, or a search.
    // When rendering the sidebar.
    let buf = draw(&sessions, at(1000), 8);

    // Then the badge's `i` is lit only while searching.
    let badge = buf
        .cell((12, 0))
        .map(|cell| (cell.symbol().to_owned(), (cell.fg, cell.bg)));
    assert_eq!(
        badge,
        Some(("i".to_owned(), expected)),
        "the badge's colours"
    );
}

#[rstest::rstest]
fn prompt_is_a_cyan_chevron() {
    // Given an empty orb.
    let sessions = Sessions::default();

    // When rendering the sidebar.
    let buf = draw(&sessions, at(1000), 5);

    // Then the box's line starts with a cyan `>`.
    assert_eq!(
        glyph(&buf, 1, 1),
        Some((">".to_owned(), CYAN)),
        "the prompt"
    );
}

#[rstest::rstest]
fn count_reads_listed_out_of_every_agent() {
    // Given one active and two settled agents, with the shelf closed.
    let sessions = settling(
        sessions(vec![agent(1, AgentStatus::Idle), stopped(2), stopped(3)]),
        &[(2, 10), (3, 20)],
    );

    // When rendering the sidebar.
    let prompt = line(&draw(&sessions, at(1000), 10), 1);

    // Then the count is the one listed agent out of three.
    assert!(prompt.trim_end().ends_with("1/3│"), "line was '{prompt}'");
}

#[rstest::rstest]
fn filtered_prompt_shows_the_project() {
    // Given the sidebar filtered to orb.
    let sessions = filtered();

    // When rendering the sidebar.
    let prompt = line(&draw(&sessions, at(1000), 10), 1);

    // Then the prompt is followed by orb's folder and name.
    assert!(
        prompt.starts_with(&format!("│> {FOLDER} orb ")),
        "line was '{prompt}'"
    );
}

#[rstest::rstest]
fn filtered_count_is_out_of_every_projects_agents() {
    // Given orb's and web's agents, filtered to orb.
    let sessions = filtered();

    // When rendering the sidebar.
    let prompt = line(&draw(&sessions, at(1000), 10), 1);

    // Then the count is orb's agent out of both.
    assert!(prompt.trim_end().ends_with("1/2│"), "line was '{prompt}'");
}

/// `sessions` searching for `text`.
fn searching(sessions: Sessions, text: &str) -> Sessions {
    Sessions {
        search: Some(Search {
            input: TextInput::new(text),
            return_to: None,
        }),
        ..sessions
    }
}

#[rstest::rstest]
fn search_text_follows_the_filtered_project() {
    // Given the sidebar filtered to orb, searching for "thr".
    let sessions = searching(filtered(), "thr");

    // When rendering the sidebar.
    let prompt = line(&draw(&sessions, at(1000), 10), 1);

    // Then the search text follows orb's name.
    assert!(
        prompt.starts_with(&format!("│> {FOLDER} orb thr ")),
        "line was '{prompt}'"
    );
}

#[rstest::rstest]
fn search_text_follows_the_chevron() {
    // Given no filter, searching for "thr".
    let sessions = searching(sessions(vec![agent(1, AgentStatus::Idle)]), "thr");

    // When rendering the sidebar.
    let prompt = line(&draw(&sessions, at(1000), 10), 1);

    // Then the search text follows the `>`.
    assert!(prompt.starts_with("│> thr "), "line was '{prompt}'");
}

#[rstest::rstest]
fn search_cursor_is_after_the_typed_text() {
    // Given a filtered sidebar searching for "thr".
    let sessions = searching(filtered(), "thr");

    // When rendering the sidebar.
    let mut buf = Buffer::empty(Rect::new(0, 0, 32, 10));
    let (_, _, cursor) = render(
        &sessions,
        None,
        &HashSet::new(),
        &[info()],
        &layouts_for(&sessions.projects),
        Path::new(HOME),
        at(1000),
        buf.area,
        &mut buf,
        &mut SidebarScroll::default(),
        &mut HitMap::default(),
    );

    // Then the cursor is after `> `, the folder and space, `orb`, a space
    // and `thr`: 11 columns into the box.
    assert_eq!(cursor, Some(Position::new(12, 1)), "the search cursor");
}

#[rstest::rstest]
fn hit_map_maps_the_search_text_to_its_graphemes() {
    // Given a filtered sidebar searching for "thr".
    let sessions = searching(filtered(), "thr");

    // When rendering the sidebar.
    let mut buf = Buffer::empty(Rect::new(0, 0, 32, 10));
    let mut hits = HitMap::default();
    let (_, _, cursor) = render(
        &sessions,
        None,
        &HashSet::new(),
        &[info()],
        &layouts_for(&sessions.projects),
        Path::new(HOME),
        at(1000),
        buf.area,
        &mut buf,
        &mut SidebarScroll::default(),
        &mut hits,
    );

    // Then the column before the search cursor maps to "r".
    let at = cursor.map(|cursor| Position::new(cursor.x - 1, cursor.y));
    assert_eq!(
        at.and_then(|at| hits.text_at(at)),
        Some(2),
        "the column before the cursor should be the last typed grapheme"
    );
}

/// The search text wider than a 32-column sidebar's input box: 40 `x`s
/// then `end`.
fn long_search() -> String {
    format!("{}end", "x".repeat(40))
}

#[rstest::rstest]
fn long_search_text_shows_its_end_before_the_whole_count() {
    // Given one agent, searching for a text wider than the box.
    let sessions = searching(sessions(vec![agent(1, AgentStatus::Idle)]), &long_search());

    // When rendering the sidebar.
    let prompt = line(&draw(&sessions, at(1000), 10), 1);

    // Then the line shows the text's end, then the count whole.
    assert!(
        prompt.starts_with("│> x") && prompt.trim_end().ends_with("end  0/1│"),
        "line was '{prompt}'"
    );
}

#[rstest::rstest]
fn long_search_text_keeps_the_filtered_project_whole() {
    // Given the sidebar filtered to orb, searching for a text wider than
    // the box.
    let sessions = searching(filtered(), &long_search());

    // When rendering the sidebar.
    let prompt = line(&draw(&sessions, at(1000), 10), 1);

    // Then orb's folder and name start the line, and the text shows its end.
    assert!(
        prompt.starts_with(&format!("│> {FOLDER} orb x")) && prompt.contains("end "),
        "line was '{prompt}'"
    );
}

#[rstest::rstest]
fn long_search_cursor_is_after_the_texts_end() {
    // Given one agent, searching for a text wider than the box.
    let sessions = searching(sessions(vec![agent(1, AgentStatus::Idle)]), &long_search());

    // When rendering the sidebar.
    let mut buf = Buffer::empty(Rect::new(0, 0, 32, 10));
    let (_, _, cursor) = render(
        &sessions,
        None,
        &HashSet::new(),
        &[info()],
        &layouts_for(&sessions.projects),
        Path::new(HOME),
        at(1000),
        buf.area,
        &mut buf,
        &mut SidebarScroll::default(),
        &mut HitMap::default(),
    );

    // Then the cursor is right after the shown `end`, clear of the count.
    let before = cursor.and_then(|at| buf.cell((at.x - 1, at.y)).map(Cell::symbol));
    assert_eq!(
        (cursor, before),
        (Some(Position::new(25, 1)), Some("d")),
        "the search cursor"
    );
}

#[rstest::rstest]
fn search_count_reads_matches_out_of_every_agent() {
    // Given three agents, searching for "2".
    let sessions = searching(
        sessions(vec![
            agent(1, AgentStatus::Idle),
            agent(2, AgentStatus::Idle),
            agent(3, AgentStatus::Idle),
        ]),
        "2",
    );

    // When rendering the sidebar.
    let prompt = line(&draw(&sessions, at(1000), 10), 1);

    // Then the count is the one match out of three.
    assert!(prompt.trim_end().ends_with("1/3│"), "line was '{prompt}'");
}

#[rstest::rstest]
fn search_highlights_the_matched_title_characters() {
    // Given "Agent 7", searching for "7".
    let sessions = searching(sessions(vec![agent(7, AgentStatus::Idle)]), "7");

    // When rendering the sidebar.
    let buf = draw(&sessions, at(1000), 8);

    // Then the title's `7` is blue and bold.
    let seven = (0..buf.area.width)
        .filter_map(|x| buf.cell((x, 3)))
        .find(|cell| cell.symbol() == "7")
        .map(|cell| (cell.fg, cell.modifier.contains(Modifier::BOLD)));
    assert_eq!(seven, Some((BLUE1, true)), "the matched `7`");
}

#[rstest::rstest]
fn search_highlights_the_matched_characters_of_a_settled_agent() {
    // Given settled "Agent 7" with the shelf closed, searching for "7".
    let sessions = searching(settling(sessions(vec![stopped(7)]), &[(7, 10)]), "7");

    // When rendering the sidebar.
    let buf = draw(&sessions, at(1000), 8);

    // Then the settled row's `7` is blue and bold.
    let seven = (0..buf.area.height)
        .find(|&y| line(&buf, usize::from(y)).contains("Agent 7"))
        .and_then(|y| {
            (0..buf.area.width)
                .filter_map(|x| buf.cell((x, y)))
                .find(|cell| cell.symbol() == "7")
        })
        .map(|cell| (cell.fg, cell.modifier.contains(Modifier::BOLD)));
    assert_eq!(seven, Some((BLUE1, true)), "the settled row's matched `7`");
}

#[rstest::rstest]
fn node_first_line_is_the_status_icon_and_title() {
    // Given an idle agent.
    let sessions = sessions(vec![agent(1, AgentStatus::Idle)]);

    // When rendering the sidebar.
    let heading = line(&draw(&sessions, at(1000), 8), 3);

    // Then its first line is the icon, then the title.
    assert!(
        heading.starts_with(&format!(" {IDLE_ICON} Agent 1 ")),
        "line was '{heading}'"
    );
}

#[rstest::rstest]
#[case::approval(AgentStatus::NeedsApproval, false, APPROVAL_ICON, YELLOW)]
#[case::input(AgentStatus::NeedsInput, false, INPUT_ICON, MAGENTA)]
#[case::failed(AgentStatus::Failed, false, FAILED_ICON, RED)]
#[case::gone(AgentStatus::Gone, false, GONE_ICON, RED)]
#[case::completed(AgentStatus::Idle, true, COMPLETED_ICON, GREEN)]
#[case::stopped(AgentStatus::Stopped, false, STOPPED_ICON, COMMENT)]
#[case::idle(AgentStatus::Idle, false, IDLE_ICON, DARK3)]
#[case::unknown(AgentStatus::Unknown, false, IDLE_ICON, DARK3)]
fn status_icon_shows_the_agents_status(
    #[case] status: AgentStatus,
    #[case] unseen: bool,
    #[case] icon: &str,
    #[case] colour: Color,
) {
    // Given an agent in `status`, not attached.
    let sessions = sessions(vec![Agent {
        unseen,
        ..agent(1, status)
    }]);

    // When rendering the sidebar.
    let buf = draw_attached(&sessions, &[], at(1000), 8);

    // Then its icon is the status's, in the status's colour.
    assert_eq!(
        glyph(&buf, 1, 3),
        Some((icon.to_owned(), colour)),
        "the icon for {status:?}"
    );
}

#[rstest::rstest]
#[case::idle(AgentStatus::Idle)]
#[case::unknown(AgentStatus::Unknown)]
fn status_icon_of_an_attached_idle_agent_is_a_filled_circle(#[case] status: AgentStatus) {
    // Given an attached agent in `status`.
    let sessions = sessions(vec![agent(1, status)]);

    // When rendering the sidebar.
    let buf = draw_attached(&sessions, &[1], at(1000), 8);

    // Then its icon is the filled circle, in the foreground colour.
    assert_eq!(
        glyph(&buf, 1, 3),
        Some((ATTACHED_ICON.to_owned(), FG)),
        "the icon for an attached {status:?} agent"
    );
}

#[rstest::rstest]
fn attached_working_agent_keeps_the_spinner() {
    // Given an attached Working agent whose turn started 134.3 s before now.
    let sessions = sessions(vec![agent(1, AgentStatus::Working)]);

    // When rendering the sidebar.
    let buf = draw_attached(&sessions, &[1], at(1000) + Duration::from_millis(300), 8);

    // Then its icon is still the spinner's fourth frame, in blue.
    assert_eq!(
        glyph(&buf, 1, 3),
        Some(("⠸".to_owned(), BLUE)),
        "the spinner of an attached Working agent"
    );
}

#[rstest::rstest]
fn working_icon_is_the_spinner_frame_for_its_elapsed_tenths() {
    // Given a Working agent whose turn started 134.3 s before now.
    let sessions = sessions(vec![agent(1, AgentStatus::Working)]);

    // When rendering the sidebar.
    let buf = draw(&sessions, at(1000) + Duration::from_millis(300), 8);

    // Then its icon is the spinner's fourth frame (1343 tenths), in blue.
    assert_eq!(
        glyph(&buf, 1, 3),
        Some(("⠸".to_owned(), BLUE)),
        "the spinner 134.3 s in"
    );
}

#[rstest::rstest]
fn working_agent_shows_its_elapsed_time() {
    // Given a Working agent whose turn started 134 s before now.
    let sessions = sessions(vec![agent(1, AgentStatus::Working)]);

    // When rendering the sidebar.
    let heading = line(&draw(&sessions, at(1000), 8), 3);

    // Then its first line ends with how long it has been working.
    assert!(heading.trim_end().ends_with(" 2m"), "line was '{heading}'");
}

#[rstest::rstest]
fn idle_agent_shows_time_since_last_activity() {
    // Given an idle agent whose last turn ended 3 h before now.
    let sessions = sessions(vec![agent(1, AgentStatus::Idle)]);

    // When rendering the sidebar.
    let heading = line(&draw(&sessions, at(10_800), 8), 3);

    // Then its first line ends with the time since.
    assert!(heading.trim_end().ends_with(" 3h"), "line was '{heading}'");
}

#[rstest::rstest]
fn untitled_agent_row_is_a_new_agent() {
    // Given a session whose agent has no title yet.
    let sessions = sessions(vec![Agent {
        title: None,
        ..agent(1, AgentStatus::Idle)
    }]);

    // When rendering the sidebar.
    let row = line(&draw(&sessions, at(1000), 8), 6);

    // Then its agent row says "New agent".
    assert!(row.contains(" New agent"), "line was '{row}'");
}

#[rstest::rstest]
fn pinned_agent_shows_an_orange_pin() {
    // Given a pinned agent.
    let mut sessions = sessions(vec![agent(1, AgentStatus::Idle)]);
    if let Some(session) = sessions.sessions.get_mut(0) {
        session.pinned_at = Some(at(5));
    }

    // When rendering the sidebar.
    let buf = draw(&sessions, at(1000), 8);

    // Then its first line carries the pin, in orange.
    let pin = (0..32)
        .filter_map(|x| glyph(&buf, x, 3))
        .find(|(symbol, _)| symbol == PIN);
    assert_eq!(pin, Some((PIN.to_owned(), ORANGE)), "the pin");
}

#[rstest::rstest]
fn node_second_line_shows_the_project() {
    // Given an idle agent in orb.
    let sessions = sessions(vec![agent(1, AgentStatus::Idle)]);

    // When rendering the sidebar.
    let place = line(&draw(&sessions, at(1000), 8), 4);

    // Then its second line is a guide, orb's folder and name, and no
    // status word.
    assert_eq!(
        place.trim_end(),
        format!(" ├╴{FOLDER} orb"),
        "the node's second line"
    );
}

#[rstest::rstest]
fn project_folder_takes_the_badge_colour() {
    // Given an agent in orb, whose badge is rose-400.
    let sessions = sessions(vec![agent(1, AgentStatus::Idle)]);

    // When rendering the sidebar.
    let buf = draw(&sessions, at(1000), 8);

    // Then the folder on its second line is rose.
    assert_eq!(
        glyph(&buf, 3, 4),
        Some((FOLDER.to_owned(), Color::Rgb(0xfb, 0x71, 0x85))),
        "orb's folder"
    );
}

#[rstest::rstest]
#[case(AgentStatus::NeedsApproval, false, "approval")]
#[case(AgentStatus::NeedsInput, false, "input")]
#[case(AgentStatus::Working, false, "working")]
#[case(AgentStatus::Failed, false, "failed")]
#[case(AgentStatus::Gone, false, "gone")]
#[case(AgentStatus::Idle, true, "done")]
#[case(AgentStatus::Stopped, false, "stopped")]
fn node_second_line_ends_with_the_status_word(
    #[case] status: AgentStatus,
    #[case] unseen: bool,
    #[case] word: &str,
) {
    // Given an agent in `status`.
    let sessions = sessions(vec![Agent {
        unseen,
        ..agent(1, status)
    }]);

    // When rendering the sidebar.
    let place = line(&draw(&sessions, at(1000), 8), 4);

    // Then its second line ends with the status's word.
    assert!(
        place.trim_end().ends_with(&format!(" {word}")),
        "line was '{place}'"
    );
}

/// Draws a 32-column sidebar 8 lines tall at 1000 s with `harnesses`
/// registered.
fn draw_with(sessions: &Sessions, harnesses: &[HarnessInfo]) -> Buffer {
    let mut buf = Buffer::empty(Rect::new(0, 0, 32, 8));
    render(
        sessions,
        None,
        &HashSet::new(),
        harnesses,
        &layouts_for(&sessions.projects),
        Path::new(HOME),
        at(1000),
        buf.area,
        &mut buf,
        &mut SidebarScroll::default(),
        &mut HitMap::default(),
    );
    buf
}

/// A pi-like harness with no mark.
fn pi_like() -> HarnessInfo {
    HarnessInfo {
        id: HarnessId::new("pi"),
        label: "pi".to_owned(),
        icon: None,
    }
}

#[rstest::rstest]
fn untagged_harness_shows_no_tag() {
    // Given a Working Claude agent.
    let sessions = sessions(vec![agent(1, AgentStatus::Working)]);

    // When rendering the sidebar.
    let place = line(&draw_with(&sessions, &[info(), pi_like()]), 4);

    // Then its second line ends `working` with only spaces before it.
    assert!(
        place.trim_end().ends_with("   working"),
        "line was '{place}'"
    );
}

#[rstest::rstest]
fn node_third_line_shows_the_branch() {
    // Given an agent on `main`.
    let sessions = sessions(vec![Agent {
        branch: Some("main".to_owned()),
        ..agent(1, AgentStatus::Idle)
    }]);

    // When rendering the sidebar.
    let footer = line(&draw(&sessions, at(1000), 8), 5);

    // Then its third line closes the card's tree on the branch, its agent
    // row hanging below.
    assert!(
        footer.starts_with(&format!(" └╴{BRANCH} main ")),
        "line was '{footer}'"
    );
}

/// The line naming the Settled shelf.
fn shelf_line(buf: &Buffer) -> String {
    lines(buf)
        .into_iter()
        .find(|line| line.contains(" Settled"))
        .unwrap_or_default()
}

#[rstest::rstest]
#[case::closed(false, FOLDER)]
#[case::open(true, FOLDER_OPEN)]
fn shelf_header_is_a_folder_open_while_the_shelf_is(
    #[case] shelf_open: bool,
    #[case] folder: &str,
) {
    // Given a settled agent, with the shelf open or closed.
    let sessions = Sessions {
        shelf_open,
        ..settling(sessions(vec![stopped(1)]), &[(1, 10)])
    };

    // When rendering the sidebar.
    let header = shelf_line(&draw(&sessions, at(1000), 8));

    // Then the header is the folder and `Settled`.
    assert!(
        header.starts_with(&format!(" {folder} Settled ")),
        "line was '{header}'"
    );
}

#[rstest::rstest]
fn shelf_header_counts_settled_agents() {
    // Given one active and two settled agents, with the shelf closed.
    let sessions = settling(
        sessions(vec![agent(1, AgentStatus::Idle), stopped(2), stopped(3)]),
        &[(2, 10), (3, 20)],
    );

    // When rendering the sidebar.
    let header = shelf_line(&draw(&sessions, at(1000), 10));

    // Then the header counts both settled agents.
    assert!(header.trim_end().ends_with(" 2"), "line was '{header}'");
}

#[rstest::rstest]
fn short_list_keeps_the_shelf_header_on_the_bottom_line() {
    // Given one active and one settled agent on a 10-line sidebar.
    let sessions = settling(
        sessions(vec![agent(1, AgentStatus::Idle), stopped(2)]),
        &[(2, 10)],
    );

    // When rendering the sidebar.
    let bottom = line(&draw(&sessions, at(1000), 10), 9);

    // Then the shelf header is on the last line.
    assert!(bottom.contains(" Settled "), "line was '{bottom}'");
}

/// orb's two settled agents, 2 settled at 10 s and 3 at 20 s, with the
/// shelf open.
fn open_shelf() -> Sessions {
    Sessions {
        shelf_open: true,
        ..settling(sessions(vec![stopped(2), stopped(3)]), &[(2, 10), (3, 20)])
    }
}

/// The lines after the shelf header.
fn settled_lines(buf: &Buffer) -> Vec<String> {
    lines(buf)
        .into_iter()
        .skip_while(|line| !line.contains(" Settled "))
        .skip(1)
        .collect()
}

#[rstest::rstest]
fn open_shelf_lists_settled_agents_newest_first() {
    // Given two settled agents, with the shelf open.
    let sessions = open_shelf();

    // When rendering the sidebar.
    let settled = settled_lines(&draw(&sessions, at(1000), 6));

    // Then the header is followed by one line per settled agent, newest
    // settle first.
    assert!(
        matches!(
            settled.as_slice(),
            [first, second] if first.contains("Agent 3") && second.contains("Agent 2")
        ),
        "lines were {settled:#?}"
    );
}

#[rstest::rstest]
fn settled_rows_hang_off_the_shelf_on_tree_guides() {
    // Given two settled agents, with the shelf open.
    let sessions = open_shelf();

    // When rendering the sidebar.
    let settled = settled_lines(&draw(&sessions, at(1000), 6));

    // Then the first row's guide branches and the last one's ends.
    let guides: Vec<_> = settled
        .iter()
        .map(|line| {
            [GUIDE, LAST_GUIDE]
                .into_iter()
                .find(|guide| line.starts_with(guide))
        })
        .collect();
    assert_eq!(
        guides,
        [Some(GUIDE), Some(LAST_GUIDE)],
        "lines were {settled:#?}"
    );
}

#[rstest::rstest]
fn settled_row_ends_with_the_time_since_it_settled() {
    // Given an agent settled 5 minutes before now, with the shelf open.
    let sessions = Sessions {
        shelf_open: true,
        ..settling(sessions(vec![stopped(2)]), &[(2, 700)])
    };

    // When rendering the sidebar.
    let settled = settled_lines(&draw(&sessions, at(1000), 5));

    // Then its row ends with the time since.
    assert!(
        settled
            .first()
            .is_some_and(|row| row.trim_end().ends_with(" 5m")),
        "lines were {settled:#?}"
    );
}

#[rstest::rstest]
#[case::failed(AgentStatus::Failed, FAILED_ICON, RED)]
#[case::gone(AgentStatus::Gone, GONE_ICON, RED)]
#[case::stopped(AgentStatus::Stopped, COMPLETED_ICON, DARK3)]
#[case::idle(AgentStatus::Idle, COMPLETED_ICON, DARK3)]
fn settled_row_icon_marks_only_failures(
    #[case] status: AgentStatus,
    #[case] icon: &str,
    #[case] colour: Color,
) {
    // Given an agent settled in `status`, with the shelf open.
    let sessions = Sessions {
        shelf_open: true,
        ..settling(sessions(vec![agent(1, status)]), &[(1, 10)])
    };

    // When rendering a 5-line sidebar, its row on the last line.
    let buf = draw(&sessions, at(1000), 5);

    // Then its icon follows the guide.
    assert_eq!(
        glyph(&buf, 3, 4),
        Some((icon.to_owned(), colour)),
        "the settled icon for {status:?}"
    );
}

#[rstest::rstest]
fn selected_node_first_line_has_the_cursorline() {
    // Given a selected agent.
    let sessions = select(sessions(vec![agent(1, AgentStatus::Idle)]), 1);

    // When rendering the sidebar.
    let buf = draw(&sessions, at(1000), 8);

    // Then its first line has the selection background across the list.
    let backgrounds: Vec<_> = (0..31)
        .map(|x| buf.cell((x, 3)).map(|cell| cell.bg))
        .collect();
    assert_eq!(backgrounds, vec![Some(VISUAL); 31], "the node's first line");
}

#[rstest::rstest]
fn selected_node_other_lines_keep_the_sidebar_background() {
    // Given a selected agent.
    let sessions = select(sessions(vec![agent(1, AgentStatus::Idle)]), 1);

    // When rendering the sidebar.
    let buf = draw(&sessions, at(1000), 8);

    // Then its second and third lines keep the dark background.
    let backgrounds: Vec<_> = [4, 5]
        .into_iter()
        .map(|y| buf.cell((0, y)).map(|cell| cell.bg))
        .collect();
    assert_eq!(
        backgrounds,
        vec![Some(BG_DARK); 2],
        "the node's other lines"
    );
}

#[rstest::rstest]
fn render_returns_the_selected_nodes_first_line() {
    // Given a selected agent.
    let sessions = select(sessions(vec![agent(1, AgentStatus::Idle)]), 1);

    // When rendering a 10-line sidebar.
    let (_, selected_y, _) = render_sized(&sessions, at(1000), 32, 10);

    // Then it reports the line under the input box.
    assert_eq!(selected_y, Some(3), "the selected node's first line");
}

#[rstest::rstest]
fn render_reports_the_list_height_and_each_rows_height() {
    // Given two agents and a collapsed shelf.
    let sessions = settling(
        sessions(vec![
            agent(1, AgentStatus::Idle),
            agent(2, AgentStatus::Idle),
            stopped(3),
        ]),
        &[(3, 10)],
    );

    // When rendering a 10-line sidebar.
    let (_, _, layout) = render_sized(&sessions, at(1000), 32, 10);

    // Then it reports the 7 lines under the input box and each row's
    // height.
    assert_eq!(
        layout,
        SidebarLayout {
            rows: 7,
            heights: vec![3, 1, 3, 1, 1],
        },
        "the layout should be the list height and one height per row"
    );
}

#[rstest::rstest]
fn sidebar_scrolls_to_show_the_whole_selected_node() {
    // Given three agents on an 8-line sidebar, with the last one (agent
    // 1) selected.
    let sessions = select(
        sessions(vec![
            agent(1, AgentStatus::Idle),
            agent(2, AgentStatus::Idle),
            agent(3, AgentStatus::Idle),
        ]),
        1,
    );

    // When rendering the sidebar.
    let (_, selected_y, _) = render_sized(&sessions, at(1000), 32, 8);

    // Then its card's three lines fill the last three lines.
    assert_eq!(selected_y, Some(5), "the selected node's first line");
}

/// `active` idle agents, 1 to `active`, and the agents `settled_ids`
/// settled at 10 s, with `selected`'s agent selected.
fn overflowing(active: i64, settled_ids: &[i64], selected: i64) -> Sessions {
    let agents = (1..=active)
        .map(|id| agent(id, AgentStatus::Idle))
        .chain(settled_ids.iter().map(|&id| stopped(id)))
        .collect();
    let settles: Vec<(i64, u64)> = settled_ids.iter().map(|&id| (id, 10)).collect();
    select(settling(sessions(agents), &settles), selected)
}

#[rstest::rstest]
fn overflowing_list_holds_the_shelf_header_on_the_bottom_line() {
    // Given four agents and a settled one on a 10-line sidebar, with the
    // first node (agent 4) selected.
    let sessions = overflowing(4, &[5], 4);

    // When rendering the sidebar.
    let bottom = line(&draw(&sessions, at(1000), 10), 9);

    // Then the shelf header is on the last line.
    assert!(bottom.contains(" Settled "), "line was '{bottom}'");
}

#[rstest::rstest]
#[case::next_to_the_header(1)]
#[case::far_from_the_header(2)]
fn selected_node_stays_whole_above_the_held_shelf_header(#[case] selected: i64) {
    // Given five agents and a settled one on a 10-line sidebar, with a
    // node near the list's end selected.
    let sessions = overflowing(5, &[6], selected);

    // When rendering the sidebar.
    let (buf, selected_y, _) = render_sized(&sessions, at(1000), 32, 10);

    // Then the card fills lines 6 to 8, right above the header's line.
    assert_eq!(
        (selected_y, line(&buf, 9).contains(" Settled ")),
        (Some(6), true),
        "agent {selected}'s first line, and the header on the last"
    );
}

#[rstest::rstest]
fn shelf_header_in_view_scrolls_with_the_list() {
    // Given four agents and an open shelf of two on a 10-line sidebar,
    // with the last settled agent selected.
    let sessions = Sessions {
        shelf_open: true,
        ..overflowing(4, &[5, 6], 5)
    };

    // When rendering the sidebar.
    let buf = draw(&sessions, at(1000), 10);

    // Then the header is on its own line, the settled agents below it.
    let header = lines(&buf)
        .iter()
        .position(|line| line.contains(" Settled "));
    assert_eq!(header, Some(7), "the header's line");
}

#[rstest::rstest]
#[case(24, 2)]
#[case(2, 8)]
#[case(80, 3)]
#[case(0, 0)]
fn sidebar_draws_at_any_size(#[case] width: u16, #[case] height: u16) {
    // Given a pinned agent, an active one and an open shelf, filtered to
    // orb, with the settled agent selected.
    let sessions = {
        let mut sessions = settling(
            sessions(vec![
                agent(1, AgentStatus::Working),
                agent(2, AgentStatus::NeedsApproval),
                stopped(3),
            ]),
            &[(3, 10)],
        );
        if let Some(session) = sessions.sessions.get_mut(0) {
            session.pinned_at = Some(at(5));
        }
        Sessions {
            shelf_open: true,
            filter: Some(ProjectId(1)),
            ..select(sessions, 3)
        }
    };

    // When rendering it at `width`x`height`.
    let (_, _, layout) = render_sized(&sessions, at(1000), width, height);

    // Then the list takes what the input box leaves.
    assert_eq!(
        layout.rows,
        height.saturating_sub(3),
        "the list's height at {width}x{height}"
    );
}

/// orb holding session 1 with `agents` as its agents, oldest first.
fn one_session(agents: Vec<Agent>) -> Sessions {
    let agents: Vec<Agent> = agents
        .into_iter()
        .enumerate()
        .map(|(at_secs, agent)| Agent {
            created_at: at(u64::try_from(at_secs).unwrap_or_default()),
            pane: agent.pane.map(|launch| PaneLaunch {
                session: SessionId(1),
                ..launch
            }),
            ..agent
        })
        .collect();
    sessions(agents)
}

#[rstest::rstest]
fn session_card_lists_one_row_per_agent_pane() {
    // Given session 1 with agent agents 1 and 2.
    let sessions = one_session(vec![
        agent(1, AgentStatus::Idle),
        agent(2, AgentStatus::Idle),
    ]);

    // When drawing the sidebar.
    let lines = lines(&draw(&sessions, at(1000), 12));

    // Then the card's fourth and fifth lines are the agents.
    assert!(
        lines.get(6).is_some_and(|line| line.contains("Agent 1"))
            && lines.get(7).is_some_and(|line| line.contains("Agent 2")),
        "lines were {lines:#?}"
    );
}

#[rstest::rstest]
fn agent_row_ends_with_its_harness_mark() {
    // Given session 1 with a pi agent.
    let sessions = one_session(vec![Agent {
        harness: HarnessId::new("pi"),
        ..agent(1, AgentStatus::Idle)
    }]);

    // When drawing the sidebar with pi's mark, π.
    let pi = HarnessInfo {
        icon: Some("π".to_owned()),
        ..pi_like()
    };
    let buf = draw_with(&sessions, &[info(), pi]);

    // Then the agent row ends with π.
    assert!(
        line(&buf, 6).trim_end().ends_with('π'),
        "line was '{}'",
        line(&buf, 6)
    );
}

#[rstest::rstest]
fn session_card_shows_its_most_urgent_agents_status() {
    // Given session 1 with a working agent and one needing approval.
    let sessions = one_session(vec![
        agent(1, AgentStatus::Working),
        agent(2, AgentStatus::NeedsApproval),
    ]);

    // When drawing the sidebar.
    let buf = draw(&sessions, at(1000), 12);

    // Then the card shows the approval icon and word.
    assert!(
        glyph(&buf, 1, 3) == Some((APPROVAL_ICON.to_owned(), YELLOW))
            && line(&buf, 4).trim_end().ends_with("approval"),
        "lines were {:#?}",
        lines(&buf)
    );
}

#[rstest::rstest]
fn session_without_agents_shows_its_folder_as_title() {
    // Given session 1 whose agent has ended.
    let mut sessions = sessions(vec![agent(1, AgentStatus::Idle)]);
    if let Some(agent) = sessions
        .projects
        .first_mut()
        .and_then(|project| project.agents.first_mut())
    {
        agent.pane = None;
    }

    // When drawing the sidebar.
    let buf = draw(&sessions, at(1000), 12);

    // Then its card is titled by its directory.
    assert!(
        line(&buf, 3).contains("orb"),
        "line was '{}'",
        line(&buf, 3)
    );
}

#[rstest::rstest]
fn settled_session_is_one_line() {
    // Given settled session 1 on an open shelf.
    let sessions = Sessions {
        shelf_open: true,
        ..settling(sessions(vec![stopped(1)]), &[(1, 700)])
    };

    // When drawing the sidebar.
    let settled = settled_lines(&draw(&sessions, at(1000), 10));

    // Then its one line holds its title and the time since it settled.
    assert!(
        settled.len() == 1
            && settled
                .first()
                .is_some_and(|row| row.contains("Agent 1") && row.trim_end().ends_with("5m")),
        "lines were {settled:#?}"
    );
}

#[rstest::rstest]
#[case::research(ProjectKind::Research, BLUE2)]
#[case::learn(ProjectKind::Learn, PURPLE)]
fn own_folder_session_shows_its_kind_icon(#[case] kind: ProjectKind, #[case] colour: Color) {
    // Given a colony in a `kind` project.
    let mut sessions = sessions(vec![agent(1, AgentStatus::Idle)]);
    if let Some(session) = sessions.sessions.first_mut() {
        session.kind = SessionKind::Colony;
    }
    if let Some(project) = sessions.projects.first_mut() {
        project.kind = kind;
    }

    // When drawing the sidebar.
    let buf = draw(&sessions, at(1000), 12);

    // Then the second line's icon, after its guide, is the kind's in its
    // colour.
    let icon = glyph(&buf, 3, 4);
    assert_eq!(
        icon.map(|(_, fg)| fg),
        Some(colour),
        "the {kind:?} icon's colour on '{}'",
        line(&buf, 4)
    );
}

/// Session 1 holding `count` stopped agents, each in its own pane, its
/// card `folded` or not.
fn card_of(count: i64, folded: bool) -> Sessions {
    let agents = (1..=count)
        .map(|id| Agent {
            pane: Some(PaneLaunch {
                pane: PaneId(id),
                session: SessionId(1),
            }),
            ..stopped(id)
        })
        .collect();
    let mut sessions = sessions(agents);
    if folded {
        sessions.folded.insert(SessionId(1));
    }
    sessions
}

#[rstest::rstest]
fn open_card_shows_a_status_icon_per_agent_then_the_down_chevron() {
    // Given session 1's open card over two stopped agents.
    let sessions = card_of(2, false);

    // When drawing the sidebar.
    let buf = draw(&sessions, at(1000), 12);

    // Then its third line ends with two stop icons and the down chevron.
    assert!(
        line(&buf, 5)
            .trim_end()
            .ends_with(&format!("{STOPPED_ICON} {STOPPED_ICON} {FOLD_OPEN}")),
        "line was '{}'",
        line(&buf, 5)
    );
}

#[rstest::rstest]
fn folded_card_shows_a_status_icon_per_agent_then_the_right_chevron() {
    // Given session 1's card folded over two stopped agents.
    let sessions = card_of(2, true);

    // When drawing the sidebar.
    let buf = draw(&sessions, at(1000), 12);

    // Then its third line ends with two stop icons and the right chevron.
    assert!(
        line(&buf, 5)
            .trim_end()
            .ends_with(&format!("{STOPPED_ICON} {STOPPED_ICON} {FOLD_CLOSED}")),
        "line was '{}'",
        line(&buf, 5)
    );
}

#[rstest::rstest]
fn folded_card_counts_agents_past_eight() {
    // Given session 1's card folded over ten agents.
    let sessions = card_of(10, true);

    // When drawing the sidebar.
    let buf = draw(&sessions, at(1000), 12);

    // Then its third line counts the two past eight.
    assert!(
        line(&buf, 5).contains(&format!("{STOPPED_ICON} +2 {FOLD_CLOSED}")),
        "line was '{}'",
        line(&buf, 5)
    );
}

#[rstest::rstest]
fn card_third_line_closes_its_tree(#[values(false, true)] folded: bool) {
    // Given session 1's card over two agents, `folded` or not.
    let sessions = card_of(2, folded);

    // When drawing the sidebar.
    let buf = draw(&sessions, at(1000), 12);

    // Then its third line starts with the last guide.
    assert!(
        line(&buf, 5).starts_with(LAST_GUIDE),
        "line was '{}'",
        line(&buf, 5)
    );
}

#[rstest::rstest]
fn agent_rows_hang_one_level_under_the_card() {
    // Given session 1's open card over two agents.
    let sessions = card_of(2, false);

    // When drawing the sidebar.
    let buf = draw(&sessions, at(1000), 12);

    // Then its agent rows branch from under the card's last line, the last one ending.
    let guides = [line(&buf, 6), line(&buf, 7)];
    assert!(
        guides[0].starts_with(CHILD_GUIDE) && guides[1].starts_with(LAST_CHILD_GUIDE),
        "lines were {guides:#?}"
    );
}

#[rstest::rstest]
fn card_without_agents_shows_no_chevron() {
    // Given session 1 whose agent has ended.
    let mut sessions = card_of(1, false);
    for project in &mut sessions.projects {
        for agent in &mut project.agents {
            agent.pane = None;
        }
    }

    // When drawing the sidebar.
    let buf = draw(&sessions, at(1000), 12);

    // Then its third line has no chevron.
    let third = line(&buf, 5);
    assert!(
        !third.contains(FOLD_OPEN) && !third.contains(FOLD_CLOSED),
        "line was '{third}'"
    );
}

/// `sessions` with session 1 a colony in Learn, in `~/.orb/learn/<slug>`.
fn learning(mut sessions: Sessions, slug: &str) -> Sessions {
    if let Some(session) = sessions.sessions.first_mut() {
        session.kind = SessionKind::Colony;
        session.dir = format!("{HOME}/.orb/learn/{slug}").into();
    }
    if let Some(project) = sessions.projects.first_mut() {
        project.kind = ProjectKind::Learn;
    }
    sessions
}

#[rstest::rstest]
fn learn_card_shows_its_folder_as_a_tilde_path() {
    // Given a Learn session in ~/.orb/learn/rust.
    let sessions = learning(card_of(1, false), "rust");

    // When drawing the sidebar.
    let buf = draw(&sessions, at(1000), 12);

    // Then its third line shows the path from home.
    assert!(
        line(&buf, 5).contains("~/.orb/learn/rust"),
        "line was '{}'",
        line(&buf, 5)
    );
}

#[rstest::rstest]
fn narrow_learn_card_cuts_its_path_before_the_chevron() {
    // Given a Learn session with a long slug, folded over one agent.
    let sessions = learning(card_of(1, true), "a-long-learning-folder");

    // When drawing a 24-column sidebar.
    let (buf, ..) = render_sized(&sessions, at(1000), 24, 12);

    // Then its third line still ends with the right chevron.
    assert!(
        line(&buf, 5).trim_end().ends_with(FOLD_CLOSED),
        "line was '{}'",
        line(&buf, 5)
    );
}

/// Draws a 32-column sidebar `height` lines tall at 1000 s with `scroll`;
/// returns the selected row's first line and the hit map it filled.
fn render_with(
    sessions: &Sessions,
    scroll: &mut SidebarScroll,
    height: u16,
) -> (Option<u16>, HitMap) {
    let mut buf = Buffer::empty(Rect::new(0, 0, 32, height));
    let mut hits = HitMap::default();
    let (selected_y, _, _) = render(
        sessions,
        None,
        &HashSet::new(),
        &[info()],
        &layouts_for(&sessions.projects),
        Path::new(HOME),
        at(1000),
        buf.area,
        &mut buf,
        scroll,
        &mut hits,
    );
    (selected_y, hits)
}

/// Agents 1 to 3, agent 3's node on top, with `selected`'s selected.
fn three_agents(selected: i64) -> Sessions {
    select(
        sessions(vec![
            agent(1, AgentStatus::Idle),
            agent(2, AgentStatus::Idle),
            agent(3, AgentStatus::Idle),
        ]),
        selected,
    )
}

#[rstest::rstest]
fn hit_map_maps_a_scrolled_rows_last_line_to_it() {
    // Given three sessions on an 8-line sidebar scrolled to the last one
    // (session 1), so session 2's last line tops the list.
    let y = 3;
    let sessions = three_agents(1);

    // When rendering the sidebar.
    let (_, hits) = render_with(&sessions, &mut SidebarScroll::default(), 8);

    // Then a click on that line lands on session 2.
    assert_eq!(
        hits.row_at(Position::new(1, y)),
        Some(SidebarItem::Session(SessionId(2))),
        "the row at line {y}"
    );
}

#[rstest::rstest]
fn hit_map_maps_the_sticky_header_row_to_the_shelf() {
    // Given four agents and a settled one on a 10-line sidebar, with
    // the shelf header held on the bottom line.
    let sessions = overflowing(4, &[5], 4);

    // When rendering the sidebar.
    let (_, hits) = render_with(&sessions, &mut SidebarScroll::default(), 10);

    // Then a click on the bottom line lands on the shelf.
    assert_eq!(
        hits.row_at(Position::new(1, 9)),
        Some(SidebarItem::SettledShelf),
        "the row on the bottom line"
    );
}

#[rstest::rstest]
fn hit_map_maps_the_shelf_gap_to_nothing() {
    // Given agent 1 and a settled agent 2 on a 12-line sidebar.
    let sessions = overflowing(1, &[2], 1);

    // When rendering the sidebar.
    let (_, hits) = render_with(&sessions, &mut SidebarScroll::default(), 12);

    // Then a blank line between agent 1 and the shelf header, held at
    // the bottom, lands on no row.
    assert_eq!(
        hits.row_at(Position::new(1, 7)),
        None,
        "the row on the gap line"
    );
}

#[rstest::rstest]
fn hit_map_records_the_input_box() {
    // Given an empty orb.
    let sessions = Sessions::default();

    // When rendering the sidebar.
    let (_, hits) = render_with(&sessions, &mut SidebarScroll::default(), 8);

    // Then the prompt's line is on the input box.
    assert!(
        hits.on_sidebar_input(Position::new(1, 1)),
        "the prompt line should be on the input box"
    );
}

#[rstest::rstest]
fn free_scroll_leaves_the_selected_row_off_screen() {
    // Given three agents on an 8-line sidebar with the top one (agent
    // 3) selected, and the view wheeled 3 lines down.
    let sessions = three_agents(3);
    let mut scroll = SidebarScroll::default();
    scroll.scroll_free(3, sessions.cursor);

    // When rendering the sidebar.
    let (selected_y, _) = render_with(&sessions, &mut scroll, 8);

    // Then the selected row's first line is off screen.
    assert_eq!(selected_y, None, "the selected row's first line");
}

#[rstest::rstest]
fn free_scroll_stops_at_the_lists_end() {
    // Given three agents on an 8-line sidebar with agent 3 selected,
    // and the view wheeled 30 lines down.
    let sessions = three_agents(3);
    let mut scroll = SidebarScroll::default();
    scroll.scroll_free(30, sessions.cursor);

    // When rendering the sidebar.
    let (_, hits) = render_with(&sessions, &mut scroll, 8);

    // Then the last line shows the list's last row, session 1's agent
    // row.
    assert_eq!(
        hits.row_at(Position::new(1, 7)),
        Some(SidebarItem::Agent {
            session: SessionId(1),
            pane: PaneId(1),
        }),
        "the row on the bottom line"
    );
}

#[rstest::rstest]
fn free_scroll_snaps_back_once_the_selection_moved() {
    // Given the view wheeled down while agent 2 was selected, and
    // agent 3 selected since.
    let sessions = three_agents(3);
    let mut scroll = SidebarScroll::default();
    scroll.scroll_free(3, Some(SidebarItem::Session(SessionId(2))));

    // When rendering the sidebar.
    let (selected_y, _) = render_with(&sessions, &mut scroll, 8);

    // Then the view follows the selection to the top of the list.
    assert_eq!(selected_y, Some(3), "the selected row's first line");
}

#[rstest::rstest]
fn free_scroll_stays_while_the_keys_stay_in_the_sidebar() {
    // Given the keys in the sidebar and the view wheeled down on agent 3.
    let sessions = three_agents(3);
    let mut scroll = SidebarScroll::default();
    scroll.follow_keys(true);
    scroll.scroll_free(3, sessions.cursor);

    // When the next draw finds the keys still in the sidebar.
    scroll.follow_keys(true);
    let (selected_y, _) = render_with(&sessions, &mut scroll, 8);

    // Then the view stays where the wheel put it.
    assert_ne!(
        selected_y,
        Some(3),
        "the view shouldn't snap back to the selection"
    );
}

#[rstest::rstest]
fn keys_moving_into_the_sidebar_bring_the_view_back() {
    // Given the keys in a pane and the view wheeled down on agent 3.
    let sessions = three_agents(3);
    let mut scroll = SidebarScroll::default();
    scroll.follow_keys(false);
    scroll.scroll_free(3, sessions.cursor);

    // When the keys move into the sidebar.
    scroll.follow_keys(true);
    let (selected_y, _) = render_with(&sessions, &mut scroll, 8);

    // Then the view follows the selection to the top of the list.
    assert_eq!(selected_y, Some(3), "the selected row's first line");
}

/// Agent 1's agent row.
fn agent_one() -> SidebarItem {
    SidebarItem::Agent {
        session: SessionId(1),
        pane: PaneId(1),
    }
}

/// Draws a 32-column sidebar 10 lines tall with pane names from
/// `layouts`. Agent 1's card takes lines 3 to 5, its agent row line 6.
fn draw_with_layouts(sessions: &Sessions, layouts: &Layouts) -> Buffer {
    let mut buf = Buffer::empty(Rect::new(0, 0, 32, 10));
    render(
        sessions,
        None,
        &HashSet::new(),
        &[info()],
        layouts,
        Path::new(HOME),
        at(1000),
        buf.area,
        &mut buf,
        &mut SidebarScroll::default(),
        &mut HitMap::default(),
    );
    buf
}

#[rstest::rstest]
fn agent_row_reads_its_panes_name() {
    // Given agent 1 running in pane 1, named "api".
    let sessions = sessions(vec![agent(1, AgentStatus::Idle)]);
    let layouts = {
        let mut layouts = Layouts::default();
        layouts.insert(
            SessionId(1),
            SessionLayout::of(PaneEntry {
                id: PaneId(1),
                zmx: ZmxSession {
                    name: "orb-p1".into(),
                    dir: "/tmp/zmx".into(),
                },
                cwd: "/tmp".into(),
                name: Some("api".into()),
                resume: None,
            }),
        );
        layouts
    };

    // When rendering the sidebar.
    let agent = line(&draw_with_layouts(&sessions, &layouts), 6);

    // Then the agent row shows the pane's name.
    assert!(agent.contains("api"), "line was '{agent}'");
}

#[rstest::rstest]
fn agent_row_falls_back_to_its_agents_title() {
    // Given agent 1 running in an unnamed pane.
    let sessions = sessions(vec![agent(1, AgentStatus::Idle)]);

    // When rendering the sidebar.
    let agent = line(
        &draw_with_layouts(&sessions, &layouts_for(&sessions.projects)),
        6,
    );

    // Then the agent row shows the agent's title.
    assert!(agent.contains("Agent 1"), "line was '{agent}'");
}

#[rstest::rstest]
#[case::renamed(Some("server"), "server")]
#[case::unnamed(None, "fix login")]
fn agent_name_is_the_same_in_sidebar_picker_and_search(
    #[case] pane_name: Option<&str>,
    #[case] expected: &str,
) {
    // Given agent 1 titled "fix login" in pane 1, named per the case.
    let sessions = sessions(vec![Agent {
        title: Some("fix login".into()),
        ..agent(1, AgentStatus::Idle)
    }]);
    let layouts = {
        let mut layouts = layouts_for(&sessions.projects);
        layouts.rename_pane(PaneId(1), pane_name.map(str::to_owned));
        layouts
    };

    // When reading the agent's name off the sidebar row, the session
    // picker row, and searching the sidebar for the expected name.
    let sidebar_label = {
        let agent = line(&draw_with_layouts(&sessions, &layouts), 6);
        let words: Vec<&str> = agent.split_whitespace().collect();
        words
            .get(1..words.len().saturating_sub(1))
            .unwrap_or_default()
            .join(" ")
    };
    let picker_name = session_items(&sessions, &layouts, SessionFilter::All)
        .into_iter()
        .find_map(|item| match item {
            PickerItem::Session { label, .. } => {
                label.rsplit_once('/').map(|(_, name)| name.to_owned())
            }
            _ => None,
        })
        .unwrap_or_default();
    let search_lists_the_agent_row = Sessions {
        search: Some(Search {
            input: TextInput::new(expected),
            return_to: None,
        }),
        ..sessions.clone()
    }
    .sidebar(&layouts)
    .iter()
    .any(|row| row.item() == agent_one());

    // Then all three agree on the expected name.
    assert_eq!(
        (
            sidebar_label.as_str(),
            picker_name.as_str(),
            search_lists_the_agent_row
        ),
        (expected, expected, true),
        "sidebar label, picker label and search should share the agent's name"
    );
}

#[rstest::rstest]
fn selected_agent_row_has_the_visual_background() {
    // Given the cursor on session 1's agent row.
    let sessions = Sessions {
        cursor: Some(agent_one()),
        ..sessions(vec![agent(1, AgentStatus::Idle)])
    };

    // When rendering the sidebar.
    let buf = draw(&sessions, at(1000), 10);

    // Then the agent row's line has the selection background.
    assert_eq!(
        buf.cell((1, 6)).map(|cell| cell.bg),
        Some(VISUAL),
        "the agent row's line"
    );
}

#[rstest::rstest]
fn vanished_agent_cursor_highlights_its_card() {
    // Given the cursor on an agent row of session 1 whose pane 99 is gone.
    let sessions = Sessions {
        cursor: Some(SidebarItem::Agent {
            session: SessionId(1),
            pane: PaneId(99),
        }),
        ..sessions(vec![agent(1, AgentStatus::Idle)])
    };

    // When rendering the sidebar.
    let buf = draw(&sessions, at(1000), 10);

    // Then the card's first line has the selection background.
    assert_eq!(
        buf.cell((1, 3)).map(|cell| cell.bg),
        Some(VISUAL),
        "the card's first line"
    );
}

#[rstest::rstest]
fn agent_line_records_its_row_in_the_hit_map() {
    // Given agent 1 on a 10-line sidebar.
    let sessions = select(sessions(vec![agent(1, AgentStatus::Idle)]), 1);

    // When rendering the sidebar.
    let (_, hits) = render_with(&sessions, &mut SidebarScroll::default(), 10);

    // Then a click on the agent's line lands on its agent row.
    assert_eq!(
        hits.row_at(Position::new(1, 6)),
        Some(agent_one()),
        "the row at the agent's line"
    );
}

#[rstest::rstest]
fn click_on_an_agent_line_selects_its_row() {
    // Given agent 1 drawn on a 10-line sidebar that has the keys.
    let sessions = select(sessions(vec![agent(1, AgentStatus::Idle)]), 1);
    let (_, hits) = render_with(&sessions, &mut SidebarScroll::default(), 10);

    // When clicking the agent's line.
    let routed = crate::mouse::route(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 6,
            modifiers: KeyModifiers::NONE,
        },
        &hits,
        Focus::Sidebar,
        None,
        &mut Clicks::default(),
        Instant::now(),
    );

    // Then the agent row is selected.
    assert_eq!(
        routed,
        MouseRoute::Intents(vec![Intent::SelectRow(agent_one())]),
        "a click on an agent line should select its row"
    );
}

#[rstest::rstest]
fn input_box_counts_sessions_not_agent_rows() {
    // Given two sessions, each running one agent.
    let sessions = sessions(vec![
        agent(1, AgentStatus::Idle),
        agent(2, AgentStatus::Idle),
    ]);

    // When rendering the sidebar.
    let prompt = line(&draw(&sessions, at(1000), 12), 1);

    // Then the count is two sessions out of two.
    assert!(prompt.trim_end().ends_with("2/2│"), "line was '{prompt}'");
}

/// Draws a 32-column sidebar 10 lines tall with `active` the active row.
/// Agent 1's card takes lines 3 to 5, its agent row line 6.
fn draw_active(sessions: &Sessions, active: SidebarItem) -> Buffer {
    let mut buf = Buffer::empty(Rect::new(0, 0, 32, 10));
    render(
        sessions,
        Some(active),
        &HashSet::new(),
        &[info()],
        &layouts_for(&sessions.projects),
        Path::new(HOME),
        at(1000),
        buf.area,
        &mut buf,
        &mut SidebarScroll::default(),
        &mut HitMap::default(),
    );
    buf
}

/// Whether every cell from `xs` on line `y` is drawn in `fg`.
fn all_fg(buf: &Buffer, xs: std::ops::RangeInclusive<u16>, y: u16, fg: Color) -> bool {
    xs.into_iter()
        .all(|x| buf.cell((x, y)).is_some_and(|cell| cell.fg == fg))
}

/// Whether every cell from `xs` on line `y` is bold.
fn all_bold(buf: &Buffer, xs: std::ops::RangeInclusive<u16>, y: u16) -> bool {
    xs.into_iter().all(|x| {
        buf.cell((x, y))
            .is_some_and(|cell| cell.modifier.contains(Modifier::BOLD))
    })
}

#[rstest::rstest]
fn active_agent_row_has_no_bar() {
    // Given the hover on agent 1's card and its agent row active.
    let sessions = select(sessions(vec![agent(1, AgentStatus::Idle)]), 1);

    // When rendering the sidebar.
    let buf = draw_active(&sessions, agent_one());

    // Then the agent row's first cell is blank.
    assert_eq!(
        buf.cell((0, 6)).map(Cell::symbol),
        Some(" "),
        "active agent row's first cell"
    );
}

#[rstest::rstest]
fn active_card_has_no_bar() {
    // Given agent 1's card active.
    let sessions = select(sessions(vec![agent(1, AgentStatus::Idle)]), 1);

    // When rendering the sidebar.
    let buf = draw_active(&sessions, SidebarItem::Session(SessionId(1)));

    // Then the card's first cell is blank.
    assert_eq!(
        buf.cell((0, 3)).map(Cell::symbol),
        Some(" "),
        "active card's first cell"
    );
}

#[rstest::rstest]
fn active_agent_rows_title_is_blue() {
    // Given the hover on agent 1's card and its agent row active.
    let sessions = select(sessions(vec![agent(1, AgentStatus::Idle)]), 1);

    // When rendering the sidebar.
    let buf = draw_active(&sessions, agent_one());

    // Then the agent row's title is blue.
    assert!(
        all_fg(&buf, 8..=14, 6, BLUE),
        "line was '{}'",
        line(&buf, 6)
    );
}

#[rstest::rstest]
fn active_agent_rows_title_is_bold() {
    // Given the hover on agent 1's card and its agent row active.
    let sessions = select(sessions(vec![agent(1, AgentStatus::Idle)]), 1);

    // When rendering the sidebar.
    let buf = draw_active(&sessions, agent_one());

    // Then the agent row's title is bold.
    assert!(all_bold(&buf, 8..=14, 6), "line was '{}'", line(&buf, 6));
}

#[rstest::rstest]
fn active_cards_title_is_blue() {
    // Given agent 1's card active.
    let sessions = select(sessions(vec![agent(1, AgentStatus::Idle)]), 1);

    // When rendering the sidebar.
    let buf = draw_active(&sessions, SidebarItem::Session(SessionId(1)));

    // Then the card's title is blue.
    assert!(all_fg(&buf, 3..=9, 3, BLUE), "line was '{}'", line(&buf, 3));
}

#[rstest::rstest]
fn active_cards_title_is_bold() {
    // Given agent 1's card active.
    let sessions = select(sessions(vec![agent(1, AgentStatus::Idle)]), 1);

    // When rendering the sidebar.
    let buf = draw_active(&sessions, SidebarItem::Session(SessionId(1)));

    // Then the card's title is bold.
    assert!(all_bold(&buf, 3..=9, 3), "line was '{}'", line(&buf, 3));
}

#[rstest::rstest]
fn active_rows_status_icon_keeps_its_colour() {
    // Given agent 1's agent row, drawn once active and once with its card
    // active instead.
    let sessions = select(sessions(vec![agent(1, AgentStatus::Idle)]), 1);
    let resting = draw_active(&sessions, SidebarItem::Session(SessionId(1)));

    // When rendering the sidebar with the agent row active.
    let buf = draw_active(&sessions, agent_one());

    // Then its status icon keeps the colour and weight it has while resting.
    assert_eq!(
        buf.cell((6, 6)).map(|cell| (cell.fg, cell.modifier)),
        resting.cell((6, 6)).map(|cell| (cell.fg, cell.modifier)),
        "active row's status icon"
    );
}

#[rstest::rstest]
fn inactive_rows_title_is_fg() {
    // Given agent 1's card active, not its agent row.
    let sessions = select(sessions(vec![agent(1, AgentStatus::Idle)]), 1);

    // When rendering the sidebar.
    let buf = draw_active(&sessions, SidebarItem::Session(SessionId(1)));

    // Then the agent row's title is in the plain colour and not bold.
    assert!(
        all_fg(&buf, 8..=14, 6, FG) && !(8..=14).any(|x| all_bold(&buf, x..=x, 6)),
        "line was '{}'",
        line(&buf, 6)
    );
}

#[rstest::rstest]
fn search_match_in_the_active_title_stays_blue1() {
    // Given a search for "gent" and agent 1's agent row active.
    let sessions = searching(
        select(sessions(vec![agent(1, AgentStatus::Idle)]), 1),
        "gent",
    );

    // When rendering the sidebar.
    let buf = draw_active(&sessions, agent_one());

    // Then the matched characters of its title keep the search's blue.
    assert!(
        all_fg(&buf, 9..=12, 6, BLUE1),
        "line was '{}'",
        line(&buf, 6)
    );
}

#[rstest::rstest]
fn hovered_row_keeps_the_visual_background_beside_the_active_row() {
    // Given the hover on agent 1's card and its agent row active.
    let sessions = select(sessions(vec![agent(1, AgentStatus::Idle)]), 1);

    // When rendering the sidebar.
    let buf = draw_active(&sessions, agent_one());

    // Then the card's first line has the hover background.
    assert_eq!(
        buf.cell((1, 3)).map(|cell| cell.bg),
        Some(VISUAL),
        "hovered card's background"
    );
}
