//! The ratatui frontend: reads input, maps keys to intents, drives the
//! terminal pane, and draws [`AppState`](orb_domain::AppState).

mod draft;
mod keymap;
mod outer_terminal;
mod picker;
mod preview;
mod rename;
mod render;
mod run;
mod sidebar;
pub mod which_key_prototype;

pub use run::{Frontend, TuiRunError};
