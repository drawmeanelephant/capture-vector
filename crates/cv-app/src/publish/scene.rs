//! Publish scenes. Their ledgers are written by the real `cv_publish`
//! publisher into a scratch workspace and read back through
//! [`DiskPort`]. A scene engine stands in for Boris: each preflight build
//! receives the captured `happy` build (its pages and proof pack).

use super::DiskPort;
use cv_core::engine::{
    Engine, EngineCaps, EngineInfo, EventSink, ExitClass, Fingerprint, PageDraft, Verb,
    WatchHandle, WriteOutcome,
};
use cv_core::events::shell::DegradedReason;
use cv_core::id::StableId;
use cv_core::rules::ArtifactSet;
use cv_core::sys::{FixedClock, Hash, SeqIdGen, Timestamp};
use cv_ledger::journal::JOURNAL_REL;
use cv_model::workbench::Workbench;
use cv_publish::adapters::{
    COPY_TO_FOLDER, CopyToFolder, Delivered, Delivery, DeliveryAdapter, HTTP_POST, TransportFailure,
};
use cv_publish::secrets::NoSecrets;
use cv_publish::{
    PreflightOutcome, PreflightRequest, Publisher, Ruleset, RulesetVerdict, Target, TargetConfig,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const CLOCK: FixedClock = FixedClock(Timestamp(1_700_000_000_000));
pub(super) struct CapturedBuild {
    pub(super) root: PathBuf,
    pub(super) check: ExitClass,
}

fn flag<'a>(args: &'a [String], name: &str) -> cv_core::Result<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
        .ok_or_else(|| cv_core::Error::Engine(format!("scene engine needs {name}")))
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let dst = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &dst)?;
        } else {
            std::fs::copy(e.path(), dst)?;
        }
    }
    Ok(())
}

impl Engine for CapturedBuild {
    fn locate(&self) -> cv_core::Result<EngineInfo> {
        Ok(EngineInfo {
            version: "boris/0.9.0".into(),
            vcs_revision: None,
            caps: self.capabilities(),
        })
    }
    fn capabilities(&self) -> EngineCaps {
        EngineCaps {
            watch: false,
            serve: false,
            authoring: false,
            writes: false,
        }
    }
    fn snapshot(&self) -> cv_core::Result<ArtifactSet> {
        Err(cv_core::Error::Engine(
            "scene engine has no snapshot".into(),
        ))
    }
    fn watch(&self, _: EventSink) -> cv_core::Result<Box<dyn WatchHandle>> {
        Err(cv_core::Error::Engine("scene engine has no watch".into()))
    }
    fn run(&self, cmd: &Verb, _: EventSink) -> cv_core::Result<ExitClass> {
        let io = |e: std::io::Error| cv_core::Error::Engine(e.to_string());
        let Verb::Custom { name, args } = cmd else {
            return Ok(ExitClass::Ok);
        };
        match name.as_str() {
            "build" if args.iter().any(|a| a == "--html-dir") => {
                let dist = cv_fixtures::fixtures_root().join("captures/happy/dist/dist");
                let out = self.root.join(flag(args, "--html-dir")?);
                copy_tree(&dist, &out).map_err(io)?;
                Ok(ExitClass::Ok)
            }
            "build" if args.iter().any(|a| a == "--out") => {
                let dir = self.root.join(flag(args, "--out")?);
                std::fs::create_dir_all(&dir).map_err(io)?;
                std::fs::write(
                    dir.join("graph.json"),
                    cv_fixtures::ir_capture("happy").file("graph.json"),
                )
                .map_err(io)?;
                Ok(ExitClass::Ok)
            }
            "check" => Ok(self.check),
            _ => Ok(ExitClass::Ok),
        }
    }

    fn run_caused(
        &self,
        cmd: &Verb,
        sink: EventSink,
        _: Option<&cv_core::events::EventId>,
    ) -> cv_core::Result<ExitClass> {
        self.run(cmd, sink)
    }

    fn write_content(
        &self,
        _: &PageDraft,
        _: Option<Fingerprint>,
    ) -> cv_core::Result<WriteOutcome> {
        Err(cv_core::Error::Engine("scene engine does not write".into()))
    }
}

pub(super) struct SceneRules;

impl Ruleset for SceneRules {
    fn hash(&self) -> Hash {
        Hash([0x5c; 32])
    }
    fn evaluate(&self, _: &cv_publish::preflight::PreflightArtifacts<'_>) -> RulesetVerdict {
        RulesetVerdict::default()
    }
}

/// Touches the transport, then loses the outcome: the run is `unknown`.
struct LostOutcome;

