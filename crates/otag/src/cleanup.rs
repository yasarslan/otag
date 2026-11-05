//! Recover only verified otag daemons and their owned descendants.

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use nix::sys::signal::Signal;
use otag_core::paths::Dirs;
use otag_core::process::{self, Process};

fn daemon_socket(command: &str) -> Option<PathBuf> {
    let (exe, arguments) = command.split_once(" daemon ")?;
    if Path::new(exe).file_name()? != "otag" {
        return None;
    }
    let (_, socket) = arguments.split_once("--socket ")?;
    Some(PathBuf::from(socket.split(" --").next()?.trim()))
}

fn daemon_state(command: &str) -> Option<PathBuf> {
    let (_, state) = command.split_once("--state ")?;
    Some(PathBuf::from(state.split(" --").next()?.trim()))
}

fn verified_daemon(process: &Process) -> bool {
    let Ok(output) = Command::new("ps")
        .args(["-p", &process.pid.to_string(), "-o", "comm="])
        .output()
    else {
        return false;
    };
    output.status.success()
        && Path::new(String::from_utf8_lossy(&output.stdout).trim())
            .file_name()
            .is_some_and(|name| name == "otag")
}

fn same_path(a: &Path, b: &Path) -> bool {
    a == b
        || match (a.canonicalize(), b.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}

/// List nondefault daemons and stop only the processes in their ownership
/// trees. Old leaked daemons are frozen before termination so they cannot
/// overwrite the real database with an accidental restored session.
pub fn run(yes: bool, scope: Option<&Path>) -> Result<()> {
    let mut dirs = Dirs::from_env();
    dirs.socket_override = None;
    dirs.state_override = None;
    let default = dirs.socket_path()?;
    let all = process::table().context("cannot inspect otag daemon ownership")?;
    let mut caller_ancestors = std::collections::HashSet::new();
    let mut caller = std::process::id() as i32;
    while let Some(process) = all.iter().find(|p| p.pid == caller) {
        if !caller_ancestors.insert(caller) {
            break;
        }
        caller = process.parent;
    }
    let protected: Vec<_> = all
        .iter()
        .filter(|p| daemon_socket(&p.command).is_some_and(|socket| same_path(&socket, &default)))
        .map(|p| p.pid)
        .collect();
    let protected = process::descendants(&all, &protected);
    let stray: Vec<_> = all.iter().filter(|p| p.pid != std::process::id() as i32 && !protected.iter().any(|c| c.pid == p.pid))
        .filter(|p| daemon_socket(&p.command).is_some_and(|socket| !same_path(&socket, &default)) && verified_daemon(p))
        .filter(|p| scope.is_none_or(|state| daemon_state(&p.command).is_some_and(|found| same_path(&found, state))))
        .filter(|p| {
            if caller_ancestors.contains(&p.pid) {
                eprintln!("leaving daemon {}: it owns this cleanup command; run cleanup from an independent terminal to stop it", p.pid);
                false
            } else { true }
        }).cloned().collect();
    if stray.is_empty() {
        println!("no stray otag daemons");
        return Ok(());
    }
    for daemon in &stray {
        println!(
            "daemon {}: {}",
            daemon.pid,
            daemon_socket(&daemon.command).unwrap_or_default().display()
        );
        for child in process::descendants(&all, &[daemon.pid])
            .iter()
            .filter(|p| p.pid != daemon.pid)
        {
            let exe = child.command.split_whitespace().next().unwrap_or("process");
            println!("  child {} (group {}): {}", child.pid, child.group, exe);
        }
    }
    if !yes {
        if !io::stdin().is_terminal() {
            bail!("cleanup needs confirmation; rerun with --yes to stop the listed daemons");
        }
        print!(
            "Stop these {} stray daemons and their children? [y/N] ",
            stray.len()
        );
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("nothing stopped");
            return Ok(());
        }
    }
    for daemon in &stray {
        // Revalidate identity before each signal. Freeze only the daemon,
        // not a potentially shared process group.
        let current = process::table()?;
        if !current.iter().any(|p| daemon.same(p)) {
            continue;
        }
        let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(daemon.pid), Signal::SIGSTOP);
        let current = process::table()?;
        let children: Vec<_> = process::descendants(&current, &[daemon.pid])
            .into_iter()
            .filter(|p| p.pid != daemon.pid)
            .collect();
        process::signal(&children, Signal::SIGHUP)?;
        process::signal(&children, Signal::SIGCONT)?;
        std::thread::sleep(Duration::from_millis(300));
        process::signal(&children, Signal::SIGKILL)?;
        process::signal(std::slice::from_ref(daemon), Signal::SIGKILL)?;
    }
    println!("stopped {} stray otag daemons", stray.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_selection_handles_spaces_and_ignores_other_programs() {
        assert_eq!(
            daemon_socket(
                "/opt/bin/otag daemon --socket /tmp/test space/otag.sock --state /tmp/state"
            ),
            Some(PathBuf::from("/tmp/test space/otag.sock"))
        );
        assert!(daemon_socket("/opt/bin/other daemon --socket /tmp/test.sock").is_none());
        assert!(daemon_socket("otag ping --socket /tmp/test.sock").is_none());
    }
}
