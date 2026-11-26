//! Path boundary: forged ledger records, run ids and copy destinations
//! must never reach files outside the project or inside the `.cv/` sidecar.

mod support;

use cv_core::id::StableId;
use cv_core::publish::{PreflightGrant, RunState};
use cv_core::sys::IdGen;
use cv_ledger::Journal;
use cv_ledger::journal::Entry;
use cv_publish::adapters::{
    COPY_TO_FOLDER, CopyToFolder, Delivered, Delivery, DeliveryAdapter, TransportFailure,
};
use cv_publish::plan::{PlanFile, SealedPlan, hash_json, load_sealed};
use cv_publish::preflight::{run_dir, site_dir};
use cv_publish::secrets::NoSecrets;
use cv_publish::{PreflightRequest, PublishError, Publisher, Ruleset, Target, TargetConfig};
use std::path::{Path, PathBuf};
use support::*;

/// Yields one fixed id, as a compromised or buggy `IdGen` port could.
struct FixedId(&'static str);

impl IdGen for FixedId {
    fn next(&mut self) -> StableId {
        StableId(self.0.into())
    }
}

/// Append a self-consistent grant + approval for `run`, the way a full
/// rewrite of `.cv/ledger/ledger.jsonl` would. Every seal is recomputed
/// from the forged plan, so only path validation can refuse it.
fn forge_approved_grant(root: &Path, run: &StableId, plan: &SealedPlan, cfg: &TargetConfig) {
    let grant = PreflightGrant {
        artifact_set_hash: plan.artifact_set_hash().unwrap(),
        graph_hash: cv_publish::plan::sha256(b""),
        plan_hash: plan.hash().unwrap(),
        target_cfg_hash: hash_json(cfg).unwrap(),
        ruleset_hash: Rules::ok().hash(),
        boris_version: "boris/0.9.0".into(),
    };
    let mut journal = Journal::open(root).unwrap();
    journal
        .append(
            CLOCK.0,
            Entry::GrantIssued {
                run: run.clone(),
                target: plan.target.clone(),
                grant: grant.clone(),
                plan: serde_json::to_value(plan).unwrap(),
                target_cfg: serde_json::to_value(cfg).unwrap(),
            },
        )
        .unwrap();
    journal
        .append(
            CLOCK.0,
            Entry::Approved {
                run: run.clone(),
                by: "forger".into(),
                grant_digest: hash_json(&grant).unwrap(),
            },
        )
        .unwrap();
}

/// `<tmp>/project` plus a sibling `<tmp>/victim/secret.txt` outside it.
fn project_with_outside_victim() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    let victim = tmp.path().join("victim");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&victim).unwrap();
    std::fs::write(victim.join("secret.txt"), "outside the project").unwrap();
    (tmp, project, victim)
}

fn forged_plan(site: &str, destination: &str, files: Vec<PlanFile>) -> SealedPlan {
    SealedPlan {
        target: Target::StaticDist.id().into(),
        adapter: COPY_TO_FOLDER.into(),
        destination: destination.into(),
        site: site.into(),
        files,
    }
}

#[test]
fn preflight_refuses_a_traversal_run_id_without_deleting_anything() {
    let (tmp, project, victim) = project_with_outside_victim();
    std::fs::write(project.join("keep.md"), "project file").unwrap();
    let eng = FakeEngine::new(&project);
    let mut p = Publisher::open(&project, &eng, &CLOCK, &NoSecrets).unwrap();

    let out = p.preflight(
        &PreflightRequest { config: cfg() },
        &Rules::ok(),
        &mut FixedId("../.."),
    );

    assert!(matches!(out, Err(PublishError::Refused(_))), "{out:?}");
    assert!(eng.verbs().is_empty());
    assert_eq!(
        std::fs::read_to_string(project.join("keep.md")).unwrap(),
        "project file"
    );
    assert!(victim.join("secret.txt").is_file());
    assert!(Journal::path_for(&project).is_file());
    assert!(!project.join(".cv/publish").exists());
    drop(tmp);
}

