//! Behavior of the indexed Boris graph navigation model: real captured IR
//! for authority, deterministic in-memory synthetic input for scale.

mod support;

use cv_contracts::graph::Graph;
use cv_core::id::{AccountId, StableId};
use cv_model::graph::{GraphNavIndex, RelationDirection};
use cv_model::module::{FieldSource, PageRecord, ProjectSnapshot, Registry};
use cv_model::workbench::Workbench;
use std::sync::Arc;
use support::graph_scale::{
    SCALE_NODES, SCALE_RELATIONS, TRUNK_DIRECT_SATELLITES, page_id, synthetic_scale_graph,
};

fn captured_ir_graph(name: &str) -> Graph {
    serde_json::from_slice(&cv_fixtures::ir_capture(name).file("graph.json"))
        .expect("captured Boris IR graph decodes")
}

fn captured_happy_graph() -> Graph {
    captured_ir_graph("happy")
}

fn ids<'a>(nodes: impl Iterator<Item = cv_model::graph::GraphNode<'a>>) -> Vec<&'a str> {
    nodes.map(|n| n.id).collect()
}

#[test]
fn captured_boris_ir_indexes_navigation_facts_without_reinterpretation() {
    let index = GraphNavIndex::build(Arc::new(captured_happy_graph()));

    assert_eq!(index.stats().nodes, 3);
    assert_eq!(index.stats().semantic_relations, 1);
    assert_eq!(
        ids(index.children_of("index")),
        ["guides/getting-started", "guides/publishing"]
    );
    assert_eq!(
        index.parent_of("guides/publishing").map(|n| n.id),
        Some("index")
    );
    assert_eq!(
        ids(index.nodes_titled("My Boris Site")),
        ["index"],
        "title lookup is exact; search normalization belongs to a later lane"
    );
    assert_eq!(
        ids(index.nodes_tagged("guides")),
        ["guides/getting-started", "guides/publishing"]
    );
    assert_eq!(
        ids(index.breadcrumb_of("guides/publishing")),
        ["index", "guides/publishing"]
    );
    assert_eq!(
        ids(index.siblings_of("guides/publishing")),
        ["guides/getting-started"]
    );
    assert_eq!(index.node_at_index(2).map(|node| node.id), Some("index"));

    let outgoing: Vec<_> = index
        .relations_of_kind(
            "guides/publishing",
            "relates_to",
            RelationDirection::Outgoing,
        )
        .collect();
    assert_eq!(outgoing.len(), 1);
    let relation = outgoing[0];
    assert_eq!(relation.kind, "relates_to");
    assert_eq!(
        (relation.from.kind.as_str(), relation.from.value.as_str()),
        ("page", "guides/publishing")
    );
    assert_eq!(
        (relation.to.kind.as_str(), relation.to.value.as_str()),
        ("page", "guides/getting-started")
    );
    assert_eq!(
        index
            .relations_of("guides/publishing", RelationDirection::Incoming)
            .count(),
        0
    );
    assert_eq!(
        index
            .relations_of_kind(
                "guides/getting-started",
                "relates_to",
                RelationDirection::Incoming
            )
            .map(|r| r.ordinal)
            .collect::<Vec<_>>(),
        [relation.ordinal]
    );

    let publishing = index.node("guides/publishing").expect("page id indexed");
    assert_eq!(publishing.index, 1);
    assert_eq!(publishing.role, "satellite");
    assert!(index.graph().expect("graph retained").frozen);
    assert_eq!(publishing.degree.outgoing, 1);
    assert_eq!(publishing.degree.incoming, 0);
    assert_eq!(publishing.degree.total, 1);
    assert_eq!(publishing.child_count, 0);
    assert_eq!(index.relation_kinds().collect::<Vec<_>>(), ["relates_to"]);
}

#[test]
fn relation_free_ir_02_decodes_with_nav_and_empty_semantic_relations() {
    let index = GraphNavIndex::build(Arc::new(captured_ir_graph("dogfood")));
    let stats = index.stats();
    assert_eq!(stats.nodes, 45);
    assert_eq!(stats.semantic_relations, 0);
    assert_eq!(index.relation_kinds().count(), 0);

    let agents = index.node("agents").expect("trunk indexed");
    assert_eq!(agents.role, "trunk");
    assert_eq!(index.children_of("agents").count(), 30);
    assert_eq!(ids(index.breadcrumb_of("agents")), ["agents"]);
    assert_eq!(index.siblings_of("agents").count(), 0);
    assert_eq!(index.nodes().filter(|node| node.role == "trunk").count(), 7);
}

