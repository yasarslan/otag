//! Watching a pane as a stream of bytes: the replay out, keys and sizes in.
//!
//! The test client does what the macOS app does — it feeds everything the
//! daemon sends into a terminal emulator of its own — so these tests check
//! the one thing that matters about raw mode: a client that applies the
//! replay and then the output ends up with the same screen the daemon has.

use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use otag_core::client::Connection;
use otag_core::pane::{PaneId, PaneSize, PaneSpec};
use otag_core::proto::{Bytes, Request, Response};
use otag_core::raw::Attachments;
use otag_core::screen::Screen;
use otag_core::server::{Handler, Outbox, Server};
use otag_core::session::Session;

const TIMEOUT: Duration = Duration::from_secs(10);

/// A program that announces itself, echoes one line, and then waits.
const SCRIPT: &str = "printf 'pane up\\n'; read line; printf 'typed %s\\n' \"$line\"; sleep 10";

/// A daemon that serves the raw-mode half of the protocol.
struct TestDaemon {
    session: Arc<Session>,
    attachments: Attachments,
}

impl Handler for TestDaemon {
    async fn handle(&self, request: Request, outbox: &Outbox) -> Option<Response> {
        match request {
            Request::Ping => Some(Response::Pong),
            Request::AttachPane { pane, size } => Some(
                match otag_core::raw::attach_pane(
                    &self.session,
                    &self.attachments,
                    outbox,
                    pane,
                    size,
                )
                .await
                {
                    Ok(response) => response,
                    Err(error) => Response::error(error.to_string()),
                },
            ),
            Request::DetachPane { pane } => {
                self.attachments.remove(outbox.id(), pane);
                Some(Response::Ok)
            }
            Request::PaneInput { pane, bytes } => {
                let _ = self.session.input_to(pane, bytes.into_vec()).await;
                None
            }
            Request::PaneResize { pane, size } => {
                let _ = self.session.resize_pane_to(pane, size).await;
                None
            }
            Request::ListPanes => Some(Response::Panes { panes: Vec::new() }),
            _ => Some(Response::error("not supported in this test")),
        }
    }
}

/// Run a daemon with one pane for the duration of `body`.
async fn with_pane<F, Fut>(size: PaneSize, body: F)
where
    F: FnOnce(std::path::PathBuf, PaneId) -> Fut,
    Fut: Future<Output = ()>,
{
    let directory = tempfile::tempdir().expect("a temporary directory");
    let socket = directory.path().join("otag.sock");

    let session = Session::new(PaneSpec::command(["/bin/sh", "-c", SCRIPT]));
    session.attach(size, None).await.expect("a pane");
    let pane = *session.visible_panes().first().expect("one pane");

    let server = Server::bind(&socket).await.expect("the server binds");
    let stop = server.shutdown_handle();
    let daemon = TestDaemon {
        session: Arc::clone(&session),
        attachments: Attachments::new(),
    };
    let serving = tokio::spawn(async move { server.run(daemon).await });

    body(socket, pane).await;

    stop.shutdown();
    let _ = tokio::time::timeout(TIMEOUT, serving).await;
}

/// A client that emulates the terminal itself, as the app does.
struct RawClient {
    connection: Connection,
    screen: Screen,
    size: PaneSize,
}

impl RawClient {
    /// Attach to one pane and apply the replay.
    async fn attach(socket: &Path, pane: PaneId, size: Option<PaneSize>) -> Self {
        let mut connection = Connection::connect(socket).await.expect("a connection");
        let response = connection
            .request(&Request::AttachPane { pane, size })
            .await
            .expect("the attach is answered");

        let Response::PaneScreen { size, bytes, .. } = &response else {
            panic!("expected the pane's screen, got {response:?}");
        };
        let mut screen = Screen::new(*size);
        screen.process(bytes.as_slice());

        Self {
            connection,
            screen,
            size: *size,
        }
    }

    /// Apply one message, the way the app hands bytes to its terminal.
    fn apply(&mut self, response: &Response) {
        match response {
            // A whole screen replaces what the client had.
            Response::PaneScreen { size, bytes, .. } => {
                self.size = *size;
                self.screen = Screen::new(*size);
                self.screen.process(bytes.as_slice());
            }
            Response::Output { bytes, .. } => self.screen.process(bytes.as_slice()),
            _ => {}
        }
    }

    /// What the client has on its screen.
    fn text(&self) -> String {
        self.screen.state().text()
    }

    async fn send(&mut self, request: &Request) {
        self.connection
            .send(request)
            .await
            .expect("the request is sent");
    }

    /// Read messages until `ready` holds, or fail saying what was seen.
    async fn read_until(&mut self, what: &str, ready: impl Fn(&Self) -> bool) -> Vec<Response> {
        let mut seen = Vec::new();
        if ready(self) {
            return seen;
        }

        let result = tokio::time::timeout(TIMEOUT, async {
            loop {
                let response = self.connection.receive().await.expect("a message arrives");
                self.apply(&response);
                let found = ready(self);
                seen.push(response);
                if found {
                    return;
                }
            }
        })
        .await;

        assert!(
            result.is_ok() && ready(self),
            "timed out waiting for {what}; the screen held:\n{}\nmessages: {seen:?}",
            self.text()
        );
        seen
    }

