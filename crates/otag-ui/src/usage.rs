//! The usage widget: how much of each agent's limits is gone.
//!
//! It lives at the bottom of the sidebar, which is sixteen columns wide, so
//! every line here is written to fit in very little space and to say the one
//! thing that matters: what the agent last reported, and when that window
//! resets.
//!
//! The widget is calm by design. There is no blinking and no bar that fills
//! up; a number that needs the user turns amber and then red, and that is
//! all the emphasis there is. Nothing here is a projection, an age or a
//! hint — a reading stands on its own, and a line of explanation beside it
//! only makes a person wonder which of the two to believe.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use otag_features::usage::{Level, Report, Row};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

use crate::theme::Theme;

/// What the widget says when an agent has nothing left.
///
/// The offer is the point: a user who has just been told they are out of
/// quota wants to know what to do about it, and `Ctrl-b g` hands the task to
/// the other agent.
pub const OFFER: &str = "out · ^b g hands over";

/// What a window with no reading yet shows instead of a percentage.
pub const NO_READING: &str = "–";

/// How far ahead a reset is still told by the clock alone, in seconds.
///
/// A five-hour window always resets today or tonight, and `02:40` is how a
/// person thinks of that. A weekly window resets days away, where the clock
/// alone would be a riddle, so it gets the date too.
const CLOCK_ONLY: i64 = 24 * 60 * 60;

/// How many rows the widget needs for a report.
///
/// One per window, one more for each window that says when it resets, one
/// per agent that reports no percentage at all, and one for the offer when
/// a window is used up.
#[must_use]
pub fn height(report: &Report) -> u16 {
    let rows: usize = report
        .rows
        .iter()
        .map(|row| 1 + usize::from(row.resets_at.is_some()))
        .sum();
    let offer = usize::from(offers_handoff(report));
    u16::try_from(
        rows.saturating_add(report.counted.len())
            .saturating_add(offer),
    )
    .unwrap_or(u16::MAX)
}

/// Whether any window is used up, so that handing the task over is the only
/// thing left to do.
#[must_use]
pub fn offers_handoff(report: &Report) -> bool {
    report
        .rows
        .iter()
        .any(|row| row.used_pct().is_some_and(|used_pct| used_pct >= 100))
}

/// When a window resets, as a person reads a clock: `02:40` tonight, and
/// `10 Nov 06:00` for a week away.
///
/// In the machine's own time zone, because that is the clock the user is
/// looking at. A time zone the system cannot tell us leaves the reset in
/// UTC rather than nothing: a wrong hour is obvious and a missing line is
/// not.
#[must_use]
pub fn reset_clock(resets_at: i64, now: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    let Ok(stamp) = Timestamp::from_second(resets_at) else {
        return String::new();
    };
    let when = TimeZone::system().to_datetime(stamp);
    let clock = format!("{:02}:{:02}", when.hour(), when.minute());
    if resets_at.saturating_sub(now) < CLOCK_ONLY {
        return clock;
    }

    let month = MONTHS
        .get(usize::from(when.month().unsigned_abs()).saturating_sub(1))
        .unwrap_or(&"");
    format!("{} {month} {clock}", when.day())
}

/// A token count short enough for a sidebar: `840`, `12k`, `1.2M`.
#[must_use]
pub fn tokens(count: u64) -> String {
    const THOUSAND: u64 = 1_000;
    const MILLION: u64 = 1_000_000;

    if count >= MILLION {
        let millions = count as f64 / MILLION as f64;
        if millions < 10.0 {
            return format!("{millions:.1}M");
        }
        return format!("{}M", millions.round() as u64);
    }
    if count >= THOUSAND {
        return format!("{}k", count / THOUSAND);
    }
    count.to_string()
}

/// The first line of a window: who, which window, how much is gone.
///
/// A window otag has never had a reading of shows a dash. A window whose
/// reset has passed shows nothing left used, because the allowance is back;
/// what it must never show is the percentage of the window before it.
#[must_use]
pub fn headline(row: &Row) -> String {
    let used = match row.used_pct() {
        Some(used_pct) => format!("{used_pct}%"),
        None => NO_READING.to_owned(),
    };
    format!("{} {} {used:>4}", row.agent.label(), row.window.label())
}

/// The second line of a window: when it resets, if the agent said so.
#[must_use]
pub fn detail(row: &Row, now: i64) -> Option<String> {
    row.resets_at
        .map(|resets_at| format!("resets {}", reset_clock(resets_at, now)))
}

/// What otag counted for an agent that reports no percentage of its own.
///
/// Marked with a tilde, and in tokens rather than as a percentage: nobody
/// knows what the account's allowance is, and a number that looks like a
/// percentage would be a lie.
#[must_use]
pub fn counted_line(agent: otag_agents::AgentKind, count: u64) -> String {
    format!("{} ~{}", agent.label(), tokens(count))
}

