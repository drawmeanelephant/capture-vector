//! Captured `boris-migration-lab` runs and `StubLab`, which replays them.
//!
//! Each capture is one pinned-lab run over the lab's own fixtures,
//! recorded by `scripts/capture-lab-fixtures.sh` under
//! `fixtures/captures/lab/<mode>/`:
//!
//! - `argv.txt`: the argv `cv_engine::lab::PreparedRun` built, one per line;
//! - `exit-code.txt`, `stdout.txt`, `stderr.txt`;
//! - `out/`: every file the lab wrote under `--out`.
//!
//! The lab writes the absolute input and output paths it was given into
//! its reports, its stderr and some emitted frontmatter. A capture runs in
//! a fresh scratch root, and [`normalize`] replaces that root with
//! [`CAPTURE_ROOT`] so the bytes do not depend on the host. Nothing else
//! is rewritten.

use crate::manifest::Manifest;
use crate::{check_rel, fixtures_root};
use cv_contracts::lab::{DecodeError, Document};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

/// `fixtures/captures/<LAB_DIR>/`: not a Boris corpus.
pub const LAB_DIR: &str = "lab";

/// What a capture's scratch root reads as in every captured file.
pub const CAPTURE_ROOT: &str = "/CAPTURE_ROOT";

/// Where one input comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabInput {
    /// A path under the lab checkout's `fixtures/` at the pinned commit.
    /// Its first segment is copied to `<root>/src/`.
    Fixture(&'static str),
    /// `<root>/site-<mode>/dist`: the pinned Boris's HTML build of capture
    /// `<mode>`'s `content/`, in a `boris init` scaffold
    /// (`build --profile boris.json`).
    BorisHtmlOf(&'static str),
}

/// One captured run: `mode` with these `(flag, input)` pairs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabCaptureSpec {
    pub mode: &'static str,
    pub inputs: &'static [(&'static str, LabInput)],
}

impl LabCaptureSpec {
    /// `<root>/out/<mode>`.
    pub fn out(&self, root: &Path) -> PathBuf {
        root.join("out").join(self.mode)
    }
}

impl LabInput {
    /// Where this input sits under a capture root.
    pub fn staged(self, root: &Path) -> PathBuf {
        match self {
            Self::Fixture(rel) => root.join("src").join(rel),
            Self::BorisHtmlOf(mode) => site_dir(root, mode).join("dist"),
        }
    }
}

/// `<root>/site-<mode>`, the Boris scaffold for [`LabInput::BorisHtmlOf`].
pub fn site_dir(root: &Path, mode: &str) -> PathBuf {
    root.join(format!("site-{mode}"))
}

use LabInput::{BorisHtmlOf, Fixture};

/// Every captured run, in capture order (a [`LabInput::BorisHtmlOf`]
/// input names an earlier row). One capture per mode.
pub const LAB_CAPTURES: &[LabCaptureSpec] = &[
    LabCaptureSpec {
        mode: "hugo-inventory",
        inputs: &[("--root", Fixture("mini-hugo"))],
    },
    LabCaptureSpec {
        mode: "hugo",
        inputs: &[("--root", Fixture("mini-hugo"))],
    },
    LabCaptureSpec {
        mode: "jekyll-inventory",
        inputs: &[("--root", Fixture("mini-jekyll"))],
    },
    LabCaptureSpec {
        mode: "jekyll",
        inputs: &[("--root", Fixture("mini-jekyll"))],
    },
    LabCaptureSpec {
        mode: "substack-inventory",
        inputs: &[("--root", Fixture("mini-substack"))],
    },
    LabCaptureSpec {
        mode: "substack",
        inputs: &[("--root", Fixture("mini-substack"))],
    },
    LabCaptureSpec {
        mode: "enex-inventory",
        inputs: &[("--root", Fixture("mini-enex"))],
    },
    LabCaptureSpec {
        mode: "enex",
        inputs: &[("--root", Fixture("mini-enex"))],
    },
    LabCaptureSpec {
        mode: "obsidian",
        inputs: &[("--vault", Fixture("mini-obsidian"))],
    },
    LabCaptureSpec {
        mode: "wordpress",
        inputs: &[
            ("--wxr", Fixture("mini-wxr/export.xml")),
            ("--media", Fixture("mini-wxr/media")),
        ],
    },
    LabCaptureSpec {
        mode: "link-audit",
        inputs: &[("--root", BorisHtmlOf("hugo"))],
    },
    LabCaptureSpec {
        mode: "frontmatter-review",
        inputs: &[("--content", Fixture("fm-review-mixed/content"))],
    },
];

pub fn spec(mode: &str) -> Option<&'static LabCaptureSpec> {
    LAB_CAPTURES.iter().find(|s| s.mode == mode)
}

