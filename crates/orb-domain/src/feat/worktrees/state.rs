//! orb's worktrees as last read, and the rule that decides which ones a sweep
//! removes.

use std::cmp::Reverse;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::AppState;
use crate::feat::git::git_service::WorktreeFacts;
use crate::feat::sessions::state::{Project, Session, SessionId, Thread};

/// How long after the newest settle a worktree is pruned.
pub const PRUNE_AFTER: Duration = Duration::from_hours(7 * 24);

/// Every worktree under `~/.orb/worktrees/<repo>/`, as the worktrees actor
/// last read them.
#[derive(Debug, Clone, Default)]
pub struct Worktrees {
    /// Sorted by path.
    pub list: Vec<Worktree>,
    /// `pruned N worktrees`, or why a delete failed. Besides the worktrees
    /// actor, the intent handler writes it: it clears it on the next intent.
    pub notice: Option<String>,
}

/// One directory under `~/.orb/worktrees/<repo>/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// The main repository; `None` when the directory isn't a linked worktree
    /// or git can't tell.
    pub repo: Option<PathBuf>,
    /// `None` until read, or when git can't read them.
    pub facts: Option<WorktreeFacts>,
    /// Kilobytes on disk; `None` until `du` lands.
    pub size_kb: Option<u64>,
}

/// Something in orb that runs in a worktree.
#[derive(Debug, Clone)]
pub enum User<'a> {
    /// A session whose directory it is, with its agents.
    Session(&'a Project, &'a Session, Vec<&'a Thread>),
}

/// What the next sweep does with a worktree, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Kept: a session in it is attached.
    Attached,
    /// Kept: an agent in it is mid-turn.
    MidTurn,
    /// Kept: a session in it isn't settled.
    Active,
    /// Kept: git's facts aren't known yet, or can't be read.
    Unknown,
    /// Kept: it has uncommitted or untracked changes.
    Dirty,
    /// Removed at the next sweep.
    PruneNow,
    /// Removed once this much more time has passed.
    PruneIn(Duration),
}

/// How the worktree picker groups a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    /// Something in it is in use.
    Active,
    /// It has users, all settled.
    Settled,
    /// Nothing in orb uses it.
    Orphan,
}

/// Everything in orb that uses the worktree at `path`: sessions whose
/// directory it is (with their agents). Sessions
/// of removed projects count, since their panes still run; sessions being
/// deleted don't. Paths match exactly as stored.
#[must_use]
pub fn users<'a>(app: &'a AppState, path: &Path) -> Vec<User<'a>> {
    let sessions = &app.sessions;
    sessions
        .sessions
        .iter()
        .filter(|session| session.dir == path && !sessions.deleting.contains(&session.id))
        .filter_map(|session| {
            let project = sessions.project(session.project)?;
            Some(User::Session(project, session, sessions.agents(session.id)))
        })
        .collect()
}

