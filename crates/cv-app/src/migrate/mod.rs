//! Migrate port: reads `.cv/imports/` (`cv_store::imports`), decodes each
//! run's lab documents through the `cv-contracts` mirrors, and keeps the
//! review triage in `.cv/imports/review.jsonl`. Also records a lab run
//! ([`run_lab`]): the spawn itself stays in `cv_engine::lab::run`.

pub mod adopt;
pub mod scene;

use cv_contracts::lab::{Disposition, Document, Inventory, Report};
use cv_core::engine::EngineSource;
use cv_engine::lab::{self as engine_lab, LabError, LabExit, LabIdentity, LabRun, ModeSpec};
use cv_engine::proc::CancelToken;
use cv_model::modules::doctor::source_label;
use cv_model::modules::migrate::{
    Family, ImportRun, Imports, LabState, MigratePort, ReviewItem, ReviewState, RunStatus,
};
use cv_store::imports::{self as store, ReviewLog, RunDir, RunRecord};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A Markdown report larger than this is not shown.
const MAX_REPORT_BYTES: u64 = 1 << 20;
/// A lab JSON document larger than this is reported unreadable.
const MAX_DOC_BYTES: u64 = 32 << 20;

/// The Markdown reports a run may carry, in the order one is picked.
const REPORTS_MD: &[&str] = &["REPORT.md", "INVENTORY.md", "FRONTMATTER_REVIEW.md"];
/// Mirrored documents that are not `report.json` / `inventory.json`.
const OTHER_DOCS: &[&str] = &["link_audit.json", "frontmatter_review.json"];

/// The family mailboxes, from the lab's mode table in `cv-engine`.
pub fn families() -> Vec<Family> {
    engine_lab::Family::ALL
        .iter()
        .map(|f| Family {
            key: f.key().into(),
            label: f.label().into(),
        })
        .collect()
}

fn family_of(mode: &str) -> Option<String> {
    engine_lab::mode(mode).map(|m| m.family.key().to_string())
}

/// Gate the lab for `root` with the production search order.
pub fn locate(root: &Path) -> LabState {
    let loc = engine_lab::locator_from_env(root);
    match engine_lab::locate(&loc, root, engine_lab::PINNED) {
        Ok(id) => LabState::Located {
            tool_id: id.tool_id,
            source: source_label(&id.source).into(),
            path: id.path.display().to_string(),
        },
        Err(e) => lab_error(e),
    }
}

fn lab_error(e: LabError) -> LabState {
    match e {
        LabError::NotFound { searched } => LabState::Absent {
            searched: searched.iter().map(|p| p.display().to_string()).collect(),
        },
        LabError::OverrideMissing(p) => LabState::OverrideMissing {
            path: p.display().to_string(),
        },
        LabError::VersionMismatch {
            path,
            found,
            expected,
            ..
        } => LabState::Mismatch {
            found,
            want: expected.into(),
            path: path.display().to_string(),
        },
        other => LabState::Failed {
            error: other.to_string(),
        },
    }
}

pub struct DiskPort {
    root: Option<PathBuf>,
    lab: LabState,
    log: Option<ReviewLog>,
    states: HashMap<(String, String), ReviewState>,
    log_error: Option<String>,
}

fn to_model(s: store::ReviewState) -> ReviewState {
    match s {
        store::ReviewState::Open => ReviewState::Open,
        store::ReviewState::Accepted => ReviewState::Accepted,
        store::ReviewState::Deferred => ReviewState::Deferred,
    }
}

fn to_store(s: ReviewState) -> store::ReviewState {
    match s {
        ReviewState::Open => store::ReviewState::Open,
        ReviewState::Accepted => store::ReviewState::Accepted,
        ReviewState::Deferred => store::ReviewState::Deferred,
    }
}

impl DiskPort {
    /// No workspace: no runs, the lab unchecked.
    pub fn none() -> Self {
        Self::detached(LabState::NoWorkspace)
    }

    fn detached(lab: LabState) -> Self {
        Self {
            root: None,
            lab,
            log: None,
            states: HashMap::new(),
            log_error: None,
        }
    }

