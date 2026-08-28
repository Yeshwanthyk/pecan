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
/// Maximum characters retained for one deterministic activity summary.
const TOOL_SUMMARY_CAP: usize = 180;
/// Maximum characters retained for one displayed activity target.
const TOOL_TARGET_CAP: usize = 160;
/// Maximum target values retained in the API metadata.
const MAX_TOOL_TARGETS: usize = 8;
/// Maximum serialized tool details retained from a transcript result.
const TOOL_DETAILS_CAP: usize = 64 * 1024;
/// Maximum base64 characters retained for one inline chat thumbnail.
const IMAGE_DATA_CAP: usize = 2 * 1024 * 1024;
/// Maximum images retained from one user message.
const MAX_USER_IMAGES: usize = 4;
/// Maximum entries returned, counted from the tail of the transcript.
pub const MAX_ENTRIES: usize = 600;

/// Coarse deterministic grouping for a tool activity row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolCategory {
    /// Read-only local inspection.
    Inspect,
    /// A workspace or external state mutation.
    Change,
    /// A verification or validation operation.
    Check,
    /// A web or information retrieval operation.
    Research,
    /// Delegation to another agent.
    Agent,
    /// A task-plan operation.
    Task,
    /// Process or runtime control.
    Runtime,
    /// A tool that does not match a known safe grouping.
    Other,
}

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
    /// Deterministic human-readable description derived from full arguments.
    pub summary: String,
    /// Coarse category used to group activity in the client.
    pub category: ToolCategory,
    /// Bounded target values extracted from known argument fields.
    pub targets: Vec<String>,
    /// Number of target values found before the display bound was applied.
    #[serde(rename = "targetCount")]
    pub target_count: usize,
    /// Structured details returned by the tool, when the producer supplied them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// One bounded image attached to a user message.
#[derive(Debug, Clone, Serialize)]
pub struct UserImage {
    /// Validated raster MIME type.
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    /// Base64 payload without a data-URL prefix.
    pub data: String,
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
        /// Small, bounded image attachments shown with the prompt.
        images: Vec<UserImage>,
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
    // Tool results are recorded after their assistant message. Keep the
    // location so workflow/tool details can be joined without rendering the
    // result as a second transcript row.
    let mut tool_positions: std::collections::HashMap<String, (usize, usize)> =
        std::collections::HashMap::new();

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
                let (text, truncated) =
                    text_blocks(message.get("content"), BLOCK_TEXT_CAP).unwrap_or_default();
                let images = user_images(message.get("content"));
                if !text.is_empty() || !images.is_empty() {
                    all.push(Dated { ts, entry: ThreadEntry::User { text, truncated, images } });
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
                                let arguments = block.get("arguments");
                                if name == "ask_user" {
                                    let questions =
                                        arguments.cloned().unwrap_or(serde_json::Value::Null);
                                    open_askuser.push((id.clone(), all.len()));
                                    all.push(Dated {
                                        ts,
                                        entry: ThreadEntry::AskUser { questions, answer: None },
                                    });
                                } else {
                                    // Derive metadata while the complete structured arguments
                                    // are still available; args_preview is intentionally bounded
                                    // only after this step.
                                    let activity = tool_activity(&name, arguments);
                                    let args_preview =
                                        arguments.map(compact_json).unwrap_or_default();
                                    tools.push(ToolCall {
                                        tool_call_id: id,
                                        name,
                                        args_preview,
                                        summary: activity.summary,
                                        category: activity.category,
                                        targets: activity.targets,
                                        target_count: activity.target_count,
                                        details: None,
                                    });
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
                let entry_index = all.len();
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
                if let Some(ThreadEntry::Assistant { tools, .. }) =
                    all.get(entry_index).map(|dated| &dated.entry)
                {
                    for (tool_index, tool) in tools.iter().enumerate() {
                        if !tool.tool_call_id.is_empty() {
                            tool_positions
                                .insert(tool.tool_call_id.clone(), (entry_index, tool_index));
                        }
                    }
                }
            }
            Some("toolResult") => {
                let call_id = str_field(message, "toolCallId").unwrap_or_default();
                if let Some((entry_index, tool_index)) = tool_positions.get(&call_id).copied()
                    && let Some(ThreadEntry::Assistant { tools, .. }) =
                        all.get_mut(entry_index).map(|dated| &mut dated.entry)
                    && let Some(tool) = tools.get_mut(tool_index)
                {
                    tool.details = message.get("details").and_then(bounded_tool_details);
                }
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

/// Retains structured tool details only when they fit the transcript bound.
fn bounded_tool_details(value: &serde_json::Value) -> Option<serde_json::Value> {
    let encoded = serde_json::to_vec(value).ok()?;
    (encoded.len() <= TOOL_DETAILS_CAP).then(|| value.clone())
}

fn as_array<'a>(value: &'a serde_json::Value) -> Option<&'a Vec<serde_json::Value>> {
    value.as_array()
}

fn str_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key).and_then(serde_json::Value::as_str).map(str::to_owned)
}

