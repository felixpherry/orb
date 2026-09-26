//! Picker — a popup list the user narrows by typing and picks one item from.
//!
//! The project picker (`␣n`) lists orb's projects to start a session in. The
//! directory picker (`␣p`) browses the filesystem from `~/` to add a project.
//! Typed text ranks the items by fuzzy score and marks where it matched.

pub mod list;
pub mod state;
pub mod validator;
