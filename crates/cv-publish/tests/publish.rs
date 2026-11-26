mod support;

use cv_core::engine::{ExitClass, Verb};
use cv_core::id::StableId;
use cv_core::publish::{RunState, RunStatus};
use cv_core::sys::Hash;
use cv_ledger::LedgerIndex;
use cv_ledger::journal::Entry;
use cv_publish::adapters::{
    COPY_TO_FOLDER, CopyToFolder, Delivered, Delivery, DeliveryAdapter, HTTP_POST, HttpPost,
    HttpTransport, RUN_COMMAND, RunCommand, TransportFailure,
};
use cv_publish::exports;
use cv_publish::plan::{PlanFile, SealedPlan, load_sealed};
use cv_publish::preflight::{PreflightArtifacts, graph_file, run_dir, site_dir};
use cv_publish::secrets::{MockKeychain, NoSecrets, Secret, SecretBackend, StdinSecret};
use cv_publish::{
    Observation, PreflightOutcome, PreflightRequest, PublishError, Publisher, Ruleset,
    RulesetVerdict, Target, TargetConfig, registry,
};
use std::cell::{Cell, RefCell};
use std::path::Path;
use support::*;

// ── registry ─────────────────────────────────────────────────────────

#[test]
fn registry_labels_match_boris() {
    let r = registry();
    let labels: Vec<_> = r.iter().map(|t| (t.id(), t.verification_label())).collect();
    assert_eq!(
        labels,
        vec![
            ("github-pages", "verified"),
            ("standard-site", "verified"),
            ("nostr-nip23", "unverified"),
            ("static-dist", "unverified"),
        ]
    );
    for t in r {
        assert_eq!(Target::from_id(t.id()), Some(t));
        assert_eq!(t.spec().verified, t.verified());
        assert_eq!(
            t.deploy_profile("https://x.test/").base_url,
            "https://x.test/"
        );
        assert_eq!(!t.default_exports().is_empty(), t == Target::GithubPages);
    }
    assert!(Target::GithubPages.deploy_profile("u").expects_robots_txt);
    assert_eq!(Target::NostrNip23.secret_name(), Some("nostr-nsec"));
    assert!(!TargetConfig::new(Target::NostrNip23, "p", "u", "a", "d").verified);
}

// ── sealed grant ─────────────────────────────────────────────────────

fn seal_case(
    tamper: impl FnOnce(&Path, &StableId, &FakeEngine, &Rules, &mut TargetConfig),
) -> Vec<&'static str> {
    let dir = tempfile::tempdir().unwrap();
    let eng = FakeEngine::new(dir.path());
    let rules = Rules::ok();
    let mut c = cfg();
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets)
        .unwrap()
        .with_adapter(flaky(dir.path()));
    let run = granted(
        p.preflight(
            &PreflightRequest { config: c.clone() },
            &rules,
            &mut ids("run"),
        )
        .unwrap(),
    );
    p.approve(&run, "tester").unwrap();
    tamper(dir.path(), &run, &eng, &rules, &mut c);
    let err = p.publish(&run, &c, &rules).unwrap_err();
    let PublishError::SealBroken(broken) = err else {
        panic!("expected SealBroken, got {err:?}")
    };
    let v = p.run(&run).unwrap();
    assert!(v.history.is_empty(), "refused runs never reach Dispatching");
    assert!(v.refusals.iter().any(|r| r.starts_with("seal broken")));
    assert!(!dir.path().join("deployed").exists(), "nothing delivered");
    broken
}

#[test]
fn hash_mismatch_refuses_publish() {
    assert_eq!(
        seal_case(|root, run, _, _, _| std::fs::write(
            root.join(site_dir(run).unwrap()).join("index.html"),
            "<h1>pwned</h1>"
        )
        .unwrap()),
        vec!["artifact_set", "plan"]
    );
    assert_eq!(
        seal_case(|root, run, _, _, _| std::fs::write(
            root.join(graph_file(run).unwrap()),
            "changed"
        )
        .unwrap()),
        vec!["graph"]
    );
    assert_eq!(
        seal_case(|root, run, _, _, _| std::fs::write(
            root.join(site_dir(run).unwrap())
                .join("_boris/proof/artifacts.json"),
            "changed"
        )
        .unwrap()),
        vec!["artifact_set", "plan"]
    );
    assert_eq!(seal_case(|_, _, _, r, _| r.0.set(9)), vec!["ruleset"]);
    assert_eq!(
        seal_case(|_, _, e, _, _| *e.version.lock().unwrap() = "boris/0.9.1".into()),
        vec!["boris_version"]
    );
    assert_eq!(
        seal_case(|_, _, _, _, c| c.deploy.base_url = "https://evil.test/".into()),
        vec!["target_cfg"]
    );
}