/// Retains browser-safe raster attachments under a strict response-size cap.
fn user_images(content: Option<&serde_json::Value>) -> Vec<UserImage> {
    let Some(blocks) = content.and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    blocks
        .iter()
        .filter_map(|block| {
            if block.get("type").and_then(serde_json::Value::as_str) != Some("image") {
                return None;
            }
            let mime_type = block.get("mimeType").and_then(serde_json::Value::as_str)?;
            if !matches!(mime_type, "image/png" | "image/jpeg" | "image/gif" | "image/webp") {
                return None;
            }
            let data = block.get("data").and_then(serde_json::Value::as_str)?;
            if data.is_empty() || data.len() > IMAGE_DATA_CAP {
                return None;
            }
            Some(UserImage { mime_type: mime_type.to_owned(), data: data.to_owned() })
        })
        .take(MAX_USER_IMAGES)
        .collect()
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

#[derive(Debug)]
struct ToolActivity {
    summary: String,
    category: ToolCategory,
    targets: Vec<String>,
    target_count: usize,
}

/// Builds the stable activity metadata exposed to the web client.
///
/// This deliberately uses exact tool names and a small set of documented
/// argument keys. Unknown tools fall back to their name instead of inferring
/// intent from arbitrary argument text. Summary verbs are action labels: a
/// tool call records an invocation request, not a successful tool result.
fn tool_activity(name: &str, arguments: Option<&serde_json::Value>) -> ToolActivity {
    let normalized = name.to_ascii_lowercase();
    let category = tool_category(&normalized, arguments);
    let (targets, target_count) = activity_targets(&normalized, arguments);
    let summary = activity_summary(name, &normalized, category, arguments, &targets, target_count);
    ToolActivity { summary, category, targets, target_count }
}

fn tool_category(name: &str, arguments: Option<&serde_json::Value>) -> ToolCategory {
    if name.starts_with("subagent_") || name == "subagent" {
        return ToolCategory::Agent;
    }
    if name.starts_with("task") {
        return ToolCategory::Task;
    }
    if name.starts_with("bg_") {
        return ToolCategory::Runtime;
    }
    match name {
        "read" | "get" | "grep" | "find" | "ls" => ToolCategory::Inspect,
        "write" | "edit" | "create" | "delete" | "remove" | "move" | "rename" | "copy"
        | "multiedit" => ToolCategory::Change,
        "source_check" | "check" | "test" | "lint" | "typecheck" | "build" => ToolCategory::Check,
        "web_search" | "fetch" | "fetch_content" | "get_search_content" => ToolCategory::Research,
        "bash" | "shell" | "exec" => bash_category(arguments),
        _ => ToolCategory::Other,
    }
}

fn bash_category(arguments: Option<&serde_json::Value>) -> ToolCategory {
    let Some(command) = string_argument(arguments, &["command"]) else {
        return ToolCategory::Runtime;
    };
    let command = command.to_ascii_lowercase();
    if command.contains("cargo test")
        || command.contains("cargo test-all")
        || command.contains("cargo fmt-check")
        || command.contains("cargo fmt --check")
        || command.contains("cargo lint")
        || command.contains("cargo clippy")
        || command.contains("cargo check")
        || command.contains("cargo build")
        || command.contains("npm run build")
        || command.contains("npm run lint")
        || command.contains("npm run typecheck")
        || command.contains("npx tsc")
    {
        ToolCategory::Check
    } else {
        ToolCategory::Runtime
    }
}

fn activity_targets(name: &str, arguments: Option<&serde_json::Value>) -> (Vec<String>, usize) {
    let keys: &[&str] = if matches!(
        name,
        "read"
            | "write"
            | "edit"
            | "create"
            | "delete"
            | "remove"
            | "move"
            | "rename"
            | "copy"
            | "multiedit"
            | "check"
            | "test"
            | "lint"
            | "typecheck"
            | "build"
    ) {
        &["filePath", "file_path", "path", "paths", "files", "notebookPath", "notebook_path"]
    } else if matches!(name, "grep" | "find" | "ls") {
        &["path"]
    } else if name == "source_check" {
        &["claim"]
    } else if name == "web_search" {
        &["query", "queries"]
    } else if matches!(name, "fetch" | "fetch_content") {
        &["url", "urls"]
    } else if name == "get_search_content" {
        &["url", "query", "findText"]
    } else if name == "get" {
        &["filePath", "file_path", "path", "url", "urls"]
    } else if name.starts_with("subagent_") || name == "subagent" {
        &["name", "agent", "agentName", "agent_name", "id", "ids"]
    } else if name.starts_with("task") {
        &["subject", "title", "task", "id", "ids", "taskId", "task_id", "task_ids", "shell_id"]
    } else if name.starts_with("bg_") {
        &["title", "name", "id", "ids", "process", "command"]
    } else {
        &[]
    };
    let (mut targets, mut target_count) = collect_targets(arguments, keys);
    if matches!(name, "edit" | "multiedit") {
        collect_edit_targets(arguments, &mut targets, &mut target_count);
    }
    (targets, target_count)
}

fn collect_edit_targets(
    arguments: Option<&serde_json::Value>,
    targets: &mut Vec<String>,
    target_count: &mut usize,
) {
    let Some(script) = string_argument(arguments, &["text"]) else { return };
    for line in script.lines() {
        let Some(path) = line.strip_prefix('[').and_then(|line| line.strip_suffix(']')) else {
            continue;
        };
        if path.is_empty() || targets.iter().any(|target| target == path) {
            continue;
        }
        *target_count = target_count.saturating_add(1);
        if targets.len() < MAX_TOOL_TARGETS {
            targets.push(truncate_chars(path, TOOL_TARGET_CAP));
        }
    }
}

fn collect_targets(arguments: Option<&serde_json::Value>, keys: &[&str]) -> (Vec<String>, usize) {
    let Some(arguments) = arguments else { return (Vec::new(), 0) };
    let mut targets = Vec::new();
    let mut target_count = 0;
    for key in keys {
        if let Some(value) = arguments.get(*key) {
            collect_target_value(value, &mut targets, &mut target_count);
        }
    }
    (targets, target_count)
}

fn collect_target_value(value: &serde_json::Value, targets: &mut Vec<String>, count: &mut usize) {
    match value {
        serde_json::Value::String(target) if !target.is_empty() => {
            *count = count.saturating_add(1);
            if targets.len() < MAX_TOOL_TARGETS {
                targets.push(truncate_chars(target, TOOL_TARGET_CAP));
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_target_value(value, targets, count);
            }
        }
        _ => {}
    }
}

fn activity_summary(
    original_name: &str,
    normalized_name: &str,
    category: ToolCategory,
    arguments: Option<&serde_json::Value>,
    targets: &[String],
    target_count: usize,
) -> String {
    let summary = match normalized_name {
        "read" | "get" => summarize_targets("Read", "input", targets, target_count),
        "grep" => summarize_pattern("Search for", arguments, targets, target_count),
        "find" => summarize_pattern("Find", arguments, targets, target_count),
        "ls" => summarize_targets("List", "files", targets, target_count),
        "write" => summarize_targets("Write", "request", targets, target_count),
        "edit" | "multiedit" => summarize_targets("Edit", "request", targets, target_count),
        "create" => summarize_targets("Create", "request", targets, target_count),
        "delete" | "remove" => summarize_targets("Delete", "request", targets, target_count),
        "move" => summarize_targets("Move", "request", targets, target_count),
        "rename" => summarize_targets("Rename", "request", targets, target_count),
        "copy" => summarize_targets("Copy", "request", targets, target_count),
        "source_check" => summarize_quoted("Check claim", arguments, &["claim"]),
        "check" => summarize_targets("Check", "input", targets, target_count),
        "test" => summarize_targets("Test", "input", targets, target_count),
        "lint" => summarize_targets("Lint", "input", targets, target_count),
        "typecheck" => summarize_targets("Typecheck", "input", targets, target_count),
        "build" => summarize_targets("Build", "input", targets, target_count),
        "web_search" => summarize_quoted("Search for", arguments, &["query", "queries"]),
        "fetch" | "fetch_content" => summarize_value("Fetch", arguments, &["url", "urls"]),
        "get_search_content" => summarize_value("Read search result", arguments, &["url", "query"]),
        "bash" | "shell" | "exec" => bash_summary(arguments),
        name if name.starts_with("subagent_") || name == "subagent" => {
            subagent_summary(name, arguments)
        }
        name if name.starts_with("task") => task_summary(original_name, name, arguments),
        name if name.starts_with("bg_") => background_summary(original_name, name, arguments),
        _ => match category {
            ToolCategory::Check => format!("Check {}", truncate_chars(original_name, 120)),
            _ => format!("Run {}", truncate_chars(original_name, 120)),
        },
    };
    truncate_chars(&summary, TOOL_SUMMARY_CAP)
}

fn summarize_targets(
    verb: &str,
    empty_target: &str,
    targets: &[String],
    target_count: usize,
) -> String {
    match target_count {
        0 => format!("{verb} {empty_target}"),
        1 => targets
            .first()
            .map_or_else(|| format!("{verb} 1 target"), |target| format!("{verb} {target}")),
        count => {
            let shown = targets.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
            if shown.is_empty() {
                format!("{verb} {count} targets")
            } else {
                format!("{verb} {count} targets ({shown})")
            }
        }
    }
}

fn summarize_pattern(
    verb: &str,
    arguments: Option<&serde_json::Value>,
    targets: &[String],
    target_count: usize,
) -> String {
    let pattern = string_argument(arguments, &["pattern"]);
    let location = targets.first().map(|target| format!(" in {target}"));
    match pattern {
        Some(pattern) => {
            format!("{verb} \"{}\"{}", truncate_chars(pattern, 110), location.unwrap_or_default())
        }
        None if target_count > 0 => format!("{verb}{}", location.unwrap_or_default()),
        None => format!("{verb} input"),
    }
}

fn summarize_quoted(verb: &str, arguments: Option<&serde_json::Value>, keys: &[&str]) -> String {
    string_argument(arguments, keys).map_or_else(
        || format!("{verb} command"),
        |value| format!("{verb} \"{}\"", truncate_chars(value, 120)),
    )
}

fn summarize_value(verb: &str, arguments: Option<&serde_json::Value>, keys: &[&str]) -> String {
    string_argument(arguments, keys).map_or_else(
        || format!("{verb} resource"),
        |value| format!("{verb} {}", truncate_chars(value, 140)),
    )
}

fn bash_summary(arguments: Option<&serde_json::Value>) -> String {
    let Some(command) = string_argument(arguments, &["command"]) else {
        return "Run shell command".to_owned();
    };
    let lower = command.to_ascii_lowercase();
    let label = if lower.contains("cargo test-all") || lower.contains("cargo test") {
        Some("Run Rust tests")
    } else if lower.contains("cargo fmt-check") || lower.contains("cargo fmt --check") {
        Some("Check Rust formatting")
    } else if lower.contains("cargo lint") || lower.contains("cargo clippy") {
        Some("Lint Rust workspace")
    } else if lower.contains("cargo check") {
        Some("Check Rust workspace")
    } else if lower.contains("cargo build") {
        Some("Build Rust workspace")
    } else if lower.contains("npm run typecheck") || lower.contains("npx tsc") {
        Some("Typecheck web app")
    } else if lower.contains("npm run lint") {
        Some("Lint web app")
    } else if lower.contains("npm run build") {
        Some("Build web app")
    } else if lower.contains("git status") {
        Some("Inspect workspace status")
    } else if lower.contains("git diff") {
        Some("Inspect workspace changes")
    } else if lower.contains("tmux list-") {
        Some("List tmux sessions")
    } else if lower.starts_with("ps ") || lower.contains(" pgrep ") || lower.contains("lsof ") {
        Some("Inspect running processes")
    } else {
        None
    };
    label.map_or_else(|| format!("Run \"{}\"", truncate_chars(command, 120)), str::to_owned)
}

fn subagent_summary(name: &str, arguments: Option<&serde_json::Value>) -> String {
    match name {
        "subagent_spawn" => {
            if let Some(agent) =
                string_argument(arguments, &["name", "agent", "agentName", "agent_name"])
            {
                return format!("Spawn subagent \"{}\"", truncate_chars(agent, 120));
            }
            string_argument(arguments, &["prompt", "task"]).map_or_else(
                || "Spawn subagent".to_owned(),
                |prompt| format!("Delegate \"{}\"", truncate_chars(prompt, 120)),
            )
        }
        "subagent_wait" => summarize_ids("Wait for subagents", arguments, &["ids", "id"]),
        "subagent_cancel" => summarize_ids("Cancel subagents", arguments, &["ids", "id"]),
        "subagent_check" => summarize_ids("Check subagent", arguments, &["id", "ids"]),
        "subagent_list" => "List subagents".to_owned(),
        _ => format!("Run {}", truncate_chars(name, 100)),
    }
}

fn string_argument<'a>(arguments: Option<&'a serde_json::Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| arguments.and_then(|value| value.get(*key)).and_then(first_argument_string))
}

