//! Snapshotting a session and putting it back.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use otag_core::layout::Direction;
use otag_core::pane::{PaneId, PaneSize, PaneSpec};
use otag_core::proto::SessionView;
use otag_core::session::Session;
use otag_core::snapshot::{PaneSnapshot, Snapshot};

const TIMEOUT: Duration = Duration::from_secs(10);

/// A terminal large enough for a sidebar and a couple of panes.
const SIZE: PaneSize = PaneSize::new(24, 80);

fn shell(script: &str) -> PaneSpec {
    PaneSpec::command(["/bin/sh", "-c", script])
}

/// Wait until every pane of the session shows something, so that a snapshot
/// has lines to keep.
async fn wait_for_text(session: &Arc<Session>, needle: &str) {
    let found = tokio::time::timeout(TIMEOUT, async {
        let mut changes = session.changes();
        loop {
            let view = session.view();
            let showing = view
                .panes
                .iter()
                .filter_map(|pane| pane.frame.as_ref())
                .filter(|frame| {
                    frame
                        .rows
                        .iter()
                        .flat_map(|row| row.runs.iter())
                        .any(|run| run.text.contains(needle))
                })
                .count();
            if showing == view.panes.len() && !view.panes.is_empty() {
                return;
            }
            // A pane drawing does not bump the session's generation, so the
            // wait is a poll rather than a wake-up.
            tokio::select! {
                _ = changes.changed() => {}
                () = tokio::time::sleep(Duration::from_millis(20)) => {}
            }
        }
    })
    .await;

    assert!(
        found.is_ok(),
        "the panes never showed {needle:?}: {:#?}",
        session.view()
    );
}

/// What a client would see, without the parts that cannot match across two
/// daemons: the generation, and the screens themselves.
fn shape(
    view: &SessionView,
) -> (
    PaneSize,
    bool,
    Vec<String>,
    usize,
    usize,
    Vec<PaneId>,
    Option<PaneId>,
) {
    (
        view.size,
        view.sidebar.is_empty(),
        view.workspaces
            .iter()
            .map(|workspace| workspace.name.clone())
            .collect(),
        view.active_workspace,
        view.active_tab,
        view.panes.iter().map(|pane| pane.id).collect(),
        view.focus,
    )
}

/// Everything on one pane of a session.
fn pane_text(session: &Session, pane: PaneId) -> String {
    session
        .pane(pane)
        .map(|handle| handle.snapshot().screen.text())
        .unwrap_or_default()
}

/// Take a snapshot of a session, with the lines its panes have shown.
async fn snapshot_of(session: &Arc<Session>) -> (Snapshot, HashMap<PaneId, Vec<String>>) {
    let snapshot = session.snapshot();
    let history = session.histories(500).await.into_iter().collect();
    (snapshot, history)
}

/// Restore a snapshot into a new session running `script`.
async fn restored(
    script: &str,
    snapshot: &Snapshot,
    history: &HashMap<PaneId, Vec<String>>,
) -> Arc<Session> {
    let session = Session::new(shell(script));
    let lines = |pane: PaneId| history.get(&pane).cloned().unwrap_or_default();
    session
        .restore(snapshot, &lines)
        .await
        .expect("the session is restored");
    session
}

#[tokio::test]
async fn a_session_comes_back_with_its_layout_and_what_its_panes_showed() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let before = Session::new(shell("printf 'written before the restart\\n'; sleep 10"));
    before
        .attach(SIZE, Some(directory.path()))
        .await
        .expect("a pane starts");
    before
        .split(Direction::LeftRight)
        .await
        .expect("the pane splits");
    before.new_tab().await.expect("a tab is added");
    before.select_tab(0).await;
    wait_for_text(&before, "written before the restart").await;

    let (snapshot, history) = snapshot_of(&before).await;
    assert_eq!(snapshot.panes.len(), 3, "two panes and the new tab's one");

    // A different daemon, so the panes cannot be the same processes.
    let after = restored("sleep 10", &snapshot, &history).await;

    assert_eq!(
        shape(&after.view()),
        shape(&before.view()),
        "the restored session is laid out like the one that was saved"
    );
    assert_eq!(after.view().tabs.len(), 2, "both tabs came back");

    for pane in after.view().panes.iter().map(|pane| pane.id) {
        let text = pane_text(&after, pane);
        assert!(
            text.contains("written before the restart"),
            "pane {pane} lost what it showed:\n{text}"
        );
        assert!(
            text.contains("restored"),
            "pane {pane} has no divider:\n{text}"
        );
    }
}

