//! CLI command implementations: the human-driven test harness over pecan-core.
//!
//! Every command honors `PECAN_AGENT_DIR` so tests can point at a synthetic
//! tree (see [`seed`]) instead of touching the real `~/.pi`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use pecan_core::scan::ScanCache;
use pecan_core::session::{SessionKind, SessionSummary};
use pecan_core::store::StateStore;
use pecan_core::tasks::{self, TaskList};
use pecan_core::thread::{self, ThreadEntry};
use pecan_core::workflows::{self, WorkflowRun};
use pecan_core::{CoreError, PiPaths};

/// Errors surfaced by the CLI layer.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CliError {
    /// Bad invocation.
    #[error("usage: {0}")]
    Usage(String),
    /// A domain operation failed.
    #[error(transparent)]
    Core(#[from] CoreError),
    /// The command refused to run for safety reasons.
    #[error("refused: {0}")]
    Refused(String),
}

/// Dispatches one CLI invocation.
pub(crate) fn run(args: &[String]) -> Result<(), CliError> {
    match args.split_first() {
        Some((cmd, rest)) if cmd == "seed" => seed(Path::new(arg(rest, "<dir>")?)),
        Some((cmd, rest)) if cmd == "index" => index(rest),
        Some((cmd, rest)) if cmd == "add" => add_remove(rest, true),
        Some((cmd, rest)) if cmd == "remove" => add_remove(rest, false),
        Some((cmd, rest)) if cmd == "settle" => set_unsettle(rest, true),
        Some((cmd, rest)) if cmd == "reopen" => set_unsettle(rest, false),
        Some((cmd, rest)) if cmd == "serve" => crate::server::run(rest),
        Some((cmd, _rest)) if cmd == "seed-init" => seed_init(),
        Some((cmd, rest)) if cmd == "thread" => show_thread(arg(rest, "<session-id>")?),
        _ => Err(CliError::Usage(
            "expected one of: seed <dir> | seed-init | index [--all] [--json] [--project <cwd>] \
             | add <cwd> | remove <cwd> | settle <id> | reopen <id> | thread <id> \
             | serve [--host <ipv4>] [--port <port>] [--no-open] \
               [--session <main-session-id>|latest [--cwd <absolute>]]"
                .to_owned(),
        )),
    }
}

/// Builds a usage error from a message fragment.
pub(crate) fn usage(message: &str) -> CliError {
    CliError::Usage(message.to_owned())
}

fn arg<'a>(rest: &'a [String], what: &str) -> Result<&'a String, CliError> {
    rest.first().ok_or_else(|| CliError::Usage(format!("missing argument {what}")))
}

fn paths() -> Result<PiPaths, CliError> {
    Ok(PiPaths::detect()?)
}

/// Applies first-run project seeding against whatever is on disk now.
fn seed_init() -> Result<(), CliError> {
    let paths = paths()?;
    let sessions = scan_all(&paths)?;
    let store = open_store(&paths)?;
    let before = store.projects()?.len();
    store.ensure_seeded(
        sessions
            .iter()
            .filter(|s| s.kind == SessionKind::Normal)
            .map(|s| s.cwd.clone())
            .collect::<Vec<_>>(),
    )?;
    let added_now = store.projects()?.len() - before;
    println!("seeded allowlist: {added_now} projects added");
    Ok(())
}

fn scan_all(paths: &PiPaths) -> Result<Vec<SessionSummary>, CliError> {
    let mut cache = ScanCache::new();
    Ok(cache.refresh(&paths.sessions_dir())?)
}

// ---------------------------------------------------------------- index ----

