//! fake-boris installer: a temp `bin/` holding the shim plus the recorded
//! outputs it replays, and a temp workspace root.
#![allow(dead_code)]

use cv_engine::locate::{Locator, PINNED};
use cv_engine::{EngineConfig, ProcessEngine};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(unix)]
const SHIM_SH: &str = include_str!("../fake-boris/boris");
#[cfg(windows)]
const SHIM_BAT: &str = include_str!("../fake-boris/boris.bat");

/// Never open the executable inode for writing in this process: a sibling
/// fork could inherit that fd and make Linux exec fail with ETXTBSY
/// (rust-lang/rust#114554). The destination writer belongs only to `cp`.
#[cfg(unix)]
pub fn install_executable(bin: &Path, contents: &str) {
    use std::os::unix::fs::PermissionsExt;

    let staging = bin.parent().unwrap().join("staging-contents");
    std::fs::write(&staging, contents).unwrap();
    let status = std::process::Command::new("cp")
        .arg(&staging)
        .arg(bin)
        .status()
        .unwrap();
    assert!(status.success(), "cp into the executable path failed");
    std::fs::remove_file(&staging).unwrap();
    std::fs::set_permissions(bin, std::fs::Permissions::from_mode(0o755)).unwrap();
}

pub struct FakeBoris {
    pub bin_dir: tempfile::TempDir,
    pub root: tempfile::TempDir,
    pub bin: PathBuf,
}

fn engine_version_lines() -> (String, String) {
    // The captured pin is the `--build-info` record; `--version` prints
    // its `version` field.
    let raw =
        std::fs::read_to_string(cv_fixtures::fixtures_root().join("engine-version.txt")).unwrap();
    let build_info = raw
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap()
        .to_string();
    let v: serde_json::Value = serde_json::from_str(&build_info).unwrap();
    (v["version"].as_str().unwrap().to_string(), build_info)
}

impl FakeBoris {
    /// Pinned identity (from the captured `engine-version.txt`), no corpus.
    pub fn new() -> Self {
        let bin_dir = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let bin = {
            let p = bin_dir.path().join("boris.bat");
            std::fs::write(&p, SHIM_BAT).unwrap();
            p
        };
        #[cfg(unix)]
        let bin = {
            let p = bin_dir.path().join("boris");
            install_executable(&p, SHIM_SH);
            p
        };
        let (version, build_info) = engine_version_lines();
        let f = Self { bin_dir, root, bin };
        f.set("version.txt", &version);
        f.set("build-info.txt", &build_info);
        f.set("verbs/validate/exit-code.txt", "0");
        f
    }

    /// Replay one captured corpus (hash-verified via the manifest).
    pub fn corpus(name: &str) -> Self {
        let f = Self::new();
        f.load_corpus(name);
        f
    }

    pub fn load_corpus(&self, name: &str) {
        let c = cv_fixtures::corpus(name);
        self.set_bytes("watch.ndjson", &c.file("watch/session.ndjson"));
        for verb in ["build", "validate", "check"] {
            for f in ["stdout.txt", "stderr.txt", "exit-code.txt"] {
                self.set_bytes(
                    &format!("verbs/{verb}/{f}"),
                    &c.file(&format!("{verb}/{f}")),
                );
            }
        }
        let ir = cv_fixtures::ir_capture(name);
        for f in ["stdout.txt", "stderr.txt", "exit-code.txt"] {
            self.set_bytes(&format!("verbs/build-out/{f}"), &ir.file(f));
        }
        if cv_fixtures::fixtures_root()
            .join("ir/captures")
            .join(name)
            .join("graph.json")
            .is_file()
        {
            self.set_bytes("verbs/build-out/graph.json", &ir.file("graph.json"));
        }
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.bin_dir.path().join(rel)
    }

    pub fn set_bytes(&self, rel: &str, bytes: &[u8]) {
        let p = self.path(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    pub fn set(&self, rel: &str, s: &str) {
        self.set_bytes(rel, s.as_bytes());
    }

    pub fn unset(&self, rel: &str) {
        let _ = std::fs::remove_file(self.path(rel));
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path(rel)).unwrap_or_default()
    }

    pub fn config(&self) -> EngineConfig {
        let mut cfg = EngineConfig::new(self.root.path());
        cfg.locator = Locator {
            env_override: Some(self.bin.clone()),
            ..Locator::default()
        };
        cfg.pin = PINNED;
        cfg.handshake_timeout = Duration::from_secs(20);
        cfg.probe_timeout = Duration::from_secs(20);
        cfg.poll_interval = Duration::from_millis(50);
        cfg
    }

    pub fn engine(&self) -> ProcessEngine {
        ProcessEngine::start(self.config()).expect("fake boris passes the gate")
    }

    pub fn root(&self) -> &Path {
        self.root.path()
    }
}

/// Collect envelopes from a sink into a shared vec.
pub fn collector() -> (
    cv_core::engine::EventSink,
    std::sync::Arc<std::sync::Mutex<Vec<cv_core::events::Envelope>>>,
) {
    let v = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let v2 = v.clone();
    (Box::new(move |e| v2.lock().unwrap().push(e)), v)
}

pub fn payloads(v: &[cv_core::events::Envelope]) -> Vec<cv_core::events::engine::EngineEvent> {
    v.iter()
        .map(|e| match &e.payload {
            cv_core::events::AppEvent::Engine(p) => p.clone(),
            other => panic!("non-engine payload {other:?}"),
        })
        .collect()
}

pub fn wait_until(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + timeout;
    while !f() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
