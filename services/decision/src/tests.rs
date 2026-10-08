use serde_json::json;

use super::*;

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

#[expect(
    clippy::unnecessary_wraps,
    reason = "builds the answered arm of a transport outcome for `decide`"
)]
fn ok(answers: &Json) -> Result<Response, TransportFailure> {
    Ok(Response {
        status: 200,
        body: system_one(answers),
    })
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

/// The order machine's choices, as `<decision:choice>` writes it.
fn order_choices() -> Data {
    Data::JsonArray(canonical_json(&json!([
        {"event": "order.ship", "schema": ship_schema(), "description": "Ship the order."},
        {"event": "order.cancel", "schema": cancel_schema(), "description": "Cancel the order"},
        {"event": "order.hold", "schema": {"type": "object"}},
    ])))
}

fn order_state() -> Data {
    Data::JsonObject(r#"{"qty":2,"sku":"A-1"}"#.into())
}

fn prepared(parameters: &[(&str, Data)]) -> Prepared {
    match parse_parameters(parameters.iter().map(|(name, value)| (*name, value)))
        .expect("valid parameters")
        .plan()
        .expect("within bounds")
    {
        Plan::Ask(prepared) => prepared,
        Plan::Return(returned) => panic!("expected a question, got {returned:?}"),
    }
}

fn choose() -> Prepared {
    prepared(&[("choices", order_choices()), ("state", order_state())])
}

#[test]
fn the_service_uri_is_the_tagless_repository_path_of_its_name() {
    assert_eq!(
        SERVICE_URI,
        format!("https://github.com/stateforward/floppy/services/{SERVICE_NAME}")
    );
}

#[test]
fn the_package_binds_the_exact_uri_slots_and_origin() {
    let manifest = scxml_validator_core::ServicePackageManifest::from_json(include_bytes!(
        "../package/decision.service.json"
    ))
    .expect("the manifest parses");
    manifest.verify(None).expect("the manifest verifies");
    assert_eq!(manifest.service_uri, SERVICE_URI);
    assert_eq!(
        manifest.service_artifact,
        "floppy_service_decision.virtualized.wasm"
    );
    assert_eq!(manifest.env_slots, [API_KEY_SLOT, BASE_URL_SLOT]);
    assert_eq!(manifest.http_origins, ["https://api.typesafe.ai"]);
}

/// The builder's `event:schema` for this service's own events
/// (`bots/builder/schemas`), which the invoking document declares.
#[test]
fn every_failure_code_and_abstain_reason_satisfy_the_builder_event_schemas() {
    let compile = |text: &str| {
        let value: serde_json::Value = serde_json::from_str(text).expect("the schema is JSON");
        scxml_event_schema::CompiledSchema::new(&scxml_event_schema::canonical_json(&value))
            .expect("the schema compiles")
    };
    let failed = compile(include_str!(
        "../../../bots/builder/schemas/decision.failed.json"
    ));
    for code in [
        FailureCode::Unconfigured,
        FailureCode::Transport,
        FailureCode::Status,
        FailureCode::Malformed,
        FailureCode::SchemaMismatch,
    ] {
        assert!(
            failed.admits(&scxml_types::DataValue::JsonObject(
                Returned::failed(code).data
            )),
            "{}",
            code.name()
        );
    }
    let abstain = compile(include_str!(
        "../../../bots/builder/schemas/decision.abstain.json"
    ));
    for data in [
        r#"{"reason":"no-candidates"}"#,
        r#"{"confidence":0.25,"reason":"declined"}"#,
        r#"{"confidence":0.5,"reason":"below-threshold"}"#,
    ] {
        assert!(
            abstain.admits(&scxml_types::DataValue::JsonObject(data.into())),
            "{data}"
        );
    }
}

#[test]
fn choose_asks_the_selection_and_the_grounded_members_in_one_batch() {
    let prepared = choose();
    let batch = prepared.batch();
    let names: Vec<&str> = batch
        .questions
        .iter()
        .map(|question| question.name.as_str())
        .collect();
    // order.ship: sku (evidence), carrier (enum); order.cancel: reason
    // (enum); order.hold: nothing required.
    assert_eq!(names, ["selection", "fill.0.0", "fill.0.1", "fill.1.0"]);
    let selection: Vec<&str> = batch.questions[0]
        .criteria
        .iter()
        .map(|criterion| criterion.label.as_str())
        .collect();
    assert_eq!(
        selection,
        ["order.ship", "order.cancel", "order.hold", ABSTAIN_LABEL]
    );
    assert_eq!(
        batch.questions[0].criteria[0].description,
        "Ship the order."
    );
    assert_eq!(batch.state, json!({"qty": 2, "sku": "A-1"}));
    let request = prepared.request(&TypeSafe, 1, Part::Reference(2));
    assert_eq!(
        request.url,
        [
            Part::Reference(2),
            Part::Literal(TypeSafe::SYSTEM_ONE_PATH.into())
        ]
    );
    let body: Json = serde_json::from_slice(&request.body).expect("JSON");
    assert_eq!(body["model"], TypeSafe::DEFAULT_MODEL);
    assert_eq!(
        body["questions"]["fill.0.1"]["criteria"],
        json!({"fedex": null, "ups": null})
    );
}

#[test]
fn choose_returns_the_chosen_event_with_grounded_data_validated_against_its_schema() {
    let returned = choose().decide(
        &TypeSafe,
        ok(&json!({
            "selection": {"order.ship": 0.7, "order.cancel": 0.1, "order.hold": 0.1, "none of these": 0.1},
            "fill.0.0": {"A-1": 1.0},
            "fill.0.1": {"ups": 0.2, "fedex": 0.8},
            "fill.1.0": {"customer": 0.5, "stock": 0.5},
        })),
    );
    assert_eq!(
        returned,
        Returned {
            name: "order.ship".into(),
            data: r#"{"carrier":"fedex","sku":"A-1"}"#.into()
        }
    );
}

#[test]
fn choose_breaks_ties_by_label_bytes() {
    let returned = choose().decide(
        &TypeSafe,
        ok(&json!({
            "selection": {"order.ship": 0.4, "order.cancel": 0.4, "order.hold": 0.1, "none of these": 0.1},
            "fill.0.0": {"A-1": 1.0},
            "fill.0.1": {"ups": 0.5, "fedex": 0.5},
            "fill.1.0": {"customer": 0.5, "stock": 0.5},
        })),
    );
    assert_eq!(returned.name, "order.cancel");
    assert_eq!(returned.data, r#"{"reason":"customer"}"#);
}

#[test]
fn choose_abstains_when_declined_below_threshold_or_with_nothing_to_offer() {
    let declined = choose().decide(
        &TypeSafe,
        ok(&json!({
            "selection": {"none of these": 0.9, "order.ship": 0.1},
            "fill.0.0": {"A-1": 1.0},
            "fill.0.1": {"ups": 1.0},
            "fill.1.0": {"stock": 1.0},
        })),
    );
    assert_eq!(declined.name, ABSTAIN_EVENT);
    assert_eq!(declined.data, r#"{"confidence":0.9,"reason":"declined"}"#);
    let strict = prepared(&[
        ("choices", order_choices()),
        ("state", order_state()),
        (
            "threshold",
            Data::Number(scxml_types::FiniteNumber::new(0.8).expect("finite")),
        ),
    ]);
    let below = strict.decide(
        &TypeSafe,
        ok(&json!({
            "selection": {"order.hold": 0.6, "none of these": 0.4},
            "fill.0.0": {"A-1": 1.0},
            "fill.0.1": {"ups": 1.0},
            "fill.1.0": {"stock": 1.0},
        })),
    );
    assert_eq!(
        below.data,
        r#"{"confidence":0.6,"reason":"below-threshold"}"#
    );
    // A choices whose every event lacks a grounded required member offers
    // nothing: it abstains without a request.
    let ungroundable = Data::JsonArray(canonical_json(&json!([
        {"event": "order.ship", "schema": ship_schema()},
    ])));
    let plan = parse_parameters([
        ("choices", &ungroundable),
        ("state", &Data::String("no evidence".into())),
    ])
    .expect("valid")
    .plan()
    .expect("within bounds");
    match plan {
        Plan::Return(returned) => assert_eq!(
            returned,
            Returned {
                name: ABSTAIN_EVENT.into(),
                data: r#"{"reason":"no-candidates"}"#.into()
            }
        ),
        Plan::Ask(_) => panic!("an empty choice list asks nothing"),
    }
}

#[test]
fn filled_data_that_fails_the_schema_is_never_returned() {
    // The evidence offers a carrier the schema's enum does not admit, and
    // the backend picks it.
    let prepared = prepared(&[
        ("choices", order_choices()),
        ("state", order_state()),
        (
            "data",
            Data::JsonObject(r#"{"carrier":"dhl","sku":"A-1"}"#.into()),
        ),
    ]);
    let returned = prepared.decide(
        &TypeSafe,
        ok(&json!({
            "selection": {"order.ship": 1.0},
            "fill.0.0": {"A-1": 1.0},
            "fill.0.1": {"dhl": 0.9, "ups": 0.05, "fedex": 0.05},
            "fill.1.0": {"stock": 1.0},
        })),
    );
    assert_eq!(returned, Returned::failed(FailureCode::SchemaMismatch));
}

#[test]
fn a_failed_request_or_a_malformed_answer_returns_its_failure_code() {
    let prepared = choose();
    assert_eq!(
        prepared.decide(&TypeSafe, Err(TransportFailure::Unavailable)),
        Returned::failed(FailureCode::Transport)
    );
    assert_eq!(
        prepared.decide(&TypeSafe, Err(TransportFailure::TooLarge)),
        Returned::failed(FailureCode::Malformed)
    );
    assert_eq!(
        prepared.decide(
            &TypeSafe,
            Ok(Response {
                status: 401,
                body: Vec::new()
            })
        ),
        Returned::failed(FailureCode::Status)
    );
    // An answer naming a label the question does not offer.
    assert_eq!(
        prepared.decide(
            &TypeSafe,
            ok(&json!({
                "selection": {"order.refund": 1.0},
                "fill.0.0": {"A-1": 1.0},
                "fill.0.1": {"ups": 1.0},
                "fill.1.0": {"stock": 1.0},
            }))
        ),
        Returned::failed(FailureCode::Malformed)
    );
    // An answer missing a question.
    assert_eq!(
        prepared.decide(&TypeSafe, ok(&json!({"selection": {"order.ship": 1.0}}))),
        Returned::failed(FailureCode::Malformed)
    );
    assert_eq!(
        Returned::failed(FailureCode::Unconfigured).data,
        r#"{"code":"unconfigured"}"#
    );
}

#[test]
fn rank_sorts_by_score_then_label_and_keeps_every_candidate() {
    let prepared = prepared(&[
        ("kind", Data::String("rank".into())),
        ("state", Data::String("pick".into())),
        (
            "candidates",
            Data::JsonArray(r#"["b","a",{"id":3}]"#.into()),
        ),
    ]);
    let returned = prepared.decide(
        &TypeSafe,
        ok(&json!({"rank": {"a": 0.4, "b": 0.4, "{\"id\":3}": 0.2}})),
    );
    assert_eq!(returned.name, RANKED_EVENT);
    assert_eq!(
        returned.data,
        r#"{"ranking":[{"candidate":"a","score":0.4},{"candidate":"b","score":0.4},{"candidate":{"id":3},"score":0.2}]}"#
    );
}

#[test]
fn classify_takes_labels_in_every_form() {
    for labels in [
        Data::String("calm  angry".into()),
        Data::JsonArray(r#"["calm","angry"]"#.into()),
        Data::JsonObject(r#"{"angry":"upset","calm":null}"#.into()),
    ] {
        let prepared = prepared(&[
            ("kind", Data::String("classify".into())),
            ("state", Data::String("I am fine".into())),
            ("labels", labels.clone()),
        ]);
        let returned = prepared.decide(
            &TypeSafe,
            ok(&json!({"classify": {"calm": 0.75, "angry": 0.25}})),
        );
        assert_eq!(returned.name, CLASSIFIED_EVENT, "{labels:?}");
        assert_eq!(returned.data, r#"{"labels":{"angry":0.25,"calm":0.75}}"#);
    }
}

#[test]
fn malformed_parameters_reject_the_start() {
    let state = ("state", order_state());
    let choices = ("choices", order_choices());
    for (parameters, why) in [
        (vec![choices.clone()], "no state"),
        (vec![state.clone()], "choose without a choice list"),
        (
            vec![state.clone(), choices.clone(), ("choices", order_choices())],
            "a repeated parameter",
        ),
        (
            vec![
                state.clone(),
                choices.clone(),
                ("labels", Data::String("a".into())),
            ],
            "a parameter of another kind",
        ),
        (
            vec![state.clone(), ("kind", Data::String("pick".into()))],
            "an unknown kind",
        ),
        (
            vec![("state", Data::Bool(true)), choices.clone()],
            "a boolean state",
        ),
        (
            vec![
                state.clone(),
                ("choices", Data::JsonArray(r#"[{"event":"a"}]"#.into())),
            ],
            "a choice without a schema",
        ),
        (
            vec![
                state.clone(),
                (
                    "choices",
                    Data::JsonArray(
                        r#"[{"event":"a","schema":{}},{"event":"a","schema":{}}]"#.into(),
                    ),
                ),
            ],
            "a choice list naming an event twice",
        ),
        (
            vec![
                state.clone(),
                (
                    "choices",
                    Data::JsonArray(r#"[{"event":"a.*","schema":{}}]"#.into()),
                ),
            ],
            "a wildcard choices event",
        ),
        (
            vec![
                state.clone(),
                choices.clone(),
                (
                    "threshold",
                    Data::Number(scxml_types::FiniteNumber::new(1.5).expect("finite")),
                ),
            ],
            "a threshold above 1",
        ),
        (
            vec![
                state.clone(),
                choices.clone(),
                ("data", Data::String("x".into())),
            ],
            "non-object evidence",
        ),
    ] {
        assert!(
            parse_parameters(parameters.iter().map(|(name, value)| (*name, value))).is_err(),
            "{why}"
        );
    }
}

#[test]
fn malformed_rank_classify_or_common_parameters_reject_the_start() {
    let state = ("state", order_state());
    let choices = ("choices", order_choices());
    for (parameters, why) in [
        (
            vec![
                state.clone(),
                ("kind", Data::String("rank".into())),
                ("candidates", Data::JsonArray(r#"["a","a"]"#.into())),
            ],
            "duplicate candidates",
        ),
        (
            vec![
                state.clone(),
                ("kind", Data::String("rank".into())),
                ("candidates", Data::JsonArray("[]".into())),
            ],
            "no candidates",
        ),
        (
            vec![
                state.clone(),
                ("kind", Data::String("classify".into())),
                ("labels", Data::JsonArray(r#"["a",1]"#.into())),
            ],
            "a non-string label",
        ),
        (
            vec![state, choices, ("model", Data::String(" ".into()))],
            "an empty model",
        ),
    ] {
        assert!(
            parse_parameters(parameters.iter().map(|(name, value)| (*name, value))).is_err(),
            "{why}"
        );
    }
}

#[test]
fn another_backend_plugs_in_behind_the_trait() {
    /// A backend that always answers the first criterion of every question.
    struct First;

    impl Backend for First {
        fn request(&self, batch: &Batch, _api_key: u32, _base: Part) -> Request {
            Request {
                url: vec![Part::Literal("http://first.invalid".into())],
                headers: Vec::new(),
                body: batch.questions.len().to_string().into_bytes(),
            }
        }

        fn answers(&self, batch: &Batch, _body: &[u8]) -> Result<Vec<Distribution>, &'static str> {
            Ok(batch
                .questions
                .iter()
                .map(|question| Distribution {
                    probabilities: (0..question.criteria.len())
                        .map(|index| if index == 0 { 1.0 } else { 0.0 })
                        .collect(),
                })
                .collect())
        }
    }

    let prepared = choose();
    assert_eq!(prepared.request(&First, 1, Part::Reference(2)).body, b"4");
    let returned = prepared.decide(
        &First,
        Ok(Response {
            status: 200,
            body: Vec::new(),
        }),
    );
    assert_eq!(returned.name, "order.ship");
    assert_eq!(returned.data, r#"{"carrier":"ups","sku":"A-1"}"#);
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
    assert_eq!(pick(vec![0.2, 0.5, 0.3]), Some(1));
    assert_eq!(
        pick(vec![0.4, 0.4, 0.2]),
        Some(1),
        "a tie goes to the smaller label"
    );
    assert_eq!(pick(vec![0.0, 0.0, 0.0]), Some(1));
    assert_eq!(
        backend::argmax(
            &[],
            &Distribution {
                probabilities: Vec::new()
            }
        ),
        None
    );
}

#[test]
fn the_event_name_bound_applies_to_wasm_builds_only() {
    let long = "e".repeat(300);
    assert_eq!(valid_event_name(&long), !cfg!(target_arch = "wasm32"));
}

#[test]
fn without_a_baseurl_value_the_base_is_the_packages_first_declared_origin() {
    let manifest = scxml_validator_core::ServicePackageManifest::from_json(include_bytes!(
        "../package/decision.service.json"
    ))
    .expect("the manifest parses");
    let origin = super::package_origin().expect("the manifest declares an origin");
    assert_eq!(Some(&origin), manifest.http_origins.first());
    assert_eq!(origin, "https://api.typesafe.ai");
}
