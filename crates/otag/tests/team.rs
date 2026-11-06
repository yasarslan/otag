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

#[test]
fn shells_are_manual_steps_and_stale_advances_cannot_skip_roles() {
    let home = common::TestHome::new().expect("isolated home");
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
        json!({"type":"start_team","cwd":home.path(),"roles":[
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
    let advance = json!({"type":"team_advance","pane":panes[0]["id"],"revision":panes[0]["team"]["revision"]});
    assert_eq!(request(&home, advance.clone())["type"], "ok");
    assert_eq!(request(&home, advance)["type"], "error");
    assert!(
        !home.path().join(".otag/team").exists(),
        "shell output must not be captured"
    );
    request(&home, json!({"type":"shutdown"}));
    common::stop_child(&mut child);
}