/// The usage widget.
#[derive(Debug, Clone, Copy)]
pub struct Usage<'a> {
    /// What to show.
    pub report: &'a Report,
    /// Colours to draw it in.
    pub theme: Theme,
    /// Now, in seconds since the epoch, which is what a reset is told from.
    pub now: i64,
}

impl Widget for Usage<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        let mut y = area.y;
        let bottom = area.y.saturating_add(area.height);
        let mut put = |text: &str, style: Style, y: &mut u16| {
            if *y >= bottom {
                return;
            }
            buffer.set_stringn(area.x, *y, text, area.width as usize, style);
            *y = y.saturating_add(1);
        };

        for row in &self.report.rows {
            put(&headline(row), self.row_style(row), &mut y);
            if let Some(detail) = detail(row, self.now) {
                // Indented and dim: it is the explanation, not the number.
                put(&format!(" {detail}"), self.theme.dim(), &mut y);
            }
        }
        for counted in &self.report.counted {
            put(
                &counted_line(counted.agent, counted.tokens),
                self.theme.dim(),
                &mut y,
            );
        }
        if offers_handoff(self.report) {
            put(OFFER, Style::new().fg(self.theme.error), &mut y);
        }
    }
}

impl Usage<'_> {
    /// How a window is drawn.
    ///
    /// By what it says, and by nothing else: a reading is never dimmed for
    /// its age. Either otag has a number, and it is the agent's own number
    /// drawn in the colour that number deserves, or it has none and draws a
    /// dash.
    fn row_style(&self, row: &Row) -> Style {
        match row.used_tenths {
            Some(_) => self.style(row.level),
            None => self.theme.dim(),
        }
    }

    /// How a level is drawn.
    fn style(&self, level: Level) -> Style {
        match level {
            Level::Calm => self.theme.plain(),
            Level::Amber => Style::new().fg(self.theme.attention),
            Level::Red => Style::new().fg(self.theme.error),
        }
    }
}

#[cfg(test)]
mod tests {
    use otag_agents::AgentKind;
    use otag_features::usage::{Counted, Window};

    use super::*;

    /// A moment in a known time zone: 2 November 2026, 23:35 UTC.
    const NOW: i64 = 1_793_741_700;

    fn row(used_tenths: u16, resets_at: Option<i64>) -> Row {
        Row {
            agent: AgentKind::Claude,
            window: Window::FiveHour,
            used_tenths: Some(used_tenths),
            resets_at,
            level: Level::of(f32::from(used_tenths) / 10.0),
        }
    }

    /// A window otag has never had a reading of.
    fn unread(window: Window) -> Row {
        Row {
            agent: AgentKind::Claude,
            window,
            used_tenths: None,
            resets_at: None,
            level: Level::Calm,
        }
    }

    fn render(report: &Report, width: u16, height: u16) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        Usage {
            report,
            theme: Theme::CALM_DARK,
            now: NOW,
        }
        .render(area, &mut buffer);

        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn a_token_count_is_shortened_but_never_rounded_to_nothing() {
        assert_eq!(tokens(0), "0");
        assert_eq!(tokens(840), "840");
        assert_eq!(tokens(12_400), "12k");
        assert_eq!(tokens(1_250_000), "1.2M");
        assert_eq!(tokens(42_000_000), "42M");
    }

    #[test]
    fn a_reset_within_the_day_is_a_clock_and_nothing_else() {
        // Three hours ahead of NOW, whatever the machine's time zone is:
        // the hour is the machine's, the shape of the answer is not.
        let clock = reset_clock(NOW + 3 * 60 * 60, NOW);
        assert_eq!(clock.len(), 5, "`02:40`, and no date: {clock}");
        assert_eq!(&clock[2..3], ":", "{clock}");
    }

    #[test]
    fn a_reset_days_away_carries_its_date() {
        let clock = reset_clock(NOW + 7 * 24 * 60 * 60, NOW);
        assert!(
            clock.len() > 8 && clock.contains(' '),
            "`10 Nov 06:00`, because a clock alone would be a riddle: {clock}"
        );
    }

    #[test]
    fn a_reset_at_a_time_no_clock_can_hold_costs_a_line_and_not_the_client() {
        assert_eq!(reset_clock(i64::MAX, NOW), "");
    }

    #[test]
    fn a_window_is_its_percentage_and_when_it_resets() {
        let report = Report {
            rows: vec![row(690, Some(NOW + 3 * 60 * 60))],
            counted: Vec::new(),
        };

        assert_eq!(height(&report), 2);
        let lines = render(&report, 16, 2);
        assert_eq!(lines[0], "Claude 5h  69%");
        assert_eq!(
            lines[1],
            format!(" resets {}", reset_clock(NOW + 3 * 60 * 60, NOW)),
            "the reset is a time of day, not a countdown"
        );
    }

