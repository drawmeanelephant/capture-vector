//! Bounded event ring + resumption. A consumer that fell behind asks for
//! everything after its cursor; if that cursor is from another session or
//! older than the ring holds, the answer is "resync from snapshot" — the
//! ring never grows to cover a slow reader.

use cv_core::events::{Cursor, Envelope, SessionId};
use std::collections::VecDeque;
use std::sync::Mutex;

pub const DEFAULT_RING: usize = 1024;

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
    buf: Mutex<VecDeque<Envelope>>,
}

impl EventRing {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            buf: Mutex::new(VecDeque::new()),
        }
    }

    pub fn capacity(&self) -> usize {
        self.cap
    }

    pub fn len(&self) -> usize {
        self.buf.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn push(&self, env: Envelope) {
        let mut b = self.buf.lock().unwrap();
        if b.len() == self.cap {
            b.pop_front();
        }
        b.push_back(env);
    }

    /// `seen` = the last cursor the consumer applied (`None` = nothing yet
    /// in that session).
    /// A cursor from any session but `current` is dead: resync.
    pub fn since(&self, seen: &Cursor, current: &SessionId) -> Resume {
        if seen.session != *current {
            return Resume::Resync;
        }
        let b = self.buf.lock().unwrap();
        let in_session: Vec<&Envelope> = b
            .iter()
            .filter(|e| e.cursor.session == seen.session)
            .collect();
        let Some(first) = in_session.first() else {
            return Resume::Resync;
        };
        if first.cursor.seq > seen.seq + 1 {
            return Resume::Resync;
        }
        Resume::Replay(
            in_session
                .into_iter()
                .filter(|e| e.cursor.seq > seen.seq)
                .cloned()
                .collect(),
        )
    }
}
