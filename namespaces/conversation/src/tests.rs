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

/// A host holding `message`, `provenance` and the event name.
fn host(message: &Json, provenance: &Json) -> FakeHost {
    FakeHost {
        values: BTreeMap::from([
            ("message".to_owned(), data(message)),
            ("provenance".to_owned(), data(provenance)),
            (
                "event".to_owned(),
                Data::String("bot.ability.conversation.input".into()),
            ),
            ("undefined".to_owned(), Data::Undefined),
        ]),
        ..FakeHost::default()
    }
}

fn attributes(direction: &str) -> Vec<Attribute> {
    vec![
        attr("location", "committed"),
        attr("direction", direction),
        attr("event", "event"),
        attr("message", "message"),
        attr("provenance", "provenance"),
    ]
}

/// The message `<conversation:message_for_turn>` assigns.
fn commit_of(direction: &str, message: &Json, provenance: &Json) -> Json {
    let mut host = host(message, provenance);
    assert_eq!(
        execute("message_for_turn", &attributes(direction), &[], &mut host),
        Ok(Executed::Handled)
    );
    let [(location, Data::JsonObject(text))] = host.assigned.as_slice() else {
        panic!("the message: {:?}", host.assigned);
    };
    assert_eq!(location, "committed");
    serde_json::from_str(text).expect("the message is JSON")
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
fn an_inbound_message_carries_its_relationship_reference() {
    let message = commit_of(
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
    assert_eq!(
        message,
        json!({
            "direction": "inbound",
            "source_ids": ["zed", "alice", [0.5, 1]],
            "target_ids": ["bot", "a\"b"],
            "content": "hello",
            "content_type": "text/plain",
            "participants": [],
            "provenance": {
                "event": "bot.ability.conversation.input",
                "id": null,
                "source": null,
                "target": null,
                "session_ref": r#"relationship-{"identity_groups":[["a\"b","bot"],["alice","zed",[0.447213595499958,0.894427190999916]]]}"#,
                "turn_ref": null
            }
        })
    );
}

#[test]
fn inbound_audio_content_is_dropped_whatever_the_case_of_its_type() {
    for content_type in ["audio/pcm", "Audio/PCM", "AUDIO/wav"] {
        let message = commit_of(
            "inbound",
            &json!({"source_ids": ["alice"], "target_ids": [], "content": "AAAA", "content_type": content_type}),
            &Json::Null,
        );
        assert_eq!(message["content"], Json::Null, "{content_type}");
        assert_eq!(message["content_type"], json!(content_type));
        assert_eq!(
            message["provenance"]["session_ref"],
            json!(r#"relationship-{"identity_groups":[[],["alice"]]}"#)
        );
    }
    // Structured and other media content is kept; outbound audio is kept.
    let message = commit_of(
        "inbound",
        &json!({"source_ids": ["bob"], "target_ids": [], "content": {"b": 1, "a": [true, null]}, "content_type": "application/json"}),
        &Json::Null,
    );
    assert_eq!(message["content"], json!({"a": [true, null], "b": 1}));
    let message = commit_of(
        "outbound",
        &json!({"source_ids": [], "target_ids": [], "content": "AAAA", "content_type": "audio/pcm"}),
        &Json::Null,
    );
    assert_eq!(message["content"], json!("AAAA"));
}

#[test]
fn an_outbound_message_takes_its_provenance_references() {
    let message = commit_of(
        "outbound",
        &json!({
            "direction": "outbound",
            "source_ids": ["bot"],
            "target_ids": ["zed", "alice"],
            "content": "hi",
            "content_type": "text/plain",
            "provenance": {"event": "x.y"}
        }),
        &json!({"event": "x.y", "id": "r-1", "session_ref": "s1", "turn_ref": "t1", "source": "q", "target": 2}),
    );
    assert_eq!(
        message,
        json!({
            "direction": "outbound",
            "source_ids": ["bot"],
            "target_ids": ["zed", "alice"],
            "content": "hi",
            "content_type": "text/plain",
            "participants": [],
            "provenance": {
                "event": "bot.ability.conversation.input",
                "id": "r-1",
                "source": "q",
                "target": null,
                "session_ref": "s1",
                "turn_ref": "t1"
            }
        })
    );
}

#[test]
fn missing_references_and_content_are_null_and_a_non_string_id_is_null() {
    let message = commit_of(
        "outbound",
        &json!({"source_ids": [], "target_ids": [], "content_type": "text/plain"}),
        &json!({"id": 7}),
    );
    assert_eq!(message["content"], Json::Null);
    assert_eq!(
        message["provenance"],
        json!({"event": "bot.ability.conversation.input", "id": null, "source": null, "target": null, "session_ref": null, "turn_ref": null})
    );
    // An undefined provenance, or none, is no provenance; an inbound
    // message takes a given session_ref (its relationship's).
    let mut host = host(
        &json!({"source_ids": ["a"], "target_ids": [], "content_type": "text/plain"}),
        &Json::Null,
    );
    let mut without = attributes("inbound");
    without.retain(|attribute| attribute.local_name != "provenance");
    assert_eq!(
        execute("message_for_turn", &without, &[], &mut host),
        Ok(Executed::Handled)
    );
    let inbound = commit_of(
        "inbound",
        &json!({"source_ids": ["a"], "target_ids": [], "content_type": "text/plain"}),
        &json!({"session_ref": "given", "turn_ref": "t"}),
    );
    assert_eq!(inbound["provenance"]["session_ref"], json!("given"));
    assert_eq!(inbound["provenance"]["turn_ref"], json!("t"));
}

#[test]
fn a_messages_participants_are_kept_and_malformed_ones_are_execution_errors() {
    let participants = json!([{"ref": "alice", "role": "source"}, {"ref": "bot", "role": "target", "own": true},
               {"ref": "voice:1", "role": "source", "resembles": [{"ref": "voice:7b1e04", "similarity": 0.42}]}]);
    let message = commit_of(
        "outbound",
        &json!({"source_ids": [], "target_ids": [], "content_type": "text/plain", "participants": participants}),
        &Json::Null,
    );
    assert_eq!(message["participants"], participants);
    let message = commit_of(
        "outbound",
        &json!({"source_ids": [], "target_ids": [], "content_type": "text/plain", "participants": null}),
        &Json::Null,
    );
    assert_eq!(message["participants"], json!([]));
    let many: Vec<Json> = (0..33)
        .map(|n| json!({"ref": format!("p{n}"), "role": "source"}))
        .collect();
    for participants in [
        json!("alice"),
        json!([{"ref": "alice", "role": "speaker"}]),
        json!([{"ref": "", "role": "source"}]),
        json!([{"ref": "x".repeat(201), "role": "source"}]),
        json!([{"ref": "alice", "role": "source", "own": "yes"}]),
        json!([{"ref": "alice", "role": "source", "name": "Alice"}]),
        json!([{"ref": "alice", "role": "source", "resembles": [{"ref": "bob", "similarity": 2}]}]),
        json!([{"ref": "alice", "role": "source", "resembles": [{"ref": "", "similarity": 0}]}]),
        json!([{"ref": "alice", "role": "source", "resembles": [{"ref": "a", "similarity": 0}, {"ref": "b", "similarity": 0}, {"ref": "c", "similarity": 0}, {"ref": "d", "similarity": 0}]}]),
        Json::Array(many),
    ] {
        let mut host = host(
            &json!({"source_ids": [], "target_ids": [], "content_type": "text/plain", "participants": participants}),
            &Json::Null,
        );
        assert_eq!(
            execution(execute(
                "message_for_turn",
                &attributes("outbound"),
                &[],
                &mut host
            )),
            "conversation message participants are not at most 32 {ref, role, own?, resembles?}",
            "{participants}"
        );
        assert!(host.assigned.is_empty());
    }
}

/// `relationship_context_ref` writes each input's stored key byte for byte:
/// these texts are the `context_ref`s memories are kept under.
#[test]
fn the_relationship_context_reference_is_the_stored_key_byte_for_byte() {
    for (source_ids, target_ids, expected) in [
        (
            json!(["zed", "alice", [0.5, 1]]),
            json!(["bot", "a\"b"]),
            r#"{"identity_groups":[["a\"b","bot"],["alice","zed",[0.447213595499958,0.894427190999916]]]}"#,
        ),
        (
            json!(["bot", "a\"b"]),
            json!(["zed", "alice", [0.5, 1]]),
            r#"{"identity_groups":[["a\"b","bot"],["alice","zed",[0.447213595499958,0.894427190999916]]]}"#,
        ),
        (
            json!(["alice"]),
            json!([]),
            r#"{"identity_groups":[[],["alice"]]}"#,
        ),
        (
            json!(["\u{e9}", "\u{1f600}", "\u{ffff}", "a\nb", "\u{7f}", "b"]),
            json!(["bot"]),
            r#"{"identity_groups":[["a\nb","b","\u007f","\u00e9","\uffff","\ud83d\ude00"],["bot"]]}"#,
        ),
        (
            json!([[1e-6, 1, -0.5, 2], [3, 4], [1e-7, 123_456.75], [-0.0, 1]]),
            json!(["x"]),
            r#"{"identity_groups":[["x"],[[0.0,1.0],[0.6,0.8],[4.364357804719432e-07,0.4364357804719432,-0.2182178902359716,0.8728715609438864],[8.100002632500855e-13,1.0]]]}"#,
        ),
        (
            json!([[1e300, 1e300]]),
            json!([[0.1, 0.2, 0.3]]),
            r#"{"identity_groups":[[[0.26726124191242445,0.5345224838248489,0.8017837257372732]],[[0.7071067811865476,0.7071067811865476]]]}"#,
        ),
    ] {
        let (Json::Array(source_ids), Json::Array(target_ids)) = (source_ids, target_ids) else {
            panic!("identity lists are arrays");
        };
        assert_eq!(relationship_context_ref(&source_ids, &target_ids), expected);
    }
    // Repeated identities are one; a vector without a unit vector is kept.
    assert_eq!(
        relationship_context_ref(&[json!("a"), json!("a"), json!([0, 0]), json!([])], &[]),
        r#"{"identity_groups":[[],["a",[0.0,0.0],[]]]}"#
    );
}

#[test]
fn a_context_reference_writes_a_number_with_a_fraction_or_an_exponent() {
    for (value, text) in [
        (0.0, "0.0"),
        (-0.0, "-0.0"),
        (1.0, "1.0"),
        (-2.5, "-2.5"),
        (0.1, "0.1"),
        (1e-4, "0.0001"),
        (1e-5, "1e-05"),
        (1.5e-7, "1.5e-07"),
        (1e15, "1000000000000000.0"),
        (1e16, "1e+16"),
        (1.2345e25, "1.2345e+25"),
        (123_456.789, "123456.789"),
        (f64::MAX, "1.7976931348623157e+308"),
        (5e-324, "5e-324"),
    ] {
        assert_eq!(key_number(value), text, "{value:e}");
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
            ],
            good.clone(),
            "conversation:message_for_turn requires a location",
        ),
        (
            {
                let mut a = attributes("inbound");
                a.retain(|x| x.local_name != "direction");
                a
            },
            good.clone(),
            "conversation:message_for_turn requires a direction",
        ),
        (
            attributes("sideways"),
            good.clone(),
            "conversation:message_for_turn direction is not inbound or outbound",
        ),
        (
            {
                let mut a = attributes("inbound");
                a.push(attr("extra", "x"));
                a
            },
            good.clone(),
            "conversation:message_for_turn takes only the location, direction, event, message and provenance attributes",
        ),
        (
            {
                let mut a = attributes("inbound");
                a.push(attr("location", "again"));
                a
            },
            good.clone(),
            "conversation:message_for_turn repeats an attribute",
        ),
        (
            {
                let mut a = attributes("inbound");
                a[0] = attr("location", "  ");
                a
            },
            good.clone(),
            "conversation:message_for_turn has an empty attribute",
        ),
        (
            attributes("inbound"),
            json!([]),
            "conversation:message_for_turn message is not an object",
        ),
        (
            attributes("inbound"),
            json!({"source_ids": "a", "target_ids": [], "content_type": "text/plain"}),
            "conversation:message_for_turn message source_ids is not an array of identities",
        ),
        (
            attributes("inbound"),
            json!({"source_ids": ["a"], "target_ids": [{"x": 1}], "content_type": "text/plain"}),
            "conversation:message_for_turn message target_ids is not an array of identities",
        ),
        (
            attributes("inbound"),
            json!({"source_ids": [["x"]], "target_ids": [], "content_type": "text/plain"}),
            "conversation:message_for_turn message source_ids is not an array of identities",
        ),
        (
            attributes("inbound"),
            json!({"source_ids": ["a"], "target_ids": [], "content_type": ""}),
            "conversation:message_for_turn message content_type is not a non-empty string",
        ),
    ];
    for (attributes, message, detail) in cases {
        let mut host = host(&message, &Json::Null);
        assert_eq!(
            execution(execute("message_for_turn", &attributes, &[], &mut host)),
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
            "conversation:message_for_turn event is not a non-empty string",
        ),
        (
            "event",
            number(1.0),
            "conversation:message_for_turn event is not a non-empty string",
        ),
        (
            "provenance",
            Data::String("p".into()),
            "conversation:message_for_turn provenance is not an object",
        ),
        (
            "provenance",
            Data::JsonArray("[]".into()),
            "conversation:message_for_turn provenance is not an object",
        ),
    ] {
        let mut host = host(&good, &Json::Null);
        host.values.insert(name.to_owned(), value);
        assert_eq!(
            execution(execute(
                "message_for_turn",
                &attributes("inbound"),
                &[],
                &mut host
            )),
            detail
        );
        assert!(host.assigned.is_empty(), "{detail}");
    }
    for (name, detail) in [
        (
            "event",
            "conversation:message_for_turn event did not evaluate",
        ),
        (
            "message",
            "conversation:message_for_turn message did not evaluate",
        ),
        (
            "provenance",
            "conversation:message_for_turn provenance did not evaluate",
        ),
    ] {
        let mut host = host(&good, &Json::Null);
        host.values.remove(name);
        assert_eq!(
            execution(execute(
                "message_for_turn",
                &attributes("inbound"),
                &[],
                &mut host
            )),
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
            "message_for_turn",
            &attributes("inbound"),
            &[Content::Start, Content::End],
            &mut host
        )),
        "conversation:message_for_turn holds no content"
    );
    assert_eq!(
        execute(
            "message_for_turn",
            &attributes("inbound"),
            &[Content::Text(" \n ".into())],
            &mut host
        ),
        Ok(Executed::Handled)
    );
    let mut host = host_failing(&good);
    assert_eq!(
        execution(execute(
            "message_for_turn",
            &attributes("inbound"),
            &[],
            &mut host
        )),
        "conversation:message_for_turn could not assign the message"
    );
    let mut host = self::host(&good, &Json::Null);
    let mut fatal = attributes("inbound");
    fatal[2] = attr("event", "fatal");
    assert_eq!(
        execute("message_for_turn", &fatal, &[], &mut host),
        Err(Failure::Host("fatal"))
    );
}

