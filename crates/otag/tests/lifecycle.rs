//! Regression tests for daemon isolation, ownership and process-tree shutdown.
mod common;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use common::TestHome;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use otag_core::paths::Dirs;
use otag_core::process;
use serde_json::{Value, json};

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        common::stop_child(&mut self.0);
    }
}

fn spawn(home: &TestHome, extra: &[&str], default_state: bool) -> OwnedChild {
    let log = std::fs::File::create(home.path().join("daemon.log")).expect("log");
    let mut command = home.command();
    command
        .arg("daemon")
        .args(extra)
        .stdout(log.try_clone().expect("log clone"))
        .stderr(log);
    if !extra.contains(&"--exit-with-parent") {
        command
            .arg("--exit-with-parent")
            .arg(std::process::id().to_string());
    }
    if !extra.contains(&"--socket") {
        command.arg("--socket").arg(home.socket());
    }
    if default_state {
        command.env_remove("OTAG_STATE_DIR");
    } else {
        command.arg("--state").arg(home.path().join("state"));
    }
    OwnedChild(command.spawn().expect("daemon starts"))
}

fn until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    panic!("timed out waiting for {what}");
}

fn listening(home: &TestHome, child: &mut OwnedChild) {
    until("daemon to listen", || {
        assert!(
            child.0.try_wait().expect("status").is_none(),
            "daemon exited: {}",
            std::fs::read_to_string(home.path().join("daemon.log")).unwrap_or_default()
        );
        UnixStream::connect(home.socket()).is_ok()
    });
}

fn request(home: &TestHome, value: Value) -> Value {
    let mut stream = UnixStream::connect(home.socket()).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    writeln!(stream, "{value}").expect("request");
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .expect("response");
    serde_json::from_str(&line).expect("JSON response")
}

#[test]
fn closed_sessions_and_teams_do_not_return_after_a_daemon_restart() {
    use otag_core::proto::Bytes;

    let home = TestHome::new().expect("home");
    let project = home.path().join("project");
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(&project)
            .status()
            .expect("git init")
            .success()
    );
    let mut daemon = spawn(&home, &[], false);
    listening(&home, &mut daemon);
    let send = |value: Value| {
        let mut stream = UnixStream::connect(home.socket()).expect("connect");
        writeln!(stream, "{value}").expect("send request without a reply");
    };
    let panes = || {
        request(&home, json!({"type":"list_panes"}))["panes"]
            .as_array()
            .expect("panes")
            .clone()
    };
    let start_team = || {
        let result = request(
            &home,
            json!({"type":"start_team","cwd":project,"task":"Test team closure.","roles":[
                {"name":"Plan"},{"name":"Coder"},{"name":"Review"}
            ]}),
        );
        assert_eq!(result["type"], "session_started", "{result}");
        result["pane"].clone()
    };
    let start = request(&home, json!({"type":"start_session","cwd":project}));
    assert_eq!(start["type"], "session_started");
    let running = start["pane"].clone();
    let command = r#"set -m; /bin/sh -c 'trap "" HUP; echo $$ > "$HOME/job.pid"; while :; do sleep 1; done' &"#;
    send(
        json!({"type":"pane_input","pane":running,"bytes":Bytes::from(format!("{command}\n").into_bytes())}),
    );
    let job_file = home.path().join("job.pid");
    until("job PID", || job_file.exists());
    let job: i32 = std::fs::read_to_string(job_file)
        .expect("job PID")
        .trim()
        .parse()
        .expect("PID number");
    let before = process::table().expect("process table");
    let owned = process::descendants(&before, &[daemon.0.id() as i32]);
    assert!(owned.iter().any(|p| p.pid == job));
    send(json!({"type":"hook","event":"PostToolUse","pane":running,"payload":"{}"}));
    until("active work", || {
        panes().iter().any(|p| p["status"] == "working")
    });
    let close = json!({"type":"close_pane","pane":running});
    assert_eq!(
        request(&home, close.clone())["type"],
        "close_confirmation_required"
    );
    assert_eq!(panes()[0]["id"], running);
    assert_eq!(
        request(
            &home,
            json!({"type":"close_pane","pane":running,"confirmed":true})
        )["type"],
        "ok"
    );
    assert!(panes().is_empty());
    assert_eq!(request(&home, close)["type"], "ok");
    until("the pane process tree to stop", || {
        let current = process::table().expect("process table");
        !owned
            .iter()
            .filter(|p| p.pid != daemon.0.id() as i32)
            .any(|p| current.iter().any(|c| p.same(c)))
    });

    let exited = start_team();
    send(json!({"type":"pane_input","pane":exited,"bytes":Bytes::from(b"exit 0\n".as_slice())}));
    until("the team role to exit", || {
        panes()
            .iter()
            .any(|p| p["id"] == exited && !p["exit"].is_null())
    });
    assert_eq!(
        request(&home, json!({"type":"close_pane","pane":exited}))["type"],
        "ok"
    );
    let remaining = panes();
    assert_eq!(remaining.len(), 2);
    assert_eq!(remaining[0]["team"]["index"], 1);
    assert_eq!(remaining[0]["team"]["waiting"], true);
    for role in remaining {
        assert_eq!(
            request(&home, json!({"type":"close_pane","pane":role["id"]}))["type"],
            "ok"
        );
    }
    assert!(panes().is_empty());

    let team = start_team();
    let store = otag_persist::Store::open(
        home.path()
            .join("state")
            .join(otag_persist::DATABASE_FILE_NAME),
    )
    .expect("state database");
    until("the live team to be saved", || {
        store
            .load_session()
            .expect("read state")
            .is_some_and(|s| s.teams.len() == 1 && s.panes.len() == 3)
    });
    send(json!({"type":"hook","event":"PostToolUse","pane":team,"payload":"{}"}));
    until("active team work", || {
        panes().iter().any(|p| p["status"] == "working")
    });
    assert_eq!(
        request(&home, json!({"type":"end_team","team":team}))["type"],
        "close_confirmation_required"
    );
    assert_eq!(panes().len(), 3);
    assert_eq!(
        request(
            &home,
            json!({"type":"end_team","team":team,"confirmed":true})
        )["type"],
        "ok"
    );
    assert!(panes().is_empty());
    let saved = store
        .load_session()
        .expect("read state")
        .expect("saved close");
    assert!(saved.workspaces.is_empty() && saved.panes.is_empty() && saved.teams.is_empty());
    drop(store);

    // A crash immediately after acknowledgment cannot rely on shutdown saving.
    daemon.0.kill().expect("stop daemon without a final save");
    daemon.0.wait().expect("reap daemon");
    let mut restarted = spawn(&home, &[], false);
    listening(&home, &mut restarted);
    assert!(panes().is_empty());
    let next = request(&home, json!({"type":"start_session","cwd":project}));
    assert!(next["pane"].as_u64().expect("new pane ID") > team.as_u64().expect("closed team ID"));
}

