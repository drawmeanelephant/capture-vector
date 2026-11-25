//! `.cv/imports/` — one directory per `boris-migration-lab` run:
//!
//! - `<run-id>/run.json`: what Capture Vector asked the lab to do and how
//!   the process ended ([`RunRecord`]), written before the spawn and
//!   rewritten when it finishes;
//! - `<run-id>/out/`: the lab's `--out`, exactly as the lab wrote it;
//! - `review.jsonl`: the triage lifecycle of review items (the lab's
//!   `human_review` / `unsupported` entries), one record per transition,
//!   last record per `(run, item)` wins. Triage never blocks anything.
//!
//! Inputs are never copied here: a record keeps each input's path and a
//! hash of it. `.cv/` comes with cloned (untrusted) projects, so
//! symlinked entries are refused or skipped rather than followed.

use crate::sidecar::{self, ADOPT_UNSTAMPED, Namespace, SidecarError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

pub const DIR: &str = ".cv/imports";
pub const RECORD: &str = "run.json";
pub const OUT: &str = "out";
pub const REVIEW: &str = "review.jsonl";
/// `format` of a [`RunRecord`].
pub const RECORD_FORMAT: &str = "cv-import-run";
pub const SCHEMA_VERSION: u32 = 1;
pub const SCHEMA: Namespace = Namespace {
    name: "imports",
    dir: DIR,
    current: SCHEMA_VERSION,
    migrations: &[ADOPT_UNSTAMPED],
};

/// One lab input as the run used it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedInput {
    /// The lab flag, e.g. `--root`.
    pub flag: String,
    /// Canonical path the lab was given.
    pub path: String,
    /// sha256 of `path` (the path string, not the content).
    pub path_sha256: String,
}

/// The lab binary that ran, as its `--version` gate saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedLab {
    pub tool_id: String,
    /// How it was located (`PATH`, `CV_MIGRATION_LAB_BIN`, …).
    pub source: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finished {
    /// `None` when the process ended by signal.
    pub exit_code: Option<i32>,
    /// The lab's exit class: `ok`, `usage` or `io`.
    pub exit: String,
    /// How the process ended, e.g. `exited 0` or `signal 9`.
    pub outcome: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Adopted {
    pub at_ms: i64,
    pub workspace: String,
}

/// `<run-id>/run.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub format: String,
    pub schema_version: u32,
    pub run_id: String,
    /// The `--mode=` value.
    pub mode: String,
    pub inputs: Vec<RecordedInput>,
    /// The argv after the binary, as spawned.
    pub argv: Vec<String>,
    pub lab: RecordedLab,
    /// The Boris pin this app gates candidate content with.
    pub boris_pin: Option<String>,
    pub started_ms: i64,
    /// `None` while the lab runs (or when the app stopped before it ended).
    pub finished: Option<Finished>,
    /// The adapter refused the run before spawning, with its reason.
    pub refused: Option<String>,
    pub adopted: Option<Adopted>,
}

impl RunRecord {
    pub fn started(
        run_id: impl Into<String>,
        mode: impl Into<String>,
        lab: RecordedLab,
        started_ms: i64,
    ) -> Self {
        Self {
            format: RECORD_FORMAT.into(),
            schema_version: SCHEMA_VERSION,
            run_id: run_id.into(),
            mode: mode.into(),
            inputs: Vec::new(),
            argv: Vec::new(),
            lab,
            boris_pin: None,
            started_ms,
            finished: None,
            refused: None,
            adopted: None,
        }
    }
}