fn host_failing(message: &Json) -> FakeHost {
    FakeHost {
        fail_assign: true,
        ..host(message, &Json::Null)
    }
}

/// The parity cases one JavaScript function wrote (`testdata/<name>.json`).
fn cases(text: &str) -> Vec<Json> {
    match serde_json::from_str(text) {
        Ok(Json::Array(cases)) => cases,
        other => panic!("the parity cases are a JSON array: {other:?}"),
    }
}

/// Runs one element over `values` and returns what it assigned, in order,
/// as JSON.
fn run(name: &str, attributes: &[Attribute], values: &[(&str, Data)]) -> Vec<(String, Json)> {
    let mut host = FakeHost {
        values: values
            .iter()
            .map(|(expression, value)| ((*expression).to_owned(), value.clone()))
            .collect(),
        ..FakeHost::default()
    };
    assert_eq!(
        execute(name, attributes, &[], &mut host),
        Ok(Executed::Handled),
        "{name}"
    );
    host.assigned
        .into_iter()
        .map(|(location, value)| {
            let json = match value {
                Data::JsonArray(text) | Data::JsonObject(text) => {
                    serde_json::from_str(&text).expect("assigned JSON")
                }
                Data::String(text) => Json::String(text),
                Data::Number(number) => json!(number.get()),
                Data::Bool(value) => Json::Bool(value),
                Data::Null => Json::Null,
                other => panic!("unexpected value {other:?}"),
            };
            (location, json)
        })
        .collect()
}

