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

fn data(value: &Json) -> Data {
    data_of(value).expect("a JSON value")
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

// <bot:attention>

/// A host for one attention bias: the stimulus, the configured lamp and
/// fan, the focus, and the sources (the mic reports from the fan).
fn attention_host(stimulus: &Json, focus: &Json) -> FakeHost {
    FakeHost {
        values: BTreeMap::from([
            ("stimulus".to_owned(), data(stimulus)),
            ("devices".to_owned(), data(&json!(["lamp", "fan"]))),
            ("focused_device".to_owned(), data(focus)),
            ("sources".to_owned(), data(&json!({"mic": "fan"}))),
        ]),
        ..FakeHost::default()
    }
}

fn attention_attributes() -> Vec<Attribute> {
    vec![
        attr("location", "focus_candidates"),
        attr("focuslocation", "focus"),
        attr("stimulus", "stimulus"),
        attr("devices", "devices"),
        attr("focus", "focused_device"),
        attr("sources", "sources"),
    ]
}

/// The focus and candidates `<bot:attention>` assigns, in order.
fn bias(stimulus: &Json, focus: &Json) -> Vec<(String, Data)> {
    let mut host = attention_host(stimulus, focus);
    assert_eq!(
        execute("attention", &attention_attributes(), &[], &mut host),
        Ok(Executed::Handled)
    );
    host.assigned
}

fn input(data: &Json) -> Json {
    json!({"event": "bot.input", "data": data, "source": null})
}

fn assigned(focus: &Json, candidates: &Json) -> Vec<(String, Data)> {
    vec![
        ("focus".to_owned(), data(focus)),
        ("focus_candidates".to_owned(), data(candidates)),
    ]
}

#[test]
fn attention_without_focus_or_device_has_no_candidates() {
    assert_eq!(
        bias(&input(&json!({"observation": {}})), &Json::Null),
        assigned(&Json::Null, &json!([]))
    );
}

#[test]
fn attention_takes_a_configured_target_device_of_an_input() {
    assert_eq!(
        bias(&input(&json!({"target_device": "lamp"})), &Json::Null),
        assigned(&Json::Null, &json!(["lamp"]))
    );
    // A device that is not configured is no candidate.
    assert_eq!(
        bias(&input(&json!({"target_device": "door"})), &Json::Null),
        assigned(&Json::Null, &json!([]))
    );
}

#[test]
fn attention_puts_the_configured_focus_first_once() {
    assert_eq!(
        bias(&input(&json!({"target_device": "fan"})), &json!("lamp")),
        assigned(&json!("lamp"), &json!(["lamp", "fan"]))
    );
    assert_eq!(
        bias(&input(&json!({"target_device": "lamp"})), &json!("lamp")),
        assigned(&json!("lamp"), &json!(["lamp"]))
    );
    // A focus that is no longer configured is dropped.
    assert_eq!(
        bias(&input(&json!({})), &json!("door")),
        assigned(&Json::Null, &json!([]))
    );
}

#[test]
fn attention_falls_back_to_the_device_the_source_reports_from() {
    // An input with no (or a null) target device, from the mic.
    let mut stimulus = input(&json!({"target_device": null}));
    stimulus["source"] = json!("mic");
    assert_eq!(
        bias(&stimulus, &Json::Null),
        assigned(&Json::Null, &json!(["fan"]))
    );
    // Any other event: the source's device only (mosfet
    // `_interrupt_reference`), never a target_device in its data.
    let other = json!({"event": "bot.ability.conversation.output",
                       "data": {"target_device": "lamp"}, "source": "mic"});
    assert_eq!(
        bias(&other, &Json::Null),
        assigned(&Json::Null, &json!(["fan"]))
    );
    // An unknown or empty source has no device.
    let other = json!({"event": "e", "data": {}, "source": "speaker"});
    assert_eq!(bias(&other, &Json::Null), assigned(&Json::Null, &json!([])));
    let other = json!({"event": "e", "data": {}, "source": ""});
    assert_eq!(bias(&other, &Json::Null), assigned(&Json::Null, &json!([])));
}

#[test]
fn attention_without_sources_or_focuslocation_writes_the_candidates_only() {
    let mut stimulus = input(&json!({}));
    stimulus["source"] = json!("mic");
    let mut host = attention_host(&stimulus, &json!("lamp"));
    let attributes: Vec<Attribute> = attention_attributes()
        .into_iter()
        .filter(|attribute| !matches!(attribute.local_name.as_str(), "sources" | "focuslocation"))
        .collect();
    execute("attention", &attributes, &[], &mut host).expect("handled");
    assert_eq!(
        host.assigned,
        [("focus_candidates".to_owned(), data(&json!(["lamp"])))]
    );
}

#[test]
fn malformed_attention_elements_or_values_raise_error_execution() {
    let stimulus = input(&json!({}));
    let without = |name: &str| -> Vec<Attribute> {
        attention_attributes()
            .into_iter()
            .filter(|attribute| attribute.local_name != name)
            .collect()
    };
    let with = |extra: Attribute| -> Vec<Attribute> {
        let mut attributes = attention_attributes();
        attributes.push(extra);
        attributes
    };
    for attributes in [
        without("location"),
        without("stimulus"),
        without("devices"),
        without("focus"),
        with(attr("model", "m")),
        with(attr("focus", "focused_device")),
        with(Attribute {
            namespace_uri: Some("urn:other".into()),
            local_name: "x".into(),
            value: "y".into(),
        }),
    ] {
        let mut host = attention_host(&stimulus, &Json::Null);
        execution(execute("attention", &attributes, &[], &mut host));
        assert!(host.assigned.is_empty());
    }
    let mut attributes = attention_attributes();
    attributes[0] = attr("location", " ");
    execution(execute(
        "attention",
        &attributes,
        &[],
        &mut attention_host(&stimulus, &Json::Null),
    ));
    execution(execute(
        "attention",
        &attention_attributes(),
        &[Content::Start, Content::End],
        &mut attention_host(&stimulus, &Json::Null),
    ));
    // Values of the wrong kind, and an expression that fails.
    for (name, value) in [
        ("stimulus", data(&json!([]))),
        ("devices", data(&json!({}))),
        ("sources", data(&json!([]))),
        ("focused_device", Data::Undefined),
    ] {
        let mut host = attention_host(&stimulus, &Json::Null);
        host.values.insert(name.to_owned(), value);
        execution(execute(
            "attention",
            &attention_attributes(),
            &[],
            &mut host,
        ));
        assert!(host.assigned.is_empty());
    }
    let mut host = attention_host(&stimulus, &Json::Null);
    host.values.remove("devices");
    execution(execute(
        "attention",
        &attention_attributes(),
        &[],
        &mut host,
    ));
    let mut host = attention_host(&stimulus, &Json::Null);
    host.fail_assign = true;
    execution(execute(
        "attention",
        &attention_attributes(),
        &[],
        &mut host,
    ));
    // A host failure passes through.
    let mut attributes = attention_attributes();
    attributes[2] = attr("stimulus", "fatal");
    assert_eq!(
        execute(
            "attention",
            &attributes,
            &[],
            &mut attention_host(&stimulus, &Json::Null)
        ),
        Err(Failure::Host("fatal"))
    );
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

/// What `<bot:recognize>` assigns for `name` and `text`.
fn recognized(name: &Data, text: &Data) -> Result<Vec<(String, Data)>, &'static str> {
    let mut host = FakeHost {
        values: BTreeMap::from([
            ("name".to_owned(), name.clone()),
            ("heard".to_owned(), text.clone()),
        ]),
        ..FakeHost::default()
    };
    match execute(
        "recognize",
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
        .expect("recognize runs")
        .as_slice()
    {
        [(location, Data::Bool(addressed))] if location == "addressed" => *addressed,
        other => panic!("unexpected assignment {other:?}"),
    }
}

#[test]
fn recognize_hears_the_name_as_a_whole_run_of_tokens() {
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
fn recognize_hears_nothing_without_a_name_or_a_text() {
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
fn recognize_refuses_a_name_or_text_that_is_not_a_string() {
    assert_eq!(
        recognized(&data(&json!(1)), &Data::String("x".into())),
        Err("bot:recognize name is not a string or null")
    );
    assert_eq!(
        recognized(&Data::String("x".into()), &data(&json!(["x"]))),
        Err("bot:recognize text is not a string or null")
    );
}

#[test]
fn recognize_refuses_a_malformed_element() {
    let mut host = FakeHost::default();
    assert_eq!(
        execution(execute(
            "recognize",
            &[attr("name", "name"), attr("text", "heard")],
            &[],
            &mut host
        )),
        "bot:recognize requires a location"
    );
    assert_eq!(
        execution(execute(
            "recognize",
            &[
                attr("location", "addressed"),
                attr("name", "name"),
                attr("text", "heard"),
                attr("confidence", "1"),
            ],
            &[],
            &mut host
        )),
        "bot:recognize takes only the location, name and text attributes"
    );
}
