//! Device pairing and request authentication.
//!
//! Every `/api` route except `POST /api/pair` needs a principal:
//!
//! - the local CLI, presenting `Authorization: Bearer <token>` read from the
//!   owner-only `cli-token` file in Pecan's state directory, or
//! - a paired browser, presenting the `HttpOnly` device cookie it received by
//!   redeeming a one-time code minted with `pecan pair`.
//!
//! Loopback is deliberately not trusted: `tailscale serve` proxies remote
//! phones from 127.0.0.1. Codes live only in memory, expire, and are dropped
//! wholesale after repeated wrong guesses. Tokens are stored as SHA-256
//! digests, and revoking a device also closes its live event streams.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, Path as UrlPath, Request, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::api_errors::{ApiError, json_body};
use super::snapshot::App;

/// How long a pairing code can be redeemed.
const CODE_TTL: Duration = Duration::from_secs(10 * 60);
/// Live codes kept at once; minting more drops the oldest.
const MAX_LIVE_CODES: usize = 8;
/// Wrong guesses tolerated before every live code is dropped.
const MAX_FAILURES: u32 = 10;
/// Crockford base32: no I, L, O, U, so codes survive being read aloud.
const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Characters in a code, shown as two groups of four.
const CODE_LEN: usize = 8;
/// Device cookie lifetime.
const COOKIE_MAX_AGE_SECS: u64 = 365 * 24 * 60 * 60;
/// `last_seen` is rewritten at most this often per device.
const TOUCH_INTERVAL_MS: i64 = 5 * 60 * 1000;
/// Longest stored device label.
const MAX_NAME_CHARS: usize = 60;

/// Who is making a request, attached to it by [`require`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Principal {
    /// The local CLI (holder of the `cli-token` file).
    Cli,
    /// A paired browser, by device id.
    Device(String),
}

/// Failures preparing authentication at startup.
#[derive(Debug, thiserror::Error)]
pub(crate) enum AuthSetupError {
    /// The OS random source failed.
    #[error("random source: {0}")]
    Random(#[from] getrandom::Error),
    /// Reading or writing the CLI token file failed.
    #[error("cli token {path}: {source}")]
    TokenFile {
        /// The token file.
        path: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },
}

/// Shared pairing state; cheap to clone.
#[derive(Clone)]
pub(crate) struct Auth {
    inner: Arc<Inner>,
}

struct Inner {
    cli_token_sha256: String,
    cookie_name: String,
    codes: Mutex<Codes>,
    revoked: tokio::sync::broadcast::Sender<String>,
}

#[derive(Default)]
struct Codes {
    /// Normalized code to its mint time.
    live: HashMap<String, Instant>,
    failures: u32,
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Auth").field("cookie", &self.inner.cookie_name).finish_non_exhaustive()
    }
}

impl Auth {
    /// Loads (or creates) the CLI token and derives this server's cookie name
    /// from its state database, so two servers on one host never share a
    /// cookie.
    ///
    /// # Errors
    /// Fails when the token file cannot be read or written.
    pub(crate) fn load(paths: &pecan_core::PiPaths) -> Result<Self, AuthSetupError> {
        let token = load_or_create_cli_token(&paths.cli_token_file())?;
        let db = paths.state_db();
        let instance = sha256_hex(db.to_string_lossy().as_bytes());
        Ok(Self::new(&token, instance.get(..8).unwrap_or("pecan")))
    }

    fn new(cli_token: &str, instance: &str) -> Self {
        let (revoked, _) = tokio::sync::broadcast::channel(16);
        Self {
            inner: Arc::new(Inner {
                cli_token_sha256: sha256_hex(cli_token.as_bytes()),
                cookie_name: format!("pecan_{instance}"),
                codes: Mutex::new(Codes::default()),
                revoked,
            }),
        }
    }

    /// Mints a single-use pairing code.
    pub(crate) fn mint_code(&self, now: Instant) -> Result<String, ApiError> {
        let code = random_code().map_err(ApiError::internal)?;
        let mut codes = self.codes()?;
        codes.live.retain(|_, minted| now.saturating_duration_since(*minted) < CODE_TTL);
        while codes.live.len() >= MAX_LIVE_CODES {
            let Some(oldest) =
                codes.live.iter().min_by_key(|(_, minted)| **minted).map(|(code, _)| code.clone())
            else {
                break;
            };
            codes.live.remove(&oldest);
        }
        codes.failures = 0;
        codes.live.insert(code.clone(), now);
        Ok(code)
    }

