//! DemoModule — the wave-0 canary module. Renders the replayed watch
//! session as a mailbox, carries a synthetic 5,000-page list for the
//! virtualization envelope, and exercises every degraded state via
//! palette verbs. Synthetic rows are UI load data, not Boris output.

use crate::command::{CommandSpec, Intent};
use crate::module::{
    Chip, InspectorModel, Module, ModuleSpec, NavItem, NavModel, ReadingLine, ReadingModel,
    Registry, RowModel, Slot, Tone,
};
use cv_core::diagnostic::Diagnostic;
use cv_core::events::engine::{BuildPhase, EngineEvent};
use cv_core::events::shell::DegradedReason;
use cv_core::events::{AppEvent, Envelope};
use cv_core::id::{AccountId, NounId, NounKind, StableId, noun_kind};

pub const ID: &str = "demo";
/// The 5k-page virtualization envelope.
pub const SYNTHETIC_PAGES: usize = 5_000;

pub fn register(r: &mut Registry) {
    r.register(Box::new(DemoModule::default()))
        .expect("demo module id is unique");
}

#[derive(Debug, Clone)]
struct EventRow {
    key: String,
    seq: u64,
    title: String,
    detail: String,
    tone: Tone,
    lines: Vec<ReadingLine>,
}

#[derive(Debug, Default)]
pub struct DemoModule {
    events: Vec<EventRow>,
}

fn phase(p: &BuildPhase) -> &'static str {
    match p {
        BuildPhase::Initial => "initial",
        BuildPhase::Rebuild => "rebuild",
    }
}

fn line(label: &str, text: impl Into<String>) -> ReadingLine {
    ReadingLine {
        label: Some(label.into()),
        text: text.into(),
        ..ReadingLine::default()
    }
}

fn diag_lines(ds: &[Diagnostic]) -> Vec<ReadingLine> {
    ds.iter()
        .map(|d| ReadingLine {
            label: Some(d.code.clone()),
            text: match &d.span {
                Some(s) => format!("{}:{}:{} {}", s.path, s.line, s.col, d.message),
                None => d.message.clone(),
            },
            tone: Tone::Error,
            mono: true,
        })
        .collect()
}

fn describe(ev: &EngineEvent) -> (String, String, Tone, Vec<ReadingLine>) {
    match ev {
        EngineEvent::WatchSessionStarted { schema, compiler } => (
            "Watch session started".into(),
            compiler.clone(),
            Tone::Ok,
            vec![
                line("compiler", compiler.clone()),
                line("schema", schema.to_string()),
            ],
        ),
        EngineEvent::BuildFinished {
            phase: p,
            pages_written,
            diagnostics,
            duration_ms,
        } => {
            let mut l = vec![
                line("phase", phase(p)),
                line("pages", pages_written.to_string()),
                line("duration", format!("{duration_ms} ms")),
            ];
            l.extend(diag_lines(diagnostics));
            (
                format!("Build succeeded ({})", phase(p)),
                format!("{pages_written} pages in {duration_ms} ms"),
                Tone::Ok,
                l,
            )
        }
        EngineEvent::BuildFailed {
            phase: p,
            diagnostics,
            recoverable,
        } => {
            let mut l = vec![
                line("phase", phase(p)),
                line("recoverable", recoverable.to_string()),
            ];
            l.extend(diag_lines(diagnostics));
            (
                format!("Build failed ({})", phase(p)),
                format!("{} diagnostic(s)", diagnostics.len()),
                Tone::Error,
                l,
            )
        }
        EngineEvent::WatchSessionEnded { reason } => (
            "Watch session ended".into(),
            format!("{reason:?}"),
            Tone::Warn,
            vec![line("reason", format!("{reason:?}"))],
        ),
        EngineEvent::VerbFinished { verb, exit } => (
            format!("{verb} finished"),
            format!("{exit:?}"),
            Tone::Normal,
            vec![
                line("verb", verb.clone()),
                line("exit", format!("{exit:?}")),
            ],
        ),
        EngineEvent::MalformedLine { raw } => (
            "Malformed engine line".into(),
            raw.clone(),
            Tone::Warn,
            vec![ReadingLine {
                label: Some("raw".into()),
                text: raw.clone(),
                tone: Tone::Warn,
                mono: true,
            }],
        ),
    }
}

fn page_key(i: usize) -> String {
    format!("page-{:04}", i + 1)
}

const DEGRADED: [(&str, &str); 4] = [
    ("engine-absent", "Simulate: engine absent"),
    ("version-mismatch", "Simulate: engine version mismatch"),
    ("watch-dead", "Simulate: watch died mid-session"),
    ("store-corrupt", "Simulate: corrupt store"),
];

impl Module for DemoModule {
    fn spec(&self) -> ModuleSpec {
        ModuleSpec {
            id: ID,
            title: "Demo",
            slot: Slot::Extra,
        }
    }

