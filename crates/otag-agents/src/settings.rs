//! Putting otag's bridge into an agent's own settings, and taking it out.
//!
//! Claude Code learns about otag by having commands registered in
//! `~/.claude/settings.json`: one per hook event, and one as the statusLine
//! command. That file belongs to the user, so everything here follows the
//! same rules: a backup is kept, nothing but otag's own entries is touched,
//! what the user had is put back on the way out, and the file is written in
//! one move so that an interrupted write cannot leave half a file behind.
//!
//! Each command is `<otag> hook <event>`, and otag recognises its own entries
//! by the program they run rather than by matching the whole string, so a
//! user who moved their otag binary is not left with a duplicate.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::hooks::{EVENTS, STATUS_LINE};
use crate::{Error, Result};

/// Suffix of the backup otag keeps before changing anything.
const BACKUP_SUFFIX: &str = "otag-backup";

/// The program name otag's own entries run.
const PROGRAM: &str = "otag";

/// How often otag asks Claude Code to re-run the status line, in seconds.
///
/// Claude Code runs a status line on events — a turn ending, a tool
/// finishing — and those stop while a session sits idle. The limits it
/// reports are the account's, so a session that is only sitting there is
/// still worth hearing from: half a minute keeps the meters current without
/// the command becoming something the user notices. Claude Code's minimum is
/// one second, and leaving the field unset means events only.
pub const STATUS_LINE_REFRESH: u64 = 30;

/// What installing otag's bridge would do, or has done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The file that is changed.
    pub settings: PathBuf,
    /// Events otag would register, or has registered.
    pub events: Vec<String>,
    /// The statusLine command that was there, when otag takes it over.
    ///
    /// otag's own statusLine command runs this one too and prints what it
    /// prints, so the user's status line keeps working.
    pub chained_status_line: Option<String>,
    /// Whether otag takes the statusLine command over at all.
    pub takes_status_line: bool,
    /// The refresh interval otag would add to the status line, in seconds.
    ///
    /// `None` when there is nothing to do: either the file already has one —
    /// whatever the user chose is theirs to keep — or otag is not the one
    /// running the status line.
    pub sets_refresh_interval: Option<u64>,
    /// Where the backup of the file as it was went.
    pub backup: Option<PathBuf>,
}

impl Change {
    /// Whether this change would alter the file.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty() && !self.takes_status_line && self.sets_refresh_interval.is_none()
    }
}

/// What installing otag's bridge would change, without changing anything.
pub fn plan(settings: &Path) -> Result<Change> {
    let value = read(settings)?;
    Ok(decide(settings, &value))
}

/// Register otag's commands, keeping a backup of the file as it was.
///
/// Doing it twice changes nothing the second time: an event otag already
/// registered is left alone, and a statusLine command that is already otag's
/// is not chained to itself.
pub fn install(settings: &Path, otag: &Path) -> Result<Change> {
    let mut value = read(settings)?;
    let mut change = decide(settings, &value);
    if change.is_empty() {
        return Ok(change);
    }

    change.backup = backup(settings)?;
    // Read before anything is replaced: an interval already in the file is
    // the user's, and it is carried over to the entry otag writes rather
    // than thrown away with the command it was set on.
    let kept = refresh_interval(&value);
    let object = as_object(&mut value);

    if !change.events.is_empty() {
        let hooks = object
            .entry("hooks".to_owned())
            .or_insert_with(|| json!({}));
        let hooks = as_object(hooks);
        for event in &change.events {
            let groups = hooks.entry(event.clone()).or_insert_with(|| json!([]));
            if let Some(groups) = groups.as_array_mut() {
                groups.push(json!({
                    "matcher": "*",
                    "hooks": [{ "type": "command", "command": command(otag, event) }],
                }));
            }
        }
    }

    if change.takes_status_line {
        object.insert(
            "statusLine".to_owned(),
            json!({ "type": "command", "command": command(otag, STATUS_LINE) }),
        );
    }
    // An idle session reports nothing on its own, and the limits it would
    // report are the account's. A timer is what keeps them arriving.
    if let Some(seconds) = kept.or(change.sets_refresh_interval)
        && let Some(status_line) = object.get_mut("statusLine")
    {
        as_object(status_line).insert("refreshInterval".to_owned(), json!(seconds));
    }

    write(settings, &value)?;
    Ok(change)
}

