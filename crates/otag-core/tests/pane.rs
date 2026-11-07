//! Panes running real programs on real pseudo-terminals.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use otag_core::layout::{BORDER, Chrome};
use otag_core::pane::{PaneId, PaneSize, PaneSpec};
use otag_core::pty::{Pty, PtyEvent};
use otag_core::session::{PaneHandle, PaneState, Session};

/// How long a test waits for a child process to do its work. Generous: a
/// loaded machine must not make the suite flaky.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Wait until the pane's screen satisfies `ready`, then return what it is.
async fn wait_for(handle: &PaneHandle, ready: impl Fn(&PaneState) -> bool) -> Arc<PaneState> {
    let mut watch = handle.watch();
    let snapshot = tokio::time::timeout(TIMEOUT, async {
        loop {
            {
                let snapshot = watch.borrow_and_update();
                if ready(&snapshot) {
                    return Arc::clone(&snapshot);
                }
            }
            if watch.changed().await.is_err() {
                // The pane is gone; report its last known state.
                return handle.snapshot();
            }
        }
    })
    .await;

    match snapshot {
        Ok(snapshot) => snapshot,
        Err(_) => {
            let snapshot = handle.snapshot();
            panic!(
                "the pane did not reach the expected state within {:?}.\nscreen:\n{}",
                TIMEOUT,
                snapshot.screen.text()
            )
        }
    }
}

/// Wait until the pane's screen contains `needle`.
async fn wait_for_text(handle: &PaneHandle, needle: &str) -> Arc<PaneState> {
    wait_for(handle, |snapshot| snapshot.screen.text().contains(needle)).await
}

fn shell(script: &str) -> PaneSpec {
    PaneSpec::command(["/bin/sh", "-c", script])
}

/// The size a single pane has in a terminal of this size: what otag's own
/// sidebar, tab bar and border leave of it.
fn inside(size: PaneSize) -> PaneSize {
    Chrome::of(size, true).content.inset(BORDER).size()
}

/// A session whose panes run `script`, attached at `size`.
async fn running(spec: PaneSpec, size: PaneSize) -> (Arc<Session>, PaneHandle) {
    let session = Session::new(spec);
    session
        .attach(size, None)
        .await
        .expect("the session starts a pane");
    let handle = session.focused_pane().expect("a pane to be focused");
    (session, handle)
}

#[tokio::test]
async fn a_pane_shows_what_its_program_prints() {
    let size = PaneSize::new(10, 40);
    let (_session, handle) = running(shell("printf 'hello from a pane\\n'"), size).await;

    let snapshot = wait_for_text(&handle, "hello from a pane").await;
    assert_eq!(snapshot.screen.row_text(0), "hello from a pane");
    assert_eq!(snapshot.screen.size(), inside(size));
}

fn fake_agent(path: &Path, output: &str, mode: u32) {
    fs::write(path, format!("#!/bin/sh\nprintf '%s\\n' '{output}'\n")).expect("write a fake agent");
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set agent permissions");
}

#[tokio::test]
async fn agents_start_from_the_native_install_directory_without_path() {
    let home = tempfile::tempdir().expect("temporary home");
    let local_bin = home.path().join(".local/bin");
    fs::create_dir_all(&local_bin).expect("create the native install directory");

    for agent in ["claude", "codex"] {
        fake_agent(&local_bin.join(agent), "native installation", 0o755);
        let spec = PaneSpec::command([agent])
            .with_env("HOME", home.path())
            .with_env("PATH", "");
        let (_session, handle) = running(spec, PaneSize::DEFAULT).await;
        wait_for_text(&handle, "native installation").await;
    }
}

