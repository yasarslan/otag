//! Drawing a whole otag session: sidebar, tab bar, and the panes.
//!
//! The daemon decided where everything goes, so there is no layout work
//! here: this module paints what the view describes. The same functions that
//! place the sidebar's and the tab bar's text also answer where a click
//! landed, so what the user sees and what a click does cannot drift apart.

use std::collections::HashMap;

use otag_agents::PaneStatus;
use otag_core::frame::ScreenState;
use otag_core::layout::Area;
use otag_core::pane::PaneId;
use otag_core::proto::SessionView;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, BorderType, Borders, Widget};

use crate::render::Pane;
use crate::theme::Theme;

/// What the sidebar calls itself.
const SIDEBAR_HEADER: &str = "workspaces";

/// Marks the workspace that is shown.
const ACTIVE_MARK: &str = "\u{25cf}";

/// Marks a workspace that is running but not shown.
const INACTIVE_MARK: &str = "\u{25cb}";

/// Blank columns between two tabs in the tab bar.
const TAB_GAP: u16 = 1;

/// What otag draws for what an agent is doing.
///
/// An idle agent is marked with nothing at all: a sidebar of symbols that
/// are always there says nothing, and the point of these is to be noticed.
#[must_use]
pub const fn status_mark(status: PaneStatus) -> Option<&'static str> {
    match status {
        PaneStatus::Idle => None,
        PaneStatus::Working => Some("\u{25d0}"),
        PaneStatus::Done => Some("\u{2713}"),
        PaneStatus::Looping => Some("\u{21bb}"),
        PaneStatus::Error => Some("\u{00d7}"),
        PaneStatus::NeedsApproval => Some("?"),
    }
}

/// The colour a status is drawn in: the attention colour for what wants the
/// user, and nothing loud for anything else.
fn status_style(status: PaneStatus, theme: Theme) -> Style {
    match status {
        PaneStatus::Error => Style::new().fg(theme.error).bg(theme.background),
        PaneStatus::NeedsApproval | PaneStatus::Looping => {
            Style::new().fg(theme.attention).bg(theme.background)
        }
        PaneStatus::Done => Style::new().fg(theme.accent).bg(theme.background),
        PaneStatus::Idle | PaneStatus::Working => theme.dim(),
    }
}

/// The screens a client is drawing, by pane.
pub type Screens = HashMap<PaneId, ScreenState>;

/// Turn one of the daemon's rectangles into one the renderer can use.
#[must_use]
pub const fn rect(area: Area) -> Rect {
    Rect {
        x: area.x,
        y: area.y,
        width: area.cols,
        height: area.rows,
    }
}

/// Draw the whole session.
pub fn draw(
    frame: &mut Frame,
    view: &SessionView,
    screens: &Screens,
    theme: Theme,
    notice: Option<&str>,
) {
    // Nothing paints a background: every cell otag does not write keeps the
    // terminal's own colours, which is also what a pane's program expects of
    // the cells it leaves alone.
    frame.render_widget(Sidebar { view, theme }, rect(view.sidebar));
    frame.render_widget(
        crate::usage::Usage {
            report: &view.usage,
            theme,
        },
        usage_area(rect(view.sidebar), &view.usage),
    );
    frame.render_widget(
        TabBar {
            view,
            theme,
            notice,
        },
        rect(view.tabs_area),
    );

    for pane in &view.panes {
        let area = rect(pane.area);
        let focused = view.focus == Some(pane.id);

        let mut block = Block::new()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(theme.pane_border(focused))
            .title(format!(" {} ", pane.title))
            .title_style(if focused {
                theme.picked_out()
            } else {
                theme.dim()
            });
        if let Some(mark) = status_mark(pane.status) {
            // On the right of the border, so the pane's own name stays
            // where the eye expects it.
            block = block
                .title_top(
                    ratatui::text::Line::from(format!(" {mark} {} ", pane.status)).right_aligned(),
                )
                .title_style(status_style(pane.status, theme));
        }
        frame.render_widget(block, area);

        let inside = rect(pane.area.inset(otag_core::layout::BORDER));
        if inside.is_empty() {
            continue;
        }
        let Some(screen) = screens.get(&pane.id) else {
            continue;
        };

        let widget = Pane::new(screen);
        frame.render_widget(widget, inside);
        if focused && let Some(position) = widget.cursor_position(inside) {
            frame.set_cursor_position(position);
        }
    }
}

