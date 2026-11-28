//! Graph — a bounded, interactive viewport over Boris's frozen IR graph.
//!
//! The module consumes the already-decoded graph authority. Its layout is
//! deterministic presentation state: hierarchy supplies the corpus scaffold,
//! semantic relations supply adjacency, and the viewport planner decides what
//! may mount. Nothing here revalidates Boris topology or interprets opaque
//! relation kinds.

use crate::graph::GraphNavIndex;
use crate::module::{
    InspectorModel, Module, ModuleSpec, NavCache, NavItem, NavModel, ProjectSnapshot, ReadingLine,
    ReadingModel, Registry, RowModel, Slot, Tone,
};
use crate::viewport::{
    Camera, LayoutNode, LayoutRelation, ViewportDragPhase, ViewportInput, ViewportModel,
    cluster_cell_world, plan_viewport,
};
use cv_contracts::graph::Graph;
use cv_core::events::Envelope;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub const ID: &str = "graph";
const CORPUS: &str = "corpus";
const NEIGHBORHOOD: &str = "neighborhood";
const ROW_H: f32 = 44.0;
const NEIGHBOR_ROWS: usize = 18;
const GRID_COLS: usize = 64;
const GRID_W: f32 = 132.0;
const GRID_H: f32 = 108.0;

pub fn register(r: &mut Registry) {
    r.register(Box::new(GraphModule::default()))
        .expect("graph module id is unique");
}

#[derive(Default)]
pub struct GraphModule {
    index: GraphNavIndex,
    graph: Option<Arc<Graph>>,
    positions: Vec<[f32; 2]>,
    corpus_nodes: Vec<LayoutNode>,
    relations: Vec<LayoutRelation>,
    order: Vec<String>,
    focus: Option<String>,
    selected: Option<String>,
    viewport: [f32; 2],
    layout_center: [f32; 2],
    drag_last: Option<[f32; 2]>,
    corpus_camera: Option<Camera>,
    neighborhood_camera: Option<Camera>,
    nav_cache: NavCache,
}

impl GraphModule {
    fn camera(&self, nav: Option<&str>) -> Camera {
        let fallback = self.layout_center;
        match nav.unwrap_or(CORPUS) {
            NEIGHBORHOOD => self
                .neighborhood_camera
                .unwrap_or(Camera::new([0.0, 0.0], 1.0)),
            _ => self.corpus_camera.unwrap_or(Camera::new(fallback, 0.05)),
        }
    }

    fn camera_mut(&mut self, nav: Option<&str>) -> &mut Camera {
        let corpus = nav.unwrap_or(CORPUS) != NEIGHBORHOOD;
        if corpus {
            if self.corpus_camera.is_none() {
                self.corpus_camera = Some(self.initial_corpus_camera());
            }
            self.corpus_camera
                .as_mut()
                .expect("corpus camera initialized")
        } else {
            if self.neighborhood_camera.is_none() {
                self.neighborhood_camera = Some(Camera::new([0.0, 0.0], 1.0));
            }
            self.neighborhood_camera
                .as_mut()
                .expect("neighborhood camera initialized")
        }
    }

    fn initial_corpus_camera(&self) -> Camera {
        Camera::new(self.layout_center, 0.05)
    }

    fn reset_state(&mut self) {
        let mut best: Option<(usize, usize, String)> = None;
        for node in self.index.nodes() {
            let score = node.child_count * 8 + node.degree.total;
            let candidate = (score, usize::MAX - node.position, node.id.to_string());
            if best.as_ref().is_none_or(|old| candidate > *old) {
                best = Some(candidate);
            }
        }
        self.focus = best.map(|(_, _, id)| id);
        self.selected = self.focus.clone();
        self.corpus_camera = Some(self.initial_corpus_camera());
        self.neighborhood_camera = Some(Camera::new([0.0, 0.0], 1.0));
    }

