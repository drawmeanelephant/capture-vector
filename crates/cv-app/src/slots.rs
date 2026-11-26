//! Composition-root registry of live engine slots by workspace root.
//! Long-running shell workers (migration adoption) reach the same
//! scheduler the watch uses instead of spawning a competing engine.

use crate::live::{EngineSlot, Scheduler};
use cv_model::workspace_key::WorkspaceKey;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

static SLOTS: OnceLock<Mutex<HashMap<WorkspaceKey, EngineSlot>>> = OnceLock::new();

/// The process-local workspace-instance key shared by scheduler identity
/// and outcome ownership. Exact paths remain the fallback on filesystems
/// with no device+file-index evidence.
pub fn workspace_key(root: &Path) -> WorkspaceKey {
    WorkspaceKey::new(root.to_path_buf(), cv_engine::file_identity::identity(root))
}

fn slots() -> &'static Mutex<HashMap<WorkspaceKey, EngineSlot>> {
    SLOTS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register_slot(root: &Path, slot: &EngineSlot) {
    slots()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(workspace_key(root), slot.clone());
}

/// The workspace's stable slot, creating and registering it atomically
/// when this root has never been opened in the process.
pub fn slot_or_register(root: &Path) -> EngineSlot {
    slots()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(workspace_key(root))
        .or_default()
        .clone()
}

/// The workspace's engine slot (scheduler, resync, verb delivery) as
/// registered at open time, whether or not boot has finished.
pub fn slot_for(root: &Path) -> Option<EngineSlot> {
    slots()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&workspace_key(root))
        .cloned()
}

/// The workspace's scheduler once its engine has passed the gate; `None`
/// while booting or when the gate failed (disclosed in Doctor).
pub fn scheduler_for(root: &Path) -> Option<Arc<Scheduler>> {
    slots()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&workspace_key(root))
        .and_then(|slot| slot.get().cloned())
}
