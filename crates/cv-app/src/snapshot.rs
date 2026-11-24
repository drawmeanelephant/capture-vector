//! Project snapshot from a captured fixture corpus: page sources from
//! `fixtures/corpus-src/<corpus>/` and output records from the captured
//! Boris artifact index. Live workspaces are read at boot and on resync,
//! never per frame.
//!
//! Frontmatter is read for display only (title, parent, tags,
//! relations); Boris diagnostics stay authoritative for what is valid.

use cv_fixtures::{Manifest, corpus, fixtures_root};
use cv_model::module::{OutputRecord, PageRecord, ProjectSnapshot};
use cv_store::paths::{confined, confined_file, is_link};
use serde_json::Value;
use std::collections::HashSet;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

const ARTIFACTS: &str = "dist/dist/_boris/proof/artifacts.json";
const SOURCE_EXT: &[&str] = &[".md", ".textile", ".cook"];

pub fn corpus_snapshot(name: &str) -> ProjectSnapshot {
    let manifest = Manifest::load(&fixtures_root().join("MANIFEST.toml"));
    let c = corpus(name);
    let src_prefix = format!("corpus-src/{name}/");
    let mut pages: Vec<PageRecord> = manifest
        .pinned_files()
        .filter_map(|(k, _)| k.strip_prefix(&src_prefix))
        .filter(|rel| !rel.starts_with("themes/") && SOURCE_EXT.iter().any(|e| rel.ends_with(e)))
        .map(|rel| {
            let raw = std::fs::read_to_string(c.corpus_src(rel)).unwrap_or_default();
            page(rel.strip_prefix("content/").unwrap_or(rel), &raw)
        })
        .collect();
    let artifacts_key = format!("captures/{name}/{ARTIFACTS}");
    let index: Option<Value> = manifest
        .pinned_files()
        .any(|(k, _)| *k == artifacts_key)
        .then(|| serde_json::from_str(&c.file_str(ARTIFACTS)).expect("artifacts.json"));
    let mut outputs: Vec<OutputRecord> = index
        .as_ref()
        .and_then(|v| v["artifacts"].as_array())
        .into_iter()
        .flatten()
        .map(output)
        .collect();
    // The proof pack describes the index rather than appearing in it.
    let dist_prefix = format!("captures/{name}/dist/dist/");
    for (k, sha) in manifest.pinned_files() {
        let Some(rel) = k.strip_prefix(&dist_prefix) else {
            continue;
        };
        if outputs.iter().any(|o| o.path == rel) {
            continue;
        }
        outputs.push(OutputRecord {
            path: rel.into(),
            kind: "proof-pack".into(),
            producer: "proof".into(),
            status: "committed".into(),
            required: true,
            bytes: c.file(&format!("dist/dist/{rel}")).len() as u64,
            sha256: sha.clone(),
            format_version: None,
        });
    }
    for p in &mut pages {
        let html = format!("{}.html", strip_ext(&p.path));
        p.output = outputs.iter().any(|o| o.path == html).then_some(html);
    }
    ProjectSnapshot {
        epoch: 1,
        compiler: serde_json::from_str::<Value>(&manifest.engine_version)
            .ok()
            .and_then(|v| v["version"].as_str().map(String::from)),
        target: index
            .as_ref()
            .and_then(|v| v["target"].as_str())
            .map(String::from),
        pages,
        outputs,
        last_publish: None,
        artifact_root: Some(fixtures_root().join(format!("captures/{name}/dist/dist")))
            .filter(|d| d.is_dir())
            .map(|d| d.to_string_lossy().into_owned()),
    }
}

fn strip_ext(p: &str) -> &str {
    p.rsplit_once('.').map_or(p, |(a, _)| a)
}

fn output(v: &Value) -> OutputRecord {
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    OutputRecord {
        path: s("path"),
        kind: s("kind"),
        producer: s("producer"),
        status: s("status"),
        required: v["required"].as_bool().unwrap_or(false),
        bytes: v["bytes"].as_u64().unwrap_or(0),
        sha256: s("sha256"),
        format_version: v["format_version"].as_str().map(String::from),
    }
}

