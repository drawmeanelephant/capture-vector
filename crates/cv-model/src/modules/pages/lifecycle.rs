//! Page lifecycle commands and their dialog flow. The module owns the
//! state transition; the injected port owns IO, fingerprinting and
//! serialized engine writes.

use super::{NO_PARENT, PagesModule, edit};
use crate::command::{CommandSpec, Intent};
use crate::dialog::{ActionTone, CHOICE, Choice, DialogAnswer, DialogKind, DialogSpec, Field};
use crate::module::PageRecord;
use cv_core::engine::{Fingerprint, MoveOutcome, PageDraft, PageMove, RemoveOutcome, WriteOutcome};
use cv_core::events::pages::PagesEvent;
use cv_core::id::StableId;
use serde::{Deserialize, Serialize};

pub const NEW: &str = "pages.new";
pub const RENAME: &str = "pages.rename";
pub const REPARENT: &str = "pages.reparent";
pub const DELETE: &str = "pages.delete";

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(NEW, "Pages: New page…"),
        CommandSpec::new(RENAME, "Pages: Rename/move page…"),
        CommandSpec::new(REPARENT, "Pages: Reparent page…"),
        CommandSpec::new(DELETE, "Pages: Delete page…"),
    ]
}

pub fn run(m: &mut PagesModule, command: &str) -> Vec<Intent> {
    match command {
        NEW => vec![Intent::dialog(new_form(m, None, None))],
        RENAME => match selected(m) {
            Some((path, _, title)) => match source_base(m, &path) {
                Ok(base) => vec![Intent::dialog(rename_prompt(
                    &path, &title, None, &base, None,
                ))],
                Err(e) => m.failed("rename", &path, e),
            },
            None => vec![Intent::Status(
                "pages: select a page to rename or move".into(),
            )],
        },
        REPARENT => match selected(m) {
            Some((path, _, title)) => match source_base(m, &path) {
                Ok(base) => vec![Intent::dialog(reparent_choose(
                    m, &path, &title, &base, None,
                ))],
                Err(e) => m.failed("reparent", &path, e),
            },
            None => vec![Intent::Status("pages: select a page to reparent".into())],
        },
        DELETE => match selected(m) {
            Some((path, _, title)) => match source_base(m, &path) {
                Ok(base) => vec![Intent::dialog(delete_confirm(
                    m, &path, &title, &base, None,
                ))],
                Err(e) => m.failed("delete", &path, e),
            },
            None => vec![Intent::Status("pages: select a page to delete".into())],
        },
        _ => vec![Intent::Status(format!("unknown command {command}"))],
    }
}

pub fn answer(m: &mut PagesModule, command: &str, a: &DialogAnswer) -> Vec<Intent> {
    if a.action.is_none() {
        return Vec::new();
    }
    match command {
        NEW => create(m, a),
        RENAME => rename(m, a),
        REPARENT => reparent(m, a),
        DELETE => delete(m, a),
        _ => vec![Intent::Status(format!("unknown answer {command}"))],
    }
}

fn selected(m: &mut PagesModule) -> Option<(String, String, String)> {
    let i = m.focused()?;
    let p = &m.pages()[i];
    Some((p.path.clone(), p.id.clone(), p.title.clone()))
}

fn changed(m: &mut PagesModule) {
    m.rebuild_for_lifecycle();
}

fn retry(spec: DialogSpec, error: impl Into<String>) -> Vec<Intent> {
    vec![Intent::dialog(spec.error(error.into()))]
}

/// Dialog-carried mutation base. The fingerprint is taken when the
/// dialog opens, not when the answer returns, so a file changed while the
/// user is reading the dialog conflicts instead of being silently moved,
/// rewritten or deleted.
#[derive(Serialize, Deserialize)]
struct LifecycleContext {
    path: String,
    base: Fingerprint,
}

fn context(path: &str, base: &Fingerprint) -> String {
    serde_json::to_string(&LifecycleContext {
        path: path.into(),
        base: *base,
    })
    .expect("lifecycle dialog context serializes")
}

fn parse_context(a: &DialogAnswer) -> Option<(String, Fingerprint)> {
    let c: LifecycleContext = serde_json::from_str(a.context.as_deref()?).ok()?;
    Some((c.path, c.base))
}

fn source_base(m: &PagesModule, path: &str) -> Result<Fingerprint, String> {
    m.port.read(&m.workspace_path(path)).map(|s| s.base)
}

