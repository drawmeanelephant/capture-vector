//! In-memory artifact models. Callers (the engine lane) read files and hand
//! bytes in; everything here is pure parsing over borrowed bytes. The
//! `cv_core::rules::ArtifactSet` manifest (paths + sha256) is derived from
//! the same bytes, so the report's `artifact_set_hash` pins exactly what
//! was evaluated.

use crate::html::HtmlDoc;
use crate::{Error, sha256};
use cv_core::id::AccountId;
use cv_core::rules::{
    Archetype, ArtifactKind, ArtifactRef, ArtifactSet as Manifest, TargetDeployProfile,
};
use cv_core::sys::Hash;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// One IR graph node (`graph.json` `nodes[]`). `raw` keeps every field so
/// `IrNode` selectors can test any of them without re-deriving semantics.
#[derive(Debug, Clone, PartialEq)]
pub struct IrNode {
    pub id: String,
    pub role: String,
    pub status: String,
    pub raw: Value,
}

impl IrNode {
    pub fn new(id: &str, role: &str, status: &str) -> Self {
        Self {
            id: id.into(),
            role: role.into(),
            status: status.into(),
            raw: json!({ "id": id, "role": role, "status": status }),
        }
    }

    pub fn is_draft(&self) -> bool {
        self.status == "draft"
    }

    /// `field` is `key` (present, non-null) or `key=value` (string/bool/number equality).
    pub fn field_matches(&self, field: Option<&str>) -> bool {
        let Some(f) = field else { return true };
        let (key, want) = match f.split_once('=') {
            Some((k, v)) => (k.trim(), Some(v.trim())),
            None => (f.trim(), None),
        };
        match (self.raw.get(key), want) {
            (None | Some(Value::Null), _) => false,
            (Some(_), None) => true,
            (Some(Value::String(s)), Some(w)) => s == w,
            (Some(v), Some(w)) => {
                let rendered = v.to_string();
                rendered == w
            }
        }
    }

    pub fn kind_matches(&self, kind: &str) -> bool {
        kind == "*" || kind == self.role
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrGraph {
    pub path: String,
    pub nodes: Vec<IrNode>,
    pub raw: Value,
    pub sha256: Hash,
}

impl IrGraph {
    pub fn from_json(path: &str, bytes: &[u8]) -> Result<Self, Error> {
        let raw: Value = serde_json::from_slice(bytes).map_err(|e| Error::Parse {
            artifact: path.into(),
            msg: e.to_string(),
        })?;
        let nodes = raw
            .get("nodes")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Parse {
                artifact: path.into(),
                msg: "missing `nodes` array".into(),
            })?
            .iter()
            .map(|n| {
                let s = |k: &str| {
                    n.get(k)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                IrNode {
                    id: s("id"),
                    role: s("role"),
                    status: if s("status").is_empty() {
                        "published".into()
                    } else {
                        s("status")
                    },
                    raw: n.clone(),
                }
            })
            .collect();
        Ok(Self {
            path: path.into(),
            nodes,
            raw,
            sha256: sha256(bytes),
        })
    }

    /// Build from already-decoded nodes (the engine lane may hold the IR
    /// in memory); the hash covers the canonical JSON of the nodes.
    pub fn from_nodes(path: &str, nodes: Vec<IrNode>) -> Self {
        let raw = json!({ "nodes": nodes.iter().map(|n| n.raw.clone()).collect::<Vec<_>>() });
        let sha256 = sha256(raw.to_string().as_bytes());
        Self {
            path: path.into(),
            nodes,
            raw,
            sha256,
        }
    }

    pub fn node(&self, id: &str) -> Option<&IrNode> {
        self.nodes.iter().find(|n| n.id == id)
    }
}

/// One emitted page of final HTML, keyed by its output route
/// (target-relative, e.g. `guides/intro.html`).
#[derive(Debug, Clone, PartialEq)]
pub struct HtmlPage {
    pub entity_id: Option<String>,
    pub route: String,
    pub doc: HtmlDoc,
    /// Parsed `<script type="application/ld+json">` blocks; `None` = invalid JSON.
    pub jsonld: Vec<Option<Value>>,
    pub sha256: Hash,
}

impl HtmlPage {
    pub fn parse(entity_id: Option<&str>, route: &str, bytes: &[u8]) -> Result<Self, Error> {
        let src = std::str::from_utf8(bytes).map_err(|e| Error::Parse {
            artifact: route.into(),
            msg: e.to_string(),
        })?;
        let doc = HtmlDoc::parse(src);
        let jsonld = doc
            .elements
            .iter()
            .filter(|e| {
                e.name == "script"
                    && e.attr("type")
                        .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/ld+json"))
            })
            .map(|e| serde_json::from_str(&e.text).ok())
            .collect();
        Ok(Self {
            entity_id: entity_id.map(str::to_string),
            route: route.trim_start_matches('/').to_string(),
            doc,
            jsonld,
            sha256: sha256(bytes),
        })
    }

    pub fn attr_values<'a>(&'a self, tag: &str, attr: &str) -> Vec<&'a str> {
        self.doc
            .elements
            .iter()
            .filter(|e| e.name == tag)
            .filter_map(|e| e.attr(attr))
            .collect()
    }

