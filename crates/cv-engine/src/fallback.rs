//! Watch fallback, used only when the startup probe shows the binary
//! can't do `watch --watch-json` here: a portable polling file watcher
//! (std only — same strategy as Boris's own portable watcher) driving
//! one-shot `boris build` runs. Events use the same vocabulary as native
//! watch so consumers can't tell the difference beyond `compiler`.

use crate::emit::Emitter;
use crate::error::Result;
use crate::exit::ExitOutcome;
use crate::probe::WATCH_SCHEMA;
use crate::proc::CancelToken;
use crate::verbs::{VerbOutput, run_argv};
use cv_core::diagnostic::{Diagnostic, Severity};
use cv_core::engine::{EventSink, ExitClass, WatchHandle};
use cv_core::events::engine::{BuildPhase, EngineEvent, WatchEndReason};
use cv_core::events::{BuildId, SessionId};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

/// Output trees and tool dirs never trigger rebuilds (they are what
/// rebuilds write).
pub const IGNORED_DIRS: &[&str] = &[
    ".boris",
    ".boris-cache",
    ".cv",
    ".git",
    "rag",
    "context",
    "target",
    "node_modules",
];
pub const IGNORED_FILES: &[&str] = &["llms.txt", "rss.xml"];

pub type Tree = BTreeMap<PathBuf, (u64, Option<SystemTime>)>;

pub fn scan(root: &Path, html_dir: &str) -> Tree {
    let mut out = Tree::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(ft) = e.file_type() else { continue };
            let at_root = dir == root;
            if ft.is_dir() {
                if !(at_root && (IGNORED_DIRS.contains(&name.as_str()) || name == html_dir)) {
                    stack.push(e.path());
                }
            } else if !(name.ends_with(".tmp") || at_root && IGNORED_FILES.contains(&name.as_str()))
            {
                let meta = e.metadata().ok();
                out.insert(
                    e.path(),
                    (
                        meta.as_ref().map_or(0, |m| m.len()),
                        meta.and_then(|m| m.modified().ok()),
                    ),
                );
            }
        }
    }
    out
}

pub fn changed(a: &Tree, b: &Tree) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = a
        .iter()
        .filter(|(k, val)| b.get(*k) != Some(val))
        .map(|(k, _)| k.clone())
        .chain(b.keys().filter(|k| !a.contains_key(*k)).cloned())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// One-shot build result in watch vocabulary.
pub fn build_event(out: &VerbOutput, phase: BuildPhase, build: &BuildId) -> EngineEvent {
    let diagnostics: Vec<Diagnostic> = out
        .diagnostics()
        .iter()
        .map(|d| Diagnostic {
            build_version: Some(build.clone()),
            ..Diagnostic::from(d)
        })
        .collect();
    match out.outcome.class() {
        Some(ExitClass::Ok) => EngineEvent::BuildFinished {
            phase,
            pages_written: out.pages_written(),
            diagnostics: diagnostics
                .into_iter()
                .filter(|d| d.severity != Severity::Error)
                .collect(),
            duration_ms: out.duration_ms,
        },
        class => EngineEvent::BuildFailed {
            phase,
            diagnostics,
            // A content failure is fixed by the next edit; usage/I-O
            // failures won't be.
            recoverable: class == Some(ExitClass::ContentFailure),
        },
    }
}

pub struct FallbackWatch {
    session: SessionId,
    stop: Arc<AtomicBool>,
    cancel: CancelToken,
    thread: Option<JoinHandle<()>>,
}

impl WatchHandle for FallbackWatch {
    fn session(&self) -> &SessionId {
        &self.session
    }
    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.cancel.cancel();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for FallbackWatch {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.stop();
        }
    }
}

pub(crate) struct FallbackStart {
    pub bin: PathBuf,
    pub root: PathBuf,
    pub html_dir: String,
    pub build_args: Vec<String>,
    pub interval: Duration,
    pub compiler: String,
    pub live: Arc<AtomicBool>,
}

pub(crate) fn start(
    f: FallbackStart,
    emitter: Arc<Emitter>,
    mut sink: EventSink,
) -> Result<FallbackWatch> {
    let session = emitter.rotate();
    emitter.emit(
        &mut sink,
        Some(&session),
        None,
        EngineEvent::WatchSessionStarted {
            schema: WATCH_SCHEMA,
            compiler: format!("{} (cv one-shot fallback)", f.compiler),
        },
    );
    let stop = Arc::new(AtomicBool::new(false));
    let cancel = CancelToken::new();
    f.live.store(true, Ordering::SeqCst);
    let thread = {
        let (stop, cancel, session) = (stop.clone(), cancel.clone(), session.clone());
        std::thread::spawn(move || {
            let mut builds = 0u64;
            let mut tree = scan(&f.root, &f.html_dir);
            let mut phase = BuildPhase::Initial;
            let reason = 'run: loop {
                builds += 1;
                let build = BuildId(format!("{}:b{builds}", session.0));
                let mut args = vec!["build".to_string()];
                args.extend(f.build_args.iter().cloned());
                match run_argv(&f.bin, &f.root, &args, &cancel) {
                    Ok(out) => {
                        let ev = build_event(&out, phase, &build);
                        emitter.emit(&mut sink, Some(&session), Some(build), ev);
                        if out.outcome == ExitOutcome::Killed {
                            break WatchEndReason::Killed;
                        }
                    }
                    Err(_) if stop.load(Ordering::SeqCst) => break WatchEndReason::Killed,
                    Err(e) => {
                        emitter.emit(
                            &mut sink,
                            Some(&session),
                            Some(build),
                            EngineEvent::BuildFailed {
                                phase,
                                diagnostics: vec![Diagnostic {
                                    code: "CV-SPAWN".into(),
                                    severity: Severity::Error,
                                    span: None,
                                    message: e.to_string(),
                                    build_version: None,
                                }],
                                recoverable: false,
                            },
                        );
                        break WatchEndReason::Desync;
                    }
                }
                phase = BuildPhase::Rebuild;
                loop {
                    let mut slept = Duration::ZERO;
                    while slept < f.interval {
                        if stop.load(Ordering::SeqCst) {
                            break 'run WatchEndReason::Killed;
                        }
                        let step = Duration::from_millis(20).min(f.interval - slept);
                        std::thread::sleep(step);
                        slept += step;
                    }
                    let next = scan(&f.root, &f.html_dir);
                    if !changed(&tree, &next).is_empty() {
                        tree = next;
                        break;
                    }
                }
            };
            f.live.store(false, Ordering::SeqCst);
            emitter.emit(
                &mut sink,
                Some(&session),
                None,
                EngineEvent::WatchSessionEnded { reason, exit: None },
            );
        })
    };
    Ok(FallbackWatch {
        session,
        stop,
        cancel,
        thread: Some(thread),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_ignores_outputs_and_detects_edits() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        std::fs::create_dir_all(r.join("content")).unwrap();
        std::fs::create_dir_all(r.join("dist")).unwrap();
        std::fs::write(r.join("content/a.md"), "a").unwrap();
        let before = scan(r, "dist");
        std::fs::write(r.join("dist/index.html"), "x").unwrap();
        std::fs::write(r.join("llms.txt"), "x").unwrap();
        std::fs::write(r.join("content/.cv-write-1.tmp"), "x").unwrap();
        assert!(changed(&before, &scan(r, "dist")).is_empty());
        std::fs::write(r.join("content/a.md"), "ab").unwrap();
        std::fs::write(r.join("content/b.md"), "b").unwrap();
        let c = changed(&before, &scan(r, "dist"));
        assert_eq!(c.len(), 2);
    }
}