#[tokio::test]
async fn agent_launch_prefers_the_first_executable_on_the_pane_path() {
    let home = tempfile::tempdir().expect("temporary home");
    let local_bin = home.path().join(".local/bin");
    let first = home.path().join("first");
    let second = home.path().join("second");
    for directory in [&local_bin, &first, &second] {
        fs::create_dir_all(directory).expect("create a bin directory");
    }
    fake_agent(&local_bin.join("claude"), "native installation", 0o755);
    fake_agent(&first.join("claude"), "not executable", 0o644);
    fake_agent(&second.join("claude"), "from pane PATH", 0o755);

    let path = std::env::join_paths(["first", "second"]).expect("join the pane PATH");
    let spec = PaneSpec::command(["claude"])
        .with_cwd(home.path())
        .with_env("HOME", home.path())
        .with_env("PATH", path);
    let (_session, handle) = running(spec, PaneSize::DEFAULT).await;
    wait_for_text(&handle, "from pane PATH").await;
}

#[tokio::test]
async fn a_pane_reports_how_its_program_ended() {
    let (_session, handle) = running(shell("exit 3"), PaneSize::DEFAULT).await;

    let snapshot = wait_for(&handle, |snapshot| snapshot.exit.is_some()).await;
    let exit = snapshot.exit.clone().expect("the program ended");
    assert_eq!(exit.code, Some(3));
    assert_eq!(exit.signal, None);
    assert!(!exit.success());
    assert!(handle.is_finished());
}

#[tokio::test]
async fn a_pane_passes_input_to_its_program() {
    let (_session, handle) = running(
        shell("read line; printf 'got %s\\n' \"$line\""),
        PaneSize::new(10, 40),
    )
    .await;

    handle
        .input(b"typed\r".to_vec())
        .await
        .expect("the pane accepts input");

    wait_for_text(&handle, "got typed").await;
}

#[tokio::test]
async fn a_pane_tells_its_program_about_a_resize() {
    // Keep reporting the size the program is told it has.
    let before = PaneSize::new(24, 80);
    let (session, handle) = running(shell("while :; do stty size; sleep 1; done"), before).await;

    let reported = inside(before);
    wait_for_text(&handle, &format!("{} {}", reported.rows(), reported.cols())).await;

    let after = PaneSize::new(30, 100);
    session.resize(after).await;

    let reported = inside(after);
    let snapshot =
        wait_for_text(&handle, &format!("{} {}", reported.rows(), reported.cols())).await;
    assert_eq!(snapshot.screen.size(), reported);
}

#[tokio::test]
async fn a_pane_answers_the_queries_a_program_makes() {
    // Move the cursor, then ask where it is. No real terminal is involved,
    // so an answer can only come from otag. The pseudo-terminal echoes the
    // input it receives, which is how the answer becomes visible.
    let (_session, handle) = running(
        shell("printf '\\033[3;5H\\033[6n'; sleep 10"),
        PaneSize::new(12, 40),
    )
    .await;

    // The reply reports the one-based position the cursor was moved to.
    let snapshot = wait_for_text(&handle, "[3;5R").await;
    assert!(
        snapshot.screen.text().contains("[3;5R"),
        "the pane did not answer the cursor query.\nscreen:\n{}",
        snapshot.screen.text()
    );
}

#[tokio::test]
async fn a_pane_tells_its_program_which_terminal_it_is_in() {
    let (_session, handle) = running(
        shell("printf 'term=%s color=%s pane=%s\\n' \"$TERM\" \"$COLORTERM\" \"$OTAG_PANE_ID\""),
        PaneSize::new(10, 100),
    )
    .await;

    let snapshot = wait_for_text(&handle, "term=").await;
    assert_eq!(
        snapshot.screen.row_text(0),
        "term=xterm-256color color=truecolor pane=1"
    );
}

#[tokio::test]
async fn a_pane_starts_in_the_directory_the_workspace_was_opened_in() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    // macOS hands out symlinked temporary paths, so compare what the shell
    // itself resolves rather than the path as written.
    let expected = directory
        .path()
        .canonicalize()
        .expect("the directory resolves");

    let session = Session::new(PaneSpec::command(["/bin/sh", "-c", "pwd -P"]));
    session
        .attach(PaneSize::new(10, 200), Some(directory.path()))
        .await
        .expect("the pane starts");
    let handle = session.focused_pane().expect("a pane to be focused");

    let snapshot = wait_for_text(&handle, "/").await;
    assert_eq!(snapshot.screen.row_text(0), expected.display().to_string());
}

