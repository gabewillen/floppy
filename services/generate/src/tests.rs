use super::*;

const SCHEMA: &str = r#"{"type":"object","properties":{"answer":{"type":"string"},"score":{"type":"integer"}},"required":["answer","score"],"additionalProperties":false}"#;

fn text(value: &str) -> Data {
    Data::String(value.into())
}

fn parse(parameters: &[(&str, Data)]) -> Result<Generation, &'static str> {
    parse_parameters(parameters.iter().map(|(name, value)| (*name, value)))
}

fn reply(content: &str) -> Vec<u8> {
    serde_json::json!({
        "choices": [{
            "index": 0,
            "finish_reason": "stop",
            "message": { "role": "assistant", "content": content },
        }],
    })
    .to_string()
    .into_bytes()
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "builds the answered arm of a transport outcome for `decide`"
)]
fn ok(body: Vec<u8>) -> Result<Response, TransportFailure> {
    Ok(Response { status: 200, body })
}

#[test]
fn a_prompt_with_a_system_message_builds_the_canonical_request() {
    let generation = parse(&[
        ("model", text("mock-model")),
        ("system", text("Be brief.")),
        ("prompt", text("Say hello.")),
    ])
    .expect("valid parameters");
    let request = build_request(2, 1, &generation);
    assert_eq!(
        request.url,
        [
            Part::Reference(2),
            Part::Literal(CHAT_COMPLETIONS_PATH.into())
        ]
    );
    assert_eq!(
        request.headers[0],
        (
            "authorization".to_owned(),
            vec![Part::Literal("Bearer ".into()), Part::Reference(1)]
        )
    );
    assert_eq!(
        String::from_utf8(request.body).expect("UTF-8"),
        r#"{"messages":[{"content":"Be brief.","role":"system"},{"content":"Say hello.","role":"user"}],"model":"mock-model"}"#
    );
}

#[test]
fn a_schema_asks_for_structured_output_with_its_name_and_strictness() {
    let generation = parse(&[
        ("model", text("m")),
        ("prompt", text("Decide.")),
        ("schema", Data::JsonObject(SCHEMA.into())),
        ("schemaname", text("verdict")),
        ("strict", Data::Bool(true)),
    ])
    .expect("valid parameters");
    let body: Json = serde_json::from_slice(&build_request(2, 1, &generation).body).expect("JSON");
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(body["response_format"]["json_schema"]["name"], "verdict");
    assert_eq!(body["response_format"]["json_schema"]["strict"], true);
    // A schema given as a string is the same request.
    let from_text = parse(&[
        ("model", text("m")),
        ("prompt", text("Decide.")),
        ("schema", text(SCHEMA)),
    ])
    .expect("valid parameters");
    let text_body: Json =
        serde_json::from_slice(&build_request(2, 1, &from_text).body).expect("JSON");
    assert_eq!(
        text_body["response_format"]["json_schema"]["name"],
        DEFAULT_SCHEMA_NAME
    );
    assert_eq!(text_body["response_format"]["json_schema"]["strict"], false);
}

#[test]
fn messages_are_sent_in_order() {
    let generation = parse(&[
        ("model", text("m")),
        (
            "messages",
            Data::JsonArray(
                r#"[{"content":"hi","role":"user"},{"content":"hello","role":"assistant"},{"content":"more","role":"user"}]"#
                    .into(),
            ),
        ),
    ])
    .expect("valid parameters");
    assert_eq!(generation.messages.len(), 3);
    assert_eq!(generation.messages[1].role, Role::Assistant);
}

