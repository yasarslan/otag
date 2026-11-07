//! Durable team roles and step ownership. Execution remains in the daemon.

use crate::pane::PaneId;
use otag_agents::AgentKind;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One requested role, in workflow order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Role {
    /// User-visible role name.
    pub name: String,
    /// No agent means a manual, editable shell.
    #[serde(default)]
    pub agent: Option<AgentKind>,
    /// Whether this role owns a write turn.
    #[serde(default)]
    pub can_edit: bool,
    /// Use an independent checkout when requested.
    #[serde(default)]
    pub worktree: bool,
}

impl Role {
    /// Shells always require a write turn, regardless of client input.
    pub fn editable(&self) -> bool {
        self.agent.is_none() || self.can_edit
    }

    /// Instruction supplied by otag rather than written by a read-only role.
    pub fn instruction(&self, index: usize) -> String {
        let task = match index {
            0 => {
                "Do not change code. Produce the task: goal, files, steps, tests, and acceptance criteria."
            }
            1 => "Implement the current task, run tests, and commit. Do not push.",
            2 => {
                "Do not change code. Review changes since the step checkpoint against the task, run tests, and list problems with file and line. Report failed tests explicitly."
            }
            _ if self.editable() => "Perform your role on the current task. Do not push.",
            _ => "Do not change code. Perform your role on the current task.",
        };
        format!("Role: {}. {task}", self.name)
    }
}

/// A role and its stable pane identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    /// Role configuration.
    pub role: Role,
    /// Stable pane identifier.
    pub pane: PaneId,
    /// Actual working directory, including an optional worktree.
    pub cwd: PathBuf,
    /// Original workflow position, recorded when the membership first shrinks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_index: Option<usize>,
}

impl Member {
    /// Position used by role policy and saved step records.
    pub fn role_index(&self, index: usize) -> usize {
        self.original_index.unwrap_or(index)
    }
}

/// A daemon-owned workflow, persisted with its session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Team {
    /// Checkpoints in execution order, including repeated review/coding cycles.
    #[serde(default)]
    pub steps: Vec<Step>,
    /// Shared-checkout ownership, present while an editor runs.
    #[serde(default)]
    pub write_root: Option<PathBuf>,
    /// Explicitly requested activation waiting for a write turn.
    #[serde(default)]
    pub queued: bool,
    /// FIFO ticket for editing activations.
    #[serde(default)]
    pub queue_ticket: u64,
    /// Role indices with observed read-only write attempts.
    #[serde(default)]
    pub violations: Vec<usize>,
    /// Confirmation is transient and cannot survive a daemon restart.
    #[serde(skip)]
    pub undo: Option<UndoPreview>,
    /// Monotonic process/step revision, used to invalidate stale UI actions.
    #[serde(default)]
    pub revision: u64,
    /// Stable team identity, derived from its first pane.
    pub id: u64,
    /// Shared project directory.
    pub project: PathBuf,
    /// Members in execution order.
    pub members: Vec<Member>,
    /// Role whose step may run.
    #[serde(default)]
    pub current: usize,
    /// Automatic progression; false is Manual.
    #[serde(default)]
    pub automatic: bool,
    /// Whether the current role has started.
    #[serde(default)]
    pub running: bool,
    /// Completed workflow.
    #[serde(default)]
    pub finished: bool,
}

/// A step checkpoint and the files observed when its turn ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// Monotonic execution identifier.
    pub number: u64,
    /// Index of the role that executed this step.
    pub role: usize,
    /// Working-tree root for this step.
    pub cwd: PathBuf,
    /// Git tree before the step.
    pub checkpoint: String,
    /// Git tree at completion; protects subsequent work from an older undo.
    #[serde(default)]
    pub ended: Option<String>,
    /// Whether the checkpoint has already been restored.
    #[serde(default)]
    pub undone: bool,
}

/// Exact files and tree identity shown by the undo confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoPreview {
    /// Team owning the checkpoint.
    pub team: u64,
    /// Stable role pane.
    pub pane: PaneId,
    /// Execution identifier.
    pub step: u64,
    /// Current tree identity, used as a confirmation token.
    pub expected: String,
    /// Exact paths to restore or remove.
    pub files: Vec<String>,
}

