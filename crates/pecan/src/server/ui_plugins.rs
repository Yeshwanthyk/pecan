//! Detection and user enablement for Pecan's allowlisted UI projections.

use pecan_core::PiPaths;
use pecan_core::store::StateStore;

/// Stable identifier for the first typed UI projection.
pub(crate) const PI_TASKS_ID: &str = "pi-tasks";

const MAX_SETTINGS_BYTES: u64 = 256 * 1024;
const PI_TASKS_SOURCES: [&str; 3] =
    ["git:github.com/Yeshwanthyk/pi-tasks", "npm:pi-tasks", "npm:@tintinweb/pi-tasks"];

/// One UI projection Pecan knows how to render safely.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UiPluginDescriptor {
    pub(crate) id: &'static str,
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) detected: bool,
    pub(crate) source_enabled: bool,
    pub(crate) enabled: bool,
}

/// Returns Pecan's bounded, allowlisted UI-plugin catalog.
pub(crate) fn catalog(
    paths: &PiPaths,
    store: &StateStore,
) -> pecan_core::Result<Vec<UiPluginDescriptor>> {
    let (detected, source_enabled) = detect_pi_tasks(paths);
    let enabled = detected && source_enabled && store.is_ui_plugin_enabled(PI_TASKS_ID)?;
    Ok(vec![UiPluginDescriptor {
        id: PI_TASKS_ID,
        name: "Pi Tasks",
        description: "Show the typed task list for each thread.",
        detected,
        source_enabled,
        enabled,
    }])
}

/// Whether the typed Pi Tasks projection may read its persisted artifacts.
pub(crate) fn pi_tasks_enabled(paths: &PiPaths, store: &StateStore) -> pecan_core::Result<bool> {
    let (detected, source_enabled) = detect_pi_tasks(paths);
    Ok(detected && source_enabled && store.is_ui_plugin_enabled(PI_TASKS_ID)?)
}

fn detect_pi_tasks(paths: &PiPaths) -> (bool, bool) {
    let path = paths.settings_file();
    let Ok(metadata) = std::fs::metadata(&path) else {
        return (false, false);
    };
    if metadata.len() > MAX_SETTINGS_BYTES {
        return (false, false);
    }
    let Ok(bytes) = std::fs::read(path) else {
        return (false, false);
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return (false, false);
    };
    let Some(packages) = value.get("packages").and_then(serde_json::Value::as_array) else {
        return (false, false);
    };
    for package in packages {
        let (source, enabled) = match package {
            serde_json::Value::String(source) => (Some(source.as_str()), true),
            serde_json::Value::Object(object) => {
                let source = object.get("source").and_then(serde_json::Value::as_str);
                let autoload =
                    object.get("autoload").and_then(serde_json::Value::as_bool).unwrap_or(true);
                let extensions_enabled = object
                    .get("extensions")
                    .and_then(serde_json::Value::as_array)
                    .is_none_or(|extensions| !extensions.is_empty());
                (source, autoload && extensions_enabled)
            }
            _ => (None, false),
        };
        if source.is_some_and(|candidate| PI_TASKS_SOURCES.contains(&candidate)) {
            return (true, enabled);
        }
    }
    (false, false)
}

#[cfg(test)]
mod tests {
    use super::detect_pi_tasks;
    use pecan_core::PiPaths;

    fn detect(settings: &str, tag: &str) -> (bool, bool) {
        let root =
            std::env::temp_dir().join(format!("pecan-ui-plugin-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&root).unwrap_or_default();
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(root.join("settings.json"), settings).expect("settings");
        let result = detect_pi_tasks(&PiPaths::from_agent_dir(root.clone()));
        std::fs::remove_dir_all(root).unwrap_or_default();
        result
    }

    #[test]
    fn detects_only_exact_allowlisted_sources() {
        assert_eq!(
            detect(r#"{"packages":["git:github.com/Yeshwanthyk/pi-tasks"]}"#, "exact"),
            (true, true)
        );
        assert_eq!(
            detect(r#"{"packages":["git:github.com/example/pi-tasks-fake"]}"#, "fake"),
            (false, false)
        );
    }

    #[test]
    fn respects_explicit_extension_disablement() {
        assert_eq!(
            detect(
                r#"{"packages":[{"source":"git:github.com/Yeshwanthyk/pi-tasks","extensions":[]}]}"#,
                "disabled"
            ),
            (true, false)
        );
    }
}
