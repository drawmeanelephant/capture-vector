//! `.cv/audit/lifecycle.jsonl` — the Findings mailbox's lifecycle
//! sidecar. One record per transition, keyed by `DedupKey`; the last
//! record for a key wins, so a finding that re-emits on the next build
//! keeps its snooze (`Ignored`) or `Wontfix`. Append-only JSONL like the
//! research journal: a torn tail is truncated on open, undecodable or
//! other-schema lines are counted and skipped, never fatal.
//!
//! `.cv/` lives inside cloned (untrusted) projects, so a symlinked
//! `.cv`, `.cv/audit` or journal is refused rather than followed.

use crate::sidecar::{self, ADOPT_UNSTAMPED, Namespace, SidecarError};
use cv_core::rules::{DedupKey, FindingLifecycle};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const DIR: &str = ".cv/audit";
pub const LIFECYCLE: &str = "lifecycle.jsonl";
pub const SCHEMA_VERSION: u32 = 1;
pub const SCHEMA: Namespace = Namespace {
    name: "audit",
    dir: DIR,
    current: SCHEMA_VERSION,
    migrations: &[ADOPT_UNSTAMPED],
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleRecord {
    pub schema_version: u32,
    pub key: DedupKey,
    pub to: FindingLifecycle,
}

impl LifecycleRecord {
    pub fn new(key: DedupKey, to: FindingLifecycle) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            key,
            to,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Scan {
    pub lines: u64,
    pub undecodable: u64,
    /// Decodable lines written under another `schema_version`.
    pub unsupported: u64,
    pub torn_bytes: u64,
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Symlink(PathBuf),
    Json(serde_json::Error),
    Schema(SidecarError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Symlink(p) => write!(f, "refusing symlinked {}", p.display()),
            Error::Json(e) => write!(f, "{e}"),
            Error::Schema(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub struct LifecycleLog {
    file: File,
    path: PathBuf,
    _writer: File,
}

fn refuse_symlink(p: &Path) -> Result<(), Error> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.file_type().is_symlink() => Err(Error::Symlink(p.to_path_buf())),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Effective lifecycle per key, in first-seen order.
pub type Lifecycles = Vec<(DedupKey, FindingLifecycle)>;

impl LifecycleLog {
    /// Open (creating) `root/.cv/audit/lifecycle.jsonl`; returns the
    /// effective lifecycle per key (last record wins, first-seen order).
    pub fn open(root: &Path) -> Result<(Self, Lifecycles, Scan), Error> {
        let cv = root.join(".cv");
        let dir = root.join(DIR);
        let path = dir.join(LIFECYCLE);
        let writer_path = path.with_extension("lock");
        for p in [&cv, &dir, &path, &writer_path] {
            refuse_symlink(p)?;
        }
        sidecar::ensure(root, &SCHEMA).map_err(Error::Schema)?;
        let writer = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&writer_path)?;
        crate::lock::try_lock_settled(&writer).map_err(std::io::Error::other)?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
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
        let mut out: Vec<(DedupKey, FindingLifecycle)> = Vec::new();
        let mut at: HashMap<DedupKey, usize> = HashMap::new();
        for line in buf[..good].split(|b| *b == b'\n') {
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            scan.lines += 1;
            let rec = match serde_json::from_slice::<LifecycleRecord>(line) {
                Ok(r) if r.schema_version == SCHEMA_VERSION => r,
                Ok(_) => {
                    scan.unsupported += 1;
                    continue;
                }
                Err(_) => {
                    scan.undecodable += 1;
                    continue;
                }
            };
            match at.get(&rec.key) {
                Some(&i) => out[i].1 = rec.to,
                None => {
                    at.insert(rec.key.clone(), out.len());
                    out.push((rec.key, rec.to));
                }
            }
        }
        Ok((
            Self {
                file,
                path,
                _writer: writer,
            },
            out,
            scan,
        ))
    }

    pub fn append(&mut self, key: &DedupKey, to: FindingLifecycle) -> Result<(), Error> {
        let mut line =
            serde_json::to_vec(&LifecycleRecord::new(key.clone(), to)).map_err(Error::Json)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.file.sync_data()?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cv_core::id::{AccountId, NounId, NounKind, StableId, noun_kind};
    use cv_core::sys::Hash;

    fn key(code: &str, page: &str) -> DedupKey {
        DedupKey {
            code: code.into(),
            subject: NounId {
                account: AccountId("local".into()),
                kind: NounKind::register(noun_kind::PAGE),
                stable: StableId(page.into()),
            },
            target_hash: Hash([7; 32]),
        }
    }

    #[test]
    fn last_record_per_key_wins_across_reopen() {
        let d = tempfile::tempdir().unwrap();
        let (mut log, got, _) = LifecycleLog::open(d.path()).unwrap();
        assert!(got.is_empty());
        log.append(&key("A", "x"), FindingLifecycle::Ignored)
            .unwrap();
        log.append(&key("B", "y"), FindingLifecycle::Wontfix)
            .unwrap();
        log.append(&key("A", "x"), FindingLifecycle::Open).unwrap();
        log.append(&key("A", "x"), FindingLifecycle::Ignored)
            .unwrap();
        drop(log);
        let (_, got, scan) = LifecycleLog::open(d.path()).unwrap();
        assert_eq!(scan.lines, 4);
        assert_eq!(
            got,
            vec![
                (key("A", "x"), FindingLifecycle::Ignored),
                (key("B", "y"), FindingLifecycle::Wontfix)
            ]
        );
    }

    #[test]
    fn a_second_writer_cannot_recover_the_active_journal() {
        let root = tempfile::tempdir().unwrap();
        let (mut log, _, _) = LifecycleLog::open(root.path()).unwrap();
        log.append(&key("A", "x"), FindingLifecycle::Ignored)
            .unwrap();
        let before = std::fs::read(log.path()).unwrap();
        assert!(LifecycleLog::open(root.path()).is_err());
        assert_eq!(std::fs::read(log.path()).unwrap(), before);
        drop(log);
        let (_, records, _) = LifecycleLog::open(root.path()).unwrap();
        assert_eq!(records, vec![(key("A", "x"), FindingLifecycle::Ignored)]);
    }

    #[test]
    fn an_unstamped_audit_journal_is_adopted_without_losing_snoozes() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let record = LifecycleRecord::new(key("A", "x"), FindingLifecycle::Ignored);
        let bytes = format!("{}\n", serde_json::to_string(&record).unwrap());
        std::fs::write(dir.join(LIFECYCLE), &bytes).unwrap();
        let (_, records, _) = LifecycleLog::open(root.path()).unwrap();
        assert_eq!(records, vec![(record.key, FindingLifecycle::Ignored)]);
        assert_eq!(std::fs::read_to_string(dir.join(LIFECYCLE)).unwrap(), bytes);
        assert_eq!(
            sidecar::read_version(root.path(), &SCHEMA).unwrap(),
            Some(1)
        );
    }

    #[test]
    fn a_newer_audit_schema_is_refused_without_changing_its_journal() {
        let root = tempfile::tempdir().unwrap();
        let (mut log, _, _) = LifecycleLog::open(root.path()).unwrap();
        log.append(&key("A", "x"), FindingLifecycle::Ignored)
            .unwrap();
        let journal = log.path().to_path_buf();
        drop(log);
        let stamp = sidecar::stamp_path(root.path(), &SCHEMA);
        let future = serde_json::json!({
            "format": sidecar::FORMAT, "namespace": "audit", "schema_version": 2
        })
        .to_string();
        std::fs::write(&stamp, &future).unwrap();
        let before = std::fs::read(&journal).unwrap();
        assert!(matches!(
            LifecycleLog::open(root.path()),
            Err(Error::Schema(SidecarError::TooNew { .. }))
        ));
        assert_eq!(std::fs::read(&journal).unwrap(), before);
        assert_eq!(std::fs::read_to_string(&stamp).unwrap(), future);
    }

    #[test]
    fn torn_tail_and_foreign_schema_are_skipped() {
        let d = tempfile::tempdir().unwrap();
        let (mut log, _, _) = LifecycleLog::open(d.path()).unwrap();
        log.append(&key("A", "x"), FindingLifecycle::Ignored)
            .unwrap();
        let path = log.path().to_path_buf();
        drop(log);
        let mut future = serde_json::to_value(LifecycleRecord::new(
            key("B", "y"),
            FindingLifecycle::Ignored,
        ))
        .unwrap();
        future["schema_version"] = 2.into();
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "{future}").unwrap();
        writeln!(f, "not json").unwrap();
        write!(f, "{{\"torn").unwrap();
        drop(f);
        let (mut log, got, scan) = LifecycleLog::open(d.path()).unwrap();
        assert_eq!(got, vec![(key("A", "x"), FindingLifecycle::Ignored)]);
        assert_eq!((scan.unsupported, scan.undecodable), (1, 1));
        assert!(scan.torn_bytes > 0);
        assert!(std::fs::read(&path).unwrap().ends_with(b"\n"));
        log.append(&key("C", "z"), FindingLifecycle::Wontfix)
            .unwrap();
        drop(log);
        let (_, got, _) = LifecycleLog::open(d.path()).unwrap();
        assert_eq!(
            got,
            vec![
                (key("A", "x"), FindingLifecycle::Ignored),
                (key("C", "z"), FindingLifecycle::Wontfix),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn planted_symlinks_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), d.path().join(".cv")).unwrap();
        assert!(matches!(
            LifecycleLog::open(d.path()),
            Err(Error::Symlink(_))
        ));
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(DIR)).unwrap();
        let target = elsewhere.path().join("victim");
        std::os::unix::fs::symlink(&target, d.path().join(DIR).join(LIFECYCLE)).unwrap();
        assert!(matches!(
            LifecycleLog::open(d.path()),
            Err(Error::Symlink(_))
        ));
        assert!(!target.exists());
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(DIR)).unwrap();
        let lock = d.path().join(DIR).join(LIFECYCLE).with_extension("lock");
        std::os::unix::fs::symlink(&target, lock).unwrap();
        assert!(matches!(
            LifecycleLog::open(d.path()),
            Err(Error::Symlink(_))
        ));
        assert!(!target.exists());
    }
}