#[test]
fn restarting_the_id_generator_preserves_the_old_run_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let eng = FakeEngine::new(dir.path());
    let rules = Rules::ok();
    let req = PreflightRequest { config: cfg() };
    let preflight = || {
        let mut publisher = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
        granted(publisher.preflight(&req, &rules, &mut ids("run")).unwrap())
    };
    let first = preflight();
    let evidence = dir
        .path()
        .join(run_dir(&first).unwrap())
        .join("evidence.txt");
    std::fs::write(&evidence, "preserve").unwrap();
    let second = preflight();
    assert_ne!(first, second);
    assert_eq!(std::fs::read_to_string(evidence).unwrap(), "preserve");
    assert!(dir.path().join(site_dir(&second).unwrap()).is_dir());
}

#[test]
fn preflight_preserves_an_existing_run_directory_and_uses_the_next_id() {
    let dir = tempfile::tempdir().unwrap();
    let eng = FakeEngine::new(dir.path());
    let mut publisher = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
    let first = StableId(format!(
        "run-000000-{}",
        publisher.journal().records().len()
    ));
    let existing = dir.path().join(run_dir(&first).unwrap());
    std::fs::create_dir_all(&existing).unwrap();
    let evidence = existing.join("evidence.txt");
    std::fs::write(&evidence, "preserve").unwrap();

    let run = granted(
        publisher
            .preflight(
                &PreflightRequest { config: cfg() },
                &Rules::ok(),
                &mut ids("run"),
            )
            .unwrap(),
    );
    assert_ne!(run, first);
    assert_eq!(std::fs::read_to_string(evidence).unwrap(), "preserve");
}

#[test]
fn run_ids_must_be_single_components() {
    let run = StableId("group/run".into());
    assert!(run_dir(&run).is_err());
    assert!(site_dir(&run).is_err());
    assert!(graph_file(&run).is_err());
}

/// The sealed-plan grammar is the host's: traversal, absolute, and empty
/// spellings are refused on every host. Windows-syntax separators are host
/// grammar — on unix they are ordinary confined file names; on Windows the
/// component parser refuses the same traversal shapes there.
#[test]
fn sealed_inventory_rejects_escaping_file_names() {
    for path in ["../one.html", "/one.html", "", "nested/../../one.html"] {
        let dir = tempfile::tempdir().unwrap();
        let plan = SealedPlan {
            target: "static-dist".into(),
            adapter: COPY_TO_FOLDER.into(),
            destination: "out".into(),
            site: "site".into(),
            files: vec![PlanFile {
                kind: None,
                advertised: None,
                path: path.into(),
                bytes: 0,
                sha256: Hash([0; 32]),
            }],
        };
        assert!(
            matches!(
                load_sealed(dir.path(), &plan),
                Err(PublishError::Refused(_))
            ),
            "{path:?} must be refused"
        );
    }
}

#[test]
fn publish_requires_persisted_approval() {
    let dir = tempfile::tempdir().unwrap();
    let eng = FakeEngine::new(dir.path());
    let rules = Rules::ok();
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets)
        .unwrap()
        .with_adapter(flaky(dir.path()));
    let run = granted(
        p.preflight(&PreflightRequest { config: cfg() }, &rules, &mut ids("run"))
            .unwrap(),
    );
    assert!(
        matches!(p.publish(&run, &cfg(), &rules), Err(PublishError::Refused(r)) if r == "not approved")
    );
    p.approve(&run, "tester").unwrap();
    // Approval is in the durable journal before any dispatch record.
    let recs = p.journal().records();
    let approved = recs
        .iter()
        .position(|r| matches!(r.entry, Entry::Approved { .. }))
        .unwrap();
    assert!(
        !recs
            .iter()
            .any(|r| matches!(r.entry, Entry::RunState { .. }))
    );
    assert_eq!(
        p.publish(&run, &cfg(), &rules).unwrap(),
        RunState::Complete(RunStatus::Complete)
    );
    let recs = p.journal().records();
    let dispatching = recs
        .iter()
        .position(|r| {
            matches!(
                &r.entry,
                Entry::RunState {
                    state: RunState::Dispatching,
                    ..
                }
            )
        })
        .unwrap();
    assert!(approved < dispatching);
}

#[test]
fn preflight_blocks_on_check_or_ruleset() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = FakeEngine::new(dir.path());
    eng.check_exit = ExitClass::ContentFailure;
    let rules = Rules(Cell::new(1), vec!["ECANONICAL: canonical mismatch".into()]);
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
    let out = p
        .preflight(&PreflightRequest { config: cfg() }, &rules, &mut ids("run"))
        .unwrap();
    let PreflightOutcome::Blocked { run, blocks } = out else {
        panic!("expected blocked")
    };
    assert_eq!(blocks, vec!["check: ContentFailure"]);
    assert_eq!(p.run(&run).unwrap().phase().as_str(), "preflight-blocked");
    assert!(p.approve(&run, "tester").is_err());
    drop(p);

    eng.check_exit = ExitClass::Ok;
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
    let out = p
        .preflight(&PreflightRequest { config: cfg() }, &rules, &mut ids("r2"))
        .unwrap();
    assert!(
        matches!(out, PreflightOutcome::Blocked { blocks, .. } if blocks == vec!["ruleset: ECANONICAL: canonical mismatch"])
    );
    // The preflight built into its own per-run dir, never the watch build.
    for v in eng.verbs() {
        if let Verb::Custom { name, args } = v
            && name == "build"
            && args.iter().any(|a| a == "--html-dir")
        {
            assert!(arg(&args, "--html-dir").starts_with(".cv/publish/"));
        }
    }
}

