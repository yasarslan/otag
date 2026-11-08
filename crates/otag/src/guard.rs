//! Running the limit guard: the part that actually acts.
//!
//! The rules live in [`otag_features::guard`]; this carries them out. One
//! task watches every session that has a plan and does what the rules
//! decide: stop at the threshold, type the resume prompt once the window has
//! reset, or hand the work to the other agent.
//!
//! It lives in the daemon rather than in a client because that is the only
//! way it works with every window closed, which is the point of guarding a
//! limit at all.
//!
//! Typing is done exactly as the night shift does it, through the same
//! `send_prompt`: pasted, with the return key a moment later, and never into
//! a pane that is working, asking a question or has failed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use otag_core::pane::PaneId;
use otag_core::proto::{Event, GuardAction, GuardPlan, GuardView};
use otag_core::session::Session;
use otag_features::guard::{
    DEFAULT_RESUME_PROMPT, DEFAULT_THRESHOLD, Decision, Plan, Progress, Watched, WhenReached,
    decide,
};
use otag_features::nightshift::worst_used;

use crate::events::Bus;

/// How often the guard looks at the sessions it is watching.
///
/// It also wakes on every change to the session, so this is the backstop
/// that notices a window resetting while nothing else happens.
const TICK: Duration = Duration::from_secs(20);

/// Everything the guard is watching, by session.
#[derive(Debug, Default)]
pub struct Guards {
    sessions: Mutex<HashMap<PaneId, Watching>>,
}

/// One session's plan and how far it has got.
#[derive(Debug)]
struct Watching {
    plan: Plan,
    progress: Progress,
}

impl Guards {
    /// Nothing being watched.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Watch a session, or stop watching it.
    pub fn set(&self, pane: PaneId, plan: Option<GuardPlan>) {
        let Ok(mut sessions) = self.sessions.lock() else {
            return;
        };

        let Some(asked) = plan else {
            sessions.remove(&pane);
            return;
        };

        let prompt = asked
            .prompt
            .map(|prompt| prompt.trim().to_owned())
            .unwrap_or_else(|| DEFAULT_RESUME_PROMPT.to_owned());
        let plan = Plan {
            threshold: asked
                .threshold
                .filter(|at| *at > 0 && *at <= 100)
                .unwrap_or(DEFAULT_THRESHOLD),
            when_reached: match asked.when_reached {
                GuardAction::Wait => WhenReached::Wait,
                GuardAction::Resume => WhenReached::Resume { prompt },
                GuardAction::Switch => WhenReached::Switch { prompt },
            },
        };

        match sessions.get_mut(&pane) {
            // A plan given again replaces the old one, and the session
            // starts unpaused: the user has just said what they want.
            Some(watching) => {
                watching.plan = plan;
                watching.progress = Progress::default();
            }
            None => {
                sessions.insert(
                    pane,
                    Watching {
                        plan,
                        progress: Progress::default(),
                    },
                );
            }
        }
    }

    /// What the guard is doing for one session, for a client.
    #[must_use]
    pub fn view(&self, pane: PaneId, used: u16) -> Option<GuardView> {
        let sessions = self.sessions.lock().ok()?;
        let watching = sessions.get(&pane)?;
        Some(GuardView {
            threshold: watching.plan.threshold,
            when_reached: match watching.plan.when_reached {
                WhenReached::Wait => GuardAction::Wait,
                WhenReached::Resume { .. } => GuardAction::Resume,
                WhenReached::Switch { .. } => GuardAction::Switch,
            },
            prompt: watching
                .plan
                .when_reached
                .prompt()
                .unwrap_or_default()
                .to_owned(),
            used,
            paused: watching.progress.paused,
            resets_at: watching.progress.resets_at,
            handed_off: watching.progress.handed_off,
        })
    }

    /// Forget every session that is not in `alive`.
    ///
    /// A pane whose program ended is nothing to guard, and a plan kept for
    /// it would be reported to clients forever.
    fn prune(&self, alive: &[PaneId]) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.retain(|pane, _| alive.contains(pane));
        }
    }
}

/// Watch every guarded session until the daemon stops.
pub async fn keep_watching(
    session: Arc<Session>,
    guards: Arc<Guards>,
    bus: Bus,
    handoffs: Arc<Mutex<crate::handoff::Recent>>,
) {
    let mut changes = session.changes();
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        guards.prune(
            &session
                .summaries()
                .iter()
                .map(|pane| pane.id)
                .collect::<Vec<_>>(),
        );

        for action in due(&session, &guards) {
            carry_out(&session, &guards, &bus, &handoffs, action).await;
        }

        tokio::select! {
            changed = changes.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            _ = ticker.tick() => {}
        }
    }
}

/// One thing to do for one session.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Pause {
        pane: PaneId,
        used: u16,
        resets_at: Option<i64>,
    },
    Resume {
        pane: PaneId,
    },
    Send {
        pane: PaneId,
        prompt: String,
    },
    HandOff {
        pane: PaneId,
        to: otag_agents::AgentKind,
    },
}