#[tokio::test]
async fn a_custom_socket_never_restores_or_writes_the_default_home_database() {
    use otag_core::pane::{PaneSize, PaneSpec};
    use otag_core::session::Session;
    let home = TestHome::new().expect("home");
    let dirs = Dirs {
        home: Some(home.path().to_owned()),
        state_home: Some(home.path().join("xdg-state")),
        ..Dirs::default()
    };
    let database = dirs
        .state_dir()
        .expect("default state")
        .join(otag_persist::DATABASE_FILE_NAME);
    let sentinel = home.path().join("must-not-run");
    let session = Session::new(PaneSpec::command(["/bin/sh", "-c", "sleep 30"]));
    session
        .attach(PaneSize::new(24, 80), Some(home.path()))
        .await
        .expect("fixture session");
    let mut saved = session.snapshot();
    saved.panes[0].command = vec![
        "/bin/sh".into(),
        "-c".into(),
        format!("touch '{}'; sleep 30", sentinel.display()),
    ];
    let store = otag_persist::Store::open(&database).expect("sentinel database");
    store.save_session(&saved).expect("snapshot");
    drop(store);
    let before = std::fs::read(&database).expect("database bytes");
    let mut child = spawn(&home, &[], true);
    listening(&home, &mut child);
    let panes = request(&home, json!({"type":"list_panes"}));
    assert_eq!(
        panes["panes"],
        json!([]),
        "the unrelated snapshot must not be restored"
    );
    assert!(!sentinel.exists(), "no saved command was run");
    request(&home, json!({"type":"shutdown"}));
    until("shutdown", || child.0.try_wait().expect("status").is_some());
    assert_eq!(
        std::fs::read(&database).expect("database bytes"),
        before,
        "the unrelated database is untouched"
    );
    assert!(
        home.socket()
            .with_extension("sock.state")
            .join(otag_persist::DATABASE_FILE_NAME)
            .exists()
    );
}

#[test]
fn a_second_daemon_cannot_open_the_same_state_on_a_different_socket() {
    let home = TestHome::new().expect("home");
    let mut first = spawn(&home, &[], false);
    listening(&home, &mut first);
    let output = home
        .command()
        .args(["daemon", "--socket"])
        .arg(home.path().join("second.sock"))
        .output()
        .expect("second daemon");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("another otag daemon owns state directory")
    );
    assert!(!home.path().join("second.sock").exists());
}

