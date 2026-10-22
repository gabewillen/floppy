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

fn choice() -> Prepared {
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
    ] {
        assert!(
            abstain.admits(&scxml_types::DataValue::JsonObject(data.into())),
            "{data}"
        );
    }
}

#[test]
fn choice_asks_the_selection_and_the_grounded_members_in_one_batch() {
    let mut prepared = choice();
    let batch = prepared.batch();
    let names: Vec<&str> = batch
        .questions
        .iter()
        .map(|question| question.name.as_str())
        .collect();
    // order.ship: sku (evidence), carrier (enum); order.cancel: reason
    // (enum); order.hold: nothing required.
    assert_eq!(
        names,
        [
            "selection",
            "order.ship::sku",
            "order.ship::carrier",
            "order.cancel::reason"
        ]
    );
    let selection: Vec<&str> = batch.questions[0]
        .criteria
        .iter()
        .map(|criterion| criterion.label.as_str())
        .collect();
    assert_eq!(
        selection,
        ["order.ship", "order.cancel", "order.hold", ABSTAIN_LABEL]
    );
    // The reference `_event_wording`: the description's first line, else the
    // event name; the pass criterion's wording is the original's.
    let descriptions: Vec<&Json> = batch.questions[0]
        .criteria
        .iter()
        .map(|criterion| &criterion.description)
        .collect();
    assert_eq!(
        descriptions,
        [
            "Ship the order.",
            "Cancel the order",
            "order.hold",
            "None of these: leave the turn unhandled."
        ]
    );
    assert_eq!(
        batch.questions[2].instructions,
        "Which value fills 'carrier' for order.ship? Fill one payload field for the selected event: the criteria are the exact values the evidence offers. Pick the value the turn supports."
    );
    assert_eq!(batch.state, json!({"qty": 2, "sku": "A-1"}));
    let request = prepared.request(&TypeSafe, Some(1), Part::Reference(2));
    assert_eq!(
        request.headers.first(),
        Some(&(
            "authorization".to_owned(),
            vec![Part::Literal("Bearer ".into()), Part::Reference(1)]
        ))
    );
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
        body["questions"]["order.ship::carrier"]["criteria"],
        json!({
            "fedex": "Which value fills 'carrier' for order.ship?",
            "ups": "Which value fills 'carrier' for order.ship?"
        })
    );
}