/// What the rules say should happen now, for every guarded session.
fn due(session: &Arc<Session>, guards: &Arc<Guards>) -> Vec<Action> {
    let usage = session.usage();
    let panes = session.agent_panes();
    let Ok(sessions) = guards.sessions.lock() else {
        return Vec::new();
    };
    let mut actions = Vec::new();

    for (pane, watching) in sessions.iter() {
        let Some(known) = panes.iter().find(|known| known.id == *pane) else {
            // The pane is gone; there is nothing to guard.
            continue;
        };
        let Some(agent) = known.agent else { continue };

        let watched = Watched {
            agent,
            status: known.status,
        };
        match decide(&watching.plan, &watching.progress, &usage, watched) {
            Decision::Pause(used) => actions.push(Action::Pause {
                pane: *pane,
                used,
                resets_at: resets_at(&usage, agent),
            }),
            Decision::Resume => actions.push(Action::Resume { pane: *pane }),
            Decision::Send(prompt) => actions.push(Action::Send {
                pane: *pane,
                prompt,
            }),
            Decision::HandOff(to) => actions.push(Action::HandOff { pane: *pane, to }),
            Decision::Wait => {}
        }
    }

    actions
}

/// When the window that is holding this agent up resets.
///
/// The worst window's own reset, so "paused until 02:40" names the moment
/// the pause actually ends rather than the next reset of any window.
fn resets_at(usage: &otag_features::usage::Report, agent: otag_agents::AgentKind) -> Option<i64> {
    let worst = worst_used(usage, agent);
    usage
        .rows
        .iter()
        .filter(|row| row.agent == agent)
        .filter(|row| row.used_pct() == Some(worst))
        .filter_map(|row| row.resets_at)
        .min()
}

/// Do one thing, and tell the clients it was done.
async fn carry_out(
    session: &Arc<Session>,
    guards: &Arc<Guards>,
    bus: &Bus,
    handoffs: &Arc<Mutex<crate::handoff::Recent>>,
    action: Action,
) {
    match action {
        Action::Pause {
            pane,
            used,
            resets_at,
        } => {
            if let Ok(mut sessions) = guards.sessions.lock()
                && let Some(watching) = sessions.get_mut(&pane)
            {
                watching.progress.paused = true;
                watching.progress.resets_at = resets_at;
            }
            tracing::info!(%pane, used, "the limit guard paused a session");
            session.note_team_attention(pane).await;
            bus.publish(Event::Notice {
                pane: Some(pane),
                text: format!(
                    "limit guard: stopped at {used}%. otag sends this session nothing until \
                     the window resets; you can still type into it."
                ),
            });
        }

        Action::Resume { pane } => {
            if let Ok(mut sessions) = guards.sessions.lock()
                && let Some(watching) = sessions.get_mut(&pane)
            {
                watching.progress.paused = false;
                watching.progress.resets_at = None;
            }
            tracing::info!(%pane, "the limit guard let a session go");
            bus.publish(Event::Notice {
                pane: Some(pane),
                text: "limit guard: the window reset, this session is free again".to_owned(),
            });
        }

        Action::Send { pane, prompt } => {
            // Marked as no longer paused *before* typing: a prompt that was
            // sent and not recorded could be sent twice, which is the one
            // mistake that cannot be taken back.
            let recorded = {
                let Ok(mut sessions) = guards.sessions.lock() else {
                    return;
                };
                let Some(watching) = sessions.get_mut(&pane) else {
                    return;
                };
                watching.progress.paused = false;
                watching.progress.resets_at = None;
                true
            };
            if !recorded {
                return;
            }

            if let Err(error) = crate::nightshift::send_prompt(session, pane, &prompt).await {
                tracing::warn!(%error, %pane, "the limit guard could not send a prompt");
                return;
            }
            tracing::info!(%pane, "the limit guard carried on after a reset");
            bus.publish(Event::Notice {
                pane: Some(pane),
                text: "limit guard: the window reset, carrying on".to_owned(),
            });
        }

        Action::HandOff { pane, to } => {
            // Recorded first, for the same reason: two agents on one task is
            // the outcome to avoid.
            {
                let Ok(mut sessions) = guards.sessions.lock() else {
                    return;
                };
                let Some(watching) = sessions.get_mut(&pane) else {
                    return;
                };
                if watching.progress.handed_off {
                    return;
                }
                watching.progress.handed_off = true;
            }

            // The same guard the button uses, so a handoff the user asked
            // for and one the guard asked for cannot collide.
            if let Ok(mut recent) = handoffs.lock()
                && let Err(message) = recent.claim(pane)
            {
                tracing::info!(%pane, message, "the limit guard's handoff was refused");
                return;
            }

            tracing::info!(%pane, %to, "the limit guard is handing a session over");
            match crate::handoff::run(
                session,
                pane,
                crate::handoff::Mode::SwitchAgent { to: Some(to) },
            )
            .await
            {
                Ok(done) => {
                    bus.publish(Event::Handoff {
                        pane: done.pane,
                        package: done.package,
                        summary: done.summary,
                    });
                }
                Err(message) => {
                    if let Ok(mut recent) = handoffs.lock() {
                        recent.release(pane);
                    }
                    // The handoff did not happen, so the session falls back
                    // to waiting for its window — which is what the user was
                    // told would happen.
                    if let Ok(mut sessions) = guards.sessions.lock()
                        && let Some(watching) = sessions.get_mut(&pane)
                    {
                        watching.progress.handed_off = false;
                        watching.progress.paused = true;
                    }
                    tracing::warn!(%pane, %message, "the limit guard could not hand over");
                    bus.publish(Event::Notice {
                        pane: Some(pane),
                        text: format!(
                            "limit guard: could not switch agent ({message}). Waiting for the \
                             window to reset instead."
                        ),
                    });
                }
            }
        }
    }
}