    /// Consumes `code` if it is live. Wrong guesses count toward a lockout
    /// that drops every live code.
    fn redeem_code(&self, code: &str, now: Instant) -> Result<bool, ApiError> {
        let normalized = normalize_code(code);
        let mut codes = self.codes()?;
        let fresh = codes
            .live
            .remove(&normalized)
            .is_some_and(|minted| now.saturating_duration_since(minted) < CODE_TTL);
        if !fresh {
            codes.failures = codes.failures.saturating_add(1);
            if codes.failures >= MAX_FAILURES {
                tracing::warn!(
                    failures = codes.failures,
                    "too many wrong pairing codes; dropping all"
                );
                codes.live.clear();
            }
        }
        Ok(fresh)
    }

    fn codes(&self) -> Result<std::sync::MutexGuard<'_, Codes>, ApiError> {
        self.inner
            .codes
            .lock()
            .map_err(|_poisoned| ApiError::internal("pairing codes lock poisoned"))
    }

    fn is_cli_token(&self, token: &str) -> bool {
        constant_time_eq(
            sha256_hex(token.as_bytes()).as_bytes(),
            self.inner.cli_token_sha256.as_bytes(),
        )
    }

    fn device_cookie<'a>(&self, headers: &'a HeaderMap) -> Option<&'a str> {
        headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(';'))
            .filter_map(|pair| pair.trim().split_once('='))
            .find(|(name, _)| *name == self.inner.cookie_name)
            .map(|(_, value)| value)
    }

    fn set_cookie(&self, token: &str, secure: bool) -> Result<HeaderValue, ApiError> {
        let secure = if secure { "; Secure" } else { "" };
        HeaderValue::from_str(&format!(
            "{}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={COOKIE_MAX_AGE_SECS}{secure}",
            self.inner.cookie_name
        ))
        .map_err(ApiError::internal)
    }

    /// A receiver of revoked device ids, for closing live streams.
    pub(crate) fn revocations(&self) -> tokio::sync::broadcast::Receiver<String> {
        self.inner.revoked.subscribe()
    }
}

/// Middleware: attaches a [`Principal`] or answers 401.
pub(crate) async fn require(State(app): State<App>, mut request: Request, next: Next) -> Response {
    match authenticate(&app, request.headers()) {
        Ok(Some(principal)) => {
            request.extensions_mut().insert(principal);
            next.run(request).await
        }
        Ok(None) => ApiError::unpaired().into_response(),
        Err(error) => error.into_response(),
    }
}

fn authenticate(app: &App, headers: &HeaderMap) -> Result<Option<Principal>, ApiError> {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if let Some(token) = bearer {
        return Ok(app.auth.is_cli_token(token.trim()).then_some(Principal::Cli));
    }
    let Some(token) = app.auth.device_cookie(headers) else {
        return Ok(None);
    };
    let store = super::api::lock(app)?;
    let Some(device) = store.device_by_token(&sha256_hex(token.as_bytes()))? else {
        return Ok(None);
    };
    let now = jiff::Timestamp::now().as_millisecond();
    if now.saturating_sub(device.last_seen_at_ms) > TOUCH_INTERVAL_MS {
        store.touch_device(&device.id)?;
    }
    Ok(Some(Principal::Device(device.id)))
}

pub(crate) fn require_cli(principal: &Principal) -> Result<(), ApiError> {
    match principal {
        Principal::Cli => Ok(()),
        Principal::Device(_) => {
            Err(ApiError::forbidden("only the local pecan CLI can manage pairing"))
        }
    }
}

#[derive(Deserialize, Debug)]
pub(crate) struct PairBody {
    code: String,
    name: Option<String>,
}

