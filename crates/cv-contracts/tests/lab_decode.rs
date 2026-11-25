//! `cv_contracts::lab` against the captured lab runs in
//! `fixtures/captures/lab/`. Captures only; the error cases edit a
//! captured document in memory.

use cv_contracts::FormatKey;
use cv_contracts::lab::*;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn lab_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/captures/lab")
}

fn modes() -> Vec<String> {
    let mut v: Vec<_> = std::fs::read_dir(lab_root())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// Every `.json` under `<mode>/out/`, as (`/`-separated rel, bytes).
fn documents(mode: &str) -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, Vec<u8>)>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let rel = format!("{prefix}{}", e.file_name().to_string_lossy());
            if e.path().is_dir() {
                walk(&e.path(), &format!("{rel}/"), out);
            } else if rel.ends_with(".json") {
                out.push((rel, std::fs::read(e.path()).unwrap()));
            }
        }
    }
    let mut v = Vec::new();
    walk(&lab_root().join(mode).join("out"), "", &mut v);
    v.sort();
    v
}

fn read(mode: &str, rel: &str) -> Vec<u8> {
    std::fs::read(lab_root().join(mode).join("out").join(rel)).unwrap()
}

fn decode(mode: &str, rel: &str) -> Document {
    Document::decode(rel, &read(mode, rel)).unwrap_or_else(|e| panic!("{mode}/{rel}: {e}"))
}

fn report(mode: &str) -> Report {
    match decode(mode, "report.json") {
        Document::Report(r) => r,
        other => panic!("{other:?}"),
    }
}

fn key_of(doc: &Document) -> FormatKey {
    let (f, v, table) = match doc {
        Document::Report(r) => return r.key(),
        Document::Inventory(i) => {
            let (f, v) = match i {
                Inventory::Hugo(i) => (&i.format, i.schema_version),
                Inventory::Jekyll(i) => (&i.format, i.schema_version),
                Inventory::Substack(i) => (&i.format, i.schema_version),
                Inventory::Enex(i) => (&i.format, i.schema_version),
            };
            (f, v, INVENTORIES)
        }
        Document::PageProvenance(p) => (&p.format, p.schema_version, PAGE_PROVENANCE),
        Document::ProvenanceManifest(p) => (&p.format, p.schema_version, PROVENANCE_MANIFESTS),
        Document::Redirects(r) => (&r.format, r.schema_version, REDIRECTS),
        Document::MediaManifest(m) => (&m.format, m.schema_version, MEDIA_MANIFESTS),
        Document::AttachmentsManifest(a) => (&a.format, a.schema_version, ATTACHMENTS_MANIFESTS),
        Document::LinkAudit(l) => (&l.format, l.schema_version, LINK_AUDITS),
        Document::FrontmatterReview(r) => (&r.format, r.schema_version, FRONTMATTER_REVIEWS),
    };
    *table
        .iter()
        .find(|k| k.format == f && k.schema_version == v)
        .unwrap()
}

#[test]
fn every_captured_lab_document_decodes_through_a_mirror() {
    let modes = modes();
    assert!(modes.len() >= 12, "{modes:?}");
    for mode in &modes {
        let docs = documents(mode);
        assert!(!docs.is_empty(), "{mode} captured no JSON");
        for (rel, bytes) in docs {
            Document::decode(&rel, &bytes).unwrap_or_else(|e| panic!("{mode}/{rel}: {e}"));
        }
    }
}

#[test]
fn every_mirrored_format_key_is_backed_by_a_capture() {
    let mut seen = BTreeSet::new();
    for mode in modes() {
        for (rel, bytes) in documents(&mode) {
            let k = key_of(&Document::decode(&rel, &bytes).unwrap());
            seen.insert((
                rel.split('/').next().unwrap().to_string(),
                k.format,
                k.schema_version,
            ));
        }
    }
    let tables: [(&str, &[FormatKey]); 9] = [
        ("report.json", REPORTS),
        ("inventory.json", INVENTORIES),
        ("provenance", PAGE_PROVENANCE),
        ("provenance_manifest.json", PROVENANCE_MANIFESTS),
        ("redirects.json", REDIRECTS),
        ("media_manifest.json", MEDIA_MANIFESTS),
        ("attachments_manifest.json", ATTACHMENTS_MANIFESTS),
        ("link_audit.json", LINK_AUDITS),
        ("frontmatter_review.json", FRONTMATTER_REVIEWS),
    ];
    for (role, table) in tables {
        for k in table {
            assert!(
                seen.contains(&(role.to_string(), k.format, k.schema_version)),
                "{role} {k:?} is mirrored but never captured"
            );
        }
    }
}

