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
        // Take line/col/message from the right so paths containing ':'
        // (Windows drive letters) survive.
        let (head, message) = loc.rsplit_once(':')?;
        let (head, col) = head.rsplit_once(':')?;
        let (path, line) = head.rsplit_once(':')?;
        Some(Self {
            level: level.to_string(),
            code: code.trim().to_string(),
            path: path.trim().to_string(),
            line: line.trim().parse().ok()?,
            col: col.trim().parse().ok()?,
            message: message.trim().to_string(),
        })
    }
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
    fn rejects_non_diagnostic_lines() {
        assert!(StderrDiagnostic::parse("ok: wrote HTML for 1 target(s):").is_none());
        assert!(StderrDiagnostic::parse("  wrote dist/index.html").is_none());
    }
}
