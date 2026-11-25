//! Migrate scenes. Each seeds a scratch workspace's `.cv/imports/` with
//! runs `StubLab` replays from the checked-in lab captures, then reads
//! them back through [`DiskPort`] like an opened workspace would.

use super::DiskPort;
use cv_fixtures::lab::StubLab;
use cv_model::modules::migrate::{LabState, MemoryPort, MigratePort};
use cv_model::workbench::Workbench;
use cv_store::imports::{self as store, Finished, RecordedLab, RunRecord};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Captured modes the populated scenes seed, oldest first.
pub const SEEDED: &[&str] = &["hugo-inventory", "jekyll", "substack", "wordpress", "hugo"];

const T0: i64 = 1_764_000_000_000;

/// Replay `mode` into `.cv/imports/<run id>/out` and record it.
pub fn seed_run(root: &Path, mode: &str, started_ms: i64) -> String {
    let id = format!("{started_ms}-{mode}");
    // The lab refuses an output path under a symlink (macOS /var).
    let root = &root.canonicalize().expect("scene root");
    let dir = store::run_dir(root, &id).expect("scene run id");
    let t = StubLab::new()
        .run(mode, &dir.join(store::OUT))
        .expect("captured lab mode");
    let mut rec = RunRecord::started(
        &id,
        mode,
        RecordedLab {
            tool_id: StubLab::new().tool_id(),
            source: "fixture".into(),
            path: "fixtures/captures/lab".into(),
        },
        started_ms,
    );
    rec.inputs = t
        .argv
        .iter()
        .filter(|a| !a.starts_with("--mode=") && !a.starts_with("--out="))
        .filter_map(|a| a.split_once('='))
        .map(|(f, p)| store::input(f, p))
        .collect();
    rec.argv = t.argv.clone();
    let pin = cv_engine::locate::PINNED;
    rec.boris_pin = Some(format!("{} @ {}", pin.version, pin.vcs_revision));
    rec.finished = Some(Finished {
        exit_code: Some(t.exit_code),
        exit: match t.exit_code {
            0 => "ok",
            2 => "usage",
            _ => "io",
        }
        .into(),
        outcome: format!("exited {}", t.exit_code),
        duration_ms: 0,
    });
    store::write(root, &rec).expect("scene run record");
    id
}

pub fn seed(root: &Path) {
    for (i, mode) in SEEDED.iter().enumerate() {
        seed_run(root, mode, T0 + i as i64 * 60_000);
    }
}

/// The newest run's `report.json` claims a schema this build does not
/// mirror.
pub fn corrupt(root: &Path) {
    let id = seed_run(root, "hugo", T0 + 3_600_000);
    let path = store::run_dir(root, &id)
        .unwrap()
        .join(store::OUT)
        .join("report.json");
    let raw = std::fs::read_to_string(&path).expect("scene report");
    let bumped = raw.replacen("\"schema_version\": 1", "\"schema_version\": 9", 1);
    assert_ne!(raw, bumped, "captured hugo report names its schema");
    std::fs::write(&path, bumped).expect("scene report");
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "cv-migrate-scene-{}-{}",
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

fn fixture_lab() -> LabState {
    LabState::Fixture {
        tool_id: StubLab::new().tool_id(),
    }
}

/// Read the seeded workspace once, then drop it: the scene keeps only
/// what the port loaded.
fn show(wb: &mut Workbench, lab: LabState, prepare: impl FnOnce(&Path)) {
    let dir = Scratch::new();
    prepare(&dir.0);
    let mut imports = DiskPort::open(&dir.0, lab).load();
    imports.store = "session only (scene)".into();
    *wb = crate::init_ports(
        Box::new(crate::publish::DiskPort::ephemeral()),
        Box::new(MemoryPort {
            imports,
            records: Vec::new(),
        }),
    );
    crate::boot(wb);
    wb.run_command("shell.goto.migrate");
}

pub fn empty(wb: &mut Workbench) {
    show(wb, fixture_lab(), |_| {});
}

pub fn runs(wb: &mut Workbench) {
    show(wb, fixture_lab(), seed);
    wb.select_row(0);
}

pub fn review(wb: &mut Workbench) {
    show(wb, fixture_lab(), seed);
    wb.set_nav("review");
    wb.select_row(0);
}

pub fn lab_absent(wb: &mut Workbench) {
    let searched = [
        "/usr/local/bin/boris-migration-lab",
        "/opt/homebrew/bin/boris-migration-lab",
    ];
    show(
        wb,
        LabState::Absent {
            searched: searched.iter().map(|s| s.to_string()).collect(),
        },
        seed,
    );
    wb.set_nav("new");
    wb.select_key("lab");
}

pub fn undecodable(wb: &mut Workbench) {
    show(wb, fixture_lab(), |root| {
        seed(root);
        corrupt(root);
    });
    wb.select_row(0);
}
