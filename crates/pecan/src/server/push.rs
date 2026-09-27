//! Web push for devices that are not looking. When a turn finishes or Pi asks
//! for input, every subscribed device without a live event stream gets a
//! payload-less push; its service worker then fetches `GET /api/push/pending`
//! with the device cookie and shows what is waiting. A device with an open
//! stream already hears the event and notifies in-page, so it is skipped.
//!
//! Payload-less pushes need no content encryption, only a VAPID (RFC 8292)
//! ES256 token. Endpoints must belong to a known push service (or the
//! explicit `PECAN_PUSH_EXTRA_ORIGIN`, for tests), so a paired device cannot
//! aim the server's requests anywhere else.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, State};
use axum::http::{Request, Uri, header};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http_body_util::Empty;
use hyper::body::Bytes;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use p256::ecdsa::signature::Signer as _;
use p256::ecdsa::{Signature, SigningKey};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::api::lock;
use super::api_errors::{ApiError, json_body};
use super::auth::{Principal, require_cli, write_owner_only};
use super::snapshot::App;

/// Bound on one request to a push service.
const PUSH_TIMEOUT: Duration = Duration::from_secs(10);
/// How long an undelivered notice waits for its device to fetch it.
const PENDING_TTL: Duration = Duration::from_secs(60 * 60);
/// Notices kept per device; older ones drop first.
const MAX_PENDING: usize = 8;
/// Longest endpoint URL accepted from a browser.
const MAX_ENDPOINT_LEN: usize = 2048;
/// VAPID token lifetime (the spec caps it at 24 hours).
const TOKEN_LIFETIME_SECS: i64 = 12 * 60 * 60;
/// How long the push service may hold a push for an offline device.
const PUSH_TTL_SECS: u32 = 60 * 60;
/// VAPID contact claim, unless `PECAN_PUSH_SUBJECT` overrides it.
const DEFAULT_SUBJECT: &str = "mailto:pecan@example.com";
/// Push services browsers subscribe with (host or parent domain).
const PUSH_HOSTS: &[&str] = &[
    "fcm.googleapis.com",
    "android.googleapis.com",
    "push.services.mozilla.com",
    "push.apple.com",
    "notify.windows.com",
];

/// Failures preparing web push at startup.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PushSetupError {
    /// The OS random source failed.
    #[error("random source: {0}")]
    Random(#[from] getrandom::Error),
    /// Reading or writing the VAPID key file failed.
    #[error("vapid key {path}: {source}")]
    KeyFile {
        /// The key file.
        path: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// The key file does not hold a P-256 private key.
    #[error("vapid key {path} is not a base64url P-256 private key; delete it to start over")]
    BadKey {
        /// The key file.
        path: String,
    },
    /// No TLS trust roots could be loaded.
    #[error("tls roots: {0}")]
    Tls(std::io::Error),
}

/// One notification waiting for a device, as its service worker shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Notice {
    title: String,
    body: String,
    /// Same tag as the in-page notification, so the OS keeps only one.
    tag: String,
    url: String,
}

impl Notice {
    /// A finished turn in session `id`.
    pub(crate) fn turn_done(id: &str, session_title: Option<&str>) -> Self {
        Self::for_session("Pi finished", id, session_title)
    }

    /// A dialog in session `id` blocking on the user.
    pub(crate) fn needs_input(id: &str, session_title: Option<&str>) -> Self {
        Self::for_session("Pi needs your input", id, session_title)
    }

    fn for_session(title: &str, id: &str, session_title: Option<&str>) -> Self {
        let body =
            session_title.map(str::trim).filter(|text| !text.is_empty()).unwrap_or("Session");
        let encoded: String = id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                    c.to_string()
                } else {
                    format!("%{:02X}", u32::from(c))
                }
            })
            .collect();
        Self {
            title: title.to_owned(),
            body: body.to_owned(),
            tag: format!("pecan:{id}"),
            url: format!("/#/s/{encoded}"),
        }
    }

    fn test() -> Self {
        Self {
            title: "Pecan".to_owned(),
            body: "Test notification: push works on this device.".to_owned(),
            tag: "pecan:test".to_owned(),
            url: "/#/settings".to_owned(),
        }
    }
}

