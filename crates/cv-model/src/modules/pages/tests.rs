use super::*;
use crate::modules::activity::tests::{assert_index_matches_scan, diag, env};
use cv_core::events::engine::BuildPhase;

pub(crate) fn page(path: &str, id: &str, parent: Option<&str>) -> PageRecord {
    PageRecord {
        path: path.into(),
        id: id.into(),
        title: id.to_uppercase(),
        parent: parent.map(String::from),
        body: format!("# {id}\n\nBody."),
        frontmatter: vec![("title".into(), id.to_uppercase())],
        ..Default::default()
    }
}

pub(crate) fn snap() -> ProjectSnapshot {
    let mut rel = page("guides/b.md", "b", Some("guides"));
    rel.relations = vec![("relates_to".into(), "a".into())];
    rel.output = Some("guides/b.html".into());
    ProjectSnapshot {
        pages: vec![
            page("index.md", "index", None),
            page("guides/index.md", "guides", Some("index")),
            page("guides/a.md", "a", Some("guides")),
            rel,
            page("orphan.md", "orphan", Some("does-not-exist")),
        ],
        ..Default::default()
    }
}

fn loaded() -> PagesModule {
    let mut m = PagesModule::default();
    m.load(&snap());
    m
}

#[test]
fn mailbox_tree_nests_parents_and_lists_children() {
    let m = loaded();
    let nav: Vec<(String, String)> = m
        .nav()
        .items
        .into_iter()
        .map(|i| (i.key, i.label))
        .collect();
    assert!(nav.contains(&("tree:index".into(), "INDEX".into())));
    assert!(nav.contains(&("tree:guides".into(), "  GUIDES".into())));
    let kids: Vec<String> = (0..m.list_len(Some("tree:guides")))
        .map(|i| m.row(Some("tree:guides"), i).key)
        .collect();
    assert_eq!(kids, ["guides/a.md", "guides/b.md"]);
    let all: Vec<String> = (0..m.list_len(None)).map(|i| m.row(None, i).key).collect();
    assert_eq!(
        all,
        [
            "index.md",
            "guides/index.md",
            "guides/a.md",
            "guides/b.md",
            "orphan.md"
        ]
    );
    assert_eq!(m.list_len(Some("orphans")), 1);
}

#[test]
fn reading_has_breadcrumbs_markdown_children_relations() {
    let m = loaded();
    let r = m.reading("guides/b.md");
    assert_eq!(r.breadcrumbs, ["INDEX", "GUIDES", "B"]);
    assert_eq!(r.markdown.as_deref(), Some("# b\n\nBody."));
    assert!(
        r.lines
            .iter()
            .any(|l| l.label.as_deref() == Some("relates_to") && l.text == "A")
    );
    let g = m.reading("guides/index.md");
    assert_eq!(
        g.lines
            .iter()
            .filter(|l| l.label.as_deref() == Some("satellite"))
            .count(),
        2
    );
    assert_eq!(
        m.reading("orphan.md").breadcrumbs,
        ["does-not-exist (missing)", "ORPHAN"]
    );
}

#[test]
fn inspector_carries_frontmatter_diagnostics_and_impact() {
    let mut m = loaded();
    let d = diag("EPARENTMISSING", "orphan.md", 7);
    m.ingest(&env(
        7,
        EngineEvent::BuildFailed {
            phase: BuildPhase::Initial,
            diagnostics: vec![d.clone()],
            recoverable: false,
        },
    ));
    let i = m.inspector("orphan.md");
    assert_eq!(i.diagnostics, vec![d.clone()]);
    assert!(!m.is_stale(&d));
    assert!(i.fields.contains(&("title".into(), "ORPHAN".into())));
    let a = m.inspector("guides/a.md");
    assert_eq!(
        a.lists[1],
        ("Referenced by".into(), vec!["B (relates_to)".to_string()])
    );
    let idx = m.inspector("index.md");
    assert_eq!(idx.lists[0].1.len(), 3);
    assert_eq!(m.list_len(Some("issues")), 1);
    assert_eq!(m.row(Some("issues"), 0).rail, Some(Tone::Error));
    assert!(m.is_stale(&diag("EPARENTMISSING", "orphan.md", 6)));
}