/// `POST /api/pair`: redeem a code for a device cookie. The only
/// unauthenticated API route.
pub(crate) async fn pair(
    State(app): State<App>,
    headers: HeaderMap,
    body: Result<Json<PairBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let body = json_body(body)?;
    if !app.auth.redeem_code(&body.code, Instant::now())? {
        return Err(ApiError::unpaired_with(
            "pairing code is wrong or expired; run `pecan pair` again",
        ));
    }
    let token = random_hex(32).map_err(ApiError::internal)?;
    let id = random_hex(6).map_err(ApiError::internal)?;
    let name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map_or_else(|| device_label(headers.get(header::USER_AGENT)), str::to_owned);
    let name: String = name.chars().filter(|c| !c.is_control()).take(MAX_NAME_CHARS).collect();
    let device = super::api::lock(&app)?.add_device(&id, &name, &sha256_hex(token.as_bytes()))?;
    tracing::info!(device = %device.id, name = %device.name, "paired device");
    let secure = headers
        .get("x-forwarded-proto")
        .is_some_and(|proto| proto.as_bytes().eq_ignore_ascii_case(b"https"));
    let cookie = app.auth.set_cookie(&token, secure)?;
    Ok(([(header::SET_COOKIE, cookie)], Json(serde_json::json!({ "device": device })))
        .into_response())
}

/// `POST /api/pair/code` (CLI only): mint a one-time pairing code.
pub(crate) async fn mint_code(
    State(app): State<App>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_cli(&principal)?;
    let code = app.auth.mint_code(Instant::now())?;
    let display =
        format!("{}-{}", code.get(..4).unwrap_or_default(), code.get(4..).unwrap_or_default());
    Ok(Json(serde_json::json!({
        "code": display,
        "expiresInSecs": CODE_TTL.as_secs(),
    })))
}

/// `GET /api/devices` (CLI only).
pub(crate) async fn list_devices(
    State(app): State<App>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_cli(&principal)?;
    let devices = super::api::lock(&app)?.devices()?;
    Ok(Json(serde_json::json!({ "devices": devices })))
}

/// `DELETE /api/devices/{id}` (CLI only): revoke and disconnect a device.
pub(crate) async fn revoke_device(
    State(app): State<App>,
    Extension(principal): Extension<Principal>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_cli(&principal)?;
    if !super::api::lock(&app)?.revoke_device(&id)? {
        return Err(ApiError::not_found("no paired device with that id"));
    }
    // No receivers just means no open streams for anyone.
    let _receivers = app.auth.inner.revoked.send(id.clone());
    tracing::info!(device = %id, "revoked device");
    Ok(Json(serde_json::json!({ "revoked": id })))
}

/// Uppercases, drops separators, and folds look-alikes (O→0, I/L→1).
fn normalize_code(raw: &str) -> String {
    raw.chars()
        .filter(|c| !matches!(c, '-' | ' ' | '_'))
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        })
        .collect()
}

fn random_code() -> Result<String, getrandom::Error> {
    let mut bytes = [0_u8; CODE_LEN];
    getrandom::fill(&mut bytes)?;
    // 256 is a multiple of 32, so `% 32` is unbiased.
    Ok(bytes
        .iter()
        .filter_map(|byte| CODE_ALPHABET.get(usize::from(byte % 32)).copied().map(char::from))
        .collect())
}

fn random_hex(len: usize) -> Result<String, getrandom::Error> {
    let mut bytes = vec![0_u8; len];
    getrandom::fill(&mut bytes)?;
    Ok(hex(&bytes))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|byte| [byte >> 4, byte & 0x0f])
        .filter_map(|nibble| DIGITS.get(usize::from(nibble)).copied().map(char::from))
        .collect()
}

pub(crate) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter().zip(right).fold(0_u8, |difference, (a, b)| difference | (a ^ b)) == 0
}

/// "Safari on iPhone"-style label from a User-Agent, for the device list.
fn device_label(user_agent: Option<&HeaderValue>) -> String {
    let ua = user_agent.and_then(|value| value.to_str().ok()).unwrap_or_default();
    let platform = [
        ("iPhone", "iPhone"),
        ("iPad", "iPad"),
        ("Android", "Android"),
        ("Macintosh", "Mac"),
        ("Windows", "Windows"),
        ("Linux", "Linux"),
    ]
    .into_iter()
    .find(|(needle, _)| ua.contains(needle))
    .map(|(_, label)| label);
    // Order matters: Edge and Chrome UAs also say "Safari".
    let browser = [
        ("Edg/", "Edge"),
        ("Firefox/", "Firefox"),
        ("FxiOS/", "Firefox"),
        ("CriOS/", "Chrome"),
        ("Chrome/", "Chrome"),
        ("Safari/", "Safari"),
    ]
    .into_iter()
    .find(|(needle, _)| ua.contains(needle))
    .map(|(_, label)| label);
    match (browser, platform) {
        (Some(browser), Some(platform)) => format!("{browser} on {platform}"),
        (Some(label), None) | (None, Some(label)) => label.to_owned(),
        (None, None) => "Browser".to_owned(),
    }
}

