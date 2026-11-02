//! The `otag` binary driven through a real pseudo-terminal.
//!
//! Every other test reaches otag through its socket. These run the binary the
//! way a user does — in a terminal, in raw mode, with keys arriving as bytes
//! — which is the only way to cover the client's own terminal handling: the
//! screen it draws, the keys it encodes, and the state it leaves the terminal
//! in.
//!
//! The test acts as the terminal, so it also has to behave like one and
//! answer the queries a client makes.

use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, MasterPty, PtySize};
use tempfile::TempDir;

/// Path of the binary under test, provided by Cargo.
const OTAG: &str = env!("CARGO_BIN_EXE_otag");

/// How long a step waits for the screen to show something.
const TIMEOUT: Duration = Duration::from_secs(20);

/// The key that introduces an otag command.
const PREFIX: &[u8] = &[0x02];

/// otag running in a terminal this test controls.
struct Terminal {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    screen: Arc<Mutex<vt100::Parser>>,
    _home: Arc<TempDir>,
    socket: PathBuf,
}

impl Terminal {
    /// Start `otag` in a terminal of the given size, with its own home
    /// directory and socket so the test never touches the real ones.
    fn start(home: Arc<TempDir>, rows: u16, cols: u16) -> Self {
        let socket = home.path().join("otag.sock");
        let pair = portable_pty::native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("a pseudo-terminal");

        let mut command = CommandBuilder::new(OTAG);
        command.env("HOME", home.path());
        command.env("OTAG_SOCKET", &socket);
        command.env("TERM", "xterm-256color");
        command.env_remove("OTAG_LOG");
        command.cwd(home.path());

        let child = pair.slave.spawn_command(command).expect("otag starts");
        drop(pair.slave);

        let screen = Arc::new(Mutex::new(vt100::Parser::new(
            NonZeroU16::new(rows).expect("rows"),
            NonZeroU16::new(cols).expect("cols"),
            200,
        )));
        let writer = Arc::new(Mutex::new(
            pair.master.take_writer().expect("a terminal writer"),
        ));

        let mut reader = pair.master.try_clone_reader().expect("a terminal reader");
        let parser = Arc::clone(&screen);
        let answerer = Arc::clone(&writer);
        std::thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 {
                    return;
                }
                let chunk = &buffer[..count];
                parser.lock().expect("the screen").process(chunk);
                answer_queries(chunk, &parser, &answerer);
            }
        });

        Self {
            master: pair.master,
            child,
            writer,
            screen,
            _home: home,
            socket,
        }
    }

    /// Type these bytes, as a keyboard would deliver them.
    fn type_bytes(&self, bytes: &[u8]) {
        let mut writer = self.writer.lock().expect("the terminal writer");
        writer.write_all(bytes).expect("the keys are written");
        writer.flush().expect("the keys arrive");
    }

    /// Type text and press return.
    fn type_line(&self, line: &str) {
        self.type_bytes(format!("{line}\r").as_bytes());
    }

    /// Everything on the screen, as the user would see it.
    fn text(&self) -> String {
        self.screen.lock().expect("the screen").screen().contents()
    }

    /// One row of the screen, with trailing blanks removed.
    fn row(&self, row: u16) -> String {
        let screen = self.screen.lock().expect("the screen");
        let screen = screen.screen();
        let (_, cols) = screen.size();
        let mut text = String::new();
        for col in 0..cols.get() {
            if let Some(cell) = screen.cell(row, col) {
                if cell.is_wide_continuation() {
                    continue;
                }
                let contents = cell.contents();
                text.push_str(if contents.is_empty() { " " } else { contents });
            }
        }
        text.trim_end().to_owned()
    }

    /// Wait until the screen contains `needle`.
    fn wait_for(&self, needle: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if self.text().contains(needle) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!(
            "the screen never showed {needle:?} within {TIMEOUT:?}.\nscreen:\n{}",
            self.text()
        );
    }

    /// Wait for a shell prompt by running a command whose output is unique.
    fn wait_for_shell(&self) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            self.type_line("echo otag-ready-marker");
            let until = Instant::now() + Duration::from_millis(500);
            while Instant::now() < until {
                if self.text().contains("otag-ready-marker") {
                    return;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        panic!(
            "no shell appeared within {TIMEOUT:?}.\nscreen:\n{}",
            self.text()
        );
    }

    /// Resize the terminal otag is drawing into.
    fn resize(&self, rows: u16, cols: u16) {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("the terminal resizes");
        self.screen.lock().expect("the screen").set_size(
            NonZeroU16::new(rows).expect("rows"),
            NonZeroU16::new(cols).expect("cols"),
        );
    }

    /// Wait for otag itself to exit, and report whether it succeeded.
    fn wait_for_exit(&mut self) -> bool {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(status)) => return status.success(),
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                Err(error) => panic!("cannot wait for otag: {error}"),
            }
        }
        panic!(
            "otag did not exit within {TIMEOUT:?}.\nscreen:\n{}",
            self.text()
        )
    }

    /// Run another otag command against the same daemon.
    fn run(&self, arguments: &[&str]) -> String {
        run_otag(self._home.path(), &self.socket, arguments)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        // Never leave a daemon behind, even if the test failed.
        let _ = std::process::Command::new(OTAG)
            .args(["kill-server", "--socket"])
            .arg(&self.socket)
            .env("HOME", self._home.path())
            .output();
    }
}

