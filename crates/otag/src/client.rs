//! Client-side commands: talk to the daemon, starting it when needed.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use otag_core::client;
use otag_core::pane::PaneId;
use otag_core::proto::{Handoff, Request, Response};
use otag_ui::usage as ui_usage;

/// How long to wait for a freshly spawned daemon to start listening.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Check that the daemon is alive, starting it if it is not, and print `pong`.
pub async fn ping(socket: &Path, state: &Path) -> Result<()> {
    ensure_daemon(socket, state).await?;
    match client::request(socket, &Request::Ping).await? {
        Response::Pong => {
            println!("pong");
            Ok(())
        }
        other => bail!(unexpected(other)),
    }
}

/// Stop the daemon, or report that there was nothing to stop.
pub async fn kill_server(socket: &Path) -> Result<()> {
    match client::request(socket, &Request::Shutdown).await {
        Ok(Response::Ok) => {
            println!("daemon stopped");
            Ok(())
        }
        Ok(other) => bail!(unexpected(other)),
        Err(otag_core::Error::NotRunning { .. }) => {
            println!("no daemon is running");
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

/// List the panes the daemon is running.
pub async fn ls(socket: &Path) -> Result<()> {
    match client::request(socket, &Request::ListPanes).await {
        Ok(Response::Panes { panes }) => {
            if panes.is_empty() {
                println!("no panes yet");
            }
            for pane in panes {
                let state = match &pane.exit {
                    Some(exit) => exit.to_string(),
                    None => "running".to_owned(),
                };
                let workspace = if pane.workspace.is_empty() {
                    String::new()
                } else {
                    format!("{}: ", pane.workspace)
                };
                let agent = match pane.agent {
                    Some(agent) => format!(" [{agent} {}]", pane.status),
                    None => String::new(),
                };
                let session = match &pane.session {
                    Some(session) => format!(" session {session}"),
                    None => String::new(),
                };
                println!(
                    "{}: {workspace}{} {} {state}{agent}{session}",
                    pane.id, pane.title, pane.size
                );
            }
            Ok(())
        }
        Ok(other) => bail!(unexpected(other)),
        Err(otag_core::Error::NotRunning { .. }) => {
            println!("no daemon is running");
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

/// Hand a pane's work over, and say what happened.
pub async fn handoff(
    socket: &Path,
    pane: u64,
    to: Option<&str>,
    model: Option<&str>,
    fallback: bool,
) -> Result<()> {
    let handoff = if fallback || model.is_some() {
        Handoff::Model {
            model: model.map(str::to_owned),
        }
    } else {
        Handoff::Agent {
            to: match to {
                Some(name) => Some(name.parse().map_err(anyhow::Error::from)?),
                None => None,
            },
        }
    };

    let request = Request::Handoff {
        pane: PaneId::from_number(pane),
        handoff,
    };
    match client::request(socket, &request).await {
        Ok(Response::HandoffDone {
            pane,
            package,
            summary,
        }) => {
            println!("{summary}");
            println!("the work carries on in pane {pane}");
            if let Some(package) = package {
                println!("the package it reads is {}", package.display());
            }
            Ok(())
        }
        Ok(other) => bail!(unexpected(other)),
        Err(otag_core::Error::NotRunning { .. }) => {
            println!("no daemon is running, so there is no pane to hand over");
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

/// Print how much of the agents' limits is used.
///
/// `--json` prints exactly what the daemon reported, for a script or for an
/// agent asking about its own budget; without it the same numbers are
/// printed the way the widget shows them.
pub async fn usage(socket: &Path, as_json: bool) -> Result<()> {
    let (report, panes) = match client::request(socket, &Request::Usage).await {
        Ok(Response::Usage { report, panes }) => (*report, panes),
        Ok(other) => bail!(unexpected(other)),
        Err(otag_core::Error::NotRunning { .. }) => {
            println!("no daemon is running");
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };

    if as_json {
        let document = serde_json::json!({ "report": report, "panes": panes });
        println!("{document}");
        return Ok(());
    }

    if report.is_empty() {
        println!(
            "no reading yet. Claude Code reports its limits with every reply, through otag's status line (`otag install-hooks`), and Codex reports them in its session files."
        );
    }
    let now = otag_features::usage::now();
    for row in &report.rows {
        // The same line every client shows: who, which window, what the
        // agent last said, and when that window resets. Nothing else: a
        // reading the user can check against the agent itself is worth more
        // than a reading with otag's own arithmetic beside it.
        let used = match row.used_pct() {
            Some(used_pct) => format!("{used_pct}%"),
            None => ui_usage::NO_READING.to_owned(),
        };
        let mut line = format!("{} {:<4} {used:>4}", row.agent.label(), row.window.label());
        if let Some(resets_at) = row.resets_at {
            line.push_str(&format!(
                "   resets {}",
                ui_usage::reset_clock(resets_at, now)
            ));
        }
        println!("{line}");
    }
    for counted in &report.counted {
        println!(
            "{} ~{} tokens (it reports no limits of its own)",
            counted.agent.label(),
            ui_usage::tokens(counted.tokens)
        );
    }

    for pane in panes
        .iter()
        .filter(|pane| pane.agent.is_some() && pane.tokens.total() > 0)
    {
        println!(
            "  pane {}: {} {} tokens",
            pane.pane,
            pane.agent.map_or("", otag_agents::AgentKind::label),
            ui_usage::tokens(pane.tokens.total())
        );
    }
    Ok(())
}

/// Print this binary's version, and the running daemon's version if there is
/// one.
pub async fn version(socket: &Path) -> Result<()> {
    println!("otag {}", otag_core::VERSION);
    match client::request(socket, &Request::Version).await {
        Ok(Response::Version { version }) => println!("daemon {version}"),
        Ok(Response::Error { message }) => println!("daemon unknown ({message})"),
        Ok(_) | Err(otag_core::Error::NotRunning { .. }) => println!("daemon not running"),
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

/// Make sure a daemon is listening on `socket`, spawning one if needed.
pub async fn ensure_daemon(socket: &Path, state: &Path) -> Result<()> {
    if client::is_running(socket).await {
        return Ok(());
    }

    spawn_daemon(socket, state)?;

    client::wait_until_ready(socket, STARTUP_TIMEOUT)
        .await
        .with_context(|| {
            format!(
                "the otag daemon did not start within {} seconds. \
                 Check the log in {} or run `otag daemon` to see why.",
                STARTUP_TIMEOUT.as_secs(),
                state.join("logs").display()
            )
        })
}

/// Start `otag daemon` as a background process that outlives this one.
fn spawn_daemon(socket: &Path, state: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;

    let exe = std::env::current_exe().context("cannot find the otag executable")?;

    let mut command = Command::new(&exe);
    command
        .arg("daemon")
        .arg("--socket")
        .arg(socket)
        .arg("--state")
        .arg(state)
        .stdin(Stdio::null())
        .stdout(stray_output(state))
        .stderr(stray_output(state))
        // Its own process group, so closing the terminal or pressing Ctrl-C
        // does not take the daemon and its agents down with the client.
        .process_group(0);

    if let Ok(parent) = std::env::var("OTAG_EXIT_WITH_PARENT") {
        command.arg("--exit-with-parent").arg(parent);
    }

    command
        .spawn()
        .with_context(|| format!("cannot start the otag daemon ({})", exe.display()))?;
    Ok(())
}

/// Where a daemon panic or a message from before logging was set up goes.
///
/// Anything unexpected on the daemon's stdout or stderr is appended to a file
/// next to the logs; if that is not possible it is discarded rather than
/// written to the user's terminal.
fn stray_output(state: &Path) -> Stdio {
    let dir = state.join("logs");
    if std::fs::create_dir_all(&dir).is_err() {
        return Stdio::null();
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("daemon.out"))
        .map_or_else(|_| Stdio::null(), Stdio::from)
}

fn unexpected(response: Response) -> String {
    match response {
        Response::Error { message } => message,
        other => format!("the otag daemon sent an unexpected answer: {other:?}"),
    }
}
