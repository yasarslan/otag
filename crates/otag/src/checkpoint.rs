//! Git tree checkpoints and confirmed, file-only restoration using a private index.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

type Result<T> = std::result::Result<T, String>;
static NEXT: AtomicU64 = AtomicU64::new(1);

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Result<Self> {
        for _ in 0..10 {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default();
            let path = std::env::temp_dir().join(format!(
                "otag-index-{}-{stamp}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            if std::fs::create_dir(&path).is_ok() {
                return Ok(Self(path));
            }
        }
        Err(
            "Cannot create a temporary checkpoint index. Check temporary-directory permissions."
                .into(),
        )
    }
    fn index(&self) -> PathBuf {
        self.0.join("index")
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(cwd: &Path, index: Option<&Path>, args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>> {
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    if input.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    let mut child = command
        .spawn()
        .map_err(|_| "Cannot run Git. Install Git and try again.")?;
    if let Some(input) = input {
        let mut stdin = child
            .stdin
            .take()
            .ok_or("Cannot send checkpoint data to Git.")?;
        std::io::Write::write_all(&mut stdin, input)
            .map_err(|_| "Cannot send checkpoint data to Git.")?;
    }
    let output = child
        .wait_with_output()
        .map_err(|_| "Cannot finish the Git checkpoint operation.")?;
    if !output.status.success() {
        return Err("Git could not capture or restore these files. Check repository access and resolve conflicts before retrying.".into());
    }
    Ok(output.stdout)
}

fn text(bytes: Vec<u8>) -> Result<String> {
    String::from_utf8(bytes)
        .map(|s| s.trim().to_owned())
        .map_err(|_| "This repository uses paths that cannot be shown as text.".into())
}

/// Canonical working-tree root; different worktrees have independent write turns.
pub fn root(cwd: &Path) -> Result<PathBuf> {
    let path = text(
        git(cwd, None, &["rev-parse", "--show-toplevel"], None).map_err(
            |_| "Team steps need a Git repository. Initialize the project with Git first.",
        )?,
    )?;
    PathBuf::from(path).canonicalize().map_err(|_| {
        "Team steps need an accessible Git repository. Initialize the project with Git first."
            .into()
    })
}

/// Capture working files, including untracked files, without changing the real index.
pub fn tree(cwd: &Path) -> Result<String> {
    let cwd = root(cwd)?;
    crate::git::exclude(&cwd)
        .map_err(|_| "Cannot exclude team output from checkpoints. Check .git/info permissions.")?;
    let scratch = Scratch::new()?;
    let index = scratch.index();
    let original = text(git(
        &cwd,
        None,
        &["rev-parse", "--git-path", "index"],
        None,
    )?)?;
    let original = cwd.join(original);
    if original.exists() {
        std::fs::copy(original, &index)
            .map_err(|_| "Cannot read the project index for a checkpoint.")?;
    } else {
        git(&cwd, Some(&index), &["read-tree", "--empty"], None)?;
    }
    git(&cwd, Some(&index), &["add", "-A", "--", "."], None)?;
    text(git(&cwd, Some(&index), &["write-tree"], None)?)
}

/// Keep a checkpoint reachable without creating commits or changing a branch.
pub fn create(cwd: &Path, team: u64, step: u64) -> Result<String> {
    let tree = tree(cwd)?;
    git(
        cwd,
        None,
        &[
            "update-ref",
            &format!("refs/otag/checkpoints/{team}/{step}"),
            &tree,
        ],
        None,
    )?;
    Ok(tree)
}

fn paths(bytes: Vec<u8>) -> Result<BTreeSet<String>> {
    bytes
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| {
            let name = std::str::from_utf8(p).map_err(
                |_| "A changed filename cannot be shown as text. Restore it with Git manually.",
            )?;
            if Path::new(name)
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
            {
                return Err("Git returned an unsafe file path. Restore this step manually.".into());
            }
            Ok(name.to_owned())
        })
        .collect()
}

/// Exact paths a file-only undo would affect, without rename coalescing.
pub fn changed(cwd: &Path, checkpoint: &str, current: &str) -> Result<Vec<String>> {
    Ok(paths(git(
        cwd,
        None,
        &[
            "diff",
            "--no-renames",
            "--name-only",
            "-z",
            checkpoint,
            current,
            "--",
        ],
        None,
    )?)?
    .into_iter()
    .collect())
}

fn safe_parent(root: &Path, name: &str) -> Result<()> {
    let mut path = root.to_path_buf();
    let components: Vec<_> = Path::new(name).components().collect();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        path.push(component);
        if path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err("An affected file has a symbolic-link parent. Restore it manually to avoid writing outside the checkout.".into());
        }
    }
    Ok(())
}

