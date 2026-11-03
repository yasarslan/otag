//! The otag socket protocol.
//!
//! Clients and the daemon exchange newline-delimited JSON over a Unix domain
//! socket: one [`Request`] object per line, answered by one [`Response`] object
//! per line.
//!
//! The protocol is designed to grow. Both enums are internally tagged and carry
//! a catch-all variant, so a client and a daemon of different versions degrade
//! to a clear error instead of a parse failure, and unknown object fields are
//! ignored rather than rejected.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::frame::ScreenFrame;
use crate::layout::{Area, Direction, Side};
use crate::pane::{PaneExit, PaneId, PaneSize};
use crate::workspace::{TabId, WorkspaceId};
use otag_agents::{AgentKind, PaneStatus};

/// A command sent from a client to the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Check that the daemon is alive. Answered with [`Response::Pong`].
    Ping,
    /// Ask the daemon for its version. Answered with [`Response::Version`].
    Version,
    /// Ask the daemon to shut down. Answered with [`Response::Ok`] before the
    /// daemon stops accepting new connections.
    Shutdown,
    /// List the panes the daemon is running. Answered with
    /// [`Response::Panes`].
    ListPanes,
    /// Attach to the session, starting a workspace and a pane if the daemon
    /// has none.
    ///
    /// Answered with [`Response::View`], after which the daemon pushes a
    /// [`Response::Frame`] whenever a visible pane's screen changes and a new
    /// [`Response::View`] whenever the layout changes, until the client sends
    /// [`Request::Detach`] or the connection ends.
    Attach {
        /// Size of the terminal the client is drawing into.
        size: PaneSize,
        /// Directory a newly started workspace should run in.
        #[serde(default)]
        cwd: Option<PathBuf>,
    },
    /// Write these bytes to the focused pane's input.
    ///
    /// Not answered: a key press is not worth a round trip, and the result
    /// is visible in the next frame.
    Input {
        /// Exactly what the program should receive.
        bytes: Vec<u8>,
    },
    /// The client's terminal changed size.
    ///
    /// Not answered; the next view carries the new geometry.
    Resize {
        /// New size of the client's terminal.
        size: PaneSize,
    },
    /// Divide the focused pane and start a pane in the new half.
    Split {
        /// How the two halves are arranged.
        direction: Direction,
    },
    /// Add a tab with one pane to the active workspace, and show it.
    NewTab,
    /// Show the tab at this position in the active workspace.
    SelectTab {
        /// Position of the tab, counting from zero.
        index: usize,
    },
    /// Add a workspace with one tab and one pane, and show it.
    NewWorkspace {
        /// Directory the workspace's panes start in.
        #[serde(default)]
        cwd: Option<PathBuf>,
    },
    /// Show the workspace at this position.
    SelectWorkspace {
        /// Position of the workspace, counting from zero.
        index: usize,
    },
    /// Focus this pane, if the active tab holds it.
    FocusPane {
        /// The pane to type into.
        pane: PaneId,
    },
    /// Focus the pane on this side of the focused one.
    FocusSide {
        /// Which way to look.
        side: Side,
    },
    /// Focus the next pane in the active tab, wrapping around.
    FocusNext,
    /// Move the focused pane's edge on this side outwards.
    ResizePane {
        /// Which edge to move.
        side: Side,
        /// How far to move it, in cells.
        cells: u16,
    },
    /// Show or hide the workspace sidebar.
    ToggleSidebar,
    /// What an agent's hook, or its statusLine command, reported.
    ///
    /// Not answered: the hook command runs inside the agent's own turn and
    /// has to be gone again in milliseconds, so it writes this and exits
    /// without waiting.
    Hook {
        /// The event otag registered the command under, which is more
        /// trustworthy than the name inside the payload.
        event: String,
        /// The pane the agent is running in, from the environment otag puts
        /// its panes in. Missing when the agent was not started by otag.
        #[serde(default)]
        pane: Option<PaneId>,
        /// Exactly what the agent wrote on the command's standard input,
        /// carried as text so that this request stays comparable and the
        /// daemon decides how to read it.
        payload: String,
    },
    /// Stop watching the session and close the connection. Answered with
    /// [`Response::Ok`]. The panes and their programs keep running.
    Detach,
    /// A request this build does not know about.
    #[serde(other)]
    Unknown,
}

