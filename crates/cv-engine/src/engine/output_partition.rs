//! Caller-controlled output/source partition validation.
//!
//! Boris is treated as the write-capable engine boundary. These helpers inspect
//! only contracted path authority before spawn; they do not reimplement Boris
//! target selection or publication semantics.

use super::EngineConfig;
use crate::error::{EngineError, Result};
use crate::paths::{
    canonical_parts as path_canonical_parts, normalize_component, normalized_parts,
};
use crate::verbs::flag_value;
use cv_core::engine::Verb;
use std::path::{Path, PathBuf};

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
        [".cv", "ir.boris-stage"],
        [".cv", "ir.staging"],
        [".cv", "ir.staging.boris-stage"],
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
        let html_transients = [".boris-stage", ".boris-sitemap-prev"]
            .iter()
            .map(|suffix| normalized_parts(Path::new(&format!("{}{suffix}", cfg.html_dir))))
            .collect::<Vec<_>>();
        let html_transient = html_transients
            .iter()
            .any(|parts| lexical.starts_with(parts) || relative.starts_with(parts));
        let derived = relative.starts_with(&cv)
            || relative.starts_with(&boris_meta)
            || relative.first().is_some_and(|p| *p == html)
            || html_transient;
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

pub(super) fn validate_output_path(cfg: &EngineConfig, raw: &str, what: &str) -> Result<()> {
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

fn validate_output_against_input_ref(
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
            "{what} {raw:?} overlaps input authority {kind} {}",
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

fn flag_values<'a>(args: &'a [String], flag: &str) -> Vec<&'a str> {
    args.iter()
        .enumerate()
        .filter_map(|(i, _)| flag_value_at(args, i, flag))
        .collect()
}

fn push_layout_surface_refs(input_refs: &mut Vec<InputRef>, layout: &str, kind: &'static str) {
    push_layout_input_refs(input_refs, layout, kind);
    if let Some(root) = theme_root_from_layout(layout) {
        push_theme_surface_refs(input_refs, &root);
    }
}

