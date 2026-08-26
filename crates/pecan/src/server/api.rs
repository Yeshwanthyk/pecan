//! HTTP API: JSON handlers and the SSE stream.

use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::HeaderMap;
use axum::response::Sse;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::{Stream, StreamExt};
use pecan_core::thread;
use serde::Deserialize;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio_stream::wrappers::BroadcastStream;

use super::ship;
use super::snapshot::{App, IndexSnapshot, ServerEvent, SessionRow};
use super::title;
use super::ui_plugins;
use super::worker::Workers;
use crate::server::api_errors::ApiError;

/// Builds the `/api` router.
#[must_use]
pub(crate) fn router(app: App, workers: Workers) -> Router {
    let with_workers = Router::new()
        // Image attachments ride in the message JSON body.
        .layer(axum::extract::DefaultBodyLimit::max(48 * 1024 * 1024))
        .route("/session/{id}", get(thread_view))
        .route("/session/new", post(new_session))
        .route("/session/{id}/message", post(message))
        .route("/session/{id}/respond", post(respond))
        .route("/session/{id}/abort", post(abort))
        .route("/session/{id}/agent-attach", post(agent_attach))
        .route("/session/{id}/set-model", post(set_model))
        .route("/session/{id}/set-thinking", post(set_thinking))
        .with_state((app.clone(), workers));
    let base = Router::new()
        .route("/bootstrap", get(bootstrap))
        .route("/sessions", get(sessions))
        .route("/session/{id}/git", get(session_git))
        .route("/session/{id}/diff", get(session_diff))
        .route("/session/{id}/ship", get(ship_plan).post(ship_execute))
        .route("/session/{id}/settle", post(settle).delete(reopen))
        .route("/session/{id}/pin", post(pin).delete(unpin))
        .route("/session/{id}/asks", get(pending_asks))
        .route("/session/{id}/title/regenerate", post(regenerate_title))
        .route("/projects/settle-stale", post(settle_stale))
        .route("/projects", post(add_project).delete(remove_project))
        .route("/ui-plugins/{id}", post(set_ui_plugin))
        .route("/state/seed-init", post(seed_init))
        .route("/events", get(events))
        .with_state(app)
        .merge(with_workers);
    base.fallback(api_not_found)
}

async fn api_not_found() -> ApiError {
    ApiError::not_found("no such API route")
}

// ------------------------------------------------------------- bootstrap ----

async fn bootstrap(State(app): State<App>) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = app.snapshot().await;
    let store = lock(&app)?;
    let seeded = store.is_seeded().map_err(ApiError::internal)?;
    let ui_plugins = ui_plugins::catalog(&app.paths, &store).map_err(ApiError::internal)?;
    let project_rows = if app.session_scope.is_some() {
        Vec::new()
    } else {
        store
            .projects()
            .map_err(ApiError::internal)?
            .into_iter()
            .filter(|(_, pref)| pref.added)
            .collect::<Vec<_>>()
    };
    let projects: Vec<serde_json::Value> = project_rows
        .iter()
        .map(|(cwd, _)| {
            serde_json::json!({
                "cwd": cwd,
                "name": display_name(&cwd),
                "sessionCount": session_count(&snap, &cwd),
                "lastActivity": last_activity(&snap, &cwd),
            })
        })
        .collect();
    let sessions: Vec<&SessionRow> = if app.session_scope.is_some() {
        snap.sessions.iter().collect()
    } else {
        let added_cwds: std::collections::HashSet<&str> =
            project_rows.iter().map(|(cwd, _)| cwd.as_str()).collect();
        snap.sessions
            .iter()
            .filter(|row| added_cwds.contains(row.summary.cwd.as_str()))
            .take(MAX_PAGE)
            .collect()
    };
    Ok(Json(serde_json::json!({
        "seeded": seeded,
        "sessionScope": app.session_scope,
        "projects": projects,
        "sessions": sessions,
        "uiPlugins": ui_plugins,
    })))
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct SetUiPluginBody {
    enabled: bool,
}

