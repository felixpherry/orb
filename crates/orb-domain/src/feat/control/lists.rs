//! The read-only answers: the lists of sessions, tabs, panes, agents and
//! projects, where the caller is, and one agent in full.
//!
//! A session list shows open sessions and an agent list live agents unless
//! asked for all; each counts the rows it left out as `hidden` and gives a
//! `hint` naming the command that lists them. Names are cut to 60 graphemes.
//!
//! It also reads what a close or settle ends, before the change runs.

use serde::Serialize;

use crate::AppState;
use crate::feat::control::agent;
use crate::feat::control::protocol::{
    self, AgentDetail, AgentRow, AgentsResult, ApiError, EndedRow, ErrorCode, HereResult, Op,
    PaneRow, PanesResult, ProjectRow, ProjectsResult, Reply, Request, SessionRow, SessionsResult,
    SettledResult, TabRow, TabsResult,
};
use crate::feat::control::target::{self, Resolved, Target};
use crate::feat::layout::state::Tab;
use crate::feat::sessions::state::{Agent, AgentStatus, Project, ProjectKind, Session};
use crate::ids::{PaneId, ProjectId, SessionId};

/// The reply to a list `request`, read from `app`.
pub(super) fn route(app: &AppState, request: &Request) -> Reply {
    match request.op {
        Op::SessionList { all, project } => answer(sessions(app, all, project)),
        Op::TabList { session } => match in_session(app, session, request.caller) {
            Ok(resolved) => Reply::ok(&tabs(app, resolved.session)),
            Err(error) => Reply::err(error),
        },
        Op::PaneList { session } => match in_session(app, session, request.caller) {
            Ok(resolved) => Reply::ok(&panes(app, resolved.session, request.caller)),
            Err(error) => Reply::err(error),
        },
        Op::AgentList {
            all,
            session,
            ref status,
        } => answer(agents(app, all, session, status, request.caller)),
        Op::ProjectList => Reply::ok(&projects(app)),
        Op::Here => match here(app, request.caller) {
            Ok(here) => Reply::ok(&here),
            Err(error) => Reply::err(error),
        },
        _ => Reply::err(ApiError::new(
            ErrorCode::BadRequest,
            "not a list",
            "orb help lists the commands",
        )),
    }
}

/// Where `caller` is: its pane, tab, session and agent; `no_target` outside orb.
fn here(app: &AppState, caller: Option<PaneId>) -> Result<HereResult, ApiError> {
    let Resolved {
        session,
        pane: Some(pane),
    } = target::resolve(app, None, caller)?
    else {
        return Err(ApiError::new(
            ErrorCode::NoTarget,
            "not inside an orb pane",
            "run orb from inside an orb pane",
        ));
    };
    let tab = app.layouts.get(session).and_then(|layout| {
        layout
            .tabs()
            .iter()
            .position(|tab| tab.pane_ids().contains(&pane))
            .map(|index| index + 1)
    });
    let session_name = app
        .sessions
        .session(session)
        .map(|found| protocol::short_name(&app.sessions.title(found)))
        .unwrap_or_default();
    let agent = app
        .sessions
        .agent_rows(session, &app.layouts)
        .into_iter()
        .find(|&(_, shown)| shown == pane)
        .map(|(agent, _)| agent.id);
    Ok(HereResult {
        pane,
        tab,
        session,
        session_name,
        agent,
    })
}

/// The session named, else the caller's.
fn in_session(
    app: &AppState,
    session: Option<SessionId>,
    caller: Option<PaneId>,
) -> Result<Resolved, ApiError> {
    target::resolve(app, session.map(Target::Session), caller)
}

/// `result` as a reply: its value, or its refusal.
fn answer<T: Serialize>(result: Result<T, ApiError>) -> Reply {
    result.map_or_else(Reply::err, |ok| Reply::ok(&ok))
}

