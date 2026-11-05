//! Running the night shift: the part that actually types.
//!
//! The rules live in [`otag_features::nightshift`]; this is what carries
//! them out. One task watches every session that has a plan and acts on what
//! the rules decide: send the next prompt, stop at the threshold, start
//! again when the window has reset.
//!
//! Typing into a running agent is the dangerous part, so it is done the way
//! a person would: the text is pasted — in paste markers when the program
//! asked for them — and the return key follows a moment later, once the
//! agent's interface has had time to take the text in. Nothing is ever typed
//! into a pane that is asking a question, has failed, or is already working.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use otag_core::pane::PaneId;
use otag_core::proto::{Event, NightShiftPlan, NightShiftView, ReportEntry, SessionReport};
use otag_core::session::Session;
use otag_features::nightshift::{Decision, Plan, Progress, Watched, decide, now};

use crate::events::Bus;

/// How often the night shift looks at the sessions it is working for.
///
/// It also wakes on every change to the session, so this is only the
/// backstop that notices a window resetting while nothing else happens.
const TICK: Duration = Duration::from_secs(20);

/// How long the text of a prompt is given to land before the return key.
const BEFORE_RETURN: Duration = Duration::from_millis(250);

/// Everything the night shift is doing, by session.
#[derive(Debug, Default)]
pub struct Shifts {
    sessions: Mutex<HashMap<PaneId, Running>>,
}

/// One session's plan and how far it has got.
#[derive(Debug)]
struct Running {
    plan: Plan,
    progress: Progress,
    /// Whether the end of the queue has been announced, so it is announced
    /// once rather than on every look.
    announced: bool,
    /// When the plan was given, for the morning report's commit list.
    started_at: i64,
}

impl Shifts {
    /// Nothing being worked on.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Take on a plan for a session, or stop working for it.
    ///
    /// What otag has already done is kept either way: the morning report is
    /// a record of the night, not of what is still running.
    pub fn set(&self, pane: PaneId, plan: Option<NightShiftPlan>) {
        let Ok(mut sessions) = self.sessions.lock() else {
            return;
        };

        match plan {
            Some(asked) => {
                let plan = Plan {
                    pause_at: asked
                        .pause_at
                        .filter(|at| *at > 0 && *at <= 100)
                        .unwrap_or(otag_features::nightshift::DEFAULT_PAUSE_AT),
                    resume_prompt: asked.resume_prompt.unwrap_or_default(),
                    queue: asked
                        .queue
                        .into_iter()
                        .map(|prompt| prompt.trim().to_owned())
                        .filter(|prompt| !prompt.is_empty())
                        .collect(),
                };
                let at = now();
                match sessions.get_mut(&pane) {
                    // A plan given again replaces the old one, and the
                    // queue starts from the beginning of what is new.
                    Some(running) => {
                        running.plan = plan;
                        running.progress.sent = 0;
                        running.progress.paused = false;
                        running.announced = false;
                        running.progress.record(at, "night shift updated");
                    }
                    None => {
                        let mut progress = Progress::default();
                        progress.record(at, "night shift started");
                        sessions.insert(
                            pane,
                            Running {
                                plan,
                                progress,
                                announced: false,
                                started_at: at,
                            },
                        );
                    }
                }
            }
            None => {
                if let Some(running) = sessions.get_mut(&pane) {
                    running.plan = Plan {
                        queue: Vec::new(),
                        resume_prompt: String::new(),
                        ..running.plan.clone()
                    };
                    running.announced = true;
                    running.progress.record(now(), "night shift stopped");
                }
            }
        }
    }

    /// What the night shift is doing for one session, for a client.
    #[must_use]
    pub fn view(&self, pane: PaneId) -> Option<NightShiftView> {
        let sessions = self.sessions.lock().ok()?;
        let running = sessions.get(&pane)?;
        if running.plan.is_empty() && running.progress.sent == 0 {
            return None;
        }
        Some(NightShiftView {
            pause_at: running.plan.pause_at,
            paused: running.progress.paused,
            sent: running.progress.sent,
            queued: running.plan.queue.len(),
            resume_prompt: running.plan.resume_prompt.clone(),
        })
    }

    /// What otag did for every session it worked for.
    #[must_use]
    pub fn reports(&self) -> Vec<(PaneId, i64, usize, usize, Vec<ReportEntry>)> {
        let Ok(sessions) = self.sessions.lock() else {
            return Vec::new();
        };
        let mut reports: Vec<_> = sessions
            .iter()
            .map(|(pane, running)| {
                (
                    *pane,
                    running.started_at,
                    running.progress.sent,
                    running
                        .plan
                        .queue
                        .len()
                        .saturating_sub(running.progress.sent),
                    running
                        .progress
                        .log
                        .iter()
                        .map(|entry| ReportEntry {
                            at: entry.at,
                            what: entry.what.clone(),
                        })
                        .collect(),
                )
            })
            .collect();
        reports.sort_by_key(|(pane, ..)| *pane);
        reports
    }
}

