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

    fn buffer_dirty(path: &str) -> bool {
        let ses = host::session();
        let ses = ses.borrow();
        ses.find(path).is_some_and(|id| ses.is_dirty(id))
    }

    fn refuse_dirty(path: &str) -> Result<(), String> {
        if Self::buffer_dirty(path) {
            return Err(format!(
                "save or close the editor on {path} before changing the page"
            ));
        }
        Ok(())
    }

    fn close_buffer(path: &str) {
        let ses = host::session();
        let id = ses.borrow().find(path);
        if let Some(id) = id {
            let _ = ses.borrow_mut().close(id, true);
        }
    }

    fn reload_buffer(path: &str, text: &str, fingerprint: cv_core::engine::Fingerprint) {
        let ses = host::session();
        let id = ses.borrow().find(path);
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
            Self::refuse_dirty(&draft.path)?;
        }
        let out = self
            .engine
            .write_content(draft, draft.base)
            .map_err(|e| e.to_string())?;
        if let WriteOutcome::Written { fingerprint } = out
            && draft.base.is_some()
        {
            Self::reload_buffer(&draft.path, &draft.body, fingerprint);
        }
        Ok(out)
    }

    fn remove(
        &mut self,
        path: &str,
        base: cv_core::engine::Fingerprint,
    ) -> Result<RemoveOutcome, String> {
        Self::refuse_dirty(path)?;
        let out = self
            .engine
            .remove_content(path, base)
            .map_err(|e| e.to_string())?;
        if out == RemoveOutcome::Removed {
            Self::close_buffer(path);
        }
        Ok(out)
    }

    fn rename(&mut self, mv: &PageMove) -> Result<MoveOutcome, String> {
        Self::refuse_dirty(&mv.from)?;
        Self::refuse_dirty(&mv.to)?;
        let out = self.engine.move_content(mv).map_err(|e| e.to_string())?;
        if matches!(out, MoveOutcome::Moved { .. }) {
            Self::close_buffer(&mv.from);
            Self::close_buffer(&mv.to);
        }
        Ok(out)
    }
}
