//! How much of each agent's limits is gone, and how fast.
//!
//! An agent tells otag how much of its windows it has used as a percentage,
//! every time it answers. otag keeps those samples, and from the last few
//! minutes of them works out how fast the percentage is climbing and when it
//! would reach a hundred. That projection is the whole point of the widget:
//! a number that is already at 90% is too late to be useful.
//!
//! Two rules run through this module. Everything an agent says is optional,
//! so a window otag has never been told about is absent rather than zero.
//! And nothing is invented: when otag has no percentage, it says so and
//! reports the tokens it counted instead, marked as an estimate.

use std::time::{SystemTime, UNIX_EPOCH};

use otag_agents::{AgentKind, LimitWindow, Limits};
use serde::{Deserialize, Serialize};

/// How long samples are kept.
pub const HISTORY: i64 = 7 * 24 * 60 * 60;

/// How far back the burn rate is measured.
pub const BURN_WINDOW: i64 = 20 * 60;

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

    /// What the widget calls it, in as few columns as possible.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::FiveHour => "5h",
            Self::Weekly => "7d",
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
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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
        self.samples.push(sample);
        let cutoff = sample.at.saturating_sub(HISTORY);
        self.samples.retain(|kept| kept.at >= cutoff);
    }

    /// Keep everything an agent reported in one go.
    pub fn record_limits(&mut self, agent: AgentKind, limits: &Limits, at: i64) {
        if let Some(window) = limits.five_hour {
            self.record(sample(agent, Window::FiveHour, window, at));
        }
        if let Some(window) = limits.weekly {
            self.record(sample(agent, Window::Weekly, window, at));
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

    /// The newest reading of one window.
    #[must_use]
    pub fn latest(&self, agent: AgentKind, window: Window) -> Option<Sample> {
        self.samples
            .iter()
            .rev()
            .find(|sample| sample.agent == agent && sample.window == window)
            .copied()
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
    /// A least-squares slope over the samples of the last twenty minutes, so
    /// that one turn that cost a lot does not become a prediction. Nothing is
    /// reported until there are three samples spread over real time, or when
    /// the number is not climbing.
    #[must_use]
    pub fn burn_rate(&self, agent: AgentKind, window: Window, now: i64) -> Option<f32> {
        let cutoff = now.saturating_sub(BURN_WINDOW);
        let recent: Vec<Sample> = self
            .samples
            .iter()
            .filter(|sample| {
                sample.agent == agent && sample.window == window && sample.at >= cutoff
            })
            .copied()
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
    /// not a limit the user will hit.
    #[must_use]
    pub fn full_in(&self, agent: AgentKind, window: Window, now: i64) -> Option<i64> {
        let latest = self.latest(agent, window)?;
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
            _ => Some(seconds),
        }
    }

    /// Everything the widget and `otag usage` show.
    #[must_use]
    pub fn report(&self, now: i64) -> Report {
        let mut rows = Vec::new();
        for agent in AgentKind::ALL {
            for window in Window::ALL {
                if let Some(sample) = self.latest(agent, window) {
                    rows.push(Row {
                        agent,
                        window,
                        used_tenths: tenths(sample.used_pct),
                        resets_in: sample
                            .resets_at
                            .map(|resets_at| resets_at.saturating_sub(now)),
                        full_in: self.full_in(agent, window, now),
                        level: Level::of(sample.used_pct),
                        at: sample.at,
                    });
                }
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
    /// Whether there is nothing to show.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.counted.is_empty()
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

/// One window, as the widget shows it.
///
/// Percentages are carried in tenths of a point rather than as a fraction:
/// what is drawn is a whole number, and an integer can be compared, sent and
/// stored without the surprises a float brings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    /// Which agent it belongs to.
    pub agent: AgentKind,
    /// Which window it reads.
    pub window: Window,
    /// How much of the window is used, in tenths of a percentage point.
    pub used_tenths: u16,
    /// How long until the window resets, in seconds.
    #[serde(default)]
    pub resets_in: Option<i64>,
    /// How long until the window is used up, in seconds.
    #[serde(default)]
    pub full_in: Option<i64>,
    /// How worried the user should be.
    #[serde(default)]
    pub level: Level,
    /// When the reading was taken, in seconds since the epoch.
    #[serde(default)]
    pub at: i64,
}

impl Row {
    /// The percentage as a whole number, which is what is drawn.
    #[must_use]
    pub const fn used_pct(&self) -> u16 {
        self.used_tenths / 10
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
fn sample(agent: AgentKind, window: Window, limit: LimitWindow, at: i64) -> Sample {
    Sample {
        agent,
        window,
        // An agent that reports more than everything, or less than nothing,
        // is clamped rather than disbelieved: the number is still the best
        // information there is.
        used_pct: limit.used_pct.clamp(0.0, 100.0),
        resets_at: limit.resets_at,
        at,
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

    fn at(agent: AgentKind, used_pct: f32, at: i64) -> Sample {
        Sample {
            agent,
            window: Window::FiveHour,
            used_pct,
            resets_at: None,
            at,
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
        usage.record_limits(AgentKind::Codex, &Limits::default(), 10);
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
        );
        let report = usage.report(0);
        assert_eq!(report.rows[0].used_pct(), 100);
        assert_eq!(report.rows[1].used_pct(), 0);
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
        });

        let report = usage.report(600);
        assert_eq!(report.rows.len(), 2);
        assert_eq!(report.rows[0].agent, AgentKind::Claude);
        assert_eq!(report.rows[0].used_pct(), 62);
        assert_eq!(report.rows[0].used_tenths, 624);
        assert_eq!(report.rows[1].agent, AgentKind::Codex);
        assert_eq!(report.rows[1].resets_in, Some(1200));
        assert_eq!(report.level(), Level::Red, "the worst window decides");
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
    fn the_clock_is_a_number_of_seconds() {
        assert!(now() > 1_700_000_000, "otag is not running in the past");
    }
}
