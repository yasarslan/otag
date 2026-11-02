//! Panes running real programs on real pseudo-terminals.

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