async fn set_ui_plugin(
    State(app): State<App>,
    axum::extract::Path(id): axum::extract::Path<String>,
    body: Result<Json<SetUiPluginBody>, JsonRejection>,
) -> Result<Json<ui_plugins::UiPluginDescriptor>, ApiError> {
    if id != ui_plugins::PI_TASKS_ID {
        return Err(ApiError::not_found("unknown UI plugin"));
    }
    let Json(body) = body.map_err(|error| ApiError::bad_request(error.to_string()))?;
    let descriptor = {
        let store = lock(&app)?;
        let current = ui_plugins::catalog(&app.paths, &store)
            .map_err(ApiError::internal)?
            .into_iter()
            .next()
            .ok_or_else(|| ApiError::not_found("unknown UI plugin"))?;
        if body.enabled && (!current.detected || !current.source_enabled) {
            return Err(ApiError::conflict("Pi Tasks must be installed and enabled in Pi first"));
        }
        store
            .set_ui_plugin_enabled(ui_plugins::PI_TASKS_ID, body.enabled)
            .map_err(ApiError::internal)?;
        ui_plugins::catalog(&app.paths, &store)
            .map_err(ApiError::internal)?
            .into_iter()
            .next()
            .ok_or_else(|| ApiError::not_found("unknown UI plugin"))?
    };
    app.refresh().await.map_err(ApiError::internal)?;
    Ok(Json(descriptor))
}

fn session_count(snap: &IndexSnapshot, cwd: &str) -> usize {
    snap.sessions.iter().filter(|s| s.summary.cwd == cwd).count()
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

// -------------------------------------------------------------- sessions ----

#[derive(Deserialize, Debug, Default)]
struct SessionsQuery {
    project: Option<String>,
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
                .projects()
                .map_err(ApiError::internal)?
                .into_iter()
                .filter(|(_, preference)| preference.added)
                .map(|(cwd, _)| cwd)
                .collect::<std::collections::HashSet<_>>(),
        )
    };
    let limit = query.limit.unwrap_or(DEFAULT_PAGE).min(MAX_PAGE);
    let offset = query.offset.unwrap_or(0);
    let matching: Vec<&SessionRow> = snap
        .sessions
        .iter()
        .filter(|row| query.project.as_deref().is_none_or(|p| row.summary.cwd == p))
        .filter(|row| added_cwds.as_ref().is_none_or(|cwds| cwds.contains(&row.summary.cwd)))
        .collect();
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
        let view = app.thread_view(row.summary.path.clone()).await.map_err(ApiError::internal)?;
        return Ok(Json(serde_json::json!({
            "summary": row.summary,
            "settled": row.settled,
            "waitingAskuser": row.waiting_askuser,
            "omitted": view.omitted,
            "entries": view.entries,
            "tasks": snap.tasks.get(&id).cloned().unwrap_or_default(),
            "workflows": snap.workflows.get(&id).cloned().unwrap_or_default(),
        })));
    }

    // A new pi session has an in-memory header but does not flush its empty
    // transcript until the first assistant message. Serve that live session
    // so the client can render the composer before the first prompt.
    let worker = workers.get(&id).await.ok_or_else(|| ApiError::not_found("no such session"))?;
    let state =
        worker.get_state().await.map_err(|error| ApiError::bad_gateway(error.to_string()))?;
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
            "parentId": null,
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
    let Json(body) = body.map_err(|error| ApiError::bad_request(error.to_string()))?;
    let (path, cwd, previous_title) = {
        let snap = app.snapshot().await;
        let row = find_row(&snap, &id)?;
        (row.summary.path.clone(), row.summary.cwd.clone(), row.summary.title.clone())
    };
    let generation = app.begin_title_generation(&id).map_err(ApiError::internal)?;
    let thread = tokio::task::spawn_blocking(move || thread::parse_thread(&path))
        .await
        .map_err(|_join_error| ApiError::internal(pecan_core::CoreError::Join))?
        .map_err(ApiError::internal)?;
    let generated = title::generate(
        std::path::Path::new(&cwd),
        &thread,
        previous_title.as_deref(),
        body.preset,
    )
    .await
    .map_err(|error| ApiError::bad_gateway(error.to_string()))?;
    if !app.commit_title_if_current(&id, generation, &generated).map_err(ApiError::internal)? {
        return Err(ApiError::conflict("title generation was superseded by a newer request"));
    }
    app.refresh().await.map_err(ApiError::internal)?;
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
        store.settle(&id).map_err(ApiError::internal)?;
    }
    app.refresh().await.map_err(ApiError::internal)?;
    Ok(Json(serde_json::json!({"settled": true})))
}

