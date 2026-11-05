//! What a session is called, and what its program is told about the paper.
//!
//! Both are things a client needs and only the daemon can know: the first
//! prompt comes from the client or from the agent's own transcript, and a
//! program's environment can only be set when the program is started.

use std::sync::Arc;
use std::time::Duration;

use otag_core::pane::{PaneId, PaneSize, PaneSpec, colorfgbg};
use otag_core::session::Session;

const TIMEOUT: Duration = Duration::from_secs(10);

/// A terminal large enough for the sidebar and a pane.
const SIZE: PaneSize = PaneSize::new(24, 80);

fn shell(script: &str) -> PaneSpec {
    PaneSpec::command(["/bin/sh", "-c", script])
}

/// What one pane has on its screen.
fn pane_text(session: &Session, pane: PaneId) -> String {
    session
        .pane(pane)
        .map(|handle| handle.snapshot().screen.text())
        .unwrap_or_default()
}

/// Wait until a pane shows `needle`, or say what it showed instead.
async fn wait_for_text(session: &Arc<Session>, pane: PaneId, needle: &str) {
    let found = tokio::time::timeout(TIMEOUT, async {
        loop {
            if pane_text(session, pane).contains(needle) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;

    assert!(
        found.is_ok(),
        "the pane never showed {needle:?}; it showed:\n{}",
        pane_text(session, pane)
    );
}

/// The first pane of a fresh session.
fn only_pane(session: &Session) -> PaneId {
    let summaries = session.summaries();
    assert_eq!(summaries.len(), 1, "expected one pane");
    summaries[0].id
}

#[tokio::test]
async fn a_session_is_named_after_what_it_was_first_asked() {
    let session = Session::new(shell("sleep 10"));
    session.attach(SIZE, None).await.expect("a pane starts");
    let pane = only_pane(&session);

    assert_eq!(
        session.summaries()[0].first_prompt,
        None,
        "nothing has been asked yet"
    );

    session
        .set_first_prompt(pane, "add a test for the parser")
        .await;
    assert_eq!(
        session.summaries()[0].first_prompt.as_deref(),
        Some("add a test for the parser")
    );
}

#[tokio::test]
async fn only_the_first_prompt_becomes_the_name() {
    let session = Session::new(shell("sleep 10"));
    session.attach(SIZE, None).await.expect("a pane starts");
    let pane = only_pane(&session);

    session.set_first_prompt(pane, "the first thing").await;
    session
        .set_first_prompt(pane, "something else entirely")
        .await;

    assert_eq!(
        session.summaries()[0].first_prompt.as_deref(),
        Some("the first thing"),
        "a name that changed every turn would not be a name"
    );
}

#[tokio::test]
async fn an_empty_prompt_is_not_a_name() {
    let session = Session::new(shell("sleep 10"));
    session.attach(SIZE, None).await.expect("a pane starts");
    let pane = only_pane(&session);

    session.set_first_prompt(pane, "   \n  ").await;
    assert_eq!(
        session.summaries()[0].first_prompt,
        None,
        "whitespace is not something anybody asked"
    );

    // And a real one afterwards still lands, which is what makes the
    // transcript reader safe to call on every pass.
    session.set_first_prompt(pane, "  trim me  ").await;
    assert_eq!(
        session.summaries()[0].first_prompt.as_deref(),
        Some("trim me")
    );
}

#[tokio::test]
async fn a_pasted_essay_does_not_travel_as_a_name() {
    let session = Session::new(shell("sleep 10"));
    session.attach(SIZE, None).await.expect("a pane starts");
    let pane = only_pane(&session);

    let essay = "x".repeat(Session::FIRST_PROMPT_LIMIT * 4);
    session.set_first_prompt(pane, &essay).await;

    let kept = session.summaries()[0]
        .first_prompt
        .clone()
        .expect("a prompt");
    assert_eq!(
        kept.chars().count(),
        Session::FIRST_PROMPT_LIMIT,
        "a name is one short line, and it is clipped here rather than in \
         every client"
    );
}

#[tokio::test]
async fn a_prompt_for_a_pane_that_is_not_there_changes_nothing() {
    let session = Session::new(shell("sleep 10"));
    session.attach(SIZE, None).await.expect("a pane starts");

    // Nothing panics and nothing is invented: the daemon never crashes
    // because of one bad identifier.
    session
        .set_first_prompt(PaneId::FIRST.next().next(), "hello")
        .await;
    assert_eq!(session.summaries()[0].first_prompt, None);
}

#[tokio::test]
async fn a_pane_is_told_which_way_round_the_terminal_is() {
    // The program prints what it was given, which is the only honest way to
    // check an environment variable reached a child process.
    let session = Session::new(shell("printf 'fgbg=[%s]\\n' \"$COLORFGBG\"; sleep 10"));
    session.set_terminal_appearance(true);
    session.attach(SIZE, None).await.expect("a pane starts");
    let pane = only_pane(&session);

    wait_for_text(&session, pane, &format!("fgbg=[{}]", colorfgbg(true))).await;
}

#[tokio::test]
async fn a_light_terminal_says_so() {
    let session = Session::new(shell("printf 'fgbg=[%s]\\n' \"$COLORFGBG\"; sleep 10"));
    session.set_terminal_appearance(false);
    session.attach(SIZE, None).await.expect("a pane starts");
    let pane = only_pane(&session);

    wait_for_text(&session, pane, &format!("fgbg=[{}]", colorfgbg(false))).await;
    assert_ne!(
        colorfgbg(true),
        colorfgbg(false),
        "the two appearances have to say different things"
    );
}

#[tokio::test]
async fn a_client_that_says_nothing_leaves_the_variable_alone() {
    // Guessing would be worse than silence: a program reading the variable
    // would be told something otag made up.
    let session = Session::new(shell("printf 'fgbg=[%s]\\n' \"$COLORFGBG\"; sleep 10"));
    session.attach(SIZE, None).await.expect("a pane starts");
    let pane = only_pane(&session);

    wait_for_text(&session, pane, "fgbg=[]").await;
}

#[tokio::test]
async fn what_a_client_says_reaches_the_panes_it_starts_afterwards() {
    let session = Session::new(shell("printf 'fgbg=[%s]\\n' \"$COLORFGBG\"; sleep 10"));
    session.attach(SIZE, None).await.expect("a pane starts");
    let first = only_pane(&session);
    wait_for_text(&session, first, "fgbg=[]").await;

    // A running program's environment cannot be changed from outside it, so
    // this is about the next pane, not this one.
    session.set_terminal_appearance(true);
    session
        .split(otag_core::layout::Direction::LeftRight)
        .await
        .expect("the pane splits");

    let second = session
        .summaries()
        .into_iter()
        .map(|summary| summary.id)
        .find(|id| *id != first)
        .expect("a second pane");
    wait_for_text(&session, second, &format!("fgbg=[{}]", colorfgbg(true))).await;
    assert!(
        pane_text(&session, first).contains("fgbg=[]"),
        "the pane that was already running is left exactly as it was"
    );
}
