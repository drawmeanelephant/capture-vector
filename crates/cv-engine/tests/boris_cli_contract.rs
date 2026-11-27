//! Black-box Boris CLI contract probes plus the static shape drift gate.
//!
//! The static tests run everywhere and compare the argv constructors in
//! cv-engine with `fixtures/boris-cli-contract.json`. The black-box probe is
//! deliberately double-gated: `--ignored` and `CV_BORIS_CONTRACT=1`, with the
//! real pinned binary named by `CV_BORIS_BIN`.

use cv_core::engine::{
    Engine, EngineCaps, EngineInfo, EventSink, ExitClass, ExportKind, Fingerprint, PageDraft, Verb,
    WatchHandle, WriteOutcome,
};
use cv_core::events::SessionId;
use cv_core::rules::ArtifactSet;
use cv_engine::{EngineScheduler, SeatPolicy};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const CONTRACT_JSON: &str = include_str!("../../../fixtures/boris-cli-contract.json");
const RUN_DIR: &str = ".cv/publish/run-1";
const SITE: &str = ".cv/publish/run-1/site";
const PROFILE: &str = "boris.json";
const NOSTR_PLAN: &str = ".cv/publish/run-1/nostr-plan.json";
const NOSTR_BUNDLE: &str = ".cv/publish/run-1/nostr-bundle.json";
const STANDARD_SITE_PLAN: &str = ".cv/publish/run-1/standard-site-plan.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prepare {
    None,
    PublishRun,
    ProofSite,
}

impl Prepare {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::PublishRun => "publish-run",
            Self::ProofSite => "proof-site",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ShapeSpec {
    id: &'static str,
    family: &'static str,
    status: &'static str,
    origin: &'static str,
    prepare: Prepare,
    argv: &'static [&'static str],
}

fn shape(
    id: &'static str,
    family: &'static str,
    status: &'static str,
    origin: &'static str,
    prepare: Prepare,
    argv: &'static [&'static str],
) -> ShapeSpec {
    ShapeSpec {
        id,
        family,
        status,
        origin,
        prepare,
        argv,
    }
}

