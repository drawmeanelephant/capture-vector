//! `.cv/imports/` — one directory per `boris-migration-lab` run:
//!
//! - `<run-id>/run.json`: what Capture Vector asked the lab to do and how
//!   the process ended ([`RunRecord`]), written before the spawn and
//!   rewritten when it finishes;
//! - `<run-id>/run.lock`: the per-run write capability. Every Capture
//!   Vector write inside the run directory borrows it; the file itself is
//!   never deleted or truncated;
//! - `<run-id>/stderr.log`: raw process stderr bytes when a process ended;
//!   `run.json` carries only their length and sha256;
//! - `<run-id>/out/`: the lab's `--out`, exactly as the lab wrote it;
//! - `review.lock` + `review.jsonl`: the triage lifecycle of review items (the lab's
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
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const DIR: &str = ".cv/imports";
pub const RECORD: &str = "run.json";
pub const OUT: &str = "out";
pub const REVIEW: &str = "review.jsonl";
pub const REVIEW_LOCK: &str = "review.lock";
pub const RUN_LOCK: &str = "run.lock";
pub const STDERR_LOG: &str = "stderr.log";
/// `format` of a [`RunRecord`].
pub const RECORD_FORMAT: &str = "cv-import-run";
/// Version of `<run-id>/run.json`. Version 2 moves stderr evidence out of
/// the JSON record; version-1 records are refused explicitly rather than
/// silently losing fields during a rewrite.
pub const RECORD_SCHEMA_VERSION: u32 = 2;
/// Version of the imports namespace layout. This is intentionally
/// independent of both the run-record and review-record schemas.
pub const NAMESPACE_SCHEMA_VERSION: u32 = 1;
/// Version of one line in `review.jsonl`.
pub const REVIEW_SCHEMA_VERSION: u32 = 1;
pub const SCHEMA: Namespace = Namespace {
    name: "imports",
    dir: DIR,
    current: NAMESPACE_SCHEMA_VERSION,
    migrations: &[ADOPT_UNSTAMPED],
};

/// One lab input as the run used it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedInput {
    /// The lab flag, e.g. `--root`.
    pub flag: String,
    /// Validated absolute path spelling the lab was given; not a canonical
    /// identity.
    pub path: String,
    /// sha256 of `path` (the path string, not the content).
    pub path_sha256: String,
}

/// The lab binary that ran, as its `--version` gate saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedLab {
    pub tool_id: String,
    /// How it was located (`PATH`, `CV_MIGRATION_LAB_BIN`, …).
    pub source: String,
    pub path: String,
}

/// Terminal classes Capture Vector may record. This is broader than the
/// lab's own exit classes because `cancelled` is an app-side process outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "lowercase")]
pub enum FinishedExit {
    Ok,
    Usage,
    Io,
    Cancelled,
}

impl FinishedExit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Usage => "usage",
            Self::Io => "io",
            Self::Cancelled => "cancelled",
        }
    }
}