fn cli_invocation_is_html(args: &[String]) -> bool {
    let explicit_html = [
        "--html",
        "--html-dir",
        "--html-layout",
        "--theme",
        "--target",
        "--target-layout",
        "--target-profile",
        "--layout-rule",
        "--sitemap",
        "--sitemap-path",
        "--static-dir",
    ]
    .iter()
    .any(|flag| {
        args.iter()
            .any(|arg| arg == flag || arg.starts_with(&format!("{flag}=")))
    });
    if explicit_html {
        return true;
    }
    let non_html = [
        "--out",
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
    .any(|flag| {
        args.iter()
            .any(|arg| arg == flag || arg.starts_with(&format!("{flag}=")))
    });
    !non_html
}

fn cli_input_refs(args: &[String]) -> Vec<InputRef> {
    let mut refs = Vec::new();
    let global_layout = if let Some(theme) = flag_value(args, "--theme") {
        format!("{theme}/layouts/main.html")
    } else if let Some(layout) = flag_value(args, "--html-layout") {
        layout.to_string()
    } else {
        "themes/boris/layouts/main.html".to_string()
    };

    let mut target_names = Vec::new();
    for value in flag_values(args, "--target") {
        if let Some((name, _)) = value.split_once('=') {
            target_names.push(name.to_string());
        }
    }
    if target_names.is_empty() {
        target_names.push("default".to_string());
    }
    let target_layouts: Vec<(&str, &str)> = flag_values(args, "--target-layout")
        .into_iter()
        .filter_map(|value| value.split_once('='))
        .collect();

    let mut layouts = Vec::new();
    for name in &target_names {
        let layout = target_layouts
            .iter()
            .find_map(|(target, path)| (target == name).then_some(*path))
            .unwrap_or(global_layout.as_str());
        if !layouts.contains(&layout) {
            layouts.push(layout);
        }
    }
    for (_, path) in &target_layouts {
        if !layouts.contains(path) {
            layouts.push(path);
        }
    }
    for layout in layouts {
        push_layout_surface_refs(&mut refs, layout, "layout file");
    }

    for (i, arg) in args.iter().enumerate() {
        if arg == "--layout-rule" && i + 3 < args.len() {
            push_layout_surface_refs(&mut refs, &args[i + 3], "layout-rule file");
        }
    }
    if let Some(static_dir) = flag_value(args, "--static-dir") {
        refs.push(InputRef {
            path: static_dir.to_string(),
            kind: "static input tree",
        });
    }
    refs
}

fn resolve_input_refs(base: &Path, declared: &[InputRef]) -> Result<Vec<(PathBuf, &'static str)>> {
    let mut resolved = Vec::new();
    for input_ref in declared {
        let parts = relative_parts(&input_ref.path, input_ref.kind)?;
        resolved.push((path_from_parts(base, &parts), input_ref.kind));
    }
    Ok(resolved)
}

fn validate_output_path_and_inputs(
    cfg: &EngineConfig,
    raw: &str,
    what: &str,
    input_refs: &[(PathBuf, &str)],
) -> Result<PathBuf> {
    validate_output_path(cfg, raw, what)?;
    let raw_parts = relative_parts(raw, what)?;
    let output_path = path_from_parts(&cfg.root, &raw_parts);
    for (input_path, kind) in input_refs {
        validate_output_against_input_ref(&output_path, input_path, raw, what, kind)?;
    }
    Ok(output_path)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DerivedOutputShape {
    HtmlDirectory,
    IrDirectory,
    RagDirectory,
    ContextDirectory,
    LlmsFile,
    RssFile,
}

fn derived_output_spellings(raw: &str, shape: DerivedOutputShape) -> Vec<String> {
    match shape {
        DerivedOutputShape::HtmlDirectory => [".boris-stage", ".boris-sitemap-prev"]
            .iter()
            .map(|suffix| format!("{raw}{suffix}"))
            .collect(),
        DerivedOutputShape::IrDirectory => vec![format!("{raw}.boris-stage")],
        DerivedOutputShape::RagDirectory => {
            [".boris-rag-stage", ".boris-rag-next", ".boris-rag-prev"]
                .iter()
                .map(|suffix| format!("{raw}{suffix}"))
                .collect()
        }
        DerivedOutputShape::ContextDirectory => [".boris-context-stage", ".boris-context-prev"]
            .iter()
            .map(|suffix| format!("{raw}{suffix}"))
            .collect(),
        DerivedOutputShape::LlmsFile => [".boris-llms-stage", ".boris-llms-prev"]
            .iter()
            .map(|suffix| format!("{raw}{suffix}"))
            .collect(),
        DerivedOutputShape::RssFile => [".boris-stage", ".boris-prev"]
            .iter()
            .map(|suffix| format!("{raw}{suffix}"))
            .collect(),
    }
}

fn validate_derived_outputs(
    cfg: &EngineConfig,
    raw: &str,
    shape: DerivedOutputShape,
    input_refs: &[(PathBuf, &str)],
) -> Result<Vec<(String, PathBuf)>> {
    let mut validated = Vec::new();
    for derived in derived_output_spellings(raw, shape) {
        let path =
            validate_output_path_and_inputs(cfg, &derived, "derived stage output", input_refs)?;
        validated.push((derived, path));
    }
    Ok(validated)
}

fn validate_output_group_pairwise(outputs: &[(String, PathBuf)], label: &str) -> Result<()> {
    let mut canonical = Vec::new();
    for (raw, path) in outputs {
        canonical.push((raw.clone(), partition_parts(path, label)?));
    }
    for (index, (left_raw, left)) in canonical.iter().enumerate() {
        for (right_raw, right) in &canonical[index + 1..] {
            if overlaps(left, right) {
                return Err(EngineError::Refused(format!(
                    "{label} {left_raw:?} and {right_raw:?} physically overlap"
                )));
            }
        }
    }
    Ok(())
}

fn validate_shaped_output_group(
    cfg: &EngineConfig,
    raw: &str,
    what: &str,
    shape: DerivedOutputShape,
    input_refs: &[(PathBuf, &str)],
) -> Result<Vec<(String, PathBuf)>> {
    let output_path = validate_output_path_and_inputs(cfg, raw, what, input_refs)?;
    let mut group = vec![(raw.to_string(), output_path)];
    group.append(&mut validate_derived_outputs(cfg, raw, shape, input_refs)?);
    validate_output_group_pairwise(&group, what)?;
    Ok(group)
}

fn shaped_output_for_flag(name: &str, flag: &str) -> Option<DerivedOutputShape> {
    match flag {
        "--html-dir" => Some(DerivedOutputShape::HtmlDirectory),
        "--out" if name == "build" => Some(DerivedOutputShape::IrDirectory),
        "--rag-dir" => Some(DerivedOutputShape::RagDirectory),
        "--context-dir" => Some(DerivedOutputShape::ContextDirectory),
        "--llms-path" => Some(DerivedOutputShape::LlmsFile),
        "--rss-path" => Some(DerivedOutputShape::RssFile),
        _ => None,
    }
}

fn validate_custom_outputs(cfg: &EngineConfig, name: &str, args: &[String]) -> Result<()> {
    let effective = output_cfg(cfg, flag_value(args, "--input"))?;
    let cfg = &effective;
    let input_refs = if flag_value(args, "--profile").is_some() || !cli_invocation_is_html(args) {
        Vec::new()
    } else {
        resolve_input_refs(&cfg.root, &cli_input_refs(args))?
    };
    let mut html_outputs: Vec<(String, PathBuf)> = Vec::new();
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
                if let Some(shape) = shaped_output_for_flag(name, flag) {
                    let group = validate_shaped_output_group(cfg, value, flag, shape, &input_refs)?;
                    if shape == DerivedOutputShape::HtmlDirectory {
                        html_outputs.extend(group);
                    }
                } else {
                    validate_output_path_and_inputs(cfg, value, flag, &input_refs)?;
                }
            }
        }
        if let Some(value) = flag_value_at(args, i, "--target")
            && let Some((_, output)) = value.split_once('=')
        {
            let group = validate_shaped_output_group(
                cfg,
                output,
                "--target",
                DerivedOutputShape::HtmlDirectory,
                &input_refs,
            )?;
            html_outputs.extend(group);
        }
    }
    let implicit = [
        ("--rag", "rag", DerivedOutputShape::RagDirectory),
        ("--context", "context", DerivedOutputShape::ContextDirectory),
        ("--llms", "llms.txt", DerivedOutputShape::LlmsFile),
        ("--rss", "rss.xml", DerivedOutputShape::RssFile),
        ("--no-rag", ".boris", DerivedOutputShape::IrDirectory),
    ];
    for (flag, output, shape) in implicit {
        if args.iter().any(|a| a == flag) {
            validate_shaped_output_group(cfg, output, flag, shape, &input_refs)?;
        }
    }
    validate_output_group_pairwise(&html_outputs, "HTML output roots")?;
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
        let input_refs = resolve_input_refs(&cfg.root, &cli_input_refs(args))?;
        validate_shaped_output_group(
            cfg,
            "dist",
            "default HTML output",
            DerivedOutputShape::HtmlDirectory,
            &input_refs,
        )?;
    }
    Ok(())
}

