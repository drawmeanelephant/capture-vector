//! Boris IR graph (`graph.json`, written by IR-mode `build --no-rag`,
//! default `.boris/`). The typed shape follows the frozen IR contract in
//! Boris's `docs/contracts/ir-schema.md` and its machine-readable twins
//! `schemas/ir-graph-0.2.0.schema.json` / `ir-graph-0.4.0.schema.json`.
//!
//! Decode is **version-keyed**, as Boris requires of consumers: the
//! document's `schemaVersion` string selects exactly one frozen shape.
//!
//! | `schemaVersion` | Shape |
//! |---|---|
//! | `0.2.0` | dependency graph only; `relations` and node `recipe` keys are never emitted |
//! | `0.3.0` | semantic-relation-bearing; `relations` is always present, possibly empty |
//! | `0.4.0` | Cooklang-bearing; every node carries a required nullable `recipe` key, `relations` present only when the corpus also carries semantic relations |
//!
//! Rules enforced here, all of them wire facts rather than interpreted
//! semantics:
//!
//! - An unsupported or future version (including `"0.1.0"`, whose edge
//!   shape differs) is [`GraphDecodeError::UnsupportedSchemaVersion`] —
//!   it can never decode into graph authority.
//! - Keys the schema lists as required stay required **even when
//!   nullable**: a node missing `parentIndex` (or a `0.4.0` node missing
//!   `recipe`) does not decode. Boris emits them as explicit nulls.
//! - A facet key owned by a later schema (`relations` under `0.2.0`,
//!   `recipe` under `0.2.0`/`0.3.0`) is
//!   [`GraphDecodeError::MisplacedFacetKey`]: the document contradicts its
//!   own version claim, and decoding it anyway would silently drop author
//!   semantic data. Genuinely unknown additive keys inside a known
//!   version remain tolerated, per the mirror contract.
//! - Closed vocabularies are typed wire facts. Endpoint `type`, node
//!   `role`/`status`, and dependency-edge `kind` decode to their known
//!   variants; a value outside the closed set decodes to a typed
//!   `Unknown` carrying the exact wire token. Semantic-relation kinds are
//!   deliberately different: that vocabulary is constrained-open, so any
//!   token matching Boris's bounded ASCII grammar is a `Token`, while a
//!   token outside the grammar is typed `Unknown` drift. Neither case
//!   crashes a lane, and neither is reinterpreted as a known value.
//!
//! The mirror is **decode-only**: no type here implements `Serialize`.
//! Boris is the sole writer of conforming IR; a re-emitting mirror could
//! not reproduce wire facts this projection deliberately collapses (a
//! `0.2.0` artifact's absent `relations` vs a `0.3.0` artifact's present
//! empty one — semantically the same empty relation set — or a `0.4.0`
//! node's present-null `recipe` vs a pre-`0.4` node's absent one). What
//! consumers need is retained instead: `schema_version` records the
//! artifact's facet line, `relations` the semantic relation set, and
//! `Node::recipe` the node's recipe data when it carries one.

use serde::{Deserialize, Deserializer};

/// `schemaVersion` strings with a frozen, explicitly mirrored shape.
pub const SCHEMA_V02: &str = "0.2.0";
pub const SCHEMA_V03: &str = "0.3.0";
pub const SCHEMA_V04: &str = "0.4.0";
pub const KNOWN_SCHEMA_VERSIONS: [&str; 3] = [SCHEMA_V02, SCHEMA_V03, SCHEMA_V04];

