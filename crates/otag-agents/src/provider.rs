//! API-provider launch policy, transient credentials, and authenticated balance reads.

use crate::{AgentAdapter, AgentKind, PaneStatus, SessionRef, Transcript};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

/// Keychain service shared with the native configuration sheet.
pub const SERVICE: &str = "com.otag.api.deepseek";
/// Keychain account; this is metadata, never an authentication value.
pub const ACCOUNT: &str = "deepseek";
/// Verified provider models exposed by the configuration sheet.
pub const MODELS: [&str; 3] = ["deepseek-flash[1m]", "deepseek-flash", "deepseek-v4-pro"];
/// Default model from the provider's integration guide.
pub const DEFAULT_MODEL: &str = MODELS[0];
const MARKER: &str = "@otag:provider:deepseek";
const ENDPOINT: &str = "https://api.deepseek.com/anthropic";

struct Secret(Vec<u8>);
impl Drop for Secret {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}
static SECRETS: OnceLock<Mutex<Vec<Arc<Secret>>>> = OnceLock::new();

fn register(secret: &Arc<Secret>) {
    if let Ok(mut registry) = SECRETS.get_or_init(|| Mutex::new(Vec::new())).lock()
        && !registry.iter().any(|s| s.0 == secret.0)
    {
        registry.push(Arc::clone(secret));
        registry.sort_by_key(|s| std::cmp::Reverse(s.0.len()));
    }
}

/// Scrub active provider credentials from transcript-derived text before persistence.
pub fn redact_text(text: &str) -> String {
    let mut text = text.to_owned();
    if let Some(registry) = SECRETS.get()
        && let Ok(registry) = registry.lock()
    {
        for secret in registry.iter() {
            if let Ok(key) = std::str::from_utf8(&secret.0) {
                text = text.replace(key, "[redacted]");
            }
        }
    }
    text
}

/// Streaming output filter; handles a credential split across terminal reads.
#[derive(Default)]
pub struct OutputFilter {
    secret: Option<Arc<Secret>>,
    pending: Vec<u8>,
}
impl std::fmt::Debug for OutputFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OutputFilter([redacted])")
    }
}
impl OutputFilter {
    /// Return safe output, retaining only a possible credential prefix.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        let Some(secret) = &self.secret else {
            return bytes.to_vec();
        };
        let key = &secret.0;
        self.pending.extend_from_slice(bytes);
        let mut output = Vec::new();
        while let Some(index) = self.pending.windows(key.len()).position(|s| s == key) {
            output.extend_from_slice(&self.pending[..index]);
            output.extend_from_slice(b"[redacted]");
            self.pending.drain(..index + key.len());
        }
        let retained = (1..key.len())
            .rev()
            .find(|&n| self.pending.ends_with(&key[..n]))
            .unwrap_or(0);
        let ready = self.pending.len() - retained;
        output.extend(self.pending.drain(..ready));
        output
    }
    /// Flush remaining non-complete prefix on process exit.
    pub fn finish(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending)
    }
}

