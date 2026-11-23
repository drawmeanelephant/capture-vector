//! `.cv/research/` on disk: `notes.jsonl` + `provenance.jsonl` are truth
//! (append-only, fsync'd, torn tail dropped on open); `index.sqlite` is a
//! FTS5 index rebuilt from them whenever it is missing, corrupt or stale.

use crate::notebook::Notebook;
use crate::port::ResearchPort;
use crate::record::{Provenance, Record};
use crate::session::Research;
use cv_core::engine::{Engine, PageDraft, WriteOutcome};
use cv_core::id::{AccountId, StableId};
use cv_core::sys::{Clock, Timestamp};
use rusqlite::{Connection, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const DIR: &str = ".cv/research";
pub const NOTES: &str = "notes.jsonl";
pub const PROVENANCE: &str = "provenance.jsonl";
pub const INDEX: &str = "index.sqlite";
const INDEX_SCHEMA: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum DiskError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

/// Append-only JSONL file.
pub struct Jsonl {
    file: File,
    lines: u64,
}

/// What [`Jsonl::open`] found.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Scan {
    pub lines: u64,
    /// Bytes of an unterminated final line that were truncated away.
    pub torn_bytes: u64,
    /// Complete lines that failed to decode (kept on disk, skipped).
    pub undecodable: u64,
}

impl Jsonl {
    pub fn open<T: DeserializeOwned>(path: &Path) -> Result<(Self, Vec<T>, Scan), DiskError> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(path)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        let good = buf.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        let mut scan = Scan {
            torn_bytes: (buf.len() - good) as u64,
            ..Scan::default()
        };
        if scan.torn_bytes > 0 {
            // Append-only handles can't truncate on Windows.
            let w = OpenOptions::new().write(true).open(path)?;
            w.set_len(good as u64)?;
            w.sync_all()?;
        }
        let mut out = Vec::new();
        for line in buf[..good].split(|b| *b == b'\n') {
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            scan.lines += 1;
            match serde_json::from_slice(line) {
                Ok(v) => out.push(v),
                Err(_) => scan.undecodable += 1,
            }
        }
        Ok((
            Self {
                file,
                lines: scan.lines,
            },
            out,
            scan,
        ))
    }

    pub fn append<T: Serialize>(&mut self, v: &T) -> Result<(), DiskError> {
        let mut line = serde_json::to_vec(v)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.file.sync_data()?;
        self.lines += 1;
        Ok(())
    }

    pub fn lines(&self) -> u64 {
        self.lines
    }
}

/// Rebuildable FTS5 projection of the notes journal.
pub struct FtsIndex {
    conn: Connection,
}

impl FtsIndex {
    fn open(path: &Path) -> Result<Self, DiskError> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cv_index_meta(
                 namespace TEXT PRIMARY KEY, schema INTEGER NOT NULL, journal_lines INTEGER NOT NULL);
             CREATE VIRTUAL TABLE IF NOT EXISTS research_notes_fts
                 USING fts5(note UNINDEXED, title, body, tags);",
        )?;
        Ok(Self { conn })
    }

    /// Open, or delete and recreate when the file is not a usable index.
    fn open_or_reset(path: &Path) -> Result<Self, DiskError> {
        match Self::open(path).and_then(|i| i.journal_lines().map(|_| i)) {
            Ok(i) => Ok(i),
            Err(_) => {
                let _ = std::fs::remove_file(path);
                Self::open(path)
            }
        }
    }

    fn journal_lines(&self) -> Result<Option<u64>, DiskError> {
        let mut st = self
            .conn
            .prepare("SELECT schema, journal_lines FROM cv_index_meta WHERE namespace = ?1")?;
        let mut rows = st.query(params![crate::NAMESPACE])?;
        Ok(match rows.next()? {
            Some(r) if r.get::<_, i64>(0)? == INDEX_SCHEMA => Some(r.get::<_, i64>(1)? as u64),
            _ => None,
        })
    }

    fn set_lines(&self, lines: u64) -> Result<(), DiskError> {
        self.conn.execute(
            "INSERT INTO cv_index_meta(namespace, schema, journal_lines) VALUES (?1, ?2, ?3)
             ON CONFLICT(namespace) DO UPDATE SET schema = ?2, journal_lines = ?3",
            params![crate::NAMESPACE, INDEX_SCHEMA, lines as i64],
        )?;
        Ok(())
    }

    pub fn rebuild(&mut self, nb: &Notebook, lines: u64) -> Result<(), DiskError> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM research_notes_fts", [])?;
        for n in nb.notes() {
            let tags: Vec<&str> = n.tags.iter().map(String::as_str).collect();
            tx.execute(
                "INSERT INTO research_notes_fts(note, title, body, tags) VALUES (?1, ?2, ?3, ?4)",
                params![n.id.0, n.title, n.body_md, tags.join(" ")],
            )?;
        }
        tx.commit()?;
        self.set_lines(lines)
    }

    fn apply(&self, rec: &Record, lines: u64) -> Result<(), DiskError> {
        match rec {
            Record::Captured { note } => {
                self.conn.execute(
                    "INSERT INTO research_notes_fts(note, title, body, tags) VALUES (?1, ?2, ?3, '')",
                    params![note.id.0, note.title, note.body_md],
                )?;
            }
            Record::Edited {
                note,
                title,
                body_md,
                ..
            } => {
                if let Some(t) = title {
                    self.conn.execute(
                        "UPDATE research_notes_fts SET title = ?2 WHERE note = ?1",
                        params![note.0, t],
                    )?;
                }
                if let Some(b) = body_md {
                    self.conn.execute(
                        "UPDATE research_notes_fts SET body = ?2 WHERE note = ?1",
                        params![note.0, b],
                    )?;
                }
            }
            Record::Tagged { note, tag, .. } => {
                self.conn.execute(
                    "UPDATE research_notes_fts SET tags = trim(tags || ' ' || ?2) WHERE note = ?1",
                    params![note.0, tag],
                )?;
            }
            _ => {}
        }
        self.set_lines(lines)
    }

    /// Every whitespace token becomes a quoted FTS5 phrase, ANDed — user
    /// text never reaches the MATCH grammar unescaped.
    pub fn search(&self, query: &str) -> Result<Vec<StableId>, DiskError> {
        let q: Vec<String> = query
            .split_whitespace()
            .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
            .collect();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let mut st = self.conn.prepare(
            "SELECT note FROM research_notes_fts WHERE research_notes_fts MATCH ?1 ORDER BY rank, note",
        )?;
        let rows = st.query_map(params![q.join(" ")], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(|r| r.ok()).map(StableId).collect())
    }
}

