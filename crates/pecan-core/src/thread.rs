//! Thread parsing: turns a session `.jsonl` transcript into renderable entries.
//!
//! Parsing is defensive and bounded: unknown record types are skipped,
//! oversized text blocks are truncated, and only the most recent
//! [`MAX_ENTRIES`] entries are returned so opening a huge thread stays fast.

use std::path::Path;

use jiff::Timestamp;
use serde::Serialize;

use crate::error::{CoreError, Result};
use crate::session::truncate_chars;

/// Maximum per-block characters kept in rendered entries.
const BLOCK_TEXT_CAP: usize = 8_000;
/// Maximum arguments-preview characters kept per tool call.
const ARGS_PREVIEW_CAP: usize = 240;
/// Maximum entries returned, counted from the tail of the transcript.
pub const MAX_ENTRIES: usize = 600;

/// A tool call emitted inside an assistant message.
#[derive(Debug, Clone, Serialize)]
pub struct ToolCall {
    /// Provider tool-call id used to correlate results.
    #[serde(rename = "toolCallId")]
    pub tool_call_id: String,
    /// Tool name (`bash`, `read`, `subagent_spawn`, ...).
    pub name: String,
    /// Compact single-line preview of the arguments JSON.
    #[serde(rename = "argsPreview")]
    pub args_preview: String,
}

/// One renderable row of a conversation thread.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ThreadEntry {
    /// A user prompt.
    User {
        /// Prompt text (truncated when very long).
        text: String,
        /// Whether `text` hit the display cap.
        truncated: bool,
    },
    /// An assistant response.
    Assistant {
        /// Visible reply text (truncated when very long).
        text: String,
        /// Whether `text` hit the display cap.
        truncated: bool,
        /// Hidden reasoning, shown collapsed.
        thinking: Option<String>,
        /// Tool calls issued during this turn.
        #[serde(default)]
        tools: Vec<ToolCall>,
        /// Model that produced this message, when recorded.
        model: Option<String>,
    },
    /// An `ask_user` interaction.
    AskUser {
        /// The raw questions payload from the tool call arguments.
        questions: serde_json::Value,
        /// The recorded free-form/selection answers, when answered.
        answer: Option<serde_json::Value>,
    },
    /// A failed tool result; rendered as a quiet inline note.
    ToolError {
        /// Error text from the tool result.
        text: String,
        /// Whether `text` hit the display cap.
        truncated: bool,
    },
}

/// Parsed view of one session transcript.
#[derive(Debug, Clone, Serialize)]
pub struct ThreadView {
    /// Entries in chronological order (oldest first), tail-bounded.
    pub entries: Vec<Dated<ThreadEntry>>,
    /// Number of older entries omitted by [`MAX_ENTRIES`].
    pub omitted: usize,
    /// Whether the transcript ends with an unanswered `ask_user` call.
    pub waiting_askuser: bool,
}

/// Wrapper attaching a timestamp to any entry.
#[derive(Debug, Clone, Serialize)]
pub struct Dated<T> {
    /// When the entry was recorded.
    pub ts: Option<Timestamp>,
    /// The wrapped payload.
    pub entry: T,
}

