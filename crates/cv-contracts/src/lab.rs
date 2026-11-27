//! `boris-migration-lab` output mirrors, keyed by `(format, schema_version)`.
//!
//! Sources: the lab's `docs/contracts/*.md` and README "Schema note" at the
//! pinned commit (`ce40d78`, `boris-migration-lab/0.8.1`), and the captures
//! under `fixtures/captures/lab/`. Only fields present in those captures
//! are typed; the rest of each object stays in `extra`.
//!
//! The lab bumps `schema_version` when a field's meaning or a required
//! shape changes, so new fields under a known version decode, while an
//! unknown `format` or version is a [`DecodeError`], never a guess.
//!
//! One format id names a whole family: `boris-hugo-migration-lab` is the
//! `format` of the Hugo `report.json`, `inventory.json`, `redirects.json`
//! and every `provenance/<id>.json`. The file's role inside `--out` picks
//! the mirror ([`Document::decode`]); the key picks the family and version.

use crate::FormatKey;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

type Extra = serde_json::Map<String, serde_json::Value>;

/// Format ids, spelled as the lab writes them.
pub mod format {
    pub const HUGO: &str = "boris-hugo-migration-lab";
    pub const JEKYLL: &str = "boris-jekyll-migration-lab";
    pub const SUBSTACK: &str = "boris-substack-migration-lab";
    pub const ENEX: &str = "boris-enex-migration-lab";
    pub const OBSIDIAN: &str = "boris-obsidian-migration-lab";
    pub const WORDPRESS: &str = "boris-wordpress-migration-lab";
    pub const OBSIDIAN_ATTACHMENTS: &str = "boris-obsidian-attachments-manifest";
    pub const WORDPRESS_MEDIA: &str = "boris-wordpress-media-manifest";
    pub const LINK_AUDIT: &str = "boris-link-audit-lab";
    pub const FRONTMATTER_REVIEW: &str = "boris-frontmatter-review-lab";
    pub const ASSET_FILENAME: &str = "boris-asset-filename-lab";
    pub const ASSET_FILENAME_REWRITES: &str = "boris-asset-filename-rewrites";
}

const fn key(format: &'static str, schema_version: u32) -> FormatKey {
    FormatKey {
        format,
        schema_version,
    }
}

/// `report.json`. WordPress is at schema 3; the others at 1.
pub const REPORTS: &[FormatKey] = &[
    key(format::HUGO, 1),
    key(format::JEKYLL, 1),
    key(format::SUBSTACK, 1),
    key(format::ENEX, 1),
    key(format::OBSIDIAN, 1),
    key(format::WORDPRESS, 3),
    key(format::ASSET_FILENAME, 1),
];
/// `inventory.json` (inventory modes, and the Substack and ENEX emit modes).
pub const INVENTORIES: &[FormatKey] = &[
    key(format::HUGO, 1),
    key(format::JEKYLL, 1),
    key(format::SUBSTACK, 1),
    key(format::ENEX, 1),
];
/// `provenance/<entity id>.json`, one file per emitted page.
pub const PAGE_PROVENANCE: &[FormatKey] = &[key(format::HUGO, 1), key(format::JEKYLL, 1)];
/// `provenance_manifest.json`, one file per run.
pub const PROVENANCE_MANIFESTS: &[FormatKey] = &[key(format::SUBSTACK, 1), key(format::ENEX, 1)];
pub const REDIRECTS: &[FormatKey] = &[key(format::HUGO, 1)];
pub const MEDIA_MANIFESTS: &[FormatKey] = &[key(format::WORDPRESS_MEDIA, 1)];
pub const ATTACHMENTS_MANIFESTS: &[FormatKey] = &[key(format::OBSIDIAN_ATTACHMENTS, 1)];
pub const LINK_AUDITS: &[FormatKey] = &[key(format::LINK_AUDIT, 1)];
pub const FRONTMATTER_REVIEWS: &[FormatKey] = &[key(format::FRONTMATTER_REVIEW, 2)];
pub const ASSET_FILENAME_MANIFESTS: &[FormatKey] = &[key(format::ASSET_FILENAME, 1)];
pub const ASSET_FILENAME_REWRITE_MANIFESTS: &[FormatKey] =
    &[key(format::ASSET_FILENAME_REWRITES, 1)];

