//! The plate — one list of what needs the user, across all of orb.
//!
//! Agents add items to it, change them, close them and list them through the
//! control API. Each item names the user's next step with a verb, says what
//! happened, carries its links, and remembers the session and pane it came
//! from. Items outlive their sessions: settling or deleting a session leaves
//! its items on the plate. orb adds its own row for each agent waiting for
//! an approval or an answer, built from its status and never stored. The user
//! moves through the plate, opens a row's pane with ⏎, and closes, parks or
//! undoes with x, p and u.

pub(crate) mod api;
pub mod board;
pub(crate) mod intent;
pub(crate) mod links;
pub mod opener;
pub mod state;
pub(crate) mod validator;
pub mod view;

pub use board::{Board, BoardRow, Group, GroupKind, OrbAsk, RowKey, RowSource, your_move_count};
pub use intent::PlateKey;
pub use links::BrowserTabs;
pub use opener::{LinkOpener, LinkService};
pub use state::{
    ItemDoneError, ItemState, Link, LinkKind, NewPlateItem, Plate, PlateChange, PlateItem, Verb,
    local_midnight,
};
pub use view::{PlateCursor, PlatePrefix, PlateUndo, PlateView};
