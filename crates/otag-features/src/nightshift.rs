//! Working through a queue of prompts without burning the limits.
//!
//! The night shift exists because of one shape of problem: there is more to
//! do than one five-hour window holds, and a person who wants it done
//! overnight should not have to wake up to send the next prompt.
//!
//! The rules are deliberately cautious. otag sends a prompt only when the
//! agent is idle and there is room in the window; at the threshold it stops
//! sending and waits for the window to reset; and it never sends two prompts
//! for one answer. Everything it does is written down, so the morning report
//! is a record rather than a reconstruction.
//!
//! This module is the reasoning only: what to do next, given what the limits
//! say and what the agent is doing. Sending anything is the daemon's job.

use std::time::{SystemTime, UNIX_EPOCH};

use otag_agents::{AgentKind, PaneStatus};

use crate::usage::Report;

/// The share of a window at which otag stops sending prompts.
///
/// Ninety per cent leaves room for the answer otag has already asked for,
/// which is the part a lower threshold gets wrong.
pub const DEFAULT_PAUSE_AT: u16 = 90;

/// How long an agent must have been idle before otag sends it the next
/// prompt.
///
/// An agent that has just answered is often about to print more. Waiting a
/// few seconds costs nothing overnight and keeps otag from typing into the
/// middle of something.
pub const SETTLE_SECONDS: i64 = 5;

/// What the user asked the night shift to do for one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Percentage of a window at which to stop sending.
    pub pause_at: u16,
    /// What to send when a window has reset and there is nothing queued.
    ///
    /// Empty means: send nothing, just carry on with the queue.
    pub resume_prompt: String,
    /// Prompts to work through, in order.
    pub queue: Vec<String>,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            pause_at: DEFAULT_PAUSE_AT,
            resume_prompt: String::new(),
            queue: Vec::new(),
        }
    }
}

impl Plan {
    /// Whether this plan has anything left to do.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty() && self.resume_prompt.is_empty()
    }
}

/// What otag has done for one session so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    /// How many of the queued prompts have been sent.
    pub sent: usize,
    /// When the last prompt was sent, in seconds since the epoch.
    pub last_sent_at: Option<i64>,
    /// Whether otag is waiting for a window to reset.
    pub paused: bool,
    /// What otag did, newest last, for the morning report.
    pub log: Vec<Entry>,
}

/// One thing the night shift did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// When, in seconds since the epoch.
    pub at: i64,
    /// What happened, in one line.
    pub what: String,
}

/// What the night shift should do for a session right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Nothing, for now.
    Wait,
    /// Stop sending: the window is nearly gone. Carries the percentage.
    Pause(u16),
    /// The window has reset; sending may start again.
    Resume,
    /// Send this prompt to the agent.
    Send(String),
    /// Everything the plan asked for is done.
    Finished,
}

/// What the session looks like to the night shift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Watched {
    /// The agent in the pane, which decides whose limits matter.
    pub agent: AgentKind,
    /// What that agent is doing.
    pub status: PaneStatus,
    /// Now, in seconds since the epoch.
    pub now: i64,
}

/// Decide what to do for one session.
///
/// The order of the checks is the policy: a question from the agent always
/// wins (otag must not type past it), then the limits, then whether the
/// agent is free to be given something.
#[must_use]
pub fn decide(plan: &Plan, progress: &Progress, usage: &Report, watched: Watched) -> Decision {
    // An agent waiting for permission, or one that has stopped on an error,
    // is not a queue otag may push. The user has to look at it.
    if matches!(
        watched.status,
        PaneStatus::NeedsApproval | PaneStatus::Error | PaneStatus::Looping
    ) {
        return Decision::Wait;
    }

    let used = worst_used(usage, watched.agent);
    if used >= plan.pause_at {
        return if progress.paused {
            Decision::Wait
        } else {
            Decision::Pause(used)
        };
    }
    if progress.paused {
        // There is room again, which means the window reset.
        return Decision::Resume;
    }

    // Only an idle agent is given something: one that is working already has
    // a prompt of otag's to answer.
    if watched.status != PaneStatus::Idle {
        return Decision::Wait;
    }
    if progress
        .last_sent_at
        .is_some_and(|at| watched.now.saturating_sub(at) < SETTLE_SECONDS)
    {
        return Decision::Wait;
    }

    match plan.queue.get(progress.sent) {
        Some(prompt) => Decision::Send(prompt.clone()),
        None if progress.sent == 0 && !plan.resume_prompt.is_empty() => {
            // A plan with no queue is a standing order: send the resume
            // prompt once the window allows it.
            Decision::Send(plan.resume_prompt.clone())
        }
        None => Decision::Finished,
    }
}

