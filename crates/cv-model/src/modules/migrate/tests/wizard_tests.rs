use super::*;

// ---- New import wizard state machine ----

fn wizard_modes() -> Vec<ImportMode> {
    vec![
        ImportMode {
            name: "wordpress".into(),
            family: "blog-cms".into(),
            description: "Converts a WordPress WXR export".into(),
            inputs: vec![
                ImportInput {
                    flag: "--wxr".into(),
                    label: "wxr file".into(),
                    kind: ImportInputKind::File,
                    required: true,
                    extensions: vec!["xml".into()],
                },
                ImportInput {
                    flag: "--media".into(),
                    label: "media folder".into(),
                    kind: ImportInputKind::Folder,
                    required: false,
                    extensions: Vec::new(),
                },
            ],
            inventory: None,
            safety: vec!["Inputs are read-only".into()],
            extras: Vec::new(),
        },
        ImportMode {
            name: "hugo-inventory".into(),
            family: "static-site".into(),
            description: "Reads a Hugo project root without converting it".into(),
            inputs: vec![ImportInput {
                flag: "--root".into(),
                label: "root folder".into(),
                kind: ImportInputKind::Folder,
                required: true,
                extensions: Vec::new(),
            }],
            inventory: None,
            safety: vec![],
            extras: Vec::new(),
        },
        ImportMode {
            name: "hugo".into(),
            family: "static-site".into(),
            description: "Converts a Hugo project root".into(),
            inputs: vec![ImportInput {
                flag: "--root".into(),
                label: "root folder".into(),
                kind: ImportInputKind::Folder,
                required: true,
                extensions: Vec::new(),
            }],
            inventory: Some("hugo-inventory".into()),
            safety: vec![],
            extras: vec![ImportExtra {
                flag: "--max-pages".into(),
                label: "max pages (--max-pages)".into(),
                hint: "cap converted pages".into(),
                kind: ImportExtraKind::Count,
            }],
        },
    ]
}

fn wizard_module_with(runs: Vec<ImportRun>) -> MigrateModule {
    let mut i = imports();
    i.lab = LabState::Located {
        tool_id: "lab".into(),
        source: "test".into(),
        path: "/lab".into(),
    };
    i.families.push(Family {
        key: "blog-cms".into(),
        label: "Blog/CMS exports".into(),
    });
    i.modes = wizard_modes();
    i.runs = runs;
    MigrateModule::new(Box::new(MemoryPort {
        imports: i,
        records: Vec::new(),
    }))
}

fn wizard_module() -> MigrateModule {
    wizard_module_with(Vec::new())
}

/// A port whose runs can grow after construction, the way `.cv/imports/`
/// does while the shell-owned worker executes the lab.
struct WizardPort {
    imports: Imports,
    appeared: std::rc::Rc<std::cell::RefCell<Vec<ImportRun>>>,
}

impl MigratePort for WizardPort {
    fn load(&mut self) -> Imports {
        let mut i = self.imports.clone();
        i.runs.extend(self.appeared.borrow().iter().cloned());
        i
    }
    fn record(&mut self, _run: &str, _item: &str, _to: ReviewState) -> Result<(), String> {
        Ok(())
    }
}

fn dynamic_wizard_module() -> (
    MigrateModule,
    std::rc::Rc<std::cell::RefCell<Vec<ImportRun>>>,
) {
    let mut i = imports();
    i.lab = LabState::Located {
        tool_id: "lab".into(),
        source: "test".into(),
        path: "/lab".into(),
    };
    i.families.push(Family {
        key: "blog-cms".into(),
        label: "Blog/CMS exports".into(),
    });
    i.modes = wizard_modes();
    i.runs = Vec::new();
    let appeared = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let port = WizardPort {
        imports: i,
        appeared: appeared.clone(),
    };
    (MigrateModule::new(Box::new(port)), appeared)
}

