//! Transcript preview — the selected thread's conversation, read from its
//! Claude transcript without starting a process.
//!
//! Claude's transcript lines are decoded leniently and rebuilt into the
//! newest branch of the conversation, shown as blocks: the user's prompts,
//! Claude's replies and thinking, tool calls with their results, and notable
//! system messages.

pub mod block;
pub mod conversation;
pub mod state;
pub mod validator;