    pub fn canonicals(&self) -> Vec<&str> {
        self.doc
            .elements
            .iter()
            .filter(|e| {
                e.name == "link"
                    && e.attr("rel").is_some_and(|r| {
                        r.split_ascii_whitespace()
                            .any(|t| t.eq_ignore_ascii_case("canonical"))
                    })
            })
            .filter_map(|e| e.attr("href"))
            .collect()
    }

    pub fn meta(&self, key_attr: &str, key: &str) -> Option<&str> {
        self.doc
            .elements
            .iter()
            .find(|e| {
                e.name == "meta"
                    && e.attr(key_attr)
                        .is_some_and(|v| v.eq_ignore_ascii_case(key))
            })
            .and_then(|e| e.attr("content"))
    }

    pub fn is_noindex(&self) -> bool {
        self.meta("name", "robots").is_some_and(|c| {
            c.split(|ch: char| ch == ',' || ch.is_ascii_whitespace())
                .any(|t| t.eq_ignore_ascii_case("noindex") || t.eq_ignore_ascii_case("none"))
        })
    }
}

/// One advertised entry of an export: a URL (sitemap/RSS/llms.txt) or an
/// entity id (RAG catalog).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportEntry {
    pub url: Option<String>,
    pub entity_id: Option<String>,
    pub lastmod: Option<String>,
}

/// A parsed machine export. `model` is the JSON view JSONPath selectors see.
#[derive(Debug, Clone, PartialEq)]
pub struct Export {
    pub kind: ArtifactKind,
    pub path: String,
    pub entries: Vec<ExportEntry>,
    pub model: Value,
    pub sha256: Hash,
}

fn xml_unescape(s: &str) -> String {
    s.trim()
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn xml_blocks<'a>(src: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = src[i..].find(&open) {
        let s = i + p;
        let after = s + open.len();
        // `<link>` must not match `<linkfoo>`.
        if !src[after..].starts_with(['>', ' ', '\t', '\n', '\r', '/']) {
            i = after;
            continue;
        }
        let Some(gt) = src[after..].find('>') else {
            break;
        };
        let body_start = after + gt + 1;
        if src[..body_start].ends_with("/>") {
            out.push("");
            i = body_start;
            continue;
        }
        let Some(e) = src[body_start..].find(&close) else {
            break;
        };
        out.push(&src[body_start..body_start + e]);
        i = body_start + e + close.len();
    }
    out
}

fn xml_text(block: &str, tag: &str) -> Option<String> {
    xml_blocks(block, tag).first().map(|t| {
        let t = t.trim();
        let t = t
            .strip_prefix("<![CDATA[")
            .and_then(|x| x.strip_suffix("]]>"))
            .unwrap_or(t);
        xml_unescape(t)
    })
}

