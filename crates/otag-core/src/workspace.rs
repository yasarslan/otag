//! Workspaces and tabs: how a session is organised above the pane level.
//!
//! A workspace is one project folder. It holds tabs, each of which holds a
//! [`Layout`] of panes and remembers which of them has the focus. Nothing
//! here runs anything; [`crate::session`] owns the panes themselves and uses
//! these types to decide which of them a client can see.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::layout::{Direction, Layout, Side};
use crate::pane::PaneId;

/// Name a workspace gets when nothing better is known about its directory.
const UNNAMED: &str = "otag";

/// Identifier of a workspace, unique within one daemon run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkspaceId(u64);

/// Identifier of a tab, unique within one daemon run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TabId(u64);

macro_rules! counted_id {
    ($name:ident) => {
        impl $name {
            /// Identifier handed out first.
            pub const FIRST: Self = Self(1);

            /// The identifier after this one. Saturates rather than wrapping,
            /// so an identifier is never reused.
            #[must_use]
            pub const fn next(self) -> Self {
                Self(self.0.saturating_add(1))
            }

            /// The identifier as a number.
            #[must_use]
            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "{}", self.0)
            }
        }
    };
}

counted_id!(WorkspaceId);
counted_id!(TabId);

/// One tab: a layout of panes and the pane the user is typing into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    id: TabId,
    title: String,
    layout: Layout,
    focus: PaneId,
}

impl Tab {
    /// A tab holding one pane.
    #[must_use]
    pub fn new(id: TabId, title: impl Into<String>, pane: PaneId) -> Self {
        Self {
            id,
            title: title.into(),
            layout: Layout::new(pane),
            focus: pane,
        }
    }

    /// Which tab this is.
    #[must_use]
    pub const fn id(&self) -> TabId {
        self.id
    }

    /// What the tab bar shows for it.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Rename the tab.
    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
    }

    /// How the tab's panes divide its space.
    #[must_use]
    pub const fn layout(&self) -> &Layout {
        &self.layout
    }

    /// The pane the user is typing into.
    #[must_use]
    pub const fn focus(&self) -> PaneId {
        self.focus
    }

    /// Focus `pane`, if the tab holds it. Returns whether it does.
    pub fn focus_pane(&mut self, pane: PaneId) -> bool {
        if !self.layout.contains(pane) {
            return false;
        }
        self.focus = pane;
        true
    }

    /// Focus the pane after the focused one, wrapping around.
    pub fn focus_next(&mut self) {
        let panes = self.layout.panes();
        let Some(index) = panes.iter().position(|pane| *pane == self.focus) else {
            // The focused pane is gone; the first one is as good a choice as
            // any and better than none.
            if let Some(first) = panes.first() {
                self.focus = *first;
            }
            return;
        };
        if let Some(next) = panes.get((index + 1) % panes.len()) {
            self.focus = *next;
        }
    }

    /// Focus the pane on `side` of the focused one, if there is one.
    ///
    /// Returns whether the focus moved.
    pub fn focus_side(&mut self, side: Side, area: crate::layout::Area) -> bool {
        match self.layout.neighbour(self.focus, side, area) {
            Some(pane) => {
                self.focus = pane;
                true
            }
            None => false,
        }
    }

    /// Split the focused pane and focus the new one.
    ///
    /// Returns whether the split happened; it does not when the focused pane
    /// has gone missing.
    pub fn split(&mut self, direction: Direction, new_pane: PaneId) -> bool {
        if !self.layout.split(self.focus, direction, new_pane) {
            return false;
        }
        self.focus = new_pane;
        true
    }

    /// Move the focused pane's edge on `side` outwards.
    pub fn resize(&mut self, side: Side, amount: u32) -> bool {
        self.layout.resize(self.focus, side, amount)
    }

    /// Remove `pane`. Returns whether the tab still has a pane; a tab that
    /// does not is over.
    pub fn close_pane(&mut self, pane: PaneId) -> bool {
        let was_focused = self.focus == pane;
        if !self.layout.close(pane) {
            return false;
        }
        if was_focused {
            // The focus has to land somewhere, and the pane that took over
            // the space is the one the user is looking at.
            if let Some(first) = self.layout.panes().first() {
                self.focus = *first;
            }
        }
        true
    }

    /// Every pane in the tab, in the order they are drawn.
    #[must_use]
    pub fn panes(&self) -> Vec<PaneId> {
        self.layout.panes()
    }
}

