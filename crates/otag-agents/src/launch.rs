//! Executable discovery for agents launched by otag.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use nix::unistd::{AccessFlags, Uid, User, access};

use crate::AgentKind;

/// A supported agent was requested but no executable was found.
#[derive(Debug, thiserror::Error)]
#[error(
    "{label} was not found. Install it, or make sure the `{program}` command works in your terminal, then try again."
)]
pub struct MissingExecutable {
    label: &'static str,
    program: &'static str,
}

/// Resolve a bare agent command using the pane's effective environment.
///
/// Other commands, including explicit executable paths, return `Ok(None)`
/// and are left to the normal process launcher.
pub fn resolve(
    program: &OsStr,
    path: Option<&OsStr>,
    home: Option<&Path>,
    cwd: Option<&Path>,
) -> Result<Option<PathBuf>, MissingExecutable> {
    let (kind, label) = match program.to_str() {
        Some("claude") => (AgentKind::Claude, "Claude Code"),
        Some("codex") => (AgentKind::Codex, "Codex CLI"),
        _ => return Ok(None),
    };
    let program = kind.name();
    let home = home.map(Path::to_path_buf).or_else(|| {
        User::from_uid(Uid::current())
            .ok()
            .flatten()
            .map(|user| user.dir)
    });
    let home = home.as_deref();
    let cwd = cwd.or(home).unwrap_or(Path::new("/"));
    let on_path = path
        .filter(|path| !path.is_empty())
        .into_iter()
        .flat_map(std::env::split_paths)
        .map(|directory| cwd.join(directory));
    let fallback = [
        home.map(|home| home.join(".local/bin")),
        Some(PathBuf::from("/opt/homebrew/bin")),
        Some(PathBuf::from("/usr/local/bin")),
        home.map(|home| home.join(".npm-global/bin")),
    ];

    for directory in on_path.chain(fallback.into_iter().flatten()) {
        let candidate = directory.join(program);
        if candidate.is_file()
            && access(&candidate, AccessFlags::X_OK).is_ok()
            && let Ok(executable) = std::path::absolute(candidate)
        {
            return Ok(Some(executable));
        }
    }

    tracing::debug!(program, ?path, "agent executable not found");
    Err(MissingExecutable { label, program })
}
