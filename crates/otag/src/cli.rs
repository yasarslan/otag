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

    /// Persistent data directory. Defaults to OTAG_STATE_DIR; custom sockets
    /// otherwise get a private directory beside the socket.
    #[arg(long, value_name = "DIR", global = true)]
    state: Option<PathBuf>,

    /// Exit when the starting process (or the supplied PID) dies.
    #[arg(long, num_args = 0..=1, default_missing_value = "0", global = true)]
    exit_with_parent: Option<u32>,

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
    /// List stray otag daemons and their descendants, then ask to stop them.
    Cleanup {
        /// Stop the listed stray daemons without asking.
        #[arg(long)]
        yes: bool,
    },
    /// List workspaces and panes.
    Ls,
    /// Hand a pane's work to another agent, or to another model.
    ///
    /// Without any option, the other agent otag knows takes over, reading a
    /// package otag writes from the session, the checkout and the pane.
    Handoff {
        /// The pane to hand over, as `otag ls` numbers them.
        #[arg(value_name = "PANE")]
        pane: u64,
        /// Hand over to this agent instead of the other one.
        #[arg(long, value_name = "AGENT", conflicts_with_all = ["model", "fallback"])]
        to: Option<String>,
        /// Keep the agent and the session, and continue on this model.
        #[arg(long, value_name = "MODEL", conflicts_with = "fallback")]
        model: Option<String>,
        /// Keep the agent and the session, and continue on otag's fallback
        /// model for that agent.
        #[arg(long)]
        fallback: bool,
    },
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
    let mut dirs = Dirs::from_env();
    if let Some(state) = &cli.state {
        dirs.state_override = Some(std::path::absolute(state)?);
    }
    let state = dirs.daemon_state_dir(&socket)?;
    let parent = cli
        .exit_with_parent
        .or_else(|| std::env::var("OTAG_EXIT_WITH_PARENT").ok()?.parse().ok());

    match cli.command.unwrap_or(Command::Attach) {
        Command::Attach => {
            logging::init_client(cli.log.as_deref());
            client_runtime()?.block_on(crate::attach::run(&socket, &state))
        }
        Command::Ping => {
            logging::init_client(cli.log.as_deref());
            client_runtime()?.block_on(crate::client::ping(&socket, &state))
        }
        Command::Daemon => crate::daemon::run(&socket, &state, parent, cli.log.as_deref()),
        Command::Cleanup { yes } => crate::cleanup::run(yes, dirs.state_override.as_deref()),
        Command::KillServer => {
            logging::init_client(cli.log.as_deref());
            client_runtime()?.block_on(crate::client::kill_server(&socket))
        }
        Command::Ls => {
            logging::init_client(cli.log.as_deref());
            client_runtime()?.block_on(crate::client::ls(&socket))
        }
        Command::Handoff {
            pane,
            to,
            model,
            fallback,
        } => {
            logging::init_client(cli.log.as_deref());
            client_runtime()?.block_on(crate::client::handoff(
                &socket,
                pane,
                to.as_deref(),
                model.as_deref(),
                fallback,
            ))
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
        let mut dirs = Dirs::from_env();
        if let Some(state) = &self.state {
            dirs.state_override = Some(std::path::absolute(state)?);
        }
        match &self.socket {
            Some(path) => Ok(path.clone()),
            None => dirs.socket_path().with_context(|| {
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