    #[test]
    fn a_window_that_never_reported_its_reset_is_one_line() {
        let report = Report {
            rows: vec![row(380, None)],
            counted: Vec::new(),
        };
        assert_eq!(height(&report), 1);
        assert_eq!(render(&report, 16, 2)[0], "Claude 5h  38%");
        assert_eq!(render(&report, 16, 2)[1], "");
    }

    #[test]
    fn a_window_with_no_reading_at_all_is_a_dash() {
        let report = Report {
            rows: vec![unread(Window::FiveHour), unread(Window::Weekly)],
            counted: Vec::new(),
        };

        assert_eq!(height(&report), 2);
        let lines = render(&report, 16, 2);
        assert_eq!(
            lines[0], "Claude 5h    –",
            "a dash says otag has not been told, which no percentage can say"
        );
        assert_eq!(lines[1], "Claude week    –");
        assert!(
            !offers_handoff(&report),
            "a window with no number is not a window that is used up"
        );
    }

    #[test]
    fn a_window_that_has_reset_reads_nothing_used() {
        let report = Report {
            rows: vec![row(0, None)],
            counted: Vec::new(),
        };
        assert_eq!(
            render(&report, 16, 1)[0],
            "Claude 5h   0%",
            "the allowance is back, which is what nothing used means"
        );
    }

    #[test]
    fn a_count_otag_made_itself_is_marked_as_one() {
        let report = Report {
            rows: Vec::new(),
            counted: vec![Counted {
                agent: AgentKind::Codex,
                tokens: 1_250_000,
            }],
        };

        assert_eq!(height(&report), 1);
        assert_eq!(
            render(&report, 16, 1)[0],
            "Codex ~1.2M",
            "a count is never shown as a percentage"
        );
    }

    #[test]
    fn a_number_that_needs_the_user_is_drawn_in_a_colour_that_says_so() {
        let widget = Usage {
            report: &Report::default(),
            theme: Theme::CALM_DARK,
            now: NOW,
        };
        assert_eq!(widget.style(Level::Calm), Theme::CALM_DARK.plain());
        assert_eq!(
            widget.style(Level::Amber),
            Style::new().fg(Theme::CALM_DARK.attention)
        );
        assert_eq!(
            widget.style(Level::Red),
            Style::new().fg(Theme::CALM_DARK.error)
        );
    }

    #[test]
    fn a_report_with_more_rows_than_space_draws_what_fits() {
        let report = Report {
            rows: vec![row(100, None), row(200, None)],
            counted: Vec::new(),
        };
        let lines = render(&report, 16, 1);
        assert_eq!(lines.len(), 1, "nothing is drawn outside the widget");
        assert_eq!(lines[0], "Claude 5h  10%");
    }

    #[test]
    fn a_window_that_is_used_up_offers_the_handoff() {
        let report = Report {
            rows: vec![row(1_000, None)],
            counted: Vec::new(),
        };

        assert!(offers_handoff(&report));
        assert_eq!(height(&report), 2);
        let lines = render(&report, 22, 2);
        assert_eq!(lines[0], "Claude 5h 100%");
        assert_eq!(lines[1], OFFER, "the user is told what to do about it");
    }

    #[test]
    fn a_window_that_is_merely_worrying_offers_nothing() {
        let report = Report {
            rows: vec![row(950, None)],
            counted: Vec::new(),
        };
        assert!(!offers_handoff(&report));
        assert_eq!(height(&report), 1);
    }

    #[test]
    fn a_reading_is_drawn_by_what_it_says_and_never_dimmed_for_its_age() {
        let widget = Usage {
            report: &Report::default(),
            theme: Theme::CALM_DARK,
            now: NOW,
        };

        assert_eq!(
            widget.row_style(&row(950, None)),
            Style::new().fg(Theme::CALM_DARK.error),
            "a reading says what it has to however long ago it arrived"
        );
        assert_eq!(
            widget.row_style(&unread(Window::FiveHour)),
            Theme::CALM_DARK.dim(),
            "a dash is not a number and is not coloured like one"
        );
    }

    #[test]
    fn an_empty_report_draws_nothing() {
        assert_eq!(height(&Report::default()), 0);
        assert_eq!(render(&Report::default(), 16, 1)[0], "");
    }

    #[test]
    fn a_widget_with_no_space_draws_nothing_and_does_not_panic() {
        let report = Report {
            rows: vec![row(624, Some(NOW + 60))],
            counted: Vec::new(),
        };
        let area = Rect::new(0, 0, 0, 0);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 16, 2));
        Usage {
            report: &report,
            theme: Theme::CALM_DARK,
            now: NOW,
        }
        .render(area, &mut buffer);
    }
}
