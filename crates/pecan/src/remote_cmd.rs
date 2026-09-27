//! `pecan remote`: reach a localhost-bound server from a phone through
//! `tailscale serve`, which terminates HTTPS on the tailnet name and proxies to
//! 127.0.0.1. Pairing still gates every request; Tailscale only adds transport.
//!
//! Read-only by default: it reports the tailnet URL and whether the port is
//! already served. `--enable` is the only path that changes Tailscale config,
//! and it refuses to replace a mapping that proxies somewhere else.

use std::time::Duration;

use serde_json::{Value, json};

use crate::cli::CliError;
use crate::session_cmd::{Output, block_on};

const DEFAULT_PORT: u16 = 7614;
const TAILSCALE_TIMEOUT: Duration = Duration::from_secs(15);

/// What `tailscale serve` currently does with `https://<host>:<port>/`.
#[derive(Debug, PartialEq, Eq)]
enum ServeState {
    /// Nothing is served on that HTTPS port.
    Off,
    /// The port already proxies to this Pecan server.
    Pecan,
    /// The port proxies to something else; enabling would clobber it.
    Other(String),
}

impl ServeState {
    fn label(&self) -> &str {
        match self {
            Self::Off => "off",
            Self::Pecan => "serving",
            Self::Other(_) => "conflict",
        }
    }
}

/// `pecan remote [--port <n>] [--enable] [--json]`.
///
/// # Errors
/// Fails when Tailscale is missing, stopped, or times out, or when `--enable`
/// would replace another service's mapping.
pub(crate) fn run(rest: &[String]) -> Result<(), CliError> {
    let mut port = DEFAULT_PORT;
    let (mut enable, mut json_out) = (false, false);
    let mut items = rest.iter();
    while let Some(item) = items.next() {
        match item.as_str() {
            "--enable" => enable = true,
            "--json" => json_out = true,
            "--port" => {
                port = items
                    .next()
                    .and_then(|value| value.parse().ok())
                    .ok_or_else(|| CliError::Usage("--port <1-65535>".to_owned()))?;
            }
            other => {
                return Err(CliError::Usage(format!("pecan remote: unknown argument {other}")));
            }
        }
    }
    block_on(remote(port, enable, json_out))
}

async fn remote(port: u16, enable: bool, json_out: bool) -> Result<(), CliError> {
    let host = tailnet_host(&tailscale_json(&["status", "--json"]).await?)?;
    let url = format!("https://{host}:{port}");
    let target = format!("http://127.0.0.1:{port}");
    let command = format!("tailscale serve --bg --https={port} {target}");
    let mut state =
        serve_state(&tailscale_json(&["serve", "status", "--json"]).await?, &host, port);
    if enable {
        match &state {
            ServeState::Off => {
                tailscale(&["serve", "--bg", &format!("--https={port}"), &target]).await?;
                state = ServeState::Pecan;
            }
            ServeState::Pecan => {}
            ServeState::Other(proxy) => {
                return Err(CliError::Refused(format!(
                    "{url} already proxies to {proxy}; pick another --port or run `tailscale serve --https={port} off` first"
                )));
            }
        }
    }
    let next = match &state {
        ServeState::Pecan => format!(
            "Pair a phone: pecan pair --url {url}\nStop: tailscale serve --https={port} off"
        ),
        ServeState::Off => {
            format!("Enable with: pecan remote --port {port} --enable\n(runs `{command}`)")
        }
        ServeState::Other(proxy) => {
            format!("Port {port} already proxies to {proxy}; choose another --port.")
        }
    };
    let proxy = match &state {
        ServeState::Other(proxy) => Some(proxy.as_str()),
        ServeState::Off | ServeState::Pecan => None,
    };
    let value = json!({ "url": url, "state": state.label(), "command": command, "proxy": proxy });
    Output::new(value, format!("{url}  [{}]\n{next}", state.label())).print(json_out);
    Ok(())
}