// ── happy path + ship-the-outputs ────────────────────────────────────

#[test]
fn complete_run_ships_exact_bytes_and_machine_exports() {
    let dir = tempfile::tempdir().unwrap();
    let eng = FakeEngine::new(dir.path());
    let rules = Rules::ok();
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets)
        .unwrap()
        .with_adapter(CopyToFolder::new(dir.path()));
    let run = granted(
        p.preflight(&PreflightRequest { config: cfg() }, &rules, &mut ids("run"))
            .unwrap(),
    );
    p.approve(&run, "tester").unwrap();
    assert_eq!(
        p.publish(&run, &cfg(), &rules).unwrap(),
        RunState::Complete(RunStatus::Complete)
    );
    for (path, body) in &eng.pages {
        assert_eq!(
            std::fs::read_to_string(dir.path().join("deployed").join(path)).unwrap(),
            *body
        );
    }
    let v = p.run(&run).unwrap();
    assert!(
        v.exports.is_none(),
        "unhosted static-dist has no machine-export contract: {v:?}"
    );
    assert_eq!(
        v.history,
        vec![
            RunState::Dispatching,
            RunState::Complete(RunStatus::Complete)
        ]
    );
    let events = p.take_events();
    assert_eq!(events.len(), 3, "grant + dispatching + complete");

    let idx = LedgerIndex::open(dir.path(), p.journal()).unwrap();
    assert_eq!(idx.run_phase(&run.0).unwrap().as_deref(), Some("complete"));
    assert_eq!(idx.nouns("delivery", &run.0).unwrap().len(), 1);
}

#[test]
fn unshipped_or_stale_exports_are_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let mut eng = FakeEngine::new(dir.path());
    eng.pages.retain(|(p, _)| *p != "llms.txt");
    let rules = Rules::ok();
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets)
        .unwrap()
        .with_adapter(CopyToFolder::new(dir.path()));
    let mut export_cfg = cfg();
    export_cfg.exports = Target::GithubPages.default_exports();
    let run = granted(
        p.preflight(
            &PreflightRequest {
                config: export_cfg.clone(),
            },
            &rules,
            &mut ids("run"),
        )
        .unwrap(),
    );
    p.approve(&run, "tester").unwrap();
    p.publish(&run, &export_cfg, &rules).unwrap();
    let v = p.run(&run).unwrap();
    let ev = v.exports.unwrap();
    let llms = ev.iter().find(|e| e.kind == "llms").unwrap();
    assert_eq!(llms.status, exports::MISSING);
    assert!(!exports::all_shipped(&ev));

    // Stale at the destination after delivery.
    let plan: SealedPlan = serde_json::from_value(v.plan.unwrap()).unwrap();
    std::fs::write(dir.path().join("deployed/sitemap.xml"), "old").unwrap();
    let read = |path: &str| std::fs::read(dir.path().join("deployed").join(path)).ok();
    let ev = exports::check(&plan, &export_cfg.exports, Some(&read));
    assert_eq!(
        ev.iter().find(|e| e.kind == "sitemap").unwrap().status,
        exports::STALE
    );
    assert!(
        exports::check(&plan, &export_cfg.exports, None)
            .iter()
            .all(|e| e.status == exports::UNCONFIRMED || e.status == exports::MISSING)
    );
}

// ── crash-after-contact ──────────────────────────────────────────────

