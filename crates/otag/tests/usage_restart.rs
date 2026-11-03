//! What the daemon still knows about the limits after it has been restarted.
//!
//! A reading belongs to a window that runs for hours, so a daemon that was
//! restarted in the middle of one must come back knowing where the account
//! stood. This drives the real binary — a daemon of its own, a home of its
//! own — because the thing being tested is the whole path: a status line in,
//! the database, and the report out again.

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long to wait for a daemon to start listening.
const STARTUP: Duration = Duration::from_secs(10);

/// A daemon with a home of its own, so nothing on the machine is touched.
struct Daemon {
    home: tempfile::TempDir,
    socket: std::path::PathBuf,
    process: Option<Child>,
}

impl Daemon {
    fn start() -> Self {
        let home = tempfile::tempdir().expect("a temporary home");
        let socket = home.path().join("otag.sock");
        let mut daemon = Self {
            home,
            socket,
            process: None,
        };
        daemon.spawn();
        daemon
    }

    /// Start a daemon process on the same socket and state directory.
    fn spawn(&mut self) {
        let log = std::fs::File::create(self.home.path().join("daemon.log")).ok();
        let errors = log.as_ref().and_then(|file| file.try_clone().ok());
        let process = self
            .command(&["daemon", "--socket", &self.socket.to_string_lossy()])
            .stdout(log.map_or_else(Stdio::null, Stdio::from))
            .stderr(errors.map_or_else(Stdio::null, Stdio::from))
            .spawn()
            .expect("the daemon can be started");
        self.process = Some(process);
        self.wait_until_listening();
    }

    /// otag, with everything pointed at this test's own home.
    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_otag"));
        command
            .args(arguments)
            .env("HOME", self.home.path())
            // An XDG machine would otherwise put otag's state, and so its
            // database, outside the temporary home.
            .env("XDG_STATE_HOME", self.home.path().join("state"))
            .env("XDG_RUNTIME_DIR", self.home.path().join("run"))
            .env("OTAG_NO_NOTIFY", "1");
        command
    }

    /// Run one otag command and return what it printed.
    fn run(&self, arguments: &[&str], stdin: &str) -> String {
        let mut arguments = arguments.to_vec();
        let socket = self.socket.to_string_lossy().into_owned();
        arguments.extend_from_slice(&["--socket", &socket]);

        let mut child = self
            .command(&arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("otag can be started");
        child
            .stdin
            .take()
            .expect("a pipe")
            .write_all(stdin.as_bytes())
            .expect("the payload can be written");
        let output = child.wait_with_output().expect("otag finishes");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn wait_until_listening(&self) {
        let started = Instant::now();
        while started.elapsed() < STARTUP {
            if self.run(&["ping"], "").contains("pong") {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!(
            "the daemon never started listening. It printed:\n{}",
            self.log()
        );
    }

    /// Stop the daemon the way a user does, and wait for it to be gone.
    fn kill_server(&mut self) {
        self.run(&["kill-server"], "");
        if let Some(mut process) = self.process.take() {
            let _ = process.wait();
        }
    }

    /// What the report says, as the structure the clients get.
    fn usage(&self) -> serde_json::Value {
        let printed = self.run(&["usage", "--json"], "");
        serde_json::from_str(&printed)
            .unwrap_or_else(|error| panic!("usage --json is JSON ({error}): {printed}"))
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.home.path().join("daemon.log")).unwrap_or_default()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some(mut process) = self.process.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}

/// Now, in seconds since the epoch.
fn now() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("a clock after 1970")
            .as_secs(),
    )
    .expect("a clock this century")
}

/// A status line payload, as Claude Code writes it.
fn status_line(used_pct: f64, resets_at: i64) -> String {
    serde_json::json!({
        "session_id": "sess-abc12345-6789",
        "rate_limits": {
            "five_hour": { "used_percentage": used_pct, "resets_at": resets_at },
        },
    })
    .to_string()
}

/// The five-hour row of a report, if it has one.
fn five_hour(report: &serde_json::Value) -> Option<&serde_json::Value> {
    report
        .get("report")?
        .get("rows")?
        .as_array()?
        .iter()
        .find(|row| row.get("window").and_then(serde_json::Value::as_str) == Some("five_hour"))
}

#[test]
fn a_reading_whose_window_is_still_running_survives_a_restart() {
    let mut daemon = Daemon::start();

    // A session reports 25% of a window that runs for another two hours.
    let resets_at = now() + 2 * 60 * 60;
    daemon.run(&["hook", "statusline"], &status_line(25.0, resets_at));

    let before = daemon.usage();
    let row = five_hour(&before).expect("the window otag was told about");
    assert_eq!(row["used_tenths"], 250, "before the restart: {before}");
    assert_eq!(row["reset"], false);
    assert_eq!(
        row["session"], "sess-abc12345-6789",
        "and otag knows which session said so"
    );

    // The daemon is stopped and started again, which is what an upgrade or
    // `otag kill-server` does.
    daemon.kill_server();
    daemon.spawn();

    let after = daemon.usage();
    let row = five_hour(&after).unwrap_or_else(|| {
        panic!(
            "the window is gone from the report after a restart: {after}\n{}",
            daemon.log()
        )
    });
    assert_eq!(
        row["reset"], false,
        "the window has not reset, so otag is not waiting for fresh data: {after}"
    );
    assert_eq!(
        row["used_tenths"], 250,
        "a reading of a window that is still running is still the reading: {after}"
    );
    assert!(
        row["resets_in"]
            .as_i64()
            .expect("the window still says when it resets")
            > 0,
        "the reset is still ahead: {after}"
    );
    assert_eq!(
        row["session"], "sess-abc12345-6789",
        "which session reported it survives too: {after}"
    );
}

#[test]
fn a_reading_whose_window_reset_while_the_daemon_was_down_is_not_current() {
    let mut daemon = Daemon::start();

    // A window that ends in a second, read at 68%.
    let resets_at = now() + 1;
    daemon.run(&["hook", "statusline"], &status_line(68.0, resets_at));
    assert_eq!(
        five_hour(&daemon.usage()).expect("the window")["used_tenths"],
        680
    );

    daemon.kill_server();
    // Past the reset, so what was stored describes a window that is over.
    std::thread::sleep(Duration::from_millis(1500));
    daemon.spawn();

    let after = daemon.usage();
    let row = five_hour(&after).expect("the window is still worth a line");
    assert_eq!(row["reset"], true, "the window reset: {after}");
    assert!(
        row["used_tenths"].is_null(),
        "68% of a window that is over is not this window's number: {after}"
    );
}
