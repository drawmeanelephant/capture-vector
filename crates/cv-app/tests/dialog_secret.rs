//! A typed secret reaches the secret port and nowhere else: not the
//! element tree or layout (`--tree`), not the shell's debug output, not the
//! structured log. One test per binary: it points `CV_LOG_FILE` at a
//! temp file before the logger reads the environment.

use cv_app::dialogs::{SECRET_SAMPLE, secret};
use cv_app::{init, logging};
use cv_model::dialog::{SecretPort, SecretValue};
use cv_widgets::harness::Harness;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Default)]
struct Keychain(Rc<RefCell<Vec<(String, String)>>>);

impl SecretPort for Keychain {
    fn store(&mut self, name: &str, value: SecretValue) -> Result<(), String> {
        self.0
            .borrow_mut()
            .push((name.to_string(), value.expose().to_string()));
        Ok(())
    }
}

#[test]
fn a_typed_secret_reaches_only_the_secret_port() {
    let log = tempfile::NamedTempFile::new().unwrap();
    // SAFETY: the only test in this binary, before any thread reads the env.
    unsafe {
        std::env::set_var("CV_LOG", "trace");
        std::env::set_var("CV_LOG_FILE", log.path());
    }
    logging::init_from_env();

    let stored = Rc::new(RefCell::new(Vec::new()));
    let mut wb = init().with_secrets(Box::new(Keychain(stored.clone())));
    secret(&mut wb);
    let mut h = Harness::new(wb);
    let leaks = |what: &str, s: String| {
        assert!(!s.contains(SECRET_SAMPLE), "the secret leaked into {what}");
        s
    };
    let tree = leaks("the tree", h.tree().to_string());
    assert!(tree.contains("App password"), "the tree shows the field");
    leaks("the layout", h.layout().to_string());
    leaks("the texts", h.texts().join("\n"));
    let debug = leaks("the debug output", format!("{:?}", h.state().dialogs));
    assert!(debug.contains("publish.credential"));

    assert!(h.click("dialog-store"));
    assert!(!h.state().dialogs.is_open());
    assert_eq!(
        *stored.borrow(),
        vec![(
            "capture-vector/bsky-app-password".to_string(),
            SECRET_SAMPLE.to_string()
        )]
    );
    leaks("the tree after storing", h.tree().to_string());
    leaks("the status", h.state().status.clone());
    leaks(
        "the debug output after storing",
        format!("{:?}", h.state().dialogs),
    );

    logging::info(
        "dialog_secret_probe",
        &[("status", h.state().status.clone().into())],
    );
    let written = std::fs::read_to_string(log.path()).unwrap();
    assert!(written.contains("dialog_secret_probe"), "the log is live");
    leaks("the log", written);
}
