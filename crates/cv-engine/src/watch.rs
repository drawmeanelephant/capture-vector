//! Native watch: `boris watch --watch-json`, NDJSON on **stderr**.
//! The first non-blank record must be `hello` with schema 1 or the child
//! tree is killed and `watch()` fails. Undecodable lines become
//! `MalformedLine`; an oversized or incomplete frame ends the session.

use crate::bounded::append;
use crate::emit::{Emitter, map_watch_event};
use crate::error::{EngineError, Result};
use crate::exit::ExitOutcome;
use crate::probe::WATCH_SCHEMA;
use crate::proc::{STOP_GRACE, Spawned, boris_command};
use cv_contracts::watch::{WatchEvent, decode_line};
use cv_core::engine::{EventSink, WatchHandle};
use cv_core::events::engine::{EngineEvent, WatchEndReason, WatchExit};
use cv_core::events::{BuildId, SessionId};
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long a leader may outlive its closed stderr before we kill it.
const EXIT_GRACE: Duration = Duration::from_secs(5);
/// Maximum raw stderr bytes per frame, excluding LF (including any CR).
pub(crate) const LINE_LIMIT: usize = 1024 * 1024;
/// Bound not just each record but also the handoff to the decoder.
const PENDING_FRAMES: usize = 4;

#[derive(Debug, thiserror::Error)]
pub(crate) enum FrameFault {
    #[error("watch stderr record exceeded the {limit}-byte line limit")]
    TooLong { limit: usize },
    #[error("watch stderr ended with an incomplete record (missing newline)")]
    Incomplete,
    #[error("could not read watch stderr")]
    ReadFailed,
}

pub(crate) enum Frame {
    Line(String),
    InvalidUtf8(std::string::FromUtf8Error),
    Fault(FrameFault),
}

/// Read only a complete LF-terminated record. Do not wait for the end of
/// an oversized frame or expose its prefix, even if the prefix is JSON.
fn read_frame(
    r: &mut impl BufRead,
    limit: usize,
) -> std::result::Result<Option<Vec<u8>>, FrameFault> {
    let mut record = Vec::new();
    loop {
        let chunk = match r.fill_buf() {
            Ok(chunk) => chunk,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(FrameFault::ReadFailed),
        };
        if chunk.is_empty() {
            return if record.is_empty() {
                Ok(None)
            } else {
                Err(FrameFault::Incomplete)
            };
        }
        let newline = chunk.iter().position(|&byte| byte == b'\n');
        let len = newline.unwrap_or(chunk.len());
        if !append(&mut record, &chunk[..len], limit) {
            return Err(FrameFault::TooLong { limit });
        }
        r.consume(len + usize::from(newline.is_some()));
        if newline.is_some() {
            if record.last() == Some(&b'\r') {
                record.pop();
            }
            return Ok(Some(record));
        }
    }
}

