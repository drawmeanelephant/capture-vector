//! `cv-app --version` / `--build-info`. The version is `cv-app`'s
//! `CARGO_PKG_VERSION`; `xtask release` stamps the commit through
//! `CV_BUILD_VCS` at compile time (empty for plain `cargo build`).

use cv_engine::locate::{EngineIdentity, Locator, PINNED};
use serde_json::{Value, json};
use std::path::Path;

pub const FORMAT: &str = "cv-build-info";
pub const SCHEMA_VERSION: &str = "1";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const VCS_REVISION: &str = match option_env!("CV_BUILD_VCS") {
    Some(v) => v,
    None => "",
};

const TOOLCHAIN_FILE: &str = include_str!("../../../rust-toolchain.toml");

/// The Rust toolchain the checkout pins (`rust-toolchain.toml`'s
/// `channel`), read at compile time.
pub fn toolchain() -> &'static str {
    TOOLCHAIN_FILE
        .lines()
        .find_map(|l| l.trim().strip_prefix("channel"))
        .and_then(|v| v.trim().strip_prefix('='))
        .map(|v| v.trim().trim_matches('"'))
        .unwrap_or("unknown")
}

pub fn version_line() -> String {
    format!("cv-app/{VERSION}")
}

/// The build record plus the engine this binary resolves right now
/// (`CV_BORIS_BIN` → bundled → PATH → sibling, gated against the pin).
pub fn report(engine: Result<&EngineIdentity, String>) -> Value {
    let engine = match engine {
        Ok(id) => json!({
            "source": id.source,
            "path": id.path,
            "version": id.version,
            "vcsRevision": id.vcs_revision,
        }),
        Err(e) => json!({ "error": e }),
    };
    json!({
        "format": FORMAT,
        "schemaVersion": SCHEMA_VERSION,
        "version": version_line(),
        "vcsRevision": VCS_REVISION,
        "target": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        "enginePin": { "version": PINNED.version, "vcsRevision": PINNED.vcs_revision },
        "engine": engine,
    })
}

/// Resolve the engine from `cwd` and render [`report`].
pub fn current(cwd: &Path) -> Value {
    let id = Locator::from_env(cwd).locate(cwd, PINNED);
    report(id.as_ref().map_err(ToString::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cv_core::engine::EngineSource;

    #[test]
    fn report_carries_package_version_and_pin() {
        let v = report(Err("no boris".into()));
        assert_eq!(v["format"], FORMAT);
        assert_eq!(
            v["version"],
            format!("cv-app/{}", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(v["enginePin"]["version"], "boris/0.9.0");
        assert_eq!(v["enginePin"]["vcsRevision"], "66efcfb");
        assert_eq!(v["engine"]["error"], "no boris");
        assert_eq!(v["vcsRevision"], VCS_REVISION);
    }

    #[test]
    fn toolchain_is_the_pinned_channel() {
        let t = toolchain();
        assert!(t.split('.').count() == 3, "{t}");
        assert!(TOOLCHAIN_FILE.contains(&format!("channel = \"{t}\"")));
    }

    #[test]
    fn report_discloses_where_the_engine_came_from() {
        let id = EngineIdentity {
            source: EngineSource::Bundled,
            path: "engines/boris".into(),
            version: "boris/0.9.0".into(),
            vcs_revision: "66efcfb".into(),
            build_info_raw: String::new(),
        };
        let v = report(Ok(&id));
        assert_eq!(v["engine"]["source"], "Bundled");
        assert_eq!(v["engine"]["path"], "engines/boris");
        assert_eq!(v["engine"]["vcsRevision"], "66efcfb");
    }
}