#[tokio::test]
async fn a_pane_in_a_missing_directory_fails_with_a_clear_message() {
    let session = Session::new(PaneSpec::shell());
    let error = session
        .attach(
            PaneSize::DEFAULT,
            Some("/no/such/directory/anywhere".as_ref()),
        )
        .await
        .expect_err("the pane cannot start");

    let message = error.to_string();
    assert!(message.contains("/no/such/directory/anywhere"), "{message}");
    assert!(message.contains("not a directory"), "{message}");
    assert!(session.is_empty());
}

#[tokio::test]
async fn a_program_that_cannot_be_started_is_reported() {
    let session = Session::new(PaneSpec::command(["/nonexistent/program"]));
    let error = session
        .attach(PaneSize::DEFAULT, None)
        .await
        .expect_err("the pane cannot start");

    assert!(
        error.to_string().contains("/nonexistent/program"),
        "{error}"
    );
    assert!(session.is_empty());
}

#[tokio::test]
async fn attaching_twice_reaches_the_same_session() {
    let session = Session::new(shell("sleep 10"));
    session
        .attach(PaneSize::DEFAULT, None)
        .await
        .expect("the pane starts");
    let first = session.focused_pane().expect("a pane").id();

    session
        .attach(PaneSize::new(30, 100), None)
        .await
        .expect("the session is found");
    let second = session.focused_pane().expect("a pane").id();

    assert_eq!(first, second, "a second client joins what is there");
    assert_eq!(session.summaries().len(), 1);
}

#[tokio::test]
async fn a_session_whose_last_program_ended_starts_a_fresh_pane_on_the_next_attach() {
    let session = Session::new(shell("exit 0"));
    session
        .attach(PaneSize::DEFAULT, None)
        .await
        .expect("the pane starts");
    let finished = session.focused_pane().expect("a pane");
    wait_for(&finished, |snapshot| snapshot.exit.is_some()).await;

    // The pane leaves the session once its program has ended, which is what
    // makes the session empty rather than full of dead panes.
    let empty = tokio::time::timeout(TIMEOUT, async {
        let mut changes = session.changes();
        while !session.is_empty() {
            if changes.changed().await.is_err() {
                return false;
            }
        }
        true
    })
    .await
    .expect("the session empties");
    assert!(empty);
    assert_eq!(session.last_exit().and_then(|exit| exit.code), Some(0));

    session
        .attach(PaneSize::DEFAULT, None)
        .await
        .expect("a new pane starts");
    let fresh = session.focused_pane().expect("a pane");
    assert_ne!(fresh.id(), finished.id(), "identifiers are never reused");
    assert_eq!(session.summaries().len(), 1);
}

#[tokio::test]
async fn a_pane_the_session_does_not_have_is_not_found() {
    let session = Session::new(shell("sleep 10"));
    assert!(session.pane(PaneId::FIRST).is_none());
    assert!(session.is_empty());

    session
        .attach(PaneSize::DEFAULT, None)
        .await
        .expect("the pane starts");
    let handle = session.focused_pane().expect("a pane");
    assert_eq!(
        session.pane(handle.id()).map(|found| found.id()),
        Some(handle.id())
    );
    assert!(session.pane(handle.id().next()).is_none());
}