#[test]
fn captured_dispositions_use_only_the_four_lab_classes() {
    let mut classes = BTreeSet::new();
    for mode in modes() {
        for (_, bytes) in documents(&mode) {
            // Every `class` / `conversion` value anywhere in the document.
            fn collect(v: &serde_json::Value, out: &mut BTreeSet<String>) {
                match v {
                    serde_json::Value::Object(m) => {
                        for (k, x) in m {
                            if matches!(k.as_str(), "class" | "conversion")
                                && let Some(s) = x.as_str()
                            {
                                out.insert(s.to_string());
                            }
                            collect(x, out);
                        }
                    }
                    serde_json::Value::Array(a) => a.iter().for_each(|x| collect(x, out)),
                    _ => {}
                }
            }
            collect(&serde_json::from_slice(&bytes).unwrap(), &mut classes);
        }
    }
    let parsed: Vec<Disposition> = classes
        .iter()
        .map(|c| serde_json::from_value(serde_json::json!(c)).unwrap())
        .collect();
    assert!(
        parsed.iter().all(|d| !matches!(d, Disposition::Unknown(_))),
        "{classes:?}"
    );
    assert_eq!(
        classes.into_iter().collect::<Vec<_>>(),
        ["exact", "human_review", "transformed", "unsupported"]
    );
}

#[test]
fn every_report_page_row_points_at_a_captured_output() {
    for mode in [
        "hugo",
        "jekyll",
        "substack",
        "enex",
        "obsidian",
        "wordpress",
    ] {
        let r = report(mode);
        let rows = r.pages();
        assert!(!rows.is_empty(), "{mode}");
        assert!(r.tool_version().starts_with("0."), "{mode}");
        for row in rows {
            assert!(!row.entity_id.is_empty(), "{mode} {row:?}");
            if let Some(out) = row.output {
                assert!(
                    lab_root().join(mode).join("out").join(out).is_file(),
                    "{mode}: {out} not captured"
                );
            }
        }
    }
}

#[test]
fn report_families_decode_with_their_own_keys() {
    let Report::WordPress(wp) = report("wordpress") else {
        panic!("wordpress family")
    };
    assert_eq!(wp.schema_version, 3);
    assert!(wp.source_export.ends_with("/mini-wxr/export.xml"));
    let Report::Jekyll(j) = report("jekyll") else {
        panic!("jekyll family")
    };
    assert_eq!(j.summary.pages, j.pages.len() as u64);
    let counted = [
        Disposition::Exact,
        Disposition::Transformed,
        Disposition::Unsupported,
        Disposition::HumanReview,
    ]
    .map(|d| j.pages.iter().filter(|p| p.class == d).count() as u64);
    let c = j.summary.by_class;
    assert_eq!(
        counted,
        [c.exact, c.transformed, c.unsupported, c.human_review]
    );
    let Report::Hugo(h) = report("hugo") else {
        panic!("hugo family")
    };
    assert_eq!(h.summary.pages_emitted, h.pages.len() as u64);
    assert!(matches!(report("substack"), Report::Substack(_)));
    assert!(matches!(report("enex"), Report::Enex(_)));
    assert!(matches!(report("obsidian"), Report::Obsidian(_)));
}

#[test]
fn inventory_modes_decode_their_inventories() {
    let Document::Inventory(Inventory::Hugo(h)) = decode("hugo-inventory", "inventory.json") else {
        panic!("hugo inventory")
    };
    assert_eq!(h.mode, "inventory");
    let c = h.class_counts;
    assert_eq!(
        c.exact + c.transformed + c.unsupported + c.human_review,
        h.pages.len() as u64
    );
    let Document::Inventory(Inventory::Jekyll(j)) = decode("jekyll-inventory", "inventory.json")
    else {
        panic!("jekyll inventory")
    };
    assert_eq!(j.mode, "inventory");
    assert!(!j.constructs.is_empty());
    for mode in ["substack-inventory", "substack"] {
        assert!(matches!(
            decode(mode, "inventory.json"),
            Document::Inventory(Inventory::Substack(_))
        ));
    }
    for mode in ["enex-inventory", "enex"] {
        let Document::Inventory(enex) = decode(mode, "inventory.json") else {
            panic!("{mode} inventory")
        };
        assert!(!enex.dispositions().is_empty(), "{mode}");
    }
}