#[test]
fn synthetic_scale_index_covers_the_scale_floor_shape() {
    let index = GraphNavIndex::build(Arc::new(synthetic_scale_graph()));
    let stats = index.stats();
    assert_eq!(stats.nodes, SCALE_NODES);
    assert_eq!(stats.semantic_relations, SCALE_RELATIONS);
    assert_eq!(stats.relation_kinds, 4);
    assert_eq!(stats.hierarchy_edges, SCALE_NODES - 1);

    let trunk = page_id(0);
    assert_eq!(
        index.children_of(&trunk).count(),
        TRUNK_DIRECT_SATELLITES,
        "the synthetic trunk carries exactly 60 direct satellites"
    );
    assert_eq!(
        index.nodes_with_degree(0).count(),
        0,
        "the scale construction connects every node semantically"
    );
    let degree_sum: usize = index.nodes().map(|n| n.degree.total).sum();
    assert_eq!(degree_sum, SCALE_RELATIONS * 2);
    assert!(index.nodes_titled("Synthetic page 01234").next().is_some());
    assert!(index.nodes_tagged("tag-07").count() > 300);
    assert_eq!(
        index
            .relations_of(&page_id(0), RelationDirection::Outgoing)
            .count(),
        6
    );
    assert_eq!(
        index
            .relations_by_kind("relates_to")
            .chain(index.relations_by_kind("depends_on"))
            .chain(index.relations_by_kind("implements"))
            .chain(index.relations_by_kind("supersedes"))
            .count(),
        SCALE_RELATIONS
    );
}

#[test]
fn replacing_the_frozen_graph_updates_the_whole_index_deterministically() {
    let first = Arc::new(synthetic_scale_graph());
    let mut index = GraphNavIndex::build(first.clone());
    let generation = index.generation();

    let mut next = synthetic_scale_graph();
    next.nodes[123].title = Some("Retitled deterministic node".into());
    next.relations[0].kind = "depends_on".into();
    let update = index.update(Some(Arc::new(next)));
    assert!(update.changed);
    assert_eq!(update.generation, generation + 1);
    assert_eq!(
        index
            .nodes_titled("Retitled deterministic node")
            .next()
            .map(|n| n.id.to_string()),
        Some(page_id(123))
    );
    assert_eq!(index.stats().nodes, SCALE_NODES);
    assert_eq!(index.stats().semantic_relations, SCALE_RELATIONS);

    let same = first.clone();
    // Re-presenting the identical object is the only cheap path.
    let mut second = GraphNavIndex::build(same);
    let generation_before_noop = second.generation();
    assert!(!second.update(Some(first)).changed);
    assert_eq!(second.generation(), generation_before_noop);
}

#[test]
fn graph_node_identity_round_trips_through_shell_page_selection() {
    let graph = Arc::new(captured_happy_graph());
    let pages: Vec<PageRecord> = graph
        .nodes
        .iter()
        .map(|node| PageRecord {
            path: node.source_path.clone(),
            id: node.id.clone(),
            title: node.title.clone().unwrap_or_else(|| node.id.clone()),
            parent: node.parent.clone(),
            tags: node.tags.clone(),
            fields: FieldSource::Boris,
            ..Default::default()
        })
        .collect();
    let mut workbench = Workbench::new(Registry::with_all());
    workbench.load_snapshot(&ProjectSnapshot {
        epoch: 1,
        pages,
        ir_graph: Some(graph),
        ..Default::default()
    });
    assert_eq!(workbench.app.graph.stats().nodes, 3);

    let pages_index = workbench
        .registry
        .index_of("pages")
        .expect("pages registered");
    workbench.activate(pages_index);
    workbench.select_key("guides/publishing.md");
    let shell_noun = workbench
        .app
        .selection
        .noun
        .clone()
        .expect("selecting the Pages row emits a page noun");
    assert_eq!(shell_noun.stable, StableId("guides/publishing".into()));

    let graph_selection = workbench
        .app
        .graph
        .selection(
            AccountId("local".into()),
            "guides/publishing",
            Some("graph".into()),
        )
        .expect("graph node emits a selection");
    assert_eq!(graph_selection.noun.as_ref(), Some(&shell_noun));
    let pages = workbench.registry.get(pages_index).expect("pages module");
    let graph_noun = graph_selection.noun.as_ref().expect("graph noun");
    let pages_key = pages
        .key_for(graph_noun)
        .expect("Pages resolves the stable graph noun back to its row key");
    assert_eq!(pages_key, "guides/publishing.md");
    assert_eq!(pages.noun(&pages_key).as_ref(), Some(graph_noun));
    assert_eq!(
        workbench.app.graph.key_for_noun(&shell_noun),
        Some(StableId("guides/publishing".into()))
    );
}