/// The daemon's answer to a [`Request`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// Answer to [`Request::Ping`].
    Pong,
    /// The request was carried out and has no payload.
    Ok,
    /// Answer to [`Request::Version`].
    Version {
        /// Version of the running daemon.
        version: String,
    },
    /// Answer to [`Request::ListPanes`].
    Panes {
        /// Every pane the daemon is running.
        panes: Vec<PaneInfo>,
    },
    /// Everything the client has to draw.
    ///
    /// Sent as the answer to [`Request::Attach`] and again whenever the
    /// layout changes, carrying the whole screen of every visible pane.
    View {
        /// What the session looks like now.
        view: Box<SessionView>,
    },
    /// A visible pane's screen changed.
    Frame {
        /// Pane the frame belongs to.
        pane: PaneId,
        /// What changed, relative to the last frame this client was sent.
        frame: ScreenFrame,
    },
    /// The session has no panes left, so there is nothing more to draw.
    ///
    /// Carries how the last pane's program ended, when that is known, so a
    /// client can report a failure whose output has scrolled away.
    SessionEnded {
        /// How the last program ended.
        #[serde(default)]
        exit: Option<PaneExit>,
    },
    /// The request could not be carried out.
    Error {
        /// Human-readable description of what went wrong.
        message: String,
    },
    /// A response this build does not know about.
    #[serde(other)]
    Unknown,
}

/// What the daemon reports about one pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInfo {
    /// Which pane this is.
    pub id: PaneId,
    /// Its current size.
    pub size: PaneSize,
    /// Name of the workspace it belongs to.
    #[serde(default)]
    pub workspace: String,
    /// What it is running.
    #[serde(default)]
    pub title: String,
    /// The agent otag has noticed in it.
    #[serde(default)]
    pub agent: Option<AgentKind>,
    /// What that agent is doing.
    #[serde(default)]
    pub status: PaneStatus,
    /// The agent session otag would resume.
    #[serde(default)]
    pub session: Option<String>,
    /// How its program ended, if it has.
    #[serde(default)]
    pub exit: Option<PaneExit>,
}

/// One workspace, as the sidebar shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceView {
    /// Which workspace this is.
    pub id: WorkspaceId,
    /// What the sidebar calls it.
    pub name: String,
    /// How many tabs it has.
    pub tabs: usize,
    /// How many panes it has, across all its tabs.
    pub panes: usize,
    /// What its agents are doing, taken from the pane that most wants the
    /// user.
    #[serde(default)]
    pub status: PaneStatus,
}

/// One tab of the active workspace, as the tab bar shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabView {
    /// Which tab this is.
    pub id: TabId,
    /// What the tab bar calls it.
    pub title: String,
    /// How many panes it holds.
    pub panes: usize,
    /// What its agents are doing, taken from the pane that most wants the
    /// user.
    #[serde(default)]
    pub status: PaneStatus,
}

/// One visible pane: where it is, and what is on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneView {
    /// Which pane this is.
    pub id: PaneId,
    /// The rectangle it occupies, border included.
    pub area: Area,
    /// What its border says it is running.
    #[serde(default)]
    pub title: String,
    /// The agent otag has noticed in it.
    #[serde(default)]
    pub agent: Option<AgentKind>,
    /// What that agent is doing.
    #[serde(default)]
    pub status: PaneStatus,
    /// Its whole screen, when the daemon is sending a full view.
    #[serde(default)]
    pub frame: Option<ScreenFrame>,
    /// How its program ended, if it has.
    #[serde(default)]
    pub exit: Option<PaneExit>,
}