pub(super) fn validate_watch_output_paths(cfg: &EngineConfig) -> Result<()> {
    let input_refs = resolve_input_refs(&cfg.root, &cli_input_refs(&cfg.watch_args))?;
    validate_shaped_output_group(
        cfg,
        &cfg.html_dir,
        "watch HTML output",
        DerivedOutputShape::HtmlDirectory,
        &input_refs,
    )?;
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

#[derive(Clone, Copy)]
enum ProfileOutputKind {
    HtmlTarget,
    IrEdition,
    RagEdition,
    ContextEdition,
}

enum ProfileDeclaredOutput {
    /// Resolved against the directory containing the selected profile.
    ProfileRelative {
        output: String,
        kind: ProfileOutputKind,
    },
    /// Boris declares sitemap/rss/llms paths target-root-relative. Joining
    /// the declared target root is inspection of the written location, not a
    /// decision about whether the target is selected.
    TargetNested { target_output: String, path: String },
}

fn profile_derived_outputs(output: &str, kind: ProfileOutputKind) -> Vec<String> {
    match kind {
        ProfileOutputKind::HtmlTarget => [".boris-stage", ".boris-sitemap-prev"]
            .iter()
            .map(|suffix| format!("{output}{suffix}"))
            .collect(),
        ProfileOutputKind::IrEdition => vec![format!("{output}.boris-stage")],
        ProfileOutputKind::RagEdition => [".boris-rag-stage", ".boris-rag-next", ".boris-rag-prev"]
            .iter()
            .map(|suffix| format!("{output}{suffix}"))
            .collect(),
        ProfileOutputKind::ContextEdition => [".boris-context-stage", ".boris-context-prev"]
            .iter()
            .map(|suffix| format!("{output}{suffix}"))
            .collect(),
    }
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

struct InputRef {
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

fn push_layout_input_refs(input_refs: &mut Vec<InputRef>, layout: &str, kind: &'static str) {
    input_refs.push(InputRef {
        path: layout.to_string(),
        kind,
    });
    // Boris target validation treats both the declared layout file and its
    // parent directory as protected input roots. The parent tree can carry
    // sibling includes/assets even though the layout declaration itself names
    // one file.
    if let Some(parent) = layout_parent_path(layout) {
        input_refs.push(InputRef {
            path: parent.to_string(),
            kind: "layout parent tree",
        });
    }
}

fn push_theme_surface_refs(input_refs: &mut Vec<InputRef>, theme_root: &str) {
    input_refs.push(InputRef {
        path: format!("{theme_root}/footer.html"),
        kind: "theme footer file",
    });
    input_refs.push(InputRef {
        path: format!("{theme_root}/assets"),
        kind: "theme assets tree",
    });
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
) -> Result<(Option<String>, Vec<ProfileDeclaredOutput>, Vec<InputRef>)> {
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
            outputs.push(ProfileDeclaredOutput::ProfileRelative {
                output: target_output.clone(),
                kind: ProfileOutputKind::HtmlTarget,
            });
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
                push_theme_surface_refs(&mut input_refs, theme);
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
                push_theme_surface_refs(&mut input_refs, &root);
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
                        push_theme_surface_refs(&mut input_refs, &root);
                    }
                }
            }
            if let Some(static_value) = target.get("static") {
                let static_locus = format!("{locus}.static");
                let dir = required_string_field(profile, static_value, "dir", &static_locus)?;
                input_refs.push(InputRef {
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
                let kind = match edition {
                    "ir" => ProfileOutputKind::IrEdition,
                    "rag" => ProfileOutputKind::RagEdition,
                    _ => ProfileOutputKind::ContextEdition,
                };
                outputs.push(ProfileDeclaredOutput::ProfileRelative {
                    output: output.to_string(),
                    kind,
                });
            }
        }
    }
    Ok((input, outputs, input_refs))
}

