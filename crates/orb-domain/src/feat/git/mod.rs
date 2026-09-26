//! Git — the repositories behind each project's threads.
//!
//! orb runs `git` to put a thread in its own worktree, to list and check out
//! branches, and to rename a worktree's branch once its thread has a title.

pub mod git_cli;
pub mod git_service;
pub mod validator;
pub mod worktree;
