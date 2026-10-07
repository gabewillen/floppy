use std::collections::BTreeMap;

use serde_json::json;

use super::*;

/// A deterministic fake host: guards evaluate from a table (anything else
/// is not a boolean), the snapshot is scripted, and assignments are
/// recorded.
#[derive(Default)]
struct FakeHost {
    guards: BTreeMap<String, bool>,
    menu_snapshot: Option<String>,
    snapshot_requests: Vec<Vec<Section>>,
    assigned: Vec<(String, Data)>,
    fail_assign: bool,
}

impl MenuHost for FakeHost {
    type Error = &'static str;

    fn guard(&mut self, cond: &str) -> Result<GuardValue, &'static str> {
        if cond == "fatal" {
            return Err("fatal");
        }
        Ok(self.guards.get(cond).copied())
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
        Ok(self.menu_snapshot.clone().ok_or(()))
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

/// The order machine's menu: three schema transitions, an error handler
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
        format!("https://github.com/stateforward/nemobot/namespaces/{NAMESPACE_NAME}")
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
        "nemobot_decision.virtualized.wasm"
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
fn menu_writes_the_canonical_menu_of_the_enabled_schema_transitions() {
    let mut host = FakeHost {
        menu_snapshot: Some(order_snapshot()),
        ..FakeHost::default()
    };
    assert_eq!(
        execute("menu", &[location("menu")], &[], &mut host),
        Ok(Executed::Handled)
    );
    assert_eq!(host.snapshot_requests, [MENU_SNAPSHOT_EXCLUDE.to_vec()]);
    let expected = canonical_json(&json!([
        {"event": "order.ship", "schema": ship_schema(), "description": "Ship the order."},
        {"event": "order.cancel", "schema": cancel_schema(), "description": "Cancel the order"},
        {"event": "order.hold", "schema": hold_schema()},
    ]));
    assert_eq!(
        host.assigned,
        [("menu".to_owned(), Data::JsonArray(expected))]
    );
}

#[test]
fn menu_leaves_out_a_transition_whose_guard_is_false() {
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
        menu_snapshot: Some(snapshot.to_string()),
        guards: BTreeMap::from([("closed".to_owned(), false)]),
        ..FakeHost::default()
    };
    execute("menu", &[location("menu")], &[], &mut host).expect("handled");
    let Data::JsonArray(menu) = &host.assigned[0].1 else {
        panic!("the menu is an array");
    };
    let menu: Json = serde_json::from_str(menu).expect("JSON");
    assert_eq!(menu.as_array().map(Vec::len), Some(1));
    assert_eq!(menu[0]["event"], "order.ship");
}

#[test]
fn malformed_menu_elements_or_snapshots_raise_error_execution() {
    let order = || FakeHost {
        menu_snapshot: Some(order_snapshot()),
        ..FakeHost::default()
    };
    for (attributes, content) in [
        (vec![], vec![]),
        (vec![location(" ")], vec![]),
        (vec![location("menu"), attr("model", "m")], vec![]),
        (vec![location("menu"), location("other")], vec![]),
        (
            vec![Attribute {
                namespace_uri: Some(scxml_types::ENV_NAMESPACE_URI.into()),
                local_name: "apikey".into(),
                value: "K".into(),
            }],
            vec![],
        ),
        (vec![location("menu")], vec![Content::Text("x".into())]),
        (vec![location("menu")], vec![Content::Start, Content::End]),
    ] {
        let mut host = order();
        execution(execute("menu", &attributes, &content, &mut host));
        assert!(host.assigned.is_empty());
    }
    let mut host = FakeHost::default();
    assert_eq!(
        execution(execute("menu", &[location("menu")], &[], &mut host)),
        "the host offers no session snapshot"
    );
    let mut host = FakeHost {
        fail_assign: true,
        ..order()
    };
    execution(execute("menu", &[location("menu")], &[], &mut host));
    let mut host = FakeHost {
        menu_snapshot: Some(snapshot(&[guarded(
            with_schema(&["x"], &json!({"type": "object"})),
            "fatal",
        )])),
        ..FakeHost::default()
    };
    assert_eq!(
        execute("menu", &[location("menu")], &[], &mut host),
        Err(Failure::Host("fatal"))
    );
    let mut host = FakeHost::default();
    assert_eq!(
        execute("choose", &[], &[], &mut host),
        Ok(Executed::NotHandled),
        "the old elements are gone"
    );
}

