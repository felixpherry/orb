//! Plumbing shared by every feature: the lock-protected [`State`] the
//! frontend and actors hold, the [`Wake`] that tells the frontend loop to
//! redraw, the [`Services`] actors use, and [`tilde`] for showing a path.

mod paths;
mod services;
mod state;

pub use paths::tilde;
pub use services::Services;
pub use state::{State, Wake};
