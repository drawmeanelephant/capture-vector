//! Verification shared by the build script and the asset's unit tests.
//! This module is compiled into the application only in test builds; the
//! build script includes the same file directly.

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::io::Read;

pub const RAW_LEN: usize = 3_451_900;
pub const RAW_SHA256: &str = "21b96a0377f067833a93af3082eb28d4ffab7a8cd46bfd513286f1d64b7b0949";

fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Inflate at most `RAW_LEN + 1` bytes, then require the exact upstream
/// length and digest. A truncated, corrupted, replaced, or overlong font
/// fails the build instead of silently becoming the app's fallback.
pub fn decode_bounded(compressed: &[u8]) -> Result<Vec<u8>, String> {
    let mut raw = Vec::new();
    GzDecoder::new(compressed)
        .take((RAW_LEN + 1) as u64)
        .read_to_end(&mut raw)
        .map_err(|e| format!("invalid gzip stream: {e}"))?;
    if raw.len() != RAW_LEN {
        return Err(format!(
            "decompressed length is {}, expected {RAW_LEN}",
            raw.len()
        ));
    }
    let digest = hex_digest(&raw);
    if digest != RAW_SHA256 {
        return Err(format!(
            "decompressed sha256 is {digest}, expected {RAW_SHA256}"
        ));
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corruption_changes_the_verification_result() {
        let good = b"capture-vector";
        let mut bad = good.to_vec();
        bad[0] ^= 1;
        assert_ne!(hex_digest(good), hex_digest(&bad));
    }
}
