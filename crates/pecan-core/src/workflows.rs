//! Workflow runs written by pi-subagents.
//!
//! Each run is an append-only event journal at
//! `<workflows>/runs/project-<sha256(cwd)>/<runId>/journal.json`. The journal
//! carries no owning session id, so callers pass the run ids a session's
//! `workflow*` tool results referenced and this module folds each journal
//! into a compact read model.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Mirrors the extension's own `MAX_WORKFLOW_ARTIFACT_BYTES` bound.
const MAX_JOURNAL_BYTES: u64 = 4 * 1024 * 1024;
const MAX_TEXT_CHARS: usize = 600;
const MAX_TASKS: usize = 64;
const MAX_RUN_ID_CHARS: usize = 128;

/// Folded view of one workflow run.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRun {
    /// Run id (`wf-...`); also the journal directory name.
    pub run_id: String,
    /// Author-chosen workflow name.
    pub name: Option<String>,
    /// Human-readable purpose of the workflow.
    pub description: Option<String>,
    /// `pending_approval`, `running`, `paused`, `completed`, `failed`, `cancelled`.
    pub status: String,
    /// Epoch-millisecond creation time.
    pub created_at: Option<i64>,
    /// Epoch-millisecond start time.
    pub started_at: Option<i64>,
    /// Epoch-millisecond terminal time.
    pub finished_at: Option<i64>,
    /// Epoch-millisecond time of the latest event.
    pub last_activity_at: Option<i64>,
    /// Completion summary, failure error, or cancellation reason.
    pub outcome: Option<String>,
    /// Declared tasks in definition order, with their folded state.
    pub tasks: Vec<WorkflowTask>,
}

/// One task of a workflow run.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowTask {
    /// Task id, unique within the run.
    pub id: String,
    /// Author-chosen label.
    pub label: String,
    /// `scout`, `writer`, `proof`, `review`, `repair`.
    pub kind: Option<String>,
    /// Task ids this task waits for.
    pub needs: Vec<String>,
    /// `pending`, `queued`, `running`, `completed`, `failed`, `cancelled`, `skipped`.
    pub status: String,
    /// Current attempt number; zero before the first admission.
    pub attempt: u32,
    /// Subagent id running the current attempt.
    pub child_id: Option<String>,
    /// Epoch-millisecond start of the current attempt.
    pub started_at: Option<i64>,
    /// Epoch-millisecond terminal time.
    pub finished_at: Option<i64>,
    /// Bounded result preview on completion.
    pub result: Option<String>,
    /// Failure error, cancellation or skip reason.
    pub error: Option<String>,
    /// Pi session id of the newest child transcript for this task; resolved
    /// by the server from the session index, never read from the journal.
    pub session_id: Option<String>,
}

/// Returns whether `id` is safe to use as a journal directory name.
#[must_use]
pub fn is_run_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_RUN_ID_CHARS
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Returns the validated `details.runId` of a `workflow*` tool-result message,
/// the only persisted link from a session transcript to its workflow runs.
#[must_use]
pub fn tool_result_run_id(message: &serde_json::Value) -> Option<&str> {
    let tool = message.get("toolName").and_then(serde_json::Value::as_str)?;
    if tool != "workflow" && !tool.starts_with("workflow_") {
        return None;
    }
    let run_id = message.pointer("/details/runId").and_then(serde_json::Value::as_str)?;
    is_run_id(run_id).then_some(run_id)
}

/// Splits a workflow child session name (`workflow:<runId>: <task label>`)
/// into its run id and task label.
#[must_use]
pub fn child_session_task(agent_name: &str) -> Option<(&str, &str)> {
    let (run_id, label) = agent_name.strip_prefix("workflow:")?.split_once(": ")?;
    let label = label.trim();
    (is_run_id(run_id) && !label.is_empty()).then_some((run_id, label))
}

/// Loads the runs among `run_ids` that have a readable journal for `cwd`,
/// newest first. Unknown or malformed journals are skipped.
pub fn load_runs(workflows_dir: &Path, cwd: &str, run_ids: &[String]) -> Vec<WorkflowRun> {
    let mut runs: Vec<WorkflowRun> = run_ids
        .iter()
        .filter(|id| is_run_id(id))
        .filter_map(|id| read_journal(&journal_path(workflows_dir, cwd, id), id))
        .collect();
    runs.sort_by(|left, right| {
        right.created_at.cmp(&left.created_at).then_with(|| left.run_id.cmp(&right.run_id))
    });
    runs
}