    fn rebuild_layout(&mut self) {
        let nodes: Vec<_> = self.index.nodes().collect();
        self.order = nodes.iter().map(|n| n.id.to_string()).collect();
        self.positions = vec![[0.0, 0.0]; nodes.len()];
        if nodes.is_empty() {
            return;
        }

        let mut children = vec![Vec::new(); nodes.len()];
        let mut roots = Vec::new();
        for node in &nodes {
            if index_is_root(&self.index, node.id) {
                roots.push(node.position);
            }
            for child in self.index.children_of(node.id) {
                if child.position != node.position {
                    children[node.position].push(child.position);
                }
            }
        }
        for kids in &mut children {
            kids.sort_unstable();
            kids.dedup();
        }
        roots.sort_unstable();
        roots.dedup();

        enum Frame {
            Enter(usize),
        }
        let mut seen = vec![false; nodes.len()];
        let mut positioned = vec![false; nodes.len()];
        let mut ordinal = 0usize;
        let mut stack: Vec<Frame> = roots.iter().rev().map(|&i| Frame::Enter(i)).collect();
        // Corpus order is depth-first hierarchy order mapped onto a square-ish
        // deterministic grid. The grid keeps the whole 5k corpus inside a
        // useful zoom envelope; parent/child edges, not depth indentation,
        // carry the hierarchy visually.
        while let Some(Frame::Enter(i)) = stack.pop() {
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            self.positions[i] = [
                (ordinal % GRID_COLS) as f32 * GRID_W,
                (ordinal / GRID_COLS) as f32 * GRID_H,
            ];
            positioned[i] = true;
            ordinal += 1;
            // Push children reversed, then immediately reverse the frame list
            // into the stack by expanding them now: DFS preorder is preserved
            // because a stack is LIFO.
            for &child in children[i].iter().rev() {
                stack.push(Frame::Enter(child));
            }
        }

        // A malformed-but-decodable forest can contain nav cycles or nodes no
        // root reaches. They remain visible in deterministic overflow cells
        // rather than being dropped by a presentation layout.
        for node in &nodes {
            if positioned[node.position] {
                continue;
            }
            self.positions[node.position] = [
                (ordinal % GRID_COLS) as f32 * GRID_W,
                (ordinal / GRID_COLS) as f32 * GRID_H,
            ];
            ordinal += 1;
        }
        if ordinal == 0 {
            self.layout_center = [0.0, 0.0];
        } else {
            let cols = ordinal.min(GRID_COLS) as f32;
            let rows = ordinal.div_ceil(GRID_COLS) as f32;
            self.layout_center = [(cols - 1.0) * 0.5 * GRID_W, (rows - 1.0) * 0.5 * GRID_H];
        }
        self.corpus_nodes = nodes
            .iter()
            .map(|n| {
                self.layout_node(
                    n.id,
                    self.positions
                        .get(n.position)
                        .copied()
                        .unwrap_or([0.0, 0.0]),
                )
            })
            .collect();
        self.relations = self.relations_layout();
    }

    fn layout_node(&self, id: &str, position: [f32; 2]) -> LayoutNode {
        let node = self.index.node(id);
        LayoutNode {
            id: id.into(),
            title: node
                .and_then(|n| n.title.map(str::to_string))
                .unwrap_or_else(|| id.into()),
            role: node.map_or("page", |n| n.role).into(),
            position,
            degree: node.map_or(0, |n| n.degree.total),
            child_count: node.map_or(0, |n| n.child_count),
            pinned: self.focus.as_deref() == Some(id) || self.selected.as_deref() == Some(id),
        }
    }

    fn relations_layout(&self) -> Vec<LayoutRelation> {
        let mut out = Vec::new();
        for node in self.index.nodes() {
            if let Some(parent) = node.parent {
                out.push(LayoutRelation {
                    from: node.id.into(),
                    to: parent.into(),
                    kind: "parent".into(),
                });
            }
        }
        if let Some(graph) = &self.graph {
            out.extend(graph.relations.iter().map(|r| LayoutRelation {
                from: r.from.value.clone(),
                to: r.to.value.clone(),
                kind: r.kind.clone(),
            }));
        }
        out
    }

    fn neighborhood_ids(&self) -> Vec<String> {
        let Some(focus) = self.focus.clone() else {
            return Vec::new();
        };
        if self.index.node(&focus).is_none() {
            return Vec::new();
        }
        let mut ids = BTreeSet::from([focus.clone()]);
        ids.extend(self.index.breadcrumb_of(&focus).map(|n| n.id.to_string()));
        ids.extend(self.index.children_of(&focus).map(|n| n.id.to_string()));
        for relation in self
            .index
            .relations_of(&focus, crate::graph::RelationDirection::Any)
        {
            if relation.from.kind == "page" && self.index.node(&relation.from.value).is_some() {
                ids.insert(relation.from.value.clone());
            }
            if relation.to.kind == "page" && self.index.node(&relation.to.value).is_some() {
                ids.insert(relation.to.value.clone());
            }
        }
        ids.into_iter().collect()
    }

