//! `pecan session …` and `pecan events`: drive live Pi sessions through a
//! running server, with machine-readable results for scripted verification.
//!
//! Human output is the default; `--json` prints one JSON document (or JSONL
//! for `events`) on stdout. Timings are measured on the client so a script
//! can hill-climb end-to-end latency.

use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};

use crate::cli::CliError;
use crate::control::{Control, ControlError, DEFAULT_SERVER};

/// Default bound for waiting on a turn to settle.
const DEFAULT_WAIT: Duration = Duration::from_secs(120);

/// Parsed `--flag value`, `--switch`, and positional arguments.
#[derive(Debug, Default)]
struct Args {
    positional: Vec<String>,
    flags: Vec<(String, Option<String>)>,
}

/// Flags that never take a value.
const SWITCHES: &[&str] = &["--json", "--no-wait", "--confirm", "--deny", "--cancel", "--refresh"];

impl Args {
    fn parse(raw: &[String]) -> Self {
        let mut args = Self::default();
        let mut iter = raw.iter().peekable();
        while let Some(item) = iter.next() {
            if item.starts_with("--") {
                let value = if SWITCHES.contains(&item.as_str()) {
                    None
                } else {
                    iter.next_if(|next| !next.starts_with("--")).cloned()
                };
                args.flags.push((item.clone(), value));
            } else {
                args.positional.push(item.clone());
            }
        }
        args
    }

    fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|(flag, _)| flag == name)
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.flags.iter().find(|(flag, _)| flag == name).and_then(|(_, value)| value.as_deref())
    }

    fn positional(&self, index: usize, what: &str) -> Result<&str, CliError> {
        self.positional
            .get(index)
            .map(String::as_str)
            .ok_or_else(|| CliError::Usage(format!("missing argument {what}")))
    }

    fn timeout(&self) -> Result<Duration, CliError> {
        self.value("--timeout").map_or(Ok(DEFAULT_WAIT), |raw| {
            raw.parse::<f64>()
                .ok()
                .filter(|secs| secs.is_finite() && *secs > 0.0)
                .map(Duration::from_secs_f64)
                .ok_or_else(|| CliError::Usage("--timeout expects seconds".to_owned()))
        })
    }

    fn control(&self) -> Control {
        let env = std::env::var("PECAN_SERVER_URL").ok();
        Control::new(self.value("--server").or(env.as_deref()).unwrap_or(DEFAULT_SERVER))
    }
}

const SESSION_USAGE: &str = "session new --cwd <dir> | send <id> <text> [--mode send|steer|queue] \
     | wait <id> | respond <id> <request-id> (--value <v>|--confirm|--deny|--cancel) \
     | abort <id> | state <id> | asks <id>   [--no-wait] [--server <url>] [--timeout <secs>] [--json] \
     [--idempotency-key <key>]";

/// Dispatches `pecan session <verb> …` for live-session control.
pub(crate) fn run_session(verb: &str, rest: &[String]) -> Result<(), CliError> {
    let args = Args::parse(rest);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| CliError::Startup(format!("tokio runtime: {error}")))?;
    runtime.block_on(async {
        let control = args.control();
        let output = match verb {
            "new" => new_session(&control, &args).await?,
            "send" => send(&control, &args).await?,
            "wait" => {
                let id = args.positional(0, "<session-id>")?;
                let mut events = control.events().await?;
                let status = status(&control, id).await?;
                let report = if status.streaming {
                    wait_settled(&mut events, id, args.timeout()?, Instant::now()).await?
                } else {
                    TurnReport::idle()
                };
                Output::new(json!({"id": id, "turn": report}), report.human())
            }
            "respond" => respond(&control, &args).await?,
            "abort" => {
                let id = args.positional(0, "<session-id>")?;
                // Abort stops the worker, so there is no settle event to await.
                let value = control.post(&format!("/api/session/{id}/abort"), &json!({})).await?;
                Output::new(value, format!("aborted {id}"))
            }
            "state" => {
                let id = args.positional(0, "<session-id>")?;
                let value =
                    control.post(&format!("/api/session/{id}/agent-attach"), &json!({})).await?;
                let human = state_human(&value);
                Output::new(value, human)
            }
            "asks" => {
                let id = args.positional(0, "<session-id>")?;
                let value = control.get(&format!("/api/session/{id}/asks")).await?;
                let human = serde_json::to_string_pretty(&value).unwrap_or_default();
                Output::new(value, human)
            }
            _ => return Err(CliError::Usage(SESSION_USAGE.to_owned())),
        };
        output.print(args.has("--json"));
        Ok(())
    })
}

