//! Panes running real programs on real pseudo-terminals.

use std::time::Duration;

use otag_core::pane::{PaneId, PaneSize, PaneSpec};
use otag_core::pty::{Pty, PtyEvent};
use otag_core::session::{PaneHandle, PaneView, Session};

/// How long a test waits for a child process to do its work. Generous: a
/// loaded machine must not make the suite flaky.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Wait until the pane's screen satisfies `ready`, then return its view.
async fn wait_for(handle: &PaneHandle, ready: impl Fn(&PaneView) -> bool) -> PaneView {
    let mut watch = handle.watch();
    let view = tokio::time::timeout(TIMEOUT, async {
        loop {
            {
                let view = watch.borrow_and_update();
                if ready(&view) {
                    return PaneView::clone(&view);
                }
            }
            if watch.changed().await.is_err() {
                // The pane is gone; report its last known state.
                return PaneView::clone(&handle.view());
            }
        }
    })
    .await;

    match view {
        Ok(view) => view,
        Err(_) => {
            let view = handle.view();
            panic!(
                "the pane did not reach the expected state within {:?}.\nscreen:\n{}",
                TIMEOUT,
                view.screen.text()
            )
        }
    }
}

/// Wait until the pane's screen contains `needle`.
async fn wait_for_text(handle: &PaneHandle, needle: &str) -> PaneView {
    wait_for(handle, |view| view.screen.text().contains(needle)).await
}

fn shell(script: &str) -> PaneSpec {
    PaneSpec::command(["/bin/sh", "-c", script])
}

#[tokio::test]
async fn a_pane_shows_what_its_program_prints() {
    let session = Session::new();
    let handle = session
        .attach_pane(
            &shell("printf 'hello from a pane\\n'"),
            PaneSize::new(10, 40),
        )
        .expect("the pane starts");

    let view = wait_for_text(&handle, "hello from a pane").await;
    assert_eq!(view.screen.row_text(0), "hello from a pane");
    assert_eq!(view.screen.size(), PaneSize::new(10, 40));
}

#[tokio::test]
async fn a_pane_reports_how_its_program_ended() {
    let session = Session::new();
    let handle = session
        .attach_pane(&shell("exit 3"), PaneSize::DEFAULT)
        .expect("the pane starts");

    let view = wait_for(&handle, |view| view.exit.is_some()).await;
    let exit = view.exit.expect("the program ended");
    assert_eq!(exit.code, Some(3));
    assert_eq!(exit.signal, None);
    assert!(!exit.success());
    assert!(handle.is_finished());
}

#[tokio::test]
async fn a_pane_passes_input_to_its_program() {
    let session = Session::new();
    let handle = session
        .attach_pane(
            &shell("read line; printf 'got %s\\n' \"$line\""),
            PaneSize::new(10, 40),
        )
        .expect("the pane starts");

    handle
        .input(b"typed\r".to_vec())
        .await
        .expect("the pane accepts input");

    wait_for_text(&handle, "got typed").await;
}

#[tokio::test]
async fn a_pane_tells_its_program_about_a_resize() {
    let session = Session::new();
    // Keep reporting the size the program is told it has.
    let handle = session
        .attach_pane(
            &shell("while :; do stty size; sleep 1; done"),
            PaneSize::new(24, 80),
        )
        .expect("the pane starts");

    wait_for_text(&handle, "24 80").await;

    handle
        .resize(PaneSize::new(30, 100))
        .await
        .expect("the pane resizes");

    let view = wait_for_text(&handle, "30 100").await;
    assert_eq!(view.screen.size(), PaneSize::new(30, 100));
}

#[tokio::test]
async fn a_pane_answers_the_queries_a_program_makes() {
    let session = Session::new();
    // Move the cursor, then ask where it is. No real terminal is involved,
    // so an answer can only come from otag. The pseudo-terminal echoes the
    // input it receives, which is how the answer becomes visible.
    let handle = session
        .attach_pane(
            &shell("printf '\\033[3;5H\\033[6n'; sleep 10"),
            PaneSize::new(12, 40),
        )
        .expect("the pane starts");

    // The reply reports the one-based position the cursor was moved to.
    let view = wait_for_text(&handle, "[3;5R").await;
    assert!(
        view.screen.text().contains("[3;5R"),
        "the pane did not answer the cursor query.\nscreen:\n{}",
        view.screen.text()
    );
}

