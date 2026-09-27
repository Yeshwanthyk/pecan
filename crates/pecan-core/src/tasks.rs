//! Bounded read-only projections of stores written by `pi-tasks`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

const MAX_TASK_STORE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_TASKS_PER_STORE: usize = 500;
const MAX_SUBJECT_BYTES: usize = 512;
const MAX_DESCRIPTION_BYTES: usize = 8 * 1024;
const MAX_RELATION_IDS: usize = 64;

/// One task in a `pi-tasks` list.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskItem {
    /// Task id within its list (`"1"`, `"2"`, ...).
    pub id: String,
    /// Imperative summary of the work.
    pub subject: String,
    /// `pending`, `in_progress`, or `completed`.
    pub status: String,
    /// Longer task context, when persisted by the producer.
    pub description: Option<String>,
    /// Present-tense label used while the task is active.
    pub active_form: Option<String>,
    /// Current owner label.
    pub owner: Option<String>,
    /// Selected execution harness.
    pub harness: Option<String>,
    /// Incomplete prerequisite ids.
    pub blocked_by: Vec<String>,
    /// Dependent task ids.
    pub blocks: Vec<String>,
    /// Current background execution state, when one exists.
    pub execution: Option<serde_json::Value>,
    /// Last producer update time in epoch milliseconds.
    pub updated_at: Option<i64>,
}

/// A task list projected into one session.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskList {
    /// Session receiving this projection.
    #[serde(rename = "sessionId")]
    pub session_id: String,
    /// Tasks in list order.
    pub tasks: Vec<TaskItem>,
}

/// Loads legacy global `tasks-*.json` files, grouped by session id.
#[must_use]
pub fn load_all(tasks_dir: &Path) -> HashMap<String, Vec<TaskList>> {
    let mut out: HashMap<String, Vec<TaskList>> = HashMap::new();
    let Ok(entries) = std::fs::read_dir(tasks_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path: PathBuf = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        if !path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("tasks-")) {
            continue;
        }
        if let Some(list) = read_list(&path) {
            out.entry(list.session_id.clone()).or_default().push(list);
        }
    }
    out
}

/// Loads the producer store resolved for every visible `(session id, cwd)`.
///
/// The default is a project-local session file. Project, named, and explicit
/// stores are shared boards and are projected into each visible session in the
/// owning working directory. Memory mode has no durable state to inspect.
#[must_use]
pub fn load_for_sessions<'a>(
    sessions: impl Iterator<Item = (&'a str, &'a str)>,
    global_tasks_dir: &Path,
) -> HashMap<String, Vec<TaskList>> {
    let override_value = std::env::var("PI_TASKS").ok();
    let mut out = HashMap::new();
    for (session_id, cwd) in sessions {
        let cwd = Path::new(cwd);
        let Some((path, shared)) =
            resolve_store(session_id, cwd, global_tasks_dir, override_value.as_deref())
        else {
            continue;
        };
        if let Some(list) = read_list_for_session(&path, session_id, shared) {
            out.entry(session_id.to_owned()).or_insert_with(Vec::new).push(list);
        }
    }
    out
}

fn resolve_store(
    session_id: &str,
    cwd: &Path,
    global_tasks_dir: &Path,
    override_value: Option<&str>,
) -> Option<(PathBuf, bool)> {
    if let Some(value) = override_value {
        if value == "off" {
            return None;
        }
        if value.starts_with('/') {
            return Some((PathBuf::from(value), true));
        }
        if value.starts_with('.') {
            return Some((cwd.join(value), true));
        }
        return Some((global_tasks_dir.join(format!("{value}.json")), true));
    }
    match task_scope(cwd) {
        "memory" => None,
        "project" => Some((cwd.join(".pi/tasks/tasks.json"), true)),
        _ => Some((cwd.join(".pi/tasks").join(format!("tasks-{session_id}.json")), false)),
    }
}

fn task_scope(cwd: &Path) -> &'static str {
    let path = cwd.join(".pi/tasks-config.json");
    let Ok(metadata) = std::fs::metadata(&path) else {
        return "session";
    };
    if metadata.len() > 64 * 1024 {
        return "session";
    }
    let Ok(bytes) = std::fs::read(path) else {
        return "session";
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return "session";
    };
    match value.get("taskScope").and_then(serde_json::Value::as_str) {
        Some("memory") => "memory",
        Some("project") => "project",
        _ => "session",
    }
}

fn read_list(path: &Path) -> Option<TaskList> {
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > MAX_TASK_STORE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let items = value.get("tasks").and_then(serde_json::Value::as_array)?;
    let session_id = items
        .iter()
        .find_map(|task| task.get("sessionId").and_then(serde_json::Value::as_str))
        .map(str::to_owned)
        .or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| stem.strip_prefix("tasks-"))
                .map(str::to_owned)
        })?;
    let tasks = items.iter().take(MAX_TASKS_PER_STORE).filter_map(parse_task).collect();
    Some(TaskList { session_id, tasks })
}

