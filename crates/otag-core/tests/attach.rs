//! Attaching to a pane over the socket: frames out, keys in.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use otag_core::client::Connection;
use otag_core::frame::ScreenState;
use otag_core::pane::{PaneSize, PaneSpec};
use otag_core::proto::{Request, Response};
use otag_core::server::{Handler, Outbox, Server};
use otag_core::session::Session;
use otag_core::{Error, attach};

const TIMEOUT: Duration = Duration::from_secs(10);

/// A daemon that runs one scripted program and serves clients attaching to it.
struct TestDaemon {
    session: Arc<Session>,
    spec: PaneSpec,
}

impl Handler for TestDaemon {
    async fn handle(&self, request: Request, outbox: &Outbox) -> Option<Response> {
        match request {
            Request::Ping => Some(Response::Pong),
            Request::Attach { size, .. } => {
                match attach::start(&self.session, outbox, &self.spec, size).await {
                    Ok(response) => Some(response),
                    Err(error) => Some(Response::error(error.to_string())),
                }
            }
            Request::Input { bytes } => {
                let pane = self.session.summaries().first().map(|pane| pane.id)?;
                if let Err(error) = attach::input(&self.session, pane, bytes).await {
                    tracing::debug!(%error, "input went nowhere");
                }
                None
            }
            Request::Resize { size } => {
                let pane = self.session.summaries().first().map(|pane| pane.id)?;
                let _ = attach::resize(&self.session, pane, size).await;
                None
            }
            Request::ListPanes => Some(Response::Panes {
                panes: self
                    .session
                    .summaries()
                    .into_iter()
                    .map(|pane| otag_core::proto::PaneInfo {
                        id: pane.id,
                        size: pane.size,
                        exit: pane.exit,
                    })
                    .collect(),
            }),
            _ => Some(Response::error("not supported in this test")),
        }
    }
}

/// Run a daemon on a throwaway socket for the duration of `body`.
async fn with_daemon<F, Fut>(script: &str, body: F)
where
    F: FnOnce(std::path::PathBuf) -> Fut,
    Fut: Future<Output = ()>,
{
    let directory = tempfile::tempdir().expect("a temporary directory");
    let socket = directory.path().join("otag.sock");

    let server = Server::bind(&socket).await.expect("the server binds");
    let stop = server.shutdown_handle();
    let daemon = TestDaemon {
        session: Arc::new(Session::new()),
        spec: PaneSpec::command(["/bin/sh", "-c", script]),
    };
    let serving = tokio::spawn(async move { server.run(daemon).await });

    body(socket).await;

    stop.shutdown();
    let _ = tokio::time::timeout(TIMEOUT, serving).await;
}

