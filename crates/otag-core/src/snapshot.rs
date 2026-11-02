//! What a session has to remember to come back as it was.
//!
//! A snapshot is everything about a session that is worth keeping and cannot
//! be worked out again: which workspaces and tabs there are, how each tab's
//! panes divide their space, where each pane was running, and what had
//! scrolled past on it.
//!
//! It is plain data with a version, serialized as JSON, so a daemon that has
//! been upgraded can still read what an older one wrote. Every field has a
//! default: a snapshot missing something must restore a session with that
//! part missing, never fail to restore at all.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::layout::Layout;
use crate::pane::{PaneId, PaneSize};
use crate::workspace::{TabId, WorkspaceId};

/// Version this build writes.
///
/// Raised when a change would be read wrongly rather than simply ignored by
/// an older build; a snapshot from a newer version is not restored.
pub const VERSION: u32 = 1;

/// How many lines of what a pane has shown a snapshot keeps for it.
pub const HISTORY_LINES: usize = 2_000;

/// A whole session, as it was when the snapshot was taken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Version of the build that wrote it.
    pub version: u32,
    /// Size of the terminal the session was last laid out for.
    #[serde(default)]
    pub size: PaneSize,
    /// Whether the sidebar was shown.
    #[serde(default = "yes")]
    pub sidebar: bool,
    /// Which workspace was shown.
    #[serde(default)]
    pub active_workspace: usize,
    /// The workspaces, in the order the sidebar listed them.
    #[serde(default)]
    pub workspaces: Vec<WorkspaceSnapshot>,
    /// Every pane, referred to by the layouts above.
    #[serde(default)]
    pub panes: Vec<PaneSnapshot>,
}

const fn yes() -> bool {
    true
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            version: VERSION,
            size: PaneSize::DEFAULT,
            sidebar: true,
            active_workspace: 0,
            workspaces: Vec::new(),
            panes: Vec::new(),
        }
    }
}

impl Snapshot {
    /// Whether this build can restore the snapshot.
    ///
    /// A snapshot from a newer otag may describe things this build has no
    /// way to put back, so it is left alone instead of half restored.
    #[must_use]
    pub const fn is_readable(&self) -> bool {
        self.version <= VERSION
    }

    /// Whether there is anything to restore.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.workspaces.is_empty() || self.panes.is_empty()
    }

    /// The pane with this identifier.
    #[must_use]
    pub fn pane(&self, id: PaneId) -> Option<&PaneSnapshot> {
        self.panes.iter().find(|pane| pane.id == id)
    }
}

/// One workspace and its tabs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    /// Which workspace it was.
    pub id: WorkspaceId,
    /// What the sidebar called it.
    #[serde(default)]
    pub name: String,
    /// The directory its panes started in.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// Which tab was shown.
    #[serde(default)]
    pub active_tab: usize,
    /// Its tabs, left to right.
    #[serde(default)]
    pub tabs: Vec<TabSnapshot>,
}

/// One tab, with the layout of its panes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabSnapshot {
    /// Which tab it was.
    pub id: TabId,
    /// What the tab bar called it.
    #[serde(default)]
    pub title: String,
    /// The pane that had the focus.
    pub focus: PaneId,
    /// How its panes divided its space.
    pub layout: Layout,
}

/// One pane: what it was running, and where.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSnapshot {
    /// Which pane it was. The layouts refer to panes by this.
    pub id: PaneId,
    /// What its border said it was running.
    #[serde(default)]
    pub title: String,
    /// The directory it was running in.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// The command to run to bring it back, empty for the user's shell.
    ///
    /// An agent pane is brought back with the command that resumes its
    /// session, which is why this is kept rather than worked out again.
    #[serde(default)]
    pub command: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Direction;

    fn snapshot() -> Snapshot {
        let mut layout = Layout::new(PaneId::FIRST);
        assert!(layout.split(PaneId::FIRST, Direction::LeftRight, PaneId::FIRST.next()));

        Snapshot {
            version: VERSION,
            size: PaneSize::new(30, 100),
            sidebar: false,
            active_workspace: 0,
            workspaces: vec![WorkspaceSnapshot {
                id: WorkspaceId::FIRST,
                name: "app".to_owned(),
                cwd: Some(PathBuf::from("/projects/app")),
                active_tab: 0,
                tabs: vec![TabSnapshot {
                    id: TabId::FIRST,
                    title: "shell".to_owned(),
                    focus: PaneId::FIRST.next(),
                    layout,
                }],
            }],
            panes: vec![
                PaneSnapshot {
                    id: PaneId::FIRST,
                    title: "shell".to_owned(),
                    cwd: Some(PathBuf::from("/projects/app")),
                    command: Vec::new(),
                },
                PaneSnapshot {
                    id: PaneId::FIRST.next(),
                    title: "claude".to_owned(),
                    cwd: Some(PathBuf::from("/projects/app")),
                    command: vec!["claude".to_owned(), "--resume".to_owned()],
                },
            ],
        }
    }

    #[test]
    fn a_snapshot_comes_back_exactly_as_it_was_written() {
        let snapshot = snapshot();
        let json = serde_json::to_string(&snapshot).expect("a snapshot encodes");
        let back: Snapshot = serde_json::from_str(&json).expect("a snapshot decodes");
        assert_eq!(back, snapshot);
    }

    #[test]
    fn a_snapshot_with_only_its_version_decodes_to_an_empty_one() {
        // Something has to be restorable from whatever is on disk, even a
        // file written by a build that knew less.
        let back: Snapshot = serde_json::from_str(r#"{"version":1}"#).expect("decodes");
        assert!(back.is_empty());
        assert!(back.is_readable());
        assert!(back.sidebar, "the sidebar is shown unless it was hidden");
        assert_eq!(back.size, PaneSize::DEFAULT);
    }

    #[test]
    fn a_snapshot_from_a_newer_otag_is_left_alone() {
        let newer = Snapshot {
            version: VERSION + 1,
            ..snapshot()
        };
        assert!(!newer.is_readable());
    }

    #[test]
    fn a_snapshot_finds_the_panes_its_layouts_refer_to() {
        let snapshot = snapshot();
        for pane in snapshot.workspaces[0].tabs[0].layout.panes() {
            assert!(
                snapshot.pane(pane).is_some(),
                "the layout refers to pane {pane}, which is not in the snapshot"
            );
        }
        assert!(snapshot.pane(PaneId::FIRST.next().next()).is_none());
    }

    #[test]
    fn a_snapshot_without_panes_has_nothing_to_restore() {
        assert!(Snapshot::default().is_empty());
        assert!(
            Snapshot {
                panes: Vec::new(),
                ..snapshot()
            }
            .is_empty(),
            "workspaces whose panes are gone are not worth restoring"
        );
    }
}