/// Sessions not being deleted, of `project` when given: open ones, or all
/// with `all`. `hidden` counts the settled ones the default left out.
fn sessions(
    app: &AppState,
    all: bool,
    project: Option<ProjectId>,
) -> Result<SessionsResult, ApiError> {
    if let Some(id) = project
        && app.sessions.project(id).is_none()
    {
        return Err(no_project(id));
    }
    let (shown, hidden): (Vec<&Session>, Vec<&Session>) = app
        .sessions
        .sessions
        .iter()
        .filter(|session| !app.sessions.deleting.contains(&session.id))
        .filter(|session| project.is_none_or(|id| session.project == id))
        .partition(|session| all || session.settled_at.is_none());
    let sessions = shown
        .into_iter()
        .map(|session| SessionRow {
            id: session.id,
            project: session.project,
            name: protocol::short_name(&app.sessions.title(session)),
            dir: session.dir.to_string_lossy().into_owned(),
            branch: session.branch.clone(),
            settled: session.settled_at.is_some(),
        })
        .collect();
    let base = command("orb session list", "--project", project.map(|id| id.0));
    Ok(SessionsResult {
        sessions,
        hidden: hidden.len(),
        hint: more(&base, hidden.len()),
    })
}

/// `"<command> --all shows N more"` when `hidden > 0`.
fn more(command: &str, hidden: usize) -> Option<String> {
    (hidden > 0).then(|| format!("{command} --all shows {hidden} more"))
}

/// `base`, plus ` <flag> <id>` when `id` is given, so a hint repeats the
/// caller's filter.
fn command(base: &str, flag: &str, id: Option<i64>) -> String {
    id.map_or_else(|| base.to_owned(), |id| format!("{base} {flag} {id}"))
}

/// `not_found` for project `id`, pointing at the project list.
fn no_project(id: ProjectId) -> ApiError {
    ApiError::new(
        ErrorCode::NotFound,
        format!("no project {}", id.0),
        "orb project list shows the projects",
    )
}

/// Projects not removed, in the order orb first used them.
fn projects(app: &AppState) -> ProjectsResult {
    ProjectsResult {
        projects: app
            .sessions
            .projects
            .iter()
            .filter(|project| !project.removed)
            .map(|project| ProjectRow {
                id: project.id,
                name: project.title.clone(),
                dir: project.root.to_string_lossy().into_owned(),
                kind: project_kind(project),
            })
            .collect(),
    }
}

/// How `project list` names `project`'s kind.
fn project_kind(project: &Project) -> &'static str {
    match project.kind {
        ProjectKind::Normal if project.repo => "repo",
        ProjectKind::Normal => "folder",
        ProjectKind::Research => "research",
        ProjectKind::Learn => "learn",
        ProjectKind::Incognito => "incognito",
        ProjectKind::SharedWorld => "shared_world",
        ProjectKind::FreshWorld => "fresh_world",
    }
}

/// Session `session`'s tabs, numbered from 1 as the tab bar shows them, with
/// their panes; none without a layout.
fn tabs(app: &AppState, session: SessionId) -> TabsResult {
    let tabs = app
        .layouts
        .get(session)
        .map(|layout| {
            layout
                .tabs()
                .iter()
                .enumerate()
                .map(|(index, tab)| TabRow {
                    index: index + 1,
                    panes: tab.pane_ids(),
                })
                .collect()
        })
        .unwrap_or_default();
    TabsResult { session, tabs }
}

