//! Ship-the-outputs pass: machine exports (sitemap, RSS, llms.txt,
//! context/RAG) must be in the sealed plan **and** read back from the
//! destination byte-identical. Generated-but-unshipped counts as missing.

use crate::plan::{SealedPlan, sha256};
use cv_core::engine::ExportKind;
use cv_ledger::journal::ExportEvidence;

pub const PRESENT: &str = "present";
pub const MISSING: &str = "missing";
pub const STALE: &str = "stale";
pub const UNCONFIRMED: &str = "unconfirmed";

/// Reads a shipped file back from the destination.
pub type ReadBack<'a> = &'a dyn Fn(&str) -> Option<Vec<u8>>;

fn kind_name(k: ExportKind) -> String {
    format!("{k:?}").to_lowercase()
}

/// `read_back = None`: the transport can't read back → `unconfirmed`.
/// `path` ending in `/` names a directory export (every file under it).
pub fn check(
    plan: &SealedPlan,
    exports: &[(ExportKind, String)],
    read_back: Option<ReadBack<'_>>,
) -> Vec<ExportEvidence> {
    let mut out = Vec::new();
    for (kind, path) in exports {
        let files: Vec<_> = if path.ends_with('/') {
            plan.files
                .iter()
                .filter(|f| f.path.starts_with(path.as_str()))
                .collect()
        } else {
            plan.file(path).into_iter().collect()
        };
        let ev = |status: &str, sha: Option<String>| ExportEvidence {
            kind: kind_name(*kind),
            path: path.clone(),
            status: status.into(),
            sha256: sha,
        };
        if files.is_empty() {
            out.push(ev(MISSING, None));
            continue;
        }
        let Some(read_back) = read_back else {
            out.push(ev(UNCONFIRMED, None));
            continue;
        };
        let mut status = PRESENT;
        for f in &files {
            match read_back(&f.path) {
                None => {
                    status = MISSING;
                    break;
                }
                Some(b) if sha256(&b) != f.sha256 => status = STALE,
                Some(_) => {}
            }
        }
        let sha = (files.len() == 1).then(|| files[0].sha256.to_hex());
        out.push(ev(status, sha));
    }
    out
}

pub fn all_shipped(ev: &[ExportEvidence]) -> bool {
    ev.iter().all(|e| e.status == PRESENT)
}
