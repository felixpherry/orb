//! Sessions — the Claude sessions orb started, grouped by project.
//!
//! Each thread in the sidebar is one background Claude session. This feature
//! tracks which threads exist, what each is doing, and which one is selected.

pub mod child_env;
pub mod claude_supervisor;
pub mod session_host;
pub mod state;
pub mod store;
pub mod transcript;
pub mod validator;
