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

fn focus_choice() -> Json {
    json!({
        "event": "bot.focus_device",
        "description": "Focus a device.",
        "schema": {"type": "object", "properties": {"device": {"type": "string"}}, "required": ["device"]},
    })
}

fn clear_choice() -> Json {
    json!({"event": "bot.clear_focus", "schema": {"type": "object"}})
}

fn pin_attributes(values: &str) -> Vec<Attribute> {
    vec![
        location("own"),
        attr("event", "bot.focus_device"),
        attr("property", "device"),
        attr("values", values),
    ]
}

#[test]
fn pin_sets_the_property_enum_of_the_named_event_only() {
    let mut host = with_values(&[
        ("own", json!([focus_choice(), clear_choice()])),
        ("candidates", json!(["lamp", "fan"])),
    ]);
    assert_eq!(
        execute("pin", &pin_attributes("candidates"), &[], &mut host),
        Ok(Executed::Handled)
    );
    let mut pinned = focus_choice();
    pinned["schema"]["properties"]["device"]["enum"] = json!(["lamp", "fan"]);
    assert_eq!(assigned_any(&host, "own"), json!([pinned, clear_choice()]));
    assert!(
        matches!(assigned(&host, "own"), Data::JsonArray(text) if scxml_event_schema::is_canonical_json(&text))
    );
}

#[test]
fn pin_with_no_values_removes_the_event() {
    let mut host = with_values(&[
        ("own", json!([focus_choice(), clear_choice()])),
        ("candidates", json!([])),
    ]);
    assert_eq!(
        execute("pin", &pin_attributes("candidates"), &[], &mut host),
        Ok(Executed::Handled)
    );
    assert_eq!(assigned_any(&host, "own"), json!([clear_choice()]));
}

#[test]
fn malformed_pins_raise_error_execution_and_assign_nothing() {
    let cases: Vec<(Vec<Attribute>, Json, Json, &str)> = vec![
        (
            pin_attributes("candidates")[..3].to_vec(),
            json!([focus_choice()]),
            json!(["lamp"]),
            "decision:pin requires a non-empty values",
        ),
        (
            [pin_attributes("candidates"), vec![attr("bound", "1")]].concat(),
            json!([focus_choice()]),
            json!(["lamp"]),
            "decision:pin takes only the location, event, property and values attributes",
        ),
        (
            pin_attributes("candidates"),
            json!([focus_choice()]),
            json!({"lamp": true}),
            "decision:pin values is not an array",
        ),
        (
            pin_attributes("candidates"),
            json!([{"event": "bot.focus_device", "schema": {"type": "object"}}]),
            json!(["lamp"]),
            "decision:pin found no such property in the event's schema",
        ),
        (
            pin_attributes("candidates"),
            json!([{"event": "bot.focus_device"}]),
            json!(["lamp"]),
            "decision:pin found a malformed choice",
        ),
    ];
    for (attributes, own, candidates, detail) in cases {
        let mut host = with_values(&[("own", own), ("candidates", candidates)]);
        assert_eq!(
            execution(execute("pin", &attributes, &[], &mut host)),
            detail
        );
        assert!(host.assigned.is_empty(), "{detail}");
    }
}

fn respond_offer() -> Json {
    json!({
        "event": "bot.ability.communication.respond",
        "description": "Say text.",
        "schema": {"title": "respond", "type": "object", "description": "Say text.",
                   "properties": {"text": {"type": "string", "examples": ["hi"]}}},
    })
}

fn ignore_offer() -> Json {
    json!({"event": "bot.ability.cognition.ignore", "schema": {"type": "object"}})
}

fn assemble_attributes(bound: &str) -> Vec<Attribute> {
    vec![
        location("choices"),
        attr("routeslocation", "routes"),
        attr("choices", "own"),
        attr("offers", "offers"),
        attr("bound", bound),
        attr("overlocation", "over"),
        attr("sizelocation", "size"),
    ]
}

