//! The plate — one list of what needs the user, across all of orb.
//!
//! Agents add items to it, change them, close them and list them through the
//! control API. Each item names the user's next step with a verb, says what
//! happened, carries its links, and remembers the session and pane it came
//! from. Items outlive their sessions: settling or deleting a session leaves
//! its items on the plate.

pub(crate) mod api;
pub mod state;
pub(crate) mod validator;

pub use state::{
    ItemState, Link, LinkKind, NewPlateItem, Plate, PlateChange, PlateItem, PlateRefusal, Verb,
    local_midnight,
};
