//! Element-tree + layout-geometry JSON — the cross-OS UI contract.
//! Built from `Runtime::elements()` (layout + retained tree) and
//! `Runtime::presented_pose`, never from GPU output. Floats are rounded to
//! 0.1pt so the bytes are stable.

use rune::{Color, ElementInfo, Runtime};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// Tree JSON schema. 2 added `radius`, `shadows`, `scale`, `hover`,
/// `press`, `focus_visible`, `help` and `selected`; see
/// `fixtures/ui/golden/README.md`.
pub const TREE_SCHEMA: u32 = 2;

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

/// The `presented_pose` selector for `path`: its keyed segments. Rune
/// resolves a selector to the first matching node, so it only names `path`
/// when no other element shares those keyed segments.
fn selector(path: &str) -> String {
    path.split('/')
        .filter(|s| !s.starts_with('#'))
        .collect::<Vec<_>>()
        .join("/")
}

fn shadows<S: 'static>(rt: &Runtime<S>, sel: &str) -> Option<Value> {
    let pose = rt.presented_pose(sel)?;
    let list: Vec<Value> = pose
        .shadows()
        .iter()
        .filter(|s| s.color.a > 0.0)
        .map(|s| {
            json!([
                r(s.x),
                r(s.y),
                r(s.blur),
                r(s.spread),
                s.color.hex(),
                s.inset
            ])
        })
        .collect();
    (!list.is_empty()).then_some(Value::Array(list))
}

fn node<S: 'static>(rt: &Runtime<S>, e: &ElementInfo, unique: bool) -> Value {
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
    if let Some(h) = &e.help {
        m.insert("help".into(), json!(h));
    }
    m.insert("rect".into(), rect(e.rect));
    if let Some(f) = hex(e.fill) {
        m.insert("fill".into(), json!(f));
    }
    if let Some(c) = e.text_color.and_then(hex) {
        m.insert("color".into(), json!(c));
    }
    if r(e.radius) != json!(0.0) {
        m.insert("radius".into(), r(e.radius));
    }
    if let Some(s) = unique.then(|| shadows(rt, &selector(&e.path))).flatten() {
        m.insert("shadows".into(), s);
    }
    if r(e.scale) != json!(1.0) {
        m.insert("scale".into(), r(e.scale));
    }
    if e.hover > 0.0 {
        m.insert("hover".into(), r(e.hover));
    }
    if e.press > 0.0 {
        m.insert("press".into(), r(e.press));
    }
    if e.focused {
        m.insert("focused".into(), json!(true));
    }
    if e.focus_visible {
        m.insert("focus_visible".into(), json!(true));
    }
    if e.item_selected {
        m.insert("selected".into(), json!(true));
    }
    m.insert("opacity".into(), r(e.opacity));
    Value::Object(m)
}

/// Flat, document-ordered tree: path, key, role, label, text, help, rect,
/// colours, radius, shadows, scale, pointer, focus and selection state,
/// opacity.
/// Fields past schema 1 appear only when they differ from the default.
pub fn element_tree<S: 'static>(rt: &Runtime<S>) -> Value {
    let els = rt.elements();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for e in els.iter().filter(|e| e.key.is_some()) {
        *seen.entry(selector(&e.path)).or_default() += 1;
    }
    let nodes: Vec<Value> = els
        .iter()
        .filter(|e| live(e))
        .map(|e| {
            let unique = e.key.is_some() && seen.get(&selector(&e.path)) == Some(&1);
            node(rt, e, unique)
        })
        .collect();
    json!({ "schema": TREE_SCHEMA, "nodes": nodes })
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

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Cx, El, Env, Shadow, col, color::hex as c};

    fn card(key: &str, y: f32) -> El {
        col()
            .key(key.to_string())
            .w(40.0)
            .h(20.0)
            .fill(c("#ffffff").unwrap())
            .radius(4.0)
            .box_shadow(Shadow {
                y,
                blur: 8.0,
                color: c("#00000080").unwrap(),
                ..Shadow::default()
            })
    }

    fn view(_: &(), _: &mut Cx<()>) -> El {
        col()
            .key("root")
            .child(card("one", 2.0))
            .child(card("two", 6.0))
            .child(col().child(card("dup", 1.0)))
            .child(col().child(card("dup", 3.0)))
            .child(col().key("plain").w(10.0).h(10.0))
    }

    fn nodes() -> Vec<Value> {
        let env = Env {
            system_dark: true,
            scale: 1.0,
        };
        let mut rt = Runtime::new((), view, (200.0, 200.0), env);
        rt.frame(0.0);
        rt.settle_all();
        rt.frame(1.0 / 60.0);
        let v = element_tree(&rt);
        assert_eq!(v["schema"], json!(TREE_SCHEMA));
        v["nodes"].as_array().unwrap().clone()
    }

    fn keyed<'a>(ns: &'a [Value], key: &str) -> Vec<&'a Value> {
        ns.iter().filter(|n| n["key"] == key).collect()
    }

    #[test]
    fn each_keyed_node_reports_its_own_shadows_and_radius() {
        let ns = nodes();
        let one = keyed(&ns, "one")[0];
        let two = keyed(&ns, "two")[0];
        assert_eq!(
            one["shadows"],
            json!([[0.0, 2.0, 8.0, 0.0, "#00000080", false]])
        );
        assert_eq!(
            two["shadows"],
            json!([[0.0, 6.0, 8.0, 0.0, "#00000080", false]])
        );
        assert_eq!(one["radius"], json!(4.0));
    }

    #[test]
    fn ambiguous_selectors_omit_shadows_rather_than_borrow_a_sibling() {
        let ns = nodes();
        let dups = keyed(&ns, "dup");
        assert_eq!(dups.len(), 2);
        assert!(dups.iter().all(|n| n.get("shadows").is_none()));
    }

    #[test]
    fn default_valued_fields_are_left_out() {
        let ns = nodes();
        let plain = keyed(&ns, "plain")[0].as_object().unwrap();
        for k in [
            "radius",
            "shadows",
            "scale",
            "hover",
            "press",
            "focus_visible",
            "help",
            "selected",
        ] {
            assert!(!plain.contains_key(k), "{k} on a plain node");
        }
    }
}
