//! `fixtures/MANIFEST.toml` — Engine-owned pin of every capture:
//! per-file sha256 + engine version + capture command. Format carries
//! its own `schema_version`.

use std::collections::BTreeMap;
use std::path::Path;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug)]
pub struct Manifest {
    pub schema_version: u32,
    /// Raw `boris --build-info` JSON recorded at capture time.
    pub engine_version: String,
    pub capture_command: String,
    pub captured: String,
    /// `captures/<corpus>/<verb>/<file>` or `corpus-src/…` → sha256 hex.
    files: BTreeMap<String, String>,
}

impl Manifest {
    pub fn load(path: &Path) -> Self {
        let raw = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("MANIFEST.toml unreadable at {path:?}: {e}"));
        Self::parse(&raw)
    }

    pub fn parse(raw: &str) -> Self {
        let table: toml::Table = raw.parse().expect("MANIFEST.toml is not valid TOML");
        let mut files = BTreeMap::new();
        let get = |k: &str| -> String {
            table
                .get(k)
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("MANIFEST.toml missing `{k}`"))
                .to_string()
        };
        for (k, v) in &table {
            if matches!(
                k.as_str(),
                "schema_version" | "engine_version" | "capture_command" | "captured"
            ) {
                continue;
            }
            if let Some(sha) = v.as_str() {
                files.insert(k.clone(), sha.to_string());
            }
        }
        let schema_version = table
            .get("schema_version")
            .and_then(|v| v.as_integer())
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(0);
        assert_eq!(
            schema_version, SCHEMA_VERSION,
            "fixture format schema_version {schema_version} unsupported"
        );
        Self {
            schema_version,
            engine_version: get("engine_version"),
            capture_command: get("capture_command"),
            captured: get("captured"),
            files,
        }
    }

    /// Every pinned path.
    pub fn pinned_files(&self) -> impl Iterator<Item = (&String, &String)> {
        self.files.iter()
    }

    /// Verify `bytes` against the pinned sha256 for `rel`.
    pub fn verify(&self, rel: &str, bytes: &[u8]) -> Result<(), String> {
        let want = self
            .files
            .get(rel)
            .ok_or_else(|| format!("{rel} not pinned in MANIFEST.toml"))?;
        let got = crate::sha256_hex(bytes);
        if &got == want {
            Ok(())
        } else {
            Err(format!("sha256 mismatch for {rel}: got {got}, want {want}"))
        }
    }
}
