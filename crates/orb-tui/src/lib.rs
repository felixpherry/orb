//! The ratatui frontend: reads input, maps keys to intents, drives the
//! terminal pane, and draws [`AppState`](orb_domain::AppState).

mod keymap;
mod outer_terminal;
mod render;
mod run;
mod sidebar;

pub use run::{Frontend, TuiRunError};
