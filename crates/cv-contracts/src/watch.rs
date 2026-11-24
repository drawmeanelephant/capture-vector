//! `boris watch --watch-json` NDJSON on **stderr**. First record must be
//! `hello` with `watch_events_schema == 1` or the child is killed.
//! Unknown event variants decode to `Unknown` — additive upstream drift
//! never crashes the stream.

use serde::{Deserialize, Serialize};

/// One line of the watch NDJSON stream.
#[derive(Debug, Clone, PartialEq)]
pub enum WatchEvent {
    Hello {
        watch_events_schema: u32,
        compiler: String,
    },
    BuildStarted {
        phase: Phase,
        mode: String,
        targets: Vec<String>,
        changed: Vec<String>,
    },
    BuildSucceeded {
        phase: Phase,
        mode: String,
        targets: Vec<String>,
        pages_written: u64,
        duration_ms: u64,
    },
    BuildFailed {
        phase: Phase,
        mode: String,
        targets: Vec<String>,
        errors: u64,
        diagnostics: Vec<WatchDiagnostic>,
        recoverable: bool,
        duration_ms: u64,
    },
    WatcherStarted {
        mode: String,
        targets: Vec<String>,
    },
    /// Additive upstream event we don't model yet — `event` name and the
    /// raw payload are retained verbatim so drift is diagnosable, never
    /// silently dropped.
    Unknown {
        event: String,
        payload: serde_json::Map<String, serde_json::Value>,
    },
}

/// Serde-shaped dispatch mirror of `WatchEvent`'s known variants.
/// `#[serde(other)]` only exists for the unknown-tag fallback; the
/// public enum is built from this so `Unknown` can carry its payload.
#[derive(Deserialize)]
#[serde(tag = "event")]
enum Tagged {
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
    #[serde(other)]
    Unknown,
}

impl<'de> Deserialize<'de> for WatchEvent {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        let tagged: Tagged =
            serde_json::from_value(value.clone()).map_err(serde::de::Error::custom)?;
        Ok(match tagged {
            Tagged::Hello {
                watch_events_schema,
                compiler,
            } => Self::Hello {
                watch_events_schema,
                compiler,
            },
            Tagged::BuildStarted {
                phase,
                mode,
                targets,
                changed,
            } => Self::BuildStarted {
                phase,
                mode,
                targets,
                changed,
            },
            Tagged::BuildSucceeded {
                phase,
                mode,
                targets,
                pages_written,
                duration_ms,
            } => Self::BuildSucceeded {
                phase,
                mode,
                targets,
                pages_written,
                duration_ms,
            },
            Tagged::BuildFailed {
                phase,
                mode,
                targets,
                errors,
                diagnostics,
                recoverable,
                duration_ms,
            } => Self::BuildFailed {
                phase,
                mode,
                targets,
                errors,
                diagnostics,
                recoverable,
                duration_ms,
            },
            Tagged::WatcherStarted { mode, targets } => Self::WatcherStarted { mode, targets },
            Tagged::Unknown => Self::Unknown {
                event: value
                    .get("event")
                    .and_then(|e| e.as_str())
                    .unwrap_or_default()
                    .to_string(),
                payload: value.as_object().cloned().unwrap_or_default(),
            },
        })
    }
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
        let WatchEvent::Unknown { event, payload } = ev else {
            panic!("unknown event must decode to Unknown, got {ev:?}")
        };
        assert_eq!(event, "future-event");
        assert_eq!(payload.get("x"), Some(&serde_json::json!(1)));
    }
}
