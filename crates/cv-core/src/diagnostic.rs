//! Diagnostics are sacred: exit codes and stderr are data and reach the
//! Activity module intact. Every diagnostic carries the `build_id` it was
//! computed against — stale policy (dim, or diff-remap into the new build)
//! is applied at render time, not at capture.

use crate::events::BuildId;
use serde::{Deserialize, Serialize};

/// Byte-offset span per the wave-0 pin: 0-based, half-open, over the
/// UTF-8 file bytes as stored on disk (CRLF counts). `line`/`col` are
/// 1-based display coordinates only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// Repo-relative path the diagnostic refers to (display/grouping key,
    /// never a `StableId`).
    pub path: String,
    pub start: u32,
    pub end: u32,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

/// A Boris diagnostic or a cv-rules finding-source diagnostic. The same
/// value renders identically in Activity and the Pages inspector — no
/// remapping.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Boris code (`EFRONTMATTER`, `EDUPLICATEID`, …) or an audit code.
    pub code: String,
    pub severity: Severity,
    pub span: Option<Span>,
    pub message: String,
    /// The build this diagnostic was computed against. Compare to the
    /// current `BuildId` to apply stale policy.
    pub build_version: Option<BuildId>,
}

/// One stderr line from Boris: `error: ECODE: path:line:col: message`.
/// Mirrors the *text* contract; the NDJSON watch stream carries the same
/// information structurally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StderrDiagnostic {
    pub level: String,
    pub code: String,
    pub path: String,
    pub line: u32,
    pub col: u32,
    pub message: String,
}

impl StderrDiagnostic {
    /// Parse `error: ECODE: index.md:4:1: message`. Returns `None` for
    /// non-diagnostic lines (`ok:`, `  wrote …`) — callers must surface
    /// those as raw output rather than dropping them.
    pub fn parse(line: &str) -> Option<Self> {
        let (level, rest) = line.split_once(':')?;
        if !matches!(level, "error" | "warning" | "note") {
            return None;
        }
        let (code, loc) = rest.trim_start().split_once(':')?;
        // Walk each ':' boundary left to right; the first one followed by
        // `<digits>:<digits>:` is the path:line:col: boundary. Leftmost
        // match keeps messages containing colon-digit text (`ratio 3:2:`)
        // intact and tolerates ':' inside paths (Windows drive letters).
        for (i, _) in loc.match_indices(':') {
            let Some(line) = parse_u32_field(&loc[i + 1..]) else {
                continue;
            };
            let after_line = &loc[i + 1 + digit_len(&loc[i + 1..]) + 1..];
            let Some(col) = parse_u32_field(after_line) else {
                continue;
            };
            let message = &after_line[digit_len(after_line) + 1..];
            return Some(Self {
                level: level.to_string(),
                code: code.trim().to_string(),
                path: loc[..i].trim().to_string(),
                line,
                col,
                message: message.trim().to_string(),
            });
        }
        None
    }
}

/// Leading run of ASCII digits as a `u32`, plus the colon that must
/// follow it; callers step past `digit_len + 1` for the next field.
fn digit_len(s: &str) -> usize {
    s.bytes().take_while(u8::is_ascii_digit).count()
}

fn parse_u32_field(s: &str) -> Option<u32> {
    let n = digit_len(s);
    (n > 0 && s.as_bytes().get(n) == Some(&b':'))
        .then(|| s[..n].parse().ok())
        .flatten()
}

impl From<&StderrDiagnostic> for Diagnostic {
    fn from(d: &StderrDiagnostic) -> Self {
        Diagnostic {
            code: d.code.clone(),
            severity: match d.level.as_str() {
                "error" => Severity::Error,
                "warning" => Severity::Warning,
                _ => Severity::Note,
            },
            span: Some(Span {
                path: d.path.clone(),
                start: 0,
                end: 0,
                line: d.line,
                col: d.col,
            }),
            message: d.message.clone(),
            build_version: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_boris_stderr_line() {
        let d = StderrDiagnostic::parse(
            "error: EFRONTMATTER: index.md:4:1: unsupported frontmatter key [Use only the supported keys]",
        )
        .unwrap();
        assert_eq!(d.code, "EFRONTMATTER");
        assert_eq!(d.path, "index.md");
        assert_eq!((d.line, d.col), (4, 1));
        assert!(d.message.starts_with("unsupported frontmatter key"));
    }

    #[test]
    fn keeps_colons_in_message() {
        let d = StderrDiagnostic::parse("error: EFRONTMATTER: index.md:4:1: expected a:b:c got x")
            .unwrap();
        assert_eq!((d.line, d.col), (4, 1));
        assert_eq!(d.message, "expected a:b:c got x");
    }

    #[test]
    fn message_colon_digit_pattern_stays_message() {
        let d =
            StderrDiagnostic::parse("error: EFRONTMATTER: index.md:4:1: ratio 3:2: bad").unwrap();
        assert_eq!(d.path, "index.md");
        assert_eq!(d.message, "ratio 3:2: bad");
    }

    #[test]
    fn tolerates_windows_drive_path() {
        let d =
            StderrDiagnostic::parse(r"error: EFRONTMATTER: C:\ws\content\index.md:4:1: bad key")
                .unwrap();
        assert_eq!(d.path, r"C:\ws\content\index.md");
        assert_eq!((d.line, d.col), (4, 1));
    }

    #[test]
    fn rejects_non_diagnostic_lines() {
        assert!(StderrDiagnostic::parse("ok: wrote HTML for 1 target(s):").is_none());
        assert!(StderrDiagnostic::parse("  wrote dist/index.html").is_none());
    }
}
