use crate::feat::harness::HarnessId;
use std::collections::HashSet;
use std::time::{Duration, SystemTime};

use super::{
    PaneId, PaneLaunch, Project, ProjectId, ProjectKind, Search, Session, SessionId, SessionKind,
    Sessions, SidebarItem, SidebarRow, Thread, ThreadId, ThreadStatus, layouts_for, most_urgent,
    sessions_for,
};
use crate::TextInput;
use crate::feat::layout::state::Layouts;
use crate::feat::sidebar::state::SidebarLayout;

/// `sessions` with the sessions its projects' threads run in.
fn fill(sessions: Sessions) -> Sessions {
    Sessions {
        sessions: sessions_for(&sessions.projects),
        ..sessions
    }
}

#[rstest::rstest]
fn pane_zmx_name_is_orb_p_and_its_id() {
    // Given pane 7.
    let pane = PaneId(7);

    // When naming its zmx session.
    let name = pane.zmx_name();

    // Then it is orb-p7.
    assert_eq!(name, "orb-p7", "a pane's zmx session is orb-p<id>");
}

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

fn thread(id: i64) -> Thread {
    Thread {
        last_session: None,
        harness: HarnessId::new("claude"),
        id: ThreadId(id),
        title: None,
        cwd: "/tmp".into(),
        transcript: None,
        status: ThreadStatus::Idle,
        turn_started_at: None,
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

/// Session `id` of project 1 in `/work/<id>`, created, active and last
/// active at second 0.
fn session(id: i64) -> Session {
    Session {
        id: SessionId(id),
        project: ProjectId(1),
        kind: SessionKind::Plain,
        dir: format!("/work/s{id}").into(),
        name: None,
        branch: None,
        created_at: SystemTime::UNIX_EPOCH,
        pinned_at: None,
        settled_at: None,
        active_since: SystemTime::UNIX_EPOCH,
        last_activity_at: SystemTime::UNIX_EPOCH,
    }
}

/// Thread `id`, created at second `secs`, running in pane `id` of
/// session `session`.
fn agent(id: i64, session: i64, secs: u64) -> Thread {
    Thread {
        created_at: at(secs),
        pane: Some(PaneLaunch {
            pane: PaneId(id),
            session: SessionId(session),
        }),
        ..thread(id)
    }
}

/// Project 1 holding `threads`, with `shown` as its sessions.
fn holding(threads: Vec<Thread>, shown: Vec<Session>) -> Sessions {
    fill(Sessions {
        projects: vec![project(1, threads)],
        sessions: shown,
        ..Sessions::default()
    })
}

#[rstest::rstest]
fn turned_after_a_titled_thread() {
    // Given session 1 whose ended thread was titled by its first prompt.
    let sessions = holding(
        vec![Thread {
            title: Some("Fix the parser".into()),
            pane: None,
            last_session: Some(SessionId(1)),
            ..thread(1)
        }],
        vec![session(1)],
    );

    // When asking whether a turn has run in it.
    let turned = sessions.turned(SessionId(1));

    // Then one has.
    assert!(turned, "a titled thread means a turn ran");
}

#[rstest::rstest]
fn not_turned_with_only_untitled_idle_threads() {
    // Given session 1 whose only thread is idle and untitled.
    let sessions = holding(vec![agent(1, 1, 0)], vec![session(1)]);

    // When asking whether a turn has run in it.
    let turned = sessions.turned(SessionId(1));

    // Then none has.
    assert!(!turned, "an untitled idle thread has had no turn");
}

#[rstest::rstest]
fn thread_session_is_its_panes_session() {
    // Given thread 1 running in a pane of session 4.
    let thread = agent(1, 4, 0);

    // When asking for its session.
    let session = thread.session();

    // Then it is session 4.
    assert_eq!(
        session,
        Some(SessionId(4)),
        "a thread belongs to its pane's session"
    );
}

#[rstest::rstest]
fn agents_are_a_sessions_threads_oldest_first() {
    // Given threads 1 (second 5) and 2 (second 3) in session 1, and
    // thread 3 in session 2.
    let sessions = holding(
        vec![agent(1, 1, 5), agent(2, 1, 3), agent(3, 2, 0)],
        vec![session(1), session(2)],
    );

    // When listing session 1's agents.
    let agents: Vec<ThreadId> = sessions
        .agents(SessionId(1))
        .into_iter()
        .map(|thread| thread.id)
        .collect();

    // Then they are threads 2 then 1.
    assert_eq!(
        agents,
        [ThreadId(2), ThreadId(1)],
        "a session's agents are its panes' threads, oldest first"
    );
}

#[rstest::rstest]
#[case::no_layout(false)]
#[case::pane_closed(true)]
fn agent_row_needs_a_layout_holding_its_pane(#[case] held_then_closed: bool) {
    // Given thread 1 in pane 1 of session 1, and layouts that never held
    // the pane or held it until it closed.
    let sessions = holding(vec![agent(1, 1, 0)], vec![session(1)]);
    let layouts = if held_then_closed {
        let mut layouts = layouts_for(&sessions.projects);
        layouts.close_pane(PaneId(1));
        layouts
    } else {
        Layouts::default()
    };

    // When listing session 1's agent rows.
    let panes: Vec<PaneId> = sessions
        .agent_rows(SessionId(1), &layouts)
        .into_iter()
        .map(|(_, pane)| pane)
        .collect();

    // Then there are none.
    assert_eq!(
        panes,
        Vec::<PaneId>::new(),
        "an agent whose pane no layout holds has no row"
    );
}

#[rstest::rstest]
fn session_title_is_its_r_name() {
    // Given session 1 named "auth" with an agent titled "Fix login".
    let named = Session {
        name: Some("auth".into()),
        ..session(1)
    };
    let titled = Thread {
        title: Some("Fix login".into()),
        ..agent(1, 1, 0)
    };
    let sessions = holding(vec![titled], vec![named.clone()]);

    // When asking for its title.
    let title = sessions.title(&named);

    // Then it is the name.
    assert_eq!(title, "auth", "the r name beats every other title");
}

#[rstest::rstest]
fn session_title_falls_back_to_the_first_agents_title() {
    // Given unnamed session 1 with an untitled agent and, created later,
    // one titled "Fix login".
    let titled = Thread {
        title: Some("Fix login".into()),
        ..agent(2, 1, 5)
    };
    let sessions = holding(vec![agent(1, 1, 0), titled], vec![session(1)]);

    // When asking for its title.
    let title = sessions.title(&session(1));

    // Then it is that agent's title.
    assert_eq!(
        title, "Fix login",
        "an unnamed session takes its first titled agent's title"
    );
}

#[rstest::rstest]
fn session_title_falls_back_to_the_directory_name() {
    // Given unnamed session 1 in /work/s1 with an untitled agent.
    let sessions = holding(vec![agent(1, 1, 0)], vec![session(1)]);

    // When asking for its title.
    let title = sessions.title(&session(1));

    // Then it is the directory's name.
    assert_eq!(
        title, "s1",
        "a session without a name or titled agent shows its folder"
    );
}

/// Thread `id` with `status`, its latest turn `unseen` or not.
fn with_status(id: i64, status: ThreadStatus, unseen: bool) -> Thread {
    Thread {
        status,
        unseen,
        ..thread(id)
    }
}

#[rstest::rstest]
#[case(ThreadStatus::NeedsApproval, false, ThreadStatus::Working, false)]
#[case(ThreadStatus::NeedsInput, false, ThreadStatus::Failed, false)]
#[case(ThreadStatus::Gone, false, ThreadStatus::Working, false)]
#[case(ThreadStatus::Working, false, ThreadStatus::Idle, true)]
#[case(ThreadStatus::Idle, true, ThreadStatus::Idle, false)]
#[case(ThreadStatus::Idle, false, ThreadStatus::Stopped, false)]
fn most_urgent_follows_the_rank_order(
    #[case] urgent: ThreadStatus,
    #[case] urgent_unseen: bool,
    #[case] other: ThreadStatus,
    #[case] other_unseen: bool,
) {
    // Given a thread that needs the user more than another, listed second.
    let threads = [
        with_status(1, other, other_unseen),
        with_status(2, urgent, urgent_unseen),
    ];

    // When picking the most urgent.
    let picked = most_urgent(&threads).map(|thread| thread.id);

    // Then it is the more urgent one.
    assert_eq!(
        picked,
        Some(ThreadId(2)),
        "{urgent:?} should outrank {other:?}"
    );
}

/// Thread `id`, created at second `secs`, so its session is active
/// since then.
fn active(id: i64, secs: u64) -> Thread {
    Thread {
        created_at: at(secs),
        ..thread(id)
    }
}

/// `sessions` with each session `(id, secs)` pinned at second `secs`.
fn pinning(mut sessions: Sessions, pins: &[(i64, u64)]) -> Sessions {
    for &(id, secs) in pins {
        sessions
            .sessions
            .iter_mut()
            .filter(|session| session.id == SessionId(id))
            .for_each(|session| session.pinned_at = Some(at(secs)));
    }
    sessions
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

/// Thread `id`, last active at second `secs`.
fn last_active(id: i64, secs: u64) -> Thread {
    Thread {
        last_activity_at: at(secs),
        ..thread(id)
    }
}

/// The ids of `sessions`' projects, most recently active first.
fn by_recency(sessions: &Sessions) -> Vec<i64> {
    sessions
        .projects_by_recency()
        .iter()
        .map(|project| project.id.0)
        .collect()
}

fn project(id: i64, threads: Vec<Thread>) -> Project {
    Project {
        id: ProjectId(id),
        title: format!("project-{id}"),
        root: "/tmp".into(),
        created_at: SystemTime::UNIX_EPOCH,
        removed: false,
        repo: true,
        threads,
        kind: ProjectKind::Normal,
    }
}

/// One project holding `threads`, with the cursor on `cursor`.
fn sessions(threads: Vec<Thread>, cursor: Option<SidebarItem>) -> Sessions {
    let projects = vec![project(1, threads)];
    Sessions {
        sessions: sessions_for(&projects),
        projects,
        cursor,
        ..Sessions::default()
    }
}

/// The session and shelf rows, in display order; agent rows left out.
fn items(sessions: &Sessions) -> Vec<SidebarItem> {
    sessions
        .sidebar(&layouts_for(&sessions.projects))
        .iter()
        .map(SidebarRow::item)
        .filter(|item| !matches!(item, SidebarItem::Agent { .. }))
        .collect()
}

fn on(id: i64) -> SidebarItem {
    SidebarItem::Session(SessionId(id))
}

/// Seven cards with no agents, listed 7 down to 1, with the cursor on
/// session `cursor`.
fn seven_cards(cursor: i64) -> Sessions {
    Sessions {
        projects: vec![project(1, vec![])],
        sessions: (1..=7).map(session).collect(),
        cursor: Some(on(cursor)),
        ..Sessions::default()
    }
}

/// A sidebar `rows` lines tall showing seven 4-line cards.
fn cards_in(rows: u16) -> SidebarLayout {
    SidebarLayout {
        rows,
        heights: vec![4; 7],
    }
}

#[rstest::rstest]
fn projects_by_recency_puts_newer_thread_activity_first() {
    // Given project 1 last active at 10 s and project 2 at 20 s.
    let sessions = fill(Sessions {
        projects: vec![
            project(1, vec![last_active(1, 10)]),
            project(2, vec![last_active(2, 20)]),
        ],
        ..Sessions::default()
    });

    // When ordering the projects by recency.
    let ids = by_recency(&sessions);

    // Then the more recently active project comes first.
    assert_eq!(ids, vec![2, 1], "newer thread activity should come first");
}

#[rstest::rstest]
fn projects_by_recency_uses_created_at_without_threads() {
    // Given project 1 last active at 10 s, and project 2 with no threads,
    // added at 20 s.
    let sessions = fill(Sessions {
        projects: vec![
            project(1, vec![last_active(1, 10)]),
            Project {
                created_at: at(20),
                ..project(2, vec![])
            },
        ],
        ..Sessions::default()
    });

    // When ordering the projects by recency.
    let ids = by_recency(&sessions);

    // Then the project added later comes first.
    assert_eq!(
        ids,
        vec![2, 1],
        "a project without threads should rank by when it was added"
    );
}

#[rstest::rstest]
fn projects_by_recency_breaks_ties_by_title() {
    // Given projects zeta (1) and alpha (2), equally recent.
    let sessions = fill(Sessions {
        projects: vec![
            Project {
                title: "zeta".into(),
                ..project(1, vec![])
            },
            Project {
                title: "alpha".into(),
                ..project(2, vec![])
            },
        ],
        ..Sessions::default()
    });

    // When ordering the projects by recency.
    let ids = by_recency(&sessions);

    // Then alpha comes first.
    assert_eq!(
        ids,
        vec![2, 1],
        "equally recent projects should sort by title"
    );
}

#[rstest::rstest]
fn sidebar_orders_pinned_by_newest_pin() {
    // Given threads 1 and 2 pinned at 10 s and 20 s, and thread 3 active
    // since 30 s.
    let sessions = pinning(
        sessions(vec![thread(1), thread(2), active(3, 30)], None),
        &[(1, 10), (2, 20)],
    );

    // When listing the sidebar.
    let items = items(&sessions);

    // Then the newest pin comes first, and the pins come before the active
    // thread.
    assert_eq!(
        items,
        vec![on(2), on(1), on(3)],
        "pinned cards should lead, newest pin first"
    );
}

#[rstest::rstest]
fn sidebar_orders_active_by_newest_created_or_unsettled() {
    // Given old thread 1 un-settled at 50 s, and newer thread 2 created at
    // 20 s.
    let sessions = sessions(vec![active(1, 50), active(2, 20)], None);

    // When listing the sidebar.
    let items = items(&sessions);

    // Then the un-settled thread comes first.
    assert_eq!(
        items,
        vec![on(1), on(2)],
        "active cards should list the newest created or un-settled first"
    );
}

#[rstest::rstest]
fn sidebar_orders_settled_by_newest_settle() {
    // Given threads settled at 10 s, 30 s and 20 s, with the shelf open.
    let sessions = Sessions {
        shelf_open: true,
        ..settling(
            sessions(vec![thread(1), thread(2), thread(3)], None),
            &[(1, 10), (2, 30), (3, 20)],
        )
    };

    // When listing the sidebar.
    let items = items(&sessions);

    // Then the header leads the settled rows, newest settle first.
    assert_eq!(
        items,
        vec![SidebarItem::SettledShelf, on(2), on(3), on(1)],
        "settled rows should list the newest settle first"
    );
}

#[rstest::rstest]
fn collapsed_shelf_hides_settled_threads() {
    // Given two settled threads and the shelf collapsed.
    let sessions = settling(
        sessions(vec![thread(1), thread(2)], None),
        &[(1, 10), (2, 20)],
    );

    // When listing the sidebar.
    let items = items(&sessions);

    // Then only the header shows.
    assert_eq!(
        items,
        vec![SidebarItem::SettledShelf],
        "a collapsed shelf should show only its header"
    );
}

#[rstest::rstest]
fn collapsed_shelf_keeps_the_selected_settled_session() {
    // Given two settled threads, the shelf collapsed, and the cursor on
    // thread 1.
    let sessions = settling(
        sessions(vec![thread(1), thread(2)], Some(on(1))),
        &[(1, 10), (2, 20)],
    );

    // When listing the sidebar.
    let items = items(&sessions);

    // Then the header and the selected thread show.
    assert_eq!(
        items,
        vec![SidebarItem::SettledShelf, on(1)],
        "a collapsed shelf should keep the cursor's settled thread"
    );
}

#[rstest::rstest]
fn sidebar_has_no_header_without_settled_threads() {
    // Given two active threads.
    let sessions = sessions(vec![active(1, 10), active(2, 20)], None);

    // When listing the sidebar.
    let items = items(&sessions);

    // Then only their cards show.
    assert_eq!(
        items,
        vec![on(2), on(1)],
        "without settled threads there should be no shelf header"
    );
}

#[rstest::rstest]
fn select_next_from_the_last_live_row_selects_the_shelf() {
    // Given an active thread 1 with its agent row selected, and a
    // settled thread 2.
    let mut sessions = settling(
        sessions(
            vec![active(1, 10), thread(2)],
            Some(SidebarItem::Agent {
                session: SessionId(1),
                pane: PaneId(1),
            }),
        ),
        &[(2, 20)],
    );

    // When selecting the next row.
    sessions.select_next(&layouts_for(&sessions.projects));

    // Then the shelf header is selected.
    assert_eq!(
        sessions.cursor,
        Some(SidebarItem::SettledShelf),
        "next after the last card should be the shelf header"
    );
}

#[rstest::rstest]
fn select_next_on_a_collapsed_shelf_wraps_to_the_first_row() {
    // Given an active thread, a settled thread, and the cursor on the
    // collapsed shelf's header.
    let mut sessions = settling(
        sessions(
            vec![active(1, 10), thread(2)],
            Some(SidebarItem::SettledShelf),
        ),
        &[(2, 20)],
    );

    // When selecting the next row.
    sessions.select_next(&layouts_for(&sessions.projects));

    // Then the first row is selected.
    assert_eq!(
        sessions.cursor,
        Some(SidebarItem::Session(SessionId(1))),
        "the collapsed shelf's header is the last row, so next wraps"
    );
}

#[rstest::rstest]
fn select_next_lands_on_a_thread_across_projects() {
    // Given project 1 with threads active since 30 s and 10 s, project 2
    // with one active since 20 s, and the newest one's agent row
    // selected.
    let mut sessions = fill(Sessions {
        projects: vec![
            project(1, vec![active(3, 30), active(1, 10)]),
            project(2, vec![active(2, 20)]),
        ],
        cursor: Some(SidebarItem::Agent {
            session: SessionId(3),
            pane: PaneId(3),
        }),
        ..Sessions::default()
    });

    // When selecting the next row.
    sessions.select_next(&layouts_for(&sessions.projects));

    // Then project 2's thread is selected, not project 1's older one.
    assert_eq!(
        sessions.cursor,
        Some(on(2)),
        "the sidebar should be one list across projects"
    );
}

#[rstest::rstest]
fn select_first_selects_the_top_row() {
    // Given two cards, with the cursor on the last card.
    let mut sessions = fill(Sessions {
        projects: vec![project(2, vec![active(1, 10), active(2, 20)])],
        cursor: Some(on(1)),
        ..Sessions::default()
    });

    // When selecting the first row.
    sessions.select_first(&layouts_for(&sessions.projects));

    // Then the top card is selected.
    assert_eq!(
        sessions.cursor,
        Some(on(2)),
        "the first row should be selected"
    );
}

#[rstest::rstest]
fn select_last_on_a_collapsed_shelf_selects_its_header() {
    // Given an active thread, selected, and a collapsed shelf.
    let mut sessions = settling(
        sessions(vec![active(1, 10), thread(2)], Some(on(1))),
        &[(2, 20)],
    );

    // When selecting the last row.
    sessions.select_last(&layouts_for(&sessions.projects));

    // Then the shelf's header is selected.
    assert_eq!(
        sessions.cursor,
        Some(SidebarItem::SettledShelf),
        "a collapsed shelf's header is the last row"
    );
}

#[rstest::rstest]
#[case(10, 6)]
#[case(40, 2)]
fn half_page_down_moves_the_cards_that_fit_in_half_the_height(
    #[case] rows: u16,
    #[case] expected: i64,
) {
    // Given seven cards with the top one selected.
    let mut sessions = seven_cards(7);

    // When moving half a page down on a sidebar `rows` lines tall.
    sessions.half_page_down(&cards_in(rows), &layouts_for(&sessions.projects));

    // Then the cursor moved by as many cards as fit in half of it.
    assert_eq!(sessions.cursor, Some(on(expected)), "half of {rows} lines");
}

#[rstest::rstest]
#[case(10, 2)]
#[case(40, 6)]
fn half_page_up_moves_the_cards_that_fit_in_half_the_height(
    #[case] rows: u16,
    #[case] expected: i64,
) {
    // Given seven cards with the bottom one selected.
    let mut sessions = seven_cards(1);

    // When moving half a page up on a sidebar `rows` lines tall.
    sessions.half_page_up(&cards_in(rows), &layouts_for(&sessions.projects));

    // Then the cursor moved by as many cards as fit in half of it.
    assert_eq!(sessions.cursor, Some(on(expected)), "half of {rows} lines");
}

#[rstest::rstest]
fn half_page_down_on_the_last_row_stays() {
    // Given seven cards with the bottom one selected.
    let mut sessions = seven_cards(1);

    // When moving half a page down.
    sessions.half_page_down(&cards_in(40), &layouts_for(&sessions.projects));

    // Then the cursor stays.
    assert_eq!(
        sessions.cursor,
        Some(on(1)),
        "there is nothing below the last row"
    );
}

#[rstest::rstest]
fn half_page_up_on_the_first_row_stays() {
    // Given seven cards with the top one selected.
    let mut sessions = seven_cards(7);

    // When moving half a page up.
    sessions.half_page_up(&cards_in(40), &layouts_for(&sessions.projects));

    // Then the cursor stays.
    assert_eq!(
        sessions.cursor,
        Some(on(7)),
        "there is nothing above the first row"
    );
}

#[rstest::rstest]
fn half_page_down_moves_one_row_when_a_card_is_taller_than_half() {
    // Given seven cards with the top one selected, on a 4-line sidebar.
    let mut sessions = seven_cards(7);

    // When moving half a page down.
    sessions.half_page_down(&cards_in(4), &layouts_for(&sessions.projects));

    // Then the cursor still moves one row.
    assert_eq!(
        sessions.cursor,
        Some(on(6)),
        "a half page should move at least one row"
    );
}

#[rstest::rstest]
fn half_page_up_moves_one_row_when_a_card_is_taller_than_half() {
    // Given seven cards with the bottom one selected, on a 4-line sidebar.
    let mut sessions = seven_cards(1);

    // When moving half a page up.
    sessions.half_page_up(&cards_in(4), &layouts_for(&sessions.projects));

    // Then the cursor still moves one row.
    assert_eq!(
        sessions.cursor,
        Some(on(2)),
        "a half page should move at least one row"
    );
}

#[rstest::rstest]
fn half_page_down_counts_an_unknown_row_as_one_line() {
    // Given seven cards with the top one selected, and a 4-line layout
    // that knows no row heights.
    let mut sessions = seven_cards(7);
    let layout = SidebarLayout {
        rows: 4,
        heights: vec![],
    };

    // When moving half a page down.
    sessions.half_page_down(&layout, &layouts_for(&sessions.projects));

    // Then the cursor moves two rows, one line each.
    assert_eq!(
        sessions.cursor,
        Some(on(5)),
        "a row without a height should count as one line"
    );
}

#[rstest::rstest]
fn half_page_down_without_a_cursor_selects_the_first_row() {
    // Given seven cards and no cursor.
    let mut sessions = Sessions {
        cursor: None,
        ..seven_cards(7)
    };

    // When moving half a page down.
    sessions.half_page_down(&cards_in(40), &layouts_for(&sessions.projects));

    // Then the first row is selected.
    assert_eq!(
        sessions.cursor,
        Some(on(7)),
        "without a cursor the first row should be selected"
    );
}

#[rstest::rstest]
fn sidebar_skips_threads_being_deleted() {
    // Given two active threads, one being deleted.
    let sessions = Sessions {
        deleting: [SessionId(1)].into(),
        ..sessions(vec![active(1, 10), active(2, 20)], None)
    };

    // When listing the sidebar.
    let items = items(&sessions);

    // Then only the other thread shows.
    assert_eq!(
        items,
        vec![on(2)],
        "a thread being deleted should be hidden"
    );
}

#[rstest::rstest]
fn working_count_skips_threads_being_deleted() {
    // Given two working threads, one being deleted.
    let working = |id| Thread {
        status: ThreadStatus::Working,
        ..thread(id)
    };
    let sessions = Sessions {
        deleting: [SessionId(1)].into(),
        ..sessions(vec![working(1), working(2)], None)
    };

    // When counting the working threads.
    let count = sessions.working_count();

    // Then only the other one counts.
    assert_eq!(
        count, 1,
        "a thread being deleted shouldn't count as running"
    );
}

#[rstest::rstest]
fn status_count_skips_threads_being_deleted() {
    // Given two threads needing approval, one being deleted.
    let needs_approval = |id| Thread {
        status: ThreadStatus::NeedsApproval,
        ..thread(id)
    };
    let sessions = Sessions {
        deleting: [SessionId(1)].into(),
        ..sessions(vec![needs_approval(1), needs_approval(2)], None)
    };

    // When counting the threads needing approval.
    let count = sessions.status_count(ThreadStatus::NeedsApproval);

    // Then only the other one counts.
    assert_eq!(
        count, 1,
        "a thread being deleted shouldn't count toward its status"
    );
}

#[rstest::rstest]
fn spinning_while_a_session_starts() {
    // Given idle threads and a session being started.
    let sessions = Sessions {
        starting: true,
        ..sessions(vec![thread(1)], None)
    };

    // When asking whether anything spins.
    let spinning = sessions.spinning();

    // Then it spins.
    assert!(spinning, "a starting session should spin");
}

#[rstest::rstest]
fn spinning_while_a_thread_works() {
    // Given a working thread and no session starting.
    let sessions = sessions(
        vec![Thread {
            status: ThreadStatus::Working,
            ..thread(1)
        }],
        None,
    );

    // When asking whether anything spins.
    let spinning = sessions.spinning();

    // Then it spins.
    assert!(spinning, "a working thread should spin");
}

#[rstest::rstest]
fn not_spinning_when_idle() {
    // Given only idle threads and no session starting.
    let sessions = sessions(vec![thread(1), thread(2)], None);

    // When asking whether anything spins.
    let spinning = sessions.spinning();

    // Then nothing spins.
    assert!(!spinning, "idle threads with no start shouldn't spin");
}

#[rstest::rstest]
fn any_in_progress_skips_threads_being_deleted() {
    // Given a working thread being deleted and an idle one.
    let sessions = Sessions {
        deleting: [SessionId(1)].into(),
        ..sessions(
            vec![
                Thread {
                    status: ThreadStatus::Working,
                    ..thread(1)
                },
                thread(2),
            ],
            None,
        )
    };

    // When asking whether a turn is underway.
    let busy = sessions.any_in_progress();

    // Then none is.
    assert!(!busy, "a thread being deleted shouldn't keep the poll fast");
}

#[rstest::rstest]
fn projects_by_recency_skips_removed_projects() {
    // Given projects 1 and 2, with 2 removed.
    let sessions = fill(Sessions {
        projects: vec![
            project(1, vec![]),
            Project {
                removed: true,
                ..project(2, vec![])
            },
        ],
        ..Sessions::default()
    });

    // When ordering the projects by recency.
    let ids = by_recency(&sessions);

    // Then only project 1 is listed.
    assert_eq!(ids, vec![1], "a removed project shouldn't be listed");
}

#[rstest::rstest]
fn sidebar_lists_a_removed_projects_threads() {
    // Given project 1 removed, with an active and a settled thread, the shelf open.
    let sessions = settling(
        fill(Sessions {
            projects: vec![Project {
                removed: true,
                ..project(1, vec![active(1, 10), thread(2)])
            }],
            shelf_open: true,
            ..Sessions::default()
        }),
        &[(2, 20)],
    );

    // When listing the sidebar.
    let rows = items(&sessions);

    // Then both threads still show.
    assert_eq!(
        rows,
        vec![on(1), SidebarItem::SettledShelf, on(2)],
        "removing a project should keep its threads listed"
    );
}

/// Project 1 with card 1 and settled thread 2, and project 2 with card 3
/// and settled thread 4, the shelf open, filtered to `filter`.
fn two_projects(filter: Option<i64>) -> Sessions {
    let [first, second] = [(1, 1, 2), (2, 3, 4)]
        .map(|(id, card, settled_id)| project(id, vec![thread(card), thread(settled_id)]));
    settling(
        fill(Sessions {
            projects: vec![first, second],
            shelf_open: true,
            filter: filter.map(ProjectId),
            ..Sessions::default()
        }),
        &[(2, 10), (4, 10)],
    )
}

#[rstest::rstest]
fn filtered_sidebar_lists_only_that_projects_rows() {
    // Given two projects, filtered to project 2.
    let sessions = two_projects(Some(2));

    // When listing the sidebar.
    let rows = items(&sessions);

    // Then only project 2's card, shelf and settled thread show.
    assert_eq!(
        rows,
        vec![on(3), SidebarItem::SettledShelf, on(4)],
        "the filter should keep only project 2's rows"
    );
}

#[rstest::rstest]
fn unfiltered_sidebar_lists_every_project() {
    // Given two projects and no filter.
    let sessions = two_projects(None);

    // When listing the sidebar.
    let rows = items(&sessions);

    // Then both projects' rows show.
    assert_eq!(
        rows,
        vec![on(3), on(1), SidebarItem::SettledShelf, on(4), on(2),],
        "no filter should list every project"
    );
}

#[rstest::rstest]
fn filtered_shelf_counts_only_that_projects_settled_threads() {
    // Given two projects with a settled thread each, filtered to project 1.
    let sessions = two_projects(Some(1));

    // When listing the sidebar.
    let count = sessions
        .sidebar(&layouts_for(&sessions.projects))
        .iter()
        .find_map(|row| match row {
            SidebarRow::ShelfHeader { count, .. } => Some(*count),
            _ => None,
        });

    // Then the shelf counts one thread.
    assert_eq!(count, Some(1), "the shelf should count project 1's only");
}

#[rstest::rstest]
fn filter_to_moves_a_hidden_cursor_to_the_first_row() {
    // Given two projects and the cursor on project 1's card.
    let mut sessions = Sessions {
        cursor: Some(on(1)),
        ..two_projects(None)
    };

    // When filtering to project 2.
    sessions.filter_to(Some(ProjectId(2)), &layouts_for(&sessions.projects));

    // Then the cursor is on project 2's card, the first row.
    assert_eq!(
        sessions.cursor,
        Some(on(3)),
        "a cursor the filter hides should move to the first row"
    );
}

#[rstest::rstest]
fn filter_to_keeps_a_listed_cursor() {
    // Given two projects and the cursor on project 2's card.
    let mut sessions = Sessions {
        cursor: Some(on(3)),
        ..two_projects(None)
    };

    // When filtering to project 2.
    sessions.filter_to(Some(ProjectId(2)), &layouts_for(&sessions.projects));

    // Then the cursor stays on the card.
    assert_eq!(
        sessions.cursor,
        Some(on(3)),
        "a cursor the filter keeps should stay"
    );
}

#[rstest::rstest]
fn filter_to_an_empty_project_leaves_no_cursor() {
    // Given project 1 with a card, empty project 2, and the cursor on the
    // card.
    let mut sessions = fill(Sessions {
        projects: vec![project(1, vec![thread(1)]), project(2, vec![])],
        cursor: Some(on(1)),
        ..Sessions::default()
    });

    // When filtering to project 2.
    sessions.filter_to(Some(ProjectId(2)), &layouts_for(&sessions.projects));

    // Then nothing is selected.
    assert_eq!(sessions.cursor, None, "an empty sidebar has no cursor");
}

#[rstest::rstest]
fn row_neighbour_skips_a_shelf_the_filter_hides() {
    // Given project 1 with only card 1, project 2 with only settled
    // thread 2, filtered to project 1.
    let sessions = settling(
        fill(Sessions {
            projects: vec![project(1, vec![thread(1)]), project(2, vec![thread(2)])],
            filter: Some(ProjectId(1)),
            ..Sessions::default()
        }),
        &[(2, 10)],
    );

    // When finding where the cursor goes after deleting card 1.
    let next = sessions.row_neighbour(on(1), &layouts_for(&sessions.projects));

    // Then nothing is left to select.
    assert_eq!(
        next, None,
        "another project's settled thread shouldn't offer a hidden shelf"
    );
}

/// `thread`, titled `title`.
fn titled(thread: Thread, title: &str) -> Thread {
    Thread {
        title: Some(title.to_owned()),
        ..thread
    }
}

/// `sessions` searching for `text`, from the cursor it has.
fn searching(sessions: Sessions, text: &str) -> Sessions {
    Sessions {
        search: Some(Search {
            input: TextInput::new(text),
            return_to: sessions.cursor,
        }),
        ..sessions
    }
}

/// Active "fix login bug" (1), "add dark mode" (2) and "fix logout" (3),
/// listed 3, 2, 1, and "fix lint" (4) settled, with the shelf closed.
fn four_titles() -> Sessions {
    settling(
        sessions(
            vec![
                titled(active(1, 10), "fix login bug"),
                titled(active(2, 20), "add dark mode"),
                titled(active(3, 30), "fix logout"),
                titled(thread(4), "fix lint"),
            ],
            None,
        ),
        &[(4, 40)],
    )
}

#[rstest::rstest]
fn search_keeps_only_matching_titles_in_sidebar_order() {
    // Given three active threads, two titled "fix log…", and the shelf
    // empty of matches.
    let sessions = searching(
        sessions(
            vec![
                titled(active(1, 10), "fix login bug"),
                titled(active(2, 20), "add dark mode"),
                titled(active(3, 30), "fix logout"),
            ],
            None,
        ),
        "fix log",
    );

    // When listing the sidebar.
    let rows = items(&sessions);

    // Then only the two matches show, newest first as usual.
    assert_eq!(
        rows,
        vec![on(3), on(1)],
        "the search should keep only matches, in sidebar order"
    );
}

#[rstest::rstest]
fn search_lists_settled_matches_while_the_shelf_is_closed() {
    // Given a settled "fix lint" with the shelf closed.
    let sessions = searching(four_titles(), "lint");

    // When listing the sidebar.
    let rows = items(&sessions);

    // Then the shelf header and the settled match show.
    assert_eq!(
        rows,
        vec![SidebarItem::SettledShelf, on(4)],
        "a settled match should show with the shelf closed"
    );
}

#[rstest::rstest]
fn search_counts_only_matching_settled_threads_on_the_shelf() {
    // Given two settled threads, one matching.
    let sessions = searching(
        settling(
            sessions(
                vec![
                    titled(thread(1), "fix lint"),
                    titled(thread(2), "add dark mode"),
                ],
                None,
            ),
            &[(1, 10), (2, 20)],
        ),
        "lint",
    );

    // When listing the sidebar.
    let count = sessions
        .sidebar(&layouts_for(&sessions.projects))
        .iter()
        .find_map(|row| match row {
            SidebarRow::ShelfHeader { count, .. } => Some(*count),
            _ => None,
        });

    // Then the header counts only the match.
    assert_eq!(count, Some(1), "the header should count the matches");
}

#[rstest::rstest]
#[case::searching("lint", true)]
#[case::not_searching("", false)]
fn shelf_header_reads_as_open_while_a_search_lists_settled_threads(
    #[case] text: &str,
    #[case] expected: bool,
) {
    // Given a closed shelf holding "fix lint", searching for `text`.
    let sessions = searching(
        settling(
            sessions(vec![titled(thread(1), "fix lint")], None),
            &[(1, 10)],
        ),
        text,
    );

    // When listing the sidebar.
    let open = sessions
        .sidebar(&layouts_for(&sessions.projects))
        .iter()
        .find_map(|row| match row {
            SidebarRow::ShelfHeader { open, .. } => Some(*open),
            _ => None,
        });

    // Then the header is open only while the search lists the match.
    assert_eq!(open, Some(expected), "the header searching for '{text}'");
}

#[rstest::rstest]
fn search_without_a_settled_match_has_no_shelf_header() {
    // Given a settled "fix lint" and a search only an active thread
    // matches.
    let sessions = searching(four_titles(), "dark");

    // When listing the sidebar.
    let rows = items(&sessions);

    // Then only the active match shows.
    assert_eq!(rows, vec![on(2)], "no settled match, no header");
}

#[rstest::rstest]
fn search_stays_inside_the_project_filter() {
    // Given two projects each with an untitled card, filtered to project
    // 2.
    let sessions = searching(two_projects(Some(2)), "new");

    // When listing the sidebar.
    let rows = items(&sessions);

    // Then only project 2's matching rows show.
    assert_eq!(
        rows,
        vec![on(3), SidebarItem::SettledShelf, on(4)],
        "the search should stay inside the filter"
    );
}

#[rstest::rstest]
#[case("")]
#[case("  ")]
fn blank_search_filters_nothing(#[case] text: &str) {
    // Given a search with blank text.
    let sessions = searching(four_titles(), text);

    // When listing the sidebar.
    let rows = items(&sessions);

    // Then every row shows as without a search.
    assert_eq!(rows, items(&four_titles()), "blank text shouldn't filter");
}

#[rstest::rstest]
fn title_matches_returns_the_matched_byte_offsets() {
    // Given a search for "fl".
    let sessions = searching(Sessions::default(), "fl");

    // When matching "fix lint".
    let offsets = sessions.title_matches("fix lint");

    // Then the `f` and the `l` match.
    assert_eq!(offsets, Some(vec![0, 4]), "offsets of 'fl' in 'fix lint'");
}

#[rstest::rstest]
fn select_first_match_without_a_match_clears_the_cursor() {
    // Given the cursor on thread 1 and a search nothing matches.
    let mut sessions = searching(
        Sessions {
            cursor: Some(on(1)),
            ..four_titles()
        },
        "zzz",
    );

    // When selecting the first match.
    sessions.select_first_match(&layouts_for(&sessions.projects));

    // Then nothing is selected.
    assert_eq!(sessions.cursor, None, "no match, no cursor");
}

#[rstest::rstest]
fn select_first_match_with_blank_text_keeps_the_cursor() {
    // Given the cursor on thread 1 and a blank search.
    let mut sessions = searching(
        Sessions {
            cursor: Some(on(1)),
            ..four_titles()
        },
        "",
    );

    // When selecting the first match.
    sessions.select_first_match(&layouts_for(&sessions.projects));

    // Then the cursor stays.
    assert_eq!(sessions.cursor, Some(on(1)), "blank text shouldn't move it");
}

#[rstest::rstest]
fn select_next_match_skips_the_shelf_header() {
    // Given "fix" matching cards 3 and 1 and settled 4, with the cursor on
    // card 1's agent row.
    let mut sessions = searching(
        Sessions {
            cursor: Some(agent_row(1, 1)),
            ..four_titles()
        },
        "fix",
    );

    // When selecting the next match.
    sessions.select_next_match(&layouts_for(&sessions.projects));

    // Then it lands on the settled match, past the header.
    assert_eq!(sessions.cursor, Some(on(4)), "the header is skipped");
}

#[rstest::rstest]
fn select_prev_match_skips_the_shelf_header() {
    // Given "fix" matching cards 3 and 1 and settled 4, with the cursor on
    // settled 4.
    let mut sessions = searching(
        Sessions {
            cursor: Some(on(4)),
            ..four_titles()
        },
        "fix",
    );

    // When selecting the previous match.
    sessions.select_prev_match(&layouts_for(&sessions.projects));

    // Then it lands on card 1's agent row, past the header.
    assert_eq!(
        sessions.cursor,
        Some(agent_row(1, 1)),
        "the header is skipped"
    );
}

#[rstest::rstest]
fn select_next_match_wraps_from_the_last_match_to_the_first() {
    // Given "fix" matching cards 3 and 1 and settled 4, with the cursor on
    // settled 4, the last match.
    let mut sessions = searching(
        Sessions {
            cursor: Some(on(4)),
            ..four_titles()
        },
        "fix",
    );

    // When selecting the next match.
    sessions.select_next_match(&layouts_for(&sessions.projects));

    // Then it wraps to card 3, the first match.
    assert_eq!(
        sessions.cursor,
        Some(on(3)),
        "next wraps to the first match"
    );
}

#[rstest::rstest]
fn select_prev_match_wraps_from_the_first_match_to_the_last() {
    // Given "fix" matching cards 3 and 1 and settled 4, with the cursor on
    // card 3, the first match.
    let mut sessions = searching(
        Sessions {
            cursor: Some(on(3)),
            ..four_titles()
        },
        "fix",
    );

    // When selecting the previous match.
    sessions.select_prev_match(&layouts_for(&sessions.projects));

    // Then it wraps to settled 4, the last match.
    assert_eq!(sessions.cursor, Some(on(4)), "prev wraps to the last match");
}

#[rstest::rstest]
fn select_next_match_stays_on_a_single_match() {
    // Given "lint" matching only settled 4, which has no agent row, with
    // the cursor on it.
    let mut sessions = searching(
        Sessions {
            cursor: Some(on(4)),
            ..four_titles()
        },
        "lint",
    );

    // When selecting the next match.
    sessions.select_next_match(&layouts_for(&sessions.projects));

    // Then the cursor stays on settled 4.
    assert_eq!(sessions.cursor, Some(on(4)), "a lone match stays put");
}

#[rstest::rstest]
fn cancel_search_restores_the_cursor_from_before() {
    // Given a search begun on thread 2 that moved the cursor to thread 3.
    let mut sessions = Sessions {
        cursor: Some(on(3)),
        ..searching(
            Sessions {
                cursor: Some(on(2)),
                ..four_titles()
            },
            "logout",
        )
    };

    // When cancelling the search.
    sessions.cancel_search(&layouts_for(&sessions.projects));

    // Then the cursor is back on thread 2.
    assert_eq!(sessions.cursor, Some(on(2)), "Esc restores the cursor");
}

#[rstest::rstest]
fn cancel_search_falls_back_to_the_first_row_when_the_old_one_is_gone() {
    // Given a search begun on thread 9, which no longer exists.
    let mut sessions = searching(
        Sessions {
            cursor: Some(on(9)),
            ..four_titles()
        },
        "logout",
    );

    // When cancelling the search.
    sessions.cancel_search(&layouts_for(&sessions.projects));

    // Then the cursor is on the first row.
    assert_eq!(sessions.cursor, Some(on(3)), "the first row");
}

#[rstest::rstest]
fn sidebar_lists_one_card_per_session() {
    // Given threads 1 and 2 in session 1, and thread 3 in session 3.
    let sessions = sessions(vec![agent(1, 1, 0), agent(2, 1, 1), thread(3)], None);

    // When listing the sidebar.
    let cards: Vec<SessionId> = sessions
        .sidebar(&layouts_for(&sessions.projects))
        .iter()
        .filter_map(|row| match row {
            SidebarRow::Card { session, .. } => Some(session.id),
            _ => None,
        })
        .collect();

    // Then there are two cards.
    assert_eq!(
        cards,
        vec![SessionId(3), SessionId(1)],
        "a session is one card, whatever its agents"
    );
}

#[rstest::rstest]
fn session_without_agents_is_listed() {
    // Given session 4 with no agent pane.
    let sessions = Sessions {
        projects: vec![project(1, vec![])],
        sessions: vec![session(4)],
        ..Sessions::default()
    };

    // When listing the sidebar.
    let rows = items(&sessions);

    // Then its card is listed.
    assert_eq!(
        rows,
        vec![on(4)],
        "a session whose agents all ended keeps its card"
    );
}

#[rstest::rstest]
fn settled_session_is_listed_after_the_shelf_header() {
    // Given active session 2 and settled session 1, the shelf open.
    let sessions = Sessions {
        shelf_open: true,
        ..settling(sessions(vec![thread(2), thread(1)], None), &[(1, 10)])
    };

    // When listing the sidebar.
    let rows = items(&sessions);

    // Then the settled session follows the header.
    assert_eq!(
        rows,
        vec![on(2), SidebarItem::SettledShelf, on(1)],
        "a settled session sits on the shelf"
    );
}

#[rstest::rstest]
fn search_lists_a_session_whose_agent_title_matches() {
    // Given session 1 named "auth" with an agent titled "Fix login",
    // and session 2, searched for "login".
    let mut sessions = sessions(vec![titled(thread(1), "Fix login"), thread(2)], None);
    if let Some(first) = sessions.sessions.iter_mut().find(|s| s.id == SessionId(1)) {
        first.name = Some("auth".into());
    }
    let sessions = searching(sessions, "login");

    // When listing the sidebar.
    let rows = items(&sessions);

    // Then only session 1 is listed.
    assert_eq!(rows, vec![on(1)], "an agent's title finds its session");
}

#[rstest::rstest]
fn search_lists_a_session_whose_title_matches() {
    // Given session 1 named "auth" and session 2, searched for "auth".
    let mut sessions = sessions(vec![thread(1), thread(2)], None);
    if let Some(first) = sessions.sessions.iter_mut().find(|s| s.id == SessionId(1)) {
        first.name = Some("auth".into());
    }
    let sessions = searching(sessions, "auth");

    // When listing the sidebar.
    let rows = items(&sessions);

    // Then only session 1 is listed.
    assert_eq!(rows, vec![on(1)], "a session's title finds it");
}

#[rstest::rstest]
fn sidebar_search_finds_an_agent_row_by_its_pane_name() {
    // Given session 1 named "api", its agent titled "fix login" in a pane
    // named "server", searched for "serv".
    let mut sessions = sessions(vec![titled(thread(1), "fix login")], None);
    if let Some(first) = sessions.sessions.iter_mut().find(|s| s.id == SessionId(1)) {
        first.name = Some("api".into());
    }
    let sessions = searching(sessions, "serv");
    let layouts = {
        let mut layouts = layouts_for(&sessions.projects);
        layouts.rename_pane(PaneId(1), Some("server".into()));
        layouts
    };

    // When listing the sidebar.
    let listed = sessions
        .sidebar(&layouts)
        .iter()
        .any(|row| row.item() == on(1));

    // Then session 1's card is listed.
    assert!(listed, "an agent row's pane name finds its session");
}

#[rstest::rstest]
fn selected_thread_is_the_sessions_oldest_agent() {
    // Given session 1 holding threads 2 (second 5) and 3 (second 1),
    // selected.
    let sessions = sessions(vec![agent(2, 1, 5), agent(3, 1, 1)], Some(on(1)));

    // When reading the selected thread.
    let selected = sessions.selected_thread().map(|thread| thread.id);

    // Then it's thread 3.
    assert_eq!(selected, Some(ThreadId(3)), "the lead thread is the oldest");
}

#[rstest::rstest]
#[case(ProjectKind::Research)]
#[case(ProjectKind::Learn)]
#[case(ProjectKind::Incognito)]
fn projects_by_recency_skips_orbs_own_projects(#[case] kind: ProjectKind) {
    // Given project 1, and project 2 of `kind`.
    let sessions = fill(Sessions {
        projects: vec![
            project(1, vec![]),
            Project {
                kind,
                ..project(2, vec![])
            },
        ],
        ..Sessions::default()
    });

    // When ordering the projects by recency.
    let ids = by_recency(&sessions);

    // Then only project 1 is listed.
    assert_eq!(ids, vec![1], "orb's own projects shouldn't be listed");
}

#[rstest::rstest]
#[case(on(1))]
fn jumpable_skips_a_row_the_filter_hides(#[case] item: SidebarItem) {
    // Given two projects, filtered to project 2.
    let sessions = two_projects(Some(2));

    // When asking whether project 1's row is jumpable.
    let jumpable = sessions.jumpable(item, &layouts_for(&sessions.projects));

    // Then it isn't.
    assert!(!jumpable, "{item:?} is hidden by the filter");
}

#[rstest::rstest]
fn jumpable_skips_a_thread_being_deleted() {
    // Given thread 1 being deleted.
    let sessions = Sessions {
        deleting: HashSet::from([SessionId(1)]),
        ..sessions(vec![thread(1)], None)
    };

    // When asking whether thread 1 is jumpable.
    let jumpable = sessions.jumpable(on(1), &layouts_for(&sessions.projects));

    // Then it isn't.
    assert!(!jumpable, "a thread being deleted is gone");
}

#[rstest::rstest]
fn jumpable_accepts_a_listed_thread() {
    // Given thread 1 listed.
    let sessions = sessions(vec![thread(1)], None);

    // When asking whether thread 1 is jumpable.
    let jumpable = sessions.jumpable(on(1), &layouts_for(&sessions.projects));

    // Then it is.
    assert!(jumpable, "a listed thread is a jump target");
}

/// The agent row of pane `pane` in session `session`.
fn agent_row(session: i64, pane: i64) -> SidebarItem {
    SidebarItem::Agent {
        session: SessionId(session),
        pane: PaneId(pane),
    }
}

/// Session 1 (newer) running thread 1 in pane 1, and session 2 running
/// thread 2 in pane 2, with the cursor on `cursor`.
fn two_sessions(cursor: SidebarItem) -> Sessions {
    sessions(vec![active(1, 20), active(2, 10)], Some(cursor))
}

#[rstest::rstest]
fn sidebar_lists_one_agent_row_per_agent_after_its_card() {
    // Given session 1 running agents 1 and 2.
    let sessions = holding(vec![agent(1, 1, 0), agent(2, 1, 5)], vec![session(1)]);

    // When listing the sidebar's rows.
    let items: Vec<SidebarItem> = sessions
        .sidebar(&layouts_for(&sessions.projects))
        .iter()
        .map(SidebarRow::item)
        .collect();

    // Then the card comes first, then one row per agent.
    assert_eq!(
        items,
        vec![on(1), agent_row(1, 1), agent_row(1, 2)],
        "each agent should get its own row after its card"
    );
}

/// Session 1 running agents 1 ("fix login") and 2, its card folded.
fn folded_card() -> Sessions {
    let mut sessions = holding(
        vec![titled(agent(1, 1, 0), "fix login"), agent(2, 1, 5)],
        vec![session(1)],
    );
    sessions.folded.insert(SessionId(1));
    sessions
}

#[rstest::rstest]
fn sidebar_lists_no_agent_rows_under_a_folded_card() {
    // Given session 1's card folded over agents 1 and 2.
    let sessions = folded_card();

    // When listing the sidebar's rows.
    let items: Vec<SidebarItem> = sessions
        .sidebar(&layouts_for(&sessions.projects))
        .iter()
        .map(SidebarRow::item)
        .collect();

    // Then only the card is listed.
    assert_eq!(items, vec![on(1)], "a folded card hides its agent rows");
}

#[rstest::rstest]
fn sidebar_lists_a_folded_cards_agent_rows_while_searching() {
    // Given session 1's card folded, and a search for "fix" matching it.
    let sessions = searching(folded_card(), "fix");

    // When listing the sidebar's rows.
    let items: Vec<SidebarItem> = sessions
        .sidebar(&layouts_for(&sessions.projects))
        .iter()
        .map(SidebarRow::item)
        .collect();

    // Then the card lists its agent rows as if open.
    assert_eq!(
        items,
        vec![on(1), agent_row(1, 1), agent_row(1, 2)],
        "a search reads a folded card as open"
    );
}

#[rstest::rstest]
fn last_agent_row_of_a_card_is_marked_last() {
    // Given session 1 running agents 1 and 2.
    let sessions = holding(vec![agent(1, 1, 0), agent(2, 1, 5)], vec![session(1)]);

    // When reading which agent rows are marked last.
    let lasts: Vec<bool> = sessions
        .sidebar(&layouts_for(&sessions.projects))
        .iter()
        .filter_map(|row| match row {
            SidebarRow::Agent { last, .. } => Some(*last),
            _ => None,
        })
        .collect();

    // Then only the final one is.
    assert_eq!(
        lasts,
        vec![false, true],
        "only the card's final agent row should be last"
    );
}

#[rstest::rstest]
fn select_next_from_a_card_selects_its_first_agent_row() {
    // Given two sessions with the cursor on session 1's card.
    let mut sessions = two_sessions(on(1));

    // When selecting the next row.
    sessions.select_next(&layouts_for(&sessions.projects));

    // Then session 1's agent row is selected.
    assert_eq!(
        sessions.cursor,
        Some(agent_row(1, 1)),
        "the row after a card is its first agent"
    );
}

#[rstest::rstest]
fn selected_session_on_an_agent_row_is_its_session() {
    // Given the cursor on session 2's agent row.
    let sessions = two_sessions(agent_row(2, 2));

    // When asking for the selected session.
    let selected = sessions.selected_session().map(|session| session.id);

    // Then it is session 2.
    assert_eq!(
        selected,
        Some(SessionId(2)),
        "an agent row selects its session"
    );
}

#[rstest::rstest]
fn select_next_from_a_vanished_agent_row_moves_past_its_card() {
    // Given the cursor on an agent row of session 1 whose pane 99 is
    // gone.
    let mut sessions = two_sessions(agent_row(1, 99));

    // When selecting the next row.
    sessions.select_next(&layouts_for(&sessions.projects));

    // Then the row after session 1's card is selected.
    assert_eq!(
        sessions.cursor,
        Some(agent_row(1, 1)),
        "a vanished agent row should read as its card"
    );
}

#[rstest::rstest]
fn cursor_row_of_a_vanished_agent_is_its_card() {
    // Given the cursor on an agent row of session 1 whose pane 99 is
    // gone.
    let sessions = two_sessions(agent_row(1, 99));

    // When reading the cursor's row.
    let row = sessions.cursor_row(&layouts_for(&sessions.projects));

    // Then it is session 1's card.
    assert_eq!(row, Some(on(1)), "a vanished agent row reads as its card");
}

#[rstest::rstest]
fn row_neighbour_of_a_session_skips_its_agent_rows() {
    // Given two sessions, each running one agent.
    let sessions = two_sessions(on(1));

    // When finding where the cursor goes after deleting session 1.
    let next = sessions.row_neighbour(on(1), &layouts_for(&sessions.projects));

    // Then it is session 2's card, past session 1's agent row.
    assert_eq!(
        next,
        Some(on(2)),
        "a deleted session's agent rows go with it"
    );
}

#[rstest::rstest]
fn select_prev_from_a_card_selects_the_agent_row_above() {
    // Given two sessions with the cursor on session 2's card.
    let mut sessions = two_sessions(on(2));

    // When selecting the previous row.
    sessions.select_prev(&layouts_for(&sessions.projects));

    // Then session 1's agent row, the row above the card, is selected.
    assert_eq!(
        sessions.cursor,
        Some(agent_row(1, 1)),
        "the row above a card is the agent row of the card before it"
    );
}

#[rstest::rstest]
fn select_last_selects_the_last_cards_last_agent_row() {
    // Given two sessions with the cursor on session 1's card.
    let mut sessions = two_sessions(on(1));

    // When selecting the last row.
    sessions.select_last(&layouts_for(&sessions.projects));

    // Then session 2's agent row is selected.
    assert_eq!(
        sessions.cursor,
        Some(agent_row(2, 2)),
        "the last row is the last card's last agent row"
    );
}

#[rstest::rstest]
fn half_page_down_counts_agent_rows_as_stops() {
    // Given two sessions on a 10-line sidebar (3-line cards, 1-line
    // agent rows) with the cursor on session 1's card.
    let mut sessions = two_sessions(on(1));
    let layout = SidebarLayout {
        rows: 10,
        heights: vec![3, 1, 3, 1],
    };

    // When moving half a page (5 lines) down.
    sessions.half_page_down(&layout, &layouts_for(&sessions.projects));

    // Then the cursor lands on session 2's agent row, three rows on.
    assert_eq!(
        sessions.cursor,
        Some(agent_row(2, 2)),
        "half a page should step through agent rows"
    );
}

#[rstest::rstest]
fn jumpable_accepts_an_agent_row_while_its_pane_runs_an_agent() {
    // Given session 1 running agent 1 in pane 1.
    let sessions = two_sessions(on(1));

    // When asking whether its agent row is jumpable.
    let jumpable = sessions.jumpable(agent_row(1, 1), &layouts_for(&sessions.projects));

    // Then it is.
    assert!(jumpable, "a running agent's row is a jump target");
}

#[rstest::rstest]
fn jumpable_skips_an_agent_row_whose_pane_no_longer_runs_an_agent() {
    // Given session 1 whose thread 1 left pane 1.
    let mut sessions = two_sessions(on(1));
    for thread in sessions
        .projects
        .iter_mut()
        .flat_map(|project| project.threads.iter_mut())
        .filter(|thread| thread.id == ThreadId(1))
    {
        thread.pane = None;
    }

    // When asking whether pane 1's agent row is jumpable.
    let jumpable = sessions.jumpable(agent_row(1, 1), &layouts_for(&sessions.projects));

    // Then it isn't.
    assert!(!jumpable, "a closed agent is skipped");
}

#[rstest::rstest]
fn jumpable_skips_an_agent_row_of_a_session_being_deleted() {
    // Given session 1, running agent 1, being deleted.
    let sessions = Sessions {
        deleting: HashSet::from([SessionId(1)]),
        ..two_sessions(on(2))
    };

    // When asking whether its agent row is jumpable.
    let jumpable = sessions.jumpable(agent_row(1, 1), &layouts_for(&sessions.projects));

    // Then it isn't.
    assert!(!jumpable, "an agent of a session being deleted is gone");
}

#[rstest::rstest]
fn select_next_match_stops_on_agent_rows() {
    // Given "fix" matching cards 3 and 1 and settled 4, with the cursor on
    // card 3.
    let mut sessions = searching(
        Sessions {
            cursor: Some(on(3)),
            ..four_titles()
        },
        "fix",
    );

    // When selecting the next match.
    sessions.select_next_match(&layouts_for(&sessions.projects));

    // Then it lands on card 3's agent row.
    assert_eq!(
        sessions.cursor,
        Some(agent_row(3, 3)),
        "search matches should stop on agent rows"
    );
}

#[rstest::rstest]
fn runs_agent_is_false_for_a_closed_pane() {
    // Given session 1 whose agent's pane 1 closed.
    let sessions = holding(
        vec![Thread {
            pane: None,
            last_session: Some(SessionId(1)),
            ..thread(1)
        }],
        vec![session(1)],
    );

    // When asking whether pane 1 runs one of session 1's agents.
    let runs = sessions.runs_agent(SessionId(1), PaneId(1), &layouts_for(&sessions.projects));

    // Then it doesn't.
    assert!(!runs, "a closed pane runs no agent");
}
