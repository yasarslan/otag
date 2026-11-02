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

use otag_core::layout::{Area, BORDER, Chrome, Direction, Layout};
use otag_core::pane::{PaneId, PaneSize};
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
        let shell = write_test_shell(home.path());
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
        // The pane runs this instead of whoever's shell happens to be
        // configured on the machine running the tests.
        command.env("SHELL", &shell);
        command.env("BASH_SILENCE_DEPRECATION_WARNING", "1");
        command.env_remove("ENV");
        command.env_remove("BASH_ENV");
        command.env_remove("PS1");
        command.env_remove("PROMPT_COMMAND");
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

    /// The size of the terminal otag is drawing into.
    fn size(&self) -> PaneSize {
        let screen = self.screen.lock().expect("the screen");
        let (rows, cols) = screen.screen().size();
        PaneSize::new(rows.get(), cols.get())
    }

    /// Where the single pane of a fresh session is drawn, inside its border.
    ///
    /// otag's own sidebar and tab bar take part of the terminal, so a test
    /// that reads what a program drew has to read the part of the screen
    /// that belongs to the program.
    fn pane_area(&self) -> Area {
        Chrome::of(self.size(), true).content.inset(BORDER)
    }

    /// Where each pane of a session split this way is drawn, inside its
    /// borders.
    ///
    /// Computed with otag's own layout, so the test looks where otag puts
    /// the panes rather than repeating the arithmetic.
    fn split_areas(&self, direction: Direction) -> Vec<Area> {
        let mut layout = Layout::new(PaneId::FIRST);
        assert!(layout.split(PaneId::FIRST, direction, PaneId::FIRST.next()));
        layout
            .areas(Chrome::of(self.size(), true).content)
            .into_iter()
            .map(|(_, area)| area.inset(BORDER))
            .collect()
    }

    /// The text of one rectangle of the screen, with rows that continue one
    /// another joined back into the line they came from.
    ///
    /// A line longer than a pane is split across rows, so searching for text
    /// that happens to land on a break would fail even though the user sees
    /// it. A row that fills its last column continues on the next one: otag
    /// draws each row where it belongs rather than letting the terminal
    /// wrap, so that, and not the terminal's own wrap flag, is what says
    /// where a line goes on.
    fn region(&self, area: Area) -> String {
        let screen = self.screen.lock().expect("the screen");
        let screen = screen.screen();
        let mut text = String::new();
        for row in area.y..area.bottom() {
            let contents = cells(screen, row, area.x, area.right());
            if contents.ends_with(' ') {
                text.push_str(contents.trim_end());
                text.push('\n');
            } else {
                text.push_str(&contents);
            }
        }
        text
    }

    /// What the program in the session's only pane has drawn.
    fn pane_text(&self) -> String {
        self.region(self.pane_area())
    }

    /// Where otag lists the workspaces.
    fn sidebar_area(&self) -> Area {
        Chrome::of(self.size(), true).sidebar
    }

    /// Where otag draws the tab bar.
    fn tabs_area(&self) -> Area {
        Chrome::of(self.size(), true).tabs
    }

    /// Wait until one rectangle of the screen contains `needle`.
    fn wait_for_in(&self, area: Area, needle: &str) {
        self.wait_until(&format!("{needle:?} in {area:?}"), || {
            self.region(area).contains(needle)
        });
    }

    /// Run a command in the focused pane until its output shows up in
    /// `area`, asking again on a timer.
    fn type_until_in(&self, area: Area, line: &str, needle: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            self.type_line(line);
            let until = Instant::now() + Duration::from_millis(500);
            while Instant::now() < until {
                if self.region(area).contains(needle) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        panic!(
            "{line:?} never showed {needle:?} in {area:?} within {TIMEOUT:?}.\nscreen:\n{}",
            self.text()
        );
    }

    /// Press the prefix key followed by `key`, which is how every otag
    /// command is given.
    fn command(&self, key: &str) {
        self.type_bytes(PREFIX);
        self.type_bytes(key.as_bytes());
    }

    /// Whether `needle` is anywhere the user could read it: in the pane, or
    /// on the screen outside it — otag's own sidebar and tab bar, or plain
    /// text printed once otag has given the terminal back.
    fn shows(&self, needle: &str) -> bool {
        self.pane_text().contains(needle) || self.text().contains(needle)
    }

    /// Wait until `ready` holds, or fail with the screen in the message.
    fn wait_until(&self, what: &str, ready: impl Fn() -> bool) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if ready() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!(
            "never saw {what} within {TIMEOUT:?}.\nscreen:\n{}",
            self.text()
        );
    }

    /// Wait until the screen contains `needle`.
    fn wait_for(&self, needle: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if self.shows(needle) {
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
    ///
    /// The marker is assembled by the shell, so that the keys the terminal
    /// may still be echoing before otag takes over cannot be mistaken for a
    /// shell that has answered.
    fn wait_for_shell(&self) {
        self.type_until("printf '%s%s\\n' otag-ready -marker", "otag-ready-marker");
    }

    /// Run a command until the screen shows `needle`, asking again on a
    /// timer.
    ///
    /// For an answer that depends on something still on its way — a resize
    /// travelling to the daemon and back — asking once races with it: the
    /// shell would answer about the terminal as it was, and that answer
    /// stands. Asking again until the answer changes does not.
    fn type_until(&self, line: &str, needle: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            self.type_line(line);
            let until = Instant::now() + Duration::from_millis(500);
            while Instant::now() < until {
                if self.shows(needle) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        panic!(
            "{line:?} never showed {needle:?} within {TIMEOUT:?}.\nscreen:\n{}",
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

    /// Stop the daemon and wait until it has really gone.
    ///
    /// `otag kill-server` is answered before the daemon has finished saving
    /// and let go of its socket; starting the next one before that would
    /// reach the daemon on its way out.
    fn kill_server(&self) {
        self.run(&["kill-server"]);
        self.wait_until("the daemon to stop", || {
            self.run(&["ls"]).contains("no daemon")
        });
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

/// The text of part of one row.
fn cells(screen: &vt100::Screen, row: u16, from: u16, to: u16) -> String {
    let mut text = String::new();
    for col in from..to {
        if let Some(cell) = screen.cell(row, col) {
            let contents = cell.contents();
            text.push_str(if contents.is_empty() { " " } else { contents });
        }
    }
    text
}

/// Write the shell the panes in these tests run, and return its path.
///
/// The tests assert on what the screen shows, so the shell has to be the
/// same everywhere: a machine whose user runs another shell, or whose prompt
/// is long enough to wrap the commands the tests type, must not change the
/// result. This is a plain, interactive bash that reads no start-up file of
/// the machine's, with a two-character prompt and a history file inside the
/// test's own home directory.
fn write_test_shell(home: &Path) -> PathBuf {
    let rc = home.join("test-bashrc");
    std::fs::write(
        &rc,
        format!(
            "PS1='$ '\n\
             PS2='> '\n\
             unset PROMPT_COMMAND\n\
             HISTFILE='{history}'\n\
             HISTSIZE=200\n\
             HISTFILESIZE=200\n",
            history = home.join("test-history").display()
        ),
    )
    .expect("the shell's start-up file is written");

    let shell = home.join("test-shell");
    std::fs::write(
        &shell,
        format!(
            "#!/bin/sh\nexec '{bash}' --noprofile --rcfile '{rc}' -i \"$@\"\n",
            bash = bash_path().display(),
            rc = rc.display()
        ),
    )
    .expect("the test shell is written");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755))
            .expect("the test shell is executable");
    }

    shell
}

/// Where bash lives on this machine.
fn bash_path() -> PathBuf {
    ["/bin/bash", "/usr/bin/bash", "/usr/local/bin/bash"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .expect("these tests need bash; install it or add it to one of the usual locations")
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
fn a_program_is_told_the_size_of_the_pane_and_not_of_the_terminal() {
    let terminal = Terminal::start(home(), 24, 80);
    terminal.wait_for_shell();

    // otag's sidebar, tab bar and the pane's border take part of the
    // terminal; the program is told what is left for it.
    let pane = terminal.pane_area();
    assert!(pane.cols < 80 && pane.rows < 24, "{pane:?}");

    terminal.type_line("stty size");
    terminal.wait_for(&format!("{} {}", pane.rows, pane.cols));
}

#[test]
fn resizing_the_terminal_resizes_the_program() {
    let terminal = Terminal::start(home(), 24, 80);
    terminal.wait_for_shell();
    let before = terminal.pane_area();
    terminal.type_line("stty size");
    terminal.wait_for(&format!("{} {}", before.rows, before.cols));

    terminal.resize(30, 100);
    let after = terminal.pane_area();
    terminal.type_until("stty size", &format!("{} {}", after.rows, after.cols));
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

    // The program addressed row 5, column 10 of its own screen, counting
    // from one. It has to land exactly there inside the pane, which it only
    // does if cursor addressing survives the emulator, the frame and the
    // renderer.
    // The shell may add a marker of its own after output that did not end
    // in a newline, so the row has to start with what the program drew
    // rather than consist only of it.
    let pane = terminal.pane_area();
    let row = terminal.region(Area::new(pane.x, pane.y + 4, pane.cols, 1));
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

    // The marker is assembled from two words, so that only the program's
    // output counts: the command line the shell echoes does not contain it,
    // wherever the prompt happens to break it.
    terminal.type_line("printf '%s%s\\n' recalled by-the-up-arrow");
    terminal.wait_for("recalledby-the-up-arrow");

    // A shell's line editor asks for application cursor keys, so this only
    // works if the pane's modes reach the client and are used to encode the
    // key. The recalled command appears on the prompt line, and running it
    // produces its output a second time.
    terminal.type_bytes(b"\x1b[A");
    terminal.type_bytes(b"\r");

    terminal.wait_until("the command run a second time", || {
        terminal
            .pane_text()
            .matches("recalledby-the-up-arrow")
            .count()
            >= 2
    });
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

    // A pane leaves the session when its program ends, so there is nothing
    // left to list.
    let listing = terminal.run(&["ls"]);
    assert!(listing.contains("no panes"), "{listing}");
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

#[test]
fn splitting_a_pane_shows_both_of_them_and_keys_go_to_the_new_one() {
    let terminal = Terminal::start(home(), 24, 80);
    terminal.wait_for_shell();
    terminal.type_line("printf '%s%s\\n' in-the first-pane");
    terminal.wait_for("in-thefirst-pane");

    terminal.command("%");

    let areas = terminal.split_areas(Direction::LeftRight);
    let (left, right) = (areas[0], areas[1]);
    assert!(right.x > left.x, "side by side: {areas:?}");

    // What the first pane printed is still on it, now in half the width.
    terminal.wait_for_in(left, "in-thefirst-pane");

    // The new pane has the focus, so what is typed reaches it and not the
    // one it was split from.
    terminal.type_until_in(
        right,
        "printf '%s%s\\n' in-the second-pane",
        "in-thesecond-pane",
    );
    assert!(
        !terminal.region(left).contains("in-thesecond-pane"),
        "the first pane received the keys.\nscreen:\n{}",
        terminal.text()
    );
}

#[test]
fn splitting_the_other_way_stacks_the_panes() {
    let terminal = Terminal::start(home(), 24, 80);
    terminal.wait_for_shell();
    terminal.type_line("printf '%s%s\\n' stacked above");
    terminal.wait_for("stackedabove");

    terminal.command("\"");

    let areas = terminal.split_areas(Direction::TopBottom);
    let (top, bottom) = (areas[0], areas[1]);
    assert!(bottom.y > top.y, "stacked: {areas:?}");

    terminal.wait_for_in(top, "stackedabove");
    terminal.type_until_in(bottom, "printf '%s%s\\n' stacked below", "stackedbelow");
}

#[test]
fn the_focus_moves_back_to_the_pane_that_was_split() {
    let terminal = Terminal::start(home(), 24, 80);
    terminal.wait_for_shell();
    terminal.command("%");

    let areas = terminal.split_areas(Direction::LeftRight);
    let (left, right) = (areas[0], areas[1]);
    // The new pane's shell has to be ready before the focus leaves it, so
    // that nothing typed afterwards is swallowed by a shell still starting.
    terminal.type_until_in(
        right,
        "printf '%s%s\\n' right-pane -ready",
        "right-pane-ready",
    );

    terminal.command("h");
    terminal.type_until_in(
        left,
        "printf '%s%s\\n' back-on-the -left",
        "back-on-the-left",
    );
}

#[test]
fn a_new_tab_is_listed_and_the_first_one_can_be_chosen_again() {
    let terminal = Terminal::start(home(), 24, 80);
    terminal.wait_for_shell();
    terminal.type_line("printf '%s%s\\n' on-the-first tab");
    terminal.wait_for("on-the-firsttab");

    terminal.command("c");

    // Both tabs are listed, numbered from one, with the new one shown.
    terminal.wait_for_in(terminal.tabs_area(), "2:shell");
    let pane = terminal.pane_area();
    terminal.wait_until("the first tab's output to be hidden", || {
        !terminal.region(pane).contains("on-the-firsttab")
    });

    terminal.command("1");
    terminal.wait_for_in(pane, "on-the-firsttab");
}

#[test]
fn a_new_workspace_is_listed_in_the_sidebar() {
    let terminal = Terminal::start(home(), 24, 80);
    terminal.wait_for_shell();

    // One workspace, and it is the one being shown.
    let sidebar = terminal.sidebar_area();
    terminal.wait_for_in(sidebar, "workspaces");
    terminal.wait_for_in(sidebar, "\u{25cf}");
    assert!(!terminal.region(sidebar).contains('\u{25cb}'));

    terminal.command("w");

    // The one that was there is still listed, no longer marked as shown.
    terminal.wait_for_in(sidebar, "\u{25cb}");
}

#[test]
fn hiding_the_sidebar_gives_its_columns_to_the_pane() {
    let terminal = Terminal::start(home(), 24, 80);
    terminal.wait_for_shell();
    let narrow = terminal.pane_area();
    terminal.type_until("stty size", &format!("{} {}", narrow.rows, narrow.cols));

    terminal.command("z");

    let wide = Chrome::of(terminal.size(), false).content.inset(BORDER);
    assert!(wide.cols > narrow.cols);
    terminal.type_until("stty size", &format!("{} {}", wide.rows, wide.cols));
}

#[test]
fn a_session_comes_back_after_the_daemon_is_restarted() {
    let home = home();
    let mut first = Terminal::start(Arc::clone(&home), 24, 80);
    first.wait_for_shell();
    first.type_line("printf '%s%s\\n' written-before the-restart");
    first.wait_for("written-beforethe-restart");

    first.command("%");
    let areas = first.split_areas(Direction::LeftRight);
    first.type_until_in(
        areas[1],
        "printf '%s%s\\n' in-the second-pane",
        "in-thesecond-pane",
    );

    // Stopping the daemon takes the client with it, and otag reports no
    // error: the session is over, not broken.
    first.kill_server();
    assert!(
        first.wait_for_exit(),
        "otag stops cleanly when the daemon does"
    );

    // A new daemon starts and puts the session back.
    let second = Terminal::start(home, 24, 80);
    second.wait_for_in(areas[0], "written-beforethe-restart");
    second.wait_for_in(areas[1], "in-thesecond-pane");
    second.wait_for_in(areas[0], "restored");
    second.wait_for_in(areas[1], "restored");

    // Both panes are running again, so the restored session can be used.
    second.type_until_in(
        areas[1],
        "printf '%s%s\\n' still working-after-the-restart",
        "stillworking-after-the-restart",
    );
}

#[test]
fn a_restored_session_keeps_its_tabs_and_which_one_was_shown() {
    let home = home();
    let mut first = Terminal::start(Arc::clone(&home), 24, 80);
    first.wait_for_shell();

    first.command("c");
    first.wait_for_in(first.tabs_area(), "2:shell");
    first.type_until("printf '%s%s\\n' on-the-second tab", "on-the-secondtab");

    first.kill_server();
    assert!(first.wait_for_exit(), "otag stops cleanly");

    let second = Terminal::start(home, 24, 80);
    second.wait_for_in(second.tabs_area(), "2:shell");
    second.wait_for_in(second.pane_area(), "on-the-secondtab");
}

#[test]
fn a_session_that_ended_is_not_restored() {
    // The shell exited, so there is nothing to come back to: the next otag
    // starts a session of its own rather than a dead one.
    let home = home();
    let mut first = Terminal::start(Arc::clone(&home), 24, 80);
    first.wait_for_shell();
    first.type_line("printf '%s%s\\n' before the-exit");
    first.wait_for("beforethe-exit");
    first.type_line("exit");
    assert!(first.wait_for_exit(), "otag stops when the shell does");

    first.kill_server();
    let second = Terminal::start(home, 24, 80);
    second.wait_for_shell();
    assert!(
        !second.pane_text().contains("beforethe-exit"),
        "a session that ended came back.\nscreen:\n{}",
        second.text()
    );
}
