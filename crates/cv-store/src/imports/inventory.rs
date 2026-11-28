//! `input-inventory.json` in a run directory: Capture Vector's own snapshot
//! of the content input taken immediately before an in-place utility lab
//! run. The lab's `out/` tree is foreign evidence; this inventory is how an
//! adoption detects that the workspace changed after the lab ran — any
//! addition, edit, deletion, or type change after capture shows up as a
//! mismatch against these digests and identities rather than disappearing
//! into the operation.

use super::*;
use std::io::Read;

pub const INPUT_INVENTORY: &str = "input-inventory.json";
pub const INPUT_INVENTORY_FORMAT: &str = "cv-import-input-inventory";
/// Version of `<run-id>/input-inventory.json`. Version 2 records
/// directories so an adoption can tell a real directory from an absent
/// path: an empty directory added after capture, a directory occupying a
/// journaled destination, or a file standing where a destination's parent
/// must be created are all evidence instead of invisible traversal
/// structure. Version-1 inventories record none of that and are decoded
/// without directory expectations.
pub const INPUT_INVENTORY_SCHEMA_VERSION: u32 = 2;

/// Physical file identity: device+inode on Unix, volume serial + file
/// index on Windows. Serialized as numbers; cross-platform meaning is
/// "the same storage object", never a portable value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileIdentity {
    pub volume: u64,
    pub index: u64,
}

/// One entry under the inventoried content input. `path` is the
/// `/`-separated path relative to the inventoried root; input trees may
/// contain names the portable Boris grammar rejects, so entries are not
/// constrained to portable spellings — they are evidence, not targets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputEntry {
    File {
        path: String,
        sha256: String,
        bytes: u64,
        identity: FileIdentity,
    },
    Symlink {
        path: String,
        target: String,
    },
    /// Recorded since schema version 2: an ordinary directory, including
    /// an empty one. Without it an empty directory is invisible to every
    /// freshness check.
    Directory {
        path: String,
    },
    /// Anything that is neither file nor link nor directory is recorded
    /// by type alone.
    Other {
        path: String,
    },
}

impl InputEntry {
    pub fn path(&self) -> &str {
        match self {
            Self::File { path, .. }
            | Self::Symlink { path, .. }
            | Self::Directory { path }
            | Self::Other { path } => path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputInventory {
    pub format: String,
    pub schema_version: u32,
    pub run_id: String,
    /// Validated absolute spelling of the inventoried `--root` input.
    pub source_root: String,
    pub entries: Vec<InputEntry>,
}

fn invalid_input(message: impl Into<String>) -> Error {
    Error::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.into(),
    ))
}

fn scan_dir(dir: &Path, prefix: &str, out: &mut Vec<InputEntry>) -> Result<(), Error> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().into_string().map_err(|_| {
            invalid_input(format!(
                "non-UTF-8 file name under inventoried input {}",
                dir.display()
            ))
        })?;
        names.push((name, entry.path()));
    }
    names.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, path) in names {
        let rel = format!("{prefix}{name}");
        let meta = std::fs::symlink_metadata(&path)?;
        if crate::paths::is_link(&meta) {
            let target = std::fs::read_link(&path)?;
            let target = target.to_str().ok_or_else(|| {
                invalid_input(format!("inventoried symlink {rel} has a non-UTF-8 target"))
            })?;
            out.push(InputEntry::Symlink {
                path: rel,
                target: target.to_string(),
            });
        } else if meta.is_dir() {
            out.push(InputEntry::Directory { path: rel.clone() });
            scan_dir(&path, &format!("{rel}/"), out)?;
        } else if meta.is_file() {
            let mut file = crate::open::open_existing_no_follow(&path, false)?;
            let identity = crate::identity::identity_of_file(&file)?.ok_or_else(|| {
                invalid_input(format!(
                    "inventoried file {rel} has no file identity on this platform"
                ))
            })?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            let after = file.metadata()?;
            if !after.is_file() {
                return Err(invalid_input(format!(
                    "inventoried file {rel} changed shape while it was being read"
                )));
            }
            out.push(InputEntry::File {
                path: rel,
                sha256: sha256_hex(&bytes),
                bytes: bytes.len() as u64,
                identity: FileIdentity {
                    volume: identity.0,
                    index: identity.1,
                },
            });
        } else {
            out.push(InputEntry::Other { path: rel });
        }
    }
    Ok(())
}

