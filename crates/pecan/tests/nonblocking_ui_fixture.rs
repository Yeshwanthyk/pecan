//! End-to-end proof that **fire-and-forget** Pi RPC extension UI updates —
//! `notify`, `setStatus`, `setWidget`, `setTitle`, `set_editor_text` — reach
//! a browser over SSE intact while never entering the blocking `/asks`
//! dialog registry.
//!
//! The fake `pi` binary speaks the RPC JSONL protocol to a real spawned
//! `pecan serve`. Every frame and assertion uses the documented contracts:
//! the exact `extension_ui_request` shapes from pi's `docs/rpc.md`
//! "Extension UI Protocol", whose fire-and-forget methods emit a request on
//! stdout but expect no response, and Pecan's own `/api/events` SSE stream +
//! `/api/session/{id}/asks`.
//!
//! Everything on Pecan's side — the worker reader, the raw-event fan-out,
//! the `forward_worker_events` task, the SSE handler for `/events`, and the
//! `record_pending_ask` registry gate — is production code from
//! `crates/pecan/src/server/`. Only the `pi` executable is simulated.
//!
//! The run also emits one *blocking* `select` afterwards as a negative
//! control: the same registry machinery provably works for dialogs, so the
//! five fire-and-forget frames staying out of `/asks` is a statement about
//! them, not about a dead registry.

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};

/// Unique per-run fixture root created under the system temp dir.
struct FixtureEnv {
    root: PathBuf,
}

impl FixtureEnv {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "pecan-nonblocking-ui-fixture-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock before unix epoch")
                .as_nanos(),
        ));
        std::fs::create_dir_all(root.join("bin")).expect("create fixture bin dir");
        std::fs::create_dir_all(root.join("agent").join("sessions"))
            .expect("create fixture sessions dir");
        std::fs::create_dir_all(root.join("agent").join("tasks"))
            .expect("create fixture tasks dir");
        std::fs::create_dir_all(root.join("agent").join("workflows"))
            .expect("create fixture workflows dir");
        std::fs::create_dir_all(root.join("project")).expect("create fixture project dir");
        Self { root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn agent_dir(&self) -> PathBuf {
        self.root.join("agent")
    }
}

impl Drop for FixtureEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The five fire-and-forget `extension_ui_request` frames the fake worker
/// emits after the first prompt — byte-for-byte the documented shapes from
/// pi's `docs/rpc.md` "Extension UI Protocol". These methods block nothing:
/// they expect no `extension_ui_response` on stdin.
const FIRE_AND_FORGET_FRAMES: &[(&str, &str)] = &[
    (
        "uuid-notify",
        r#"{"type":"extension_ui_request","id":"uuid-notify","method":"notify","message":"Command blocked by user","notifyType":"warning"}"#,
    ),
    (
        "uuid-setstatus",
        r#"{"type":"extension_ui_request","id":"uuid-setstatus","method":"setStatus","statusKey":"my-ext","statusText":"Turn 3 running..."}"#,
    ),
    (
        "uuid-setwidget",
        r#"{"type":"extension_ui_request","id":"uuid-setwidget","method":"setWidget","widgetKey":"ui-events-proof/v1","widgetLines":["--- My Widget ---","Line 1","Line 2"],"widgetPlacement":"aboveEditor"}"#,
    ),
    (
        "uuid-settitle",
        r#"{"type":"extension_ui_request","id":"uuid-settitle","method":"setTitle","title":"pi - my project"}"#,
    ),
    (
        "uuid-set_editor_text",
        r#"{"type":"extension_ui_request","id":"uuid-set_editor_text","method":"set_editor_text","text":"prefilled text for the user"}"#,
    ),
];

/// One blocking dialog frame, emitted after the second prompt as a negative
/// control: it must be the *only* entry the `/asks` registry records.
const BLOCKING_FRAME: (&str, &str) = (
    "uuid-select",
    r#"{"type":"extension_ui_request","id":"uuid-select","method":"select","title":"Pick an option","options":["Allow","Block"],"timeout":10000}"#,
);

