//! Shared isolation and kill-on-drop guard for every binary integration test.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use otag_core::process;

pub struct TestHome(tempfile::TempDir);

impl TestHome {
    pub fn new() -> std::io::Result<Self> {
        let mut builder = tempfile::Builder::new();
        builder.prefix("otag-test-");
        let dir = match std::env::var_os("OTAG_TEST_RUN_DIR") {
            Some(root) => builder.tempdir_in(root)?,
            None => builder.tempdir()?,
        };
        Ok(Self(dir))
    }

    pub fn path(&self) -> &Path {
        self.0.path()
    }

    pub fn socket(&self) -> PathBuf {
        self.path().join("otag.sock")
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_otag"));
        configure(&mut command, self.path(), &self.socket());
        command
    }

    /// Shut down every daemon this fixture could have started, then assert
    /// it really exited. A deleted socket is not evidence of process exit.
    pub fn stop(&self) {
        let Ok(all) = process::table() else {
            panic!("cannot check for leaked test daemons");
        };
        let marker = format!("--state {}", self.path().join("state").display());
        let socket = format!("--socket {}", self.socket().display());
        let owned: Vec<_> = all
            .into_iter()
            .filter(|p| {
                p.command.contains(" daemon ")
                    && (p.command.contains(&marker) || p.command.contains(&socket))
            })
            .collect();
        for daemon in &owned {
            let _ = kill(Pid::from_raw(daemon.pid), Signal::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let current = process::table().expect("can check test daemon ownership");
            if !owned.iter().any(|p| current.iter().any(|c| p.same(c))) {
                return;
            }
            if Instant::now() >= deadline {
                for daemon in &owned {
                    let _ = process::terminate(&[daemon.pid]);
                }
                assert!(
                    std::thread::panicking(),
                    "test daemon leaked after shutdown: {owned:?}"
                );
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn configure(command: &mut Command, home: &Path, socket: &Path) {
    command
        .env("HOME", home)
        .env("SHELL", "/bin/sh")
        .env("OTAG_SOCKET", socket)
        .env("OTAG_STATE_DIR", home.join("state"))
        .env("XDG_STATE_HOME", home.join("xdg-state"))
        .env("XDG_RUNTIME_DIR", home.join("run"))
        .env("OTAG_EXIT_WITH_PARENT", std::process::id().to_string())
        .env("OTAG_NO_NOTIFY", "1")
        .env_remove("OTAG_LOG");
}

pub fn configure_pty(command: &mut portable_pty::CommandBuilder, home: &Path, socket: &Path) {
    command.env("HOME", home);
    command.env("OTAG_SOCKET", socket);
    command.env("OTAG_STATE_DIR", home.join("state"));
    command.env("XDG_STATE_HOME", home.join("xdg-state"));
    command.env("XDG_RUNTIME_DIR", home.join("run"));
    command.env("OTAG_EXIT_WITH_PARENT", std::process::id().to_string());
    command.env("OTAG_NO_NOTIFY", "1");
}

pub fn stop_child(child: &mut std::process::Child) {
    if child.try_wait().expect("can inspect child").is_some() {
        return;
    }
    let _ = kill(Pid::from_raw(child.id() as i32), Signal::SIGTERM);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if child.try_wait().expect("can reap child").is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = process::terminate(&[child.id() as i32]);
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        std::thread::panicking(),
        "test daemon required forced cleanup"
    );
}
