//! Device pairing end to end against a real `pecan serve`: unpaired browsers
//! are refused, a one-time code from the CLI (`pecan pair`) buys a device
//! cookie, devices cannot reach CLI-only routes, and `pecan devices revoke`
//! both invalidates the cookie and closes that device's live event stream.

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::path::PathBuf;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    use serde_json::{Value, json};

    /// Pre-seeded CLI bearer token the spawned server adopts.
    const CLI_TOKEN: &str = "fixture-cli-token-0123456789abcdef0123456789";

    /// Who a raw request authenticates as.
    #[derive(Clone, Copy)]
    enum As<'a> {
        Nobody,
        Cli,
        Cookie(&'a str),
    }

    impl As<'_> {
        fn header(self) -> String {
            match self {
                Self::Nobody => String::new(),
                Self::Cli => format!("Authorization: Bearer {CLI_TOKEN}\r\n"),
                Self::Cookie(cookie) => format!("Cookie: {cookie}\r\n"),
            }
        }
    }

    struct Response {
        status: u16,
        head: String,
        body: Value,
    }

    fn request(
        addr: &str,
        who: As<'_>,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Response {
        let mut stream = std::net::TcpStream::connect(addr).expect("connect");
        stream.set_read_timeout(Some(Duration::from_secs(5))).expect("read timeout");
        let body_text = body.map(Value::to_string).unwrap_or_default();
        let raw = format!(
            "{method} {path} HTTP/1.1\r\nHost: {addr}\r\n{}User-Agent: Mozilla/5.0 (iPhone) Version/18.0 Safari/605.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body_text}",
            who.header(),
            body_text.len(),
        );
        stream.write_all(raw.as_bytes()).expect("write request");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).expect("read response");
        let text = String::from_utf8_lossy(&response);
        let (head, rest) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let status =
            head.split_whitespace().nth(1).and_then(|code| code.parse().ok()).expect("status code");
        Response {
            status,
            head: head.to_owned(),
            body: serde_json::from_str(rest).unwrap_or_else(|_| json!({})),
        }
    }

    /// `name=value` from the response's `Set-Cookie` header.
    fn cookie_pair(head: &str) -> Option<String> {
        head.lines()
            .find_map(|line| line.strip_prefix("set-cookie: "))
            .and_then(|value| value.split(';').next())
            .map(str::to_owned)
    }

    fn pecan_cli(agent_dir: &PathBuf, addr: &str, args: &[&str]) -> (bool, Value) {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_pecan"))
            .args(args)
            .env("PECAN_AGENT_DIR", agent_dir)
            .env("PECAN_SERVER_URL", format!("http://{addr}"))
            .env_remove("PECAN_TOKEN")
            .output()
            .expect("run pecan cli");
        let body = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| json!({}));
        (output.status.success(), body)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pairing_gates_every_route_and_revocation_ends_streams() {
        let root = std::env::temp_dir().join(format!(
            "pecan-pairing-fixture-{}-{}",
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

        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("probe port")
            .local_addr()
            .expect("probe addr")
            .port();
        let addr = format!("127.0.0.1:{port}");
        let mut pecan = tokio::process::Command::new(env!("CARGO_BIN_EXE_pecan"))
            .args(["serve", "--port", &port.to_string(), "--no-open"])
            .env("PECAN_AGENT_DIR", &agent_dir)
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

        // Unpaired: every API route, including the live stream, says so.
        for path in ["/api/bootstrap", "/api/events"] {
            let refused = request(&addr, As::Nobody, "GET", path, None);
            assert_eq!(refused.status, 401, "{path}: {}", refused.body);
            assert_eq!(refused.body["code"], "unpaired", "{path}: {}", refused.body);
        }
        let forged = request(&addr, As::Cookie("pecan_x=deadbeef"), "GET", "/api/bootstrap", None);
        assert_eq!(forged.status, 401);

        // A wrong code is refused and does not set a cookie.
        let wrong =
            request(&addr, As::Nobody, "POST", "/api/pair", Some(&json!({ "code": "ZZZZ-ZZZZ" })));
        assert_eq!(wrong.status, 401, "{}", wrong.body);
        assert!(cookie_pair(&wrong.head).is_none());

        // Minting is CLI-only.
        let anonymous_mint = request(&addr, As::Nobody, "POST", "/api/pair/code", Some(&json!({})));
        assert_eq!(anonymous_mint.status, 401);

        let (ok, minted) = pecan_cli(&agent_dir, &addr, &["pair", "--json"]);
        assert!(ok, "pecan pair failed: {minted}");
        let code = minted["code"].as_str().expect("code").to_owned();
        assert!(minted["url"].as_str().is_some_and(|url| url.ends_with(&format!("?pair={code}"))));

        // Redeeming (lower-case, no dash) pairs this browser.
        let normalized = code.replace('-', "").to_lowercase();
        let paired =
            request(&addr, As::Nobody, "POST", "/api/pair", Some(&json!({ "code": normalized })));
        assert_eq!(paired.status, 200, "{}", paired.body);
        assert_eq!(paired.body["device"]["name"], "Safari on iPhone");
        let set_cookie = paired.head.to_lowercase();
        assert!(set_cookie.contains("httponly") && set_cookie.contains("samesite=strict"));
        let cookie = cookie_pair(&paired.head).expect("device cookie");
        let device_id = paired.body["device"]["id"].as_str().expect("device id").to_owned();

        // Single use.
        let replay =
            request(&addr, As::Nobody, "POST", "/api/pair", Some(&json!({ "code": code })));
        assert_eq!(replay.status, 401);

        let bootstrap = request(&addr, As::Cookie(&cookie), "GET", "/api/bootstrap", None);
        assert_eq!(bootstrap.status, 200, "{}", bootstrap.body);
        let device_listing = request(&addr, As::Cookie(&cookie), "GET", "/api/devices", None);
        assert_eq!(device_listing.status, 403, "devices must not manage devices");
        let cli_listing = request(&addr, As::Cli, "GET", "/api/devices", None);
        assert_eq!(cli_listing.status, 200, "{}", cli_listing.body);

        let (ok, listed) = pecan_cli(&agent_dir, &addr, &["devices", "--json"]);
        assert!(ok, "pecan devices failed: {listed}");
        assert!(
            listed["devices"]
                .as_array()
                .is_some_and(|devices| devices.iter().any(|device| device["id"] == device_id)),
            "{listed}",
        );

        // Open the device's live stream, then revoke: the stream must end.
        let mut stream = std::net::TcpStream::connect(&addr).expect("connect events");
        stream.set_read_timeout(Some(Duration::from_secs(10))).expect("events timeout");
        write!(
            stream,
            "GET /api/events HTTP/1.1\r\nHost: {addr}\r\nCookie: {cookie}\r\nAccept: text/event-stream\r\nConnection: close\r\n\r\n"
        )
        .expect("write events request");
        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line).expect("events status");
        assert!(status_line.contains(" 200 "), "{status_line}");

        let (ok, revoked) =
            pecan_cli(&agent_dir, &addr, &["devices", "revoke", &device_id, "--json"]);
        assert!(ok, "revoke failed: {revoked}");
        let mut rest = Vec::new();
        reader.read_to_end(&mut rest).expect("revoked stream must close, not time out");

        let after = request(&addr, As::Cookie(&cookie), "GET", "/api/bootstrap", None);
        assert_eq!(after.status, 401);
        assert_eq!(after.body["code"], "unpaired");
        let (ok, _) = pecan_cli(&agent_dir, &addr, &["devices", "revoke", &device_id, "--json"]);
        assert!(!ok, "revoking an unknown device must fail");

        // Without the CLI token the CLI is just another unpaired client.
        std::fs::remove_file(agent_dir.join("pecan").join("cli-token")).expect("drop token");
        let (ok, _) = pecan_cli(&agent_dir, &addr, &["pair", "--json"]);
        assert!(!ok, "pair without the CLI token must fail");

        pecan.kill().await.expect("kill pecan");
        std::fs::remove_dir_all(&root).expect("remove fixture root");
    }
}