/// A launch-time value. It cannot be serialized and its debug output omits values.
pub struct Prepared {
    command: Vec<OsString>,
    environment: Vec<(OsString, OsString)>,
    removed: Vec<&'static str>,
    filter: OutputFilter,
}
impl std::fmt::Debug for Prepared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Prepared([redacted])")
    }
}
impl Prepared {
    /// Resolve non-secret launch metadata, reading Keychain only for a provider session.
    pub fn resolve(command: &[OsString]) -> io::Result<Self> {
        if command.first().is_some_and(|v| v == MARKER) {
            let model = command
                .get(1)
                .and_then(|s| s.to_str())
                .ok_or_else(invalid_model)?;
            if !MODELS.contains(&model) {
                return Err(invalid_model());
            }
            Self::with_credential(model, &command[2..], read_key()?)
        } else {
            Ok(Self {
                command: command.to_vec(),
                environment: Vec::new(),
                removed: Vec::new(),
                filter: OutputFilter::default(),
            })
        }
    }
    /// Prepare an isolated launch with a transient credential supplied by a credential store.
    pub fn with_credential(model: &str, command: &[OsString], key: Vec<u8>) -> io::Result<Self> {
        if !MODELS.contains(&model) {
            return Err(invalid_model());
        }
        if key.is_empty() || key.len() > 4096 || !key.iter().all(|b| b.is_ascii_graphic()) {
            return Err(io::Error::other(
                "The API key is invalid. Enter a valid key in API LLM.",
            ));
        }
        let secret = Arc::new(Secret(key));
        register(&secret);
        let token = OsString::from(
            String::from_utf8(secret.0.clone())
                .map_err(|_| io::Error::other("The API key is invalid."))?,
        );
        let env = vec![
            ("ANTHROPIC_BASE_URL".into(), ENDPOINT.into()),
            ("ANTHROPIC_AUTH_TOKEN".into(), token),
            ("ANTHROPIC_MODEL".into(), model.into()),
            ("ANTHROPIC_DEFAULT_OPUS_MODEL".into(), model.into()),
            ("ANTHROPIC_DEFAULT_SONNET_MODEL".into(), model.into()),
            ("ANTHROPIC_DEFAULT_HAIKU_MODEL".into(), model.into()),
            ("CLAUDE_CODE_SUBAGENT_MODEL".into(), model.into()),
        ];
        Ok(Self {
            command: command.to_vec(),
            environment: env,
            removed: vec!["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"],
            filter: OutputFilter {
                secret: Some(secret),
                pending: Vec::new(),
            },
        })
    }
    /// Actual executable and arguments; provider metadata has been resolved.
    pub fn command(&self) -> &[OsString] {
        &self.command
    }
    /// Environment changes applied only to this child at spawn time.
    pub fn environment(&self) -> &[(OsString, OsString)] {
        &self.environment
    }
    /// Conflicting inherited authentication variables to remove for this child.
    pub fn removed(&self) -> &[&'static str] {
        &self.removed
    }
    /// Whether the launch contains credentials and needs sanitized process errors.
    pub fn is_private(&self) -> bool {
        self.filter.secret.is_some()
    }
    /// Transfer the output filter to the PTY reader; no secret enters saved pane specs.
    pub fn take_filter(&mut self) -> OutputFilter {
        std::mem::take(&mut self.filter)
    }
}

fn invalid_model() -> io::Error {
    io::Error::other("Choose a supported API model in API LLM.")
}

#[cfg(target_os = "macos")]
fn key_query() -> Command {
    let mut command = Command::new("/usr/bin/security");
    command
        .args(["find-generic-password", "-s", SERVICE, "-a", ACCOUNT])
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    command
}
/// Check Keychain metadata without retrieving the key.
pub fn configured() -> bool {
    if std::env::var_os("OTAG_DISABLE_API_PROVIDER").is_some() {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        key_query()
            .stdout(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}
fn read_key() -> io::Result<Vec<u8>> {
    if std::env::var_os("OTAG_DISABLE_API_PROVIDER").is_some() {
        return Err(io::Error::other(
            "API providers are disabled for this daemon.",
        ));
    }
    #[cfg(target_os = "macos")]
    {
        let output = key_query().arg("-w").output().map_err(|_| {
            io::Error::other("Cannot access Keychain. Unlock it and configure API LLM.")
        })?;
        if !output.status.success() {
            return Err(io::Error::other(
                "No API key is available. Unlock Keychain and configure API LLM.",
            ));
        }
        let mut key = output.stdout;
        while key.last().is_some_and(|b| matches!(*b, b'\n' | b'\r')) {
            key.pop();
        }
        Ok(key)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(io::Error::other(
            "API provider sessions require macOS Keychain.",
        ))
    }
}

/// Read the documented balance endpoint; never expose raw errors or response bodies.
pub fn balance() -> io::Result<Option<String>> {
    let secret = Secret(read_key()?);
    let token = std::str::from_utf8(&secret.0)
        .map_err(|_| io::Error::other("Check the API key in Keychain."))?;
    let mut header = ureq::http::HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| io::Error::other("Check the API key in Keychain."))?;
    header.set_sensitive(true);
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .max_redirects(0)
        .https_only(true)
        .build();
    let client: ureq::Agent = config.into();
    let mut response = client
        .get("https://api.deepseek.com/user/balance")
        .header("Authorization", header)
        .call()
        .map_err(|_| {
            io::Error::other(
                "The API connection could not be verified. Check the key and network connection.",
            )
        })?;
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|_| io::Error::other("The provider balance is unavailable."))?;
    parse_balance(&body)
}

