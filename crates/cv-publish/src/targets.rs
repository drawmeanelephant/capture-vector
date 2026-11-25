//! Target registry. `verified` mirrors Boris's own registry exactly
//! (boris `docs/contracts/publication-platforms.md`): only GitHub Pages
//! and Standard.site are verified targets. Nostr NIP-23 is a shipped CLI
//! family Boris explicitly keeps off the verified seam; a static `dist/`
//! copy is Boris's default output, not a verified hosting target.

use cv_core::engine::ExportKind;
use cv_core::publish::TargetSpec;
use cv_core::rules::{SlashPolicy, TargetDeployProfile};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    GithubPages,
    StandardSite,
    NostrNip23,
    StaticDist,
}

/// Every target, in display order.
pub fn registry() -> [Target; 4] {
    [
        Target::GithubPages,
        Target::StandardSite,
        Target::NostrNip23,
        Target::StaticDist,
    ]
}

impl Target {
    pub fn id(self) -> &'static str {
        match self {
            Self::GithubPages => "github-pages",
            Self::StandardSite => "standard-site",
            Self::NostrNip23 => "nostr-nip23",
            Self::StaticDist => "static-dist",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::GithubPages => "GitHub Pages",
            Self::StandardSite => "Standard.site (ATProto)",
            Self::NostrNip23 => "Nostr NIP-23",
            Self::StaticDist => "Static dist",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        registry().into_iter().find(|t| t.id() == id)
    }

    /// Boris's verified-target registry membership — never upgraded here.
    pub fn verified(self) -> bool {
        matches!(self, Self::GithubPages | Self::StandardSite)
    }

    pub fn verification_label(self) -> &'static str {
        if self.verified() {
            "verified"
        } else {
            "unverified"
        }
    }

    pub fn spec(self) -> TargetSpec {
        TargetSpec {
            id: self.id().into(),
            verified: self.verified(),
        }
    }

    /// Secret this target's transport needs, by keychain name. Delivered
    /// only through a `SecretBackend` (keychain / stdin), never argv/env.
    pub fn secret_name(self) -> Option<&'static str> {
        match self {
            Self::StandardSite => Some("atproto-app-password"),
            Self::NostrNip23 => Some("nostr-nsec"),
            Self::GithubPages | Self::StaticDist => None,
        }
    }

    /// HTML-hosting targets serve the machine exports; record targets
    /// (ATProto, Nostr) don't host a site tree.
    pub fn hosts_html(self) -> bool {
        matches!(self, Self::GithubPages | Self::StaticDist)
    }

    pub fn deploy_profile(self, base_url: &str) -> TargetDeployProfile {
        let (trailing_slash, expects_robots_txt) = match self {
            // Pages serves `dir/index.html` and redirects to `dir/`.
            Self::GithubPages => (SlashPolicy::Always, true),
            Self::StaticDist => (SlashPolicy::Either, true),
            Self::StandardSite | Self::NostrNip23 => (SlashPolicy::Either, false),
        };
        TargetDeployProfile {
            base_url: base_url.into(),
            trailing_slash,
            expects_robots_txt,
        }
    }

    /// Machine exports that must ship with an HTML-hosting target,
    /// keyed by output-relative path (a trailing `/` means a directory).
    pub fn default_exports(self) -> Vec<(ExportKind, String)> {
        if !self.hosts_html() {
            return vec![];
        }
        vec![
            (ExportKind::Sitemap, "sitemap.xml".into()),
            (ExportKind::Rss, "feed.xml".into()),
            (ExportKind::Llms, "llms.txt".into()),
            (ExportKind::Context, "context/".into()),
        ]
    }
}

/// The full target configuration a grant seals (`target_cfg_hash`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TargetConfig {
    pub target: Target,
    pub verified: bool,
    /// Boris publication profile driving the build. `None` is explicit:
    /// this target is Boris's synthetic `default` HTML target, not a named
    /// publication profile pretending to be one.
    pub profile: Option<String>,
    pub deploy: TargetDeployProfile,
    /// Delivery adapter id + its destination (project-relative folder
    /// outside `.cv/`, URL, …).
    pub adapter: String,
    pub destination: String,
    pub exports: Vec<(ExportKind, String)>,
}

impl TargetConfig {
    pub fn new(
        target: Target,
        profile: &str,
        base_url: &str,
        adapter: &str,
        destination: &str,
    ) -> Self {
        Self {
            target,
            verified: target.verified(),
            profile: Some(profile.into()),
            deploy: target.deploy_profile(base_url),
            adapter: adapter.into(),
            destination: destination.into(),
            exports: target.default_exports(),
        }
    }

    /// The synthetic Boris `default` HTML target. Static-dist means exactly
    /// this target: it is not a publication profile target and must never
    /// validate or seal proof artifacts from a different named target.
    pub fn default_html(target: Target, base_url: &str, adapter: &str, destination: &str) -> Self {
        Self {
            target,
            verified: target.verified(),
            profile: None,
            deploy: target.deploy_profile(base_url),
            adapter: adapter.into(),
            destination: destination.into(),
            exports: target.default_exports(),
        }
    }
}