    /// Runs under `root/.cv/imports/`; triage persists beside them.
    pub fn open(root: &Path, lab: LabState) -> Self {
        let mut port = Self::detached(lab);
        port.root = Some(root.to_path_buf());
        match ReviewLog::open(root) {
            Ok((log, states, _)) => {
                port.log = Some(log);
                port.states = states.into_iter().map(|(k, s)| (k, to_model(s))).collect();
            }
            Err(e) => port.log_error = Some(e.to_string()),
        }
        port
    }
}

impl MigratePort for DiskPort {
    fn load(&mut self) -> Imports {
        let mut out = Imports {
            lab: self.lab.clone(),
            families: families(),
            runs: Vec::new(),
            error: self.log_error.clone(),
            store: match (&self.log, &self.root) {
                (Some(_), _) => format!("{}/{}", store::DIR, store::REVIEW),
                (None, Some(_)) => "not persisted (review log unavailable)".into(),
                (None, None) => "session only (no workspace)".into(),
            },
        };
        let Some(root) = &self.root else {
            return out;
        };
        match store::scan(root) {
            Ok(dirs) => {
                out.runs = dirs
                    .iter()
                    .map(|d| {
                        let mut r = project(d);
                        for i in &mut r.review {
                            if let Some(s) = self.states.get(&(r.id.clone(), i.id.clone())) {
                                i.state = *s;
                            }
                        }
                        r
                    })
                    .collect();
            }
            Err(e) => out.error = Some(e.to_string()),
        }
        out
    }

    fn record(&mut self, run: &str, item: &str, to: ReviewState) -> Result<(), String> {
        if let Some(log) = &mut self.log {
            log.append(run, item, to_store(to))
                .map_err(|e| e.to_string())?;
        }
        self.states.insert((run.into(), item.into()), to);
        Ok(())
    }
}

fn status(rec: &RunRecord) -> RunStatus {
    if rec.adopted.is_some() {
        return RunStatus::Adopted;
    }
    if rec.refused.is_some() {
        return RunStatus::Refused;
    }
    match rec.finished.as_ref().map(|f| f.exit.as_str()) {
        None => RunStatus::Running,
        Some("ok") => RunStatus::Ok,
        Some("usage") => RunStatus::Refused,
        Some(_) => RunStatus::Failed,
    }
}

fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(path)
        .to_string()
}

/// A regular, non-symlinked file under the run's output, capped at `max`.
fn read_capped(path: &Path, max: u64) -> Option<Result<Vec<u8>, String>> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() {
        return Some(Err(format!(
            "{} is not a regular file",
            file_name(&path.display().to_string())
        )));
    }
    if meta.len() > max {
        return Some(Err(format!("{} bytes exceeds the read limit", meta.len())));
    }
    Some(std::fs::read(path).map_err(|e| e.to_string()))
}

/// The lab's ranking, for ordering counts: exact < transformed <
/// unsupported < human_review, then anything else as first seen.
const CLASSES: [&str; 4] = ["exact", "transformed", "unsupported", "human_review"];

fn tally<'a>(classes: impl Iterator<Item = (&'a Disposition, u64)>) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = CLASSES.iter().map(|c| (c.to_string(), 0)).collect();
    for (d, n) in classes {
        match out.iter_mut().find(|(k, _)| k == d.as_str()) {
            Some((_, c)) => *c += n,
            None => out.push((d.as_str().to_string(), n)),
        }
    }
    out
}

fn class_counts(c: &cv_contracts::lab::ClassCounts) -> Vec<(String, u64)> {
    vec![
        ("exact".into(), c.exact),
        ("transformed".into(), c.transformed),
        ("unsupported".into(), c.unsupported),
        ("human_review".into(), c.human_review),
    ]
}

fn needs_review(d: &Disposition) -> bool {
    matches!(d, Disposition::HumanReview | Disposition::Unsupported)
}

struct Entry<'a> {
    tag: &'static str,
    class: &'a Disposition,
    source: String,
    construct: String,
    reason: String,
}