async fn reopen(
    State(app): State<App>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_visible_session(&app, &id).await?;
    {
        let store = lock(&app)?;
        store.reopen(&id).map_err(ApiError::internal)?;
    }
    app.refresh().await.map_err(ApiError::internal)?;
    Ok(Json(serde_json::json!({"settled": false})))
}

#[derive(Deserialize, Debug)]
struct SettleStaleBody {
    cwd: String,
    /// Settle sessions whose last activity is older than this many days.
    days: f64,
}

/// Bulk-settles every session in `cwd` idle beyond `days` days.
async fn settle_stale(
    State(app): State<App>,
    body: Result<Json<SettleStaleBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    reject_global_route(&app)?;
    let Json(body) = body.map_err(|e| ApiError::bad_request(e.to_string()))?;
    if !(0.0..=365.0).contains(&body.days) {
        return Err(ApiError::bad_request("days must be within 0..=365".into()));
    }
    let cutoff_ms = jiff::Timestamp::now().as_second() * 1000 - (body.days * 86_400_000.0) as i64;
    let ids: Vec<String> = {
        let snap = app.snapshot().await;
        snap.sessions
            .iter()
            .filter(|row| row.summary.cwd == body.cwd)
            .filter(|row| row.summary.last_activity.as_second() * 1000 < cutoff_ms)
            .filter(|row| !row.settled)
            .map(|row| row.summary.id.clone())
            .collect()
    };
    if !ids.is_empty() {
        {
            let store = lock(&app)?;
            store.settle_many(&ids).map_err(ApiError::internal)?;
        }
        app.refresh().await.map_err(ApiError::internal)?;
    }
    Ok(Json(serde_json::json!({ "settled": ids.len() })))
}

async fn pin(
    State(app): State<App>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_visible_session(&app, &id).await?;
    {
        let store = lock(&app)?;
        store.set_pinned(&id, true).map_err(ApiError::internal)?;
    }
    app.refresh().await.map_err(ApiError::internal)?;
    Ok(Json(serde_json::json!({"pinned": true})))
}