#[test]
fn crash_after_contact_is_unknown_reconciles_and_never_retries() {
    let dir = tempfile::tempdir().unwrap();
    let eng = FakeEngine::new(dir.path());
    let rules = Rules::ok();
    let mut run_ids = ids("run");
    let run = {
        let mut a = flaky(dir.path());
        a.crash_after = Some(2);
        let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets)
            .unwrap()
            .with_adapter(a);
        let run = granted(
            p.preflight(&PreflightRequest { config: cfg() }, &rules, &mut run_ids)
                .unwrap(),
        );
        p.approve(&run, "tester").unwrap();
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            p.publish(&run, &cfg(), &rules)
        }));
        assert!(crashed.is_err());
        run
        // process "dies": publisher dropped without recording an outcome
    };

    let retry = flaky(dir.path());
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets)
        .unwrap()
        .with_adapter(retry);
    let v = p.run(&run).unwrap();
    assert_eq!(
        v.state,
        Some(RunState::Unknown),
        "recovered to unknown, not retried"
    );
    assert_eq!(v.history, vec![RunState::Dispatching, RunState::Unknown]);

    // The same run can never be dispatched again.
    assert!(
        matches!(p.publish(&run, &cfg(), &rules), Err(PublishError::Refused(r)) if r.contains("never retried"))
    );
    // A fresh run on the same target is blocked until reconciliation.
    let run2 = granted(
        p.preflight(&PreflightRequest { config: cfg() }, &rules, &mut run_ids)
            .unwrap(),
    );
    p.approve(&run2, "tester").unwrap();
    assert!(
        matches!(p.publish(&run2, &cfg(), &rules), Err(PublishError::Refused(r)) if r.contains("unresolved"))
    );
    assert!(p.run(&run2).unwrap().history.is_empty());

    // Read-only probe of the destination sealed for this run.
    assert_eq!(
        p.reconcile_by_probe(&run).unwrap(),
        Some(RunState::Complete(RunStatus::Partial))
    );
    let v = p.run(&run).unwrap();
    assert_eq!(v.reconciliations.len(), 1);
    assert!(
        v.exports.is_none(),
        "unhosted static-dist has no machine-export evidence to check"
    );
    assert!(
        p.reconcile(&run, Observation::NotDelivered, "tester")
            .is_err(),
        "terminal"
    );

    // Now the target is free; run2 delivers. run1 was never re-delivered.
    assert_eq!(
        p.publish(&run2, &cfg(), &rules).unwrap(),
        RunState::Complete(RunStatus::Complete)
    );
    assert_eq!(p.run(&run).unwrap().history.len(), 3);
}

#[test]
fn transport_failure_classifies_by_contact() {
    for (contacted, want) in [
        (true, RunState::Unknown),
        (false, RunState::Complete(RunStatus::Failed)),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let eng = FakeEngine::new(dir.path());
        let rules = Rules::ok();
        let mut a = flaky(dir.path());
        a.fail = Some(TransportFailure {
            contacted,
            message: "connection reset".into(),
        });
        let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets)
            .unwrap()
            .with_adapter(a);
        let run = granted(
            p.preflight(&PreflightRequest { config: cfg() }, &rules, &mut ids("run"))
                .unwrap(),
        );
        p.approve(&run, "tester").unwrap();
        assert_eq!(p.publish(&run, &cfg(), &rules).unwrap(), want);
        assert!(p.publish(&run, &cfg(), &rules).is_err(), "never retried");
    }
}

#[test]
fn committed_delivery_without_dispatching_recovers_unknown_and_suppresses_retry() {
    let dir = tempfile::tempdir().unwrap();
    let eng = FakeEngine::new(dir.path());
    let rules = Rules::ok();
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets)
        .unwrap()
        .with_adapter(flaky(dir.path()));
    let run = granted(
        p.preflight(&PreflightRequest { config: cfg() }, &rules, &mut ids("run"))
            .unwrap(),
    );
    p.approve(&run, "tester").unwrap();
    let grant = p.run(&run).unwrap().grant.unwrap();
    let plan_hash = grant.plan_hash;
    drop(p);

    // A crash can commit the pre-contact Delivery evidence but die before
    // the separate Dispatching record. That evidence alone must mean the
    // transport may have been contacted.
    let mut journal = cv_ledger::Journal::open(dir.path()).unwrap();
    journal
        .append(
            CLOCK.0,
            Entry::Delivery(cv_ledger::nouns::DeliveryNoun {
                id: cv_ledger::nouns::derive_id("dlv", &[&run.0, COPY_TO_FOLDER]),
                run: run.clone(),
                target: Target::StaticDist.id().into(),
                adapter: COPY_TO_FOLDER.into(),
                plan_hash,
                files: 2,
                bytes: 10,
            }),
        )
        .unwrap();
    drop(journal);

    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets)
        .unwrap()
        .with_adapter(flaky(dir.path()));
    let v = p.run(&run).unwrap();
    assert_eq!(v.state, Some(RunState::Unknown));
    assert!(v.was_dispatched());
    assert!(
        matches!(p.publish(&run, &cfg(), &rules), Err(PublishError::Refused(r)) if r.contains("never retried"))
    );
    assert!(
        !dir.path().join("deployed").exists(),
        "delivery evidence without a terminal state must never be retried"
    );
}

#[test]
fn an_orphan_run_directory_makes_preflight_choose_the_next_unused_id() {
    let dir = tempfile::tempdir().unwrap();
    let orphan = dir
        .path()
        .join(run_dir(&StableId("run-000000-0".into())).unwrap());
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("evidence.txt"), "preserve").unwrap();
    let eng = FakeEngine::new(dir.path());
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();

    let run = granted(
        p.preflight(
            &PreflightRequest { config: cfg() },
            &Rules::ok(),
            &mut ids("run"),
        )
        .unwrap(),
    );

    assert_ne!(run, StableId("run-000000-0".into()));
    assert_eq!(
        std::fs::read_to_string(orphan.join("evidence.txt")).unwrap(),
        "preserve"
    );
    assert!(dir.path().join(site_dir(&run).unwrap()).is_dir());
}

