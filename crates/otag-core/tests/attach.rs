//! Attaching to a session over the socket: views and frames out, keys and
//! commands in.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use otag_core::client::Connection;
use otag_core::frame::ScreenState;
use otag_core::layout::{BORDER, Chrome, Direction, Side};
use otag_core::pane::{PaneId, PaneSize, PaneSpec};
use otag_core::proto::{PaneInfo, Request, Response, SessionView};
use otag_core::server::{Handler, Outbox, Server};
use otag_core::session::Session;
use otag_core::{Error, attach};

const TIMEOUT: Duration = Duration::from_secs(10);

/// A program that announces itself, repeats one line, and then waits.
///
/// Every pane of a test daemon runs it, so a test can tell panes apart by
/// typing into the focused one.
const SCRIPT: &str = "printf 'pane up\\n'; read line; printf 'typed %s\\n' \"$line\"; sleep 10";

/// A daemon serving one session, answering everything a client can ask.
struct TestDaemon {
    session: Arc<Session>,
}

impl Handler for TestDaemon {
    async fn handle(&self, request: Request, outbox: &Outbox) -> Option<Response> {
        match request {
            Request::Ping => Some(Response::Pong),
            Request::Attach { size, cwd } => {
                match attach::start(&self.session, outbox, size, cwd.as_deref()).await {
                    Ok(response) => Some(response),
                    Err(error) => Some(Response::error(error.to_string())),
                }
            }
            Request::Input { bytes } => {
                if let Err(error) = self.session.input(bytes.into_vec()).await {
                    tracing::debug!(%error, "input went nowhere");
                }
                None
            }
            Request::Resize { size } => {
                self.session.resize(size).await;
                None
            }
            Request::Split { direction } => self.session.split(direction).await.err().map(error),
            Request::NewTab => self.session.new_tab().await.err().map(error),
            Request::SelectTab { index } => {
                self.session.select_tab(index).await;
                None
            }
            Request::NewWorkspace { cwd } => self
                .session
                .new_workspace(cwd.as_deref())
                .await
                .err()
                .map(error),
            Request::SelectWorkspace { index } => {
                self.session.select_workspace(index).await;
                None
            }
            Request::FocusPane { pane } => {
                self.session.focus_pane(pane).await;
                None
            }
            Request::FocusSide { side } => {
                self.session.focus_side(side).await;
                None
            }
            Request::FocusNext => {
                self.session.focus_next().await;
                None
            }
            Request::ResizePane { side, cells } => {
                self.session.resize_pane(side, cells).await;
                None
            }
            Request::ToggleSidebar => {
                self.session.toggle_sidebar().await;
                None
            }
            Request::ListPanes => Some(Response::Panes {
                panes: self
                    .session
                    .summaries()
                    .into_iter()
                    .map(|pane| PaneInfo {
                        id: pane.id,
                        size: pane.size,
                        workspace: pane.workspace,
                        title: pane.title,
                        agent: pane.agent,
                        status: pane.status,
                        session: pane.session,
                        tokens: pane.tokens,
                        exit: pane.exit,
                    })
                    .collect(),
            }),
            _ => Some(Response::error("not supported in this test")),
        }
    }
}

fn error(error: Error) -> Response {
    Response::error(error.to_string())
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
        session: Session::new(PaneSpec::command(["/bin/sh", "-c", script])),
    };
    let serving = tokio::spawn(async move { server.run(daemon).await });

    body(socket).await;

    stop.shutdown();
    let _ = tokio::time::timeout(TIMEOUT, serving).await;
}

/// The inside of a pane filling the whole content area of a terminal.
fn whole_pane(size: PaneSize, sidebar: bool) -> PaneSize {
    Chrome::of(size, sidebar).content.inset(BORDER).size()
}

/// A client that keeps the view and the screens it has been sent, the way
/// otag's own client does.
struct TestClient {
    connection: Connection,
    view: SessionView,
    screens: HashMap<PaneId, ScreenState>,
}

impl TestClient {
    async fn attach(socket: &Path, size: PaneSize) -> Self {
        let mut connection = Connection::connect(socket).await.expect("a connection");
        let response = connection
            .request(&Request::Attach { size, cwd: None })
            .await
            .expect("the attach is answered");

        let Response::View { view } = response else {
            panic!("expected a view, got {response:?}");
        };
        let mut client = Self {
            connection,
            view: SessionView::default(),
            screens: HashMap::new(),
        };
        client.take_view(*view);
        client
    }

