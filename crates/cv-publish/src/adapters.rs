//! Delivery adapters — the extension point. An adapter receives only
//! already-verified sealed bytes and reports what it *knows*: if the
//! transport may have been touched and the outcome is unclear, it must
//! say `contacted: true` so the run lands in `Unknown`, never a guess.
//! IndexNow-class pings are an [`HttpPost`] with a different body.

use crate::Observation;
use crate::plan::{PlanFile, SealedPlan, sha256};
use crate::secrets::{Secret, SecretBackend};
use crate::targets::TargetConfig;
use cv_core::engine::{Engine, ExitClass, Verb};
use cv_core::id::StableId;
use cv_core::publish::RunStatus;
use std::path::{Path, PathBuf};

pub struct Delivery<'a> {
    pub run: &'a StableId,
    pub plan: &'a SealedPlan,
    pub files: &'a [(PlanFile, Vec<u8>)],
    pub config: &'a TargetConfig,
    pub secrets: &'a dyn SecretBackend,
    pub engine: &'a dyn Engine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivered {
    /// Accepted for asynchronous processing; confirm via reconcile.
    Submitted,
    Complete(RunStatus),
}

/// `message` must never contain secret material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportFailure {
    pub contacted: bool,
    pub message: String,
}

impl TransportFailure {
    pub fn before_contact(m: impl Into<String>) -> Self {
        Self {
            contacted: false,
            message: m.into(),
        }
    }
    pub fn after_contact(m: impl Into<String>) -> Self {
        Self {
            contacted: true,
            message: m.into(),
        }
    }
}

pub trait DeliveryAdapter {
    fn id(&self) -> &str;
    fn deliver(&self, d: &Delivery<'_>) -> Result<Delivered, TransportFailure>;
    /// Whether [`Self::read_back`] can observe the destination.
    fn can_read_back(&self) -> bool {
        false
    }
    /// Read a delivered file back from the destination (ship-the-outputs
    /// confirmation). `None` = absent at the destination.
    fn read_back(&self, _cfg: &TargetConfig, _path: &str) -> Option<Vec<u8>> {
        None
    }
    /// Read-only observation used by reconciliation. Must never deliver.
    fn probe(&self, _plan: &SealedPlan, _cfg: &TargetConfig) -> Option<Observation> {
        None
    }
}

/// Copy the sealed bytes into a folder (staged `*.tmp` + rename).
pub struct CopyToFolder {
    root: PathBuf,
}

impl CopyToFolder {
    /// `root` resolves relative destinations (the project root).
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }
    fn dest(&self, cfg: &TargetConfig) -> PathBuf {
        self.root.join(&cfg.destination)
    }
}

pub const COPY_TO_FOLDER: &str = "copy-to-folder";
pub const RUN_COMMAND: &str = "run-command";
pub const HTTP_POST: &str = "http-post";

impl DeliveryAdapter for CopyToFolder {
    fn id(&self) -> &str {
        COPY_TO_FOLDER
    }

    fn deliver(&self, d: &Delivery<'_>) -> Result<Delivered, TransportFailure> {
        let dest = self.dest(d.config);
        std::fs::create_dir_all(&dest)
            .map_err(|e| TransportFailure::before_contact(format!("create {dest:?}: {e}")))?;
        for (f, bytes) in d.files {
            let p = dest.join(&f.path);
            let write = || -> std::io::Result<()> {
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let tmp = p.with_extension("cv-tmp");
                std::fs::write(&tmp, bytes)?;
                std::fs::rename(&tmp, &p)
            };
            write().map_err(|e| TransportFailure::after_contact(format!("{}: {e}", f.path)))?;
        }
        Ok(Delivered::Complete(RunStatus::Complete))
    }

    fn can_read_back(&self) -> bool {
        true
    }

