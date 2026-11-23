//! Native watch: `boris watch --watch-json`, NDJSON on **stderr**.
//! The first non-blank record must be `hello` with schema 1 or the child
//! tree is killed and `watch()` fails. Every later line becomes an event;
//! undecodable lines become `MalformedLine` (never dropped).

use crate::emit::{Emitter, map_watch_event};
use crate::error::{EngineError, Result};
use crate::exit::ExitOutcome;
use crate::probe::WATCH_SCHEMA;
use crate::proc::{STOP_GRACE, Spawned, boris_command, drain};
use cv_contracts::watch::{WatchEvent, decode_line};
use cv_core::engine::{EventSink, WatchHandle};
use cv_core::events::engine::{EngineEvent, WatchEndReason};
use cv_core::events::{BuildId, SessionId};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Read lines lossily: invalid UTF-8 must surface as a malformed line,
/// not end the stream.
pub(crate) fn line_reader<R: Read + Send + 'static>(r: R) -> Receiver<String> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mut r = BufReader::new(r);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match r.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf)
                        .trim_end_matches(['\n', '\r'])
                        .to_string();
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

pub struct ProcessWatch {
    session: SessionId,
    child: Arc<Mutex<Spawned>>,
    stopped: Arc<AtomicBool>,
    pump: Option<JoinHandle<()>>,
}

impl ProcessWatch {
    pub fn is_running(&self) -> bool {
        self.pump.as_ref().is_some_and(|p| !p.is_finished())
    }
}

impl WatchHandle for ProcessWatch {
    fn session(&self) -> &SessionId {
        &self.session
    }

    /// Kill the child's whole process group / job, then wait for the
    /// pump to emit `WatchSessionEnded { Killed }`.
    fn stop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.child.lock().unwrap().kill_tree(STOP_GRACE);
        if let Some(p) = self.pump.take() {
            let _ = p.join();
        }
    }
}

impl Drop for ProcessWatch {
    fn drop(&mut self) {
        if self.pump.is_some() {
            self.stop();
        }
    }
}

fn handshake(rx: &Receiver<String>, timeout: Duration) -> std::result::Result<String, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(line) => match decode_line(&line) {
                Ok(None) => continue,
                Ok(Some(WatchEvent::Hello {
                    watch_events_schema,
                    compiler,
                })) => {
                    return if watch_events_schema == WATCH_SCHEMA {
                        Ok(compiler)
                    } else {
                        Err(format!(
                            "watch_events_schema {watch_events_schema} unsupported (need {WATCH_SCHEMA})"
                        ))
                    };
                }
                _ => return Err(format!("first record was not hello: {line:?}")),
            },
            Err(RecvTimeoutError::Timeout) => return Err(format!("no hello within {timeout:?}")),
            Err(RecvTimeoutError::Disconnected) => return Err("stderr closed before hello".into()),
        }
    }
}

pub(crate) struct WatchStart<'a> {
    pub bin: &'a Path,
    pub root: &'a Path,
    pub args: &'a [String],
    pub handshake_timeout: Duration,
    pub live: Arc<AtomicBool>,
}

pub(crate) fn start(
    w: WatchStart<'_>,
    emitter: Arc<Emitter>,
    mut sink: EventSink,
) -> Result<ProcessWatch> {
    let mut cmd = boris_command(w.bin, w.root);
    cmd.args(["watch", "--watch-json"]).args(w.args);
    let mut child = Spawned::spawn(cmd, Stdio::null())?;
    // stdout carries nothing contractual in watch mode; drain so it can
    // never block the child.
    let _ = drain(child.take_stdout().expect("piped"));
    let rx = line_reader(child.take_stderr().expect("piped"));

    let compiler = match handshake(&rx, w.handshake_timeout) {
        Ok(c) => c,
        Err(why) => {
            child.kill_tree(Duration::ZERO);
            return Err(EngineError::Handshake(why));
        }
    };

    let session = emitter.rotate();
    emitter.emit(
        &mut sink,
        Some(&session),
        None,
        EngineEvent::WatchSessionStarted {
            schema: WATCH_SCHEMA,
            compiler,
        },
    );

    let child = Arc::new(Mutex::new(child));
    let stopped = Arc::new(AtomicBool::new(false));
    w.live.store(true, Ordering::SeqCst);
    let pump = {
        let (child, stopped, session, live) =
            (child.clone(), stopped.clone(), session.clone(), w.live);
        std::thread::spawn(move || {
            let mut build: Option<BuildId> = None;
            let mut builds = 0u64;
            for line in rx.iter() {
                let payload = match decode_line(&line) {
                    Ok(None) => continue,
                    Ok(Some(WatchEvent::BuildStarted { .. })) => {
                        builds += 1;
                        build = Some(BuildId(format!("{}:b{builds}", session.0)));
                        continue;
                    }
                    // A second hello mid-session is a protocol violation.
                    Ok(Some(WatchEvent::Hello { .. })) => EngineEvent::MalformedLine { raw: line },
                    Ok(Some(ev)) => match map_watch_event(&ev, build.as_ref()) {
                        Some(p) => p,
                        None => continue,
                    },
                    Err(_) => EngineEvent::MalformedLine { raw: line },
                };
                emitter.emit(&mut sink, Some(&session), build.clone(), payload);
            }
            // stderr EOF: the child (or at least its stderr) is gone.
            let outcome = loop {
                let mut c = child.lock().unwrap();
                match c.try_wait() {
                    Ok(Some(_)) => break c.outcome(),
                    Ok(None) if stopped.load(Ordering::SeqCst) => {
                        drop(c);
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Ok(None) => {
                        // A wrapper (cmd /c on Windows) can lag a beat
                        // behind its pipe's EOF. Give the real exit a
                        // bounded grace before declaring the leader
                        // outlived its stderr; a genuinely detached
                        // leader still gets the tree kill.
                        drop(c);
                        let deadline =
                            std::time::Instant::now() + Duration::from_secs(2);
                        break loop {
                            std::thread::sleep(Duration::from_millis(10));
                            let mut c = child.lock().unwrap();
                            match c.try_wait() {
                                Ok(Some(_)) => break c.outcome(),
                                Ok(None)
                                    if std::time::Instant::now() < deadline =>
                                {
                                    drop(c);
                                    continue;
                                }
                                Ok(None) => {
                                    c.kill_tree(Duration::ZERO);
                                    break None;
                                }
                                Err(_) => break None,
                            }
                        };
                    }
                    Err(_) => break None,
                }
            };
            let reason = if stopped.load(Ordering::SeqCst) {
                WatchEndReason::Killed
            } else if outcome == Some(ExitOutcome::Exited { code: 0 }) {
                WatchEndReason::CleanExit
            } else {
                WatchEndReason::Desync
            };
            live.store(false, Ordering::SeqCst);
            emitter.emit(
                &mut sink,
                Some(&session),
                None,
                EngineEvent::WatchSessionEnded { reason },
            );
        })
    };
    Ok(ProcessWatch {
        session,
        child,
        stopped,
        pump: Some(pump),
    })
}
