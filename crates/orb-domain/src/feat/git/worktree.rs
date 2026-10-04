//! Where orb puts worktrees and how it names them: a new worktree lives at
//! `<worktrees root>/<repo>/orb-<hex>` on branch `orb/<hex>`, and that branch
//! is later renamed after the thread's title. Also finds the project's
//! previous worktree, the one the workspace picker offers.

use std::hash::{BuildHasher, RandomState};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use unicode_segmentation::UnicodeSegmentation;

use crate::feat::sessions::state::{Project, ThreadId};

/// The longest branch slug, in bytes.
const SLUG_MAX: usize = 40;

/// Eight random lowercase hex digits; `attempt` varies them between retries.
pub fn hex(attempt: u32) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let hash = RandomState::new().hash_one((nanos, attempt));
    format!("{:08x}", hash & 0xffff_ffff)
}

/// Where a new worktree of the repository at `repo_root` goes.
pub fn new_worktree_path(worktrees_root: &Path, repo_root: &Path, hex: &str) -> PathBuf {
    let repo = repo_root
        .file_name()
        .map_or_else(|| PathBuf::from("repo"), PathBuf::from);
    worktrees_root.join(repo).join(format!("orb-{hex}"))
}

/// Whether `cwd` is a worktree orb made: inside orb's worktrees root, never
/// the root itself, and with no `..` that could climb back out of it.
pub fn is_orb_worktree(worktrees_root: &Path, cwd: &Path) -> bool {
    cwd != worktrees_root
        && cwd.starts_with(worktrees_root)
        && !cwd
            .components()
            .any(|component| component == Component::ParentDir)
}

/// The branch orb made with the worktree at `cwd`, `orb/<hex>`, when `cwd` is
/// an orb worktree named `orb-<hex>`.
pub fn hex_branch(worktrees_root: &Path, cwd: &Path) -> Option<String> {
    if !is_orb_worktree(worktrees_root, cwd) {
        return None;
    }
    let hex = cwd.file_name()?.to_str()?.strip_prefix("orb-")?;
    let is_hex = hex.len() == 8
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    is_hex.then(|| format!("orb/{hex}"))
}

/// A branch-name slug of `title`: lowercase ASCII letters and digits, with
/// every run of anything else as one `-`, at most 40 bytes. `None` when
/// nothing is left.
pub fn slug(title: &str) -> Option<String> {
    let mut slug = String::new();
    for grapheme in title.graphemes(true) {
        match grapheme.as_bytes() {
            [byte] if byte.is_ascii_alphanumeric() => slug.push(byte.to_ascii_lowercase().into()),
            _ if !slug.is_empty() && !slug.ends_with('-') => slug.push('-'),
            _ => {}
        }
    }
    slug.truncate(SLUG_MAX);
    let slug = slug.trim_end_matches('-');
    (!slug.is_empty()).then(|| slug.to_owned())
}

/// The project's previous worktree seen from `cwd`: the worktree (not the
/// project's root, nor `cwd`) of the thread with the latest activity, other
/// than `except`. Returns its path and branch.
pub fn previous_worktree(
    project: &Project,
    cwd: &Path,
    except: Option<ThreadId>,
) -> Option<(PathBuf, Option<String>)> {
    project
        .threads
        .iter()
        .filter(|other| Some(other.id) != except && other.cwd != project.root && other.cwd != cwd)
        .max_by_key(|other| other.last_activity_at)
        .map(|other| (other.cwd.clone(), other.branch.clone()))
}

#[cfg(test)]
mod tests {
    use crate::feat::harness::HarnessId;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    use super::{hex_branch, is_orb_worktree, previous_worktree, slug};
    use crate::feat::sessions::state::{
        Project, ProjectId, ProjectKind, Thread, ThreadId, ThreadStatus,
    };

    const ROOT: &str = "/tmp/orb";

    /// A thread in `cwd` on `branch`, last active `activity` seconds in.
    fn thread(id: i64, cwd: &str, branch: &str, activity: u64) -> Thread {
        Thread {
            harness: HarnessId::new("claude"),
            id: ThreadId(id),
            title: None,
            cwd: PathBuf::from(cwd),
            transcript: None,
            status: ThreadStatus::Idle,
            turn_started_at: None,
            attach_argv: Vec::new(),
            branch: Some(branch.to_owned()),
            pinned_at: None,
            settled_at: None,
            active_since: SystemTime::UNIX_EPOCH,
            created_at: SystemTime::UNIX_EPOCH,
            last_activity_at: SystemTime::UNIX_EPOCH + Duration::from_secs(activity),
            unseen: false,
            group: None,
            model: None,
            permission: None,
        }
    }

    fn project(threads: Vec<Thread>) -> Project {
        Project {
            id: ProjectId(1),
            title: "orb".to_owned(),
            root: PathBuf::from(ROOT),
            created_at: SystemTime::UNIX_EPOCH,
            removed: false,
            threads,
            draft: None,
            groups: vec![],
            kind: ProjectKind::Normal,
        }
    }

