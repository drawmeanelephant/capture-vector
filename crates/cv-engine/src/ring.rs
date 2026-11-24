//! Bounded event ring + resumption. A consumer that fell behind asks for
//! everything after its cursor; if that cursor is from another session or
//! older than the ring holds, the answer is "resync from snapshot" — the
//! ring never grows to cover a slow reader.

use crate::bounded::append;
use cv_core::events::{Cursor, Envelope, SessionId};
use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::Mutex;

pub const DEFAULT_RING: usize = 1024;
/// Replay storage budget for encoded envelopes and their charged metadata.
pub const DEFAULT_RING_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
pub enum Resume {
    /// Events strictly after the cursor, in order (may be empty).
    Replay(Vec<Envelope>),
    /// Cursor can't be served (session changed or evicted): call
    /// `Engine::snapshot()` and restart from `ProcessEngine::cursor()`.
    Resync,
}

pub struct EventRing {
    cap: usize,
    byte_cap: usize,
    state: Mutex<State>,
}

struct Entry {
    seq: u64,
    data: Box<[u8]>,
}

impl Entry {
    fn bytes(&self) -> usize {
        self.data.len() + size_of::<Self>()
    }
}

#[derive(Default)]
struct State {
    session: Option<SessionId>,
    last_seq: Option<u64>,
    entries: VecDeque<Entry>,
    bytes: usize,
}

impl State {
    fn clear_entries(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
}

/// Encoding bounds the entire envelope, including nested diagnostics,
/// without allocating an arbitrarily large temporary JSON string.
struct EncodeWithin {
    data: Vec<u8>,
    limit: usize,
}

impl Write for EncodeWithin {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if !append(&mut self.data, bytes, self.limit) {
            return Err(io::Error::other("event exceeds replay budget"));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl EventRing {
    pub fn new(cap: usize) -> Self {
        Self::with_byte_capacity(cap, DEFAULT_RING_BYTES)
    }

    pub fn with_byte_capacity(cap: usize, byte_cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            byte_cap,
            state: Mutex::new(State::default()),
        }
    }

    pub fn capacity(&self) -> usize {
        self.cap
    }