/// Journal location pi-subagents uses for `run_id` launched from `cwd`.
#[must_use]
pub fn journal_path(workflows_dir: &Path, cwd: &str, run_id: &str) -> PathBuf {
    project_dir(workflows_dir, cwd).join(run_id).join("journal.json")
}

fn project_dir(workflows_dir: &Path, cwd: &str) -> PathBuf {
    let digest = Sha256::digest(cwd.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    workflows_dir.join("runs").join(format!("project-{hex}"))
}

fn read_journal(file: &Path, run_id: &str) -> Option<WorkflowRun> {
    if std::fs::metadata(file).ok()?.len() > MAX_JOURNAL_BYTES {
        return None;
    }
    let bytes = std::fs::read(file).ok()?;
    let events: Vec<serde_json::Value> = serde_json::from_slice(&bytes).ok()?;
    fold(run_id, &events)
}

/// Folds journal events into a [`WorkflowRun`]; `None` without a creation event.
fn fold(run_id: &str, events: &[serde_json::Value]) -> Option<WorkflowRun> {
    let mut run: Option<WorkflowRun> = None;
    let mut index: HashMap<String, usize> = HashMap::new();
    for event in events {
        if str_of(event, "runId").as_deref() != Some(run_id) {
            continue;
        }
        let at = event.get("at").and_then(serde_json::Value::as_i64);
        let tag = str_of(event, "_tag").unwrap_or_default();
        if tag == "WorkflowCreated" {
            let definition = event.get("definition");
            let tasks: Vec<WorkflowTask> = definition
                .and_then(|d| d.get("tasks"))
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(declared_task)
                .take(MAX_TASKS)
                .collect();
            index = tasks.iter().enumerate().map(|(i, task)| (task.id.clone(), i)).collect();
            run = Some(WorkflowRun {
                run_id: run_id.to_owned(),
                name: definition.and_then(|d| str_of(d, "name")),
                description: definition.and_then(|d| str_of(d, "description")),
                status: "pending_approval".to_owned(),
                created_at: at,
                started_at: None,
                finished_at: None,
                last_activity_at: at,
                outcome: None,
                tasks,
            });
            continue;
        }
        let Some(run) = run.as_mut() else { continue };
        run.last_activity_at = at.or(run.last_activity_at);
        match tag.as_str() {
            "WorkflowStarted" => {
                run.status = "running".to_owned();
                run.started_at = at;
            }
            "WorkflowPaused" => "paused".clone_into(&mut run.status),
            "WorkflowResumed" => "running".clone_into(&mut run.status),
            "WorkflowCompleted" | "WorkflowFailed" | "WorkflowCancelled" => {
                let (status, key) = match tag.as_str() {
                    "WorkflowCompleted" => ("completed", "summary"),
                    "WorkflowFailed" => ("failed", "error"),
                    _ => ("cancelled", "reason"),
                };
                status.clone_into(&mut run.status);
                run.finished_at = at;
                run.outcome = bounded(event, key);
            }
            _ => {
                let Some(task) = str_of(event, "taskId")
                    .and_then(|id| index.get(&id).copied())
                    .and_then(|i| run.tasks.get_mut(i))
                else {
                    continue;
                };
                apply_task_event(task, &tag, event, at);
            }
        }
    }
    run
}

fn apply_task_event(
    task: &mut WorkflowTask,
    tag: &str,
    event: &serde_json::Value,
    at: Option<i64>,
) {
    match tag {
        "TaskQueued" => {
            task.status = "queued".to_owned();
            task.attempt = task.attempt.saturating_add(1);
            task.child_id = str_of(event, "childId");
            task.started_at = None;
            task.finished_at = None;
            task.result = None;
            task.error = None;
        }
        "TaskStarted" | "TaskEvaluationStarted" => {
            task.status = "running".to_owned();
            task.started_at = at;
        }
        "TaskRetryRequested" => "pending".clone_into(&mut task.status),
        "TaskCompleted" => {
            task.status = "completed".to_owned();
            task.finished_at = at;
            task.result = bounded(event, "resultPreview");
        }
        "TaskFailed" | "TaskCancelled" | "TaskSkipped" => {
            let (status, key) = match tag {
                "TaskFailed" => ("failed", "error"),
                "TaskCancelled" => ("cancelled", "reason"),
                _ => ("skipped", "reason"),
            };
            status.clone_into(&mut task.status);
            task.finished_at = at;
            task.error = bounded(event, key);
        }
        _ => {}
    }
}

fn declared_task(value: &serde_json::Value) -> Option<WorkflowTask> {
    let id = str_of(value, "id")?;
    Some(WorkflowTask {
        label: str_of(value, "label").unwrap_or_else(|| id.clone()),
        id,
        kind: str_of(value, "kind"),
        needs: value
            .get("needs")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .take(MAX_TASKS)
            .collect(),
        status: "pending".to_owned(),
        attempt: 0,
        child_id: None,
        started_at: None,
        finished_at: None,
        result: None,
        error: None,
        session_id: None,
    })
}

fn str_of(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key).and_then(serde_json::Value::as_str).map(str::to_owned)
}

