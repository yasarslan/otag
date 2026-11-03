//! How much of each agent's limits is gone.
//!
//! An agent tells otag how much of its windows it has used as a percentage,
//! every time it answers. otag keeps those readings and shows the last one
//! for each window: the percentage, and when the window resets. Nothing
//! else. What a client draws is what the agent last said, so the number in
//! otag and the number in the agent are never two different numbers.
//!
//! The burn rate and the projection built on it are still measured here —
//! the handoff decides on them — but no client shows them: a prediction
//! beside a reading makes the reading look like a prediction too.
//!
//! Two rules run through this module. Everything an agent says is optional,
//! so a window otag has never been told about is a dash rather than a zero.
//! And nothing is invented: when otag has no percentage, it says so and
//! reports the tokens it counted instead, marked as an estimate.

use std::time::{SystemTime, UNIX_EPOCH};

use otag_agents::{AgentKind, LimitWindow, Limits};
use serde::{Deserialize, Serialize};

/// How long samples are kept.
pub const HISTORY: i64 = 7 * 24 * 60 * 60;

/// How far back the burn rate of the five-hour window is measured.
///
/// Short on purpose: five hours is a sitting, and what matters is the pace
/// of the work going on now.
pub const BURN_WINDOW: i64 = 20 * 60;

/// How far back the burn rate of the weekly window is measured.
///
/// A week is not a sitting. Twenty minutes of a busy afternoon, stretched
/// across seven days, says the week will be gone by the evening — which is
/// how a weekly window at a third full came to read "full in 4h38m". A day
/// of readings is long enough to average a working day's pace and short
/// enough to notice a change of pace.
pub const WEEKLY_BURN_WINDOW: i64 = 24 * 60 * 60;

/// How many samples a projection needs.
///
/// Two points always make a line, and the line through two samples taken a
/// second apart is nonsense. Three is the smallest number that can disagree
/// with itself.
pub const BURN_SAMPLES: usize = 3;

/// Where the widget turns amber, as a percentage.
pub const AMBER: f32 = 70.0;

/// Where the widget turns red and otag offers a handoff, as a percentage.
pub const RED: f32 = 90.0;

/// One of an agent's limit windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Window {
    /// The shorter window, five hours for both agents otag knows.
    FiveHour,
    /// The longer window, a week for both agents otag knows.
    Weekly,
}

impl Window {
    /// Both windows, shorter first.
    pub const ALL: [Self; 2] = [Self::FiveHour, Self::Weekly];

    /// How long the window runs for, in seconds.
    #[must_use]
    pub const fn length(self) -> i64 {
        match self {
            Self::FiveHour => 5 * 60 * 60,
            Self::Weekly => 7 * 24 * 60 * 60,
        }
    }

    /// How far back this window's burn rate is measured, in seconds.
    ///
    /// Proportional to the window itself: a rate measured over a twentieth
    /// of the window is a rate, and a rate measured over a thousandth of it
    /// is a burst.
    #[must_use]
    pub const fn burn_window(self) -> i64 {
        match self {
            Self::FiveHour => BURN_WINDOW,
            Self::Weekly => WEEKLY_BURN_WINDOW,
        }
    }

    /// What the widget calls it, in as few columns as possible.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::FiveHour => "5h",
            Self::Weekly => "week",
        }
    }
}

/// How worried the user should be about a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// There is plenty left.
    #[default]
    Calm,
    /// Worth knowing about.
    Amber,
    /// Nearly gone; otag offers to hand the work over.
    Red,
}

impl Level {
    /// The level a percentage falls into.
    #[must_use]
    pub fn of(used_pct: f32) -> Self {
        if used_pct >= RED {
            Self::Red
        } else if used_pct >= AMBER {
            Self::Amber
        } else {
            Self::Calm
        }
    }
}

/// One reading of one window.
///
/// Not `Copy`: a reading remembers which of the agent's sessions reported it,
/// so that a client can say where a number came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    /// Which agent it belongs to.
    pub agent: AgentKind,
    /// Which window it reads.
    pub window: Window,
    /// How much of the window is used, as a percentage.
    pub used_pct: f32,
    /// When the window resets, in seconds since the epoch.
    #[serde(default)]
    pub resets_at: Option<i64>,
    /// When the reading was taken, in seconds since the epoch.
    pub at: i64,
    /// The agent session that reported it, when it said which.
    ///
    /// Usage is account-wide, so a reading can come from any session on the
    /// machine, including one otag is not running. This is how a client says
    /// which one otag heard from last.
    #[serde(default)]
    pub session: Option<String>,
}

impl Sample {
    /// Whether the window this reading belongs to has reset since.
    ///
    /// The percentage in such a reading describes a window that is over, so
    /// it says nothing about the one running now, however recently the
    /// reading itself arrived.
    #[must_use]
    pub fn has_reset(&self, now: i64) -> bool {
        matches!(self.resets_at, Some(resets_at) if resets_at <= now)
    }
}

/// What otag counted itself, when no agent told it a percentage.
///
/// Tokens are not a percentage and must never be shown as one: nobody knows
/// what an account's allowance is. They are still worth showing, because
/// they say which agent is spending and roughly how fast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Estimate {
    /// Everything that had to be paid for, cache reads included.
    pub tokens: u64,
    /// When it was counted, in seconds since the epoch.
    pub at: i64,
}

