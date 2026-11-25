use super::*;
use crate::workbench::{ShellRequest, Workbench};
use cv_core::diagnostic::Span;
use cv_core::engine::EngineCaps;
use cv_core::events::{EventId, SessionId, SnapshotEpoch};
use cv_core::id::WorkspaceId;

fn env(session: &str, seq: u64, payload: AppEvent) -> Envelope {
    Envelope {
        workspace: WorkspaceId("w".into()),
        snapshot_epoch: SnapshotEpoch(1),
        build_id: None,
        cursor: Cursor {
            session: SessionId(session.into()),
            seq,
        },
        event_id: EventId(format!("{session}-{seq}")),
        payload,
    }
}

fn engine(session: &str, seq: u64, ev: EngineEvent) -> Envelope {
    env(session, seq, AppEvent::Engine(ev))
}

fn hello() -> EngineEvent {
    EngineEvent::WatchSessionStarted {
        schema: 1,
        compiler: "boris/0.9.0".into(),
    }
}

fn diag(code: &str) -> Diagnostic {
    Diagnostic {
        code: code.into(),
        severity: Severity::Error,
        span: Some(Span {
            path: "index.md".into(),
            start: 0,
            end: 1,
            line: 2,
            col: 3,
        }),
        message: "unknown key".into(),
        build_version: None,
    }
}

fn located(path: &str) -> EngineState {
    EngineState::Located(Located {
        source: EngineSource::Path,
        path: path.into(),
        version: "boris/0.9.0".into(),
        vcs_revision: "66efcfb".into(),
        build_info_raw: r#"{"format":"boris-build-info","version":"boris/0.9.0","vcsRevision":"66efcfb","signingToken":"leak"}"#.into(),
    })
}

fn module(f: Facts) -> DoctorModule {
    let feed = Feed::default();
    feed.update(|x| *x = f);
    DoctorModule::new(feed)
}

fn rows(m: &DoctorModule, nav: &str) -> Vec<RowModel> {
    (0..m.list_len(Some(nav)))
        .map(|i| m.row(Some(nav), i))
        .collect()
}

fn fatal_initial(session: &str, seq: u64) -> Envelope {
    engine(
        session,
        seq,
        EngineEvent::BuildFailed {
            phase: BuildPhase::Initial,
            diagnostics: vec![],
            recoverable: false,
        },
    )
}

fn diagnostic_failure(session: &str, seq: u64, code: &str) -> Envelope {
    engine(
        session,
        seq,
        EngineEvent::BuildFailed {
            phase: BuildPhase::Initial,
            diagnostics: vec![diag(code)],
            recoverable: true,
        },
    )
}

fn verb_finished(session: &str, seq: u64, verb: &str) -> Envelope {
    engine(
        session,
        seq,
        EngineEvent::VerbFinished {
            verb: verb.into(),
            exit: ExitClass::ContentFailure,
        },
    )
}

fn build_finished(session: &str, seq: u64) -> Envelope {
    engine(
        session,
        seq,
        EngineEvent::BuildFinished {
            phase: BuildPhase::Initial,
            pages_written: 0,
            diagnostics: vec![],
            duration_ms: 1,
        },
    )
}

fn ended(session: &str, seq: u64, exit: Option<cv_core::events::engine::WatchExit>) -> Envelope {
    engine(
        session,
        seq,
        EngineEvent::WatchSessionEnded {
            reason: WatchEndReason::Desync,
            exit,
        },
    )
}

#[test]
fn a_fatal_failure_takes_diagnostics_only_from_its_recovery_window() {
    let mut w = WatchLog::default();
    w.ingest(&engine("s", 0, hello()));
    w.ingest(&fatal_initial("s", 1));
    // An ordinary later build in the same live session is not the diagnostic
    // one-shot: the watch must have ended before recovery can be running.
    w.ingest(&diagnostic_failure("s", 2, "EEARLY"));
    assert!(!w.fatal.as_ref().unwrap().from_followup);
    assert!(w.fatal.as_ref().unwrap().diagnostics.is_empty());

    w.ingest(&ended("s", 3, None));
    w.ingest(&diagnostic_failure("s", 4, "EFRONTMATTER"));
    assert!(!w.fatal.as_ref().unwrap().from_followup);
    w.ingest(&verb_finished("s", 5, "build"));
    let f = w.fatal.as_ref().unwrap();
    assert!(f.from_followup);
    assert_eq!(f.diagnostics[0].code, "EFRONTMATTER");
    assert_eq!(w.last_cursor.as_ref().unwrap().session.0, "s");
    assert_eq!(w.engine_events, 6);
}