/// Copy the fixture holding `rel` from `lab_fixtures` into `<root>/src/`
/// (once per fixture) and return the staged path of `rel`.
pub fn stage_fixture(lab_fixtures: &Path, rel: &str, root: &Path) -> io::Result<PathBuf> {
    check_rel(rel);
    let top = rel.split('/').next().expect("non-empty rel");
    let dst = root.join("src").join(top);
    if !dst.exists() {
        copy_tree(&lab_fixtures.join(top), &dst)?;
    }
    Ok(root.join("src").join(rel))
}

/// Recursive copy that refuses symlinks: the lab refuses them too, and a
/// followed link would stage bytes from outside the fixture.
pub fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    let mut entries: Vec<_> = std::fs::read_dir(from)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let ty = e.file_type()?;
        let dst = to.join(e.file_name());
        if ty.is_symlink() {
            return Err(io::Error::other(format!("refusing symlink {:?}", e.path())));
        } else if ty.is_dir() {
            copy_tree(&e.path(), &dst)?;
        } else {
            std::fs::copy(e.path(), dst)?;
        }
    }
    Ok(())
}

/// Replace occurrences of `root` with [`CAPTURE_ROOT`] in UTF-8 text when
/// they are whole paths or path prefixes, across the raw, JSON-escaped,
/// Windows verbatim/non-verbatim, and slash-normalized spellings a captured
/// report can carry. A match must end the path token or continue at a
/// separator valid for that spelling, so sibling names such as `root2` and
/// Unix filenames containing backslashes are not rewritten. Other bytes
/// (images) pass through unchanged.
pub fn normalize(bytes: &[u8], root: &Path) -> Vec<u8> {
    let root = root.to_str().expect("capture root is UTF-8");
    match std::str::from_utf8(bytes) {
        Ok(s) => normalize_text(s, &root_spellings(root)).into_bytes(),
        Err(_) => bytes.to_vec(),
    }
}

struct RootSpelling {
    text: String,
    separators: Vec<&'static str>,
}

fn root_spellings(root: &str) -> Vec<RootSpelling> {
    let mut bases = vec![root.to_string()];
    if let Some(non_verbatim) = non_verbatim_windows(root) {
        bases.push(non_verbatim);
    }

    let mut spellings = Vec::new();
    for base in bases {
        let windows = looks_windows_path(&base);
        let raw_separators: &[&str] = if windows { &["/", "\\"] } else { &["/"] };
        push_spelling(&mut spellings, base.clone(), raw_separators);
        let escaped_separators: &[&str] = if windows { &["/", "\\\\"] } else { &["/"] };
        push_spelling(&mut spellings, json_escaped(&base), escaped_separators);
        if windows {
            let slashes = base.replace('\\', "/");
            push_spelling(&mut spellings, slashes.clone(), raw_separators);
            push_spelling(&mut spellings, json_escaped(&slashes), escaped_separators);
        }
    }
    spellings.sort_by(|a, b| {
        b.text
            .len()
            .cmp(&a.text.len())
            .then_with(|| a.text.cmp(&b.text))
    });
    spellings
}

fn push_spelling(spellings: &mut Vec<RootSpelling>, text: String, separators: &[&'static str]) {
    if text.is_empty() {
        return;
    }
    if let Some(existing) = spellings.iter_mut().find(|s| s.text == text) {
        for separator in separators {
            if !existing.separators.contains(separator) {
                existing.separators.push(separator);
            }
        }
    } else {
        spellings.push(RootSpelling {
            text,
            separators: separators.to_vec(),
        });
    }
}

fn normalize_text(input: &str, spellings: &[RootSpelling]) -> String {
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0;
    while cursor < input.len() {
        let rest = &input[cursor..];
        if let Some(spelling) = spellings.iter().find(|spelling| {
            rest.starts_with(&spelling.text)
                && path_start_boundary(input, cursor)
                && path_end_boundary(input, cursor + spelling.text.len(), spelling)
        }) {
            out.push_str(CAPTURE_ROOT);
            cursor += spelling.text.len();
        } else {
            let c = rest.chars().next().expect("cursor is a char boundary");
            out.push(c);
            cursor += c.len_utf8();
        }
    }
    out
}

fn path_start_boundary(input: &str, start: usize) -> bool {
    if start == 0 {
        return true;
    }
    input[..start]
        .chars()
        .next_back()
        .is_some_and(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '=' | '(' | '[' | '{'))
}

fn path_end_boundary(input: &str, end: usize, spelling: &RootSpelling) -> bool {
    let rest = &input[end..];
    if rest.is_empty()
        || spelling
            .separators
            .iter()
            .any(|separator| rest.starts_with(separator))
    {
        return true;
    }
    rest.chars()
        .next()
        .is_some_and(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '`'))
}

