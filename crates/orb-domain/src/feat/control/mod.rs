//! Control — lets an agent (or a script) running inside orb ask orb to do
//! things: list sessions, tabs, panes and agents, and later add panes, type into
//! them and talk to other agents.
//!
//! A running orb listens on a socket only the user can open. The `orb session`,
//! `orb tab`, `orb pane` and `orb agent` commands send it one request each and
//! print its JSON reply.

pub mod client;
pub mod protocol;
pub mod server;
pub(crate) mod target;