#[test]
fn an_ir_projection_in_the_recovery_window_is_not_the_diagnostic_build() {
    let mut w = WatchLog::default();
    w.ingest(&engine("s", 0, hello()));
    w.ingest(&fatal_initial("s", 1));
    w.ingest(&ended("s", 2, None));
    w.ingest(&diagnostic_failure("s", 3, "EIR"));
    w.ingest(&verb_finished("s", 4, "ir"));
    assert!(!w.fatal.as_ref().unwrap().from_followup);
    assert!(w.fatal.as_ref().unwrap().diagnostics.is_empty());

    w.ingest(&diagnostic_failure("s", 5, "EFRONTMATTER"));
    w.ingest(&verb_finished("s", 6, "build"));
    let f = w.fatal.as_ref().unwrap();
    assert!(f.from_followup);
    assert_eq!(f.diagnostics[0].code, "EFRONTMATTER");
}

#[test]
fn a_finished_recovery_build_closes_the_window_before_later_ir_output() {
    let mut w = WatchLog::default();
    w.ingest(&engine("s", 0, hello()));
    w.ingest(&fatal_initial("s", 1));
    w.ingest(&ended("s", 2, None));
    w.ingest(&build_finished("s", 3));
    w.ingest(&verb_finished("s", 4, "build"));
    w.ingest(&diagnostic_failure("s", 5, "EIR"));
    w.ingest(&verb_finished("s", 6, "ir"));
    let f = w.fatal.as_ref().unwrap();
    assert!(!f.from_followup);
    assert!(f.diagnostics.is_empty());
}

#[test]
fn a_new_watch_session_closes_the_fatal_recovery_window() {
    let mut w = WatchLog::default();
    w.ingest(&engine("s", 0, hello()));
    w.ingest(&fatal_initial("s", 1));
    w.ingest(&ended("s", 2, None));
    w.ingest(&engine("r", 0, hello()));
    w.ingest(&diagnostic_failure("r", 1, "ENEW"));
    let f = w.fatal.as_ref().unwrap();
    assert!(!f.from_followup);
    assert!(f.diagnostics.is_empty(), "unrelated diagnostics grafted");
    assert_eq!(w.session.as_deref(), Some("r"));
}

#[test]
fn watch_process_exit_and_last_verb_exit_are_separate_facts() {
    use cv_core::events::engine::WatchExit;
    let mut m = module(Facts::default());
    m.ingest(&engine("s", 0, hello()));
    m.ingest(&engine(
        "s",
        1,
        EngineEvent::VerbFinished {
            verb: "ir".into(),
            exit: ExitClass::Ok,
        },
    ));
    m.ingest(&ended("s", 2, Some(WatchExit::Exited { code: 1 })));
    let r = rows(&m, "watch");
    let process = r.iter().find(|r| r.key == "watch.process-exit").unwrap();
    assert_eq!(process.title, "Watch process exit");
    assert_eq!(process.detail, "1 (content)");
    let verb = r.iter().find(|r| r.key == "watch.verb-exit").unwrap();
    assert_eq!(verb.title, "Last verb exit");
    assert_eq!(verb.detail, "0 (ok)");
    assert!(r.iter().all(|r| r.title != "Last exit"));
}

#[test]
fn a_watch_is_dead_after_disclosure_until_the_next_handshake() {
    let mut w = WatchLog::default();
    w.ingest(&engine("s1", 0, hello()));
    assert!(w.live());
    w.ingest(&env(
        "shell",
        1,
        AppEvent::Shell(ShellEvent::Degraded {
            reason: DegradedReason::WatchDead,
        }),
    ));
    assert!(!w.live() && w.dead);
    w.ingest(&engine("s2", 0, hello()));
    assert!(w.live());
    assert_eq!(w.session.as_deref(), Some("s2"));
}

#[test]
fn an_absent_engine_lists_every_searched_path_and_counts_as_a_problem() {
    let m = module(Facts {
        engine: EngineState::NotFound,
        candidates: vec![
            (EngineSource::Bundled, "/app/engines/boris".into()),
            (EngineSource::Path, "/usr/bin/boris".into()),
        ],
        ..Facts::default()
    });
    let r = rows(&m, "engine");
    assert_eq!(r[0].title, "Boris not found");
    assert_eq!(r[0].detail, "2 searched");
    assert_eq!(r[0].rail, Some(Tone::Error));
    let nav = m.nav();
    assert_eq!(nav.items[0].count, Some(1));
    let search = m.reading("engine.search");
    let paths: Vec<_> = search.lines.iter().map(|l| l.text.as_str()).collect();
    assert!(paths.contains(&"/app/engines/boris") && paths.contains(&"/usr/bin/boris"));
}

