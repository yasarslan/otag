//! End-to-end tests for the `otag` binary, including the daemon it starts.
//!
//! Every test runs the real binary in its own temporary directory and passes
//! the socket path explicitly — through `OTAG_SOCKET` in the child's
//! environment or through `--socket`. The test process never changes its own
//! environment, so the tests stay independent of each other and never touch
//! the real home directory.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

/// Path of the binary under test, provided by Cargo.
const OTAG: &str = env!("CARGO_BIN_EXE_otag");

/// An isolated home directory and socket path for one test.
struct Sandbox {
    dir: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            dir: TempDir::new().expect("temporary directory"),
        }
    }

    fn home(&self) -> &Path {
        self.dir.path()
    }

    fn socket(&self) -> PathBuf {
        self.dir.path().join("otag.sock")
    }

    /// A command with an isolated `HOME` and the socket path in the
    /// environment, the way a user's shell would provide it.
    fn otag(&self) -> Command {
        let mut command = self.bare_otag();
        command.env("OTAG_SOCKET", self.socket());
        command
    }

    /// A command with an isolated `HOME` and no socket in the environment, for
    /// testing the `--socket` flag.
    fn bare_otag(&self) -> Command {
        let mut command = Command::new(OTAG);
        command
            .env("HOME", self.home())
            .env_remove("OTAG_SOCKET")
            .env_remove("OTAG_LOG")
            .env_remove("XDG_RUNTIME_DIR")
            .env_remove("XDG_STATE_HOME");
        command
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // Never leave a daemon behind, even if the test failed.
        let _ = self
            .bare_otag()
            .args(["kill-server", "--socket"])
            .arg(self.socket())
            .output();
    }
}

/// Run a command and require that it succeeded, returning its stdout.
fn succeeds(command: &mut Command) -> String {
    let output = command.output().expect("the otag binary runs");
    assert!(
        output.status.success(),
        "command failed: {:?}\nstdout: {}\nstderr: {}",
        command,
        text(&output.stdout),
        text(&output.stderr),
    );
    stdout(&output)
}

