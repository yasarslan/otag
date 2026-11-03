//! The daemon's copy of how much of the agents' limits is used.
//!
//! Samples arrive from two places — the status line Claude Code pushes to
//! otag, and the rollout files Codex writes — and from two others when
//! neither says anything: the token counts in a transcript, and nothing at
//! all. They are kept here, published to the clients whenever the numbers
//! they would draw change, and written to otag's database so that a restart
//! does not lose the history a projection needs.

use std::sync::{Arc, Mutex};

use otag_agents::{AgentKind, Limits, Tokens};
use otag_core::session::Session;
use otag_features::usage::{Estimate, Report, Sample, Usage, now};

/// How much of the agents' limits is used, as the daemon knows it.
///
/// A plain mutex rather than an async one: every operation is arithmetic over
/// a few hundred samples, and holding it across an await is impossible by
/// construction.
#[derive(Debug, Default)]
pub struct Shared {
    usage: Mutex<Usage>,
}

impl Shared {
    /// An empty history.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Put back what was saved before the daemon restarted.
    pub fn load(&self, samples: Vec<Sample>) {
        self.with(|usage| usage.load(samples));
    }

    /// Keep what an agent reported about its windows.
    ///
    /// Reports nothing when the agent said nothing about either window,
    /// which is the normal case for an account that has no limits to report.
    pub fn record_limits(&self, agent: AgentKind, limits: &Limits) -> bool {
        if limits.is_empty() {
            return false;
        }
        self.with(|usage| usage.record_limits(agent, limits, now()));
        true
    }

    /// Keep what otag counted for an agent itself.
    pub fn record_tokens(&self, agent: AgentKind, tokens: Tokens) {
        self.with(|usage| {
            usage.record_estimate(
                agent,
                Estimate {
                    tokens: tokens.total(),
                    at: now(),
                },
            );
        });
    }

    /// What the widget and `otag usage` show.
    #[must_use]
    pub fn report(&self) -> Report {
        self.usage
            .lock()
            .map(|usage| usage.report(now()))
            .unwrap_or_default()
    }

    /// Every sample worth saving.
    #[must_use]
    pub fn samples(&self) -> Vec<Sample> {
        self.usage
            .lock()
            .map(|usage| usage.samples().to_vec())
            .unwrap_or_default()
    }

    /// Change the history, or do nothing if a holder of the lock panicked.
    fn with(&self, edit: impl FnOnce(&mut Usage)) {
        match self.usage.lock() {
            Ok(mut usage) => edit(&mut usage),
            // One lost sample is not worth bringing the daemon down for.
            Err(_) => tracing::debug!("the usage history is unusable"),
        }
    }
}

/// Tell the clients what the numbers are now.
///
/// Doing this on every sample is what keeps the widget honest; the session
/// drops a report that says the same as the last one, so a quiet minute costs
/// no redraws.
pub async fn publish(session: &Arc<Session>, shared: &Shared) {
    session.set_usage(shared.report()).await;
}

/// What each pane has spent, summed per agent.
///
/// This is the per-pane attribution the widget needs in order to say which
/// agent is doing the spending, and it is also the only number otag has when
/// an agent reports no percentage at all.
#[must_use]
pub fn per_agent(session: &Session) -> Vec<(AgentKind, Tokens)> {
    let mut totals: Vec<(AgentKind, Tokens)> = Vec::new();
    for (_, agent, tokens) in session.pane_usage() {
        let Some(agent) = agent else {
            continue;
        };
        match totals.iter_mut().find(|(kept, _)| *kept == agent) {
            Some((_, kept)) => {
                kept.input = kept.input.saturating_add(tokens.input);
                kept.output = kept.output.saturating_add(tokens.output);
                kept.cache_read = kept.cache_read.saturating_add(tokens.cache_read);
                kept.cache_written = kept.cache_written.saturating_add(tokens.cache_written);
                // The context is one turn's size, not a total, so the
                // largest of them is the only meaningful answer.
                kept.context = kept.context.max(tokens.context);
            }
            None => totals.push((agent, tokens)),
        }
    }
    totals
}

