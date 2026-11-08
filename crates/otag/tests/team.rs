//! Team integration uses isolated shells and a fake agent.
mod common;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::process::Stdio;
use std::time::{Duration, Instant};

fn request(home: &common::TestHome, value: Value) -> Value {
    receive(&mut connect(home, value))
}

fn connect(home: &common::TestHome, value: Value) -> BufReader<UnixStream> {
    let mut stream = UnixStream::connect(home.socket()).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    writeln!(stream, "{value}").expect("send");
    BufReader::new(stream)
}

fn receive(stream: &mut BufReader<UnixStream>) -> Value {
    let mut line = String::new();
    stream.read_line(&mut line).expect("answer");
    serde_json::from_str(&line).expect("JSON")
}

fn until(message: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "{message}");
        std::thread::sleep(Duration::from_millis(35));
    }
}

fn hook(home: &common::TestHome, pane: &Value, event: &str, payload: Value) {
    let mut stream = connect(
        home,
        json!({"type":"hook","event":event,"pane":pane,"payload":payload.to_string()}),
    );
    writeln!(stream.get_mut(), "{}", json!({"type":"ping"})).expect("wait for hook");
    assert_eq!(receive(&mut stream)["type"], "pong");
}

#[test]
fn automatic_team_handoff_waits_for_attention_and_survives_restart() {
    let home = common::TestHome::new().expect("isolated home");
    let project = home.path().join("project");
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(&project)
            .status()
            .expect("git")
            .success()
    );
    let bin = home.path().join(".local/bin");
    std::fs::create_dir_all(&bin).expect("native agent directory");
    let agent = bin.join("claude");
    std::fs::write(
        &agent,
        r#"#!/bin/sh
if [ "$1" = --version ]; then echo 2.1.285; exit; fi
printf 'Claude welcome screen\n'
printf '%s\n' "$@" > "$HOME/role-$OTAG_PANE_ID"
while :; do /bin/sleep 10; done
"#,
    )
    .expect("fake agent");
    std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).expect("executable");
    let mut path = vec![bin];
    path.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let path = std::env::join_paths(path).expect("test PATH");
    let spawn = || {
        let child = home
            .command()
            .arg("daemon")
            .env("PATH", &path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("daemon");
        until("daemon did not start", || {
            UnixStream::connect(home.socket()).is_ok()
        });
        child
    };
    let mut daemon = spawn();
    let panes = || {
        request(&home, json!({"type":"list_panes"}))["panes"]
            .as_array()
            .expect("panes")
            .clone()
    };
    let find = |id: &Value| {
        panes()
            .into_iter()
            .find(|pane| pane["id"] == *id)
            .expect("role")
    };
    let start = || {
        let response = request(
            &home,
            json!({"type":"start_team","cwd":project,"task":"Audit the sample project.","roles":[
                {"name":"Plan","agent":"claude"},{"name":"Coder","agent":"claude","can_edit":true},
                {"name":"Review","agent":"claude"},{"name":"Custom","agent":"claude"}
            ]}),
        );
        assert_eq!(response["type"], "session_started", "{response}");
        response["pane"].clone()
    };
    let automatic = |team: &Value, enabled| {
        assert_eq!(
            request(
                &home,
                json!({"type":"team_mode","team":team,"automatic":enabled})
            )["type"],
            "ok"
        );
    };
    let result = |id: &Value, fixture: &str| {
        let path = home.path().join(format!("result-{id}.jsonl"));
        std::fs::write(&path, fixture).expect("test transcript");
        hook(
            &home,
            id,
            "Stop",
            json!({"session_id":format!("test-{id}"),"transcript_path":path}),
        );
    };
    let ready = include_str!("../../../tests/fixtures/claude-team-final.jsonl");
    let question = include_str!("../../../tests/fixtures/claude-team-question.jsonl");

    let plan = start();
    let plan_args = home.path().join(format!("role-{plan}"));
    until("Plan did not receive the task", || {
        std::fs::read_to_string(&plan_args).is_ok_and(|args| {
            args.contains("--- Team task ---\nAudit the sample project.\n--- End team task ---")
        })
    });
    let members: Vec<_> = panes()
        .into_iter()
        .filter(|p| p["team"]["id"] == plan)
        .collect();
    assert_eq!(members.len(), 4);
    for role in &members[1..] {
        let id = &role["id"];
        let args = home.path().join(format!("role-{id}"));
        until("standby role did not launch", || args.exists());
        assert!(
            !std::fs::read_to_string(args)
                .expect("standby args")
                .contains("Role:")
        );
        let mut stream = connect(&home, json!({"type":"attach_pane","pane":id}));
        loop {
            use otag_core::proto::Response;
            let response = serde_json::from_value(receive(&mut stream)).expect("raw response");
            match response {
                Response::PaneScreen { bytes, .. } | Response::Output { bytes, .. } => {
                    if String::from_utf8_lossy(bytes.as_slice()).contains("Claude welcome screen") {
                        break;
                    }
                }
                other => panic!("standby role did not show its welcome screen: {other:?}"),
            }
        }
    }
    assert_eq!(find(&plan)["team"]["next_role"], "Coder");
    assert_eq!(find(&plan)["team"]["can_advance"], false);
    result(&plan, ready);
    until("the finished transcript did not enable handoff", || {
        find(&plan)["team"]["can_advance"] == true
    });
    std::thread::sleep(Duration::from_millis(2200));
    assert_eq!(find(&plan)["team"]["current"], true);
    automatic(&plan, true);
    hook(&home, &plan, "PreToolUse", json!({"tool_name":"Read"}));
    hook(&home, &plan, "PostToolUse", json!({"tool_name":"Read"}));
    result(&plan, ready);
    hook(
        &home,
        &plan,
        "Notification",
        json!({"notification_type":"idle_prompt"}),
    );
    let coder = &members[1]["id"];
    until("READY did not reach Coder", || {
        let role = find(coder);
        role["team"]["current"] == true && role["team"]["waiting"] == false
    });
    until("Coder never received its instructions", || {
        std::fs::read_to_string(home.path().join(format!("role-{coder}")))
            .is_ok_and(|args| args.contains("Role: Coder"))
    });

    let blocked = ["QUESTION", "FAILED", "PROBLEMS"].map(|marker| {
        let id = start();
        automatic(&id, true);
        result(
            &id,
            &question.replace("OTAG_RESULT: QUESTION", &format!("OTAG_RESULT: {marker}")),
        );
        id
    });
    for id in &blocked {
        until("a blocking result did not pause the team", || {
            find(id)["team"]["needs_you"] == true
        });
        let role = find(id);
        assert_eq!(role["team"]["current"], true);
        assert_eq!(role["team"]["automatic"], true);
        assert_eq!(role["status"], "needs_approval");
    }
    let approval = start();
    automatic(&approval, true);
    hook(
        &home,
        &approval,
        "Notification",
        json!({
            "notification_type":"permission_prompt", "message":"Claude needs permission to use Bash"
        }),
    );
    result(&approval, ready);
    assert_eq!(find(&approval)["team"]["needs_you"], true);
    assert_eq!(find(&approval)["team"]["current"], true);

    let guarded = start();
    automatic(&guarded, true);
    assert_eq!(
        request(
            &home,
            json!({"type":"guard","pane":guarded,
        "plan":{"threshold":65,"when_reached":"wait"}})
        )["type"],
        "ok"
    );
    hook(
        &home,
        &guarded,
        "statusline",
        json!({"rate_limits":{"five_hour":{"used_percentage":70}}}),
    );
    until("limit guard did not stop Plan", || {
        find(&guarded)["guard"]["paused"] == true
    });
    result(&guarded, ready);
    std::thread::sleep(Duration::from_millis(2200));
    assert_eq!(find(&approval)["team"]["current"], true);
    assert_eq!(find(&guarded)["team"]["needs_you"], true);
    assert_eq!(find(&guarded)["team"]["current"], true);

    automatic(&approval, false);
    request(&home, json!({"type":"shutdown"}));
    common::stop_child(&mut daemon);
    let mut restored = spawn();
    assert_eq!(find(&plan)["team"]["automatic"], true);
    assert_eq!(find(coder)["team"]["current"], true);
    assert_eq!(find(coder)["team"]["waiting"], true);
    assert_eq!(find(&approval)["team"]["automatic"], false);
    assert_eq!(find(&guarded)["team"]["needs_you"], true);
    request(&home, json!({"type":"shutdown"}));
    common::stop_child(&mut restored);
}

