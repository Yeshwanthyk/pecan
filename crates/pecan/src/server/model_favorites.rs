//! Favorite models for the composer picker, read live from pi's own
//! `enabledModels` setting (the same list `pi` cycles with Ctrl+P), so one
//! edit to `settings.json` updates both pi and Pecan.

use pecan_core::PiPaths;

/// Settings larger than this are ignored rather than parsed.
const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;
/// Maximum favorites returned to the client.
const MAX_FAVORITES: usize = 32;
/// Maximum characters kept per model pattern.
const MAX_PATTERN_CHARS: usize = 200;

/// Returns pi's `enabledModels` patterns in order; empty when unset or unreadable.
pub(crate) fn favorite_models(paths: &PiPaths) -> Vec<String> {
    let path = paths.settings_file();
    let readable = std::fs::metadata(&path).is_ok_and(|meta| meta.len() <= MAX_SETTINGS_BYTES);
    if !readable {
        return Vec::new();
    }
    std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .map(|settings| parse_enabled_models(&settings))
        .unwrap_or_default()
}

fn parse_enabled_models(settings: &serde_json::Value) -> Vec<String> {
    settings
        .get("enabledModels")
        .and_then(serde_json::Value::as_array)
        .map(|patterns| {
            patterns
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|pattern| {
                    !pattern.is_empty() && pattern.chars().count() <= MAX_PATTERN_CHARS
                })
                .take(MAX_FAVORITES)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_enabled_models_in_order() {
        let settings = json!({"enabledModels": [
            "openai-codex/gpt-6-astra", " vibeproxy-anthropic/claude-opus-5-5 ", "", 7
        ]});
        assert_eq!(
            parse_enabled_models(&settings),
            vec!["openai-codex/gpt-6-astra", "vibeproxy-anthropic/claude-opus-5-5"]
        );
    }

    #[test]
    fn missing_or_malformed_settings_yield_no_favorites() {
        assert!(parse_enabled_models(&json!({})).is_empty());
        assert!(parse_enabled_models(&json!({"enabledModels": "gpt"})).is_empty());
        let dir = std::env::temp_dir().join(format!("pecan-favs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap_or_default();
        std::fs::write(dir.join("settings.json"), "{not json").unwrap_or_default();
        assert!(favorite_models(&PiPaths::from_agent_dir(dir.clone())).is_empty());
        std::fs::remove_dir_all(&dir).unwrap_or_default();
    }
}