/// Take otag's commands out again, putting back the statusLine command it
/// took over.
///
/// Only otag's own entries are removed: anything the user added since is
/// theirs, and restoring the backup wholesale would throw it away.
pub fn uninstall(settings: &Path, chained_status_line: Option<&str>) -> Result<Change> {
    let mut value = read(settings)?;
    let mut change = Change {
        settings: settings.to_path_buf(),
        events: Vec::new(),
        chained_status_line: chained_status_line.map(str::to_owned),
        takes_status_line: false,
        sets_refresh_interval: None,
        backup: None,
    };

    let ours = is_ours(status_line_command(&value));
    // Which of the two set this is no longer knowable, so it goes back with
    // the user's own command: an interval they chose must not be lost, and
    // one otag chose is a sensible thing to leave on a status line.
    let kept = refresh_interval(&value);
    let object = as_object(&mut value);

    if let Some(hooks) = object.get_mut("hooks").and_then(Value::as_object_mut) {
        for event in EVENTS {
            let Some(groups) = hooks.get_mut(event).and_then(Value::as_array_mut) else {
                continue;
            };
            let before = groups.len();
            groups.retain_mut(|group| {
                let Some(entries) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                    return true;
                };
                entries.retain(|entry| !is_ours(entry.get("command").and_then(Value::as_str)));
                // A group with no commands left in it is otag's leftover.
                !entries.is_empty()
            });
            if groups.len() != before {
                change.events.push(event.to_owned());
            }
            if groups.is_empty() {
                hooks.remove(event);
            }
        }
        if hooks.is_empty() {
            object.remove("hooks");
        }
    }

    if ours {
        change.takes_status_line = true;
        match chained_status_line {
            Some(previous) => {
                let mut line = json!({ "type": "command", "command": previous });
                if let Some(seconds) = kept {
                    as_object(&mut line).insert("refreshInterval".to_owned(), json!(seconds));
                }
                object.insert("statusLine".to_owned(), line);
            }
            None => {
                object.remove("statusLine");
            }
        }
    }

    if change.is_empty() {
        return Ok(change);
    }

    change.backup = backup(settings)?;
    write(settings, &value)?;
    Ok(change)
}

/// What would have to change for otag's bridge to be installed.
fn decide(settings: &Path, value: &Value) -> Change {
    let installed = installed_events(value);
    let status_line = status_line_command(value);
    // otag sets the interval on its own status line only, and only when the
    // file has none: a user who chose an interval chose it.
    let sets_refresh_interval = (refresh_interval(value).is_none()).then_some(STATUS_LINE_REFRESH);

    Change {
        settings: settings.to_path_buf(),
        events: EVENTS
            .into_iter()
            .filter(|event| !installed.contains(&(*event).to_owned()))
            .map(str::to_owned)
            .collect(),
        // Nothing to chain when the command is already otag's, or when the
        // user had none.
        chained_status_line: status_line
            .filter(|command| !is_ours(Some(command)))
            .map(str::to_owned),
        takes_status_line: !is_ours(status_line),
        sets_refresh_interval,
        backup: None,
    }
}

/// The refresh interval the status line is set to run on, if any.
fn refresh_interval(value: &Value) -> Option<u64> {
    value
        .get("statusLine")
        .and_then(|line| line.get("refreshInterval"))
        .and_then(Value::as_u64)
}

/// The events that already run one of otag's commands.
fn installed_events(value: &Value) -> Vec<String> {
    let Some(hooks) = value.get("hooks").and_then(Value::as_object) else {
        return Vec::new();
    };

    hooks
        .iter()
        .filter(|(_, groups)| {
            groups
                .as_array()
                .is_some_and(|groups| groups.iter().any(group_is_ours))
        })
        .map(|(event, _)| event.clone())
        .collect()
}

/// Whether one hook group runs one of otag's commands.
fn group_is_ours(group: &Value) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| is_ours(entry.get("command").and_then(Value::as_str)))
        })
}

/// The statusLine command, if one is set.
fn status_line_command(value: &Value) -> Option<&str> {
    value
        .get("statusLine")
        .and_then(|line| line.get("command"))
        .and_then(Value::as_str)
}