/// Every sample otag has, newest last.
#[derive(Debug, Clone, Default)]
pub struct Usage {
    samples: Vec<Sample>,
    estimates: Vec<(AgentKind, Estimate)>,
}

impl Usage {
    /// An empty history.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Keep a sample, and forget what is older than a week.
    ///
    /// A reading identical to the newest one for that window is kept too: the
    /// burn rate needs to know that time passed without the number moving,
    /// which is what tells "stopped" apart from "still climbing".
    pub fn record(&mut self, sample: Sample) {
        let cutoff = sample.at.saturating_sub(HISTORY);
        self.samples.push(sample);
        self.samples.retain(|kept| kept.at >= cutoff);
    }

    /// Keep everything an agent reported in one go.
    ///
    /// `session` is the agent session that reported it, when it said which;
    /// usage is account-wide, so that is the only thing tying a reading to
    /// where it came from.
    pub fn record_limits(
        &mut self,
        agent: AgentKind,
        limits: &Limits,
        at: i64,
        session: Option<&str>,
    ) {
        if let Some(window) = limits.five_hour {
            self.record(sample(agent, Window::FiveHour, window, at, session));
        }
        if let Some(window) = limits.weekly {
            self.record(sample(agent, Window::Weekly, window, at, session));
        }
    }

    /// Keep what otag counted for an agent itself.
    pub fn record_estimate(&mut self, agent: AgentKind, estimate: Estimate) {
        match self.estimates.iter_mut().find(|(kept, _)| *kept == agent) {
            Some((_, kept)) => *kept = estimate,
            None => self.estimates.push((agent, estimate)),
        }
    }

    /// Replace everything with what was read back from the database.
    pub fn load(&mut self, samples: Vec<Sample>) {
        self.samples = samples;
        self.samples.sort_unstable_by_key(|sample| sample.at);
    }

    /// Every sample, oldest first.
    #[must_use]
    pub fn samples(&self) -> &[Sample] {
        &self.samples
    }

    /// The reading that describes a window as it stands now.
    ///
    /// Readings arrive from every session on the machine, and the newest
    /// arrival is not the best informed one. A session that has been idle
    /// for hours re-runs its status line on a timer and reports the
    /// `rate_limits` snapshot it was handed when it last answered — which
    /// can name a window that ended days ago. A resumed session does the
    /// same before its first reply.
    ///
    /// So readings are ranked by the window they describe rather than by
    /// when they arrived. A window is named by when it resets, and a later
    /// reset is a later window; a reading that does not say when its window
    /// resets is placed by when it arrived, which puts it ahead of a window
    /// whose reset has passed and behind one still to come. Within one
    /// window the percentage only climbs, so the highest reading is the
    /// latest state of it, and the newest arrival at that height wins the
    /// tie and so carries the window's freshness.
    #[must_use]
    pub fn latest(&self, agent: AgentKind, window: Window) -> Option<Sample> {
        self.samples
            .iter()
            .filter(|sample| sample.agent == agent && sample.window == window)
            .max_by_key(|sample| {
                (
                    sample.resets_at.unwrap_or(sample.at),
                    tenths(sample.used_pct),
                    sample.at,
                )
            })
            .cloned()
    }

    /// What otag counted for an agent itself.
    #[must_use]
    pub fn estimate(&self, agent: AgentKind) -> Option<Estimate> {
        self.estimates
            .iter()
            .find(|(kept, _)| *kept == agent)
            .map(|(_, estimate)| *estimate)
    }

    /// How fast a window is filling, in percentage points per hour.
    ///
    /// A least-squares slope over the samples of this window's own burn
    /// window — twenty minutes for the five-hour one, a day for the weekly
    /// one — so that one turn that cost a lot does not become a prediction
    /// about a week. Nothing is reported until there are three samples
    /// spread over real time, or when the number is not climbing.
    ///
    /// The measurement never reaches back past the start of the window being
    /// measured: percentages from the window before it describe a different
    /// allowance, and the drop between them is not a rate.
    #[must_use]
    pub fn burn_rate(&self, agent: AgentKind, window: Window, now: i64) -> Option<f32> {
        let mut cutoff = now.saturating_sub(window.burn_window());
        if let Some(resets_at) = self
            .latest(agent, window)
            .and_then(|latest| latest.resets_at)
        {
            cutoff = cutoff.max(resets_at.saturating_sub(window.length()));
        }
        let recent: Vec<&Sample> = self
            .samples
            .iter()
            .filter(|sample| {
                sample.agent == agent && sample.window == window && sample.at >= cutoff
            })
            .collect();
        if recent.len() < BURN_SAMPLES {
            return None;
        }

        let count = recent.len() as f64;
        let mean_at = recent.iter().map(|sample| sample.at as f64).sum::<f64>() / count;
        let mean_used = f64::from(recent.iter().map(|sample| sample.used_pct).sum::<f32>()) / count;

        let mut covariance = 0.0;
        let mut variance = 0.0;
        for sample in &recent {
            let offset = sample.at as f64 - mean_at;
            covariance += offset * (f64::from(sample.used_pct) - mean_used);
            variance += offset * offset;
        }
        if variance <= f64::EPSILON {
            // Every sample arrived in the same second, so no time has passed
            // to divide by.
            return None;
        }

        let per_second = covariance / variance;
        let per_hour = per_second * 3600.0;
        (per_hour > 0.0).then_some(per_hour as f32)
    }

