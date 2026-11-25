//! Doctor goldens: one per engine story. Facts are fixed strings, not
//! this machine's, so the trees match on every OS.

use crate::{mid_watch, open_broken, snapshot};
use cv_core::engine::{EngineCaps, EngineSource};
use cv_core::events::shell::DegradedReason;
use cv_model::modules::doctor::{
    self, AppBuild, EngineState, Facts, Located, Lock, LockHolder, Namespace, Probe, Stamp, Store,
};
use cv_model::workbench::Workbench;

const ROOT: &str = "/Users/ana/sites/happy";
const BUNDLED: &str = "/Applications/Capture Vector.app/Contents/Resources/engines/boris";
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
        ],
    }
}

fn facts(engine: EngineState, probe: Option<Probe>) -> Facts {
    Facts {
        engine,
        pin: super::pin(),
        candidates: candidates(),
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

/// The watch failed fatally on a broken page; the one-shot build named
/// the diagnostics. The Watch section is open on that failure.
pub fn watch_dead(wb: &mut Workbench) {
    open_broken(wb, "broken-frontmatter");
    wb.disclose(DegradedReason::WatchDead);
    show(wb, facts(bundled(), Some(native_probe())));
    wb.set_nav("watch");
    wb.select_key("watch.fatal");
}
