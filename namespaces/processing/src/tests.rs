use std::collections::BTreeMap;

use serde_json::json;

use super::*;

/// A deterministic fake host: values are read from a table (`_sessionid`
/// is `session.floppy` unless a test replaces it), assignments are
/// recorded.
struct FakeHost {
    assigned: Vec<(String, Data)>,
    fail_assign: bool,
    values: BTreeMap<String, Data>,
}

impl Default for FakeHost {
    fn default() -> Self {
        let mut values = BTreeMap::new();
        values.insert("_sessionid".into(), Data::String("session.floppy".into()));
        Self {
            assigned: Vec::new(),
            fail_assign: false,
            values,
        }
    }
}

impl ProcessingHost for FakeHost {
    type Error = &'static str;

    fn read(&mut self, location: &str) -> Result<Result<Data, ()>, &'static str> {
        if location == "fatal" {
            return Err("fatal");
        }
        Ok(self.values.get(location).cloned().ok_or(()))
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
        "../package/processing.extension.json"
    ))
    .expect("the manifest is a valid extension package manifest");
    assert_eq!(manifest.namespace_uri, NAMESPACE_URI);
    assert_eq!(
        manifest.extension_artifact,
        "floppy_processing.virtualized.wasm"
    );
    // Executable content is synchronous and does no I/O: the manifest
    // declares no origin and no slot.
    manifest
        .verify_schema(&scxml_validator_core::SchemaSource::new(
            "processing.xsd",
            include_bytes!("../package/processing.xsd").to_vec(),
        ))
        .expect("the schema digest, targetNamespace, and WIT identity verify");
}

fn location(value: &str) -> Attribute {
    attr("location", value)
}
/// A host whose `offers` location holds `choices`.
fn holding(choices: &Json) -> FakeHost {
    let mut host = FakeHost::default();
    host.values
        .insert("offers".into(), Data::JsonArray(canonical_json(choices)));
    host
}

/// Executes `<processing:model_facing_schema>` with `attributes` on `host`.
fn model_facing_with(
    attributes: &[Attribute],
    host: &mut FakeHost,
) -> Result<Executed, Failure<&'static str>> {
    execute("model_facing_schema", attributes, &[], host)
}

/// The value last assigned to `location`.
fn assigned(host: &FakeHost, location: &str) -> Data {
    host.assigned
        .iter()
        .rev()
        .find(|(at, _)| at == location)
        .map_or_else(
            || panic!("nothing was assigned to {location}"),
            |(_, value)| value.clone(),
        )
}

fn assigned_json(host: &FakeHost, location: &str) -> Json {
    match assigned(host, location) {
        Data::JsonArray(text) => serde_json::from_str(&text).expect("canonical JSON"),
        other => panic!("expected a JSON array, got {other:?}"),
    }
}

/// An offer as `<decision:choice>` writes it, with every keyword the
/// compaction drops at the top and nested, and a property named `title`.
fn verbose_offers() -> Json {
    json!([{
        "event": "bot.ability.communication.respond",
        "description": "Say a reply.",
        "target": "communication",
        "schema": {
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "bot.ability.communication.respond",
            "description": "Say a reply.",
            "examples": [{"parts": [{"words": "hi"}]}],
            "type": "object",
            "properties": {
                "parts": {"type": "array", "title": "Parts", "description": "What to say.",
                          "items": {"type": "object", "properties": {"words": {"type": "string", "title": "Words", "examples": ["hi"]}}}},
                "title": {"type": "string", "default": {"title": "kept"}},
            },
            "$defs": {"examples": {"enum": [{"title": "kept"}], "title": "dropped"}},
            "anyOf": [{"title": "dropped", "required": ["title"]}],
            "required": ["parts"],
            "additionalProperties": false,
        },
    }])
}

#[test]
fn model_facing_schema_drops_schema_title_and_examples_and_keeps_property_names() {
    let mut host = holding(&verbose_offers());
    let over = attr("overlocation", "over");
    assert_eq!(
        model_facing_with(&[location("offers"), over], &mut host),
        Ok(Executed::Handled)
    );
    assert_eq!(
        assigned_json(&host, "offers"),
        json!([{
            "event": "bot.ability.communication.respond",
            "description": "Say a reply.",
            "target": "communication",
            "schema": {
                "description": "Say a reply.",
                "type": "object",
                "properties": {
                    "parts": {"type": "array", "description": "What to say.",
                              "items": {"type": "object", "properties": {"words": {"type": "string"}}}},
                    "title": {"type": "string", "default": {"title": "kept"}},
                },
                "$defs": {"examples": {"enum": [{"title": "kept"}]}},
                "anyOf": [{"required": ["title"]}],
                "required": ["parts"],
                "additionalProperties": false,
            },
        }])
    );
    assert_eq!(assigned(&host, "over"), Data::Bool(false));
    // Overlocation is written before location.
    assert_eq!(host.assigned[0].0, "over");
}