#[cfg(unix)]
#[test]
fn a_symlinked_sealed_site_is_a_durable_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let eng = FakeEngine::new(dir.path());
    let rules = Rules::ok();
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets)
        .unwrap()
        .with_adapter(CopyToFolder::new(dir.path()));
    let run = granted(
        p.preflight(&PreflightRequest { config: cfg() }, &rules, &mut ids("run"))
            .unwrap(),
    );
    p.approve(&run, "tester").unwrap();
    let site = dir.path().join(site_dir(&run).unwrap());
    std::fs::remove_dir_all(&site).unwrap();
    std::os::unix::fs::symlink(outside.path(), &site).unwrap();

    let err = p.publish(&run, &cfg(), &rules).unwrap_err();
    assert!(
        matches!(
            err,
            PublishError::Refused(_) | PublishError::SealBroken(_) | PublishError::Io(_)
        ),
        "{err:?}"
    );
    assert!(
        !p.run(&run).unwrap().refusals.is_empty(),
        "the failed seal verification must be durable evidence"
    );
    assert!(!dir.path().join("deployed").exists());
}

// ── adapters ─────────────────────────────────────────────────────────

#[test]
fn run_command_goes_through_the_engine_port() {
    let dir = tempfile::tempdir().unwrap();
    let eng = FakeEngine::new(dir.path());
    let rules = Rules::ok();
    let c = TargetConfig::default_html(
        Target::NostrNip23,
        "https://example.test/",
        RUN_COMMAND,
        "relays",
    );
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets)
        .unwrap()
        .with_adapter(RunCommand::default());
    let run = granted(
        p.preflight(
            &PreflightRequest { config: c.clone() },
            &rules,
            &mut ids("run"),
        )
        .unwrap(),
    );
    p.approve(&run, "tester").unwrap();
    assert_eq!(
        p.publish(&run, &c, &rules).unwrap(),
        RunState::Complete(RunStatus::Complete)
    );
    assert_eq!(eng.publishes(), 1);
    let publish = eng
        .verbs()
        .into_iter()
        .find_map(|v| match v {
            Verb::Custom { name, args } if name == "publish" => Some(args),
            _ => None,
        })
        .unwrap();
    assert_eq!(arg(&publish, "--target"), "nostr-nip23");
    assert!(!publish.iter().any(|a| a == "--profile"));
    assert!(
        Path::new(arg(&publish, "--html-dir")).ends_with(format!(".cv/publish/{}/site", run.0)),
        "engine is pointed at the sealed site, not a target id alone"
    );
    assert!(
        p.run(&run).unwrap().exports.is_none(),
        "record targets ship no site exports"
    );
}

struct MockHttp {
    status: u16,
    seen: RefCell<Vec<(String, Option<String>, String)>>,
}

