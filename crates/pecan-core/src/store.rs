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
const SCHEMA_VERSION: i64 = 5;

/// A user's curation decision about one project.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProjectPref {
    /// Whether the project appears in the sidebar.
    pub added: bool,
    /// Epoch millis when the project was added or seeded.
    pub added_at_ms: i64,
}

/// A browser paired with this server. The bearer token itself is never
/// stored; only its SHA-256 digest is, so a copied database cannot sign in.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    /// Stable public id used to list and revoke the device.
    pub id: String,
    /// Human label (browser and platform) captured at pairing time.
    pub name: String,
    /// Epoch millis when the device was paired.
    pub created_at_ms: i64,
    /// Epoch millis of the last authenticated request (coarse, see
    /// [`StateStore::touch_device`]).
    pub last_seen_at_ms: i64,
}

/// A device's web push subscription. Pushes carry no payload, so only the
/// push service endpoint is kept, never the browser's encryption keys.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushSubscription {
    /// The subscribed device.
    pub device_id: String,
    /// The device's label, for listings.
    pub device_name: String,
    /// Push service URL the server POSTs to.
    pub endpoint: String,
    /// Epoch millis when the endpoint was last (re)registered.
    pub updated_at_ms: i64,
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
    #[cfg(test)]
    pub(crate) fn open_in_memory() -> Result<Self> {
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
            CREATE TABLE IF NOT EXISTS device (
                id              TEXT PRIMARY KEY,
                name            TEXT NOT NULL,
                token_sha256    TEXT NOT NULL UNIQUE,
                created_at_ms   INTEGER NOT NULL,
                last_seen_at_ms INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS push_subscription (
                device_id     TEXT PRIMARY KEY,
                endpoint      TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
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
            1..=4 => {
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

    /// Marks a project as added to the sidebar. When it was not already
    /// added, the `idle` sessions move to Done in the same transaction so the
    /// project opens with only recent work; pinned and already-settled
    /// sessions are left as they are. Returns whether the project was newly
    /// added.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the write fails.
    pub fn add_project<'a>(
        &self,
        cwd: &str,
        idle: impl IntoIterator<Item = &'a str>,
    ) -> Result<bool> {
        let now = now_millis();
        self.with_tx(|tx| {
            let already: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM project WHERE cwd = ?1 AND added = 1)",
                [cwd],
                |row| row.get(0),
            )?;
            tx.execute(
                "INSERT INTO project (cwd, added, added_at_ms) VALUES (?1, 1, ?2)
                 ON CONFLICT(cwd) DO UPDATE SET added = 1",
                rusqlite::params![cwd, now],
            )?;
            if already {
                return Ok(false);
            }
            let mut archive = tx.prepare(
                "INSERT OR IGNORE INTO settled (session_id, settled_at_ms)
                 SELECT ?1, ?2 WHERE NOT EXISTS (SELECT 1 FROM pinned WHERE session_id = ?1)",
            )?;
            for session_id in idle {
                archive.execute(rusqlite::params![session_id, now])?;
            }
            Ok(true)
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

    /// Records a newly paired device.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the write fails (including a
    /// duplicate id or token digest).
    pub fn add_device(&self, id: &str, name: &str, token_sha256: &str) -> Result<Device> {
        let now = now_millis();
        self.conn.execute(
            "INSERT INTO device (id, name, token_sha256, created_at_ms, last_seen_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            rusqlite::params![id, name, token_sha256, now],
        )?;
        Ok(Device {
            id: id.to_owned(),
            name: name.to_owned(),
            created_at_ms: now,
            last_seen_at_ms: now,
        })
    }

    /// The device holding the token with this digest, if still paired.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the read fails.
    pub fn device_by_token(&self, token_sha256: &str) -> Result<Option<Device>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, created_at_ms, last_seen_at_ms FROM device WHERE token_sha256 = ?1",
        )?;
        let mut rows = stmt.query_map([token_sha256], device_from_row)?;
        Ok(rows.next().transpose()?)
    }

    /// Marks a device as seen now.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the write fails.
    pub fn touch_device(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE device SET last_seen_at_ms = ?2 WHERE id = ?1",
            rusqlite::params![id, now_millis()],
        )?;
        Ok(())
    }

    /// Every paired device, most recently paired first.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the read fails.
    pub fn devices(&self) -> Result<Vec<Device>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, created_at_ms, last_seen_at_ms FROM device
             ORDER BY created_at_ms DESC, id",
        )?;
        let rows = stmt.query_map([], device_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Forgets a device; its token stops working immediately. Returns whether
    /// a device with that id existed.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the write fails.
    pub fn revoke_device(&self, id: &str) -> Result<bool> {
        self.with_tx(|tx| {
            tx.execute("DELETE FROM push_subscription WHERE device_id = ?1", [id])?;
            Ok(tx.execute("DELETE FROM device WHERE id = ?1", [id])? > 0)
        })
    }

    /// Stores (or replaces) a paired device's push endpoint.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the write fails.
    pub fn set_push_subscription(&self, device_id: &str, endpoint: &str) -> Result<()> {
        let now = now_millis();
        self.conn.execute(
            "INSERT INTO push_subscription (device_id, endpoint, created_at_ms, updated_at_ms)
             SELECT id, ?2, ?3, ?3 FROM device WHERE id = ?1
             ON CONFLICT(device_id) DO UPDATE SET endpoint = ?2, updated_at_ms = ?3",
            rusqlite::params![device_id, endpoint, now],
        )?;
        Ok(())
    }

    /// Drops a device's push endpoint. Returns whether one existed.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the write fails.
    pub fn remove_push_subscription(&self, device_id: &str) -> Result<bool> {
        Ok(self.conn.execute("DELETE FROM push_subscription WHERE device_id = ?1", [device_id])?
            > 0)
    }

    /// Every push subscription of a still-paired device.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the read fails.
    pub fn push_subscriptions(&self) -> Result<Vec<PushSubscription>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT p.device_id, d.name, p.endpoint, p.updated_at_ms
             FROM push_subscription p JOIN device d ON d.id = p.device_id
             ORDER BY p.updated_at_ms DESC, p.device_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(PushSubscription {
                device_id: row.get(0)?,
                device_name: row.get(1)?,
                endpoint: row.get(2)?,
                updated_at_ms: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Whether a thread is settled.
    ///
    /// # Errors
    /// Returns [`CoreError::Sql`] when the read fails.
    #[cfg(test)]
    pub(crate) fn is_settled(&self, session_id: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM settled WHERE session_id = ?1)",
            [session_id],
            |row| row.get(0),
        )?)
    }
}