/// Writes the fake `pi` executable used by the spawned server: answers
/// `get_state` with a session id, emits the five fire-and-forget frames
/// after the first prompt and the blocking `select` after the second, then
/// acknowledges the prompt.
fn write_fake_pi(env: &FixtureEnv) -> PathBuf {
    let bin = env.path("bin/pi");
    let session_id = "fixture-session-1";
    let fire_and_forget = FIRE_AND_FORGET_FRAMES
        .iter()
        .map(|(_, raw)| format!("{raw},"))
        .collect::<Vec<_>>()
        .join("\n");
    let select = BLOCKING_FRAME.1.to_owned();
    let script = r#"#!/usr/bin/env node
// Deterministic fake `pi` for the pecan non-blocking-UI fixture test.
// Speaks just enough of the RPC JSONL protocol pecan needs:
//   - answers `get_state` with a session id
//   - after the first `prompt` emits the five fire-and-forget
//     extension_ui_request frames (notify / setStatus / setWidget /
//     setTitle / set_editor_text), which expect no response
//   - after the second `prompt` emits one blocking `select` frame
const readline = require("node:readline");
const SESSION_ID = "__SESSION_ID__";
const FIRE_AND_FORGET = [
__FIRE_AND_FORGET__
];
const SELECT = __SELECT__;
let prompts = 0;
const rl = readline.createInterface({ input: process.stdin, terminal: false });
rl.on("line", (line) => {
  if (!line.trim()) return;
  let cmd;
  try { cmd = JSON.parse(line); } catch { return; }
  if (cmd.type === "get_state") {
    process.stdout.write(JSON.stringify({ type: "response", id: cmd.id, success: true, data: { sessionId: SESSION_ID, isStreaming: false } }) + "\n");
    return;
  }
  if (cmd.type === "prompt") {
    prompts += 1;
    if (prompts === 1) {
      for (const frame of FIRE_AND_FORGET) process.stdout.write(JSON.stringify(frame) + "\n");
    } else if (prompts === 2) {
      process.stdout.write(JSON.stringify(SELECT) + "\n");
    }
    process.stdout.write(JSON.stringify({ type: "response", id: cmd.id, command: "prompt", success: true }) + "\n");
  }
});
rl.on("close", () => process.exit(0));
"#
    .replace("__SESSION_ID__", session_id)
    .replace("__FIRE_AND_FORGET__", &fire_and_forget)
    .replace("__SELECT__", &format!("{select}"));
    std::fs::write(&bin, script).expect("write fake pi script");
    let mut permissions = std::fs::metadata(&bin).expect("stat fake pi").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&bin, permissions).expect("chmod fake pi");
    bin
}

/// Minimal HTTP/1.1 client (no extra dependencies): returns (status, body).
/// Only the initial connect can fail; everything after a live connection is
/// asserted directly. Blocking std sockets are fine here — the server is
/// loopback-local and each call completes in milliseconds.
fn http_json(
    addr: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<(u16, Value), std::io::Error> {
    let mut stream = std::net::TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let body_text = body.map(Value::to_string).unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body_text}",
        body_text.len(),
    );
    stream.write_all(request.as_bytes()).expect("write http request");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("read http response");
    let text = String::from_utf8_lossy(&response);
    let head = text.split("\r\n\r\n").next().unwrap_or_default();
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .expect("http status code");
    let body = text
        .split("\r\n\r\n")
        .nth(1)
        .map(|raw| serde_json::from_str(raw).unwrap_or_else(|_| json!({})))
        .unwrap_or_else(|| json!({}));
    Ok((status, body))
}