/// The frozen IR graph, projected for consumers. Construction is
/// in-memory authority only (tests build it directly, marked synthetic);
/// bytes become a `Graph` exclusively through [`decode_graph`] /
/// [`decode_graph_value`] or the version-keyed `Deserialize` impl.
#[derive(Debug, Clone, PartialEq)]
pub struct Graph {
    /// The artifact's own `schemaVersion` string — one of
    /// [`KNOWN_SCHEMA_VERSIONS`] for decoded graphs.
    pub schema_version: String,
    pub frozen: bool,
    pub nodes: Vec<Node>,
    /// Build-dependency edges (`parent`, `include`, `reference`, or typed
    /// drift outside that closed set). These are not semantic relations.
    pub edges: Vec<DependencyEdge>,
    /// Boris's build-invalidation reverse index for dependency `edges`.
    /// Semantic relations deliberately have no upstream reverse index.
    pub reverse_index: Vec<ReverseIndexEntry>,
    /// Derived page navigation over the already-validated frozen forest.
    pub nav: Vec<NavEntry>,
    /// Author semantic relations. Empty both when the artifact predates
    /// the facet (`0.2.0`, which cannot carry them) and when a
    /// relation-aware artifact's set is empty; `schema_version` carries
    /// the facet distinction.
    pub relations: Vec<SemanticRelation>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Stable 0-based index in this frozen graph's id-sorted node array.
    pub index: usize,
    pub id: String,
    /// Relative to the content input.
    pub source_path: String,
    /// Boris's closed role vocabulary. Drift outside `trunk`/`satellite`
    /// remains typed instead of masquerading as either one.
    pub role: NodeRole,
    pub parent: Option<String>,
    pub parent_index: Option<usize>,
    pub title: Option<String>,
    /// Boris's closed nullable status vocabulary: `None` is the wire null;
    /// `Some(NodeStatus::Unknown(_))` is a non-null drifted string.
    pub status: Option<NodeStatus>,
    pub tags: Vec<String>,
    pub body_offset: usize,
    /// The node's Cooklang recipe data when it carries one. `None` both
    /// for a `0.4.0` node whose `recipe` key is null and for pre-`0.4`
    /// nodes, which never carry the facet; `Graph::schema_version`
    /// records whether the artifact is recipe-aware at all.
    pub recipe: Option<Recipe>,
}

/// Structured recipe extracted from a `.cook` page (the IR 0.4 facet).
/// Quantities stay strings exactly as authored — Cooklang admits `1/2`
/// and `1-2`, so nothing here converts or merges them.
#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
    pub ingredients: Vec<Ingredient>,
    pub cookware: Vec<Cookware>,
    pub timers: Vec<Timer>,
}

/// One ingredient reference, in authored order. References are never
/// merged: adding `200%g` to `1%cup` is not decidable without a unit model.
#[derive(Debug, Clone, PartialEq)]
pub struct Ingredient {
    pub name: String,
    pub quantity: Quantity,
    /// Short-hand preparation from `(...)`; empty when absent.
    pub preparation: String,
    /// Entity id of the referenced recipe when the author wrote
    /// `@./path/Name`, else null. Joinable against `nodes[].id`.
    pub recipe_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cookware {
    pub name: String,
    pub quantity: Quantity,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Timer {
    pub name: String,
    pub quantity: Quantity,
}

/// Amount and unit exactly as authored. Either string may be empty.
#[derive(Debug, Clone, PartialEq)]
pub struct Quantity {
    pub amount: String,
    pub unit: String,
}

/// One per-page navigation entry, in the same order as `nodes`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct NavEntry {
    pub index: usize,
    pub id: String,
    /// Root-to-self inclusive, as Boris node indices.
    pub breadcrumb: Vec<usize>,
    /// Direct child node indices in entity-id order.
    pub children: Vec<usize>,
    /// Direct peers sharing the same parent, excluding self; empty for trunks.
    pub siblings: Vec<usize>,
}

/// Target-keyed incoming dependency-edge lookup. This is build invalidation
/// evidence, not semantic-relation adjacency.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReverseIndexEntry {
    pub target: Endpoint,
    pub incoming_edges: Vec<usize>,
}

/// The closed endpoint `type` vocabulary. `Unknown` carries the exact
/// nonconforming wire token, so a future or malformed value can be shown
/// as drift without being mistaken for `page` or `source`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EndpointType {
    Page,
    Source,
    Unknown(String),
}

impl EndpointType {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Page => "page",
            Self::Source => "source",
            Self::Unknown(s) => s,
        }
    }

    fn from_wire(s: String) -> Self {
        match s.as_str() {
            "page" => Self::Page,
            "source" => Self::Source,
            _ => Self::Unknown(s),
        }
    }
}

impl<'de> Deserialize<'de> for EndpointType {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(Self::from_wire)
    }
}

/// The closed node `role` vocabulary. `Unknown` carries drift verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NodeRole {
    Trunk,
    Satellite,
    Unknown(String),
}

impl NodeRole {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Trunk => "trunk",
            Self::Satellite => "satellite",
            Self::Unknown(s) => s,
        }
    }

    fn from_wire(s: String) -> Self {
        match s.as_str() {
            "trunk" => Self::Trunk,
            "satellite" => Self::Satellite,
            _ => Self::Unknown(s),
        }
    }
}

impl<'de> Deserialize<'de> for NodeRole {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(Self::from_wire)
    }
}

