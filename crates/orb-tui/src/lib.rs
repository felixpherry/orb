//! The ratatui frontend: reads keys, maps them to intents, and draws
//! [`AppState`](orb_domain::AppState).

mod keymap;
mod run;

pub use run::{TuiRunError, run};