#[test]
fn model_facing_schema_writes_canonical_json_whose_length_is_what_the_bound_measures() {
    let mut host = holding(&verbose_offers());
    model_facing_with(&[location("offers")], &mut host).expect("compacts");
    let Data::JsonArray(text) = assigned(&host, "offers") else {
        panic!("a JSON array");
    };
    let (expected, over) = model_facing_choices(&verbose_offers(), DEFAULT_BOUND).unwrap();
    assert_eq!((text.as_str(), over), (expected.as_str(), false));
    assert_eq!(text, canonical_json(&serde_json::from_str(&text).unwrap()));
    // The bound is inclusive: exactly the compacted length keeps descriptions.
    let (exact, over) = model_facing_choices(&verbose_offers(), text.len()).unwrap();
    assert_eq!((exact, over), (text, false));
}

#[test]
fn model_facing_schema_over_the_bound_drops_descriptions_before_it_drops_the_choices() {
    let (full, _) = model_facing_choices(&verbose_offers(), usize::MAX).unwrap();
    let (bare, over) = model_facing_choices(&verbose_offers(), full.len() - 1).unwrap();
    assert!(!over);
    assert!(!bare.contains("description"), "{bare}");
    let bare: Json = serde_json::from_str(&bare).unwrap();
    assert_eq!(bare[0]["target"], json!("communication"));
    assert_eq!(
        bare[0]["schema"]["properties"]["parts"],
        json!({"type": "array", "items": {"type": "object", "properties": {"words": {"type": "string"}}}})
    );
    // A property named description is a name, not a keyword: it stays.
    let named = json!([{"event": "e", "schema": {"properties": {"description": {"type": "string", "description": "x"}}}}]);
    let (named_full, _) = model_facing_choices(&named, usize::MAX).unwrap();
    let (bare, over) = model_facing_choices(&named, named_full.len() - 1).unwrap();
    assert!(!over);
    assert_eq!(
        serde_json::from_str::<Json>(&bare).unwrap(),
        json!([{"event": "e", "schema": {"properties": {"description": {"type": "string"}}}}])
    );
}

#[test]
fn model_facing_schema_still_over_the_bound_assigns_no_choices_and_sets_overlocation() {
    let mut host = holding(&verbose_offers());
    model_facing_with(
        &[
            location("offers"),
            attr("bound", "40"),
            attr("overlocation", "over"),
        ],
        &mut host,
    )
    .expect("compacts");
    assert_eq!(assigned(&host, "offers"), Data::JsonArray("[]".into()));
    assert_eq!(assigned(&host, "over"), Data::Bool(true));
    // Without overlocation only the choices are assigned.
    let mut host = holding(&verbose_offers());
    model_facing_with(&[location("offers"), attr("bound", "40")], &mut host).expect("compacts");
    assert_eq!(
        host.assigned,
        vec![("offers".into(), Data::JsonArray("[]".into()))]
    );
    // An empty list is never over.
    assert_eq!(
        model_facing_choices(&json!([]), 2),
        Ok(("[]".into(), false))
    );
}

#[test]
fn model_facing_schema_is_idempotent() {
    for bound in [usize::MAX, 400, 10] {
        let (once, over) = model_facing_choices(&verbose_offers(), bound).unwrap();
        let (twice, again) =
            model_facing_choices(&serde_json::from_str(&once).unwrap(), bound).unwrap();
        assert_eq!(twice, once, "bound {bound}");
        assert_eq!(again, over && once != "[]", "bound {bound}");
    }
}

