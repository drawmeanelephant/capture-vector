use super::*;

#[test]
fn context_finds_an_open_wikilink_on_the_caret_line() {
    let t = "see [[non-ex and more";
    let c = wikilink_context(t, 12).unwrap();
    assert_eq!(c.prefix, "non-ex");
    assert_eq!(c.range, 6..12);
    assert!(wikilink_context("see [[a]] b", 11).is_none());
    assert!(wikilink_context("[[a|label", 9).is_none());
    assert!(wikilink_context("[[a\nb", 5).is_none());
}

#[test]
fn accept_closes_the_link_once() {
    let mut b = Buffer::open("p.md", "x [[no", None);
    let ctx = wikilink_context(&b.text(), 6).unwrap();
    let caret = accept(&mut b, &ctx, "intro").unwrap();
    assert_eq!(b.text(), "x [[intro]]");
    assert_eq!(caret, 11);
    let mut b = Buffer::open("p.md", "x [[no]] y", None);
    let ctx = wikilink_context(&b.text(), 6).unwrap();
    accept(&mut b, &ctx, "intro").unwrap();
    assert_eq!(b.text(), "x [[intro]] y");
}

#[test]
fn other_formats_degrade_instead_of_guessing() {
    let j = r#"{"format":"x","schema_version":2,"compiler_id":"c","frozen":true,"entities":[]}"#;
    assert_eq!(
        CompletionIndex::parse(j),
        Err(CompletionError::Unsupported {
            format: "x".into(),
            schema_version: 2
        })
    );
    assert!(matches!(
        CompletionIndex::parse("{"),
        Err(CompletionError::Malformed(_))
    ));
}
