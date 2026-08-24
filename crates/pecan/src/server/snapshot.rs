//! Shared server state: the in-memory index snapshot plus event broadcast.
//!
//! The snapshot is rebuilt wholesale on filesystem changes (cheap: header
//! reads + mtime cache) and swapped atomically; request handlers always read
//! a consistent view without locking the scan.

use std::collections::{HashMap, HashSet};
use std::io::BufRead;
use std::sync::Arc;

use pecan_core::scan::ScanCache;
use pecan_core::session::{SessionKind, SessionSummary};
use pecan_core::store::StateStore;
use pecan_core::tasks::TaskList;
use pecan_core::thread::ThreadEntry;
use pecan_core::thread::{ThreadView, parse_thread};
use pecan_core::workflows::WorkflowRun;
use pecan_core::{CoreError, PiPaths};
use tokio::sync::broadcast;

/// Sessions modified within this window get a bounded tail scan for an
/// unanswered `ask_user` call; older files never pay that cost.
const WAITING_SCAN_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;
/// A conservative ceiling for heuristic child discovery in embedded mode.
const MAX_SCOPED_SUBAGENTS: usize = 32;

/// One cached parsed transcript keyed by file identity.
#[derive(Debug)]
pub(crate) struct CachedThread {
    modified: std::time::SystemTime,
    bytes: u64,
    view: Arc<ThreadView>,
}

/// Parsed transcripts keyed by path, so repeated reads of one session
/// (index waiting-checks, thread views, SSE refreshes) skip re-parsing.
/// Session files are append-only, so `(mtime, size)` detects new entries.
pub(crate) type ThreadCache = HashMap<std::path::PathBuf, CachedThread>;

/// Upper bound on cached transcripts; the cap bounds memory when a machine
/// accumulates thousands of sessions. Oversubscription clears the map and
/// rebuilds it from the sessions actually being read.
const THREAD_CACHE_CAP: usize = 1024;

/// Restricts one server instance to a main session and its bounded child set.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionScope {
    /// The only normal session exposed by the server.
    pub(crate) main_session_id: String,
    /// Documents the conservative fallback used for child discovery.
    pub(crate) child_resolution: &'static str,
    /// Maximum number of child transcripts exposed.
    pub(crate) max_children: usize,
}

impl SessionScope {
    /// Creates a scope rooted at one validated normal session.
    pub(crate) fn new(main_session_id: String) -> Self {
        Self {
            main_session_id,
            child_resolution: "parent-spawn-name-prompt-cwd-time",
            max_children: MAX_SCOPED_SUBAGENTS,
        }
    }

    fn filter_rows(&self, rows: &mut Vec<SessionRow>) {
        let Some(main) = rows.iter().find(|row| row.summary.id == self.main_session_id) else {
            rows.clear();
            return;
        };
        let cwd = main.summary.cwd.clone();
        let opened_at = main.summary.opened_at;
        let spawned_agents = spawned_agents(&main.summary.path);
        let mut child_ids: Vec<String> = rows
            .iter()
            .filter(|row| {
                row.summary.kind == SessionKind::Subagent
                    && row.summary.cwd == cwd
                    && row.summary.opened_at >= opened_at
                    && row.summary.agent_name.as_deref().is_some_and(|raw_name| {
                        let name = normalize_agent_name(raw_name);
                        first_user_prompt(&row.summary.path).is_some_and(|prompt| {
                            spawned_agents
                                .get(name)
                                .is_some_and(|prompts| prompts.contains(&prompt))
                        })
                    })
            })
            .take(self.max_children)
            .map(|row| row.summary.id.clone())
            .collect();
        child_ids.sort_unstable();
        rows.retain(|row| {
            row.summary.id == self.main_session_id
                || child_ids.binary_search(&row.summary.id).is_ok()
        });
    }
}

fn normalize_agent_name(value: &str) -> &str {
    value.strip_prefix("subagents: ").unwrap_or(value).trim()
}

