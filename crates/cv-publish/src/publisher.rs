//! The publish loop over the ledger. Every decision reads durable
//! journal state; every transition is fsynced before the next step.

use crate::adapters::{COPY_TO_FOLDER, Delivered, Delivery, DeliveryAdapter, copy_destination};
use crate::plan::{PlanFile, SealedPlan, hash_json, inventory, load_sealed, sha256};
use crate::preflight::{
    PreflightArtifacts, PreflightOutcome, PreflightRequest, Ruleset, graph_file, run_dir, site_dir,
    verbs,
};
use crate::secrets::SecretBackend;
use crate::targets::{DEFAULT_HTML_TARGET, TargetConfig};
use crate::{PublishError, Result};
use cv_core::engine::{Engine, ExitClass};
use cv_core::events::publish::PublishEvent;
use cv_core::id::StableId;
use cv_core::publish::{PreflightGrant, RunState, RunStatus};
use cv_core::sys::{Clock, Hash, IdGen};
use cv_ledger::journal::Entry;

use crate::proofeval::{evaluate_proof_checks, proof_finding_detail};
use cv_ledger::nouns::{DeliveryNoun, derive_id};
use cv_ledger::{Journal, RunView, ingest, runs};
use cv_store::paths::{confined, confined_file};
use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// What a human (or a read-only adapter probe) observed at the target
/// while reconciling an `Unknown`/`Submitted` run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    Delivered(RunStatus),
    NotDelivered,
}

/// Source inputs for the frozen digest. Symlinks contribute their target
/// path, never their target's content, and are never recursed through —
/// a symlinked directory cycle must not hang preflight.
enum SourceInput {
    File(PathBuf),
    Link(PathBuf),
}

fn collect_input_files(dir: &Path, out: &mut Vec<SourceInput>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        let path = entry.path();
        if meta.file_type().is_symlink() {
            out.push(SourceInput::Link(path));
        } else if meta.is_dir() {
            collect_input_files(&path, out)?;
        } else if meta.is_file() {
            out.push(SourceInput::File(path));
        }
    }
    Ok(())
}

/// Evaluate Boris's proof artifact inventory against the built output:
/// target identity, path portability and duplication, the proof-payload
/// exclusion, committed status, and byte truth. Returns the deployable
/// payload, the artifact kind of each payload file, and whether the
/// inventory itself was valid enough to trust as a deploy contract.
/// Non-payload namespaces, compared component-wise and case-insensitively:
/// `_boris/proof/` is evidence, `.boris-cache/` is compiler cache. Case
/// folding matters because copy-to-folder targets case-insensitive
/// filesystems (APFS, NTFS) where `_boris/PROOF` *is* the proof directory.
fn is_reserved_namespace(path: &str) -> bool {
    let components: Vec<String> = path.split('/').map(|c| c.to_lowercase()).collect();
    components.first().is_some_and(|c| c == ".boris-cache")
        || (components.first().is_some_and(|c| c == "_boris")
            && components.get(1).is_some_and(|c| c == "proof"))
}

/// Page-ness comes only from Boris's committed artifact kind, never from
/// the file extension. Shared by graph membership, Audit construction, and
/// delivery URL projection.
pub fn is_html_page(kind: Option<&str>) -> bool {
    kind == Some("html-page")
}

/// File identity (dev+inode) and link count of one payload candidate.
/// Hardlink aliases let proof bytes or externally mutable content ship as
/// payload without any lexical tell; identity comparison is the only sound
/// check. Unix-only: Windows delivery is unproven platform behavior.
#[cfg(unix)]
fn file_identity(path: &Path) -> Option<(u64, u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).ok()?;
    Some((m.dev(), m.ino(), m.nlink()))
}

/// Identities of every regular file under the reserved namespaces of one
/// output tree. Payload sharing any of these identities is proof evidence
/// or cache content hardlinked into the payload area.
#[cfg(unix)]
fn reserved_identities(site: &Path) -> std::collections::BTreeSet<(u64, u64)> {
    let mut out = std::collections::BTreeSet::new();
    for reserved in ["_boris/proof", ".boris-cache"] {
        let root = site.join(reserved);
        if !root.is_dir() {
            continue;
        }
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if let Some((dev, ino, _)) = file_identity(&p) {
                        out.insert((dev, ino));
                    }
                }
            }
        }
    }
    out
}

