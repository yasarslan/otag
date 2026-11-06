//! Watching one session's limits, and doing what the user asked when they
//! run out.
//!
//! The night shift beside this module is about *getting work done* while
//! nobody is watching. The guard is about *not being surprised*: a session
//! the user is driving themselves runs into a five-hour window, and the
//! question is only what should happen at that moment. Three honest answers,
//! which are the three the user chooses between.
//!
//! ## What "pause" can and cannot mean
//!
//! Neither agent can be suspended. Both now have hooks of the same shape —
//! Codex's `~/.codex/hooks.json` carries `PreToolUse`, `PostToolUse`,
//! `UserPromptSubmit`, `SessionStart`, `SessionEnd`, `PermissionRequest`,
//! `Notification`, `Stop` and `Interrupt`, as Claude Code's do — and a hook
//! can only ever *deny* a tool call. Denying one would tell the agent the
//! user refused, which is a lie, and it would carry on and try something
//! else. Sending `Escape` does interrupt both CLIs, and it throws away the
//! turn in flight.
//!
//! So the guard stops **after the current turn**, for both agents, and the
//! interface says so in those words. It never interrupts work in progress,
//! never answers an agent on the user's behalf, and never stops the user
//! typing into a session themselves. What it governs is what *otag* sends.
//!
//! This module is the reasoning only: what to do, given what the limits say
//! and what the agent is doing. Doing it is the daemon's job.

use otag_agents::{AgentKind, PaneStatus};

use crate::nightshift::worst_used;
use crate::usage::Report;

/// The share of a window at which the guard acts.
///
/// The same ninety per cent the night shift stops at, and for the same
/// reason: it leaves room for the answer to a question already asked.
pub const DEFAULT_THRESHOLD: u16 = 90;

/// What otag should do when a session reaches its threshold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhenReached {
    /// Stop, and leave it to the user to carry on.
    Wait,
    /// Stop, and send `prompt` once the window has reset.
    Resume {
        /// What to type when there is room again.
        prompt: String,
    },
    /// Hand the work to the other agent and carry on there.
    ///
    /// When the other agent is over its own threshold too there is nowhere
    /// to go, so this falls back to [`WhenReached::Resume`] with `prompt` —
    /// which is why the prompt is carried here as well.
    Switch {
        /// What to type when the window has reset, if the handoff cannot
        /// happen.
        prompt: String,
    },
}

impl WhenReached {
    /// What otag types when a window has reset.
    #[must_use]
    pub fn prompt(&self) -> Option<&str> {
        match self {
            Self::Wait => None,
            Self::Resume { prompt } | Self::Switch { prompt } => {
                Some(prompt.as_str()).filter(|prompt| !prompt.is_empty())
            }
        }
    }
}

/// The resume prompt otag offers, which the user can change.
pub const DEFAULT_RESUME_PROMPT: &str =
    "Continue exactly where you stopped. Re-read your notes if needed.";

/// What the user asked the guard to do for one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Percentage of a window at which to act.
    pub threshold: u16,
    /// What to do when it is reached.
    pub when_reached: WhenReached,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            threshold: DEFAULT_THRESHOLD,
            when_reached: WhenReached::Wait,
        }
    }
}

/// What the guard has done for one session so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    /// Whether otag has stopped this session.
    pub paused: bool,
    /// When the window the guard is waiting on resets, in seconds since the
    /// epoch, when otag knows.
    pub resets_at: Option<i64>,
    /// Whether the work has been handed to the other agent already.
    pub handed_off: bool,
}

/// What the guard should do for a session right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Nothing: there is room, or a turn is in flight, or the user is needed.
    Wait,
    /// Stop sending. The threshold is reached. Carries the percentage.
    Pause(u16),
    /// There is room again; the session is no longer held.
    Resume,
    /// Type this into the session.
    Send(String),
    /// Hand the work to this agent and carry on there.
    HandOff(AgentKind),
}

/// What one session looks like to the guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Watched {
    /// The agent in the pane, which decides whose limits matter.
    pub agent: AgentKind,
    /// What that agent is doing.
    pub status: PaneStatus,
}

