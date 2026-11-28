//! `.cv/store/` on disk — the on-disk `cv_core::store::Store` port.
//! `docs.jsonl` is the source of truth (append-only, fsync'd, a torn
//! tail is truncated on open, undecodable or other-schema lines are
//! counted and skipped). `index.sqlite` in OS app-data (keyed by the
//! canonical project root, never under `.cv/`) is a rebuildable index
//! that serves `get`/`query`; a missing, stale, foreign or corrupt
//! index is deleted and rebuilt by replaying the journal.
//!
//! Predicate evaluation never reaches SQL: `query` scans the index's
//! JSON column and delegates each document to `Predicate::matches_json`,
//! the same comparator `Predicate::matches` and `StubStore` use, so the
//! disk store and the in-memory oracle cannot diverge. Boris IR is
//! never persisted — callers journal event envelopes and lane documents,
//! not `GraphDocument`s.
//!
//! `.cv/` lives inside cloned (untrusted) projects, so a symlinked
//! `.cv`, `.cv/store`, journal or lock is refused rather than followed.

use crate::sidecar::{self, ADOPT_UNSTAMPED, SidecarError};
use cv_core::id::StableId;
use cv_core::store::{Namespace, Predicate, Store, Value};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const DIR: &str = ".cv/store";
pub const DOCS: &str = "docs.jsonl";
pub const WRITER_LOCK: &str = "writer.lock";
pub const WRITER_LOCK_REL: &str = ".cv/store/writer.lock";
pub const INDEX: &str = "index.sqlite";
const RECORD_SCHEMA: u32 = 1;
const INDEX_SCHEMA: i64 = 1;

/// `.cv/store/` sidecar schema. v1 = `docs.jsonl` of [`Record`] lines;
/// v0 is the same shape, unstamped.
pub const SCHEMA: sidecar::Namespace = sidecar::Namespace {
    name: "store",
    dir: DIR,
    current: RECORD_SCHEMA,
    migrations: &[ADOPT_UNSTAMPED],
};

#[derive(Debug, Serialize, Deserialize)]
struct Record {
    schema_version: u32,
    ns: String,
    key: String,
    doc: Value,
}

/// What the journal scan found on open.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Scan {
    pub lines: u64,
    pub undecodable: u64,
    /// Decodable lines written under another `schema_version`.
    pub unsupported: u64,
    /// Bytes of an unterminated final line that were truncated away.
    pub torn_bytes: u64,
}

/// What [`DiskStore::open`] recovered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub scan: Scan,
    /// The index was (re)built from the journal this open.
    pub index_rebuilt: bool,
    pub schema: sidecar::Outcome,
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Sql(rusqlite::Error),
    Json(serde_json::Error),
    Schema(SidecarError),
    /// `.cv/store` state is a symlink to somewhere else.
    Symlink(PathBuf),
    /// Another writer holds the stable store lock.
    Locked(PathBuf),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Sql(e) => write!(f, "{e}"),
            Error::Json(e) => write!(f, "{e}"),
            Error::Schema(e) => write!(f, "{e}"),
            Error::Symlink(p) => write!(f, "refusing symlinked {}", p.display()),
            Error::Locked(p) => write!(
                f,
                "lock acquisition failed because the operation would block: {}",
                p.display()
            ),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Sql(e)
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Json(e)
    }
}
impl From<SidecarError> for Error {
    fn from(e: SidecarError) -> Self {
        Error::Schema(e)
    }
}

fn refuse_symlink(p: &Path) -> Result<(), Error> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.file_type().is_symlink() => Err(Error::Symlink(p.to_path_buf())),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn store_err(e: impl std::fmt::Display) -> cv_core::Error {
    cv_core::Error::Store(e.to_string())
}

/// OS app-data path for this project's rebuildable index (keyed by the
/// canonical root, never inside `.cv/`).
pub fn index_path(root: &Path) -> std::io::Result<PathBuf> {
    Ok(crate::paths::project_index_dir(root, "store")?.join(INDEX))
}

/// A `docs.jsonl` scan: the file replayed into records, in append order.
fn read_journal(path: &Path) -> Result<(File, Vec<Record>, Scan), Error> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    let good = buf.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let mut scan = Scan {
        torn_bytes: (buf.len() - good) as u64,
        ..Scan::default()
    };
    if scan.torn_bytes > 0 {
        file.set_len(good as u64)?;
        file.sync_all()?;
    }
    file.seek(SeekFrom::End(0))?;
    let mut out = Vec::new();
    for line in buf[..good].split(|b| *b == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        scan.lines += 1;
        match serde_json::from_slice::<Record>(line) {
            Ok(r) if r.schema_version == RECORD_SCHEMA => out.push(r),
            Ok(_) => scan.unsupported += 1,
            Err(_) => scan.undecodable += 1,
        }
    }
    Ok((file, out, scan))
}

const INDEX_DDL: &str = "
CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS docs (
  ns TEXT NOT NULL, key TEXT NOT NULL, json TEXT NOT NULL,
  PRIMARY KEY (ns, key));