/// The menu of `transitions` with every guard answering by its text:
/// `"yes"` true, `"no"` false, anything else not a boolean.
fn menu_events(transitions: &[Json]) -> Result<Vec<String>, Failure<()>> {
    snapshot_menu(&snapshot(transitions), &mut |cond| {
        Ok(match cond {
            "yes" => Some(true),
            "no" => Some(false),
            _ => None,
        })
    })
    .map(|menu| menu.into_iter().map(|candidate| candidate.event).collect())
}

/// `transition` with the guard `cond`.
fn guarded(mut transition: Json, cond: &str) -> Json {
    transition["has_cond"] = json!(true);
    transition["cond"] = json!(cond);
    transition
}

#[test]
fn the_snapshot_menu_follows_selection_order_and_skips_what_cannot_be_offered() {
    let schema = json!({"type": "object"});
    let events = menu_events(&[
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
        snapshot_menu(&tampered.to_string(), &mut |_| Ok::<_, ()>(Some(true)))
            .map(|menu| menu.len()),
        Err(Failure::Execution("the session snapshot is malformed"))
    );
    assert_eq!(
        snapshot_menu(r#"{"snapshot":{"root":{}}}"#, &mut |_| Ok::<_, ()>(Some(
            true
        )))
        .map(|menu| menu.len()),
        Ok(0)
    );
}

/// The menu offers only enabled transitions: a guard that is exactly
/// `true` offers, `false` neither offers nor preempts, and a failed or
/// non-boolean guard does not offer but still preempts later transitions.
#[test]
fn the_snapshot_menu_offers_only_transitions_whose_guards_hold() {
    let schema = json!({"type": "object"});
    let events = menu_events(&[
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
    snapshot_menu(
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
        menu_events(&[lying]),
        Err(Failure::Execution("the session snapshot is malformed")),
        "has_cond without its cond"
    );
    assert_eq!(
        snapshot_menu(
            &snapshot(&[guarded(with_schema(&["x"], &schema), "fatal")]),
            &mut |_| Err("fatal")
        )
        .map(|menu| menu.len()),
        Err(Failure::Host("fatal")),
        "a non-recoverable host failure passes through"
    );
}

/// The menu of `transitions` under the active atomic states `atomic`, every
/// guard enabled.
fn menu_events_in(atomic: &[&str], transitions: &[Json]) -> Vec<String> {
    snapshot_menu(&snapshot_in(atomic, transitions), &mut |_| {
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
fn the_snapshot_menu_preempts_only_within_one_atomic_states_chain() {
    let schema = json!({"type": "object"});
    // Leaf a1's chain (a1, A, P), then leaf b1's (b1, B): A's schema-less
    // `go` does not hide b1's schema'd `go`.
    let events = menu_events_in(
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
    let events = menu_events_in(
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
        menu_events(&[sourceless]),
        Err(Failure::Execution("the session snapshot is malformed"))
    );
    let unconfigured =
        json!({"snapshot": {"root": {"transitions": [with_schema(&["x"], &schema)]}}});
    assert_eq!(
        snapshot_menu(&unconfigured.to_string(), &mut |_| Ok::<_, ()>(Some(true)))
            .map(|menu| menu.len()),
        Err(Failure::Execution("the session snapshot is malformed"))
    );
}

/// The event-name bound is a wasm-only resource bound: natively a long
/// event name is offered; on wasm one over the bound is not.
#[test]
fn the_event_name_bound_applies_to_wasm_builds_only() {
    let schema = json!({"type": "object"});
    let long = "e".repeat(257);
    let offered = menu_events_in(&["s"], &[with_schema(&[long.as_str()], &schema)]);
    if cfg!(target_arch = "wasm32") {
        assert_eq!(MAX_EVENT_NAME_BYTES, 256);
        assert!(offered.is_empty());
    } else {
        assert_eq!(MAX_EVENT_NAME_BYTES, usize::MAX);
        assert_eq!(offered, std::slice::from_ref(&long));
    }
    let at_bound = "e".repeat(256);
    assert_eq!(
        menu_events_in(&["s"], &[with_schema(&[at_bound.as_str()], &schema)]),
        [at_bound],
        "a name at the wasm bound is offered on every build"
    );
}