    #[rstest::rstest]
    #[case("Fix the sidebar!", Some("fix-the-sidebar"))]
    #[case("  --Add: git worktrees?? ", Some("add-git-worktrees"))]
    #[case("Café ☕ menu", Some("caf-menu"))]
    #[case(
        "a very long title that keeps going well past the forty byte cap",
        Some("a-very-long-title-that-keeps-going-well")
    )]
    #[case("☕ — ☕", None)]
    #[case("", None)]
    fn slug_keeps_lowercase_ascii_words(#[case] title: &str, #[case] expected: Option<&str>) {
        assert_eq!(
            slug(title).as_deref(),
            expected,
            "slug of {title:?} should be {expected:?}"
        );
    }

    #[rstest::rstest]
    #[case("/home/u/.orb/worktrees/orb/orb-0a1b2c3d", Some("orb/0a1b2c3d"))]
    #[case("/home/u/.orb/worktrees/orb/orb-0A1B2C3D", None)]
    #[case("/home/u/.orb/worktrees/orb/orb-feature", None)]
    #[case("/home/u/dev/orb/orb-0a1b2c3d", None)]
    fn hex_branch_reads_the_worktree_directory_name(
        #[case] cwd: &str,
        #[case] expected: Option<&str>,
    ) {
        // Given orb's worktrees root.
        let root = Path::new("/home/u/.orb/worktrees");

        // When reading the hex branch of cwd.
        let branch = hex_branch(root, Path::new(cwd));

        // Then it is orb/<hex> only for an orb-<8 hex> worktree under the root.
        assert_eq!(
            branch.as_deref(),
            expected,
            "hex branch of {cwd} should be {expected:?}"
        );
    }

    #[rstest::rstest]
    #[case("/home/u/.orb/worktrees/orb/orb-0a1b2c3d", true)]
    #[case("/home/u/.orb/worktrees", false)]
    #[case("/home/u/.orb/worktrees/", false)]
    #[case("/home/u/.orb/worktrees/orb/../..", false)]
    #[case("/home/u/.orb/worktrees/../research/x", false)]
    #[case("/home/u/dev/orb", false)]
    fn orb_worktree_is_strictly_inside_the_worktrees_root(
        #[case] cwd: &str,
        #[case] expected: bool,
    ) {
        // Given orb's worktrees root.
        let root = Path::new("/home/u/.orb/worktrees");

        // When asking whether cwd is an orb worktree.
        let orb = is_orb_worktree(root, Path::new(cwd));

        // Then only a path strictly inside the root, without `..`, is one.
        assert_eq!(orb, expected, "{cwd} should be an orb worktree: {expected}");
    }

    #[rstest::rstest]
    fn previous_worktree_picks_the_latest_activity_among_other_worktrees() {
        // Given a fresh thread in the root, a root sibling active latest, and two worktree threads.
        let fresh = thread(1, ROOT, "main", 0);
        let project = project(vec![
            fresh.clone(),
            thread(2, ROOT, "main", 90),
            thread(3, "/wt/orb-old", "orb/old", 10),
            thread(4, "/wt/orb-new", "orb/new", 50),
        ]);

        // When finding the previous worktree for the fresh thread.
        let previous = previous_worktree(&project, &fresh.cwd, Some(fresh.id));

        // Then it's the most recently active worktree thread's directory and branch.
        assert_eq!(
            previous,
            Some((PathBuf::from("/wt/orb-new"), Some("orb/new".to_owned()))),
            "the latest worktree thread should be the seed"
        );
    }

    #[rstest::rstest]
    fn previous_worktree_is_none_without_worktree_threads() {
        // Given a project whose threads all run in its root.
        let fresh = thread(1, ROOT, "main", 0);
        let project = project(vec![fresh.clone(), thread(2, ROOT, "main", 90)]);

        // When finding the previous worktree.
        let previous = previous_worktree(&project, &fresh.cwd, Some(fresh.id));

        // Then there is none.
        assert_eq!(previous, None, "root threads aren't a previous worktree");
    }

    #[rstest::rstest]
    fn previous_worktree_from_the_root_without_an_excluded_thread_finds_the_latest() {
        // Given a root thread active latest and two worktree threads.
        let project = project(vec![
            thread(1, ROOT, "main", 90),
            thread(2, "/wt/orb-old", "orb/old", 10),
            thread(3, "/wt/orb-new", "orb/new", 50),
        ]);

        // When finding the previous worktree from the root, excluding no thread.
        let previous = previous_worktree(&project, Path::new(ROOT), None);

        // Then it's the most recently active worktree thread's directory and branch.
        assert_eq!(
            previous,
            Some((PathBuf::from("/wt/orb-new"), Some("orb/new".to_owned()))),
            "a draft should find the latest worktree thread"
        );
    }
}