/// Streams server events as JSONL until interrupted or the stream closes.
pub(crate) fn run_events(rest: &[String]) -> Result<(), CliError> {
    let args = Args::parse(rest);
    let only = args.value("--session").map(str::to_owned);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| CliError::Startup(format!("tokio runtime: {error}")))?;
    runtime.block_on(async {
        let mut events = args.control().events().await?;
        while let Some(event) = events.next().await? {
            if only.as_deref().is_none_or(|id| event_session(&event) == Some(id)) {
                println!("{event}");
            }
        }
        Ok(())
    })
}

/// `pecan health [--refresh] [--json]`: is `pi` runnable for this server?
///
/// # Errors
/// Fails when the server is unreachable or reports `pi` as not ok.
pub(crate) fn run_health(rest: &[String]) -> Result<(), CliError> {
    let args = Args::parse(rest);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| CliError::Startup(format!("tokio runtime: {error}")))?;
    runtime.block_on(async {
        let path = if args.has("--refresh") { "/api/health?refresh=true" } else { "/api/health" };
        let health = args.control().get(path).await?;
        let status = health.get("status").and_then(Value::as_str).unwrap_or("unknown");
        let version = health.get("version").and_then(Value::as_str).unwrap_or("-");
        let detail = health.get("detail").and_then(Value::as_str).unwrap_or_default();
        Output::new(
            health.clone(),
            format!("pi: {status} {version} {detail}").trim_end().to_owned(),
        )
        .print(args.has("--json"));
        if status == "ok" {
            Ok(())
        } else {
            Err(CliError::Refused(format!("pi is {status}: {detail}")))
        }
    })
}

/// `pecan pair [--url <phone-facing base>] [--json]`: mint a one-time code a
/// browser redeems for a device cookie.
///
/// # Errors
/// Fails when the server is unreachable or refuses the CLI token.
pub(crate) fn run_pair(rest: &[String]) -> Result<(), CliError> {
    let args = Args::parse(rest);
    block_on(async {
        let control = args.control();
        let minted = control.post("/api/pair/code", &json!({})).await?;
        let code = minted.get("code").and_then(Value::as_str).unwrap_or_default();
        let minutes = minted.get("expiresInSecs").and_then(Value::as_u64).unwrap_or(0) / 60;
        let base = args.value("--url").unwrap_or(control.base()).trim_end_matches('/');
        let url = format!("{base}/?pair={code}");
        let mut json = minted.clone();
        if let Some(map) = json.as_object_mut() {
            map.insert("url".to_owned(), json!(url));
        }
        Output::new(
            json,
            format!(
                "Pairing code: {code}  (single use, expires in {minutes} min)\n\
                 Open {url}\n\
                 or open {base} and type the code."
            ),
        )
        .print(args.has("--json"));
        Ok(())
    })
}

