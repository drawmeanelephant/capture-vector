//! The support bundle: every Doctor section as one JSON document a user
//! can paste into an issue. It follows the logging redaction rules
//! (`cv_core::redact`) and goes further, because it leaves the machine:
//!
//! - no page bodies and no frontmatter values;
//! - no engine paths or search-order paths: they are environment values
//!   (`PATH`, `CV_BORIS_BIN`), so only the binary's file name and source
//!   are kept;
//! - every string has the workspace root and home directory replaced by
//!   placeholders ([`Facts::redact`]);
//! - fields whose key looks like a secret are dropped, at any depth.

use super::{EngineState, Facts, LockHolder, Stamp, WatchLog, pin_matches};
use cv_core::redact::drop_secret_keys;
use serde_json::{Value, json};

pub const FORMAT: &str = "cv-doctor-bundle";
pub const SCHEMA_VERSION: &str = "1";

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn engine(f: &Facts) -> Value {
    let binary = f.engine.path().map(file_name);
    let source = f.engine.source();
    let mut v = match &f.engine {
        EngineState::NotStarted => json!({ "state": "not-started" }),
        EngineState::Locating => json!({ "state": "locating" }),
        EngineState::Fixture {
            corpus,
            version,
            vcs_revision,
        } => json!({
            "state": "fixture",
            "corpus": corpus,
            "version": version,
            "vcsRevision": vcs_revision,
        }),
        EngineState::Located(l) => {
            let build_info = serde_json::from_str::<Value>(&l.build_info_raw)
                .unwrap_or_else(|_| Value::String("<not JSON>".into()));
            json!({
                "state": "located",
                "version": l.version,
                "vcsRevision": l.vcs_revision,
                "buildInfo": build_info,
            })
        }
        EngineState::NotFound => json!({ "state": "not-found" }),
        EngineState::OverrideMissing { .. } => json!({ "state": "override-missing" }),
        EngineState::VersionMismatch {
            found, found_vcs, ..
        } => json!({
            "state": "version-mismatch",
            "found": found,
            "foundVcsRevision": found_vcs,
        }),
        EngineState::BuildInfoInvalid { raw, .. } => json!({
            "state": "build-info-invalid",
            "buildInfoBytes": raw.len(),
        }),
        EngineState::Failed { error } => json!({ "state": "failed", "error": error }),
    };
    v["binary"] = json!(binary);
    v["source"] = json!(source);
    v["pin"] = json!({ "version": f.pin.version, "vcsRevision": f.pin.vcs_revision });
    v["pinMatches"] = json!(pin_matches(f));
    v["candidatesSearched"] = json!(f.candidates.len());
    v
}

fn probe(f: &Facts) -> Value {
    match &f.probe {
        None => Value::Null,
        Some(p) => json!({
            "os": p.os,
            "caps": p.caps,
            "watchMode": if p.native_watch { "native" } else { "one-shot-fallback" },
            "evidence": p.evidence,
        }),
    }
}

fn watch(w: &WatchLog) -> Value {
    json!({
        "live": w.live(),
        "dead": w.dead,
        "session": w.session,
        "schema": w.schema,
        "compiler": w.compiler,
        "lastCursor": w.last_cursor.as_ref().map(|c| json!({ "session": c.session.0, "seq": c.seq })),
        "engineEvents": w.engine_events,
        "ended": w.ended.as_ref().map(|r| format!("{r:?}")),
        "lastVerb": w.last_verb.as_ref().map(|(verb, exit)| json!({ "verb": verb, "exit": exit })),
        "lastFatal": w.fatal.as_ref().map(|f| json!({
            "phase": f.phase,
            "fromFollowupBuild": f.from_followup,
            "diagnostics": f.diagnostics.iter().map(|d| json!({
                "code": d.code,
                "severity": d.severity,
                "path": d.span.as_ref().map(|s| &s.path),
                "line": d.span.as_ref().map(|s| s.line),
                "col": d.span.as_ref().map(|s| s.col),
                "message": d.message,
            })).collect::<Vec<_>>(),
        })),
        "degraded": w.degraded,
    })
}

fn store(f: &Facts) -> Value {
    let namespaces: Vec<Value> = f
        .store
        .namespaces
        .iter()
        .map(|ns| {
            let stamp = match &ns.stamp {
                Stamp::Absent => json!("absent"),
                Stamp::Unstamped => json!(0),
                Stamp::Version(v) => json!(v),
                Stamp::Unreadable(e) => json!({ "unreadable": e }),
            };
            json!({
                "namespace": ns.name,
                "dir": ns.dir,
                "stamp": stamp,
                "supported": ns.supported,
                "lock": ns.lock.as_ref().map(|l| json!({
                    "path": l.path,
                    "holder": match l.holder {
                        LockHolder::ThisWindow => "this-window",
                        LockHolder::OtherProcess => "other-process",
                    },
                })),
                "hasIndex": ns.index.is_some(),
                "error": ns.error,
            })
        })
        .collect();
    json!({ "workspaceOpen": f.store.root.is_some(), "namespaces": namespaces })
}

fn scrub(v: &mut Value, redact: &[(String, String)]) {
    match v {
        Value::String(s) => {
            for (from, to) in redact.iter().filter(|(from, _)| !from.is_empty()) {
                if s.contains(from.as_str()) {
                    *s = s.replace(from.as_str(), to);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| scrub(x, redact)),
        Value::Object(m) => m.values_mut().for_each(|x| scrub(x, redact)),
        _ => {}
    }
}

/// The bundle as a value (tests inspect it; [`render`] prints it).
pub fn build(f: &Facts, w: &WatchLog) -> Value {
    let a = &f.app;
    let mut v = json!({
        "format": FORMAT,
        "schemaVersion": SCHEMA_VERSION,
        "app": {
            "version": a.version,
            "vcsRevision": a.vcs_revision,
            "target": a.target,
            "profile": a.profile,
            "toolchain": a.toolchain,
            "logging": a.logging,
        },
        "engine": engine(f),
        "probe": probe(f),
        "watch": watch(w),
        "store": store(f),
    });
    drop_secret_keys(&mut v);
    // Longest prefix first, so a root under the home directory becomes
    // `<workspace>` rather than `~/…`.
    let mut redact = f.redact.clone();
    // Error text can quote the engine or a search path verbatim.
    redact.extend(
        f.engine
            .path()
            .into_iter()
            .chain(f.candidates.iter().map(|(_, p)| p.as_str()))
            .map(|p| (p.to_string(), format!("<engine:{}>", file_name(p)))),
    );
    redact.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
    scrub(&mut v, &redact);
    v
}

pub fn render(f: &Facts, w: &WatchLog) -> String {
    let mut s = serde_json::to_string_pretty(&build(f, w)).unwrap_or_default();
    s.push('\n');
    s
}
