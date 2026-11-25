//! Test doubles shared by the cv-publish integration tests.

#![allow(dead_code)]

use cv_core::engine::{
    Engine, EngineCaps, EngineInfo, EventSink, ExitClass, Fingerprint, PageDraft, Verb,
    WatchHandle, WriteOutcome,
};
use cv_core::id::StableId;
use cv_core::rules::ArtifactSet;
use cv_core::sys::{FixedClock, Hash, SeqIdGen, Timestamp};
use cv_publish::adapters::{
    COPY_TO_FOLDER, CopyToFolder, Delivered, Delivery, DeliveryAdapter, TransportFailure,
};
use cv_publish::plan::SealedPlan;
use cv_publish::preflight::PreflightArtifacts;
use cv_publish::{Observation, PreflightOutcome, Ruleset, RulesetVerdict, Target, TargetConfig};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Fake engine: "builds" plain site files into the `--html-dir` it is
/// handed and records every verb. No Boris JSON is authored here.
pub struct FakeEngine {
    pub root: PathBuf,
    pub version: Mutex<String>,
    pub check_exit: ExitClass,
    pub pages: Vec<(&'static str, &'static str)>,
    /// When set, the HTML build plants a proof inventory derived from the
    /// pinned happy capture with only its target field tampered to this value.
    pub proof_target: Option<String>,
    pub omit_proof: bool,
    pub malformed_proof: bool,
    pub mutate_source_on_ir: bool,
    pub symlink_output: bool,
    pub verbs: Mutex<Vec<Verb>>,
}

impl FakeEngine {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.into(),
            version: Mutex::new("boris/0.9.0".into()),
            check_exit: ExitClass::Ok,
            pages: vec![
                ("index.html", "<h1>home</h1>"),
                ("guides/a.html", "<h1>a</h1>"),
                ("sitemap.xml", "<urlset/>"),
                ("feed.xml", "<rss/>"),
                ("llms.txt", "# site"),
                ("context/index.md", "ctx"),
            ],
            proof_target: None,
            omit_proof: false,
            malformed_proof: false,
            mutate_source_on_ir: false,
            symlink_output: false,
            verbs: Mutex::new(vec![]),
        }
    }
    pub fn verbs(&self) -> Vec<Verb> {
        self.verbs.lock().unwrap().clone()
    }
    pub fn publishes(&self) -> usize {
        self.verbs()
            .iter()
            .filter(|v| {
                matches!(v, Verb::Publish { .. })
                    || matches!(v, Verb::Custom { name, .. } if name == "publish")
            })
            .count()
    }
}

pub fn arg<'a>(args: &'a [String], flag: &str) -> &'a str {
    let i = args.iter().position(|a| a == flag).unwrap();
    &args[i + 1]
}

