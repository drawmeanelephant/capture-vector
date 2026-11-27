//! Indexed navigation model over Boris's frozen IR graph.
//!
//! The authority boundary is deliberately narrow: the input is the
//! `graph.json` shape decoded by `cv-contracts`; this index stores positions
//! into that immutable document and copies only lookup keys. It does not
//! validate Boris topology, infer reciprocal relations, or interpret relation
//! kinds. Parentage uses Boris's frozen `parentIndex`; children, breadcrumbs
//! and siblings use Boris's authoritative `nav` projection rather than being
//! re-derived in Rust. Semantic-relation incoming adjacency is the reverse
//! lookup the IR contract leaves to consumers. `relations` remain distinct
//! from dependency `edges` here, exactly as they are in Boris.
//!
//! Schema policy for pinned Boris 0.9 IR: node/nav/dependency shape is shared
//! by IR 0.2+; semantic `relations` default to empty for relation-free 0.2
//! artifacts; the additive 0.4 recipe facet is tolerated and ignored. A future
//! breaking schema needs a `cv-contracts` review rather than inference here.

use cv_contracts::graph::{Endpoint, Graph};
use cv_core::id::{AccountId, NounId, NounKind, Selection, StableId, noun_kind};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Incident-direction filter for semantic relations. These are orientations
/// already present in Boris's `from`/`to` endpoints, not inferred meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RelationDirection {
    Outgoing,
    Incoming,
    Any,
}

/// Precomputed semantic-relation degree for one page node.
///
/// `total` is unique incident relations. Boris rejects self-relations before
/// freezing, so it is also `incoming + outgoing` for a valid frozen graph.
/// Parent/child and dependency edges are not counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord, Hash)]
pub struct RelationDegree {
    pub outgoing: usize,
    pub incoming: usize,
    pub total: usize,
}

/// A borrowed node view. `index` is Boris's stable node index inside this
/// frozen graph; `position` is the array position carrying it (the same in a
/// valid frozen graph). Neither is cross-build identity: use `id` for that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphNode<'a> {
    pub position: usize,
    pub index: usize,
    pub id: &'a str,
    pub source_path: &'a str,
    pub role: &'a str,
    pub title: Option<&'a str>,
    pub status: Option<&'a str>,
    pub tags: &'a [String],
    pub parent: Option<&'a str>,
    pub body_offset: usize,
    pub child_count: usize,
    pub degree: RelationDegree,
}

/// A borrowed semantic-relation view. `ordinal` is the relation's position
/// in Boris's canonically ordered `relations` array for this graph snapshot;
/// it is positional evidence, not a cross-build identifier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GraphRelation<'a> {
    pub ordinal: usize,
    pub kind: &'a str,
    pub from: &'a Endpoint,
    pub to: &'a Endpoint,
}

/// Shape statistics for scale evidence and degraded-surface bookkeeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GraphIndexStats {
    pub nodes: usize,
    pub semantic_relations: usize,
    pub relation_kinds: usize,
    pub distinct_titles: usize,
    pub distinct_tags: usize,
    pub hierarchy_edges: usize,
    pub max_degree: usize,
    pub max_children: usize,
}

/// Result of one [`GraphNavIndex::update`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphUpdate {
    pub generation: u64,
    /// `false` only when the same `Arc` (or absence of graph) was presented
    /// again. Equal bytes in a fresh graph object are conservatively indexed
    /// as a change; Boris emits a whole frozen graph rather than a patch.
    pub changed: bool,
    pub stats: GraphIndexStats,
}

/// Indexes over one immutable Boris IR graph.
///
/// The index is rebuilt as a whole when a new frozen graph arrives because
/// Boris has no incremental graph-patch contract. Lookup vectors retain
/// Boris's deterministic node/relation order, so equal input produces equal
/// navigation output without iterating hash maps.
#[derive(Debug, Clone, Default)]
pub struct GraphNavIndex {
    graph: Option<Arc<Graph>>,
    generation: u64,
    by_id: BTreeMap<String, usize>,
    by_node_index: BTreeMap<usize, usize>,
    by_title: BTreeMap<String, Vec<usize>>,
    by_tag: BTreeMap<String, Vec<usize>>,
    by_degree: BTreeMap<usize, Vec<usize>>,
    parent: Vec<Option<usize>>,
    nav_position: Vec<Option<usize>>,
    child_count: Vec<usize>,
    outgoing: Vec<Vec<usize>>,
    incoming: Vec<Vec<usize>>,
    incident: Vec<Vec<usize>>,
    degree: Vec<RelationDegree>,
    relations_by_kind: BTreeMap<String, Vec<usize>>,
}