/// The list of workspaces down the left-hand side.
#[derive(Debug, Clone, Copy)]
struct Sidebar<'a> {
    view: &'a SessionView,
    theme: Theme,
}

impl Widget for Sidebar<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let width = usize::from(area.width);
        buffer.set_stringn(area.x, area.y, SIDEBAR_HEADER, width, self.theme.dim());

        for (index, workspace) in self.view.workspaces.iter().enumerate() {
            let Some(y) = workspace_row(area, index) else {
                // The sidebar is full; the rest are still reachable by key.
                break;
            };
            let active = index == self.view.active_workspace;
            let mark = if active { ACTIVE_MARK } else { INACTIVE_MARK };
            let style = if active {
                self.theme.picked_out()
            } else {
                self.theme.plain()
            };
            let written = buffer
                .set_stringn(
                    area.x,
                    y,
                    format!("{mark} {}", workspace.name),
                    width,
                    style,
                )
                .0;
            if let Some(status) = status_mark(workspace.status) {
                buffer.set_stringn(
                    written.saturating_add(1),
                    y,
                    status,
                    usize::from(area.right().saturating_sub(written)),
                    status_style(workspace.status, self.theme),
                );
            }
        }
    }
}

/// Where the usage widget goes: the bottom of the sidebar.
///
/// It takes only the rows it needs, and nothing at all when the sidebar is
/// hidden or too short to hold both a workspace and a number. The workspaces
/// keep the top, because a sidebar whose list moves when a percentage appears
/// would be unreadable.
fn usage_area(sidebar: Rect, report: &otag_features::usage::Report) -> Rect {
    /// Rows kept for the header and at least one workspace.
    const KEPT: u16 = 2;

    let wanted = crate::usage::height(report);
    if sidebar.is_empty() || wanted == 0 || sidebar.height <= KEPT {
        return Rect::ZERO;
    }

    let height = wanted.min(sidebar.height.saturating_sub(KEPT));
    Rect {
        x: sidebar.x,
        y: sidebar.bottom().saturating_sub(height),
        width: sidebar.width,
        height,
    }
}

/// Where the sidebar draws the workspace at `index`, if it fits.
fn workspace_row(area: Rect, index: usize) -> Option<u16> {
    let offset = u16::try_from(index).ok()?.checked_add(1)?;
    let y = area.y.checked_add(offset)?;
    (y < area.bottom()).then_some(y)
}

/// Which workspace the sidebar has at this cell, if any.
#[must_use]
pub fn workspace_at(view: &SessionView, x: u16, y: u16) -> Option<usize> {
    let area = rect(view.sidebar);
    if area.is_empty() || x < area.x || x >= area.right() {
        return None;
    }
    (0..view.workspaces.len()).find(|index| workspace_row(area, *index) == Some(y))
}

/// The tabs of the shown workspace, with the workspace's name after them.
#[derive(Debug, Clone, Copy)]
struct TabBar<'a> {
    view: &'a SessionView,
    theme: Theme,
    /// Something otag has to tell the user: what a handoff did, or why a
    /// request was refused. It takes the place of the workspace's name while
    /// it is there, because the name is always one keypress away and the
    /// notice is not.
    notice: Option<&'a str>,
}

impl Widget for TabBar<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        for (index, label, span) in tab_spans(self.view) {
            if span.x >= area.right() {
                break;
            }
            let style = if index == self.view.active_tab {
                self.theme.picked_out()
            } else {
                self.theme.dim()
            };
            buffer.set_stringn(
                span.x,
                area.y,
                &label,
                usize::from(area.right().saturating_sub(span.x)),
                style,
            );
        }

        // The far end of the bar carries either a notice or, when there is
        // none, the name of the workspace: it says which project this is,
        // and it is out of the way of the tabs.
        let (text, style) = match self.notice {
            Some(notice) => (notice, Style::new().fg(self.theme.attention)),
            None => match self.view.workspaces.get(self.view.active_workspace) {
                Some(workspace) => (workspace.name.as_str(), self.theme.dim()),
                None => return,
            },
        };

        let width = u16::try_from(text.chars().count()).unwrap_or(u16::MAX);
        let used = tab_spans(self.view)
            .last()
            .map_or(area.x, |(_, _, span)| span.right());
        let x = area.right().saturating_sub(width);
        if x > used {
            buffer.set_stringn(x, area.y, text, usize::from(width), style);
        } else if used < area.right() {
            // Not enough room for all of it; what fits is still worth
            // showing, because a notice the user never sees is worse.
            buffer.set_stringn(
                used.saturating_add(1),
                area.y,
                text,
                usize::from(area.right().saturating_sub(used).saturating_sub(1)),
                style,
            );
        }
    }
}

