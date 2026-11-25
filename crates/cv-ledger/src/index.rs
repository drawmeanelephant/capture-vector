//! SQLite projection of the journal. Disposable: delete it and
//! [`LedgerIndex::open`] rebuilds it from JSONL. Never written to except
//! by replaying journal records.
//!
//! Ownership: a `LedgerIndex` is a point-in-time snapshot handle. A
//! rebuild installs a brand-new file by rename, so a handle opened
//! before the rebuild keeps reading the pre-replacement inode (or a
//! deleted file on Windows). Owners must be dropped before a rebuild
//! and re-opened after it; the composition root enforces this by
//! rebuilding only while reopening a workspace, when every store owner
//! of the previous session has been dropped.

use crate::Result;
use crate::journal::{Entry, Journal, Record};
use crate::nouns::kind;
use crate::runs::runs;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};

/// Where the index lived before it moved to OS app-data. Kept only so
/// `open` can delete the stale file; the index is disposable.
pub const INDEX_REL: &str = ".cv/ledger/index.sqlite";

pub struct LedgerIndex {
    conn: Connection,
    path: PathBuf,
}

/// Bump when the table layout or fill rules change: an index written
/// under another version is rebuilt, never trusted.
const INDEX_SCHEMA_VERSION: u64 = 1;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS nouns (
  kind TEXT NOT NULL, id TEXT NOT NULL, run TEXT NOT NULL, target TEXT NOT NULL,
  json TEXT NOT NULL, PRIMARY KEY (kind, id));
CREATE TABLE IF NOT EXISTS runs (
  run TEXT PRIMARY KEY, target TEXT NOT NULL, phase TEXT NOT NULL);
";

impl LedgerIndex {
    /// OS app-data path for this project's rebuildable index (keyed by
    /// the canonical root, never inside `.cv/`).
    pub fn path_for(project_root: &Path) -> std::io::Result<PathBuf> {
        Ok(cv_store::paths::project_index_dir(project_root, "ledger")?.join("index.sqlite"))
    }

    /// Open the index and bring it in line with the journal. A stale,
    /// foreign, or corrupt index file is discarded and rebuilt.
    pub fn open(project_root: &Path, journal: &Journal) -> Result<Self> {
        let index_dir = cv_store::paths::create_project_index_dir(project_root, "ledger")?;
        let path = index_dir.join("index.sqlite");
        // Drop a pre-app-data index (and its journal artifacts); the
        // JSONL journal is the truth and the index rebuilds from it.
        for suffix in ["", "-journal", "-wal", "-shm"] {
            let _ = std::fs::remove_file(project_root.join(format!("{INDEX_REL}{suffix}")));
        }
        let records = journal.records();
        // Stale, foreign and corrupt indexes all rebuild through the
        // same complete-replacement protocol as a forced rebuild:
        // replaying into the existing file keeps a foreign shape (wrong
        // columns, debris tables) and any stale sidecar artifacts.
        if Self::needs_rebuild(&path, records) {
            Self::replace_with_rebuild(&path, records)?;
        }
        Self::connect(&path)
    }