fn bounded(value: &serde_json::Value, key: &str) -> Option<String> {
    let text = value.get(key).and_then(serde_json::Value::as_str)?.trim();
    if text.is_empty() {
        return None;
    }
    let mut out: String = text.chars().take(MAX_TEXT_CHARS).collect();
    if text.chars().nth(MAX_TEXT_CHARS).is_some() {
        out.push('\u{2026}');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CWD: &str = "/work/demo";

    fn write_journal(root: &Path, run_id: &str, events: &str) {
        let dir = project_dir(root, CWD).join(run_id);
        std::fs::create_dir_all(&dir).expect("mkdir run");
        std::fs::write(dir.join("journal.json"), events).expect("write journal");
    }

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("pecan-wf-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&root).unwrap_or_default();
        root
    }

    #[test]
    fn project_dir_matches_extension_hash() {
        // sha256("/work/demo"), as `createHash("sha256")` in the extension.
        let dir = project_dir(Path::new("/w"), CWD);
        let name = dir.file_name().and_then(|n| n.to_str()).expect("name");
        assert!(name.starts_with("project-"));
        assert_eq!(name.len(), "project-".len() + 64);
    }

    #[test]
    fn folds_graph_with_retry_and_failure() {
        let root = temp_root("fold");
        write_journal(
            &root,
            "wf-1",
            r#"[
              {"_tag":"WorkflowCreated","runId":"wf-1","at":10,"definition":{"name":"ship","description":"do it","tasks":[
                {"id":"a","label":"Build","kind":"writer","prompt":"p"},
                {"id":"b","label":"Review","kind":"review","prompt":"p","needs":["a"]},
                {"id":"c","label":"Docs","kind":"writer","prompt":"p","needs":["b"]}]}},
              {"_tag":"WorkflowStarted","runId":"wf-1","at":11},
              {"_tag":"TaskQueued","runId":"wf-1","at":12,"taskId":"a","childId":"sa-1"},
              {"_tag":"TaskStarted","runId":"wf-1","at":13,"taskId":"a"},
              {"_tag":"TaskFailed","runId":"wf-1","at":14,"taskId":"a","error":"boom"},
              {"_tag":"TaskRetryRequested","runId":"wf-1","at":15,"taskId":"a"},
              {"_tag":"TaskQueued","runId":"wf-1","at":16,"taskId":"a","childId":"sa-2"},
              {"_tag":"TaskStarted","runId":"wf-1","at":17,"taskId":"a"},
              {"_tag":"TaskCompleted","runId":"wf-1","at":18,"taskId":"a","resultPreview":"built"},
              {"_tag":"TaskQueued","runId":"wf-1","at":19,"taskId":"b","childId":"sa-3"},
              {"_tag":"TaskStarted","runId":"wf-1","at":20,"taskId":"b"},
              {"_tag":"TaskFailed","runId":"wf-1","at":21,"taskId":"b","error":"rejected"},
              {"_tag":"TaskSkipped","runId":"wf-1","at":22,"taskId":"c","reason":"dependency failed"},
              {"_tag":"TaskCompleted","runId":"other","at":23,"taskId":"b"},
              {"_tag":"WorkflowFailed","runId":"wf-1","at":24,"error":"task b failed"}
            ]"#,
        );
        let runs = load_runs(&root, CWD, &["wf-1".to_owned()]);
        let run = runs.first().expect("run");
        assert_eq!(run.name.as_deref(), Some("ship"));
        assert_eq!(run.status, "failed");
        assert_eq!(run.outcome.as_deref(), Some("task b failed"));
        assert_eq!(
            (run.started_at, run.finished_at, run.last_activity_at),
            (Some(11), Some(24), Some(24))
        );
        assert_eq!(run.tasks.len(), 3);
        let (a, b, c) = (&run.tasks[0], &run.tasks[1], &run.tasks[2]);
        assert_eq!(
            (a.status.as_str(), a.attempt, a.child_id.as_deref()),
            ("completed", 2, Some("sa-2"))
        );
        assert_eq!((a.result.as_deref(), a.error.as_deref()), (Some("built"), None));
        assert_eq!((b.status.as_str(), b.error.as_deref()), ("failed", Some("rejected")));
        assert_eq!(b.needs, vec!["a".to_owned()]);
        assert_eq!((c.status.as_str(), c.attempt), ("skipped", 0));
        std::fs::remove_dir_all(&root).unwrap_or_default();
    }

    #[test]
    fn pause_and_pending_approval_states() {
        let root = temp_root("pause");
        write_journal(
            &root,
            "wf-p",
            r#"[{"_tag":"WorkflowCreated","runId":"wf-p","at":1,"definition":{"tasks":[{"id":"a","label":"A","kind":"scout","prompt":"p"}]}},
                {"_tag":"WorkflowStarted","runId":"wf-p","at":2},
                {"_tag":"WorkflowPaused","runId":"wf-p","at":3,"reason":"user"}]"#,
        );
        write_journal(
            &root,
            "wf-q",
            r#"[{"_tag":"WorkflowCreated","runId":"wf-q","at":5,"definition":{"tasks":[]}}]"#,
        );
        let runs = load_runs(&root, CWD, &["wf-p".to_owned(), "wf-q".to_owned()]);
        let statuses: Vec<_> =
            runs.iter().map(|r| (r.run_id.as_str(), r.status.as_str())).collect();
        assert_eq!(statuses, vec![("wf-q", "pending_approval"), ("wf-p", "paused")]);
        std::fs::remove_dir_all(&root).unwrap_or_default();
    }

    #[test]
    fn links_tool_results_and_child_names_to_runs() {
        let result =
            serde_json::json!({"toolName": "workflow_control", "details": {"runId": "wf-1"}});
        assert_eq!(tool_result_run_id(&result), Some("wf-1"));
        let other = serde_json::json!({"toolName": "subagent_spawn", "details": {"runId": "wf-1"}});
        assert_eq!(tool_result_run_id(&other), None);
        let unsafe_id = serde_json::json!({"toolName": "workflow", "details": {"runId": "../x"}});
        assert_eq!(tool_result_run_id(&unsafe_id), None);

        assert_eq!(child_session_task("workflow:wf-1: Build: api"), Some(("wf-1", "Build: api")));
        assert_eq!(child_session_task("subagents: scout"), None);
        assert_eq!(child_session_task("workflow:../x: Build"), None);
        assert_eq!(child_session_task("workflow:wf-1:  "), None);
    }

    #[test]
    fn rejects_unsafe_ids_malformed_and_foreign_journals() {
        let root = temp_root("bad");
        write_journal(&root, "wf-bad", "{not json");
        write_journal(&root, "wf-empty", "[]");
        let ids = ["../escape", "wf-bad", "wf-empty", "wf-missing", ""].map(str::to_owned);
        assert!(load_runs(&root, CWD, &ids).is_empty());
        // A journal for another project is not visible from this cwd.
        write_journal(
            &root,
            "wf-x",
            r#"[{"_tag":"WorkflowCreated","runId":"wf-x","at":1,"definition":{"tasks":[]}}]"#,
        );
        assert!(load_runs(&root, "/elsewhere", &["wf-x".to_owned()]).is_empty());
        assert!(!is_run_id("a/b"));
        assert!(is_run_id("wf-60370e06-6733-49a4-83ed-07ed01ae3a0e"));
        std::fs::remove_dir_all(&root).unwrap_or_default();
    }
}