/// The relationships and index `relationshipIndexFor()` and
/// `relationshipsWithInput()` in `abilities/conversation/conversation.js`
/// chose (`testdata/relationship_for_input.json`, written by that
/// JavaScript; a new relationship's reference is this crate's
/// [`relationship_context_ref`], which the JavaScript took from
/// `<conversation:commit>`).
#[test]
fn relationship_for_input_chooses_as_the_javascript_it_replaces() {
    for case in cases(include_str!("testdata/relationship_for_input.json")) {
        let given = case["relationships"].as_array().expect("relationships");
        let mut expected = case["relationships_after"].clone();
        let index = case["index"].as_u64().expect("an index");
        if usize::try_from(index).expect("an index") == given.len() {
            let ids = |name: &str| case["input"][name].as_array().expect("ids").clone();
            let context_ref = relationship_context_ref(&ids("source_ids"), &ids("target_ids"));
            let added = &mut expected[usize::try_from(index).expect("an index")];
            added["context_ref"] = json!(context_ref);
            added["session_ref"] = json!(format!("relationship-{context_ref}"));
        }
        let assigned = run(
            "relationship_for_input",
            &[
                attr("location", "relationships"),
                attr("indexlocation", "relationship_index"),
                attr("input", "input"),
            ],
            &[
                ("relationships", data(&case["relationships"])),
                ("input", data(&case["input"])),
            ],
        );
        assert_eq!(
            assigned,
            vec![
                ("relationships".to_owned(), expected),
                (
                    "relationship_index".to_owned(),
                    json!(f64::from(u32::try_from(index).expect("small")))
                ),
            ],
            "{}",
            case["name"]
        );
    }
}

