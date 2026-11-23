use super::*;

const PAGE: &str = "---\nid: broken-link-page\ntitle: Page with Broken Wikilink\n---\n\nThis page links to [[x]].\n";

#[test]
fn reads_fields_and_body_offsets() {
    let f = read(PAGE);
    assert!(f.present);
    assert_eq!(f.field("id").unwrap().value, "broken-link-page");
    assert_eq!(
        &PAGE[f.field("title").unwrap().value_range.clone()],
        "Page with Broken Wikilink"
    );
    assert_eq!(&PAGE[f.body_offset..], "\nThis page links to [[x]].\n");
    assert!(f.notes.is_empty());
    assert_eq!(f.rows().len(), SCHEMA.len());
}

#[test]
fn unknown_keys_are_advisory_notes_not_errors() {
    let f = read("---\nparentEntry: a\n  nested: x\n---\nbody");
    assert_eq!(
        f.notes[0],
        Note::UnknownKey {
            key: "parentEntry".into(),
            line: 2
        }
    );
    assert!(matches!(f.notes[1], Note::Unparsed { line: 3, .. }));
    assert_eq!(f.rows().len(), SCHEMA.len() + 1);
}

#[test]
fn absent_and_unclosed_frontmatter() {
    let f = read(" ---\nbody");
    assert!(!f.present);
    assert_eq!(f.body_offset, 0);
    let f = read("---\ntitle: x\n");
    assert_eq!(f.notes, vec![Note::Unclosed]);
}

#[test]
fn form_edits_are_undoable_buffer_edits() {
    let mut b = Buffer::open("p.md", PAGE, None);
    set_field(&mut b, "title", "Fixed").unwrap();
    set_field(&mut b, "status", "draft").unwrap();
    let t = b.text();
    assert!(
        t.starts_with("---\nid: broken-link-page\ntitle: Fixed\nstatus: draft\n---\n"),
        "{t}"
    );
    set_field(&mut b, "id", "").unwrap();
    assert!(!b.text().contains("id:"));
    b.undo();
    b.undo();
    b.undo();
    assert_eq!(b.text(), PAGE);
}

#[test]
fn crlf_is_preserved_and_missing_frontmatter_is_created() {
    let mut b = Buffer::open("p.md", "---\r\ntitle: a\r\n---\r\nbody", None);
    set_field(&mut b, "title", "b").unwrap();
    set_field(&mut b, "summary", "s").unwrap();
    assert_eq!(b.text(), "---\r\ntitle: b\r\nsummary: s\r\n---\r\nbody");
    let mut b = Buffer::open("p.md", "body", None);
    set_field(&mut b, "title", "T").unwrap();
    assert_eq!(b.text(), "---\ntitle: T\n---\nbody");
    assert_eq!(body(&b.text()), "body");
}

#[test]
fn typing_a_space_then_a_word_keeps_both() {
    let mut b = Buffer::open("p.md", "---\ntitle: A\n---\n", None);
    set_field(&mut b, "title", "A ").unwrap();
    assert_eq!(read(&b.text()).field("title").unwrap().value, "A");
    set_field(&mut b, "title", "A B").unwrap();
    assert_eq!(b.text(), "---\ntitle: A B\n---\n");
}