/// Why a lab document did not decode.
#[derive(Debug)]
pub enum DecodeError {
    /// Not JSON, or a typed field has the wrong shape.
    Json(serde_json::Error),
    /// A file name inside `--out` that no mirror covers.
    UnknownDocument(String),
    /// `format` is not mirrored for this document role.
    UnknownFormat {
        document: &'static str,
        format: String,
    },
    /// A mirrored format at a `schema_version` this crate does not know.
    UnsupportedSchemaVersion {
        document: &'static str,
        format: String,
        found: u64,
        supported: Vec<u32>,
    },
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => write!(f, "lab document is not valid: {e}"),
            Self::UnknownDocument(rel) => write!(f, "{rel:?} is not a mirrored lab document"),
            Self::UnknownFormat { document, format } => {
                write!(f, "{document} format {format:?} is not mirrored")
            }
            Self::UnsupportedSchemaVersion {
                document,
                format,
                found,
                supported,
            } => write!(
                f,
                "{document} {format} schema_version {found} is not supported (known: {supported:?})"
            ),
        }
    }
}

impl std::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for DecodeError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

#[derive(Deserialize)]
struct Header {
    format: String,
    schema_version: u64,
}

/// Read `format` + `schema_version`, find them in `accepted`, then decode.
fn decode_keyed<T: DeserializeOwned>(
    document: &'static str,
    accepted: &[FormatKey],
    bytes: &[u8],
) -> Result<(FormatKey, T), DecodeError> {
    let h: Header = serde_json::from_slice(bytes)?;
    let family: Vec<&FormatKey> = accepted.iter().filter(|k| k.format == h.format).collect();
    if family.is_empty() {
        return Err(DecodeError::UnknownFormat {
            document,
            format: h.format,
        });
    }
    let Some(k) = family
        .iter()
        .find(|k| u64::from(k.schema_version) == h.schema_version)
    else {
        return Err(DecodeError::UnsupportedSchemaVersion {
            document,
            format: h.format,
            found: h.schema_version,
            supported: family.iter().map(|k| k.schema_version).collect(),
        });
    };
    Ok((**k, serde_json::from_slice(bytes)?))
}

/// The lab's per-construct and per-page conversion class. The lab ranks
/// them `exact` < `transformed` < `unsupported` < `human_review` and
/// reports the worst per page; that ranking is the lab's, not ours.
/// A value outside the four is kept as `Unknown`, so drift is shown
/// rather than mapped onto a known class.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Disposition {
    Exact,
    Transformed,
    Unsupported,
    HumanReview,
    Unknown(String),
}

impl Disposition {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Exact => "exact",
            Self::Transformed => "transformed",
            Self::Unsupported => "unsupported",
            Self::HumanReview => "human_review",
            Self::Unknown(s) => s,
        }
    }

    fn from_name(s: String) -> Self {
        match s.as_str() {
            "exact" => Self::Exact,
            "transformed" => Self::Transformed,
            "unsupported" => Self::Unsupported,
            "human_review" => Self::HumanReview,
            _ => Self::Unknown(s),
        }
    }
}

impl Serialize for Disposition {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Disposition {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(Self::from_name)
    }
}

/// Frontmatter-key disposition (`migration_semantics.zig`
/// `FieldDisposition`), used by `frontmatter-review`. Unlisted values are
/// kept as `Unknown`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FieldDisposition {
    SupportedDirect,
    SupportedNormalized,
    IdentitySource,
    ParentCandidate,
    RelationCandidate,
    BodyCandidate,
    SidecarOnly,
    PlatformResidue,
    ManualReview,
    Unknown(String),
}

