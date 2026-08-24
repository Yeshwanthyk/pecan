//! Pecan's own persisted state in `SQLite`: project allowlist and settled threads.
//!
//! Pecan never mutates pi's data. This module owns a single database file
//! (`~/.pi/agent/pecan/pecan.db`) recording which projects the user added
//! and which session threads are settled. The schema is forward-only;
//! new capabilities (e.g. full-text search) arrive as additive migrations.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::Connection;

use crate::error::{CoreError, Result};

/// Current schema version.
const SCHEMA_VERSION: i64 = 3;

/// A user's curation decision about one project.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProjectPref {
    /// Whether the project appears in the sidebar.
    pub added: bool,
    /// Epoch millis when the project was added or seeded.
    pub added_at_ms: i64,
}

/// SQLite-backed state store.
///
/// A single connection guarded by the caller keeps call sites simple; WAL
/// mode lets future readers proceed concurrently with checkpointing without
/// extra coordination here.
#[derive(Debug)]
pub struct StateStore {
    conn: Connection,
}

impl StateStore {
    /// Opens (creating if needed) the store at `path` and applies migrations.
    ///
    /// # Errors
    /// Returns [`CoreError`] when the file cannot be opened or the schema
    /// cannot be prepared.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|source| CoreError::Io { path: dir.to_owned(), source })?;
        }
        let conn = Connection::open(path)?;
        Self::configure(&conn)?;
        Self::migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Builds an in-memory store, mainly for tests.
    ///
    /// # Errors
    /// See [`StateStore::open`].
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Applies connection pragmas for durability and concurrent access.
    fn configure(conn: &Connection) -> Result<()> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5_000)?;
        Ok(())
    }

    /// Creates the schema when absent and stamps the version. Forward-only:
    /// newer versions refuse to open rather than silently degrade.
    fn migrate(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS project (
                cwd         TEXT PRIMARY KEY,
                added       INTEGER NOT NULL DEFAULT 1,
                added_at_ms INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS settled (
                session_id    TEXT PRIMARY KEY,
                settled_at_ms INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS pinned (
                session_id   TEXT PRIMARY KEY,
                pinned_at_ms INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS session_title (
                session_id    TEXT PRIMARY KEY,
                title         TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS ui_plugin (
                plugin_id     TEXT PRIMARY KEY,
                enabled       INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL
            );
            "#,
        )?;
        let version: String = conn.query_row(
            "SELECT COALESCE((SELECT value FROM meta WHERE key = 'schema_version'), '0')",
            [],
            |row| row.get(0),
        )?;
        match version.parse::<i64>().unwrap_or(0) {
            0 => {
                conn.execute(
                    "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
                    [SCHEMA_VERSION.to_string()],
                )?;
                Ok(())
            }
            1 | 2 => {
                conn.execute(
                    "UPDATE meta SET value = ?1 WHERE key = 'schema_version'",
                    [SCHEMA_VERSION.to_string()],
                )?;
                Ok(())
            }
            v if v == SCHEMA_VERSION => Ok(()),
            v => Err(CoreError::Sql(rusqlite::Error::InvalidParameterName(format!(
                "database schema version {v} is newer than supported {SCHEMA_VERSION}"
            )))),
        }
    }

    /// Imports a legacy `state.json` (pre-SQLite) once, then renames it aside.
    ///
    /// Missing files are normal (`Ok(0)`); unreadable ones warn and stay
    /// untouched so nothing is lost.
    ///
    /// # Errors
    /// Returns [`CoreError`] when reads or writes fail mid-import.
    pub fn import_legacy_json(&self, legacy_path: &Path) -> Result<usize> {
        let Ok(bytes) = std::fs::read(legacy_path) else {
            return Ok(0);
        };
        #[derive(serde::Deserialize)]
        struct LegacyPref {
            #[serde(default = "yes")]
            added: bool,
            added_at: i64,
        }
        #[derive(serde::Deserialize)]
        struct Legacy {
            #[serde(default)]
            projects: HashMap<String, LegacyPref>,
            #[serde(default)]
            settled: HashMap<String, i64>,
        }
        fn yes() -> bool {
            true
        }
        let parsed: Legacy = match serde_json::from_slice(&bytes) {
            Ok(parsed) => parsed,
            Err(error) => {
                tracing::warn!(path = %legacy_path.display(), %error, "unreadable legacy state");
                return Ok(0);
            }
        };
        let project_count = parsed.projects.len();
        let settled_count = parsed.settled.len();

        self.with_tx(|tx| {
            for (cwd, pref) in &parsed.projects {
                tx.execute(
                    "INSERT OR REPLACE INTO project (cwd, added, added_at_ms) VALUES (?1, ?2, ?3)",
                    rusqlite::params![cwd, i64::from(pref.added), pref.added_at],
                )?;
            }
            for (session_id, at) in &parsed.settled {
                tx.execute(
                    "INSERT OR REPLACE INTO settled (session_id, settled_at_ms) VALUES (?1, ?2)",
                    rusqlite::params![session_id, at],
                )?;
            }
            if project_count > 0 {
                tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('seeded', '1')", [])?;
            }
            Ok(())
        })?;

        // Retire the legacy file only after a successful import.
        let aside = legacy_path.with_extension("json.imported");
        if std::fs::rename(legacy_path, &aside).is_err() {
            tracing::warn!(
                path = %legacy_path.display(),
                "could not rename imported legacy state"
            );
        }
        Ok(project_count + settled_count)
    }

    /// Runs `f` inside a transaction, rolling back on error.
    fn with_tx<T>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> rusqlite::Result<T>,
    ) -> Result<T> {
        let tx = self.conn.unchecked_transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// Seeds every discovered project as added on the very first run only,
    /// mirroring the existing tree so nothing disappears unexpectedly.
    /// Subagent working directories must be filtered by the caller.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when writes fail.
    pub fn ensure_seeded(&self, all_cwds: impl IntoIterator<Item = String>) -> Result<()> {
        let now = now_millis();
        self.with_tx(|tx| {
            let already_seeded: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM meta WHERE key = 'seeded')",
                [],
                |row| row.get(0),
            )?;
            if already_seeded {
                return Ok(());
            }
            for cwd in all_cwds {
                tx.execute(
                    "INSERT OR IGNORE INTO project (cwd, added, added_at_ms) VALUES (?1, 1, ?2)",
                    rusqlite::params![cwd, now],
                )?;
            }
            tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('seeded', '1')", [])?;
            Ok(())
        })
    }

    /// Whether first-run seeding has happened.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the metadata query fails.
    pub fn is_seeded(&self) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key = 'seeded')",
            [],
            |row| row.get(0),
        )?)
    }

    /// Marks a project as added to the sidebar.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the write fails.
    pub fn add_project(&self, cwd: &str) -> Result<()> {
        let now = now_millis();
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO project (cwd, added, added_at_ms) VALUES (?1, 1, ?2)
                 ON CONFLICT(cwd) DO UPDATE SET added = 1",
                rusqlite::params![cwd, now],
            )?;
            Ok(())
        })
    }

    /// Removes a project from the sidebar. Sessions remain on disk untouched.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the delete fails.
    pub fn remove_project(&self, cwd: &str) -> Result<()> {
        self.conn.execute("DELETE FROM project WHERE cwd = ?1", [cwd])?;
        Ok(())
    }

    /// All known project preferences keyed by cwd.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the read fails.
    pub fn projects(&self) -> Result<HashMap<String, ProjectPref>> {
        let mut stmt = self.conn.prepare("SELECT cwd, added, added_at_ms FROM project")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                ProjectPref { added: row.get::<_, i64>(1)? != 0, added_at_ms: row.get(2)? },
            ))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (cwd, pref) = row?;
            map.insert(cwd, pref);
        }
        Ok(map)
    }

    /// Whether a project is currently added.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the read fails.
    pub fn is_added(&self, cwd: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM project WHERE cwd = ?1 AND added = 1)",
            [cwd],
            |row| row.get(0),
        )?)
    }

    /// Marks a thread settled.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the write fails.
    pub fn settle(&self, session_id: &str) -> Result<()> {
        let now = now_millis();
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO settled (session_id, settled_at_ms) VALUES (?1, ?2)
                 ON CONFLICT(session_id) DO UPDATE SET settled_at_ms = excluded.settled_at_ms",
                rusqlite::params![session_id, now],
            )?;
            Ok(())
        })
    }

    /// Marks many threads settled in one transaction (bulk curation).
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the write fails.
    pub fn settle_many(&self, session_ids: &[String]) -> Result<()> {
        let now = now_millis();
        self.with_tx(|tx| {
            for id in session_ids {
                tx.execute(
                    "INSERT INTO settled (session_id, settled_at_ms) VALUES (?1, ?2)
                     ON CONFLICT(session_id) DO UPDATE SET settled_at_ms = excluded.settled_at_ms",
                    rusqlite::params![id, now],
                )?;
            }
            Ok(())
        })
    }

    /// Restores a settled thread back to active.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the delete fails.
    pub fn reopen(&self, session_id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM settled WHERE session_id = ?1", [session_id])?;
        Ok(())
    }

    /// All settled session ids valued by their settled-at epoch millis.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the read fails.
    pub fn settled(&self) -> Result<HashMap<String, i64>> {
        let mut stmt = self.conn.prepare("SELECT session_id, settled_at_ms FROM settled")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let mut map = HashMap::new();
        for row in rows {
            let (id, at) = row?;
            map.insert(id, at);
        }
        Ok(map)
    }

    /// Pins or unpins a thread.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the write fails.
    pub fn set_pinned(&self, session_id: &str, pinned: bool) -> Result<()> {
        if pinned {
            let now = now_millis();
            self.with_tx(|tx| {
                tx.execute(
                    "INSERT INTO pinned (session_id, pinned_at_ms) VALUES (?1, ?2)
                     ON CONFLICT(session_id) DO UPDATE SET pinned_at_ms = excluded.pinned_at_ms",
                    rusqlite::params![session_id, now],
                )?;
                Ok(())
            })
        } else {
            self.conn.execute("DELETE FROM pinned WHERE session_id = ?1", [session_id])?;
            Ok(())
        }
    }

    /// All pinned session ids valued by their pinned-at epoch millis.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the read fails.
    pub fn pinned(&self) -> Result<HashMap<String, i64>> {
        let mut stmt = self.conn.prepare("SELECT session_id, pinned_at_ms FROM pinned")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let mut map = HashMap::new();
        for row in rows {
            let (id, at) = row?;
            map.insert(id, at);
        }
        Ok(map)
    }

    /// Stores a generated title outside the Pi transcript.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the write fails.
    pub fn set_title(&self, session_id: &str, title: &str) -> Result<()> {
        let now = now_millis();
        self.conn.execute(
            "INSERT INTO session_title (session_id, title, updated_at_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT(session_id) DO UPDATE SET
                 title = excluded.title,
                 updated_at_ms = excluded.updated_at_ms",
            rusqlite::params![session_id, title, now],
        )?;
        Ok(())
    }

    /// All generated titles keyed by session id.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the read fails.
    pub fn titles(&self) -> Result<HashMap<String, String>> {
        let mut stmt = self.conn.prepare("SELECT session_id, title FROM session_title")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let mut map = HashMap::new();
        for row in rows {
            let (id, title) = row?;
            map.insert(id, title);
        }
        Ok(map)
    }

    /// Enables or disables one allowlisted UI projection.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the write fails.
    pub fn set_ui_plugin_enabled(&self, plugin_id: &str, enabled: bool) -> Result<()> {
        let now = now_millis();
        self.conn.execute(
            "INSERT INTO ui_plugin (plugin_id, enabled, updated_at_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT(plugin_id) DO UPDATE SET
                 enabled = excluded.enabled,
                 updated_at_ms = excluded.updated_at_ms",
            rusqlite::params![plugin_id, i64::from(enabled), now],
        )?;
        Ok(())
    }

    /// Whether an allowlisted UI projection is enabled.
    ///
    /// Missing rows are disabled by default so discovery never opts a plugin in.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the read fails.
    pub fn is_ui_plugin_enabled(&self, plugin_id: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT COALESCE((SELECT enabled FROM ui_plugin WHERE plugin_id = ?1), 0)",
            [plugin_id],
            |row| row.get::<_, i64>(0).map(|value| value != 0),
        )?)
    }

    /// Whether a thread is settled.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the read fails.
    pub fn is_settled(&self, session_id: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM settled WHERE session_id = ?1)",
            [session_id],
            |row| row.get(0),
        )?)
    }
}

