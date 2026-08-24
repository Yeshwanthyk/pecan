//! Session identity: the JSONL header record and lightweight summaries.

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::Serialize;

use crate::error::CoreError;

/// Upper bound on bytes read from a session file when hunting for a preview.
const PREVIEW_SCAN_BYTES: u64 = 128 * 1024;
/// Maximum characters kept in a session preview.
const PREVIEW_MAX_CHARS: usize = 160;
/// Path prefix pi-subagents uses for child agent sessions.
const SUBAGENT_CWD_PREFIX: &str = "/tmp/pi-subagent";

/// The first record of a pi session file (`{"type":"session", ...}`).
#[derive(Debug, Clone)]
pub struct SessionHeader {
    /// Stable session id.
    pub id: String,
    /// When the session was opened.
    pub opened_at: Timestamp,
    /// Working directory the session ran in; groups sessions into projects.
    pub cwd: String,
    /// Model provider, when recorded.
    pub provider: Option<String>,
    /// Model id, when recorded.
    pub model_id: Option<String>,
}

/// Whether a session was started by a user or spawned as a subagent child.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    /// An interactive session opened by a user.
    Normal,
    /// A child session spawned by pi-subagents.
    Subagent,
}

/// Lightweight per-session data used by list views.
///
/// Transcript metadata is derived from bounded reads; user-owned overrides
/// are overlaid from Pecan's state store by the server snapshot. Serialized
/// camelCase for the web client.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    /// Stable session id from the header.
    pub id: String,
    /// Absolute path of the session `.jsonl` file.
    pub path: PathBuf,
    /// Project working directory from the header.
    pub cwd: String,
    /// When the session was opened (sort key for "by open time").
    pub opened_at: Timestamp,
    /// File modification time; approximates last activity cheaply.
    pub last_activity: Timestamp,
    /// Size of the session file in bytes.
    pub bytes: u64,
    /// Provider recorded in the header, when present.
    pub provider: Option<String>,
    /// Model id recorded in the header, when present.
    pub model: Option<String>,
    /// First user prompt, truncated; the human-facing thread title.
    pub preview: Option<String>,
    /// Generated user-facing title persisted by Pecan, when present.
    pub title: Option<String>,
    /// User-opened vs subagent-spawned.
    pub kind: SessionKind,
    /// Parent session id, when this session is a spawned subagent.
    pub parent_id: Option<String>,
    /// Subagent display name from its `session_info` record (e.g. "scout").
    pub agent_name: Option<String>,
}

impl SessionHeader {
    /// Parses one JSONL line as a session header.
    ///
    /// Returns `None` for any line that is not a well-formed session record;
    /// scan callers simply skip such files rather than failing the whole index.
    #[must_use]
    pub fn parse_line(line: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        if value.get("type")?.as_str()? != "session" {
            return None;
        }
        let id = value.get("id")?.as_str()?.to_owned();
        let ts_raw = value.get("timestamp")?.as_str()?;
        let opened_at = ts_raw.parse::<Timestamp>().ok()?;
        let cwd = value.get("cwd").and_then(serde_json::Value::as_str)?.to_owned();
        let provider = str_field(&value, "provider");
        let model_id = str_field(&value, "modelId");
        Some(Self { id, opened_at, cwd, provider, model_id })
    }
}

fn str_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key).and_then(serde_json::Value::as_str).map(str::to_owned)
}

/// Reads and parses the header line of a session file.
///
/// # Errors
/// Returns [`CoreError::Io`] when the file cannot be opened or read.
pub fn read_header(path: &Path) -> crate::error::Result<Option<SessionHeader>> {
    let file = std::fs::File::open(path)
        .map_err(|source| CoreError::Io { path: path.to_owned(), source })?;
    let mut reader = std::io::BufReader::new(file);
    let mut line = String::new();
    let read = std::io::BufRead::read_line(&mut reader, &mut line)
        .map_err(|source| CoreError::Io { path: path.to_owned(), source })?;
    if read == 0 {
        return Ok(None);
    }
    Ok(SessionHeader::parse_line(line.trim_end_matches(['\n', '\r'])))
}

/// Extracts the first user text from the head of a session file as a preview.
///
/// Reads at most [`PREVIEW_SCAN_BYTES`] so huge transcripts stay cheap to index.
/// Malformed content yields `None` rather than an error: previews are cosmetic.
pub fn extract_preview(path: &Path) -> Option<String> {
    let bytes = bounded_head(path, PREVIEW_SCAN_BYTES)?;
    let text = String::from_utf8_lossy(&bytes);
    for line in text.lines().skip(1) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(serde_json::Value::as_str) != Some("message") {
            continue;
        }
        let message = value.get("message")?;
        if message.get("role").and_then(serde_json::Value::as_str) != Some("user") {
            continue;
        }
        if let Some(preview) = user_text_of(message) {
            if !preview.is_empty() {
                return Some(truncate_chars(&preview, PREVIEW_MAX_CHARS));
            }
        }
    }
    None
}

