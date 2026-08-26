//! RPC worker management: one lazily spawned `pi --mode rpc` child per session.
//!
//! Setting `PECAN_PI_EXTENSIONS` (comma/space separated absolute paths) pins
//! explicit `--extension` entry points into every worker — e.g. the real
//! local pi-askuser package — matching the proof harness in
//! `scripts/prove-askuser-bridge.sh`. Unset by default: pi's own package
//! discovery stays on.
//!
//! Workers are the write path. Reads stay filesystem-driven; a worker exists
/// only while the user is actively steering a thread, and idle workers are
/// reaped after [`IDLE_REAP_MS`] (the same lifecycle pican uses).
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, broadcast, oneshot};

/// Idle workers are killed after this long without a command.
pub(crate) const IDLE_REAP_MS: i64 = 10 * 60 * 1000;
/// Reap sweep cadence.
const REAP_SWEEP_EVERY: Duration = Duration::from_secs(30);
/// Per-command response timeout; cold pi starts with heavy extensions can be slow.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

/// Errors from the worker layer.
#[derive(Debug, thiserror::Error)]
pub(crate) enum WorkerError {
    /// The `pi` executable is not available.
    #[error("pi executable not found")]
    PiMissing,
    /// Spawning or talking to the worker failed.
    #[error("worker io failed: {0}")]
    Io(#[from] std::io::Error),
    /// The worker rejected the command or died before replying.
    #[error("worker rejected: {0}")]
    Rejected(String),
    /// No reply within the command timeout.
    #[error("worker timed out")]
    Timeout,
}

impl WorkerError {
    fn rejected(value: &serde_json::Value) -> Self {
        let msg =
            value.get("error").and_then(serde_json::Value::as_str).unwrap_or("command failed");
        WorkerError::Rejected(msg.to_owned())
    }
}

/// A live `pi --mode rpc` child process with request correlation.
pub(crate) struct WorkerHandle {
    next_id: AtomicU64,
    stdin: Mutex<tokio::process::ChildStdin>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<serde_json::Value>>>>,
    /// Raw pi events for SSE fan-out (responses excluded).
    pub raw_events: broadcast::Sender<serde_json::Value>,
    child: Mutex<tokio::process::Child>,
    last_used_ms: AtomicI64,
    /// Process start time (epoch millis); responses recorded before this
    /// moment cannot belong to this process.
    spawned_ms: i64,
    forward_started: AtomicBool,
    /// Working directory used by a newly-created, not-yet-persisted session.
    cwd: std::path::PathBuf,
}

impl std::fmt::Debug for WorkerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerHandle").finish_non_exhaustive()
    }
}

impl WorkerHandle {
    /// Returns the worker's working directory.
    pub(crate) fn cwd(&self) -> &std::path::Path {
        &self.cwd
    }

    /// Sends one RPC command and awaits its correlated response.
    ///
    /// # Errors
    /// Returns [`WorkerError`] on I/O failure, rejection, or timeout.
    pub(crate) async fn command(
        &self,
        mut cmd: serde_json::Value,
    ) -> Result<serde_json::Value, WorkerError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let key = format!("pecan-{id}");
        cmd["id"] = serde_json::Value::String(key.clone());
        self.touch();

        let (send, recv) = oneshot::channel();
        self.pending.lock().await.insert(key.clone(), send);
        let line = format!("{}\n", cmd);
        let write = async {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await
        };
        if let Err(error) = write.await {
            self.pending.lock().await.remove(&key);
            return Err(WorkerError::Io(error));
        }

