//! Drawing a whole otag session: sidebar, tab bar, and the panes.
//!
//! The daemon decided where everything goes, so there is no layout work
//! here: this module paints what the view describes. The same functions that
//! place the sidebar's and the tab bar's text also answer where a click
//! landed, so what the user sees and what a click does cannot drift apart.

use std::collections::HashMap;

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
pub fn draw(frame: &mut Frame, view: &SessionView, screens: &Screens, theme: Theme) {
    let whole = frame.area();
    // otag's own background, so that the parts no pane covers are otag's
    // rather than whatever the terminal had.
    frame.render_widget(Block::new().style(Style::new().bg(theme.background)), whole);

    frame.render_widget(Sidebar { view, theme }, rect(view.sidebar));
    frame.render_widget(TabBar { view, theme }, rect(view.tabs_area));

    for pane in &view.panes {
        let area = rect(pane.area);
        let focused = view.focus == Some(pane.id);

        frame.render_widget(
            Block::new()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(theme.pane_border(focused))
                .title(format!(" {} ", pane.title))
                .title_style(if focused {
                    theme.picked_out()
                } else {
                    theme.dim()
                }),
            area,
        );

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
            buffer.set_stringn(
                area.x,
                y,
                format!("{mark} {}", workspace.name),
                width,
                style,
            );
        }
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

        // The workspace's name sits at the far end, out of the way of the
        // tabs but telling the user which project they are looking at.
        if let Some(workspace) = self.view.workspaces.get(self.view.active_workspace) {
            let width = u16::try_from(workspace.name.chars().count()).unwrap_or(u16::MAX);
            let used = tab_spans(self.view)
                .last()
                .map_or(area.x, |(_, _, span)| span.right());
            let x = area.right().saturating_sub(width);
            if x > used {
                buffer.set_stringn(
                    x,
                    area.y,
                    &workspace.name,
                    usize::from(width),
                    self.theme.dim(),
                );
            }
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
        let label = format!("{}:{}", index + 1, tab.title);
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
                },
                WorkspaceView {
                    id: WorkspaceId::FIRST.next(),
                    name: "api".to_owned(),
                    tabs: 1,
                    panes: 1,
                },
            ],
            active_workspace: 0,
            tabs: vec![
                TabView {
                    id: TabId::FIRST,
                    title: "shell".to_owned(),
                    panes: 2,
                },
                TabView {
                    id: TabId::FIRST.next(),
                    title: "build".to_owned(),
                    panes: 1,
                },
            ],
            active_tab: 0,
            panes: vec![
                PaneView {
                    id: pane_id(1),
                    area: left,
                    title: "shell".to_owned(),
                    frame: None,
                    exit: None,
                },
                PaneView {
                    id: pane_id(2),
                    area: right,
                    title: "claude".to_owned(),
                    frame: None,
                    exit: None,
                },
            ],
            focus: Some(pane_id(2)),
        }
    }

    fn render(view: &SessionView, screens: &Screens) -> Buffer {
        let size = view.size;
        let mut terminal =
            Terminal::new(TestBackend::new(size.cols(), size.rows())).expect("a test terminal");
        terminal
            .draw(|frame| draw(frame, view, screens, Theme::default()))
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
}