/// Reads explicit `subagent_spawn` names and prompts from the parent transcript.
/// Child session metadata does not carry the parent session UUID, so this is
/// the strongest persisted relationship available without widening the scope.
fn spawned_agents(path: &std::path::Path) -> HashMap<String, HashSet<String>> {
    let Ok(file) = std::fs::File::open(path) else {
        return HashMap::new();
    };
    let mut spawns: HashMap<String, HashSet<String>> = HashMap::new();
    for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let Some(blocks) = value.pointer("/message/content").and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for block in blocks {
            if block.get("type").and_then(serde_json::Value::as_str) != Some("toolCall")
                || block.get("name").and_then(serde_json::Value::as_str) != Some("subagent_spawn")
            {
                continue;
            }
            let name =
                block.pointer("/arguments/name").and_then(serde_json::Value::as_str).map(str::trim);
            let prompt = block
                .pointer("/arguments/prompt")
                .and_then(serde_json::Value::as_str)
                .map(str::trim);
            if let (Some(name), Some(prompt)) = (name, prompt)
                && !name.is_empty()
                && !prompt.is_empty()
            {
                spawns.entry(name.to_owned()).or_default().insert(prompt.to_owned());
            }
        }
    }
    spawns
}

fn first_user_prompt(path: &std::path::Path) -> Option<String> {
    parse_thread(path).ok()?.entries.into_iter().find_map(|dated| match dated.entry {
        ThreadEntry::User { text, .. } => Some(text.trim().to_owned()),
        _ => None,
    })
}

/// One session row as served to the client: summary + derived flags.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionRow {
    /// Core summary (flattened).
    #[serde(flatten)]
    pub(crate) summary: SessionSummary,
    /// Whether the user settled this thread.
    pub(crate) settled: bool,
    /// Whether the user pinned this thread.
    pub(crate) pinned: bool,
    /// Whether the transcript ends with an unanswered `ask_user` call.
    pub(crate) waiting_askuser: bool,
}

/// Everything list views need, rebuilt atomically on changes.
#[derive(Debug, Clone)]
pub(crate) struct IndexSnapshot {
    /// All sessions sorted by open time, newest first.
    pub(crate) sessions: Arc<Vec<SessionRow>>,
    /// Task lists grouped by session id.
    pub(crate) tasks: HashMap<String, Vec<TaskList>>,
    /// Workflow runs grouped by session id.
    pub(crate) workflows: HashMap<String, Vec<WorkflowRun>>,
}

impl IndexSnapshot {
    /// Builds a fresh snapshot from disk plus current settle state.
    ///
    /// # Errors
    /// Returns [`pecan_core::CoreError`] when the sessions tree is unreadable
    /// or the state store fails.
    pub(crate) fn build(
        paths: &PiPaths,
        store: &StateStore,
        scope: Option<&SessionScope>,
        scans: &mut ScanCache,
        threads: &mut ThreadCache,
    ) -> pecan_core::Result<Self> {
        let summaries = scans.refresh(&paths.sessions_dir())?;
        let settled = store.settled()?;
        let pinned = store.pinned()?;
        let titles = store.titles()?;
        let now_ms = jiff::Timestamp::now().as_second().saturating_mul(1_000);
        let mut rows: Vec<SessionRow> = Vec::with_capacity(summaries.len());
        for mut s in summaries {
            s.title = titles.get(&s.id).cloned();
            let recent = now_ms.saturating_sub(s.last_activity.as_second().saturating_mul(1_000))
                < WAITING_SCAN_WINDOW_MS;
            let waiting_askuser = recent
                && s.kind == SessionKind::Normal
                && cached_thread_view(&s.path, threads)
                    .map(|view| view.waiting_askuser)
                    .unwrap_or(false);
            let settled_flag = settled.contains_key(&s.id);
            let pinned_flag = pinned.contains_key(&s.id);
            rows.push(SessionRow {
                summary: s,
                settled: settled_flag,
                pinned: pinned_flag,
                waiting_askuser,
            });
        }
        if let Some(scope) = scope {
            scope.filter_rows(&mut rows);
        }
        let visible_ids: HashSet<&str> = rows.iter().map(|row| row.summary.id.as_str()).collect();
        let tasks = if crate::server::ui_plugins::pi_tasks_enabled(paths, store)? {
            let added_cwds = if scope.is_some() {
                None
            } else {
                Some(
                    store
                        .projects()?
                        .into_iter()
                        .filter(|(_, preference)| preference.added)
                        .map(|(cwd, _)| cwd)
                        .collect::<HashSet<_>>(),
                )
            };
            pecan_core::tasks::load_for_sessions(
                rows.iter()
                    .filter(|row| {
                        added_cwds.as_ref().is_none_or(|cwds| cwds.contains(&row.summary.cwd))
                    })
                    .map(|row| (row.summary.id.as_str(), row.summary.cwd.as_str())),
                &paths.tasks_dir(),
            )
        } else {
            HashMap::new()
        };
        let mut workflows = pecan_core::workflows::load_all(&paths.workflows_dir());
        workflows.retain(|id, _| visible_ids.contains(id.as_str()));
        Ok(Self { sessions: Arc::new(rows), tasks, workflows })
    }
}