/// Session `session`'s panes: each tab's, then the floats, with `caller`'s
/// marked.
fn panes(app: &AppState, session: SessionId, caller: Option<PaneId>) -> PanesResult {
    let Some(layout) = app.layouts.get(session) else {
        return PanesResult { panes: Vec::new() };
    };
    let agents = app.sessions.agent_rows(session, &app.layouts);
    let placed = layout
        .tabs()
        .iter()
        .enumerate()
        .flat_map(|(index, tab)| {
            tab.pane_ids()
                .into_iter()
                .map(move |pane| (pane, Some(index + 1)))
        })
        .chain(layout.floats().pane_ids().map(|pane| (pane, None)));
    let panes = placed
        .map(|(pane, tab)| {
            let entry = app.layouts.entry(pane);
            PaneRow {
                id: pane,
                session,
                tab,
                float: tab.is_none(),
                name: entry.and_then(|entry| entry.name.clone()),
                cwd: entry
                    .map(|entry| entry.cwd.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                agent: agents
                    .iter()
                    .find(|&&(_, shown)| shown == pane)
                    .map(|(agent, _)| agent.id),
                caller: Some(pane) == caller,
            }
        })
        .collect();
    PanesResult { panes }
}

/// Whether `agent list` shows an agent with `status` without `--all` or
/// `--status`.
fn listed_by_default(status: AgentStatus) -> bool {
    !matches!(status, AgentStatus::Stopped | AgentStatus::Gone)
}

/// Agents with a pane, of `session` when given (refused as an unknown
/// session is), filtered by `status` when it names any, else live ones unless
/// `all`. `hidden` counts only what the live-only default left out, so it is
/// 0 with `all` or `status`.
fn agents(
    app: &AppState,
    all: bool,
    session: Option<SessionId>,
    status: &[AgentStatus],
    caller: Option<PaneId>,
) -> Result<AgentsResult, ApiError> {
    if let Some(id) = session {
        target::resolve(app, Some(Target::Session(id)), caller)?;
    }
    let (shown, hidden): (Vec<_>, Vec<_>) = app
        .sessions
        .sessions
        .iter()
        .filter(|found| session.is_none_or(|id| found.id == id))
        .flat_map(|found| {
            app.sessions
                .agent_rows(found.id, &app.layouts)
                .into_iter()
                .map(move |(agent, pane)| (found.id, agent, pane))
        })
        .filter(|(_, agent, _)| status.is_empty() || status.contains(&agent.status))
        .partition(|(_, agent, _)| !status.is_empty() || all || listed_by_default(agent.status));
    let agents = shown
        .into_iter()
        .map(|(session, agent, pane)| agent_row(app, session, agent, pane))
        .collect();
    let base = command("orb agent list", "--session", session.map(|id| id.0));
    Ok(AgentsResult {
        agents,
        hidden: hidden.len(),
        hint: more(&base, hidden.len()),
    })
}

/// `agent`'s slim row in `agent list`: no cwd or branch, `title` only when
/// it differs from `name`, both cut to 60 graphemes.
fn agent_row(app: &AppState, session: SessionId, agent: &Agent, pane: PaneId) -> AgentRow {
    let name = app.layouts.agent_name(pane, Some(agent));
    AgentRow {
        id: agent.id,
        pane: Some(pane),
        session,
        harness: agent.harness.to_string(),
        name: protocol::short_name(name),
        title: agent
            .title
            .as_deref()
            .filter(|title| *title != name)
            .map(protocol::short_name),
        status: agent.status,
    }
}

/// `agent`'s full row, as `agent get` shows it: whole name and title, cwd
/// and branch.
pub(super) fn agent_detail(
    app: &AppState,
    session: SessionId,
    agent: &Agent,
    pane: PaneId,
) -> AgentDetail {
    AgentDetail {
        id: agent.id,
        pane: Some(pane),
        session,
        harness: agent.harness.to_string(),
        name: app.layouts.agent_name(pane, Some(agent)).to_owned(),
        title: agent.title.clone(),
        status: agent.status,
        cwd: agent.cwd.to_string_lossy().into_owned(),
        branch: agent.branch.clone(),
    }
}

/// What closing or settling `panes` ends, read before the change: each
/// pane's own name, and its live agent's list name.
pub(super) fn ended(app: &AppState, panes: &[PaneId]) -> Vec<EndedRow> {
    panes
        .iter()
        .map(|&pane| EndedRow {
            pane,
            name: app.layouts.entry(pane).and_then(|entry| entry.name.clone()),
            agent: agent::pane_agent(app, pane)
                .filter(|found| agent::live(found.status))
                .map(|found| protocol::short_name(app.layouts.agent_name(pane, Some(found)))),
        })
        .collect()
}

/// Every pane of the tab holding `pane`; none for a float.
pub(super) fn tab_panes(app: &AppState, pane: PaneId) -> Vec<PaneId> {
    app.layouts
        .owner_of(pane)
        .and_then(|session| app.layouts.get(session))
        .and_then(|layout| layout.tabs().iter().find(|tab| tab.holds(pane)))
        .map(Tab::pane_ids)
        .unwrap_or_default()
}

/// What settling `session` ends, read before it settles: its name and every
/// pane of its tabs and floats.
pub(super) fn settled(app: &AppState, session: SessionId) -> SettledResult {
    SettledResult {
        session,
        name: app
            .sessions
            .session(session)
            .map(|found| protocol::short_name(&app.sessions.title(found)))
            .unwrap_or_default(),
        ended: ended(app, &app.layouts.session_panes(session)),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use unicode_segmentation::UnicodeSegmentation;

    use super::route;
    use crate::AppState;
    use crate::feat::control::protocol::{ErrorCode, Op, Request};
    use crate::feat::sessions::state::{AgentStatus, ProjectKind};
    use crate::ids::{PaneId, ProjectId, SessionId};
    use crate::intent_handler::test_support::{
        agent, named, settle, state_with, stopped, with_projects,
    };

    /// What `op` lists from `app`, asked from outside orb; `null` when refused.
    fn listed(app: &AppState, op: Op) -> Value {
        route(app, &Request { caller: None, op })
            .result
            .unwrap_or_default()
    }

    /// The code and hint of `op`'s refusal from `app`, when refused.
    fn refused(app: &AppState, op: Op) -> Option<(ErrorCode, String)> {
        route(app, &Request { caller: None, op })
            .error
            .map(|error| (error.code, error.hint.unwrap_or_default()))
    }

    /// The `id` of each row under `key` in `result`.
    fn ids(result: &Value, key: &str) -> Vec<i64> {
        result
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| row.get("id").and_then(Value::as_i64))
            .collect()
    }

    /// `result`'s ids under `key`, its `hidden` count and its `hint`.
    fn summary(result: &Value, key: &str) -> (Vec<i64>, Option<u64>, Option<String>) {
        (
            ids(result, key),
            result.get("hidden").and_then(Value::as_u64),
            result
                .get("hint")
                .and_then(Value::as_str)
                .map(str::to_owned),
        )
    }

    /// The first row under `key` in `result`.
    fn first_row<'a>(result: &'a Value, key: &str) -> Option<&'a Value> {
        result.get(key).and_then(Value::as_array)?.first()
    }

    /// Sessions 1–3 with agents 1 (idle), 2 and 3 (stopped); 2 and 3 settled.
    fn two_settled() -> AppState {
        settle(
            state_with(vec![agent(1, AgentStatus::Idle), stopped(2), stopped(3)], 1),
            &[2, 3],
        )
    }

    /// `agent list` with no filter.
    fn every_agent(all: bool) -> Op {
        Op::AgentList {
            all,
            session: None,
            status: vec![],
        }
    }

    #[rstest::rstest]
    fn session_list_shows_open_sessions_and_counts_the_hidden() {
        // Given session 1 open and sessions 2 and 3 settled.
        let app = two_settled();

        // When listing sessions.
        let result = listed(
            &app,
            Op::SessionList {
                all: false,
                project: None,
            },
        );

        // Then only session 1 shows, with the two settled ones counted.
        assert_eq!(
            summary(&result, "sessions"),
            (
                vec![1],
                Some(2),
                Some("orb session list --all shows 2 more".to_owned())
            ),
            "settled sessions are hidden by default, got {result}"
        );
    }

    #[rstest::rstest]
    fn session_list_all_shows_settled_ones_too() {
        // Given session 1 open and sessions 2 and 3 settled.
        let app = two_settled();

        // When listing sessions with --all.
        let result = listed(
            &app,
            Op::SessionList {
                all: true,
                project: None,
            },
        );

        // Then all three show, with nothing hidden and no hint.
        assert_eq!(
            summary(&result, "sessions"),
            (vec![1, 2, 3], Some(0), None),
            "--all shows settled sessions, got {result}"
        );
    }

    #[rstest::rstest]
    fn session_list_filters_by_project() {
        // Given sessions 1 and 2 of project 1, 2 settled, and session 3
        // moved to project 2.
        let mut app = settle(
            state_with(
                vec![
                    agent(1, AgentStatus::Idle),
                    stopped(2),
                    agent(3, AgentStatus::Idle),
                ],
                1,
            ),
            &[2],
        );
        for session in &mut app.sessions.sessions {
            if session.id == SessionId(3) {
                session.project = ProjectId(2);
            }
        }

        // When listing project 1's sessions.
        let result = listed(
            &app,
            Op::SessionList {
                all: false,
                project: Some(ProjectId(1)),
            },
        );

        // Then only session 1 shows, and the hint repeats the project.
        assert_eq!(
            summary(&result, "sessions"),
            (
                vec![1],
                Some(1),
                Some("orb session list --project 1 --all shows 1 more".to_owned())
            ),
            "another project's sessions are left out, not hidden, got {result}"
        );
    }

    #[rstest::rstest]
    fn session_list_of_an_unknown_project_is_not_found() {
        // Given one project, 1.
        let app = two_settled();

        // When listing project 99's sessions.
        let refusal = refused(
            &app,
            Op::SessionList {
                all: false,
                project: Some(ProjectId(99)),
            },
        );

        // Then it is refused, pointing at the project list.
        assert!(
            refusal
                .as_ref()
                .is_some_and(|(code, hint)| *code == ErrorCode::NotFound
                    && hint.contains("orb project list")),
            "an unknown project is named, not listed as empty, got {refusal:?}"
        );
    }

    #[rstest::rstest]
    fn agent_list_shows_live_agents_and_counts_the_hidden() {
        // Given agents 1 (idle), 2 (stopped) and 3 (gone).
        let app = state_with(
            vec![
                agent(1, AgentStatus::Idle),
                stopped(2),
                agent(3, AgentStatus::Gone),
            ],
            1,
        );

        // When listing agents.
        let result = listed(&app, every_agent(false));

        // Then only agent 1 shows, with the other two counted.
        assert_eq!(
            summary(&result, "agents"),
            (
                vec![1],
                Some(2),
                Some("orb agent list --all shows 2 more".to_owned())
            ),
            "stopped and gone agents are hidden by default, got {result}"
        );
    }

    #[rstest::rstest]
    fn agent_list_all_shows_stopped_and_gone_ones() {
        // Given agents 1 (idle), 2 (stopped) and 3 (gone).
        let app = state_with(
            vec![
                agent(1, AgentStatus::Idle),
                stopped(2),
                agent(3, AgentStatus::Gone),
            ],
            1,
        );

        // When listing agents with --all.
        let result = listed(&app, every_agent(true));

        // Then all three show, with nothing hidden and no hint.
        let (mut shown, hidden, hint) = summary(&result, "agents");
        shown.sort_unstable();
        assert_eq!(
            (shown, hidden, hint),
            (vec![1, 2, 3], Some(0), None),
            "--all shows every agent, got {result}"
        );
    }

    #[rstest::rstest]
    fn agent_list_status_lists_exactly_those() {
        // Given agents 1 (working), 2 (needs input), 3 (stopped), 4 (idle).
        let app = state_with(
            vec![
                agent(1, AgentStatus::Working),
                agent(2, AgentStatus::NeedsInput),
                stopped(3),
                agent(4, AgentStatus::Idle),
            ],
            1,
        );

        // When listing agents that need input or are stopped.
        let result = listed(
            &app,
            Op::AgentList {
                all: false,
                session: None,
                status: vec![AgentStatus::NeedsInput, AgentStatus::Stopped],
            },
        );

        // Then agents 2 and 3 show, with nothing counted as hidden.
        let (mut shown, hidden, hint) = summary(&result, "agents");
        shown.sort_unstable();
        assert_eq!(
            (shown, hidden, hint),
            (vec![2, 3], Some(0), None),
            "--status lists those statuses, stopped included, got {result}"
        );
    }

    #[rstest::rstest]
    fn agent_list_status_wins_over_all() {
        // Given agents 1 (idle) and 2 (stopped).
        let app = state_with(vec![agent(1, AgentStatus::Idle), stopped(2)], 1);

        // When listing stopped agents with --all as well.
        let result = listed(
            &app,
            Op::AgentList {
                all: true,
                session: None,
                status: vec![AgentStatus::Stopped],
            },
        );

        // Then only the stopped agent shows.
        assert_eq!(
            ids(&result, "agents"),
            vec![2],
            "--status decides what shows even with --all, got {result}"
        );
    }

    #[rstest::rstest]
    fn agent_list_shows_failed_agents_by_default() {
        // Given agent 1, failed.
        let app = state_with(vec![agent(1, AgentStatus::Failed)], 1);

        // When listing agents.
        let result = listed(&app, every_agent(false));

        // Then it shows.
        assert_eq!(
            ids(&result, "agents"),
            vec![1],
            "a failed agent is one the caller should see, got {result}"
        );
    }

    #[rstest::rstest]
    fn agent_list_filters_by_session() {
        // Given agents 1 and 2, each in its own session.
        let app = state_with(
            vec![agent(1, AgentStatus::Idle), agent(2, AgentStatus::Idle)],
            1,
        );

        // When listing session 2's agents.
        let result = listed(
            &app,
            Op::AgentList {
                all: false,
                session: Some(SessionId(2)),
                status: vec![],
            },
        );

        // Then only agent 2 shows.
        assert_eq!(
            ids(&result, "agents"),
            vec![2],
            "--session lists that session's agents, got {result}"
        );
    }

    #[rstest::rstest]
    fn agent_list_of_an_unknown_session_is_not_found() {
        // Given agents 1 and 2, each in its own session.
        let app = state_with(
            vec![agent(1, AgentStatus::Idle), agent(2, AgentStatus::Idle)],
            1,
        );

        // When listing session 99's agents.
        let refusal = refused(
            &app,
            Op::AgentList {
                all: false,
                session: Some(SessionId(99)),
                status: vec![],
            },
        );

        // Then it is refused as not found.
        assert_eq!(
            refusal.map(|(code, _)| code),
            Some(ErrorCode::NotFound),
            "an unknown session is named, not listed as empty"
        );
    }

    #[rstest::rstest]
    fn agent_list_row_drops_cwd_branch_and_a_title_equal_to_its_name() {
        // Given agent 1 titled "fix login", its pane unnamed.
        let app = state_with(vec![named(agent(1, AgentStatus::Idle), "fix login")], 1);

        // When listing agents.
        let result = listed(&app, every_agent(false));

        // Then its row has no cwd, branch or title.
        let mut keys: Vec<&str> = first_row(&result, "agents")
            .and_then(Value::as_object)
            .map(|row| row.keys().map(String::as_str).collect())
            .unwrap_or_default();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["harness", "id", "name", "pane", "session", "status"],
            "a slim row leaves out what the session row or name already says"
        );
    }

    #[rstest::rstest]
    fn agent_list_row_keeps_a_title_that_differs_from_its_name() {
        // Given agent 1 titled "fix login", its pane renamed "server".
        let mut app = state_with(vec![named(agent(1, AgentStatus::Idle), "fix login")], 1);
        app.layouts
            .rename_pane(PaneId(1), Some("server".to_owned()));

        // When listing agents.
        let result = listed(&app, every_agent(false));

        // Then its row names the pane and keeps the title.
        let row = first_row(&result, "agents");
        let field = |key: &str| row?.get(key)?.as_str();
        assert_eq!(
            (field("name"), field("title")),
            (Some("server"), Some("fix login")),
            "a title that differs from the name is kept, got {result}"
        );
    }

    #[rstest::rstest]
    #[case::session(Op::SessionList { all: false, project: None }, "sessions")]
    #[case::agent(Op::AgentList { all: false, session: None, status: vec![] }, "agents")]
    fn list_names_are_cut_to_60_graphemes(#[case] op: Op, #[case] key: &str) {
        // Given agent 1 titled with 700 two-codepoint graphemes.
        let long = "e\u{301}".repeat(700);
        let app = state_with(vec![named(agent(1, AgentStatus::Idle), &long)], 1);

        // When listing.
        let result = listed(&app, op);

        // Then the row's name is 60 graphemes, ending in an ellipsis.
        let name = first_row(&result, key)
            .and_then(|row| row.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            name.graphemes(true).count() == 60 && name.ends_with('…'),
            "a long name is cut to 60 graphemes, got {} graphemes",
            name.graphemes(true).count()
        );
    }

    #[rstest::rstest]
    #[case::repo(ProjectKind::Normal, true, "repo")]
    #[case::folder(ProjectKind::Normal, false, "folder")]
    #[case::research(ProjectKind::Research, false, "research")]
    #[case::learn(ProjectKind::Learn, false, "learn")]
    #[case::incognito(ProjectKind::Incognito, false, "incognito")]
    #[case::shared_world(ProjectKind::SharedWorld, false, "shared_world")]
    #[case::fresh_world(ProjectKind::FreshWorld, false, "fresh_world")]
    fn project_list_names_each_kind(
        #[case] kind: ProjectKind,
        #[case] repo: bool,
        #[case] expected: &str,
    ) {
        // Given project 1 "work" at /work, of `kind`.
        let mut app = with_projects(&["work"]);
        for project in &mut app.sessions.projects {
            project.kind = kind;
            project.repo = repo;
        }

        // When listing projects.
        let result = listed(&app, Op::ProjectList);

        // Then its row carries its ID, title, folder and kind.
        let row = first_row(&result, "projects");
        assert_eq!(
            row.cloned(),
            Some(serde_json::json!({
                "id": 1, "name": "work", "dir": "/work", "kind": expected,
            })),
            "a {kind:?} project (repo: {repo}) lists as {expected}"
        );
    }

    #[rstest::rstest]
    fn project_list_leaves_out_removed_projects() {
        // Given projects 1 "a" and 2 "b", with 2 removed.
        let mut app = with_projects(&["a", "b"]);
        for project in &mut app.sessions.projects {
            project.removed = project.id == ProjectId(2);
        }

        // When listing projects.
        let result = listed(&app, Op::ProjectList);

        // Then only project 1 shows.
        assert_eq!(
            ids(&result, "projects"),
            vec![1],
            "a removed project isn't listed, got {result}"
        );
    }
}
