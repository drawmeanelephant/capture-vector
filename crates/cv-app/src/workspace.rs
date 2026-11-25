//! Workspace model: which project folder is open, how it was chosen
//! (`--project` → `CV_PROJECT` → most recent), and the recent list kept
//! in the user's config dir. Read and written on boot/command paths only.

use std::path::{Path, PathBuf};

pub const ENV_PROJECT: &str = "CV_PROJECT";
/// Overrides where the recent list lives (tests, portable installs).
pub const ENV_CONFIG_DIR: &str = "CV_CONFIG_DIR";
pub const RECENT_FILE: &str = "recent-workspaces.json";
pub const RECENT_MAX: usize = 8;

/// `--project <dir>` or `--project=<dir>`. rune leaves flags it doesn't
/// know alone, so this rides beside `--scene`/`--tree`.
pub fn project_arg(args: impl IntoIterator<Item = String>) -> Option<PathBuf> {
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--project" {
            return it.next().map(PathBuf::from);
        }
        if let Some(v) = a.strip_prefix("--project=") {
            return Some(v.into());
        }
    }
    None
}

/// `CV_PROJECT`, when set and non-empty.
pub fn project_env() -> Option<PathBuf> {
    std::env::var_os(ENV_PROJECT)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Explicit choices win; otherwise reopen the newest recent folder that
/// still exists.
pub fn resolve(arg: Option<PathBuf>, env: Option<PathBuf>, recent: &RecentList) -> Option<PathBuf> {
    arg.or(env)
        .or_else(|| recent.entries.iter().find(|p| p.is_dir()).cloned())
}

/// The user's home folder, when the platform environment discloses it.
pub fn home_dir() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(windows) {
        var("USERPROFILE").or_else(|| var("HOME"))
    } else {
        var("HOME")
    }
}

/// Per-user config dir for Capture Vector.
pub fn config_dir() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(d) = var(ENV_CONFIG_DIR) {
        return Some(d);
    }
    if cfg!(windows) {
        return var("APPDATA").map(|d| d.join("Capture Vector"));
    }
    if cfg!(target_os = "macos") {
        return var("HOME").map(|h| h.join("Library/Application Support/Capture Vector"));
    }
    var("XDG_CONFIG_HOME")
        .or_else(|| var("HOME").map(|h| h.join(".config")))
        .map(|d| d.join("capture-vector"))
}

/// Recently opened workspace roots, newest first.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecentList {
    pub entries: Vec<PathBuf>,
    file: Option<PathBuf>,
}

impl RecentList {
    /// A missing or unreadable file is an empty list, never an error.
    pub fn load(file: Option<PathBuf>) -> Self {
        let entries = file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|s| serde_json::from_str::<Vec<PathBuf>>(&s).ok())
            .unwrap_or_default();
        Self { entries, file }
    }

    pub fn push(&mut self, root: &Path) {
        self.entries.retain(|p| p != root);
        self.entries.insert(0, root.to_path_buf());
        self.entries.truncate(RECENT_MAX);
    }

    /// Write-then-rename so a crash never leaves a torn list.
    pub fn save(&self) -> std::io::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        if let Some(d) = file.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.entries)?)?;
        std::fs::rename(&tmp, file)
    }
}

/// `canonicalize` without Windows' `\\?\` verbatim prefix, which child
/// processes and humans both read badly.
pub fn normalize(root: &Path) -> std::io::Result<PathBuf> {
    let p = std::fs::canonicalize(root)?;
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\")
            && rest.as_bytes().get(1) == Some(&b':')
        {
            return Ok(PathBuf::from(rest));
        }
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_flag_forms() {
        let a = |v: &[&str]| project_arg(v.iter().map(|s| s.to_string()));
        assert_eq!(a(&["--scene", "x", "--project", "/w"]), Some("/w".into()));
        assert_eq!(a(&["--project=/w"]), Some("/w".into()));
        assert_eq!(a(&["--project"]), None);
        assert_eq!(a(&["--tree", "t.json"]), None);
    }

    #[test]
    fn precedence_is_arg_env_recent() {
        let t = tempfile::tempdir().unwrap();
        let live = t.path().join("live");
        std::fs::create_dir_all(&live).unwrap();
        let mut recent = RecentList::default();
        recent.push(&live);
        recent.push(&t.path().join("gone"));
        assert_eq!(resolve(None, None, &recent), Some(live.clone()));
        assert_eq!(
            resolve(None, Some("/env".into()), &recent),
            Some("/env".into())
        );
        assert_eq!(
            resolve(Some("/arg".into()), Some("/env".into()), &recent),
            Some("/arg".into())
        );
        assert_eq!(resolve(None, None, &RecentList::default()), None);
    }

    #[test]
    fn recent_list_dedupes_caps_and_round_trips() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("cfg").join(RECENT_FILE);
        let mut r = RecentList::load(Some(file.clone()));
        assert!(r.entries.is_empty());
        for i in 0..RECENT_MAX + 3 {
            r.push(Path::new(&format!("/w{i}")));
        }
        r.push(Path::new("/w5"));
        assert_eq!(r.entries.len(), RECENT_MAX);
        assert_eq!(r.entries[0], PathBuf::from("/w5"));
        assert_eq!(
            r.entries.iter().filter(|p| **p == Path::new("/w5")).count(),
            1
        );
        r.save().unwrap();
        assert_eq!(RecentList::load(Some(file.clone())).entries, r.entries);
        std::fs::write(&file, "not json").unwrap();
        assert!(RecentList::load(Some(file)).entries.is_empty());
    }
}
