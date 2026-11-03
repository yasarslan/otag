//! The usage widget: how much of each agent's limits is gone.
//!
//! It lives at the bottom of the sidebar, which is sixteen columns wide, so
//! every line here is written to fit in very little space and to say the one
//! thing that matters: the percentage, and when the window would be full at
//! the rate of the last twenty minutes.
//!
//! The widget is calm by design. There is no blinking and no bar that fills
//! up; a number that needs the user turns amber and then red, and that is
//! all the emphasis there is.

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

/// How many rows the widget needs for a report.
///
/// One per window, one more for each window with a projection, one per agent
/// that reports no percentage at all, and one for the offer when a window is
/// used up.
#[must_use]
pub fn height(report: &Report) -> u16 {
    let rows: usize = report
        .rows
        .iter()
        .map(|row| 1 + usize::from(detail(row).is_some()))
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

/// A duration a person reads at a glance: `6d 17h`, `2h`, `1h20m`, `45m`,
/// `30s`.
///
/// Weekly windows reset days away, and `161h` is not a number anyone reads;
/// past a day the minutes stop mattering, so they are dropped.
#[must_use]
pub fn duration(seconds: i64) -> String {
    /// Hours in a day.
    const DAY: i64 = 24;

    let seconds = seconds.max(0);
    let minutes = seconds / 60;
    let hours = minutes / 60;

    if hours >= DAY {
        let days = hours / DAY;
        let rest = hours % DAY;
        if rest == 0 {
            return format!("{days}d");
        }
        return format!("{days}d {rest}h");
    }
    if hours > 0 {
        let rest = minutes % 60;
        if rest == 0 {
            return format!("{hours}h");
        }
        return format!("{hours}h{rest:02}m");
    }
    if minutes > 0 {
        return format!("{minutes}m");
    }
    format!("{seconds}s")
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

/// How long ago a reading was taken: `updated 3h ago`.
#[must_use]
pub fn ago(seconds: i64) -> String {
    format!("updated {} ago", duration(seconds))
}

/// An agent session identifier short enough to show: the first eight
/// characters, which is what the agents themselves print.
#[must_use]
pub fn session_label(id: &str) -> String {
    const SHOWN: usize = 8;

    let short: String = id.chars().take(SHOWN).collect();
    short
}

/// The first line of a window: who, which window, how much is gone.
///
/// A window that has reset shows no percentage at all. The last one otag was
/// told belongs to a window that is over, and showing it as the number of
/// the window running now is the one thing this widget must never do.
#[must_use]
pub fn headline(row: &Row) -> String {
    match row.used_pct() {
        Some(used_pct) => format!("{} {} {used_pct:>3}%", row.agent, row.window.label()),
        None => format!("{} {} reset", row.agent, row.window.label()),
    }
}

/// The second line of a window, when there is something to add.
///
/// A window waiting for its first reading says so, and an old reading says
/// how old it is: both of those change what the line above it means. After
/// that comes the projection, because it is the useful number, and the reset
/// when nothing is climbing, so the user knows how long they are waiting.
#[must_use]
pub fn detail(row: &Row) -> Option<String> {
    if row.reset {
        return Some("awaiting data".to_owned());
    }
    if row.is_stale() {
        return Some(ago(row.age));
    }
    if let Some(full_in) = row.full_in {
        return Some(format!("full in {}", duration(full_in)));
    }
    row.resets_in
        .filter(|resets_in| *resets_in > 0)
        .map(|resets_in| format!("resets {}", duration(resets_in)))
}

/// What otag counted for an agent that reports no percentage of its own.
///
/// Marked with a tilde, and in tokens rather than as a percentage: nobody
/// knows what the account's allowance is, and a number that looks like a
/// percentage would be a lie.
#[must_use]
pub fn counted_line(agent: otag_agents::AgentKind, count: u64) -> String {
    format!("{agent} ~{}", tokens(count))
}

/// The usage widget.
#[derive(Debug, Clone, Copy)]
pub struct Usage<'a> {
    /// What to show.
    pub report: &'a Report,
    /// Colours to draw it in.
    pub theme: Theme,
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
            if let Some(detail) = detail(row) {
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
    /// A number otag cannot vouch for is dimmed rather than coloured: an old
    /// reading must not shout, and a window waiting for data has nothing to
    /// shout about.
    fn row_style(&self, row: &Row) -> Style {
        if row.reset || row.is_stale() {
            return self.theme.dim();
        }
        self.style(row.level)
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

    fn row(used_tenths: u16, full_in: Option<i64>, resets_in: Option<i64>) -> Row {
        Row {
            agent: AgentKind::Claude,
            window: Window::FiveHour,
            used_tenths: Some(used_tenths),
            resets_in,
            full_in,
            level: Level::of(f32::from(used_tenths) / 10.0),
            at: 0,
            age: 0,
            reset: false,
            session: None,
        }
    }

    /// A window that reset, with no reading of the new one yet.
    fn reset_row(age: i64) -> Row {
        Row {
            agent: AgentKind::Claude,
            window: Window::FiveHour,
            used_tenths: None,
            resets_in: None,
            full_in: None,
            level: Level::Calm,
            at: 0,
            age,
            reset: true,
            session: None,
        }
    }

    fn render(report: &Report, width: u16, height: u16) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        Usage {
            report,
            theme: Theme::CALM_DARK,
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
    fn a_duration_is_read_at_a_glance() {
        assert_eq!(duration(30), "30s");
        assert_eq!(duration(90), "1m");
        assert_eq!(duration(2_700), "45m");
        assert_eq!(duration(4_800), "1h20m");
        assert_eq!(duration(7_200), "2h");
        assert_eq!(
            duration(-5),
            "0s",
            "a window that has reset is not negative"
        );
    }

    #[test]
    fn a_reset_days_away_is_shown_in_days() {
        // A weekly window: `161h` is not a number anyone reads.
        assert_eq!(duration(579_600), "6d 17h");
        assert_eq!(duration(86_400), "1d");
        assert_eq!(duration(86_399), "23h59m");
        assert_eq!(duration(604_800), "7d");
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
    fn a_window_is_one_line_with_the_projection_below_it() {
        let report = Report {
            rows: vec![row(624, Some(4_800), Some(5_040))],
            counted: Vec::new(),
        };

        assert_eq!(height(&report), 2);
        let lines = render(&report, 16, 2);
        assert_eq!(lines[0], "claude 5h  62%");
        assert_eq!(
            lines[1], " full in 1h20m",
            "the projection is what the widget is for"
        );
    }

    #[test]
    fn a_window_with_nothing_climbing_shows_when_it_resets_instead() {
        let report = Report {
            rows: vec![row(380, None, Some(5_040))],
            counted: Vec::new(),
        };
        assert_eq!(render(&report, 16, 2)[1], " resets 1h24m");
    }

    #[test]
    fn a_window_with_nothing_to_add_is_one_line() {
        let report = Report {
            rows: vec![row(380, None, None)],
            counted: Vec::new(),
        };
        assert_eq!(height(&report), 1);
        assert_eq!(render(&report, 16, 2)[1], "");
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
            "codex ~1.2M",
            "a count is never shown as a percentage"
        );
    }

    #[test]
    fn a_number_that_needs_the_user_is_drawn_in_a_colour_that_says_so() {
        let widget = Usage {
            report: &Report::default(),
            theme: Theme::CALM_DARK,
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
            rows: vec![row(100, None, None), row(200, None, None)],
            counted: Vec::new(),
        };
        let lines = render(&report, 16, 1);
        assert_eq!(lines.len(), 1, "nothing is drawn outside the widget");
        assert_eq!(lines[0], "claude 5h  10%");
    }

    #[test]
    fn a_window_that_is_used_up_offers_the_handoff() {
        let report = Report {
            rows: vec![row(1_000, None, Some(600))],
            counted: Vec::new(),
        };

        assert!(offers_handoff(&report));
        assert_eq!(height(&report), 3);
        let lines = render(&report, 22, 3);
        assert_eq!(lines[0], "claude 5h 100%");
        assert_eq!(lines[2], OFFER, "the user is told what to do about it");
    }

    #[test]
    fn a_window_that_is_merely_worrying_offers_nothing() {
        let report = Report {
            rows: vec![row(950, None, None)],
            counted: Vec::new(),
        };
        assert!(!offers_handoff(&report));
        assert_eq!(height(&report), 1);
    }

    #[test]
    fn how_long_ago_a_reading_was_taken_reads_as_a_sentence() {
        assert_eq!(ago(10_800), "updated 3h ago");
        assert_eq!(ago(900), "updated 15m ago");
        assert_eq!(ago(4_800), "updated 1h20m ago");
        assert_eq!(ago(0), "updated 0s ago");
    }

    #[test]
    fn a_window_that_has_reset_shows_no_percentage() {
        let report = Report {
            rows: vec![reset_row(10_800)],
            counted: Vec::new(),
        };

        assert_eq!(height(&report), 2);
        let lines = render(&report, 16, 2);
        assert_eq!(
            lines[0], "claude 5h reset",
            "the percentage from the window before is not shown as this one's"
        );
        assert_eq!(lines[1], " awaiting data");
        assert!(
            !offers_handoff(&report),
            "a window with no number is not a window that is used up"
        );
    }

    #[test]
    fn an_old_reading_says_how_old_it_is() {
        let mut row = row(680, None, Some(5_040));
        row.age = 10_800;
        let report = Report {
            rows: vec![row],
            counted: Vec::new(),
        };

        let lines = render(&report, 16, 2);
        assert_eq!(lines[0], "claude 5h  68%");
        assert_eq!(
            lines[1], " updated 3h ago",
            "how old the number is matters more than when the window resets"
        );
    }

    #[test]
    fn a_fresh_reading_shows_the_projection_and_not_an_age() {
        let mut row = row(680, Some(4_800), Some(5_040));
        row.age = 30;
        let report = Report {
            rows: vec![row],
            counted: Vec::new(),
        };
        assert_eq!(render(&report, 16, 2)[1], " full in 1h20m");
    }

    #[test]
    fn a_number_otag_cannot_vouch_for_is_dimmed_rather_than_coloured() {
        let widget = Usage {
            report: &Report::default(),
            theme: Theme::CALM_DARK,
        };
        let mut old = row(950, None, None);
        old.age = 10_800;

        assert_eq!(
            widget.row_style(&old),
            Theme::CALM_DARK.dim(),
            "an old number must not shout"
        );
        assert_eq!(widget.row_style(&reset_row(60)), Theme::CALM_DARK.dim());
        assert_eq!(
            widget.row_style(&row(950, None, None)),
            Style::new().fg(Theme::CALM_DARK.error),
            "a current number still says what it has to"
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
            rows: vec![row(624, Some(60), None)],
            counted: Vec::new(),
        };
        let area = Rect::new(0, 0, 0, 0);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 16, 2));
        Usage {
            report: &report,
            theme: Theme::CALM_DARK,
        }
        .render(area, &mut buffer);
    }
}
