//! HTTP API: JSON handlers and the SSE stream.

use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::HeaderMap;
use axum::response::Sse;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::Stream;
use pecan_core::session::SessionKind;
use pecan_core::thread;
use serde::Deserialize;
use std::time::Duration;

use super::auth::{self, Principal};
use super::folders;
use super::git;
use super::push;
use super::ship;
use super::snapshot::{App, IndexSnapshot, ServerEvent, SessionRow};
use super::title;
use super::worker::Workers;
use crate::server::api_errors::{ApiError, json_body};

/// Builds the `/api` router.
pub(crate) fn router(app: App, workers: Workers) -> Router {
    let with_workers = Router::new()
        // Image attachments ride in the message JSON body.
        .layer(axum::extract::DefaultBodyLimit::max(48 * 1024 * 1024))
        .route("/session/{id}", get(thread_view))
        .route("/session/new", post(new_session))
        .route("/session/{id}/message", post(message))
        .route("/session/{id}/respond", post(respond))
        .route("/session/{id}/abort", post(abort))
        .route("/session/{id}/status", get(session_status))
        .route("/session/{id}/agent-attach", post(agent_attach))
        .route("/session/{id}/set-model", post(set_model))
        .route("/session/{id}/set-thinking", post(set_thinking))
        .with_state((app.clone(), workers));
    let base = Router::new()
        .route("/bootstrap", get(bootstrap))
        .route("/health", get(health))
        .route("/sessions", get(sessions))
        .route("/session/{id}/git", get(session_git))
        .route("/session/{id}/diff", get(session_diff))
        .route("/session/{id}/ship", get(ship_plan).post(ship_execute))
        .route("/session/{id}/settle", post(settle).delete(reopen))
        .route("/session/{id}/pin", post(pin).delete(unpin))
        .route("/session/{id}/asks", get(pending_asks))
        .route("/session/{id}/title/regenerate", post(regenerate_title))
        .route("/projects", post(add_project).delete(remove_project))
        .route("/folders", get(folders))
        .route("/events", get(events))
        .route("/pair/code", post(auth::mint_code))
        .route("/devices", get(auth::list_devices))
        .route("/devices/{id}", axum::routing::delete(auth::revoke_device))
        .route("/push/key", get(push::key))
        .route("/push/subscription", axum::routing::put(push::subscribe).delete(push::unsubscribe))
        .route("/push/subscriptions", get(push::subscriptions))
        .route("/push/pending", get(push::pending))
        .route("/push/test", post(push::test))
        .with_state(app.clone())
        .merge(with_workers)
        .route_layer(axum::middleware::from_fn_with_state(app.clone(), auth::require));
    Router::new()
        .route("/pair", post(auth::pair))
        .with_state(app)
        .merge(base)
        .fallback(api_not_found)
}

async fn api_not_found() -> ApiError {
    ApiError::not_found("no such API route")
}

// ------------------------------------------------------------- bootstrap ----

async fn bootstrap(State(app): State<App>) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = app.snapshot().await;
    let store = lock(&app)?;
    let seeded = store.is_seeded()?;
    let project_rows = if app.session_scope.is_some() {
        Vec::new()
    } else {
        store.projects()?.into_iter().filter(|(_, pref)| pref.added).collect::<Vec<_>>()
    };
    let projects: Vec<serde_json::Value> = project_rows
        .iter()
        .map(|(cwd, _)| {
            serde_json::json!({
                "cwd": cwd,
                "name": display_name(cwd),
                "sessionCount": session_count(&snap, cwd),
                "lastActivity": last_activity(&snap, cwd),
            })
        })
        .collect();
    let sessions: Vec<&SessionRow> = if app.session_scope.is_some() {
        snap.sessions.iter().collect()
    } else {
        let added_cwds: std::collections::HashSet<&str> =
            project_rows.iter().map(|(cwd, _)| cwd.as_str()).collect();
        let mut rows: Vec<&SessionRow> = snap
            .sessions
            .iter()
            .filter(|row| added_cwds.contains(row.summary.cwd.as_str()))
            .collect();
        rows.sort_by_key(|row| list_order(row));
        rows.truncate(MAX_PAGE);
        rows
    };
    Ok(Json(serde_json::json!({
        "seeded": seeded,
        "sessionScope": app.session_scope,
        "projects": projects,
        "sessions": sessions,
    })))
}

/// Page order for list views: threads before subagents (only reached via
/// their parent), active before Done, then most recently active first, so the
/// page cap never hides a thread behind old child runs.
fn list_order(row: &SessionRow) -> (bool, bool, std::cmp::Reverse<jiff::Timestamp>) {
    (
        row.summary.kind == SessionKind::Subagent,
        row.settled,
        std::cmp::Reverse(row.summary.last_activity),
    )
}

/// User-opened sessions in a project; subagents are not threads of their own.
fn session_count(snap: &IndexSnapshot, cwd: &str) -> usize {
    snap.sessions
        .iter()
        .filter(|s| s.summary.cwd == cwd && s.summary.kind == SessionKind::Normal)
        .count()
}

fn last_activity(snap: &IndexSnapshot, cwd: &str) -> Option<String> {
    snap.sessions
        .iter()
        .filter(|s| s.summary.cwd == cwd)
        .map(|s| s.summary.last_activity)
        .max()
        .map(|ts| ts.to_string())
}

fn display_name(cwd: &str) -> String {
    cwd.rsplit('/').next().unwrap_or(cwd).to_owned()
}

#[derive(Deserialize, Debug, Default)]
struct HealthQuery {
    /// Skip the cache and probe `pi` again.
    #[serde(default)]
    refresh: bool,
}

/// Whether `pi` is runnable, so clients can explain a broken setup up front.
async fn health(
    State(app): State<App>,
    axum::extract::Query(query): axum::extract::Query<HealthQuery>,
) -> Json<super::health::Health> {
    Json(app.health.get(app.paths.agent_dir(), query.refresh).await)
}

// -------------------------------------------------------------- sessions ----

#[derive(Deserialize, Debug, Default)]
struct SessionsQuery {
    project: Option<String>,
    /// Only sessions of this kind, e.g. a thread's subagent children.
    kind: Option<SessionKind>,
    offset: Option<usize>,
    limit: Option<usize>,
}

const DEFAULT_PAGE: usize = 80;
const MAX_PAGE: usize = 400;

async fn sessions(
    State(app): State<App>,
    axum::extract::Query(query): axum::extract::Query<SessionsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = app.snapshot().await;
    let added_cwds = if app.session_scope.is_some() || query.project.is_some() {
        None
    } else {
        let store = lock(&app)?;
        Some(
            store
                .projects()?
                .into_iter()
                .filter(|(_, preference)| preference.added)
                .map(|(cwd, _)| cwd)
                .collect::<std::collections::HashSet<_>>(),
        )
    };
    let limit = query.limit.unwrap_or(DEFAULT_PAGE).min(MAX_PAGE);
    let offset = query.offset.unwrap_or(0);
    let mut matching: Vec<&SessionRow> = snap
        .sessions
        .iter()
        .filter(|row| query.project.as_deref().is_none_or(|p| row.summary.cwd == p))
        .filter(|row| query.kind.is_none_or(|kind| row.summary.kind == kind))
        .filter(|row| added_cwds.as_ref().is_none_or(|cwds| cwds.contains(&row.summary.cwd)))
        .collect();
    matching.sort_by_key(|row| list_order(row));
    let page: Vec<&SessionRow> = matching.iter().skip(offset).take(limit).copied().collect();
    Ok(Json(serde_json::json!({
        "total": matching.len(),
        "offset": offset,
        "limit": limit,
        "sessions": page,
    })))
}

