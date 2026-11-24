//! Project snapshot from a captured fixture corpus: page sources from
//! `fixtures/corpus-src/<corpus>/` and output records from the captured
//! Boris artifact index. Read once at scene/boot time, never per frame.
//!
//! Frontmatter is read for display only (title, parent, tags,
//! relations); Boris diagnostics stay authoritative for what is valid.

use cv_fixtures::{Manifest, corpus, fixtures_root};
use cv_model::module::{OutputRecord, PageRecord, ProjectSnapshot};
use serde_json::Value;

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
/// Bound on page sources read for one snapshot.
const MAX_PAGES: usize = 20_000;

/// The profile's content input: `boris.json`'s `input`, else `content/`
/// when present, else the root.
pub fn content_input(root: &std::path::Path) -> String {
    std::fs::read_to_string(root.join("boris.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v["input"].as_str().map(String::from))
        .unwrap_or_else(|| {
            if root.join("content").is_dir() {
                "content".into()
            } else {
                ".".into()
            }
        })
}

fn walk(dir: &std::path::Path, rel: &str, html_dir: &str, out: &mut Vec<(String, String)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if out.len() >= MAX_PAGES {
            return;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let child = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) && child != html_dir {
                walk(&e.path(), &child, html_dir, out);
            }
        } else if SOURCE_EXT.iter().any(|x| name.ends_with(x)) {
            out.push((child, std::fs::read_to_string(e.path()).unwrap_or_default()));
        }
    }
}

/// Project snapshot from a real workspace: page sources under the
/// content input, output records from the last build's proof pack
/// (`<html_dir>/_boris/proof/artifacts.json`) when one exists.
pub fn workspace_snapshot(
    root: &std::path::Path,
    html_dir: &str,
    compiler: Option<String>,
    epoch: u64,
) -> ProjectSnapshot {
    let input = content_input(root);
    let mut sources = Vec::new();
    let base = if input == "." {
        root.to_path_buf()
    } else {
        root.join(&input)
    };
    walk(&base, "", html_dir, &mut sources);
    let index: Option<Value> =
        std::fs::read_to_string(root.join(html_dir).join("_boris/proof/artifacts.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok());
    let outputs: Vec<OutputRecord> = index
        .as_ref()
        .and_then(|v| v["artifacts"].as_array())
        .into_iter()
        .flatten()
        .map(output)
        .collect();
    let mut pages: Vec<PageRecord> = sources.iter().map(|(rel, raw)| page(rel, raw)).collect();
    for p in &mut pages {
        let html = format!("{}.html", strip_ext(&p.path));
        p.output = outputs.iter().any(|o| o.path == html).then_some(html);
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
}
