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
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{File, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::thread::JoinHandle;

pub const DIR: &str = ".cv/store";
pub const DOCS: &str = "docs.jsonl";
pub const WRITER_LOCK: &str = "writer.lock";
pub const WRITER_LOCK_REL: &str = ".cv/store/writer.lock";
pub const INDEX: &str = "index.sqlite";
const RECORD_SCHEMA: u32 = 1;
const INDEX_SCHEMA: &str = "2";
const ACTOR_QUEUE: usize = 128;

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
    /// This open created `docs.jsonl`.
    pub journal_created: bool,
    /// This open created `docs.jsonl` and flushed its containing directory
    /// entries (Unix evidence only; Windows has no directory handle to flush).
    pub journal_dir_synced: bool,
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
        Ok(m) if crate::paths::is_link(&m) => Err(Error::Symlink(p.to_path_buf())),
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

struct JournalScan {
    records: Vec<Record>,
    scan: Scan,
    committed_len: usize,
    hash: Sha256,
}

/// Decode the committed prefix of journal bytes. Blank lines do not name
/// records, but they remain part of the hash input because the index must
/// track the exact journal bytes it projected.
fn scan_bytes(raw: &[u8]) -> JournalScan {
    let committed_len = raw.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let committed = &raw[..committed_len];
    let mut hash = Sha256::new();
    hash.update(committed);
    let mut scan = Scan {
        torn_bytes: (raw.len() - committed_len) as u64,
        ..Scan::default()
    };
    let mut records = Vec::new();
    for line in committed.split(|b| *b == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        scan.lines += 1;
        match serde_json::from_slice::<Record>(line) {
            Ok(r) if r.schema_version == RECORD_SCHEMA => records.push(r),
            Ok(_) => scan.unsupported += 1,
            Err(_) => scan.undecodable += 1,
        }
    }
    JournalScan {
        records,
        scan,
        committed_len,
        hash,
    }
}

/// Read the currently committed journal prefix through the writer's open
/// file. The actor serializes every use of this file, so the end-position
/// restore cannot race another store operation.
fn scan_journal_file(file: &mut File) -> Result<JournalScan, Error> {
    file.seek(SeekFrom::Start(0))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    file.seek(SeekFrom::End(0))?;
    Ok(scan_bytes(&buf))
}

/// A `docs.jsonl` scan: the file replayed into records, in append order.
fn read_journal(path: &Path) -> Result<(File, JournalScan, bool), Error> {
    let (mut file, created) = crate::open::open_create_no_follow_status(path)?;
    let scan = scan_journal_file(&mut file)?;
    if scan.scan.torn_bytes > 0 {
        file.set_len(scan.committed_len as u64)?;
        file.sync_all()?;
        file.seek(SeekFrom::End(0))?;
    }
    Ok((file, scan, created))
}

fn hash_hex(hash: &Sha256) -> String {
    let digest = hash.clone().finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Make a freshly created journal's directory entries durable (Unix;
/// Windows has no directory handle to flush).
fn sync_dirs(project_root: &Path, path: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        let mut dir = path.parent();
        while let Some(d) = dir {
            File::open(d)?.sync_all()?;
            if d == project_root {
                break;
            }
            dir = d.parent();
        }
    }
    #[cfg(not(unix))]
    let _ = (project_root, path);
    Ok(())
}

const INDEX_DDL: &str = "
CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);
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

fn meta_value(conn: &Connection, k: &str) -> Result<Option<String>, Error> {
    Ok(conn
        .query_row("SELECT v FROM meta WHERE k = ?1", params![k], |r| {
            r.get::<_, String>(0)
        })
        .optional()?)
}