fn index(rest: &[String]) -> Result<(), CliError> {
    let json = rest.iter().any(|a| a == "--json");
    let show_all = rest.iter().any(|a| a == "--all");
    let project = rest.iter().position(|a| a == "--project").and_then(|i| rest.get(i + 1));

    let paths = paths()?;
    let store = open_store(&paths)?;
    let mut sessions = scan_all(&paths)?;
    let seeded = store.is_seeded()?;
    // Once seeded, the default view is added-only; --all lifts the filter.
    let filtered_by_allowlist = seeded && !show_all;
    sessions.retain(|s| project.is_none_or(|p| s.cwd == *p));
    let tasks_by_session = tasks::load_all(&paths.tasks_dir());
    let workflows_by_session = workflows::load_all(&paths.workflows_dir());
    let settled_map = store.settled()?;

    if json {
        let payload = serde_json::json!({
            "agentDir": paths.agent_dir(),
            "sessions": sessions,
            "settled": settled_map,
            "projects": store.projects()?,
        });
        println!("{payload:#}");
        return Ok(());
    }

    println!("agent dir : {}", paths.agent_dir().display());
    let mut added = 0_usize;
    for (cwd, pref) in store.projects()? {
        if pref.added {
            added += 1;
            println!("project   : {} ({})", display_name(&cwd), cwd);
        }
    }
    if added == 0 {
        println!("project   : (none added yet)");
    }
    if !seeded {
        println!("(state not seeded yet — run `pecan seed-init` or start the server)");
    }

    let visible: Vec<SessionSummary> = if filtered_by_allowlist {
        let mut keep: Vec<SessionSummary> = Vec::new();
        for s in &sessions {
            if store.is_added(&s.cwd)? {
                keep.push(s.clone());
            }
        }
        keep
    } else {
        sessions.clone()
    };
    let is_settled = |id: &str| settled_map.contains_key(id);
    let mut active: Vec<&SessionSummary> = visible.iter().filter(|s| !is_settled(&s.id)).collect();
    let mut settled: Vec<&SessionSummary> = visible.iter().filter(|s| is_settled(&s.id)).collect();

    println!("\nACTIVE ({})", active.len());
    print_rows(&mut active, &settled_map, &tasks_by_session, &workflows_by_session);
    println!("\n-- settled -- ({})", settled.len());
    print_rows(&mut settled, &settled_map, &tasks_by_session, &workflows_by_session);
    Ok(())
}

fn print_rows(
    rows: &mut [&SessionSummary],
    settled_map: &HashMap<String, i64>,
    tasks_by_session: &HashMap<String, Vec<TaskList>>,
    workflows_by_session: &HashMap<String, Vec<WorkflowRun>>,
) {
    for s in rows.iter() {
        let mut tags: Vec<String> = Vec::new();
        if s.kind == SessionKind::Subagent {
            tags.push("subagent".to_owned());
        }
        if tasks_by_session.contains_key(&s.id) {
            tags.push("tasks".to_owned());
        }
        if workflows_by_session.contains_key(&s.id) {
            tags.push("workflows".to_owned());
        }
        if settled_map.contains_key(&s.id) {
            tags.push("settled".to_owned());
        }
        println!(
            "  {:<38} {}  {:<14} {:<44} {}",
            truncate(&s.id, 38),
            s.opened_at.strftime("%Y-%m-%d %H:%M"),
            display_name(&s.cwd),
            s.preview.as_deref().map_or_else(|| s.path.display().to_string(), str::to_owned),
            tags.join(","),
        );
    }
}

// ------------------------------------------------------- add/remove/settle --

fn with_store(f: impl FnOnce(&StateStore) -> pecan_core::Result<()>) -> Result<(), CliError> {
    let paths = paths()?;
    let store = open_store(&paths)?;
    f(&store)?;
    Ok(())
}

/// Opens the SQLite state store, importing any legacy `state.json` once.
pub(crate) fn open_store(paths: &PiPaths) -> pecan_core::Result<StateStore> {
    let store = StateStore::open(&paths.state_db())?;
    let _ = store.import_legacy_json(&paths.state_file());
    Ok(store)
}

fn add_remove(rest: &[String], add: bool) -> Result<(), CliError> {
    let cwd = arg(rest, "<cwd>")?;
    with_store(|store| if add { store.add_project(cwd) } else { store.remove_project(cwd) })?;
    println!("{} {}", if add { "added" } else { "removed" }, display_name(cwd));
    Ok(())
}

fn set_unsettle(rest: &[String], settle: bool) -> Result<(), CliError> {
    let id = arg(rest, "<session-id>")?;
    with_store(|store| if settle { store.settle(id) } else { store.reopen(id) })?;
    println!("{} {}", if settle { "settled" } else { "reopened" }, id);
    Ok(())
}

// ---------------------------------------------------------------- thread ----