fn json_escaped(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c <= '\u{1f}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn non_verbatim_windows(path: &str) -> Option<String> {
    for (prefix, unc_marker, replacement) in [
        ("\\\\?\\", "UNC\\", "\\\\"),
        ("\\\\.\\", "UNC\\", "\\\\"),
        ("//?/", "UNC/", "//"),
        ("//./", "UNC/", "//"),
    ] {
        if let Some(rest) = path.strip_prefix(prefix) {
            if let Some(unc) = rest.strip_prefix(unc_marker) {
                return Some(format!("{replacement}{unc}"));
            }
            if is_drive_path(rest) {
                return Some(rest.to_string());
            }
        }
    }
    None
}

fn looks_windows_path(path: &str) -> bool {
    is_drive_path(path)
        || path.starts_with("\\\\?\\")
        || path.starts_with("\\\\.\\")
        || path.starts_with("//?/")
        || path.starts_with("//./")
        || (path.starts_with("\\\\") && path.len() > 2)
        || (path.starts_with("//") && path.len() > 2)
}

fn is_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn normalize_str(s: &str, root: &Path) -> String {
    String::from_utf8(normalize(s.as_bytes(), root)).expect("normalizing UTF-8 keeps it UTF-8")
}

/// What one lab run showed, with its scratch root normalized. Stub and
/// process runs are compared as transcripts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabTranscript {
    /// The argv after the binary.
    pub argv: Vec<String>,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    /// Every file under `--out`: `/`-separated relative path → bytes.
    pub out: BTreeMap<String, Vec<u8>>,
}

impl LabTranscript {
    /// Read a finished run whose paths all sit under the canonical `root`.
    pub fn observe(
        root: &Path,
        argv: &[OsString],
        exit_code: i32,
        stdout: &str,
        stderr: &str,
        out_dir: &Path,
    ) -> io::Result<Self> {
        let mut out = BTreeMap::new();
        if out_dir.exists() {
            read_tree(out_dir, "", &mut out)?;
        }
        for bytes in out.values_mut() {
            *bytes = normalize(bytes, root);
        }
        Ok(Self {
            argv: argv
                .iter()
                .map(|a| normalize_str(&a.to_string_lossy(), root))
                .collect(),
            exit_code,
            stdout: normalize_str(stdout, root),
            stderr: normalize_str(stderr, root),
            out,
        })
    }

    /// Write the capture layout into `dir` (which should be empty).
    pub fn write_capture(&self, dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let argv: String = self.argv.iter().map(|a| format!("{a}\n")).collect();
        std::fs::write(dir.join("argv.txt"), argv)?;
        std::fs::write(dir.join("exit-code.txt"), format!("{}\n", self.exit_code))?;
        std::fs::write(dir.join("stdout.txt"), &self.stdout)?;
        std::fs::write(dir.join("stderr.txt"), &self.stderr)?;
        write_tree(&self.out, &dir.join("out"))
    }

    /// Decode the JSON document at `rel` under `--out`.
    pub fn document(&self, rel: &str) -> Option<Result<Document, DecodeError>> {
        self.out.get(rel).map(|b| Document::decode(rel, b))
    }
}

fn write_tree(files: &BTreeMap<String, Vec<u8>>, dir: &Path) -> io::Result<()> {
    for (rel, bytes) in files {
        check_rel(rel);
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().expect("file under dir"))?;
        std::fs::write(path, bytes)?;
    }
    Ok(())
}

fn read_tree(dir: &Path, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) -> io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let name = e
            .file_name()
            .into_string()
            .map_err(|n| io::Error::other(format!("non-UTF-8 file name {n:?} under {dir:?}")))?;
        let rel = format!("{prefix}{name}");
        let ty = e.file_type()?;
        if ty.is_symlink() {
            return Err(io::Error::other(format!("symlink {rel} in lab output")));
        } else if ty.is_dir() {
            read_tree(&e.path(), &format!("{rel}/"), out)?;
        } else {
            out.insert(rel, std::fs::read(e.path())?);
        }
    }
    Ok(())
}

/// One checked-in capture; every read is verified against the manifest.
pub struct LabCapture {
    pub mode: String,
    dir: PathBuf,
    manifest: Manifest,
}

/// Open the capture of `mode`.
pub fn lab_capture(mode: &str) -> LabCapture {
    check_rel(mode);
    let dir = lab_root().join(mode);
    assert!(dir.is_dir(), "no lab capture for mode '{mode}'");
    LabCapture {
        mode: mode.into(),
        dir,
        manifest: Manifest::load(&fixtures_root().join("MANIFEST.toml")),
    }
}