#[test]
fn removing_the_socket_or_its_directory_stops_the_daemon() {
    for directory in [false, true] {
        let home = TestHome::new().expect("home");
        let socket = home.path().join("endpoint/otag.sock");
        let mut child = spawn(&home, &["--socket", socket.to_str().expect("path")], false);
        until("socket", || UnixStream::connect(&socket).is_ok());
        if directory {
            std::fs::remove_dir_all(socket.parent().expect("directory")).expect("remove directory");
        } else {
            std::fs::remove_file(&socket).expect("remove socket");
        }
        until("daemon to exit", || {
            child.0.try_wait().expect("status").is_some()
        });
    }
}

#[test]
fn daemon_shutdown_kills_foreground_jobs_that_ignore_hangup() {
    for signal in [None, Some(Signal::SIGTERM), Some(Signal::SIGINT)] {
        let home = TestHome::new().expect("home");
        let script = home.path().join("shell");
        // Interactive job control gives the background child a separate group.
        // Both shell and child ignore HUP, exercising SIGKILL escalation.
        std::fs::write(&script, format!("#!/bin/sh\ntrap '' HUP\nset -m\n/bin/sh -c 'trap \"\" HUP; echo $$ > \"$HOME/job.pid\"; while :; do sleep 1; done' &\necho $$ > '{}/shell.pid'\nwait\n", home.path().display())).expect("shell");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("executable");
        let mut command = home.command();
        command
            .args(["daemon", "--exit-with-parent"])
            .arg(std::process::id().to_string())
            .env("SHELL", &script)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = OwnedChild(command.spawn().expect("daemon"));
        listening(&home, &mut child);
        request(&home, json!({"type":"start_session","cwd":home.path()}));
        until("job PID", || home.path().join("job.pid").exists());
        let job: i32 = std::fs::read_to_string(home.path().join("job.pid"))
            .expect("PID")
            .trim()
            .parse()
            .expect("PID number");
        let before = process::table().expect("process table");
        let owned = process::descendants(&before, &[child.0.id() as i32]);
        assert!(
            owned.iter().any(|p| p.pid == job),
            "the job belongs to this daemon"
        );
        if let Some(signal) = signal {
            kill(Pid::from_raw(child.0.id() as i32), signal).expect("signal");
        } else {
            request(&home, json!({"type":"shutdown"}));
        }
        until("daemon shutdown", || {
            child.0.try_wait().expect("status").is_some()
        });
        until("every owned descendant to exit", || {
            let current = process::table().expect("process table");
            !owned.iter().any(|p| current.iter().any(|c| p.same(c)))
        });
    }
}

#[test]
fn exiting_parent_stops_the_daemon() {
    let home = TestHome::new().expect("home");
    let mut parent = std::process::Command::new("/bin/sh")
        .args(["-c", "sleep 30"])
        .spawn()
        .expect("parent");
    let mut child = spawn(
        &home,
        &["--exit-with-parent", &parent.id().to_string()],
        false,
    );
    listening(&home, &mut child);
    parent.kill().expect("parent death");
    parent.wait().expect("reap parent");
    until("daemon to follow parent", || {
        child.0.try_wait().expect("status").is_some()
    });
}

#[test]
fn cleanup_requires_confirmation_and_preserves_default_and_unrelated_processes() {
    let home = TestHome::new().expect("home");
    let dirs = Dirs {
        home: Some(home.path().to_owned()),
        state_home: Some(home.path().join("xdg-state")),
        runtime_dir: Some(home.path().join("run")),
        ..Dirs::default()
    };
    let default = dirs.socket_path().expect("default socket");
    let mut protected = spawn(&home, &["--socket", default.to_str().expect("path")], false);
    until("default daemon", || UnixStream::connect(&default).is_ok());
    let stray = TestHome::new().expect("stray home");
    let mut leaked = spawn(&stray, &[], false);
    listening(&stray, &mut leaked);
    request(&stray, json!({"type":"start_session", "cwd":stray.path()}));
    let descendants = process::descendants(
        &process::table().expect("process table"),
        &[leaked.0.id() as i32],
    );
    let mut unrelated = OwnedChild(
        std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("unrelated process"),
    );
    let mut cleanup = home.command();
    cleanup
        .args(["--state"])
        .arg(stray.path().join("state"))
        .arg("cleanup");
    let refused = cleanup.output().expect("cleanup preview");
    assert!(
        !refused.status.success(),
        "noninteractive cleanup requires --yes; stdout={} stderr={}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(leaked.0.try_wait().expect("status").is_none());
    let stopped = cleanup.arg("--yes").output().expect("cleanup");
    assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    until("stray daemon exit", || {
        leaked.0.try_wait().expect("status").is_some()
    });
    until("stray descendants exit", || {
        let now = process::table().expect("process table");
        !descendants.iter().any(|p| now.iter().any(|c| p.same(c)))
    });
    assert!(
        protected.0.try_wait().expect("status").is_none(),
        "default daemon is protected"
    );
    assert!(
        unrelated.0.try_wait().expect("status").is_none(),
        "unrelated process is protected"
    );
}
