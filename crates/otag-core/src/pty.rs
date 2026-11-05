//! Running a pane's program on a pseudo-terminal.
//!
//! The pseudo-terminal interface is blocking, so each pane gets two plain
//! threads — one reading its output, one writing its input — that talk to the
//! async world through channels. Everything else about a pane stays in the
//! daemon's runtime.

use std::io::{Read, Write};

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize};
use tokio::sync::mpsc;

use crate::pane::{COLORTERM, PaneExit, PaneId, PaneSize, PaneSpec, TERM};
use crate::{Error, Result};

/// Size of one read from a pane. Large enough that a program printing fast
/// does not cause a syscall per line.
const READ_CHUNK: usize = 8 * 1024;

/// How many unread output chunks a pane may queue before its program is made
/// to wait. This is the backpressure that keeps a noisy pane from growing the
/// daemon's memory without bound.
const OUTPUT_QUEUE: usize = 64;

/// How many unwritten input chunks a pane may queue.
const INPUT_QUEUE: usize = 64;

/// Something that happened on a pane's pseudo-terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyEvent {
    /// The program wrote output.
    Output(Vec<u8>),
    /// The program ended. Always the last event.
    Exited(PaneExit),
}

/// A pane's pseudo-terminal and the process running on it.
///
/// Dropping this kills the process: a pane that no longer exists must not
/// leave a program behind.
pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    input: mpsc::Sender<Vec<u8>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    size: PaneSize,
    pid: Option<u32>,
}

impl Pty {
    /// Run `spec` on a new pseudo-terminal of the given size.
    ///
    /// The returned receiver yields the program's output and, last, its exit.
    pub fn spawn(
        id: PaneId,
        spec: &PaneSpec,
        size: PaneSize,
    ) -> Result<(Self, mpsc::Receiver<PtyEvent>)> {
        let pair = portable_pty::native_pty_system()
            .openpty(pty_size(size))
            .map_err(|source| Error::pty("cannot open a pseudo-terminal for a new pane", source))?;

        let command = build_command(id, spec)?;
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|source| Error::pty(spawn_failure_context(spec), source))?;
        // The master only sees end-of-file once every handle on the other
        // side is closed, so this one must go now that the child has it.
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|source| Error::pty("cannot read from a new pane", source))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|source| Error::pty("cannot write to a new pane", source))?;
        let killer = child.clone_killer();
        // Kept so that otag can look at what the pane is running: an agent
        // started inside the shell is a descendant of this process.
        let pid = child.process_id();

        let (events_tx, events_rx) = mpsc::channel(OUTPUT_QUEUE);
        let (input_tx, input_rx) = mpsc::channel(INPUT_QUEUE);
        spawn_reader(id, reader, child, events_tx);
        spawn_writer(id, writer, input_rx);

        Ok((
            Self {
                master: pair.master,
                input: input_tx,
                killer,
                size,
                pid,
            },
            events_rx,
        ))
    }

    /// Current size of the pseudo-terminal.
    #[must_use]
    pub const fn size(&self) -> PaneSize {
        self.size
    }

    /// Identifier of the process started on it, when the system gave one.
    #[must_use]
    pub const fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Send `bytes` to the program's input.
    ///
    /// Waits while the program is not reading, which is what keeps a paste
    /// larger than the kernel's buffer from being lost.
    pub async fn write(&mut self, bytes: Vec<u8>) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.input
            .send(bytes)
            .await
            .map_err(|_| Error::PaneGone { id: None })
    }

    /// Tell the program its terminal changed size.
    pub fn resize(&mut self, size: PaneSize) -> Result<()> {
        if size == self.size {
            return Ok(());
        }
        self.master
            .resize(pty_size(size))
            .map_err(|source| Error::pty("cannot resize a pane", source))?;
        self.size = size;
        Ok(())
    }

    /// Ask the program to stop.
    pub fn kill(&mut self) {
        // portable-pty's killer targets only the initial shell. Foreground
        // jobs may have their own groups; daemon shutdown also walks those
        // descendants before the shell can disappear.
        if let Some(pid) = self.pid {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        if let Err(error) = self.killer.kill() {
            // A process that has already exited cannot be killed, which is
            // the common case here and not worth reporting.
            tracing::debug!(%error, "cannot kill a pane's process");
        }
    }
}

