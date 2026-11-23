//! Smart-mailbox filter builder: cycle field / op / value over the
//! values notes actually carry, add terms, save the conjunction.

use crate::command::CommandSpec;
use cv_core::id::AttrValue;
use cv_core::store::{Op, Predicate, Term};
use cv_notes::{Research, ResearchPort};

const FIELDS: &[&str] = &[
    "triage", "tags", "capture", "format", "promoted", "notebook",
];
const OPS: &[Op] = &[Op::Eq, Op::Ne, Op::Contains];

#[derive(Debug, Default)]
pub struct Filter {
    field: usize,
    op: usize,
    value: usize,
    terms: Vec<Term>,
}

pub fn commands() -> Vec<CommandSpec> {
    [
        ("field", "Filter: next field"),
        ("op", "Filter: next operator"),
        ("value", "Filter: next value"),
        ("add", "Filter: add term"),
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

    /// Distinct values of the current field; tags split into `,tag,`.
    fn values<P: ResearchPort>(&self, s: &Research<P>) -> Vec<AttrValue> {
        let field = FIELDS[self.field];
        let mut out: Vec<AttrValue> = Vec::new();
        for n in s.notebook.notes() {
            let row = s.notebook.attr_row(n);
            let vals = match (field, row.fields.get(field)) {
                ("tags", _) => n
                    .tags
                    .iter()
                    .map(|t| AttrValue::Text(format!(",{t},")))
                    .collect(),
                (_, Some(v)) => vec![v.clone()],
                _ => vec![],
            };
            for v in vals {
                if !out.contains(&v) {
                    out.push(v);
                }
            }
        }
        out.sort_by_key(value_str);
        out
    }

    fn draft<P: ResearchPort>(&self, s: &Research<P>) -> Option<Term> {
        let vals = self.values(s);
        let value = vals.get(self.value % vals.len().max(1))?.clone();
        let op = if FIELDS[self.field] == "tags" {
            Op::Contains
        } else {
            OPS[self.op]
        };
        Some(Term {
            field: FIELDS[self.field].into(),
            op,
            value,
        })
    }

    pub fn run<P: ResearchPort>(&mut self, verb: &str, s: &mut Research<P>) -> String {
        match verb {
            "field" => {
                self.field = (self.field + 1) % FIELDS.len();
                self.value = 0;
            }
            "op" => self.op = (self.op + 1) % OPS.len(),
            "value" => self.value += 1,
            "add" => match self.draft(s) {
                Some(t) => self.terms.push(t),
                None => return format!("filter: no values for {}", FIELDS[self.field]),
            },
            "clear" => self.terms.clear(),
            "save" => {
                if self.terms.is_empty() {
                    return "filter: add a term first".into();
                }
                let name = self.label();
                return match s.save_view(&name, self.predicate()) {
                    Ok(id) => {
                        self.terms.clear();
                        format!("saved smart mailbox {} ({name})", id.0)
                    }
                    Err(e) => format!("filter: {e}"),
                };
            }
            _ => return format!("filter: unknown verb {verb}"),
        }
        let next = self
            .draft(s)
            .map(|t| term_str(&t))
            .unwrap_or_else(|| "-".into());
        format!("filter {} · next: {next}", self.label())
    }
}