impl HttpTransport for &MockHttp {
    fn post(
        &self,
        url: &str,
        auth_header: Option<(&'static str, &Secret)>,
        _: &str,
        body: &[u8],
    ) -> Result<u16, TransportFailure> {
        self.seen.borrow_mut().push((
            url.into(),
            auth_header.map(|(n, s)| format!("{n}: {}", s.expose())),
            String::from_utf8(body.to_vec()).unwrap(),
        ));
        Ok(self.status)
    }
}

fn http_run(status: u16, keychain: &MockKeychain) -> (tempfile::TempDir, MockHttp, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let http = MockHttp {
        status,
        seen: RefCell::new(vec![]),
    };
    let mut trail = Vec::new();
    {
        let eng = FakeEngine::new(dir.path());
        let rules = Rules::ok();
        let mut c = TargetConfig::default_html(
            Target::GithubPages,
            "https://example.test/",
            HTTP_POST,
            "https://indexnow.test/ping",
        );
        c.exports.clear();
        let mut p = Publisher::open(dir.path(), &eng, &CLOCK, keychain)
            .unwrap()
            .with_adapter(HttpPost {
                transport: &http,
                secret: Some("indexnow-key".into()),
            });
        let run = granted(
            p.preflight(
                &PreflightRequest { config: c.clone() },
                &rules,
                &mut ids("run"),
            )
            .unwrap(),
        );
        p.approve(&run, "tester").unwrap();
        let st = p.publish(&run, &c, &rules);
        trail.push(format!("{st:?}"));
        if st.as_ref().is_ok_and(|s| *s == RunState::Submitted) {
            let r = p
                .reconcile(&run, Observation::Delivered(RunStatus::Complete), "tester")
                .unwrap();
            trail.push(format!("{r:?}"));
        }
        trail.push(format!("{:?}", p.take_events()));
        trail.push(format!("{:?}", p.runs()));
        trail.extend(eng.verbs().iter().map(|v| format!("{v:?}")));
    }
    (dir, http, trail)
}

#[test]
fn http_post_maps_status_to_state() {
    let kc = MockKeychain::default().with("indexnow-key", "k");
    let (_d, http, trail) = http_run(202, &kc);
    assert!(trail[0].contains("Submitted"));
    assert!(trail[1].contains("Complete(Complete)"));
    let (url, _, body) = &http.seen.borrow()[0];
    assert_eq!(url, "https://indexnow.test/ping");
    assert!(body.contains("https://example.test/guides/a.html"));
    assert!(body.contains(r#""host":"example.test""#));
    assert!(http_run(403, &kc).2[0].contains("Complete(Failed)"));
    assert!(http_run(503, &kc).2[0].contains("Unknown"));
    // Missing secret refuses before contact.
    let (_d, http, trail) = http_run(200, &MockKeychain::default());
    assert!(trail[0].contains("Complete(Failed)"));
    assert!(http.seen.borrow().is_empty());
}

// ── secrets ──────────────────────────────────────────────────────────

const CANARY: &str = "CANARY-7f3a9c-do-not-log";

#[test]
fn secret_canary_never_reaches_logs_events_or_ledger() {
    let kc = MockKeychain::default().with("indexnow-key", CANARY);
    let (dir, http, trail) = http_run(200, &kc);
    // It *was* delivered to the transport boundary as a named header…
    let auth = http.seen.borrow()[0].1.clone().unwrap();
    assert!(
        auth.starts_with("Authorization: "),
        "auth is a header: {auth}"
    );
    assert!(auth.contains(CANARY));
    assert!(!http.seen.borrow()[0].2.contains(CANARY), "not in the body");
    // …and nowhere else: events, run views, engine argv, every file on disk.
    for t in &trail {
        assert!(!t.contains(CANARY), "leaked: {t}");
    }
    fn scan(p: &Path) {
        for e in std::fs::read_dir(p).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                scan(&p);
            } else {
                let b = std::fs::read(&p).unwrap();
                assert!(
                    !b.windows(CANARY.len()).any(|w| w == CANARY.as_bytes()),
                    "canary persisted in {p:?}"
                );
            }
        }
    }
    scan(dir.path());
    assert_eq!(
        format!("{:?}", kc.get("indexnow-key").unwrap()),
        "Secret(<redacted>)"
    );
}

#[test]
fn stdin_channel_reads_once() {
    let s = StdinSecret::new("nostr-nsec", std::io::Cursor::new(format!("{CANARY}\n")));
    assert!(s.get("other").is_err());
    assert_eq!(s.get("nostr-nsec").unwrap().expose(), CANARY);
    assert!(s.get("nostr-nsec").is_err(), "consumed");
}

#[test]
fn copy_to_folder_never_clobbers_siblings() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out");
    std::fs::create_dir_all(&dest).unwrap();
    // Siblings that a naive `with_extension("cv-tmp")` would overwrite.
    std::fs::write(dest.join("index.cv-tmp"), b"keep me").unwrap();
    std::fs::write(dest.join("index.css"), b"keep me too").unwrap();
    let files = vec![(
        PlanFile {
            kind: None,
            advertised: None,
            path: "index.html".into(),
            bytes: 2,
            sha256: cv_publish::plan::sha256(b"hi"),
        },
        b"hi".to_vec(),
    )];
    let c = TargetConfig::new(
        Target::StaticDist,
        "p",
        "https://example.test/",
        COPY_TO_FOLDER,
        "out",
    );
    let plan = SealedPlan {
        target: "static-dist".into(),
        adapter: COPY_TO_FOLDER.into(),
        destination: c.destination.clone(),
        site: "site".into(),
        files: vec![files[0].0.clone()],
    };
    let eng = FakeEngine::new(dir.path());
    let run = StableId("run-x".into());
    let out = CopyToFolder::new(dir.path()).deliver(&Delivery {
        run: &run,
        plan: &plan,
        files: &files,
        site: dir.path(),
        config: &c,
        secrets: &NoSecrets,
        engine: &eng,
    });
    assert_eq!(out.unwrap(), Delivered::Complete(RunStatus::Complete));
    assert_eq!(std::fs::read(dest.join("index.html")).unwrap(), b"hi");
    assert_eq!(
        std::fs::read(dest.join("index.cv-tmp")).unwrap(),
        b"keep me"
    );
    assert_eq!(
        std::fs::read(dest.join("index.css")).unwrap(),
        b"keep me too"
    );
    assert_eq!(
        std::fs::read_dir(&dest).unwrap().count(),
        3,
        "no staging leftovers"
    );
}

