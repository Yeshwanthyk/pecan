//! pi-subagents wiring fixture: the `PECAN_PI_EXTENSIONS` entry point for
//! the real local pi-subagents package (its subagents extension) must reach
//! every spawned `pi` worker as an explicit `--extension` flag — the same
//! wiring `scripts/prove-subagents-bridge.sh` exercises against a real pi +
//! the real package. The activity-rail extension is opt-in (`PECAN_ACTIVITY_RAIL_EXTENSION`,
//! unset by default) and is intentionally not part of this documented value.
//!
//! Everything on Pecan's side is production code from
//! `crates/pecan/src/server/`; only the `pi` executable is simulated (a
//! fake binary that dumps its argv and answers `get_state` with a session
//! id, exactly the protocol the real worker startup needs). The paths are
//! kept synthetic so the test does not depend on the package being checked
//! out; the *shape* mirrors the real repository layout.

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
            "pecan-subagents-fixture-{}-{}",
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

/// Writes the fake `pi` executable: dumps its argv synchronously at startup
/// and answers `get_state` with a session id so `new_session` completes.
fn write_fake_pi(env: &FixtureEnv) -> PathBuf {
    let bin = env.path("bin/pi");
    let argv = env.path("argv.json");
    let script = r#"#!/usr/bin/env node
const fs = require("node:fs");
const readline = require("node:readline");
const ARGV = "__ARGV__";
fs.writeFileSync(ARGV, JSON.stringify(process.argv));
const rl = readline.createInterface({ input: process.stdin, terminal: false });
rl.on("line", (line) => {
  if (!line.trim()) return;
  let cmd;
  try { cmd = JSON.parse(line); } catch { return; }
  if (cmd.type === "get_state") {
    process.stdout.write(JSON.stringify({ type: "response", id: cmd.id, success: true, data: { sessionId: "fixture-session-1", isStreaming: false } }) + "\n");
  }
});
rl.on("close", () => process.exit(0));
"#
    .replace("__ARGV__", &format!("{}", argv.display()));
    std::fs::write(&bin, script).expect("write fake pi script");
    let mut permissions = std::fs::metadata(&bin).expect("stat fake pi").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&bin, permissions).expect("chmod fake pi");
    bin
}

/// Minimal HTTP/1.1 client (no extra dependencies): returns (status, body).
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

#[tokio::test(flavor = "multi_thread")]
async fn pi_subagents_extension_reaches_worker_argv() {
    // The documented value: the real pi-subagents subagents extension,
    // comma-joined, exactly what the proof script sets.
    let subagents_entry = "/ext/pi-subagents/extensions/subagents/index.ts";

    let env = FixtureEnv::new();
    let fake_pi = write_fake_pi(&env);
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
        .env("PECAN_PI_EXTENSIONS", subagents_entry)
        .env("RUST_LOG", "pecan=warn")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pecan serve");
    let mut pecan_stdout = pecan.stdout.take().expect("pecan stdout");
    let mut pecan_stderr = pecan.stderr.take().expect("pecan stderr");
    let stdout_drain = tokio::spawn(async move {
        let mut sink = Vec::new();
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut pecan_stdout, &mut sink).await;
    });
    let stderr_drain = tokio::spawn(async move {
        let mut sink = Vec::new();
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut pecan_stderr, &mut sink).await;
    });

    // A session creation spawns the fake worker, which dumps its argv before
    // answering get_state — so argv.json exists once the session is created.
    let _ = poll_sk(&addr, "/api/bootstrap", Duration::from_secs(30), |body| {
        body.get("ok").is_some() || body.get("seeded").is_some() || !body.is_null()
    })
    .await;
    let (status, created) =
        http_json(&addr, "POST", "/api/session/new", Some(&json!({ "cwd": env.path("project") })))
            .expect("post new session");
    assert_eq!(status, 200, "new session failed: {created}");
    assert_eq!(
        created.get("id"),
        Some(&json!("fixture-session-1")),
        "session id must come from the worker state: {created}",
    );

    let raw_argv = {
        let text = std::fs::read_to_string(env.path("argv.json")).expect("read fake pi argv dump");
        text.trim().to_owned()
    };
    let args: Vec<String> =
        serde_json::from_str::<Vec<String>>(&raw_argv).expect("argv dumped as a json string array");
    let mut found: Vec<String> = Vec::new();
    for (index, arg) in args.iter().enumerate() {
        if arg == "--extension" {
            found.push(args.get(index + 1).cloned().unwrap_or_default());
        }
    }
    assert_eq!(
        found,
        vec![subagents_entry.to_owned()],
        "PECAN_PI_EXTENSIONS for pi-subagents must become exactly one --extension flag",
    );

    let _ = pecan.kill().await;
    let _ = pecan.wait().await;
    let _ = stdout_drain.await;
    let _ = stderr_drain.await;
}
