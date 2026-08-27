//! Workflow run metadata written by pi-workflows (`~/.pi/agent/workflows/wf_*`).

use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;

const MAX_WORKFLOW_FILE_BYTES: u64 = 1024 * 1024;

/// Lightweight view of one workflow run.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRun {
    /// Run id (`wf_...`); also the artifact directory name.
    #[serde(rename = "runId")]
    pub run_id: String,
    /// Owning pi session id, when recorded.
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
    /// Author-chosen workflow name.
    pub name: Option<String>,
    /// Human-readable purpose of the workflow.
    pub description: Option<String>,
    /// Whether the run was launched as a background workflow.
    pub background: bool,
    /// Epoch-millisecond start time.
    pub started_at: Option<i64>,
    /// Epoch-millisecond completion time.
    pub finished_at: Option<i64>,
    /// `running`, `completed`, `failed`, `cancelled`, ...
    pub status: Option<String>,
    /// Currently executing phase, when the run is active.
    pub current_phase: Option<String>,
    /// Declared workflow phases.
    pub phases: Vec<WorkflowPhase>,
    /// Terminal error, when one was recorded.
    pub error: Option<String>,
    /// Name of the persisted result sidecar.
    pub result_artifact: Option<String>,
    /// Name of the persisted transcript sidecar.
    pub transcript_artifact: Option<String>,
    /// Agent labels participating in the run.
    pub agents: Vec<WorkflowAgentSummary>,
}

/// One declared phase in a workflow run.
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowPhase {
    /// Phase title.
    pub title: String,
    /// Optional phase description.
    pub detail: Option<String>,
}

/// Minimal per-agent info surfaced in lists.
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowAgentSummary {
    /// Author-chosen agent label.
    pub label: Option<String>,
    /// Phase the agent belonged to.
    pub phase: Option<String>,
    /// `queued`, `running`, `done`, `error`, ...
    pub state: Option<String>,
    /// Model id used by the agent.
    pub model: Option<String>,
    /// Provider id used by the agent.
    pub provider: Option<String>,
    /// Number of completed tool operations.
    pub completed_operations: Option<u64>,
}

/// Loads all parseable workflow runs under `workflows_dir`.
///
/// Only each run's `workflow.json` is read; result/transcript artifacts are
/// never touched at index time.
pub fn load_all(workflows_dir: &Path) -> HashMap<String, Vec<WorkflowRun>> {
    let mut out: HashMap<String, Vec<WorkflowRun>> = HashMap::new();
    let Ok(entries) = std::fs::read_dir(workflows_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let dir_path = entry.path();
        if !dir_path.is_dir() {
            continue;
        }
        let Some(meta) = read_run(&dir_path) else { continue };
        if let Some(session_id) = meta.session_id.clone() {
            out.entry(session_id).or_default().push(meta);
        }
    }
    for runs in out.values_mut() {
        runs.sort_by(|left, right| {
            right.started_at.cmp(&left.started_at).then_with(|| left.run_id.cmp(&right.run_id))
        });
    }
    out
}

/// Reads one run directory's `workflow.json`.
fn read_run(run_dir: &Path) -> Option<WorkflowRun> {
    let file = run_dir.join("workflow.json");
    if std::fs::metadata(&file).ok()?.len() > MAX_WORKFLOW_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(&file).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let run_id = value
        .get("runId")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .or_else(|| run_dir.file_name().and_then(|n| n.to_str()).map(str::to_owned))?;
    let agents = value
        .get("agents")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|a| WorkflowAgentSummary {
                    label: a.get("label").and_then(serde_json::Value::as_str).map(str::to_owned),
                    phase: a.get("phase").and_then(serde_json::Value::as_str).map(str::to_owned),
                    state: a.get("state").and_then(serde_json::Value::as_str).map(str::to_owned),
                    model: a.get("model").and_then(serde_json::Value::as_str).map(str::to_owned),
                    provider: a
                        .get("provider")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                    completed_operations: a
                        .get("completedOperations")
                        .and_then(serde_json::Value::as_u64),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(WorkflowRun {
        run_id,
        session_id: value.get("sessionId").and_then(serde_json::Value::as_str).map(str::to_owned),
        name: value.get("name").and_then(serde_json::Value::as_str).map(str::to_owned),
        description: value
            .get("description")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        background: value.get("background").and_then(serde_json::Value::as_bool).unwrap_or(false),
        started_at: epoch_ms(&value, "startedAt"),
        finished_at: epoch_ms(&value, "finishedAt"),
        status: value.get("status").and_then(serde_json::Value::as_str).map(str::to_owned),
        current_phase: value
            .get("currentPhase")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        phases: parse_phases(&value),
        error: value.get("error").and_then(serde_json::Value::as_str).map(str::to_owned),
        result_artifact: value
            .get("resultArtifact")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        transcript_artifact: value
            .get("transcriptArtifact")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        agents,
    })
}

fn epoch_ms(value: &serde_json::Value, key: &str) -> Option<i64> {
    value.get(key).and_then(serde_json::Value::as_i64).filter(|value| *value >= 0)
}

fn parse_phases(value: &serde_json::Value) -> Vec<WorkflowPhase> {
    value
        .get("phases")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|phase| {
            Some(WorkflowPhase {
                title: phase.get("title").and_then(serde_json::Value::as_str)?.to_owned(),
                detail: phase.get("detail").and_then(serde_json::Value::as_str).map(str::to_owned),
            })
        })
        .take(64)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_runs_by_session() {
        let root = std::env::temp_dir().join(format!("pecan-wf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let run_dir = root.join("wf_test123");
        std::fs::create_dir_all(&run_dir).expect("mkdir run");
        std::fs::write(
            run_dir.join("workflow.json"),
            r#"{"runId":"wf_test123","sessionId":"sess-9","name":"review","status":"completed","phases":[{"title":"P1","detail":"inspect"}],"resultArtifact":"result.json","transcriptArtifact":"transcripts.json",
                "agents":[{"label":"a1","phase":"P1","state":"done","model":"m1","provider":"p1","completedOperations":3}]}"#,
        )
        .expect("write wf");

        let all = load_all(&root);
        let runs = all.get("sess-9").expect("grouped by session");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_id, "wf_test123");
        assert_eq!(runs[0].agents.len(), 1);
        assert_eq!(runs[0].status.as_deref(), Some("completed"));
        assert_eq!(runs[0].agents[0].model.as_deref(), Some("m1"));
        assert_eq!(runs[0].phases[0].title, "P1");
        assert_eq!(runs[0].result_artifact.as_deref(), Some("result.json"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn skips_dirs_without_metadata() {
        let root = std::env::temp_dir().join(format!("pecan-wf-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let run_dir = root.join("wf_nope");
        std::fs::create_dir_all(&run_dir).expect("mkdir run");
        assert!(load_all(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
