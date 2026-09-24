//! Plumbing shared by every feature: the lock-protected [`State`] the
//! frontend and actors hold, and the [`Wake`] that tells the frontend loop to
//! redraw.

mod state;

pub use state::{State, Wake};
