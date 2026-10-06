use std::collections::BTreeMap;

use serde_json::json;

use super::*;

/// A deterministic fake host: expressions evaluate from a table, snapshots
/// come from scripted documents, assignments, raises, and requests are
/// recorded, and the backend answers with a scripted response.
struct FakeHost {
    values: BTreeMap<String, Value>,
    slots: BTreeMap<String, u32>,
    menu_snapshot: Option<String>,
    state_snapshot: Option<String>,
    snapshot_requests: Vec<Vec<Section>>,
    assigned: Vec<(String, Data)>,
    raised: Vec<(String, Data)>,
    requests: Vec<Request>,
    answer: Result<Response, ()>,
    refuse_raise: bool,
}

impl Default for FakeHost {
    fn default() -> Self {
        Self {
            values: BTreeMap::new(),
            slots: BTreeMap::from([(API_KEY_SLOT.into(), 1), (BASE_URL_SLOT.into(), 2)]),
            menu_snapshot: None,
            state_snapshot: Some(
                r#"{"version":"scxml.session-snapshot/v2","snapshot":{"root":{"configuration":{"atomic":["s"],"full":["s"]}}}}"#
                    .into(),
            ),
            snapshot_requests: Vec::new(),
            assigned: Vec::new(),
            raised: Vec::new(),
            requests: Vec::new(),
            answer: Err(()),
            refuse_raise: false,
        }
    }
}

impl FakeHost {
    fn answering(answers: &Json) -> Self {
        Self {
            answer: Ok(Response {
                status: 200,
                body: system_one(answers),
            }),
            ..Self::default()
        }
    }

    fn body(&self) -> Json {
        assert_eq!(self.requests.len(), 1, "exactly one request");
        serde_json::from_slice(&self.requests[0].body).expect("the body is JSON")
    }

    fn raised_json(&self) -> Vec<(String, Json)> {
        self.raised
            .iter()
            .map(|(name, data)| match data {
                Data::JsonObject(text) => (
                    name.clone(),
                    serde_json::from_str(text).expect("raised data is JSON"),
                ),
                other => panic!("raised data must be a JSON object, got {other:?}"),
            })
            .collect()
    }
}

impl DecisionHost for FakeHost {
    type Error = &'static str;

    fn evaluate(&mut self, expression: &str) -> Result<Result<Value, ()>, &'static str> {
        if expression == "fatal" {
            return Err("fatal");
        }
        Ok(self.values.get(expression).cloned().ok_or(()))
    }

    fn assign(&mut self, location: &str, value: &Data) -> Result<Result<(), ()>, &'static str> {
        self.assigned.push((location.to_owned(), value.clone()));
        Ok(Ok(()))
    }

    fn slot(&mut self, slot: &str) -> Option<u32> {
        self.slots.get(slot).copied()
    }

    fn post(&mut self, request: &Request) -> Result<Result<Response, ()>, &'static str> {
        self.requests.push(request.clone());
        Ok(self.answer.clone())
    }

    fn snapshot(&mut self, exclude: &[Section]) -> Result<Result<String, ()>, &'static str> {
        self.snapshot_requests.push(exclude.to_vec());
        let document = if exclude.contains(&Section::Transitions) {
            &self.state_snapshot
        } else {
            &self.menu_snapshot
        };
        Ok(document.clone().ok_or(()))
    }

    fn raise(&mut self, name: &str, data: &Data) -> Result<Result<(), ()>, &'static str> {
        if self.refuse_raise {
            return Ok(Err(()));
        }
        self.raised.push((name.to_owned(), data.clone()));
        Ok(Ok(()))
    }
}

