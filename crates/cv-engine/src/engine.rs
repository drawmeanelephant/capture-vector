//! `ProcessEngine` — the `RawSubprocess` adapter behind the `Engine` port.

use crate::emit::Emitter;
use crate::error::{EngineError, Result};
use crate::exit::ExitOutcome;
use crate::fallback::{self, FallbackStart, build_event};
use crate::locate::{EngineIdentity, Locator, PINNED, Pin};
use crate::paths::{
    canonical_parts as path_canonical_parts, canonicalize_with_missing, normalize_component,
};
use crate::probe::{CapabilityReport, PROBE_TIMEOUT, WatchMode, probe};
use crate::proc::CancelToken;
use crate::ring::{DEFAULT_RING, Resume};
use crate::secrets::{SecretSource, SignOutput, nostr_sign};
use crate::snapshot::DiskTruth;
use crate::verbs::{IR_DIR, VerbOutput, argv, flag_value, run_argv, verb_name};
use crate::watch::WatchStart;
use cv_core::engine::{
    Engine, EngineCaps, EngineInfo, EventSink, ExitClass, Fingerprint, MoveOutcome, PageDraft,
    PageMove, RemoveOutcome, Verb, WatchHandle, WriteOutcome,
};
use cv_core::events::engine::{BuildPhase, EngineEvent, VerbReport};
use cv_core::events::{BuildId, Cursor};
use cv_core::id::WorkspaceId;
use cv_core::rules::ArtifactSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub root: PathBuf,
    pub workspace: WorkspaceId,
    pub locator: Locator,
    pub pin: Pin,
    /// Workspace-relative HTML output tree (Boris default `dist`).
    pub html_dir: String,
    /// Workspace-relative content input the write path stages under.
    pub content_input: PathBuf,
    /// Extra args for `boris watch --watch-json` (e.g. `--serve`).
    pub watch_args: Vec<String>,
    pub ring_capacity: usize,
    pub handshake_timeout: Duration,
    pub probe_timeout: Duration,
    pub poll_interval: Duration,
}

impl EngineConfig {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            workspace: WorkspaceId(root.to_string_lossy().into_owned()),
            locator: Locator::from_env(&root),
            root,
            pin: PINNED,
            html_dir: "dist".into(),
            content_input: PathBuf::from("content"),
            watch_args: vec![],
            ring_capacity: DEFAULT_RING,
            handshake_timeout: Duration::from_secs(10),
            probe_timeout: PROBE_TIMEOUT,
            poll_interval: Duration::from_millis(500),
        }
    }
}

fn relative_parts(raw: &str, what: &str) -> Result<Vec<String>> {
    let path = Path::new(raw);
    if path.is_absolute() || raw.trim().is_empty() || raw.contains('\\') {
        return Err(EngineError::Refused(format!(
            "{what} {raw:?} must be a workspace-relative path"
        )));
    }
    let parts = path
        .components()
        .filter(|c| *c != std::path::Component::CurDir)
        .map(|c| match c {
            std::path::Component::Normal(n) => Ok(n.to_string_lossy().into_owned()),
            std::path::Component::ParentDir => Ok("..".to_string()),
            _ => Err(EngineError::Refused(format!(
                "{what} {raw:?} must be a workspace-relative path"
            ))),
        })
        .collect::<Result<Vec<_>>>()?;
    if parts.is_empty() {
        return Err(EngineError::Refused(format!(
            "{what} {raw:?} must name a workspace-relative child, not the workspace root"
        )));
    }
    Ok(parts)
}

fn overlaps(a: &[String], b: &[String]) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

fn owned_cv_output(parts: &[String]) -> bool {
    [
        [".cv", "publish"],
        [".cv", "ir"],
        [".cv", "ir.staging"],
        [".cv", "out"],
    ]
    .iter()
    .map(|lane| lane.map(str::to_string))
    .any(|lane| parts.starts_with(&lane))
}

fn output_cfg(cfg: &EngineConfig, input: Option<&str>) -> Result<EngineConfig> {
    let mut cfg = cfg.clone();
    if let Some(input) = input {
        let absolute = Path::new(input).is_absolute();
        if input.trim().is_empty()
            || input != input.trim()
            || (!absolute && input != "." && cv_store::paths::portable_relative(input).is_err())
        {
            return Err(EngineError::Refused(format!(
                "content input {input:?} is not a usable workspace content authority"
            )));
        }
        cfg.content_input = input.into();
    }
    Ok(cfg)
}

fn path_from_parts(root: &Path, parts: &[String]) -> PathBuf {
    parts
        .iter()
        .fold(root.to_path_buf(), |p, part| p.join(part))
}

fn partition_parts(path: &Path, what: &str) -> Result<Vec<String>> {
    path_canonical_parts(path).map_err(|e| {
        EngineError::Refused(format!(
            "{what} cannot be canonicalized for partition validation: {e}"
        ))
    })
}

