//! Workspace-instance identity shared by scheduler registration and
//! generation-spanning outcome ownership. Paths are spellings, not
//! lifetimes: `.cv`-carried instance evidence is authoritative only together
//! with the containing directory's physical file identity. Device+file-index
//! identity remains the compatibility
//! fallback for evidence constructed before/without that token, exact
//! `PathBuf` fallback preserves non-UTF-8 names without lossy-string
//! collisions, and mixed evidence is conservatively unproven.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct WorkspaceKey {
    path: PathBuf,
    file: Option<(u64, u64)>,
    instance: Option<InstanceEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum InstanceEvidence {
    Token(String),
    Error(String),
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
            instance: Some(InstanceEvidence::Token(instance)),
        }
    }

    /// Identity whose `.cv` evidence exists but cannot be trusted. The
    /// error marker keeps repeated opens deterministic without making that
    /// corrupted state equal either the prior valid token or the legacy
    /// device+file-index fallback.
    pub fn with_instance_error(path: PathBuf, file: Option<(u64, u64)>, error: String) -> Self {
        Self {
            path,
            file,
            instance: Some(InstanceEvidence::Error(error)),
        }
    }

    pub fn instance_error(&self) -> Option<&str> {
        match &self.instance {
            Some(InstanceEvidence::Error(error)) => Some(error),
            _ => None,
        }
    }

    pub fn instance_token(&self) -> Option<&str> {
        match &self.instance {
            Some(InstanceEvidence::Token(token)) => Some(token),
            _ => None,
        }
    }

    pub fn file_identity(&self) -> Option<(u64, u64)> {
        self.file
    }

    fn same_container(&self, other: &Self) -> bool {
        match (self.file, other.file) {
            (Some(a), Some(b)) => a == b,
            (None, None) => self.path == other.path,
            _ => false,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Same workspace instance, not merely same spelling or copied lineage
    /// bytes. `.cv` instance evidence must agree with the containing
    /// directory's physical identity; this keeps renames/firmlink aliases
    /// unified while separating coexisting byte copies. The older
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
            (Some(InstanceEvidence::Token(a)), Some(InstanceEvidence::Token(b))) => {
                a == b && self.same_container(other)
            }
            (Some(InstanceEvidence::Error(a)), Some(InstanceEvidence::Error(b))) => {
                a == b && self.same_container(other)
            }
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
        match (&self.instance, self.file) {
            (Some(InstanceEvidence::Token(token)), _) => {
                2u8.hash(state);
                token.hash(state);
            }
            (Some(InstanceEvidence::Error(error)), _) => {
                3u8.hash(state);
                error.hash(state);
            }
            (None, Some((dev, ino))) => {
                1u8.hash(state);
                dev.hash(state);
                ino.hash(state);
            }
            (None, None) => {
                0u8.hash(state);
                self.path.hash(state);
            }
        }
        // Instance evidence equality also requires the containing directory's
        // physical identity. Use file identity when present; otherwise the
        // exact path is the only available container evidence.
        match self.file {
            Some((dev, ino)) => {
                dev.hash(state);
                ino.hash(state);
            }
            None => self.path.hash(state),
        }
    }
}
