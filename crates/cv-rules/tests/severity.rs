//! Findings carry the severity of the rule that produced them, and only a
//! failed `Error` at a blocking tier can block a publish.

use cv_core::id::AccountId;
use cv_core::rules::{AuthorityTier, Rule, Severity, SlashPolicy, TargetDeployProfile};
use cv_rules::{ArtifactSet, Export, HtmlPage, PackMeta, Report, RulePack, Ruleset, eval};

const BASE: &str = "https://docs.example";

/// One page whose `article:modified_time` disagrees with its sitemap
/// `<lastmod>`, so `TGT-LASTMOD-PARITY` fails.
fn lastmod_mismatch() -> ArtifactSet {
    let html = format!(
        r#"<!doctype html><html lang="en"><head><title>Docs</title>
<link rel="canonical" href="{BASE}/index.html">
<meta property="article:modified_time" content="2026-01-01T00:00:00Z"></head>
<body><main><h1>Docs</h1></main></body></html>"#
    );
    let sitemap = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc>{BASE}/index.html</loc><lastmod>2026-02-02</lastmod></url></urlset>"#
    );
    ArtifactSet::builder()
        .account(AccountId("acct".into()))
        .deploy(TargetDeployProfile {
            base_url: format!("{BASE}/"),
            trailing_slash: SlashPolicy::Either,
            expects_robots_txt: false,
        })
        .page(HtmlPage::parse(Some("index"), "index.html", html.as_bytes()).unwrap())
        .export(Export::sitemap_xml("sitemap.xml", sitemap.as_bytes()).unwrap())
        .build()
}

/// The shipped `TGT-LASTMOD-PARITY` rule, re-tiered and re-graded.
fn lastmod_rule(tier: AuthorityTier, severity: Severity) -> Ruleset {
    let shipped = Ruleset::v1()
        .rule("TGT-LASTMOD-PARITY")
        .map(|(_, r)| r.clone())
        .expect("TGT-LASTMOD-PARITY ships in v1");
    let rule = Rule {
        tier,
        severity,
        ..shipped
    };
    Ruleset::new(vec![RulePack {
        meta: PackMeta {
            id: "targets".into(),
            version: 1,
            description: "TGT-LASTMOD-PARITY only".into(),
        },
        rules: vec![rule],
    }])
    .unwrap()
}

fn failed_lastmod(tier: AuthorityTier, severity: Severity) -> Report {
    let report = eval(&lastmod_mismatch(), &lastmod_rule(tier, severity));
    let failed: Vec<_> = report.failed().collect();
    assert_eq!(failed.len(), 1, "{:#?}", report.findings);
    assert_eq!(failed[0].finding.code, "TGT-LASTMOD-PARITY");
    assert_eq!(failed[0].finding.tier, tier);
    assert_eq!(failed[0].finding.severity(), severity);
    report
}

fn assert_blocks(report: &Report, blocks: bool) {
    assert_eq!(report.blocks_publish(), blocks);
    assert_eq!(report.verdict().blocks_publish(), blocks);
    assert_eq!(report.to_core().has_blocking_errors(), blocks);
}

#[test]
fn every_v1_finding_carries_its_rules_severity_and_tier() {
    let rules = Ruleset::v1();
    let mut seen = Vec::new();
    for set in [ArtifactSet::builder().build(), lastmod_mismatch()] {
        for f in eval(&set, &rules).findings {
            let (_, rule) = rules
                .rule(&f.finding.code)
                .expect("finding names a v1 rule");
            assert_eq!(f.finding.severity(), rule.severity, "{}", rule.id);
            assert_eq!(f.finding.tier, rule.tier, "{}", rule.id);
            if !seen.contains(&f.finding.severity()) {
                seen.push(f.finding.severity());
            }
        }
    }
    assert_eq!(seen.len(), 3, "v1 should exercise Error, Warning and Info");
}

#[test]
fn shipped_lastmod_parity_failure_is_a_warning_that_does_not_block() {
    let report = eval(&lastmod_mismatch(), &Ruleset::v1());
    let lastmod: Vec<_> = report
        .failed()
        .filter(|f| f.finding.code == "TGT-LASTMOD-PARITY")
        .collect();
    assert_eq!(lastmod.len(), 1);
    assert_eq!(lastmod[0].finding.severity(), Severity::Warning);
    assert!(!lastmod[0].blocks_publish());
    assert!(!lastmod[0].finding.blocks_publish());
}

#[test]
fn shipped_live_robots_served_finding_is_a_normative_warning() {
    let report = eval(&lastmod_mismatch(), &Ruleset::v1());
    let served: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.finding.code == "TGT-LIVE-ROBOTS-SERVED")
        .collect();
    assert_eq!(served.len(), 1);
    assert_eq!(served[0].finding.tier, AuthorityTier::NormativeSpec);
    assert_eq!(served[0].finding.severity(), Severity::Warning);
    assert!(!served[0].blocks_publish());
}

#[test]
fn failed_engine_documented_warning_does_not_block() {
    let report = failed_lastmod(AuthorityTier::EngineDocumented, Severity::Warning);
    assert_blocks(&report, false);
    assert_eq!(report.verdict().advisory, 1);
}

#[test]
fn failed_normative_spec_info_does_not_block() {
    assert_blocks(
        &failed_lastmod(AuthorityTier::NormativeSpec, Severity::Info),
        false,
    );
}

#[test]
fn failed_normative_spec_error_blocks() {
    let report = failed_lastmod(AuthorityTier::NormativeSpec, Severity::Error);
    assert_blocks(&report, true);
    assert_eq!(report.verdict().blocking, 1);
}

#[test]
fn failed_engine_documented_error_blocks() {
    assert_blocks(
        &failed_lastmod(AuthorityTier::EngineDocumented, Severity::Error),
        true,
    );
}

#[test]
fn failed_heuristic_error_never_blocks() {
    let report = failed_lastmod(AuthorityTier::Heuristic, Severity::Error);
    assert_blocks(&report, false);
    assert_eq!(report.verdict().advisory, 1);
}

#[test]
fn severity_survives_the_core_report_round_trip() {
    let core = failed_lastmod(AuthorityTier::EngineDocumented, Severity::Warning).to_core();
    let back: cv_core::rules::Report =
        serde_json::from_slice(&serde_json::to_vec(&core).unwrap()).unwrap();
    assert_eq!(back, core);
    assert!(
        back.findings
            .iter()
            .all(|f| f.severity() == Severity::Warning)
    );
    assert!(!back.has_blocking_errors());
}