fn show_thread(session_id: &String) -> Result<(), CliError> {
    let paths = paths()?;
    let sessions = scan_all(&paths)?;
    let found = sessions.iter().find(|s| &s.id == session_id);
    let Some(summary) = found else {
        return Err(CliError::Refused(format!("no session with id {session_id}")));
    };
    let view = thread::parse_thread(&summary.path)?;
    if view.omitted > 0 {
        println!("… {} older entries omitted", view.omitted);
    }
    for dated in &view.entries {
        let clock =
            dated.ts.map_or_else(|| "--:--:--".to_owned(), |t| t.strftime("%H:%M:%S").to_string());
        match &dated.entry {
            ThreadEntry::User { text, truncated } => {
                println!("[{clock}] USER   {}{}", first_line(text), ellipsis_flag(*truncated))
            }
            ThreadEntry::Assistant { text, truncated, thinking, tools, model } => {
                println!(
                    "[{clock}] AI({}) {}{}",
                    model.as_deref().unwrap_or("?"),
                    first_line(text),
                    ellipsis_flag(*truncated)
                );
                if thinking.is_some() {
                    println!("       thinking: (collapsed)");
                }
                for tool in tools {
                    println!("       tool {}: {}", tool.name, tool.args_preview);
                }
            }
            ThreadEntry::AskUser { questions, answer } => {
                let n = questions
                    .get("questions")
                    .and_then(serde_json::Value::as_array)
                    .map_or(0, Vec::len);
                println!(
                    "[{clock}] ASK    {n} question(s) — {}",
                    answer.as_ref().map_or("UNANSWERED", |_| "answered")
                );
            }
            ThreadEntry::ToolError { text, truncated } => {
                println!("[{clock}] ERROR  {}{}", first_line(text), ellipsis_flag(*truncated))
            }
        }
    }
    if view.waiting_askuser {
        println!(">> waiting for ask_user answer");
    }
    Ok(())
}

// ------------------------------------------------------------------ seed ----

