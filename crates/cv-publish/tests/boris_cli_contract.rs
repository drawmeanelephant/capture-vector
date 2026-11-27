//! Static drift gate for the Boris argv shapes cv-publish constructs.
//! This test never spawns a process; the black-box probe lives in cv-engine.

mod support;

use cv_core::engine::Verb;
use cv_core::id::StableId;
use cv_publish::adapters::{Delivery, DeliveryAdapter, RUN_COMMAND, RunCommand};
use cv_publish::plan::SealedPlan;
use cv_publish::preflight::verbs;
use cv_publish::secrets::NoSecrets;
use cv_publish::{Target, TargetConfig};
use std::path::Path;
use support::FakeEngine;

const CONTRACT_JSON: &str = include_str!("../../../fixtures/boris-cli-contract.json");
const SITE: &str = ".cv/publish/run-1/site";

fn contract_shape(id: &str) -> Vec<String> {
    let c: serde_json::Value =
        serde_json::from_str(CONTRACT_JSON).expect("boris CLI contract JSON parses");
    assert_eq!(c["format"], "cv-boris-cli-contract");
    assert_eq!(c["schema_version"], 1);
    c["shapes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap_or_else(|| panic!("contract shape {id} missing"))["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().to_string())
        .collect()
}

fn verb_argv(verb: &Verb) -> Vec<String> {
    let Verb::Custom { name, args } = verb else {
        panic!("cv-publish shapes are custom verbs: {verb:?}")
    };
    std::iter::once(name.clone())
        .chain(args.iter().cloned())
        .collect()
}

#[test]
fn preflight_shapes_match_checked_in_contract() {
    let cfg = TargetConfig::default_html(
        Target::StaticDist,
        "https://example.test/",
        RUN_COMMAND,
        "deployed",
    );
    let run = StableId("run-1".into());
    let got = verbs(&cfg, &run).unwrap();
    let expected = [
        ("build", "publish.preflight.build"),
        ("validate", "publish.preflight.validate"),
        ("check", "publish.preflight.check"),
        ("proof-verify", "publish.preflight.proof-verify"),
        ("ir", "publish.preflight.ir"),
    ];
    assert_eq!(got.len(), expected.len());
    for ((step, verb), (want_step, id)) in got.iter().zip(expected) {
        assert_eq!(*step, want_step);
        assert_eq!(
            contract_shape(id),
            verb_argv(verb),
            "{id} drifted from the checked-in Boris CLI contract"
        );
    }
}

fn run_command_shape(cfg: &TargetConfig) -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    let engine = FakeEngine::new(dir.path());
    let run = StableId("run-1".into());
    let plan = SealedPlan {
        target: cfg.target.id().into(),
        adapter: cfg.adapter.clone(),
        destination: cfg.destination.clone(),
        site: SITE.into(),
        files: vec![],
    };
    let delivery = Delivery {
        run: &run,
        plan: &plan,
        files: &[],
        site: Path::new(SITE),
        config: cfg,
        secrets: &NoSecrets,
        engine: &engine,
    };
    RunCommand::default().deliver(&delivery).unwrap();
    for verb in engine.verbs() {
        if matches!(&verb, Verb::Custom { name, .. } if name == "publish") {
            return verb_argv(&verb);
        }
    }
    panic!("RunCommand did not emit a publish verb")
}

#[test]
fn run_command_shapes_match_checked_in_contract() {
    let no_profile = TargetConfig::default_html(
        Target::NostrNip23,
        "https://example.test/",
        RUN_COMMAND,
        "relays",
    );
    assert_eq!(
        contract_shape("publish.adapter.generic-no-profile"),
        run_command_shape(&no_profile),
        "run-command shape without a profile drifted"
    );

    let named_profile = TargetConfig::new(
        Target::NostrNip23,
        "boris.json",
        "https://example.test/",
        RUN_COMMAND,
        "relays",
    );
    assert_eq!(
        contract_shape("publish.adapter.generic-profile"),
        run_command_shape(&named_profile),
        "run-command shape with a profile drifted"
    );
}