#[test]
fn copy_to_folder_accepts_names_at_the_length_limit() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out");
    // Near the common 255-byte component limit: staging must not add to it.
    let name = format!("{}.html", "a".repeat(245));
    let files = vec![(
        PlanFile {
            kind: None,
            advertised: None,
            path: name.clone(),
            bytes: 2,
            sha256: cv_publish::plan::sha256(b"hi"),
        },
        b"hi".to_vec(),
    )];
    let c = TargetConfig::new(
        Target::StaticDist,
        "p",
        "https://example.test/",
        COPY_TO_FOLDER,
        "out",
    );
    let plan = SealedPlan {
        target: "static-dist".into(),
        adapter: COPY_TO_FOLDER.into(),
        destination: c.destination.clone(),
        site: "site".into(),
        files: vec![files[0].0.clone()],
    };
    let eng = FakeEngine::new(dir.path());
    let run = StableId("run-with-a-long-identifier-0000000000".into());
    let out = CopyToFolder::new(dir.path()).deliver(&Delivery {
        run: &run,
        plan: &plan,
        files: &files,
        site: dir.path(),
        config: &c,
        secrets: &NoSecrets,
        engine: &eng,
    });
    assert_eq!(out.unwrap(), Delivered::Complete(RunStatus::Complete));
    assert_eq!(std::fs::read(dest.join(&name)).unwrap(), b"hi");
    assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 1);
}

#[test]
fn copy_to_folder_refuses_absolute_destinations_before_contact() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out");
    let eng = FakeEngine::new(dir.path());
    let mut config = cfg();
    config.destination = dest.to_str().unwrap().into();
    let adapter = CopyToFolder::new(dir.path());
    let plan = SealedPlan {
        target: "static-dist".into(),
        adapter: COPY_TO_FOLDER.into(),
        destination: config.destination.clone(),
        site: "site".into(),
        files: vec![],
    };
    let result = adapter.deliver(&Delivery {
        run: &StableId("run".into()),
        plan: &plan,
        files: &[],
        site: dir.path(),
        config: &config,
        secrets: &NoSecrets,
        engine: &eng,
    });
    assert!(!result.unwrap_err().contacted);
    assert!(adapter.read_back(&config, "index.html").is_none());
    assert!(adapter.probe(&plan, &config).is_none());
    assert!(!dest.exists());
}

// ── tier gate, end to end: cv-rules verdict → Preflight → publish ────

/// The real cv-rules engine behind the Preflight `Ruleset` port: parse
/// the HTML the preflight build sealed, `eval`, and split the report on
/// `AuditFinding::blocks_publish` — the tier decides, nothing else.
struct RealRules(cv_rules::Ruleset);

impl RealRules {
    fn one_failing_rule(tier: &str) -> Self {
        let pack = cv_rules::RulePack::from_toml(&format!(
            r#"
[pack]
id = "gate"
version = 1
description = "t"
[[rule]]
id = "GATE-H1-MISSING-H2"
severity = "Error"
tier = "{tier}"
archetypes = ["Any"]
remediation = "r"
check = {{ Exists = {{ artifact = "Html", selector = {{ Html = "h2" }} }} }}
[rule.citation]
source = "s"
url = "https://example.org/"
frozen_quote = "q"
captured = "2026-11-23"
"#
        ))
        .unwrap();
        Self(cv_rules::Ruleset::new(vec![pack]).unwrap())
    }
}

impl Ruleset for RealRules {
    fn hash(&self) -> Hash {
        self.0.hash()
    }
    fn evaluate(&self, artifacts: &PreflightArtifacts<'_>) -> RulesetVerdict {
        let mut set = cv_rules::ArtifactSet::builder();
        for f in artifacts.files.iter().filter(|f| f.path.ends_with(".html")) {
            let bytes = std::fs::read(artifacts.site.join(&f.path)).unwrap();
            set = set.page(cv_rules::HtmlPage::parse(None, &f.path, &bytes).unwrap());
        }
        let report = cv_rules::eval(&set.build(), &self.0);
        let (blocking, advisory): (Vec<_>, Vec<_>) =
            report.failed().partition(|f| f.blocks_publish());
        let label = |v: Vec<&cv_rules::AuditFinding>| {
            v.into_iter()
                .map(|f| format!("{}: {}", f.finding.code, f.finding.subject.stable.0))
                .collect()
        };
        RulesetVerdict {
            blocking: label(blocking),
            advisory: label(advisory),
        }
    }
}

