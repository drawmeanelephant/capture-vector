//! Deterministic synthetic graph-scale input.
//!
//! This is deliberately **not** a Boris capture or JSON fixture: its schema
//! marker says so, it exists only as in-memory model input, and it is used to
//! scale-test CV's indexing data structure without claiming Boris emitted it.

use cv_contracts::graph::{Endpoint, Graph, NavEntry, Node, Relation, ReverseIndexEntry};
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
        kind: "page".into(),
        value,
    }
}

/// A corpus-shaped 5k/30k input. Node ids are zero-padded so array order is
/// also entity-id order, matching the deterministic ordering Boris's real IR
/// documents have without depending on that fact for correctness.
pub fn synthetic_scale_graph() -> Graph {
    let parents: Vec<Option<usize>> = (0..SCALE_NODES)
        .map(|i| match i {
            0 => None,
            1..=TRUNK_DIRECT_SATELLITES => Some(0),
            _ => Some(1 + (i - TRUNK_DIRECT_SATELLITES - 1) % 60),
        })
        .collect();
    let nodes: Vec<Node> = (0..SCALE_NODES)
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
                    "trunk"
                } else {
                    "satellite"
                }
                .into(),
                parent: parents[i].map(page_id),
                parent_index: parents[i],
                title: Some(format!("Synthetic page {i:05}")),
                status: None,
                tags,
                body_offset: 0,
            }
        })
        .collect();

    let mut children: Vec<Vec<usize>> = vec![Vec::new(); SCALE_NODES];
    for (child, parent) in parents.iter().enumerate() {
        if let Some(parent) = parent {
            children[*parent].push(child);
        }
    }
    let nav: Vec<NavEntry> = (0..SCALE_NODES)
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
    // They are distinct from the 30k semantic relations below.
    let edges: Vec<Relation> = (1..SCALE_NODES)
        .map(|child| Relation {
            from: endpoint(page_id(child)),
            to: endpoint(page_id(
                parents[child].expect("non-root synthetic nodes have parents"),
            )),
            kind: "parent".into(),
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
    let mut relations = Vec::with_capacity(SCALE_RELATIONS);
    for source in 0..SCALE_NODES {
        for j in 0..RELATIONS_PER_NODE {
            // 799 is coprime with 5000, so each source's six targets are
            // distinct and evenly distributed without randomness.
            let target = (source + 1 + j * 799) % SCALE_NODES;
            relations.push(Relation {
                from: endpoint(page_id(source)),
                to: endpoint(page_id(target)),
                kind: kinds[(source + j) % kinds.len()].into(),
            });
        }
    }
    assert_eq!(relations.len(), SCALE_RELATIONS);
    relations.sort_by(|a, b| {
        (&a.from.value, &a.to.value, &a.kind).cmp(&(&b.from.value, &b.to.value, &b.kind))
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
