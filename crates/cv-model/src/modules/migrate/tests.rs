use super::*;
use crate::module::Module;
use std::collections::BTreeMap;

fn answer() -> DialogAnswer {
    DialogAnswer {
        action: None,
        context: None,
        values: BTreeMap::new(),
        secret: None,
    }
}

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
fn adopt_is_enabled_for_a_finished_run_and_offers_new_or_existing() {
    let mut m = module();
    m.select(Some("run:20261125-hugo"));
    let cmds = m.commands();
    let c = cmds.iter().find(|c| c.id == ADOPT).unwrap();
    assert!(c.disabled.is_none(), "adopt must enable for a finished run");
    let intents = m.run(ADOPT);
    let Some(Intent::Dialog(spec)) = intents.into_iter().next() else {
        panic!("adopt opens the target chooser");
    };
    assert_eq!(spec.id, ADOPT);
    let DialogKind::Choose(choices) = &spec.kind else {
        panic!("chooser kind");
    };
    assert_eq!(choices[0].id, "new");
    assert_eq!(choices[1].id, "existing");
}

#[test]
fn adopt_stays_disabled_for_a_running_run() {
    let mut i = imports();
    for r in &mut i.runs {
        r.status = RunStatus::Running;
    }
    let mut m = MigrateModule::new(Box::new(MemoryPort {
        imports: i,
        records: Vec::new(),
    }));
    m.select(Some("run:20261125-hugo"));
    let c = m.commands().into_iter().find(|c| c.id == ADOPT).unwrap();
    assert!(c.disabled.is_some());
    assert_eq!(
        m.run(ADOPT),
        vec![Intent::Status(
            "Adopt: run 20261125-hugo has not finished successfully".into(),
        )]
    );
}

#[test]
fn answering_existing_collects_a_subfolder_and_emits_the_request() {
    let mut m = module();
    m.select(Some("run:20261125-hugo"));
    let intents = m.run(ADOPT);
    let Some(Intent::Dialog(spec)) = intents.into_iter().next() else {
        panic!("chooser");
    };
    let mut a = answer();
    a.context = spec.context.clone();
    a.action = Some("continue".into());
    a.values.insert(
        crate::dialog::CHOICE.into(),
        crate::dialog::Value::Choice("existing".into()),
    );
    let intents = m.answer(ADOPT, &a);
    let Some(Intent::Dialog(form)) = intents.into_iter().next() else {
        panic!("existing opens the subfolder form");
    };
    assert_eq!(form.id, ADOPT_EXISTING);

    // An unsafe subfolder is refused in the form, not at the writer.
    let mut bad = answer();
    bad.context = form.context.clone();
    bad.action = Some("adopt".into());
    bad.values.insert(
        "subfolder".into(),
        crate::dialog::Value::Text("../escape".into()),
    );
    let retry = m.answer(ADOPT_EXISTING, &bad);
    assert!(
        matches!(retry.first(), Some(Intent::Dialog(_))),
        "{retry:?}"
    );

    // An absolute path is refused, never silently rewritten to a
    // relative one — the form must come back with an error instead of
    // emitting the request.
    for absolute in ["/abs/tree", "\\abs\\tree", " leading-space"] {
        let mut abs = answer();
        abs.context = form.context.clone();
        abs.action = Some("adopt".into());
        abs.values.insert(
            "subfolder".into(),
            crate::dialog::Value::Text(absolute.into()),
        );
        let retry = m.answer(ADOPT_EXISTING, &abs);
        assert!(
            matches!(retry.first(), Some(Intent::Dialog(_))),
            "{absolute:?} must be refused in the form, got {retry:?}"
        );
        assert!(
            !retry.iter().any(|i| matches!(i, Intent::AdoptRun(_))),
            "{absolute:?} must never reach the writer"
        );
    }

    let mut ok = answer();
    ok.context = form.context.clone();
    ok.action = Some("adopt".into());
    ok.values.insert(
        "subfolder".into(),
        crate::dialog::Value::Text("imported/site".into()),
    );
    let intents = m.answer(ADOPT_EXISTING, &ok);
    let mut it = intents.into_iter();
    let Some(Intent::AdoptRun(req)) = it.next() else {
        panic!("the validated form must emit the adoption request");
    };
    assert_eq!(req.run_id, "20261125-hugo");
    assert_eq!(
        req.target,
        crate::adoption::AdoptionTarget::ExistingWorkspace {
            subfolder: "imported/site".into()
        }
    );
}

#[test]
fn choosing_new_prepares_the_workspace_form_with_the_mode_archetype() {
    let mut i = imports();
    i.runs
        .iter_mut()
        .find(|r| r.id == "20261125-hugo")
        .unwrap()
        .suggested_archetype = Some("blog".into());
    let mut m = MigrateModule::new(Box::new(MemoryPort {
        imports: i,
        records: Vec::new(),
    }));
    m.select(Some("run:20261125-hugo"));
    let intents = m.run(ADOPT);
    let Some(Intent::Dialog(spec)) = intents.into_iter().next() else {
        panic!("chooser");
    };
    let mut a = answer();
    a.context = spec.context.clone();
    a.action = Some("continue".into());
    a.values.insert(
        crate::dialog::CHOICE.into(),
        crate::dialog::Value::Choice("new".into()),
    );
    let intents = m.answer(ADOPT, &a);
    assert_eq!(
        intents,
        vec![Intent::PrepareAdoptNew {
            run_id: "20261125-hugo".into(),
            suggested_archetype: Some("blog".into()),
        }]
    );
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
