//! Lift Boris's proof pack (`_boris/proof/{artifacts,checks,claims}.json`)
//! into ledger nouns. Boris's verdicts are copied, never recomputed.

use crate::nouns::{ArtifactNoun, CheckNoun, ClaimNoun, derive_id};
use crate::{LedgerError, Result};
use cv_contracts::proof::EvidenceFile;

pub use cv_contracts::proof::{ArtifactIndex, ChecksFile};
use cv_core::id::StableId;
use std::path::Path;

pub const PROOF_DIR: &str = "_boris/proof";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProofNouns {
    pub artifacts: Vec<ArtifactNoun>,
    pub checks: Vec<CheckNoun>,
    pub claims: Vec<ClaimNoun>,
}

fn read(dir: &Path, name: &str) -> Result<Option<Vec<u8>>> {
    match std::fs::read(dir.join(name)) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Boris pins `schema_version = 1` on every proof-pack file
/// (`docs/contracts/publication-{artifacts,checks,claims}.md`). Ingest
/// refuses anything else rather than silently lifting drifted nouns.
const PROOF_SCHEMA: u32 = 1;

fn schema(what: &str, got: u32) -> Result<()> {
    if got == PROOF_SCHEMA {
        Ok(())
    } else {
        Err(LedgerError::Proof(format!(
            "{what}: schema_version {got} unsupported (want {PROOF_SCHEMA})"
        )))
    }
}

/// Parse proof JSON bytes (any may be absent — an absent file yields no
/// nouns, never invented ones).
pub fn from_bytes(
    run: &StableId,
    artifacts: Option<&[u8]>,
    checks: Option<&[u8]>,
    claims: Option<&[u8]>,
) -> Result<ProofNouns> {
    let bad = |what: &str, e: serde_json::Error| LedgerError::Proof(format!("{what}: {e}"));
    let mut out = ProofNouns::default();
    if let Some(b) = artifacts {
        let idx: ArtifactIndex = serde_json::from_slice(b).map_err(|e| bad("artifacts.json", e))?;
        schema("artifacts.json", idx.schema_version)?;
        for a in idx.artifacts {
            out.artifacts.push(ArtifactNoun {
                id: derive_id("art", &[&run.0, &idx.target, &a.path, &a.sha256]),
                run: run.clone(),
                target: idx.target.clone(),
                path: a.path,
                kind: a.kind,
                status: a.status,
                required: a.required,
                bytes: a.bytes,
                sha256: a.sha256,
            });
        }
    }
    if let Some(b) = checks {
        let f: ChecksFile = serde_json::from_slice(b).map_err(|e| bad("checks.json", e))?;
        schema("checks.json", f.schema_version)?;
        let inv = f.artifact_inventory.as_ref().map(|d| d.sha256.clone());
        for c in f.checks {
            let counts = c.counts.as_ref();
            out.checks.push(CheckNoun {
                id: derive_id("chk", &[&run.0, &f.target, &c.id]),
                run: run.clone(),
                target: f.target.clone(),
                check_id: c.id,
                status: c.status,
                ran: c.ran,
                coverage: c.coverage,
                checked: counts.map_or(0, |n| n.checked),
                findings: counts.map_or(0, |n| n.findings),
                inventory_sha256: inv.clone(),
            });
        }
    }
    if let Some(b) = claims {
        let f: EvidenceFile = serde_json::from_slice(b).map_err(|e| bad("claims.json", e))?;
        schema("claims.json", f.schema_version)?;
        let report = f
            .inputs
            .get("publication_checks")
            .and_then(|v| v.get("sha256"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let list = f
            .inputs
            .get("claims")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for c in list {
            let s = |k: &str| c.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let claim_id = s("id");
            out.claims.push(ClaimNoun {
                id: derive_id("clm", &[&run.0, &f.target, &claim_id]),
                run: run.clone(),
                target: f.target.clone(),
                claim_id,
                status: s("status"),
                statement: s("statement"),
                check_id: c
                    .get("evidence")
                    .and_then(|e| e.get("check_id"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                checks_report_sha256: report.clone(),
            });
        }
    }
    Ok(out)
}

/// The target named by the proof artifact inventory under a built output
/// tree. `None` means Boris did not produce an inventory. The mirror validates
/// both the inventory's format identity and schema version before trusting
/// anything inside it.
pub fn artifact_inventory(out_dir: &Path) -> Result<Option<ArtifactIndex>> {
    let dir = out_dir.join(PROOF_DIR);
    let Some(bytes) = read(&dir, "artifacts.json")? else {
        return Ok(None);
    };
    let idx: ArtifactIndex = serde_json::from_slice(&bytes)
        .map_err(|e| LedgerError::Proof(format!("artifacts.json: {e}")))?;
    if idx.format != "boris-publication-artifacts" {
        return Err(LedgerError::Proof(format!(
            "artifacts.json: format {:?} is not boris-publication-artifacts",
            idx.format
        )));
    }
    schema("artifacts.json", idx.schema_version)?;
    validate_artifact_contract(&idx)?;
    Ok(Some(idx))
}

/// Boris's committed-artifact contract, mirrored for validation only:
/// artifact kinds are a closed enum, each kind has exactly one producer,
/// status is a closed enum, and the inventory is in canonical path order.
/// Boris itself rejects inventories violating these; a mirror that accepts
/// them would seal payload Boris never committed.
fn validate_artifact_contract(idx: &ArtifactIndex) -> Result<()> {
    fn producer_for(kind: &str) -> Option<&'static str> {
        Some(match kind {
            "html-page" => "html-render",
            "theme-asset" => "theme-assets",
            "content-asset" => "content-assets",
            "rendered-search" => "rendered-search",
            "sitemap" => "sitemap",
            "rss" => "rss",
            "llms" => "llms",
            "static-file" => "static-files",
            _ => return None,
        })
    }
    let mut prev: Option<&str> = None;
    for a in &idx.artifacts {
        let Some(expected) = producer_for(&a.kind) else {
            return Err(LedgerError::Proof(format!(
                "artifacts.json: {} has unknown kind {:?}",
                a.path, a.kind
            )));
        };
        if a.producer != expected {
            return Err(LedgerError::Proof(format!(
                "artifacts.json: {} producer {:?} is not {expected:?} for kind {:?}",
                a.path, a.producer, a.kind
            )));
        }
        if !(a.sha256.len() == 64
            && a.sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        {
            return Err(LedgerError::Proof(format!(
                "artifacts.json: {} sha256 is not lowercase hex digest shape",
                a.path
            )));
        }
        if !matches!(
            a.status.as_str(),
            "committed" | "omitted-by-plan" | "not-applicable"
        ) {
            return Err(LedgerError::Proof(format!(
                "artifacts.json: {} has unknown status {:?}",
                a.path, a.status
            )));
        }
        if let Some(prev) = prev
            && prev > a.path.as_str()
        {
            return Err(LedgerError::Proof(format!(
                "artifacts.json: {} is out of canonical path order",
                a.path
            )));
        }
        prev = Some(&a.path);
    }
    Ok(())
}

/// Read the proof pack under a built output tree.
pub fn from_output_dir(run: &StableId, out_dir: &Path) -> Result<ProofNouns> {
    let dir = out_dir.join(PROOF_DIR);
    let a = read(&dir, "artifacts.json")?;
    let c = read(&dir, "checks.json")?;
    let l = read(&dir, "claims.json")?;
    from_bytes(run, a.as_deref(), c.as_deref(), l.as_deref())
}