    fn ingest(&mut self, env: &Envelope) -> bool {
        let AppEvent::Engine(ev) = &env.payload else {
            return false;
        };
        let (title, detail, tone, mut lines) = describe(ev);
        lines.push(ReadingLine {
            label: Some("event".into()),
            text: env.event_id.0.clone(),
            tone: Tone::Dim,
            mono: true,
        });
        self.events.push(EventRow {
            key: format!("ev-{}", env.event_id.0),
            seq: env.cursor.seq,
            title,
            detail,
            tone,
            lines,
        });
        true
    }

    fn nav(&self) -> NavModel {
        NavModel {
            items: vec![
                NavItem {
                    key: "session".into(),
                    label: "Watch session".into(),
                    count: Some(self.events.len() as u64),
                },
                NavItem {
                    key: "pages".into(),
                    label: "Synthetic pages".into(),
                    count: Some(SYNTHETIC_PAGES as u64),
                },
            ],
        }
    }

    fn list_len(&self, nav: Option<&str>) -> usize {
        match nav {
            Some("pages") => SYNTHETIC_PAGES,
            _ => self.events.len(),
        }
    }

    fn row(&self, nav: Option<&str>, i: usize) -> RowModel {
        match nav {
            Some("pages") => RowModel {
                key: page_key(i),
                title: format!("Synthetic page {:04}", i + 1),
                detail: format!("content/synthetic/{}.md", page_key(i)),
                badge: i.is_multiple_of(97).then(|| Chip::new("stale", Tone::Warn)),
                tone: if i.is_multiple_of(97) {
                    Tone::Warn
                } else {
                    Tone::Normal
                },
                rail: None,
                path: false,
            },
            _ => self
                .events
                .get(i)
                .map(|e| RowModel {
                    key: e.key.clone(),
                    title: e.title.clone(),
                    detail: e.detail.clone(),
                    badge: Some(Chip::new(format!("#{}", e.seq), Tone::Normal)),
                    tone: e.tone,
                    rail: None,
                    path: false,
                })
                .unwrap_or_default(),
        }
    }

    fn index_of_key(&self, nav: Option<&str>, key: &str) -> Option<usize> {
        match nav {
            Some("pages") => {
                let n: usize = key.strip_prefix("page-")?.parse().ok()?;
                let i = n.checked_sub(1)?;
                (i < SYNTHETIC_PAGES && page_key(i) == key).then_some(i)
            }
            _ => self.events.iter().position(|e| e.key == key),
        }
    }

    fn reading(&self, key: &str) -> ReadingModel {
        if let Some(e) = self.events.iter().find(|e| e.key == key) {
            return ReadingModel {
                title: e.title.clone(),
                lines: e.lines.clone(),
                ..Default::default()
            };
        }
        if let Some(n) = key.strip_prefix("page-") {
            return ReadingModel {
                title: format!("Synthetic page {n}"),
                lines: vec![
                    line("path", format!("content/synthetic/{key}.md")),
                    ReadingLine {
                        text: "Synthetic row for the 5,000-page virtualization envelope.".into(),
                        tone: Tone::Dim,
                        ..ReadingLine::default()
                    },
                ],
                ..Default::default()
            };
        }
        ReadingModel::default()
    }

    fn inspector(&self, key: &str) -> InspectorModel {
        let mut fields = vec![("key".to_string(), key.to_string())];
        if let Some(e) = self.events.iter().find(|e| e.key == key) {
            fields.push(("seq".into(), e.seq.to_string()));
            fields.push(("tone".into(), format!("{:?}", e.tone).to_lowercase()));
        } else {
            fields.push(("kind".into(), "page".into()));
        }
        InspectorModel {
            fields,
            ..Default::default()
        }
    }

    fn noun(&self, key: &str) -> Option<NounId> {
        Some(NounId {
            account: AccountId("demo".into()),
            kind: NounKind::register(if key.starts_with("page-") {
                noun_kind::PAGE
            } else {
                noun_kind::EVENT
            }),
            stable: StableId(key.into()),
        })
    }

    fn commands(&self) -> Vec<CommandSpec> {
        let mut c: Vec<_> = DEGRADED
            .iter()
            .map(|(v, t)| CommandSpec::new(format!("demo.degrade.{v}"), *t))
            .collect();
        c.push(CommandSpec::new("demo.edit", "Demo: mark window dirty"));
        c
    }

    fn run(&mut self, command: &str) -> Vec<Intent> {
        let reason = match command.strip_prefix("demo.degrade.") {
            Some("engine-absent") => DegradedReason::EngineAbsent,
            Some("version-mismatch") => DegradedReason::EngineVersionMismatch {
                found: "0.8.2".into(),
                want: crate::cascade::WANT_ENGINE.into(),
            },
            Some("watch-dead") => DegradedReason::WatchDead,
            Some("store-corrupt") => DegradedReason::StoreCorrupt,
            _ if command == "demo.edit" => return vec![Intent::MarkDirty],
            _ => return vec![],
        };
        vec![Intent::Degrade(reason)]
    }
}