fn catalog() -> Vec<ShapeSpec> {
    use Prepare::{None as N, ProofSite as Proof, PublishRun as Run};
    vec![
        shape(
            "engine.build",
            "build",
            "emitted",
            "cv-engine Verb::Build",
            N,
            &["build"],
        ),
        shape(
            "engine.validate",
            "validate",
            "emitted",
            "cv-engine Verb::Validate",
            N,
            &["validate"],
        ),
        shape(
            "engine.check",
            "check",
            "emitted",
            "cv-engine Verb::Check",
            N,
            &["check", "--format", "json"],
        ),
        shape(
            "engine.plan.profile",
            "profile",
            "emitted",
            "cv-engine Verb::Plan",
            N,
            &["plan", "--profile", PROFILE],
        ),
        shape(
            "engine.graph",
            "graph",
            "emitted",
            "cv-engine Verb::Graph",
            N,
            &["graph"],
        ),
        shape(
            "engine.impact.index",
            "impact",
            "emitted",
            "cv-engine Verb::Impact",
            N,
            &["impact", "index", "--format", "json"],
        ),
        shape(
            "engine.export.out",
            "build",
            "emitted",
            "cv-engine Verb::Export Out",
            N,
            &["build", "--out", ".boris"],
        ),
        shape(
            "engine.export.rag",
            "build",
            "emitted",
            "cv-engine Verb::Export Rag",
            N,
            &["build", "--rag"],
        ),
        shape(
            "engine.export.context",
            "build",
            "emitted",
            "cv-engine Verb::Export Context",
            N,
            &["build", "--context"],
        ),
        shape(
            "engine.export.llms",
            "build",
            "emitted",
            "cv-engine Verb::Export Llms",
            N,
            &["build", "--llms"],
        ),
        shape(
            "engine.export.rss",
            "build",
            "emitted",
            "cv-engine Verb::Export Rss",
            N,
            &["build", "--rss"],
        ),
        shape(
            "engine.export.sitemap",
            "build",
            "emitted",
            "cv-engine Verb::Export Sitemap",
            N,
            &["build", "--sitemap"],
        ),
        shape(
            "engine.ir.live",
            "build",
            "emitted",
            "cv-engine Verb::Ir logical live projection",
            N,
            &["build", "--out", ".cv/ir"],
        ),
        shape(
            "engine.ir.live-input",
            "build",
            "emitted",
            "cv-engine Verb::Ir logical live projection with input",
            N,
            &["build", "--out", ".cv/ir", "--input", "content"],
        ),
        shape(
            "engine.ir.staging",
            "build",
            "emitted",
            "cv-engine staged Verb::Ir child argv",
            N,
            &["build", "--out", ".cv/ir.staging"],
        ),
        shape(
            "engine.ir.staging-input",
            "build",
            "emitted",
            "cv-engine staged Verb::Ir child argv with input",
            N,
            &["build", "--out", ".cv/ir.staging", "--input", "content"],
        ),
        shape(
            "engine.init.docs",
            "init",
            "emitted",
            "cv-engine Verb::Init docs archetype",
            N,
            &["init", "site", "--type", "docs"],
        ),
        shape(
            "engine.probe.validate-input",
            "validate",
            "emitted",
            "cv-engine capability probe validate --input",
            N,
            &["validate", "--input", "content"],
        ),
        shape(
            "engine.scheduler.separate-out-dir",
            "build",
            "emitted",
            "cv-engine scheduler SeparateOutDir first redirect",
            N,
            &["build", "--html-dir", ".cv/out/1"],
        ),
        shape(
            "engine.secrets.nostr-sign",
            "publish",
            "emitted",
            "cv-engine secrets::nostr_sign stdout bundle",
            Run,
            &["nostr", "sign", "--plan", NOSTR_PLAN, "--key-stdin"],
        ),
        shape(
            "engine.secrets.nostr-sign-out",
            "publish",
            "emitted",
            "cv-engine secrets::nostr_sign --out bundle",
            Run,
            &[
                "nostr",
                "sign",
                "--plan",
                NOSTR_PLAN,
                "--key-stdin",
                "--out",
                NOSTR_BUNDLE,
            ],
        ),
        shape(
            "publish.preflight.build",
            "build",
            "emitted",
            "cv-publish preflight::verbs static-dist build",
            Run,
            &["build", "--html-dir", SITE],
        ),
        shape(
            "publish.preflight.validate",
            "validate",
            "emitted",
            "cv-publish preflight::verbs static-dist validate",
            Run,
            &["validate", "--html-dir", SITE],
        ),
        shape(
            "publish.preflight.check",
            "check",
            "emitted",
            "cv-publish preflight::verbs check",
            Run,
            &["check"],
        ),
        shape(
            "publish.preflight.proof-verify",
            "proof",
            "emitted",
            "cv-publish preflight::verbs proof verify",
            Proof,
            &["proof", "verify", "--html-dir", SITE],
        ),
        shape(
            "publish.preflight.ir",
            "build",
            "emitted",
            "cv-publish preflight::verbs IR projection",
            Run,
            &["build", "--out", ".cv/publish/run-1/ir"],
        ),
        shape(
            "publish.adapter.generic-no-profile",
            "publish",
            "emitted",
            "cv-publish RunCommand default without a named profile",
            Proof,
            &["publish", "--target", "nostr-nip23", "--html-dir", SITE],
        ),
        shape(
            "publish.adapter.generic-profile",
            "publish",
            "emitted",
            "cv-publish RunCommand default with a named profile",
            Proof,
            &[
                "publish",
                "--target",
                "nostr-nip23",
                "--profile",
                PROFILE,
                "--html-dir",
                SITE,
            ],
        ),
        shape(
            "profile.build",
            "profile",
            "intended",
            "named publication-profile build baseline",
            N,
            &["build", "--profile", PROFILE],
        ),
        shape(
            "profile.validate",
            "profile",
            "intended",
            "named publication-profile validation",
            N,
            &["validate", "--profile", PROFILE],
        ),
        shape(
            "profile.plan-html-dir-override",
            "profile",
            "intended",
            "plan declaration output override",
            Run,
            &["plan", "--profile", PROFILE, "--html-dir", SITE],
        ),
        shape(
            "profile.build-html-dir-refused",
            "profile",
            "reference",
            "spike redirection shape formerly encoded by preflight",
            Run,
            &["build", "--profile", PROFILE, "--html-dir", SITE],
        ),
        shape(
            "profile.build-target-refused",
            "profile",
            "reference",
            "spike redirection shape through explicit target selection",
            Run,
            &[
                "build",
                "--profile",
                PROFILE,
                "--target",
                "public=.cv/publish/run-1/site",
            ],
        ),
        shape(
            "profile.build-target-exact",
            "profile",
            "reference",
            "explicit selector exactly matching the plain profile target",
            N,
            &["build", "--profile", PROFILE, "--target", "public=dist"],
        ),
        shape(
            "profile.explicit-target-build",
            "profile",
            "intended",
            "normalized-plan explicit CLI compilation seam",
            Run,
            &[
                "build",
                "--input",
                "content",
                "--target",
                "public=.cv/publish/run-1/site",
                "--theme",
                "themes/boris",
            ],
        ),
        shape(
            "reference.check-html-dir-refused",
            "check",
            "reference",
            "former preflight check shape",
            Run,
            &["check", "--html-dir", SITE],
        ),
        shape(
            "nostr.plan",
            "publish",
            "intended",
            "native Nostr unsigned plan artifact chain",
            N,
            &["nostr", "plan", "--profile", PROFILE],
        ),
        shape(
            "nostr.publish",
            "publish",
            "intended",
            "native Nostr exact signed-event publication",
            Run,
            &[
                "nostr",
                "publish",
                "--plan",
                NOSTR_PLAN,
                "--bundle",
                NOSTR_BUNDLE,
            ],
        ),
        shape(
            "standard-site.plan",
            "publish",
            "intended",
            "native Standard.site deterministic plan artifact chain",
            N,
            &["standard-site", "plan", "--profile", PROFILE],
        ),
        shape(
            "standard-site.records",
            "publish",
            "intended",
            "native Standard.site canonical records artifact chain",
            N,
            &["standard-site", "records", "--profile", PROFILE],
        ),
        shape(
            "standard-site.verify",
            "publish",
            "intended",
            "native Standard.site offline verification surfaces",
            Proof,
            &[
                "standard-site",
                "verify",
                "--profile",
                PROFILE,
                "--dist",
                SITE,
            ],
        ),
        shape(
            "standard-site.publish",
            "publish",
            "intended",
            "native Standard.site committed-plan publication",
            Run,
            &[
                "standard-site",
                "publish",
                "--profile",
                PROFILE,
                "--plan",
                STANDARD_SITE_PLAN,
            ],
        ),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct EnginePin {
    version: String,
    vcs_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Observed {
    exit_code: i32,
    exit_class: String,
    error_line: Option<String>,
    stdout_excerpt: String,
    stderr_excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ContractShape {
    id: String,
    family: String,
    status: String,
    origin: String,
    prepare: String,
    argv: Vec<String>,
    observed: Observed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Contract {
    format: String,
    schema_version: u32,
    engine: EnginePin,
    workspace: String,
    canonical_values: BTreeMap<String, String>,
    notes: Vec<String>,
    shapes: Vec<ContractShape>,
}

fn contract_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/boris-cli-contract.json")
}

fn contract() -> Contract {
    let c: Contract = serde_json::from_str(CONTRACT_JSON).expect("boris CLI contract JSON parses");
    assert_eq!(c.format, "cv-boris-cli-contract");
    assert_eq!(c.schema_version, 1);
    c
}

fn contract_shape<'a>(c: &'a Contract, id: &str) -> &'a ContractShape {
    c.shapes
        .iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("contract shape {id} missing"))
}

fn assert_catalog_matches_contract(c: &Contract) {
    let catalog = catalog();
    let mut ids = std::collections::BTreeSet::new();
    for spec in &catalog {
        assert!(ids.insert(spec.id), "duplicate catalog id {}", spec.id);
    }
    assert_eq!(
        c.shapes.len(),
        catalog.len(),
        "checked-in contract and probe catalog differ in shape count"
    );
    for spec in catalog {
        let s = contract_shape(c, spec.id);
        assert_eq!(s.family, spec.family, "{} family", spec.id);
        assert_eq!(s.status, spec.status, "{} status", spec.id);
        assert_eq!(s.origin, spec.origin, "{} origin", spec.id);
        assert_eq!(s.prepare, spec.prepare.as_str(), "{} prepare", spec.id);
        let argv: Vec<String> = spec.argv.iter().map(|a| a.to_string()).collect();
        assert_eq!(s.argv, argv, "{} argv", spec.id);
    }
}

fn assert_shape(c: &Contract, id: &str, actual: Vec<String>) {
    assert_eq!(
        contract_shape(c, id).argv,
        actual,
        "{id} drifted from the checked-in Boris CLI contract"
    );
}

fn os_strings(args: Vec<OsString>) -> Vec<String> {
    args.into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn catalog_is_the_checked_in_contract_shape_set() {
    assert_catalog_matches_contract(&contract());
}

#[test]
fn engine_emitted_shapes_match_checked_in_contract() {
    let c = contract();
    let argv = |v: &Verb| cv_engine::verbs::argv(v).unwrap();
    for (id, verb) in [
        ("engine.build", Verb::Build),
        ("engine.validate", Verb::Validate),
        ("engine.check", Verb::Check),
        ("engine.plan.profile", Verb::Plan),
        ("engine.graph", Verb::Graph),
        ("engine.impact.index", Verb::Impact { id: "index".into() }),
        (
            "engine.export.out",
            Verb::Export {
                kind: ExportKind::Out,
            },
        ),
        (
            "engine.export.rag",
            Verb::Export {
                kind: ExportKind::Rag,
            },
        ),
        (
            "engine.export.context",
            Verb::Export {
                kind: ExportKind::Context,
            },
        ),
        (
            "engine.export.llms",
            Verb::Export {
                kind: ExportKind::Llms,
            },
        ),
        (
            "engine.export.rss",
            Verb::Export {
                kind: ExportKind::Rss,
            },
        ),
        (
            "engine.export.sitemap",
            Verb::Export {
                kind: ExportKind::Sitemap,
            },
        ),
        (
            "engine.init.docs",
            Verb::Init {
                dir: "site".into(),
                archetype: "docs".into(),
            },
        ),
    ] {
        assert_shape(&c, id, argv(&verb));
    }

    for (id, input) in [
        ("engine.ir.live", None),
        ("engine.ir.live-input", Some("content".to_string())),
    ] {
        let verb = Verb::Ir {
            out: cv_engine::verbs::IR_DIR.into(),
            input,
        };
        assert_shape(&c, id, argv(&verb));
        let staged = cv_engine::ir::staged_verb(&verb).unwrap().unwrap();
        let staged_id = if id == "engine.ir.live" {
            "engine.ir.staging"
        } else {
            "engine.ir.staging-input"
        };
        assert_shape(&c, staged_id, argv(&staged));
    }

    assert_shape(
        &c,
        "engine.probe.validate-input",
        cv_engine::probe::validate_input_args("content"),
    );

    let plan = Path::new(NOSTR_PLAN);
    assert_shape(
        &c,
        "engine.secrets.nostr-sign",
        os_strings(cv_engine::secrets::nostr_sign_args(plan, None)),
    );
    assert_shape(
        &c,
        "engine.secrets.nostr-sign-out",
        os_strings(cv_engine::secrets::nostr_sign_args(
            plan,
            Some(Path::new(NOSTR_BUNDLE)),
        )),
    );

    assert!(
        cv_engine::verbs::argv(&Verb::Publish {
            target: "nostr-nip23".into(),
        })
        .is_err(),
        "Verb::Publish remains unsupported; publish shapes are Custom verbs owned by cv-publish"
    );
}

struct DummyWatch(SessionId);

impl WatchHandle for DummyWatch {
    fn session(&self) -> &SessionId {
        &self.0
    }
    fn stop(&mut self) {}
}

#[derive(Default)]
struct RecordingEngine {
    verbs: Mutex<Vec<Verb>>,
}

impl Engine for RecordingEngine {
    fn locate(&self) -> cv_core::Result<EngineInfo> {
        Err(cv_core::Error::Engine("unused".into()))
    }
    fn capabilities(&self) -> EngineCaps {
        EngineCaps::default()
    }
    fn snapshot(&self) -> cv_core::Result<ArtifactSet> {
        Err(cv_core::Error::Engine("unused".into()))
    }
    fn watch(&self, _: EventSink) -> cv_core::Result<Box<dyn WatchHandle>> {
        Ok(Box::new(DummyWatch(SessionId("watch".into()))))
    }
    fn run(&self, cmd: &Verb, _: EventSink) -> cv_core::Result<ExitClass> {
        self.verbs.lock().unwrap().push(cmd.clone());
        Ok(ExitClass::Ok)
    }
    fn run_caused(
        &self,
        cmd: &Verb,
        sink: EventSink,
        _: Option<&cv_core::events::EventId>,
    ) -> cv_core::Result<ExitClass> {
        self.run(cmd, sink)
    }
    fn write_content(
        &self,
        _: &PageDraft,
        _: Option<Fingerprint>,
    ) -> cv_core::Result<WriteOutcome> {
        Err(cv_core::Error::Engine("unused".into()))
    }
}

#[test]
fn scheduler_redirect_shape_matches_checked_in_contract() {
    let engine = Arc::new(RecordingEngine::default());
    let scheduler =
        EngineScheduler::new(engine.clone(), SeatPolicy::SeparateOutDir(".cv/out".into()));
    scheduler.start_watch(Box::new(|_| {})).unwrap();
    scheduler.run(&Verb::Build, Box::new(|_| {})).unwrap();
    let verb = engine.verbs.lock().unwrap().last().cloned().unwrap();
    assert_shape(
        &contract(),
        "engine.scheduler.separate-out-dir",
        cv_engine::verbs::argv(&verb).unwrap(),
    );
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dst = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &dst);
        } else {
            std::fs::copy(entry.path(), dst).unwrap();
        }
    }
}

