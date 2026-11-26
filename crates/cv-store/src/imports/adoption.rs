//! Adoption provenance and crash-recovery records for one migration run.
//! Split from `imports.rs` to keep each authority file below the repository
//! size budget; the types remain part of the `imports` API through the
//! parent's re-export.

use super::*;

/// `adoption.json` in a run directory: the page-level link from adopted
/// Boris content back to the lab run and source input. This is Capture
/// Vector evidence, not a lab mirror; the copied lab documents remain the
/// authoritative conversion provenance.
pub const ADOPTION: &str = "adoption.json";
pub const ADOPTION_FORMAT: &str = "cv-import-adoption";
/// Version of `<run-id>/adoption.json`, independent of the run-record schema.
pub const ADOPTION_SCHEMA_VERSION: u32 = 1;

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
            schema_version: ADOPTION_SCHEMA_VERSION,
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

fn check_adoption_lock(lock: &RunLock, run_id: &str) -> Result<(), Error> {
    if run_id != lock.run_id {
        return Err(Error::LockMismatch {
            lock: lock.run_id.clone(),
            record: run_id.into(),
        });
    }
    Ok(())
}

/// Write `<run>/adoption.json` atomically inside the run locked by `lock`.
pub fn write_adoption(lock: &RunLock, rec: &AdoptionRecord) -> Result<PathBuf, Error> {
    check_adoption_lock(lock, &rec.run_id)?;
    let root = &lock.root;
    let dir = &lock.dir;
    let path = dir.join(ADOPTION);
    refuse_symlinks(&[&root.join(".cv"), &root.join(DIR), dir, &path])?;
    sidecar::ensure(root, &SCHEMA).map_err(Error::Schema)?;
    let mut bytes = serde_json::to_vec_pretty(rec).map_err(Error::Json)?;
    bytes.push(b'\n');
    write_atomic_in_run(dir, ADOPTION, &bytes)?;
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
    if h.schema_version != u64::from(ADOPTION_SCHEMA_VERSION) {
        return Err(format!(
            "{ADOPTION} schema_version {} is not supported (this build reads {ADOPTION_SCHEMA_VERSION})",
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
/// Version of `<run-id>/adoption-pending.json`, independent of the run-record schema.
pub const PENDING_SCHEMA_VERSION: u32 = 1;

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
    /// Byte evidence of every file the starter tree held when the journal
    /// landed, workspace-relative like [`PendingFile::target`]. Only files
    /// proven byte-identical to this manifest may ever be deleted by the
    /// starter replacement; anything else is foreign and refuses the
    /// resume. Empty for existing-workspace merges (nothing is replaced)
    /// and for journals written before the manifest existed — in which
    /// case nothing may be deleted either, because there is no proof.
    #[serde(default)]
    pub starter: Vec<PendingFile>,
}

impl PendingAdoption {
    pub fn new(
        run_id: impl Into<String>,
        target: impl Into<String>,
        content_input: impl Into<String>,
    ) -> Self {
        Self {
            format: PENDING_FORMAT.into(),
            schema_version: PENDING_SCHEMA_VERSION,
            run_id: run_id.into(),
            target: target.into(),
            content_input: content_input.into(),
            content_subfolder: None,
            files: Vec::new(),
            starter: Vec::new(),
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

/// Write the recovery unit atomically inside the run locked by `lock`.
pub fn write_pending(lock: &RunLock, rec: &PendingAdoption) -> Result<PathBuf, Error> {
    check_adoption_lock(lock, &rec.run_id)?;
    let root = &lock.root;
    let dir = &lock.dir;
    let path = dir.join(PENDING);
    refuse_symlinks(&[&root.join(".cv"), &root.join(DIR), dir, &path])?;
    sidecar::ensure(root, &SCHEMA).map_err(Error::Schema)?;
    let mut bytes = serde_json::to_vec_pretty(rec).map_err(Error::Json)?;
    bytes.push(b'\n');
    write_atomic_in_run(dir, PENDING, &bytes)?;
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
    if h.schema_version != u64::from(PENDING_SCHEMA_VERSION) {
        return Err(format!(
            "{PENDING} schema_version {} is not supported (this build reads {PENDING_SCHEMA_VERSION})",
            h.schema_version
        ));
    }
    let parsed: PendingAdoption =
        serde_json::from_slice(&bytes).map_err(|e| format!("{PENDING} is not valid: {e}"))?;
    // The journal lives inside the run's own directory; if the embedded
    // run id names anything else, the file is foreign or tampered and
    // must never drive a recovery decision for this run.
    if parsed.run_id != run {
        return Err(format!(
            "{PENDING} names run {:?} but lies in {run:?}'s directory",
            parsed.run_id
        ));
    }
    Ok(Some(parsed))
}

/// Delete the recovery unit after the adoption it covers is durable. The
/// run lock proves this deletion belongs to the same adoption operation.
pub fn clear_pending(lock: &RunLock) -> Result<(), Error> {
    let root = &lock.root;
    let dir = &lock.dir;
    let path = dir.join(PENDING);
    refuse_symlinks(&[&root.join(".cv"), &root.join(DIR), dir, &path])?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