/// Fabricates a complete synthetic pi tree under `dir`.
///
/// Creates `<dir>/agent/sessions/<project dirs>/*.jsonl`, `<dir>/tasks`,
/// and `<dir>/agent/workflows`. Refuses to touch anything that does not look
/// like the dedicated test ground.
fn seed(dir: &Path) -> Result<(), CliError> {
    let looks_safe = dir.to_string_lossy().contains("test-ground");
    let empty = !dir.exists() || std::fs::read_dir(dir).is_ok_and(|entries| entries.count() == 0);
    if !looks_safe && !empty {
        return Err(CliError::Refused(format!(
            "{} does not look like a test ground; pass an empty dir or one containing 'test-ground'",
            dir.display()
        )));
    }

    let now = Timestamp::now();
    let agent_dir = dir.join("agent");
    let sessions_root = agent_dir.join("sessions");
    let tasks_root = dir.join("tasks");
    let workflows_root = agent_dir.join("workflows");
    let _ = std::fs::remove_dir_all(&sessions_root);
    let _ = std::fs::remove_dir_all(&tasks_root);
    let _ = std::fs::remove_dir_all(&workflows_root);
    std::fs::create_dir_all(&sessions_root)
        .map_err(|source| CoreError::Io { path: sessions_root.clone(), source })?;

    let base = dir.join("repos");
    let projects = [
        ("pican", base.join("pican")),
        ("webshop", base.join("webshop")),
        ("kernel-notes", base.join("kernel-notes")),
    ];

    struct Spec {
        uuid: String,
        project_dir: PathBuf,
        cwd: String,
        title: String,
        hours_ago: i64,
        body: Body,
    }
    enum Body {
        Plain,
        WithBash,
        WithOpenAskUser,
        Scout,
    }

    let specs = vec![
        Spec {
            uuid: "00000000-0000-4000-8000-000000000001".to_owned(),
            project_dir: projects[0].1.clone(),
            cwd: projects[0].1.display().to_string(),
            title: "Fix flaky retry queue test in worker pool".to_owned(),
            hours_ago: 2,
            body: Body::WithBash,
        },
        Spec {
            uuid: "00000000-0000-4000-8000-000000000002".to_owned(),
            project_dir: projects[0].1.clone(),
            cwd: projects[0].1.display().to_string(),
            title: "Add tailscale serve detection".to_owned(),
            hours_ago: 120,
            body: Body::Plain,
        },
        Spec {
            uuid: "00000000-0000-4000-8000-000000000003".to_owned(),
            project_dir: projects[1].1.clone(),
            cwd: projects[1].1.display().to_string(),
            title: "Design checkout sidebar states".to_owned(),
            hours_ago: 26,
            body: Body::WithOpenAskUser,
        },
        Spec {
            uuid: "00000000-0000-4000-8000-000000000004".to_owned(),
            project_dir: projects[1].1.clone(),
            cwd: projects[1].1.display().to_string(),
            title: "Migrate product images to CDN".to_owned(),
            hours_ago: 200,
            body: Body::Plain,
        },
        Spec {
            uuid: "00000000-0000-4000-8000-000000000005".to_owned(),
            project_dir: projects[2].1.clone(),
            cwd: projects[2].1.display().to_string(),
            title: "Notes on io_uring batching".to_owned(),
            hours_ago: 720,
            body: Body::Plain,
        },
        Spec {
            uuid: "00000000-0000-4000-8000-000000000006".to_owned(),
            project_dir: sessions_root.join("--tmp-pi-subagent-codex-scout-src--"),
            cwd: "/tmp/pi-subagent-codex-scout/src".to_owned(),
            title: "scout: survey rpc docs".to_owned(),
            hours_ago: 2,
            body: Body::Scout,
        },
    ];

    let mut seeded_ids: Vec<(String, String)> = Vec::new();
    for spec in specs.iter() {
        let opened = now - jiff::Span::new().hours(spec.hours_ago);
        let munged = munge_cwd(&spec.project_dir, &spec.cwd);
        let target = sessions_root.join(&munged);
        std::fs::create_dir_all(&target)
            .map_err(|source| CoreError::Io { path: target.clone(), source })?;
        let file_name = format!("{}_{}.jsonl", stamp(&opened), spec.uuid);
        let path = target.join(file_name);

        let header = serde_json::json!({
            "type": "session",
            "id": spec.uuid,
            "timestamp": opened.to_string(),
            "cwd": spec.cwd,
            "provider": "anthropic",
            "modelId": "claude-sonnet-4-5",
        });
        let mut lines = vec![header.to_string()];
        let t1 = opened + jiff::Span::new().seconds(30);
        lines.push(message_line(
            &t1,
            serde_json::json!({
                "role": "user", "content": spec.title,
            }),
        ));
        match spec.body {
            Body::Plain => {
                let t2 = t1 + jiff::Span::new().seconds(20);
                lines.push(message_line(&t2, assistant_text("On it — will report back.")));
            }
            Body::WithBash | Body::Scout => {
                let t2 = t1 + jiff::Span::new().seconds(5);
                lines.push(message_line(
                    &t2,
                    serde_json::json!({
                        "role": "assistant",
                        "model": "claude-sonnet-4-5",
                        "content": [
                            {"type": "text", "text": "Checking current state."},
                            {"type": "toolCall", "id": "call_seed_1", "name": "bash",
                             "arguments": {"command": "cargo test -q"}},
                        ],
                    }),
                ));
                let t3 = t2 + jiff::Span::new().seconds(40);
                lines.push(message_line(
                    &t3,
                    serde_json::json!({
                        "role": "toolResult", "toolCallId": "call_seed_1",
                        "toolName": "bash", "isError": false,
                        "content": [{"type": "text", "text": "test result: ok. 42 passed"}],
                    }),
                ));
                let t4 = t3 + jiff::Span::new().seconds(15);
                let text = if matches!(spec.body, Body::Scout) {
                    "Surveyed RPC docs; steering contract confirmed."
                } else {
                    "Fixed the race; suite green."
                };
                lines.push(message_line(&t4, assistant_text(text)));
            }
            Body::WithOpenAskUser => {
                let t2 = t1 + jiff::Span::new().seconds(10);
                lines.push(message_line(
                    &t2,
                    serde_json::json!({
                        "role": "assistant",
                        "model": "claude-sonnet-4-5",
                        "content": [
                            {"type": "text", "text": "Before I proceed:"},
                            {"type": "toolCall", "id": "call_seed_ask", "name": "ask_user",
                             "arguments": {"questions": [
                                {"id": "scope", "question": "Desktop-first or mobile-first?",
                                 "options": [{"label": "Desktop"}, {"label": "Mobile"}]},
                             ]}},
                        ],
                    }),
                ));
            }
        }

        std::fs::write(&path, format!("{}\n", lines.join("\n")))
            .map_err(|source| CoreError::Io { path: path.clone(), source })?;
        seeded_ids.push((spec.uuid.clone(), spec.title.clone()));
    }

    // Task list attached to the live pican session.
    std::fs::create_dir_all(&tasks_root)
        .map_err(|source| CoreError::Io { path: tasks_root.clone(), source })?;
    let task_file = tasks_root.join("tasks-00000000-0000-4000-8000-000000000001.json");
    let task_doc = serde_json::json!({
        "nextId": 4,
        "highWaterMark": 3,
        "tasks": [
            {"id": "1", "subject": "Reproduce flake", "status": "completed",
             "sessionId": "00000000-0000-4000-8000-000000000001"},
            {"id": "2", "subject": "Fix queue race", "status": "in_progress",
             "sessionId": "00000000-0000-4000-8000-000000000001"},
            {"id": "3", "subject": "Add regression test", "status": "pending",
             "blockedBy": ["2"],
             "sessionId": "00000000-0000-4000-8000-000000000001"},
        ],
    });
    std::fs::write(&task_file, format!("{task_doc:#}\n"))
        .map_err(|source| CoreError::Io { path: task_file.clone(), source })?;

    // Workflow run attached to the same session.
    let wf_dir = workflows_root.join("wf_seed0001");
    std::fs::create_dir_all(&wf_dir)
        .map_err(|source| CoreError::Io { path: wf_dir.clone(), source })?;
    let wf_doc = serde_json::json!({
        "runId": "wf_seed0001",
        "sessionId": "00000000-0000-4000-8000-000000000001",
        "name": "parallel-fix-review",
        "status": "completed",
        "agents": [
            {"label": "fixer", "phase": "Fix", "state": "done"},
            {"label": "reviewer", "phase": "Review", "state": "done"},
        ],
    });
    std::fs::write(wf_dir.join("workflow.json"), format!("{wf_doc:#}\n"))
        .map_err(|source| CoreError::Io { path: wf_dir.clone(), source })?;

    println!(
        "seeded {} sessions across {} projects under {}",
        seeded_ids.len(),
        projects.len(),
        dir.display()
    );
    for (id, title) in &seeded_ids {
        println!("  {id}  {title}");
    }
    Ok(())
}