fn report_entries(r: &Report) -> Vec<Entry<'_>> {
    let mut v = Vec::new();
    match r {
        Report::Hugo(h) => {
            for p in h.pages.iter().filter(|p| needs_review(&p.class)) {
                let reason = if p.notes.is_empty() && !p.dropped_keys.is_empty() {
                    format!("dropped keys: {}", p.dropped_keys.join(", "))
                } else {
                    p.notes.join("; ")
                };
                v.push(Entry {
                    tag: "page",
                    class: &p.class,
                    source: p.source.clone().unwrap_or_else(|| p.output.clone()),
                    construct: p.kind.clone(),
                    reason,
                });
            }
        }
        Report::Jekyll(j) => {
            for p in j.pages.iter().filter(|p| needs_review(&p.class)) {
                v.push(Entry {
                    tag: "page",
                    class: &p.class,
                    source: p.source_path.clone().unwrap_or_else(|| p.output.clone()),
                    construct: p.kind.clone(),
                    reason: p.review.join("; "),
                });
            }
            for n in j.not_emitted.iter().filter(|n| needs_review(&n.class)) {
                v.push(Entry {
                    tag: "not-emitted",
                    class: &n.class,
                    source: n.path.clone(),
                    construct: "not emitted".into(),
                    reason: n.reason.clone(),
                });
            }
        }
        Report::Substack(s) => v.extend(finding_entries(&s.findings)),
        Report::Enex(e) => v.extend(finding_entries(&e.findings)),
        Report::Obsidian(o) => {
            for p in o.pages.iter().filter(|p| needs_review(&p.conversion)) {
                v.push(Entry {
                    tag: "page",
                    class: &p.conversion,
                    source: p.source_path.clone(),
                    construct: "note".into(),
                    reason: p.notes.join("; "),
                });
            }
        }
        Report::WordPress(w) => {
            for p in w.pages.iter().filter(|p| needs_review(&p.conversion)) {
                v.push(Entry {
                    tag: "page",
                    class: &p.conversion,
                    source: p.output_path.clone(),
                    construct: p.post_type.clone(),
                    reason: p.feature_codes.join(", "),
                });
            }
        }
    }
    v
}

fn finding_entries(f: &[cv_contracts::lab::Finding]) -> Vec<Entry<'_>> {
    f.iter()
        .filter(|f| needs_review(&f.class))
        .map(|f| Entry {
            tag: "finding",
            class: &f.class,
            source: if f.entity_id.is_empty() {
                "(whole run)".into()
            } else {
                f.entity_id.clone()
            },
            construct: f.code.clone(),
            reason: f.detail.clone(),
        })
        .collect()
}

