//! `pecan remote` against a fake `tailscale` (via `PECAN_TAILSCALE`): it reads
//! status without side effects, `--enable` issues exactly one
//! `tailscale serve` for the Pecan port, and it never replaces a mapping that
//! proxies somewhere else.

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use serde_json::Value;

    /// Fake `tailscale`: `$FAKE_TS_MODE` picks the world (`off`, `ours`,
    /// `conflict`, `stopped`); every invocation's argv is appended to `calls`.
    const FAKE: &str = r#"#!/bin/sh
echo "$*" >> "$FAKE_TS_DIR/calls"
case "$1 $2" in
  "status --json")
    if [ "$FAKE_TS_MODE" = stopped ]; then state=Stopped; else state=Running; fi
    printf '{"BackendState":"%s","Self":{"DNSName":"box.tail0.ts.net."}}' "$state" ;;
  "serve status")
    case "$FAKE_TS_MODE" in
      ours) printf '{"Web":{"box.tail0.ts.net:7700":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:7700"}}}}}' ;;
      conflict) printf '{"Web":{"box.tail0.ts.net:7700":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:3000"}}}}}' ;;
      *) printf '{}' ;;
    esac ;;
  "serve --bg") ;;
  *) echo "unexpected: $*" >&2; exit 1 ;;
esac
"#;

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pecan-remote-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("fixture dir");
        let script = dir.join("tailscale");
        std::fs::write(&script, FAKE).expect("write fake tailscale");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        dir
    }

    fn remote(dir: &Path, mode: &str, extra: &[&str]) -> (bool, Value, String) {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_pecan"))
            .args(["remote", "--port", "7700", "--json"])
            .args(extra)
            .env("PECAN_TAILSCALE", dir.join("tailscale"))
            .env("FAKE_TS_DIR", dir)
            .env("FAKE_TS_MODE", mode)
            .output()
            .expect("run pecan remote");
        let json = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
        (output.status.success(), json, String::from_utf8_lossy(&output.stderr).into_owned())
    }

    fn calls(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn status_is_read_only_and_enable_serves_the_pecan_port() {
        let dir = fixture("enable");
        let (ok, report, stderr) = remote(&dir, "off", &[]);
        assert!(ok, "{stderr}");
        assert_eq!(report["url"], "https://box.tail0.ts.net:7700");
        assert_eq!(report["state"], "off");
        assert!(
            calls(&dir).iter().all(|call| !call.starts_with("serve --bg")),
            "status must not change config"
        );

        let (ok, report, stderr) = remote(&dir, "off", &["--enable"]);
        assert!(ok, "{stderr}");
        assert_eq!(report["state"], "serving");
        let serves: Vec<_> =
            calls(&dir).into_iter().filter(|call| call.starts_with("serve --bg")).collect();
        assert_eq!(serves, ["serve --bg --https=7700 http://127.0.0.1:7700"]);

        // Already ours: enabling again is a no-op.
        let (ok, report, _) = remote(&dir, "ours", &["--enable"]);
        assert!(ok);
        assert_eq!(report["state"], "serving");
        assert_eq!(calls(&dir).iter().filter(|call| call.starts_with("serve --bg")).count(), 1);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn refuses_to_clobber_other_mappings_or_run_while_stopped() {
        let dir = fixture("refuse");
        let (ok, _, stderr) = remote(&dir, "conflict", &["--enable"]);
        assert!(!ok);
        assert!(stderr.contains("http://127.0.0.1:3000"), "{stderr}");
        let (ok, _, stderr) = remote(&dir, "stopped", &[]);
        assert!(!ok);
        assert!(stderr.contains("Stopped"), "{stderr}");
        assert!(calls(&dir).iter().all(|call| !call.starts_with("serve --bg")));

        let (ok, _, stderr) = std::process::Command::new(env!("CARGO_BIN_EXE_pecan"))
            .args(["remote"])
            .env("PECAN_TAILSCALE", dir.join("missing-tailscale"))
            .output()
            .map(|out| {
                (out.status.success(), (), String::from_utf8_lossy(&out.stderr).into_owned())
            })
            .expect("run pecan remote");
        assert!(!ok);
        assert!(stderr.contains("cannot run"), "{stderr}");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