#[test]
fn heuristic_only_ruleset_cannot_block_preflight_or_publish() {
    // Same failing Error rule over the same built pages; only the tier differs.
    let dir = tempfile::tempdir().unwrap();
    let eng = FakeEngine::new(dir.path());
    let heuristic = RealRules::one_failing_rule("Heuristic");
    let site = dir.path().join("probe");
    for (p, body) in &eng.pages {
        let f = site.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, body).unwrap();
    }
    let files: Vec<PlanFile> = ["index.html", "guides/a.html"]
        .iter()
        .map(|p| PlanFile {
            kind: None,
            advertised: None,
            path: (*p).into(),
            bytes: 0,
            sha256: Hash([0; 32]),
        })
        .collect();
    let kinds = std::collections::BTreeMap::new();
    let v = heuristic.evaluate(&PreflightArtifacts {
        site: &site,
        files: &files,
        kinds: &kinds,
        ir_graph: None,
        config: &cfg(),
    });
    assert_eq!(
        v.advisory.len(),
        2,
        "the heuristic rule really fails: {v:?}"
    );
    assert!(v.blocking.is_empty());

    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets)
        .unwrap()
        .with_adapter(CopyToFolder::new(dir.path()));
    let run = granted(
        p.preflight(
            &PreflightRequest { config: cfg() },
            &heuristic,
            &mut ids("run"),
        )
        .unwrap(),
    );
    p.approve(&run, "tester").unwrap();
    assert_eq!(
        p.publish(&run, &cfg(), &heuristic).unwrap(),
        RunState::Complete(RunStatus::Complete)
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("deployed/index.html")).unwrap(),
        "<h1>home</h1>"
    );

    // Contrast: the identical rule at a blocking tier refuses the grant.
    for tier in ["NormativeSpec", "EngineDocumented"] {
        let blocking = RealRules::one_failing_rule(tier);
        let out = p
            .preflight(
                &PreflightRequest { config: cfg() },
                &blocking,
                &mut ids(&format!("b-{tier}")),
            )
            .unwrap();
        let PreflightOutcome::Blocked { run, blocks } = out else {
            panic!("{tier}: expected blocked")
        };
        assert_eq!(blocks.len(), 2, "{tier}: {blocks:?}");
        assert!(
            blocks
                .iter()
                .all(|b| b.starts_with("ruleset: GATE-H1-MISSING-H2"))
        );
        assert!(p.approve(&run, "tester").is_err());
    }
}

// ── advisory findings + discarded grants ─────────────────────────────

#[test]
fn advisory_findings_are_recorded_with_the_grant_and_never_block() {
    let dir = tempfile::tempdir().unwrap();
    let eng = FakeEngine::new(dir.path());
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
    let run = granted(
        p.preflight(
            &PreflightRequest { config: cfg() },
            &Rules::ok(),
            &mut ids("run"),
        )
        .unwrap(),
    );
    let v = p.run(&run).unwrap();
    assert_eq!(v.advisory, ["heuristic: meta description short"]);
    assert_eq!(v.phase().as_str(), "granted");
}

#[test]
fn a_discarded_grant_can_be_neither_approved_nor_published() {
    let dir = tempfile::tempdir().unwrap();
    let eng = FakeEngine::new(dir.path());
    let rules = Rules::ok();
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets)
        .unwrap()
        .with_adapter(CopyToFolder::new(dir.path()));
    let run = granted(
        p.preflight(&PreflightRequest { config: cfg() }, &rules, &mut ids("run"))
            .unwrap(),
    );
    p.approve(&run, "tester").unwrap();
    p.discard(&run, "tester").unwrap();
    assert_eq!(p.run(&run).unwrap().phase().as_str(), "discarded");
    assert!(matches!(
        p.publish(&run, &cfg(), &rules),
        Err(PublishError::Refused(r)) if r == "grant discarded"
    ));
    assert!(p.approve(&run, "tester").is_err());
    assert!(p.discard(&run, "tester").is_err());
    assert!(!dir.path().join("deployed").exists());
}

#[test]
fn a_dispatched_run_cannot_be_discarded_or_approved_again() {
    let dir = tempfile::tempdir().unwrap();
    let eng = FakeEngine::new(dir.path());
    let rules = Rules::ok();
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets)
        .unwrap()
        .with_adapter(CopyToFolder::new(dir.path()));
    let run = granted(
        p.preflight(&PreflightRequest { config: cfg() }, &rules, &mut ids("run"))
            .unwrap(),
    );
    p.approve(&run, "tester").unwrap();
    p.publish(&run, &cfg(), &rules).unwrap();
    assert!(p.discard(&run, "tester").is_err());
    assert!(p.approve(&run, "tester").is_err());
    assert_eq!(p.run(&run).unwrap().phase().as_str(), "complete");
}

#[test]
fn preflight_argv_is_what_boris_0_9_0_accepts() {
    let run = StableId("run-1".into());
    let argv: Vec<(String, Vec<String>)> = cv_publish::preflight::verbs(&cfg(), &run)
        .unwrap()
        .into_iter()
        .map(|(_, v)| match v {
            Verb::Custom { name, args } => (name, args),
            other => panic!("{other:?}"),
        })
        .collect();
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(
        argv,
        [
            ("build".into(), s(&["--html-dir", ".cv/publish/run-1/site"])),
            (
                "validate".into(),
                s(&["--html-dir", ".cv/publish/run-1/site"])
            ),
            ("check".into(), vec![]),
            (
                "proof".into(),
                s(&["verify", "--html-dir", ".cv/publish/run-1/site"])
            ),
            ("build".into(), s(&["--out", ".cv/publish/run-1/ir"])),
        ]
    );
}
