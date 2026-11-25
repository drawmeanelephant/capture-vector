//! Composition-root Pages port: lifecycle verbs read confined workspace
//! files and mutate them through the scheduler-owned staged writer. Open
//! editors are part of the conflict policy: dirty buffers refuse the
//! mutation, clean buffers are reloaded in place or invalidated on moves
//! and deletes.

use crate::live::{EngineSlot, SlotEngine};
use cv_core::engine::{Engine, MoveOutcome, PageDraft, PageMove, RemoveOutcome, WriteOutcome};
use cv_model::modules::pages::port::{PagesPort, Source};
use cv_store::paths::confined_file;
use cv_ui_editor::save::fingerprint_file;
use cv_widgets::editor::host;
use std::path::{Path, PathBuf};

pub struct WorkspacePages {
    root: PathBuf,
    engine: SlotEngine,
}

impl WorkspacePages {
    pub fn new(root: &Path, slot: EngineSlot) -> Self {
        Self {
            root: root.to_path_buf(),
            engine: SlotEngine(slot),
        }
    }

    fn buffer_dirty(&self, path: &str) -> bool {
        let ses = host::session();
        let ses = ses.borrow();
        crate::file_identity::buffer_for(&self.root, path).is_some_and(|id| ses.is_dirty(id))
    }

    fn refuse_dirty(&self, path: &str) -> Result<(), String> {
        if self.buffer_dirty(path) {
            return Err(format!(
                "save or close the editor on {path} before changing the page"
            ));
        }
        Ok(())
    }

    fn close_buffer(&self, id: cv_ui_editor::BufferId) {
        let ses = host::session();
        let _ = ses.borrow_mut().close(id, true);
    }

    fn reload_buffer(&self, path: &str, text: &str, fingerprint: cv_core::engine::Fingerprint) {
        let ses = host::session();
        let id = crate::file_identity::buffer_for(&self.root, path);
        if let Some(id) = id
            && let Some(e) = ses.borrow_mut().get_mut(id)
        {
            e.buffer.reload(text, Some(fingerprint));
        }
    }
}

impl PagesPort for WorkspacePages {
    fn read(&self, path: &str) -> Result<Source, String> {
        let full = confined_file(&self.root, Path::new(path)).map_err(|e| e.to_string())?;
        let text = std::fs::read_to_string(&full).map_err(|e| e.to_string())?;
        let base = fingerprint_file(&full)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{path} no longer exists"))?;
        Ok(Source { text, base })
    }

    fn write(&mut self, draft: &PageDraft) -> Result<WriteOutcome, String> {
        if draft.base.is_some() {
            self.refuse_dirty(&draft.path)?;
        }
        let out = self
            .engine
            .write_content(draft, draft.base)
            .map_err(|e| e.to_string())?;
        if let WriteOutcome::Written { fingerprint } = out {
            self.engine.0.request_resync();
            if draft.base.is_some() {
                self.reload_buffer(&draft.path, &draft.body, fingerprint);
            }
        }
        Ok(out)
    }

    fn remove(
        &mut self,
        path: &str,
        base: cv_core::engine::Fingerprint,
    ) -> Result<RemoveOutcome, String> {
        self.refuse_dirty(path)?;
        let buffer = crate::file_identity::buffer_for(&self.root, path);
        let out = self
            .engine
            .remove_content(path, base)
            .map_err(|e| e.to_string())?;
        if out == RemoveOutcome::Removed {
            self.engine.0.request_resync();
            if let Some(id) = buffer {
                self.close_buffer(id);
            }
        }
        Ok(out)
    }

    fn rename(&mut self, mv: &PageMove) -> Result<MoveOutcome, String> {
        self.refuse_dirty(&mv.from)?;
        self.refuse_dirty(&mv.to)?;
        let from_buffer = crate::file_identity::buffer_for(&self.root, &mv.from);
        let to_buffer = crate::file_identity::buffer_for(&self.root, &mv.to);
        let out = self.engine.move_content(mv).map_err(|e| e.to_string())?;
        if matches!(out, MoveOutcome::Moved { .. }) {
            self.engine.0.request_resync();
            for id in [from_buffer, to_buffer].into_iter().flatten() {
                self.close_buffer(id);
            }
        }
        Ok(out)
    }
}