fn happy_workspace() -> tempfile::TempDir {
    let t = tempfile::Builder::new()
        .prefix("cv-boris-contract-")
        .tempdir()
        .unwrap();
    copy_tree(
        &cv_fixtures::fixtures_root().join("corpus-src/happy"),
        t.path(),
    );
    t
}

fn exit_code(outcome: cv_engine::exit::ExitOutcome) -> i32 {
    match outcome {
        cv_engine::exit::ExitOutcome::Exited { code } => code,
        other => panic!("boris did not exit normally: {other:?}"),
    }
}

fn invoke(bin: &Path, cwd: &Path, argv: &[String]) -> (i32, String, String) {
    let out = cv_engine::verbs::run_argv(bin, cwd, argv, &cv_engine::proc::CancelToken::new())
        .unwrap_or_else(|e| panic!("spawn boris {argv:?}: {e}"));
    (exit_code(out.outcome), out.stdout, out.stderr)
}

fn prepare_workspace(bin: &Path, root: &Path, prepare: Prepare) {
    match prepare {
        Prepare::None => {}
        Prepare::PublishRun => {
            std::fs::create_dir_all(root.join(SITE)).unwrap();
        }
        Prepare::ProofSite => {
            std::fs::create_dir_all(root.join(SITE)).unwrap();
            let build: Vec<String> = ["build", "--html-dir", SITE]
                .into_iter()
                .map(String::from)
                .collect();
            let (code, _, stderr) = invoke(bin, root, &build);
            assert_eq!(code, 0, "proof-site setup build failed: {stderr}");
        }
    }
}

