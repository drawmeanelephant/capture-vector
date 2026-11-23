//! The secrets channel. Key material comes from a `SecretSource`
//! (OS keychain in the app, `MockSecretSource` in tests), lives in a
//! zeroizing buffer, and reaches Boris exactly once: on stdin of
//! `boris nostr sign --key-stdin`. Never argv, never env, never logged —
//! and any echo of it in the child's output is redacted before return.

use crate::error::Result;
use crate::exit::ExitOutcome;
use crate::proc::{CancelToken, boris_command, run_captured};
use std::path::Path;
use zeroize::Zeroizing;

pub struct SecretKey(Zeroizing<Vec<u8>>);

impl SecretKey {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }
    fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey(<redacted>)")
    }
}

pub trait SecretSource: Send + Sync {
    fn nostr_key(&self) -> Result<SecretKey>;
}

/// CI/test backend. Never reads the real `CV_NOSTR_NSEC`.
pub struct MockSecretSource {
    key: Vec<u8>,
}

impl MockSecretSource {
    pub fn new(key: impl Into<Vec<u8>>) -> Self {
        Self { key: key.into() }
    }
}

impl SecretSource for MockSecretSource {
    fn nostr_key(&self) -> Result<SecretKey> {
        Ok(SecretKey::new(self.key.clone()))
    }
}

#[derive(Debug, Clone)]
pub struct SignOutput {
    pub outcome: ExitOutcome,
    /// The signed bundle (when no `--out`), redacted.
    pub stdout: String,
    pub stderr: String,
}

const REDACTED: &str = "<redacted>";

fn redact(text: &[u8], key: &[u8]) -> String {
    let s = String::from_utf8_lossy(text).into_owned();
    let k = String::from_utf8_lossy(key);
    let k = k.trim();
    if k.is_empty() {
        s
    } else {
        s.replace(k, REDACTED)
    }
}

/// `boris nostr sign --plan PLAN --key-stdin [--out OUT]`.
pub fn nostr_sign(
    bin: &Path,
    cwd: &Path,
    plan: &Path,
    out: Option<&Path>,
    secrets: &dyn SecretSource,
    cancel: &CancelToken,
) -> Result<SignOutput> {
    let key = secrets.nostr_key()?;
    let mut cmd = boris_command(bin, cwd);
    cmd.args(["nostr", "sign", "--plan"])
        .arg(plan)
        .arg("--key-stdin");
    if let Some(o) = out {
        cmd.arg("--out").arg(o);
    }
    let mut stdin = Zeroizing::new(key.expose().to_vec());
    stdin.push(b'\n');
    let c = run_captured(cmd, Some(&stdin), cancel)?;
    Ok(SignOutput {
        outcome: c.outcome,
        stdout: redact(&c.stdout, key.expose()),
        stderr: redact(&c.stderr, key.expose()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_key() {
        let k = SecretKey::new(b"nsec1supersecret".to_vec());
        assert_eq!(format!("{k:?}"), "SecretKey(<redacted>)");
    }

    #[test]
    fn echoes_are_redacted() {
        assert_eq!(
            redact(b"bad key nsec1abc!", b"nsec1abc\n"),
            "bad key <redacted>!"
        );
    }
}