impl GraphNavIndex {
    pub fn build(graph: Arc<Graph>) -> Self {
        let mut index = Self::default();
        index.update(Some(graph));
        index
    }

    /// Replace the frozen graph projection. A whole re-index is the honest
    /// update operation: Boris republishes a whole frozen `graph.json` and
    /// does not emit node/relation patches. Presenting the same `Arc` again
    /// is the only cheap no-op.
    pub fn update(&mut self, graph: Option<Arc<Graph>>) -> GraphUpdate {
        let unchanged = match (&self.graph, &graph) {
            (None, None) => true,
            (Some(old), Some(new)) => Arc::ptr_eq(old, new),
            _ => false,
        };
        if unchanged {
            return GraphUpdate {
                generation: self.generation,
                changed: false,
                stats: self.stats(),
            };
        }
        self.graph = graph;
        self.generation += 1;
        self.rebuild();
        GraphUpdate {
            generation: self.generation,
            changed: true,
            stats: self.stats(),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn graph(&self) -> Option<&Arc<Graph>> {
        self.graph.as_ref()
    }

    pub fn stats(&self) -> GraphIndexStats {
        let nodes = self.graph.as_ref().map_or(0, |g| g.nodes.len());
        let semantic_relations = self.graph.as_ref().map_or(0, |g| g.relations.len());
        GraphIndexStats {
            nodes,
            semantic_relations,
            relation_kinds: self.relations_by_kind.len(),
            distinct_titles: self.by_title.len(),
            distinct_tags: self.by_tag.len(),
            hierarchy_edges: self.parent.iter().flatten().count(),
            max_degree: self.degree.iter().map(|d| d.total).max().unwrap_or(0),
            max_children: self.child_count.iter().copied().max().unwrap_or(0),
        }
    }

    /// Exact page-id lookup. The id is Boris's stable page entity id, not a
    /// filesystem path or array position.
    pub fn node(&self, id: &str) -> Option<GraphNode<'_>> {
        self.by_id.get(id).and_then(|&pos| self.node_at(pos))
    }

    /// Lookup by Boris's frozen node index. This is positional evidence for
    /// nav/dependency indexes, not cross-build identity.
    pub fn node_at_index(&self, index: usize) -> Option<GraphNode<'_>> {
        self.by_node_index
            .get(&index)
            .and_then(|&pos| self.node_at(pos))
    }

    pub fn nodes(&self) -> impl Iterator<Item = GraphNode<'_>> + '_ {
        let len = self.graph.as_ref().map_or(0, |g| g.nodes.len());
        (0..len).filter_map(move |pos| self.node_at(pos))
    }

    /// Exact title lookup. Titles are display keys, not identity; several
    /// nodes can share one title. Case folding/substring search is a later
    /// search-lane concern and is not invented here.
    pub fn nodes_titled(&self, title: &str) -> impl Iterator<Item = GraphNode<'_>> + '_ {
        let positions = self.by_title.get(title).map(Vec::as_slice);
        positions_of(positions).filter_map(move |&pos| self.node_at(pos))
    }

    /// Exact tag lookup. Tags remain case-sensitive author tokens.
    pub fn nodes_tagged(&self, tag: &str) -> impl Iterator<Item = GraphNode<'_>> + '_ {
        let positions = self.by_tag.get(tag).map(Vec::as_slice);
        positions_of(positions).filter_map(move |&pos| self.node_at(pos))
    }

    /// Nodes whose unique incident semantic-relation degree is exactly
    /// `degree`, in Boris node order.
    pub fn nodes_with_degree(&self, degree: usize) -> impl Iterator<Item = GraphNode<'_>> + '_ {
        let positions = self.by_degree.get(&degree).map(Vec::as_slice);
        positions_of(positions).filter_map(move |&pos| self.node_at(pos))
    }