fn normalize(text: &str, root: &Path) -> String {
    let mut out = text.to_string();
    if let Ok(canonical) = std::fs::canonicalize(root) {
        out = out.replace(canonical.to_string_lossy().as_ref(), "<workspace>");
    }
    out.replace(root.to_string_lossy().as_ref(), "<workspace>")
}

fn excerpt(text: &str) -> String {
    let joined = text.lines().take(6).collect::<Vec<_>>().join("\n");
    joined.chars().take(1200).collect()
}

fn first_error(stderr: &str, code: i32) -> Option<String> {
    if code == 0 {
        return None;
    }
    stderr
        .lines()
        .find(|l| l.starts_with("error:"))
        .or_else(|| stderr.lines().next())
        .map(str::to_string)
}

fn exit_class(code: i32) -> &'static str {
    match code {
        0 => "ok",
        1 => "content-failure",
        2 => "usage-error",
        _ => "io-failure",
    }
}

fn observe(bin: &Path, spec: &ShapeSpec) -> Observed {
    let ws = happy_workspace();
    prepare_workspace(bin, ws.path(), spec.prepare);
    let argv: Vec<String> = spec.argv.iter().map(|a| a.to_string()).collect();
    let (code, stdout, stderr) = invoke(bin, ws.path(), &argv);
    let stdout = normalize(&stdout, ws.path());
    let stderr = normalize(&stderr, ws.path());
    Observed {
        exit_code: code,
        exit_class: exit_class(code).into(),
        error_line: first_error(&stderr, code),
        stdout_excerpt: excerpt(&stdout),
        stderr_excerpt: excerpt(&stderr),
    }
}

