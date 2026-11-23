//! Issue #26 probes (spike pattern, rerun with `cargo test -p cv-app --test probes -- --nocapture`).
//! Verdicts live in `spike/FINDINGS.md`.

use cv_app::{SCENES, init};
use cv_widgets::harness::{Harness, SIZE};
use cv_widgets::reference;

fn scene(name: &str) -> cv_model::workbench::Workbench {
    let (_, f) = SCENES.iter().find(|(n, _)| *n == name).unwrap();
    let mut wb = init();
    f(&mut wb);
    wb
}

/// The dogfood corpus, Pages module, one page selected: the reading pane
/// hands its body to `rune_kit::Markdown` inside the real shell tree.
fn dogfood_page(page: &str) -> Harness {
    let mut wb = scene("dogfood");
    let i = wb.registry.index_of("pages").expect("pages module");
    wb.activate(i);
    wb.set_nav("all");
    wb.select_key(page);
    assert_eq!(wb.selected_key(), Some(page), "page row selected");
    Harness::new(wb)
}

/// Texts under the `sketch` scroller, in document order.
fn sketch_texts(h: &Harness) -> Vec<String> {
    let els = h.rt.elements();
    let Some(root) = els.iter().find(|e| e.key.as_deref() == Some("sketch")) else {
        return vec![];
    };
    let prefix = format!("{}/", root.path);
    els.iter()
        .filter(|e| e.path.starts_with(&prefix) && !e.exiting)
        .filter_map(|e| e.text.clone())
        .map(|t| t.replace(['\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}'], ""))
        .collect()
}

#[test]
fn markdown_sketch_renders_corpus_page_in_shell_tree() {
    let h = dogfood_page("guides/asides.md");
    assert!(
        h.has("sketch-label") && h.has("sketch"),
        "reading pane mounts the sketch"
    );
    let t = sketch_texts(&h);
    eprintln!("sketch texts ({}):", t.len());
    for s in &t {
        eprintln!("  {s:?}");
    }
    let has = |needle: &str| t.iter().any(|s| s.contains(needle));
    // Sane CommonMark: headings, emphasis-bearing paragraphs, fenced code, GFM table.
    assert!(has("Using Asides"), "h1");
    assert!(has("Authoring rules") && has("Live examples"), "h2s");
    assert!(has("semantic"), "paragraph text");
    assert!(has("Close tag must start a line."), "fenced code body");
    assert!(has("Actionable advice"), "table cell");
    // Expected divergences (Boris owns these semantics; the sketch is advisory):
    // wikilinks stay raw syntax.
    assert!(has("[[guides/overview|content model]]"), "raw wikilink");
    // <Aside> tags are HTML blocks to CommonMark: dropped, their bodies render as plain markdown
    // with no callout label.
    // (The only `<Aside` text left is the fenced code sample's.)
    let tags: Vec<_> = t.iter().filter(|s| s.contains("<Aside")).collect();
    assert_eq!(tags.len(), 1, "live Aside tags are not shown: {tags:?}");
    assert!(
        tags[0].contains("Body markdown here."),
        "only the fenced sample"
    );
    assert!(
        has("Prefer short, actionable tips."),
        "Aside body still renders"
    );
    assert!(!t.iter().any(|s| s.trim() == "Tip"), "no admonition label");
}

#[test]
fn markdown_sketch_shows_includes_and_details_raw() {
    let h = dogfood_page("getting-started.md");
    let t = sketch_texts(&h);
    let has = |needle: &str| t.iter().any(|s| s.contains(needle));
    assert!(has("Getting Started"));
    // `{{include …}}` is not expanded — the fragment's text never appears.
    assert!(has("{{include includes/shared-tip.md}}"), "{t:?}");
    let h = dogfood_page("guides/apex-markdown.md");
    let t = sketch_texts(&h);
    eprintln!("apex-markdown sketch: {} texts", t.len());
    assert!(!t.is_empty(), "a long gallery page still renders");
}

/// Device scale: 1.0 keeps the CPU oracle quick in CI; `CV_REF_SCALE=2` matches `--snap`
/// (`scripts/check-ui-png.sh`) so the PNGs in `CV_REF_OUT` diff against the Lavapipe goldens.
fn ref_scale() -> f32 {
    std::env::var("CV_REF_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0)
}

fn snap_harness(name: &str) -> Harness {
    let mut h = Harness::new(scene(name));
    h.rt.set_viewport(SIZE, ref_scale());
    h.settle();
    h
}

#[test]
fn rune_ref_rasterizes_shell_frame_headless_and_reproducibly() {
    let k = ref_scale();
    let size = ((SIZE.0 * k) as u32, (SIZE.1 * k) as u32);
    for name in ["canary", "pages"] {
        let h = snap_harness(name);
        let (sc, rep) = reference::scene(&h.rt, size);
        eprintln!("{name}: {} instances; {rep:?}", sc.primitives.len());
        assert_eq!(rep.invalid, None, "{name}: rune-ref accepts the frame");
        assert_eq!(rep.off_page_glyphs, 0, "{name}: one glyph page");
        let t0 = std::time::Instant::now();
        let a = reference::render(&sc);
        let dt = t0.elapsed();
        let b = reference::render(&reference::scene(&snap_harness(name).rt, size).0);
        eprintln!("{name}: blake3={} render {dt:?}", a.hash());
        assert_eq!(a.hash(), b.hash(), "{name}: two fresh runs, same bytes");
        let png = reference::png(&a);
        if let Some(dir) = std::env::var_os("CV_REF_OUT") {
            std::fs::write(std::path::Path::new(&dir).join(format!("{name}.png")), &png).unwrap();
        }
    }
}