    /// How long until a window is used up, in seconds.
    ///
    /// Nothing is reported when the number is not climbing, nor when the
    /// window resets before it would fill: a projection past the reset is
    /// not a limit the user will hit. Nothing is reported either when the
    /// newest reading is from a window that has already reset, because then
    /// otag does not know what the current percentage is to project from.
    ///
    /// The weekly window is projected only when otag knows when it resets.
    /// Running out of a week is a thing worth saying, but a projection days
    /// ahead with nothing to measure it against is a guess, and a guess with
    /// a number on it reads like knowledge.
    #[must_use]
    pub fn full_in(&self, agent: AgentKind, window: Window, now: i64) -> Option<i64> {
        let latest = self.latest(agent, window)?;
        if latest.has_reset(now) {
            return None;
        }
        let per_hour = self.burn_rate(agent, window, now)?;
        let left = 100.0 - latest.used_pct;
        if left <= 0.0 {
            return Some(0);
        }

        let seconds = (f64::from(left) / f64::from(per_hour) * 3600.0).round();
        if !seconds.is_finite() || seconds < 0.0 {
            return None;
        }
        let seconds = seconds as i64;

        match latest.resets_at {
            Some(resets_at) if resets_at.saturating_sub(now) <= seconds => None,
            None if window == Window::Weekly => None,
            _ => Some(seconds),
        }
    }

    /// Everything the widget and `otag usage` show.
    ///
    /// One row per window, carrying the three facts a client draws: whose
    /// window it is, how much of it the agent last reported, and when it
    /// resets. A window whose reset has passed reads zero until a reading of
    /// the new one arrives — the allowance is back, and that is what zero
    /// means. A window otag has never had a reading for carries no
    /// percentage at all, which clients draw as a dash.
    #[must_use]
    pub fn report(&self, now: i64) -> Report {
        let mut rows = Vec::new();
        for agent in AgentKind::ALL {
            for window in Window::ALL {
                let sample = self.latest(agent, window);
                let (used_tenths, resets_at) = match &sample {
                    // The window is over, so the percentage of it is over
                    // too, and the reset it named is in the past. The new
                    // window starts empty and otag does not know when it
                    // ends until an agent says so.
                    Some(sample) if sample.has_reset(now) => (Some(0), None),
                    Some(sample) => (Some(tenths(sample.used_pct)), sample.resets_at),
                    // Nothing has ever been reported for this window.
                    None if agent.shows_limit_windows() => (None, None),
                    None => continue,
                };
                rows.push(Row {
                    agent,
                    window,
                    used_tenths,
                    resets_at,
                    level: used_tenths
                        .map_or_else(Level::default, |tenths| Level::of(f32::from(tenths) / 10.0)),
                });
            }
        }

        let mut counted: Vec<Counted> = self
            .estimates
            .iter()
            .filter(|(agent, _)| {
                // An agent that reports percentages needs no estimate; one
                // that does not is all otag has to go on.
                Window::ALL
                    .into_iter()
                    .all(|window| self.latest(*agent, window).is_none())
            })
            .map(|(agent, estimate)| Counted {
                agent: *agent,
                tokens: estimate.tokens,
            })
            .collect();
        counted.sort_unstable_by_key(|counted| counted.agent);

        Report { rows, counted }
    }
}

/// How much of the limits is gone, ready to be drawn or printed.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Report {
    /// One row per window an agent has reported, shorter window first.
    #[serde(default)]
    pub rows: Vec<Row>,
    /// What otag counted itself, for the agents that report no percentage.
    #[serde(default)]
    pub counted: Vec<Counted>,
}

impl Report {
    /// Whether no agent has reported anything at all.
    ///
    /// Not the same as having no rows: the windows that are always worth a
    /// line are there from the start, carrying a dash until the first
    /// reading arrives.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.rows.iter().any(|row| row.used_tenths.is_some()) && self.counted.is_empty()
    }

    /// The worst level any window is at.
    #[must_use]
    pub fn level(&self) -> Level {
        self.rows
            .iter()
            .map(|row| row.level)
            .max()
            .unwrap_or_default()
    }
}

/// One window, as every client shows it.
///
/// Percentages are carried in tenths of a point rather than as a fraction:
/// what is drawn is a whole number, and an integer can be compared, sent and
/// stored without the surprises a float brings.
///
/// There is deliberately nothing else here. A row is what the agent last
/// said about one window, and every client draws the same two things from
/// it, so that otag never shows a number the agent would not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    /// Which agent it belongs to.
    pub agent: AgentKind,
    /// Which window it reads.
    pub window: Window,
    /// How much of the window is used, in tenths of a percentage point, or
    /// nothing when otag has never had a reading of it.
    #[serde(default)]
    pub used_tenths: Option<u16>,
    /// When the window resets, in seconds since the epoch, when the agent
    /// said so and the reset is still to come.
    #[serde(default)]
    pub resets_at: Option<i64>,
    /// How worried the user should be.
    #[serde(default)]
    pub level: Level,
}