fn utf8<'a>(path: &str, bytes: &'a [u8]) -> Result<&'a str, Error> {
    std::str::from_utf8(bytes).map_err(|e| Error::Parse {
        artifact: path.into(),
        msg: e.to_string(),
    })
}

impl Export {
    pub fn sitemap_xml(path: &str, bytes: &[u8]) -> Result<Self, Error> {
        let src = utf8(path, bytes)?;
        let entries: Vec<ExportEntry> = xml_blocks(src, "url")
            .into_iter()
            .map(|b| ExportEntry {
                url: xml_text(b, "loc"),
                entity_id: None,
                lastmod: xml_text(b, "lastmod"),
            })
            .collect();
        let model = json!({ "urls": entries.iter().map(|e| json!({"loc": e.url, "lastmod": e.lastmod})).collect::<Vec<_>>() });
        Ok(Self::new(
            ArtifactKind::Sitemap,
            path,
            entries,
            model,
            bytes,
        ))
    }

    pub fn rss_xml(path: &str, bytes: &[u8]) -> Result<Self, Error> {
        let src = utf8(path, bytes)?;
        let head = src.split("<item").next().unwrap_or_default();
        let channel_link = xml_text(head, "link");
        let items: Vec<(Option<String>, Option<String>)> = xml_blocks(src, "item")
            .into_iter()
            .map(|b| (xml_text(b, "link"), xml_text(b, "guid")))
            .collect();
        let entries = items
            .iter()
            .map(|(link, _)| ExportEntry {
                url: link.clone(),
                entity_id: None,
                lastmod: None,
            })
            .collect();
        let model = json!({
            "channel_link": channel_link,
            "items": items.iter().map(|(l, g)| json!({"link": l, "guid": g})).collect::<Vec<_>>(),
        });
        Ok(Self::new(ArtifactKind::Rss, path, entries, model, bytes))
    }

    /// llms.txt per llmstxt.org: H1 title, optional `>` summary, `##`
    /// sections of `- [title](url): note` links.
    pub fn llms_txt(path: &str, bytes: &[u8]) -> Result<Self, Error> {
        let src = utf8(path, bytes)?;
        let mut lines = src
            .lines()
            .map(str::trim_end)
            .skip_while(|l| l.trim().is_empty());
        let first = lines.next().unwrap_or_default();
        let title = first.strip_prefix("# ").map(|t| t.trim().to_string());
        let mut summary: Option<String> = None;
        let mut sections: Vec<(String, Vec<Value>)> = Vec::new();
        let mut entries = Vec::new();
        let rest: Vec<&str> = if title.is_some() {
            lines.collect()
        } else {
            std::iter::once(first).chain(lines).collect()
        };
        for line in rest {
            let t = line.trim_start();
            if let Some(h) = t.strip_prefix("## ") {
                sections.push((h.trim().to_string(), Vec::new()));
            } else if let (Some(q), true) = (t.strip_prefix('>'), sections.is_empty()) {
                summary.get_or_insert_with(|| q.trim().to_string());
            } else if let Some(item) = t.strip_prefix("- [")
                && let Some((name, rest)) = item.split_once("](")
                && let Some((url, note)) = rest.split_once(')')
            {
                let note = note.trim_start_matches(':').trim();
                let link =
                    json!({"title": name, "url": url, "note": (!note.is_empty()).then_some(note)});
                if sections.is_empty() {
                    sections.push((String::new(), Vec::new()));
                }
                if let Some(s) = sections.last_mut() {
                    s.1.push(link);
                }
                entries.push(ExportEntry {
                    url: Some(url.to_string()),
                    entity_id: None,
                    lastmod: None,
                });
            }
        }
        let model = json!({
            "title": title,
            "summary": summary,
            "sections": sections.into_iter().map(|(t, l)| json!({"title": t, "links": l})).collect::<Vec<_>>(),
        });
        Ok(Self::new(ArtifactKind::Llms, path, entries, model, bytes))
    }

    /// Boris RAG `catalog.jsonl` (rag-export contract): one JSON row per
    /// line; content rows carry a non-empty `entity_id`.
    pub fn rag_catalog_jsonl(path: &str, bytes: &[u8]) -> Result<Self, Error> {
        Self::catalog(ArtifactKind::Rag, path, bytes)
    }

