//! What orb asks of git: the branches a thread could switch to, a project's
//! default branch, making, checking out, renaming and removing worktrees and
//! branches, whether a branch is merged, and making a directory a repository.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use error_stack::Report;
use wherror::Error;

/// A git call failed. Every report carries a one-line reason as its latest
/// `String` attachment, fit for the mode line.
#[derive(Debug, Error)]
#[error(debug)]
pub struct GitError;

/// A branch a thread could be on: a local branch or a remote ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRef {
    /// `main` for a local branch, `origin/feature` for a remote ref.
    pub name: String,
    pub remote: bool,
    /// The branch checked out where the refs were listed.
    pub current: bool,
    /// The repository's default branch (`origin/HEAD`'s), local or on origin.
    pub default: bool,
    /// Where a local branch is checked out, the root checkout included.
    pub worktree: Option<PathBuf>,
}

/// Runs git against repositories on disk.
pub trait Git: Send + Sync {
    fn name(&self) -> &'static str;

    /// The refs of the repository containing `cwd`: the current branch, the
    /// default branch, other local branches newest commit first, then remote
    /// refs newest commit first. `origin/<b>` is left out when a local `<b>`
    /// exists.
    ///
    /// # Errors
    ///
    /// Returns an error if git can't list the refs.
    fn refs(&self, cwd: &Path) -> Result<Vec<GitRef>, Report<GitError>>;

    /// `origin/HEAD`'s branch, else the current branch.
    ///
    /// # Errors
    ///
    /// Returns an error if the repository has neither.
    fn default_branch(&self, repo: &Path) -> Result<String, Report<GitError>>;

    /// Whether the repository has an `origin` remote.
    fn has_origin(&self, repo: &Path) -> bool;

    /// Fetches `branch` from origin into `origin/<branch>`. `Ok(false)` when
    /// origin has no such branch.
    ///
    /// # Errors
    ///
    /// Returns an error if the fetch fails for any other reason.
    fn fetch(&self, repo: &Path, branch: &str) -> Result<bool, Report<GitError>>;

    /// Adds a worktree at `path` on a new `branch` started from `base`.
    ///
    /// # Errors
    ///
    /// Returns an error if git refuses, e.g. when `branch` or `path` exists.
    fn add_worktree(
        &self,
        repo: &Path,
        path: &Path,
        branch: &str,
        base: &str,
    ) -> Result<(), Report<GitError>>;

    /// Removes the worktree at `path`. Without `force`, git refuses a dirty
    /// worktree.
    ///
    /// # Errors
    ///
    /// Returns an error if git refuses.
    fn remove_worktree(
        &self,
        repo: &Path,
        path: &Path,
        force: bool,
    ) -> Result<(), Report<GitError>>;

    /// Deletes a local branch. Without `force`, git refuses an unmerged one.
    ///
    /// # Errors
    ///
    /// Returns an error if git refuses.
    fn delete_branch(&self, repo: &Path, branch: &str, force: bool)
    -> Result<(), Report<GitError>>;

    /// Whether the local branch exists.
    fn branch_exists(&self, repo: &Path, branch: &str) -> bool;

    /// Whether `git branch -d` would take the local `branch` as merged: it is
    /// in its upstream's history when it has one, else in `HEAD`'s.
    fn is_merged(&self, repo: &Path, branch: &str) -> bool;

    /// Whether `origin/<branch>` is known, as of the last fetch.
    fn has_remote_branch(&self, repo: &Path, branch: &str) -> bool;

    /// Makes `dir` a new, empty git repository (`git init`).
    ///
    /// # Errors
    ///
    /// Returns an error if git refuses.
    fn init(&self, dir: &Path) -> Result<(), Report<GitError>>;

    /// Checks out `git_ref` in `cwd`; a remote ref gets a local branch that
    /// tracks it. Returns the local branch's name.
    ///
    /// # Errors
    ///
    /// Returns an error if git refuses, e.g. over uncommitted changes.
    fn checkout(&self, cwd: &Path, git_ref: &GitRef) -> Result<String, Report<GitError>>;

    /// Renames the local branch `old` to `new`.
    ///
    /// # Errors
    ///
    /// Returns an error if git refuses, e.g. when `new` exists.
    fn rename_branch(&self, cwd: &Path, old: &str, new: &str) -> Result<(), Report<GitError>>;
}

