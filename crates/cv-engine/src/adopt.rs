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
use cv_store::paths::{confined_file, portable_relative};
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
    /// Files inside the workspace that are neither byte-proven starter
    /// debris nor the adoption's own partial output. They are never
    /// deleted — the operation refuses until the user moves them aside.
    Foreign(Vec<String>),
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
            Self::Foreign(files) => write!(
                f,
                "the workspace holds {} file(s) that are neither starter content nor this \
                 adoption's output; they were left untouched — move them aside or remove \
                 them and retry: {}",
                files.len(),
                files.join(", ")
            ),
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

/// Exact mode-to-report authority for every emitting mode whose report
/// contract this build mirrors. Membership in this table is the adoption
/// allowlist; there is no normalization or suffix inference.
fn adoptable_report_format(mode: &str) -> Option<&'static str> {
    use cv_contracts::lab::format;
    match mode {
        "hugo" => Some(format::HUGO),
        "jekyll" => Some(format::JEKYLL),
        "substack" => Some(format::SUBSTACK),
        "enex" => Some(format::ENEX),
        "obsidian" => Some(format::OBSIDIAN),
        "wordpress" => Some(format::WORDPRESS),
        // The asset utility emits a sanitized candidate content tree but no
        // page-id rows; adoption copies that tree through the same confined
        // transaction and records no invented page provenance.
        "asset-filename" => Some(format::ASSET_FILENAME),
        _ => None,
    }
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
    if record.refused.is_some()
        || record
            .finished
            .as_ref()
            .is_none_or(|f| f.exit != store::FinishedExit::Ok)
    {
        return Err(AdoptError::Run(format!(
            "run {run_id} has not finished successfully"
        )));
    }
    // Adoption is an allowlist, never a syntactic inventory denylist.
    // Unknown spellings, case/whitespace variants, inventory companions,
    // cross-cutting analysis modes, and emitters without a mirrored report
    // contract all refuse closed.
    let Some(expected_format) = adoptable_report_format(&record.mode) else {
        return Err(AdoptError::Run(format!(
            "run {run_id} mode {:?} is not an exact registered emitting mode",
            record.mode
        )));
    };
    // The directory name, embedded record identity, and spawned argv are
    // three copies of the same fact. Adoption rewrites this record only
    // after content/provenance work, so a mismatch must refuse here rather
    // than become a deterministic post-copy lock-identity failure.
    if record.run_id != run_id {
        return Err(AdoptError::Run(format!(
            "run.json names {:?} but lies in {run_id:?}'s directory",
            record.run_id
        )));
    }
    let expected_arg = format!("--mode={}", record.mode);
    let mode_args: Vec<&str> = record
        .argv
        .iter()
        .filter_map(|a| a.strip_prefix("--mode="))
        .collect();
    if mode_args != [record.mode.as_str()] || !record.argv.iter().any(|a| a == &expected_arg) {
        return Err(AdoptError::Run(format!(
            "run {run_id} argv does not carry exactly {expected_arg:?}"
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
    if report.key().format != expected_format {
        return Err(AdoptError::Report(format!(
            "report.json is a {} document but run {run_id} is mode {}",
            report.key().format,
            record.mode
        )));
    }
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
    /// Page path relative to the content input (no input prefix).
    pub display: String,
}

/// Validate a merge subfolder as one portable relative path below the
/// content input. The writer revalidates the complete target; this is the
/// user-input boundary.
pub fn subfolder(raw: &str) -> Result<String, AdoptError> {
    if raw.starts_with(['/', '\\']) || raw.trim_start() != raw {
        return Err(AdoptError::Subfolder(
            "the subfolder must be relative to the content input, not absolute".into(),
        ));
    }
    let s = raw.trim_end_matches('/');
    if s.is_empty() {
        return Err(AdoptError::Subfolder(
            "choose a subfolder inside the content input".into(),
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
    let input = content_input_authority(content_input)?;
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

mod input_authority;
mod preflight;
use input_authority::{content_input_authority, content_input_target_authority};
pub use preflight::{Collision, CollisionKind, collisions, copy_content};
use preflight::{bytes_identical, incoming_id_collisions, preflight_write_target_shapes};

/// Byte evidence of every file currently inside the content input,
/// keyed by workspace-relative target — the starter manifest recorded
/// into the journal before anything is touched.
fn starter_manifest(
    target_root: &Path,
    content_input: &str,
) -> Result<Vec<store::PendingFile>, AdoptError> {
    let input = content_input_authority(content_input)?;
    let dir = target_root.join(input);
    let mut files = Vec::new();
    if dir.is_dir() {
        scan_files(&dir, "", &mut files)?;
    }
    let mut out = Vec::new();
    for f in files {
        let target = join_rel(&[input, &f.relative]);
        let bytes = std::fs::read(&f.source)?;
        out.push(store::PendingFile {
            target,
            sha256: store::sha256_hex(&bytes),
            bytes: bytes.len() as u64,
        });
    }
    Ok(out)
}

/// Replace the starter tree selectively. Never a total clear: a file is
/// deleted only when it is byte-proven starter debris — identical to its
/// entry in the journal's starter manifest, so nothing a user or tool
/// wrote after the crash can ever be removed. Files at planned target
/// paths already byte-identical to the incoming source are the
/// adoption's own partial output and are left for the copy phase to skip.
/// Everything else is foreign: classification completes before the first
/// deletion, so a foreign file is a pre-write refusal that leaves the
/// whole tree — and especially the foreign file — untouched.
///
/// Returns the workspace-relative targets still present at planned paths
/// (byte-identical partial output), so the copy phase skips them in fresh
/// attempts too.
fn sweep_starter(
    target_root: &Path,
    content_input: &str,
    journal: &store::PendingAdoption,
    planned: &[PlannedFile],
) -> Result<(), AdoptError> {
    let input = content_input_authority(content_input)?;
    if input.is_empty() || input == "." {
        // A whole-workspace input would make the workspace root itself
        // the sweep domain — `.cv/`, the journal, the Boris config.
        // Replacement is only ever meaningful for a content subtree.
        return Err(AdoptError::Content(
            "refusing to replace a whole-workspace content input".into(),
        ));
    }
    let dir = target_root.join(input);
    let manifest: BTreeMap<&str, &store::PendingFile> = journal
        .starter
        .iter()
        .map(|f| (f.target.as_str(), f))
        .collect();
    let mut present = Vec::new();
    if dir.is_dir() {
        scan_files(&dir, "", &mut present)?;
    }

    let mut delete = Vec::new();
    let mut foreign = Vec::new();
    for f in &present {
        let target = join_rel(&[input, &f.relative]);
        let bytes = std::fs::read(&f.source)?;
        if let Some(entry) = manifest.get(target.as_str())
            && store::sha256_hex(&bytes) == entry.sha256
            && bytes.len() as u64 == entry.bytes
        {
            delete.push((target, f.source.clone()));
            continue;
        }
        // Not byte-proven starter. The only other legitimate occupant is
        // the adoption's own partial output: at a planned target path and
        // byte-identical to the incoming source. Note what is not here:
        // stranded `.cv-write-*.tmp` debris from a crash mid-rename. A
        // filename convention is user-controlled and is not provenance
        // evidence — automated deletion by name is a data-loss capability
        // this design refuses to have. Debris blocks the resume as
        // foreign; the user recognizes it as litter, removes it, and the
        // retry converges. Rare, explicit, and safe beats automatic.
        let partial_output = planned
            .iter()
            .find(|p| p.target == target)
            .is_some_and(|p| bytes_identical(&f.source, &p.source));
        if !partial_output {
            foreign.push(target);
        }
    }
    if !foreign.is_empty() {
        foreign.sort();
        return Err(AdoptError::Foreign(foreign));
    }
    for (_, path) in delete {
        std::fs::remove_file(&path)?;
    }
    // Prune directories left empty, deepest first. An empty directory is
    // structure, not data — pruning one can never lose a byte.
    prune_empty_dirs(&dir)?;
    Ok(())
}

/// Remove directories that hold no files, bottom-up, stopping at the
/// content input itself (its absence or presence are both fine).
fn prune_empty_dirs(dir: &Path) -> Result<(), AdoptError> {
    if !dir.is_dir() {
        return Ok(());
    }
    let mut dirs = Vec::new();
    collect_dirs(dir, &mut dirs)?;
    // Deepest first so parents empty out after their children vanish.
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for d in dirs {
        // Only remove when empty; a foreign empty directory contains no
        // data, and removing it loses nothing either.
        if std::fs::read_dir(&d).is_ok_and(|mut r| r.next().is_none()) {
            let _ = std::fs::remove_dir(&d);
        }
    }
    Ok(())
}

fn collect_dirs(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<(), AdoptError> {
    for e in std::fs::read_dir(dir)
        .map_err(|e| AdoptError::Content(format!("cannot read {}: {e}", dir.display())))?
    {
        let path = e?.path();
        if path.is_dir() && !path.symlink_metadata()?.file_type().is_symlink() {
            out.push(path.clone());
            collect_dirs(&path, out)?;
        }
    }
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
pub fn copy_provenance(
    src: &AdoptionSource,
    target_root: &Path,
    lock: &store::RunLock,
) -> Result<usize, AdoptError> {
    store::write(lock, &src.record)?;
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
    lock: &store::RunLock,
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
    let event = rec.event();
    if let Some(prior) = store::scan_adoptions(target_root)?
        .into_iter()
        .find(|r| r.run_id == src.run_id)
    {
        let mut pages = prior.pages;
        for page in rec.pages {
            if !pages.contains(&page) {
                pages.push(page);
            }
        }
        rec.pages = pages;
        rec.events = prior.events;
    }
    // Event identity is the adoption destination, not the wall-clock retry
    // that completed it. A crash after the sidecar write but before journal
    // cleanup leaves the same destination's event behind; the resumable
    // retry completes that event rather than inventing a second adoption.
    if let Some(prior_event) = rec.events.iter_mut().find(|e| {
        e.workspace == event.workspace
            && e.target == event.target
            && e.content_subfolder == event.content_subfolder
    }) {
        for page in event.pages {
            if !prior_event.pages.contains(&page) {
                prior_event.pages.push(page);
            }
        }
        prior_event.at_ms = event.at_ms;
    } else {
        rec.events.push(event);
    }
    store::write_adoption(lock, &rec)?;
    let mut run = src.record.clone();
    run.adopted = Some(store::Adopted { at_ms, workspace });
    store::write(lock, &run)?;
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
    validate_then_build_with_input(run, sink, None)
}

/// Adoption's gate over the exact content-input authority the transaction
/// wrote. `boris.json` may move source truth away from the default
/// `content/`; a bare validate/build would green the wrong tree there.
pub fn validate_then_build_with_input(
    run: &dyn Fn(
        cv_core::engine::Verb,
        cv_core::engine::EventSink,
    ) -> cv_core::Result<cv_core::engine::ExitClass>,
    sink: &dyn Fn() -> cv_core::engine::EventSink,
    input: Option<&str>,
) -> Result<
    (
        cv_core::engine::ExitClass,
        Option<cv_core::engine::ExitClass>,
    ),
    cv_core::Error,
> {
    use cv_core::engine::{ExitClass, Verb};
    let verb = |name: &str| match input {
        Some(input) => Verb::Custom {
            name: name.into(),
            args: vec!["--input".into(), input.into()],
        },
        None => {
            if name == "validate" {
                Verb::Validate
            } else {
                Verb::Build
            }
        }
    };
    let validate = run(verb("validate"), sink())?;
    if validate != ExitClass::Ok {
        return Ok((validate, None));
    }
    let build = run(verb("build"), sink())?;
    Ok((validate, Some(build)))
}

/// Workspace-instance evidence captured before an existing-workspace
/// adoption batch. The token proves `.cv` lineage; the containing
/// directory's file identity distinguishes that surviving lineage from a
/// byte copy restored at the same path. The mutation-lock carrier identity
/// is captured too: replacing `.cv` with same-token bytes before or during
/// the transaction otherwise hands adoption an independent copied lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedWorkspaceInstance {
    pub token: String,
    pub file: Option<(u64, u64)>,
    pub mutation_lock: Option<(u64, u64)>,
}

impl ExpectedWorkspaceInstance {
    pub fn new(
        token: impl Into<String>,
        file: Option<(u64, u64)>,
        mutation_lock: Option<(u64, u64)>,
    ) -> Self {
        Self {
            token: token.into(),
            file,
            mutation_lock,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentityCheckPhase {
    BeforeWrites,
    AfterContent,
    AfterCommit,
}

fn revalidate_workspace_instance(
    target_root: &Path,
    expected: &ExpectedWorkspaceInstance,
    phase: IdentityCheckPhase,
    mutation_lock: Option<&cv_store::workspace_mutation::WorkspaceMutationLock>,
) -> Result<(), AdoptError> {
    let consequence = match phase {
        IdentityCheckPhase::BeforeWrites => "nothing was written",
        IdentityCheckPhase::AfterContent => {
            "content or provenance bytes may have been written before the change was detected; the adoption recovery journal was retained so the partial state can be inspected or completed deliberately"
        }
        IdentityCheckPhase::AfterCommit => {
            "content, provenance, or adoption bookkeeping may have been written before the change was detected; the recovery journal may already have been cleared, so inspect the workspace and adoption sidecars before retrying"
        }
    };
    let current_file = crate::file_identity::identity(target_root);
    let evidence_error = |e: std::io::Error| {
        AdoptError::Run(format!(
            "adoption refused: workspace identity evidence could not be revalidated inside the mutation batch ({e}); {consequence}"
        ))
    };
    let current_token = cv_store::workspace_instance::read(target_root).map_err(evidence_error)?;
    let current_lock =
        cv_store::workspace_mutation::current_identity(target_root).map_err(evidence_error)?;
    let held_lock = mutation_lock
        .expect("existing-workspace identity validation requires its mutation lock")
        .identity()
        .map_err(evidence_error)?;
    let same_lock_carrier = matches!(
        (current_lock, held_lock),
        (Some(current), Some(held))
            if current == held && expected.mutation_lock == Some(current)
    );
    if current_token.as_deref() == Some(expected.token.as_str())
        && current_file == expected.file
        && same_lock_carrier
    {
        return Ok(());
    }
    Err(AdoptError::Run(format!(
        "adoption refused: the workspace identity captured before the mutation batch no longer matches this path; {consequence}"
    )))
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

/// Acquire the target workspace's per-run capability for the whole adoption
/// operation. A mirrored run in a new workspace is claimed fresh; an existing
/// or crash-mirrored run is locked in place. `RunExists` is the ordinary race
/// between those two probes, never a reason to write outside the capability.
fn adoption_lock(target_root: &Path, run_id: &str) -> Result<store::RunLock, AdoptError> {
    match store::claim_run(target_root, run_id) {
        Ok(lock) => Ok(lock),
        Err(store::Error::RunExists(_)) => Ok(store::lock_run(target_root, run_id)?),
        Err(e) => Err(e.into()),
    }
}

struct TxParams<'a> {
    target_root: &'a Path,
    src: &'a AdoptionSource,
    content_input: &'a str,
    merge_subfolder: Option<&'a str>,
    target_kind: &'a str,
    existing_ids: &'a BTreeMap<String, String>,
    at_ms: i64,
    expected_instance: Option<&'a ExpectedWorkspaceInstance>,
    workspace_mutation_lock: Option<cv_store::workspace_mutation::WorkspaceMutationLock>,
}

/// Inputs for an existing-workspace adoption whose captured instance must
/// still own the target path when the serialized engine transaction begins.
/// Grouping them keeps the evidence inseparable from the mutation request.
pub struct ExistingWorkspaceTransaction<'a> {
    pub target_root: &'a Path,
    pub src: &'a AdoptionSource,
    pub content_input: &'a str,
    pub merge_subfolder: Option<&'a str>,
    /// Reads the target's current explicit Boris ids. The engine invokes
    /// this only after acquiring the workspace mutation lock and validating
    /// instance identity, so collision evidence cannot be captured before
    /// the serialized critical section.
    pub scan_existing_ids: Box<dyn FnOnce() -> BTreeMap<String, String> + Send + 'a>,
    pub at_ms: i64,
    pub expected_instance: &'a ExpectedWorkspaceInstance,
}

/// One crash-aware adoption transaction: preflight, journal, content,
/// provenance, sidecars, journal cleanup — ordered so an interruption is
/// either fully before the first write or resumable by re-running the
/// same request. This is resumable-interruption atomicity, not failure
/// atomicity: deterministic conflicts (path/id/parent-shape/journal)
/// are pre-write refusals, and once bytes land the operation converges
/// by resume rather than rollback.
///
/// Crash semantics:
/// - before the journal is durable there is nothing to resume and nothing
///   was written, so a retry is a fresh attempt;
/// - with the journal durable, a retry of the same target shape treats
///   byte-identical targets as already-written (staged writes are atomic
///   per file, so identical bytes can only be the interrupted attempt's
///   own work), refuses targets whose bytes differ, and re-checks the
///   journal itself against the live operation before trusting it;
/// - for a `new` workspace the starter replacement is inside the
///   recovery unit and is never a total clear: the journal records byte
///   evidence of the whole starter tree, and only files byte-identical
///   to that evidence are deleted. Anything else — a user note written
///   into the never-yet-opened directory after a crash, an edited
///   starter page, a foreign squatter at a planned path — is foreign
///   data, refuses the resume, and is never deleted or overwritten.
///   Byte-identical files at planned paths are the interrupted attempt's
///   own output (staged writes are atomic) and are skipped, so resume
///   converges without restarting the copy. One race limitation, not
///   ownership proof: a file written into the tree between `boris init`
///   completing and the starter-manifest capture is indistinguishable
///   from starter — even though the app never opens the workspace before
///   the adoption finishes, nothing technically prevents another tool
///   from writing there in that window.
///
/// What this does not serialize: filesystem reactions of an already
/// running watch are the caller's concern (the composition root stops
/// and restores the watch around the batch); see the scheduler's
/// `mutation_batch` for the write-lock half.
///
/// Raw adoption transaction without prior workspace-instance evidence. This
/// is appropriate for a newly created workspace and for engine-local
/// preflight/recovery tests; production existing-workspace adoption must use
/// [`transact_existing_with_identity`] so captured identity is revalidated
/// inside the engine transaction.
pub fn transact(
    target_root: &Path,
    src: &AdoptionSource,
    content_input: &str,
    merge_subfolder: Option<&str>,
    target_kind: &str,
    existing_ids: &BTreeMap<String, String>,
    at_ms: i64,
) -> Result<TxReport, AdoptError> {
    content_input_authority(content_input)?;
    content_input_target_authority(target_root, content_input)?;
    // A newly initialized workspace is already visible to other Capture
    // Vector processes, so "new" only means there is no live scheduler in
    // this process. Establish the `.cv` carrier as part of the intentional
    // mutation and serialize the transaction like any other workspace write.
    let mutation_lock = cv_store::workspace_mutation::lock_for_content_mutation(target_root)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                AdoptError::Run(format!(
                    "adoption blocked: the workspace is busy with another mutation ({e}); nothing was written"
                ))
            } else {
                AdoptError::Io(e)
            }
        })?;
    transact_impl(TxParams {
        target_root,
        src,
        content_input,
        merge_subfolder,
        target_kind,
        existing_ids,
        at_ms,
        expected_instance: None,
        workspace_mutation_lock: Some(mutation_lock),
    })
}

/// Existing-workspace adoption transaction with captured instance evidence.
/// The revalidation happens before the run lock, journal, or any content
/// byte is written. Callers must run this inside the scheduler mutation
/// critical section; the engine performs the comparison here so the
/// capture-to-commit race cannot be closed incorrectly in the composition root.
pub fn transact_existing_with_identity(
    tx: ExistingWorkspaceTransaction<'_>,
) -> Result<TxReport, AdoptError> {
    content_input_authority(tx.content_input)?;
    content_input_target_authority(tx.target_root, tx.content_input)?;
    // Lock order is fixed and documented: process-local scheduler batch
    // (held by the composition root) -> workspace-instance mutation lock ->
    // per-run lock. No adoption path acquires those capabilities in another
    // order. The kernel releases this file lock even if a holder crashes;
    // the carrier is never deleted, so there is no stale-lock payload to
    // interpret or clean up.
    let mutation_lock = cv_store::workspace_mutation::lock(tx.target_root).map_err(|e| {
        AdoptError::Run(format!(
            "adoption blocked: the workspace mutation lock is unavailable or held by a concurrent writer ({e}); nothing was written"
        ))
    })?;
    revalidate_workspace_instance(
        tx.target_root,
        tx.expected_instance,
        IdentityCheckPhase::BeforeWrites,
        Some(&mutation_lock),
    )?;
    let existing_ids = (tx.scan_existing_ids)();
    transact_impl(TxParams {
        target_root: tx.target_root,
        src: tx.src,
        content_input: tx.content_input,
        merge_subfolder: tx.merge_subfolder,
        target_kind: "existing",
        existing_ids: &existing_ids,
        at_ms: tx.at_ms,
        expected_instance: Some(tx.expected_instance),
        workspace_mutation_lock: Some(mutation_lock),
    })
}

fn transact_impl(params: TxParams<'_>) -> Result<TxReport, AdoptError> {
    if params.src.mode == "asset-filename" && params.target_kind == "existing" {
        return Err(AdoptError::Run(
            "asset-filename adoption into an existing workspace is refused: the lab report carries no page-id authority, so duplicate Boris ids cannot be preflighted; choose a new workspace".into(),
        ));
    }
    let TxParams {
        target_root,
        src,
        content_input,
        merge_subfolder,
        target_kind,
        existing_ids,
        at_ms,
        expected_instance,
        workspace_mutation_lock,
    } = params;
    if let Some(expected) = expected_instance {
        revalidate_workspace_instance(
            target_root,
            expected,
            IdentityCheckPhase::BeforeWrites,
            workspace_mutation_lock.as_ref(),
        )?;
    }
    let lock = adoption_lock(target_root, &src.run_id)?;
    let subfolder_name = match merge_subfolder {
        Some(raw) => Some(subfolder(raw)?),
        None => None,
    };
    let planned = plan(src, content_input, merge_subfolder)?;
    let intrinsic = incoming_id_collisions(src);
    if !intrinsic.is_empty() {
        return Err(AdoptError::Collisions(intrinsic));
    }
    // Zero-write preflight precedes even the recovery journal: the journal
    // is itself one of the transaction's writes, not a harmless prelude.
    preflight_write_target_shapes(target_root, src, &planned)?;
    // An existing adoption sidecar is evidence even when it is damaged:
    // decode it before the first write so a later adoption can never
    // silently overwrite provenance Capture Vector could not interpret.
    store::read_adoption_record(target_root, &src.run_id).map_err(AdoptError::Report)?;
    let pending = store::read_pending(target_root, &src.run_id).map_err(AdoptError::Report)?;
    let input_norm = content_input_authority(content_input)?;
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
    // Journal enforcement, resume only: the recovery unit is evidence
    // that must prove itself against the live operation before a single
    // byte is trusted or skipped.
    if resume {
        let journal = pending.as_ref().expect("resume implies a journal");
        // 1. Every recorded target must be a portable, confined relative
        //    path — an unauthenticated entry means the journal is foreign
        //    or tampered, never a recovery input.
        let mut journal_targets = BTreeSet::new();
        for f in &journal.files {
            if portable_relative(&f.target).is_err() {
                return Err(AdoptError::Run(format!(
                    "{} records {:?}, which is not a portable confined target;                      remove {} or repair it before retrying",
                    store::PENDING,
                    f.target,
                    store::PENDING
                )));
            }
            journal_targets.insert(f.target.clone());
        }
        // 1b. Starter-manifest entries are evidence of what may be
        //     deleted; unauthenticated entries would license deleting
        //     foreign data, so they are refused exactly like file entries.
        for f in &journal.starter {
            if portable_relative(&f.target).is_err() {
                return Err(AdoptError::Run(format!(
                    "{} records starter {:?}, which is not a portable confined target;                      remove {} or repair it before retrying",
                    store::PENDING,
                    f.target,
                    store::PENDING
                )));
            }
        }
        // 2. The recorded file set must be exactly this operation's plan:
        //    a subset or superset is a different operation's journal.
        let planned_targets: BTreeSet<String> = planned.iter().map(|p| p.target.clone()).collect();
        if journal_targets != planned_targets {
            return Err(AdoptError::Run(format!(
                "{} records {} file(s) but this adoption plans {}; the interrupted                  operation is not this one — remove {} or complete it first",
                store::PENDING,
                journal_targets.len(),
                planned_targets.len(),
                store::PENDING
            )));
        }
        // 3. Every recorded hash must match the live source bytes: the
        //    sha256 entries are enforced integrity evidence, not decor.
        //    A run output is immutable, so a mismatch means the journal
        //    was written for different content — never resume from it.
        for f in &journal.files {
            let planned_file = planned
                .iter()
                .find(|p| p.target == f.target)
                .expect("sets are equal");
            let bytes = std::fs::read(&planned_file.source)?;
            let live = store::sha256_hex(&bytes);
            if live != f.sha256 || bytes.len() as u64 != f.bytes {
                return Err(AdoptError::Run(format!(
                    "{} records different bytes for {} than the run output holds;                      the journal is not evidence of this operation — remove {} to start over",
                    store::PENDING,
                    f.target,
                    store::PENDING
                )));
            }
        }
    }

    // For a new workspace the starter content is replaced wholesale, so
    // the deletion is part of the recovery unit: the journal lands FIRST,
    // then the starter tree is cleared (idempotently — an interrupted
    // attempt may have removed some or all of it already), and only then
    // does preflight run, against the clean tree. A crash anywhere in
    // that sequence is exactly what resume converges; there is no
    // journal-less mutation window. For an existing-workspace merge
    // nothing is deleted, so preflight can — and must — refuse before
    // the journal and the first write.
    // The recovery unit lands before the first mutation. For a new
    // workspace it also carries byte evidence of the starter tree, so
    // the replacement can never delete anything it cannot prove was
    // generated starter content.
    let write_journal = |with_starter: bool| -> Result<store::PendingAdoption, AdoptError> {
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
        if with_starter {
            journal.starter = starter_manifest(target_root, input_norm)?;
        }
        store::write_pending(&lock, &journal)?;
        Ok(journal)
    };

    let new_workspace = target_kind == "new";
    if new_workspace {
        let input = input_norm.trim().trim_matches('/');
        if input.is_empty() || input == "." {
            // Refuse before the journal lands: replacement is only ever
            // meaningful for a content subtree, never the workspace root
            // with `.cv/` and the Boris config in it.
            return Err(AdoptError::Content(
                "refusing to replace a whole-workspace content input".into(),
            ));
        }
    }
    let needs_copy = if new_workspace {
        // The journal is written only on a fresh attempt; on resume the
        // original is evidence and must never be re-recorded over the
        // mutated tree. The sweep then deletes only byte-proven starter
        // debris — a total clear on resume can destroy foreign files a
        // user wrote into the never-yet-opened directory after the crash,
        // and is exactly the data loss the manifest exists to prevent.
        let journal = match &pending {
            Some(j) => j.clone(),
            None => write_journal(true)?,
        };
        sweep_starter(target_root, input_norm, &journal, &planned)?;
        // Survivors at planned paths are byte-identical partial output by
        // the sweep's own classification, so identical bytes are a skip in
        // fresh attempts too — nothing else can be present to collide.
        collisions(target_root, src, &planned, existing_ids, true)?
    } else {
        let needs_copy = collisions(target_root, src, &planned, existing_ids, resume)?;
        write_journal(false)?;
        needs_copy
    };

    let copied = (|| {
        let (written, skipped) = copy_content(target_root, &planned, &needs_copy)?;
        let provenance = copy_provenance(src, target_root, &lock)?;
        Ok::<_, AdoptError>((written, skipped, provenance))
    })();
    let (written, skipped, provenance) = match copied {
        Ok(copied) => copied,
        Err(e) => {
            // A rename-away can turn the first path-based write failure into
            // a bare operating-system error. If identity evidence proves that
            // shape, the identity diagnosis is the truthful cause and the
            // partial-write/recovery disclosure must accompany it.
            if let Some(expected) = expected_instance {
                revalidate_workspace_instance(
                    target_root,
                    expected,
                    IdentityCheckPhase::AfterContent,
                    workspace_mutation_lock.as_ref(),
                )?;
            }
            return Err(e);
        }
    };
    // POSIX cannot pin a directory against rename, so a swap after the
    // pre-write validation is detection-not-prevention. Before durable
    // success provenance is recorded or the recovery journal is cleared,
    // prove the path still denotes the captured instance. Failure leaves
    // the journal in place and reports partial-write truth rather than
    // false success.
    if let Some(expected) = expected_instance {
        revalidate_workspace_instance(
            target_root,
            expected,
            IdentityCheckPhase::AfterContent,
            workspace_mutation_lock.as_ref(),
        )?;
    }
    record_adoption(
        src,
        target_root,
        &lock,
        at_ms,
        target_kind,
        subfolder_name.as_deref(),
        &planned,
    )?;
    store::clear_pending(&lock)?;
    // Final ownership check immediately before success is reported. The
    // earlier post-content check preserves the recovery journal in the
    // common detection shape; this final check closes as much of the
    // bookkeeping window as POSIX permits. If interference wins even this
    // race, the error is still honest rather than a false success, and its
    // message says the journal may already be gone.
    if let Some(expected) = expected_instance {
        revalidate_workspace_instance(
            target_root,
            expected,
            IdentityCheckPhase::AfterCommit,
            workspace_mutation_lock.as_ref(),
        )?;
    }
    Ok(TxReport {
        planned: planned.len(),
        written,
        resumed: skipped,
        provenance_copied: provenance,
    })
}