    /// Context bundle catalog, same row shape as the RAG catalog.
    pub fn context_bundle_jsonl(path: &str, bytes: &[u8]) -> Result<Self, Error> {
        Self::catalog(ArtifactKind::ContextBundle, path, bytes)
    }

    fn catalog(kind: ArtifactKind, path: &str, bytes: &[u8]) -> Result<Self, Error> {
        let src = utf8(path, bytes)?;
        let mut rows = Vec::new();
        for (n, line) in src
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
        {
            let v: Value = serde_json::from_str(line).map_err(|e| Error::Parse {
                artifact: path.into(),
                msg: format!("line {}: {e}", n + 1),
            })?;
            rows.push(v);
        }
        let entries = rows
            .iter()
            .filter_map(|r| r.get("entity_id").and_then(Value::as_str))
            .filter(|id| !id.is_empty())
            .map(|id| ExportEntry {
                url: None,
                entity_id: Some(id.to_string()),
                lastmod: None,
            })
            .collect();
        Ok(Self::new(
            kind,
            path,
            entries,
            json!({ "rows": rows }),
            bytes,
        ))
    }

    /// robots.txt per RFC 9309: groups of user-agent lines + allow/disallow
    /// rules; `Sitemap:` lines are collected separately.
    pub fn robots_txt(path: &str, bytes: &[u8]) -> Result<Self, Error> {
        let src = utf8(path, bytes)?;
        let mut groups: Vec<(Vec<String>, Vec<Value>)> = Vec::new();
        let mut sitemaps = Vec::new();
        let mut last_was_agent = false;
        for raw in src.lines() {
            let line = raw.split('#').next().unwrap_or_default().trim();
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_string());
            match k.as_str() {
                "user-agent" => {
                    if !last_was_agent || groups.is_empty() {
                        groups.push((Vec::new(), Vec::new()));
                    }
                    if let Some(g) = groups.last_mut() {
                        g.0.push(v);
                    }
                    last_was_agent = true;
                    continue;
                }
                "allow" | "disallow" => {
                    if let Some(g) = groups.last_mut() {
                        g.1.push(json!({ k.as_str(): v }));
                    }
                }
                "sitemap" => sitemaps.push(v),
                _ => {}
            }
            last_was_agent = false;
        }
        let model = json!({
            "groups": groups.into_iter().map(|(a, r)| json!({"user_agents": a, "rules": r})).collect::<Vec<_>>(),
            "sitemaps": sitemaps,
        });
        Ok(Self::new(
            ArtifactKind::RobotsTxt,
            path,
            Vec::new(),
            model,
            bytes,
        ))
    }

    fn new(
        kind: ArtifactKind,
        path: &str,
        entries: Vec<ExportEntry>,
        model: Value,
        bytes: &[u8],
    ) -> Self {
        Self {
            kind,
            path: path.into(),
            entries,
            model,
            sha256: sha256(bytes),
        }
    }
}

/// How an input stands for this evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Present,
    /// The build is known not to produce it (declared via `not_emitted`).
    NotEmitted,
    /// Unknown or unreadable — every check over it is `Limited`.
    Unavailable,
}

pub(crate) fn kind_rank(k: ArtifactKind) -> u8 {
    match k {
        ArtifactKind::Ir => 0,
        ArtifactKind::Html => 1,
        ArtifactKind::Sitemap => 2,
        ArtifactKind::Rss => 3,
        ArtifactKind::Llms => 4,
        ArtifactKind::Rag => 5,
        ArtifactKind::ContextBundle => 6,
        ArtifactKind::RobotsTxt => 7,
    }
}

