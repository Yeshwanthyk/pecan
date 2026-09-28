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
use pecan_core::thread::{ThreadView, parse_thread};
use pecan_core::{CoreError, PiPaths, workflows};

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

/// One live dialog request from a session's worker (`ask_user`, select,
/// confirm, input, editor), kept so answers survive browser reloads and so
/// stale answers can be rejected instead of silently dropped.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordedAsk {
    /// Dialog method: `select`, `confirm`, `input`, or `editor`.
    pub(crate) method: String,
    /// Optional prompt title.
    pub(crate) title: Option<String>,
    /// Options for `select` dialogs.
    pub(crate) options: Option<Vec<serde_json::Value>>,
    /// Body text shown under the title of `confirm` dialogs.
    pub(crate) message: Option<String>,
    /// Placeholder hint for `input` dialogs.
    pub(crate) placeholder: Option<String>,
    /// Prefilled content for `editor` dialogs.
    pub(crate) prefill: Option<String>,
    /// When the request was observed (epoch millis).
    pub(crate) recorded_at_ms: i64,
}

/// Live dialog requests per session, keyed by pi request id.
pub(crate) type PendingAsks = HashMap<String, HashMap<String, RecordedAsk>>;

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
        let evidence = SpawnEvidence::read(&main.summary.path);
        let mut child_ids: Vec<String> = rows
            .iter()
            .filter(|row| {
                row.summary.kind == SessionKind::Subagent
                    && !row.settled
                    && row.summary.cwd == cwd
                    && row.summary.opened_at >= opened_at
                    && row.summary.agent_name.as_deref().is_some_and(|name| {
                        evidence.owns(name, || first_user_prompt(&row.summary.path))
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

/// Strips the `<owner>: ` prefix pi-subagents puts on child session names
/// (`subagents: scout`, `workflow:wf-1: Build`), leaving the spawn title.
fn normalize_agent_name(value: &str) -> &str {
    let title = match value.split_once(": ") {
        Some((owner, title)) if !owner.is_empty() && !owner.contains(char::is_whitespace) => title,
        _ => value,
    };
    title.trim()
}

/// Persisted parent-side evidence of spawned children. Child session metadata
/// does not carry the parent session UUID, so explicit `subagent_spawn`
/// name/prompt pairs and `workflow*` run ids are the strongest relationship
/// available without widening the scope.
#[derive(Debug, Default)]
struct SpawnEvidence {
    /// `subagent_spawn` names mapped to their prompts.
    spawns: HashMap<String, HashSet<String>>,
    /// Workflow run ids this transcript started or controlled.
    workflow_runs: HashSet<String>,
}

impl SpawnEvidence {
    fn read(path: &std::path::Path) -> Self {
        let mut evidence = Self::default();
        let Ok(file) = std::fs::File::open(path) else {
            return evidence;
        };
        for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let Some(message) = value.get("message") else { continue };
            if let Some(run_id) = workflows::tool_result_run_id(message) {
                evidence.workflow_runs.insert(run_id.to_owned());
                continue;
            }
            let Some(blocks) = message.get("content").and_then(serde_json::Value::as_array) else {
                continue;
            };
            for block in blocks {
                evidence.record_spawn(block);
            }
        }
        evidence
    }

    fn record_spawn(&mut self, block: &serde_json::Value) {
        if block.get("type").and_then(serde_json::Value::as_str) != Some("toolCall")
            || block.get("name").and_then(serde_json::Value::as_str) != Some("subagent_spawn")
        {
            return;
        }
        let name =
            block.pointer("/arguments/name").and_then(serde_json::Value::as_str).map(str::trim);
        let prompt =
            block.pointer("/arguments/prompt").and_then(serde_json::Value::as_str).map(str::trim);
        if let (Some(name), Some(prompt)) = (name, prompt)
            && !name.is_empty()
            && !prompt.is_empty()
        {
            self.spawns.entry(name.to_owned()).or_default().insert(prompt.to_owned());
        }
    }

    /// Whether this transcript spawned the child named `agent_name`. Workflow
    /// children match by run id; direct subagents by spawn name and prompt.
    fn owns<P: AsRef<str>>(&self, agent_name: &str, prompt: impl FnOnce() -> Option<P>) -> bool {
        if let Some((run_id, _)) = workflows::child_session_task(agent_name) {
            return self.workflow_runs.contains(run_id);
        }
        self.spawns
            .get(normalize_agent_name(agent_name))
            .is_some_and(|prompts| prompt().is_some_and(|prompt| prompts.contains(prompt.as_ref())))
    }
}

fn first_user_prompt(path: &std::path::Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(serde_json::Value::as_str) != Some("message") {
            continue;
        }
        let Some(message) = value.get("message") else { continue };
        if message.get("role").and_then(serde_json::Value::as_str) != Some("user") {
            continue;
        }
        let Some(content) = message.get("content") else { continue };
        let text = match content {
            serde_json::Value::String(text) => text.clone(),
            serde_json::Value::Array(blocks) => blocks
                .iter()
                .filter_map(|block| {
                    (block.get("type").and_then(serde_json::Value::as_str) == Some("text"))
                        .then(|| block.get("text").and_then(serde_json::Value::as_str))
                        .flatten()
                })
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    None
}

type ParentCandidate = (String, String, jiff::Timestamp, SpawnEvidence);

/// Resolves subagent sessions to their parent by matching the persisted spawn
/// name and prompt (or workflow run id) against candidate normal-session
/// transcripts.
fn link_parent_sessions(rows: &mut [SessionRow]) {
    let active_child_cwds: HashSet<&str> = rows
        .iter()
        .filter(|row| row.summary.kind == SessionKind::Subagent && !row.settled)
        .map(|row| row.summary.cwd.as_str())
        .collect();
    let parents: Vec<ParentCandidate> = rows
        .iter()
        .filter(|row| {
            row.summary.kind == SessionKind::Normal
                && !row.settled
                && active_child_cwds.contains(row.summary.cwd.as_str())
        })
        .map(|row| {
            (
                row.summary.id.clone(),
                row.summary.cwd.clone(),
                row.summary.opened_at,
                SpawnEvidence::read(&row.summary.path),
            )
        })
        .collect();

    for child in
        rows.iter_mut().filter(|row| row.summary.kind == SessionKind::Subagent && !row.settled)
    {
        let Some(name) = child.summary.agent_name.as_deref() else {
            continue;
        };
        // Read the child's prompt once; workflow children never need it.
        let prompt = if workflows::child_session_task(name).is_some() {
            None
        } else {
            let Some(prompt) = first_user_prompt(&child.summary.path) else { continue };
            Some(prompt)
        };
        child.parent_session_id = parents
            .iter()
            .filter(|(_, cwd, opened_at, evidence)| {
                *cwd == child.summary.cwd
                    && *opened_at <= child.summary.opened_at
                    && evidence.owns(name, || prompt.as_deref())
            })
            .max_by_key(|(_, _, opened_at, _)| *opened_at)
            .map(|(id, _, _, _)| id.clone());
    }
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
    /// Resolved owning session id for a subagent, when persisted evidence identifies it.
    pub(crate) parent_session_id: Option<String>,
}

/// Everything list views need, rebuilt atomically on changes.
#[derive(Debug, Clone)]
pub(crate) struct IndexSnapshot {
    /// All sessions sorted by open time, newest first.
    pub(crate) sessions: Arc<Vec<SessionRow>>,
    /// Task lists grouped by session id.
    pub(crate) tasks: HashMap<String, Vec<TaskList>>,
}

impl IndexSnapshot {
    /// Builds a fresh snapshot from disk plus store data already captured by
    /// [`StoreData::capture`].
    ///
    /// Takes no reference to the live `StateStore`: the filesystem scan and
    /// transcript parsing below can be slow, and the caller captures
    /// `store_data` under a short-lived lock precisely so that lock is not
    /// held for the duration of this call.
    ///
    /// # Errors
    /// Returns [`pecan_core::CoreError`] when the sessions tree is unreadable.
    pub(crate) fn build(
        paths: &PiPaths,
        store_data: &StoreData,
        scope: Option<&SessionScope>,
        scans: &mut ScanCache,
        threads: &mut ThreadCache,
    ) -> pecan_core::Result<Self> {
        let summaries = scans.refresh(&paths.sessions_dir())?;
        let settled = &store_data.settled;
        let pinned = &store_data.pinned;
        let titles = &store_data.titles;
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
                parent_session_id: None,
            });
        }
        if let Some(scope) = scope {
            scope.filter_rows(&mut rows);
            // Embedded mode has a single known parent, so resolve only the
            // already-scoped rows. Global mode uses the frontend's cheap
            // recent-agent fallback and never parses historical transcripts.
            link_parent_sessions(&mut rows);
        }
        let tasks = pecan_core::tasks::load_for_sessions(
            rows.iter()
                .filter(|row| {
                    store_data
                        .added_cwds
                        .as_ref()
                        .is_none_or(|cwds| cwds.contains(&row.summary.cwd))
                })
                .map(|row| (row.summary.id.as_str(), row.summary.cwd.as_str())),
            &paths.tasks_dir(),
        );
        Ok(Self { sessions: Arc::new(rows), tasks })
    }
}

/// Store-derived data captured once, under a short-lived lock, before the
/// filesystem scan and transcript parsing in [`IndexSnapshot::build`].
pub(crate) struct StoreData {
    settled: HashMap<String, i64>,
    pinned: HashMap<String, i64>,
    titles: HashMap<String, String>,
    /// Added project cwds, only meaningful in global (non-scoped) mode.
    added_cwds: Option<HashSet<String>>,
}

impl StoreData {
    /// Reads everything [`IndexSnapshot::build`] needs from the store.
    ///
    /// # Errors
    /// Returns [`pecan_core::CoreError`] when the state store fails.
    fn capture(store: &StateStore, scope: Option<&SessionScope>) -> pecan_core::Result<Self> {
        let settled = store.settled()?;
        let pinned = store.pinned()?;
        let titles = store.titles()?;
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
        Ok(Self { settled, pinned, titles, added_cwds })
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
    if let Some(cached) = threads.get(path)
        && cached.modified == modified
        && cached.bytes == bytes
    {
        return Some(Arc::clone(&cached.view));
    }
    let view = Arc::new(parse_thread(path).ok()?);
    if threads.len() >= THREAD_CACHE_CAP {
        threads.clear();
    }
    threads.insert(path.to_path_buf(), CachedThread { modified, bytes, view: Arc::clone(&view) });
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
    pub(crate) events: super::event_log::EventLog,
    /// Latest title-generation request number per session.
    pub(crate) title_generations: Arc<std::sync::Mutex<HashMap<String, u64>>>,
    /// Serializes checkout-wide Ship mutations so two browser clients cannot race.
    pub(crate) ship_lock: Arc<tokio::sync::Mutex<()>>,
    /// Incremental session scanner shared across snapshot rebuilds.
    pub(crate) scan_cache: Arc<std::sync::Mutex<ScanCache>>,
    /// Parsed transcripts shared across snapshot rebuilds and thread views.
    pub(crate) thread_cache: Arc<std::sync::Mutex<ThreadCache>>,
    /// Live dialog requests per session awaiting an answer.
    pub(crate) pending_asks: Arc<std::sync::Mutex<PendingAsks>>,
    /// Per-launch capability required for credential-bearing Ship mutations.
    pub(crate) ship_token: Arc<str>,
    /// Optional single-session boundary for embedded clients.
    pub(crate) session_scope: Option<SessionScope>,
    /// Replay-safe results for retried mutations.
    pub(crate) idempotency: super::idempotency::Idempotency,
    /// Cached `pi` readiness probe.
    pub(crate) health: super::health::HealthCache,
    /// Device pairing and request authentication.
    pub(crate) auth: super::auth::Auth,
    /// Web push to devices that are not watching.
    pub(crate) push: super::push::Push,
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
        let scope = self.session_scope.clone();
        let scans = Arc::clone(&self.scan_cache);
        let threads = Arc::clone(&self.thread_cache);
        // Captures store data under a short-lived lock, released before the
        // (potentially slow) filesystem scan and transcript parsing below.
        let store_data = {
            let store = self.store.lock().map_err(|_poisoned| CoreError::LockPoisoned)?;
            StoreData::capture(&store, scope.as_ref())?
        };
        let built = tokio::task::spawn_blocking(move || {
            let mut scans = scans.lock().map_err(|_poisoned| CoreError::LockPoisoned)?;
            let mut threads = threads.lock().map_err(|_poisoned| CoreError::LockPoisoned)?;
            IndexSnapshot::build(&paths, &store_data, scope.as_ref(), &mut scans, &mut threads)
        })
        .await
        .map_err(|_join| CoreError::Join)??;
        *self.snapshot.write().await = Arc::new(built);
        self.events.send(ServerEvent::IndexChanged);
        Ok(())
    }

    /// Returns cached transcript paths that reference any of `run_ids`.
    ///
    /// Only already-parsed threads are considered: a thread nobody has opened
    /// has no client to refresh.
    pub(crate) fn threads_with_workflow_runs(
        &self,
        run_ids: &HashSet<String>,
    ) -> pecan_core::Result<Vec<std::path::PathBuf>> {
        let cache = self.thread_cache.lock().map_err(|_poisoned| CoreError::LockPoisoned)?;
        Ok(cache
            .iter()
            .filter(|(_, cached)| {
                cached.view.workflow_run_ids.iter().any(|id| run_ids.contains(id))
            })
            .map(|(path, _)| path.clone())
            .collect())
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
            let cache = self.thread_cache.lock().map_err(|_poisoned| CoreError::LockPoisoned)?;
            if let Some(cached) = cache.get(&path)
                && cached.modified == modified
                && cached.bytes == bytes
            {
                return Ok(Arc::clone(&cached.view));
            }
        }
        let parse_path = path.clone();
        let view = tokio::task::spawn_blocking(move || parse_thread(&parse_path))
            .await
            .map_err(|_join| CoreError::Join)??;
        let view = Arc::new(view);
        {
            let mut cache =
                self.thread_cache.lock().map_err(|_poisoned| CoreError::LockPoisoned)?;
            if cache.len() >= THREAD_CACHE_CAP {
                cache.clear();
            }
            cache.insert(path, CachedThread { modified, bytes, view: Arc::clone(&view) });
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
                agent_name: None,
            },
            settled: false,
            pinned: false,
            waiting_askuser: false,
            parent_session_id: None,
        }
    }

    fn parent_with_spawn(name: &str) -> SessionRow {
        let mut parent = row("main", "/project", "2026-01-01T00:00:00Z", SessionKind::Normal);
        let path = std::env::temp_dir().join(format!(
            "pecan-scope-parent-{}-{:?}-{}.jsonl",
            std::process::id(),
            std::thread::current().id(),
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
        let path = std::env::temp_dir().join(format!(
            "pecan-scope-child-{}-{:?}-{id}.jsonl",
            std::process::id(),
            std::thread::current().id()
        ));
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
        std::fs::remove_file(parent_path).unwrap_or_default();
        std::fs::remove_file(child_path).unwrap_or_default();
        std::fs::remove_file(unrelated_path).unwrap_or_default();
    }

    #[test]
    fn skips_settled_child_parent_resolution() {
        let parent = parent_with_spawn("settled-child");
        let parent_path = parent.summary.path.clone();
        let mut child = child_with_prompt(
            "settled-child-id",
            "settled-child",
            "Bound task",
            "2026-01-02T00:00:00Z",
        );
        child.settled = true;
        let child_path = child.summary.path.clone();
        let mut rows = vec![parent, child];

        super::link_parent_sessions(&mut rows);

        assert_eq!(rows[1].parent_session_id, None);
        std::fs::remove_file(parent_path).unwrap_or_default();
        std::fs::remove_file(child_path).unwrap_or_default();
    }

    #[test]
    fn links_child_to_matching_parent_session() {
        let parent = parent_with_spawn("owned-child");
        let parent_path = parent.summary.path.clone();
        let child = child_with_prompt("child", "owned-child", "Bound task", "2026-01-02T00:00:00Z");
        let child_path = child.summary.path.clone();
        let mut rows = vec![parent, child];

        super::link_parent_sessions(&mut rows);

        assert_eq!(rows[1].parent_session_id.as_deref(), Some("main"));
        std::fs::remove_file(parent_path).unwrap_or_default();
        std::fs::remove_file(child_path).unwrap_or_default();
    }

    #[test]
    fn links_workflow_children_by_run_id_only() {
        let mut parent = row("main", "/project", "2026-01-01T00:00:00Z", SessionKind::Normal);
        let parent_path = std::env::temp_dir().join(format!(
            "pecan-workflow-parent-{}-{:?}.jsonl",
            std::process::id(),
            std::thread::current().id()
        ));
        let result = serde_json::json!({
            "type": "message",
            "message": {
                "role": "toolResult",
                "toolName": "workflow",
                "details": { "runId": "wf-1" }
            }
        });
        std::fs::write(&parent_path, format!("{result}\n")).expect("write parent fixture");
        parent.summary.path = parent_path.clone();
        let mut owned = row("owned", "/project", "2026-01-02T00:00:00Z", SessionKind::Subagent);
        owned.summary.agent_name = Some("workflow:wf-1: Build".to_owned());
        let mut foreign = row("foreign", "/project", "2026-01-02T00:00:00Z", SessionKind::Subagent);
        foreign.summary.agent_name = Some("workflow:wf-2: Build".to_owned());
        let mut rows = vec![parent, owned, foreign];

        super::link_parent_sessions(&mut rows);

        assert_eq!(rows[1].parent_session_id.as_deref(), Some("main"));
        assert_eq!(rows[2].parent_session_id, None);
        std::fs::remove_file(parent_path).unwrap_or_default();
    }

    #[test]
    fn normalizes_owner_prefixed_agent_names() {
        assert_eq!(super::normalize_agent_name("subagents: scout"), "scout");
        assert_eq!(super::normalize_agent_name("workflow:wf-1: Build: api"), "Build: api");
        assert_eq!(super::normalize_agent_name("fix the bug: now"), "fix the bug: now");
        assert_eq!(super::normalize_agent_name("scout"), "scout");
    }

    #[test]
    fn missing_scope_main_fails_closed() {
        let mut rows =
            vec![row("child", "/project", "2026-01-02T00:00:00Z", SessionKind::Subagent)];

        SessionScope::new("missing".to_owned()).filter_rows(&mut rows);

        assert!(rows.is_empty());
    }
}