#[test]
fn malformed_model_facing_schema_elements_or_choices_raise_error_execution() {
    let offers = verbose_offers();
    let cases: Vec<(Vec<Attribute>, Option<Json>, &str)> = vec![
        (
            vec![],
            Some(offers.clone()),
            "processing:model_facing_schema requires a non-empty location",
        ),
        (
            vec![location(" ")],
            Some(offers.clone()),
            "processing:model_facing_schema requires a non-empty location",
        ),
        (
            vec![location("offers"), location("offers")],
            Some(offers.clone()),
            "processing:model_facing_schema repeats an attribute",
        ),
        (
            vec![location("offers"), attr("events", "e")],
            Some(offers.clone()),
            "processing:model_facing_schema takes only the location, bound and overlocation attributes",
        ),
        (
            vec![location("offers"), attr("overlocation", "")],
            Some(offers.clone()),
            "processing:model_facing_schema overlocation is empty",
        ),
        (
            vec![location("offers"), attr("bound", "-1")],
            Some(offers.clone()),
            "processing:model_facing_schema bound is not a decimal integer",
        ),
        (
            vec![location("offers"), attr("bound", "1e3")],
            Some(offers.clone()),
            "processing:model_facing_schema bound is not a decimal integer",
        ),
        (
            vec![location("offers"), attr("bound", "99999999999999999999999")],
            Some(offers.clone()),
            "processing:model_facing_schema bound is not a decimal integer",
        ),
        (
            vec![location("offers")],
            None,
            "processing:model_facing_schema location does not hold choices",
        ),
        (
            vec![location("offers")],
            Some(json!([1])),
            "processing:model_facing_schema found a choice that is not an object",
        ),
        (
            vec![location("offers")],
            Some(json!([{"schema": {}}])),
            "processing:model_facing_schema found a malformed choice",
        ),
        (
            vec![location("offers")],
            Some(json!([{"event": "", "schema": {}}])),
            "processing:model_facing_schema found a malformed choice",
        ),
        (
            vec![location("offers")],
            Some(json!([{"event": "e"}])),
            "processing:model_facing_schema found a malformed choice",
        ),
        (
            vec![location("offers")],
            Some(json!([{"event": "e", "schema": {}, "target": ""}])),
            "processing:model_facing_schema found a malformed choice",
        ),
        (
            vec![location("offers")],
            Some(json!([{"event": "e", "schema": {}, "description": 1}])),
            "processing:model_facing_schema found a malformed choice",
        ),
        (
            vec![location("offers")],
            Some(json!([{"event": "e", "schema": {}, "extra": 1}])),
            "processing:model_facing_schema found a malformed choice",
        ),
    ];
    for (attributes, value, detail) in cases {
        let mut host = value.map_or_else(FakeHost::default, |value| holding(&value));
        assert_eq!(
            execution(model_facing_with(&attributes, &mut host)),
            detail,
            "{attributes:?}"
        );
        assert!(host.assigned.is_empty(), "{attributes:?}");
    }
}

#[test]
fn model_facing_schema_reads_only_arrays_takes_no_content_and_fails_on_assignment() {
    let offers = verbose_offers();
    // A location holding an object, not an array.
    let mut host = FakeHost::default();
    host.values
        .insert("offers".into(), Data::JsonObject("{}".into()));
    assert_eq!(
        execution(model_facing_with(&[location("offers")], &mut host)),
        "processing:model_facing_schema location does not hold choices"
    );
    // Content is refused.
    let mut host = holding(&offers);
    assert_eq!(
        execution(execute(
            "model_facing_schema",
            &[location("offers")],
            &[Content::Start, Content::End],
            &mut host
        )),
        "processing:model_facing_schema holds no content"
    );
    // A failed assignment is error.execution; a host failure passes through.
    let mut host = holding(&offers);
    host.fail_assign = true;
    assert_eq!(
        execution(model_facing_with(&[location("offers")], &mut host)),
        "processing:model_facing_schema could not assign the choices"
    );
    let mut host = holding(&offers);
    host.fail_assign = true;
    assert_eq!(
        execution(model_facing_with(
            &[location("offers"), attr("overlocation", "over")],
            &mut host
        )),
        "processing:model_facing_schema could not assign overlocation"
    );
    assert_eq!(
        model_facing_with(&[location("fatal")], &mut FakeHost::default()),
        Err(Failure::Host("fatal"))
    );
}

/// `size` as the data-model number the elements assign.
fn number(size: usize) -> Data {
    let size = u32::try_from(size).expect("a small size");
    Data::Number(scxml_types::FiniteNumber::new(f64::from(size)).expect("finite"))
}

