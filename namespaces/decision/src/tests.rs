use std::collections::BTreeMap;

use serde_json::json;

use super::*;

/// A deterministic fake host: guards evaluate from a table (anything else
/// is not a boolean), the snapshot is scripted, and assignments are
/// recorded.
#[derive(Default)]
struct FakeHost {
    guards: BTreeMap<String, bool>,
    choices_snapshot: Option<String>,
    snapshot_requests: Vec<Vec<Section>>,
    assigned: Vec<(String, Data)>,
    fail_assign: bool,
    values: BTreeMap<String, Data>,
}

impl ChoiceHost for FakeHost {
    type Error = &'static str;

    fn guard(&mut self, cond: &str) -> Result<GuardValue, &'static str> {
        if cond == "fatal" {
            return Err("fatal");
        }
        Ok(self.guards.get(cond).copied())
    }

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

    fn snapshot(&mut self, exclude: &[Section]) -> Result<Result<String, ()>, &'static str> {
        self.snapshot_requests.push(exclude.to_vec());
        Ok(self.choices_snapshot.clone().ok_or(()))
    }
}

/// One snapshot transition listing with an `event:schema`.
fn with_schema(events: &[&str], schema: &Json) -> Json {
    let text = canonical_json(schema);
    json!({
        "source": "s", "index": 0, "events": events, "targets": [], "type": "external",
        "has_cond": false,
        "event_schema": {"kind": "schema", "schema": text, "sha256": sha256_hex(text.as_bytes())},
    })
}

/// One snapshot transition listing without a schema.
fn without_schema(events: &[&str]) -> Json {
    json!({"source": "s", "index": 1, "events": events, "targets": [], "type": "external", "has_cond": false})
}

/// A snapshot whose one active atomic state is `s`, the source of every
/// transition the listing helpers build.
fn snapshot(transitions: &[Json]) -> String {
    snapshot_in(&["s"], transitions)
}

/// A snapshot whose active atomic states are `atomic`.
fn snapshot_in(atomic: &[&str], transitions: &[Json]) -> String {
    json!({
        "version": "scxml.session-snapshot/v2",
        "snapshot": {"root": {
            "configuration": {"atomic": atomic, "full": atomic},
            "transitions": transitions,
        }},
    })
    .to_string()
}

/// `transition` listed from `source`.
fn from(source: &str, mut transition: Json) -> Json {
    transition["source"] = json!(source);
    transition
}

fn ship_schema() -> Json {
    json!({
        "description": "Ship the order.\nMore text.",
        "type": "object",
        "properties": {"sku": {"type": "string"}, "carrier": {"enum": ["ups", "fedex"]}},
        "required": ["sku", "carrier"],
    })
}

fn cancel_schema() -> Json {
    json!({
        "title": "Cancel the order",
        "type": "object",
        "properties": {"reason": {"enum": ["customer", "stock"]}},
        "required": ["reason"],
        "additionalProperties": false,
    })
}

fn hold_schema() -> Json {
    json!({"type": "object"})
}

/// The order machine's choices: three schema transitions, an error handler
/// without a schema, and a wildcard listed after them.
fn order_snapshot() -> String {
    snapshot(&[
        with_schema(&["order.ship"], &ship_schema()),
        with_schema(&["order.cancel"], &cancel_schema()),
        with_schema(&["order.hold.*"], &hold_schema()),
        without_schema(&["error.execution"]),
        without_schema(&["*"]),
    ])
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
        "../package/decision.extension.json"
    ))
    .expect("the manifest is a valid extension package manifest");
    assert_eq!(manifest.namespace_uri, NAMESPACE_URI);
    assert_eq!(
        manifest.extension_artifact,
        "floppy_decision.virtualized.wasm"
    );
    // Executable content is synchronous and does no I/O: the manifest
    // declares no origin and no slot (the decision service returns the
    // chosen event).
    manifest
        .verify_schema(&scxml_validator_core::SchemaSource::new(
            "decision.xsd",
            include_bytes!("../package/decision.xsd").to_vec(),
        ))
        .expect("the schema digest, targetNamespace, and WIT identity verify");
}

fn location(value: &str) -> Attribute {
    attr("location", value)
}