/// The relationship `isNewParticipant()`, `relationshipsWithParticipant()`
/// and `relationshipsWithoutSource()` in
/// `abilities/conversation/conversation.js` wrote
/// (`testdata/participant_for_source.json`, written by that JavaScript).
#[test]
fn participants_change_as_the_javascript_they_replace() {
    for case in cases(include_str!("testdata/participant_for_source.json")) {
        for (name, expected) in [
            ("participant_for_source", &case["added"]),
            ("fail_turn", &case["removed"]),
        ] {
            let assigned = run(
                name,
                &[
                    attr("location", "relationships[relationship_index]"),
                    attr("source", "source"),
                ],
                &[
                    (
                        "relationships[relationship_index]",
                        data(&case["relationship"]),
                    ),
                    ("source", data(&case["source"])),
                ],
            );
            assert_eq!(
                assigned,
                vec![(
                    "relationships[relationship_index]".to_owned(),
                    expected.clone()
                )],
                "{name} {case}"
            );
        }
    }
}

/// The references `detectorRefs()` in `abilities/conversation/conversation.js`
/// listed (`testdata/detector_refs.json`, written by that JavaScript).
#[test]
fn detector_refs_list_as_the_javascript_they_replace() {
    for case in cases(include_str!("testdata/detector_refs.json")) {
        let assigned = run(
            "detector_refs",
            &[
                attr("location", "refs"),
                attr("relationships", "relationships"),
            ],
            &[("relationships", data(&case["relationships"]))],
        );
        assert_eq!(
            assigned,
            vec![("refs".to_owned(), case["refs"].clone())],
            "{case}"
        );
    }
}

