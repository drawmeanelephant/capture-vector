//! Collision and zero-write preflight authority for migration adoption.
//! Split from the transaction orchestration to keep each file below the
//! repository size budget; the public collision API is re-exported by
//! `adopt` unchanged.

use super::{AdoptError, AdoptionSource, PlannedFile, provenance_paths};
use crate::write;
use cv_core::engine::WriteOutcome;
use cv_store::imports as store;
use cv_store::paths::portable_relative;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollisionKind {
    Path,
    Id,
    /// A parent component of an intended target is a file, a link, or
    /// unresolvable — a deterministic shape conflict detected before any
    /// write rather than a mid-copy failure after partial content landed.
    ParentShape,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    pub kind: CollisionKind,
    /// The incoming path or entity id.
    pub incoming: String,
    /// The existing path or id owner.
    pub existing: String,
}

impl std::fmt::Display for Collision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            CollisionKind::Path => write!(
                f,
                "path {} already exists ({})",
                self.incoming, self.existing
            ),
            CollisionKind::Id => write!(
                f,
                "page id {} is already used by {}",
                self.incoming, self.existing
            ),
            CollisionKind::ParentShape => {
                write!(
                    f,
                    "path {} cannot be written: {}",
                    self.incoming, self.existing
                )
            }
        }
    }
}

/// True when the target already holds exactly the bytes adoption would
/// write. Length is checked before content so large mismatches cost nothing.
pub(super) fn bytes_identical(target: &Path, source: &Path) -> bool {
    let (Ok(t), Ok(s)) = (std::fs::read(target), std::fs::read(source)) else {
        return false;
    };
    t == s
}

/// Refuse every path and id overlap before the first write. `existing_ids`
/// is the composition root's current Boris-view of the target workspace
/// (`id` → content-input-relative path); adoption itself never parses
/// Boris frontmatter.
///
/// `resume` changes path semantics only: with a durable recovery journal,
/// a target holding exactly the incoming bytes is evidence of the
/// interrupted attempt's own staged write and is skipped, while different
/// bytes can only be foreign data and still refuse. Without the journal,
/// every existing target path is a collision — even byte-identical ones,
/// because nothing distinguishes them from the user's own pre-existing
/// content. Returns the files that still need copying in resume mode.
/// Classification of one parent component of an intended target.
/// Preflight must distinguish these exactly: an absent component will be
/// created, an ordinary directory is fine, and everything else — an
/// ordinary file, a link, or a resolution failure — is a deterministic
/// conflict that must refuse the adoption *before the first write*,
/// never surface as a mid-copy failure after partial content landed.
fn blocked_parents(target_root: &Path, rel_targets: &[String]) -> Vec<Collision> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for target in rel_targets {
        let rel = Path::new(target);
        let mut ancestors: Vec<&Path> = rel.ancestors().skip(1).collect();
        ancestors.reverse();
        for parent in ancestors {
            if parent.as_os_str().is_empty() || !seen.insert(parent.to_path_buf()) {
                continue;
            }
            let abs = target_root.join(parent);
            match std::fs::symlink_metadata(&abs) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    out.push(Collision {
                        kind: CollisionKind::ParentShape,
                        incoming: target.clone(),
                        existing: format!(
                            "{} is a link, so nothing may be written below it",
                            parent.display()
                        ),
                    });
                    break;
                }
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => {
                    out.push(Collision {
                        kind: CollisionKind::ParentShape,
                        incoming: target.clone(),
                        existing: format!(
                            "{} is not a directory, so nothing may be written below it",
                            parent.display()
                        ),
                    });
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Absent components are created by the staged writer,
                    // and nothing can exist below a missing directory.
                    break;
                }
                Err(e) => {
                    out.push(Collision {
                        kind: CollisionKind::ParentShape,
                        incoming: target.clone(),
                        existing: format!("{} cannot be resolved: {e}", parent.display()),
                    });
                    break;
                }
            }
        }
    }
    out
}