/// Decide what the guard should do for one session.
///
/// The order of the checks *is* the policy:
///
/// 1. A question, an error or a loop means the user is needed. otag must
///    never type past one of those.
/// 2. A turn in flight is left alone. This is the "after the current turn"
///    promise, and it is a check rather than a comment.
/// 3. Then the threshold, and then what the plan asked for.
#[must_use]
pub fn decide(plan: &Plan, progress: &Progress, usage: &Report, watched: Watched) -> Decision {
    // 1. The user is needed. Nothing the guard does could help.
    if matches!(
        watched.status,
        PaneStatus::NeedsApproval | PaneStatus::Error | PaneStatus::Looping
    ) {
        return Decision::Wait;
    }

    let used = worst_used(usage, watched.agent);
    let over = used >= plan.threshold;

    // 2. Work in progress is never interrupted, in either direction: otag
    //    does not stop a turn, and it does not type into one.
    if watched.status == PaneStatus::Working {
        return Decision::Wait;
    }

    if over {
        if progress.paused {
            return Decision::Wait;
        }
        // Switching agents is the one thing worth doing *at* the threshold
        // rather than after the window resets, because the point of it is
        // not to wait at all.
        if let WhenReached::Switch { .. } = plan.when_reached
            && !progress.handed_off
            && let Some(other) = somewhere_to_go(plan, usage, watched.agent)
        {
            return Decision::HandOff(other);
        }
        return Decision::Pause(used);
    }

    if !progress.paused {
        return Decision::Wait;
    }

    // There is room again, which means the window reset.
    match plan.when_reached.prompt() {
        // A plan that says "wait for me" has nothing to send; the session is
        // simply no longer held.
        None => Decision::Resume,
        Some(prompt) => Decision::Send(prompt.to_owned()),
    }
}

/// The other agent, when it has enough room to be worth switching to.
///
/// `None` means there is nowhere to go — the other agent is over the same
/// threshold — and the caller falls back to waiting for the window, which is
/// what the plan's own prompt is for.
#[must_use]
pub fn somewhere_to_go(plan: &Plan, usage: &Report, from: AgentKind) -> Option<AgentKind> {
    let other = other_than(from);
    (worst_used(usage, other) < plan.threshold).then_some(other)
}

