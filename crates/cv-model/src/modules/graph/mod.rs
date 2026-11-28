//! Graph — shell navigation over Boris's frozen IR graph.
//!
//! This module is deliberately a navigation surface, not a second graph
//! compiler. Node identity, hierarchy, navigation order, semantic-relation
//! direction, and relation-kind tokens all come from the decoded Boris IR or
//! from `GraphNavIndex`'s positional index over it. Search and focus state are
//! presentation state only.

mod search;
mod viewport;

use crate::command::{CommandSpec, Intent};
use crate::dialog::{ActionTone, DialogAnswer, DialogKind, DialogSpec, Field};
use crate::graph::{GraphNavIndex, RelationDirection};
use crate::module::{
    Chip, GraphProjection, GraphProjectionAvailability, InspectorModel, Link, Module, ModuleSpec,
    NavCache, NavItem, NavModel, ProjectSnapshot, ReadingLine, ReadingModel, Registry, RowModel,
    Slot, Tone,
};
use crate::viewport::{
    Camera, LayoutNode, LayoutRelation, ViewportDragPhase, ViewportInput, ViewportModel,
    cluster_cell_world, plan_viewport,
};
use cv_contracts::graph::{Endpoint, EndpointType};
use cv_core::events::Envelope;
use cv_core::id::{AccountId, AttrValue, NounAttrRow, NounId};
use search::{GraphFilter, RoleFilter};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

pub const ID: &str = "graph";

const CORPUS: &str = "corpus";
const NEIGHBORHOOD: &str = "neighborhood";
const SEARCH: &str = "search";
const PROJECTION_ROW: &str = "graph-projection-state";
const NEIGHBOR_ROWS: usize = 18;
const ROW_H: f32 = 44.0;
const GRID_COLS: usize = 64;
const GRID_W: f32 = 132.0;
const GRID_H: f32 = 108.0;

pub const SEARCH_COMMAND: &str = "graph.search";
pub const CLEAR_SEARCH_COMMAND: &str = "graph.search.clear";
pub const PARENT_COMMAND: &str = "graph.parent";
pub const CHILD_COMMAND: &str = "graph.child";
pub const SIBLING_PREVIOUS_COMMAND: &str = "graph.sibling.previous";
pub const SIBLING_NEXT_COMMAND: &str = "graph.sibling.next";
pub const INCOMING_COMMAND: &str = "graph.relation.incoming";
pub const OUTGOING_COMMAND: &str = "graph.relation.outgoing";

pub fn register(r: &mut Registry) {
    r.register(Box::new(GraphModule::default()))
        .expect("graph module id is unique");
}

/// A semantic neighbor of one page. `direction` is the orientation already
/// present in Boris's endpoints; `kind` is the verbatim constrained-open
/// token, never an interpreted label.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RelationNeighbor {
    id: String,
    direction: SemanticDirection,
    kind: String,
    ordinal: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SemanticDirection {
    Outgoing,
    Incoming,
}

#[derive(Default)]
pub struct GraphModule {
    index: GraphNavIndex,
    projection: GraphProjection,
    /// Boris node order, as ids. Rows key by id rather than source path so a
    /// rename/reparent with a stable Boris page id does not break selection.
    order: Vec<String>,
    position_by_id: BTreeMap<String, usize>,
    focus: Option<String>,
    selected: Option<String>,
    filter: GraphFilter,
    search_results: Vec<String>,
    nav_cache: NavCache,
    neighborhood_cache: OnceLock<Vec<String>>,
    positions: Vec<[f32; 2]>,
    corpus_nodes: Vec<LayoutNode>,
    relations: Vec<LayoutRelation>,
    viewport: [f32; 2],
    layout_center: [f32; 2],
    drag_last: Option<[f32; 2]>,
    corpus_camera: Option<Camera>,
    neighborhood_camera: Option<Camera>,
}

impl GraphModule {
    fn focused_id(&self) -> Option<&str> {
        self.focus
            .as_deref()
            .filter(|id| self.index.node(id).is_some())
    }