/// Reads the CLI token, creating it (owner-only) on first run.
fn load_or_create_cli_token(path: &Path) -> Result<String, AuthSetupError> {
    let file_error =
        |source| AuthSetupError::TokenFile { path: path.display().to_string(), source };
    match std::fs::read_to_string(path) {
        Ok(existing) if existing.trim().len() >= 32 => return Ok(existing.trim().to_owned()),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(file_error(error)),
    }
    let token = random_hex(32)?;
    write_owner_only(path, token.as_bytes()).map_err(file_error)?;
    Ok(token)
}

/// Writes a secret file readable only by its owner, creating parent dirs.
pub(crate) fn write_owner_only(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    std::io::Write::write_all(&mut options.open(path)?, contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth() -> Auth {
        Auth::new("cli-secret", "abcd1234")
    }

    #[test]
    fn codes_are_single_use_and_forgiving_to_type() {
        let auth = auth();
        let now = Instant::now();
        let code = auth.mint_code(now).expect("mint");
        assert_eq!(code.len(), CODE_LEN, "8 characters");
        let typed =
            format!("{}-{}", code.get(..4).unwrap_or_default(), code.get(4..).unwrap_or_default())
                .to_ascii_lowercase();
        assert!(auth.redeem_code(&typed, now).expect("redeem"), "lowercase with dash works");
        assert!(!auth.redeem_code(&code, now).expect("redeem"), "second use fails");
    }

    #[test]
    fn expired_codes_fail() {
        let auth = auth();
        let minted = Instant::now();
        let code = auth.mint_code(minted).expect("mint");
        let later = minted + CODE_TTL + Duration::from_secs(1);
        assert!(!auth.redeem_code(&code, later).expect("redeem"), "expired");
    }

    #[test]
    fn repeated_wrong_guesses_drop_live_codes() {
        let auth = auth();
        let now = Instant::now();
        let code = auth.mint_code(now).expect("mint");
        for _ in 0..MAX_FAILURES {
            assert!(!auth.redeem_code("ZZZZZZZZ", now).expect("guess"), "wrong guess");
        }
        assert!(!auth.redeem_code(&code, now).expect("redeem"), "locked out after guesses");
        let fresh = auth.mint_code(now).expect("mint again");
        assert!(auth.redeem_code(&fresh, now).expect("redeem"), "a new code works");
    }

    #[test]
    fn normalizes_look_alike_characters() {
        assert_eq!(normalize_code("ab-cd o1l i"), "ABCD0111");
    }

    #[test]
    fn cli_token_and_cookie_are_matched_exactly() {
        let auth = auth();
        assert!(auth.is_cli_token("cli-secret"), "right token");
        assert!(!auth.is_cli_token("cli-secre"), "wrong token");
        let mut headers = HeaderMap::new();
        headers
            .insert(header::COOKIE, HeaderValue::from_static("other=1; pecan_abcd1234=tok; x=2"));
        assert_eq!(auth.device_cookie(&headers), Some("tok"), "finds this server's cookie");
        headers.insert(header::COOKIE, HeaderValue::from_static("pecan_ffff0000=tok"));
        assert_eq!(auth.device_cookie(&headers), None, "ignores another server's cookie");
    }

    #[test]
    fn labels_devices_from_user_agents() {
        let ua = |raw: &'static str| device_label(Some(&HeaderValue::from_static(raw)));
        assert_eq!(
            ua(
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) Version/18.0 Mobile/15E148 Safari/604.1"
            ),
            "Safari on iPhone"
        );
        assert_eq!(
            ua("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Chrome/140.0 Safari/537.36"),
            "Chrome on Mac"
        );
        assert_eq!(device_label(None), "Browser");
    }

    #[test]
    fn cli_token_file_is_created_once_and_reused() {
        let dir = std::env::temp_dir().join(format!("pecan-auth-{}", std::process::id()));
        let path = dir.join("cli-token");
        std::fs::remove_dir_all(&dir).unwrap_or_default();
        let first = load_or_create_cli_token(&path).expect("create");
        assert_eq!(first.len(), 64, "32 random bytes in hex");
        assert_eq!(
            load_or_create_cli_token(&path).expect("reload"),
            first,
            "stable across restarts"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "owner-only");
        }
        std::fs::remove_dir_all(&dir).unwrap_or_default();
    }
}
