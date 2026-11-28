//! Deterministic synthetic graph-scale input for tests.
//!
//! This is deliberately **not** a Boris capture or JSON fixture: its schema
//! marker says so, it exists only as in-memory model input, and it is used to
//! scale-test CV's indexing and shell interaction costs without claiming
//! Boris emitted it.

use cv_contracts::graph::{
    DependencyEdge, DependencyKind, Endpoint, EndpointType, Graph, NavEntry, Node, NodeRole,
    ReverseIndexEntry, SemanticRelation, SemanticRelationKind,
};
use std::collections::BTreeMap;

pub const SCALE_NODES: usize = 5_000;
pub const SCALE_RELATIONS: usize = 30_000;
pub const TRUNK_DIRECT_SATELLITES: usize = 60;
const RELATIONS_PER_NODE: usize = 6;
const TAGS: usize = 16;

pub fn page_id(i: usize) -> String {
    format!("page-{i:05}")
}

fn endpoint(value: String) -> Endpoint {
    Endpoint {
        kind: EndpointType::Page,
        value,
    }
}

/// A corpus-shaped input with six outgoing semantic relations per node and
/// up to sixty direct satellites on its trunk. Node ids are zero-padded so
/// array order is also entity-id order, matching the deterministic ordering
/// Boris's real IR documents have without depending on that fact for
/// correctness.
pub fn synthetic_graph(node_count: usize) -> Graph {
    let trunk_satellites = TRUNK_DIRECT_SATELLITES.min(node_count.saturating_sub(1));
    let parents: Vec<Option<usize>> = (0..node_count)
        .map(|i| {
            if i == 0 {
                None
            } else if i <= trunk_satellites {
                Some(0)
            } else {
                Some(1 + (i - trunk_satellites - 1) % trunk_satellites.max(1))
            }
        })
        .collect();
    let nodes: Vec<Node> = (0..node_count)
        .map(|i| {
            let mut tags = vec![format!("tag-{:02}", i % TAGS)];
            if i % 100 == 0 {
                tags.push("century".into());
            }
            Node {
                index: i,
                id: page_id(i),
                source_path: format!("synthetic/{i:05}.md"),
                role: if parents[i].is_none() {
                    NodeRole::Trunk
                } else {
                    NodeRole::Satellite
                },
                parent: parents[i].map(page_id),
                parent_index: parents[i],
                title: Some(format!("Synthetic page {i:05}")),
                status: None,
                tags,
                body_offset: 0,
                recipe: None,
            }
        })
        .collect();

    let mut children: Vec<Vec<usize>> = vec![Vec::new(); node_count];
    for (child, parent) in parents.iter().enumerate() {
        if let Some(parent) = parent {
            children[*parent].push(child);
        }
    }
    let nav: Vec<NavEntry> = (0..node_count)
        .map(|i| {
            let mut breadcrumb = vec![i];
            let mut at = i;
            while let Some(parent) = parents[at] {
                breadcrumb.push(parent);
                at = parent;
            }
            breadcrumb.reverse();
            let siblings = parents[i]
                .map(|parent| {
                    children[parent]
                        .iter()
                        .copied()
                        .filter(|&sibling| sibling != i)
                        .collect()
                })
                .unwrap_or_default();
            NavEntry {
                index: i,
                id: page_id(i),
                breadcrumb,
                children: children[i].clone(),
                siblings,
            }
        })
        .collect();

    // Parent structure also appears as build-dependency edges in the real IR.
    // They are distinct from the semantic relations below.
    let edges: Vec<DependencyEdge> = (1..node_count)
        .map(|child| DependencyEdge {
            from: endpoint(page_id(child)),
            to: endpoint(page_id(
                parents[child].expect("non-root synthetic nodes have parents"),
            )),
            kind: DependencyKind::Parent,
        })
        .collect();
    let mut incoming: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, edge) in edges.iter().enumerate() {
        incoming
            .entry(edge.to.value.clone())
            .or_default()
            .push(index);
    }
    let reverse_index = incoming
        .into_iter()
        .map(|(target, incoming_edges)| ReverseIndexEntry {
            target: endpoint(target),
            incoming_edges,
        })
        .collect();

    let kinds = ["relates_to", "depends_on", "implements", "supersedes"];
    let mut relations = Vec::with_capacity(node_count * RELATIONS_PER_NODE);
    for source in 0..node_count {
        for j in 0..RELATIONS_PER_NODE {
            // 799 is coprime with both floor sizes exercised here (500 and
            // 5000), so each source's six targets are distinct without
            // randomness.
            let target = (source + 1 + j * 799) % node_count;
            relations.push(SemanticRelation {
                from: endpoint(page_id(source)),
                to: endpoint(page_id(target)),
                kind: SemanticRelationKind::Token(kinds[(source + j) % kinds.len()].into()),
            });
        }
    }
    relations.sort_by(|a, b| {
        (&a.from.value, &a.to.value, a.kind.as_str()).cmp(&(
            &b.from.value,
            &b.to.value,
            b.kind.as_str(),
        ))
    });
    Graph {
        schema_version: "synthetic-scale-v1".into(),
        frozen: true,
        nodes,
        edges,
        reverse_index,
        nav,
        relations,
    }
}

/// The #228 scale-floor input: exactly 5,000 nodes, 30,000 semantic
/// relations, and 60 direct satellites on the trunk.
pub fn synthetic_scale_graph() -> Graph {
    let graph = synthetic_graph(SCALE_NODES);
    assert_eq!(graph.relations.len(), SCALE_RELATIONS);
    graph
}
