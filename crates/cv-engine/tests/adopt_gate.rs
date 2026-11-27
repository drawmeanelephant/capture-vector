use cv_core::engine::{EventSink, ExitClass, Verb};
use cv_engine::adopt;
use std::sync::{Arc, Mutex};

#[test]
fn input_aware_gate_validates_and_builds_the_same_tree_adoption_wrote() {
    let verbs = Arc::new(Mutex::new(Vec::new()));
    let seen = verbs.clone();
    let run = move |verb: Verb, _sink: EventSink| {
        seen.lock().unwrap().push(verb);
        Ok(ExitClass::Ok)
    };
    let sink = || Box::new(|_: cv_core::events::Envelope| {}) as EventSink;
    let (validate, build) =
        adopt::validate_then_build_with_input(&run, &sink, Some("src")).unwrap();
    assert_eq!(validate, ExitClass::Ok);
    assert_eq!(build, Some(ExitClass::Ok));
    assert_eq!(
        *verbs.lock().unwrap(),
        vec![
            Verb::Custom {
                name: "validate".into(),
                args: vec!["--input".into(), "src".into()],
            },
            Verb::Custom {
                name: "build".into(),
                args: vec!["--input".into(), "src".into()],
            },
        ],
        "the gate carries the adoption write authority rather than defaulting to content/"
    );
}