/// One non-null node `status` value. The wire null stays outside this
/// enum as `None` on [`Node::status`]; `Unknown` is a non-null string
/// outside the closed vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NodeStatus {
    Draft,
    Published,
    Archived,
    Unknown(String),
}

impl NodeStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Draft => "draft",
            Self::Published => "published",
            Self::Archived => "archived",
            Self::Unknown(s) => s,
        }
    }

    fn from_wire(s: String) -> Self {
        match s.as_str() {
            "draft" => Self::Draft,
            "published" => Self::Published,
            "archived" => Self::Archived,
            _ => Self::Unknown(s),
        }
    }
}

impl<'de> Deserialize<'de> for NodeStatus {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(Self::from_wire)
    }
}

/// The closed build dependency-edge `kind` vocabulary. `layout`, `asset`,
/// future upstream additions, and malformed values are all `Unknown` here:
/// preserved as evidence, never treated as one of the IR 0.2 kinds.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DependencyKind {
    Parent,
    Include,
    Reference,
    Unknown(String),
}

impl DependencyKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Parent => "parent",
            Self::Include => "include",
            Self::Reference => "reference",
            Self::Unknown(s) => s,
        }
    }

    fn from_wire(s: String) -> Self {
        match s.as_str() {
            "parent" => Self::Parent,
            "include" => Self::Include,
            "reference" => Self::Reference,
            _ => Self::Unknown(s),
        }
    }
}

impl<'de> Deserialize<'de> for DependencyKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(Self::from_wire)
    }
}

/// A semantic-relation kind. This vocabulary is constrained-open rather
/// than closed: Boris assigns no domain meaning to author tokens, but the
/// machine schema bounds their shape to `[a-z][a-z0-9_]{0,63}`. `Token` is
/// therefore any schema-conforming author token, canonical or novel.
/// `Unknown` is wire drift outside that grammar, carried verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SemanticRelationKind {
    Token(String),
    Unknown(String),
}

impl SemanticRelationKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Token(s) | Self::Unknown(s) => s,
        }
    }

    fn from_wire(s: String) -> Self {
        let bytes = s.as_bytes();
        let conforms = bytes.len() <= 64
            && matches!(bytes.first(), Some(b'a'..=b'z'))
            && bytes[1..]
                .iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_');
        if conforms {
            Self::Token(s)
        } else {
            Self::Unknown(s)
        }
    }
}

impl<'de> Deserialize<'de> for SemanticRelationKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(Self::from_wire)
    }
}

/// One build-dependency edge. Its kind is closed and is deliberately not
/// interchangeable with an author semantic-relation kind.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DependencyEdge {
    pub from: Endpoint,
    pub to: Endpoint,
    pub kind: DependencyKind,
}

/// One author semantic relation. Shape-adjacent to a dependency edge on
/// the wire, but a distinct type because its kind vocabulary and graph
/// meaning are different.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SemanticRelation {
    pub from: Endpoint,
    pub to: Endpoint,
    pub kind: SemanticRelationKind,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Endpoint {
    #[serde(rename = "type")]
    pub kind: EndpointType,
    pub value: String,
}

/// Why a `graph.json` document did not decode. Every variant means the
/// bytes cannot become graph authority; consumers decide how to degrade.
#[derive(Debug)]
pub enum GraphDecodeError {
    /// Not valid JSON, a typed key has the wrong shape, or a key the
    /// schema requires (including nullable ones) is missing.
    Json(serde_json::Error),
    /// `schemaVersion` is a string with no mirrored frozen shape —
    /// an unsupported or future IR schema.
    UnsupportedSchemaVersion {
        found: String,
        supported: [&'static str; 3],
    },
    /// A facet key owned by a later schema appears in a document claiming
    /// an earlier one (`relations` under `0.2.0`, or `recipe` on a node
    /// under `0.2.0`/`0.3.0`). Boris never emits this; the version claim
    /// and the payload contradict each other.
    MisplacedFacetKey {
        key: &'static str,
        schema_version: String,
    },
}

impl std::fmt::Display for GraphDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => write!(f, "graph.json is not valid IR: {e}"),
            Self::UnsupportedSchemaVersion { found, supported } => write!(
                f,
                "graph.json schemaVersion {found:?} is not supported (known: {})",
                supported.join(", ")
            ),
            Self::MisplacedFacetKey {
                key,
                schema_version,
            } => write!(
                f,
                "graph.json claims schemaVersion {schema_version:?} but carries {key:?}, \
                 a facet key owned by a later IR schema"
            ),
        }
    }
}

impl std::error::Error for GraphDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for GraphDecodeError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