#[test]
fn a_located_engine_shows_its_path_source_and_build_info() {
    let m = module(Facts {
        engine: located("/opt/boris/bin/boris"),
        pin: Pin {
            version: "boris/0.9.0".into(),
            vcs_revision: "66efcfb".into(),
        },
        candidates: vec![(EngineSource::Path, "/opt/boris/bin/boris".into())],
        ..Facts::default()
    });
    let keys: Vec<_> = rows(&m, "engine").into_iter().map(|r| r.key).collect();
    assert_eq!(
        keys,
        [
            "engine.status",
            "engine.path",
            "engine.source",
            "engine.version",
            "engine.build-info",
            "engine.pin",
            "engine.search"
        ]
    );
    assert_eq!(m.row(Some("engine"), 1).title, "/opt/boris/bin/boris");
    assert_eq!(m.row(Some("engine"), 2).detail, "PATH");
    let pin = m.row(Some("engine"), 5);
    assert_eq!(pin.badge, Some(Chip::new("match", Tone::Ok)));
    assert!(m.nav().items.iter().all(|n| n.count.is_none()));
    let search = m.reading("engine.search");
    assert_eq!(search.lines.last().unwrap().label.as_deref(), Some("used"));
}

#[test]
fn a_version_mismatch_names_what_was_found_and_the_pin() {
    let m = module(Facts {
        engine: EngineState::VersionMismatch {
            source: EngineSource::Path,
            path: "/usr/local/bin/boris".into(),
            found: "boris/0.8.2".into(),
            found_vcs: Some("ce40d78".into()),
        },
        pin: Pin {
            version: "boris/0.9.0".into(),
            vcs_revision: "66efcfb".into(),
        },
        ..Facts::default()
    });
    let r = rows(&m, "engine");
    assert_eq!(r[0].title, "boris/0.8.2");
    assert_eq!(r[0].detail, "PATH");
    let status = m.reading("engine.status");
    assert!(
        status
            .lines
            .iter()
            .any(|l| l.text == "boris/0.9.0 @ 66efcfb")
    );
    let pin = r.iter().find(|r| r.key == "engine.pin").unwrap();
    assert_eq!(pin.badge, Some(Chip::new("differs", Tone::Warn)));
}

#[test]
fn store_rows_disclose_stamps_locks_and_index() {
    let m = module(Facts {
        store: Store {
            root: Some("/ws".into()),
            namespaces: vec![
                Namespace {
                    name: "research".into(),
                    dir: ".cv/research".into(),
                    supported: 1,
                    stamp: Stamp::Version(1),
                    lock: Some(Lock {
                        path: ".cv/research.writer.lock".into(),
                        holder: LockHolder::OtherProcess,
                    }),
                    index: Some("/data/research/index.sqlite".into()),
                    error: None,
                },
                Namespace {
                    name: "ledger".into(),
                    dir: ".cv/ledger".into(),
                    supported: 1,
                    stamp: Stamp::Version(4),
                    lock: None,
                    index: None,
                    error: None,
                },
            ],
        },
        ..Facts::default()
    });
    let r = rows(&m, "store");
    let by = |k: &str| r.iter().find(|r| r.key == k).unwrap().clone();
    assert_eq!(
        by("store.ns.research").badge,
        Some(Chip::new("current", Tone::Ok))
    );
    assert_eq!(by("store.lock.research").detail, "another process");
    assert_eq!(
        by("store.index.research").title,
        "/data/research/index.sqlite"
    );
    assert_eq!(
        by("store.ns.ledger").badge,
        Some(Chip::new("too new", Tone::Error))
    );
    assert_eq!(m.nav().items[3].count, Some(2));
}

#[test]
fn the_fatal_row_carries_its_diagnostics_into_the_inspector() {
    let mut m = module(Facts::default());
    m.ingest(&engine(
        "s",
        0,
        EngineEvent::BuildFailed {
            phase: BuildPhase::Rebuild,
            diagnostics: vec![diag("EPARENTMISSING")],
            recoverable: false,
        },
    ));
    let row = rows(&m, "watch")
        .into_iter()
        .find(|r| r.key == "watch.fatal")
        .unwrap();
    assert_eq!(row.detail, "1 diagnostic");
    assert_eq!(
        m.inspector("watch.fatal").diagnostics[0].code,
        "EPARENTMISSING"
    );
    assert!(
        m.reading("watch.fatal")
            .lines
            .iter()
            .any(|l| l.text == "EPARENTMISSING: index.md:2:3: unknown key")
    );
}

