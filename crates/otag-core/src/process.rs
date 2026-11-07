//! Process ownership snapshots for shutdown and recovery. Never select agents
//! by name: only descendants of processes otag actually started are owned.

use std::collections::HashSet;
use std::io;
use std::process::Command;
use std::time::Duration;

use nix::sys::signal::{Signal, kill, killpg};
use nix::unistd::{Pid, Uid};

/// One process identity. Start time prevents signalling a reused PID.
#[derive(Clone, Debug)]
pub struct Process {
    /// Operating system process identifier.
    pub pid: i32,
    /// Parent process identifier.
    pub parent: i32,
    /// Job-control process group.
    pub group: i32,
    /// Process owner.
    pub uid: u32,
    /// Start time reported by the operating system.
    pub started: String,
    /// Command line, used only to identify the daemon and display recovery.
    pub command: String,
}

impl Process {
    /// Whether this is the same process, rather than a reused identifier.
    pub fn same(&self, other: &Self) -> bool {
        self.pid == other.pid && self.uid == other.uid && self.started == other.started
    }
}

/// Read processes owned by the current user. Failure is explicit: recovery
/// must never guess about ownership when the process table is unavailable.
pub fn table() -> io::Result<Vec<Process>> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid=,pgid=,uid=,lstart=,command="])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("cannot read process ownership with ps"));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse)
        .collect())
}

fn parse(line: &str) -> Option<Process> {
    let mut fields = line.split_whitespace();
    let pid = fields.next()?.parse().ok()?;
    let parent = fields.next()?.parse().ok()?;
    let group = fields.next()?.parse().ok()?;
    let uid = fields.next()?.parse().ok()?;
    if uid != Uid::current().as_raw() {
        return None;
    }
    let started = fields.by_ref().take(5).collect::<Vec<_>>().join(" ");
    let command = fields.collect::<Vec<_>>().join(" ");
    if command.is_empty() {
        return None;
    }
    Some(Process {
        pid,
        parent,
        group,
        uid,
        started,
        command,
    })
}

/// Take an ownership snapshot including the roots and all their descendants.
pub fn descendants(all: &[Process], roots: &[i32]) -> Vec<Process> {
    let mut owned: HashSet<_> = roots.iter().copied().collect();
    loop {
        let before = owned.len();
        for process in all {
            if owned.contains(&process.parent) {
                owned.insert(process.pid);
            }
        }
        if owned.len() == before {
            break;
        }
    }
    all.iter()
        .filter(|p| owned.contains(&p.pid))
        .cloned()
        .collect()
}

/// Signal only identities still present. Entire owned process groups are
/// signalled together; a shared group is handled by individual PID instead.
pub fn signal(processes: &[Process], signal: Signal) -> io::Result<()> {
    let current = table()?;
    let live: Vec<_> = processes
        .iter()
        .filter(|p| current.iter().any(|c| p.same(c)))
        .collect();
    let ids: HashSet<_> = live.iter().map(|p| p.pid).collect();
    let mut groups = HashSet::new();
    for process in &live {
        if process.group > 1
            && current
                .iter()
                .filter(|p| p.group == process.group)
                .all(|p| ids.contains(&p.pid))
        {
            if groups.insert(process.group) {
                let _ = killpg(Pid::from_raw(process.group), signal);
            }
        } else {
            let _ = kill(Pid::from_raw(process.pid), signal);
        }
    }
    Ok(())
}

/// Stop owned pane trees, including foreground jobs in separate groups.
pub fn terminate(roots: &[i32]) -> io::Result<()> {
    let owned = descendants(&table()?, roots);
    terminate_owned(owned)
}

/// Stop pane trees whose roots are still direct children of this daemon.
pub(crate) fn terminate_panes(pids: &[u32]) -> io::Result<()> {
    let all = table()?;
    let daemon = std::process::id() as i32;
    let roots: Vec<_> = all
        .iter()
        .filter(|process| process.parent == daemon && pids.contains(&(process.pid as u32)))
        .map(|process| process.pid)
        .collect();
    terminate_owned(descendants(&all, &roots))
}

/// Stop every live descendant of a daemon, without relying on possibly stale
/// pane PIDs and without signalling the daemon itself.
pub fn terminate_children(daemon: i32) -> io::Result<()> {
    let owned = descendants(&table()?, &[daemon])
        .into_iter()
        .filter(|p| p.pid != daemon)
        .collect();
    terminate_owned(owned)
}

fn terminate_owned(owned: Vec<Process>) -> io::Result<()> {
    if owned.is_empty() {
        return Ok(());
    }
    signal(&owned, Signal::SIGHUP)?;
    std::thread::sleep(Duration::from_millis(300));
    // Include children forked by a signal handler while their parent lived.
    let current = table()?;
    let live_roots: Vec<_> = owned
        .iter()
        .filter(|p| current.iter().any(|c| p.same(c)))
        .map(|p| p.pid)
        .collect();
    let mut remaining = owned;
    remaining.extend(descendants(&current, &live_roots));
    signal(&remaining, Signal::SIGKILL)
}