#[tokio::test]
async fn a_restored_pane_starts_in_the_directory_it_was_running_in() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let expected = directory
        .path()
        .canonicalize()
        .expect("the directory resolves");

    // Wide enough that the path is one line of the pane rather than a line
    // broken across rows.
    let before = Session::new(shell("sleep 10"));
    before
        .attach(PaneSize::new(10, 400), Some(directory.path()))
        .await
        .expect("a pane starts");
    let (snapshot, history) = snapshot_of(&before).await;

    let after = restored("pwd -P; sleep 10", &snapshot, &history).await;
    let pane = after.view().panes[0].id;

    let found = tokio::time::timeout(TIMEOUT, async {
        loop {
            if pane_text(&after, pane).contains(&expected.display().to_string()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        found.is_ok(),
        "the pane did not start in {}:\n{}",
        expected.display(),
        pane_text(&after, pane)
    );
}

#[tokio::test]
async fn a_pane_that_cannot_be_started_again_is_left_out_and_the_rest_restored() {
    let before = Session::new(shell("sleep 10"));
    before.attach(SIZE, None).await.expect("a pane starts");
    before
        .split(Direction::LeftRight)
        .await
        .expect("the pane splits");

    let (mut snapshot, history) = snapshot_of(&before).await;
    let survivor = snapshot.panes[0].id;
    // The second pane now claims to have been running something that is not
    // there any more, which is what an uninstalled agent looks like.
    snapshot.panes[1].command = vec!["/nonexistent/program".to_owned()];

    let after = restored("sleep 10", &snapshot, &history).await;
    let view = after.view();
    assert_eq!(view.panes.len(), 1, "{view:#?}");
    assert_eq!(view.panes[0].id, survivor);
    assert_eq!(
        view.panes[0].area, view.content,
        "the pane that is left fills the tab"
    );
    assert_eq!(view.focus, Some(survivor));
}

#[tokio::test]
async fn a_tab_whose_panes_are_all_gone_is_not_restored() {
    let before = Session::new(shell("sleep 10"));
    before.attach(SIZE, None).await.expect("a pane starts");
    before.new_tab().await.expect("a tab is added");

    let (mut snapshot, history) = snapshot_of(&before).await;
    for pane in &mut snapshot.panes {
        if pane.id != PaneId::FIRST {
            pane.command = vec!["/nonexistent/program".to_owned()];
        }
    }

    let after = restored("sleep 10", &snapshot, &history).await;
    let view = after.view();
    assert_eq!(view.tabs.len(), 1, "{view:#?}");
    assert_eq!(view.panes.len(), 1);
    assert_eq!(view.panes[0].id, PaneId::FIRST);
}

#[tokio::test]
async fn a_session_with_nothing_left_to_restore_starts_fresh() {
    let session = Session::new(shell("sleep 10"));
    let snapshot = Snapshot {
        panes: vec![PaneSnapshot {
            id: PaneId::FIRST,
            title: "shell".to_owned(),
            cwd: None,
            command: vec!["/nonexistent/program".to_owned()],
        }],
        ..Snapshot::default()
    };

    session
        .restore(&snapshot, &|_| Vec::new())
        .await
        .expect("restoring an unusable snapshot is not an error");
    assert!(session.is_empty());

    // Attaching then starts a session of its own.
    session.attach(SIZE, None).await.expect("a pane starts");
    assert_eq!(session.view().panes.len(), 1);
}

#[tokio::test]
async fn restoring_does_nothing_to_a_session_that_is_already_running() {
    let before = Session::new(shell("sleep 10"));
    before.attach(SIZE, None).await.expect("a pane starts");
    before
        .split(Direction::LeftRight)
        .await
        .expect("the pane splits");
    let (snapshot, history) = snapshot_of(&before).await;

    let after = Session::new(shell("sleep 10"));
    after.attach(SIZE, None).await.expect("a pane starts");
    let before_restore = shape(&after.view());

    after
        .restore(&snapshot, &|pane| {
            history.get(&pane).cloned().unwrap_or_default()
        })
        .await
        .expect("restoring is refused quietly");
    assert_eq!(
        shape(&after.view()),
        before_restore,
        "a running session is left alone"
    );
}

#[tokio::test]
async fn a_snapshot_from_a_newer_otag_is_not_restored() {
    let before = Session::new(shell("sleep 10"));
    before.attach(SIZE, None).await.expect("a pane starts");
    let (snapshot, _) = snapshot_of(&before).await;

    let newer = Snapshot {
        version: otag_core::snapshot::VERSION + 1,
        ..snapshot
    };
    let after = Session::new(shell("sleep 10"));
    after
        .restore(&newer, &|_| Vec::new())
        .await
        .expect("a snapshot that cannot be read is not an error");
    assert!(after.is_empty());
}

#[tokio::test]
async fn identifiers_are_not_reused_after_a_restore() {
    let before = Session::new(shell("sleep 10"));
    before.attach(SIZE, None).await.expect("a pane starts");
    before
        .split(Direction::LeftRight)
        .await
        .expect("the pane splits");
    let (snapshot, history) = snapshot_of(&before).await;

    let after = restored("sleep 10", &snapshot, &history).await;
    let highest = after
        .view()
        .panes
        .iter()
        .map(|pane| pane.id.get())
        .max()
        .expect("the panes came back");

    after
        .split(Direction::TopBottom)
        .await
        .expect("a pane is added");
    let added = after.view().focus.expect("the new pane is focused");
    assert!(
        added.get() > highest,
        "pane {added} reuses an identifier a restored pane already has"
    );
}

#[tokio::test]
async fn what_a_pane_showed_is_kept_as_text_and_nothing_else() {
    // Lines come back from a file, so anything in them that could move the
    // cursor or change a colour must be treated as the text it looks like.
    let session = Session::new(shell("sleep 10"));
    let snapshot = Snapshot {
        workspaces: vec![otag_core::snapshot::WorkspaceSnapshot {
            id: otag_core::workspace::WorkspaceId::FIRST,
            name: "app".to_owned(),
            cwd: None,
            active_tab: 0,
            tabs: vec![otag_core::snapshot::TabSnapshot {
                id: otag_core::workspace::TabId::FIRST,
                title: "shell".to_owned(),
                focus: PaneId::FIRST,
                layout: otag_core::layout::Layout::new(PaneId::FIRST),
            }],
        }],
        panes: vec![PaneSnapshot {
            id: PaneId::FIRST,
            title: "shell".to_owned(),
            cwd: None,
            command: Vec::new(),
        }],
        size: SIZE,
        ..Snapshot::default()
    };

    let mischief = vec!["\u{1b}[2Jcleared the screen".to_owned()];
    session
        .restore(&snapshot, &|_| mischief.clone())
        .await
        .expect("the session is restored");

    let text = pane_text(&session, PaneId::FIRST);
    assert!(text.contains("cleared the screen"), "{text}");
    assert!(
        text.contains("restored"),
        "the divider was wiped by the restored text:\n{text}"
    );
}
