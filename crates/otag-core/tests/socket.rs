//! End-to-end tests for the daemon socket: a real server on a real socket in a
//! temporary directory, driven by the real client.

use std::path::{Path, PathBuf};
use std::time::Duration;

use otag_core::client;
use otag_core::proto::{Request, Response};
use otag_core::server::{Server, ShutdownHandle};
use tempfile::TempDir;
use tokio::task::JoinHandle;

/// Answers the requests the daemon handles in milestone M0.
fn handler(request: Request) -> Response {
    match request {
        Request::Ping => Response::Pong,
        Request::Version => Response::Version {
            version: otag_core::VERSION.to_owned(),
        },
        other => Response::error(format!("unsupported request: {other:?}")),
    }
}

/// A server running in the background on a socket inside a temporary directory.
struct TestServer {
    _dir: TempDir,
    path: PathBuf,
    shutdown: ShutdownHandle,
    task: JoinHandle<otag_core::Result<()>>,
}

impl TestServer {
    async fn start() -> Self {
        let dir = TempDir::new().expect("temporary directory");
        let path = dir.path().join("otag.sock");
        let server = Server::bind(&path).await.expect("server binds");
        let shutdown = server.shutdown_handle();
        let task = tokio::spawn(server.run(handler));
        client::wait_until_ready(&path, Duration::from_secs(5))
            .await
            .expect("server becomes ready");
        Self {
            _dir: dir,
            path,
            shutdown,
            task,
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    async fn stop(self) {
        self.shutdown.shutdown();
        self.task
            .await
            .expect("server task joins")
            .expect("server exits cleanly");
    }
}

#[tokio::test]
async fn ping_is_answered_with_pong() {
    let server = TestServer::start().await;
    let response = client::request(server.path(), &Request::Ping)
        .await
        .expect("ping succeeds");
    assert_eq!(response, Response::Pong);
    server.stop().await;
}

#[tokio::test]
async fn version_reports_the_running_daemon() {
    let server = TestServer::start().await;
    let response = client::request(server.path(), &Request::Version)
        .await
        .expect("version succeeds");
    assert_eq!(
        response,
        Response::Version {
            version: otag_core::VERSION.to_owned()
        }
    );
    server.stop().await;
}

#[tokio::test]
async fn one_connection_carries_several_requests() {
    let server = TestServer::start().await;
    let mut connection = client::Connection::connect(server.path())
        .await
        .expect("connects");
    for _ in 0..3 {
        assert_eq!(
            connection.request(&Request::Ping).await.expect("ping"),
            Response::Pong
        );
    }
    server.stop().await;
}

#[tokio::test]
async fn concurrent_clients_are_all_answered() {
    let server = TestServer::start().await;
    let mut clients = Vec::new();
    for _ in 0..8 {
        let path = server.path().to_path_buf();
        clients.push(tokio::spawn(async move {
            client::request(&path, &Request::Ping).await
        }));
    }
    for client in clients {
        let response = client
            .await
            .expect("client task joins")
            .expect("ping succeeds");
        assert_eq!(response, Response::Pong);
    }
    server.stop().await;
}

#[tokio::test]
async fn shutdown_is_acknowledged_and_stops_the_server() {
    let dir = TempDir::new().expect("temporary directory");
    let path = dir.path().join("otag.sock");
    let server = Server::bind(&path).await.expect("server binds");
    let task = tokio::spawn(server.run(handler));
    client::wait_until_ready(&path, Duration::from_secs(5))
        .await
        .expect("server becomes ready");

    let response = client::request(&path, &Request::Shutdown)
        .await
        .expect("shutdown succeeds");
    assert_eq!(response, Response::Ok);

    task.await
        .expect("server task joins")
        .expect("server exits cleanly");
    assert!(
        !path.exists(),
        "the socket is removed when the daemon stops"
    );
    assert!(!client::is_running(&path).await);
}

#[tokio::test]
async fn a_malformed_line_does_not_drop_the_connection() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let server = TestServer::start().await;
    let stream = tokio::net::UnixStream::connect(server.path())
        .await
        .expect("connects");
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    write_half
        .write_all(b"this is not json\n")
        .await
        .expect("writes");
    let line = lines
        .next_line()
        .await
        .expect("reads")
        .expect("gets an answer");
    let response: Response = otag_core::proto::decode_line(&line).expect("decodes");
    assert!(
        matches!(response, Response::Error { .. }),
        "got {response:?}"
    );

    write_half
        .write_all(b"{\"type\":\"ping\"}\n")
        .await
        .expect("writes");
    let line = lines
        .next_line()
        .await
        .expect("reads")
        .expect("gets an answer");
    assert_eq!(
        otag_core::proto::decode_line::<Response>(&line).expect("decodes"),
        Response::Pong
    );

    server.stop().await;
}

#[tokio::test]
async fn an_unknown_request_gets_an_actionable_error() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let server = TestServer::start().await;
    let stream = tokio::net::UnixStream::connect(server.path())
        .await
        .expect("connects");
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    write_half
        .write_all(b"{\"type\":\"from_a_newer_client\"}\n")
        .await
        .expect("writes");
    let line = lines
        .next_line()
        .await
        .expect("reads")
        .expect("gets an answer");
    match otag_core::proto::decode_line::<Response>(&line).expect("decodes") {
        Response::Error { message } => {
            assert!(message.contains("kill-server"), "message: {message}");
        }
        other => panic!("expected an error response, got {other:?}"),
    }