/// What otag registers for an event.
fn command(otag: &Path, event: &str) -> String {
    let path = otag.to_string_lossy();
    // A path with a space in it has to survive being read as a shell
    // command; a single-quoted string is literal in every shell otag's
    // commands can be run by.
    if path.contains(|character: char| character.is_whitespace()) {
        format!("'{}' hook {event}", path.replace('\'', "'\\''"))
    } else {
        format!("{path} hook {event}")
    }
}

/// Whether a command is one of otag's.
///
/// Recognised by the program it runs, not by the whole string, so that a
/// user who moved or renamed their otag binary does not end up with the old
/// entry beside the new one.
fn is_ours(command: Option<&str>) -> bool {
    let Some(command) = command else {
        return false;
    };
    let words = tokens(command);
    let Some(program) = words.first() else {
        return false;
    };
    let program = program.rsplit('/').next().unwrap_or(program);
    program == PROGRAM && words.get(1).map(String::as_str) == Some("hook")
}

/// The words of a command, with the quotes taken off.
///
/// A quoted word keeps its spaces, so the program a command runs is found
/// even when its path has one in it.
fn tokens(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;

    for character in command.chars() {
        match quote {
            Some(open) if character == open => quote = None,
            Some(_) => word.push(character),
            None if character == '\'' || character == '"' => quote = Some(character),
            None if character.is_whitespace() => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            None => word.push(character),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// Read a settings file, or an empty object when there is none.
fn read(settings: &Path) -> Result<Value> {
    let text = match std::fs::read_to_string(settings) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(source) => {
            return Err(Error::Settings {
                what: "cannot read".to_owned(),
                path: settings.to_path_buf(),
                source: Box::new(source),
            });
        }
    };
    if text.trim().is_empty() {
        return Ok(json!({}));
    }

    let value: Value = serde_json::from_str(&text).map_err(|source| Error::Settings {
        what: "cannot read".to_owned(),
        path: settings.to_path_buf(),
        source: Box::new(source),
    })?;
    // Anything that is not an object is not a settings file, and otag must
    // not write over it.
    if !value.is_object() {
        return Err(Error::Settings {
            what: "is not a settings object, so otag will not change".to_owned(),
            path: settings.to_path_buf(),
            source: Box::new(std::io::Error::other("the file is not a JSON object")),
        });
    }
    Ok(value)
}

/// Write a settings file in one move, readable only by its owner.
fn write(settings: &Path, value: &Value) -> Result<()> {
    let failed = |what: &str, source: Box<dyn std::error::Error + Send + Sync>| Error::Settings {
        what: what.to_owned(),
        path: settings.to_path_buf(),
        source,
    };

    if let Some(parent) = settings.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| failed("cannot create the directory of", Box::new(source)))?;
    }

    let mut text = serde_json::to_string_pretty(value)
        .map_err(|source| failed("cannot write", Box::new(source)))?;
    text.push('\n');

    // Written beside the real file and then moved onto it, so that an
    // interrupted write leaves the original untouched.
    let temporary = settings.with_extension("json.otag-new");
    std::fs::write(&temporary, &text).map_err(|source| failed("cannot write", Box::new(source)))?;
    if let Err(source) = private(&temporary) {
        tracing::debug!(%source, "cannot restrict the permissions of a settings file");
    }
    std::fs::rename(&temporary, settings)
        .map_err(|source| failed("cannot write", Box::new(source)))?;
    Ok(())
}

/// Keep a copy of a settings file as it is now.
fn backup(settings: &Path) -> Result<Option<PathBuf>> {
    if !settings.exists() {
        return Ok(None);
    }

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let name = settings
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "settings.json".to_owned());
    let path = settings.with_file_name(format!("{name}.{BACKUP_SUFFIX}-{stamp}"));

    std::fs::copy(settings, &path).map_err(|source| Error::Settings {
        what: "cannot back up".to_owned(),
        path: settings.to_path_buf(),
        source: Box::new(source),
    })?;
    Ok(Some(path))
}

