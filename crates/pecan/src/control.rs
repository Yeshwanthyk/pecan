//! HTTP client for a running `pecan serve`: the CLI's control plane.
//!
//! The server stays authoritative for Pi workers; the CLI speaks the same
//! JSON API and SSE stream the browser uses. Every request is bounded by a
//! timeout so scripted runs never hang.

use std::time::Duration;

use futures::StreamExt;
use http_body_util::{BodyExt, BodyStream, Full};
use hyper::body::Bytes;
use hyper::{Method, Request, StatusCode, Uri};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use serde_json::Value;

/// Default server location, matching `pecan serve`.
pub(crate) const DEFAULT_SERVER: &str = "http://127.0.0.1:7614";
/// Upper bound for one plain request/response exchange.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

/// Failures talking to a Pecan server.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ControlError {
    /// The server URL or request path did not form a valid URI.
    #[error("invalid server url {url}: {source}")]
    InvalidUrl {
        /// The rejected URL.
        url: String,
        /// Parser error.
        source: hyper::http::uri::InvalidUri,
    },
    /// The request could not be built.
    #[error("request: {0}")]
    Request(#[from] hyper::http::Error),
    /// Connecting or exchanging bytes failed.
    #[error("cannot reach pecan at {url} (is `pecan serve` running?): {source}")]
    Connect {
        /// Server URL.
        url: String,
        /// Transport error.
        source: hyper_util::client::legacy::Error,
    },
    /// Reading a response body failed.
    #[error("reading response: {0}")]
    Body(#[from] hyper::Error),
    /// The server answered with a non-success status.
    #[error("{status}: {message}")]
    Status {
        /// HTTP status.
        status: StatusCode,
        /// Server-provided error message.
        message: String,
    },
    /// The response body was not the expected JSON.
    #[error("invalid json from server: {0}")]
    Decode(#[from] serde_json::Error),
    /// A bounded wait elapsed.
    #[error("timed out after {0:?} waiting for {1}")]
    Timeout(Duration, &'static str),
    /// The event stream ended before the awaited condition.
    #[error("event stream closed before {0}")]
    StreamClosed(&'static str),
}

/// A thin JSON client for one Pecan server.
#[derive(Clone)]
pub(crate) struct Control {
    base: String,
    http: Client<HttpConnector, Full<Bytes>>,
    /// CLI bearer token; never printed.
    token: Option<String>,
}

impl std::fmt::Debug for Control {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Control")
            .field("base", &self.base)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

/// The CLI's credential: `$PECAN_TOKEN`, else the `cli-token` file that
/// `pecan serve` wrote under the same agent directory.
fn cli_token() -> Option<String> {
    if let Ok(token) = std::env::var("PECAN_TOKEN")
        && !token.trim().is_empty()
    {
        return Some(token.trim().to_owned());
    }
    let path = pecan_core::PiPaths::detect().ok()?.cli_token_file();
    let token = std::fs::read_to_string(path).ok()?;
    Some(token.trim().to_owned()).filter(|token| !token.is_empty())
}

impl Control {
    /// Builds a client for `base` (e.g. `http://127.0.0.1:7614`).
    pub(crate) fn new(base: &str) -> Self {
        let http = Client::builder(TokioExecutor::new()).build(HttpConnector::new());
        Self { base: base.trim_end_matches('/').to_owned(), http, token: cli_token() }
    }

    /// The server base URL.
    pub(crate) fn base(&self) -> &str {
        &self.base
    }

    fn authorize(&self, builder: hyper::http::request::Builder) -> hyper::http::request::Builder {
        match &self.token {
            Some(token) => builder.header("authorization", format!("Bearer {token}")),
            None => builder,
        }
    }

    fn uri(&self, path: &str) -> Result<Uri, ControlError> {
        let url = format!("{}{path}", self.base);
        url.parse().map_err(|source| ControlError::InvalidUrl { url, source })
    }

    /// `GET` a JSON resource.
    pub(crate) async fn get(&self, path: &str) -> Result<Value, ControlError> {
        self.send(Method::GET, path, None, None).await
    }

    /// `DELETE` a resource.
    pub(crate) async fn delete(&self, path: &str) -> Result<Value, ControlError> {
        self.send(Method::DELETE, path, None, None).await
    }

    /// `POST` a JSON body.
    pub(crate) async fn post(&self, path: &str, body: &Value) -> Result<Value, ControlError> {
        self.send(Method::POST, path, Some(body), None).await
    }

    /// `POST` a JSON body with an `Idempotency-Key`, so a retry after a lost
    /// response returns the first result instead of repeating the action.
    pub(crate) async fn post_keyed(
        &self,
        path: &str,
        body: &Value,
        key: Option<&str>,
    ) -> Result<Value, ControlError> {
        self.send(Method::POST, path, Some(body), key).await
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        idempotency_key: Option<&str>,
    ) -> Result<Value, ControlError> {
        let bytes = body.map(Value::to_string).unwrap_or_default();
        let mut builder = self
            .authorize(Request::builder())
            .method(method)
            .uri(self.uri(path)?)
            .header("content-type", "application/json");
        if let Some(key) = idempotency_key {
            builder = builder.header("idempotency-key", key);
        }
        let request = builder.body(Full::new(Bytes::from(bytes)))?;
        let exchange = async {
            let response = self
                .http
                .request(request)
                .await
                .map_err(|source| ControlError::Connect { url: self.base.clone(), source })?;
            let status = response.status();
            let raw = response.into_body().collect().await?.to_bytes();
            Ok::<_, ControlError>((status, raw))
        };
        let (status, raw) = tokio::time::timeout(REQUEST_TIMEOUT, exchange)
            .await
            .map_err(|_elapsed| ControlError::Timeout(REQUEST_TIMEOUT, "server response"))??;
        let value: Value = if raw.is_empty() { Value::Null } else { serde_json::from_slice(&raw)? };
        if !status.is_success() {
            let message = value
                .get("error")
                .and_then(Value::as_str)
                .map_or_else(|| value.to_string(), str::to_owned);
            return Err(ControlError::Status { status, message });
        }
        Ok(value)
    }

    /// Opens the server-sent event stream and returns parsed `data:` payloads.
    pub(crate) async fn events(&self) -> Result<EventStream, ControlError> {
        let request = self
            .authorize(Request::builder())
            .method(Method::GET)
            .uri(self.uri("/api/events")?)
            .header("accept", "text/event-stream")
            .body(Full::new(Bytes::new()))?;
        let response = tokio::time::timeout(REQUEST_TIMEOUT, self.http.request(request))
            .await
            .map_err(|_elapsed| ControlError::Timeout(REQUEST_TIMEOUT, "event stream"))?
            .map_err(|source| ControlError::Connect { url: self.base.clone(), source })?;
        if !response.status().is_success() {
            return Err(ControlError::Status {
                status: response.status(),
                message: "event stream rejected".to_owned(),
            });
        }
        Ok(EventStream { body: BodyStream::new(response.into_body()), buffer: Vec::new() })
    }
}

/// Incremental SSE decoder over a streaming response body.
pub(crate) struct EventStream {
    body: BodyStream<hyper::body::Incoming>,
    buffer: Vec<u8>,
}

impl std::fmt::Debug for EventStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventStream").field("buffered", &self.buffer.len()).finish()
    }
}

impl EventStream {
    /// Returns the next JSON event, or `None` when the stream ends.
    pub(crate) async fn next(&mut self) -> Result<Option<Value>, ControlError> {
        loop {
            if let Some(event) = take_event(&mut self.buffer) {
                if let Some(value) = event {
                    return Ok(Some(value));
                }
                continue;
            }
            match self.body.next().await {
                Some(frame) => {
                    if let Ok(data) = frame?.into_data() {
                        self.buffer.extend_from_slice(&data);
                    }
                }
                None => return Ok(None),
            }
        }
    }
}

/// Pops one complete SSE record from `buffer`.
///
/// Returns `None` when no full record is buffered yet, `Some(None)` for a
/// record without JSON data (comments, keep-alives), and `Some(Some(json))`.
fn take_event(buffer: &mut Vec<u8>) -> Option<Option<Value>> {
    let end = buffer.windows(2).position(|pair| pair == b"\n\n")?;
    let record: Vec<u8> = buffer.drain(..end + 2).collect();
    let text = String::from_utf8_lossy(&record);
    let data: String = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");
    if data.is_empty() {
        return Some(None);
    }
    Some(serde_json::from_str(&data).ok())
}

#[cfg(test)]
mod tests {
    use super::take_event;

    #[test]
    fn decodes_complete_records_and_skips_comments() {
        let mut buffer = b": keep-alive\n\nevent: agent\ndata: {\"a\":1}\n\ndata: {\"b\"".to_vec();
        assert_eq!(take_event(&mut buffer), Some(None), "comment record has no data");
        assert_eq!(
            take_event(&mut buffer),
            Some(Some(serde_json::json!({"a": 1}))),
            "json data record decodes"
        );
        assert_eq!(take_event(&mut buffer), None, "partial record waits for more bytes");
        buffer.extend_from_slice(b":2}\n\n");
        assert_eq!(
            take_event(&mut buffer),
            Some(Some(serde_json::json!({"b": 2}))),
            "record completes across chunks"
        );
    }

    #[test]
    fn malformed_data_is_skipped_not_fatal() {
        let mut buffer = b"data: not json\n\n".to_vec();
        assert_eq!(take_event(&mut buffer), Some(None), "bad json is dropped");
    }
}