/// The tracks `trackRef()` in `abilities/conversation/conversation.js` wrote
/// (`testdata/new_track_ref.json`, written by that JavaScript), byte for
/// byte.
#[test]
fn new_track_ref_writes_what_the_javascript_it_replaces_wrote() {
    for case in cases(include_str!("testdata/new_track_ref.json")) {
        let assigned = run(
            "new_track_ref",
            &[attr("location", "track"), attr("source", "source")],
            &[("source", data(&case["source"]))],
        );
        assert_eq!(
            assigned,
            vec![("track".to_owned(), case["track"].clone())],
            "{case}"
        );
    }
    let assigned = run(
        "new_track_ref",
        &[attr("location", "track"), attr("source", "source")],
        &[("source", Data::Undefined)],
    );
    assert_eq!(assigned, vec![("track".to_owned(), Json::Null)]);
}

/// The stimuli `turnContent()` in `abilities/conversation/conversation.js`
/// built (`testdata/content_for_detector.json`, written by that JavaScript).
#[test]
fn content_for_detector_builds_what_the_javascript_it_replaces_built() {
    for case in cases(include_str!("testdata/content_for_detector.json")) {
        let assigned = run(
            "content_for_detector",
            &[
                attr("location", "content"),
                attr("input", "input"),
                attr("source", "source"),
            ],
            &[
                ("input", data(&case["input"])),
                ("source", data(&case["source"])),
            ],
        );
        assert_eq!(
            assigned,
            vec![("content".to_owned(), case["content"].clone())],
            "{case}"
        );
    }
}

