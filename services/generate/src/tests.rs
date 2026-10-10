use super::*;

const SCHEMA: &str = r#"{"type":"object","properties":{"answer":{"type":"string"},"score":{"type":"integer"}},"required":["answer","score"],"additionalProperties":false}"#;

fn text(value: &str) -> Data {
    Data::String(value.into())
}

fn parse(parameters: &[(&str, Data)]) -> Result<Generation, &'static str> {
    parse_with_files(parameters, &[])
}

/// A PNG's signature followed by `rest`: enough for its media type.
fn png(rest: &[u8]) -> Vec<u8> {
    [b"\x89PNG\r\n\x1a\n".as_slice(), rest].concat()
}

/// `parameters` parsed with a granted directory holding exactly `files`
/// (guest path, bytes).
fn parse_with_files(
    parameters: &[(&str, Data)],
    files: &[(&str, Vec<u8>)],
) -> Result<Generation, &'static str> {
    parse_parameters(
        parameters.iter().map(|(name, value)| (*name, value)),
        |path: &str, left: usize| {
            let (_, bytes) = files
                .iter()
                .find(|(granted, _)| *granted == path)
                .ok_or(ImageReadFailure::Unreadable)?;
            if bytes.len() > left {
                return Err(ImageReadFailure::TooLarge);
            }
            Ok(bytes.clone())
        },
    )
}