#[tokio::test]
async fn a_view_hands_out_panes_that_outlive_the_session_having_them() {
    // What a client gets with a view has to stay usable afterwards: by the
    // time it draws, the program may have printed its last line and ended,
    // and the session may already have let the pane go. The final screen is
    // then only reachable through the handle taken with the view.
    let (session, handle) = running(shell("printf 'goodbye\\n'; exit 7"), PaneSize::DEFAULT).await;
    let (view, panes) = session.view_with_panes();
    assert_eq!(view.panes.len(), 1);
    let kept = panes.get(&handle.id()).expect("the view's pane");

    wait_for_text(&handle, "goodbye").await;
    tokio::time::timeout(TIMEOUT, async {
        while !session.is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the session lets the finished pane go");
    assert!(session.pane(handle.id()).is_none());

    let snapshot = kept.snapshot();
    assert!(
        snapshot.screen.text().contains("goodbye"),
        "screen:\n{}",
        snapshot.screen.text()
    );
    assert_eq!(snapshot.exit.as_ref().and_then(|exit| exit.code), Some(7));
}

#[tokio::test]
async fn input_to_a_pane_whose_program_ended_is_refused() {
    let (_session, handle) = running(shell("exit 0"), PaneSize::DEFAULT).await;
    wait_for(&handle, |snapshot| snapshot.exit.is_some()).await;

    // The pane's task has returned, so its command channel is closed.
    let error = handle
        .input(b"x".to_vec())
        .await
        .expect_err("the pane is gone");
    assert!(error.to_string().contains("no longer running"), "{error}");
}

#[tokio::test]
async fn dropping_a_pseudo_terminal_stops_its_program() {
    let (pty, mut events) =
        Pty::spawn(PaneId::FIRST, &shell("sleep 60"), PaneSize::DEFAULT).expect("the pane starts");
    drop(pty);

    // The program is killed, so its output ends and an exit is reported.
    let exit = tokio::time::timeout(TIMEOUT, async {
        loop {
            match events.recv().await {
                Some(PtyEvent::Exited(exit)) => return Some(exit),
                Some(PtyEvent::Output(_)) => {}
                None => return None,
            }
        }
    })
    .await
    .expect("the program ends");

    let exit = exit.expect("an exit is reported");
    assert!(!exit.success(), "{exit}");
}

#[tokio::test]
async fn a_noisy_program_does_not_flood_the_client_with_frames() {
    // Thousands of lines as fast as the shell can print them.
    let (_session, handle) = running(
        shell("i=0; while [ $i -lt 2000 ]; do echo \"line $i\"; i=$((i+1)); done; echo done"),
        PaneSize::new(24, 80),
    )
    .await;

    let mut watch = handle.watch();
    let mut frames = 0_u32;
    tokio::time::timeout(TIMEOUT, async {
        loop {
            if watch.borrow_and_update().screen.text().contains("done") {
                return;
            }
            if watch.changed().await.is_err() {
                return;
            }
            frames += 1;
        }
    })
    .await
    .expect("the program finishes");

    // Without coalescing this would be one frame per chunk of output; the
    // exact number depends on timing, but it must stay far below the 2000
    // lines printed.
    assert!(frames < 200, "the pane published {frames} frames");
}

#[tokio::test]
async fn a_session_of_its_own_runs_its_command_in_a_workspace_of_its_own() {
    // What a client's "new session" does: a project directory, a command,
    // and a pane to attach to.
    let directory = tempfile::tempdir().expect("a temporary directory");
    let session = Session::new(PaneSpec::shell());
    session
        .attach(PaneSize::new(24, 80), None)
        .await
        .expect("the first pane");

    let command = vec![
        "/bin/sh".to_owned(),
        "-c".to_owned(),
        "printf 'from the session\\n'; sleep 10".to_owned(),
    ];
    let pane = session
        .start_session(Some(directory.path()), &command, "claude")
        .await
        .expect("the session starts");

    let handle = session.pane(pane).expect("the new pane");
    let state = wait_for(&handle, |state| {
        state.screen.text().contains("from the session")
    })
    .await;
    assert!(state.screen.text().contains("from the session"));

    let summary = session
        .summaries()
        .into_iter()
        .find(|summary| summary.id == pane)
        .expect("the new pane is listed");
    assert_eq!(summary.title, "claude", "the pane says what it is running");

    let view = session.view();
    assert_eq!(
        view.panes.len(),
        1,
        "the new session is shown on its own, not beside the old one"
    );
    assert_eq!(view.focus, Some(pane));
    assert_eq!(view.workspaces.len(), 2, "it is a workspace of its own");
}

#[tokio::test]
async fn a_session_without_a_command_runs_the_shell() {
    let session = Session::new(PaneSpec::command([
        "/bin/sh",
        "-c",
        "printf 'shell\\n'; sleep 10",
    ]));
    session
        .attach(PaneSize::new(24, 80), None)
        .await
        .expect("the first pane");

    let pane = session
        .start_session(None, &[], "shell")
        .await
        .expect("the session starts");

    let handle = session.pane(pane).expect("the new pane");
    let state = wait_for(&handle, |state| state.screen.text().contains("shell")).await;
    assert!(state.screen.text().contains("shell"));
}

#[tokio::test]
async fn a_second_session_in_a_project_joins_the_one_already_open() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let project = directory.path().join("app");
    std::fs::create_dir(&project).expect("the project directory");
    let session = Session::new(PaneSpec::shell());

    let first = session
        .start_session(Some(&project), &[], "claude")
        .await
        .expect("the first session starts");

    // The same project by another name: a path through a symbolic link.
    // Before this, the second session made an entry called "app 2".
    let link = directory.path().join("app-link");
    std::os::unix::fs::symlink(&project, &link).expect("the link");
    let second = session
        .start_session(Some(&link), &[], "codex")
        .await
        .expect("the second session starts");

    let view = session.view();
    assert_eq!(
        view.workspaces.len(),
        1,
        "two sessions in one project are one entry in the sidebar"
    );
    let workspace = view.workspaces.first().expect("the project");
    assert_eq!(workspace.name, "app", "and it keeps the project's own name");
    assert_eq!(workspace.tabs, 2, "one tab each");
    assert_eq!(view.focus, Some(second), "the new session is the one shown");
    assert_ne!(first, second);
}

#[tokio::test]
async fn two_different_projects_sharing_a_name_stay_apart() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let one = directory.path().join("one").join("app");
    let other = directory.path().join("other").join("app");
    std::fs::create_dir_all(&one).expect("the first project");
    std::fs::create_dir_all(&other).expect("the second project");
    let session = Session::new(PaneSpec::shell());

    session
        .start_session(Some(&one), &[], "claude")
        .await
        .expect("the first session starts");
    session
        .start_session(Some(&other), &[], "claude")
        .await
        .expect("the second session starts");

    let view = session.view();
    assert_eq!(view.workspaces.len(), 2, "they are different projects");
    let names: Vec<&str> = view
        .workspaces
        .iter()
        .map(|workspace| workspace.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["app", "app 2"],
        "and the number is what tells two checkouts of one name apart"
    );
}

