//! Rebuild-burst cost at the 5k-page scale envelope (#78), on the real
//! workbench path: queued resync snapshots and build envelopes, one
//! `drain()` (dedup digest, journal, cascade, every module's ingest,
//! reclamp), and the `nav()` call the shell makes every frame. Ignored
//! by default; run in release:
//! `cargo test --release -p cv-model --test burst_bench -- --ignored --nocapture`.

use cv_core::diagnostic::{Diagnostic, Severity, Span};
use cv_core::events::engine::{BuildPhase, EngineEvent};
use cv_core::events::{AppEvent, BuildId, Cursor, Envelope, EventId, SessionId, SnapshotEpoch};
use cv_core::id::WorkspaceId;
use cv_fixtures::stub_store::StubStore;
use cv_model::module::{OutputRecord, PageRecord, ProjectSnapshot, Registry};
use cv_model::workbench::{Inbound, Workbench};
use std::time::{Duration, Instant};

const N: usize = 5_000;
const BURST: u64 = 200;
const RUNS: usize = 7;

/// The resync snapshot `live.rs` sends on every build result: `n` pages
/// with bodies and frontmatter under one trunk, one HTML output each.
fn snapshot(n: usize, epoch: u64) -> ProjectSnapshot {
    let pages: Vec<PageRecord> = (0..n)
        .map(|i| {
            let id = if i == 0 {
                "index".to_string()
            } else {
                format!("p/{i:04}")
            };
            PageRecord {
                path: format!("{id}.md"),
                title: format!("Page {i:04}"),
                parent: (i > 0).then(|| "index".into()),
                frontmatter: vec![("title".into(), format!("Page {i:04}"))],
                body: format!("# Page {i:04}\n\n{}", "lorem ipsum ".repeat(12)),
                output: Some(format!("{id}.html")),
                id,
                ..Default::default()
            }
        })
        .collect();
    let outputs = pages
        .iter()
        .map(|p| OutputRecord {
            path: p.output.clone().unwrap_or_default(),
            kind: "html-page".into(),
            producer: "boris".into(),
            status: "written".into(),
            required: true,
            bytes: 1024,
            sha256: "0".repeat(64),
            format_version: None,
        })
        .collect();
    ProjectSnapshot {
        epoch,
        compiler: Some("boris/0.9.0".into()),
        pages,
        outputs,
        ..Default::default()
    }
}

/// One standing diagnostic per page: the diagnostics river.
fn river(n: usize) -> Vec<Diagnostic> {
    (0..n)
        .map(|i| Diagnostic {
            code: "EREFERENCEMISSING".into(),
            severity: if i % 3 == 0 {
                Severity::Warning
            } else {
                Severity::Error
            },
            span: Some(Span {
                path: format!("p/{i:04}.md"),
                start: 0,
                end: 0,
                line: 1,
                col: 1,
            }),
            message: format!("missing reference {i}"),
            build_version: Some(BuildId("b1".into())),
        })
        .collect()
}

fn failed(seq: u64, phase: BuildPhase, diagnostics: Vec<Diagnostic>) -> Envelope {
    Envelope {
        cause: None,
        workspace: WorkspaceId("bench".into()),
        snapshot_epoch: SnapshotEpoch(1),
        build_id: Some(BuildId(format!("b{seq}"))),
        cursor: Cursor {
            session: SessionId("bench".into()),
            seq,
        },
        event_id: EventId(format!("bench-{seq}")),
        payload: AppEvent::Engine(EngineEvent::BuildFailed {
            phase,
            diagnostics,
            recoverable: true,
        }),
    }
}

/// 5,000 pages + outputs, a 5,000-diagnostic river, and the middle row
/// of `module`'s "all" mailbox selected.
fn workbench(module: &str) -> (Workbench, String) {
    let mut wb = Workbench::new(Registry::with_all()).with_store(Box::new(StubStore::new()));
    wb.load_snapshot(&snapshot(N, 1));
    let mut sink = wb.sink();
    sink(failed(1, BuildPhase::Initial, river(N)));
    wb.drain();
    wb.activate(wb.registry.index_of(module).unwrap());
    wb.set_nav("all");
    wb.select_row(wb.list_len() / 2);
    let key = wb.selected_key().unwrap().to_string();
    (wb, key)
}

/// One `drain()` of a queued burst of `BURST` rebuilds, each re-reporting
/// 50 standing diagnostics. With `resync`, each build envelope follows a
/// full snapshot, in the order `live.rs`'s resync sink sends them.
fn burst(module: &str, resync: bool) -> Duration {
    let (mut wb, key) = workbench(module);
    let mut sink = wb.sink();
    let inbox = wb.inbox();
    let all = river(N);
    let snap = resync.then(|| snapshot(N, 2));
    for seq in 2..2 + BURST {
        if let Some(s) = &snap {
            let mut s = s.clone();
            s.epoch = seq;
            inbox.send(Inbound::Snapshot(s));
        }
        let at = (seq as usize * 50) % N;
        sink(failed(seq, BuildPhase::Rebuild, all[at..at + 50].to_vec()));
    }
    let start = Instant::now();
    assert!(wb.drain());
    let took = start.elapsed();
    assert_eq!(
        wb.selected_key(),
        Some(key.as_str()),
        "{module}: selection kept"
    );
    assert_eq!(wb.app.committed, 1 + BURST, "{module}: nothing dropped");
    if resync {
        assert_eq!(wb.app.snapshot_epoch, 1 + BURST, "latest snapshot wins");
    }
    took
}

/// Mean cost of one `nav()` call.
fn nav(wb: &Workbench, module: &str) -> Duration {
    const CALLS: u32 = 1_000;
    let m = wb
        .registry
        .get(wb.registry.index_of(module).unwrap())
        .unwrap();
    let start = Instant::now();
    for _ in 0..CALLS {
        std::hint::black_box(m.nav());
    }
    start.elapsed() / CALLS
}

fn report(what: &str, mut runs: Vec<Duration>) {
    runs.sort();
    let (lo, med, hi) = (runs[0], runs[runs.len() / 2], runs[runs.len() - 1]);
    eprintln!(
        "bench {what}: median {med:?} (min {lo:?}, max {hi:?}, {} runs)",
        runs.len()
    );
}

#[test]
#[ignore = "benchmark; run in release with --ignored"]
fn rebuild_burst_drain_and_nav_cost() {
    for (module, resync) in [
        ("pages", false),
        ("activity", false),
        ("outputs", false),
        ("pages", true),
    ] {
        let label = if resync {
            "snapshot+envelope pairs"
        } else {
            "envelopes"
        };
        report(
            &format!("drain {BURST} {label}, {module} row selected"),
            (0..RUNS).map(|_| burst(module, resync)).collect(),
        );
    }
    let (wb, _) = workbench("pages");
    for module in ["activity", "pages", "outputs", "audit"] {
        report(
            &format!("nav() per call, {module}"),
            (0..RUNS).map(|_| nav(&wb, module)).collect(),
        );
    }
}