/// Wall-clock epoch millis for state timestamps.
fn now_millis() -> i64 {
    jiff::Timestamp::now().as_second().saturating_mul(1_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_only_once_then_respects_removal() {
        let store = StateStore::open_in_memory().expect("open");
        assert!(!store.is_seeded().expect("is_seeded"));
        store.ensure_seeded(["/a".to_owned(), "/b".to_owned()]).expect("seed");
        assert!(store.is_seeded().expect("is_seeded"));
        assert!(store.is_added("/a").expect("added"));
        assert!(store.is_added("/b").expect("added"));

        store.remove_project("/a").expect("remove");
        // Re-seeding after first run must not resurrect removed projects,
        // and later discoveries stay unadded.
        store.ensure_seeded(["/a".to_owned(), "/c".to_owned()]).expect("re-seed");
        assert!(!store.is_added("/a").expect("removed stays removed"));
        assert!(!store.is_added("/c").expect("new discovery unadded"));
        assert!(store.is_added("/b").expect("kept"));
    }

    #[test]
    fn settles_and_reopens_threads() {
        let store = StateStore::open_in_memory().expect("open");
        store.settle("sid-1").expect("settle");
        assert!(store.is_settled("sid-1").expect("settled"));
        assert_eq!(store.settled().expect("map").len(), 1);
        store.reopen("sid-1").expect("reopen");
        assert!(!store.is_settled("sid-1").expect("unsettled"));
    }

    #[test]
    fn add_remove_projects_round_trip() {
        let store = StateStore::open_in_memory().expect("open");
        store.add_project("/proj").expect("add");
        assert!(store.is_added("/proj").expect("added"));
        store.remove_project("/proj").expect("remove");
        assert!(!store.is_added("/proj").expect("gone"));
        assert_eq!(store.projects().expect("projects").len(), 0);
    }

    #[test]
    fn imports_legacy_json_once_and_renames_it_aside() {
        let dir = std::env::temp_dir().join(format!("pecan-store-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let legacy = dir.join("state.json");
        std::fs::write(
            &legacy,
            r#"{"version":1,"projects":{"/p":{"added":true,"added_at":123}},
                "settled":{"s9":456},"seeded":true}"#,
        )
        .expect("write legacy");

        let store = StateStore::open(&dir.join("pecan.db")).expect("open");
        let imported = store.import_legacy_json(&legacy).expect("import");
        assert_eq!(imported, 2);
        assert!(store.is_added("/p").expect("project imported"));
        assert!(store.is_settled("s9").expect("settle imported"));
        assert!(store.is_seeded().expect("seeded flag imported"));
        assert!(!legacy.exists(), "legacy renamed aside");
        assert!(dir.join("state.json.imported").exists());

        // Second call is a no-op (file gone).
        assert_eq!(store.import_legacy_json(&legacy).expect("second"), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_newer_schema_versions() {
        let dir = std::env::temp_dir().join(format!("pecan-store-future-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let db = dir.join("pecan.db");
        {
            let conn = Connection::open(&db).expect("raw open");
            conn.execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO meta VALUES ('schema_version', '999');",
            )
            .expect("stamp future version");
        }
        assert!(StateStore::open(&db).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generated_titles_round_trip_without_touching_session_data() {
        let store = StateStore::open_in_memory().expect("open");
        store.set_title("sid-1", "Review session recovery").expect("set title");
        store.set_title("sid-1", "Fix session recovery").expect("replace title");
        assert_eq!(
            store.titles().expect("titles").get("sid-1").map(String::as_str),
            Some("Fix session recovery")
        );
    }

    #[test]
    fn ui_plugins_are_opt_in_and_persist_disablement() {
        let store = StateStore::open_in_memory().expect("open");
        assert!(!store.is_ui_plugin_enabled("pi-tasks").expect("default"));
        store.set_ui_plugin_enabled("pi-tasks", true).expect("enable");
        assert!(store.is_ui_plugin_enabled("pi-tasks").expect("enabled"));
        store.set_ui_plugin_enabled("pi-tasks", false).expect("disable");
        assert!(!store.is_ui_plugin_enabled("pi-tasks").expect("disabled"));
    }

    #[test]
    fn migrates_version_one_database_for_titles() {
        let dir = std::env::temp_dir().join(format!("pecan-store-v1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let db = dir.join("pecan.db");
        {
            let conn = Connection::open(&db).expect("raw open");
            conn.execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO meta VALUES ('schema_version', '1');",
            )
            .expect("stamp v1");
        }
        let store = StateStore::open(&db).expect("migrate");
        store.set_title("sid-2", "Mapped title").expect("write title");
        assert_eq!(store.titles().expect("titles").len(), 1);
        let version: String = store
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |row| row.get(0))
            .expect("version");
        assert_eq!(version, "3");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