impl std::fmt::Debug for Pty {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The pseudo-terminal handles are opaque, so report what is useful.
        formatter
            .debug_struct("Pty")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Translate a pane size into the pseudo-terminal's own shape. otag does not
/// track pixel sizes, so programs that ask get zero, as they do under other
/// multiplexers.
const fn pty_size(size: PaneSize) -> PtySize {
    PtySize {
        rows: size.rows(),
        cols: size.cols(),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Build the command for a pane, including the environment every pane gets.
fn build_command(id: PaneId, spec: &PaneSpec) -> Result<CommandBuilder> {
    let mut command = match spec.argv() {
        [] => CommandBuilder::new_default_prog(),
        [program, arguments @ ..] => {
            let mut command = CommandBuilder::new(program);
            command.args(arguments);
            command
        }
    };

    // What otag's screen emulation implements, not what the user's own
    // terminal happens to be: the program draws into otag, not into it.
    command.env("TERM", TERM);
    command.env("COLORTERM", COLORTERM);
    command.env(crate::pane::PANE_ID_ENV, id.to_string());

    for (key, value) in spec.env() {
        command.env(key, value);
    }

    if let Some(cwd) = spec.cwd() {
        if !cwd.is_dir() {
            return Err(Error::Location {
                what: "pane working directory",
                reason: format!("{} is not a directory", cwd.display()),
            });
        }
        command.cwd(cwd);
    }

    Ok(command)
}

fn spawn_failure_context(spec: &PaneSpec) -> String {
    match spec.argv().first() {
        Some(program) => format!("cannot start {} in a new pane", program.to_string_lossy()),
        None => "cannot start a shell in a new pane. Check that $SHELL is executable".to_owned(),
    }
}

/// Read the program's output until it ends, then report how it ended.
///
/// The output and the exit travel the same channel, in that order, so that
/// whoever watches a pane cannot learn that the program ended before it has
/// seen everything the program printed.
///
/// How the terminal reports the end differs between systems: closing the last
/// handle on the program's side gives end-of-file on some and `EIO` on Linux.
/// Both mean the same thing here — there is nothing more to read — and
/// neither says anything about what was read before, so output already in
/// hand is always passed on and only then is the exit reported.
fn spawn_reader(
    id: PaneId,
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    events: mpsc::Sender<PtyEvent>,
) {
    let name = format!("otag-pane-{id}-out");
    if let Err(error) = std::thread::Builder::new().name(name).spawn(move || {
        let mut buffer = vec![0_u8; READ_CHUNK];
        loop {
            match reader.read(&mut buffer) {
                // End of file: the program closed its side of the terminal.
                Ok(0) => break,
                Ok(count) => {
                    if events
                        .blocking_send(PtyEvent::Output(buffer[..count].to_vec()))
                        .is_err()
                    {
                        // Nobody is listening any more; the pane is gone.
                        return;
                    }
                }
                // A signal arrived mid-read; the terminal is still there.
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                // Anything else is the end of the output, not a reason to
                // drop what was read: on Linux the terminal answers `EIO`
                // once the program's side is closed, where other systems
                // answer end-of-file.
                Err(error) => {
                    tracing::debug!(%error, pane = %id, "pane output ended");
                    break;
                }
            }
        }

        let exit = match child.wait() {
            Ok(status) => PaneExit {
                code: status.signal().is_none().then(|| status.exit_code()),
                signal: status.signal().map(ToOwned::to_owned),
            },
            Err(error) => {
                tracing::warn!(%error, pane = %id, "cannot collect a pane's exit status");
                PaneExit {
                    code: None,
                    signal: None,
                }
            }
        };
        let _ = events.blocking_send(PtyEvent::Exited(exit));
    }) {
        tracing::error!(%error, pane = %id, "cannot start the thread reading a pane");
    }
}

/// Write everything sent to the pane until the pane is dropped.
fn spawn_writer(id: PaneId, mut writer: Box<dyn Write + Send>, mut input: mpsc::Receiver<Vec<u8>>) {
    let name = format!("otag-pane-{id}-in");
    if let Err(error) = std::thread::Builder::new().name(name).spawn(move || {
        while let Some(bytes) = input.blocking_recv() {
            if let Err(error) = writer.write_all(&bytes).and_then(|()| writer.flush()) {
                tracing::debug!(%error, pane = %id, "pane input ended");
                return;
            }
        }
    }) {
        tracing::error!(%error, pane = %id, "cannot start the thread writing to a pane");
    }
}
