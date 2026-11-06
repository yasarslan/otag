//! Team integration uses only isolated manual shells, never real agent accounts.
mod common;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::Stdio;
use std::time::{Duration, Instant};

fn request(home: &common::TestHome, value: Value) -> Value {
    let mut stream = UnixStream::connect(home.socket()).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    writeln!(stream, "{value}").expect("send");
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).expect("answer");
    serde_json::from_str(&line).expect("JSON")
}

fn wait_for_turn(home: &common::TestHome, pane: &Value) {
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        let state = request(home, json!({"type":"list_panes"}));
        let active = state["panes"]
            .as_array()
            .expect("panes")
            .iter()
            .find(|p| p["id"] == *pane)
            .expect("role");
        if active["team"]["waiting"] == false {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "queued role never received its write turn"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn shells_are_manual_steps_and_stale_advances_cannot_skip_roles() {
    let home = common::TestHome::new().expect("isolated home");
    let project = home.path().join("project");
    std::fs::create_dir(&project).expect("project");
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&project)
            .status()
            .expect("git")
            .success()
    );
    let mut child = home
        .command()
        .arg("daemon")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("daemon");
    let deadline = Instant::now() + Duration::from_secs(5);
    while UnixStream::connect(home.socket()).is_err() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let created = request(
        &home,
        json!({"type":"start_team","cwd":project,"roles":[
            {"name":"Plan","agent":null},{"name":"Coder","agent":null},{"name":"Review","agent":null}
        ]}),
    );
    assert_eq!(created["type"], "session_started");
    let panes = request(&home, json!({"type":"list_panes"}));
    let panes = panes["panes"].as_array().expect("panes");
    assert_eq!(panes.len(), 3);
    assert_eq!(panes[0]["nickname"], "Plan");
    assert_eq!(panes[0]["team"]["manual"], true);
    assert_eq!(panes[0]["team"]["automatic"], false);
    assert_eq!(panes[1]["team"]["waiting"], true);
    let nested = project.join("nested");
    std::fs::create_dir(&nested).expect("nested project");
    let second = request(
        &home,
        json!({"type":"start_team","cwd":nested,"roles":[
            {"name":"Second plan","agent":null},{"name":"Second coder","agent":null},{"name":"Second review","agent":null}
        ]}),
    );
    assert_eq!(second["type"], "session_started", "{second}");
    let state = request(&home, json!({"type":"list_panes"}));
    let queued = state["panes"]
        .as_array()
        .expect("panes")
        .iter()
        .find(|p| p["id"] == second["pane"])
        .expect("queued role");
    assert_eq!(
        queued["team"]["waiting"], true,
        "a second shell cannot acquire the first shell's write turn"
    );
    let advance = json!({"type":"team_advance","pane":panes[0]["id"],"revision":panes[0]["team"]["revision"]});
    assert_eq!(request(&home, advance.clone())["type"], "ok");
    assert_eq!(request(&home, advance)["type"], "error");
    wait_for_turn(&home, &second["pane"]);
    let second_state = request(&home, json!({"type":"list_panes"}));
    let second_plan = second_state["panes"]
        .as_array()
        .expect("panes")
        .iter()
        .find(|p| p["id"] == second["pane"])
        .expect("second plan");
    assert_eq!(
        request(
            &home,
            json!({"type":"team_advance","pane":second_plan["id"],"revision":second_plan["team"]["revision"]})
        )["type"],
        "ok"
    );
    wait_for_turn(&home, &panes[1]["id"]);
    let current = request(&home, json!({"type":"list_panes"}));
    let coder = &current["panes"][1];
    assert_eq!(coder["team"]["current"], true);
    assert_eq!(coder["team"]["waiting"], false);
    std::fs::write(project.join("code.txt"), "step change").expect("simulated shell edit");
    let preview = request(
        &home,
        json!({"type":"team_undo_preview","pane":coder["id"],"step":coder["team"]["undo_step"]}),
    );
    assert_eq!(preview["type"], "team_undo_preview", "{preview}");
    assert_eq!(preview["preview"]["files"], json!(["code.txt"]));
    assert_eq!(
        request(
            &home,
            json!({"type":"team_undo_confirm","team":preview["preview"]["team"],"step":preview["preview"]["step"],"expected":preview["preview"]["expected"]})
        )["type"],
        "ok"
    );
    assert!(!project.join("code.txt").exists());
    assert!(
        !project.join(".otag/team").exists(),
        "shell output must not be captured"
    );
    request(&home, json!({"type":"shutdown"}));
    common::stop_child(&mut child);
    let mut restored = home
        .command()
        .arg("daemon")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("restart");
    let deadline = Instant::now() + Duration::from_secs(5);
    while UnixStream::connect(home.socket()).is_err() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let state = request(&home, json!({"type":"list_panes"}));
    let roles = state["panes"].as_array().expect("restored roles");
    assert_eq!(roles.len(), 6);
    assert!(
        roles.iter().all(|p| p["team"]["waiting"] == true),
        "restart must not silently reacquire a write turn"
    );
    request(&home, json!({"type":"shutdown"}));
    common::stop_child(&mut restored);
}
