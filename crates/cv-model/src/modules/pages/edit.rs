//! Pure text and path helpers for page lifecycle commands. These are
//! structural frontmatter edits only: unknown keys and values are preserved,
//! no key set or scalar validity is judged here, and Boris remains the
//! authority that validates the result.

/// Lexical boundary validation for a `/`-separated, input-relative page
/// path. The filesystem port re-validates against links and host names.
pub fn valid_page_path(path: &str) -> Result<(), String> {
    if path.trim() != path || path.is_empty() {
        return Err("path is empty or has surrounding whitespace".into());
    }
    if path.starts_with('/') || path.contains('\\') {
        return Err("use a relative path with '/' separators".into());
    }
    if path
        .split('/')
        .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err("path cannot contain empty, '.' or '..' segments".into());
    }
    if path == ".cv" || path.starts_with(".cv/") {
        return Err(".cv is Capture Vector evidence, not Boris content".into());
    }
    if path.chars().any(char::is_control) {
        return Err("path cannot contain control characters".into());
    }
    Ok(())
}

/// New pages are Markdown sources in Boris's default page family.
pub fn valid_new_page_path(path: &str) -> Result<(), String> {
    valid_page_path(path)?;
    let markdown = path.ends_with(".md") || path.ends_with(".mdx");
    if !markdown || matches!(path, ".md" | ".mdx") {
        return Err("new pages are Markdown files ending in .md or .mdx".into());
    }
    Ok(())
}

/// A move may rename within the page family the source already has; it is
/// not a covert conversion to another source format.
pub fn valid_move_path(from: &str, to: &str) -> Result<(), String> {
    valid_page_path(to)?;
    fn ext(p: &str) -> Option<&str> {
        p.rsplit_once('.').map(|(_, e)| e)
    }
    match (ext(from), ext(to)) {
        (Some(a), Some(b)) if a == b => Ok(()),
        _ => Err("the moved page keeps the same file extension".into()),
    }
}

/// Boris's author-facing double-quoted scalar. Its dquoted form has no
/// escape sequences, so raw quotes and Unicode line separators cannot be
/// represented and are refused instead of rewritten.
pub fn quoted(what: &str, s: &str) -> Result<String, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err(format!("{what} is empty"));
    }
    if s.contains(['\n', '\r', '"', '\u{85}', '\u{2028}', '\u{2029}']) {
        return Err(format!("{what} cannot be represented in frontmatter"));
    }
    Ok(format!("\"{s}\""))
}

fn eol(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

fn fence(line: &str) -> bool {
    line.strip_suffix('\r').unwrap_or(line) == "---"
}

fn lines(text: &str) -> Vec<String> {
    text.split_inclusive('\n').map(str::to_string).collect()
}

fn close_fence(lines: &[String]) -> Option<usize> {
    if !lines
        .first()
        .is_some_and(|l| fence(l.trim_end_matches('\n')))
    {
        return None;
    }
    lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, l)| fence(l.trim_end_matches('\n')))
        .map(|(i, _)| i)
}

fn key_line(line: &str, key: &str) -> bool {
    !line.starts_with([' ', '\t']) && line.split_once(':').is_some_and(|(k, _)| k == key)
}

/// The unadorned value of the first top-level `key:` line. Surrounding
/// double quotes are removed; no escape semantics are invented.
pub fn frontmatter_value(text: &str, key: &str) -> Option<String> {
    let lines = lines(text);
    let close = close_fence(&lines)?;
    for raw in &lines[1..close] {
        let line = raw.trim_end_matches(['\n', '\r']);
        if key_line(line, key) {
            let v = line.split_once(':')?.1.trim();
            return Some(
                v.strip_prefix('"')
                    .and_then(|x| x.strip_suffix('"'))
                    .unwrap_or(v)
                    .to_string(),
            );
        }
    }
    None
}

/// Set or remove the first top-level `key` line, preserving every other
/// byte of the document outside that line. When there is no frontmatter,
/// a minimal closed fence is added. Unclosed fences are left to the
/// editor/Boris rather than repaired here.
pub fn set_frontmatter_key(text: &str, key: &str, value: Option<&str>) -> Result<String, String> {
    let nl = eol(text);
    let mut lines = lines(text);
    let starts = lines
        .first()
        .is_some_and(|l| fence(l.trim_end_matches('\n')));
    if !starts {
        return match value {
            Some(v) => Ok(format!("---{nl}{key}: {v}{nl}---{nl}{nl}{text}")),
            None => Ok(text.into()),
        };
    }
    let Some(close) = close_fence(&lines) else {
        return Err("frontmatter opening fence is not closed; edit it in the editor first".into());
    };
    let replacement = value.map(|v| format!("{key}: {v}{nl}"));
    let mut found = false;
    for line in lines.iter_mut().take(close).skip(1) {
        if key_line(line.trim_end_matches(['\n', '\r']), key) {
            found = true;
            *line = replacement.clone().unwrap_or_default();
            break;
        }
    }
    if !found && let Some(r) = replacement {
        lines.insert(close, r);
    }
    Ok(lines.concat())
}

