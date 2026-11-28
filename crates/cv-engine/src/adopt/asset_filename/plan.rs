//! Transformation planning: validate the lab evidence against the captured
//! input inventory, compute destination-collision groups, and group
//! operations into dependency components.
//!
//! Nodes are rewritten assets and Markdown pages carrying rewrite rows;
//! edges are the rewrite rows themselves. A connected component applies
//! coherently or stays untouched: if any rewrite inside a page's component
//! is collision-blocked, every operation in that component is blocked — a
//! collision-contaminated page can never end up referencing an asset that
//! moved, nor can the move outlive its page.
//!
//! Unchanged and rejected rows produce no executable operations but still
//! bind the plan: their evidence is validated against the inventory, and
//! their destinations participate in collision-group membership.

use super::evidence::{Candidate, Evidence};
use super::inventory::input_rel;
use super::{AdoptError, malformed, sha256_hex_shape};
use cv_store::imports as store;
use std::collections::{BTreeMap, BTreeSet};

/// How the plan disposes of one manifest asset row, for provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AssetOutcome {
    Unchanged,
    Rejected,
    /// Collision-blocked rewritten row; the payload names the reason.
    Collision(String),
    Moved,
}

/// How the plan disposes of one page that carries rewrite rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PageOutcome {
    Written {
        post_sha256: String,
        post_bytes: u64,
    },
    /// Transitively blocked because a member asset is collision-bound.
    Collision(String),
}

pub(super) struct Plan {
    /// Applied components, in deterministic order. This exact list is what
    /// the journal binds and what a resume recomputes and compares.
    pub components: Vec<store::PlannedComponent>,
    /// Manifest source path → disposition, in manifest order elsewhere.
    pub asset_outcomes: BTreeMap<String, AssetOutcome>,
    /// Rewrite-page path → disposition.
    pub page_outcomes: BTreeMap<String, PageOutcome>,
}

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }
    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.parent[b] = a;
        }
    }
}

