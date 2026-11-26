//! The sealed plan: exactly which bytes go where. Hashes are sha256 over
//! canonical JSON (struct field order + sorted file list).

use crate::{PublishError, Result};
use cv_core::sys::Hash;
use cv_store::paths::{confined_file, is_link};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PlanFile {
    /// Output-relative, `/`-separated.
    pub path: String,
    pub bytes: u64,
    pub sha256: Hash,
    /// Boris's committed artifact `kind` when this file is payload selected
    /// through the proof inventory; `None` for uninventoried walks. Sealed
    /// into the plan hash, so page-ness downstream never falls back to the
    /// file extension.
    ///
    /// Compatibility is a conscious decision, not an accident: `None` is
    /// the safe direction. Kind only ever *narrows* what is treated as a
    /// page — a plan written before this field existed degrades to "no
    /// page projection" (no URL list entries, no Audit HtmlPage parsing)
    /// rather than falsely claiming page-ness for content assets. The
    /// artifact-set seal independently binds the inventory bytes carrying
    /// the authoritative kinds, so a missing cached kind can never make
    /// the seal lie. Journals are per-project runtime evidence written and
    /// read by the same build, and this PR is unmerged, so no migration is
    /// required.
    #[serde(default)]
    pub kind: Option<String>,
    /// Boris's `advertised` flag for the artifact: draft pages commit as
    /// html-page but are not advertised to indexers. Same safe-direction
    /// compatibility reasoning as `kind`: `None` never over-advertises
    /// because URL projection treats absent as not-explicitly-advertised.
    #[serde(default)]
    pub advertised: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SealedPlan {
    pub target: String,
    pub adapter: String,
    pub destination: String,
    /// Project-relative output dir the preflight built into.
    pub site: String,
    pub files: Vec<PlanFile>,
}

pub fn sha256(bytes: &[u8]) -> Hash {
    Hash(Sha256::digest(bytes).into())
}

pub fn hash_json<T: Serialize>(v: &T) -> Result<Hash> {
    Ok(sha256(&serde_json::to_vec(v)?))
}

impl SealedPlan {
    pub(crate) fn validate_file_paths(&self) -> Result<()> {
        for f in &self.files {
            host_relative(&f.path)?;
        }
        Ok(())
    }

    pub fn hash(&self) -> Result<Hash> {
        hash_json(self)
    }

    /// `artifact_set_hash`: the byte inventory alone, independent of
    /// where it's being delivered.
    pub fn artifact_set_hash(&self) -> Result<Hash> {
        hash_json(&self.files)
    }

    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }

    pub fn file(&self, path: &str) -> Option<&PlanFile> {
        self.files.iter().find(|f| f.path == path)
    }
}

