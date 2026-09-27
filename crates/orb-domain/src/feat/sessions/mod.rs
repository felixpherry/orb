//! Sessions — the Claude sessions orb started, in one list across projects.
//!
//! Each thread in the sidebar is one background Claude session. This feature
//! tracks which threads exist, what each is doing, and which one is selected.
//! The user can pin a thread to the top, settle it onto the Settled shelf
//! (which stops its session), give it a name of orb's own, or delete it. A
//! search narrows the sidebar to the threads and drafts whose title matches
//! what the user types, and ends with the cursor on the match or back where
//! it was.

pub mod child_env;
pub mod claude_supervisor;
pub mod session_host;
pub mod sessions_actor;
pub mod state;
pub mod store;
pub mod transcript;
pub mod validator;