/// What the tab bar draws for each tab, and where.
///
/// Used both to draw the bar and to decide what a click on it means.
fn tab_spans(view: &SessionView) -> Vec<(usize, String, Rect)> {
    let area = rect(view.tabs_area);
    let mut spans = Vec::with_capacity(view.tabs.len());
    let mut x = area.x;

    for (index, tab) in view.tabs.iter().enumerate() {
        let mark = status_mark(tab.status)
            .map(|mark| format!(" {mark}"))
            .unwrap_or_default();
        let label = format!("{}:{}{mark}", index + 1, tab.title);
        let width = u16::try_from(label.chars().count()).unwrap_or(u16::MAX);
        spans.push((
            index,
            label,
            Rect::new(x, area.y, width, area.height.min(1)),
        ));
        x = x.saturating_add(width).saturating_add(TAB_GAP);
    }
    spans
}

/// Which tab the tab bar has at this cell, if any.
#[must_use]
pub fn tab_at(view: &SessionView, x: u16, y: u16) -> Option<usize> {
    if y != view.tabs_area.y || view.tabs_area.is_empty() {
        return None;
    }
    tab_spans(view)
        .into_iter()
        .find(|(_, _, span)| x >= span.x && x < span.right())
        .map(|(index, _, _)| index)
}

#[cfg(test)]
mod tests {
    use otag_core::frame::{CursorState, InputModes, ScreenFrame, ScreenRow, StyledRun};
    use otag_core::pane::PaneSize;
    use otag_core::proto::{PaneView, TabView, WorkspaceView};
    use otag_core::workspace::{TabId, WorkspaceId};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    fn pane_id(id: u64) -> PaneId {
        let mut pane = PaneId::FIRST;
        for _ in 1..id {
            pane = pane.next();
        }
        pane
    }

    fn screen(size: PaneSize, text: &str) -> ScreenState {
        let mut screen = ScreenState::new(size);
        screen.apply(&ScreenFrame {
            size,
            cursor: CursorState::default(),
            modes: InputModes::default(),
            full: true,
            rows: vec![ScreenRow {
                index: 0,
                runs: vec![StyledRun {
                    text: text.to_owned(),
                    style: otag_core::frame::CellStyle::default(),
                }],
            }],
        });
        screen
    }

    /// A view of two panes side by side in a 24x80 terminal.
    fn view() -> SessionView {
        let left = Area::new(16, 1, 32, 23);
        let right = Area::new(48, 1, 32, 23);
        SessionView {
            generation: 1,
            size: PaneSize::new(24, 80),
            sidebar: Area::new(0, 0, 16, 24),
            tabs_area: Area::new(16, 0, 64, 1),
            content: Area::new(16, 1, 64, 23),
            workspaces: vec![
                WorkspaceView {
                    id: WorkspaceId::FIRST,
                    name: "my-app".to_owned(),
                    tabs: 2,
                    panes: 2,
                    status: PaneStatus::Working,
                },
                WorkspaceView {
                    id: WorkspaceId::FIRST.next(),
                    name: "api".to_owned(),
                    tabs: 1,
                    panes: 1,
                    status: PaneStatus::NeedsApproval,
                },
            ],
            active_workspace: 0,
            tabs: vec![
                TabView {
                    id: TabId::FIRST,
                    title: "shell".to_owned(),
                    panes: 2,
                    status: PaneStatus::Idle,
                },
                TabView {
                    id: TabId::FIRST.next(),
                    title: "build".to_owned(),
                    panes: 1,
                    status: PaneStatus::Idle,
                },
            ],
            active_tab: 0,
            panes: vec![
                PaneView {
                    id: pane_id(1),
                    area: left,
                    title: "shell".to_owned(),
                    agent: None,
                    status: PaneStatus::Idle,
                    frame: None,
                    exit: None,
                },
                PaneView {
                    id: pane_id(2),
                    area: right,
                    title: "claude".to_owned(),
                    agent: Some(otag_agents::AgentKind::Claude),
                    status: PaneStatus::Working,
                    frame: None,
                    exit: None,
                },
            ],
            focus: Some(pane_id(2)),
            usage: Default::default(),
        }
    }

