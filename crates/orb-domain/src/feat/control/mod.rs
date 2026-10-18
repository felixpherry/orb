//! Control — lets an agent (or a script) running inside orb ask orb to do
//! things: list sessions, tabs, panes and agents, add and close panes, tabs and
//! sessions, type into panes and read them, start, prompt and read other
//! agents, and add, change, close and list items on the user's plate.
//!
//! A running orb listens on a socket only the user can open. The `orb session`,
//! `orb tab`, `orb pane`, `orb agent` and `orb plate` commands send it one
//! request each and print its JSON reply. `orb help` and `--help` explain each
//! command, and a refusal carries a hint naming the command that fixes it.
//! `orb integration install` puts a skill in Claude's and pi's folders that
//! teaches agents these commands. A second socket answers terminal-browser, so
//! `terminal-browser open --split` opens an orb pane.

pub(crate) mod agent;
pub mod browser_socket;
pub mod change;
pub mod client;
pub(crate) mod help;
pub mod pane_control;
pub mod protocol;
pub mod server;
pub(crate) mod target;
pub(crate) mod validator;

pub use agent::quoted;