fn parse_balance(body: &str) -> io::Result<Option<String>> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|_| io::Error::other("The provider balance is unavailable."))?;
    let rows = value.get("balance_infos").and_then(|v| v.as_array());
    let amounts: Vec<String> = rows
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let currency = row.get("currency")?.as_str()?;
            let amount = row.get("total_balance")?.as_str()?;
            if !matches!(currency, "USD" | "CNY")
                || amount.is_empty()
                || amount.len() > 30
                || !amount
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b == b'.' || b == b'-')
            {
                return None;
            }
            Some(format!("{currency} {amount}"))
        })
        .collect();
    Ok((!amounts.is_empty()).then(|| amounts.join(" · ")))
}

/// Wrap a command in serializable provider metadata; the key is acquired later.
pub fn wrap(mut command: Vec<String>, model: Option<&str>) -> Vec<String> {
    let model = model.unwrap_or(DEFAULT_MODEL);
    if command.first().is_some_and(|s| s == "claude") && !command.iter().any(|s| s == "--model") {
        command.splice(1..1, ["--model".into(), model.into()]);
    }
    let mut wrapped = vec![MARKER.into(), model.into()];
    wrapped.extend(command);
    wrapped
}
/// Preserve the selected provider model when passive session discovery records resume arguments.
pub fn preserve_model(previous: &[String], mut next: Vec<String>) -> Vec<String> {
    if previous.first().is_some_and(|s| s == MARKER) && next.first().is_some_and(|s| s == "claude")
    {
        next = wrap(next, previous.get(1).map(String::as_str));
    }
    if previous.first().is_some_and(|s| s == MARKER)
        && next.first().is_some_and(|s| s == MARKER)
        && let Some(model) = previous.get(1)
    {
        if let Some(next_model) = next.get_mut(1) {
            *next_model = model.clone();
        }
        if let Some(index) = next.iter().position(|s| s == "--model")
            && let Some(value) = next.get_mut(index + 1)
        {
            *value = model.clone();
        }
    }
    next
}
/// Select a provider model in non-secret launch metadata and the compatible CLI arguments.
pub fn select_model(command: Vec<String>, model: &str) -> Vec<String> {
    preserve_model(&[MARKER.into(), model.into()], command)
}