#[test]
fn choices_writes_the_canonical_choices_of_the_enabled_schema_transitions() {
    let mut host = FakeHost {
        choices_snapshot: Some(order_snapshot()),
        ..FakeHost::default()
    };
    assert_eq!(
        execute("choice", &[location("choices")], &[], &mut host),
        Ok(Executed::Handled)
    );
    assert_eq!(host.snapshot_requests, [CHOICES_SNAPSHOT_EXCLUDE.to_vec()]);
    let expected = canonical_json(&json!([
        {"event": "order.ship", "schema": ship_schema(), "description": "Ship the order."},
        {"event": "order.cancel", "schema": cancel_schema(), "description": "Cancel the order"},
        {"event": "order.hold", "schema": hold_schema()},
    ]));
    assert_eq!(
        host.assigned,
        [("choices".to_owned(), Data::JsonArray(expected))]
    );
}

#[test]
fn choices_leaves_out_a_transition_whose_guard_is_false() {
    let mut snapshot: Json =
        serde_json::from_str(&order_snapshot()).expect("the order snapshot is JSON");
    for transition in snapshot["snapshot"]["root"]["transitions"]
        .as_array_mut()
        .expect("transitions")
    {
        if transition["events"] != json!(["order.ship"]) {
            transition["has_cond"] = json!(true);
            transition["cond"] = json!("closed");
        }
    }
    let mut host = FakeHost {
        choices_snapshot: Some(snapshot.to_string()),
        guards: BTreeMap::from([("closed".to_owned(), false)]),
        ..FakeHost::default()
    };
    execute("choice", &[location("choices")], &[], &mut host).expect("handled");
    let Data::JsonArray(choices) = &host.assigned[0].1 else {
        panic!("the choices are an array");
    };
    let choices: Json = serde_json::from_str(choices).expect("JSON");
    assert_eq!(choices.as_array().map(Vec::len), Some(1));
    assert_eq!(choices[0]["event"], "order.ship");
}

#[test]
fn malformed_choices_elements_or_snapshots_raise_error_execution() {
    let order = || FakeHost {
        choices_snapshot: Some(order_snapshot()),
        ..FakeHost::default()
    };
    for (attributes, content) in [
        (vec![], vec![]),
        (vec![location(" ")], vec![]),
        (vec![location("choices"), attr("model", "m")], vec![]),
        (vec![location("choices"), location("other")], vec![]),
        (
            vec![Attribute {
                namespace_uri: Some(scxml_types::ENV_NAMESPACE_URI.into()),
                local_name: "apikey".into(),
                value: "K".into(),
            }],
            vec![],
        ),
        (vec![location("choices")], vec![Content::Text("x".into())]),
        (
            vec![location("choices")],
            vec![Content::Start, Content::End],
        ),
    ] {
        let mut host = order();
        execution(execute("choice", &attributes, &content, &mut host));
        assert!(host.assigned.is_empty());
    }
    let mut host = FakeHost::default();
    assert_eq!(
        execution(execute("choice", &[location("choices")], &[], &mut host)),
        "the host offers no session snapshot"
    );
    let mut host = FakeHost {
        fail_assign: true,
        ..order()
    };
    execution(execute("choice", &[location("choices")], &[], &mut host));
    let mut host = FakeHost {
        choices_snapshot: Some(snapshot(&[guarded(
            with_schema(&["x"], &json!({"type": "object"})),
            "fatal",
        )])),
        ..FakeHost::default()
    };
    assert_eq!(
        execute("choice", &[location("choices")], &[], &mut host),
        Err(Failure::Host("fatal"))
    );
    let mut host = FakeHost::default();
    assert_eq!(
        execute("choose", &[], &[], &mut host),
        Ok(Executed::NotHandled),
        "the old elements are gone"
    );
}

/// The choices of `transitions` with every guard answering by its text:
/// `"yes"` true, `"no"` false, anything else not a boolean.
fn choices_events(transitions: &[Json]) -> Result<Vec<String>, Failure<()>> {
    snapshot_choices(&snapshot(transitions), &mut |cond| {
        Ok(match cond {
            "yes" => Some(true),
            "no" => Some(false),
            _ => None,
        })
    })
    .map(|choices| {
        choices
            .into_iter()
            .map(|candidate| candidate.event)
            .collect()
    })
}

/// `transition` with the guard `cond`.
fn guarded(mut transition: Json, cond: &str) -> Json {
    transition["has_cond"] = json!(true);
    transition["cond"] = json!(cond);
    transition
}

