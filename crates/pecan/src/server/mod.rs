//! The local web server: embedded SPA + JSON API + SSE + RPC worker pool.

mod api;
mod api_errors;
mod assets;
mod auth;
mod event_log;
mod folders;
mod git;
mod health;
mod idempotency;
mod model_favorites;
mod push;
mod ship;
mod snapshot;
mod title;
mod watcher;
mod worker;

use std::sync::Arc;

use axum::Router;
use axum::routing::get;
use pecan_core::PiPaths;
use pecan_core::scan::ScanCache;
use pecan_core::session::{SessionKind, SessionSummary};

use self::snapshot::{App, IndexSnapshot, SessionScope};
use self::worker::Workers;
use crate::cli::open_store;

/// Default loopback port.
const DEFAULT_PORT: u16 = 7614;

/// Runs `pecan serve`: builds state, starts the watcher, serves forever.
///
/// # Errors
/// Returns a message when startup fails (bind error, missing dirs, etc.).
pub(crate) fn run(args: &[String]) -> Result<(), crate::cli::CliError> {
    let options = parse_serve_options(args)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| crate::cli::startup(&format!("tokio runtime: {e}")))?;
    runtime.block_on(async move {
        serve(
            options.host,
            options.port,
            options.open_browser,
            options.session_id,
            options.session_cwd,
        )
        .await
    })
}

#[derive(Debug, PartialEq)]
struct ServeOptions {
    port: u16,
    host: [u8; 4],
    open_browser: bool,
    session_id: Option<String>,
    session_cwd: Option<String>,
}

fn parse_serve_options(args: &[String]) -> Result<ServeOptions, crate::cli::CliError> {
    let mut options = ServeOptions {
        port: DEFAULT_PORT,
        host: [127, 0, 0, 1],
        open_browser: true,
        session_id: None,
        session_cwd: None,
    };
    let mut port_seen = false;
    let mut host_seen = false;
    let mut no_open_seen = false;
    let mut i = 0;
    while i < args.len() {
        let Some(current) = args.get(i) else { break };
        match current.as_str() {
            "--port" => {
                if port_seen {
                    return Err(crate::cli::usage("serve: duplicate --port"));
                }
                let value = args
                    .get(i + 1)
                    .filter(|value| !value.starts_with("--"))
                    .ok_or_else(|| crate::cli::usage("serve: --port expects a number"))?;
                options.port = value
                    .parse()
                    .map_err(|e| crate::cli::usage(&format!("--port expects a number: {e}")))?;
                port_seen = true;
                i += 1;
            }
            "--host" => {
                if host_seen {
                    return Err(crate::cli::usage("serve: duplicate --host"));
                }
                let raw = args
                    .get(i + 1)
                    .filter(|value| !value.starts_with("--"))
                    .ok_or_else(|| crate::cli::usage("serve: --host expects an IPv4 address"))?;
                i += 1;
                options.host = parse_host(raw)
                    .ok_or_else(|| crate::cli::usage("--host expects an IPv4 address"))?;
                host_seen = true;
            }
            "--session" => {
                if options.session_id.is_some() {
                    return Err(crate::cli::usage("serve: duplicate --session"));
                }
                let value = args
                    .get(i + 1)
                    .filter(|value| !value.starts_with("--"))
                    .ok_or_else(|| crate::cli::usage("serve: --session expects a session id"))?;
                options.session_id = Some(value.clone());
                i += 1;
            }
            "--cwd" => {
                if options.session_cwd.is_some() {
                    return Err(crate::cli::usage("serve: duplicate --cwd"));
                }
                let value = args
                    .get(i + 1)
                    .filter(|value| !value.starts_with("--"))
                    .ok_or_else(|| crate::cli::usage("serve: --cwd expects an absolute path"))?;
                if !std::path::Path::new(value).is_absolute() {
                    return Err(crate::cli::usage("serve: --cwd expects an absolute path"));
                }
                options.session_cwd = Some(value.clone());
                i += 1;
            }
            "--no-open" => {
                if no_open_seen {
                    return Err(crate::cli::usage("serve: duplicate --no-open"));
                }
                options.open_browser = false;
                no_open_seen = true;
            }
            other => return Err(crate::cli::usage(&format!("serve: unknown flag {other}"))),
        }
        i += 1;
    }
    match (options.session_id.as_deref(), options.session_cwd.as_deref()) {
        (Some("latest"), Some(_)) | (None, None) => {}
        (Some("latest"), None) => {
            return Err(crate::cli::usage("serve: --session latest requires --cwd <absolute>"));
        }
        (Some(_), None) => {}
        (Some(_), Some(_)) => {
            return Err(crate::cli::usage("serve: --cwd is valid only with --session latest"));
        }
        (None, Some(_)) => {
            return Err(crate::cli::usage("serve: --cwd is valid only with --session latest"));
        }
    }
    Ok(options)
}

