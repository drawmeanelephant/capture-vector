//! Dependency-component in-place adoption of `asset-filename` utility
//! output.
//!
//! The lab's candidate tree is evidence, not a tree to copy. Capture
//! Vector's own run-time input inventory (`input-inventory.json` inside the
//! run directory) is the authority for what the content input held when the
//! lab ran; the journal binds that inventory plus a component plan, and the
//! executor moves assets through transaction-scoped hard-link chains and
//! rewrites Markdown to candidate post-image bytes in global phases.
//!
//! Anything on disk that is neither the inventory nor the journaled plan is
//! foreign: a fresh attempt refuses it as stale evidence, a resume refuses
//! it outright. A crash between the lab run and adoption, or between two
//! adoption attempts, can therefore never silently fold user bytes into the
//! transformation.

mod evidence;
mod exec;
mod inventory;
mod plan;

use super::{
    AdoptError, AdoptionSource, ExpectedWorkspaceInstance, IdentityCheckPhase, RecordAdoptionInput,
    TxReport, content_input_authority, content_input_target_authority, copy_provenance,
    record_adoption, revalidate_workspace_instance,
};
use cv_store::imports as store;
use cv_store::paths::confined;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Per-path volume lookup the pre-journal mount check consults.
#[doc(hidden)]
pub type VolumeOf<'a> = dyn Fn(&Path) -> Option<u64> + 'a;

/// Inputs for an existing-workspace in-place asset-filename transformation.
pub struct AssetFilenameTransaction<'a> {
    pub target_root: &'a Path,
    pub src: &'a AdoptionSource,
    pub content_input: &'a str,
    pub at_ms: i64,
    pub expected_instance: &'a ExpectedWorkspaceInstance,
    /// Test seam overriding the per-path volume lookup the pre-journal
    /// mount check uses: production passes `None` and the platform's file
    /// identity answers. Tests inject a foreign volume at a mount point
    /// inside the content input.
    #[doc(hidden)]
    pub volume_of: Option<&'a VolumeOf<'a>>,
}

fn stale(message: impl Into<String>) -> AdoptError {
    AdoptError::Stale(format!(
        "asset-filename utility evidence is stale against the live workspace: {}; nothing was overwritten",
        message.into()
    ))
}

fn malformed(message: impl Into<String>) -> AdoptError {
    AdoptError::Report(format!(
        "asset-filename lab evidence is inconsistent: {}",
        message.into()
    ))
}

fn journal_error(reason: impl Into<String>) -> AdoptError {
    AdoptError::Run(format!(
        "{} is not the complete asset transformation authorized by asset_filename_manifest.json and rewrite_manifest.json: {}; the journal was left in place and this attempt recorded no adoption",
        store::PENDING,
        reason.into()
    ))
}