    /// Read messages for a while, without expecting anything in particular.
    async fn read_for(&mut self, duration: Duration) -> Vec<Response> {
        let mut seen = Vec::new();
        let _ = tokio::time::timeout(duration, async {
            while let Ok(response) = self.connection.receive().await {
                self.apply(&response);
                seen.push(response);
            }
        })
        .await;
        seen
    }

    /// Read until the daemon says the pane has ended.
    async fn read_until_ended(&mut self) -> Vec<Response> {
        let mut seen = Vec::new();
        let result = tokio::time::timeout(TIMEOUT, async {
            loop {
                let Ok(response) = self.connection.receive().await else {
                    // The connection closed without the news, which is a
                    // failure of its own and reported below.
                    return false;
                };
                self.apply(&response);
                let ended = matches!(response, Response::PaneEnded { .. });
                seen.push(response);
                if ended {
                    return true;
                }
            }
        })
        .await;

        assert!(
            result == Ok(true),
            "never heard that the pane ended; the screen held:\n{}\nmessages: {seen:?}",
            self.text()
        );
        seen
    }
}

#[tokio::test]
async fn attaching_in_raw_mode_replays_what_the_pane_already_showed() {
    with_pane(PaneSize::new(24, 80), |socket, pane| async move {
        // The output came before this client existed, so the only way it can
        // be on its screen is the replay.
        let mut client = RawClient::attach(&socket, pane, None).await;
        client
            .read_until("the pane's first line", |client| {
                client.text().contains("pane up")
            })
            .await;
    })
    .await;
}

#[tokio::test]
async fn what_the_program_writes_afterwards_arrives_as_output() {
    with_pane(PaneSize::new(24, 80), |socket, pane| async move {
        let mut client = RawClient::attach(&socket, pane, None).await;
        client
            .read_until("the pane's first line", |client| {
                client.text().contains("pane up")
            })
            .await;

        client
            .send(&Request::PaneInput {
                pane,
                bytes: Bytes(b"hello\r".to_vec()),
            })
            .await;

        let seen = client
            .read_until("the echoed line", |client| {
                client.text().contains("typed hello")
            })
            .await;
        assert!(
            seen.iter()
                .any(|response| matches!(response, Response::Output { .. })),
            "the program's own output arrives as output, not as a screen: {seen:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn the_size_a_raw_client_asks_for_is_the_size_the_pane_gets() {
    with_pane(PaneSize::new(24, 80), |socket, pane| async move {
        let wanted = PaneSize::new(30, 100);
        let client = RawClient::attach(&socket, pane, Some(wanted)).await;

        assert_eq!(
            client.size, wanted,
            "the replay describes the pane at the size the client asked for"
        );
    })
    .await;
}

#[tokio::test]
async fn a_resize_sends_the_screen_again_because_the_old_bytes_assumed_the_old_size() {
    with_pane(PaneSize::new(24, 80), |socket, pane| async move {
        let mut client = RawClient::attach(&socket, pane, None).await;
        client
            .read_until("the pane's first line", |client| {
                client.text().contains("pane up")
            })
            .await;

        let wanted = PaneSize::new(20, 60);
        client
            .send(&Request::PaneResize { pane, size: wanted })
            .await;

        client
            .read_until("the screen at the new size", |client| client.size == wanted)
            .await;
        assert!(
            client.text().contains("pane up"),
            "the replay carries what the pane still shows: {}",
            client.text()
        );
    })
    .await;
}

#[tokio::test]
async fn detaching_stops_the_output() {
    with_pane(PaneSize::new(24, 80), |socket, pane| async move {
        let mut client = RawClient::attach(&socket, pane, None).await;
        client
            .read_until("the pane's first line", |client| {
                client.text().contains("pane up")
            })
            .await;

        let response = client
            .connection
            .request(&Request::DetachPane { pane })
            .await
            .expect("the detach is answered");
        assert_eq!(response, Response::Ok);

        client
            .send(&Request::PaneInput {
                pane,
                bytes: Bytes(b"hello\r".to_vec()),
            })
            .await;

        // The program still echoes, but nothing is sent to this client any
        // more, so its screen stays as it was.
        let seen = client.read_for(Duration::from_millis(500)).await;
        assert!(
            !seen
                .iter()
                .any(|response| matches!(response, Response::Output { .. })),
            "a detached client is sent nothing: {seen:?}"
        );
        assert!(!client.text().contains("typed hello"), "{}", client.text());
    })
    .await;
}

#[tokio::test]
async fn a_pane_whose_program_ends_says_so() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let socket = directory.path().join("otag.sock");

    // A program that prints and ends at once, which is the case where the
    // last output must still arrive before the news that it ended.
    let session = Session::new(PaneSpec::command(["/bin/sh", "-c", "printf 'bye\\n'"]));
    session
        .attach(PaneSize::new(24, 80), None)
        .await
        .expect("a pane");
    let pane = *session.visible_panes().first().expect("one pane");

    let server = Server::bind(&socket).await.expect("the server binds");
    let stop = server.shutdown_handle();
    let daemon = TestDaemon {
        session: Arc::clone(&session),
        attachments: Attachments::new(),
    };
    let serving = tokio::spawn(async move { server.run(daemon).await });

    {
        let mut client = RawClient::attach(&socket, pane, None).await;
        client.read_until_ended().await;
        // Whatever the program printed last reaches the client before the
        // news that it ended.
        assert!(client.text().contains("bye"), "{}", client.text());
    }

    stop.shutdown();
    let _ = tokio::time::timeout(TIMEOUT, serving).await;
}
