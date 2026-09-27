//! Session directory scanning with an mtime-keyed cache.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use jiff::Timestamp;

use crate::error::{CoreError, Result};
use crate::session::{self, SessionKind, SessionSummary};

/// One cached session summary keyed by file identity.
#[derive(Debug, Clone)]
struct CachedSession {
    modified: SystemTime,
    bytes: u64,
    summary: SessionSummary,
}

/// Incremental scanner over pi's sessions directory.
///
/// A full refresh walks the tree once comparing `(mtime, size)`; unchanged
/// files reuse their cached summary, so steady-state refreshes only parse
/// what actually changed.
#[derive(Debug, Default)]
pub struct ScanCache {
    by_path: HashMap<PathBuf, CachedSession>,
}

impl ScanCache {
    /// Creates an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Refreshes the index from the sessions directory and returns all
    /// summaries sorted by open time, newest first.
    ///
    /// Files whose header line is not a valid session record are skipped;
    /// unreadable directories produce an error only when they are the root.
    ///
    /// # Errors
    /// Returns [`CoreError::Io`] when the sessions root cannot be read.
    pub fn refresh(&mut self, sessions_dir: &Path) -> Result<Vec<SessionSummary>> {
        let mut seen: HashMap<PathBuf, (SystemTime, u64)> = HashMap::new();
        for project_dir in list_subdirs(sessions_dir)? {
            let Ok(entries) = std::fs::read_dir(&project_dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                let Ok(meta) = entry.metadata() else { continue };
                if !meta.is_file() {
                    continue;
                }
                let Ok(modified) = meta.modified() else { continue };
                seen.insert(path, (modified, meta.len()));
            }
        }

        self.by_path.retain(|path, _| seen.contains_key(path));

        let mut out: Vec<SessionSummary> = Vec::with_capacity(seen.len());
        for (path, (modified, bytes)) in seen {
            if let Some(cached) = self.by_path.get(&path)
                && cached.modified == modified
                && cached.bytes == bytes
            {
                out.push(cached.summary.clone());
                continue;
            }
            match build_summary(&path, modified, bytes) {
                Some(summary) => {
                    self.by_path
                        .insert(path, CachedSession { modified, bytes, summary: summary.clone() });
                    out.push(summary);
                }
                None => {
                    tracing::debug!(path = %path.display(), "skipping unreadable session file");
                }
            }
        }

        out.sort_by_key(|session| std::cmp::Reverse(session.opened_at));
        Ok(out)
    }
}

/// Lists immediate subdirectories of `dir`, sorted for deterministic scans.
fn list_subdirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries =
        std::fs::read_dir(dir).map_err(|source| CoreError::Io { path: dir.to_owned(), source })?;
    let mut dirs: Vec<PathBuf> =
        entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    Ok(dirs)
}

/// Builds a summary from one session file, or `None` when it has no valid header.
fn build_summary(path: &Path, modified: SystemTime, bytes: u64) -> Option<SessionSummary> {
    let header = session::read_header(path).ok()??;
    let preview = session::extract_preview(path);
    let last_activity = Timestamp::try_from(modified).unwrap_or(Timestamp::UNIX_EPOCH);
    let agent_name = session::subagent_name(path);
    let kind = if agent_name.is_some() {
        SessionKind::Subagent
    } else {
        session::kind_for(&header.cwd, path)
    };
    Some(SessionSummary {
        id: header.id,
        path: path.to_owned(),
        cwd: header.cwd,
        opened_at: header.opened_at,
        last_activity,
        bytes,
        provider: header.provider,
        model: header.model_id,
        preview,
        title: None,
        kind,
        agent_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER_A: &str =
        r#"{"type":"session","id":"sid-a","timestamp":"2026-01-02T10:00:00Z","cwd":"/tmp/pa"}"#;
    const HEADER_B: &str =
        r#"{"type":"session","id":"sid-b","timestamp":"2026-01-03T10:00:00Z","cwd":"/tmp/pb"}"#;

    struct TempTree(PathBuf);
    impl TempTree {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("pecan-scan-{tag}-{}", std::process::id()));
            std::fs::remove_dir_all(&dir).unwrap_or_default();
            std::fs::create_dir_all(&dir).expect("mkdir root");
            Self(dir)
        }

        fn project(&self, name: &str) -> PathBuf {
            let p = self.0.join(name);
            std::fs::create_dir_all(&p).expect("mkdir project");
            p
        }

        fn write_session(&self, dir: &Path, name: &str, header: &str) -> PathBuf {
            let p = dir.join(name);
            std::fs::write(&p, format!("{header}\n")).expect("write session");
            p
        }
    }
    impl Drop for TempTree {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap_or_default();
        }
    }

    #[test]
    fn sorts_newest_first_and_caches_unchanged_files() {
        let tree = TempTree::new("order");
        let pa = tree.project("--tmp-pa--");
        let pb = tree.project("--tmp-pb--");
        tree.write_session(&pa, "older.jsonl", HEADER_A);
        tree.write_session(&pb, "newer.jsonl", HEADER_B);

        let mut cache = ScanCache::new();
        let first = cache.refresh(&tree.0).expect("refresh");
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].id, "sid-b");
        assert_eq!(first[1].id, "sid-a");

        // Second refresh must reuse the cache and stay stable.
        let second = cache.refresh(&tree.0).expect("refresh 2");
        assert_eq!(second.len(), 2);
        assert_eq!(second[0].id, "sid-b");
    }

    #[test]
    fn drops_sessions_removed_from_disk() {
        let tree = TempTree::new("stale");
        let pa = tree.project("--tmp-pa--");
        let file = tree.write_session(&pa, "s.jsonl", HEADER_A);

        let mut cache = ScanCache::new();
        assert_eq!(cache.refresh(&tree.0).expect("refresh").len(), 1);
        std::fs::remove_file(&file).expect("remove");
        assert_eq!(cache.refresh(&tree.0).expect("refresh").len(), 0);
    }

    #[test]
    fn skips_invalid_headers_without_failing_scan() {
        let tree = TempTree::new("invalid");
        let pa = tree.project("--tmp-pa--");
        tree.write_session(&pa, "bad.jsonl", "{\"type\":\"message\"}");
        tree.write_session(&pa, "empty.jsonl", "");
        tree.write_session(&pa, "good.jsonl", HEADER_A);
        let mut cache = ScanCache::new();
        let out = cache.refresh(&tree.0).expect("refresh");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "sid-a");
    }
}
