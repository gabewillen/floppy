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
fn attention_biases_a_perceived_batch_percept_by_percept() {
    let percept = |device: &str| json!({"stimulus": input(&json!({"target_device": device})), "source": null, "participants": []});
    let perceived = |percepts: Json| json!({"event": "bot.perceived", "data": {"percepts": percepts}, "source": "session.floppy"});
    // The focus first, then each percept's device in arrival order, once.
    assert_eq!(
        bias(
            &perceived(json!([
                percept("fan"),
                percept("door"),
                percept("lamp"),
                percept("fan")
            ])),
            &json!("lamp")
        ),
        assigned(&json!("lamp"), &json!(["lamp", "fan"]))
    );
    assert_eq!(
        bias(
            &perceived(json!([percept("fan"), percept("lamp")])),
            &Json::Null
        ),
        assigned(&Json::Null, &json!(["fan", "lamp"]))
    );
    // A percept that is not a device's input names no device.
    let product = json!({"stimulus": {"event": "bot.ability.conversation.output",
                         "data": {"target_device": "lamp"}}, "source": null});
    assert_eq!(
        bias(&perceived(json!([product, percept("fan")])), &Json::Null),
        assigned(&Json::Null, &json!(["fan"]))
    );
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

/// The inputs of the projection checks, and what the element writes for
/// each: the system channel (`null` for none) and the content, laid out as
/// decision D141 records.
const INPUTS: &str = include_str!("testdata/model_facing_content.inputs.json");
const EXPECTED: &str = include_str!("testdata/model_facing_content.expected.json");

/// Every input attribute, each named after its input field (the
/// instructions as `system_text`).
const INPUT_ATTRIBUTES: [&str; 11] = [
    "stimulus",
    "self",
    "recent",
    "environment",
    "acted",
    "held",
    "prior_episodes",
    "standing_directives",
    "feedback",
    "instructions",
    "schema",
];

fn content_attributes() -> Vec<Attribute> {
    let mut attributes = vec![
        attr("location", "prompt"),
        attr("failurelocation", "prompt_failure"),
        attr("systemlocation", "system"),
    ];
    attributes.extend(INPUT_ATTRIBUTES.map(|name| attr(name, name)));
    attributes
}

/// A host holding one input's fields under the attribute names above.
fn content_host(input: &Json) -> FakeHost {
    FakeHost {
        values: INPUT_ATTRIBUTES
            .iter()
            .map(|name| ((*name).to_owned(), data(&input[*name])))
            .collect(),
        ..FakeHost::default()
    }
}

/// What the element assigns for `input` with `attributes`, by location.
fn assigned_with(input: &Json, attributes: &[Attribute]) -> BTreeMap<String, Data> {
    let mut host = content_host(input);
    assert_eq!(
        execute(CONTENT, attributes, &[], &mut host),
        Ok(Executed::Handled)
    );
    let locations: Vec<&str> = host
        .assigned
        .iter()
        .map(|(location, _)| location.as_str())
        .collect();
    let order: Vec<&str> = ["prompt_failure", "system", "fit", "prompt"]
        .into_iter()
        .filter(|location| locations.contains(location))
        .collect();
    assert_eq!(locations, order, "failure, system, fit, then the content");
    host.assigned.into_iter().collect()
}

/// What the element assigns for `input`: (failure, system, prompt).
fn projected(input: &Json) -> (Data, Data, Data) {
    let mut assigned = assigned_with(input, &content_attributes());
    let mut take = |location: &str| assigned.remove(location).expect("assigned");
    (take("prompt_failure"), take("system"), take("prompt"))
}

fn text(value: &Json) -> Data {
    value
        .as_str()
        .map_or(Data::Null, |text| Data::String(text.to_owned()))
}

#[test]
fn each_fixture_input_projects_to_its_expected_system_and_content() {
    let inputs: Map<String, Json> = serde_json::from_str(INPUTS).expect("the inputs parse");
    let expected: Map<String, Json> = serde_json::from_str(EXPECTED).expect("the outputs parse");
    assert_eq!(inputs.len(), expected.len());
    for (name, input) in &inputs {
        let want = &expected[name];
        assert_eq!(
            projected(input),
            (Data::Null, text(&want["system"]), text(&want["content"])),
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
        (Data::String(message), Data::Null, Data::Null) => message,
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[test]
fn a_projection_over_a_budget_fails_with_the_exceeded_budgets_message() {
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
    let mut feedback = stimulus_with(&json!({}));
    feedback["feedback"] = json!("f".repeat(65_537));
    assert_eq!(
        failed(&feedback),
        "prompt projection scalar length budget exceeded"
    );
    let mut reserved = stimulus_with(&json!({}));
    reserved["instructions"] = json!("z".repeat(65_000));
    reserved["schema"] = json!({"pad": "q".repeat(200_000)});
    assert_eq!(
        failed(&reserved),
        "prompt projection encoded byte budget exceeded"
    );
}

/// A projection input whose stimulus is `text`, recalling `episodes` and
/// `directives`.
fn recalling(text: &str, episodes: &[Json], directives: &[Json]) -> Json {
    json!({
        "stimulus": {"event": "bot.input", "source": "gabe", "data": {"text": text}},
        "prior_episodes": episodes, "standing_directives": directives,
        "environment": null, "instructions": null, "schema": null,
    })
}

fn prompt(input: &Json) -> String {
    match projected(input) {
        (Data::Null, _, Data::String(prompt)) => prompt,
        other => panic!("expected a prompt, got {other:?}"),
    }
}

/// `count` heard turns of `size` bytes of words each, numbered.
fn turns(count: usize, size: usize) -> Vec<Json> {
    (0..count)
        .map(|index| {
            json!({"at": "2026-07-07T12:00:00Z", "direction": "heard",
                   "text": format!("turn-{index}-{}", "w".repeat(size))})
        })
        .collect()
}

#[test]
fn recalled_episodes_over_the_budget_are_trimmed_to_the_newest_not_failed() {
    let episodes: Vec<Json> = (0..6)
        .map(|index| json!({"stimulus_name": format!("episode-{index}-{}", "x".repeat(60_000)), "output": []}))
        .collect();
    let directives: Vec<Json> = (0..3)
        .map(|index| json!({"text": format!("directive-{index}")}))
        .collect();
    let rendered = prompt(&recalling("incoming speech", &episodes, &directives));
    assert!(rendered.len() <= MAX_PROJECTION_BYTES);
    assert!(rendered.contains("incoming speech"));
    let kept: Vec<usize> = (0..6)
        .filter(|index| rendered.contains(&format!("episode-{index}-")))
        .collect();
    assert!(!kept.is_empty() && kept.len() < 6, "{kept:?}");
    assert_eq!(kept, ((6 - kept.len())..6).collect::<Vec<_>>());
    assert!(rendered.contains(&format!(
        "<prior_episodes count=\"{}\" omitted=\"{}\">",
        kept.len(),
        6 - kept.len()
    )));
    for index in 0..3 {
        assert!(rendered.contains(&format!("directive-{index}")));
    }
}

#[test]
fn recent_turns_claim_the_budget_first_and_keep_the_newest_uncounted() {
    let mut input = recalling("now", &[], &[json!({"text": "d".repeat(50_000)})]);
    input["recent"] = Json::Array(turns(8, 40_000));
    let rendered = prompt(&input);
    assert!(rendered.len() <= MAX_PROJECTION_BYTES);
    let kept: Vec<usize> = (0..8)
        .filter(|index| rendered.contains(&format!("turn-{index}-")))
        .collect();
    assert_eq!(kept, ((8 - kept.len())..8).collect::<Vec<_>>());
    assert!(!kept.is_empty() && kept.len() < 8, "{kept:?}");
    assert!(rendered.starts_with(&format!(
        "<cognition:recalled>\n  <recent omitted=\"{}\">\n    <memory:turn ",
        8 - kept.len()
    )));
    // The stable part took what the directive needed: the varying part is
    // left with its frame and the directive omitted, or left out.
    assert!(!rendered.contains(&"d".repeat(100)));
    assert!(rendered.ends_with("<object stimulus:event=\"bot.input\" stimulus:source=\"gabe\">\n  <content>\n    <entry key=\"text\" value=\"now\" />\n  </content>\n</object>"));
}

#[test]
fn recalled_items_trim_to_what_the_instructions_and_the_tool_leave() {
    let directives: Vec<Json> = (0..40)
        .map(|index| json!({"text": format!("memory-{index}-{}", "m".repeat(5_000))}))
        .collect();
    let mut input = recalling("incoming speech", &[], &directives);
    input["instructions"] = json!("i".repeat(60_000));
    input["schema"] = json!({"pad": "q".repeat(20_000)});
    let tool = input["schema"].to_string().len();
    let rendered = prompt(&input);
    assert!(60_000 + rendered.len() + tool <= MAX_PROJECTION_BYTES);
    assert!(rendered.contains("incoming speech"));
    let kept: Vec<usize> = (0..40)
        .filter(|index| rendered.contains(&format!("memory-{index}-")))
        .collect();
    assert!(!kept.is_empty() && kept.len() < 40, "{kept:?}");
    assert_eq!(kept, ((40 - kept.len())..40).collect::<Vec<_>>());
    assert!(rendered.contains(&format!("omitted=\"{}\"", 40 - kept.len())));
}

#[test]
fn recalled_items_that_fit_render_without_an_omitted_marker() {
    let rendered = prompt(&recalling(
        "now",
        &[json!({"stimulus_name": "a"})],
        &[json!({"text": "b"}), json!({"text": "c"})],
    ));
    assert!(rendered.starts_with("<cognition:recalled>\n  <prior_episodes count=\"1\">"));
    assert!(rendered.contains("<standing_directives count=\"2\">"));
    assert!(!rendered.contains("omitted="));
}

#[test]
fn a_recalled_item_over_a_scalar_budget_is_omitted_not_failed() {
    let directives = [
        json!({"text": "older"}),
        json!({"text": "z".repeat(70_000)}),
        json!({"text": "newest"}),
    ];
    let rendered = prompt(&recalling("now", &[], &directives));
    assert!(rendered.contains("older") && rendered.contains("newest"));
    assert!(!rendered.contains(&"z".repeat(100)));
    assert!(rendered.contains("<standing_directives count=\"2\" omitted=\"1\">"));
}

#[test]
fn every_recalled_item_left_out_leaves_the_field_empty_with_its_count() {
    let mut input = recalling("now", &[], &[json!({"text": "y".repeat(30_000)})]);
    input["instructions"] = json!("i".repeat(60_000));
    input["schema"] = json!({"pad": "q".repeat(180_000)});
    let rendered = prompt(&input);
    assert!(rendered.contains("<standing_directives count=\"0\" omitted=\"1\" />"));
}

#[test]
fn a_recalled_part_whose_frame_does_not_fit_is_left_out() {
    let mut input = recalling("now", &[], &[json!({"text": "small"})]);
    input["instructions"] = json!("i".repeat(60_000));
    let stimulus = prompt(&recalling("now", &[], &[])).len();
    // Room for the stimulus and its newline, not for the recalled frame.
    let pad = MAX_PROJECTION_BYTES - 60_000 - stimulus - 1 - "{\"pad\":\"\"}".len();
    input["schema"] = json!({"pad": "q".repeat(pad)});
    assert_eq!(prompt(&input).len(), stimulus);
}

#[test]
fn recalled_items_never_hide_a_stimulus_that_alone_overflows() {
    let mut input = recalling(&"s".repeat(20_000), &[], &[json!({"text": "small"})]);
    input["instructions"] = json!("i".repeat(60_000));
    input["schema"] = json!({"pad": "q".repeat(190_000)});
    assert_eq!(
        failed(&input),
        "prompt projection encoded byte budget exceeded"
    );
}

#[test]
fn trimmed_projections_fit_every_budget_they_admit() {
    let episodes = [
        json!({"stimulus_name": "it's <older> & \"quoted\"\nline", "focus_candidates": ["lamp"], "output": [{"target": "t", "event": "e", "data": {"k": [1, 2]}, "reason": "r"}]}),
        json!({"stimulus_name": "plain ".repeat(20), "behavior": {"event": "bot.behavior.create", "name": "n", "triggers": ["a"]}}),
    ];
    let directives = [
        json!({"text": "first"}),
        json!({"text": "é".repeat(300), "kind": "rule"}),
    ];
    let (mut fitted, mut overflowed) = (0, false);
    for pad in (MAX_PROJECTION_BYTES - 3_000..MAX_PROJECTION_BYTES - 150).step_by(7) {
        let mut input = recalling("now", &episodes, &directives);
        input["recent"] = Json::Array(turns(3, 200));
        input["instructions"] = json!("i".repeat(60_000));
        input["feedback"] = json!("repair");
        input["schema"] = json!({"pad": "q".repeat(pad - 60_000 - 10)});
        let tool = input["schema"].to_string().len();
        match projected(&input) {
            (Data::Null, _, Data::String(rendered)) => {
                assert!(!overflowed, "{pad}");
                fitted += 1;
                assert!(
                    60_000 + rendered.len() + tool <= MAX_PROJECTION_BYTES,
                    "{pad}"
                );
            }
            // Too few bytes left for the stimulus alone (the budgets
            // shrink as the pad grows).
            (Data::String(message), Data::Null, Data::Null) => {
                overflowed = true;
                assert_eq!(message, "prompt projection encoded byte budget exceeded");
            }
            other => panic!("{other:?}"),
        }
    }
    assert!(fitted > 300, "{fitted}");
}

/// What the element writes to its fit and content locations for `input`
/// fitted to `budget` bytes.
fn fitted(input: &Json, budget: f64) -> (Data, String) {
    let mut attributes = content_attributes();
    attributes.push(attr("fitlocation", "fit"));
    attributes.push(attr("budget", "budget"));
    let mut host = content_host(input);
    host.values
        .insert("budget".to_owned(), data(&json!(budget)));
    execute(CONTENT, &attributes, &[], &mut host).expect("handled");
    let mut assigned: BTreeMap<String, Data> = host.assigned.into_iter().collect();
    match (assigned.remove("fit"), assigned.remove("prompt")) {
        (Some(fit), Some(Data::String(prompt))) => (fit, prompt),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_fitted_budget_sends_whole_cuts_recalled_memory_or_overflows() {
    let mut input = recalling("now", &[], &[]);
    input["recent"] = Json::Array(turns(10, 1_000));
    input["instructions"] = json!("sys");
    input["schema"] = json!({"a": 1});
    let whole = prompt(&input);
    let beside = "sys".len() + "{\"a\":1}".len();
    #[expect(clippy::cast_precision_loss, reason = "small test sizes")]
    let bytes = |size: usize| (size + beside) as f64;
    assert_eq!(
        fitted(&input, bytes(whole.len())),
        (Data::String("fit".into()), whole.clone())
    );
    let (fit, cut) = fitted(&input, bytes(whole.len() - 1) + 0.9);
    assert_eq!(fit, Data::String("cut".into()));
    assert!(cut.len() < whole.len() && cut.contains("turn-9-") && !cut.contains("turn-0-"));
    assert!(cut.contains("<recent omitted=\"1\">"));
    let stimulus = prompt(&recalling("now", &[], &[]));
    assert_eq!(
        fitted(&input, bytes(stimulus.len() - 1)),
        (Data::String("overflow".into()), stimulus)
    );
    assert_eq!(
        fitted(&input, -5.0),
        (
            Data::String("overflow".into()),
            prompt(&recalling("now", &[], &[]))
        )
    );
    let mut attributes = content_attributes();
    attributes.push(attr("fitlocation", "fit"));
    assert_eq!(assigned_with(&input, &attributes)["fit"], Data::Null);
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
    let mut input = stimulus_with(&json!("hi"));
    input["instructions"] = Json::Null;
    let mut host = content_host(&input);
    let attributes = [attr("location", "prompt"), attr("stimulus", "stimulus")];
    execute(CONTENT, &attributes, &[], &mut host).expect("handled");
    let prompt = "<string stimulus:event=\"bot.input\" stimulus:source=\"gabe\">hi</string>";
    assert_eq!(
        host.assigned,
        [("prompt".to_owned(), Data::String(prompt.to_owned()))]
    );
    for name in [
        "self",
        "recent",
        "acted",
        "feedback",
        "instructions",
        "environment",
    ] {
        let mut host = content_host(&input);
        host.values.insert(name.to_owned(), Data::Null);
        assert_eq!(
            projected_from(&mut host),
            (Data::Null, Data::Null, Data::String(prompt.to_owned())),
            "{name}"
        );
    }
}

/// What the element assigns from `host`: (failure, system, prompt).
fn projected_from(host: &mut FakeHost) -> (Data, Data, Data) {
    execute(CONTENT, &content_attributes(), &[], host).expect("handled");
    let mut assigned: BTreeMap<String, Data> = host.assigned.drain(..).collect();
    let mut take = |location: &str| assigned.remove(location).expect("assigned");
    (take("prompt_failure"), take("system"), take("prompt"))
}

#[test]
fn an_acted_fact_over_its_bound_is_kept_as_its_bare_element() {
    let mut input = stimulus_with(&json!({}));
    input["instructions"] = Json::Null;
    input["acted"] = json!([{"target": "t", "event": "e", "data": {"text": "a".repeat(20_000)}}]);
    assert!(prompt(&input).starts_with("<cognition:acted omitted=\"size\" />\n<object "));
    input["acted"] = json!({"percepts": [0, 2], "acts": [{"target": "t", "event": "e"}]});
    assert!(prompt(&input).starts_with(
        "<cognition:acted>\n  <percepts count=\"2\">\n    <item>0</item>\n    <item>2</item>\n  </percepts>"
    ));
    input["acted"] = json!([]);
    assert!(prompt(&input).starts_with("<object "));
}

#[test]
fn numbers_are_written_as_json_writes_them() {
    for (value, text) in [
        (0.0, "0"),
        (-0.0, "0"),
        (3.0, "3"),
        (-12.0, "-12"),
        (1.5, "1.5"),
        (0.0005, "0.0005"),
        (0.000_01, "0.00001"),
        (1e-7, "1e-7"),
        (1.25e-10, "1.25e-10"),
        (1e16, "10000000000000000"),
        (1e21, "1e+21"),
        (1.5e22, "1.5e+22"),
        (-2.5e-8, "-2.5e-8"),
    ] {
        assert_eq!(ecmascript_number(value), text, "{value}");
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
            repeated,
            "cognition:model_facing_content repeats an attribute",
        ),
        (
            unknown,
            "cognition:model_facing_content takes only the location, failurelocation, systemlocation, fitlocation, stimulus, self, recent, environment, acted, prior_episodes, standing_directives, feedback, instructions, schema, budget and held attributes",
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
            "recent",
            data(&json!({})),
            "cognition:model_facing_content recent is not an array",
        ),
        (
            "prior_episodes",
            data(&json!({})),
            "cognition:model_facing_content prior_episodes is not an array",
        ),
        (
            "standing_directives",
            data(&json!("x")),
            "cognition:model_facing_content standing_directives is not an array",
        ),
        (
            "instructions",
            data(&json!(1)),
            "cognition:model_facing_content instructions is not a string",
        ),
        (
            "feedback",
            data(&json!([])),
            "cognition:model_facing_content feedback is not a string",
        ),
        (
            "environment",
            Data::Undefined,
            "cognition:model_facing_content environment is not a JSON value",
        ),
        (
            "self",
            Data::Undefined,
            "cognition:model_facing_content self is not a JSON value",
        ),
        (
            "acted",
            Data::Undefined,
            "cognition:model_facing_content acted is not a JSON value",
        ),
    ] {
        let mut host = content_host(&input);
        host.values.insert(name.to_owned(), value);
        assert_eq!(
            execution(execute(CONTENT, &content_attributes(), &[], &mut host)),
            detail,
            "{name}"
        );
        assert!(host.assigned.is_empty());
    }
    let mut host = content_host(&input);
    host.values.remove("schema");
    assert_eq!(
        execution(execute(CONTENT, &content_attributes(), &[], &mut host)),
        "cognition:model_facing_content schema did not evaluate"
    );
    let mut attributes = content_attributes();
    attributes.push(attr("budget", "budget"));
    let mut host = content_host(&input);
    host.values.insert("budget".to_owned(), data(&json!("10")));
    assert_eq!(
        execution(execute(CONTENT, &attributes, &[], &mut host)),
        "cognition:model_facing_content budget is not a number"
    );
    let mut host = content_host(&input);
    host.fail_assign = true;
    assert_eq!(
        execution(execute(CONTENT, &content_attributes(), &[], &mut host)),
        "cognition:model_facing_content could not assign failurelocation"
    );
    let mut attributes = content_attributes();
    attributes[3] = attr("stimulus", "fatal");
    assert_eq!(
        execute(CONTENT, &attributes, &[], &mut content_host(&input)),
        Err(Failure::Host("fatal"))
    );
}

#[test]
fn without_a_stimulus_the_content_has_no_stimulus_block() {
    let mut input = stimulus_with(&json!({}));
    input["instructions"] = Json::Null;
    input["acted"] = json!([{"target": "t", "event": "e"}]);
    input["feedback"] = json!("fix it");
    let attributes: Vec<Attribute> = content_attributes()
        .into_iter()
        .filter(|attribute| attribute.local_name != "stimulus")
        .collect();
    let mut assigned = assigned_with(&input, &attributes);
    let prompt = assigned.remove("prompt").expect("assigned");
    let Data::String(prompt) = prompt else {
        panic!("the content is a string: {prompt:?}");
    };
    assert!(prompt.starts_with("<cognition:acted>"), "{prompt}");
    assert!(prompt.ends_with("</cognition:acted>\nfix it"), "{prompt}");
    assert!(
        !prompt.contains("stimulus:") && !prompt.contains("<null"),
        "{prompt}"
    );
}

#[test]
fn a_device_on_an_object_renders_on_as_its_own_attribute() {
    let mut input = recalling("hi", &[], &[]);
    input["environment"] = json!({
        "id": "session",
        "self": {"state": "processing", "held_percepts": 0, "owned_devices": {
            "lamp": {"ref": "lamp", "id": "session.lamp", "state": "on", "on": "desk",
                     "attributes": {"brightness": 40}},
            "fan": {"ref": "fan", "id": "session.fan", "state": "off", "attributes": {}},
        }},
    });
    let prompt = prompt(&input);
    assert!(
        prompt.contains(r#"<device ref="lamp" id="session.lamp" state="on" on="desk">"#),
        "{prompt}"
    );
    assert!(
        prompt.contains(r#"<device ref="fan" id="session.fan" state="off" />"#),
        "{prompt}"
    );
    assert!(
        prompt.contains("<held_percepts>0</held_percepts>"),
        "{prompt}"
    );
}

#[test]
fn the_firmware_renders_as_a_device_does_before_the_owned_devices() {
    let mut input = recalling("hi", &[], &[]);
    input["environment"] = json!({
        "id": "session",
        "self": {"state": "processing", "owned_devices": {
            "lamp": {"ref": "lamp", "id": "session.lamp", "state": "on", "attributes": {}},
        }, "firmware": {"ref": "firmware", "id": "session.floppy.firmware", "state": "idle",
                        "attributes": {"minutes": 0}}},
    });
    let prompt = prompt(&input);
    let firmware = prompt
        .find(r#"<firmware ref="firmware" id="session.floppy.firmware" state="idle">"#)
        .unwrap_or_else(|| panic!("{prompt}"));
    let devices = prompt
        .find("<owned_devices>")
        .unwrap_or_else(|| panic!("{prompt}"));
    assert!(firmware < devices, "{prompt}");
    assert!(prompt.contains("<minutes>0</minutes>"), "{prompt}");
    assert!(!prompt.contains("<firmware>"), "{prompt}");
}

const DATA: &str = "model_facing_data";

/// What `<cognition:model_facing_data root="input" data="data">` assigns to
/// `prompt` for `data`.
fn model_facing(value: &Json) -> Result<String, Failure<&'static str>> {
    let mut host = FakeHost {
        values: BTreeMap::from([("data".to_owned(), data(value))]),
        ..FakeHost::default()
    };
    let attributes = [
        attr("location", "prompt"),
        attr("root", "input"),
        attr("data", "data"),
    ];
    execute(DATA, &attributes, &[], &mut host)?;
    match host.assigned.as_slice() {
        [(location, Data::String(text))] if location == "prompt" => Ok(text.clone()),
        assigned => panic!("one string assigned to prompt: {assigned:?}"),
    }
}

#[test]
fn model_facing_data_renders_an_object_as_its_fields() {
    let prompt = model_facing(&json!({
        "task": "design",
        "attempt": 2,
        "previous_plan": null,
        "names": ["a", "b"],
        "diagnostics": [{"code": "e1", "message": "it failed"}],
        "a key": "x",
    }))
    .expect("rendered");
    assert_eq!(
        prompt,
        "<input attempt=\"2\" task=\"design\">\n  <entry key=\"a key\" value=\"x\" />\n  \
         <diagnostics count=\"1\">\n    <item code=\"e1\" message=\"it failed\" />\n  \
         </diagnostics>\n  <names count=\"2\">\n    <item>a</item>\n    <item>b</item>\n  \
         </names>\n</input>"
    );
}

#[test]
fn model_facing_data_holds_a_document_as_it_is_written() {
    let document = "<scxml version=\"1.0\">\n  <final id=\"done\"/>\n</scxml>";
    let prompt = model_facing(&json!({"previous_scxml": document, "spec": "line one\nline two"}))
        .expect("rendered");
    assert_eq!(
        prompt,
        format!(
            "<input>\n  <previous_scxml><![CDATA[{document}]]></previous_scxml>\n  \
             <spec>line one\nline two</spec>\n</input>"
        )
    );
    let closing = model_facing(&json!({"text": "a ]]> b\n<c/>"})).expect("rendered");
    assert!(
        closing.contains("<![CDATA[a ]]]]><![CDATA[> b\n<c/>]]>"),
        "{closing}"
    );
}

#[test]
fn model_facing_data_holds_any_other_value_as_its_content() {
    assert_eq!(
        model_facing(&json!("hello")).expect("rendered"),
        "<input content=\"hello\" />"
    );
    assert_eq!(model_facing(&Json::Null).expect("rendered"), "<input />");
}

#[test]
fn model_facing_data_rejects_a_root_that_is_not_a_name() {
    let mut host = FakeHost::default();
    let attributes = [
        attr("location", "prompt"),
        attr("root", "not a name"),
        attr("data", "data"),
    ];
    assert_eq!(
        execute(DATA, &attributes, &[], &mut host),
        Err(Failure::Execution(
            "cognition:model_facing_data root is not an XML name"
        ))
    );
    assert!(host.assigned.is_empty());
}

#[test]
fn the_held_actions_render_as_the_held_block_after_the_acted_fact() {
    let mut input = stimulus_with(&json!({}));
    input["instructions"] = Json::Null;
    input["acted"] = json!([{"target": "t", "event": "e"}]);
    input["held"] = json!([
        {"event": "bot.ability.persona.revise", "description": "Revise <who> you are."},
        {"event": "bot.ability.identity.adopt"},
        {"description": "no event, not rendered"},
    ]);
    let rendered = prompt(&input);
    let held = "<held count=\"2\">\n  <event name=\"bot.ability.persona.revise\">Revise &lt;who&gt; you are.</event>\n  <event name=\"bot.ability.identity.adopt\" />\n</held>\n<object ";
    assert!(
        rendered.contains("</cognition:acted>\n<held count"),
        "{rendered}"
    );
    assert!(rendered.contains(held), "{rendered}");
    input["held"] = json!([]);
    assert!(!prompt(&input).contains("<held"));
    input["held"] = Json::Null;
    assert!(!prompt(&input).contains("<held"));
}

#[test]
fn a_held_block_over_its_bound_drops_its_descriptions_then_its_events() {
    let mut input = stimulus_with(&json!({}));
    input["instructions"] = Json::Null;
    input["held"] = json!([{"event": "e", "description": "d".repeat(20_000)}]);
    assert!(
        prompt(&input).starts_with("<held count=\"1\">\n  <event name=\"e\" />\n</held>\n<object ")
    );
    let many: Vec<Json> = (0..2_000)
        .map(|n| json!({"event": format!("event.number.{n}")}))
        .collect();
    input["held"] = Json::Array(many);
    assert!(prompt(&input).starts_with("<held count=\"2000\" omitted=\"size\" />\n<object "));
}

#[test]
fn a_held_that_is_not_an_array_raises_error_execution() {
    let mut host = content_host(&stimulus_with(&json!({})));
    host.values.insert("held".into(), Data::String("x".into()));
    assert_eq!(
        execution(execute(CONTENT, &content_attributes(), &[], &mut host)),
        "cognition:model_facing_content held is not an array"
    );
    assert!(host.assigned.is_empty());
}

// <cognition:within_ceiling>

fn ceiling_host(choices: &Json, ceiling: Option<Data>) -> FakeHost {
    let mut values = BTreeMap::from([("choices".to_owned(), data(choices))]);
    if let Some(ceiling) = ceiling {
        values.insert("ceiling".to_owned(), ceiling);
    }
    FakeHost {
        values,
        ..FakeHost::default()
    }
}

fn ceiling_attributes() -> Vec<Attribute> {
    vec![
        attr("location", "offered"),
        attr("heldlocation", "held"),
        attr("choices", "choices"),
        attr("ceiling", "ceiling"),
    ]
}

fn turn_choices() -> Json {
    json!([
        {"event": "bot.ability.cognition.ignore", "schema": {}, "target": "c"},
        {"event": "bot.ability.seeing.look", "schema": {}, "target": "s", "effort": 1},
        {"event": "bot.ability.persona.revise", "schema": {}, "target": "p", "effort": 2, "description": "Revise."},
        {"event": "bot.ability.identity.adopt", "schema": {}, "target": "i", "effort": 0},
        {"event": "bot.ability.persona.revise", "schema": {}, "target": "q", "effort": 3},
    ])
}

/// What `<cognition:within_ceiling>` assigns for `ceiling`, in order.
fn framed(ceiling: Option<Data>) -> Vec<(String, Json)> {
    let mut attributes = ceiling_attributes();
    if ceiling.is_none() {
        attributes.pop();
    }
    let mut host = ceiling_host(&turn_choices(), ceiling);
    assert_eq!(
        execute("within_ceiling", &attributes, &[], &mut host),
        Ok(Executed::Handled)
    );
    host.assigned
        .into_iter()
        .map(|(location, value)| match value {
            Data::JsonArray(text) => (location, serde_json::from_str(&text).expect("JSON")),
            other => panic!("expected an array, got {other:?}"),
        })
        .collect()
}

#[test]
fn within_ceiling_offers_the_choices_at_most_the_ceiling_and_holds_the_rest() {
    let assigned = framed(Some(Data::Number(
        scxml_types::FiniteNumber::new(1.0).expect("finite"),
    )));
    assert_eq!(
        assigned,
        [
            (
                "held".to_owned(),
                json!([{"event": "bot.ability.persona.revise", "description": "Revise."}])
            ),
            (
                "offered".to_owned(),
                json!([
                    {"event": "bot.ability.cognition.ignore", "schema": {}, "target": "c"},
                    {"event": "bot.ability.seeing.look", "schema": {}, "target": "s"},
                    {"event": "bot.ability.identity.adopt", "schema": {}, "target": "i"},
                ])
            ),
        ],
        "held first, once per event; offered in order, without effort"
    );
    let zero = framed(Some(Data::Number(
        scxml_types::FiniteNumber::new(0.0).expect("finite"),
    )));
    assert_eq!(
        zero[0].1,
        json!([
            {"event": "bot.ability.seeing.look"},
            {"event": "bot.ability.persona.revise", "description": "Revise."},
        ])
    );
}

#[test]
fn without_a_ceiling_the_whole_frame_is_offered_and_nothing_held() {
    for ceiling in [None, Some(Data::Null)] {
        let assigned = framed(ceiling);
        assert_eq!(assigned[0], ("held".to_owned(), json!([])));
        assert_eq!(assigned[1].1.as_array().map(Vec::len), Some(5));
        assert!(!assigned[1].1.to_string().contains("effort"));
    }
}

#[test]
fn an_event_offered_within_the_ceiling_by_one_target_is_not_held() {
    let choices = json!([
        {"event": "e", "schema": {}, "target": "a", "effort": 5},
        {"event": "e", "schema": {}, "target": "b"},
    ]);
    let framed = frame(&choices, Some(0)).expect("choices");
    assert!(framed.held.is_empty());
    assert_eq!(
        framed.offered,
        [json!({"event": "e", "schema": {}, "target": "b"})]
    );
}

#[test]
fn malformed_within_ceiling_elements_or_values_raise_error_execution() {
    let number = |value: f64| Data::Number(scxml_types::FiniteNumber::new(value).expect("finite"));
    for (attributes, choices, ceiling, detail) in [
        (
            vec![attr("choices", "choices")],
            turn_choices(),
            None,
            "cognition:within_ceiling requires a location",
        ),
        (
            vec![attr("location", "offered")],
            turn_choices(),
            None,
            "cognition:within_ceiling requires choices",
        ),
        (
            vec![
                attr("location", "offered"),
                attr("choices", "choices"),
                attr("effort", "x"),
            ],
            turn_choices(),
            None,
            "cognition:within_ceiling takes only the location, heldlocation, choices and ceiling attributes",
        ),
        (
            ceiling_attributes(),
            turn_choices(),
            Some(number(1.5)),
            "cognition:within_ceiling ceiling is not a whole number or null",
        ),
        (
            ceiling_attributes(),
            turn_choices(),
            Some(number(-1.0)),
            "cognition:within_ceiling ceiling is not a whole number or null",
        ),
        (
            ceiling_attributes(),
            turn_choices(),
            Some(Data::String("1".into())),
            "cognition:within_ceiling ceiling is not a whole number or null",
        ),
        (
            ceiling_attributes(),
            json!([{"event": "e", "effort": "heavy"}]),
            Some(number(1.0)),
            "cognition:within_ceiling choices is not an array of choices",
        ),
        (
            ceiling_attributes(),
            json!([{"schema": {}}]),
            Some(number(1.0)),
            "cognition:within_ceiling choices is not an array of choices",
        ),
        (
            ceiling_attributes(),
            json!({"event": "e"}),
            Some(number(1.0)),
            "cognition:within_ceiling choices is not an array of choices",
        ),
    ] {
        let mut host = ceiling_host(&choices, ceiling);
        assert_eq!(
            execution(execute("within_ceiling", &attributes, &[], &mut host)),
            detail
        );
        assert!(host.assigned.is_empty(), "{detail}: nothing is assigned");
    }
    let mut host = ceiling_host(&turn_choices(), Some(Data::Null));
    host.fail_assign = true;
    assert_eq!(
        execution(execute(
            "within_ceiling",
            &ceiling_attributes(),
            &[],
            &mut host
        )),
        "cognition:within_ceiling could not assign heldlocation"
    );
}