/// What happened to one push.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "outcome")]
enum Delivery {
    /// The push service accepted it.
    Sent,
    /// The subscription is gone (404/410) and was removed.
    Removed,
    /// Anything else; the subscription is kept.
    Failed { error: String },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceDelivery {
    device_id: String,
    device_name: String,
    #[serde(flatten)]
    delivery: Delivery,
}

/// Outcome of one fan-out.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    sent: usize,
    removed: usize,
    failed: usize,
    results: Vec<DeviceDelivery>,
}

/// Undelivered notices per device, with when each was queued.
type PendingQueues = HashMap<String, VecDeque<(Instant, Notice)>>;

type HttpsClient = Client<HttpsConnector<HttpConnector>, Empty<Bytes>>;

/// Shared push state; cheap to clone.
#[derive(Clone)]
pub(crate) struct Push {
    inner: Arc<Inner>,
}

struct Inner {
    key: SigningKey,
    /// Uncompressed public key, base64url: the browser's
    /// `applicationServerKey` and the VAPID `k=` parameter.
    public_key: String,
    subject: String,
    extra_origin: Option<String>,
    client: HttpsClient,
    pending: Mutex<PendingQueues>,
    /// Open event streams per device.
    streams: Mutex<HashMap<String, usize>>,
}

impl std::fmt::Debug for Push {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Push").field("public_key", &self.inner.public_key).finish_non_exhaustive()
    }
}