    fn neighborhood_layout(&self) -> (Vec<LayoutNode>, &[LayoutRelation]) {
        let Some(focus) = self.focus.clone() else {
            return (Vec::new(), &self.relations);
        };
        let mut positions: BTreeMap<String, [f32; 2]> = BTreeMap::new();
        positions.insert(focus.clone(), [0.0, 0.0]);

        let breadcrumb: Vec<String> = self
            .index
            .breadcrumb_of(&focus)
            .map(|n| n.id.to_string())
            .collect();
        let depth = breadcrumb.len().saturating_sub(1);
        for (i, id) in breadcrumb.iter().enumerate() {
            if id != &focus {
                positions.insert(id.clone(), [-((depth - i) as f32) * 260.0, 0.0]);
            }
        }

        let children: Vec<String> = self
            .index
            .children_of(&focus)
            .map(|n| n.id.to_string())
            .collect();
        for (col, group) in children.chunks(NEIGHBOR_ROWS).enumerate() {
            let top = -((group.len().saturating_sub(1)) as f32) * 0.5 * ROW_H;
            for (row, id) in group.iter().enumerate() {
                positions.insert(
                    id.clone(),
                    [300.0 + col as f32 * 190.0, top + row as f32 * ROW_H],
                );
            }
        }

        let mut incoming = Vec::new();
        let mut outgoing = Vec::new();
        for relation in self
            .index
            .relations_of(&focus, crate::graph::RelationDirection::Any)
        {
            if relation.from.value == focus && relation.to.kind == "page" {
                outgoing.push(relation.to.value.clone());
            } else if relation.to.value == focus && relation.from.kind == "page" {
                incoming.push(relation.from.value.clone());
            }
        }
        incoming.sort();
        incoming.dedup();
        outgoing.sort();
        outgoing.dedup();
        for (col, group) in incoming.chunks(NEIGHBOR_ROWS).enumerate() {
            let top = -((group.len().saturating_sub(1)) as f32) * 0.5 * ROW_H;
            for (row, id) in group.iter().enumerate() {
                positions
                    .entry(id.clone())
                    .or_insert([-560.0 - col as f32 * 190.0, top + row as f32 * ROW_H]);
            }
        }
        let right = 300.0 + children.len().div_ceil(NEIGHBOR_ROWS) as f32 * 190.0 + 120.0;
        for (col, group) in outgoing.chunks(NEIGHBOR_ROWS).enumerate() {
            let top = -((group.len().saturating_sub(1)) as f32) * 0.5 * ROW_H;
            for (row, id) in group.iter().enumerate() {
                positions
                    .entry(id.clone())
                    .or_insert([right + col as f32 * 190.0, top + row as f32 * ROW_H]);
            }
        }

        let nodes = positions
            .iter()
            .map(|(id, position)| self.layout_node(id, *position))
            .collect();
        (nodes, self.relations.as_slice())
    }

    fn row_for_id(&self, id: &str) -> RowModel {
        let Some(node) = self.index.node(id) else {
            return RowModel::default();
        };
        let title = node.title.unwrap_or(node.id);
        let mut detail = node.role.to_string();
        if node.child_count > 0 {
            detail.push_str(&format!(" · {} satellites", node.child_count));
        }
        if node.degree.total > 0 {
            detail.push_str(&format!(" · {} relations", node.degree.total));
        }
        RowModel {
            key: id.into(),
            title: title.into(),
            detail,
            badge: (self.focus.as_deref() == Some(id))
                .then(|| crate::module::Chip::new("focus", Tone::Ok)),
            tone: Tone::Normal,
            rail: (self.selected.as_deref() == Some(id)).then_some(Tone::Ok),
            path: false,
        }
    }