/// The one-line reason a git failure carries.
pub fn git_reason(report: &Report<GitError>) -> String {
    report
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| "git failed".to_owned())
}

/// Shared handle to the [`Git`] in use.
#[derive(Clone)]
pub struct GitService {
    git: Arc<dyn Git>,
}

impl GitService {
    pub fn new(git: Arc<dyn Git>) -> Self {
        Self { git }
    }

    /// The refs of the repository containing `cwd`, in the order [`Git::refs`]
    /// gives.
    ///
    /// # Errors
    ///
    /// Returns an error if git can't list the refs.
    pub fn refs(&self, cwd: &Path) -> Result<Vec<GitRef>, Report<GitError>> {
        self.git.refs(cwd)
    }

    /// `origin/HEAD`'s branch, else the current branch.
    ///
    /// # Errors
    ///
    /// Returns an error if the repository has neither.
    pub fn default_branch(&self, repo: &Path) -> Result<String, Report<GitError>> {
        self.git.default_branch(repo)
    }

    /// Whether the repository has an `origin` remote.
    pub fn has_origin(&self, repo: &Path) -> bool {
        self.git.has_origin(repo)
    }

    /// Fetches `branch` from origin. `Ok(false)` when origin has no such branch.
    ///
    /// # Errors
    ///
    /// Returns an error if the fetch fails for any other reason.
    pub fn fetch(&self, repo: &Path, branch: &str) -> Result<bool, Report<GitError>> {
        self.git.fetch(repo, branch)
    }

    /// Adds a worktree at `path` on a new `branch` started from `base`.
    ///
    /// # Errors
    ///
    /// Returns an error if git refuses.
    pub fn add_worktree(
        &self,
        repo: &Path,
        path: &Path,
        branch: &str,
        base: &str,
    ) -> Result<(), Report<GitError>> {
        self.git.add_worktree(repo, path, branch, base)
    }

    /// Removes the worktree at `path`.
    ///
    /// # Errors
    ///
    /// Returns an error if git refuses.
    pub fn remove_worktree(
        &self,
        repo: &Path,
        path: &Path,
        force: bool,
    ) -> Result<(), Report<GitError>> {
        self.git.remove_worktree(repo, path, force)
    }

    /// Deletes a local branch.
    ///
    /// # Errors
    ///
    /// Returns an error if git refuses.
    pub fn delete_branch(
        &self,
        repo: &Path,
        branch: &str,
        force: bool,
    ) -> Result<(), Report<GitError>> {
        self.git.delete_branch(repo, branch, force)
    }

    /// Whether the local branch exists.
    pub fn branch_exists(&self, repo: &Path, branch: &str) -> bool {
        self.git.branch_exists(repo, branch)
    }

    /// Whether `git branch -d` would take the local `branch` as merged.
    pub fn is_merged(&self, repo: &Path, branch: &str) -> bool {
        self.git.is_merged(repo, branch)
    }

    /// Whether `origin/<branch>` is known, as of the last fetch.
    pub fn has_remote_branch(&self, repo: &Path, branch: &str) -> bool {
        self.git.has_remote_branch(repo, branch)
    }

    /// Makes `dir` a new, empty git repository.
    ///
    /// # Errors
    ///
    /// Returns an error if git refuses.
    pub fn init(&self, dir: &Path) -> Result<(), Report<GitError>> {
        self.git.init(dir)
    }

    /// Checks out `git_ref` in `cwd`. Returns the local branch's name.
    ///
    /// # Errors
    ///
    /// Returns an error if git refuses.
    pub fn checkout(&self, cwd: &Path, git_ref: &GitRef) -> Result<String, Report<GitError>> {
        self.git.checkout(cwd, git_ref)
    }

    /// Renames the local branch `old` to `new`.
    ///
    /// # Errors
    ///
    /// Returns an error if git refuses.
    pub fn rename_branch(&self, cwd: &Path, old: &str, new: &str) -> Result<(), Report<GitError>> {
        self.git.rename_branch(cwd, old, new)
    }
}

impl fmt::Debug for GitService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Git<{}>", self.git.name())
    }
}
