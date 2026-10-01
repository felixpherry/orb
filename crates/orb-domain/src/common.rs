//! Plumbing shared by every feature: the lock-protected [`State`] the
//! frontend and actors hold, the [`Wake`] that tells the frontend loop to
//! redraw, the [`Services`] actors use, [`tilde`] for showing a path,
//! [`run_within`] for a child process that mustn't hang orb, and the
//! [`TextInput`] behind every line the user types into.

mod child;
mod paths;
mod services;
mod state;
mod text_input;

pub use child::{Finished, run_within};
pub use paths::tilde;
pub use services::Services;
pub use state::{State, Wake};
pub use text_input::TextInput;
