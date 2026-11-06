//! Logging setup.
//!
//! Client commands log to stderr and stay quiet by default, so that `otag
//! ping` prints `pong` and nothing else. The daemon logs to a daily-rolling
//! file in its log directory, because it has no terminal to write to.

use std::path::Path;

use anyhow::{Context, Result};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Environment variable holding the default log filter.
const LOG_ENV: &str = "OTAG_LOG";

/// Set up stderr logging for a client command.
///
/// Failures are ignored: a client that cannot set up logging should still run
/// its command.
pub fn init_client(filter: Option<&str>) {
    let _ = tracing_subscriber::registry()
        .with(env_filter(filter, "warn"))
        .with(tracing_subscriber::filter::filter_fn(allowed_target))
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .without_time()
                .with_writer(std::io::stderr),
        )
        .try_init();
}

/// Set up file logging for the daemon.
///
/// The returned guard flushes buffered lines when dropped and must be kept
/// alive for as long as the daemon runs.
pub fn init_daemon(log_dir: &Path, filter: Option<&str>) -> Result<WorkerGuard> {
    std::fs::create_dir_all(log_dir)
        .with_context(|| format!("cannot create the log directory {log_dir:?}"))?;

    let appender = tracing_appender::rolling::daily(log_dir, "otag.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);

    tracing_subscriber::registry()
        .with(env_filter(filter, "info"))
        .with(tracing_subscriber::filter::filter_fn(allowed_target))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(writer),
        )
        .try_init()
        .map_err(|error| anyhow::anyhow!("cannot set up daemon logging: {error}"))?;

    Ok(guard)
}

/// Build a filter from the flag, then `OTAG_LOG`, then the given default.
pub(crate) fn env_filter(filter: Option<&str>, default: &str) -> EnvFilter {
    let mut filter = selected_filter(filter, default);
    // Transport wire traces can contain authentication headers.
    for directive in ["ureq=off", "ureq_proto=off", "rustls=off"] {
        if let Ok(directive) = directive.parse() {
            filter = filter.add_directive(directive);
        }
    }
    filter
}

/// An unconditional boundary: a more-specific user trace directive cannot override it.
pub(crate) fn allowed_target(metadata: &tracing::Metadata<'_>) -> bool {
    !["ureq", "ureq_proto", "rustls"]
        .iter()
        .any(|target| metadata.target().starts_with(target))
}

fn selected_filter(filter: Option<&str>, default: &str) -> EnvFilter {
    if let Some(filter) = filter
        && let Ok(filter) = EnvFilter::try_new(filter)
    {
        return filter;
    }
    EnvFilter::try_from_env(LOG_ENV).unwrap_or_else(|_| EnvFilter::new(default))
}
