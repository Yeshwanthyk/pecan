//! Up-front readiness: is `pi` runnable and is its agent directory usable?
//!
//! The phone shows this instead of discovering a missing `pi` through a
//! worker spawn that times out.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

/// Bound on `pi --version`; node cold starts are slow but not this slow.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// Reuse a probe result for this long.
const CACHE_TTL: Duration = Duration::from_secs(60);

/// Overall readiness.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PiStatus {
    /// `pi` ran and reported a version.
    Ok,
    /// No `pi` executable on `PATH`.
    Missing,
    /// `pi` exists but failed, hung, or printed nothing.
    Error,
}

/// Probe result served at `GET /api/health`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Health {
    /// Overall readiness.
    pub(crate) status: PiStatus,
    /// Version string from `pi --version`, when it ran.
    pub(crate) version: Option<String>,
    /// Human-readable problem, when not ok.
    pub(crate) detail: Option<String>,
    /// Whether the agent directory exists.
    pub(crate) agent_dir: bool,
    /// Whether Pi has stored credentials (`auth.json`). Informational: env
    /// keys and custom providers also work without it.
    pub(crate) auth_file: bool,
}

/// Cached health probe.
#[derive(Debug, Clone, Default)]
pub(crate) struct HealthCache {
    last: Arc<Mutex<Option<(Instant, Health)>>>,
}

impl HealthCache {
    /// Returns a cached result or probes `pi` again.
    pub(crate) async fn get(&self, agent_dir: &std::path::Path, force: bool) -> Health {
        if !force
            && let Ok(last) = self.last.lock()
            && let Some((at, health)) = last.as_ref()
            && at.elapsed() < CACHE_TTL
        {
            return health.clone();
        }
        let health = probe("pi", agent_dir).await;
        if let Ok(mut last) = self.last.lock() {
            *last = Some((Instant::now(), health.clone()));
        }
        health
    }
}

async fn probe(program: &str, agent_dir: &std::path::Path) -> Health {
    let agent_dir_exists = agent_dir.is_dir();
    let auth_file = agent_dir.join("auth.json").is_file();
    let run = tokio::process::Command::new(program)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let (status, version, detail) = match tokio::time::timeout(PROBE_TIMEOUT, run).await {
        Err(_elapsed) => (PiStatus::Error, None, Some(format!("`{program} --version` timed out"))),
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            (PiStatus::Missing, None, Some(format!("`{program}` is not on PATH")))
        }
        Ok(Err(error)) => (PiStatus::Error, None, Some(format!("cannot run `{program}`: {error}"))),
        Ok(Ok(output)) => classify(program, &output),
    };
    Health { status, version, detail, agent_dir: agent_dir_exists, auth_file }
}

fn classify(
    program: &str,
    output: &std::process::Output,
) -> (PiStatus, Option<String>, Option<String>) {
    let text = if output.stdout.iter().any(|byte| !byte.is_ascii_whitespace()) {
        String::from_utf8_lossy(&output.stdout)
    } else {
        String::from_utf8_lossy(&output.stderr)
    };
    let first = text.lines().map(str::trim).find(|line| !line.is_empty()).map(str::to_owned);
    match (output.status.success(), first) {
        (true, Some(version)) => (PiStatus::Ok, Some(version), None),
        (true, None) => {
            (PiStatus::Error, None, Some(format!("`{program} --version` printed nothing")))
        }
        (false, first) => (
            PiStatus::Error,
            None,
            Some(first.unwrap_or_else(|| format!("`{program} --version` failed"))),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{PiStatus, probe};

    #[tokio::test]
    async fn missing_binary_reports_missing() {
        let dir = std::env::temp_dir();
        let health = probe("pecan-definitely-not-a-real-binary", &dir).await;
        assert_eq!(health.status, PiStatus::Missing, "unknown program is missing");
        assert!(health.detail.is_some(), "missing explains itself");
        assert!(health.agent_dir, "temp dir exists");
    }

    #[tokio::test]
    async fn runnable_binary_reports_its_version_line() {
        // `sh --version`-style probes vary; `echo` always prints its args.
        let dir = std::env::temp_dir().join("pecan-health-no-such-dir");
        let health = probe("echo", &dir).await;
        assert_eq!(health.status, PiStatus::Ok, "echo runs");
        assert_eq!(
            health.version.as_deref(),
            Some("--version"),
            "first stdout line is the version"
        );
        assert!(!health.agent_dir, "absent agent dir is reported");
        assert!(!health.auth_file, "absent auth file is reported");
    }

    #[tokio::test]
    async fn failing_binary_reports_error() {
        let dir = std::env::temp_dir();
        let health = probe("false", &dir).await;
        assert_eq!(health.status, PiStatus::Error, "non-zero exit is an error");
    }
}