/// Builds the munged per-project directory name exactly as pi stores it.
fn munge_cwd(_project_dir: &Path, cwd: &str) -> String {
    format!("-{}-", cwd.replace('/', "-"))
}

/// Formats a timestamp as a pi-style file-name stamp.
fn stamp(ts: &Timestamp) -> String {
    let z = ts.to_zoned(jiff::tz::TimeZone::UTC);
    format!(
        "{:04}-{:02}-{:02}T{:02}-{:02}-{:02}-{:03}Z",
        z.year(),
        i32::from(z.month()),
        z.day(),
        z.hour(),
        z.minute(),
        z.second(),
        z.subsec_nanosecond() / 1_000_000
    )
}

/// Wraps a role/content payload into a pi transcript record.
fn message_line(ts: &Timestamp, message: serde_json::Value) -> String {
    serde_json::json!({
        "type": "message",
        "timestamp": ts.to_string(),
        "message": message,
    })
    .to_string()
}

/// Assistant message payload with plain text content.
fn assistant_text(text: &str) -> serde_json::Value {
    serde_json::json!({
        "role": "assistant",
        "model": "claude-sonnet-4-5",
        "content": [{"type": "text", "text": text}],
    })
}

// ----------------------------------------------------------------- shared ----

fn display_name(cwd: &str) -> &str {
    cwd.rsplit('/').next().unwrap_or(cwd)
}

fn truncate(text: &str, max: usize) -> String {
    pecan_core::session::truncate_chars(text, max)
}

fn first_line(text: &str) -> String {
    text.lines().next().map(str::to_owned).unwrap_or_default()
}

fn ellipsis_flag(truncated: bool) -> &'static str {
    if truncated { " …" } else { "" }
}