fn new_form(m: &PagesModule, answer: Option<&DialogAnswer>, error: Option<String>) -> DialogSpec {
    let title = answer.and_then(|a| a.text("title")).unwrap_or_default();
    let parent = answer.and_then(|a| a.text("parent")).unwrap_or(NO_PARENT);
    let path = answer.and_then(|a| a.text("path")).unwrap_or_default();
    let options = m.parent_options();
    let selected = options.iter().position(|o| o == parent).unwrap_or(0);
    let refs: Vec<&str> = options.iter().map(String::as_str).collect();
    let mut spec = DialogSpec::new(
        NEW,
        "New page",
        DialogKind::Form(vec![
            Field::text("title", "Title", title).placeholder("Getting Started"),
            Field::choice("parent", "Parent", &refs, selected),
            Field::text("path", "Path", path)
                .placeholder("guides/new-page.md")
                .hint("Relative to the content input. An existing file is never overwritten."),
        ]),
    )
    .body("Creates a Markdown source. Boris's next build judges it.")
    .cancel("Cancel")
    .action("create", "Create", ActionTone::Primary);
    if let Some(e) = error {
        spec = spec.error(e);
    }
    spec
}

fn create(m: &mut PagesModule, a: &DialogAnswer) -> Vec<Intent> {
    let title = a.text("title").unwrap_or_default().trim();
    let parent_choice = a.text("parent").unwrap_or(NO_PARENT);
    let parent = (parent_choice != NO_PARENT).then_some(parent_choice);
    let path = a.text("path").unwrap_or_default().trim();

    if let Some(p) = parent
        && !m.has_page_id(p)
    {
        return retry(
            new_form(m, Some(a), None),
            format!("parent {p} no longer exists"),
        );
    }
    if let Err(e) = edit::valid_new_page_path(path) {
        return retry(new_form(m, Some(a), None), e);
    }
    let body = match edit::new_page(title, parent) {
        Ok(b) => b,
        Err(e) => return retry(new_form(m, Some(a), None), e),
    };
    let id = edit::path_id(path);
    let draft = PageDraft {
        path: m.workspace_path(path),
        body,
        base: None,
    };
    match m.port.write(&draft) {
        Ok(WriteOutcome::Written { .. }) => {
            let mut frontmatter = vec![("title".into(), title.to_string())];
            if let Some(p) = parent {
                frontmatter.push(("parent".into(), p.to_string()));
            }
            m.pages_mut().push(PageRecord {
                path: path.into(),
                id: id.clone(),
                title: title.into(),
                parent: parent.map(str::to_string),
                frontmatter,
                body: format!("# {title}\n"),
                ..Default::default()
            });
            changed(m);
            m.outcome(PagesEvent::Created {
                page: StableId(id),
                path: path.into(),
            })
        }
        Ok(WriteOutcome::StaleBase { .. }) => retry(
            new_form(m, Some(a), None),
            format!("{path} already exists; nothing was overwritten"),
        ),
        Err(e) => m.failed("create", path, e),
    }
}

fn rename_prompt(
    path: &str,
    title: &str,
    answer: Option<&DialogAnswer>,
    base: &Fingerprint,
    error: Option<String>,
) -> DialogSpec {
    let value = answer.and_then(|a| a.text("path")).unwrap_or(path);
    let mut spec = DialogSpec::new(
        RENAME,
        "Rename or move page",
        DialogKind::Prompt(
            Field::text("path", "New path", value)
                .hint("Relative to the content input. The Boris id is pinned before the move, so links and snoozes keep their target."),
        ),
    )
    .body(format!("“{title}” is currently {path}. An existing target is never overwritten."))
    .context(context(path, base))
    .cancel("Cancel")
    .action("move", "Rename/move", ActionTone::Primary);
    if let Some(e) = error {
        spec = spec.error(e);
    }
    spec
}

fn rename(m: &mut PagesModule, a: &DialogAnswer) -> Vec<Intent> {
    let Some((old, base)) = parse_context(a) else {
        return Vec::new();
    };
    let new = a.text("path").unwrap_or_default().trim();
    let Some((id, title)) = m.page_id_title(&old) else {
        return m.failed(
            "rename",
            &old,
            "the page is no longer in the snapshot".into(),
        );
    };
    if let Err(e) = edit::valid_move_path(&old, new) {
        return retry(rename_prompt(&old, &title, Some(a), &base, None), e);
    }
    if new == old {
        return vec![Intent::Status(format!("{old} unchanged"))];
    }
    let source = match m.port.read(&m.workspace_path(&old)) {
        Ok(s) => s,
        Err(e) => return m.failed("rename", &old, e),
    };
    let body = match edit::ensure_id(&source.text, &id) {
        Ok(b) => b,
        Err(e) => return m.failed("rename", &old, e),
    };
    let mv = PageMove {
        from: m.workspace_path(&old),
        to: m.workspace_path(new),
        body,
        base,
    };
    match m.port.rename(&mv) {
        Ok(MoveOutcome::Moved { .. }) => {
            m.move_page_path(&old, new);
            changed(m);
            m.outcome(PagesEvent::Renamed {
                page: StableId(id),
                from: old,
                to: new.into(),
            })
        }
        Ok(MoveOutcome::SourceStale { .. }) => match source_base(m, &old) {
            Ok(fresh) => retry(
                rename_prompt(&old, &title, Some(a), &fresh, None),
                format!("{old} changed on disk; nothing was moved"),
            ),
            Err(e) => m.failed("rename", &old, e),
        },
        Ok(MoveOutcome::TargetExists { .. }) => match source_base(m, &old) {
            Ok(fresh) => retry(
                rename_prompt(&old, &title, Some(a), &fresh, None),
                format!("{new} already exists; nothing was overwritten"),
            ),
            Err(e) => m.failed("rename", &old, e),
        },
        Err(e) => m.failed("rename", &old, e),
    }
}

