//! The secrets channel. Key material comes from a `SecretSource`
//! (OS keychain in the app, `MockSecretSource` in tests), lives in a
//! zeroizing buffer, and reaches Boris exactly once: on stdin of
//! `boris nostr sign --key-stdin`. Never argv, never env, never logged —
//! and any echo of it in the child's output is redacted before return.

use crate::error::Result;
use crate::exit::ExitOutcome;
use crate::proc::{CancelToken, boris_command, run_captured};
use std::ffi::OsString;
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
    key: Zeroizing<Vec<u8>>,
}

impl MockSecretSource {
    pub fn new(key: impl Into<Vec<u8>>) -> Self {
        Self {
            key: Zeroizing::new(key.into()),
        }
    }
}

impl SecretSource for MockSecretSource {
    fn nostr_key(&self) -> Result<SecretKey> {
        Ok(SecretKey::new(self.key.to_vec()))
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
        return s;
    }
    // Normalized encodings a child might echo: raw, lowercase hex,
    // standard base64. Partial or line-split leaks aren't recoverable
    // by replace — structure beats scrubbing for those.
    s.replace(k, REDACTED)
        .replace(&hex_lower(key), REDACTED)
        .replace(&b64_standard(key), REDACTED)
}

fn hex_lower(key: &[u8]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

/// RFC 4648 standard base64 with padding — no dep for one encoder.
fn b64_standard(key: &[u8]) -> String {
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(key.len().div_ceil(3) * 4);
    for chunk in key.chunks(3) {
        let n = chunk.iter().fold(0u32, |acc, &b| (acc << 8) | b as u32) << (8 * (3 - chunk.len()));
        for i in 0..4 {
            out.push(if i < chunk.len() + 1 {
                B64[(n >> (18 - 6 * i)) as usize & 63] as char
            } else {
                '='
            });
        }
    }
    out
}

/// `nostr sign --plan PLAN --key-stdin [--out OUT]` as argv after the
/// binary name. The key itself never appears here.
pub fn nostr_sign_args(plan: &Path, out: Option<&Path>) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["nostr", "sign", "--plan"]
        .into_iter()
        .map(OsString::from)
        .collect();
    args.push(plan.as_os_str().to_os_string());
    args.push("--key-stdin".into());
    if let Some(out) = out {
        args.push("--out".into());
        args.push(out.as_os_str().to_os_string());
    }
    args
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
    cmd.args(nostr_sign_args(plan, out));
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

    #[test]
    fn normalized_encodings_are_redacted_too() {
        let key = b"nsec1abc";
        // hex echo
        let echoed = format!("signing with {}", hex_lower(key));
        assert_eq!(redact(echoed.as_bytes(), key), "signing with <redacted>");
        // base64 echo (b"nsec1abc" → "bnNlYzFhYmM=")
        let echoed = format!("key={}", b64_standard(key));
        assert_eq!(redact(echoed.as_bytes(), key), "key=<redacted>");
        // sanity: the encoder itself is RFC 4648-shaped
        assert_eq!(b64_standard(b"Man"), "TWFu");
        assert_eq!(
            b64_standard(b"any carnal pleasure."),
            "YW55IGNhcm5hbCBwbGVhc3VyZS4="
        );
    }
}
