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
            tx.try_send(touched).unwrap_or_default();
        }
    })
    .map_err(std::io::Error::other)?;

    let watched = [
        (paths.sessions_dir(), RecursiveMode::Recursive),
        (paths.tasks_dir(), RecursiveMode::NonRecursive),
        // Workflow runs are `runs/project-<hash>/<runId>/journal.json`.
        (paths.workflows_dir(), RecursiveMode::Recursive),
    ];
    for (dir, mode) in &watched {
        if dir.exists() {
            debouncer.watch(dir, *mode).map_err(std::io::Error::other)?;
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
        || name == "journal.json"
}

/// Emits `thread-changed` for changed transcript files present in the index.
async fn notify_threads(app: &Arc<crate::server::snapshot::App>, changed: &HashSet<PathBuf>) {
    use crate::server::snapshot::ServerEvent;
    let snap = app.snapshot().await;
    let run_ids: HashSet<String> = changed
        .iter()
        .filter(|path| path.file_name().is_some_and(|name| name == "journal.json"))
        .filter_map(|path| path.parent()?.file_name()?.to_str().map(str::to_owned))
        .collect();
    let workflow_threads: HashSet<PathBuf> = if run_ids.is_empty() {
        HashSet::new()
    } else {
        app.threads_with_workflow_runs(&run_ids)
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "workflow thread lookup failed");
                Vec::new()
            })
            .into_iter()
            .collect()
    };
    let mut seen: HashSet<&str> = HashSet::new();
    for row in snap.sessions.iter() {
        let touched =
            changed.contains(&row.summary.path) || workflow_threads.contains(&row.summary.path);
        if touched && seen.insert(row.summary.id.as_str()) {
            app.events.send(ServerEvent::ThreadChanged { id: row.summary.id.clone() });
        }
    }
}