pub fn input(flag: &str, path: &str) -> RecordedInput {
    RecordedInput {
        flag: flag.into(),
        path: path.into(),
        path_sha256: sha256_hex(path.as_bytes()),
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A path-free stable id for a review item, from the fields that name it.
pub fn item_id(parts: &[&str]) -> String {
    sha256_hex(parts.join("\0").as_bytes())[..16].to_string()
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Symlink(PathBuf),
    RunId(String),
    Json(serde_json::Error),
    Schema(SidecarError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Symlink(p) => write!(f, "refusing symlinked {}", p.display()),
            Error::RunId(id) => write!(f, "run id {id:?} must be one plain path segment"),
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

fn is_symlink(p: &Path) -> Result<bool, Error> {
    match std::fs::symlink_metadata(p) {
        Ok(m) => Ok(m.file_type().is_symlink()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

fn refuse_symlinks(paths: &[&Path]) -> Result<(), Error> {
    for p in paths {
        if is_symlink(p)? {
            return Err(Error::Symlink(p.to_path_buf()));
        }
    }
    Ok(())
}

fn plain_segment(id: &str) -> bool {
    let mut parts = Path::new(id).components();
    matches!(
        (parts.next(), parts.next()),
        (Some(Component::Normal(_)), None)
    ) && !id.contains(['/', '\\'])
}

/// `<root>/.cv/imports/<run_id>`.
pub fn run_dir(root: &Path, run_id: &str) -> Result<PathBuf, Error> {
    if !plain_segment(run_id) {
        return Err(Error::RunId(run_id.into()));
    }
    Ok(root.join(DIR).join(run_id))
}

/// Write (or rewrite) `record` atomically; returns the run directory.
pub fn write(root: &Path, record: &RunRecord) -> Result<PathBuf, Error> {
    let dir = run_dir(root, &record.run_id)?;
    let path = dir.join(RECORD);
    let tmp = dir.join(format!("{RECORD}.tmp"));
    refuse_symlinks(&[&root.join(".cv"), &root.join(DIR), &dir, &path, &tmp])?;
    sidecar::ensure(root, &SCHEMA).map_err(Error::Schema)?;
    std::fs::create_dir_all(&dir)?;
    let mut bytes = serde_json::to_vec_pretty(record).map_err(Error::Json)?;
    bytes.push(b'\n');
    let mut f = File::create(&tmp)?;
    f.write_all(&bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, &path)?;
    Ok(dir)
}

/// One `<run-id>/` directory as found on disk.
#[derive(Debug)]
pub struct RunDir {
    pub id: String,
    pub dir: PathBuf,
    /// Why `run.json` is missing or unreadable, otherwise the record.
    pub record: Result<RunRecord, String>,
}

impl RunDir {
    pub fn out(&self) -> PathBuf {
        self.dir.join(OUT)
    }
}

fn read_record(path: &Path) -> Result<RunRecord, String> {
    if is_symlink(path).map_err(|e| e.to_string())? {
        return Err(format!("{RECORD} is a symlink"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{RECORD}: {e}"))?;
    #[derive(Deserialize)]
    struct Header {
        format: String,
        schema_version: u64,
    }
    let h: Header =
        serde_json::from_slice(&bytes).map_err(|e| format!("{RECORD} is not valid: {e}"))?;
    if h.format != RECORD_FORMAT {
        return Err(format!(
            "{RECORD} format {:?} is not {RECORD_FORMAT}",
            h.format
        ));
    }
    if h.schema_version != u64::from(SCHEMA_VERSION) {
        return Err(format!(
            "{RECORD} schema_version {} is not supported (this build reads {SCHEMA_VERSION})",
            h.schema_version
        ));
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("{RECORD} is not valid: {e}"))
}

/// Every run directory under `.cv/imports/`, sorted by id. Read-only: no
/// stamp is written. Symlinked entries are skipped.
pub fn scan(root: &Path) -> Result<Vec<RunDir>, Error> {
    let base = root.join(DIR);
    refuse_symlinks(&[&root.join(".cv"), &base])?;
    let entries = match std::fs::read_dir(&base) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for e in entries {
        let e = e?;
        if !e.file_type()?.is_dir() {
            continue;
        }
        let Ok(id) = e.file_name().into_string() else {
            continue;
        };
        let dir = e.path();
        let record = read_record(&dir.join(RECORD));
        out.push(RunDir { id, dir, record });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Triage state of one review item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewState {
    Open,
    Accepted,
    Deferred,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRecord {
    pub schema_version: u32,
    pub run: String,
    pub item: String,
    pub to: ReviewState,
}

/// Effective state per `(run, item)`, in first-seen order.
pub type ReviewStates = Vec<((String, String), ReviewState)>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Scan {
    pub lines: u64,
    pub undecodable: u64,
    pub unsupported: u64,
    pub torn_bytes: u64,
}

/// `.cv/imports/review.jsonl`, append-only. A torn tail is truncated on
/// open; undecodable or other-schema lines are counted and skipped.
pub struct ReviewLog {
    file: File,
    path: PathBuf,
}

impl ReviewLog {
    pub fn open(root: &Path) -> Result<(Self, ReviewStates, Scan), Error> {
        let base = root.join(DIR);
        let path = base.join(REVIEW);
        refuse_symlinks(&[&root.join(".cv"), &base, &path])?;
        sidecar::ensure(root, &SCHEMA).map_err(Error::Schema)?;
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
        let mut out: ReviewStates = Vec::new();
        let mut at: HashMap<(String, String), usize> = HashMap::new();
        for line in buf[..good].split(|b| *b == b'\n') {
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            scan.lines += 1;
            let rec = match serde_json::from_slice::<ReviewRecord>(line) {
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
            let key = (rec.run, rec.item);
            match at.get(&key) {
                Some(&i) => out[i].1 = rec.to,
                None => {
                    at.insert(key.clone(), out.len());
                    out.push((key, rec.to));
                }
            }
        }
        Ok((Self { file, path }, out, scan))
    }

    pub fn append(&mut self, run: &str, item: &str, to: ReviewState) -> Result<(), Error> {
        let rec = ReviewRecord {
            schema_version: SCHEMA_VERSION,
            run: run.into(),
            item: item.into(),
            to,
        };
        let mut line = serde_json::to_vec(&rec).map_err(Error::Json)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.file.sync_data()?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// `adoption.json` in a run directory: the page-level link from adopted
/// Boris content back to the lab run and source input. This is Capture
/// Vector evidence, not a lab mirror; the copied lab documents remain the
/// authoritative conversion provenance.
pub const ADOPTION: &str = "adoption.json";
pub const ADOPTION_FORMAT: &str = "cv-import-adoption";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptedPage {
    /// The lab's entity id for the emitted page.
    pub entity_id: String,
    /// Page path relative to the target workspace's content input.
    pub path: String,
    /// Source path exactly as the lab report names it, when it does.
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptionRecord {
    pub format: String,
    pub schema_version: u32,
    pub run_id: String,
    pub mode: String,
    pub at_ms: i64,
    pub workspace: String,
    /// `new` or `existing`.
    pub target: String,
    /// Subfolder below the content input for an existing-workspace merge.
    pub content_subfolder: Option<String>,
    /// External inputs the run consumed; they are referenced, never copied.
    pub inputs: Vec<RecordedInput>,
    pub pages: Vec<AdoptedPage>,
}

impl AdoptionRecord {
    pub fn new(
        run_id: impl Into<String>,
        mode: impl Into<String>,
        at_ms: i64,
        workspace: impl Into<String>,
        target: impl Into<String>,
    ) -> Self {
        Self {
            format: ADOPTION_FORMAT.into(),
            schema_version: SCHEMA_VERSION,
            run_id: run_id.into(),
            mode: mode.into(),
            at_ms,
            workspace: workspace.into(),
            target: target.into(),
            content_subfolder: None,
            inputs: Vec::new(),
            pages: Vec::new(),
        }
    }
}

fn write_json_atomic(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let tmp = path.with_extension("json.tmp");
    refuse_symlinks(&[path, &tmp])?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Write `<run>/adoption.json` atomically under the imports schema stamp.
pub fn write_adoption(root: &Path, rec: &AdoptionRecord) -> Result<PathBuf, Error> {
    let dir = run_dir(root, &rec.run_id)?;
    refuse_symlinks(&[&root.join(".cv"), &root.join(DIR), &dir])?;
    sidecar::ensure(root, &SCHEMA).map_err(Error::Schema)?;
    std::fs::create_dir_all(&dir)?;
    let mut bytes = serde_json::to_vec_pretty(rec).map_err(Error::Json)?;
    bytes.push(b'\n');
    let path = dir.join(ADOPTION);
    write_json_atomic(&path, &bytes)?;
    Ok(path)
}

fn read_adoption(path: &Path) -> Result<AdoptionRecord, String> {
    if is_symlink(path).map_err(|e| e.to_string())? {
        return Err(format!("{ADOPTION} is a symlink"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{ADOPTION}: {e}"))?;
    #[derive(Deserialize)]
    struct Header {
        format: String,
        schema_version: u64,
    }
    let h: Header =
        serde_json::from_slice(&bytes).map_err(|e| format!("{ADOPTION} is not valid: {e}"))?;
    if h.format != ADOPTION_FORMAT {
        return Err(format!(
            "{ADOPTION} format {:?} is not {ADOPTION_FORMAT}",
            h.format
        ));
    }
    if h.schema_version != u64::from(SCHEMA_VERSION) {
        return Err(format!(
            "{ADOPTION} schema_version {} is not supported (this build reads {SCHEMA_VERSION})",
            h.schema_version
        ));
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("{ADOPTION} is not valid: {e}"))
}

/// Valid adoption records under `.cv/imports/`, sorted by run id. Missing
/// records simply mean the run has not been adopted; undecodable records
/// are skipped here and stay visible in the run's ordinary scan problem.
pub fn scan_adoptions(root: &Path) -> Result<Vec<AdoptionRecord>, Error> {
    let base = root.join(DIR);
    refuse_symlinks(&[&root.join(".cv"), &base])?;
    let entries = match std::fs::read_dir(&base) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for e in entries {
        let e = e?;
        if !e.file_type()?.is_dir() {
            continue;
        }
        let path = e.path().join(ADOPTION);
        if !path.exists() {
            continue;
        }
        if let Ok(rec) = read_adoption(&path) {
            out.push(rec);
        }
    }
    out.sort_by(|a, b| a.run_id.cmp(&b.run_id));
    Ok(out)
}

/// `adoption-pending.json` in a run directory: the crash-recovery unit of
/// an in-progress adoption. It is written before the first content byte
/// lands and deleted only after the adoption sidecar and run record are
/// durable. Its presence means a previous attempt was interrupted: a
/// retry with the same target shape resumes byte-identical files instead
/// of refusing them as collisions, while any different bytes still refuse.
pub const PENDING: &str = "adoption-pending.json";
pub const PENDING_FORMAT: &str = "cv-import-adoption-pending";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingFile {
    /// Workspace-relative staged-write target, `/`-separated.
    pub target: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingAdoption {
    pub format: String,
    pub schema_version: u32,
    pub run_id: String,
    /// `new` or `existing`.
    pub target: String,
    pub content_input: String,
    pub content_subfolder: Option<String>,
    pub files: Vec<PendingFile>,
}

impl PendingAdoption {
    pub fn new(
        run_id: impl Into<String>,
        target: impl Into<String>,
        content_input: impl Into<String>,
    ) -> Self {
        Self {
            format: PENDING_FORMAT.into(),
            schema_version: SCHEMA_VERSION,
            run_id: run_id.into(),
            target: target.into(),
            content_input: content_input.into(),
            content_subfolder: None,
            files: Vec::new(),
        }
    }

    /// True when a retry names the same target shape; any other shape is a
    /// different operation that must not resume this one.
    pub fn matches(&self, target: &str, content_input: &str, subfolder: Option<&str>) -> bool {
        self.target == target
            && self.content_input == content_input
            && self.content_subfolder.as_deref() == subfolder
    }
}

/// Write the recovery unit atomically inside the run directory.
pub fn write_pending(root: &Path, rec: &PendingAdoption) -> Result<PathBuf, Error> {
    let dir = run_dir(root, &rec.run_id)?;
    refuse_symlinks(&[&root.join(".cv"), &root.join(DIR), &dir])?;
    sidecar::ensure(root, &SCHEMA).map_err(Error::Schema)?;
    std::fs::create_dir_all(&dir)?;
    let mut bytes = serde_json::to_vec_pretty(rec).map_err(Error::Json)?;
    bytes.push(b'\n');
    let path = dir.join(PENDING);
    write_json_atomic(&path, &bytes)?;
    Ok(path)
}

/// The interrupted adoption recorded for `run`, if one is durable and
/// decodable. An undecodable or foreign journal is an error, never a
/// silent fresh start: resuming over unknown partial state could mix two
/// different target shapes. Errors are strings like [`read_record`]'s.
pub fn read_pending(root: &Path, run: &str) -> Result<Option<PendingAdoption>, String> {
    let dir = run_dir(root, run).map_err(|e| e.to_string())?;
    let path = dir.join(PENDING);
    if is_symlink(&path).map_err(|e| e.to_string())? {
        return Err(format!("{PENDING} is a symlink"));
    }
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{PENDING}: {e}")),
    };
    #[derive(Deserialize)]
    struct Header {
        format: String,
        schema_version: u64,
    }
    let h: Header =
        serde_json::from_slice(&bytes).map_err(|e| format!("{PENDING} is not valid: {e}"))?;
    if h.format != PENDING_FORMAT {
        return Err(format!(
            "{PENDING} format {:?} is not {PENDING_FORMAT}",
            h.format
        ));
    }
    if h.schema_version != u64::from(SCHEMA_VERSION) {
        return Err(format!(
            "{PENDING} schema_version {} is not supported (this build reads {SCHEMA_VERSION})",
            h.schema_version
        ));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| format!("{PENDING} is not valid: {e}"))
}

/// Delete the recovery unit after the adoption it covers is durable.
pub fn clear_pending(root: &Path, run: &str) -> Result<(), Error> {
    let dir = run_dir(root, run)?;
    let path = dir.join(PENDING);
    refuse_symlinks(&[&root.join(".cv"), &root.join(DIR), &dir, &path])?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lab() -> RecordedLab {
        RecordedLab {
            tool_id: "boris-migration-lab/0.8.1".into(),
            source: "PATH".into(),
            path: "/bin/boris-migration-lab".into(),
        }
    }

    #[test]
    fn a_written_record_scans_back_beside_its_output() {
        let d = tempfile::tempdir().unwrap();
        let mut rec = RunRecord::started("r1", "hugo", lab(), 5);
        rec.inputs.push(input("--root", "/src/site"));
        let dir = write(d.path(), &rec).unwrap();
        std::fs::create_dir_all(dir.join(OUT)).unwrap();
        rec.finished = Some(Finished {
            exit_code: Some(0),
            exit: "ok".into(),
            outcome: "exited 0".into(),
            duration_ms: 12,
        });
        write(d.path(), &rec).unwrap();
        let runs = scan(d.path()).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, "r1");
        assert_eq!(runs[0].record.as_ref().unwrap(), &rec);
        assert_eq!(runs[0].out(), dir.join(OUT));
        assert_eq!(rec.inputs[0].path_sha256.len(), 64);
    }

    #[test]
    fn missing_or_foreign_records_are_reported_per_run() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().join(DIR);
        std::fs::create_dir_all(base.join("bare")).unwrap();
        std::fs::create_dir_all(base.join("future")).unwrap();
        std::fs::write(
            base.join("future").join(RECORD),
            br#"{"format":"cv-import-run","schema_version":9}"#,
        )
        .unwrap();
        let runs = scan(d.path()).unwrap();
        assert_eq!(runs.len(), 2);
        assert!(runs.iter().all(|r| r.record.is_err()));
        assert!(
            runs[1]
                .record
                .as_ref()
                .unwrap_err()
                .contains("schema_version 9")
        );
    }

    #[test]
    fn no_imports_directory_scans_as_no_runs_without_writing() {
        let d = tempfile::tempdir().unwrap();
        assert!(scan(d.path()).unwrap().is_empty());
        assert!(!d.path().join(".cv").exists());
    }

    #[test]
    fn run_ids_are_one_plain_segment() {
        let d = tempfile::tempdir().unwrap();
        for bad in ["", ".", "..", "a/b", "a\\b"] {
            let rec = RunRecord::started(bad, "hugo", lab(), 0);
            assert!(
                matches!(write(d.path(), &rec), Err(Error::RunId(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn review_states_persist_last_write_wins() {
        let d = tempfile::tempdir().unwrap();
        let (mut log, got, _) = ReviewLog::open(d.path()).unwrap();
        assert!(got.is_empty());
        log.append("r1", "a", ReviewState::Accepted).unwrap();
        log.append("r1", "b", ReviewState::Deferred).unwrap();
        log.append("r1", "a", ReviewState::Open).unwrap();
        drop(log);
        let mut f = OpenOptions::new()
            .append(true)
            .open(d.path().join(DIR).join(REVIEW))
            .unwrap();
        write!(f, "{{\"torn").unwrap();
        drop(f);
        let (_, got, scan) = ReviewLog::open(d.path()).unwrap();
        assert!(scan.torn_bytes > 0);
        assert_eq!(
            got,
            vec![
                (("r1".into(), "a".into()), ReviewState::Open),
                (("r1".into(), "b".into()), ReviewState::Deferred),
            ]
        );
        // The stamp and log are not run directories.
        assert!(super::scan(d.path()).unwrap().is_empty());
    }

    #[test]
    fn item_ids_are_short_and_path_free() {
        let id = item_id(&["page", "content/posts/a.md"]);
        assert_eq!(id.len(), 16);
        assert!(!id.contains('/'));
        assert_ne!(id, item_id(&["page", "content/posts/b.md"]));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_imports_dir_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".cv")).unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), d.path().join(DIR)).unwrap();
        assert!(matches!(scan(d.path()), Err(Error::Symlink(_))));
        assert!(matches!(ReviewLog::open(d.path()), Err(Error::Symlink(_))));
    }
}