/// The message `turnProduct()` and `turnProvenance()` in
/// `abilities/conversation/conversation.js` made, committed
/// (`testdata/complete_turn.json`, written by that JavaScript).
#[test]
fn complete_turn_commits_what_the_javascript_it_replaces_made() {
    for case in cases(include_str!("testdata/complete_turn.json")) {
        let (expected, _) = committed_message::<()>(
            Direction::Inbound,
            "bot.ability.conversation.input",
            case["product"].as_object().expect("a product"),
            case["provenance"].as_object().expect("a provenance"),
        )
        .expect("the product commits");
        let assigned = run(
            "complete_turn",
            &[
                attr("location", "message"),
                attr("event", "event"),
                attr("input", "input"),
                attr("turns", "turns"),
                attr("session_ref", "session"),
            ],
            &[
                (
                    "event",
                    Data::String("bot.ability.conversation.input".into()),
                ),
                ("input", data(&case["input"])),
                ("turns", data(&case["turns"])),
                ("session", data(&case["session_ref"])),
            ],
        );
        assert_eq!(
            assigned,
            vec![("message".to_owned(), Json::Object(expected))],
            "{case}"
        );
    }
}

#[test]
fn complete_turn_writes_the_participants_it_is_given() {
    let participants = json!([{"ref": "alice", "role": "source"}]);
    let attributes = [
        attr("location", "message"),
        attr("event", "event"),
        attr("input", "input"),
        attr("turns", "turns"),
        attr("session_ref", "session"),
        attr("participants", "participants"),
    ];
    let values = [
        (
            "event",
            Data::String("bot.ability.conversation.input".into()),
        ),
        (
            "input",
            data(&json!({"source_ids": ["alice"], "target_ids": [], "content_type": "text/plain"})),
        ),
        ("turns", data(&json!([{"turn_ref": "t1", "text": "hi"}]))),
        ("session", Data::String("s".into())),
        ("participants", data(&participants)),
    ];
    let assigned = run("complete_turn", &attributes, &values);
    assert_eq!(assigned[0].1["participants"], participants);
    let assigned = run("complete_turn", &attributes[..5], &values[..4]);
    assert_eq!(assigned[0].1["participants"], json!([]));
}

/// The open turn `textPartsWith()`, `audioWith()`, `sampleRateWith()`,
/// `channelsWith()`, `contentWith()` and `contentTypeWith()` in
/// `abilities/turn_detector/turn_detector.js` kept, and the turn
/// `closedTurn()` closed (`testdata/turn_detector.json`, written by that
/// JavaScript).
#[test]
fn the_turn_detector_ingests_and_ends_as_the_javascript_it_replaces() {
    for case in cases(include_str!("testdata/turn_detector.json")) {
        let mut turn = Json::Null;
        for (stimulus, expected) in case["stimuli"]
            .as_array()
            .expect("stimuli")
            .iter()
            .zip(case["states"].as_array().expect("states"))
        {
            let assigned = run(
                "ingest_content",
                &[attr("location", "turn"), attr("content", "stimulus")],
                &[("turn", data(&turn)), ("stimulus", data(stimulus))],
            );
            let [(location, state)] = assigned.as_slice() else {
                panic!("one assignment: {assigned:?}");
            };
            assert_eq!(location, "turn");
            assert_eq!(state, expected, "{}", case["name"]);
            turn = state.clone();
        }
        let assigned = run(
            "on_end",
            &[
                attr("location", "closed"),
                attr("turn", "turn"),
                attr("participant_ref", "participant_ref"),
                attr("conversation_ref", "active_conversation_ref"),
                attr("turn_ref", "turn_ref"),
                attr("source_participant_ref", "source_participant_ref"),
            ],
            &[
                ("turn", data(&turn)),
                ("participant_ref", Data::String("alice".into())),
                (
                    "active_conversation_ref",
                    Data::String("relationship-x".into()),
                ),
                ("turn_ref", Data::String("input-1:turn:alice".into())),
                ("source_participant_ref", Data::String("alice".into())),
            ],
        );
        assert_eq!(
            assigned,
            vec![("closed".to_owned(), case["closed"].clone())],
            "{}",
            case["name"]
        );
    }
}

#[test]
fn anything_but_a_turn_content_is_the_cleared_content() {
    let assigned = run(
        "ingest_content",
        &[attr("location", "turn"), attr("content", "stimulus")],
        &[
            ("turn", Data::String("garbage".into())),
            ("stimulus", Data::Null),
        ],
    );
    assert_eq!(
        assigned,
        vec![(
            "turn".to_owned(),
            json!({"text_parts": [], "audio": [], "audio_sample_rate_hz": null, "audio_channels": null, "content": null, "content_type": null})
        )]
    );
}