/// `pecan devices [--json]` and `pecan devices revoke <id>`.
///
/// # Errors
/// Fails when the server is unreachable, refuses the CLI token, or the id is
/// unknown.
pub(crate) fn run_devices(rest: &[String]) -> Result<(), CliError> {
    let args = Args::parse(rest);
    block_on(async {
        let control = args.control();
        if args.positional.first().map(String::as_str) == Some("revoke") {
            let id = args.positional(1, "<device-id>")?;
            let value = control.delete(&format!("/api/devices/{id}")).await?;
            Output::new(value, format!("revoked {id}")).print(args.has("--json"));
            return Ok(());
        }
        let value = control.get("/api/devices").await?;
        let devices = value.get("devices").and_then(Value::as_array).cloned().unwrap_or_default();
        let human = if devices.is_empty() {
            "no paired devices (run `pecan pair`)".to_owned()
        } else {
            devices
                .iter()
                .map(|device| {
                    let text = |key| device.get(key).and_then(Value::as_str).unwrap_or("-");
                    let seen = device.get("lastSeenAtMs").and_then(Value::as_i64).map_or_else(
                        || "-".to_owned(),
                        |ms| {
                            jiff::Timestamp::from_millisecond(ms)
                                .map_or_else(|_| ms.to_string(), |at| at.to_string())
                        },
                    );
                    format!("{}  {:<24}  last seen {seen}", text("id"), text("name"))
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        Output::new(value, human).print(args.has("--json"));
        Ok(())
    })
}

/// `pecan push [--json]` lists push subscriptions; `pecan push test` sends a
/// test notification to every subscribed device and reports each outcome.
pub(crate) fn run_push(rest: &[String]) -> Result<(), CliError> {
    let args = Args::parse(rest);
    block_on(async {
        let control = args.control();
        let rows = |value: &Value, key: &str, line: &dyn Fn(&Value) -> String| {
            value
                .get(key)
                .and_then(Value::as_array)
                .map(|items| items.iter().map(line).collect::<Vec<_>>().join("\n"))
                .unwrap_or_default()
        };
        let text = |item: &Value, key: &str| {
            item.get(key).and_then(Value::as_str).unwrap_or("-").to_owned()
        };
        match args.positional.first().map(String::as_str) {
            Some("test") => {
                let value = control.post("/api/push/test", &serde_json::json!({})).await?;
                let count = |key| value.get(key).and_then(Value::as_u64).unwrap_or(0);
                let (sent, removed, failed) = (count("sent"), count("removed"), count("failed"));
                let mut human = format!("sent {sent}, removed {removed}, failed {failed}");
                let detail = rows(&value, "results", &|item| {
                    let error = item.get("error").and_then(Value::as_str).unwrap_or("");
                    format!(
                        "{}  {:<24}  {} {error}",
                        text(item, "deviceId"),
                        text(item, "deviceName"),
                        text(item, "outcome")
                    )
                });
                if !detail.is_empty() {
                    human = format!("{human}\n{detail}");
                }
                if sent + removed + failed == 0 {
                    human = "no subscribed devices (enable notifications in Settings)".to_owned();
                }
                Output::new(value, human).print(args.has("--json"));
                if failed > 0 {
                    return Err(CliError::Refused(format!("{failed} push(es) failed")));
                }
                Ok(())
            }
            None => {
                let value = control.get("/api/push/subscriptions").await?;
                let listed = rows(&value, "subscriptions", &|item| {
                    let endpoint = text(item, "endpoint");
                    let origin = endpoint.split('/').take(3).collect::<Vec<_>>().join("/");
                    format!(
                        "{}  {:<24}  {origin}",
                        text(item, "deviceId"),
                        text(item, "deviceName")
                    )
                });
                let human = if listed.is_empty() {
                    "no subscribed devices (enable notifications in Settings)".to_owned()
                } else {
                    listed
                };
                Output::new(value, human).print(args.has("--json"));
                Ok(())
            }
            Some(other) => Err(CliError::Usage(format!("pecan push: unknown subcommand {other}"))),
        }
    })
}

pub(crate) fn block_on<F: Future<Output = Result<(), CliError>>>(
    future: F,
) -> Result<(), CliError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| CliError::Startup(format!("tokio runtime: {error}")))?
        .block_on(future)
}

/// A command result with both machine and human renderings.
pub(crate) struct Output {
    json: Value,
    human: String,
}

impl Output {
    pub(crate) fn new(json: Value, human: String) -> Self {
        Self { json, human }
    }

    pub(crate) fn print(&self, as_json: bool) {
        if as_json {
            println!("{:#}", self.json);
        } else {
            println!("{}", self.human);
        }
    }
}

async fn new_session(control: &Control, args: &Args) -> Result<Output, CliError> {
    let cwd = args
        .value("--cwd")
        .ok_or_else(|| CliError::Usage("session new --cwd <absolute dir>".to_owned()))?;
    let cwd = std::path::absolute(cwd)
        .map_err(|error| CliError::Usage(format!("--cwd: {error}")))?
        .display()
        .to_string();
    let started = Instant::now();
    let value = control
        .post_keyed("/api/session/new", &json!({"cwd": cwd}), args.value("--idempotency-key"))
        .await?;
    let id = value.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
    let create_ms = millis(started.elapsed());
    Ok(Output::new(json!({"id": id, "cwd": cwd, "createMs": create_ms}), id))
}

async fn send(control: &Control, args: &Args) -> Result<Output, CliError> {
    let id = args.positional(0, "<session-id>")?;
    let text = args.positional.get(1..).unwrap_or_default().join(" ");
    if text.trim().is_empty() {
        return Err(CliError::Usage("session send <id> <text>".to_owned()));
    }
    let mode = args.value("--mode").unwrap_or("send");
    if !matches!(mode, "send" | "steer" | "queue") {
        return Err(CliError::Usage("--mode expects send, steer, or queue".to_owned()));
    }
    post_then_wait(control, args, id, "message", &json!({"text": text, "mode": mode})).await
}

/// Posts `body` to `/api/session/{id}/{action}` and, unless `--no-wait`,
/// reports the turn until it settles or blocks on a dialog. The event stream
/// is opened before posting so no event of the turn can be missed.
async fn post_then_wait(
    control: &Control,
    args: &Args,
    id: &str,
    action: &str,
    body: &Value,
) -> Result<Output, CliError> {
    let timeout = args.timeout()?;
    let mut events = if args.has("--no-wait") { None } else { Some(control.events().await?) };
    let started = Instant::now();
    let accepted = control
        .post_keyed(&format!("/api/session/{id}/{action}"), body, args.value("--idempotency-key"))
        .await?;
    let Some(events) = events.as_mut() else {
        return Ok(Output::new(json!({"id": id, "accepted": accepted}), format!("{action}: {id}")));
    };
    let report = wait_settled(events, id, timeout, started).await?;
    let human = report.human();
    Ok(Output::new(json!({"id": id, "accepted": accepted, "turn": report}), human))
}

async fn respond(control: &Control, args: &Args) -> Result<Output, CliError> {
    let id = args.positional(0, "<session-id>")?;
    let request_id = args.positional(1, "<request-id>")?;
    let mut body = json!({"requestId": request_id});
    let extra = if args.has("--cancel") {
        ("cancelled", json!(true))
    } else if args.has("--confirm") || args.has("--deny") {
        ("confirmed", json!(args.has("--confirm")))
    } else if let Some(value) = args.value("--value") {
        ("value", json!(value))
    } else {
        return Err(CliError::Usage(
            "respond needs --value <v>, --confirm, --deny, or --cancel".to_owned(),
        ));
    };
    if let Some(map) = body.as_object_mut() {
        map.insert(extra.0.to_owned(), extra.1);
    }
    post_then_wait(control, args, id, "respond", &body).await
}

/// Live status of one session's worker.
#[derive(Debug)]
struct Status {
    streaming: bool,
}

async fn status(control: &Control, id: &str) -> Result<Status, ControlError> {
    let value = control.get(&format!("/api/session/{id}/status")).await?;
    Ok(Status { streaming: value.get("streaming").and_then(Value::as_bool).unwrap_or(false) })
}

/// What happened during one agent turn, as observed over SSE.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct TurnReport {
    /// `settled`, `waiting-for-user`, or `idle` (nothing was running).
    outcome: &'static str,
    /// Time to the first text delta, if any text streamed.
    first_token_ms: Option<u64>,
    /// Time until the outcome was reached.
    total_ms: u64,
    /// Final assistant text of the turn.
    text: String,
    /// Tool names invoked, in order.
    tools: Vec<String>,
    /// Provider error message, when the turn failed.
    error: Option<String>,
    /// Blocking dialog awaiting an answer, when outcome is `waiting-for-user`.
    dialog: Option<Value>,
    /// Number of agent events observed.
    events: usize,
}