    /// Column names of `table` in order; empty when the table is absent.
    fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
        let mut st = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let cols = st
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(cols)
    }

    /// Whether the opened database is exactly the owned projection
    /// shape: the meta, nouns and runs tables with their expected
    /// columns and nothing else. Foreign debris or wrong columns mean
    /// the file cannot be trusted however fresh its metadata counters
    /// look.
    fn owned_schema_valid(conn: &Connection) -> Result<bool> {
        let mut st = conn.prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        )?;
        let mut tables: Vec<String> = st
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        tables.sort();
        if tables != ["meta", "nouns", "runs"] {
            return Ok(false);
        }
        Ok(Self::table_columns(conn, "meta")? == ["k", "v"]
            && Self::table_columns(conn, "nouns")? == ["kind", "id", "run", "target", "json"]
            && Self::table_columns(conn, "runs")? == ["run", "target", "phase"])
    }

    /// Whether the index at `path` cannot stand in for a replay of
    /// `records`: missing, unreadable, foreign, wrong-shaped, wrong
    /// schema version or a stale journal length. The probe connection
    /// is always closed before returning, so the caller may replace the
    /// file immediately afterwards (a rename over it would otherwise
    /// trip Windows sharing discipline).
    fn needs_rebuild(path: &Path, records: &[Record]) -> bool {
        if !path.is_file() {
            return true;
        }
        (|| -> Result<bool> {
            let idx = Self::connect(path)?;
            let stale = idx.schema_version()? != Some(INDEX_SCHEMA_VERSION)
                || idx.indexed_len()? != Some(records.len() as u64)
                || !Self::owned_schema_valid(&idx.conn)?;
            idx.close()?;
            Ok(stale)
        })()
        .unwrap_or(true)
    }

    /// Build a complete replacement index beside `path` and install it
    /// with a same-directory rename, clearing stale sidecars before and
    /// after the swap. The old index remains readable until the
    /// replacement is complete.
    fn replace_with_rebuild(path: &Path, records: &[Record]) -> Result<()> {
        let dir = path.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "index path has no parent")
        })?;
        let tmp = tempfile::Builder::new()
            .prefix("index-rebuild-")
            .suffix(".sqlite")
            .tempfile_in(dir)?;
        let tmp = tmp.into_temp_path();
        let tmp_path: &Path = tmp.as_ref();
        let result = (|| {
            let mut idx = Self::connect(tmp_path)?;
            idx.rebuild(records)?;
            idx.close()?;
            // The replacement rename covers only the database file: a
            // WAL or hot journal the old index left behind would replay
            // into the replacement on next open, restoring the stale
            // shape while the rebuild reports success.
            cv_store::paths::remove_sqlite_sidecars(path)?;
            cv_store::paths::replace_file(tmp_path, path)?;
            cv_store::paths::remove_sqlite_sidecars(path)?;
            Ok(())
        })();
        let _ = cv_store::paths::remove_sqlite_sidecars(tmp_path);
        result
    }

    /// Close the journal writer shape around a full index reset: acquire
    /// the journal's writer lock by opening it, build a complete SQLite
    /// replacement beside the live index, then install it by rename.
    /// Returns the committed journal records replayed into the index.
    pub fn rebuild_from_journal(project_root: &Path) -> Result<u64> {
        let journal = Journal::open(project_root)?;
        let records = journal.records();
        let index_dir = cv_store::paths::create_project_index_dir(project_root, "ledger")?;
        let path = index_dir.join("index.sqlite");
        // The journal writer lock is held here, so sidecars are always
        // stale.
        Self::replace_with_rebuild(&path, records)?;
        Ok(records.len() as u64)
    }

    fn close(self) -> Result<()> {
        self.conn.close().map_err(|(_, e)| e.into())
    }

    fn connect(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    fn schema_version(&self) -> Result<Option<u64>> {
        Ok(self
            .conn
            .query_row("SELECT v FROM meta WHERE k = 'index_schema'", [], |r| {
                r.get::<_, i64>(0)
            })
            .optional()?
            .map(|v| v as u64))
    }

    fn indexed_len(&self) -> Result<Option<u64>> {
        Ok(self
            .conn
            .query_row("SELECT v FROM meta WHERE k = 'journal_len'", [], |r| {
                r.get::<_, i64>(0)
            })
            .optional()?
            .map(|v| v as u64))
    }

    /// Drop everything and replay the journal.
    pub fn rebuild(&mut self, records: &[Record]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute_batch("DELETE FROM nouns; DELETE FROM runs; DELETE FROM meta;")?;
        for r in records {
            let (k, id, run, target, json) = match &r.entry {
                Entry::Artifact(n) => (
                    kind::ARTIFACT,
                    &n.id,
                    &n.run,
                    &n.target,
                    serde_json::to_string(n)?,
                ),
                Entry::Check(n) => (
                    kind::CHECK,
                    &n.id,
                    &n.run,
                    &n.target,
                    serde_json::to_string(n)?,
                ),
                Entry::Claim(n) => (
                    kind::CLAIM,
                    &n.id,
                    &n.run,
                    &n.target,
                    serde_json::to_string(n)?,
                ),
                Entry::Delivery(n) => (
                    kind::DELIVERY,
                    &n.id,
                    &n.run,
                    &n.target,
                    serde_json::to_string(n)?,
                ),
                _ => continue,
            };
            tx.execute(
                "INSERT OR REPLACE INTO nouns (kind, id, run, target, json) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![k, id.0, run.0, target, json],
            )?;
        }
        for (id, v) in runs(records) {
            tx.execute(
                "INSERT INTO runs (run, target, phase) VALUES (?1, ?2, ?3)",
                params![id.0, v.target, v.phase().as_str()],
            )?;
        }
        tx.execute(
            "INSERT INTO meta (k, v) VALUES ('journal_len', ?1), ('index_schema', ?2)",
            params![records.len() as i64, INDEX_SCHEMA_VERSION as i64],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// `(id, json)` of every noun of `kind` in a run, ordered by id.
    pub fn nouns(&self, kind: &str, run: &str) -> Result<Vec<(String, String)>> {
        let mut st = self
            .conn
            .prepare("SELECT id, json FROM nouns WHERE kind = ?1 AND run = ?2 ORDER BY id")?;
        let rows = st
            .query_map(params![kind, run], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn run_phase(&self, run: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT phase FROM runs WHERE run = ?1", params![run], |r| {
                r.get(0)
            })
            .optional()?)
    }

    /// `(run, target, phase)` for every run.
    pub fn runs(&self) -> Result<Vec<(String, String, String)>> {
        let mut st = self
            .conn
            .prepare("SELECT run, target, phase FROM runs ORDER BY run")?;
        let rows = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