#[test]
fn ecmascript_trim_is_not_rusts() {
    assert_eq!(js_trim("\u{feff} a \u{3000}"), "a");
    assert_eq!(js_trim("\u{85}a"), "\u{85}a");
}

#[test]
fn malformed_new_elements_raise_error_execution_and_assign_nothing() {
    let relationship =
        json!({"session_ref": "r", "source_ids": [], "target_ids": [], "participants": []});
    for (name, attributes, values, detail) in [
        (
            "relationship_for_input",
            vec![attr("location", "rs"), attr("input", "input")],
            vec![],
            "conversation:relationship_for_input requires an indexlocation",
        ),
        (
            "relationship_for_input",
            vec![
                attr("location", "rs"),
                attr("indexlocation", "i"),
                attr("input", "input"),
            ],
            vec![
                ("rs", data(&json!([{"session_ref": 1}]))),
                (
                    "input",
                    data(&json!({"source_ids": [], "target_ids": ["a"]})),
                ),
            ],
            "conversation:relationship_for_input location is not an array of relationships",
        ),
        (
            "relationship_for_input",
            vec![
                attr("location", "rs"),
                attr("indexlocation", "i"),
                attr("input", "input"),
            ],
            vec![
                ("rs", data(&json!([]))),
                ("input", data(&json!({"source_ids": "a", "target_ids": []}))),
            ],
            "conversation:relationship_for_input input source_ids is not an array of identities",
        ),
        (
            "participant_for_source",
            vec![attr("location", "r"), attr("source", "s")],
            vec![("r", data(&relationship)), ("s", data(&json!({"a": 1})))],
            "conversation:participant_for_source source is not an identity",
        ),
        (
            "fail_turn",
            vec![attr("location", "r"), attr("source", "s")],
            vec![
                ("r", data(&json!({"participants": 1}))),
                ("s", data(&json!("a"))),
            ],
            "conversation:fail_turn location is not a relationship",
        ),
    ] {
        let mut host = FakeHost {
            values: values
                .into_iter()
                .map(|(expression, value)| (expression.to_owned(), value))
                .collect(),
            ..FakeHost::default()
        };
        assert_eq!(
            execution(execute(name, &attributes, &[], &mut host)),
            detail
        );
        assert!(host.assigned.is_empty(), "{detail}");
    }
}

#[test]
fn malformed_other_elements_raise_error_execution_and_assign_nothing() {
    for (name, attributes, values, detail) in [
        (
            "detector_refs",
            vec![attr("location", "l"), attr("relationships", "r")],
            vec![("r", data(&json!({})))],
            "conversation:detector_refs relationships is not an array of relationships",
        ),
        (
            "new_track_ref",
            vec![attr("location", "l"), attr("source", "s"), attr("x", "1")],
            vec![],
            "conversation:new_track_ref takes only the location and source attributes",
        ),
        (
            "content_for_detector",
            vec![
                attr("location", "l"),
                attr("input", "i"),
                attr("source", "s"),
            ],
            vec![
                ("i", data(&json!({"content": "x"}))),
                ("s", data(&json!("a"))),
            ],
            "conversation:content_for_detector input content_type is not a string",
        ),
        (
            "complete_turn",
            vec![
                attr("location", "l"),
                attr("event", "e"),
                attr("input", "i"),
                attr("turns", "t"),
                attr("session_ref", "r"),
            ],
            vec![
                ("e", Data::String("x".into())),
                (
                    "i",
                    data(
                        &json!({"source_ids": [], "target_ids": [], "content_type": "text/plain"}),
                    ),
                ),
                ("t", data(&json!([]))),
                ("r", Data::String("r".into())),
            ],
            "conversation:complete_turn turns is not an array of turns",
        ),
        (
            "ingest_content",
            vec![attr("location", "l"), attr("content", "c")],
            vec![("l", Data::Null), ("c", data(&json!("text")))],
            "conversation:ingest_content content is not a stimulus or null",
        ),
        (
            "on_end",
            vec![attr("location", "l"), attr("turn", "t")],
            vec![],
            "conversation:on_end requires a participant_ref",
        ),
    ] {
        let mut host = FakeHost {
            values: values
                .into_iter()
                .map(|(expression, value)| (expression.to_owned(), value))
                .collect(),
            ..FakeHost::default()
        };
        assert_eq!(
            execution(execute(name, &attributes, &[], &mut host)),
            detail
        );
        assert!(host.assigned.is_empty(), "{detail}");
    }
}