#[test]
fn provenance_decodes_per_page_and_per_run() {
    let Report::Jekyll(j) = report("jekyll") else {
        panic!()
    };
    for page in &j.pages {
        let Document::PageProvenance(p) = decode("jekyll", &page.provenance) else {
            panic!("{}", page.provenance)
        };
        assert_eq!(p.entity_id, page.entity_id);
        assert_eq!(p.class, page.class);
    }
    for mode in ["substack", "enex"] {
        let Document::ProvenanceManifest(m) = decode(mode, "provenance_manifest.json") else {
            panic!("{mode}")
        };
        assert!(!m.pages.is_empty(), "{mode}");
    }
}

#[test]
fn utility_modes_decode() {
    let Document::LinkAudit(a) = decode("link-audit", "link_audit.json") else {
        panic!()
    };
    assert!(a.html_files > 0);
    let Document::FrontmatterReview(r) = decode("frontmatter-review", "frontmatter_review.json")
    else {
        panic!()
    };
    assert_eq!(r.schema_version, 2);
    assert_eq!(r.total_unknown_keys, r.field_summaries.len() as u64);
    let dispositions = r.field_summaries.iter().map(|s| &s.disposition).chain(
        r.files
            .iter()
            .flat_map(|f| &f.unknown_keys)
            .map(|k| &k.disposition),
    );
    for d in dispositions {
        assert!(!matches!(d, FieldDisposition::Unknown(_)), "{d:?}");
    }
    assert!(matches!(
        decode("wordpress", "media_manifest.json"),
        Document::MediaManifest(_)
    ));
    assert!(matches!(
        decode("obsidian", "attachments_manifest.json"),
        Document::AttachmentsManifest(_)
    ));
    assert!(matches!(
        decode("hugo", "redirects.json"),
        Document::Redirects(_)
    ));
}

fn edited(mode: &str, rel: &str, edit: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut v: serde_json::Value = serde_json::from_slice(&read(mode, rel)).unwrap();
    edit(&mut v);
    serde_json::to_vec(&v).unwrap()
}

#[test]
fn an_unknown_schema_version_is_a_typed_error() {
    let bytes = edited("jekyll", "report.json", |v| v["schema_version"] = 2.into());
    match Report::decode(&bytes) {
        Err(DecodeError::UnsupportedSchemaVersion {
            format,
            found,
            supported,
            ..
        }) => {
            assert_eq!(format, format::JEKYLL);
            assert_eq!(found, 2);
            assert_eq!(supported, [1]);
        }
        other => panic!("{other:?}"),
    }
    // WordPress is mirrored at 3 only; its older shapes are not guessed.
    let bytes = edited("wordpress", "report.json", |v| {
        v["schema_version"] = 2.into()
    });
    assert!(matches!(
        Document::decode("report.json", &bytes),
        Err(DecodeError::UnsupportedSchemaVersion { found: 2, .. })
    ));
}

#[test]
fn an_unknown_format_or_file_is_a_typed_error() {
    let bytes = edited("hugo", "report.json", |v| {
        v["format"] = "boris-pelican-migration-lab".into()
    });
    assert!(matches!(
        Report::decode(&bytes),
        Err(DecodeError::UnknownFormat {
            document: "report.json",
            ..
        })
    ));
    // A Hugo id is a known family, but not a link-audit document.
    let hugo = read("hugo", "report.json");
    assert!(matches!(
        Document::decode("link_audit.json", &hugo),
        Err(DecodeError::UnknownFormat { .. })
    ));
    assert!(matches!(
        Document::decode("REPORT.md", b"# Report"),
        Err(DecodeError::UnknownDocument(_))
    ));
}

#[test]
fn additive_fields_under_a_known_version_are_kept() {
    let bytes = edited("hugo", "report.json", |v| {
        v["later_field"] = serde_json::json!({"n": 1});
        v["pages"][0]["page_extra"] = true.into();
    });
    let Report::Hugo(h) = Report::decode(&bytes).unwrap() else {
        panic!()
    };
    assert_eq!(h.extra["later_field"]["n"], 1);
    assert_eq!(h.pages[0].extra["page_extra"], true);
}

#[test]
fn an_unlisted_class_is_disclosed_not_mapped() {
    let bytes = edited("jekyll", "report.json", |v| {
        v["pages"][0]["class"] = "partial".into()
    });
    let Report::Jekyll(j) = Report::decode(&bytes).unwrap() else {
        panic!()
    };
    assert_eq!(j.pages[0].class, Disposition::Unknown("partial".into()));
    assert_eq!(j.pages[0].class.as_str(), "partial");
    assert_eq!(
        serde_json::to_value(&j.pages[0].class).unwrap(),
        serde_json::json!("partial")
    );
}