/// Parses a dotted-quad IPv4 address for `--host`.
fn parse_host(raw: &str) -> Option<[u8; 4]> {
    if raw == "localhost" {
        return Some([127, 0, 0, 1]);
    }
    let octets: Vec<u8> =
        raw.split('.').map(|part| part.parse::<u8>().ok()).collect::<Option<_>>()?;
    (octets.len() == 4).then_some(octets.try_into().ok()?)
}

async fn serve(
    host: [u8; 4],
    port: u16,
    open_browser: bool,
    session_id: Option<String>,
    session_cwd: Option<String>,
) -> Result<(), crate::cli::CliError> {
    let ship_token = generate_ship_token()?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pecan=info".into()),
        )
        .init();

    let paths = PiPaths::detect().map_err(|e| crate::cli::startup(&e.to_string()))?;
    let session_scope = if session_id.is_some() {
        let mut cache = ScanCache::new();
        let sessions = cache
            .refresh(&paths.sessions_dir())
            .map_err(|error| crate::cli::startup(&error.to_string()))?;
        let id = resolve_session_id(&sessions, session_id.as_deref(), session_cwd.as_deref())?
            .ok_or_else(|| crate::cli::usage("serve: missing session selector"))?;
        Some(SessionScope::new(id))
    } else {
        None
    };
    let store = Arc::new(std::sync::Mutex::new(
        open_store(&paths).map_err(|e| crate::cli::startup(&e.to_string()))?,
    ));
    let auth = auth::Auth::load(&paths).map_err(|e| crate::cli::startup(&e.to_string()))?;
    let push = push::Push::load(&paths).map_err(|e| crate::cli::startup(&e.to_string()))?;
    let events = event_log::EventLog::new();
    let app = App {
        paths: paths.clone(),
        store,
        snapshot: Arc::new(tokio::sync::RwLock::new(Arc::new(IndexSnapshot {
            sessions: Arc::new(Vec::new()),
            tasks: std::collections::HashMap::new(),
        }))),
        events,
        scan_cache: Arc::new(std::sync::Mutex::new(ScanCache::new())),
        thread_cache: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        pending_asks: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        title_generations: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        ship_lock: Arc::new(tokio::sync::Mutex::new(())),
        ship_token: Arc::from(ship_token.as_str()),
        session_scope,
        idempotency: idempotency::Idempotency::default(),
        health: health::HealthCache::default(),
        auth,
        push,
    };
    app.refresh().await.map_err(|e| crate::cli::startup(&e.to_string()))?;

    // Projects are tracked only when the user explicitly adds them; the
    // index stays global but the sidebar starts empty.
    //
    // `pecan seed-init` (POST /api/state/seed-init) remains available as an
    // explicit opt-in bulk import.

    let workers = Workers::new();
    let project_roots = watched_project_roots(&app)
        .await
        .map_err(|error| crate::cli::startup(&error.to_string()))?;
    let _watcher = watcher::spawn(&paths, Arc::new(app.clone()), &project_roots)
        .map_err(|e| crate::cli::startup(&format!("watcher: {e}")))?;

    let router = Router::new()
        .route("/", get(assets::serve))
        .fallback(get(assets::serve))
        .nest("/api", api::router(app.clone(), workers));
    let addr = std::net::SocketAddr::from((host, port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| crate::cli::startup(&format!("bind {addr}: {e}")))?;
    let bound_addr = listener
        .local_addr()
        .map_err(|e| crate::cli::startup(&format!("listener address: {e}")))?;
    let url = format!("http://{bound_addr}");
    println!("pecan serving {url}");
    let ship_url = format!("{url}/?ship-token={ship_token}");
    println!("Ship-enabled URL: {ship_url}");
    println!("Pair a phone or browser: pecan pair");
    if open_browser
        && cfg!(target_os = "macos")
        && let Err(error) = open_paired(&app, &ship_url)
    {
        tracing::warn!(%error, "failed to open browser");
    }

    tokio::select! {
        result = axum::serve(listener, router).into_future() => {
            result.map_err(|e| crate::cli::startup(&format!("serve: {e}")))
        }
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(|e| crate::cli::startup(&format!("shutdown signal: {e}")))?;
            tracing::info!("shutting down");
            Ok(())
        }
    }
}

