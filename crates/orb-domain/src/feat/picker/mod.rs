//! Picker — a popup list the user narrows by typing and picks one item from.
//!
//! The project picker (`␣n`) lists orb's projects to make a session in. The
//! directory picker (`␣p`) browses the filesystem from `~/` to add a project.
//! The session picker (`␣␣`) lists sessions newest chat first to jump into one.
//! Typed text ranks the items by fuzzy score and marks where it matched.

pub mod list;
pub mod state;
pub mod validator;
