//! Bounded, ephemeral thread-title generation through the local Pi CLI.

use std::path::Path;
use std::time::Duration;

use pecan_core::session::truncate_chars;
use pecan_core::thread::{ThreadEntry, ThreadView};

const PROVIDER: &str = "openai-codex";
const GENERATION_TIMEOUT: Duration = Duration::from_secs(45);
const TRANSCRIPT_LIMIT: usize = 8_000;
const TITLE_LIMIT: usize = 50;
const FALLBACK_TITLE: &str = "New thread";

/// Small, audited set of model configurations allowed for title generation.
///
/// Keeping the wire value as a preset prevents callers from passing arbitrary
/// providers, models, or reasoning levels into the Pi process boundary.
#[derive(Clone, Copy, Debug, Default, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum TitleModelPreset {
    /// Fastest and least expensive option; used when no preset is supplied.
    #[default]
    LunaLow,
    /// More reasoning for titles whose subject changes across a long thread.
    LunaMedium,
    /// A larger model with low reasoning for more nuanced wording.
    SolLow,
}

impl TitleModelPreset {
    fn config(self) -> TitleModelConfig {
        match self {
            Self::LunaLow => TitleModelConfig { model: "gpt-5.6-luna", thinking: "low" },
            Self::LunaMedium => TitleModelConfig { model: "gpt-5.6-luna", thinking: "medium" },
            Self::SolLow => TitleModelConfig { model: "gpt-5.6-sol", thinking: "low" },
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct TitleModelConfig {
    model: &'static str,
    thinking: &'static str,
}

/// Failures from the isolated title-generation process.
#[derive(Debug, thiserror::Error)]
pub(super) enum TitleError {
    /// Pi could not be started.
    #[error("could not start Pi title generation: {0}")]
    Start(#[source] std::io::Error),
    /// The bounded generation window expired.
    #[error("Pi title generation timed out after 45 seconds")]
    Timeout,
    /// Pi exited unsuccessfully.
    #[error("Pi title generation failed: {0}")]
    Failed(String),
    /// Pi returned no usable title.
    #[error("Pi returned no usable thread title")]
    Empty,
}

/// Generates a title with an ephemeral, tool-free Pi invocation from an audited preset.
pub(super) async fn generate(
    cwd: &Path,
    thread: &ThreadView,
    previous_title: Option<&str>,
    preset: TitleModelPreset,
) -> Result<String, TitleError> {
    let prompt = build_prompt(thread, previous_title);
    let config = preset.config();
    let mut command = tokio::process::Command::new("pi");
    command
        .args([
            "--print",
            "--no-session",
            "--no-tools",
            "--no-extensions",
            "--no-skills",
            "--no-context-files",
            "--provider",
            PROVIDER,
            "--model",
            config.model,
            "--thinking",
            config.thinking,
        ])
        .arg(prompt)
        .current_dir(cwd)
        .kill_on_drop(true);

    let output = tokio::time::timeout(GENERATION_TIMEOUT, command.output())
        .await
        .map_err(|_elapsed| TitleError::Timeout)?
        .map_err(TitleError::Start)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.lines().next().map(str::trim).filter(|line| !line.is_empty());
        return Err(TitleError::Failed(detail.map_or_else(
            || format!("process exited with {}", output.status),
            |line| truncate_chars(line, 300),
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let candidate = extract_title(&stdout);
    let title = sanitize_thread_title(&candidate);
    if title == FALLBACK_TITLE {
        return Err(TitleError::Empty);
    }
    Ok(title)
}

fn build_prompt(thread: &ThreadView, previous_title: Option<&str>) -> String {
    let editorial_rules = r#"Editorial rules:
- Use 3-8 words and fewer than 40 characters when possible.
- Use a compact noun phrase or clear action phrase.
- Capture the durable subject and desired outcome, not workflow instructions.
- Omit models, subagents, tools, output formats, tests, commits, and monitoring unless they are the topic.
- Do not claim the work is complete.
- Avoid project names already visible in the UI, quotes, labels, filler, and trailing punctuation.
- Return only the title with no label, JSON, or commentary."#;

    if let Some(previous) = previous_title {
        let context = recent_thread_context(thread);
        format!(
            "Regenerate the title for an existing Pecan thread so the user can recognize it weeks later.\n\
             The previous title was \"{}\".\n\
             Read USER messages first and identify the latest explicit durable goal. Use ASSISTANT messages only to resolve vague subjects. Preserve accurate scope when the goal has not changed. Return a meaningfully improved title.\n\n\
             {editorial_rules}\n\nThread contents:\n{context}",
            previous.replace('"', "\\\"")
        )
    } else {
        let message = first_user_message(thread);
        format!(
            "Generate a title that will help the user recognize this Pecan thread weeks later.\n\
             Silently reduce the request to its subject, desired outcome, and incidental instructions. Title the subject and outcome; discard incidental instructions.\n\n\
             {editorial_rules}\n\nUser message:\n{message}"
        )
    }
}

fn first_user_message(thread: &ThreadView) -> String {
    thread
        .entries
        .iter()
        .find_map(|dated| match &dated.entry {
            ThreadEntry::User { text, .. } => Some(take_chars(text, TRANSCRIPT_LIMIT)),
            _ => None,
        })
        .unwrap_or_default()
}

fn recent_thread_context(thread: &ThreadView) -> String {
    let mut chunks = Vec::new();
    let mut chars = 0_usize;
    for dated in thread.entries.iter().rev() {
        let chunk = match &dated.entry {
            ThreadEntry::User { text, .. } => Some(format!("USER:\n{text}")),
            ThreadEntry::Assistant { text, .. } if !text.trim().is_empty() => {
                Some(format!("ASSISTANT:\n{text}"))
            }
            _ => None,
        };
        let Some(chunk) = chunk else { continue };
        chars = chars.saturating_add(chunk.chars().count());
        chunks.push(chunk);
        if chars >= TRANSCRIPT_LIMIT {
            break;
        }
    }
    chunks.reverse();
    let joined = chunks.join("\n\n");
    if joined.chars().count() <= TRANSCRIPT_LIMIT {
        joined
    } else {
        let tail: String = joined.chars().rev().take(TRANSCRIPT_LIMIT).collect();
        let ordered: String = tail.chars().rev().collect();
        format!("[Earlier content truncated]\n\n{ordered}")
    }
}

fn extract_title(raw: &str) -> String {
    let parsed = serde_json::from_str::<serde_json::Value>(raw.trim()).ok();
    parsed
        .as_ref()
        .and_then(|value| value.get("title"))
        .and_then(serde_json::Value::as_str)
        .map_or_else(|| raw.to_owned(), str::to_owned)
}

fn sanitize_thread_title(raw: &str) -> String {
    let Some(line) = raw.trim().lines().next() else {
        return FALLBACK_TITLE.to_owned();
    };
    let unquoted = line.trim().trim_matches(['\'', '"', '`']);
    let normalized = unquoted.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return FALLBACK_TITLE.to_owned();
    }
    if normalized.chars().count() <= TITLE_LIMIT {
        return normalized;
    }
    let prefix: String = normalized.chars().take(47).collect();
    format!("{}...", prefix.trim_end())
}

fn take_chars(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pecan_core::thread::Dated;

    fn thread(entries: Vec<ThreadEntry>) -> ThreadView {
        ThreadView {
            entries: entries.into_iter().map(|entry| Dated { ts: None, entry }).collect(),
            omitted: 0,
            waiting_askuser: false,
        }
    }

    #[test]
    fn sanitizer_is_single_line_unquoted_and_utf8_safe() {
        assert_eq!(
            sanitize_thread_title("  `Fix session recovery`\nextra"),
            "Fix session recovery"
        );
        let long = format!("\"{}\"", "é".repeat(60));
        let sanitized = sanitize_thread_title(&long);
        assert_eq!(sanitized.chars().count(), 50);
        assert!(sanitized.ends_with("..."), "long title should end in ellipsis");
        assert_eq!(sanitize_thread_title("  \n"), FALLBACK_TITLE);
    }

    #[test]
    fn prompt_distinguishes_initial_and_regenerated_titles() {
        let view = thread(vec![
            ThreadEntry::User {
                text: "Investigate reconnect regressions".to_owned(),
                truncated: false,
                images: Vec::new(),
            },
            ThreadEntry::Assistant {
                text: "The session state is stale".to_owned(),
                truncated: false,
                thinking: None,
                tools: Vec::new(),
                model: None,
            },
        ]);
        let initial = build_prompt(&view, None);
        assert!(initial.contains("User message:\nInvestigate reconnect regressions"));
        assert!(initial.contains("discard incidental instructions"));

        let regenerated = build_prompt(&view, Some("Reconnect review"));
        assert!(regenerated.contains("previous title was \"Reconnect review\""));
        assert!(regenerated.contains("USER:\nInvestigate reconnect regressions"));
        assert!(regenerated.contains("ASSISTANT:\nThe session state is stale"));
    }

    #[test]
    fn regeneration_context_keeps_recent_bounded_content() {
        let view = thread(vec![
            ThreadEntry::User { text: "old ".repeat(3_000), truncated: false, images: Vec::new() },
            ThreadEntry::User {
                text: "latest durable goal".to_owned(),
                truncated: false,
                images: Vec::new(),
            },
        ]);
        let context = recent_thread_context(&view);
        assert!(context.contains("[Earlier content truncated]"));
        assert!(context.contains("latest durable goal"));
        assert!(context.chars().count() <= TRANSCRIPT_LIMIT + 31);
    }

    #[test]
    fn presets_map_only_to_audited_pi_configurations() {
        assert_eq!(
            TitleModelPreset::default().config(),
            TitleModelConfig { model: "gpt-5.6-luna", thinking: "low" }
        );
        assert_eq!(
            TitleModelPreset::LunaMedium.config(),
            TitleModelConfig { model: "gpt-5.6-luna", thinking: "medium" }
        );
        assert_eq!(
            TitleModelPreset::SolLow.config(),
            TitleModelConfig { model: "gpt-5.6-sol", thinking: "low" }
        );
    }

    #[test]
    fn preset_deserialization_rejects_arbitrary_configuration() {
        let defaulted = serde_json::from_str::<TitleModelPreset>(r#""luna-low""#);
        assert_eq!(defaulted.ok(), Some(TitleModelPreset::LunaLow));

        let arbitrary = serde_json::from_str::<TitleModelPreset>(r#""custom-provider/model""#);
        assert!(arbitrary.is_err());
    }
}
