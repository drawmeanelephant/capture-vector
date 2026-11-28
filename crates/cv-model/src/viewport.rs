//! Bounded spatial-viewport projection shared by canvas-style modules.
//!
//! This is presentation state, not graph authority: callers hand in a
//! deterministic world layout plus already-resolved adjacency, and the
//! planner returns the bounded set of screen-space nodes and edges to mount.
//! The cap is deliberately small; disclosure counts explain what was culled
//! or folded instead of pretending the whole corpus is visible.

use std::collections::{BTreeMap, HashMap};

pub const MAX_MOUNTED_NODES: usize = 160;
pub const MAX_MOUNTED_EDGES: usize = 240;
pub const MIN_ZOOM: f32 = 0.01;
pub const MAX_ZOOM: f32 = 4.0;
pub const CLUSTER_SCREEN_SIZE: f32 = 140.0;
const FULL_ZOOM: f32 = 0.75;
const COMPACT_ZOOM: f32 = 0.25;
const VISUAL_MARGIN: f32 = 96.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub center: [f32; 2],
    pub zoom: f32,
}

impl Camera {
    pub fn new(center: [f32; 2], zoom: f32) -> Self {
        Self {
            center,
            zoom: zoom.clamp(MIN_ZOOM, MAX_ZOOM),
        }
    }

    /// Positive screen deltas move the content with the pointer, so the
    /// world camera moves the opposite way.
    pub fn pan_screen(&mut self, dx: f32, dy: f32) {
        if !dx.is_finite() || !dy.is_finite() {
            return;
        }
        self.center[0] -= dx / self.zoom;
        self.center[1] -= dy / self.zoom;
    }

    /// Zoom while keeping the world point under `(x, y)` fixed on screen.
    pub fn zoom_at(&mut self, factor: f32, at: [f32; 2], viewport: [f32; 2]) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let old = self.zoom;
        let new = (old * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        if (new - old).abs() < f32::EPSILON {
            return;
        }
        let world = [
            self.center[0] + (at[0] - viewport[0] / 2.0) / old,
            self.center[1] + (at[1] - viewport[1] / 2.0) / old,
        ];
        self.center = [
            world[0] - (at[0] - viewport[0] / 2.0) / new,
            world[1] - (at[1] - viewport[1] / 2.0) / new,
        ];
        self.zoom = new;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewportDetail {
    Aggregate,
    Compact,
    Full,
}

impl ViewportDetail {
    pub fn label(self) -> &'static str {
        match self {
            Self::Aggregate => "overview",
            Self::Compact => "compact",
            Self::Full => "detail",
        }
    }
}

/// One world-positioned input node. `pinned` marks focus/selection and wins
/// label and cap priority without changing layout authority.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutNode {
    pub id: String,
    pub title: String,
    pub role: String,
    pub position: [f32; 2],
    pub degree: usize,
    pub child_count: usize,
    pub pinned: bool,
}

/// Adjacency by node id. The planner drops endpoints absent from the layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutRelation {
    pub from: String,
    pub to: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ViewportNode {
    pub key: String,
    pub id: Option<String>,
    pub title: String,
    pub role: String,
    /// Center in viewport-local points.
    pub center: [f32; 2],
    pub size: [f32; 2],
    pub label: Option<String>,
    pub degree: usize,
    pub child_count: usize,
    pub selected: bool,
    pub focused: bool,
    pub cluster_count: Option<usize>,
}