impl Engine for FakeEngine {
    fn locate(&self) -> cv_core::Result<EngineInfo> {
        Ok(EngineInfo {
            version: self.version.lock().unwrap().clone(),
            vcs_revision: None,
            caps: self.capabilities(),
        })
    }
    fn capabilities(&self) -> EngineCaps {
        EngineCaps {
            watch: false,
            serve: false,
            authoring: false,
            writes: false,
        }
    }
    fn snapshot(&self) -> cv_core::Result<ArtifactSet> {
        Err(cv_core::Error::Engine("unused".into()))
    }
    fn watch(&self, _: EventSink) -> cv_core::Result<Box<dyn WatchHandle>> {
        Err(cv_core::Error::Engine("unused".into()))
    }
    fn run(&self, cmd: &Verb, _: EventSink) -> cv_core::Result<ExitClass> {
        self.verbs.lock().unwrap().push(cmd.clone());
        match cmd {
            Verb::Custom { name, args }
                if name == "build" && args.iter().any(|a| a == "--html-dir") =>
            {
                let site = self.root.join(arg(args, "--html-dir"));
                #[cfg(unix)]
                if self.symlink_output {
                    std::os::unix::fs::symlink(
                        self.root.join("content/index.md"),
                        site.join("linked.md"),
                    )
                    .unwrap();
                }
                for (p, body) in &self.pages {
                    let f = site.join(p);
                    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
                    std::fs::write(f, body).unwrap();
                }
                let proof = site.join("_boris/proof");
                if !self.omit_proof {
                    std::fs::create_dir_all(&proof).unwrap();
                }
                if self.malformed_proof {
                    std::fs::write(proof.join("artifacts.json"), b"not json").unwrap();
                } else if self.omit_proof {
                } else if let Some(target) = &self.proof_target {
                    let captured = String::from_utf8(
                        cv_fixtures::corpus("happy").file("dist/dist/_boris/proof/artifacts.json"),
                    )
                    .unwrap();
                    let tampered = captured.replace(
                        "\"target\": \"default\"",
                        &format!("\"target\": \"{target}\""),
                    );
                    std::fs::write(proof.join("artifacts.json"), tampered).unwrap();
                } else {
                    let artifacts: Vec<_> = self
                        .pages
                        .iter()
                        .map(|(path, body)| {
                            serde_json::json!({
                                "path": path,
                                "kind": if path.ends_with(".html") { "html-page" } else { "static-file" },
                                "producer": if path.ends_with(".html") { "html-render" } else { "static-files" },
                                "required": true,
                                "status": "committed",
                                "bytes": body.len(),
                                "sha256": format!("{:x}", Sha256::digest(body.as_bytes())),
                                "format_version": null,
                            })
                        })
                        .collect();
                    let mut artifacts = artifacts;
                    artifacts.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
                    let index = serde_json::json!({
                        "format": "boris-publication-artifacts",
                        "schema_version": 1,
                        "target": "default",
                        "artifacts": artifacts,
                    });
                    let bytes = serde_json::to_vec_pretty(&index).unwrap();
                    let digest = format!("{:x}", Sha256::digest(&bytes));
                    std::fs::write(proof.join("artifacts.json"), &bytes).unwrap();
                    let checks = serde_json::json!({
                        "format": "boris-publication-checks",
                        "schema_version": 1,
                        "target": "default",
                        "artifact_inventory": {
                            "path": "_boris/proof/artifacts.json",
                            "bytes": bytes.len(),
                            "sha256": digest,
                            "format": "boris-publication-artifacts",
                            "schema_version": 1,
                            "target": "default",
                        },
                        "checks": [],
                        "findings": [],
                    });
                    std::fs::write(
                        proof.join("checks.json"),
                        serde_json::to_vec_pretty(&checks).unwrap(),
                    )
                    .unwrap();
                }
                Ok(ExitClass::Ok)
            }
            Verb::Custom { name, args } if name == "build" && args.iter().any(|a| a == "--out") => {
                let dir = self.root.join(arg(args, "--out"));
                std::fs::create_dir_all(&dir).unwrap();
                let mut nodes: Vec<_> = self
                    .pages
                    .iter()
                    .filter(|(path, _)| path.ends_with(".html"))
                    .map(|(path, _)| {
                        let id = path.strip_suffix(".html").unwrap();
                        serde_json::json!({
                            "id": id,
                            "sourcePath": format!("{id}.md"),
                            "role": if id == "index" { "trunk" } else { "satellite" },
                        })
                    })
                    .collect();
                if self.mutate_source_on_ir {
                    std::fs::write(self.root.join("content/injected.md"), "# injected\n").unwrap();
                    nodes.push(serde_json::json!({
                        "id": "injected",
                        "sourcePath": "injected.md",
                        "role": "satellite",
                    }));
                }
                std::fs::write(
                    dir.join("graph.json"),
                    serde_json::to_vec_pretty(&serde_json::json!({
                        "schemaVersion": 1,
                        "nodes": nodes,
                        "edges": [],
                    }))
                    .unwrap(),
                )
                .unwrap();
                Ok(ExitClass::Ok)
            }
            Verb::Custom { name, .. } if name == "check" => Ok(self.check_exit),
            _ => Ok(ExitClass::Ok),
        }
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

/// Engine backed by the pinned real Boris happy capture. Tests can damage
/// that captured output explicitly, but no proof JSON is hand-authored here.
/// Post-copy mutation hook for one-off proof-evidence regressions.
pub type PostBuildHook = Box<dyn Fn(&Path) + Send>;

pub struct CapturedEngine {
    pub root: PathBuf,
    pub fixture: &'static str,
    pub omit_proof: bool,
    pub malformed_proof: bool,
    pub proof_payload_entry: bool,
    pub duplicate_entry: bool,
    pub case_collision_entry: bool,
    pub failed_check: bool,
    pub uppercase_sha: bool,
    pub plant_uninventoried: bool,
    pub omit_css_record: bool,
    pub cache_record: bool,
    pub hardlink_alias: bool,
    pub multiply_linked: bool,
    /// Post-copy mutation hook for one-off proof-evidence regressions.
    pub post_build: Option<PostBuildHook>,
    /// Exit class the double reports for `proof verify` — Boris's own
    /// deploy verdict. Tests exercising a blocking verdict set this to a
    /// non-Ok class; the knob simulates what real Boris would exit over the
    /// copied capture, it is not hand-authored proof JSON.
    pub verify_exit: ExitClass,
}

impl CapturedEngine {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.into(),
            fixture: "captures/happy/dist/dist",
            omit_proof: false,
            malformed_proof: false,
            proof_payload_entry: false,
            duplicate_entry: false,
            case_collision_entry: false,
            failed_check: false,
            uppercase_sha: false,
            plant_uninventoried: false,
            omit_css_record: false,
            cache_record: false,
            hardlink_alias: false,
            multiply_linked: false,
            post_build: None,
            verify_exit: ExitClass::Ok,
        }
    }

