//! `fetch-boris` + `release`: the packaged app embeds THE pinned boris
//! (`cv_engine::locate::PINNED`, asserted equal to `fixtures/MANIFEST.toml`
//! by `cv-engine/tests/pin.rs`). No BYO engine: release only bundles the
//! binary `fetch-boris` built from the pinned tag, and both commands run
//! it through cv-engine's own version + vcsRevision gate.
//!
//! Network is touched only by an explicit `fetch-boris` run (git clone +
//! zig's package fetch); nothing here runs under `cargo test`.

use cv_core::engine::EngineSource;
use cv_engine::locate::{self, BUNDLED_DIR, EngineIdentity, PINNED, Pin};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BORIS_REPO: &str = "https://github.com/drawmeanelephant/boris.git";
/// Zig series boris v0.9.0 builds with (`minimum_zig_version` 0.17.0).
pub const ZIG_SERIES: &str = "0.17.";
const PRODUCT: &str = "capture-vector";

/// `boris/0.9.0` → `v0.9.0`.
pub fn boris_tag(pin: Pin) -> String {
    format!(
        "v{}",
        pin.version.strip_prefix("boris/").unwrap_or(pin.version)
    )
}

/// `zig build` flags: release-safe, baseline CPU (a native build SIGILLs
/// on an older machine), and the pinned revision baked in verbatim.
pub fn zig_args(pin: Pin) -> Vec<String> {
    vec![
        "build".into(),
        "--release=safe".into(),
        "-Dcpu=baseline".into(),
        format!("-Dvcs-revision={}", pin.vcs_revision),
    ]
}

/// Where `fetch-boris` stages the engine and `release` picks it up.
pub fn staged_engine(root: &Path) -> PathBuf {
    root.join("vendor")
        .join(BUNDLED_DIR)
        .join(locate::exe_name())
}

pub fn artifact_name(version: &str, os: &str, arch: &str) -> String {
    format!("{PRODUCT}-{version}-{os}-{arch}")
}

/// `[package] version` of a Cargo.toml.
pub fn package_version(manifest: &str) -> Result<String, String> {
    let t: toml::Table = manifest.parse().map_err(|e| format!("Cargo.toml: {e}"))?;
    t.get("package")
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "Cargo.toml has no [package] version".into())
}

/// The packaged `cv-app --build-info` must report this release's version
/// and commit, and resolve the bundled engine at `engine` under the pin.
pub fn check_build_info(
    v: &Value,
    version: &str,
    vcs: &str,
    engine: &Path,
    pin: Pin,
) -> Result<(), String> {
    let want = |field: &str, got: &Value, exp: &str| {
        if got.as_str() == Some(exp) {
            Ok(())
        } else {
            Err(format!("--build-info {field} is {got}, expected {exp:?}"))
        }
    };
    want("format", &v["format"], "cv-build-info")?;
    want("version", &v["version"], &format!("cv-app/{version}"))?;
    want("vcsRevision", &v["vcsRevision"], vcs)?;
    want("engine.source", &v["engine"]["source"], "Bundled")?;
    want("engine.version", &v["engine"]["version"], pin.version)?;
    let found = v["engine"]["vcsRevision"].as_str().unwrap_or_default();
    if found.is_empty() || !found.starts_with(pin.vcs_revision) {
        return Err(format!(
            "--build-info engine.vcsRevision is {found:?}, expected {:?}",
            pin.vcs_revision
        ));
    }
    let path = Path::new(v["engine"]["path"].as_str().unwrap_or_default());
    let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    };
    if !same(path, engine) {
        return Err(format!(
            "--build-info engine.path is {path:?}, expected the bundled {engine:?}"
        ));
    }
    Ok(())
}