fn drive_hugo_to_inventory_job(m: &mut MigrateModule) -> crate::command::ImportJob {
    m.run(NEW);
    m.answer(FAMILY, &answer(Some("choose"), &[(CHOICE, "static-site")]));
    m.answer(MODE, &answer(Some("choose"), &[(CHOICE, "hugo")]));
    let out = m.answer(
        INPUT,
        &answer(Some("go"), &[("root", abs("elsewhere/site"))]),
    );
    import_job(&out).expect("hugo starts its inventory").clone()
}

fn drive_hugo_to_inventory(m: &mut MigrateModule) -> String {
    drive_hugo_to_inventory_job(m).run_id
}

use crate::dialog::{CHOICE, DialogAnswer, Value};

/// Platform-absolute fixture spelling, leaked for `&'static` convenience in
/// tests: the wizard correctly rejects Unix-style `/...` input on Windows,
/// so fixture paths must be absolute on the host actually running the test.
/// `C:/...` is absolute on Windows; bytes (including trailing whitespace)
/// are otherwise preserved verbatim.
fn abs(rel: &'static str) -> &'static str {
    let s = if cfg!(windows) {
        format!("C:/{rel}")
    } else {
        format!("/{rel}")
    };
    Box::leak(s.into_boxed_str())
}

fn answer(action: Option<&str>, values: &[(&str, &str)]) -> DialogAnswer {
    let mut a = DialogAnswer {
        action: action.map(str::to_string),
        ..Default::default()
    };
    for (k, v) in values {
        a.values.insert((*k).into(), Value::Text((*v).into()));
    }
    a
}

fn dialog_id(intents: &[Intent]) -> Option<&str> {
    intents.iter().find_map(|i| match i {
        Intent::Dialog(d) => Some(d.id.as_str()),
        _ => None,
    })
}

fn import_job(intents: &[Intent]) -> Option<&crate::command::ImportJob> {
    intents.iter().find_map(|i| match i {
        Intent::RunImport(job) => Some(job),
        _ => None,
    })
}

use wizard::{FAMILY, INPUT, INVENTORY, MODE};

fn to_wordpress_form(m: &mut MigrateModule) {
    m.run(NEW);
    m.answer(FAMILY, &answer(Some("choose"), &[(CHOICE, "blog-cms")]));
    m.answer(MODE, &answer(Some("choose"), &[(CHOICE, "wordpress")]));
}

#[test]
fn cancelling_the_wizard_resets_it_so_a_new_import_can_start() {
    let mut m = wizard_module();
    m.run(NEW);
    m.answer(FAMILY, &answer(Some("choose"), &[(CHOICE, "blog-cms")]));
    assert!(m.wizard.family.is_some());
    let out = m.answer(MODE, &answer(None, &[]));
    assert!(
        out.iter()
            .any(|i| matches!(i, Intent::Outcome(s) if s.contains("cancelled"))),
        "{out:?}"
    );
    assert_eq!(m.wizard, wizard::Wizard::default());
    // A fresh start is a dialog again, not a stuck state.
    assert_eq!(dialog_id(&m.run(NEW)), Some(FAMILY));
}

#[test]
fn back_from_mode_selection_returns_to_the_family_step() {
    let mut m = wizard_module();
    m.run(NEW);
    m.answer(FAMILY, &answer(Some("choose"), &[(CHOICE, "blog-cms")]));
    let out = m.answer(MODE, &answer(Some("back"), &[]));
    assert_eq!(dialog_id(&out), Some(FAMILY));
    // The family step still lists every family, not only the one left.
    let Some(Intent::Dialog(d)) = out.iter().find(|i| matches!(i, Intent::Dialog(_))) else {
        panic!("expected a dialog, got {out:?}");
    };
    let crate::dialog::DialogKind::Choose(choices) = &d.kind else {
        panic!("family step is a choice dialog");
    };
    assert!(choices.iter().any(|c| c.id == "static-site"));
    assert!(choices.iter().any(|c| c.id == "blog-cms"));
}

#[test]
fn a_missing_required_input_reopens_the_form_with_typed_values_kept() {
    let mut m = wizard_module();
    to_wordpress_form(&mut m);
    let out = m.answer(
        INPUT,
        &answer(
            Some("go"),
            &[("wxr", ""), ("media", abs("elsewhere/media"))],
        ),
    );
    let Some(Intent::Dialog(d)) = out.iter().find(|i| matches!(i, Intent::Dialog(_))) else {
        panic!("expected the form again, got {out:?}");
    };
    assert_eq!(d.id, INPUT);
    assert!(d.error.is_some(), "the refusal is shown in the form");
    assert!(import_job(&out).is_none(), "nothing runs without --wxr");
    assert_eq!(
        m.wizard.inputs.get("--media").map(String::as_str),
        Some(abs("elsewhere/media")),
        "the typed optional input survives validation"
    );
    let crate::dialog::DialogKind::Form(fields) = &d.kind else {
        panic!("input step is a form");
    };
    let media = fields.iter().find(|f| f.id == "media").unwrap();
    assert_eq!(
        media.value(),
        Value::Path(abs("elsewhere/media").into()),
        "the form is prefilled"
    );
}

#[test]
fn a_valid_file_input_starts_the_emit_job_without_an_inventory() {
    let mut m = wizard_module();
    to_wordpress_form(&mut m);
    let out = m.answer(
        INPUT,
        &answer(Some("go"), &[("wxr", abs("elsewhere/export.xml"))]),
    );
    let job = import_job(&out).expect("a valid form starts the import");
    assert_eq!(job.mode, "wordpress");
    assert_eq!(job.phase, crate::command::ImportPhase::Emit);
    assert_eq!(
        job.inputs,
        vec![(
            "--wxr".into(),
            std::path::PathBuf::from(abs("elsewhere/export.xml"))
        )]
    );
    assert!(
        out.iter()
            .any(|i| matches!(i, Intent::Dialog(d) if d.id == wizard::PROGRESS)),
        "progress is shown while the lab runs"
    );
}

#[test]
fn hugo_starts_its_read_only_inventory_companion_first() {
    let mut m = wizard_module();
    m.run(NEW);
    m.answer(FAMILY, &answer(Some("choose"), &[(CHOICE, "static-site")]));
    m.answer(MODE, &answer(Some("choose"), &[(CHOICE, "hugo")]));
    let out = m.answer(
        INPUT,
        &answer(Some("go"), &[("root", abs("elsewhere/site"))]),
    );
    let job = import_job(&out).expect("hugo starts a job");
    assert_eq!(job.mode, "hugo-inventory");
    assert_eq!(job.phase, crate::command::ImportPhase::Inventory);
}

fn inventory_lands(appeared: &std::rc::Rc<std::cell::RefCell<Vec<ImportRun>>>, run_id: &str) {
    let mut done = ImportRun::new(run_id, RunStatus::Ok);
    done.mode = "hugo-inventory".into();
    // A decoded Hugo inventory carries count evidence even when every
    // count is zero; that is the positive evidence the emit gate requires.
    done.counts = vec![
        ("exact".into(), 0),
        ("transformed".into(), 0),
        ("unsupported".into(), 0),
        ("human_review".into(), 0),
    ];
    done.count_evidence = true;
    appeared.borrow_mut().push(done);
}

#[test]
fn inventory_completion_never_emits_without_an_explicit_decision() {
    let (mut m, appeared) = dynamic_wizard_module();
    let job = drive_hugo_to_inventory_job(&mut m);
    inventory_lands(&appeared, &job.run_id);

    let finished = m.import_finished(&job);
    assert!(
        import_job(&finished).is_none(),
        "finishing the inventory must not emit on its own: {finished:?}"
    );
    assert_eq!(dialog_id(&finished), Some(INVENTORY));

    // "Stop here" keeps the inventory and emits nothing.
    let stop = m.answer(INVENTORY, &answer(None, &[]));
    assert!(import_job(&stop).is_none());
    assert_eq!(m.wizard, wizard::Wizard::default());
}

#[test]
fn collision_resolved_start_retargets_the_running_wizard() {
    let (mut m, _appeared) = dynamic_wizard_module();
    let requested = drive_hugo_to_inventory(&mut m);
    let cancel = match &m.wizard.stage {
        wizard::WizardStage::Running { cancel, .. } => cancel.clone(),
        _ => panic!("inventory is running"),
    };
    let actual = crate::command::ImportJob {
        workspace: std::path::PathBuf::from("/workspace"),
        run_id: "import-2-hugo-inventory".into(),
        mode: "hugo-inventory".into(),
        phase: crate::command::ImportPhase::Inventory,
        inputs: Vec::new(),
        extras: Vec::new(),
        cancel,
        operation: crate::status::StatusOperation::mint(),
    };
    m.retarget_started_import(&requested, &actual);
    match &m.wizard.stage {
        wizard::WizardStage::Running { run_id, .. } => {
            assert_eq!(run_id, "import-2-hugo-inventory")
        }
        _ => panic!("wizard remains running"),
    }
}

#[test]
fn a_collision_notice_for_a_different_operation_never_retargets_the_wizard() {
    let (mut m, _appeared) = dynamic_wizard_module();
    let requested = drive_hugo_to_inventory(&mut m);
    // The production race: another operation's worker collided on the same
    // candidate id this wizard staged, claimed a later id, and published its
    // retargeting notice. The requested run-id text matches by collision,
    // but the cancellation token belongs to that other operation — the
    // notice must leave this wizard bound to its own candidate, because the
    // id alone never discriminates two operations that staged it.
    let foreign = crate::command::ImportJob {
        workspace: std::path::PathBuf::from("/workspace"),
        run_id: "import-2-hugo-inventory".into(),
        mode: "hugo-inventory".into(),
        phase: crate::command::ImportPhase::Inventory,
        inputs: Vec::new(),
        extras: Vec::new(),
        cancel: cv_core::engine::CancelToken::new(),
        operation: crate::status::StatusOperation::mint(),
    };
    m.retarget_started_import(&requested, &foreign);
    match &m.wizard.stage {
        wizard::WizardStage::Running { run_id, .. } => assert_eq!(
            run_id, &requested,
            "a different operation's start notice must not rebind this wizard"
        ),
        _ => panic!("wizard remains running"),
    }
}

#[test]
fn internal_companion_and_out_of_stage_answers_are_rejected() {
    let mut m = wizard_module();
    // A deep MODE answer before the wizard is at that step is a no-op.
    assert!(
        m.answer(MODE, &answer(Some("choose"), &[(CHOICE, "hugo-inventory")]))
            .is_empty()
    );

    m.run(NEW);
    m.answer(FAMILY, &answer(Some("choose"), &[(CHOICE, "static-site")]));
    let out = m.answer(MODE, &answer(Some("choose"), &[(CHOICE, "hugo-inventory")]));
    assert!(
        import_job(&out).is_none(),
        "internal companion never starts"
    );
    assert_eq!(
        dialog_id(&out),
        Some(MODE),
        "caller is returned to offered choices"
    );
    assert!(m.wizard.mode.is_none());
}

#[test]
fn stale_inventory_emit_answer_does_not_start_concurrent_emit() {
    let (mut m, _appeared) = dynamic_wizard_module();
    let _inventory = drive_hugo_to_inventory(&mut m);
    assert!(matches!(
        m.wizard.stage,
        wizard::WizardStage::Running { .. }
    ));
    let out = m.answer(INVENTORY, &answer(Some("emit"), &[]));
    assert!(
        out.is_empty(),
        "stale/deep inventory answer is ignored: {out:?}"
    );
    assert!(matches!(
        m.wizard.stage,
        wizard::WizardStage::Running { .. }
    ));
}

#[test]
fn choosing_emit_after_the_inventory_starts_the_emitting_mode() {
    let (mut m, appeared) = dynamic_wizard_module();
    let inventory = drive_hugo_to_inventory_job(&mut m);
    inventory_lands(&appeared, &inventory.run_id);
    m.import_finished(&inventory);
    let out = m.answer(INVENTORY, &answer(Some("emit"), &[]));
    let job = import_job(&out).expect("an explicit emit starts the import");
    assert_eq!(job.mode, "hugo");
    assert_eq!(job.phase, crate::command::ImportPhase::Emit);
    assert_eq!(
        job.inputs,
        vec![(
            "--root".into(),
            std::path::PathBuf::from(abs("elsewhere/site"))
        )],
        "the same source paths are used"
    );
}

#[test]
fn verified_emit_completion_is_an_outcome_first_product_result() {
    let (mut m, appeared) = dynamic_wizard_module();
    let inventory = drive_hugo_to_inventory_job(&mut m);
    inventory_lands(&appeared, &inventory.run_id);
    m.import_finished(&inventory);
    let out = m.answer(INVENTORY, &answer(Some("emit"), &[]));
    let job = import_job(&out).expect("emit starts").clone();

    let mut done = ImportRun::new(&job.run_id, RunStatus::Ok);
    done.mode = "hugo".into();
    appeared.borrow_mut().push(done);
    let finished = m.import_finished(&job);

    assert!(
        finished
            .iter()
            .any(|i| matches!(i, Intent::Outcome(s) if s == "Import finished; the run is in Runs")),
        "the completion leads with the product result: {finished:?}"
    );
    assert!(
        finished.iter().any(|i| matches!(
            i,
            Intent::Reveal { module, key, .. } if module == super::ID && key == &format!("run:{}", job.run_id)
        )),
        "the run remains inspectable: {finished:?}"
    );
}

#[test]
fn a_zero_exit_emit_without_output_evidence_is_never_finished() {
    let (mut m, appeared) = dynamic_wizard_module();
    let inventory = drive_hugo_to_inventory_job(&mut m);
    inventory_lands(&appeared, &inventory.run_id);
    m.import_finished(&inventory);
    let out = m.answer(INVENTORY, &answer(Some("emit"), &[]));
    let job = import_job(&out).expect("emit starts").clone();
    // The lab exits zero but leaves no readable report: projection records
    // a problem on an otherwise Ok run.
    let mut done = ImportRun::new(&job.run_id, RunStatus::Ok);
    done.mode = "hugo".into();
    done.problem = Some("the lab reported success but the output contains no report.json".into());
    appeared.borrow_mut().push(done);
    let finished = m.import_finished(&job);
    let text = format!("{finished:?}");
    assert!(
        text.contains("could not be verified"),
        "zero-exit without evidence is a problem state: {text}"
    );
    assert!(
        !text.contains("Import finished"),
        "never labeled finished: {text}"
    );
}

#[test]
fn inventory_companions_are_not_offered_as_emitting_modes() {
    let mut m = wizard_module();
    m.run(NEW);
    let out = m.answer(FAMILY, &answer(Some("choose"), &[(CHOICE, "static-site")]));
    let Some(Intent::Dialog(d)) = out.iter().find(|i| matches!(i, Intent::Dialog(_))) else {
        panic!("expected the mode step, got {out:?}");
    };
    let crate::dialog::DialogKind::Choose(choices) = &d.kind else {
        panic!("mode step is a choice dialog");
    };
    assert!(choices.iter().any(|c| c.id == "hugo"));
    assert!(
        !choices.iter().any(|c| c.id == "hugo-inventory"),
        "the read-only companion runs through hugo, never directly"
    );
}

#[test]
fn progress_cancellation_cancels_the_shared_token() {
    let mut m = wizard_module();
    to_wordpress_form(&mut m);
    m.answer(
        INPUT,
        &answer(Some("go"), &[("wxr", abs("elsewhere/export.xml"))]),
    );
    let wizard::WizardStage::Running { cancel, .. } = &m.wizard.stage else {
        panic!("the job is running");
    };
    let cancel = cancel.clone();
    assert!(!cancel.is_cancelled());
    m.answer(wizard::PROGRESS, &answer(None, &[]));
    assert!(cancel.is_cancelled(), "escape cancels the lab's token");
}

#[test]
fn paths_with_trailing_whitespace_are_preserved_verbatim() {
    let mut m = wizard_module();
    to_wordpress_form(&mut m);
    // `/tmp/export.xml ` (trailing space) is a different file from
    // `/tmp/export.xml`; trimming it would corrupt the user's pick.
    let out = m.answer(
        INPUT,
        &answer(Some("go"), &[("wxr", abs("tmp/export.xml "))]),
    );
    let job = import_job(&out).expect("a valid form starts the import");
    let (_, path) = job.inputs.iter().find(|(f, _)| f == "--wxr").unwrap();
    assert_eq!(
        path,
        &std::path::PathBuf::from(abs("tmp/export.xml ")),
        "the spelling reaches the job verbatim"
    );
}

#[test]
fn whitespace_only_input_is_rejected_not_trimmed_to_absent() {
    let mut m = wizard_module();
    to_wordpress_form(&mut m);
    let out = m.answer(INPUT, &answer(Some("go"), &[("wxr", "   ")]));
    let Some(Intent::Dialog(d)) = out.iter().find(|i| matches!(i, Intent::Dialog(_))) else {
        panic!("expected the form again, got {out:?}");
    };
    assert!(
        d.error.as_deref().unwrap_or("").contains("whitespace"),
        "{:?}",
        d.error
    );
    assert!(import_job(&out).is_none(), "whitespace-only never runs");
}

#[test]
fn clearing_an_optional_input_stays_cleared_after_a_failed_submit() {
    let mut m = wizard_module();
    to_wordpress_form(&mut m);
    // First submit: valid wxr plus an optional --media pick.
    let out = m.answer(
        INPUT,
        &answer(
            Some("go"),
            &[
                ("wxr", abs("elsewhere/export.xml")),
                ("media", abs("media-one")),
            ],
        ),
    );
    assert!(import_job(&out).is_some(), "first submit starts");

    // Fresh wizard walk; same picks, then media is cleared while the
    // required wxr is ALSO emptied, so validation fails.
    let mut m = wizard_module();
    to_wordpress_form(&mut m);
    let _ = m.answer(
        INPUT,
        &answer(
            Some("go"),
            &[
                ("wxr", abs("elsewhere/export.xml")),
                ("media", abs("media-one")),
            ],
        ),
    );
    // Keep the white-box fixture at the input step after the first valid
    // submit; production cannot submit input while the worker is Running.
    m.wizard.stage = wizard::WizardStage::Input;
    // validation failure on resubmit with media cleared
    let failed = m.answer(INPUT, &answer(Some("go"), &[("wxr", ""), ("media", "")]));
    assert!(
        import_job(&failed).is_none(),
        "missing wxr fails validation"
    );
    assert_eq!(
        m.wizard.inputs.get("--media"),
        None,
        "cleared media must stay cleared even when validation fails"
    );
    // Now fix only the wxr and submit: media must NOT resurrect.
    let out = m.answer(
        INPUT,
        &answer(Some("go"), &[("wxr", abs("elsewhere/export.xml"))]),
    );
    let job = import_job(&out).expect("fixed submit starts");
    assert!(
        !job.inputs.iter().any(|(f, _)| f == "--media"),
        "cleared --media must not resurrect: {:?}",
        job.inputs
    );
}

#[test]
fn count_extras_are_type_checked_and_passed_through() {
    let (mut m, appeared) = dynamic_wizard_module();
    m.run(NEW);
    m.answer(FAMILY, &answer(Some("choose"), &[(CHOICE, "static-site")]));
    m.answer(MODE, &answer(Some("choose"), &[(CHOICE, "hugo")]));
    // Non-numeric --max-pages rejected at the boundary.
    let bad = m.answer(
        INPUT,
        &answer(Some("go"), &[("root", abs("site")), ("max-pages", "abc")]),
    );
    let Some(Intent::Dialog(d)) = bad.iter().find(|i| matches!(i, Intent::Dialog(_))) else {
        panic!("expected the form again, got {bad:?}");
    };
    assert!(
        d.error
            .as_deref()
            .unwrap_or("")
            .contains("positive integer"),
        "{:?}",
        d.error
    );
    assert!(import_job(&bad).is_none());
    // Numeric input starts the inventory companion, which declares no
    // extras of its own — scalar options follow the mode actually running.
    let out = m.answer(
        INPUT,
        &answer(Some("go"), &[("root", abs("site")), ("max-pages", "12")]),
    );
    let inv = import_job(&out)
        .expect("valid extras start the inventory")
        .clone();
    assert_eq!(inv.mode, "hugo-inventory");
    assert!(inv.extras.is_empty(), "{:?}", inv.extras);
    // After the inventory, the explicit emit passes the extra verbatim
    // into the emitting mode's job.
    let mut done = ImportRun::new(&inv.run_id, RunStatus::Ok);
    done.mode = "hugo-inventory".into();
    done.counts = vec![("transformed".into(), 3)];
    done.count_evidence = true;
    appeared.borrow_mut().push(done);
    m.import_finished(&inv);
    let out = m.answer(INVENTORY, &answer(Some("emit"), &[]));
    let job = import_job(&out).expect("an explicit emit starts the import");
    assert_eq!(job.mode, "hugo");
    assert_eq!(
        job.extras,
        vec![("--max-pages".to_string(), "12".to_string())]
    );
}

#[test]
fn inventory_with_unreadable_report_blocks_emit() {
    let (mut m, appeared) = dynamic_wizard_module();
    let job = drive_hugo_to_inventory_job(&mut m);
    let mut done = ImportRun::new(&job.run_id, RunStatus::Ok);
    done.mode = "hugo-inventory".into();
    done.problem = Some("report.json could not be decoded".into());
    appeared.borrow_mut().push(done);

    let finished = m.import_finished(&job);
    assert!(
        import_job(&finished).is_none(),
        "no emit without counts: {finished:?}"
    );
    assert_ne!(
        dialog_id(&finished),
        Some(INVENTORY),
        "the emit decision must not be offered when counts are unavailable"
    );
    assert!(
        finished
            .iter()
            .any(|i| matches!(i, Intent::Outcome(s) if s.contains("emit is blocked"))),
        "the block is disclosed: {finished:?}"
    );
    assert_eq!(
        m.wizard,
        wizard::Wizard::default(),
        "the wizard resets rather than sitting in a dead end"
    );
}

#[test]
fn inventory_with_no_decoded_counts_blocks_emit_even_without_a_problem() {
    let (mut m, appeared) = dynamic_wizard_module();
    let job = drive_hugo_to_inventory_job(&mut m);
    // The inventory exits 0 but leaves no decodable count evidence — the
    // absent-report case. Projection may not always surface a problem, so
    // the wizard gate itself must require positive count evidence.
    let mut done = ImportRun::new(&job.run_id, RunStatus::Ok);
    done.mode = "hugo-inventory".into();
    appeared.borrow_mut().push(done);

    let finished = m.import_finished(&job);
    assert!(
        import_job(&finished).is_none(),
        "no emit without count evidence: {finished:?}"
    );
    assert!(
        finished
            .iter()
            .any(|i| matches!(i, Intent::Outcome(s) if s.contains("emit is blocked"))),
        "the block is disclosed: {finished:?}"
    );
    assert!(
        finished
            .iter()
            .any(|i| matches!(i, Intent::Outcome(s) if s.contains("no count data"))),
        "the missing-data reason is named: {finished:?}"
    );
}