        match tokio::time::timeout(COMMAND_TIMEOUT, recv).await {
            Ok(Ok(value)) => {
                let success = value.get("success").and_then(serde_json::Value::as_bool);
                match success {
                    Some(true) => Ok(value),
                    _ => Err(WorkerError::rejected(&value)),
                }
            }
            Ok(Err(_)) => Err(WorkerError::Rejected("worker dropped the request".to_owned())),
            Err(_) => {
                self.pending.lock().await.remove(&key);
                Err(WorkerError::Timeout)
            }
        }
    }

    /// Convenience: `get_state`.
    pub(crate) async fn get_state(&self) -> Result<serde_json::Value, WorkerError> {
        let res = self.command(serde_json::json!({"type": "get_state"})).await?;
        Ok(res.get("data").cloned().unwrap_or(serde_json::Value::Null))
    }

    /// Writes a protocol frame without id rewriting and without awaiting a
    /// correlated reply (e.g. `extension_ui_response`, which must keep the
    /// request's own id).
    ///
    /// # Errors
    /// Returns [`WorkerError`] when the pipe is broken.
    pub(crate) async fn send_raw(&self, frame: serde_json::Value) -> Result<(), WorkerError> {
        self.touch();
        let line = format!("{frame}\n");
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    /// Kills the child process.
    ///
    /// # Errors
    /// Returns [`WorkerError::Io`] when the kill itself fails; absence is fine.
    pub(crate) async fn shutdown(self: Arc<Self>) {
        let mut child = self.child.lock().await;
        let _ = child.start_kill();
    }

    /// Claims the single SSE-forwarding task for this worker.
    pub(crate) fn try_start_forwarding(&self) -> bool {
        self.forward_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn touch(&self) {
        self.last_used_ms.store(now_millis(), Ordering::Relaxed);
    }

    /// Process start time (epoch millis).
    pub(crate) fn spawned_ms(&self) -> i64 {
        self.spawned_ms
    }

    fn idle_for(&self) -> i64 {
        now_millis().saturating_sub(self.last_used_ms.load(Ordering::Relaxed))
    }
}

fn now_millis() -> i64 {
    jiff::Timestamp::now().as_second().saturating_mul(1_000)
}

/// Owns all live workers keyed by session id. Clone shares one pool.
#[derive(Debug, Clone, Default)]
pub(crate) struct Workers {
    map: Arc<Mutex<HashMap<String, Arc<WorkerHandle>>>>,
}

impl Workers {
    /// Creates an empty pool and starts the idle-reaper.
    #[must_use]
    pub(crate) fn new() -> Self {
        let pool = Self::default();
        let weak = Arc::downgrade(&pool.map);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(REAP_SWEEP_EVERY).await;
                let Some(map) = weak.upgrade() else { return };
                reap_once(&map).await;
            }
        });
        pool
    }

    /// Returns the live worker for `session_id`, spawning one when absent.
    ///
    /// # Errors
    /// Returns [`WorkerError`] when pi is missing or startup/attach fails.
    pub(crate) async fn get_or_spawn(
        &self,
        session_id: &str,
        session_path: &std::path::Path,
    ) -> Result<Arc<WorkerHandle>, WorkerError> {
        if let Some(existing) = self.map.lock().await.get(session_id) {
            return Ok(existing.clone());
        }
        let handle = spawn_worker(Some(session_path), None).await?;
        self.map.lock().await.insert(session_id.to_owned(), handle.clone());
        Ok(handle)
    }

    /// Registers an already-spawned worker (e.g. a freshly created session).
    pub(crate) async fn insert(&self, session_id: String, handle: Arc<WorkerHandle>) {
        self.map.lock().await.insert(session_id, handle);
    }

    /// Spawns a brand-new pi session rooted at `cwd`.
    ///
    /// # Errors
    /// Returns [`WorkerError`] when pi is missing or startup fails.
    pub(crate) async fn spawn_new(
        &self,
        cwd: &std::path::Path,
    ) -> Result<Arc<WorkerHandle>, WorkerError> {
        spawn_worker(None, Some(cwd)).await
    }

    /// Drops a worker unconditionally (e.g. after it crashed).
    pub(crate) async fn remove(&self, session_id: &str) -> Option<Arc<WorkerHandle>> {
        self.map.lock().await.remove(session_id)
    }

    /// Returns a live worker by session id, when present.
    pub(crate) async fn get(&self, session_id: &str) -> Option<Arc<WorkerHandle>> {
        self.map.lock().await.get(session_id).cloned()
    }
}

