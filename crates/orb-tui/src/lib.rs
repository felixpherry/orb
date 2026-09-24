//! The ratatui frontend: reads input, maps keys to intents, drives the
//! terminal pane, and draws [`AppState`](orb_domain::AppState).

mod child_env;
mod keymap;
mod outer_terminal;
mod run;

pub use run::{TuiRunError, run};