fn collapse_relative_parts(raw: &str, raw_parts: &[String], what: &str) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for part in raw_parts {
        if part == ".." {
            if out.pop().is_none() {
                return Err(EngineError::Refused(format!(
                    "{what} {raw:?} escapes the workspace through parent traversal"
                )));
            }
        } else {
            out.push(normalize_component(part));
        }
    }
    if out.is_empty() {
        return Err(EngineError::Refused(format!(
            "{what} {raw:?} must name a workspace-relative child, not the workspace root"
        )));
    }
    Ok(out)
}

fn validate_output_location(
    cfg: &EngineConfig,
    output_path: &Path,
    lexical: &[String],
    raw: &str,
    what: &str,
) -> Result<()> {
    let absolute = partition_parts(output_path, what)?;
    let root_parts = partition_parts(&cfg.root, what)?;
    let Some(relative) = absolute.strip_prefix(root_parts.as_slice()) else {
        return Err(EngineError::Refused(format!(
            "{what} {raw:?} resolves outside the workspace"
        )));
    };

    let cv = [".cv".to_string()];
    let boris_meta = [".boris".to_string()];
    let profile = ["boris.json".to_string()];
    if overlaps(lexical, &profile) || overlaps(relative, &profile) {
        return Err(EngineError::Refused(format!(
            "{what} {raw:?} overlaps workspace profile metadata"
        )));
    }
    let cv_violation = |parts: &[String]| parts.starts_with(&cv) && !owned_cv_output(parts);
    if cv_violation(lexical) || cv_violation(relative) {
        return Err(EngineError::Refused(format!(
            "{what} {raw:?} overlaps Capture Vector metadata outside an owned output lane"
        )));
    }
    if (lexical.starts_with(&boris_meta) || relative.starts_with(&boris_meta)) && raw != ".boris" {
        return Err(EngineError::Refused(format!(
            "{what} {raw:?} overlaps Boris metadata"
        )));
    }

    let configured = &cfg.content_input;
    let input_path = if configured.is_absolute() {
        configured.clone()
    } else {
        cfg.root.join(configured)
    };
    let input_parts = partition_parts(&input_path, what)?;
    let input = configured.to_string_lossy();
    if input_parts == root_parts {
        let html = normalize_component(&cfg.html_dir);
        let derived = relative.starts_with(&cv)
            || relative.starts_with(&boris_meta)
            || relative.first().is_some_and(|p| *p == html);
        if !derived {
            return Err(EngineError::Refused(format!(
                "{what} {raw:?} overlaps the workspace content input, which is the workspace root"
            )));
        }
    } else if overlaps(&absolute, &input_parts) {
        return Err(EngineError::Refused(format!(
            "{what} {raw:?} overlaps the workspace content input {input:?}"
        )));
    }
    Ok(())
}

fn validate_output_path(cfg: &EngineConfig, raw: &str, what: &str) -> Result<()> {
    let raw_parts = relative_parts(raw, what)?;
    let lexical = collapse_relative_parts(raw, &raw_parts, what)?;
    let output_path = path_from_parts(&cfg.root, &raw_parts);
    validate_output_location(cfg, &output_path, &lexical, raw, what)
}

/// Validate a profile-declared output at the base Boris 0.9.0 evidences for
/// publication profiles: the directory containing the selected profile file.
/// The raw spelling still cannot escape that base through leading `..`;
/// after that, canonical containment decides whether the resolved location
/// is inside the workspace.
fn validate_profile_output_at_base(
    cfg: &EngineConfig,
    base: &Path,
    raw: &str,
    what: &str,
) -> Result<()> {
    let raw_parts = relative_parts(raw, what)?;
    let collapsed = collapse_relative_parts(raw, &raw_parts, what)?;
    let root_parts = partition_parts(&cfg.root, what)?;
    let base_parts = partition_parts(base, what)?;
    let mut lexical = base_parts
        .strip_prefix(root_parts.as_slice())
        .map(|prefix| prefix.to_vec())
        .unwrap_or_default();
    lexical.extend(collapsed);
    let output_path = path_from_parts(base, &raw_parts);
    validate_output_location(cfg, &output_path, &lexical, raw, what)
}

fn validate_output_against_profile_input_ref(
    output_path: &Path,
    input_path: &Path,
    raw: &str,
    what: &str,
    kind: &str,
) -> Result<()> {
    let output_parts = partition_parts(output_path, what)?;
    let input_parts = partition_parts(input_path, what)?;
    if overlaps(&output_parts, &input_parts) {
        return Err(EngineError::Refused(format!(
            "{what} {raw:?} overlaps publication profile {kind} {}",
            input_path.display()
        )));
    }
    Ok(())
}

fn flag_value_at<'a>(args: &'a [String], i: usize, flag: &str) -> Option<&'a str> {
    let arg = &args[i];
    if arg == flag {
        args.get(i + 1).map(String::as_str)
    } else {
        arg.strip_prefix(flag)?.strip_prefix('=')
    }
}