async fn unpin(
    State(app): State<App>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_visible_session(&app, &id).await?;
    {
        let store = lock(&app)?;
        store.set_pinned(&id, false).map_err(ApiError::internal)?;
    }
    app.refresh().await.map_err(ApiError::internal)?;
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

/// Answers a pending extension UI request (ask_user dialogs, confirms).
async fn respond(
    State((app, workers)): State<(App, Workers)>,
    axum::extract::Path(id): axum::extract::Path<String>,
    body: Result<Json<RespondBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Json(body) = body.map_err(|e| ApiError::bad_request(e.to_string()))?;
    let worker = require_worker(&app, &workers, &id).await?;
    // A request recorded before this worker process spawned belongs to a
    // dead worker; answering it would silently vanish.
    let recorded_before_spawn = {
        let asks = app
            .pending_asks
            .lock()
            .map_err(|_| ApiError::internal(pecan_core::CoreError::LockPoisoned))?;
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
    worker.send_raw(frame).await.map_err(|e| ApiError::bad_gateway(e.to_string()))?;
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
    if let Some(value) = body.value.as_deref() {
        frame["value"] = serde_json::Value::String(value.to_owned());
    }
    if let Some(confirmed) = body.confirmed {
        frame["confirmed"] = serde_json::Value::Bool(confirmed);
    }
    if let Some(cancelled) = body.cancelled {
        frame["cancelled"] = serde_json::Value::Bool(cancelled);
    }
    frame
}

/// Lists live dialog requests awaiting answers for one session.
async fn pending_asks(
    State(app): State<App>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let asks = app
        .pending_asks
        .lock()
        .map_err(|_| ApiError::internal(pecan_core::CoreError::LockPoisoned))?;
    let mut list: Vec<(&String, &super::snapshot::RecordedAsk)> =
        asks.get(&id).map(|per_session| per_session.iter().collect()).unwrap_or_default();
    list.sort_by_key(|(_, ask)| ask.recorded_at_ms);
    let asks: Vec<serde_json::Value> = list
        .into_iter()
        .map(|(request_id, ask)| {
            let mut value = serde_json::to_value(ask).unwrap_or(serde_json::Value::Null);
            value["id"] = serde_json::Value::String(request_id.clone());
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
    let output = tokio::process::Command::new("git")
        .args(["-C", &cwd, "rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .await;
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
    let Json(body) = body.map_err(|error| ApiError::bad_request(error.to_string()))?;
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
        store.settle(&id).map_err(ApiError::internal)?;
    }
    app.refresh().await.map_err(ApiError::internal)?;
    Ok(Json(result))
}

fn require_ship_token(app: &App, headers: &HeaderMap) -> Result<(), ApiError> {
    let supplied =
        headers.get("x-pecan-ship-token").and_then(|value| value.to_str().ok()).unwrap_or_default();
    if !constant_time_eq(supplied.as_bytes(), app.ship_token.as_bytes()) {
        return Err(ApiError::unauthorized("Ship capability is missing or invalid"));
    }
    Ok(())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter().zip(right).fold(0_u8, |difference, (a, b)| difference | (a ^ b)) == 0
}

#[cfg(test)]
mod ship_capability_tests {
    use super::constant_time_eq;

    #[test]
    fn ship_capability_rejects_missing_truncated_and_different_tokens() {
        assert!(constant_time_eq(b"same-token", b"same-token"));
        assert!(!constant_time_eq(b"", b"same-token"));
        assert!(!constant_time_eq(b"same", b"same-token"));
        assert!(!constant_time_eq(b"same-tokem", b"same-token"));
    }
}

const DIFF_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_DIFF_BYTES: usize = 4 * 1024 * 1024;
const MAX_UNTRACKED_FILES: usize = 40;
const MAX_GIT_ERROR_BYTES: u64 = 4 * 1024;

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
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(ApiError::bad_gateway(format!(
            "git diff failed: {}",
            detail.lines().next().unwrap_or("unknown git error")
        )));
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
    let stdout_limit = u64::try_from(max_stdout_bytes)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .saturating_add(1);
    let mut child = tokio::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| ApiError::bad_gateway(format!("could not run git: {error}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ApiError::internal("git stdout pipe was unavailable".to_owned()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ApiError::internal("git stderr pipe was unavailable".to_owned()))?;
    let read_output =
        async move {
            let mut stdout_bytes = Vec::new();
            stdout.take(stdout_limit).read_to_end(&mut stdout_bytes).await.map_err(|error| {
                ApiError::bad_gateway(format!("could not read git output: {error}"))
            })?;
            let mut stderr_bytes = Vec::new();
            stderr.take(MAX_GIT_ERROR_BYTES).read_to_end(&mut stderr_bytes).await.map_err(
                |error| ApiError::bad_gateway(format!("could not read git error: {error}")),
            )?;
            let status = child.wait().await.map_err(|error| {
                ApiError::bad_gateway(format!("could not wait for git: {error}"))
            })?;
            let truncated = stdout_bytes.len() > max_stdout_bytes;
            stdout_bytes.truncate(max_stdout_bytes);
            Ok(LimitedGitOutput { status, stdout: stdout_bytes, stderr: stderr_bytes, truncated })
        };
    tokio::time::timeout(DIFF_TIMEOUT, read_output)
        .await
        .map_err(|_elapsed| ApiError::bad_gateway("git diff timed out".to_owned()))?
}

fn git_failure(stderr: &[u8]) -> ApiError {
    let detail = String::from_utf8_lossy(stderr);
    ApiError::bad_gateway(format!(
        "git diff failed: {}",
        detail.lines().next().unwrap_or("unknown git error")
    ))
}

async fn git_command(cwd: &str, args: &[&str]) -> Result<std::process::Output, ApiError> {
    tokio::time::timeout(
        DIFF_TIMEOUT,
        tokio::process::Command::new("git").arg("-C").arg(cwd).args(args).output(),
    )
    .await
    .map_err(|_elapsed| ApiError::bad_gateway("git diff timed out".to_owned()))?
    .map_err(|error| ApiError::bad_gateway(format!("could not run git: {error}")))
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
    let Json(body) = body.map_err(|e| ApiError::bad_request(e.to_string()))?;
    if !body.cwd.starts_with('/') {
        return Err(ApiError::bad_request("cwd must be absolute".into()));
    }
    {
        let store = lock(&app)?;
        store.add_project(&body.cwd).map_err(ApiError::internal)?;
    }
    app.refresh().await.map_err(ApiError::internal)?;
    Ok(Json(serde_json::json!({"added": true})))
}

async fn remove_project(
    State(app): State<App>,
    body: Result<Json<ProjectBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    reject_global_route(&app)?;
    let Json(body) = body.map_err(|e| ApiError::bad_request(e.to_string()))?;
    {
        let store = lock(&app)?;
        store.remove_project(&body.cwd).map_err(ApiError::internal)?;
    }
    app.refresh().await.map_err(ApiError::internal)?;
    Ok(Json(serde_json::json!({"removed": true})))
}

async fn seed_init(State(app): State<App>) -> Result<Json<serde_json::Value>, ApiError> {
    reject_global_route(&app)?;
    let cwds: Vec<String> = {
        let snap = app.snapshot().await;
        snap.sessions
            .iter()
            .filter(|row| row.summary.kind == pecan_core::session::SessionKind::Normal)
            .map(|row| row.summary.cwd.clone())
            .collect()
    };
    {
        let store = lock(&app)?;
        store.ensure_seeded(cwds).map_err(ApiError::internal)?;
    }
    app.refresh().await.map_err(ApiError::internal)?;
    Ok(Json(serde_json::json!({"seeded": true})))
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
    body: Result<Json<NewSessionBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    reject_global_route(&app)?;
    let Json(body) = body.map_err(|e| ApiError::bad_request(e.to_string()))?;
    let dir = std::path::PathBuf::from(&body.cwd);
    if !body.cwd.starts_with('/') || !dir.is_dir() {
        return Err(ApiError::bad_request("cwd must be an existing directory".into()));
    }
    let worker = workers.spawn_new(&dir).await.map_err(|e| ApiError::bad_gateway(e.to_string()))?;
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
                return Err(ApiError::bad_gateway(error.to_string()));
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
    app.refresh().await.map_err(ApiError::internal)?;
    forward_worker_events(app, workers, &id, &worker);
    Ok(Json(serde_json::json!({ "id": id })))
}

#[derive(Deserialize, Debug)]
struct MessageBody {
    text: String,
    /// `send` when idle; `steer`/`queue` choose queueing behavior while streaming.
    mode: String,
    /// Optional image attachments (base64, no data: prefix).
    #[serde(default)]
    images: Vec<IncomingImage>,
}

async fn message(
    State((app, workers)): State<(App, Workers)>,
    axum::extract::Path(id): axum::extract::Path<String>,
    body: Result<Json<MessageBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Json(body) = body.map_err(|e| ApiError::bad_request(e.to_string()))?;
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
        workers.get_or_spawn(&id, &path).await.map_err(|e| ApiError::bad_gateway(e.to_string()))?
    } else {
        workers.get(&id).await.ok_or_else(|| ApiError::not_found("no such session"))?
    };
    forward_worker_events(app.clone(), workers.clone(), &id, &worker);

    let state = worker.get_state().await.map_err(|e| ApiError::bad_gateway(e.to_string()))?;
    let streaming = state.get("isStreaming").and_then(serde_json::Value::as_bool) == Some(true);
    let cmd = match (streaming, body.mode.as_str()) {
        (true, "send") => return Err(ApiError::conflict("agent is streaming; use steer or queue")),
        (false, m) if m == "steer" || m == "queue" => prompt_cmd("prompt", &body),
        (_, "send") => prompt_cmd("prompt", &body),
        (_, "steer") => prompt_cmd("steer", &body),
        (_, "queue") => prompt_cmd("follow_up", &body),
        (_, other) => return Err(ApiError::bad_request(format!("unknown mode {other}"))),
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
    if !body.images.is_empty() {
        cmd["images"] = serde_json::Value::Array(
            body.images
                .iter()
                .map(|image| {
                    serde_json::json!({
                        "type": "image",
                        "data": image.data,
                        "mimeType": image.mime_type,
                    })
                })
                .collect(),
        );
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
        Err(error) => Err(ApiError::bad_gateway(error.to_string())),
    }
}

async fn agent_attach(
    State((app, workers)): State<(App, Workers)>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let worker = require_worker(&app, &workers, &id).await?;
    forward_worker_events(app.clone(), workers.clone(), &id, &worker);
    agent_snapshot(worker, &id).await
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
                    let _ =
                        app.events.send(ServerEvent::AgentEvent { id: session_id.clone(), event });
                    if settled {
                        let _ =
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
        let _ = app.events.send(ServerEvent::AgentEvent {
            id: session_id.clone(),
            event: serde_json::json!({
                "type": "extension_ui_request",
                "method": "setWidget",
                "widgetKey": "pi-subagents/activity/v1"
            }),
        });
        let _ = workers.remove(&session_id).await;
    });
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
    let Json(body) = body.map_err(|e| ApiError::bad_request(e.to_string()))?;
    let worker = require_worker(&app, &workers, &id).await?;
    worker
        .command(serde_json::json!({
            "type": "set_model",
            "provider": body.provider,
            "modelId": body.model_id,
        }))
        .await
        .map_err(|e| ApiError::bad_gateway(e.to_string()))?;
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
    let Json(body) = body.map_err(|e| ApiError::bad_request(e.to_string()))?;
    let worker = require_worker(&app, &workers, &id).await?;
    worker
        .command(serde_json::json!({"type": "set_thinking_level", "level": body.level}))
        .await
        .map_err(|e| ApiError::bad_gateway(e.to_string()))?;
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
        return workers
            .get_or_spawn(id, &row.summary.path)
            .await
            .map_err(|e| ApiError::bad_gateway(e.to_string()));
    }
    workers.get(id).await.ok_or_else(|| ApiError::not_found("no such session"))
}

/// Collects model/thinking/context info from a live worker.
async fn agent_snapshot(
    worker: std::sync::Arc<super::worker::WorkerHandle>,
    id: &str,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state = worker.get_state().await.map_err(|e| ApiError::bad_gateway(e.to_string()))?;
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
    })))
}

// -------------------------------------------------------------------- sse ----

async fn events(
    State(app): State<App>,
) -> Sse<impl Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    let rx = app.events.subscribe();
    let stream = BroadcastStream::new(rx).map(|item| match item {
        Ok(event) => Ok(axum::response::sse::Event::default()
            .event(match event {
                ServerEvent::IndexChanged => "index-changed",
                ServerEvent::ThreadChanged { .. } | ServerEvent::AgentEvent { .. } => "agent",
            })
            .data(serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_owned()))),
        Err(_) => Ok(axum::response::sse::Event::default().comment("lagged")),
    });
    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

// ------------------------------------------------------------------ misc ----

fn lock(app: &App) -> Result<std::sync::MutexGuard<'_, pecan_core::store::StateStore>, ApiError> {
    app.store.lock().map_err(|_| ApiError::internal(pecan_core::CoreError::LockPoisoned))
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
