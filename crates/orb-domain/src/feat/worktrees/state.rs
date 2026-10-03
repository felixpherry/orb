//! orb's worktrees as last read, and the rule that decides which ones a sweep
//! removes.

use std::cmp::Reverse;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::AppState;
use crate::feat::git::git_service::WorktreeFacts;
use crate::feat::sessions::state::{DraftWorkspace, Group, GroupKind, Project, Thread, ThreadId};

/// How long after the newest settle a worktree is pruned.
pub const PRUNE_AFTER: Duration = Duration::from_hours(7 * 24);

/// Every worktree under `~/.orb/worktrees/<repo>/`, as the worktrees actor
/// last read them.
#[derive(Debug, Clone, Default)]
pub struct Worktrees {
    /// Sorted by path.
    pub list: Vec<Worktree>,
    /// `pruned N worktrees`, or why a delete failed.
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

/// Something in orb that runs in a worktree, or will.
#[derive(Debug, Clone)]
pub enum User<'a> {
    /// A thread outside any group whose session runs there.
    Thread(&'a Project, &'a Thread),
    /// A Feature group that lives there, with its threads.
    Group(&'a Project, &'a Group, Vec<&'a Thread>),
    /// A project's draft pointed at the worktree.
    Draft(&'a Project),
}

/// What the next sweep does with a worktree, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Kept: a thread in it is attached.
    Attached,
    /// Kept: a thread in it is mid-turn.
    MidTurn,
    /// Kept: a draft starts there.
    Draft,
    /// Kept: a thread or group in it is active.
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

/// Everything in orb that uses the worktree at `path`: lone threads whose
/// session runs there, Feature groups that live there (with their threads),
/// and drafts pointed at it. Removed projects count, since their threads
/// still run. Paths match exactly as stored.
#[must_use]
pub fn users<'a>(app: &'a AppState, path: &Path) -> Vec<User<'a>> {
    app.sessions
        .projects
        .iter()
        .flat_map(|project| {
            let threads = project
                .threads
                .iter()
                .filter(|thread| thread.group.is_none() && thread.cwd == path)
                .map(|thread| User::Thread(project, thread));
            let groups = project
                .groups
                .iter()
                .filter(|group| group.kind == GroupKind::Feature && group.dir.as_deref() == Some(path))
                .map(|group| {
                    let threads = project
                        .threads
                        .iter()
                        .filter(|thread| thread.group == Some(group.id))
                        .collect();
                    User::Group(project, group, threads)
                });
            let draft = project
                .draft
                .as_ref()
                .filter(|draft| matches!(&draft.workspace, DraftWorkspace::Existing(dir) if dir == path))
                .map(|_| User::Draft(project));
            threads.chain(groups).chain(draft)
        })
        .collect()
}

/// The threads among `users`: lone threads and every group's threads.
fn user_threads<'a>(users: &[User<'a>]) -> Vec<&'a Thread> {
    users
        .iter()
        .flat_map(|user| match user {
            User::Thread(_, thread) => vec![*thread],
            User::Group(_, _, threads) => threads.clone(),
            User::Draft(_) => Vec::new(),
        })
        .collect()
}

/// What the sweep does with a worktree that has these `users` and `facts`,
/// checked in order: kept while a thread in it is attached or mid-turn, a
/// draft starts there, a user is active, its facts are unknown or it has
/// changes; otherwise pruned now with no users, or [`PRUNE_AFTER`] after the
/// newest settle.
#[must_use]
pub fn verdict(
    users: &[User<'_>],
    facts: Option<&WorktreeFacts>,
    attached: &HashSet<ThreadId>,
    now: SystemTime,
) -> Verdict {
    let threads = user_threads(users);
    if threads.iter().any(|thread| attached.contains(&thread.id)) {
        return Verdict::Attached;
    }
    if threads.iter().any(|thread| thread.status.in_progress()) {
        return Verdict::MidTurn;
    }
    if users.iter().any(|user| matches!(user, User::Draft(_))) {
        return Verdict::Draft;
    }
    let settles: Vec<Option<SystemTime>> = users
        .iter()
        .filter_map(|user| match user {
            User::Thread(_, thread) => Some(thread.settled_at),
            User::Group(_, group, _) => Some(group.settled_at),
            User::Draft(_) => None,
        })
        .collect();
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
        Verdict::Attached | Verdict::MidTurn | Verdict::Draft | Verdict::Active => RowState::Active,
        _ if users.is_empty() => RowState::Orphan,
        _ => RowState::Settled,
    }
}

