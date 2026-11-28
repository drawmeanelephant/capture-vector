//! The live content-input scan and its reconciliation against Capture
//! Vector's run-time input inventory. Two modes:
//!
//! - fresh: the live tree must equal the inventory exactly — anything that
//!   changed after the lab run is stale evidence, and there is no journaled
//!   operation that could explain it;
//! - resume: the live tree must equal the inventory *plus* exactly the
//!   journaled plan's in-flight states (an unlinked source, a linked
//!   destination, a written page, a staged temp). Everything else is
//!   foreign and stops recovery — a post-crash addition or edit can never
//!   be mistaken for this transformation's own work.

use super::{AdoptError, malformed, stale};
use cv_store::imports as store;
use cv_store::paths::portable_relative;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

/// One live content-input entry, hashed and identified like the
/// run-time inventory records it. `Directory` is recorded only when the
/// bound inventory schema records directories (input-inventory version
/// 2+); older inventories treat directories as traversal structure, so
/// the live scan must too or every directory would read as an addition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LiveEntry {
    File {
        sha256: String,
        bytes: u64,
        identity: store::FileIdentity,
    },
    Symlink {
        target: String,
    },
    Directory,
    Other,
}

fn foreign(message: impl Into<String>) -> AdoptError {
    AdoptError::Foreign(vec![message.into()])
}

fn scan_dir(
    dir: &Path,
    prefix: &str,
    include_dirs: bool,
    out: &mut BTreeMap<String, LiveEntry>,
) -> Result<(), AdoptError> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| AdoptError::Content(format!("cannot read {}: {e}", dir.display())))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().into_string().map_err(|_| {
            AdoptError::Content(format!(
                "non-UTF-8 file name under the live content input {}",
                dir.display()
            ))
        })?;
        names.push((name, entry.path()));
    }
    names.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, path) in names {
        let rel = format!("{prefix}{name}");
        let meta = std::fs::symlink_metadata(&path)?;
        if cv_store::paths::is_link(&meta) {
            let target = std::fs::read_link(&path)?;
            let target = target.to_str().ok_or_else(|| {
                AdoptError::Content(format!("live symlink {rel} has a non-UTF-8 target"))
            })?;
            out.insert(
                rel,
                LiveEntry::Symlink {
                    target: target.to_string(),
                },
            );
        } else if meta.is_dir() {
            if include_dirs {
                out.insert(rel.clone(), LiveEntry::Directory);
            }
            scan_dir(&path, &format!("{rel}/"), include_dirs, out)?;
        } else if meta.is_file() {
            // Open first, then read and take identity off the same handle:
            // the hash and the identity describe one storage object even if
            // the directory entry is swapped mid-read.
            let mut file = std::fs::File::open(&path)?;
            let meta = file.metadata()?;
            if !meta.is_file() {
                return Err(AdoptError::Content(format!(
                    "live entry {rel} is not an ordinary file"
                )));
            }
            let identity = crate::file_identity::identity_of_file(&file).ok_or_else(|| {
                AdoptError::Run(format!(
                    "this platform cannot report a file identity for {rel}; the asset transformation cannot run"
                ))
            })?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            out.insert(
                rel,
                LiveEntry::File {
                    sha256: store::sha256_hex(&bytes),
                    bytes: bytes.len() as u64,
                    identity: store::FileIdentity {
                        volume: identity.0,
                        index: identity.1,
                    },
                },
            );
        } else {
            out.insert(rel, LiveEntry::Other);
        }
    }
    Ok(())
}

/// Inventory the live content input without following links, in the same
/// shape as the stored run-time inventory. `include_dirs` must match
/// whether the bound inventory records directories — compare like with
/// like or reconciliation reports every directory as an addition.
pub(super) fn scan(
    input_root: &Path,
    include_dirs: bool,
) -> Result<BTreeMap<String, LiveEntry>, AdoptError> {
    let meta = std::fs::symlink_metadata(input_root).map_err(|e| {
        AdoptError::Content(format!(
            "content input {} cannot be inventoried: {e}",
            input_root.display()
        ))
    })?;
    if cv_store::paths::is_link(&meta) || !meta.is_dir() {
        return Err(AdoptError::Content(format!(
            "content input {} is not an ordinary directory",
            input_root.display()
        )));
    }
    let mut out = BTreeMap::new();
    scan_dir(input_root, "", include_dirs, &mut out)?;
    Ok(out)
}

