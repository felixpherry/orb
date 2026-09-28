//! The `git` command line as orb's [`Git`].
//!
//! Every `git` process runs against the directory it's given (`git -C`), with
//! orb's child environment, no stdin, and no terminal prompts, so a remote
//! that wants credentials fails instead of drawing over orb. When git fails,
//! the first line it printed to stderr becomes the reason.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use error_stack::Report;

use super::git_service::{Git, GitError, GitRef};

/// What `git fetch` prints when the remote lacks the branch.
const MISSING_REMOTE_REF: &str = "couldn't find remote ref";

/// Runs the `git` on `PATH`.
#[derive(Debug, Clone)]
pub struct GitCli {
    /// The environment every `git` process gets.
    env: Vec<(OsString, OsString)>,
}

impl GitCli {
    /// A git whose processes run with exactly `env`.
    pub fn new(env: Vec<(OsString, OsString)>) -> Self {
        Self { env }
    }

    fn output<I, S>(&self, dir: &Path, args: I) -> Result<Output, Report<GitError>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env_clear()
            .envs(self.env.iter().map(|(key, value)| (key, value)))
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .output()
            .map_err(|error| {
                let reason = format!("couldn't run git: {error}");
                Report::new(error).change_context(GitError).attach(reason)
            })
    }

    /// Runs git in `dir`; returns its stdout.
    fn run<I, S>(&self, dir: &Path, args: I) -> Result<String, Report<GitError>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<OsString> = args
            .into_iter()
            .map(|arg| arg.as_ref().to_owned())
            .collect();
        let output = self.output(dir, &args)?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            let subcommand = args
                .first()
                .map_or_else(String::new, |arg| arg.to_string_lossy().into_owned());
            Err(failure(&output, &subcommand))
        }
    }

    /// Whether git exits 0 in `dir`.
    fn succeeds<I, S>(&self, dir: &Path, args: I) -> bool
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.output(dir, args)
            .is_ok_and(|output| output.status.success())
    }

    /// `origin/HEAD`'s branch.
    fn origin_head(&self, repo: &Path) -> Option<String> {
        let target = self
            .run(repo, ["symbolic-ref", "refs/remotes/origin/HEAD"])
            .ok()?;
        non_empty(target.trim().strip_prefix("refs/remotes/origin/")?)
    }

    /// The branch checked out in `dir`; none on a detached HEAD.
    fn current_branch(&self, dir: &Path) -> Option<String> {
        non_empty(self.run(dir, ["branch", "--show-current"]).ok()?.trim())
    }
}