fn evaluate_proof_inventory(
    idx: &ingest::ArtifactIndex,
    output_files: &[PlanFile],
    site: &Path,
    blocks: &mut Vec<String>,
) -> (
    Vec<PlanFile>,
    std::collections::BTreeMap<String, String>,
    bool,
) {
    let mut files = Vec::new();
    let mut kinds = std::collections::BTreeMap::new();
    if idx.target != DEFAULT_HTML_TARGET {
        blocks.push(format!(
            "proof artifacts: target {} is not {DEFAULT_HTML_TARGET}",
            idx.target
        ));
        return (files, kinds, false);
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut casefolded = std::collections::BTreeMap::new();
    for artifact in &idx.artifacts {
        let normalized = artifact
            .path
            .split('/')
            .filter(|seg| !seg.is_empty() && *seg != ".")
            .collect::<Vec<_>>()
            .join("/");
        if normalized != artifact.path || artifact.path.split('/').any(|seg| seg == "..") {
            blocks.push(format!(
                "proof artifacts: {} is not a portable normalized path",
                artifact.path
            ));
            continue;
        }
        if !seen.insert(artifact.path.clone()) {
            blocks.push(format!(
                "proof artifacts: duplicate artifact path {}",
                artifact.path
            ));
            continue;
        }
        if let Some(other) = casefolded.insert(artifact.path.to_lowercase(), artifact.path.clone())
            && other != artifact.path
        {
            blocks.push(format!(
                "proof artifacts: {} case-collides with {} on case-insensitive filesystems",
                artifact.path, other
            ));
            continue;
        }
        if is_reserved_namespace(&artifact.path) {
            blocks.push(format!(
                "proof artifacts: {} is in a reserved namespace, never payload",
                artifact.path
            ));
            continue;
        }
        if artifact.status != "committed" {
            if artifact.required {
                blocks.push(format!(
                    "proof artifacts: {} is {}",
                    artifact.path, artifact.status
                ));
            }
            continue;
        }
        match output_files.iter().find(|f| f.path == artifact.path) {
            Some(f) if f.bytes == artifact.bytes && f.sha256.to_hex() == artifact.sha256 => {
                kinds.insert(artifact.path.clone(), artifact.kind.clone());
                let mut f = f.clone();
                f.kind = Some(artifact.kind.clone());
                f.advertised = artifact.advertised;
                files.push(f)
            }
            Some(_) => blocks.push(format!(
                "proof artifacts: {} does not match the sealed bytes",
                artifact.path
            )),
            None => blocks.push(format!(
                "proof artifacts: {} is missing from the output",
                artifact.path
            )),
        }
    }
    // Completeness: every non-reserved regular output file must carry a
    // committed inventory record. An uninventoried file is not silently
    // dropped payload — the tree no longer matches what Boris committed.
    let committed: std::collections::BTreeSet<&str> =
        files.iter().map(|f| f.path.as_str()).collect();
    for f in output_files {
        if !is_reserved_namespace(&f.path) && !committed.contains(f.path.as_str()) {
            blocks.push(format!(
                "proof artifacts: output file {} has no committed inventory record",
                f.path
            ));
        }
    }
    #[cfg(unix)]
    {
        let reserved = reserved_identities(site);
        for f in &files {
            if let Some((dev, ino, nlink)) = file_identity(&site.join(&f.path)) {
                if reserved.contains(&(dev, ino)) {
                    blocks.push(format!(
                        "proof artifacts: {} is a hardlink alias of reserved-namespace bytes",
                        f.path
                    ));
                } else if nlink > 1 {
                    blocks.push(format!(
                        "proof artifacts: {} is multiply linked and can mutate outside the sealed tree",
                        f.path
                    ));
                }
            }
        }
    }
    files.sort();
    if files.is_empty() {
        blocks.push("proof artifacts: no committed payload artifacts".into());
    }
    (files, kinds, true)
}

fn graph_html_paths(graph: &[u8]) -> Result<std::collections::BTreeSet<String>> {
    let value: serde_json::Value = serde_json::from_slice(graph)
        .map_err(|e| PublishError::Refused(format!("graph: invalid json: {e}")))?;
    let nodes = value
        .get("nodes")
        .and_then(|v| v.as_array())
        .ok_or_else(|| PublishError::Refused("graph: missing nodes".into()))?;
    let mut out = std::collections::BTreeSet::new();
    for node in nodes {
        if node.get("sourcePath").and_then(|v| v.as_str()).is_some() {
            let id = node
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| PublishError::Refused("graph: page node missing id".into()))?;
            out.insert(format!("{id}.html"));
        }
    }
    Ok(out)
}

