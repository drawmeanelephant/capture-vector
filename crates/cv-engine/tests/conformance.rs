//! fake-boris conformance: `StubEngine ≡ ProcessEngine` over every
//! captured corpus, plus gate, handshake, malformed-line, exit-mapping,
//! cancellation and secrets-channel behaviour. No real boris, no display,
//! no network.

mod common;

use common::{FakeBoris, collector, payloads, wait_until};
use cv_core::engine::{Engine, ExitClass, Verb};
use cv_core::events::engine::EngineEvent;
use cv_core::events::{AppEvent, Envelope};
use cv_engine::EngineError;
use cv_engine::probe::WatchMode;
use cv_engine::proc::CancelToken;
use cv_engine::secrets::MockSecretSource;
use cv_fixtures::stub_engine::StubEngine;
use std::time::{Duration, Instant};

const T: Duration = Duration::from_secs(30);

/// One port-level observation: the envelope's cursor plus its payload
/// as JSON, with only implementation-minted values normalized —
/// `build_version` ids and wall-clock `duration_ms` of one-shot verbs.
/// Diagnostics, phases, counts, exit classes and order are compared as-is.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    session: String,
    seq: u64,
    payload: serde_json::Value,
}

fn observe(envs: &[Envelope], one_shot: bool) -> Vec<Seen> {
    envs.iter()
        .map(|e| {
            let mut payload = serde_json::to_value(&e.payload).unwrap();
            if let Some(inner) = payload
                .pointer_mut("/Engine")
                .and_then(|v| v.as_object_mut())
                .and_then(|o| o.values_mut().next())
                .and_then(|v| v.as_object_mut())
            {
                if one_shot && inner.contains_key("duration_ms") {
                    inner.insert("duration_ms".into(), 0.into());
                }
                for d in inner
                    .get_mut("diagnostics")
                    .and_then(|v| v.as_array_mut())
                    .into_iter()
                    .flatten()
                {
                    let minted = d.get("build_version").is_some_and(|b| !b.is_null());
                    d["build_version"] = serde_json::Value::Bool(minted);
                }
            }
            Seen {
                session: e.cursor.session.0.clone(),
                seq: e.cursor.seq,
                payload,
            }
        })
        .collect()
}

/// Cursor ordering semantics, impl-neutral: every observation shares
/// one session and seq runs gap-free from `from`.
fn assert_cursor(what: &str, seen: &[Seen], from: u64) {
    for (i, s) in seen.iter().enumerate() {
        assert_eq!(s.session, seen[0].session, "{what}: one session");
        assert_eq!(s.seq, from + i as u64, "{what}: gap-free seq");
    }
}

/// Session-relative view for cross-impl comparison (ids differ by impl).
fn relative(seen: &[Seen]) -> Vec<(u64, serde_json::Value)> {
    seen.iter().map(|s| (s.seq, s.payload.clone())).collect()
}

fn engine_payload(env: &Envelope) -> Option<&EngineEvent> {
    match &env.payload {
        AppEvent::Engine(e) => Some(e),
        _ => None,
    }
}

/// What one implementation showed for the shared scenario.
struct Transcript {
    watch1: Vec<Seen>,
    verbs: Vec<(Verb, ExitClass, Vec<Seen>)>,
    watch2: Vec<Seen>,
}

/// THE shared scenario, driven only through `cv_core::engine::Engine`:
/// start watch → recorded builds + diagnostics → stop; one-shot build /
/// validate / check with their exit classes; restart watch → stop.
/// `settle(n)` blocks until `n` envelopes reached the sink (a no-op for
/// the synchronous stub; the subprocess delivers from a pump thread).
fn scenario(engine: &dyn Engine, expect_watch: usize) -> Transcript {
    let settle = |got: &std::sync::Arc<std::sync::Mutex<Vec<Envelope>>>, n: usize| {
        wait_until("watch events", T, || got.lock().unwrap().len() >= n)
    };
    let ended = |got: &std::sync::Arc<std::sync::Mutex<Vec<Envelope>>>| {
        wait_until("watch end", T, || {
            got.lock().unwrap().iter().any(|e| {
                matches!(
                    engine_payload(e),
                    Some(EngineEvent::WatchSessionEnded { .. })
                )
            })
        })
    };

    let (sink, got) = collector();
    let mut h = engine.watch(sink).unwrap();
    settle(&got, expect_watch);
    let session = h.session().clone();
    h.stop();
    ended(&got);
    let watch1 = observe(&got.lock().unwrap(), false);
    assert!(watch1.iter().all(|s| s.session == session.0));

    let mut verbs = vec![];
    for verb in [Verb::Build, Verb::Validate, Verb::Check] {
        let (sink, got) = collector();
        let exit = engine.run(&verb, sink).unwrap();
        let seen = observe(&got.lock().unwrap(), true);
        verbs.push((verb, exit, seen));
    }

    let (sink, got) = collector();
    let mut h = engine.watch(sink).unwrap();
    settle(&got, expect_watch);
    drop(std::mem::replace(&mut h, Box::new(Gone(session))));
    ended(&got);
    let watch2 = observe(&got.lock().unwrap(), false);
    Transcript {
        watch1,
        verbs,
        watch2,
    }
}

