//! The domain layer: application state, intents, commands, the intent handler,
//! and the features behind them (the terminal pane and Claude sessions).
//!
//! The frontend maps a key to an [`Intent`]; [`IntentHandler::handle`] applies
//! it to [`AppState`] and returns the [`Command`]s that follow. The frontend
//! and the actors share the state through [`State`]; the renderer reads it on
//! the next redraw.

mod app_state;
mod command;
mod common;
pub mod feat;
mod intent;
mod intent_handler;

pub use app_state::{AppState, Focus};
pub use command::Command;
pub use common::{Services, State, TextInput, Wake, tilde};
pub use intent::Intent;
pub use intent_handler::IntentHandler;