fn first_argument_string(value: &serde_json::Value) -> Option<&str> {
    match value {
        serde_json::Value::String(value) if !value.is_empty() => Some(value),
        serde_json::Value::Array(values) => values.iter().find_map(first_argument_string),
        _ => None,
    }
}

fn argument_strings<'a>(arguments: Option<&'a serde_json::Value>, keys: &[&str]) -> Vec<&'a str> {
    let Some(arguments) = arguments else { return Vec::new() };
    let mut values = Vec::new();
    for key in keys {
        if let Some(value) = arguments.get(*key) {
            collect_argument_strings(value, &mut values);
        }
    }
    values
}

fn collect_argument_strings<'a>(value: &'a serde_json::Value, values: &mut Vec<&'a str>) {
    match value {
        serde_json::Value::String(value) if !value.is_empty() => values.push(value),
        serde_json::Value::Array(items) => {
            for item in items {
                collect_argument_strings(item, values);
            }
        }
        _ => {}
    }
}

fn task_summary(
    original_name: &str,
    normalized_name: &str,
    arguments: Option<&serde_json::Value>,
) -> String {
    let subject = string_argument(arguments, &["subject", "title", "task"]);
    let task_ids = argument_strings(arguments, &["taskId", "task_id", "task_ids", "id", "ids"]);
    if normalized_name.contains("create") {
        return subject.map_or_else(
            || "Create task".to_owned(),
            |value| format!("Create task \"{}\"", truncate_chars(value, 110)),
        );
    }
    if normalized_name.contains("delete") || normalized_name.contains("remove") {
        return summarize_task_ids_or_subject("Delete task", task_ids, subject);
    }
    if normalized_name.contains("list") {
        return "List tasks".to_owned();
    }
    if normalized_name.contains("output") {
        return summarize_task_ids("Read task output", arguments, &["task_id", "taskId", "id"]);
    }
    if normalized_name.contains("stop") {
        return summarize_task_ids(
            "Stop task",
            arguments,
            &["task_id", "taskId", "id", "shell_id"],
        );
    }
    if normalized_name.contains("execute") {
        return summarize_task_ids(
            "Execute tasks",
            arguments,
            &["task_ids", "taskId", "task_id", "ids"],
        );
    }
    if normalized_name.contains("claim") {
        return summarize_task_ids("Claim task", arguments, &["taskId", "task_id", "id"]);
    }
    if normalized_name.contains("get") {
        return summarize_task_ids("Get task", arguments, &["taskId", "task_id", "id"]);
    }
    let update_verb = if normalized_name == "taskupdate" {
        "Update task".to_owned()
    } else {
        format!("Update task via {}", truncate_chars(original_name, 90))
    };
    summarize_task_ids_or_subject(&update_verb, task_ids, subject)
}

