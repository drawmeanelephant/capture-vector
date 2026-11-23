//! Per-OS capability probe, run once at startup against the gated binary.
//! Nothing is assumed from `cfg!(windows)`: watch support is whatever the
//! binary demonstrably does here. When it can't watch, the engine runs
//! one-shot builds under a portable file watcher instead.

use crate::proc::{CancelToken, Spawned, boris_command, run_captured};
use cv_contracts::watch::{WatchEvent, decode_line};
use cv_core::engine::{EngineCaps, ExitClass};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Stdio;
use std::sync::mpsc;
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
    let mut child = match Spawned::spawn(cmd, Stdio::null()) {
        Ok(c) => c,
        Err(e) => {
            return WatchProbe::Refused {
                why: e.to_string(),
                usage: false,
            };
        }
    };
    let _stdout = crate::proc::drain(child.take_stdout().expect("piped"));
    let stderr = child.take_stderr().expect("piped");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + timeout;
    let verdict = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left.min(Duration::from_millis(50))) {
            Ok(line) => match decode_line(&line) {
                Ok(None) => continue,
                Ok(Some(WatchEvent::Hello {
                    watch_events_schema,
                    compiler,
                })) if watch_events_schema == WATCH_SCHEMA => break WatchProbe::Hello { compiler },
                Ok(Some(WatchEvent::Hello {
                    watch_events_schema,
                    ..
                })) => {
                    break WatchProbe::Refused {
                        why: format!("watch_events_schema {watch_events_schema} != {WATCH_SCHEMA}"),
                        usage: false,
                    };
                }
                // Anything before a hello voids the handshake.
                _ => {
                    break WatchProbe::Refused {
                        why: format!("first record was not hello: {line:?}"),
                        usage: false,
                    };
                }
            },
            Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => {
                if let Ok(Some(status)) = child.try_wait() {
                    // Give the reader a moment to flush the last lines.
                    if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
                        if let Ok(Some(WatchEvent::Hello {
                            compiler,
                            watch_events_schema,
                        })) = decode_line(&line)
                            && watch_events_schema == WATCH_SCHEMA
                        {
                            break WatchProbe::Hello { compiler };
                        }
                        break WatchProbe::Refused {
                            why: format!("exited {status} after {line:?}"),
                            usage: status.code() == Some(2),
                        };
                    }
                    break WatchProbe::Refused {
                        why: format!("exited {status} before hello"),
                        usage: status.code() == Some(2),
                    };
                }
            }
            Err(_) => {
                break WatchProbe::Refused {
                    why: format!("no hello within {timeout:?}"),
                    usage: false,
                };
            }
        }
    };
    child.kill_tree(Duration::ZERO);
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
