//! Shell-level graph navigation: selection identity, search/filter, and
//! keyboard neighbor traversal. Graph inputs are deterministic in-memory
//! structures, not hand-authored Boris JSON fixtures.

use cv_contracts::graph::{
    Endpoint, EndpointType, Graph, NavEntry, Node, NodeRole, SemanticRelation, SemanticRelationKind,
};
use cv_model::dialog::{DialogAnswer, Value};
use cv_model::module::Registry;
use cv_model::module::{FieldSource, PageRecord, ProjectSnapshot};
use cv_model::modules::graph::{
    CHILD_COMMAND, ID as GRAPH_ID, INCOMING_COMMAND, OUTGOING_COMMAND, PARENT_COMMAND,
    SEARCH_COMMAND, SIBLING_NEXT_COMMAND, SIBLING_PREVIOUS_COMMAND,
};
use cv_model::workbench::{KeyInput, KeyPress, Workbench};
use std::collections::BTreeMap;
use std::sync::Arc;

fn endpoint(value: &str) -> Endpoint {
    Endpoint {
        kind: EndpointType::Page,
        value: value.into(),
    }
}

fn node(index: usize, id: &str, parent: Option<(&str, usize)>, title: &str, tags: &[&str]) -> Node {
    Node {
        index,
        id: id.into(),
        source_path: format!("{id}.md"),
        role: if parent.is_some() {
            NodeRole::Satellite
        } else {
            NodeRole::Trunk
        },
        parent: parent.map(|(id, _)| id.into()),
        parent_index: parent.map(|(_, index)| index),
        title: Some(title.into()),
        status: None,
        tags: tags.iter().map(|tag| tag.to_string()).collect(),
        body_offset: 0,
        recipe: None,
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

fn relation(from: &str, to: &str, kind: &str) -> SemanticRelation {
    SemanticRelation {
        from: endpoint(from),
        to: endpoint(to),
        kind: SemanticRelationKind::Token(kind.into()),
    }
}

fn walking_graph() -> Graph {
    Graph {
        schema_version: "synthetic-shell-v1".into(),
        frozen: true,
        nodes: vec![
            node(0, "root", None, "Root", &["docs"]),
            node(1, "a", Some(("root", 0)), "Alpha", &["blue"]),
            node(2, "b", Some(("root", 0)), "Beta", &["red"]),
            node(3, "c", Some(("a", 1)), "Gamma", &["blue"]),
            node(4, "d", Some(("root", 0)), "Delta", &[]),
        ],
        edges: Vec::new(),
        reverse_index: Vec::new(),
        nav: vec![
            nav(0, "root", &[0], &[1, 2, 4], &[]),
            nav(1, "a", &[0, 1], &[3], &[2, 4]),
            nav(2, "b", &[0, 2], &[], &[1, 4]),
            nav(3, "c", &[0, 1, 3], &[], &[]),
            nav(4, "d", &[0, 4], &[], &[1, 2]),
        ],
        relations: vec![
            relation("d", "a", "depends_on"),
            relation("a", "c", "implements"),
            relation("a", "b", "relates_to"),
        ],
    }
}

fn snapshot(graph: Graph) -> ProjectSnapshot {
    let pages = graph
        .nodes
        .iter()
        .map(|node| PageRecord {
            path: node.source_path.clone(),
            id: node.id.clone(),
            title: node.title.clone().unwrap_or_else(|| node.id.clone()),
            parent: node.parent.clone(),
            tags: node.tags.clone(),
            fields: FieldSource::Boris,
            ..Default::default()
        })
        .collect();
    ProjectSnapshot {
        epoch: 1,
        pages,
        ir_graph: Some(Arc::new(graph)),
        ..Default::default()
    }
}

fn workbench() -> Workbench {
    let mut wb = Workbench::new(Registry::with_all());
    wb.load_snapshot(&snapshot(walking_graph()));
    wb
}

fn activate(wb: &mut Workbench, id: &str) {
    let index = wb.registry.index_of(id).expect("module registered");
    wb.activate(index);
}

fn key(wb: &mut Workbench, input: KeyInput) -> bool {
    key_shift(wb, input, false)
}

fn key_shift(wb: &mut Workbench, input: KeyInput, shift: bool) -> bool {
    wb.key(&KeyPress { key: input, shift })
}

fn reset_to_a(wb: &mut Workbench) {
    wb.set_nav("corpus");
    wb.select_key("a");
    assert_eq!(wb.selected_key(), Some("a"));
}

fn search_answer(fields: &[(&str, &str)], role: &str) -> DialogAnswer {
    let mut values = BTreeMap::new();
    for (field, value) in fields {
        values.insert(field.to_string(), Value::Text((*value).into()));
    }
    values.insert("role".into(), Value::Choice(role.into()));
    DialogAnswer {
        action: Some("apply".into()),
        context: None,
        values,
        secret: None,
    }
}

fn apply_search(wb: &mut Workbench, fields: &[(&str, &str)], role: &str) {
    let answer = search_answer(fields, role);
    let index = wb.registry.index_of(GRAPH_ID).unwrap();
    let intents = wb
        .registry
        .get_mut(index)
        .expect("graph module")
        .answer(SEARCH_COMMAND, &answer);
    for intent in intents {
        wb.apply(intent);
    }
}

fn search_keys(wb: &Workbench) -> Vec<String> {
    let index = wb.registry.index_of(GRAPH_ID).unwrap();
    let module = wb.registry.get(index).unwrap();
    (0..module.list_len(Some("search")))
        .map(|i| module.row(Some("search"), i).key)
        .collect()
}

#[test]
fn graph_and_pages_selection_round_trip_through_shell_activation() {
    let mut wb = workbench();
    activate(&mut wb, "pages");
    wb.select_key("a.md");
    let noun = wb.app.selection.noun.clone().expect("page noun");

    activate(&mut wb, GRAPH_ID);
    assert_eq!(wb.selected_key(), Some("a"));
    assert_eq!(wb.app.selection.noun.as_ref(), Some(&noun));
    assert_eq!(wb.active_nav(), Some("corpus"));

    activate(&mut wb, "pages");
    assert_eq!(wb.selected_key(), Some("a.md"));
    assert_eq!(wb.app.selection.noun.as_ref(), Some(&noun));

    activate(&mut wb, GRAPH_ID);
    assert_eq!(wb.selected_key(), Some("a"));
    assert_eq!(wb.app.selection.noun.as_ref(), Some(&noun));
}

#[test]
fn search_filter_covers_id_title_tag_relation_kind_and_role() {
    let mut wb = workbench();
    activate(&mut wb, GRAPH_ID);

    assert!(key(&mut wb, KeyInput::Command('f')));
    assert!(wb.dialogs.is_open());
    wb.dialogs.close(SEARCH_COMMAND);
    assert!(!wb.dialogs.is_open());

    apply_search(&mut wb, &[("page", "a")], "any");
    assert_eq!(search_keys(&wb), ["a"]);
    assert_eq!(wb.selected_key(), Some("a"));
    assert_eq!(wb.active_nav(), Some("search"));

    apply_search(&mut wb, &[("title", "beta")], "any");
    assert_eq!(search_keys(&wb), ["b"]);

    apply_search(&mut wb, &[("tag", "blue")], "any");
    assert_eq!(search_keys(&wb), ["a", "c"]);

    apply_search(&mut wb, &[("kind", "depends_on")], "any");
    assert_eq!(search_keys(&wb), ["a", "d"]);

    apply_search(&mut wb, &[], "trunk");
    assert_eq!(search_keys(&wb), ["root"]);

    apply_search(&mut wb, &[], "satellite");
    assert_eq!(search_keys(&wb), ["a", "b", "c", "d"]);

    apply_search(&mut wb, &[("title", "missing")], "any");
    assert!(search_keys(&wb).is_empty());
    assert_eq!(wb.status, "graph filter matches no pages");
}

#[test]
fn keyboard_commands_traverse_hierarchy_siblings_and_relation_directions() {
    let mut wb = workbench();
    activate(&mut wb, GRAPH_ID);
    reset_to_a(&mut wb);

    assert!(key(&mut wb, KeyInput::Left));
    assert_eq!(wb.selected_key(), Some("root"));

    reset_to_a(&mut wb);
    assert!(key(&mut wb, KeyInput::Right));
    assert_eq!(wb.selected_key(), Some("c"));

    reset_to_a(&mut wb);
    assert!(key_shift(&mut wb, KeyInput::Right, true));
    assert_eq!(wb.selected_key(), Some("b"));

    reset_to_a(&mut wb);
    assert!(key_shift(&mut wb, KeyInput::Left, true));
    assert_eq!(wb.selected_key(), Some("d"));

    reset_to_a(&mut wb);
    assert!(key(&mut wb, KeyInput::Char('[')));
    assert_eq!(wb.selected_key(), Some("d"));

    reset_to_a(&mut wb);
    assert!(key(&mut wb, KeyInput::Char(']')));
    assert_eq!(wb.selected_key(), Some("c"));

    // The same commands are also palette commands, with their chord metadata
    // visible rather than hidden in a module-specific key hack.
    let commands = wb.commands();
    for id in [
        PARENT_COMMAND,
        CHILD_COMMAND,
        SIBLING_PREVIOUS_COMMAND,
        SIBLING_NEXT_COMMAND,
        INCOMING_COMMAND,
        OUTGOING_COMMAND,
    ] {
        assert!(commands.iter().any(|command| command.id == id), "{id}");
    }
}

#[test]
fn neighborhood_rows_disclose_structure_relation_direction_and_kind() {
    let mut wb = workbench();
    activate(&mut wb, GRAPH_ID);
    reset_to_a(&mut wb);
    wb.set_nav("neighborhood");
    wb.select_key("a");

    let index = wb.registry.index_of(GRAPH_ID).unwrap();
    let module = wb.registry.get(index).unwrap();
    let rows: Vec<_> = (0..module.list_len(Some("neighborhood")))
        .map(|i| module.row(Some("neighborhood"), i))
        .collect();
    assert_eq!(
        rows.iter().map(|row| row.key.as_str()).collect::<Vec<_>>(),
        ["a", "root", "b", "d", "c"]
    );
    assert!(rows[1].detail.contains("parent"));
    assert!(rows[2].detail.contains("sibling"));
    assert!(rows[2].detail.contains("outgoing · relates_to"));
    assert!(rows[3].detail.contains("incoming · depends_on"));
    assert!(rows[4].detail.contains("satellite"));
    assert!(rows[4].detail.contains("outgoing · implements"));

    // The cached mailbox is invalidated when focus moves rather than
    // freezing the first focus's neighborhood into every later frame.
    wb.select_key("c");
    let module = wb.registry.get(index).unwrap();
    let rows: Vec<_> = (0..module.list_len(Some("neighborhood")))
        .map(|i| module.row(Some("neighborhood"), i))
        .collect();
    assert_eq!(
        rows.iter().map(|row| row.key.as_str()).collect::<Vec<_>>(),
        ["c", "root", "a"]
    );
}

#[test]
fn stable_page_identity_survives_rename_and_reparent_in_the_graph() {
    let mut wb = workbench();
    activate(&mut wb, "pages");
    wb.select_key("a.md");
    let noun = wb.app.selection.noun.clone().expect("page noun");
    activate(&mut wb, GRAPH_ID);
    assert_eq!(wb.selected_key(), Some("a"));

    let mut moved = walking_graph();
    moved.nodes[1].source_path = "renamed/a.md".into();
    moved.nodes[1].parent = None;
    moved.nodes[1].parent_index = None;
    moved.nodes[1].role = NodeRole::Trunk;
    moved.nav[1].breadcrumb = vec![1];
    moved.nav[1].siblings = Vec::new();
    moved.nav[0].children = vec![2, 4];
    let mut snap = snapshot(moved);
    snap.epoch = 2;
    snap.pages[1].path = "renamed/a.md".into();
    snap.pages[1].parent = None;
    wb.load_snapshot(&snap);

    activate(&mut wb, GRAPH_ID);
    assert_eq!(wb.selected_key(), Some("a"));
    assert_eq!(wb.app.selection.noun.as_ref(), Some(&noun));

    activate(&mut wb, "pages");
    assert_eq!(wb.selected_key(), Some("renamed/a.md"));
    assert_eq!(wb.app.selection.noun.as_ref(), Some(&noun));
}
