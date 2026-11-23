//! Live smoke against a REAL pinned boris (0.9.0 / 66efcfb). Ignored by
//! default; run with
//! `CV_BORIS_BIN=/path/to/boris cargo test -p cv-engine --test live_smoke -- --ignored`.

use cv_core::engine::{Engine, ExitClass, Verb};
use cv_core::events::AppEvent;
use cv_core::events::engine::EngineEvent;
use cv_engine::{EngineConfig, ProcessEngine};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), dst).unwrap();
        }
    }
}

fn workspace(corpus: &str, sub: &str) -> tempfile::TempDir {
    let t = tempfile::Builder::new()
        .prefix("cv-live-")
        .tempdir()
        .unwrap();
    copy_tree(
        &cv_fixtures::fixtures_root().join("corpus-src").join(corpus),
        &t.path().join(sub),
    );
    t
}

#[test]
#[ignore = "needs a real pinned boris via CV_BORIS_BIN"]
fn live_pinned_boris() {
    let ws = workspace("happy", "");
    let engine = ProcessEngine::start(EngineConfig::new(ws.path())).expect("gate");
    eprintln!(
        "identity: {:?}\nprobe: {:?}",
        engine.identity(),
        engine.report()
    );
    assert_eq!(engine.identity().version, "boris/0.9.0");

    assert_eq!(
        engine.run(&Verb::Build, Box::new(|_| {})).unwrap(),
        ExitClass::Ok
    );
    assert!(engine.snapshot().unwrap().html.is_some());
    assert_eq!(
        engine.run(&Verb::Validate, Box::new(|_| {})).unwrap(),
        ExitClass::Ok
    );

    let broken = workspace("broken-frontmatter", "content");
    let b = ProcessEngine::start(EngineConfig::new(broken.path())).unwrap();
    assert_eq!(
        b.run(&Verb::Validate, Box::new(|_| {})).unwrap(),
        ExitClass::ContentFailure
    );

    if engine.capabilities().watch {
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        let mut h = engine
            .watch(Box::new(move |e| g.lock().unwrap().push(e)))
            .unwrap();
        let t = Instant::now();
        while !got.lock().unwrap().iter().any(|e| {
            matches!(
                &e.payload,
                AppEvent::Engine(EngineEvent::BuildFinished { .. })
            )
        }) {
            assert!(
                t.elapsed() < Duration::from_secs(60),
                "{:?}",
                got.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        h.stop();
    }
}