fn run(cmd: &mut Command) -> Result<String, String> {
    let shown = format!("{cmd:?}");
    let out = cmd.output().map_err(|e| format!("{shown}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{shown}: {}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn status(cmd: &mut Command) -> Result<(), String> {
    let shown = format!("{cmd:?}");
    let s = cmd.status().map_err(|e| format!("{shown}: {e}"))?;
    s.success()
        .then_some(())
        .ok_or_else(|| format!("{shown}: {s}"))
}

fn gate(bin: &Path) -> Result<EngineIdentity, String> {
    let cwd = bin.parent().unwrap_or(Path::new("."));
    locate::gate(EngineSource::Bundled, bin, cwd, PINNED).map_err(|e| e.to_string())
}

fn sha256_file(p: &Path) -> Result<String, String> {
    let bytes = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn copy_exe(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(d) = to.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("copy {} -> {}: {e}", from.display(), to.display()))
}

/// `fetch-boris [--src <checkout>]`: clone (or reuse) boris at the pinned
/// tag, refuse any other commit, zig-build it, gate it, stage it.
pub fn fetch_boris(root: &Path, args: &[String]) -> Result<PathBuf, String> {
    let tag = boris_tag(PINNED);
    let src = match args.iter().position(|a| a == "--src") {
        Some(i) => PathBuf::from(args.get(i + 1).ok_or("--src needs a path")?),
        None => {
            let dir = root.join("vendor").join("boris");
            if dir.join(".git").exists() {
                status(
                    Command::new("git")
                        .arg("-C")
                        .arg(&dir)
                        .args(["fetch", "--depth", "1", "origin", "tag", &tag]),
                )?;
                status(
                    Command::new("git")
                        .arg("-C")
                        .arg(&dir)
                        .args(["checkout", "--detach", &tag]),
                )?;
            } else {
                status(
                    Command::new("git")
                        .args(["clone", "--depth", "1", "--branch", &tag, BORIS_REPO])
                        .arg(&dir),
                )?;
            }
            dir
        }
    };
    let head = run(Command::new("git")
        .arg("-C")
        .arg(&src)
        .args(["rev-parse", "HEAD"]))?;
    if !head.starts_with(PINNED.vcs_revision) {
        return Err(format!(
            "{} is at {head}, not the pinned {} ({tag})",
            src.display(),
            PINNED.vcs_revision
        ));
    }
    let zig = run(Command::new("zig").arg("version"))
        .map_err(|e| format!("zig {ZIG_SERIES}x is required: {e}"))?;
    if !zig.starts_with(ZIG_SERIES) {
        return Err(format!("zig {ZIG_SERIES}x is required, found {zig}"));
    }
    status(Command::new("zig").args(zig_args(PINNED)).current_dir(&src))?;
    let built = src.join("zig-out").join("bin").join(locate::exe_name());
    let id = gate(&built)?;
    let staged = staged_engine(root);
    copy_exe(&built, &staged)?;
    let info = staged.with_extension("build-info.json");
    std::fs::write(&info, format!("{}\n", id.build_info_raw))
        .map_err(|e| format!("{}: {e}", info.display()))?;
    gate(&staged)?;
    println!(
        "staged {} ({} vcs {}, sha256 {})",
        staged.display(),
        id.version,
        id.vcs_revision,
        sha256_file(&staged)?
    );
    Ok(staged)
}

fn git_vcs(root: &Path) -> Result<String, String> {
    let rev =
        run(Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["rev-parse", "--short=12", "HEAD"]))?;
    let dirty = run(Command::new("git").arg("-C").arg(root).args([
        "status",
        "--porcelain",
        "--untracked-files=no",
    ]))?;
    Ok(if dirty.is_empty() {
        rev
    } else {
        format!("{rev}-dirty")
    })
}

/// `release [--out <dir>]`: build cv-app (release, version-stamped),
/// lay out `<name>/{cv-app, engines/boris, VERSION, build-info.json}`,
/// verify the packaged binary resolves its bundled engine, and archive.
pub fn release(root: &Path, args: &[String]) -> Result<PathBuf, String> {
    let engine = staged_engine(root);
    if !engine.is_file() {
        return Err(format!(
            "{} missing: run `cargo run -p xtask -- fetch-boris` first",
            engine.display()
        ));
    }
    gate(&engine)?;
    let manifest = root.join("crates/cv-app/Cargo.toml");
    let version = package_version(
        &std::fs::read_to_string(&manifest).map_err(|e| format!("{}: {e}", manifest.display()))?,
    )?;
    let vcs = git_vcs(root)?;

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    status(
        Command::new(cargo)
            .args(["build", "--release", "--locked", "-p", "cv-app"])
            .env("CV_BUILD_VCS", &vcs)
            .current_dir(root),
    )?;
    let app_name = format!("cv-app{}", std::env::consts::EXE_SUFFIX);
    let built = root.join("target").join("release").join(&app_name);

    let out = match args.iter().position(|a| a == "--out") {
        Some(i) => PathBuf::from(args.get(i + 1).ok_or("--out needs a path")?),
        None => root.join("target").join("dist"),
    };
    let name = artifact_name(&version, std::env::consts::OS, std::env::consts::ARCH);
    let pkg = out.join(&name);
    if pkg.exists() {
        std::fs::remove_dir_all(&pkg).map_err(|e| format!("{}: {e}", pkg.display()))?;
    }
    let app = pkg.join(&app_name);
    let bundled = pkg.join(BUNDLED_DIR).join(locate::exe_name());
    copy_exe(&built, &app)?;
    copy_exe(&engine, &bundled)?;
    std::fs::write(pkg.join("VERSION"), format!("{version}\n")).map_err(|e| e.to_string())?;

    // The packaged binary, with no override in play, must find THE
    // bundled engine — the lookup order is checked end to end here.
    let raw = run(Command::new(&app)
        .arg("--build-info")
        .env_remove(locate::ENV_OVERRIDE)
        .env_remove(locate::ENV_SIBLING)
        .current_dir(&pkg))?;
    let info: Value =
        serde_json::from_str(&raw).map_err(|e| format!("cv-app --build-info: {e}: {raw}"))?;
    check_build_info(&info, &version, &vcs, &bundled, PINNED)?;
    std::fs::write(pkg.join("build-info.json"), format!("{raw}\n")).map_err(|e| e.to_string())?;

    let archive = out.join(format!("{name}.tar.gz"));
    status(
        Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&out)
            .arg(&name),
    )?;
    let sum = sha256_file(&archive)?;
    std::fs::write(
        out.join(format!("{name}.tar.gz.sha256")),
        format!("{sum}  {name}.tar.gz\n"),
    )
    .map_err(|e| e.to_string())?;
    println!("{}\n{}  sha256 {sum}", pkg.display(), archive.display());
    Ok(archive)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tag_and_zig_flags_follow_the_pin() {
        assert_eq!(boris_tag(PINNED), "v0.9.0");
        assert_eq!(
            zig_args(PINNED),
            [
                "build",
                "--release=safe",
                "-Dcpu=baseline",
                "-Dvcs-revision=66efcfb"
            ]
        );
    }

    #[test]
    fn engine_stages_where_release_and_the_locator_expect_it() {
        let p = staged_engine(Path::new("/r"));
        assert_eq!(p, Path::new("/r/vendor/engines").join(locate::exe_name()));
        assert!(p.starts_with("/r/vendor"), "vendor/ is gitignored");
    }

    #[test]
    fn artifact_and_version_naming() {
        assert_eq!(
            artifact_name("0.1.0", "linux", "x86_64"),
            "capture-vector-0.1.0-linux-x86_64"
        );
        assert_eq!(
            package_version("[package]\nname = \"cv-app\"\nversion = \"1.2.3\"\n").unwrap(),
            "1.2.3"
        );
        assert!(package_version("[package]\nversion.workspace = true\n").is_err());
        let real = std::fs::read_to_string(crate::root().join("crates/cv-app/Cargo.toml")).unwrap();
        assert!(package_version(&real).is_ok());
    }

    #[test]
    fn build_info_check_requires_the_bundled_pinned_engine() {
        let ok = json!({
            "format": "cv-build-info",
            "version": "cv-app/0.1.0",
            "vcsRevision": "abc123",
            "engine": {"source": "Bundled", "path": "pkg/engines/boris",
                       "version": "boris/0.9.0", "vcsRevision": "66efcfb"},
        });
        let e = Path::new("pkg/engines/boris");
        assert!(check_build_info(&ok, "0.1.0", "abc123", e, PINNED).is_ok());
        assert!(check_build_info(&ok, "0.2.0", "abc123", e, PINNED).is_err());
        assert!(check_build_info(&ok, "0.1.0", "other", e, PINNED).is_err());
        assert!(
            check_build_info(&ok, "0.1.0", "abc123", Path::new("/usr/bin/boris"), PINNED).is_err()
        );
        let mut from_path = ok.clone();
        from_path["engine"]["source"] = json!("Path");
        assert!(check_build_info(&from_path, "0.1.0", "abc123", e, PINNED).is_err());
        let mut absent = ok.clone();
        absent["engine"] = json!({"error": "not found"});
        assert!(check_build_info(&absent, "0.1.0", "abc123", e, PINNED).is_err());
        let mut wrong_rev = ok;
        wrong_rev["engine"]["vcsRevision"] = json!("ce40d78");
        assert!(check_build_info(&wrong_rev, "0.1.0", "abc123", e, PINNED).is_err());
    }
}