/// Every captured mode on disk, sorted.
pub fn lab_captures() -> Vec<String> {
    let mut v: Vec<_> = std::fs::read_dir(lab_root())
        .expect("fixtures/captures/lab")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn lab_root() -> PathBuf {
    fixtures_root().join("captures").join(LAB_DIR)
}

impl LabCapture {
    /// Verified bytes of `rel`, e.g. `stderr.txt` or `out/report.json`.
    pub fn file(&self, rel: &str) -> Vec<u8> {
        check_rel(rel);
        let key = format!("captures/{LAB_DIR}/{}/{rel}", self.mode);
        let bytes = std::fs::read(self.dir.join(rel))
            .unwrap_or_else(|e| panic!("lab fixture {key} unreadable: {e}"));
        self.manifest
            .verify(&key, &bytes)
            .unwrap_or_else(|e| panic!("lab fixture {key} failed manifest verification: {e}"));
        bytes
    }

    pub fn file_str(&self, rel: &str) -> String {
        String::from_utf8(self.file(rel)).expect("lab fixture not UTF-8")
    }

    /// The whole capture as a transcript, every file verified.
    pub fn transcript(&self) -> LabTranscript {
        let mut on_disk = BTreeMap::new();
        read_tree(&self.dir.join("out"), "", &mut on_disk)
            .unwrap_or_else(|e| panic!("lab capture {}: {e}", self.mode));
        let out = on_disk
            .into_keys()
            .map(|rel| {
                let bytes = self.file(&format!("out/{rel}"));
                (rel, bytes)
            })
            .collect();
        LabTranscript {
            argv: self
                .file_str("argv.txt")
                .lines()
                .map(String::from)
                .collect(),
            exit_code: self
                .file_str("exit-code.txt")
                .trim()
                .parse()
                .expect("exit-code.txt is an integer"),
            stdout: self.file_str("stdout.txt"),
            stderr: self.file_str("stderr.txt"),
            out,
        }
    }

    /// Decode `out/<rel>`.
    pub fn document(&self, rel: &str) -> Result<Document, DecodeError> {
        Document::decode(rel, &self.file(&format!("out/{rel}")))
    }
}

/// Why `StubLab` cannot replay a run.
#[derive(Debug)]
pub enum StubLabError {
    /// No capture for this mode: the stub never substitutes another.
    NoCapture(String),
    /// A publication-backed output has files but no exact ownership marker.
    OutputNotOwned(PathBuf),
    /// The output path exists and is not a directory.
    OutputNotDirectory(PathBuf),
    /// The output path or one of its ancestors is a symlink.
    OutputSymlink(PathBuf),
    StageNotOwned(PathBuf),
    BackupNotOwned(PathBuf),
    /// A pre-commit owned sibling delete failed. The pinned shared helper
    /// uses this error name for backup deletion too.
    StageCleanupFailed {
        path: PathBuf,
        source: io::Error,
    },
    /// Post-commit deletion of the moved-aside previous output failed. The
    /// pinned rollback is best-effort and the backup may already be partial.
    BackupCleanupFailed {
        path: PathBuf,
        source: io::Error,
    },
    PublishRollbackFailed(PathBuf),
    Io(io::Error),
}

impl std::fmt::Display for StubLabError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCapture(m) => write!(f, "stub lab has no capture for mode {m:?}"),
            Self::OutputNotOwned(p) => write!(
                f,
                "stub lab output {} is not owned by the captured lab",
                p.display()
            ),
            Self::OutputNotDirectory(p) => {
                write!(f, "stub lab output {} is not a directory", p.display())
            }
            Self::OutputSymlink(p) => {
                write!(f, "stub lab output {} is a symlink", p.display())
            }
            Self::StageNotOwned(p) => write!(
                f,
                "stub lab stage {} is not owned by the captured lab",
                p.display()
            ),
            Self::BackupNotOwned(p) => write!(
                f,
                "stub lab backup {} is not owned by the captured lab",
                p.display()
            ),
            Self::StageCleanupFailed { path, source } => write!(
                f,
                "stub lab publication sibling {} cleanup failed: {source}",
                path.display()
            ),
            Self::BackupCleanupFailed { path, source } => write!(
                f,
                "stub lab backup {} cleanup failed: {source}",
                path.display()
            ),
            Self::PublishRollbackFailed(p) => write!(
                f,
                "stub lab publication rollback failed for {}",
                p.display()
            ),
            Self::Io(e) => write!(f, "stub lab: {e}"),
        }
    }
}

impl std::error::Error for StubLabError {}

const OUTPUT_MARKER: &str = ".boris-migration-lab-output";

// Publication-backed modes write a complete owned sibling stage, then
// rename it into place. They may write into an absent or empty output, and
// they may replace a non-empty output only when its marker is byte-identical
// to the capture's owner marker. Post-commit backup-cleanup rollback is
// best-effort and pinned-faithful rather than atomic: once deletion of the
// previous backup starts, `BackupCleanupFailed` can leave a partially
// deleted previous tree in the final output. Modes without a marker write
// directly, as the pinned non-publication modes do.
const STAGE_SUFFIX: &str = ".boris-migration-lab-stage";
const BACKUP_SUFFIX: &str = ".boris-migration-lab-prev";