fn stdout(output: &Output) -> String {
    text(&output.stdout)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn bare_otag_needs_a_terminal_and_starts_nothing_without_one() {
    // Bare `otag` draws a pane, which a test has no terminal for. It must
    // say so instead of hanging, and must not leave a daemon running for a
    // client that could never draw.
    let sandbox = Sandbox::new();
    assert!(!sandbox.socket().exists());

    let output = sandbox.otag().output().expect("the otag binary runs");
    assert!(!output.status.success());

    let message = text(&output.stderr);
    assert!(message.contains("needs a terminal"), "{message}");
    assert!(
        !sandbox.socket().exists(),
        "no daemon is started for a client that cannot draw"
    );
}

#[test]
fn ping_starts_the_daemon_and_prints_pong() {
    let sandbox = Sandbox::new();
    assert!(!sandbox.socket().exists());

    let out = succeeds(sandbox.otag().arg("ping"));
    assert_eq!(out.trim(), "pong");
    assert!(
        sandbox.socket().exists(),
        "the daemon's socket exists after the first run"
    );
}

#[test]
fn ls_reports_that_there_are_no_panes_yet() {
    let sandbox = Sandbox::new();
    succeeds(sandbox.otag().arg("ping"));

    let listing = succeeds(sandbox.otag().arg("ls"));
    assert_eq!(listing.trim(), "no panes yet");
}

#[test]
fn ping_reuses_a_running_daemon() {
    let sandbox = Sandbox::new();
    assert_eq!(succeeds(sandbox.otag().arg("ping")).trim(), "pong");
    assert_eq!(succeeds(sandbox.otag().arg("ping")).trim(), "pong");

    let reported = succeeds(sandbox.otag().arg("version"));
    assert!(
        reported.contains(&format!("daemon {}", env!("CARGO_PKG_VERSION"))),
        "version output: {reported}"
    );
}

#[test]
fn the_socket_flag_overrides_the_environment() {
    let sandbox = Sandbox::new();
    let out = succeeds(
        sandbox
            .bare_otag()
            .arg("ping")
            .arg("--socket")
            .arg(sandbox.socket()),
    );
    assert_eq!(out.trim(), "pong");
    assert!(sandbox.socket().exists());
}

#[test]
fn kill_server_stops_the_daemon_and_removes_the_socket() {
    let sandbox = Sandbox::new();
    assert_eq!(succeeds(sandbox.otag().arg("ping")).trim(), "pong");

    let out = succeeds(sandbox.otag().arg("kill-server"));
    assert_eq!(out.trim(), "daemon stopped");

    // The daemon removes its socket on the way out; give it a moment to do so.
    for _ in 0..200 {
        if !sandbox.socket().exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(
        !sandbox.socket().exists(),
        "the socket is removed when the daemon stops"
    );
}

#[test]
fn kill_server_without_a_daemon_succeeds_quietly() {
    let sandbox = Sandbox::new();
    let out = succeeds(sandbox.otag().arg("kill-server"));
    assert_eq!(out.trim(), "no daemon is running");
}

#[test]
fn ls_without_a_daemon_does_not_start_one() {
    let sandbox = Sandbox::new();
    let out = succeeds(sandbox.otag().arg("ls"));
    assert_eq!(out.trim(), "no daemon is running");
    assert!(!sandbox.socket().exists(), "`ls` does not start a daemon");
}

#[test]
fn version_without_a_daemon_reports_the_binary_only() {
    let sandbox = Sandbox::new();
    let out = succeeds(sandbox.otag().arg("version"));
    assert!(
        out.contains(&format!("otag {}", env!("CARGO_PKG_VERSION"))),
        "version output: {out}"
    );
    assert!(out.contains("daemon not running"), "version output: {out}");
    assert!(
        !sandbox.socket().exists(),
        "`version` does not start a daemon"
    );
}

#[test]
fn an_unwritable_socket_path_fails_with_an_actionable_message() {
    let sandbox = Sandbox::new();
    // A regular file cannot hold a socket, so creating the parent directory
    // fails on every platform.
    let blocker = sandbox.home().join("not-a-directory");
    std::fs::write(&blocker, b"").expect("writes the blocking file");

    let output = sandbox
        .bare_otag()
        .arg("daemon")
        .arg("--socket")
        .arg(blocker.join("otag.sock"))
        .output()
        .expect("the otag binary runs");

    assert!(
        !output.status.success(),
        "the daemon should refuse to start"
    );
    let stderr = text(&output.stderr);
    assert!(stderr.starts_with("otag: "), "stderr: {stderr}");
    assert!(stderr.contains("otag.sock"), "stderr: {stderr}");
}

#[test]
fn help_and_version_flags_work_without_a_daemon() {
    let sandbox = Sandbox::new();
    let help = succeeds(sandbox.otag().arg("--help"));
    for command in ["ping", "daemon", "kill-server", "ls", "version"] {
        assert!(help.contains(command), "`{command}` is listed in --help");
    }

    let version = succeeds(sandbox.otag().arg("--version"));
    assert!(
        version.contains(env!("CARGO_PKG_VERSION")),
        "version output: {version}"
    );
    assert!(
        !sandbox.socket().exists(),
        "`--help` does not start a daemon"
    );
}

#[test]
fn usage_says_there_is_nothing_yet_and_how_to_get_it() {
    let sandbox = Sandbox::new();
    succeeds(sandbox.otag().arg("ping"));

    let reported = succeeds(sandbox.otag().arg("usage"));
    assert!(
        reported.contains("no usage yet") && reported.contains("install-hooks"),
        "a user with no numbers is told how to get them: {reported}"
    );
}

#[test]
fn usage_prints_json_for_a_script_to_read() {
    let sandbox = Sandbox::new();
    succeeds(sandbox.otag().arg("ping"));

    let reported = succeeds(sandbox.otag().args(["usage", "--json"]));
    let document: serde_json::Value = serde_json::from_str(&reported).expect("the output is JSON");
    assert!(
        document["report"]["rows"].is_array(),
        "the shape is stable even when there is nothing to report: {reported}"
    );
}

#[test]
fn usage_without_a_daemon_says_so_rather_than_failing() {
    let sandbox = Sandbox::new();
    let reported = succeeds(sandbox.otag().arg("usage"));
    assert_eq!(reported.trim(), "no daemon is running");
}