/// Whether the index at `path` cannot stand in for a replay of this exact
/// journal: missing, unreadable, foreign, wrong-shaped, wrong schema version,
/// or built from different journal bytes. The probe connection is always
/// closed before returning, so the caller may replace the file immediately
/// afterwards.
fn needs_rebuild(path: &Path, lines: u64, journal_hash: &str) -> bool {
    if !path.is_file() {
        return true;
    }
    (|| -> Result<bool, Error> {
        let conn = connect_index(path)?;
        let lines = lines.to_string();
        let stale = meta_value(&conn, "index_schema")?.as_deref() != Some(INDEX_SCHEMA)
            || meta_value(&conn, "journal_len")?.as_deref() != Some(lines.as_str())
            || meta_value(&conn, "journal_hash")?.as_deref() != Some(journal_hash)
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
fn rebuild_index_atomic(
    path: &Path,
    records: &[Record],
    lines: u64,
    journal_hash: &str,
) -> Result<(), Error> {
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
                "INSERT INTO meta (k, v) VALUES
                 ('index_schema', ?1), ('journal_len', ?2), ('journal_hash', ?3)",
                params![INDEX_SCHEMA, lines.to_string(), journal_hash,],
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

struct Inner {
    journal: File,
    index_path: PathBuf,
    index: Option<Connection>,
    lines: u64,
    journal_hash: Sha256,
    /// Held until the store drops; the lock file stays on disk.
    _writer_lock: File,
}

fn close_index(conn: Connection) {
    match conn.close() {
        Ok(()) => {}
        Err((conn, _)) => drop(conn),
    }
}

fn projection(records: &[Record]) -> BTreeMap<(String, String), Value> {
    let mut out = BTreeMap::new();
    for r in records {
        out.insert((r.ns.clone(), r.key.clone()), r.doc.clone());
    }
    out
}

fn index_put(
    conn: &mut Connection,
    ns: &Namespace,
    key: &StableId,
    doc: &Value,
    lines: u64,
    journal_hash: &str,
) -> Result<(), Error> {
    let json = serde_json::to_string(doc)?;
    // The journal already holds the truth; index the row in the same
    // transaction as the freshness token so a failed index write leaves
    // the index stale (rebuilt from the journal), never half-fresh.
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT OR REPLACE INTO docs (ns, key, json) VALUES (?1, ?2, ?3)",
        params![ns.0, key.0, json],
    )?;
    tx.execute(
        "INSERT INTO meta (k, v) VALUES ('journal_len', ?1), ('journal_hash', ?2)
         ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        params![lines.to_string(), journal_hash],
    )?;
    tx.commit()?;
    Ok(())
}

fn rebuild_inner_index(inner: &mut Inner) -> Result<(), Error> {
    if let Some(conn) = inner.index.take() {
        close_index(conn);
    }
    let scan = scan_journal_file(&mut inner.journal)?;
    let hash = hash_hex(&scan.hash);
    rebuild_index_atomic(&inner.index_path, &scan.records, scan.scan.lines, &hash)?;
    inner.index = Some(connect_index(&inner.index_path)?);
    inner.lines = scan.scan.lines;
    inner.journal_hash = scan.hash;
    Ok(())
}

/// Append the truth record first. That append is the commit point: once it
/// is durable, an index failure can never make the write uncommitted. The
/// index transaction is refreshed afterwards, with an immediate rebuild as
/// its recovery path; a failed rebuild leaves no index connection and reads
/// scan the journal until the next open retries recovery.
fn put_inner(inner: &mut Inner, ns: &Namespace, key: &StableId, doc: &Value) -> Result<(), Error> {
    let rec = Record {
        schema_version: RECORD_SCHEMA,
        ns: ns.0.clone(),
        key: key.0.clone(),
        doc: doc.clone(),
    };
    let mut line = serde_json::to_vec(&rec)?;
    line.push(b'\n');
    inner.journal.write_all(&line)?;
    inner.journal.sync_data()?;
    inner.journal_hash.update(&line);
    inner.lines += 1;

    if inner.index.is_some() {
        let hash = hash_hex(&inner.journal_hash);
        let lines = inner.lines;
        let indexed = inner
            .index
            .as_mut()
            .map(|conn| index_put(conn, ns, key, doc, lines, &hash));
        if matches!(indexed, Some(Err(_))) {
            let _ = rebuild_inner_index(inner);
        }
    }
    Ok(())
}

fn index_get(conn: &Connection, ns: &Namespace, key: &StableId) -> Result<Option<Value>, Error> {
    let json = conn
        .query_row(
            "SELECT json FROM docs WHERE ns = ?1 AND key = ?2",
            params![ns.0, key.0],
            |r| r.get::<_, String>(0),
        )
        .optional()?;
    json.map(|j| serde_json::from_str(&j).map_err(Error::from))
        .transpose()
}

fn fallback_get(inner: &mut Inner, ns: &Namespace, key: &StableId) -> Result<Option<Value>, Error> {
    let scan = scan_journal_file(&mut inner.journal)?;
    Ok(projection(&scan.records)
        .get(&(ns.0.clone(), key.0.clone()))
        .cloned())
}

fn get_inner(inner: &mut Inner, ns: &Namespace, key: &StableId) -> Result<Option<Value>, Error> {
    if inner.index.is_some() {
        let first = index_get(inner.index.as_ref().expect("checked"), ns, key);
        if let Ok(value) = first {
            return Ok(value);
        }
        if rebuild_inner_index(inner).is_ok() {
            let retry = index_get(inner.index.as_ref().expect("rebuilt"), ns, key);
            if let Ok(value) = retry {
                return Ok(value);
            }
        }
    }
    fallback_get(inner, ns, key)
}

fn index_query(
    conn: &mut Connection,
    ns: &Namespace,
    pred: &Predicate,
) -> Result<Vec<Value>, Error> {
    let mut st = conn.prepare("SELECT json FROM docs WHERE ns = ?1 ORDER BY key")?;
    let rows = st.query_map(params![ns.0], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        let doc: Value = serde_json::from_str(&row?)?;
        if pred.matches_json(&doc) {
            out.push(doc);
        }
    }
    Ok(out)
}

fn fallback_query(
    inner: &mut Inner,
    ns: &Namespace,
    pred: &Predicate,
) -> Result<Vec<Value>, Error> {
    let scan = scan_journal_file(&mut inner.journal)?;
    Ok(projection(&scan.records)
        .into_iter()
        .filter(|((n, _), _)| n == &ns.0)
        .map(|(_, doc)| doc)
        .filter(|doc| pred.matches_json(doc))
        .collect())
}

fn query_inner(inner: &mut Inner, ns: &Namespace, pred: &Predicate) -> Result<Vec<Value>, Error> {
    if inner.index.is_some() {
        let first = index_query(inner.index.as_mut().expect("checked"), ns, pred);
        if let Ok(value) = first {
            return Ok(value);
        }
        if rebuild_inner_index(inner).is_ok() {
            let retry = index_query(inner.index.as_mut().expect("rebuilt"), ns, pred);
            if let Ok(value) = retry {
                return Ok(value);
            }
        }
    }
    fallback_query(inner, ns, pred)
}

enum Command {
    Put {
        ns: Namespace,
        key: StableId,
        doc: Value,
        reply: Sender<cv_core::Result<()>>,
    },
    Get {
        ns: Namespace,
        key: StableId,
        reply: Sender<cv_core::Result<Option<Value>>>,
    },
    Query {
        ns: Namespace,
        pred: Predicate,
        reply: Sender<cv_core::Result<Vec<Value>>>,
    },
}

#[cfg(test)]
fn put_threads() -> &'static Mutex<Option<(std::thread::ThreadId, std::thread::ThreadId)>> {
    static THREADS: std::sync::OnceLock<
        Mutex<Option<(std::thread::ThreadId, std::thread::ThreadId)>>,
    > = std::sync::OnceLock::new();
    THREADS.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
fn note_put_thread(slot: usize) {
    let mut threads = put_threads().lock().unwrap_or_else(|e| e.into_inner());
    let (caller, actor) = threads
        .take()
        .unwrap_or_else(|| (std::thread::current().id(), std::thread::current().id()));
    *threads = Some(if slot == 0 {
        (std::thread::current().id(), actor)
    } else {
        (caller, std::thread::current().id())
    });
}

fn actor_loop(rx: Receiver<Command>, mut inner: Inner) {
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Command::Put {
                ns,
                key,
                doc,
                reply,
            } => {
                #[cfg(test)]
                note_put_thread(1);
                let _ = reply.send(put_inner(&mut inner, &ns, &key, &doc).map_err(store_err));
            }
            Command::Get { ns, key, reply } => {
                let _ = reply.send(get_inner(&mut inner, &ns, &key).map_err(store_err));
            }
            Command::Query { ns, pred, reply } => {
                let _ = reply.send(query_inner(&mut inner, &ns, &pred).map_err(store_err));
            }
        }
    }
}

/// The on-disk `Store`: `.cv/store/docs.jsonl` is truth, the app-data
/// SQLite index serves reads. One writer lock covers the journal and
/// the index for the life of the store. Journal and SQLite work run on
/// this store's single actor thread; callers keep the synchronous port
/// semantics without performing disk work inline.
pub struct DiskStore {
    tx: Option<SyncSender<Command>>,
    join: Option<JoinHandle<()>>,
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

    fn request<T>(
        &self,
        command: impl FnOnce(Sender<cv_core::Result<T>>) -> Command,
    ) -> cv_core::Result<T> {
        let Some(tx) = &self.tx else {
            return Err(store_err("store actor stopped"));
        };
        let (reply, rx) = channel();
        tx.send(command(reply))
            .map_err(|_| store_err("store actor stopped"))?;
        rx.recv().map_err(|_| store_err("store actor stopped"))?
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
        let writer = crate::open::open_create_no_follow(&writer_path)?;
        crate::lock::try_lock_settled(&writer).map_err(|e| match e {
            TryLockError::WouldBlock => Error::Locked(writer_path.clone()),
            TryLockError::Error(e) => Error::Io(e),
        })?;
        let (journal, journal_scan, journal_created) = read_journal(&journal_path)?;
        let mut journal_dir_synced = false;
        if journal_created {
            journal.sync_all()?;
            sync_dirs(root, &journal_path)?;
            journal_dir_synced = cfg!(unix);
        }
        let index_dir = crate::paths::create_project_index_dir(root, "store")?;
        let index_path = index_dir.join(INDEX);
        let journal_hash = hash_hex(&journal_scan.hash);
        let mut index_rebuilt = false;
        if force_index_rebuild || needs_rebuild(&index_path, journal_scan.scan.lines, &journal_hash)
        {
            rebuild_index_atomic(
                &index_path,
                &journal_scan.records,
                journal_scan.scan.lines,
                &journal_hash,
            )?;
            index_rebuilt = true;
        }
        let index = connect_index(&index_path)?;
        // Drop a pre-app-data index (and its journal artifacts); it
        // rebuilds from `docs.jsonl`.
        for suffix in ["", "-journal", "-wal", "-shm"] {
            let _ = std::fs::remove_file(dir.join(format!("{INDEX}{suffix}")));
        }
        let inner = Inner {
            journal,
            index_path,
            index: Some(index),
            lines: journal_scan.scan.lines,
            journal_hash: journal_scan.hash,
            _writer_lock: writer,
        };
        let scan = journal_scan.scan;
        let (tx, rx) = sync_channel(ACTOR_QUEUE);
        let join = std::thread::Builder::new()
            .name("cv-store-docs".into())
            .spawn(move || actor_loop(rx, inner))?;
        Ok((
            Self {
                tx: Some(tx),
                join: Some(join),
            },
            Report {
                scan,
                index_rebuilt,
                schema,
                journal_created,
                journal_dir_synced,
            },
        ))
    }
}

impl Drop for DiskStore {
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Store for DiskStore {
    fn put(&self, ns: &Namespace, key: &StableId, doc: &Value) -> cv_core::Result<()> {
        #[cfg(test)]
        note_put_thread(0);
        let ns = ns.clone();
        let key = key.clone();
        let doc = doc.clone();
        self.request(|reply| Command::Put {
            ns,
            key,
            doc,
            reply,
        })
    }

    fn get(&self, ns: &Namespace, key: &StableId) -> cv_core::Result<Option<Value>> {
        let ns = ns.clone();
        let key = key.clone();
        self.request(|reply| Command::Get { ns, key, reply })
    }

    fn query(&self, ns: &Namespace, pred: &Predicate) -> cv_core::Result<Vec<Value>> {
        let ns = ns.clone();
        let pred = pred.clone();
        self.request(|reply| Command::Query { ns, pred, reply })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_appends_run_on_the_store_actor_not_the_caller() {
        let root = tempfile::tempdir().unwrap();
        let (store, _) = DiskStore::open(root.path()).unwrap();
        store
            .put(
                &Namespace("a".into()),
                &StableId("k".into()),
                &serde_json::json!({}),
            )
            .unwrap();
        let (caller, actor) = put_threads()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .expect("put thread ids recorded");
        assert_ne!(caller, actor, "disk work must not run inline");
    }
}