/// Small per-pane presentation of daemon-owned team state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Membership {
    /// Display name of the shared project, including roles in other worktrees.
    pub project: String,
    /// Most recent checkpoint belonging to this role.
    pub undo_step: Option<u64>,
    /// This role attempted to edit while read-only.
    pub violation: bool,
    /// Revision the client must send with a step action.
    pub revision: u64,
    /// Team identity.
    pub id: u64,
    /// Role order.
    pub index: usize,
    /// Whether this role owns the current step.
    pub current: bool,
    /// Waiting for a step or a write turn.
    pub waiting: bool,
    /// Selected progression mode.
    pub automatic: bool,
    /// Shell roles are always manual.
    pub manual: bool,
}

impl Team {
    /// Remove a member while preserving the identity of surviving roles and steps.
    pub(crate) fn remove_member(&mut self, pane: PaneId) {
        let Some(index) = self.members.iter().position(|m| m.pane == pane) else {
            return;
        };
        for (position, member) in self.members.iter_mut().enumerate() {
            member.original_index.get_or_insert(position);
        }
        self.members.remove(index);
        self.violations.retain(|&position| position != index);
        for position in &mut self.violations {
            if *position > index {
                *position -= 1;
            }
        }
        if self.current == index {
            self.running = false;
            self.queued = false;
            self.write_root = None;
        } else if self.current > index {
            self.current -= 1;
        }
        self.finished = self.current >= self.members.len();
        self.undo = None;
        self.revision = self.revision.saturating_add(1);
    }

    /// Presentation for one member.
    pub fn membership(&self, pane: PaneId) -> Option<Membership> {
        let index = self.members.iter().position(|m| m.pane == pane)?;
        let role_index = self.members[index].role_index(index);
        Some(Membership {
            project: self
                .project
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            undo_step: self
                .steps
                .iter()
                .rev()
                .find(|s| s.role == role_index && !s.undone)
                .map(|s| s.number),
            violation: self.violations.contains(&index),
            revision: self.revision,
            id: self.id,
            index: role_index,
            current: index == self.current && !self.finished,
            waiting: self.finished || index != self.current || !self.running,
            automatic: self.automatic,
            manual: self.members[index].role.agent.is_none(),
        })
    }
}

/// Validate an entire request before starting any process.
pub fn validate(roles: &[Role]) -> Result<(), String> {
    if roles.len() < 3 {
        return Err("A team needs Plan/Prompt, Coder, and Review/Test roles.".into());
    }
    for (index, role) in roles.iter().enumerate() {
        if role.name.trim().is_empty() || role.name.chars().count() > 80 {
            return Err("Give each role a name between 1 and 80 characters.".into());
        }
        if matches!(index, 0 | 2) && role.agent.is_some() && role.can_edit {
            return Err("Plan and Review roles must be read-only.".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shell_cannot_be_made_read_only_by_a_client() {
        let role = Role {
            name: "Shell".into(),
            agent: None,
            can_edit: false,
            worktree: false,
        };
        assert!(role.editable());
    }
    #[test]
    fn instructions_cover_the_default_workflow() {
        let role = Role {
            name: "Role".into(),
            agent: Some(AgentKind::Claude),
            can_edit: false,
            worktree: false,
        };
        assert!(role.instruction(0).contains("acceptance criteria"));
        assert!(role.instruction(1).contains("Do not push"));
        assert!(role.instruction(2).contains("file and line"));
        assert!(role.instruction(3).contains("Do not change code"));
    }

    #[test]
    fn team_progress_and_order_survive_snapshot_serialization() {
        let role = Role {
            name: "Extra".into(),
            agent: Some(AgentKind::Codex),
            can_edit: true,
            worktree: false,
        };
        let team = Team {
            steps: Vec::new(),
            write_root: None,
            queued: false,
            queue_ticket: 0,
            violations: Vec::new(),
            undo: None,
            revision: 0,
            id: 1,
            project: "/project".into(),
            members: vec![Member {
                role,
                pane: PaneId::FIRST,
                cwd: "/project".into(),
                original_index: None,
            }],
            current: 0,
            automatic: false,
            running: true,
            finished: false,
        };
        let mut snapshot = crate::snapshot::Snapshot::default();
        snapshot.teams.push(team.clone());
        let json = serde_json::to_string(&snapshot).expect("encode");
        let restored: crate::snapshot::Snapshot = serde_json::from_str(&json).expect("decode");
        assert_eq!(restored.teams, vec![team]);
        let old: crate::snapshot::Snapshot =
            serde_json::from_str("{\"version\":1}").expect("old snapshot");
        assert!(old.teams.is_empty());
    }
}
