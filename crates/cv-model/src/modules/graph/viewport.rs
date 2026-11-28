use super::*;

impl GraphModule {
    pub(super) fn camera(&self, nav: Option<&str>) -> Camera {
        match nav.unwrap_or(CORPUS) {
            NEIGHBORHOOD => self
                .neighborhood_camera
                .unwrap_or(Camera::new([0.0, 0.0], 1.0)),
            _ => self
                .corpus_camera
                .unwrap_or(Camera::new(self.layout_center, 0.05)),
        }
    }

    pub(super) fn camera_mut(&mut self, nav: Option<&str>) -> &mut Camera {
        if nav.unwrap_or(CORPUS) == NEIGHBORHOOD {
            self.neighborhood_camera
                .get_or_insert(Camera::new([0.0, 0.0], 1.0))
        } else {
            let center = self.layout_center;
            self.corpus_camera.get_or_insert(Camera::new(center, 0.05))
        }
    }

    pub(super) fn reset_cameras(&mut self) {
        self.corpus_camera = Some(Camera::new(self.layout_center, 0.05));
        self.neighborhood_camera = Some(Camera::new([0.0, 0.0], 1.0));
        self.drag_last = None;
    }

    /// Deterministic corpus layout. Boris node order is mapped onto a
    /// square-ish grid; hierarchy and semantic adjacency are carried by the
    /// edges rather than by a second topology calculation.
    pub(super) fn rebuild_layout(&mut self) {
        let nodes: Vec<_> = self.index.nodes().collect();
        self.positions = vec![[0.0, 0.0]; nodes.len()];
        for (ordinal, node) in nodes.iter().enumerate() {
            self.positions[node.position] = [
                (ordinal % GRID_COLS) as f32 * GRID_W,
                (ordinal / GRID_COLS) as f32 * GRID_H,
            ];
        }
        if nodes.is_empty() {
            self.layout_center = [0.0, 0.0];
        } else {
            let cols = nodes.len().min(GRID_COLS) as f32;
            let rows = nodes.len().div_ceil(GRID_COLS) as f32;
            self.layout_center = [(cols - 1.0) * 0.5 * GRID_W, (rows - 1.0) * 0.5 * GRID_H];
        }
        self.corpus_nodes = nodes
            .iter()
            .map(|node| LayoutNode {
                id: node.id.into(),
                title: node.title.unwrap_or(node.id).into(),
                role: node.role.as_str().into(),
                position: self
                    .positions
                    .get(node.position)
                    .copied()
                    .unwrap_or([0.0, 0.0]),
                degree: node.degree.total,
                child_count: node.child_count,
                pinned: false,
            })
            .collect();
        let mut relations = Vec::new();
        for node in &nodes {
            if let Some(parent) = node.parent {
                relations.push(LayoutRelation {
                    from: node.id.into(),
                    to: parent.into(),
                    kind: "parent".into(),
                });
            }
        }
        if let Some(graph) = self.index.graph() {
            relations.extend(
                graph
                    .relations
                    .iter()
                    .filter(|relation| {
                        relation.from.kind == EndpointType::Page
                            && relation.to.kind == EndpointType::Page
                    })
                    .map(|relation| LayoutRelation {
                        from: relation.from.value.clone(),
                        to: relation.to.value.clone(),
                        kind: relation.kind.as_str().into(),
                    }),
            );
        }
        self.relations = relations;
    }

    pub(super) fn world_position(&self, id: &str) -> Option<[f32; 2]> {
        self.index
            .node(id)
            .and_then(|node| self.positions.get(node.position).copied())
    }

    pub(super) fn focus_on(&mut self, id: &str) {
        self.focus = Some(id.into());
        self.invalidate_navigation();
        self.neighborhood_camera = Some(Camera::new([0.0, 0.0], 1.0));
        if let Some(position) = self.world_position(id) {
            let camera = self.camera_mut(Some(CORPUS));
            camera.center = position;
            if camera.zoom < 0.75 {
                camera.zoom = 0.75;
            }
        }
    }

    pub(super) fn neighborhood_layout(&self) -> (Vec<LayoutNode>, Vec<LayoutRelation>) {
        let Some(focus) = self.focused_id().map(str::to_string) else {
            return (Vec::new(), Vec::new());
        };
        let mut positions: BTreeMap<String, [f32; 2]> = BTreeMap::new();
        positions.insert(focus.clone(), [0.0, 0.0]);
        let breadcrumb: Vec<String> = self
            .index
            .breadcrumb_of(&focus)
            .map(|node| node.id.to_string())
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
            .map(|node| node.id.to_string())
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
        for neighbor in self.relation_neighbors(&focus) {
            match neighbor.direction {
                SemanticDirection::Incoming => incoming.push(neighbor.id),
                SemanticDirection::Outgoing => outgoing.push(neighbor.id),
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
            .filter_map(|(id, position)| {
                self.index.node(id).map(|node| LayoutNode {
                    id: node.id.into(),
                    title: node.title.unwrap_or(node.id).into(),
                    role: node.role.as_str().into(),
                    position: *position,
                    degree: node.degree.total,
                    child_count: node.child_count,
                    pinned: id == &focus,
                })
            })
            .collect();
        let relations = self
            .relations
            .iter()
            .filter(|relation| {
                positions.contains_key(&relation.from) && positions.contains_key(&relation.to)
            })
            .cloned()
            .collect();
        (nodes, relations)
    }

    pub(super) fn activate_key(&mut self, nav: Option<&str>, key: &str) -> bool {
        if let Some(id) = key.strip_prefix("node:") {
            if self.index.node(id).is_none() {
                return false;
            }
            self.selected = Some(id.into());
            self.focus_on(id);
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

    pub(super) fn viewport_disclosures(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.projection.availability == GraphProjectionAvailability::Loaded
            && let Some(detail) = &self.projection.detail
        {
            out.push(format!("graph projection fallback · {detail}"));
        }
        if self.projection.stale_coverage() {
            out.push(format!(
                "stale graph coverage · uncovered scanned pages: {}; graph sources without scanned files: {}",
                self.projection.uncovered_pages, self.projection.missing_sources
            ));
        }
        if let Some(graph) = self.index.graph()
            && graph.relations.is_empty()
        {
            out.push(if graph.schema_version == cv_contracts::graph::SCHEMA_V02 {
                "IR 0.2.0 predates the semantic-relations facet; the decoded relation set is empty"
                    .into()
            } else {
                "the frozen Boris IR graph carries an empty semantic relation set".into()
            });
        }
        out
    }

    pub(super) fn with_viewport_disclosures(&self, mut model: ViewportModel) -> ViewportModel {
        let disclosures = self.viewport_disclosures();
        if !disclosures.is_empty() {
            model.status = format!("{} · {}", model.status, disclosures.join(" · "));
        }
        model
    }
}