/// A host whose locations hold `values` (each canonical JSON).
fn with_values(values: &[(&str, Json)]) -> FakeHost {
    let mut host = FakeHost::default();
    for (location, value) in values {
        let text = canonical_json(value);
        let data = if value.is_object() {
            Data::JsonObject(text)
        } else {
            Data::JsonArray(text)
        };
        host.values.insert((*location).into(), data);
    }
    host
}

/// The canonical JSON (array or object) last assigned to `location`.
fn assigned_any(host: &FakeHost, location: &str) -> Json {
    match assigned(host, location) {
        Data::JsonArray(text) | Data::JsonObject(text) => {
            serde_json::from_str(&text).expect("canonical JSON")
        }
        other => panic!("expected JSON, got {other:?}"),
    }
}
fn deactivate_choice() -> Json {
    json!({"event": "bot.deactivate", "schema": {"type": "object"}})
}
fn respond_offer() -> Json {
    json!({
        "event": "bot.ability.communication.respond",
        "description": "Say a reply.",
        "schema": {"title": "respond", "type": "object", "description": "Say a reply.",
                   "properties": {"parts": {"type": "array", "examples": [[{"words": "hi"}]]}}},
    })
}

fn ignore_offer() -> Json {
    json!({"event": "bot.ability.cognition.ignore", "schema": {"type": "object"}})
}

fn collect_attributes(bound: &str) -> Vec<Attribute> {
    vec![
        location("choices"),
        attr("choices", "own"),
        attr("offers", "offers"),
        attr("bound", bound),
        attr("overlocation", "over"),
        attr("sizelocation", "size"),
    ]
}

fn collecting() -> FakeHost {
    with_values(&[
        ("own", json!([deactivate_choice()])),
        (
            "offers",
            json!({
                "communication": [respond_offer(), ignore_offer()],
                "cognition": [ignore_offer()],
            }),
        ),
    ])
}

#[test]
fn collect_offered_events_tags_each_choice_with_its_offering_session_in_name_order() {
    let mut host = collecting();
    assert_eq!(
        execute(
            "collect_offered_events",
            &collect_attributes("32768"),
            &[],
            &mut host
        ),
        Ok(Executed::Handled)
    );
    let choices = json!([
        {"event": "bot.deactivate", "schema": {"type": "object"}, "target": "session.floppy"},
        {"event": "bot.ability.cognition.ignore", "schema": {"type": "object"}, "target": "session.floppy.cognition"},
        {"event": "bot.ability.communication.respond", "description": "Say a reply.", "target": "session.floppy.communication",
         "schema": {"type": "object", "description": "Say a reply.", "properties": {"parts": {"type": "array"}}}},
        {"event": "bot.ability.cognition.ignore", "schema": {"type": "object"}, "target": "session.floppy.communication"},
    ]);
    assert_eq!(assigned_any(&host, "choices"), choices);
    assert_eq!(assigned(&host, "over"), Data::Bool(false));
    let size = canonical_json(&choices).len();
    assert_eq!(assigned(&host, "size"), number(size));
    // The flags are assigned before the choices.
    let order: Vec<&str> = host.assigned.iter().map(|(at, _)| at.as_str()).collect();
    assert_eq!(order, ["over", "size", "choices"]);
}

#[test]
fn collect_offered_events_over_the_bound_drops_descriptions_then_keeps_the_list_and_sets_over() {
    let full = {
        let mut host = collecting();
        execute(
            "collect_offered_events",
            &collect_attributes("32768"),
            &[],
            &mut host,
        )
        .expect("assembled");
        canonical_json(&assigned_any(&host, "choices")).len()
    };
    // One byte short of the full list: descriptions go, and that fits.
    let mut host = collecting();
    execute(
        "collect_offered_events",
        &collect_attributes(&(full - 1).to_string()),
        &[],
        &mut host,
    )
    .expect("assembled");
    let choices = assigned_any(&host, "choices");
    assert!(!canonical_json(&choices).contains("description"));
    assert_eq!(assigned(&host, "over"), Data::Bool(false));
    // Under even that: the description-less list is kept, over is set, and
    // size is that list's size.
    let mut host = collecting();
    execute(
        "collect_offered_events",
        &collect_attributes("10"),
        &[],
        &mut host,
    )
    .expect("assembled");
    assert_eq!(assigned_any(&host, "choices"), choices);
    assert_eq!(assigned(&host, "over"), Data::Bool(true));
    assert_eq!(
        assigned(&host, "size"),
        number(canonical_json(&choices).len())
    );
}