impl std::fmt::Display for FinishedExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finished {
    /// `None` when the process ended by signal.
    pub exit_code: Option<i32>,
    pub exit: FinishedExit,
    /// How the process ended, e.g. `exited 0` or `signal 9`.
    pub outcome: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Adopted {
    pub at_ms: i64,
    pub workspace: String,
}

/// `<run-id>/run.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// The adapter or lab refused the run, in plain language.
    pub refused: Option<String>,
    /// sha256 of raw `<run>/stderr.log` bytes, when process evidence exists.
    pub stderr_sha256: Option<String>,
    /// Byte length of raw `<run>/stderr.log`; the bytes themselves stay out
    /// of the JSON record so scanning does not decode or lossy-convert them.
    pub stderr_bytes: Option<u64>,
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
            schema_version: RECORD_SCHEMA_VERSION,
            run_id: run_id.into(),
            mode: mode.into(),
            inputs: Vec::new(),
            argv: Vec::new(),
            lab,
            boris_pin: None,
            started_ms,
            finished: None,
            refused: None,
            stderr_sha256: None,
            stderr_bytes: None,
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

fn sha256_hex(bytes: &[u8]) -> String {
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
    /// A run-id claim lost to an existing file, directory or symlink.
    RunExists(PathBuf),
    /// Adoption can only lock an existing run directory.
    RunMissing(PathBuf),
    /// The existing run-id path is not a directory.
    NotDirectory(PathBuf),
    /// Another live writer holds this lock.
    LockHeld(PathBuf),
    /// A write was attempted with a lock for another run.
    LockMismatch {
        lock: String,
        record: String,
    },
    Json(serde_json::Error),
    Schema(SidecarError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Symlink(p) => write!(f, "refusing symlinked {}", p.display()),
            Error::RunId(id) => write!(f, "run id {id:?} must be one plain path segment"),
            Error::RunExists(p) => write!(
                f,
                "run id path {} already exists; this import was not started",
                p.display()
            ),
            Error::RunMissing(p) => write!(f, "run directory {} does not exist", p.display()),
            Error::NotDirectory(p) => write!(f, "run path {} is not a directory", p.display()),
            Error::LockHeld(p) => write!(
                f,
                "lock acquisition failed because the operation would block: {}",
                p.display()
            ),
            Error::LockMismatch { lock, record } => {
                write!(f, "run lock {lock:?} cannot write a record for {record:?}")
            }
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

/// A held exclusive lock on one run directory's `run.lock`.
///
/// This is a capability, not a cache: the file stays open exactly as long as
/// the operation that claimed or adopted the run. Construction is possible
/// only through [`claim_run`] or [`lock_run`], and run writes borrow it, so
/// the type system makes writes outside a held operation unrepresentable.
#[derive(Debug)]
pub struct RunLock {
    root: PathBuf,
    run_id: String,
    dir: PathBuf,
    _file: File,
}

impl RunLock {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// What [`probe_run`] observed without creating anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunProbe {
    /// A live writer holds `run.lock`.
    Held,
    /// `run.lock` exists and can be locked. The probe releases it at once.
    Free,
    /// The run directory or its lock file does not exist.
    Missing,
}

fn lock_exclusive(file: &File, path: &Path) -> Result<(), Error> {
    crate::lock::try_lock_settled(file).map_err(|e| match e {
        TryLockError::WouldBlock => Error::LockHeld(path.to_path_buf()),
        TryLockError::Error(e) => Error::Io(e),
    })
}

fn ensure_imports_schema(root: &Path) -> Result<(), Error> {
    let cv = root.join(".cv");
    let base = root.join(DIR);
    refuse_symlinks(&[&cv, &base])?;
    sidecar::ensure(root, &SCHEMA).map_err(Error::Schema)?;
    // The namespace directory is created by `ensure`; re-check after that
    // write window rather than following a swapped symlink onwards.
    refuse_symlinks(&[&cv, &base])?;
    Ok(())
}

/// Claim a fresh run id for one import operation.
///
/// The atomic correctness mechanism is `create_dir`: if two windows choose
/// the same id exactly one creates `<run-id>`, and the loser writes no
/// record. The winner then creates `run.lock` exclusively and locks it for
/// the operation lifetime. Failure after the directory creation is
/// fail-closed: the empty claim stays behind rather than letting another
/// writer reuse ambiguous output.
pub fn claim_run(root: &Path, run_id: &str) -> Result<RunLock, Error> {
    let dir = run_dir(root, run_id)?;
    let cv = root.join(".cv");
    let base = root.join(DIR);
    refuse_symlinks(&[&cv, &base, &dir])?;
    ensure_imports_schema(root)?;
    if is_symlink(&dir)? {
        return Err(Error::Symlink(dir));
    }
    if let Err(e) = std::fs::create_dir(&dir) {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            if is_symlink(&dir)? {
                return Err(Error::Symlink(dir));
            }
            return Err(Error::RunExists(dir));
        }
        return Err(e.into());
    }
    let lock_path = dir.join(RUN_LOCK);
    refuse_symlinks(&[&lock_path])?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&lock_path)?;
    lock_exclusive(&file, &lock_path)?;
    Ok(RunLock {
        root: root.to_path_buf(),
        run_id: run_id.into(),
        dir,
        _file: file,
    })
}

/// Lock an existing run for a future adoption operation.
///
/// Runs written before per-run locks existed may not have `run.lock` yet;
/// adoption creates that lock file exclusively inside the already-existing
/// run directory and then locks it. The run directory itself is never
/// created here, and a symlinked lock path is refused rather than followed.
pub fn lock_run(root: &Path, run_id: &str) -> Result<RunLock, Error> {
    let dir = run_dir(root, run_id)?;
    let lock_path = dir.join(RUN_LOCK);
    refuse_symlinks(&[&root.join(".cv"), &root.join(DIR), &dir, &lock_path])?;
    let meta = match std::fs::symlink_metadata(&dir) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::RunMissing(dir));
        }
        Err(e) => return Err(e.into()),
    };
    if !meta.is_dir() {
        return Err(Error::NotDirectory(dir));
    }
    let file = crate::open::open_create_no_follow(&lock_path)?;
    refuse_symlinks(&[&lock_path])?;
    lock_exclusive(&file, &lock_path)?;
    Ok(RunLock {
        root: root.to_path_buf(),
        run_id: run_id.into(),
        dir,
        _file: file,
    })
}