    pub fn parent_of(&self, id: &str) -> Option<GraphNode<'_>> {
        let &pos = self.by_id.get(id)?;
        self.parent[pos].and_then(|parent| self.node_at(parent))
    }

    /// Direct children in Boris's authoritative `nav.children` order. The
    /// immutable graph owns the adjacency array; this resolves its node
    /// indices without copying a dense hub's payload into a parallel index.
    pub fn children_of(&self, id: &str) -> impl Iterator<Item = GraphNode<'_>> + '_ {
        self.nav_entry(id)
            .into_iter()
            .flat_map(|entry| entry.children.iter())
            .filter_map(move |&index| self.node_at_index(index))
    }

    /// Root-to-self inclusive navigation chain from Boris's `nav.breadcrumb`.
    pub fn breadcrumb_of(&self, id: &str) -> impl Iterator<Item = GraphNode<'_>> + '_ {
        self.nav_entry(id)
            .into_iter()
            .flat_map(|entry| entry.breadcrumb.iter())
            .filter_map(move |&index| self.node_at_index(index))
    }

    /// Direct peers from Boris's `nav.siblings` (already excluding self).
    pub fn siblings_of(&self, id: &str) -> impl Iterator<Item = GraphNode<'_>> + '_ {
        self.nav_entry(id)
            .into_iter()
            .flat_map(|entry| entry.siblings.iter())
            .filter_map(move |&index| self.node_at_index(index))
    }

    /// Semantic relations incident to one page in the requested orientation.
    /// `Any` uses the precomputed unique incident set, so a malformed
    /// self-loop cannot appear twice even though valid Boris graphs reject it.
    pub fn relations_of(
        &self,
        id: &str,
        direction: RelationDirection,
    ) -> impl Iterator<Item = GraphRelation<'_>> + '_ {
        let positions = self.by_id.get(id).map(|&pos| match direction {
            RelationDirection::Outgoing => self.outgoing[pos].as_slice(),
            RelationDirection::Incoming => self.incoming[pos].as_slice(),
            RelationDirection::Any => self.incident[pos].as_slice(),
        });
        positions_of(positions).filter_map(move |&ord| self.relation_at(ord))
    }

    /// Semantic relations incident to one page, filtered by Boris's opaque
    /// relation-kind token and endpoint orientation.
    pub fn relations_of_kind<'a>(
        &'a self,
        id: &str,
        kind: &'a str,
        direction: RelationDirection,
    ) -> impl Iterator<Item = GraphRelation<'a>> + 'a {
        self.relations_of(id, direction)
            .filter(move |relation| relation.kind == kind)
    }

    /// Every relation carrying `kind`, whether or not its endpoints resolve
    /// to a page node present in this graph. Endpoint validation remains
    /// Boris's job; the kind index does not discard evidence.
    pub fn relations_by_kind(&self, kind: &str) -> impl Iterator<Item = GraphRelation<'_>> + '_ {
        let positions = self.relations_by_kind.get(kind).map(Vec::as_slice);
        positions_of(positions).filter_map(move |&ord| self.relation_at(ord))
    }

    pub fn relation_kinds(&self) -> impl Iterator<Item = &str> + '_ {
        self.relations_by_kind.keys().map(String::as_str)
    }

    /// The shell page noun for a node present in this graph. This reuses the
    /// existing page-noun stable id (`Boris entity id`) rather than inventing
    /// a parallel graph-node identity.
    pub fn noun(&self, account: AccountId, page_id: &str) -> Option<NounId> {
        self.node(page_id)?;
        Some(NounId {
            account,
            kind: NounKind::register(noun_kind::PAGE),
            stable: StableId(page_id.into()),
        })
    }

    /// A shell `Selection` for a graph node. The mailbox string belongs to
    /// the future graph surface; the noun is the same page noun Pages emits.
    pub fn selection(
        &self,
        account: AccountId,
        page_id: &str,
        mailbox: Option<String>,
    ) -> Option<Selection> {
        let noun = self.noun(account, page_id)?;
        Some(Selection {
            account: Some(noun.account.clone()),
            mailbox,
            noun: Some(noun),
        })
    }

    /// Round-trip a shell page noun back to the graph key it selects. The
    /// method returns `None` for non-page nouns and for page ids absent from
    /// the current frozen graph.
    pub fn key_for_noun(&self, noun: &NounId) -> Option<StableId> {
        if noun.kind.as_str() != noun_kind::PAGE || self.node(&noun.stable.0).is_none() {
            return None;
        }
        Some(noun.stable.clone())
    }

    fn rebuild(&mut self) {
        self.clear_indexes();
        let Some(graph) = self.graph.clone() else {
            return;
        };
        self.index_nodes(&graph.nodes);
        self.index_hierarchy(&graph);
        self.index_relations(&graph.relations);
    }

    fn clear_indexes(&mut self) {
        self.by_id.clear();
        self.by_node_index.clear();
        self.by_title.clear();
        self.by_tag.clear();
        self.by_degree.clear();
        self.parent.clear();
        self.nav_position.clear();
        self.child_count.clear();
        self.outgoing.clear();
        self.incoming.clear();
        self.incident.clear();
        self.degree.clear();
        self.relations_by_kind.clear();
    }

    fn index_nodes(&mut self, nodes: &[cv_contracts::graph::Node]) {
        for (pos, node) in nodes.iter().enumerate() {
            // A valid frozen Boris graph has unique ids. First-win keeps a
            // structurally malformed but decodable document deterministic
            // without turning the index into a validator.
            self.by_id.entry(node.id.clone()).or_insert(pos);
            self.by_node_index.entry(node.index).or_insert(pos);
            if let Some(title) = node.title.as_deref().filter(|title| !title.is_empty()) {
                push_position(&mut self.by_title, title, pos);
            }
            for tag in &node.tags {
                push_position(&mut self.by_tag, tag, pos);
            }
        }
    }

    fn index_hierarchy(&mut self, graph: &Graph) {
        let len = graph.nodes.len();
        self.parent = vec![None; len];
        self.nav_position = vec![None; len];
        self.child_count = vec![0; len];
        for (pos, node) in graph.nodes.iter().enumerate() {
            let Some(parent_index) = node.parent_index else {
                continue;
            };
            let Some(&parent_pos) = self.by_node_index.get(&parent_index) else {
                continue;
            };
            if parent_pos != pos {
                self.parent[pos] = Some(parent_pos);
            }
        }
        // `nav` is Boris's authoritative derived navigation projection. Keep
        // only node-position lookup metadata; the immutable graph already owns
        // the child/breadcrumb/sibling arrays, so copying them would duplicate
        // the largest hub payloads without adding authority.
        for (nav_pos, entry) in graph.nav.iter().enumerate() {
            let Some(&pos) = self.by_node_index.get(&entry.index) else {
                continue;
            };
            self.nav_position[pos] = Some(nav_pos);
            self.child_count[pos] = entry.children.len();
        }
    }

    fn index_relations(&mut self, relations: &[cv_contracts::graph::Relation]) {
        let len = self.graph.as_ref().map_or(0, |g| g.nodes.len());
        self.outgoing = vec![Vec::new(); len];
        self.incoming = vec![Vec::new(); len];
        self.incident = vec![Vec::new(); len];
        self.degree = vec![RelationDegree::default(); len];
        for (ordinal, relation) in relations.iter().enumerate() {
            push_position(&mut self.relations_by_kind, &relation.kind, ordinal);
            if relation.from.kind == "page"
                && let Some(&from) = self.by_id.get(&relation.from.value)
            {
                self.outgoing[from].push(ordinal);
            }
            if relation.to.kind == "page"
                && let Some(&to) = self.by_id.get(&relation.to.value)
            {
                self.incoming[to].push(ordinal);
            }
        }
        for pos in 0..len {
            let outgoing = self.outgoing[pos].len();
            let incoming = self.incoming[pos].len();
            self.incident[pos].extend(self.outgoing[pos].iter().copied());
            self.incident[pos].extend(self.incoming[pos].iter().copied());
            self.incident[pos].sort_unstable();
            self.incident[pos].dedup();
            let total = self.incident[pos].len();
            self.degree[pos] = RelationDegree {
                outgoing,
                incoming,
                total,
            };
            self.by_degree.entry(total).or_default().push(pos);
        }
    }

    fn nav_entry(&self, id: &str) -> Option<&cv_contracts::graph::NavEntry> {
        let &pos = self.by_id.get(id)?;
        let nav_pos = self.nav_position.get(pos).copied().flatten()?;
        self.graph.as_ref()?.nav.get(nav_pos)
    }

    fn node_at(&self, pos: usize) -> Option<GraphNode<'_>> {
        let graph = self.graph.as_ref()?;
        let node = graph.nodes.get(pos)?;
        Some(GraphNode {
            position: pos,
            index: node.index,
            id: &node.id,
            source_path: &node.source_path,
            role: &node.role,
            title: node.title.as_deref(),
            status: node.status.as_deref(),
            tags: &node.tags,
            parent: node.parent.as_deref(),
            body_offset: node.body_offset,
            child_count: self.child_count.get(pos).copied().unwrap_or(0),
            degree: self.degree.get(pos).copied().unwrap_or_default(),
        })
    }

    fn relation_at(&self, ordinal: usize) -> Option<GraphRelation<'_>> {
        let relation = self.graph.as_ref()?.relations.get(ordinal)?;
        Some(GraphRelation {
            ordinal,
            kind: &relation.kind,
            from: &relation.from,
            to: &relation.to,
        })
    }
}

