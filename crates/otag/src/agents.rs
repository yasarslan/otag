//! Noticing the agents running in the panes.
//!
//! Hooks are the better source of what an agent is doing, but they only
//! exist once the user has installed them, and they say nothing about an
//! agent that was simply started in a shell. So the daemon also looks: once
//! a second it lists the processes, finds the ones below each pane's shell,
//! and asks the adapters whether any of them is an agent they know.
//!
//! Everything here is a guess that may be wrong, so it is cheap and it never
//! fails: one `ps` per second for all panes at once, no reading of an agent's
//! memory or file descriptors, and a pane whose processes cannot be read is
//! left as it was.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use otag_agents::{AgentKind, adapter_for_process};
use otag_core::pane::PaneId;
use otag_core::session::{AgentPane, Session};

use crate::usage;

/// How often the processes are listed.
///
/// A second is below what a user notices and costs one short-lived process,
/// which is less than the agent itself spends on one frame of its spinner.
const INTERVAL: Duration = Duration::from_secs(1);

/// The arguments that list what is running, in the order the fields are read.
///
/// The process's own name is deliberately not asked for: macOS cuts it to
/// sixteen characters, which turns every agent installed below a long path
/// into something unrecognisable. The command line is not cut, and its first
/// word is the program.
const PS: [&str; 3] = ["-ax", "-o", "pid=,ppid=,args="];

/// Watch the panes for agents until the daemon stops.
pub async fn keep_watching(session: Arc<Session>, shared: Arc<usage::Shared>) {
    let home = home_dir();
    let mut ticks = tokio::time::interval(INTERVAL);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut read = Read::default();

    loop {
        ticks.tick().await;
        watch_once(&session, home.as_deref(), &shared, &mut read).await;
    }
}

/// When each pane's session file was last read, so that an unchanged file is
/// not parsed again every second.
type Read = HashMap<PaneId, SystemTime>;

/// One pass: find the agents, what they are doing, the sessions they resume
/// by, and what they have spent.
async fn watch_once(
    session: &Arc<Session>,
    home: Option<&Path>,
    shared: &Arc<usage::Shared>,
    read: &mut Read,
) {
    let panes = session.agent_panes();
    if panes.is_empty() {
        return;
    }

    let processes = match list_processes().await {
        Some(processes) => processes,
        // Without a process list there is nothing to conclude. What otag
        // already knew about each pane stays as it was, because a missing
        // `ps` is not evidence that the agents are gone.
        None => return,
    };
    let tree = Tree::of(&processes);
    let ids: Vec<PaneId> = panes.iter().map(|pane| pane.id).collect();
    let mut fresh = false;

    for pane in panes {
        let found = pane.pid.and_then(|pid| tree.agent_in(pid));
        if found != pane.agent {
            session.set_agent(pane.id, found).await;
        }
        let Some(agent) = found else {
            continue;
        };

        if pane.trusts_the_screen
            && let Some(status) = agent.adapter().detect_status(&pane.screen)
        {
            session.set_status(pane.id, status, false).await;
        }

        if pane.session_id.is_none() {
            adopt_session(session, home, &pane, agent).await;
            continue;
        }
        if read_session(session, shared, &pane, agent, read).await {
            fresh = true;
        }
    }

    if fresh {
        // What each pane spent has changed, and with it what otag can say
        // about an agent that reports no percentage of its own.
        for (agent, tokens) in usage::per_agent(session) {
            shared.record_tokens(agent, tokens);
        }
        usage::publish(session, shared).await;
    }
    read.retain(|pane, _| ids.contains(pane));
}

