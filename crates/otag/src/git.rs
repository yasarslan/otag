//! What `git` says about the directory a pane is working in.
//!
//! otag runs `git` rather than linking a library: the user's repository may
//! use anything from worktrees to submodules, and the only thing guaranteed
//! to understand it is the `git` the user already has. Every call is read-only
//! and bounded, with one exception — `exclude`, which adds one line to
//! `.git/info/exclude` so that otag's own files do not show up as untracked
//! work. That file is not tracked by git, so nothing otag writes can end up
//! in a commit.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use otag_features::handoff::{Checkout, EXCLUDE_LINE};

/// How long any one `git` call may take.
///
/// A repository on a network filesystem can be slow, and a handoff that
/// hangs is worse than one without a diff in it.
const TIMEOUT: Duration = Duration::from_secs(3);

/// How much of one `git` call's output is read.
const MAX_OUTPUT: usize = 64 * 1024;

/// What git says about this directory, as far as it says anything.
///
/// Outside a repository, or without git installed, every field is empty:
/// otag works in plain directories too.
pub async fn checkout(cwd: &Path) -> Checkout {
    let (branch, status, diff_stat) = tokio::join!(
        run(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]),
        run(cwd, &["status", "--porcelain"]),
        run(cwd, &["diff", "--stat"]),
    );

    Checkout {
        branch: branch
            .map(|text| text.trim().to_owned())
            // A repository with no commits yet answers `HEAD`, which says
            // nothing worth carrying.
            .filter(|branch| !branch.is_empty() && branch != "HEAD"),
        status: lines(status),
        diff_stat: lines(diff_stat),
    }
}

/// Keep otag's own files out of the user's `git status`.
///
/// Returns whether the line had to be added. `.git/info/exclude` is a local,
/// untracked file: adding to it cannot change anything the user commits, and
/// it is the reason otag never touches `.gitignore`.
pub fn exclude(cwd: &Path) -> std::io::Result<bool> {
    let Some(info) = git_dir(cwd) else {
        // Not a repository, so there is nothing to keep clean.
        return Ok(false);
    };
    let path = info.join("exclude");

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.lines().any(|line| line.trim() == EXCLUDE_LINE) {
        return Ok(false);
    }

    std::fs::create_dir_all(&info)?;
    let separator = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let addition = format!("{separator}# otag's handoff packages\n{EXCLUDE_LINE}\n");
    std::fs::write(&path, format!("{existing}{addition}"))?;
    Ok(true)
}

/// The `info` directory of the repository `cwd` is in, if it is in one.
///
/// Walks up from `cwd` rather than asking git, so that it works without
/// spawning a process; a `.git` file rather than a directory — which is what
/// a worktree has — is followed to where it points.
fn git_dir(cwd: &Path) -> Option<std::path::PathBuf> {
    for directory in cwd.ancestors() {
        let candidate = directory.join(".git");
        if candidate.is_dir() {
            return Some(candidate.join("info"));
        }
        if candidate.is_file() {
            // A worktree's `.git` is a file saying `gitdir: <path>`.
            let text = std::fs::read_to_string(&candidate).ok()?;
            let target = text.strip_prefix("gitdir:")?.trim();
            let target = Path::new(target);
            let target = if target.is_absolute() {
                target.to_path_buf()
            } else {
                directory.join(target)
            };
            return Some(target.join("info"));
        }
    }
    None
}

/// Run one read-only `git` command, or report nothing.
async fn run(cwd: &Path, arguments: &[&str]) -> Option<String> {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(cwd)
        // Never let git stop for a passphrase or a credential helper: this
        // runs inside a daemon with nobody to answer it.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(arguments)
        .stdin(Stdio::null())
        .stderr(Stdio::null());

    let output = tokio::time::timeout(TIMEOUT, command.output()).await;
    let output = match output {
        Ok(Ok(output)) if output.status.success() => output,
        // Not a repository, no git, or git took too long. All three mean the
        // same thing here: no information.
        _ => return None,
    };

    let text = String::from_utf8_lossy(&output.stdout);
    Some(text.chars().take(MAX_OUTPUT).collect())
}

/// One command's output as non-empty lines.
fn lines(output: Option<String>) -> Vec<String> {
    output
        .into_iter()
        .flat_map(|text| {
            text.lines()
                .map(str::trim_end)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository with one commit and one unstaged change.
    fn repository() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path();

        for arguments in [
            vec!["init", "--initial-branch=work"],
            vec!["config", "user.email", "tests@example.invalid"],
            vec!["config", "user.name", "otag tests"],
        ] {
            let done = std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(&arguments)
                .output()
                .expect("git runs");
            assert!(done.status.success(), "git {arguments:?} failed");
        }

        std::fs::write(path.join("file.txt"), "one\n").expect("a file");
        for arguments in [vec!["add", "."], vec!["commit", "-m", "first"]] {
            std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(&arguments)
                .output()
                .expect("git runs");
        }
        std::fs::write(path.join("file.txt"), "one\ntwo\n").expect("a change");

        dir
    }

    #[tokio::test]
    async fn a_repository_reports_its_branch_and_its_changes() {
        let repository = repository();
        let facts = checkout(repository.path()).await;

        assert_eq!(facts.branch.as_deref(), Some("work"));
        assert!(
            facts.status.iter().any(|line| line.contains("file.txt")),
            "{:?}",
            facts.status
        );
        assert!(
            facts.diff_stat.iter().any(|line| line.contains("file.txt")),
            "{:?}",
            facts.diff_stat
        );
    }

    #[tokio::test]
    async fn a_plain_directory_reports_nothing_rather_than_failing() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let facts = checkout(dir.path()).await;
        assert!(facts.is_empty(), "{facts:?}");
    }

    #[test]
    fn otags_own_files_are_excluded_once() {
        let repository = repository();

        assert!(exclude(repository.path()).expect("the file can be written"));
        let path = repository.path().join(".git").join("info").join("exclude");
        let written = std::fs::read_to_string(&path).expect("the exclude file");
        assert!(written.contains(EXCLUDE_LINE), "{written}");

        assert!(
            !exclude(repository.path()).expect("nothing to do"),
            "a second call adds nothing"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("the exclude file"),
            written,
            "the file is left exactly as it was"
        );
    }

    #[test]
    fn excluding_keeps_what_the_user_already_had() {
        let repository = repository();
        let path = repository.path().join(".git").join("info").join("exclude");
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the directory");
        std::fs::write(&path, "my-own-file").expect("what the user had");

        exclude(repository.path()).expect("the file can be written");

        let written = std::fs::read_to_string(&path).expect("the exclude file");
        assert!(written.starts_with("my-own-file\n"), "{written}");
        assert!(written.contains(EXCLUDE_LINE), "{written}");
    }

    #[test]
    fn a_plain_directory_has_nothing_to_exclude() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        assert!(!exclude(dir.path()).expect("nothing to do"));
    }

    #[test]
    fn a_worktrees_git_file_is_followed_to_where_it_points() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let elsewhere = dir.path().join("real-git-dir");
        std::fs::create_dir_all(&elsewhere).expect("the directory");
        let worktree = dir.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("the directory");
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", elsewhere.display()),
        )
        .expect("the pointer file");

        assert!(exclude(&worktree).expect("the file can be written"));
        let written = std::fs::read_to_string(elsewhere.join("info").join("exclude"))
            .expect("the exclude file of the real git directory");
        assert!(written.contains(EXCLUDE_LINE), "{written}");
    }
}