/// What [`open`] recovered.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OpenReport {
    pub notes: Scan,
    pub provenance: Scan,
    /// Decoded records the fold rejected (e.g. edits of unknown notes).
    pub rejected: u64,
    pub index_rebuilt: bool,
}

pub struct DiskPort {
    root: PathBuf,
    notes: Jsonl,
    provenance: Jsonl,
    index: FtsIndex,
    clock: Box<dyn Clock + Send>,
    writer: Option<Box<dyn Engine>>,
}

impl DiskPort {
    /// Attach the engine whose `write_content` performs promotions.
    pub fn with_writer(mut self, engine: Box<dyn Engine>) -> Self {
        self.writer = Some(engine);
        self
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Replay `.cv/research/` under `root` into a session.
pub fn open(
    root: &Path,
    account: AccountId,
    clock: Box<dyn Clock + Send>,
) -> Result<(Research<DiskPort>, OpenReport), DiskError> {
    let dir = root.join(DIR);
    let (notes, recs, notes_scan) = Jsonl::open::<Record>(&dir.join(NOTES))?;
    let (provenance, provs, prov_scan) = Jsonl::open::<Provenance>(&dir.join(PROVENANCE))?;
    let mut nb = Notebook::new(account);
    let mut report = OpenReport {
        notes: notes_scan,
        provenance: prov_scan,
        ..OpenReport::default()
    };
    for r in &recs {
        if nb.apply(r).is_err() {
            report.rejected += 1;
        }
    }
    for p in &provs {
        if nb.apply_provenance(p).is_err() {
            report.rejected += 1;
        }
    }
    let mut index = FtsIndex::open_or_reset(&dir.join(INDEX))?;
    if index.journal_lines()? != Some(notes.lines()) {
        index.rebuild(&nb, notes.lines())?;
        report.index_rebuilt = true;
    }
    let port = DiskPort {
        root: root.to_path_buf(),
        notes,
        provenance,
        index,
        clock,
        writer: None,
    };
    Ok((Research::new(nb, port), report))
}

impl ResearchPort for DiskPort {
    fn now(&self) -> Timestamp {
        self.clock.now()
    }
    fn commit(&mut self, rec: &Record) -> Result<(), String> {
        self.notes.append(rec).map_err(|e| e.to_string())?;
        // The index is derived: a failed update only means the next open
        // sees a stale line count and rebuilds.
        let _ = self.index.apply(rec, self.notes.lines());
        Ok(())
    }
    fn commit_provenance(&mut self, p: &Provenance) -> Result<(), String> {
        self.provenance.append(p).map_err(|e| e.to_string())
    }
    fn write_page(&mut self, draft: &PageDraft) -> Option<Result<WriteOutcome, String>> {
        let w = self.writer.as_ref()?;
        Some(
            w.write_content(draft, draft.base)
                .map_err(|e| e.to_string()),
        )
    }
    fn search(&self, query: &str) -> Option<Vec<StableId>> {
        self.index.search(query).ok()
    }
    fn describe(&self) -> String {
        format!("{DIR}/{NOTES} ({} lines)", self.notes.lines())
    }
}