fn entry_matches(live: &LiveEntry, entry: &store::InputEntry) -> bool {
    match (live, entry) {
        (
            LiveEntry::File {
                sha256,
                bytes,
                identity,
            },
            store::InputEntry::File {
                sha256: esha,
                bytes: eb,
                identity: ei,
                ..
            },
        ) => sha256 == esha && bytes == eb && identity == ei,
        (
            LiveEntry::Symlink { target },
            store::InputEntry::Symlink {
                target: etarget, ..
            },
        ) => target == etarget,
        (LiveEntry::Directory, store::InputEntry::Directory { .. }) => true,
        (LiveEntry::Other, store::InputEntry::Other { .. }) => true,
        _ => false,
    }
}

/// Fresh attempt: the live tree must equal the run-time inventory exactly.
pub(super) fn reconcile_fresh(
    live: &BTreeMap<String, LiveEntry>,
    inventory: &[store::InputEntry],
) -> Result<(), AdoptError> {
    let by_path: BTreeMap<&str, &store::InputEntry> =
        inventory.iter().map(|e| (e.path(), e)).collect();
    for (path, live_entry) in live {
        match by_path.get(path.as_str()) {
            Some(entry) if entry_matches(live_entry, entry) => {}
            Some(_) => {
                return Err(stale(format!(
                    "{path} changed after the lab run; the captured input inventory no longer matches"
                )));
            }
            None => {
                return Err(stale(format!(
                    "{path} appeared after the lab run; the captured input inventory does not record it"
                )));
            }
        }
    }
    for entry in inventory {
        if !live.contains_key(entry.path()) {
            return Err(stale(format!(
                "{} disappeared after the lab run; the captured input inventory still records it",
                entry.path()
            )));
        }
    }
    Ok(())
}

