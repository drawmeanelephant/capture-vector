use super::*;
use crate::module::Module;

fn imports() -> Imports {
    let mut ok = ImportRun::new("20261125-hugo", RunStatus::Ok);
    ok.mode = "hugo".into();
    ok.family = Some("static-site".into());
    ok.source = "mini-hugo".into();
    ok.started_ms = 2;
    ok.counts = vec![("exact".into(), 0), ("human_review".into(), 2)];
    ok.review = vec![
        ReviewItem {
            id: "aaaa".into(),
            class: "human_review".into(),
            source: "content/en/guides/inner/_index.md".into(),
            construct: "section".into(),
            reason: "nested section flattened".into(),
            state: ReviewState::Open,
        },
        ReviewItem {
            id: "bbbb".into(),
            class: "unsupported".into(),
            source: "content/en/guides/_index.md".into(),
            construct: "branch_bundle".into(),
            reason: String::new(),
            state: ReviewState::Open,
        },
    ];
    let mut bad = ImportRun::new("20261124-jekyll", RunStatus::Ok);
    bad.mode = "jekyll".into();
    bad.family = Some("static-site".into());
    bad.source = "mini-jekyll".into();
    bad.started_ms = 1;
    bad.problem = Some("report.json schema_version 9 is not supported".into());
    Imports {
        lab: LabState::Absent {
            searched: vec!["/usr/bin/boris-migration-lab".into()],
        },
        families: vec![
            Family {
                key: "static-site".into(),
                label: "Static-site generators".into(),
            },
            Family {
                key: "social".into(),
                label: "Social/data downloads".into(),
            },
        ],
        modes: Vec::new(),
        runs: vec![bad, ok],
        error: None,
        store: "in memory".into(),
    }
}

fn module() -> MigrateModule {
    MigrateModule::new(Box::new(MemoryPort {
        imports: imports(),
        records: Vec::new(),
    }))
}

#[test]
fn it_sits_in_the_utilities_rail_group() {
    assert_eq!(module().spec().slot, Slot::Utilities);
    let r = Registry::with_all();
    let core: Vec<_> = r.iter().take(6).map(|m| m.spec().slot).collect();
    assert_eq!(core, Slot::CORE);
}

#[test]
fn number_chords_still_go_to_the_core_modules_and_skip_utilities() {
    let wb = crate::workbench::Workbench::new(Registry::with_all());
    let cmds = wb.commands();
    let chord = |id: &str| {
        cmds.iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("no {id}"))
            .chord
            .clone()
    };
    let core: Vec<_> = wb
        .registry
        .iter()
        .filter(|m| Slot::CORE.contains(&m.spec().slot))
        .map(|m| m.spec().id)
        .collect();
    assert_eq!(core.len(), 6);
    for (n, id) in core.iter().enumerate() {
        let want = format!("mod+{}", n + 1);
        assert_eq!(
            chord(&format!("shell.goto.{id}")).as_deref(),
            Some(want.as_str())
        );
    }
    assert_eq!(chord("shell.goto.migrate"), None);
}

#[test]
fn nav_opens_on_runs_then_new_import_review_and_families() {
    let m = module();
    let keys: Vec<_> = m
        .nav()
        .items
        .into_iter()
        .map(|i| (i.key, i.count))
        .collect();
    assert_eq!(
        keys,
        vec![
            ("runs".into(), Some(2)),
            ("new".into(), Some(1)),
            ("review".into(), Some(2)),
            ("family:static-site".into(), Some(2)),
            ("family:social".into(), None),
        ]
    );
}

#[test]
fn runs_list_newest_first_with_status_and_lab_counts() {
    let m = module();
    assert_eq!(m.list_len(Some("runs")), 2);
    let first = m.row(Some("runs"), 0);
    assert_eq!(first.key, "run:20261125-hugo");
    assert_eq!(first.title, "mini-hugo");
    assert_eq!(first.detail, "hugo · 2 human_review · 2 to review");
    assert_eq!(first.badge.unwrap().text, "ok");
    let second = m.row(Some("runs"), 1);
    assert_eq!(second.badge.unwrap().text, "unreadable");
    assert_eq!(second.rail, Some(Tone::Warn));
    assert_eq!(m.list_len(Some("family:social")), 0);
}

#[test]
fn review_items_triage_persists_through_the_port_and_never_blocks() {
    let mut m = module();
    assert_eq!(m.badge(), Some(2));
    let key = m.row(Some("review"), 0).key;
    m.select(Some(&key));
    m.run(ACCEPT);
    assert_eq!(m.badge(), Some(1));
    // Accepted items sort after open ones.
    assert_eq!(m.row(Some("review"), 1).key, key);
    assert_eq!(m.row(Some("review"), 1).badge.unwrap().text, "accepted");
    m.run(REFRESH);
    assert_eq!(m.badge(), Some(1), "state survives a reload from the port");
    let i = m.inspector(&key);
    assert!(i.badges.iter().any(|c| c.text == "does not block"));
}

#[test]
fn transitions_without_a_review_item_selected_only_say_so() {
    let mut m = module();
    m.select(Some("run:20261125-hugo"));
    assert_eq!(
        m.run(DEFER),
        vec![Intent::Status("select a review item first".into())]
    );
    assert_eq!(m.badge(), Some(2));
}

#[test]
fn lab_absent_is_disclosed_in_new_import_with_the_fix() {
    let m = module();
    let lab = m.row(Some("new"), 1);
    assert_eq!(lab.title, "boris-migration-lab not found");
    assert_eq!(lab.badge.unwrap().tone, Tone::Error);
    let r = m.reading("lab");
    assert!(
        r.lines
            .iter()
            .any(|l| l.text.contains("CV_MIGRATION_LAB_BIN"))
    );
}