fn list(v: &str) -> Vec<String> {
    v.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Same fence rules as `frontmatter::read`: the first line must be a
/// `---` fence (LF or CRLF) and the body is everything after the closing
/// fence line, verbatim. An unclosed opening fence is no frontmatter.
fn split_frontmatter(raw: &str) -> Option<(&str, &str)> {
    fn is_fence(line: &str) -> bool {
        line.strip_suffix('\r').unwrap_or(line) == "---"
    }
    let mut lines = raw.split_inclusive('\n');
    let first = lines.next()?;
    if !is_fence(first.trim_end_matches('\n')) {
        return None;
    }
    let mut at = first.len();
    for l in lines {
        let start = at;
        at += l.len();
        if is_fence(l.trim_end_matches('\n')) {
            return Some((&raw[first.len()..start], &raw[at..]));
        }
    }
    None
}

/// One page record. Without an explicit `id` the path (sans extension)
/// stands in, matching the `[[guides/x]]` links the corpora use.
pub fn page(path: &str, raw: &str) -> PageRecord {
    let (fm, body) = split_frontmatter(raw).unwrap_or(("", raw));
    let mut p = PageRecord {
        path: path.into(),
        id: strip_ext(path).into(),
        body: body.to_string(),
        ..Default::default()
    };
    for (k, v) in fm.lines().filter_map(|l| l.split_once(':')) {
        let v = v.trim().to_string();
        match k.trim() {
            "id" => p.id = v.clone(),
            "title" => p.title = v.clone(),
            "parent" => p.parent = Some(v.clone()),
            "tags" => p.tags = list(&v),
            "relations" => {
                p.relations = list(&v)
                    .into_iter()
                    .filter_map(|r| r.split_once('=').map(|(a, b)| (a.into(), b.into())))
                    .collect()
            }
            _ => {}
        }
        p.frontmatter.push((k.trim().into(), v));
    }
    if p.title.is_empty() {
        p.title = p.id.clone();
    }
    p
}

/// Directories never walked for page sources.
const SKIP_DIRS: &[&str] = &["themes", "node_modules", "target"];

const MAX_PAGES: usize = 20_000;
const MAX_SOURCE_BYTES: u64 = 2 << 20;
const MAX_TOTAL_SOURCE_BYTES: u64 = 64 << 20;
/// Number of directory levels below the selected content root.
const MAX_DEPTH: usize = 32;
/// Includes ignored names and read_dir errors, not just accepted pages.
const MAX_ENTRIES: usize = 100_000;
const MAX_PROFILE_BYTES: u64 = 1 << 20;
const MAX_INDEX_BYTES: u64 = 16 << 20;

fn bounded_text(reader: impl Read, max_bytes: u64, bytes_left: &mut u64) -> Option<String> {
    let limit = max_bytes.min(*bytes_left);
    let mut bytes = Vec::new();
    // Check actual bytes, not just the size reported before opening. One
    // extra byte distinguishes an exact fit from truncated text.
    let result = reader.take(limit + 1).read_to_end(&mut bytes);
    let len = bytes.len() as u64;
    *bytes_left = bytes_left.saturating_sub(len);
    if result.is_err() || len > limit {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Opens without following a final link and without blocking, so an
/// entry replaced by a link, FIFO, or device after its type check is
/// refused here or by the caller's check on the opened handle.
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

fn regular_text(path: &Path, meta: &Metadata, limit: u64, bytes_left: &mut u64) -> Option<String> {
    if !meta.is_file() || is_link(meta) || meta.len() > limit.min(*bytes_left) {
        return None;
    }
    let file = open_unfollowed(path).ok()?;
    let opened = file.metadata().ok()?;
    if !opened.is_file() || is_link(&opened) {
        return None;
    }
    bounded_text(file, limit, bytes_left)
}

fn workspace_json(root: &Path, relative: &Path, mut limit: u64) -> Option<Value> {
    let path = confined_file(root, relative).ok()?;
    let meta = fs::symlink_metadata(&path).ok()?;
    let raw = regular_text(&path, &meta, limit, &mut limit)?;
    serde_json::from_str(&raw).ok()
}

fn ordinary_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !is_link(&m))
}

fn workspace_root(root: &Path) -> Option<PathBuf> {
    // Inspect the root itself even if the path ends in a separator or `/.`.
    let root: PathBuf = root.components().collect();
    ordinary_dir(&root).then_some(root)
}

/// The profile's content input: `boris.json`'s `input`, else `content/`
/// when present, else the root.
pub fn content_input(root: &Path) -> String {
    workspace_root(root).map_or_else(|| ".".into(), |dir| input_at(&dir))
}

fn input_at(root: &Path) -> String {
    workspace_json(root, Path::new("boris.json"), MAX_PROFILE_BYTES)
        .and_then(|v| v["input"].as_str().map(String::from))
        .unwrap_or_else(|| {
            // Still select a linked `content` so the snapshot can reject
            // it rather than silently scanning the rest of the workspace.
            if fs::symlink_metadata(root.join("content")).is_ok_and(|m| m.is_dir() || is_link(&m)) {
                "content".into()
            } else {
                ".".into()
            }
        })
}

fn source_root(root: &Path, input: &Path) -> Option<PathBuf> {
    if input.components().all(|c| c == Component::CurDir) {
        return Some(root.to_path_buf());
    }
    let dir = confined(root, input).ok()?;
    ordinary_dir(&dir).then_some(dir)
}

struct Scan<'a> {
    html_dir: &'a str,
    pages: Vec<PageRecord>,
    max_pages: usize,
    max_source_bytes: u64,
    max_depth: usize,
    bytes_left: u64,
    entries_left: usize,
}