/// Everything a client draws: the furniture, the layout, and the panes.
///
/// The daemon computes the geometry, including where its own sidebar and tab
/// bar go, because it is also what tells each pane's program how large it is.
/// Two clients of one session therefore cannot disagree about where a pane
/// is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionView {
    /// Counter of structural changes, for logs and tests.
    #[serde(default)]
    pub generation: u64,
    /// Size of the terminal this view was computed for.
    pub size: PaneSize,
    /// Where the workspace sidebar goes; empty when it is hidden.
    #[serde(default)]
    pub sidebar: Area,
    /// Where the tab bar goes.
    #[serde(default)]
    pub tabs_area: Area,
    /// The space the panes divide between them.
    #[serde(default)]
    pub content: Area,
    /// Every workspace, in the order the sidebar lists them.
    #[serde(default)]
    pub workspaces: Vec<WorkspaceView>,
    /// Which workspace is shown, as an index into `workspaces`.
    #[serde(default)]
    pub active_workspace: usize,
    /// The active workspace's tabs, left to right.
    #[serde(default)]
    pub tabs: Vec<TabView>,
    /// Which tab is shown, as an index into `tabs`.
    #[serde(default)]
    pub active_tab: usize,
    /// The panes of the active tab.
    #[serde(default)]
    pub panes: Vec<PaneView>,
    /// The pane the user is typing into.
    #[serde(default)]
    pub focus: Option<PaneId>,
}

impl SessionView {
    /// The visible pane at this cell, if there is one.
    ///
    /// Used to turn a click into a focus change; a click on a pane's border
    /// counts as a click on the pane, because the border is part of what the
    /// user sees as that pane.
    #[must_use]
    pub fn pane_at(&self, x: u16, y: u16) -> Option<PaneId> {
        self.panes
            .iter()
            .find(|pane| pane.area.contains(x, y))
            .map(|pane| pane.id)
    }

    /// Where one visible pane is.
    #[must_use]
    pub fn pane(&self, id: PaneId) -> Option<&PaneView> {
        self.panes.iter().find(|pane| pane.id == id)
    }

    /// The pane the user is typing into.
    #[must_use]
    pub fn focused(&self) -> Option<&PaneView> {
        self.focus.and_then(|focus| self.pane(focus))
    }

    /// Whether the session has nothing left to draw.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.panes.is_empty()
    }
}

impl Response {
    /// Build an [`Response::Error`] from anything string-like.
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: message.into(),
        }
    }

    /// Build a [`Response::View`] from a view.
    #[must_use]
    pub fn view(view: SessionView) -> Self {
        Self::View {
            view: Box::new(view),
        }
    }
}

/// Encode a value as a single protocol line, including the trailing newline.
pub fn encode_line<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    Ok(line)
}

