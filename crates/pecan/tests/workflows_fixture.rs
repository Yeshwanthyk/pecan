//! pi-subagents workflow fixture: `pecan seed` writes a parent session whose
//! `workflow` tool result names a run, the run's event journal, and one child
//! session named `workflow:<runId>: <task label>`. The served thread view must
//! fold the journal into tasks and link the task to its child transcript, and
//! a scoped server must keep that child and resolve its parent through the
//! run id alone.

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::path::PathBuf;
    use std::process::Stdio;
    use std::time::Duration;

    use serde_json::{Value, json};

    /// Pre-seeded CLI bearer token the spawned server adopts.
    const CLI_TOKEN: &str = "fixture-cli-token-0123456789abcdef0123456789";
    const PARENT: &str = "00000000-0000-4000-8000-000000000001";
    const WORKFLOW_CHILD: &str = "00000000-0000-4000-8000-000000000007";
    const PLAIN_SESSION: &str = "00000000-0000-4000-8000-000000000002";

    /// Unique per-run seed root; the name contains `test-ground` so the seed
    /// command accepts it.
    struct FixtureEnv {
        root: PathBuf,
    }

    impl FixtureEnv {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "pecan-workflows-test-ground-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("system clock before unix epoch")
                    .as_nanos(),
            ));
            std::fs::create_dir_all(&root).expect("create fixture root");
            Self { root }
        }
    }

    impl Drop for FixtureEnv {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).unwrap_or_default();
        }
    }

    /// Minimal HTTP/1.1 GET (no extra dependencies): returns (status, body).
    fn get_json(addr: &str, path: &str) -> Result<(u16, Value), std::io::Error> {
        let mut stream = std::net::TcpStream::connect(addr)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {CLI_TOKEN}\r\nConnection: close\r\n\r\n",
        );
        stream.write_all(request.as_bytes())?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response)?;
        let text = String::from_utf8_lossy(&response);
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let status =
            head.split_whitespace().nth(1).and_then(|code| code.parse::<u16>().ok()).unwrap_or(0);
        Ok((status, serde_json::from_str(body).unwrap_or_else(|_| json!({}))))
    }

    async fn get_ok(addr: &str, path: &str) -> Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok((200, body)) = get_json(addr, path) {
                return body;
            }
            assert!(std::time::Instant::now() < deadline, "timed out waiting for {path}");
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    /// Seeds a fresh fixture and serves it with `extra` serve args.
    async fn seeded_server(env: &FixtureEnv, extra: &[&str]) -> (tokio::process::Child, String) {
        let seeded = std::process::Command::new(env!("CARGO_BIN_EXE_pecan"))
            .arg("seed")
            .arg(&env.root)
            .output()
            .expect("run pecan seed");
        assert!(
            seeded.status.success(),
            "seed failed: {}",
            String::from_utf8_lossy(&seeded.stderr)
        );

        let agent_dir = env.root.join("agent");
        std::fs::create_dir_all(agent_dir.join("pecan")).expect("create pecan dir");
        std::fs::write(agent_dir.join("pecan").join("cli-token"), CLI_TOKEN).expect("seed token");
        let port = {
            let listener =
                tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind probe port");
            listener.local_addr().expect("probe addr").port()
        };
        let pecan = tokio::process::Command::new(env!("CARGO_BIN_EXE_pecan"))
            .args(["serve", "--port", &port.to_string(), "--no-open"])
            .args(extra)
            .env("PECAN_AGENT_DIR", &agent_dir)
            .env("RUST_LOG", "pecan=warn")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn pecan serve");
        (pecan, format!("127.0.0.1:{port}"))
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn workflow_tasks_link_to_child_sessions() {
        let env = FixtureEnv::new();
        let (mut pecan, addr) = seeded_server(&env, &[]).await;

        let parent = get_ok(&addr, &format!("/api/session/{PARENT}")).await;
        let tasks = &parent["workflows"][0]["tasks"];
        assert_eq!(parent["workflows"][0]["runId"], json!("wf-seed0001"), "{parent}");
        assert_eq!(tasks[0]["id"], json!("fix"));
        assert_eq!(tasks[0]["status"], json!("completed"));
        assert_eq!(tasks[0]["sessionId"], json!(WORKFLOW_CHILD), "{tasks}");
        assert_eq!(tasks[1]["id"], json!("review"));
        assert_eq!(tasks[1]["status"], json!("running"));
        assert_eq!(tasks[1]["sessionId"], Value::Null, "no child transcript for review");

        let entries = parent["entries"].as_array().cloned().unwrap_or_default();
        let kind =
            |k: &str| entries.iter().find(|dated| dated["entry"]["kind"] == json!(k)).cloned();
        let asked = kind("childQuestions").expect("ask_parent question is rendered");
        let question = &asked["entry"]["questions"][0];
        assert_eq!(question["requestId"], json!("pq-1"), "{asked}");
        assert_eq!(question["answered"], json!(true), "reply by requestId marks it answered");
        let handoff = kind("childResults").expect("result batch is rendered");
        assert_eq!(handoff["entry"]["results"][0]["status"], json!("done"), "{handoff}");

        let child = get_ok(&addr, &format!("/api/session/{WORKFLOW_CHILD}")).await;
        assert_eq!(child["summary"]["kind"], json!("subagent"), "{child}");
        assert_eq!(child["summary"]["agentName"], json!("workflow:wf-seed0001: Fix queue race"));

        let plain = get_ok(&addr, &format!("/api/session/{PLAIN_SESSION}")).await;
        assert_eq!(plain["summary"]["kind"], json!("normal"));
        assert_eq!(plain["workflows"], json!([]), "sessions without runs show no workflows");

        pecan.start_kill().unwrap_or_default();
        pecan.wait().await.unwrap_or_default();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn scoped_serve_keeps_workflow_child_and_links_parent() {
        let env = FixtureEnv::new();
        let (mut pecan, addr) = seeded_server(&env, &["--session", PARENT]).await;

        let sessions = get_ok(&addr, "/api/sessions").await;
        let rows = sessions["sessions"].as_array().cloned().unwrap_or_default();
        let ids: Vec<&str> = rows.iter().filter_map(|row| row["id"].as_str()).collect();
        assert_eq!(
            ids,
            vec![PARENT, WORKFLOW_CHILD],
            "scope keeps only the run's child, listed after its thread: {sessions}"
        );
        assert_eq!(rows[1]["parentSessionId"], json!(PARENT), "linked via run id: {sessions}");

        let children = get_ok(&addr, "/api/sessions?kind=subagent").await;
        let child_ids: Vec<&str> = children["sessions"]
            .as_array()
            .map(|rows| rows.iter().filter_map(|row| row["id"].as_str()).collect())
            .unwrap_or_default();
        assert_eq!(child_ids, vec![WORKFLOW_CHILD], "kind filter keeps only children: {children}");
        let (status, _) =
            get_json(&addr, "/api/sessions?kind=robot").expect("request unknown session kind");
        assert_eq!(status, 400, "unknown kinds are rejected at the boundary");

        let (status, _) = get_json(&addr, &format!("/api/session/{PLAIN_SESSION}"))
            .expect("request out-of-scope session");
        assert_eq!(status, 404, "sessions outside the scope are not served");

        pecan.start_kill().unwrap_or_default();
        pecan.wait().await.unwrap_or_default();
    }
}