    fn default_focus(&self) -> Option<String> {
        let mut best: Option<(usize, String)> = None;
        for node in self.index.nodes() {
            let score = node.child_count * 8 + node.degree.total;
            if best.as_ref().is_none_or(|(old, _)| score > *old) {
                best = Some((score, node.id.to_string()));
            }
        }
        best.map(|(_, id)| id)
    }

    fn rebuild_order(&mut self) {
        self.order = self.index.nodes().map(|n| n.id.to_string()).collect();
        self.position_by_id = self
            .order
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i))
            .collect();
    }

    fn repair_identity(&mut self) {
        if self.focused_id().is_none() {
            self.focus = self.default_focus();
        }
        if self
            .selected
            .as_deref()
            .is_some_and(|id| self.index.node(id).is_none())
        {
            self.selected = None;
        }
    }

    fn apply_filter(&mut self, filter: GraphFilter) {
        self.filter = filter.trimmed();
        let compiled = self.filter.compile();
        self.search_results = if self.filter.active() {
            self.index
                .nodes()
                .filter(|node| compiled.matches(&self.index, node))
                .map(|node| node.id.to_string())
                .collect()
        } else {
            Vec::new()
        };
        self.invalidate_navigation();
    }

    fn refresh_search(&mut self) {
        let filter = self.filter.clone();
        self.apply_filter(filter);
    }

    fn push_unique(ids: &mut Vec<String>, id: &str) {
        if !ids.iter().any(|seen| seen == id) {
            ids.push(id.to_string());
        }
    }

    fn invalidate_navigation(&mut self) {
        self.nav_cache.invalidate();
        self.neighborhood_cache.take();
    }

    fn cached_neighborhood_ids(&self) -> &[String] {
        self.neighborhood_cache
            .get_or_init(|| self.neighborhood_ids())
            .as_slice()
    }

    /// Ordered neighborhood ids: focus, ancestors, siblings, satellites,
    /// then semantic neighbors ordered by direction/kind/id. Rows are unique
    /// by page id because the row key is identity; multi-relation evidence is
    /// disclosed in the row detail and reading pane rather than duplicated.
    fn neighborhood_ids(&self) -> Vec<String> {
        let Some(focus) = self.focused_id().map(str::to_string) else {
            return Vec::new();
        };
        let mut ids = vec![focus.clone()];
        for node in self.index.breadcrumb_of(&focus) {
            Self::push_unique(&mut ids, node.id);
        }
        for node in self.index.siblings_of(&focus) {
            Self::push_unique(&mut ids, node.id);
        }
        for node in self.index.children_of(&focus) {
            Self::push_unique(&mut ids, node.id);
        }
        for neighbor in self.relation_neighbors(&focus) {
            Self::push_unique(&mut ids, &neighbor.id);
        }
        ids
    }

    fn relation_neighbors(&self, id: &str) -> Vec<RelationNeighbor> {
        let mut out = Vec::new();
        for relation in self.index.relations_of(id, RelationDirection::Any) {
            if relation.from.kind == EndpointType::Page
                && relation.from.value == id
                && relation.to.kind == EndpointType::Page
                && relation.to.value != id
                && self.index.node(&relation.to.value).is_some()
            {
                out.push(RelationNeighbor {
                    id: relation.to.value.clone(),
                    direction: SemanticDirection::Outgoing,
                    kind: relation.kind.as_str().to_string(),
                    ordinal: relation.ordinal,
                });
            }
            if relation.to.kind == EndpointType::Page
                && relation.to.value == id
                && relation.from.kind == EndpointType::Page
                && relation.from.value != id
                && self.index.node(&relation.from.value).is_some()
            {
                out.push(RelationNeighbor {
                    id: relation.from.value.clone(),
                    direction: SemanticDirection::Incoming,
                    kind: relation.kind.as_str().to_string(),
                    ordinal: relation.ordinal,
                });
            }
        }
        out.sort_by(|a, b| {
            (a.direction, &a.kind, &a.id, a.ordinal).cmp(&(b.direction, &b.kind, &b.id, b.ordinal))
        });
        out
    }

    fn first_relation_neighbor(&self, direction: SemanticDirection) -> Option<String> {
        let focus = self.focused_id()?;
        self.relation_neighbors(focus)
            .into_iter()
            .find(|neighbor| neighbor.direction == direction)
            .map(|neighbor| neighbor.id)
    }

    fn parent_target(&self) -> Option<String> {
        self.focused_id()
            .and_then(|id| self.index.parent_of(id))
            .map(|node| node.id.to_string())
    }

    fn child_target(&self) -> Option<String> {
        self.focused_id()
            .and_then(|id| self.index.children_of(id).next())
            .map(|node| node.id.to_string())
    }

    fn sibling_target(&self, delta: isize) -> Option<String> {
        let focus = self.focused_id()?;
        let parent = self.index.parent_of(focus)?;
        let siblings: Vec<_> = self.index.children_of(parent.id).collect();
        let pos = siblings.iter().position(|node| node.id == focus)?;
        let len = siblings.len() as isize;
        let next = (pos as isize + delta).rem_euclid(len) as usize;
        siblings.get(next).map(|node| node.id.to_string())
    }

    fn reveal(&mut self, id: &str, nav: &str) -> Vec<Intent> {
        self.focus_on(id);
        vec![Intent::Reveal {
            module: ID.into(),
            key: id.into(),
            nav: Some(nav.into()),
        }]
    }

    fn move_to(&mut self, target: Option<String>, missing: &str) -> Vec<Intent> {
        match target {
            Some(id) => self.reveal(&id, NEIGHBORHOOD),
            None => vec![Intent::Status(format!("graph: {missing}"))],
        }
    }

    fn search_form(&self, answer: Option<&DialogAnswer>) -> DialogSpec {
        let text = |field: &str, current: &str| {
            answer
                .and_then(|a| a.text(field))
                .unwrap_or(current)
                .to_string()
        };
        let role = answer
            .and_then(|a| a.text("role"))
            .map(RoleFilter::from_choice)
            .unwrap_or(self.filter.role);
        DialogSpec::new(
            SEARCH_COMMAND,
            "Search graph",
            DialogKind::Form(vec![
                Field::text("page", "Page id contains", &text("page", &self.filter.page_id))
                    .placeholder("guides/getting"),
                Field::text("title", "Title contains", &text("title", &self.filter.title))
                    .placeholder("Getting Started"),
                Field::text("tag", "Tag contains", &text("tag", &self.filter.tag))
                    .placeholder("guides"),
                Field::text(
                    "kind",
                    "Incident relation kind contains",
                    &text("kind", &self.filter.relation_kind),
                )
                .placeholder("relates_to"),
                Field::choice("role", "Role", &RoleFilter::CHOICES, role.choice_index()),
            ]),
        )
        .body(
            "Filters are conjunctive and case-insensitive. Relation kind matches either endpoint direction. Apply an empty filter to clear it.",
        )
        .cancel("Cancel")
        .action("apply", "Apply filter", ActionTone::Primary)
    }

    fn filter_from_answer(answer: &DialogAnswer) -> GraphFilter {
        GraphFilter {
            page_id: answer.text("page").unwrap_or_default().into(),
            title: answer.text("title").unwrap_or_default().into(),
            tag: answer.text("tag").unwrap_or_default().into(),
            relation_kind: answer.text("kind").unwrap_or_default().into(),
            role: answer
                .text("role")
                .map(RoleFilter::from_choice)
                .unwrap_or_default(),
        }
        .trimmed()
    }

    fn relation_context(&self, focus: &str, id: &str) -> Vec<String> {
        let mut labels = Vec::new();
        for relation in self.index.relations_of(focus, RelationDirection::Any) {
            if relation.from.kind == EndpointType::Page
                && relation.from.value == focus
                && relation.to.kind == EndpointType::Page
                && relation.to.value == id
            {
                labels.push(format!("outgoing · {}", relation.kind.as_str()));
            }
            if relation.to.kind == EndpointType::Page
                && relation.to.value == focus
                && relation.from.kind == EndpointType::Page
                && relation.from.value == id
            {
                labels.push(format!("incoming · {}", relation.kind.as_str()));
            }
        }
        labels
    }

    fn neighborhood_context(&self, focus: &str, id: &str) -> Vec<String> {
        if id == focus {
            return vec!["focus".into()];
        }
        let mut labels = Vec::new();
        if self.index.parent_of(focus).is_some_and(|n| n.id == id) {
            labels.push("parent".into());
        } else if self.index.breadcrumb_of(focus).any(|n| n.id == id) {
            labels.push("ancestor".into());
        }
        if self.index.siblings_of(focus).any(|n| n.id == id) {
            labels.push("sibling".into());
        }
        if self.index.children_of(focus).any(|n| n.id == id) {
            labels.push("satellite".into());
        }
        labels.extend(self.relation_context(focus, id));
        labels
    }

    fn row_for_id(&self, nav: Option<&str>, id: &str) -> RowModel {
        let Some(node) = self.index.node(id) else {
            return RowModel::default();
        };
        let mut context = Vec::new();
        if nav == Some(NEIGHBORHOOD)
            && let Some(focus) = self.focused_id()
        {
            context = self.neighborhood_context(focus, id);
        }
        let mut detail = node.id.to_string();
        if !context.is_empty() {
            detail.push_str(&format!(" · {}", context.join(", ")));
        }
        detail.push_str(&format!(" · {}", node.role.as_str()));
        if node.child_count > 0 {
            detail.push_str(&format!(" · {} satellites", node.child_count));
        }
        if node.degree.total > 0 {
            detail.push_str(&format!(" · {} relations", node.degree.total));
        }
        RowModel {
            key: id.into(),
            title: node.title.unwrap_or(node.id).into(),
            detail,
            badge: (self.focused_id() == Some(id)).then(|| Chip::new("focus", Tone::Ok)),
            tone: Tone::Normal,
            rail: (self.selected.as_deref() == Some(id)).then_some(Tone::Ok),
            path: false,
        }
    }

    fn endpoint_text(&self, endpoint: &Endpoint) -> String {
        let title = if endpoint.kind == EndpointType::Page {
            self.index
                .node(&endpoint.value)
                .and_then(|node| node.title)
                .map(|title| format!("{title} ({})", endpoint.value))
        } else {
            None
        };
        title.unwrap_or_else(|| match endpoint.kind {
            EndpointType::Page => endpoint.value.clone(),
            _ => format!("{}:{}", endpoint.kind.as_str(), endpoint.value),
        })
    }

    fn relation_lines(&self, key: &str) -> Vec<ReadingLine> {
        self.index
            .relations_of(key, RelationDirection::Any)
            .map(|relation| {
                let (direction, other) = if relation.from.value == key {
                    ("outgoing", relation.to)
                } else {
                    ("incoming", relation.from)
                };
                ReadingLine {
                    label: Some(format!("{direction} · {}", relation.kind.as_str())),
                    text: self.endpoint_text(other),
                    tone: Tone::Normal,
                    mono: false,
                }
            })
            .collect()
    }

    fn no_graph(&self) -> Option<String> {
        if !self.order.is_empty() {
            return None;
        }
        Some(match self.projection.availability {
            GraphProjectionAvailability::Absent => {
                "no Boris IR graph projection was found".to_string()
            }
            GraphProjectionAvailability::Malformed => {
                "the newest Boris IR graph projection is malformed".to_string()
            }
            _ => "no Boris IR graph is loaded".to_string(),
        })
    }

    fn projection_warnings(&self) -> Vec<ReadingLine> {
        let mut lines = Vec::new();
        if self.projection.availability == GraphProjectionAvailability::Loaded
            && let Some(detail) = &self.projection.detail
        {
            lines.push(ReadingLine {
                label: Some("graph projection".into()),
                text: format!("fallback · {detail}"),
                tone: Tone::Warn,
                mono: false,
            });
        }
        if self.projection.stale_coverage() {
            lines.push(ReadingLine {
                label: Some("graph projection".into()),
                text: format!(
                    "stale coverage · uncovered scanned pages: {}; graph sources without scanned files: {}",
                    self.projection.uncovered_pages, self.projection.missing_sources
                ),
                tone: Tone::Warn,
                mono: false,
            });
        }
        lines
    }

    fn empty_relation_line(&self) -> Option<ReadingLine> {
        let graph = self.index.graph()?;
        if !graph.relations.is_empty() {
            return None;
        }
        let text = if graph.schema_version == cv_contracts::graph::SCHEMA_V02 {
            "IR 0.2.0 predates the semantic-relations facet; the decoded relation set is empty."
        } else {
            "The frozen Boris IR graph carries an empty semantic relation set."
        };
        Some(ReadingLine {
            label: Some("semantic relations".into()),
            text: text.into(),
            tone: Tone::Dim,
            mono: false,
        })
    }

    fn projection_row(&self) -> RowModel {
        let (title, tone) = match self.projection.availability {
            GraphProjectionAvailability::Absent => ("No graph projection", Tone::Warn),
            GraphProjectionAvailability::Malformed => ("Malformed graph projection", Tone::Error),
            _ => ("No graph loaded", Tone::Warn),
        };
        let detail = self
            .projection
            .detail
            .clone()
            .or_else(|| self.projection.source.clone())
            .unwrap_or_else(|| "graph authority unavailable".into());
        RowModel {
            key: PROJECTION_ROW.into(),
            title: title.into(),
            detail,
            badge: Some(Chip::new("projection", tone)),
            tone,
            rail: Some(tone),
            path: false,
        }
    }

    fn no_focus(&self) -> Option<String> {
        (self.focused_id().is_none()).then(|| "select a graph page first".to_string())
    }

    fn movement_commands(&self) -> Vec<CommandSpec> {
        let blocked = self.no_graph().or_else(|| self.no_focus());
        let missing = |why: Option<String>| blocked.clone().or(why);
        vec![
            CommandSpec::new(PARENT_COMMAND, "Graph: Go to parent")
                .chord("left")
                .disabled(missing(
                    self.parent_target()
                        .is_none()
                        .then(|| "the focused page has no parent".to_string()),
                )),
            CommandSpec::new(CHILD_COMMAND, "Graph: Go to first satellite")
                .chord("right")
                .disabled(missing(
                    self.child_target()
                        .is_none()
                        .then(|| "the focused page has no satellites".to_string()),
                )),
            CommandSpec::new(SIBLING_PREVIOUS_COMMAND, "Graph: Previous sibling")
                .chord("shift+left")
                .disabled(missing(
                    self.sibling_target(-1)
                        .is_none()
                        .then(|| "the focused page has no siblings".to_string()),
                )),
            CommandSpec::new(SIBLING_NEXT_COMMAND, "Graph: Next sibling")
                .chord("shift+right")
                .disabled(missing(
                    self.sibling_target(1)
                        .is_none()
                        .then(|| "the focused page has no siblings".to_string()),
                )),
            CommandSpec::new(INCOMING_COMMAND, "Graph: Incoming relation")
                .chord("[")
                .disabled(missing(
                    self.first_relation_neighbor(SemanticDirection::Incoming)
                        .is_none()
                        .then(|| "the focused page has no incoming page relations".to_string()),
                )),
            CommandSpec::new(OUTGOING_COMMAND, "Graph: Outgoing relation")
                .chord("]")
                .disabled(missing(
                    self.first_relation_neighbor(SemanticDirection::Outgoing)
                        .is_none()
                        .then(|| "the focused page has no outgoing page relations".to_string()),
                )),
        ]
    }
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
        let projection_changed = self.projection != snap.graph_projection;
        self.projection = snap.graph_projection.clone();
        let update = self.index.update(snap.ir_graph.clone());
        if !update.changed {
            if projection_changed {
                self.invalidate_navigation();
            }
            return projection_changed;
        }
        self.rebuild_order();
        self.rebuild_layout();
        self.repair_identity();
        self.reset_cameras();
        self.refresh_search();
        self.invalidate_navigation();
        true
    }

    fn nav(&self) -> NavModel {
        self.nav_cache.get_or_build(|| NavModel {
            items: vec![
                NavItem {
                    key: CORPUS.into(),
                    label: "Corpus".into(),
                    count: Some(self.order.len() as u64),
                },
                NavItem {
                    key: NEIGHBORHOOD.into(),
                    label: "Neighborhood".into(),
                    count: Some(self.cached_neighborhood_ids().len() as u64),
                },
                NavItem {
                    key: SEARCH.into(),
                    label: "Search/filter".into(),
                    count: Some(self.search_results.len() as u64),
                },
            ],
        })
    }

    fn list_len(&self, nav: Option<&str>) -> usize {
        match nav.unwrap_or(CORPUS) {
            NEIGHBORHOOD => self.cached_neighborhood_ids().len(),
            SEARCH => self.search_results.len(),
            _ if self.order.is_empty() => 1,
            _ => self.order.len(),
        }
    }

    fn row(&self, nav: Option<&str>, index: usize) -> RowModel {
        let nav = nav.unwrap_or(CORPUS);
        if nav == CORPUS && self.order.is_empty() {
            return if index == 0 {
                self.projection_row()
            } else {
                RowModel::default()
            };
        }
        let id = match nav {
            NEIGHBORHOOD => self.cached_neighborhood_ids().get(index).cloned(),
            SEARCH => self.search_results.get(index).cloned(),
            _ => self.order.get(index).cloned(),
        };
        id.map_or_else(RowModel::default, |id| self.row_for_id(Some(nav), &id))
    }

    fn index_of_key(&self, nav: Option<&str>, key: &str) -> Option<usize> {
        match nav.unwrap_or(CORPUS) {
            NEIGHBORHOOD => self
                .cached_neighborhood_ids()
                .iter()
                .position(|id| id == key),
            SEARCH => self.search_results.iter().position(|id| id == key),
            _ if self.order.is_empty() => (key == PROJECTION_ROW).then_some(0),
            _ => self.position_by_id.get(key).copied(),
        }
    }

    fn reading(&self, key: &str) -> ReadingModel {
        let Some(node) = self.index.node(key) else {
            let text = if self.order.is_empty() {
                match self.projection.availability {
                    GraphProjectionAvailability::Absent => {
                        "No Boris IR graph is loaded: no graph.json projection was found in .cv/ir, .boris, or the output directory. Run an IR build to establish graph authority.".into()
                    }
                    GraphProjectionAvailability::Malformed => {
                        let detail = self
                            .projection
                            .detail
                            .as_deref()
                            .unwrap_or("the projection did not decode");
                        format!("No Boris IR graph is loaded: {detail}. An older valid projection was not found.")
                    }
                    _ => {
                        "No Boris IR graph is loaded. Run an IR build; malformed or stale projections stay out of graph authority.".into()
                    }
                }
            } else {
                "The selected page is not present in the frozen Boris graph.".into()
            };
            return ReadingModel {
                title: "No graph page selected".into(),
                lines: vec![ReadingLine {
                    label: None,
                    text,
                    tone: Tone::Dim,
                    mono: false,
                }],
                ..ReadingModel::default()
            };
        };
        let mut lines = self.projection_warnings();
        if let Some(empty) = self.empty_relation_line() {
            lines.push(empty);
        }
        lines.extend([
            ReadingLine {
                label: Some("page id".into()),
                text: node.id.into(),
                tone: Tone::Normal,
                mono: true,
            },
            ReadingLine {
                label: Some("source".into()),
                text: node.source_path.into(),
                tone: Tone::Dim,
                mono: true,
            },
            ReadingLine {
                label: Some("role".into()),
                text: node.role.as_str().into(),
                tone: Tone::Normal,
                mono: false,
            },
        ]);
        if let Some(status) = node.status {
            lines.push(ReadingLine {
                label: Some("status".into()),
                text: status.as_str().into(),
                tone: Tone::Normal,
                mono: false,
            });
        }
        if !node.tags.is_empty() {
            lines.push(ReadingLine {
                label: Some("tags".into()),
                text: node.tags.join(", "),
                tone: Tone::Normal,
                mono: false,
            });
        }
        if let Some(parent) = self.index.parent_of(key) {
            lines.push(ReadingLine {
                label: Some("parent".into()),
                text: parent.title.unwrap_or(parent.id).into(),
                tone: Tone::Normal,
                mono: false,
            });
        }
        let children: Vec<_> = self.index.children_of(key).collect();
        for child in children.iter().take(12) {
            lines.push(ReadingLine {
                label: Some("satellite".into()),
                text: child.title.unwrap_or(child.id).into(),
                tone: Tone::Normal,
                mono: false,
            });
        }
        if children.len() > 12 {
            lines.push(ReadingLine {
                label: Some("satellites".into()),
                text: format!("{} more direct satellites", children.len() - 12),
                tone: Tone::Dim,
                mono: false,
            });
        }
        lines.extend(self.relation_lines(key));
        ReadingModel {
            title: node.title.unwrap_or(node.id).into(),
            lines,
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
        let mut kinds = BTreeSet::new();
        let mut relations = Vec::new();
        for line in self.relation_lines(key) {
            if let Some(label) = &line.label {
                kinds.insert(label.clone());
                relations.push(format!("{label} → {}", line.text));
            }
        }
        InspectorModel {
            fields: vec![
                ("page id".into(), node.id.into()),
                ("source".into(), node.source_path.into()),
                ("role".into(), node.role.as_str().into()),
                (
                    "status".into(),
                    node.status.map_or("none".into(), |s| s.as_str().into()),
                ),
                ("tags".into(), node.tags.join(", ")),
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
            lists: vec![("Relations by direction and kind".into(), relations)],
            badges: if kinds.is_empty() {
                Vec::new()
            } else {
                vec![Chip::new(
                    format!("{} relation rows", kinds.len()),
                    Tone::Normal,
                )]
            },
            ..InspectorModel::default()
        }
    }

    fn noun(&self, key: &str) -> Option<NounId> {
        self.index.noun(AccountId("local".into()), key)
    }

    fn key_for(&self, noun: &NounId) -> Option<String> {
        self.index.key_for_noun(noun).map(|stable| stable.0)
    }

    fn contains(&self, key: &str) -> bool {
        self.index.node(key).is_some()
    }

    fn source(&self, key: &str) -> Option<String> {
        self.index.node(key).map(|node| node.source_path.into())
    }

    fn link(&self, key: &str) -> Option<Link> {
        let node = self.index.node(key)?;
        Some(Link {
            module: crate::modules::pages::ID,
            key: node.source_path.into(),
            label: "Open in Pages".into(),
        })
    }

    fn select(&mut self, key: Option<&str>) {
        self.selected = key.map(str::to_string);
        if let Some(key) = key.filter(|key| self.index.node(key).is_some()) {
            self.focus_on(key);
        }
    }

    fn has_viewport(&self, _nav: Option<&str>) -> bool {
        self.index.graph().is_some() && !self.order.is_empty()
    }

    fn viewport(&self, nav: Option<&str>) -> Option<ViewportModel> {
        if !self.has_viewport(nav) {
            return None;
        }
        let camera = self.camera(nav);
        let selected = self.selected.as_deref();
        let focused = self.focused_id();
        let viewport = self.viewport;
        let model = if nav.unwrap_or(CORPUS) == NEIGHBORHOOD {
            let (nodes, relations) = self.neighborhood_layout();
            plan_viewport(&nodes, &relations, camera, viewport, selected, focused)
        } else {
            plan_viewport(
                &self.corpus_nodes,
                &self.relations,
                camera,
                viewport,
                selected,
                focused,
            )
        };
        Some(self.with_viewport_disclosures(model))
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
                    self.corpus_camera = Some(Camera::new(self.layout_center, 0.05));
                }
                true
            }
        }
    }

    fn commands(&self) -> Vec<CommandSpec> {
        let mut commands = vec![
            CommandSpec::new(SEARCH_COMMAND, "Graph: Search/filter…")
                .chord("mod+f")
                .disabled(self.no_graph()),
            CommandSpec::new(CLEAR_SEARCH_COMMAND, "Graph: Clear search/filter").disabled(
                if self.no_graph().is_some() {
                    self.no_graph()
                } else {
                    (!self.filter.active()).then(|| "no graph filter is active".to_string())
                },
            ),
        ];
        commands.extend(self.movement_commands());
        commands
    }

    fn run(&mut self, command: &str) -> Vec<Intent> {
        match command {
            SEARCH_COMMAND => vec![Intent::dialog(self.search_form(None))],
            CLEAR_SEARCH_COMMAND => {
                self.apply_filter(GraphFilter::default());
                vec![Intent::Status("graph filter cleared".into())]
            }
            PARENT_COMMAND => {
                let target = self.parent_target();
                self.move_to(target, "the focused page has no parent")
            }
            CHILD_COMMAND => {
                let target = self.child_target();
                self.move_to(target, "the focused page has no satellites")
            }
            SIBLING_PREVIOUS_COMMAND => {
                let target = self.sibling_target(-1);
                self.move_to(target, "the focused page has no siblings")
            }
            SIBLING_NEXT_COMMAND => {
                let target = self.sibling_target(1);
                self.move_to(target, "the focused page has no siblings")
            }
            INCOMING_COMMAND => {
                let target = self.first_relation_neighbor(SemanticDirection::Incoming);
                self.move_to(target, "the focused page has no incoming page relations")
            }
            OUTGOING_COMMAND => {
                let target = self.first_relation_neighbor(SemanticDirection::Outgoing);
                self.move_to(target, "the focused page has no outgoing page relations")
            }
            _ => vec![Intent::Status(format!("unknown command {command}"))],
        }
    }

    fn answer(&mut self, command: &str, answer: &DialogAnswer) -> Vec<Intent> {
        if command != SEARCH_COMMAND || answer.action.is_none() {
            return Vec::new();
        }
        let filter = Self::filter_from_answer(answer);
        self.apply_filter(filter);
        if !self.filter.active() {
            return vec![Intent::Status("graph filter cleared".into())];
        }
        match self.search_results.first().cloned() {
            Some(first) => self.reveal(&first, SEARCH),
            None => vec![Intent::Status("graph filter matches no pages".into())],
        }
    }

    fn attr_rows(&self) -> Vec<NounAttrRow> {
        self.index
            .nodes()
            .filter_map(|node| {
                let noun = self.noun(node.id)?;
                let text = |s: &str| AttrValue::Text(s.into());
                let mut fields = BTreeMap::new();
                fields.insert("page_id".into(), text(node.id));
                if let Some(title) = node.title {
                    fields.insert("title".into(), text(title));
                }
                fields.insert("role".into(), text(node.role.as_str()));
                fields.insert("tags".into(), text(&node.tags.join(",")));
                fields.insert(
                    "relation_kinds".into(),
                    text(
                        &self
                            .index
                            .relations_of(node.id, RelationDirection::Any)
                            .map(|r| r.kind.as_str())
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .collect::<Vec<_>>()
                            .join(","),
                    ),
                );
                fields.insert("satellites".into(), AttrValue::Int(node.child_count as i64));
                fields.insert("relations".into(), AttrValue::Int(node.degree.total as i64));
                Some(NounAttrRow { noun, fields })
            })
            .collect()
    }
}