fn validate_custom_outputs(cfg: &EngineConfig, args: &[String]) -> Result<()> {
    let effective = output_cfg(cfg, flag_value(args, "--input"))?;
    let cfg = &effective;
    const PATH_FLAGS: &[&str] = &[
        "--out",
        "--html-dir",
        "--dist",
        "--llms-path",
        "--rss-path",
        "--sitemap-path",
        "--rag-dir",
        "--context-dir",
        "--report",
        "--report-out",
        "--session-root",
        "--plan",
        "--bundle",
    ];
    for (i, _arg) in args.iter().enumerate() {
        for flag in PATH_FLAGS {
            if let Some(value) = flag_value_at(args, i, flag) {
                if *flag == "--out" && value.trim_start_matches("./") == ".boris" {
                    return Err(EngineError::Refused(format!(
                        "{flag} {value:?} overlaps Boris metadata"
                    )));
                }
                validate_output_path(cfg, value, flag)?;
            }
        }
        if let Some(value) = flag_value_at(args, i, "--target")
            && let Some((_, output)) = value.split_once('=')
        {
            validate_output_path(cfg, output, "--target")?;
        }
    }
    let implicit = [
        ("--rag", "rag"),
        ("--context", "context"),
        ("--llms", "llms.txt"),
        ("--rss", "rss.xml"),
        ("--no-rag", ".boris"),
    ];
    for (flag, output) in implicit {
        if args.iter().any(|a| a == flag) {
            validate_output_path(cfg, output, flag)?;
        }
    }
    Ok(())
}

fn validate_default_html_output(cfg: &EngineConfig, name: &str, args: &[String]) -> Result<()> {
    if !matches!(name, "build" | "watch") || flag_value(args, "--profile").is_some() {
        return Ok(());
    }
    let effective = output_cfg(cfg, flag_value(args, "--input"))?;
    let cfg = &effective;
    let redirected = args.iter().any(|a| {
        [
            "--out",
            "--html-dir",
            "--target",
            "--rag",
            "--rag-dir",
            "--context",
            "--context-dir",
            "--llms",
            "--llms-path",
            "--rss",
            "--rss-path",
            "--no-rag",
        ]
        .iter()
        .any(|flag| a == flag || a.starts_with(&format!("{flag}=")))
    });
    if !redirected {
        validate_output_path(cfg, "dist", "default HTML output")?;
    }
    Ok(())
}

fn validate_watch_output_paths(cfg: &EngineConfig) -> Result<()> {
    validate_output_path(cfg, &cfg.html_dir, "watch HTML output")?;
    let verb = Verb::Custom {
        name: "watch".into(),
        args: cfg.watch_args.clone(),
    };
    validate_verb_output_paths(cfg, &verb)
}

fn validate_custom_init_targets(cfg: &EngineConfig, args: &[String]) -> Result<()> {
    // Boris init grammar is `[DIR] [--type NAME] [--quiet]`. Validate every
    // positional candidate rather than trusting position zero: the real CLI
    // also accepts the target after `--type NAME`. Unknown options are left
    // to Boris's usage error, while every candidate that could be DIR must
    // be partition-clean before spawn.
    let mut i = 0;
    let mut positional_only = false;
    let mut positional_count = 0;
    while i < args.len() {
        let arg = &args[i];
        if !positional_only && arg == "--" {
            positional_only = true;
        } else if !positional_only && arg == "--type" {
            i += 1;
        } else if !positional_only && (arg.starts_with("--type=") || arg == "--quiet") {
            // Flag or inline value only; no positional candidate here.
        } else if !positional_only && arg.starts_with('-') {
            // Unknown flag: Boris will refuse usage before writing. Its
            // possible value is still validated when it does not look like
            // a flag, which is deliberately conservative.
        } else {
            positional_count += 1;
            validate_output_path(cfg, arg, "init target")?;
        }
        i += 1;
    }
    // `[DIR]` is optional in Boris init; when it is absent the target is the
    // process working directory. Validate that implicit target too, except
    // for informational usage spellings that exit before writing.
    let informational = args
        .iter()
        .any(|a| matches!(a.as_str(), "-h" | "--help" | "-V" | "--version"));
    if positional_count == 0 && !informational {
        validate_output_path(cfg, ".", "implicit init target")?;
    }
    Ok(())
}

enum ProfileDeclaredOutput {
    /// Resolved against the directory containing the selected profile.
    ProfileRelative(String),
    /// Boris declares sitemap/rss/llms paths target-root-relative. Joining
    /// the declared target root is inspection of the written location, not a
    /// decision about whether the target is selected.
    TargetNested { target_output: String, path: String },
}

fn profile_shape_error(profile: &Path, detail: &str) -> EngineError {
    EngineError::Refused(format!(
        "publication profile {} has {detail}; output overlap cannot be proven",
        profile.display()
    ))
}

