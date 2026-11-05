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
mod common;

/// How long to wait for a daemon to start listening.
const STARTUP: Duration = Duration::from_secs(10);

/// A daemon with a home of its own, so nothing on the machine is touched.
struct Daemon {
    home: common::TestHome,
    socket: std::path::PathBuf,
    process: Option<Child>,
}

impl Daemon {
    fn start() -> Self {
        let home = common::TestHome::new().expect("a temporary home");
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
        common::configure(&mut command, self.home.path(), &self.socket);
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
            // Binding precedes restore. Use a non-starting protocol request,
            // not ping (which could spawn a second, unowned daemon).
            if self
                .run(&["version"], "")
                .contains(&format!("daemon {}", env!("CARGO_PKG_VERSION")))
            {
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
            common::stop_child(&mut process);
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
    assert_eq!(row["resets_at"], resets_at);

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
        row["used_tenths"], 250,
        "a reading of a window that is still running is still the reading: {after}"
    );
    assert_eq!(
        row["resets_at"], resets_at,
        "and it still says when that window resets: {after}"
    );
}

#[test]
fn a_reading_whose_window_reset_while_the_daemon_was_down_reads_nothing_used() {
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
    assert_eq!(
        row["used_tenths"], 0,
        "the window reset, so the allowance is back: {after}"
    );
    assert!(
        row["resets_at"].is_null(),
        "and otag does not know when the new one ends until an agent says: {after}"
    );
}

#[test]
fn a_reading_outlives_every_payload_that_carries_no_limits() {
    let mut daemon = Daemon::start();

    // The session doing the work reports 69% of a window that resets in
    // three hours.
    let resets_at = now() + 3 * 60 * 60;
    daemon.run(&["hook", "statusline"], &status_line(69.0, resets_at));

    // Payloads without `rate_limits` arrive from everywhere: another session
    // on the machine, one resumed from last week before its first reply, an
    // account on an API key, and a session whose windows are explicitly
    // null. None of them knows anything about the limits, so none of them
    // may touch what otag was told.
    let silent = [
        serde_json::json!({ "session_id": "sess-other-00000000" }).to_string(),
        serde_json::json!({
            "session_id": "sess-resumed-11111111",
            "transcript_path": "/tmp/otag-test/resumed.jsonl",
        })
        .to_string(),
        serde_json::json!({
            "session_id": "sess-null-22222222",
            "rate_limits": serde_json::Value::Null,
        })
        .to_string(),
        serde_json::json!({
            "session_id": "sess-empty-33333333",
            "rate_limits": { "five_hour": serde_json::Value::Null, "seven_day": serde_json::Value::Null },
        })
        .to_string(),
        serde_json::json!({
            "session_id": "sess-partial-44444444",
            // A window with a reset but no percentage is not a reading.
            "rate_limits": { "five_hour": { "resets_at": now() - 7 * 24 * 60 * 60 } },
        })
        .to_string(),
    ];
    for payload in &silent {
        daemon.run(&["hook", "statusline"], payload);
        let report = daemon.usage();
        let row = five_hour(&report).unwrap_or_else(|| {
            panic!("a payload with no limits took the window away: {report}");
        });
        assert_eq!(
            row["used_tenths"], 690,
            "a payload that says nothing about the limits says nothing: {report}\n{payload}"
        );
        assert_eq!(row["resets_at"], resets_at, "{report}\n{payload}");
    }

    // And a restart, which reads the readings back out of the database.
    daemon.kill_server();
    daemon.spawn();

    let after = daemon.usage();
    let row = five_hour(&after).unwrap_or_else(|| {
        panic!(
            "the window is gone after a restart: {after}\n{}",
            daemon.log()
        )
    });
    assert_eq!(
        row["used_tenths"], 690,
        "the last thing an agent actually said is still 69%: {after}"
    );
    assert_eq!(
        row["resets_at"], resets_at,
        "resetting at the time it was given: {after}"
    );
}

#[test]
fn a_session_reporting_a_window_that_has_ended_cannot_undo_the_one_running() {
    let daemon = Daemon::start();

    // The session doing the work: 69% of the window running now.
    let resets_at = now() + 3 * 60 * 60;
    daemon.run(&["hook", "statusline"], &status_line(69.0, resets_at));

    // A session resumed from last week re-runs its status line on a timer
    // and reports the snapshot it was handed then: a higher percentage of a
    // window that ended days ago. It arrives later and is still older news.
    let ended = now() - 11 * 24 * 60 * 60;
    daemon.run(
        &["hook", "statusline"],
        &serde_json::json!({
            "session_id": "sess-resumed-55555555",
            "rate_limits": {
                "five_hour": { "used_percentage": 85.0, "resets_at": ended },
            },
        })
        .to_string(),
    );

    let after = daemon.usage();
    let row = five_hour(&after).expect("the window");
    assert_eq!(
        row["used_tenths"], 690,
        "a reading of a window that is over says nothing about this one: {after}"
    );
    assert_eq!(row["resets_at"], resets_at, "{after}");
}