    pub fn len(&self) -> usize {
        self.state.lock().unwrap().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn push(&self, env: Envelope) {
        self.record(&env);
    }

    pub(crate) fn record(&self, env: &Envelope) {
        let mut s = self.state.lock().unwrap();
        let session_bytes = env.cursor.session.0.len();
        if session_bytes > self.byte_cap {
            // Even the session metadata wouldn't fit: no replay possible.
            *s = State::default();
            return;
        }
        if s.session.as_ref() != Some(&env.cursor.session) {
            s.session = Some(env.cursor.session.clone());
            s.clear_entries();
        } else if s.last_seq.and_then(|seq| seq.checked_add(1)) != Some(env.cursor.seq) {
            // Never serve a suffix with an interior gap.
            s.clear_entries();
        }
        s.last_seq = Some(env.cursor.seq);
        let cap = self.byte_cap - session_bytes;
        let Some(limit) = cap.checked_sub(size_of::<Entry>()) else {
            s.clear_entries();
            return;
        };
        let mut encoded = EncodeWithin {
            data: Vec::new(),
            limit,
        };
        if serde_json::to_writer(&mut encoded, env).is_err() {
            // This record was delivered live but cannot be replayed.
            // Preserve only a contiguous suffix *after* the hole.
            s.clear_entries();
            return;
        }
        let entry = Entry {
            seq: env.cursor.seq,
            data: encoded.data.into_boxed_slice(),
        };
        let bytes = entry.bytes();
        while s.entries.len() >= self.cap || s.bytes > cap - bytes {
            let old = s
                .entries
                .pop_front()
                .expect("entry present while over budget");
            s.bytes -= old.bytes();
        }
        s.bytes += bytes;
        s.entries.push_back(entry);
    }

    /// `seen` = the last cursor the consumer applied.
    /// A cursor from any session but `current` is dead: resync.
    pub fn since(&self, seen: &Cursor, current: &SessionId) -> Resume {
        if seen.session != *current {
            return Resume::Resync;
        }
        let s = self.state.lock().unwrap();
        if s.session.as_ref() != Some(current) {
            return Resume::Resync;
        }
        if let Some(first) = s.entries.front() {
            if first.seq > seen.seq.saturating_add(1) {
                return Resume::Resync;
            }
        } else if s.last_seq.is_none_or(|seq| seen.seq < seq) {
            return Resume::Resync;
        }
        let replay = s
            .entries
            .iter()
            .filter(|e| e.seq > seen.seq)
            .map(|e| serde_json::from_slice(&e.data))
            .collect::<serde_json::Result<Vec<Envelope>>>();
        match replay {
            Ok(events) => Resume::Replay(events),
            Err(_) => Resume::Resync,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cv_core::events::engine::EngineEvent;
    use cv_core::events::{AppEvent, EventId, SnapshotEpoch};
    use cv_core::id::WorkspaceId;

    fn event(session: &str, seq: u64, raw: String) -> Envelope {
        Envelope {
            workspace: WorkspaceId("ws".into()),
            snapshot_epoch: SnapshotEpoch(0),
            build_id: None,
            cursor: Cursor {
                session: SessionId(session.into()),
                seq,
            },
            event_id: EventId(format!("{session}:{seq}")),
            payload: AppEvent::Engine(EngineEvent::MalformedLine { raw }),
        }
    }

    fn cost(env: &Envelope) -> usize {
        serde_json::to_vec(env).unwrap().len() + size_of::<Entry>()
    }

    #[test]
    fn byte_eviction_preserves_an_ordered_suffix() {
        let a = event("s", 0, "a".into());
        let b = event("s", 1, "b".into());
        let c = event("s", 2, "c".repeat(20));
        let d = event("s", 3, "d".into());
        let cap = cost(&c) + cost(&d) + 1; // the session identifier costs one byte
        let ring = EventRing::with_byte_capacity(10, cap);
        for env in [&a, &b, &c, &d] {
            ring.record(env);
        }
        assert_eq!(ring.len(), 2);
        assert!(matches!(
            ring.since(&a.cursor, &a.cursor.session),
            Resume::Resync
        ));
        assert!(matches!(
            ring.since(&b.cursor, &b.cursor.session),
            Resume::Replay(events) if events.iter().map(|e| &e.cursor).eq([&c.cursor, &d.cursor])
        ));
        assert_eq!(ring.state.lock().unwrap().bytes, cap - 1);
    }

    #[test]
    fn an_unstored_latest_event_cannot_pass_as_an_empty_replay() {
        let a = event("s", 0, "a".into());
        let b = event("s", 1, "too long to fit".into());
        let c = event("s", 2, "c".into());
        let ring = EventRing::with_byte_capacity(10, cost(&a) + 1);
        ring.record(&a);
        ring.record(&b);
        assert!(ring.is_empty());
        assert!(matches!(
            ring.since(&a.cursor, &a.cursor.session),
            Resume::Resync
        ));
        assert!(
            matches!(ring.since(&b.cursor, &b.cursor.session), Resume::Replay(v) if v.is_empty())
        );
        ring.record(&c);
        assert!(matches!(
            ring.since(&a.cursor, &a.cursor.session),
            Resume::Resync
        ));
        assert!(
            matches!(ring.since(&b.cursor, &b.cursor.session), Resume::Replay(v) if v.len() == 1 && v[0].cursor == c.cursor)
        );
    }

    #[test]
    fn session_rotation_and_max_sequence_do_not_replay_old_records() {
        let a = event("old", 0, "a".into());
        let b = event("new", u64::MAX, "b".into());
        let ring = EventRing::new(1024);
        ring.record(&a);
        ring.record(&b);
        assert_eq!(ring.len(), 1);
        assert!(matches!(
            ring.since(&a.cursor, &b.cursor.session),
            Resume::Resync
        ));
        assert!(
            matches!(ring.since(&b.cursor, &b.cursor.session), Resume::Replay(v) if v.is_empty())
        );
    }
}