impl<'a> Scan<'a> {
    fn new(html_dir: &'a str) -> Self {
        Self {
            html_dir,
            pages: Vec::new(),
            max_pages: MAX_PAGES,
            max_source_bytes: MAX_SOURCE_BYTES,
            max_depth: MAX_DEPTH,
            bytes_left: MAX_TOTAL_SOURCE_BYTES,
            entries_left: MAX_ENTRIES,
        }
    }

    fn complete(&self) -> bool {
        self.pages.len() >= self.max_pages || self.bytes_left == 0
    }

    fn walk(&mut self, dir: &Path, rel: &str, depth: usize) {
        if self.complete() || self.entries_left == 0 {
            return;
        }
        let Ok(rd) = fs::read_dir(dir) else {
            return;
        };
        let mut entries = Vec::new();
        for entry in rd.take(self.entries_left) {
            self.entries_left -= 1;
            if let Ok(entry) = entry {
                entries.push(entry.file_name());
            }
        }
        entries.sort_unstable();
        for entry in entries {
            if self.complete() {
                return;
            }
            let name = entry.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            let path = dir.join(&entry);
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            // On Windows, is_link also rejects junctions and other
            // reparse points which can report themselves as directories.
            if is_link(&meta) {
                continue;
            }
            let child = if rel.is_empty() {
                name.to_string()
            } else {
                format!("{rel}/{name}")
            };
            if meta.is_dir() {
                if depth < self.max_depth
                    && !SKIP_DIRS.contains(&name.as_ref())
                    && child != self.html_dir
                {
                    self.walk(&path, &child, depth + 1);
                }
            } else if SOURCE_EXT.iter().any(|ext| name.ends_with(ext))
                && let Some(raw) =
                    regular_text(&path, &meta, self.max_source_bytes, &mut self.bytes_left)
            {
                self.pages.push(page(&child, &raw));
            }
        }
    }
}

