//! Palette verbs: `StubEngine ≡ ProcessEngine` for every reported verb.
//! Each verb replays its real capture (`fixtures/captures/verbs/`) through
//! fake-boris and through the stub, and both must report the same argv,
//! exit, output and event order.

mod common;

use common::{FakeBoris, collector};
use cv_core::engine::{Engine, ExitClass, ExportKind, Verb};
use cv_core::events::engine::EngineEvent;
use cv_core::events::{AppEvent, Envelope};
use cv_engine::EngineError;
use cv_engine::proc::CancelToken;
use cv_fixtures::stub_engine::{StubEngine, verb_capture_key};
use std::time::{Duration, Instant};

fn every_verb() -> Vec<(&'static str, Verb)> {
    let mut v: Vec<(&str, Verb)> = [
        Verb::Build,
        Verb::Validate,
        Verb::Check,
        Verb::Plan,
        Verb::Graph,
        Verb::Impact { id: "index".into() },
        Verb::Impact {
            id: "guides/getting-started".into(),
        },
        Verb::Impact {
            id: "missing".into(),
        },
    ]
    .into_iter()
    .chain(
        [
            ExportKind::Out,
            ExportKind::Rag,
            ExportKind::Context,
            ExportKind::Llms,
            ExportKind::Rss,
            ExportKind::Sitemap,
        ]
        .map(|kind| Verb::Export { kind }),
    )
    .map(|v| ("happy", v))
    .collect();
    for verb in [Verb::Build, Verb::Validate, Verb::Check] {
        v.push(("happy-textile", verb));
    }
    v
}

fn capture(corpus: &str, verb: &Verb) -> String {
    format!("{corpus}/{}/", verb_capture_key(verb).unwrap())
}

/// Payloads with the wall clock zeroed: the stub has none.
fn observed(envs: &[Envelope]) -> Vec<(u64, serde_json::Value)> {
    envs.iter()
        .map(|e| {
            let mut p = serde_json::to_value(&e.payload).unwrap();
            if let Some(inner) = p
                .pointer_mut("/Engine")
                .and_then(|v| v.as_object_mut())
                .and_then(|o| o.values_mut().next())
                .and_then(|v| v.as_object_mut())
                && inner.contains_key("duration_ms")
            {
                inner.insert("duration_ms".into(), 0.into());
            }
            (e.cursor.seq, p)
        })
        .collect()
}

#[test]
fn every_captured_verb_is_spelled_as_cv_engine_spells_it() {
    let c = cv_fixtures::corpus(cv_fixtures::VERBS);
    for (corpus, verb) in every_verb() {
        let dir = capture(corpus, &verb);
        let argv: Vec<String> = c
            .file_str(&format!("{dir}argv.txt"))
            .lines()
            .map(String::from)
            .collect();
        assert_eq!(argv, cv_engine::verbs::argv(&verb).unwrap(), "{dir}");
    }
    // No capture goes untested.
    let covered: Vec<String> = every_verb()
        .iter()
        .map(|(corpus, verb)| format!("{}argv.txt", capture(corpus, verb)))
        .collect();
    for f in c.pinned_under("") {
        if f.ends_with("/argv.txt") {
            assert!(covered.contains(&f), "{f} has no conformance case");
        }
    }
}

#[test]
fn process_and_stub_report_every_verb_alike() {
    let c = cv_fixtures::corpus(cv_fixtures::VERBS);
    for (corpus, verb) in every_verb() {
        let dir = capture(corpus, &verb);
        let args = cv_engine::verbs::argv(&verb).unwrap();
        let fake = FakeBoris::new();
        for f in ["stdout.txt", "stderr.txt", "exit-code.txt"] {
            fake.set_bytes(
                &format!("verbs/{}/{f}", args[0]),
                &c.file(&format!("{dir}{f}")),
            );
        }
        let process = fake.engine();
        let (sink, got) = collector();
        let exit = process
            .run_reported(&verb, sink, &CancelToken::new())
            .unwrap();
        let from_process = observed(&got.lock().unwrap());

        let stub = StubEngine::new(corpus);
        let (sink, got) = collector();
        assert_eq!(
            stub.run_reported(&verb, sink, &CancelToken::new()).unwrap(),
            exit,
            "{dir}"
        );
        let from_stub = observed(&got.lock().unwrap());
        assert_eq!(from_process, from_stub, "{dir}");

        let last = got.lock().unwrap().last().cloned().unwrap();
        let AppEvent::Engine(EngineEvent::VerbReported(r)) = &last.payload else {
            panic!("{dir}: {:?}", last.payload)
        };
        assert_eq!(r.verb, verb.name(), "{dir}");
        assert_eq!(r.argv, args, "{dir}");
        assert_eq!(
            r.stderr,
            c.file_str(&format!("{dir}stderr.txt")),
            "{dir}: stderr verbatim"
        );
        assert_eq!(r.code.map(ExitClass::from_code), Some(exit), "{dir}");
        assert_eq!(
            fake.read("argv.log").lines().last(),
            Some(args.join(" ").as_str()),
            "{dir}: what the child was given"
        );
    }
}

#[test]
fn the_captures_cover_success_content_failure_and_usage_error() {
    let exit = |corpus: &str, verb: Verb| {
        StubEngine::new(corpus)
            .run_reported(&verb, Box::new(|_| {}), &CancelToken::new())
            .unwrap()
    };
    assert_eq!(exit("happy", Verb::Build), ExitClass::Ok);
    assert_eq!(
        exit("happy-textile", Verb::Build),
        ExitClass::ContentFailure
    );
    assert_eq!(
        exit(
            "happy",
            Verb::Export {
                kind: ExportKind::Rss
            }
        ),
        ExitClass::UsageError
    );
}

#[test]
fn cancelling_a_reported_verb_ends_it_as_verb_failed() {
    let fake = FakeBoris::new();
    fake.set("verbs/build/hold", "");
    let engine = fake.engine();
    let cancel = CancelToken::new();
    let c = cancel.clone();
    let killer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        c.cancel();
    });
    let t = Instant::now();
    let (sink, got) = collector();
    let r = engine.run_reported_cancellable(&Verb::Build, sink, &cancel);
    killer.join().unwrap();
    assert!(matches!(r, Err(EngineError::Cancelled)), "{r:?}");
    assert!(t.elapsed() < Duration::from_secs(20));
    assert!(matches!(
        got.lock().unwrap().last().map(|e| &e.payload),
        Some(AppEvent::Engine(EngineEvent::VerbFailed { verb, .. })) if verb == "build"
    ));
}