    fn activate_key(&mut self, nav: Option<&str>, key: &str) -> bool {
        if let Some(id) = key.strip_prefix("node:") {
            if self.index.node(id).is_none() {
                return false;
            }
            self.selected = Some(id.into());
            self.focus = Some(id.into());
            self.nav_cache.invalidate();
            if nav.unwrap_or(CORPUS) == NEIGHBORHOOD {
                self.neighborhood_camera = Some(Camera::new([0.0, 0.0], 1.0));
            } else if let Some(node) = self.index.node(id) {
                let center = self.positions.get(node.position).copied();
                let camera = self.camera_mut(nav);
                camera.center = center.unwrap_or(camera.center);
                camera.zoom = camera.zoom.max(0.75);
            }
            return true;
        }
        if let Some(rest) = key.strip_prefix("cluster:") {
            let mut parts = rest.split(':');
            let (Some(cx), Some(cy)) = (parts.next(), parts.next()) else {
                return false;
            };
            let (Ok(cx), Ok(cy)) = (cx.parse::<i64>(), cy.parse::<i64>()) else {
                return false;
            };
            let cell = cluster_cell_world(self.camera(nav).zoom);
            let viewport = self.viewport;
            let camera = self.camera_mut(nav);
            camera.center = [(cx as f32 + 0.5) * cell, (cy as f32 + 0.5) * cell];
            camera.zoom_at(1.9, [viewport[0] / 2.0, viewport[1] / 2.0], viewport);
            if camera.zoom < 0.3 {
                camera.zoom = 0.3;
            }
            return true;
        }
        false
    }
}

fn index_is_root(index: &GraphNavIndex, id: &str) -> bool {
    index.parent_of(id).is_none()
}

impl Module for GraphModule {
    fn spec(&self) -> ModuleSpec {
        ModuleSpec {
            id: ID,
            title: "Graph",
            slot: Slot::Extra,
        }
    }

    fn ingest(&mut self, _env: &Envelope) -> bool {
        false
    }

    fn load(&mut self, snap: &ProjectSnapshot) -> bool {
        let same = match (&self.graph, &snap.ir_graph) {
            (None, None) => true,
            (Some(old), Some(new)) => Arc::ptr_eq(old, new),
            _ => false,
        };
        if same {
            return false;
        }
        self.graph = snap.ir_graph.clone();
        self.index.update(snap.ir_graph.clone());
        self.rebuild_layout();
        self.reset_state();
        self.nav_cache.invalidate();
        true
    }

    fn nav(&self) -> NavModel {
        self.nav_cache.get_or_build(|| NavModel {
            items: vec![
                NavItem {
                    key: CORPUS.into(),
                    label: "Corpus overview".into(),
                    count: Some(self.order.len() as u64),
                },
                NavItem {
                    key: NEIGHBORHOOD.into(),
                    label: "Focused neighborhood".into(),
                    count: Some(self.neighborhood_ids().len() as u64),
                },
            ],
        })
    }

    fn list_len(&self, nav: Option<&str>) -> usize {
        match nav.unwrap_or(CORPUS) {
            NEIGHBORHOOD => self.neighborhood_ids().len(),
            _ => self.order.len(),
        }
    }

    fn row(&self, nav: Option<&str>, index: usize) -> RowModel {
        let id = match nav.unwrap_or(CORPUS) {
            NEIGHBORHOOD => self.neighborhood_ids().get(index).cloned(),
            _ => self.order.get(index).cloned(),
        };
        id.map_or_else(RowModel::default, |id| self.row_for_id(&id))
    }

    fn index_of_key(&self, nav: Option<&str>, key: &str) -> Option<usize> {
        match nav.unwrap_or(CORPUS) {
            NEIGHBORHOOD => self.neighborhood_ids().iter().position(|id| id == key),
            _ => self.order.iter().position(|id| id == key),
        }
    }

    fn reading(&self, key: &str) -> ReadingModel {
        let Some(node) = self.index.node(key) else {
            return ReadingModel {
                title: "No graph page selected".into(),
                ..ReadingModel::default()
            };
        };
        ReadingModel {
            title: node.title.unwrap_or(node.id).into(),
            lines: vec![
                ReadingLine {
                    label: Some("role".into()),
                    text: node.role.into(),
                    tone: Tone::Normal,
                    mono: false,
                },
                ReadingLine {
                    label: Some("adjacency".into()),
                    text: format!(
                        "{} satellites · {} semantic relations",
                        node.child_count, node.degree.total
                    ),
                    tone: Tone::Dim,
                    mono: false,
                },
            ],
            breadcrumbs: self
                .index
                .breadcrumb_of(key)
                .map(|n| n.title.unwrap_or(n.id).into())
                .collect(),
            markdown: None,
        }
    }