/// Build the deterministic plan. `inventory` is the captured run-time
/// input inventory (already proven equal to the live tree on a fresh
/// attempt, or carried inside the journal on resume); `run_volume` is the
/// device identity of the run directory the backup links will live in —
/// `None` means the platform cannot report file identity and the
/// transformation must refuse before any content mutation.
pub(super) fn build(
    run_id: &str,
    evidence: &Evidence,
    inventory: &[store::InputEntry],
    candidate: &Candidate,
    run_volume: Option<u64>,
    run_dir_rel: &str,
) -> Result<Plan, AdoptError> {
    let by_path: BTreeMap<&str, &store::InputEntry> =
        inventory.iter().map(|e| (e.path(), e)).collect();
    let by_lower: BTreeMap<String, &str> = by_path.keys().map(|p| (p.to_lowercase(), *p)).collect();

    // Rewrite destinations an accepted (rewritten) asset row claims, for
    // rewrite-row validation.
    let rewritten_dests: BTreeMap<&str, usize> = evidence
        .manifest
        .assets
        .iter()
        .enumerate()
        .filter(|(_, a)| a.action == "rewritten" && !a.dest_path.is_empty())
        .map(|(i, a)| (a.dest_path.as_str(), i))
        .collect();

    // Collision groups: every manifest row's nonempty destination claims a
    // destination shape. Rewritten, unchanged, and rejected rows all count —
    // a rejected row that already occupies its destination blocks a rewrite
    // converging on the same case-insensitive name.
    let mut dest_group: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    for asset in &evidence.manifest.assets {
        if !asset.dest_path.is_empty() {
            dest_group
                .entry(asset.dest_path.to_lowercase())
                .or_default()
                .insert(asset.source_path.as_str());
        }
    }

    let mut asset_outcomes = BTreeMap::new();
    let mut blocked: BTreeMap<usize, String> = BTreeMap::new();

    for (index, asset) in evidence.manifest.assets.iter().enumerate() {
        input_rel(&asset.source_path, "asset source")?;
        if !asset.dest_path.is_empty() {
            input_rel(&asset.dest_path, "asset destination")?;
        }
        let source_entry = by_path.get(asset.source_path.as_str()).ok_or_else(|| {
            malformed(format!(
                "manifest source {} is absent from the captured input inventory",
                asset.source_path
            ))
        })?;
        match (asset.reason.as_str(), asset.sha256.as_ref(), source_entry) {
            ("symlink", None, store::InputEntry::Symlink { .. }) => {}
            (reason, Some(sha), store::InputEntry::File { sha256, bytes, .. }) => {
                if sha != sha256 || asset.bytes != *bytes {
                    return Err(malformed(format!(
                        "manifest asset {} disagrees with the captured input inventory ({reason})",
                        asset.source_path
                    )));
                }
            }
            _ => {
                return Err(malformed(format!(
                    "manifest asset {} has a different shape in the captured input inventory",
                    asset.source_path
                )));
            }
        }
        match asset.action.as_str() {
            "unchanged" => {
                if asset.dest_path != asset.source_path {
                    return Err(malformed(format!(
                        "unchanged asset {} names a different destination {}",
                        asset.source_path, asset.dest_path
                    )));
                }
                asset_outcomes.insert(asset.source_path.clone(), AssetOutcome::Unchanged);
            }
            "rejected" => {
                asset_outcomes.insert(asset.source_path.clone(), AssetOutcome::Rejected);
            }
            "rewritten" => {
                if asset.dest_path.is_empty() || asset.dest_path == asset.source_path {
                    return Err(malformed(format!(
                        "rewritten asset {} has no real destination change",
                        asset.source_path
                    )));
                }
                let key = asset.dest_path.to_lowercase();
                let reason = if let Some(occupant) = by_lower.get(&key) {
                    if *occupant == asset.source_path {
                        format!(
                            "destination {} aliases its own source on this filesystem",
                            asset.dest_path
                        )
                    } else {
                        format!(
                            "destination {} is already occupied by {occupant}",
                            asset.dest_path
                        )
                    }
                } else if dest_group.get(&key).is_some_and(|s| s.len() > 1) {
                    let others: Vec<&str> = dest_group[&key]
                        .iter()
                        .copied()
                        .filter(|s| *s != asset.source_path)
                        .collect();
                    format!(
                        "destination {} collides with {}",
                        asset.dest_path,
                        others.join(", ")
                    )
                } else {
                    asset_outcomes.insert(asset.source_path.clone(), AssetOutcome::Moved);
                    continue;
                };
                blocked.insert(index, reason.clone());
                asset_outcomes.insert(asset.source_path.clone(), AssetOutcome::Collision(reason));
            }
            action => {
                return Err(malformed(format!("unknown asset action {action:?}")));
            }
        }
    }

    // Rewrite rows must reference existing candidate pages and accepted
    // rewritten destinations. Rows for pages or assets the lab never
    // emitted are evidence corruption, not defaults.
    let mut pages_with_rows: BTreeMap<String, Vec<&cv_contracts::lab::AssetFilenameRewrite>> =
        BTreeMap::new();
    for row in &evidence.rewrites.rewrites {
        input_rel(&row.page_path, "rewrite page")?;
        input_rel(&row.rewritten_dest, "rewritten destination")?;
        input_rel(&row.original_dest, "original destination")?;
        if !candidate.pages.contains_key(row.page_path.as_str()) {
            return Err(malformed(format!(
                "rewrite row names page {} which is absent from the lab candidate",
                row.page_path
            )));
        }
        if !rewritten_dests.contains_key(row.rewritten_dest.as_str()) {
            return Err(malformed(format!(
                "rewrite destination {} has no rewritten asset row",
                row.rewritten_dest
            )));
        }
        // The original destination is the reference spelling inside the
        // page at lab time — legitimately percent-encoded or otherwise
        // rewritten — so it is intentionally *not* required to be a
        // manifest source. It drives no operation: the candidate page's
        // post-image bytes are authoritative, not substitution logic.
        pages_with_rows
            .entry(row.page_path.clone())
            .or_default()
            .push(row);
    }
    // A path cannot be both a rewrite page and an asset endpoint: the two
    // roles have disjoint ownership semantics.
    for asset in &evidence.manifest.assets {
        for rel in [&asset.source_path, &asset.dest_path] {
            if !rel.is_empty() && pages_with_rows.contains_key(rel) {
                return Err(malformed(format!(
                    "{rel} is named both as an asset endpoint and as a rewrite page"
                )));
            }
        }
    }
    for page in pages_with_rows.keys() {
        if candidate.accepted_assets.contains(page) {
            return Err(malformed(format!(
                "{page} is both a rewrite page and an accepted asset destination"
            )));
        }
    }

    // No-rewrite pages must be bit-identical between inventory and
    // candidate: a page the lab rewrote nothing in cannot legitimately
    // differ.
    for (page, post) in &candidate.pages {
        if pages_with_rows.contains_key(page) {
            continue;
        }
        match by_path.get(page.as_str()) {
            Some(store::InputEntry::File { sha256, bytes, .. })
                if *sha256 == post.sha256 && *bytes == post.bytes => {}
            _ => {
                return Err(malformed(format!(
                    "candidate page {page} differs from the inventoried page but carries no rewrite rows"
                )));
            }
        }
    }

    // Dependency components: rewritten assets and rewrite pages, edges
    // through rewrite rows. Assets 0..n are manifest indices of rewritten
    // rows; pages n.. are page paths.
    let rewritten: Vec<usize> = evidence
        .manifest
        .assets
        .iter()
        .enumerate()
        .filter(|(_, a)| a.action == "rewritten")
        .map(|(i, _)| i)
        .collect();
    let asset_node: BTreeMap<usize, usize> = rewritten
        .iter()
        .enumerate()
        .map(|(node, &index)| (index, node))
        .collect();
    let page_node: BTreeMap<String, usize> = pages_with_rows
        .keys()
        .enumerate()
        .map(|(node, page)| (page.clone(), rewritten.len() + node))
        .collect();
    let mut uf = UnionFind::new(rewritten.len() + page_node.len());
    for row in &evidence.rewrites.rewrites {
        let asset_index = rewritten_dests[row.rewritten_dest.as_str()];
        let Some(&an) = asset_node.get(&asset_index) else {
            continue;
        };
        let Some(&pn) = page_node.get(row.page_path.as_str()) else {
            continue;
        };
        uf.union(an, pn);
    }

    // Group nodes by component root.
    let mut members: BTreeMap<usize, (Vec<usize>, Vec<String>)> = BTreeMap::new();
    for (&asset_index, &node) in &asset_node {
        let root = uf.find(node);
        members.entry(root).or_default().0.push(asset_index);
    }
    for (page, &node) in &page_node {
        let root = uf.find(node);
        members.entry(root).or_default().1.push(page.clone());
    }

    // A component is blocked when any member asset is collision-bound.
    let mut components: Vec<(BTreeSet<String>, Vec<usize>, Vec<String>)> = Vec::new();
    for (_root, (assets, pages)) in members {
        let key: BTreeSet<String> = assets
            .iter()
            .map(|&i| evidence.manifest.assets[i].source_path.clone())
            .chain(pages.iter().cloned())
            .collect();
        components.push((key, assets, pages));
    }
    components.sort_by(|a, b| a.0.iter().next().cmp(&b.0.iter().next()));

    let run_volume = run_volume.ok_or_else(|| {
        AdoptError::Run(
            "this platform cannot report file identity for the run directory; the asset transformation cannot run".into(),
        )
    })?;
    let mut page_outcomes = BTreeMap::new();
    let mut planned: Vec<store::PlannedComponent> = Vec::new();
    let mut move_index = 0usize;
    for (_key, assets, pages) in components {
        let blocking = assets.iter().find_map(|i| blocked.get(i)).cloned();
        if let Some(reason) = blocking {
            for page in pages {
                page_outcomes.insert(page, PageOutcome::Collision(reason.clone()));
            }
            continue;
        }
        let mut moves = Vec::new();
        for &index in &assets {
            let asset = &evidence.manifest.assets[index];
            let store::InputEntry::File { identity, .. } = by_path[asset.source_path.as_str()]
            else {
                return Err(malformed(format!(
                    "move source {} is not an inventoried file",
                    asset.source_path
                )));
            };
            if identity.volume != run_volume {
                return Err(AdoptError::Run(format!(
                    "asset {} lives on a different volume than the run directory; hard-link backup is impossible",
                    asset.source_path
                )));
            }
            let Some(sha) = asset.sha256.clone() else {
                return Err(malformed(format!(
                    "rewritten asset {} has no sha256 evidence",
                    asset.source_path
                )));
            };
            if !sha256_hex_shape(&sha) {
                return Err(malformed(format!(
                    "rewritten asset {} carries malformed sha256 evidence",
                    asset.source_path
                )));
            }
            moves.push(store::PlannedAssetMove {
                source: asset.source_path.clone(),
                dest: asset.dest_path.clone(),
                backup: format!("{run_dir_rel}/asset-backup/{move_index:03}"),
                sha256: sha,
                bytes: asset.bytes,
                identity: *identity,
            });
            move_index += 1;
        }
        let mut markdown = Vec::new();
        for page in pages {
            let Some(store::InputEntry::File {
                sha256: pre_sha,
                bytes: pre_bytes,
                identity: pre_identity,
                ..
            }) = by_path.get(page.as_str()).copied()
            else {
                return Err(malformed(format!(
                    "rewrite page {page} is not an inventoried file"
                )));
            };
            let post = candidate.pages.get(page.as_str()).ok_or_else(|| {
                malformed(format!("candidate page evidence for {page} is missing"))
            })?;
            if pre_sha == &post.sha256 && pre_bytes == &post.bytes {
                // A rewrite-page whose candidate equals its pre-image carries
                // no effective write; skip it but keep it coherent inside the
                // component (no operation needed).
                page_outcomes.insert(
                    page.clone(),
                    PageOutcome::Written {
                        post_sha256: post.sha256.clone(),
                        post_bytes: post.bytes,
                    },
                );
                continue;
            }
            let temp = format!("{page}.cv-adopt-{run_id}.tmp");
            input_rel(&temp, "staged-write temporary")?;
            markdown.push(store::PlannedMarkdown {
                page: page.clone(),
                temp,
                pre_sha256: pre_sha.clone(),
                pre_bytes: *pre_bytes,
                pre_identity: *pre_identity,
                post_sha256: post.sha256.clone(),
                post_bytes: post.bytes,
                staged_identity: None,
            });
            page_outcomes.insert(
                page.clone(),
                PageOutcome::Written {
                    post_sha256: post.sha256.clone(),
                    post_bytes: post.bytes,
                },
            );
        }
        planned.push(store::PlannedComponent {
            asset_moves: moves,
            markdown,
        });
    }
    Ok(Plan {
        components: planned,
        asset_outcomes,
        page_outcomes,
    })
}

/// The journaled components stripped of in-phase `staged_identity` marks —
/// the deterministic shape a resume recomputes and compares.
pub(super) fn normalized(components: &[store::PlannedComponent]) -> Vec<store::PlannedComponent> {
    components
        .iter()
        .map(|c| store::PlannedComponent {
            asset_moves: c.asset_moves.clone(),
            markdown: c
                .markdown
                .iter()
                .map(|m| store::PlannedMarkdown {
                    staged_identity: None,
                    ..m.clone()
                })
                .collect(),
        })
        .collect()
}