impl Push {
    /// Loads (or creates) the VAPID key and builds the HTTPS client.
    ///
    /// # Errors
    /// Fails when the key file is unreadable or invalid, or no TLS roots load.
    pub(crate) fn load(paths: &pecan_core::PiPaths) -> Result<Self, PushSetupError> {
        let key = load_or_create_key(&paths.vapid_key_file())?;
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .map_err(PushSetupError::Tls)?
            .https_or_http()
            .enable_http1()
            .build();
        let client = Client::builder(hyper_util::rt::TokioExecutor::new()).build(connector);
        let env = |name| {
            std::env::var(name)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        Ok(Self::new(
            key,
            client,
            env("PECAN_PUSH_SUBJECT").unwrap_or_else(|| DEFAULT_SUBJECT.to_owned()),
            env("PECAN_PUSH_EXTRA_ORIGIN").map(|origin| origin.trim_end_matches('/').to_owned()),
        ))
    }

    fn new(
        key: SigningKey,
        client: HttpsClient,
        subject: String,
        extra_origin: Option<String>,
    ) -> Self {
        let public_key =
            URL_SAFE_NO_PAD.encode(key.verifying_key().to_sec1_point(false).as_bytes());
        Self {
            inner: Arc::new(Inner {
                key,
                public_key,
                subject,
                extra_origin,
                client,
                pending: Mutex::default(),
                streams: Mutex::default(),
            }),
        }
    }

    /// Marks an event stream open for `device` until the guard drops.
    pub(crate) fn stream_opened(&self, device: &str) -> LiveStream {
        if let Ok(mut streams) = self.inner.streams.lock() {
            *streams.entry(device.to_owned()).or_default() += 1;
        }
        LiveStream { push: self.clone(), device: device.to_owned() }
    }

    fn has_live_stream(&self, device: &str) -> bool {
        self.inner.streams.lock().is_ok_and(|streams| streams.contains_key(device))
    }

    /// Queues `notice` and pushes it, in the background, to every subscribed
    /// device that has no open stream. Each push is bounded by
    /// [`PUSH_TIMEOUT`]; failures are logged.
    pub(crate) fn notify(&self, app: &App, notice: Notice) {
        let (push, app) = (self.clone(), app.clone());
        tokio::spawn(async move {
            match push.deliver(&app, &notice, None, true).await {
                Ok(report) if report.failed > 0 => {
                    tracing::warn!(sent = report.sent, failed = report.failed, results = ?report.results, "some pushes failed");
                }
                Ok(report) => {
                    tracing::debug!(sent = report.sent, removed = report.removed, "pushed")
                }
                Err(error) => tracing::warn!(?error, "push fan-out failed"),
            }
        });
    }

    async fn deliver(
        &self,
        app: &App,
        notice: &Notice,
        only: Option<&str>,
        skip_live: bool,
    ) -> Result<Report, ApiError> {
        let targets: Vec<_> = lock(app)?
            .push_subscriptions()?
            .into_iter()
            .filter(|sub| only.is_none_or(|device| device == sub.device_id))
            .filter(|sub| !(skip_live && self.has_live_stream(&sub.device_id)))
            .collect();
        let now = Instant::now();
        for sub in &targets {
            self.enqueue(&sub.device_id, notice, now);
        }
        let deliveries =
            futures::future::join_all(targets.iter().map(|sub| self.send(&sub.endpoint))).await;
        let mut report = Report::default();
        for (sub, delivery) in targets.into_iter().zip(deliveries) {
            match &delivery {
                Delivery::Sent => report.sent += 1,
                Delivery::Removed => {
                    lock(app)?.remove_push_subscription(&sub.device_id)?;
                    report.removed += 1;
                }
                Delivery::Failed { .. } => report.failed += 1,
            }
            report.results.push(DeviceDelivery {
                device_id: sub.device_id,
                device_name: sub.device_name,
                delivery,
            });
        }
        Ok(report)
    }

    async fn send(&self, endpoint: &str) -> Delivery {
        let (uri, audience) = match self.check_endpoint(endpoint) {
            Ok(checked) => checked,
            Err(error) => return Delivery::Failed { error },
        };
        let now = jiff::Timestamp::now().as_second();
        let request = Request::post(uri)
            .header(header::AUTHORIZATION, self.vapid(&audience, now))
            .header("ttl", PUSH_TTL_SECS)
            .header("urgency", "high")
            .header(header::CONTENT_LENGTH, 0)
            .body(Empty::new());
        let request = match request {
            Ok(request) => request,
            Err(error) => return Delivery::Failed { error: error.to_string() },
        };
        match tokio::time::timeout(PUSH_TIMEOUT, self.inner.client.request(request)).await {
            Err(elapsed) => Delivery::Failed { error: format!("push service: {elapsed}") },
            Ok(Err(error)) => Delivery::Failed { error: format!("push service: {error}") },
            Ok(Ok(response)) => match response.status().as_u16() {
                200..=299 => Delivery::Sent,
                404 | 410 => Delivery::Removed,
                status => Delivery::Failed { error: format!("push service answered {status}") },
            },
        }
    }

    /// `Authorization: vapid t=<ES256 JWT>, k=<public key>` for `audience`.
    fn vapid(&self, audience: &str, now_secs: i64) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = json!({
            "aud": audience,
            "exp": now_secs.saturating_add(TOKEN_LIFETIME_SECS),
            "sub": self.inner.subject,
        });
        let unsigned = format!("{header}.{}", URL_SAFE_NO_PAD.encode(claims.to_string()));
        let signature: Signature = self.inner.key.sign(unsigned.as_bytes());
        format!(
            "vapid t={unsigned}.{}, k={}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes()),
            self.inner.public_key
        )
    }