    fn render(view: &SessionView, screens: &Screens) -> Buffer {
        let size = view.size;
        let mut terminal =
            Terminal::new(TestBackend::new(size.cols(), size.rows())).expect("a test terminal");
        terminal
            .draw(|frame| draw(frame, view, screens, Theme::default(), None))
            .expect("the session draws");
        terminal.backend().buffer().clone()
    }

    fn row(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_owned()))
            .collect()
    }

    #[test]
    fn the_sidebar_lists_the_workspaces_and_marks_the_one_shown() {
        let buffer = render(&view(), &Screens::new());
        assert!(row(&buffer, 0).starts_with(SIDEBAR_HEADER));
        assert!(
            row(&buffer, 1).starts_with(&format!("{ACTIVE_MARK} my-app")),
            "got {:?}",
            row(&buffer, 1)
        );
        assert!(row(&buffer, 2).starts_with(&format!("{INACTIVE_MARK} api")));
    }

    #[test]
    fn the_tab_bar_numbers_the_tabs_from_one() {
        let buffer = render(&view(), &Screens::new());
        let bar = row(&buffer, 0);
        assert!(bar.contains("1:shell"), "got {bar:?}");
        assert!(bar.contains("2:build"), "got {bar:?}");
        assert!(bar.trim_end().ends_with("my-app"), "got {bar:?}");
    }

    #[test]
    fn a_pane_is_drawn_inside_its_border() {
        let mut screens = Screens::new();
        // The pane's own area is its border box; the screen fills the inside.
        screens.insert(pane_id(1), screen(PaneSize::new(21, 30), "left-pane"));
        screens.insert(pane_id(2), screen(PaneSize::new(21, 30), "right-pane"));

        let buffer = render(&view(), &screens);
        let line = row(&buffer, 2);
        assert!(
            line.contains("left-pane") && line.contains("right-pane"),
            "both panes are drawn: {line:?}"
        );
        // Column 16 is the left pane's border, 17 the first cell inside it.
        assert_eq!(
            buffer.cell((17, 2)).map(|cell| cell.symbol().to_owned()),
            Some("l".to_owned())
        );
    }

    #[test]
    fn a_pane_s_title_is_on_its_border() {
        let buffer = render(&view(), &Screens::new());
        let line = row(&buffer, 1);
        assert!(line.contains(" shell "), "got {line:?}");
        assert!(line.contains(" claude "), "got {line:?}");
    }

    #[test]
    fn only_the_focused_pane_has_the_focused_border() {
        let buffer = render(&view(), &Screens::new());
        let theme = Theme::default();
        let focused = buffer.cell((48, 1)).expect("the right pane's corner");
        let other = buffer.cell((16, 1)).expect("the left pane's corner");
        assert_eq!(focused.fg, theme.border_focus);
        assert_eq!(other.fg, theme.border);
    }

    #[test]
    fn a_pane_with_no_screen_yet_draws_its_border_and_nothing_else() {
        // A view can arrive before the pane's first frame does.
        let buffer = render(&view(), &Screens::new());
        assert_eq!(
            buffer.cell((17, 2)).map(|cell| cell.symbol().to_owned()),
            Some(" ".to_owned())
        );
    }

    #[test]
    fn a_click_on_the_tab_bar_finds_the_tab() {
        let view = view();
        assert_eq!(tab_at(&view, 16, 0), Some(0));
        assert_eq!(tab_at(&view, 22, 0), Some(0), "the last cell of 1:shell");
        assert_eq!(tab_at(&view, 24, 0), Some(1));
        assert_eq!(tab_at(&view, 23, 0), None, "the gap between tabs");
        assert_eq!(tab_at(&view, 70, 0), None);
        assert_eq!(tab_at(&view, 16, 5), None, "not on the bar at all");
    }

    #[test]
    fn a_click_on_the_sidebar_finds_the_workspace() {
        let view = view();
        assert_eq!(workspace_at(&view, 2, 1), Some(0));
        assert_eq!(workspace_at(&view, 2, 2), Some(1));
        assert_eq!(workspace_at(&view, 2, 0), None, "the header");
        assert_eq!(workspace_at(&view, 2, 3), None, "below the list");
        assert_eq!(workspace_at(&view, 40, 1), None, "not in the sidebar");
    }

    #[test]
    fn a_hidden_sidebar_takes_no_clicks() {
        let mut view = view();
        view.sidebar = Area::EMPTY;
        assert_eq!(workspace_at(&view, 0, 1), None);
    }

    #[test]
    fn a_terminal_too_small_for_the_whole_sidebar_draws_what_fits() {
        let mut view = view();
        view.size = PaneSize::new(2, 80);
        view.sidebar = Area::new(0, 0, 16, 2);
        view.tabs_area = Area::new(16, 0, 64, 1);
        view.content = Area::new(16, 1, 64, 1);
        view.panes = vec![PaneView {
            id: pane_id(1),
            area: Area::new(16, 1, 64, 1),
            title: "shell".to_owned(),
            agent: None,
            status: PaneStatus::Idle,
            frame: None,
            exit: None,
        }];

        // One row is left for the list, so only the first workspace shows,
        // and nothing panics about the ones that do not fit.
        let buffer = render(&view, &Screens::new());
        assert!(row(&buffer, 1).starts_with(&format!("{ACTIVE_MARK} my-app")));
    }

    #[test]
    fn an_empty_view_draws_nothing_and_does_not_panic() {
        let view = SessionView {
            size: PaneSize::new(5, 10),
            ..SessionView::default()
        };
        let buffer = render(&view, &Screens::new());
        assert_eq!(row(&buffer, 0), " ".repeat(10));
    }

    #[test]
    fn a_pane_s_border_says_what_its_agent_is_doing() {
        let buffer = render(&view(), &Screens::new());
        // The right-hand pane runs an agent that is working.
        let border = row(&buffer, 1);
        assert!(border.contains(" claude "), "got {border:?}");
        assert!(
            border.contains("working"),
            "the pane does not say what it is doing: {border:?}"
        );
    }

    #[test]
    fn an_idle_pane_is_marked_with_nothing() {
        // A row of symbols that are always there says nothing.
        let mut view = view();
        for pane in &mut view.panes {
            pane.status = PaneStatus::Idle;
        }
        let buffer = render(&view, &Screens::new());
        let border = row(&buffer, 1);
        assert!(!border.contains("idle"), "got {border:?}");
        assert_eq!(status_mark(PaneStatus::Idle), None);
    }

    #[test]
    fn the_sidebar_marks_a_workspace_whose_agents_want_the_user() {
        let buffer = render(&view(), &Screens::new());
        // The second workspace has a pane waiting for permission.
        let line = row(&buffer, 2);
        assert!(
            line.contains('?'),
            "the sidebar does not mark the workspace: {line:?}"
        );

        let theme = Theme::default();
        let marked = (0..16)
            .filter_map(|x| buffer.cell((x, 2)))
            .find(|cell| cell.symbol() == "?")
            .expect("the mark is drawn");
        assert_eq!(marked.fg, theme.attention);
    }

    #[test]
    fn the_tab_bar_marks_a_tab_without_moving_what_a_click_finds() {
        let mut view = view();
        view.tabs[0].status = PaneStatus::NeedsApproval;

        let buffer = render(&view, &Screens::new());
        let bar = row(&buffer, 0);
        assert!(bar.contains("1:shell ?"), "got {bar:?}");

        // The mark is part of the tab's own label, so a click on it still
        // chooses that tab.
        assert_eq!(tab_at(&view, 24, 0), Some(0));
        assert_eq!(tab_at(&view, 26, 0), Some(1));
    }

    #[test]
    fn every_status_that_wants_the_user_is_drawn_in_a_colour_that_says_so() {
        let theme = Theme::default();
        for status in [PaneStatus::NeedsApproval, PaneStatus::Looping] {
            assert_eq!(status_style(status, theme).fg, Some(theme.attention));
        }
        assert_eq!(status_style(PaneStatus::Error, theme).fg, Some(theme.error));
        assert_eq!(
            status_style(PaneStatus::Working, theme).fg,
            Some(theme.muted),
            "working is not worth the eye"
        );
    }
}