/// Host-valid output-relative publish path. Copy-to-folder delivers on the
/// local host, so the grammar is the host's — not the cross-platform
/// portable subset `portable_relative` enforces for sidecar state. Real
/// Boris output names such as `aux.html` or `guides/a:b.html` are valid
/// payload on unix hosts. The security-critical properties are unchanged:
/// relative, normalized, no parent traversal, no escape. Windows-reserved
/// names are a destination concern on Windows hosts, where `Path::components`
/// already parses host separators.
pub(crate) fn host_relative(path: &str) -> Result<&Path> {
    use std::path::Component;
    let p = Path::new(path);
    let mut count = 0usize;
    for c in p.components() {
        match c {
            Component::Normal(seg) => {
                if seg.as_encoded_bytes().contains(&0) {
                    return Err(PublishError::Refused(format!(
                        "invalid relative publish path: {path:?}"
                    )));
                }
                // Windows host grammar: a colon inside any component spells a
                // drive or ADS stream separator there, never a payload name —
                // writing it forks the file identity (`guides/a` + stream
                // `b.html`) so inventory and bytes diverge. Unix hosts treat
                // the same bytes as an ordinary confined filename, and
                // Windows-invalid characters (`<>"|?*`) are refused on the
                // host that cannot write them. No cross-platform parity is
                // claimed: validity is per-host by construction.
                #[cfg(windows)]
                {
                    if seg
                        .as_encoded_bytes()
                        .iter()
                        .any(|b| matches!(b, b':' | b'<' | b'>' | b'"' | b'|' | b'?' | b'*'))
                    {
                        return Err(PublishError::Refused(format!(
                            "invalid windows publish path: {path:?}"
                        )));
                    }
                    // ASCII control characters are never valid Win32 file
                    // name bytes (NUL is already refused above).
                    if seg.as_encoded_bytes().iter().any(|b| (1..=31).contains(b)) {
                        return Err(PublishError::Refused(format!(
                            "windows control character in publish path: {path:?}"
                        )));
                    }
                    let lossy = seg.to_string_lossy();
                    // Trailing dots/spaces are stripped by Win32
                    // canonicalization per component, so `dot./page.html`
                    // and `dot/page.html` silently converge on one file
                    // identity — the namespace collision class this grammar
                    // exists to prevent. Unfaithful shapes refuse outright.
                    if lossy.ends_with('.') || lossy.ends_with(' ') {
                        return Err(PublishError::Refused(format!(
                            "windows trailing dot/space in publish path: {path:?}"
                        )));
                    }
                    // Reserved device names stay reserved with extensions:
                    // `aux.html`/`nul.txt` open devices, not files. Matching
                    // happens after Windows-equivalent trailing dot/space
                    // normalization of the stem (`NUL .txt` canonicalizes
                    // toward the device), and COM/LPT recognize the
                    // superscript digits real Boris emits (`COM¹.html`).
                    let stem = lossy.split('.').next().unwrap_or("");
                    let normalized = stem
                        .trim_end_matches(|c| c == '.' || c == ' ')
                        .to_uppercase();
                    let device = matches!(normalized.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                        || ["COM", "LPT"].iter().any(|prefix| {
                            let Some(rest) = normalized.strip_prefix(prefix) else {
                                return false;
                            };
                            let mut chars = rest.chars();
                            match (chars.next(), chars.next()) {
                                (Some(d), None) => {
                                    d.is_ascii_digit() && d != '0'
                                        || matches!(d, '\u{b9}' | '\u{b2}' | '\u{b3}')
                                }
                                _ => false,
                            }
                        });
                    if device {
                        return Err(PublishError::Refused(format!(
                            "windows reserved device name in publish path: {path:?}"
                        )));
                    }
                }
                count += 1;
            }
            _ => {
                return Err(PublishError::Refused(format!(
                    "invalid relative publish path: {path:?}"
                )));
            }
        }
    }
    if count == 0 || path.contains('\0') {
        return Err(PublishError::Refused(format!(
            "invalid relative publish path: {path:?}"
        )));
    }
    Ok(p)
}

/// Walk an output tree into a sorted inventory.
pub fn inventory(root: &Path) -> Result<Vec<PlanFile>> {
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<PlanFile>) -> Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        let meta = e.metadata()?;
        if is_link(&meta) {
            return Err(PublishError::Refused(format!(
                "linked file in output: {p:?}"
            )));
        }
        if meta.is_dir() {
            walk(root, &p, out)?;
        } else if meta.is_file() {
            let rel = p
                .strip_prefix(root)
                .map_err(|_| PublishError::Refused(format!("path escapes output: {p:?}")))?;
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_str())
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| PublishError::Refused("non-UTF-8 path in output".into()))?
                .join("/");
            host_relative(&rel)?;
            let bytes = std::fs::read(&p)?;
            out.push(PlanFile {
                kind: None,
                advertised: None,
                path: rel,
                bytes: bytes.len() as u64,
                sha256: sha256(&bytes),
            });
        } else {
            return Err(PublishError::Refused(format!(
                "non-regular file in output: {p:?}"
            )));
        }
    }
    Ok(())
}

/// Read every sealed file and verify its bytes. Adapters receive only
/// these verified buffers — never a path to re-read.
pub fn load_sealed(site: &Path, plan: &SealedPlan) -> Result<Vec<(PlanFile, Vec<u8>)>> {
    plan.validate_file_paths()?;
    let mut out = Vec::with_capacity(plan.files.len());
    for f in &plan.files {
        let path = confined_file(site, Path::new(&f.path))?;
        let bytes = std::fs::read(path)?;
        if bytes.len() as u64 != f.bytes || sha256(&bytes) != f.sha256 {
            return Err(PublishError::SealBroken(vec!["artifact_set"]));
        }
        out.push((f.clone(), bytes));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    /// Host grammar is per-host by construction: real Boris page names that
    /// are valid confined files on Unix are device names, wildcards, or
    /// ADS/drive spellings on Windows. No cross-platform parity is claimed.
    #[test]
    fn host_relative_probe() {
        assert!(super::host_relative("../escape.html").is_err());
        #[cfg(not(windows))]
        {
            assert!(super::host_relative("aux.html").is_ok());
            assert!(super::host_relative("guides/a:b.html").is_ok());
            assert!(super::host_relative("star*.html").is_ok());
            // The same real Boris shapes are ordinary confined payload here.
            assert!(super::host_relative("COM\u{b9}.html").is_ok());
            assert!(super::host_relative("dot./page.html").is_ok());
        }
        #[cfg(windows)]
        {
            assert!(super::host_relative("aux.html").is_err());
            assert!(super::host_relative("guides/a:b.html").is_err());
            assert!(super::host_relative("star*.html").is_err());
            assert!(super::host_relative("NUL.txt").is_err());
            assert!(super::host_relative("com1.html").is_err());
            assert!(super::host_relative("lpt9").is_err());
            assert!(super::host_relative("con").is_err());
            // Real Boris 0.9.0 emitted shapes, reviewer-verified.
            assert!(super::host_relative("COM\u{b9}.html").is_err());
            assert!(super::host_relative("dot./page.html").is_err());
            assert!(super::host_relative("NUL .txt").is_err());
            assert!(super::host_relative("trail /page.html").is_err());
        }
    }
}