fn deployable_inventory(site: &Path) -> Result<Vec<PlanFile>> {
    let output = inventory(site)?;
    let idx = ingest::artifact_inventory(site)?
        .ok_or_else(|| PublishError::Refused("proof artifacts: artifacts.json missing".into()))?;
    if idx.target != DEFAULT_HTML_TARGET {
        return Err(PublishError::SealBroken(vec!["artifact_set"]));
    }
    let mut files = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut casefolded = std::collections::BTreeSet::new();
    for artifact in &idx.artifacts {
        if is_reserved_namespace(&artifact.path) || !seen.insert(artifact.path.clone()) {
            return Err(PublishError::SealBroken(vec!["artifact_set"]));
        }
        if !casefolded.insert(artifact.path.to_lowercase()) {
            return Err(PublishError::SealBroken(vec!["artifact_set"]));
        }
        if artifact.status != "committed" {
            continue;
        }
        let Some(f) = output.iter().find(|f| f.path == artifact.path) else {
            return Err(PublishError::SealBroken(vec!["artifact_set"]));
        };
        if f.bytes != artifact.bytes || f.sha256.to_hex() != artifact.sha256 {
            return Err(PublishError::SealBroken(vec!["artifact_set"]));
        }
        let mut f = f.clone();
        f.kind = Some(artifact.kind.clone());
        f.advertised = artifact.advertised;
        files.push(f);
    }
    // Completeness: any non-reserved output file without a committed
    // inventory record means the tree no longer matches what Boris
    // committed — the seal is broken rather than the file silently dropped.
    let committed: std::collections::BTreeSet<&str> =
        files.iter().map(|f| f.path.as_str()).collect();
    for f in &output {
        if !is_reserved_namespace(&f.path) && !committed.contains(f.path.as_str()) {
            return Err(PublishError::SealBroken(vec!["artifact_set"]));
        }
    }
    #[cfg(unix)]
    {
        let reserved = reserved_identities(site);
        for f in &files {
            if let Some((dev, ino, nlink)) = file_identity(&site.join(&f.path))
                && (reserved.contains(&(dev, ino)) || nlink > 1)
            {
                return Err(PublishError::SealBroken(vec!["artifact_set"]));
            }
        }
    }
    files.sort();
    Ok(files)
}

fn artifact_seal_hash(site: &Path, files: &[PlanFile]) -> Result<Hash> {
    let inventory_bytes = std::fs::read(site.join("_boris/proof/artifacts.json"))?;
    hash_json(&(files, sha256(&inventory_bytes)))
}

pub struct Publisher<'a> {
    root: PathBuf,
    journal: Journal,
    engine: &'a dyn Engine,
    clock: &'a dyn Clock,
    secrets: &'a dyn SecretBackend,
    adapters: BTreeMap<String, Box<dyn DeliveryAdapter + 'a>>,
    events: Vec<PublishEvent>,
}

impl<'a> Publisher<'a> {
    /// Open the project ledger and recover: any run whose last durable
    /// state is `Dispatching` was interrupted after the transport may have
    /// been touched → `Unknown`. Never re-dispatched.
    pub fn open(
        root: &Path,
        engine: &'a dyn Engine,
        clock: &'a dyn Clock,
        secrets: &'a dyn SecretBackend,
    ) -> Result<Self> {
        let mut p = Self {
            root: root.to_path_buf(),
            journal: Journal::open(root)?,
            engine,
            clock,
            secrets,
            adapters: BTreeMap::new(),
            events: Vec::new(),
        };
        p.recover()?;
        Ok(p)
    }

    pub fn with_adapter(mut self, a: impl DeliveryAdapter + 'a) -> Self {
        self.adapters.insert(a.id().to_string(), Box::new(a));
        self
    }

    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    pub fn run(&self, run: &StableId) -> Option<RunView> {
        runs(self.journal.records()).remove(run)
    }

    pub fn runs(&self) -> BTreeMap<StableId, RunView> {
        runs(self.journal.records())
    }

    /// Drain publish events emitted since the last call.
    pub fn take_events(&mut self) -> Vec<PublishEvent> {
        std::mem::take(&mut self.events)
    }

    fn append(&mut self, e: Entry) -> Result<()> {
        self.journal.append(self.clock.now(), e)?;
        Ok(())
    }

    fn set_state(&mut self, run: &StableId, state: RunState, note: Option<String>) -> Result<()> {
        self.append(Entry::RunState {
            run: run.clone(),
            state: state.clone(),
            note,
        })?;
        self.events.push(PublishEvent::RunStateChanged {
            run: run.clone(),
            to: state,
        });
        Ok(())
    }

