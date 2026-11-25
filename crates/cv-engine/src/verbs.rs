//! `Verb` → Boris argv, and one-shot execution with verbatim capture.
//! The mapping is a spelling table only: what a verb *means* is Boris's.

use crate::error::{EngineError, Result};
use crate::exit::ExitOutcome;
use crate::proc::{CancelToken, boris_command, run_captured};
use cv_core::diagnostic::StderrDiagnostic;
use cv_core::engine::{ExportKind, Verb};
use std::path::Path;
use std::time::Instant;

/// Workspace-relative directory the live session's IR projection writes.
/// Boris refuses an `--out` outside the workspace (`WorkspaceEscape`),
/// so OS app-data is not an option; `.cv/` is CV's own sidecar.
pub const IR_DIR: &str = ".cv/ir";

/// Does this verb write into the workspace? Mutating verbs are
/// serialized; the rest run in parallel. Unknown (`Custom`) verbs are
/// assumed mutating — serializing a read is safe, the reverse is not.
pub fn is_mutating(verb: &Verb) -> bool {
    !matches!(
        verb,
        Verb::Validate | Verb::Check | Verb::Plan | Verb::Graph
    )
}

pub(crate) fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().enumerate().find_map(|(i, a)| {
        if a == flag {
            args.get(i + 1).map(String::as_str)
        } else {
            a.strip_prefix(flag)?.strip_prefix('=')
        }
    })
}

/// Does this verb write the HTML output tree a live watch holds?
pub fn writes_html(verb: &Verb) -> bool {
    match verb {
        Verb::Build => true,
        Verb::Custom { name, args } => {
            name == "build"
                && flag_value(args, "--html-dir").is_none()
                && flag_value(args, "--out").is_none()
        }
        _ => false,
    }
}

/// The directory a verb writes instead of the HTML tree: `--out` (IR
/// mode) for `Ir`, `Export { Out }` and a custom `build --out`.
pub fn own_out_dir(verb: &Verb) -> Option<String> {
    match verb {
        Verb::Ir { out, .. } => Some(out.clone()),
        Verb::Export {
            kind: ExportKind::Out,
        } => Some(".boris".into()),
        Verb::Custom { name, args } if name == "build" => {
            flag_value(args, "--out").map(String::from)
        }
        _ => None,
    }
}

pub fn verb_name(verb: &Verb) -> String {
    verb.name()
}