fn summarize_ids(verb: &str, arguments: Option<&serde_json::Value>, keys: &[&str]) -> String {
    summarize_id_values(verb, argument_strings(arguments, keys))
}

fn summarize_task_ids(verb: &str, arguments: Option<&serde_json::Value>, keys: &[&str]) -> String {
    summarize_task_id_values(verb, argument_strings(arguments, keys))
}

fn summarize_id_values(verb: &str, ids: Vec<&str>) -> String {
    match ids.as_slice() {
        [] => verb.to_owned(),
        [id] => format!("{verb} {}", truncate_chars(id, 100)),
        _ => format!(
            "{verb} {} ({})",
            ids.len(),
            ids.iter().take(3).map(|id| truncate_chars(id, 60)).collect::<Vec<_>>().join(", ")
        ),
    }
}

fn summarize_task_id_values(verb: &str, ids: Vec<&str>) -> String {
    match ids.as_slice() {
        [] => verb.to_owned(),
        [id] => format!("{verb} #{}", truncate_chars(id, 100)),
        _ => {
            let shown = ids
                .iter()
                .take(6)
                .map(|id| format!("#{}", truncate_chars(id, 60)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{verb} {shown}")
        }
    }
}

fn summarize_task_ids_or_subject(verb: &str, ids: Vec<&str>, subject: Option<&str>) -> String {
    if !ids.is_empty() {
        return summarize_task_id_values(verb, ids);
    }
    subject.map_or_else(
        || verb.to_owned(),
        |value| format!("{verb} \"{}\"", truncate_chars(value, 110)),
    )
}

fn background_summary(
    original_name: &str,
    normalized_name: &str,
    arguments: Option<&serde_json::Value>,
) -> String {
    if normalized_name == "bg_list" {
        return "List background terminals".to_owned();
    }
    if normalized_name == "bg_status" {
        return summarize_ids("Check background terminal", arguments, &["id"]);
    }
    if normalized_name == "bg_kill" {
        return summarize_ids("Stop background terminals", arguments, &["ids", "id"]);
    }
    let target = string_argument(arguments, &["title", "name", "process", "command"]);
    let action = if normalized_name == "bg_start" {
        "Start background terminal"
    } else if normalized_name.ends_with("_stop") || normalized_name.ends_with("_kill") {
        "Stop background process"
    } else if normalized_name.ends_with("_status") || normalized_name.ends_with("_list") {
        "Check background process"
    } else {
        return format!("Run {}", truncate_chars(original_name, 120));
    };
    target.map_or_else(
        || action.to_owned(),
        |value| format!("{action} {}", truncate_chars(value, 100)),
    )
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

    fn string_value(value: &str) -> serde_json::Value {
        serde_json::Value::String(value.to_owned())
    }

    fn object_value(fields: &[(&str, serde_json::Value)]) -> serde_json::Value {
        serde_json::Value::Object(
            fields.iter().map(|(key, value)| ((*key).to_owned(), value.clone())).collect(),
        )
    }

    fn string_array(values: &[&str]) -> serde_json::Value {
        serde_json::Value::Array(values.iter().map(|value| string_value(value)).collect())
    }

    #[test]
    fn joins_workflow_tool_details_from_tool_result() {
        let dir = temp_dir("tool-details");
        let file = write_thread(
            &dir,
            &[
                r#"{"type":"message","timestamp":"2026-01-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"wf-call","name":"workflow","arguments":{"script":"return"}}]}}"#.to_owned(),
                r#"{"type":"message","timestamp":"2026-01-01T00:00:02Z","message":{"role":"toolResult","toolCallId":"wf-call","toolName":"workflow","isError":false,"details":{"kind":"draft","draftId":"draft_abc123","artifactPath":"/tmp/draft.json","script":"phase()"},"content":[{"type":"text","text":"draft ready"}]}}"#.to_owned(),
            ],
        );
        let view = parse_thread(&file).expect("parse");
        let ThreadEntry::Assistant { tools, .. } = &view.entries[0].entry else {
            panic!("expected assistant entry");
        };
        assert_eq!(
            tools[0]
                .details
                .as_ref()
                .and_then(|value| value.get("kind"))
                .and_then(serde_json::Value::as_str),
            Some("draft")
        );
        assert_eq!(
            tools[0]
                .details
                .as_ref()
                .and_then(|value| value.get("artifactPath"))
                .and_then(serde_json::Value::as_str),
            Some("/tmp/draft.json")
        );
        let _ = std::fs::remove_dir_all(&dir);
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
        assert!(tools[0].details.is_none());
        assert_eq!(model.as_deref(), Some("m1"));
        let serialized = serde_json::to_value(&tools[0]).expect("serialize tool call");
        assert_eq!(serialized["toolCallId"], "c1");
        assert_eq!(serialized["argsPreview"], "{\"command\":\"ls\"}");
        assert_eq!(serialized["summary"], "Run \"ls\"");
        assert_eq!(serialized["category"], "runtime");
        assert_eq!(serialized["targetCount"], 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn derives_activity_from_full_arguments_before_preview_truncation() {
        let dir = temp_dir("tool-activity");
        let arguments = object_value(&[
            ("path", string_value("src/main.rs")),
            ("content", string_value(&"x".repeat(1_000))),
        ]);
        let record = object_value(&[
            ("type", string_value("message")),
            ("timestamp", string_value("2026-01-01T00:00:01Z")),
            (
                "message",
                object_value(&[
                    ("role", string_value("assistant")),
                    (
                        "content",
                        serde_json::Value::Array(vec![object_value(&[
                            ("type", string_value("toolCall")),
                            ("id", string_value("write-1")),
                            ("name", string_value("write")),
                            ("arguments", arguments),
                        ])]),
                    ),
                ]),
            ),
        ]);
        let file =
            write_thread(&dir, &[serde_json::to_string(&record).expect("serialize tool activity")]);
        let view = parse_thread(&file).expect("parse");
        let ThreadEntry::Assistant { tools, .. } = &view.entries[0].entry else {
            panic!("expected assistant entry");
        };
        let Some(tool) = tools.first() else {
            panic!("expected write tool");
        };
        assert_eq!(tool.category, ToolCategory::Change);
        assert_eq!(tool.summary, "Write src/main.rs");
        assert_eq!(tool.targets, ["src/main.rs"]);
        assert_eq!(tool.target_count, 1);
        assert_eq!(tool.args_preview.chars().count(), ARGS_PREVIEW_CAP);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn classifies_common_tools_with_conservative_fallbacks() {
        let cases = [
            ("read", ToolCategory::Inspect, "Read src/lib.rs"),
            ("edit", ToolCategory::Change, "Edit src/lib.rs"),
            ("grep", ToolCategory::Inspect, "Search for \"TODO\" in src"),
            ("find", ToolCategory::Inspect, "Find \"*.rs\" in src"),
            ("ls", ToolCategory::Inspect, "List src"),
            ("bash", ToolCategory::Check, "Run Rust tests"),
            ("web_search", ToolCategory::Research, "Search for \"rust async\""),
            ("fetch", ToolCategory::Research, "Fetch https://example.com"),
            ("fetch_content", ToolCategory::Research, "Fetch https://example.com"),
            ("get_search_content", ToolCategory::Research, "Read search result rust async"),
            ("get", ToolCategory::Inspect, "Read src/lib.rs"),
            ("source_check", ToolCategory::Check, "Check claim \"the claim\""),
            ("check", ToolCategory::Check, "Check src/lib.rs"),
            ("subagent_spawn", ToolCategory::Agent, "Spawn subagent \"scout\""),
            ("subagent_wait", ToolCategory::Agent, "Wait for subagents sa-1"),
            ("subagent_check", ToolCategory::Agent, "Check subagent sa-1"),
            ("TaskCreate", ToolCategory::Task, "Create task \"activity\""),
            ("TaskList", ToolCategory::Task, "List tasks"),
            ("TaskGet", ToolCategory::Task, "Get task #7"),
            ("TaskUpdate", ToolCategory::Task, "Update task #7"),
            ("TaskClaim", ToolCategory::Task, "Claim task #7"),
            ("TaskOutput", ToolCategory::Task, "Read task output #7"),
            ("TaskStop", ToolCategory::Task, "Stop task #7"),
            ("TaskExecute", ToolCategory::Task, "Execute tasks #1, #2"),
            ("bg_start", ToolCategory::Runtime, "Start background terminal dev"),
            ("bg_status", ToolCategory::Runtime, "Check background terminal term-1"),
            ("bg_list", ToolCategory::Runtime, "List background terminals"),
            ("bg_kill", ToolCategory::Runtime, "Stop background terminals 2 (term-1, term-2)"),
            ("vendor_tool", ToolCategory::Other, "Run vendor_tool"),
        ];
        for (name, category, summary) in cases {
            let arguments = match name {
                "read" | "edit" | "get" | "check" => {
                    object_value(&[("path", string_value("src/lib.rs"))])
                }
                "source_check" => object_value(&[("claim", string_value("the claim"))]),
                "grep" => object_value(&[
                    ("pattern", string_value("TODO")),
                    ("path", string_value("src")),
                ]),
                "find" => object_value(&[
                    ("pattern", string_value("*.rs")),
                    ("path", string_value("src")),
                ]),
                "ls" => object_value(&[("path", string_value("src"))]),
                "bash" => object_value(&[("command", string_value("cargo test"))]),
                "web_search" => object_value(&[("query", string_value("rust async"))]),
                "fetch" | "fetch_content" => {
                    object_value(&[("url", string_value("https://example.com"))])
                }
                "get_search_content" => object_value(&[("query", string_value("rust async"))]),
                "subagent_spawn" => object_value(&[
                    ("name", string_value("scout")),
                    ("prompt", string_value("review this")),
                ]),
                "subagent_wait" | "subagent_check" => {
                    object_value(&[("ids", string_array(&["sa-1"]))])
                }
                "TaskCreate" => object_value(&[("subject", string_value("activity"))]),
                "TaskList" => object_value(&[]),
                "TaskGet" | "TaskUpdate" => object_value(&[("taskId", string_value("7"))]),
                "TaskClaim" => object_value(&[("taskId", string_value("7"))]),
                "TaskOutput" => object_value(&[("task_id", string_value("7"))]),
                "TaskStop" => object_value(&[("shell_id", string_value("7"))]),
                "TaskExecute" => object_value(&[("task_ids", string_array(&["1", "2"]))]),
                "bg_start" => object_value(&[("name", string_value("dev"))]),
                "bg_status" => object_value(&[("id", string_value("term-1"))]),
                "bg_kill" => object_value(&[("ids", string_array(&["term-1", "term-2"]))]),
                _ => object_value(&[]),
            };
            let activity = tool_activity(name, Some(&arguments));
            assert_eq!(activity.category, category, "category for {name}");
            assert_eq!(activity.summary, summary, "summary for {name}");
        }
        let query_batch = object_value(&[("queries", string_array(&["first", "second"]))]);
        assert_eq!(tool_activity("web_search", Some(&query_batch)).summary, "Search for \"first\"");
    }

    #[test]
    fn extracts_edit_targets_from_row_script_headers() {
        let arguments = object_value(&[(
            "text",
            string_value("[src/lib.rs]\n@APPEND\n+line\n[web/src/app.tsx]\n@DEL 4"),
        )]);
        let activity = tool_activity("edit", Some(&arguments));
        assert_eq!(activity.targets, ["src/lib.rs", "web/src/app.tsx"]);
        assert_eq!(activity.target_count, 2);
        assert_eq!(activity.summary, "Edit 2 targets (src/lib.rs, web/src/app.tsx)");
    }

    #[test]
    fn activity_summary_truncates_utf8_safely() {
        let command = "e\u{301}".repeat(300);
        let arguments = object_value(&[("command", serde_json::Value::String(command))]);
        let activity = tool_activity("bash", Some(&arguments));
        assert!(activity.summary.chars().count() <= TOOL_SUMMARY_CAP);
        assert!(activity.summary.is_char_boundary(activity.summary.len()));
        assert!(activity.summary.contains('\u{2026}'));
    }

    #[test]
    fn parses_bounded_raster_images_from_user_messages() {
        let dir = temp_dir("user-images");
        let file = write_thread(
            &dir,
            &[r#"{"type":"message","timestamp":"2026-01-01T00:00:01Z","message":{"role":"user","content":[{"type":"text","text":"see this"},{"type":"image","mimeType":"image/png","data":"cG5n"},{"type":"image","mimeType":"image/svg+xml","data":"PHN2Zz4="}]}}"#.to_owned()],
        );
        let view = parse_thread(&file).expect("parse");
        let ThreadEntry::User { text, images, .. } = &view.entries[0].entry else {
            panic!("expected user entry");
        };
        assert_eq!(text, "see this");
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].mime_type, "image/png");
        assert_eq!(images[0].data, "cG5n");
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