/// The agent a session would be handed to.
#[must_use]
pub const fn other_than(agent: AgentKind) -> AgentKind {
    match agent {
        AgentKind::Claude => AgentKind::Codex,
        AgentKind::Codex | AgentKind::Deepseek => AgentKind::Claude,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{Row, Window};

    /// One window's reading.
    fn row(agent: AgentKind, window: Window, percent: u16) -> Row {
        Row {
            agent,
            window,
            used_tenths: Some(percent * 10),
            resets_at: None,
            level: crate::usage::Level::default(),
        }
    }

    /// A report saying one agent has used this much of its five-hour window.
    fn used(agent: AgentKind, percent: u16) -> Report {
        Report {
            rows: vec![row(agent, Window::FiveHour, percent)],
            ..Report::default()
        }
    }

    fn both(claude: u16, codex: u16) -> Report {
        let mut report = used(AgentKind::Claude, claude);
        report.rows.extend(used(AgentKind::Codex, codex).rows);
        report
    }

    fn idle(agent: AgentKind) -> Watched {
        Watched {
            agent,
            status: PaneStatus::Idle,
        }
    }

    fn waiting() -> Plan {
        Plan {
            threshold: 90,
            when_reached: WhenReached::Wait,
        }
    }

    fn resuming() -> Plan {
        Plan {
            threshold: 90,
            when_reached: WhenReached::Resume {
                prompt: DEFAULT_RESUME_PROMPT.to_owned(),
            },
        }
    }

    fn switching() -> Plan {
        Plan {
            threshold: 90,
            when_reached: WhenReached::Switch {
                prompt: DEFAULT_RESUME_PROMPT.to_owned(),
            },
        }
    }

    #[test]
    fn below_the_threshold_the_guard_does_nothing() {
        for plan in [waiting(), resuming(), switching()] {
            assert_eq!(
                decide(
                    &plan,
                    &Progress::default(),
                    &used(AgentKind::Claude, 50),
                    idle(AgentKind::Claude)
                ),
                Decision::Wait
            );
        }
    }

    #[test]
    fn a_waiting_plan_pauses_and_stays_paused() {
        let plan = waiting();
        let usage = used(AgentKind::Claude, 91);

        assert_eq!(
            decide(&plan, &Progress::default(), &usage, idle(AgentKind::Claude)),
            Decision::Pause(91)
        );

        // Already paused, and the plan has nothing to send: the user resumes.
        let paused = Progress {
            paused: true,
            ..Progress::default()
        };
        assert_eq!(
            decide(&plan, &paused, &usage, idle(AgentKind::Claude)),
            Decision::Wait
        );
    }

    #[test]
    fn a_waiting_plan_lets_go_when_the_window_resets() {
        let paused = Progress {
            paused: true,
            ..Progress::default()
        };
        assert_eq!(
            decide(
                &waiting(),
                &paused,
                &used(AgentKind::Claude, 3),
                idle(AgentKind::Claude)
            ),
            Decision::Resume,
            "nothing is typed, but the session is no longer held"
        );
    }

    #[test]
    fn a_resuming_plan_types_its_prompt_once_there_is_room() {
        let plan = resuming();
        assert_eq!(
            decide(
                &plan,
                &Progress::default(),
                &used(AgentKind::Claude, 95),
                idle(AgentKind::Claude)
            ),
            Decision::Pause(95)
        );

        let paused = Progress {
            paused: true,
            ..Progress::default()
        };
        assert_eq!(
            decide(
                &plan,
                &paused,
                &used(AgentKind::Claude, 2),
                idle(AgentKind::Claude)
            ),
            Decision::Send(DEFAULT_RESUME_PROMPT.to_owned())
        );
    }

    #[test]
    fn a_switching_plan_hands_the_work_over_at_the_threshold() {
        // The point of switching is not to wait, so it happens when the
        // threshold is reached rather than when the window resets.
        assert_eq!(
            decide(
                &switching(),
                &Progress::default(),
                &both(93, 10),
                idle(AgentKind::Claude)
            ),
            Decision::HandOff(AgentKind::Codex)
        );
        assert_eq!(
            decide(
                &switching(),
                &Progress::default(),
                &both(10, 93),
                idle(AgentKind::Codex)
            ),
            Decision::HandOff(AgentKind::Claude)
        );
    }

    #[test]
    fn switching_falls_back_to_waiting_for_the_window() {
        // Both agents are out of room, so there is nowhere to hand the work.
        // Falling back to the resume behaviour is what the user was told
        // would happen.
        let plan = switching();
        let usage = both(93, 97);

        assert_eq!(
            decide(&plan, &Progress::default(), &usage, idle(AgentKind::Claude)),
            Decision::Pause(93),
            "it pauses rather than handing over to an agent with no room"
        );
        assert_eq!(somewhere_to_go(&plan, &usage, AgentKind::Claude), None);

        // And then behaves exactly like a resuming plan.
        let paused = Progress {
            paused: true,
            ..Progress::default()
        };
        assert_eq!(
            decide(&plan, &paused, &both(1, 1), idle(AgentKind::Claude)),
            Decision::Send(DEFAULT_RESUME_PROMPT.to_owned())
        );
    }

    #[test]
    fn a_session_is_handed_over_only_once() {
        let handed = Progress {
            handed_off: true,
            ..Progress::default()
        };
        assert_eq!(
            decide(
                &switching(),
                &handed,
                &both(93, 10),
                idle(AgentKind::Claude)
            ),
            Decision::Pause(93),
            "two agents on one task is the thing to avoid"
        );
    }

    #[test]
    fn work_in_progress_is_never_interrupted() {
        // The promise the interface makes: otag stops after the current
        // turn. This is that promise as a check.
        let working = Watched {
            agent: AgentKind::Claude,
            status: PaneStatus::Working,
        };
        for plan in [waiting(), resuming(), switching()] {
            assert_eq!(
                decide(&plan, &Progress::default(), &both(99, 1), working),
                Decision::Wait,
                "the guard acted in the middle of a turn"
            );
        }
    }

    #[test]
    fn a_turn_is_never_typed_into_either() {
        let working = Watched {
            agent: AgentKind::Claude,
            status: PaneStatus::Working,
        };
        let paused = Progress {
            paused: true,
            ..Progress::default()
        };
        assert_eq!(
            decide(&resuming(), &paused, &used(AgentKind::Claude, 1), working),
            Decision::Wait
        );
    }

    #[test]
    fn a_session_that_needs_the_user_is_left_to_the_user() {
        for status in [
            PaneStatus::NeedsApproval,
            PaneStatus::Error,
            PaneStatus::Looping,
        ] {
            let watched = Watched {
                agent: AgentKind::Claude,
                status,
            };
            for plan in [waiting(), resuming(), switching()] {
                assert_eq!(
                    decide(&plan, &Progress::default(), &both(99, 1), watched),
                    Decision::Wait,
                    "the guard acted on a session showing {status:?}"
                );
            }
        }
    }

    #[test]
    fn the_threshold_reads_the_worse_of_the_two_windows() {
        // A weekly window nearly gone matters as much as a five-hour one.
        let mut usage = used(AgentKind::Claude, 10);
        usage.rows.push(row(AgentKind::Claude, Window::Weekly, 96));

        assert_eq!(
            decide(
                &waiting(),
                &Progress::default(),
                &usage,
                idle(AgentKind::Claude)
            ),
            Decision::Pause(96)
        );
    }

    #[test]
    fn an_agent_that_reports_nothing_is_not_held_up() {
        // Nothing reported is nothing to go on, and a guard that stopped a
        // session it cannot measure would stop it forever.
        assert_eq!(
            decide(
                &waiting(),
                &Progress::default(),
                &Report::default(),
                idle(AgentKind::Codex)
            ),
            Decision::Wait
        );
    }

    #[test]
    fn the_two_agents_are_each_others_other() {
        assert_eq!(other_than(AgentKind::Claude), AgentKind::Codex);
        assert_eq!(other_than(AgentKind::Codex), AgentKind::Claude);
    }

    #[test]
    fn a_plan_that_waits_has_nothing_to_type() {
        assert_eq!(WhenReached::Wait.prompt(), None);
        assert_eq!(
            WhenReached::Resume {
                prompt: "go on".to_owned()
            }
            .prompt(),
            Some("go on")
        );
        assert_eq!(
            WhenReached::Resume {
                prompt: String::new()
            }
            .prompt(),
            None,
            "an empty prompt is no prompt, and otag types nothing"
        );
    }
}