/// Decode `graph.json` bytes into graph authority, version-keyed.
pub fn decode_graph(bytes: &[u8]) -> Result<Graph, GraphDecodeError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    decode_graph_value(value)
}

/// Decode an already-parsed `graph.json` document, version-keyed.
pub fn decode_graph_value(value: serde_json::Value) -> Result<Graph, GraphDecodeError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Header {
        schema_version: String,
    }
    // A missing or non-string schemaVersion is ordinary invalid JSON
    // shape here: version keying is impossible either way.
    let header = Header::deserialize(&value)?;
    let version = header.schema_version;
    match version.as_str() {
        SCHEMA_V02 => decode_v02(value, version),
        SCHEMA_V03 => decode_v03(value, version),
        SCHEMA_V04 => decode_v04(value, version),
        _ => Err(GraphDecodeError::UnsupportedSchemaVersion {
            found: version,
            supported: KNOWN_SCHEMA_VERSIONS,
        }),
    }
}

impl<'de> Deserialize<'de> for Graph {
    /// The only serde decode path: version-keyed, exactly as
    /// [`decode_graph_value`]. Any `serde_json::from_*::<Graph>` call is
    /// the keyed boundary, never shape inference.
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        decode_graph_value(value).map_err(serde::de::Error::custom)
    }
}

/// `required` keys that are nullable on the wire. Serde treats a plain
/// `Option<T>` field as optional-when-missing; routing through
/// `deserialize_with` keeps the key required while still decoding null.
fn required_nullable<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d)
}

/// The node shape shared by IR 0.2 and 0.3: every key required, four of
/// them nullable. `recipe` is deliberately not a field here — under these
/// versions it is a misplaced facet key, rejected before decode.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireNodeCore {
    index: usize,
    id: String,
    source_path: String,
    role: NodeRole,
    #[serde(deserialize_with = "required_nullable")]
    parent: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    parent_index: Option<usize>,
    #[serde(deserialize_with = "required_nullable")]
    title: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    status: Option<NodeStatus>,
    tags: Vec<String>,
    body_offset: usize,
}

/// IR 0.4 adds the required nullable `recipe` key to every node.
#[derive(Deserialize)]
struct WireNodeV04 {
    #[serde(flatten)]
    core: WireNodeCore,
    #[serde(deserialize_with = "required_nullable")]
    recipe: Option<WireRecipe>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireRecipe {
    ingredients: Vec<WireIngredient>,
    cookware: Vec<WireCookware>,
    timers: Vec<WireTimer>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireIngredient {
    name: String,
    quantity: WireQuantity,
    preparation: String,
    #[serde(deserialize_with = "required_nullable")]
    recipe_ref: Option<String>,
}

#[derive(Deserialize)]
struct WireCookware {
    name: String,
    quantity: WireQuantity,
}

#[derive(Deserialize)]
struct WireTimer {
    name: String,
    quantity: WireQuantity,
}

#[derive(Deserialize)]
struct WireQuantity {
    amount: String,
    unit: String,
}

/// IR 0.2 root: dependency graph only. `relations` is not a field —
/// under this version it is a misplaced facet key, rejected pre-decode.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireGraphV02 {
    frozen: bool,
    nodes: Vec<WireNodeCore>,
    edges: Vec<DependencyEdge>,
    reverse_index: Vec<ReverseIndexEntry>,
    nav: Vec<NavEntry>,
}

/// IR 0.3 root: `relations` is always present on a successful frozen
/// graph, including as an empty array.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireGraphV03 {
    frozen: bool,
    nodes: Vec<WireNodeCore>,
    edges: Vec<DependencyEdge>,
    reverse_index: Vec<ReverseIndexEntry>,
    nav: Vec<NavEntry>,
    relations: Vec<SemanticRelation>,
}

/// IR 0.4 `relations`: absent when the corpus carries no semantic
/// relations, an array when it does. Present-`null` is neither — the
/// schema types the key as an array — so the present case decodes as a
/// plain vector, which rejects null.
fn present_array<'de, D>(d: D) -> Result<Option<Vec<SemanticRelation>>, D::Error>
where
    D: Deserializer<'de>,
{
    Vec::<SemanticRelation>::deserialize(d).map(Some)
}

/// IR 0.4 root: nodes carry the recipe facet; `relations` is present
/// only when the corpus also carries semantic relations.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireGraphV04 {
    frozen: bool,
    nodes: Vec<WireNodeV04>,
    edges: Vec<DependencyEdge>,
    reverse_index: Vec<ReverseIndexEntry>,
    nav: Vec<NavEntry>,
    #[serde(default, deserialize_with = "present_array")]
    relations: Option<Vec<SemanticRelation>>,
}