/// Make a file readable only by its owner.
fn private(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

/// The object inside a value, replacing anything that is not one.
fn as_object(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = json!({});
    }
    value
        .as_object_mut()
        .unwrap_or_else(|| unreachable!("the value was just made an object"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A settings file in a temporary home, with the given content.
    fn settings_with(content: Option<&str>) -> (tempfile::TempDir, PathBuf) {
        let home = tempfile::tempdir().expect("a temporary home");
        let path = crate::claude::settings_path(home.path());
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the directory");
        if let Some(content) = content {
            std::fs::write(&path, content).expect("the settings file");
        }
        (home, path)
    }

    fn otag() -> PathBuf {
        PathBuf::from("/usr/local/bin/otag")
    }

    fn read_back(path: &Path) -> Value {
        let text = std::fs::read_to_string(path).expect("the settings file is readable");
        serde_json::from_str(&text).expect("the settings file is JSON")
    }

    /// The commands registered for one event.
    fn commands_for(value: &Value, event: &str) -> Vec<String> {
        value
            .get("hooks")
            .and_then(|hooks| hooks.get(event))
            .and_then(Value::as_array)
            .map(|groups| {
                groups
                    .iter()
                    .filter_map(|group| group.get("hooks")?.as_array())
                    .flatten()
                    .filter_map(|entry| entry.get("command")?.as_str())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn installing_registers_every_event_otag_acts_on() {
        let (_home, path) = settings_with(None);

        let change = install(&path, &otag()).expect("the settings are written");
        assert_eq!(change.events, EVENTS);
        assert!(change.takes_status_line);
        assert_eq!(change.chained_status_line, None);
        assert_eq!(change.backup, None, "there was no file to back up");

        let value = read_back(&path);
        for event in EVENTS {
            assert_eq!(
                commands_for(&value, event),
                vec![format!("/usr/local/bin/otag hook {event}")],
                "{event}"
            );
        }
        assert_eq!(
            status_line_command(&value),
            Some("/usr/local/bin/otag hook statusline")
        );
    }

    #[test]
    fn installing_keeps_everything_else_in_the_file() {
        let (_home, path) = settings_with(Some(
            r#"{
                 "model": "opus",
                 "permissions": { "allow": ["Bash(cargo test:*)"] },
                 "hooks": {
                   "PreToolUse": [
                     { "matcher": "Bash", "hooks": [{ "type": "command", "command": "mine.sh" }] }
                   ]
                 }
               }"#,
        ));

        install(&path, &otag()).expect("the settings are written");
        let value = read_back(&path);

        assert_eq!(value.get("model").and_then(Value::as_str), Some("opus"));
        assert!(value.get("permissions").is_some(), "the rest is untouched");
        assert_eq!(
            commands_for(&value, "PreToolUse"),
            vec![
                "mine.sh".to_owned(),
                "/usr/local/bin/otag hook PreToolUse".to_owned()
            ],
            "the user's own hook still runs, and otag's runs beside it"
        );
    }

    #[test]
    fn installing_keeps_a_backup_of_the_file_as_it_was() {
        let (_home, path) = settings_with(Some(r#"{"model":"opus"}"#));

        let change = install(&path, &otag()).expect("the settings are written");
        let backup = change.backup.expect("a backup was kept");
        assert_eq!(read_back(&backup), json!({ "model": "opus" }));
    }

    #[test]
    fn installing_twice_changes_nothing_the_second_time() {
        let (_home, path) = settings_with(None);
        install(&path, &otag()).expect("the first install");
        let before = std::fs::read_to_string(&path).expect("the file");

        let change = install(&path, &otag()).expect("the second install");
        assert!(change.is_empty(), "{change:?}");
        assert_eq!(
            std::fs::read_to_string(&path).expect("the file"),
            before,
            "the file was not touched"
        );
    }

    #[test]
    fn the_status_line_the_user_had_is_kept_to_be_chained() {
        let (_home, path) = settings_with(Some(
            r#"{"statusLine":{"type":"command","command":"python3 /home/user/line.py"}}"#,
        ));

        let change = install(&path, &otag()).expect("the settings are written");
        assert_eq!(
            change.chained_status_line.as_deref(),
            Some("python3 /home/user/line.py"),
            "otag's own status line runs this one and prints what it prints"
        );
        assert_eq!(
            status_line_command(&read_back(&path)),
            Some("/usr/local/bin/otag hook statusline")
        );
    }

    #[test]
    fn installing_asks_for_the_status_line_to_be_re_run_on_a_timer() {
        let (_home, path) = settings_with(None);

        let change = install(&path, &otag()).expect("the settings are written");
        assert_eq!(change.sets_refresh_interval, Some(STATUS_LINE_REFRESH));
        assert_eq!(
            refresh_interval(&read_back(&path)),
            Some(STATUS_LINE_REFRESH),
            "an idle session must keep reporting the account's limits"
        );
    }

    #[test]
    fn an_interval_the_user_chose_is_left_alone() {
        let (_home, path) = settings_with(Some(
            r#"{"statusLine":{"type":"command","command":"my-line","refreshInterval":5}}"#,
        ));

        let change = install(&path, &otag()).expect("the settings are written");
        assert_eq!(change.sets_refresh_interval, None);
        let value = read_back(&path);
        assert_eq!(
            refresh_interval(&value),
            Some(5),
            "a user who chose an interval chose it, and taking the command over does not lose it"
        );
        assert_eq!(
            status_line_command(&value),
            Some("/usr/local/bin/otag hook statusline")
        );
    }

    #[test]
    fn an_otag_status_line_from_before_the_timer_gains_one() {
        // What `otag install-hooks` left behind before otag asked for a
        // timer: everything is registered, and nothing re-runs it.
        let (_home, path) = settings_with(None);
        install(&path, &otag()).expect("the first install");
        let mut value = read_back(&path);
        as_object(value.get_mut("statusLine").expect("otag's status line"))
            .remove("refreshInterval");
        std::fs::write(&path, value.to_string()).expect("the settings file");

        let plan = plan(&path).expect("a plan");
        assert!(!plan.is_empty(), "there is something left to do");
        assert!(plan.events.is_empty(), "the hooks are all registered");
        assert!(!plan.takes_status_line, "the status line is already otag's");
        assert_eq!(plan.sets_refresh_interval, Some(STATUS_LINE_REFRESH));

        install(&path, &otag()).expect("the second install");
        let value = read_back(&path);
        assert_eq!(refresh_interval(&value), Some(STATUS_LINE_REFRESH));
        assert_eq!(
            status_line_command(&value),
            Some("/usr/local/bin/otag hook statusline"),
            "and otag's own command is still the one that runs"
        );
    }

    #[test]
    fn installing_twice_over_a_complete_install_changes_nothing() {
        let (_home, path) = settings_with(None);
        install(&path, &otag()).expect("the first install");
        let after_first = read_back(&path);

        let again = install(&path, &otag()).expect("the second install");
        assert!(again.is_empty(), "there was nothing left to do");
        assert_eq!(read_back(&path), after_first);
    }

    #[test]
    fn uninstalling_takes_the_timer_out_with_otags_status_line() {
        let (_home, path) = settings_with(None);
        install(&path, &otag()).expect("the install");

        uninstall(&path, None).expect("the uninstall");
        let value = read_back(&path);
        assert_eq!(value.get("statusLine"), None);
        assert_eq!(
            refresh_interval(&value),
            None,
            "otag's timer goes when otag's status line goes"
        );
    }

    #[test]
    fn uninstalling_puts_back_what_the_user_had() {
        let (_home, path) = settings_with(Some(
            r#"{"model":"opus","statusLine":{"type":"command","command":"python3 line.py"}}"#,
        ));
        let change = install(&path, &otag()).expect("the install");
        let chained = change.chained_status_line.clone();

        let removed = uninstall(&path, chained.as_deref()).expect("the uninstall");
        assert_eq!(removed.events, EVENTS);
        assert!(removed.takes_status_line);

        let value = read_back(&path);
        assert_eq!(value.get("hooks"), None, "no empty leftovers");
        assert_eq!(status_line_command(&value), Some("python3 line.py"));
        assert_eq!(
            refresh_interval(&value),
            Some(STATUS_LINE_REFRESH),
            "the timer goes back with the command it was set on"
        );
        assert_eq!(value.get("model").and_then(Value::as_str), Some("opus"));
    }

    #[test]
    fn uninstalling_removes_the_status_line_it_added_when_there_was_none() {
        let (_home, path) = settings_with(None);
        install(&path, &otag()).expect("the install");

        uninstall(&path, None).expect("the uninstall");
        assert_eq!(read_back(&path).get("statusLine"), None);
    }

    #[test]
    fn uninstalling_leaves_the_user_s_own_hooks_alone() {
        // A backup restored wholesale would throw away anything the user
        // changed since, so only otag's own entries are removed.
        let (_home, path) = settings_with(Some(
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"mine.sh"}]}]}}"#,
        ));
        install(&path, &otag()).expect("the install");

        uninstall(&path, None).expect("the uninstall");
        assert_eq!(
            commands_for(&read_back(&path), "PreToolUse"),
            vec!["mine.sh"]
        );
    }

    #[test]
    fn uninstalling_what_was_never_installed_changes_nothing() {
        let (_home, path) = settings_with(Some(r#"{"model":"opus"}"#));
        let change = uninstall(&path, None).expect("the uninstall");
        assert!(change.is_empty());
        assert_eq!(
            std::fs::read_to_string(&path).expect("the file"),
            r#"{"model":"opus"}"#,
            "the file was not touched"
        );
    }

    #[test]
    fn an_otag_that_moved_is_recognised_as_otag() {
        // Otherwise installing after an upgrade would leave the old entry
        // beside the new one, and the agent would run two of them.
        let (_home, path) = settings_with(None);
        install(&path, &otag()).expect("the install");

        let change = install(&path, Path::new("/opt/otag/bin/otag")).expect("the second install");
        assert!(change.is_empty(), "{change:?}");
    }

    #[test]
    fn a_path_with_a_space_in_it_is_quoted() {
        let (_home, path) = settings_with(None);
        let moved = PathBuf::from("/Applications/My Tools/otag");

        install(&path, &moved).expect("the install");
        let value = read_back(&path);
        assert_eq!(
            commands_for(&value, "Stop"),
            vec!["'/Applications/My Tools/otag' hook Stop".to_owned()]
        );
        // And it is still recognised as otag's own.
        assert!(
            install(&path, &moved)
                .expect("the second install")
                .is_empty()
        );
    }

    #[test]
    fn a_plan_says_what_would_change_without_changing_it() {
        let (_home, path) = settings_with(Some(r#"{"model":"opus"}"#));

        let change = plan(&path).expect("a plan");
        assert_eq!(change.events, EVENTS);
        assert!(change.takes_status_line);
        assert_eq!(
            std::fs::read_to_string(&path).expect("the file"),
            r#"{"model":"opus"}"#,
            "nothing was written"
        );
    }

    #[test]
    fn an_empty_file_is_treated_as_an_empty_settings_object() {
        let (_home, path) = settings_with(Some("\n"));
        install(&path, &otag()).expect("the install");
        assert!(read_back(&path).get("hooks").is_some());
    }

    #[test]
    fn a_settings_file_that_is_not_json_is_left_alone() {
        let (_home, path) = settings_with(Some("{ not json"));
        let error = install(&path, &otag()).expect_err("it cannot be read");
        assert!(error.to_string().contains("cannot read"), "{error}");
        assert_eq!(
            std::fs::read_to_string(&path).expect("the file"),
            "{ not json",
            "the file was not touched"
        );
    }

    #[test]
    fn a_settings_file_that_is_not_an_object_is_left_alone() {
        let (_home, path) = settings_with(Some("[1, 2, 3]"));
        let error = install(&path, &otag()).expect_err("it is not a settings file");
        assert!(
            error.to_string().contains("not a settings object"),
            "{error}"
        );
    }

    #[test]
    fn a_settings_file_otag_writes_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let (_home, path) = settings_with(None);
        install(&path, &otag()).expect("the install");

        let mode = std::fs::metadata(&path)
            .expect("the file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "mode {mode:o}");
    }

    #[test]
    fn otag_s_own_commands_are_told_from_everything_else() {
        assert!(is_ours(Some("/usr/local/bin/otag hook Stop")));
        assert!(is_ours(Some("'/My Tools/otag' hook statusline")));
        assert!(is_ours(Some("otag hook Stop")));

        assert!(!is_ours(None));
        assert!(!is_ours(Some("")));
        assert!(!is_ours(Some("python3 line.py")));
        assert!(
            !is_ours(Some("otag ls")),
            "otag's other commands are not hooks"
        );
        assert!(
            !is_ours(Some("my-otag-wrapper hook Stop")),
            "something else that mentions otag is not otag"
        );
    }
}