fn looks_like_secret(arg: &str) -> bool {
    let a = arg.trim();
    a.starts_with("nsec1") || (a.len() == 64 && a.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Flags that receive secret material — blocked by name regardless of
/// the value's shape (`--token` with an innocuous value is still where a
/// credential would ride).
const SECRET_FLAGS: &[&str] = &[
    "--key",
    "--key-stdin",
    "--nsec",
    "--password",
    "--token",
    "--app-password",
    "--secret",
];

fn is_secret_flag(arg: &str) -> bool {
    let name = arg.split('=').next().unwrap_or(arg);
    SECRET_FLAGS.contains(&name)
}

/// The argv after the binary name.
pub fn argv(verb: &Verb) -> Result<Vec<String>> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect();
    Ok(match verb {
        Verb::Build => s(&["build"]),
        Verb::Validate => s(&["validate"]),
        Verb::Check => s(&["check"]),
        Verb::Plan => s(&["plan"]),
        Verb::Graph => s(&["graph"]),
        Verb::Export { kind } => match kind {
            ExportKind::Out => s(&["build", "--out", ".boris"]),
            ExportKind::Rag => s(&["build", "--rag"]),
            ExportKind::Context => s(&["build", "--context"]),
            ExportKind::Llms => s(&["build", "--llms"]),
            ExportKind::Rss => s(&["build", "--rss"]),
            ExportKind::Sitemap => s(&["build", "--sitemap"]),
        },
        Verb::Ir { out, input } => {
            let mut args = s(&["build", "--out", out]);
            if let Some(input) = input {
                args.extend(["--input".to_string(), input.clone()]);
            }
            args
        }
        // Publishing needs credentials/receipts owned by the publish lane;
        // nothing here may improvise it.
        Verb::Publish { target } => {
            return Err(EngineError::UnsupportedVerb(format!(
                "publish:{target} (publish lane owns delivery)"
            )));
        }
        Verb::Init { dir, archetype } => {
            // A leading dash would turn a folder or archetype name into a flag.
            if [dir, archetype]
                .iter()
                .any(|v| v.is_empty() || v.starts_with('-'))
            {
                return Err(EngineError::Refused(
                    "init needs a folder and an archetype, neither starting with '-'".into(),
                ));
            }
            vec![
                "init".into(),
                dir.clone(),
                "--type".into(),
                archetype.clone(),
            ]
        }
        Verb::Custom { name, args } => {
            if name.starts_with('-') {
                return Err(EngineError::Refused("verb name must not be a flag".into()));
            }
            if name == "nostr" && args.first().is_some_and(|a| a == "sign") {
                return Err(EngineError::UnsupportedVerb(
                    "nostr sign goes through cv_engine::secrets::nostr_sign (stdin-only key)"
                        .into(),
                ));
            }
            if args
                .iter()
                .any(|a| is_secret_flag(a) || looks_like_secret(a))
            {
                return Err(EngineError::Refused(
                    "argument looks like secret material; secrets never ride argv".into(),
                ));
            }
            std::iter::once(name.clone())
                .chain(args.iter().cloned())
                .collect()
        }
    })
}

/// A finished one-shot verb. stdout/stderr are verbatim.
#[derive(Debug, Clone)]
pub struct VerbOutput {
    pub outcome: ExitOutcome,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

impl VerbOutput {
    /// Parsed `error: ECODE: path:line:col: msg` lines. Lines that do not
    /// parse stay in `stderr` — consumers render that verbatim.
    pub fn diagnostics(&self) -> Vec<StderrDiagnostic> {
        self.stderr
            .lines()
            .filter_map(StderrDiagnostic::parse)
            .collect()
    }

    /// Count of `  wrote <path>.html` lines Boris prints (either stream).
    pub fn pages_written(&self) -> u64 {
        self.stdout
            .lines()
            .chain(self.stderr.lines())
            .filter(|l| l.starts_with("  wrote ") && l.ends_with(".html"))
            .count() as u64
    }
}

pub fn run_argv(
    bin: &Path,
    cwd: &Path,
    args: &[String],
    cancel: &CancelToken,
) -> Result<VerbOutput> {
    let mut cmd = boris_command(bin, cwd);
    cmd.args(args);
    let started = Instant::now();
    let c = run_captured(cmd, None, cancel)?;
    Ok(VerbOutput {
        outcome: c.outcome,
        stdout: String::from_utf8_lossy(&c.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&c.stderr).into_owned(),
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_are_classified() {
        for v in [Verb::Validate, Verb::Check, Verb::Plan, Verb::Graph] {
            assert!(!is_mutating(&v), "{v:?}");
        }
        for v in [
            Verb::Build,
            Verb::Export {
                kind: ExportKind::Rag,
            },
            Verb::Custom {
                name: "init".into(),
                args: vec![],
            },
        ] {
            assert!(is_mutating(&v), "{v:?}");
        }
        assert!(writes_html(&Verb::Build));
        assert!(!writes_html(&Verb::Export {
            kind: ExportKind::Out
        }));
        assert!(!writes_html(&Verb::Custom {
            name: "build".into(),
            args: vec!["--html-dir".into(), ".cv/out/1".into()],
        }));
    }

    #[test]
    fn ir_mode_builds_write_their_own_dir_not_the_html_tree() {
        let ir = Verb::Ir {
            out: IR_DIR.into(),
            input: Some("content".into()),
        };
        assert!(is_mutating(&ir));
        assert!(!writes_html(&ir));
        assert_eq!(own_out_dir(&ir).as_deref(), Some(IR_DIR));
        assert_eq!(
            argv(&ir).unwrap(),
            ["build", "--out", ".cv/ir", "--input", "content"]
        );
        for args in [["--out", "x"].as_slice(), ["--out=x"].as_slice()] {
            let custom = Verb::Custom {
                name: "build".into(),
                args: args.iter().map(|a| a.to_string()).collect(),
            };
            assert!(!writes_html(&custom), "{args:?}");
            assert_eq!(own_out_dir(&custom).as_deref(), Some("x"), "{args:?}");
        }
        assert_eq!(own_out_dir(&Verb::Build), None);
        assert_eq!(
            own_out_dir(&Verb::Export {
                kind: ExportKind::Out
            })
            .as_deref(),
            Some(".boris")
        );
    }

    #[test]
    fn secrets_never_ride_argv() {
        let sign = Verb::Custom {
            name: "nostr".into(),
            args: vec!["sign".into(), "--plan".into(), "p.json".into()],
        };
        assert!(matches!(argv(&sign), Err(EngineError::UnsupportedVerb(_))));
        let leaky = Verb::Custom {
            name: "x".into(),
            args: vec!["nsec1qqqqqqqqqq".into()],
        };
        assert!(matches!(argv(&leaky), Err(EngineError::Refused(_))));
        let hex = Verb::Custom {
            name: "x".into(),
            args: vec!["ab".repeat(32)],
        };
        assert!(matches!(argv(&hex), Err(EngineError::Refused(_))));
        // Secret-receiving flags are blocked by name, value-agnostic.
        for flag in [
            "--key",
            "--key-stdin",
            "--nsec",
            "--password",
            "--token",
            "--app-password",
            "--secret",
        ] {
            for a in [flag.to_string(), format!("{flag}=x")] {
                let v = Verb::Custom {
                    name: "x".into(),
                    args: vec![a.clone()],
                };
                assert!(
                    matches!(argv(&v), Err(EngineError::Refused(_))),
                    "{a} must be refused"
                );
            }
        }
        // A flag can't be a verb name.
        let flag_name = Verb::Custom {
            name: "--serve".into(),
            args: vec![],
        };
        assert!(matches!(argv(&flag_name), Err(EngineError::Refused(_))));
        assert!(
            argv(&Verb::Publish {
                target: "pages".into()
            })
            .is_err()
        );
    }

    #[test]
    fn exports_spell_boris_flags() {
        assert_eq!(
            argv(&Verb::Export {
                kind: ExportKind::Out
            })
            .unwrap(),
            ["build", "--out", ".boris"]
        );
        assert_eq!(argv(&Verb::Build).unwrap(), ["build"]);
    }

    #[test]
    fn init_spells_the_folder_and_archetype_and_refuses_flags() {
        let init = |dir: &str, archetype: &str| {
            argv(&Verb::Init {
                dir: dir.into(),
                archetype: archetype.into(),
            })
        };
        assert_eq!(
            init("site", "docs").unwrap(),
            ["init", "site", "--type", "docs"]
        );
        for (dir, archetype) in [
            ("--quiet", "docs"),
            ("site", "-x"),
            ("", "docs"),
            ("site", ""),
        ] {
            assert!(
                matches!(init(dir, archetype), Err(EngineError::Refused(_))),
                "{dir:?} {archetype:?}"
            );
        }
        assert!(is_mutating(&Verb::Init {
            dir: "site".into(),
            archetype: "docs".into()
        }));
    }
}
