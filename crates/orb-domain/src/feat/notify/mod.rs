//! Notify — tells the user, in macOS Notification Center, that a thread
//! needs them while they're away from orb's pane.
//!
//! A notification's title names the thread's project and the thread, and its
//! body says why: the turn finished, or Claude needs an approval or an answer.
//! It goes out through `terminal-notifier` when orb finds it at startup:
//! clicking it brings orb's kitty window forward and goes to orb's zellij tab
//! and pane, and a later notification about the same thread replaces it.
//! Otherwise it goes out through `osascript`, whose click opens Script
//! Editor. Either way its text is passed as arguments, never as code.

pub mod notifier;
pub mod osascript;
pub mod terminal_notifier;
