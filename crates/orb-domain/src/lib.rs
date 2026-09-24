//! The domain layer: application state, intents, and the intent handler.
//!
//! The frontend maps a key to an [`Intent`]; [`IntentHandler::handle`] applies
//! it to [`AppState`], which the renderer reads on the next redraw.

mod app_state;
mod intent;
mod intent_handler;

pub use app_state::AppState;
pub use intent::Intent;
pub use intent_handler::IntentHandler;
