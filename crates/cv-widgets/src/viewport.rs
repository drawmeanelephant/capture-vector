//! Generic bounded spatial viewport renderer. The module model is already
//! culled; this file only mounts its nodes/edges and routes canvas input back.

use crate::theme::{self, Size, Styled, color, font};
use cv_model::theme::Theme;
use cv_model::viewport::{ViewportDragPhase, ViewportInput, ViewportModel, ViewportNode};
use cv_model::workbench::Workbench;
use rune::{Color, El, Phase, Shape, canvas, col, el, row, text};

fn edge_shape(t: &Theme, m: &ViewportModel) -> Shape {
    let mut shape = Shape::new();
    for edge in &m.edges {
        let width = (1.0 + (edge.count.saturating_sub(1) as f32).sqrt()).min(5.0);
        let alpha = if edge.kind == "parent" { 0.24 } else { 0.42 };
        shape = shape.stroke(
            vec![edge.from, edge.to],
            width,
            color(t.colors.accent).alpha(alpha),
        );
    }
    shape
}

fn node_fill(t: &Theme, n: &ViewportNode) -> Color {
    if n.is_cluster() {
        theme::tint(t.colors.warn, 0x38)
    } else if n.focused {
        theme::tint(t.colors.accent, 0x66)
    } else if n.selected {
        theme::tint(t.colors.accent, 0x42)
    } else if n.role == "trunk" {
        theme::tint(t.colors.ok, 0x34)
    } else {
        color(t.colors.pane_alt)
    }
}

fn node(t: &Theme, n: &ViewportNode) -> El {
    let key = n.key.clone();
    let activate = n.key.clone();
    let w = n.size[0];
    let h = n.size[1];
    let border = if n.focused {
        color(t.colors.accent)
    } else if n.is_cluster() {
        color(t.colors.warn)
    } else {
        color(t.colors.border)
    };
    let help = if let Some(count) = n.cluster_count {
        format!(
            "{count} pages folded here. Open the cluster or zoom in to reveal them; the status line reports exactly what is hidden."
        )
    } else {
        format!(
            "{} — {} · {} satellites · {} semantic relations",
            n.title, n.role, n.child_count, n.degree
        )
    };
    let label = n.label.clone().unwrap_or_default();
    let mut e = col()
        .key(format!("graph-{key}"))
        .absolute(n.center[0] - w / 2.0, n.center[1] - h / 2.0)
        .size(w, h)
        .fill(node_fill(t, n))
        .border(if n.label.is_some() { 1.0 } else { 0.5 }, border)
        .rounded(
            t,
            if n.label.is_some() {
                Size::Sm
            } else {
                Size::Pill
            },
        )
        .center()
        .states(t)
        .help(help)
        .on_press(move |s: &mut Workbench| {
            s.viewport_input(ViewportInput::Activate {
                key: activate.clone(),
            });
        });
    if let Some(label_text) = n.label.clone() {
        e = e.child(
            text(label)
                .font(font(t.type_.label))
                .color(color(t.colors.text))
                .single_line()
                .truncate()
                .px(t.spacing.unit)
                .help(label_text),
        );
    }
    e
}

fn control(t: &Theme, key: &str, label_text: &str, input: ViewportInput) -> El {
    col()
        .key(format!("graph-control-{key}"))
        .role("button")
        .size(28.0, 24.0)
        .center()
        .fill(color(t.colors.pane_alt))
        .border(1.0, color(t.colors.border))
        .rounded(t, Size::Sm)
        .states(t)
        .help(match key {
            "in" => "Zoom in",
            "out" => "Zoom out",
            _ => "Reset the graph camera",
        })
        .on_press(move |s: &mut Workbench| {
            s.viewport_input(input.clone());
        })
        .child(
            text(label_text)
                .font(font(t.type_.label))
                .color(color(t.colors.text))
                .single_line(),
        )
}

fn controls(t: &Theme, m: &ViewportModel) -> El {
    let center = [m.viewport[0] / 2.0, m.viewport[1] / 2.0];
    row()
        .key("graph-controls")
        .pin(None, None, Some(10.0), Some(10.0))
        .gap(4.0)
        .child(control(
            t,
            "out",
            "−",
            ViewportInput::Zoom {
                factor: 0.8,
                x: center[0],
                y: center[1],
            },
        ))
        .child(control(
            t,
            "in",
            "+",
            ViewportInput::Zoom {
                factor: 1.25,
                x: center[0],
                y: center[1],
            },
        ))
        .child(control(t, "reset", "fit", ViewportInput::Reset))
}

/// The viewport pane. Every mounted graph node is a real keyed element, which
/// is what lets the headless gate prove that large corpora stay bounded.
pub(crate) fn pane(s: &Workbench) -> El {
    let t = &s.theme;
    let Some(m) = s.active_viewport() else {
        return col().key("viewport-empty").w_full().grow(1.0);
    };
    let status = m.status.clone();
    let zoom = m.camera.zoom;
    let nodes: Vec<El> = m.nodes.iter().map(|n| node(t, n)).collect();
    let canvas_shape = edge_shape(t, &m);
    col()
        .key("viewport-pane")
        .w_full()
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .fill(color(t.colors.bg))
        .child(
            row()
                .key("viewport-status")
                .w_full()
                .px(t.spacing.unit * 2.0)
                .py(t.spacing.unit)
                .gap(t.spacing.unit * 2.0)
                .items_center()
                .border_sides([0.0, 0.0, 0.5, 0.0], color(t.colors.border))
                .child(
                    text(format!("graph · {:.0}%", zoom * 100.0))
                        .key("viewport-zoom")
                        .font(font(t.type_.label))
                        .color(color(t.colors.text_dim))
                        .single_line(),
                )
                .child(
                    text(status.clone())
                        .key("viewport-disclosure")
                        .font(font(t.type_.label))
                        .color(color(t.colors.text_dim))
                        .single_line()
                        .truncate_fade()
                        .help(status),
                ),
        )
        .child(
            el().key("graph-canvas")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .clip_soft(0.0)
                .on_layout_tx(|s: &mut Workbench, rect, _tx| {
                    s.viewport_input(ViewportInput::Resize {
                        width: rect[2],
                        height: rect[3],
                    });
                })
                .on_drag(|s: &mut Workbench, d, tx| {
                    let phase = match d.phase {
                        Phase::Down => ViewportDragPhase::Down,
                        Phase::Move => ViewportDragPhase::Move,
                        Phase::Up => ViewportDragPhase::Up,
                    };
                    s.viewport_input(ViewportInput::DragPan {
                        total_dx: d.dx,
                        total_dy: d.dy,
                        phase,
                    });
                    tx.instant();
                })
                .on_wheel(|s: &mut Workbench, e, tx| {
                    if e.dx == 0.0 && e.dy == 0.0 {
                        return false;
                    }
                    s.viewport_input(ViewportInput::Pan { dx: e.dx, dy: e.dy });
                    tx.instant();
                    true
                })
                .on_magnify(|s: &mut Workbench, e, tx| {
                    s.viewport_input(ViewportInput::Zoom {
                        factor: 1.0 + e.delta,
                        x: e.x,
                        y: e.y,
                    });
                    tx.instant();
                    true
                })
                .child(canvas(canvas_shape).key("graph-edges").pin(
                    Some(0.0),
                    Some(0.0),
                    Some(0.0),
                    Some(0.0),
                ))
                .children(nodes)
                .child(controls(t, &m)),
        )
}
