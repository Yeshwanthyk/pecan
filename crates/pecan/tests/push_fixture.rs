//! Web push end to end against a real `pecan serve` and a fake push service
//! (admitted through `PECAN_PUSH_EXTRA_ORIGIN`): only paired browsers
//! subscribe, endpoints outside known push services are refused,
//! `pecan push test` sends a VAPID-signed payload-less push that verifies
//! against the advertised key, the service worker drains the queued notice
//! once, and a subscription the service reports gone (410) is dropped.

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::process::Stdio;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use p256::ecdsa::signature::Verifier as _;
    use p256::ecdsa::{Signature, VerifyingKey};
    use serde_json::{Value, json};

    const CLI_TOKEN: &str = "fixture-cli-token-0123456789abcdef0123456789";

    /// Request line plus lower-cased headers the fake push service received.
    type Seen = Arc<Mutex<Vec<(String, Vec<(String, String)>)>>>;

    /// Answers `/gone/*` with 410 and everything else with 201.
    fn fake_push_service() -> (String, Seen) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake push");
        let origin = format!("http://{}", listener.local_addr().expect("fake addr"));
        let seen: Seen = Arc::default();
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut headers = Vec::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        headers.push((name.trim().to_lowercase(), value.trim().to_owned()));
                    }
                }
                let status =
                    if request_line.contains(" /gone/") { "410 Gone" } else { "201 Created" };
                log.lock().expect("log").push((request_line.trim().to_owned(), headers));
                let reply =
                    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                stream.write_all(reply.as_bytes()).unwrap_or_default();
            }
        });
        (origin, seen)
    }

    fn request(
        addr: &str,
        auth: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> (u16, String, Value) {
        let mut stream = std::net::TcpStream::connect(addr).expect("connect");
        stream.set_read_timeout(Some(Duration::from_secs(15))).expect("read timeout");
        let body_text = body.map(Value::to_string).unwrap_or_default();
        let raw = format!(
            "{method} {path} HTTP/1.1\r\nHost: {addr}\r\n{auth}User-Agent: Mozilla/5.0 (iPhone) Version/18.0 Safari/605.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body_text}",
            body_text.len(),
        );
        stream.write_all(raw.as_bytes()).expect("write request");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).expect("read response");
        let text = String::from_utf8_lossy(&response);
        let (head, rest) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let status =
            head.split_whitespace().nth(1).and_then(|code| code.parse().ok()).expect("status");
        (status, head.to_owned(), serde_json::from_str(rest).unwrap_or_else(|_| json!({})))
    }

    fn cli() -> String {
        format!("Authorization: Bearer {CLI_TOKEN}\r\n")
    }

    fn cookie(value: &str) -> String {
        format!("Cookie: {value}\r\n")
    }

    fn pecan_cli(agent_dir: &PathBuf, addr: &str, args: &[&str]) -> (bool, Value, String) {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_pecan"))
            .args(args)
            .env("PECAN_AGENT_DIR", agent_dir)
            .env("PECAN_SERVER_URL", format!("http://{addr}"))
            .env_remove("PECAN_TOKEN")
            .output()
            .expect("run pecan cli");
        let body = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| json!({}));
        (output.status.success(), body, String::from_utf8_lossy(&output.stderr).into_owned())
    }

    /// Pairs a fresh browser and returns its `(cookie, device id)`.
    fn pair_device(agent_dir: &PathBuf, addr: &str) -> (String, String) {
        let (ok, minted, stderr) = pecan_cli(agent_dir, addr, &["pair", "--json"]);
        assert!(ok, "pecan pair: {stderr}");
        let code = minted["code"].as_str().expect("code");
        let (status, head, body) =
            request(addr, "", "POST", "/api/pair", Some(&json!({ "code": code })));
        assert_eq!(status, 200, "{body}");
        let pair = head
            .lines()
            .find_map(|line| line.strip_prefix("set-cookie: "))
            .and_then(|value| value.split(';').next())
            .expect("device cookie")
            .to_owned();
        (pair, body["device"]["id"].as_str().expect("device id").to_owned())
    }

    /// Checks `Authorization: vapid t=<jwt>, k=<key>` is an ES256 token for
    /// `audience` signed by `public_key`.
    fn assert_vapid(authorization: &str, public_key: &str, audience: &str) {
        let (token, key) = authorization
            .strip_prefix("vapid t=")
            .and_then(|rest| rest.split_once(", k="))
            .expect("vapid t=..., k=...");
        assert_eq!(key, public_key);
        let (unsigned, signature) = token.rsplit_once('.').expect("signed jwt");
        let signature = Signature::from_slice(&URL_SAFE_NO_PAD.decode(signature).expect("sig b64"))
            .expect("sig");
        let verifier =
            VerifyingKey::from_sec1_bytes(&URL_SAFE_NO_PAD.decode(key).expect("key b64"))
                .expect("key");
        assert!(
            verifier.verify(unsigned.as_bytes(), &signature).is_ok(),
            "VAPID signature verifies"
        );
        let claims = unsigned.split('.').nth(1).expect("claims");
        let claims: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims).expect("claims b64"))
                .expect("claims json");
        assert_eq!(claims["aud"], audience);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn subscribed_devices_get_signed_pushes_and_gone_ones_are_dropped() {
        let root = std::env::temp_dir().join(format!(
            "pecan-push-fixture-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos(),
        ));
        let agent_dir = root.join("agent");
        std::fs::create_dir_all(agent_dir.join("sessions")).expect("sessions dir");
        std::fs::create_dir_all(agent_dir.join("pecan")).expect("pecan dir");
        std::fs::write(agent_dir.join("pecan").join("cli-token"), CLI_TOKEN).expect("seed token");
        let (push_origin, seen) = fake_push_service();

        let port =
            TcpListener::bind("127.0.0.1:0").expect("probe").local_addr().expect("addr").port();
        let addr = format!("127.0.0.1:{port}");
        let mut pecan = tokio::process::Command::new(env!("CARGO_BIN_EXE_pecan"))
            .args(["serve", "--port", &port.to_string(), "--no-open"])
            .env("PECAN_AGENT_DIR", &agent_dir)
            .env("PECAN_PUSH_EXTRA_ORIGIN", &push_origin)
            .env("RUST_LOG", "pecan=warn")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn pecan serve");
        let start = Instant::now();
        while std::net::TcpStream::connect(&addr).is_err() {
            assert!(start.elapsed() < Duration::from_secs(30), "server never listened");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Unpaired callers cannot read the key or subscribe.
        let (status, _, _) = request(&addr, "", "GET", "/api/push/key", None);
        assert_eq!(status, 401);

        let (phone, phone_id) = pair_device(&agent_dir, &addr);
        let (tablet, _) = pair_device(&agent_dir, &addr);
        let (status, _, key) = request(&addr, &cookie(&phone), "GET", "/api/push/key", None);
        assert_eq!(status, 200, "{key}");
        assert_eq!(key["subscribed"], false);
        let public_key = key["publicKey"].as_str().expect("public key").to_owned();
        assert_eq!(URL_SAFE_NO_PAD.decode(&public_key).expect("b64").len(), 65);

        // Endpoints must be a known push service; the CLI has no subscription.
        for bad in ["https://evil.example/push", "http://fcm.googleapis.com/x", "nonsense"] {
            let (status, _, body) = request(
                &addr,
                &cookie(&phone),
                "PUT",
                "/api/push/subscription",
                Some(&json!({ "endpoint": bad })),
            );
            assert_eq!(status, 400, "{bad}: {body}");
        }
        let endpoint = format!("{push_origin}/ok/phone");
        let (status, _, _) = request(
            &addr,
            &cli(),
            "PUT",
            "/api/push/subscription",
            Some(&json!({ "endpoint": endpoint })),
        );
        assert_eq!(status, 403);
        let (status, _, _) =
            request(&addr, &cookie(&phone), "GET", "/api/push/subscriptions", None);
        assert_eq!(status, 403, "devices cannot list every subscription");

        let (status, _, body) = request(
            &addr,
            &cookie(&phone),
            "PUT",
            "/api/push/subscription",
            Some(&json!({ "endpoint": endpoint })),
        );
        assert_eq!(status, 200, "{body}");
        let (status, _, body) = request(
            &addr,
            &cookie(&tablet),
            "PUT",
            "/api/push/subscription",
            Some(&json!({ "endpoint": format!("{push_origin}/gone/tablet") })),
        );
        assert_eq!(status, 200, "{body}");
        let (_, _, key) = request(&addr, &cookie(&phone), "GET", "/api/push/key", None);
        assert_eq!(key["subscribed"], true);

        let (ok, listed, stderr) = pecan_cli(&agent_dir, &addr, &["push", "--json"]);
        assert!(ok, "{stderr}");
        assert_eq!(listed["subscriptions"].as_array().map(Vec::len), Some(2), "{listed}");

        // One test fans out: the phone is sent, the tablet's endpoint is gone.
        let (ok, report, stderr) = pecan_cli(&agent_dir, &addr, &["push", "test", "--json"]);
        assert!(ok, "{stderr} {report}");
        assert_eq!(
            (report["sent"].as_u64(), report["removed"].as_u64(), report["failed"].as_u64()),
            (Some(1), Some(1), Some(0)),
            "{report}"
        );
        {
            let seen = seen.lock().expect("seen");
            let (line, headers) =
                seen.iter().find(|(line, _)| line.contains("/ok/phone")).expect("phone push");
            assert!(line.starts_with("POST "), "{line}");
            let header = |name: &str| {
                headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
            };
            assert_eq!(header("content-length"), Some("0"), "payload-less");
            assert!(header("ttl").is_some());
            assert_vapid(
                header("authorization").expect("authorization"),
                &public_key,
                &push_origin,
            );
        }
        let (_, listed, _) = pecan_cli(&agent_dir, &addr, &["push", "--json"]);
        let remaining = listed["subscriptions"].as_array().cloned().unwrap_or_default();
        assert_eq!(remaining.len(), 1, "{listed}");
        assert_eq!(remaining[0]["deviceId"], phone_id.as_str());

        // The service worker drains what the push was for, exactly once.
        let (status, _, pending) =
            request(&addr, &cookie(&phone), "GET", "/api/push/pending", None);
        assert_eq!(status, 200);
        let notices = pending["notifications"].as_array().cloned().unwrap_or_default();
        assert_eq!(notices.len(), 1, "{pending}");
        assert_eq!(notices[0]["tag"], "pecan:test");
        assert_eq!(notices[0]["url"], "/#/settings");
        let (_, _, pending) = request(&addr, &cookie(&phone), "GET", "/api/push/pending", None);
        assert_eq!(pending["notifications"], json!([]));

        // After unsubscribing there is nothing to push to.
        let (_, _, removed) =
            request(&addr, &cookie(&phone), "DELETE", "/api/push/subscription", None);
        assert_eq!(removed["removed"], true);
        let (ok, report, _) = pecan_cli(&agent_dir, &addr, &["push", "test", "--json"]);
        assert!(ok);
        assert_eq!(report["sent"], 0);

        pecan.kill().await.expect("kill pecan");
        std::fs::remove_dir_all(&root).unwrap_or_default();
    }
}