/// Bounded scan for a `session_info` record near the head of the file, which
/// marks spawned subagents with their parent id and display name.
pub fn subagent_info(path: &Path) -> Option<(String, Option<String>)> {
    let bytes = bounded_head(path, PREVIEW_SCAN_BYTES)?;
    let text = String::from_utf8_lossy(&bytes);
    for line in text.lines().skip(1) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(serde_json::Value::as_str) != Some("session_info") {
            continue;
        }
        let parent_id = str_field(&value, "parentId")?;
        if parent_id.is_empty() {
            return None;
        }
        return Some((parent_id, str_field(&value, "name")));
    }
    None
}

/// Extracts displayable user text from a message object without allocating
/// for non-text blocks.
fn user_text_of(message: &serde_json::Value) -> Option<String> {
    match message.get("content") {
        Some(serde_json::Value::String(text)) => Some(text.clone()),
        Some(serde_json::Value::Array(blocks)) => {
            let mut out = String::new();
            for block in blocks {
                if block.get("type").and_then(serde_json::Value::as_str) == Some("text") {
                    if let Some(part) = block.get("text").and_then(serde_json::Value::as_str) {
                        if !out.is_empty() {
                            out.push('\n');
                        }
                        out.push_str(part);
                    }
                }
            }
            Some(out)
        }
        _ => None,
    }
}

/// Reads up to `max` bytes from the start of a file.
fn bounded_head(path: &Path, max: u64) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let take = len.min(max);
    let mut buf = vec![0_u8; usize::try_from(take).ok()?];
    let mut reader = std::io::BufReader::new(file);
    let mut filled = 0_usize;
    while filled < buf.len() {
        match std::io::Read::read(&mut reader, &mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(_) => return None,
        }
    }
    buf.truncate(filled);
    Some(buf)
}

/// Truncates to at most `max` chars on a char boundary, appending an ellipsis.
#[must_use]
pub fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}\u{2026}")
}

/// Classifies a session by its working directory and folder location.
#[must_use]
pub fn kind_for(cwd: &str, path: &Path) -> SessionKind {
    let in_subagent_dir =
        path.components().any(|c| c.as_os_str().to_string_lossy().starts_with("--tmp-pi-subagent"));
    if cwd.starts_with(SUBAGENT_CWD_PREFIX) || in_subagent_dir {
        return SessionKind::Subagent;
    }
    SessionKind::Normal
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = r#"{"type":"session","id":"abc","timestamp":"2025-11-21T21:13:09.513Z","cwd":"/tmp/proj","provider":"anthropic","modelId":"m1"}"#;

    #[test]
    fn parses_header_line() {
        let header = SessionHeader::parse_line(HEADER).expect("header must parse");
        assert_eq!(header.id, "abc");
        assert_eq!(header.cwd, "/tmp/proj");
        assert_eq!(header.provider.as_deref(), Some("anthropic"));
        assert_eq!(header.model_id.as_deref(), Some("m1"));
    }

    #[test]
    fn rejects_non_session_lines() {
        let msg = r#"{"type":"message","timestamp":"2025-11-21T21:13:09.513Z"}"#;
        assert!(SessionHeader::parse_line(msg).is_none());
        assert!(SessionHeader::parse_line("not json").is_none());
    }

    #[test]
    fn truncates_on_char_boundaries() {
        let s = "h\u{00e9}e".repeat(200);
        let t = truncate_chars(&s, 10);
        assert_eq!(t.chars().count(), 10);
        assert!(t.ends_with('\u{2026}'));
    }

    #[test]
    fn classifies_subagent_cwd() {
        let p = Path::new("/x/--tmp-pi-subagent-codex-scout--/a.jsonl");
        assert_eq!(kind_for("/tmp/pi-subagent-codex-scout/src", p), SessionKind::Subagent);
        assert_eq!(
            kind_for("/Users/x/proj", Path::new("/x/--Users-proj--/a.jsonl")),
            SessionKind::Normal
        );
    }

    #[test]
    fn extracts_preview_from_fixture() {
        let dir = std::env::temp_dir().join(format!("pecan-preview-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let file = dir.join("s.jsonl");
        std::fs::write(
            &file,
            format!("{HEADER}\n{{\"type\":\"message\",\"timestamp\":1,\"message\":{{\"role\":\"user\",\"content\":\"fix the flaky test\"}}}}\n"),
        )
        .expect("write");
        assert_eq!(extract_preview(&file).as_deref(), Some("fix the flaky test"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
