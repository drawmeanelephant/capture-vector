//! `eval(&ArtifactSet, &Ruleset) -> Report`: pure, no IO, deterministic.

use crate::artifacts::{ArtifactSet, Availability, Export, HtmlPage};
use crate::html::CssSelector;
use crate::jsonpath::{JsonPath, is_live_only};
use crate::report::{AuditFinding, Report};
use crate::ruleset::{RulePack, Ruleset};
use crate::sha256;
use cv_core::id::{NounId, NounKind, StableId, noun_kind};
use cv_core::rules::{
    Archetype, ArtifactKind, CheckSpec, CheckStatus, CompareOp, DedupKey, Evidence, Finding,
    FindingLifecycle, FixIt, ParityField, Rule, Selector, SlashPolicy,
};
use serde_json::{Value, json};

pub fn eval(set: &ArtifactSet, rules: &Ruleset) -> Report {
    let mut out = Vec::new();
    for pack in rules.packs() {
        for rule in &pack.rules {
            Ctx {
                set,
                pack,
                rule,
                out: &mut out,
            }
            .run();
        }
    }
    out.sort_by(|a: &AuditFinding, b: &AuditFinding| {
        (
            &a.pack,
            &a.finding.code,
            &a.finding.subject,
            a.finding.key.target_hash,
        )
            .cmp(&(
                &b.pack,
                &b.finding.code,
                &b.finding.subject,
                b.finding.key.target_hash,
            ))
    });
    out.dedup_by(|a, b| a.finding.key == b.finding.key);
    Report {
        artifact_set_hash: set.hash(),
        ruleset_hash: rules.hash(),
        findings: out,
    }
}