    server.stop().await;
}

#[tokio::test]
async fn a_stale_socket_file_is_reclaimed() {
    let dir = TempDir::new().expect("temporary directory");
    let path = dir.path().join("otag.sock");
    std::fs::write(&path, b"left over by a crashed daemon").expect("writes the stale file");

    let server = Server::bind(&path).await.expect("server binds");
    let shutdown = server.shutdown_handle();
    let task = tokio::spawn(server.run(handler));
    client::wait_until_ready(&path, Duration::from_secs(5))
        .await
        .expect("server becomes ready");

    shutdown.shutdown();
    task.await
        .expect("server task joins")
        .expect("server exits cleanly");
}

#[tokio::test]
async fn binding_twice_reports_that_a_daemon_is_already_running() {
    let server = TestServer::start().await;
    let error = Server::bind(server.path())
        .await
        .expect_err("the second bind fails");
    assert!(
        matches!(error, otag_core::Error::AlreadyRunning { .. }),
        "got {error:?}"
    );
    server.stop().await;
}

#[tokio::test]
async fn the_socket_is_accessible_to_the_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let server = TestServer::start().await;
    let mode = std::fs::metadata(server.path())
        .expect("socket metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "socket mode is {mode:o}");
    server.stop().await;
}

#[tokio::test]
async fn connecting_without_a_daemon_says_how_to_start_one() {
    let dir = TempDir::new().expect("temporary directory");
    let path = dir.path().join("otag.sock");
    let error = client::request(&path, &Request::Ping)
        .await
        .expect_err("there is no daemon");
    assert!(
        matches!(error, otag_core::Error::NotRunning { .. }),
        "got {error:?}"
    );
    assert!(error.to_string().contains("Run `otag`"), "{error}");
}

#[tokio::test]
async fn waiting_for_a_daemon_that_never_starts_times_out() {
    let dir = TempDir::new().expect("temporary directory");
    let path = dir.path().join("otag.sock");
    let error = client::wait_until_ready(&path, Duration::from_millis(100))
        .await
        .expect_err("no daemon appears");
    assert!(matches!(error, otag_core::Error::NotRunning { .. }));
}

#[tokio::test]
async fn a_shutdown_requested_before_serving_still_stops_the_server() {
    let dir = TempDir::new().expect("temporary directory");
    let path = dir.path().join("otag.sock");
    let server = Server::bind(&path).await.expect("server binds");
    let shutdown = server.shutdown_handle();

    // Nothing is polling the server yet; the request must not be lost.
    shutdown.shutdown();

    tokio::spawn(server.run(handler))
        .await
        .expect("server task joins")
        .expect("server exits cleanly");
}

#[tokio::test]
async fn a_shutdown_racing_an_incoming_connection_is_not_lost() {
    for _ in 0..20 {
        let server = TestServer::start().await;
        // A liveness probe just like the one `Server::bind` performs, so the
        // accept loop is busy at the moment shutdown is requested.
        let _probe = tokio::net::UnixStream::connect(server.path())
            .await
            .expect("probe connects");
        server.stop().await;
    }
}