fn required_string_field<'a>(
    profile: &Path,
    object: &'a serde_json::Value,
    key: &str,
    locus: &str,
) -> Result<&'a str> {
    match object.get(key) {
        Some(serde_json::Value::String(s)) => Ok(s),
        _ => Err(profile_shape_error(
            profile,
            &format!("missing or non-string {locus}.{key}"),
        )),
    }
}

struct ProfileInputRef {
    path: String,
    kind: &'static str,
}

fn optional_string_field(
    profile: &Path,
    object: &serde_json::Value,
    key: &str,
    locus: &str,
) -> Result<Option<String>> {
    match object.get(key) {
        None => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
        _ => Err(profile_shape_error(
            profile,
            &format!("non-string {locus}.{key}"),
        )),
    }
}

fn layout_parent_path(layout: &str) -> Option<&str> {
    let (parent, _) = layout.rsplit_once('/')?;
    if parent.is_empty() || parent == "." {
        None
    } else {
        Some(parent)
    }
}

fn push_layout_input_refs(input_refs: &mut Vec<ProfileInputRef>, layout: &str, kind: &'static str) {
    input_refs.push(ProfileInputRef {
        path: layout.to_string(),
        kind,
    });
    // Boris target validation treats both the declared layout file and its
    // parent directory as protected input roots. The parent tree can carry
    // sibling includes/assets even though the layout declaration itself names
    // one file.
    if let Some(parent) = layout_parent_path(layout) {
        input_refs.push(ProfileInputRef {
            path: parent.to_string(),
            kind: "layout parent tree",
        });
    }
}

fn theme_root_from_layout(layout: &str) -> Option<String> {
    let marker = "/layouts/";
    let index = layout.find(marker)?;
    let file = &layout[index + marker.len()..];
    if index == 0 || file.is_empty() || file.contains('/') {
        return None;
    }
    Some(layout[..index].to_string())
}

fn declared_profile_outputs(
    profile: &Path,
    bytes: &[u8],
) -> Result<(
    Option<String>,
    Vec<ProfileDeclaredOutput>,
    Vec<ProfileInputRef>,
)> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| {
        EngineError::Refused(format!(
            "publication profile {} cannot be parsed for output inspection: {e}",
            profile.display()
        ))
    })?;
    let input = match value.get("input") {
        None => None,
        Some(serde_json::Value::String(input)) => Some(input.clone()),
        _ => {
            return Err(profile_shape_error(
                profile,
                "non-string input; effective input authority cannot be proven",
            ));
        }
    };
    let mut outputs = Vec::new();
    let mut input_refs = Vec::new();
    if let Some(targets) = value.get("targets") {
        let targets = targets
            .as_array()
            .ok_or_else(|| profile_shape_error(profile, "non-array targets"))?;
        for (index, target) in targets.iter().enumerate() {
            let locus = format!("targets[{index}]");
            let target_output =
                required_string_field(profile, target, "output", &locus)?.to_string();
            outputs.push(ProfileDeclaredOutput::ProfileRelative(
                target_output.clone(),
            ));
            for section in ["sitemap", "rss", "llms"] {
                if let Some(section_value) = target.get(section) {
                    let section_locus = format!("{locus}.{section}");
                    let path =
                        required_string_field(profile, section_value, "path", &section_locus)?;
                    outputs.push(ProfileDeclaredOutput::TargetNested {
                        target_output: target_output.clone(),
                        path: path.to_string(),
                    });
                }
            }
            let theme = optional_string_field(profile, target, "theme", &locus)?;
            let explicit_layout = optional_string_field(profile, target, "layout", &locus)?;
            let fallback_layout = if let Some(theme) = &theme {
                input_refs.push(ProfileInputRef {
                    path: theme.clone(),
                    kind: "theme tree",
                });
                format!("{theme}/layouts/main.html")
            } else {
                explicit_layout
                    .clone()
                    .unwrap_or_else(|| "themes/boris/layouts/main.html".to_string())
            };
            push_layout_input_refs(&mut input_refs, &fallback_layout, "layout file");
            if theme.is_none()
                && let Some(root) = theme_root_from_layout(&fallback_layout)
            {
                input_refs.push(ProfileInputRef {
                    path: root,
                    kind: "derived theme tree",
                });
            }
            if let Some(rules) = target.get("layout_rules") {
                let rules = rules.as_array().ok_or_else(|| {
                    profile_shape_error(profile, &format!("non-array {locus}.layout_rules"))
                })?;
                for (rule_index, rule) in rules.iter().enumerate() {
                    let rule_locus = format!("{locus}.layout_rules[{rule_index}]");
                    let layout = required_string_field(profile, rule, "layout", &rule_locus)?;
                    push_layout_input_refs(&mut input_refs, layout, "layout-rule file");
                    if theme.is_none()
                        && let Some(root) = theme_root_from_layout(layout)
                    {
                        input_refs.push(ProfileInputRef {
                            path: root,
                            kind: "derived theme tree",
                        });
                    }
                }
            }
            if let Some(static_value) = target.get("static") {
                let static_locus = format!("{locus}.static");
                let dir = required_string_field(profile, static_value, "dir", &static_locus)?;
                input_refs.push(ProfileInputRef {
                    path: dir.to_string(),
                    kind: "static input tree",
                });
            }
        }
    }
    if let Some(editions) = value.get("editions") {
        if !editions.is_object() {
            return Err(profile_shape_error(profile, "non-object editions"));
        }
        for edition in ["ir", "rag", "context"] {
            if let Some(section) = editions.get(edition) {
                let locus = format!("editions.{edition}");
                let output = required_string_field(profile, section, "output", &locus)?;
                outputs.push(ProfileDeclaredOutput::ProfileRelative(output.to_string()));
            }
        }
    }
    Ok((input, outputs, input_refs))
}

