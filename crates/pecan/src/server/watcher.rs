//! Filesystem watcher: rebuilds the index when pi writes sessions/tasks/workflows.
//!
//! Uses a debounced full refresh (the scan cache makes this cheap) and emits
//! per-thread `ThreadChanged` events for whichever transcript files actually
//! changed so open views can live-update.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, new_debouncer};
use tokio::sync::mpsc;

/// Collapse bursts of writes into one refresh.
const DEBOUNCE: Duration = Duration::from_millis(350);
/// Channel capacity before coalescing drops older signals.
const CHANNEL_DEPTH: usize = 16;

/// Installs watchers over pi's data directories and spawns the refresh loop.
///
/// # Errors
/// Returns [`std::io::Error`] when a watch cannot be installed on the root
/// directories.
pub(crate) fn spawn(
    paths: &pecan_core::PiPaths,
    app: Arc<crate::server::snapshot::App>,
    project_roots: &[PathBuf],
) -> std::io::Result<WatcherGuard> {
    let (tx, mut rx) = mpsc::channel::<Vec<PathBuf>>(CHANNEL_DEPTH);

    let mut debouncer = new_debouncer(DEBOUNCE, None, move |result: DebounceEventResult| {
        let Ok(events) = result else { return };
        let mut touched: Vec<PathBuf> = Vec::new();
        for event in &events {
            for path in &event.paths {
                if is_relevant(path) {
                    touched.push(path.to_path_buf());
                }
            }
        }
        if !touched.is_empty() {
            let _ = tx.try_send(touched);
        }
    })
    .map_err(std::io::Error::other)?;

    let watched = [
        (paths.sessions_dir(), RecursiveMode::Recursive),
        (paths.tasks_dir(), RecursiveMode::NonRecursive),
        // Workflow runs are directories containing workflow.json and sidecars.
        (paths.workflows_dir(), RecursiveMode::Recursive),
    ];
    for (dir, mode) in &watched {
        if dir.exists() {
            debouncer.watch(dir, mode.clone()).map_err(std::io::Error::other)?;
        }
    }
    for root in project_roots {
        let pi_dir = root.join(".pi");
        if pi_dir.exists() {
            debouncer.watch(&pi_dir, RecursiveMode::Recursive).map_err(std::io::Error::other)?;
        }
    }

    tokio::spawn(async move {
        while let Some(first) = rx.recv().await {
            let mut changed: HashSet<PathBuf> = HashSet::from_iter(first);
            while let Ok(more) = rx.try_recv() {
                changed.extend(more);
            }
            match app.refresh().await {
                Ok(()) => notify_threads(&app, &changed).await,
                Err(error) => tracing::warn!(%error, "index refresh failed"),
            }
        }
    });

    // Keep the debouncer alive for as long as the guard lives.
    Ok(WatcherGuard { _debouncer: Box::new(debouncer) })
}

/// Owns the watcher lifetime; dropping it stops watching.
pub(crate) struct WatcherGuard {
    _debouncer: Box<dyn std::any::Any + Send>,
}

/// Whether a filesystem event should trigger an index refresh.
fn is_relevant(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if name.ends_with(".tmp") || name.starts_with('.') {
        return false;
    }
    name.ends_with(".jsonl")
        || name.starts_with("tasks-")
        || name == "tasks.json"
        || name == "tasks-config.json"
        || name == "workflow.json"
        || name == "result.json"
}

/// Emits `thread-changed` for changed transcript files present in the index.
async fn notify_threads(app: &Arc<crate::server::snapshot::App>, changed: &HashSet<PathBuf>) {
    use crate::server::snapshot::ServerEvent;
    let snap = app.snapshot().await;
    let mut seen: HashSet<&str> = HashSet::new();
    for row in snap.sessions.iter() {
        if changed.contains(&row.summary.path) && seen.insert(row.summary.id.as_str()) {
            let _ = app.events.send(ServerEvent::ThreadChanged { id: row.summary.id.clone() });
        }
    }
}
