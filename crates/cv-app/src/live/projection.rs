//! Live IR projection policy: debounce, ordered event journalling, and
//! fallback disclosure. Process spawning stays in `cv-engine`.

use super::{IR_WINDOW, LiveDelivery, Scheduler};
use cv_core::engine::{EventSink, ExitClass};
use cv_engine::cursor::{CursorTracker, Observation};
use cv_engine::debounce::Debounced;
use cv_model::status::StatusNotice;
use cv_model::workbench::Inbound;
use std::sync::{Arc, Mutex, Weak};

pub(super) struct IrProjection {
    worker: Debounced,
}

impl IrProjection {
    pub(super) fn request(&self) {
        self.worker.request();
    }
}

impl LiveDelivery {
    /// The coalescing worker for [`Self::project_ir`]. `None` (logged)
    /// when the OS refuses a thread: Pages then keeps its marked fallback.
    /// Projection events go through the same cursor-tracking route as the
    /// watch: they are minted in the shared engine stream and must never
    /// be swallowed after consuming sequence numbers.
    pub(super) fn ir_projector(
        &self,
        sched: Weak<Scheduler>,
        tracker: Arc<Mutex<CursorTracker>>,
    ) -> Option<IrProjection> {
        let delivery = self.clone();
        Debounced::spawn("cv-ir-projection", IR_WINDOW, move || {
            delivery.project_ir(&sched, tracker.clone())
        })
        .inspect_err(|e| {
            crate::logging::warn(
                "ir_projection_unavailable",
                &[("error", serde_json::json!(e.to_string()))],
            )
        })
        .ok()
        .map(|worker| IrProjection { worker })
    }

    /// Boris IR stages into CV scratch and publishes `.cv/ir` only after
    /// a successful build, then a resync lets Pages read it. IR mode needs
    /// no layout, so it also runs after a failed HTML build. Success and
    /// failure envelopes both stay in the shared event stream; Activity
    /// coalesces their recurrence without dropping their event ids.
    pub(super) fn project_ir(&self, sched: &Weak<Scheduler>, tracker: Arc<Mutex<CursorTracker>>) {
        let Some(sched) = sched.upgrade() else {
            return;
        };
        let sink = self.projection_sink(tracker);
        let result = sched.run(&self.engine.ir_projection(), sink);
        drop(sched);
        if matches!(&result, Ok(r) if r.exit == ExitClass::Ok) {
            self.resync();
            return;
        }
        let why = match &result {
            Ok(r) => format!("{:?}", r.exit),
            Err(e) => e.to_string(),
        };
        crate::logging::warn("ir_projection_failed", &[("why", serde_json::json!(why))]);
        if let Err(e) = result {
            self.inbox.send(Inbound::Status(StatusNotice::engine_evidence(format!(
                "Boris IR projection failed: {e} · Pages keeps the last published Boris IR or marked source fields · {}",
                self.workspace_identity()
            ))));
        }
    }

    /// Route one-shot IR output through the shared cursor tracker without
    /// applying watch policy: a projection must not recursively request
    /// itself, and its `Initial` phase is not the watch child's startup
    /// failure. Successful completion resyncs once in `project_ir`.
    pub(super) fn projection_sink(&self, tracker: Arc<Mutex<CursorTracker>>) -> EventSink {
        let delivery = self.clone();
        let mut deliver = cv_widgets::editor::host::tee_builds(self.inbox.sink());
        Box::new(move |env| {
            let obs = tracker
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .observe(&env.cursor);
            if obs == Observation::Duplicate {
                return;
            }
            if matches!(obs, Observation::Resync(_)) {
                delivery.request_resync();
            }
            deliver(env);
        })
    }
}