/// Inspect the declared output strings of an explicitly selected Boris
/// publication profile. This is not target resolution: Capture Vector does
/// not decide which targets Boris will execute, apply mode implications, or
/// interpret publication semantics. Once `--profile` is present on an
/// HTML-aware invocation, however, every declared output is potentially
/// attacker-controlled output authority, so v1 validates the complete
/// declared set conservatively. Boris 0.9.0's plain build without
/// `--profile` uses its synthetic `default` target rather than boris.json.
fn validate_profile_declared_outputs(
    cfg: &EngineConfig,
    name: &str,
    args: &[String],
) -> Result<()> {
    if !matches!(name, "build" | "validate" | "watch" | "proof") {
        return Ok(());
    }
    let Some(profile) = flag_value(args, "--profile") else {
        return Ok(());
    };
    let profile_path = {
        let path = Path::new(profile);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            cfg.root.join(path)
        }
    };
    let bytes = std::fs::read(&profile_path).map_err(|e| {
        EngineError::Refused(format!(
            "publication profile {} cannot be read for output inspection: {e}",
            profile_path.display()
        ))
    })?;
    let base = profile_path.parent().unwrap_or(&cfg.root);
    let (declared_input, outputs, declared_input_refs) =
        declared_profile_outputs(&profile_path, &bytes)?;
    // Pinned Boris precedence: an invocation `--input` overrides the
    // profile; otherwise the profile's `input` is the authority, and an
    // omitted input defaults to `content`. Every relative input spelling in
    // profile mode resolves against the selected profile file's directory.
    // `cfg.content_input` is not implicitly passed to a custom profile
    // invocation, so it is not the tree Boris will read here.
    let invocation_input = flag_value(args, "--input");
    let mut effective = output_cfg(cfg, invocation_input)?;
    if let Some(input) = invocation_input {
        if !Path::new(input).is_absolute() {
            let input_parts = relative_parts(input, "invocation input")?;
            effective.content_input = path_from_parts(base, &input_parts);
        }
    } else {
        let input = declared_input.as_deref().unwrap_or("content");
        let input_parts = relative_parts(input, "profile input")?;
        effective.content_input = path_from_parts(base, &input_parts);
    }
    let mut input_refs = Vec::new();
    for input_ref in &declared_input_refs {
        let parts = relative_parts(&input_ref.path, input_ref.kind)?;
        input_refs.push((
            path_from_parts(base, &parts),
            input_ref.kind,
            input_ref.path.clone(),
        ));
    }
    let mut output_roots: Vec<(String, PathBuf)> = Vec::new();
    let mut artifact_outputs: Vec<(String, PathBuf)> = Vec::new();
    for output in outputs {
        match output {
            ProfileDeclaredOutput::ProfileRelative(output) => {
                validate_profile_output_at_base(
                    &effective,
                    base,
                    &output,
                    "profile target output",
                )?;
                let raw_parts = relative_parts(&output, "profile target output")?;
                output_roots.push((output.clone(), path_from_parts(base, &raw_parts)));
                for (input_path, kind, _) in &input_refs {
                    validate_output_against_profile_input_ref(
                        &path_from_parts(base, &raw_parts),
                        input_path,
                        &output,
                        "profile target output",
                        kind,
                    )?;
                }
            }
            ProfileDeclaredOutput::TargetNested {
                target_output,
                path,
            } => {
                let output = format!("{target_output}/{path}");
                validate_profile_output_at_base(
                    &effective,
                    base,
                    &output,
                    "profile artifact output",
                )?;
                let raw_parts = relative_parts(&output, "profile artifact output")?;
                let artifact_path = path_from_parts(base, &raw_parts);
                for (input_path, kind, _) in &input_refs {
                    validate_output_against_profile_input_ref(
                        &artifact_path,
                        input_path,
                        &output,
                        "profile artifact output",
                        kind,
                    )?;
                }
                artifact_outputs.push((output, artifact_path));
            }
        }
    }
    let mut canonical_roots = Vec::new();
    for (raw, path) in &output_roots {
        canonical_roots.push((raw.clone(), partition_parts(path, "profile output root")?));
    }
    for (index, (left_raw, left)) in canonical_roots.iter().enumerate() {
        for (right_raw, right) in &canonical_roots[index + 1..] {
            if overlaps(left, right) {
                return Err(EngineError::Refused(format!(
                    "profile output roots {left_raw:?} and {right_raw:?} physically overlap"
                )));
            }
        }
    }
    // Artifact files are legitimate descendants of their owning target root,
    // but two artifact outputs must not physically alias one another even when
    // Boris's lexical collision check sees distinct spellings.
    let mut canonical_artifacts = Vec::new();
    for (raw, path) in &artifact_outputs {
        canonical_artifacts.push((
            raw.clone(),
            partition_parts(path, "profile artifact output")?,
        ));
    }
    for (index, (left_raw, left)) in canonical_artifacts.iter().enumerate() {
        for (right_raw, right) in &canonical_artifacts[index + 1..] {
            if overlaps(left, right) {
                return Err(EngineError::Refused(format!(
                    "profile artifact outputs {left_raw:?} and {right_raw:?} physically overlap"
                )));
            }
        }
    }
    Ok(())
}