/// How much of the worst of an agent's windows is used, as a percentage.
///
/// Nothing reported is nothing to go on, and a night shift that refuses to
/// run until an agent reports its limits would never run for an agent that
/// does not report them at all. So an agent with no numbers is treated as
/// having room, and the threshold only ever stops a session otag can
/// actually measure.
#[must_use]
pub fn worst_used(usage: &Report, agent: AgentKind) -> u16 {
    usage
        .rows
        .iter()
        .filter(|row| row.agent == agent)
        .map(crate::usage::Row::used_pct)
        .max()
        .unwrap_or(0)
}

/// Now, in seconds since the epoch.
#[must_use]
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

impl Progress {
    /// Write down something the night shift did.
    pub fn record(&mut self, at: i64, what: impl Into<String>) {
        /// How many entries one session keeps. A night is not a month.
        const KEPT: usize = 200;

        self.log.push(Entry {
            at,
            what: what.into(),
        });
        if self.log.len() > KEPT {
            let extra = self.log.len() - KEPT;
            self.log.drain(0..extra);
        }
    }

    /// Whether every prompt of `plan` has been sent.
    #[must_use]
    pub fn is_done(&self, plan: &Plan) -> bool {
        if plan.queue.is_empty() {
            return self.sent > 0 || plan.resume_prompt.is_empty();
        }
        self.sent >= plan.queue.len()
    }
}

#[cfg(test)]
mod tests {
    use crate::usage::{Level, Row, Window};

    use super::*;

    fn usage(agent: AgentKind, used_pct: u16) -> Report {
        Report {
            rows: vec![Row {
                agent,
                window: Window::FiveHour,
                used_tenths: used_pct.saturating_mul(10),
                resets_in: Some(3_600),
                full_in: None,
                level: Level::Calm,
                at: 0,
            }],
            counted: Vec::new(),
        }
    }

    fn watched(status: PaneStatus) -> Watched {
        Watched {
            agent: AgentKind::Claude,
            status,
            now: 1_000,
        }
    }

    fn plan(queue: &[&str]) -> Plan {
        Plan {
            pause_at: DEFAULT_PAUSE_AT,
            resume_prompt: String::new(),
            queue: queue.iter().map(|prompt| (*prompt).to_owned()).collect(),
        }
    }

    #[test]
    fn an_idle_agent_with_room_is_given_the_next_prompt() {
        let plan = plan(&["first", "second"]);
        let progress = Progress::default();

        assert_eq!(
            decide(
                &plan,
                &progress,
                &usage(AgentKind::Claude, 10),
                watched(PaneStatus::Idle)
            ),
            Decision::Send("first".to_owned())
        );
    }

    #[test]
    fn the_queue_moves_on_only_as_far_as_it_has_been_sent() {
        let plan = plan(&["first", "second"]);
        let progress = Progress {
            sent: 1,
            ..Progress::default()
        };

        assert_eq!(
            decide(
                &plan,
                &progress,
                &usage(AgentKind::Claude, 10),
                watched(PaneStatus::Idle)
            ),
            Decision::Send("second".to_owned())
        );
    }

    #[test]
    fn a_working_agent_is_left_alone() {
        let plan = plan(&["first"]);
        let progress = Progress::default();

        assert_eq!(
            decide(
                &plan,
                &progress,
                &usage(AgentKind::Claude, 10),
                watched(PaneStatus::Working)
            ),
            Decision::Wait
        );
    }

    #[test]
    fn an_agent_asking_for_permission_is_never_typed_past() {
        // Even with room and a queue: the user has to answer it.
        let plan = plan(&["first"]);
        let progress = Progress::default();

        for status in [
            PaneStatus::NeedsApproval,
            PaneStatus::Error,
            PaneStatus::Looping,
        ] {
            assert_eq!(
                decide(
                    &plan,
                    &progress,
                    &usage(AgentKind::Claude, 1),
                    watched(status)
                ),
                Decision::Wait,
                "{status} must stop the night shift"
            );
        }
    }

