//! Problems rail: diagnostics for one buffer, tagged by build.
//!
//! Policy (cv-core pin): a diagnostic is **fresh** only when it came from
//! the current build *and* the buffer still holds the bytes that build saw
//! (no edits since the saved revision the build ran over). Anything else is
//! **stale**: shown dimmed, grouped under "stale", never deleted and never
//! diff-remapped onto edited text. Navigation targets exist only for fresh
//! diagnostics: the exact byte span when the engine gave one, otherwise the
//! display line (stderr diagnostics carry `line`/`col` only).

use crate::buffer::Buffer;
use cv_core::diagnostic::{Diagnostic, Severity};
use cv_core::events::BuildId;
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    Stale,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub diagnostic: Diagnostic,
    pub freshness: Freshness,
    /// 0-based display line for the gutter, if the diagnostic has one.
    pub line: Option<usize>,
    /// Where a click moves the caret (fresh only).
    pub target: Option<Range<usize>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GutterMark {
    pub line: usize,
    pub severity: Severity,
    pub stale: bool,
}

/// The diagnostics the last build reported for one buffer.
#[derive(Debug, Clone, Default)]
pub struct Problems {
    build: Option<BuildId>,
    /// Buffer revision the build's bytes correspond to.
    built_revision: u64,
    items: Vec<Diagnostic>,
}

pub fn same_path(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.replace('\\', "/").trim_start_matches("./").to_string();
    norm(a) == norm(b)
}

impl Problems {
    /// Replace the set with `build`'s diagnostics for `buf`. Untagged
    /// diagnostics (stderr parses) are tagged with `build`.
    pub fn ingest(&mut self, build: BuildId, diags: &[Diagnostic], buf: &Buffer) {
        self.items = diags
            .iter()
            .filter(|d| {
                d.span
                    .as_ref()
                    .is_some_and(|s| same_path(&s.path, buf.path()))
            })
            .cloned()
            .map(|mut d| {
                d.build_version.get_or_insert_with(|| build.clone());
                d
            })
            .collect();
        self.build = Some(build);
        self.built_revision = buf.saved_revision();
    }

    pub fn build(&self) -> Option<&BuildId> {
        self.build.as_ref()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn freshness(&self, d: &Diagnostic, buf: &Buffer) -> Freshness {
        let current = d.build_version.is_some() && d.build_version == self.build;
        if current && buf.revision() == self.built_revision {
            Freshness::Fresh
        } else {
            Freshness::Stale
        }
    }

    /// Rail rows: fresh first, then the stale group, each in file order.
    pub fn rail(&self, buf: &Buffer) -> Vec<Problem> {
        let mut out: Vec<Problem> = self
            .items
            .iter()
            .map(|d| {
                let freshness = self.freshness(d, buf);
                let span = d.span.as_ref();
                let line = span.filter(|s| s.line > 0).map(|s| s.line as usize - 1);
                let target = match (freshness, span) {
                    (Freshness::Fresh, Some(s)) if s.end > s.start => {
                        let r = s.start as usize..s.end as usize;
                        (buf.is_boundary(r.start) && buf.is_boundary(r.end)).then_some(r)
                    }
                    (Freshness::Fresh, _) => line.and_then(|l| buf.line_range(l)),
                    (Freshness::Stale, _) => None,
                };
                Problem {
                    diagnostic: d.clone(),
                    freshness,
                    line,
                    target,
                }
            })
            .collect();
        out.sort_by_key(|p| {
            (
                p.freshness == Freshness::Stale,
                p.line.unwrap_or(usize::MAX),
            )
        });
        out
    }

    pub fn gutter(&self, buf: &Buffer) -> Vec<GutterMark> {
        self.rail(buf)
            .into_iter()
            .filter_map(|p| {
                Some(GutterMark {
                    line: p.line?,
                    severity: p.diagnostic.severity,
                    stale: p.freshness == Freshness::Stale,
                })
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "problems_tests.rs"]
mod tests;