/// Everything `eval` reads. Construct with [`ArtifactSet::builder`].
#[derive(Debug, Clone)]
pub struct ArtifactSet {
    pub account: AccountId,
    pub manifest: Manifest,
    pub ir: Option<IrGraph>,
    /// Sorted by route.
    pub pages: Vec<HtmlPage>,
    /// Sorted by artifact kind.
    pub exports: Vec<Export>,
    pub not_emitted: Vec<ArtifactKind>,
    /// Explicit archetype per entity id; falls back to the IR `role`.
    pub archetypes: BTreeMap<String, Archetype>,
}

impl ArtifactSet {
    pub fn builder() -> ArtifactSetBuilder {
        ArtifactSetBuilder::default()
    }

    pub fn deploy(&self) -> Option<&TargetDeployProfile> {
        self.manifest.deploy.as_ref()
    }

    pub fn export(&self, kind: ArtifactKind) -> Option<&Export> {
        self.exports.iter().find(|e| e.kind == kind)
    }

    /// The IR graph, unless the manifest declares it missing.
    pub fn ir_graph(&self) -> Option<&IrGraph> {
        self.ir
            .as_ref()
            .filter(|_| !self.manifest.missing.contains(&ArtifactKind::Ir))
    }

    pub fn availability(&self, kind: ArtifactKind) -> Availability {
        let present = match kind {
            ArtifactKind::Ir => self.ir.is_some(),
            ArtifactKind::Html => !self.pages.is_empty(),
            k => self.export(k).is_some(),
        };
        if self.manifest.missing.contains(&kind) {
            Availability::Unavailable
        } else if present {
            Availability::Present
        } else if self.not_emitted.contains(&kind) {
            Availability::NotEmitted
        } else {
            Availability::Unavailable
        }
    }

    pub fn page_archetype(&self, page: &HtmlPage) -> Option<Archetype> {
        let id = page.entity_id.as_deref()?;
        if let Some(a) = self.archetypes.get(id) {
            return Some(*a);
        }
        match self.ir_graph()?.node(id)?.role.as_str() {
            "trunk" => Some(Archetype::Trunk),
            "satellite" => Some(Archetype::Satellite),
            _ => None,
        }
    }

    pub fn page_for_entity(&self, id: &str) -> Option<&HtmlPage> {
        self.pages
            .iter()
            .find(|p| p.entity_id.as_deref() == Some(id))
    }

    /// Content hash over the manifest and every evaluation-relevant input.
    pub fn hash(&self) -> Hash {
        #[derive(Serialize)]
        struct Canon<'a> {
            account: &'a AccountId,
            manifest: &'a Manifest,
            pages: Vec<(&'a str, Option<&'a str>, Hash)>,
            not_emitted: &'a [ArtifactKind],
            archetypes: &'a BTreeMap<String, Archetype>,
        }
        let canon = Canon {
            account: &self.account,
            manifest: &self.manifest,
            pages: self
                .pages
                .iter()
                .map(|p| (p.route.as_str(), p.entity_id.as_deref(), p.sha256))
                .collect(),
            not_emitted: &self.not_emitted,
            archetypes: &self.archetypes,
        };
        sha256(&serde_json::to_vec(&canon).expect("artifact manifest serializes"))
    }
}

#[derive(Debug, Default)]
pub struct ArtifactSetBuilder {
    account: Option<AccountId>,
    ir: Option<IrGraph>,
    pages: Vec<HtmlPage>,
    exports: Vec<Export>,
    deploy: Option<TargetDeployProfile>,
    missing: Vec<ArtifactKind>,
    not_emitted: Vec<ArtifactKind>,
    archetypes: BTreeMap<String, Archetype>,
}

