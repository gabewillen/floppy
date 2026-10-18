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

impl IdentityHost for FakeHost {
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
        "../package/identity.extension.json"
    ))
    .expect("the manifest is a valid extension package manifest");
    assert_eq!(manifest.namespace_uri, NAMESPACE_URI);
    assert_eq!(
        manifest.extension_artifact,
        "floppy_identity.virtualized.wasm"
    );
    manifest
        .verify_schema(&scxml_validator_core::SchemaSource::new(
            "identity.xsd",
            include_bytes!("../package/identity.xsd").to_vec(),
        ))
        .expect("the schema digest, targetNamespace, and WIT identity verify");
}

#[test]
fn other_local_names_are_not_handled() {
    let mut host = FakeHost::default();
    assert_eq!(
        execute("recognize", &[attr("location", "x")], &[], &mut host),
        Ok(Executed::NotHandled)
    );
    assert!(host.assigned.is_empty());
}

/// What `<identity:hears_name>` assigns for `name` and `text`.
fn recognized(name: &Data, text: &Data) -> Result<Vec<(String, Data)>, &'static str> {
    let mut host = FakeHost {
        values: BTreeMap::from([
            ("name".to_owned(), name.clone()),
            ("heard".to_owned(), text.clone()),
        ]),
        ..FakeHost::default()
    };
    match execute(
        "hears_name",
        &[
            attr("location", "addressed"),
            attr("name", "name"),
            attr("text", "heard"),
        ],
        &[],
        &mut host,
    ) {
        Ok(Executed::Handled) => Ok(host.assigned),
        Err(Failure::Execution(detail)) => Err(detail),
        other => panic!("unexpected {other:?}"),
    }
}

fn heard(name: &str, text: &str) -> bool {
    match recognized(&Data::String(name.into()), &Data::String(text.into()))
        .expect("hears_name runs")
        .as_slice()
    {
        [(location, Data::Bool(addressed))] if location == "addressed" => *addressed,
        other => panic!("unexpected assignment {other:?}"),
    }
}

#[test]
fn hears_name_hears_the_name_as_a_whole_run_of_tokens() {
    assert!(heard("Floppy", "Floppy, what's your name?"));
    assert!(heard("floppy", "hey FLOPPY!"));
    assert!(heard("Mister Floppy", "hello, mister   floppy."));
    assert!(heard("R2-D2", "r2d2 come here"));
    assert!(!heard("Al", "always"));
    assert!(!heard("Mister Floppy", "mister big floppy"));
    assert!(!heard("Floppy", "what is your name?"));
    assert!(!heard("!!", "!! anyone"));
    assert!(!heard("Floppy", ""));
}

#[test]
fn hears_name_hears_nothing_without_a_name_or_a_text() {
    for (name, text) in [
        (Data::Null, Data::String("Floppy".into())),
        (Data::String("Floppy".into()), Data::Null),
        (Data::Undefined, Data::Undefined),
    ] {
        assert_eq!(
            recognized(&name, &text),
            Ok(vec![("addressed".to_owned(), Data::Bool(false))])
        );
    }
}

#[test]
fn hears_name_refuses_a_name_or_text_that_is_not_a_string() {
    assert_eq!(
        recognized(&data(&json!(1)), &Data::String("x".into())),
        Err("identity:hears_name name is not a string or null")
    );
    assert_eq!(
        recognized(&Data::String("x".into()), &data(&json!(["x"]))),
        Err("identity:hears_name text is not a string or null")
    );
}

#[test]
fn hears_name_refuses_a_malformed_element() {
    let mut host = FakeHost::default();
    assert_eq!(
        execution(execute(
            "hears_name",
            &[attr("name", "name"), attr("text", "heard")],
            &[],
            &mut host
        )),
        "identity:hears_name requires a location"
    );
    assert_eq!(
        execution(execute(
            "hears_name",
            &[
                attr("location", "addressed"),
                attr("name", "name"),
                attr("text", "heard"),
                attr("confidence", "1"),
            ],
            &[],
            &mut host
        )),
        "identity:hears_name takes only the location, name and text attributes"
    );
    assert_eq!(
        execution(execute(
            "hears_name",
            &[
                attr("location", "addressed"),
                attr("name", "name"),
                attr("text", "heard"),
            ],
            &[Content::Start, Content::End],
            &mut host
        )),
        "identity:hears_name holds no content"
    );
    assert!(host.assigned.is_empty());
}

