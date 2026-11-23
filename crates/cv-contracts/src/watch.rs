//! `boris watch --watch-json` NDJSON on **stderr**. First record must be
//! `hello` with `watch_events_schema == 1` or the child is killed.
//! Unknown event variants decode to `Unknown` — additive upstream drift
//! never crashes the stream.

use serde::{Deserialize, Serialize};

/// One line of the watch NDJSON stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum WatchEvent {
    #[serde(rename = "hello")]
    Hello {
        watch_events_schema: u32,
        compiler: String,
    },
    #[serde(rename = "build-started")]
    BuildStarted {
        phase: Phase,
        mode: String,
        targets: Vec<String>,
        #[serde(default)]
        changed: Vec<String>,
    },
    #[serde(rename = "build-succeeded")]
    BuildSucceeded {
        phase: Phase,
        mode: String,
        targets: Vec<String>,
        pages_written: u64,
        duration_ms: u64,
    },
    #[serde(rename = "build-failed")]
    BuildFailed {
        phase: Phase,
        mode: String,
        targets: Vec<String>,
        errors: u64,
        diagnostics: Vec<WatchDiagnostic>,
        recoverable: bool,
        duration_ms: u64,
    },
    #[serde(rename = "watcher-started")]
    WatcherStarted { mode: String, targets: Vec<String> },
    /// Additive upstream event we don't model yet — kept, not dropped.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Initial,
    Rebuild,
}

/// Structured diagnostic inside `build-failed` (and future events).
/// Fields per upstream watch schema 1; tolerant of additions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct WatchDiagnostic {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub col: Option<u32>,
    #[serde(default)]
    pub message: Option<String>,
    /// Forward tolerance for fields added upstream.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Decode one NDJSON line. Blank lines return `Ok(None)`; malformed JSON
/// is an `Err` the caller must surface (never tolerate silently).
pub fn decode_line(line: &str) -> serde_json::Result<Option<WatchEvent>> {
    if line.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(line).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_event_variants_decode() {
        let ev: WatchEvent = serde_json::from_str(r#"{"event":"future-event","x":1}"#).unwrap();
        assert_eq!(ev, WatchEvent::Unknown);
    }
}
