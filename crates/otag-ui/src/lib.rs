//! Terminal user interface for otag: rendering, input encoding and themes.
//!
//! This crate draws what the daemon sends and turns what the user does into
//! bytes a pane's program understands. It holds no state about panes and
//! talks to no socket: the client wires the two together.
#![forbid(unsafe_code)]

pub mod keys;
pub mod render;
pub mod session;
pub mod terminal;
pub mod theme;
