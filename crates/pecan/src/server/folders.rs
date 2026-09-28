//! Folder picker for linking projects: read-only directory completion under a
//! typed path plus the working directories Pi sessions already used.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use super::snapshot::SessionRow;
use pecan_core::session::SessionKind;

/// Most directory entries returned for one completion.
const MAX_ENTRIES: usize = 60;
/// Most known session folders suggested.
const MAX_SUGGESTIONS: usize = 8;
/// Upper bound on one directory read (network mounts can hang).
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// One folder the picker can offer.
#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Folder {
    pub(crate) path: String,
    pub(crate) name: String,
}

/// A folder Pi sessions have run in, with how much happened there.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KnownFolder {
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) sessions: usize,
    pub(crate) last_activity: jiff::Timestamp,
}

/// Expands a leading `~` against `home`; other paths pass through.
pub(crate) fn expand_home(input: &str, home: Option<&Path>) -> String {
    let Some(home) = home else { return input.to_owned() };
    let home = home.to_string_lossy();
    if input == "~" {
        return home.into_owned();
    }
    match input.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", home.trim_end_matches('/')),
        None => input.to_owned(),
    }
}

/// The user's home directory, when set.
pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|home| !home.is_empty()).map(PathBuf::from)
}

/// Splits an absolute query into the directory to list and a name prefix:
/// `/a/b/` lists `/a/b` fully, `/a/bc` lists `/a` filtered by `bc`.
fn split_query(query: &str) -> Option<(&str, &str)> {
    if !query.starts_with('/') {
        return None;
    }
    let (dir, prefix) = query.rsplit_once('/')?;
    Some((if dir.is_empty() { "/" } else { dir }, prefix))
}

/// Child directories matching the typed path, sorted, hidden ones only when
/// the prefix asks for them. Unreadable directories yield no entries.
pub(crate) async fn complete(query: String) -> Vec<Folder> {
    let task = tokio::task::spawn_blocking(move || list_children(&query));
    match tokio::time::timeout(READ_TIMEOUT, task).await {
        Ok(Ok(entries)) => entries,
        Ok(Err(error)) => {
            tracing::warn!(%error, "folder completion task failed");
            Vec::new()
        }
        Err(_) => Vec::new(),
    }
}

fn list_children(query: &str) -> Vec<Folder> {
    let Some((dir, prefix)) = split_query(query) else { return Vec::new() };
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let needle = prefix.to_lowercase();
    let mut entries: Vec<Folder> = read
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let hidden = name.starts_with('.') && !prefix.starts_with('.');
            (!hidden && name.to_lowercase().starts_with(&needle)).then(|| Folder {
                path: Path::new(dir).join(&name).to_string_lossy().into_owned(),
                name,
            })
        })
        .collect();
    entries.sort_by_cached_key(|folder| folder.name.to_lowercase());
    entries.truncate(MAX_ENTRIES);
    entries
}

/// Folders Pi sessions ran in that are not linked yet, most recent first,
/// optionally filtered by a case-insensitive substring of the path.
pub(crate) fn known_folders<'a>(
    rows: impl IntoIterator<Item = &'a SessionRow>,
    linked: &std::collections::HashSet<&str>,
    filter: &str,
) -> Vec<KnownFolder> {
    let needle = filter.to_lowercase();
    let mut by_cwd: std::collections::HashMap<&str, KnownFolder> = std::collections::HashMap::new();
    for row in rows {
        let summary = &row.summary;
        if summary.kind != SessionKind::Normal
            || linked.contains(summary.cwd.as_str())
            || !summary.cwd.to_lowercase().contains(&needle)
        {
            continue;
        }
        let known = by_cwd.entry(summary.cwd.as_str()).or_insert_with(|| KnownFolder {
            path: summary.cwd.clone(),
            name: display_name(&summary.cwd),
            sessions: 0,
            last_activity: summary.last_activity,
        });
        known.sessions += 1;
        known.last_activity = known.last_activity.max(summary.last_activity);
    }
    let mut known: Vec<KnownFolder> = by_cwd.into_values().collect();
    known.sort_by_key(|folder| std::cmp::Reverse(folder.last_activity));
    known.truncate(MAX_SUGGESTIONS);
    known
}

fn display_name(cwd: &str) -> String {
    Path::new(cwd)
        .file_name()
        .map_or_else(|| cwd.to_owned(), |name| name.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_expands_only_at_the_start() {
        let home = Path::new("/home/me");
        assert_eq!(expand_home("~", Some(home)), "/home/me");
        assert_eq!(expand_home("~/code", Some(home)), "/home/me/code");
        assert_eq!(expand_home("/x/~/y", Some(home)), "/x/~/y");
        assert_eq!(expand_home("~other", Some(home)), "~other");
        assert_eq!(expand_home("~/code", None), "~/code");
    }

    #[test]
    fn query_splits_into_directory_and_prefix() {
        assert_eq!(split_query("/"), Some(("/", "")));
        assert_eq!(split_query("/Us"), Some(("/", "Us")));
        assert_eq!(split_query("/a/b/"), Some(("/a/b", "")));
        assert_eq!(split_query("/a/bc"), Some(("/a", "bc")));
        assert_eq!(split_query("relative"), None);
    }

    #[test]
    fn completion_lists_matching_visible_directories() {
        let root = std::env::temp_dir().join(format!("pecan-folders-{}", std::process::id()));
        for dir in ["Alpha", "apple", "beta", ".hidden"] {
            std::fs::create_dir_all(root.join(dir)).expect("create fixture dir");
        }
        std::fs::write(root.join("afile"), "").expect("create fixture file");
        let base = root.to_string_lossy().into_owned();

        let names = |query: &str| -> Vec<String> {
            list_children(query).into_iter().map(|folder| folder.name).collect()
        };
        assert_eq!(names(&format!("{base}/a")), ["Alpha", "apple"], "dirs only, case-insensitive");
        assert_eq!(names(&format!("{base}/")), ["Alpha", "apple", "beta"], "hidden skipped");
        assert_eq!(names(&format!("{base}/.h")), [".hidden"], "dot prefix shows hidden");
        assert!(names(&format!("{base}/missing/")).is_empty(), "unreadable dir is empty");
        assert!(names("relative/path").is_empty());

        std::fs::remove_dir_all(&root).unwrap_or_default();
    }
}
