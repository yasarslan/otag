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
    /// Draw a pane in this terminal, starting the daemon and a pane if
    /// needed. This is what plain `otag` does.
    Attach,
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
    /// Print how much of the agents' limits is used.
    Usage {
        /// Print the numbers as JSON instead of as lines.
        #[arg(long)]
        json: bool,
    },
    /// Report an agent event to the daemon. Agents run this themselves.
    ///
    /// The event is the name otag registered the command under; the payload
    /// arrives on standard input. Exits cleanly and quickly whatever
    /// happens, so that a hook can never hold an agent up.
    Hook {
        /// Which event this is, or `statusline` for the status line.
        #[arg(value_name = "EVENT")]
        event: String,
    },
    /// Register otag's hooks and status line in Claude Code's settings.
    ///
    /// Says what it would change and asks before changing it, and keeps a
    /// copy of the file as it was.
    InstallHooks {
        /// The settings file to change. Defaults to Claude Code's own.
        #[arg(long, value_name = "PATH")]
        settings: Option<PathBuf>,
        /// Do not ask; change the file.
        #[arg(long)]
        yes: bool,
    },
    /// Take otag's hooks and status line out again.
    UninstallHooks {
        /// The settings file to change. Defaults to Claude Code's own.
        #[arg(long, value_name = "PATH")]
        settings: Option<PathBuf>,
        /// Do not ask; change the file.
        #[arg(long)]
        yes: bool,
    },
    /// Print the version of this binary and of the running daemon.
    Version,
}

/// Parse the command line and carry out the requested command.
pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let socket = cli.socket_path()?;

    match cli.command.unwrap_or(Command::Attach) {
        Command::Attach => {
            logging::init_client(cli.log.as_deref());
            client_runtime()?.block_on(crate::attach::run(&socket))
        }
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
        Command::Usage { json } => {
            logging::init_client(cli.log.as_deref());
            client_runtime()?.block_on(crate::client::usage(&socket, json))
        }
        Command::Hook { event } => {
            // A hook is logged nowhere by default: it runs inside an agent's
            // turn, several times a minute, and its output would be the
            // agent's own.
            client_runtime()?.block_on(crate::hook::run(
                &socket,
                &event,
                crate::install::hook_state_dir().as_deref(),
            ))
        }
        Command::InstallHooks { settings, yes } => {
            logging::init_client(cli.log.as_deref());
            crate::install::install(settings, yes)
        }
        Command::UninstallHooks { settings, yes } => {
            logging::init_client(cli.log.as_deref());
            crate::install::uninstall(settings, yes)
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