impl FieldDisposition {
    const NAMES: [(&'static str, Self); 9] = [
        ("supported_direct", Self::SupportedDirect),
        ("supported_normalized", Self::SupportedNormalized),
        ("identity_source", Self::IdentitySource),
        ("parent_candidate", Self::ParentCandidate),
        ("relation_candidate", Self::RelationCandidate),
        ("body_candidate", Self::BodyCandidate),
        ("sidecar_only", Self::SidecarOnly),
        ("platform_residue", Self::PlatformResidue),
        ("manual_review", Self::ManualReview),
    ];

    pub fn as_str(&self) -> &str {
        match self {
            Self::Unknown(s) => s,
            known => {
                Self::NAMES
                    .iter()
                    .find(|(_, v)| v == known)
                    .expect("every known variant is named")
                    .0
            }
        }
    }
}

impl Serialize for FieldDisposition {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for FieldDisposition {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Self::NAMES
            .iter()
            .find(|(n, _)| *n == s)
            .map(|(_, v)| v.clone())
            .unwrap_or(Self::Unknown(s)))
    }
}

/// Per-class totals (`summary.by_class`, `class_counts`, Jekyll `summary`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassCounts {
    pub exact: u64,
    pub transformed: u64,
    pub unsupported: u64,
    pub human_review: u64,
    #[serde(flatten)]
    pub extra: Extra,
}

/// One row of a `dispositions` table (Substack, ENEX).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DispositionCount {
    pub construct: String,
    pub class: Disposition,
    pub count: u64,
    #[serde(flatten)]
    pub extra: Extra,
}