impl DeliveryAdapter for LostOutcome {
    fn id(&self) -> &str {
        HTTP_POST
    }
    fn deliver(&self, _: &Delivery<'_>) -> Result<Delivered, TransportFailure> {
        Err(TransportFailure::after_contact(
            "connection reset after the upload began",
        ))
    }
}

pub(super) fn config(target: Target, adapter: &str, destination: &str) -> TargetConfig {
    // Only static-dist is an unhosted copy. Hosted and record targets seed
    // with a real configuration shape rather than a falsely empty base.
    let base = if target == Target::StaticDist {
        ""
    } else {
        "https://example.test/"
    };
    TargetConfig::default_html(target, base, adapter, destination)
}

pub(super) fn open<'a>(root: &Path, engine: &'a CapturedBuild) -> Publisher<'a> {
    Publisher::open(root, engine, &CLOCK, &NoSecrets).expect("scene ledger opens")
}

pub(super) fn granted(o: cv_publish::Result<PreflightOutcome>) -> StableId {
    match o.expect("scene preflight runs") {
        PreflightOutcome::Granted { run, .. } => run,
        PreflightOutcome::Blocked { blocks, .. } => panic!("scene preflight blocked: {blocks:?}"),
    }
}

/// Four runs, one per target, in record order: delivered to a folder
/// with its exports read back; blocked by a failing Boris check; touched
/// with no recorded outcome (`unknown`); approved but not dispatched.
pub fn seed(root: &Path) {
    let ok = CapturedBuild {
        root: root.into(),
        check: ExitClass::Ok,
    };
    let failing = CapturedBuild {
        root: root.into(),
        check: ExitClass::ContentFailure,
    };
    let mut ids = SeqIdGen::new("run");
    let preflight = |p: &mut Publisher<'_>, cfg: &TargetConfig, ids: &mut SeqIdGen| {
        p.preflight(
            &PreflightRequest {
                config: cfg.clone(),
            },
            &SceneRules,
            ids,
        )
    };

    let cfg = config(Target::StaticDist, COPY_TO_FOLDER, "deployed");
    let mut p = open(root, &ok).with_adapter(CopyToFolder::new(root));
    let run = granted(preflight(&mut p, &cfg, &mut ids));
    p.approve(&run, "local").expect("scene approval");
    p.publish(&run, &cfg, &SceneRules).expect("scene delivery");
    drop(p);

    let cfg = config(Target::GithubPages, COPY_TO_FOLDER, "pages");
    let mut p = open(root, &failing);
    preflight(&mut p, &cfg, &mut ids).expect("scene preflight runs");
    drop(p);

    let cfg = config(Target::NostrNip23, HTTP_POST, "https://relay.example.test/");
    let mut p = open(root, &ok).with_adapter(LostOutcome);
    let run = granted(preflight(&mut p, &cfg, &mut ids));
    p.approve(&run, "local").expect("scene approval");
    p.publish(&run, &cfg, &SceneRules).expect("scene dispatch");
    drop(p);

    let cfg = config(Target::StandardSite, HTTP_POST, "https://pds.example.test/");
    let mut p = open(root, &ok);
    let run = granted(preflight(&mut p, &cfg, &mut ids));
    p.approve(&run, "local").expect("scene approval");
}

/// Cut the journal's second committed record short, keeping its newline:
/// a committed line that no longer decodes.
pub fn corrupt(root: &Path) {
    let path = root.join(JOURNAL_REL);
    let raw = std::fs::read(&path).expect("scene journal");
    let mut lines: Vec<&[u8]> = raw.split(|b| *b == b'\n').collect();
    lines[1] = &lines[1][..lines[1].len() / 2];
    std::fs::write(&path, lines.join(&b'\n')).expect("scene journal");
}

/// A throwaway workspace; the ledger is read before it is removed.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "cv-publish-scene-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("scene scratch dir");
        Self(p)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn show(wb: &mut Workbench, prepare: impl FnOnce(&Path)) {
    let dir = Scratch::new();
    prepare(&dir.0);
    let port = DiskPort::open(&dir.0);
    let warning = crate::publish_warning(&port);
    *wb = crate::init_with(Box::new(port));
    crate::boot(wb);
    wb.run_command("shell.goto.publish");
    if let Some(w) = warning {
        wb.disclose(DegradedReason::StoreCorrupt);
        wb.evidence_status(w);
    }
}

pub fn runs(wb: &mut Workbench) {
    show(wb, seed);
    wb.select_row(0);
}

pub fn detail(wb: &mut Workbench) {
    show(wb, seed);
    wb.set_nav("completed");
    wb.select_row(0);
}

