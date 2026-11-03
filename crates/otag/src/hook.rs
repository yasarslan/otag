//! The bridge an agent calls: `otag hook <event>`.
//!
//! Every one of these runs inside the agent's own turn, so the rules are
//! strict: read what the agent wrote, hand it to the daemon, and be gone
//! again in well under 50 ms. Nothing here may fail in a way the agent
//! notices — no daemon, a socket that nobody is listening on, a payload that
//! is not JSON at all: the command still exits cleanly, because a broken
//! bridge must never block the agent.
//!
//! The statusLine command is not a hook, but it arrives the same way and is
//! handled here too. It has one extra duty: a user who already had a status
//! line keeps it, so otag runs the command it took over and prints what that
//! prints.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::Result;
use otag_core::client;
use otag_core::pane::{PANE_ID_ENV, PaneId};
use otag_core::proto::Request;

/// Name of the file otag keeps the status line it took over in.
pub const CHAIN_FILE: &str = "statusline-chain";

/// How long the whole command may take.
///
/// Reaching the daemon is a connect and a write on a local socket, which is
/// a fraction of this; the budget exists so that a daemon in trouble costs
/// the agent a blink rather than its turn.
const BUDGET: Duration = Duration::from_millis(40);

/// How much of the agent's standard input is read.
///
/// A payload is a few hundred bytes; the cap is what stops a malformed
/// stream from being read until the agent gives up.
const MAX_PAYLOAD: u64 = 1 << 20;

/// How long the status line otag took over may take.
///
/// It is the user's own command, so it gets more room than otag's own work,
/// but not an unbounded amount: the agent is waiting for its status line.
const CHAIN_BUDGET: Duration = Duration::from_secs(2);

/// Report an agent event to the daemon, and always exit cleanly.
pub async fn run(socket: &Path, event: &str, state_dir: Option<&Path>) -> Result<()> {
    deliver(socket, event, read_payload(), state_dir).await
}

/// Everything `run` does once the agent's payload has been read.
async fn deliver(
    socket: &Path,
    event: &str,
    payload: String,
    state_dir: Option<&Path>,
) -> Result<()> {
    // Told first, printed second: the daemon's copy of what the agent is
    // doing is the part that has to be fresh.
    tokio::time::timeout(BUDGET, notify(socket, event, payload.clone()))
        .await
        .ok();

    if event == otag_agents::hooks::STATUS_LINE
        && let Some(command) = state_dir.and_then(chained_command)
        && let Some(output) = chained_output(&command, &payload).await
    {
        print!("{output}");
    }

    Ok(())
}

/// Send the event to the daemon, ignoring every way that can fail.
async fn notify(socket: &Path, event: &str, payload: String) {
    let request = Request::Hook {
        event: event.to_owned(),
        pane: pane_from_env(),
        payload,
    };

    let Ok(mut connection) = client::Connection::connect(socket).await else {
        // No daemon, or one that is shutting down. The event is lost on
        // purpose: the agent must not wait for otag to come back.
        return;
    };
    // The daemon does not answer this, so a successful write is the end of
    // it.
    let _ = connection.send(&request).await;
}

/// Which pane the agent is running in, as otag told its panes.
fn pane_from_env() -> Option<PaneId> {
    std::env::var(PANE_ID_ENV)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(PaneId::from_number)
}

/// What the agent wrote on standard input, or nothing when it wrote nothing.
fn read_payload() -> String {
    let mut text = String::new();
    // A hook with no payload is normal: the statusLine command is sometimes
    // run by hand, and an agent may send nothing at all.
    let _ = std::io::stdin()
        .lock()
        .take(MAX_PAYLOAD)
        .read_to_string(&mut text);
    text
}

/// The status line otag took over, if there was one.
fn chained_command(state_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(chain_path(state_dir)).ok()?;
    let command = text.trim();
    (!command.is_empty()).then(|| command.to_owned())
}

/// Where the status line otag took over is remembered.
#[must_use]
pub fn chain_path(state_dir: &Path) -> PathBuf {
    state_dir.join(CHAIN_FILE)
}

/// Run the status line otag took over and collect what it printed.
async fn chained_output(command: &str, payload: &str) -> Option<String> {
    let mut child = match tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        // The user's command is gone or not runnable. otag prints nothing
        // rather than an error: the agent would show it as the status line.
        Err(_) => return None,
    };

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(payload.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }

    let Ok(Ok(output)) = tokio::time::timeout(CHAIN_BUDGET, child.wait_with_output()).await else {
        return None;
    };
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_directory_without_a_chain_file_chains_nothing() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        assert_eq!(chained_command(dir.path()), None);
    }

    #[test]
    fn a_chain_file_with_only_whitespace_chains_nothing() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(chain_path(dir.path()), "  \n").expect("a chain file");
        assert_eq!(chained_command(dir.path()), None);
    }

    #[test]
    fn the_chained_command_is_read_without_its_newline() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(chain_path(dir.path()), "my-status --fancy\n").expect("a chain file");
        assert_eq!(
            chained_command(dir.path()).as_deref(),
            Some("my-status --fancy")
        );
    }

    #[tokio::test]
    async fn the_chained_status_line_is_given_the_payload_on_its_standard_input() {
        // `cat` stands in for the user's own status line: whatever it reads
        // is what otag has to pass on, and whatever it writes is what the
        // agent has to see.
        assert_eq!(
            chained_output("cat", "{\"model\":\"x\"}").await.as_deref(),
            Some("{\"model\":\"x\"}")
        );
    }

    #[tokio::test]
    async fn a_chained_command_that_cannot_run_is_ignored() {
        // Run through `sh`, so a command that does not exist fails inside
        // the shell rather than at spawn time, and still prints nothing.
        assert_eq!(
            chained_output("otag-no-such-command-exists", "{}")
                .await
                .as_deref(),
            Some("")
        );
    }

    #[tokio::test]
    async fn a_hook_with_no_daemon_listening_still_succeeds() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let socket = dir.path().join("otag.sock");
        deliver(&socket, "Stop", "{}".to_owned(), Some(dir.path()))
            .await
            .expect("a hook never fails");
    }
}