impl ArtifactSetBuilder {
    pub fn account(mut self, a: AccountId) -> Self {
        self.account = Some(a);
        self
    }
    pub fn ir(mut self, g: IrGraph) -> Self {
        self.ir = Some(g);
        self
    }
    pub fn page(mut self, p: HtmlPage) -> Self {
        self.pages.retain(|x| x.route != p.route);
        self.pages.push(p);
        self
    }
    pub fn export(mut self, e: Export) -> Self {
        self.exports.retain(|x| x.kind != e.kind);
        self.exports.push(e);
        self
    }
    pub fn deploy(mut self, d: TargetDeployProfile) -> Self {
        self.deploy = Some(d);
        self
    }
    /// Input that should exist but could not be obtained → `Limited`
    /// (overrides any bytes supplied for the same kind).
    pub fn missing(mut self, k: ArtifactKind) -> Self {
        self.missing.push(k);
        self
    }
    /// Input the build is known not to produce (e.g. no `--rss`).
    pub fn not_emitted(mut self, k: ArtifactKind) -> Self {
        self.not_emitted.push(k);
        self
    }
    pub fn archetype(mut self, entity_id: &str, a: Archetype) -> Self {
        self.archetypes.insert(entity_id.into(), a);
        self
    }

    pub fn build(self) -> ArtifactSet {
        let mut pages = self.pages;
        pages.sort_by(|a, b| a.route.cmp(&b.route));
        let mut exports = self.exports;
        exports.sort_by_key(|e| kind_rank(e.kind));
        let norm = |mut v: Vec<ArtifactKind>| {
            v.sort_by_key(|k| kind_rank(*k));
            v.dedup();
            v
        };
        let html = (!pages.is_empty()).then(|| {
            let mut buf = String::new();
            for p in &pages {
                buf.push_str(&p.route);
                buf.push('\0');
                buf.push_str(&p.sha256.to_hex());
                buf.push('\n');
            }
            ArtifactRef {
                path: "html/".into(),
                sha256: sha256(buf.as_bytes()),
            }
        });
        let manifest = Manifest {
            ir: self.ir.as_ref().map(|g| ArtifactRef {
                path: g.path.clone(),
                sha256: g.sha256,
            }),
            html,
            exports: exports
                .iter()
                .map(|e| ArtifactRef {
                    path: e.path.clone(),
                    sha256: e.sha256,
                })
                .collect(),
            deploy: self.deploy,
            missing: norm(self.missing),
        };
        ArtifactSet {
            account: self.account.unwrap_or_else(|| AccountId("local".into())),
            manifest,
            ir: self.ir,
            pages,
            exports,
            not_emitted: norm(self.not_emitted),
            archetypes: self.archetypes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exports() {
        let sm = Export::sitemap_xml(
            "sitemap.xml",
            b"<urlset><url><loc>https://d.example/a.html</loc><lastmod>2026-01-02</lastmod></url><url><loc>https://d.example/b&amp;c.html</loc></url></urlset>",
        )
        .unwrap();
        assert_eq!(sm.entries.len(), 2);
        assert_eq!(
            sm.entries[1].url.as_deref(),
            Some("https://d.example/b&c.html")
        );
        assert_eq!(sm.entries[0].lastmod.as_deref(), Some("2026-01-02"));

        let rss = Export::rss_xml(
            "rss.xml",
            b"<rss><channel><link>https://d.example/</link><item><link>https://d.example/a.html</link><guid>https://d.example/a.html</guid></item></channel></rss>",
        )
        .unwrap();
        assert_eq!(rss.entries.len(), 1);
        assert_eq!(rss.model["channel_link"], "https://d.example/");

        let llms = Export::llms_txt(
            "llms.txt",
            b"# Site\n\n> Summary.\n\n## Docs\n\n- [A](/a.html): First.\n  - [B](/b.html)\n",
        )
        .unwrap();
        assert_eq!(llms.model["title"], "Site");
        assert_eq!(llms.model["summary"], "Summary.");
        assert_eq!(llms.entries.len(), 2);
        let no_h1 = Export::llms_txt("llms.txt", b"Site\n- [A](/a.html)\n").unwrap();
        assert!(no_h1.model["title"].is_null());

        let robots = Export::robots_txt(
            "robots.txt",
            b"User-agent: a\nUser-agent: b\nDisallow: /x # c\n\nUser-agent: *\nAllow: /\nSitemap: https://d.example/sitemap.xml\n",
        )
        .unwrap();
        assert_eq!(robots.model["groups"].as_array().unwrap().len(), 2);
        assert_eq!(robots.model["sitemaps"][0], "https://d.example/sitemap.xml");
    }
}
