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
    target_read: Option<SnapshotRead>,
    read_targets: Vec<String>,
    fail_snapshot_of: bool,
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

    fn snapshot_of(
        &mut self,
        exclude: &[Section],
        target: &str,
    ) -> Result<SnapshotRead, &'static str> {
        self.snapshot_requests.push(exclude.to_vec());
        self.read_targets.push(target.to_owned());
        if self.fail_snapshot_of {
            return Err("fatal");
        }
        Ok(self.target_read.clone().unwrap_or(SnapshotRead::NotRunning))
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
        "snapshot": {"sessions": [{
            "configuration": {"atomic": atomic, "full": atomic},
            "transitions": transitions,
        }]},
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
    for transition in snapshot["snapshot"]["sessions"][0]["transitions"]
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
    tampered["snapshot"]["sessions"][0]["transitions"][0]["event_schema"]["sha256"] = json!("00");
    assert_eq!(
        snapshot_choices(&tampered.to_string(), &mut |_| Ok::<_, ()>(Some(true)))
            .map(|choices| choices.len()),
        Err(Failure::Execution("the session snapshot is malformed"))
    );
    // A schema checked before is checked again when its text changes under
    // the same digest.
    let mut tampered: Json =
        serde_json::from_str(&snapshot(&[with_schema(&["x"], &schema)])).expect("JSON");
    tampered["snapshot"]["sessions"][0]["transitions"][0]["event_schema"]["schema"] =
        json!(r#"{"type":"array"}"#);
    assert_eq!(
        snapshot_choices(&tampered.to_string(), &mut |_| Ok::<_, ()>(Some(true)))
            .map(|choices| choices.len()),
        Err(Failure::Execution("the session snapshot is malformed"))
    );
    assert_eq!(
        snapshot_choices(r#"{"snapshot":{"sessions":[{}]}}"#, &mut |_| Ok::<_, ()>(
            Some(true)
        ))
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
        json!({"snapshot": {"sessions": [{"transitions": [with_schema(&["x"], &schema)]}]}});
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

/// The namespace has no `pin`: a property's values are a transition's
/// guard, not an enum written into the choices.
#[test]
fn pin_is_not_an_element_of_the_namespace() {
    let mut host = FakeHost::default();
    assert_eq!(
        execute("pin", &[location("choices")], &[], &mut host),
        Ok(Executed::NotHandled)
    );
    assert!(host.assigned.is_empty(), "nothing is assigned");
}

/// An effort is declared only by a session `<decision:offers>` reads
/// (decision D242): `<decision:choice>` refuses the attribute.
#[test]
fn an_effort_attribute_is_refused() {
    let mut host = FakeHost {
        choices_snapshot: Some(order_snapshot()),
        ..FakeHost::default()
    };
    assert_eq!(
        execution(execute(
            "choice",
            &[location("choices"), attr("effort", "2")],
            &[],
            &mut host
        )),
        "decision:choice takes only the location and events attributes"
    );
    assert!(host.assigned.is_empty(), "nothing is assigned");
}

/// A child's snapshot with `offerable` (its JSON text) in its data model.
fn child_snapshot(offerable: Option<&Json>, transitions: &[Json]) -> String {
    let mut document: Json = serde_json::from_str(&snapshot(transitions)).expect("JSON");
    if let Some(offerable) = offerable {
        document["snapshot"]["sessions"][0]["data_model"] = json!([
            {"name": "unrelated", "value": {"type": "number", "value": 1}},
            {"name": "offerable", "value": {"type": "json-object", "value": offerable.to_string()}},
        ]);
    }
    document.to_string()
}

/// The child's offers: the order machine's transitions, a guarded
/// `order.ship` listed first.
fn offers_snapshot(offerable: Option<&Json>) -> String {
    child_snapshot(
        offerable,
        &[
            guarded(with_schema(&["order.ship"], &ship_schema()), "ready"),
            with_schema(&["order.ship"], &ship_schema()),
            with_schema(&["order.cancel"], &cancel_schema()),
            with_schema(&["order.hold.*"], &hold_schema()),
            without_schema(&["order.note"]),
        ],
    )
}

fn offers_attributes(undeclared: Option<&str>) -> Vec<Attribute> {
    let mut attributes = vec![location("offers"), attr("target", "child")];
    if let Some(undeclared) = undeclared {
        attributes.push(attr("undeclared", undeclared));
    }
    attributes
}

/// The offers `<decision:offers>` assigns for `read`, or why it failed.
fn executed_offers(
    attributes: &[Attribute],
    read: SnapshotRead,
) -> Result<Option<Json>, &'static str> {
    let mut host = FakeHost {
        target_read: Some(read),
        ..FakeHost::default()
    };
    host.values
        .insert("child".into(), Data::String("#_child".into()));
    let outcome = execute("offers", attributes, &[], &mut host);
    // Every outcome, a failure included, read the target once.
    assert_eq!(host.read_targets, ["#_child"], "the target's value is read");
    assert_eq!(
        host.snapshot_requests,
        [OFFERS_SNAPSHOT_EXCLUDE.to_vec()],
        "the system, queue and pending-send sections are excluded"
    );
    match outcome {
        Ok(Executed::Handled) => {}
        Err(Failure::Execution(detail)) => {
            assert!(host.assigned.is_empty(), "a failure assigns nothing");
            return Err(detail);
        }
        other => panic!("unexpected outcome {other:?}"),
    }
    match host.assigned.as_slice() {
        [] => Ok(None),
        [(location, Data::JsonArray(offers))] => {
            assert_eq!(location, "offers");
            Ok(Some(
                serde_json::from_str(offers).expect("the offers are JSON"),
            ))
        }
        other => panic!("one offers array is assigned: {other:?}"),
    }
}

