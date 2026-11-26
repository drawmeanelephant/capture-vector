//! Migrate port: reads `.cv/imports/` (`cv_store::imports`), decodes each
//! run's lab documents through the `cv-contracts` mirrors, and keeps the
//! review triage in `.cv/imports/review.jsonl`. Also records a lab run
//! ([`run_lab`]): the spawn itself stays in `cv_engine::lab::run`.

pub mod job;
pub mod scene;

use cv_contracts::lab::{Disposition, Document, Inventory, Report};
use cv_core::engine::EngineSource;
use cv_engine::error::EngineError;
use cv_engine::lab::{self as engine_lab, LabError, LabExit, LabIdentity, LabRun, ModeSpec};
use cv_engine::proc::CancelToken;
use cv_model::modules::doctor::source_label;
use cv_model::modules::migrate::{
    Family, ImportExtra, ImportExtraKind, ImportInput, ImportInputKind, ImportMode, ImportRun,
    ImportRunInput, Imports, LabState, MigratePort, ReviewItem, ReviewState, RunStatus,
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

/// The pinned lab's mode table projected for the wizard. This is metadata
/// only; `cv-engine` remains the sole validator of flags, path kinds, argv,
/// overlap and output ownership.
/// Issue #172 acceptance scope: one file-input mode (WordPress WXR) and one
/// folder-input mode with inventory first (Hugo), plus cancellation. Every
/// other emitting mode — including ones whose inventory contracts exist —
/// stays hidden until its own issue widens the wizard. This is an explicit
/// allowlist, not a decode-capability filter: what the wizard offers is a
/// product decision, and #172 is exactly these two.
const WIZARD_EMITTERS: &[&str] = &["wordpress", "hugo"];

pub fn modes() -> Vec<ImportMode> {
    let emitters = engine_lab::MODES
        .iter()
        .filter(|mode| WIZARD_EMITTERS.contains(&mode.name))
        .map(|mode| mode.name)
        .collect::<std::collections::HashSet<_>>();
    engine_lab::MODES
        .iter()
        // An inventory companion whose emitting mode was filtered out is an
        // orphan: without the emitter nothing can pair with it, and the
        // wizard's companion-hiding is keyed on offered emitters — so it
        // would surface as a selectable import in its own right. Orphans
        // leave with their emitters.
        .filter(|mode| {
            WIZARD_EMITTERS.contains(&mode.name)
                || (mode.name.ends_with("-inventory")
                    && emitters.contains(mode.name.trim_end_matches("-inventory")))
        })
        .map(|mode| ImportMode {
            name: mode.name.into(),
            family: mode.family.key().into(),
            description: mode.description().into(),
            inputs: mode
                .inputs
                .iter()
                .map(|input| ImportInput {
                    flag: input.flag.into(),
                    label: format!(
                        "{} {}",
                        input.flag.trim_start_matches('-'),
                        match input.kind {
                            engine_lab::InputKind::File => "file",
                            engine_lab::InputKind::Dir => "folder",
                        }
                    ),
                    kind: match input.kind {
                        engine_lab::InputKind::File => ImportInputKind::File,
                        engine_lab::InputKind::Dir => ImportInputKind::Folder,
                    },
                    required: input.required,
                    extensions: engine_lab::input_extensions(input.flag)
                        .iter()
                        .map(|e| e.to_string())
                        .collect(),
                })
                .collect(),
            inventory: mode.inventory().map(str::to_string),
            safety: mode
                .safety_notes()
                .into_iter()
                .map(str::to_string)
                .collect(),
            extras: mode
                .extras
                .iter()
                .map(|e| ImportExtra {
                    flag: e.flag.into(),
                    label: format!("{} ({})", e.label, e.flag),
                    hint: e.hint.into(),
                    kind: match e.kind {
                        engine_lab::modes::ExtraKind::Text => ImportExtraKind::Text,
                        engine_lab::modes::ExtraKind::Count => ImportExtraKind::Count,
                    },
                })
                .collect(),
        })
        .collect()
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
    ///
    /// Opening is read-only with respect to run directories: unfinished
    /// records are projected from their per-run locks, never rewritten by a
    /// workspace-wide sweep. Review-log opening may claim its own narrow
    /// writer lock, but it never writes inside any run folder.
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
            modes: modes(),
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
                        let mut r = project(root, d);
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
        let Some(log) = &mut self.log else {
            return Err(format!(
                "review decisions are not persisted: {}",
                self.log_error
                    .as_deref()
                    .unwrap_or("review log unavailable")
            ));
        };
        log.append(run, item, to_store(to)).map_err(|e| {
            format!(
                "review decision was not written for {run}/{item}: {e}; this window is read-only or the log is unavailable"
            )
        })?;
        self.states.insert((run.into(), item.into()), to);
        Ok(())
    }
}

