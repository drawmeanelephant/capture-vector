//! The entire cv-contracts suite: decode real captures only.
//! Hand-authored Boris JSON is banned — these read `fixtures/` directly.

use cv_contracts::{build_info::BuildInfo, graph::Graph, proof::*, watch};
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn corpora() -> Vec<PathBuf> {
    let mut v: Vec<_> = std::fs::read_dir(fixtures().join("captures"))
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    v.sort();
    v
}

#[test]
fn engine_version_decodes() {
    let raw = std::fs::read_to_string(fixtures().join("engine-version.txt")).unwrap();
    let info: BuildInfo = serde_json::from_str(&raw).unwrap();
    assert!(info.is_boris());
    assert_eq!(info.version, "boris/0.9.0");
    assert_eq!(info.vcs_revision, "66efcfb");
}

#[test]
fn every_watch_session_decodes_line_by_line() {
    let mut sessions = 0;
    let mut lines = 0;
    for corpus in corpora() {
        let session = corpus.join("watch/session.ndjson");
        if !session.exists() {
            continue;
        }
        sessions += 1;
        let mut saw_hello = false;
        for line in std::fs::read_to_string(&session).unwrap().lines() {
            let ev = watch::decode_line(line).unwrap().unwrap();
            lines += 1;
            if let watch::WatchEvent::Hello {
                watch_events_schema,
                ..
            } = ev
            {
                assert_eq!(watch_events_schema, 1, "hello must carry schema 1");
                saw_hello = true;
            }
        }
        assert!(saw_hello, "{session:?} lacks a hello record");
    }
    assert!(
        sessions >= 9,
        "expected the 9 captured sessions, got {sessions}"
    );
    assert!(
        lines > sessions,
        "sessions should carry events beyond hello"
    );
}

#[test]
fn proof_family_decodes() {
    let proof = fixtures().join("captures/happy/dist/dist/_boris/proof");
    let artifacts: ArtifactIndex =
        serde_json::from_str(&std::fs::read_to_string(proof.join("artifacts.json")).unwrap())
            .unwrap();
    assert_eq!(artifacts.format, "boris-publication-artifacts");
    assert_eq!(artifacts.artifacts.len(), 5);

    let checks: ChecksFile =
        serde_json::from_str(&std::fs::read_to_string(proof.join("checks.json")).unwrap()).unwrap();
    assert_eq!(checks.checks.len(), 3);
    assert!(checks.checks.iter().all(|c| c.status == "passed"));

    for name in ["claims.json", "touches.json", "proof-pack.json"] {
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(proof.join(name)).unwrap()).unwrap();
        assert_eq!(v["schema_version"], 1, "{name} schema_version");
    }
    let pack: ProofPack =
        serde_json::from_str(&std::fs::read_to_string(proof.join("proof-pack.json")).unwrap())
            .unwrap();
    assert_eq!(pack.format, "boris-publication-proof-pack");

    let idx: SearchIndex = serde_json::from_str(
        &std::fs::read_to_string(proof.join("../search/search-index.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(idx.format, "boris-rendered-search-index");
    assert!(!idx.documents.is_empty());
}
#[test]
fn every_captured_ir_graph_decodes() {
    let mut graphs = 0;
    for dir in ["editor/captures", "research/captures"] {
        for corpus in std::fs::read_dir(fixtures().join(dir)).unwrap() {
            let corpus = corpus.unwrap().path();
            for rel in ["graph.json", "build-ir/graph.json"] {
                let path = corpus.join(rel);
                let Ok(raw) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let g: Graph = serde_json::from_str(&raw)
                    .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                assert!(!g.nodes.is_empty(), "{}", path.display());
                graphs += 1;
            }
        }
    }
    assert!(graphs >= 4, "only {graphs} IR graphs captured");
    let quoted: Graph = serde_json::from_str(
        &std::fs::read_to_string(fixtures().join("editor/captures/quoted-scalars/graph.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(quoted.relations[0].kind, "relates_to");
    assert_eq!(quoted.relations[0].from.kind, "page");
}