/// Decode a single protocol line. The trailing newline is optional.
pub fn decode_line<T: for<'de> Deserialize<'de>>(line: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(line.trim_end_matches(['\n', '\r']))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{CellStyle, CursorShape, CursorState, InputModes, ScreenRow, StyledRun};

    fn roundtrip_request(request: Request) {
        let line = encode_line(&request).expect("request encodes");
        assert!(line.ends_with('\n'));
        let decoded: Request = decode_line(&line).expect("request decodes");
        assert_eq!(decoded, request);
    }

    fn frame() -> ScreenFrame {
        ScreenFrame {
            size: PaneSize::new(2, 8),
            cursor: CursorState {
                row: 1,
                col: 3,
                visible: true,
                shape: CursorShape::SteadyBlock,
            },
            modes: InputModes {
                application_cursor: true,
                ..InputModes::default()
            },
            full: true,
            rows: vec![ScreenRow {
                index: 0,
                runs: vec![StyledRun {
                    text: "hi".to_owned(),
                    style: CellStyle::default(),
                }],
            }],
        }
    }

    fn view() -> SessionView {
        SessionView {
            generation: 7,
            size: PaneSize::new(24, 80),
            sidebar: Area::new(0, 0, 16, 24),
            tabs_area: Area::new(16, 0, 64, 1),
            content: Area::new(16, 1, 64, 23),
            workspaces: vec![WorkspaceView {
                id: WorkspaceId::FIRST,
                name: "app".to_owned(),
                tabs: 1,
                panes: 1,
                status: PaneStatus::Working,
            }],
            active_workspace: 0,
            tabs: vec![TabView {
                id: TabId::FIRST,
                title: "shell".to_owned(),
                panes: 1,
                status: PaneStatus::Working,
            }],
            active_tab: 0,
            panes: vec![PaneView {
                id: PaneId::FIRST,
                area: Area::new(16, 1, 64, 23),
                title: "claude".to_owned(),
                agent: Some(AgentKind::Claude),
                status: PaneStatus::Working,
                frame: Some(frame()),
                exit: None,
            }],
            focus: Some(PaneId::FIRST),
        }
    }

    #[test]
    fn requests_roundtrip() {
        roundtrip_request(Request::Ping);
        roundtrip_request(Request::Version);
        roundtrip_request(Request::Shutdown);
        roundtrip_request(Request::ListPanes);
        roundtrip_request(Request::Detach);
        roundtrip_request(Request::Attach {
            size: PaneSize::new(30, 100),
            cwd: None,
        });
        roundtrip_request(Request::Attach {
            size: PaneSize::DEFAULT,
            cwd: Some(PathBuf::from("/some/project")),
        });
        roundtrip_request(Request::Input {
            bytes: vec![0x1b, b'[', b'A'],
        });
        roundtrip_request(Request::Resize {
            size: PaneSize::new(24, 80),
        });
        roundtrip_request(Request::Split {
            direction: Direction::LeftRight,
        });
        roundtrip_request(Request::Split {
            direction: Direction::TopBottom,
        });
        roundtrip_request(Request::NewTab);
        roundtrip_request(Request::SelectTab { index: 3 });
        roundtrip_request(Request::NewWorkspace { cwd: None });
        roundtrip_request(Request::NewWorkspace {
            cwd: Some(PathBuf::from("/other/project")),
        });
        roundtrip_request(Request::SelectWorkspace { index: 1 });
        roundtrip_request(Request::FocusPane {
            pane: PaneId::FIRST,
        });
        roundtrip_request(Request::FocusSide { side: Side::Left });
        roundtrip_request(Request::FocusNext);
        roundtrip_request(Request::ResizePane {
            side: Side::Down,
            cells: 2,
        });
        roundtrip_request(Request::ToggleSidebar);
    }

    #[test]
    fn attaching_without_a_directory_is_allowed() {
        let decoded: Request =
            decode_line(r#"{"type":"attach","size":{"rows":10,"cols":20}}"#).expect("decodes");
        assert_eq!(
            decoded,
            Request::Attach {
                size: PaneSize::new(10, 20),
                cwd: None,
            }
        );
    }

    #[test]
    fn an_attach_with_an_impossible_size_is_clamped_not_rejected() {
        let decoded: Request =
            decode_line(r#"{"type":"attach","size":{"rows":0,"cols":0}}"#).expect("decodes");
        assert_eq!(
            decoded,
            Request::Attach {
                size: PaneSize::new(1, 1),
                cwd: None,
            }
        );
    }

    #[test]
    fn responses_roundtrip() {
        for response in [
            Response::Pong,
            Response::Ok,
            Response::Version {
                version: "0.1.0".to_owned(),
            },
            Response::Panes {
                panes: vec![PaneInfo {
                    id: PaneId::FIRST,
                    size: PaneSize::DEFAULT,
                    workspace: "app".to_owned(),
                    title: "shell".to_owned(),
                    agent: None,
                    status: PaneStatus::Idle,
                    session: None,
                    exit: None,
                }],
            },
            Response::Panes {
                panes: vec![PaneInfo {
                    id: PaneId::FIRST,
                    size: PaneSize::DEFAULT,
                    workspace: "app".to_owned(),
                    title: "shell".to_owned(),
                    agent: Some(AgentKind::Claude),
                    status: PaneStatus::Working,
                    session: Some("a-session".to_owned()),
                    exit: Some(PaneExit {
                        code: Some(0),
                        signal: None,
                    }),
                }],
            },
            Response::view(view()),
            Response::Frame {
                pane: PaneId::FIRST,
                frame: frame(),
            },
            Response::SessionEnded { exit: None },
            Response::SessionEnded {
                exit: Some(PaneExit {
                    code: None,
                    signal: Some("SIGTERM".to_owned()),
                }),
            },
            Response::error("boom"),
        ] {
            let line = encode_line(&response).expect("response encodes");
            let decoded: Response = decode_line(&line).expect("response decodes");
            assert_eq!(decoded, response);
        }
    }

    #[test]
    fn request_tags_are_snake_case() {
        let line = encode_line(&Request::Ping).expect("encodes");
        assert_eq!(line, "{\"type\":\"ping\"}\n");
        let line = encode_line(&Request::NewTab).expect("encodes");
        assert_eq!(line, "{\"type\":\"new_tab\"}\n");
    }

    #[test]
    fn a_pane_listing_tolerates_a_missing_exit() {
        let decoded: Response =
            decode_line(r#"{"type":"panes","panes":[{"id":1,"size":{"rows":5,"cols":5}}]}"#)
                .expect("decodes");
        let Response::Panes { panes } = decoded else {
            panic!("expected a pane listing");
        };
        assert_eq!(panes[0].exit, None);
        assert_eq!(panes[0].title, "");
    }

    #[test]
    fn a_view_tolerates_everything_optional_being_absent() {
        // A daemon that has nothing to show yet, or an older one, must not
        // put the client into a broken state.
        let decoded: Response =
            decode_line(r#"{"type":"view","view":{"size":{"rows":5,"cols":9}}}"#).expect("decodes");
        let Response::View { view } = decoded else {
            panic!("expected a view");
        };
        assert_eq!(view.size, PaneSize::new(5, 9));
        assert!(view.is_empty());
        assert_eq!(view.focus, None);
        assert_eq!(view.generation, 0);
    }

    #[test]
    fn an_unknown_direction_or_side_decodes_rather_than_failing() {
        let decoded: Request =
            decode_line(r#"{"type":"split","direction":"sideways"}"#).expect("decodes");
        assert_eq!(
            decoded,
            Request::Split {
                direction: Direction::Unknown
            }
        );
        let decoded: Request =
            decode_line(r#"{"type":"focus_side","side":"inwards"}"#).expect("decodes");
        assert_eq!(
            decoded,
            Request::FocusSide {
                side: Side::Unknown
            }
        );
    }

    #[test]
    fn a_view_finds_the_pane_under_a_click() {
        let view = view();
        assert_eq!(view.pane_at(20, 5), Some(PaneId::FIRST));
        assert_eq!(view.pane_at(0, 0), None, "the sidebar is not a pane");
        assert_eq!(view.focused().map(|pane| pane.id), Some(PaneId::FIRST));
    }

    #[test]
    fn unknown_request_type_decodes_to_unknown() {
        let decoded: Request = decode_line("{\"type\":\"teleport\"}").expect("decodes");
        assert_eq!(decoded, Request::Unknown);
    }

    #[test]
    fn unknown_response_type_decodes_to_unknown() {
        let decoded: Response = decode_line("{\"type\":\"teleported\"}").expect("decodes");
        assert_eq!(decoded, Response::Unknown);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let decoded: Request =
            decode_line("{\"type\":\"ping\",\"from_the_future\":42}").expect("decodes");
        assert_eq!(decoded, Request::Ping);
    }

    #[test]
    fn carriage_return_is_tolerated() {
        let decoded: Request = decode_line("{\"type\":\"ping\"}\r\n").expect("decodes");
        assert_eq!(decoded, Request::Ping);
    }

    #[test]
    fn truncated_line_is_an_error_not_a_panic() {
        assert!(decode_line::<Request>("{\"type\":\"pi").is_err());
        assert!(decode_line::<Request>("").is_err());
        assert!(decode_line::<Request>("not json at all").is_err());
    }
}