/// Kills and removes workers idle beyond [`IDLE_REAP_MS`].
async fn reap_once(map: &Mutex<HashMap<String, Arc<WorkerHandle>>>) {
    let stale: Vec<String> = {
        let guard = map.lock().await;
        guard
            .iter()
            .filter(|(_, w)| w.idle_for() > IDLE_REAP_MS)
            .map(|(id, _)| id.clone())
            .collect()
    };
    if stale.is_empty() {
        return;
    }
    let mut guard = map.lock().await;
    for id in stale {
        tracing::info!(%id, "reaping idle rpc worker");
        if let Some(worker) = guard.remove(&id) {
            worker.shutdown().await;
        }
    }
}

/// Parses the `PECAN_PI_EXTENSIONS` value into explicit `pi --extension`
/// entry points. Entries are split on commas, spaces, and tabs; empty entries
/// are dropped. The variable is unset by default, so pi's own package
/// discovery (and everything installed there, e.g. pi-askuser) still applies.
pub(crate) fn parse_extension_env(raw: &str) -> Vec<String> {
    raw.split([',', ' ', '\t']).filter(|entry| !entry.is_empty()).map(str::to_owned).collect()
}

/// Reads `PECAN_PI_EXTENSIONS` once per worker spawn; absent means no
/// explicit loads and ambient discovery stays on.
fn read_extension_args() -> Vec<String> {
    std::env::var("PECAN_PI_EXTENSIONS").map(|raw| parse_extension_env(&raw)).unwrap_or_default()
}

