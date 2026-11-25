//! Composition-root registry of live engine slots by workspace root.
//! Long-running shell workers (migration adoption) reach the same
//! scheduler the watch uses instead of spawning a competing engine.

use crate::live::{EngineSlot, Scheduler};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

static SLOTS: OnceLock<Mutex<HashMap<PathBuf, EngineSlot>>> = OnceLock::new();

fn slots() -> &'static Mutex<HashMap<PathBuf, EngineSlot>> {
    SLOTS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register_slot(root: &Path, slot: &EngineSlot) {
    slots()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(root.to_path_buf(), slot.clone());
}

/// The workspace's scheduler once its engine has passed the gate; `None`
/// while booting or when the gate failed (disclosed in Doctor).
pub fn scheduler_for(root: &Path) -> Option<Arc<Scheduler>> {
    slots()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(root)
        .and_then(|slot| slot.get().cloned())
}
