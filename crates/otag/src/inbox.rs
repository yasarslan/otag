//! Delivering an answer to an agent that is waiting.
//!
//! The only way into a running agent is its own screen, so an answer from
//! the inbox is typed into the pane. That is irreversible, which sets the
//! rules here:
//!
//! - nothing is typed unless the pane's screen is asking right now, judged
//!   by that agent's own adapter;
//! - the keys are the ones the prompt itself offers
//!   ([`otag_agents::approval`]);
//! - after sending, otag waits for the prompt to disappear and reports what
//!   it saw. An answer that cannot be confirmed is reported as not
//!   delivered, so the user goes and looks instead of believing it worked.

use std::sync::Arc;
use std::time::Duration;

use otag_agents::AgentKind;
use otag_agents::approval::Answer;
use otag_core::pane::PaneId;
use otag_core::session::Session;

/// How long otag waits for the prompt to go away before saying it could not
/// answer.
///
/// Long enough for an agent to redraw its screen, short enough that the user
/// is not left waiting on a dialog.
const CONFIRM_WITHIN: Duration = Duration::from_millis(1_500);

/// What happened to one answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Whether the agent took the answer: the keys were sent and the prompt
    /// is gone.
    pub delivered: bool,
    /// One line for the user saying what happened.
    pub summary: String,
}

impl Outcome {
    /// An answer that reached the agent.
    fn delivered(answer: Answer, pane: PaneId) -> Self {
        Self {
            delivered: true,
            summary: format!("{answer}ed in pane {pane}"),
        }
    }

    /// An answer that did not, and why.
    fn refused(summary: impl Into<String>) -> Self {
        Self {
            delivered: false,
            summary: summary.into(),
        }
    }
}

/// Type an answer into the pane the agent is waiting in.
///
/// `agent` is the agent otag believes is in the pane; without one there is
/// no prompt to recognise and no keys to send, so the answer is refused
/// rather than guessed at.
pub async fn deliver(
    session: &Arc<Session>,
    pane: PaneId,
    agent: Option<AgentKind>,
    answer: Answer,
) -> Outcome {
    let Some(agent) = agent else {
        return Outcome::refused(format!(
            "otag does not know which agent is in pane {pane}, so it will not type into it. \
             Answer in the pane itself."
        ));
    };
    let Some(handle) = session.pane(pane) else {
        return Outcome::refused(format!("pane {pane} is gone"));
    };

    let adapter = agent.adapter();
    if !adapter.is_asking(&handle.snapshot().screen.text()) {
        return Outcome::refused(format!(
            "{agent} in pane {pane} is not asking anything now, so nothing was typed"
        ));
    }

    if let Err(error) = session.input_to(pane, adapter.answer_keys(answer)).await {
        tracing::warn!(%error, %pane, "cannot answer an agent");
        return Outcome::refused(format!("cannot type into pane {pane}: {error}"));
    }

    if confirmed(&handle, adapter).await {
        tracing::info!(%pane, %agent, %answer, "answered an agent from the inbox");
        return Outcome::delivered(answer, pane);
    }

    tracing::info!(%pane, %agent, %answer, "an answer was sent but the prompt is still up");
    Outcome::refused(format!(
        "{agent} in pane {pane} is still asking. Look at the pane: the answer may need \
         a different key."
    ))
}

