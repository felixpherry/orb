//! Sidebar view — how the sidebar is laid out on screen: its width, whether
//! it's hidden, and how the last frame drew its rows, which the sidebar's
//! half-page jumps measure by. While it's hidden, it can't be resized or
//! focused.

pub mod state;
pub mod validator;