/// Read one pane's session file, when it has changed since otag last did.
///
/// Returns whether anything new was learned. The file belongs to the agent
/// and may be large, so it is parsed on a thread that may block, and only
/// when the filesystem says it has moved on.
async fn read_session(
    session: &Arc<Session>,
    shared: &Arc<usage::Shared>,
    pane: &AgentPane,
    agent: AgentKind,
    read: &mut Read,
) -> bool {
    let Some(path) = pane.session_path.clone() else {
        return false;
    };
    let Ok(modified) = std::fs::metadata(&path).and_then(|data| data.modified()) else {
        return false;
    };
    if read.get(&pane.id) == Some(&modified) {
        return false;
    }
    read.insert(pane.id, modified);

    let parsed = tokio::task::spawn_blocking(move || agent.adapter().read_transcript(&path)).await;
    let transcript = match parsed {
        Ok(Ok(transcript)) => transcript,
        Ok(Err(error)) => {
            tracing::debug!(%error, "a session file could not be read");
            return false;
        }
        Err(error) => {
            tracing::debug!(%error, "reading a session file was cancelled");
            return false;
        }
    };

    // What the agent says about its own limits always wins over what otag
    // counted; both are kept, and the report decides which to show. The
    // session it came from is the one this pane is running.
    shared.record_limits(agent, &transcript.limits, pane.session_id.as_deref());
    session.set_tokens(pane.id, transcript.tokens).await;
    // Without the size of the model's context, a count of context tokens
    // cannot be drawn as a share of anything.
    if let Some(window) = transcript.context_window {
        session.set_context_window(pane.id, window).await;
    }
    true
}

/// Find the agent session a pane is running, and how to resume it.
async fn adopt_session(
    session: &Arc<Session>,
    home: Option<&Path>,
    pane: &AgentPane,
    agent: AgentKind,
) {
    let (Some(home), Some(cwd)) = (home, pane.cwd.as_deref()) else {
        return;
    };
    let adapter = agent.adapter();
    let Some(found) = adapter.find_session_for(home, cwd, Some(pane.started_at)) else {
        return;
    };

    // The command that brings this pane back is the one that continues the
    // session, so a restored pane carries on rather than starting over.
    let resume = adapter.resume_command(&found);
    session.set_session_path(pane.id, found.path).await;
    session.set_session_id(pane.id, found.id).await;
    session.set_restore_command(pane.id, resume).await;
}

/// One running process, as `ps` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Process {
    pid: u32,
    parent: u32,
    /// The program's own name, without its directory.
    exe: String,
    /// What it was started with, the program itself first.
    argv: Vec<String>,
}

/// The processes, arranged so that a pane's own process and everything below
/// it can be walked.
#[derive(Debug, Default)]
struct Tree<'a> {
    by_pid: HashMap<u32, &'a Process>,
    children: HashMap<u32, Vec<&'a Process>>,
}

impl<'a> Tree<'a> {
    /// Arrange a process list by identifier and by parent.
    fn of(processes: &'a [Process]) -> Self {
        let mut by_pid = HashMap::with_capacity(processes.len());
        let mut children: HashMap<u32, Vec<&'a Process>> = HashMap::new();
        for process in processes {
            by_pid.insert(process.pid, process);
            children.entry(process.parent).or_default().push(process);
        }
        Self { by_pid, children }
    }

    /// The agent this process is, or is running, if either.
    ///
    /// A pane usually runs the user's shell with the agent started inside
    /// it, so the whole subtree is searched. The pane's own process is
    /// looked at first, because a pane otag restored runs the agent itself
    /// rather than a shell.
    fn agent_in(&self, root: u32) -> Option<AgentKind> {
        let mut queue = vec![root];
        let mut seen = 0usize;

        if let Some(process) = self.by_pid.get(&root)
            && let Some(adapter) = adapter_for_process(&process.exe, &process.argv)
        {
            return Some(adapter.kind());
        }

        while let Some(pid) = queue.pop() {
            // A process table is a tree, but it is read in pieces and a
            // parent may be reported after it was reaped; this stops a cycle
            // from becoming a loop.
            seen += 1;
            if seen > MAX_PROCESSES_PER_PANE {
                return None;
            }

            for child in self.children.get(&pid).into_iter().flatten() {
                if let Some(adapter) = adapter_for_process(&child.exe, &child.argv) {
                    return Some(adapter.kind());
                }
                queue.push(child.pid);
            }
        }
        None
    }
}

/// How many processes below one pane are looked at before giving up.
const MAX_PROCESSES_PER_PANE: usize = 10_000;

/// Ask the operating system what is running.
async fn list_processes() -> Option<Vec<Process>> {
    let output = tokio::process::Command::new("ps")
        .args(PS)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_ps(&String::from_utf8_lossy(&output.stdout)))
}