#[tokio::test]
async fn a_pane_tells_its_program_which_terminal_it_is_in() {
    let session = Session::new();
    let handle = session
        .attach_pane(
            &shell(
                "printf 'term=%s color=%s pane=%s\\n' \"$TERM\" \"$COLORTERM\" \"$OTAG_PANE_ID\"",
            ),
            PaneSize::new(10, 60),
        )
        .expect("the pane starts");

    let view = wait_for_text(&handle, "term=").await;
    assert_eq!(
        view.screen.row_text(0),
        "term=xterm-256color color=truecolor pane=1"
    );
}

#[tokio::test]
async fn a_pane_starts_in_the_directory_it_was_given() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    // macOS hands out symlinked temporary paths, so compare what the shell
    // itself resolves rather than the path as written.
    let expected = directory
        .path()
        .canonicalize()
        .expect("the directory resolves");

    let session = Session::new();
    let handle = session
        .attach_pane(
            &PaneSpec::command(["/bin/sh", "-c", "pwd -P"]).with_cwd(directory.path()),
            PaneSize::new(10, 200),
        )
        .expect("the pane starts");

    let view = wait_for_text(&handle, "/").await;
    assert_eq!(view.screen.row_text(0), expected.display().to_string());
}

#[tokio::test]
async fn a_pane_in_a_missing_directory_fails_with_a_clear_message() {
    let session = Session::new();
    let error = session
        .attach_pane(
            &PaneSpec::shell().with_cwd("/no/such/directory/anywhere"),
            PaneSize::DEFAULT,
        )
        .expect_err("the pane cannot start");

    let message = error.to_string();
    assert!(message.contains("/no/such/directory/anywhere"), "{message}");
    assert!(message.contains("not a directory"), "{message}");
    assert!(session.is_empty());
}

#[tokio::test]
async fn a_program_that_cannot_be_started_is_reported() {
    let session = Session::new();
    let error = session
        .attach_pane(
            &PaneSpec::command(["/nonexistent/program"]),
            PaneSize::DEFAULT,
        )
        .expect_err("the pane cannot start");

    assert!(
        error.to_string().contains("/nonexistent/program"),
        "{error}"
    );
}

#[tokio::test]
async fn attaching_twice_reaches_the_same_pane() {
    let session = Session::new();
    let first = session
        .attach_pane(&shell("sleep 10"), PaneSize::DEFAULT)
        .expect("the pane starts");
    let second = session
        .attach_pane(&shell("sleep 10"), PaneSize::DEFAULT)
        .expect("the pane is found");

    assert_eq!(first.id(), second.id());
    assert_eq!(session.summaries().len(), 1);
}

#[tokio::test]
async fn attaching_after_a_program_ended_gives_a_fresh_pane() {
    let session = Session::new();
    let finished = session
        .attach_pane(&shell("exit 0"), PaneSize::DEFAULT)
        .expect("the pane starts");
    wait_for(&finished, |view| view.exit.is_some()).await;

    let fresh = session
        .attach_pane(&shell("sleep 10"), PaneSize::DEFAULT)
        .expect("a new pane starts");

    assert_ne!(fresh.id(), finished.id());
    assert!(!fresh.is_finished());
    let summaries = session.summaries();
    assert_eq!(summaries.len(), 1, "the finished pane is not kept");
    assert_eq!(summaries[0].id, fresh.id());
}

#[tokio::test]
async fn a_pane_the_session_does_not_have_is_not_found() {
    let session = Session::new();
    assert!(session.pane(PaneId::FIRST).is_none());
    assert!(session.is_empty());

    let handle = session
        .attach_pane(&shell("sleep 10"), PaneSize::DEFAULT)
        .expect("the pane starts");
    assert_eq!(
        session.pane(handle.id()).map(|found| found.id()),
        Some(handle.id())
    );
    assert!(session.pane(handle.id().next()).is_none());
}

#[tokio::test]
async fn input_to_a_pane_whose_program_ended_is_refused() {
    let session = Session::new();
    let handle = session
        .attach_pane(&shell("exit 0"), PaneSize::DEFAULT)
        .expect("the pane starts");
    wait_for(&handle, |view| view.exit.is_some()).await;

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
    let session = Session::new();
    // Thousands of lines as fast as the shell can print them.
    let handle = session
        .attach_pane(
            &shell("i=0; while [ $i -lt 2000 ]; do echo \"line $i\"; i=$((i+1)); done; echo done"),
            PaneSize::new(24, 80),
        )
        .expect("the pane starts");

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