fn status(root: &Path, rec: &RunRecord) -> RunStatus {
    if rec.adopted.is_some() {
        return RunStatus::Adopted;
    }
    if rec.refused.is_some() {
        return RunStatus::Refused;
    }
    match rec.finished.as_ref().map(|f| f.exit) {
        // A recorded outcome is authoritative even if lock state has changed
        // since the worker wrote it. Only outcome-less records are projected
        // through the per-run lock, read-only.
        None => match store::probe_run(root, &rec.run_id) {
            Ok(store::RunProbe::Held) => RunStatus::Running,
            Ok(store::RunProbe::Free | store::RunProbe::Missing) | Err(_) => RunStatus::Unfinished,
        },
        Some(store::FinishedExit::Ok) => RunStatus::Ok,
        Some(store::FinishedExit::Cancelled) => RunStatus::Cancelled,
        Some(store::FinishedExit::Usage) => RunStatus::Refused,
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

fn expected_lab_format(mode: &str) -> String {
    format!(
        "boris-{}-migration-lab",
        mode.trim_end_matches("-inventory")
    )
}

fn report_facts(r: &Report, run: &mut ImportRun) {
    let k = r.key();
    run.format = Some((k.format.into(), k.schema_version));
    run.tool_version = Some(r.tool_version().into());
    let expected = expected_lab_format(&run.mode);
    if k.format != expected {
        run.problem = Some(format!(
            "report.json is a {} document but this run is mode {}; a wrong-family report supplies no count evidence",
            k.format, run.mode
        ));
        return;
    }
    let pages = r.pages();
    match r {
        Report::Hugo(h) => {
            run.counts = class_counts(&h.summary.by_class);
            run.count_evidence = true;
            run.totals = vec![("pages emitted".into(), h.summary.pages_emitted)];
        }
        Report::Jekyll(j) => {
            run.counts = class_counts(&j.summary.by_class);
            run.count_evidence = true;
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

/// One of the disposition classes the pinned lab can emit. Unknown-only
/// rows are a tampered or unsupported shape, not count evidence.
fn known_class(d: &Disposition) -> bool {
    CLASSES.iter().any(|c| *c == d.as_str())
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
    // Strict family/mode consistency: a decodable wrong-family inventory
    // must never supply count evidence for this run. The mode stem names
    // exactly one lab format family; anything else is a misplaced or
    // tampered document and fails closed with a visible problem.
    let stem = run.mode.trim_end_matches("-inventory");
    let expected = format!("boris-{stem}-migration-lab");
    if *format != expected {
        run.problem = Some(format!(
            "inventory.json is a {format} document but this run is mode {};              a wrong-family inventory supplies no count evidence",
            run.mode
        ));
        return;
    }
    run.counts = match i {
        Inventory::Hugo(h) => class_counts(&h.class_counts),
        Inventory::Jekyll(j) => tally(j.constructs.iter().map(|d| (&d.class, d.count))),
        Inventory::Substack(s) => tally(s.dispositions.iter().map(|d| (&d.class, d.count))),
        Inventory::Enex(e) => tally(e.dispositions.iter().map(|d| (&d.class, d.count))),
    };
    // Count-evidence provenance by family shape: Hugo's class_counts is a
    // structurally required summary object — present even when every count
    // is zero, so valid-zero is evidence. The tabled families only count as
    // evidence when at least one row of a known class exists.
    run.count_evidence = match i {
        Inventory::Hugo(_) => true,
        Inventory::Jekyll(j) => j.constructs.iter().any(|d| known_class(&d.class)),
        Inventory::Substack(s) => s.dispositions.iter().any(|d| known_class(&d.class)),
        Inventory::Enex(e) => e.dispositions.iter().any(|d| known_class(&d.class)),
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
/// Returns `true` when at least one lab document was decoded. A run that
/// exits 0 but leaves nothing decodable is missing the evidence every
/// downstream decision rests on — the caller must surface that, not treat
/// it as a valid zero-count outcome.
fn read_documents(out: &Path, run: &mut ImportRun) -> Result<bool, String> {
    let read = |rel: &str| read_capped(&out.join(rel), MAX_DOC_BYTES);
    let inventory_mode = run.mode.ends_with("-inventory");
    let (primary, wrong_role, role) = if inventory_mode {
        ("inventory.json", "report.json", "inventory")
    } else {
        ("report.json", "inventory.json", "emit")
    };
    let mut wrong_problem = None;
    let mut wrong_exact_family = false;
    if let Some(bytes) = read(wrong_role) {
        let bytes = bytes.map_err(|e| format!("{wrong_role}: {e}"))?;
        let decoded_family = if inventory_mode {
            let r = Report::decode(&bytes).map_err(|e| named(wrong_role, e))?;
            r.key().format.to_string()
        } else {
            let i = Inventory::decode(&bytes).map_err(|e| named(wrong_role, e))?;
            match &i {
                Inventory::Hugo(x) => x.format.clone(),
                Inventory::Jekyll(x) => x.format.clone(),
                Inventory::Substack(x) => x.format.clone(),
                Inventory::Enex(x) => x.format.clone(),
            }
        };
        let expected = expected_lab_format(&run.mode);
        wrong_exact_family = decoded_family == expected;
        let family = if wrong_exact_family {
            "right-family"
        } else {
            "wrong-family"
        };
        wrong_problem = Some(format!(
            "{wrong_role} is a {family} document but this is a {role} run in mode {}; wrong-role artifacts supply no count evidence",
            run.mode
        ));
    }
    let mut decoded = false;
    if let Some(bytes) = read(primary) {
        let bytes = bytes.map_err(|e| format!("{primary}: {e}"))?;
        if inventory_mode {
            let i = Inventory::decode(&bytes).map_err(|e| named(primary, e))?;
            inventory_facts(&i, run);
        } else {
            let r = Report::decode(&bytes).map_err(|e| named(primary, e))?;
            report_facts(&r, run);
        }
        decoded = true;
    } else {
        for rel in OTHER_DOCS {
            if let Some(bytes) = read(rel) {
                let bytes = bytes.map_err(|e| format!("{rel}: {e}"))?;
                let d = Document::decode(rel, &bytes).map_err(|e| named(rel, e))?;
                other_facts(&d, run);
                decoded = true;
                break;
            }
        }
    }
    if let Some(problem) = wrong_problem {
        // Emit modes can legitimately leave their exact-family inventory as
        // auxiliary evidence beside the authoritative report. A wrong-role
        // artifact only fails closed when it is all the run offers; a
        // wrong-family artifact is always a visible mismatch.
        if !decoded {
            return Err(problem);
        }
        if !wrong_exact_family {
            run.problem = Some(problem);
        }
    }
    Ok(decoded)
}

fn project(root: &Path, d: &RunDir) -> ImportRun {
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
    let mut run = ImportRun::new(&d.id, status(root, rec));
    run.mode = rec.mode.clone();
    run.inputs = rec
        .inputs
        .iter()
        .map(|i| ImportRunInput {
            flag: i.flag.clone(),
            path: i.path.clone(),
            path_sha256: i.path_sha256.clone(),
        })
        .collect();
    run.family = family_of(&rec.mode);
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
    if matches!(run.status, RunStatus::Running | RunStatus::Unfinished) {
        return run;
    }
    if out.join("provenance_manifest.json").is_file() {
        run.provenance = Some(format!("{}/provenance_manifest.json", run.out));
    } else if out.join("provenance").is_dir() {
        run.provenance = Some(format!("{}/provenance/", run.out));
    }
    match read_documents(&out, &mut run) {
        Err(e) => run.problem = Some(e),
        Ok(false) if run.status == RunStatus::Ok => {
            run.problem = Some(
                concat!(
                    "the lab reported success but the output contains no report.json, ",
                    "inventory.json, or other decodable lab document"
                )
                .into(),
            );
        }
        Ok(false) => {}
        Ok(true) => {}
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

fn exit_name(e: LabExit) -> store::FinishedExit {
    match e {
        LabExit::Ok => store::FinishedExit::Ok,
        LabExit::UsageError => store::FinishedExit::Usage,
        LabExit::IoFailure => store::FinishedExit::Io,
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

fn show(path: &Path) -> String {
    path.display().to_string()
}

/// Adapter refusals are mechanical safety failures, so translate the typed
/// error once at the app boundary instead of exposing debug formatting.
pub(crate) fn plain_prepare_refusal(e: &LabError) -> String {
    match e {
        LabError::MissingInput { mode, flag } => {
            format!("Mode {mode} needs {flag}; choose that input before running it.")
        }
        LabError::FlagNotAllowed { mode, flag } => {
            format!("Mode {mode} does not accept {flag}; the run was not started.")
        }
        LabError::DuplicateInput { flag } => {
            format!("{flag} was given more than once; the run was not started.")
        }
        LabError::InputPath { flag, path, source } => format!(
            "{flag} at {} could not be read ({source}); the run was not started.",
            show(path)
        ),
        LabError::InputKind {
            flag,
            path,
            expected,
        } => format!(
            "{flag} must be a {}, but {} is not; the run was not started.",
            expected.noun(),
            show(path)
        ),
        LabError::InputRelative { flag, path } => format!(
            "{flag} must be an absolute path, but {} is relative — it cannot be interpreted without guessing. Type or pick the full path; the run was not started.",
            show(path)
        ),
        LabError::InputSymlink {
            flag,
            input,
            component,
        } => format!(
            "{flag} at {} contains the symlink {}; the migration lab refuses symlinked inputs because they can redirect reads outside the tree you chose. Choose the real path instead; the run was not started.",
            show(input),
            show(component)
        ),
        LabError::OutPath { path, reason } => format!(
            "The output path {} cannot be used because it {reason}; the run was not started.",
            show(path)
        ),
        LabError::Overlap { flag, input, out } => format!(
            "The output path {} overlaps {flag} input {}. Choose an output outside the input; the run was not started.",
            show(out),
            show(input)
        ),
        other => other.to_string(),
    }
}

/// The lab reports refusals with a stable error name on stderr; the exit
/// code is mode-dependent (2 when mode-specific validation fires first,
/// 3 once validateRoots runs — both proven against the real pinned lab),
/// so the name, not the exit code, is the refusal signal.
/// Keep raw stderr bytes in `stderr.log` and put the actionable explanation,
/// including the relevant path, in `refused`.
/// The pinned lab's refusal error names, as `migration-lab (<mode>)
/// failed: <Name>` on stderr at the pinned commit.
pub(crate) fn lab_refusal_name(stderr: &str) -> Option<&'static str> {
    [
        ("InputSymlink", "InputSymlink"),
        ("OutputSymlink", "OutputSymlink"),
        ("InputOutputOverlap", "InputOutputOverlap"),
        ("OutputNotOwned", "OutputNotOwned"),
        ("OutputNotDirectory", "OutputNotDirectory"),
    ]
    .into_iter()
    .find(|(name, _)| stderr.contains(name))
    .map(|(_, full)| full)
}

pub(crate) fn plain_lab_refusal(stderr: &str, out: &Path) -> String {
    let path = show(out);
    if stderr.contains("InputSymlink") {
        return "The migration lab refused the run because a symlink appears in an input path. Choose the real path instead of a symlink; nothing was read or written.".to_string();
    }
    if stderr.contains("OutputSymlink") {
        return format!(
            "The migration lab refused to write to {path} because a symlink appears in that output path. Nothing was written there."
        );
    }
    if stderr.contains("InputOutputOverlap") || stderr.contains("overlap") {
        return format!(
            "The migration lab refused to write because the output path {path} overlaps an input path. Choose an output outside the input."
        );
    }
    if stderr.contains("OutputNotOwned")
        || stderr.contains("OutputNotEmpty")
        || stderr.contains("non-empty")
        || stderr.contains("not empty")
    {
        return format!(
            "The migration lab refused to write to {path} because it is not empty and is not owned by an earlier lab run. That folder was left unchanged."
        );
    }
    let detail = stderr
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .unwrap_or("the lab rejected the command line or paths");
    format!("The migration lab refused the run for {path}: {detail}")
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
    extras: Vec<(String, String)>,
    cancel: &CancelToken,
) -> Result<RunRecord, RunError> {
    // The output spelling is anchored at the canonical workspace root: the
    // workspace location is the user's chosen home for outputs, not
    // migration-input evidence, and canonicalizing it once here is the
    // narrow normalization that keeps `.cv/imports` reachable on macOS
    // alias mount points without ever flattening a user-selected input.
    let root = &std::fs::canonicalize(root).map_err(|e| RunError::Store(e.into()))?;
    let lock = store::claim_run(root, run_id).map_err(RunError::Store)?;
    let dir = lock.dir().to_path_buf();
    let run = LabRun {
        mode,
        inputs,
        extras,
        out: dir.join(store::OUT),
    };
    let mut rec = started_record(run_id, mode.name, lab, now_ms());
    rec.inputs = run
        .inputs
        .iter()
        .map(|(f, p)| store::input(f, &p.display().to_string()))
        .collect();
    let prepared = match run.prepare() {
        Ok(p) => p,
        Err(e) => {
            rec.refused = Some(plain_prepare_refusal(&e));
            store::write(&lock, &rec).map_err(RunError::Store)?;
            return Ok(rec);
        }
    };
    // Confinement is a hard invariant, proven here rather than assumed:
    // after every normalization the output must still be exactly the run's
    // own directory inside this workspace's `.cv/imports`.
    let confined = root.join(".cv").join("imports").join(run_id);
    if !prepared.out.starts_with(&confined) {
        rec.refused = Some(format!(
            "The output path {} escaped {} after normalization; the run was not started.",
            prepared.out.display(),
            confined.display()
        ));
        store::write(&lock, &rec).map_err(RunError::Store)?;
        return Ok(rec);
    }
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
    store::write(&lock, &rec).map_err(RunError::Store)?;
    let wait_started = std::time::Instant::now();
    let out = match engine_lab::run(lab, &run, cancel) {
        Ok(out) => out,
        Err(LabError::Engine(e)) => {
            let cancelled = matches!(e, EngineError::Cancelled) && cancel.is_cancelled();
            rec.finished = Some(store::Finished {
                exit_code: None,
                exit: if cancelled {
                    store::FinishedExit::Cancelled
                } else {
                    store::FinishedExit::Io
                },
                outcome: e.to_string(),
                duration_ms: wait_started.elapsed().as_millis() as u64,
            });
            store::write(&lock, &rec).map_err(RunError::Store)?;
            return Ok(rec);
        }
        Err(e) => {
            // Input or output state can change between the first validation
            // and cv-engine's validation immediately before the spawn. That
            // is still a pre-spawn refusal, not a lab failure.
            rec.refused = Some(plain_prepare_refusal(&e));
            store::write(&lock, &rec).map_err(RunError::Store)?;
            return Ok(rec);
        }
    };
    let (exit_code, outcome) = outcome_text(out.outcome);
    let (len, sha256) = store::write_stderr(&lock, &out.stderr_bytes).map_err(RunError::Store)?;
    rec.stderr_bytes = Some(len);
    rec.stderr_sha256 = Some(sha256);
    // The pinned lab reports safety refusals as exit 3 (its io_error code)
    // with a stable error name on stderr; exit 2 is a usage failure. Either
    // way the run never emitted, so both read as refusals — but only the
    // named cases are user-actionable path refusals.
    if out.exit == LabExit::UsageError || lab_refusal_name(&out.stderr).is_some() {
        rec.refused = Some(plain_lab_refusal(&out.stderr, &out.run.out));
    }
    rec.finished = Some(store::Finished {
        exit_code,
        exit: exit_name(out.exit),
        outcome,
        duration_ms: out.duration_ms,
    });
    store::write(&lock, &rec).map_err(RunError::Store)?;
    Ok(rec)
}

pub use job::{run_stub_lab, spawn_import, spawn_import_with};

#[cfg(test)]
mod tests;