impl Row {
    /// The percentage as a whole number, which is what is drawn, or nothing
    /// when otag has had no reading of this window.
    #[must_use]
    pub const fn used_pct(&self) -> Option<u16> {
        match self.used_tenths {
            Some(tenths) => Some(tenths / 10),
            None => None,
        }
    }
}

/// What otag counted for an agent that reports no percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counted {
    /// Which agent it belongs to.
    pub agent: AgentKind,
    /// Everything that had to be paid for, cache reads included.
    pub tokens: u64,
}

/// One reading, from what an agent reported about a window.
fn sample(
    agent: AgentKind,
    window: Window,
    limit: LimitWindow,
    at: i64,
    session: Option<&str>,
) -> Sample {
    Sample {
        agent,
        window,
        // An agent that reports more than everything, or less than nothing,
        // is clamped rather than disbelieved: the number is still the best
        // information there is.
        used_pct: limit.used_pct.clamp(0.0, 100.0),
        resets_at: limit.resets_at,
        at,
        session: session.map(str::to_owned),
    }
}

/// A percentage in tenths of a point, rounded and never negative.
fn tenths(used_pct: f32) -> u16 {
    let tenths = (used_pct.clamp(0.0, 100.0) * 10.0).round();
    if tenths.is_finite() { tenths as u16 } else { 0 }
}