#[test]
fn new_import_and_adopt_are_honest_disabled_entry_points() {
    let m = module();
    let cmds = m.commands();
    for id in [NEW, ADOPT] {
        let c = cmds.iter().find(|c| c.id == id).unwrap();
        assert!(c.disabled.is_some(), "{id}");
    }
}

#[test]
fn run_inspector_carries_format_argv_exit_and_paths() {
    let mut i = imports();
    let r = &mut i.runs[1];
    r.format = Some(("boris-hugo-migration-lab".into(), 1));
    r.argv = vec!["--mode=hugo".into(), "--root=/src".into()];
    r.exit_code = Some(0);
    r.out = ".cv/imports/20261125-hugo/out".into();
    let m = MigrateModule::new(Box::new(MemoryPort {
        imports: i,
        records: Vec::new(),
    }));
    let f: BTreeMap<_, _> = m
        .inspector("run:20261125-hugo")
        .fields
        .into_iter()
        .collect();
    assert_eq!(f["format"], "boris-hugo-migration-lab");
    assert_eq!(f["schema"], "1");
    assert_eq!(f["argv"], "--mode=hugo --root=/src");
    assert_eq!(f["exit code"], "0");
    assert_eq!(f["output"], ".cv/imports/20261125-hugo/out");
}

#[test]
fn every_run_and_review_item_projects_a_registered_noun() {
    let m = module();
    let rows = m.attr_rows();
    assert_eq!(rows.len(), 4);
    let kinds: Vec<_> = rows
        .iter()
        .map(|r| r.noun.kind.as_str().to_string())
        .collect();
    assert_eq!(
        kinds.iter().filter(|k| *k == noun_kind::IMPORT_RUN).count(),
        2
    );
    assert_eq!(
        kinds
            .iter()
            .filter(|k| *k == noun_kind::REVIEW_ITEM)
            .count(),
        2
    );
    for r in rows {
        assert!(!r.noun.stable.0.contains('/'), "{:?}", r.noun);
        assert_eq!(
            crate::noun_views::view_for(&r.noun.kind),
            crate::noun_views::NounView::Lane(ID)
        );
    }
}

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

fn drive_hugo_to_inventory(m: &mut MigrateModule) -> String {
    m.run(NEW);
    m.answer(FAMILY, &answer(Some("choose"), &[(CHOICE, "static-site")]));
    m.answer(MODE, &answer(Some("choose"), &[(CHOICE, "hugo")]));
    let out = m.answer(
        INPUT,
        &answer(Some("go"), &[("root", abs("elsewhere/site"))]),
    );
    import_job(&out)
        .expect("hugo starts its inventory")
        .run_id
        .clone()
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
            .any(|i| matches!(i, Intent::Status(s) if s.contains("cancelled"))),
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
    // A decoded inventory carries the count classes even when every count
    // is zero; that is the positive evidence the emit gate requires.
    done.counts = vec![
        ("exact".into(), 0),
        ("transformed".into(), 0),
        ("unsupported".into(), 0),
        ("human_review".into(), 0),
    ];
    appeared.borrow_mut().push(done);
}

#[test]
fn inventory_completion_never_emits_without_an_explicit_decision() {
    let (mut m, appeared) = dynamic_wizard_module();
    let run_id = drive_hugo_to_inventory(&mut m);
    inventory_lands(&appeared, &run_id);

    let finished = m.run(FINISHED);
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
fn choosing_emit_after_the_inventory_starts_the_emitting_mode() {
    let (mut m, appeared) = dynamic_wizard_module();
    let run_id = drive_hugo_to_inventory(&mut m);
    inventory_lands(&appeared, &run_id);
    m.run(FINISHED);
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
    let inv = import_job(&out).expect("valid extras start the inventory");
    assert_eq!(inv.mode, "hugo-inventory");
    assert!(inv.extras.is_empty(), "{:?}", inv.extras);
    // After the inventory, the explicit emit passes the extra verbatim
    // into the emitting mode's job.
    let mut done = ImportRun::new(&inv.run_id, RunStatus::Ok);
    done.mode = "hugo-inventory".into();
    done.counts = vec![("transformed".into(), 3)];
    appeared.borrow_mut().push(done);
    m.run(FINISHED);
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
    let run_id = drive_hugo_to_inventory(&mut m);
    let mut done = ImportRun::new(&run_id, RunStatus::Ok);
    done.mode = "hugo-inventory".into();
    done.problem = Some("report.json could not be decoded".into());
    appeared.borrow_mut().push(done);

    let finished = m.run(FINISHED);
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
            .any(|i| matches!(i, Intent::Status(s) if s.contains("emit is blocked"))),
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
    let run_id = drive_hugo_to_inventory(&mut m);
    // The inventory exits 0 but leaves no decodable count evidence — the
    // absent-report case. Projection may not always surface a problem, so
    // the wizard gate itself must require positive count evidence.
    let mut done = ImportRun::new(&run_id, RunStatus::Ok);
    done.mode = "hugo-inventory".into();
    appeared.borrow_mut().push(done);

    let finished = m.run(FINISHED);
    assert!(
        import_job(&finished).is_none(),
        "no emit without count evidence: {finished:?}"
    );
    assert!(
        finished
            .iter()
            .any(|i| matches!(i, Intent::Status(s) if s.contains("emit is blocked"))),
        "the block is disclosed: {finished:?}"
    );
    assert!(
        finished
            .iter()
            .any(|i| matches!(i, Intent::Status(s) if s.contains("no count data"))),
        "the missing-data reason is named: {finished:?}"
    );
}