fn offered_events(offers: &Json) -> Vec<(String, Option<u64>)> {
    offers
        .as_array()
        .expect("the offers are an array")
        .iter()
        .map(|entry| {
            (
                entry["event"].as_str().expect("event").to_owned(),
                entry.get("effort").and_then(Json::as_u64),
            )
        })
        .collect()
}

/// Only the declared events are offered, with their declared efforts; a
/// guarded transition is never offered but shadows the later one for its
/// event; a schemaless transition offers nothing.
#[test]
fn offers_writes_the_declared_events_of_the_unguarded_schema_transitions() {
    let offerable = json!({
        "order.ship": {},
        "order.cancel": {"effort": 2},
        "order.note": {},
        "order.absent": {"effort": 1},
    });
    let offers = executed_offers(
        &offers_attributes(None),
        SnapshotRead::Document(offers_snapshot(Some(&offerable))),
    )
    .expect("handled")
    .expect("assigned");
    assert_eq!(
        offered_events(&offers),
        [("order.cancel".to_owned(), Some(2))],
        "ship is shadowed by its guarded transition, hold undeclared, note schemaless"
    );
    assert_eq!(
        offers,
        serde_json::from_str::<Json>(&choices_json(&[Choice {
            event: "order.cancel".into(),
            schema: cancel_schema(),
            description: Some("Cancel the order".into()),
            effort: Some(2),
            draft: None,
        }]))
        .expect("JSON"),
        "the canonical choices"
    );
}

/// A declared draft marker rides its event's choice, as declared.
#[test]
fn offers_carry_each_declared_draft_marker_into_its_choice() {
    let draft = json!({"field": "words", "event": "order.cancel.draft", "within": "parts"});
    let offerable = json!({"order.cancel": {"draft": draft}});
    let offers = executed_offers(
        &offers_attributes(None),
        SnapshotRead::Document(offers_snapshot(Some(&offerable))),
    )
    .expect("handled")
    .expect("assigned");
    assert_eq!(offers[0]["event"], "order.cancel");
    assert_eq!(offers[0]["draft"], draft);
    assert!(offers[0].get("effort").is_none());
}

/// `undeclared="all"` offers every event the walk finds, with the efforts
/// of those declared; a session that declares nothing offers nothing by
/// default.
#[test]
fn undeclared_all_offers_every_event_and_none_offers_only_declared_ones() {
    let offerable = json!({"order.hold": {"effort": 3}});
    let all = executed_offers(
        &offers_attributes(Some("all")),
        SnapshotRead::Document(offers_snapshot(Some(&offerable))),
    )
    .expect("handled")
    .expect("assigned");
    assert_eq!(
        offered_events(&all),
        [
            ("order.cancel".to_owned(), None),
            ("order.hold".to_owned(), Some(3))
        ]
    );
    let undeclared = executed_offers(
        &offers_attributes(Some("all")),
        SnapshotRead::Document(offers_snapshot(None)),
    )
    .expect("handled")
    .expect("assigned");
    assert_eq!(offered_events(&undeclared).len(), 2);
    for none in [None, Some("none")] {
        assert_eq!(
            executed_offers(
                &offers_attributes(none),
                SnapshotRead::Document(offers_snapshot(None))
            ),
            Ok(Some(json!([]))),
            "nothing declared, nothing offered"
        );
    }
}