fn sha256_hex_shape(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn verify_run_input(
    src: &AdoptionSource,
    target_root: &Path,
    input: &str,
) -> Result<(), AdoptError> {
    let expected = target_root.join(input).canonicalize().map_err(|e| {
        AdoptError::Content(format!(
            "content input {input:?} cannot be canonicalized: {e}"
        ))
    })?;
    let matches = src
        .record
        .inputs
        .iter()
        .filter(|i| i.flag == "--root")
        .any(|i| {
            PathBuf::from(&i.path)
                .canonicalize()
                .is_ok_and(|p| p == expected)
        });
    if matches {
        Ok(())
    } else {
        Err(AdoptError::Run(format!(
            "run {} was not captured against this workspace's {input:?} content input",
            src.run_id
        )))
    }
}

fn identity_of(pair: (u64, u64)) -> store::FileIdentity {
    store::FileIdentity {
        volume: pair.0,
        index: pair.1,
    }
}

/// Everything checked and bound before the first mutation byte: the run
/// lock, the decoded evidence, the validated candidate, the recomputed
/// plan, and the journal (fresh-written or validated for resume).
struct Prepared {
    lock: store::RunLock,
    input_root: PathBuf,
    journal: store::PendingAdoption,
    evidence: evidence::Evidence,
    plan: plan::Plan,
}

enum PrepareOutcome {
    /// A prior adoption record for this run already carries an asset
    /// transformation: the operation is complete and must not repeat.
    AlreadyAdopted,
    Ready(Box<Prepared>),
}

/// Read the captured input inventory, or refuse with the run-again error
/// when the run predates the inventory contract. Schema-1 inventories are
/// refused outright: they record no directory entries, so a directory
/// appearing at a journaled destination or beneath a journaled parent is
/// invisible to every freshness and collision check — the same class of
/// post-journal deadlock the schema-2 directory entries exist to prevent.
/// The honest answer is a fresh capture, not a partial guard.
fn required_inventory(
    target_root: &Path,
    run_id: &str,
) -> Result<store::ReadInputInventory, AdoptError> {
    let read = match store::read_input_inventory(target_root, run_id).map_err(AdoptError::Report)? {
        Some(read) => read,
        None => {
            return Err(AdoptError::Run(format!(
                "run {run_id} carries no captured input inventory; the transformation cannot prove the workspace is unchanged — run the asset-filename utility again"
            )));
        }
    };
    if read.inventory.schema_version < 2 {
        return Err(AdoptError::Run(format!(
            "run {run_id} carries a schema-{} input inventory that cannot see directory changes; the transformation cannot prove the workspace is unchanged — run the asset-filename utility again",
            read.inventory.schema_version
        )));
    }
    Ok(read)
}

/// Everything up to and including the journal, before any content
/// mutation: instance revalidation, evidence, candidate, inventory, live
/// reconciliation, plan. `mutation_lock` is held by the caller for the
/// whole transaction and only borrowed here.
fn prepare(
    tx: &AssetFilenameTransaction<'_>,
    mutation_lock: &cv_store::workspace_mutation::WorkspaceMutationLock,
) -> Result<PrepareOutcome, AdoptError> {
    let input = content_input_authority(tx.content_input)?;
    content_input_target_authority(tx.target_root, input)?;
    if tx.src.mode != "asset-filename" {
        return Err(AdoptError::Run(format!(
            "run {} is mode {}, not asset-filename",
            tx.src.run_id, tx.src.mode
        )));
    }
    revalidate_workspace_instance(
        tx.target_root,
        tx.expected_instance,
        IdentityCheckPhase::BeforeWrites,
        Some(mutation_lock),
    )?;
    verify_run_input(tx.src, tx.target_root, input)?;
    let lock = super::adoption_lock(tx.target_root, &tx.src.run_id)?;

    let inventory_read = required_inventory(tx.target_root, &tx.src.run_id)?;
    // The recorded source root must be this workspace's content input.
    let expected_input = tx.target_root.join(input).canonicalize().map_err(|e| {
        AdoptError::Content(format!(
            "content input {input:?} cannot be canonicalized: {e}"
        ))
    })?;
    let recorded = PathBuf::from(&inventory_read.inventory.source_root)
        .canonicalize()
        .ok();
    if recorded.as_ref() != Some(&expected_input) {
        return Err(malformed(
            "the captured input inventory names a different content input",
        ));
    }
    let evidence = evidence::load(tx.target_root, tx.src)?;
    let candidate = evidence::candidate(tx.target_root, tx.src, &evidence)?;
    store::read_adoption_record(tx.target_root, &tx.src.run_id).map_err(AdoptError::Report)?;
    let pending =
        store::read_pending(tx.target_root, &tx.src.run_id).map_err(AdoptError::Report)?;

    let input_root = confined(tx.target_root, Path::new(input)).map_err(|e| {
        AdoptError::Content(format!(
            "content input {input:?} is not confined to the workspace: {e}"
        ))
    })?;
    let live = inventory::scan(&input_root)?;
    let run_volume = crate::file_identity::identity(lock.dir()).map(|v| v.0);
    let run_dir_rel = format!("{}/{}", store::DIR, tx.src.run_id);
    let plan = plan::build(
        &tx.src.run_id,
        &evidence,
        &inventory_read.inventory.entries,
        &candidate,
        run_volume,
        &run_dir_rel,
    )?;
    let run_volume = run_volume.ok_or_else(|| {
        AdoptError::Run(
            "this platform cannot report file identity for the run directory; the asset transformation cannot run".into(),
        )
    })?;
    // A journaled destination or staged temp beneath a foreign-volume
    // mount inside the input can never take the transformation's hard
    // links — the run's first attempt would fail EXDEV and every resume
    // would fail the same way forever. Refuse before the journal exists.
    let platform_volume = |path: &Path| crate::file_identity::identity(path).map(|i| i.0);
    let volume_of: &dyn Fn(&Path) -> Option<u64> = match tx.volume_of {
        Some(f) => f,
        None => &platform_volume,
    };
    inventory::journaled_creation_volume(&input_root, &plan.components, run_volume, volume_of)?;

    let journal = match pending {
        Some(journal) if journal.matches("existing", input, None) => {
            let Some(t) = &journal.transformation else {
                return Err(AdoptError::Run(format!(
                    "{} is not a snapshot asset-transformation journal; complete or remove it first",
                    store::PENDING
                )));
            };
            // The journal must bind the same workspace instance that is
            // mutating now — token, workspace-file identity, and the
            // mutation-lock carrier captured inside this attempt.
            let held_lock = mutation_lock.identity().map_err(|e| {
                AdoptError::Run(format!(
                    "the mutation-lock carrier identity could not be read ({e})"
                ))
            })?;
            if t.workspace_token != tx.expected_instance.token
                || t.workspace_file != tx.expected_instance.file.map(identity_of)
                || t.mutation_lock != tx.expected_instance.mutation_lock.map(identity_of)
                || t.mutation_lock != held_lock.map(identity_of)
            {
                return Err(journal_error(
                    "it binds a different workspace instance than the one holding this mutation batch",
                ));
            }
            if t.report_sha256 != evidence.report_sha256
                || t.manifest_sha256 != evidence.manifest_sha256
                || t.rewrite_sha256 != evidence.rewrite_sha256
                || t.candidate_sha256 != candidate.digest
                || t.inventory_sha256 != inventory_read.sha256
                || t.inventory != inventory_read.inventory.entries
            {
                return Err(journal_error(
                    "the lab evidence, candidate, or input inventory it binds no longer matches the recorded digests",
                ));
            }
            if plan::normalized(&t.components) != plan::normalized(&plan.components) {
                return Err(journal_error(
                    "its component plan is not the transformation the current evidence authorizes",
                ));
            }
            inventory::reconcile_resume(&live, t)?;
            journal
        }
        Some(_) => {
            return Err(AdoptError::Run(format!(
                "an interrupted adoption of {} into this workspace has a different target shape; complete that retry first or remove {}",
                tx.src.run_id,
                store::PENDING
            )));
        }
        None => {
            if let Some(prior) = store::scan_adoptions(tx.target_root)?
                .into_iter()
                .find(|r| r.run_id == tx.src.run_id)
                && !prior.asset_operations.is_empty()
            {
                return Ok(PrepareOutcome::AlreadyAdopted);
            }
            inventory::reconcile_fresh(&live, &inventory_read.inventory.entries)?;
            let t = store::PendingTransformation {
                phase: store::TransformationPhase::Planned,
                workspace_token: tx.expected_instance.token.clone(),
                workspace_file: tx.expected_instance.file.map(identity_of),
                mutation_lock: mutation_lock
                    .identity()
                    .map_err(|e| {
                        AdoptError::Run(format!(
                            "the mutation-lock carrier identity could not be read ({e})"
                        ))
                    })?
                    .map(identity_of),
                report_sha256: evidence.report_sha256.clone(),
                manifest_sha256: evidence.manifest_sha256.clone(),
                rewrite_sha256: evidence.rewrite_sha256.clone(),
                candidate_sha256: candidate.digest.clone(),
                inventory_sha256: inventory_read.sha256.clone(),
                inventory: inventory_read.inventory.entries.clone(),
                components: plan.components.clone(),
            };
            let mut journal = store::PendingAdoption::new(&tx.src.run_id, "existing", input);
            journal.transformation = Some(t);
            store::write_pending(&lock, &journal)?;
            journal
        }
    };
    Ok(PrepareOutcome::Ready(Box::new(Prepared {
        lock,
        input_root,
        journal,
        evidence,
        plan,
    })))
}

fn zero_report() -> TxReport {
    TxReport {
        planned: 0,
        written: 0,
        resumed: 0,
        provenance_copied: 0,
        asset_moves: 0,
        markdown_writes: 0,
        skipped_collisions: 0,
    }
}

fn provenance(
    evidence: &evidence::Evidence,
    plan: &plan::Plan,
    statuses: &BTreeMap<String, store::AssetOperationStatus>,
) -> Result<Vec<store::AssetOperationProvenance>, AdoptError> {
    let mut out = Vec::new();
    for asset in &evidence.manifest.assets {
        let outcome = plan
            .asset_outcomes
            .get(&asset.source_path)
            .cloned()
            .unwrap_or(plan::AssetOutcome::Rejected);
        let (status, target, reason) = match &outcome {
            plan::AssetOutcome::Unchanged => (
                store::AssetOperationStatus::Unchanged,
                asset.dest_path.clone(),
                asset.reason.clone(),
            ),
            plan::AssetOutcome::Rejected => (
                store::AssetOperationStatus::Rejected,
                asset.dest_path.clone(),
                asset.reason.clone(),
            ),
            plan::AssetOutcome::Collision(reason) => (
                store::AssetOperationStatus::SkippedCollision,
                asset.dest_path.clone(),
                reason.clone(),
            ),
            plan::AssetOutcome::Moved => (
                statuses.get(&asset.dest_path).copied().ok_or_else(|| {
                    AdoptError::Run(format!(
                        "the executor recorded no outcome for journaled destination {}",
                        asset.dest_path
                    ))
                })?,
                asset.dest_path.clone(),
                "renamed in place".to_string(),
            ),
        };
        out.push(store::AssetOperationProvenance {
            kind: store::AssetOperationKind::Asset,
            status,
            source: asset.source_path.clone(),
            target,
            reason,
            sha256: asset.sha256.clone(),
            bytes: asset.bytes,
            rewrites: 0,
        });
    }
    let mut pages: Vec<&String> = plan.page_outcomes.keys().collect();
    pages.sort();
    for page in pages {
        let outcome = plan.page_outcomes[page].clone();
        let (status, reason, sha256, bytes) = match &outcome {
            plan::PageOutcome::Collision(reason) => (
                store::AssetOperationStatus::SkippedCollision,
                format!("component blocked: {reason}"),
                None,
                0,
            ),
            plan::PageOutcome::Unchanged {
                post_sha256,
                post_bytes,
            } => (
                store::AssetOperationStatus::Unchanged,
                "the candidate page already matches the inventoried page".to_string(),
                Some(post_sha256.clone()),
                *post_bytes,
            ),
            plan::PageOutcome::Written {
                post_sha256,
                post_bytes,
            } => (
                statuses.get(page.as_str()).copied().ok_or_else(|| {
                    AdoptError::Run(format!(
                        "the executor recorded no outcome for journaled page {page}"
                    ))
                })?,
                "image_or_link_dest".to_string(),
                Some(post_sha256.clone()),
                *post_bytes,
            ),
        };
        let row_count = evidence
            .rewrites
            .rewrites
            .iter()
            .filter(|r| &r.page_path == page)
            .count() as u64;
        out.push(store::AssetOperationProvenance {
            kind: store::AssetOperationKind::Markdown,
            status,
            source: page.clone(),
            target: page.clone(),
            reason,
            sha256,
            bytes,
            rewrites: row_count,
        });
    }
    Ok(out)
}

/// Transform the workspace named by an asset-filename utility run in place.
/// Adds no page, invents no page id: the only writes are the journaled
/// asset link/unlink chains and Markdown post-image installs.
pub fn transact_existing_with_identity(
    tx: AssetFilenameTransaction<'_>,
) -> Result<TxReport, AdoptError> {
    let mutation_lock = cv_store::workspace_mutation::lock(tx.target_root).map_err(|e| {
        AdoptError::Run(format!(
            "asset transformation blocked: the workspace mutation lock is unavailable or held by a concurrent writer ({e}); nothing was written"
        ))
    })?;
    let PrepareOutcome::Ready(prepared) = prepare(&tx, &mutation_lock)? else {
        return Ok(zero_report());
    };
    let mut journal = prepared.journal;
    let ctx = exec::ExecCtx {
        target_root: tx.target_root,
        input_root: &prepared.input_root,
        lock: &prepared.lock,
        src: tx.src,
    };
    let done = (|| {
        let outcome = exec::execute(&ctx, &mut journal)?;
        let provenance_copied = copy_provenance(tx.src, tx.target_root, &prepared.lock)?;
        let rows = provenance(&prepared.evidence, &prepared.plan, &outcome.statuses)?;
        let skipped_collisions = rows
            .iter()
            .filter(|r| r.status == store::AssetOperationStatus::SkippedCollision)
            .count();
        record_adoption(RecordAdoptionInput {
            src: tx.src,
            target_root: tx.target_root,
            lock: &prepared.lock,
            at_ms: tx.at_ms,
            target_kind: "existing",
            subfolder_name: None,
            planned: &[],
            asset_operations: rows,
        })?;
        if let Some(t) = journal.transformation.as_mut() {
            t.phase = store::TransformationPhase::Recorded;
        }
        store::write_pending(&prepared.lock, &journal)?;
        Ok::<_, AdoptError>((outcome, provenance_copied, skipped_collisions))
    })();
    let (outcome, provenance_copied, skipped_collisions) = match done {
        Ok(done) => done,
        Err(e) => {
            revalidate_workspace_instance(
                tx.target_root,
                tx.expected_instance,
                IdentityCheckPhase::AfterContent,
                Some(&mutation_lock),
            )?;
            return Err(e);
        }
    };
    store::clear_pending(&prepared.lock)?;
    revalidate_workspace_instance(
        tx.target_root,
        tx.expected_instance,
        IdentityCheckPhase::AfterCommit,
        Some(&mutation_lock),
    )?;
    let move_count = prepared
        .plan
        .components
        .iter()
        .map(|c| c.asset_moves.len())
        .sum::<usize>();
    let md_count = prepared
        .plan
        .components
        .iter()
        .map(|c| c.markdown.len())
        .sum::<usize>();
    Ok(TxReport {
        planned: move_count + md_count,
        written: outcome.written,
        resumed: outcome.resumed,
        provenance_copied,
        asset_moves: move_count,
        markdown_writes: md_count,
        skipped_collisions,
    })
}

/// Journal-only seam for crash/resume tests: runs everything `transact`
/// runs up to and including the journal write (or the full resume-side
/// journal validation when a journal is already present) and returns the
/// journaled plan, so a test can put the filesystem into any mid-phase
/// state before calling [`transact_existing_with_identity`].
#[doc(hidden)]
pub fn journal_only_for_tests(
    tx: AssetFilenameTransaction<'_>,
) -> Result<store::PendingAdoption, AdoptError> {
    let mutation_lock = cv_store::workspace_mutation::lock(tx.target_root).map_err(|e| {
        AdoptError::Run(format!(
            "asset transformation blocked: the workspace mutation lock is unavailable ({e})"
        ))
    })?;
    match prepare(&tx, &mutation_lock)? {
        PrepareOutcome::AlreadyAdopted => Err(AdoptError::Run(
            "run already recorded an asset transformation".into(),
        )),
        PrepareOutcome::Ready(prepared) => Ok(prepared.journal),
    }
}