#[test]
fn run_ids_refuse_traversal_absolute_and_drive_forms() {
    for id in [
        "",
        ".",
        "..",
        "../..",
        "/tmp",
        r"..\..",
        "C:",
        r"C:\x",
        r"\\server\share",
        "run:stream",
    ] {
        let run = StableId(id.into());
        assert!(
            matches!(run_dir(&run), Err(PublishError::Refused(_))),
            "{id:?}"
        );
    }
}

#[test]
fn load_sealed_refuses_plan_paths_that_escape_the_site() {
    let tmp = tempfile::tempdir().unwrap();
    let site = tmp.path().join("site");
    std::fs::create_dir_all(&site).unwrap();
    let outside = tmp.path().join("outside.txt");
    std::fs::write(&outside, "outside").unwrap();
    for path in [
        "../outside.txt".to_string(),
        "nested/../../outside.txt".into(),
        outside.to_str().unwrap().into(),
        "".into(),
    ] {
        let plan = forged_plan(
            "site",
            "deployed",
            vec![PlanFile {
                kind: None,
                advertised: None,
                path: path.clone(),
                bytes: 7,
                sha256: cv_publish::plan::sha256(b"outside"),
            }],
        );
        assert!(
            matches!(load_sealed(&site, &plan), Err(PublishError::Refused(_))),
            "{path:?}"
        );
    }
}

#[test]
fn publish_refuses_a_forged_plan_site_outside_the_run_directory() {
    let (_tmp, project, victim) = project_with_outside_victim();
    let run = StableId("run-forged".into());
    let plan = forged_plan(
        "../victim",
        &cfg().destination,
        cv_publish::plan::inventory(&victim).unwrap(),
    );
    forge_approved_grant(&project, &run, &plan, &cfg());
    let eng = FakeEngine::new(&project);
    let mut p = Publisher::open(&project, &eng, &CLOCK, &NoSecrets)
        .unwrap()
        .with_adapter(CopyToFolder::new(&project));

    let out = p.publish(&run, &cfg(), &Rules::ok());

    assert!(matches!(out, Err(PublishError::Refused(_))), "{out:?}");
    assert!(p.run(&run).unwrap().history.is_empty());
    assert!(!project.join("deployed").exists(), "nothing exfiltrated");
}

#[test]
fn publish_refuses_a_forged_traversal_run_id() {
    let (_tmp, project, _victim) = project_with_outside_victim();
    let site = project.join("site");
    std::fs::create_dir_all(&site).unwrap();
    std::fs::write(site.join("index.html"), "<h1>forged</h1>").unwrap();
    let run = StableId("../..".into());
    let plan = forged_plan(
        "site",
        &cfg().destination,
        cv_publish::plan::inventory(&site).unwrap(),
    );
    forge_approved_grant(&project, &run, &plan, &cfg());
    let eng = FakeEngine::new(&project);
    let mut p = Publisher::open(&project, &eng, &CLOCK, &NoSecrets)
        .unwrap()
        .with_adapter(CopyToFolder::new(&project));

    let out = p.publish(&run, &cfg(), &Rules::ok());

    assert!(matches!(out, Err(PublishError::Refused(_))), "{out:?}");
    assert!(p.run(&run).unwrap().history.is_empty());
    assert!(!project.join("deployed").exists());
}

