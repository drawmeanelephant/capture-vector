//! Fills the Doctor's [`Feed`]: the engine lookup and probe (from
//! `cv-engine`'s typed results), the `.cv/` sidecar state (read-only
//! stamp reads), and this build's identity. The Doctor module renders it.

pub mod scene;

use crate::build_info;
use cv_core::engine::EngineSource;
use cv_engine::EngineError;
use cv_engine::locate::{EngineIdentity, Locator, PINNED};
use cv_engine::probe::{CapabilityReport, WatchMode};
use cv_model::modules::doctor::{
    self, AppBuild, EngineState, Feed, Located, Lock, LockHolder, Namespace, Pin, Probe, Stamp,
    Store,
};
use cv_store::sidecar;
use std::path::Path;

pub fn pin() -> Pin {
    Pin {
        version: PINNED.version.into(),
        vcs_revision: PINNED.vcs_revision.into(),
    }
}

pub fn app_build() -> AppBuild {
    AppBuild {
        version: build_info::version_line(),
        vcs_revision: build_info::VCS_REVISION.into(),
        target: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
        .into(),
        toolchain: build_info::toolchain().into(),
        logging: crate::logging::describe(),
    }
}

fn home() -> Option<String> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .map(|h| h.to_string_lossy().into_owned())
        .filter(|h| h.len() > 1)
}

/// A fresh feed for a window with `root` open (or none), installed so
/// the next registry's Doctor reads it.
pub fn install(root: Option<&Path>) -> Feed {
    let feed = Feed::default();
    feed.update(|f| {
        f.pin = pin();
        f.app = app_build();
        if let Some(r) = root {
            let r = r.display().to_string();
            f.store.root = Some(r.clone());
            f.redact.push((r, "<workspace>".into()));
        }
        f.redact.extend(home().map(|h| (h, "~".into())));
    });
    doctor::install(feed.clone());
    feed
}

pub fn candidates(l: &Locator) -> Vec<(EngineSource, String)> {
    l.candidates()
        .into_iter()
        .map(|(s, p)| (s, p.display().to_string()))
        .collect()
}

pub fn located(id: &EngineIdentity) -> EngineState {
    EngineState::Located(Located {
        source: id.source.clone(),
        path: id.path.display().to_string(),
        version: id.version.clone(),
        vcs_revision: id.vcs_revision.clone(),
        build_info_raw: id.build_info_raw.clone(),
    })
}

pub fn probe(r: &CapabilityReport) -> Probe {
    Probe {
        os: r.os.clone(),
        caps: r.caps,
        native_watch: r.watch_mode == WatchMode::Native,
        evidence: r.evidence.clone(),
    }
}

/// Why the gate refused, as Doctor shows it.
pub fn engine_state(e: &EngineError) -> EngineState {
    match e {
        EngineError::NotFound { .. } => EngineState::NotFound,
        EngineError::OverrideMissing(p) => EngineState::OverrideMissing {
            path: p.display().to_string(),
        },
        EngineError::VersionMismatch {
            path,
            source_kind,
            found,
            found_vcs,
            ..
        } => EngineState::VersionMismatch {
            source: source_kind.clone(),
            path: path.display().to_string(),
            found: found.clone(),
            found_vcs: found_vcs.clone(),
        },
        EngineError::BuildInfoInvalid { path, raw } => EngineState::BuildInfoInvalid {
            path: path.display().to_string(),
            raw: raw.clone(),
        },
        other => EngineState::Failed {
            error: other.to_string(),
        },
    }
}

/// How this window's open of one namespace went.
#[derive(Debug, Clone, Default)]
pub struct Opened {
    pub lock: Option<LockHolder>,
    pub error: Option<String>,
}

/// Per-namespace opens from `open_workspace`.
#[derive(Debug, Clone, Default)]
pub struct StoreOpen {
    pub research: Opened,
    pub audit: Opened,
    pub ledger: Opened,
}

fn stamp(root: &Path, ns: &sidecar::Namespace) -> Stamp {
    match sidecar::read_version(root, ns) {
        Ok(None) => Stamp::Absent,
        Ok(Some(0)) => Stamp::Unstamped,
        Ok(Some(v)) => Stamp::Version(v),
        Err(e) => Stamp::Unreadable(e.to_string()),
    }
}

/// Read every namespace's stamp (never writing), with what the opens
/// reported. Runs on the open path, never inside a frame.
pub fn store(root: &Path, opened: &StoreOpen) -> Store {
    let research_index = cv_store::paths::project_index_dir(root, "research")
        .ok()
        .map(|d| d.join(cv_notes::disk::INDEX).display().to_string());
    let audit_lock = format!(
        "{}/{}",
        cv_store::audit::DIR,
        Path::new(cv_store::audit::LIFECYCLE)
            .with_extension("lock")
            .display()
    );
    let ns = |s: &sidecar::Namespace, o: &Opened, lock: Option<String>, index: Option<String>| {
        Namespace {
            name: s.name.into(),
            dir: s.dir.into(),
            supported: s.current,
            stamp: stamp(root, s),
            lock: lock.zip(o.lock).map(|(path, holder)| Lock { path, holder }),
            index,
            error: o.error.clone(),
        }
    };
    Store {
        root: Some(root.display().to_string()),
        namespaces: vec![
            ns(
                &cv_notes::disk::SIDECAR,
                &opened.research,
                Some(cv_notes::disk::WRITER_LOCK.into()),
                research_index,
            ),
            ns(
                &cv_store::audit::SCHEMA,
                &opened.audit,
                Some(audit_lock),
                None,
            ),
            ns(&cv_ledger::journal::SIDECAR, &opened.ledger, None, None),
        ],
    }
}

/// The fixture demo: what the captured session says about its compiler.
pub fn fixture(corpus: &str, engine: &dyn cv_core::engine::Engine) -> EngineState {
    let info = engine.locate().ok();
    EngineState::Fixture {
        corpus: corpus.into(),
        version: info
            .as_ref()
            .map_or_else(|| "unknown".into(), |i| i.version.clone()),
        vcs_revision: info.and_then(|i| i.vcs_revision),
    }
}