/// One project folder, with its tabs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    id: WorkspaceId,
    name: String,
    cwd: Option<PathBuf>,
    tabs: Vec<Tab>,
    active: usize,
}

impl Workspace {
    /// A workspace with one tab.
    #[must_use]
    pub fn new(id: WorkspaceId, name: impl Into<String>, cwd: Option<PathBuf>, tab: Tab) -> Self {
        Self {
            id,
            name: name.into(),
            cwd,
            tabs: vec![tab],
            active: 0,
        }
    }

    /// Which workspace this is.
    #[must_use]
    pub const fn id(&self) -> WorkspaceId {
        self.id
    }

    /// What the sidebar shows for it.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Rename the workspace.
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    /// The directory its panes start in.
    #[must_use]
    pub fn cwd(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }

    /// Its tabs, left to right.
    #[must_use]
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// Which tab is shown, as an index into [`Workspace::tabs`].
    #[must_use]
    pub const fn active(&self) -> usize {
        self.active
    }

    /// The tab that is shown.
    #[must_use]
    pub fn tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    /// The tab that is shown, to change.
    pub fn tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    /// Add a tab after the one that is shown, and show it.
    pub fn add_tab(&mut self, tab: Tab) {
        let at = (self.active + 1).min(self.tabs.len());
        self.tabs.insert(at, tab);
        self.active = at;
    }

    /// Show the tab at `index`, if there is one. Returns whether there was.
    pub fn select(&mut self, index: usize) -> bool {
        if index >= self.tabs.len() {
            return false;
        }
        self.active = index;
        true
    }

    /// Remove `pane` from whichever tab holds it, dropping a tab that is
    /// left empty.
    ///
    /// Returns whether the workspace still has a tab; a workspace that does
    /// not is over.
    pub fn close_pane(&mut self, pane: PaneId) -> bool {
        let Some(index) = self.tabs.iter().position(|tab| tab.layout().contains(pane)) else {
            return !self.tabs.is_empty();
        };

        if let Some(tab) = self.tabs.get_mut(index)
            && !tab.close_pane(pane)
        {
            self.tabs.remove(index);
            // Showing the tab that took the closed one's place keeps the
            // user where they were; the last tab is the fallback.
            self.active = self.active.min(self.tabs.len().saturating_sub(1));
        }
        !self.tabs.is_empty()
    }

    /// Every pane in every tab.
    #[must_use]
    pub fn panes(&self) -> Vec<PaneId> {
        self.tabs.iter().flat_map(Tab::panes).collect()
    }
}

/// The name a workspace gets for a directory: the folder's own name.
#[must_use]
pub fn name_for(cwd: Option<&Path>) -> String {
    cwd.and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| UNNAMED.to_owned())
}

