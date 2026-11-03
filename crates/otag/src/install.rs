//! `otag install-hooks` and `otag uninstall-hooks`.
//!
//! This is the only place in otag that writes an agent's own settings file,
//! and it only ever runs because the user asked for it. It says what it is
//! about to change, asks before changing it, keeps a backup, and touches
//! nothing but otag's own entries.
//!
//! The status line otag takes over is remembered in otag's state directory,
//! so that `otag hook statusline` can keep running it and
//! `otag uninstall-hooks` can put it back.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use otag_agents::settings::{self, Change};
use otag_core::paths::Dirs;

/// Register otag's hooks and status line in the agent's settings.
pub fn install(settings_path: Option<PathBuf>, assume_yes: bool) -> Result<()> {
    let settings_path = resolve(settings_path)?;
    let planned = settings::plan(&settings_path)
        .with_context(|| format!("cannot read {}", settings_path.display()))?;

    if planned.is_empty() {
        println!(
            "otag's hooks are already installed in {}.",
            settings_path.display()
        );
        return Ok(());
    }

    describe(&planned);
    if !confirm("Change this file?", assume_yes)? {
        println!("Nothing was changed.");
        return Ok(());
    }

    let otag = std::env::current_exe().context("cannot find the otag executable")?;
    let done = settings::install(&settings_path, &otag)
        .with_context(|| format!("cannot write {}", settings_path.display()))?;

    if let Some(backup) = &done.backup {
        println!("A copy of the file as it was is in {}.", backup.display());
    }
    remember_chain(done.chained_status_line.as_deref())?;

    println!("otag's hooks are installed. Agents started from now on report to otag.");
    Ok(())
}

/// Take otag's hooks and status line out of the agent's settings again.
pub fn uninstall(settings_path: Option<PathBuf>, assume_yes: bool) -> Result<()> {
    let settings_path = resolve(settings_path)?;
    let chained = remembered_chain();

    if !confirm(
        &format!("Remove otag's entries from {}?", settings_path.display()),
        assume_yes,
    )? {
        println!("Nothing was changed.");
        return Ok(());
    }

    let done = settings::uninstall(&settings_path, chained.as_deref())
        .with_context(|| format!("cannot write {}", settings_path.display()))?;

    if done.is_empty() {
        println!(
            "otag had nothing registered in {}.",
            settings_path.display()
        );
        return Ok(());
    }
    if let Some(backup) = &done.backup {
        println!("A copy of the file as it was is in {}.", backup.display());
    }
    if done.takes_status_line {
        match &done.chained_status_line {
            Some(command) => println!("Your status line command is back: {command}"),
            None => println!("The statusLine entry otag added is gone."),
        }
    }
    forget_chain();

    println!("otag's hooks are removed.");
    Ok(())
}

/// Which settings file to work on.
///
/// Claude Code's user settings, unless the user named another file — which
/// is how a project's own settings, or a test's temporary home, is reached.
fn resolve(settings_path: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = settings_path {
        return Ok(path);
    }
    let home = home_dir().context(
        "cannot find your home directory. \
         Pass --settings with the path of the settings file to change.",
    )?;
    Ok(otag_agents::claude::settings_path(&home))
}

/// The user's home directory, as the operating system reports it.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// Say what is about to change, in the user's terms.
fn describe(planned: &Change) {
    println!("otag would change {}:", planned.settings.display());
    for event in &planned.events {
        println!("  + run `otag hook {event}` on {event}");
    }
    if planned.takes_status_line {
        match &planned.chained_status_line {
            Some(command) => {
                println!("  + take over statusLine, still running yours: {command}");
            }
            None => println!("  + set statusLine to `otag hook statusline`"),
        }
    }
    if let Some(seconds) = planned.sets_refresh_interval {
        println!(
            "  + re-run the status line every {seconds}s, so a session that is \
             sitting idle keeps reporting the account's limits"
        );
    }
    println!("A copy of the file as it is now is kept next to it.");
}

