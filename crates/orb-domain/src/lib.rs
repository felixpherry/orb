//! The domain layer: application state, intents, commands, and the intent handler.
//!
//! The frontend maps a key to an [`Intent`]; [`IntentHandler::handle`] applies
//! it to [`AppState`], which the renderer reads on the next redraw, and returns
//! the [`Command`]s the frontend loop carries out.

mod app_state;
mod command;
pub mod feat;
mod intent;
mod intent_handler;

pub use app_state::{AppState, Focus};
pub use command::Command;
pub use intent::Intent;
pub use intent_handler::IntentHandler;