/// One `findings` row (Substack, ENEX). `entity_id` is empty for
/// run-level findings such as `orphan_post_html`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub entity_id: String,
    pub code: String,
    pub class: Disposition,
    pub detail: String,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `source: { "root": … }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRoot {
    pub root: String,
    #[serde(flatten)]
    pub extra: Extra,
}

/// Substack `source`: the export path, its sha256, and how many PII
/// files the lab skipped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubstackSource {
    pub path: String,
    pub sha256: String,
    pub ignored_pii_files: u64,
    #[serde(flatten)]
    pub extra: Extra,
}

/// One emitted (or, for Substack, considered) page, projected the same way
/// for every family. Field names differ by family; nothing is derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRow<'a> {
    pub entity_id: &'a str,
    /// Path under `--out`, e.g. `content/posts/hello.md`. `None` for a
    /// Substack row the lab did not emit.
    pub output: Option<&'a str>,
    /// Source path relative to the input root, when the family names one.
    pub source: Option<&'a str>,
    pub disposition: &'a Disposition,
}

// ---- report.json ---------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HugoReport {
    pub format: String,
    pub schema_version: u32,
    pub tool_version: String,
    pub mode: String,
    pub source_root: String,
    pub summary: HugoSummary,
    pub pages: Vec<HugoPage>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HugoSummary {
    pub pages_emitted: u64,
    pub by_class: ClassCounts,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HugoPage {
    pub entity_id: String,
    pub output: String,
    pub source: Option<String>,
    pub kind: String,
    pub language: String,
    pub role: String,
    pub parent: Option<String>,
    pub class: Disposition,
    pub dropped_keys: Vec<String>,
    pub notes: Vec<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JekyllReport {
    pub format: String,
    pub schema_version: u32,
    pub mode: String,
    pub tool_version: String,
    pub source: SourceRoot,
    pub summary: JekyllSummary,
    pub pages: Vec<JekyllPage>,
    pub not_emitted: Vec<NotEmitted>,
    pub constructs: Vec<JekyllConstruct>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JekyllSummary {
    pub pages: u64,
    pub not_emitted: u64,
    #[serde(flatten)]
    pub by_class: ClassCounts,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JekyllPage {
    pub entity_id: String,
    pub output: String,
    /// `None` for pages the lab generated (trunks).
    pub source_path: Option<String>,
    pub kind: String,
    pub parent: Option<String>,
    pub status: String,
    pub published_at: Option<String>,
    pub class: Disposition,
    pub review: Vec<String>,
    /// `provenance/<entity id>.json`, relative to `--out`.
    pub provenance: String,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotEmitted {
    pub path: String,
    pub class: Disposition,
    pub reason: String,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JekyllConstruct {
    pub construct: String,
    pub class: Disposition,
    pub count: u64,
    pub landing: String,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubstackReport {
    pub format: String,
    pub schema_version: u32,
    pub mode: String,
    pub tool_version: String,
    pub source: SubstackSource,
    pub dispositions: Vec<DispositionCount>,
    pub pages: Vec<SubstackPage>,
    pub findings: Vec<Finding>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubstackPage {
    pub entity_id: String,
    pub post_id: String,
    pub output: Option<String>,
    pub emitted: bool,
    pub boris_status: String,
    pub conversion: Disposition,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnexReport {
    pub format: String,
    pub schema_version: u32,
    pub mode: String,
    pub tool_version: String,
    pub source: SourceRoot,
    pub dispositions: Vec<DispositionCount>,
    pub pages: Vec<EnexPage>,
    pub findings: Vec<Finding>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnexPage {
    pub entity_id: String,
    pub parent: String,
    pub output: String,
    pub boris_status: String,
    pub conversion: Disposition,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObsidianReport {
    pub format: String,
    pub schema_version: u32,
    pub tool_version: String,
    pub source_vault: String,
    pub pages: Vec<ObsidianPage>,
    pub links: Vec<ObsidianLink>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObsidianPage {
    pub source_path: String,
    pub entity_id: String,
    pub output_path: String,
    pub title: String,
    pub conversion: Disposition,
    pub notes: Vec<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObsidianLink {
    pub source_path: String,
    pub raw: String,
    pub target: String,
    /// The lab's link resolution verdict, e.g. `resolved`.
    pub status: String,
    pub is_embed: bool,
    pub resolved_to: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WordPressReport {
    pub format: String,
    pub schema_version: u32,
    pub tool_version: String,
    pub source_export: String,
    pub media_dir: Option<String>,
    pub site_title: String,
    pub pages: Vec<WordPressPage>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WordPressPage {
    pub output_path: String,
    pub post_id: String,
    pub post_type: String,
    pub title: String,
    pub slug: String,
    pub status_boris: String,
    pub proposed_entity_id: String,
    pub proposed_parent: Option<String>,
    pub conversion: Disposition,
    pub feature_codes: Vec<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `asset-filename` `report.json` (schema 1). The mode copies a candidate
/// content tree and reports aggregate policy/counts; it deliberately has
/// no Boris page authority, so existing-workspace adoption is an
/// operation-based in-place transformation rather than a candidate copy.
/// That transformation invents no page ids the lab did not emit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetFilenameReport {
    pub format: String,
    pub schema_version: u32,
    pub tool_version: String,
    pub source_root: String,
    pub counts: AssetFilenameCounts,
    pub policy: AssetFilenamePolicy,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetFilenameCounts {
    pub assets: u64,
    pub unchanged: u64,
    pub rewritten: u64,
    pub rejected: u64,
    pub markdown_rewrites: u64,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetFilenamePolicy {
    pub source_readonly: bool,
    pub remote_fetch: bool,
    pub execute_js: bool,
    pub silent_overwrite: bool,
    pub boris_core_unchanged: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

/// A `report.json`, by family.
#[derive(Debug, Clone, PartialEq)]
pub enum Report {
    Hugo(HugoReport),
    Jekyll(JekyllReport),
    Substack(SubstackReport),
    Enex(EnexReport),
    Obsidian(ObsidianReport),
    WordPress(WordPressReport),
    AssetFilename(AssetFilenameReport),
}

impl Report {
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let (k, v): (_, serde_json::Value) = decode_keyed("report.json", REPORTS, bytes)?;
        Ok(match k.format {
            format::HUGO => Self::Hugo(serde_json::from_value(v)?),
            format::JEKYLL => Self::Jekyll(serde_json::from_value(v)?),
            format::SUBSTACK => Self::Substack(serde_json::from_value(v)?),
            format::ENEX => Self::Enex(serde_json::from_value(v)?),
            format::OBSIDIAN => Self::Obsidian(serde_json::from_value(v)?),
            format::WORDPRESS => Self::WordPress(serde_json::from_value(v)?),
            format::ASSET_FILENAME => Self::AssetFilename(serde_json::from_value(v)?),
            other => unreachable!("REPORTS lists {other} without a mirror"),
        })
    }

    pub fn key(&self) -> FormatKey {
        let (f, v) = match self {
            Self::Hugo(r) => (&r.format, r.schema_version),
            Self::Jekyll(r) => (&r.format, r.schema_version),
            Self::Substack(r) => (&r.format, r.schema_version),
            Self::Enex(r) => (&r.format, r.schema_version),
            Self::Obsidian(r) => (&r.format, r.schema_version),
            Self::WordPress(r) => (&r.format, r.schema_version),
            Self::AssetFilename(r) => (&r.format, r.schema_version),
        };
        *REPORTS
            .iter()
            .find(|k| k.format == f && k.schema_version == v)
            .expect("decoded reports carry a mirrored key")
    }

    pub fn tool_version(&self) -> &str {
        match self {
            Self::Hugo(r) => &r.tool_version,
            Self::Jekyll(r) => &r.tool_version,
            Self::Substack(r) => &r.tool_version,
            Self::Enex(r) => &r.tool_version,
            Self::Obsidian(r) => &r.tool_version,
            Self::WordPress(r) => &r.tool_version,
            Self::AssetFilename(r) => &r.tool_version,
        }
    }

    /// Every page row in report order.
    pub fn pages(&self) -> Vec<PageRow<'_>> {
        match self {
            Self::Hugo(r) => r
                .pages
                .iter()
                .map(|p| PageRow {
                    entity_id: &p.entity_id,
                    output: Some(&p.output),
                    source: p.source.as_deref(),
                    disposition: &p.class,
                })
                .collect(),
            Self::Jekyll(r) => r
                .pages
                .iter()
                .map(|p| PageRow {
                    entity_id: &p.entity_id,
                    output: Some(&p.output),
                    source: p.source_path.as_deref(),
                    disposition: &p.class,
                })
                .collect(),
            Self::Substack(r) => r
                .pages
                .iter()
                .map(|p| PageRow {
                    entity_id: &p.entity_id,
                    output: p.output.as_deref(),
                    source: None,
                    disposition: &p.conversion,
                })
                .collect(),
            Self::Enex(r) => r
                .pages
                .iter()
                .map(|p| PageRow {
                    entity_id: &p.entity_id,
                    output: Some(&p.output),
                    source: None,
                    disposition: &p.conversion,
                })
                .collect(),
            Self::Obsidian(r) => r
                .pages
                .iter()
                .map(|p| PageRow {
                    entity_id: &p.entity_id,
                    output: Some(&p.output_path),
                    source: Some(&p.source_path),
                    disposition: &p.conversion,
                })
                .collect(),
            Self::WordPress(r) => r
                .pages
                .iter()
                .map(|p| PageRow {
                    entity_id: &p.proposed_entity_id,
                    output: Some(&p.output_path),
                    source: None,
                    disposition: &p.conversion,
                })
                .collect(),
            Self::AssetFilename(_) => Vec::new(),
        }
    }
}

// ---- inventory.json ------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HugoInventory {
    pub format: String,
    pub schema_version: u32,
    pub mode: String,
    pub tool_version: String,
    pub source: SourceRoot,
    pub class_counts: ClassCounts,
    pub pages: Vec<HugoInventoryPage>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HugoInventoryPage {
    pub path: String,
    pub language: String,
    pub kind: String,
    pub class: Disposition,
    pub front_matter_format: String,
    pub front_matter_keys: Vec<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// Jekyll's inventory keeps its page lists (`posts`, `drafts`, `pages`,
/// static files, …) in `extra`; the typed surface is the construct
/// disposition table every family shares.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JekyllInventory {
    pub format: String,
    pub schema_version: u32,
    pub mode: String,
    pub tool_version: String,
    pub source: SourceRoot,
    pub constructs: Vec<DispositionCount>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubstackInventory {
    pub format: String,
    pub schema_version: u32,
    pub mode: String,
    pub tool_version: String,
    pub source: SubstackSource,
    pub dispositions: Vec<DispositionCount>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnexInventory {
    pub format: String,
    pub schema_version: u32,
    pub mode: String,
    pub tool_version: String,
    pub source: SourceRoot,
    pub dispositions: Vec<DispositionCount>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// An `inventory.json`, by family.
#[derive(Debug, Clone, PartialEq)]
pub enum Inventory {
    Hugo(HugoInventory),
    Jekyll(JekyllInventory),
    Substack(SubstackInventory),
    Enex(EnexInventory),
}

impl Inventory {
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let (k, v): (_, serde_json::Value) = decode_keyed("inventory.json", INVENTORIES, bytes)?;
        Ok(match k.format {
            format::HUGO => Self::Hugo(serde_json::from_value(v)?),
            format::JEKYLL => Self::Jekyll(serde_json::from_value(v)?),
            format::SUBSTACK => Self::Substack(serde_json::from_value(v)?),
            format::ENEX => Self::Enex(serde_json::from_value(v)?),
            other => unreachable!("INVENTORIES lists {other} without a mirror"),
        })
    }

    /// The `class` of every construct or page the inventory counted.
    pub fn dispositions(&self) -> Vec<&Disposition> {
        match self {
            Self::Hugo(i) => i.pages.iter().map(|p| &p.class).collect(),
            Self::Jekyll(i) => i.constructs.iter().map(|d| &d.class).collect(),
            Self::Substack(i) => i.dispositions.iter().map(|d| &d.class).collect(),
            Self::Enex(i) => i.dispositions.iter().map(|d| &d.class).collect(),
        }
    }
}

// ---- provenance ----------------------------------------------------------

/// `provenance/<entity id>.json` (Hugo, Jekyll). The family-specific
/// evidence (Jekyll `raw_frontmatter`, Hugo `dropped_keys`, …) is in
/// `extra`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageProvenance {
    pub format: String,
    pub schema_version: u32,
    pub entity_id: String,
    pub class: Disposition,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `provenance_manifest.json` (Substack, ENEX): one entry per page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProvenanceManifest {
    pub format: String,
    pub schema_version: u32,
    pub source: serde_json::Value,
    pub pages: Vec<ProvenanceEntry>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProvenanceEntry {
    pub entity_id: String,
    pub output: Option<String>,
    pub conversion: Disposition,
    #[serde(flatten)]
    pub extra: Extra,
}

// ---- mode-specific manifests ---------------------------------------------

/// Hugo `redirects.json`: aliases and old routes, mapped to entity ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Redirects {
    pub format: String,
    pub schema_version: u32,
    pub redirects: Vec<Redirect>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Redirect {
    pub kind: String,
    pub from: String,
    pub to_entity_id: String,
    pub source: String,
    #[serde(flatten)]
    pub extra: Extra,
}

/// WordPress `media_manifest.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaManifest {
    pub format: String,
    pub schema_version: u32,
    pub policy: String,
    pub entries: Vec<MediaEntry>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaEntry {
    pub source_output: String,
    pub original_reference: String,
    pub upload_key: Option<String>,
    pub matched_source: Option<String>,
    pub emitted_asset_path: Option<String>,
    pub status: String,
    pub reason: String,
    #[serde(flatten)]
    pub extra: Extra,
}

/// Obsidian `attachments_manifest.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentsManifest {
    pub format: String,
    pub schema_version: u32,
    pub entries: Vec<Attachment>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub source_path: String,
    pub output_path: String,
    pub referenced: bool,
    pub copied: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

// ---- cross-cutting modes -------------------------------------------------

/// `link-audit` `link_audit.json`. Findings are the lab's advisory
/// evidence only; Capture Vector never promotes them to Boris authority.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkAudit {
    pub format: String,
    pub schema_version: u32,
    pub html_files: u64,
    pub findings: Vec<LinkAuditFinding>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkAuditFinding {
    pub source: String,
    pub target: String,
    pub line: u64,
    pub kind: String,
    pub reason: String,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `frontmatter-review` `frontmatter_review.json` (schema 2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrontmatterReview {
    pub format: String,
    pub schema_version: u32,
    pub tool_version: String,
    pub source_root: String,
    pub total_unknown_keys: u64,
    pub total_occurrences: u64,
    pub field_summaries: Vec<FieldSummary>,
    pub files: Vec<ReviewedFile>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldSummary {
    pub key: String,
    pub disposition: FieldDisposition,
    pub occurrence_count: u64,
    pub affected_file_count: u64,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewedFile {
    pub source_path: String,
    pub incompatible_fence: bool,
    pub unknown_keys: Vec<UnknownKey>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnknownKey {
    pub key: String,
    pub line: u64,
    pub value: String,
    pub disposition: FieldDisposition,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `asset_filename_manifest.json`: one row per discovered asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetFilenameManifest {
    pub format: String,
    pub schema_version: u32,
    pub policy: String,
    pub assets: Vec<AssetFilenameAsset>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetFilenameAsset {
    pub source_path: String,
    pub within_tree_source: String,
    pub page_stem: String,
    pub within_tree_dest: String,
    pub dest_path: String,
    pub action: String,
    pub reason: String,
    pub sha256: Option<String>,
    pub bytes: u64,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `rewrite_manifest.json`: Markdown destination rewrites in the candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetFilenameRewriteManifest {
    pub format: String,
    pub schema_version: u32,
    pub rewrites: Vec<AssetFilenameRewrite>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetFilenameRewrite {
    pub page_path: String,
    pub original_dest: String,
    pub rewritten_dest: String,
    pub reason: String,
    #[serde(flatten)]
    pub extra: Extra,
}

// ---- dispatch by file role -----------------------------------------------

/// Any mirrored JSON document a lab run writes under `--out`.
#[derive(Debug, Clone, PartialEq)]
pub enum Document {
    Report(Report),
    Inventory(Inventory),
    PageProvenance(PageProvenance),
    ProvenanceManifest(ProvenanceManifest),
    Redirects(Redirects),
    MediaManifest(MediaManifest),
    AttachmentsManifest(AttachmentsManifest),
    LinkAudit(LinkAudit),
    FrontmatterReview(FrontmatterReview),
    AssetFilenameManifest(AssetFilenameManifest),
    AssetFilenameRewriteManifest(AssetFilenameRewriteManifest),
}

impl Document {
    /// Decode `bytes` as the document at `rel`, a `/`-separated path
    /// relative to the run's `--out` (`report.json`,
    /// `provenance/posts/hello.json`, …).
    pub fn decode(rel: &str, bytes: &[u8]) -> Result<Self, DecodeError> {
        let one = |accepted, document| decode_keyed::<serde_json::Value>(document, accepted, bytes);
        Ok(match rel {
            "report.json" => Self::Report(Report::decode(bytes)?),
            "inventory.json" => Self::Inventory(Inventory::decode(bytes)?),
            "provenance_manifest.json" => Self::ProvenanceManifest(serde_json::from_value(
                one(PROVENANCE_MANIFESTS, "provenance_manifest.json")?.1,
            )?),
            "redirects.json" => {
                Self::Redirects(serde_json::from_value(one(REDIRECTS, "redirects.json")?.1)?)
            }
            "media_manifest.json" => Self::MediaManifest(serde_json::from_value(
                one(MEDIA_MANIFESTS, "media_manifest.json")?.1,
            )?),
            "attachments_manifest.json" => Self::AttachmentsManifest(serde_json::from_value(
                one(ATTACHMENTS_MANIFESTS, "attachments_manifest.json")?.1,
            )?),
            "link_audit.json" => Self::LinkAudit(serde_json::from_value(
                one(LINK_AUDITS, "link_audit.json")?.1,
            )?),
            "frontmatter_review.json" => Self::FrontmatterReview(serde_json::from_value(
                one(FRONTMATTER_REVIEWS, "frontmatter_review.json")?.1,
            )?),
            "asset_filename_manifest.json" => Self::AssetFilenameManifest(serde_json::from_value(
                one(ASSET_FILENAME_MANIFESTS, "asset_filename_manifest.json")?.1,
            )?),
            "rewrite_manifest.json" => Self::AssetFilenameRewriteManifest(serde_json::from_value(
                one(ASSET_FILENAME_REWRITE_MANIFESTS, "rewrite_manifest.json")?.1,
            )?),
            _ if rel.starts_with("provenance/") && rel.ends_with(".json") => Self::PageProvenance(
                serde_json::from_value(one(PAGE_PROVENANCE, "provenance/<id>.json")?.1)?,
            ),
            _ => return Err(DecodeError::UnknownDocument(rel.to_string())),
        })
    }
}
