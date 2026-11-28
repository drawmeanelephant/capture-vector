//! Module grammar — the rail/chrome contract every lane renders through.
//! A module owns its state slice and exposes pure *Model data; the shell
//! renders it with generic providers and never matches on module identity.

use crate::command::{CommandSpec, Intent};
use crate::dialog::DialogAnswer;
use crate::viewport::{ViewportInput, ViewportModel};
use cv_contracts::graph::Graph;
use cv_core::diagnostic::Diagnostic;
use cv_core::events::Envelope;
use cv_core::id::{NounAttrRow, NounId};
use serde::Serialize;

/// Fixed rail order. `Extra` holds non-core modules (the wave-0 demo);
/// `Utilities` modules sit in the rail's Utilities group, below and apart
/// from the rest (Migrate); `Utility` modules are not on the rail: the
/// shell opens them by command or from its chrome (Doctor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Slot {
    Pages,
    Research,
    Audit,
    Publish,
    Outputs,
    Activity,
    Extra,
    Utilities,
    Utility,
}

impl Slot {
    pub const CORE: [Slot; 6] = [
        Slot::Pages,
        Slot::Research,
        Slot::Audit,
        Slot::Publish,
        Slot::Outputs,
        Slot::Activity,
    ];
    pub fn title(self) -> &'static str {
        match self {
            Slot::Pages => "Pages",
            Slot::Research => "Research",
            Slot::Audit => "Audit",
            Slot::Publish => "Publish",
            Slot::Outputs => "Outputs",
            Slot::Activity => "Activity",
            Slot::Extra => "Extra",
            Slot::Utilities => "Utilities",
            Slot::Utility => "Utility",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModuleSpec {
    /// Stable id, also the command namespace (`<id>.<verb>`).
    pub id: &'static str,
    pub title: &'static str,
    pub slot: Slot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tone {
    #[default]
    Normal,
    Dim,
    Ok,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NavItem {
    pub key: String,
    pub label: String,
    pub count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct NavModel {
    pub items: Vec<NavItem>,
}

/// One list row. `key` is the stable id handlers resolve by — never the index.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct RowModel {
    pub key: String,
    pub title: String,
    pub detail: String,
    pub badge: Option<Chip>,
    pub tone: Tone,
    /// Severity rail drawn at the row's leading edge.
    pub rail: Option<Tone>,
    /// `title` is a path: a cut drops its start so the file name stays.
    pub path: bool,
}

/// A short status word drawn as a tinted pill; `tone` says what it means.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Chip {
    pub text: String,
    pub tone: Tone,
}

impl Chip {
    pub fn new(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            tone,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct ReadingLine {
    pub label: Option<String>,
    pub text: String,
    pub tone: Tone,
    pub mono: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct ReadingModel {
    pub title: String,
    pub lines: Vec<ReadingLine>,
    /// Ancestor chain, root first.
    pub breadcrumbs: Vec<String>,
    /// Markdown source rendered as a draft sketch — Boris output is
    /// authoritative.
    pub markdown: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct InspectorModel {
    /// Status chips above the fields (authority, the publish gate).
    pub badges: Vec<Chip>,
    pub fields: Vec<(String, String)>,
    /// Rendered by one shared surface wherever they appear — the value is
    /// never remapped per module.
    pub diagnostics: Vec<Diagnostic>,
    /// Titled lists (impact, provenance inputs, occurrences).
    pub lists: Vec<(String, Vec<String>)>,
}

/// A module's `nav()` result, built on first read after a change. The
/// owner calls `invalidate` on every `&mut self` path that can change
/// its mailboxes, labels or counts; reads take `&self`, so no read can
/// interleave with a mutation and see a stale model.
#[derive(Debug, Default)]
pub struct NavCache(std::sync::OnceLock<NavModel>);

impl NavCache {
    pub fn get_or_build(&self, build: impl FnOnce() -> NavModel) -> NavModel {
        self.0.get_or_init(build).clone()
    }
    pub fn invalidate(&mut self) {
        self.0.take();
    }
}

/// Click-through target: a row key in another module's list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Link {
    pub module: &'static str,
    pub key: String,
    pub label: String,
}

/// Whether the composition root could establish Boris IR graph authority
/// for this snapshot. `Unknown` is for synthetic snapshots constructed
/// away from the workspace/projection boundary; the other variants are
/// boundary facts, not a reinterpretation of the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphProjectionAvailability {
    #[default]
    Unknown,
    Loaded,
    Absent,
    Malformed,
}

/// Evidence about the `graph.json` projection backing one snapshot.
/// Coverage counts compare the scanned content inputs with the frozen
/// graph's own source paths and `source` dependency endpoints. They are
/// disclosed as coverage facts; they do not validate Boris topology.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct GraphProjection {
    pub availability: GraphProjectionAvailability,
    /// Workspace-relative projection path when one was considered, or a
    /// capture label in fixture-backed scenes.
    pub source: Option<String>,
    /// Decode/fallback detail from the projection boundary.
    pub detail: Option<String>,
    /// Scanned page sources not represented by the loaded graph.
    pub uncovered_pages: u64,
    /// Graph node/source-endpoint paths not present in the scanned sources.
    pub missing_sources: u64,
}

impl GraphProjection {
    pub fn stale_coverage(&self) -> bool {
        self.uncovered_pages > 0 || self.missing_sources > 0
    }
}

/// Project state resynced from the engine (`Engine::snapshot` + proof
/// pack). Pure data: the composition root loads it, modules consume it.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct ProjectSnapshot {
    pub epoch: u64,
    /// Compiler identity that produced the outputs (`boris/0.9.0`).
    pub compiler: Option<String>,
    /// Publication profile target the artifacts belong to.
    pub target: Option<String>,
    pub pages: Vec<PageRecord>,
    pub outputs: Vec<OutputRecord>,
    /// The frozen Boris IR graph backing this snapshot, shared rather than
    /// copied. It is in-memory authority only and is deliberately never
    /// serialized into shell state or the store.
    #[serde(skip_serializing)]
    pub ir_graph: Option<std::sync::Arc<Graph>>,
    /// Boundary evidence for how `ir_graph` was established (or why none
    /// was). This is metadata about files and coverage, not persisted IR.
    pub graph_projection: GraphProjection,
    /// Hashes recorded at the last publish, by output path.
    pub last_publish: Option<PublishMark>,
    /// Directory the outputs' paths are relative to (the built HTML
    /// tree), when the composition root knows it.
    pub artifact_root: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct PageRecord {
    /// Source path relative to the content root — the row key, and the
    /// path Boris diagnostics cite.
    pub path: String,
    /// Graph id as authored (`id:` frontmatter or the content path stem).
    pub id: String,
    /// Whether `id` was authored explicitly in frontmatter. Only explicit
    /// ids are stable identity for provenance: the path-stem fallback
    /// belongs to whatever file happens to sit at that path, so matching
    /// sidecar provenance against it would let an unrelated squatter
    /// inherit a migration's Imported from.
    #[serde(default)]
    pub explicit_id: bool,
    pub title: String,
    pub parent: Option<String>,
    pub tags: Vec<String>,
    /// `(relation, target id)` pairs.
    pub relations: Vec<(String, String)>,
    /// `[[wikilink]]` targets from Boris IR edges; empty before IR coverage.
    pub wikilinks: Vec<String>,
    /// Frontmatter exactly as authored, in source order.
    pub frontmatter: Vec<(String, String)>,
    pub body: String,
    /// Rendered dist path, when the last build wrote one.
    pub output: Option<String>,
    pub fields: FieldSource,
    /// `(mode, source)` from the `.cv/imports/<run>/adoption.json` sidecar
    /// when this page was adopted from a migration run. Provenance lives
    /// in the sidecar only — never in frontmatter.
    pub imported_from: Option<(String, String)>,
}

/// Where a page's display fields (id, title, parent, tags, relations)
/// came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum FieldSource {
    /// The Boris IR graph from the last IR build.
    Boris,
    /// Read from source frontmatter because no Boris IR covers the page
    /// yet: a display guess that Boris may read differently.
    #[default]
    Source,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct OutputRecord {
    pub path: String,
    pub kind: String,
    pub producer: String,
    pub status: String,
    pub required: bool,
    pub bytes: u64,
    pub sha256: String,
    pub format_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct PublishMark {
    pub label: String,
    pub hashes: std::collections::BTreeMap<String, String>,
}

/// A lane module. Pure: no rune, no IO, no process spawning.
pub trait Module {
    fn spec(&self) -> ModuleSpec;
    /// Apply one committed envelope; return true when visible state changed.
    fn ingest(&mut self, env: &Envelope) -> bool;
    /// Apply one committed envelope together with its causal delivery stamp.
    /// Modules whose presentation is causally ordered override this; the
    /// default preserves the ordinary envelope-only contract.
    fn ingest_stamped(&mut self, env: &Envelope, _causal: u64) -> bool {
        self.ingest(env)
    }
    fn nav(&self) -> NavModel;
    fn list_len(&self, nav: Option<&str>) -> usize;
    fn row(&self, nav: Option<&str>, index: usize) -> RowModel;
    /// Index of the first row in `nav` whose key is `key`. The default
    /// builds every row; modules with large lists answer from an index.
    fn index_of_key(&self, nav: Option<&str>, key: &str) -> Option<usize> {
        (0..self.list_len(nav)).find(|&i| self.row(nav, i).key == key)
    }
    fn reading(&self, key: &str) -> ReadingModel;
    fn inspector(&self, _key: &str) -> InspectorModel {
        InspectorModel::default()
    }

    /// Whether this nav exposes a bounded spatial viewport in place of the
    /// generic reading pane. This is a view-grammar capability flag, not a
    /// module-identity hook.
    fn has_viewport(&self, _nav: Option<&str>) -> bool {
        false
    }

    /// The bounded viewport projection for this nav. Implementations must
    /// already have applied camera culling and deterministic level-of-detail;
    /// the shell mounts only what this model contains.
    fn viewport(&self, _nav: Option<&str>) -> Option<ViewportModel> {
        None
    }

    /// Canvas input routed back to the owning module. Returns true when the
    /// input changed the model and the frame should rebuild.
    fn viewport_input(&mut self, _nav: Option<&str>, _input: ViewportInput) -> bool {
        false
    }
    fn noun(&self, _key: &str) -> Option<NounId> {
        None
    }
    /// The row key currently holding `noun`, when the key is not stable
    /// across a lane mutation (a page path moves; its StableId does not).
    fn key_for(&self, _noun: &NounId) -> Option<String> {
        None
    }
    /// The primary selection's row key, kept current as selection changes
    /// and re-told just before `run` (`None` when nothing is selected) for
    /// verbs and command enablement that act on the selection.
    fn select(&mut self, _key: Option<&str>) {}
    /// The shell is showing this module. Modules backed by refreshable
    /// composition-root evidence can pull a fresh read-only snapshot here;
    /// the hook produces no events or intents.
    fn activated(&mut self) {}
    /// Lane-owned verbs for the palette. Ids are `<module id>.<verb>`.
    fn commands(&self) -> Vec<CommandSpec> {
        Vec::new()
    }
    fn run(&mut self, _command: &str) -> Vec<Intent> {
        Vec::new()
    }
    /// The answer to a dialog this module asked for with `Intent::Dialog`;
    /// `command` is the spec's id.
    fn answer(&mut self, _command: &str, _answer: &DialogAnswer) -> Vec<Intent> {
        Vec::new()
    }
    /// A migration worker atomically claimed its actual run id. `requested`
    /// is the stale candidate the wizard originally staged; `job` is the
    /// actual job now holding the run claim. Modules use this to retarget
    /// in-flight state before the completion notice arrives.
    fn import_started(
        &mut self,
        _requested: &str,
        _job: &crate::command::ImportJob,
    ) -> Vec<Intent> {
        Vec::new()
    }
    /// A migration worker ended. `job` carries the operation's stable
    /// identity in its cancellation token — shared by the original job and
    /// every collision-retargeted clone, where the run id is not. Modules
    /// correlate the token with their in-flight operation before applying
    /// anything; a completion that belongs to another operation or another
    /// module instance must leave the wizard, progress dialog and
    /// cancellation control untouched.
    fn import_finished(&mut self, _job: &crate::command::ImportJob) -> Vec<Intent> {
        Vec::new()
    }
    /// Merge decoded migration-lab link-audit evidence. Only Audit
    /// implements it; other lanes ignore this composition-root handoff.
    fn merge_link_audit(
        &mut self,
        _run_id: &str,
        _findings: &[crate::modules::audit::LinkAuditEvidence],
    ) {
    }

    /// Intents an `ingest` left owing (a finished run closes its
    /// progress dialog); the shell applies them after each commit.
    fn take_owed(&mut self) -> Vec<Intent> {
        Vec::new()
    }
    /// Resync from a project snapshot. Returns whether state changed.
    fn load(&mut self, _snap: &ProjectSnapshot) -> bool {
        false
    }
    /// Whether `key` names a row this module can reveal.
    fn contains(&self, _key: &str) -> bool {
        true
    }

    /// Click-through target for a row, if it names a noun elsewhere.
    fn link(&self, _key: &str) -> Option<Link> {
        None
    }
    /// The page source a row edits, relative to the content input; its
    /// companion is then the editor. `None`: a reading companion.
    fn source(&self, _key: &str) -> Option<String> {
        None
    }
    /// Count shown on the module's rail entry (e.g. open findings).
    fn badge(&self) -> Option<u64> {
        None
    }
    /// Uniform attribute projection of every noun this module owns.
    fn attr_rows(&self) -> Vec<NounAttrRow> {
        Vec::new()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RegistryError {
    DuplicateId(&'static str),
}

/// Module registry, filled by the generated `modules::register_all`.
/// Order is by slot then id, independent of registration order.
#[derive(Default)]
pub struct Registry {
    modules: Vec<Box<dyn Module>>,
}

impl Registry {
    pub fn register(&mut self, m: Box<dyn Module>) -> Result<(), RegistryError> {
        let spec = m.spec();
        if self.modules.iter().any(|x| x.spec().id == spec.id) {
            return Err(RegistryError::DuplicateId(spec.id));
        }
        self.modules.push(m);
        self.modules.sort_by_key(|x| (x.spec().slot, x.spec().id));
        Ok(())
    }
    pub fn with_all() -> Self {
        let mut r = Self::default();
        crate::modules::register_all(&mut r);
        r
    }
    /// Unregister `id`; returns whether it was registered.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.modules.len();
        self.modules.retain(|m| m.spec().id != id);
        self.modules.len() != before
    }
    pub fn len(&self) -> usize {
        self.modules.len()
    }
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }
    pub fn get(&self, i: usize) -> Option<&dyn Module> {
        self.modules.get(i).map(|m| m.as_ref())
    }
    pub fn get_mut(&mut self, i: usize) -> Option<&mut (dyn Module + 'static)> {
        self.modules.get_mut(i).map(|m| m.as_mut())
    }
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.modules.iter().position(|m| m.spec().id == id)
    }
    pub fn iter(&self) -> impl Iterator<Item = &dyn Module> {
        self.modules.iter().map(|m| m.as_ref())
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Box<dyn Module>> {
        self.modules.iter_mut()
    }
}
