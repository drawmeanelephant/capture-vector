//! Shared helpers for output-partition integration tests.

use cv_core::engine::{Engine, EngineCaps, EngineSource, Verb};
use cv_engine::locate::{EngineIdentity, PINNED};
use cv_engine::probe::{CapabilityReport, WatchMode};
use cv_engine::{EngineConfig, ProcessEngine};
use std::path::Path;

pub(crate) fn partition_engine(root: &Path, input: &str) -> ProcessEngine {
    let mut cfg = EngineConfig::new(root);
    cfg.content_input = input.into();
    ProcessEngine::with_report(
        cfg,
        EngineIdentity {
            source: EngineSource::Path,
            path: root.join("boris"),
            version: PINNED.version.into(),
            vcs_revision: PINNED.vcs_revision.into(),
            build_info_raw: String::new(),
        },
        CapabilityReport {
            os: std::env::consts::OS.into(),
            caps: EngineCaps {
                writes: true,
                ..Default::default()
            },
            watch_mode: WatchMode::Native,
            evidence: vec![],
        },
    )
}

pub(crate) fn custom(name: &str, args: &[&str]) -> Verb {
    Verb::Custom {
        name: name.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
    }
}

pub(crate) fn partition_error(engine: &ProcessEngine, verb: &Verb) -> Option<String> {
    engine
        .run(verb, Box::new(|_| {}))
        .err()
        .map(|e| e.to_string())
}

pub(crate) fn write_happy_profile(root: &Path, output: &str) {
    std::fs::write(
        root.join("boris.json"),
        format!(
            r#"{{
  "format": "boris-publication-profile",
  "schema_version": 1,
  "input": "content",
  "site": {{ "title": "Profile Probe" }},
  "targets": [
    {{ "name": "public", "output": "{output}", "public": true, "theme": "themes/boris" }}
  ]
}}"#
        ),
    )
    .unwrap();
}

pub(crate) fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let target = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

pub(crate) fn write_input_profile(root: &Path, input: Option<&str>, output: &str) {
    write_happy_profile(root, output);
    let path = root.join("boris.json");
    let mut text = std::fs::read_to_string(&path).unwrap();
    if let Some(input) = input {
        text = text.replace(r#""input": "content""#, &format!(r#""input": "{input}""#));
    } else {
        text = text
            .lines()
            .filter(|line| !line.contains(r#""input": "content""#))
            .collect::<Vec<_>>()
            .join("\n");
    }
    std::fs::write(path, text).unwrap();
}

pub(crate) fn write_custom_profile(root: &Path, target_json: &str) {
    std::fs::write(
        root.join("boris.json"),
        format!(
            r#"{{
  "format": "boris-publication-profile",
  "schema_version": 1,
  "input": "content",
  "site": {{ "title": "Profile Probe" }},
  "targets": [ {target_json} ]
}}"#
        ),
    )
    .unwrap();
}