#[test]
fn a_team_retains_its_screen_and_checks_the_exact_result_before_advancing() {
    use otag_core::{proto::Response, screen::Screen};

    let home = common::TestHome::new().expect("isolated home");
    let project = home.path().join("project");
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(&project)
            .status()
            .expect("git")
            .success()
    );
    let bin = home.path().join(".local/bin");
    let versions = home.path().join(".local/share/claude/versions");
    std::fs::create_dir_all(&bin).expect("native launcher directory");
    std::fs::create_dir_all(&versions).expect("version directory");
    let agent = versions.join("2.1.319");
    std::fs::write(
        &agent,
        r#"#!/bin/sh
if [ "$1" = --version ]; then echo '2.1.319 (Claude Code)'; exit; fi
test "$1" = --permission-mode && test "$2" = dontAsk &&
test "$3" = --setting-sources && test -z "$4" || exit 91
case "$*" in
  *"Role:"*) ;;
  *) printf 'Claude welcome screen\n'; exec /bin/sleep 30 ;;
esac
i=0
while [ "$i" -lt 100 ]; do printf 'plan-history-%03d\n' "$i"; i=$((i+1)); done
printf '\033[?1049h\033[HPlan screen retained\n(esc to interrupt)'
while [ ! -f "$HOME/finish-plan" ]; do /bin/sleep 0.1; done
printf '\r\033[K* Worked for 31s · done 11:08 AM\n❯ '
exec /bin/sleep 30
"#,
    )
    .expect("fake agent");
    std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).expect("executable");
    std::os::unix::fs::symlink(&agent, bin.join("claude")).expect("native launcher symlink");
    let spawn = || {
        let daemon = home
            .command()
            .arg("daemon")
            .env("PATH", "/usr/bin:/bin")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("daemon");
        until("daemon did not start", || {
            UnixStream::connect(home.socket()).is_ok()
        });
        daemon
    };
    let mut daemon = spawn();
    let created = request(
        &home,
        json!({"type":"start_team","cwd":project,"task":"Prepare a sample plan.","roles":[
            {"name":"Custom plan","agent":"claude"},{"name":"Coder"},{"name":"Review"}
        ]}),
    );
    assert_eq!(created["type"], "session_started", "{created}");
    let pane = &created["pane"];
    let current = || request(&home, json!({"type":"list_panes"}))["panes"][0].clone();
    let size = json!({"rows":24,"cols":80});
    let attach = || connect(&home, json!({"type":"attach_pane","pane":pane,"size":size}));
    let mut terminal = attach();
    let mut screen = Screen::new(otag_core::pane::PaneSize::new(24, 80));
    loop {
        match serde_json::from_value(receive(&mut terminal)).expect("raw response") {
            Response::PaneScreen { bytes, .. } | Response::Output { bytes, .. } => {
                screen.process(bytes.as_slice())
            }
            other => panic!("unexpected raw response: {other:?}"),
        }
        if screen.state().text().contains("Plan screen retained") {
            break;
        }
    }
    let read_screen = |reader: &mut BufReader<UnixStream>| {
        let response = serde_json::from_value(receive(reader)).expect("raw screen response");
        let Response::PaneScreen { size, bytes, .. } = response else {
            panic!("expected replay, got {response:?}");
        };
        let mut replayed = Screen::new(size);
        replayed.process(bytes.as_slice());
        replayed
    };
    let has_history = |screen: &mut Screen| {
        screen
            .scrollback_lines(200)
            .iter()
            .any(|line| line.contains("plan-history-010"))
    };
    until(
        "native team role should start Working with the button disabled",
        || {
            let plan = current();
            plan["status"] == "working" && plan["team"]["can_advance"] == false
        },
    );
    let mut reattached = read_screen(&mut attach());
    assert!(reattached.state().text().contains("Plan screen retained"));
    reattached.process(b"\x1b[?1049l");
    assert!(reattached.state().text().contains("plan-history-099"));
    assert!(has_history(&mut reattached));

    request(&home, json!({"type":"shutdown"}));
    common::stop_child(&mut daemon);
    daemon = spawn();
    let mut restored = read_screen(&mut attach());
    assert!(restored.state().text().contains("Plan screen retained"));
    assert!(has_history(&mut restored));
    assert_eq!(current()["team"]["waiting"], true);
    let resume = request(
        &home,
        json!({"type":"team_advance","pane":pane,"revision":current()["team"]["revision"]}),
    );
    assert_eq!(resume["type"], "ok", "{resume}");
    terminal = attach();
    let mut resumed = read_screen(&mut terminal);
    until("the resumed role should be working", || {
        current()["status"] == "working"
    });
    std::fs::write(home.path().join("finish-plan"), "").expect("finish the fake Claude turn");
    until(
        "new Claude footer should enable the team button before transcript discovery",
        || current()["team"]["can_advance"] == true,
    );
    while !resumed.state().modes().alternate_screen
        || !resumed.state().text().contains("done 11:08 AM")
    {
        match serde_json::from_value(receive(&mut terminal)).expect("active role output") {
            Response::Output { bytes, .. } | Response::PaneScreen { bytes, .. } => {
                resumed.process(bytes.as_slice());
            }
            other => panic!("unexpected raw response: {other:?}"),
        }
    }

    let canonical_project = project.canonicalize().expect("canonical project");
    let transcript_dir = home
        .path()
        .join(".claude/projects")
        .join(otag_agents::claude::encode_cwd(&canonical_project));
    std::fs::create_dir_all(&transcript_dir).expect("transcript directory");
    let transcript = transcript_dir.join("native-agent-result.jsonl");
    let fixture = include_str!("../../../tests/fixtures/claude-2.1.319-team-finished.jsonl");
    std::fs::write(&transcript, fixture).expect("transcript");
    // The fake agent execs sleep and never sends hooks, just as the daemon
    // cannot identify a native agent whose process name is its version.
    until(
        "finished transcript should report QUESTION instead of the footer alone",
        || {
            request(
                &home,
                json!({
                    "type":"team_advance", "pane":pane, "revision":current()["team"]["revision"]
                }),
            )["result"]
                == "QUESTION"
        },
    );
    let ready = fixture
        .replace(
            "Which platform should the sample target?",
            "Sample plan complete.",
        )
        .replace("OTAG_RESULT: QUESTION", "OTAG_RESULT: READY");
    std::fs::write(&transcript, ready).expect("READY transcript");
    until("READY should enable advancing Plan", || {
        let plan = current();
        plan["status"] == "done" && plan["team"]["can_advance"] == true
    });
    std::fs::write(&transcript, fixture).expect("QUESTION transcript");
    until("QUESTION should restore Needs you", || {
        current()["status"] == "needs_approval"
    });
    let plan = current();
    assert_eq!(plan["team"]["role_name"], "Custom plan");
    assert!(plan["first_prompt"].is_null());
    let revision = &plan["team"]["revision"];
    let advance = |confirmation: Value| {
        request(
            &home,
            json!({"type":"team_advance","pane":pane,"revision":revision,"confirmation":confirmation}),
        )
    };
    let warning = advance(Value::Null);
    assert_eq!(warning["type"], "team_advance_warning", "{warning}");
    assert_eq!(warning["result"], "QUESTION");
    let mut token = warning["confirmation"].clone();
    for result in ["FAILED", "PROBLEMS"] {
        std::fs::write(
            &transcript,
            fixture.replace("OTAG_RESULT: QUESTION", &format!("OTAG_RESULT: {result}")),
        )
        .expect("new result");
        until("new transcript result was not recorded", || {
            advance(Value::Null)["result"] == result
        });
        let warning = advance(token);
        assert_eq!(warning["type"], "team_advance_warning", "{warning}");
        assert_eq!(warning["result"], result);
        assert_eq!(current()["team"]["revision"], *revision);
        assert!(!project.join(".otag/team").exists());
        token = warning["confirmation"].clone();
    }
    std::fs::write(&transcript, fixture.replace("\\nOTAG_RESULT: QUESTION", ""))
        .expect("transcript without a result marker");
    until("missing marker was not recorded", || {
        advance(Value::Null)["result"] == "MISSING"
    });
    assert_eq!(current()["team"]["can_advance"], true);
    let warning = advance(token);
    assert_eq!(warning["type"], "team_advance_warning", "{warning}");
    assert_eq!(warning["result"], "MISSING");
    let token = warning["confirmation"].clone();
    assert_eq!(advance(token)["type"], "ok");

    // Both the uninterrupted terminal and a new attach after hiding the
    // window must replay the answer and the older output from the same pane.
    for reader in [&mut terminal, &mut attach()] {
        let mut replayed = read_screen(reader);
        assert!(replayed.state().text().contains("Plan screen retained"));
        assert!(has_history(&mut replayed));
    }
    let blank_task = request(
        &home,
        json!({"type":"start_team","cwd":project,"task":"   ","roles":[
            {"name":"Plan"},{"name":"Coder"},{"name":"Review"}
        ]}),
    );
    assert_eq!(blank_task["type"], "error");
    assert_eq!(
        request(&home, json!({"type":"list_panes"}))["panes"]
            .as_array()
            .expect("panes")
            .len(),
        3
    );
    request(&home, json!({"type":"shutdown"}));
    common::stop_child(&mut daemon);
}

