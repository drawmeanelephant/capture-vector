//! Secrets reach transports only through a `SecretBackend`: the OS
//! keychain (mocked in CI) or a one-shot stdin channel. `Secret` has no
//! `Display`/`Serialize` and redacts `Debug`, so it can't leak into
//! logs, events, or ledger records by accident.

use crate::{PublishError, Result};
use std::io::BufRead;
use std::sync::Mutex;
use zeroize::Zeroizing;

#[cfg(any(test, feature = "mock"))]
use std::collections::BTreeMap;

/// The bytes are zeroized on drop — a Secret that outlives the call
/// can't leave a copy in freed memory.
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Self(Zeroizing::new(s.into()))
    }
    /// The only accessor; call it at the transport boundary.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

pub trait SecretBackend: Send + Sync {
    fn get(&self, name: &str) -> Result<Secret>;
}

/// No secrets at all — targets that need one refuse before contact.
pub struct NoSecrets;

impl SecretBackend for NoSecrets {
    fn get(&self, name: &str) -> Result<Secret> {
        Err(PublishError::Secret(format!(
            "no secret '{name}' available"
        )))
    }
}

/// In-memory keychain stand-in for tests/CI. Feature-gated so release
/// wiring can't select it by accident (`mock` is a default feature so
/// this crate's own tests see it; dependents set `default-features =
/// false`).
#[cfg(any(test, feature = "mock"))]
#[derive(Default)]
pub struct MockKeychain(BTreeMap<String, String>);

#[cfg(any(test, feature = "mock"))]
impl MockKeychain {
    pub fn with(mut self, name: &str, value: &str) -> Self {
        self.0.insert(name.into(), value.into());
        self
    }
}

#[cfg(any(test, feature = "mock"))]
impl SecretBackend for MockKeychain {
    fn get(&self, name: &str) -> Result<Secret> {
        self.0
            .get(name)
            .map(|v| Secret::new(v.clone()))
            .ok_or_else(|| PublishError::Secret(format!("keychain has no '{name}'")))
    }
}

/// Reads one line from a stdin-like channel, once, for one named secret.
pub struct StdinSecret<R> {
    name: String,
    src: Mutex<Option<R>>,
}

impl<R: BufRead + Send> StdinSecret<R> {
    pub fn new(name: &str, src: R) -> Self {
        Self {
            name: name.into(),
            src: Mutex::new(Some(src)),
        }
    }
}

impl<R: BufRead + Send> SecretBackend for StdinSecret<R> {
    fn get(&self, name: &str) -> Result<Secret> {
        if name != self.name {
            return Err(PublishError::Secret(format!(
                "stdin channel holds '{}', not '{name}'",
                self.name
            )));
        }
        let mut src = self.src.lock().expect("stdin secret poisoned");
        let mut r = src
            .take()
            .ok_or_else(|| PublishError::Secret("stdin channel already consumed".into()))?;
        let mut line = String::new();
        r.read_line(&mut line)?;
        let v = line.trim_end_matches(['\r', '\n']);
        if v.is_empty() {
            return Err(PublishError::Secret(format!(
                "empty secret '{name}' on stdin"
            )));
        }
        Ok(Secret::new(v))
    }
}
