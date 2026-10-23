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

impl AutonomyHost for FakeHost {
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

fn execution(result: Result<Executed, Failure<&'static str>>) -> &'static str {
    match result {
        Err(Failure::Execution(detail)) => detail,
        other => panic!("expected error.execution, got {other:?}"),
    }
}

/// The data value of a JSON value (JSON text for arrays and objects).
fn data(value: &Json) -> Data {
    match value {
        Json::Null => Data::Null,
        Json::Bool(value) => Data::Bool(*value),
        Json::Number(value) => Data::Number(
            scxml_types::FiniteNumber::new(value.as_f64().expect("a number")).expect("finite"),
        ),
        Json::String(text) => Data::String(text.clone()),
        Json::Array(_) => Data::JsonArray(value.to_string()),
        Json::Object(_) => Data::JsonObject(value.to_string()),
    }
}

/// The JSON of an assigned value.
fn json(value: &Data) -> Json {
    json_of(value).expect("the assigned value is JSON")
}

/// Runs one element over `values` and returns what it assigned, by location.
fn run(
    local_name: &str,
    attributes: &[Attribute],
    values: &[(&str, Json)],
) -> Result<BTreeMap<String, Json>, &'static str> {
    let mut host = FakeHost {
        values: values
            .iter()
            .map(|(name, value)| ((*name).to_owned(), data(value)))
            .collect(),
        ..FakeHost::default()
    };
    match execute(local_name, attributes, &[], &mut host) {
        Ok(Executed::Handled) => Ok(host
            .assigned
            .iter()
            .map(|(location, value)| (location.clone(), json(value)))
            .collect()),
        Err(Failure::Execution(detail)) => {
            assert!(host.assigned.is_empty(), "a failure assigns nothing");
            Err(detail)
        }
        other => panic!("unexpected {other:?}"),
    }
}

/// Every `testdata/<name>.json` case.
fn cases(text: &str) -> Vec<Json> {
    match serde_json::from_str(text).expect("the cases are JSON") {
        Json::Array(cases) => cases,
        other => panic!("not an array of cases: {other}"),
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
        "../package/autonomy.extension.json"
    ))
    .expect("the manifest is a valid extension package manifest");
    assert_eq!(manifest.namespace_uri, NAMESPACE_URI);
    assert_eq!(
        manifest.extension_artifact,
        "floppy_autonomy.virtualized.wasm"
    );
    manifest
        .verify_schema(&scxml_validator_core::SchemaSource::new(
            "autonomy.xsd",
            include_bytes!("../package/autonomy.xsd").to_vec(),
        ))
        .expect("the schema digest, targetNamespace, and WIT identity verify");
}

#[test]
fn other_local_names_are_not_handled() {
    let mut host = FakeHost::default();
    assert_eq!(
        execute("input", &[attr("location", "x")], &[], &mut host),
        Ok(Executed::NotHandled)
    );
    assert!(host.assigned.is_empty());
}

fn payload_attributes() -> [Attribute; 4] {
    [
        attr("location", "input"),
        attr("stimulus", "s"),
        attr("focus", "f"),
        attr("focus_candidates", "c"),
    ]
}

/// The input `behaviorInputPayload()` (with its focus assigns) in
/// `abilities/autonomy/autonomy.js` and `turnBehaviorInput()` in
/// `abilities/reflection/reflection.js` built, before this element replaced
/// both (`testdata/behavior_input_payload.json`, written by that
/// JavaScript), is built member for member.
#[test]
fn the_payload_is_what_the_javascript_it_replaces_built() {
    for case in cases(include_str!("testdata/behavior_input_payload.json")) {
        assert_eq!(
            case["autonomy"], case["reflection"],
            "the two copies agreed"
        );
        let assigned = run(
            "behavior_input_payload",
            &payload_attributes(),
            &[
                ("s", case["stimulus"].clone()),
                ("f", case["focus"].clone()),
                ("c", case["focus_candidates"].clone()),
            ],
        )
        .expect("the case runs");
        assert_eq!(assigned["input"], case["autonomy"], "{case}");
    }
}

/// The behavior input wraps any data that is not an
/// object, an array included (ECMAScript's `Object.assign` spread an
/// array's indices), and a focus that evaluates to `undefined` is left out.
#[test]
fn array_data_is_wrapped_and_an_undefined_focus_is_left_out() {
    let mut host = FakeHost {
        values: BTreeMap::from([
            ("s".to_owned(), data(&json!({"event": "x", "data": [1, 2]}))),
            ("f".to_owned(), Data::Undefined),
            ("c".to_owned(), data(&json!(["lamp"]))),
        ]),
        ..FakeHost::default()
    };
    assert_eq!(
        execute(
            "behavior_input_payload",
            &payload_attributes(),
            &[],
            &mut host
        ),
        Ok(Executed::Handled)
    );
    assert_eq!(
        host.assigned
            .iter()
            .map(|(location, value)| (location.as_str(), json(value)))
            .collect::<Vec<_>>(),
        [(
            "input",
            json!({"event": [1, 2], "focus_candidates": ["lamp"]})
        )]
    );
}

#[test]
fn malformed_payloads_raise_error_execution_and_assign_nothing() {
    assert_eq!(
        run(
            "behavior_input_payload",
            &payload_attributes(),
            &[
                ("s", json!("bot.input")),
                ("f", json!(null)),
                ("c", json!([]))
            ],
        ),
        Err("autonomy:behavior_input_payload stimulus is not an object")
    );
    assert_eq!(
        run(
            "behavior_input_payload",
            &payload_attributes()[..3],
            &[("s", json!({})), ("f", json!(null))],
        ),
        Err("autonomy:behavior_input_payload requires focus_candidates")
    );
    assert_eq!(
        run(
            "behavior_input_payload",
            &[attr("location", "input"), attr("stimulus", " ")],
            &[],
        ),
        Err("autonomy:behavior_input_payload has an empty attribute")
    );
    let mut host = FakeHost::default();
    assert_eq!(
        execution(execute(
            "behavior_input_payload",
            &payload_attributes(),
            &[Content::Start, Content::End],
            &mut host
        )),
        "autonomy:behavior_input_payload holds no content"
    );
}

#[test]
fn a_failed_assignment_or_host_failure_is_reported() {
    let mut host = FakeHost {
        values: BTreeMap::from([
            ("s".to_owned(), data(&json!({"data": {}}))),
            ("f".to_owned(), Data::Null),
            ("c".to_owned(), data(&json!([]))),
        ]),
        fail_assign: true,
        ..FakeHost::default()
    };
    assert_eq!(
        execution(execute(
            "behavior_input_payload",
            &payload_attributes(),
            &[],
            &mut host
        )),
        "autonomy:behavior_input_payload could not assign the payload"
    );
    assert_eq!(
        execute(
            "behavior_input_payload",
            &[
                attr("location", "input"),
                attr("stimulus", "fatal"),
                attr("focus", "f"),
                attr("focus_candidates", "c")
            ],
            &[],
            &mut FakeHost::default()
        ),
        Err(Failure::Host("fatal"))
    );
}