/// Opens this machine's browser already paired: the URL carries a fresh
/// one-time code the page redeems on load.
fn open_paired(app: &App, ship_url: &str) -> Result<(), String> {
    let code = app.auth.mint_code(std::time::Instant::now()).map_err(|error| error.to_string())?;
    let url = format!("{ship_url}&pair={code}");
    std::process::Command::new("open").arg(url).spawn().map(drop).map_err(|error| error.to_string())
}

fn generate_ship_token() -> Result<String, crate::cli::CliError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| crate::cli::startup(&format!("generate Ship token: {error}")))?;
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        write!(&mut token, "{byte:02x}")
            .map_err(|error| crate::cli::startup(&format!("encode Ship token: {error}")))?;
    }
    Ok(token)
}

async fn watched_project_roots(app: &App) -> pecan_core::Result<Vec<std::path::PathBuf>> {
    if app.session_scope.is_some() {
        let snapshot = app.snapshot().await;
        let mut roots: Vec<std::path::PathBuf> = snapshot
            .sessions
            .iter()
            .map(|row| std::path::PathBuf::from(&row.summary.cwd))
            .collect();
        roots.sort_unstable();
        roots.dedup();
        return Ok(roots);
    }
    let store = app.store.lock().map_err(|_poisoned| pecan_core::CoreError::LockPoisoned)?;
    Ok(store
        .projects()?
        .into_iter()
        .filter(|(_, preference)| preference.added)
        .map(|(cwd, _)| std::path::PathBuf::from(cwd))
        .collect())
}

