//! Shell-generation ownership tests for verb delivery.

use super::*;

#[test]
fn a_retired_boot_generation_cannot_resurrect_its_delivery() {
    let root = tempfile::tempdir().unwrap();
    let wb = Workbench::new(Registry::with_all());
    let delivery = test_delivery(root.path(), WatchMode::Native, &wb);
    let slot = EngineSlot::default();
    let boot_generation = slot.generation();

    slot.retire_verbs();

    assert!(
        !slot.install_verbs(slot.verb_route(delivery.clone(), None), boot_generation),
        "a stale boot must not install the dead shell generation it finished in"
    );
    assert!(slot.route().is_none());

    let reopened_generation = slot.generation();
    assert!(slot.install_verbs(slot.verb_route(delivery, None), reopened_generation));
    assert!(slot.route().is_some());
}
