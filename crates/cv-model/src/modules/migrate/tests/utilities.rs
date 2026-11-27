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