/// Read messages until the assembled screen contains `needle`.
async fn read_until(
    connection: &mut Connection,
    screen: &mut ScreenState,
    needle: &str,
) -> Vec<Response> {
    let mut seen = Vec::new();
    // What the client already has may be enough: a pane that is quiet sends
    // nothing, so waiting for a message first would wait forever.
    if screen.text().contains(needle) {
        return seen;
    }

    let result = tokio::time::timeout(TIMEOUT, async {
        loop {
            let response = connection.receive().await.expect("a message arrives");
            if let Response::Frame { frame, .. } = &response {
                screen.apply(frame);
            }
            let found = screen.text().contains(needle);
            seen.push(response);
            if found {
                return;
            }
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "never saw {needle:?}.\nscreen:\n{}\nmessages: {seen:#?}",
        screen.text()
    );
    seen
}

async fn attach_to(socket: &Path, size: PaneSize) -> (Connection, ScreenState) {
    let mut connection = Connection::connect(socket).await.expect("a connection");
    let response = connection
        .request(&Request::Attach { size, cwd: None })
        .await
        .expect("the attach is answered");

    let Response::Attached { frame, .. } = response else {
        panic!("expected to be attached, got {response:?}");
    };
    let mut screen = ScreenState::new(size);
    screen.apply(&frame);
    (connection, screen)
}

#[tokio::test]
async fn attaching_gives_the_whole_screen_and_then_what_changes() {
    with_daemon(
        "printf 'first line\\n'; sleep 0.3; printf 'second line\\n'; sleep 10",
        |socket| async move {
            let (mut connection, mut screen) = attach_to(&socket, PaneSize::new(10, 40)).await;

            read_until(&mut connection, &mut screen, "first line").await;
            let later = read_until(&mut connection, &mut screen, "second line").await;

            assert_eq!(screen.row_text(0), "first line");
            assert_eq!(screen.row_text(1), "second line");
            // What came after the attach are frames, not whole screens.
            assert!(
                later
                    .iter()
                    .all(|message| matches!(message, Response::Frame { .. })),
                "{later:#?}"
            );
        },
    )
    .await;
}

#[tokio::test]
async fn keys_sent_over_the_socket_reach_the_program() {
    with_daemon(
        "read line; printf 'you typed %s\\n' \"$line\"; sleep 10",
        |socket| async move {
            let (mut connection, mut screen) = attach_to(&socket, PaneSize::new(10, 40)).await;

            connection
                .send(&Request::Input {
                    bytes: b"abc\r".to_vec(),
                })
                .await
                .expect("the input is sent");

            read_until(&mut connection, &mut screen, "you typed abc").await;
        },
    )
    .await;
}

#[tokio::test]
async fn a_resize_reaches_the_program_and_comes_back_as_a_full_frame() {
    with_daemon(
        "while :; do stty size; sleep 1; done",
        |socket| async move {
            let (mut connection, mut screen) = attach_to(&socket, PaneSize::new(24, 80)).await;
            read_until(&mut connection, &mut screen, "24 80").await;

            connection
                .send(&Request::Resize {
                    size: PaneSize::new(30, 100),
                })
                .await
                .expect("the resize is sent");

            read_until(&mut connection, &mut screen, "30 100").await;
            assert_eq!(screen.size(), PaneSize::new(30, 100));
        },
    )
    .await;
}

#[tokio::test]
async fn the_end_of_a_program_is_reported_after_its_last_output() {
    with_daemon("printf 'goodbye\\n'; exit 7", |socket| async move {
        let (mut connection, mut screen) = attach_to(&socket, PaneSize::new(10, 40)).await;
        let messages = read_until(&mut connection, &mut screen, "goodbye").await;

        // The exit may already be in what we read, or arrive right after.
        let mut exit = messages.into_iter().find_map(|message| match message {
            Response::PaneExited { exit, .. } => Some(exit),
            _ => None,
        });
        while exit.is_none() {
            match tokio::time::timeout(TIMEOUT, connection.receive()).await {
                Ok(Ok(Response::PaneExited { exit: reported, .. })) => exit = Some(reported),
                Ok(Ok(_)) => {}
                _ => break,
            }
        }

        let exit = exit.expect("the program's end is reported");
        assert_eq!(exit.code, Some(7));
        assert!(screen.text().contains("goodbye"));
    })
    .await;
}

#[tokio::test]
async fn detaching_leaves_the_program_running() {
    with_daemon("printf 'still here\\n'; sleep 10", |socket| async move {
        let (mut connection, mut screen) = attach_to(&socket, PaneSize::new(10, 40)).await;
        read_until(&mut connection, &mut screen, "still here").await;

        let response = connection
            .request(&Request::Detach)
            .await
            .expect("the detach is answered");
        assert_eq!(response, Response::Ok);
        drop(connection);

        // Attaching again finds the same pane, with its screen intact.
        let (mut again, screen_again) = attach_to(&socket, PaneSize::new(10, 40)).await;
        assert_eq!(screen_again.row_text(0), "still here");

        let listing = again
            .request(&Request::ListPanes)
            .await
            .expect("the listing is answered");
        let Response::Panes { panes } = listing else {
            panic!("expected a pane listing, got {listing:?}");
        };
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].exit, None);
    })
    .await;
}

#[tokio::test]
async fn two_clients_attached_to_one_pane_both_see_it() {
    with_daemon("printf 'shared output\\n'; sleep 10", |socket| async move {
        let (mut first, mut first_screen) = attach_to(&socket, PaneSize::new(10, 40)).await;
        let (mut second, mut second_screen) = attach_to(&socket, PaneSize::new(10, 40)).await;

        read_until(&mut first, &mut first_screen, "shared output").await;
        read_until(&mut second, &mut second_screen, "shared output").await;

        // The second client typing is seen by the first.
        second
            .send(&Request::Input {
                bytes: b"typed by the second\r".to_vec(),
            })
            .await
            .expect("the input is sent");
        read_until(&mut first, &mut first_screen, "typed by the second").await;
    })
    .await;
}

#[tokio::test]
async fn a_client_that_goes_away_mid_stream_does_not_disturb_the_pane() {
    with_daemon(
        "i=0; while [ $i -lt 500 ]; do echo \"line $i\"; i=$((i+1)); done; printf 'finished\\n'; sleep 10",
        |socket| async move {
            // Attach and then stop reading entirely, which is what a client
            // being killed looks like to the daemon.
            let (abandoned, _) = attach_to(&socket, PaneSize::new(24, 80)).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
            drop(abandoned);

            // The pane kept running and a new client sees the whole result.
            let (mut connection, mut screen) = attach_to(&socket, PaneSize::new(24, 80)).await;
            read_until(&mut connection, &mut screen, "finished").await;
        },
    )
    .await;
}

#[tokio::test]
async fn a_request_the_daemon_does_not_know_is_an_error_not_a_disconnect() {
    with_daemon("sleep 10", |socket| async move {
        let mut connection = Connection::connect(&socket).await.expect("a connection");
        let response = connection
            .request(&Request::Unknown)
            .await
            .expect("an answer arrives");

        let Response::Error { message } = response else {
            panic!("expected an error, got {response:?}");
        };
        assert!(message.contains("does not support"), "{message}");

        // The connection still works.
        assert_eq!(
            connection.request(&Request::Ping).await.expect("an answer"),
            Response::Pong
        );
    })
    .await;
}

#[tokio::test]
async fn input_for_a_pane_that_is_gone_is_not_fatal() {
    let session = Session::new();
    let error = attach::input(&session, otag_core::pane::PaneId::FIRST, b"x".to_vec())
        .await
        .expect_err("there is no such pane");
    assert!(matches!(error, Error::PaneGone { .. }), "{error}");
    assert!(error.to_string().contains("no longer running"), "{error}");
}