#[tokio::test]
async fn a_pane_reports_the_directory_it_is_working_in() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let project = directory.path().join("app");
    std::fs::create_dir(&project).expect("the project directory");
    let session = Session::new(PaneSpec::shell());

    let pane = session
        .start_session(Some(&project), &[], "claude")
        .await
        .expect("the session starts");

    // What a client needs to offer "another session in this project": the
    // workspace's name is a folder name, which is not a path.
    let summary = session
        .summaries()
        .into_iter()
        .find(|summary| summary.id == pane)
        .expect("the pane is listed");
    assert_eq!(summary.cwd.as_deref(), Some(project.as_path()));
}

#[tokio::test]
async fn history_watchers_wake_on_screen_output_without_structural_changes() {
    let session = Session::new(shell(
        "printf 'ready'; read -r answer; printf 'received %s' \"$answer\"; read -r rest",
    ));
    let mut content = session.content_changes();
    session
        .attach(PaneSize::new(24, 80), None)
        .await
        .expect("a pane");
    let handle = session.focused_pane().expect("the pane");
    wait_for_text(&handle, "ready").await;
    let generation = session.view().generation;
    content.borrow_and_update();
    handle.input(b"hello\n".to_vec()).await.expect("input");
    tokio::time::timeout(TIMEOUT, async {
        loop {
            content.changed().await.expect("content channel stays open");
            if handle.snapshot().screen.text().contains("received hello") {
                break;
            }
        }
    })
    .await
    .expect("output wakes history saving");
    assert_eq!(
        session.view().generation,
        generation,
        "screen output does not wake structural subscribers"
    );
    handle
        .input(b"\n".to_vec())
        .await
        .expect("finish the program");
    wait_for(&handle, |state| state.exit.is_some()).await;
}
