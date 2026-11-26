//! Workspace-instance identity shared by scheduler registration and
//! generation-spanning outcome ownership. Paths are spellings, not
//! lifetimes: device+file-index identity recognizes aliases when the
//! platform exposes it, while exact `PathBuf` fallback preserves non-UTF-8
//! names without lossy-string collisions.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct WorkspaceKey {
    path: PathBuf,
    file: Option<(u64, u64)>,
}

impl WorkspaceKey {
    pub fn new(path: PathBuf, file: Option<(u64, u64)>) -> Self {
        Self { path, file }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Same workspace instance, not merely same spelling. File identity is
    /// authoritative when both sides have it; exact paths are the fallback
    /// only when neither side has file-index evidence. Mixed evidence is
    /// conservatively treated as unproven rather than risking foreign truth.
    pub fn same_instance(&self, other: &Self) -> bool {
        self == other
    }
}

impl PartialEq for WorkspaceKey {
    fn eq(&self, other: &Self) -> bool {
        match (self.file, other.file) {
            (Some(a), Some(b)) => a == b,
            (None, None) => self.path == other.path,
            _ => false,
        }
    }
}

impl Eq for WorkspaceKey {}

impl Hash for WorkspaceKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self.file {
            Some((dev, ino)) => {
                1u8.hash(state);
                dev.hash(state);
                ino.hash(state);
            }
            None => {
                0u8.hash(state);
                self.path.hash(state);
            }
        }
    }
}
