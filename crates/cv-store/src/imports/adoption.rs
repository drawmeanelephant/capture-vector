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
pub const ADOPTION_SCHEMA_VERSION: u32 = 3;

/// Which workspace object an in-place asset-filename operation touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetOperationKind {
    Asset,
    Markdown,
}

/// What the transformation did with one lab-evidence row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetOperationStatus {
    Applied,
    Resumed,
    SkippedCollision,
    Rejected,
    Unchanged,
}

/// Truthful per-operation provenance for an in-place asset-filename
/// transformation. Paths are content-input-relative, matching the lab
/// manifests; `sha256` is the final content hash for applied writes and the
/// source evidence hash for untouched asset rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetOperationProvenance {
    pub kind: AssetOperationKind,
    pub status: AssetOperationStatus,
    pub source: String,
    pub target: String,
    pub reason: String,
    pub sha256: Option<String>,
    pub bytes: u64,
    pub rewrites: u64,
}

/// The recovery representation of one write in an in-place asset
/// transformation. Paths are workspace-relative because recovery executes
/// filesystem operations; hashes bound both the required pre-state and the
/// converged post-state without copying page bodies into `.cv/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingAssetOperation {
    pub kind: AssetOperationKind,
    pub source: String,
    pub target: String,
    pub source_sha256: String,
    pub source_bytes: u64,
    pub target_sha256: String,
    pub target_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptedPage {
    /// The lab's entity id for the emitted page.
    pub entity_id: String,
    /// Page path relative to the target workspace's content input.
    pub path: String,
    /// Source path exactly as the lab report names it, when it does.
    pub source: Option<String>,
}

/// One adoption event. Version-1 sidecars represented exactly one event
/// in the top-level record; version 2 keeps this list so repeated merges
/// of one run never erase the target/time evidence of an earlier adoption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptionEvent {
    pub at_ms: i64,
    pub workspace: String,
    /// `new` or `existing`.
    pub target: String,
    pub content_subfolder: Option<String>,
    pub pages: Vec<AdoptedPage>,
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
    /// Union of all adopted pages, retained as the cheap lookup projection.
    pub pages: Vec<AdoptedPage>,
    /// Every adoption event, oldest first. Empty only in a caller-built
    /// record before [`write_adoption`] normalizes it.
    #[serde(default)]
    pub events: Vec<AdoptionEvent>,
    /// Per-operation evidence for an in-place asset-filename
    /// transformation. Empty for foreign-tree copy adoptions.
    #[serde(default)]
    pub asset_operations: Vec<AssetOperationProvenance>,
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
            events: Vec::new(),
            asset_operations: Vec::new(),
        }
    }

    /// The single event represented by this record's top-level target
    /// fields. Version-1 records are normalized through this projection.
    pub fn event(&self) -> AdoptionEvent {
        AdoptionEvent {
            at_ms: self.at_ms,
            workspace: self.workspace.clone(),
            target: self.target.clone(),
            content_subfolder: self.content_subfolder.clone(),
            pages: self.pages.clone(),
        }
    }

    fn normalized(mut self) -> Self {
        self.schema_version = ADOPTION_SCHEMA_VERSION;
        if self.events.is_empty() {
            // Version 1 is exactly one event stored in the top-level fields.
            let event = self.event();
            self.events.push(event);
        } else {
            // In version 2 the event list is authoritative and the
            // top-level target/page fields are a derived compatibility
            // projection. Rebuild that projection rather than trusting two
            // potentially contradictory copies of the same evidence.
            let mut pages = Vec::new();
            for event in &self.events {
                for page in &event.pages {
                    if !pages.contains(page) {
                        pages.push(page.clone());
                    }
                }
            }
            self.pages = pages;
            if let Some(last) = self.events.last() {
                self.at_ms = last.at_ms;
                self.workspace = last.workspace.clone();
                self.target = last.target.clone();
                self.content_subfolder = last.content_subfolder.clone();
            }
        }
        self
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
    refuse_links(&[&root.join(".cv"), &root.join(DIR), dir, &path])?;
    sidecar::ensure(root, &SCHEMA).map_err(Error::Schema)?;
    let rec = rec.clone().normalized();
    let mut bytes = serde_json::to_vec_pretty(&rec).map_err(Error::Json)?;
    bytes.push(b'\n');
    write_atomic_in_run(dir, ADOPTION, &bytes)?;
    Ok(path)
}