#[test]
fn the_snapshot_choices_follows_selection_order_and_skips_what_cannot_be_offered() {
    let schema = json!({"type": "object"});
    let events = choices_events(&[
        without_schema(&["order"]),
        // Preempted by the earlier `order` descriptor.
        with_schema(&["order.ship"], &schema),
        with_schema(&["a.*", "b"], &schema),
        // Duplicate of `b`; `c` is new.
        with_schema(&["b", "c"], &schema),
        json!({"source": "s", "index": 2, "events": ["d"], "targets": [], "type": "external",
               "has_cond": false, "event_schema": {"kind": "schemaexpr", "expr": "schemas.d"}}),
        // A wildcard matches every later event.
        with_schema(&["*"], &schema),
        with_schema(&["e"], &schema),
    ])
    .expect("the snapshot is well formed");
    assert_eq!(events, ["a", "b", "c"]);

    let mut tampered: Json =
        serde_json::from_str(&snapshot(&[with_schema(&["x"], &schema)])).expect("JSON");
    tampered["snapshot"]["root"]["transitions"][0]["event_schema"]["sha256"] = json!("00");
    assert_eq!(
        snapshot_choices(&tampered.to_string(), &mut |_| Ok::<_, ()>(Some(true)))
            .map(|choices| choices.len()),
        Err(Failure::Execution("the session snapshot is malformed"))
    );
    assert_eq!(
        snapshot_choices(r#"{"snapshot":{"root":{}}}"#, &mut |_| Ok::<_, ()>(Some(
            true
        )))
        .map(|choices| choices.len()),
        Ok(0)
    );
}

/// The choices offer only enabled transitions: a guard that is exactly
/// `true` offers, `false` neither offers nor preempts, and a failed or
/// non-boolean guard does not offer but still preempts later transitions.
#[test]
fn the_snapshot_choices_offers_only_transitions_whose_guards_hold() {
    let schema = json!({"type": "object"});
    let events = choices_events(&[
        guarded(with_schema(&["on"], &schema), "yes"),
        guarded(with_schema(&["off"], &schema), "no"),
        // The disabled `off` takes nothing: this one offers it.
        with_schema(&["off"], &schema),
        guarded(with_schema(&["maybe"], &schema), "1 + 1"),
        // Preempted by the earlier `maybe`, whose guard is not a boolean.
        with_schema(&["maybe"], &schema),
        // A disabled schema-less transition preempts nothing either.
        guarded(without_schema(&["later"]), "no"),
        with_schema(&["later"], &schema),
    ])
    .expect("the snapshot is well formed");
    assert_eq!(events, ["on", "off", "later"]);

    let mut asked = Vec::new();
    snapshot_choices(
        &snapshot(&[guarded(with_schema(&["x"], &schema), "plan !== null")]),
        &mut |cond| {
            asked.push(cond.to_owned());
            Ok::<_, ()>(Some(true))
        },
    )
    .expect("well formed");
    assert_eq!(asked, ["plan !== null"], "the guard's text is evaluated");

    let mut lying = with_schema(&["x"], &schema);
    lying["has_cond"] = json!(true);
    assert_eq!(
        choices_events(&[lying]),
        Err(Failure::Execution("the session snapshot is malformed")),
        "has_cond without its cond"
    );
    assert_eq!(
        snapshot_choices(
            &snapshot(&[guarded(with_schema(&["x"], &schema), "fatal")]),
            &mut |_| Err("fatal")
        )
        .map(|choices| choices.len()),
        Err(Failure::Host("fatal")),
        "a non-recoverable host failure passes through"
    );
}

/// The choices of `transitions` under the active atomic states `atomic`, every
/// guard enabled.
fn choices_events_in(atomic: &[&str], transitions: &[Json]) -> Vec<String> {
    snapshot_choices(&snapshot_in(atomic, transitions), &mut |_| {
        Ok::<_, ()>(Some(true))
    })
    .expect("the snapshot is well formed")
    .into_iter()
    .map(|candidate| candidate.event)
    .collect()
}