/// A target no running session answers is nothing to read (decision
/// D109): nothing is assigned and nothing raised; a failed read or a
/// malformed document is `error.execution`; a document without its session
/// object offers nothing, as `<decision:choice>` reads one.
#[test]
fn a_target_not_running_assigns_nothing_and_a_failed_read_raises() {
    assert_eq!(
        executed_offers(&offers_attributes(None), SnapshotRead::NotRunning),
        Ok(None)
    );
    assert_eq!(
        executed_offers(&offers_attributes(None), SnapshotRead::Failed),
        Err("decision:offers cannot read the target session")
    );
    assert_eq!(
        executed_offers(
            &offers_attributes(None),
            SnapshotRead::Document("not a snapshot".into())
        ),
        Err("the session snapshot is malformed")
    );
    assert_eq!(
        executed_offers(
            &offers_attributes(None),
            SnapshotRead::Document("{}".into())
        ),
        Ok(Some(json!([])))
    );
}

/// A non-recoverable host failure of the target's read is the host's, not
/// `error.execution`, and assigns nothing.
#[test]
fn a_host_failure_of_the_read_is_passed_on() {
    let mut host = FakeHost {
        fail_snapshot_of: true,
        ..FakeHost::default()
    };
    host.values
        .insert("child".into(), Data::String("#_child".into()));
    assert_eq!(
        execute("offers", &offers_attributes(None), &[], &mut host),
        Err(Failure::Host("fatal"))
    );
    assert_eq!(host.read_targets, ["#_child"]);
    assert!(host.assigned.is_empty());
}

/// A declaration that is not an object of `{effort?, draft?}` entries is
/// `error.execution`.
#[test]
fn a_malformed_offerable_raises_error_execution() {
    for offerable in [
        json!({"order.cancel": 1}),
        json!({"order.cancel": {"effort": 1001}}),
        json!({"order.cancel": {"effort": -1}}),
        json!({"order.cancel": {"effort": 1.5}}),
        json!({"order.cancel": {"cost": 1}}),
        json!({"order.cancel": {"draft": "words"}}),
    ] {
        assert_eq!(
            executed_offers(
                &offers_attributes(None),
                SnapshotRead::Document(offers_snapshot(Some(&offerable)))
            ),
            Err(MALFORMED_OFFERABLE),
            "{offerable}"
        );
    }
    let mut document: Json = serde_json::from_str(&offers_snapshot(None)).expect("JSON");
    document["snapshot"]["sessions"][0]["data_model"] =
        json!([{"name": "offerable", "value": {"type": "json-array", "value": "[]"}}]);
    assert_eq!(
        executed_offers(
            &offers_attributes(None),
            SnapshotRead::Document(document.to_string())
        ),
        Err(MALFORMED_OFFERABLE)
    );
}

/// A malformed element raises `error.execution` and reads nothing.
#[test]
fn malformed_offers_elements_raise_error_execution() {
    let read = || SnapshotRead::Document(offers_snapshot(None));
    let cases: [(Vec<Attribute>, &str); 6] = [
        (
            vec![attr("target", "child")],
            "decision:offers requires a non-empty location",
        ),
        (
            vec![location("offers")],
            "decision:offers requires a non-empty target",
        ),
        (
            offers_attributes(Some("some")),
            "decision:offers undeclared is none or all",
        ),
        (
            vec![
                location("offers"),
                attr("target", "child"),
                attr("events", "x"),
            ],
            "decision:offers takes only the location, target and undeclared attributes",
        ),
        (
            vec![location("offers"), attr("target", "missing")],
            "decision:offers target is not a string",
        ),
        (
            vec![location("offers"), attr("target", "number")],
            "decision:offers target is not a string",
        ),
    ];
    for (attributes, detail) in cases {
        let mut host = FakeHost {
            target_read: Some(read()),
            ..FakeHost::default()
        };
        host.values
            .insert("child".into(), Data::String("#_child".into()));
        host.values.insert(
            "number".into(),
            Data::Number(scxml_types::FiniteNumber::new(1.0).expect("finite")),
        );
        assert_eq!(
            execute("offers", &attributes, &[], &mut host),
            Err(Failure::Execution(detail))
        );
        assert!(host.assigned.is_empty() && host.read_targets.is_empty());
    }
    let mut host = FakeHost::default();
    assert_eq!(
        execute(
            "offers",
            &offers_attributes(None),
            &[Content::Start, Content::End],
            &mut host
        ),
        Err(Failure::Execution("decision:offers holds no content"))
    );
}
