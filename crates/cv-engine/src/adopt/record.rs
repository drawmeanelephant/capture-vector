use super::*;

pub struct RecordAdoptionInput<'a> {
    pub src: &'a AdoptionSource,
    pub target_root: &'a Path,
    pub lock: &'a store::RunLock,
    pub at_ms: i64,
    pub target_kind: &'a str,
    pub subfolder_name: Option<&'a str>,
    pub planned: &'a [PlannedFile],
    pub asset_operations: Vec<store::AssetOperationProvenance>,
}

/// Write the page-level adoption sidecar and mark the run adopted.
pub fn record_adoption(input: RecordAdoptionInput<'_>) -> Result<(), AdoptError> {
    let RecordAdoptionInput {
        src,
        target_root,
        lock,
        at_ms,
        target_kind,
        subfolder_name,
        planned,
        asset_operations,
    } = input;
    let workspace = target_root.display().to_string();
    let mut rec = AdoptionRecord::new(&src.run_id, &src.mode, at_ms, &workspace, target_kind);
    rec.content_subfolder = subfolder_name.map(str::to_string);
    rec.inputs = src.record.inputs.clone();
    rec.asset_operations = asset_operations;
    let by_display: BTreeMap<_, _> = planned.iter().map(|p| (p.display.clone(), p)).collect();
    rec.pages = src
        .pages
        .iter()
        .filter_map(|p| {
            let display = join_rel(&[subfolder_name.unwrap_or(""), &p.path]);
            by_display.get(&display).map(|_| AdoptedPage {
                entity_id: p.entity_id.clone(),
                path: display,
                source: p.source.clone(),
            })
        })
        .collect();
    let event = rec.event();
    if let Some(prior) = store::scan_adoptions(target_root)?
        .into_iter()
        .find(|r| r.run_id == src.run_id)
    {
        let mut pages = prior.pages;
        for page in rec.pages {
            if !pages.contains(&page) {
                pages.push(page);
            }
        }
        rec.pages = pages;
        rec.events = prior.events;
        if rec.asset_operations.is_empty() {
            rec.asset_operations = prior.asset_operations;
        }
    }
    // Event identity is the adoption destination, not the wall-clock retry
    // that completed it. A crash after the sidecar write but before journal
    // cleanup leaves the same destination's event behind; the resumable
    // retry completes that event rather than inventing a second adoption.
    if let Some(prior_event) = rec.events.iter_mut().find(|e| {
        e.workspace == event.workspace
            && e.target == event.target
            && e.content_subfolder == event.content_subfolder
    }) {
        for page in event.pages {
            if !prior_event.pages.contains(&page) {
                prior_event.pages.push(page);
            }
        }
        prior_event.at_ms = event.at_ms;
    } else {
        rec.events.push(event);
    }
    store::write_adoption(lock, &rec)?;
    let mut run = src.record.clone();
    run.adopted = Some(store::Adopted { at_ms, workspace });
    store::write(lock, &run)?;
    Ok(())
}