/// Observe a run lock without creating or modifying anything.
///
/// A successful `Free` probe is released before returning. Lock-open or
/// lock-read errors are errors rather than a fail-open "free" answer, so a
/// caller that needs the lock must refuse the operation.
pub fn probe_run(root: &Path, run_id: &str) -> Result<RunProbe, Error> {
    let dir = run_dir(root, run_id)?;
    let lock_path = dir.join(RUN_LOCK);
    refuse_symlinks(&[&root.join(".cv"), &root.join(DIR), &dir, &lock_path])?;
    match std::fs::symlink_metadata(&dir) {
        Ok(m) if m.file_type().is_symlink() => return Err(Error::Symlink(dir)),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(RunProbe::Missing),
        Err(e) => return Err(e.into()),
    }
    match std::fs::symlink_metadata(&lock_path) {
        Ok(m) if m.file_type().is_symlink() => return Err(Error::Symlink(lock_path)),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(RunProbe::Missing),
        Err(e) => return Err(e.into()),
    }
    // Windows lock acquisition may require a write-capable handle. The open
    // is non-creating, non-truncating, and final-component no-follow on every
    // platform; mutation is impossible because the probe only lock-tests and
    // closes the handle.
    let file = match crate::open::open_existing_no_follow(&lock_path, cfg!(windows)) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(RunProbe::Missing),
        Err(e) => return Err(e.into()),
    };
    match file.try_lock() {
        Ok(()) => Ok(RunProbe::Free),
        Err(TryLockError::WouldBlock) => Ok(RunProbe::Held),
        Err(TryLockError::Error(e)) => Err(e.into()),
    }
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_tmp(dir: &Path, target: &str) -> PathBuf {
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(".{target}.tmp-{}-{n}", std::process::id()))
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> Result<(), Error> {
    File::open(dir)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> Result<(), Error> {
    // Windows directory fsync is not represented by opening the directory as
    // a std File. Atomic rename still gives process-crash atomicity there;
    // power-loss durability is platform-dependent and not claimed.
    Ok(())
}

/// Write bytes through a unique temporary sibling, fsync the file, then
/// rename. This is process-crash atomic; on Unix the directory is fsynced
/// too. Each temporary name is unique and created exclusively; only a temp
/// file created by this call is removed after a failed write, while fixed
/// foreign `*.tmp` entries are never deletion or truncation vectors.
fn write_atomic_in_run(dir: &Path, target: &str, bytes: &[u8]) -> Result<(), Error> {
    let path = dir.join(target);
    refuse_symlinks(&[&path])?;
    for _ in 0..100 {
        let tmp = unique_tmp(dir, target);
        let mut f = match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        };
        let written = f.write_all(bytes).and_then(|()| f.sync_all());
        drop(f);
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        sync_dir(dir)?;
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("could not allocate a unique temporary name for {target}"),
    )
    .into())
}

/// Write raw process stderr bytes inside the locked run and return
/// `(byte_len, sha256)` metadata for `run.json`.
pub fn write_stderr(lock: &RunLock, bytes: &[u8]) -> Result<(u64, String), Error> {
    refuse_symlinks(&[
        &lock.root.join(".cv"),
        &lock.root.join(DIR),
        &lock.dir,
        &lock.dir.join(STDERR_LOG),
    ])?;
    sidecar::ensure(&lock.root, &SCHEMA).map_err(Error::Schema)?;
    write_atomic_in_run(&lock.dir, STDERR_LOG, bytes)?;
    Ok((bytes.len() as u64, sha256_hex(bytes)))
}

