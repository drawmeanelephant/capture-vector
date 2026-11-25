//! Adoption of a finished migration-lab run into Boris content. The lab
//! output stays the candidate source of truth; this module only reads its
//! mirrored report, plans confined writes, refuses collisions before the
//! first write, copies bytes through the staged writer, and preserves the
//! lab's provenance under `.cv/imports/<run-id>/`. It never parses Boris
//! frontmatter and never decides whether the resulting graph is valid —
//! `boris validate` remains the judge.

use crate::error::EngineError;
use crate::write;
use cv_contracts::lab::Report;
use cv_core::engine::WriteOutcome;
use cv_store::imports::{self as store, AdoptedPage, AdoptionRecord, RunRecord};
use cv_store::paths::{confined, confined_file, portable_relative};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum AdoptError {
    Store(store::Error),
    Engine(EngineError),
    Io(std::io::Error),
    Run(String),
    Report(String),
    Content(String),
    Subfolder(String),
    Collisions(Vec<Collision>),
    Stale(String),
}

impl std::fmt::Display for AdoptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(e) => write!(f, "{e}"),
            Self::Engine(e) => write!(f, "{e}"),
            Self::Io(e) => write!(f, "{e}"),
            Self::Run(e) | Self::Report(e) | Self::Content(e) | Self::Subfolder(e) => {
                write!(f, "{e}")
            }
            Self::Stale(e) => write!(f, "{e}"),
            Self::Collisions(cs) => {
                write!(f, "adoption refused: ")?;
                for (i, c) in cs.iter().enumerate() {
                    if i > 0 {
                        write!(f, "; ")?;
                    }
                    write!(f, "{c}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for AdoptError {}

impl From<store::Error> for AdoptError {
    fn from(e: store::Error) -> Self {
        Self::Store(e)
    }
}
impl From<EngineError> for AdoptError {
    fn from(e: EngineError) -> Self {
        Self::Engine(e)
    }
}
impl From<std::io::Error> for AdoptError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePage {
    pub entity_id: String,
    /// Path below the candidate `content/` directory, `/`-separated.
    pub path: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentFile {
    /// Path below the candidate `content/` directory, `/`-separated.
    pub relative: String,
    pub source: PathBuf,
}

/// A finished run's adoptable output.
#[derive(Debug, Clone)]
pub struct AdoptionSource {
    pub run_id: String,
    pub mode: String,
    pub record: RunRecord,
    pub out: PathBuf,
    pub content: PathBuf,
    pub files: Vec<ContentFile>,
    pub pages: Vec<SourcePage>,
}

fn is_link(path: &Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(m) => Ok(cv_store::paths::is_link(&m)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

fn scan_files(dir: &Path, prefix: &str, out: &mut Vec<ContentFile>) -> Result<(), AdoptError> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| AdoptError::Content(format!("cannot read {}: {e}", dir.display())))?;
    for e in entries {
        let e = e?;
        let name = e.file_name().into_string().map_err(|_| {
            AdoptError::Content(format!("non-UTF-8 file name under {}", dir.display()))
        })?;
        let rel = format!("{prefix}{name}");
        let path = e.path();
        let ty = e.file_type()?;
        if ty.is_symlink() {
            return Err(AdoptError::Content(format!(
                "candidate content contains symlink {rel}"
            )));
        }
        if ty.is_dir() {
            scan_files(&path, &format!("{rel}/"), out)?;
        } else if ty.is_file() {
            portable_relative(&rel).map_err(|_| {
                AdoptError::Content(format!("candidate path {rel:?} is not portable"))
            })?;
            out.push(ContentFile {
                relative: rel,
                source: path,
            });
        }
    }
    Ok(())
}

/// Read one successful run and decode its lab report. Inventory-only and
/// cross-cutting reports have no `content/` tree and are refused here.
pub fn source(root: &Path, run_id: &str) -> Result<AdoptionSource, AdoptError> {
    let dir = store::run_dir(root, run_id)?;
    let runs = store::scan(root)?;
    let run = runs
        .into_iter()
        .find(|r| r.id == run_id)
        .ok_or_else(|| AdoptError::Run(format!("no import run {run_id:?}")))?;
    let record = run.record.map_err(AdoptError::Run)?;
    if record.refused.is_some() || record.finished.as_ref().is_none_or(|f| f.exit != "ok") {
        return Err(AdoptError::Run(format!(
            "run {run_id} has not finished successfully"
        )));
    }
    let out = dir.join(store::OUT);
    if is_link(&out)? {
        return Err(AdoptError::Content(format!(
            "{} is a symlink",
            out.display()
        )));
    }
    let report_path = out.join("report.json");
    let report_bytes = std::fs::read(&report_path)
        .map_err(|e| AdoptError::Report(format!("report.json is unreadable: {e}")))?;
    let report = Report::decode(&report_bytes).map_err(|e| AdoptError::Report(e.to_string()))?;
    let mut pages = Vec::new();
    for p in report.pages() {
        let Some(output) = p.output else { continue };
        let Some(rel) = output.strip_prefix("content/") else {
            continue;
        };
        pages.push(SourcePage {
            entity_id: p.entity_id.to_string(),
            path: rel.to_string(),
            source: p.source.map(str::to_string),
        });
    }
    let content = out.join("content");
    if is_link(&content)? || !content.is_dir() {
        return Err(AdoptError::Content(format!(
            "{} is not an ordinary directory",
            content.display()
        )));
    }
    let mut files = Vec::new();
    scan_files(&content, "", &mut files)?;
    files.sort_by(|a, b| a.relative.cmp(&b.relative));
    if files.is_empty() {
        return Err(AdoptError::Content(
            "candidate content/ has no files to adopt".into(),
        ));
    }
    Ok(AdoptionSource {
        run_id: run_id.into(),
        mode: record.mode.clone(),
        record,
        out,
        content,
        files,
        pages,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    pub source: PathBuf,
    /// Workspace-relative staged-write target, `/`-separated.
    pub target: String,
    /// Page path relative to the content input (no `content/` prefix).
    pub display: String,
}

/// Validate a merge subfolder as one portable relative path below the
/// content input. The writer revalidates the complete target; this is the
/// user-input boundary.
pub fn subfolder(raw: &str) -> Result<String, AdoptError> {
    if raw.starts_with(['/', '\\']) || raw.trim_start() != raw {
        return Err(AdoptError::Subfolder(
            "the subfolder must be relative to content/, not absolute".into(),
        ));
    }
    let s = raw.trim_end_matches('/');
    if s.is_empty() {
        return Err(AdoptError::Subfolder(
            "choose a subfolder below content/".into(),
        ));
    }
    portable_relative(s).map_err(|_| {
        AdoptError::Subfolder(
            "the subfolder must be a portable relative path without '..', drive names, or path separators in a name"
                .into(),
        )
    })?;
    Ok(s.to_string())
}

fn join_rel(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty() && **p != ".")
        .copied()
        .collect::<Vec<_>>()
        .join("/")
}

/// Plan byte-for-byte copies below `content_input` (itself workspace-relative).
pub fn plan(
    src: &AdoptionSource,
    content_input: &str,
    merge_subfolder: Option<&str>,
) -> Result<Vec<PlannedFile>, AdoptError> {
    let input = content_input.trim().trim_matches('/');
    if !input.is_empty() && input != "." {
        portable_relative(input).map_err(|_| {
            AdoptError::Content(format!("content input {content_input:?} is not portable"))
        })?;
    }
    let sub = match merge_subfolder {
        Some(s) => Some(subfolder(s)?),
        None => None,
    };
    let mut out = Vec::new();
    for f in &src.files {
        let display = join_rel(&[sub.as_deref().unwrap_or(""), &f.relative]);
        let target = join_rel(&[input, &display]);
        portable_relative(&target)
            .map_err(|_| AdoptError::Content(format!("target {target:?} is not portable")))?;
        out.push(PlannedFile {
            source: f.source.clone(),
            target,
            display,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollisionKind {
    Path,
    Id,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    pub kind: CollisionKind,
    /// The incoming path or entity id.
    pub incoming: String,
    /// The existing path or id owner.
    pub existing: String,
}

impl std::fmt::Display for Collision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            CollisionKind::Path => write!(
                f,
                "path {} already exists ({})",
                self.incoming, self.existing
            ),
            CollisionKind::Id => write!(
                f,
                "page id {} is already used by {}",
                self.incoming, self.existing
            ),
        }
    }
}

/// True when the target already holds exactly the bytes adoption would
/// write. Length is checked before content so large mismatches cost nothing.
fn bytes_identical(target: &Path, source: &Path) -> bool {
    let (Ok(t), Ok(s)) = (std::fs::read(target), std::fs::read(source)) else {
        return false;
    };
    t == s
}

/// Refuse every path and id overlap before the first write. `existing_ids`
/// is the composition root's current Boris-view of the target workspace
/// (`id` → content-input-relative path); adoption itself never parses
/// Boris frontmatter.
///
/// `resume` changes path semantics only: with a durable recovery journal,
/// a target holding exactly the incoming bytes is evidence of the
/// interrupted attempt's own staged write and is skipped, while different
/// bytes can only be foreign data and still refuse. Without the journal,
/// every existing target path is a collision — even byte-identical ones,
/// because nothing distinguishes them from the user's own pre-existing
/// content. Returns the files that still need copying in resume mode.
pub fn collisions(
    target_root: &Path,
    src: &AdoptionSource,
    planned: &[PlannedFile],
    existing_ids: &BTreeMap<String, String>,
    resume: bool,
) -> Result<Vec<bool>, AdoptError> {
    let mut out = Vec::new();
    let mut incoming_ids = BTreeSet::new();
    let mut needs_copy = Vec::new();
    for p in planned {
        let exists = confined_file(target_root, Path::new(&p.target))
            .ok()
            .is_some_and(|path| std::fs::symlink_metadata(&path).is_ok());
        let mut copy = true;
        if exists {
            let target = target_root.join(&p.target);
            if resume && bytes_identical(&target, &p.source) {
                copy = false;
            } else {
                out.push(Collision {
                    kind: CollisionKind::Path,
                    incoming: p.display.clone(),
                    existing: p.target.clone(),
                });
            }
        }
        needs_copy.push(copy);
    }
    for page in &src.pages {
        if !incoming_ids.insert(page.entity_id.clone()) {
            out.push(Collision {
                kind: CollisionKind::Id,
                incoming: page.entity_id.clone(),
                existing: "another page in the same migration run".into(),
            });
        }
        if let Some(existing) = existing_ids.get(&page.entity_id) {
            // An owner at one of the adoption's own planned paths is the
            // path preflight's case, not an id collision: fresh attempts
            // already refuse it as a path collision, and in resume mode it
            // is the interrupted attempt's own partial output — refusing
            // it here too would make every real-world retry self-refusing.
            let own = planned.iter().any(|pl| pl.display == *existing);
            if !own {
                out.push(Collision {
                    kind: CollisionKind::Id,
                    incoming: page.entity_id.clone(),
                    existing: existing.clone(),
                });
            }
        }
    }
    out.sort_by(|a, b| {
        a.incoming
            .cmp(&b.incoming)
            .then(a.existing.cmp(&b.existing))
    });
    out.dedup();
    if out.is_empty() {
        Ok(needs_copy)
    } else {
        Err(AdoptError::Collisions(out))
    }
}

/// Copy planned files byte-for-byte with `expected=None` staged writes.
/// `needs_copy[i] == false` skips `planned[i]` (a verified resume skip);
/// every written file still refuses a surprise predecessor instead of
/// overwriting it. Returns `(written, skipped)`.
pub fn copy_content(
    target_root: &Path,
    planned: &[PlannedFile],
    needs_copy: &[bool],
) -> Result<(usize, usize), AdoptError> {
    let mut written = 0;
    let mut skipped = 0;
    for (p, needed) in planned.iter().zip(needs_copy) {
        if !needed {
            skipped += 1;
            continue;
        }
        let bytes = std::fs::read(&p.source)?;
        match write::write_staged(target_root, &p.target, &bytes, None)? {
            WriteOutcome::Written { .. } => written += 1,
            WriteOutcome::StaleBase { .. } => {
                return Err(AdoptError::Stale(format!(
                    "{} appeared or changed during adoption; nothing was overwritten",
                    p.target
                )));
            }
        }
    }
    Ok((written, skipped))
}

/// Replace the freshly initialized starter's content directory. This is
/// only for a target `boris init` has just written in the same adoption
/// operation; merges never call it.
pub fn clear_new_workspace_content(
    target_root: &Path,
    content_input: &str,
) -> Result<(), AdoptError> {
    let input = content_input.trim().trim_matches('/');
    if input.is_empty() || input == "." {
        return Err(AdoptError::Content(
            "refusing to replace a whole-workspace content input".into(),
        ));
    }
    let path = confined(target_root, Path::new(input)).map_err(|e| {
        AdoptError::Content(format!("content input {input:?} is not confined: {e}"))
    })?;
    let meta = std::fs::symlink_metadata(&path)?;
    if cv_store::paths::is_link(&meta) || !meta.is_dir() {
        return Err(AdoptError::Content(format!(
            "{} is not an ordinary starter content directory",
            path.display()
        )));
    }
    std::fs::remove_dir_all(&path)?;
    Ok(())
}

fn provenance_paths(out: &Path) -> Result<Vec<PathBuf>, AdoptError> {
    let mut out_files = Vec::new();
    for name in ["report.json", "redirects.json", "provenance_manifest.json"] {
        let p = out.join(name);
        if p.is_file() && !is_link(&p)? {
            out_files.push(p);
        }
    }
    if let Ok(entries) = std::fs::read_dir(out) {
        for e in entries {
            let e = e?;
            let name = e.file_name().into_string().unwrap_or_default();
            if name.ends_with("_manifest.json") && name != "provenance_manifest.json" {
                let p = e.path();
                if p.is_file() && !is_link(&p)? {
                    out_files.push(p);
                }
            }
        }
    }
    let prov = out.join("provenance");
    if prov.is_dir() && !is_link(&prov)? {
        let mut nested = Vec::new();
        scan_files(&prov, "", &mut nested)?;
        out_files.extend(nested.into_iter().map(|f| f.source));
    }
    out_files.sort();
    out_files.dedup();
    Ok(out_files)
}

/// Copy report/provenance/manifest/redirect evidence to the target's
/// `.cv/imports/<run>/`, preserving `provenance/` nesting. Existing bytes
/// that are identical are reused; different bytes are a refusal, never an
/// overwrite.
pub fn copy_provenance(src: &AdoptionSource, target_root: &Path) -> Result<usize, AdoptError> {
    let base = store::run_dir(target_root, &src.run_id)?;
    store::write(target_root, &src.record)?;
    std::fs::create_dir_all(&base)?;
    let mut copied = 0;
    for source in provenance_paths(&src.out)? {
        let rel = source.strip_prefix(&src.out).map_err(|_| {
            AdoptError::Content(format!("{} is outside the run output", source.display()))
        })?;
        let rel = rel.to_string_lossy().replace('\\', "/");
        portable_relative(&rel)
            .map_err(|_| AdoptError::Content(format!("provenance path {rel:?} is not portable")))?;
        let bytes = std::fs::read(&source)?;
        let target_rel = format!("{}/{}/{}", store::DIR, src.run_id, rel);
        let target =
            confined_file(target_root, Path::new(&target_rel)).map_err(EngineError::from)?;
        if let Ok(existing) = std::fs::read(&target) {
            if existing == bytes {
                continue;
            }
            return Err(AdoptError::Collisions(vec![Collision {
                kind: CollisionKind::Path,
                incoming: rel,
                existing: target_rel,
            }]));
        }
        match write::write_staged(target_root, &target_rel, &bytes, None)? {
            WriteOutcome::Written { .. } => copied += 1,
            WriteOutcome::StaleBase { .. } => {
                return Err(AdoptError::Stale(format!(
                    "provenance target {target_rel} changed during adoption"
                )));
            }
        }
    }
    Ok(copied)
}

/// Write the page-level adoption sidecar and mark the run adopted.
pub fn record_adoption(
    src: &AdoptionSource,
    target_root: &Path,
    at_ms: i64,
    target_kind: &str,
    subfolder_name: Option<&str>,
    planned: &[PlannedFile],
) -> Result<(), AdoptError> {
    let workspace = target_root.display().to_string();
    let mut rec = AdoptionRecord::new(&src.run_id, &src.mode, at_ms, &workspace, target_kind);
    rec.content_subfolder = subfolder_name.map(str::to_string);
    rec.inputs = src.record.inputs.clone();
    let by_display: BTreeMap<_, _> = planned.iter().map(|p| (p.display.clone(), p)).collect();
    rec.pages = src
        .pages
        .iter()
        .filter_map(|p| {
            let display = join_rel(&[subfolder_name.unwrap_or(""), &p.path]);
            by_display.get(&display).map(|_| AdoptedPage {
                entity_id: p.entity_id.clone(),
                path: display,
                source: p.source.clone(),
            })
        })
        .collect();
    store::write_adoption(target_root, &rec)?;
    let mut run = src.record.clone();
    run.adopted = Some(store::Adopted { at_ms, workspace });
    store::write(target_root, &run)?;
    Ok(())
}

/// Post-adoption gate: `boris validate`, then (only when validation
/// passes) `boris build`, both through the caller's engine path so events
/// flow into Activity/Pages like any other run. The sink factory is asked
/// for a fresh sink per verb because `EventSink` is a consumable box.
pub fn validate_then_build(
    run: &dyn Fn(
        cv_core::engine::Verb,
        cv_core::engine::EventSink,
    ) -> cv_core::Result<cv_core::engine::ExitClass>,
    sink: &dyn Fn() -> cv_core::engine::EventSink,
) -> Result<
    (
        cv_core::engine::ExitClass,
        Option<cv_core::engine::ExitClass>,
    ),
    cv_core::Error,
> {
    use cv_core::engine::{ExitClass, Verb};
    let validate = run(Verb::Validate, sink())?;
    if validate != ExitClass::Ok {
        return Ok((validate, None));
    }
    let build = run(Verb::Build, sink())?;
    Ok((validate, Some(build)))
}

/// What one committed adoption transaction produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxReport {
    pub planned: usize,
    pub written: usize,
    /// Resume-skipped because the interrupted attempt had already landed
    /// exactly these bytes.
    pub resumed: usize,
    pub provenance_copied: usize,
}

/// One crash-aware adoption transaction: preflight, journal, content,
/// provenance, sidecars, journal cleanup — in that order, so an
/// interruption at any point is either fully before the first write or
/// resumable by re-running the same request.
///
/// Crash semantics:
/// - before the journal is durable there is nothing to resume and nothing
///   was written, so a retry is a fresh attempt;
/// - with the journal durable, a retry of the same target shape treats
///   byte-identical targets as already-written (staged writes are atomic
///   per file, so identical bytes can only be the interrupted attempt's
///   own completed writes) and still refuses any different bytes;
/// - a retry naming a different target shape refuses instead of mixing
///   two operations' partial state;
/// - provenance copies and the adoption/run records are inside the unit:
///   they are re-run idempotently on resume and the journal is deleted
///   only after they are durable.
///
/// Concurrency: this function performs no scheduler calls, so the caller
/// can — and the live workspace worker must — run the whole transaction
/// inside `EngineScheduler::mutation_batch`, serializing preflight and
/// every write against page edits, moves, builds, and watch starts.
pub fn transact(
    target_root: &Path,
    src: &AdoptionSource,
    content_input: &str,
    merge_subfolder: Option<&str>,
    target_kind: &str,
    existing_ids: &BTreeMap<String, String>,
    at_ms: i64,
) -> Result<TxReport, AdoptError> {
    let subfolder_name = match merge_subfolder {
        Some(raw) => Some(subfolder(raw)?),
        None => None,
    };
    let planned = plan(src, content_input, merge_subfolder)?;
    let pending = store::read_pending(target_root, &src.run_id).map_err(AdoptError::Report)?;
    let input_norm = content_input.trim().trim_matches('/');
    let resume = match &pending {
        None => false,
        Some(p) if p.matches(target_kind, input_norm, subfolder_name.as_deref()) => true,
        Some(p) => {
            return Err(AdoptError::Run(format!(
                "an interrupted adoption of {} into this workspace names {} below {:?} as '{}';                  complete that retry first or remove {}",
                src.run_id,
                p.content_subfolder.as_deref().unwrap_or("the content root"),
                p.content_input,
                p.target,
                store::PENDING
            )));
        }
    };
    let needs_copy = collisions(target_root, src, &planned, existing_ids, resume)?;

    // The recovery unit lands before the first content byte. Hashes are
    // of the candidate source bytes: the journal is evidence of what the
    // operation meant to write, and resume compares live bytes anyway.
    let mut journal = store::PendingAdoption::new(&src.run_id, target_kind, input_norm);
    journal.content_subfolder = subfolder_name.clone();
    for p in &planned {
        let bytes = std::fs::read(&p.source)?;
        journal.files.push(store::PendingFile {
            target: p.target.clone(),
            sha256: store::sha256_hex(&bytes),
            bytes: bytes.len() as u64,
        });
    }
    store::write_pending(target_root, &journal)?;

    let (written, skipped) = copy_content(target_root, &planned, &needs_copy)?;
    let provenance = copy_provenance(src, target_root)?;
    record_adoption(
        src,
        target_root,
        at_ms,
        target_kind,
        subfolder_name.as_deref(),
        &planned,
    )?;
    store::clear_pending(target_root, &src.run_id)?;
    Ok(TxReport {
        planned: planned.len(),
        written,
        resumed: skipped,
        provenance_copied: provenance,
    })
}