/// Restore only confirmed paths if the working files still match the preview.
/// Leaves HEAD, branches, commit history, and the real index unchanged.
pub fn undo(cwd: &Path, checkpoint: &str, expected: &str, confirmed: &[String]) -> Result<()> {
    let cwd = root(cwd)?;
    if tree(&cwd)? != expected {
        return Err(
            "Files changed after the undo preview. Review a fresh file list before confirming."
                .into(),
        );
    }
    let actual = changed(&cwd, checkpoint, expected)?;
    if actual != confirmed {
        return Err("The undo file list changed. Review a fresh preview.".into());
    }
    for name in &actual {
        safe_parent(&cwd, name)?;
    }
    let base = paths(git(
        &cwd,
        None,
        &["ls-tree", "-r", "--name-only", "-z", checkpoint],
        None,
    )?)?;
    let scratch = Scratch::new()?;
    let index = scratch.index();
    git(&cwd, Some(&index), &["read-tree", checkpoint], None)?;
    // Remove added files first, deepest paths first, to handle file/directory replacements.
    let mut removed: Vec<_> = actual.iter().filter(|p| !base.contains(*p)).collect();
    removed.sort_by_key(|p| std::cmp::Reverse(p.len()));
    for name in removed {
        let path = cwd.join(name);
        match path.symlink_metadata() {
            Ok(meta) if meta.is_dir() => std::fs::remove_dir(&path),
            Ok(_) => std::fs::remove_file(&path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
        .map_err(|_| "Cannot remove a file created by this step. Check permissions and retry.")?;
    }
    let mut input = Vec::new();
    for name in actual.iter().filter(|p| base.contains(*p)) {
        let path = cwd.join(name);
        if path.is_dir() {
            std::fs::remove_dir(&path)
                .map_err(|_| "An affected directory is not empty. Restore this step manually.")?;
        }
        input.extend_from_slice(name.as_bytes());
        input.push(0);
    }
    if !input.is_empty() {
        git(
            &cwd,
            Some(&index),
            &["checkout-index", "--force", "-z", "--stdin"],
            Some(&input),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checkpoint_and_undo_preserve_branch_index_dirty_files_and_binary_data() {
        let project = tempfile::tempdir().expect("project");
        let cwd = project.path();
        git(cwd, None, &["init", "--quiet"], None).expect("init");
        std::fs::write(cwd.join("dirty.bin"), [0, 255, 1]).expect("binary");
        git(cwd, None, &["add", "--", "dirty.bin"], None).expect("stage");
        let index = std::fs::read(cwd.join(".git/index")).expect("index");
        let head = std::fs::read(cwd.join(".git/HEAD")).expect("head");
        std::fs::write(cwd.join("untracked"), "keep").expect("untracked");
        let checkpoint = create(cwd, 1, 1).expect("checkpoint");
        assert_eq!(std::fs::read(cwd.join(".git/index")).expect("index"), index);
        std::fs::write(cwd.join("dirty.bin"), [3, 0, 4]).expect("edit");
        std::fs::remove_file(cwd.join("untracked")).expect("delete");
        std::fs::write(cwd.join("new-file"), "created").expect("add");
        let current = tree(cwd).expect("current");
        let files = changed(cwd, &checkpoint, &current).expect("preview");
        assert_eq!(files, ["dirty.bin", "new-file", "untracked"]);
        undo(cwd, &checkpoint, &current, &files).expect("undo");
        assert_eq!(
            std::fs::read(cwd.join("dirty.bin")).expect("binary"),
            [0, 255, 1]
        );
        assert_eq!(
            std::fs::read_to_string(cwd.join("untracked")).expect("untracked"),
            "keep"
        );
        assert!(!cwd.join("new-file").exists());
        assert_eq!(std::fs::read(cwd.join(".git/index")).expect("index"), index);
        assert_eq!(std::fs::read(cwd.join(".git/HEAD")).expect("head"), head);
    }
    #[test]
    fn changed_files_invalidate_confirmation() {
        let project = tempfile::tempdir().expect("project");
        let cwd = project.path();
        git(cwd, None, &["init", "--quiet"], None).expect("init");
        let checkpoint = create(cwd, 1, 1).expect("checkpoint");
        std::fs::write(cwd.join("file"), "first").expect("edit");
        let current = tree(cwd).expect("current");
        let files = changed(cwd, &checkpoint, &current).expect("preview");
        std::fs::write(cwd.join("file"), "later").expect("external edit");
        assert!(undo(cwd, &checkpoint, &current, &files).is_err());
        assert_eq!(
            std::fs::read_to_string(cwd.join("file")).expect("read"),
            "later"
        );
    }
}
