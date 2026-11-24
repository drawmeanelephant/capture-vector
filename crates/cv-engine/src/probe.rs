//! Per-OS capability probe, run once at startup against the gated binary.
//! Nothing is assumed from `cfg!(windows)`: watch support is whatever the
//! binary demonstrably does here. When it can't watch, the engine runs
//! one-shot builds under a portable file watcher instead.

use crate::proc::{CancelToken, Spawned, boris_command, run_captured};
use crate::watch::{HandshakeFault, LINE_LIMIT, handshake, line_reader};
use cv_core::engine::{EngineCaps, ExitClass};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{Duration, Instant};

pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
pub const WATCH_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WatchMode {
    /// `boris watch --watch-json` performed the schema-1 handshake.
    Native,
    /// Probe failed: one-shot `boris build` per change via file watcher.
    OneShotFallback,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityReport {
    pub os: String,
    pub caps: EngineCaps,
    pub watch_mode: WatchMode,
    /// Human-readable record of what each probe observed (surfaced in
    /// Doctor and the ledger).
    pub evidence: Vec<String>,
}

enum WatchProbe {
    Hello { compiler: String },
    Refused { why: String, usage: bool },
}

/// Spawn `boris watch --watch-json <extra>` in a scratch workspace and
/// wait for the handshake on stderr; kill the tree either way.
fn probe_watch(bin: &Path, scratch: &Path, extra: &[&str], timeout: Duration) -> WatchProbe {
    let mut cmd = boris_command(bin, scratch);
    cmd.args(["watch", "--watch-json"]).args(extra);
    let mut child = match Spawned::spawn_watch(cmd) {
        Ok(c) => c,
        Err(e) => {
            return WatchProbe::Refused {
                why: e.to_string(),
                usage: false,
            };
        }
    };
    let (rx, reader) = line_reader(child.take_stderr().expect("piped"), LINE_LIMIT);
    let hello = handshake(&rx, timeout);
    drop(rx);
    let verdict = match hello {
        Ok(compiler) => WatchProbe::Hello { compiler },
        Err(e) => {
            // An older binary can reject --serve as a usage error. Give
            // the leader a moment to exit after its stderr diagnostic.
            // Protocol faults and timeouts never qualify for this retry.
            let mut usage = false;
            if matches!(e, HandshakeFault::FirstRecord | HandshakeFault::Closed) {
                let deadline = Instant::now() + Duration::from_millis(200);
                loop {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            usage = status.code() == Some(2);
                            break;
                        }
                        Ok(None) if Instant::now() < deadline => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        _ => break,
                    }
                }
            }
            WatchProbe::Refused {
                why: e.to_string(),
                usage,
            }
        }
    };
    child.kill_tree(Duration::ZERO);
    let _ = reader.join();
    verdict
}

/// Probe the gated binary. `root` is the workspace (checked for
/// writability only — the watch/validate probes run in a scratch dir so a
/// probe can never touch the user's `dist/`).
pub fn probe(bin: &Path, root: &Path, timeout: Duration) -> CapabilityReport {
    let mut evidence = Vec::new();
    let scratch = tempfile::Builder::new().prefix("cv-probe-").tempdir();
    let mut caps = EngineCaps::default();

    match &scratch {
        Ok(s) => {
            let content = s.path().join("content");
            let _ = std::fs::create_dir_all(&content);
            let input = content.to_string_lossy().into_owned();

            match probe_watch(
                bin,
                s.path(),
                &["--serve", "--port", "0", "--input", &input],
                timeout,
            ) {
                WatchProbe::Hello { compiler } => {
                    caps.watch = true;
                    caps.serve = true;
                    evidence.push(format!(
                        "watch+serve: hello schema {WATCH_SCHEMA} from {compiler}"
                    ));
                }
                WatchProbe::Refused { why, usage } => {
                    evidence.push(format!("watch --serve: {why}"));
                    if usage {
                        match probe_watch(bin, s.path(), &["--input", &input], timeout) {
                            WatchProbe::Hello { compiler } => {
                                caps.watch = true;
                                evidence.push(format!(
                                    "watch: hello schema {WATCH_SCHEMA} from {compiler}"
                                ));
                            }
                            WatchProbe::Refused { why, .. } => {
                                evidence.push(format!("watch: {why}"))
                            }
                        }
                    }
                }
            }

            let mut v = boris_command(bin, s.path());
            v.args(["validate", "--input", &input]);
            match run_captured(v, None, &CancelToken::new()) {
                Ok(c) => {
                    let class = c.outcome.class();
                    caps.authoring = !matches!(class, Some(ExitClass::UsageError) | None);
                    evidence.push(format!("validate: {:?}", c.outcome));
                }
                Err(e) => evidence.push(format!("validate: {e}")),
            }
        }
        Err(e) => evidence.push(format!("scratch dir: {e}")),
    }

    match tempfile::NamedTempFile::new_in(root) {
        Ok(_) => caps.writes = true,
        Err(e) => evidence.push(format!("workspace not writable: {e}")),
    }

    CapabilityReport {
        os: std::env::consts::OS.to_string(),
        caps,
        watch_mode: if caps.watch {
            WatchMode::Native
        } else {
            WatchMode::OneShotFallback
        },
        evidence,
    }
}
