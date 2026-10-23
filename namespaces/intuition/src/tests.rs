use std::collections::BTreeMap;

use serde_json::json;

use super::*;

/// A deterministic fake host: expressions read from a table (anything else
/// fails to evaluate), and assignments are recorded.
#[derive(Default)]
struct FakeHost {
    values: BTreeMap<String, Data>,
    assigned: Vec<(String, Data)>,
    fail_assign: bool,
}

impl IntuitionHost for FakeHost {
    type Error = &'static str;

    fn read(&mut self, expression: &str) -> Result<Result<Data, ()>, &'static str> {
        if expression == "fatal" {
            return Err("fatal");
        }
        Ok(self.values.get(expression).cloned().ok_or(()))
    }

    fn assign(&mut self, location: &str, value: &Data) -> Result<Result<(), ()>, &'static str> {
        if self.fail_assign {
            return Ok(Err(()));
        }
        self.assigned.push((location.to_owned(), value.clone()));
        Ok(Ok(()))
    }
}

fn attr(local_name: &str, value: &str) -> Attribute {
    Attribute {
        namespace_uri: None,
        local_name: local_name.into(),
        value: value.into(),
    }
}

fn number(value: f64) -> Data {
    Data::Number(scxml_types::FiniteNumber::new(value).expect("a finite number"))
}

fn state(value: &Json) -> Data {
    Data::JsonObject(canonical_json(value))
}