/// Resume attempt: the live tree must equal the inventory plus exactly the
/// journaled transformation's in-flight states.
///
/// For every live entry there are four legal explanations: an unchanged
/// inventory entry; a journaled move's source still holding the original
/// bytes or already unlinked; a journaled destination holding the
/// journaled identity; a journaled page at pre- or post-image; or a
/// journaled staged temp with the identity the journal recorded for it.
/// Anything else refuses recovery. A journaled temp whose identity the
/// journal never recorded is ambiguous foreign data — it is never deleted
/// or overwritten, recovery just stops.
pub(super) fn reconcile_resume(
    live: &BTreeMap<String, LiveEntry>,
    transformation: &store::PendingTransformation,
) -> Result<(), AdoptError> {
    let inventory: BTreeMap<&str, &store::InputEntry> = transformation
        .inventory
        .iter()
        .map(|e| (e.path(), e))
        .collect();
    let mut by_source: BTreeMap<&str, &store::PlannedAssetMove> = BTreeMap::new();
    let mut by_dest: BTreeMap<&str, &store::PlannedAssetMove> = BTreeMap::new();
    let mut by_page: BTreeMap<&str, &store::PlannedMarkdown> = BTreeMap::new();
    let mut by_temp: BTreeMap<&str, &store::PlannedMarkdown> = BTreeMap::new();
    let mut journaled_paths: BTreeSet<&str> = BTreeSet::new();
    for component in &transformation.components {
        for mv in &component.asset_moves {
            by_source.insert(mv.source.as_str(), mv);
            by_dest.insert(mv.dest.as_str(), mv);
            journaled_paths.insert(mv.source.as_str());
            journaled_paths.insert(mv.dest.as_str());
        }
        for md in &component.markdown {
            by_page.insert(md.page.as_str(), md);
            by_temp.insert(md.temp.as_str(), md);
            journaled_paths.insert(md.page.as_str());
            journaled_paths.insert(md.temp.as_str());
        }
    }
    // A crashed attempt creates destination ancestors on the way to a
    // journaled dest; a directory at one of those paths is the plan's own
    // in-flight state, not a foreign addition. A non-directory at the
    // same path, or a directory anywhere else, still refuses.
    let mut allowed_dirs: BTreeSet<&str> = BTreeSet::new();
    for component in &transformation.components {
        for mv in &component.asset_moves {
            let mut ancestor = mv.dest.as_str();
            while let Some((parent, _)) = ancestor.rsplit_once('/') {
                if !journaled_paths.contains(parent) {
                    allowed_dirs.insert(parent);
                }
                ancestor = parent;
            }
        }
    }
    let mut consumed: BTreeSet<&str> = BTreeSet::new();
    for (path, live_entry) in live {
        if let Some(mv) = by_dest.get(path.as_str()) {
            match live_entry {
                LiveEntry::File {
                    sha256,
                    bytes,
                    identity,
                } if *sha256 == mv.sha256 && *bytes == mv.bytes && *identity == mv.identity => {}
                _ => {
                    return Err(foreign(format!(
                        "{path} occupies a journaled destination but is not the journaled file"
                    )));
                }
            }
            consumed.insert(path.as_str());
            continue;
        }
        if let Some(mv) = by_source.get(path.as_str()) {
            // The source may still hold the original identity/bytes. Any
            // other shape — a same-bytes rewrite under a new identity, an
            // edited file, a link — is foreign: a user-restored source must
            // survive, never be removed as a "partial copy".
            match live_entry {
                LiveEntry::File {
                    sha256,
                    bytes,
                    identity,
                } if *sha256 == mv.sha256 && *bytes == mv.bytes && *identity == mv.identity => {}
                _ => {
                    return Err(foreign(format!(
                        "{path} is at a journaled move source but is not the journaled source file"
                    )));
                }
            }
            consumed.insert(path.as_str());
            continue;
        }
        if let Some(md) = by_page.get(path.as_str()) {
            let pre = matches!(
                live_entry,
                LiveEntry::File {
                    sha256,
                    bytes,
                    identity,
                } if *sha256 == md.pre_sha256 && *bytes == md.pre_bytes && *identity == md.pre_identity
            );
            let post = matches!(
                live_entry,
                LiveEntry::File { sha256, bytes, .. }
                    if *sha256 == md.post_sha256 && *bytes == md.post_bytes
            );
            if !pre && !post {
                return Err(foreign(format!(
                    "{path} is a journaled page but matches neither its pre-image nor its post-image"
                )));
            }
            consumed.insert(path.as_str());
            continue;
        }
        if let Some(md) = by_temp.get(path.as_str()) {
            let ours = match (live_entry, md.staged_identity) {
                (
                    LiveEntry::File {
                        sha256,
                        bytes,
                        identity,
                    },
                    Some(staged),
                ) => *identity == staged && *sha256 == md.post_sha256 && *bytes == md.post_bytes,
                _ => false,
            };
            if !ours {
                return Err(foreign(format!(
                    "{path} is an unexplained staged-write temporary file; it was not removed because its identity was never journaled"
                )));
            }
            consumed.insert(path.as_str());
            continue;
        }
        match inventory.get(path.as_str()) {
            Some(entry) if entry_matches(live_entry, entry) => {
                consumed.insert(path.as_str());
            }
            Some(_) => {
                return Err(foreign(format!(
                    "{path} changed after the transformation journal was written"
                )));
            }
            None => {
                if matches!(live_entry, LiveEntry::Directory)
                    && allowed_dirs.contains(path.as_str())
                {
                    continue;
                }
                return Err(foreign(format!(
                    "{path} appeared after the transformation journal was written"
                )));
            }
        }
    }
    // Missing-from-live entries: journaled sources and destinations may be
    // gone by the plan's own work; journaled pages must exist (the executor
    // never removes a page); every other inventoried entry must survive.
    for component in &transformation.components {
        for md in &component.markdown {
            if !live.contains_key(md.page.as_str()) {
                return Err(foreign(format!("journaled page {} is missing", md.page)));
            }
        }
    }
    for entry in &transformation.inventory {
        let path = entry.path();
        if consumed.contains(path) || by_dest.contains_key(path) || by_source.contains_key(path) {
            continue;
        }
        if !live.contains_key(path) {
            return Err(foreign(format!(
                "{path} disappeared after the transformation journal was written"
            )));
        }
    }
    Ok(())
}

/// Validate a journaled relative path stays inside the content input.
pub(super) fn input_rel(rel: &str, what: &str) -> Result<(), AdoptError> {
    portable_relative(rel)
        .map_err(|_| malformed(format!("{what} path {rel:?} is not portable")))?;
    Ok(())
}
