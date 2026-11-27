use super::*;

fn stamp_badge(ns: &Namespace) -> (String, Chip, Tone) {
    let s = ns.supported;
    match &ns.stamp {
        Stamp::Absent => (
            "not stamped".into(),
            Chip::new("fresh", Tone::Dim),
            Tone::Normal,
        ),
        Stamp::Unstamped => (
            "v0".into(),
            Chip::new("unstamped", Tone::Warn),
            Tone::Normal,
        ),
        Stamp::Version(v) if *v > s => (
            format!("v{v} > v{s}"),
            Chip::new("too new", Tone::Error),
            Tone::Error,
        ),
        Stamp::Version(v) if *v < s => (
            format!("v{v} → v{s}"),
            Chip::new("older", Tone::Warn),
            Tone::Normal,
        ),
        Stamp::Version(v) => (
            format!("schema v{v}"),
            Chip::new("current", Tone::Ok),
            Tone::Normal,
        ),
        Stamp::Unreadable(e) => (e.clone(), Chip::new("unreadable", Tone::Error), Tone::Error),
    }
}

fn imports_items(h: &ImportsHealth) -> Vec<Item> {
    let runs = if let Some(e) = &h.scan_error {
        Item::new("store.imports.runs", "Import runs", "scan failed")
            .tone(Tone::Error)
            .badge("error", Tone::Error)
            .line("scan error", e.clone())
    } else {
        let n = h.runs.unwrap_or_default();
        let mut i = Item::new(
            "store.imports.runs",
            "Import runs",
            match n {
                0 => "none".to_string(),
                1 => "1 run".to_string(),
                n => format!("{n} runs"),
            },
        )
        .say("Run directories under .cv/imports/, read without taking their run locks. Symlinked .cv or imports paths are refused rather than followed.");
        if n == 0 {
            i = i.tone(Tone::Dim);
        }
        if h.unreadable_runs > 0 {
            i = i
                .tone(Tone::Warn)
                .badge("degraded", Tone::Warn)
                .line("unreadable run.json", h.unreadable_runs.to_string());
        }
        i
    };

    let malformed = h.undecodable_review_lines + h.unsupported_review_lines;
    let review = if let Some(e) = &h.review_error {
        Item::new("store.imports.review", "Review log", "unreadable")
            .tone(Tone::Error)
            .badge("error", Tone::Error)
            .line("read error", e.clone())
    } else {
        let mut i = Item::new(
            "store.imports.review",
            "Review log",
            match h.review_lines {
                0 => "no decisions".to_string(),
                1 => "1 line".to_string(),
                n => format!("{n} lines"),
            },
        )
        .say("review.jsonl is append-only human-review evidence. Malformed or too-new lines are counted here and skipped by readers; nothing is rewritten in place.");
        if h.review_lines == 0 {
            i = i.tone(Tone::Dim);
        }
        if malformed > 0 || h.torn_review_bytes > 0 {
            i = i.tone(Tone::Warn).badge("degraded", Tone::Warn);
        }
        if h.undecodable_review_lines > 0 {
            i = i.line("undecodable", h.undecodable_review_lines.to_string());
        }
        if h.unsupported_review_lines > 0 {
            i = i.line("unsupported", h.unsupported_review_lines.to_string());
        }
        if h.torn_review_bytes > 0 {
            i = i.line("torn tail bytes", h.torn_review_bytes.to_string());
        }
        i
    };
    vec![runs, review]
}

pub(super) fn store_items(f: &Facts) -> Vec<Item> {
    let Some(root) = &f.store.root else {
        return vec![
            Item::new("store.none", "No workspace open", "nothing kept in .cv/")
                .tone(Tone::Dim)
                .say("The fixture demo reads the checkout's captures and keeps no sidecar state."),
        ];
    };
    let mut out = vec![Item::new("store.root", "Workspace", root.clone()).path()];
    for ns in &f.store.namespaces {
        let (value, chip, tone) = stamp_badge(ns);
        let mut i = Item::new(format!("store.ns.{}", ns.name), ns.dir.clone(), value)
            .tone(tone)
            .line("stamp", format!("{}/schema.json", ns.dir))
            .line("reads", format!("up to v{}", ns.supported));
        i.badge = Some(chip);
        if let Some(e) = &ns.error {
            i = i
                .tone(Tone::Error)
                .badge("error", Tone::Error)
                .line("open error", e.clone());
        }
        out.push(i);
        if let Some(l) = &ns.lock {
            let lock = Item::new(format!("store.lock.{}", ns.name), l.path.clone(), "");
            out.push(match l.holder {
                LockHolder::ThisWindow => Item {
                    value: "this window".into(),
                    ..lock
                },
                LockHolder::OtherProcess => Item {
                    value: "another process".into(),
                    ..lock
                }
                .tone(Tone::Error)
                .badge("locked", Tone::Error)
                .say("Another Capture Vector has this workspace open. Close it (or end a stuck one), then reopen the workspace."),
                LockHolder::NotHeld => Item {
                    value: "not held".into(),
                    ..lock
                }
                .tone(Tone::Dim),
                LockHolder::Unknown => Item {
                    value: "unknown".into(),
                    ..lock
                }
                .tone(Tone::Warn)
                .say("The lock exists, but Doctor could not determine its holder."),
            });
        }
        if let Some(ix) = &ns.index {
            out.push(
                Item::new(format!("store.index.{}", ns.name), format!("{} index", ns.name), ix.clone())
                    .path()
                    .say(format!(
                        "Rebuildable: the JSONL under {} is the truth. The index rebuilds on open whenever it is missing, corrupt or stale; to force a rebuild, run Rebuild local indexes from .cv JSONL from the command palette (mod+K). Only if that action reports failure: quit, delete this file, and reopen the workspace.",
                        ns.dir
                    )),
            );
        }
    }
    if let Some(h) = &f.store.imports {
        out.extend(imports_items(h));
    }
    out
}