/// Project snapshot from a real workspace: page sources under the
/// content input, output records from the last build's proof pack
/// (`<html_dir>/_boris/proof/artifacts.json`) when one exists.
/// Unreadable or oversized files are omitted. The walk checks the tree
/// as found; directory checks are not atomic against concurrent
/// replacement, but a page open refuses a link, FIFO, or device swapped
/// in after its type check.
pub fn workspace_snapshot(
    root: &Path,
    html_dir: &str,
    compiler: Option<String>,
    epoch: u64,
) -> ProjectSnapshot {
    let mut scan = Scan::new(html_dir);
    let index = workspace_root(root).and_then(|base| {
        if let Some(dir) = source_root(&base, Path::new(&input_at(&base))) {
            scan.walk(&dir, "", 0);
        }
        workspace_json(
            &base,
            &Path::new(html_dir).join("_boris/proof/artifacts.json"),
            MAX_INDEX_BYTES,
        )
    });
    let outputs: Vec<OutputRecord> = index
        .as_ref()
        .and_then(|v| v["artifacts"].as_array())
        .into_iter()
        .flatten()
        .map(output)
        .collect();
    let paths: HashSet<&str> = outputs.iter().map(|o| o.path.as_str()).collect();
    let mut pages = scan.pages;
    for p in &mut pages {
        let html = format!("{}.html", strip_ext(&p.path));
        p.output = paths.contains(html.as_str()).then_some(html);
    }
    ProjectSnapshot {
        epoch,
        compiler,
        target: index
            .as_ref()
            .and_then(|v| v["target"].as_str())
            .map(String::from),
        pages,
        outputs,
        last_publish: None,
        artifact_root: Some(root.join(html_dir).to_string_lossy().into_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_frontmatter_still_parses_fields() {
        let p = page("a.md", "---\r\nid: x\r\ntitle: T\r\n---\r\nbody\r\n");
        assert_eq!(p.id, "x");
        assert_eq!(p.title, "T");
        assert_eq!(p.body, "body\r\n");
    }

    #[test]
    fn dash_leading_body_is_kept_verbatim() {
        let p = page("a.md", "---\ntitle: T\n---\n- one\n- two\n---\n");
        assert_eq!(p.body, "- one\n- two\n---\n");
    }

    #[test]
    fn unclosed_fence_is_no_frontmatter() {
        let raw = "---\ntitle: T\nbody without a closing fence\n";
        let p = page("a.md", raw);
        assert_eq!(p.body, raw);
        assert_eq!(p.id, "a");
    }

    fn write(root: &Path, rel: &str, body: impl AsRef<[u8]>) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn paths(pages: &[PageRecord]) -> Vec<&str> {
        pages.iter().map(|p| p.path.as_str()).collect()
    }

    #[test]
    fn workspace_snapshot_uses_default_content_and_boris_outputs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "content/index.md", "---\ntitle: Home\n---\nHello");
        write(root, "content/guides/recipe.cook", "Cook");
        write(root, "content/guides/format.textile", "Textile");
        write(root, "content/readme.txt", "Not a page");
        write(root, "outside-default-input.md", "Not a content page");
        write(
            root,
            "dist/_boris/proof/artifacts.json",
            corpus("happy").file(ARTIFACTS),
        );

        let snap = workspace_snapshot(root, "dist", Some("boris".into()), 7);
        assert_eq!(
            paths(&snap.pages),
            ["guides/format.textile", "guides/recipe.cook", "index.md"]
        );
        assert_eq!((snap.epoch, snap.compiler.as_deref()), (7, Some("boris")));
        assert_eq!(snap.target.as_deref(), Some("default"));
        assert_eq!(snap.pages[2].title, "Home");
        assert_eq!(snap.pages[2].body, "Hello");
        assert_eq!(snap.pages[2].output.as_deref(), Some("index.html"));
    }

    #[test]
    fn workspace_snapshot_respects_a_nested_profile_input() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "boris.json", r#"{"input":"site/posts/."}"#);
        write(root, "site/posts/guide.md", "Guide");
        write(root, "content/default.md", "Not the configured input");

        let snap = workspace_snapshot(root, "dist", None, 1);
        assert_eq!(paths(&snap.pages), ["guide.md"]);
    }

    #[test]
    fn scan_preserves_skips_and_includes_pages_at_the_depth_limit() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for path in [
            ".hidden/no.md",
            "dist/no.md",
            "themes/no.md",
            "node_modules/no.md",
            "target/no.md",
            "folder.md/within.md",
            "folder.md/deeper/no.md",
            "root.md",
        ] {
            write(root, path, "Body");
        }

        let mut scan = Scan::new("dist");
        scan.max_depth = 1;
        scan.walk(root, "", 0);
        assert_eq!(paths(&scan.pages), ["folder.md/within.md", "root.md"]);
    }

    #[test]
    fn scan_omits_oversized_and_invalid_pages_without_truncating_valid_pages() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "a-oversized.md", "12345");
        write(root, "b-invalid.md", [0xff]);
        write(root, "c-exact.md", "1234");
        write(root, "d-empty.md", "");

        let mut scan = Scan::new("dist");
        scan.max_source_bytes = 4;
        scan.max_pages = 2;
        scan.walk(root, "", 0);
        assert_eq!(paths(&scan.pages), ["c-exact.md", "d-empty.md"]);
        assert_eq!(scan.pages[0].body, "1234");
        assert_eq!(scan.bytes_left, MAX_TOTAL_SOURCE_BYTES - 5);
    }

    #[test]
    fn scan_never_accepts_more_than_the_remaining_source_budget() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "a.md", "1234");
        write(root, "b-cannot-fit.md", "1234");
        write(root, "c-fits.md", "123");
        write(root, "d-after-limit.md", "1");

        let mut scan = Scan::new("dist");
        scan.bytes_left = 7;
        scan.walk(root, "", 0);
        assert_eq!(paths(&scan.pages), ["a.md", "c-fits.md"]);
        assert_eq!(scan.bytes_left, 0);
    }

    #[test]
    fn bounded_reader_checks_actual_length_with_one_extra_byte() {
        let mut remaining = 8;
        let mut input = &b"12345678"[..];
        assert!(bounded_text(&mut input, 4, &mut remaining).is_none());
        assert_eq!(remaining, 3);
        assert_eq!(input, b"678");
        assert_eq!(
            bounded_text(input, 4, &mut remaining).as_deref(),
            Some("678")
        );
        assert_eq!(remaining, 0);
    }

    #[test]
    fn scan_limits_pages_in_sort_order() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for path in ["c.md", "a.md", "b.md"] {
            write(root, path, "Body");
        }
        let mut scan = Scan::new("dist");
        scan.max_pages = 2;
        scan.walk(root, "", 0);
        assert_eq!(paths(&scan.pages), ["a.md", "b.md"]);
    }

    #[test]
    fn scan_limits_entries_before_sorting() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for path in ["a.md", "b.md", "c.md"] {
            write(root, path, "Body");
        }
        let mut scan = Scan::new("dist");
        scan.entries_left = 2;
        scan.walk(root, "", 0);
        assert_eq!(scan.entries_left, 0);
        assert_eq!(scan.pages.len(), 2);
        assert!(scan.pages[0].path < scan.pages[1].path);
    }

    #[cfg(unix)]
    #[test]
    fn workspace_snapshot_skips_linked_files_and_directories() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "content/real/one.md", "Inert page");
        symlink("real/one.md", root.join("content/copied.md")).unwrap();
        symlink("real", root.join("content/copied-dir")).unwrap();

        let snap = workspace_snapshot(root, "dist", None, 1);
        assert_eq!(paths(&snap.pages), ["real/one.md"]);
    }

    #[cfg(unix)]
    #[test]
    fn workspace_snapshot_refuses_a_link_in_the_content_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "actual/nested/one.md", "Inert page");
        std::os::unix::fs::symlink("actual", root.join("content")).unwrap();

        let implicit = workspace_snapshot(root, "dist", None, 1);
        assert!(implicit.pages.is_empty());
        write(root, "boris.json", r#"{"input":"content/nested"}"#);
        let explicit = workspace_snapshot(root, "dist", None, 2);
        assert!(explicit.pages.is_empty());
    }

    #[test]
    fn scan_skips_a_sparse_page_over_the_size_cap_without_reading_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        File::create(root.join("a-huge.md"))
            .unwrap()
            .set_len(MAX_SOURCE_BYTES + 1)
            .unwrap();
        write(root, "b-page.md", "Body");

        let mut scan = Scan::new("dist");
        scan.walk(root, "", 0);
        assert_eq!(paths(&scan.pages), ["b-page.md"]);
        assert_eq!(scan.bytes_left, MAX_TOTAL_SOURCE_BYTES - 4);
    }

    #[test]
    fn scan_stops_descending_past_the_default_depth_cap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let deepest = vec!["d"; MAX_DEPTH].join("/");
        write(root, &format!("{deepest}/in.md"), "Body");
        write(root, &format!("{deepest}/d/out.md"), "Body");
        write(root, "top.md", "Body");

        let mut scan = Scan::new("dist");
        scan.walk(root, "", 0);
        assert_eq!(
            paths(&scan.pages),
            [format!("{deepest}/in.md").as_str(), "top.md"]
        );
    }

    /// A regression that blocks in `open` fails here instead of hanging
    /// the test run; the stuck thread is abandoned.
    fn before_deadline<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("reading the workspace blocked")
    }

    #[cfg(unix)]
    fn mkfifo(path: &Path) {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo");
    }

    /// File links only: a Windows symlink and a Unix symlink both carry
    /// the reparse bit `is_link` checks. std has no cross-platform link
    /// API, so the tests share one helper. FIFOs and devices stay
    /// unix-only — Windows has no equivalent a page open could hit.
    #[cfg(unix)]
    fn link_file(target: &Path, link: &Path) -> io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn link_file(target: &Path, link: &Path) -> io::Result<()> {
        std::os::windows::fs::symlink_file(target, link)
    }

    fn scan_pages(root: &Path) -> Vec<String> {
        let root = root.to_path_buf();
        before_deadline(move || {
            let mut scan = Scan::new("dist");
            scan.walk(&root, "", 0);
            scan.pages.into_iter().map(|p| p.path).collect()
        })
    }

    #[cfg(unix)]
    #[test]
    fn scan_skips_a_fifo_named_like_a_page_without_blocking() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        mkfifo(&root.join("a-pipe.md"));
        write(root, "b-page.md", "Body");

        assert_eq!(scan_pages(root), ["b-page.md"]);
    }

    #[cfg(unix)]
    #[test]
    fn scan_skips_a_link_to_a_device_named_like_a_page() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::os::unix::fs::symlink("/dev/zero", root.join("a-zero.md")).unwrap();
        write(root, "b-page.md", "Body");

        assert_eq!(scan_pages(root), ["b-page.md"]);
    }

    #[test]
    fn scan_skips_a_link_to_a_page_outside_the_root() {
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "secret.md", "Outside");
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        link_file(&outside.path().join("secret.md"), &root.join("a-link.md")).unwrap();
        write(root, "b-page.md", "Body");

        assert_eq!(scan_pages(root), ["b-page.md"]);
    }

    /// Stands in for an entry replaced between the type check and the open.
    fn read_swapped(swapped_in: PathBuf, checked: Metadata) -> Option<String> {
        before_deadline(move || {
            let mut left = MAX_TOTAL_SOURCE_BYTES;
            regular_text(&swapped_in, &checked, MAX_SOURCE_BYTES, &mut left)
        })
    }

    #[cfg(unix)]
    #[test]
    fn regular_text_does_not_block_on_a_fifo_swapped_in_after_the_type_check() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "page.md", "Body");
        mkfifo(&root.join("pipe.md"));
        let checked = fs::symlink_metadata(root.join("page.md")).unwrap();

        assert_eq!(read_swapped(root.join("pipe.md"), checked), None);
    }

    #[test]
    fn regular_text_does_not_follow_a_link_swapped_in_after_the_type_check() {
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "secret.md", "Outside");
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "page.md", "Body");
        link_file(&outside.path().join("secret.md"), &root.join("link.md")).unwrap();
        let checked = fs::symlink_metadata(root.join("page.md")).unwrap();

        assert_eq!(read_swapped(root.join("link.md"), checked.clone()), None);
        assert_eq!(
            read_swapped(root.join("page.md"), checked).as_deref(),
            Some("Body")
        );
    }
}