    #[test]
    fn the_threshold_pauses_once_and_then_waits() {
        let plan = plan(&["first"]);
        let progress = Progress::default();
        let usage = usage(AgentKind::Claude, 91);

        assert_eq!(
            decide(&plan, &progress, &usage, watched(PaneStatus::Idle)),
            Decision::Pause(91)
        );

        let paused = Progress {
            paused: true,
            ..Progress::default()
        };
        assert_eq!(
            decide(&plan, &paused, &usage, watched(PaneStatus::Idle)),
            Decision::Wait,
            "a session that is already paused is not paused again"
        );
    }

    #[test]
    fn a_window_that_has_reset_resumes() {
        let plan = plan(&["first"]);
        let paused = Progress {
            paused: true,
            ..Progress::default()
        };

        assert_eq!(
            decide(
                &plan,
                &paused,
                &usage(AgentKind::Claude, 4),
                watched(PaneStatus::Idle)
            ),
            Decision::Resume
        );
    }

    #[test]
    fn another_agents_limits_do_not_pause_this_one() {
        let plan = plan(&["first"]);
        let progress = Progress::default();

        assert_eq!(
            decide(
                &plan,
                &progress,
                &usage(AgentKind::Codex, 99),
                watched(PaneStatus::Idle)
            ),
            Decision::Send("first".to_owned()),
            "the Codex window says nothing about Claude's"
        );
    }

    #[test]
    fn a_prompt_just_sent_is_given_time_to_land() {
        let plan = plan(&["first", "second"]);
        let progress = Progress {
            sent: 1,
            last_sent_at: Some(999),
            ..Progress::default()
        };

        assert_eq!(
            decide(
                &plan,
                &progress,
                &usage(AgentKind::Claude, 10),
                watched(PaneStatus::Idle)
            ),
            Decision::Wait,
            "an agent that has just been given something is not given more"
        );
    }

    #[test]
    fn an_empty_queue_with_a_resume_prompt_sends_it_once() {
        let plan = Plan {
            pause_at: DEFAULT_PAUSE_AT,
            resume_prompt: "carry on where you left off".to_owned(),
            queue: Vec::new(),
        };

        assert_eq!(
            decide(
                &plan,
                &Progress::default(),
                &usage(AgentKind::Claude, 10),
                watched(PaneStatus::Idle)
            ),
            Decision::Send("carry on where you left off".to_owned())
        );

        let sent = Progress {
            sent: 1,
            ..Progress::default()
        };
        assert_eq!(
            decide(
                &plan,
                &sent,
                &usage(AgentKind::Claude, 10),
                watched(PaneStatus::Idle)
            ),
            Decision::Finished
        );
    }

    #[test]
    fn a_queue_that_is_done_says_so() {
        let plan = plan(&["only"]);
        let progress = Progress {
            sent: 1,
            ..Progress::default()
        };

        assert_eq!(
            decide(
                &plan,
                &progress,
                &usage(AgentKind::Claude, 10),
                watched(PaneStatus::Idle)
            ),
            Decision::Finished
        );
        assert!(progress.is_done(&plan));
    }

    #[test]
    fn the_log_keeps_the_newest_entries_and_not_the_whole_night() {
        let mut progress = Progress::default();
        for index in 0..300 {
            progress.record(index, format!("entry {index}"));
        }

        assert_eq!(progress.log.len(), 200);
        assert_eq!(progress.log.last().map(|entry| entry.at), Some(299));
        assert_eq!(progress.log.first().map(|entry| entry.at), Some(100));
    }

    #[test]
    fn an_agent_that_reports_no_limits_is_treated_as_having_room() {
        // Otherwise a night shift would never run for an agent that reports
        // nothing, which is worse than running it and watching the count.
        let plan = plan(&["first"]);
        assert_eq!(worst_used(&Report::default(), AgentKind::Claude), 0);
        assert_eq!(
            decide(
                &plan,
                &Progress::default(),
                &Report::default(),
                watched(PaneStatus::Idle)
            ),
            Decision::Send("first".to_owned())
        );
    }
}