/// Returns the cached transcript for `path`, parsing (and caching) on miss.
///
/// Unreadable files yield `None` rather than failing the whole index build.
fn cached_thread_view(
    path: &std::path::Path,
    threads: &mut ThreadCache,
) -> Option<Arc<ThreadView>> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let bytes = meta.len();
    if let Some(cached) = threads.get(path) {
        if cached.modified == modified && cached.bytes == bytes {
            return Some(cached.view.clone());
        }
    }
    let view = Arc::new(parse_thread(path).ok()?);
    if threads.len() >= THREAD_CACHE_CAP {
        threads.clear();
    }
    threads.insert(path.to_path_buf(), CachedThread { modified, bytes, view: view.clone() });
    Some(view)
}

/// Events pushed to browser clients over SSE.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(crate) enum ServerEvent {
    /// The index changed; client should refresh lists.
    IndexChanged,
    /// A specific thread gained new entries.
    ThreadChanged {
        /// Session id whose transcript changed.
        id: String,
    },
    /// A raw pi agent event forwarded from an RPC worker.
    AgentEvent {
        /// Session id of the worker that emitted it.
        id: String,
        /// The pi event payload.
        event: serde_json::Value,
    },
}

/// Handle shared across handlers and background tasks.
#[derive(Clone)]
pub(crate) struct App {
    /// Resolved pi paths.
    pub(crate) paths: PiPaths,
    /// SQLite-backed curation state.
    pub(crate) store: Arc<std::sync::Mutex<StateStore>>,
    /// Latest index snapshot.
    pub(crate) snapshot: Arc<tokio::sync::RwLock<Arc<IndexSnapshot>>>,
    /// Fan-out channel for [`ServerEvent`]s.
    pub(crate) events: broadcast::Sender<ServerEvent>,
    /// Latest title-generation request number per session.
    pub(crate) title_generations: Arc<std::sync::Mutex<HashMap<String, u64>>>,
    /// Serializes checkout-wide Ship mutations so two browser clients cannot race.
    pub(crate) ship_lock: Arc<tokio::sync::Mutex<()>>,
    /// Incremental session scanner shared across snapshot rebuilds.
    pub(crate) scan_cache: Arc<std::sync::Mutex<ScanCache>>,
    /// Parsed transcripts shared across snapshot rebuilds and thread views.
    pub(crate) thread_cache: Arc<std::sync::Mutex<ThreadCache>>,
    /// Per-launch capability required for credential-bearing Ship mutations.
    pub(crate) ship_token: Arc<str>,
    /// Optional single-session boundary for embedded clients.
    pub(crate) session_scope: Option<SessionScope>,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App").field("paths", &self.paths).finish()
    }
}

