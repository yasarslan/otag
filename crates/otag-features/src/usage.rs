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

/// How old a reading may be before it is shown as old, in seconds.
///
/// An agent reports its limits every time it answers, so a reading from ten
/// minutes ago means nobody has been working, not that the number moved. It
/// is still the best information there is, but it is shown as what it is.
pub const FRESH: i64 = 10 * 60;

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
    /// Readings arrive from every session on the machine, and a session that
    /// has been idle for an hour still reports the percentage it was last
    /// told — so "the one that arrived last" is not "the one that knows
    /// most". What a window has used only climbs until it resets, so within
    /// one window the highest reading is the most current one, and the
    /// newest confirmation of that height carries the freshest time.
    ///
    /// Windows are told apart by when they reset: a reading of a later
    /// window always wins over one of an earlier window, however high the
    /// earlier one got. Readings with no reset time cannot be grouped that
    /// way, so there the newest arrival is all otag has to go on.
    #[must_use]
    pub fn latest(&self, agent: AgentKind, window: Window) -> Option<Sample> {
        let mut current: Option<&Sample> = None;
        for sample in self
            .samples
            .iter()
            .filter(|sample| sample.agent == agent && sample.window == window)
        {
            current = Some(match current {
                // Two readings of the same window, told apart by both
                // naming the same reset: the higher one is the later state
                // of that window, and a reading that merely repeats it
                // keeps the window's freshness because it arrived after.
                Some(kept)
                    if kept.resets_at.is_some()
                        && kept.resets_at == sample.resets_at
                        && kept.used_pct > sample.used_pct =>
                {
                    kept
                }
                // Anything else: a reading of a window that resets at some
                // other time is a reading of another window, and a reading
                // that does not say when it resets cannot be compared with
                // one that does. Either way the newer arrival is the answer.
                _ => sample,
            });
        }
        current.cloned()
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
    #[must_use]
    pub fn report(&self, now: i64) -> Report {
        let mut rows = Vec::new();
        for agent in AgentKind::ALL {
            for window in Window::ALL {
                if let Some(sample) = self.latest(agent, window) {
                    // A reading of a window that is over says nothing about
                    // the one running now: it is reported as a window
                    // waiting for its first reading, not as a percentage.
                    let reset = sample.has_reset(now);
                    rows.push(Row {
                        agent,
                        window,
                        used_tenths: (!reset).then(|| tenths(sample.used_pct)),
                        resets_in: if reset {
                            None
                        } else {
                            sample
                                .resets_at
                                .map(|resets_at| resets_at.saturating_sub(now))
                        },
                        full_in: self.full_in(agent, window, now),
                        level: if reset {
                            Level::Calm
                        } else {
                            Level::of(sample.used_pct)
                        },
                        at: sample.at,
                        age: now.saturating_sub(sample.at).max(0),
                        reset,
                        session: sample.session.clone(),
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    /// Which agent it belongs to.
    pub agent: AgentKind,
    /// Which window it reads.
    pub window: Window,
    /// How much of the window is used, in tenths of a percentage point, or
    /// nothing when the window has reset and no reading of the new one has
    /// arrived yet.
    #[serde(default)]
    pub used_tenths: Option<u16>,
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
    /// How old the reading is, in seconds.
    #[serde(default)]
    pub age: i64,
    /// Whether the window has reset since the reading was taken, so that
    /// otag is waiting for fresh data rather than showing a percentage.
    #[serde(default)]
    pub reset: bool,
    /// The agent session the reading came from, when it said which.
    #[serde(default)]
    pub session: Option<String>,
}

impl Row {
    /// The percentage as a whole number, which is what is drawn, or nothing
    /// when there is no current reading to draw.
    #[must_use]
    pub const fn used_pct(&self) -> Option<u16> {
        match self.used_tenths {
            Some(tenths) => Some(tenths / 10),
            None => None,
        }
    }

    /// Whether the reading is old enough that it should be shown as old.
    #[must_use]
    pub const fn is_stale(&self) -> bool {
        self.age > FRESH
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
        assert_eq!(report.rows.len(), 2);
        assert_eq!(report.rows[0].agent, AgentKind::Claude);
        assert_eq!(report.rows[0].used_pct(), Some(62));
        assert_eq!(report.rows[0].used_tenths, Some(624));
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
    fn a_reading_whose_window_has_reset_is_no_longer_a_percentage() {
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
        assert_eq!(report.rows.len(), 1, "the window is still worth a line");
        let row = &report.rows[0];
        assert!(row.reset, "otag is waiting for a reading of the new window");
        assert_eq!(row.used_pct(), None, "68% belongs to a window that is over");
        assert_eq!(row.full_in, None, "there is nothing to project from");
        assert_eq!(row.resets_in, None, "the reset already happened");
        assert_eq!(row.level, Level::Calm, "a window that reset worries nobody");
        assert_eq!(row.age, 4_600);
        assert!(row.is_stale());
        assert_eq!(report.level(), Level::Calm);
    }

    #[test]
    fn a_reading_of_the_window_running_now_is_current_however_close_the_reset() {
        let mut usage = Usage::new();
        usage.record(Sample {
            agent: AgentKind::Claude,
            window: Window::FiveHour,
            used_pct: 68.0,
            resets_at: Some(10_000),
            at: 9_000,
            session: None,
        });

        let row = &usage.report(9_999).rows[0];
        assert!(!row.reset, "the window has not reset yet");
        assert_eq!(row.used_pct(), Some(68));
        assert_eq!(row.resets_in, Some(1));
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

        let row = &usage.report(13_600).rows[0];
        assert!(!row.reset);
        assert_eq!(row.used_pct(), Some(4), "the new window's own number");
        assert_eq!(row.resets_in, Some(14_400));
        assert_eq!(row.age, 240);
        assert!(!row.is_stale(), "four minutes old is current");
    }

    #[test]
    fn a_window_with_no_reset_time_is_reported_as_it_was_read() {
        let mut usage = Usage::new();
        usage.record(at(AgentKind::Claude, 50.0, 0));

        let row = &usage.report(HISTORY).rows[0];
        assert!(
            !row.reset,
            "otag was never told when this window resets, so it cannot claim it did"
        );
        assert_eq!(row.used_pct(), Some(50));
        assert!(
            row.is_stale(),
            "but it is honest about how old the number is"
        );
    }

    #[test]
    fn a_reading_is_old_only_past_the_freshness_window() {
        let mut usage = Usage::new();
        usage.record(at(AgentKind::Claude, 50.0, 0));

        assert!(!usage.report(FRESH).rows[0].is_stale());
        assert!(usage.report(FRESH + 1).rows[0].is_stale());
    }

    #[test]
    fn a_clock_that_went_backwards_does_not_make_a_reading_younger_than_new() {
        let mut usage = Usage::new();
        usage.record(at(AgentKind::Claude, 50.0, 1_000));
        let row = &usage.report(500).rows[0];
        assert_eq!(row.age, 0, "an age is never negative");
        assert!(!row.is_stale());
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

        let row = &usage.report(2_000).rows[0];
        assert_eq!(
            row.used_pct(),
            Some(25),
            "a window's usage only climbs, so the highest reading of it is the current one"
        );
        assert_eq!(row.session.as_deref(), Some("busy"));
    }

    #[test]
    fn a_climbing_number_from_any_session_is_taken_at_once() {
        let mut usage = Usage::new();
        usage.record(reading(25.0, 20_000, 1_000, "one"));
        // Another session, otag may not even be running it, reports more.
        usage.record(reading(27.0, 20_000, 1_100, "two"));

        let row = &usage.report(1_100).rows[0];
        assert_eq!(row.used_pct(), Some(27));
        assert_eq!(row.session.as_deref(), Some("two"));
        assert_eq!(row.age, 0, "and it is as fresh as the reading that said so");
    }

    #[test]
    fn repeating_the_same_number_keeps_the_window_fresh() {
        let mut usage = Usage::new();
        usage.record(reading(25.0, 20_000, 1_000, "one"));
        usage.record(reading(25.0, 20_000, 4_000, "two"));

        let row = &usage.report(4_000).rows[0];
        assert_eq!(row.used_pct(), Some(25));
        assert_eq!(row.age, 0, "the newest confirmation dates the reading");
        assert_eq!(row.session.as_deref(), Some("two"));
    }

    #[test]
    fn a_reading_of_the_new_window_beats_a_higher_one_of_the_old() {
        let mut usage = Usage::new();
        usage.record(reading(92.0, 20_000, 19_000, "before"));
        // The window reset, and the first reading of the new one is low.
        usage.record(reading(3.0, 38_000, 20_100, "after"));

        let row = &usage.report(20_100).rows[0];
        assert_eq!(
            row.used_pct(),
            Some(3),
            "a later window always wins, however high the one before it got"
        );
        assert_eq!(row.resets_in, Some(17_900));
        assert_eq!(row.session.as_deref(), Some("after"));
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
            usage.report(1_000).rows[0].session.as_deref(),
            Some("a9f87cc5-726a")
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
