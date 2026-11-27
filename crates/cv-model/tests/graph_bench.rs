//! Graph indexing/update cost at the #228 scale floor. Ignored by default;
//! run in release for meaningful numbers:
//! `cargo test --release -p cv-model --test graph_bench -- --ignored --nocapture`.
//!
//! The assertions pin the scale shape only. Performance evidence is the
//! printed median cost; a passing process exit is not treated as a scale proof.

mod support;

use cv_contracts::graph::Graph;
use cv_model::graph::{GraphNavIndex, RelationDirection};
use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::graph_scale::{SCALE_NODES, SCALE_RELATIONS, page_id, synthetic_scale_graph};

const RUNS: usize = 7;
const UPDATES: usize = 20;

fn median(mut runs: Vec<Duration>) -> Duration {
    runs.sort();
    runs[runs.len() / 2]
}

fn build_cost(graph: &Arc<Graph>) -> Duration {
    let start = Instant::now();
    let index = GraphNavIndex::build(graph.clone());
    black_box(index.stats());
    start.elapsed()
}

fn update_cost(a: &Arc<Graph>, b: &Arc<Graph>) -> Duration {
    let mut index = GraphNavIndex::build(a.clone());
    let start = Instant::now();
    for i in 0..UPDATES {
        let next = if i % 2 == 0 { b } else { a };
        black_box(index.update(Some(next.clone())));
    }
    start.elapsed() / UPDATES as u32
}

fn lookup_cost(index: &GraphNavIndex) -> Duration {
    const LOOKUPS: usize = 10_000;
    let start = Instant::now();
    for i in 0..LOOKUPS {
        black_box(index.node(&page_id(i % SCALE_NODES)));
    }
    start.elapsed() / LOOKUPS as u32
}

fn neighborhood_cost(index: &GraphNavIndex) -> Duration {
    const LOOKUPS: usize = 1_000;
    let start = Instant::now();
    for i in 0..LOOKUPS {
        let id = page_id(i % SCALE_NODES);
        black_box(
            index
                .relations_of(&id, RelationDirection::Any)
                .collect::<Vec<_>>(),
        );
    }
    start.elapsed() / LOOKUPS as u32
}

#[test]
#[ignore = "benchmark; run in release with --ignored"]
fn graph_index_and_update_cost_at_scale_floor() {
    let full = Arc::new(synthetic_scale_graph());
    let mut changed_shape = synthetic_scale_graph();
    changed_shape.nodes[0].title = Some("Changed deterministic trunk".into());
    let changed = Arc::new(changed_shape);
    let mut without_relations = synthetic_scale_graph();
    without_relations.relations.clear();
    let nodes_only = Arc::new(without_relations);

    let initial = median((0..RUNS).map(|_| build_cost(&full)).collect());
    let node_index_only = median((0..RUNS).map(|_| build_cost(&nodes_only)).collect());
    let update = median((0..RUNS).map(|_| update_cost(&full, &changed)).collect());
    let index = GraphNavIndex::build(full);
    let lookup = median((0..RUNS).map(|_| lookup_cost(&index)).collect());
    let neighborhood = median((0..RUNS).map(|_| neighborhood_cost(&index)).collect());
    let stats = index.stats();
    assert_eq!(
        (stats.nodes, stats.semantic_relations),
        (SCALE_NODES, SCALE_RELATIONS)
    );

    let relation_index_marginal = initial.saturating_sub(node_index_only);
    eprintln!(
        "bench graph-index scale floor: initial {initial:?}; whole-graph update {update:?}; \
         node lookup {lookup:?}; incident-neighborhood collect {neighborhood:?}; \
         nodes/relations {SCALE_NODES}/{SCALE_RELATIONS}"
    );
    eprintln!(
        "bench graph-index limiter evidence: node/tag/title/hierarchy-only build \
         {node_index_only:?}; marginal semantic-relation adjacency+kind indexing \
         {relation_index_marginal:?}. The limiting factor is whole-graph \
         replacement: update costs about as much as the initial index. Within \
         that rebuild the measured dominant component is semantic-relation \
         adjacency+kind indexing; Boris supplies no patch contract, so update \
         is not incremental."
    );
}
