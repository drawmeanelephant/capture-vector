//! Promotion drafts vs real Boris captures: Boris is the judge.

#[path = "support/fixture_notes.rs"]
mod fixture_notes;
#[path = "support/research.rs"]
mod research;

use cv_core::id::StableId;
use cv_notes::{Format, PageIndex, PromoteError, promote, promote_with};
use fixture_notes::*;

fn page_rel(corpus: &str, path: &str) -> String {
    format!("corpus-src/{corpus}/{path}")
}

#[test]
fn promoted_draft_is_exactly_what_boris_validated() {
    let note = field_note();
    let draft = promote_with(&note, &promoted_spec(&note)).unwrap();
    assert_eq!(draft.path, format!("content/notes/{PAGE_ID}.md"));
    assert_eq!(
        research::file_str(&page_rel("promoted", &draft.path)),
        draft.body
    );
    assert_eq!(
        research::file_str("captures/promoted/validate/exit-code.txt").trim(),
        "0"
    );
    assert!(
        research::file_str("captures/promoted/validate/stderr.txt").contains("validation passed")
    );
    assert_eq!(
        research::file_str("captures/promoted/build-ir/exit-code.txt").trim(),
        "0"
    );
}

#[test]
fn frontmatter_carries_only_closed_keys_and_no_provenance() {
    let note = field_note();
    let draft = promote_with(&note, &promoted_spec(&note)).unwrap();
    let fm = draft.body.split("---").nth(1).unwrap();
    let keys: Vec<&str> = fm
        .lines()
        .filter_map(|l| l.split(':').next())
        .filter(|k| !k.is_empty())
        .collect();
    assert_eq!(keys, ["id", "title", "parent", "tags"]);
    assert!(
        !draft.body.contains(&note.id.0),
        "note id must stay in the .cv/ sidecar"
    );
}

#[test]
fn rust_does_not_judge_boris_does() {
    // A draft with a dangling parent is still produced; Boris rejects it.
    let note = field_note();
    let draft = promote_with(&note, &orphan_spec(&note)).unwrap();
    assert_eq!(
        research::file_str(&page_rel("orphan", &draft.path)),
        draft.body
    );
    assert_eq!(
        research::file_str("captures/orphan/validate/exit-code.txt").trim(),
        "1"
    );
    assert!(research::file_str("captures/orphan/validate/stderr.txt").contains("EPARENTMISSING"));
}

#[test]
fn reparented_page_keeps_its_stable_id_in_boris_ir() {
    let note = field_note();
    let before =
        PageIndex::from_graph_json(&research::file("captures/promoted/build-ir/graph.json"))
            .unwrap();
    let after =
        PageIndex::from_graph_json(&research::file("captures/reparented/build-ir/graph.json"))
            .unwrap();
    let id = promoted_spec(&note).page;
    let (b, a) = (before.get(&id).unwrap(), after.get(&id).unwrap());
    assert_eq!(b.source_path, format!("notes/{PAGE_ID}.md"));
    assert_eq!(a.source_path, format!("guides/{PAGE_ID}.md"));
    assert_eq!(b.parent.as_ref().unwrap().0, "index");
    assert_eq!(a.parent.as_ref().unwrap().0, "guides/getting-started");
    let d = promote_with(&note, &reparented_spec(&note)).unwrap();
    assert_eq!(research::file_str(&page_rel("reparented", &d.path)), d.body);
    assert_eq!(
        research::file_str("captures/reparented/validate/exit-code.txt").trim(),
        "0"
    );
}

#[test]
fn default_plan_and_refusals() {
    let mut note = field_note();
    assert_eq!(
        promote(&note).unwrap().path,
        format!("content/notes/{PAGE_ID}.md")
    );
    note.format = Format::Textile;
    assert_eq!(
        promote(&note).unwrap_err(),
        PromoteError::Format(Format::Textile)
    );
    note.format = Format::Markdown;
    note.title = "two\nlines".into();
    assert_eq!(
        promote(&note).unwrap_err(),
        PromoteError::Multiline("title")
    );
    let mut spec = promoted_spec(&field_note());
    spec.path = ".cv/research/x.md".into();
    assert!(matches!(
        promote_with(&field_note(), &spec),
        Err(PromoteError::Path(_))
    ));
}

#[test]
fn scalars_emit_quoted_so_yaml_traps_stay_inert() {
    // Plain-scalar traps — leading block indicators, coercion words,
    // commas — must survive as literal text inside Boris dquotes.
    let mut note = field_note();
    note.title = "| yes, 123: still a string".into();
    note.tags = ["a, b".to_string(), "x".to_string()].into_iter().collect();
    let draft = promote(&note).unwrap();
    assert!(
        draft.body.starts_with("---\nid: \""),
        "id must be quoted:\n{}",
        draft.body
    );
    assert!(
        draft
            .body
            .contains("title: \"| yes, 123: still a string\"\n")
    );
    assert!(draft.body.contains("tags: [\"a, b\", \"x\"]\n"));
}

#[test]
fn scalars_boris_cannot_quote_are_refused() {
    // Boris's dquoted has no escapes: a raw `"` is illegal, and
    // U+0085/U+2028/U+2029 split lines for YAML readers. Refuse rather
    // than emit frontmatter Boris or a YAML tool would misparse.
    let mut note = field_note();
    for bad in ['"', '\u{85}', '\u{2028}', '\u{2029}'] {
        note.title = format!("a{bad}b");
        assert_eq!(
            promote(&note).unwrap_err(),
            PromoteError::Unquotable("title"),
            "input with {bad:?} must be refused"
        );
    }
    let mut spec = promoted_spec(&field_note());
    spec.parent = Some(StableId("bad\"parent".into()));
    assert_eq!(
        promote_with(&field_note(), &spec).unwrap_err(),
        PromoteError::Unquotable("parent")
    );
    let mut note = field_note();
    note.tags = ["ok".to_string(), "bad\u{2028}tag".to_string()]
        .into_iter()
        .collect();
    assert_eq!(promote(&note).unwrap_err(), PromoteError::Unquotable("tag"));
}
