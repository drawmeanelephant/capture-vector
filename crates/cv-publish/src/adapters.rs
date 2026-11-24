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
use cv_store::paths::{confined, confined_file, portable_relative};
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct Delivery<'a> {
    pub run: &'a StableId,
    pub plan: &'a SealedPlan,
    pub files: &'a [(PlanFile, Vec<u8>)],
    /// The sealed site directory the files were verified from.
    pub site: &'a Path,
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
    fn dest(&self, cfg: &TargetConfig) -> std::io::Result<PathBuf> {
        confined(&self.root, portable_relative(&cfg.destination)?)
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
        let before_contact = |e: std::io::Error| TransportFailure::before_contact(e.to_string());
        let dest = self.dest(d.config).map_err(before_contact)?;
        let paths = d
            .files
            .iter()
            .map(|(f, _)| {
                portable_relative(&f.path)?;
                let relative = Path::new(&d.config.destination).join(&f.path);
                confined_file(&self.root, &relative)
            })
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(before_contact)?;
        std::fs::create_dir_all(&dest)
            .map_err(|e| TransportFailure::before_contact(format!("create {dest:?}: {e}")))?;
        #[cfg(unix)]
        let mut parents = std::collections::BTreeSet::new();
        for ((f, bytes), p) in d.files.iter().zip(paths) {
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| TransportFailure::after_contact(format!("{}: {e}", f.path)))?;
                #[cfg(unix)]
                parents.insert(parent.to_path_buf());
            }
            let write = || -> std::io::Result<()> {
                // Fixed-length staging name (fits wherever the final name
                // does), unique per run+path; `create_new` never clobbers
                // a pre-existing sibling.
                let tag = sha256(format!("{}\0{}", d.run.0, f.path).as_bytes());
                let tag: String = tag.0[..8].iter().map(|b| format!("{b:02x}")).collect();
                let tmp = p.with_file_name(format!(".cv-tmp-{tag}"));
                let mut out = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&tmp)?;
                out.write_all(bytes)?;
                out.sync_all()?;
                drop(out);
                std::fs::rename(&tmp, &p)
            };
            write().map_err(|e| TransportFailure::after_contact(format!("{}: {e}", f.path)))?;
        }
        // A rename isn't durable until the containing directory is
        // fsynced; the journal records `Complete` on this return, so
        // make the tree match first. `dest`'s own create_dir_all needs
        // its parent fsynced for the same reason.
        #[cfg(unix)]
        {
            if let Some(dp) = dest.parent() {
                parents.insert(dp.to_path_buf());
            }
            for dir in &parents {
                std::fs::File::open(dir)
                    .and_then(|d| d.sync_all())
                    .map_err(|e| TransportFailure::after_contact(format!("fsync {dir:?}: {e}")))?;
            }
        }
        Ok(Delivered::Complete(RunStatus::Complete))
    }

    fn can_read_back(&self) -> bool {
        true
    }

    fn read_back(&self, cfg: &TargetConfig, path: &str) -> Option<Vec<u8>> {
        let relative = portable_relative(&cfg.destination)
            .ok()?
            .join(portable_relative(path).ok()?);
        std::fs::read(confined_file(&self.root, &relative).ok()?).ok()
    }

    fn probe(&self, plan: &SealedPlan, cfg: &TargetConfig) -> Option<Observation> {
        self.dest(cfg).ok()?;
        plan.validate_file_paths().ok()?;
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

/// Hand delivery to an engine verb pointed at the sealed site dir
/// (default `publish --target T --profile P --html-dir SITE`). Process
/// execution stays inside cv-engine behind the port. After the verb
/// returns, the site is re-verified against the seal: if its bytes
/// changed mid-delivery, what shipped is unknowable.
pub struct RunCommand {
    verb: fn(&TargetConfig, &Path) -> Verb,
}

impl Default for RunCommand {
    fn default() -> Self {
        Self {
            verb: |cfg, site| Verb::Custom {
                name: "publish".into(),
                args: vec![
                    "--target".into(),
                    cfg.target.id().into(),
                    "--profile".into(),
                    cfg.profile.clone(),
                    "--html-dir".into(),
                    site.to_string_lossy().into_owned(),
                ],
            },
        }
    }
}

impl RunCommand {
    pub fn new(verb: fn(&TargetConfig, &Path) -> Verb) -> Self {
        Self { verb }
    }
}

impl DeliveryAdapter for RunCommand {
    fn id(&self) -> &str {
        RUN_COMMAND
    }

    fn deliver(&self, d: &Delivery<'_>) -> Result<Delivered, TransportFailure> {
        let verb = (self.verb)(d.config, d.site);
        let out = match d.engine.run(&verb, Box::new(|_| {})) {
            Ok(ExitClass::Ok) => Ok(Delivered::Complete(RunStatus::Complete)),
            // Rejected at argument parsing: nothing was sent.
            Ok(ExitClass::UsageError) => {
                return Err(TransportFailure::before_contact("usage error"));
            }
            Ok(other) => Err(TransportFailure::after_contact(format!("exit {other:?}"))),
            Err(e) => Err(TransportFailure::after_contact(format!("engine: {e}"))),
        };
        if crate::plan::load_sealed(d.site, d.plan).is_err() {
            return Err(TransportFailure::after_contact(
                "sealed site changed during delivery",
            ));
        }
        out
    }
}

/// HTTP transport port — the shell supplies the real client.
///
/// Auth is a *header* the transport writes — `("Authorization",
/// &secret)` — never material to fold into the URL or request line.
/// Keeping the shape header-only means a transport that logs the
/// request line can't leak a credential by accident. When the real
/// transport lands it must be: https-only, explicit timeout, bounded
/// response body, no cross-origin redirect while an auth header is
/// attached, and it must never log URL+auth together.
pub trait HttpTransport {
    fn post(
        &self,
        url: &str,
        auth_header: Option<(&'static str, &Secret)>,
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
        let auth_header = match &self.secret {
            Some(name) => Some((
                "Authorization",
                d.secrets
                    .get(name)
                    .map_err(|e| TransportFailure::before_contact(e.to_string()))?,
            )),
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
            auth_header.as_ref().map(|(n, s)| (*n, s)),
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