/// A `messages` parameter of one user message with `parts`.
fn user_parts(parts: &str) -> (&'static str, Data) {
    (
        "messages",
        Data::JsonArray(format!(r#"[{{"content":{parts},"role":"user"}}]"#)),
    )
}

/// A server-sent-event stream of `chunks`, then `data: [DONE]`.
fn stream(chunks: &[Json]) -> Vec<u8> {
    chunks
        .iter()
        .map(|chunk| format!("data: {chunk}\n\n"))
        .chain(std::iter::once("data: [DONE]\n\n".to_owned()))
        .collect::<String>()
        .into_bytes()
}

/// One chunk whose first choice carries `delta` and `finish_reason`.
fn chunk(delta: &Json, finish_reason: Option<&str>) -> Json {
    serde_json::json!({
        "object": "chat.completion.chunk",
        "choices": [{ "index": 0, "delta": delta, "finish_reason": finish_reason }],
    })
}

/// A streamed reply of `content`, split in two deltas after a role chunk.
fn reply(content: &str) -> Vec<u8> {
    let middle = content.chars().count() / 2;
    let head: String = content.chars().take(middle).collect();
    let tail: String = content.chars().skip(middle).collect();
    stream(&[
        chunk(
            &serde_json::json!({ "role": "assistant", "content": "" }),
            None,
        ),
        chunk(&serde_json::json!({ "content": head }), None),
        chunk(&serde_json::json!({ "content": tail }), None),
        chunk(&serde_json::json!({}), Some("stop")),
    ])
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
        r#"{"messages":[{"content":"Be brief.","role":"system"},{"content":"Say hello.","role":"user"}],"model":"mock-model","reasoning_effort":"low","stream":true}"#
    );
    assert_eq!(
        request.headers[2],
        (
            "accept".to_owned(),
            vec![Part::Literal("text/event-stream".into())]
        )
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
fn reasoning_effort_is_sent_as_given() {
    let generation = parse(&[
        ("model", text("m")),
        ("prompt", text("p")),
        ("reasoning_effort", text("high")),
    ])
    .expect("valid parameters");
    let body: Json = serde_json::from_slice(&build_request(2, 1, &generation).body).expect("JSON");
    assert_eq!(body["reasoning_effort"], "high");
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
fn an_image_part_is_sent_as_an_image_url_with_its_data_url() {
    let generation = parse_with_files(
        &[
            ("model", text("m")),
            user_parts(
                r#"[{"kind":"text","content":"Read all visible text in this image. Return only the text."},{"kind":"image","content":"media/page%201.png"}]"#,
            ),
        ],
        &[("/media/page 1.png", png(b"ab"))],
    )
    .expect("valid parameters");
    let request = build_request(0, 1, &generation);
    assert_eq!(
        String::from_utf8(request.body).expect("UTF-8"),
        r#"{"messages":[{"content":[{"text":"Read all visible text in this image. Return only the text.","type":"text"},{"image_url":{"url":"data:image/png;base64,iVBORw0KGgphYg=="},"type":"image_url"}],"role":"user"}],"model":"m","reasoning_effort":"low","stream":true}"#,
    );
}

#[test]
fn an_image_reference_resolves_against_the_guest_root() {
    for (reference, path) in [
        ("media/page.png", "/media/page.png"),
        ("/media/page.png", "/media/page.png"),
        ("media/a%20b/c.png", "/media/a b/c.png"),
    ] {
        assert_eq!(image_path(reference).as_deref(), Ok(path), "{reference}");
    }
    for reference in [
        "",
        "/",
        "file:///media/page.png",
        "https://example.com/page.png",
        "data:image/png;base64,AAAA",
        "//host/media/page.png",
        "media/../etc/passwd",
        "media/%2e%2e/etc/passwd",
        "media/./page.png",
        "media//page.png",
        "media/page.png?x",
        "media/page.png#x",
        "media/a%2Fb.png",
        "media/%zz.png",
        "media/%ff.png",
        "media/a%00.png",
    ] {
        assert!(image_path(reference).is_err(), "{reference}");
    }
}

#[test]
fn image_media_types_come_from_their_signatures() {
    assert_eq!(image_media_type(&png(b"")), Some("image/png"));
    assert_eq!(
        image_media_type(&[0xff, 0xd8, 0xff, 0xe0]),
        Some("image/jpeg")
    );
    assert_eq!(image_media_type(b"GIF89a.."), Some("image/gif"));
    assert_eq!(
        image_media_type(b"RIFF\0\0\0\0WEBPVP8 "),
        Some("image/webp")
    );
    assert_eq!(image_media_type(b"RIFF\0\0\0\0WAVE"), None);
    assert_eq!(image_media_type(b"<svg/>"), None);
    // Standard base64 with padding, RFC 4648's test vectors.
    for (rest, encoded) in [
        (b"".as_slice(), "iVBORw0KGgo="),
        (b"f", "iVBORw0KGgpm"),
        (b"fo", "iVBORw0KGgpmbw=="),
        (b"foo", "iVBORw0KGgpmb28="),
    ] {
        assert_eq!(
            data_url(&png(rest)),
            Some(format!("data:image/png;base64,{encoded}"))
        );
    }
}

#[test]
fn an_image_that_cannot_be_read_rejects_the_start() {
    let files = [
        ("/media/page.png", png(b"")),
        ("/media/note.txt", b"just text".to_vec()),
    ];
    for (parts, why) in [
        (
            r#"[{"kind":"image","content":"elsewhere/page.png"}]"#,
            "a reference outside the grant",
        ),
        (
            r#"[{"kind":"image","content":"media/missing.png"}]"#,
            "a missing file",
        ),
        (
            r#"[{"kind":"image","content":"media/note.txt"}]"#,
            "a file that is not an image",
        ),
        (
            r#"[{"kind":"image","content":"../media/page.png"}]"#,
            "a dot segment",
        ),
        ("[]", "no part"),
        (
            r#"[{"kind":"video","content":"media/page.png"}]"#,
            "an unknown kind",
        ),
        (r#"[{"kind":"image"}]"#, "a part without content"),
        (
            r#"[{"kind":"text","content":"x","extra":1}]"#,
            "a part with another member",
        ),
        (r#"[{"kind":"text","content":1}]"#, "non-string content"),
        (r#"["text"]"#, "a part that is not an object"),
    ] {
        assert!(
            parse_with_files(&[("model", text("m")), user_parts(parts)], &files).is_err(),
            "{why}"
        );
    }
    let in_system = parse_with_files(
        &[
            ("model", text("m")),
            (
                "messages",
                Data::JsonArray(
                    r#"[{"content":[{"kind":"image","content":"media/page.png"}],"role":"system"}]"#
                        .into(),
                ),
            ),
        ],
        &files,
    );
    assert!(in_system.is_err(), "an image outside a user message");
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
            vec![
                model.clone(),
                prompt.clone(),
                ("reasoning_effort", text("max")),
            ],
            "an unknown reasoning effort",
        ),
        (
            vec![
                model.clone(),
                prompt.clone(),
                ("reasoning_effort", Data::Bool(true)),
            ],
            "a non-text reasoning effort",
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
        Returned {
            name: RESULT_EVENT,
            data: r#"{"text":"hello there"}"#.into()
        }
    );
}

#[test]
fn a_structured_reply_is_validated_before_it_is_returned() {
    let generation = parse(&[
        ("model", text("m")),
        ("prompt", text("p")),
        ("schema", text(SCHEMA)),
    ])
    .expect("valid");
    assert_eq!(
        outcome(&generation, ok(reply(r#"{"score":41,"answer":"yes"}"#))),
        Returned {
            name: RESULT_EVENT,
            data: r#"{"json":{"answer":"yes","score":41}}"#.into()
        }
    );
    assert_eq!(
        outcome(&generation, ok(reply(r#"{"answer":"yes"}"#))),
        Returned::failed(FailureCode::SchemaMismatch)
    );
    assert_eq!(
        outcome(&generation, ok(reply("not json"))),
        Returned::failed(FailureCode::NotJson)
    );
}

#[test]
fn every_failure_has_its_closed_code() {
    let generation = parse(&[("model", text("m")), ("prompt", text("p"))]).expect("valid");
    let failed = |answered| outcome(&generation, answered);
    assert_eq!(
        failed(Err(TransportFailure::Unavailable)),
        Returned::failed(FailureCode::Transport)
    );
    assert_eq!(
        failed(Err(TransportFailure::TooLarge)),
        Returned::failed(FailureCode::Malformed)
    );
    assert_eq!(
        failed(Ok(Response {
            status: 500,
            body: Vec::new()
        })),
        Returned::failed(FailureCode::Status)
    );
    assert_eq!(
        failed(ok(b"{}".to_vec())),
        Returned::failed(FailureCode::Malformed)
    );
    assert_eq!(
        failed(ok(stream(&[
            chunk(&serde_json::json!({ "content": "cut" }), None),
            chunk(&serde_json::json!({}), Some("length")),
        ]))),
        Returned::failed(FailureCode::Truncated)
    );
    assert_eq!(
        failed(ok(stream(&[chunk(
            &serde_json::json!({}),
            Some("content_filter")
        )]))),
        Returned::failed(FailureCode::Filtered)
    );
    assert_eq!(
        failed(ok(stream(&[
            chunk(&serde_json::json!({ "refusal": "n" }), None),
            chunk(&serde_json::json!({ "refusal": "o" }), Some("stop")),
        ]))),
        Returned::failed(FailureCode::Refusal)
    );
    assert_eq!(
        Returned::failed(FailureCode::Unconfigured).data,
        r#"{"code":"unconfigured"}"#
    );
}

#[test]
fn a_stream_is_joined_in_order_up_to_done() {
    // CRLF lines, comments, other fields, a usage chunk without choices, and
    // a multi-line data event are all read as server-sent events.
    let body = concat!(
        ": keep-alive\r\n\r\n",
        "event: message\r\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hel\"}}]}\r\n\r\n",
        "data: {\"choices\":[{\"index\":0,\r\n",
        "data: \"delta\":{\"content\":\"lo\"},\"finish_reason\":\"stop\"}]}\r\n\r\n",
        "data: {\"choices\":[],\"usage\":{\"total_tokens\":3}}\r\n\r\n",
        "data: [DONE]\r\n\r\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ignored\"}}]}\r\n\r\n",
    );
    assert_eq!(reply_text(body.as_bytes()), Ok("Hello".to_owned()));
    // A multi-byte character split across chunks survives.
    assert_eq!(
        reply_text(&reply("héllo wörld")),
        Ok("héllo wörld".to_owned())
    );
}

#[test]
fn a_reasoning_models_reasoning_is_never_the_reply() {
    // As vLLM streams it: an empty content first, then reasoning, then the
    // answer.
    let opening = chunk(
        &serde_json::json!({ "role": "assistant", "content": "" }),
        None,
    );
    let reasoning = chunk(&serde_json::json!({ "reasoning": "think" }), None);
    let reasoning_content = chunk(&serde_json::json!({ "reasoning_content": "more" }), None);
    let answer = chunk(&serde_json::json!({ "content": "hi" }), Some("stop"));
    assert_eq!(
        reply_text(&stream(&[
            opening.clone(),
            reasoning.clone(),
            reasoning_content,
            answer
        ])),
        Ok("hi".to_owned())
    );
    // A token limit spent entirely on reasoning is a truncated reply, and a
    // stream that ends with only the empty content has none.
    let spent = chunk(&serde_json::json!({ "reasoning": "still" }), Some("length"));
    assert_eq!(
        reply_text(&stream(&[opening.clone(), reasoning.clone(), spent])),
        Err(FailureCode::Truncated)
    );
    let stopped = chunk(&serde_json::json!({}), Some("stop"));
    assert_eq!(
        reply_text(&stream(&[opening, reasoning, stopped])),
        Err(FailureCode::Malformed)
    );
}

#[test]
fn a_stream_that_is_cut_short_or_malformed_is_never_a_reply() {
    let content = chunk(&serde_json::json!({ "content": "partial" }), None);
    let without_done = format!("data: {content}\n\n").into_bytes();
    assert_eq!(reply_text(&without_done), Err(FailureCode::Malformed));
    assert_eq!(
        reply_text(b"data: not json\n\ndata: [DONE]\n\n"),
        Err(FailureCode::Malformed)
    );
    assert_eq!(
        reply_text(&stream(&[
            serde_json::json!({ "error": { "message": "boom" } })
        ])),
        Err(FailureCode::Malformed)
    );
    assert_eq!(
        reply_text(&stream(&[chunk(&serde_json::json!({}), Some("stop"))])),
        Err(FailureCode::Malformed),
        "a stream without content"
    );
    assert_eq!(reply_text(&[0xff, 0xfe]), Err(FailureCode::Malformed));
    // A non-streamed completion is not a stream.
    let completion = serde_json::json!({
        "choices": [{ "index": 0, "finish_reason": "stop", "message": { "content": "hi" } }],
    });
    assert_eq!(
        reply_text(completion.to_string().as_bytes()),
        Err(FailureCode::Malformed)
    );
}

/// The builder's `event:schema` for each of this service's events
/// (`bots/builder/schemas`), which the invoking document declares: a service
/// package declares no event contract.
#[test]
fn every_failure_code_and_both_events_satisfy_the_builder_event_schemas() {
    let compile = |text: &str| {
        let value: Json = serde_json::from_str(text).expect("the schema is JSON");
        CompiledSchema::new(&scxml_event_schema::canonical_json(&value))
            .expect("the schema compiles")
    };
    let failed = compile(include_str!(
        "../../../bots/builder/schemas/generate.failed.json"
    ));
    for code in [
        FailureCode::Unconfigured,
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
            failed.admits(&Data::JsonObject(Returned::failed(code).data)),
            "{}",
            code.name()
        );
    }
    assert!(!failed.admits(&Data::JsonObject(r#"{"code":"other"}"#.into())));
    let result = compile(include_str!(
        "../../../bots/builder/schemas/generate.result.json"
    ));
    for data in [r#"{"text":"t"}"#, r#"{"json":{"a":1}}"#, r#"{"json":null}"#] {
        assert!(result.admits(&Data::JsonObject(data.into())), "{data}");
    }
    for data in ["{}", r#"{"text":1}"#, r#"{"text":"t","json":1}"#] {
        assert!(!result.admits(&Data::JsonObject(data.into())), "{data}");
    }
}

#[test]
fn both_slots_are_required_and_the_package_declares_no_origin() {
    let manifest = scxml_validator_core::ServicePackageManifest::from_json(include_bytes!(
        "../package/generate.service.json"
    ))
    .expect("the manifest parses");
    assert!(
        manifest.http_origins.is_empty(),
        "no default backend: the baseurl value supplies the one origin"
    );
    assert_eq!(endpoint(Some(2), Some(1)), Ok((2, 1)));
    for (base_url, api_key) in [(None, Some(1)), (Some(2), None), (None, None)] {
        assert_eq!(
            endpoint(base_url, api_key),
            Err(FailureCode::Unconfigured),
            "{base_url:?} {api_key:?}"
        );
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
    assert_eq!(
        SERVICE_URI,
        format!("https://github.com/stateforward/floppy/services/{SERVICE_NAME}")
    );
    assert_eq!(
        manifest.service_artifact,
        "floppy_service_generate.virtualized.wasm"
    );
    assert_eq!(manifest.env_slots, [API_KEY_SLOT, BASE_URL_SLOT]);
}