/// Inventory `source_root` and write the record inside the run locked by
/// `lock`, before the lab process is spawned. `source_root` must be the
/// validated absolute `--root` spelling the run record carries.
pub fn capture_input_inventory(lock: &RunLock, source_root: &Path) -> Result<PathBuf, Error> {
    let root = lock.root();
    let dir = lock.dir();
    let path = dir.join(INPUT_INVENTORY);
    refuse_links(&[&root.join(".cv"), &root.join(DIR), dir, &path])?;
    sidecar::ensure(root, &SCHEMA).map_err(Error::Schema)?;
    let meta = std::fs::symlink_metadata(source_root)?;
    if crate::paths::is_link(&meta) || !meta.is_dir() {
        return Err(invalid_input(format!(
            "inventoried input {} is not an ordinary directory",
            source_root.display()
        )));
    }
    let mut entries = Vec::new();
    scan_dir(source_root, "", &mut entries)?;
    let rec = InputInventory {
        format: INPUT_INVENTORY_FORMAT.into(),
        schema_version: INPUT_INVENTORY_SCHEMA_VERSION,
        run_id: lock.run_id().to_string(),
        source_root: source_root.display().to_string(),
        entries,
    };
    let mut bytes = serde_json::to_vec_pretty(&rec).map_err(Error::Json)?;
    bytes.push(b'\n');
    write_atomic_in_run(dir, INPUT_INVENTORY, &bytes)?;
    Ok(path)
}

/// The captured inventory plus the digest of its file bytes, which the
/// adoption journal binds.
pub struct ReadInputInventory {
    pub inventory: InputInventory,
    pub sha256: String,
}

/// The input inventory recorded for `run`, if durable and decodable.
/// `None` means the file is absent; a malformed or foreign record is an
/// error, never a silent skip.
pub fn read_input_inventory(root: &Path, run: &str) -> Result<Option<ReadInputInventory>, String> {
    let dir = run_dir(root, run).map_err(|e| e.to_string())?;
    let path = dir.join(INPUT_INVENTORY);
    refuse_links(&[&root.join(".cv"), &root.join(DIR), &dir, &path]).map_err(|e| e.to_string())?;
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{INPUT_INVENTORY}: {e}")),
    };
    #[derive(Deserialize)]
    struct Header {
        format: String,
        schema_version: u64,
    }
    let h: Header = serde_json::from_slice(&bytes)
        .map_err(|e| format!("{INPUT_INVENTORY} is not valid: {e}"))?;
    if h.format != INPUT_INVENTORY_FORMAT {
        return Err(format!(
            "{INPUT_INVENTORY} format {:?} is not {INPUT_INVENTORY_FORMAT}",
            h.format
        ));
    }
    if !(1..=u64::from(INPUT_INVENTORY_SCHEMA_VERSION)).contains(&h.schema_version) {
        return Err(format!(
            "{INPUT_INVENTORY} schema_version {} is not supported (this build reads 1..={INPUT_INVENTORY_SCHEMA_VERSION})",
            h.schema_version
        ));
    }
    let rec: InputInventory = serde_json::from_slice(&bytes)
        .map_err(|e| format!("{INPUT_INVENTORY} is not valid: {e}"))?;
    if rec.run_id != run {
        return Err(format!(
            "{INPUT_INVENTORY} names run {:?} but lies in {run:?}'s directory",
            rec.run_id
        ));
    }
    Ok(Some(ReadInputInventory {
        inventory: rec,
        sha256: sha256_hex(&bytes),
    }))
}