impl App {
    /// Marks a new title request and returns its per-session generation.
    pub(crate) fn begin_title_generation(&self, id: &str) -> pecan_core::Result<u64> {
        let mut generations =
            self.title_generations.lock().map_err(|_poisoned| CoreError::LockPoisoned)?;
        let generation = generations.get(id).copied().map_or(0, |value| value).saturating_add(1);
        generations.insert(id.to_owned(), generation);
        Ok(generation)
    }

    /// Persists a title only while its generation is still current.
    ///
    /// The generation lock remains held through the store write, preventing a
    /// newer request from starting between the freshness check and persistence.
    pub(crate) fn commit_title_if_current(
        &self,
        id: &str,
        generation: u64,
        title: &str,
    ) -> pecan_core::Result<bool> {
        let generations =
            self.title_generations.lock().map_err(|_poisoned| CoreError::LockPoisoned)?;
        if generations.get(id).copied() != Some(generation) {
            return Ok(false);
        }
        let store = self.store.lock().map_err(|_poisoned| CoreError::LockPoisoned)?;
        store.set_title(id, title)?;
        Ok(true)
    }

    /// Rebuilds the snapshot and notifies SSE subscribers.
    ///
    /// # Errors
    /// Returns errors from snapshot construction.
    pub(crate) async fn refresh(&self) -> pecan_core::Result<()> {
        let paths = self.paths.clone();
        let store = self.store.clone();
        let scope = self.session_scope.clone();
        let scans = self.scan_cache.clone();
        let threads = self.thread_cache.clone();
        let built = tokio::task::spawn_blocking(move || {
            let store = store.lock().map_err(|_| CoreError::LockPoisoned)?;
            let mut scans = scans.lock().map_err(|_| CoreError::LockPoisoned)?;
            let mut threads = threads.lock().map_err(|_| CoreError::LockPoisoned)?;
            IndexSnapshot::build(&paths, &store, scope.as_ref(), &mut scans, &mut threads)
        })
        .await
        .map_err(|_| CoreError::Join)??;
        *self.snapshot.write().await = Arc::new(built);
        let _ = self.events.send(ServerEvent::IndexChanged);
        Ok(())
    }

    /// Returns the parsed transcript at `path`, reusing the cache when the
    /// file is unchanged since the last parse.
    ///
    /// # Errors
    /// Returns [`CoreError`] when stat or parse fails or a lock is poisoned.
    pub(crate) async fn thread_view(
        &self,
        path: std::path::PathBuf,
    ) -> pecan_core::Result<Arc<ThreadView>> {
        let meta = std::fs::metadata(&path)
            .map_err(|source| CoreError::Io { path: path.clone(), source })?;
        let modified =
            meta.modified().map_err(|source| CoreError::Io { path: path.clone(), source })?;
        let bytes = meta.len();
        {
            let cache = self.thread_cache.lock().map_err(|_| CoreError::LockPoisoned)?;
            if let Some(cached) = cache.get(&path) {
                if cached.modified == modified && cached.bytes == bytes {
                    return Ok(cached.view.clone());
                }
            }
        }
        let parse_path = path.clone();
        let view = tokio::task::spawn_blocking(move || parse_thread(&parse_path))
            .await
            .map_err(|_| CoreError::Join)??;
        let view = Arc::new(view);
        {
            let mut cache = self.thread_cache.lock().map_err(|_| CoreError::LockPoisoned)?;
            if cache.len() >= THREAD_CACHE_CAP {
                cache.clear();
            }
            cache.insert(path, CachedThread { modified, bytes, view: view.clone() });
        }
        Ok(view)
    }

