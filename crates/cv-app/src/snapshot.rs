//! Project snapshot from a captured fixture corpus: page sources from
//! `fixtures/corpus-src/<corpus>/` and output records from the captured
//! Boris artifact index. Live workspaces are read at boot and on resync,
//! never per frame.
//!
//! Display fields (id, title, parent, tags, relations) come from the
//! Boris IR graph of the last IR build. Before one covers a page, its
//! frontmatter is read as a marked guess (`FieldSource::Source`); Boris
//! diagnostics stay authoritative for what is valid.

use cv_contracts::graph::{DependencyKind, EndpointType, Graph, Node};
use cv_engine::verbs::IR_DIR;
use cv_fixtures::{Manifest, corpus, fixtures_root};
use cv_model::module::{FieldSource, OutputRecord, PageRecord, ProjectSnapshot};
use cv_store::paths::{confined, confined_file, is_link};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

const ARTIFACTS: &str = "dist/dist/_boris/proof/artifacts.json";
const SOURCE_EXT: &[&str] = &[".md", ".mdx", ".textile", ".cook"];

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
        ir_graph: None,
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

fn scalar(v: &str) -> String {
    v.strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .unwrap_or(v)
        .to_string()
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
///
/// `explicit_id` conformance: bounded to the Boris forms actually probed
/// against the real 0.9.0 engine as oracle (`validate` + duplicate-
/// detection binding probes, see snapshot tests), not an exhaustive
/// grammar proof. For those probed forms the scanner matches Boris
/// exactly: plain, no-space, space-before-colon, and double-quoted values
/// bind identically; indented lines, uppercase keys, single-quoted and
/// empty values are `EFRONTMATTER` rejections in Boris, so any residual
/// scanner divergence on them is unobservable in a workspace that
/// validates. Unprobed exotica (escapes inside quotes, unusual Unicode
/// in keys) remain an honestly unproven margin.
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
        // Boris rejects indented frontmatter lines outright ("no nested
        // mappings"); trailing space before the colon is accepted. Match
        // that exactly: a leading-whitespace line is no key at all.
        if k.starts_with(char::is_whitespace) {
            continue;
        }
        let v = v.trim().to_string();
        match k.trim() {
            "id" => {
                let id = scalar(&v);
                if !id.is_empty() {
                    p.id = id;
                    p.explicit_id = true;
                }
            }
            "title" => p.title = scalar(&v),
            "parent" => p.parent = Some(scalar(&v)),
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
/// file wins; a malformed, unreadable, or undecodable newer candidate —
/// including one claiming an unsupported IR `schemaVersion` — never
/// hides an older good one and never becomes graph authority. On a tie
/// the earlier candidate wins, so the projection the live session keeps
/// fresh is read first.
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
        workspace_json(root, &rel, MAX_INDEX_BYTES)
            .and_then(|v| cv_contracts::graph::decode_graph_value(v).ok())
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
    // Index Boris's emitted rows once instead of scanning the whole
    // relation/dependency arrays once per page (quadratic at the graph scale
    // floor). The filter predicates and ordering are unchanged.
    let mut relations_by_source: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
    for r in &graph.relations {
        if r.from.kind == EndpointType::Page {
            relations_by_source
                .entry(r.from.value.as_str())
                .or_default()
                .push((r.kind.as_str(), r.to.value.as_str()));
        }
    }
    let mut wikilinks_by_source: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in &graph.edges {
        if e.kind == DependencyKind::Reference
            && e.from.kind == EndpointType::Page
            && e.to.kind == EndpointType::Page
        {
            wikilinks_by_source
                .entry(e.from.value.as_str())
                .or_default()
                .push(e.to.value.as_str());
        }
    }
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
        p.relations = relations_by_source
            .get(n.id.as_str())
            .into_iter()
            .flatten()
            .map(|(kind, target)| ((*kind).to_string(), (*target).to_string()))
            .collect();
        p.wikilinks = wikilinks_by_source
            .get(n.id.as_str())
            .into_iter()
            .flatten()
            .map(|target| (*target).to_string())
            .collect();
        p.fields = FieldSource::Boris;
    }
}

/// Attach migration provenance from `.cv/imports/*/adoption.json` to the
/// pages it names. Association is by stable identity — the lab's entity
/// id, which the output pages carry as their explicit Boris `id` — so it
/// survives the file being renamed or moved inside the workspace. A page
/// whose id the user edits away is a different page and drops the link;
/// a different page moved onto an adopted page's old path must never
/// inherit it, so there is deliberately no path fallback. Missing or
/// undecodable sidecars leave pages unmarked; the Migrate mailbox is
/// where that damage is disclosed.
fn apply_imports(root: &Path, pages: &mut [PageRecord]) {
    let Ok(adoptions) = cv_store::imports::scan_adoptions(root) else {
        return;
    };
    let mut by_entity: HashMap<&str, (String, String)> = HashMap::new();
    for a in &adoptions {
        for page in &a.pages {
            let source = page.source.clone().unwrap_or_else(|| {
                a.inputs
                    .first()
                    .map(|i| i.path.clone())
                    .unwrap_or_else(|| a.run_id.clone())
            });
            by_entity.insert(page.entity_id.as_str(), (a.mode.clone(), source));
        }
    }
    // Only an explicit, unambiguous id carries provenance. A path-derived
    // fallback id belongs to the path, not the page: an unrelated file
    // sitting there must never inherit Imported from. Two pages sharing
    // one explicit id make the association ambiguous, so neither is
    // marked — Boris already refuses that workspace with EDUPLICATEID.
    let mut explicit_counts: HashMap<String, usize> = HashMap::new();
    for p in pages.iter().filter(|p| p.explicit_id) {
        *explicit_counts.entry(p.id.clone()).or_insert(0) += 1;
    }
    for p in pages.iter_mut() {
        p.imported_from = if p.explicit_id && explicit_counts.get(&p.id) == Some(&1) {
            by_entity.get(p.id.as_str()).cloned()
        } else {
            None
        };
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
    let mut frozen_graph = None;
    let index = workspace_root(root).and_then(|base| {
        if let Some(dir) = source_root(&base, Path::new(&input_at(&base))) {
            scan.walk(&dir, "", 0);
        }
        if let Some(graph) = ir_graph(&base, html_dir) {
            apply_ir(&mut scan.pages, &graph);
            frozen_graph = Some(Arc::new(graph));
        }
        apply_imports(&base, &mut scan.pages);
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
        // Boris emits page HTML by stable entity id, not by source path:
        // a renamed source with an explicit id keeps its output route.
        let html = format!("{}.html", p.id);
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
        ir_graph: frozen_graph,
        last_publish: None,
        artifact_root: Some(root.join(html_dir).to_string_lossy().into_owned()),
    }
}

#[cfg(test)]
mod tests;