fn decode_v02(value: serde_json::Value, version: String) -> Result<Graph, GraphDecodeError> {
    reject_misplaced_facets(&value, &version)?;
    let wire: WireGraphV02 = serde_json::from_value(value)?;
    Ok(Graph {
        schema_version: version,
        frozen: wire.frozen,
        nodes: wire.nodes.into_iter().map(node_core).collect(),
        edges: wire.edges,
        reverse_index: wire.reverse_index,
        nav: wire.nav,
        relations: Vec::new(),
    })
}

fn decode_v03(value: serde_json::Value, version: String) -> Result<Graph, GraphDecodeError> {
    reject_misplaced_facets(&value, &version)?;
    let wire: WireGraphV03 = serde_json::from_value(value)?;
    Ok(Graph {
        schema_version: version,
        frozen: wire.frozen,
        nodes: wire.nodes.into_iter().map(node_core).collect(),
        edges: wire.edges,
        reverse_index: wire.reverse_index,
        nav: wire.nav,
        relations: wire.relations,
    })
}

fn decode_v04(value: serde_json::Value, version: String) -> Result<Graph, GraphDecodeError> {
    reject_misplaced_facets(&value, &version)?;
    let wire: WireGraphV04 = serde_json::from_value(value)?;
    Ok(Graph {
        schema_version: version,
        frozen: wire.frozen,
        nodes: wire
            .nodes
            .into_iter()
            .map(|n| {
                let mut node = node_core(n.core);
                node.recipe = n.recipe.map(recipe);
                node
            })
            .collect(),
        edges: wire.edges,
        reverse_index: wire.reverse_index,
        nav: wire.nav,
        relations: wire.relations.unwrap_or_default(),
    })
}

/// Facet keys are versioned: a document claiming one version must not
/// carry a key a later version owns. `relations` belongs to 0.3+ and is
/// never valid in a 0.2 artifact; `recipe` belongs to 0.4 nodes and is
/// never valid before it. Genuinely unknown keys are not rejected here —
/// additive drift inside a known version stays tolerated.
fn reject_misplaced_facets(
    value: &serde_json::Value,
    version: &str,
) -> Result<(), GraphDecodeError> {
    // `relations` is owned by 0.3+: never valid in a 0.2 artifact,
    // required in 0.3 (the typed decode enforces that), facet-optional in 0.4.
    if version == SCHEMA_V02 && value.get("relations").is_some() {
        return Err(GraphDecodeError::MisplacedFacetKey {
            key: "relations",
            schema_version: version.into(),
        });
    }
    // `recipe` is owned by 0.4 nodes: never valid on earlier nodes. The
    // typed decode enforces its presence under 0.4; here only the
    // pre-0.4 contradiction is rejected. Non-object nodes are left to
    // the typed decode, which rejects them as wrong shapes.
    if version != SCHEMA_V04
        && let Some(nodes) = value.get("nodes").and_then(|n| n.as_array())
        && nodes.iter().any(|n| n.get("recipe").is_some())
    {
        return Err(GraphDecodeError::MisplacedFacetKey {
            key: "recipe",
            schema_version: version.into(),
        });
    }
    Ok(())
}

fn node_core(wire: WireNodeCore) -> Node {
    Node {
        index: wire.index,
        id: wire.id,
        source_path: wire.source_path,
        role: wire.role,
        parent: wire.parent,
        parent_index: wire.parent_index,
        title: wire.title,
        status: wire.status,
        tags: wire.tags,
        body_offset: wire.body_offset,
        recipe: None,
    }
}

fn recipe(wire: WireRecipe) -> Recipe {
    let quantity = |q: WireQuantity| Quantity {
        amount: q.amount,
        unit: q.unit,
    };
    Recipe {
        ingredients: wire
            .ingredients
            .into_iter()
            .map(|i| Ingredient {
                name: i.name,
                quantity: quantity(i.quantity),
                preparation: i.preparation,
                recipe_ref: i.recipe_ref,
            })
            .collect(),
        cookware: wire
            .cookware
            .into_iter()
            .map(|c| Cookware {
                name: c.name,
                quantity: quantity(c.quantity),
            })
            .collect(),
        timers: wire
            .timers
            .into_iter()
            .map(|t| Timer {
                name: t.name,
                quantity: quantity(t.quantity),
            })
            .collect(),
    }
}