    /// Parses an endpoint a browser handed us and returns it with its origin
    /// (the VAPID audience), or why it is refused.
    fn check_endpoint(&self, endpoint: &str) -> Result<(Uri, String), String> {
        if endpoint.len() > MAX_ENDPOINT_LEN {
            return Err("push endpoint is too long".to_owned());
        }
        let uri: Uri = endpoint.parse().map_err(|error| format!("push endpoint: {error}"))?;
        let (Some(scheme), Some(authority)) = (uri.scheme_str(), uri.authority()) else {
            return Err("push endpoint must be an absolute URL".to_owned());
        };
        let origin = format!("{scheme}://{authority}");
        if self.inner.extra_origin.as_deref() == Some(origin.as_str()) {
            return Ok((uri, origin));
        }
        let host = authority.host().to_ascii_lowercase();
        let known = PUSH_HOSTS.iter().any(|known| {
            host == *known || host.strip_suffix(known).is_some_and(|rest| rest.ends_with('.'))
        });
        let plain = scheme == "https"
            && !authority.as_str().contains('@')
            && authority.port_u16().is_none_or(|port| port == 443);
        if known && plain {
            Ok((uri, origin))
        } else {
            Err(format!("{origin} is not a known web push service"))
        }
    }

    fn pending(&self) -> Result<MutexGuard<'_, PendingQueues>, ApiError> {
        self.inner
            .pending
            .lock()
            .map_err(|_poisoned| ApiError::internal("push queue lock poisoned"))
    }

    fn enqueue(&self, device: &str, notice: &Notice, now: Instant) {
        let Ok(mut pending) = self.pending() else { return };
        let queue = pending.entry(device.to_owned()).or_default();
        queue.retain(|(queued, old)| {
            now.saturating_duration_since(*queued) < PENDING_TTL && old.tag != notice.tag
        });
        queue.push_back((now, notice.clone()));
        while queue.len() > MAX_PENDING {
            queue.pop_front();
        }
    }

    fn take_pending(&self, device: &str, now: Instant) -> Result<Vec<Notice>, ApiError> {
        Ok(self
            .pending()?
            .remove(device)
            .unwrap_or_default()
            .into_iter()
            .filter(|(queued, _)| now.saturating_duration_since(*queued) < PENDING_TTL)
            .map(|(_, notice)| notice)
            .collect())
    }
}

/// Keeps a device counted as streaming until dropped.
#[derive(Debug)]
pub(crate) struct LiveStream {
    push: Push,
    device: String,
}

impl Drop for LiveStream {
    fn drop(&mut self) {
        let Ok(mut streams) = self.push.inner.streams.lock() else { return };
        if let Some(count) = streams.get_mut(&self.device) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                streams.remove(&self.device);
            }
        }
    }
}

fn require_device(principal: &Principal) -> Result<&str, ApiError> {
    match principal {
        Principal::Device(id) => Ok(id),
        Principal::Cli => Err(ApiError::forbidden("only a paired browser has a push subscription")),
    }
}

/// `GET /api/push/key`: the server's `applicationServerKey`, and whether the
/// calling device is subscribed.
pub(crate) async fn key(
    State(app): State<App>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    let subscribed = match &principal {
        Principal::Device(id) => {
            lock(&app)?.push_subscriptions()?.iter().any(|sub| sub.device_id == *id)
        }
        Principal::Cli => false,
    };
    Ok(Json(json!({ "publicKey": app.push.inner.public_key, "subscribed": subscribed })))
}

/// A browser `PushSubscription` as JSON; only the endpoint is kept.
#[derive(Debug, Deserialize)]
pub(crate) struct SubscribeBody {
    endpoint: String,
}

/// `PUT /api/push/subscription` (devices): store this device's endpoint.
pub(crate) async fn subscribe(
    State(app): State<App>,
    Extension(principal): Extension<Principal>,
    body: Result<Json<SubscribeBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let device = require_device(&principal)?;
    let body = json_body(body)?;
    app.push.check_endpoint(&body.endpoint).map_err(ApiError::bad_request)?;
    lock(&app)?.set_push_subscription(device, &body.endpoint)?;
    tracing::info!(%device, "push subscribed");
    Ok(Json(json!({ "subscribed": true })))
}

