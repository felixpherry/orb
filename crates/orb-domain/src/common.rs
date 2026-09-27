//! Plumbing shared by every feature: the lock-protected [`State`] the
//! frontend and actors hold, the [`Wake`] that tells the frontend loop to
//! redraw, the [`Services`] actors use, [`tilde`] for showing a path, and the
//! [`TextInput`] behind every line the user types into.

mod paths;
mod services;
mod state;
mod text_input;

pub use paths::tilde;
pub use services::Services;
pub use state::{State, Wake};
pub use text_input::TextInput;