/// The durable adoption sidecar for one run. Like the recovery journal,
/// malformed or foreign evidence is an explicit error rather than
/// something a later adoption may silently overwrite.
pub fn read_adoption_record(root: &Path, run: &str) -> Result<Option<AdoptionRecord>, String> {
    let dir = run_dir(root, run).map_err(|e| e.to_string())?;
    let path = dir.join(ADOPTION);
    refuse_links(&[&root.join(".cv"), &root.join(DIR), &dir, &path]).map_err(|e| e.to_string())?;
    if !path.exists() {
        return Ok(None);
    }
    let rec = read_adoption(&path)?;
    if rec.run_id != run {
        return Err(format!(
            "{ADOPTION} names run {:?} but lies in {run:?}'s directory",
            rec.run_id
        ));
    }
    Ok(Some(rec))
}

fn read_adoption(path: &Path) -> Result<AdoptionRecord, String> {
    if is_link(path).map_err(|e| e.to_string())? {
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
    if !(1..=u64::from(ADOPTION_SCHEMA_VERSION)).contains(&h.schema_version) {
        return Err(format!(
            "{ADOPTION} schema_version {} is not supported (this build reads 1..={ADOPTION_SCHEMA_VERSION})",
            h.schema_version
        ));
    }
    let rec: AdoptionRecord =
        serde_json::from_slice(&bytes).map_err(|e| format!("{ADOPTION} is not valid: {e}"))?;
    Ok(rec.normalized())
}

/// Valid adoption records under `.cv/imports/`, sorted by run id. Missing
/// records simply mean the run has not been adopted; undecodable records
/// are skipped here and stay visible in the run's ordinary scan problem.
pub fn scan_adoptions(root: &Path) -> Result<Vec<AdoptionRecord>, Error> {
    let base = root.join(DIR);
    refuse_links(&[&root.join(".cv"), &base])?;
    let entries = match std::fs::read_dir(&base) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for e in entries {
        let e = e?;
        if !entry_is_real_dir(&e)? {
            continue;
        }
        let path = e.path().join(ADOPTION);
        if is_link(&path)? {
            return Err(Error::Symlink(path));
        }
        if !path.exists() {
            continue;
        }
        if let Ok(rec) = read_adoption(&path)
            && e.file_name().to_str() == Some(rec.run_id.as_str())
        {
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
pub const PENDING_SCHEMA_VERSION: u32 = 3;

/// Furthest completed phase of an in-place asset-filename transformation.
/// Progress only ever advances: a recovery attempt re-runs earlier phases
/// idempotently but never records a smaller state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformationPhase {
    /// The journaled plan exists; no backup, link, write, or unlink ran.
    Planned,
    BackupsLinked,
    DestinationsLinked,
    MarkdownWritten,
    SourcesUnlinked,
    Verified,
    /// The adoption sidecar and run record are durable; only journal
    /// cleanup remains.
    Recorded,
}

/// One journaled asset relocation. `source`/`dest` are content-input
/// relative; `backup` is workspace-relative inside `.cv/imports/<run>/`.
/// The move is a hard-link chain: backup and dest both link the journaled
/// source identity, so `identity` + `sha256` pin every hop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedAssetMove {
    pub source: String,
    pub dest: String,
    pub backup: String,
    pub sha256: String,
    pub bytes: u64,
    pub identity: FileIdentity,
}

/// One journaled in-place Markdown rewrite. `page`/`temp` are
/// content-input relative; `temp` is a sibling path only this
/// transformation may create. `pre_*` comes from the run-time input
/// inventory, `post_*` from the lab candidate. `staged_identity` is filled
/// while the phase runs so a crash-left temp is recognized as ours rather
/// than deleted as foreign data; an unjournaled temp is ambiguous foreign
/// evidence and refuses recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedMarkdown {
    pub page: String,
    pub temp: String,
    pub pre_sha256: String,
    pub pre_bytes: u64,
    pub pre_identity: FileIdentity,
    pub post_sha256: String,
    pub post_bytes: u64,
    #[serde(default)]
    pub staged_identity: Option<FileIdentity>,
}

/// One dependency component: a set of asset moves and Markdown writes
/// that must land coherently or not at all. Collision-bound components
/// never reach this list — they are provenance, not plans.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedComponent {
    pub asset_moves: Vec<PlannedAssetMove>,
    pub markdown: Vec<PlannedMarkdown>,
}

