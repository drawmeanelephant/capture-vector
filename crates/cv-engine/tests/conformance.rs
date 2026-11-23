//! fake-boris conformance: `StubEngine ≡ ProcessEngine` over every
//! captured corpus, plus gate, handshake, malformed-line, exit-mapping,
//! cancellation and secrets-channel behaviour. No real boris, no display,
//! no network.

mod common;

use common::{FakeBoris, collector, payloads, wait_until};
use cv_core::engine::{Engine, ExitClass, Verb};
use cv_core::events::engine::{EngineEvent, WatchEndReason};
use cv_engine::EngineError;
use cv_engine::probe::WatchMode;
use cv_engine::proc::CancelToken;
use cv_engine::secrets::MockSecretSource;
use cv_fixtures::stub_engine::StubEngine;
use std::time::{Duration, Instant};

const T: Duration = Duration::from_secs(30);

/// Event identity modulo diagnostics (the stub never maps them).
fn project(evs: &[EngineEvent]) -> Vec<serde_json::Value> {
    evs.iter()
        .filter(|e| !matches!(e, EngineEvent::WatchSessionEnded { .. }))
        .map(|e| {
            let mut v = serde_json::to_value(e).unwrap();
            if let Some(obj) = v
                .as_object_mut()
                .and_then(|o| o.values_mut().next())
                .and_then(|i| i.as_object_mut())
            {
                obj.remove("diagnostics");
                obj.remove("duration_ms");
            }
            v
        })
        .collect()
}

#[test]
fn stub_equals_process_on_every_corpus() {
    for name in cv_fixtures::corpora() {
        let fake = FakeBoris::corpus(&name);
        let engine = fake.engine();
        assert_eq!(engine.report().watch_mode, WatchMode::Native, "{name}");

        let stub = StubEngine::new(&name);
        let (s_sink, s_got) = collector();
        let _ = stub.watch(s_sink).unwrap();

        let (p_sink, p_got) = collector();
        let _h = engine.watch(p_sink).unwrap();
        wait_until("watch end", T, || {
            p_got.lock().unwrap().iter().any(|e| {
                matches!(
                    &e.payload,
                    cv_core::events::AppEvent::Engine(EngineEvent::WatchSessionEnded { .. })
                )
            })
        });
        let p = payloads(&p_got.lock().unwrap());
        assert_eq!(
            project(&payloads(&s_got.lock().unwrap())),
            project(&p),
            "{name}: watch stream"
        );
        assert!(matches!(
            p.last(),
            Some(EngineEvent::WatchSessionEnded {
                reason: WatchEndReason::CleanExit
            })
        ));
        // Cursor: one session, gap-free seq from 0.
        let got = p_got.lock().unwrap();
        for (i, e) in got.iter().enumerate() {
            assert_eq!(e.cursor.seq, i as u64, "{name}");
            assert_eq!(e.cursor.session, got[0].cursor.session);
        }
        drop(got);

        for verb in [Verb::Build, Verb::Validate, Verb::Check] {
            let s = stub.run(&verb, Box::new(|_| {})).unwrap();
            let p = engine.run(&verb, Box::new(|_| {})).unwrap();
            assert_eq!(s, p, "{name} {verb:?}");
        }
    }
}

#[test]
fn broken_corpus_diagnostics_come_from_boris_stderr() {
    let fake = FakeBoris::corpus("broken-frontmatter");
    let engine = fake.engine();
    let out = engine.run_verb(&Verb::Build, &CancelToken::new()).unwrap();
    // Verbatim, byte-for-byte what Boris printed.
    assert_eq!(
        out.stderr,
        cv_fixtures::corpus("broken-frontmatter").file_str("build/stderr.txt")
    );
    let (sink, got) = collector();
    assert_eq!(
        engine.run(&Verb::Build, sink).unwrap(),
        ExitClass::ContentFailure
    );
    let evs = payloads(&got.lock().unwrap());
    let EngineEvent::BuildFailed {
        diagnostics,
        recoverable,
        ..
    } = &evs[0]
    else {
        panic!("{evs:?}")
    };
    assert!(*recoverable);
    assert_eq!(diagnostics[0].code, "EFRONTMATTER");
    assert!(matches!(
        &evs[1],
        EngineEvent::VerbFinished {
            exit: ExitClass::ContentFailure,
            ..
        }
    ));
}

#[test]
fn gate_rejects_unpinned_engines() {
    let fake = FakeBoris::new();
    fake.set("version.txt", "boris/0.8.2\n");
    assert!(matches!(
        cv_engine::ProcessEngine::start(fake.config()),
        Err(EngineError::VersionMismatch { .. })
    ));
    fake.set("version.txt", "boris/0.9.0\n");
    fake.set(
        "build-info.txt",
        r#"{"format": "boris-build-info", "schemaVersion": "1", "version": "boris/0.9.0", "vcsRevision": "ce40d78"}"#,
    );
    assert!(matches!(
        cv_engine::ProcessEngine::start(fake.config()),
        Err(EngineError::VersionMismatch { found_vcs: Some(v), .. }) if v == "ce40d78"
    ));
    fake.set("build-info.txt", "not json");
    assert!(matches!(
        cv_engine::ProcessEngine::start(fake.config()),
        Err(EngineError::BuildInfoInvalid { .. })
    ));
}