/// In a parallel state each active atomic state selects its own
/// transition (SCXML 3.13): a transition in one region's chain does not
/// preempt a sibling region's transition for the same event.
#[test]
fn the_snapshot_choices_preempts_only_within_one_atomic_states_chain() {
    let schema = json!({"type": "object"});
    // Leaf a1's chain (a1, A, P), then leaf b1's (b1, B): A's schema-less
    // `go` does not hide b1's schema'd `go`.
    let events = choices_events_in(
        &["a1", "b1"],
        &[
            from("a1", with_schema(&["a"], &schema)),
            from("A", without_schema(&["go"])),
            from("P", with_schema(&["go", "p"], &schema)),
            from("b1", with_schema(&["go"], &schema)),
            from("B", with_schema(&["go.more", "b"], &schema)),
        ],
    );
    // P's `go` is preempted by A's in a1's chain; b1's `go` is offered once;
    // B's `go.more` is preempted by b1's `go` in b1's chain.
    assert_eq!(events, ["a", "p", "go", "b"]);

    // The same event offered by two chains is offered once.
    let events = choices_events_in(
        &["a1", "b1"],
        &[
            from("a1", with_schema(&["go"], &schema)),
            from("b1", with_schema(&["go"], &schema)),
        ],
    );
    assert_eq!(events, ["go"]);

    // A listing whose source is not a string, or a snapshot without its
    // configuration, is malformed.
    let mut sourceless = with_schema(&["x"], &schema);
    sourceless["source"] = json!(1);
    assert_eq!(
        choices_events(&[sourceless]),
        Err(Failure::Execution("the session snapshot is malformed"))
    );
    let unconfigured =
        json!({"snapshot": {"root": {"transitions": [with_schema(&["x"], &schema)]}}});
    assert_eq!(
        snapshot_choices(&unconfigured.to_string(), &mut |_| Ok::<_, ()>(Some(true)))
            .map(|choices| choices.len()),
        Err(Failure::Execution("the session snapshot is malformed"))
    );
}

/// The event-name bound is a wasm-only resource bound: natively a long
/// event name is offered; on wasm one over the bound is not.
#[test]
fn the_event_name_bound_applies_to_wasm_builds_only() {
    let schema = json!({"type": "object"});
    let long = "e".repeat(257);
    let offered = choices_events_in(&["s"], &[with_schema(&[long.as_str()], &schema)]);
    if cfg!(target_arch = "wasm32") {
        assert_eq!(MAX_EVENT_NAME_BYTES, 256);
        assert!(offered.is_empty());
    } else {
        assert_eq!(MAX_EVENT_NAME_BYTES, usize::MAX);
        assert_eq!(offered, std::slice::from_ref(&long));
    }
    let at_bound = "e".repeat(256);
    assert_eq!(
        choices_events_in(&["s"], &[with_schema(&[at_bound.as_str()], &schema)]),
        [at_bound],
        "a name at the wasm bound is offered on every build"
    );
}

/// The events of the choices `execute` writes for the order snapshot with
/// `attributes`.
fn executed_choices(attributes: &[Attribute]) -> Vec<String> {
    let mut host = FakeHost {
        choices_snapshot: Some(order_snapshot()),
        ..FakeHost::default()
    };
    assert_eq!(
        execute("choice", attributes, &[], &mut host),
        Ok(Executed::Handled)
    );
    let [(location, Data::JsonArray(choices))] = host.assigned.as_slice() else {
        panic!("one choices array is assigned: {:?}", host.assigned);
    };
    assert_eq!(location, "choices");
    let choices: Json = serde_json::from_str(choices).expect("the choices are JSON");
    choices
        .as_array()
        .expect("the choices are an array")
        .iter()
        .map(|entry| entry["event"].as_str().expect("event").to_owned())
        .collect()
}

#[test]
fn the_events_filter_keeps_the_events_a_descriptor_names() {
    assert_eq!(
        executed_choices(&[location("choices"), attr("events", "order.cancel")]),
        ["order.cancel"]
    );
    assert_eq!(
        executed_choices(&[
            location("choices"),
            attr("events", " order.hold\n order.ship ")
        ]),
        ["order.ship", "order.hold"],
        "choices order, not descriptor order"
    );
}

#[test]
fn the_events_filter_matches_descriptors_as_token_prefixes() {
    assert_eq!(
        executed_choices(&[location("choices"), attr("events", "order")]),
        ["order.ship", "order.cancel", "order.hold"]
    );
    assert_eq!(
        executed_choices(&[location("choices"), attr("events", "order.*")]),
        ["order.ship", "order.cancel", "order.hold"]
    );
    assert_eq!(
        executed_choices(&[location("choices"), attr("events", "*")]),
        ["order.ship", "order.cancel", "order.hold"]
    );
    assert_eq!(
        executed_choices(&[location("choices"), attr("events", "order.sh ord")]),
        Vec::<String>::new(),
        "a descriptor matches whole tokens only"
    );
}