fn generated_contract(version: String, vcs_revision: String, bin: &Path) -> Contract {
    let mut canonical_values = BTreeMap::new();
    canonical_values.insert("profile".into(), PROFILE.into());
    canonical_values.insert("run".into(), "run-1".into());
    canonical_values.insert("run_dir".into(), RUN_DIR.into());
    canonical_values.insert("site".into(), SITE.into());
    canonical_values.insert("nostr_plan".into(), NOSTR_PLAN.into());
    canonical_values.insert("nostr_bundle".into(), NOSTR_BUNDLE.into());
    canonical_values.insert("standard_site_plan".into(), STANDARD_SITE_PLAN.into());
    Contract {
        format: "cv-boris-cli-contract".into(),
        schema_version: 1,
        engine: EnginePin {
            version,
            vcs_revision,
        },
        workspace: "fixtures/corpus-src/happy, copied fresh per shape".into(),
        canonical_values,
        notes: vec![
            "argv entries are canonical representatives: run ids and output paths are dynamic in production, while the flags and their order are the contract.".into(),
            "status=emitted means a static test links the shape to an in-product constructor; intended means a documented target-native or profile seam not yet wired end-to-end; reference means a former or exploratory shape kept as negative/positive evidence.".into(),
            "The happy fixture has no Standard.site or enabled Nostr publication metadata. Native publication-family rows therefore prove parser/artifact-boundary behavior with an invalid or missing artifact, never live network delivery.".into(),
            "observed stdout/stderr excerpts are generated evidence. The black-box gate compares exit_code and error_line; excerpts explain the classification without pinning progress output.".into(),
        ],
        shapes: catalog()
            .iter()
            .map(|spec| ContractShape {
                id: spec.id.into(),
                family: spec.family.into(),
                status: spec.status.into(),
                origin: spec.origin.into(),
                prepare: spec.prepare.as_str().into(),
                argv: spec.argv.iter().map(|a| a.to_string()).collect(),
                observed: observe(bin, spec),
            })
            .collect(),
    }
}

