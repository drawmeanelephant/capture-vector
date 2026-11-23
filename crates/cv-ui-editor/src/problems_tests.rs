use super::*;
use cv_core::diagnostic::{Span, StderrDiagnostic};

fn diag(start: u32, end: u32, line: u32) -> Diagnostic {
    Diagnostic {
        code: "EX".into(),
        severity: Severity::Error,
        span: Some(Span {
            path: "p.md".into(),
            start,
            end,
            line,
            col: 1,
        }),
        message: "m".into(),
        build_version: None,
    }
}

#[test]
fn fresh_until_typed_then_stale_and_never_remapped() {
    let mut b = Buffer::open("p.md", "ab\ncd\n", None);
    let mut p = Problems::default();
    p.ingest(BuildId("b1".into()), &[diag(3, 5, 2)], &b);
    let rail = p.rail(&b);
    assert_eq!(rail[0].freshness, Freshness::Fresh);
    assert_eq!(rail[0].target, Some(3..5));
    assert_eq!(rail[0].diagnostic.build_version, Some(BuildId("b1".into())));
    b.insert(0, "zz").unwrap();
    let rail = p.rail(&b);
    assert_eq!(rail[0].freshness, Freshness::Stale);
    assert_eq!(rail[0].target, None, "stale spans are not remapped");
    assert_eq!(
        p.gutter(&b),
        vec![GutterMark {
            line: 1,
            severity: Severity::Error,
            stale: true
        }]
    );
    b.undo();
    assert_eq!(
        p.rail(&b)[0].freshness,
        Freshness::Fresh,
        "same bytes again"
    );
}

#[test]
fn diagnostics_from_another_build_are_stale() {
    let b = Buffer::open("p.md", "ab", None);
    let mut d = diag(0, 1, 1);
    d.build_version = Some(BuildId("old".into()));
    let mut p = Problems::default();
    p.ingest(BuildId("new".into()), &[d, diag(0, 1, 1)], &b);
    let rail = p.rail(&b);
    assert_eq!(rail[0].freshness, Freshness::Fresh);
    assert_eq!(rail[1].freshness, Freshness::Stale);
}

#[test]
fn stderr_diagnostics_navigate_by_line_and_other_paths_are_filtered() {
    let b = Buffer::open("page.md", "a\nb\nlink [[x]]\n", None);
    let s = StderrDiagnostic::parse("error: EREFERENCEMISSING: page.md:3:6: missing").unwrap();
    let mut other = diag(0, 1, 1);
    other.span.as_mut().unwrap().path = "elsewhere.md".into();
    let mut p = Problems::default();
    p.ingest(BuildId("b".into()), &[Diagnostic::from(&s), other], &b);
    let rail = p.rail(&b);
    assert_eq!(rail.len(), 1);
    assert_eq!(rail[0].line, Some(2));
    assert_eq!(rail[0].target, Some(4..14));
}

#[test]
fn a_clean_build_clears_the_rail() {
    let b = Buffer::open("p.md", "ab", None);
    let mut p = Problems::default();
    p.ingest(BuildId("1".into()), &[diag(0, 1, 1)], &b);
    p.ingest(BuildId("2".into()), &[], &b);
    assert!(p.rail(&b).is_empty());
}