fn resolve_session_id(
    sessions: &[SessionSummary],
    session_id: Option<&str>,
    session_cwd: Option<&str>,
) -> Result<Option<String>, crate::cli::CliError> {
    let Some(requested) = session_id else {
        return Ok(None);
    };
    if requested == "latest" {
        let cwd = session_cwd.ok_or_else(|| {
            crate::cli::usage("serve: --session latest requires --cwd <absolute>")
        })?;
        return sessions
            .iter()
            .find(|summary| summary.kind == SessionKind::Normal && summary.cwd == cwd)
            .map(|summary| Some(summary.id.clone()))
            .ok_or_else(|| {
                crate::cli::CliError::Refused(format!("no normal sessions found for cwd {cwd}"))
            });
    }
    let summary = sessions
        .iter()
        .find(|summary| summary.id == requested)
        .ok_or_else(|| crate::cli::CliError::Refused(format!("no session with id {requested}")))?;
    if summary.kind != SessionKind::Normal {
        return Err(crate::cli::CliError::Refused(format!(
            "session {requested} is a subagent; --session requires a main session"
        )));
    }
    Ok(Some(summary.id.clone()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use jiff::Timestamp;
    use pecan_core::session::{SessionKind, SessionSummary};

    use super::{DEFAULT_PORT, ServeOptions, parse_serve_options, resolve_session_id};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn parses_embedded_session_and_ephemeral_port() {
        let parsed =
            parse_serve_options(&args(&["--session", "session-1", "--port", "0", "--no-open"]));
        assert!(matches!(
            parsed,
            Ok(ServeOptions {
                port: 0,
                host: [127, 0, 0, 1],
                open_browser: false,
                session_id: Some(ref id),
                session_cwd: None,
            })
            if id == "session-1"
        ));
    }

    #[test]
    fn normal_mode_defaults_are_unchanged() {
        assert!(matches!(
            parse_serve_options(&[]),
            Ok(ServeOptions {
                port: DEFAULT_PORT,
                host: [127, 0, 0, 1],
                open_browser: true,
                session_id: None,
                session_cwd: None,
            })
        ));
    }

    #[test]
    fn rejects_missing_and_duplicate_session_flags() {
        assert!(parse_serve_options(&args(&["--session"])).is_err());
        assert!(parse_serve_options(&args(&["--session", "one", "--session", "two",])).is_err());
    }

    #[test]
    fn parses_latest_with_absolute_cwd() {
        let parsed = parse_serve_options(&args(&["--session", "latest", "--cwd", "/work/project"]));
        assert!(matches!(
            parsed,
            Ok(ServeOptions {
                session_id: Some(ref id),
                session_cwd: Some(ref cwd),
                ..
            }) if id == "latest" && cwd == "/work/project"
        ));
    }

    #[test]
    fn rejects_invalid_cwd_combinations() {
        assert!(parse_serve_options(&args(&["--cwd", "/work/project"])).is_err());
        assert!(parse_serve_options(&args(&["--session", "latest"])).is_err());
        assert!(parse_serve_options(&args(&["--session", "latest", "--cwd"])).is_err());
        assert!(parse_serve_options(&args(&["--unknown"])).is_err());
        assert!(
            parse_serve_options(&args(&["--session", "session-1", "--cwd", "/work/project",]))
                .is_err()
        );
        assert!(
            parse_serve_options(&args(&["--session", "latest", "--cwd", "relative/project",]))
                .is_err()
        );
        assert!(
            parse_serve_options(&args(&["--session", "latest", "--cwd", "/one", "--cwd", "/two",]))
                .is_err()
        );
    }

    #[test]
    fn latest_uses_first_normal_exact_cwd_in_scanner_order() {
        let sessions = vec![
            summary("new-subagent", "/work/project", SessionKind::Subagent),
            summary("new-normal", "/work/project", SessionKind::Normal),
            summary("other", "/work/other", SessionKind::Normal),
            summary("old-normal", "/work/project", SessionKind::Normal),
        ];

        let selected = resolve_session_id(&sessions, Some("latest"), Some("/work/project"));

        assert!(matches!(selected, Ok(Some(ref id)) if id == "new-normal"));
    }

    #[test]
    fn latest_refuses_when_exact_cwd_has_no_normal_session() {
        let sessions = vec![summary("near-match", "/work/project-child", SessionKind::Normal)];

        let selected = resolve_session_id(&sessions, Some("latest"), Some("/work/project"));

        assert!(matches!(selected, Err(crate::cli::CliError::Refused(ref message)) if
            message == "no normal sessions found for cwd /work/project"));
    }

    #[test]
    fn exact_id_resolution_remains_supported_and_rejects_subagents() {
        let sessions = vec![
            summary("child", "/work/project", SessionKind::Subagent),
            summary("main", "/work/project", SessionKind::Normal),
        ];

        let main = resolve_session_id(&sessions, Some("main"), None);
        let child = resolve_session_id(&sessions, Some("child"), None);

        assert!(matches!(main, Ok(Some(ref id)) if id == "main"));
        assert!(matches!(child, Err(crate::cli::CliError::Refused(_))));
    }

    fn summary(id: &str, cwd: &str, kind: SessionKind) -> SessionSummary {
        SessionSummary {
            id: id.to_owned(),
            path: PathBuf::from(format!("/{id}.jsonl")),
            cwd: cwd.to_owned(),
            opened_at: Timestamp::UNIX_EPOCH,
            last_activity: Timestamp::UNIX_EPOCH,
            bytes: 1,
            provider: None,
            model: None,
            preview: None,
            title: None,
            kind,
            agent_name: None,
        }
    }
}