pub(crate) fn line_reader<R: Read + Send + 'static>(
    r: R,
    limit: usize,
) -> (Receiver<Frame>, JoinHandle<()>) {
    let (tx, rx) = sync_channel(PENDING_FRAMES);
    let reader = std::thread::spawn(move || {
        let mut r = BufReader::new(r);
        loop {
            match read_frame(&mut r, limit) {
                Ok(Some(bytes)) => {
                    let frame = match String::from_utf8(bytes) {
                        Ok(line) => Frame::Line(line),
                        Err(e) => Frame::InvalidUtf8(e),
                    };
                    if tx.send(frame).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(fault) => {
                    let _ = tx.send(Frame::Fault(fault));
                    break;
                }
            }
        }
    });
    (rx, reader)
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
        if let Some(p) = self.pump.take() {
            self.stopped.store(true, Ordering::SeqCst);
            self.child.lock().unwrap().kill_tree(STOP_GRACE);
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

#[derive(Debug, thiserror::Error)]
pub(crate) enum HandshakeFault {
    #[error("no hello within {0:?}")]
    Timeout(Duration),
    #[error("stderr closed before hello")]
    Closed,
    #[error("first record was not hello")]
    FirstRecord,
    #[error("watch_events_schema {0} unsupported (need {WATCH_SCHEMA})")]
    Schema(u32),
    #[error(transparent)]
    Frame(#[from] FrameFault),
}

pub(crate) fn handshake(
    rx: &Receiver<Frame>,
    timeout: Duration,
) -> std::result::Result<String, HandshakeFault> {
    let deadline = Instant::now() + timeout;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(HandshakeFault::Timeout(timeout));
        }
        let left = deadline - now;
        match rx.recv_timeout(left) {
            Ok(Frame::Line(line)) => match decode_line(&line) {
                Ok(None) => continue,
                Ok(Some(WatchEvent::Hello {
                    watch_events_schema,
                    compiler,
                })) => {
                    return if watch_events_schema == WATCH_SCHEMA {
                        Ok(compiler)
                    } else {
                        Err(HandshakeFault::Schema(watch_events_schema))
                    };
                }
                _ => return Err(HandshakeFault::FirstRecord),
            },
            Ok(Frame::InvalidUtf8(_)) => return Err(HandshakeFault::FirstRecord),
            Ok(Frame::Fault(fault)) => return Err(fault.into()),
            Err(RecvTimeoutError::Timeout) => return Err(HandshakeFault::Timeout(timeout)),
            Err(RecvTimeoutError::Disconnected) => return Err(HandshakeFault::Closed),
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

fn exit_after_stderr(child: &Mutex<Spawned>, stopped: &AtomicBool) -> Option<ExitOutcome> {
    // The leader may still be tearing down (cmd.exe on Windows lags
    // its handle close). Give it a grace period after stderr EOF.
    let deadline = Instant::now() + EXIT_GRACE;
    loop {
        let mut c = child.lock().unwrap();
        match c.try_wait() {
            Ok(Some(_)) => return c.outcome(),
            Ok(None) if stopped.load(Ordering::SeqCst) || Instant::now() < deadline => {
                drop(c);
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                c.kill_tree(Duration::ZERO);
                return c.outcome();
            }
        }
    }
}

pub(crate) fn start(
    w: WatchStart<'_>,
    emitter: Arc<Emitter>,
    sink: EventSink,
) -> Result<ProcessWatch> {
    start_with_limit(w, emitter, sink, LINE_LIMIT)
}

fn start_with_limit(
    w: WatchStart<'_>,
    emitter: Arc<Emitter>,
    mut sink: EventSink,
    limit: usize,
) -> Result<ProcessWatch> {
    let mut cmd = boris_command(w.bin, w.root);
    cmd.args(["watch", "--watch-json"]).args(w.args);
    let mut child = Spawned::spawn_watch(cmd)?;
    let (rx, reader) = line_reader(child.take_stderr().expect("piped"), limit);

    let compiler = match handshake(&rx, w.handshake_timeout) {
        Ok(c) => c,
        Err(why) => {
            drop(rx);
            child.kill_tree(Duration::ZERO);
            let _ = reader.join();
            return Err(EngineError::Handshake(why.to_string()));
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
            let mut frame_fault = None;
            for frame in &rx {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                let line = match frame {
                    Frame::Line(line) => line,
                    Frame::InvalidUtf8(invalid_utf8) => {
                        emitter.emit(
                            &mut sink,
                            Some(&session),
                            build.clone(),
                            EngineEvent::MalformedLine {
                                raw: String::from_utf8_lossy(invalid_utf8.as_bytes()).into_owned(),
                            },
                        );
                        continue;
                    }
                    Frame::Fault(fault) => {
                        frame_fault = Some(fault);
                        break;
                    }
                };
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
            // Release any sender blocked behind the bounded handoff.
            drop(rx);
            let reader_failed = reader.join().is_err();
            let (reason, exit) = if let Some(fault) = frame_fault {
                child.lock().unwrap().kill_tree(Duration::ZERO);
                emitter.emit(
                    &mut sink,
                    Some(&session),
                    build,
                    EngineEvent::MalformedLine {
                        raw: fault.to_string(),
                    },
                );
                (WatchEndReason::Desync, child.lock().unwrap().outcome())
            } else {
                let outcome = exit_after_stderr(&child, &stopped);
                if stopped.load(Ordering::SeqCst) {
                    (WatchEndReason::Killed, outcome)
                } else if !reader_failed && outcome == Some(ExitOutcome::Exited { code: 0 }) {
                    (WatchEndReason::CleanExit, outcome)
                } else {
                    (WatchEndReason::Desync, outcome)
                }
            };
            let exit = exit.map(|o| match o {
                ExitOutcome::Exited { code } => WatchExit::Exited { code },
                ExitOutcome::Signaled { signal } => WatchExit::Signaled { signal },
                ExitOutcome::Killed => WatchExit::Killed,
            });
            live.store(false, Ordering::SeqCst);
            emitter.emit(
                &mut sink,
                Some(&session),
                None,
                EngineEvent::WatchSessionEnded { reason, exit },
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

#[cfg(test)]
mod tests {
    use super::*;
    use cv_core::events::AppEvent;
    use cv_core::id::WorkspaceId;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn frame_limit_applies_across_buffer_chunks_and_does_not_accept_partial_frames() {
        let mut r = BufReader::with_capacity(2, &b"abc\r\nnext\nlast"[..]);
        assert_eq!(read_frame(&mut r, 4).unwrap().as_deref(), Some(&b"abc"[..]));
        assert_eq!(
            read_frame(&mut r, 4).unwrap().as_deref(),
            Some(&b"next"[..])
        );
        assert!(matches!(read_frame(&mut r, 4), Err(FrameFault::Incomplete)));

        let mut r = BufReader::with_capacity(2, &b"abcde\n"[..]);
        assert!(matches!(
            read_frame(&mut r, 4),
            Err(FrameFault::TooLong { limit: 4 })
        ));
    }

    /// A newline-free source that never ends, counting what was pulled.
    struct Endless(Arc<AtomicUsize>);
    impl Read for Endless {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            buf.fill(b'a');
            self.0.fetch_add(buf.len(), Ordering::SeqCst);
            Ok(buf.len())
        }
    }

    #[test]
    fn oversized_frame_is_rejected_without_reading_to_the_next_newline() {
        let pulled = Arc::new(AtomicUsize::new(0));
        let mut r = BufReader::with_capacity(64, Endless(pulled.clone()));
        assert!(matches!(
            read_frame(&mut r, 1024),
            Err(FrameFault::TooLong { limit: 1024 })
        ));
        assert!(pulled.load(Ordering::SeqCst) <= 1024 + 64);
    }

    #[test]
    fn line_reader_ends_on_an_endless_line_after_reading_only_the_limit() {
        let pulled = Arc::new(AtomicUsize::new(0));
        let (rx, reader) = line_reader(Endless(pulled.clone()), 1024);
        let frame = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            frame,
            Frame::Fault(FrameFault::TooLong { limit: 1024 })
        ));
        reader.join().unwrap();
        assert!(rx.recv().is_err());
        // BufReader's default capacity is the only read-ahead allowed.
        assert!(pulled.load(Ordering::SeqCst) <= 1024 + 8 * 1024);
    }

    #[test]
    fn multibyte_utf8_split_across_buffer_chunks_still_decodes() {
        let mut r = BufReader::with_capacity(2, "\u{20ac}\u{20ac}\n".as_bytes());
        let bytes = read_frame(&mut r, 6).unwrap().unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), "\u{20ac}\u{20ac}");
    }

    #[test]
    fn reader_backpressures_and_unblocks_when_receiver_is_dropped() {
        struct OneLinePerRead(Arc<AtomicUsize>);
        impl Read for OneLinePerRead {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let i = self.0.fetch_add(1, Ordering::SeqCst);
                if i == 10 {
                    return Ok(0);
                }
                buf[..2].copy_from_slice(b"x\n");
                Ok(2)
            }
        }
        let reads = Arc::new(AtomicUsize::new(0));
        let (rx, reader) = line_reader(OneLinePerRead(reads.clone()), 4);
        let deadline = Instant::now() + Duration::from_secs(5);
        while reads.load(Ordering::SeqCst) < PENDING_FRAMES + 1 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(reads.load(Ordering::SeqCst), PENDING_FRAMES + 1);
        drop(rx);
        reader.join().unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), PENDING_FRAMES + 1);
    }

    #[test]
    fn oversized_record_cannot_be_a_hello_or_a_build() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        #[cfg(unix)]
        let bin = {
            use std::os::unix::fs::PermissionsExt;
            let bin = dir.join("boris");
            std::fs::write(&bin, include_str!("../tests/fake-boris/boris")).unwrap();
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            bin
        };
        #[cfg(windows)]
        let bin = {
            let bin = dir.join("boris.bat");
            std::fs::write(&bin, include_str!("../tests/fake-boris/boris.bat")).unwrap();
            bin
        };
        let capture = cv_fixtures::corpus("happy").file_str("watch/session.ndjson");
        let lines: Vec<_> = capture.lines().collect();
        let limit = lines.iter().map(|l| l.len()).max().unwrap();
        let padded = |line: &str| format!("{line:width$} \n", width = limit);
        let output = dir.join("watch.ndjson");
        let emitter = Arc::new(Emitter::new(WorkspaceId("ws".into()), 10));
        let live = Arc::new(AtomicBool::new(false));
        let config = || WatchStart {
            bin: &bin,
            root: dir,
            args: &[],
            handshake_timeout: Duration::from_secs(5),
            live: live.clone(),
        };
        std::fs::write(&output, format!("{}{capture}", padded(lines[0]))).unwrap();
        assert!(matches!(
            start_with_limit(config(), emitter.clone(), Box::new(|_| {}), limit),
            Err(EngineError::Handshake(why)) if why.contains("line limit")
        ));
        assert!(!live.load(Ordering::SeqCst));

        // Use an unmodified hello/build-started, then a valid build record
        // with excess trailing whitespace. Neither the valid-looking prefix
        // nor any later records can be applied to the desynced session.
        let text = format!(
            "{}\n{}\n{}{}\n",
            lines[0],
            lines[1],
            padded(lines[2]),
            lines[2],
        );
        std::fs::write(output, text).unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        let _watch = start_with_limit(
            config(),
            emitter,
            Box::new(move |env| tx.try_send(env.payload).unwrap()),
            limit,
        )
        .unwrap();
        let timeout = Duration::from_secs(5);
        assert!(matches!(
            rx.recv_timeout(timeout).unwrap(),
            AppEvent::Engine(EngineEvent::WatchSessionStarted { .. })
        ));
        assert!(matches!(
            rx.recv_timeout(timeout).unwrap(),
            AppEvent::Engine(EngineEvent::MalformedLine { raw })
                if raw == format!("watch stderr record exceeded the {limit}-byte line limit")
        ));
        assert!(matches!(
            rx.recv_timeout(timeout).unwrap(),
            AppEvent::Engine(EngineEvent::WatchSessionEnded {
                reason: WatchEndReason::Desync,
                ..
            })
        ));
        assert!(!live.load(Ordering::SeqCst));
    }
}