    /// The real Boris capture whose output commits an `.html` content asset
    /// (`kind: content-asset`) alongside the three html pages.
    pub fn with_html_content_asset(root: &Path) -> Self {
        Self {
            fixture: "captures/happy-assets/dist/dist",
            ..Self::new(root)
        }
    }
}

impl Engine for CapturedEngine {
    fn locate(&self) -> cv_core::Result<EngineInfo> {
        Ok(EngineInfo {
            version: "boris/0.9.0".into(),
            vcs_revision: Some("66efcfb".into()),
            caps: self.capabilities(),
        })
    }
    fn capabilities(&self) -> EngineCaps {
        EngineCaps {
            watch: false,
            serve: false,
            authoring: false,
            writes: false,
        }
    }
    fn snapshot(&self) -> cv_core::Result<ArtifactSet> {
        Err(cv_core::Error::Engine("unused".into()))
    }
    fn watch(&self, _: EventSink) -> cv_core::Result<Box<dyn WatchHandle>> {
        Err(cv_core::Error::Engine("unused".into()))
    }
    fn run(&self, cmd: &Verb, _: EventSink) -> cv_core::Result<ExitClass> {
        let Verb::Custom { name, args } = cmd else {
            return Ok(ExitClass::Ok);
        };
        match name.as_str() {
            "build" if args.iter().any(|a| a == "--html-dir") => {
                let out = self.root.join(arg(args, "--html-dir"));
                copy_tree(&cv_fixtures::fixtures_root().join(self.fixture), &out);
                if self.omit_proof {
                    let _ = std::fs::remove_file(out.join("_boris/proof/artifacts.json"));
                }
                if self.malformed_proof {
                    std::fs::write(out.join("_boris/proof/artifacts.json"), b"not json").unwrap();
                }
                if self.duplicate_entry {
                    let path = out.join("_boris/proof/artifacts.json");
                    let mut idx: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    let entry = idx["artifacts"][0].clone();
                    idx["artifacts"].as_array_mut().unwrap().push(entry);
                    let mut sorted = idx["artifacts"].as_array().unwrap().clone();
                    sorted.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
                    idx["artifacts"] = serde_json::json!(sorted);
                    std::fs::write(&path, serde_json::to_vec_pretty(&idx).unwrap()).unwrap();
                }
                if self.case_collision_entry {
                    let path = out.join("_boris/proof/artifacts.json");
                    let mut idx: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    let mut entry = idx["artifacts"][0].clone();
                    entry["path"] = serde_json::json!(
                        idx["artifacts"][0]["path"].as_str().unwrap().to_uppercase()
                    );
                    idx["artifacts"].as_array_mut().unwrap().push(entry);
                    let mut sorted = idx["artifacts"].as_array().unwrap().clone();
                    sorted.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
                    idx["artifacts"] = serde_json::json!(sorted);
                    std::fs::write(&path, serde_json::to_vec_pretty(&idx).unwrap()).unwrap();
                }
                if self.uppercase_sha {
                    let path = out.join("_boris/proof/artifacts.json");
                    let raw = std::fs::read_to_string(&path).unwrap();
                    let mut idx: serde_json::Value = serde_json::from_str(&raw).unwrap();
                    let first = idx["artifacts"][0]["sha256"]
                        .as_str()
                        .unwrap()
                        .to_uppercase();
                    idx["artifacts"][0]["sha256"] = serde_json::json!(first);
                    std::fs::write(&path, serde_json::to_vec_pretty(&idx).unwrap()).unwrap();
                }
                if self.failed_check {
                    let path = out.join("_boris/proof/checks.json");
                    let mut idx: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    for c in idx["checks"].as_array_mut().unwrap() {
                        if c["id"].as_str() == Some("rendered-html") {
                            c["status"] = serde_json::json!("failed");
                            c["counts"]["findings"] = serde_json::json!(1);
                        }
                    }
                    idx["findings"].as_array_mut().unwrap().push(serde_json::json!({
                        "code": "HTML_FRAGMENT_MISSING",
                        "severity": "error",
                        "confidence": "certain",
                        "subject": { "kind": "page", "id": "guides/publishing", "target": "default" },
                        "output_location": { "path": "guides/publishing.html", "line": 29, "column": 40 },
                    }));
                    std::fs::write(&path, serde_json::to_vec_pretty(&idx).unwrap()).unwrap();
                }
                if self.plant_uninventoried {
                    std::fs::write(out.join("not-in-the-inventory.txt"), b"planted").unwrap();
                }
                if self.omit_css_record {
                    let path = out.join("_boris/proof/artifacts.json");
                    let mut idx: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    idx["artifacts"]
                        .as_array_mut()
                        .unwrap()
                        .retain(|a| a["path"].as_str() != Some("assets/css/boris.css"));
                    std::fs::write(&path, serde_json::to_vec_pretty(&idx).unwrap()).unwrap();
                }
                if self.cache_record {
                    let path = out.join("_boris/proof/artifacts.json");
                    let mut idx: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    let mut entry = idx["artifacts"][0].clone();
                    entry["path"] = serde_json::json!(".boris-cache/sitemap-output-path");
                    idx["artifacts"].as_array_mut().unwrap().push(entry);
                    let mut sorted = idx["artifacts"].as_array().unwrap().clone();
                    sorted.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
                    idx["artifacts"] = serde_json::json!(sorted);
                    std::fs::write(&path, serde_json::to_vec_pretty(&idx).unwrap()).unwrap();
                }
                #[cfg(unix)]
                if self.hardlink_alias {
                    let proof_file = out.join("_boris/proof/index.html");
                    if !proof_file.exists() {
                        std::fs::write(&proof_file, b"proof").unwrap();
                    }
                    std::fs::hard_link(&proof_file, out.join("alias.html")).unwrap();
                    let bytes = std::fs::read(&proof_file).unwrap();
                    let path = out.join("_boris/proof/artifacts.json");
                    let mut idx: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    let mut entry = idx["artifacts"][0].clone();
                    entry["path"] = serde_json::json!("alias.html");
                    entry["bytes"] = serde_json::json!(bytes.len());
                    entry["sha256"] = serde_json::json!(cv_publish::plan::sha256(&bytes).to_hex());
                    idx["artifacts"].as_array_mut().unwrap().push(entry);
                    let mut sorted = idx["artifacts"].as_array().unwrap().clone();
                    sorted.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
                    idx["artifacts"] = serde_json::json!(sorted);
                    std::fs::write(&path, serde_json::to_vec_pretty(&idx).unwrap()).unwrap();
                }
                #[cfg(unix)]
                if self.multiply_linked {
                    std::fs::hard_link(
                        out.join("index.html"),
                        self.root.join("outside-the-tree-copy.html"),
                    )
                    .unwrap();
                }
                if self.proof_payload_entry {
                    let path = out.join("_boris/proof/artifacts.json");
                    let mut idx: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    let mut entry = idx["artifacts"][0].clone();
                    entry["path"] = serde_json::json!("_boris/proof/index.html");
                    idx["artifacts"].as_array_mut().unwrap().push(entry);
                    let mut sorted = idx["artifacts"].as_array().unwrap().clone();
                    sorted.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
                    idx["artifacts"] = serde_json::json!(sorted);
                    std::fs::write(&path, serde_json::to_vec_pretty(&idx).unwrap()).unwrap();
                }
                if let Some(hook) = &self.post_build {
                    hook(&out);
                }
                Ok(ExitClass::Ok)
            }
            "build" if args.iter().any(|a| a == "--out") => {
                let dir = self.root.join(arg(args, "--out"));
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(
                    dir.join("graph.json"),
                    cv_fixtures::ir_capture("happy").file("graph.json"),
                )
                .unwrap();
                Ok(ExitClass::Ok)
            }
            "proof" => Ok(self.verify_exit),
            "proof" => Ok(self.verify_exit),
            _ => Ok(ExitClass::Ok),
        }
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

pub struct Rules(pub Cell<u8>, pub Vec<String>);

impl Rules {
    pub fn ok() -> Self {
        Self(Cell::new(1), vec![])
    }
}

impl Ruleset for Rules {
    fn hash(&self) -> Hash {
        Hash([self.0.get(); 32])
    }
    fn evaluate(&self, _: &PreflightArtifacts<'_>) -> RulesetVerdict {
        RulesetVerdict {
            blocking: self.1.clone(),
            advisory: vec!["heuristic: meta description short".into()],
        }
    }
}

/// Counts deliveries; can panic mid-delivery after touching the target.
pub struct Flaky {
    pub inner: CopyToFolder,
    pub calls: Cell<usize>,
    pub crash_after: Option<usize>,
    pub fail: Option<TransportFailure>,
}

impl DeliveryAdapter for Flaky {
    fn id(&self) -> &str {
        COPY_TO_FOLDER
    }
    fn deliver(&self, d: &Delivery<'_>) -> Result<Delivered, TransportFailure> {
        self.calls.set(self.calls.get() + 1);
        if let Some(f) = &self.fail {
            return Err(f.clone());
        }
        if let Some(n) = self.crash_after {
            let partial: Vec<_> = d.files.iter().take(n).cloned().collect();
            let _ = self.inner.deliver(&Delivery {
                files: &partial,
                ..*d
            });
            panic!("simulated crash after contact");
        }
        self.inner.deliver(d)
    }
    fn can_read_back(&self) -> bool {
        true
    }
    fn read_back(&self, cfg: &TargetConfig, path: &str) -> Option<Vec<u8>> {
        self.inner.read_back(cfg, path)
    }
    fn probe(&self, plan: &SealedPlan, cfg: &TargetConfig) -> Option<Observation> {
        self.inner.probe(plan, cfg)
    }
}

pub fn flaky(root: &Path) -> Flaky {
    Flaky {
        inner: CopyToFolder::new(root),
        calls: Cell::new(0),
        crash_after: None,
        fail: None,
    }
}

pub fn cfg() -> TargetConfig {
    TargetConfig::default_html(
        Target::StaticDist,
        "https://example.test/",
        COPY_TO_FOLDER,
        "deployed",
    )
}

pub fn granted(o: PreflightOutcome) -> StableId {
    match o {
        PreflightOutcome::Granted { run, .. } => run,
        PreflightOutcome::Blocked { blocks, .. } => panic!("blocked: {blocks:?}"),
    }
}

pub const CLOCK: FixedClock = FixedClock(Timestamp(1_700_000_000_000));

pub fn ids(prefix: &str) -> SeqIdGen {
    SeqIdGen::new(prefix)
}