fn parse_task(task: &serde_json::Value) -> Option<TaskItem> {
    let status = task.get("status").and_then(serde_json::Value::as_str).unwrap_or("pending");
    if !matches!(status, "pending" | "in_progress" | "completed") {
        return None;
    }
    Some(TaskItem {
        id: bounded_string(task.get("id"), 80)?,
        subject: bounded_string(task.get("subject"), MAX_SUBJECT_BYTES)?,
        status: status.to_owned(),
        description: bounded_string(task.get("description"), MAX_DESCRIPTION_BYTES),
        active_form: bounded_string(task.get("activeForm"), MAX_SUBJECT_BYTES),
        owner: bounded_string(task.get("owner"), 160),
        harness: task
            .get("harness")
            .and_then(serde_json::Value::as_str)
            .filter(|value| matches!(*value, "pi" | "claude" | "codex"))
            .map(str::to_owned),
        blocked_by: bounded_ids(task.get("blockedBy")),
        blocks: bounded_ids(task.get("blocks")),
        execution: bounded_value(task.get("execution"), 64 * 1024),
        updated_at: task
            .get("updatedAt")
            .and_then(serde_json::Value::as_i64)
            .filter(|value| *value >= 0),
    })
}

fn read_list_for_session(path: &Path, session_id: &str, shared: bool) -> Option<TaskList> {
    let mut list = read_list(path)?;
    if !shared && list.session_id != session_id {
        return None;
    }
    list.session_id = session_id.to_owned();
    Some(list)
}

fn bounded_string(value: Option<&serde_json::Value>, max_bytes: usize) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= max_bytes)
        .map(str::to_owned)
}

fn bounded_ids(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_RELATION_IDS)
        .filter_map(|id| bounded_string(Some(id), 80))
        .collect()
}

fn bounded_value(value: Option<&serde_json::Value>, max_bytes: usize) -> Option<serde_json::Value> {
    let value = value?;
    let encoded = serde_json::to_vec(value).ok()?;
    (encoded.len() <= max_bytes).then(|| value.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_legacy_lists_by_session() {
        let root = std::env::temp_dir().join(format!("pecan-tasks-{}", std::process::id()));
        std::fs::remove_dir_all(&root).unwrap_or_default();
        std::fs::create_dir_all(&root).expect("mkdir tasks");
        std::fs::write(
            root.join("tasks-sess7.json"),
            r#"{"nextId":2,"tasks":[
                {"id":"1","subject":"first","status":"completed","sessionId":"sess7","blockedBy":[]},
                {"id":"2","subject":"second","status":"pending","sessionId":"sess7","owner":"agent-1","blockedBy":["1"]}]}"#,
        )
        .expect("write tasks");

        let all = load_all(&root);
        let lists = all.get("sess7").expect("grouped");
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].tasks.len(), 2);
        assert_eq!(lists[0].tasks[0].status, "completed");
        assert_eq!(lists[0].tasks[1].owner.as_deref(), Some("agent-1"));
        assert_eq!(lists[0].tasks[1].blocked_by, ["1"]);
        std::fs::remove_dir_all(root).unwrap_or_default();
    }

    #[test]
    fn resolves_default_project_local_session_store() {
        let root = std::env::temp_dir().join(format!("pecan-tasks-project-{}", std::process::id()));
        let tasks = root.join(".pi/tasks");
        std::fs::remove_dir_all(&root).unwrap_or_default();
        std::fs::create_dir_all(&tasks).expect("mkdir tasks");
        std::fs::write(
            tasks.join("tasks-sess-local.json"),
            r#"{"nextId":2,"tasks":[{"id":"1","subject":"local","description":"detail","status":"in_progress","sessionId":"sess-local","blockedBy":[]}]}"#,
        )
        .expect("write tasks");
        let loaded = load_for_sessions(
            std::iter::once(("sess-local", root.to_str().expect("utf8"))),
            &root.join("global"),
        );
        assert_eq!(loaded.get("sess-local").map(Vec::len), Some(1));
        std::fs::remove_dir_all(root).unwrap_or_default();
    }

    #[test]
    fn ignores_non_task_files() {
        let root = std::env::temp_dir().join(format!("pecan-tasks-bad-{}", std::process::id()));
        std::fs::remove_dir_all(&root).unwrap_or_default();
        std::fs::create_dir_all(&root).expect("mkdir tasks");
        std::fs::write(root.join("other.json"), "{}").expect("write other");
        assert!(load_all(&root).is_empty());
        std::fs::remove_dir_all(root).unwrap_or_default();
    }
}