fn inventory_entries(i: &Inventory) -> Vec<Entry<'_>> {
    match i {
        Inventory::Hugo(h) => h
            .pages
            .iter()
            .filter(|p| needs_review(&p.class))
            .map(|p| Entry {
                tag: "inventory-page",
                class: &p.class,
                source: p.path.clone(),
                construct: p.kind.clone(),
                reason: String::new(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn items(entries: Vec<Entry<'_>>) -> Vec<ReviewItem> {
    let mut seen: HashMap<String, u32> = HashMap::new();
    entries
        .into_iter()
        .map(|e| {
            let base = store::item_id(&[e.tag, &e.source, &e.construct]);
            let n = seen.entry(base.clone()).or_insert(0);
            *n += 1;
            let id = if *n == 1 {
                base
            } else {
                store::item_id(&[e.tag, &e.source, &e.construct, &n.to_string()])
            };
            ReviewItem {
                id,
                class: e.class.as_str().into(),
                source: e.source,
                construct: e.construct,
                reason: e.reason,
                state: ReviewState::Open,
            }
        })
        .collect()
}

fn report_facts(r: &Report, run: &mut ImportRun) {
    let k = r.key();
    run.format = Some((k.format.into(), k.schema_version));
    run.tool_version = Some(r.tool_version().into());
    let pages = r.pages();
    match r {
        Report::Hugo(h) => {
            run.counts = class_counts(&h.summary.by_class);
            run.totals = vec![("pages emitted".into(), h.summary.pages_emitted)];
        }
        Report::Jekyll(j) => {
            run.counts = class_counts(&j.summary.by_class);
            run.totals = vec![
                ("pages".into(), j.summary.pages),
                ("not emitted".into(), j.summary.not_emitted),
            ];
        }
        _ => {
            run.counts = tally(pages.iter().map(|p| (p.disposition, 1)));
            run.totals = vec![("pages".into(), pages.len() as u64)];
        }
    }
    run.review = items(report_entries(r));
}

fn inventory_facts(i: &Inventory, run: &mut ImportRun) {
    let (format, version, tool) = match i {
        Inventory::Hugo(x) => (&x.format, x.schema_version, &x.tool_version),
        Inventory::Jekyll(x) => (&x.format, x.schema_version, &x.tool_version),
        Inventory::Substack(x) => (&x.format, x.schema_version, &x.tool_version),
        Inventory::Enex(x) => (&x.format, x.schema_version, &x.tool_version),
    };
    run.format = Some((format.clone(), version));
    run.tool_version = Some(tool.clone());
    run.counts = match i {
        Inventory::Hugo(h) => class_counts(&h.class_counts),
        Inventory::Jekyll(j) => tally(j.constructs.iter().map(|d| (&d.class, d.count))),
        Inventory::Substack(s) => tally(s.dispositions.iter().map(|d| (&d.class, d.count))),
        Inventory::Enex(e) => tally(e.dispositions.iter().map(|d| (&d.class, d.count))),
    };
    run.review = items(inventory_entries(i));
}

fn other_facts(doc: &Document, run: &mut ImportRun) {
    let (format, version) = match doc {
        Document::LinkAudit(a) => {
            run.totals = vec![
                ("html files".into(), a.html_files),
                ("findings".into(), a.findings.len() as u64),
            ];
            (&a.format, a.schema_version)
        }
        Document::FrontmatterReview(f) => {
            run.tool_version = Some(f.tool_version.clone());
            run.totals = vec![
                ("unknown keys".into(), f.total_unknown_keys),
                ("occurrences".into(), f.total_occurrences),
            ];
            (&f.format, f.schema_version)
        }
        _ => return,
    };
    run.format = Some((format.clone(), version));
}

/// Some decode errors already lead with the document's name.
fn named(rel: &str, e: impl std::fmt::Display) -> String {
    let s = e.to_string();
    if s.starts_with(rel) {
        s
    } else {
        format!("{rel}: {s}")
    }
}

/// Decode the run's main document; `Err` names the file and why.
fn read_documents(out: &Path, run: &mut ImportRun) -> Result<(), String> {
    let read = |rel: &str| read_capped(&out.join(rel), MAX_DOC_BYTES);
    if let Some(bytes) = read("report.json") {
        let bytes = bytes.map_err(|e| format!("report.json: {e}"))?;
        let r = Report::decode(&bytes).map_err(|e| named("report.json", e))?;
        report_facts(&r, run);
        return Ok(());
    }
    if let Some(bytes) = read("inventory.json") {
        let bytes = bytes.map_err(|e| format!("inventory.json: {e}"))?;
        let i = Inventory::decode(&bytes).map_err(|e| named("inventory.json", e))?;
        inventory_facts(&i, run);
        return Ok(());
    }
    for rel in OTHER_DOCS {
        if let Some(bytes) = read(rel) {
            let bytes = bytes.map_err(|e| format!("{rel}: {e}"))?;
            let d = Document::decode(rel, &bytes).map_err(|e| named(rel, e))?;
            other_facts(&d, run);
            return Ok(());
        }
    }
    Ok(())
}

fn project(d: &RunDir) -> ImportRun {
    let rel = format!("{}/{}", store::DIR, d.id);
    let rec = match &d.record {
        Ok(r) => r,
        Err(e) => {
            let mut run = ImportRun::new(&d.id, RunStatus::Failed);
            run.source = d.id.clone();
            run.out = format!("{rel}/{}", store::OUT);
            run.problem = Some(e.clone());
            return run;
        }
    };
    let mut run = ImportRun::new(&d.id, status(rec));
    run.mode = rec.mode.clone();
    run.family = family_of(&rec.mode);
    run.suggested_archetype =
        engine_lab::mode(&rec.mode).and_then(|m| m.archetype.map(str::to_string));
    run.source = rec
        .inputs
        .first()
        .map(|i| file_name(&i.path))
        .unwrap_or_else(|| d.id.clone());
    run.started_ms = rec.started_ms;
    run.lab = Some(rec.lab.tool_id.clone());
    run.boris_pin = rec.boris_pin.clone();
    run.argv = rec.argv.clone();
    run.refused = rec.refused.clone();
    if let Some(f) = &rec.finished {
        run.exit_code = f.exit_code;
        run.outcome = Some(f.outcome.clone());
        run.duration_ms = Some(f.duration_ms);
    }
    run.out = format!("{rel}/{}", store::OUT);
    let out = d.out();
    if run.status == RunStatus::Running {
        return run;
    }
    if out.join("provenance_manifest.json").is_file() {
        run.provenance = Some(format!("{}/provenance_manifest.json", run.out));
    } else if out.join("provenance").is_dir() {
        run.provenance = Some(format!("{}/provenance/", run.out));
    }
    if let Err(e) = read_documents(&out, &mut run) {
        run.problem = Some(e);
    }
    run.report = REPORTS_MD.iter().find_map(|name| {
        let bytes = read_capped(&out.join(name), MAX_REPORT_BYTES)?.ok()?;
        Some((
            name.to_string(),
            String::from_utf8_lossy(&bytes).into_owned(),
        ))
    });
    run
}

fn outcome_text(o: cv_engine::exit::ExitOutcome) -> (Option<i32>, String) {
    use cv_engine::exit::ExitOutcome;
    match o {
        ExitOutcome::Exited { code } => (Some(code), format!("exited {code}")),
        ExitOutcome::Signaled { signal } => (None, format!("signal {signal}")),
        ExitOutcome::Killed => (None, "killed".into()),
    }
}

fn exit_name(e: LabExit) -> &'static str {
    match e {
        LabExit::Ok => "ok",
        LabExit::UsageError => "usage",
        LabExit::IoFailure => "io",
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A record for a run the gated `lab` is about to start.
pub fn started_record(run_id: &str, mode: &str, lab: &LabIdentity, started_ms: i64) -> RunRecord {
    let source = match lab.source {
        EngineSource::EnvOverride => "CV_MIGRATION_LAB_BIN",
        ref s => source_label(s),
    };
    let mut rec = RunRecord::started(
        run_id,
        mode,
        store::RecordedLab {
            tool_id: lab.tool_id.clone(),
            source: source.into(),
            path: lab.path.display().to_string(),
        },
        started_ms,
    );
    let pin = cv_engine::locate::PINNED;
    rec.boris_pin = Some(format!("{} @ {}", pin.version, pin.vcs_revision));
    rec
}

#[derive(Debug)]
pub enum RunError {
    Store(store::Error),
    Lab(LabError),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(e) => write!(f, "{e}"),
            Self::Lab(e) => write!(f, "{e}"),
        }
    }
}

/// Run `mode` into `<root>/.cv/imports/<run_id>/out`, recording the run
/// before the spawn and again when it ends. A run the adapter refuses
/// (bad input, overlap, symlinked path) is recorded as refused.
pub fn run_lab(
    root: &Path,
    run_id: &str,
    lab: &LabIdentity,
    mode: &'static ModeSpec,
    inputs: Vec<(String, PathBuf)>,
    cancel: &CancelToken,
) -> Result<RunRecord, RunError> {
    let dir = store::run_dir(root, run_id).map_err(RunError::Store)?;
    let run = LabRun {
        mode,
        inputs,
        out: dir.join(store::OUT),
    };
    let mut rec = started_record(run_id, mode.name, lab, now_ms());
    rec.inputs = run
        .inputs
        .iter()
        .map(|(f, p)| store::input(f, &p.display().to_string()))
        .collect();
    std::fs::create_dir_all(&dir).map_err(|e| RunError::Store(e.into()))?;
    let prepared = match run.prepare() {
        Ok(p) => p,
        Err(e) => {
            rec.refused = Some(e.to_string());
            store::write(root, &rec).map_err(RunError::Store)?;
            return Ok(rec);
        }
    };
    rec.inputs = prepared
        .inputs
        .iter()
        .map(|(f, p)| store::input(f, &p.display().to_string()))
        .collect();
    rec.argv = prepared
        .argv
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    store::write(root, &rec).map_err(RunError::Store)?;
    let out = engine_lab::run(lab, &run, cancel).map_err(RunError::Lab)?;
    let (exit_code, outcome) = outcome_text(out.outcome);
    rec.finished = Some(store::Finished {
        exit_code,
        exit: exit_name(out.exit).into(),
        outcome,
        duration_ms: out.duration_ms,
    });
    store::write(root, &rec).map_err(RunError::Store)?;
    Ok(rec)
}

#[cfg(test)]
mod tests;
