//! SQLite projection of the journal. Disposable: delete it and
//! [`LedgerIndex::open`] rebuilds it from JSONL. Never written to except
//! by replaying journal records.

use crate::Result;
use crate::journal::{Entry, Journal, Record};
use crate::nouns::kind;
use crate::runs::runs;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};

pub const INDEX_REL: &str = ".cv/ledger/index.sqlite";

pub struct LedgerIndex {
    conn: Connection,
    path: PathBuf,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS nouns (
  kind TEXT NOT NULL, id TEXT NOT NULL, run TEXT NOT NULL, target TEXT NOT NULL,
  json TEXT NOT NULL, PRIMARY KEY (kind, id));
CREATE TABLE IF NOT EXISTS runs (
  run TEXT PRIMARY KEY, target TEXT NOT NULL, phase TEXT NOT NULL);
";

impl LedgerIndex {
    pub fn path_for(project_root: &Path) -> PathBuf {
        project_root.join(INDEX_REL)
    }

    /// Open the index and bring it in line with the journal. A stale,
    /// foreign, or corrupt index file is discarded and rebuilt.
    pub fn open(project_root: &Path, journal: &Journal) -> Result<Self> {
        let path = Self::path_for(project_root);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut idx = match Self::connect(&path) {
            Ok(i) => i,
            Err(_) => {
                std::fs::remove_file(&path)?;
                Self::connect(&path)?
            }
        };
        if idx.indexed_len()? != Some(journal.records().len() as u64) {
            idx.rebuild(journal.records())?;
        }
        Ok(idx)
    }

    fn connect(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
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
            "INSERT INTO meta (k, v) VALUES ('journal_len', ?1)",
            params![records.len() as i64],
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