#[test]
fn handshake_is_required() {
    let fake = FakeBoris::corpus("happy");
    let mut cfg = fake.config();
    cfg.handshake_timeout = Duration::from_millis(1500);
    let engine = cv_engine::ProcessEngine::start(cfg).unwrap();

    let good = cv_fixtures::corpus("happy").file_str("watch/session.ndjson");
    let mut lines: Vec<&str> = good.lines().collect();
    let bad_schema = lines[0].replace("\"watch_events_schema\":1", "\"watch_events_schema\":2");
    lines[0] = &bad_schema;
    fake.set("watch.ndjson", &(lines.join("\n") + "\n"));
    assert!(matches!(
        engine.start_watch(Box::new(|_| {})),
        Err(EngineError::Handshake(_))
    ));

    fake.set(
        "watch.ndjson",
        &(good.lines().skip(1).collect::<Vec<_>>().join("\n") + "\n"),
    );
    assert!(matches!(
        engine.start_watch(Box::new(|_| {})),
        Err(EngineError::Handshake(_))
    ));

    // Silent child that never says hello: killed on timeout, tree and all.
    fake.set("watch.ndjson", "");
    fake.set("watch-hold", "");
    let t = Instant::now();
    assert!(matches!(
        engine.start_watch(Box::new(|_| {})),
        Err(EngineError::Handshake(_))
    ));
    assert!(t.elapsed() < Duration::from_secs(10));
    assert!(!engine.watch_live());
}

#[test]
fn malformed_lines_are_surfaced_not_dropped() {
    let fake = FakeBoris::corpus("happy");
    let engine = fake.engine();
    let good = cv_fixtures::corpus("happy").file_str("watch/session.ndjson");
    let mut bytes = Vec::new();
    for (i, l) in good.lines().enumerate() {
        bytes.extend_from_slice(l.as_bytes());
        bytes.push(b'\n');
        if i == 1 {
            bytes.extend_from_slice(b"error: content or layout failure: ParseFailed\n");
            bytes.extend_from_slice(b"{\"event\":\"build-succeeded\"\n");
            bytes.extend_from_slice(b"\xff\xfe not utf8\n");
        }
    }
    fake.set_bytes("watch.ndjson", &bytes);
    let (sink, got) = collector();
    let _h = engine.watch(sink).unwrap();
    wait_until("end", T, || {
        payloads(&got.lock().unwrap())
            .iter()
            .any(|e| matches!(e, EngineEvent::WatchSessionEnded { .. }))
    });
    let raws: Vec<String> = payloads(&got.lock().unwrap())
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::MalformedLine { raw } => Some(raw),
            _ => None,
        })
        .collect();
    assert_eq!(raws.len(), 3, "{raws:?}");
    assert_eq!(raws[0], "error: content or layout failure: ParseFailed");
}

#[test]
fn exit_codes_map_exactly() {
    let fake = FakeBoris::new();
    let engine = fake.engine();
    for (code, class) in [
        ("0", ExitClass::Ok),
        ("1", ExitClass::ContentFailure),
        ("2", ExitClass::UsageError),
        ("3", ExitClass::IoFailure),
        ("7", ExitClass::IoFailure),
    ] {
        fake.set("verbs/validate/exit-code.txt", code);
        assert_eq!(
            engine.run(&Verb::Validate, Box::new(|_| {})).unwrap(),
            class,
            "exit {code}"
        );
    }
    // A verb the binary doesn't know is Boris's usage error, not ours.
    assert_eq!(
        engine.run(&Verb::Plan, Box::new(|_| {})).unwrap(),
        ExitClass::UsageError
    );
}

#[test]
fn cancel_kills_the_process_tree() {
    let fake = FakeBoris::new();
    let engine = fake.engine();
    fake.set("verbs/build/hold", "");
    let cancel = CancelToken::new();
    let c = cancel.clone();
    let t = Instant::now();
    let killer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        c.cancel();
    });
    let r = engine.run_cancellable(&Verb::Build, Box::new(|_| {}), &cancel);
    killer.join().unwrap();
    assert!(matches!(r, Err(EngineError::Cancelled)), "{r:?}");
    // The held grandchild shares stdout/stderr: returning at all proves it died.
    assert!(t.elapsed() < Duration::from_secs(20));
}

#[test]
fn nostr_key_rides_stdin_only() {
    let fake = FakeBoris::new();
    let engine = fake.engine();
    let key = "nsec1fakefakefakefakefakefakefakefake";
    fake.set("verbs/nostr/stdout.txt", "{\"bundle\":true}\n");
    fake.set("verbs/nostr/stderr.txt", &format!("debug echo {key}\n"));
    let plan = fake.root().join("plan.json");
    std::fs::write(&plan, "{}").unwrap();
    let out = engine
        .sign_nostr(
            &plan,
            None,
            &MockSecretSource::new(key),
            &CancelToken::new(),
        )
        .unwrap();
    assert_eq!(out.outcome.class(), Some(ExitClass::Ok));
    assert_eq!(fake.read("stdin.txt").trim(), key);
    let argv = fake.read("argv.log");
    assert!(
        argv.contains("nostr sign --plan") && argv.contains("--key-stdin"),
        "{argv}"
    );
    assert!(!argv.contains(key));
    assert!(!fake.read("env.log").contains(key));
    assert!(!out.stderr.contains(key) && out.stderr.contains("<redacted>"));
    // The generic verb path refuses to carry it at all.
    let leaky = Verb::Custom {
        name: "nostr".into(),
        args: vec!["sign".into(), "--key-stdin".into()],
    };
    assert!(engine.run(&leaky, Box::new(|_| {})).is_err());
}
