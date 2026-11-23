//! Consumer side of the CV cursor. `CursorTracker` turns each incoming
//! envelope into apply / skip / resync; `channel_sink` is a bounded
//! hand-off whose overflow is *recorded* (lag ⇒ the next delivered
//! envelope shows a gap ⇒ resync), never silently absorbed.

use cv_core::engine::EventSink;
use cv_core::events::{Cursor, Envelope, SessionId};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResyncReason {
    /// First event ever, or the engine started a new session (watch child
    /// restarted): disk truth may have moved without events.
    SessionChanged {
        from: Option<SessionId>,
        to: SessionId,
    },
    /// Events between `expected` and `got` were never seen.
    Gap { expected: u64, got: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    /// In order — apply it.
    Next,
    /// Already applied (replay overlap) — skip.
    Duplicate,
    /// Call `Engine::snapshot()`, replace derived state, then apply this
    /// envelope on top.
    Resync(ResyncReason),
}

#[derive(Debug, Default, Clone)]
pub struct CursorTracker {
    last: Option<Cursor>,
}

impl CursorTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn last(&self) -> Option<&Cursor> {
        self.last.as_ref()
    }

    pub fn observe(&mut self, c: &Cursor) -> Observation {
        let obs = match &self.last {
            None => Observation::Resync(ResyncReason::SessionChanged {
                from: None,
                to: c.session.clone(),
            }),
            Some(l) if l.session != c.session => {
                Observation::Resync(ResyncReason::SessionChanged {
                    from: Some(l.session.clone()),
                    to: c.session.clone(),
                })
            }
            Some(l) if c.seq <= l.seq => return Observation::Duplicate,
            Some(l) if c.seq == l.seq + 1 => Observation::Next,
            Some(l) => Observation::Resync(ResyncReason::Gap {
                expected: l.seq + 1,
                got: c.seq,
            }),
        };
        self.last = Some(c.clone());
        obs
    }
}

/// Count of envelopes dropped because the consumer lagged.
#[derive(Debug, Clone, Default)]
pub struct Lag(Arc<AtomicU64>);

impl Lag {
    pub fn dropped(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// A bounded sink: the engine never blocks on (or buffers unboundedly
/// for) a slow UI. Overflow increments `Lag` and leaves a seq gap.
pub fn channel_sink(capacity: usize) -> (EventSink, Receiver<Envelope>, Lag) {
    let (tx, rx): (SyncSender<Envelope>, _) = sync_channel(capacity.max(1));
    let lag = Lag::default();
    let l = lag.clone();
    let sink: EventSink = Box::new(move |env| match tx.try_send(env) {
        Ok(()) | Err(TrySendError::Disconnected(_)) => {}
        Err(TrySendError::Full(_)) => {
            l.0.fetch_add(1, Ordering::SeqCst);
        }
    });
    (sink, rx, lag)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str, seq: u64) -> Cursor {
        Cursor {
            session: SessionId(s.into()),
            seq,
        }
    }

    #[test]
    fn tracker_classifies() {
        let mut t = CursorTracker::new();
        assert!(matches!(
            t.observe(&c("a", 0)),
            Observation::Resync(ResyncReason::SessionChanged { from: None, .. })
        ));
        assert_eq!(t.observe(&c("a", 1)), Observation::Next);
        assert_eq!(t.observe(&c("a", 1)), Observation::Duplicate);
        assert_eq!(
            t.observe(&c("a", 4)),
            Observation::Resync(ResyncReason::Gap {
                expected: 2,
                got: 4
            })
        );
        assert!(matches!(
            t.observe(&c("b", 0)),
            Observation::Resync(ResyncReason::SessionChanged { .. })
        ));
    }
}
