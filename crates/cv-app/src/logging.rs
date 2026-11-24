//! Structured JSONL logging for the composition root.
//!
//! `CV_LOG` selects the level (`error|warn|info|debug|trace`; unset or an
//! unknown value disables logging). `CV_LOG_FILE` appends to a file
//! instead of stderr. One JSON object is written per line:
//!
//! ```json
//! {"ts_ms":42,"level":"info","event":"workspace_open","root":"/tmp/w"}
//! ```
//!
//! Fields whose key looks like a secret (`secret`, `password`, `token`,
//! `nsec`, …) are dropped before serialization, so a careless call site
//! cannot leak a credential. Never log argv values, environment values, or
//! page bodies.

use serde_json::{Map, Value};
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// Log severity, lowest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl Level {
    /// Parse a `CV_LOG` value. Unknown values disable logging.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "error" => Some(Self::Error),
            "warn" | "warning" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            "trace" => Some(Self::Trace),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

enum Sink {
    Stderr,
    File(PathBuf),
}

struct Logger {
    max: Option<Level>,
    sink: Sink,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// Read `CV_LOG` / `CV_LOG_FILE` once. Safe to call more than once.
pub fn init_from_env() {
    let max = std::env::var("CV_LOG").ok().and_then(|v| Level::parse(&v));
    let sink = std::env::var_os("CV_LOG_FILE")
        .filter(|v| !v.is_empty())
        .map(|v| Sink::File(PathBuf::from(v)))
        .unwrap_or(Sink::Stderr);
    let _ = LOGGER.set(Logger { max, sink });
}

pub fn error(event: &str, fields: &[(&str, Value)]) {
    record(Level::Error, event, fields);
}

pub fn warn(event: &str, fields: &[(&str, Value)]) {
    record(Level::Warn, event, fields);
}

pub fn info(event: &str, fields: &[(&str, Value)]) {
    record(Level::Info, event, fields);
}

pub fn debug(event: &str, fields: &[(&str, Value)]) {
    record(Level::Debug, event, fields);
}

fn record(level: Level, event: &str, fields: &[(&str, Value)]) {
    let Some(logger) = LOGGER.get() else {
        return;
    };
    if !should_log(logger.max, level) {
        return;
    }
    let line = render(level, event, fields, now_ms());
    match &logger.sink {
        Sink::Stderr => {
            let _ = std::io::stderr().lock().write_all(line.as_bytes());
        }
        Sink::File(path) => match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            Ok(mut file) => {
                let _ = file.write_all(line.as_bytes());
            }
            Err(_) => {
                let _ = std::io::stderr().lock().write_all(line.as_bytes());
            }
        },
    }
}

fn should_log(max: Option<Level>, level: Level) -> bool {
    max.is_some_and(|max| level <= max)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn render(level: Level, event: &str, fields: &[(&str, Value)], now_ms: u64) -> String {
    let mut object = Map::new();
    object.insert("ts_ms".into(), Value::from(now_ms));
    object.insert("level".into(), Value::from(level.as_str()));
    object.insert("event".into(), Value::from(event));
    for (key, value) in fields {
        if !is_secret_key(key) {
            object.insert((*key).to_string(), value.clone());
        }
    }
    let mut line = serde_json::to_string(&Value::Object(object))
        .unwrap_or_else(|_| r#"{"level":"error","event":"log_encode_error"}"#.into());
    line.push('\n');
    line
}

fn is_secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("secret")
        || key.contains("password")
        || key.contains("passphrase")
        || key.contains("token")
        || key.contains("nsec")
        || key.contains("api_key")
        || key.contains("apikey")
        || key.contains("credential")
        || key.starts_with("private_key")
        || key == "authorization"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn levels_parse_and_order() {
        assert_eq!(Level::parse(" WARN "), Some(Level::Warn));
        assert_eq!(Level::parse("warning"), Some(Level::Warn));
        assert_eq!(Level::parse("verbose"), None);
        assert!(Level::Error < Level::Info);
        assert!(Level::Trace > Level::Debug);
        assert_eq!(Level::Info.as_str(), "info");
    }

    #[test]
    fn level_filter_admits_at_or_below_max() {
        assert!(should_log(Some(Level::Info), Level::Error));
        assert!(should_log(Some(Level::Info), Level::Info));
        assert!(!should_log(Some(Level::Info), Level::Debug));
        assert!(!should_log(None, Level::Error));
    }

    #[test]
    fn secret_keys_are_recognized() {
        for key in [
            "secret",
            "api_key",
            "CV_NOSTR_NSEC",
            "password",
            "access_token",
            "private_key",
            "authorization",
        ] {
            assert!(is_secret_key(key), "{key}");
        }
        for key in ["page", "root", "status", "key", "count"] {
            assert!(!is_secret_key(key), "{key}");
        }
    }

    #[test]
    fn render_is_one_json_object_and_drops_secrets() {
        let line = render(
            Level::Info,
            "workspace_open",
            &[
                ("root", json!("/tmp/w")),
                ("page", json!("index.md")),
                ("api_key", json!("leak")),
                ("password", json!("leak")),
            ],
            42,
        );
        assert!(line.ends_with('\n'));
        let value: Value = serde_json::from_str(line.trim()).expect("valid JSON");
        assert_eq!(value["ts_ms"], json!(42));
        assert_eq!(value["level"], json!("info"));
        assert_eq!(value["event"], json!("workspace_open"));
        assert_eq!(value["root"], json!("/tmp/w"));
        assert_eq!(value["page"], json!("index.md"));
        assert!(value.get("api_key").is_none());
        assert!(value.get("password").is_none());
    }
}