fn execution(result: Result<Executed, Failure<&'static str>>) -> &'static str {
    match result {
        Err(Failure::Execution(detail)) => detail,
        other => panic!("expected error.execution, got {other:?}"),
    }
}

/// The tuner state `<intuition:observe>` writes for `tuner` and `confidence`.
fn observed(tuner: Data, confidence: Data) -> Json {
    let mut host = FakeHost::default();
    host.values.insert("confidence_tuner".into(), tuner);
    host.values.insert("confidence".into(), confidence);
    assert_eq!(
        execute(
            "observe",
            &[
                attr("location", "confidence_tuner"),
                attr("confidence", "confidence")
            ],
            &[],
            &mut host,
        ),
        Ok(Executed::Handled)
    );
    match host.assigned.as_slice() {
        [(location, Data::JsonObject(text))] if location == "confidence_tuner" => {
            serde_json::from_str(text).expect("canonical JSON")
        }
        other => panic!("expected one tuner state, got {other:?}"),
    }
}

/// What `<intuition:escalate>` writes for `confidence` and `tuner`.
fn escalated(confidence: Data, tuner: Data) -> bool {
    let mut host = FakeHost::default();
    host.values.insert("confidence".into(), confidence);
    host.values.insert("confidence_tuner".into(), tuner);
    assert_eq!(
        execute(
            "escalate",
            &[
                attr("location", "escalate"),
                attr("confidence", "confidence"),
                attr("tuner", "confidence_tuner"),
            ],
            &[],
            &mut host,
        ),
        Ok(Executed::Handled)
    );
    match host.assigned.as_slice() {
        [(location, Data::Bool(value))] if location == "escalate" => *value,
        other => panic!("expected one decision, got {other:?}"),
    }
}

#[test]
fn the_namespace_uri_is_the_tagless_repository_path_of_its_name() {
    assert_eq!(
        NAMESPACE_URI,
        format!("https://github.com/stateforward/floppy/namespaces/{NAMESPACE_NAME}")
    );
}

#[test]
fn the_package_binds_the_exact_uri_and_schema_and_declares_no_io() {
    let manifest = scxml_validator_core::ExtensionPackageManifest::from_json(include_bytes!(
        "../package/intuition.extension.json"
    ))
    .expect("the manifest is a valid extension package manifest");
    assert_eq!(manifest.namespace_uri, NAMESPACE_URI);
    assert_eq!(
        manifest.extension_artifact,
        "floppy_intuition.virtualized.wasm"
    );
    manifest
        .verify_schema(&scxml_validator_core::SchemaSource::new(
            "intuition.xsd",
            include_bytes!("../package/intuition.xsd").to_vec(),
        ))
        .expect("the schema digest, targetNamespace, and WIT identity verify");
}

#[test]
fn other_local_names_are_not_handled() {
    let mut host = FakeHost::default();
    assert_eq!(
        execute("tuner", &[], &[], &mut host),
        Ok(Executed::NotHandled)
    );
    assert!(host.assigned.is_empty());
}

#[test]
fn observe_sets_the_first_mean_then_moves_the_baseline_by_the_ema() {
    // The first observation sets the mean and resets the variance.
    assert_eq!(
        observed(
            state(&json!({"mean": 70, "var": 100, "n": 0})),
            number(34.0)
        ),
        json!({"mean": 34, "var": 100, "n": 1})
    );
    // Later ones move the mean and variance by the EMA (alpha 0.08).
    let next = observed(
        state(&json!({"mean": 70, "var": 100, "n": 8})),
        number(34.0),
    );
    assert!((next["mean"].as_f64().expect("a mean") - 67.12).abs() < 1e-9);
    assert!((next["var"].as_f64().expect("a variance") - 195.68).abs() < 1e-9);
    assert_eq!(next["n"], 9);
    // The variance never drops under its floor.
    assert_eq!(
        observed(
            state(&json!({"mean": 34, "var": 100, "n": 1})),
            number(34.0)
        ),
        json!({"mean": 34, "var": 100, "n": 2})
    );
}

#[test]
fn observe_clamps_the_confidence_to_the_scale() {
    assert_eq!(
        observed(
            state(&json!({"mean": 70, "var": 100, "n": 0})),
            number(250.0)
        ),
        json!({"mean": 100, "var": 100, "n": 1})
    );
    assert_eq!(
        observed(
            state(&json!({"mean": 70, "var": 100, "n": 0})),
            number(-5.0)
        ),
        json!({"mean": 0, "var": 100, "n": 1})
    );
}

#[test]
fn observe_without_a_confidence_keeps_the_state() {
    for none in [Data::Null, Data::Undefined] {
        assert_eq!(
            observed(state(&json!({"mean": 50, "var": 120, "n": 3})), none),
            json!({"mean": 50, "var": 120, "n": 3})
        );
    }
}

#[test]
fn anything_but_a_tuner_state_is_the_initial_state() {
    for tuner in [
        Data::Null,
        Data::String("tuner".into()),
        state(&json!({"mean": 1, "var": 1})),
        state(&json!({"mean": 1, "var": 1, "n": -1})),
        state(&json!({"mean": 1, "var": 1, "n": 1.5})),
        Data::JsonArray("[]".into()),
    ] {
        assert_eq!(
            observed(tuner, Data::Null),
            json!({"mean": 70, "var": 100, "n": 0})
        );
    }
}

#[test]
fn escalate_is_below_the_floor_during_the_warmup() {
    let warm = state(&json!({"mean": 90, "var": 100, "n": 7}));
    assert!(escalated(number(34.0), warm.clone()));
    assert!(!escalated(number(35.0), warm));
}

#[test]
fn escalate_follows_the_tuner_after_the_warmup() {
    // max(17.5, 90 - 10) = 80.
    let tuned = state(&json!({"mean": 90, "var": 100, "n": 20}));
    assert!(escalated(number(70.0), tuned.clone()));
    assert!(!escalated(number(80.0), tuned));
    // The soft floor: max(17.5, 20 - 10) = 17.5.
    let low = state(&json!({"mean": 20, "var": 100, "n": 20}));
    assert!(escalated(number(17.0), low.clone()));
    assert!(!escalated(number(18.0), low));
    // The deviation is never under the variance floor's.
    let tight = state(&json!({"mean": 90, "var": 1, "n": 20}));
    assert!(escalated(number(79.0), tight.clone()));
    assert!(!escalated(number(80.0), tight));
}

#[test]
fn escalate_without_a_confidence_never_escalates() {
    for none in [Data::Null, Data::Undefined] {
        assert!(!escalated(
            none,
            state(&json!({"mean": 90, "var": 100, "n": 20}))
        ));
    }
    assert!(escalated(number(10.0), Data::Null));
}

/// Checks that each case raises `error.execution` with its detail and
/// assigns nothing.
fn refused(cases: Vec<(&str, Vec<Attribute>, Vec<Content>, &str)>) {
    for (name, attributes, content, detail) in cases {
        let mut host = FakeHost::default();
        host.values.insert(
            "confidence_tuner".into(),
            state(&json!({"mean": 70, "var": 100, "n": 0})),
        );
        host.values.insert("confidence".into(), number(50.0));
        host.values.insert("text".into(), Data::String("50".into()));
        assert_eq!(
            execution(execute(name, &attributes, &content, &mut host)),
            detail
        );
        assert!(host.assigned.is_empty(), "{detail}");
    }
}

#[test]
fn malformed_observes_raise_error_execution_and_assign_nothing() {
    refused(vec![
        (
            "observe",
            vec![attr("confidence", "confidence")],
            vec![],
            "intuition:observe requires a location",
        ),
        (
            "observe",
            vec![attr("location", "confidence_tuner")],
            vec![],
            "intuition:observe requires a confidence",
        ),
        (
            "observe",
            vec![
                attr("location", "confidence_tuner"),
                attr("confidence", "confidence"),
                attr("tuner", "confidence_tuner"),
            ],
            vec![],
            "intuition:observe takes only the location and confidence attributes",
        ),
        (
            "observe",
            vec![
                attr("location", "confidence_tuner"),
                attr("location", "confidence_tuner"),
            ],
            vec![],
            "intuition:observe repeats an attribute",
        ),
        (
            "observe",
            vec![attr("location", " "), attr("confidence", "confidence")],
            vec![],
            "intuition:observe has an empty attribute",
        ),
        (
            "observe",
            vec![
                attr("location", "confidence_tuner"),
                attr("confidence", "confidence"),
            ],
            vec![Content::Text("50".into())],
            "intuition:observe holds no content",
        ),
        (
            "observe",
            vec![
                attr("location", "confidence_tuner"),
                attr("confidence", "text"),
            ],
            vec![],
            "intuition:observe confidence is not a number or null",
        ),
        (
            "observe",
            vec![
                attr("location", "confidence_tuner"),
                attr("confidence", "missing"),
            ],
            vec![],
            "intuition:observe confidence did not evaluate",
        ),
    ]);
}

#[test]
fn malformed_escalates_raise_error_execution_and_assign_nothing() {
    refused(vec![
        (
            "escalate",
            vec![
                attr("confidence", "confidence"),
                attr("tuner", "confidence_tuner"),
            ],
            vec![],
            "intuition:escalate requires a location",
        ),
        (
            "escalate",
            vec![
                attr("location", "escalate"),
                attr("tuner", "confidence_tuner"),
            ],
            vec![],
            "intuition:escalate requires a confidence",
        ),
        (
            "escalate",
            vec![
                attr("location", "escalate"),
                attr("confidence", "confidence"),
            ],
            vec![],
            "intuition:escalate requires a tuner",
        ),
        (
            "escalate",
            vec![
                attr("location", "escalate"),
                attr("confidence", "text"),
                attr("tuner", "confidence_tuner"),
            ],
            vec![],
            "intuition:escalate confidence is not a number or null",
        ),
        (
            "escalate",
            vec![
                attr("location", "escalate"),
                attr("confidence", "confidence"),
                attr("tuner", "missing"),
            ],
            vec![],
            "intuition:escalate tuner did not evaluate",
        ),
        (
            "escalate",
            vec![
                attr("location", "escalate"),
                attr("confidence", "confidence"),
                attr("tuner", "confidence_tuner"),
            ],
            vec![Content::Start, Content::End],
            "intuition:escalate holds no content",
        ),
    ]);
}

#[test]
fn a_failed_assignment_or_host_failure_is_reported() {
    let mut host = FakeHost {
        fail_assign: true,
        ..FakeHost::default()
    };
    host.values.insert("confidence".into(), number(50.0));
    host.values.insert("confidence_tuner".into(), Data::Null);
    assert_eq!(
        execution(execute(
            "observe",
            &[
                attr("location", "confidence_tuner"),
                attr("confidence", "confidence")
            ],
            &[],
            &mut host,
        )),
        "intuition:observe could not assign the tuner state"
    );
    assert_eq!(
        execution(execute(
            "escalate",
            &[
                attr("location", "escalate"),
                attr("confidence", "confidence"),
                attr("tuner", "confidence_tuner"),
            ],
            &[],
            &mut host,
        )),
        "intuition:escalate could not assign the decision"
    );
    let mut host = FakeHost::default();
    assert_eq!(
        execute(
            "escalate",
            &[
                attr("location", "escalate"),
                attr("confidence", "fatal"),
                attr("tuner", "confidence_tuner"),
            ],
            &[],
            &mut host,
        ),
        Err(Failure::Host("fatal"))
    );
}
