//! The bridge between an agent and otag, driven the way an agent drives it:
//! the real binary, a settings file of its own, and no daemon listening.
//!
//! Nothing here may touch the machine's own configuration. Every command runs
//! with `HOME` pointing at a temporary directory, so the settings file, the
//! backup and otag's state all live and die with the test.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// A status line the user already had, which otag has to keep working.
const THEIR_STATUS_LINE: &str = "printf 'their own status line'";

/// What `otag hook` may take, measured on the whole process.
///
/// The budget for the work itself is 50 ms; this is the same budget plus room
/// for starting a process on a loaded machine, because what the agent waits
/// for is the process, not the function.
const BUDGET: Duration = Duration::from_millis(1500);

/// Run otag with a temporary home and no daemon to talk to.
fn otag(home: &Path, arguments: &[&str], stdin: &str) -> (String, bool, Duration) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_otag"))
        .args(arguments)
        .env("HOME", home)
        .env("OTAG_STATE_DIR", home.join("state"))
        // An XDG machine would otherwise place otag's state outside the
        // temporary home.
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_RUNTIME_DIR", home.join("run"))
        .env("OTAG_SOCKET", home.join("nothing-is-listening.sock"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("otag can be started");

    let started = Instant::now();
    child
        .stdin
        .take()
        .expect("a pipe")
        .write_all(stdin.as_bytes())
        .expect("the payload can be written");
    let output = child.wait_with_output().expect("otag finishes");

    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        output.status.success(),
        started.elapsed(),
    )
}

#[test]
fn the_bridge_is_installed_used_and_removed_without_losing_what_the_user_had() {
    let home = tempfile::tempdir().expect("a temporary home");
    let home = home.path();
    let settings = home.join(".claude").join("settings.json");
    std::fs::create_dir_all(settings.parent().expect("a parent")).expect("a settings directory");
    std::fs::write(
        &settings,
        format!(r#"{{"statusLine":{{"type":"command","command":"{THEIR_STATUS_LINE}"}}}}"#),
    )
    .expect("settings the user already had");

    let settings_argument = settings.display().to_string();
    let (output, ok, _) = otag(
        home,
        &["install-hooks", "--settings", &settings_argument, "--yes"],
        "",
    );
    assert!(ok, "installing the bridge succeeds: {output}");

    let installed = std::fs::read_to_string(&settings).expect("the settings file");
    assert!(
        installed.contains("hook SessionStart") && installed.contains("hook Stop"),
        "the events otag needs are registered: {installed}"
    );
    assert!(
        installed.contains("hook statusline"),
        "otag's status line is registered: {installed}"
    );
    assert!(
        !installed.contains(THEIR_STATUS_LINE),
        "the user's own status line moved out of the settings file: {installed}"
    );

    // Their status line still runs, because otag runs it.
    let (output, ok, _) = otag(home, &["hook", "statusline"], r#"{"session_id":"x"}"#);
    assert!(ok, "the status line hook succeeds without a daemon");
    assert_eq!(output, "their own status line");

    let (output, ok, _) = otag(
        home,
        &["uninstall-hooks", "--settings", &settings_argument, "--yes"],
        "",
    );
    assert!(ok, "removing the bridge succeeds: {output}");

    let removed = std::fs::read_to_string(&settings).expect("the settings file");
    assert!(
        !removed.contains("otag"),
        "nothing of otag is left behind: {removed}"
    );
    assert!(
        removed.contains(THEIR_STATUS_LINE),
        "the user's own status line is back: {removed}"
    );
}

#[test]
fn a_hook_with_no_daemon_is_quick_and_quiet() {
    let home = tempfile::tempdir().expect("a temporary home");

    let (output, ok, took) = otag(
        home.path(),
        &["hook", "Stop"],
        r#"{"session_id":"abc","hook_event_name":"Stop"}"#,
    );

    assert!(ok, "a hook never fails the agent that called it");
    assert!(output.is_empty(), "a hook says nothing: {output:?}");
    assert!(
        took < BUDGET,
        "a hook runs inside the agent's turn, but took {took:?}"
    );
}

#[test]
fn installing_the_bridge_twice_changes_nothing_the_second_time() {
    let home = tempfile::tempdir().expect("a temporary home");
    let settings = home.path().join("settings.json");
    std::fs::write(&settings, "{}").expect("an empty settings file");
    let settings_argument = settings.display().to_string();

    let arguments = ["install-hooks", "--settings", &settings_argument, "--yes"];
    let (_, ok, _) = otag(home.path(), &arguments, "");
    assert!(ok);
    let once = std::fs::read_to_string(&settings).expect("the settings file");

    let (output, ok, _) = otag(home.path(), &arguments, "");
    assert!(ok, "{output}");
    assert!(
        output.contains("already installed"),
        "the second run says so: {output}"
    );
    assert_eq!(
        std::fs::read_to_string(&settings).expect("the settings file"),
        once,
        "the file is left exactly as it was"
    );
}

#[test]
fn a_bridge_command_without_a_terminal_refuses_rather_than_assuming_consent() {
    let home = tempfile::tempdir().expect("a temporary home");
    let settings = home.path().join("settings.json");
    std::fs::write(&settings, "{}").expect("an empty settings file");

    let (_, ok, _) = otag(
        home.path(),
        &[
            "install-hooks",
            "--settings",
            &settings.display().to_string(),
        ],
        "",
    );

    assert!(!ok, "a script may not have otag change settings unasked");
    assert_eq!(
        std::fs::read_to_string(&settings).expect("the settings file"),
        "{}",
        "nothing was written"
    );
}
