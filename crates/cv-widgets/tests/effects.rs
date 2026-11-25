//! The `effects` compositor under real GPU frames (`--features effects`):
//! headless rune runs through `fx::gfx::Gfx`, as the shipped app draws.

#![cfg(feature = "effects")]

use cv_model::module::Registry;
use cv_model::workbench::Workbench;
use cv_widgets::fx::gfx::Gfx;
use cv_widgets::{app, shell};
use rune::compose::{Compositor, Frame};
use rune::{App, AppSpec};
use std::sync::atomic::{AtomicBool, Ordering};

fn effects_on() -> Workbench {
    let mut wb = Workbench::new(Registry::with_all());
    wb.set_effects(true);
    wb
}

static REDUCED: AtomicBool = AtomicBool::new(false);

/// `Gfx`, noting whether it drew a frame with motion reduced.
#[derive(Default)]
struct Watched(Gfx);

impl Compositor<Workbench> for Watched {
    fn animating(&self) -> bool {
        self.0.animating()
    }

    fn render(&mut self, f: &mut Frame<'_, Workbench>) {
        if f.rt.state.theme.motion.reduced {
            REDUCED.store(true, Ordering::Relaxed);
        }
        self.0.render(f);
    }
}

/// Reducing motion removes the effect slots; one on its way out is drawn
/// inside an isolated layer, whose pass the compositor must also draw into.
#[test]
fn toggling_reduced_motion_with_effects_on_does_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("motion.json");
    let snap = dir.path().join("after.png");
    std::fs::write(
        &script,
        r#"{"events":[
            {"t":1.0,"type":"tap","key":"status-4"},
            {"t":3.0,"type":"mark","name":"after"}
        ]}"#,
    )
    .unwrap();
    App::new(AppSpec::new("cv", app::SIZE, effects_on, shell::view))
        .compositor(Watched::default())
        .args([
            "--script".to_string(),
            script.to_str().unwrap().to_string(),
            "--snap".to_string(),
            snap.to_str().unwrap().to_string(),
            "--at".to_string(),
            "mark:after".to_string(),
        ])
        .run();
    assert!(REDUCED.load(Ordering::Relaxed), "the tap reduced motion");
    assert!(snap.exists());
}