/// Wait for the agent's prompt to go away, and say whether it did.
async fn confirmed(
    handle: &otag_core::session::PaneHandle,
    adapter: &'static dyn otag_agents::AgentAdapter,
) -> bool {
    let mut screens = handle.watch();
    let deadline = tokio::time::Instant::now() + CONFIRM_WITHIN;

    loop {
        if !adapter.is_asking(&handle.snapshot().screen.text()) {
            return true;
        }
        // The pane publishes its screen at most every frame interval, so
        // waiting for the next one costs nothing and wakes as soon as the
        // agent has redrawn.
        match tokio::time::timeout_at(deadline, screens.changed()).await {
            Ok(Ok(())) => {}
            // The pane's task is gone: whatever was asking is not asking
            // any more.
            Ok(Err(_)) => return true,
            Err(_) => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use otag_core::pane::{PaneSize, PaneSpec};

    use super::*;

    /// A session with one pane running a program that prints `screen` and
    /// then waits, so that the pane's screen says what the test needs.
    async fn with_screen(screen: &str) -> (Arc<Session>, PaneId) {
        let session = Session::new(PaneSpec::command([
            "sh",
            "-c",
            &format!("printf '%s' '{screen}'; sleep 30"),
        ]));
        session
            .attach(PaneSize::new(24, 80), None)
            .await
            .expect("a pane");
        let pane = *session.visible_panes().first().expect("one pane");

        // The program has to have printed before the screen says anything.
        for _ in 0..50 {
            let handle = session.pane(pane).expect("the pane");
            if handle.snapshot().screen.text().contains(screen) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        (session, pane)
    }

    #[tokio::test]
    async fn an_answer_for_a_pane_without_a_known_agent_is_refused() {
        let (session, pane) = with_screen("Do you want to proceed?").await;

        let outcome = deliver(&session, pane, None, Answer::Allow).await;
        assert!(!outcome.delivered);
        assert!(
            outcome.summary.contains("does not know which agent"),
            "{}",
            outcome.summary
        );
    }

    #[tokio::test]
    async fn an_answer_for_a_pane_that_is_gone_is_refused() {
        let session = Session::new(PaneSpec::command(["sleep", "30"]));
        session
            .attach(PaneSize::new(24, 80), None)
            .await
            .expect("a pane");

        let outcome = deliver(
            &session,
            PaneId::from_number(999),
            Some(AgentKind::Claude),
            Answer::Deny,
        )
        .await;
        assert!(!outcome.delivered);
        assert!(outcome.summary.contains("gone"), "{}", outcome.summary);
    }

    #[tokio::test]
    async fn nothing_is_typed_into_an_agent_that_is_not_asking() {
        // The pane shows an agent at work, not a prompt.
        let (session, pane) = with_screen("esc to interrupt").await;

        let outcome = deliver(&session, pane, Some(AgentKind::Claude), Answer::Allow).await;
        assert!(!outcome.delivered);
        assert!(
            outcome.summary.contains("not asking anything"),
            "{}",
            outcome.summary
        );
    }

    #[tokio::test]
    async fn an_answer_that_leaves_the_prompt_up_is_not_reported_as_delivered() {
        // `sleep` ignores what is typed, so the question stays on screen —
        // which is exactly the case where otag must not claim success.
        let (session, pane) = with_screen("Do you want to proceed?").await;

        let outcome = deliver(&session, pane, Some(AgentKind::Claude), Answer::Allow).await;
        assert!(!outcome.delivered);
        assert!(
            outcome.summary.contains("still asking"),
            "{}",
            outcome.summary
        );
    }

    #[tokio::test]
    async fn an_answer_is_delivered_once_the_prompt_is_gone() {
        // A program that asks, and clears the question as soon as anything
        // is typed, which is what an agent does.
        let session = Session::new(PaneSpec::command([
            "sh",
            "-c",
            "printf 'Do you want to proceed?'; read -r answer; printf '\\033[2J\\033[Hdone'; sleep 30",
        ]));
        session
            .attach(PaneSize::new(24, 80), None)
            .await
            .expect("a pane");
        let pane = *session.visible_panes().first().expect("one pane");
        for _ in 0..50 {
            let handle = session.pane(pane).expect("the pane");
            if handle.snapshot().screen.text().contains("proceed?") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let outcome = deliver(&session, pane, Some(AgentKind::Claude), Answer::Allow).await;
        assert!(outcome.delivered, "{}", outcome.summary);
        assert!(outcome.summary.contains("allow"), "{}", outcome.summary);
    }
}
