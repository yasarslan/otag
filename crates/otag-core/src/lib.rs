//! Core building blocks of otag: the client/server socket protocol, the
//! resolution of on-disk locations, and the daemon-side socket server.
//!
//! Agent-specific knowledge never belongs here; it lives in `otag-agents`.
#![forbid(unsafe_code)]

pub mod client;
pub mod error;
pub mod paths;
pub mod proto;
pub mod server;

pub use error::{Error, Result};

/// Version of otag, taken from the crate metadata at compile time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