// <conversation:turn_participants>

/// What `<conversation:turn_participants>` writes for `input` and `parent`.
fn stamped(input: &Json, parent: Option<&Json>) -> Json {
    let mut host = FakeHost::default();
    host.values.insert("input".to_owned(), data(input));
    let mut attributes = vec![attr("location", "participants"), attr("input", "input")];
    if let Some(parent) = parent {
        host.values.insert("parent".to_owned(), data(parent));
        attributes.push(attr("parent", "parent"));
    }
    assert_eq!(
        execute("turn_participants", &attributes, &[], &mut host),
        Ok(Executed::Handled)
    );
    match host.assigned.as_slice() {
        [(location, Data::JsonArray(text))] if location == "participants" => {
            serde_json::from_str(text).expect("the participants are JSON")
        }
        other => panic!("expected the participants, got {other:?}"),
    }
}

#[test]
fn a_turn_stamps_its_names_as_sources_then_targets_and_never_an_embedding() {
    assert_eq!(
        stamped(
            &json!({"source_ids": ["zoe", [0.1, 0.2], "ann"], "target_ids": ["floppy"]}),
            None,
        ),
        json!([
            {"ref": "ann", "role": "source"},
            {"ref": "zoe", "role": "source"},
            {"ref": "floppy", "role": "target"},
        ])
    );
}

#[test]
fn a_turn_carries_its_parents_participants_after_its_own_once_each() {
    let parent = json!({"event": "bot.ability.listening.speech.output", "data": {"participants": [
        {"ref": "ann", "role": "source"},
        {"ref": "person-42", "role": "source"},
    ]}});
    assert_eq!(
        stamped(
            &json!({"source_ids": ["ann"], "target_ids": []}),
            Some(&parent)
        ),
        json!([
            {"ref": "ann", "role": "source"},
            {"ref": "person-42", "role": "source"},
        ])
    );
    // A parent with no participants, or none at all, carries nothing.
    let input = json!({"source_ids": ["ann"], "target_ids": []});
    for parent in [
        json!(null),
        json!({"event": "bot.ability.communication.input", "data": {"source_ids": ["ann"]}}),
    ] {
        assert_eq!(
            stamped(&input, Some(&parent)),
            json!([{"ref": "ann", "role": "source"}])
        );
    }
}

#[test]
fn a_ref_over_200_characters_is_not_stamped_and_the_list_is_bounded_at_32() {
    let long = "é".repeat(201);
    let names: Vec<String> = (0..40).map(|index| format!("p{index:02}")).collect();
    let participants = stamped(
        &json!({"source_ids": [long, "é".repeat(200)], "target_ids": names}),
        None,
    );
    let participants = participants.as_array().expect("an array");
    assert_eq!(participants.len(), MAX_PARTICIPANTS);
    assert_eq!(
        participants[0],
        json!({"ref": "é".repeat(200), "role": "source"})
    );
    assert_eq!(participants[1], json!({"ref": "p00", "role": "target"}));
    assert_eq!(participants[31], json!({"ref": "p30", "role": "target"}));
}

#[test]
fn turn_participants_of_the_wrong_kind_raise_error_execution() {
    let mut host = FakeHost::default();
    host.values
        .insert("input".to_owned(), data(&json!({"source_ids": ["ann"]})));
    host.values.insert(
        "parent".to_owned(),
        data(&json!({"data": {"participants": [{"ref": "ann", "role": "speaker"}]}})),
    );
    host.values.insert(
        "good".to_owned(),
        data(&json!({"source_ids": [], "target_ids": []})),
    );
    let attributes = [attr("location", "participants"), attr("input", "input")];
    assert_eq!(
        execution(execute("turn_participants", &attributes, &[], &mut host)),
        "conversation:turn_participants input is not an object with source_ids and target_ids"
    );
    let attributes = [
        attr("location", "participants"),
        attr("input", "good"),
        attr("parent", "parent"),
    ];
    assert_eq!(
        execution(execute("turn_participants", &attributes, &[], &mut host)),
        "conversation:turn_participants parent carries participants that are not {ref, role}"
    );
    assert_eq!(
        execution(execute(
            "turn_participants",
            &[attr("input", "good")],
            &[],
            &mut host
        )),
        "conversation:turn_participants requires a location"
    );
    assert!(host.assigned.is_empty());
}