fn publication_sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut sibling = path.as_os_str().to_os_string();
    sibling.push(suffix);
    PathBuf::from(sibling)
}

fn reject_symlink_along(path: &Path) -> Result<(), StubLabError> {
    for ancestor in path.ancestors() {
        if let Ok(meta) = std::fs::symlink_metadata(ancestor)
            && meta.file_type().is_symlink()
        {
            return Err(StubLabError::OutputSymlink(ancestor.to_path_buf()));
        }
    }
    Ok(())
}

fn metadata_if_present(path: &Path) -> Result<Option<std::fs::Metadata>, StubLabError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => Ok(Some(meta)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(StubLabError::Io(e)),
    }
}

fn path_exists(path: &Path) -> Result<bool, StubLabError> {
    Ok(metadata_if_present(path)?.is_some())
}

fn directory_exists(path: &Path) -> Result<bool, StubLabError> {
    Ok(metadata_if_present(path)?.is_some_and(|meta| meta.is_dir()))
}

fn directory_is_empty(path: &Path) -> Result<bool, StubLabError> {
    Ok(std::fs::read_dir(path)
        .map_err(StubLabError::Io)?
        .next()
        .is_none())
}

fn is_owned_directory(path: &Path, marker: &[u8]) -> bool {
    std::fs::read(path.join(OUTPUT_MARKER))
        .map(|found| found == marker)
        .unwrap_or(false)
}

#[derive(Debug, Clone, Copy)]
enum SiblingKind {
    Stage,
    Backup,
}

impl SiblingKind {
    fn ownership_error(self, path: PathBuf) -> StubLabError {
        match self {
            Self::Stage => StubLabError::StageNotOwned(path),
            Self::Backup => StubLabError::BackupNotOwned(path),
        }
    }
}

fn remove_tree(path: &Path) -> io::Result<()> {
    std::fs::remove_dir_all(path)
}

fn delete_owned_if_present(
    path: &Path,
    marker: &[u8],
    kind: SiblingKind,
) -> Result<(), StubLabError> {
    delete_owned_if_present_with(path, marker, kind, &remove_tree)
}

fn delete_owned_if_present_with(
    path: &Path,
    marker: &[u8],
    kind: SiblingKind,
    remove_tree: &dyn Fn(&Path) -> io::Result<()>,
) -> Result<(), StubLabError> {
    let Some(meta) = metadata_if_present(path)? else {
        return Ok(());
    };
    if !meta.is_dir() || !is_owned_directory(path, marker) {
        return Err(kind.ownership_error(path.to_path_buf()));
    }
    remove_tree(path).map_err(|source| StubLabError::StageCleanupFailed {
        path: path.to_path_buf(),
        source,
    })
}

struct Publication {
    final_path: PathBuf,
    stage_path: PathBuf,
    backup_path: PathBuf,
    marker: Vec<u8>,
    active: bool,
}

impl Publication {
    fn begin(out: &Path, marker: &[u8]) -> Result<Self, StubLabError> {
        let stage_path = publication_sibling(out, STAGE_SUFFIX);
        let backup_path = publication_sibling(out, BACKUP_SUFFIX);
        reject_symlink_along(out)?;
        reject_symlink_along(&stage_path)?;
        reject_symlink_along(&backup_path)?;
        let mut publication = Self {
            final_path: out.to_path_buf(),
            stage_path,
            backup_path,
            marker: marker.to_vec(),
            active: true,
        };
        if let Err(err) = publication.prepare() {
            publication.abandon();
            return Err(err);
        }
        Ok(publication)
    }

    fn prepare(&mut self) -> Result<(), StubLabError> {
        if let Some(meta) = metadata_if_present(&self.final_path)? {
            if !meta.is_dir() {
                return Err(StubLabError::OutputNotDirectory(self.final_path.clone()));
            }
            if !directory_is_empty(&self.final_path)?
                && !is_owned_directory(&self.final_path, &self.marker)
            {
                return Err(StubLabError::OutputNotOwned(self.final_path.clone()));
            }
        }
        delete_owned_if_present(&self.stage_path, &self.marker, SiblingKind::Stage)?;
        std::fs::create_dir_all(&self.stage_path).map_err(StubLabError::Io)?;
        std::fs::write(self.stage_path.join(OUTPUT_MARKER), &self.marker).map_err(StubLabError::Io)
    }

    fn abandon(&mut self) {
        if self.active && is_owned_directory(&self.stage_path, &self.marker) {
            let _ = std::fs::remove_dir_all(&self.stage_path);
        }
        self.active = false;
    }

