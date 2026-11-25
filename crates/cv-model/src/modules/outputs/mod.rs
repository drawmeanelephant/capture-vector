//! Outputs — compiler products as nouns: IR, RAG packs, context bundles,
//! llms.txt, RSS, sitemap, proof pack, search index, dist. Classes come
//! from the artifact index Boris writes; the inspector shows provenance
//! (compiler, profile target, snapshot, freshness vs the last publish).

use crate::module::{
    Chip, InspectorModel, Link, Module, ModuleSpec, NavItem, NavModel, OutputRecord,
    ProjectSnapshot, PublishMark, ReadingLine, ReadingModel, Registry, RowModel, Slot, Tone,
};
use cv_core::events::engine::EngineEvent;
use cv_core::events::{AppEvent, Envelope};
use cv_core::id::{AccountId, AttrValue, NounAttrRow, NounId, NounKind, StableId, noun_kind};
use std::collections::BTreeMap;

pub const ID: &str = "outputs";

/// `(mailbox key, label)`, in display order. Every class is listed even
/// when empty, so a missing product is visible rather than absent.
pub const CLASSES: &[(&str, &str)] = &[
    ("ir", "IR graph"),
    ("rag", "RAG packs"),
    ("context", "Context bundles"),
    ("llms", "llms.txt"),
    ("rss", "RSS"),
    ("sitemap", "Sitemap"),
    ("proof", "Proof pack"),
    ("search", "Search index"),
    ("dist", "Dist"),
];

pub fn register(r: &mut Registry) {
    r.register(Box::new(OutputsModule::default()))
        .expect("outputs module id is unique");
}

/// Classify by the artifact index's own `kind`/`path`.
pub fn class(o: &OutputRecord) -> &'static str {
    let (p, k) = (o.path.to_lowercase(), o.kind.to_lowercase());
    let file = p.rsplit('/').next().unwrap_or(&p);
    if k == "ir" || p.ends_with("graph.json") {
        "ir"
    } else if k.contains("rag") || p.contains("/rag/") {
        "rag"
    } else if k.contains("context") || p.contains("/context/") {
        "context"
    } else if file.starts_with("llms") && file.ends_with(".txt") {
        "llms"
    } else if k.contains("rss") || k.contains("feed") || file == "rss.xml" || file == "feed.xml" {
        "rss"
    } else if k.contains("sitemap") || file.starts_with("sitemap") {
        "sitemap"
    } else if p.starts_with("_boris/proof/") || k.contains("proof") {
        "proof"
    } else if k.contains("search") {
        "search"
    } else {
        "dist"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    NeverPublished,
    Published,
    Changed,
    New,
}

impl Freshness {
    pub fn label(self) -> &'static str {
        match self {
            Freshness::NeverPublished => "never published",
            Freshness::Published => "as published",
            Freshness::Changed => "changed since publish",
            Freshness::New => "new since publish",
        }
    }
}

pub fn freshness(o: &OutputRecord, mark: Option<&PublishMark>) -> Freshness {
    match mark.map(|m| m.hashes.get(&o.path)) {
        None => Freshness::NeverPublished,
        Some(None) => Freshness::New,
        Some(Some(h)) if *h == o.sha256 => Freshness::Published,
        Some(Some(_)) => Freshness::Changed,
    }
}

#[derive(Default)]
pub struct OutputsModule {
    snap: ProjectSnapshot,
    all: Vec<usize>,
    by_class: BTreeMap<&'static str, Vec<usize>>,
    by_path: BTreeMap<String, usize>,
    /// Page source paths per output path.
    sources: BTreeMap<String, Vec<String>>,
    /// Outcome of the newest build; `false` means outputs predate a
    /// failed build and are the last good ones.
    last_build_ok: Option<bool>,
}

impl OutputsModule {
    fn view(&self, nav: Option<&str>) -> &[usize] {
        match nav.unwrap_or("all") {
            "all" => &self.all,
            k => self.by_class.get(k).map_or(&[], Vec::as_slice),
        }
    }
}

impl Module for OutputsModule {
    fn spec(&self) -> ModuleSpec {
        ModuleSpec {
            id: ID,
            title: "Outputs",
            slot: Slot::Outputs,
        }
    }

    fn ingest(&mut self, env: &Envelope) -> bool {
        let ok = match &env.payload {
            AppEvent::Engine(EngineEvent::BuildFinished { .. }) => true,
            AppEvent::Engine(EngineEvent::BuildFailed { .. }) => false,
            _ => return false,
        };
        self.last_build_ok = Some(ok);
        true
    }

    fn load(&mut self, snap: &ProjectSnapshot) -> bool {
        // Pages are only read below, for `sources`: don't copy their bodies.
        self.snap = ProjectSnapshot {
            epoch: snap.epoch,
            compiler: snap.compiler.clone(),
            target: snap.target.clone(),
            pages: Vec::new(),
            outputs: snap.outputs.clone(),
            last_publish: snap.last_publish.clone(),
            artifact_root: snap.artifact_root.clone(),
        };
        self.by_class.clear();
        self.by_path.clear();
        self.sources.clear();
        let mut idx: Vec<usize> = (0..snap.outputs.len()).collect();
        idx.sort_by(|&a, &b| snap.outputs[a].path.cmp(&snap.outputs[b].path));
        for &i in &idx {
            let o = &snap.outputs[i];
            self.by_class.entry(class(o)).or_default().push(i);
            self.by_path.insert(o.path.clone(), i);
        }
        self.all = idx;
        for p in &snap.pages {
            if let Some(o) = &p.output {
                self.sources
                    .entry(o.clone())
                    .or_default()
                    .push(p.path.clone());
            }
        }
        true
    }

