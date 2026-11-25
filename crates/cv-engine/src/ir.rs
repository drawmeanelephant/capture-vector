//! Staged publication of Boris IR output. Boris may delete files in its
//! `--out` directory on a failed build, so the live projection never runs
//! against the published `.cv/ir` tree directly: the child writes a
//! CV-owned staging tree, and a successful run publishes regular files
//! with sibling temporary files + atomic renames (`graph.json` last).
//! `graph.json` is the self-contained atomic commit point Pages consume.
//! Auxiliary files are best-effort Boris output. Publication is limited
//! to the closed set of files pinned Boris 0.9.0 generates; unknown user
//! files in an output directory are never deleted or overwritten.

use crate::error::{EngineError, Result};
use crate::verbs::IR_DIR;
use crate::write::retry_sharing;
use cv_core::engine::Verb;
use cv_store::paths::{confined, confirm_confined_file, create_confined_parent, is_link};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, ErrorKind};
use std::path::{Component, Path, PathBuf};
use tempfile::NamedTempFile;

/// CV-owned scratch output the Boris child writes. Never read by Pages.
pub const IR_STAGING_DIR: &str = ".cv/ir.staging";

/// Every regular file pinned Boris 0.9.0 is known to generate for
/// `build --out`. These names are Boris-owned output; everything else in
/// a user's output directory is unrecognized data CV must not touch.
const BORIS_IR_OUTPUT_FILES: [&str; 4] = [
    "build-report.json",
    "completion.json",
    "graph.json",
    "manifest.json",
];

fn is_boris_ir_output(rel: &Path) -> bool {
    let mut parts = rel.components();
    let Some(Component::Normal(name)) = parts.next() else {
        return false;
    };
    parts.next().is_none() && BORIS_IR_OUTPUT_FILES.contains(&name.to_string_lossy().as_ref())
}

fn is_graph(rel: &Path) -> bool {
    rel == Path::new("graph.json")
}

fn is_cv_owned_ir_output(out: &str) -> bool {
    parts(out).is_ok_and(|out| parts(IR_DIR).is_ok_and(|cv| out == cv))
}

fn path_error(e: io::Error) -> EngineError {
    match e.kind() {
        ErrorKind::InvalidInput => EngineError::PathEscape(e.to_string()),
        _ => EngineError::Io(e),
    }
}

fn parts(p: &str) -> Result<Vec<String>> {
    Path::new(p)
        .components()
        .filter(|c| *c != Component::CurDir)
        .map(|c| match c {
            Component::Normal(n) => Ok(n.to_string_lossy().to_lowercase()),
            _ => Err(EngineError::Refused(format!(
                "IR output {p:?} must be a workspace-relative directory"
            ))),
        })
        .collect()
}

fn nested(a: &[String], b: &[String]) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

/// The staging verb for a `Verb::Ir`, or an error when its output would
/// overlap the scratch tree. Staging relies on the scheduler's mutating
/// serialization; direct concurrent engine use is outside the port contract.
pub fn staged_verb(verb: &Verb) -> Result<Option<Verb>> {
    let Verb::Ir { out, input } = verb else {
        return Ok(None);
    };
    let out_parts = parts(out)?;
    let stage_parts = parts(IR_STAGING_DIR)?;
    if out_parts.is_empty() || nested(&out_parts, &stage_parts) {
        return Err(EngineError::Refused(format!(
            "IR output {out:?} overlaps CV staging dir {IR_STAGING_DIR:?}"
        )));
    }
    Ok(Some(Verb::Ir {
        out: IR_STAGING_DIR.into(),
        input: input.clone(),
    }))
}

