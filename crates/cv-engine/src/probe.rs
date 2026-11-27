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

/// The one-shot validation probe argv for a content input root.
pub fn validate_input_args(input: &str) -> Vec<String> {
    vec!["validate".into(), "--input".into(), input.into()]
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

/// Whether the real staged-write parent can create a temporary file.
/// This uses the same confined-parent resolution as `write_content` and a
/// temporary file in that directory, then removes both the temporary file
/// (by drop) and any input directories the probe itself had to create.
fn probe_write_capability(root: &Path, content_input: &Path) -> (bool, Option<std::io::Error>) {
    let target = content_input.join(".cv-write-probe");
    // Deepest first: remove only directories missing before the probe.
    let mut created = Vec::new();
    let mut missing = root.join(content_input);
    while !missing.exists() {
        created.push(missing.clone());
        if !missing.pop() {
            break;
        }
    }
    let run = || -> std::io::Result<()> {
        let target = cv_store::paths::create_confined_parent(root, &target)?;
        let parent = target.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "content input has no parent",
            )
        })?;
        let file = tempfile::Builder::new()
            .prefix(".cv-write-")
            .suffix(".tmp")
            .tempfile_in(parent)?;
        drop(file);
        Ok(())
    };
    let result = run();
    for dir in created {
        let _ = std::fs::remove_dir(dir);
    }
    match result {
        Ok(()) => (true, None),
        Err(e) => (false, Some(e)),
    }
}

/// Probe the gated binary. `root` is the workspace and `content_input`
/// the same input root live writes use. The watch/validate probes run in a
/// scratch dir so they can never touch the user's `dist/`; the write probe
/// creates and removes only a temporary file in the real input parent.
pub fn probe(bin: &Path, root: &Path, content_input: &Path, timeout: Duration) -> CapabilityReport {
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
            v.args(validate_input_args(&input));
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

    let (writes, write_err) = probe_write_capability(root, content_input);
    caps.writes = writes;
    if let Some(e) = write_err {
        evidence.push(format!("content input not writable: {e}"));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn content_write_capability_uses_the_input_parent_not_the_workspace_root() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let content = root.path().join("content");
        std::fs::create_dir(&content).unwrap();
        let mut mode = std::fs::metadata(&content).unwrap().permissions();
        mode.set_mode(0o555);
        std::fs::set_permissions(&content, mode).unwrap();

        assert!(!probe_write_capability(root.path(), Path::new("content")).0);

        let mut mode = std::fs::metadata(&content).unwrap().permissions();
        mode.set_mode(0o755);
        std::fs::set_permissions(&content, mode).unwrap();
        assert!(probe_write_capability(root.path(), Path::new("content")).0);
        assert!(
            root.path()
                .read_dir()
                .unwrap()
                .all(|e| { e.unwrap().file_name() == "content" }),
            "probe left files in the workspace"
        );
    }
}