/// `DELETE /api/push/subscription` (devices).
pub(crate) async fn unsubscribe(
    State(app): State<App>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    let device = require_device(&principal)?;
    let removed = lock(&app)?.remove_push_subscription(device)?;
    Ok(Json(json!({ "removed": removed })))
}

/// `GET /api/push/pending` (devices): drains what the last pushes were for.
pub(crate) async fn pending(
    State(app): State<App>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    let device = require_device(&principal)?;
    let notifications = app.push.take_pending(device, Instant::now())?;
    Ok(Json(json!({ "notifications": notifications })))
}

/// `GET /api/push/subscriptions` (CLI only).
pub(crate) async fn subscriptions(
    State(app): State<App>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    require_cli(&principal)?;
    let subscriptions = lock(&app)?.push_subscriptions()?;
    Ok(Json(json!({ "subscriptions": subscriptions })))
}

/// `POST /api/push/test`: pushes a test notice, even to devices with an open
/// stream. The CLI reaches every subscribed device; a device only itself.
pub(crate) async fn test(
    State(app): State<App>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    let only = match &principal {
        Principal::Device(id) => Some(id.as_str()),
        Principal::Cli => None,
    };
    let report = app.push.deliver(&app, &Notice::test(), only, false).await?;
    Ok(Json(json!(report)))
}