#[test]
fn the_events_filter_with_no_match_writes_an_empty_choices() {
    assert_eq!(
        executed_choices(&[
            location("choices"),
            attr("events", "bot.input order.refund")
        ]),
        Vec::<String>::new()
    );
}

#[test]
fn without_the_events_attribute_the_choices_is_unfiltered() {
    assert_eq!(
        executed_choices(&[location("choices")]),
        ["order.ship", "order.cancel", "order.hold"]
    );
    assert_eq!(
        executed_choices(&[attr("events", "order.cancel"), location("choices")]),
        ["order.cancel"],
        "attribute order does not matter"
    );
}

/// The filter applies after selection order: an earlier transition the
/// filter leaves out still preempts a later one for its event.
#[test]
fn the_events_filter_keeps_selection_order_preemption() {
    let schema = json!({"type": "object"});
    let mut host = FakeHost {
        choices_snapshot: Some(snapshot(&[
            with_schema(&["bot.input"], &schema),
            with_schema(&["bot.input.text", "bot.focus_device"], &schema),
        ])),
        ..FakeHost::default()
    };
    execute(
        "choice",
        &[
            location("choices"),
            attr("events", "bot.input.text bot.focus_device"),
        ],
        &[],
        &mut host,
    )
    .expect("handled");
    let Data::JsonArray(choices) = &host.assigned[0].1 else {
        panic!("the choices are an array");
    };
    let choices: Json = serde_json::from_str(choices).expect("JSON");
    assert_eq!(
        choices
            .as_array()
            .expect("array")
            .iter()
            .map(|entry| entry["event"].clone())
            .collect::<Vec<_>>(),
        [json!("bot.focus_device")],
        "bot.input.text is taken by the earlier bot.input transition"
    );
}

#[test]
fn a_malformed_events_attribute_raises_error_execution() {
    for (value, detail) in [
        ("", "decision:choice events names no event descriptor"),
        (" \t\n", "decision:choice events names no event descriptor"),
        (
            "a..b",
            "decision:choice events holds a malformed event descriptor",
        ),
        (
            "a.*.b",
            "decision:choice events holds a malformed event descriptor",
        ),
        (
            "ok .x",
            "decision:choice events holds a malformed event descriptor",
        ),
    ] {
        let mut host = FakeHost {
            choices_snapshot: Some(order_snapshot()),
            ..FakeHost::default()
        };
        assert_eq!(
            execution(execute(
                "choice",
                &[location("choices"), attr("events", value)],
                &[],
                &mut host
            )),
            detail,
            "{value:?}"
        );
        assert!(host.assigned.is_empty());
        assert!(host.snapshot_requests.is_empty(), "refused before the read");
    }
    let mut host = FakeHost {
        choices_snapshot: Some(order_snapshot()),
        ..FakeHost::default()
    };
    assert_eq!(
        execution(execute(
            "choice",
            &[
                location("choices"),
                attr("events", "a"),
                attr("events", "b")
            ],
            &[],
            &mut host
        )),
        "decision:choice repeats an attribute"
    );
    let mut host = FakeHost {
        choices_snapshot: Some(order_snapshot()),
        ..FakeHost::default()
    };
    assert_eq!(
        execution(execute(
            "choice",
            &[
                location("choices"),
                Attribute {
                    namespace_uri: Some(NAMESPACE_URI.into()),
                    local_name: "events".into(),
                    value: "a".into(),
                },
            ],
            &[],
            &mut host
        )),
        "decision:choice takes only the location and events attributes",
        "only the unqualified events attribute"
    );
}

/// A host whose `offers` location holds `choices`.
fn holding(choices: &Json) -> FakeHost {
    let mut host = FakeHost::default();
    host.values
        .insert("offers".into(), Data::JsonArray(canonical_json(choices)));
    host
}