#[test]
fn the_support_bundle_leaves_out_paths_env_values_and_secret_fields() {
    let mut f = Facts {
        engine: located("/home/ana/bin/boris"),
        candidates: vec![
            (EngineSource::EnvOverride, "/home/ana/bin/boris".into()),
            (EngineSource::Path, "/usr/bin/boris".into()),
        ],
        probe: Some(Probe {
            os: "linux".into(),
            caps: EngineCaps::default(),
            native_watch: false,
            evidence: vec!["scratch at /home/ana/tmp/cv-probe".into()],
        }),
        redact: vec![
            ("/home/ana/work/site".into(), "<workspace>".into()),
            ("/home/ana".into(), "~".into()),
        ],
        ..Facts::default()
    };
    f.store.root = Some("/home/ana/work/site".into());
    f.store.namespaces.push(Namespace {
        name: "research".into(),
        dir: ".cv/research".into(),
        supported: 1,
        stamp: Stamp::Version(1),
        lock: None,
        index: Some("/home/ana/.local/share/capture-vector/k/research/index.sqlite".into()),
        error: Some("research silo already open (writer lock at \"/home/ana/work/site/.cv/research.writer.lock\")".into()),
    });
    let text = bundle::render(&f, &WatchLog::default());
    assert!(!text.contains("/home/ana"), "{text}");
    assert!(!text.contains("/usr/bin"), "{text}");
    assert!(!text.contains("leak"), "{text}");
    assert!(
        text.contains("<workspace>/.cv/research.writer.lock"),
        "{text}"
    );
    assert!(text.contains("~/tmp/cv-probe"), "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["format"], bundle::FORMAT);
    assert_eq!(v["engine"]["binary"], "boris");
    assert_eq!(v["engine"]["source"], "Path");
    assert_eq!(v["engine"]["candidatesSearched"], 2);
    assert_eq!(v["engine"]["buildInfo"]["vcsRevision"], "66efcfb");
    assert_eq!(v["store"]["namespaces"][0]["hasIndex"], true);
}

#[test]
fn rebuild_index_is_a_workspace_command_and_requests_reopen_for_maintenance() {
    let mut facts = Facts::default();
    facts.store.root = Some("/workspace".into());
    let mut m = module(facts);
    let cmds = m.commands();
    assert!(
        cmds.iter().any(|c| c.id == "doctor.rebuild-index"),
        "{cmds:?}"
    );
    let intents = m.run("doctor.rebuild-index");
    assert_eq!(intents.len(), 2, "{intents:?}");
    assert_eq!(intents[1], Intent::RebuildIndexes);

    let mut wb = Workbench::new(Registry::default());
    wb.workspace = Some(std::path::PathBuf::from("/workspace"));
    wb.apply(intents[1].clone());
    assert_eq!(
        wb.requests,
        vec![ShellRequest::OpenWorkspace(std::path::PathBuf::from(
            "/workspace"
        ))]
    );
    assert!(wb.take_index_rebuild_request());
    assert!(!wb.take_index_rebuild_request());
}

#[test]
fn copy_bundle_puts_the_bundle_on_the_clipboard() {
    let mut wb = Workbench::new(Registry::with_all());
    wb.run_command(COPY_BUNDLE);
    let [ShellRequest::Copy(text)] = wb.requests.as_slice() else {
        panic!("{:?}", wb.requests);
    };
    assert!(text.contains(bundle::FORMAT));
    assert!(
        wb.status.starts_with("Support bundle copied"),
        "{}",
        wb.status
    );
}

#[test]
fn shell_doctor_opens_doctor_on_the_engine_verdict() {
    let mut wb = Workbench::new(Registry::with_all());
    wb.run_command("shell.doctor");
    let m = wb.registry.get(wb.active).unwrap();
    assert_eq!(m.spec().id, ID);
    assert_eq!(wb.active_nav(), Some("engine"));
    assert_eq!(wb.selected_key(), Some("engine.status"));
    let ids: Vec<_> = wb.commands().into_iter().map(|c| c.id).collect();
    assert!(ids.contains(&"shell.doctor".to_string()));
    assert!(ids.contains(&COPY_BUNDLE.to_string()));
    assert!(!ids.contains(&"shell.goto.doctor".to_string()));
}