/// Ask the user, unless they already said yes on the command line.
///
/// Without a terminal to ask on — a script, a pipe — the answer is no: a
/// command that changes the user's settings may not assume consent.
fn confirm(question: &str, assume_yes: bool) -> Result<bool> {
    if assume_yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        bail!(
            "{question} There is no terminal to ask on. \
             Run this command yourself, or pass --yes if you are sure."
        );
    }

    print!("{question} [y/N] ");
    std::io::stdout().flush().ok();

    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .context("cannot read your answer")?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "Yes"))
}

/// Remember the status line otag took over, so the hook can run it.
fn remember_chain(command: Option<&str>) -> Result<()> {
    let Some(state_dir) = state_dir() else {
        if command.is_some() {
            println!(
                "Note: otag cannot find a state directory, so your own status line \
                 will not be run. Put it back with `otag uninstall-hooks`."
            );
        }
        return Ok(());
    };

    match command {
        Some(command) => {
            std::fs::create_dir_all(&state_dir)
                .with_context(|| format!("cannot create {}", state_dir.display()))?;
            let path = crate::hook::chain_path(&state_dir);
            std::fs::write(&path, format!("{command}\n"))
                .with_context(|| format!("cannot write {}", path.display()))?;
        }
        // Nothing to chain: an older chain file would make otag run a
        // status line the user no longer has.
        None => forget_chain(),
    }
    Ok(())
}

/// The status line otag took over, if it remembered one.
fn remembered_chain() -> Option<String> {
    let path = crate::hook::chain_path(&state_dir()?);
    let text = std::fs::read_to_string(path).ok()?;
    let command = text.trim();
    (!command.is_empty()).then(|| command.to_owned())
}

/// Forget the status line otag took over.
///
/// A file that cannot be removed is not an error worth stopping for: the
/// settings file is already correct, and the stale file only matters if otag
/// is installed again.
fn forget_chain() {
    if let Some(state_dir) = state_dir() {
        let _ = std::fs::remove_file(crate::hook::chain_path(&state_dir));
    }
}

/// otag's own state directory, when the platform offers one.
fn state_dir() -> Option<PathBuf> {
    Dirs::from_env().state_dir().ok()
}

/// The state directory `otag hook` reads the chained status line from.
#[must_use]
pub fn hook_state_dir() -> Option<PathBuf> {
    state_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_named_settings_file_is_used_as_it_is() {
        let path = PathBuf::from("/somewhere/settings.json");
        assert_eq!(
            resolve(Some(path.clone())).expect("a path"),
            path,
            "a file the user named is never second-guessed"
        );
    }

    #[test]
    fn a_question_is_answered_yes_only_when_the_user_said_so() {
        assert!(confirm("anything?", true).expect("an answer"));
    }

    #[test]
    fn a_question_without_a_terminal_refuses_rather_than_assuming_consent() {
        // `cargo test` gives the test no terminal, which is exactly the
        // situation this guards against.
        if std::io::stdin().is_terminal() {
            return;
        }
        let error = confirm("change your settings?", false).expect_err("a refusal");
        assert!(error.to_string().contains("--yes"), "{error}");
    }

    #[test]
    fn what_would_change_is_described_from_the_plan() {
        // Nothing to assert on stdout here; the point is that describing a
        // plan with and without a status line does not panic on a missing
        // field.
        describe(&Change {
            settings: PathBuf::from("/tmp/settings.json"),
            events: vec!["Stop".to_owned()],
            chained_status_line: Some("my-line".to_owned()),
            takes_status_line: true,
            sets_refresh_interval: Some(30),
            backup: None,
        });
        describe(&Change {
            settings: PathBuf::from("/tmp/settings.json"),
            events: Vec::new(),
            chained_status_line: None,
            takes_status_line: false,
            sets_refresh_interval: None,
            backup: None,
        });
    }
}