/// Write (or rewrite) `record` atomically inside the run locked by `lock`;
/// returns the run directory. The lock is both the operation lifetime proof
/// and the run identity, so a caller cannot accidentally write one run's
/// record while holding another run's lock. Atomicity is a process-crash
/// boundary, not a claim that every platform survives power loss.
pub fn write(lock: &RunLock, record: &RunRecord) -> Result<PathBuf, Error> {
    if record.run_id != lock.run_id {
        return Err(Error::LockMismatch {
            lock: lock.run_id.clone(),
            record: record.run_id.clone(),
        });
    }
    let root = &lock.root;
    let dir = &lock.dir;
    let path = dir.join(RECORD);
    refuse_symlinks(&[&root.join(".cv"), &root.join(DIR), dir, &path])?;
    sidecar::ensure(root, &SCHEMA).map_err(Error::Schema)?;
    let mut bytes = serde_json::to_vec_pretty(record).map_err(Error::Json)?;
    bytes.push(b'\n');
    write_atomic_in_run(dir, RECORD, &bytes)?;
    Ok(dir.clone())
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
    if h.schema_version != u64::from(RECORD_SCHEMA_VERSION) {
        return Err(format!(
            "{RECORD} schema_version {} is not supported (this build reads {RECORD_SCHEMA_VERSION})",
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

/// `.cv/imports/review.jsonl`, append-only. Opening is pure observation:
/// it never creates the imports directory, lock, or log and never repairs
/// or truncates a torn tail. Each append is an explicit operation-scoped
/// mutation: create/acquire `review.lock`, append one complete record (first
/// terminating an undecodable partial tail with a newline when necessary),
/// fsync, and release the lock when the operation ends. No window holds a
/// session-long review writer capability.
pub struct ReviewLog {
    root: PathBuf,
    path: PathBuf,
}

fn open_review_lock_for_append(lock_path: &Path) -> Result<File, Error> {
    refuse_symlinks(&[lock_path])?;
    let file = crate::open::open_create_no_follow(lock_path)?;
    refuse_symlinks(&[lock_path])?;
    Ok(file)
}

fn parse_review_states(buf: &[u8], good: usize, scan: &mut Scan) -> ReviewStates {
    let mut out: ReviewStates = Vec::new();
    let mut at: HashMap<(String, String), usize> = HashMap::new();
    for line in buf[..good].split(|b| *b == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        scan.lines += 1;
        let rec = match serde_json::from_slice::<ReviewRecord>(line) {
            Ok(r) if r.schema_version == REVIEW_SCHEMA_VERSION => r,
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
    out
}

impl ReviewLog {
    /// Open the review sidecar read-only. A missing log is an empty log;
    /// opening itself creates nothing. Bytes after the final newline are
    /// reported as torn and ignored rather than repaired.
    pub fn open(root: &Path) -> Result<(Self, ReviewStates, Scan), Error> {
        let base = root.join(DIR);
        let path = base.join(REVIEW);
        let lock_path = base.join(REVIEW_LOCK);
        refuse_symlinks(&[&root.join(".cv"), &base, &path, &lock_path])?;
        let buf = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        let good = buf.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        let mut scan = Scan {
            torn_bytes: (buf.len() - good) as u64,
            ..Scan::default()
        };
        let states = parse_review_states(&buf, good, &mut scan);
        Ok((
            Self {
                root: root.to_path_buf(),
                path,
            },
            states,
            scan,
        ))
    }

    /// Append one triage transition under a lock held only for this
    /// operation. Concurrent windows may both append in sequence; their
    /// last-write-per-item data model remains explicit, while a lock that
    /// stays contested beyond the bounded acquisition attempt refuses the
    /// operation instead of writing unlocked.
    pub fn append(&mut self, run: &str, item: &str, to: ReviewState) -> Result<(), Error> {
        let base = self.root.join(DIR);
        let lock_path = base.join(REVIEW_LOCK);
        refuse_symlinks(&[&self.root.join(".cv"), &base, &self.path, &lock_path])?;
        // Schema/imports creation is part of this explicit mutation, never
        // part of opening or scanning. The schema lock serializes its own
        // stamp independently of this review operation lock.
        ensure_imports_schema(&self.root)?;
        refuse_symlinks(&[&self.root.join(".cv"), &base, &self.path, &lock_path])?;
        let lock_file = open_review_lock_for_append(&lock_path)?;
        lock_exclusive(&lock_file, &lock_path)?;
        refuse_symlinks(&[&self.root.join(".cv"), &base, &self.path, &lock_path])?;
        let mut file = crate::open::open_create_no_follow(&self.path)?;
        let len = file.metadata()?.len();
        if len > 0 {
            file.seek(SeekFrom::End(-1))?;
            let mut last = [0u8; 1];
            file.read_exact(&mut last)?;
            file.seek(SeekFrom::End(0))?;
            if last[0] != b'\n' {
                // Do not truncate or rewrite foreign bytes. Terminating the
                // partial line keeps it visible as undecodable evidence while
                // preventing the new JSON record from concatenating with it.
                file.write_all(b"\n")?;
            }
        } else {
            file.seek(SeekFrom::End(0))?;
        }
        let rec = ReviewRecord {
            schema_version: REVIEW_SCHEMA_VERSION,
            run: run.into(),
            item: item.into(),
            to,
        };
        let mut line = serde_json::to_vec(&rec).map_err(Error::Json)?;
        line.push(b'\n');
        file.write_all(&line)?;
        file.sync_data()?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
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

    fn record(id: &str) -> RunRecord {
        RunRecord::started(id, "hugo", lab(), 5)
    }

    fn legacy_run(root: &Path, id: &str) -> PathBuf {
        let dir = run_dir(root, id).unwrap();
        std::fs::create_dir_all(dir.join(OUT)).unwrap();
        let mut bytes = serde_json::to_vec_pretty(&record(id)).unwrap();
        bytes.push(b'\n');
        std::fs::write(dir.join(RECORD), bytes).unwrap();
        dir
    }

    #[test]
    fn a_written_record_scans_back_beside_its_output() {
        let d = tempfile::tempdir().unwrap();
        let lock = claim_run(d.path(), "r1").unwrap();
        let mut rec = record("r1");
        rec.inputs.push(input("--root", "/src/site"));
        let dir = write(&lock, &rec).unwrap();
        std::fs::create_dir_all(dir.join(OUT)).unwrap();
        rec.finished = Some(Finished {
            exit_code: Some(0),
            exit: FinishedExit::Ok,
            outcome: "exited 0".into(),
            duration_ms: 12,
        });
        write(&lock, &rec).unwrap();
        let runs = scan(d.path()).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, "r1");
        assert_eq!(runs[0].record.as_ref().unwrap(), &rec);
        assert_eq!(runs[0].out(), dir.join(OUT));
        assert_eq!(rec.inputs[0].path_sha256.len(), 64);
    }

    #[test]
    fn two_thread_claims_for_one_run_id_have_exactly_one_writer() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut threads = Vec::new();
        for _ in 0..2 {
            let root = root.clone();
            let barrier = barrier.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                claim_run(&root, "same")
            }));
        }
        let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, Err(Error::RunExists(_))))
                .count(),
            1,
            "results: {results:?}"
        );
        let winner = results.into_iter().find_map(Result::ok).unwrap();
        assert!(!winner.dir().join(RECORD).exists());
        write(&winner, &record("same")).unwrap();
        assert_eq!(scan(d.path()).unwrap().len(), 1);
    }

    #[test]
    fn preexisting_file_and_directory_run_ids_are_refused_without_a_write() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().join(DIR);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("file-id"), b"foreign").unwrap();
        std::fs::create_dir(base.join("dir-id")).unwrap();
        assert!(matches!(
            claim_run(d.path(), "file-id"),
            Err(Error::RunExists(_))
        ));
        assert!(matches!(
            claim_run(d.path(), "dir-id"),
            Err(Error::RunExists(_))
        ));
        assert!(!base.join("file-id").join(RUN_LOCK).exists());
        assert!(!base.join("dir-id").join(RUN_LOCK).exists());
    }

    #[cfg(unix)]
    #[test]
    fn preexisting_symlinked_run_id_and_lock_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let base = d.path().join(DIR);
        std::fs::create_dir_all(&base).unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), base.join("link-id")).unwrap();
        assert!(matches!(
            claim_run(d.path(), "link-id"),
            Err(Error::Symlink(_))
        ));
        assert!(!elsewhere.path().join(RUN_LOCK).exists());

        let dir = legacy_run(d.path(), "legacy");
        let target = elsewhere.path().join("target.lock");
        std::fs::write(&target, b"").unwrap();
        std::os::unix::fs::symlink(&target, dir.join(RUN_LOCK)).unwrap();
        assert!(matches!(
            lock_run(d.path(), "legacy"),
            Err(Error::Symlink(_))
        ));
        assert!(matches!(
            probe_run(d.path(), "legacy"),
            Err(Error::Symlink(_))
        ));
    }

    #[test]
    fn lock_run_adopts_a_legacy_lock_path_and_probe_follows_hold_release() {
        let d = tempfile::tempdir().unwrap();
        let dir = legacy_run(d.path(), "legacy");
        assert_eq!(probe_run(d.path(), "legacy").unwrap(), RunProbe::Missing);
        let lock = lock_run(d.path(), "legacy").unwrap();
        assert_eq!(probe_run(d.path(), "legacy").unwrap(), RunProbe::Held);
        write(&lock, &record("legacy")).unwrap();
        drop(lock);
        assert_eq!(probe_run(d.path(), "legacy").unwrap(), RunProbe::Free);
        assert!(
            dir.join(RUN_LOCK).is_file(),
            "the lock file is never removed"
        );
    }

    #[test]
    fn a_run_lock_cannot_write_another_runs_record() {
        let d = tempfile::tempdir().unwrap();
        let lock = claim_run(d.path(), "r1").unwrap();
        assert!(matches!(
            write(&lock, &record("r2")),
            Err(Error::LockMismatch { .. })
        ));
        assert!(!run_dir(d.path(), "r2").unwrap().exists());
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
            assert!(
                matches!(claim_run(d.path(), bad), Err(Error::RunId(_))),
                "{bad}"
            );
        }
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

    #[test]
    fn unknown_or_legacy_run_record_fields_are_refused_not_dropped() {
        let d = tempfile::tempdir().unwrap();
        let lock = claim_run(d.path(), "strict").unwrap();
        write(&lock, &record("strict")).unwrap();
        let path = lock.dir().join(RECORD);
        let original = std::fs::read(&path).unwrap();

        let mut json: serde_json::Value = serde_json::from_slice(&original).unwrap();
        json["future_field"] = serde_json::json!("foreign evidence");
        std::fs::write(&path, serde_json::to_vec_pretty(&json).unwrap()).unwrap();
        let err = scan(d.path()).unwrap()[0]
            .record
            .as_ref()
            .unwrap_err()
            .clone();
        assert!(err.contains("unknown field"), "{err}");

        let mut json: serde_json::Value = serde_json::from_slice(&original).unwrap();
        json["lab"]["future_field"] = serde_json::json!("foreign evidence");
        std::fs::write(&path, serde_json::to_vec_pretty(&json).unwrap()).unwrap();
        let err = scan(d.path()).unwrap()[0]
            .record
            .as_ref()
            .unwrap_err()
            .clone();
        assert!(err.contains("unknown field"), "{err}");

        let mut json: serde_json::Value = serde_json::from_slice(&original).unwrap();
        json["schema_version"] = serde_json::json!(1);
        std::fs::write(&path, serde_json::to_vec_pretty(&json).unwrap()).unwrap();
        let err = scan(d.path()).unwrap()[0]
            .record
            .as_ref()
            .unwrap_err()
            .clone();
        assert!(err.contains("schema_version 1 is not supported"), "{err}");
    }

    #[test]
    fn fixed_run_json_tmp_is_not_a_collision_or_deletion_vector() {
        let d = tempfile::tempdir().unwrap();
        let lock = claim_run(d.path(), "tmp").unwrap();
        let fixed = lock.dir().join(format!("{RECORD}.tmp"));
        std::fs::write(&fixed, b"foreign fixed tmp").unwrap();
        write(&lock, &record("tmp")).unwrap();
        assert_eq!(std::fs::read(&fixed).unwrap(), b"foreign fixed tmp");
        assert!(lock.dir().join(RECORD).is_file());
        let leftovers = std::fs::read_dir(lock.dir())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .count();
        assert_eq!(leftovers, 0, "successful atomic writes leave no temp files");
    }

    #[test]
    fn stderr_evidence_is_raw_bytes_with_hash_and_length_not_inline_lossy_text() {
        let d = tempfile::tempdir().unwrap();
        let lock = claim_run(d.path(), "stderr").unwrap();
        let raw = b"ok\ninvalid:\xff\xfeend\n";
        let (len, sha256) = write_stderr(&lock, raw).unwrap();
        assert_eq!(len, raw.len() as u64);
        let mut rec = record("stderr");
        rec.stderr_bytes = Some(len);
        rec.stderr_sha256 = Some(sha256.clone());
        write(&lock, &rec).unwrap();
        assert_eq!(std::fs::read(lock.dir().join(STDERR_LOG)).unwrap(), raw);
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(lock.dir().join(RECORD)).unwrap()).unwrap();
        assert!(json.get("stderr").is_none(), "inline lossy stderr is gone");
        assert_eq!(json["stderr_bytes"], serde_json::json!(raw.len()));
        assert_eq!(json["stderr_sha256"], serde_json::json!(sha256));
    }
}
