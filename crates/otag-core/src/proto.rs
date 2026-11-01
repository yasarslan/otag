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

use serde::{Deserialize, Serialize};

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
    /// The request could not be carried out.
    Error {
        /// Human-readable description of what went wrong.
        message: String,
    },
    /// A response this build does not know about.
    #[serde(other)]
    Unknown,
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
    }

    #[test]
    fn responses_roundtrip() {
        for response in [
            Response::Pong,
            Response::Ok,
            Response::Version {
                version: "0.1.0".to_owned(),
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
