//! Workspace-instance identity shared by scheduler registration and
//! generation-spanning outcome ownership. Paths are spellings, not
//! lifetimes: `.cv`-carried instance evidence is authoritative when both
//! sides have it. Device+file-index identity remains the compatibility
//! fallback for evidence constructed before/without that token, exact
//! `PathBuf` fallback preserves non-UTF-8 names without lossy-string
//! collisions, and mixed evidence is conservatively unproven.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct WorkspaceKey {
    path: PathBuf,
    file: Option<(u64, u64)>,
    instance: Option<String>,
}

impl WorkspaceKey {
    pub fn new(path: PathBuf, file: Option<(u64, u64)>) -> Self {
        Self {
            path,
            file,
            instance: None,
        }
    }

    /// Identity carrying `.cv` workspace-instance evidence. Moving the
    /// workspace preserves that evidence; deleting and recreating it mints
    /// a different token even if the filesystem later recycles an inode.
    pub fn with_instance(path: PathBuf, file: Option<(u64, u64)>, instance: String) -> Self {
        Self {
            path,
            file,
            instance: Some(instance),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Same workspace instance, not merely same spelling. `.cv` instance
    /// evidence is authoritative when both sides have it. The older
    /// device+file-index/exact-path evidence is only a compatibility
    /// fallback when neither side carries instance evidence; mixed evidence
    /// is conservatively treated as unproven rather than risking foreign truth.
    pub fn same_instance(&self, other: &Self) -> bool {
        self == other
    }
}

impl PartialEq for WorkspaceKey {
    fn eq(&self, other: &Self) -> bool {
        match (&self.instance, &other.instance) {
            (Some(a), Some(b)) => a == b,
            (None, None) => match (self.file, other.file) {
                (Some(a), Some(b)) => a == b,
                (None, None) => self.path == other.path,
                _ => false,
            },
            _ => false,
        }
    }
}

impl Eq for WorkspaceKey {}

impl Hash for WorkspaceKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        if let Some(instance) = &self.instance {
            2u8.hash(state);
            instance.hash(state);
            return;
        }
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