/// Placeholder handle so the second watch is ended by *drop*, not stop.
struct Gone(cv_core::events::SessionId);
impl cv_core::engine::WatchHandle for Gone {
    fn session(&self) -> &cv_core::events::SessionId {
        &self.0
    }
    fn stop(&mut self) {}
}

fn assert_semantics(name: &str, t: &Transcript) {
    for (what, w) in [("watch1", &t.watch1), ("watch2", &t.watch2)] {
        assert_cursor(&format!("{name} {what}"), w, 0);
        assert!(
            matches!(
                w.first().map(|s| &s.payload),
                Some(v) if v.pointer("/Engine/WatchSessionStarted").is_some()
            ),
            "{name} {what}: hello first"
        );
        let ends: Vec<_> = w
            .iter()
            .filter(|s| s.payload.pointer("/Engine/WatchSessionEnded").is_some())
            .collect();
        assert_eq!(ends.len(), 1, "{name} {what}: exactly one end");
        assert_eq!(
            ends[0].payload.pointer("/Engine/WatchSessionEnded/reason"),
            Some(&serde_json::json!("Killed")),
            "{name} {what}: stopped ⇒ Killed"
        );
        assert_eq!(w.last(), Some(ends[0]), "{name} {what}: end is last");
    }
    assert_ne!(
        t.watch1[0].session, t.watch2[0].session,
        "{name}: a restarted watch is a new session"
    );
    // One-shot verbs continue the closed session's cursor gap-free.
    let mut next = t.watch1.len() as u64;
    for (verb, exit, seen) in &t.verbs {
        assert_cursor(&format!("{name} {verb:?}"), seen, next);
        assert_eq!(seen[0].session, t.watch1[0].session);
        next += seen.len() as u64;
        let last = &seen.last().unwrap().payload;
        assert_eq!(
            last.pointer("/Engine/VerbFinished"),
            Some(&serde_json::json!({ "verb": verb.name(), "exit": exit })),
            "{name} {verb:?}: VerbFinished last, exit class echoed"
        );
        let builds = seen
            .iter()
            .filter(|s| {
                s.payload.pointer("/Engine/BuildFinished").is_some()
                    || s.payload.pointer("/Engine/BuildFailed").is_some()
            })
            .count();
        assert_eq!(builds, usize::from(*verb == Verb::Build), "{name} {verb:?}");
    }
}

#[test]
fn stub_equals_process_on_every_corpus() {
    for name in cv_fixtures::corpora() {
        let fake = FakeBoris::corpus(&name);
        // A live child: the recorded stream, then held open until stopped.
        fake.set("watch-hold", "");
        let process = fake.engine();
        assert_eq!(process.report().watch_mode, WatchMode::Native, "{name}");
        let stub = StubEngine::new(&name);

        // The stub replays synchronously: its count is the recording's.
        let (sink, got) = collector();
        drop(stub.watch(sink).unwrap());
        let expect = got.lock().unwrap().len() - 1;
        let stub = StubEngine::new(&name);

        let s = scenario(&stub, expect);
        let p = scenario(&process, expect);
        assert_semantics(&format!("{name} stub"), &s);
        assert_semantics(&format!("{name} process"), &p);

        assert_eq!(
            relative(&s.watch1),
            relative(&p.watch1),
            "{name}: watch stream"
        );
        assert_eq!(
            relative(&s.watch2),
            relative(&p.watch2),
            "{name}: restart stream"
        );
        for ((verb, se, ss), (_, pe, ps)) in s.verbs.iter().zip(&p.verbs) {
            assert_eq!(se, pe, "{name} {verb:?}: exit class");
            assert_eq!(relative(ss), relative(ps), "{name} {verb:?}: events");
        }
        // Diagnostics are part of the compared surface, not projected away.
        if [
            "broken-duplicate-id",
            "broken-frontmatter",
            "broken-parent",
            "broken-wikilink",
        ]
        .contains(&name.as_str())
        {
            let diags: usize = s.verbs[0]
                .2
                .iter()
                .filter_map(|x| x.payload.pointer("/Engine/BuildFailed/diagnostics"))
                .map(|d| d.as_array().unwrap().len())
                .sum();
            assert!(diags > 0, "{name}: broken build carries diagnostics");
        }
    }
}

/// The stub replays captured exit codes through `ExitClass::from_code`;
/// the subprocess must classify every raw code the same way.
#[test]
fn process_exit_mapping_equals_stub_mapping() {
    let fake = FakeBoris::new();
    let engine = fake.engine();
    for code in 0..=9 {
        fake.set("verbs/check/exit-code.txt", &code.to_string());
        let (sink, got) = collector();
        let exit = engine.run(&Verb::Check, sink).unwrap();
        assert_eq!(exit, ExitClass::from_code(code), "exit {code}");
        assert!(matches!(
            payloads(&got.lock().unwrap()).last(),
            Some(EngineEvent::VerbFinished { exit: e, .. }) if *e == exit
        ));
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
