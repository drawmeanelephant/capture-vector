use super::*;

#[test]
fn workspace_utility_commands_start_utility_phase_jobs() {
    for (command, mode) in [
        (LINK_AUDIT, "link-audit"),
        (FRONTMATTER_REVIEW, "frontmatter-review"),
        (ASSET_FILENAME, "asset-filename"),
    ] {
        let intents = module().run(command);
        let jobs: Vec<_> = intents
            .iter()
            .filter_map(|i| match i {
                Intent::RunImport(j) => Some(j),
                _ => None,
            })
            .collect();
        assert_eq!(jobs.len(), 1, "{command} emits a job");
        assert_eq!(jobs[0].mode, mode);
        assert_eq!(jobs[0].phase, ImportPhase::Utility);
        assert!(jobs[0].inputs.is_empty());
    }
}

#[test]
fn asset_filename_existing_choice_requests_in_place_transformation_without_a_subfolder() {
    let mut i = imports();
    let run = i.runs.iter_mut().find(|r| r.id == "20261125-hugo").unwrap();
    run.mode = "asset-filename".into();
    let mut m = MigrateModule::new(Box::new(MemoryPort {
        imports: i,
        records: Vec::new(),
    }));
    m.select(Some("run:20261125-hugo"));
    let Some(Intent::Dialog(spec)) = m.run(ADOPT).into_iter().next() else {
        panic!("chooser");
    };
    let DialogKind::Choose(choices) = &spec.kind else {
        panic!("chooser kind");
    };
    assert_eq!(choices[1].label, "This workspace (rename assets in place)");
    let mut a = blank_answer();
    a.context = spec.context.clone();
    a.action = Some("continue".into());
    a.values.insert(
        crate::dialog::CHOICE.into(),
        crate::dialog::Value::Choice("existing".into()),
    );
    let intents = m.answer(ADOPT, &a);
    let Some(Intent::AdoptRun(req)) = intents.clone().into_iter().next() else {
        panic!("asset-filename existing adoption must not open the subfolder form: {intents:?}");
    };
    assert_eq!(
        req.target,
        crate::adoption::AdoptionTarget::ExistingWorkspaceInPlace
    );
}