";

fn connect_index(path: &Path) -> Result<Connection, Error> {
    let conn = Connection::open(path)?;
    conn.execute_batch(INDEX_DDL)?;
    Ok(conn)
}

/// Column names of `table` in order; empty when the table is absent.
fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>, Error> {
    let mut st = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let cols = st
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(cols)
}

/// Whether the opened database is exactly the owned projection shape:
/// the meta and docs tables with their expected columns and nothing
/// else. Foreign debris or wrong columns mean the file cannot be
/// trusted however fresh its metadata counters look.
fn owned_schema_valid(conn: &Connection) -> Result<bool, Error> {
    let mut st = conn.prepare(
        "SELECT name FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' AND type != 'table'",
    )?;
    if st.exists([])? {
        return Ok(false);
    }
    let mut st = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )?;
    let mut tables: Vec<String> = st
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    tables.sort();
    if tables != ["docs", "meta"] {
        return Ok(false);
    }
    Ok(table_columns(conn, "meta")? == ["k", "v"]
        && table_columns(conn, "docs")? == ["ns", "key", "json"])
}

fn meta_value(conn: &Connection, k: &str) -> Result<Option<i64>, Error> {
    Ok(conn
        .query_row("SELECT v FROM meta WHERE k = ?1", params![k], |r| r.get(0))
        .optional()?)
}

/// Whether the index at `path` cannot stand in for a replay of `lines`
/// journal lines: missing, unreadable, foreign, wrong-shaped, wrong
/// schema version or a stale journal length. The probe connection is
/// always closed before returning, so the caller may replace the file
/// immediately afterwards.
fn needs_rebuild(path: &Path, lines: u64) -> bool {
    if !path.is_file() {
        return true;
    }
    (|| -> Result<bool, Error> {
        let conn = connect_index(path)?;
        let stale = meta_value(&conn, "index_schema")? != Some(INDEX_SCHEMA)
            || meta_value(&conn, "journal_len")? != Some(lines as i64)
            || !owned_schema_valid(&conn)?;
        conn.close().map_err(|(_, e)| Error::Sql(e))?;
        Ok(stale)
    })()
    .unwrap_or(true)
}

/// Build a complete replacement index beside `path` and install it with
/// a same-directory rename, clearing stale sidecars before and after the
/// swap. The old index remains readable until the replacement is
/// complete.
fn rebuild_index_atomic(path: &Path, records: &[Record], lines: u64) -> Result<(), Error> {
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
        let mut conn = connect_index(tmp_path)?;
        {
            let tx = conn.transaction()?;
            for r in records {
                tx.execute(
                    "INSERT OR REPLACE INTO docs (ns, key, json) VALUES (?1, ?2, ?3)",
                    params![r.ns, r.key, serde_json::to_string(&r.doc)?],
                )?;
            }
            tx.execute(
                "INSERT INTO meta (k, v) VALUES ('index_schema', ?1), ('journal_len', ?2)",
                params![INDEX_SCHEMA, lines as i64],
            )?;
            tx.commit()?;
        }
        conn.close().map_err(|(_, e)| Error::Sql(e))?;
        // The replacement rename covers only the database file: a WAL
        // or hot journal the old index left behind would replay into
        // the replacement on next open. Connections are quiesced (the
        // writer lock is ours), so sidecars here are always stale.
        crate::paths::remove_sqlite_sidecars(path)?;
        crate::paths::replace_file(tmp_path, path)?;
        crate::paths::remove_sqlite_sidecars(path)?;
        Ok(())
    })();
    let _ = crate::paths::remove_sqlite_sidecars(tmp_path);
    result
}

/// The on-disk `Store`: `.cv/store/docs.jsonl` is truth, the app-data
/// SQLite index serves reads. One writer lock covers the journal and
/// the index for the life of the store.
pub struct DiskStore {
    inner: Mutex<Inner>,
}

struct Inner {
    journal: File,
    index: Connection,
    lines: u64,
    /// Held until the store drops; the lock file stays on disk.
    _writer_lock: File,
}

impl DiskStore {
    /// Open `.cv/store/` under `root`, stamping/migrating the sidecar
    /// schema first and rebuilding the index when it is missing, stale,
    /// foreign or corrupt.
    pub fn open(root: &Path) -> Result<(Self, Report), Error> {
        Self::open_inner(root, false)
    }

    /// Open after forcibly rebuilding the SQLite index from the journal.
    pub fn open_rebuilding_index(root: &Path) -> Result<(Self, Report), Error> {
        Self::open_inner(root, true)
    }

