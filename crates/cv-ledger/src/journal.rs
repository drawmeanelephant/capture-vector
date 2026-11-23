//! Append-only JSONL truth. One record per line; a record is committed
//! when its newline is on disk (`sync_data` after every append).

use crate::nouns::{ArtifactNoun, CheckNoun, ClaimNoun, DeliveryNoun};
use crate::{LedgerError, Result};
use cv_core::id::StableId;
use cv_core::publish::{PreflightGrant, RunState};
use cv_core::sys::{Hash, Timestamp};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Project-relative journal location.
pub const JOURNAL_REL: &str = ".cv/ledger/ledger.jsonl";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub seq: u64,
    pub at: Timestamp,
    pub entry: Entry,
}

/// Machine-export freshness verdict for one declared export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportEvidence {
    pub kind: String,
    pub path: String,
    /// `present` (sealed and unchanged), `missing`, or `stale`.
    pub status: String,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Entry {
    Artifact(ArtifactNoun),
    Check(CheckNoun),
    Claim(ClaimNoun),
    Delivery(DeliveryNoun),
    /// Preflight passed; the grant and the exact sealed plan + target
    /// config are stored verbatim so publish can re-derive and compare.
    GrantIssued {
        run: StableId,
        target: String,
        grant: PreflightGrant,
        plan: serde_json::Value,
        target_cfg: serde_json::Value,
    },
    PreflightBlocked {
        run: StableId,
        target: String,
        blocks: Vec<String>,
    },
    /// Explicit user intent, persisted before any dispatch.
    Approved {
        run: StableId,
        by: String,
        grant_digest: Hash,
    },
    RunState {
        run: StableId,
        state: RunState,
        note: Option<String>,
    },
    /// Publish refused (hash mismatch, not approved, unreconciled run…).
    Refused {
        run: StableId,
        reason: String,
    },
    Reconciled {
        run: StableId,
        observation: String,
        by: String,
    },
    ExportsChecked {
        run: StableId,
        exports: Vec<ExportEvidence>,
    },
}

pub struct Journal {
    path: PathBuf,
    file: File,
    records: Vec<Record>,
    torn_tail: Option<String>,
}

impl Journal {
    pub fn path_for(project_root: &Path) -> PathBuf {
        project_root.join(JOURNAL_REL)
    }

    /// Open (creating if absent). A final line without its newline is a
    /// torn append — never committed — and is truncated away; a corrupt
    /// committed line fails loudly.
    pub fn open(project_root: &Path) -> Result<Self> {
        let path = Self::path_for(project_root);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let raw = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        let committed_len = raw.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        let torn_tail = (committed_len < raw.len())
            .then(|| String::from_utf8_lossy(&raw[committed_len..]).into_owned());
        let mut records = Vec::new();
        for (i, line) in raw[..committed_len].split(|b| *b == b'\n').enumerate() {
            if line.is_empty() {
                continue;
            }
            let rec: Record = serde_json::from_slice(line).map_err(|e| LedgerError::Corrupt {
                line: i + 1,
                msg: e.to_string(),
            })?;
            if rec.seq != records.len() as u64 {
                return Err(LedgerError::Corrupt {
                    line: i + 1,
                    msg: format!("seq {} out of order", rec.seq),
                });
            }
            records.push(rec);
        }
        if torn_tail.is_some() {
            let f = OpenOptions::new().write(true).open(&path)?;
            f.set_len(committed_len as u64)?;
            f.sync_all()?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            path,
            file,
            records,
            torn_tail,
        })
    }

    /// Append + fsync. Returns only after the record is durable.
    pub fn append(&mut self, at: Timestamp, entry: Entry) -> Result<&Record> {
        let rec = Record {
            seq: self.records.len() as u64,
            at,
            entry,
        };
        let mut line = serde_json::to_vec(&rec)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.file.sync_data()?;
        self.records.push(rec);
        Ok(self.records.last().expect("just pushed"))
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Bytes of a torn final append dropped on open, if any.
    pub fn torn_tail(&self) -> Option<&str> {
        self.torn_tail.as_deref()
    }
}