#[test]
fn project_level_diagnostics_get_a_row() {
    let mut m = loaded();
    let mut d = diag("ETEXTILE", "x", 1);
    d.span = None;
    m.ingest(&env(
        1,
        EngineEvent::BuildFailed {
            phase: BuildPhase::Initial,
            diagnostics: vec![d.clone()],
            recoverable: false,
        },
    ));
    assert_eq!(m.list_len(Some("issues")), 1);
    assert_eq!(m.row(Some("issues"), 0).key, PROJECT_KEY);
    assert_eq!(m.inspector(PROJECT_KEY).diagnostics, vec![d]);
}

#[test]
fn attr_rows_and_nouns() {
    let m = loaded();
    let rows = m.attr_rows();
    assert_eq!(rows.len(), 5);
    let b = rows.iter().find(|r| r.noun.stable.0 == "b").unwrap();
    assert_eq!(b.fields["role"], AttrValue::Text("satellite".into()));
    assert_eq!(b.fields["depth"], AttrValue::Int(2));
    assert_eq!(m.noun("index.md").unwrap().kind.as_str(), noun_kind::PAGE);
    assert_eq!(m.link("guides/b.md").unwrap().module, "outputs");
}

#[test]
fn parent_cycles_are_listed_not_dropped() {
    let mut m = PagesModule::default();
    m.load(&ProjectSnapshot {
        pages: vec![page("x.md", "x", Some("y")), page("y.md", "y", Some("x"))],
        ..Default::default()
    });
    assert_eq!(m.list_len(None), 2);
    assert_eq!(m.reading("x.md").breadcrumbs, ["Y", "X"]);
}

fn diags_for(seq: u64, paths: &[Option<&str>]) -> Envelope {
    let diagnostics = paths
        .iter()
        .map(|p| {
            let mut d = diag("E", p.unwrap_or_default(), seq);
            if p.is_none() {
                d.span = None;
            }
            d
        })
        .collect();
    env(
        seq,
        EngineEvent::BuildFailed {
            phase: BuildPhase::Rebuild,
            diagnostics,
            recoverable: true,
        },
    )
}

#[test]
fn index_of_key_matches_the_row_scan_in_every_mailbox() {
    let extra = [PROJECT_KEY, "missing.md", ""];
    let mut m = loaded();
    assert_index_matches_scan(&m, &extra);
    // Located, unlocated and unknown-path diagnostics: the project row.
    m.ingest(&diags_for(1, &[Some("guides/a.md"), None, Some("gone.md")]));
    assert_eq!(m.index_of_key(Some(ISSUES), PROJECT_KEY), Some(1));
    assert_index_matches_scan(&m, &extra);
    // A page whose path is the project key, and a duplicated path.
    let mut s = snap();
    s.pages.push(page(PROJECT_KEY, "proj", None));
    s.pages.push(page("guides/a.md", "a2", Some("index")));
    m.load(&s);
    assert_index_matches_scan(&m, &extra);
}

#[test]
fn nav_cache_refreshes_on_load_and_ingest() {
    let count =
        |m: &PagesModule, k: &str| m.nav().items.iter().find(|i| i.key == k).map(|i| i.count);
    let mut m = loaded();
    assert_eq!(count(&m, ISSUES), Some(Some(0)));
    m.ingest(&diags_for(1, &[Some("orphan.md")]));
    assert_eq!(count(&m, ISSUES), Some(Some(1)));
    assert_eq!(m.nav(), m.build_nav());
    let mut s = snap();
    s.pages.push(page("guides/c.md", "c", Some("guides")));
    s.pages.push(page("c/child.md", "c-child", Some("c")));
    m.load(&s);
    assert_eq!(count(&m, ALL), Some(Some(7)));
    assert_eq!(count(&m, "tree:guides"), Some(Some(3)));
    assert_eq!(count(&m, "tree:c"), Some(Some(1)));
    assert_eq!(m.nav(), m.build_nav());
}