impl TurnReport {
    fn idle() -> Self {
        Self { outcome: "idle", ..Self::default() }
    }

    fn human(&self) -> String {
        let mut lines = vec![format!(
            "{} in {}ms (first token {})",
            self.outcome,
            self.total_ms,
            self.first_token_ms.map_or_else(|| "-".to_owned(), |ms| format!("{ms}ms"))
        )];
        if !self.tools.is_empty() {
            lines.push(format!("tools: {}", self.tools.join(", ")));
        }
        if let Some(error) = &self.error {
            lines.push(format!("error: {error}"));
        }
        if let Some(dialog) = &self.dialog {
            lines.push(format!(
                "waiting: {} {} (request {})",
                dialog.get("method").and_then(Value::as_str).unwrap_or("dialog"),
                dialog.get("title").and_then(Value::as_str).unwrap_or_default(),
                dialog.get("id").and_then(Value::as_str).unwrap_or_default(),
            ));
        }
        if !self.text.is_empty() {
            lines.push(self.text.clone());
        }
        lines.join("\n")
    }
}

/// Consumes agent events for `id` until the turn settles or blocks on a
/// dialog, bounded by `timeout`.
async fn wait_settled(
    events: &mut crate::control::EventStream,
    id: &str,
    timeout: Duration,
    started: Instant,
) -> Result<TurnReport, ControlError> {
    let mut report = TurnReport::default();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let next = tokio::time::timeout_at(deadline, events.next())
            .await
            .map_err(|_elapsed| ControlError::Timeout(timeout, "turn to settle"))??;
        let Some(frame) = next else {
            return Err(ControlError::StreamClosed("turn settled"));
        };
        if event_session(&frame) != Some(id) {
            continue;
        }
        let Some(event) = frame.get("event") else { continue };
        report.events += 1;
        match event.get("type").and_then(Value::as_str) {
            Some("message_update") => {
                let delta = event.pointer("/assistantMessageEvent/type").and_then(Value::as_str);
                if delta == Some("text_delta") && report.first_token_ms.is_none() {
                    report.first_token_ms = Some(millis(started.elapsed()));
                }
            }
            Some("message_end") if event.pointer("/message/role") == Some(&json!("assistant")) => {
                let message = event.get("message").unwrap_or(&Value::Null);
                report.text = assistant_text(message);
                report.error =
                    message.get("errorMessage").and_then(Value::as_str).map(str::to_owned);
            }
            Some("tool_execution_start") => {
                if let Some(name) = event.get("toolName").and_then(Value::as_str) {
                    report.tools.push(name.to_owned());
                }
            }
            Some("extension_ui_request")
                if matches!(
                    event.get("method").and_then(Value::as_str),
                    Some("select" | "confirm" | "input" | "editor")
                ) =>
            {
                report.outcome = "waiting-for-user";
                report.dialog = Some(event.clone());
                report.total_ms = millis(started.elapsed());
                return Ok(report);
            }
            Some("agent_settled") => {
                report.outcome = "settled";
                report.total_ms = millis(started.elapsed());
                return Ok(report);
            }
            _ => {}
        }
    }
}