/// Polls an HTTP GET until the predicate holds or the deadline passes.
/// Connect failures are retried so the very first poll can race the server
/// coming up.
async fn poll_sk(
    addr: &str,
    path: &str,
    deadline: Duration,
    mut predicate: impl FnMut(&Value) -> bool,
) -> Value {
    let start = std::time::Instant::now();
    loop {
        let (status, body) = match http_json(addr, "GET", path, None) {
            Ok(result) => result,
            Err(error) => {
                assert!(start.elapsed() < deadline, "GET {path} connect failed: {error}",);
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        assert_eq!(status, 200, "GET {path} failed: {body}");
        if predicate(&body) {
            return body;
        }
        assert!(start.elapsed() < deadline, "timed out waiting for {path}: {body}",);
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// Persistent reader over one `/api/events` SSE connection. Incrementally
/// consumes the HTTP response head, then yields complete SSE frames as
/// `(event name, raw data JSON)`. Comments, keep-alive pings, and events
/// without data are skipped.
struct SseReader {
    stream: std::net::TcpStream,
    buffer: Vec<u8>,
    deadline: std::time::Instant,
}

impl SseReader {
    /// Opens `/api/events` and consumes the response head. Only the initial
    /// connect and the status line of the head are checked here; the stream
    /// then stays open for [`Self::next_frame`].
    fn connect(addr: &str, timeout: Duration, deadline: Duration) -> Self {
        let start = std::time::Instant::now();
        loop {
            match Self::try_connect(addr, timeout) {
                Ok(reader) => return reader,
                Err(error) => {
                    assert!(start.elapsed() < deadline, "GET /api/events connect failed: {error}",);
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }

    fn try_connect(addr: &str, timeout: Duration) -> Result<Self, std::io::Error> {
        let mut stream = std::net::TcpStream::connect(addr)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        let request = format!(
            "GET /api/events HTTP/1.1\r\nHost: {addr}\r\nAccept: text/event-stream\r\nConnection: keep-alive\r\n\r\n",
        );
        stream.write_all(request.as_bytes())?;
        let mut buffer = Vec::new();
        let deadline = std::time::Instant::now() + duration_millis(10_000);
        while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for /api/events response head",
            );
            let mut chunk = [0_u8; 4096];
            match stream.read(&mut chunk) {
                Ok(0) => {
                    return Err(std::io::Error::other("sse stream closed during response head"));
                }
                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                Err(error) if is_timeout(&error) => {}
                Err(error) => return Err(error),
            }
        }
        let head = String::from_utf8_lossy(&buffer);
        assert!(
            head.lines().next().is_some_and(|line| line.contains(" 200 ")),
            "GET /api/events rejected: {}",
            head.lines().next().unwrap_or("(no status line)"),
        );
        let head_end = buffer
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("response head terminator")
            + 4;
        buffer.drain(..head_end);
        Ok(Self { stream, buffer, deadline: std::time::Instant::now() + duration_millis(10_000) })
    }

    /// Blocks for the next complete SSE frame, retrying while the socket is
    /// quiet, and returns `(event name, data JSON)`; `None` means the stream
    /// closed before the deadline.
    fn next_frame(&mut self) -> Option<(String, String)> {
        loop {
            if let Some(end) = self.buffer.windows(2).position(|window| window == b"\n\n") {
                let frame: Vec<u8> = self.buffer.drain(..=end + 1).collect();
                let text = String::from_utf8_lossy(&frame);
                let mut event = String::new();
                let mut data = String::new();
                for line in text.lines() {
                    let line = line.trim_end_matches('\r');
                    if let Some(rest) = line.strip_prefix("event:") {
                        event = rest.trim().to_owned();
                    } else if let Some(rest) = line.strip_prefix("data:") {
                        if !data.is_empty() {
                            data.push('\n');
                        }
                        data.push_str(rest.trim_start());
                    }
                    // comment frames (": ...") and unknown lines are ignored
                }
                if !data.is_empty() {
                    return Some((event, data));
                }
                continue;
            }
            assert!(
                std::time::Instant::now() < self.deadline,
                "timed out waiting for the next SSE frame",
            );
            let mut chunk = [0_u8; 16 * 1024];
            match self.stream.read(&mut chunk) {
                Ok(0) => return None,
                Ok(n) => self.buffer.extend_from_slice(&chunk[..n]),
                Err(error) if is_timeout(&error) => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_error) => return None,
            }
        }
    }

    /// Returns the next `agent` event whose JSON `event.event` payload
    /// matches `predicate`, skipping unrelated SSE frames (`index-changed`,
    /// `thread-changed`, other sessions, keep-alives). The whole payload is
    /// returned so the caller can assert it was forwarded intact.
    fn wait_for_agent_event(
        &mut self,
        session_id: &str,
        mut predicate: impl FnMut(&Value) -> bool,
    ) -> Value {
        loop {
            let (event_name, data) = self.next_frame().expect(
                "sse stream closed before the expected agent event arrived (is pecan still alive?)",
            );
            if event_name != "agent" {
                continue;
            }
            let parsed: Value =
                serde_json::from_str(&data).expect("agent event data must be valid json");
            assert_eq!(
                parsed.get("type"),
                Some(&json!("agent-event")),
                "agent event must carry the agent-event envelope: {parsed:?}",
            );
            assert_eq!(
                parsed.get("id"),
                Some(&json!(session_id)),
                "agent event must target the fixture session: {parsed:?}",
            );
            if parsed
                .get("event")
                .and_then(Value::as_object)
                .is_some_and(|payload| payload.get("type") == Some(&json!("extension_ui_request")))
                && predicate(parsed.get("event").expect("agent event payload"))
            {
                return parsed.get("event").expect("agent event payload").clone();
            }
        }
    }
}

fn duration_millis(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

fn is_timeout(error: &std::io::Error) -> bool {
    matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)
}

#[tokio::test(flavor = "multi_thread")]
async fn fire_and_forget_ui_requests_forward_over_sse_and_skip_the_asks_registry() {
    let env = FixtureEnv::new();
    let fake_pi = write_fake_pi(&env);

    // Pick a free loopback port deterministically (the server prints its
    // bound URL to stdout, which is block-buffered when piped).
    let port = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind probe port");
        listener.local_addr().expect("probe addr").port()
    };
    let addr = format!("127.0.0.1:{port}");

    let mut path = fake_pi.parent().expect("fake pi parent").as_os_str().to_owned();
    path.push(":");
    path.push(std::env::var("PATH").unwrap_or_default());
    let mut pecan = tokio::process::Command::new(env!("CARGO_BIN_EXE_pecan"))
        .args(["serve", "--port", &port.to_string(), "--no-open"])
        .env("PATH", path)
        .env("PECAN_AGENT_DIR", env.agent_dir())
        .env("RUST_LOG", "pecan=warn")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pecan serve");
    let mut pecan_stdout = pecan.stdout.take().expect("pecan stdout");
    let mut pecan_stderr = pecan.stderr.take().expect("pecan stderr");
    // Drain pipes so a full buffer cannot stall the server. Async copies run
    // on the runtime without blocking it.
    let stdout_drain = tokio::spawn(async move {
        let mut sink = Vec::new();
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut pecan_stdout, &mut sink).await;
    });
    let stderr_drain = tokio::spawn(async move {
        let mut sink = Vec::new();
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut pecan_stderr, &mut sink).await;
    });

    // 1. Wait for the API to come up, then open the SSE stream before any
    //    worker exists, so every broadcast agent event is observed.
    let _ = poll_sk(&addr, "/api/bootstrap", Duration::from_secs(30), |body| {
        body.get("ok").is_some() || body.get("seeded").is_some() || !body.is_null()
    })
    .await;
    let mut sse = SseReader::connect(&addr, Duration::from_secs(5), Duration::from_secs(30));

    // 2. Create a session; the fake worker answers get_state with its id.
    let (status, created) =
        http_json(&addr, "POST", "/api/session/new", Some(&json!({ "cwd": env.path("project") })))
            .expect("post new session");
    assert_eq!(status, 200, "new session failed: {created}");
    assert_eq!(
        created.get("id"),
        Some(&json!("fixture-session-1")),
        "session id must come from the worker state: {created}",
    );

    // 3. Prompt once. The fake worker answers by emitting the five
    //    fire-and-forget extension_ui_request frames.
    let (status, accepted) = http_json(
        &addr,
        "POST",
        "/api/session/fixture-session-1/message",
        Some(&json!({ "text": "hello", "mode": "send" })),
    )
    .expect("post message");
    assert_eq!(status, 200, "message failed: {accepted}");

    // 4. Every frame arrives on the SSE stream forwarded intact: the full
    //    documented payload, byte-for-byte JSON, under the fixture session.
    let expected: Vec<(String, Value)> = FIRE_AND_FORGET_FRAMES
        .iter()
        .map(|(id, raw)| (id.to_string(), serde_json::from_str(raw).expect("fixture frame json")))
        .collect();
    let mut seen: Vec<Value> = Vec::new();
    for (_, payload) in &expected {
        let frame = sse.wait_for_agent_event("fixture-session-1", |event| {
            event.get("id") == Some(&json!(payload.get("id").expect("fixture frame id")))
        });
        assert_eq!(&frame, payload, "fire-and-forget frame must be forwarded intact over SSE");
        seen.push(frame);
    }
    assert_eq!(
        seen.len(),
        5,
        "expected exactly the five fire-and-forget frames on SSE, one per fixture frame",
    );

    // 5. None of them entered the blocking registry: /asks stays empty even
    //    though the server demonstrably processed all five frames.
    let asks =
        poll_sk(&addr, "/api/session/fixture-session-1/asks", Duration::from_secs(10), |body| {
            body.get("asks").and_then(Value::as_array).is_some_and(Vec::is_empty)
        })
        .await;
    assert_eq!(asks["asks"], json!([]), "fire-and-forget updates must not enter /asks");

    // 6. Negative control: a second prompt emits one blocking `select`. It
    //    forwards over SSE the same way…
    let (status, accepted) = http_json(
        &addr,
        "POST",
        "/api/session/fixture-session-1/message",
        Some(&json!({ "text": "second", "mode": "send" })),
    )
    .expect("post second message");
    assert_eq!(status, 200, "second message failed: {accepted}");
    let select_payload: Value =
        serde_json::from_str(BLOCKING_FRAME.1).expect("fixture select frame json");
    let select_on_sse = sse.wait_for_agent_event("fixture-session-1", |event| {
        event.get("id") == Some(&json!("uuid-select"))
    });
    assert_eq!(
        &select_on_sse, &select_payload,
        "the blocking select must also forward intact over SSE",
    );

    // …but unlike the fire-and-forget frames, it is the one entry the
    // blocking registry records, with none of the five fire-and-forget ids.
    let asks =
        poll_sk(&addr, "/api/session/fixture-session-1/asks", Duration::from_secs(10), |body| {
            body.get("asks")
                .and_then(Value::as_array)
                .is_some_and(|list| list.iter().any(|ask| ask["id"] == json!("uuid-select")))
        })
        .await;
    let listed = asks["asks"].as_array().expect("asks array").clone();
    assert_eq!(listed.len(), 1, "only the blocking select is registered: {listed:?}");
    assert_eq!(listed[0].get("id"), Some(&json!("uuid-select")));
    assert_eq!(listed[0].get("method"), Some(&json!("select")));
    for (fire_and_forget_id, _) in FIRE_AND_FORGET_FRAMES {
        assert!(
            !listed[0]["id"].as_str().is_some_and(|id| id == *fire_and_forget_id),
            "fire-and-forget id {fire_and_forget_id} must never appear in /asks",
        );
    }

    // 7. Answering the control dialog drains the registry as usual.
    let (status, body) = http_json(
        &addr,
        "POST",
        "/api/session/fixture-session-1/respond",
        Some(&json!({"requestId": "uuid-select", "value": "Allow"})),
    )
    .expect("post respond");
    assert_eq!(status, 200, "respond failed: {body}");
    let drained =
        poll_sk(&addr, "/api/session/fixture-session-1/asks", Duration::from_secs(10), |body| {
            body.get("asks").and_then(Value::as_array).is_some_and(Vec::is_empty)
        })
        .await;
    assert_eq!(drained["asks"], json!([]));

    let _ = pecan.kill().await;
    let _ = pecan.wait().await;
    let _ = stdout_drain.await;
    let _ = stderr_drain.await;
}