pub fn unknown(wb: &mut Workbench) {
    show(wb, seed);
    wb.set_nav("unresolved");
    wb.select_row(0);
}

pub fn empty(wb: &mut Workbench) {
    show(wb, |_| {});
    wb.select_row(0);
}

pub fn degraded(wb: &mut Workbench) {
    show(wb, |root| {
        seed(root);
        corrupt(root);
    });
    wb.select_row(0);
}

/// What a scene's writable port publishes with: the captured build,
/// the shipped Audit packs, a fixed clock.
pub fn writer(root: &Path) -> super::write::Writer {
    super::write::Writer {
        engine: Box::new(CapturedBuild {
            root: root.into(),
            check: ExitClass::Ok,
        }),
        rules: Box::new(super::write::AuditRules::shipped()),
        clock: Box::new(CLOCK),
        ids: Box::new(SeqIdGen::new("run")),
    }
}

/// Touches the folder, then loses the outcome: an `unknown` copy run.
struct LostCopy;

impl DeliveryAdapter for LostCopy {
    fn id(&self) -> &str {
        COPY_TO_FOLDER
    }
    fn deliver(&self, _: &Delivery<'_>) -> Result<Delivered, TransportFailure> {
        Err(TransportFailure::after_contact(
            "the app quit while copying into the folder",
        ))
    }
}

/// One static-dist run, approved and dispatched through [`LostCopy`]:
/// unknown, with nothing at its destination.
pub fn seed_unknown_copy(root: &Path) {
    let engine = CapturedBuild {
        root: root.into(),
        check: ExitClass::Ok,
    };
    let cfg = super::write::delivery_config(Target::StaticDist).expect("static-dist delivers");
    let mut p = open(root, &engine).with_adapter(LostCopy);
    let run = granted(p.preflight(
        &PreflightRequest {
            config: cfg.clone(),
        },
        &SceneRules,
        &mut SeqIdGen::new("lost"),
    ));
    p.approve(&run, "local").expect("scene approval");
    p.publish(&run, &cfg, &SceneRules).expect("scene dispatch");
}

/// Replace one sealed page of every run's preflight output.
pub fn tamper(root: &Path) {
    let runs = root.join(".cv/publish");
    for e in std::fs::read_dir(&runs).expect("scene runs").flatten() {
        let page = e.path().join("site/index.html");
        if page.exists() {
            std::fs::write(page, "<p>edited after preflight</p>").expect("scene tamper");
        }
    }
}

/// A writable workspace kept on disk while `drive` runs its commands.
fn drive(
    wb: &mut Workbench,
    prepare: impl FnOnce(&Path),
    drive: impl FnOnce(&mut Workbench, &Path),
) {
    let dir = Scratch::new();
    prepare(&dir.0);
    *wb = crate::init_with(Box::new(DiskPort::writable(&dir.0, writer(&dir.0))));
    crate::boot(wb);
    wb.run_command("shell.goto.publish");
    drive(wb, &dir.0);
}

const PREFLIGHT_STATIC: &str = "publish.preflight.static-dist";

/// Preflight sealed a static-dist run; its approval dialog is open.
pub fn approve(wb: &mut Workbench) {
    drive(
        wb,
        |_| {},
        |wb, _| {
            wb.run_command(PREFLIGHT_STATIC);
            wb.run_command("publish.approve");
        },
    );
}

/// Preflight → approve → dispatch: copied to the folder, complete.
pub fn delivered(wb: &mut Workbench) {
    drive(
        wb,
        |_| {},
        |wb, _| {
            wb.run_command(PREFLIGHT_STATIC);
            wb.run_command("publish.approve");
            wb.dialog_act("approve");
            wb.run_command("publish.dispatch");
        },
    );
}

/// A sealed page changed after preflight: dispatch refuses.
pub fn refused(wb: &mut Workbench) {
    drive(
        wb,
        |_| {},
        |wb, root| {
            wb.run_command(PREFLIGHT_STATIC);
            wb.run_command("publish.approve");
            wb.dialog_act("approve");
            tamper(root);
            wb.run_command("publish.dispatch");
        },
    );
}

/// An unknown copy run reconciled by the read-only probe.
pub fn reconciled(wb: &mut Workbench) {
    drive(wb, seed_unknown_copy, |wb, _| {
        wb.set_nav("unresolved");
        wb.select_row(0);
        wb.run_command("publish.reconcile");
        let probe = wb
            .dialogs
            .open()
            .and_then(|d| d.visible_choices().iter().position(|(id, _)| id == "probe"));
        if let Some(row) = probe {
            wb.dialog_choose(row);
        }
    });
}
