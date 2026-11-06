//! Session-local permission policies for managed roles.

use crate::AgentKind;

/// Fail closed when the installed executable predates the verified policy.
pub fn check_read_only(agent: AgentKind) -> Result<(), String> {
    if agent != AgentKind::Claude {
        return Ok(());
    }
    let output = std::process::Command::new("claude")
        .arg("--version")
        .output()
        .map_err(|_| "Install or update the selected agent before using a read-only team role.")?;
    let version = String::from_utf8_lossy(&output.stdout);
    let components: Vec<u32> = version
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .split('.')
        .filter_map(|p| p.parse().ok())
        .collect();
    if components.as_slice() < [2, 1, 285].as_slice() {
        return Err("Read-only team roles need agent version 2.1.285 or newer for the verified sandbox policy. Update the agent or choose another agent.".into());
    }
    Ok(())
}
use std::path::Path;

/// Build a managed role's start command without changing account settings.
pub fn command(agent: AgentKind, cwd: &Path, prompt: &str, read_only: bool) -> Vec<String> {
    let mut command = agent.adapter().start_command(Some(prompt), None);
    if !read_only {
        return command;
    }
    let options: Vec<String> = match agent {
        AgentKind::Codex => vec![
            "--sandbox".into(),
            "read-only".into(),
            "--ask-for-approval".into(),
            "never".into(),
        ],
        AgentKind::Claude => {
            let settings = serde_json::json!({
                "permissions": { "deny": ["Edit", "Write", "NotebookEdit", "Agent", "Task"], "allow": ["Bash"] },
                "sandbox": { "enabled": true, "failIfUnavailable": true,
                    "allowUnsandboxedCommands": false, "excludedCommands": [],
                    "filesystem": { "denyWrite": [cwd] } }
            });
            vec![
                "--permission-mode".into(),
                "dontAsk".into(),
                "--setting-sources".into(),
                String::new(),
                "--settings".into(),
                settings.to_string(),
                "--tools".into(),
                "Read,Glob,Grep,Bash".into(),
                "--strict-mcp-config".into(),
                "--mcp-config".into(),
                "{\"mcpServers\":{}}".into(),
            ]
        }
    };
    // Put options before the positional prompt, including variadic tool options.
    let mut options = options;
    options.push("--".into());
    command.splice(1..1, options);
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn read_only_mapping_uses_enforcement_not_only_a_prompt() {
        let cwd = Path::new("/project");
        let codex = command(AgentKind::Codex, cwd, "Task", true);
        assert!(codex.windows(2).any(|a| a == ["--sandbox", "read-only"]));
        let claude = command(AgentKind::Claude, cwd, "Task", true);
        let index = claude
            .iter()
            .position(|a| a == "--settings")
            .expect("settings");
        let policy: serde_json::Value = serde_json::from_str(&claude[index + 1]).expect("json");
        assert_eq!(policy["sandbox"]["allowUnsandboxedCommands"], false);
        assert_eq!(policy["sandbox"]["failIfUnavailable"], true);
        assert_eq!(policy["sandbox"]["filesystem"]["denyWrite"][0], "/project");
    }
}
