//! Command-line interface.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use otag_core::paths::{Dirs, SOCKET_ENV};

use crate::logging;

/// Terminal-native multiplexer for AI coding agents.
#[derive(Debug, Parser)]
#[command(name = "otag", version, about, long_about = None)]
pub struct Cli {
    /// Path of the daemon socket.
    ///
    /// Defaults to the `OTAG_SOCKET` environment variable, then to a
    /// per-user location under otag's state directory.
    #[arg(long, value_name = "PATH", global = true)]
    socket: Option<PathBuf>,

    /// Log filter, in the `tracing` syntax, for example `debug` or
    /// `otag_core=trace`. Defaults to the `OTAG_LOG` environment variable.
    #[arg(long, value_name = "FILTER", global = true)]
    log: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

/// Subcommands otag understands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Check that the daemon is alive, starting it if it is not.
    Ping,
    /// Run the daemon in the foreground.
    ///
    /// Normally otag starts the daemon for you; this is for debugging and for
    /// service managers such as launchd or systemd.
    Daemon,
    /// Stop the daemon.
    KillServer,
    /// List workspaces and panes.
    Ls,
    /// Print the version of this binary and of the running daemon.
    Version,
}

/// Parse the command line and carry out the requested command.
pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let socket = cli.socket_path()?;

    match cli.command.unwrap_or(Command::Ping) {
        // Attaching to the daemon's user interface arrives with the first
        // pane; until then, bare `otag` does what `otag ping` does.
        Command::Ping => {
            logging::init_client(cli.log.as_deref());
            client_runtime()?.block_on(crate::client::ping(&socket))
        }
        Command::Daemon => crate::daemon::run(&socket, cli.log.as_deref()),
        Command::KillServer => {
            logging::init_client(cli.log.as_deref());
            client_runtime()?.block_on(crate::client::kill_server(&socket))
        }
        Command::Ls => {
            logging::init_client(cli.log.as_deref());
            client_runtime()?.block_on(crate::client::ls(&socket))
        }
        Command::Version => {
            logging::init_client(cli.log.as_deref());
            client_runtime()?.block_on(crate::client::version(&socket))
        }
    }
}

impl Cli {
    /// Resolve the socket path from the flag, the environment, or the
    /// platform's conventions, in that order.
    fn socket_path(&self) -> Result<PathBuf> {
        match &self.socket {
            Some(path) => Ok(path.clone()),
            None => Dirs::from_env().socket_path().with_context(|| {
                format!("pass --socket or set {SOCKET_ENV} to choose the socket path")
            }),
        }
    }
}

/// A small runtime: client commands are short-lived and should start fast.
fn client_runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")
}