impl Git for GitCli {
    fn name(&self) -> &'static str {
        "git"
    }

    fn refs(&self, cwd: &Path) -> Result<Vec<GitRef>, Report<GitError>> {
        let listed = self.run(
            cwd,
            [
                "for-each-ref",
                "--format=%(refname)%09%(committerdate:unix)%09%(symref)",
                "refs/heads",
                "refs/remotes",
            ],
        )?;
        let worktrees = checked_out(&self.run(cwd, ["worktree", "list", "--porcelain", "-z"])?);
        Ok(ordered(
            &listed,
            self.current_branch(cwd).as_deref(),
            self.origin_head(cwd).as_deref(),
            &worktrees,
        ))
    }

    fn default_branch(&self, repo: &Path) -> Result<String, Report<GitError>> {
        self.origin_head(repo)
            .or_else(|| self.current_branch(repo))
            .ok_or_else(|| {
                Report::new(GitError).attach("no branch to start a worktree from".to_owned())
            })
    }

    fn has_origin(&self, repo: &Path) -> bool {
        self.succeeds(repo, ["remote", "get-url", "origin"])
    }

    fn fetch(&self, repo: &Path, branch: &str) -> Result<bool, Report<GitError>> {
        let refspec = format!("+refs/heads/{branch}:refs/remotes/origin/{branch}");
        let output = self.output(repo, ["fetch", "--quiet", "origin", &refspec])?;
        if output.status.success() {
            Ok(true)
        } else if String::from_utf8_lossy(&output.stderr).contains(MISSING_REMOTE_REF) {
            Ok(false)
        } else {
            Err(failure(&output, "fetch"))
        }
    }

    fn add_worktree(
        &self,
        repo: &Path,
        path: &Path,
        branch: &str,
        base: &str,
    ) -> Result<(), Report<GitError>> {
        let args = ["worktree", "add", "-b", branch]
            .map(OsStr::new)
            .into_iter()
            .chain([path.as_os_str(), OsStr::new(base)]);
        self.run(repo, args)?;
        Ok(())
    }

    fn remove_worktree(
        &self,
        repo: &Path,
        path: &Path,
        force: bool,
    ) -> Result<(), Report<GitError>> {
        let args = ["worktree", "remove"]
            .into_iter()
            .chain(force.then_some("--force"))
            .map(OsStr::new)
            .chain([path.as_os_str()]);
        self.run(repo, args)?;
        Ok(())
    }

    fn delete_branch(
        &self,
        repo: &Path,
        branch: &str,
        force: bool,
    ) -> Result<(), Report<GitError>> {
        let flag = if force { "-D" } else { "-d" };
        self.run(repo, ["branch", flag, branch])?;
        Ok(())
    }

    fn branch_exists(&self, repo: &Path, branch: &str) -> bool {
        let refname = format!("refs/heads/{branch}");
        self.succeeds(repo, ["show-ref", "--verify", "--quiet", &refname])
    }

    fn is_merged(&self, repo: &Path, branch: &str) -> bool {
        let upstream = format!("{branch}@{{upstream}}");
        let target = if self.succeeds(repo, ["rev-parse", "--verify", "--quiet", &upstream]) {
            upstream
        } else {
            "HEAD".to_owned()
        };
        let refname = format!("refs/heads/{branch}");
        self.succeeds(repo, ["merge-base", "--is-ancestor", &refname, &target])
    }

    fn has_remote_branch(&self, repo: &Path, branch: &str) -> bool {
        let refname = format!("refs/remotes/origin/{branch}");
        self.succeeds(repo, ["show-ref", "--verify", "--quiet", &refname])
    }

    fn init(&self, dir: &Path) -> Result<(), Report<GitError>> {
        self.run(dir, ["init", "--quiet"])?;
        Ok(())
    }

    fn checkout(&self, cwd: &Path, git_ref: &GitRef) -> Result<String, Report<GitError>> {
        let name = git_ref.name.as_str();
        match (git_ref.remote, name.split_once('/')) {
            (true, Some((_, branch))) => {
                self.run(cwd, ["checkout", "--track", name, "--"])?;
                Ok(branch.to_owned())
            }
            _ => {
                self.run(cwd, ["checkout", name, "--"])?;
                Ok(name.to_owned())
            }
        }
    }

    fn rename_branch(&self, cwd: &Path, old: &str, new: &str) -> Result<(), Report<GitError>> {
        self.run(cwd, ["branch", "-m", old, new])?;
        Ok(())
    }
}

/// A report whose reason is git's first stderr line, else
/// `git <subcommand> failed`.
fn failure(output: &Output, subcommand: &str) -> Report<GitError> {
    let reason = String::from_utf8_lossy(&output.stderr)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map_or_else(|| format!("git {subcommand} failed"), str::to_owned);
    Report::new(GitError).attach(reason)
}

fn non_empty(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_owned())
}

/// One entry of `git worktree list --porcelain`.
struct Worktree {
    path: PathBuf,
    branch: Option<String>,
    prunable: bool,
}