#[test]
fn codex_completed_turns_can_be_sent_with_or_without_a_result_marker() {
    let home = common::TestHome::new().expect("isolated home");
    let project = home.path().join("project");
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(&project)
            .status()
            .expect("git")
            .success()
    );
    let bin = home.path().join(".local/bin");
    std::fs::create_dir_all(&bin).expect("native bin");
    let agent = bin.join("codex");
    std::fs::write(
        &agent,
        "#!/bin/sh\nprintf 'Esc to interrupt\\n'\nexec /bin/sleep 30\n",
    )
    .expect("fake agent");
    std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).expect("executable");
    let spawn = || {
        let daemon = home
            .command()
            .arg("daemon")
            .env("PATH", "/usr/bin:/bin")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("daemon");
        until("daemon did not start", || {
            UnixStream::connect(home.socket()).is_ok()
        });
        daemon
    };
    let mut daemon = spawn();
    let rollouts = home.path().join(".codex/sessions/2026/11/08");
    std::fs::create_dir_all(&rollouts).expect("rollout directory");
    let fixture = include_str!("../../../tests/fixtures/codex-team-final.jsonl");
    let metadata =
        json!({"type":"session_meta","payload":{"cwd":project.canonicalize().expect("cwd")}});

    for (index, result) in ["READY", "QUESTION", "MISSING"].into_iter().enumerate() {
        let started = request(
            &home,
            json!({"type":"start_team","cwd":project,"task":"Test handoff.","roles":[
                {"name":"Plan","agent":"codex"},{"name":"Coder"},{"name":"Review"}
            ]}),
        );
        assert_eq!(started["type"], "session_started", "{started}");
        let pane = &started["pane"];
        let plan = || request(&home, json!({"type":"list_panes"}))["panes"][0].clone();
        until(
            "Codex must be working before the completion record arrives",
            || plan()["status"] == "working" && plan()["team"]["can_advance"] == false,
        );
        let transcript = if result == "MISSING" {
            fixture.replace("\\nOTAG_RESULT: READY", "")
        } else {
            fixture.replace("OTAG_RESULT: READY", &format!("OTAG_RESULT: {result}"))
        };
        let file = rollouts.join(format!(
            "rollout-2026-11-08T11-08-00-00000000-0000-4000-8000-{index:012}.jsonl"
        ));
        std::fs::write(&file, format!("{metadata}\n{transcript}")).expect("Codex rollout");
        until("finished Codex turn should enable the button", || {
            plan()["team"]["can_advance"] == true
        });
        if result == "READY" {
            let assert_reconnected = || {
                let mut connection = connect(&home, json!({"type":"subscribe"}));
                loop {
                    let response = receive(&mut connection);
                    let Some(panes) = response["event"]["panes"].as_array() else {
                        continue;
                    };
                    assert_eq!(panes[0]["status"], "done");
                    assert_eq!(panes[0]["team"]["can_advance"], true);
                    assert_eq!(panes[0]["team"]["step_finished"], true);
                    assert_eq!(panes[0]["team"]["waiting"], false);
                    assert_eq!(panes[1]["team"]["waiting"], true);
                    break;
                }
            };
            // SessionStart sets the underlying pane status to Idle without
            // changing its transcript, just like a quiet screen can.
            hook(&home, pane, "SessionStart", json!({}));
            assert_eq!(plan()["status"], "done");
            assert_eq!(plan()["team"]["can_advance"], true);
            assert_reconnected();

            let old_revision = plan()["team"]["revision"].clone();
            let send_input = |bytes| {
                let mut stream = connect(
                    &home,
                    json!({"type":"pane_input","pane":pane,"bytes":bytes}),
                );
                writeln!(stream.get_mut(), "{}", json!({"type":"ping"})).expect("wait for input");
                assert_eq!(receive(&mut stream)["type"], "pong");
            };
            send_input(json!([110, 101, 120, 116])); // Draft "next".
            assert_eq!(plan()["team"]["can_advance"], true);
            send_input(json!([13]));
            assert_eq!(plan()["team"]["can_advance"], false);
            assert_eq!(plan()["team"]["step_finished"], false);
            assert_eq!(plan()["status"], "working");
            assert_ne!(plan()["team"]["revision"], old_revision);
            let blocked = request(
                &home,
                json!({
                    "type":"team_advance", "pane":pane, "revision":plan()["team"]["revision"]
                }),
            );
            assert_eq!(blocked["type"], "error", "{blocked}");

            // A metadata-only write to the old finished transcript must not
            // complete the new turn.
            std::fs::write(&file, format!("{metadata}\n{transcript}\n")).expect("old rollout");
            std::thread::sleep(Duration::from_millis(1100));
            assert_eq!(plan()["team"]["can_advance"], false);
            let second = transcript
                .replace("sample-turn", "next-turn")
                .replace("Tests passed.", "Follow-up tests passed.");
            std::fs::write(&file, format!("{metadata}\n{transcript}\n{second}")).expect("new turn");
            until("follow-up completion did not enable handoff", || {
                plan()["team"]["can_advance"] == true
            });
            request(&home, json!({"type":"shutdown"}));
            common::stop_child(&mut daemon);
            // Sending after restart must use the saved answer.
            std::fs::remove_file(&file).expect("remove fixture rollout before restart");
            daemon = spawn();
            assert_reconnected();
        }
        let revision = plan()["team"]["revision"].clone();
        let advance = |confirmation: Value| {
            request(
                &home,
                json!({
                    "type":"team_advance","pane":pane,"revision":revision,"confirmation":confirmation
                }),
            )
        };
        let response = advance(Value::Null);
        if result == "READY" {
            assert_eq!(response["type"], "ok", "{response}");
            assert_eq!(plan()["status"], "done");
            assert_eq!(plan()["team"]["can_advance"], false);
            let output = project
                .join(".otag/team")
                .join(pane.to_string())
                .join("task.md");
            assert!(
                std::fs::read_to_string(output)
                    .expect("saved result")
                    .contains("Follow-up tests passed.")
            );
        } else {
            assert_eq!(response["type"], "team_advance_warning", "{response}");
            assert_eq!(response["result"], result);
            assert_eq!(plan()["team"]["current"], true);
            assert_eq!(advance(response["confirmation"].clone())["type"], "ok");
        }
        assert_eq!(
            request(
                &home,
                json!({"type":"end_team","team":pane,"confirmed":true})
            )["type"],
            "ok"
        );
        if result != "READY" {
            std::fs::remove_file(file).expect("remove fixture rollout");
        }
    }
    request(&home, json!({"type":"shutdown"}));
    common::stop_child(&mut daemon);
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
        json!({"type":"start_team","cwd":project,"task":"Exercise the write queue.","roles":[
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
        json!({"type":"start_team","cwd":project,"task":"Exercise the second write turn.","roles":[
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