    fn take_view(&mut self, view: SessionView) {
        let mut screens = HashMap::new();
        for pane in &view.panes {
            let mut screen = ScreenState::new(pane.area.inset(BORDER).size());
            if let Some(frame) = &pane.frame {
                screen.apply(frame);
            } else if let Some(previous) = self.screens.remove(&pane.id) {
                screen = previous;
            }
            screens.insert(pane.id, screen);
        }
        self.screens = screens;
        self.view = view;
    }

    fn apply(&mut self, response: &Response) {
        match response {
            Response::View { view } => self.take_view((**view).clone()),
            Response::Frame { pane, frame } => {
                self.screens
                    .entry(*pane)
                    .or_insert_with(|| ScreenState::new(frame.size))
                    .apply(frame);
            }
            _ => {}
        }
    }

    /// Everything on every visible pane.
    fn text(&self) -> String {
        self.view
            .panes
            .iter()
            .filter_map(|pane| self.screens.get(&pane.id))
            .map(ScreenState::text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// What is on one pane.
    fn pane_text(&self, pane: PaneId) -> String {
        self.screens
            .get(&pane)
            .map(ScreenState::text)
            .unwrap_or_default()
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
            result.is_ok(),
            "never saw {what}.\nview: {:#?}\nscreens:\n{}",
            self.view,
            self.text()
        );
        seen
    }

    /// Read until every visible pane shows `needle`.
    async fn read_until_text(&mut self, needle: &str) -> Vec<Response> {
        let wanted = needle.to_owned();
        self.read_until(&format!("{needle:?} on the screen"), move |client| {
            client.text().contains(&wanted)
        })
        .await
    }

    /// Read until the session has this many visible panes.
    async fn read_until_panes(&mut self, panes: usize) {
        self.read_until(&format!("{panes} panes"), move |client| {
            client.view.panes.len() == panes
        })
        .await;
    }
}

#[tokio::test]
async fn attaching_gives_a_whole_view_and_then_only_what_changes() {
    with_daemon(
        "printf 'first line\\n'; sleep 0.3; printf 'second line\\n'; sleep 10",
        |socket| async move {
            let mut client = TestClient::attach(&socket, PaneSize::new(10, 40)).await;
            assert_eq!(client.view.panes.len(), 1);
            assert_eq!(client.view.workspaces.len(), 1);
            assert_eq!(client.view.tabs.len(), 1);

            client.read_until_text("first line").await;
            let later = client.read_until_text("second line").await;

            let pane = client.view.panes[0].id;
            assert!(client.pane_text(pane).contains("first line"));
            // Nothing structural happened, so only frames followed.
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
async fn a_pane_fills_what_the_chrome_leaves_it() {
    with_daemon(
        "while :; do stty size; sleep 1; done",
        |socket| async move {
            let size = PaneSize::new(24, 80);
            let mut client = TestClient::attach(&socket, size).await;

            let inside = whole_pane(size, true);
            client
                .read_until_text(&format!("{} {}", inside.rows(), inside.cols()))
                .await;
            assert_eq!(client.screens[&client.view.panes[0].id].size(), inside);
        },
    )
    .await;
}

#[tokio::test]
async fn hiding_the_sidebar_gives_its_columns_to_the_pane() {
    with_daemon(
        "while :; do stty size; sleep 1; done",
        |socket| async move {
            let size = PaneSize::new(24, 80);
            let mut client = TestClient::attach(&socket, size).await;
            let narrow = whole_pane(size, true);
            client
                .read_until_text(&format!("{} {}", narrow.rows(), narrow.cols()))
                .await;

            client.send(&Request::ToggleSidebar).await;

            let wide = whole_pane(size, false);
            assert!(wide.cols() > narrow.cols());
            client
                .read_until_text(&format!("{} {}", wide.rows(), wide.cols()))
                .await;
            assert!(client.view.sidebar.is_empty());
        },
    )
    .await;
}

#[tokio::test]
async fn keys_sent_over_the_socket_reach_the_focused_pane() {
    with_daemon(SCRIPT, |socket| async move {
        let mut client = TestClient::attach(&socket, PaneSize::new(24, 80)).await;
        client.read_until_text("pane up").await;

        client
            .send(&Request::Input {
                bytes: b"abc\r".to_vec().into(),
            })
            .await;
        client.read_until_text("typed abc").await;
    })
    .await;
}

#[tokio::test]
async fn a_resize_reaches_the_program_and_comes_back_as_a_full_frame() {
    with_daemon(
        "while :; do stty size; sleep 1; done",
        |socket| async move {
            let mut client = TestClient::attach(&socket, PaneSize::new(24, 80)).await;
            let before = whole_pane(PaneSize::new(24, 80), true);
            client
                .read_until_text(&format!("{} {}", before.rows(), before.cols()))
                .await;

            client
                .send(&Request::Resize {
                    size: PaneSize::new(30, 100),
                })
                .await;

            let after = whole_pane(PaneSize::new(30, 100), true);
            client
                .read_until_text(&format!("{} {}", after.rows(), after.cols()))
                .await;
            assert_eq!(client.view.size, PaneSize::new(30, 100));
        },
    )
    .await;
}

#[tokio::test]
async fn splitting_puts_the_new_pane_beside_the_old_one_and_focuses_it() {
    with_daemon(SCRIPT, |socket| async move {
        let mut client = TestClient::attach(&socket, PaneSize::new(24, 80)).await;
        client.read_until_text("pane up").await;
        let first = client.view.panes[0].id;

        client
            .send(&Request::Split {
                direction: Direction::LeftRight,
            })
            .await;
        client.read_until_panes(2).await;

        let left = client.view.panes[0].area;
        let right = client.view.panes[1].area;
        assert_eq!(left.y, right.y, "side by side, not stacked");
        assert_eq!(right.x, left.right(), "touching, with no gap");
        assert_eq!(left.cols + right.cols, client.view.content.cols);

        let second = client.view.panes[1].id;
        assert_eq!(client.view.focus, Some(second), "the new pane is focused");

        // Typing goes to the new pane and not to the old one.
        client
            .send(&Request::Input {
                bytes: b"second\r".to_vec().into(),
            })
            .await;
        client
            .read_until("the new pane to answer", move |client| {
                client.pane_text(second).contains("typed second")
            })
            .await;
        assert!(!client.pane_text(first).contains("typed second"));
    })
    .await;
}

#[tokio::test]
async fn splitting_the_other_way_stacks_the_panes() {
    with_daemon(SCRIPT, |socket| async move {
        let mut client = TestClient::attach(&socket, PaneSize::new(24, 80)).await;
        client.read_until_text("pane up").await;

        client
            .send(&Request::Split {
                direction: Direction::TopBottom,
            })
            .await;
        client.read_until_panes(2).await;

        let top = client.view.panes[0].area;
        let bottom = client.view.panes[1].area;
        assert_eq!(top.x, bottom.x);
        assert_eq!(bottom.y, top.bottom());
        assert_eq!(top.rows + bottom.rows, client.view.content.rows);
    })
    .await;
}

#[tokio::test]
async fn the_focus_moves_between_panes_and_the_keys_follow_it() {
    with_daemon(SCRIPT, |socket| async move {
        let mut client = TestClient::attach(&socket, PaneSize::new(24, 80)).await;
        client.read_until_text("pane up").await;
        let first = client.view.panes[0].id;

        client
            .send(&Request::Split {
                direction: Direction::LeftRight,
            })
            .await;
        client.read_until_panes(2).await;

        client.send(&Request::FocusSide { side: Side::Left }).await;
        client
            .read_until("the focus to move left", move |client| {
                client.view.focus == Some(first)
            })
            .await;

        client
            .send(&Request::Input {
                bytes: b"leftwards\r".to_vec().into(),
            })
            .await;
        client
            .read_until("the left pane to answer", move |client| {
                client.pane_text(first).contains("typed leftwards")
            })
            .await;
    })
    .await;
}

#[tokio::test]
async fn a_pane_can_be_focused_by_its_identifier() {
    with_daemon(SCRIPT, |socket| async move {
        let mut client = TestClient::attach(&socket, PaneSize::new(24, 80)).await;
        let first = client.view.panes[0].id;
        client
            .send(&Request::Split {
                direction: Direction::LeftRight,
            })
            .await;
        client.read_until_panes(2).await;

        client.send(&Request::FocusPane { pane: first }).await;
        client
            .read_until("the first pane focused", move |client| {
                client.view.focus == Some(first)
            })
            .await;

        // The next pane after it is the other one, wrapping around a list of
        // two.
        client.send(&Request::FocusNext).await;
        client
            .read_until("the focus to move on", move |client| {
                client.view.focus != Some(first)
            })
            .await;
    })
    .await;
}

#[tokio::test]
async fn resizing_a_pane_moves_the_divider_it_shares() {
    with_daemon(SCRIPT, |socket| async move {
        let mut client = TestClient::attach(&socket, PaneSize::new(24, 80)).await;
        client
            .send(&Request::Split {
                direction: Direction::LeftRight,
            })
            .await;
        client.read_until_panes(2).await;
        let before = client.view.panes[1].area.cols;

        // The focused pane is the right-hand one; growing it leftwards takes
        // columns from its neighbour.
        client
            .send(&Request::ResizePane {
                side: Side::Left,
                cells: 5,
            })
            .await;
        client
            .read_until("the divider to move", move |client| {
                client.view.panes[1].area.cols > before
            })
            .await;

        let left = client.view.panes[0].area;
        let right = client.view.panes[1].area;
        assert_eq!(right.x, left.right(), "still touching");
        assert_eq!(left.cols + right.cols, client.view.content.cols);
    })
    .await;
}

#[tokio::test]
async fn a_pane_whose_program_ends_gives_its_space_to_the_other() {
    with_daemon(
        "printf 'pane up\\n'; read line; [ \"$line\" = bye ] && exit 0; sleep 10",
        |socket| async move {
            let mut client = TestClient::attach(&socket, PaneSize::new(24, 80)).await;
            let whole = client.view.content;

            client
                .send(&Request::Split {
                    direction: Direction::LeftRight,
                })
                .await;
            client.read_until_panes(2).await;
            let first = client.view.panes[0].id;

            // The focused pane is the new one; ending it leaves the first.
            client
                .send(&Request::Input {
                    bytes: b"bye\r".to_vec().into(),
                })
                .await;
            client.read_until_panes(1).await;

            assert_eq!(client.view.panes[0].id, first);
            assert_eq!(client.view.panes[0].area, whole);
            assert_eq!(client.view.focus, Some(first));
        },
    )
    .await;
}

#[tokio::test]
async fn a_new_tab_hides_the_first_one_and_selecting_it_back_shows_it() {
    with_daemon(SCRIPT, |socket| async move {
        let mut client = TestClient::attach(&socket, PaneSize::new(24, 80)).await;
        client.read_until_text("pane up").await;
        let first = client.view.panes[0].id;

        client.send(&Request::NewTab).await;
        client
            .read_until("the second tab", |client| client.view.tabs.len() == 2)
            .await;

        assert_eq!(client.view.active_tab, 1);
        assert_eq!(client.view.panes.len(), 1, "one tab's panes are shown");
        assert_ne!(client.view.panes[0].id, first);
        assert_eq!(
            client.view.panes[0].area, client.view.content,
            "the new tab's pane fills the content area"
        );

        client.send(&Request::SelectTab { index: 0 }).await;
        client
            .read_until("the first tab again", move |client| {
                client.view.active_tab == 0
            })
            .await;
        assert_eq!(client.view.panes[0].id, first);
        // The pane kept running while it was hidden.
        assert!(client.pane_text(first).contains("pane up"));
    })
    .await;
}

#[tokio::test]
async fn a_new_workspace_is_shown_and_the_first_one_is_still_listed() {
    with_daemon(SCRIPT, |socket| async move {
        let mut client = TestClient::attach(&socket, PaneSize::new(24, 80)).await;
        let first = client.view.panes[0].id;

        client.send(&Request::NewWorkspace { cwd: None }).await;
        client
            .read_until("the second workspace", |client| {
                client.view.workspaces.len() == 2
            })
            .await;

        assert_eq!(client.view.active_workspace, 1);
        assert_ne!(client.view.panes[0].id, first);

        client.send(&Request::SelectWorkspace { index: 0 }).await;
        client
            .read_until("the first workspace again", move |client| {
                client.view.active_workspace == 0
            })
            .await;
        assert_eq!(client.view.panes[0].id, first);
    })
    .await;
}

#[tokio::test]
async fn a_workspace_is_named_after_the_directory_it_was_started_in() {
    with_daemon(SCRIPT, |socket| async move {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let project = directory.path().join("my-project");
        std::fs::create_dir(&project).expect("the project directory");

        let mut connection = Connection::connect(&socket).await.expect("a connection");
        let response = connection
            .request(&Request::Attach {
                size: PaneSize::new(24, 80),
                cwd: Some(project.clone()),
            })
            .await
            .expect("the attach is answered");

        let Response::View { view } = response else {
            panic!("expected a view, got {response:?}");
        };
        assert_eq!(view.workspaces[0].name, "my-project");
    })
    .await;
}

#[tokio::test]
async fn the_end_of_the_last_program_ends_the_session() {
    with_daemon("printf 'goodbye\\n'; exit 7", |socket| async move {
        let mut client = TestClient::attach(&socket, PaneSize::new(24, 80)).await;

        // A session with no panes left has nothing more to draw, and says so
        // once rather than sending an empty view.
        let exit = loop {
            match tokio::time::timeout(TIMEOUT, client.connection.receive()).await {
                Ok(Ok(Response::SessionEnded { exit })) => {
                    break exit.expect("the program's end is reported");
                }
                Ok(Ok(message)) => client.apply(&message),
                other => panic!("the session never reported how it ended: {other:?}"),
            }
        };
        assert_eq!(exit.code, Some(7));
        assert!(
            client.text().contains("goodbye"),
            "the last output arrived before the end.\nscreen:\n{}",
            client.text()
        );
    })
    .await;
}

#[tokio::test]
async fn detaching_leaves_the_programs_running() {
    with_daemon("printf 'still here\\n'; sleep 10", |socket| async move {
        let mut client = TestClient::attach(&socket, PaneSize::new(24, 80)).await;
        client.read_until_text("still here").await;

        let response = client
            .connection
            .request(&Request::Detach)
            .await
            .expect("the detach is answered");
        assert_eq!(response, Response::Ok);
        drop(client);

        // Attaching again finds the same session, with its screen intact.
        let mut again = TestClient::attach(&socket, PaneSize::new(24, 80)).await;
        assert!(again.text().contains("still here"));

        let listing = again
            .connection
            .request(&Request::ListPanes)
            .await
            .expect("the listing is answered");
        let Response::Panes { panes } = listing else {
            panic!("expected a pane listing, got {listing:?}");
        };
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].exit, None);
        assert_eq!(panes[0].title, "shell");
    })
    .await;
}

#[tokio::test]
async fn two_clients_attached_to_one_session_both_see_it() {
    with_daemon(SCRIPT, |socket| async move {
        let mut first = TestClient::attach(&socket, PaneSize::new(24, 80)).await;
        let mut second = TestClient::attach(&socket, PaneSize::new(24, 80)).await;

        first.read_until_text("pane up").await;
        second.read_until_text("pane up").await;

        // The second client typing is seen by the first.
        second
            .send(&Request::Input {
                bytes: b"by the second\r".to_vec().into(),
            })
            .await;
        first.read_until_text("typed by the second").await;

        // And a split asked for by one is drawn by both.
        second
            .send(&Request::Split {
                direction: Direction::LeftRight,
            })
            .await;
        first.read_until_panes(2).await;
        second.read_until_panes(2).await;
    })
    .await;
}

#[tokio::test]
async fn a_client_that_goes_away_mid_stream_does_not_disturb_the_panes() {
    with_daemon(
        "i=0; while [ $i -lt 500 ]; do echo \"line $i\"; i=$((i+1)); done; printf 'finished\\n'; sleep 10",
        |socket| async move {
            // Attach and then stop reading entirely, which is what a client
            // being killed looks like to the daemon.
            let abandoned = TestClient::attach(&socket, PaneSize::new(24, 80)).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
            drop(abandoned);

            // The pane kept running and a new client sees the whole result.
            let mut client = TestClient::attach(&socket, PaneSize::new(24, 80)).await;
            client.read_until_text("finished").await;
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
async fn input_to_a_session_with_no_panes_is_not_fatal() {
    let session = Session::new(PaneSpec::shell());
    let error = session
        .input(b"x".to_vec())
        .await
        .expect_err("there is nothing to type into");
    assert!(matches!(error, Error::NoPanes), "{error}");
    assert!(error.to_string().contains("no panes"), "{error}");

    let error = session
        .input_to(PaneId::FIRST, b"x".to_vec())
        .await
        .expect_err("there is no such pane");
    assert!(matches!(error, Error::PaneGone { .. }), "{error}");
    assert!(error.to_string().contains("no longer running"), "{error}");
}