fn remove_tree(root: &Path, rel: &str) -> io::Result<()> {
    let path = confined(root, Path::new(rel))?;
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_dir() && !is_link(&meta) => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Start every projection from an empty scratch tree. Data in it is CV
/// scratch by construction; the published tree is never removed here.
pub fn prepare(root: &Path) -> Result<()> {
    remove_tree(root, IR_STAGING_DIR).map_err(path_error)
}

/// Best-effort scratch cleanup. The next `prepare` retries it.
pub fn cleanup(root: &Path) {
    let _ = remove_tree(root, IR_STAGING_DIR);
}

fn ordinary_file(meta: &Metadata) -> bool {
    meta.is_file() && !is_link(meta)
}

fn open_unfollowed(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

fn inventory(dir: &Path, base: &Path, out: &mut Vec<(PathBuf, PathBuf)>) -> Result<()> {
    for entry in fs::read_dir(dir).map_err(EngineError::Io)? {
        let path = entry.map_err(EngineError::Io)?.path();
        let meta = fs::symlink_metadata(&path).map_err(EngineError::Io)?;
        if is_link(&meta) || !(meta.is_file() || meta.is_dir()) {
            return Err(EngineError::Refused(format!(
                "IR staging contains a non-regular or linked entry at {path:?}"
            )));
        }
        if meta.is_dir() {
            inventory(&path, base, out)?;
        } else {
            let rel = path
                .strip_prefix(base)
                .map_err(|_| EngineError::Refused("IR staging entry escaped its root".into()))?
                .to_path_buf();
            out.push((path, rel));
        }
    }
    Ok(())
}

fn existing_generated_output(
    root: &Path,
    final_out: &str,
    final_rel: &Path,
) -> Result<Vec<(PathBuf, PathBuf)>> {
    if !is_cv_owned_ir_output(final_out) {
        return Ok(vec![]);
    }
    let mut files = Vec::new();
    for name in BORIS_IR_OUTPUT_FILES {
        let rel = PathBuf::from(name);
        let target = confined(root, &final_rel.join(&rel)).map_err(path_error)?;
        match fs::symlink_metadata(&target) {
            Ok(meta) if ordinary_file(&meta) => files.push((target, rel)),
            Ok(meta) if meta.is_dir() || is_link(&meta) => {
                return Err(EngineError::Refused(format!(
                    "Boris IR output path {name:?} is not a regular file"
                )));
            }
            Ok(_) => {
                return Err(EngineError::Refused(format!(
                    "Boris IR output path {name:?} is not a regular file"
                )));
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(EngineError::Io(e)),
        }
    }
    Ok(files)
}

struct StagedFile {
    rel: PathBuf,
    target: PathBuf,
    tmp: NamedTempFile,
}

fn copy_staged(root: &Path, final_out: &Path, source: &Path, rel: &Path) -> Result<StagedFile> {
    let meta = fs::symlink_metadata(source).map_err(EngineError::Io)?;
    if !ordinary_file(&meta) {
        return Err(EngineError::Refused(format!(
            "IR staging entry is not a regular file: {source:?}"
        )));
    }
    let mut input = open_unfollowed(source).map_err(EngineError::Io)?;
    let opened = input.metadata().map_err(EngineError::Io)?;
    if !ordinary_file(&opened) {
        return Err(EngineError::Refused(format!(
            "IR staging entry changed before open: {source:?}"
        )));
    }
    let target_rel = final_out.join(rel);
    let target = create_confined_parent(root, &target_rel).map_err(path_error)?;
    let parent = target
        .parent()
        .ok_or_else(|| EngineError::Refused("IR publish target has no parent".into()))?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".cv-ir-publish-")
        .tempfile_in(parent)
        .map_err(EngineError::Io)?;
    io::copy(&mut input, &mut tmp).map_err(EngineError::Io)?;
    tmp.as_file().sync_all().map_err(EngineError::Io)?;
    Ok(StagedFile {
        rel: rel.to_path_buf(),
        target,
        tmp,
    })
}

fn persist(tmp: NamedTempFile, target: &Path) -> io::Result<()> {
    let mut tmp = Some(tmp);
    retry_sharing(|| match tmp.take().expect("tmp present").persist(target) {
        Ok(_) => Ok(()),
        Err(e) => {
            let err = io::Error::new(e.error.kind(), e.error.to_string());
            let raw = e.error.raw_os_error();
            tmp = Some(e.file);
            Err(raw.map(io::Error::from_raw_os_error).unwrap_or(err))
        }
    })
}

/// Publish a successful staging tree over `final_out`. Every candidate
/// file is first copied beside its target. Only the closed set of file
/// names pinned Boris owns is published. Generation-cleaning of stale
/// names happens only in CV-owned `.cv/ir`; unknown files in every output
/// directory are user data and are never touched.
/// `graph.json` — the sole consistency contract — is atomically renamed
/// last, so a pre-commit failure preserves the old graph.
pub fn publish(root: &Path, final_out: &str) -> Result<()> {
    publish_impl(root, final_out, persist)
}

fn publish_impl(
    root: &Path,
    final_out: &str,
    mut persist_file: impl FnMut(NamedTempFile, &Path) -> io::Result<()>,
) -> Result<()> {
    let final_rel = Path::new(final_out);
    let stage_rel = Path::new(IR_STAGING_DIR);
    let staging = confined(root, stage_rel).map_err(path_error)?;
    let meta = fs::symlink_metadata(&staging).map_err(EngineError::Io)?;
    if !meta.is_dir() || is_link(&meta) {
        return Err(EngineError::Refused(format!(
            "IR staging root {IR_STAGING_DIR:?} is not an ordinary directory"
        )));
    }
    // Validate the final path before any output is touched.
    confined(root, final_rel).map_err(path_error)?;

    let mut files = Vec::new();
    inventory(&staging, &staging, &mut files)?;
    files.sort_by(|a, b| a.1.cmp(&b.1));
    if !files.iter().any(|(_, rel)| is_graph(rel)) {
        cleanup(root);
        return Err(EngineError::Refused(
            "IR projection exited successfully without graph.json; keeping the last published IR"
                .into(),
        ));
    }
    if let Some((_, rel)) = files.iter().find(|(_, rel)| !is_boris_ir_output(rel)) {
        cleanup(root);
        return Err(EngineError::Refused(format!(
            "IR staging contains unrecognized output {}; keeping the last published IR",
            rel.display()
        )));
    }

    let existing = existing_generated_output(root, final_out, final_rel)?;
    let candidate_names: std::collections::BTreeSet<&Path> =
        files.iter().map(|(_, rel)| rel.as_path()).collect();
    let mut staged = Vec::new();
    for (source, rel) in &files {
        staged.push(copy_staged(root, final_rel, source, rel)?);
    }

    // Generation-clean only stale files whose names pinned Boris owns.
    // Unrecognized files are user data and are neither scanned recursively
    // nor touched. The old graph stays untouched until its atomic replace.
    let may_clean = is_cv_owned_ir_output(final_out);
    for (path, rel) in existing {
        if may_clean && !is_graph(&rel) && !candidate_names.contains(rel.as_path()) {
            fs::remove_file(path).map_err(EngineError::Io)?;
        }
    }
    staged.sort_by_key(|f| is_graph(&f.rel));
    for file in staged {
        let target = confirm_confined_file(root, &final_rel.join(&file.rel)).map_err(path_error)?;
        debug_assert_eq!(target, file.target);
        persist_file(file.tmp, &target).map_err(EngineError::Io)?;
    }
    cleanup(root);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, body: &[u8]) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[test]
    fn a_successful_publish_replaces_graph_and_keeps_auxiliary_output() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), ".cv/ir/graph.json", b"old-graph");
        write(t.path(), ".cv/ir/obsolete.json", b"user data");
        write(t.path(), ".cv/ir/manifest.json", b"stale boris manifest");
        write(t.path(), ".cv/ir.staging/graph.json", b"new-graph");
        write(t.path(), ".cv/ir.staging/completion.json", b"completion");

        publish(t.path(), ".cv/ir").unwrap();
        assert_eq!(
            fs::read(t.path().join(".cv/ir/graph.json")).unwrap(),
            b"new-graph"
        );
        assert_eq!(
            fs::read(t.path().join(".cv/ir/completion.json")).unwrap(),
            b"completion"
        );
        assert_eq!(
            fs::read(t.path().join(".cv/ir/obsolete.json")).unwrap(),
            b"user data"
        );
        assert!(!t.path().join(".cv/ir/manifest.json").exists());
        assert!(!t.path().join(IR_STAGING_DIR).exists());
    }

    #[test]
    fn a_pre_commit_failure_leaves_no_mixed_auxiliary_generation() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), ".cv/ir/graph.json", b"old-graph");
        write(t.path(), ".cv/ir/old-aux.json", b"user data");
        write(t.path(), ".cv/ir/manifest.json", b"stale boris manifest");
        write(t.path(), ".cv/ir.staging/graph.json", b"new-graph");
        write(t.path(), ".cv/ir.staging/build-report.json", b"new-report");
        write(
            t.path(),
            ".cv/ir.staging/completion.json",
            b"new-completion",
        );

        let mut renames = 0;
        assert!(
            publish_impl(t.path(), ".cv/ir", |tmp, target| {
                renames += 1;
                if renames == 2 {
                    return Err(io::Error::other("injected rename failure"));
                }
                persist(tmp, target)
            })
            .is_err()
        );
        assert_eq!(
            fs::read(t.path().join(".cv/ir/graph.json")).unwrap(),
            b"old-graph"
        );
        assert_eq!(
            fs::read(t.path().join(".cv/ir/old-aux.json")).unwrap(),
            b"user data"
        );
        assert!(!t.path().join(".cv/ir/manifest.json").exists());
        assert_eq!(
            fs::read(t.path().join(".cv/ir/build-report.json")).unwrap(),
            b"new-report"
        );
        assert!(!t.path().join(".cv/ir/completion.json").exists());
    }

    #[test]
    fn unknown_user_files_survive_in_a_non_cv_output_directory() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "outdir/keep.txt", b"user data");
        write(t.path(), "outdir/manifest.json", b"pre-existing manifest");
        write(t.path(), "outdir/nested/user.txt", b"nested user data");
        write(t.path(), ".cv/ir.staging/graph.json", b"new-graph");
        write(t.path(), ".cv/ir.staging/completion.json", b"completion");

        publish(t.path(), "outdir").unwrap();
        assert_eq!(
            fs::read(t.path().join("outdir/keep.txt")).unwrap(),
            b"user data"
        );
        assert_eq!(
            fs::read(t.path().join("outdir/nested/user.txt")).unwrap(),
            b"nested user data"
        );
        assert_eq!(
            fs::read(t.path().join("outdir/graph.json")).unwrap(),
            b"new-graph"
        );
    }

    #[test]
    fn unrecognized_staging_output_is_refused_without_replacing_user_files() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "outdir/user-notes.txt", b"user data");
        write(t.path(), "outdir/graph.json", b"old-graph");
        write(t.path(), ".cv/ir.staging/graph.json", b"new-graph");
        write(
            t.path(),
            ".cv/ir.staging/user-notes.txt",
            b"unexpected output",
        );

        let e = publish(t.path(), "outdir").unwrap_err();
        assert!(e.to_string().contains("unrecognized output"));
        assert_eq!(
            fs::read(t.path().join("outdir/user-notes.txt")).unwrap(),
            b"user data"
        );
        assert_eq!(
            fs::read(t.path().join("outdir/graph.json")).unwrap(),
            b"old-graph"
        );
    }

    #[test]
    fn success_without_a_graph_keeps_the_last_published_graph() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), ".cv/ir/graph.json", b"old-graph");
        write(t.path(), ".cv/ir.staging/build-report.json", b"report");

        let e = publish(t.path(), ".cv/ir").unwrap_err();
        assert!(e.to_string().contains("without graph.json"));
        assert_eq!(
            fs::read(t.path().join(".cv/ir/graph.json")).unwrap(),
            b"old-graph"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_staging_graph_is_refused_without_touching_the_published_graph() {
        let t = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "graph.json", b"outside");
        write(t.path(), ".cv/ir/graph.json", b"old-graph");
        fs::create_dir_all(t.path().join(IR_STAGING_DIR)).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("graph.json"),
            t.path().join(IR_STAGING_DIR).join("graph.json"),
        )
        .unwrap();

        assert!(publish(t.path(), ".cv/ir").is_err());
        assert_eq!(
            fs::read(t.path().join(".cv/ir/graph.json")).unwrap(),
            b"old-graph"
        );
    }
}
