//! Per-namespace `.cv/<ns>/schema.json` stamps + forward migrations.
//!
//! Every `.cv/` namespace (`research`, `ledger`, …) owns a directory and
//! a stamp `{"format":"cv-sidecar","namespace":..,"schema_version":N}`.
//! [`ensure`] runs before a namespace's files are opened:
//!
//! - no directory (or an empty one) → fresh: stamp `current`, nothing to
//!   migrate;
//! - files but no stamp → written before stamping existed: version 0;
//! - `found < current` → each [`Migration`] runs in order and the stamp
//!   is rewritten (atomically) after every step, so a crash resumes at
//!   the last completed step;
//! - `found > current`, a missing step, a foreign/garbled stamp → typed
//!   error and nothing on disk is touched. Never a silent downgrade.

use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const FORMAT: &str = "cv-sidecar";
pub const STAMP: &str = "schema.json";

/// Upgrades a namespace directory from `from` to `from + 1`.
#[derive(Clone, Copy)]
pub struct Migration {
    pub from: u32,
    pub run: fn(&Path) -> std::io::Result<()>,
}

/// One `.cv/` namespace and the schema this build reads and writes.
#[derive(Clone, Copy)]
pub struct Namespace {
    pub name: &'static str,
    /// Directory relative to the project root, e.g. `.cv/research`.
    pub dir: &'static str,
    pub current: u32,
    pub migrations: &'static [Migration],
}

/// Stamps a namespace written before `schema.json` existed (version 0)
/// as version 1 without touching its files: the on-disk shape is v1.
pub const ADOPT_UNSTAMPED: Migration = Migration {
    from: 0,
    run: |_| Ok(()),
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Fresh,
    Current,
    Migrated { from: u32, to: u32 },
}

