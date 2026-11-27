//! Headless harness: drive the shell with scripted input in-process,
//! dump element-tree / layout JSON, compare against goldens.
//! Bless: `CV_BLESS=1 cargo test`. Mismatches write `<name>.actual.json`.

use crate::app::{input_on_key, input_poll};
use crate::{keys, shell, tree};
use cv_model::theme::Mode;
use cv_model::workbench::Workbench;
use rune::{Env, Runtime};
use serde_json::Value;
use std::path::Path;

/// The frame size harnesses boot at — mirrors `app::SIZE`.
pub use crate::app::SIZE;

pub struct Harness {
    pub rt: Runtime<Workbench>,
}

impl Harness {
    pub fn new(wb: Workbench) -> Self {
        Self::with_size(wb, SIZE)
    }

    pub fn with_size(wb: Workbench, size: (f32, f32)) -> Self {
        let env = Env {
            system_dark: wb.theme.mode == Mode::Dark,
            scale: 1.0,
        };
        let reduced = wb.theme.motion.reduced;
        let mut rt = Runtime::with_text(wb, shell::view, size, env, crate::fonts::text_system());
        rt.poll = Some(input_poll);
        rt.on_key = Some(input_on_key);
        rt.set_system_reduced_motion(reduced);
        // What the CLI's headless runs (and so the PNG gate) report, so trees
        // and renders lay out the same macOS titlebar.
        rt.set_titlebar_inset(rune::Chrome::DEFAULT.titlebar_inset());
        keys::pin_keycaps(rune::KeyPlatform::Mac);
        let mut h = Self { rt };
        h.settle();
        h
    }

    pub fn state(&self) -> &Workbench {
        &self.rt.state
    }

    pub fn state_mut(&mut self) -> &mut Workbench {
        &mut self.rt.state
    }

    /// Run frames until springs land — goldens never capture mid-motion.
    pub fn settle(&mut self) {
        self.rt.frame(0.0);
        self.rt.settle_all();
        self.rt.frame(1.0 / 60.0);
        self.rt.settle_all();
        self.rt.frame(1.0 / 60.0);
    }

    pub fn key(&mut self, token: &str) {
        for ev in keys::parse(token) {
            self.rt.key(ev);
            self.settle();
        }
    }

    pub fn click(&mut self, key: &str) -> bool {
        let Some((x, y)) = self.rt.find(key) else {
            return false;
        };
        self.rt.pointer_down(x, y);
        self.rt.pointer_up(x, y);
        self.settle();
        true
    }

    /// Rest the pointer on `key` and settle, so hover styling is captured.
    pub fn hover(&mut self, key: &str) -> bool {
        let Some((x, y)) = self.rt.find(key) else {
            return false;
        };
        self.rt.pointer_move(x, y);
        self.settle();
        true
    }

    /// Press `key` and keep holding it (no release), then settle.
    pub fn hold(&mut self, key: &str) -> bool {
        let Some((x, y)) = self.rt.find(key) else {
            return false;
        };
        self.rt.pointer_move(x, y);
        self.rt.pointer_down(x, y);
        self.settle();
        true
    }

    /// Script steps: `key:<token>` (see `keys::parse`), `click:<element key>`,
    /// `cmd:<command id>`, `frame`. Unknown steps panic — scripts are tests.
    pub fn script(&mut self, steps: &[&str]) {
        for step in steps {
            match step.split_once(':') {
                Some(("key", t)) => self.key(t),
                Some(("click", k)) => assert!(self.click(k), "no element keyed {k}"),
                Some(("cmd", c)) => {
                    self.rt.state.run_command(c);
                    self.settle();
                }
                None if *step == "frame" => self.settle(),
                _ => panic!("bad script step {step}"),
            }
        }
    }

    /// Where macOS draws the traffic lights over a headless run's content.
    pub fn traffic_lights() -> Option<[[f32; 4]; 3]> {
        rune::Chrome::DEFAULT.traffic_light_rects()
    }

    pub fn has(&self, key: &str) -> bool {
        self.rt.find(key).is_some()
    }

    pub fn tree(&self) -> Value {
        tree::element_tree(&self.rt)
    }

    pub fn layout(&self) -> Value {
        tree::layout(&self.rt.elements())
    }

    pub fn texts(&self) -> Vec<String> {
        self.rt
            .elements()
            .into_iter()
            .filter_map(|e| e.text)
            .collect()
    }
}

/// Compare `value` with `<dir>/<name>.json`; bless when `CV_BLESS=1`.
pub fn check_golden(dir: &Path, name: &str, value: &Value) -> Result<(), String> {
    let path = dir.join(format!("{name}.json"));
    let actual = serde_json::to_string_pretty(value).map_err(|e| e.to_string())? + "\n";
    if std::env::var_os("CV_BLESS").is_some_and(|v| v == "1") {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        std::fs::write(&path, &actual).map_err(|e| e.to_string())?;
        return Ok(());
    }
    let want = std::fs::read_to_string(&path)
        .map_err(|e| format!("{}: {e} (bless with CV_BLESS=1)", path.display()))?;
    if want.replace("\r\n", "\n") == actual {
        return Ok(());
    }
    let out = dir.join(format!("{name}.actual.json"));
    let _ = std::fs::write(&out, &actual);
    let first = want
        .lines()
        .zip(actual.lines())
        .position(|(a, b)| a != b)
        .unwrap_or(want.lines().count().min(actual.lines().count()));
    Err(format!(
        "golden {} differs at line {}; actual written to {}. Review the diff, then bless with CV_BLESS=1.",
        path.display(),
        first + 1,
        out.display()
    ))
}
