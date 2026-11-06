//! Regression for the complete transient-key -> PTY -> history/database boundary.
use otag_agents::{AgentKind, provider};
use otag_core::pane::{PaneId, PaneSize, PaneSpec};
use otag_core::pty::{Pty, PtyEvent};
use otag_core::snapshot::{PaneSnapshot, Snapshot};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::test]
async fn fake_provider_key_never_reaches_logs_state_database_or_saved_history() {
    let directory = tempfile::tempdir().expect("isolated state");
    let fake = "fake-key-for-state-and-log-isolation";
    let launch = provider::Prepared::with_credential(
        provider::DEFAULT_MODEL,
        &[
            "/bin/sh".into(),
            "-c".into(),
            "printf '%s' \"$ANTHROPIC_AUTH_TOKEN\"".into(),
        ],
        fake.as_bytes().to_vec(),
    )
    .expect("transient launch");
    let log_path = directory.path().join("daemon.log");
    let log = std::fs::File::create(&log_path).expect("log");
    let subscriber = tracing_subscriber::registry()
        .with(crate::logging::env_filter(
            Some("trace,ureq::run=trace"),
            "trace",
        ))
        .with(tracing_subscriber::filter::filter_fn(
            crate::logging::allowed_target,
        ))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .without_time()
                .with_writer(move || log.try_clone().expect("log handle")),
        )
        .set_default();
    tracing::info!(?launch, "prepared isolated provider launch");
    tracing::trace!(target: "ureq::run", credential = fake, "must never be logged");
    let metadata = AgentKind::Deepseek
        .adapter()
        .start_command(None, Some(provider::DEFAULT_MODEL));
    let spec = PaneSpec::command(&metadata);
    let (_pty, mut output) =
        Pty::spawn_prepared(PaneId::FIRST, &spec, PaneSize::DEFAULT, launch).expect("PTY");
    let mut bytes = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(event) = output.recv().await {
            match event {
                PtyEvent::Output(chunk) => bytes.extend(chunk),
                PtyEvent::Exited(_) => break,
            }
        }
    })
    .await
    .expect("provider exits");
    let text = String::from_utf8(bytes).expect("output");
    assert_eq!(text, "[redacted]");
    tracing::info!(output = %text, "provider output");
    drop(subscriber);
    let database = directory.path().join("state.sqlite");
    let store = otag_persist::Store::open(&database).expect("state");
    let mut snapshot = Snapshot::default();
    snapshot.panes.push(PaneSnapshot {
        id: PaneId::FIRST,
        title: "Provider".into(),
        cwd: None,
        nickname: String::new(),
        command: metadata,
    });
    store.save_session(&snapshot).expect("save state");
    store
        .save_history(PaneId::FIRST, &[text])
        .expect("save history");
    assert!(
        !store
            .load_history(PaneId::FIRST)
            .expect("decoded history")
            .join("\n")
            .contains(fake)
    );
    drop(store);
    for path in [&database, &log_path] {
        let bytes = std::fs::read(path).expect("scan artifact");
        assert!(
            !bytes.windows(fake.len()).any(|s| s == fake.as_bytes()),
            "credential leaked to a persisted artifact"
        );
    }
}