impl ViewportNode {
    pub fn is_cluster(&self) -> bool {
        self.cluster_count.is_some()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ViewportEdge {
    pub key: String,
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub kind: String,
    pub count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ViewportHidden {
    pub total: usize,
    pub in_region: usize,
    pub individual: usize,
    pub clustered_nodes: usize,
    pub clusters: usize,
    pub culled: usize,
    pub capped: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ViewportModel {
    pub camera: Camera,
    pub viewport: [f32; 2],
    pub detail: ViewportDetail,
    pub nodes: Vec<ViewportNode>,
    pub edges: Vec<ViewportEdge>,
    pub hidden: ViewportHidden,
    pub status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewportDragPhase {
    Down,
    Move,
    Up,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ViewportInput {
    Resize {
        width: f32,
        height: f32,
    },
    Pan {
        dx: f32,
        dy: f32,
    },
    /// Total pointer travel since drag start. The module differences it so
    /// rune's cumulative drag events do not apply the same pan repeatedly.
    DragPan {
        total_dx: f32,
        total_dy: f32,
        phase: ViewportDragPhase,
    },
    Zoom {
        factor: f32,
        x: f32,
        y: f32,
    },
    Activate {
        key: String,
    },
    Reset,
}

fn detail_for(zoom: f32) -> ViewportDetail {
    if zoom >= FULL_ZOOM {
        ViewportDetail::Full
    } else if zoom >= COMPACT_ZOOM {
        ViewportDetail::Compact
    } else {
        ViewportDetail::Aggregate
    }
}

fn screen(camera: Camera, viewport: [f32; 2], world: [f32; 2]) -> [f32; 2] {
    [
        (world[0] - camera.center[0]) * camera.zoom + viewport[0] / 2.0,
        (world[1] - camera.center[1]) * camera.zoom + viewport[1] / 2.0,
    ]
}

fn in_view(p: [f32; 2], viewport: [f32; 2]) -> bool {
    p[0] >= -VISUAL_MARGIN
        && p[1] >= -VISUAL_MARGIN
        && p[0] <= viewport[0] + VISUAL_MARGIN
        && p[1] <= viewport[1] + VISUAL_MARGIN
}

fn priority(n: &LayoutNode, pinned: bool) -> (usize, usize, String) {
    (
        usize::from(pinned || n.pinned) * 1_000_000 + n.child_count * 8 + n.degree,
        usize::MAX - n.child_count,
        n.id.clone(),
    )
}

fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[0] + b[2] && a[0] + a[2] > b[0] && a[1] < b[1] + b[3] && a[1] + a[3] > b[1]
}

/// Project a world layout into a bounded viewport. `relations` is scanned
/// completely so cluster edge weights remain truthful even when endpoints
/// are folded; only the returned edge models are capped.
pub fn plan_viewport(
    nodes: &[LayoutNode],
    relations: &[LayoutRelation],
    camera: Camera,
    viewport: [f32; 2],
    selected: Option<&str>,
    focused: Option<&str>,
) -> ViewportModel {
    let viewport = [
        if viewport[0].is_finite() && viewport[0] > 1.0 {
            viewport[0]
        } else {
            1.0
        },
        if viewport[1].is_finite() && viewport[1] > 1.0 {
            viewport[1]
        } else {
            1.0
        },
    ];
    let detail = detail_for(camera.zoom);
    let positions: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();
    let screen_positions: Vec<[f32; 2]> = nodes
        .iter()
        .map(|n| screen(camera, viewport, n.position))
        .collect();
    let mut visible: Vec<usize> = (0..nodes.len())
        .filter(|&i| in_view(screen_positions[i], viewport))
        .collect();
    visible.sort_by_key(|&i| {
        let n = &nodes[i];
        priority(
            n,
            selected == Some(n.id.as_str()) || focused == Some(n.id.as_str()),
        )
    });
    visible.reverse();

    let mut out_nodes = Vec::new();
    let mut representation: HashMap<usize, (String, [f32; 2])> = HashMap::new();
    let mut individual = 0usize;
    let mut clustered_nodes = 0usize;
    let mut clusters = 0usize;
    let mut capped = 0usize;

    if detail == ViewportDetail::Aggregate {
        let cell_world = CLUSTER_SCREEN_SIZE / camera.zoom;
        let mut cells: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
        for i in visible {
            let w = nodes[i].position;
            cells
                .entry((
                    (w[0] / cell_world).floor() as i64,
                    (w[1] / cell_world).floor() as i64,
                ))
                .or_default()
                .push(i);
        }
        let mut cells: Vec<_> = cells.into_iter().collect();
        cells.sort_by(|((ax, ay), am), ((bx, by), bm)| {
            let pa = priority(
                &nodes[am[0]],
                selected == Some(nodes[am[0]].id.as_str())
                    || focused == Some(nodes[am[0]].id.as_str()),
            );
            let pb = priority(
                &nodes[bm[0]],
                selected == Some(nodes[bm[0]].id.as_str())
                    || focused == Some(nodes[bm[0]].id.as_str()),
            );
            pb.cmp(&pa).then_with(|| (ax, ay).cmp(&(bx, by)))
        });
        for ((cx, cy), members) in cells {
            if out_nodes.len() >= MAX_MOUNTED_NODES {
                capped += members.len();
                continue;
            }
            let world_center = [
                (cx as f32 + 0.5) * cell_world,
                (cy as f32 + 0.5) * cell_world,
            ];
            let center = screen(camera, viewport, world_center);
            if members.len() == 1 {
                let i = members[0];
                let n = &nodes[i];
                individual += 1;
                let key = format!("node:{}", n.id);
                representation.insert(i, (key.clone(), screen_positions[i]));
                out_nodes.push(node_model(
                    n,
                    key,
                    screen_positions[i],
                    [14.0, 14.0],
                    None,
                    selected,
                    focused,
                ));
            } else {
                clusters += 1;
                clustered_nodes += members.len();
                let key = format!("cluster:{cx}:{cy}");
                for &i in &members {
                    representation.insert(i, (key.clone(), center));
                }
                let exemplar = members
                    .iter()
                    .map(|&i| &nodes[i])
                    .min_by(|a, b| a.id.cmp(&b.id))
                    .expect("non-empty cluster");
                out_nodes.push(ViewportNode {
                    key: key.clone(),
                    id: None,
                    title: format!("{} pages", members.len()),
                    role: "cluster".into(),
                    center,
                    size: [86.0, 28.0],
                    label: Some(format!("{} · {}", members.len(), exemplar.title)),
                    degree: members.iter().map(|&i| nodes[i].degree).sum(),
                    child_count: members.iter().map(|&i| nodes[i].child_count).sum(),
                    selected: false,
                    focused: false,
                    cluster_count: Some(members.len()),
                });
            }
        }
        out_nodes.sort_by(|a, b| a.key.cmp(&b.key));
    } else {
        let keep = visible.len().min(MAX_MOUNTED_NODES);
        capped = visible.len().saturating_sub(keep);
        let mut kept = visible[..keep].to_vec();
        kept.sort_by(|&a, &b| nodes[a].id.cmp(&nodes[b].id));

        // Greedy deterministic label collision suppression. Higher-priority
        // nodes claim space first; lower-priority nodes remain as dots instead
        // of becoming an unreadable overlapping label cluster.
        let mut label_order = kept.clone();
        label_order.sort_by_key(|&i| {
            let n = &nodes[i];
            priority(
                n,
                selected == Some(n.id.as_str()) || focused == Some(n.id.as_str()),
            )
        });
        label_order.reverse();
        let mut accepted_labels: Vec<[f32; 4]> = Vec::new();
        let mut labelled = vec![false; nodes.len()];
        for i in label_order {
            let n = &nodes[i];
            let pinned = selected == Some(n.id.as_str()) || focused == Some(n.id.as_str());
            let important =
                pinned || n.pinned || detail == ViewportDetail::Full || n.child_count > 0;
            if !important {
                continue;
            }
            let size = node_size(detail, true);
            let c = screen_positions[i];
            let rect = [
                c[0] - size[0] / 2.0 - 3.0,
                c[1] - size[1] / 2.0 - 3.0,
                size[0] + 6.0,
                size[1] + 6.0,
            ];
            if accepted_labels.iter().all(|old| !overlaps(*old, rect)) {
                accepted_labels.push(rect);
                labelled[i] = true;
            }
        }

        for i in kept {
            let n = &nodes[i];
            let label = labelled[i].then(|| n.title.clone());
            let size = node_size(detail, label.is_some());
            individual += 1;
            let key = format!("node:{}", n.id);
            representation.insert(i, (key.clone(), screen_positions[i]));
            out_nodes.push(node_model(
                n,
                key,
                screen_positions[i],
                size,
                label,
                selected,
                focused,
            ));
        }
    }

    let edges = bounded_edges(relations, &positions, &representation);
    let hidden = ViewportHidden {
        total: nodes.len(),
        in_region: individual + clustered_nodes + capped,
        individual,
        clustered_nodes,
        clusters,
        culled: nodes
            .len()
            .saturating_sub(individual + clustered_nodes + capped),
        capped,
    };
    let status = disclosure_status(detail, hidden);
    ViewportModel {
        camera,
        viewport,
        detail,
        nodes: out_nodes,
        edges,
        hidden,
        status,
    }
}

fn node_size(detail: ViewportDetail, labelled: bool) -> [f32; 2] {
    match (detail, labelled) {
        (ViewportDetail::Full, true) => [124.0, 28.0],
        (ViewportDetail::Compact, true) => [96.0, 22.0],
        (_, false) => [13.0, 13.0],
        (_, true) => [86.0, 22.0],
    }
}

fn node_model(
    n: &LayoutNode,
    key: String,
    center: [f32; 2],
    size: [f32; 2],
    label: Option<String>,
    selected: Option<&str>,
    focused: Option<&str>,
) -> ViewportNode {
    ViewportNode {
        key,
        id: Some(n.id.clone()),
        title: n.title.clone(),
        role: n.role.clone(),
        center,
        size,
        label,
        degree: n.degree,
        child_count: n.child_count,
        selected: selected == Some(n.id.as_str()),
        focused: focused == Some(n.id.as_str()),
        cluster_count: None,
    }
}

fn bounded_edges(
    relations: &[LayoutRelation],
    positions: &HashMap<&str, usize>,
    representation: &HashMap<usize, (String, [f32; 2])>,
) -> Vec<ViewportEdge> {
    #[derive(Clone)]
    struct Acc {
        from: [f32; 2],
        to: [f32; 2],
        count: usize,
    }
    let mut acc: BTreeMap<(String, String, String), Acc> = BTreeMap::new();
    for relation in relations {
        let (Some(&from_i), Some(&to_i)) = (
            positions.get(relation.from.as_str()),
            positions.get(relation.to.as_str()),
        ) else {
            continue;
        };
        let (Some((from_key, from_p)), Some((to_key, to_p))) =
            (representation.get(&from_i), representation.get(&to_i))
        else {
            continue;
        };
        if from_key == to_key {
            continue;
        }
        let key = (from_key.clone(), to_key.clone(), relation.kind.clone());
        acc.entry(key).and_modify(|e| e.count += 1).or_insert(Acc {
            from: *from_p,
            to: *to_p,
            count: 1,
        });
    }
    acc.into_iter()
        .take(MAX_MOUNTED_EDGES)
        .map(|((from, to, kind), e)| ViewportEdge {
            key: format!("edge:{from}:{to}:{kind}"),
            from: e.from,
            to: e.to,
            kind,
            count: e.count,
        })
        .collect()
}

fn disclosure_status(detail: ViewportDetail, h: ViewportHidden) -> String {
    let mut parts = vec![format!(
        "{} · {} shown",
        detail.label(),
        h.individual + h.clustered_nodes
    )];
    if h.clusters > 0 {
        parts.push(format!(
            "{} folded into {} clusters",
            h.clustered_nodes, h.clusters
        ));
    }
    if h.culled > 0 {
        parts.push(format!("{} outside view", h.culled));
    }
    if h.capped > 0 {
        parts.push(format!("{} capped", h.capped));
    }
    if h.clusters > 0 || h.capped > 0 {
        parts.push("zoom in or open a cluster to reveal".into());
    } else if h.culled > 0 {
        parts.push("pan or zoom out to reveal".into());
    }
    parts.join(" · ")
}

pub fn cluster_cell_world(zoom: f32) -> f32 {
    CLUSTER_SCREEN_SIZE / zoom.clamp(MIN_ZOOM, MAX_ZOOM)
}