    fn commit_with_remover(
        &mut self,
        remove_tree: &dyn Fn(&Path) -> io::Result<()>,
    ) -> Result<(), StubLabError> {
        delete_owned_if_present_with(
            &self.backup_path,
            &self.marker,
            SiblingKind::Backup,
            remove_tree,
        )?;
        let mut moved_previous = false;
        if directory_exists(&self.final_path)? {
            std::fs::rename(&self.final_path, &self.backup_path).map_err(StubLabError::Io)?;
            moved_previous = true;
        } else if path_exists(&self.final_path)? {
            return Err(StubLabError::OutputNotDirectory(self.final_path.clone()));
        }

        if let Err(source) = std::fs::rename(&self.stage_path, &self.final_path) {
            if moved_previous && std::fs::rename(&self.backup_path, &self.final_path).is_err() {
                return Err(StubLabError::PublishRollbackFailed(self.final_path.clone()));
            }
            return Err(StubLabError::Io(source));
        }
        self.active = false;

        if moved_previous && let Err(source) = remove_tree(&self.backup_path) {
            if std::fs::rename(&self.final_path, &self.stage_path).is_err() {
                return Err(StubLabError::BackupCleanupFailed {
                    path: self.backup_path.clone(),
                    source,
                });
            }
            if std::fs::rename(&self.backup_path, &self.final_path).is_err() {
                if std::fs::rename(&self.stage_path, &self.final_path).is_err() {
                    return Err(StubLabError::PublishRollbackFailed(self.final_path.clone()));
                }
                return Err(StubLabError::BackupCleanupFailed {
                    path: self.backup_path.clone(),
                    source,
                });
            }
            self.active = true;
            return Err(StubLabError::BackupCleanupFailed {
                path: self.backup_path.clone(),
                source,
            });
        }
        Ok(())
    }
}