#[test]
fn malformed_collect_offered_events_raise_error_execution_and_assign_nothing() {
    let cases: Vec<(Vec<Attribute>, Json, Json, &str)> = vec![
        (
            vec![location("choices")],
            json!([]),
            json!({}),
            "processing:collect_offered_events requires a non-empty choices",
        ),
        (
            [collect_attributes("1"), vec![attr("events", "x")]].concat(),
            json!([]),
            json!({}),
            "processing:collect_offered_events takes only the location, choices, offers, devices, focus, bound, overlocation and sizelocation attributes",
        ),
        (
            collect_attributes("-1"),
            json!([]),
            json!({}),
            "processing:collect_offered_events bound is not a decimal integer",
        ),
        (
            collect_attributes("1"),
            json!({}),
            json!({}),
            "processing:collect_offered_events choices does not hold choices",
        ),
        (
            collect_attributes("1"),
            json!([]),
            json!([]),
            "processing:collect_offered_events offers does not hold offers",
        ),
        (
            collect_attributes("1"),
            json!([]),
            json!({"cognition": {}}),
            "processing:collect_offered_events offers does not hold offers",
        ),
        (
            collect_attributes("1"),
            json!([]),
            json!({"cognition": [{"event": "x", "schema": {}, "extra": 1}]}),
            "processing:collect_offered_events found a malformed choice",
        ),
        (
            collect_attributes("1"),
            json!([]),
            json!({"": []}),
            "processing:collect_offered_events offers names an empty invoke id",
        ),
    ];
    for (attributes, own, offers, detail) in cases {
        let mut host = with_values(&[("own", own), ("offers", offers)]);
        assert_eq!(
            execution(execute(
                "collect_offered_events",
                &attributes,
                &[],
                &mut host
            )),
            detail
        );
        assert!(host.assigned.is_empty(), "{detail}");
    }
}
#[test]
fn send_builds_the_dispatch_tool_schema_of_the_choices() {
    let mut host = with_values(&[(
        "choices",
        json!([
            {"event": "bot.deactivate", "schema": {"$schema": "x", "title": "Deactivate", "description": "Deactivate the bot.", "examples": [{"why": "done"}], "type": "object", "properties": {"why": {"type": "string"}}}},
            {"event": "bot.ability.communication.respond", "description": "Say a reply.", "target": "communication",
             "schema": {"type": "object", "additionalProperties": true, "properties": {"parts": {"type": "array"}}, "required": ["parts"]}},
            {"event": "bot.ability.cognition.ignore", "target": "cognition", "schema": "not an object"},
            {"event": "bot.ability.cognition.ignore", "target": "communication", "description": "later", "schema": {}},
            {"event": "bot.ability.cognition.ignore", "target": "cognition", "schema": {}},
            {"event": "bot.count", "schema": {"type": "integer"}},
        ]),
    )]);
    assert_eq!(
        execute(
            "send",
            &[location("schema"), attr("choices", "choices")],
            &[],
            &mut host
        ),
        Ok(Executed::Handled)
    );
    let Data::JsonObject(text) = assigned(&host, "schema") else {
        panic!("expected a JSON object");
    };
    let reason =
        json!({"type": "string", "description": "Optional short reason this event was selected."});
    let deactivate_example = json!({"event": "bot.deactivate", "data": {"why": "done"}});
    let expected = json!({
        "description": "Dispatch all events necessary for the input. Call once. Multi-select by listing every offered event this turn requires in the events array (for example a device action and speech together when both are needed). Each item must match one offered event branch; payload fields and requirements are defined on that event's data schema. An empty events list leaves the turn unhandled for the host cascade (e.g. deliberative reasoning); use an explicit ignore/pass event when the stage should handle the turn with no environment actions.",
        "type": "object",
        "properties": {"events": {
            "type": "array",
            "description": "All events necessary for the input this turn. Each item is one offered event branch (enum-pinned name + that event's data schema). Include every action required together; empty array selects none.",
            "items": {"anyOf": [
                {
                    "type": "object",
                    "description": "Deactivate the bot.",
                    "properties": {
                        "event": {"type": "string", "enum": ["bot.deactivate"]},
                        "data": {"title": "Deactivate", "description": "Deactivate the bot.", "examples": [{"why": "done"}], "type": "object", "properties": {"why": {"type": "string"}}, "additionalProperties": false},
                        "reason": reason,
                    },
                    "required": ["event"],
                    "additionalProperties": false,
                    "examples": [deactivate_example],
                },
                {
                    "type": "object",
                    "description": "bot.ability.communication.respond",
                    "properties": {
                        "event": {"type": "string", "enum": ["bot.ability.communication.respond"]},
                        "data": {"type": "object", "additionalProperties": true, "properties": {"parts": {"type": "array"}}, "required": ["parts"]},
                        "reason": reason,
                        "target": {"type": "string", "enum": ["communication"], "description": "Actor that receives this event. Fixed for this turn because only one actor enables it in the live topology."},
                    },
                    "required": ["event", "data", "target"],
                    "additionalProperties": false,
                },
                {
                    "type": "object",
                    "description": "bot.ability.cognition.ignore",
                    "properties": {
                        "event": {"type": "string", "enum": ["bot.ability.cognition.ignore"]},
                        "data": {"type": "object", "properties": {}, "additionalProperties": false},
                        "reason": reason,
                        "target": {"type": "string", "enum": ["cognition", "communication"], "description": "Actor that should receive the event. Required because multiple actors enable this event this turn; choose exactly one offered actor key."},
                    },
                    "required": ["event", "target"],
                    "additionalProperties": false,
                },
                {
                    "type": "object",
                    "description": "bot.count",
                    "properties": {
                        "event": {"type": "string", "enum": ["bot.count"]},
                        "data": {"type": "object", "properties": {"value": {"type": "integer"}}, "required": ["value"], "additionalProperties": false},
                        "reason": reason,
                    },
                    "required": ["event", "data"],
                    "additionalProperties": false,
                },
            ]},
            "examples": [[], deactivate_example],
        }},
        "required": ["events"],
        "additionalProperties": false,
    });
    assert_eq!(text, canonical_json(&expected));
}