/// Executes `<decision:compact>` with `attributes` on `host`.
fn compact_with(
    attributes: &[Attribute],
    host: &mut FakeHost,
) -> Result<Executed, Failure<&'static str>> {
    execute("compact", attributes, &[], host)
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
        "description": "Say text.",
        "target": "communication",
        "schema": {
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "bot.ability.communication.respond",
            "description": "Say text.",
            "examples": [{"text": "hi"}],
            "type": "object",
            "properties": {
                "text": {"type": "string", "title": "Text", "description": "What to say.", "examples": ["hi"]},
                "title": {"type": "string", "default": {"title": "kept"}},
            },
            "$defs": {"examples": {"enum": [{"title": "kept"}], "title": "dropped"}},
            "anyOf": [{"title": "dropped", "required": ["title"]}],
            "required": ["text"],
            "additionalProperties": false,
        },
    }])
}

#[test]
fn compact_drops_schema_title_and_examples_and_keeps_property_names() {
    let mut host = holding(&verbose_offers());
    let over = attr("overlocation", "over");
    assert_eq!(
        compact_with(&[location("offers"), over], &mut host),
        Ok(Executed::Handled)
    );
    assert_eq!(
        assigned_json(&host, "offers"),
        json!([{
            "event": "bot.ability.communication.respond",
            "description": "Say text.",
            "target": "communication",
            "schema": {
                "description": "Say text.",
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "What to say."},
                    "title": {"type": "string", "default": {"title": "kept"}},
                },
                "$defs": {"examples": {"enum": [{"title": "kept"}]}},
                "anyOf": [{"required": ["title"]}],
                "required": ["text"],
                "additionalProperties": false,
            },
        }])
    );
    assert_eq!(assigned(&host, "over"), Data::Bool(false));
    // Overlocation is written before location.
    assert_eq!(host.assigned[0].0, "over");
}

#[test]
fn compact_writes_canonical_json_whose_length_is_what_the_bound_measures() {
    let mut host = holding(&verbose_offers());
    compact_with(&[location("offers")], &mut host).expect("compacts");
    let Data::JsonArray(text) = assigned(&host, "offers") else {
        panic!("a JSON array");
    };
    let (expected, over) = compact_choices(&verbose_offers(), DEFAULT_COMPACT_BOUND).unwrap();
    assert_eq!((text.as_str(), over), (expected.as_str(), false));
    assert_eq!(text, canonical_json(&serde_json::from_str(&text).unwrap()));
    // The bound is inclusive: exactly the compacted length keeps descriptions.
    let (exact, over) = compact_choices(&verbose_offers(), text.len()).unwrap();
    assert_eq!((exact, over), (text, false));
}

#[test]
fn compact_over_the_bound_drops_descriptions_before_it_drops_the_choices() {
    let (full, _) = compact_choices(&verbose_offers(), usize::MAX).unwrap();
    let (bare, over) = compact_choices(&verbose_offers(), full.len() - 1).unwrap();
    assert!(!over);
    assert!(!bare.contains("description"), "{bare}");
    let bare: Json = serde_json::from_str(&bare).unwrap();
    assert_eq!(bare[0]["target"], json!("communication"));
    assert_eq!(
        bare[0]["schema"]["properties"]["text"],
        json!({"type": "string"})
    );
    // A property named description is a name, not a keyword: it stays.
    let named = json!([{"event": "e", "schema": {"properties": {"description": {"type": "string", "description": "x"}}}}]);
    let (named_full, _) = compact_choices(&named, usize::MAX).unwrap();
    let (bare, over) = compact_choices(&named, named_full.len() - 1).unwrap();
    assert!(!over);
    assert_eq!(
        serde_json::from_str::<Json>(&bare).unwrap(),
        json!([{"event": "e", "schema": {"properties": {"description": {"type": "string"}}}}])
    );
}