fn validate_verb_output_paths(cfg: &EngineConfig, verb: &Verb) -> Result<()> {
    if let Verb::Ir { out, input } = verb {
        let effective = output_cfg(cfg, input.as_deref())?;
        validate_output_path(&effective, out, "IR output")?;
    }
    if let Verb::Init { dir, .. } = verb {
        validate_output_path(cfg, dir, "init target")?;
    }
    if let Verb::Export { kind } = verb {
        use cv_core::engine::ExportKind;
        let output = match kind {
            ExportKind::Out => ".boris",
            ExportKind::Rag => "rag",
            ExportKind::Context => "context",
            ExportKind::Llms => "llms.txt",
            ExportKind::Rss => "rss.xml",
            ExportKind::Sitemap => "dist",
        };
        validate_output_path(cfg, output, "export output")?;
    }
    if let Verb::Custom { name, args } = verb {
        if name == "init" {
            validate_custom_init_targets(cfg, args)?;
        }
        validate_custom_outputs(cfg, args)?;
        validate_profile_declared_outputs(cfg, name, args)?;
        validate_default_html_output(cfg, name, args)?;
    }
    if matches!(verb, Verb::Build) {
        validate_output_path(cfg, "dist", "build output")?;
    }
    Ok(())
}

pub struct ProcessEngine {
    cfg: EngineConfig,
    identity: EngineIdentity,
    report: CapabilityReport,
    emitter: Arc<Emitter>,
    watch_live: Arc<AtomicBool>,
    runs: AtomicU64,
}

impl ProcessEngine {
    /// Locate + gate + probe. Fails on any mismatch; never runs unpinned.
    pub fn start(cfg: EngineConfig) -> Result<Self> {
        let identity = cfg.locator.locate(&cfg.root, cfg.pin)?;
        let report = probe(
            &identity.path,
            &cfg.root,
            &cfg.content_input,
            cfg.probe_timeout,
        );
        Ok(Self::with_report(cfg, identity, report))
    }

    /// Skip probing with a known report (tests, or a cached probe).
    pub fn with_report(
        cfg: EngineConfig,
        identity: EngineIdentity,
        report: CapabilityReport,
    ) -> Self {
        Self {
            emitter: Arc::new(Emitter::new(cfg.workspace.clone(), cfg.ring_capacity)),
            cfg,
            identity,
            report,
            watch_live: Arc::new(AtomicBool::new(false)),
            runs: AtomicU64::new(0),
        }
    }

    pub fn identity(&self) -> &EngineIdentity {
        &self.identity
    }
    pub fn report(&self) -> &CapabilityReport {
        &self.report
    }
    pub fn root(&self) -> &Path {
        &self.cfg.root
    }
    /// The HTML output tree the watch writes: the scheduler's seat.
    pub fn html_dir(&self) -> &str {
        &self.cfg.html_dir
    }

    /// The IR projection the live session runs beside its watch: Boris
    /// IR mode into [`IR_DIR`], over the content root the watch was given.
    pub fn ir_projection(&self) -> Verb {
        Verb::Ir {
            out: IR_DIR.into(),
            input: flag_value(&self.cfg.watch_args, "--input").map(String::from),
        }
    }
    /// The next envelope will carry `(session, seq)` = this cursor.
    pub fn cursor(&self) -> Cursor {
        self.emitter.cursor()
    }
    /// Replay from the bounded ring, or `Resync`.
    pub fn resume(&self, seen: &Cursor) -> Resume {
        self.emitter.resume(seen)
    }
    pub fn ring_capacity(&self) -> usize {
        self.emitter.ring().capacity()
    }