#[test]
fn send_without_choices_offers_any_event() {
    let mut host = with_values(&[("choices", json!([]))]);
    execute(
        "send",
        &[location("schema"), attr("choices", "choices")],
        &[],
        &mut host,
    )
    .expect("built");
    let events = &assigned_any(&host, "schema")["properties"]["events"];
    assert_eq!(
        events["items"],
        json!({"type": "object", "properties": {"event": {"type": "string"}, "data": {"type": "object", "additionalProperties": false}}, "required": ["event"], "additionalProperties": false})
    );
    assert_eq!(events["examples"], json!([[]]));
}

#[test]
fn malformed_sends_raise_error_execution_and_assign_nothing() {
    let attributes = [location("schema"), attr("choices", "choices")];
    let cases: Vec<(&[Attribute], Json, &str)> = vec![
        (
            &attributes[..1],
            json!([]),
            "processing:send requires a non-empty choices",
        ),
        (
            &attributes,
            json!({}),
            "processing:send found a malformed choice",
        ),
        (
            &attributes,
            json!([1]),
            "processing:send found a choice that is not an object",
        ),
        (
            &attributes,
            json!([{"event": "x"}]),
            "processing:send found a malformed choice",
        ),
    ];
    for (attributes, choices, detail) in cases {
        let mut host = with_values(&[("choices", choices)]);
        let got = execution(execute("send", attributes, &[], &mut host));
        assert!(
            got == detail || got == "processing:send choices does not hold choices",
            "{detail}: {got}"
        );
        assert!(host.assigned.is_empty(), "{detail}");
    }
    assert_eq!(
        execution(execute(
            "send",
            &[
                location("schema"),
                attr("choices", "choices"),
                attr("bound", "1")
            ],
            &[],
            &mut FakeHost::default()
        )),
        "processing:send takes only the location, choices and max attributes"
    );
    assert_eq!(
        execution(execute(
            "send",
            &attributes,
            &[Content::Start, Content::End],
            &mut FakeHost::default()
        )),
        "processing:send holds no content"
    );
    assert_eq!(
        execute(
            "send",
            &[location("schema"), attr("choices", "fatal")],
            &[],
            &mut FakeHost::default()
        ),
        Err(Failure::Host("fatal"))
    );
}

#[test]
fn collect_offered_events_without_offers_tags_only_the_documents_own() {
    let mut host = with_values(&[("own", json!([deactivate_choice()]))]);
    execute(
        "collect_offered_events",
        &[location("choices"), attr("choices", "own")],
        &[],
        &mut host,
    )
    .expect("collected");
    assert_eq!(
        assigned_any(&host, "choices"),
        json!([{"event": "bot.deactivate", "schema": {"type": "object"}, "target": "session.floppy"}])
    );
    // Only the choices are assigned without overlocation and sizelocation.
    assert_eq!(host.assigned.len(), 1);
}

