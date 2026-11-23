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

/// One page record. Without an explicit `id` the path (sans extension)
/// stands in, matching the `[[guides/x]]` links the corpora use.
pub fn page(path: &str, raw: &str) -> PageRecord {
    let (fm, body) = match raw
        .strip_prefix("---\n")
        .and_then(|r| r.split_once("\n---"))
    {
        Some((fm, rest)) => (fm, rest.trim_start_matches(['-', '\n'])),
        None => ("", raw),
    };
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