    /// Current snapshot handle.
    pub(crate) async fn snapshot(&self) -> Arc<IndexSnapshot> {
        self.snapshot.read().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use jiff::Timestamp;
    use pecan_core::session::{SessionKind, SessionSummary};

    use super::{SessionRow, SessionScope};

    fn row(id: &str, cwd: &str, opened_at: &str, kind: SessionKind) -> SessionRow {
        let opened_at = opened_at.parse::<Timestamp>().expect("valid test timestamp");
        SessionRow {
            summary: SessionSummary {
                id: id.to_owned(),
                path: PathBuf::from(format!("/{id}.jsonl")),
                cwd: cwd.to_owned(),
                opened_at,
                last_activity: opened_at,
                bytes: 1,
                provider: None,
                model: None,
                preview: None,
                title: None,
                kind,
                parent_id: None,
                agent_name: None,
            },
            settled: false,
            pinned: false,
            waiting_askuser: false,
        }
    }

    fn parent_with_spawn(name: &str) -> SessionRow {
        let mut parent = row("main", "/project", "2026-01-01T00:00:00Z", SessionKind::Normal);
        let path = std::env::temp_dir().join(format!(
            "pecan-scope-parent-{}-{}.jsonl",
            std::process::id(),
            name
        ));
        let line = serde_json::json!({
            "type": "message",
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "toolCall",
                    "name": "subagent_spawn",
                    "arguments": { "name": name, "prompt": "Bound task" }
                }]
            }
        });
        std::fs::write(&path, format!("{line}\n")).expect("write parent fixture");
        parent.summary.path = path;
        parent
    }

    fn child_with_prompt(id: &str, name: &str, prompt: &str, opened_at: &str) -> SessionRow {
        let mut child = row(id, "/project", opened_at, SessionKind::Subagent);
        child.summary.agent_name = Some(format!("subagents: {name}"));
        let path = std::env::temp_dir()
            .join(format!("pecan-scope-child-{}-{id}.jsonl", std::process::id()));
        let header = serde_json::json!({
            "type": "session",
            "id": id,
            "timestamp": opened_at,
            "cwd": "/project"
        });
        let message = serde_json::json!({
            "type": "message",
            "timestamp": opened_at,
            "message": {
                "role": "user",
                "content": [{ "type": "text", "text": prompt }]
            }
        });
        std::fs::write(&path, format!("{header}\n{message}\n")).expect("write child fixture");
        child.summary.path = path;
        child
    }

    #[test]
    fn session_scope_keeps_only_explicitly_spawned_later_same_cwd_subagents() {
        let child = child_with_prompt("child", "owned-child", "Bound task", "2026-01-02T00:00:00Z");
        let unrelated =
            child_with_prompt("unrelated", "owned-child", "Different task", "2026-01-03T00:00:00Z");
        let child_path = child.summary.path.clone();
        let unrelated_path = unrelated.summary.path.clone();
        let parent = parent_with_spawn("owned-child");
        let parent_path = parent.summary.path.clone();
        let mut rows = vec![
            row("other-main", "/project", "2026-01-04T00:00:00Z", SessionKind::Normal),
            row("other-cwd-child", "/elsewhere", "2026-01-03T00:00:00Z", SessionKind::Subagent),
            unrelated,
            child,
            parent,
            row("old-child", "/project", "2025-12-31T00:00:00Z", SessionKind::Subagent),
        ];

        SessionScope::new("main".to_owned()).filter_rows(&mut rows);

        let ids: Vec<&str> = rows.iter().map(|candidate| candidate.summary.id.as_str()).collect();
        assert_eq!(ids, vec!["child", "main"]);
        let _ = std::fs::remove_file(parent_path);
        let _ = std::fs::remove_file(child_path);
        let _ = std::fs::remove_file(unrelated_path);
    }

    #[test]
    fn missing_scope_main_fails_closed() {
        let mut rows =
            vec![row("child", "/project", "2026-01-02T00:00:00Z", SessionKind::Subagent)];

        SessionScope::new("missing".to_owned()).filter_rows(&mut rows);

        assert!(rows.is_empty());
    }
}