    /// Ordinary content mutation participates in the same cross-process
    /// workspace critical section as adoption. Acquisition is bounded by
    /// cv-store's settling window, so contention is an honest retryable
    /// refusal rather than an indefinite wait or an unlocked write.
    fn workspace_mutation_lock(
        &self,
    ) -> Result<cv_store::workspace_mutation::WorkspaceMutationLock> {
        cv_store::workspace_mutation::lock_for_content_mutation(&self.cfg.root).map_err(|e| {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                EngineError::Refused(
                    "workspace is busy with another mutation or migration adoption; try again when it completes"
                        .into(),
                )
            } else {
                EngineError::Io(e)
            }
        })
    }

    pub fn disk_truth(&self) -> DiskTruth {
        self.emitter.bump_epoch();
        crate::snapshot::read(&self.cfg.root, &self.cfg.html_dir)
    }

    /// Run a verb, returning verbatim stdout/stderr (Activity renders
    /// these as-is). Emits nothing.
    pub fn run_verb(&self, verb: &Verb, cancel: &CancelToken) -> Result<VerbOutput> {
        validate_verb_output_paths(&self.cfg, verb)?;
        let _content_mutation = if crate::verbs::may_mutate_content(verb) {
            Some(self.workspace_mutation_lock()?)
        } else {
            None
        };
        run_argv(&self.identity.path, &self.cfg.root, &argv(verb)?, cancel)
    }

    /// `Engine::run` with cancellation. Build verbs emit
    /// `BuildFinished`/`BuildFailed`; every completed verb ends with
    /// `VerbFinished`, while a failure before a Boris exit lands as
    /// `VerbFailed` so the stream never silently consumes cursor events.
    pub fn run_cancellable(
        &self,
        verb: &Verb,
        sink: EventSink,
        cancel: &CancelToken,
    ) -> Result<ExitClass> {
        self.run_cancellable_caused(verb, sink, cancel, None, false)
    }

    /// `Engine::run_reported` with cancellation: as `run_cancellable`,
    /// but the terminal envelope is `VerbReported`.
    pub fn run_reported_cancellable(
        &self,
        verb: &Verb,
        sink: EventSink,
        cancel: &CancelToken,
    ) -> Result<ExitClass> {
        self.run_cancellable_caused(verb, sink, cancel, None, true)
    }

    fn run_cancellable_caused(
        &self,
        verb: &Verb,
        mut sink: EventSink,
        cancel: &CancelToken,
        cause: Option<&cv_core::events::EventId>,
        report: bool,
    ) -> Result<ExitClass> {
        match self.run_cancellable_inner(verb, &mut sink, cancel, cause, report) {
            Ok(exit) => Ok(exit),
            Err(e) => {
                self.emitter.emit_caused(
                    &mut sink,
                    None,
                    None,
                    EngineEvent::VerbFailed {
                        verb: verb_name(verb),
                        error: e.to_string(),
                    },
                    cause,
                );
                Err(e)
            }
        }
    }

    fn run_cancellable_inner(
        &self,
        verb: &Verb,
        sink: &mut EventSink,
        cancel: &CancelToken,
        cause: Option<&cv_core::events::EventId>,
        report: bool,
    ) -> Result<ExitClass> {
        validate_verb_output_paths(&self.cfg, verb)?;
        let staged = crate::ir::staged_verb(verb)?;
        if staged.is_some() {
            crate::ir::prepare(&self.cfg.root)?;
        }
        let out = self.run_verb(staged.as_ref().unwrap_or(verb), cancel)?;
        let exit = out
            .outcome
            .class()
            .ok_or(EngineError::Abnormal(out.outcome))?;
        if let Verb::Ir { out, .. } = verb {
            if exit == ExitClass::Ok {
                if let Err(e) = crate::ir::publish(&self.cfg.root, out) {
                    crate::ir::cleanup(&self.cfg.root);
                    return Err(e);
                }
            } else {
                crate::ir::cleanup(&self.cfg.root);
            }
        }
        let n = self.runs.fetch_add(1, Ordering::SeqCst) + 1;
        let build = BuildId(format!("{}:r{n}", self.emitter.session().0));
        let args = argv(verb)?;
        if args.first().map(String::as_str) == Some("build") {
            let ev = build_event(&out, BuildPhase::Initial, &build);
            self.emitter
                .emit_caused(sink, None, Some(build.clone()), ev, cause);
        }
        let terminal = if report {
            EngineEvent::VerbReported(VerbReport {
                verb: verb_name(verb),
                argv: args,
                exit,
                code: match out.outcome {
                    ExitOutcome::Exited { code } => Some(code),
                    _ => None,
                },
                duration_ms: out.duration_ms,
                stdout: VerbReport::bounded_stdout(&out.stdout),
                stderr: out.stderr,
            })
        } else {
            EngineEvent::VerbFinished {
                verb: verb_name(verb),
                exit,
            }
        };
        self.emitter
            .emit_caused(sink, None, Some(build), terminal, cause);
        Ok(exit)
    }

    pub fn sign_nostr(
        &self,
        plan: &Path,
        out: Option<&Path>,
        secrets: &dyn SecretSource,
        cancel: &CancelToken,
    ) -> Result<SignOutput> {
        if let Some(out) = out {
            let rel_owned;
            let rel = if out.is_absolute() {
                let root = canonicalize_with_missing(&self.cfg.root)?;
                let absolute = canonicalize_with_missing(out)?;
                rel_owned = absolute
                    .strip_prefix(&root)
                    .map_err(|_| {
                        EngineError::Refused(format!(
                            "sign output {:?} resolves outside the workspace",
                            out
                        ))
                    })?
                    .to_path_buf();
                &rel_owned
            } else {
                out
            };
            validate_output_path(&self.cfg, &rel.to_string_lossy(), "sign output")?;
        }
        let _content_mutation = self.workspace_mutation_lock()?;
        nostr_sign(
            &self.identity.path,
            &self.cfg.root,
            plan,
            out,
            secrets,
            cancel,
        )
    }

    pub fn watch_live(&self) -> bool {
        self.watch_live.load(Ordering::SeqCst)
    }

    pub fn start_watch(&self, sink: EventSink) -> Result<Box<dyn WatchHandle>> {
        validate_watch_output_paths(&self.cfg)?;
        if self.watch_live.swap(true, Ordering::SeqCst) {
            return Err(EngineError::WatchAlreadyLive);
        }
        let r: Result<Box<dyn WatchHandle>> = match self.report.watch_mode {
            WatchMode::Native => crate::watch::start(
                WatchStart {
                    bin: &self.identity.path,
                    root: &self.cfg.root,
                    args: &self.cfg.watch_args,
                    handshake_timeout: self.cfg.handshake_timeout,
                    live: self.watch_live.clone(),
                },
                self.emitter.clone(),
                sink,
            )
            .map(|w| Box::new(w) as Box<dyn WatchHandle>),
            WatchMode::OneShotFallback => fallback::start(
                FallbackStart {
                    bin: self.identity.path.clone(),
                    root: self.cfg.root.clone(),
                    html_dir: self.cfg.html_dir.clone(),
                    build_args: vec![],
                    interval: self.cfg.poll_interval,
                    compiler: self.identity.version.clone(),
                    live: self.watch_live.clone(),
                },
                self.emitter.clone(),
                sink,
            )
            .map(|w| Box::new(w) as Box<dyn WatchHandle>),
        };
        if r.is_err() {
            self.watch_live.store(false, Ordering::SeqCst);
        }
        r
    }
}

