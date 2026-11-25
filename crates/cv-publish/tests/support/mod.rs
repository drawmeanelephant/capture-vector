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
use cv_publish::plan::{PlanFile, SealedPlan};
use cv_publish::{Observation, PreflightOutcome, Ruleset, RulesetVerdict, Target, TargetConfig};
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
            Verb::Custom { name, args } if name == "build" => {
                let site = self.root.join(arg(args, "--html-dir"));
                for (p, body) in &self.pages {
                    let f = site.join(p);
                    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
                    std::fs::write(f, body).unwrap();
                }
                Ok(ExitClass::Ok)
            }
            Verb::Custom { name, args } if name == "graph" => {
                std::fs::write(self.root.join(arg(args, "--out")), "graph TD; a-->b").unwrap();
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
    fn evaluate(&self, _: &Path, _: &[PlanFile]) -> RulesetVerdict {
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
    TargetConfig::new(
        Target::StaticDist,
        "boris.json",
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