    fn read_back(&self, cfg: &TargetConfig, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.dest(cfg).join(path)).ok()
    }

    fn probe(&self, plan: &SealedPlan, cfg: &TargetConfig) -> Option<Observation> {
        let ok = plan
            .files
            .iter()
            .filter(|f| {
                self.read_back(cfg, &f.path)
                    .is_some_and(|b| sha256(&b) == f.sha256)
            })
            .count();
        Some(match ok {
            0 => Observation::NotDelivered,
            n if n == plan.files.len() => Observation::Delivered(RunStatus::Complete),
            _ => Observation::Delivered(RunStatus::Partial),
        })
    }
}

/// Hand delivery to an engine verb (default `Verb::Publish{target}`).
/// Process execution stays inside cv-engine behind the port.
pub struct RunCommand {
    verb: fn(&TargetConfig) -> Verb,
}

impl Default for RunCommand {
    fn default() -> Self {
        Self {
            verb: |cfg| Verb::Publish {
                target: cfg.target.id().into(),
            },
        }
    }
}

impl RunCommand {
    pub fn new(verb: fn(&TargetConfig) -> Verb) -> Self {
        Self { verb }
    }
}

impl DeliveryAdapter for RunCommand {
    fn id(&self) -> &str {
        RUN_COMMAND
    }

    fn deliver(&self, d: &Delivery<'_>) -> Result<Delivered, TransportFailure> {
        let verb = (self.verb)(d.config);
        match d.engine.run(&verb, Box::new(|_| {})) {
            Ok(ExitClass::Ok) => Ok(Delivered::Complete(RunStatus::Complete)),
            // Rejected at argument parsing: nothing was sent.
            Ok(ExitClass::UsageError) => Err(TransportFailure::before_contact("usage error")),
            Ok(other) => Err(TransportFailure::after_contact(format!("exit {other:?}"))),
            Err(e) => Err(TransportFailure::after_contact(format!("engine: {e}"))),
        }
    }
}

/// HTTP transport port — the shell supplies the real client.
pub trait HttpTransport {
    fn post(
        &self,
        url: &str,
        auth: Option<&Secret>,
        content_type: &str,
        body: &[u8],
    ) -> Result<u16, TransportFailure>;
}

/// POST a JSON manifest of the shipped pages (IndexNow-shaped
/// `{"host","urlList"}`) to `config.destination`.
pub struct HttpPost<T> {
    pub transport: T,
    /// Keychain name of a bearer token, if the endpoint needs one.
    pub secret: Option<String>,
}

impl<T: HttpTransport> DeliveryAdapter for HttpPost<T> {
    fn id(&self) -> &str {
        HTTP_POST
    }

    fn deliver(&self, d: &Delivery<'_>) -> Result<Delivered, TransportFailure> {
        let auth = match &self.secret {
            Some(name) => Some(
                d.secrets
                    .get(name)
                    .map_err(|e| TransportFailure::before_contact(e.to_string()))?,
            ),
            None => None,
        };
        let base = d.config.deploy.base_url.trim_end_matches('/');
        let host = base
            .split("://")
            .nth(1)
            .unwrap_or(base)
            .split('/')
            .next()
            .unwrap_or("");
        let urls: Vec<String> = d
            .plan
            .files
            .iter()
            .filter(|f| f.path.ends_with(".html"))
            .map(|f| format!("{base}/{}", f.path))
            .collect();
        let body = serde_json::to_vec(&serde_json::json!({ "host": host, "urlList": urls }))
            .map_err(|e| TransportFailure::before_contact(e.to_string()))?;
        let status = self.transport.post(
            &d.config.destination,
            auth.as_ref(),
            "application/json",
            &body,
        )?;
        match status {
            200 | 201 | 204 => Ok(Delivered::Complete(RunStatus::Complete)),
            202 => Ok(Delivered::Submitted),
            400..=499 => Ok(Delivered::Complete(RunStatus::Failed)),
            s => Err(TransportFailure::after_contact(format!("http {s}"))),
        }
    }
}
