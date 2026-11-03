//! Telling the user, outside the terminal, that an agent needs them.
//!
//! The whole point of otag is that an agent waiting for permission does not
//! have to be watched. That only works if otag can reach the user when the
//! terminal is not in front of them, so a pane whose agent starts wanting
//! something sends one desktop notification.
//!
//! One. Not one a second while the agent waits, and not one for every change
//! a pane goes through: only for the moment a pane starts wanting the user,
//! and never again until it stops wanting them. A notifier that cries wolf
//! gets muted, and then the feature is worth nothing.
//!
//! Notifications are best-effort by design: no desktop, no notification
//! program, a user who has them turned off at the system level — all of these
//! are silent, because otag's job is the panes.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use otag_agents::PaneStatus;
use otag_core::pane::PaneId;
use otag_core::session::Session;

/// Environment variable that turns notifications off.
///
/// There is no configuration file yet; when there is, this becomes a setting.
pub const DISABLE_ENV: &str = "OTAG_NO_NOTIFY";

/// What the notification is from, as the desktop shows it.
const TITLE: &str = "otag";

/// How long a notification program may take before otag stops waiting.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Watch the panes and notify when one starts wanting the user.
pub async fn keep_watching(session: Arc<Session>) {
    if is_disabled() {
        tracing::info!("desktop notifications are off ({DISABLE_ENV} is set)");
        return;
    }
    let Some(sender) = Sender::of_this_desktop() else {
        tracing::info!("no way to send desktop notifications on this system");
        return;
    };

    let mut changes = session.changes();
    let mut known: HashMap<PaneId, PaneStatus> = HashMap::new();

    loop {
        for message in worth_telling(&session, &mut known) {
            sender.send(&message).await;
        }
        if changes.changed().await.is_err() {
            // The session is gone, which means the daemon is stopping.
            return;
        }
    }
}

/// What has happened since the last look that the user should hear about.
///
/// Updates what is known about every pane as it goes, so a pane that keeps
/// wanting the user is mentioned once and a pane that stops wanting them can
/// ask again later.
fn worth_telling(session: &Session, known: &mut HashMap<PaneId, PaneStatus>) -> Vec<String> {
    let panes = session.agent_panes();
    let mut messages = Vec::new();

    for pane in &panes {
        let before = known.insert(pane.id, pane.status);
        let Some(agent) = pane.agent else {
            continue;
        };
        if !pane.status.wants_attention() {
            continue;
        }
        // Only the moment it starts wanting the user. The same status seen
        // again is the same situation, not a new one.
        if before == Some(pane.status) {
            continue;
        }
        messages.push(format!("{agent} in pane {} {}", pane.id, pane.status));
    }

    // A pane that has gone must not keep its status forever, or the pane
    // that reuses its number — there is none, but a restored session can
    // bring a number back — would be silently muted.
    known.retain(|id, _| panes.iter().any(|pane| pane.id == *id));
    messages
}

/// Whether the user has turned notifications off.
fn is_disabled() -> bool {
    disabled_by(std::env::var_os(DISABLE_ENV).as_deref())
}

/// Whether that value of the variable means "off".
///
/// Set but empty means on: that is what an unset variable looks like after
/// most shells' quoting accidents, and silently muting otag because of one
/// would be hard to explain.
fn disabled_by(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| {
        let value = value.to_string_lossy().trim().to_ascii_lowercase();
        !value.is_empty() && value != "0" && value != "false" && value != "no"
    })
}

/// How this system shows a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sender {
    /// macOS, through `osascript`.
    MacOs,
    /// A freedesktop system, through `notify-send`.
    NotifySend,
}

impl Sender {
    /// The way this system notifies, if otag knows one and it is installed.
    fn of_this_desktop() -> Option<Self> {
        if cfg!(target_os = "macos") && which("osascript") {
            return Some(Self::MacOs);
        }
        which("notify-send").then_some(Self::NotifySend)
    }

    /// Show one notification, or quietly give up.
    async fn send(self, message: &str) {
        let mut command = match self {
            Self::MacOs => {
                let mut command = tokio::process::Command::new("osascript");
                command.arg("-e").arg(format!(
                    "display notification {} with title {}",
                    applescript(message),
                    applescript(TITLE)
                ));
                command
            }
            Self::NotifySend => {
                let mut command = tokio::process::Command::new("notify-send");
                // `--` so a message beginning with a dash is a message and
                // not an option.
                command
                    .arg("--app-name")
                    .arg(TITLE)
                    .arg("--")
                    .arg(TITLE)
                    .arg(message);
                command
            }
        };

        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        match tokio::time::timeout(TIMEOUT, command.status()).await {
            Ok(Ok(status)) if status.success() => {}
            // Every other outcome is the desktop's business, not otag's.
            other => tracing::debug!(?other, "a desktop notification was not shown"),
        }
    }
}

/// A piece of text as an AppleScript string literal.
///
/// The message contains an agent's words, so it is quoted rather than pasted
/// into the script: a stray quotation mark would otherwise be read as
/// AppleScript.
fn applescript(text: &str) -> String {
    let escaped: String = text
        .chars()
        .filter(|character| !character.is_control())
        .flat_map(|character| match character {
            '"' => vec!['\\', '"'],
            '\\' => vec!['\\', '\\'],
            other => vec![other],
        })
        .collect();
    format!("\"{escaped}\"")
}