/// What the sweep does with a worktree that has these `users` and `facts`,
/// checked in order: kept while a session in it is attached or an agent in
/// it is mid-turn, a session in it isn't settled, its
/// facts are unknown or it has changes; otherwise pruned now with no users,
/// or [`PRUNE_AFTER`] after the newest settle.
#[must_use]
pub fn verdict(
    users: &[User<'_>],
    facts: Option<&WorktreeFacts>,
    attached: &HashSet<SessionId>,
    now: SystemTime,
) -> Verdict {
    let sessions = || {
        users.iter().map(|user| match user {
            User::Session(_, session, agents) => (*session, agents),
        })
    };
    if sessions().any(|(session, _)| attached.contains(&session.id)) {
        return Verdict::Attached;
    }
    if sessions().any(|(_, agents)| agents.iter().any(|thread| thread.status.in_progress())) {
        return Verdict::MidTurn;
    }
    let settles: Vec<Option<SystemTime>> =
        sessions().map(|(session, _)| session.settled_at).collect();
    if settles.iter().any(Option::is_none) {
        return Verdict::Active;
    }
    let Some(facts) = facts else {
        return Verdict::Unknown;
    };
    if facts.changes > 0 {
        return Verdict::Dirty;
    }
    // No users leaves no settle time.
    let Some(newest) = settles.into_iter().flatten().max() else {
        return Verdict::PruneNow;
    };
    let elapsed = now.duration_since(newest).unwrap_or_default();
    match PRUNE_AFTER.saturating_sub(elapsed) {
        Duration::ZERO => Verdict::PruneNow,
        left => Verdict::PruneIn(left),
    }
}

/// How the picker groups a worktree with these `users` and `verdict`.
#[must_use]
pub fn row_state(users: &[User<'_>], verdict: Verdict) -> RowState {
    match verdict {
        Verdict::Attached | Verdict::MidTurn | Verdict::Active => RowState::Active,
        _ if users.is_empty() => RowState::Orphan,
        _ => RowState::Settled,
    }
}

/// When the worktree was last used: the newest chat among its sessions'
/// agents (a session without agents counts from its last activity), or with
/// no users, its last commit. `None` reads as
/// never.
#[must_use]
pub fn last_used(users: &[User<'_>], facts: Option<&WorktreeFacts>) -> Option<SystemTime> {
    match users {
        [] => facts
            .and_then(|facts| facts.last_commit.as_ref())
            .map(|(time, _)| *time),
        users => users
            .iter()
            .map(|user| match user {
                User::Session(_, session, agents) => agents
                    .iter()
                    .map(|thread| thread.last_chat())
                    .max()
                    .unwrap_or(session.last_activity_at),
            })
            .max(),
    }
}

/// orb's worktrees as the picker lists them: active ones first, worktrees
/// with no users last, newest use first within each, then by path.
#[must_use]
pub fn order(app: &AppState) -> Vec<&Worktree> {
    let mut list: Vec<&Worktree> = app.worktrees.list.iter().collect();
    list.sort_by_cached_key(|worktree| {
        let users = users(app, &worktree.path);
        let facts = worktree.facts.as_ref();
        // Whether a row is active never depends on the clock.
        let active = row_state(&users, verdict(&users, facts, &app.attached, UNIX_EPOCH))
            == RowState::Active;
        let used = last_used(&users, facts);
        (
            !active,
            users.is_empty(),
            Reverse(used),
            worktree.path.clone(),
        )
    });
    list
}

#[cfg(test)]
mod tests {
    use crate::feat::harness::HarnessId;
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::{RowState, User, Verdict, Worktree, Worktrees, order, row_state, users, verdict};
    use crate::AppState;
    use crate::feat::git::git_service::WorktreeFacts;
    use crate::feat::sessions::state::{
        PaneId, PaneLaunch, Project, ProjectId, ProjectKind, SessionId, Sessions, Thread, ThreadId,
        ThreadStatus, sessions_for,
    };

    const WT: &str = "/w/orb/orb-1";

    fn days(n: u64) -> Duration {
        Duration::from_hours(24 * n)
    }

    /// A fixed clock, far enough from the epoch that day counts are exact.
    fn now() -> SystemTime {
        UNIX_EPOCH + days(100)
    }

    /// An idle, unsettled thread outside any group, running in `cwd`.
    fn thread(id: i64, cwd: &str) -> Thread {
        Thread {
            last_session: None,
            harness: HarnessId::new("claude"),
            id: ThreadId(id),
            title: None,
            cwd: PathBuf::from(cwd),
            transcript: None,
            status: ThreadStatus::Idle,
            turn_started_at: None,
            pane: Some(PaneLaunch {
                pane: PaneId(id),
                session: SessionId(id),
            }),
            branch: None,
            pinned_at: None,
            settled_at: None,
            active_since: UNIX_EPOCH,
            created_at: UNIX_EPOCH,
            last_activity_at: UNIX_EPOCH,
            unseen: false,
            group: None,
            model: None,
            permission: None,
        }
    }

    /// `thread`, settled `ago` days before `now`.
    fn settled(thread: Thread, ago: u64) -> Thread {
        Thread {
            settled_at: Some(now() - days(ago)),
            ..thread
        }
    }

    /// `thread`, last chatted in `secs` seconds after the epoch.
    fn chatted(thread: Thread, secs: u64) -> Thread {
        Thread {
            last_activity_at: UNIX_EPOCH + Duration::from_secs(secs),
            ..thread
        }
    }

    fn project(threads: Vec<Thread>) -> Project {
        Project {
            id: ProjectId(1),
            title: "orb".to_owned(),
            root: PathBuf::from("/code/orb"),
            created_at: UNIX_EPOCH,
            threads,
            repo: true,
            removed: false,
            kind: ProjectKind::Normal,
        }
    }

    /// `projects`, each thread's session in its directory.
    fn app(projects: Vec<Project>) -> AppState {
        AppState {
            sessions: Sessions {
                sessions: sessions_for(&projects),
                projects,
                ..Sessions::default()
            },
            ..AppState::default()
        }
    }

    fn facts(changes: usize) -> WorktreeFacts {
        WorktreeFacts {
            branch: Some("orb/1".to_owned()),
            changes,
            last_commit: None,
        }
    }

    /// The verdict on `WT` in `app`, at `now`.
    fn verdict_at(app: &AppState, facts: Option<&WorktreeFacts>) -> Verdict {
        verdict(&users(app, Path::new(WT)), facts, &app.attached, now())
    }

    #[rstest::rstest]
    fn verdict_of_an_orphan_is_prune_now() {
        // Given a worktree nothing uses.
        let app = app(vec![project(vec![])]);

        // When judging it with clean facts.
        let verdict = verdict_at(&app, Some(&facts(0)));

        // Then it prunes now.
        assert_eq!(verdict, Verdict::PruneNow, "an orphan should prune now");
    }

    #[rstest::rstest]
    fn worktree_whose_sessions_settled_a_week_ago_is_prunable() {
        // Given a thread in the worktree settled eight days ago.
        let app = app(vec![project(vec![settled(thread(1, WT), 8)])]);

        // When judging it with clean facts.
        let verdict = verdict_at(&app, Some(&facts(0)));

        // Then it prunes now.
        assert_eq!(
            verdict,
            Verdict::PruneNow,
            "8 settled days is past the limit"
        );
    }

    #[rstest::rstest]
    fn verdict_after_six_settled_days_is_prune_in_one_day() {
        // Given a thread in the worktree settled six days ago.
        let app = app(vec![project(vec![settled(thread(1, WT), 6)])]);

        // When judging it with clean facts.
        let verdict = verdict_at(&app, Some(&facts(0)));

        // Then it prunes in a day.
        assert_eq!(
            verdict,
            Verdict::PruneIn(days(1)),
            "6 settled days leaves one"
        );
    }

    #[rstest::rstest]
    fn verdict_of_two_users_counts_from_the_newest_settle() {
        // Given two threads in the worktree, settled ten and six days ago.
        let app = app(vec![project(vec![
            settled(thread(1, WT), 10),
            settled(thread(2, WT), 6),
        ])]);

        // When judging it with clean facts.
        let verdict = verdict_at(&app, Some(&facts(0)));

        // Then the countdown runs from the newer settle.
        assert_eq!(
            verdict,
            Verdict::PruneIn(days(1)),
            "the newest settle should set the countdown"
        );
    }

    #[rstest::rstest]
    fn worktree_of_an_unsettled_session_is_kept() {
        // Given an unsettled, idle thread in the worktree.
        let app = app(vec![project(vec![thread(1, WT)])]);

        // When judging it with clean facts.
        let verdict = verdict_at(&app, Some(&facts(0)));

        // Then it's kept as active.
        assert_eq!(verdict, Verdict::Active, "an unsettled thread keeps it");
    }

    #[rstest::rstest]
    fn worktree_of_an_attached_session_is_kept() {
        // Given a thread settled eight days ago that is attached.
        let app = AppState {
            attached: HashSet::from([SessionId(1)]),
            ..app(vec![project(vec![settled(thread(1, WT), 8)])])
        };

        // When judging it with clean facts.
        let verdict = verdict_at(&app, Some(&facts(0)));

        // Then it's kept as attached.
        assert_eq!(verdict, Verdict::Attached, "an attached thread keeps it");
    }

    #[rstest::rstest]
    fn verdict_with_a_mid_turn_thread_is_mid_turn() {
        // Given a thread in the worktree running a turn.
        let working = Thread {
            status: ThreadStatus::Working,
            ..thread(1, WT)
        };
        let app = app(vec![project(vec![working])]);

        // When judging it with clean facts.
        let verdict = verdict_at(&app, Some(&facts(0)));

        // Then it's kept as mid-turn.
        assert_eq!(verdict, Verdict::MidTurn, "a turn underway keeps it");
    }

    #[rstest::rstest]
    fn verdict_with_changes_is_dirty() {
        // Given a thread settled eight days ago.
        let app = app(vec![project(vec![settled(thread(1, WT), 8)])]);

        // When judging it with three uncommitted files.
        let verdict = verdict_at(&app, Some(&facts(3)));

        // Then it's kept as dirty.
        assert_eq!(verdict, Verdict::Dirty, "uncommitted changes keep it");
    }

    #[rstest::rstest]
    fn verdict_without_facts_is_unknown() {
        // Given a thread settled eight days ago.
        let app = app(vec![project(vec![settled(thread(1, WT), 8)])]);

        // When judging it before its facts are read.
        let verdict = verdict_at(&app, None);

        // Then it's unknown.
        assert_eq!(verdict, Verdict::Unknown, "unread facts should be unknown");
    }

    #[rstest::rstest]
    fn users_list_a_session_once_with_its_agents() {
        // Given threads 1 and 2 both running in session 1 in the worktree.
        let second = Thread {
            pane: Some(PaneLaunch {
                pane: PaneId(2),
                session: SessionId(1),
            }),
            ..thread(2, WT)
        };
        let app = app(vec![project(vec![thread(1, WT), second])]);

        // When listing the worktree's users.
        let users = users(&app, Path::new(WT));

        // Then session 1 is the one user, holding both agents.
        assert!(
            matches!(users.as_slice(), [User::Session(_, session, agents)]
                if session.id == SessionId(1) && agents.len() == 2),
            "expected session 1 with two agents, got {users:?}"
        );
    }

    #[rstest::rstest]
    fn order_puts_newest_use_first_and_orphans_last() {
        // Given an orphan with a recent commit, and two worktrees whose
        // threads last chatted at 10 s and 20 s.
        let worktree = |path: &str, last_commit: Option<u64>| Worktree {
            path: PathBuf::from(path),
            repo: None,
            facts: Some(WorktreeFacts {
                last_commit: last_commit
                    .map(|secs| (UNIX_EPOCH + Duration::from_secs(secs), "commit".to_owned())),
                ..facts(0)
            }),
            size_kb: None,
        };
        let app = AppState {
            worktrees: Worktrees {
                list: vec![
                    worktree("/w/orb/orb-a", Some(1_000)),
                    worktree("/w/orb/orb-b", None),
                    worktree("/w/orb/orb-c", None),
                ],
                notice: None,
            },
            ..app(vec![project(vec![
                chatted(thread(1, "/w/orb/orb-b"), 10),
                chatted(thread(2, "/w/orb/orb-c"), 20),
            ])])
        };

        // When ordering the worktrees.
        let paths: Vec<&Path> = order(&app).iter().map(|w| w.path.as_path()).collect();

        // Then the newest use leads and the orphan trails.
        assert_eq!(
            paths,
            [
                Path::new("/w/orb/orb-c"),
                Path::new("/w/orb/orb-b"),
                Path::new("/w/orb/orb-a"),
            ],
            "newest use first, orphans last"
        );
    }

    #[rstest::rstest]
    fn order_puts_active_worktrees_before_newer_settled_ones() {
        // Given a settled worktree last chatted at 20 s and an active one last
        // chatted at 10 s.
        let worktree = |path: &str| Worktree {
            path: PathBuf::from(path),
            repo: None,
            facts: Some(facts(0)),
            size_kb: None,
        };
        let app = AppState {
            worktrees: Worktrees {
                list: vec![worktree("/w/orb/orb-a"), worktree("/w/orb/orb-b")],
                notice: None,
            },
            ..app(vec![project(vec![
                settled(chatted(thread(1, "/w/orb/orb-a"), 20), 1),
                chatted(thread(2, "/w/orb/orb-b"), 10),
            ])])
        };

        // When ordering the worktrees.
        let paths: Vec<&Path> = order(&app).iter().map(|w| w.path.as_path()).collect();

        // Then the active one leads despite its older use.
        assert_eq!(
            paths,
            [Path::new("/w/orb/orb-b"), Path::new("/w/orb/orb-a")],
            "active first, then newest use"
        );
    }

    #[rstest::rstest]
    #[case(true, Verdict::Active, RowState::Active)]
    #[case(true, Verdict::PruneIn(days(1)), RowState::Settled)]
    #[case(false, Verdict::PruneNow, RowState::Orphan)]
    fn row_state_follows_users_and_verdict(
        #[case] used: bool,
        #[case] verdict: Verdict,
        #[case] expected: RowState,
    ) {
        // Given a worktree with or without a settled thread in it.
        let threads = if used {
            vec![settled(thread(1, WT), 6)]
        } else {
            vec![]
        };
        let app = app(vec![project(threads)]);

        // When working out its row state.
        let state = row_state(&users(&app, Path::new(WT)), verdict);

        // Then it matches the case.
        assert_eq!(state, expected, "row state for {verdict:?}");
    }

    #[rstest::rstest]
    fn users_include_sessions_of_removed_projects() {
        // Given a removed project with a session in the worktree.
        let app = app(vec![Project {
            removed: true,
            ..project(vec![thread(1, WT)])
        }]);

        // When listing the worktree's users.
        let users = users(&app, Path::new(WT));

        // Then the session is one of them.
        assert!(
            matches!(users.as_slice(), [User::Session(_, session, _)] if session.id == SessionId(1)),
            "a removed project's session should count, got {users:?}"
        );
    }
}