/// Parses a session file into a [`ThreadView`].
///
/// # Errors
/// Returns [`CoreError::Io`] when the file cannot be opened or read.
pub fn parse_thread(path: &Path) -> Result<ThreadView> {
    let bytes =
        std::fs::read(path).map_err(|source| CoreError::Io { path: path.to_owned(), source })?;
    let mut all: Vec<Dated<ThreadEntry>> = Vec::new();
    // toolCallId -> index into `all` of the unanswered AskUser entry.
    let mut open_askuser: Vec<(String, usize)> = Vec::new();

    for line in bytes.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(serde_json::Value::as_str) != Some("message") {
            continue;
        }
        let ts = parse_ts(&value);
        let Some(message) = value.get("message") else { continue };
        match message.get("role").and_then(serde_json::Value::as_str) {
            Some("user") => {
                if let Some((text, truncated)) = text_blocks(message.get("content"), BLOCK_TEXT_CAP)
                {
                    all.push(Dated { ts, entry: ThreadEntry::User { text, truncated } });
                }
            }
            Some("assistant") => {
                let mut text = String::new();
                let mut text_truncated = false;
                let mut thinking: Option<String> = None;
                let mut tools: Vec<ToolCall> = Vec::new();
                if let Some(blocks) = message.get("content").and_then(as_array) {
                    for block in blocks {
                        match block.get("type").and_then(serde_json::Value::as_str) {
                            Some("text") => {
                                let Some(part) = str_field(block, "text") else { continue };
                                push_block(&mut text, &mut text_truncated, &part, BLOCK_TEXT_CAP);
                            }
                            Some("thinking") => {
                                if thinking.is_none() {
                                    let part = str_field(block, "thinking").unwrap_or_default();
                                    thinking = Some(truncate_chars(&part, BLOCK_TEXT_CAP));
                                }
                            }
                            Some("toolCall") => {
                                let name =
                                    str_field(block, "name").unwrap_or_else(|| "tool".into());
                                let id = str_field(block, "id").unwrap_or_default();
                                let args_preview =
                                    block.get("arguments").map(compact_json).unwrap_or_default();
                                if name == "ask_user" {
                                    let questions = block
                                        .get("arguments")
                                        .cloned()
                                        .unwrap_or(serde_json::Value::Null);
                                    open_askuser.push((id.clone(), all.len()));
                                    all.push(Dated {
                                        ts,
                                        entry: ThreadEntry::AskUser { questions, answer: None },
                                    });
                                } else {
                                    tools.push(ToolCall { tool_call_id: id, name, args_preview });
                                }
                            }
                            _ => {}
                        }
                    }
                }
                if text.is_empty() && thinking.is_none() && tools.is_empty() {
                    continue;
                }
                let model = str_field(message, "model");
                all.push(Dated {
                    ts,
                    entry: ThreadEntry::Assistant {
                        text,
                        truncated: text_truncated,
                        thinking,
                        tools,
                        model,
                    },
                });
            }
            Some("toolResult") => {
                let call_id = str_field(message, "toolCallId").unwrap_or_default();
                if let Some((_, idx)) = open_askuser.iter().rev().find(|(id, _)| *id == call_id) {
                    let idx = *idx;
                    if let Some(ThreadEntry::AskUser { answer, .. }) =
                        all.get_mut(idx).map(|d| &mut d.entry)
                    {
                        *answer = Some(
                            message.get("content").cloned().unwrap_or(serde_json::Value::Null),
                        );
                        if let Some(pos) = open_askuser.iter().position(|(_, i)| *i == idx) {
                            open_askuser.remove(pos);
                        }
                    }
                }
                // Standalone tool results are intentionally not rendered in v1:
                // their outcome is visible through the next assistant turn or
                // the error badge attached below.
                if message.get("isError").and_then(serde_json::Value::as_bool) == Some(true) {
                    // Errors stay visible, but as tool errors — never as if
                    // the user had typed them.
                    if let Some((text, truncated)) = text_blocks(message.get("content"), 500) {
                        all.push(Dated { ts, entry: ThreadEntry::ToolError { text, truncated } });
                    }
                }
            }
            _ => {}
        }
    }

    let waiting_askuser = !open_askuser.is_empty();
    let omitted = all.len().saturating_sub(MAX_ENTRIES);
    let entries = all.into_iter().skip(omitted).collect();
    Ok(ThreadView { entries, omitted, waiting_askuser })
}

fn as_array<'a>(value: &'a serde_json::Value) -> Option<&'a Vec<serde_json::Value>> {
    value.as_array()
}

fn str_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key).and_then(serde_json::Value::as_str).map(str::to_owned)
}

/// Joins text blocks into `(joined, truncated)`; returns `None` when no text.
fn text_blocks(content: Option<&serde_json::Value>, cap: usize) -> Option<(String, bool)> {
    let mut out = String::new();
    let mut truncated = false;
    match content {
        Some(serde_json::Value::String(text)) => {
            out.push_str(text);
        }
        Some(serde_json::Value::Array(blocks)) => {
            for block in blocks {
                if block.get("type").and_then(serde_json::Value::as_str) == Some("text") {
                    if let Some(part) = str_field(block, "text") {
                        push_block(&mut out, &mut truncated, &part, cap);
                    }
                }
            }
        }
        _ => return None,
    }
    if out.is_empty() { None } else { Some((out, truncated)) }
}

/// Appends one text part under a total char budget.
fn push_block(out: &mut String, truncated: &mut bool, part: &str, cap: usize) {
    if *truncated {
        return;
    }
    let remaining = cap.saturating_sub(out.chars().count());
    if part.chars().count() <= remaining {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(part);
    } else {
        let piece: String = part.chars().take(remaining).collect();
        if !out.is_empty() && !piece.is_empty() {
            out.push('\n');
        }
        out.push_str(&piece);
        out.push('\u{2026}');
        *truncated = true;
    }
}

