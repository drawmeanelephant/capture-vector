//! Doctor goldens: one per engine story. Facts are fixed strings, not
//! this machine's, so the trees match on every OS.

use crate::{mid_watch, open_broken_recovered, snapshot};
use cv_core::engine::{EngineCaps, EngineSource};
use cv_core::events::shell::DegradedReason;
use cv_model::modules::doctor::{
    self, AppBuild, EngineState, Facts, ImportsHealth, Lab, Located, Lock, LockHolder, Namespace,
    Probe, Stamp, Store,
};
use cv_model::modules::migrate::LabState;
use cv_model::workbench::Workbench;

const ROOT: &str = "/Users/ana/sites/happy";
const BUNDLED: &str = "/Applications/Capture Vector.app/Contents/Resources/engines/boris";
const LAB_BUNDLED: &str =
    "/Applications/Capture Vector.app/Contents/Resources/engines/boris-migration-lab";
const BUILD_INFO: &str = r#"{"format":"boris-build-info","schemaVersion":"1","version":"boris/0.9.0","vcsRevision":"66efcfb"}"#;

fn candidates() -> Vec<(EngineSource, String)> {
    vec![
        (
            EngineSource::Bundled,
            "/Applications/Capture Vector.app/Contents/MacOS/engines/boris".into(),
        ),
        (EngineSource::Bundled, BUNDLED.into()),
        (
            EngineSource::Bundled,
            "/Applications/Capture Vector.app/Contents/MacOS/boris".into(),
        ),
        (EngineSource::Path, "/opt/homebrew/bin/boris".into()),
        (EngineSource::Path, "/usr/local/bin/boris".into()),
        (EngineSource::Path, "/usr/bin/boris".into()),
    ]
}

fn lab_candidates() -> Vec<(EngineSource, String)> {
    vec![
        (
            EngineSource::Bundled,
            "/Applications/Capture Vector.app/Contents/MacOS/engines/boris-migration-lab".into(),
        ),
        (EngineSource::Bundled, LAB_BUNDLED.into()),
        (
            EngineSource::Bundled,
            "/Applications/Capture Vector.app/Contents/MacOS/boris-migration-lab".into(),
        ),
        (
            EngineSource::Path,
            "/opt/homebrew/bin/boris-migration-lab".into(),
        ),
        (
            EngineSource::Path,
            "/usr/local/bin/boris-migration-lab".into(),
        ),
    ]
}

fn lab(state: LabState) -> Lab {
    Lab {
        state,
        pin: super::lab_pin(),
        candidates: lab_candidates(),
    }
}

fn healthy_lab() -> Lab {
    lab(LabState::Located {
        tool_id: super::lab_pin().version,
        source: "bundled".into(),
        path: LAB_BUNDLED.into(),
    })
}

fn store() -> Store {
    let ns = |name: &str, lock: Option<&str>, index: Option<&str>| Namespace {
        name: name.into(),
        dir: format!(".cv/{name}"),
        supported: 1,
        stamp: Stamp::Version(1),
        lock: lock.map(|p| Lock {
            path: p.into(),
            holder: LockHolder::ThisWindow,
        }),
        index: index.map(Into::into),
        error: None,
    };
    Store {
        root: Some(ROOT.into()),
        namespaces: vec![
            ns(
                "research",
                Some(".cv/research.writer.lock"),
                Some(
                    "/Users/ana/Library/Application Support/capture-vector/3f2a9c1d0b7e4a61/research/index.sqlite",
                ),
            ),
            ns("audit", Some(".cv/audit/lifecycle.lock"), None),
            Namespace {
                stamp: Stamp::Absent,
                ..ns("ledger", None, None)
            },
            ns("imports", None, None),
        ],
        imports: Some(ImportsHealth {
            runs: Some(2),
            review_lines: 1,
            ..ImportsHealth::default()
        }),
    }
}

fn facts(engine: EngineState, probe: Option<Probe>) -> Facts {
    Facts {
        engine,
        pin: super::pin(),
        candidates: candidates(),
        lab: healthy_lab(),
        probe,
        store: store(),
        app: AppBuild {
            version: "cv-app/0.1.0".into(),
            vcs_revision: String::new(),
            target: "macos-aarch64".into(),
            profile: "release".into(),
            toolchain: crate::build_info::toolchain().into(),
            logging: "off".into(),
        },
        redact: vec![(ROOT.into(), "<workspace>".into())],
    }
}

fn bundled() -> EngineState {
    EngineState::Located(Located {
        source: EngineSource::Bundled,
        path: BUNDLED.into(),
        version: "boris/0.9.0".into(),
        vcs_revision: "66efcfb".into(),
        build_info_raw: BUILD_INFO.into(),
    })
}

