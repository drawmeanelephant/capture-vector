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

/// Refuse every path and id overlap before the first write. `existing_ids`
/// is the composition root's current Boris-view of the target workspace
/// (`id` → content-input-relative path); adoption itself never parses
/// Boris frontmatter.
pub fn collisions(
    target_root: &Path,
    src: &AdoptionSource,
    planned: &[PlannedFile],
    existing_ids: &BTreeMap<String, String>,
) -> Result<(), AdoptError> {
    let mut out = Vec::new();
    let mut incoming_ids = BTreeSet::new();
    for p in planned {
        if confined_file(target_root, Path::new(&p.target))
            .ok()
            .is_some_and(|path| std::fs::symlink_metadata(path).is_ok())
        {
            out.push(Collision {
                kind: CollisionKind::Path,
                incoming: p.display.clone(),
                existing: p.target.clone(),
            });
        }
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
            out.push(Collision {
                kind: CollisionKind::Id,
                incoming: page.entity_id.clone(),
                existing: existing.clone(),
            });
        }
    }
    out.sort_by(|a, b| {
        a.incoming
            .cmp(&b.incoming)
            .then(a.existing.cmp(&b.existing))
    });
    out.dedup();
    if out.is_empty() {
        Ok(())
    } else {
        Err(AdoptError::Collisions(out))
    }
}

/// Copy planned files byte-for-byte with `expected=None` staged writes.
/// A race after preflight stops the adoption instead of overwriting.
pub fn copy_content(target_root: &Path, planned: &[PlannedFile]) -> Result<usize, AdoptError> {
    let mut written = 0;
    for p in planned {
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
    Ok(written)
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
