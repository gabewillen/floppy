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

impl ConversationHost for FakeHost {
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

/// A data value of a JSON value (canonical text for arrays and objects).
fn data(value: &Json) -> Data {
    match value {
        Json::Null => Data::Null,
        Json::Bool(value) => Data::Bool(*value),
        Json::Number(value) => Data::Number(
            scxml_types::FiniteNumber::new(value.as_f64().expect("a finite number"))
                .expect("a finite number"),
        ),
        Json::String(text) => Data::String(text.clone()),
        Json::Array(_) => Data::JsonArray(value.to_string()),
        Json::Object(_) => Data::JsonObject(value.to_string()),
    }
}

fn number(value: f64) -> Data {
    Data::Number(scxml_types::FiniteNumber::new(value).expect("a finite number"))
}

/// A host holding `message`, `provenance`, the event name and sequence 3.
fn host(message: &Json, provenance: &Json) -> FakeHost {
    FakeHost {
        values: BTreeMap::from([
            ("message".to_owned(), data(message)),
            ("provenance".to_owned(), data(provenance)),
            (
                "event".to_owned(),
                Data::String("bot.ability.conversation.input".into()),
            ),
            ("sequence".to_owned(), number(3.0)),
            ("undefined".to_owned(), Data::Undefined),
        ]),
        ..FakeHost::default()
    }
}

fn attributes(direction: &str) -> Vec<Attribute> {
    vec![
        attr("location", "committed"),
        attr("keylocation", "key"),
        attr("direction", direction),
        attr("event", "event"),
        attr("message", "message"),
        attr("sequence", "sequence"),
        attr("provenance", "provenance"),
    ]
}

/// The key and message `<conversation:commit>` assigns, in order.
fn commit_of(direction: &str, message: &Json, provenance: &Json) -> (String, Json) {
    let mut host = host(message, provenance);
    assert_eq!(
        execute("commit", &attributes(direction), &[], &mut host),
        Ok(Executed::Handled)
    );
    let [
        (key_location, Data::String(key)),
        (location, Data::JsonObject(text)),
    ] = host.assigned.as_slice()
    else {
        panic!("the key, then the message: {:?}", host.assigned);
    };
    assert_eq!(
        (key_location.as_str(), location.as_str()),
        ("key", "committed")
    );
    (
        key.clone(),
        serde_json::from_str(text).expect("the message is JSON"),
    )
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
        "../package/conversation.extension.json"
    ))
    .expect("the manifest is a valid extension package manifest");
    assert_eq!(manifest.namespace_uri, NAMESPACE_URI);
    assert_eq!(
        manifest.extension_artifact,
        "floppy_conversation.virtualized.wasm"
    );
    manifest
        .verify_schema(&scxml_validator_core::SchemaSource::new(
            "conversation.xsd",
            include_bytes!("../package/conversation.xsd").to_vec(),
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

#[test]
fn an_inbound_message_is_keyed_by_its_sorted_sources_and_carries_its_relationship() {
    let (key, message) = commit_of(
        "inbound",
        &json!({
            "source_ids": ["zed", "alice", [0.5, 1]],
            "target_ids": ["bot", "a\"b"],
            "content": "hello",
            "content_type": "text/plain",
            "sample_rate_hz": 16000
        }),
        &Json::Null,
    );
    assert_eq!(key, r#"["alice","zed",[0.5,1]]"#);
    assert_eq!(
        message,
        json!({
            "sequence": 3,
            "direction": "inbound",
            "source_ids": ["zed", "alice", [0.5, 1]],
            "target_ids": ["bot", "a\"b"],
            "content": "hello",
            "content_type": "text/plain",
            "provenance": {
                "event": "bot.ability.conversation.input",
                "id": null,
                "source": null,
                "target": null,
                "session_ref": r#"relationship-["alice","zed",[0.5,1]]["a\"b","bot"]"#,
                "turn_ref": null
            }
        })
    );
}

#[test]
fn inbound_audio_content_is_dropped_whatever_the_case_of_its_type() {
    for content_type in ["audio/pcm", "Audio/PCM", "AUDIO/wav"] {
        let (key, message) = commit_of(
            "inbound",
            &json!({"source_ids": ["alice"], "target_ids": [], "content": "AAAA", "content_type": content_type}),
            &Json::Null,
        );
        assert_eq!(key, r#"["alice"]"#);
        assert_eq!(message["content"], Json::Null, "{content_type}");
        assert_eq!(message["content_type"], json!(content_type));
        assert_eq!(
            message["provenance"]["session_ref"],
            json!(r#"relationship-["alice"][]"#)
        );
    }
    // Structured and other media content is kept; outbound audio is kept.
    let (_, message) = commit_of(
        "inbound",
        &json!({"source_ids": ["bob"], "target_ids": [], "content": {"b": 1, "a": [true, null]}, "content_type": "application/json"}),
        &Json::Null,
    );
    assert_eq!(message["content"], json!({"a": [true, null], "b": 1}));
    let (_, message) = commit_of(
        "outbound",
        &json!({"source_ids": [], "target_ids": [], "content": "AAAA", "content_type": "audio/pcm"}),
        &Json::Null,
    );
    assert_eq!(message["content"], json!("AAAA"));
}

#[test]
fn an_outbound_message_takes_its_provenance_references() {
    let (key, message) = commit_of(
        "outbound",
        &json!({
            "sequence": 0,
            "direction": "outbound",
            "source_ids": ["bot"],
            "target_ids": ["zed", "alice"],
            "content": "hi",
            "content_type": "text/plain",
            "provenance": {"event": "x.y"}
        }),
        &json!({"event": "x.y", "id": "r-1", "session_ref": "s1", "turn_ref": "t1", "source": "q"}),
    );
    assert_eq!(key, r#"["bot"]"#);
    assert_eq!(
        message,
        json!({
            "sequence": 3,
            "direction": "outbound",
            "source_ids": ["bot"],
            "target_ids": ["zed", "alice"],
            "content": "hi",
            "content_type": "text/plain",
            "provenance": {
                "event": "bot.ability.conversation.input",
                "id": "r-1",
                "source": null,
                "target": null,
                "session_ref": "s1",
                "turn_ref": "t1"
            }
        })
    );
}

#[test]
fn missing_references_and_content_are_null_and_a_non_string_id_is_null() {
    let (key, message) = commit_of(
        "outbound",
        &json!({"source_ids": [], "target_ids": [], "content_type": "text/plain"}),
        &json!({"id": 7}),
    );
    assert_eq!(key, "[]");
    assert_eq!(message["content"], Json::Null);
    assert_eq!(
        message["provenance"],
        json!({"event": "bot.ability.conversation.input", "id": null, "source": null, "target": null, "session_ref": null, "turn_ref": null})
    );
    // An undefined provenance, or none, is no provenance; an inbound
    // message ignores a given session_ref.
    let mut host = host(
        &json!({"source_ids": ["a"], "target_ids": [], "content_type": "text/plain"}),
        &Json::Null,
    );
    let mut without = attributes("inbound");
    without.retain(|attribute| attribute.local_name != "provenance");
    assert_eq!(
        execute("commit", &without, &[], &mut host),
        Ok(Executed::Handled)
    );
    let (_, inbound) = commit_of(
        "inbound",
        &json!({"source_ids": ["a"], "target_ids": [], "content_type": "text/plain"}),
        &json!({"session_ref": "given", "turn_ref": "t"}),
    );
    assert_eq!(
        inbound["provenance"]["session_ref"],
        json!(r#"relationship-["a"][]"#)
    );
    assert_eq!(inbound["provenance"]["turn_ref"], json!("t"));
}

#[test]
fn the_key_location_is_optional() {
    let mut host = host(
        &json!({"source_ids": ["a"], "target_ids": [], "content_type": "text/plain"}),
        &Json::Null,
    );
    let mut without = attributes("inbound");
    without.retain(|attribute| attribute.local_name != "keylocation");
    assert_eq!(
        execute("commit", &without, &[], &mut host),
        Ok(Executed::Handled)
    );
    assert_eq!(host.assigned.len(), 1);
    assert_eq!(host.assigned[0].0, "committed");
}

#[test]
fn identities_sort_by_their_ecmascript_json_text() {
    // Strings before embeddings ('"' < '['), UTF-16 order (U+FFFF sorts
    // after an astral character's lead surrogate), ECMAScript numbers.
    assert_eq!(
        sorted_ids(&[
            json!([1e-6, 1e21, -0.5, 2]),
            json!("\u{ffff}"),
            json!("\u{1f600}"),
            json!("b"),
            json!("a\nb"),
            json!([1e-7, 123_456.75]),
        ]),
        "[\"a\\nb\",\"b\",\"\u{1f600}\",\"\u{ffff}\",[0.000001,1e+21,-0.5,2],[1e-7,123456.75]]"
    );
    assert_eq!(sorted_ids(&[]), "[]");
}

#[test]
fn numbers_print_as_ecmascript_prints_them() {
    for (value, text) in [
        (0.0, "0"),
        (-0.0, "0"),
        (1.0, "1"),
        (-2.5, "-2.5"),
        (0.1, "0.1"),
        (1e-6, "0.000001"),
        (1.5e-7, "1.5e-7"),
        (1e20, "100000000000000000000"),
        (1e21, "1e+21"),
        (1.2345e25, "1.2345e+25"),
        (123_456.789, "123456.789"),
        (f64::MAX, "1.7976931348623157e+308"),
        (5e-324, "5e-324"),
    ] {
        assert_eq!(js_number(value), text, "{value:e}");
    }
}

#[test]
fn malformed_elements_and_values_are_execution_errors() {
    let good = json!({"source_ids": ["a"], "target_ids": [], "content_type": "text/plain"});
    let cases: Vec<(Vec<Attribute>, Json, &str)> = vec![
        (
            vec![
                attr("direction", "inbound"),
                attr("event", "event"),
                attr("message", "message"),
                attr("sequence", "sequence"),
            ],
            good.clone(),
            "conversation:commit requires a location",
        ),
        (
            {
                let mut a = attributes("inbound");
                a.retain(|x| x.local_name != "direction");
                a
            },
            good.clone(),
            "conversation:commit requires a direction",
        ),
        (
            attributes("sideways"),
            good.clone(),
            "conversation:commit direction is not inbound or outbound",
        ),
        (
            {
                let mut a = attributes("inbound");
                a.push(attr("extra", "x"));
                a
            },
            good.clone(),
            "conversation:commit takes only the location, keylocation, direction, event, message, sequence and provenance attributes",
        ),
        (
            {
                let mut a = attributes("inbound");
                a.push(attr("location", "again"));
                a
            },
            good.clone(),
            "conversation:commit repeats an attribute",
        ),
        (
            {
                let mut a = attributes("inbound");
                a[0] = attr("location", "  ");
                a
            },
            good.clone(),
            "conversation:commit has an empty attribute",
        ),
        (
            attributes("inbound"),
            json!([]),
            "conversation:commit message is not an object",
        ),
        (
            attributes("inbound"),
            json!({"source_ids": "a", "target_ids": [], "content_type": "text/plain"}),
            "conversation:commit message source_ids is not an array of identities",
        ),
        (
            attributes("inbound"),
            json!({"source_ids": ["a"], "target_ids": [{"x": 1}], "content_type": "text/plain"}),
            "conversation:commit message target_ids is not an array of identities",
        ),
        (
            attributes("inbound"),
            json!({"source_ids": [["x"]], "target_ids": [], "content_type": "text/plain"}),
            "conversation:commit message source_ids is not an array of identities",
        ),
        (
            attributes("inbound"),
            json!({"source_ids": ["a"], "target_ids": [], "content_type": ""}),
            "conversation:commit message content_type is not a non-empty string",
        ),
    ];
    for (attributes, message, detail) in cases {
        let mut host = host(&message, &Json::Null);
        assert_eq!(
            execution(execute("commit", &attributes, &[], &mut host)),
            detail
        );
        assert!(host.assigned.is_empty(), "{detail}");
    }
}

#[test]
fn values_of_the_wrong_kind_are_execution_errors() {
    let good = json!({"source_ids": ["a"], "target_ids": [], "content_type": "text/plain"});
    for (name, value, detail) in [
        (
            "event",
            Data::String(String::new()),
            "conversation:commit event is not a non-empty string",
        ),
        (
            "event",
            number(1.0),
            "conversation:commit event is not a non-empty string",
        ),
        (
            "sequence",
            number(-1.0),
            "conversation:commit sequence is not a non-negative integer",
        ),
        (
            "sequence",
            number(1.5),
            "conversation:commit sequence is not a non-negative integer",
        ),
        (
            "sequence",
            Data::String("1".into()),
            "conversation:commit sequence is not a non-negative integer",
        ),
        (
            "provenance",
            Data::String("p".into()),
            "conversation:commit provenance is not an object",
        ),
        (
            "provenance",
            Data::JsonArray("[]".into()),
            "conversation:commit provenance is not an object",
        ),
    ] {
        let mut host = host(&good, &Json::Null);
        host.values.insert(name.to_owned(), value);
        assert_eq!(
            execution(execute("commit", &attributes("inbound"), &[], &mut host)),
            detail
        );
        assert!(host.assigned.is_empty(), "{detail}");
    }
    for (name, detail) in [
        ("event", "conversation:commit event did not evaluate"),
        ("message", "conversation:commit message did not evaluate"),
        ("sequence", "conversation:commit sequence did not evaluate"),
        (
            "provenance",
            "conversation:commit provenance did not evaluate",
        ),
    ] {
        let mut host = host(&good, &Json::Null);
        host.values.remove(name);
        assert_eq!(
            execution(execute("commit", &attributes("inbound"), &[], &mut host)),
            detail
        );
    }
}

#[test]
fn content_failed_assignments_and_host_failures() {
    let good = json!({"source_ids": ["a"], "target_ids": [], "content_type": "text/plain"});
    let mut host = host(&good, &Json::Null);
    assert_eq!(
        execution(execute(
            "commit",
            &attributes("inbound"),
            &[Content::Start, Content::End],
            &mut host
        )),
        "conversation:commit holds no content"
    );
    assert_eq!(
        execute(
            "commit",
            &attributes("inbound"),
            &[Content::Text(" \n ".into())],
            &mut host
        ),
        Ok(Executed::Handled)
    );
    let mut host = host_failing(&good);
    assert_eq!(
        execution(execute("commit", &attributes("inbound"), &[], &mut host)),
        "conversation:commit could not assign keylocation"
    );
    let mut host = host_failing(&good);
    let mut without = attributes("inbound");
    without.retain(|attribute| attribute.local_name != "keylocation");
    assert_eq!(
        execution(execute("commit", &without, &[], &mut host)),
        "conversation:commit could not assign the message"
    );
    let mut host = self::host(&good, &Json::Null);
    let mut fatal = attributes("inbound");
    fatal[3] = attr("event", "fatal");
    assert_eq!(
        execute("commit", &fatal, &[], &mut host),
        Err(Failure::Host("fatal"))
    );
}

fn host_failing(message: &Json) -> FakeHost {
    FakeHost {
        fail_assign: true,
        ..host(message, &Json::Null)
    }
}
