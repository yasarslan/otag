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
use crate::pane::{PaneExit, PaneId, PaneSize};

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
    /// Attach to a pane, starting one if the daemon has none.
    ///
    /// Answered with [`Response::Attached`], after which the daemon pushes a
    /// [`Response::Frame`] whenever the pane's screen changes, until the
    /// client sends [`Request::Detach`] or the connection ends.
    Attach {
        /// Size of the terminal the client is drawing into.
        size: PaneSize,
        /// Directory a newly started pane should run in.
        #[serde(default)]
        cwd: Option<PathBuf>,
    },
    /// Write these bytes to the attached pane's input.
    ///
    /// Not answered: a key press is not worth a round trip, and the result
    /// is visible in the next frame.
    Input {
        /// Exactly what the program should receive.
        bytes: Vec<u8>,
    },
    /// The client's terminal changed size.
    ///
    /// Not answered; the next frame carries the new size.
    Resize {
        /// New size of the client's terminal.
        size: PaneSize,
    },
    /// Stop watching the pane and close the connection. Answered with
    /// [`Response::Ok`]. The pane and its program keep running.
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
    /// Answer to [`Request::Attach`], carrying the pane's whole screen.
    Attached {
        /// Pane the client is now watching.
        pane: PaneId,
        /// The pane's screen as it is now.
        frame: ScreenFrame,
    },
    /// The attached pane's screen changed.
    Frame {
        /// Pane the frame belongs to.
        pane: PaneId,
        /// What changed, relative to the last frame this client was sent.
        frame: ScreenFrame,
    },
    /// The attached pane's program ended. The last frame before this one
    /// shows what it left on the screen.
    PaneExited {
        /// Pane whose program ended.
        pane: PaneId,
        /// How it ended.
        exit: PaneExit,
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
    /// How its program ended, if it has.
    #[serde(default)]
    pub exit: Option<PaneExit>,
}

impl Response {
    /// Build an [`Response::Error`] from anything string-like.
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: message.into(),
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

    fn roundtrip_request(request: Request) {
        let line = encode_line(&request).expect("request encodes");
        assert!(line.ends_with('\n'));
        let decoded: Request = decode_line(&line).expect("request decodes");
        assert_eq!(decoded, request);
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
        let frame = ScreenFrame {
            size: PaneSize::new(2, 8),
            cursor: crate::frame::CursorState {
                row: 1,
                col: 3,
                visible: true,
                shape: crate::frame::CursorShape::SteadyBlock,
            },
            modes: crate::frame::InputModes {
                application_cursor: true,
                ..crate::frame::InputModes::default()
            },
            full: true,
            rows: vec![crate::frame::ScreenRow {
                index: 0,
                runs: vec![crate::frame::StyledRun {
                    text: "hi".to_owned(),
                    style: crate::frame::CellStyle::default(),
                }],
            }],
        };

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
                    exit: None,
                }],
            },
            Response::Panes {
                panes: vec![PaneInfo {
                    id: PaneId::FIRST,
                    size: PaneSize::DEFAULT,
                    exit: Some(PaneExit {
                        code: Some(0),
                        signal: None,
                    }),
                }],
            },
            Response::Attached {
                pane: PaneId::FIRST,
                frame: frame.clone(),
            },
            Response::Frame {
                pane: PaneId::FIRST,
                frame,
            },
            Response::PaneExited {
                pane: PaneId::FIRST,
                exit: PaneExit {
                    code: None,
                    signal: Some("SIGTERM".to_owned()),
                },
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
