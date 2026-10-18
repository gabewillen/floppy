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

impl CognitionHost for FakeHost {
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
        "../package/cognition.extension.json"
    ))
    .expect("the manifest is a valid extension package manifest");
    assert_eq!(manifest.namespace_uri, NAMESPACE_URI);
    assert_eq!(
        manifest.extension_artifact,
        "floppy_cognition.virtualized.wasm"
    );
    manifest
        .verify_schema(&scxml_validator_core::SchemaSource::new(
            "cognition.xsd",
            include_bytes!("../package/cognition.xsd").to_vec(),
        ))
        .expect("the schema digest, targetNamespace, and WIT identity verify");
}

#[test]
fn other_local_names_are_not_handled() {
    let mut host = FakeHost::default();
    assert_eq!(
        execute("attention", &[attr("location", "x")], &[], &mut host),
        Ok(Executed::NotHandled)
    );
    assert!(host.assigned.is_empty());
}

// <cognition:attention_bias_for_turn>

/// A host for one attention bias: the stimulus, the configured lamp and
/// fan, and the focus.
fn attention_host(stimulus: &Json, focus: &Json) -> FakeHost {
    FakeHost {
        values: BTreeMap::from([
            ("stimulus".to_owned(), data(stimulus)),
            ("devices".to_owned(), data(&json!(["lamp", "fan"]))),
            ("focused_device".to_owned(), data(focus)),
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
    ]
}

const ATTENTION: &str = "attention_bias_for_turn";

/// The focus and candidates `<cognition:attention_bias_for_turn>` assigns,
/// in order.
fn bias(stimulus: &Json, focus: &Json) -> Vec<(String, Data)> {
    let mut host = attention_host(stimulus, focus);
    assert_eq!(
        execute(ATTENTION, &attention_attributes(), &[], &mut host),
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
fn attention_takes_no_device_from_the_stimulus_source() {
    // A device's input reaches the bot with its target_device stamped by
    // the environment (decision D105): a source names no device.
    let mut stimulus = input(&json!({"target_device": null}));
    stimulus["source"] = json!("fan");
    assert_eq!(
        bias(&stimulus, &Json::Null),
        assigned(&Json::Null, &json!([]))
    );
    // Any other event: never a target_device in its data.
    let other = json!({"event": "bot.ability.conversation.output",
                       "data": {"target_device": "lamp"}, "source": "fan"});
    assert_eq!(bias(&other, &Json::Null), assigned(&Json::Null, &json!([])));
}

#[test]
fn attention_without_focuslocation_writes_the_candidates_only() {
    let mut host = attention_host(&input(&json!({})), &json!("lamp"));
    let attributes: Vec<Attribute> = attention_attributes()
        .into_iter()
        .filter(|attribute| attribute.local_name != "focuslocation")
        .collect();
    execute(ATTENTION, &attributes, &[], &mut host).expect("handled");
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
        with(attr("sources", "sources")),
        with(attr("focus", "focused_device")),
        with(Attribute {
            namespace_uri: Some("urn:other".into()),
            local_name: "x".into(),
            value: "y".into(),
        }),
    ] {
        let mut host = attention_host(&stimulus, &Json::Null);
        execution(execute(ATTENTION, &attributes, &[], &mut host));
        assert!(host.assigned.is_empty());
    }
    let mut attributes = attention_attributes();
    attributes[0] = attr("location", " ");
    execution(execute(
        ATTENTION,
        &attributes,
        &[],
        &mut attention_host(&stimulus, &Json::Null),
    ));
    execution(execute(
        ATTENTION,
        &attention_attributes(),
        &[Content::Start, Content::End],
        &mut attention_host(&stimulus, &Json::Null),
    ));
    // Values of the wrong kind, and an expression that fails.
    for (name, value) in [
        ("stimulus", data(&json!([]))),
        ("devices", data(&json!({}))),
        ("focused_device", Data::Undefined),
    ] {
        let mut host = attention_host(&stimulus, &Json::Null);
        host.values.insert(name.to_owned(), value);
        execution(execute(ATTENTION, &attention_attributes(), &[], &mut host));
        assert!(host.assigned.is_empty());
    }
    let mut host = attention_host(&stimulus, &Json::Null);
    host.values.remove("devices");
    execution(execute(ATTENTION, &attention_attributes(), &[], &mut host));
    let mut host = attention_host(&stimulus, &Json::Null);
    host.fail_assign = true;
    execution(execute(ATTENTION, &attention_attributes(), &[], &mut host));
    // A host failure passes through.
    let mut attributes = attention_attributes();
    attributes[2] = attr("stimulus", "fatal");
    assert_eq!(
        execute(
            ATTENTION,
            &attributes,
            &[],
            &mut attention_host(&stimulus, &Json::Null)
        ),
        Err(Failure::Host("fatal"))
    );
}

// <cognition:model_facing_content>

const CONTENT: &str = "model_facing_content";

/// The inputs of the projection checks, and what the JavaScript this
/// element replaced (`promptProjection()` of `abilities/reasoning/
/// reasoning.js`) rendered for each, the inputs' keys sorted as the data
/// model hands them over.
const INPUTS: &str = include_str!("testdata/model_facing_content.inputs.json");
const EXPECTED: &str = include_str!("testdata/model_facing_content.expected.json");

fn content_attributes() -> Vec<Attribute> {
    vec![
        attr("location", "prompt"),
        attr("failurelocation", "prompt_failure"),
        attr("stimulus", "stimulus"),
        attr("prior_episodes", "prior_episodes"),
        attr("standing_directives", "standing_directives"),
        attr("environment", "environment"),
        attr("instructions", "system"),
        attr("schema", "schema"),
    ]
}

/// A host holding one input's fields under the attribute names above.
fn content_host(input: &Json) -> FakeHost {
    let field = |key: &str| data(&input[key]);
    FakeHost {
        values: BTreeMap::from([
            ("stimulus".to_owned(), field("stimulus")),
            ("prior_episodes".to_owned(), field("prior_episodes")),
            (
                "standing_directives".to_owned(),
                field("standing_directives"),
            ),
            ("environment".to_owned(), field("environment")),
            ("system".to_owned(), field("instructions")),
            ("schema".to_owned(), field("schema")),
        ]),
        ..FakeHost::default()
    }
}

/// What the element assigns for `input`: (failure, prompt).
fn projected(input: &Json) -> (Data, Data) {
    let mut host = content_host(input);
    assert_eq!(
        execute(CONTENT, &content_attributes(), &[], &mut host),
        Ok(Executed::Handled)
    );
    match host.assigned.as_slice() {
        [(failure_location, failure), (location, prompt)]
            if failure_location == "prompt_failure" && location == "prompt" =>
        {
            (failure.clone(), prompt.clone())
        }
        other => panic!("expected the failure then the prompt, got {other:?}"),
    }
}

#[test]
fn the_projection_renders_what_the_javascript_it_replaced_rendered() {
    let inputs: Map<String, Json> = serde_json::from_str(INPUTS).expect("the inputs parse");
    let expected: Map<String, Json> = serde_json::from_str(EXPECTED).expect("the outputs parse");
    assert_eq!(inputs.len(), expected.len());
    for (name, input) in &inputs {
        let prompt = expected[name].as_str().expect("a prompt").to_owned();
        assert_eq!(
            projected(input),
            (Data::Null, Data::String(prompt)),
            "{name}"
        );
    }
}

/// A projection input with `data` as its stimulus's data.
fn stimulus_with(data: &Json) -> Json {
    json!({
        "stimulus": {"event": "bot.input", "source": "gabe", "data": data},
        "prior_episodes": [], "standing_directives": [], "environment": null,
        "instructions": "s", "schema": null,
    })
}

fn failed(input: &Json) -> String {
    match projected(input) {
        (Data::String(message), Data::Null) => message,
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[test]
fn a_projection_over_a_budget_fails_with_the_originals_message() {
    let items: Vec<Json> = (0..4097).map(Json::from).collect();
    assert_eq!(
        failed(&stimulus_with(&json!({"items": items}))),
        "prompt projection collection item budget exceeded"
    );
    assert_eq!(
        failed(&stimulus_with(&json!({"text": "x".repeat(65_537)}))),
        "prompt projection scalar length budget exceeded"
    );
    let nodes: Map<String, Json> = (0..1400)
        .map(|index| (format!("k{index:04}"), json!([[]])))
        .collect();
    assert_eq!(
        failed(&stimulus_with(&Json::Object(nodes))),
        "prompt projection node budget exceeded"
    );
    assert_eq!(
        failed(&stimulus_with(&json!({
            "a": "é".repeat(60_000), "b": "é".repeat(60_000), "c": "é".repeat(20_000),
        }))),
        "prompt projection encoded byte budget exceeded"
    );
    let mut instructions = stimulus_with(&json!({}));
    instructions["stimulus"] = Json::Null;
    instructions["instructions"] = json!("y".repeat(65_537));
    assert_eq!(
        failed(&instructions),
        "prompt projection scalar length budget exceeded"
    );
    let mut cumulative = stimulus_with(&json!({}));
    cumulative["instructions"] = json!("z".repeat(65_000));
    cumulative["schema"] = json!({"pad": "q".repeat(200_000)});
    assert_eq!(
        failed(&cumulative),
        "prompt projection cumulative encoded byte budget exceeded"
    );
}

#[test]
fn without_failurelocation_a_budget_is_error_execution() {
    let items: Vec<Json> = (0..4097).map(Json::from).collect();
    let mut host = content_host(&stimulus_with(&json!({"items": items})));
    let attributes: Vec<Attribute> = content_attributes()
        .into_iter()
        .filter(|attribute| attribute.local_name != "failurelocation")
        .collect();
    assert_eq!(
        execution(execute(CONTENT, &attributes, &[], &mut host)),
        "prompt projection collection item budget exceeded"
    );
    assert!(host.assigned.is_empty());
}

#[test]
fn the_optional_inputs_may_be_left_out() {
    let mut host = content_host(&stimulus_with(&json!("hi")));
    let attributes: Vec<Attribute> = content_attributes()
        .into_iter()
        .filter(|attribute| {
            !["failurelocation", "environment", "instructions", "schema"]
                .contains(&attribute.local_name.as_str())
        })
        .collect();
    execute(CONTENT, &attributes, &[], &mut host).expect("handled");
    let prompt = "<cognition:processor_input>\n  <host_input>\n    <processing:input>\n      \
                  <input content=\"hi\" stimulus:event=\"bot.input\" stimulus:source=\"gabe\" />\n    \
                  </processing:input>\n  </host_input>\n  <prior_episodes count=\"0\" />\n  \
                  <standing_directives count=\"0\" />\n</cognition:processor_input>";
    assert_eq!(
        host.assigned,
        [("prompt".to_owned(), Data::String(prompt.to_owned()))]
    );
}

#[test]
fn python_and_ecmascript_numbers_are_written_as_each_writes_them() {
    for (value, python, ecmascript) in [
        (0.0, "0", "0"),
        (-0.0, "0", "0"),
        (3.0, "3", "3"),
        (-12.0, "-12", "-12"),
        (1.5, "1.5", "1.5"),
        (0.0005, "0.0005", "0.0005"),
        (0.000_01, "1e-05", "0.00001"),
        (1e-7, "1e-07", "1e-7"),
        (1.25e-10, "1.25e-10", "1.25e-10"),
        (1e16, "10000000000000000", "10000000000000000"),
        (1e21, "1e+21", "1e+21"),
        (1.5e22, "1.5e+22", "1.5e+22"),
        (-2.5e-8, "-2.5e-08", "-2.5e-8"),
    ] {
        assert_eq!(python_number(value), python, "{value}");
        assert_eq!(ecmascript_number(value), ecmascript, "{value}");
    }
}

#[test]
fn malformed_content_elements_or_values_raise_error_execution() {
    let input = stimulus_with(&json!({}));
    let without = |name: &str| -> Vec<Attribute> {
        content_attributes()
            .into_iter()
            .filter(|attribute| attribute.local_name != name)
            .collect()
    };
    let mut repeated = content_attributes();
    repeated.push(attr("schema", "schema"));
    let mut unknown = content_attributes();
    unknown.push(attr("focus", "focus"));
    let mut empty = content_attributes();
    empty[0] = attr("location", " ");
    for (attributes, detail) in [
        (
            without("location"),
            "cognition:model_facing_content requires a location",
        ),
        (
            without("stimulus"),
            "cognition:model_facing_content requires a stimulus",
        ),
        (
            without("prior_episodes"),
            "cognition:model_facing_content requires prior_episodes",
        ),
        (
            without("standing_directives"),
            "cognition:model_facing_content requires standing_directives",
        ),
        (
            repeated,
            "cognition:model_facing_content repeats an attribute",
        ),
        (
            unknown,
            "cognition:model_facing_content takes only the location, failurelocation, stimulus, prior_episodes, standing_directives, environment, instructions and schema attributes",
        ),
        (
            empty,
            "cognition:model_facing_content has an empty attribute",
        ),
    ] {
        let mut host = content_host(&input);
        assert_eq!(
            execution(execute(CONTENT, &attributes, &[], &mut host)),
            detail
        );
        assert!(host.assigned.is_empty());
    }
}

#[test]
fn content_values_of_the_wrong_kind_raise_error_execution() {
    let input = stimulus_with(&json!({}));
    let mut host = content_host(&input);
    assert_eq!(
        execution(execute(
            CONTENT,
            &content_attributes(),
            &[Content::Text("x".into())],
            &mut host
        )),
        "cognition:model_facing_content holds no content"
    );
    for (name, value, detail) in [
        (
            "stimulus",
            data(&json!("text")),
            "cognition:model_facing_content stimulus is not an object or null",
        ),
        (
            "stimulus",
            data(&json!({"data": {}})),
            "cognition:model_facing_content stimulus has no event",
        ),
        (
            "prior_episodes",
            data(&json!({})),
            "cognition:model_facing_content prior_episodes is not an array",
        ),
        (
            "standing_directives",
            Data::Null,
            "cognition:model_facing_content standing_directives is not an array",
        ),
        (
            "system",
            data(&json!(1)),
            "cognition:model_facing_content instructions is not a string",
        ),
        (
            "environment",
            Data::Undefined,
            "cognition:model_facing_content environment is not a JSON value",
        ),
    ] {
        let mut host = content_host(&input);
        host.values.insert(name.to_owned(), value);
        assert_eq!(
            execution(execute(CONTENT, &content_attributes(), &[], &mut host)),
            detail
        );
        assert!(host.assigned.is_empty());
    }
    let mut host = content_host(&input);
    host.values.remove("schema");
    assert_eq!(
        execution(execute(CONTENT, &content_attributes(), &[], &mut host)),
        "cognition:model_facing_content schema did not evaluate"
    );
    let mut host = content_host(&input);
    host.fail_assign = true;
    assert_eq!(
        execution(execute(CONTENT, &content_attributes(), &[], &mut host)),
        "cognition:model_facing_content could not assign failurelocation"
    );
    let mut attributes = content_attributes();
    attributes[2] = attr("stimulus", "fatal");
    assert_eq!(
        execute(CONTENT, &attributes, &[], &mut content_host(&input)),
        Err(Failure::Host("fatal"))
    );
}