#[test]
fn malformed_parameters_reject_the_start() {
    let model = ("model", text("m"));
    let prompt = ("prompt", text("p"));
    for (parameters, why) in [
        (vec![prompt.clone()], "no model"),
        (vec![model.clone()], "no conversation"),
        (
            vec![
                model.clone(),
                prompt.clone(),
                ("messages", Data::JsonArray("[]".into())),
            ],
            "both conversations",
        ),
        (
            vec![model.clone(), prompt.clone(), ("other", text("x"))],
            "an unknown parameter",
        ),
        (
            vec![model.clone(), prompt.clone(), ("model", text("n"))],
            "a repeated parameter",
        ),
        (
            vec![model.clone(), ("prompt", text("  "))],
            "an empty prompt",
        ),
        (
            vec![model.clone(), ("messages", Data::JsonArray("[]".into()))],
            "no message",
        ),
        (
            vec![
                model.clone(),
                ("messages", Data::JsonArray(r#"[{"role":"user"}]"#.into())),
            ],
            "a message without content",
        ),
        (
            vec![model.clone(), prompt.clone(), ("strict", Data::Bool(true))],
            "strict without a schema",
        ),
        (
            vec![
                model.clone(),
                prompt.clone(),
                ("schema", text(SCHEMA)),
                ("schemaname", text("bad name")),
            ],
            "a malformed schema name",
        ),
        (
            vec![
                model.clone(),
                prompt.clone(),
                ("schema", text(r#"{"$ref":"https://example.com/s.json"}"#)),
            ],
            "an external reference",
        ),
        (
            vec![model.clone(), prompt.clone(), ("strict", text("true"))],
            "strict as text",
        ),
        (
            vec![("model", Data::Bool(true)), prompt],
            "a non-text model",
        ),
    ] {
        assert!(parse(&parameters).is_err(), "{why}");
    }
}

#[test]
fn a_text_reply_becomes_the_result_event() {
    let generation = parse(&[("model", text("m")), ("prompt", text("p"))]).expect("valid");
    assert_eq!(
        outcome(&generation, ok(reply("hello there"))),
        Emitted {
            name: RESULT_EVENT,
            data: r#"{"text":"hello there"}"#.into()
        }
    );
}

#[test]
fn a_structured_reply_is_validated_before_it_is_emitted() {
    let generation = parse(&[
        ("model", text("m")),
        ("prompt", text("p")),
        ("schema", text(SCHEMA)),
    ])
    .expect("valid");
    assert_eq!(
        outcome(&generation, ok(reply(r#"{"score":41,"answer":"yes"}"#))),
        Emitted {
            name: RESULT_EVENT,
            data: r#"{"json":{"answer":"yes","score":41}}"#.into()
        }
    );
    assert_eq!(
        outcome(&generation, ok(reply(r#"{"answer":"yes"}"#))),
        Emitted::failed(FailureCode::SchemaMismatch)
    );
    assert_eq!(
        outcome(&generation, ok(reply("not json"))),
        Emitted::failed(FailureCode::NotJson)
    );
}

#[test]
fn every_failure_has_its_closed_code() {
    let generation = parse(&[("model", text("m")), ("prompt", text("p"))]).expect("valid");
    let failed = |answered| outcome(&generation, answered);
    assert_eq!(
        failed(Err(TransportFailure::Unavailable)),
        Emitted::failed(FailureCode::Transport)
    );
    assert_eq!(
        failed(Err(TransportFailure::TooLarge)),
        Emitted::failed(FailureCode::Malformed)
    );
    assert_eq!(
        failed(Ok(Response {
            status: 500,
            body: Vec::new()
        })),
        Emitted::failed(FailureCode::Status)
    );
    assert_eq!(
        failed(ok(b"{}".to_vec())),
        Emitted::failed(FailureCode::Malformed)
    );
    let choice = |choice: Json| {
        serde_json::json!({ "choices": [choice] })
            .to_string()
            .into_bytes()
    };
    assert_eq!(
        failed(ok(choice(serde_json::json!({
            "finish_reason": "length",
            "message": {"content": "cut"}
        })))),
        Emitted::failed(FailureCode::Truncated)
    );
    assert_eq!(
        failed(ok(choice(serde_json::json!({
            "finish_reason": "content_filter",
            "message": {"content": null}
        })))),
        Emitted::failed(FailureCode::Filtered)
    );
    assert_eq!(
        failed(ok(choice(serde_json::json!({
            "finish_reason": "stop",
            "message": {"content": null, "refusal": "no"}
        })))),
        Emitted::failed(FailureCode::Refusal)
    );
    assert_eq!(
        Emitted::failed(FailureCode::Unconfigured).data,
        r#"{"code":"unconfigured"}"#
    );
}

#[test]
fn every_failure_code_and_both_events_satisfy_the_package_event_schemas() {
    let events: Json =
        serde_json::from_str(include_str!("../package/events.json")).expect("events.json is JSON");
    let compile = |name: &str| {
        let mut schema = events["$defs"][name].clone();
        schema["$schema"] = events["$schema"].clone();
        CompiledSchema::new(&schema.to_string()).expect("the schema compiles")
    };
    let failed = compile("Failed");
    for code in [
        FailureCode::Unconfigured,
        FailureCode::Refused,
        FailureCode::Transport,
        FailureCode::Status,
        FailureCode::Malformed,
        FailureCode::Truncated,
        FailureCode::Filtered,
        FailureCode::Refusal,
        FailureCode::NotJson,
        FailureCode::SchemaMismatch,
    ] {
        assert!(
            failed.admits(&Data::JsonObject(Emitted::failed(code).data)),
            "{}",
            code.name()
        );
    }
    assert!(!failed.admits(&Data::JsonObject(r#"{"code":"other"}"#.into())));
    let result = compile("Result");
    for data in [r#"{"text":"t"}"#, r#"{"json":{"a":1}}"#, r#"{"json":null}"#] {
        assert!(result.admits(&Data::JsonObject(data.into())), "{data}");
    }
    for data in ["{}", r#"{"text":1}"#, r#"{"text":"t","json":1}"#] {
        assert!(!result.admits(&Data::JsonObject(data.into())), "{data}");
    }
}

#[test]
fn the_package_manifest_binds_the_exact_uri_and_artifacts() {
    let manifest = scxml_validator_core::ServicePackageManifest::from_json(include_bytes!(
        "../package/generate.service.json"
    ))
    .expect("the manifest parses");
    manifest.verify(None).expect("the manifest verifies");
    assert_eq!(manifest.service_uri, SERVICE_URI);
    assert_eq!(SERVICE_URI, format!("{SERVICE_URI_BASE}@{SERVICE_TAG}"));
    assert!(SERVICE_URI_BASE.ends_with(SERVICE_NAME));
    assert_eq!(
        manifest.service_artifact,
        "nemobot_service_generate.virtualized.wasm"
    );
    assert_eq!(manifest.env_slots, [API_KEY_SLOT, BASE_URL_SLOT]);
    assert!(manifest.wasi_permissions.is_empty());
}
