//! The entire cv-contracts suite: decode real captures only.
//! Hand-authored Boris JSON is banned — these read `fixtures/` directly.
//! Negative inputs are in-memory mutations of real captures, never
//! checked-in JSON: the mutation constructs exactly the nonconformance
//! under test and cites the capture it descends from.

use cv_contracts::{
    build_info::BuildInfo,
    graph::{
        DependencyKind, EndpointType, Graph, GraphDecodeError, NodeRole, NodeStatus,
        SemanticRelationKind, decode_graph, decode_graph_value,
    },
    proof::*,
    watch,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

/// One captured `graph.json` as mutable JSON, for nonconformance
/// mutations. The capture bytes stay pinned on disk; only the in-memory
/// copy is bent.
fn captured_graph_value(rel: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixtures().join(rel)).unwrap()).unwrap()
}

fn happy03() -> Value {
    captured_graph_value("ir/captures/happy/graph.json")
}

fn dogfood02() -> Value {
    captured_graph_value("ir/captures/dogfood/graph.json")
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
    for dir in ["editor/captures", "research/captures", "ir/captures"] {
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
    assert!(graphs >= 8, "only {graphs} IR graphs captured");
    let quoted: Graph = serde_json::from_str(
        &std::fs::read_to_string(fixtures().join("editor/captures/quoted-scalars/graph.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        quoted.relations[0].kind,
        SemanticRelationKind::Token("relates_to".into())
    );
    assert_eq!(quoted.relations[0].from.kind, EndpointType::Page);
}

#[test]
fn unsupported_and_future_schema_versions_never_decode() {
    for found in ["999.0", "0.1.0", "0.5.0", "1.0.0", "0.3", "0.3.0 "] {
        let mut v = happy03();
        v["schemaVersion"] = Value::from(found);
        let Err(GraphDecodeError::UnsupportedSchemaVersion {
            found: reported, ..
        }) = decode_graph_value(v)
        else {
            panic!("schemaVersion {found:?} must not decode");
        };
        assert_eq!(reported, found);
    }
    // Non-string and missing version claims are invalid IR shape, not a
    // version rejection.
    let mut v = happy03();
    v["schemaVersion"] = Value::from(3);
    assert!(matches!(
        decode_graph_value(v),
        Err(GraphDecodeError::Json(_))
    ));
    let mut v = happy03();
    v.as_object_mut().unwrap().remove("schemaVersion");
    assert!(matches!(
        decode_graph_value(v),
        Err(GraphDecodeError::Json(_))
    ));
}

#[test]
fn required_node_keys_are_required_even_when_nullable() {
    // Every one of these keys is required by the frozen schema. Four are
    // nullable, but null is a value — the key must be present.
    for key in [
        "index",
        "id",
        "sourcePath",
        "role",
        "parent",
        "parentIndex",
        "title",
        "status",
        "tags",
        "bodyOffset",
    ] {
        let mut v = happy03();
        v["nodes"][0].as_object_mut().unwrap().remove(key);
        assert!(
            matches!(decode_graph_value(v), Err(GraphDecodeError::Json(_))),
            "node missing required {key} must not decode"
        );
    }
    // Null in the nullable keys is conforming: the real captures already
    // carry explicit nulls for trunk parent/parentIndex/status.
    let g = decode_graph_value(dogfood02()).expect("0.2 capture decodes");
    let trunk = g.nodes.iter().find(|n| n.role == NodeRole::Trunk).unwrap();
    assert_eq!(trunk.parent, None);
    assert_eq!(trunk.parent_index, None);
}

#[test]
fn closed_vocabularies_decode_known_values_as_typed_variants() {
    let g = decode_graph_value(dogfood02()).expect("0.2 capture decodes");
    assert!(g.nodes.iter().any(|n| n.role == NodeRole::Trunk));
    assert!(g.nodes.iter().any(|n| n.role == NodeRole::Satellite));
    assert!(
        g.nodes
            .iter()
            .any(|n| n.status.as_ref() == Some(&NodeStatus::Published))
    );
    for kind in [
        DependencyKind::Parent,
        DependencyKind::Include,
        DependencyKind::Reference,
    ] {
        assert!(g.edges.iter().any(|e| e.kind == kind), "missing {kind:?}");
    }
    assert!(
        g.edges
            .iter()
            .any(|e| e.from.kind == EndpointType::Page || e.to.kind == EndpointType::Page)
    );
    assert!(
        g.edges
            .iter()
            .any(|e| e.from.kind == EndpointType::Source || e.to.kind == EndpointType::Source)
    );

    let h = decode_graph_value(happy03()).expect("0.3 capture decodes");
    assert_eq!(
        h.relations[0].kind,
        SemanticRelationKind::Token("relates_to".into())
    );
}

#[test]
fn nullable_status_is_none_and_each_known_status_is_typed() {
    let g = decode_graph_value(happy03()).expect("0.3 capture decodes");
    assert!(
        g.nodes.iter().all(|n| n.status.is_none()),
        "the real capture carries explicit null statuses"
    );

    for (wire, want) in [
        ("draft", NodeStatus::Draft),
        ("published", NodeStatus::Published),
        ("archived", NodeStatus::Archived),
    ] {
        let mut v = happy03();
        v["nodes"][0]["status"] = Value::from(wire);
        let g = decode_graph_value(v).expect("known status decodes");
        assert_eq!(g.nodes[0].status.as_ref(), Some(&want));
    }
}

#[test]
fn closed_vocabulary_drift_decodes_to_typed_unknown_verbatim() {
    let mut v = happy03();
    v["nodes"][0]["role"] = Value::from("branch");
    v["nodes"][0]["status"] = Value::from("live");
    v["edges"][0]["kind"] = Value::from("layout");
    v["edges"][0]["from"]["type"] = Value::from("asset");
    let g = decode_graph_value(v).expect("closed-vocabulary drift does not crash decode");

    assert_eq!(g.nodes[0].role, NodeRole::Unknown("branch".into()));
    assert_eq!(
        g.nodes[0].status,
        Some(NodeStatus::Unknown("live".into())),
        "a drifted non-null string is distinct from the wire null"
    );
    assert_eq!(g.edges[0].kind, DependencyKind::Unknown("layout".into()));
    assert_eq!(g.edges[0].from.kind, EndpointType::Unknown("asset".into()));
}

#[test]
fn semantic_relation_kind_is_open_but_schema_drift_is_typed() {
    let cases: Vec<(String, bool)> = vec![
        ("verified_by2".into(), true),
        ("novel_author_token9".into(), true),
        ("Unknown".into(), false),
        ("unknown-kind".into(), false),
        ("1kind".into(), false),
        ("".into(), false),
        ("a".repeat(65), false),
    ];
    for (wire, conforming) in cases {
        let mut v = happy03();
        v["relations"][0]["kind"] = Value::from(wire.clone());
        let g = decode_graph_value(v).expect("relation-kind drift does not crash decode");
        let kind = &g.relations[0].kind;
        assert_eq!(kind.as_str(), wire);
        if conforming {
            assert!(
                matches!(kind, SemanticRelationKind::Token(_)),
                "{wire:?} must remain an open author token"
            );
        } else {
            assert!(
                matches!(kind, SemanticRelationKind::Unknown(_)),
                "{wire:?} is outside the constrained-open grammar"
            );
        }
    }
}

#[test]
fn required_root_keys_are_required() {
    for key in ["frozen", "nodes", "edges", "reverseIndex", "nav"] {
        let mut v = happy03();
        v.as_object_mut().unwrap().remove(key);
        assert!(
            matches!(decode_graph_value(v), Err(GraphDecodeError::Json(_))),
            "graph missing required {key} must not decode"
        );
    }
    // 0.3.0 carries `relations` always, including as an empty array:
    // dropping the key is not the same as an empty one.
    let mut v = happy03();
    v.as_object_mut().unwrap().remove("relations");
    assert!(matches!(
        decode_graph_value(v),
        Err(GraphDecodeError::Json(_))
    ));
}

#[test]
fn facet_keys_of_a_later_schema_are_rejected_under_an_earlier_one() {
    // `relations` under a 0.2.0 claim: Boris must never emit relations
    // while claiming IR 0.2, so this document contradicts itself.
    let mut v = dogfood02();
    v["relations"] = Value::Array(Vec::new());
    assert!(matches!(
        decode_graph_value(v),
        Err(GraphDecodeError::MisplacedFacetKey {
            key: "relations",
            ..
        })
    ));
    // `recipe` on a node under 0.2.0 or 0.3.0, same contradiction.
    for mut v in [dogfood02(), happy03()] {
        v["nodes"][0]["recipe"] = Value::Null;
        let version = v["schemaVersion"].as_str().unwrap().to_string();
        assert!(
            matches!(
                decode_graph_value(v),
                Err(GraphDecodeError::MisplacedFacetKey { key: "recipe", .. })
            ),
            "recipe under {version} must not decode"
        );
    }
    // Genuinely unknown additive keys inside a known version stay
    // tolerated: they are drift evidence, not a versioned facet.
    let mut v = happy03();
    v["someFutureAdditiveKey"] = Value::from(true);
    v["nodes"][0]["anotherFutureKey"] = Value::from(1);
    let g = decode_graph_value(v).expect("additive drift decodes");
    assert_eq!(g.schema_version, "0.3.0");
}

#[test]
fn ir_04_cooklang_capture_decodes_with_recipe_evidence() {
    let raw = std::fs::read(fixtures().join("ir/captures/cook-one-modeflag/graph.json")).unwrap();
    let g = decode_graph(&raw).expect("real IR 0.4 capture decodes");
    assert_eq!(g.schema_version, "0.4.0");
    assert!(g.frozen);
    assert_eq!(g.nodes.len(), 1);
    assert!(
        g.relations.is_empty(),
        "a relation-free 0.4 corpus omits the key"
    );

    let recipe = g.nodes[0].recipe.as_ref().expect("recipe facet retained");
    assert_eq!(recipe.ingredients.len(), 2);
    let water = &recipe.ingredients[0];
    assert_eq!(water.name, "water");
    // Quantities are strings exactly as authored — never numeric coercion.
    assert_eq!(water.quantity.amount, "2");
    assert_eq!(water.quantity.unit, "cups");
    assert_eq!(water.preparation, "");
    assert_eq!(water.recipe_ref, None);
    assert_eq!(recipe.ingredients[1].name, "salt");
    assert!(recipe.cookware.is_empty());
    assert!(recipe.timers.is_empty());
}

#[test]
fn ir_04_recipe_key_is_required_even_when_nullable() {
    let mut v = captured_graph_value("ir/captures/cook-one-modeflag/graph.json");
    // Null is conforming: present on every node once any page carries a
    // recipe, null for the pages that do not.
    v["nodes"][0]["recipe"] = Value::Null;
    let g = decode_graph_value(v).expect("present-null recipe decodes");
    assert_eq!(g.nodes[0].recipe, None);

    let mut v = captured_graph_value("ir/captures/cook-one-modeflag/graph.json");
    v["nodes"][0].as_object_mut().unwrap().remove("recipe");
    assert!(
        matches!(decode_graph_value(v), Err(GraphDecodeError::Json(_))),
        "a 0.4 node missing the required recipe key must not decode"
    );
}

#[test]
fn ir_04_node_keys_are_required_through_the_recipe_facets_flatten() {
    // The 0.4 wire node flattens the shared core; a missing required key
    // must still fail there exactly as it does under 0.2/0.3.
    for key in ["index", "parentIndex", "tags", "bodyOffset"] {
        let mut v = captured_graph_value("ir/captures/cook-one-modeflag/graph.json");
        v["nodes"][0].as_object_mut().unwrap().remove(key);
        assert!(
            matches!(decode_graph_value(v), Err(GraphDecodeError::Json(_))),
            "0.4 node missing required {key} must not decode"
        );
    }
}

#[test]
fn ir_04_relations_present_null_is_not_the_absent_case() {
    // Absent means the corpus carries no semantic relations; an array
    // means it does. Present-null is neither — the schema types the key
    // as an array.
    let mut v = captured_graph_value("ir/captures/cook-one-modeflag/graph.json");
    v["relations"] = Value::Null;
    assert!(
        matches!(decode_graph_value(v), Err(GraphDecodeError::Json(_))),
        "present-null relations under 0.4 must not decode"
    );
    let mut v = captured_graph_value("ir/captures/cook-one-modeflag/graph.json");
    v["relations"] = Value::Array(Vec::new());
    let g = decode_graph_value(v).expect("present empty array decodes");
    assert!(g.relations.is_empty());
}

#[test]
fn ir_04_tolerates_relations_present_and_absent() {
    // The real capture carries no `relations` key: a recipe corpus
    // without semantic relations. Boris emits the key only when the
    // corpus also carries relations; both shapes decode. The added
    // array mirrors what boris/0.9.0 emits for a two-page cook corpus
    // with `relations: [relates_to=stock]` frontmatter — probed against
    // the pinned engine in November 2026: schemaVersion stays 0.4.0,
    // `relations` appears with exactly this endpoint shape.
    let absent = captured_graph_value("ir/captures/cook-one-modeflag/graph.json");
    assert!(absent.get("relations").is_none());
    let g = decode_graph_value(absent).expect("absent relations decode");
    assert!(g.relations.is_empty());

    let mut present = captured_graph_value("ir/captures/cook-one-modeflag/graph.json");
    present["relations"] = serde_json::json!([{
        "from": {"type": "page", "value": "soup"},
        "to": {"type": "page", "value": "stock"},
        "kind": "relates_to"
    }]);
    let g = decode_graph_value(present).expect("present relations decode");
    assert_eq!(g.relations.len(), 1);
    assert_eq!(
        g.relations[0].kind,
        SemanticRelationKind::Token("relates_to".into())
    );
}

#[test]
fn duplicate_tags_decode_verbatim() {
    // Boris accepts and preserves duplicate tags in author order
    // (`tags: [repeat, repeat, other]`); the mirror must not drop,
    // reorder, or deduplicate that evidence.
    let g: Graph = serde_json::from_str(
        &std::fs::read_to_string(fixtures().join("editor/captures/duplicate-tags/graph.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(g.schema_version, "0.2.0");
    let trunk = g.nodes.iter().find(|n| n.id == "index").unwrap();
    assert_eq!(trunk.tags, ["repeat", "repeat", "other"]);
    let satellite = g
        .nodes
        .iter()
        .find(|n| n.id == "guides/non-adjacent")
        .unwrap();
    assert_eq!(satellite.tags, ["repeat", "other", "repeat"]);
}