fn opt_in_binary() -> Option<PathBuf> {
    if std::env::var_os("CV_BORIS_CONTRACT").is_none_or(|v| v != "1") {
        return None;
    }
    Some(PathBuf::from(
        std::env::var_os(cv_engine::locate::ENV_OVERRIDE)
            .expect("CV_BORIS_CONTRACT=1 also needs CV_BORIS_BIN=/path/to/real/pinned/boris"),
    ))
}

#[test]
#[ignore = "needs CV_BORIS_CONTRACT=1 and CV_BORIS_BIN=/path/to/real/pinned/boris"]
fn black_box_boris_cli_contract() {
    let Some(bin) = opt_in_binary() else {
        eprintln!("skipped: set CV_BORIS_CONTRACT=1 and CV_BORIS_BIN=/path/to/real/pinned/boris");
        return;
    };
    let gate_root = tempfile::Builder::new()
        .prefix("cv-boris-contract-gate-")
        .tempdir()
        .unwrap();
    let identity = cv_engine::locate::gate(
        cv_core::engine::EngineSource::EnvOverride,
        &bin,
        gate_root.path(),
        cv_engine::locate::PINNED,
    )
    .expect("CV_BORIS_BIN must be the pinned real boris/0.9.0 (vcs 66efcfb)");

    if std::env::var_os("CV_BORIS_CONTRACT_WRITE").is_some_and(|v| v == "1") {
        let generated = generated_contract(identity.version, identity.vcs_revision, &bin);
        let path = contract_path();
        let tmp = path.with_extension("json.tmp");
        let mut bytes = serde_json::to_string_pretty(&generated).unwrap();
        bytes.push('\n');
        std::fs::write(&tmp, bytes).unwrap();
        std::fs::rename(&tmp, &path).unwrap();
        eprintln!("wrote {}", path.display());
        return;
    }

    let expected = contract();
    assert_eq!(expected.engine.version, identity.version);
    assert_eq!(expected.engine.vcs_revision, identity.vcs_revision);
    assert_catalog_matches_contract(&expected);

    let mut failures = Vec::new();
    for spec in catalog() {
        let actual = observe(&bin, &spec);
        let wanted = &contract_shape(&expected, spec.id).observed;
        let status = if actual.exit_code == wanted.exit_code
            && actual.error_line == wanted.error_line
        {
            "ok"
        } else {
            failures.push(format!(
                "{}: argv {:?}\n  expected exit {} error {:?}\n  actual exit {} error {:?}\n  actual stderr:\n{}",
                spec.id,
                spec.argv,
                wanted.exit_code,
                wanted.error_line,
                actual.exit_code,
                actual.error_line,
                actual.stderr_excerpt,
            ));
            "DRIFT"
        };
        eprintln!(
            "{status} {:42} exit={} {}",
            spec.id,
            actual.exit_code,
            actual.error_line.as_deref().unwrap_or("")
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
