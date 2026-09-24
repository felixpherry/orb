//! Plumbing shared by every feature: the lock-protected [`State`] the
//! frontend and actors hold, the [`Wake`] that tells the frontend loop to
//! redraw, and the [`Services`] actors use.

mod services;
mod state;

pub use services::Services;
pub use state::{State, Wake};