#[test]
fn compact_still_over_the_bound_assigns_no_choices_and_sets_overlocation() {
    let mut host = holding(&verbose_offers());
    compact_with(
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
    compact_with(&[location("offers"), attr("bound", "40")], &mut host).expect("compacts");
    assert_eq!(
        host.assigned,
        vec![("offers".into(), Data::JsonArray("[]".into()))]
    );
    // An empty list is never over.
    assert_eq!(compact_choices(&json!([]), 2), Ok(("[]".into(), false)));
}

#[test]
fn compact_is_idempotent() {
    for bound in [usize::MAX, 400, 10] {
        let (once, over) = compact_choices(&verbose_offers(), bound).unwrap();
        let (twice, again) = compact_choices(&serde_json::from_str(&once).unwrap(), bound).unwrap();
        assert_eq!(twice, once, "bound {bound}");
        assert_eq!(again, over && once != "[]", "bound {bound}");
    }
}

#[test]
fn malformed_compact_elements_or_choices_raise_error_execution() {
    let offers = verbose_offers();
    let cases: Vec<(Vec<Attribute>, Option<Json>, &str)> = vec![
        (
            vec![],
            Some(offers.clone()),
            "decision:compact requires a non-empty location",
        ),
        (
            vec![location(" ")],
            Some(offers.clone()),
            "decision:compact requires a non-empty location",
        ),
        (
            vec![location("offers"), location("offers")],
            Some(offers.clone()),
            "decision:compact repeats an attribute",
        ),
        (
            vec![location("offers"), attr("events", "e")],
            Some(offers.clone()),
            "decision:compact takes only the location, bound and overlocation attributes",
        ),
        (
            vec![location("offers"), attr("overlocation", "")],
            Some(offers.clone()),
            "decision:compact overlocation is empty",
        ),
        (
            vec![location("offers"), attr("bound", "-1")],
            Some(offers.clone()),
            "decision:compact bound is not a decimal integer",
        ),
        (
            vec![location("offers"), attr("bound", "1e3")],
            Some(offers.clone()),
            "decision:compact bound is not a decimal integer",
        ),
        (
            vec![location("offers"), attr("bound", "99999999999999999999999")],
            Some(offers.clone()),
            "decision:compact bound is not a decimal integer",
        ),
        (
            vec![location("offers")],
            None,
            "decision:compact location does not hold choices",
        ),
        (
            vec![location("offers")],
            Some(json!([1])),
            "decision:compact found a choice that is not an object",
        ),
        (
            vec![location("offers")],
            Some(json!([{"schema": {}}])),
            "decision:compact found a malformed choice",
        ),
        (
            vec![location("offers")],
            Some(json!([{"event": "", "schema": {}}])),
            "decision:compact found a malformed choice",
        ),
        (
            vec![location("offers")],
            Some(json!([{"event": "e"}])),
            "decision:compact found a malformed choice",
        ),
        (
            vec![location("offers")],
            Some(json!([{"event": "e", "schema": {}, "target": ""}])),
            "decision:compact found a malformed choice",
        ),
        (
            vec![location("offers")],
            Some(json!([{"event": "e", "schema": {}, "description": 1}])),
            "decision:compact found a malformed choice",
        ),
        (
            vec![location("offers")],
            Some(json!([{"event": "e", "schema": {}, "extra": 1}])),
            "decision:compact found a malformed choice",
        ),
    ];
    for (attributes, value, detail) in cases {
        let mut host = value.map_or_else(FakeHost::default, |value| holding(&value));
        assert_eq!(
            execution(compact_with(&attributes, &mut host)),
            detail,
            "{attributes:?}"
        );
        assert!(host.assigned.is_empty(), "{attributes:?}");
    }
}

#[test]
fn compact_reads_only_arrays_takes_no_content_and_fails_on_assignment() {
    let offers = verbose_offers();
    // A location holding an object, not an array.
    let mut host = FakeHost::default();
    host.values
        .insert("offers".into(), Data::JsonObject("{}".into()));
    assert_eq!(
        execution(compact_with(&[location("offers")], &mut host)),
        "decision:compact location does not hold choices"
    );
    // Content is refused.
    let mut host = holding(&offers);
    assert_eq!(
        execution(execute(
            "compact",
            &[location("offers")],
            &[Content::Start, Content::End],
            &mut host
        )),
        "decision:compact holds no content"
    );
    // A failed assignment is error.execution; a host failure passes through.
    let mut host = holding(&offers);
    host.fail_assign = true;
    assert_eq!(
        execution(compact_with(&[location("offers")], &mut host)),
        "decision:compact could not assign the choices"
    );
    let mut host = holding(&offers);
    host.fail_assign = true;
    assert_eq!(
        execution(compact_with(
            &[location("offers"), attr("overlocation", "over")],
            &mut host
        )),
        "decision:compact could not assign overlocation"
    );
    assert_eq!(
        compact_with(&[location("fatal")], &mut FakeHost::default()),
        Err(Failure::Host("fatal"))
    );
}