/// Read what `ps -o pid=,ppid=,args=` printed.
///
/// Two numbers and then the command line, which runs to the end of the line
/// and may contain anything at all. A line that does not start with two
/// numbers is skipped rather than guessed at: that is the header `ps` prints
/// when it was asked for one, and anything a command line wrapped onto a
/// second line.
fn parse_ps(text: &str) -> Vec<Process> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let parent = fields.next()?.parse().ok()?;
            let argv: Vec<String> = fields.map(str::to_owned).collect();
            // The program is the first word of the command line. A process
            // that reports no command line at all — a kernel thread, or one
            // that rewrote its own arguments — is kept, so that the panes
            // below it can still be walked.
            let exe = argv.first().cloned().unwrap_or_default();
            Some(Process {
                pid,
                parent,
                exe,
                argv,
            })
        })
        .collect()
}

/// The user's home directory, where the agents keep their sessions.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A process list in the shape macOS and Linux both print.
    const SAMPLE: &str = "\
  501     1 -zsh
  502   501 node /usr/local/bin/claude
  503   502 sh -c rg --files
  601     1 -bash
  602   601 vim notes-about-claude.md
";

    fn processes() -> Vec<Process> {
        parse_ps(SAMPLE)
    }

    #[test]
    fn a_process_list_is_read_field_by_field() {
        let processes = processes();
        assert_eq!(processes.len(), 5);
        assert_eq!(processes[0].pid, 501);
        assert_eq!(processes[0].parent, 1);
        assert_eq!(processes[0].exe, "-zsh");
        assert_eq!(processes[0].argv, vec!["-zsh".to_owned()]);
    }

    #[test]
    fn lines_that_are_not_processes_are_skipped() {
        let processes = parse_ps("  PID  PPID ARGS\nnot a process line\n\n  7 1 /bin/sh\n");
        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].pid, 7);
    }

    #[test]
    fn a_pane_running_the_agent_itself_is_found() {
        // What a pane otag restored looks like: the agent is the pane's own
        // process, not a child of a shell.
        let processes = parse_ps("  20     1 claude --resume abc\n");
        let tree = Tree::of(&processes);
        assert_eq!(tree.agent_in(20), Some(AgentKind::Claude));
    }

    #[test]
    fn an_agent_started_inside_a_shell_is_found() {
        let processes = processes();
        let tree = Tree::of(&processes);
        assert_eq!(tree.agent_in(501), Some(AgentKind::Claude));
    }

    #[test]
    fn an_agent_several_levels_down_is_still_found() {
        let processes = parse_ps(
            "  10     1 -zsh\n  11    10 sh wrapper\n  12    11 /usr/bin/codex --full-auto\n",
        );
        let tree = Tree::of(&processes);
        assert_eq!(tree.agent_in(10), Some(AgentKind::Codex));
    }

    #[test]
    fn a_shell_with_no_agent_in_it_reports_none() {
        let processes = processes();
        let tree = Tree::of(&processes);
        assert_eq!(
            tree.agent_in(601),
            None,
            "a file named after an agent is not the agent"
        );
    }

    #[test]
    fn a_pane_that_is_not_in_the_list_reports_none() {
        let processes = processes();
        let tree = Tree::of(&processes);
        assert_eq!(tree.agent_in(9999), None);
    }

    #[test]
    fn a_parent_that_is_its_own_child_does_not_loop() {
        // `ps` output is read as text and may be inconsistent; a cycle in it
        // must not hang the daemon.
        let processes = parse_ps("  10    10 -zsh\n");
        let tree = Tree::of(&processes);
        assert_eq!(tree.agent_in(10), None);
    }

    #[tokio::test]
    async fn the_real_process_list_can_be_read() {
        let processes = list_processes().await.expect("ps works on this machine");
        assert!(
            processes
                .iter()
                .any(|process| process.pid == std::process::id()),
            "the test's own process is running"
        );
    }

    #[tokio::test]
    async fn a_pass_over_a_session_without_panes_does_nothing() {
        let session = Session::new(otag_core::pane::PaneSpec::shell());
        let shared = usage::Shared::new();
        watch_once(&session, None, &shared, &mut Read::default()).await;
        assert!(session.is_empty());
    }

    /// A program that looks like Claude Code to otag: the name of the
    /// process is the agent's, and what it draws is what the agent draws
    /// while it is working.
    const FAKE_AGENT: &str = "#!/bin/sh\nprintf 'working (esc to interrupt)\\n'\nsleep 30\n";

    #[tokio::test]
    async fn a_pane_running_an_agent_is_noticed_with_what_it_is_doing() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let fake = dir.path().join(otag_agents::claude::PROGRAM);
        std::fs::write(&fake, FAKE_AGENT).expect("a fake agent");
        set_executable(&fake);

        let session = Session::new(otag_core::pane::PaneSpec::command([&fake]));
        session
            .attach(otag_core::pane::PaneSize::new(24, 80), Some(dir.path()))
            .await
            .expect("a pane");

        // The pane has to draw its first line and `ps` has to see it, so the
        // pass is repeated rather than waited for with a fixed delay.
        let shared = usage::Shared::new();
        let mut read = Read::default();
        let mut found = None;
        for _ in 0..50 {
            watch_once(&session, Some(dir.path()), &shared, &mut read).await;
            let panes = session.agent_panes();
            let pane = panes.first().expect("the pane is still there");
            if pane.agent.is_some() && pane.status != otag_agents::PaneStatus::Idle {
                found = Some(pane.clone());
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        let pane = found.expect("the agent in the pane is noticed");
        assert_eq!(pane.agent, Some(AgentKind::Claude));
        assert_eq!(
            pane.status,
            otag_agents::PaneStatus::Working,
            "the agent's own screen says it is working"
        );

        // What the interface is told carries it too.
        let view = session.view();
        assert_eq!(
            view.panes.first().map(|pane| pane.agent),
            Some(Some(AgentKind::Claude))
        );
        assert_eq!(
            view.tabs.first().map(|tab| tab.status),
            Some(otag_agents::PaneStatus::Working),
            "the tab shows what the pane in it is doing"
        );
    }

    /// Make a file runnable, which a fake agent has to be.
    fn set_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(path)
            .expect("the file exists")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).expect("the file can be made runnable");
    }

    /// Where the redacted samples of real agent files live.
    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures");

    /// A pane as the watcher sees it, pointed at a session file.
    fn pane_with_session(path: &Path) -> AgentPane {
        AgentPane {
            id: otag_core::pane::PaneId::FIRST,
            pid: None,
            cwd: None,
            started_at: SystemTime::now(),
            agent: Some(AgentKind::Codex),
            status: otag_agents::PaneStatus::Working,
            session_id: Some("a-session".to_owned()),
            session_path: Some(path.to_path_buf()),
            tokens: otag_agents::Tokens::default(),
            trusts_the_screen: true,
            screen: String::new(),
        }
    }

    #[tokio::test]
    async fn what_a_rollout_file_says_about_the_limits_reaches_the_usage_report() {
        let session = Session::new(otag_core::pane::PaneSpec::shell());
        let shared = usage::Shared::new();
        let path = Path::new(FIXTURES).join("codex-rollout.jsonl");
        let pane = pane_with_session(&path);
        let mut read = Read::default();

        assert!(
            read_session(&session, &shared, &pane, AgentKind::Codex, &mut read).await,
            "the file was read"
        );

        let report = shared.report();
        let codex: Vec<Option<u16>> = report
            .rows
            .iter()
            .filter(|row| row.agent == AgentKind::Codex)
            .map(otag_features::usage::Row::used_pct)
            .collect();
        assert_eq!(
            codex,
            vec![Some(36), Some(9)],
            "both windows the rollout carries"
        );
    }

    #[tokio::test]
    async fn a_session_file_that_has_not_changed_is_not_read_again() {
        let session = Session::new(otag_core::pane::PaneSpec::shell());
        let shared = usage::Shared::new();
        let path = Path::new(FIXTURES).join("codex-rollout.jsonl");
        let pane = pane_with_session(&path);
        let mut read = Read::default();

        assert!(read_session(&session, &shared, &pane, AgentKind::Codex, &mut read).await);
        assert!(
            !read_session(&session, &shared, &pane, AgentKind::Codex, &mut read).await,
            "parsing a transcript every second would be the daemon's whole budget"
        );
    }

    #[tokio::test]
    async fn a_pane_with_no_session_file_is_not_read() {
        let session = Session::new(otag_core::pane::PaneSpec::shell());
        let shared = usage::Shared::new();
        let mut pane = pane_with_session(Path::new("/no/such/file.jsonl"));
        let mut read = Read::default();

        assert!(!read_session(&session, &shared, &pane, AgentKind::Codex, &mut read).await);

        pane.session_path = None;
        assert!(!read_session(&session, &shared, &pane, AgentKind::Codex, &mut read).await);
        assert!(shared.report().is_empty());
    }
}
