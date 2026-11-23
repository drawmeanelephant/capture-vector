//! Rendering goes through Oliver only; here Oliver is replayed from
//! captured `oliver render` output.

#[path = "support/fixture_notes.rs"]
mod fixture_notes;
#[path = "support/research.rs"]
mod research;

use cv_notes::{Format, OliverRender, RenderCache};
use std::cell::Cell;

struct Replay {
    calls: Cell<usize>,
}

impl OliverRender for Replay {
    fn render(&self, format: Format, source: &str) -> Result<String, String> {
        self.calls.set(self.calls.get() + 1);
        let f = format.oliver_from();
        assert_eq!(source, research::file_str(&format!("oliver/{f}.in")));
        match research::file_str(&format!("oliver/{f}.exit-code.txt")).trim() {
            "0" => Ok(research::file_str(&format!("oliver/{f}.html"))),
            _ => Err(research::file_str(&format!("oliver/{f}.stderr.txt"))),
        }
    }
}

#[test]
fn textile_and_cooklang_render_via_oliver_and_cache() {
    let (mut r, _) = fixture_notes::session();
    let mut ids = Vec::new();
    for (f, body) in [
        (Format::Textile, fixture_notes::TEXTILE_BODY),
        (Format::Cooklang, fixture_notes::COOK_BODY),
        (Format::Markdown, fixture_notes::MD_BODY),
    ] {
        ids.push(
            r.capture_note(format!("{f:?}"), body.into(), f, vec![])
                .unwrap(),
        );
    }
    let oliver = Replay {
        calls: Cell::new(0),
    };
    let mut cache = RenderCache::default();
    for id in &ids {
        let n = r.notebook.note(id).unwrap().clone();
        let html = cache.render(&n, &oliver).clone().unwrap();
        assert!(html.contains('<'), "{html}");
        cache.render(&n, &oliver);
    }
    assert_eq!(oliver.calls.get(), 3, "cached per (note, modified)");
    let cook = r.notebook.note(&ids[1]).unwrap().clone();
    assert!(
        cache
            .get(&cook)
            .unwrap()
            .as_ref()
            .unwrap()
            .contains("ingredient")
    );
    r.edit(&ids[1], Some("Soup"), None).unwrap();
    let cook = r.notebook.note(&ids[1]).unwrap().clone();
    assert!(cache.get(&cook).is_none(), "edit invalidates");
}