/// Whether a program is on the path.
fn which(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| directory.join(program).is_file())
}

#[cfg(test)]
mod tests {
    use otag_agents::AgentKind;
    use otag_core::pane::{PaneSize, PaneSpec};

    use super::*;

    async fn session_with_a_pane() -> (Arc<Session>, PaneId) {
        let session = Session::new(PaneSpec::command(["sleep", "30"]));
        session
            .attach(PaneSize::new(24, 80), None)
            .await
            .expect("a pane");
        let pane = session
            .visible_panes()
            .first()
            .copied()
            .expect("the pane is there");
        (session, pane)
    }

    #[tokio::test]
    async fn a_pane_that_starts_wanting_the_user_is_worth_telling_once() {
        let (session, pane) = session_with_a_pane().await;
        session.set_agent(pane, Some(AgentKind::Claude)).await;
        let mut known = HashMap::new();

        // Working is not something to interrupt anyone for.
        session.set_status(pane, PaneStatus::Working, true).await;
        assert!(worth_telling(&session, &mut known).is_empty());

        session
            .set_status(pane, PaneStatus::NeedsApproval, true)
            .await;
        let messages = worth_telling(&session, &mut known);
        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains("claude"), "{}", messages[0]);
        assert!(messages[0].contains("needs approval"), "{}", messages[0]);

        assert!(
            worth_telling(&session, &mut known).is_empty(),
            "the same agent waiting is the same situation"
        );
    }

    #[tokio::test]
    async fn an_agent_that_goes_from_one_problem_to_another_is_told_again() {
        let (session, pane) = session_with_a_pane().await;
        session.set_agent(pane, Some(AgentKind::Claude)).await;
        let mut known = HashMap::new();

        session
            .set_status(pane, PaneStatus::NeedsApproval, true)
            .await;
        assert_eq!(worth_telling(&session, &mut known).len(), 1);

        session.set_status(pane, PaneStatus::Error, true).await;
        let messages = worth_telling(&session, &mut known);
        assert_eq!(messages.len(), 1, "an error is news even after a question");
        assert!(messages[0].contains("error"), "{}", messages[0]);
    }

    #[tokio::test]
    async fn an_agent_that_asks_again_after_working_is_told_again() {
        let (session, pane) = session_with_a_pane().await;
        session.set_agent(pane, Some(AgentKind::Claude)).await;
        let mut known = HashMap::new();

        session
            .set_status(pane, PaneStatus::NeedsApproval, true)
            .await;
        assert_eq!(worth_telling(&session, &mut known).len(), 1);

        session.set_status(pane, PaneStatus::Working, true).await;
        assert!(worth_telling(&session, &mut known).is_empty());

        session
            .set_status(pane, PaneStatus::NeedsApproval, true)
            .await;
        assert_eq!(
            worth_telling(&session, &mut known).len(),
            1,
            "a second question is a second interruption"
        );
    }

    #[tokio::test]
    async fn a_pane_with_no_agent_in_it_is_never_announced() {
        let (session, pane) = session_with_a_pane().await;
        let mut known = HashMap::new();

        session
            .set_status(pane, PaneStatus::NeedsApproval, true)
            .await;
        assert!(
            worth_telling(&session, &mut known).is_empty(),
            "a shell is the user's own business"
        );
    }

    #[test]
    fn a_message_is_quoted_before_it_reaches_applescript() {
        assert_eq!(applescript("plain"), "\"plain\"");
        assert_eq!(
            applescript("say \"hello\""),
            "\"say \\\"hello\\\"\"",
            "a quotation mark in an agent's words is not a script"
        );
        assert_eq!(applescript("back\\slash"), "\"back\\\\slash\"");
        assert_eq!(
            applescript("one\ntwo"),
            "\"onetwo\"",
            "a newline would end the script line"
        );
    }

    #[test]
    fn notifications_are_on_unless_the_variable_says_otherwise() {
        use std::ffi::OsStr;

        assert!(!disabled_by(None));
        assert!(!disabled_by(Some(OsStr::new(""))), "set but empty means on");
        assert!(!disabled_by(Some(OsStr::new("0"))));
        assert!(!disabled_by(Some(OsStr::new("false"))));
        assert!(!disabled_by(Some(OsStr::new("no"))));
        assert!(disabled_by(Some(OsStr::new("1"))));
        assert!(disabled_by(Some(OsStr::new("yes"))));
        assert!(disabled_by(Some(OsStr::new(" TRUE "))));
    }

    #[test]
    fn a_program_that_is_not_installed_is_not_found() {
        assert!(!which("otag-no-such-program-exists"));
        assert!(which("sh"), "every system otag runs on has a shell");
    }

    #[test]
    fn the_shape_of_a_message_says_what_and_where() {
        // The notification has one line and no room for anything but the
        // agent, the pane and what it wants.
        let message = format!("{} in pane {} {}", AgentKind::Codex, 3, PaneStatus::Error);
        assert_eq!(message, "codex in pane 3 error");
    }
}