/// Each branch checked out in a worktree that's still on disk, with its path.
fn checked_out(porcelain: &str) -> Vec<(String, PathBuf)> {
    let mut worktrees: Vec<Worktree> = Vec::new();
    for field in porcelain.split('\0') {
        match field.split_once(' ').unwrap_or((field, "")) {
            ("worktree", path) => worktrees.push(Worktree {
                path: PathBuf::from(path),
                branch: None,
                prunable: false,
            }),
            ("branch", refname) => {
                if let Some(worktree) = worktrees.last_mut() {
                    worktree.branch = refname.strip_prefix("refs/heads/").map(str::to_owned);
                }
            }
            ("prunable", _) => {
                if let Some(worktree) = worktrees.last_mut() {
                    worktree.prunable = true;
                }
            }
            _ => {}
        }
    }
    worktrees
        .into_iter()
        .filter(|worktree| !worktree.prunable && worktree.path.exists())
        .filter_map(|worktree| Some((worktree.branch?, worktree.path)))
        .collect()
}

/// The refs in `for-each-ref`'s `listed` output, in T3's order: current,
/// default, other local branches newest first, then remote refs newest first.
/// A current branch with no commit yet is listed too.
/// Symbolic refs (`origin/HEAD`) and `origin/<b>` where a local `<b>` exists
/// are left out.
fn ordered(
    listed: &str,
    current: Option<&str>,
    default: Option<&str>,
    worktrees: &[(String, PathBuf)],
) -> Vec<GitRef> {
    let (mut locals, mut remotes) = listed
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let (refname, time, symref) = (fields.next()?, fields.next()?, fields.next()?);
            if !symref.is_empty() {
                return None;
            }
            let time = time.parse().unwrap_or_default();
            match (
                refname.strip_prefix("refs/heads/"),
                refname.strip_prefix("refs/remotes/"),
            ) {
                (Some(name), _) => Some((
                    time,
                    GitRef {
                        name: name.to_owned(),
                        remote: false,
                        current: current == Some(name),
                        default: default == Some(name),
                        worktree: worktrees
                            .iter()
                            .find(|(branch, _)| branch == name)
                            .map(|(_, path)| path.clone()),
                    },
                )),
                (None, Some(name)) => Some((
                    time,
                    GitRef {
                        name: name.to_owned(),
                        remote: true,
                        current: false,
                        default: default.is_some_and(|default| {
                            name.split_once('/') == Some(("origin", default))
                        }),
                        worktree: None,
                    },
                )),
                (None, None) => None,
            }
        })
        .partition::<Vec<(i64, GitRef)>, _>(|(_, git_ref)| !git_ref.remote);
    // A new repository's branch has no commit yet, so `for-each-ref` skips it.
    if let Some(unborn) = current.filter(|name| !locals.iter().any(|(_, l)| l.name == *name)) {
        locals.push((
            0,
            GitRef {
                name: unborn.to_owned(),
                remote: false,
                current: true,
                default: default == Some(unborn),
                worktree: worktrees
                    .iter()
                    .find(|(branch, _)| branch == unborn)
                    .map(|(_, path)| path.clone()),
            },
        ));
    }
    remotes.retain(|(_, remote)| {
        remote
            .name
            .strip_prefix("origin/")
            .is_none_or(|branch| !locals.iter().any(|(_, local)| local.name == branch))
    });
    for refs in [&mut locals, &mut remotes] {
        refs.sort_by(|(time_a, a), (time_b, b)| {
            time_b.cmp(time_a).then_with(|| a.name.cmp(&b.name))
        });
    }
    let mut refs: Vec<GitRef> = locals
        .into_iter()
        .chain(remotes)
        .map(|(_, git_ref)| git_ref)
        .collect();
    refs.sort_by_key(|git_ref| match (git_ref.current, git_ref.default) {
        (true, _) => 0,
        (false, true) => 1,
        (false, false) => 2,
    });
    refs
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests set repos up with `?` and assert on the outcome"
)]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::path::{Path, PathBuf};

    use error_stack::{Report, ResultExt};
    use tempfile::TempDir;

    use super::GitCli;
    use crate::feat::git::git_service::{Git, GitError, GitRef};

    /// A home directory for test repos, and a git that ignores the user's
    /// config.
    struct Sandbox {
        home: TempDir,
        env: Vec<(OsString, OsString)>,
    }

    impl Sandbox {
        fn new() -> Result<Self, Report<GitError>> {
            let home = TempDir::new().change_context(GitError)?;
            let env = vec![
                ("PATH".into(), std::env::var_os("PATH").unwrap_or_default()),
                ("HOME".into(), home.path().into()),
                ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
                ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ];
            Ok(Self { home, env })
        }

        fn git(&self) -> GitCli {
            GitCli::new(self.env.clone())
        }

        fn path(&self, name: &str) -> PathBuf {
            self.home.path().join(name)
        }

        fn run<I, S>(&self, dir: &Path, args: I) -> Result<(), Report<GitError>>
        where
            I: IntoIterator<Item = S>,
            S: AsRef<OsStr>,
        {
            self.git().run(dir, args).map(drop)
        }

        /// A new repo `name` on `branch`, with one commit.
        fn repo(&self, name: &str, branch: &str) -> Result<PathBuf, Report<GitError>> {
            let path = self.path(name);
            self.run(self.home.path(), ["init", "-q", "-b", branch, name])?;
            self.run(&path, ["config", "user.name", "orb"])?;
            self.run(&path, ["config", "user.email", "orb@example.com"])?;
            self.commit(&path, 1000)?;
            Ok(path)
        }

        /// An empty commit in `repo` dated `time` (unix seconds).
        fn commit(&self, repo: &Path, time: u32) -> Result<(), Report<GitError>> {
            let date = OsString::from(format!("@{time} +0000"));
            let env = self
                .env
                .iter()
                .cloned()
                .chain([
                    ("GIT_COMMITTER_DATE".into(), date.clone()),
                    ("GIT_AUTHOR_DATE".into(), date),
                ])
                .collect();
            GitCli::new(env)
                .run(repo, ["commit", "-q", "--allow-empty", "-m", "commit"])
                .map(drop)
        }

        /// Adds `url` as the remote `name` of `repo` and fetches it.
        fn add_remote(&self, repo: &Path, name: &str, url: &Path) -> Result<(), Report<GitError>> {
            self.run(
                repo,
                [
                    OsStr::new("remote"),
                    OsStr::new("add"),
                    OsStr::new(name),
                    url.as_os_str(),
                ],
            )?;
            self.run(repo, ["fetch", "-q", name])
        }
    }

    fn names(refs: &[GitRef]) -> Vec<&str> {
        refs.iter().map(|git_ref| git_ref.name.as_str()).collect()
    }

    fn named<'a>(refs: &'a [GitRef], name: &str) -> Option<&'a GitRef> {
        refs.iter().find(|git_ref| git_ref.name == name)
    }

    fn reason<T>(result: Result<T, Report<GitError>>) -> Option<String> {
        result.err()?.downcast_ref::<String>().cloned()
    }

    #[rstest::rstest]
    fn refs_put_current_then_default_then_locals_by_recency_then_remotes()
    -> Result<(), Report<GitError>> {
        // Given a repo on `feature`, whose origin's HEAD is `main`, with older
        // and newer local branches and two remote-only branches.
        let sandbox = Sandbox::new()?;
        let repo = sandbox.repo("repo", "main")?;
        sandbox.run(&repo, ["checkout", "-q", "-b", "old"])?;
        sandbox.commit(&repo, 2000)?;
        sandbox.run(&repo, ["checkout", "-q", "-b", "new", "main"])?;
        sandbox.commit(&repo, 3000)?;
        sandbox.run(&repo, ["checkout", "-q", "-b", "feature", "main"])?;
        sandbox.commit(&repo, 1500)?;
        let origin = sandbox.repo("origin", "main")?;
        sandbox.run(&origin, ["checkout", "-q", "-b", "r1"])?;
        sandbox.commit(&origin, 4000)?;
        sandbox.run(&origin, ["checkout", "-q", "-b", "r2"])?;
        sandbox.commit(&origin, 5000)?;
        sandbox.add_remote(&repo, "origin", &origin)?;
        sandbox.run(&repo, ["remote", "set-head", "origin", "main"])?;

        // When listing the refs.
        let refs = sandbox.git().refs(&repo)?;

        // Then the current branch leads, then the default, then the other
        // locals newest first, then the remotes newest first.
        assert_eq!(
            names(&refs),
            ["feature", "main", "new", "old", "origin/r2", "origin/r1"],
            "refs should be in T3's order"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn refs_hide_origin_ref_matching_a_local_branch() -> Result<(), Report<GitError>> {
        // Given a local `main` and an origin with `main`.
        let sandbox = Sandbox::new()?;
        let repo = sandbox.repo("repo", "main")?;
        let origin = sandbox.repo("origin", "main")?;
        sandbox.add_remote(&repo, "origin", &origin)?;

        // When listing the refs.
        let refs = sandbox.git().refs(&repo)?;

        // Then `origin/main` isn't listed.
        assert!(
            named(&refs, "origin/main").is_none(),
            "origin/main should be hidden behind the local main, got {:?}",
            names(&refs)
        );
        Ok(())
    }

    #[rstest::rstest]
    fn refs_keep_non_origin_remote_refs() -> Result<(), Report<GitError>> {
        // Given a local `main` and an `upstream` remote with `main`.
        let sandbox = Sandbox::new()?;
        let repo = sandbox.repo("repo", "main")?;
        let upstream = sandbox.repo("upstream", "main")?;
        sandbox.add_remote(&repo, "upstream", &upstream)?;

        // When listing the refs.
        let refs = sandbox.git().refs(&repo)?;

        // Then `upstream/main` is listed.
        assert!(
            named(&refs, "upstream/main").is_some(),
            "upstream/main should be kept, got {:?}",
            names(&refs)
        );
        Ok(())
    }

    #[rstest::rstest]
    fn refs_drop_origin_head() -> Result<(), Report<GitError>> {
        // Given an origin whose HEAD is set.
        let sandbox = Sandbox::new()?;
        let repo = sandbox.repo("repo", "main")?;
        let origin = sandbox.repo("origin", "main")?;
        sandbox.add_remote(&repo, "origin", &origin)?;
        sandbox.run(&repo, ["remote", "set-head", "origin", "main"])?;

        // When listing the refs.
        let refs = sandbox.git().refs(&repo)?;

        // Then `origin/HEAD` isn't listed.
        assert!(
            named(&refs, "origin/HEAD").is_none(),
            "origin/HEAD should be dropped, got {:?}",
            names(&refs)
        );
        Ok(())
    }

    #[rstest::rstest]
    fn refs_mark_a_branch_checked_out_in_another_worktree() -> Result<(), Report<GitError>> {
        // Given `side` checked out in a second worktree.
        let sandbox = Sandbox::new()?;
        let repo = sandbox.repo("repo", "main")?;
        let worktree = sandbox.path("worktree");
        sandbox
            .git()
            .add_worktree(&repo, &worktree, "side", "main")?;

        // When listing the refs from the root checkout.
        let refs = sandbox.git().refs(&repo)?;

        // Then `side` names that worktree.
        let expected = worktree.canonicalize().change_context(GitError)?;
        assert_eq!(
            named(&refs, "side")
                .and_then(|side| side.worktree.as_deref())
                .and_then(|path| path.canonicalize().ok()),
            Some(expected),
            "side should be marked as checked out in the worktree"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn refs_skip_a_worktree_whose_directory_is_gone() -> Result<(), Report<GitError>> {
        // Given `side`'s worktree directory deleted without git knowing.
        let sandbox = Sandbox::new()?;
        let repo = sandbox.repo("repo", "main")?;
        let worktree = sandbox.path("worktree");
        sandbox
            .git()
            .add_worktree(&repo, &worktree, "side", "main")?;
        fs::remove_dir_all(&worktree).change_context(GitError)?;

        // When listing the refs.
        let refs = sandbox.git().refs(&repo)?;

        // Then `side` isn't marked as checked out anywhere.
        assert_eq!(
            named(&refs, "side").map(|side| side.worktree.clone()),
            Some(None),
            "a missing worktree shouldn't claim its branch"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn default_branch_reads_origin_head() -> Result<(), Report<GitError>> {
        // Given a repo on `main` whose origin's HEAD is `trunk`.
        let sandbox = Sandbox::new()?;
        let repo = sandbox.repo("repo", "main")?;
        let origin = sandbox.repo("origin", "trunk")?;
        sandbox.add_remote(&repo, "origin", &origin)?;
        sandbox.run(&repo, ["remote", "set-head", "origin", "trunk"])?;

        // When asking for the default branch.
        let default = sandbox.git().default_branch(&repo)?;

        // Then it's origin's HEAD.
        assert_eq!(
            default, "trunk",
            "the default should be origin/HEAD's branch"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn default_branch_falls_back_to_the_current_branch() -> Result<(), Report<GitError>> {
        // Given a repo without an origin, on `feature`.
        let sandbox = Sandbox::new()?;
        let repo = sandbox.repo("repo", "main")?;
        sandbox.run(&repo, ["checkout", "-q", "-b", "feature"])?;

        // When asking for the default branch.
        let default = sandbox.git().default_branch(&repo)?;

        // Then it's the current branch.
        assert_eq!(
            default, "feature",
            "the default should fall back to the current branch"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn refs_list_the_branch_of_a_repository_with_no_commits() -> Result<(), Report<GitError>> {
        // Given a repository just made with `git init`, on the unborn `trunk`.
        let sandbox = Sandbox::new()?;
        let repo = sandbox.path("fresh");
        fs::create_dir_all(&repo).change_context(GitError)?;
        sandbox.run(&repo, ["init", "-q", "-b", "trunk"])?;

        // When listing the refs.
        let refs = sandbox.git().refs(&repo)?;

        // Then `trunk` is the one, current branch.
        assert_eq!(
            refs.iter()
                .map(|git_ref| (git_ref.name.as_str(), git_ref.current))
                .collect::<Vec<_>>(),
            [("trunk", true)],
            "the unborn branch should be listed as current"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn init_makes_a_directory_a_repository() -> Result<(), Report<GitError>> {
        // Given a plain directory.
        let sandbox = Sandbox::new()?;
        let dir = sandbox.path("plain");
        fs::create_dir_all(&dir).change_context(GitError)?;

        // When initializing git there.
        sandbox.git().init(&dir)?;

        // Then git can list its refs.
        assert!(
            sandbox.git().refs(&dir).is_ok(),
            "the directory should be a repository"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn refs_fail_outside_a_repository() -> Result<(), Report<GitError>> {
        // Given a plain directory.
        let sandbox = Sandbox::new()?;
        let dir = sandbox.path("plain");
        fs::create_dir_all(&dir).change_context(GitError)?;

        // When listing its refs.
        let refs = sandbox.git().refs(&dir);

        // Then git refuses.
        assert!(refs.is_err(), "a plain directory has no refs");
        Ok(())
    }

    #[rstest::rstest]
    #[case("main", true)]
    #[case("local-only", false)]
    fn has_remote_branch_tells_whether_origin_had_it_at_the_last_fetch(
        #[case] branch: &str,
        #[case] expected: bool,
    ) -> Result<(), Report<GitError>> {
        // Given a clone-like repo that fetched origin's `main`, and a branch
        // only it has.
        let sandbox = Sandbox::new()?;
        let repo = sandbox.repo("repo", "main")?;
        sandbox.run(&repo, ["branch", "local-only"])?;
        let origin = sandbox.repo("origin", "main")?;
        sandbox.add_remote(&repo, "origin", &origin)?;

        // When asking whether origin has `branch`.
        let known = sandbox.git().has_remote_branch(&repo, branch);

        // Then only origin's branch is known.
        assert_eq!(known, expected, "origin/{branch} known");
        Ok(())
    }

    /// A repo on `main` with a branch `slug` one commit ahead of it.
    fn slug_ahead(sandbox: &Sandbox) -> Result<PathBuf, Report<GitError>> {
        let repo = sandbox.repo("repo", "main")?;
        sandbox.run(&repo, ["checkout", "-q", "-b", "slug"])?;
        sandbox.commit(&repo, 2000)?;
        sandbox.run(&repo, ["checkout", "-q", "main"])?;
        Ok(repo)
    }

    #[rstest::rstest]
    fn branch_with_a_commit_head_lacks_is_not_merged() -> Result<(), Report<GitError>> {
        // Given `slug` one commit ahead of HEAD, with no upstream.
        let sandbox = Sandbox::new()?;
        let repo = slug_ahead(&sandbox)?;

        // When asking whether it's merged.
        let merged = sandbox.git().is_merged(&repo, "slug");

        // Then it isn't, as `git branch -d` says.
        assert!(!merged, "a commit only slug has keeps it unmerged");
        Ok(())
    }

    #[rstest::rstest]
    fn branch_in_heads_history_is_merged() -> Result<(), Report<GitError>> {
        // Given `slug` one commit ahead, then fast-forwarded into HEAD.
        let sandbox = Sandbox::new()?;
        let repo = slug_ahead(&sandbox)?;
        sandbox.run(&repo, ["merge", "-q", "--ff-only", "slug"])?;

        // When asking whether it's merged.
        let merged = sandbox.git().is_merged(&repo, "slug");

        // Then it is.
        assert!(merged, "slug's commits are all in HEAD");
        Ok(())
    }

    #[rstest::rstest]
    fn branch_in_its_upstreams_history_is_merged() -> Result<(), Report<GitError>> {
        // Given `slug` one commit ahead of HEAD, pushed to its upstream.
        let sandbox = Sandbox::new()?;
        let repo = slug_ahead(&sandbox)?;
        let origin = sandbox.repo("origin", "main")?;
        sandbox.add_remote(&repo, "origin", &origin)?;
        sandbox.run(&repo, ["push", "-q", "-u", "origin", "slug"])?;

        // When asking whether it's merged.
        let merged = sandbox.git().is_merged(&repo, "slug");

        // Then it is, as `git branch -d` deletes it.
        assert!(merged, "a branch its upstream has is merged");
        Ok(())
    }

    #[rstest::rstest]
    fn branch_ahead_of_its_upstream_is_not_merged_even_in_head() -> Result<(), Report<GitError>> {
        // Given `slug` pushed to its upstream, then a commit on top, merged
        // into HEAD.
        let sandbox = Sandbox::new()?;
        let repo = slug_ahead(&sandbox)?;
        let origin = sandbox.repo("origin", "main")?;
        sandbox.add_remote(&repo, "origin", &origin)?;
        sandbox.run(&repo, ["push", "-q", "-u", "origin", "slug"])?;
        sandbox.run(&repo, ["checkout", "-q", "slug"])?;
        sandbox.commit(&repo, 3000)?;
        sandbox.run(&repo, ["checkout", "-q", "main"])?;
        sandbox.run(&repo, ["merge", "-q", "--ff-only", "slug"])?;

        // When asking whether it's merged.
        let merged = sandbox.git().is_merged(&repo, "slug");

        // Then it isn't: `git branch -d` checks the upstream first.
        assert!(!merged, "the upstream lacks slug's last commit");
        Ok(())
    }

    #[rstest::rstest]
    fn fetch_reports_a_branch_missing_on_origin() -> Result<(), Report<GitError>> {
        // Given an origin without `nope`.
        let sandbox = Sandbox::new()?;
        let repo = sandbox.repo("repo", "main")?;
        let origin = sandbox.repo("origin", "main")?;
        sandbox.add_remote(&repo, "origin", &origin)?;

        // When fetching `nope`.
        let fetched = sandbox.git().fetch(&repo, "nope")?;

        // Then the fetch reports the branch missing.
        assert!(!fetched, "a branch origin lacks should fetch as false");
        Ok(())
    }

    #[rstest::rstest]
    fn fetch_fails_with_gits_reason_for_a_bad_remote() -> Result<(), Report<GitError>> {
        // Given an origin pointing at a directory that doesn't exist.
        let sandbox = Sandbox::new()?;
        let repo = sandbox.repo("repo", "main")?;
        let missing = sandbox.path("missing");
        sandbox.run(
            &repo,
            [
                OsStr::new("remote"),
                OsStr::new("add"),
                OsStr::new("origin"),
                missing.as_os_str(),
            ],
        )?;

        // When fetching `main`.
        let reason = reason(sandbox.git().fetch(&repo, "main"));

        // Then the fetch fails with git's first line.
        assert!(
            reason
                .as_deref()
                .is_some_and(|reason| reason.contains("does not appear to be a git repository")),
            "the reason should be git's, got {reason:?}"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn checkout_of_a_remote_ref_creates_a_tracking_branch() -> Result<(), Report<GitError>> {
        // Given an origin with a `feature` branch the repo doesn't have.
        let sandbox = Sandbox::new()?;
        let repo = sandbox.repo("repo", "main")?;
        let origin = sandbox.repo("origin", "main")?;
        sandbox.run(&origin, ["branch", "feature"])?;
        sandbox.add_remote(&repo, "origin", &origin)?;
        let remote = named(&sandbox.git().refs(&repo)?, "origin/feature")
            .cloned()
            .ok_or_else(|| Report::new(GitError).attach("origin/feature isn't listed"))?;

        // When checking out `origin/feature`.
        sandbox.git().checkout(&repo, &remote)?;

        // Then a local `feature` tracks it.
        let upstream = sandbox
            .git()
            .run(&repo, ["rev-parse", "--abbrev-ref", "feature@{upstream}"])?;
        assert_eq!(
            upstream.trim(),
            "origin/feature",
            "feature should track origin/feature"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn rename_onto_an_existing_branch_fails_and_keeps_the_old_one() -> Result<(), Report<GitError>>
    {
        // Given branches `a` and `b`.
        let sandbox = Sandbox::new()?;
        let repo = sandbox.repo("repo", "main")?;
        sandbox.run(&repo, ["branch", "a"])?;
        sandbox.run(&repo, ["branch", "b"])?;

        // When renaming `a` to `b`.
        let renamed = sandbox.git().rename_branch(&repo, "a", "b");

        // Then the rename fails and `a` is still there.
        assert!(
            renamed.is_err() && sandbox.git().branch_exists(&repo, "a"),
            "renaming onto b should fail and keep a"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn remove_worktree_without_force_refuses_a_dirty_worktree() -> Result<(), Report<GitError>> {
        // Given a worktree with an untracked file.
        let sandbox = Sandbox::new()?;
        let repo = sandbox.repo("repo", "main")?;
        let worktree = sandbox.path("worktree");
        sandbox
            .git()
            .add_worktree(&repo, &worktree, "side", "main")?;
        fs::write(worktree.join("notes.txt"), "draft").change_context(GitError)?;

        // When removing it without force.
        let removed = sandbox.git().remove_worktree(&repo, &worktree, false);

        // Then git refuses.
        assert!(
            removed.is_err(),
            "a dirty worktree shouldn't be removed without force"
        );
        Ok(())
    }
}