fn reparent_choose(
    m: &PagesModule,
    path: &str,
    title: &str,
    base: &Fingerprint,
    error: Option<String>,
) -> DialogSpec {
    let mut choices = vec![Choice {
        id: String::new(),
        label: "Trunk (no parent)".into(),
    }];
    choices.extend(
        m.reparent_targets(path)
            .into_iter()
            .map(|(id, label)| Choice { id, label }),
    );
    let mut spec = DialogSpec::new(REPARENT, "Reparent page", DialogKind::Choose(choices))
        .body(format!("Choose the new parent of “{title}” ({path})."))
        .context(context(path, base))
        .cancel("Cancel")
        .action("reparent", "Reparent", ActionTone::Primary);
    if let Some(e) = error {
        spec = spec.error(e);
    }
    spec
}

fn reparent(m: &mut PagesModule, a: &DialogAnswer) -> Vec<Intent> {
    let Some((path, base)) = parse_context(a) else {
        return Vec::new();
    };
    let chosen = a.text(CHOICE).unwrap_or_default();
    let parent = (!chosen.is_empty()).then_some(chosen);
    let Some((id, title, current)) = m.page_id_title_parent(&path) else {
        return m.failed(
            "reparent",
            &path,
            "the page is no longer in the snapshot".into(),
        );
    };
    if parent == current.as_deref() {
        return vec![Intent::Status(format!("{id} already has that parent"))];
    }
    if let Some(p) = parent
        && !m.reparent_targets(&path).iter().any(|(id, _)| id == p)
    {
        return retry(
            reparent_choose(m, &path, &title, &base, None),
            format!("{p} cannot parent this page"),
        );
    }
    let source = match m.port.read(&m.workspace_path(&path)) {
        Ok(s) => s,
        Err(e) => return m.failed("reparent", &path, e),
    };
    let body = match edit::set_parent(&source.text, parent) {
        Ok(b) => b,
        Err(e) => return m.failed("reparent", &path, e),
    };
    let draft = PageDraft {
        path: m.workspace_path(&path),
        body,
        base: Some(base),
    };
    match m.port.write(&draft) {
        Ok(WriteOutcome::Written { .. }) => {
            m.set_page_parent(&path, parent);
            changed(m);
            m.outcome(PagesEvent::Reparented {
                page: StableId(id),
                parent: parent.map(|p| StableId(p.into())),
            })
        }
        Ok(WriteOutcome::StaleBase { .. }) => match source_base(m, &path) {
            Ok(fresh) => retry(
                reparent_choose(m, &path, &title, &fresh, None),
                format!("{path} changed on disk; nothing was written"),
            ),
            Err(e) => m.failed("reparent", &path, e),
        },
        Err(e) => m.failed("reparent", &path, e),
    }
}

fn delete_confirm(
    m: &PagesModule,
    path: &str,
    title: &str,
    base: &Fingerprint,
    error: Option<String>,
) -> DialogSpec {
    let impact = m.delete_impact(path);
    let mut spec = DialogSpec::new(DELETE, "Delete page?", DialogKind::Confirm)
        .body(format!(
            "“{title}” ({path}) is removed from source content. Boris's next build judges the result.\n\n{impact}"
        ))
        .context(context(path, base))
        .cancel("Cancel")
        .action("delete", "Delete page", ActionTone::Danger);
    if let Some(e) = error {
        spec = spec.error(e);
    }
    spec
}

fn delete(m: &mut PagesModule, a: &DialogAnswer) -> Vec<Intent> {
    if !a.is("delete") {
        return Vec::new();
    }
    let Some((path, base)) = parse_context(a) else {
        return Vec::new();
    };
    let Some((id, title, _)) = m.page_id_title_parent(&path) else {
        return m.failed(
            "delete",
            &path,
            "the page is no longer in the snapshot".into(),
        );
    };
    match m.port.remove(&m.workspace_path(&path), base) {
        Ok(RemoveOutcome::Removed) => {
            m.remove_page(&path);
            changed(m);
            m.outcome(PagesEvent::Deleted {
                page: StableId(id),
                path,
            })
        }
        Ok(RemoveOutcome::StaleBase { .. }) => match source_base(m, &path) {
            Ok(fresh) => retry(
                delete_confirm(m, &path, &title, &fresh, None),
                format!("{path} changed on disk; nothing was deleted"),
            ),
            Err(e) => m.failed("delete", &path, e),
        },
        Err(e) => m.failed("delete", &path, e),
    }
}