    fn refuse(&mut self, run: &StableId, reason: String) -> PublishError {
        if let Err(e) = self.append(Entry::Refused {
            run: run.clone(),
            reason: reason.clone(),
        }) {
            return e;
        }
        PublishError::Refused(reason)
    }

    fn recover(&mut self) -> Result<Vec<StableId>> {
        let stuck: Vec<_> = self
            .runs()
            .into_iter()
            .filter(|(_, v)| {
                v.state == Some(RunState::Dispatching) || v.delivery_contact_uncertain()
            })
            .map(|(id, _)| id)
            .collect();
        for run in &stuck {
            self.set_state(
                run,
                RunState::Unknown,
                Some("interrupted after dispatch; reconcile before publishing again".into()),
            )?;
        }
        Ok(stuck)
    }

    // ── preflight ────────────────────────────────────────────────────

    /// Build the target profile into the run's own output dir, run
    /// validate/check/ruleset on those bytes, seal a grant (or record
    /// the blocks). Nothing is delivered.
    pub fn preflight(
        &mut self,
        req: &PreflightRequest,
        ruleset: &dyn Ruleset,
        ids: &mut dyn IdGen,
    ) -> Result<PreflightOutcome> {
        let cfg = &req.config;
        validate_destination(&cfg.adapter, &cfg.destination)?;
        let seed = ids.next();
        run_dir(&seed)?;
        let target = cfg.target.id().to_string();
        let publish = confined(&self.root, Path::new(".cv/publish"))?;
        std::fs::create_dir_all(&publish)?;
        // An IdGen can restart when the app restarts, and a crash before the
        // first journal append can leave an orphan run directory behind. Pick
        // the first durable-sequence id unused in both the journal and the
        // filesystem; existing directories are evidence and are never reused.
        let mut nonce = self.journal.records().len() as u64;
        let (run, _run_rel, dir) = loop {
            let candidate = StableId(format!("{}-{}", seed.0, nonce));
            let rel = run_dir(&candidate)?;
            let dir = confined(&self.root, Path::new(&rel))?;
            let occupied = match std::fs::symlink_metadata(&dir) {
                Ok(_) => true,
                Err(e) if e.kind() == ErrorKind::NotFound => false,
                Err(e) => return Err(e.into()),
            };
            if self.run(&candidate).is_none() && !occupied {
                std::fs::create_dir(&dir)?;
                break (candidate, rel, dir);
            }
            nonce = nonce
                .checked_add(1)
                .ok_or_else(|| PublishError::Refused("publish run id sequence exhausted".into()))?;
        };
        let site_rel = site_dir(&run)?;
        std::fs::create_dir(dir.join("site"))?;

        let mut blocks = Vec::new();
        let source_before = self.source_digest()?;
        for (step, verb) in verbs(cfg, &run)? {
            match self.engine.run(&verb, Box::new(|_| {})) {
                Ok(ExitClass::Ok) => {}
                Ok(x) => {
                    let detail = if step == "proof-verify" {
                        proof_finding_detail(&dir.join("site"))
                    } else {
                        String::new()
                    };
                    let verdict = if step == "proof-verify" {
                        " — deploy blocked by Boris proof verify"
                    } else {
                        ""
                    };
                    blocks.push(format!("{step}: {x:?}{verdict}{detail}"));
                }
                Err(e) => blocks.push(format!("{step}: {e}")),
            }
            if self.source_digest()? != source_before {
                blocks.push("source inputs changed during preflight".into());
                break;
            }
            if step == "build" && !blocks.is_empty() {
                break;
            }
        }
        if self.source_digest()? != source_before
            && !blocks
                .iter()
                .any(|b| b == "source inputs changed during preflight")
        {
            blocks.push("source inputs changed during preflight".into());
        }
        let site = confined(&self.root, Path::new(&site_rel))?;
        let output_files = match inventory(&site) {
            Ok(files) => files,
            Err(e) => {
                blocks.push(format!("output inventory: {e}"));
                Vec::new()
            }
        };
        if blocks.is_empty() && output_files.is_empty() {
            blocks.push("build: output dir is empty".into());
        }
        let (files, kinds, proof_inventory_valid) = match ingest::artifact_inventory(&site) {
            Ok(Some(idx)) => evaluate_proof_inventory(&idx, &output_files, &site, &mut blocks),
            Ok(None) => {
                blocks.push("proof artifacts: artifacts.json missing".into());
                Default::default()
            }
            Err(e) => {
                blocks.push(format!("proof artifacts: {e}"));
                Default::default()
            }
        };
        if proof_inventory_valid
            && let Ok(inv_bytes) = std::fs::read(site.join("_boris/proof/artifacts.json"))
        {
            evaluate_proof_checks(&site, &sha256(&inv_bytes).to_hex(), &mut blocks);
        }
        let graph = self.read_graph(&run)?;
        if blocks.is_empty() && graph.is_none() {
            blocks.push("graph: not produced".into());
        }
        if proof_inventory_valid && let Some(graph_bytes) = &graph {
            match graph_html_paths(graph_bytes) {
                Ok(expected) => {
                    let actual: std::collections::BTreeSet<String> = files
                        .iter()
                        .filter(|f| is_html_page(kinds.get(&f.path).map(String::as_str)))
                        .map(|f| f.path.clone())
                        .collect();
                    for path in expected.difference(&actual) {
                        blocks.push(format!("graph: {path} has no sealed HTML artifact"));
                    }
                    for path in actual.difference(&expected) {
                        blocks.push(format!(
                            "graph: sealed html-page artifact {path} has no IR page"
                        ));
                    }
                }
                Err(e) => blocks.push(e.to_string()),
            }
        }
        let mut advisory = Vec::new();
        if blocks.is_empty() {
            let verdict = ruleset.evaluate(&PreflightArtifacts {
                site: &site,
                files: &files,
                kinds: &kinds,
                ir_graph: Some(confined_file(&self.root, Path::new(&graph_file(&run)?))?.as_path()),
                config: cfg,
            });
            blocks.extend(
                verdict
                    .blocking
                    .into_iter()
                    .map(|b| format!("ruleset: {b}")),
            );
            advisory = verdict.advisory;
        }

        let proof = if proof_inventory_valid {
            match ingest::from_output_dir(&run, &site) {
                Ok(p) => p,
                Err(e) => {
                    blocks.push(format!("proof artifacts: {e}"));
                    Default::default()
                }
            }
        } else {
            Default::default()
        };
        for n in proof.artifacts {
            self.append(Entry::Artifact(n))?;
        }
        for n in proof.checks {
            self.append(Entry::Check(n))?;
        }
        for n in proof.claims {
            self.append(Entry::Claim(n))?;
        }
        if !advisory.is_empty() {
            self.append(Entry::Advisory {
                run: run.clone(),
                findings: advisory,
            })?;
        }

        if !blocks.is_empty() {
            self.append(Entry::PreflightBlocked {
                run: run.clone(),
                target,
                blocks: blocks.clone(),
            })?;
            return Ok(PreflightOutcome::Blocked { run, blocks });
        }

        let plan = SealedPlan {
            target: target.clone(),
            adapter: cfg.adapter.clone(),
            destination: cfg.destination.clone(),
            site: site_rel,
            files,
        };
        let grant = PreflightGrant {
            artifact_set_hash: match artifact_seal_hash(&site, &plan.files) {
                Ok(h) => h,
                Err(e) => {
                    let blocks = vec![format!("proof artifacts: unreadable for sealing: {e}")];
                    self.append(Entry::PreflightBlocked {
                        run: run.clone(),
                        target,
                        blocks: blocks.clone(),
                    })?;
                    return Ok(PreflightOutcome::Blocked { run, blocks });
                }
            },
            graph_hash: sha256(graph.as_deref().unwrap_or_default()),
            plan_hash: plan.hash()?,
            target_cfg_hash: hash_json(cfg)?,
            ruleset_hash: ruleset.hash(),
            boris_version: self.engine.locate()?.version,
        };
        self.append(Entry::GrantIssued {
            run: run.clone(),
            target,
            grant: grant.clone(),
            plan: serde_json::to_value(&plan)?,
            target_cfg: serde_json::to_value(cfg)?,
        })?;
        self.events.push(PublishEvent::PreflightIssued {
            grant: Box::new(grant.clone()),
        });
        Ok(PreflightOutcome::Granted { run, grant })
    }

