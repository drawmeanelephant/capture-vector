//! Project snapshot from a captured fixture corpus: page sources from
//! `fixtures/corpus-src/<corpus>/` and output records from the captured
//! Boris artifact index. Live workspaces are read at boot and on resync,
//! never per frame.
//!
//! Display fields (id, title, parent, tags, relations) come from the
//! Boris IR graph of the last IR build. Before one covers a page, its
//! frontmatter is read as a marked guess (`FieldSource::Source`); Boris
//! diagnostics stay authoritative for what is valid.

use cv_contracts::graph::{Graph, Node};
use cv_engine::verbs::IR_DIR;
use cv_fixtures::{Manifest, corpus, fixtures_root};
use cv_model::module::{FieldSource, OutputRecord, PageRecord, ProjectSnapshot};
use cv_store::paths::{confined, confined_file, is_link};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
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
    // Same as the workspace index: a malformed one yields no output
    // records rather than a panic.
    let index: Option<Value> = manifest
        .pinned_files()
        .any(|(k, _)| *k == artifacts_key)
        .then(|| c.file(ARTIFACTS))
        .and_then(|raw| serde_json::from_slice(&raw).ok());
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

/// One page record guessed from source, marked `FieldSource::Source`.
/// Without an explicit `id` the path (sans extension) stands in,
/// matching the `[[guides/x]]` links the corpora use.
pub fn page(path: &str, raw: &str) -> PageRecord {
    let (fm, body) = split_frontmatter(raw).unwrap_or(("", raw));
    let mut p = PageRecord {
        path: path.into(),
        id: strip_ext(path).into(),
        body: body.to_string(),
        fields: FieldSource::Source,
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

/// IR-mode builds write `graph.json` under `--out`: the live session's
/// own projection (`IR_DIR`), Boris's default `.boris`, or the HTML dir
/// of a workspace built with `--out <html_dir>`. The freshest *valid*
/// file wins; a malformed or unreadable newer candidate never hides an
/// older good one. On a tie the earlier candidate wins, so the projection
/// the live session keeps fresh is read first.
fn ir_graph(root: &Path, html_dir: &str) -> Option<Graph> {
    let mut candidates = Vec::new();
    for (priority, dir) in [IR_DIR, ".boris", html_dir].into_iter().enumerate() {
        let rel = Path::new(dir).join("graph.json");
        let Some(modified) = confined_file(root, &rel)
            .ok()
            .and_then(|path| fs::symlink_metadata(path).ok())
            .and_then(|meta| meta.modified().ok())
        else {
            continue;
        };
        candidates.push((modified, priority, rel));
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    candidates.into_iter().find_map(|(_, _, rel)| {
        workspace_json(root, &rel, MAX_INDEX_BYTES).and_then(|v| serde_json::from_value(v).ok())
    })
}

/// Replace the frontmatter guess with what Boris reported for every page
/// the graph covers. Pages it does not cover (added since the last IR
/// build) keep their `FieldSource::Source` mark.
fn apply_ir(pages: &mut [PageRecord], graph: &Graph) {
    let by_path: HashMap<&str, &Node> = graph
        .nodes
        .iter()
        .map(|n| (n.source_path.as_str(), n))
        .collect();
    for p in pages {
        let Some(n) = by_path.get(p.path.as_str()) else {
            continue;
        };
        p.id = n.id.clone();
        p.title = n
            .title
            .clone()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| n.id.clone());
        p.parent = n.parent.clone();
        p.tags = n.tags.clone();
        p.relations = graph
            .relations
            .iter()
            .filter(|r| r.from.kind == "page" && r.from.value == n.id)
            .map(|r| (r.kind.clone(), r.to.value.clone()))
            .collect();
        p.fields = FieldSource::Boris;
    }
}

/// Project snapshot from a real workspace: page sources under the
/// content input, output records from the last build's proof pack
/// (`<html_dir>/_boris/proof/artifacts.json`) when one exists, and
/// display fields from the last IR graph (`ir_graph`). Unreadable or oversized files are omitted. The walk checks the tree
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
        if let Some(graph) = ir_graph(&base, html_dir) {
            apply_ir(&mut scan.pages, &graph);
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
    use std::time::SystemTime;

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

    /// Copies a captured editor-lane corpus into a fresh workspace,
    /// with its captured IR graph at `ir_rel` when Boris wrote one.
    fn captured_workspace(name: &str, ir_rel: &str) -> tempfile::TempDir {
        let editor = fixtures_root().join("editor");
        let manifest = Manifest::load(&editor.join("MANIFEST.toml"));
        let temp = tempfile::tempdir().unwrap();
        let src_prefix = format!("corpus-src/{name}/");
        for (key, _) in manifest.pinned_files() {
            if let Some(rel) = key.strip_prefix(&src_prefix) {
                write(temp.path(), rel, fs::read(editor.join(key)).unwrap());
            }
        }
        let key = format!("captures/{name}/graph.json");
        if let Ok(bytes) = fs::read(editor.join(&key)) {
            manifest.verify(&key, &bytes).unwrap();
            write(temp.path(), ir_rel, bytes);
        }
        temp
    }

    fn by_path<'a>(snap: &'a ProjectSnapshot, path: &str) -> &'a PageRecord {
        snap.pages.iter().find(|p| p.path == path).unwrap()
    }

    #[test]
    fn quoted_scalars_display_as_boris_reports_them() {
        let ws = captured_workspace("quoted-scalars", ".boris/graph.json");
        let snap = workspace_snapshot(ws.path(), "dist", None, 1);

        let home = by_path(&snap, "index.md");
        assert_eq!(home.fields, FieldSource::Boris);
        assert_eq!(home.id, "home");
        assert_eq!(home.title, "Mini Site (English)");
        assert_eq!(home.parent, None);
        assert_eq!(home.tags, ["start", "quoted: yes"]);

        let double = by_path(&snap, "guides/double.md");
        assert_eq!(double.fields, FieldSource::Boris);
        assert_eq!(double.id, "guides/double");
        assert_eq!(double.title, "Double: quoted (title)");
        assert_eq!(double.parent.as_deref(), Some("home"));
        assert_eq!(
            double.relations,
            [("relates_to".to_string(), "home".to_string())]
        );
    }

    #[test]
    fn an_ir_graph_beside_the_html_dir_is_read_too() {
        let ws = captured_workspace("quoted-scalars", "dist/graph.json");
        let snap = workspace_snapshot(ws.path(), "dist", None, 1);
        assert_eq!(by_path(&snap, "index.md").title, "Mini Site (English)");
        assert_eq!(by_path(&snap, "index.md").fields, FieldSource::Boris);
    }

    fn set_mtime(root: &Path, rel: &str, at: SystemTime) {
        File::options()
            .write(true)
            .open(root.join(rel))
            .unwrap()
            .set_modified(at)
            .unwrap();
    }

    #[test]
    fn the_live_ir_projection_is_read_first_and_a_newer_build_still_wins() {
        let ws = captured_workspace("quoted-scalars", ".cv/ir/graph.json");
        // A graph from another build that also covers `index.md`.
        let other = cv_fixtures::ir_capture("happy").file("graph.json");
        write(ws.path(), ".boris/graph.json", &other);
        write(ws.path(), "dist/graph.json", &other);
        let at = SystemTime::now() - std::time::Duration::from_secs(60);
        for rel in [".cv/ir/graph.json", ".boris/graph.json", "dist/graph.json"] {
            set_mtime(ws.path(), rel, at);
        }
        let title = |epoch| {
            let snap = workspace_snapshot(ws.path(), "dist", None, epoch);
            by_path(&snap, "index.md").title.clone()
        };
        assert_eq!(title(1), "Mini Site (English)");

        set_mtime(ws.path(), "dist/graph.json", SystemTime::now());
        assert_eq!(title(2), "My Boris Site");
    }

    #[test]
    fn a_malformed_newest_ir_graph_falls_back_to_an_older_valid_graph() {
        let ws = captured_workspace("quoted-scalars", ".boris/graph.json");
        write(ws.path(), ".cv/ir/graph.json", "{\"not\": ");
        let older = SystemTime::now() - std::time::Duration::from_secs(60);
        set_mtime(ws.path(), ".boris/graph.json", older);
        set_mtime(ws.path(), ".cv/ir/graph.json", SystemTime::now());

        let snap = workspace_snapshot(ws.path(), "dist", None, 1);
        assert_eq!(by_path(&snap, "index.md").title, "Mini Site (English)");
        assert_eq!(by_path(&snap, "index.md").fields, FieldSource::Boris);
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_newest_ir_graph_is_not_followed_and_an_older_valid_graph_wins() {
        let ws = captured_workspace("quoted-scalars", ".boris/graph.json");
        let outside = tempfile::tempdir().unwrap();
        write(
            outside.path(),
            "graph.json",
            cv_fixtures::ir_capture("happy").file("graph.json"),
        );
        std::fs::create_dir_all(ws.path().join(".cv/ir")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("graph.json"),
            ws.path().join(".cv/ir/graph.json"),
        )
        .unwrap();
        let older = SystemTime::now() - std::time::Duration::from_secs(60);
        set_mtime(ws.path(), ".boris/graph.json", older);

        let snap = workspace_snapshot(ws.path(), "dist", None, 1);
        assert_eq!(by_path(&snap, "index.md").title, "Mini Site (English)");
        assert_eq!(by_path(&snap, "index.md").fields, FieldSource::Boris);
    }

    #[test]
    fn pages_without_boris_ir_are_marked_as_read_from_source() {
        let ws = captured_workspace("quoted-scalars", ".boris/graph.json");
        write(
            ws.path(),
            "content/guides/new.md",
            "---\ntitle: \"Added after the build\"\n---\n",
        );
        let snap = workspace_snapshot(ws.path(), "dist", None, 1);
        assert_eq!(by_path(&snap, "index.md").fields, FieldSource::Boris);
        assert_eq!(by_path(&snap, "guides/new.md").fields, FieldSource::Source);

        fs::remove_file(ws.path().join(".boris/graph.json")).unwrap();
        let snap = workspace_snapshot(ws.path(), "dist", None, 2);
        assert!(snap.pages.iter().all(|p| p.fields == FieldSource::Source));
    }

    /// Boris rejects single-quoted scalars, so it writes no IR for them and
    /// the pages stay marked as a source guess.
    #[test]
    fn single_quoted_scalars_get_no_boris_ir_and_stay_marked() {
        let captures = fixtures_root().join("editor/captures/single-quoted");
        let stderr = fs::read_to_string(captures.join("ir-stderr.txt")).unwrap();
        assert!(
            stderr.contains("EFRONTMATTER: single.md:2:1: single-quoted values are not supported"),
            "{stderr}"
        );
        assert!(!captures.join("graph.json").exists());

        let ws = captured_workspace("single-quoted", ".boris/graph.json");
        let snap = workspace_snapshot(ws.path(), "dist", None, 1);
        assert_eq!(snap.pages.len(), 2);
        assert!(snap.pages.iter().all(|p| p.fields == FieldSource::Source));
    }

    #[test]
    fn workspace_snapshot_keeps_pages_when_the_artifact_index_is_malformed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "content/index.md", "Hello");
        write(
            root,
            "dist/_boris/proof/artifacts.json",
            "{\"artifacts\": [",
        );

        let snap = workspace_snapshot(root, "dist", None, 1);
        assert_eq!(paths(&snap.pages), ["index.md"]);
        assert!(snap.outputs.is_empty());
        assert!(snap.target.is_none());
        assert!(snap.pages[0].output.is_none());
    }

    #[test]
    fn corpus_snapshot_reads_the_captured_artifact_index() {
        let snap = corpus_snapshot("happy");
        assert_eq!(snap.target.as_deref(), Some("default"));
        assert!(snap.outputs.iter().any(|o| o.path == "index.html"));
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
