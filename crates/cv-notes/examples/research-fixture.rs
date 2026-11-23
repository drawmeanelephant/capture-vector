//! Emits the promotion corpora + Oliver inputs for
//! `scripts/capture-research-fixtures.sh`. Usage: `research-fixture <out>`.

#[path = "../tests/support/fixture_notes.rs"]
mod fixture_notes;

use fixture_notes::*;
use std::path::Path;

fn put(out: &Path, rel: &str, body: &str) {
    let p = out.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .expect("usage: research-fixture <out>");
    let out = Path::new(&out);
    let note = field_note();
    for (corpus, spec) in [
        ("promoted", promoted_spec(&note)),
        ("reparented", reparented_spec(&note)),
        ("orphan", orphan_spec(&note)),
    ] {
        let draft = cv_notes::promote_with(&note, &spec).unwrap();
        put(out, &format!("{corpus}/{}", draft.path), &draft.body);
    }
    put(out, "oliver/markdown.in", MD_BODY);
    put(out, "oliver/textile.in", TEXTILE_BODY);
    put(out, "oliver/cooklang.in", COOK_BODY);
}
