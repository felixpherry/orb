//! Notify — tells the user, with a desktop notification, that a thread needs
//! them while they're away from orb's pane.
//!
//! A notification's title names the thread's project and the thread, and its
//! body says why: the turn finished, or the session needs an approval or an answer.
//!
//! On macOS it goes to Notification Center through `terminal-notifier` when orb
//! finds it at startup: clicking it brings orb's kitty window forward and goes
//! to orb's zellij tab and pane, and a later notification about the same thread
//! replaces it. Otherwise, or when terminal-notifier fails (for example because
//! it isn't allowed to notify), it goes out through `osascript`, whose click
//! opens Script Editor. Either way its text is passed as arguments, never as code.
//!
//! On Linux it goes to the desktop's notification server. An approval or an
//! answer is critical, a finished turn normal, and a later notification about
//! the same thread replaces it. Clicking the latest one brings orb's window
//! forward on niri and goes to orb's zellij tab and pane. With no notification
//! server, nothing is shown.
//!
//! On any other OS, nothing is shown.

pub mod click;
pub mod none;
pub mod notifier;
pub mod osascript;
pub mod terminal_notifier;
pub mod xdg;
