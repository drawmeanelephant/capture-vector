//! Draft-sketch ↔ Boris parity over the captured `fixtures/editor` corpus.
//! The report is a golden (`fixtures/editor/parity.json`); bless with
//! `CV_BLESS=1`. Run and print via `cargo xtask parity`.

use cv_widgets::editor::parity::{self, PageParity};
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/editor")
}

fn report() -> Vec<PageParity> {
    ["index", "guide"]
        .iter()
        .map(|page| {
            let md = std::fs::read_to_string(
                root().join(format!("corpus-src/parity/content/{page}.md")),
            )
            .unwrap();
            let html =
                std::fs::read_to_string(root().join(format!("captures/parity/html/{page}.html")))
                    .unwrap();
            parity::compare(page, cv_ui_editor::frontmatter::body(&md), &html)
        })
        .collect()
}

#[test]
fn parity_report_matches_golden_and_diverges_where_boris_owns_semantics() {
    let r = report();
    for p in &r {
        eprintln!(
            "{}: {}% of Boris body words in sketch; missing {:?}; sketch-only {:?}",
            p.page, p.word_overlap_pct, p.missing_in_sketch, p.extra_in_sketch
        );
        for c in &p.constructs {
            eprintln!(
                "  {:<9} in source: {:<5} diverges: {:<5} raw syntax in sketch: {:<5} Boris-only words: {:?}",
                c.name, c.in_source, c.diverges, c.raw_in_sketch, c.boris_only
            );
        }
    }
    let index = &r[0];
    // Every Boris-owned construct in the probe diverges in the sketch.
    for c in &index.constructs {
        assert!(c.in_source && c.diverges, "{c:?}");
    }
    // Boris resolves what the sketch can't: the include's body text.
    assert!(
        index.missing_in_sketch.iter().any(|w| w == "Included"),
        "{:?}",
        index.missing_in_sketch
    );
    let v = serde_json::to_value(&r).unwrap();
    cv_widgets::harness::check_golden(&root(), "parity", &v).unwrap();
}