#[test]
fn reconcile_by_probe_refuses_a_forged_destination_outside_the_project() {
    let (_tmp, project, victim) = project_with_outside_victim();
    let run = StableId("run-forged".into());
    let mut forged_cfg = cfg();
    forged_cfg.destination = "../victim".into();
    let plan = forged_plan(
        &site_dir(&run).unwrap(),
        &forged_cfg.destination,
        cv_publish::plan::inventory(&victim).unwrap(),
    );
    forge_approved_grant(&project, &run, &plan, &forged_cfg);
    Journal::open(&project)
        .unwrap()
        .append(
            CLOCK.0,
            Entry::RunState {
                run: run.clone(),
                state: RunState::Unknown,
                note: None,
            },
        )
        .unwrap();
    let eng = FakeEngine::new(&project);
    let mut p = Publisher::open(&project, &eng, &CLOCK, &NoSecrets)
        .unwrap()
        .with_adapter(CopyToFolder::new(&project));

    let out = p.reconcile_by_probe(&run);

    assert!(matches!(out, Err(PublishError::Refused(_))), "{out:?}");
    let v = p.run(&run).unwrap();
    assert_eq!(v.state, Some(RunState::Unknown));
    assert!(v.reconciliations.is_empty(), "no oracle over outside files");
}

fn deliver_one(root: &Path, destination: &str, path: &str) -> Result<Delivered, TransportFailure> {
    let files = vec![(
        PlanFile {
            kind: None,
            advertised: None,
            path: path.into(),
            bytes: 2,
            sha256: cv_publish::plan::sha256(b"hi"),
        },
        b"hi".to_vec(),
    )];
    let mut config = cfg();
    config.destination = destination.into();
    let plan = forged_plan("site", destination, vec![files[0].0.clone()]);
    let eng = FakeEngine::new(root);
    CopyToFolder::new(root).deliver(&Delivery {
        run: &StableId("run-x".into()),
        plan: &plan,
        files: &files,
        site: root,
        config: &config,
        secrets: &NoSecrets,
        engine: &eng,
    })
}

#[test]
fn copy_to_folder_refuses_plan_paths_that_escape_the_destination() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let absolute = tmp.path().join("absolute.html");
    for path in [
        "../escape.html".to_string(),
        "../../escape.html".into(),
        absolute.to_str().unwrap().into(),
    ] {
        let out = deliver_one(&project, "out", &path);
        assert!(out.is_err(), "{path:?} unexpectedly delivered: {out:?}");
    }
    assert!(!project.join("out").exists(), "refused before contact");
    // Windows-syntax paths are host grammar, not escapes: on unix they are
    // ordinary (if odd) file names confined inside the destination; on
    // Windows `Path::components` parses the prefix/separators and the
    // host-relative grammar refuses them there.
    for path in [r"C:\escape.html", r"\\server\share\escape.html"] {
        let out = deliver_one(&project, "out", path);
        #[cfg(unix)]
        {
            assert!(out.is_ok(), "{path:?} is host-valid on unix: {out:?}");
            assert!(project.join("out").join(path).exists());
            std::fs::remove_file(project.join("out").join(path)).unwrap();
        }
        #[cfg(windows)]
        assert!(out.is_err(), "{path:?} must be refused on windows");
    }
    assert!(!project.join("escape.html").exists());
    assert!(!tmp.path().join("escape.html").exists());
    assert!(!absolute.exists());
}

#[test]
fn copy_to_folder_refuses_destinations_inside_the_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let ledger = dir.path().join(".cv/ledger");
    std::fs::create_dir_all(&ledger).unwrap();
    std::fs::write(ledger.join("ledger.jsonl"), "evidence\n").unwrap();
    for destination in [".cv/ledger", ".CV/ledger", ".cv"] {
        let out = deliver_one(dir.path(), destination, "ledger.jsonl");
        assert!(!out.unwrap_err().contacted, "{destination:?}");
    }
    assert_eq!(
        std::fs::read_to_string(ledger.join("ledger.jsonl")).unwrap(),
        "evidence\n"
    );
}

#[test]
fn preflight_refuses_a_copy_destination_inside_the_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let eng = FakeEngine::new(dir.path());
    let mut p = Publisher::open(dir.path(), &eng, &CLOCK, &NoSecrets).unwrap();
    let mut config = cfg();
    config.destination = ".cv/publish".into();

    let out = p.preflight(&PreflightRequest { config }, &Rules::ok(), &mut ids("run"));

    assert!(matches!(out, Err(PublishError::Refused(_))), "{out:?}");
    assert!(eng.verbs().is_empty());
}