fn publish_with_writer(
    out: &Path,
    marker: &[u8],
    write_stage: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<(), StubLabError> {
    publish_with_writer_and_remover(out, marker, write_stage, &remove_tree)
}

fn publish_with_writer_and_remover(
    out: &Path,
    marker: &[u8],
    write_stage: impl FnOnce(&Path) -> io::Result<()>,
    remove_tree: &dyn Fn(&Path) -> io::Result<()>,
) -> Result<(), StubLabError> {
    let mut publication = Publication::begin(out, marker)?;
    let result = match write_stage(&publication.stage_path) {
        Err(e) => Err(StubLabError::Io(e)),
        Ok(()) => publication.commit_with_remover(remove_tree),
    };
    if result.is_err() {
        publication.abandon();
    }
    result
}

/// Replays captured lab runs. The stub answers by mode only; inputs are
/// whatever the capture used (see [`LAB_CAPTURES`]).
#[derive(Debug, Default, Clone, Copy)]
pub struct StubLab;

impl StubLab {
    pub fn new() -> Self {
        Self
    }

    /// The tool id of the lab that produced the captures.
    pub fn tool_id(&self) -> String {
        Manifest::load(&fixtures_root().join("MANIFEST.toml"))
            .lab_captures
            .expect("MANIFEST.toml [lab_captures]")
            .tool_id
    }

    /// Replay `mode`: write its captured `out/` tree into `out`, as the lab
    /// would, and return the transcript.
    pub fn run(&self, mode: &str, out: &Path) -> Result<LabTranscript, StubLabError> {
        if !lab_captures().iter().any(|m| m == mode) {
            return Err(StubLabError::NoCapture(mode.into()));
        }
        let t = lab_capture(mode).transcript();
        if let Some(marker) = t.out.get(OUTPUT_MARKER) {
            publish_with_writer(out, marker, |stage| write_tree(&t.out, stage))?;
        } else {
            write_tree(&t.out, out).map_err(StubLabError::Io)?;
        }
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn publication_scratch(name: &str) -> PathBuf {
        let unique = format!(
            "cv-fixtures-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::canonicalize(root).unwrap()
    }

    #[test]
    fn normalize_replaces_the_root_in_text_only() {
        let root = Path::new("/scratch/abc");
        assert_eq!(
            normalize(b"wrote /scratch/abc/out/x", root),
            b"wrote /CAPTURE_ROOT/out/x".to_vec()
        );
        let png = [0x89, b'P', b'N', b'G', 0xff, b'/', b's'];
        assert_eq!(normalize(&png, root), png.to_vec());
    }

    #[test]
    fn normalize_replaces_markdown_code_span_paths() {
        let root = Path::new("/scratch/abc");
        assert_eq!(
            normalize(b"Source root | `/scratch/abc/src/mini-jekyll` |", root),
            b"Source root | `/CAPTURE_ROOT/src/mini-jekyll` |".to_vec()
        );
        assert_eq!(
            normalize(b"Source vault: `/scratch/abc/src/mini-obsidian`", root),
            b"Source vault: `/CAPTURE_ROOT/src/mini-obsidian`".to_vec()
        );
        assert_eq!(
            normalize(b"Exact root: `/scratch/abc`", root),
            b"Exact root: `/CAPTURE_ROOT`".to_vec()
        );
        assert_eq!(
            normalize(b"Source root | `/scratch/abc2/src/mini-jekyll` |", root),
            b"Source root | `/scratch/abc2/src/mini-jekyll` |".to_vec()
        );
    }

    #[test]
    fn normalize_replaces_windows_drive_spellings() {
        let root = Path::new("\\\\?\\C:\\scratch\\abc");
        let verbatim = root.to_str().unwrap();
        let non_verbatim = "C:\\scratch\\abc";
        for (input, want) in [
            (
                format!("wrote {verbatim}\\out"),
                "wrote /CAPTURE_ROOT\\out".to_string(),
            ),
            (
                format!("wrote {non_verbatim}\\out"),
                "wrote /CAPTURE_ROOT\\out".to_string(),
            ),
            (
                format!("wrote {}/out", verbatim.replace('\\', "/")),
                "wrote /CAPTURE_ROOT/out".to_string(),
            ),
            (
                format!("wrote {}/out", non_verbatim.replace('\\', "/")),
                "wrote /CAPTURE_ROOT/out".to_string(),
            ),
            (
                format!("{verbatim} {non_verbatim}"),
                "/CAPTURE_ROOT /CAPTURE_ROOT".to_string(),
            ),
        ] {
            assert_eq!(normalize(input.as_bytes(), root), want.into_bytes());
        }
    }

    #[test]
    fn normalize_replaces_windows_unc_spellings() {
        let root = Path::new("\\\\?\\UNC\\server\\share\\scratch");
        let verbatim = root.to_str().unwrap();
        let non_verbatim = "\\\\server\\share\\scratch";
        for spelling in [
            verbatim.to_string(),
            non_verbatim.to_string(),
            verbatim.replace('\\', "/"),
            non_verbatim.replace('\\', "/"),
        ] {
            assert_eq!(
                normalize(format!("root={spelling}").as_bytes(), root),
                b"root=/CAPTURE_ROOT".to_vec(),
                "{spelling}"
            );
        }
    }

    #[test]
    fn normalize_replaces_json_escaped_windows_backslashes() {
        let root = Path::new("\\\\?\\C:\\scratch\\abc");
        let verbatim = root.to_str().unwrap().replace('\\', "\\\\");
        let non_verbatim = "C:\\scratch\\abc".replace('\\', "\\\\");
        let input = format!(
            "{{\"source\":\"{verbatim}\",\"other\":\"{non_verbatim}\",\"slash\":\"{}\"}}",
            "C:/scratch/abc"
        );
        assert_eq!(
            normalize(input.as_bytes(), root),
            br#"{"source":"/CAPTURE_ROOT","other":"/CAPTURE_ROOT","slash":"/CAPTURE_ROOT"}"#
                .to_vec()
        );

        let descendant = format!("{{\"source\":\"{verbatim}\\\\out\"}}");
        assert_eq!(
            normalize(descendant.as_bytes(), root),
            br#"{"source":"/CAPTURE_ROOT\\out"}"#.to_vec()
        );
    }

    #[test]
    fn normalize_does_not_replace_sibling_prefixes() {
        let root = Path::new("/scratch/abc");
        for input in [
            b"wrote /scratch/abc2/out".as_slice(),
            b"wrote /scratch/abc.txt".as_slice(),
            b"wrote /tmp/scratch/abc/out".as_slice(),
        ] {
            assert_eq!(normalize(input, root), input.to_vec());
        }
        assert_eq!(
            normalize(b"--out=/scratch/abc/out", root),
            b"--out=/CAPTURE_ROOT/out".to_vec()
        );

        let windows = Path::new("\\\\?\\C:\\scratch\\abc");
        assert_eq!(
            normalize(br#"wrote C:\scratch\abc2\out"#, windows),
            br#"wrote C:\scratch\abc2\out"#.to_vec()
        );
        assert_eq!(
            normalize(br#"{"source":"C:\\scratch\\abc2"}"#, windows),
            br#"{"source":"C:\\scratch\\abc2"}"#.to_vec()
        );
    }

    #[test]
    fn normalize_does_not_treat_unix_backslashes_as_windows_separators() {
        let root = Path::new("/scratch/a\\b");
        assert_eq!(
            normalize(b"wrote /scratch/a\\b/out", root),
            b"wrote /CAPTURE_ROOT/out".to_vec()
        );
        assert_eq!(
            normalize(b"wrote /scratch/a/b/out", root),
            b"wrote /scratch/a/b/out".to_vec()
        );

        let parent = Path::new("/scratch/a");
        assert_eq!(
            normalize(b"wrote /scratch/a\\b/out", parent),
            b"wrote /scratch/a\\b/out".to_vec()
        );
    }

    #[test]
    fn publication_write_failure_preserves_previous_output_and_cleans_stage() {
        let root = publication_scratch("publication-write-failure");
        let out = root.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let marker = b"format=boris-migration-lab-output\nowner=test-owner\nschema_version=1\n";
        std::fs::write(out.join(OUTPUT_MARKER), marker).unwrap();
        std::fs::write(out.join("old.txt"), b"old").unwrap();
        let stage = publication_sibling(&out, STAGE_SUFFIX);
        let backup = publication_sibling(&out, BACKUP_SUFFIX);

        let result = publish_with_writer(&out, marker, |stage| {
            std::fs::write(stage.join("partial.txt"), b"partial")?;
            Err(io::Error::other("injected write failure"))
        });

        assert!(matches!(result, Err(StubLabError::Io(_))));
        assert_eq!(std::fs::read(out.join("old.txt")).unwrap(), b"old");
        assert!(!out.join("partial.txt").exists());
        assert!(!stage.exists());
        assert!(!backup.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn backup_cleanup_failure_can_leave_a_partial_previous_output() {
        let root = publication_scratch("publication-backup-partial");
        let out = root.join("out");
        std::fs::create_dir_all(out.join("protected")).unwrap();
        let marker = b"format=boris-migration-lab-output\nowner=test-owner\nschema_version=1\n";
        std::fs::write(out.join(OUTPUT_MARKER), marker).unwrap();
        std::fs::write(out.join("old.txt"), b"old").unwrap();
        std::fs::write(out.join("protected").join("secret.txt"), b"secret").unwrap();
        let stage = publication_sibling(&out, STAGE_SUFFIX);
        let backup = publication_sibling(&out, BACKUP_SUFFIX);
        let injected_remove = |path: &Path| {
            if path == backup.as_path() {
                std::fs::remove_file(path.join(OUTPUT_MARKER))?;
                std::fs::remove_file(path.join("old.txt"))?;
                return Err(io::Error::other("injected partial backup cleanup failure"));
            }
            remove_tree(path)
        };

        let result = publish_with_writer_and_remover(
            &out,
            marker,
            |stage| std::fs::write(stage.join("new.txt"), b"new"),
            &injected_remove,
        );

        match result {
            Err(StubLabError::BackupCleanupFailed { path, .. }) => assert_eq!(path, backup),
            other => panic!("expected BackupCleanupFailed, got {other:?}"),
        }
        assert!(!out.join(OUTPUT_MARKER).exists());
        assert!(!out.join("old.txt").exists());
        assert!(!out.join("new.txt").exists());
        assert_eq!(
            std::fs::read(out.join("protected").join("secret.txt")).unwrap(),
            b"secret"
        );
        assert!(!stage.exists());
        assert!(!backup.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn preexisting_backup_cleanup_failure_uses_the_pinned_stage_error_name() {
        let root = publication_scratch("publication-backup-preexisting");
        let out = root.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let marker = b"format=boris-migration-lab-output\nowner=test-owner\nschema_version=1\n";
        std::fs::write(out.join(OUTPUT_MARKER), marker).unwrap();
        std::fs::write(out.join("current.txt"), b"current").unwrap();
        let backup = publication_sibling(&out, BACKUP_SUFFIX);
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(backup.join(OUTPUT_MARKER), marker).unwrap();
        std::fs::write(backup.join("stale.txt"), b"stale").unwrap();
        let stage = publication_sibling(&out, STAGE_SUFFIX);
        let injected_remove = |path: &Path| {
            if path == backup.as_path() {
                return Err(io::Error::other(
                    "injected pre-existing backup cleanup failure",
                ));
            }
            remove_tree(path)
        };

        let result = publish_with_writer_and_remover(
            &out,
            marker,
            |stage| std::fs::write(stage.join("new.txt"), b"new"),
            &injected_remove,
        );

        match result {
            Err(StubLabError::StageCleanupFailed { path, .. }) => assert_eq!(path, backup),
            other => panic!("expected StageCleanupFailed, got {other:?}"),
        }
        assert_eq!(std::fs::read(out.join("current.txt")).unwrap(), b"current");
        assert_eq!(std::fs::read(backup.join("stale.txt")).unwrap(), b"stale");
        assert!(!stage.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn capture_specs_are_one_per_mode_and_reference_earlier_rows() {
        for (i, s) in LAB_CAPTURES.iter().enumerate() {
            assert!(
                LAB_CAPTURES[..i].iter().all(|o| o.mode != s.mode),
                "{}",
                s.mode
            );
            for (_, input) in s.inputs {
                if let BorisHtmlOf(m) = input {
                    assert!(LAB_CAPTURES[..i].iter().any(|o| o.mode == *m), "{m}");
                }
            }
        }
    }
}