/// Spawns and wires one worker: reader task routes responses to waiters and
/// forwards everything else onto the event channel.
///
/// `session_path` attaches an existing transcript; `cwd` roots a fresh one.
async fn spawn_worker(
    session_path: Option<&std::path::Path>,
    cwd: Option<&std::path::Path>,
) -> Result<Arc<WorkerHandle>, WorkerError> {
    let mut cmd = tokio::process::Command::new("pi");
    cmd.args(["--mode", "rpc"]);
    // `PECAN_PI_EXTENSIONS` pins explicit extension files (e.g. the local
    // pi-askuser package) into every worker, mirroring the RPC proof
    // harness; unset by default so nothing changes on existing setups.
    for extension in read_extension_args() {
        cmd.arg("--extension").arg(extension);
    }
    // `--session <path>` attaches the real transcript at startup, so every
    // prompt lands in the same .jsonl our filesystem read path watches.
    if let Some(path) = session_path {
        cmd.arg("--session").arg(path);
    }
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            WorkerError::PiMissing
        } else {
            WorkerError::Io(e)
        }
    })?;
    let stdin = child.stdin.take().ok_or_else(|| WorkerError::Rejected("no stdin".to_owned()))?;
    let stdout =
        child.stdout.take().ok_or_else(|| WorkerError::Rejected("no stdout".to_owned()))?;
    let stderr =
        child.stderr.take().ok_or_else(|| WorkerError::Rejected("no stderr".to_owned()))?;

    let (events_tx, _) = broadcast::channel::<serde_json::Value>(256);
    let handle = Arc::new(WorkerHandle {
        next_id: AtomicU64::new(0),
        stdin: Mutex::new(stdin),
        pending: Arc::new(Mutex::new(HashMap::new())),
        raw_events: events_tx,
        child: Mutex::new(child),
        last_used_ms: AtomicI64::new(now_millis()),
        spawned_ms: now_millis(),
        forward_started: AtomicBool::new(false),
        cwd: cwd.map(std::path::Path::to_owned).unwrap_or_default(),
    });

    // Drain stderr continuously. A piped stderr that is never read can fill
    // its OS buffer and block pi before it emits the session id.
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if !line.trim().is_empty() {
                tracing::warn!("pi worker stderr: {line}");
            }
        }
    });

    // Reader: strict LF framing per the RPC protocol.
    let weak = Arc::downgrade(&handle);
    let pending = Arc::clone(&handle.pending);
    tokio::spawn(async move {
        let mut buf = stdout;
        let mut carry: Vec<u8> = Vec::with_capacity(16 * 1024);
        let mut chunk = [0_u8; 8 * 1024];
        'read: loop {
            match buf.read(&mut chunk).await {
                Ok(0) => break 'read,
                Ok(n) => carry.extend_from_slice(&chunk[..n]),
                Err(_) => break 'read,
            }
            while let Some(pos) = carry.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = carry.drain(..=pos).collect();
                let mut text = String::from_utf8_lossy(&line[..line.len() - 1]).to_string();
                if text.ends_with('\r') {
                    text.pop();
                }
                if text.is_empty() {
                    continue;
                }
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                    continue;
                };
                if value.get("type").and_then(serde_json::Value::as_str) == Some("response") {
                    if let Some(id) = value.get("id").and_then(serde_json::Value::as_str) {
                        if let Some(sender) = pending.lock().await.remove(id) {
                            let _ = sender.send(value);
                        }
                    }
                    continue;
                }
                if let Some(handle) = weak.upgrade() {
                    let _ = handle.raw_events.send(value);
                }
            }
        }
        // Child exited: fail every waiter so callers do not hang.
        for (_, sender) in pending.lock().await.drain() {
            let _ = sender.send(serde_json::json!({
                "type": "response", "success": false, "error": "worker exited",
            }));
        }
    });

    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::parse_extension_env;

    #[test]
    fn extension_env_splits_on_separators_and_drops_empties() {
        assert_eq!(parse_extension_env(""), Vec::<String>::new(), "empty value yields no entries");
        assert_eq!(
            parse_extension_env("/a.ts,/b.ts /c.ts"),
            vec!["/a.ts".to_owned(), "/b.ts".to_owned(), "/c.ts".to_owned()],
            "commas and spaces both split",
        );
        assert_eq!(
            parse_extension_env("  /a.ts\t,/b.ts ,"),
            vec!["/a.ts".to_owned(), "/b.ts".to_owned()],
            "leading, trailing, and doubled separators are ignored",
        );
    }

    #[test]
    fn pi_subagents_wiring_env_pins_the_subagents_entry_point() {
        // The PECAN_PI_EXTENSIONS value the pi-subagents slice documents
        // (the real local pi-subagents extension only; the activity rail is
        // opt-in) must map to one explicit `pi --extension` arg.
        assert_eq!(
            parse_extension_env("/ext/pi-subagents/extensions/subagents/index.ts"),
            vec!["/ext/pi-subagents/extensions/subagents/index.ts".to_owned()],
            "the subagents entry point must survive the env split",
        );
    }

    #[test]
    fn pi_subagents_opt_in_rail_rides_along_in_order_when_set() {
        // Operators opt the activity rail in by appending its entry point to
        // PECAN_PI_EXTENSIONS (the proof script's PECAN_ACTIVITY_RAIL_EXTENSION
        // is a script-level convenience that composes the same env value); an
        // extra entry must keep its position after the subagents entry.
        assert_eq!(
            parse_extension_env(
                "/ext/pi-subagents/extensions/subagents/index.ts,/ext/pi-subagents/extensions/activity-rail/index.ts",
            ),
            vec![
                "/ext/pi-subagents/extensions/subagents/index.ts".to_owned(),
                "/ext/pi-subagents/extensions/activity-rail/index.ts".to_owned(),
            ],
            "subagents then opt-in rail entry points must survive the env split in order",
        );
    }

    #[test]
    fn pi_subagents_wiring_env_ignores_trailing_separators() {
        assert_eq!(
            parse_extension_env("/ext/pi-subagents/extensions/subagents/index.ts, "),
            vec!["/ext/pi-subagents/extensions/subagents/index.ts".to_owned()],
            "trailing commas/spaces are dropped from the documented value",
        );
    }
}