/// Provider adapter sharing transcript semantics with its compatible CLI.
pub struct Deepseek;
impl AgentAdapter for Deepseek {
    fn kind(&self) -> AgentKind {
        AgentKind::Deepseek
    }
    fn matches_process(&self, exe: &str, argv: &[String]) -> bool {
        crate::claude::Claude.matches_process(exe, argv)
            && argv.iter().any(|s| MODELS.contains(&s.as_str()))
    }
    fn is_asking(&self, screen: &str) -> bool {
        crate::claude::Claude.is_asking(screen)
    }
    fn asking_summary(&self, screen: &str) -> Option<String> {
        crate::claude::Claude.asking_summary(screen)
    }
    fn answer_keys(&self, answer: crate::approval::Answer) -> Vec<u8> {
        crate::claude::Claude.answer_keys(answer)
    }
    fn detect_status(&self, screen: &str) -> Option<PaneStatus> {
        crate::claude::Claude.detect_status(screen)
    }
    fn session_store(&self, home: &Path) -> PathBuf {
        crate::claude::Claude.session_store(home)
    }
    fn find_session_for(
        &self,
        home: &Path,
        cwd: &Path,
        after: Option<SystemTime>,
    ) -> Option<SessionRef> {
        crate::claude::Claude.find_session_for(home, cwd, after)
    }
    fn start_command(&self, prompt: Option<&str>, model: Option<&str>) -> Vec<String> {
        let model = model.unwrap_or(DEFAULT_MODEL);
        wrap(
            crate::claude::Claude.start_command(prompt, Some(model)),
            Some(model),
        )
    }
    fn resume_command(&self, session: &SessionRef) -> Vec<String> {
        self.resume_on_model(session, DEFAULT_MODEL)
    }
    fn resume_on_model(&self, session: &SessionRef, model: &str) -> Vec<String> {
        wrap(
            crate::claude::Claude.resume_on_model(session, model),
            Some(model),
        )
    }
    fn read_transcript(&self, path: &Path) -> crate::Result<Transcript> {
        let mut transcript = crate::claude::Claude.read_transcript(path)?;
        transcript.prompts = transcript.prompts.iter().map(|s| redact_text(s)).collect();
        transcript.last_message = transcript.last_message.map(|s| redact_text(&s));
        transcript.final_message = transcript.final_message.map(|s| redact_text(&s));
        transcript.commands = transcript.commands.iter().map(|s| redact_text(s)).collect();
        Ok(transcript)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    #[test]
    fn credentials_are_not_arguments_debug_output_or_other_launches() {
        let fake = "fake-provider-key-for-isolation-check";
        let launch =
            Prepared::with_credential(DEFAULT_MODEL, &["claude".into()], fake.as_bytes().to_vec())
                .expect("launch");
        assert!(!format!("{launch:?}").contains(fake));
        assert!(
            !launch
                .command()
                .iter()
                .any(|s| s.to_string_lossy().contains(fake))
        );
        assert!(
            launch
                .environment()
                .iter()
                .any(|(k, v)| k == "ANTHROPIC_AUTH_TOKEN" && v == OsStr::new(fake))
        );
        assert!(
            Prepared::resolve(&["/bin/sh".into()])
                .expect("unrelated")
                .environment()
                .is_empty()
        );
        assert_eq!(redact_text(fake), "[redacted]");
    }
    #[test]
    fn split_terminal_reads_never_expose_a_complete_credential() {
        let fake = b"fake-provider-key";
        for split in 0..=fake.len() {
            let mut launch =
                Prepared::with_credential(DEFAULT_MODEL, &["claude".into()], fake.to_vec())
                    .expect("launch");
            let mut filter = launch.take_filter();
            let mut output = filter.feed(&fake[..split]);
            output.extend(filter.feed(&fake[split..]));
            output.extend(filter.finish());
            assert_eq!(output, b"[redacted]");
        }
    }
    #[test]
    fn selected_models_and_provider_identity_survive_passive_resume_discovery() {
        let start = Deepseek.start_command(Some("Task"), Some(MODELS[2]));
        let resumed = preserve_model(
            &start,
            vec!["claude".into(), "--resume".into(), "sample-session".into()],
        );
        assert_eq!(resumed[0], MARKER);
        assert_eq!(resumed[1], MODELS[2]);
        assert_eq!(
            crate::adapter_for_process("claude", &resumed[3..]).map(AgentAdapter::kind),
            Some(AgentKind::Deepseek)
        );
        let policy = crate::team::command_on_model(
            AgentKind::Deepseek,
            Path::new("/project"),
            "Role task",
            true,
            Some(MODELS[2]),
        );
        let model = policy.iter().position(|s| s == "--model").expect("model");
        let prompt = policy
            .iter()
            .position(|s| s == "--")
            .expect("prompt delimiter");
        assert!(model < prompt);
        assert_eq!(policy[model + 1], MODELS[2]);
        assert!(policy.iter().any(|s| s == "--settings"));
    }
    #[test]
    fn balance_parser_exposes_only_valid_currency_and_amount_fields() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/deepseek-balance.json");
        assert_eq!(
            parse_balance(&std::fs::read_to_string(path).expect("fixture")).expect("balance"),
            Some("USD 12.30".into())
        );
        assert!(
            parse_balance(
                r#"{"balance_infos":[{"currency":"USD","total_balance":"fake-provider-key"}]}"#
            )
            .expect("safe")
            .is_none()
        );
        assert!(parse_balance("{}").expect("missing fields").is_none());
    }
}