fn validate_profile_output_and_inputs(
    cfg: &EngineConfig,
    base: &Path,
    raw: &str,
    what: &str,
    input_refs: &[(PathBuf, &str)],
) -> Result<PathBuf> {
    validate_profile_output_at_base(cfg, base, raw, what)?;
    let raw_parts = relative_parts(raw, what)?;
    let output_path = path_from_parts(base, &raw_parts);
    for (input_path, kind) in input_refs {
        validate_output_against_input_ref(&output_path, input_path, raw, what, kind)?;
    }
    Ok(output_path)
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
        input_refs.push((path_from_parts(base, &parts), input_ref.kind));
    }
    let mut output_roots: Vec<(String, PathBuf)> = Vec::new();
    let mut artifact_outputs: Vec<(String, PathBuf, String)> = Vec::new();
    for output in outputs {
        match output {
            ProfileDeclaredOutput::ProfileRelative { output, kind } => {
                let output_path = validate_profile_output_and_inputs(
                    &effective,
                    base,
                    &output,
                    "profile target output",
                    &input_refs,
                )?;
                output_roots.push((output.clone(), output_path));
                for derived in profile_derived_outputs(&output, kind) {
                    let derived_path = validate_profile_output_and_inputs(
                        &effective,
                        base,
                        &derived,
                        "profile derived stage output",
                        &input_refs,
                    )?;
                    output_roots.push((derived, derived_path));
                }
            }
            ProfileDeclaredOutput::TargetNested {
                target_output,
                path,
            } => {
                let output = format!("{target_output}/{path}");
                let artifact_path = validate_profile_output_and_inputs(
                    &effective,
                    base,
                    &output,
                    "profile artifact output",
                    &input_refs,
                )?;
                artifact_outputs.push((output, artifact_path, target_output));
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
    for (raw, path, owner) in &artifact_outputs {
        canonical_artifacts.push((
            raw.clone(),
            partition_parts(path, "profile artifact output")?,
            owner.clone(),
        ));
    }
    for (index, (left_raw, left, _)) in canonical_artifacts.iter().enumerate() {
        for (right_raw, right, _) in &canonical_artifacts[index + 1..] {
            if overlaps(left, right) {
                return Err(EngineError::Refused(format!(
                    "profile artifact outputs {left_raw:?} and {right_raw:?} physically overlap"
                )));
            }
        }
    }
    for (artifact_raw, artifact, owner) in &canonical_artifacts {
        for (root_raw, root) in &canonical_roots {
            if root_raw == owner {
                continue;
            }
            if overlaps(artifact, root) {
                return Err(EngineError::Refused(format!(
                    "profile artifact output {artifact_raw:?} physically overlaps output root {root_raw:?}"
                )));
            }
        }
    }
    Ok(())
}

pub(super) fn validate_verb_output_paths(cfg: &EngineConfig, verb: &Verb) -> Result<()> {
    if let Verb::Ir { out, input } = verb {
        let effective = output_cfg(cfg, input.as_deref())?;
        if out == crate::ir::IR_STAGING_DIR {
            validate_shaped_output_group(
                &effective,
                out,
                "IR output",
                DerivedOutputShape::IrDirectory,
                &[],
            )?;
        } else {
            validate_output_path_and_inputs(&effective, out, "IR output", &[])?;
        }
    }
    if let Verb::Init { dir, .. } = verb {
        validate_output_path(cfg, dir, "init target")?;
    }
    if let Verb::Export { kind } = verb {
        use cv_core::engine::ExportKind;
        let (output, shape) = match kind {
            ExportKind::Out => (".boris", DerivedOutputShape::IrDirectory),
            ExportKind::Rag => ("rag", DerivedOutputShape::RagDirectory),
            ExportKind::Context => ("context", DerivedOutputShape::ContextDirectory),
            ExportKind::Llms => ("llms.txt", DerivedOutputShape::LlmsFile),
            ExportKind::Rss => ("rss.xml", DerivedOutputShape::RssFile),
            ExportKind::Sitemap => ("dist", DerivedOutputShape::HtmlDirectory),
        };
        validate_shaped_output_group(cfg, output, "export output", shape, &[])?;
    }
    if let Verb::Custom { name, args } = verb {
        if name == "init" {
            validate_custom_init_targets(cfg, args)?;
        }
        validate_custom_outputs(cfg, name, args)?;
        validate_profile_declared_outputs(cfg, name, args)?;
        validate_default_html_output(cfg, name, args)?;
    }
    if matches!(verb, Verb::Build) {
        let input_refs = resolve_input_refs(&cfg.root, &cli_input_refs(&[]))?;
        validate_shaped_output_group(
            cfg,
            "dist",
            "build output",
            DerivedOutputShape::HtmlDirectory,
            &input_refs,
        )?;
    }
    Ok(())
}
