//! Element-tree + layout-geometry JSON — the cross-OS UI contract.
//! Built from `Runtime::elements()` (layout + retained tree), never from
//! GPU output. Floats are rounded to 0.1pt so the bytes are stable.

use rune::{Color, ElementInfo};
use serde_json::{Map, Value, json};

fn r(v: f32) -> Value {
    json!(((v as f64) * 10.0).round() / 10.0)
}

fn rect(a: [f32; 4]) -> Value {
    Value::Array(a.iter().map(|v| r(*v)).collect())
}

fn hex(c: Color) -> Option<String> {
    (c.a > 0.0).then(|| c.hex())
}

fn live(e: &ElementInfo) -> bool {
    !e.exiting && !e.virtual_extent
}

/// Flat, document-ordered tree: path, key, role, label, text, rect, colours, state.
pub fn element_tree(els: &[ElementInfo]) -> Value {
    let nodes: Vec<Value> = els
        .iter()
        .filter(|e| live(e))
        .map(|e| {
            let mut m = Map::new();
            m.insert("path".into(), json!(e.path));
            if let Some(k) = &e.key {
                m.insert("key".into(), json!(k));
            }
            if !e.role.is_empty() {
                m.insert("role".into(), json!(e.role));
            }
            if let Some(l) = &e.label {
                m.insert("label".into(), json!(l));
            }
            if let Some(t) = &e.text {
                m.insert("text".into(), json!(t));
            }
            m.insert("rect".into(), rect(e.rect));
            if let Some(f) = hex(e.fill) {
                m.insert("fill".into(), json!(f));
            }
            if let Some(c) = e.text_color.and_then(hex) {
                m.insert("color".into(), json!(c));
            }
            if e.focused {
                m.insert("focused".into(), json!(true));
            }
            m.insert("opacity".into(), r(e.opacity));
            Value::Object(m)
        })
        .collect();
    json!({ "schema": 1, "nodes": nodes })
}

/// Keyed geometry only: `{ key: [x, y, w, h] }`, sorted by key.
pub fn layout(els: &[ElementInfo]) -> Value {
    let mut keyed: Vec<(&str, [f32; 4])> = els
        .iter()
        .filter(|e| live(e))
        .filter_map(|e| e.key.as_deref().map(|k| (k, e.rect)))
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(b.0));
    let mut m = Map::new();
    for (k, rc) in keyed {
        m.entry(k.to_string()).or_insert_with(|| rect(rc));
    }
    json!({ "schema": 1, "keys": m })
}