fn assembling() -> FakeHost {
    with_values(&[
        ("own", json!([clear_choice()])),
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
fn assemble_tags_offers_by_target_in_name_order_compacts_and_builds_the_routes() {
    let mut host = assembling();
    assert_eq!(
        execute("assemble", &assemble_attributes("32768"), &[], &mut host),
        Ok(Executed::Handled)
    );
    let choices = json!([
        clear_choice(),
        {"event": "bot.ability.cognition.ignore", "schema": {"type": "object"}, "target": "cognition"},
        {"event": "bot.ability.communication.respond", "description": "Say text.", "target": "communication",
         "schema": {"type": "object", "description": "Say text.", "properties": {"text": {"type": "string"}}}},
        {"event": "bot.ability.cognition.ignore", "schema": {"type": "object"}, "target": "communication"},
    ]);
    assert_eq!(assigned_any(&host, "choices"), choices);
    assert_eq!(
        assigned_any(&host, "routes"),
        json!({
            "bot.clear_focus": [null],
            "bot.ability.cognition.ignore": ["cognition", "communication"],
            "bot.ability.communication.respond": ["communication"],
        })
    );
    assert_eq!(assigned(&host, "over"), Data::Bool(false));
    let size = canonical_json(&choices).len();
    assert_eq!(assigned(&host, "size"), number(size));
    // The flags and the routes are assigned before the choices.
    let order: Vec<&str> = host.assigned.iter().map(|(at, _)| at.as_str()).collect();
    assert_eq!(order, ["over", "size", "routes", "choices"]);
}

#[test]
fn assemble_over_the_bound_drops_descriptions_then_keeps_the_list_and_sets_over() {
    let full = {
        let mut host = assembling();
        execute("assemble", &assemble_attributes("32768"), &[], &mut host).expect("assembled");
        canonical_json(&assigned_any(&host, "choices")).len()
    };
    // One byte short of the full list: descriptions go, and that fits.
    let mut host = assembling();
    execute(
        "assemble",
        &assemble_attributes(&(full - 1).to_string()),
        &[],
        &mut host,
    )
    .expect("assembled");
    let choices = assigned_any(&host, "choices");
    assert!(!canonical_json(&choices).contains("description"));
    assert_eq!(assigned(&host, "over"), Data::Bool(false));
    // Under even that: the description-less list is kept, over is set, and
    // size is that list's size.
    let mut host = assembling();
    execute("assemble", &assemble_attributes("10"), &[], &mut host).expect("assembled");
    assert_eq!(assigned_any(&host, "choices"), choices);
    assert_eq!(assigned(&host, "over"), Data::Bool(true));
    assert_eq!(
        assigned(&host, "size"),
        number(canonical_json(&choices).len())
    );
}

#[test]
fn malformed_assembles_raise_error_execution_and_assign_nothing() {
    let cases: Vec<(Vec<Attribute>, Json, Json, &str)> = vec![
        (
            assemble_attributes("1")[..3].to_vec(),
            json!([]),
            json!({}),
            "decision:assemble requires a non-empty offers",
        ),
        (
            [assemble_attributes("1"), vec![attr("events", "x")]].concat(),
            json!([]),
            json!({}),
            "decision:assemble takes only the location, routeslocation, choices, offers, bound, overlocation and sizelocation attributes",
        ),
        (
            assemble_attributes("-1"),
            json!([]),
            json!({}),
            "decision:assemble bound is not a decimal integer",
        ),
        (
            assemble_attributes("1"),
            json!({}),
            json!({}),
            "decision:assemble choices does not hold choices",
        ),
        (
            assemble_attributes("1"),
            json!([]),
            json!([]),
            "decision:assemble offers does not hold offers",
        ),
        (
            assemble_attributes("1"),
            json!([]),
            json!({"cognition": {}}),
            "decision:assemble offers does not hold offers",
        ),
        (
            assemble_attributes("1"),
            json!([]),
            json!({"cognition": [{"event": "x", "schema": {}, "extra": 1}]}),
            "decision:assemble found a malformed choice",
        ),
        (
            assemble_attributes("1"),
            json!([]),
            json!({"": []}),
            "decision:assemble offers names an empty target",
        ),
    ];
    for (attributes, own, offers, detail) in cases {
        let mut host = with_values(&[("own", own), ("offers", offers)]);
        assert_eq!(
            execution(execute("assemble", &attributes, &[], &mut host)),
            detail
        );
        assert!(host.assigned.is_empty(), "{detail}");
    }
}

fn routing(selections: &Json) -> Json {
    let mut host = with_values(&[
        ("selections", selections.clone()),
        (
            "routes",
            json!({
                "bot.clear_focus": [null],
                "bot.ability.cognition.ignore": ["cognition", "communication"],
                "bot.ability.communication.respond": ["communication"],
            }),
        ),
    ]);
    assert_eq!(
        execute(
            "route",
            &[
                location("routed"),
                attr("selections", "selections"),
                attr("routes", "routes")
            ],
            &[],
            &mut host
        ),
        Ok(Executed::Handled)
    );
    assigned_any(&host, "routed")
}

#[test]
fn route_places_each_selection_in_order_or_says_why_not() {
    let routed = routing(&json!([
        {"event": "bot.clear_focus"},
        {"event": "bot.ability.communication.respond", "data": {"text": "hi"}, "reason": "greet"},
        {"event": "bot.ability.cognition.ignore", "data": null, "target": "cognition"},
        {"event": "bot.ability.cognition.ignore"},
        {"event": "bot.ability.communication.respond", "target": "cognition"},
        {"event": "bot.focus_device", "data": {"device": "lamp"}},
        {"data": {}},
        "respond",
    ]));
    assert_eq!(
        routed,
        json!([
            {"event": "bot.clear_focus", "data": {}, "target": null},
            {"event": "bot.ability.communication.respond", "data": {"text": "hi"}, "target": "communication"},
            {"event": "bot.ability.cognition.ignore", "data": {}, "target": "cognition"},
            {"dropped": "bot.ability.cognition.ignore names no target among [\"cognition\",\"communication\"]"},
            {"dropped": "bot.ability.communication.respond is not offered by cognition"},
            {"dropped": "bot.focus_device was not offered this turn"},
            {"dropped": "a malformed selection"},
            {"dropped": "a malformed selection"},
        ])
    );
}

#[test]
fn malformed_routes_raise_error_execution_and_assign_nothing() {
    let route_attributes = [
        location("routed"),
        attr("selections", "selections"),
        attr("routes", "routes"),
    ];
    let cases: Vec<(&[Attribute], Json, Json, &str)> = vec![
        (
            &route_attributes[..2],
            json!([]),
            json!({}),
            "decision:route requires a non-empty routes",
        ),
        (
            &route_attributes,
            json!({}),
            json!({}),
            "decision:route selections is not an array",
        ),
        (
            &route_attributes,
            json!([]),
            json!({"x": "cognition"}),
            "decision:route routes is not a routing table",
        ),
        (
            &route_attributes,
            json!([]),
            json!({"x": [1]}),
            "decision:route routes is not a routing table",
        ),
    ];
    for (attributes, selections, routes, detail) in cases {
        let mut host = with_values(&[("selections", selections), ("routes", routes)]);
        assert_eq!(
            execution(execute("route", attributes, &[], &mut host)),
            detail
        );
        assert!(host.assigned.is_empty(), "{detail}");
    }
    let mut host = FakeHost::default();
    assert_eq!(
        execution(execute(
            "route",
            &route_attributes,
            &[Content::Text("x".into())],
            &mut host
        )),
        "decision:route holds no content"
    );
}

#[test]
fn names_lists_each_choice_with_its_target() {
    let mut host = with_values(&[(
        "choices",
        json!([
            clear_choice(),
            {"event": "bot.ability.cognition.ignore", "schema": {}, "target": "cognition"},
        ]),
    )]);
    assert_eq!(
        execute(
            "names",
            &[location("names"), attr("choices", "choices")],
            &[],
            &mut host
        ),
        Ok(Executed::Handled)
    );
    assert_eq!(
        assigned(&host, "names"),
        Data::String("bot.clear_focus bot.ability.cognition.ignore@cognition".into())
    );
    let mut host = with_values(&[("choices", json!([]))]);
    execute(
        "names",
        &[location("names"), attr("choices", "choices")],
        &[],
        &mut host,
    )
    .expect("named");
    assert_eq!(assigned(&host, "names"), Data::String(String::new()));
    let mut host = with_values(&[("choices", json!([{"schema": {}}]))]);
    assert_eq!(
        execution(execute(
            "names",
            &[location("names"), attr("choices", "choices")],
            &[],
            &mut host
        )),
        "decision:names found a malformed choice"
    );
    assert_eq!(
        execution(execute(
            "names",
            &[location("names")],
            &[],
            &mut FakeHost::default()
        )),
        "decision:names requires a non-empty choices"
    );
}

#[test]
fn the_new_elements_pass_host_failures_through() {
    let mut host = FakeHost::default();
    assert_eq!(
        execute(
            "names",
            &[location("names"), attr("choices", "fatal")],
            &[],
            &mut host
        ),
        Err(Failure::Host("fatal"))
    );
    let mut host = with_values(&[("choices", json!([]))]);
    host.fail_assign = true;
    assert_eq!(
        execution(execute(
            "names",
            &[location("names"), attr("choices", "choices")],
            &[],
            &mut host
        )),
        "decision:names could not assign the names"
    );
}

#[test]
fn selection_builds_the_strict_dispatch_schema_of_the_choices() {
    let mut host = with_values(&[(
        "choices",
        json!([
            {"event": "bot.clear_focus", "schema": {"$schema": "x", "title": "Clear", "examples": [{}], "type": "object", "properties": {"title": {"type": "string", "title": "kept"}}}},
            {"event": "bot.ability.communication.respond", "description": "Say text.", "target": "communication",
             "schema": {"type": "object", "additionalProperties": true, "properties": {"text": {"type": "string"}}, "required": ["text"]}},
            {"event": "bot.ability.cognition.ignore", "target": "cognition", "schema": "not an object"},
            {"event": "bot.ability.cognition.ignore", "target": "communication", "description": "later", "schema": {}},
            {"event": "bot.ability.cognition.ignore", "target": "cognition", "schema": {}},
        ]),
    )]);
    assert_eq!(
        execute(
            "selection",
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
        json!({"type": "string", "description": "A short reason this event was selected."});
    let expected = json!({
        "type": "object",
        "properties": {"events": {
            "type": "array",
            "description": "All events necessary for the input this turn; an empty array selects none.",
            "items": {"anyOf": [
                {
                    "type": "object",
                    "description": "bot.clear_focus",
                    "properties": {
                        "event": {"type": "string", "enum": ["bot.clear_focus"]},
                        "data": {"type": "object", "properties": {"title": {"type": "string", "title": "kept"}}, "additionalProperties": false},
                        "reason": reason,
                    },
                    "required": ["event", "data", "reason"],
                    "additionalProperties": false,
                },
                {
                    "type": "object",
                    "description": "Say text.",
                    "properties": {
                        "event": {"type": "string", "enum": ["bot.ability.communication.respond"]},
                        "data": {"type": "object", "additionalProperties": true, "properties": {"text": {"type": "string"}}, "required": ["text"]},
                        "reason": reason,
                        "target": {"type": "string", "enum": ["communication"], "description": "The ability that receives this event."},
                    },
                    "required": ["event", "data", "reason", "target"],
                    "additionalProperties": false,
                },
                {
                    "type": "object",
                    "description": "bot.ability.cognition.ignore",
                    "properties": {
                        "event": {"type": "string", "enum": ["bot.ability.cognition.ignore"]},
                        "data": {"type": "object", "additionalProperties": false},
                        "reason": reason,
                        "target": {"type": "string", "enum": ["cognition", "communication"], "description": "The ability that receives this event."},
                    },
                    "required": ["event", "data", "reason", "target"],
                    "additionalProperties": false,
                },
            ]},
        }},
        "required": ["events"],
        "additionalProperties": false,
    });
    assert_eq!(text, canonical_json(&expected));
    let mut host = with_values(&[("choices", json!([]))]);
    execute(
        "selection",
        &[location("schema"), attr("choices", "choices")],
        &[],
        &mut host,
    )
    .expect("built");
    assert_eq!(
        assigned_any(&host, "schema")["properties"]["events"]["items"]["anyOf"],
        json!([])
    );
}

#[test]
fn malformed_selections_raise_error_execution_and_assign_nothing() {
    let attributes = [location("schema"), attr("choices", "choices")];
    let cases: Vec<(&[Attribute], Json, &str)> = vec![
        (
            &attributes[..1],
            json!([]),
            "decision:selection requires a non-empty choices",
        ),
        (
            &attributes,
            json!({}),
            "decision:selection found a malformed choice",
        ),
        (
            &attributes,
            json!([1]),
            "decision:selection found a choice that is not an object",
        ),
        (
            &attributes,
            json!([{"event": "x"}]),
            "decision:selection found a malformed choice",
        ),
    ];
    for (attributes, choices, detail) in cases {
        let mut host = with_values(&[("choices", choices)]);
        let got = execution(execute("selection", attributes, &[], &mut host));
        assert!(
            got == detail || got == "decision:selection choices does not hold choices",
            "{detail}: {got}"
        );
        assert!(host.assigned.is_empty(), "{detail}");
    }
    assert_eq!(
        execution(execute(
            "selection",
            &[
                location("schema"),
                attr("choices", "choices"),
                attr("bound", "1")
            ],
            &[],
            &mut FakeHost::default()
        )),
        "decision:selection takes only the location and choices attributes"
    );
    assert_eq!(
        execution(execute(
            "selection",
            &attributes,
            &[Content::Start, Content::End],
            &mut FakeHost::default()
        )),
        "decision:selection holds no content"
    );
    assert_eq!(
        execute(
            "selection",
            &[location("schema"), attr("choices", "fatal")],
            &[],
            &mut FakeHost::default()
        ),
        Err(Failure::Host("fatal"))
    );
}

fn real(value: f64) -> Data {
    Data::Number(scxml_types::FiniteNumber::new(value).expect("finite"))
}

fn tuner_host(state: Json, observe: Option<f64>) -> FakeHost {
    let mut host = with_values(&[("tuner", state)]);
    if let Some(value) = observe {
        host.values.insert("confidence".into(), real(value));
    }
    host
}

fn run_tuner(host: &mut FakeHost, observe: bool) {
    let mut attributes = vec![location("tuner"), attr("thresholdlocation", "threshold")];
    if observe {
        attributes.push(attr("observe", "confidence"));
    }
    assert_eq!(
        execute("tuner", &attributes, &[], host),
        Ok(Executed::Handled)
    );
}

#[test]
fn the_tuner_keeps_mosfets_confidence_baseline() {
    // During the warmup the threshold is the floor; a malformed state is
    // the initial one.
    for state in [
        json!({"mean": 70, "variance": 100, "n": 0}),
        json!({"mean": "70", "variance": 100, "n": 0}),
        json!({"mean": 70, "variance": 100, "n": -1}),
        json!([]),
    ] {
        let mut host = tuner_host(state, None);
        run_tuner(&mut host, false);
        assert_eq!(assigned(&host, "threshold"), real(0.35));
        assert_eq!(
            assigned(&host, "tuner"),
            Data::JsonObject(r#"{"mean":70,"n":0,"variance":100}"#.into())
        );
        assert_eq!(host.assigned[0].0, "threshold");
    }
    // The first observation sets the mean; an equal one keeps it.
    let mut host = tuner_host(json!({"mean": 70, "variance": 100, "n": 0}), Some(34.0));
    run_tuner(&mut host, true);
    assert_eq!(
        assigned_any(&host, "tuner"),
        json!({"mean": 34, "variance": 100, "n": 1})
    );
    let mut host = tuner_host(json!({"mean": 34, "variance": 100, "n": 1}), Some(34.0));
    run_tuner(&mut host, true);
    assert_eq!(
        assigned_any(&host, "tuner"),
        json!({"mean": 34, "variance": 100, "n": 2})
    );
    // After the warmup: max(35 / 2, 70 - 1 * sqrt(100)) = 60.
    let mut host = tuner_host(json!({"mean": 70, "variance": 100, "n": 8}), None);
    run_tuner(&mut host, false);
    assert_eq!(assigned(&host, "threshold"), real(0.6));
    // The EMA, byte for byte as the document computed it.
    let mut host = tuner_host(json!({"mean": 70, "variance": 100, "n": 8}), Some(34.0));
    run_tuner(&mut host, true);
    assert_eq!(
        assigned(&host, "tuner"),
        Data::JsonObject(r#"{"mean":67.12,"n":9,"variance":195.68}"#.into())
    );
    assert_eq!(
        assigned(&host, "threshold"),
        real((67.12 - 195.68_f64.sqrt()) / 100.0)
    );
    let mut host = tuner_host(
        json!({"mean": 67.12, "variance": 195.68, "n": 9}),
        Some(34.0),
    );
    run_tuner(&mut host, true);
    assert_eq!(
        assigned(&host, "tuner"),
        Data::JsonObject(r#"{"mean":64.4704,"n":10,"variance":267.78035200000005}"#.into())
    );
    // The soft floor, and observations clamped to 0-100.
    let mut host = tuner_host(json!({"mean": 10, "variance": 100, "n": 9}), Some(250.0));
    run_tuner(&mut host, true);
    assert_eq!(
        assigned_any(&host, "tuner"),
        json!({"mean": 17.2, "variance": 740, "n": 10})
    );
    let mut host = tuner_host(json!({"mean": 10, "variance": 100, "n": 9}), None);
    run_tuner(&mut host, false);
    assert_eq!(assigned(&host, "threshold"), real(0.175));
    let mut host = tuner_host(json!({"mean": 70, "variance": 100, "n": 0}), Some(-5.0));
    run_tuner(&mut host, true);
    assert_eq!(
        assigned_any(&host, "tuner"),
        json!({"mean": 0, "variance": 100, "n": 1})
    );
}

#[test]
fn malformed_tuners_raise_error_execution() {
    let mut host = with_values(&[
        ("tuner", json!({"mean": 70, "variance": 100, "n": 0})),
        ("confidence", json!([])),
    ]);
    assert_eq!(
        execution(execute(
            "tuner",
            &[location("tuner"), attr("observe", "confidence")],
            &[],
            &mut host
        )),
        "decision:tuner observe is not a number"
    );
    assert!(host.assigned.is_empty());
    for (attributes, detail) in [
        (
            vec![attr("observe", "x")],
            "decision:tuner requires a non-empty location",
        ),
        (
            vec![location("tuner"), attr("thresholdlocation", " ")],
            "decision:tuner thresholdlocation is empty",
        ),
        (
            vec![location("tuner"), attr("bound", "1")],
            "decision:tuner takes only the location, thresholdlocation and observe attributes",
        ),
    ] {
        assert_eq!(
            execution(execute("tuner", &attributes, &[], &mut FakeHost::default())),
            detail
        );
    }
    let mut host = tuner_host(json!({"mean": 70, "variance": 100, "n": 0}), None);
    host.fail_assign = true;
    assert_eq!(
        execution(execute("tuner", &[location("tuner")], &[], &mut host)),
        "decision:tuner could not assign the tuner state"
    );
    assert_eq!(
        execute("tuner", &[location("fatal")], &[], &mut FakeHost::default()),
        Err(Failure::Host("fatal"))
    );
}
