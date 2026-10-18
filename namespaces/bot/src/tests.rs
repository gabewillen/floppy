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

impl BotHost for FakeHost {
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

/// The data value of a JSON value (canonical JSON text for arrays and
/// objects: `serde_json` sorts object members).
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
        "../package/bot.extension.json"
    ))
    .expect("the manifest is a valid extension package manifest");
    assert_eq!(manifest.namespace_uri, NAMESPACE_URI);
    assert_eq!(manifest.extension_artifact, "floppy_bot.virtualized.wasm");
    // Executable content is synchronous and does no I/O: the manifest
    // declares no origin and no slot.
    manifest
        .verify_schema(&scxml_validator_core::SchemaSource::new(
            "bot.xsd",
            include_bytes!("../package/bot.xsd").to_vec(),
        ))
        .expect("the schema digest, targetNamespace, and WIT identity verify");
}

#[test]
fn other_local_names_are_not_handled() {
    let mut host = FakeHost::default();
    assert_eq!(
        execute("choice", &[attr("location", "x")], &[], &mut host),
        Ok(Executed::NotHandled)
    );
    assert!(host.assigned.is_empty());
}

// <bot:stimulus>

fn stimulus_host(event: Data, data_value: Data, source: Data) -> FakeHost {
    FakeHost {
        values: BTreeMap::from([
            ("_event.name".to_owned(), event),
            ("_event.data".to_owned(), data_value),
            ("_event.invokeid".to_owned(), source),
            ("the_id".to_owned(), Data::String("s-1".into())),
            ("nothing".to_owned(), Data::Null),
            ("undef".to_owned(), Data::Undefined),
            ("number".to_owned(), data(&json!(3))),
        ]),
        ..FakeHost::default()
    }
}

fn stimulus_attributes() -> Vec<Attribute> {
    vec![
        attr("location", "pending"),
        attr("event", "_event.name"),
        attr("data", "_event.data"),
        attr("source", "_event.invokeid"),
    ]
}

fn envelope(host: &mut FakeHost, attributes: &[Attribute]) -> String {
    assert_eq!(
        execute("stimulus", attributes, &[], host),
        Ok(Executed::Handled)
    );
    match host.assigned.as_slice() {
        [(location, Data::JsonObject(text))] if location == "pending" => text.clone(),
        other => panic!("one stimulus assignment, got {other:?}"),
    }
}

#[test]
fn stimulus_writes_the_canonical_envelope_of_the_event_in_hand() {
    // From outside the session: no invoke id, so the source is null.
    for source in [Data::Undefined, Data::Null, Data::String(String::new())] {
        let mut host = stimulus_host(
            Data::String("bot.input".into()),
            data(&json!({"observation": {"event": "text", "data": "hi"}})),
            source,
        );
        let text = envelope(&mut host, &stimulus_attributes());
        assert_eq!(
            text,
            r#"{"data":{"observation":{"data":"hi","event":"text"}},"event":"bot.input","source":null}"#
        );
        let parsed: Json = serde_json::from_str(&text).expect("JSON");
        assert_eq!(parsed.to_string(), text, "canonical");
    }
    // From an invoked child: its invoke id.
    let mut host = stimulus_host(
        Data::String("bot.ability.conversation.output".into()),
        data(&json!({"messages": []})),
        Data::String("communication".into()),
    );
    assert_eq!(
        envelope(&mut host, &stimulus_attributes()),
        r#"{"data":{"messages":[]},"event":"bot.ability.conversation.output","source":"communication"}"#
    );
}

#[test]
fn stimulus_data_of_any_json_kind_is_kept_and_canonical_text_byte_for_byte() {
    for (value, expected) in [
        (Data::String("x".into()), r#""x""#),
        (data(&json!(1)), "1"),
        (data(&json!(1.5)), "1.5"),
        (Data::Bool(false), "false"),
        (Data::Null, "null"),
        (Data::JsonArray("[1,2]".into()), "[1,2]"),
        (Data::JsonObject(r#"{"a":1e+21}"#.into()), r#"{"a":1e+21}"#),
    ] {
        let mut host = stimulus_host(Data::String("e".into()), value, Data::Null);
        assert_eq!(
            envelope(&mut host, &stimulus_attributes()),
            format!(r#"{{"data":{expected},"event":"e","source":null}}"#)
        );
    }
}

#[test]
fn stimulus_includes_target_and_id_when_given() {
    let mut attributes = stimulus_attributes();
    attributes.push(attr("id", "the_id"));
    attributes.push(attr("target", "nothing"));
    let mut host = stimulus_host(
        Data::String("e".into()),
        data(&json!({})),
        Data::String("c".into()),
    );
    assert_eq!(
        envelope(&mut host, &attributes),
        r#"{"data":{},"event":"e","id":"s-1","source":"c","target":null}"#
    );
    // An undefined target or id is left out.
    let mut attributes = stimulus_attributes();
    attributes.push(attr("id", "undef"));
    attributes.push(attr("target", "undef"));
    let mut host = stimulus_host(Data::String("e".into()), data(&json!({})), Data::Null);
    assert_eq!(
        envelope(&mut host, &attributes),
        r#"{"data":{},"event":"e","source":null}"#
    );
}

#[test]
fn malformed_stimulus_elements_or_values_raise_error_execution() {
    let host = || stimulus_host(Data::String("e".into()), data(&json!({})), Data::Null);
    let without = |name: &str| -> Vec<Attribute> {
        stimulus_attributes()
            .into_iter()
            .filter(|attribute| attribute.local_name != name)
            .collect()
    };
    let with = |extra: Attribute| -> Vec<Attribute> {
        let mut attributes = stimulus_attributes();
        attributes.push(extra);
        attributes
    };
    for attributes in [
        without("location"),
        without("event"),
        without("data"),
        without("source"),
        with(attr("event", "_event.name")),
        with(attr("delay", "1s")),
        with(attr("id", "")),
        with(attr("id", "number")),
        with(attr("target", "number")),
        with(attr("target", "missing")),
    ] {
        let mut host = host();
        execution(execute("stimulus", &attributes, &[], &mut host));
        assert!(host.assigned.is_empty());
    }
    execution(execute(
        "stimulus",
        &stimulus_attributes(),
        &[Content::Text("x".into())],
        &mut host(),
    ));
    for (name, value) in [
        ("_event.name", Data::String(String::new())),
        ("_event.name", Data::Null),
        ("_event.data", Data::Undefined),
        ("_event.data", Data::Xml("<a/>".into())),
        ("_event.invokeid", Data::Bool(true)),
    ] {
        let mut host = host();
        host.values.insert(name.to_owned(), value);
        execution(execute("stimulus", &stimulus_attributes(), &[], &mut host));
        assert!(host.assigned.is_empty());
    }
    let mut failing = host();
    failing.fail_assign = true;
    execution(execute(
        "stimulus",
        &stimulus_attributes(),
        &[],
        &mut failing,
    ));
}