    fn source_digest(&self) -> Result<Hash> {
        let mut inputs = Vec::new();
        for name in ["boris.json", "content", "theme", "themes", "static"] {
            let path = self.root.join(name);
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                inputs.push(SourceInput::Link(path));
            } else if meta.is_file() {
                inputs.push(SourceInput::File(path));
            } else if meta.is_dir() {
                collect_input_files(&path, &mut inputs)?;
            }
        }
        inputs.sort_by(|a, b| {
            let key = |x: &SourceInput| match x {
                SourceInput::File(p) | SourceInput::Link(p) => p.clone(),
            };
            key(a).cmp(&key(b))
        });
        let mut material = Vec::new();
        for input in inputs {
            let (path, bytes) = match &input {
                SourceInput::File(p) => (p, std::fs::read(p)?),
                SourceInput::Link(p) => (
                    p,
                    std::fs::read_link(p)?.to_string_lossy().as_bytes().to_vec(),
                ),
            };
            let rel = path
                .strip_prefix(&self.root)
                .unwrap_or(path)
                .to_string_lossy();
            let rel = rel.as_bytes();
            material.extend_from_slice(&(rel.len() as u64).to_le_bytes());
            material.extend_from_slice(rel);
            material.push(match &input {
                SourceInput::File(_) => 1,
                SourceInput::Link(_) => 2,
            });
            material.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            material.extend_from_slice(sha256(&bytes).to_hex().as_bytes());
        }
        Ok(sha256(&material))
    }

    fn read_graph(&self, run: &StableId) -> Result<Option<Vec<u8>>> {
        let graph = confined_file(&self.root, Path::new(&graph_file(run)?))?;
        match std::fs::read(graph) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    // ── approve / publish ────────────────────────────────────────────

    /// Explicit user intent, durably recorded before any dispatch.
    pub fn approve(&mut self, run: &StableId, by: &str) -> Result<()> {
        let v = self
            .run(run)
            .ok_or_else(|| PublishError::UnknownRun(run.clone()))?;
        let Some(grant) = &v.grant else {
            return Err(self.refuse(run, "no grant: preflight blocked".into()));
        };
        if v.discarded_by.is_some() {
            return Err(self.refuse(run, "grant discarded".into()));
        }
        if v.was_dispatched() {
            return Err(self.refuse(run, format!("already dispatched ({})", v.phase().as_str())));
        }
        let grant_digest = hash_json(grant)?;
        self.append(Entry::Approved {
            run: run.clone(),
            by: by.into(),
            grant_digest,
        })
    }

    /// Withdraw a grant that was never dispatched. The run's evidence
    /// stays; it can no longer be approved or published.
    pub fn discard(&mut self, run: &StableId, by: &str) -> Result<()> {
        let v = self
            .run(run)
            .ok_or_else(|| PublishError::UnknownRun(run.clone()))?;
        if v.grant.is_none() {
            return Err(self.refuse(run, "no grant to discard".into()));
        }
        if v.was_dispatched() {
            return Err(self.refuse(
                run,
                format!(
                    "already dispatched ({}); reconcile it instead",
                    v.phase().as_str()
                ),
            ));
        }
        if v.discarded_by.is_some() {
            return Err(PublishError::Refused("grant already discarded".into()));
        }
        self.append(Entry::GrantDiscarded {
            run: run.clone(),
            by: by.into(),
        })
    }

    /// Re-derive every sealed hash from what is on disk *now* and name
    /// each one that no longer matches.
    fn broken_seals(
        &self,
        v: &RunView,
        grant: &PreflightGrant,
        plan: &SealedPlan,
        cfg: &TargetConfig,
        ruleset: &dyn Ruleset,
        site: &Path,
    ) -> Result<Vec<&'static str>> {
        let mut broken = Vec::new();
        let current = SealedPlan {
            files: if site.is_dir() {
                deployable_inventory(site).unwrap_or_default()
            } else {
                vec![]
            },
            ..plan.clone()
        };
        if artifact_seal_hash(site, &current.files).unwrap_or(Hash([0; 32]))
            != grant.artifact_set_hash
        {
            broken.push("artifact_set");
        }
        if plan.hash()? != grant.plan_hash || current.hash()? != grant.plan_hash {
            broken.push("plan");
        }
        let run = v
            .run
            .as_ref()
            .ok_or_else(|| PublishError::Refused("missing run ID".into()))?;
        let graph = self.read_graph(run)?.unwrap_or_default();
        if sha256(&graph) != grant.graph_hash {
            broken.push("graph");
        }
        if hash_json(cfg)? != grant.target_cfg_hash
            || v.target_cfg.as_ref() != Some(&serde_json::to_value(cfg)?)
        {
            broken.push("target_cfg");
        }
        if ruleset.hash() != grant.ruleset_hash {
            broken.push("ruleset");
        }
        if self.engine.locate()?.version != grant.boris_version {
            broken.push("boris_version");
        }
        Ok(broken)
    }

    /// Deliver exactly the sealed plan, once. Returns the resulting state.
    pub fn publish(
        &mut self,
        run: &StableId,
        cfg: &TargetConfig,
        ruleset: &dyn Ruleset,
    ) -> Result<RunState> {
        let all = self.runs();
        let v = all
            .get(run)
            .cloned()
            .ok_or_else(|| PublishError::UnknownRun(run.clone()))?;
        if v.was_dispatched() {
            return Err(self.refuse(
                run,
                format!(
                    "already dispatched ({}); runs are never retried",
                    v.phase().as_str()
                ),
            ));
        }
        let (Some(grant), Some(plan)) = (v.grant.clone(), v.plan.clone()) else {
            return Err(self.refuse(run, "no grant: preflight blocked".into()));
        };
        if v.discarded_by.is_some() {
            return Err(self.refuse(run, "grant discarded".into()));
        }
        let approved = self.journal.records().iter().any(|r| {
            matches!(&r.entry, Entry::Approved { run: a, grant_digest, .. }
                if a == run && hash_json(&grant).is_ok_and(|h| h == *grant_digest))
        });
        if !approved {
            return Err(self.refuse(run, "not approved".into()));
        }
        if let Some((other, _)) = all
            .iter()
            .find(|(id, o)| *id != run && o.target == v.target && o.is_unresolved())
        {
            let other = other.0.clone();
            return Err(self.refuse(
                run,
                format!("run {other} on this target is unresolved; reconcile it first"),
            ));
        }
        let verified = (|| {
            let plan: SealedPlan = serde_json::from_value(plan)?;
            validate_plan(run, &plan)?;
            validate_destination(&cfg.adapter, &cfg.destination)?;
            let site = confined(&self.root, Path::new(&plan.site))?;
            let broken = self.broken_seals(&v, &grant, &plan, cfg, ruleset, &site)?;
            if !broken.is_empty() {
                return Err(PublishError::SealBroken(broken));
            }
            if !self.adapters.contains_key(&cfg.adapter) {
                return Err(PublishError::Refused(format!(
                    "no delivery adapter '{}'",
                    cfg.adapter
                )));
            }
            let files = load_sealed(&site, &plan)?;
            Ok::<(SealedPlan, PathBuf, Vec<(crate::PlanFile, Vec<u8>)>), PublishError>((
                plan, site, files,
            ))
        })();
        let (plan, site, files) = match verified {
            Ok(v) => v,
            Err(e) => {
                // Seal verification is part of the publish decision. Even an
                // I/O or confinement failure must leave durable evidence; a
                // bare error would otherwise look exactly like a run that was
                // never attempted after a restart.
                let reason = refusal_reason(&e);
                let _ = self.refuse(run, reason);
                return Err(e);
            }
        };

        // Durable before contact: delivery noun + Dispatching.
        self.append(Entry::Delivery(DeliveryNoun {
            id: derive_id("dlv", &[&run.0, &cfg.adapter, &grant.plan_hash.to_hex()]),
            run: run.clone(),
            target: plan.target.clone(),
            adapter: cfg.adapter.clone(),
            plan_hash: grant.plan_hash,
            files: plan.files.len() as u64,
            bytes: plan.total_bytes(),
        }))?;
        self.set_state(run, RunState::Dispatching, None)?;

        let adapter = &self.adapters[&cfg.adapter];
        let result = adapter.deliver(&Delivery {
            run,
            plan: &plan,
            files: &files,
            site: &site,
            config: cfg,
            secrets: self.secrets,
            engine: self.engine,
        });
        let (state, note) = match result {
            Ok(Delivered::Submitted) => (RunState::Submitted, None),
            Ok(Delivered::Complete(s)) => (RunState::Complete(s), None),
            Err(f) if f.contacted => (RunState::Unknown, Some(f.message)),
            Err(f) => (RunState::Complete(RunStatus::Failed), Some(f.message)),
        };
        self.set_state(run, state.clone(), note)?;
        if matches!(
            state,
            RunState::Complete(RunStatus::Complete | RunStatus::Partial)
        ) {
            self.ship_check(run, &plan, cfg)?;
        }
        Ok(state)
    }

    fn ship_check(&mut self, run: &StableId, plan: &SealedPlan, cfg: &TargetConfig) -> Result<()> {
        if cfg.exports.is_empty() {
            return Ok(());
        }
        let adapter = &self.adapters[&cfg.adapter];
        let read = |p: &str| adapter.read_back(cfg, p);
        let exports = crate::exports::check(
            plan,
            &cfg.exports,
            adapter
                .can_read_back()
                .then_some(&read as crate::exports::ReadBack<'_>),
        );
        self.append(Entry::ExportsChecked {
            run: run.clone(),
            exports,
        })
    }

    // ── reconcile ────────────────────────────────────────────────────

    /// Resolve an `Unknown`/`Submitted` run from an observation. Records
    /// evidence and a terminal state; never re-delivers.
    pub fn reconcile(&mut self, run: &StableId, obs: Observation, by: &str) -> Result<RunState> {
        let v = self
            .run(run)
            .ok_or_else(|| PublishError::UnknownRun(run.clone()))?;
        if !matches!(v.state, Some(RunState::Unknown | RunState::Submitted)) {
            return Err(self.refuse(
                run,
                format!(
                    "reconcile needs unknown/submitted, run is {}",
                    v.phase().as_str()
                ),
            ));
        }
        let status = match obs {
            Observation::Delivered(s) => s,
            Observation::NotDelivered => RunStatus::Failed,
        };
        let shipment = if matches!(status, RunStatus::Complete | RunStatus::Partial)
            && let (Some(plan), Some(cfg)) = (v.plan, v.target_cfg)
        {
            Some(decode_shipment(run, plan, cfg)?)
        } else {
            None
        };
        self.append(Entry::Reconciled {
            run: run.clone(),
            observation: format!("{obs:?}"),
            by: by.into(),
        })?;
        let state = RunState::Complete(status);
        self.set_state(run, state.clone(), Some(format!("reconciled by {by}")))?;
        if let Some((plan, cfg)) = shipment
            && self.adapters.contains_key(&cfg.adapter)
        {
            self.ship_check(run, &plan, &cfg)?;
        }
        Ok(state)
    }

    /// Reconcile from the adapter's read-only probe, if it has one. The
    /// probe inspects the destination sealed for *this* run, not the
    /// current target config.
    pub fn reconcile_by_probe(&mut self, run: &StableId) -> Result<Option<RunState>> {
        let v = self
            .run(run)
            .ok_or_else(|| PublishError::UnknownRun(run.clone()))?;
        let (Some(plan), Some(cfg)) = (v.plan, v.target_cfg) else {
            return Ok(None);
        };
        let (plan, cfg) = decode_shipment(run, plan, cfg)?;
        let Some(adapter) = self.adapters.get(&cfg.adapter) else {
            return Ok(None);
        };
        let Some(obs) = adapter.probe(&plan, &cfg) else {
            return Ok(None);
        };
        let by = format!("probe:{}", cfg.adapter);
        self.reconcile(run, obs, &by).map(Some)
    }
}