    fn inspector(&self, key: &str) -> InspectorModel {
        let Some(node) = self.index.node(key) else {
            return InspectorModel::default();
        };
        InspectorModel {
            fields: vec![
                ("page id".into(), node.id.into()),
                ("source".into(), node.source_path.into()),
                ("role".into(), node.role.into()),
                ("satellites".into(), node.child_count.to_string()),
                (
                    "incoming relations".into(),
                    node.degree.incoming.to_string(),
                ),
                (
                    "outgoing relations".into(),
                    node.degree.outgoing.to_string(),
                ),
            ],
            ..InspectorModel::default()
        }
    }

    fn select(&mut self, key: Option<&str>) {
        self.selected = key.map(str::to_string);
        if let Some(key) = key.filter(|key| self.index.node(key).is_some()) {
            self.focus = Some(key.into());
            self.nav_cache.invalidate();
        }
    }

    fn contains(&self, key: &str) -> bool {
        self.index.node(key).is_some()
    }

    fn has_viewport(&self, _nav: Option<&str>) -> bool {
        true
    }

    fn viewport(&self, nav: Option<&str>) -> Option<ViewportModel> {
        let camera = self.camera(nav);
        let selected = self.selected.as_deref();
        let focused = self.focus.as_deref();
        let viewport = self.viewport;
        if nav.unwrap_or(CORPUS) == NEIGHBORHOOD {
            let (nodes, relations) = self.neighborhood_layout();
            Some(plan_viewport(
                &nodes, relations, camera, viewport, selected, focused,
            ))
        } else {
            Some(plan_viewport(
                &self.corpus_nodes,
                &self.relations,
                camera,
                viewport,
                selected,
                focused,
            ))
        }
    }

    fn viewport_input(&mut self, nav: Option<&str>, input: ViewportInput) -> bool {
        match input {
            ViewportInput::Resize { width, height } => {
                if !width.is_finite() || !height.is_finite() || width <= 1.0 || height <= 1.0 {
                    return false;
                }
                let next = [width, height];
                if self.viewport == next {
                    return false;
                }
                self.viewport = next;
                true
            }
            ViewportInput::Pan { dx, dy } => {
                self.camera_mut(nav).pan_screen(dx, dy);
                true
            }
            ViewportInput::DragPan {
                total_dx,
                total_dy,
                phase,
            } => {
                match phase {
                    ViewportDragPhase::Down => self.drag_last = Some([0.0, 0.0]),
                    ViewportDragPhase::Move => {
                        let last = self.drag_last.unwrap_or([0.0, 0.0]);
                        let delta = [total_dx - last[0], total_dy - last[1]];
                        self.drag_last = Some([total_dx, total_dy]);
                        self.camera_mut(nav).pan_screen(delta[0], delta[1]);
                    }
                    ViewportDragPhase::Up => self.drag_last = None,
                }
                true
            }
            ViewportInput::Zoom { factor, x, y } => {
                let viewport = self.viewport;
                self.camera_mut(nav).zoom_at(factor, [x, y], viewport);
                true
            }
            ViewportInput::Activate { key } => self.activate_key(nav, &key),
            ViewportInput::Reset => {
                if nav.unwrap_or(CORPUS) == NEIGHBORHOOD {
                    self.neighborhood_camera = Some(Camera::new([0.0, 0.0], 1.0));
                } else {
                    self.corpus_camera = Some(self.initial_corpus_camera());
                }
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::viewport::{MAX_ZOOM, MIN_ZOOM};

    #[test]
    fn zoom_is_clamped_to_the_camera_envelope() {
        let mut camera = Camera::new([0.0, 0.0], 1.0);
        camera.zoom_at(1_000.0, [10.0, 10.0], [100.0, 100.0]);
        assert_eq!(camera.zoom, MAX_ZOOM);
        camera.zoom_at(0.000_001, [10.0, 10.0], [100.0, 100.0]);
        assert_eq!(camera.zoom, MIN_ZOOM);
    }
}