/// The machine's `MagicDNS` name, once Tailscale is up.
fn tailnet_host(status: &Value) -> Result<String, CliError> {
    let backend = status.get("BackendState").and_then(Value::as_str).unwrap_or("Unknown");
    if backend != "Running" {
        return Err(CliError::Refused(format!("tailscale is {backend}; run `tailscale up` first")));
    }
    status
        .pointer("/Self/DNSName")
        .and_then(Value::as_str)
        .map(|name| name.trim_end_matches('.'))
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            CliError::Refused(
                "tailscale has no MagicDNS name; enable MagicDNS and HTTPS".to_owned(),
            )
        })
}

fn serve_state(serve: &Value, host: &str, port: u16) -> ServeState {
    let Some(web) = serve.get("Web").and_then(Value::as_object) else {
        return ServeState::Off;
    };
    let Some(proxy) = web
        .get(&format!("{host}:{port}"))
        .and_then(|site| site.pointer("/Handlers/~1/Proxy"))
        .and_then(Value::as_str)
    else {
        return ServeState::Off;
    };
    let ours = [format!("http://127.0.0.1:{port}"), format!("http://localhost:{port}")];
    if ours.iter().any(|candidate| proxy.trim_end_matches('/') == candidate) {
        ServeState::Pecan
    } else {
        ServeState::Other(proxy.to_owned())
    }
}

/// `$PECAN_TAILSCALE` (tests) or `tailscale` on `PATH`.
fn tailscale_program() -> String {
    std::env::var("PECAN_TAILSCALE").unwrap_or_else(|_| "tailscale".to_owned())
}

async fn tailscale(args: &[&str]) -> Result<Vec<u8>, CliError> {
    let program = tailscale_program();
    let output = tokio::time::timeout(
        TAILSCALE_TIMEOUT,
        tokio::process::Command::new(&program).args(args).kill_on_drop(true).output(),
    )
    .await
    .map_err(|elapsed| CliError::Startup(format!("`{program} {}` {elapsed}", args.join(" "))))?
    .map_err(|error| CliError::Startup(format!("cannot run {program}: {error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::Startup(format!(
            "`{program} {}` failed: {}",
            args.join(" "),
            stderr.trim()
        )));
    }
    Ok(output.stdout)
}

async fn tailscale_json(args: &[&str]) -> Result<Value, CliError> {
    let stdout = tailscale(args).await?;
    if stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    serde_json::from_slice(&stdout).map_err(|error| {
        CliError::Startup(format!("tailscale {} returned bad JSON: {error}", args.join(" ")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: &str = "box.tail0.ts.net";

    fn served(key: &str, proxy: &str) -> Value {
        json!({ "Web": { key: { "Handlers": { "/": { "Proxy": proxy } } } } })
    }

    #[test]
    fn reads_the_magicdns_host_only_when_running() {
        let running =
            json!({ "BackendState": "Running", "Self": { "DNSName": "box.tail0.ts.net." } });
        assert_eq!(tailnet_host(&running).ok().as_deref(), Some(HOST));
        let stopped =
            json!({ "BackendState": "Stopped", "Self": { "DNSName": "box.tail0.ts.net." } });
        assert!(
            matches!(tailnet_host(&stopped), Err(CliError::Refused(message)) if message.contains("Stopped"))
        );
        let nameless = json!({ "BackendState": "Running", "Self": { "DNSName": "" } });
        assert!(matches!(tailnet_host(&nameless), Err(CliError::Refused(_))));
    }

    #[test]
    fn classifies_the_serve_mapping_for_the_port() {
        assert_eq!(serve_state(&json!({}), HOST, 7614), ServeState::Off);
        assert_eq!(
            serve_state(&served("box.tail0.ts.net:7614", "http://127.0.0.1:7614"), HOST, 7614),
            ServeState::Pecan
        );
        assert_eq!(
            serve_state(&served("box.tail0.ts.net:7614", "http://127.0.0.1:3000"), HOST, 7614),
            ServeState::Other("http://127.0.0.1:3000".to_owned())
        );
        assert_eq!(
            serve_state(&served("box.tail0.ts.net:443", "http://127.0.0.1:7614"), HOST, 7614),
            ServeState::Off,
            "another HTTPS port does not count"
        );
    }
}