    fn open_inner(root: &Path, force_index_rebuild: bool) -> Result<(Self, Report), Error> {
        let cv = root.join(".cv");
        let dir = root.join(DIR);
        let journal_path = dir.join(DOCS);
        let writer_path = dir.join(WRITER_LOCK);
        for p in [&cv, &dir, &journal_path, &writer_path] {
            refuse_symlink(p)?;
        }
        let schema = sidecar::ensure(root, &SCHEMA)?;
        let writer = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&writer_path)?;
        crate::lock::try_lock_settled(&writer).map_err(|e| match e {
            TryLockError::WouldBlock => Error::Locked(writer_path.clone()),
            TryLockError::Error(e) => Error::Io(e),
        })?;
        let (journal, records, scan) = read_journal(&journal_path)?;
        let index_dir = crate::paths::create_project_index_dir(root, "store")?;
        let index_path = index_dir.join(INDEX);
        let mut index_rebuilt = false;
        if force_index_rebuild || needs_rebuild(&index_path, scan.lines) {
            rebuild_index_atomic(&index_path, &records, scan.lines)?;
            index_rebuilt = true;
        }
        let index = connect_index(&index_path)?;
        // Drop a pre-app-data index (and its journal artifacts); it
        // rebuilds from `docs.jsonl`.
        for suffix in ["", "-journal", "-wal", "-shm"] {
            let _ = std::fs::remove_file(dir.join(format!("{INDEX}{suffix}")));
        }
        Ok((
            Self {
                inner: Mutex::new(Inner {
                    journal,
                    index,
                    lines: scan.lines,
                    _writer_lock: writer,
                }),
            },
            Report {
                scan,
                index_rebuilt,
                schema,
            },
        ))
    }
}

impl Store for DiskStore {
    fn put(&self, ns: &Namespace, key: &StableId, doc: &Value) -> cv_core::Result<()> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let rec = Record {
            schema_version: RECORD_SCHEMA,
            ns: ns.0.clone(),
            key: key.0.clone(),
            doc: doc.clone(),
        };
        let mut line = serde_json::to_vec(&rec).map_err(store_err)?;
        line.push(b'\n');
        inner
            .journal
            .write_all(&line)
            .and_then(|()| inner.journal.sync_data())
            .map_err(store_err)?;
        let json = serde_json::to_string(doc).map_err(store_err)?;
        // The journal already holds the truth; index the row in the same
        // transaction as the line counter so a failed index write leaves
        // the index stale (rebuilt on next open), never half-fresh.
        let lines = inner.lines + 1;
        (|| -> Result<(), Error> {
            let tx = inner.index.transaction()?;
            tx.execute(
                "INSERT OR REPLACE INTO docs (ns, key, json) VALUES (?1, ?2, ?3)",
                params![ns.0, key.0, json],
            )?;
            tx.execute(
                "INSERT INTO meta (k, v) VALUES ('journal_len', ?1)
                 ON CONFLICT(k) DO UPDATE SET v = ?1",
                params![lines as i64],
            )?;
            tx.commit()?;
            Ok(())
        })()
        .map_err(store_err)?;
        inner.lines = lines;
        Ok(())
    }

    fn get(&self, ns: &Namespace, key: &StableId) -> cv_core::Result<Option<Value>> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let json = inner
            .index
            .query_row(
                "SELECT json FROM docs WHERE ns = ?1 AND key = ?2",
                params![ns.0, key.0],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(store_err)?;
        json.map(|j| serde_json::from_str(&j).map_err(store_err))
            .transpose()
    }

    fn query(&self, ns: &Namespace, pred: &Predicate) -> cv_core::Result<Vec<Value>> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut st = inner
            .index
            .prepare("SELECT json FROM docs WHERE ns = ?1 ORDER BY key")
            .map_err(store_err)?;
        let rows = st
            .query_map(params![ns.0], |r| r.get::<_, String>(0))
            .map_err(store_err)?;
        let mut out = Vec::new();
        for row in rows {
            let doc: Value = serde_json::from_str(&row.map_err(store_err)?).map_err(store_err)?;
            if pred.matches_json(&doc) {
                out.push(doc);
            }
        }
        Ok(out)
    }
}

/// The `Store` the shell runs before a workspace binds a `DiskStore`
/// (and when the on-disk store refuses to open): journaled evidence has
/// nowhere durable to go, so it lives in memory for the session and is
/// never attributed to the wrong project.
#[derive(Default)]
pub struct EphemeralStore {
    inner: Mutex<BTreeMap<(Namespace, StableId), Value>>,
}

impl EphemeralStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Store for EphemeralStore {
    fn put(&self, ns: &Namespace, key: &StableId, doc: &Value) -> cv_core::Result<()> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((ns.clone(), key.clone()), doc.clone());
        Ok(())
    }

    fn get(&self, ns: &Namespace, key: &StableId) -> cv_core::Result<Option<Value>> {
        Ok(self
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(ns.clone(), key.clone()))
            .cloned())
    }

    fn query(&self, ns: &Namespace, pred: &Predicate) -> cv_core::Result<Vec<Value>> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Ok(inner
            .iter()
            .filter(|((n, _), _)| n == ns)
            .filter(|(_, doc)| pred.matches_json(doc))
            .map(|(_, doc)| doc.clone())
            .collect())
    }
}