#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    #[error(
        "{namespace}: schema_version {found} was written by a newer Capture Vector (this build reads <= {supported})"
    )]
    TooNew {
        namespace: &'static str,
        found: u32,
        supported: u32,
    },
    #[error("{namespace}: no migration from schema_version {from}")]
    NoMigration { namespace: &'static str, from: u32 },
    #[error("{namespace}: migration {from} -> {} failed: {source}", .from + 1)]
    MigrationFailed {
        namespace: &'static str,
        from: u32,
        source: std::io::Error,
    },
    #[error("{namespace}: invalid stamp {path:?}: {msg}")]
    Malformed {
        namespace: &'static str,
        path: PathBuf,
        msg: String,
    },
    #[error("{namespace}: {path:?} is a symlink; refusing to write outside the workspace")]
    Escapes {
        namespace: &'static str,
        path: PathBuf,
    },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub fn stamp_path(root: &Path, ns: &Namespace) -> PathBuf {
    root.join(ns.dir).join(STAMP)
}

/// Version on disk: `None` for a fresh namespace, `Some(0)` for files
/// without a stamp.
pub fn read_version(root: &Path, ns: &Namespace) -> Result<Option<u32>, SidecarError> {
    let dir = root.join(ns.dir);
    let path = dir.join(STAMP);
    let bad = |msg: String| SidecarError::Malformed {
        namespace: ns.name,
        path: path.clone(),
        msg,
    };
    match std::fs::read(&path) {
        Ok(raw) => {
            let v: Value = serde_json::from_slice(&raw).map_err(|e| bad(e.to_string()))?;
            if v["format"] != FORMAT {
                return Err(bad(format!("format {} is not {FORMAT:?}", v["format"])));
            }
            if v["namespace"] != ns.name {
                return Err(bad(format!(
                    "namespace {} is not {:?}",
                    v["namespace"], ns.name
                )));
            }
            let n = v["schema_version"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| bad("schema_version is not a u32".into()))?;
            Ok(Some(n))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let populated = match std::fs::read_dir(&dir) {
                Ok(mut rd) => rd.next().is_some(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                Err(e) => return Err(e.into()),
            };
            Ok(populated.then_some(0))
        }
        Err(e) => Err(e.into()),
    }
}

fn write_stamp(root: &Path, ns: &Namespace, version: u32) -> std::io::Result<()> {
    let dir = root.join(ns.dir);
    std::fs::create_dir_all(&dir)?;
    let body = json!({"format": FORMAT, "namespace": ns.name, "schema_version": version});
    let tmp = dir.join(format!("{STAMP}.tmp"));
    // `.cv/` ships inside cloned projects: never follow a planted
    // `schema.json.tmp` symlink. Unlink it, then create exclusively
    // (O_EXCL refuses symlinks).
    match std::fs::remove_file(&tmp) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(format!("{body}\n").as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, dir.join(STAMP))
}

/// `root/<ns.dir>.schema.lock`: serializes stamping and migration across
/// processes (and threads) opening the same workspace.
pub fn lock_path(root: &Path, ns: &Namespace) -> PathBuf {
    root.join(format!("{}.schema.lock", ns.dir))
}

/// `.cv/` comes with the project: every path component we would write
/// through (the namespace dirs, its stamp and lock) must be a real
/// entry inside `root`, not a symlink to somewhere else.
fn check_contained(root: &Path, ns: &Namespace) -> Result<(), SidecarError> {
    let mut paths = Vec::new();
    let mut p = root.to_path_buf();
    for c in Path::new(ns.dir).components() {
        p.push(c);
        paths.push(p.clone());
    }
    paths.push(stamp_path(root, ns));
    paths.push(root.join(ns.dir).join(format!("{STAMP}.tmp")));
    paths.push(lock_path(root, ns));
    for path in paths {
        match std::fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(SidecarError::Escapes {
                    namespace: ns.name,
                    path,
                });
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Bring `root/<ns.dir>` to `ns.current`, or fail without writing.
pub fn ensure(root: &Path, ns: &Namespace) -> Result<Outcome, SidecarError> {
    check_contained(root, ns)?;
    // A newer schema fails before anything (even the lock) is created.
    if let Some(found) = read_version(root, ns)?
        && found > ns.current
    {
        return Err(SidecarError::TooNew {
            namespace: ns.name,
            found,
            supported: ns.current,
        });
    }
    let lock = lock_path(root, ns);
    if let Some(parent) = lock.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = crate::open::open_create_no_follow(&lock)?;
    file.lock()?;
    // Re-read under the lock: another opener may have stamped or migrated.
    ensure_locked(root, ns)
}

fn ensure_locked(root: &Path, ns: &Namespace) -> Result<Outcome, SidecarError> {
    let Some(found) = read_version(root, ns)? else {
        write_stamp(root, ns, ns.current)?;
        return Ok(Outcome::Fresh);
    };
    if found > ns.current {
        return Err(SidecarError::TooNew {
            namespace: ns.name,
            found,
            supported: ns.current,
        });
    }
    if found == ns.current {
        return Ok(Outcome::Current);
    }
    // Check the whole chain exists before running any step.
    let steps: Vec<&Migration> = (found..ns.current)
        .map(|v| {
            ns.migrations
                .iter()
                .find(|m| m.from == v)
                .ok_or(SidecarError::NoMigration {
                    namespace: ns.name,
                    from: v,
                })
        })
        .collect::<Result<_, _>>()?;
    let dir = root.join(ns.dir);
    for m in steps {
        (m.run)(&dir).map_err(|source| SidecarError::MigrationFailed {
            namespace: ns.name,
            from: m.from,
            source,
        })?;
        write_stamp(root, ns, m.from + 1)?;
    }
    Ok(Outcome::Migrated {
        from: found,
        to: ns.current,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1: Namespace = Namespace {
        name: "alpha",
        dir: ".cv/alpha",
        current: 1,
        migrations: &[ADOPT_UNSTAMPED],
    };

    /// v1 → v2 renames `data.jsonl` to `records.jsonl`; v2 → v3 appends
    /// a line. Exercises a real multi-step chain.
    const V3: Namespace = Namespace {
        name: "alpha",
        dir: ".cv/alpha",
        current: 3,
        migrations: &[
            ADOPT_UNSTAMPED,
            Migration {
                from: 1,
                run: |d| std::fs::rename(d.join("data.jsonl"), d.join("records.jsonl")),
            },
            Migration {
                from: 2,
                run: |d| {
                    let p = d.join("records.jsonl");
                    let mut s = std::fs::read_to_string(&p)?;
                    s.push_str("{\"v3\":true}\n");
                    std::fs::write(p, s)
                },
            },
        ],
    };

    fn put(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn stamp(root: &Path, ns: &str, v: u32) {
        put(
            root,
            &format!(".cv/{ns}/{STAMP}"),
            &json!({"format": FORMAT, "namespace": ns, "schema_version": v}).to_string(),
        );
    }

    #[test]
    fn fresh_namespace_is_stamped_current() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(ensure(t.path(), &V3).unwrap(), Outcome::Fresh);
        assert_eq!(read_version(t.path(), &V3).unwrap(), Some(3));
        assert_eq!(ensure(t.path(), &V3).unwrap(), Outcome::Current);
    }

    #[test]
    fn unstamped_files_are_version_zero_and_adopted() {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), ".cv/alpha/data.jsonl", "{}\n");
        assert_eq!(read_version(t.path(), &V1).unwrap(), Some(0));
        assert_eq!(
            ensure(t.path(), &V1).unwrap(),
            Outcome::Migrated { from: 0, to: 1 }
        );
        assert_eq!(
            std::fs::read_to_string(t.path().join(".cv/alpha/data.jsonl")).unwrap(),
            "{}\n"
        );
    }

    #[test]
    fn older_schema_runs_every_step_in_order() {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), ".cv/alpha/data.jsonl", "{\"v1\":true}\n");
        stamp(t.path(), "alpha", 1);
        assert_eq!(
            ensure(t.path(), &V3).unwrap(),
            Outcome::Migrated { from: 1, to: 3 }
        );
        let d = t.path().join(".cv/alpha");
        assert!(!d.join("data.jsonl").exists());
        assert_eq!(
            std::fs::read_to_string(d.join("records.jsonl")).unwrap(),
            "{\"v1\":true}\n{\"v3\":true}\n"
        );
        assert_eq!(read_version(t.path(), &V3).unwrap(), Some(3));
        assert!(!d.join(format!("{STAMP}.tmp")).exists());
    }

    #[test]
    fn failed_step_keeps_the_last_completed_stamp() {
        let t = tempfile::tempdir().unwrap();
        stamp(t.path(), "alpha", 1);
        // No data.jsonl: the 1 → 2 rename fails.
        let err = ensure(t.path(), &V3).unwrap_err();
        assert!(
            matches!(err, SidecarError::MigrationFailed { from: 1, .. }),
            "{err}"
        );
        assert_eq!(read_version(t.path(), &V3).unwrap(), Some(1));
    }

    #[test]
    fn newer_schema_fails_and_touches_nothing() {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), ".cv/alpha/data.jsonl", "future\n");
        stamp(t.path(), "alpha", 9);
        let before = std::fs::read(stamp_path(t.path(), &V3)).unwrap();
        let err = ensure(t.path(), &V3).unwrap_err();
        assert!(matches!(
            err,
            SidecarError::TooNew {
                found: 9,
                supported: 3,
                ..
            }
        ));
        assert!(err.to_string().contains("newer Capture Vector"));
        assert_eq!(std::fs::read(stamp_path(t.path(), &V3)).unwrap(), before);
        assert_eq!(
            std::fs::read_to_string(t.path().join(".cv/alpha/data.jsonl")).unwrap(),
            "future\n"
        );
    }

    #[test]
    fn a_gap_in_the_chain_fails_before_any_step_runs() {
        const GAP: Namespace = Namespace {
            migrations: &[ADOPT_UNSTAMPED, V3.migrations[2]],
            ..V3
        };
        let t = tempfile::tempdir().unwrap();
        put(t.path(), ".cv/alpha/data.jsonl", "{}\n");
        let err = ensure(t.path(), &GAP).unwrap_err();
        assert!(matches!(err, SidecarError::NoMigration { from: 1, .. }));
        // Step 0 → 1 did not run either: still unstamped.
        assert_eq!(read_version(t.path(), &GAP).unwrap(), Some(0));
    }

    #[test]
    fn foreign_or_garbled_stamps_are_errors() {
        let t = tempfile::tempdir().unwrap();
        for body in [
            "not json".to_string(),
            json!({"format": "other", "namespace": "alpha", "schema_version": 1}).to_string(),
            json!({"format": FORMAT, "namespace": "beta", "schema_version": 1}).to_string(),
            json!({"format": FORMAT, "namespace": "alpha", "schema_version": "1"}).to_string(),
        ] {
            put(t.path(), ".cv/alpha/schema.json", &body);
            assert!(
                matches!(ensure(t.path(), &V1), Err(SidecarError::Malformed { .. })),
                "{body}"
            );
        }
    }

    #[test]
    fn namespaces_are_versioned_independently() {
        const BETA: Namespace = Namespace {
            name: "beta",
            dir: ".cv/beta",
            current: 1,
            migrations: &[ADOPT_UNSTAMPED],
        };
        let t = tempfile::tempdir().unwrap();
        put(t.path(), ".cv/alpha/data.jsonl", "{}\n");
        stamp(t.path(), "alpha", 1);
        stamp(t.path(), "beta", 7);
        assert!(matches!(
            ensure(t.path(), &BETA),
            Err(SidecarError::TooNew { found: 7, .. })
        ));
        assert_eq!(
            ensure(t.path(), &V3).unwrap(),
            Outcome::Migrated { from: 1, to: 3 }
        );
        assert_eq!(read_version(t.path(), &BETA).unwrap(), Some(7));
    }

    #[cfg(unix)]
    #[test]
    fn planted_tmp_symlink_is_not_followed() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("victim");
        std::fs::write(&victim, "keep").unwrap();
        let dir = root.path().join(V1.dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("data.jsonl"), "{}\n").unwrap();
        std::os::unix::fs::symlink(&victim, dir.join(format!("{STAMP}.tmp"))).unwrap();
        let err = ensure(root.path(), &V1).unwrap_err();
        assert!(matches!(err, SidecarError::Escapes { .. }), "{err}");
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
        assert_eq!(read_version(root.path(), &V1).unwrap(), Some(0));
    }

    #[test]
    fn concurrent_fresh_opens_stamp_once() {
        let root = tempfile::tempdir().unwrap();
        let barrier = std::sync::Barrier::new(12);
        let outcomes: Vec<Outcome> = std::thread::scope(|sc| {
            let hs: Vec<_> = (0..12)
                .map(|_| {
                    sc.spawn(|| {
                        barrier.wait();
                        ensure(root.path(), &V1).unwrap()
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(outcomes.iter().filter(|o| **o == Outcome::Fresh).count(), 1);
        assert!(
            outcomes
                .iter()
                .all(|o| matches!(o, Outcome::Fresh | Outcome::Current))
        );
        assert_eq!(read_version(root.path(), &V1).unwrap(), Some(1));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_cv_dir_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join(".cv")).unwrap();
        let err = ensure(root.path(), &V1).unwrap_err();
        assert!(matches!(err, SidecarError::Escapes { .. }), "{err}");
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }
}
