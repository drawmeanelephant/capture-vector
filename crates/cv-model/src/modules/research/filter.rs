//! Smart-mailbox filter builder: a form picks a field, an operator and a
//! value; terms add up to a conjunction that can be saved as a mailbox.

use crate::command::CommandSpec;
use crate::dialog::{ActionTone, DialogAnswer, DialogKind, DialogSpec, Field, Value};
use cv_core::id::AttrValue;
use cv_core::store::{Op, Predicate, Term};
use cv_notes::{Research, ResearchPort};

const FIELDS: &[&str] = &[
    "triage", "tags", "capture", "format", "promoted", "notebook",
];
const OPS: &[Op] = &[Op::Eq, Op::Ne, Op::Contains];

/// The form's answer command.
pub const BUILD: &str = "research.filter.build";

#[derive(Debug, Default)]
pub struct Filter {
    terms: Vec<Term>,
}

pub fn commands() -> Vec<CommandSpec> {
    [
        ("build", "Filter: add term…"),
        ("clear", "Filter: clear"),
        ("save", "Filter: save as smart mailbox"),
    ]
    .into_iter()
    .map(|(v, t)| CommandSpec::new(format!("research.filter.{v}"), format!("Research {t}")))
    .collect()
}

fn op_str(op: &Op) -> &'static str {
    match op {
        Op::Eq => "=",
        Op::Ne => "≠",
        Op::Lt => "<",
        Op::Le => "≤",
        Op::Gt => ">",
        Op::Ge => "≥",
        Op::Contains => "contains",
    }
}

fn value_str(v: &AttrValue) -> String {
    match v {
        AttrValue::Bool(b) => b.to_string(),
        AttrValue::Int(i) => i.to_string(),
        AttrValue::Float(f) => f.to_string(),
        AttrValue::Text(t) => t.clone(),
    }
}

fn term_str(t: &Term) -> String {
    format!("{} {} {}", t.field, op_str(&t.op), value_str(&t.value))
}

/// The value a typed word stands for in `field`: tags match as
/// `,tag,`, and a field notes carry as a boolean parses as one.
fn parse_value<P: ResearchPort>(s: &Research<P>, field: &str, typed: &str) -> Option<AttrValue> {
    let typed = typed.trim();
    if typed.is_empty() {
        return None;
    }
    if field == "tags" {
        return Some(AttrValue::Text(format!(",{typed},")));
    }
    let boolean = s.notebook.notes().into_iter().any(|n| {
        matches!(
            s.notebook.attr_row(n).fields.get(field),
            Some(AttrValue::Bool(_))
        )
    });
    if boolean {
        return typed.parse().ok().map(AttrValue::Bool);
    }
    Some(AttrValue::Text(typed.to_string()))
}

impl Filter {
    pub fn predicate(&self) -> Predicate {
        Predicate {
            conj: self.terms.clone(),
        }
    }

    pub fn label(&self) -> String {
        if self.terms.is_empty() {
            "any".into()
        } else {
            self.terms
                .iter()
                .map(term_str)
                .collect::<Vec<_>>()
                .join(" ∧ ")
        }
    }

    /// The add-term form, prefilled from `answer` when asking again.
    pub fn form(&self, answer: Option<&DialogAnswer>, error: Option<String>) -> DialogSpec {
        let ops: Vec<&str> = OPS.iter().map(op_str).collect();
        let pick = |id: &str, all: &[&str]| {
            answer
                .and_then(|a| a.text(id))
                .and_then(|t| all.iter().position(|x| *x == t))
                .unwrap_or(0)
        };
        let typed = answer.and_then(|a| a.text("value")).unwrap_or_default();
        let save = answer.is_some_and(|a| a.values.get("save") == Some(&Value::Toggle(true)));
        let mut spec = DialogSpec::new(
            BUILD,
            "Add filter term",
            DialogKind::Form(vec![
                Field::choice("field", "Field", FIELDS, pick("field", FIELDS)),
                Field::choice("op", "Operator", &ops, pick("op", &ops)),
                Field::text("value", "Value", typed)
                    .placeholder("inbox, archived, a tag, true…")
                    .hint("Tags match one tag; promoted takes true or false."),
                Field::toggle("save", "Save as a smart mailbox", save),
            ]),
        )
        .body(format!("Current filter: {}", self.label()))
        .cancel("Cancel")
        .action("add", "Add term", ActionTone::Primary);
        if let Some(e) = error {
            spec = spec.error(e);
        }
        spec
    }

    /// Apply the form's answer: `Ok` is the status line, `Err` the
    /// validation text to ask again with.
    pub fn answer<P: ResearchPort>(
        &mut self,
        a: &DialogAnswer,
        s: &mut Research<P>,
    ) -> Result<String, String> {
        let field = a.text("field").unwrap_or(FIELDS[0]);
        let typed = a.text("value").unwrap_or_default();
        let value = parse_value(s, field, typed).ok_or_else(|| match typed.trim() {
            "" => "Type a value to match.".to_string(),
            t => format!("{field} takes true or false, not “{t}”."),
        })?;
        let op = OPS
            .iter()
            .find(|o| Some(op_str(o)) == a.text("op"))
            .copied()
            .unwrap_or(Op::Eq);
        let op = match (field, &value, op) {
            ("tags", _, _) => Op::Contains,
            (_, AttrValue::Bool(_), Op::Contains) => Op::Eq,
            (_, _, op) => op,
        };
        self.terms.push(Term {
            field: field.into(),
            op,
            value,
        });
        if a.values.get("save") == Some(&Value::Toggle(true)) {
            return Ok(self.run("save", s));
        }
        Ok(format!("filter {}", self.label()))
    }

    pub fn run<P: ResearchPort>(&mut self, verb: &str, s: &mut Research<P>) -> String {
        match verb {
            "clear" => {
                self.terms.clear();
                "filter cleared".into()
            }
            "save" => {
                if self.terms.is_empty() {
                    return "filter: add a term first".into();
                }
                let name = self.label();
                match s.save_view(&name, self.predicate()) {
                    Ok(id) => {
                        self.terms.clear();
                        format!("saved smart mailbox {} ({name})", id.0)
                    }
                    Err(e) => format!("filter: {e}"),
                }
            }
            _ => format!("filter: unknown verb {verb}"),
        }
    }
}
