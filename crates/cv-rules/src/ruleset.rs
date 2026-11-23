//! Rule packs: TOML in, validated `Ruleset` out. The v1 packs are compiled
//! in with `include_str!` — loading them performs no IO.

use crate::html::CssSelector;
use crate::jsonpath::{JsonPath, is_live_only};
use crate::{Error, sha256};
use cv_core::rules::{AuthorityTier, CheckSpec, Rule, Selector};
use cv_core::sys::Hash;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// v1 packs, `(file name, source)`, in pack-id order.
pub const V1_PACKS: &[(&str, &str)] = &[
    (
        "a11y-content.toml",
        include_str!("../../../rulepacks/a11y-content.toml"),
    ),
    (
        "aeo-llm.toml",
        include_str!("../../../rulepacks/aeo-llm.toml"),
    ),
    (
        "seo-core.toml",
        include_str!("../../../rulepacks/seo-core.toml"),
    ),
    (
        "targets.toml",
        include_str!("../../../rulepacks/targets.toml"),
    ),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackMeta {
    pub id: String,
    pub version: u32,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RulePack {
    pub meta: PackMeta,
    pub rules: Vec<Rule>,
}

#[derive(Deserialize)]
struct PackFile {
    pack: PackMeta,
    #[serde(default)]
    rule: Vec<Rule>,
}

impl RulePack {
    pub fn from_toml(src: &str) -> Result<Self, Error> {
        let f: PackFile = toml::from_str(src).map_err(|e| Error::Pack(e.to_string()))?;
        let pack = Self {
            meta: f.pack,
            rules: f.rule,
        };
        pack.validate()?;
        Ok(pack)
    }

    fn validate(&self) -> Result<(), Error> {
        let bad = |rule: &str, m: &str| Err(Error::Pack(format!("{}/{rule}: {m}", self.meta.id)));
        if self.meta.id.is_empty() {
            return Err(Error::Pack("pack id is empty".into()));
        }
        for r in &self.rules {
            let c = &r.citation;
            if r.id.trim().is_empty() {
                return bad("?", "empty rule id");
            }
            if r.remediation.trim().is_empty() {
                return bad(&r.id, "empty remediation");
            }
            if r.archetypes.is_empty() {
                return bad(&r.id, "no archetypes");
            }
            // Folklore rules don't ship uncited: every citation field is mandatory.
            if c.source.trim().is_empty() || c.frozen_quote.trim().is_empty() {
                return bad(&r.id, "citation source/frozen_quote empty");
            }
            if !c.url.starts_with("https://") {
                return bad(&r.id, "citation url must be https://");
            }
            if !is_iso_date(&c.captured) {
                return bad(&r.id, "citation captured must be YYYY-MM-DD");
            }
            validate_check(&r.check).or_else(|e| bad(&r.id, &e.to_string()))?;
        }
        Ok(())
    }
}

fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

fn validate_check(c: &CheckSpec) -> Result<(), Error> {
    match c {
        CheckSpec::Parity { .. } => Ok(()),
        CheckSpec::Count { selector, .. }
        | CheckSpec::Exists { selector, .. }
        | CheckSpec::NotPresent { selector, .. } => match selector {
            Selector::Html(s) => CssSelector::parse(s).map(|_| ()),
            Selector::JsonPath(p) if is_live_only(p) => Ok(()),
            Selector::JsonPath(p) => JsonPath::parse(p).map(|_| ()),
            Selector::IrNode { kind, .. } if kind.is_empty() => {
                Err(Error::Selector("IrNode kind is empty (use `*`)".into()))
            }
            Selector::IrNode { .. } => Ok(()),
        },
    }
}

/// A validated, ordered set of rule packs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Ruleset {
    packs: Vec<RulePack>,
}

impl Ruleset {
    pub fn new(mut packs: Vec<RulePack>) -> Result<Self, Error> {
        packs.sort_by(|a, b| a.meta.id.cmp(&b.meta.id));
        let mut pack_ids = BTreeSet::new();
        let mut rule_ids = BTreeSet::new();
        for p in &packs {
            p.validate()?;
            if !pack_ids.insert(p.meta.id.as_str()) {
                return Err(Error::Pack(format!("duplicate pack `{}`", p.meta.id)));
            }
            for r in &p.rules {
                if !rule_ids.insert(r.id.as_str()) {
                    return Err(Error::Pack(format!("duplicate rule `{}`", r.id)));
                }
            }
        }
        Ok(Self { packs })
    }

    pub fn from_toml_sources(srcs: &[&str]) -> Result<Self, Error> {
        Self::new(
            srcs.iter()
                .map(|s| RulePack::from_toml(s))
                .collect::<Result<_, _>>()?,
        )
    }

    /// The shipped v1 packs: `a11y-content`, `aeo-llm`, `seo-core`, `targets`.
    pub fn v1() -> Self {
        Self::from_toml_sources(&V1_PACKS.iter().map(|(_, s)| *s).collect::<Vec<_>>())
            .expect("shipped v1 rule packs are valid (tested)")
    }

    pub fn packs(&self) -> &[RulePack] {
        &self.packs
    }

    pub fn without_pack(&self, id: &str) -> Self {
        Self {
            packs: self
                .packs
                .iter()
                .filter(|p| p.meta.id != id)
                .cloned()
                .collect(),
        }
    }

    pub fn rule(&self, id: &str) -> Option<(&RulePack, &Rule)> {
        self.packs
            .iter()
            .find_map(|p| p.rules.iter().find(|r| r.id == id).map(|r| (p, r)))
    }

    pub fn hash(&self) -> Hash {
        sha256(&serde_json::to_vec(&self.packs).expect("ruleset serializes"))
    }

    /// `rulepacks/SOURCES.md`, generated from every rule's citation.
    pub fn sources_markdown(&self) -> String {
        #[derive(Default)]
        struct Src<'a> {
            rows: Vec<(&'a str, &'a str, AuthorityTier, String, &'a str)>,
            quotes: BTreeSet<&'a str>,
        }
        let mut by_source: BTreeMap<(&str, &str), Src> = BTreeMap::new();
        for p in &self.packs {
            for r in &p.rules {
                let s = by_source
                    .entry((r.citation.source.as_str(), r.citation.url.as_str()))
                    .or_default();
                s.rows.push((
                    r.id.as_str(),
                    p.meta.id.as_str(),
                    r.tier,
                    format!("{:?}", r.severity),
                    r.citation.captured.as_str(),
                ));
                s.quotes.insert(r.citation.frozen_quote.as_str());
            }
        }
        let rules: usize = self.packs.iter().map(|p| p.rules.len()).sum();
        let mut out = String::new();
        out.push_str("# Rule-pack sources\n\n");
        out.push_str(
            "<!-- GENERATED from rule citations by `cargo run -p cv-rules --example gen-sources > rulepacks/SOURCES.md`; do not edit by hand. -->\n\n",
        );
        out.push_str(&format!(
            "{rules} rules in {} packs cite {} sources. A rule without a citation does not load.\n\n",
            self.packs.len(),
            by_source.len()
        ));
        out.push_str(
            "Tiers: `NormativeSpec` (a published standard) and `EngineDocumented` (the Boris compiler contract or a search engine's own documentation) may block a publish when they fail at `Error` severity; `Heuristic` findings are advisory and can never block.\n",
        );
        for ((source, url), s) in &by_source {
            out.push_str(&format!("\n## {source}\n\n<{url}>\n\n"));
            for q in &s.quotes {
                out.push_str(&format!("> {q}\n\n"));
            }
            out.push_str("| Rule | Pack | Tier | Severity | Captured |\n|---|---|---|---|---|\n");
            let mut rows = s.rows.clone();
            rows.sort();
            for (id, pack, tier, sev, cap) in rows {
                out.push_str(&format!("| `{id}` | {pack} | {tier:?} | {sev} | {cap} |\n"));
            }
        }
        out
    }
}