fn native_probe() -> Probe {
    Probe {
        os: "macos".into(),
        caps: EngineCaps {
            watch: true,
            serve: true,
            authoring: true,
            writes: true,
        },
        native_watch: true,
        evidence: vec![
            "watch+serve: hello schema 1 from boris/0.9.0".into(),
            "validate: Exited { code: 0 }".into(),
        ],
    }
}

fn show(wb: &mut Workbench, f: Facts) {
    doctor::installed().update(|x| *x = f);
    wb.run_command("shell.doctor");
}

/// A gated bundled engine with a live watch session.
pub fn healthy(wb: &mut Workbench) {
    mid_watch(wb, "happy");
    show(wb, facts(bundled(), Some(native_probe())));
}

/// Nothing on the search path: every candidate is listed.
pub fn engine_absent(wb: &mut Workbench) {
    wb.load_snapshot(&snapshot::corpus_snapshot("happy"));
    show(wb, facts(EngineState::NotFound, None));
    wb.disclose(DegradedReason::EngineAbsent);
}

/// An older boris on PATH refused by the gate.
pub fn version_mismatch(wb: &mut Workbench) {
    wb.load_snapshot(&snapshot::corpus_snapshot("happy"));
    show(
        wb,
        facts(
            EngineState::VersionMismatch {
                source: EngineSource::Path,
                path: "/usr/local/bin/boris".into(),
                found: "boris/0.8.2".into(),
                found_vcs: Some("ce40d78".into()),
            },
            None,
        ),
    );
    wb.disclose(DegradedReason::EngineVersionMismatch {
        found: "0.8.2".into(),
        want: "0.9.0".into(),
    });
}

/// The engine is healthy, but no migration lab is on the search path.
pub fn lab_absent(wb: &mut Workbench) {
    wb.load_snapshot(&snapshot::corpus_snapshot("happy"));
    let mut f = facts(bundled(), Some(native_probe()));
    f.lab.state = LabState::Absent {
        searched: lab_candidates().into_iter().map(|(_, path)| path).collect(),
    };
    show(wb, f);
    wb.set_nav("lab");
    wb.select_key("lab.status");
}

/// An older migration lab on PATH is refused by the pin gate.
pub fn lab_mismatch(wb: &mut Workbench) {
    wb.load_snapshot(&snapshot::corpus_snapshot("happy"));
    let mut f = facts(bundled(), Some(native_probe()));
    f.lab.state = LabState::Mismatch {
        found: "boris-migration-lab/0.8.0".into(),
        want: super::lab_pin().version,
        path: "/usr/local/bin/boris-migration-lab".into(),
    };
    show(wb, f);
    wb.set_nav("lab");
    wb.select_key("lab.status");
}

/// `.cv/imports` is a healthy fresh namespace: no migration runs yet.
pub fn imports_missing(wb: &mut Workbench) {
    wb.load_snapshot(&snapshot::corpus_snapshot("happy"));
    let mut f = facts(bundled(), Some(native_probe()));
    for ns in &mut f.store.namespaces {
        if ns.name == "imports" {
            ns.stamp = Stamp::Absent;
        }
    }
    f.store.imports = Some(ImportsHealth {
        runs: Some(0),
        ..ImportsHealth::default()
    });
    show(wb, f);
    wb.set_nav("store");
    wb.select_key("store.imports.runs");
}

/// `.cv/imports` exists but its evidence is degraded: the scan refuses a
/// symlinked sidecar and the namespace stamp is newer than this build.
pub fn imports_degraded(wb: &mut Workbench) {
    wb.load_snapshot(&snapshot::corpus_snapshot("happy"));
    let mut f = facts(bundled(), Some(native_probe()));
    for ns in &mut f.store.namespaces {
        if ns.name == "imports" {
            ns.stamp = Stamp::Version(9);
        }
    }
    f.store.imports = Some(ImportsHealth {
        scan_error: Some(".cv is a symlink; refusing to follow it".into()),
        review_error: Some(".cv is a symlink; refusing to follow it".into()),
        ..ImportsHealth::default()
    });
    show(wb, f);
    wb.set_nav("store");
    wb.select_key("store.ns.imports");
}

/// The watch failed fatally on a broken page; the one-shot build named
/// the diagnostics. The Watch section is open on that failure.
pub fn watch_dead(wb: &mut Workbench) {
    open_broken_recovered(wb, "broken-frontmatter");
    wb.disclose(DegradedReason::WatchDead);
    show(wb, facts(bundled(), Some(native_probe())));
    wb.set_nav("watch");
    wb.select_key("watch.fatal");
}