/// Answer the queries a client sends to find out about its terminal.
///
/// A real terminal replies to these; without an answer the client waits for
/// its own timeout.
fn answer_queries(
    chunk: &[u8],
    screen: &Arc<Mutex<vt100::Parser>>,
    writer: &Arc<Mutex<Box<dyn Write + Send>>>,
) {
    let mut replies = Vec::new();
    for window in chunk.windows(4) {
        if window == b"\x1b[6n" {
            let (row, col) = screen
                .lock()
                .expect("the screen")
                .screen()
                .cursor_position();
            replies.extend_from_slice(
                format!("\x1b[{};{}R", row.saturating_add(1), col.saturating_add(1)).as_bytes(),
            );
        }
    }
    if !replies.is_empty() {
        let mut writer = writer.lock().expect("the terminal writer");
        let _ = writer.write_all(&replies);
        let _ = writer.flush();
    }
}

/// Run an otag command and return its output.
fn run_otag(home: &Path, socket: &Path, arguments: &[&str]) -> String {
    let output = std::process::Command::new(OTAG)
        .args(arguments)
        .arg("--socket")
        .arg(socket)
        .env("HOME", home)
        .env_remove("OTAG_LOG")
        .output()
        .expect("the otag binary runs");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn home() -> Arc<TempDir> {
    Arc::new(TempDir::new().expect("a temporary home directory"))
}

#[test]
fn a_shell_runs_inside_otag_and_keys_reach_it() {
    let terminal = Terminal::start(home(), 24, 80);
    terminal.wait_for_shell();

    terminal.type_line("echo the-shell-is-alive");
    terminal.wait_for("the-shell-is-alive");
}

#[test]
fn a_program_is_told_the_terminal_it_is_running_in() {
    let terminal = Terminal::start(home(), 24, 80);
    terminal.wait_for_shell();

    // What otag's emulation implements, not what the outer terminal is.
    terminal.type_line("echo \"term=$TERM colorterm=$COLORTERM\"");
    terminal.wait_for("term=xterm-256color colorterm=truecolor");
}

#[test]
fn a_program_is_told_the_size_of_the_pane() {
    let terminal = Terminal::start(home(), 24, 80);
    terminal.wait_for_shell();

    terminal.type_line("stty size");
    terminal.wait_for("24 80");
}

#[test]
fn resizing_the_terminal_resizes_the_program() {
    let terminal = Terminal::start(home(), 24, 80);
    terminal.wait_for_shell();
    terminal.type_line("stty size");
    terminal.wait_for("24 80");

    terminal.resize(30, 100);
    terminal.type_line("stty size");
    terminal.wait_for("30 100");
}

#[test]
fn a_full_screen_program_draws_and_leaves_the_screen_as_it_found_it() {
    let terminal = Terminal::start(home(), 24, 80);
    terminal.wait_for_shell();

    // A program that moves to the alternate screen, clears it and draws at a
    // position of its own. This is what an editor or a system monitor does.
    //
    // The marker is assembled from two words, so that the command line the
    // shell echoes does not contain it: only the program's own output does.
    terminal.type_line("printf '\\033[?1049h\\033[2J\\033[5;10H%s%s' drawn onscreen");
    terminal.wait_for("drawnonscreen");

    // The program addressed row 5, column 10, counting from one. It has to
    // land exactly there, which it only does if cursor addressing survives
    // the emulator, the frame and the renderer.
    // The shell may add a marker of its own after output that did not end
    // in a newline, so the row has to start with what the program drew
    // rather than consist only of it.
    let row = terminal.row(4);
    assert!(
        row.starts_with(&format!("{}drawnonscreen", " ".repeat(9))),
        "the program drew at the wrong place: {row:?}\nscreen:\n{}",
        terminal.text()
    );

    // Leaving the alternate screen brings back what was there before, and
    // what the program drew is gone.
    terminal.type_line("printf '\\033[?1049l'; echo back-on-the-main-screen");
    terminal.wait_for("back-on-the-main-screen");
    assert!(
        !terminal.text().contains("drawnonscreen"),
        "the alternate screen is left behind.\nscreen:\n{}",
        terminal.text()
    );
}

#[test]
fn the_up_arrow_recalls_a_command_from_the_shell_s_history() {
    let terminal = Terminal::start(home(), 24, 80);
    terminal.wait_for_shell();

    terminal.type_line("echo recalled-by-the-up-arrow");
    terminal.wait_for("recalled-by-the-up-arrow");

    // A shell's line editor asks for application cursor keys, so this only
    // works if the pane's modes reach the client and are used to encode the
    // key. The recalled command appears on the prompt line, and running it
    // produces its output a second time.
    terminal.type_bytes(b"\x1b[A");
    terminal.type_bytes(b"\r");

    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        if terminal.text().matches("recalled-by-the-up-arrow").count() >= 3 {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!(
        "the up arrow did not recall the command.\nscreen:\n{}",
        terminal.text()
    );
}

#[test]
fn the_prefix_key_can_be_sent_to_the_pane_by_pressing_it_twice() {
    let terminal = Terminal::start(home(), 24, 80);
    terminal.wait_for_shell();

    terminal.type_line("cat -v");
    std::thread::sleep(Duration::from_millis(300));

    terminal.type_bytes(PREFIX);
    terminal.type_bytes(PREFIX);
    terminal.type_bytes(b"\r");
    terminal.wait_for("^B");

    terminal.type_bytes(&[0x04]);
}

#[test]
fn detaching_leaves_the_pane_running_and_reattaching_finds_it() {
    let home = home();
    let mut first = Terminal::start(Arc::clone(&home), 24, 80);
    first.wait_for_shell();
    first.type_line("echo written-before-detaching");
    first.wait_for("written-before-detaching");

    // The prefix key, then the key that detaches.
    first.type_bytes(PREFIX);
    first.type_bytes(b"d");
    assert!(first.wait_for_exit(), "otag detaches without an error");

    // The pane is still there, with its program.
    let listing = first.run(&["ls"]);
    assert!(listing.contains("running"), "{listing}");

    let second = Terminal::start(Arc::clone(&home), 24, 80);
    second.wait_for("written-before-detaching");
}

#[test]
fn otag_stops_when_the_pane_s_program_ends() {
    let mut terminal = Terminal::start(home(), 24, 80);
    terminal.wait_for_shell();

    terminal.type_line("exit");
    assert!(
        terminal.wait_for_exit(),
        "otag stops cleanly when the shell exits"
    );

    let listing = terminal.run(&["ls"]);
    assert!(listing.contains("exited"), "{listing}");
}

#[test]
fn a_program_that_fails_is_reported_after_the_terminal_is_restored() {
    let mut terminal = Terminal::start(home(), 24, 80);
    terminal.wait_for_shell();

    terminal.type_line("exit 3");
    // The message is printed to the restored terminal, so it is plain text
    // on the screen rather than part of otag's own drawing.
    terminal.wait_for("exited with status 3");
    // otag itself did what it was asked, so it reports success: the
    // program's own status is reported in words instead.
    assert!(terminal.wait_for_exit(), "otag stops cleanly");
}