fn refusal_reason(e: &PublishError) -> String {
    match e {
        PublishError::Refused(s) => s.clone(),
        PublishError::SealBroken(b) => format!("seal broken: {}", b.join(", ")),
        e => e.to_string(),
    }
}

fn decode_shipment(
    run: &StableId,
    plan: serde_json::Value,
    cfg: serde_json::Value,
) -> Result<(SealedPlan, TargetConfig)> {
    let plan: SealedPlan = serde_json::from_value(plan)?;
    let cfg: TargetConfig = serde_json::from_value(cfg)?;
    validate_plan(run, &plan)?;
    validate_destination(&cfg.adapter, &cfg.destination)?;
    Ok((plan, cfg))
}

fn validate_destination(adapter: &str, destination: &str) -> Result<()> {
    if adapter == COPY_TO_FOLDER {
        copy_destination(destination).map_err(|_| {
            PublishError::Refused(format!(
                "invalid copy-to-folder destination: {destination:?}"
            ))
        })?;
    }
    Ok(())
}

fn validate_plan(run: &StableId, plan: &SealedPlan) -> Result<()> {
    if plan.site != site_dir(run)? {
        return Err(PublishError::Refused(
            "plan site is not this run's output directory".into(),
        ));
    }
    validate_destination(&plan.adapter, &plan.destination)?;
    plan.validate_file_paths()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Target;

    #[test]
    fn decoded_plan_cannot_select_another_runs_site() {
        let run = StableId("run-current".into());
        let other = StableId("run-other".into());
        let cfg = TargetConfig::new(
            Target::StaticDist,
            "p",
            "https://example.test/",
            COPY_TO_FOLDER,
            "out",
        );
        let mut plan = SealedPlan {
            target: cfg.target.id().into(),
            adapter: cfg.adapter.clone(),
            destination: cfg.destination.clone(),
            site: site_dir(&other).unwrap(),
            files: vec![],
        };
        let decode = |plan: &SealedPlan| {
            decode_shipment(
                &run,
                serde_json::to_value(plan).unwrap(),
                serde_json::to_value(&cfg).unwrap(),
            )
        };
        assert!(matches!(decode(&plan), Err(PublishError::Refused(_))));
        plan.site = site_dir(&run).unwrap();
        assert!(decode(&plan).is_ok());
    }
}
