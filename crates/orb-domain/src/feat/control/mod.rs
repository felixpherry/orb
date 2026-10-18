//! Control — lets an agent (or a script) running inside orb ask orb to do
//! things: list sessions, tabs, panes, agents and projects, add and close panes, tabs and
//! sessions, type into panes and read them, start, prompt and read other
//! agents, and add, change, close and list items on the user's plate.
//!
//! A running orb listens on a socket only the user can open. The `orb session`,
//! `orb tab`, `orb pane`, `orb agent`, `orb plate` and `orb project` commands send it one
//! request each and print its reply: JSON for data, plain text for a pane's
//! screen and an agent's answer. `orb help` and `--help` explain each
//! command, and a refusal carries a hint naming the command that fixes it.
//! `orb integration install` puts a skill in Claude's and pi's folders that
//! teaches agents these commands. A second socket answers terminal-browser, so
//! `terminal-browser open --split` opens an orb pane.

pub(crate) mod agent;
pub mod browser_socket;
pub mod change;
pub mod client;
pub(crate) mod help;
pub(crate) mod lists;
pub mod pane_control;
pub mod protocol;
pub(crate) mod screen_text;
pub mod server;
pub(crate) mod target;
pub(crate) mod validator;

pub use agent::quoted;