#[cfg(test)]
mod tests {
    use otag_agents::LimitWindow;
    use otag_features::usage::{Level, Window};

    use super::*;

    fn limits(five_hour: f32) -> Limits {
        Limits {
            five_hour: Some(LimitWindow {
                used_pct: five_hour,
                resets_at: None,
            }),
            weekly: None,
        }
    }

    #[test]
    fn what_an_agent_reports_reaches_the_report() {
        let shared = Shared::default();
        assert!(shared.record_limits(AgentKind::Claude, &limits(91.0)));

        let report = shared.report();
        assert_eq!(report.rows.len(), 1);
        assert_eq!(report.rows[0].agent, AgentKind::Claude);
        assert_eq!(report.rows[0].window, Window::FiveHour);
        assert_eq!(report.rows[0].used_pct(), 91);
        assert_eq!(report.level(), Level::Red);
    }

    #[test]
    fn an_agent_that_reports_no_limits_adds_nothing() {
        let shared = Shared::default();
        assert!(!shared.record_limits(AgentKind::Claude, &Limits::default()));
        assert!(shared.report().is_empty());
        assert!(shared.samples().is_empty());
    }

    #[test]
    fn counted_tokens_stand_in_for_an_agent_that_reports_no_percentage() {
        let shared = Shared::default();
        shared.record_tokens(
            AgentKind::Codex,
            Tokens {
                input: 100,
                output: 20,
                ..Tokens::default()
            },
        );

        let report = shared.report();
        assert!(report.rows.is_empty());
        assert_eq!(report.counted.len(), 1);
        assert_eq!(report.counted[0].agent, AgentKind::Codex);
        assert_eq!(report.counted[0].tokens, 120);
    }

    #[test]
    fn samples_put_back_after_a_restart_are_part_of_the_history() {
        let shared = Shared::default();
        shared.load(vec![Sample {
            agent: AgentKind::Claude,
            window: Window::FiveHour,
            used_pct: 40.0,
            resets_at: None,
            at: now() - 60,
        }]);

        assert_eq!(shared.samples().len(), 1);
        assert_eq!(shared.report().rows[0].used_pct(), 40);
    }

    #[tokio::test]
    async fn a_report_that_says_the_same_thing_is_not_published_twice() {
        let session = Session::new(otag_core::pane::PaneSpec::shell());
        let shared = Shared::default();
        shared.record_limits(AgentKind::Claude, &limits(10.0));

        publish(&session, &shared).await;
        let after_first = session.view().generation;
        publish(&session, &shared).await;

        assert_eq!(
            session.view().generation,
            after_first,
            "the same numbers must not make every client redraw"
        );
        assert_eq!(session.view().usage.rows.len(), 1);
    }

    #[tokio::test]
    async fn what_the_panes_spent_is_summed_per_agent() {
        let session = Session::new(otag_core::pane::PaneSpec::command(["sleep", "30"]));
        session
            .attach(otag_core::pane::PaneSize::new(24, 80), None)
            .await
            .expect("a pane");
        session
            .split(otag_core::layout::Direction::LeftRight)
            .await
            .expect("a second pane");
        let panes = session.visible_panes();

        for (pane, tokens) in panes.iter().zip([
            Tokens {
                input: 10,
                context: 500,
                ..Tokens::default()
            },
            Tokens {
                input: 5,
                context: 900,
                ..Tokens::default()
            },
        ]) {
            session.set_agent(*pane, Some(AgentKind::Claude)).await;
            session.set_tokens(*pane, tokens).await;
        }

        let totals = per_agent(&session);
        assert_eq!(totals.len(), 1);
        assert_eq!(totals[0].0, AgentKind::Claude);
        assert_eq!(totals[0].1.input, 15, "the panes add up");
        assert_eq!(
            totals[0].1.context, 900,
            "a context is one turn's size, so the largest is the answer"
        );
    }
}
