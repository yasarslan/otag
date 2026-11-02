//! Error type shared by the core crate.

use std::path::PathBuf;

/// Convenience alias for results produced by otag's core.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong in the core crate.
///
/// Messages are written for end users: they say what happened and, where
/// useful, what to do about it.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A required directory or file location could not be determined.
    #[error(
        "cannot determine the otag {what} location: {reason}. \
         Set OTAG_SOCKET to an explicit socket path to work around this."
    )]
    Location {
        /// Which location was being resolved, for example `socket`.
        what: &'static str,
        /// Why the resolution failed.
        reason: String,
    },

    /// No daemon is listening on the socket.
    #[error("no otag daemon is listening on {path}. Run `otag` to start one.")]
    NotRunning {
        /// Socket path that was tried.
        path: PathBuf,
    },

    /// A daemon is already listening on the socket.
    #[error("an otag daemon is already running on {path}")]
    AlreadyRunning {
        /// Socket path that is already in use.
        path: PathBuf,
    },

    /// The peer closed the connection before sending a complete response.
    #[error("the otag daemon closed the connection before answering")]
    ConnectionClosed,

    /// The daemon answered with an error.
    #[error("the otag daemon reported an error: {message}")]
    Daemon {
        /// Message as reported by the daemon.
        message: String,
    },

    /// An I/O operation failed.
    #[error("{context}: {source}")]
    Io {
        /// What was being attempted.
        context: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// A pane's pseudo-terminal could not be created or driven.
    #[error("{context}: {source}")]
    Pty {
        /// What was being attempted.
        context: String,
        /// Underlying failure, as reported by the pseudo-terminal layer.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// A request named a pane the daemon does not have.
    #[error("{}", pane_gone(.id))]
    PaneGone {
        /// Pane that was asked for, when the caller named one.
        id: Option<crate::pane::PaneId>,
    },

    /// A request needed a pane to work on and the session has none.
    #[error("the otag session has no panes. Run `otag` to start one.")]
    NoPanes,

    /// A protocol message could not be encoded or decoded.
    #[error("malformed otag protocol message: {0}")]
    Protocol(#[from] serde_json::Error),
}

fn pane_gone(id: &Option<crate::pane::PaneId>) -> String {
    match id {
        Some(id) => format!("pane {id} is no longer running"),
        None => "that pane is no longer running".to_owned(),
    }
}

impl Error {
    /// Wrap a pseudo-terminal failure with a description of what was being
    /// attempted.
    pub fn pty(
        context: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::Pty {
            context: context.into(),
            source: source.into(),
        }
    }

    /// Wrap an I/O error with a description of what was being attempted.
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}