/// Pin the effective id as an explicit `id:` key when the source does not
/// already carry one. This is what lets a path-derived StableId survive a
/// rename; an existing explicit id is never rewritten.
pub fn ensure_id(text: &str, id: &str) -> Result<String, String> {
    if frontmatter_value(text, "id").is_some() {
        return Ok(text.into());
    }
    let q = quoted("page id", id)?;
    set_frontmatter_key(text, "id", Some(&q))
}

/// Edit only the `parent:` key. `None` removes the line, making a Trunk.
pub fn set_parent(text: &str, parent: Option<&str>) -> Result<String, String> {
    let q = parent.map(|p| quoted("parent", p)).transpose()?;
    set_frontmatter_key(text, "parent", q.as_deref())
}

/// A Boris-conformant Markdown draft using only closed frontmatter keys.
/// No `id` is written: Boris derives the same id from the new page's path.
pub fn new_page(title: &str, parent: Option<&str>) -> Result<String, String> {
    let mut out = format!("---\ntitle: {}\n", quoted("title", title)?);
    if let Some(p) = parent {
        out.push_str(&format!("parent: {}\n", quoted("parent", p)?));
    }
    out.push_str("---\n\n# ");
    out.push_str(title.trim());
    out.push('\n');
    Ok(out)
}

/// Path-derived effective id (`guides/x.md` → `guides/x`).
pub fn path_id(path: &str) -> String {
    path.rsplit_once('.').map_or(path, |(stem, _)| stem).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_paths_are_relative_portable_and_inside_the_workspace() {
        for good in ["index.md", "guides/東京.md", "a.b/page.mdx"] {
            assert!(valid_page_path(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            " index.md",
            "/index.md",
            "guides\\index.md",
            "guides//index.md",
            "guides/./index.md",
            "../index.md",
            ".cv/page.md",
            "bad\u{7}.md",
        ] {
            assert!(valid_page_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn new_pages_are_markdown_and_moves_do_not_convert_formats() {
        assert!(valid_new_page_path("guides/new.md").is_ok());
        assert!(valid_new_page_path("guides/new.mdx").is_ok());
        for bad in ["new.txt", ".md", "new"] {
            assert!(valid_new_page_path(bad).is_err(), "{bad}");
        }
        assert!(valid_move_path("guides/a.md", "docs/b.md").is_ok());
        assert!(valid_move_path("guides/a.md", "docs/b.textile").is_err());
    }

    #[test]
    fn quoted_scalars_refuse_what_boris_cannot_represent() {
        assert_eq!(quoted("title", " A title ").unwrap(), "\"A title\"");
        for bad in ["", "has \"quote\"", "two\nlines"] {
            assert!(quoted("title", bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn setting_parent_preserves_every_other_byte() {
        let src =
            "---\ntitle: Guide\nx-unknown: kept\nparent: old\n\nbody: not a field\n---\n# Body\n";
        let out = set_parent(src, Some("index")).unwrap();
        assert_eq!(
            out,
            "---\ntitle: Guide\nx-unknown: kept\nparent: \"index\"\n\nbody: not a field\n---\n# Body\n"
        );
        let out = set_parent(src, None).unwrap();
        assert_eq!(
            out,
            "---\ntitle: Guide\nx-unknown: kept\n\nbody: not a field\n---\n# Body\n"
        );
    }

    #[test]
    fn crlf_files_keep_crlf_when_parent_is_inserted() {
        let out = set_parent("---\r\ntitle: Guide\r\n---\r\nbody\r\n", Some("index")).unwrap();
        assert_eq!(
            out,
            "---\r\ntitle: Guide\r\nparent: \"index\"\r\n---\r\nbody\r\n"
        );
    }

    #[test]
    fn missing_frontmatter_gets_a_minimal_closed_fence_and_unclosed_is_refused() {
        assert_eq!(
            set_parent("# Body\n", Some("index")).unwrap(),
            "---\nparent: \"index\"\n---\n\n# Body\n"
        );
        assert!(set_parent("---\ntitle: x\n# no close\n", Some("index")).is_err());
    }

    #[test]
    fn ensure_id_pins_only_a_missing_id() {
        let pinned = ensure_id("---\ntitle: Guide\n---\nbody", "guides/guide").unwrap();
        assert!(pinned.starts_with("---\ntitle: Guide\nid: \"guides/guide\"\n---\n"));
        let explicit = "---\nid: custom\ntitle: Guide\n---\nbody";
        assert_eq!(ensure_id(explicit, "guides/guide").unwrap(), explicit);
    }

    #[test]
    fn new_pages_use_closed_keys_and_leave_id_path_derived() {
        assert_eq!(
            new_page("Getting Started", Some("index")).unwrap(),
            "---\ntitle: \"Getting Started\"\nparent: \"index\"\n---\n\n# Getting Started\n"
        );
        assert!(!new_page("Solo", None).unwrap().contains("id:"));
    }

    #[test]
    fn path_ids_strip_one_extension() {
        assert_eq!(
            path_id("guides/getting-started.md"),
            "guides/getting-started"
        );
        assert_eq!(path_id("my.notes.mdx"), "my.notes");
    }
}
