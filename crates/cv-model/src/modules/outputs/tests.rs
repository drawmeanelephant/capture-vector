use super::*;
use crate::module::PageRecord;
use crate::modules::activity::tests::env;
use cv_core::events::engine::BuildPhase;

fn out(path: &str, kind: &str, sha: &str) -> OutputRecord {
    OutputRecord {
        path: path.into(),
        kind: kind.into(),
        producer: "p".into(),
        status: "committed".into(),
        sha256: sha.into(),
        bytes: 10,
        ..Default::default()
    }
}

fn snap() -> ProjectSnapshot {
    ProjectSnapshot {
        epoch: 4,
        compiler: Some("boris/0.9.0".into()),
        target: Some("default".into()),
        outputs: vec![
            out("index.html", "html-page", "a"),
            out("llms.txt", "llms", "b"),
            out("_boris/proof/checks.json", "proof-checks", "c"),
            out("_boris/search/search-index.json", "rendered-search", "d"),
            out("rss.xml", "feed", "e"),
            out("sitemap.xml", "sitemap", "f"),
            out(".boris/graph.json", "ir", "g"),
        ],
        pages: vec![PageRecord {
            path: "index.md".into(),
            output: Some("index.html".into()),
            ..Default::default()
        }],
        ir_graph: None,
        last_publish: None,
        artifact_root: None,
    }
}

#[test]
fn classes_are_mailboxes_and_empty_ones_are_disclosed() {
    let mut m = OutputsModule::default();
    m.load(&snap());
    let nav = m.nav().items;
    assert_eq!(nav.len(), 1 + CLASSES.len());
    for (k, n) in [
        ("ir", 1),
        ("llms", 1),
        ("rss", 1),
        ("sitemap", 1),
        ("proof", 1),
        ("search", 1),
        ("dist", 1),
        ("rag", 0),
        ("context", 0),
    ] {
        assert_eq!(m.list_len(Some(k)), n, "{k}");
    }
}

#[test]
fn provenance_and_freshness_vs_last_publish() {
    let mut s = snap();
    let mut m = OutputsModule::default();
    m.load(&s);
    let f = |m: &OutputsModule, k: &str, f: &str| {
        m.inspector(k)
            .fields
            .into_iter()
            .find(|(n, _)| n == f)
            .unwrap()
            .1
    };
    assert_eq!(f(&m, "index.html", "fresh"), "never published");
    assert_eq!(f(&m, "index.html", "compiler"), "boris/0.9.0");
    assert_eq!(f(&m, "index.html", "profile"), "default");
    assert_eq!(f(&m, "index.html", "snapshot"), "epoch 4");
    assert_eq!(m.inspector("index.html").lists[0].1, ["index.md"]);
    s.last_publish = Some(PublishMark {
        label: "pub-1".into(),
        hashes: [
            ("index.html".to_string(), "a".to_string()),
            ("llms.txt".to_string(), "old".to_string()),
        ]
        .into(),
    });
    m.load(&s);
    assert_eq!(f(&m, "index.html", "fresh"), "as published");
    assert_eq!(f(&m, "llms.txt", "fresh"), "changed since publish");
    assert_eq!(f(&m, "rss.xml", "fresh"), "new since publish");
    m.ingest(&env(
        1,
        EngineEvent::BuildFailed {
            phase: BuildPhase::Rebuild,
            diagnostics: vec![],
            recoverable: true,
        },
    ));
    assert_eq!(m.row(Some("dist"), 0).tone, Tone::Warn);
    assert_eq!(m.link("index.html").unwrap().key, "index.md");
    let rows = m.attr_rows();
    assert_eq!(rows.len(), 7);
    assert!(
        rows.iter()
            .all(|r| r.noun.kind.as_str() == noun_kind::OUTPUT)
    );
}