// ---------------------------------------------------------------- thread ----

async fn thread_view(
    State((app, workers)): State<(App, Workers)>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = app.snapshot().await;
    if let Some(row) = snap.sessions.iter().find(|row| row.summary.id == id) {
        let view = app.thread_view(row.summary.path.clone()).await?;
        let mut workflows =
            load_workflow_runs(&app, &row.summary.cwd, &view.workflow_run_ids).await;
        link_task_sessions(&mut workflows, &row.summary.cwd, &snap.sessions);
        return Ok(Json(serde_json::json!({
            "summary": row.summary,
            "settled": row.settled,
            "waitingAskuser": row.waiting_askuser,
            "omitted": view.omitted,
            "entries": view.entries,
            "tasks": snap.tasks.get(&id).cloned().unwrap_or_default(),
            "workflows": workflows,
        })));
    }

    // A new pi session has an in-memory header but does not flush its empty
    // transcript until the first assistant message. Serve that live session
    // so the client can render the composer before the first prompt.
    let worker = workers.get(&id).await.ok_or_else(|| ApiError::not_found("no such session"))?;
    let state = worker.get_state().await?;
    let now = jiff::Timestamp::now().to_string();
    let model = state.get("model");
    Ok(Json(serde_json::json!({
        "summary": {
            "id": id,
            "path": "",
            "cwd": worker.cwd().to_string_lossy(),
            "openedAt": now,
            "lastActivity": now,
            "bytes": 0,
            "provider": model.and_then(|value| value.get("provider")),
            "model": model.and_then(|value| value.get("id")),
            "preview": null,
            "title": null,
            "kind": "normal",
            "agentName": null,
        },
        "settled": false,
        "waitingAskuser": false,
        "omitted": 0,
        "entries": [],
        "tasks": [],
        "workflows": [],
    })))
}

/// Folds the workflow journals a transcript referenced; read failures degrade
/// to an empty list so the thread itself still renders.
async fn load_workflow_runs(
    app: &App,
    cwd: &str,
    run_ids: &[String],
) -> Vec<pecan_core::workflows::WorkflowRun> {
    if run_ids.is_empty() {
        return Vec::new();
    }
    let dir = app.paths.workflows_dir();
    let cwd = cwd.to_owned();
    let run_ids = run_ids.to_vec();
    tokio::task::spawn_blocking(move || pecan_core::workflows::load_runs(&dir, &cwd, &run_ids))
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "workflow journal load failed");
            Vec::new()
        })
}

/// Points each workflow task at its newest child transcript, matched by the
/// `workflow:<runId>: <label>` session name the extension writes.
fn link_task_sessions(
    runs: &mut [pecan_core::workflows::WorkflowRun],
    cwd: &str,
    sessions: &[SessionRow],
) {
    if runs.is_empty() {
        return;
    }
    // Rows are sorted newest first, so the first match is the latest attempt.
    let mut children: std::collections::HashMap<(&str, &str), &str> =
        std::collections::HashMap::new();
    for row in sessions.iter().filter(|row| row.summary.cwd == cwd) {
        if let Some(task) =
            row.summary.agent_name.as_deref().and_then(pecan_core::workflows::child_session_task)
        {
            children.entry(task).or_insert(row.summary.id.as_str());
        }
    }
    for run in runs {
        for task in &mut run.tasks {
            task.session_id = children
                .get(&(run.run_id.as_str(), task.label.as_str()))
                .map(|id| (*id).to_owned());
        }
    }
}

fn find_row<'a>(snap: &'a IndexSnapshot, id: &str) -> Result<&'a SessionRow, ApiError> {
    snap.sessions
        .iter()
        .find(|row| row.summary.id == id)
        .ok_or_else(|| ApiError::not_found("no such session"))
}

async fn regenerate_title(
    State(app): State<App>,
    axum::extract::Path(id): axum::extract::Path<String>,
    body: Result<Json<RegenerateTitleBody>, JsonRejection>,
) -> Result<Json<GeneratedTitleResponse>, ApiError> {
    let body = json_body(body)?;
    let (path, cwd, previous_title) = {
        let snap = app.snapshot().await;
        let row = find_row(&snap, &id)?;
        (row.summary.path.clone(), row.summary.cwd.clone(), row.summary.title.clone())
    };
    let generation = app.begin_title_generation(&id)?;
    let thread = tokio::task::spawn_blocking(move || thread::parse_thread(&path))
        .await
        .map_err(|_join_error| ApiError::internal(pecan_core::CoreError::Join))??;
    let generated = title::generate(
        std::path::Path::new(&cwd),
        &thread,
        previous_title.as_deref(),
        body.preset,
    )
    .await
    .map_err(|error| ApiError::bad_gateway(error.to_string()))?;
    if !app.commit_title_if_current(&id, generation, &generated)? {
        return Err(ApiError::conflict("title generation was superseded by a newer request"));
    }
    app.refresh().await?;
    Ok(Json(GeneratedTitleResponse { title: generated }))
}

#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
struct RegenerateTitleBody {
    #[serde(default)]
    preset: title::TitleModelPreset,
}

#[derive(serde::Serialize)]
struct GeneratedTitleResponse {
    title: String,
}

#[cfg(test)]
mod title_request_tests {
    use super::RegenerateTitleBody;
    use crate::server::title::TitleModelPreset;