/// Duplicate entity ids inside the migration evidence are invalid no
/// matter what the target workspace holds, so they are classified before
/// even the recovery journal may be written.
pub(super) fn incoming_id_collisions(src: &AdoptionSource) -> Vec<Collision> {
    let mut incoming_ids = BTreeSet::new();
    let mut out = Vec::new();
    for page in &src.pages {
        if !incoming_ids.insert(page.entity_id.clone()) {
            out.push(Collision {
                kind: CollisionKind::Id,
                incoming: page.entity_id.clone(),
                existing: "another page in the same migration run".into(),
            });
        }
    }
    out
}

/// Prove the shape of every logical write target before the recovery
/// journal — itself a write — is allowed to land. Content targets are
/// shape-checked here only; whether an ordinary existing file is a
/// collision, starter debris, or byte-identical partial output belongs to
/// the mode-specific collision/recovery logic. Sidecars and provenance
/// targets may be ordinary files because rewrite/byte-reuse is part of
/// their authority contract. Links, directories, non-files, broken parent
/// chains, and resolution failures are deterministic conflicts.
pub(super) fn preflight_write_target_shapes(
    target_root: &Path,
    src: &AdoptionSource,
    planned: &[PlannedFile],
) -> Result<(), AdoptError> {
    let mut rel_targets: Vec<String> = planned.iter().map(|p| p.target.clone()).collect();
    rel_targets.push(format!("{}/{}", store::DIR, cv_store::sidecar::STAMP));
    rel_targets.push(format!("{}/{}/{}", store::DIR, src.run_id, store::PENDING));
    rel_targets.push(format!("{}/{}/{}", store::DIR, src.run_id, store::ADOPTION));
    rel_targets.push(format!("{}/{}/{}", store::DIR, src.run_id, store::RECORD));
    let provenance = provenance_paths(&src.out)?;
    let mut provenance_targets = Vec::new();
    for source in &provenance {
        let rel = source.strip_prefix(&src.out).map_err(|_| {
            AdoptError::Content(format!("{} is outside the run output", source.display()))
        })?;
        let rel = rel.to_string_lossy().replace('\\', "/");
        portable_relative(&rel)
            .map_err(|_| AdoptError::Content(format!("provenance path {rel:?} is not portable")))?;
        // Reading every provenance byte is part of zero-write preflight:
        // otherwise an unreadable source document is discovered only after
        // content has landed, exactly the partial-write shape this pass
        // exists to exclude.
        std::fs::read(source)?;
        let target_rel = format!("{}/{}/{}", store::DIR, src.run_id, rel);
        rel_targets.push(target_rel.clone());
        provenance_targets.push((source.clone(), rel, target_rel));
    }
    rel_targets.sort();
    rel_targets.dedup();
    let mut out = blocked_parents(target_root, &rel_targets);
    for rel in &rel_targets {
        let target = target_root.join(rel);
        match std::fs::symlink_metadata(&target) {
            Ok(meta) if cv_store::paths::is_link(&meta) || !meta.is_file() => {
                out.push(Collision {
                    kind: CollisionKind::ParentShape,
                    incoming: rel.clone(),
                    existing: format!("{rel} is not an ordinary writable file"),
                });
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => out.push(Collision {
                kind: CollisionKind::ParentShape,
                incoming: rel.clone(),
                existing: format!("{rel} cannot be resolved: {e}"),
            }),
        }
    }
    // A provenance target holding different bytes is not a writable or
    // reusable target: `copy_provenance` will refuse it. Classify that
    // deterministic conflict before the journal/content writes instead of
    // discovering it after partial content has landed.
    for (source, rel, target_rel) in provenance_targets {
        let target = target_root.join(&target_rel);
        let ordinary = std::fs::symlink_metadata(&target)
            .map(|m| m.is_file() && !cv_store::paths::is_link(&m))
            .unwrap_or(false);
        if ordinary && !bytes_identical(&target, &source) {
            out.push(Collision {
                kind: CollisionKind::Path,
                incoming: rel,
                existing: target_rel,
            });
        }
    }
    out.sort_by(|a, b| {
        a.incoming
            .cmp(&b.incoming)
            .then(a.existing.cmp(&b.existing))
    });
    out.dedup();
    if out.is_empty() {
        Ok(())
    } else {
        Err(AdoptError::Collisions(out))
    }
}

pub fn collisions(
    target_root: &Path,
    src: &AdoptionSource,
    planned: &[PlannedFile],
    existing_ids: &BTreeMap<String, String>,
    resume: bool,
) -> Result<Vec<bool>, AdoptError> {
    preflight_write_target_shapes(target_root, src, planned)?;
    let mut out = Vec::new();
    let mut needs_copy = Vec::new();

    for p in planned {
        let target = target_root.join(&p.target);
        let metadata = match std::fs::symlink_metadata(&target) {
            Ok(metadata) => Some(metadata),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                out.push(Collision {
                    kind: CollisionKind::ParentShape,
                    incoming: p.display.clone(),
                    existing: format!("{} cannot be resolved: {e}", p.target),
                });
                None
            }
        };
        let mut copy = true;
        if let Some(metadata) = metadata {
            // `symlink_metadata` is the no-follow final-component check: a
            // link exactly at the target is existing foreign state, never an
            // absent file merely because confined resolution refused it.
            let ordinary_file = metadata.is_file() && !cv_store::paths::is_link(&metadata);
            if resume && ordinary_file && bytes_identical(&target, &p.source) {
                copy = false;
            } else {
                out.push(Collision {
                    kind: CollisionKind::Path,
                    incoming: p.display.clone(),
                    existing: p.target.clone(),
                });
            }
        }
        needs_copy.push(copy);
    }
    out.extend(incoming_id_collisions(src));
    for page in &src.pages {
        if let Some(existing) = existing_ids.get(&page.entity_id) {
            // An owner at one of the adoption's own planned paths is the
            // path preflight's case, not an id collision: fresh attempts
            // already refuse it as a path collision, and in resume mode it
            // is the interrupted attempt's own partial output — refusing
            // it here too would make every real-world retry self-refusing.
            let own = planned.iter().any(|pl| pl.display == *existing);
            if !own {
                out.push(Collision {
                    kind: CollisionKind::Id,
                    incoming: page.entity_id.clone(),
                    existing: existing.clone(),
                });
            }
        }
    }
    out.sort_by(|a, b| {
        a.incoming
            .cmp(&b.incoming)
            .then(a.existing.cmp(&b.existing))
    });
    out.dedup();
    if out.is_empty() {
        Ok(needs_copy)
    } else {
        Err(AdoptError::Collisions(out))
    }
}

/// Copy planned files byte-for-byte with `expected=None` staged writes.
/// `needs_copy[i] == false` skips `planned[i]` (a verified resume skip);
/// every written file still refuses a surprise predecessor instead of
/// overwriting it. Returns `(written, skipped)`.
pub fn copy_content(
    target_root: &Path,
    planned: &[PlannedFile],
    needs_copy: &[bool],
) -> Result<(usize, usize), AdoptError> {
    let mut written = 0;
    let mut skipped = 0;
    for (p, needed) in planned.iter().zip(needs_copy) {
        if !needed {
            skipped += 1;
            continue;
        }
        let bytes = std::fs::read(&p.source)?;
        match write::write_staged(target_root, &p.target, &bytes, None)? {
            WriteOutcome::Written { .. } => written += 1,
            WriteOutcome::StaleBase { .. } => {
                return Err(AdoptError::Stale(format!(
                    "{} appeared or changed during adoption; nothing was overwritten",
                    p.target
                )));
            }
        }
    }
    Ok((written, skipped))
}