/// The current time in seconds since the epoch.
///
/// A clock before the epoch gives zero rather than a panic: every comparison
/// in this module saturates, so a nonsense clock costs a projection, not the
/// daemon.
#[must_use]
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use otag_agents::LimitWindow;

    use super::*;

    /// The row of one window, from a report that now always carries a line
    /// for the windows otag shows before they are read.
    fn row_of(report: &Report, agent: AgentKind, window: Window) -> Row {
        report
            .rows
            .iter()
            .find(|row| row.agent == agent && row.window == window)
            .cloned()
            .expect("the window has a row")
    }

    fn at(agent: AgentKind, used_pct: f32, at: i64) -> Sample {
        Sample {
            agent,
            window: Window::FiveHour,
            used_pct,
            resets_at: None,
            at,
            session: None,
        }
    }

    #[test]
    fn a_level_follows_the_thresholds() {
        assert_eq!(Level::of(0.0), Level::Calm);
        assert_eq!(Level::of(69.9), Level::Calm);
        assert_eq!(Level::of(70.0), Level::Amber);
        assert_eq!(Level::of(89.9), Level::Amber);
        assert_eq!(Level::of(90.0), Level::Red);
        assert_eq!(Level::of(100.0), Level::Red);
    }

    #[test]
    fn the_worst_level_is_the_one_that_matters() {
        assert!(Level::Red > Level::Amber && Level::Amber > Level::Calm);
    }

    #[test]
    fn what_an_agent_reports_becomes_one_sample_per_window() {
        let mut usage = Usage::new();
        usage.record_limits(
            AgentKind::Claude,
            &Limits {
                five_hour: Some(LimitWindow {
                    used_pct: 62.0,
                    resets_at: Some(1_000),
                }),
                weekly: Some(LimitWindow {
                    used_pct: 38.0,
                    resets_at: None,
                }),
            },
            500,
            None,
        );

        assert_eq!(usage.samples().len(), 2);
        assert_eq!(
            usage
                .latest(AgentKind::Claude, Window::FiveHour)
                .map(|sample| sample.used_pct),
            Some(62.0)
        );
        assert_eq!(
            usage
                .latest(AgentKind::Claude, Window::Weekly)
                .map(|sample| sample.resets_at),
            Some(None)
        );
        assert_eq!(usage.latest(AgentKind::Codex, Window::FiveHour), None);
    }

    #[test]
    fn a_window_an_agent_said_nothing_about_is_absent_rather_than_zero() {
        let mut usage = Usage::new();
        usage.record_limits(AgentKind::Codex, &Limits::default(), 10, None);
        assert!(usage.samples().is_empty());
        assert!(usage.report(10).is_empty());
    }

    #[test]
    fn an_impossible_percentage_is_clamped() {
        let mut usage = Usage::new();
        usage.record_limits(
            AgentKind::Claude,
            &Limits {
                five_hour: Some(LimitWindow {
                    used_pct: 140.0,
                    resets_at: None,
                }),
                weekly: Some(LimitWindow {
                    used_pct: -3.0,
                    resets_at: None,
                }),
            },
            0,
            None,
        );
        let report = usage.report(0);
        assert_eq!(report.rows[0].used_pct(), Some(100));
        assert_eq!(report.rows[1].used_pct(), Some(0));
    }

    #[test]
    fn samples_older_than_a_week_are_forgotten() {
        let mut usage = Usage::new();
        usage.record(at(AgentKind::Claude, 10.0, 0));
        usage.record(at(AgentKind::Claude, 20.0, HISTORY + 1));

        assert_eq!(usage.samples().len(), 1, "the old one is gone");
        assert_eq!(usage.samples()[0].used_pct, 20.0);
    }

    #[test]
    fn a_burn_rate_needs_three_samples() {
        let mut usage = Usage::new();
        usage.record(at(AgentKind::Claude, 10.0, 0));
        assert_eq!(
            usage.burn_rate(AgentKind::Claude, Window::FiveHour, 60),
            None
        );

        usage.record(at(AgentKind::Claude, 12.0, 60));
        assert_eq!(
            usage.burn_rate(AgentKind::Claude, Window::FiveHour, 120),
            None,
            "two samples are a guess, not a rate"
        );

        usage.record(at(AgentKind::Claude, 14.0, 120));
        let rate = usage
            .burn_rate(AgentKind::Claude, Window::FiveHour, 120)
            .expect("a rate");
        assert!(
            (rate - 120.0).abs() < 0.5,
            "two points per minute is 120 points per hour, got {rate}"
        );
    }

    #[test]
    fn a_number_that_is_not_climbing_projects_nothing() {
        let mut usage = Usage::new();
        for second in 0..5 {
            usage.record(at(AgentKind::Claude, 40.0, second * 60));
        }
        assert_eq!(
            usage.burn_rate(AgentKind::Claude, Window::FiveHour, 240),
            None
        );
        assert_eq!(
            usage.full_in(AgentKind::Claude, Window::FiveHour, 240),
            None
        );
    }

    #[test]
    fn samples_outside_the_burn_window_are_not_part_of_the_rate() {
        let mut usage = Usage::new();
        // An hour ago the number moved fast; in the last minutes it has not
        // moved at all, and that is what a projection has to follow.
        usage.record(at(AgentKind::Claude, 10.0, 0));
        usage.record(at(AgentKind::Claude, 50.0, 600));
        let now = BURN_WINDOW + 1200;
        usage.record(at(AgentKind::Claude, 60.0, now - 600));
        usage.record(at(AgentKind::Claude, 60.0, now - 300));
        usage.record(at(AgentKind::Claude, 60.0, now));

        assert_eq!(
            usage.burn_rate(AgentKind::Claude, Window::FiveHour, now),
            None
        );
    }

    #[test]
    fn a_projection_says_when_the_window_would_be_full() {
        let mut usage = Usage::new();
        // Ten points in ten minutes: sixty points an hour, forty points left.
        usage.record(at(AgentKind::Claude, 50.0, 0));
        usage.record(at(AgentKind::Claude, 55.0, 300));
        usage.record(at(AgentKind::Claude, 60.0, 600));

        let full_in = usage
            .full_in(AgentKind::Claude, Window::FiveHour, 600)
            .expect("a projection");
        assert!(
            (full_in - 2400).abs() <= 60,
            "forty points at sixty an hour is forty minutes, got {full_in}s"
        );
    }

    #[test]
    fn a_window_that_resets_before_it_fills_is_not_projected() {
        let mut usage = Usage::new();
        for (used_pct, second) in [(50.0, 0), (55.0, 300), (60.0, 600)] {
            usage.record(Sample {
                agent: AgentKind::Claude,
                window: Window::FiveHour,
                used_pct,
                // The window resets in ten minutes; it cannot fill first.
                resets_at: Some(1200),
                at: second,
                session: None,
            });
        }

        assert!(
            usage
                .burn_rate(AgentKind::Claude, Window::FiveHour, 600)
                .is_some(),
            "the number is still climbing"
        );
        assert_eq!(
            usage.full_in(AgentKind::Claude, Window::FiveHour, 600),
            None,
            "the user will never reach the limit this window"
        );
    }

    #[test]
    fn a_window_already_full_is_projected_as_now() {
        let mut usage = Usage::new();
        for (used_pct, second) in [(96.0, 0), (98.0, 300), (100.0, 600)] {
            usage.record(at(AgentKind::Claude, used_pct, second));
        }
        assert_eq!(
            usage.full_in(AgentKind::Claude, Window::FiveHour, 600),
            Some(0)
        );
    }

    #[test]
    fn a_report_carries_the_newest_reading_of_each_window() {
        let mut usage = Usage::new();
        usage.record(at(AgentKind::Claude, 10.0, 0));
        usage.record(at(AgentKind::Claude, 62.4, 600));
        usage.record(Sample {
            agent: AgentKind::Codex,
            window: Window::Weekly,
            used_pct: 91.0,
            resets_at: Some(1800),
            at: 600,
            session: None,
        });

        let report = usage.report(600);
        let claude = row_of(&report, AgentKind::Claude, Window::FiveHour);
        assert_eq!(claude.used_pct(), Some(62));
        assert_eq!(claude.used_tenths, Some(624));
        let codex = row_of(&report, AgentKind::Codex, Window::Weekly);
        assert_eq!(codex.resets_at, Some(1800));
        assert_eq!(report.level(), Level::Red, "the worst window decides");
        assert!(
            !report
                .rows
                .iter()
                .any(|row| row.agent == AgentKind::Codex && row.window == Window::FiveHour),
            "an agent that has never reported a window gets no line for it"
        );
    }

    #[test]
    fn the_windows_claude_reports_have_a_line_before_the_first_reading() {
        let report = Usage::new().report(0);
        assert_eq!(
            report.rows.len(),
            2,
            "Claude Code reports both windows with every reply, so both are \
             worth a line that says otag has not been told yet"
        );
        for window in Window::ALL {
            let row = row_of(&report, AgentKind::Claude, window);
            assert_eq!(row.used_tenths, None, "which clients draw as a dash");
            assert_eq!(row.resets_at, None);
        }
        assert!(report.is_empty(), "but nothing has been reported");
    }

    #[test]
    fn what_otag_counted_is_shown_only_for_an_agent_that_reports_no_percentage() {
        let mut usage = Usage::new();
        usage.record_estimate(
            AgentKind::Codex,
            Estimate {
                tokens: 1_234,
                at: 0,
            },
        );
        usage.record_estimate(AgentKind::Claude, Estimate { tokens: 99, at: 0 });
        usage.record(at(AgentKind::Claude, 10.0, 0));

        let report = usage.report(0);
        assert_eq!(report.counted.len(), 1);
        assert_eq!(report.counted[0].agent, AgentKind::Codex);
        assert_eq!(report.counted[0].tokens, 1_234);
    }

    #[test]
    fn a_newer_count_replaces_the_one_before_it() {
        let mut usage = Usage::new();
        usage.record_estimate(AgentKind::Codex, Estimate { tokens: 10, at: 0 });
        usage.record_estimate(AgentKind::Codex, Estimate { tokens: 20, at: 60 });

        assert_eq!(
            usage.estimate(AgentKind::Codex),
            Some(Estimate { tokens: 20, at: 60 })
        );
    }

    #[test]
    fn samples_read_back_from_the_database_are_put_in_order() {
        let mut usage = Usage::new();
        usage.load(vec![
            at(AgentKind::Claude, 30.0, 600),
            at(AgentKind::Claude, 10.0, 0),
        ]);
        assert_eq!(
            usage
                .latest(AgentKind::Claude, Window::FiveHour)
                .map(|sample| sample.used_pct),
            Some(30.0)
        );
    }

    #[test]
    fn a_report_survives_a_round_trip_through_json() {
        let mut usage = Usage::new();
        usage.record(at(AgentKind::Claude, 62.0, 0));
        let report = usage.report(0);

        let text = serde_json::to_string(&report).expect("a report can be written");
        let read: Report = serde_json::from_str(&text).expect("and read back");
        assert_eq!(read, report);
    }

    #[test]
    fn an_old_report_without_the_newer_fields_still_reads() {
        let report: Report =
            serde_json::from_str(r#"{"rows":[{"agent":"claude","window":"5h_unknown"}]}"#)
                .unwrap_or_default();
        assert!(report.is_empty(), "a row otag cannot read is not invented");
    }

    #[test]
    fn a_reading_whose_window_has_reset_reads_nothing_used() {
        let mut usage = Usage::new();
        // 68% of a window that reset an hour ago.
        usage.record(Sample {
            agent: AgentKind::Claude,
            window: Window::FiveHour,
            used_pct: 68.0,
            resets_at: Some(10_000),
            at: 9_000,
            session: None,
        });

        let report = usage.report(13_600);
        let row = row_of(&report, AgentKind::Claude, Window::FiveHour);
        assert_eq!(
            row.used_pct(),
            Some(0),
            "the allowance is back, and 68% belongs to a window that is over"
        );
        assert_eq!(row.resets_at, None, "the reset it named already happened");
        assert_eq!(row.level, Level::Calm, "a window that reset worries nobody");
        assert_eq!(report.level(), Level::Calm);
    }

    #[test]
    fn a_reading_of_the_window_running_now_stands_however_close_the_reset() {
        let mut usage = Usage::new();
        usage.record(Sample {
            agent: AgentKind::Claude,
            window: Window::FiveHour,
            used_pct: 68.0,
            resets_at: Some(10_000),
            at: 9_000,
            session: None,
        });

        let report = usage.report(9_999);
        let row = row_of(&report, AgentKind::Claude, Window::FiveHour);
        assert_eq!(row.used_pct(), Some(68));
        assert_eq!(row.resets_at, Some(10_000));
    }

    #[test]
    fn a_fresh_reading_replaces_one_whose_window_had_reset() {
        let mut usage = Usage::new();
        usage.record(Sample {
            agent: AgentKind::Claude,
            window: Window::FiveHour,
            used_pct: 68.0,
            resets_at: Some(10_000),
            at: 9_000,
            session: None,
        });
        // The new window, reported four minutes ago.
        usage.record(Sample {
            agent: AgentKind::Claude,
            window: Window::FiveHour,
            used_pct: 4.0,
            resets_at: Some(28_000),
            at: 13_360,
            session: None,
        });

        let report = usage.report(13_600);
        let row = row_of(&report, AgentKind::Claude, Window::FiveHour);
        assert_eq!(row.used_pct(), Some(4), "the new window's own number");
        assert_eq!(row.resets_at, Some(28_000));
    }

    #[test]
    fn a_window_with_no_reset_time_is_reported_as_it_was_read() {
        let mut usage = Usage::new();
        usage.record(at(AgentKind::Claude, 50.0, 0));

        let report = usage.report(HISTORY);
        let row = row_of(&report, AgentKind::Claude, Window::FiveHour);
        assert_eq!(
            row.used_pct(),
            Some(50),
            "otag was never told when this window resets, so it cannot claim it did"
        );
        assert_eq!(row.resets_at, None);
    }

    /// A reading of one window, as one session would report it.
    fn reading(used_pct: f32, resets_at: i64, at: i64, session: &str) -> Sample {
        Sample {
            agent: AgentKind::Claude,
            window: Window::FiveHour,
            used_pct,
            resets_at: Some(resets_at),
            at,
            session: Some(session.to_owned()),
        }
    }

    #[test]
    fn an_idle_session_repeating_an_old_number_does_not_undo_a_fresher_one() {
        let mut usage = Usage::new();
        // The session that has been working knows the window is at 25%.
        usage.record(reading(25.0, 20_000, 1_000, "busy"));
        // A session that has been idle since the window was at 8% reports
        // that, later, because its status line runs on a timer.
        usage.record(reading(8.0, 20_000, 2_000, "idle"));

        let latest = usage
            .latest(AgentKind::Claude, Window::FiveHour)
            .expect("a reading");
        assert_eq!(
            latest.used_pct, 25.0,
            "a window's usage only climbs, so the highest reading of it is the current one"
        );
        assert_eq!(latest.session.as_deref(), Some("busy"));
    }

    #[test]
    fn a_session_reporting_a_window_that_has_ended_never_replaces_the_one_running() {
        let mut usage = Usage::new();
        // The window running now, read at 69% by the session doing the work.
        usage.record(reading(69.0, 20_000, 10_000, "working"));
        // A session resumed from last week re-runs its status line on a
        // timer and reports the snapshot it was handed then: 85% of a window
        // that ended days ago. It arrives later, and it is still older news.
        usage.record(reading(85.0, 20_000 - 11 * 24 * 60 * 60, 10_100, "resumed"));

        let latest = usage
            .latest(AgentKind::Claude, Window::FiveHour)
            .expect("a reading");
        assert_eq!(
            latest.used_pct, 69.0,
            "a reading of a window that is over says nothing about the one running"
        );
        assert_eq!(latest.session.as_deref(), Some("working"));

        let row = row_of(&usage.report(10_100), AgentKind::Claude, Window::FiveHour);
        assert_eq!(row.used_pct(), Some(69), "so the report still reads 69%");
        assert_eq!(row.resets_at, Some(20_000));
    }

    #[test]
    fn a_climbing_number_from_any_session_is_taken_at_once() {
        let mut usage = Usage::new();
        usage.record(reading(25.0, 20_000, 1_000, "one"));
        // Another session, otag may not even be running it, reports more.
        usage.record(reading(27.0, 20_000, 1_100, "two"));

        let latest = usage
            .latest(AgentKind::Claude, Window::FiveHour)
            .expect("a reading");
        assert_eq!(latest.used_pct, 27.0);
        assert_eq!(latest.session.as_deref(), Some("two"));
    }

    #[test]
    fn repeating_the_same_number_keeps_the_newest_arrival() {
        let mut usage = Usage::new();
        usage.record(reading(25.0, 20_000, 1_000, "one"));
        usage.record(reading(25.0, 20_000, 4_000, "two"));

        let latest = usage
            .latest(AgentKind::Claude, Window::FiveHour)
            .expect("a reading");
        assert_eq!(latest.used_pct, 25.0);
        assert_eq!(
            latest.at, 4_000,
            "the newest confirmation dates the reading"
        );
        assert_eq!(latest.session.as_deref(), Some("two"));
    }

    #[test]
    fn a_reading_of_the_new_window_beats_a_higher_one_of_the_old() {
        let mut usage = Usage::new();
        usage.record(reading(92.0, 20_000, 19_000, "before"));
        // The window reset, and the first reading of the new one is low.
        usage.record(reading(3.0, 38_000, 20_100, "after"));

        let report = usage.report(20_100);
        let row = row_of(&report, AgentKind::Claude, Window::FiveHour);
        assert_eq!(
            row.used_pct(),
            Some(3),
            "a later window always wins, however high the one before it got"
        );
        assert_eq!(row.resets_at, Some(38_000));
    }

    #[test]
    fn readings_with_no_reset_time_fall_back_to_the_newest_arrival() {
        let mut usage = Usage::new();
        usage.record(at(AgentKind::Claude, 40.0, 1_000));
        usage.record(at(AgentKind::Claude, 20.0, 2_000));

        assert_eq!(
            usage.report(2_000).rows[0].used_pct(),
            Some(20),
            "nothing says which window these belong to, so the newest is all otag has"
        );
    }

    #[test]
    fn what_an_agent_reports_remembers_which_session_said_it() {
        let mut usage = Usage::new();
        usage.record_limits(
            AgentKind::Claude,
            &Limits {
                five_hour: Some(LimitWindow {
                    used_pct: 25.0,
                    resets_at: Some(20_000),
                }),
                weekly: None,
            },
            1_000,
            Some("a9f87cc5-726a"),
        );

        assert_eq!(
            usage
                .latest(AgentKind::Claude, Window::FiveHour)
                .and_then(|sample| sample.session)
                .as_deref(),
            Some("a9f87cc5-726a"),
            "usage is account-wide, so which session said so is worth keeping"
        );
    }

    /// One reading of the weekly window.
    fn weekly(used_pct: f32, resets_at: Option<i64>, at: i64) -> Sample {
        Sample {
            agent: AgentKind::Claude,
            window: Window::Weekly,
            used_pct,
            resets_at,
            at,
            session: None,
        }
    }

    #[test]
    fn a_burn_window_is_proportional_to_the_window_it_measures() {
        assert_eq!(Window::FiveHour.burn_window(), BURN_WINDOW);
        assert_eq!(Window::Weekly.burn_window(), WEEKLY_BURN_WINDOW);
        for window in Window::ALL {
            let share = window.length() / window.burn_window();
            assert!(
                (4..=30).contains(&share),
                "a rate is measured over a fraction of its own window, not a \
                 thousandth of it and not the whole of it: {window:?} is 1/{share}"
            );
        }
    }

    #[test]
    fn a_busy_twenty_minutes_does_not_decide_how_the_week_is_going() {
        let mut usage = Usage::new();
        let resets_at = 6 * 24 * 60 * 60;
        let now = 2 * 24 * 60 * 60;

        // Two days of steady work, a bit over a point an hour, and then a
        // busy twenty minutes: nearly four points in it. Measured over those
        // twenty minutes alone the week reads as eleven points an hour and
        // gone by this evening, which is the number this guards against.
        for hour in 0..48 {
            usage.record(weekly(hour as f32 * 1.25, Some(resets_at), hour * 3_600));
        }
        usage.record(weekly(56.25, Some(resets_at), now - 1_200));
        usage.record(weekly(58.10, Some(resets_at), now - 600));
        usage.record(weekly(60.00, Some(resets_at), now));

        let per_hour = usage
            .burn_rate(AgentKind::Claude, Window::Weekly, now)
            .expect("a rate");
        assert!(
            per_hour < 3.0,
            "a day of readings is a pace, not a burst: {per_hour} points an hour"
        );

        let full_in = usage
            .full_in(AgentKind::Claude, Window::Weekly, now)
            .expect("a projection, because it lands before the reset");
        assert!(
            full_in > 24 * 60 * 60,
            "forty points at that pace is more than a day away, not 4h38m: {full_in}s"
        );
        assert!(
            full_in < resets_at - now,
            "and it is only shown because it lands before the reset: {full_in}s"
        );
    }

    #[test]
    fn the_five_hour_window_still_follows_the_last_twenty_minutes() {
        let mut usage = Usage::new();
        let resets_at = 4 * 60 * 60;
        // An hour ago it was crawling; in the last twenty minutes it is not.
        for minute in 0..60 {
            usage.record(Sample {
                agent: AgentKind::Claude,
                window: Window::FiveHour,
                used_pct: minute as f32 * 0.01,
                resets_at: Some(resets_at),
                at: minute * 60,
                session: None,
            });
        }
        let now = 2 * 60 * 60;
        for (used_pct, at) in [(20.0, now - 1_200), (30.0, now - 600), (40.0, now)] {
            usage.record(Sample {
                agent: AgentKind::Claude,
                window: Window::FiveHour,
                used_pct,
                resets_at: Some(resets_at),
                at,
                session: None,
            });
        }

        let per_hour = usage
            .burn_rate(AgentKind::Claude, Window::FiveHour, now)
            .expect("a rate");
        assert!(
            per_hour > 50.0,
            "the five-hour window follows the sitting it is in: {per_hour}"
        );
    }

    #[test]
    fn a_rate_never_reaches_back_into_the_window_before_it() {
        let mut usage = Usage::new();
        // The weekly window reset two hours ago, so a day of readings would
        // otherwise include the last window's 90% and read as falling.
        let resets_at = 7 * 24 * 60 * 60 + 2 * 60 * 60;
        let started = resets_at - Window::Weekly.length();
        for hour in 0..10 {
            usage.record(weekly(
                80.0 + hour as f32,
                None,
                started - (10 - hour) * 3_600,
            ));
        }
        let now = started + 2 * 60 * 60;
        for (used_pct, at) in [(1.0, started + 600), (2.0, started + 3_600), (3.0, now)] {
            usage.record(weekly(used_pct, Some(resets_at), at));
        }

        let per_hour = usage
            .burn_rate(AgentKind::Claude, Window::Weekly, now)
            .expect("a rate for the window that is running");
        assert!(
            (0.5..3.0).contains(&per_hour),
            "only this window's own readings count: {per_hour} points an hour"
        );
    }

    #[test]
    fn a_weekly_window_with_no_reset_time_is_not_projected() {
        let mut usage = Usage::new();
        for (used_pct, at) in [(10.0, 0), (20.0, 12 * 3_600), (30.0, 24 * 3_600)] {
            usage.record(weekly(used_pct, None, at));
        }
        let now = 24 * 3_600;

        assert!(
            usage
                .burn_rate(AgentKind::Claude, Window::Weekly, now)
                .is_some(),
            "the number is climbing"
        );
        assert_eq!(
            usage.full_in(AgentKind::Claude, Window::Weekly, now),
            None,
            "days ahead with no reset to land before is a guess, not a forecast"
        );
    }

    #[test]
    fn a_weekly_projection_past_the_reset_is_not_shown() {
        let mut usage = Usage::new();
        // Climbing slowly, with the week nearly over: it will reset first.
        let resets_at = 3 * 60 * 60;
        for (used_pct, at) in [(40.0, 0), (41.0, 3_600), (42.0, 2 * 3_600)] {
            usage.record(weekly(used_pct, Some(resets_at), at));
        }

        assert_eq!(
            usage.full_in(AgentKind::Claude, Window::Weekly, 2 * 3_600),
            None,
            "the user will never reach the limit this week"
        );
    }

    #[test]
    fn the_clock_is_a_number_of_seconds() {
        assert!(now() > 1_700_000_000, "otag is not running in the past");
    }
}