    #[test]
    fn title_request_accepts_default_and_allowlisted_preset() {
        let default_body = serde_json::from_str::<RegenerateTitleBody>("{}");
        assert!(matches!(default_body.map(|body| body.preset), Ok(TitleModelPreset::LunaLow)));

        let selected = serde_json::from_str::<RegenerateTitleBody>(r#"{"preset":"sol-low"}"#);
        assert!(matches!(selected.map(|body| body.preset), Ok(TitleModelPreset::SolLow)));
    }

    #[test]
    fn title_request_rejects_unknown_preset_and_configuration_fields() {
        let unknown = serde_json::from_str::<RegenerateTitleBody>(r#"{"preset":"other"}"#);
        assert!(unknown.is_err());

        let injected = serde_json::from_str::<RegenerateTitleBody>(
            r#"{"preset":"luna-low","provider":"other","model":"custom"}"#,
        );
        assert!(injected.is_err());
    }
}

// ----------------------------------------------------------- settle state ----

async fn settle(
    State(app): State<App>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_visible_session(&app, &id).await?;
    {
        let store = lock(&app)?;
        store.settle(&id)?;
    }
    app.refresh().await?;
    Ok(Json(serde_json::json!({"settled": true})))
}

async fn reopen(
    State(app): State<App>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_visible_session(&app, &id).await?;
    {
        let store = lock(&app)?;
        store.reopen(&id)?;
    }
    app.refresh().await?;
    Ok(Json(serde_json::json!({"settled": false})))
}

async fn pin(
    State(app): State<App>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_visible_session(&app, &id).await?;
    {
        let store = lock(&app)?;
        store.set_pinned(&id, true)?;
    }
    app.refresh().await?;
    Ok(Json(serde_json::json!({"pinned": true})))
}

async fn unpin(
    State(app): State<App>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_visible_session(&app, &id).await?;
    {
        let store = lock(&app)?;
        store.set_pinned(&id, false)?;
    }
    app.refresh().await?;
    Ok(Json(serde_json::json!({"pinned": false})))
}

#[derive(Deserialize, Debug)]
struct RespondBody {
    /// The `extension_ui_request` id being answered.
    #[serde(rename = "requestId")]
    request_id: String,
    /// Selected option value or free-text input.
    value: Option<String>,
    /// Confirm-dialog outcome.
    confirmed: Option<bool>,
    /// Dismiss without answering.
    cancelled: Option<bool>,
}

/// Answers a pending extension UI request (`ask_user` dialogs, confirms).
async fn respond(
    State((app, workers)): State<(App, Workers)>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    body: Result<Json<RespondBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let scope = format!("respond:{id}");
    idempotent(&app.clone(), &headers, &scope, respond_once(app, workers, id, body)).await
}

async fn respond_once(
    app: App,
    workers: Workers,
    id: String,
    body: Result<Json<RespondBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let body = json_body(body)?;
    let worker = require_worker(&app, &workers, &id).await?;
    // A request recorded before this worker process spawned belongs to a
    // dead worker; answering it would silently vanish.
    let recorded_before_spawn = {
        let asks = app.pending_asks.lock().map_err(|poisoned| {
            tracing::error!(%poisoned, "pending_asks mutex poisoned");
            ApiError::internal(pecan_core::CoreError::LockPoisoned)
        })?;
        match asks.get(&id).and_then(|per_session| per_session.get(&body.request_id)) {
            Some(ask) => ask.recorded_at_ms < worker.spawned_ms(),
            None => true,
        }
    };
    if recorded_before_spawn {
        return Err(ApiError::conflict(
            "this question is no longer pending (the agent worker restarted); ask the agent to continue",
        ));
    }
    let frame = dialog_response_frame(&body.request_id, &body);
    worker.send_raw(frame).await?;
    remove_pending_ask(&app, &id, &body.request_id);
    Ok(Json(serde_json::json!({ "responded": true })))
}

/// Builds the `extension_ui_response` frame answering a dialog request,
/// echoing the exact request id so pi matches it to the blocking call.
fn dialog_response_frame(request_id: &str, body: &RespondBody) -> serde_json::Value {
    let mut frame = serde_json::json!({
        "type": "extension_ui_response",
        "id": request_id,
    });
    if let Some(map) = frame.as_object_mut() {
        if let Some(value) = body.value.as_deref() {
            map.insert("value".to_owned(), serde_json::Value::String(value.to_owned()));
        }
        if let Some(confirmed) = body.confirmed {
            map.insert("confirmed".to_owned(), serde_json::Value::Bool(confirmed));
        }
        if let Some(cancelled) = body.cancelled {
            map.insert("cancelled".to_owned(), serde_json::Value::Bool(cancelled));
        }
    }
    frame
}

/// Lists live dialog requests awaiting answers for one session.
async fn pending_asks(
    State(app): State<App>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let asks = app.pending_asks.lock().map_err(|poisoned| {
        tracing::error!(%poisoned, "pending_asks mutex poisoned");
        ApiError::internal(pecan_core::CoreError::LockPoisoned)
    })?;
    let mut list: Vec<(&String, &super::snapshot::RecordedAsk)> =
        asks.get(&id).map(|per_session| per_session.iter().collect()).unwrap_or_default();
    list.sort_by_key(|(_, ask)| ask.recorded_at_ms);
    let asks: Vec<serde_json::Value> = list
        .into_iter()
        .map(|(request_id, ask)| {
            let mut value = serde_json::to_value(ask).unwrap_or(serde_json::Value::Null);
            if let Some(map) = value.as_object_mut() {
                map.insert("id".to_owned(), serde_json::Value::String(request_id.clone()));
            }
            value
        })
        .collect();
    Ok(Json(serde_json::json!({ "asks": asks })))
}

/// Lightweight git context for a session's working directory.
async fn session_git(
    State(app): State<App>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cwd = {
        let snap = app.snapshot().await;
        find_row(&snap, &id)?.summary.cwd.clone()
    };
    let output =
        git::run("git", &cwd, &["rev-parse", "--abbrev-ref", "HEAD"], git::STATUS_TIMEOUT).await;
    let branch = match output {
        Ok(out) if out.status.success() => {
            Some(String::from_utf8_lossy(&out.stdout).trim().to_owned())
        }
        _ => None,
    };
    Ok(Json(serde_json::json!({ "branch": branch })))
}

async fn ship_plan(
    State(app): State<App>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<ship::ShipPlan>, ApiError> {
    let (cwd, title) = {
        let snap = app.snapshot().await;
        let row = find_row(&snap, &id)?;
        (row.summary.cwd.clone(), row.summary.title.clone().or_else(|| row.summary.preview.clone()))
    };
    Ok(Json(ship::plan(&cwd, title.as_deref()).await?))
}

async fn ship_execute(
    State(app): State<App>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    body: Result<Json<ship::ShipRequest>, JsonRejection>,
) -> Result<Json<ship::ShipResult>, ApiError> {
    require_ship_token(&app, &headers)?;
    let body = json_body(body)?;
    let (cwd, title) = {
        let snap = app.snapshot().await;
        let row = find_row(&snap, &id)?;
        (row.summary.cwd.clone(), row.summary.title.clone().or_else(|| row.summary.preview.clone()))
    };
    let _permit = app
        .ship_lock
        .try_lock()
        .map_err(|_busy| ApiError::conflict("another Ship action is already running"))?;
    let result = ship::execute(&cwd, title.as_deref(), &body).await?;
    {
        let store = lock(&app)?;
        store.settle(&id)?;
    }
    app.refresh().await?;
    Ok(Json(result))
}

fn require_ship_token(app: &App, headers: &HeaderMap) -> Result<(), ApiError> {
    let supplied =
        headers.get("x-pecan-ship-token").and_then(|value| value.to_str().ok()).unwrap_or_default();
    if !auth::constant_time_eq(supplied.as_bytes(), app.ship_token.as_bytes()) {
        return Err(ApiError::unauthorized("Ship capability is missing or invalid"));
    }
    Ok(())
}

#[cfg(test)]
mod ship_capability_tests {
    use crate::server::auth::constant_time_eq;

    #[test]
    fn ship_capability_rejects_missing_truncated_and_different_tokens() {
        assert!(constant_time_eq(b"same-token", b"same-token"));
        assert!(!constant_time_eq(b"", b"same-token"));
        assert!(!constant_time_eq(b"same", b"same-token"));
        assert!(!constant_time_eq(b"same-tokem", b"same-token"));
    }
}

const MAX_DIFF_BYTES: usize = 4 * 1024 * 1024;
const MAX_UNTRACKED_FILES: usize = 40;
const MAX_GIT_ERROR_BYTES: usize = 4 * 1024;

/// Bounded workspace diff for review. This is intentionally labelled as a
/// workspace diff because a dirty checkout may contain changes from outside
/// the selected agent thread.
async fn session_diff(
    State(app): State<App>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cwd = {
        let snap = app.snapshot().await;
        find_row(&snap, &id)?.summary.cwd.clone()
    };
    let branch = git_stdout(&cwd, &["symbolic-ref", "--short", "HEAD"])
        .await
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let has_head = git_command(&cwd, &["rev-parse", "--verify", "HEAD"]).await?.status.success();
    let (mut patch, mut output_truncated) = if has_head {
        let output = git_command_limited(
            &cwd,
            &["diff", "--no-ext-diff", "--find-renames", "--no-color", "HEAD", "--"],
            MAX_DIFF_BYTES,
        )
        .await?;
        if !output.status.success() && !output.truncated {
            return Err(git_failure(&output.stderr));
        }
        (String::from_utf8_lossy(&output.stdout).into_owned(), output.truncated)
    } else {
        (String::new(), false)
    };
    let untracked = git_stdout(&cwd, &["ls-files", "--others", "--exclude-standard", "-z"])
        .await?
        .split('\0')
        .filter(|path| !path.is_empty())
        .take(MAX_UNTRACKED_FILES + 1)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let untracked_truncated = untracked.len() > MAX_UNTRACKED_FILES;
    let supplemental = if has_head {
        untracked.clone()
    } else {
        git_stdout(&cwd, &["ls-files", "--cached", "--others", "--exclude-standard", "-z"])
            .await?
            .split('\0')
            .filter(|path| !path.is_empty())
            .take(MAX_UNTRACKED_FILES + 1)
            .map(str::to_owned)
            .collect()
    };
    let supplemental_truncated = supplemental.len() > MAX_UNTRACKED_FILES;
    for path in supplemental.iter().take(MAX_UNTRACKED_FILES) {
        if patch.len() >= MAX_DIFF_BYTES {
            break;
        }
        let output = git_command_limited(
            &cwd,
            &["diff", "--no-index", "--no-color", "--", "/dev/null", path],
            MAX_DIFF_BYTES.saturating_sub(patch.len()),
        )
        .await?;
        if output.status.success() || output.status.code() == Some(1) || output.truncated {
            patch.push_str(&String::from_utf8_lossy(&output.stdout));
        }
        if output.truncated {
            output_truncated = true;
            break;
        }
    }
    output_truncated |= patch.len() > MAX_DIFF_BYTES;
    if output_truncated {
        let mut boundary = MAX_DIFF_BYTES;
        while !patch.is_char_boundary(boundary) {
            boundary = boundary.saturating_sub(1);
        }
        patch.truncate(boundary);
        patch.push_str("\n\n# Pecan truncated this workspace diff at 4 MiB.\n");
    }
    let files = patch.lines().filter(|line| line.starts_with("diff --git ")).count();
    Ok(Json(serde_json::json!({
        "branch": branch,
        "files": files,
        "patch": patch,
        "truncated": output_truncated || untracked_truncated || supplemental_truncated,
        "untracked": untracked.len().min(MAX_UNTRACKED_FILES),
    })))
}

async fn git_stdout(cwd: &str, args: &[&str]) -> Result<String, ApiError> {
    let output = git_command(cwd, args).await?;
    if !output.status.success() {
        return Err(git_failure(&output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

struct LimitedGitOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    truncated: bool,
}

async fn git_command_limited(
    cwd: &str,
    args: &[&str],
    max_stdout_bytes: usize,
) -> Result<LimitedGitOutput, ApiError> {
    let bounded = git::run_bounded(
        "git",
        cwd,
        args,
        git::STATUS_TIMEOUT,
        max_stdout_bytes,
        MAX_GIT_ERROR_BYTES,
    )
    .await?;
    Ok(LimitedGitOutput {
        status: bounded.status,
        stdout: bounded.stdout,
        stderr: bounded.stderr,
        truncated: bounded.stdout_truncated,
    })
}

fn git_failure(stderr: &[u8]) -> ApiError {
    git::failure("git diff", stderr)
}

async fn git_command(cwd: &str, args: &[&str]) -> Result<std::process::Output, ApiError> {
    git::run("git", cwd, args, git::STATUS_TIMEOUT).await
}

// --------------------------------------------------------------- projects ----

#[derive(Deserialize, Debug)]
struct ProjectBody {
    cwd: String,
}

async fn add_project(
    State(app): State<App>,
    body: Result<Json<ProjectBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    reject_global_route(&app)?;
    let body = json_body(body)?;
    let expanded = folders::expand_home(body.cwd.trim(), folders::home_dir().as_deref());
    let cwd = expanded.trim_end_matches('/');
    let cwd = if cwd.is_empty() { "/" } else { cwd };
    if !cwd.starts_with('/') {
        return Err(ApiError::bad_request("cwd must be an absolute path or start with ~/".into()));
    }
    if !tokio::fs::metadata(cwd).await.is_ok_and(|meta| meta.is_dir()) {
        return Err(ApiError::bad_request(format!("{cwd} is not a folder on this machine")));
    }
    let snap = app.snapshot().await;
    let idle = pecan_core::session::idle_session_ids(
        snap.sessions.iter().map(|row| &row.summary),
        cwd,
        jiff::Timestamp::now(),
    );
    {
        let store = lock(&app)?;
        store.add_project(cwd, idle)?;
    }
    app.refresh().await?;
    Ok(Json(serde_json::json!({"added": true})))
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct FoldersQuery {
    #[serde(default)]
    path: String,
}

/// Folder picker: known Pi session folders (filtered by the typed text) and,
/// for an absolute or `~` path, the matching child directories.
async fn folders(
    State(app): State<App>,
    axum::extract::Query(query): axum::extract::Query<FoldersQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    reject_global_route(&app)?;
    let home = folders::home_dir();
    let typed = query.path.trim();
    let expanded = folders::expand_home(typed, home.as_deref());
    let snap = app.snapshot().await;
    let linked_cwds = {
        let store = lock(&app)?;
        store
            .projects()?
            .into_iter()
            .filter(|(_, pref)| pref.added)
            .map(|(cwd, _)| cwd)
            .collect::<Vec<_>>()
    };
    let linked: std::collections::HashSet<&str> = linked_cwds.iter().map(String::as_str).collect();
    let filter = if expanded.starts_with('/') { expanded.as_str() } else { typed };
    let known = folders::known_folders(snap.sessions.iter(), &linked, filter);
    let entries =
        if expanded.starts_with('/') { folders::complete(expanded).await } else { Vec::new() };
    Ok(Json(serde_json::json!({
        "home": home.map(|home| home.to_string_lossy().into_owned()),
        "known": known,
        "entries": entries,
    })))
}

async fn remove_project(
    State(app): State<App>,
    body: Result<Json<ProjectBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    reject_global_route(&app)?;
    let body = json_body(body)?;
    {
        let store = lock(&app)?;
        store.remove_project(&body.cwd)?;
    }
    app.refresh().await?;
    Ok(Json(serde_json::json!({"removed": true})))
}

// ------------------------------------------------------------ rpc actions ----

#[derive(Deserialize, Debug)]
struct NewSessionBody {
    cwd: String,
}

/// One client-supplied image attachment (base64 payload, no data: prefix).
#[derive(Deserialize, Debug)]
struct IncomingImage {
    /// Base64-encoded image bytes.
    data: String,
    /// MIME type; must be an `image/*` type.
    #[serde(rename = "mimeType")]
    mime_type: String,
}

/// Upper bound per base64 image payload (~7.5 MB decoded).
const MAX_IMAGE_BASE64_CHARS: usize = 10 * 1024 * 1024;
/// Maximum attachments per message.
const MAX_IMAGES_PER_MESSAGE: usize = 4;

impl IncomingImage {
    fn validate(&self) -> Result<(), ApiError> {
        if !self.mime_type.starts_with("image/") {
            return Err(ApiError::bad_request("attachments must be images".into()));
        }
        if self.data.is_empty() || self.data.len() > MAX_IMAGE_BASE64_CHARS {
            return Err(ApiError::bad_request("image payload too large or empty".into()));
        }
        Ok(())
    }
}

fn session_id_from_state(state: &serde_json::Value) -> Option<&str> {
    state.get("sessionId").and_then(serde_json::Value::as_str)
}

#[cfg(test)]
mod new_session_tests {
    use super::session_id_from_state;

    #[test]
    fn reads_session_id_from_get_state_payload() {
        let state = serde_json::json!({"sessionId": "session-1"});
        assert_eq!(session_id_from_state(&state), Some("session-1"));
    }

    #[test]
    fn does_not_double_unwrap_get_state_payload() {
        let state = serde_json::json!({"data": {"sessionId": "session-1"}});
        assert_eq!(session_id_from_state(&state), None);
    }
}

/// Spawns a fresh pi session rooted at `cwd` and returns its session id.
async fn new_session(
    State((app, workers)): State<(App, Workers)>,
    headers: HeaderMap,
    body: Result<Json<NewSessionBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    idempotent(&app.clone(), &headers, "new-session", new_session_once(app, workers, body)).await
}

async fn new_session_once(
    app: App,
    workers: Workers,
    body: Result<Json<NewSessionBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    reject_global_route(&app)?;
    let body = json_body(body)?;
    let dir = std::path::PathBuf::from(&body.cwd);
    if !body.cwd.starts_with('/') || !dir.is_dir() {
        return Err(ApiError::bad_request("cwd must be an existing directory".into()));
    }
    let worker = workers.spawn_new(&dir)?;
    // pi answers early commands before its session is initialized; poll until
    // the id materializes rather than failing on the first boot-time reply.
    let mut id = None;
    for attempt in 0..120_u32 {
        match worker.get_state().await {
            Ok(state) => {
                if let Some(found) = session_id_from_state(&state) {
                    id = Some(found.to_owned());
                    break;
                }
            }
            Err(error) if attempt > 5 => {
                worker.shutdown().await;
                return Err(error.into());
            }
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let Some(id) = id else {
        worker.shutdown().await;
        tracing::warn!("new session: worker never reported a session id (30s window)");
        return Err(ApiError::bad_gateway(
            "worker reported no session id within startup window (30s)".into(),
        ));
    };
    workers.insert(id.clone(), std::sync::Arc::clone(&worker)).await;
    // The new transcript is created by pi during startup. Refresh the
    // filesystem-backed index before the client navigates to the new id.
    app.refresh().await?;
    forward_worker_events(app, workers, &id, &worker);
    Ok(Json(serde_json::json!({ "id": id })))
}

/// Message send behavior: `send` when idle, `steer`/`queue` to affect an
/// already-streaming turn.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum MessageMode {
    Send,
    Steer,
    Queue,
}

#[derive(Deserialize, Debug)]
struct MessageBody {
    text: String,
    mode: MessageMode,
    /// Optional image attachments (base64, no data: prefix).
    #[serde(default)]
    images: Vec<IncomingImage>,
}

async fn message(
    State((app, workers)): State<(App, Workers)>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    body: Result<Json<MessageBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let scope = format!("message:{id}");
    idempotent(&app.clone(), &headers, &scope, message_once(app, workers, id, body)).await
}

async fn message_once(
    app: App,
    workers: Workers,
    id: String,
    body: Result<Json<MessageBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let body = json_body(body)?;
    if body.text.trim().is_empty() && body.images.is_empty() {
        return Err(ApiError::bad_request("empty message".into()));
    }
    if body.images.len() > MAX_IMAGES_PER_MESSAGE {
        return Err(ApiError::bad_request("too many image attachments".into()));
    }
    for image in &body.images {
        image.validate()?;
    }
    let path = {
        let snap = app.snapshot().await;
        snap.sessions.iter().find(|row| row.summary.id == id).map(|row| row.summary.path.clone())
    };
    let worker = if let Some(path) = path {
        workers.get_or_spawn(&id, &path).await?
    } else {
        workers.get(&id).await.ok_or_else(|| ApiError::not_found("no such session"))?
    };
    forward_worker_events(app.clone(), workers.clone(), &id, &worker);

    let state = worker.get_state().await?;
    let streaming = state.get("isStreaming").and_then(serde_json::Value::as_bool) == Some(true);
    let cmd = match (streaming, body.mode) {
        (true, MessageMode::Send) => {
            return Err(ApiError::conflict("agent is streaming; use steer or queue"));
        }
        (false, MessageMode::Steer | MessageMode::Queue) => prompt_cmd("prompt", &body),
        (_, MessageMode::Send) => prompt_cmd("prompt", &body),
        (_, MessageMode::Steer) => prompt_cmd("steer", &body),
        (_, MessageMode::Queue) => prompt_cmd("follow_up", &body),
    };
    // Fire-and-forget semantics for the client: pi streams results via SSE.
    tokio::spawn(async move {
        if let Err(error) = worker.command(cmd).await {
            tracing::warn!(%error, "prompt command failed");
        }
    });
    Ok(Json(serde_json::json!({"accepted": true, "wasStreaming": streaming})))
}

/// Builds a prompt-family command with optional image attachments.
fn prompt_cmd(kind: &str, body: &MessageBody) -> serde_json::Value {
    let mut cmd = serde_json::json!({"type": kind, "message": body.text});
    if !body.images.is_empty()
        && let Some(map) = cmd.as_object_mut()
    {
        let images = body
            .images
            .iter()
            .map(|image| {
                serde_json::json!({
                    "type": "image",
                    "data": image.data,
                    "mimeType": image.mime_type,
                })
            })
            .collect();
        map.insert("images".to_owned(), serde_json::Value::Array(images));
    }
    cmd
}

async fn abort(
    State((app, workers)): State<(App, Workers)>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_visible_session(&app, &id).await?;
    let Some(worker) = workers.remove(&id).await else {
        return Ok(Json(serde_json::json!({"aborted": false})));
    };
    let result = worker.command(serde_json::json!({"type": "abort"})).await;
    worker.shutdown().await;
    match result {
        Ok(_) => Ok(Json(serde_json::json!({"aborted": true}))),
        Err(error) => Err(error.into()),
    }
}

/// Live run state without spawning a worker: `{live, streaming}`.
async fn session_status(
    State((app, workers)): State<(App, Workers)>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_visible_session(&app, &id).await?;
    let Some(worker) = workers.get(&id).await else {
        return Ok(Json(serde_json::json!({"live": false, "streaming": false})));
    };
    let state = worker.get_state().await?;
    let streaming = state.get("isStreaming").and_then(serde_json::Value::as_bool).unwrap_or(false);
    Ok(Json(serde_json::json!({"live": true, "streaming": streaming})))
}

async fn agent_attach(
    State((app, workers)): State<(App, Workers)>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let worker = require_worker(&app, &workers, &id).await?;
    forward_worker_events(app.clone(), workers.clone(), &id, &worker);
    agent_snapshot(worker, &id, super::model_favorites::favorite_models(&app.paths)).await
}

/// Pipes raw pi events from a worker onto the SSE bus exactly once per spawn.
fn forward_worker_events(
    app: App,
    workers: Workers,
    id: &str,
    worker: &std::sync::Arc<super::worker::WorkerHandle>,
) {
    if !worker.try_start_forwarding() {
        return;
    }
    let mut rx = worker.raw_events.subscribe();
    let session_id = id.to_owned();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if event.get("type").and_then(serde_json::Value::as_str)
                        == Some(super::worker::WORKER_EXIT)
                    {
                        break;
                    }
                    // A finished turn means the transcript changed on disk.
                    let settled = matches!(
                        event.get("type").and_then(serde_json::Value::as_str),
                        Some("agent_settled") | Some("message_end")
                    );
                    if event.get("type").and_then(serde_json::Value::as_str)
                        == Some("extension_ui_request")
                    {
                        record_pending_ask(&app, &session_id, &event);
                    }
                    notify_away_devices(&app, &session_id, &event).await;
                    app.events.send(ServerEvent::AgentEvent { id: session_id.clone(), event });
                    if settled {
                        app.events.send(ServerEvent::ThreadChanged { id: session_id.clone() });
                        if let Err(error) = app.refresh().await {
                            tracing::warn!(%error, "post-turn refresh failed");
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
        app.events.send(ServerEvent::AgentEvent {
            id: session_id.clone(),
            event: serde_json::json!({
                "type": "extension_ui_request",
                "method": "setWidget",
                "widgetKey": "pi-subagents/activity/v1"
            }),
        });
        // The worker is gone (idle reap, crash, abort): its dialogs can no
        // longer be answered and any in-flight turn will never settle.
        if let Ok(mut asks) = app.pending_asks.lock() {
            asks.remove(&session_id);
        }
        app.events.send(ServerEvent::AgentEvent {
            id: session_id.clone(),
            event: serde_json::json!({ "type": super::worker::WORKER_EXIT }),
        });
        workers.remove(&session_id).await;
    });
}

/// Pushes a finished turn or a blocking dialog to devices that are not
/// watching (see [`push::Push::notify`]).
async fn notify_away_devices(app: &App, id: &str, event: &serde_json::Value) {
    let kind = event.get("type").and_then(serde_json::Value::as_str);
    let blocking = kind == Some("extension_ui_request")
        && event
            .get("method")
            .and_then(serde_json::Value::as_str)
            .is_some_and(is_blocking_dialog_method);
    if kind != Some("agent_settled") && !blocking {
        return;
    }
    let snap = app.snapshot().await;
    let title = find_row(&snap, id).ok().and_then(|row| row.summary.title.as_deref());
    let notice = if blocking {
        push::Notice::needs_input(id, title)
    } else {
        push::Notice::turn_done(id, title)
    };
    app.push.notify(app, notice);
}

/// Returns whether an extension request blocks the agent waiting for a user
/// answer. Non-blocking UI updates (widgets, status, notifications) do not
/// belong in the answer registry.
fn is_blocking_dialog_method(method: &str) -> bool {
    matches!(method, "select" | "confirm" | "input" | "editor")
}

/// Preserves the renderable fields of one blocking dialog request. Called
/// only for methods accepted by [`is_blocking_dialog_method`]; fire-and-forget
/// UI updates are never registered.
fn dialog_fields(method: &str, event: &serde_json::Value) -> super::snapshot::RecordedAsk {
    super::snapshot::RecordedAsk {
        method: method.to_owned(),
        title: event.get("title").and_then(serde_json::Value::as_str).map(str::to_owned),
        options: event.get("options").and_then(serde_json::Value::as_array).cloned(),
        message: event.get("message").and_then(serde_json::Value::as_str).map(str::to_owned),
        placeholder: event
            .get("placeholder")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        prefill: event.get("prefill").and_then(serde_json::Value::as_str).map(str::to_owned),
        recorded_at_ms: jiff::Timestamp::now().as_second().saturating_mul(1_000),
    }
}

/// Records a dialog request so answers survive browser reloads.
fn record_pending_ask(app: &App, session_id: &str, event: &serde_json::Value) {
    let Some(request_id) = event.get("id").and_then(serde_json::Value::as_str) else {
        return;
    };
    let Some(method) = event.get("method").and_then(serde_json::Value::as_str) else {
        return;
    };
    if !is_blocking_dialog_method(method) {
        return;
    }
    let ask = dialog_fields(method, event);
    match app.pending_asks.lock() {
        Ok(mut asks) => record_pending_ask_in(&mut asks, session_id, request_id, ask),
        Err(error) => tracing::warn!(%error, "pending asks lock poisoned"),
    }
}

/// Keys a recorded dialog by its exact pi request id inside a session bucket.
fn record_pending_ask_in(
    asks: &mut super::snapshot::PendingAsks,
    session_id: &str,
    request_id: &str,
    ask: super::snapshot::RecordedAsk,
) {
    asks.entry(session_id.to_owned()).or_default().insert(request_id.to_owned(), ask);
}

#[cfg(test)]
mod dialog_request_tests {
    use super::{
        RespondBody, dialog_fields, dialog_response_frame, is_blocking_dialog_method,
        record_pending_ask_in, remove_pending_ask_in,
    };
    use crate::server::snapshot::PendingAsks;

    #[test]
    fn every_dialog_method_is_blocking() {
        for method in ["select", "confirm", "input", "editor"] {
            assert!(is_blocking_dialog_method(method), "{method} is a blocking dialog");
        }
    }

    #[test]
    fn fire_and_forget_updates_are_not_dialogs() {
        for method in ["setWidget", "setStatus", "setTitle", "notify", "set_editor_text"] {
            assert!(!is_blocking_dialog_method(method), "{method} must not be recorded");
        }
    }

    #[test]
    fn preserves_each_dialogs_renderable_fields() {
        let select = dialog_fields(
            "select",
            &serde_json::json!({
                "id": "request-select",
                "method": "select",
                "title": "Pick",
                "options": ["Allow", "Block"],
                "timeout": 10000,
            }),
        );
        assert_eq!(select.method, "select");
        assert_eq!(select.title.as_deref(), Some("Pick"));
        assert_eq!(
            select.options,
            Some(vec![serde_json::json!("Allow"), serde_json::json!("Block")])
        );
        assert_eq!(select.message, None);
        assert_eq!(select.prefill, None);

        let confirm = dialog_fields(
            "confirm",
            &serde_json::json!({
                "id": "request-confirm",
                "method": "confirm",
                "title": "Clear session?",
                "message": "All messages will be lost.",
            }),
        );
        assert_eq!(confirm.method, "confirm");
        assert_eq!(confirm.title.as_deref(), Some("Clear session?"));
        assert_eq!(confirm.message.as_deref(), Some("All messages will be lost."));

        let input = dialog_fields(
            "input",
            &serde_json::json!({
                "id": "request-input",
                "method": "input",
                "title": "Enter a value",
                "placeholder": "type something...",
            }),
        );
        assert_eq!(input.method, "input");
        assert_eq!(input.placeholder.as_deref(), Some("type something..."));

        let editor = dialog_fields(
            "editor",
            &serde_json::json!({
                "id": "request-editor",
                "method": "editor",
                "title": "Edit some text",
                "prefill": "Line 1\nLine 2",
            }),
        );
        assert_eq!(editor.method, "editor");
        assert_eq!(editor.title.as_deref(), Some("Edit some text"));
        assert_eq!(editor.prefill.as_deref(), Some("Line 1\nLine 2"));
    }

    #[test]
    fn registers_and_cleans_up_dialogs_by_exact_request_id() {
        let mut asks = PendingAsks::new();
        let event = serde_json::json!({
            "id": "uuid-1",
            "method": "select",
            "title": "Allow dangerous command?",
            "options": ["Allow", "Block"],
        });
        record_pending_ask_in(&mut asks, "session-1", "uuid-1", dialog_fields("select", &event));
        let event = serde_json::json!({
            "id": "uuid-2",
            "method": "editor",
            "title": "Edit some text",
            "prefill": "Line 1\nLine 2",
        });
        record_pending_ask_in(&mut asks, "session-1", "uuid-2", dialog_fields("editor", &event));
        record_pending_ask_in(
            &mut asks,
            "session-2",
            "uuid-3",
            dialog_fields(
                "input",
                &serde_json::json!({"id": "uuid-3", "method": "input", "title": "Value"}),
            ),
        );

        assert_eq!(asks["session-1"].len(), 2);
        assert_eq!(asks["session-1"]["uuid-1"].method, "select");
        assert_eq!(asks["session-1"]["uuid-2"].prefill.as_deref(), Some("Line 1\nLine 2"));
        assert!(asks["session-2"].contains_key("uuid-3"));

        // Answer one dialog; the bucket keeps the sibling request.
        remove_pending_ask_in(&mut asks, "session-1", "uuid-1");
        assert_eq!(asks["session-1"].len(), 1);
        assert!(asks["session-1"].contains_key("uuid-2"));

        // Answering the last dialog drops the whole session bucket.
        remove_pending_ask_in(&mut asks, "session-1", "uuid-2");
        assert!(!asks.contains_key("session-1"));
        assert!(asks.contains_key("session-2"));
    }

    #[test]
    fn response_frame_echoes_the_exact_request_id() {
        let value = dialog_response_frame(
            "uuid-1",
            &RespondBody {
                request_id: "uuid-1".into(),
                value: Some("Allow".into()),
                confirmed: None,
                cancelled: None,
            },
        );
        assert_eq!(
            value,
            serde_json::json!({"type": "extension_ui_response", "id": "uuid-1", "value": "Allow"})
        );

        let confirmed = dialog_response_frame(
            "uuid-2",
            &RespondBody {
                request_id: "uuid-2".into(),
                value: None,
                confirmed: Some(false),
                cancelled: None,
            },
        );
        assert_eq!(
            confirmed,
            serde_json::json!({"type": "extension_ui_response", "id": "uuid-2", "confirmed": false})
        );

        let cancelled = dialog_response_frame(
            "uuid-3",
            &RespondBody {
                request_id: "uuid-3".into(),
                value: None,
                confirmed: None,
                cancelled: Some(true),
            },
        );
        assert_eq!(
            cancelled,
            serde_json::json!({"type": "extension_ui_response", "id": "uuid-3", "cancelled": true})
        );
    }
}

/// Drops an answered dialog request from the live registry.
fn remove_pending_ask(app: &App, session_id: &str, request_id: &str) {
    if let Ok(mut asks) = app.pending_asks.lock() {
        remove_pending_ask_in(&mut asks, session_id, request_id);
    }
}

/// Removes one answered dialog and drops empty session buckets so answered
/// dialogs (and stale workers) cannot linger in the registry.
fn remove_pending_ask_in(
    asks: &mut super::snapshot::PendingAsks,
    session_id: &str,
    request_id: &str,
) {
    if let Some(per_session) = asks.get_mut(session_id) {
        per_session.remove(request_id);
        if per_session.is_empty() {
            asks.remove(session_id);
        }
    }
}

#[derive(Deserialize, Debug)]
struct ModelBody {
    provider: String,
    #[serde(rename = "modelId")]
    model_id: String,
}

async fn set_model(
    State((app, workers)): State<(App, Workers)>,
    axum::extract::Path(id): axum::extract::Path<String>,
    body: Result<Json<ModelBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let body = json_body(body)?;
    let worker = require_worker(&app, &workers, &id).await?;
    worker
        .command(serde_json::json!({
            "type": "set_model",
            "provider": body.provider,
            "modelId": body.model_id,
        }))
        .await?;
    Ok(Json(serde_json::json!({"ok": true})))
}

#[derive(Deserialize, Debug)]
struct ThinkingBody {
    level: String,
}

async fn set_thinking(
    State((app, workers)): State<(App, Workers)>,
    axum::extract::Path(id): axum::extract::Path<String>,
    body: Result<Json<ThinkingBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let body = json_body(body)?;
    let worker = require_worker(&app, &workers, &id).await?;
    worker.command(serde_json::json!({"type": "set_thinking_level", "level": body.level})).await?;
    Ok(Json(serde_json::json!({"ok": true})))
}

/// Fetches or spawns the worker for a session, mapping errors for handlers.
async fn require_worker(
    app: &App,
    workers: &Workers,
    id: &str,
) -> Result<std::sync::Arc<super::worker::WorkerHandle>, ApiError> {
    let snap = app.snapshot().await;
    if let Some(row) = snap.sessions.iter().find(|row| row.summary.id == id) {
        return workers.get_or_spawn(id, &row.summary.path).await.map_err(ApiError::from);
    }
    workers.get(id).await.ok_or_else(|| ApiError::not_found("no such session"))
}

/// Collects model/thinking/context info from a live worker, plus pi's
/// favorite model patterns.
async fn agent_snapshot(
    worker: std::sync::Arc<super::worker::WorkerHandle>,
    id: &str,
    favorites: Vec<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state = worker.get_state().await?;
    let stats = worker
        .command(serde_json::json!({"type": "get_session_stats"}))
        .await
        .ok()
        .and_then(|res| res.get("data").cloned());
    let models = worker
        .command(serde_json::json!({"type": "get_available_models"}))
        .await
        .ok()
        .and_then(|res| res.get("data").and_then(|d| d.get("models")).cloned())
        .unwrap_or(serde_json::Value::Null);
    Ok(Json(serde_json::json!({
        "sessionId": id,
        "state": state,
        "stats": stats,
        "models": models,
        "favorites": favorites,
    })))
}

// -------------------------------------------------------------------- sse ----

#[derive(Deserialize, Debug, Default)]
struct EventsQuery {
    /// Last sequence the client saw (`EventSource` cannot set headers when
    /// the page re-creates it, so the cursor also rides in the query).
    after: Option<u64>,
}

/// Server-sent events. Each event carries its sequence as the SSE `id`; a
/// client reconnecting with `?after=<seq>` or `Last-Event-ID` gets exactly
/// what it missed, or a `reset` event when the gap left the replay window.
/// The first frame is always `ready` with `{head, reset, replayed}`.
async fn events(
    State(app): State<App>,
    axum::extract::Extension(principal): axum::extract::Extension<Principal>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<EventsQuery>,
) -> Sse<impl Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    let header_cursor = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok());
    let cursor = query.after.or(header_cursor);
    let subscription = app.events.subscribe(cursor);
    let ready = serde_json::json!({
        "head": subscription.head,
        "reset": subscription.reset,
        "replayed": subscription.replay.len(),
    });
    let mut pending: std::collections::VecDeque<axum::response::sse::Event> =
        std::collections::VecDeque::with_capacity(subscription.replay.len() + 1);
    pending.push_back(axum::response::sse::Event::default().event("ready").data(ready.to_string()));
    pending.extend(subscription.replay.iter().map(sse_event));
    // Replay always ends at `head`; the live receiver starts right after it.
    let last = subscription.head;
    // A revoked device loses its open stream, not just its next request.
    let (device, live_stream) = match principal {
        Principal::Device(id) => {
            let guard = app.push.stream_opened(&id);
            (Some((id, app.auth.revocations())), Some(guard))
        }
        Principal::Cli => (None, None),
    };
    let state = EventStreamState {
        log: app.events,
        live: subscription.live,
        pending,
        last,
        device,
        _live_stream: live_stream,
    };
    let stream = futures::stream::unfold(state, |mut state| async move {
        loop {
            if let Some(event) = state.pending.pop_front() {
                return Some((Ok(event), state));
            }
            let received = match state.device.as_mut() {
                Some((id, revocations)) => tokio::select! {
                    item = state.live.recv() => item,
                    revoked = revocations.recv() => match revoked {
                        Ok(revoked) if revoked == *id => return None,
                        // Lagged may have skipped this device's revocation;
                        // closing makes the client reconnect and re-check.
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => return None,
                        Ok(_) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            state.device = None;
                            continue;
                        }
                    },
                },
                None => state.live.recv().await,
            };
            match received {
                Ok(item) if item.seq <= state.last => {}
                Ok(item) => {
                    state.last = item.seq;
                    return Some((Ok(sse_event(&item)), state));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // Too slow for the live buffer: resume from the replay
                    // window, or tell the client to refetch.
                    let resumed = state.log.subscribe(Some(state.last));
                    state.live = resumed.live;
                    if resumed.reset {
                        state.last = resumed.head;
                        let data = serde_json::json!({ "head": resumed.head }).to_string();
                        state.pending.push_back(
                            axum::response::sse::Event::default().event("reset").data(data),
                        );
                    } else {
                        for item in &resumed.replay {
                            state.pending.push_back(sse_event(item));
                            state.last = item.seq;
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

struct EventStreamState {
    log: super::event_log::EventLog,
    live: tokio::sync::broadcast::Receiver<super::event_log::Sequenced>,
    pending: std::collections::VecDeque<axum::response::sse::Event>,
    /// The streaming device and a feed of revoked ids, for browsers only.
    device: Option<(String, tokio::sync::broadcast::Receiver<String>)>,
    last: u64,
    /// Marks the device as watching, so it is not pushed, until the stream drops.
    _live_stream: Option<push::LiveStream>,
}

fn sse_event(item: &super::event_log::Sequenced) -> axum::response::sse::Event {
    axum::response::sse::Event::default()
        .id(item.seq.to_string())
        .event(match item.event.as_ref() {
            ServerEvent::IndexChanged => "index-changed",
            ServerEvent::ThreadChanged { .. } | ServerEvent::AgentEvent { .. } => "agent",
        })
        .data(serde_json::to_string(item.event.as_ref()).unwrap_or_else(|_| "{}".to_owned()))
}

// ------------------------------------------------------------------ misc ----

/// Runs a mutation at most once per `Idempotency-Key` header (when present):
/// a retry after a lost response gets the first result back instead of
/// sending the prompt or dialog answer twice.
async fn idempotent(
    app: &App,
    headers: &HeaderMap,
    scope: &str,
    run: impl Future<Output = Result<Json<serde_json::Value>, ApiError>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use super::idempotency::{Claim, ClaimError};
    let Some(raw) = headers.get("idempotency-key") else {
        return run.await;
    };
    let key = raw
        .to_str()
        .map_err(|_non_ascii| ApiError::bad_request(ClaimError::InvalidKey.to_string()))?;
    let guard = match app.idempotency.claim(scope, key) {
        Ok(Claim::Replay(body)) => return Ok(Json(body)),
        Ok(Claim::Fresh(guard)) => guard,
        Err(error @ ClaimError::InvalidKey) => {
            return Err(ApiError::bad_request(error.to_string()));
        }
        Err(error @ ClaimError::InFlight) => return Err(ApiError::conflict(&error.to_string())),
        Err(error @ ClaimError::Poisoned) => return Err(ApiError::internal(error)),
    };
    let result = run.await;
    guard.finish(result.as_ref().ok().map(|Json(body)| body));
    result
}

pub(crate) fn lock(
    app: &App,
) -> Result<std::sync::MutexGuard<'_, pecan_core::store::StateStore>, ApiError> {
    app.store.lock().map_err(|poisoned| {
        tracing::error!(%poisoned, "state store mutex poisoned");
        ApiError::internal(pecan_core::CoreError::LockPoisoned)
    })
}

async fn require_visible_session(app: &App, id: &str) -> Result<(), ApiError> {
    let snap = app.snapshot().await;
    let _ = find_row(&snap, id)?;
    Ok(())
}

fn reject_global_route(app: &App) -> Result<(), ApiError> {
    if app.session_scope.is_some() {
        return Err(ApiError::not_found("not available in single-session mode"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, kind: SessionKind, settled: bool, last_activity: &str) -> SessionRow {
        let at = last_activity.parse::<jiff::Timestamp>().expect("valid test timestamp");
        SessionRow {
            summary: pecan_core::session::SessionSummary {
                id: id.to_owned(),
                path: std::path::PathBuf::from(format!("/{id}.jsonl")),
                cwd: "/p".to_owned(),
                opened_at: at,
                last_activity: at,
                bytes: 1,
                provider: None,
                model: None,
                preview: None,
                title: None,
                kind,
                agent_name: None,
            },
            settled,
            pinned: false,
            waiting_askuser: false,
            parent_session_id: None,
        }
    }

    #[test]
    fn list_order_puts_threads_before_children_and_done_last() {
        let mut rows = [
            row("new-child", SessionKind::Subagent, false, "2026-09-27T10:00:00Z"),
            row("done", SessionKind::Normal, true, "2026-09-27T09:00:00Z"),
            row("old", SessionKind::Normal, false, "2026-09-01T00:00:00Z"),
            row("recent", SessionKind::Normal, false, "2026-09-26T00:00:00Z"),
        ];
        rows.sort_by_key(list_order);
        let ids: Vec<&str> = rows.iter().map(|row| row.summary.id.as_str()).collect();
        assert_eq!(ids, ["recent", "old", "done", "new-child"]);
    }
}
