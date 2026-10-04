//! Sessions — the sessions orb started, in one list across projects.
//!
//! Each thread in the sidebar is one background session. This feature
//! tracks which threads exist, what each is doing, and which one is selected.
//! The user can pin a thread to the top, settle it onto the Settled shelf
//! (which stops its session), give it a name of orb's own, or delete it. A
//! search narrows the sidebar to the threads and drafts whose title matches
//! what the user types, and ends with the cursor on the match or back where
//! it was.
//!
//! Threads can work together in a group: a Feature group shares one worktree
//! on a branch named after it, and a Research or Learn group shares a folder
//! under orb's own directory, copied from the user's template for the kind.
//!
//! When a harness refuses a folder the user hasn't trusted, orb can record
//! the user's trust in the harness's own config.

pub mod child_env;
pub mod session_host;
pub mod sessions_actor;
pub mod state;
pub mod store;
pub mod template;
pub mod transcript;
pub mod validator;