/// Serializes a value to a bounded single-line preview.
fn compact_json(value: &serde_json::Value) -> String {
    let raw = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_owned());
    let raw = raw.trim();
    // File-bearing tools lose their path key to the preview cap, so hoist it
    // into a `[path]` prefix that stays first under truncation.
    if let Some(object) = value.as_object() {
        let path = ["filePath", "file_path", "path", "notebookPath"]
            .iter()
            .find_map(|key| object.get(*key).and_then(serde_json::Value::as_str));
        if let Some(path) = path {
            if !path.is_empty() {
                return truncate_chars(&format!("[{path}] {raw}"), ARGS_PREVIEW_CAP);
            }
        }
    }
    truncate_chars(raw, ARGS_PREVIEW_CAP)
}

/// Parses a record timestamp from an RFC3339 string or epoch-millis number.
fn parse_ts(value: &serde_json::Value) -> Option<Timestamp> {
    match value.get("timestamp") {
        Some(serde_json::Value::String(text)) => text.parse::<Timestamp>().ok(),
        Some(serde_json::Value::Number(n)) => {
            n.as_i64().and_then(|ms| Timestamp::from_millisecond(ms).ok())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const HEADER: &str =
        r#"{"type":"session","id":"t1","timestamp":"2026-01-01T00:00:00Z","cwd":"/p"}"#;

    fn write_thread(dir: &Path, lines: &[String]) -> PathBuf {
        let file = dir.join("t.jsonl");
        let mut body = String::from(HEADER);
        body.push('\n');
        for l in lines {
            body.push_str(l);
            body.push('\n');
        }
        std::fs::write(&file, body).expect("write thread");
        file
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pecan-thread-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn parses_user_assistant_and_tools() {
        let dir = temp_dir("basic");
        let file = write_thread(
            &dir,
            &[
                r#"{"type":"message","timestamp":"2026-01-01T00:00:01Z","message":{"role":"user","content":"run ls"}}"#.to_owned(),
                r#"{"type":"message","timestamp":1767225602000,"message":{"role":"assistant","model":"m1","content":[{"type":"text","text":"sure"},{"type":"toolCall","id":"c1","name":"bash","arguments":{"command":"ls"}}]}}"#.to_owned(),
                r#"{"type":"message","timestamp":"2026-01-01T00:00:03Z","message":{"role":"toolResult","toolCallId":"c1","toolName":"bash","isError":false,"content":[{"type":"text","text":"a.txt"}]}}"#.to_owned(),
            ],
        );
        let view = parse_thread(&file).expect("parse");
        assert_eq!(view.entries.len(), 2);
        assert!(
            matches!(view.entries[0].entry, ThreadEntry::User { ref text, .. } if text == "run ls")
        );
        let ThreadEntry::Assistant { text, tools, model, .. } = &view.entries[1].entry else {
            panic!("expected assistant entry");
        };
        assert_eq!(text, "sure");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "bash");
        assert_eq!(tools[0].name, "bash");
        assert_eq!(model.as_deref(), Some("m1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detects_waiting_askuser_then_resolves_on_answer() {
        let dir = temp_dir("askuser");
        let ask_call = r#"{"type":"message","timestamp":"2026-01-01T00:00:02Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"q1","name":"ask_user","arguments":{"questions":[{"id":"a","question":"go?"}]}}]}}"#;
        let ask_answer = r#"{"type":"message","timestamp":"2026-01-01T00:00:09Z","message":{"role":"toolResult","toolCallId":"q1","toolName":"ask_user","isError":false,"content":[{"type":"text","text":"yes"}]}}"#;

        let open_file = write_thread(&dir, &[ask_call.to_owned()]);
        assert!(parse_thread(&open_file).expect("parse").waiting_askuser);

        let closed_file = write_thread(&dir, &[ask_call.to_owned(), ask_answer.to_owned()]);
        let view = parse_thread(&closed_file).expect("parse");
        assert!(!view.waiting_askuser);
        match &view.entries[0].entry {
            ThreadEntry::AskUser { answer, .. } => assert!(answer.is_some()),
            other => panic!("expected askuser, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn surfaces_tool_errors_as_visible_rows() {
        let dir = temp_dir("errors");
        let file = write_thread(
            &dir,
            &[r#"{"type":"message","timestamp":"2026-01-01T00:00:02Z","message":{"role":"toolResult","toolCallId":"c9","toolName":"bash","isError":true,"content":[{"type":"text","text":"boom"}]}}"#.to_owned()],
        );
        let view = parse_thread(&file).expect("parse");
        assert!(
            matches!(view.entries[0].entry, ThreadEntry::ToolError { ref text, .. } if text.contains("boom"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