    fn nav(&self) -> NavModel {
        let mut items = vec![NavItem {
            key: "all".into(),
            label: "All outputs".into(),
            count: Some(self.all.len() as u64),
        }];
        items.extend(CLASSES.iter().map(|(k, l)| NavItem {
            key: (*k).into(),
            label: (*l).into(),
            count: Some(self.view(Some(k)).len() as u64),
        }));
        NavModel { items }
    }

    fn list_len(&self, nav: Option<&str>) -> usize {
        self.view(nav).len()
    }

    fn row(&self, nav: Option<&str>, i: usize) -> RowModel {
        let Some(o) = self.view(nav).get(i).map(|&j| &self.snap.outputs[j]) else {
            return RowModel::default();
        };
        let f = freshness(o, self.snap.last_publish.as_ref());
        let stale = self.last_build_ok == Some(false);
        RowModel {
            key: o.path.clone(),
            title: o.path.clone(),
            detail: format!("{} · {} B", o.kind, o.bytes),
            badge: Some(Chip::new(
                f.label(),
                match f {
                    Freshness::Changed | Freshness::New => Tone::Warn,
                    Freshness::Published => Tone::Ok,
                    Freshness::NeverPublished => Tone::Dim,
                },
            )),
            tone: match f {
                _ if stale => Tone::Warn,
                Freshness::Changed | Freshness::New => Tone::Warn,
                Freshness::Published => Tone::Ok,
                Freshness::NeverPublished => Tone::Normal,
            },
            rail: None,
            path: true,
        }
    }

    fn index_of_key(&self, nav: Option<&str>, key: &str) -> Option<usize> {
        // `load` fills every view in path order.
        let v = self.view(nav);
        let path = |j: usize| self.snap.outputs[j].path.as_str();
        let i = v.partition_point(|&j| path(j) < key);
        v.get(i).is_some_and(|&j| path(j) == key).then_some(i)
    }

    fn reading(&self, key: &str) -> ReadingModel {
        let Some(o) = self.by_path.get(key).map(|&i| &self.snap.outputs[i]) else {
            return ReadingModel {
                title: key.into(),
                ..Default::default()
            };
        };
        let line = |l: &str, t: String, mono| ReadingLine {
            label: Some(l.into()),
            text: t,
            tone: Tone::Normal,
            mono,
        };
        let mut lines = vec![
            line("class", class(o).into(), false),
            line("kind", o.kind.clone(), true),
            line("producer", o.producer.clone(), true),
        ];
        for s in self.sources.get(key).into_iter().flatten() {
            lines.push(line("source", s.clone(), true));
        }
        if self.last_build_ok == Some(false) {
            lines.push(ReadingLine {
                label: Some("note".into()),
                text: "latest build failed — this is the last good output".into(),
                tone: Tone::Warn,
                mono: false,
            });
        }
        ReadingModel {
            title: o.path.clone(),
            lines,
            ..Default::default()
        }
    }

    fn inspector(&self, key: &str) -> InspectorModel {
        let Some(o) = self.by_path.get(key).map(|&i| &self.snap.outputs[i]) else {
            return InspectorModel::default();
        };
        let opt = |v: &Option<String>| v.clone().unwrap_or_else(|| "—".into());
        let mark = self.snap.last_publish.as_ref();
        InspectorModel {
            fields: vec![
                ("path".into(), o.path.clone()),
                ("class".into(), class(o).into()),
                ("status".into(), o.status.clone()),
                ("required".into(), o.required.to_string()),
                ("bytes".into(), o.bytes.to_string()),
                ("sha256".into(), o.sha256.clone()),
                ("format".into(), opt(&o.format_version)),
                ("compiler".into(), opt(&self.snap.compiler)),
                ("profile".into(), opt(&self.snap.target)),
                ("snapshot".into(), format!("epoch {}", self.snap.epoch)),
                ("fresh".into(), freshness(o, mark).label().into()),
                (
                    "publish".into(),
                    mark.map_or_else(|| "—".into(), |m| m.label.clone()),
                ),
            ],
            diagnostics: vec![],
            lists: vec![(
                "Sources".into(),
                self.sources.get(key).cloned().unwrap_or_default(),
            )],
        }
    }

    fn noun(&self, key: &str) -> Option<NounId> {
        self.by_path.get(key)?;
        Some(NounId {
            account: AccountId("local".into()),
            kind: NounKind(noun_kind::OUTPUT.into()),
            stable: StableId(key.into()),
        })
    }

    fn contains(&self, key: &str) -> bool {
        self.by_path.contains_key(key)
    }

    fn link(&self, key: &str) -> Option<Link> {
        let src = self.sources.get(key)?.first()?;
        Some(Link {
            module: super::pages::ID,
            key: src.clone(),
            label: "Open source page".into(),
        })
    }

    fn attr_rows(&self) -> Vec<NounAttrRow> {
        let mark = self.snap.last_publish.as_ref();
        self.all
            .iter()
            .filter_map(|&i| {
                let o = &self.snap.outputs[i];
                let t = |s: &str| AttrValue::Text(s.into());
                let mut f = BTreeMap::new();
                f.insert("path".into(), t(&o.path));
                f.insert("class".into(), t(class(o)));
                f.insert("kind".into(), t(&o.kind));
                f.insert("producer".into(), t(&o.producer));
                f.insert("bytes".into(), AttrValue::Int(o.bytes as i64));
                f.insert("required".into(), AttrValue::Bool(o.required));
                f.insert("fresh".into(), t(freshness(o, mark).label()));
                Some(NounAttrRow {
                    noun: self.noun(&o.path)?,
                    fields: f,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