/// A `system_one` response body answering each question
/// (`{name: {label: probability}}`) with its most probable label as the
/// reported `choice`.
fn system_one(answers: &Json) -> Vec<u8> {
    let answers: Map<String, Json> = answers
        .as_object()
        .expect("answers are an object")
        .iter()
        .map(|(name, probabilities)| {
            let choice = probabilities
                .as_object()
                .expect("probabilities are an object")
                .iter()
                .max_by(|left, right| {
                    left.1
                        .as_f64()
                        .unwrap_or(0.0)
                        .total_cmp(&right.1.as_f64().unwrap_or(0.0))
                })
                .map(|(label, _)| label.clone())
                .expect("at least one label");
            (
                name.clone(),
                json!({"type": "choice", "choice": choice, "confidence": 0.9, "probabilities": probabilities}),
            )
        })
        .collect();
    json!({"model": "jev-latest", "answers": answers, "usage": {"input_tokens": 1, "output_tokens": 1}})
        .to_string()
        .into_bytes()
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

fn env(local_name: &str, value: &str) -> Attribute {
    Attribute {
        namespace_uri: Some(scxml_types::ENV_NAMESPACE_URI.into()),
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

fn order_host(answers: &Json) -> FakeHost {
    let mut host = FakeHost::answering(answers);
    host.menu_snapshot = Some(order_snapshot());
    host.values.insert(
        "order".into(),
        Value::Json(r#"{"sku":"A-1","qty":2}"#.into()),
    );
    host
}

fn choose_attributes() -> Vec<Attribute> {
    vec![attr("dataexpr", "order"), env("apikey", "TYPESAFE_API_KEY")]
}

#[test]
fn the_namespace_uri_is_the_repository_path_of_its_name_at_its_tag() {
    assert_eq!(
        NAMESPACE_URI,
        format!("{NAMESPACE_URI_BASE}@{NAMESPACE_TAG}")
    );
    assert_eq!(NAMESPACE_URI_BASE.rsplit('/').next(), Some(NAMESPACE_NAME));
}

#[test]
fn the_package_binds_the_exact_uri_schema_digest_slots_origin_and_abstain_contract() {
    let manifest = scxml_validator_core::ExtensionPackageManifest::from_json(include_bytes!(
        "../package/decision.extension.json"
    ))
    .expect("the manifest is a valid extension package manifest");
    assert_eq!(manifest.namespace_uri, NAMESPACE_URI);
    assert_eq!(
        manifest.extension_artifact,
        "nemobot_decision.virtualized.wasm"
    );
    assert_eq!(manifest.http_origins, ["https://api.typesafe.ai"]);
    assert_eq!(manifest.env_slots, [API_KEY_SLOT, BASE_URL_SLOT]);
    assert!(manifest.wasi_permissions.is_empty(), "no WASI permission");
    manifest
        .verify_schema(&scxml_validator_core::SchemaSource::new(
            "decision.xsd",
            include_bytes!("../package/decision.xsd").to_vec(),
        ))
        .expect("the schema digest, targetNamespace, and WIT identity verify");
    let emits: Vec<&str> = manifest
        .events
        .emits
        .iter()
        .map(|event| event.name.as_str())
        .collect();
    assert_eq!(emits, [ABSTAIN_EVENT]);
    let files = BTreeMap::from([(
        "events.json".to_owned(),
        include_bytes!("../package/events.json").to_vec(),
    )]);
    let declared = manifest
        .events
        .resolve("extension package", &files)
        .expect("the declared abstain schema resolves against its digest");
    let abstain = CompiledSchema::new(&declared.emits[0].schema).expect("the schema compiles");
    for data in [
        json!({"reason": "no-candidates"}),
        json!({"reason": "declined", "confidence": 0.5}),
        json!({"reason": "below-threshold", "confidence": 0.1}),
    ] {
        assert!(
            abstain.admits(&Data::JsonObject(canonical_json(&data))),
            "every abstain the element raises satisfies the declared contract: {data}"
        );
    }
}

#[test]
fn choose_offers_the_schema_transitions_fills_grounded_data_and_raises_the_event() {
    let answers = json!({
        "selection": {"order.ship": 0.7, "order.cancel": 0.2, "order.hold": 0.05, "none of these": 0.05},
        "fill.0.0": {"A-1": 1.0},
        "fill.0.1": {"ups": 0.4, "fedex": 0.6},
        "fill.1.0": {"customer": 0.5, "stock": 0.5},
    });
    let mut host = order_host(&answers);
    assert_eq!(
        execute("choose", &choose_attributes(), &[], &mut host),
        Ok(Executed::Handled)
    );
    assert_eq!(
        host.raised_json(),
        [(
            "order.ship".to_owned(),
            json!({"carrier": "fedex", "sku": "A-1"})
        )],
        "the chosen event is raised with schema-valid grounded data"
    );
    assert_eq!(
        host.snapshot_requests,
        [
            STATE_SNAPSHOT_EXCLUDE.to_vec(),
            MENU_SNAPSHOT_EXCLUDE.to_vec()
        ],
        "the default state and the menu each read one snapshot"
    );
    let request = &host.requests[0];
    assert_eq!(
        request.url,
        [
            Part::Reference(2),
            Part::Literal(TypeSafe::SYSTEM_ONE_PATH.into())
        ],
        "the baseurl slot's reference, then the path; no URL is compiled in"
    );
    assert_eq!(
        request.headers[0],
        (
            "authorization".to_owned(),
            vec![Part::Literal("Bearer ".into()), Part::Reference(1)]
        )
    );
    let body = String::from_utf8(request.body.clone()).expect("UTF-8");
    let expected = canonical_json(&json!({
        "model": "jev-latest",
        "questions": {
            "selection": {
                "type": "choice",
                "instructions": CHOOSE_INSTRUCTIONS,
                "criteria": {
                    "order.ship": "Ship the order.",
                    "order.cancel": "Cancel the order",
                    "order.hold": null,
                    "none of these": ABSTAIN_DESCRIPTION,
                },
            },
            "fill.0.0": {
                "type": "choice",
                "instructions": format!("Which value fills \"sku\" for the event order.ship? {FILL_INSTRUCTIONS}"),
                "criteria": {"A-1": null},
            },
            "fill.0.1": {
                "type": "choice",
                "instructions": format!("Which value fills \"carrier\" for the event order.ship? {FILL_INSTRUCTIONS}"),
                "criteria": {"ups": null, "fedex": null},
            },
            "fill.1.0": {
                "type": "choice",
                "instructions": format!("Which value fills \"reason\" for the event order.cancel? {FILL_INSTRUCTIONS}"),
                "criteria": {"customer": null, "stock": null},
            },
        },
        "state": {"configuration": {"atomic": ["s"], "full": ["s"]}},
    }));
    assert_eq!(body, expected, "the request body is canonical JSON");
}

#[test]
fn choose_breaks_ties_by_label_bytes_whatever_the_reported_choice() {
    // order.cancel and order.hold tie; "order.cancel" < "order.hold". The
    // reason members tie too; "customer" < "stock".
    let answers = json!({
        "selection": {"order.ship": 0.1, "order.cancel": 0.4, "order.hold": 0.4, "none of these": 0.1},
        "fill.0.0": {"A-1": 1.0},
        "fill.0.1": {"ups": 0.5, "fedex": 0.5},
        "fill.1.0": {"stock": 0.5, "customer": 0.5},
    });
    let mut host = order_host(&answers);
    assert_eq!(
        execute("choose", &choose_attributes(), &[], &mut host),
        Ok(Executed::Handled)
    );
    assert_eq!(
        host.raised_json(),
        [("order.cancel".to_owned(), json!({"reason": "customer"}))]
    );
}

#[test]
fn choose_raises_the_abstain_event_when_none_fits_or_below_threshold() {
    let declined = json!({
        "selection": {"order.ship": 0.1, "order.cancel": 0.1, "order.hold": 0.1, "none of these": 0.7},
        "fill.0.0": {"A-1": 1.0},
        "fill.0.1": {"ups": 1.0},
        "fill.1.0": {"stock": 1.0},
    });
    let mut host = order_host(&declined);
    assert_eq!(
        execute("choose", &choose_attributes(), &[], &mut host),
        Ok(Executed::Handled)
    );
    assert_eq!(
        host.raised_json(),
        [(
            ABSTAIN_EVENT.to_owned(),
            json!({"reason": "declined", "confidence": 0.7})
        )]
    );

    let weak = json!({
        "selection": {"order.ship": 0.4, "order.cancel": 0.3, "order.hold": 0.2, "none of these": 0.1},
        "fill.0.0": {"A-1": 1.0},
        "fill.0.1": {"ups": 1.0},
        "fill.1.0": {"stock": 1.0},
    });
    let mut host = order_host(&weak);
    let mut attributes = choose_attributes();
    attributes.push(attr("threshold", "0.5"));
    assert_eq!(
        execute("choose", &attributes, &[], &mut host),
        Ok(Executed::Handled)
    );
    assert_eq!(
        host.raised_json(),
        [(
            ABSTAIN_EVENT.to_owned(),
            json!({"reason": "below-threshold", "confidence": 0.4})
        )]
    );
}

#[test]
fn an_empty_menu_abstains_without_a_request_or_a_key() {
    let mut host = FakeHost::default();
    host.slots.clear();
    host.menu_snapshot = Some(snapshot(&[
        without_schema(&["go"]),
        // order.ship needs a sku nothing grounds: not offered.
        with_schema(&["order.ship"], &ship_schema()),
    ]));
    assert_eq!(
        execute("choose", &[], &[], &mut host),
        Ok(Executed::Handled)
    );
    assert_eq!(
        host.raised_json(),
        [(ABSTAIN_EVENT.to_owned(), json!({"reason": "no-candidates"}))]
    );
    assert!(
        host.requests.is_empty(),
        "nothing is sent for an empty menu"
    );
}

#[test]
fn filled_data_that_fails_the_schema_is_never_raised() {
    let schema = json!({
        "type": "object",
        "properties": {"sku": {"type": "string", "pattern": "^B-"}},
        "required": ["sku"],
    });
    let mut host = FakeHost::answering(&json!({
        "selection": {"order.ship": 0.9, "none of these": 0.1},
        "fill.0.0": {"A-1": 1.0},
    }));
    host.menu_snapshot = Some(snapshot(&[with_schema(&["order.ship"], &schema)]));
    host.values
        .insert("order".into(), Value::Json(r#"{"sku":"A-1"}"#.into()));
    assert_eq!(
        execution(execute("choose", &choose_attributes(), &[], &mut host)),
        "the filled data does not satisfy the chosen event's schema; nothing was raised"
    );
    assert!(host.raised.is_empty());
}

#[test]
fn an_answer_outside_the_menu_or_a_failed_request_raises_nothing() {
    let mut host = order_host(&json!({
        "selection": {"order.ship": 0.9, "order.refund": 0.1},
        "fill.0.0": {"A-1": 1.0},
        "fill.0.1": {"ups": 1.0},
        "fill.1.0": {"stock": 1.0},
    }));
    assert_eq!(
        execution(execute("choose", &choose_attributes(), &[], &mut host)),
        "the decision response gives a probability to a label the question does not offer"
    );
    assert!(host.raised.is_empty());

    let mut host = order_host(&json!({"selection": {"order.ship": 1.0}}));
    assert_eq!(
        execution(execute("choose", &choose_attributes(), &[], &mut host)),
        "the decision response does not answer exactly the questions asked"
    );

    let mut host = order_host(&json!({}));
    host.answer = Ok(Response {
        status: 500,
        body: b"{}".to_vec(),
    });
    assert_eq!(
        execution(execute("choose", &choose_attributes(), &[], &mut host)),
        "the decision service answered with a non-success status"
    );
    assert!(host.raised.is_empty());

    let mut host = order_host(&json!({}));
    host.answer = Err(());
    assert_eq!(
        execution(execute("choose", &choose_attributes(), &[], &mut host)),
        "the decision request was refused or failed"
    );
}

#[test]
fn choose_without_a_key_sends_nothing() {
    let mut host = order_host(&json!({}));
    host.slots.clear();
    assert_eq!(
        execution(execute("choose", &choose_attributes(), &[], &mut host)),
        "decision has no API key: env:apikey names no variable the run holds"
    );
    assert!(host.requests.is_empty());
}

/// No backend URL is compiled in: without a `baseurl` value the element
/// fails before any request, so the key never reaches a host the component
/// chose.
#[test]
fn choose_without_a_base_url_sends_nothing() {
    let mut host = order_host(&json!({}));
    host.slots.remove(BASE_URL_SLOT);
    assert_eq!(
        execution(execute("choose", &choose_attributes(), &[], &mut host)),
        "decision has no base URL: env:baseurl names no variable the run holds"
    );
    assert!(host.requests.is_empty());
}

#[test]
fn a_base_url_slot_and_a_model_reach_the_request() {
    let mut host = FakeHost::answering(&json!({"classify": {"a": 0.25, "b": 0.75}}));
    host.slots.insert(BASE_URL_SLOT.into(), 7);
    let attributes = [
        attr("location", "kind"),
        attr("labels", " a\n b "),
        attr("model", "jev-1"),
        attr("statexpr", "'hello'"),
    ];
    host.values
        .insert("'hello'".into(), Value::Text("hello".into()));
    assert_eq!(
        execute("classify", &attributes, &[], &mut host),
        Ok(Executed::Handled)
    );
    assert_eq!(
        host.requests[0].url,
        [
            Part::Reference(7),
            Part::Literal(TypeSafe::SYSTEM_ONE_PATH.into())
        ]
    );
    assert_eq!(host.body()["model"], "jev-1");
    assert_eq!(host.body()["state"], "hello");
    assert!(
        host.snapshot_requests.is_empty(),
        "statexpr reads no snapshot"
    );
    assert_eq!(
        host.assigned,
        [(
            "kind".to_owned(),
            Data::JsonObject(r#"{"a":0.25,"b":0.75}"#.into())
        )]
    );
}

#[test]
fn classify_takes_described_labels_from_labelsexpr() {
    let mut host = FakeHost::answering(&json!({"classify": {"spam": 0.9}}));
    host.values.insert(
        "labels".into(),
        Value::Json(r#"{"spam":"Unwanted mail","ham":{"kind":"wanted"},"other":null}"#.into()),
    );
    assert_eq!(
        execute(
            "classify",
            &[attr("location", "kind"), attr("labelsexpr", "labels")],
            &[],
            &mut host
        ),
        Ok(Executed::Handled)
    );
    assert_eq!(
        host.body()["questions"]["classify"]["criteria"],
        json!({"spam": "Unwanted mail", "ham": {"kind": "wanted"}, "other": null})
    );
    assert_eq!(
        host.assigned[0].1,
        Data::JsonObject(r#"{"ham":0.0,"other":0.0,"spam":0.9}"#.into()),
        "every label is in the distribution; unmentioned labels are 0"
    );
}

#[test]
fn rank_sorts_by_score_then_label_and_keeps_every_candidate() {
    let mut host = FakeHost::answering(&json!({
        "rank": {"pear": 0.3, "apple": 0.3, "{\"id\":7}": 0.4},
    }));
    host.values.insert(
        "fruit".into(),
        Value::Json(r#"["pear","apple",{"id":7},"fig"]"#.into()),
    );
    assert_eq!(
        execute(
            "rank",
            &[attr("location", "ranked"), attr("candidatesexpr", "fruit")],
            &[],
            &mut host
        ),
        Ok(Executed::Handled)
    );
    assert_eq!(
        host.body()["questions"]["rank"]["criteria"],
        json!({"pear": null, "apple": null, "{\"id\":7}": {"id": 7}, "fig": null})
    );
    assert_eq!(
        host.assigned,
        [(
            "ranked".to_owned(),
            Data::JsonArray(
                r#"[{"candidate":{"id":7},"score":0.4},{"candidate":"apple","score":0.3},{"candidate":"pear","score":0.3},{"candidate":"fig","score":0.0}]"#
                    .into()
            )
        )]
    );
}

#[test]
fn rank_refuses_duplicate_or_empty_candidates() {
    for candidates in ["[]", r#"["a","a"]"#, r#"[1,"1"]"#, r#"[""]"#] {
        let mut host = FakeHost::default();
        host.values
            .insert("c".into(), Value::Json(candidates.into()));
        assert_eq!(
            execution(execute(
                "rank",
                &[attr("location", "r"), attr("candidatesexpr", "c")],
                &[],
                &mut host
            )),
            "decision:rank candidatesexpr must evaluate to a non-empty array of distinct candidates",
            "{candidates}"
        );
        assert!(host.requests.is_empty());
    }
}

#[test]
fn malformed_elements_are_refused_before_anything_is_sent() {
    let cases: [(&str, Vec<Attribute>, Vec<Content>); 6] = [
        ("choose", vec![attr("bogus", "x")], vec![]),
        (
            "choose",
            vec![attr("model", "a"), attr("modelexpr", "b")],
            vec![],
        ),
        ("choose", vec![attr("threshold", "1.5")], vec![]),
        ("choose", vec![], vec![Content::Start, Content::End]),
        ("rank", vec![attr("candidatesexpr", "c")], vec![]),
        ("classify", vec![attr("location", "l")], vec![]),
    ];
    for (name, attributes, content) in cases {
        let mut host = order_host(&json!({}));
        assert!(
            matches!(
                execute(name, &attributes, &content, &mut host),
                Err(Failure::Execution(_))
            ),
            "{name} {attributes:?}"
        );
        assert!(host.requests.is_empty() && host.raised.is_empty());
    }
    let mut host = FakeHost::default();
    assert_eq!(
        execute("decide", &[], &[], &mut host),
        Ok(Executed::NotHandled)
    );
    assert_eq!(
        execute(
            "rank",
            &[attr("location", "l"), attr("candidatesexpr", "fatal")],
            &[],
            &mut host
        ),
        Err(Failure::Host("fatal")),
        "a non-recoverable host failure passes through"
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

/// `choose` evaluates each offered transition's guard through the host.
#[test]
fn choose_skips_a_transition_whose_guard_is_false() {
    let mut host = order_host(&json!({
        "selection": {"order.ship": 0.8, "none of these": 0.2},
    }));
    let guarded_cancel = {
        let mut snapshot: Json =
            serde_json::from_str(&order_snapshot()).expect("the order snapshot is JSON");
        let transitions = snapshot["snapshot"]["root"]["transitions"]
            .as_array_mut()
            .expect("transitions");
        for transition in transitions.iter_mut() {
            if transition["events"] != json!(["order.ship"]) {
                transition["has_cond"] = json!(true);
                transition["cond"] = json!("closed");
            }
        }
        snapshot.to_string()
    };
    host.menu_snapshot = Some(guarded_cancel);
    host.values.insert("closed".into(), Value::Bool(false));
    let _ = execute("choose", &choose_attributes(), &[], &mut host);
    let offered: Vec<String> = host.body()["questions"]["selection"]["criteria"]
        .as_object()
        .expect("the selection question")
        .keys()
        .cloned()
        .collect();
    assert!(offered.contains(&"order.ship".to_owned()), "{offered:?}");
    assert_eq!(
        offered.len(),
        2,
        "only order.ship and the abstain label: {offered:?}"
    );
}

#[test]
fn explicit_candidates_replace_the_snapshot_menu() {
    let mut host = FakeHost::answering(&json!({
        "selection": {"greet": 0.8, "none of these": 0.2},
        "fill.0.0": {"en": 0.2, "fr": 0.8},
    }));
    host.values.insert(
        "menu".into(),
        Value::Json(
            r#"[{"event":"greet","description":"Say hello","schema":{"type":"object","properties":{"lang":{"enum":["en","fr"]}},"required":["lang"]}}]"#
                .into(),
        ),
    );
    assert_eq!(
        execute(
            "choose",
            &[attr("candidatesexpr", "menu"), env("apikey", "K")],
            &[],
            &mut host
        ),
        Ok(Executed::Handled)
    );
    assert_eq!(
        host.raised_json(),
        [("greet".to_owned(), json!({"lang": "fr"}))]
    );
    assert_eq!(
        host.snapshot_requests,
        [STATE_SNAPSHOT_EXCLUDE.to_vec()],
        "explicit candidates read no menu snapshot"
    );

    for menu in [
        r#"[{"event":"a b","schema":true}]"#,
        r#"[{"event":"x","schema":true},{"event":"x","schema":true}]"#,
        r#"[{"event":"x","schema":{"$ref":"https://example.org/s.json"}}]"#,
        r#"[{"event":"x","schema":true,"extra":1}]"#,
    ] {
        assert!(
            explicit_menu(&serde_json::from_str(menu).expect("JSON")).is_err(),
            "{menu}"
        );
    }
}

#[test]
fn a_refused_raise_is_error_execution() {
    let mut host = order_host(&json!({
        "selection": {"order.hold": 0.9, "none of these": 0.1},
        "fill.0.0": {"A-1": 1.0},
        "fill.0.1": {"ups": 1.0},
        "fill.1.0": {"stock": 1.0},
    }));
    host.refuse_raise = true;
    assert_eq!(
        execution(execute("choose", &choose_attributes(), &[], &mut host)),
        "decision:choose could not raise its event"
    );
}

/// A backend that is not HTTP-shaped like `TypeSafe`: it answers every
/// question with a uniform distribution, so every decision falls to the
/// deterministic tie-break.
struct Uniform;

impl Backend for Uniform {
    fn request(&self, _batch: &Batch, api_key: u32, _base_url: u32) -> Request {
        Request {
            url: vec![Part::Literal("http://127.0.0.1/uniform".into())],
            headers: vec![("x-key".into(), vec![Part::Reference(api_key)])],
            body: Vec::new(),
        }
    }

    fn answers(&self, batch: &Batch, _body: &[u8]) -> Result<Vec<Distribution>, &'static str> {
        Ok(batch
            .questions
            .iter()
            .map(|question| Distribution {
                probabilities: vec![0.5; question.criteria.len()],
            })
            .collect())
    }
}

#[test]
fn another_backend_plugs_in_behind_the_trait() {
    let mut host = order_host(&json!({}));
    host.answer = Ok(Response {
        status: 200,
        body: Vec::new(),
    });
    assert_eq!(
        execute_with(&Uniform, "choose", &choose_attributes(), &[], &mut host),
        Ok(Executed::Handled)
    );
    // "none of these" < "order.cancel" < "order.hold" < "order.ship".
    assert_eq!(
        host.raised_json(),
        [(
            ABSTAIN_EVENT.to_owned(),
            json!({"reason": "declined", "confidence": 0.5})
        )]
    );
}

#[test]
fn argmax_prefers_probability_then_the_smallest_label() {
    let criteria: Vec<Criterion> = ["b", "a", "c"]
        .iter()
        .map(|label| Criterion {
            label: (*label).into(),
            description: Json::Null,
        })
        .collect();
    let pick =
        |probabilities: Vec<f64>| backend::argmax(&criteria, &Distribution { probabilities });
    assert_eq!(pick(vec![0.2, 0.2, 0.6]), Some(2));
    assert_eq!(pick(vec![0.4, 0.4, 0.2]), Some(1));
    assert_eq!(pick(vec![0.0, 0.0, 0.0]), Some(1));
    assert_eq!(
        backend::argmax(
            &[],
            &Distribution {
                probabilities: vec![]
            }
        ),
        None
    );
}

#[test]
fn malformed_system_one_answers_fail_closed() {
    let batch = Batch {
        model: None,
        state: json!({}),
        questions: vec![Question {
            name: "q".into(),
            instructions: "i".into(),
            criteria: vec![Criterion {
                label: "a".into(),
                description: Json::Null,
            }],
        }],
    };
    for body in [
        "not json",
        r#"{"answers":[]}"#,
        r#"{"answers":{"q":{"type":"score","choice":"a","probabilities":{"a":1}}}}"#,
        r#"{"answers":{"q":{"type":"choice","choice":"z","probabilities":{"a":1}}}}"#,
        r#"{"answers":{"q":{"type":"choice","choice":"a","probabilities":{"a":1.5}}}}"#,
        r#"{"answers":{"q":{"type":"choice","choice":"a","probabilities":{"a":1}},"r":{}}}"#,
    ] {
        assert!(TypeSafe.answers(&batch, body.as_bytes()).is_err(), "{body}");
    }
    assert_eq!(
        TypeSafe.answers(
            &batch,
            br#"{"model":"m","answers":{"q":{"type":"choice","choice":"a","confidence":1,"probabilities":{"a":1}}},"usage":{}}"#
        ),
        Ok(vec![Distribution {
            probabilities: vec![1.0]
        }])
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
/// event name is offered and accepted; on wasm one over the bound is not.
#[test]
fn the_event_name_bound_applies_to_wasm_builds_only() {
    let schema = json!({"type": "object"});
    let long = "e".repeat(257);
    let offered = menu_events_in(&["s"], &[with_schema(&[long.as_str()], &schema)]);
    let explicit = explicit_menu(&json!([{"event": long, "schema": true}]));
    if cfg!(target_arch = "wasm32") {
        assert_eq!(MAX_EVENT_NAME_BYTES, 256);
        assert!(offered.is_empty());
        assert!(explicit.is_err());
    } else {
        assert_eq!(MAX_EVENT_NAME_BYTES, usize::MAX);
        assert_eq!(offered, [long.clone()]);
        assert_eq!(explicit.map(|menu| menu.len()), Ok(1));
    }
    let at_bound = "e".repeat(256);
    assert_eq!(
        menu_events_in(&["s"], &[with_schema(&[at_bound.as_str()], &schema)]),
        [at_bound],
        "a name at the wasm bound is offered on every build"
    );
}
