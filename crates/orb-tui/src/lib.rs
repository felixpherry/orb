//! The ratatui frontend: reads input, maps keys to intents, drives the
//! terminal pane, and draws [`AppState`](orb_domain::AppState).

mod dashboard;
mod keymap;
mod mode_line;
mod outer_terminal;
mod picker;
mod rename;
mod render;
mod run;
mod session_picker;
mod sidebar;
mod which_key;

pub use run::{Frontend, TuiRunError};
