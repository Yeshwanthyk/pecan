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
        .route("/session/{id}", get(thread_view))
        .route("/session/{id}/git", get(session_git))
        .route("/session/{id}/diff", get(session_diff))
        .route("/session/{id}/ship", get(ship_plan).post(ship_execute))
        .route("/session/{id}/settle", post(settle).delete(reopen))
        .route("/session/{id}/pin", post(pin).delete(unpin))
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
    State(app): State<App>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = app.snapshot().await;
    let row = find_row(&snap, &id)?;
    let view = app.thread_view(row.summary.path.clone()).await.map_err(ApiError::internal)?;
    Ok(Json(serde_json::json!({
        "summary": row.summary,
        "settled": row.settled,
        "waitingAskuser": row.waiting_askuser,
        "omitted": view.omitted,
        "entries": view.entries,
        "tasks": snap.tasks.get(&id).cloned().unwrap_or_default(),
        "workflows": snap.workflows.get(&id).cloned().unwrap_or_default(),
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
    let mut frame = serde_json::json!({
        "type": "extension_ui_response",
        "id": body.request_id,
    });
    if let Some(value) = body.value {
        frame["value"] = serde_json::Value::String(value);
    }
    if let Some(confirmed) = body.confirmed {
        frame["confirmed"] = serde_json::Value::Bool(confirmed);
    }
    if let Some(cancelled) = body.cancelled {
        frame["cancelled"] = serde_json::Value::Bool(cancelled);
    }
    worker.send_raw(frame).await.map_err(|e| ApiError::bad_gateway(e.to_string()))?;
    Ok(Json(serde_json::json!({ "responded": true })))
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
                if let Some(found) = state
                    .get("data")
                    .and_then(|data| data.get("sessionId"))
                    .and_then(serde_json::Value::as_str)
                {
                    id = Some(found.to_owned());
                    break;
                }
            }
            Err(error) if attempt > 5 => {
                return Err(ApiError::bad_gateway(error.to_string()));
            }
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let Some(id) = id else {
        tracing::warn!("new session: worker never reported a session id (30s window)");
        return Err(ApiError::bad_gateway(
            "worker reported no session id within startup window (30s)".into(),
        ));
    };
    workers.insert(id.clone(), worker.clone()).await;
    forward_worker_events(app, workers, &id, &worker);
    Ok(Json(serde_json::json!({ "id": id })))
}

#[derive(Deserialize, Debug)]
struct MessageBody {
    text: String,
    /// `send` when idle; `steer`/`queue` choose queueing behavior while streaming.
    mode: String,
}

async fn message(
    State((app, workers)): State<(App, Workers)>,
    axum::extract::Path(id): axum::extract::Path<String>,
    body: Result<Json<MessageBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Json(body) = body.map_err(|e| ApiError::bad_request(e.to_string()))?;
    if body.text.trim().is_empty() {
        return Err(ApiError::bad_request("empty message".into()));
    }
    let path = {
        let snap = app.snapshot().await;
        find_row(&snap, &id)?.summary.path.clone()
    };
    let worker =
        workers.get_or_spawn(&id, &path).await.map_err(|e| ApiError::bad_gateway(e.to_string()))?;
    forward_worker_events(app.clone(), workers.clone(), &id, &worker);

    let state = worker.get_state().await.map_err(|e| ApiError::bad_gateway(e.to_string()))?;
    let streaming = state.get("isStreaming").and_then(serde_json::Value::as_bool) == Some(true);
    let cmd = match (streaming, body.mode.as_str()) {
        (true, "send") => return Err(ApiError::conflict("agent is streaming; use steer or queue")),
        (false, m) if m == "steer" || m == "queue" => {
            serde_json::json!({"type": "prompt", "message": body.text})
        }
        (_, "send") => serde_json::json!({"type": "prompt", "message": body.text}),
        (_, "steer") => serde_json::json!({"type": "steer", "message": body.text}),
        (_, "queue") => serde_json::json!({"type": "follow_up", "message": body.text}),
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
    let path = {
        let snap = app.snapshot().await;
        find_row(&snap, &id)?.summary.path.clone()
    };
    let worker =
        workers.get_or_spawn(&id, &path).await.map_err(|e| ApiError::bad_gateway(e.to_string()))?;
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
    let row = find_row(&snap, id)?;
    workers
        .get_or_spawn(id, &row.summary.path)
        .await
        .map_err(|e| ApiError::bad_gateway(e.to_string()))
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
