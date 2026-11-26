//! Round-8 regression tests for generation admission at scheduler mutations.

use super::*;
use cv_core::engine::Verb;
use cv_engine::locate::Locator;
use cv_model::modules::engine::VerbRun;

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn fake_boris(root: &Path) -> PathBuf {
    const SHIM: &str = include_str!("../../../../cv-engine/tests/fake-boris/boris");
    let bin = root.join("boris");
    const HOLD_CASE: &str = r#"    if [ -f "$D/watch-hold" ]; then"#;
    const WORKSPACE_HOLD_CASE: &str =
        r#"    if [ -f "$D/watch-hold" ] && ! printf '%s' "$*" | grep -q -- '--serve'; then"#;
    assert!(SHIM.contains(HOLD_CASE));
    install_executable(&bin, &SHIM.replace(HOLD_CASE, WORKSPACE_HOLD_CASE));
    let raw =
        std::fs::read_to_string(cv_fixtures::fixtures_root().join("engine-version.txt")).unwrap();
    let build_info = raw
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap()
        .to_string();
    let v: serde_json::Value = serde_json::from_str(&build_info).unwrap();
    write(
        &root.join("version.txt"),
        format!("{}\n", v["version"].as_str().unwrap()).as_bytes(),
    );
    write(&root.join("build-info.txt"), build_info.as_bytes());
    write(&root.join("verbs/validate/exit-code.txt"), b"0");
    write(&root.join("verbs/build/exit-code.txt"), b"0");
    write(
        &root.join("watch.ndjson"),
        b"{\"event\":\"hello\",\"watch_events_schema\":1,\"compiler\":\"boris/0.9.0\"}\n",
    );
    write(&root.join("watch-hold"), b"");
    bin
}

fn boot(root: &Path, bin: &Path, slot: EngineSlot, inbox: Inbox) -> EngineBoot {
    let mut cfg = EngineConfig::new(root);
    cfg.locator = Locator {
        env_override: Some(bin.to_path_buf()),
        bundled_dirs: Vec::new(),
        path_var: None,
        sibling_dirs: Vec::new(),
    };
    EngineBoot {
        cfg,
        slot,
        inbox,
        warning: None,
        notice: None,
        doctor: Feed::default(),
    }
}

fn workspace() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    write(
        &root.join("content/index.md"),
        b"---\ntitle: Home\n---\nHome\n",
    );
    (t, root)
}

/// Exact rapid-reopen end state when the newest boot wins the boot mutex
/// before an older boot: the older boot still performs the destructive
/// engine replacement before noticing that its shell generation is stale.
/// The generation check therefore protects verb delivery installation only;
/// it does not protect the current generation's watch.
#[cfg(unix)]
#[test]
fn r8_stale_boot_replaces_live_generation_before_refusing_install() {
    let (_tmp, root) = workspace();
    let bin = fake_boris(&root);
    let slot = EngineSlot::default();
    let wb_new = Workbench::new(Registry::with_all());
    let wb_stale = Workbench::new(Registry::with_all());

    // Triple-open generation arithmetic with no installed route yet:
    // initial generation 0 is retired once for the second open and again
    // for the third. The third open is the live generation.
    slot.retire_verbs();
    let stale_generation = slot.generation();
    slot.retire_verbs();
    let live_generation = slot.generation();
    assert_ne!(stale_generation, live_generation);

    // Permissible mutex ordering: the newest boot acquires first, installs
    // its route, publishes its watch, and only then the older boot enters.
    run(
        boot(&root, &bin, slot.clone(), wb_new.inbox()),
        live_generation,
    );
    let sched = slot
        .get()
        .expect("newest boot installed the scheduler")
        .clone();
    assert!(
        sched.watch_session().is_some(),
        "newest boot published a watch"
    );
    assert!(
        slot.route().is_some(),
        "newest boot installed verb delivery"
    );

    run(
        boot(&root, &bin, slot.clone(), wb_stale.inbox()),
        stale_generation,
    );

    let mut wb_new = wb_new;
    wb_new.drain();
    println!(
        "R8-STALE-BOOT sched_watch={} engine_watch_live={} ui_watch_live={} degraded={:?} route_current={}",
        sched.watch_session().is_some(),
        sched.engine().watch_live(),
        wb_new.app.watch_live,
        wb_new.app.degraded,
        slot.route().is_some()
    );
    assert!(
        sched.watch_session().is_some(),
        "STALE BOOT DESTROYED THE LIVE GENERATION: replacement ran before the generation check and stopped the current watch"
    );
    assert!(wb_new.app.watch_live);
    assert!(wb_new.app.degraded.is_none());
    if let Some(w) = sched.watch_session() {
        sched.stop_watch_session(&w);
    }
}

/// Exact verb end state when a build accepted before retirement reaches
/// the scheduler only after the reopened generation has published its
/// watch. The stale route declines to restore, but the build has already
/// taken the new watch's seat: the live workspace is left watch-dead.
#[cfg(unix)]
#[test]
fn r8_stale_verb_after_replacement_stops_current_watch_without_restore() {
    let (_tmp, root) = workspace();
    let bin = fake_boris(&root);
    let slot = EngineSlot::default();
    let wb_live = Workbench::new(Registry::with_all());

    // Install a live generation and clone its route, as `start_verb` does
    // before spawning the worker.
    run(
        boot(&root, &bin, slot.clone(), wb_live.inbox()),
        slot.generation(),
    );
    let sched = slot.get().expect("boot installed the scheduler").clone();
    let stale_route = slot.route().expect("first generation route");
    assert!(sched.watch_session().is_some());

    // Reopen retires the route. A successful replacement then installs a
    // fresh route and publishes the current generation's watch.
    slot.retire_verbs();
    let replacement_generation = slot.generation();
    let wb_replacement = Workbench::new(Registry::with_all());
    run(
        boot(&root, &bin, slot.clone(), wb_replacement.inbox()),
        replacement_generation,
    );
    assert!(sched.watch_session().is_some(), "replacement watch is live");
    let replacement_session = sched.watch_session().unwrap();

    // This is the worker body of the pre-retirement `start_verb`, delayed
    // by the operating system until after replacement publication. Nothing
    // in the production lock ordering makes that delay impossible.
    stale_route.run(
        &sched,
        VerbRun {
            verb: Verb::Build,
            cancel: cv_core::engine::CancelToken::new(),
        },
    );

    let mut wb_replacement = wb_replacement;
    wb_replacement.drain();
    println!(
        "R8-STALE-VERB stopped_session={} sched_watch={} engine_watch_live={} ui_watch_live={} degraded={:?} route_current={}",
        sched.watch_session().as_ref() == Some(&replacement_session),
        sched.watch_session().is_some(),
        sched.engine().watch_live(),
        wb_replacement.app.watch_live,
        wb_replacement.app.degraded,
        slot.route().is_some()
    );
    assert!(
        sched.watch_session().is_some(),
        "STALE VERB DESTROYED THE LIVE GENERATION: it stopped the replacement watch and its dead-generation route correctly refused restoration"
    );
    assert!(wb_replacement.app.watch_live);
    assert!(wb_replacement.app.degraded.is_none());
    if let Some(w) = sched.watch_session() {
        sched.stop_watch_session(&w);
    }
}
