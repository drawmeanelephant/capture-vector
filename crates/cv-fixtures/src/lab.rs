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

/// Replace every occurrence of `root` with [`CAPTURE_ROOT`] in UTF-8
/// text. Other bytes (images) pass through unchanged.
pub fn normalize(bytes: &[u8], root: &Path) -> Vec<u8> {
    let root = root.to_str().expect("capture root is UTF-8");
    match std::str::from_utf8(bytes) {
        Ok(s) => s.replace(root, CAPTURE_ROOT).into_bytes(),
        Err(_) => bytes.to_vec(),
    }
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
            Self::Io(e) => write!(f, "stub lab: {e}"),
        }
    }
}

impl std::error::Error for StubLabError {}

const OUTPUT_MARKER: &str = ".boris-migration-lab-output";

/// Publication-backed modes replace a complete owned output tree. They may
/// write into an absent or empty output, and they may replace a non-empty
/// output only when its marker is byte-identical to the capture's owner
/// marker. Anything else is refused before mutation, matching the pinned
/// lab's publication contract. Modes without a marker write directly, as the
/// pinned non-publication modes do.
fn prepare_publication_output(out: &Path, marker: &[u8]) -> Result<(), StubLabError> {
    for path in out.ancestors() {
        if let Ok(meta) = std::fs::symlink_metadata(path)
            && meta.file_type().is_symlink()
        {
            return Err(StubLabError::OutputSymlink(path.to_path_buf()));
        }
    }
    let meta = match std::fs::symlink_metadata(out) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(StubLabError::Io(e)),
    };
    if !meta.is_dir() {
        return Err(StubLabError::OutputNotDirectory(out.to_path_buf()));
    }
    let empty = std::fs::read_dir(out)
        .map_err(StubLabError::Io)?
        .next()
        .is_none();
    if empty {
        return Ok(());
    }
    let owned = std::fs::read(out.join(OUTPUT_MARKER))
        .map(|found| found == marker)
        .unwrap_or(false);
    if !owned {
        return Err(StubLabError::OutputNotOwned(out.to_path_buf()));
    }
    std::fs::remove_dir_all(out).map_err(StubLabError::Io)
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
            prepare_publication_output(out, marker)?;
        }
        write_tree(&t.out, out).map_err(StubLabError::Io)?;
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