#[test]
fn choice_returns_the_chosen_event_with_grounded_data_validated_against_its_schema() {
    let returned = choice().decide(
        &TypeSafe,
        &ok(&json!({
            "selection": {"order.ship": 0.7, "order.cancel": 0.1, "order.hold": 0.1, "__unhandled__": 0.1},
            "order.ship::sku": {"A-1": 1.0},
            "order.ship::carrier": {"ups": 0.2, "fedex": 0.8},
            "order.cancel::reason": {"customer": 0.5, "stock": 0.5},
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
fn choice_breaks_ties_by_label_bytes() {
    let returned = choice().decide(
        &TypeSafe,
        &ok(&json!({
            "selection": {"order.ship": 0.4, "order.cancel": 0.4, "order.hold": 0.1, "__unhandled__": 0.1},
            "order.ship::sku": {"A-1": 1.0},
            "order.ship::carrier": {"ups": 0.5, "fedex": 0.5},
            "order.cancel::reason": {"customer": 0.5, "stock": 0.5},
        })),
    );
    assert_eq!(returned.name, "order.cancel");
    assert_eq!(returned.data, r#"{"reason":"customer"}"#);
}

#[test]
fn choice_abstains_when_declined_or_with_nothing_to_offer() {
    let declined = choice().decide(
        &TypeSafe,
        &ok(&json!({
            "selection": {"__unhandled__": 0.9, "order.ship": 0.1},
            "order.ship::sku": {"A-1": 1.0},
            "order.ship::carrier": {"ups": 1.0},
            "order.cancel::reason": {"stock": 1.0},
        })),
    );
    assert_eq!(declined.name, ABSTAIN_EVENT);
    assert_eq!(declined.data, r#"{"confidence":0.9,"reason":"declined"}"#);
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

fn predicate() -> Prepared {
    prepared(&[
        ("kind", Data::String("predicate".into())),
        ("choices", order_choices()),
        ("state", order_state()),
    ])
}

#[test]
fn predicate_returns_only_the_chosen_event_with_its_probability() {
    let prepared = predicate();
    // The same batch as choice, with the original's selection instructions
    // and a pass to reasoning in place of the choice's pass criterion.
    assert_eq!(prepared.batch().questions.len(), 4);
    assert!(
        prepared.batch().questions[0]
            .instructions
            .starts_with("Choose the single criterion that answers this turn")
    );
    let selection: Vec<(&str, &Json)> = prepared.batch().questions[0]
        .criteria
        .iter()
        .map(|criterion| (criterion.label.as_str(), &criterion.description))
        .collect();
    assert_eq!(
        selection.last(),
        Some(&(
            "__reasoning__",
            &json!(
                "None of these fits: it needs thought, so think it through first, with reasoning."
            )
        ))
    );
    assert_eq!(selection.len(), 4, "{selection:?}");
    let returned = prepared.decide(
        &TypeSafe,
        &ok(&json!({
            "selection": {"order.ship": 0.5, "order.cancel": 0.25, "order.hold": 0.125, "__reasoning__": 0.125},
            "order.ship::sku": {"A-1": 1.0},
            "order.ship::carrier": {"ups": 0.2, "fedex": 0.8},
            "order.cancel::reason": {"customer": 0.4, "stock": 0.6},
        })),
    );
    // The reference D-R-28: the other options' probabilities are not
    // selections.
    assert_eq!(returned.name, SELECTED_EVENT);
    assert_eq!(
        returned.data,
        canonical_json(&json!({"selections": [
            {"event": "order.ship", "data": {"carrier": "fedex", "sku": "A-1"}, "confidence": 0.5},
        ]}))
    );
    // The pass criterion winning hands the turn to its tier.
    let declined = predicate().decide(
        &TypeSafe,
        &ok(&json!({
            "selection": {"__reasoning__": 0.7, "order.ship": 0.3},
            "order.ship::sku": {"A-1": 1.0},
            "order.ship::carrier": {"ups": 1.0},
            "order.cancel::reason": {"stock": 1.0},
        })),
    );
    assert_eq!(declined.name, ABSTAIN_EVENT);
    assert_eq!(
        declined.data,
        r#"{"confidence":0.7,"hand_to":"reasoning","reason":"declined"}"#
    );
    // The chosen event's filled data failing its schema fails the answer.
    let mismatched = prepared_predicate_with_evidence(r#"{"carrier":"dhl","sku":"A-1"}"#).decide(
        &TypeSafe,
        &ok(&json!({
            "selection": {"order.ship": 1.0},
            "order.ship::sku": {"A-1": 1.0},
            "order.ship::carrier": {"dhl": 1.0},
            "order.cancel::reason": {"stock": 1.0},
        })),
    );
    assert_eq!(mismatched, Returned::failed(FailureCode::SchemaMismatch));
    // Another event's data is not checked: only the chosen one is returned.
    let held = prepared_predicate_with_evidence(r#"{"carrier":"dhl","sku":"A-1"}"#).decide(
        &TypeSafe,
        &ok(&json!({
            "selection": {"order.hold": 1.0},
            "order.ship::sku": {"A-1": 1.0},
            "order.ship::carrier": {"dhl": 1.0},
            "order.cancel::reason": {"stock": 1.0},
        })),
    );
    assert_eq!(
        held.data,
        canonical_json(
            &json!({"selections": [{"event": "order.hold", "data": {}, "confidence": 1.0}]})
        )
    );
}

#[test]
fn a_predicate_passes_to_each_tier_its_hand_ons_name() {
    let both = prepared(&[
        ("kind", Data::String("predicate".into())),
        ("choices", order_choices()),
        ("state", order_state()),
        (
            "hand_ons",
            Data::JsonArray(r#"["intuition","reasoning"]"#.into()),
        ),
    ]);
    let passes: Vec<(&str, &Json)> = both.batch().questions[0]
        .criteria
        .iter()
        .skip(3)
        .map(|criterion| (criterion.label.as_str(), &criterion.description))
        .collect();
    assert_eq!(
        passes,
        [
            (
                "__intuition__",
                &json!("None of these fits: answer it in words now, with intuition.")
            ),
            (
                "__reasoning__",
                &json!(
                    "None of these fits: it needs thought, so think it through first, with reasoning."
                )
            ),
        ]
    );
    for (tier, label) in [
        ("intuition", "__intuition__"),
        ("reasoning", "__reasoning__"),
    ] {
        let declined = both.decide(
            &TypeSafe,
            &ok(&json!({
                "selection": {label: 0.6, "order.ship": 0.4},
                "order.ship::sku": {"A-1": 1.0},
                "order.ship::carrier": {"ups": 1.0},
                "order.cancel::reason": {"stock": 1.0},
            })),
        );
        assert_eq!(
            declined.data,
            canonical_json(&json!({"confidence": 0.6, "hand_to": tier, "reason": "declined"}))
        );
    }
    // A tier the request does not name is not offered.
    let intuition_only = prepared(&[
        ("kind", Data::String("predicate".into())),
        ("choices", order_choices()),
        ("state", order_state()),
        ("hand_ons", Data::JsonArray(r#"["intuition"]"#.into())),
    ]);
    assert_eq!(
        intuition_only.decide(
            &TypeSafe,
            &ok(&json!({
                "selection": {"__reasoning__": 1.0},
                "order.ship::sku": {"A-1": 1.0},
                "order.ship::carrier": {"ups": 1.0},
                "order.cancel::reason": {"stock": 1.0},
            }))
        ),
        Returned::failed(FailureCode::Malformed)
    );
}

#[test]
fn a_selection_carries_its_only_offerer_as_target() {
    // order.ship has one offerer, order.cancel two: the original's
    // _single_enabler.
    let tagged = Data::JsonArray(canonical_json(&json!([
        {"event": "order.ship", "schema": ship_schema(), "target": "session.floppy.orders"},
        {"event": "order.cancel", "schema": cancel_schema(), "target": "session.floppy.orders"},
        {"event": "order.cancel", "schema": cancel_schema(), "target": "session.floppy.returns"},
        {"event": "order.hold", "schema": {"type": "object"}},
    ])));
    let asked = prepared(&[
        ("kind", Data::String("predicate".into())),
        ("choices", tagged),
        ("state", order_state()),
    ]);
    let answer = |chosen: &str| {
        let mut selection = json!({"order.ship": 0.1, "order.cancel": 0.1, "order.hold": 0.1, "__reasoning__": 0.1});
        selection[chosen] = json!(0.7);
        asked.decide(
            &TypeSafe,
            &ok(&json!({
                "selection": selection,
                "order.ship::sku": {"A-1": 1.0},
                "order.ship::carrier": {"ups": 0.2, "fedex": 0.8},
                "order.cancel::reason": {"customer": 0.4, "stock": 0.6},
            })),
        )
    };
    assert_eq!(
        answer("order.ship").data,
        canonical_json(&json!({"selections": [
            {"event": "order.ship", "data": {"carrier": "fedex", "sku": "A-1"}, "confidence": 0.7, "target": "session.floppy.orders"},
        ]}))
    );
    assert_eq!(
        answer("order.cancel").data,
        canonical_json(&json!({"selections": [
            {"event": "order.cancel", "data": {"reason": "stock"}, "confidence": 0.7},
        ]}))
    );
}

fn prepared_predicate_with_evidence(evidence: &str) -> Prepared {
    prepared(&[
        ("kind", Data::String("predicate".into())),
        ("choices", order_choices()),
        ("state", order_state()),
        ("data", Data::JsonObject(evidence.into())),
    ])
}

#[test]
fn members_ground_from_non_null_evidence_then_enum_never_const() {
    let choices = Data::JsonArray(canonical_json(&json!([
        {"event": "lamp.set", "schema": {
            "type": "object",
            "properties": {
                "mode": {"enum": ["on", null, "off"]},
                "room": {"const": "hall"},
            },
            "required": ["mode"],
        }},
        {"event": "lamp.move", "schema": {
            "type": "object",
            "properties": {"room": {"const": "hall"}},
            "required": ["room"],
        }},
    ])));
    let asked = prepared(&[
        ("kind", Data::String("predicate".into())),
        ("choices", choices.clone()),
        ("state", Data::JsonObject(r#"{"turn":{}}"#.into())),
        ("data", Data::JsonObject(r#"{"mode":null}"#.into())),
    ]);
    // lamp.move's room has only a const: it is not offered.
    let batch = asked.batch();
    let names: Vec<&str> = batch
        .questions
        .iter()
        .map(|question| question.name.as_str())
        .collect();
    assert_eq!(names, ["selection", "lamp.set::mode"]);
    let options: Vec<&str> = batch.questions[1]
        .criteria
        .iter()
        .map(|criterion| criterion.label.as_str())
        .collect();
    assert_eq!(
        options,
        ["on", "off"],
        "null evidence and null enum values are no options"
    );
    // With evidence, its value comes first.
    let evidenced = prepared(&[
        ("kind", Data::String("predicate".into())),
        ("choices", choices),
        ("state", Data::JsonObject(r#"{"turn":{}}"#.into())),
        ("data", Data::JsonObject(r#"{"mode":"dim"}"#.into())),
    ]);
    let options: Vec<&str> = evidenced.batch().questions[1]
        .criteria
        .iter()
        .map(|criterion| criterion.label.as_str())
        .collect();
    assert_eq!(options, ["dim", "on", "off"]);
}

#[test]
fn a_stimulus_grounds_the_data_and_the_state_is_the_model_facing_turn() {
    // The reference grounds from the live stimulus data and shows the model
    // its model-facing text (D-R-50): the state is the document's, as given.
    let turn = Data::JsonObject(r#"{"turn":"<bot.input>ship it</bot.input>"}"#.into());
    let asked = prepared(&[
        ("kind", Data::String("predicate".into())),
        ("choices", order_choices()),
        ("state", turn.clone()),
        (
            "stimulus",
            Data::JsonObject(
                r#"{"event":"bot.input","data":{"carrier":"ups","sku":"B-2"},"source":"body"}"#
                    .into(),
            ),
        ),
    ]);
    assert_eq!(
        asked.batch().state,
        json!({"turn": "<bot.input>ship it</bot.input>"})
    );
    let options: Vec<&str> = asked.batch().questions[1]
        .criteria
        .iter()
        .map(|criterion| criterion.label.as_str())
        .collect();
    assert_eq!(options, ["B-2"], "order.ship::sku grounds from its data");
    // A stimulus that is not a named envelope is its own evidence when it
    // is an object; any other value grounds nothing (and order.ship, whose
    // sku has no option, is not offered).
    for (stimulus, offered) in [
        (Data::JsonObject(r#"{"sku":"C-3"}"#.into()), 4),
        (Data::Null, 3),
        (Data::String("hello".into()), 3),
        (Data::JsonObject(r#"{"event":"bot.input"}"#.into()), 3),
    ] {
        let asked = prepared(&[
            ("choices", order_choices()),
            ("state", turn.clone()),
            ("stimulus", stimulus),
        ]);
        assert_eq!(asked.batch().questions[0].criteria.len(), offered);
    }
    for (parameters, why) in [
        (
            vec![("choices", order_choices()), ("stimulus", Data::Null)],
            "a stimulus without a state",
        ),
        (
            vec![
                ("choices", order_choices()),
                ("state", turn.clone()),
                ("stimulus", Data::Null),
                ("data", order_state()),
            ],
            "a stimulus with data",
        ),
        (
            vec![
                ("kind", Data::String("score".into())),
                ("state", turn.clone()),
                ("candidates", Data::JsonArray(r#"["a"]"#.into())),
                ("stimulus", Data::Null),
            ],
            "a score with a stimulus",
        ),
    ] {
        assert!(
            parse_parameters(parameters.iter().map(|(name, value)| (*name, value))).is_err(),
            "{why}"
        );
    }
}

#[test]
fn the_deliberative_handoff_is_never_offered() {
    // The original's TypeSafe processor skips the deliberative handoff.
    let choices = Data::JsonArray(canonical_json(&json!([
        {"event": DELIBERATIVE_HANDOFF_EVENT, "schema": {"type": "object"}},
        {"event": "order.hold", "schema": {"type": "object"}},
    ])));
    let asked = prepared(&[
        ("kind", Data::String("predicate".into())),
        ("choices", choices),
        ("state", order_state()),
    ]);
    let selection: Vec<&str> = asked.batch().questions[0]
        .criteria
        .iter()
        .map(|criterion| criterion.label.as_str())
        .collect();
    assert_eq!(selection, ["order.hold", HandOn::Reasoning.label()]);
    let only = Data::JsonArray(canonical_json(&json!([
        {"event": DELIBERATIVE_HANDOFF_EVENT, "schema": {"type": "object"}},
    ])));
    let plan = parse_parameters([("choices", &only), ("state", &order_state())])
        .expect("valid parameters")
        .plan()
        .expect("within bounds");
    assert!(
        matches!(plan, Plan::Return(ref returned) if returned.data == r#"{"reason":"no-candidates"}"#),
        "{plan:?}"
    );
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
        &ok(&json!({
            "selection": {"order.ship": 1.0},
            "order.ship::sku": {"A-1": 1.0},
            "order.ship::carrier": {"dhl": 0.9, "ups": 0.05, "fedex": 0.05},
            "order.cancel::reason": {"stock": 1.0},
        })),
    );
    assert_eq!(returned, Returned::failed(FailureCode::SchemaMismatch));
}

#[test]
fn a_failed_request_or_a_malformed_answer_returns_its_failure_code() {
    let prepared = choice();
    assert_eq!(
        prepared.decide(&TypeSafe, &Err(TransportFailure::Unavailable)),
        Returned::failed(FailureCode::Transport)
    );
    assert_eq!(
        prepared.decide(&TypeSafe, &Err(TransportFailure::TooLarge)),
        Returned::failed(FailureCode::Malformed)
    );
    assert_eq!(
        prepared.decide(
            &TypeSafe,
            &Ok(Response {
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
            &ok(&json!({
                "selection": {"order.refund": 1.0},
                "order.ship::sku": {"A-1": 1.0},
                "order.ship::carrier": {"ups": 1.0},
                "order.cancel::reason": {"stock": 1.0},
            }))
        ),
        Returned::failed(FailureCode::Malformed)
    );
    // An answer missing a question.
    assert_eq!(
        prepared.decide(&TypeSafe, &ok(&json!({"selection": {"order.ship": 1.0}}))),
        Returned::failed(FailureCode::Malformed)
    );
    assert_eq!(
        Returned::failed(FailureCode::Unconfigured).data,
        r#"{"code":"unconfigured"}"#
    );
}

#[test]
fn score_sorts_by_score_then_label_and_keeps_every_candidate() {
    let prepared = prepared(&[
        ("kind", Data::String("score".into())),
        ("state", Data::String("pick".into())),
        (
            "candidates",
            Data::JsonArray(r#"["b","a",{"id":3}]"#.into()),
        ),
    ]);
    let returned = prepared.decide(
        &TypeSafe,
        &ok(&json!({"score": {"a": 0.4, "b": 0.4, "{\"id\":3}": 0.2}})),
    );
    assert_eq!(returned.name, RANKED_EVENT);
    assert_eq!(
        returned.data,
        r#"{"ranking":[{"candidate":"a","score":0.4},{"candidate":"b","score":0.4},{"candidate":{"id":3},"score":0.2}]}"#
    );
}

#[test]
fn a_choice_over_labels_takes_them_in_every_form() {
    for labels in [
        Data::String("calm  angry".into()),
        Data::JsonArray(r#"["calm","angry"]"#.into()),
        Data::JsonObject(r#"{"angry":"upset","calm":null}"#.into()),
    ] {
        let prepared = prepared(&[
            ("kind", Data::String("choice".into())),
            ("state", Data::String("I am fine".into())),
            ("labels", labels.clone()),
        ]);
        let returned = prepared.decide(
            &TypeSafe,
            &ok(&json!({"choice": {"calm": 0.75, "angry": 0.25}})),
        );
        assert_eq!(returned.name, CLASSIFIED_EVENT, "{labels:?}");
        assert_eq!(returned.data, r#"{"labels":{"angry":0.25,"calm":0.75}}"#);
    }
}

#[test]
fn a_turns_assembled_choices_are_one_candidate_per_event() {
    // A document's choices, untagged and tagged with their target, as
    // `<processing:collect_offered_events>` writes them.
    let choices = choices_from_json(&json!([
        {"event": "bot.focus_device", "schema": {"type": "object"}},
        {"event": "order.cancel", "schema": cancel_schema(), "target": "orders"},
        {"event": "order.cancel", "schema": cancel_schema(), "target": "returns", "description": "Later"},
    ]))
    .expect("tagged choices parse");
    let events: Vec<&str> = choices.iter().map(|choice| choice.event.as_str()).collect();
    assert_eq!(events, ["bot.focus_device", "order.cancel"]);
    assert_eq!(
        choices
            .get(1)
            .and_then(|choice| choice.description.as_deref()),
        None,
        "the first listing's description (none: its schema has only a title) is kept"
    );
    for (repeated, why) in [
        (
            json!([{"event": "a", "schema": {}, "target": "x"}, {"event": "a", "schema": {}}]),
            "a repeat without a target",
        ),
        (
            json!([{"event": "a", "schema": {}}, {"event": "a", "schema": {}, "target": "x"}]),
            "an untagged event listed again",
        ),
        (
            json!([{"event": "a", "schema": {}, "target": "x"}, {"event": "a", "schema": {}, "target": "x"}]),
            "the same target twice",
        ),
        (
            json!([{"event": "a", "schema": {}, "target": "x"}, {"event": "a", "schema": {"type": "object"}, "target": "y"}]),
            "another schema for the same event",
        ),
        (
            json!([{"event": "a", "schema": {}, "target": ""}]),
            "an empty target",
        ),
        (
            json!([{"event": "a", "schema": {}, "target": "x"}, {"event": "a", "schema": {}, "target": "y", "description": 1}]),
            "a repeat with a malformed description",
        ),
    ] {
        assert!(choices_from_json(&repeated).is_err(), "{why}");
    }
}

#[test]
fn malformed_parameters_reject_the_start() {
    let state = ("state", order_state());
    let choices = ("choices", order_choices());
    for (parameters, why) in [
        (vec![choices.clone()], "no state"),
        (vec![state.clone()], "a choice without choices or labels"),
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
            "a choice with both choices and labels",
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
                (
                    "choices",
                    Data::JsonArray(r#"[{"event":"__unhandled__","schema":{}}]"#.into()),
                ),
            ],
            "a choice naming the pass criterion",
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
            "a threshold, which neither choice nor predicate takes",
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
fn malformed_score_label_or_common_parameters_reject_the_start() {
    let state = ("state", order_state());
    let choices = ("choices", order_choices());
    for (parameters, why) in [
        (
            vec![
                state.clone(),
                ("kind", Data::String("score".into())),
                ("candidates", Data::JsonArray(r#"["a","a"]"#.into())),
            ],
            "duplicate candidates",
        ),
        (
            vec![
                state.clone(),
                ("kind", Data::String("score".into())),
                ("candidates", Data::JsonArray("[]".into())),
            ],
            "no candidates",
        ),
        (
            vec![
                state.clone(),
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
        fn request(&self, batch: &Batch, _api_key: Option<u32>, _base: Part) -> Request {
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

        fn usage(&self, _body: &[u8]) -> Option<Usage> {
            None
        }
    }

    let mut prepared = choice();
    assert_eq!(
        prepared.request(&First, Some(1), Part::Reference(2)).body,
        b"4"
    );
    let returned = prepared.decide(
        &First,
        &Ok(Response {
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

#[test]
fn a_keyless_request_sends_no_authorization() {
    let request = choice().request(&TypeSafe, None, Part::Reference(2));
    let names: Vec<&str> = request
        .headers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(names, ["content-type", "accept"]);
}

/// A predicate whose selection instructions are `instructions`, fitted to
/// `max_context_tokens` at `bytes_per_token`.
fn fitted(instructions: &str, max_context_tokens: f64, bytes_per_token: f64) -> Prepared {
    let number = |value: f64| Data::Number(scxml_types::FiniteNumber::new(value).expect("finite"));
    prepared(&[
        ("kind", Data::String("predicate".into())),
        ("choices", order_choices()),
        ("state", order_state()),
        ("instructions", Data::String(instructions.into())),
        ("max_context_tokens", number(max_context_tokens)),
        ("bytes_per_token", number(bytes_per_token)),
    ])
}

#[test]
fn a_request_over_its_context_keeps_the_leading_whole_lines_that_fit() {
    let instructions = "<environment id=\"room\"/>\nYou already did: nothing.\nThe feedback.";
    // The body's size with only the first line, and with every line.
    let size = |instructions: &str| {
        let mut whole = prepared(&[
            ("kind", Data::String("predicate".into())),
            ("choices", order_choices()),
            ("state", order_state()),
            ("instructions", Data::String(instructions.into())),
        ]);
        whole
            .request(&TypeSafe, None, Part::Reference(2))
            .body
            .len()
    };
    let first = size("<environment id=\"room\"/>");
    let all = size(instructions);
    let tokens = |bytes: usize| f64::from(u32::try_from(bytes).expect("small"));
    // Whole: it fits.
    let mut whole = fitted(instructions, tokens(all), 1.0);
    let request = whole.request(&TypeSafe, None, Part::Reference(2));
    assert_eq!(request.body.len(), all);
    assert_eq!(whole.batch().questions[0].instructions, instructions);
    assert_eq!(
        whole.done_data(&TypeSafe, &ok(&json!({}))),
        Some(canonical_json(&json!({
            "context_fit": "fit",
            "request_bytes": all,
            "usage": {"input_tokens": 1, "output_tokens": 1},
        })))
    );
    // Cut to the first line: the state and the menu are untouched.
    let mut cut = fitted(instructions, tokens(first + 1), 1.0);
    let request = cut.request(&TypeSafe, None, Part::Reference(2));
    assert_eq!(request.body.len(), first);
    assert_eq!(
        cut.batch().questions[0].instructions,
        "<environment id=\"room\"/>"
    );
    assert_eq!(cut.batch().questions[0].criteria.len(), 4);
    assert_eq!(cut.batch().state, json!({"qty": 2, "sku": "A-1"}));
    // The budget is tokens times the learned bytes a token.
    let mut halved = fitted(instructions, tokens(first + 1) * 2.0, 0.5);
    halved.request(&TypeSafe, None, Part::Reference(2));
    assert_eq!(
        halved.batch().questions[0].instructions,
        "<environment id=\"room\"/>"
    );
    // Nothing fits: the default instructions are sent anyway.
    let mut over = fitted(instructions, 1.0, 1.0);
    over.request(&TypeSafe, None, Part::Reference(2));
    assert!(
        over.batch().questions[0]
            .instructions
            .starts_with("Choose the single criterion that answers this turn")
    );
    let done = over
        .done_data(&TypeSafe, &Err(TransportFailure::Unavailable))
        .expect("a request was built");
    assert!(done.contains(r#""context_fit":"overflow""#), "{done}");
    assert!(!done.contains("usage"), "no answer, no usage: {done}");
}

#[test]
fn done_data_reports_the_request_bytes_and_the_answers_usage() {
    let mut prepared = choice();
    assert_eq!(prepared.done_data(&TypeSafe, &ok(&json!({}))), None);
    let bytes = prepared
        .request(&TypeSafe, Some(1), Part::Reference(2))
        .body
        .len();
    assert_eq!(
        prepared.done_data(&TypeSafe, &ok(&json!({}))),
        Some(canonical_json(&json!({
            "request_bytes": bytes,
            "usage": {"input_tokens": 1, "output_tokens": 1},
        })))
    );
    assert_eq!(
        prepared.done_data(
            &TypeSafe,
            &Ok(Response {
                status: 500,
                body: br#"{"usage":{"input_tokens":9}}"#.to_vec()
            })
        ),
        Some(canonical_json(&json!({"request_bytes": bytes}))),
        "a failed answer teaches nothing"
    );
}

#[test]
fn instructions_split_into_lines_as_the_reference_splits_them() {
    assert_eq!(
        split_lines("a\r\nb\rc\nd\u{2028}e\n"),
        ["a", "b", "c", "d", "e"]
    );
    assert_eq!(split_lines("a\n\nb"), ["a", "", "b"]);
    assert!(split_lines("").is_empty());
}

#[test]
fn malformed_tier_parameters_reject_the_start() {
    let state = ("state", order_state());
    let choices = ("choices", order_choices());
    for (parameters, why) in [
        (
            vec![
                state.clone(),
                (
                    "choices",
                    Data::JsonArray(r#"[{"event":"__reasoning__","schema":{}}]"#.into()),
                ),
            ],
            "a choice naming a tier's pass criterion",
        ),
        (
            vec![
                state.clone(),
                choices.clone(),
                ("hand_ons", Data::JsonArray(r#"["reasoning"]"#.into())),
            ],
            "hand_ons on a choice",
        ),
        (
            vec![
                state.clone(),
                choices.clone(),
                ("kind", Data::String("predicate".into())),
                ("hand_ons", Data::JsonArray("[]".into())),
            ],
            "no tier",
        ),
        (
            vec![
                state.clone(),
                choices.clone(),
                ("kind", Data::String("predicate".into())),
                (
                    "hand_ons",
                    Data::JsonArray(r#"["reasoning","reasoning"]"#.into()),
                ),
            ],
            "a tier twice",
        ),
        (
            vec![
                state.clone(),
                choices.clone(),
                ("kind", Data::String("predicate".into())),
                ("hand_ons", Data::JsonArray(r#"["reflex"]"#.into())),
            ],
            "an unknown tier",
        ),
    ] {
        assert!(
            parse_parameters(parameters.iter().map(|(name, value)| (*name, value))).is_err(),
            "{why}"
        );
    }
}

#[test]
fn malformed_context_fit_parameters_reject_the_start() {
    let state = ("state", order_state());
    let choices = ("choices", order_choices());
    for (parameters, why) in [
        (
            vec![
                state.clone(),
                choices.clone(),
                (
                    "max_context_tokens",
                    Data::Number(scxml_types::FiniteNumber::new(0.0).expect("finite")),
                ),
            ],
            "no context",
        ),
        (
            vec![
                state.clone(),
                choices.clone(),
                (
                    "max_context_tokens",
                    Data::Number(scxml_types::FiniteNumber::new(10.5).expect("finite")),
                ),
            ],
            "a fractional context",
        ),
        (
            vec![
                state.clone(),
                choices.clone(),
                (
                    "bytes_per_token",
                    Data::Number(scxml_types::FiniteNumber::new(3.5).expect("finite")),
                ),
            ],
            "more bytes a token than the starting estimate",
        ),
        (
            vec![
                state.clone(),
                choices.clone(),
                (
                    "bytes_per_token",
                    Data::Number(scxml_types::FiniteNumber::new(0.0).expect("finite")),
                ),
            ],
            "no bytes a token",
        ),
        (
            vec![
                ("state", Data::String("pick".into())),
                ("kind", Data::String("score".into())),
                ("candidates", Data::JsonArray(r#"["a"]"#.into())),
                (
                    "max_context_tokens",
                    Data::Number(scxml_types::FiniteNumber::new(100.0).expect("finite")),
                ),
            ],
            "a context fit on a score",
        ),
    ] {
        assert!(
            parse_parameters(parameters.iter().map(|(name, value)| (*name, value))).is_err(),
            "{why}"
        );
    }
}