impl Engine for ProcessEngine {
    fn locate(&self) -> cv_core::Result<EngineInfo> {
        Ok(EngineInfo {
            version: self.identity.version.clone(),
            vcs_revision: Some(self.identity.vcs_revision.clone()),
            caps: self.report.caps,
        })
    }

    fn capabilities(&self) -> EngineCaps {
        self.report.caps
    }

    fn snapshot(&self) -> cv_core::Result<ArtifactSet> {
        Ok(self.disk_truth().artifacts)
    }

    fn watch(&self, sink: EventSink) -> cv_core::Result<Box<dyn WatchHandle>> {
        Ok(self.start_watch(sink)?)
    }

    fn run(&self, cmd: &Verb, sink: EventSink) -> cv_core::Result<ExitClass> {
        self.run_caused(cmd, sink, None)
    }

    fn run_caused(
        &self,
        cmd: &Verb,
        sink: EventSink,
        cause: Option<&cv_core::events::EventId>,
    ) -> cv_core::Result<ExitClass> {
        Ok(self.run_cancellable_caused(cmd, sink, &CancelToken::new(), cause, false)?)
    }

    fn run_reported(
        &self,
        cmd: &Verb,
        sink: EventSink,
        cancel: &cv_core::engine::CancelToken,
    ) -> cv_core::Result<ExitClass> {
        Ok(self.run_reported_cancellable(cmd, sink, cancel)?)
    }

    fn write_content(
        &self,
        draft: &PageDraft,
        base: Option<Fingerprint>,
    ) -> cv_core::Result<WriteOutcome> {
        if !self.report.caps.writes {
            return Err(EngineError::Refused("workspace is not writable (probe)".into()).into());
        }
        let _workspace_mutation = self.workspace_mutation_lock()?;
        Ok(crate::write::write_staged(
            &self.cfg.root,
            &draft.path,
            draft.body.as_bytes(),
            base.or(draft.base),
        )?)
    }

    fn remove_content(&self, path: &str, base: Fingerprint) -> cv_core::Result<RemoveOutcome> {
        if !self.report.caps.writes {
            return Err(EngineError::Refused("workspace is not writable (probe)".into()).into());
        }
        let _workspace_mutation = self.workspace_mutation_lock()?;
        Ok(crate::write::remove_staged(&self.cfg.root, path, base)?)
    }

    fn move_content(&self, mv: &PageMove) -> cv_core::Result<MoveOutcome> {
        if !self.report.caps.writes {
            return Err(EngineError::Refused("workspace is not writable (probe)".into()).into());
        }
        let _workspace_mutation = self.workspace_mutation_lock()?;
        Ok(crate::write::move_staged(&self.cfg.root, mv)?)
    }
}
