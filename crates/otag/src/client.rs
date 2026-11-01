//! Client-side commands: talk to the daemon, starting it when needed.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use otag_core::client;
use otag_core::paths::Dirs;
use otag_core::proto::{Request, Response};

/// How long to wait for a freshly spawned daemon to start listening.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Check that the daemon is alive, starting it if it is not, and print `pong`.
pub async fn ping(socket: &Path) -> Result<()> {
    ensure_daemon(socket).await?;
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

/// List workspaces and panes.
pub async fn ls(socket: &Path) -> Result<()> {
    // Workspaces and panes arrive with the layout milestones; until then this
    // command exists so the shape of the interface is fixed.
    match client::request(socket, &Request::Ping).await {
        Ok(Response::Pong) => {
            println!("no workspaces yet");
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
async fn ensure_daemon(socket: &Path) -> Result<()> {
    if client::is_running(socket).await {
        return Ok(());
    }

    spawn_daemon(socket)?;

    client::wait_until_ready(socket, STARTUP_TIMEOUT)
        .await
        .with_context(|| {
            format!(
                "the otag daemon did not start within {} seconds. \
                 Check the log in {} or run `otag daemon` to see why.",
                STARTUP_TIMEOUT.as_secs(),
                log_dir_hint()
            )
        })
}

/// Start `otag daemon` as a background process that outlives this one.
fn spawn_daemon(socket: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;

    let exe = std::env::current_exe().context("cannot find the otag executable")?;

    let mut command = Command::new(&exe);
    command
        .arg("daemon")
        .arg("--socket")
        .arg(socket)
        .stdin(Stdio::null())
        .stdout(stray_output())
        .stderr(stray_output())
        // Its own process group, so closing the terminal or pressing Ctrl-C
        // does not take the daemon and its agents down with the client.
        .process_group(0);

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
fn stray_output() -> Stdio {
    let Some(dir) = log_dir() else {
        return Stdio::null();
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return Stdio::null();
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("daemon.out"))
        .map_or_else(|_| Stdio::null(), Stdio::from)
}

fn log_dir() -> Option<PathBuf> {
    Dirs::from_env().log_dir().ok()
}

fn log_dir_hint() -> String {
    log_dir().map_or_else(
        || "otag's log directory".to_owned(),
        |dir| dir.display().to_string(),
    )
}

fn unexpected(response: Response) -> String {
    match response {
        Response::Error { message } => message,
        other => format!("the otag daemon sent an unexpected answer: {other:?}"),
    }
}