fn device_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Device> {
    Ok(Device {
        id: row.get(0)?,
        name: row.get(1)?,
        created_at_ms: row.get(2)?,
        last_seen_at_ms: row.get(3)?,
    })
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
        assert!(store.add_project("/proj", []).expect("add"));
        assert!(store.is_added("/proj").expect("added"));
        store.remove_project("/proj").expect("remove");
        assert!(!store.is_added("/proj").expect("gone"));
        assert_eq!(store.projects().expect("projects").len(), 0);
    }

    #[test]
    fn adding_a_project_archives_idle_sessions_once() {
        let store = StateStore::open_in_memory().expect("open");
        store.set_pinned("pinned-old", true).expect("pin");
        assert!(store.add_project("/proj", ["old-1", "pinned-old"]).expect("add"));
        assert!(store.is_settled("old-1").expect("read"), "idle session moves to Done");
        assert!(!store.is_settled("pinned-old").expect("read"), "pinned stays out of Done");

        store.reopen("old-1").expect("reopen");
        assert!(!store.add_project("/proj", ["old-1"]).expect("re-add"), "already added");
        assert!(!store.is_settled("old-1").expect("read"), "re-adding keeps user reopens");
    }

    #[test]
    fn push_subscriptions_follow_their_device() {
        let store = StateStore::open_in_memory().expect("store");
        store.set_push_subscription("ghost", "https://push.example/x").expect("unknown device");
        assert!(store.push_subscriptions().expect("list").is_empty(), "needs a paired device");
        store.add_device("dev-1", "Safari on iPhone", "digest-1").expect("add");
        store.set_push_subscription("dev-1", "https://push.example/a").expect("subscribe");
        store.set_push_subscription("dev-1", "https://push.example/b").expect("resubscribe");
        let subs = store.push_subscriptions().expect("list");
        assert_eq!(subs.len(), 1, "one endpoint per device");
        assert_eq!(subs.first().map(|sub| sub.endpoint.as_str()), Some("https://push.example/b"));
        assert_eq!(subs.first().map(|sub| sub.device_name.as_str()), Some("Safari on iPhone"));
        assert!(store.revoke_device("dev-1").expect("revoke"));
        assert!(store.push_subscriptions().expect("list").is_empty(), "revoke unsubscribes");
        assert!(!store.remove_push_subscription("dev-1").expect("remove"), "already gone");
    }

    #[test]
    fn pairs_looks_up_and_revokes_devices() {
        let store = StateStore::open_in_memory().expect("open");
        let device = store.add_device("dev-1", "Safari on iPhone", "digest-1").expect("add");
        assert_eq!(store.device_by_token("digest-1").expect("lookup"), Some(device));
        assert_eq!(store.device_by_token("digest-2").expect("lookup"), None, "unknown token");
        store.touch_device("dev-1").expect("touch");
        assert_eq!(store.devices().expect("list").len(), 1);
        assert!(store.revoke_device("dev-1").expect("revoke"));
        assert!(!store.revoke_device("dev-1").expect("second revoke"), "already gone");
        assert_eq!(store.device_by_token("digest-1").expect("lookup"), None, "revoked token");
    }

    #[test]
    fn rejects_a_reused_token_digest() {
        let store = StateStore::open_in_memory().expect("open");
        store.add_device("dev-1", "one", "same").expect("add");
        assert!(store.add_device("dev-2", "two", "same").is_err(), "digest is unique");
    }

    #[test]
    fn refuses_newer_schema_versions() {
        let dir = std::env::temp_dir().join(format!("pecan-store-future-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).unwrap_or_default();
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
        std::fs::remove_dir_all(&dir).unwrap_or_default();
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
    fn migrates_version_one_database_for_titles() {
        let dir = std::env::temp_dir().join(format!("pecan-store-v1-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).unwrap_or_default();
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
        assert_eq!(version, SCHEMA_VERSION.to_string(), "stamped current");
        std::fs::remove_dir_all(&dir).unwrap_or_default();
    }
}