/// Reads the VAPID key, creating it (owner-only) on first run.
fn load_or_create_key(path: &Path) -> Result<SigningKey, PushSetupError> {
    let display = path.display().to_string();
    match std::fs::read_to_string(path) {
        Ok(existing) => {
            return URL_SAFE_NO_PAD
                .decode(existing.trim())
                .ok()
                .and_then(|bytes| SigningKey::from_slice(&bytes).ok())
                .ok_or(PushSetupError::BadKey { path: display });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(PushSetupError::KeyFile { path: display, source }),
    }
    // A random 32-byte string is a valid scalar except with negligible odds.
    let mut bytes = [0_u8; 32];
    let key = loop {
        getrandom::fill(&mut bytes)?;
        if let Ok(key) = SigningKey::from_slice(&bytes) {
            break key;
        }
    };
    write_owner_only(path, URL_SAFE_NO_PAD.encode(bytes).as_bytes())
        .map_err(|source| PushSetupError::KeyFile { path: display, source })?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use p256::ecdsa::VerifyingKey;
    use p256::ecdsa::signature::Verifier as _;

    use super::*;

    fn push(extra_origin: Option<&str>) -> Push {
        let key = SigningKey::from_slice(&[7_u8; 32]).expect("fixed key");
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .expect("roots")
            .https_or_http()
            .enable_http1()
            .build();
        let client = Client::builder(hyper_util::rt::TokioExecutor::new()).build(connector);
        Push::new(key, client, DEFAULT_SUBJECT.to_owned(), extra_origin.map(str::to_owned))
    }

    #[test]
    fn vapid_tokens_verify_against_the_advertised_key() {
        let push = push(None);
        let header = push.vapid("https://fcm.googleapis.com", 1_000);
        let (token, key) = header
            .strip_prefix("vapid t=")
            .and_then(|rest| rest.split_once(", k="))
            .expect("vapid t=..., k=...");
        assert_eq!(key, push.inner.public_key);
        let public = URL_SAFE_NO_PAD.decode(key).expect("b64url key");
        assert_eq!(public.len(), 65, "uncompressed P-256 point");
        let (unsigned, signature) = token.rsplit_once('.').expect("signed JWT");
        let signature =
            Signature::from_slice(&URL_SAFE_NO_PAD.decode(signature).expect("b64url sig"))
                .expect("r||s");
        let verifier = VerifyingKey::from_sec1_bytes(&public).expect("point");
        assert!(verifier.verify(unsigned.as_bytes(), &signature).is_ok(), "signature verifies");
        let claims = unsigned.split('.').nth(1).expect("claims");
        let claims: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims).expect("b64url claims"))
                .expect("json claims");
        assert_eq!(claims["aud"], "https://fcm.googleapis.com");
        assert_eq!(claims["exp"], 1_000 + TOKEN_LIFETIME_SECS);
        assert_eq!(claims["sub"], DEFAULT_SUBJECT);
    }

    #[test]
    fn only_known_push_services_are_accepted() {
        let push = push(Some("http://127.0.0.1:9"));
        for ok in [
            "https://fcm.googleapis.com/fcm/send/abc",
            "https://web.push.apple.com/QGx",
            "https://updates.push.services.mozilla.com/wpush/v2/x",
            "http://127.0.0.1:9/sub",
        ] {
            assert!(push.check_endpoint(ok).is_ok(), "{ok}");
        }
        assert_eq!(
            push.check_endpoint("https://web.push.apple.com/x").map(|(_, origin)| origin),
            Ok("https://web.push.apple.com".to_owned())
        );
        for refused in [
            "http://fcm.googleapis.com/x",
            "https://evilpush.apple.com/x",
            "https://push.apple.com.evil.test/x",
            "https://fcm.googleapis.com:8443/x",
            "https://user@fcm.googleapis.com/x",
            "http://127.0.0.1:10/sub",
            "/relative",
            "not a url",
        ] {
            assert!(push.check_endpoint(refused).is_err(), "{refused}");
        }
        assert!(
            push.check_endpoint(&format!(
                "https://fcm.googleapis.com/{}",
                "a".repeat(MAX_ENDPOINT_LEN)
            ))
            .is_err()
        );
    }

    #[test]
    fn pending_notices_collapse_by_tag_cap_and_drain() {
        let push = push(None);
        let now = Instant::now();
        push.enqueue("dev", &Notice::turn_done("s1", Some("Fix build")), now);
        push.enqueue("dev", &Notice::needs_input("s1", Some("Fix build")), now);
        for index in 0..MAX_PENDING + 3 {
            push.enqueue("dev", &Notice::turn_done(&format!("other-{index}"), None), now);
        }
        let drained = push.take_pending("dev", now).expect("drain");
        assert_eq!(drained.len(), MAX_PENDING, "capped");
        assert!(drained.iter().all(|notice| notice.tag != "pecan:s1"), "oldest dropped first");
        assert!(push.take_pending("dev", now).expect("drain").is_empty(), "drained once");

        push.enqueue("dev", &Notice::turn_done("s2", None), now);
        let later = now + PENDING_TTL + Duration::from_secs(1);
        assert!(push.take_pending("dev", later).expect("drain").is_empty(), "expired");
    }

    #[test]
    fn notices_match_the_in_page_notification() {
        let notice = Notice::needs_input("a b/c", Some("  "));
        assert_eq!(notice.title, "Pi needs your input");
        assert_eq!(notice.body, "Session");
        assert_eq!(notice.tag, "pecan:a b/c");
        assert_eq!(notice.url, "/#/s/a%20b%2Fc");
    }

    #[test]
    fn streams_are_counted_until_every_guard_drops() {
        let push = push(None);
        let first = push.stream_opened("dev");
        let second = push.stream_opened("dev");
        drop(first);
        assert!(push.has_live_stream("dev"));
        drop(second);
        assert!(!push.has_live_stream("dev"));
    }

    #[test]
    fn key_file_is_created_once_and_invalid_keys_are_refused() {
        let dir = std::env::temp_dir().join(format!("pecan-vapid-{}", std::process::id()));
        let path = dir.join("vapid-key");
        std::fs::remove_dir_all(&dir).unwrap_or_default();
        let first = load_or_create_key(&path).expect("create");
        let again = load_or_create_key(&path).expect("reload");
        assert_eq!(first.to_bytes(), again.to_bytes(), "stable across restarts");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "owner-only");
        }
        std::fs::write(&path, "not-a-key").expect("corrupt");
        assert!(matches!(load_or_create_key(&path), Err(PushSetupError::BadKey { .. })));
        std::fs::remove_dir_all(&dir).unwrap_or_default();
    }
}
