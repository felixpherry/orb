//! Notify — tells the user, in macOS Notification Center, that a thread
//! needs them while they're away from orb's pane.
//!
//! A notification's title names the thread's project and the thread, and its
//! body says why: the turn finished, or Claude needs an approval or an answer.
//! It goes out through `osascript`, with its text passed as arguments rather
//! than written into the script.

pub mod notifier;
pub mod osascript;