/// Work for every session that has a plan, until the daemon stops.
pub async fn keep_working(session: Arc<Session>, shifts: Arc<Shifts>, bus: Bus) {
    let mut changes = session.changes();
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        for action in due(&session, &shifts) {
            carry_out(&session, &shifts, &bus, action).await;
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
    Send { pane: PaneId, prompt: String },
    Pause { pane: PaneId, used: u16 },
    Resume { pane: PaneId },
    Finished { pane: PaneId },
}

/// What the rules say should happen now, for every session with a plan.
fn due(session: &Arc<Session>, shifts: &Arc<Shifts>) -> Vec<Action> {
    let usage = session.usage();
    let panes = session.agent_panes();
    let Ok(sessions) = shifts.sessions.lock() else {
        return Vec::new();
    };
    let at = now();
    let mut actions = Vec::new();

    for (pane, running) in sessions.iter() {
        let Some(known) = panes.iter().find(|known| known.id == *pane) else {
            // The pane is gone; there is nothing to send and nothing to say.
            continue;
        };
        let Some(agent) = known.agent else { continue };

        let watched = Watched {
            agent,
            status: known.status,
            now: at,
        };
        match decide(&running.plan, &running.progress, &usage, watched) {
            Decision::Send(prompt) => actions.push(Action::Send {
                pane: *pane,
                prompt,
            }),
            Decision::Pause(used) => actions.push(Action::Pause { pane: *pane, used }),
            Decision::Resume => actions.push(Action::Resume { pane: *pane }),
            Decision::Finished if !running.announced => {
                actions.push(Action::Finished { pane: *pane });
            }
            Decision::Finished | Decision::Wait => {}
        }
    }

    actions
}

/// Do one thing, and write down that it was done.
async fn carry_out(session: &Arc<Session>, shifts: &Arc<Shifts>, bus: &Bus, action: Action) {
    match action {
        Action::Send { pane, prompt } => {
            // Written down *before* sending: a prompt that was sent and not
            // recorded could be sent twice, which is the one mistake that
            // cannot be taken back.
            let short = shorten(&prompt);
            let recorded = {
                let Ok(mut sessions) = shifts.sessions.lock() else {
                    return;
                };
                let Some(running) = sessions.get_mut(&pane) else {
                    return;
                };
                running.progress.sent = running.progress.sent.saturating_add(1);
                running.progress.last_sent_at = Some(now());
                running.progress.record(now(), format!("sent: {short}"));
                true
            };
            if !recorded {
                return;
            }

            if let Err(error) = send_prompt(session, pane, &prompt).await {
                tracing::warn!(%error, %pane, "the night shift could not send a prompt");
                if let Ok(mut sessions) = shifts.sessions.lock()
                    && let Some(running) = sessions.get_mut(&pane)
                {
                    running
                        .progress
                        .record(now(), format!("could not send: {error}"));
                }
                return;
            }

            tracing::info!(%pane, prompt = %short, "the night shift sent a prompt");
            bus.publish(Event::Notice {
                pane: Some(pane),
                text: format!("night shift: sent \"{short}\""),
            });
        }

        Action::Pause { pane, used } => {
            if let Ok(mut sessions) = shifts.sessions.lock()
                && let Some(running) = sessions.get_mut(&pane)
            {
                running.progress.paused = true;
                running
                    .progress
                    .record(now(), format!("paused at {used}% of the window"));
            }
            tracing::info!(%pane, used, "the night shift paused a session");
            bus.publish(Event::Notice {
                pane: Some(pane),
                text: format!("night shift: paused at {used}%, waiting for the window to reset"),
            });
        }

        Action::Resume { pane } => {
            if let Ok(mut sessions) = shifts.sessions.lock()
                && let Some(running) = sessions.get_mut(&pane)
            {
                running.progress.paused = false;
                running
                    .progress
                    .record(now(), "the window reset; carrying on");
            }
            tracing::info!(%pane, "the night shift carried on after a reset");
            bus.publish(Event::Notice {
                pane: Some(pane),
                text: "night shift: the window reset, carrying on".to_owned(),
            });
        }

        Action::Finished { pane } => {
            if let Ok(mut sessions) = shifts.sessions.lock()
                && let Some(running) = sessions.get_mut(&pane)
            {
                running.announced = true;
                running.progress.record(now(), "the queue is finished");
            }
            tracing::info!(%pane, "the night shift finished a queue");
            bus.publish(Event::Notice {
                pane: Some(pane),
                text: "night shift: everything queued has been sent".to_owned(),
            });
        }
    }
}

/// Type a prompt into a pane and press return.
///
/// The text is wrapped in paste markers when the program has asked for them,
/// which is what tells an agent's interface that a block of text is one
/// paste rather than a burst of keys.
pub(crate) async fn send_prompt(
    session: &Arc<Session>,
    pane: PaneId,
    prompt: &str,
) -> otag_core::Result<()> {
    let handle = session
        .pane(pane)
        .ok_or(otag_core::Error::PaneGone { id: Some(pane) })?;
    let bracketed = handle.snapshot().screen.modes().bracketed_paste;

    let mut text = Vec::new();
    if bracketed {
        text.extend_from_slice(b"\x1b[200~");
    }
    // Carriage returns inside a prompt would submit it early, so a prompt
    // is sent as one line.
    text.extend(prompt.replace(['\r', '\n'], " ").into_bytes());
    if bracketed {
        text.extend_from_slice(b"\x1b[201~");
    }
    handle.input(text).await?;

    // The return key follows separately: an agent's interface needs a
    // moment to take in a paste before it is submitted.
    tokio::time::sleep(BEFORE_RETURN).await;
    handle.input(b"\r".to_vec()).await
}

/// A prompt, short enough for a log line or a notification.
fn shorten(prompt: &str) -> String {
    /// How much of a prompt is worth repeating.
    const LIMIT: usize = 60;

    let one_line = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= LIMIT {
        return one_line;
    }
    let kept: String = one_line.chars().take(LIMIT).collect();
    format!("{}…", kept.trim_end())
}

/// Build the morning report: what otag did, and what came of it.
pub async fn report(session: &Arc<Session>, shifts: &Arc<Shifts>) -> Vec<SessionReport> {
    let summaries = session.summaries();
    let panes = session.agent_panes();
    let mut reports = Vec::new();

    for (pane, started_at, sent, remaining, log) in shifts.reports() {
        let summary = summaries.iter().find(|summary| summary.id == pane);
        let cwd = panes
            .iter()
            .find(|known| known.id == pane)
            .and_then(|known| known.cwd.clone());
        let commits = match cwd {
            Some(cwd) => crate::git::commits_since(&cwd, started_at).await,
            None => Vec::new(),
        };

        reports.push(SessionReport {
            pane,
            workspace: summary
                .map(|summary| summary.workspace.clone())
                .unwrap_or_default(),
            title: summary
                .map(|summary| summary.title.clone())
                .unwrap_or_default(),
            agent: summary.and_then(|summary| summary.agent),
            tokens: summary.map(|summary| summary.tokens).unwrap_or_default(),
            sent,
            remaining,
            log,
            commits,
        });
    }

    reports
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_with_nothing_in_it_is_not_shown_as_a_night_shift() {
        let shifts = Shifts::new();
        shifts.set(PaneId::FIRST, Some(NightShiftPlan::default()));

        assert_eq!(
            shifts.view(PaneId::FIRST),
            None,
            "an empty plan has nothing to report"
        );
        assert_eq!(
            shifts.reports().len(),
            1,
            "it is still being worked for, and still reported"
        );
    }

    #[test]
    fn a_plan_keeps_the_queue_and_the_threshold_it_was_given() {
        let shifts = Shifts::new();
        shifts.set(
            PaneId::FIRST,
            Some(NightShiftPlan {
                pause_at: Some(70),
                resume_prompt: Some("carry on".to_owned()),
                queue: vec!["  one  ".to_owned(), String::new(), "two".to_owned()],
            }),
        );

        let view = shifts.view(PaneId::FIRST).expect("a night shift");
        assert_eq!(view.pause_at, 70);
        assert_eq!(view.queued, 2, "an empty prompt is not a prompt");
        assert_eq!(view.resume_prompt, "carry on");
        assert_eq!(view.sent, 0);
        assert!(!view.paused);
    }

    #[test]
    fn an_impossible_threshold_falls_back_to_the_default() {
        let shifts = Shifts::new();
        shifts.set(
            PaneId::FIRST,
            Some(NightShiftPlan {
                pause_at: Some(0),
                resume_prompt: None,
                queue: vec!["one".to_owned()],
            }),
        );

        assert_eq!(
            shifts.view(PaneId::FIRST).expect("a night shift").pause_at,
            otag_features::nightshift::DEFAULT_PAUSE_AT
        );
    }

    #[test]
    fn stopping_keeps_what_was_done_for_the_report() {
        let shifts = Shifts::new();
        shifts.set(
            PaneId::FIRST,
            Some(NightShiftPlan {
                pause_at: None,
                resume_prompt: None,
                queue: vec!["one".to_owned()],
            }),
        );
        shifts.set(PaneId::FIRST, None);

        assert_eq!(shifts.view(PaneId::FIRST), None, "nothing left to do");
        let reports = shifts.reports();
        let (pane, _, _, _, log) = reports.first().expect("the session is still reported");
        assert_eq!(*pane, PaneId::FIRST);
        assert!(
            log.iter().any(|entry| entry.what == "night shift stopped"),
            "{log:?}"
        );
    }

    #[test]
    fn a_prompt_is_shortened_for_a_log_line_without_losing_its_start() {
        assert_eq!(shorten("  run   the tests "), "run the tests");
        let long = "a".repeat(100);
        let short = shorten(&long);
        assert_eq!(
            short.chars().count(),
            61,
            "sixty characters and an ellipsis"
        );
        assert!(short.ends_with('…'));
    }
}