/// A name not already taken by one of `existing`, by adding a number.
///
/// Two checkouts of the same project would otherwise be two entries called
/// the same thing, which is exactly when telling them apart matters.
#[must_use]
pub fn unique_name(wanted: &str, existing: &[String]) -> String {
    if !existing.iter().any(|name| name == wanted) {
        return wanted.to_owned();
    }
    for suffix in 2..=u32::MAX {
        let candidate = format!("{wanted} {suffix}");
        if !existing.contains(&candidate) {
            return candidate;
        }
    }
    wanted.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Area;

    fn pane(id: u64) -> PaneId {
        let mut pane = PaneId::FIRST;
        for _ in 1..id {
            pane = pane.next();
        }
        pane
    }

    fn tab() -> Tab {
        Tab::new(TabId::FIRST, "shell", pane(1))
    }

    #[test]
    fn identifiers_count_up_and_never_wrap() {
        assert_eq!(WorkspaceId::FIRST.get(), 1);
        assert_eq!(WorkspaceId::FIRST.next().get(), 2);
        assert_eq!(TabId::FIRST.next().to_string(), "2");
    }

    #[test]
    fn a_new_tab_focuses_its_only_pane() {
        let tab = tab();
        assert_eq!(tab.focus(), pane(1));
        assert_eq!(tab.panes(), vec![pane(1)]);
    }

    #[test]
    fn splitting_focuses_the_new_pane() {
        let mut tab = tab();
        assert!(tab.split(Direction::LeftRight, pane(2)));
        assert_eq!(tab.focus(), pane(2));
    }

    #[test]
    fn the_focus_moves_to_the_pane_on_that_side() {
        let mut tab = tab();
        assert!(tab.split(Direction::LeftRight, pane(2)));
        let area = Area::new(0, 0, 40, 10);

        assert!(tab.focus_side(Side::Left, area));
        assert_eq!(tab.focus(), pane(1));
        assert!(!tab.focus_side(Side::Left, area), "nothing further left");
        assert_eq!(tab.focus(), pane(1));
    }

    #[test]
    fn the_focus_cycles_through_the_panes() {
        let mut tab = tab();
        assert!(tab.split(Direction::LeftRight, pane(2)));
        assert!(tab.split(Direction::LeftRight, pane(3)));
        assert_eq!(tab.focus(), pane(3));

        tab.focus_next();
        assert_eq!(tab.focus(), pane(1), "the cycle wraps around");
        tab.focus_next();
        assert_eq!(tab.focus(), pane(2));
    }

    #[test]
    fn focusing_a_pane_the_tab_does_not_have_is_refused() {
        let mut tab = tab();
        assert!(!tab.focus_pane(pane(9)));
        assert_eq!(tab.focus(), pane(1));
        assert!(tab.focus_pane(pane(1)));
    }

    #[test]
    fn closing_the_focused_pane_moves_the_focus_to_what_is_left() {
        let mut tab = tab();
        assert!(tab.split(Direction::LeftRight, pane(2)));
        assert!(tab.close_pane(pane(2)));
        assert_eq!(tab.focus(), pane(1));
    }

    #[test]
    fn closing_the_last_pane_ends_the_tab() {
        let mut tab = tab();
        assert!(!tab.close_pane(pane(1)));
    }

    #[test]
    fn a_new_tab_is_added_after_the_one_shown_and_shown_itself() {
        let mut workspace = Workspace::new(WorkspaceId::FIRST, "app", None, tab());
        workspace.add_tab(Tab::new(TabId::FIRST.next(), "shell", pane(2)));
        assert_eq!(workspace.active(), 1);
        assert_eq!(workspace.tabs().len(), 2);

        assert!(workspace.select(0));
        workspace.add_tab(Tab::new(TabId::FIRST.next().next(), "shell", pane(3)));
        assert_eq!(workspace.active(), 1, "inserted right after the active tab");
        assert_eq!(
            workspace.tab().map(Tab::focus),
            Some(pane(3)),
            "and shown straight away"
        );
    }

    #[test]
    fn selecting_a_tab_that_is_not_there_is_refused() {
        let mut workspace = Workspace::new(WorkspaceId::FIRST, "app", None, tab());
        assert!(!workspace.select(7));
        assert_eq!(workspace.active(), 0);
    }

    #[test]
    fn a_tab_whose_last_pane_closes_is_removed() {
        let mut workspace = Workspace::new(WorkspaceId::FIRST, "app", None, tab());
        workspace.add_tab(Tab::new(TabId::FIRST.next(), "shell", pane(2)));

        assert!(workspace.close_pane(pane(2)));
        assert_eq!(workspace.tabs().len(), 1);
        assert_eq!(workspace.active(), 0, "the shown tab still exists");

        assert!(!workspace.close_pane(pane(1)), "the workspace is over");
    }

    #[test]
    fn closing_a_pane_the_workspace_does_not_have_changes_nothing() {
        let mut workspace = Workspace::new(WorkspaceId::FIRST, "app", None, tab());
        assert!(workspace.close_pane(pane(9)));
        assert_eq!(workspace.panes(), vec![pane(1)]);
    }

    #[test]
    fn a_workspace_is_named_after_its_folder() {
        assert_eq!(name_for(Some(Path::new("/projects/my-app"))), "my-app");
        assert_eq!(name_for(Some(Path::new("/"))), UNNAMED);
        assert_eq!(name_for(None), UNNAMED);
    }

    #[test]
    fn a_repeated_name_gets_a_number() {
        let taken = vec!["app".to_owned(), "app 2".to_owned()];
        assert_eq!(unique_name("app", &taken), "app 3");
        assert_eq!(unique_name("other", &taken), "other");
    }
}
