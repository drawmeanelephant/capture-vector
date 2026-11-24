//! Audit port: artifact bytes from the built HTML tree, lifecycle
//! transitions into `.cv/audit/lifecycle.jsonl` (`cv_store::audit`).

use cv_core::rules::{DedupKey, FindingLifecycle};
use cv_model::modules::audit::AuditPort;
use cv_store::audit::{DIR, LIFECYCLE, LifecycleLog};
use std::io::Read;
use std::path::{Component, Path};

/// Rendered artifacts larger than this are reported unreadable.
const MAX_ARTIFACT_BYTES: u64 = 32 << 20;

pub struct DiskPort {
    log: Option<LifecycleLog>,
    loaded: Vec<(DedupKey, FindingLifecycle)>,
}

impl DiskPort {
    /// No workspace: transitions live for the session only.
    pub fn ephemeral() -> Self {
        Self {
            log: None,
            loaded: Vec::new(),
        }
    }

    pub fn open(root: &Path) -> Result<Self, cv_store::audit::Error> {
        let (log, loaded, _) = LifecycleLog::open(root)?;
        Ok(Self {
            log: Some(log),
            loaded,
        })
    }
}

impl AuditPort for DiskPort {
    fn read(&self, root: &str, rel: &str) -> Result<Vec<u8>, String> {
        let p = Path::new(rel);
        if !p.components().all(|c| matches!(c, Component::Normal(_))) {
            return Err(format!("{rel}: path escapes the artifact root"));
        }
        let root = Path::new(root)
            .canonicalize()
            .map_err(|e| format!("{rel}: {e}"))?;
        let path = root
            .join(p)
            .canonicalize()
            .map_err(|e| format!("{rel}: {e}"))?;
        if !path.starts_with(&root) {
            return Err(format!("{rel}: path escapes the artifact root"));
        }
        let meta = std::fs::metadata(&path).map_err(|e| format!("{rel}: {e}"))?;
        if !meta.is_file() {
            return Err(format!("{rel}: artifact is not a regular file"));
        }
        if meta.len() > MAX_ARTIFACT_BYTES {
            return Err(format!(
                "{rel}: {} bytes exceeds the audit limit",
                meta.len()
            ));
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&path)
            .and_then(|f| f.take(MAX_ARTIFACT_BYTES + 1).read_to_end(&mut bytes))
            .map_err(|e| format!("{rel}: {e}"))?;
        if bytes.len() as u64 > MAX_ARTIFACT_BYTES {
            return Err(format!("{rel}: artifact exceeds the audit limit"));
        }
        Ok(bytes)
    }

    fn lifecycle(&mut self) -> Result<Vec<(DedupKey, FindingLifecycle)>, String> {
        Ok(std::mem::take(&mut self.loaded))
    }

    fn record(&mut self, key: &DedupKey, to: FindingLifecycle) -> Result<(), String> {
        match &mut self.log {
            Some(l) => l.append(key, to).map_err(|e| e.to_string()),
            None => Ok(()),
        }
    }

    fn describe(&self) -> String {
        match self.log {
            Some(_) => format!("{DIR}/{LIFECYCLE}"),
            None => "session only (no workspace)".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn artifact_symlinks_cannot_escape_the_build_root() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("page.html");
        std::fs::write(&victim, "outside").unwrap();
        std::os::unix::fs::symlink(&victim, root.path().join("page.html")).unwrap();
        let port = DiskPort::ephemeral();
        assert!(
            port.read(root.path().to_str().unwrap(), "page.html")
                .is_err()
        );
    }
}
