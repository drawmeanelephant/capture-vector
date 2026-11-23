//! Module grammar — the rail/chrome contract every lane renders through.
//! A module owns its state slice and exposes pure *Model data; the shell
//! renders it with generic providers and never matches on module identity.

use crate::command::{CommandSpec, Intent};
use cv_core::events::Envelope;
use cv_core::id::NounId;
use serde::Serialize;

/// Fixed rail order. `Extra` holds non-core modules (the wave-0 demo).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Slot {
    Pages,
    Research,
    Audit,
    Publish,
    Activity,
    Extra,
}

impl Slot {
    pub const CORE: [Slot; 5] = [
        Slot::Pages,
        Slot::Research,
        Slot::Audit,
        Slot::Publish,
        Slot::Activity,
    ];
    pub fn title(self) -> &'static str {
        match self {
            Slot::Pages => "Pages",
            Slot::Research => "Research",
            Slot::Audit => "Audit",
            Slot::Publish => "Publish",
            Slot::Activity => "Activity",
            Slot::Extra => "Extra",
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
    pub badge: Option<String>,
    pub tone: Tone,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct InspectorModel {
    pub fields: Vec<(String, String)>,
}

/// A lane module. Pure: no rune, no IO, no process spawning.
pub trait Module {
    fn spec(&self) -> ModuleSpec;
    /// Apply one committed envelope; return true when visible state changed.
    fn ingest(&mut self, env: &Envelope) -> bool;
    fn nav(&self) -> NavModel;
    fn list_len(&self, nav: Option<&str>) -> usize;
    fn row(&self, nav: Option<&str>, index: usize) -> RowModel;
    fn reading(&self, key: &str) -> ReadingModel;
    fn inspector(&self, _key: &str) -> InspectorModel {
        InspectorModel::default()
    }
    fn noun(&self, _key: &str) -> Option<NounId> {
        None
    }
    /// Lane-owned verbs for the palette. Ids are `<module id>.<verb>`.
    fn commands(&self) -> Vec<CommandSpec> {
        Vec::new()
    }
    fn run(&mut self, _command: &str) -> Vec<Intent> {
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
