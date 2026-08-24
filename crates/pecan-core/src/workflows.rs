//! Workflow run metadata written by pi-workflows (`~/.pi/agent/workflows/wf_*`).

use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;

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
    /// `running`, `completed`, `failed`, `cancelled`, ...
    pub status: Option<String>,
    /// Agent labels participating in the run.
    pub agents: Vec<WorkflowAgentSummary>,
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
    out
}

/// Reads one run directory's `workflow.json`.
fn read_run(run_dir: &Path) -> Option<WorkflowRun> {
    let file = run_dir.join("workflow.json");
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
                })
                .collect()
        })
        .unwrap_or_default();
    Some(WorkflowRun {
        run_id,
        session_id: value.get("sessionId").and_then(serde_json::Value::as_str).map(str::to_owned),
        name: value.get("name").and_then(serde_json::Value::as_str).map(str::to_owned),
        status: value.get("status").and_then(serde_json::Value::as_str).map(str::to_owned),
        agents,
    })
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
            r#"{"runId":"wf_test123","sessionId":"sess-9","name":"review","status":"completed",
                "agents":[{"label":"a1","phase":"P1","state":"done"}]}"#,
        )
        .expect("write wf");

        let all = load_all(&root);
        let runs = all.get("sess-9").expect("grouped by session");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_id, "wf_test123");
        assert_eq!(runs[0].agents.len(), 1);
        assert_eq!(runs[0].status.as_deref(), Some("completed"));
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
