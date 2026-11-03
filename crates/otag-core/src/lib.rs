//! Core building blocks of otag: the client/server socket protocol, the
//! resolution of on-disk locations, and the daemon-side socket server.
//!
//! Agent-specific knowledge never belongs here; it lives in `otag-agents`.
#![forbid(unsafe_code)]

pub mod attach;
pub mod client;
pub mod error;
pub mod frame;
pub mod layout;
pub mod pane;
pub mod paths;
pub mod proto;
pub mod pty;
pub mod raw;
pub mod replay;
pub mod screen;
pub mod server;
pub mod session;
pub mod snapshot;
pub mod workspace;

pub use error::{Error, Result};

/// Version of otag, taken from the crate metadata at compile time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