/// The version-3 snapshot journal for an in-place asset-filename
/// transformation: every external fact the recovery attempt is allowed to
/// trust, bound at journal time. Workspace token/file/lock evidence ties
/// the journal to the workspace instance that wrote it; evidence digests
/// pin the lab documents and Capture Vector's input inventory; the
/// embedded inventory and component plans are the whole authorized
/// operation. Anything on disk that is neither this inventory nor these
/// plans is foreign and refuses recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingTransformation {
    pub phase: TransformationPhase,
    pub workspace_token: String,
    pub workspace_file: Option<FileIdentity>,
    pub mutation_lock: Option<FileIdentity>,
    pub report_sha256: String,
    pub manifest_sha256: String,
    pub rewrite_sha256: String,
    /// Digest over the sorted `(path, sha256, bytes)` candidate file list.
    pub candidate_sha256: String,
    /// sha256 of the `input-inventory.json` file bytes.
    pub inventory_sha256: String,
    pub inventory: Vec<InputEntry>,
    pub components: Vec<PlannedComponent>,
}

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
    /// Operation rows for an in-place asset-filename transformation.
    /// `Some` marks the journal as operation-based; `None` is the older
    /// foreign-tree copy recovery unit. Version-1 journals must decode as
    /// `None` even if a foreign hand added this field. Superseded by
    /// `transformation`: the version-3 schema keeps the field so old
    /// journals still decode, but nothing new writes it.
    #[serde(default)]
    pub asset_operations: Option<Vec<PendingAssetOperation>>,
    /// Version-3 snapshot journal for an in-place asset-filename
    /// transformation. `None` for copy-tree adoptions and every journal
    /// predating the snapshot schema; journals older than version 3 must
    /// decode as `None` even if a foreign hand added this field.
    #[serde(default)]
    pub transformation: Option<PendingTransformation>,
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
            asset_operations: None,
            transformation: None,
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
    refuse_links(&[&root.join(".cv"), &root.join(DIR), dir, &path])?;
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
    refuse_links(&[&root.join(".cv"), &root.join(DIR), &dir, &path]).map_err(|e| e.to_string())?;
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
    if !(1..=u64::from(PENDING_SCHEMA_VERSION)).contains(&h.schema_version) {
        return Err(format!(
            "{PENDING} schema_version {} is not supported (this build reads 1..={PENDING_SCHEMA_VERSION})",
            h.schema_version
        ));
    }
    let parsed: PendingAdoption =
        serde_json::from_slice(&bytes).map_err(|e| format!("{PENDING} is not valid: {e}"))?;
    if h.schema_version == 1 && parsed.asset_operations.is_some() {
        return Err(format!(
            "{PENDING} schema_version 1 cannot carry asset transformation operations"
        ));
    }
    if h.schema_version < 3 && parsed.transformation.is_some() {
        return Err(format!(
            "{PENDING} schema_version {} cannot carry a transformation snapshot",
            h.schema_version
        ));
    }
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
    refuse_links(&[&root.join(".cv"), &root.join(DIR), dir, &path])?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