fn lamp() -> Json {
    json!({"id": "session.lamp", "state": "off", "attributes": {}, "events": ["lamp.on", "lamp.off"]})
}

fn with_devices(focus: Data) -> FakeHost {
    let mut host = collecting();
    host.values.insert(
        "devices".into(),
        Data::JsonObject(canonical_json(&json!({"lamp": lamp()}))),
    );
    host.values.insert("focus".into(), focus);
    host
}

fn device_attributes() -> Vec<Attribute> {
    vec![
        location("choices"),
        attr("choices", "own"),
        attr("offers", "offers"),
        attr("devices", "devices"),
        attr("focus", "focus"),
    ]
}

#[test]
fn collect_offered_events_offers_the_focused_devices_events_under_its_id_by_its_reference() {
    let mut host = with_devices(Data::String("lamp".into()));
    execute(
        "collect_offered_events",
        &device_attributes(),
        &[],
        &mut host,
    )
    .expect("collected");
    let targets: Vec<String> = assigned_any(&host, "choices")
        .as_array()
        .expect("an array")
        .iter()
        .map(|choice| {
            format!(
                "{}@{}",
                choice["event"].as_str().unwrap(),
                choice["target"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(
        targets,
        [
            "bot.deactivate@session.floppy",
            "bot.ability.cognition.ignore@session.floppy.cognition",
            "bot.ability.communication.respond@session.floppy.communication",
            "bot.ability.cognition.ignore@session.floppy.communication",
            "lamp.on@session.lamp",
            "lamp.off@session.lamp",
        ]
    );
    assert_eq!(
        assigned_any(&host, "choices")[4]["schema"],
        json!({"type": "object"})
    );
    // No focus, or a focus no read device has, offers no device.
    for focus in [Data::Null, Data::String("fan".into())] {
        let mut host = with_devices(focus);
        execute(
            "collect_offered_events",
            &device_attributes(),
            &[],
            &mut host,
        )
        .expect("collected");
        assert_eq!(assigned_any(&host, "choices").as_array().unwrap().len(), 4);
    }
}

#[test]
fn malformed_device_offers_raise_error_execution_and_assign_nothing() {
    let cases: Vec<(Vec<Attribute>, Data, Json, &str)> = vec![
        (
            device_attributes()[..4].to_vec(),
            Data::Null,
            json!({}),
            "processing:collect_offered_events takes devices and focus together",
        ),
        (
            device_attributes(),
            Data::Bool(true),
            json!({}),
            "processing:collect_offered_events focus is neither a string nor null",
        ),
        (
            device_attributes(),
            Data::String("lamp".into()),
            json!({"lamp": {"events": []}}),
            "processing:collect_offered_events devices does not hold devices",
        ),
        (
            device_attributes(),
            Data::String("lamp".into()),
            json!({"lamp": {"id": "session.lamp", "events": [1]}}),
            "processing:collect_offered_events devices does not hold devices",
        ),
    ];
    for (attributes, focus, devices, detail) in cases {
        let mut host = with_devices(focus);
        host.values
            .insert("devices".into(), Data::JsonObject(canonical_json(&devices)));
        assert_eq!(
            execution(execute(
                "collect_offered_events",
                &attributes,
                &[],
                &mut host
            )),
            detail
        );
        assert!(host.assigned.is_empty(), "{detail}");
    }
    // Without a session id there is nothing to tag the document's own with.
    let mut host = collecting();
    host.values.remove("_sessionid");
    assert_eq!(
        execution(execute(
            "collect_offered_events",
            &collect_attributes("32768"),
            &[],
            &mut host
        )),
        "processing:collect_offered_events found no _sessionid"
    );
    assert!(host.assigned.is_empty());
}

#[test]
fn send_with_max_bounds_the_events_array() {
    let mut host = with_values(&[("choices", json!([deactivate_choice()]))]);
    execute(
        "send",
        &[
            location("schema"),
            attr("choices", "choices"),
            attr("max", "1"),
        ],
        &[],
        &mut host,
    )
    .expect("built");
    let events = &assigned_any(&host, "schema")["properties"]["events"];
    assert_eq!(events["maxItems"], json!(1));
    // Without max the array is unbounded.
    let mut host = with_values(&[("choices", json!([deactivate_choice()]))]);
    execute(
        "send",
        &[location("schema"), attr("choices", "choices")],
        &[],
        &mut host,
    )
    .expect("built");
    assert!(
        assigned_any(&host, "schema")["properties"]["events"]
            .get("maxItems")
            .is_none()
    );
    for max in ["", "-1", "one"] {
        let mut host = with_values(&[("choices", json!([]))]);
        let got = execution(execute(
            "send",
            &[
                location("schema"),
                attr("choices", "choices"),
                attr("max", max),
            ],
            &[],
            &mut host,
        ));
        assert_eq!(
            got, "processing:send max is not a decimal integer",
            "{max:?}"
        );
    }
}

#[test]
fn normalize_confidence_scales_a_probability_and_clamps_to_the_scale() {
    // A probability in (0, 1] is scaled, rounded half to even.
    assert_eq!(normalize_confidence(0.875), 88);
    assert_eq!(normalize_confidence(0.125), 12);
    assert_eq!(normalize_confidence(0.5), 50);
    assert_eq!(normalize_confidence(1.0), 100);
    assert_eq!(normalize_confidence(0.004), 0);
    // Anything else is on the 0-100 scale already, clamped.
    assert_eq!(normalize_confidence(0.0), 0);
    assert_eq!(normalize_confidence(42.5), 42);
    assert_eq!(normalize_confidence(43.5), 44);
    assert_eq!(normalize_confidence(250.0), 100);
    assert_eq!(normalize_confidence(-3.0), 0);
}

fn confidence_of(selections: Json) -> Data {
    let mut host = with_values(&[("selections", selections)]);
    execute(
        "selection_confidence",
        &[location("confidence"), attr("selections", "selections")],
        &[],
        &mut host,
    )
    .expect("assigned");
    assigned(&host, "confidence")
}

#[test]
fn selection_confidence_is_the_least_normalized_confidence() {
    assert_eq!(
        confidence_of(json!([
            {"event": "a", "confidence": 0.91},
            {"event": "b", "confidence": 0.425},
            {"event": "c"},
        ])),
        number(42)
    );
    // With no number among them there is no confidence.
    assert_eq!(confidence_of(json!([{"event": "a"}])), Data::Null);
    assert_eq!(confidence_of(json!([])), Data::Null);
}

#[test]
fn malformed_selection_confidences_raise_error_execution_and_assign_nothing() {
    let attributes = [location("confidence"), attr("selections", "selections")];
    let cases: Vec<(&[Attribute], Option<Json>, &str)> = vec![
        (
            &attributes[..1],
            Some(json!([])),
            "processing:selection_confidence requires a non-empty selections",
        ),
        (
            &attributes[1..],
            Some(json!([])),
            "processing:selection_confidence requires a non-empty location",
        ),
        (
            &attributes,
            None,
            "processing:selection_confidence selections is not an array of selections",
        ),
        (
            &attributes,
            Some(json!({})),
            "processing:selection_confidence selections is not an array of selections",
        ),
        (
            &attributes,
            Some(json!([1])),
            "processing:selection_confidence selections is not an array of selections",
        ),
    ];
    for (attributes, selections, detail) in cases {
        let mut host = selections.map_or_else(FakeHost::default, |selections| {
            with_values(&[("selections", selections)])
        });
        assert_eq!(
            execution(execute("selection_confidence", attributes, &[], &mut host)),
            detail
        );
        assert!(host.assigned.is_empty(), "{detail}");
    }
    assert_eq!(
        execution(execute(
            "selection_confidence",
            &[location("c"), attr("selections", "s"), attr("max", "1")],
            &[],
            &mut FakeHost::default()
        )),
        "processing:selection_confidence takes only the location and selections attributes"
    );
    let mut host = with_values(&[("selections", json!([]))]);
    host.fail_assign = true;
    assert_eq!(
        execution(execute("selection_confidence", &attributes, &[], &mut host)),
        "processing:selection_confidence could not assign the confidence"
    );
    assert_eq!(
        execute(
            "selection_confidence",
            &[location("c"), attr("selections", "fatal")],
            &[],
            &mut FakeHost::default()
        ),
        Err(Failure::Host("fatal"))
    );
}

#[test]
fn an_element_the_namespace_does_not_declare_is_not_handled() {
    assert_eq!(
        execute("route", &[], &[], &mut FakeHost::default()),
        Ok(Executed::NotHandled)
    );
}
