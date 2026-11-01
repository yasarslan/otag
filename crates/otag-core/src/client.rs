//! Talking to the daemon from a client process.
//!
//! Every function here takes the socket path explicitly, so callers decide
//! where it comes from (see [`crate::paths`]) and tests can point at a
//! throwaway socket without touching the environment.

use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::proto::{self, Request, Response};
use crate::{Error, Result};

/// How often [`wait_until_ready`] retries while a daemon is starting up.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// An open connection to the daemon, able to carry several requests.
#[derive(Debug)]
pub struct Connection {
    lines: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    write_half: tokio::net::unix::OwnedWriteHalf,
}

impl Connection {
    /// Connect to the daemon listening on `path`.
    ///
    /// Returns [`Error::NotRunning`] when nothing is listening, which is the
    /// normal case before the daemon has been started.
    pub async fn connect(path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(path)
            .await
            .map_err(|source| match source.kind() {
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                    Error::NotRunning {
                        path: path.to_path_buf(),
                    }
                }
                _ => Error::io(
                    format!("cannot connect to the otag socket {path:?}"),
                    source,
                ),
            })?;
        let (read_half, write_half) = stream.into_split();
        Ok(Self {
            lines: BufReader::new(read_half).lines(),
            write_half,
        })
    }

    /// Send one request and read its response.
    pub async fn request(&mut self, request: &Request) -> Result<Response> {
        let encoded = proto::encode_line(request)?;
        self.write_half
            .write_all(encoded.as_bytes())
            .await
            .map_err(|source| Error::io("cannot send a request to the otag daemon", source))?;
        self.write_half
            .flush()
            .await
            .map_err(|source| Error::io("cannot send a request to the otag daemon", source))?;

        let line = self
            .lines
            .next_line()
            .await
            .map_err(|source| Error::io("cannot read the otag daemon's answer", source))?
            .ok_or(Error::ConnectionClosed)?;
        Ok(proto::decode_line(&line)?)
    }
}

/// Connect, send a single request, and return the response.
pub async fn request(path: &Path, request: &Request) -> Result<Response> {
    Connection::connect(path).await?.request(request).await
}

/// Whether a daemon answers a ping on `path`.
pub async fn is_running(path: &Path) -> bool {
    matches!(request(path, &Request::Ping).await, Ok(Response::Pong))
}

/// Wait until a daemon answers a ping on `path`, giving up after `timeout`.
///
/// Used after spawning the daemon, which needs a moment to bind its socket.
pub async fn wait_until_ready(path: &Path, timeout: Duration) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if is_running(path).await {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Error::NotRunning {
                path: path.to_path_buf(),
            });
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