enum Subject<'a> {
    Page(&'a HtmlPage),
    Entity(&'a str),
    /// Fixed synthetic subject ("ruleset", "live-http", …) — never a
    /// filesystem path. Real artifact locations go through
    /// `Subject::Artifact` so a rename can't churn `DedupKey.subject`
    /// (`StableId` is never a filesystem path).
    Output(&'a str),
    Artifact(ArtifactKind),
}

/// Predicate a check applies to an observed count.
#[derive(Clone, Copy)]
enum Pred {
    Cmp(CompareOp, u64),
    AtLeastOne,
    None,
}

impl Pred {
    fn holds(self, n: u64) -> bool {
        match self {
            Pred::Cmp(op, t) => match op {
                CompareOp::Lt => n < t,
                CompareOp::Le => n <= t,
                CompareOp::Eq => n == t,
                CompareOp::Ge => n >= t,
                CompareOp::Gt => n > t,
            },
            Pred::AtLeastOne => n >= 1,
            Pred::None => n == 0,
        }
    }

    fn describe(self) -> String {
        match self {
            Pred::Cmp(op, t) => format!("{op:?} {t}"),
            Pred::AtLeastOne => "exists".into(),
            Pred::None => "not present".into(),
        }
    }
}

fn kind_name(k: ArtifactKind) -> String {
    format!("{k:?}")
}

struct Ctx<'a, 'o> {
    set: &'a ArtifactSet,
    pack: &'a RulePack,
    rule: &'a Rule,
    out: &'o mut Vec<AuditFinding>,
}

impl<'a> Ctx<'a, '_> {
    fn emit(
        &mut self,
        status: CheckStatus,
        subject: Subject<'_>,
        target: &str,
        summary: String,
        detail: Value,
    ) {
        let (kind, stable) = match subject {
            Subject::Page(p) => (
                noun_kind::PAGE,
                p.entity_id.clone().unwrap_or_else(|| p.route.clone()),
            ),
            Subject::Entity(id) => (noun_kind::PAGE, id.to_string()),
            Subject::Output(label) => (noun_kind::OUTPUT, label.to_string()),
            Subject::Artifact(k) => (noun_kind::OUTPUT, kind_name(k)),
        };
        let subject = NounId {
            account: self.set.account.clone(),
            kind: NounKind::register(kind),
            stable: StableId(stable),
        };
        let mut detail = detail;
        if let Value::Object(m) = &mut detail {
            m.insert("pack".into(), json!(self.pack.meta.id));
            m.insert("citation".into(), json!(self.rule.citation.url));
        }
        let finding = Finding {
            code: self.rule.id.clone(),
            tier: self.rule.tier,
            severity: self.rule.severity,
            status,
            key: DedupKey {
                code: self.rule.id.clone(),
                subject: subject.clone(),
                target_hash: sha256(target.as_bytes()),
            },
            subject,
            evidence: Evidence {
                summary,
                detail: Some(detail),
            },
            fix: (status == CheckStatus::Failed).then(|| FixIt {
                description: self.rule.remediation.clone(),
                replacement: None,
            }),
            lifecycle: FindingLifecycle::Open,
        };
        self.out.push(AuditFinding {
            pack: self.pack.meta.id.clone(),
            finding,
        });
    }

    fn limited(&mut self, subject: Subject<'_>, target: &str, why: String) {
        self.emit(
            CheckStatus::Limited,
            subject,
            target,
            format!("not checked: {why}"),
            json!({ "reason": why }),
        );
    }

    fn limited_unavailable(&mut self, kind: ArtifactKind) {
        let declared = self.set.manifest.missing.contains(&kind);
        let why = if declared {
            format!("{} input declared missing", kind_name(kind))
        } else {
            format!("{} input not supplied", kind_name(kind))
        };
        self.limited(
            Subject::Output("artifact-set"),
            &format!("unavailable:{kind:?}"),
            why,
        );
    }

    fn in_scope(&self, page: &HtmlPage) -> bool {
        in_scope(self.set, self.rule, page)
    }

    fn run(&mut self) {
        match &self.rule.check {
            CheckSpec::Parity { field } => match field {
                ParityField::CanonicalUrl => self.parity_canonical(),
                ParityField::PageSet => self.parity_pageset(),
                ParityField::LastModified => self.parity_lastmod(),
            },
            CheckSpec::Count {
                artifact,
                selector,
                op,
                threshold,
            } => self.count(*artifact, selector, Pred::Cmp(*op, *threshold)),
            CheckSpec::Exists { artifact, selector } => {
                self.count(*artifact, selector, Pred::AtLeastOne)
            }
            CheckSpec::NotPresent { artifact, selector } => {
                self.count(*artifact, selector, Pred::None)
            }
        }
    }

    fn count(&mut self, artifact: ArtifactKind, selector: &Selector, pred: Pred) {
        if let Selector::JsonPath(p) = selector
            && is_live_only(p)
        {
            let why = format!("`{p}` needs a live HTTP fetch; cv-rules performs no network IO");
            self.limited(Subject::Output("live-http"), p, why);
            return;
        }
        let mut availability = self.set.availability(artifact);
        if artifact == ArtifactKind::RobotsTxt && availability != Availability::Present {
            match self.set.deploy() {
                Some(d) if !d.expects_robots_txt => return,
                None => {
                    let why = "deploy profile and robots.txt both unavailable".to_string();
                    self.limited(Subject::Output("robots.txt"), "robots", why);
                    return;
                }
                // With a deploy contract, an omitted robots.txt counts as
                // absent unless the caller explicitly couldn't inspect it.
                Some(_) if !self.set.manifest.missing.contains(&artifact) => {
                    availability = Availability::NotEmitted;
                }
                Some(_) => {}
            }
        }
        match artifact {
            ArtifactKind::Html => self.count_html(selector, pred),
            ArtifactKind::Ir => self.count_ir(selector, pred),
            k => self.count_export(k, availability, selector, pred),
        }
    }

    fn verdict(&mut self, subject: Subject<'_>, target: &str, what: &str, n: u64, pred: Pred) {
        if !pred.holds(n) {
            let summary = format!("{what}: observed {n}, expected {}", pred.describe());
            self.emit(
                CheckStatus::Failed,
                subject,
                target,
                summary,
                json!({ "selector": what, "observed": n, "expected": pred.describe() }),
            );
        }
    }

    fn count_html(&mut self, selector: &Selector, pred: Pred) {
        match self.set.availability(ArtifactKind::Html) {
            Availability::Present => {}
            Availability::NotEmitted => return,
            Availability::Unavailable => return self.limited_unavailable(ArtifactKind::Html),
        }
        let (set, rule) = (self.set, self.rule);
        let pages = set.pages.iter().filter(|p| in_scope(set, rule, p));
        match selector {
            Selector::Html(s) => {
                let Ok(css) = CssSelector::parse(s) else {
                    return self.limited(
                        Subject::Output("ruleset"),
                        s,
                        format!("unsupported selector `{s}`"),
                    );
                };
                for page in pages {
                    let n = page.doc.select(&css).len() as u64;
                    self.verdict(Subject::Page(page), &page.route, s, n, pred);
                }
            }
            Selector::JsonPath(p) => {
                let Ok(jp) = JsonPath::parse(p) else {
                    return self.limited(
                        Subject::Output("ruleset"),
                        p,
                        format!("unsupported JSONPath `{p}`"),
                    );
                };
                for page in pages {
                    let n = page.jsonld.iter().flatten().map(|v| jp.count(v)).sum();
                    self.verdict(
                        Subject::Page(page),
                        &page.route,
                        &format!("JSON-LD {p}"),
                        n,
                        pred,
                    );
                }
            }
            Selector::IrNode { kind, field } => {
                let Some(ir) = set.ir_graph() else {
                    return self.limited_unavailable(ArtifactKind::Ir);
                };
                for page in pages {
                    let n = page
                        .entity_id
                        .as_deref()
                        .and_then(|id| ir.node(id))
                        .is_some_and(|node| {
                            node.kind_matches(kind) && node.field_matches(field.as_deref())
                        }) as u64;
                    self.verdict(
                        Subject::Page(page),
                        &page.route,
                        &format!("IR {kind} {field:?}"),
                        n,
                        pred,
                    );
                }
            }
        }
    }

    fn count_ir(&mut self, selector: &Selector, pred: Pred) {
        let Some(ir) = self.set.ir_graph() else {
            return self.limited_unavailable(ArtifactKind::Ir);
        };
        let path = ir.path.as_str();
        match selector {
            Selector::IrNode { kind, field } => {
                let matched: Vec<&str> = ir
                    .nodes
                    .iter()
                    .filter(|n| n.kind_matches(kind) && n.field_matches(field.as_deref()))
                    .map(|n| n.id.as_str())
                    .collect();
                if matches!(pred, Pred::None) {
                    for id in matched {
                        self.verdict(
                            Subject::Entity(id),
                            id,
                            &format!("IR {kind} {field:?}"),
                            1,
                            pred,
                        );
                    }
                } else {
                    let n = matched.len() as u64;
                    self.verdict(
                        Subject::Artifact(ArtifactKind::Ir),
                        path,
                        &format!("IR {kind} {field:?}"),
                        n,
                        pred,
                    );
                }
            }
            Selector::JsonPath(p) => match JsonPath::parse(p) {
                Ok(jp) => self.verdict(
                    Subject::Artifact(ArtifactKind::Ir),
                    path,
                    p,
                    jp.count(&ir.raw),
                    pred,
                ),
                Err(_) => self.limited(
                    Subject::Output("ruleset"),
                    p,
                    format!("unsupported JSONPath `{p}`"),
                ),
            },
            Selector::Html(s) => self.limited(
                Subject::Output("ruleset"),
                s,
                "HTML selector over IR is unsupported; select final HTML".into(),
            ),
        }
    }

    fn count_export(
        &mut self,
        kind: ArtifactKind,
        availability: Availability,
        selector: &Selector,
        pred: Pred,
    ) {
        if availability == Availability::Unavailable {
            return self.limited_unavailable(kind);
        }
        let export = self.set.export(kind);
        let path = export.map_or(kind_name(kind), |e| e.path.clone());
        match selector {
            Selector::JsonPath(p) => match JsonPath::parse(p) {
                Ok(jp) => {
                    let n = export.map_or(0, |e| jp.count(&e.model));
                    self.verdict(Subject::Artifact(kind), &path, p, n, pred);
                }
                Err(_) => self.limited(
                    Subject::Output("ruleset"),
                    p,
                    format!("unsupported JSONPath `{p}`"),
                ),
            },
            // Join: select pages (final HTML or IR), test membership in the export.
            Selector::Html(s) => {
                if self.set.availability(ArtifactKind::Html) == Availability::Unavailable {
                    return self.limited_unavailable(ArtifactKind::Html);
                }
                let Ok(css) = CssSelector::parse(s) else {
                    return self.limited(
                        Subject::Output("ruleset"),
                        s,
                        format!("unsupported selector `{s}`"),
                    );
                };
                let (set, rule) = (self.set, self.rule);
                for page in set
                    .pages
                    .iter()
                    .filter(|p| in_scope(set, rule, p) && !p.doc.select(&css).is_empty())
                {
                    let n = export.is_some_and(|e| self.export_lists(e, page)) as u64;
                    let what = format!("page matching `{s}` listed in {}", kind_name(kind));
                    self.verdict(Subject::Page(page), &page.route, &what, n, pred);
                }
            }
            Selector::IrNode { kind: nk, field } => {
                let set = self.set;
                let Some(ir) = set.ir_graph() else {
                    return self.limited_unavailable(ArtifactKind::Ir);
                };
                let by_entity = matches!(kind, ArtifactKind::Rag | ArtifactKind::ContextBundle);
                for node in ir
                    .nodes
                    .iter()
                    .filter(|n| n.kind_matches(nk) && n.field_matches(field.as_deref()))
                {
                    let page = set.page_for_entity(&node.id);
                    if page.is_some_and(|p| !self.in_scope(p)) {
                        continue;
                    }
                    let what = format!(
                        "IR {nk} {} listed in {}",
                        field.as_deref().unwrap_or("*"),
                        kind_name(kind)
                    );
                    let n = match (export, page) {
                        (None, _) => 0,
                        (Some(e), _) if by_entity => e
                            .entries
                            .iter()
                            .any(|x| x.entity_id.as_deref() == Some(&node.id))
                            as u64,
                        (Some(e), Some(p)) => self.export_lists(e, p) as u64,
                        (Some(_), None) => {
                            let why = format!(
                                "no emitted route for `{}` to match against {}",
                                node.id,
                                kind_name(kind)
                            );
                            self.limited(Subject::Entity(&node.id), &node.id, why);
                            continue;
                        }
                    };
                    self.verdict(Subject::Entity(&node.id), &node.id, &what, n, pred);
                }
            }
        }
    }

    fn base_path(&self) -> Option<String> {
        self.set
            .deploy()
            .map(|d| url_path(&normalized_base(&d.base_url)).to_string())
    }

    fn export_lists(&self, e: &Export, page: &HtmlPage) -> bool {
        if matches!(e.kind, ArtifactKind::Rag | ArtifactKind::ContextBundle) {
            return page
                .entity_id
                .as_deref()
                .is_some_and(|id| e.entries.iter().any(|x| x.entity_id.as_deref() == Some(id)));
        }
        let base = self.base_path();
        e.entries
            .iter()
            .filter_map(|x| x.url.as_deref())
            .any(|u| route_matches(u, base.as_deref(), &page.route))
    }

    // ---- parity -----------------------------------------------------------

    fn parity_canonical(&mut self) {
        let Some(deploy) = self.set.deploy().cloned() else {
            let why = "TargetDeployProfile unavailable; canonical parity needs the deploy base"
                .to_string();
            return self.limited(Subject::Output("deploy-profile"), "canonical", why);
        };
        if self.set.availability(ArtifactKind::Html) == Availability::Unavailable {
            return self.limited_unavailable(ArtifactKind::Html);
        }
        let base = normalized_base(&deploy.base_url);
        let origin = url_origin(&base).to_string();
        let policy = deploy.trailing_slash;
        let set = self.set;
        let fail = |ctx: &mut Self,
                    subject: Subject<'_>,
                    target: &str,
                    msg: String,
                    found: &str,
                    want: &str| {
            ctx.emit(
                CheckStatus::Failed,
                subject,
                target,
                msg,
                json!({ "found": found, "expected": want }),
            );
        };
        for page in &set.pages {
            let want = format!("{base}/{}", percent_encode(&page.route));
            for c in page.canonicals() {
                if c != want {
                    fail(
                        self,
                        Subject::Page(page),
                        &format!("canonical:{c}"),
                        format!("canonical `{c}` ≠ deploy URL `{want}`"),
                        c,
                        &want,
                    );
                }
            }
            for href in page.attr_values("a", "href") {
                let h = strip_query(href);
                if url_origin(h) == origin && h != base && !h.starts_with(&format!("{base}/")) {
                    fail(
                        self,
                        Subject::Page(page),
                        &format!("href:{href}"),
                        format!("internal href `{href}` is outside deploy base `{base}`"),
                        href,
                        &base,
                    );
                }
            }
            for c in page.canonicals().into_iter().chain(
                page.attr_values("a", "href")
                    .into_iter()
                    .filter(|h| url_origin(h) == origin),
            ) {
                if let Some(msg) = slash_violation(c, policy, &base) {
                    fail(
                        self,
                        Subject::Page(page),
                        &format!("slash:{c}"),
                        msg,
                        c,
                        &format!("{policy:?}"),
                    );
                }
            }
        }
        for kind in [ArtifactKind::Sitemap, ArtifactKind::Rss, ArtifactKind::Llms] {
            match set.availability(kind) {
                Availability::NotEmitted => continue,
                Availability::Unavailable => {
                    self.limited_unavailable(kind);
                    continue;
                }
                Availability::Present => {}
            }
            let Some(e) = set.export(kind) else { continue };
            let entries = e.entries.iter().filter_map(|x| x.url.as_deref());
            let channel = e.model.get("channel_link").and_then(Value::as_str);
            for (u, is_entry) in entries
                .map(|u| (u, true))
                .chain(channel.map(|u| (u, false)))
            {
                let absolute = u.contains("://");
                if !absolute {
                    if kind == ArtifactKind::Llms {
                        continue; // llms.txt links are route-relative by contract.
                    }
                    fail(
                        self,
                        Subject::Artifact(kind),
                        u,
                        format!("{} URL `{u}` is not absolute", kind_name(kind)),
                        u,
                        &base,
                    );
                    continue;
                }
                let path = strip_query(u);
                if path != base
                    && path != format!("{base}/")
                    && !path.starts_with(&format!("{base}/"))
                {
                    fail(
                        self,
                        Subject::Artifact(kind),
                        u,
                        format!(
                            "{} URL `{u}` is outside deploy base `{base}`",
                            kind_name(kind)
                        ),
                        u,
                        &base,
                    );
                    continue;
                }
                if let Some(page) = set
                    .pages
                    .iter()
                    .filter(|_| is_entry)
                    .find(|p| route_matches(u, Some(url_path(&base)), &p.route))
                {
                    for c in page.canonicals() {
                        if c != path {
                            let msg = format!(
                                "{} URL `{u}` disagrees with page canonical `{c}`",
                                kind_name(kind)
                            );
                            fail(self, Subject::Artifact(kind), u, msg, u, c);
                        }
                    }
                }
                if let Some(msg) = slash_violation(u, policy, &base) {
                    fail(
                        self,
                        Subject::Artifact(kind),
                        &format!("slash:{u}"),
                        msg,
                        u,
                        &format!("{policy:?}"),
                    );
                }
            }
        }
    }

    fn parity_pageset(&mut self) {
        if self.set.availability(ArtifactKind::Html) == Availability::Unavailable {
            return self.limited_unavailable(ArtifactKind::Html);
        }
        let set = self.set;
        let base = self.base_path();
        let routes: Vec<&str> = set.pages.iter().map(|p| p.route.as_str()).collect();
        // Leg 1: every advertised page is in the sitemap.
        match (set.availability(ArtifactKind::Sitemap), set.ir_graph()) {
            (Availability::Unavailable, _) => self.limited_unavailable(ArtifactKind::Sitemap),
            (Availability::NotEmitted, _) => {}
            (Availability::Present, None) => {
                let why = "IR unavailable; cannot tell drafts from advertised pages".to_string();
                self.limited(Subject::Output("sitemap"), "advertised-set", why);
            }
            (Availability::Present, Some(ir)) => {
                let sm = set.export(ArtifactKind::Sitemap).expect("present");
                for page in &set.pages {
                    let draft = page
                        .entity_id
                        .as_deref()
                        .and_then(|id| ir.node(id))
                        .is_none_or(|n| n.is_draft());
                    if draft || page.is_noindex() || self.export_lists(sm, page) {
                        continue;
                    }
                    self.emit(
                        CheckStatus::Failed,
                        Subject::Page(page),
                        &format!("sitemap-missing:{}", page.route),
                        format!(
                            "advertised page `{}` is absent from {}",
                            page.route, sm.path
                        ),
                        json!({ "route": page.route }),
                    );
                }
            }
        }
        // Leg 2: every export entry resolves to an emitted page.
        for kind in [ArtifactKind::Sitemap, ArtifactKind::Rss, ArtifactKind::Llms] {
            match set.availability(kind) {
                Availability::NotEmitted => continue,
                Availability::Unavailable => {
                    if kind != ArtifactKind::Sitemap {
                        self.limited_unavailable(kind);
                    }
                    continue;
                }
                Availability::Present => {}
            }
            let e = set.export(kind).expect("present");
            for u in e.entries.iter().filter_map(|x| x.url.as_deref()) {
                if !routes.iter().any(|r| route_matches(u, base.as_deref(), r)) {
                    self.emit(
                        CheckStatus::Failed,
                        Subject::Artifact(kind),
                        &format!("dangling:{u}"),
                        format!(
                            "{} entry `{u}` resolves to no emitted page",
                            kind_name(kind)
                        ),
                        json!({ "url": u }),
                    );
                }
            }
        }
        // Leg 3: internal page hrefs resolve to emitted pages.
        let origin = set
            .deploy()
            .map(|d| url_origin(&normalized_base(&d.base_url)).to_string());
        for page in &set.pages {
            for href in page.attr_values("a", "href") {
                let route =
                    match internal_route(href, &page.route, origin.as_deref(), base.as_deref()) {
                        None => continue,
                        Some(Link::Page(route)) => route,
                        Some(Link::OutsideBase) => {
                            self.emit(
                                CheckStatus::Failed,
                                Subject::Page(page),
                                &format!("href:{href}"),
                                format!(
                                    "internal link `{href}` leaves deploy base path `{}`",
                                    base.as_deref().unwrap_or("/")
                                ),
                                json!({ "href": href }),
                            );
                            continue;
                        }
                    };
                if !routes.contains(&route.as_str()) {
                    self.emit(
                        CheckStatus::Failed,
                        Subject::Page(page),
                        &format!("href:{href}"),
                        format!(
                            "internal link `{href}` resolves to `{route}`, which is not emitted"
                        ),
                        json!({ "href": href, "resolved": route }),
                    );
                }
            }
        }
    }

    fn parity_lastmod(&mut self) {
        let set = self.set;
        match set.availability(ArtifactKind::Sitemap) {
            Availability::NotEmitted => return,
            Availability::Unavailable => return self.limited_unavailable(ArtifactKind::Sitemap),
            Availability::Present => {}
        }
        if set.availability(ArtifactKind::Html) == Availability::Unavailable {
            return self.limited_unavailable(ArtifactKind::Html);
        }
        let base = self.base_path();
        let sm = set.export(ArtifactKind::Sitemap).expect("present");
        for entry in &sm.entries {
            let (Some(u), Some(lm)) = (entry.url.as_deref(), entry.lastmod.as_deref()) else {
                continue;
            };
            let Some(page) = set
                .pages
                .iter()
                .find(|p| route_matches(u, base.as_deref(), &p.route))
            else {
                continue;
            };
            let Some(modified) = page.meta("property", "article:modified_time") else {
                continue;
            };
            if date_part(lm) != date_part(modified) {
                self.emit(
                    CheckStatus::Failed,
                    Subject::Page(page),
                    &format!("lastmod:{u}"),
                    format!("sitemap lastmod `{lm}` ≠ article:modified_time `{modified}`"),
                    json!({ "sitemap": lm, "html": modified }),
                );
            }
        }
    }
}

/// `archetype_scope`: a rule applies to a page only if it targets `Any` or
/// the page's archetype (explicit, else IR role). Narrative `Article`
/// pages therefore never reach answer-first rules scoped to `Faq`/`Satellite`.
fn in_scope(set: &ArtifactSet, rule: &Rule, page: &HtmlPage) -> bool {
    let a = &rule.archetypes;
    a.contains(&Archetype::Any) || set.page_archetype(page).is_some_and(|p| a.contains(&p))
}

fn date_part(s: &str) -> &str {
    s.get(..10).unwrap_or(s)
}

fn strip_query(u: &str) -> &str {
    u.split(['?', '#']).next().unwrap_or(u)
}

/// `scheme://host[:port]`, or `""` for non-absolute URLs.
fn url_origin(u: &str) -> &str {
    let Some(p) = u.find("://") else { return "" };
    let rest = &u[p + 3..];
    let end = rest.find(['/', '?', '#']).map_or(u.len(), |i| p + 3 + i);
    &u[..end]
}

/// The path part of an absolute URL (`""` for a bare origin), or `u` itself.
fn url_path(u: &str) -> &str {
    let o = url_origin(u);
    if o.is_empty() { u } else { &u[o.len()..] }
}

fn normalized_base(b: &str) -> String {
    b.trim().trim_end_matches('/').to_string()
}

fn percent_encode(route: &str) -> String {
    let mut s = String::with_capacity(route.len());
    for b in route.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |k: usize| b.get(k).and_then(|&c| (c as char).to_digit(16));
        if b[i] == b'%'
            && let (Some(hi), Some(lo)) = (hex(i + 1), hex(i + 2))
        {
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Does export URL `u` name emitted `route`? With a known deploy base path
/// the match is exact; without one, a path-suffix match is used.
fn route_matches(u: &str, base_path: Option<&str>, route: &str) -> bool {
    let path = percent_decode(strip_query(url_path(u)));
    match base_path {
        Some(bp) => {
            let bp = percent_decode(bp);
            let rel = path
                .strip_prefix(bp.as_str())
                .filter(|r| r.starts_with('/') || r.is_empty())
                .unwrap_or(&path);
            let rel = rel.trim_start_matches('/');
            rel == route
                || (rel.is_empty() || rel.ends_with('/')) && format!("{rel}index.html") == route
        }
        None => {
            let p = path.trim_start_matches('/');
            p == route || p.ends_with(&format!("/{route}"))
        }
    }
}

/// Where an internal `<a href>` points.
#[derive(Debug, PartialEq, Eq)]
enum Link {
    Page(String),
    /// A relative or root-relative link that leaves the deploy base path
    /// (the absolute same-origin form is canonical parity's finding).
    OutsideBase,
}

/// Resolve an `<a href>` on `from_route`, if it is an internal page link
/// (`.html`, same origin, scheme-relative or relative).
fn internal_route(
    href: &str,
    from_route: &str,
    origin: Option<&str>,
    base_path: Option<&str>,
) -> Option<Link> {
    let h = strip_query(href.trim());
    let scheme_relative;
    let h = match h.strip_prefix("//") {
        Some(rest) => {
            let scheme = origin
                .and_then(|o| o.split_once("://"))
                .map_or("https", |(s, _)| s);
            scheme_relative = format!("{scheme}://{rest}");
            scheme_relative.as_str()
        }
        None => h,
    };
    if h.is_empty() || h.contains(':') && !h.contains("://") {
        return None; // mailto:, tel:, javascript:, data:
    }
    if !h.ends_with(".html") {
        return None;
    }
    let absolute = h.contains("://");
    let path = if absolute {
        if Some(url_origin(h)) != origin {
            return None;
        }
        url_path(h).to_string()
    } else {
        h.to_string()
    };
    let path = percent_decode(&path);
    let bp = percent_decode(base_path.unwrap_or(""));
    let bp = bp.trim_matches('/');
    let outside = || (!absolute).then_some(Link::OutsideBase);
    let joined = if let Some(abs) = path.strip_prefix('/') {
        if bp.is_empty() {
            abs.to_string()
        } else {
            match abs
                .strip_prefix(bp)
                .filter(|r| r.is_empty() || r.starts_with('/'))
            {
                Some(r) => r.trim_start_matches('/').to_string(),
                None => return outside(),
            }
        }
    } else {
        match from_route.rfind('/') {
            Some(i) => format!("{}/{path}", &from_route[..i]),
            None => path,
        }
    };
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            // Above the site root: browsers clamp at `/`, which is only
            // still inside the deploy when the deploy is the origin root.
            ".." => {
                if parts.pop().is_none() && !bp.is_empty() {
                    return outside();
                }
            }
            s => parts.push(s),
        }
    }
    Some(Link::Page(parts.join("/")))
}

fn slash_violation(u: &str, policy: SlashPolicy, base: &str) -> Option<String> {
    let path = strip_query(url_path(u));
    let last = path.rsplit('/').next().unwrap_or("");
    if last.contains('.') || strip_query(u) == base || path.is_empty() || path == "/" {
        return None;
    }
    match policy {
        SlashPolicy::Always if !path.ends_with('/') => Some(format!(
            "`{u}` lacks the trailing slash the deploy profile requires"
        )),
        SlashPolicy::Never if path.ends_with('/') => Some(format!(
            "`{u}` has a trailing slash the deploy profile forbids"
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(h: &str, from: &str, origin: Option<&str>, bp: Option<&str>) -> Option<String> {
        internal_route(h, from, origin, bp).map(|l| match l {
            Link::Page(p) => p,
            Link::OutsideBase => "<outside>".into(),
        })
    }

    #[test]
    fn url_helpers() {
        assert_eq!(
            url_origin("https://a.example/x/y.html"),
            "https://a.example"
        );
        assert_eq!(url_path("https://a.example"), "");
        assert!(route_matches(
            "https://a.example/p/x.html",
            Some("/p"),
            "x.html"
        ));
        assert!(!route_matches(
            "https://a.example/x.html",
            Some("/p"),
            "y.html"
        ));
        assert!(route_matches("/a%20b.html", Some(""), "a b.html"));
        assert!(route_matches(
            "https://a.example/p/",
            Some("/p"),
            "index.html"
        ));
        assert_eq!(
            route("../b.html#x", "g/a.html", None, None).as_deref(),
            Some("b.html")
        );
        assert_eq!(
            route("/p/b.html", "a.html", None, Some("/p")).as_deref(),
            Some("b.html")
        );
        assert_eq!(route("mailto:x@y", "a.html", None, None), None);
        assert_eq!(
            route(
                "https://other.example/b.html",
                "a.html",
                Some("https://a.example"),
                None
            ),
            None
        );
        assert_eq!(percent_encode("a b/c.html"), "a%20b/c.html");
        let o = Some("https://a.example");
        assert_eq!(route("//cdn.example/x.html", "a.html", o, None), None);
        assert_eq!(
            route("//a.example/b.html", "a.html", o, None).as_deref(),
            Some("b.html")
        );
        assert_eq!(
            route("../x.html", "a.html", None, None).as_deref(),
            Some("x.html")
        );
        for h in ["../x.html", "/other/x.html", "/pp/x.html"] {
            assert_eq!(
                route(h, "a.html", o, Some("/p")).as_deref(),
                Some("<outside>"),
                "{h}"
            );
        }
        assert_eq!(
            route("https://a.example/other/x.html", "a.html", o, Some("/p")),
            None
        );
        assert_eq!(
            route("/my%20docs/b.html", "a.html", o, Some("/my%20docs")).as_deref(),
            Some("b.html")
        );
        assert!(route_matches(
            "https://a.example/my%20docs/b.html",
            Some("/my%20docs"),
            "b.html"
        ));
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("a%\u{e9}x"), "a%\u{e9}x");
        assert_eq!(percent_decode("%2"), "%2");
    }
}