fn event_session(frame: &Value) -> Option<&str> {
    frame.get("id").and_then(Value::as_str)
}

fn assistant_text(message: &Value) -> String {
    message
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

fn state_human(value: &Value) -> String {
    let state = value.get("state").unwrap_or(&Value::Null);
    let model = state
        .pointer("/model/id")
        .and_then(Value::as_str)
        .or_else(|| state.get("modelId").and_then(Value::as_str))
        .unwrap_or("?");
    format!(
        "model {model} · thinking {} · streaming {}",
        state.get("thinkingLevel").and_then(Value::as_str).unwrap_or("?"),
        state.get("isStreaming").and_then(Value::as_bool).unwrap_or(false),
    )
}

fn millis(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{Args, assistant_text};

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn parses_positionals_values_and_switches() {
        let args = Args::parse(&strings(&["abc", "hello", "world", "--mode", "steer", "--json"]));
        assert_eq!(args.positional, strings(&["abc", "hello", "world"]), "positionals kept");
        assert_eq!(args.value("--mode"), Some("steer"), "flag value read");
        assert!(args.has("--json"), "switch present");
        assert!(!args.has("--no-wait"), "absent switch");
    }

    #[test]
    fn switches_never_swallow_the_next_positional() {
        let args = Args::parse(&strings(&["--json", "abc"]));
        assert_eq!(args.positional, strings(&["abc"]), "positional after switch");
    }

    #[test]
    fn rejects_bad_timeouts() {
        for raw in ["0", "-1", "nan", "soon"] {
            let args = Args::parse(&strings(&["--timeout", raw]));
            assert!(args.timeout().is_err(), "timeout {raw} rejected");
        }
        let args = Args::parse(&strings(&["--timeout", "2.5"]));
        assert_eq!(args.timeout().ok().map(|d| d.as_millis()), Some(2500), "fractional seconds");
    }

    #[test]
    fn joins_only_text_blocks() {
        let message = serde_json::json!({"content": [
            {"type": "thinking", "thinking": "hmm"},
            {"type": "text", "text": "a"},
            {"type": "toolCall", "name": "bash"},
            {"type": "text", "text": "b"},
        ]});
        assert_eq!(assistant_text(&message), "ab", "text blocks concatenated");
    }
}