/// When the worktree was last used: the newest chat among its users' threads
/// (a group without threads counts from its creation, a draft from its
/// own), or with no users, its last commit. `None` reads as never.
#[must_use]
pub fn last_used(users: &[User<'_>], facts: Option<&WorktreeFacts>) -> Option<SystemTime> {
    match users {
        [] => facts
            .and_then(|facts| facts.last_commit.as_ref())
            .map(|(time, _)| *time),
        users => users
            .iter()
            .filter_map(|user| match user {
                User::Thread(_, thread) => Some(thread.last_chat()),
                User::Group(_, group, threads) => Some(
                    threads
                        .iter()
                        .map(|thread| thread.last_chat())
                        .max()
                        .unwrap_or(group.created_at),
                ),
                User::Draft(project) => project.draft.as_ref().map(|draft| draft.created_at),
            })
            .max(),
    }
}

/// orb's worktrees as the picker lists them: newest use first, worktrees
/// with no users last, then by path.
#[must_use]
pub fn order(app: &AppState) -> Vec<&Worktree> {
    let mut list: Vec<&Worktree> = app.worktrees.list.iter().collect();
    list.sort_by_cached_key(|worktree| {
        let users = users(app, &worktree.path);
        let used = last_used(&users, worktree.facts.as_ref());
        (users.is_empty(), Reverse(used), worktree.path.clone())
    });
    list
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::{RowState, User, Verdict, Worktree, Worktrees, order, row_state, users, verdict};
    use crate::AppState;
    use crate::feat::git::git_service::WorktreeFacts;
    use crate::feat::sessions::state::{
        Draft, DraftWorkspace, Group, GroupDefaults, GroupId, GroupKind, Project, ProjectId,
        ProjectKind, Sessions, Thread, ThreadId, ThreadStatus,
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
            id: ThreadId(id),
            title: None,
            cwd: PathBuf::from(cwd),
            transcript: None,
            status: ThreadStatus::Idle,
            turn_started_at: None,
            attach_argv: Vec::new(),
            branch: None,
            pinned_at: None,
            settled_at: None,
            active_since: UNIX_EPOCH,
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

    /// `thread`, in group `id`.
    fn grouped(thread: Thread, id: i64) -> Thread {
        Thread {
            group: Some(GroupId(id)),
            ..thread
        }
    }

    /// An unsettled Feature group living in `dir`.
    fn group(id: i64, dir: &str) -> Group {
        Group {
            id: GroupId(id),
            kind: GroupKind::Feature,
            name: format!("group-{id}"),
            dir: Some(PathBuf::from(dir)),
            branch: None,
            created_at: UNIX_EPOCH,
            pinned_at: None,
            settled_at: None,
            active_since: UNIX_EPOCH,
            defaults: GroupDefaults::default(),
            draft: false,
        }
    }

    fn project(threads: Vec<Thread>, groups: Vec<Group>) -> Project {
        Project {
            id: ProjectId(1),
            title: "orb".to_owned(),
            root: PathBuf::from("/code/orb"),
            created_at: UNIX_EPOCH,
            threads,
            draft: None,
            removed: false,
            kind: ProjectKind::Normal,
            groups,
        }
    }

    fn app(projects: Vec<Project>) -> AppState {
        AppState {
            sessions: Sessions {
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
        let app = app(vec![project(vec![], vec![])]);

        // When judging it with clean facts.
        let verdict = verdict_at(&app, Some(&facts(0)));

        // Then it prunes now.
        assert_eq!(verdict, Verdict::PruneNow, "an orphan should prune now");
    }

    #[rstest::rstest]
    fn verdict_after_eight_settled_days_is_prune_now() {
        // Given a thread in the worktree settled eight days ago.
        let app = app(vec![project(vec![settled(thread(1, WT), 8)], vec![])]);

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
        let app = app(vec![project(vec![settled(thread(1, WT), 6)], vec![])]);

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
        let app = app(vec![project(
            vec![settled(thread(1, WT), 10), settled(thread(2, WT), 6)],
            vec![],
        )]);

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
    fn verdict_with_an_active_user_is_active() {
        // Given an unsettled, idle thread in the worktree.
        let app = app(vec![project(vec![thread(1, WT)], vec![])]);

        // When judging it with clean facts.
        let verdict = verdict_at(&app, Some(&facts(0)));

        // Then it's kept as active.
        assert_eq!(verdict, Verdict::Active, "an unsettled thread keeps it");
    }

    #[rstest::rstest]
    fn verdict_with_an_attached_settled_thread_is_attached() {
        // Given a thread settled eight days ago that is attached.
        let app = AppState {
            attached: HashSet::from([ThreadId(1)]),
            ..app(vec![project(vec![settled(thread(1, WT), 8)], vec![])])
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
        let app = app(vec![project(vec![working], vec![])]);

        // When judging it with clean facts.
        let verdict = verdict_at(&app, Some(&facts(0)));

        // Then it's kept as mid-turn.
        assert_eq!(verdict, Verdict::MidTurn, "a turn underway keeps it");
    }

    #[rstest::rstest]
    fn verdict_with_a_draft_is_draft() {
        // Given a project draft pointed at the worktree.
        let app = app(vec![Project {
            draft: Some(Draft {
                workspace: DraftWorkspace::Existing(PathBuf::from(WT)),
                branch: None,
                model: None,
                permission: None,
                created_at: UNIX_EPOCH,
                repo: true,
                from: None,
            }),
            ..project(vec![], vec![])
        }]);

        // When judging it with clean facts.
        let verdict = verdict_at(&app, Some(&facts(0)));

        // Then it's kept for the draft.
        assert_eq!(verdict, Verdict::Draft, "a draft pointed at it keeps it");
    }

    #[rstest::rstest]
    fn verdict_with_changes_is_dirty() {
        // Given a thread settled eight days ago.
        let app = app(vec![project(vec![settled(thread(1, WT), 8)], vec![])]);

        // When judging it with three uncommitted files.
        let verdict = verdict_at(&app, Some(&facts(3)));

        // Then it's kept as dirty.
        assert_eq!(verdict, Verdict::Dirty, "uncommitted changes keep it");
    }

    #[rstest::rstest]
    fn verdict_without_facts_is_unknown() {
        // Given a thread settled eight days ago.
        let app = app(vec![project(vec![settled(thread(1, WT), 8)], vec![])]);

        // When judging it before its facts are read.
        let verdict = verdict_at(&app, None);

        // Then it's unknown.
        assert_eq!(verdict, Verdict::Unknown, "unread facts should be unknown");
    }

    #[rstest::rstest]
    fn verdict_of_a_feature_group_settled_eight_days_is_prune_now() {
        // Given a Feature group in the worktree settled eight days ago, with an
        // idle thread.
        let app = app(vec![project(
            vec![grouped(thread(1, WT), 7)],
            vec![Group {
                settled_at: Some(now() - days(8)),
                ..group(7, WT)
            }],
        )]);

        // When judging it with clean facts.
        let verdict = verdict_at(&app, Some(&facts(0)));

        // Then it prunes now.
        assert_eq!(
            verdict,
            Verdict::PruneNow,
            "the group's settle should count"
        );
    }

    #[rstest::rstest]
    fn verdict_of_an_active_feature_group_with_idle_threads_is_active() {
        // Given an unsettled Feature group in the worktree with an idle thread.
        let app = app(vec![project(
            vec![grouped(thread(1, WT), 7)],
            vec![group(7, WT)],
        )]);

        // When judging it with clean facts.
        let verdict = verdict_at(&app, Some(&facts(0)));

        // Then it's kept as active.
        assert_eq!(verdict, Verdict::Active, "an unsettled group keeps it");
    }

    #[rstest::rstest]
    fn users_ignore_threads_inside_a_group_as_lone_users() {
        // Given a Feature group in the worktree whose thread also runs there.
        let app = app(vec![project(
            vec![grouped(thread(1, WT), 7)],
            vec![group(7, WT)],
        )]);

        // When listing the worktree's users.
        let users = users(&app, Path::new(WT));

        // Then the thread comes only through its group.
        assert!(
            matches!(users.as_slice(), [User::Group(_, _, threads)] if threads.len() == 1),
            "expected one group user holding the thread, got {users:?}"
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
            ..app(vec![project(
                vec![
                    chatted(thread(1, "/w/orb/orb-b"), 10),
                    chatted(thread(2, "/w/orb/orb-c"), 20),
                ],
                vec![],
            )])
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
        let app = app(vec![project(threads, vec![])]);

        // When working out its row state.
        let state = row_state(&users(&app, Path::new(WT)), verdict);

        // Then it matches the case.
        assert_eq!(state, expected, "row state for {verdict:?}");
    }

    #[rstest::rstest]
    fn users_include_threads_of_removed_projects() {
        // Given a removed project with a thread in the worktree.
        let app = app(vec![Project {
            removed: true,
            ..project(vec![thread(1, WT)], vec![])
        }]);

        // When listing the worktree's users.
        let users = users(&app, Path::new(WT));

        // Then the thread is one of them.
        assert!(
            matches!(users.as_slice(), [User::Thread(_, thread)] if thread.id == ThreadId(1)),
            "a removed project's thread should count, got {users:?}"
        );
    }
}