/// What `<identity:sorted_identities>` assigns for `ids`.
fn sorted(ids: &Data) -> Result<Json, &'static str> {
    let mut host = FakeHost {
        values: BTreeMap::from([("ids".to_owned(), ids.clone())]),
        ..FakeHost::default()
    };
    match execute(
        "sorted_identities",
        &[attr("location", "sorted"), attr("ids", "ids")],
        &[],
        &mut host,
    ) {
        Ok(Executed::Handled) => match host.assigned.as_slice() {
            [(location, Data::JsonArray(text))] if location == "sorted" => {
                Ok(serde_json::from_str(text).expect("the identities are JSON"))
            }
            other => panic!("unexpected assignment {other:?}"),
        },
        Err(Failure::Execution(detail)) => Err(detail),
        other => panic!("unexpected {other:?}"),
    }
}

/// The identities `sortedIdentities()` in `abilities/conversation/conversation.js`
/// sorted, before this element replaced it (`testdata/sorted_identities.json`,
/// written by that JavaScript), are sorted identically.
#[test]
fn sorted_identities_sorts_as_the_javascript_it_replaces() {
    let cases: Json = serde_json::from_str(include_str!("testdata/sorted_identities.json"))
        .expect("the parity cases are JSON");
    for case in cases.as_array().expect("an array of cases") {
        assert_eq!(
            sorted(&data(&case["ids"])),
            Ok(case["sorted"].clone()),
            "{case}"
        );
    }
}

#[test]
fn sorted_identities_puts_names_before_embeddings_and_keeps_repeats() {
    assert_eq!(
        sorted(&data(&json!([[2, 1], "b", [1.5], "a", "b"]))),
        Ok(json!(["a", "b", "b", [1.5], [2, 1]]))
    );
}

#[test]
fn malformed_sorted_identities_raise_error_execution_and_assign_nothing() {
    for (ids, detail) in [
        (
            data(&json!("alice")),
            "identity:sorted_identities ids is not an array of identities",
        ),
        (
            data(&json!(["alice", {"id": 1}])),
            "identity:sorted_identities ids is not an array of identities",
        ),
        (
            data(&json!([["x"]])),
            "identity:sorted_identities ids is not an array of identities",
        ),
    ] {
        assert_eq!(sorted(&ids), Err(detail));
    }
    let mut host = FakeHost::default();
    assert_eq!(
        execution(execute(
            "sorted_identities",
            &[attr("location", "sorted")],
            &[],
            &mut host
        )),
        "identity:sorted_identities requires ids"
    );
    assert_eq!(
        execution(execute(
            "sorted_identities",
            &[attr("location", "sorted"), attr("ids", "missing")],
            &[],
            &mut host
        )),
        "identity:sorted_identities ids did not evaluate"
    );
    assert!(host.assigned.is_empty());
}

#[test]
fn a_failed_assignment_or_host_failure_is_reported() {
    let mut host = FakeHost {
        values: BTreeMap::from([("ids".to_owned(), data(&json!(["a"])))]),
        fail_assign: true,
        ..FakeHost::default()
    };
    assert_eq!(
        execution(execute(
            "sorted_identities",
            &[attr("location", "sorted"), attr("ids", "ids")],
            &[],
            &mut host
        )),
        "identity:sorted_identities could not assign the identities"
    );
    assert_eq!(
        execute(
            "hears_name",
            &[
                attr("location", "addressed"),
                attr("name", "fatal"),
                attr("text", "heard"),
            ],
            &[],
            &mut FakeHost::default()
        ),
        Err(Failure::Host("fatal"))
    );
}

#[test]
fn python_float_writes_python_repr() {
    for (value, text) in [
        (1.0, "1.0"),
        (0.5, "0.5"),
        (1e-5, "1e-05"),
        (1e-4, "0.0001"),
        (1e16, "1e+16"),
        (1e15, "1000000000000000.0"),
        (-2.5, "-2.5"),
        (0.0, "0.0"),
        (12_345_678_901_234_567_890.5, "1.2345678901234567e+19"),
    ] {
        assert_eq!(python_float(value), text);
    }
}
