//! End-to-end extension-dialog fixture: a fake `pi` binary speaks the RPC
//! JSONL protocol to a real spawned `pecan serve`, and the test proves the
//! blocking extension dialogs (select / confirm / input / editor) appear in
//! Pecan and resolve back through it.
//!
//! The fake worker is the only simulated piece. Everything on Pecan's side —
//! the worker reader, the pending-ask registry, the `/asks` endpoint, the
//! `/respond` handler and the `extension_ui_response` frame it writes to pi's
//! stdin — is the production code from `crates/pecan/src/server/`.
//!
//! The dialogs and the response protocol are byte-for-byte the documented
//! pi RPC extension UI contract (docs/rpc.md "Extension UI Protocol"),
//! cross-checked live against `pi --mode rpc` with a real extension.

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::time::Duration;

    /// Pre-seeded CLI bearer token the spawned server adopts.
    const CLI_TOKEN: &str = "fixture-cli-token-0123456789abcdef0123456789";

    use serde_json::{Value, json};

    /// Unique per-run fixture root created under the system temp dir.
    struct FixtureEnv {
        root: PathBuf,
    }

    impl FixtureEnv {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "pecan-dialog-fixture-{}-{}",
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
            std::fs::remove_dir_all(&self.root).unwrap_or_default();
        }
    }

    /// Request ids and payloads the fake pi emits — identical to the documented
    /// examples and to what a real extension produces via `ctx.ui.*` in RPC mode.
    const DIALOG_FRAMES: &[(&str, &str)] = &[
        (
            "uuid-select",
            r#"{"type":"extension_ui_request","id":"uuid-select","method":"select","title":"Pick an option","options":["Allow","Block"],"timeout":10000}"#,
        ),
        (
            "uuid-confirm",
            r#"{"type":"extension_ui_request","id":"uuid-confirm","method":"confirm","title":"Clear session?","message":"All messages will be lost.","timeout":5000}"#,
        ),
        (
            "uuid-input",
            r#"{"type":"extension_ui_request","id":"uuid-input","method":"input","title":"Enter a value","placeholder":"type something..."}"#,
        ),
        (
            "uuid-editor",
            r#"{"type":"extension_ui_request","id":"uuid-editor","method":"editor","title":"Edit some text","prefill":"Line 1\nLine 2"}"#,
        ),
    ];

    /// Writes the fake `pi` executable used by the spawned server.
    fn write_fake_pi(env: &FixtureEnv) -> PathBuf {
        let bin = env.path("bin/pi");
        let results = env.path("responses.jsonl");
        let argv = env.path("argv.json");
        let session_id = "fixture-session-1";
        let frames =
            DIALOG_FRAMES.iter().map(|(_, raw)| format!("{raw},")).collect::<Vec<_>>().join("\n");
        let script = r#"#!/usr/bin/env node
    // Deterministic fake `pi` for the pecan extension-dialog fixture test.
    // Speaks just enough of the RPC JSONL protocol pecan needs:
    //   - answers `get_state` with a session id
    //   - emits the four blocking extension_ui_request frames after the first
    //     `prompt` command (mimicking an extension turn)
    //   - appends every `extension_ui_response` pecan writes to stdin, proving
    //     the answers Pecan built reached the worker on the wire
    //   - dumps its argv so the test can assert the spawn flags
    const fs = require("node:fs");
    const readline = require("node:readline");
    const RESULTS = "__RESULTS__";
    const ARGV = "__ARGV__";
    const SESSION_ID = "__SESSION_ID__";
    const FRAMES = [
    __FRAMES__
    ];
    fs.writeFileSync(ARGV, JSON.stringify(process.argv));
    let prompted = false;
    const rl = readline.createInterface({ input: process.stdin, terminal: false });
    rl.on("line", (line) => {
      if (!line.trim()) return;
      let cmd;
      try { cmd = JSON.parse(line); } catch { return; }
      if (cmd.type === "get_state") {
        process.stdout.write(JSON.stringify({ type: "response", id: cmd.id, success: true, data: { sessionId: SESSION_ID, isStreaming: false } }) + "\n");
        return;
      }
      if (cmd.type === "prompt" && !prompted) {
        prompted = true;
        for (const frame of FRAMES) process.stdout.write(JSON.stringify(frame) + "\n");
        process.stdout.write(JSON.stringify({ type: "response", id: cmd.id, command: "prompt", success: true }) + "\n");
        return;
      }
      if (cmd.type === "extension_ui_response") {
        fs.appendFileSync(RESULTS, line.trim() + "\n");
      }
    });
    rl.on("close", () => process.exit(0));
    "#
        .replace("__RESULTS__", &format!("{}", results.display()))
        .replace("__ARGV__", &format!("{}", argv.display()))
        .replace("__SESSION_ID__", session_id)
        .replace("__FRAMES__", &frames);
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
            "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {CLI_TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body_text}",
            body_text.len(),
        );
        stream.write_all(request.as_bytes())?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response)?;
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

    #[tokio::test(flavor = "multi_thread")]
    async fn extension_dialogs_appear_in_pecan_and_resolve_back_to_pi() {
        let env = FixtureEnv::new();
        let fake_pi = write_fake_pi(&env);
        let responses_path = env.path("responses.jsonl");

        // Pick a free loopback port deterministically (the server prints its
        // bound URL to stdout, which is block-buffered when piped).
        let port = {
            let listener =
                tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind probe port");
            listener.local_addr().expect("probe addr").port()
        };
        let addr = format!("127.0.0.1:{port}");

        let mut path = fake_pi.parent().expect("fake pi parent").as_os_str().to_owned();
        path.push(":");
        path.push(std::env::var("PATH").unwrap_or_default());
        let token_file = env.agent_dir().join("pecan").join("cli-token");
        std::fs::create_dir_all(token_file.parent().expect("token dir")).expect("create pecan dir");
        std::fs::write(&token_file, CLI_TOKEN).expect("seed cli token");
        let mut pecan = tokio::process::Command::new(env!("CARGO_BIN_EXE_pecan"))
            .args(["serve", "--port", &port.to_string(), "--no-open"])
            .env("PATH", path)
            .env("PECAN_AGENT_DIR", env.agent_dir())
            // Explicit extension loads must reach the worker as `--extension`
            // flags; the fake pi dumps its argv so the test can verify.
            .env(
                "PECAN_PI_EXTENSIONS",
                "/ext/pi-askuser/index.ts,/ext/other-extension.ts",
            )
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
            tokio::io::AsyncReadExt::read_to_end(&mut pecan_stdout, &mut sink)
                .await
                .unwrap_or_default();
        });
        let stderr_drain = tokio::spawn(async move {
            let mut sink = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(&mut pecan_stderr, &mut sink)
                .await
                .unwrap_or_default();
        });

        // 1. Wait for the API to come up, then create a session. The fake pi
        //    answers get_state with its session id so new_session completes.
        let _ = poll_sk(&addr, "/api/bootstrap", Duration::from_secs(30), |body| {
            body.get("ok").is_some() || body.get("seeded").is_some() || !body.is_null()
        })
        .await;
        let (status, created) = http_json(
            &addr,
            "POST",
            "/api/session/new",
            Some(&json!({ "cwd": env.path("project") })),
        )
        .expect("post new session");
        assert_eq!(status, 200, "new session failed: {created}");
        assert_eq!(
            created.get("id"),
            Some(&json!("fixture-session-1")),
            "session id must come from the worker state: {created}",
        );
        // 1b. The worker's argv carries each `PECAN_PI_EXTENSIONS` entry as an
        //     explicit `pi --extension` flag, in order. The fake pi writes
        //     argv.json synchronously at startup (before answering get_state),
        //     so it always exists once the session was created.
        let raw_argv = {
            let text =
                std::fs::read_to_string(env.path("argv.json")).expect("read fake pi argv dump");
            text.trim().to_owned()
        };
        let args: Vec<String> = serde_json::from_str::<Vec<String>>(&raw_argv)
            .expect("argv dumped as a json string array");
        let mut found: Vec<String> = Vec::new();
        for (index, arg) in args.iter().enumerate() {
            if arg == "--extension" {
                found.push(args.get(index + 1).cloned().unwrap_or_default());
            }
        }
        assert_eq!(
            found,
            vec!["/ext/pi-askuser/index.ts".to_owned(), "/ext/other-extension.ts".to_owned(),],
            "each PECAN_PI_EXTENSIONS entry must be passed as --extension",
        );

        // 2. Prompt once; the fake worker answers by emitting the four blocking
        //    extension dialog requests, which Pecan records for this session.
        let (status, accepted) = http_json(
            &addr,
            "POST",
            "/api/session/fixture-session-1/message",
            Some(&json!({ "text": "hello", "mode": "send" })),
        )
        .expect("post message");
        assert_eq!(status, 200, "message failed: {accepted}");

        // 3. The requests appear through Pecan: GET /asks lists all four with
        //    exact ids and the renderable fields preserved.
        let asks = poll_sk(
            &addr,
            "/api/session/fixture-session-1/asks",
            Duration::from_secs(30),
            |body| body.get("asks").and_then(Value::as_array).is_some_and(|a| a.len() == 4),
        )
        .await;
        let listed = asks["asks"].as_array().expect("asks array").clone();
        let by_id: std::collections::HashMap<&str, &Value> =
            listed.iter().map(|ask| (ask["id"].as_str().expect("ask id"), ask)).collect();
        let mut methods = listed
            .iter()
            .map(|ask| ask["method"].as_str().expect("ask method").to_owned())
            .collect::<Vec<_>>();
        methods.sort();
        assert_eq!(
            methods,
            vec!["confirm", "editor", "input", "select"],
            "all four dialog methods must be listed: {listed:?}",
        );
        assert_eq!(by_id["uuid-select"].get("options"), Some(&json!(["Allow", "Block"])));
        assert_eq!(by_id["uuid-select"].get("title"), Some(&json!("Pick an option")));
        assert_eq!(
            by_id["uuid-confirm"].get("message"),
            Some(&json!("All messages will be lost."))
        );
        assert_eq!(by_id["uuid-input"].get("placeholder"), Some(&json!("type something...")));
        assert_eq!(by_id["uuid-editor"].get("prefill"), Some(&json!("Line 1\nLine 2")));

        // 4. Answer each dialog exactly as the web host does, through `/respond`.
        let answers: &[(&str, Value)] = &[
            ("uuid-select", json!({"requestId": "uuid-select", "value": "Allow"})),
            ("uuid-confirm", json!({"requestId": "uuid-confirm", "confirmed": true})),
            ("uuid-input", json!({"requestId": "uuid-input", "value": "typed answer"})),
            ("uuid-editor", json!({"requestId": "uuid-editor", "value": "Edited\nLine 3"})),
        ];
        for (id, payload) in answers {
            let (status, body) =
                http_json(&addr, "POST", "/api/session/fixture-session-1/respond", Some(payload))
                    .expect("post respond");
            assert_eq!(status, 200, "respond {id} failed: {body}");
        }

        // 5. The resolutions reach the worker on the wire: the fake pi records
        //    every `extension_ui_response` frame Pecan wrote to its stdin, and
        //    each frame echoes the exact request id with the answer field.
        let lines = poll_sk_jsonl(&responses_path, 4, Duration::from_secs(15)).await;
        let wire: std::collections::HashMap<String, Value> = lines
            .into_iter()
            .map(|frame| (frame["id"].as_str().expect("response id").to_owned(), frame))
            .collect();
        assert_eq!(wire["uuid-select"].get("type"), Some(&json!("extension_ui_response")));
        assert_eq!(wire["uuid-select"].get("value"), Some(&json!("Allow")));
        assert_eq!(wire["uuid-confirm"].get("confirmed"), Some(&json!(true)));
        assert_eq!(wire["uuid-input"].get("value"), Some(&json!("typed answer")));
        assert_eq!(wire["uuid-editor"].get("value"), Some(&json!("Edited\nLine 3")));
        assert_eq!(wire.len(), 4, "unexpected extra frames: {wire:?}");

        // 6. Answering is complete: the registry is empty again for the session.
        let drained = poll_sk(
            &addr,
            "/api/session/fixture-session-1/asks",
            Duration::from_secs(10),
            |body| body.get("asks").and_then(Value::as_array).is_some_and(Vec::is_empty),
        )
        .await;
        assert_eq!(drained["asks"], json!([]));

        pecan.kill().await.unwrap_or_default();
        let _wait = pecan.wait().await;
        stdout_drain.await.unwrap_or_default();
        stderr_drain.await.unwrap_or_default();
    }

    /// Reads a JSONL file until it holds at least `expected` records.
    async fn poll_sk_jsonl(path: &Path, expected: usize, deadline: Duration) -> Vec<Value> {
        let start = std::time::Instant::now();
        loop {
            let lines = std::fs::read_to_string(path).unwrap_or_default();
            let frames = lines
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .collect::<Vec<_>>();
            if frames.len() >= expected {
                return frames;
            }
            assert!(
                start.elapsed() < deadline,
                "timed out waiting for {expected} response frames in {} (have {})",
                path.display(),
                frames.len(),
            );
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }
}