fn positions_of(positions: Option<&[usize]>) -> impl Iterator<Item = &usize> {
    positions.unwrap_or(&[]).iter()
}

/// Push into a multi-index without cloning duplicate keys. Scale corpora have
/// many repeated tags and relation kinds; `BTreeMap::entry` would clone the
/// key before discovering that a bucket already exists.
fn push_position(map: &mut BTreeMap<String, Vec<usize>>, key: &str, pos: usize) {
    if let Some(positions) = map.get_mut(key) {
        positions.push(pos);
    } else {
        map.insert(key.to_string(), vec![pos]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cv_contracts::graph::{Endpoint, NavEntry, Node, Relation};

    fn endpoint(value: &str) -> Endpoint {
        Endpoint {
            kind: "page".into(),
            value: value.into(),
        }
    }

    fn node(index: usize, id: &str, parent: Option<(&str, usize)>) -> Node {
        Node {
            index,
            id: id.into(),
            source_path: format!("{id}.md"),
            role: if parent.is_some() {
                "satellite"
            } else {
                "trunk"
            }
            .into(),
            parent: parent.map(|(id, _)| id.to_string()),
            parent_index: parent.map(|(_, index)| index),
            title: Some(format!("Title {id}")),
            status: None,
            tags: vec!["tag".into()],
            body_offset: 0,
        }
    }

    fn nav(
        index: usize,
        id: &str,
        breadcrumb: &[usize],
        children: &[usize],
        siblings: &[usize],
    ) -> NavEntry {
        NavEntry {
            index,
            id: id.into(),
            breadcrumb: breadcrumb.into(),
            children: children.into(),
            siblings: siblings.into(),
        }
    }

    fn small_graph() -> Graph {
        Graph {
            schema_version: "synthetic-unit-v1".into(),
            frozen: true,
            nodes: vec![
                node(0, "index", None),
                node(1, "a", Some(("index", 0))),
                node(2, "b", Some(("index", 0))),
            ],
            edges: Vec::new(),
            reverse_index: Vec::new(),
            nav: vec![
                nav(0, "index", &[0], &[1, 2], &[]),
                nav(1, "a", &[0, 1], &[], &[2]),
                nav(2, "b", &[0, 2], &[], &[1]),
            ],
            relations: vec![Relation {
                from: endpoint("a"),
                to: endpoint("b"),
                kind: "relates_to".into(),
            }],
        }
    }

    #[test]
    fn update_to_none_clears_every_index() {
        let mut index = GraphNavIndex::build(Arc::new(small_graph()));
        assert_eq!(index.stats().nodes, 3);
        let update = index.update(None);
        assert!(update.changed);
        assert_eq!(index.stats(), GraphIndexStats::default());
        assert!(index.node("index").is_none());
        assert_eq!(index.relations_of("a", RelationDirection::Any).count(), 0);
    }

    #[test]
    fn the_same_graph_arc_is_a_noop_update() {
        let graph = Arc::new(small_graph());
        let mut index = GraphNavIndex::build(graph.clone());
        let generation = index.generation();
        let update = index.update(Some(graph));
        assert!(!update.changed);
        assert_eq!(update.generation, generation);
    }
}
