use super::*;

const SCHEMA: &str = r#"{"type":"object","properties":{"answer":{"type":"string"},"score":{"type":"integer"}},"required":["answer","score"],"additionalProperties":false}"#;

fn text(value: &str) -> Data {
    Data::String(value.into())
}

fn number(value: f64) -> Data {
    Data::Number(scxml_types::FiniteNumber::new(value).expect("a finite number"))
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

/// The answer of a stream without a schema.
fn reply_text_of(body: &[u8]) -> Result<String, FailureCode> {
    stream_reply(body)
        .and_then(|reply| reply.answer(None).map_err(|failure| failure.code))
        .map(|answer| answer["content"].as_str().unwrap_or_default().to_owned())
}

/// A non-streamed completion whose one tool call is `function`'s with
/// `arguments`, as a backend answers a required call (content `null`).
fn tool_completion(function: &str, arguments: &str) -> Vec<u8> {
    serde_json::json!({
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "finish_reason": "tool_calls",
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call-1",
                    "type": "function",
                    "function": { "name": function, "arguments": arguments },
                }],
            },
        }],
    })
    .to_string()
    .into_bytes()
}

/// A streamed reply of `function`'s call with `arguments`, its arguments
/// split in two `delta.tool_calls` fragments after the one naming it.
fn tool_reply(function: &str, arguments: &str) -> Vec<u8> {
    let middle = arguments.chars().count() / 2;
    let head: String = arguments.chars().take(middle).collect();
    let tail: String = arguments.chars().skip(middle).collect();
    let fragment = |value: Json| chunk(&serde_json::json!({ "tool_calls": [value] }), None);
    stream(&[
        chunk(
            &serde_json::json!({ "role": "assistant", "content": "" }),
            None,
        ),
        fragment(serde_json::json!({
            "index": 0, "id": "call-1", "type": "function",
            "function": { "name": function, "arguments": "" },
        })),
        fragment(serde_json::json!({ "index": 0, "function": { "arguments": head } })),
        fragment(serde_json::json!({ "index": 0, "function": { "arguments": tail } })),
        chunk(&serde_json::json!({}), Some("tool_calls")),
    ])
}

/// A non-streamed completion of `content`, finished by `finish_reason`.
fn completion(content: &str, finish_reason: &str) -> Vec<u8> {
    serde_json::json!({
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "finish_reason": finish_reason,
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
    let request = build_request(2, Some(1), None, &generation);
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
        r#"{"messages":[{"content":"Be brief.","role":"system"},{"content":"Say hello.","role":"user"}],"model":"mock-model","reasoning_effort":"low"}"#
    );
    assert_eq!(
        request.headers[2],
        (
            "accept".to_owned(),
            vec![Part::Literal("application/json".into())]
        )
    );
}

#[test]
fn a_streamed_generation_asks_for_server_sent_events() {
    let generation = parse(&[
        ("model", text("m")),
        ("prompt", text("p")),
        ("stream", Data::Bool(true)),
    ])
    .expect("valid parameters");
    let request = build_request(2, Some(1), None, &generation);
    assert_eq!(
        String::from_utf8(request.body).expect("UTF-8"),
        r#"{"messages":[{"content":"p","role":"user"}],"model":"m","reasoning_effort":"low","stream":true,"stream_options":{"include_usage":true}}"#
    );
    assert_eq!(
        request.headers[2],
        (
            "accept".to_owned(),
            vec![Part::Literal("text/event-stream".into())]
        )
    );
    // Its reply is the stream's text, and a backend that answers one
    // completion anyway is read as one (decision D101).
    assert_eq!(
        outcome(&generation, 0, ok(reply("hello there"))),
        Returned {
            name: RESULT_EVENT,
            data: r#"{"content":"hello there","request_bytes":0,"tool_calls":[]}"#.into()
        }
    );
    assert_eq!(
        outcome(&generation, 0, ok(completion("hi", "stop"))),
        Returned {
            name: RESULT_EVENT,
            data: r#"{"content":"hi","request_bytes":0,"tool_calls":[]}"#.into()
        }
    );
}

/// A request's bytes are its body's as the original counts them
/// (`OutputData.request_bytes`): everything but the streaming members.
#[test]
fn a_requests_bytes_are_its_body_without_the_streaming_members() {
    let plain = parse(&[("model", text("m")), ("prompt", text("héllo"))]).expect("valid");
    let request = build_request(2, Some(1), None, &plain);
    assert_eq!(request.request_bytes, request.body.len());
    let streamed = parse(&[
        ("model", text("m")),
        ("prompt", text("héllo")),
        ("stream", Data::Bool(true)),
    ])
    .expect("valid");
    let streamed = build_request(2, Some(1), None, &streamed);
    assert_eq!(streamed.request_bytes, request.body.len());
    assert!(streamed.body.len() > streamed.request_bytes);
    // The run's stream slot adds its member to the body as it is sent,
    // never to the count.
    assert_eq!(
        build_request(2, Some(1), Some(3), &plain).request_bytes,
        request.body.len()
    );
    // The result carries the bytes it is given.
    assert_eq!(
        outcome(&plain, request.request_bytes, ok(completion("hi", "stop"))).data,
        format!(
            r#"{{"content":"hi","request_bytes":{},"tool_calls":[]}}"#,
            request.request_bytes
        )
    );
}

/// The usage a reply's backend reports is the result's `usage`, as the
/// original's `Usage` reads it: the completion's, or the last stream chunk
/// that carries one; a count left out is absent, and no count is no usage.
#[test]
fn a_replys_usage_is_the_results_usage() {
    let generation = parse(&[("model", text("m")), ("prompt", text("p"))]).expect("valid");
    let mut counted: Json = serde_json::from_slice(&completion("hi", "stop")).expect("JSON");
    counted["usage"] = serde_json::json!({
        "prompt_tokens": 7, "completion_tokens": 5, "total_tokens": 12,
        "completion_tokens_details": { "reasoning_tokens": 3 },
    });
    assert_eq!(
        outcome(&generation, 9, ok(counted.to_string().into_bytes())).data,
        r#"{"content":"hi","request_bytes":9,"tool_calls":[],"usage":{"input_tokens":7,"output_tokens":5,"reasoning_tokens":3,"total_tokens":12}}"#
    );
    // A count that is not a whole number is left out; no count, no usage.
    counted["usage"] =
        serde_json::json!({ "prompt_tokens": 7, "completion_tokens": -1, "total_tokens": "12" });
    assert_eq!(
        outcome(&generation, 9, ok(counted.to_string().into_bytes())).data,
        r#"{"content":"hi","request_bytes":9,"tool_calls":[],"usage":{"input_tokens":7}}"#
    );
    counted["usage"] = serde_json::json!({ "completion_tokens_details": null });
    assert_eq!(
        outcome(&generation, 9, ok(counted.to_string().into_bytes())).data,
        r#"{"content":"hi","request_bytes":9,"tool_calls":[]}"#
    );
    // A stream's usage arrives on a last chunk without choices
    // (`include_usage`); a `null` usage on the others leaves it.
    let mut first = chunk(&serde_json::json!({ "content": "hi" }), Some("stop"));
    first["usage"] = Json::Null;
    let body = stream(&[
        first,
        serde_json::json!({ "choices": [], "usage": { "prompt_tokens": 4, "completion_tokens": 1, "total_tokens": 5 } }),
    ]);
    assert_eq!(
        outcome(&generation, 9, ok(body)).data,
        r#"{"content":"hi","request_bytes":9,"tool_calls":[],"usage":{"input_tokens":4,"output_tokens":1,"total_tokens":5}}"#
    );
    // A failure carries neither.
    counted["choices"][0]["finish_reason"] = "length".into();
    assert_eq!(
        outcome(&generation, 9, ok(counted.to_string().into_bytes())),
        Returned::failed(FailureCode::Truncated)
    );
}

/// The run's `stream` slot decides whether a call streams (decision D101):
/// its reference is the body's last member, and the accept header takes
/// either answer.
#[test]
fn the_runs_stream_slot_decides_the_stream_member() {
    let generation = parse(&[("model", text("m")), ("prompt", text("p"))]).expect("valid");
    let request = build_request(2, Some(1), Some(3), &generation);
    assert_eq!(
        request.body_parts(),
        [
            Part::Literal(
                r#"{"messages":[{"content":"p","role":"user"}],"model":"m","reasoning_effort":"low","stream":"#
                    .into()
            ),
            Part::Reference(3),
            Part::Literal("}".into()),
        ]
    );
    assert!(request.headers.contains(&(
        "accept".to_owned(),
        vec![Part::Literal("application/json, text/event-stream".into())]
    )));
    // Without the slot the call is not streamed.
    let plain = build_request(2, Some(1), None, &generation);
    assert_eq!(
        plain.body_parts(),
        [Part::Literal(
            String::from_utf8(plain.body.clone()).expect("UTF-8")
        )]
    );
    // An invocation that asks for a stream streams whatever the slot says.
    let streamed = parse(&[
        ("model", text("m")),
        ("prompt", text("p")),
        ("stream", Data::Bool(true)),
    ])
    .expect("valid");
    assert_eq!(build_request(2, Some(1), Some(3), &streamed).stream, None);
}

/// `max_completion_tokens` bounds a reply's tokens (decision D101): a model
/// that writes on and on stops there and the call fails as `truncated`.
#[test]
fn max_completion_tokens_is_sent_and_bounded() {
    let generation = parse(&[
        ("model", text("m")),
        ("prompt", text("p")),
        ("max_completion_tokens", number(8192.0)),
    ])
    .expect("valid");
    let body: Json =
        serde_json::from_slice(&build_request(2, Some(1), None, &generation).body).expect("JSON");
    assert_eq!(body["max_completion_tokens"], 8192);
    for bad in [number(0.0), number(1.5), number(2_000_000.0), text("8192")] {
        assert!(
            parse(&[
                ("model", text("m")),
                ("prompt", text("p")),
                ("max_completion_tokens", bad)
            ])
            .is_err()
        );
    }
    assert_eq!(
        outcome(&generation, 0, ok(completion("cut", "length"))),
        Returned::failed(FailureCode::Truncated)
    );
}

/// The original's sampling and token bounds (its image decoder's
/// `temperature` 0 and 512 tokens; vLLM's `thinking_token_budget`) are sent
/// as given and bounded.
#[test]
fn temperature_and_the_thinking_budget_are_sent_and_bounded() {
    let generation = parse(&[
        ("model", text("m")),
        ("prompt", text("p")),
        ("temperature", number(0.0)),
        ("max_completion_tokens", number(512.0)),
        ("thinking_token_budget", number(1024.0)),
    ])
    .expect("valid");
    let body: Json =
        serde_json::from_slice(&build_request(2, Some(1), None, &generation).body).expect("JSON");
    assert_eq!(body["temperature"], 0.0);
    assert_eq!(body["max_completion_tokens"], 512);
    assert_eq!(body["thinking_token_budget"], 1024);
    let warm = parse(&[
        ("model", text("m")),
        ("prompt", text("p")),
        ("temperature", number(0.7)),
    ])
    .expect("valid");
    let body: Json =
        serde_json::from_slice(&build_request(2, Some(1), None, &warm).body).expect("JSON");
    assert_eq!(body["temperature"], 0.7);
    assert!(body.get("thinking_token_budget").is_none(), "{body}");
    for (name, bad) in [
        ("temperature", number(-0.1)),
        ("temperature", number(2.5)),
        ("temperature", text("0")),
        ("thinking_token_budget", number(0.0)),
        ("thinking_token_budget", number(1.5)),
        ("thinking_token_budget", number(2_000_000.0)),
        ("thinking_token_budget", text("1024")),
    ] {
        assert!(
            parse(&[("model", text("m")), ("prompt", text("p")), (name, bad)]).is_err(),
            "{name}"
        );
    }
}

/// `tool_choice` `auto` lets the model answer in its content, as the
/// original's processor allows: a reply with no call is read as one call of
/// the tool whose arguments are the content, checked as any call's, and
/// blank content selects nothing; a reply with calls is read as before.
#[test]
fn under_auto_a_reply_without_a_call_is_read_from_its_content() {
    let auto = parse(&[
        ("model", text("m")),
        ("prompt", text("p")),
        ("schema", text(SCHEMA)),
        ("schemaname", text("verdict")),
        ("tool_choice", text("auto")),
    ])
    .expect("valid");
    let body: Json =
        serde_json::from_slice(&build_request(2, Some(1), None, &auto).body).expect("JSON");
    assert_eq!(body["tool_choice"], "auto");
    let arguments = r#"{"score":41,"answer":"yes"}"#;
    let from_content = Returned {
        name: RESULT_EVENT,
        data: scxml_event_schema::canonical_json(&serde_json::json!({
            "content": arguments,
            "request_bytes": 0,
            "tool_calls": [{"id": CONTENT_CALL_ID, "name": "verdict", "args": {"answer": "yes", "score": 41}}],
        })),
    };
    assert_eq!(
        outcome(&auto, 0, ok(completion(arguments, "stop"))),
        from_content
    );
    assert_eq!(outcome(&auto, 0, ok(reply(arguments))), from_content);
    assert_eq!(
        outcome(&auto, 0, ok(completion(" ", "stop"))),
        Returned {
            name: RESULT_EVENT,
            data: r#"{"content":" ","request_bytes":0,"tool_calls":[]}"#.into()
        }
    );
    assert_eq!(
        outcome(&auto, 0, ok(completion(r#"{"answer":"yes"}"#, "stop"))),
        rejected(
            FailureCode::SchemaMismatch,
            "schema-mismatch:",
            "(root): missing properties 'score'"
        )
    );
    assert_eq!(
        outcome(&auto, 0, ok(completion("Yes, 41.", "stop"))),
        rejected(
            FailureCode::NotJson,
            "not-json",
            "the arguments are not JSON: expected value at line 1 column 1"
        )
    );
    // A call is read as under `required`.
    assert_eq!(
        outcome(&auto, 0, ok(tool_completion("verdict", arguments))),
        calls_result(
            "",
            &[(
                "call-1",
                "verdict",
                serde_json::json!({"answer": "yes", "score": 41})
            )]
        )
    );
    // `required` stays the default, and only a schema takes a tool choice.
    let required = parse(&[
        ("model", text("m")),
        ("prompt", text("p")),
        ("schema", text(SCHEMA)),
    ])
    .expect("valid");
    assert_eq!(
        required.schema.as_ref().map(|tool| tool.tool_choice),
        Some(TOOL_CHOICE)
    );
    assert_eq!(
        outcome(&required, 0, ok(completion(arguments, "stop"))),
        Returned::failed(FailureCode::Malformed)
    );
    for parameters in [
        vec![
            ("model", text("m")),
            ("prompt", text("p")),
            ("tool_choice", text("auto")),
        ],
        vec![
            ("model", text("m")),
            ("prompt", text("p")),
            ("schema", text(SCHEMA)),
            ("tool_choice", text("none")),
        ],
    ] {
        assert!(parse(&parameters).is_err(), "{parameters:?}");
    }
}

/// Arguments a local model encoded as JSON strings are decoded once, as the
/// original's text generator and processing decode them: the whole
/// arguments, and each string where the schema takes only an array or an
/// object (through `$ref` and the first admitting `anyOf` branch); a string
/// the schema also takes as a string is kept.
#[test]
fn string_encoded_arguments_are_decoded_once() {
    let schema = serde_json::json!({
        "type": "object",
        "$defs": {"say": {
            "type": "object", "required": ["event", "data"], "additionalProperties": false,
            "properties": {"event": {"const": "bot.say"}, "data": {
                "type": "object", "required": ["text"], "additionalProperties": false,
                "properties": {"text": {"type": "string"}},
            }},
        }, "wait": {
            "type": "object", "required": ["event", "data"], "additionalProperties": false,
            "properties": {"event": {"const": "bot.wait"}, "data": {
                "type": "object", "required": ["seconds"], "additionalProperties": false,
                "properties": {"seconds": {"type": "integer"}},
            }},
        }},
        "properties": {
            "events": {"type": "array", "items": {"anyOf": [{"$ref": "#/$defs/say"}, {"$ref": "#/$defs/wait"}]}},
            "note": {"type": ["object", "string"]},
        },
        "required": ["events"],
    })
    .to_string();
    let generation = parse(&[
        ("model", text("m")),
        ("prompt", text("p")),
        ("schema", text(&schema)),
        ("schemaname", text("dispatch")),
    ])
    .expect("valid");
    let decoded = serde_json::json!({
        "events": [
            {"event": "bot.say", "data": {"text": "{\"not\": \"decoded\"}"}},
            {"event": "bot.wait", "data": {"seconds": 2}},
        ],
        "note": "{\"kept\": true}",
    });
    let answered = calls_result("", &[("call-1", "dispatch", decoded.clone())]);
    let events = serde_json::json!([
        {"event": "bot.say", "data": {"text": "{\"not\": \"decoded\"}"}},
        {"event": "bot.wait", "data": "{\"seconds\": 2}"},
    ]);
    for arguments in [
        // Each nested level a string.
        serde_json::json!({"events": events.to_string(), "note": decoded["note"]}).to_string(),
        // The whole arguments a JSON string of the object.
        Json::String(decoded.to_string()).to_string(),
    ] {
        assert_eq!(
            outcome(&generation, 0, ok(tool_completion("dispatch", &arguments))),
            answered,
            "{arguments}"
        );
    }
    // A string that is not JSON, or holds neither an array nor an object,
    // where the schema takes only those, is not JSON.
    for (arguments, message) in [
        (
            r#"{"events":"[{\"event\":"}"#,
            "/events: a string-encoded value is not JSON: EOF while parsing a value at line 1 column 10",
        ),
        (
            r#"{"events":"7"}"#,
            "/events: a string-encoded value holds neither the array nor the object expected",
        ),
        (r#""[1]""#, "the arguments are an array, not a JSON object"),
    ] {
        assert_eq!(
            outcome(&generation, 0, ok(tool_completion("dispatch", arguments))),
            rejected(FailureCode::NotJson, "not-json", message),
            "{arguments}"
        );
    }
    // A string no branch decodes to is kept, and the check refuses it.
    assert_eq!(
        outcome(
            &generation,
            0,
            ok(tool_completion(
                "dispatch",
                r#"{"events":[{"event":"bot.wait","data":"{\"seconds\":\"two\"}"}]}"#
            ))
        ),
        rejected(
            FailureCode::SchemaMismatch,
            "schema-mismatch:/events/0/data",
            "/events/0/data: want object, but got string"
        )
    );
}

/// The `$ref` bound counts only a chain of `$ref`s with no value between
/// them, so every item of a long dispatch is decoded through its branches'
/// `$ref`s, and a schema that refers to itself with no value between ends.
#[test]
fn a_ref_bound_counts_a_chain_with_no_value_between() {
    let schema = serde_json::json!({
        "type": "object",
        "$defs": {"wait": {
            "type": "object", "required": ["event", "data"], "additionalProperties": false,
            "properties": {"event": {"const": "bot.wait"}, "data": {
                "type": "object", "required": ["seconds"], "additionalProperties": false,
                "properties": {"seconds": {"type": "integer"}},
            }},
        }, "say": {"$ref": "#/$defs/speak"}, "speak": {
            "type": "object", "required": ["event"],
            "properties": {"event": {"const": "bot.say"}},
        }},
        "properties": {
            "events": {"type": "array", "items": {"anyOf": [{"$ref": "#/$defs/say"}, {"$ref": "#/$defs/wait"}]}},
        },
        "required": ["events"],
    })
    .to_string();
    let generation = parse(&[
        ("model", text("m")),
        ("prompt", text("p")),
        ("schema", text(&schema)),
        ("schemaname", text("dispatch")),
    ])
    .expect("valid");
    let items = 2 * MAX_DECODED_REFS;
    let sent: Vec<Json> = (0..items)
        .map(|_| serde_json::json!({"event": "bot.wait", "data": "{\"seconds\": 2}"}))
        .collect();
    let decoded: Vec<Json> = (0..items)
        .map(|_| serde_json::json!({"event": "bot.wait", "data": {"seconds": 2}}))
        .collect();
    assert_eq!(
        outcome(
            &generation,
            0,
            ok(tool_completion(
                "dispatch",
                &serde_json::json!({"events": sent}).to_string()
            ))
        ),
        calls_result(
            "",
            &[("call-1", "dispatch", serde_json::json!({"events": decoded}))]
        )
    );
    // A `$ref` to itself ends at the bound: its string is kept, and the
    // check takes it.
    let looped = parse(&[
        ("model", text("m")),
        ("prompt", text("p")),
        (
            "schema",
            text(
                &serde_json::json!({
                    "type": "object",
                    "$defs": {"loop": {"anyOf": [{"$ref": "#/$defs/loop"}, {"type": "string"}]}},
                    "properties": {"note": {"$ref": "#/$defs/loop"}},
                })
                .to_string(),
            ),
        ),
        ("schemaname", text("note")),
    ])
    .expect("valid");
    assert_eq!(
        outcome(&looped, 0, ok(tool_completion("note", r#"{"note":"[1]"}"#))),
        calls_result(
            "",
            &[("call-1", "note", serde_json::json!({"note": "[1]"}))]
        )
    );
}

#[test]
fn a_schema_is_the_one_tool_and_a_call_is_required() {
    let generation = parse(&[
        ("model", text("m")),
        ("prompt", text("Decide.")),
        ("schema", Data::JsonObject(SCHEMA.into())),
        ("schemaname", text("verdict")),
    ])
    .expect("valid parameters");
    let body: Json =
        serde_json::from_slice(&build_request(2, Some(1), None, &generation).body).expect("JSON");
    assert!(body.get("response_format").is_none(), "{body}");
    assert_eq!(body["tools"].as_array().map(Vec::len), Some(1), "{body}");
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["function"]["name"], "verdict");
    assert!(
        body["tools"][0]["function"].get("strict").is_none(),
        "{body}"
    );
    assert_eq!(body["tool_choice"], "required");
    // With tools and no effort asked, the effort is none.
    assert_eq!(body["reasoning_effort"], "none");
    // A schema given as a string is the same request.
    let from_text = parse(&[
        ("model", text("m")),
        ("prompt", text("Decide.")),
        ("schema", text(SCHEMA)),
    ])
    .expect("valid parameters");
    let text_body: Json =
        serde_json::from_slice(&build_request(2, Some(1), None, &from_text).body).expect("JSON");
    assert_eq!(
        text_body["tools"][0]["function"]["name"],
        DEFAULT_SCHEMA_NAME
    );
    assert_eq!(text_body["tool_choice"], "required");
    // Without a schema, no tool is offered.
    let plain = parse(&[("model", text("m")), ("prompt", text("Say."))]).expect("valid");
    let plain_body: Json =
        serde_json::from_slice(&build_request(2, Some(1), None, &plain).body).expect("JSON");
    assert!(plain_body.get("tools").is_none() && plain_body.get("tool_choice").is_none());
}

#[test]
fn the_schemas_root_description_is_the_functions_and_the_rest_its_parameters() {
    let schema = r#"{"description":"Pick one.","type":"object","title":"t","properties":{"data":{"type":"object","description":"kept","properties":{"name":{"type":"string"}},"required":["name"]}},"required":["data"]}"#;
    let generation = parse(&[
        ("model", text("m")),
        ("prompt", text("Decide.")),
        ("schema", text(schema)),
    ])
    .expect("valid parameters");
    let body: Json =
        serde_json::from_slice(&build_request(2, Some(1), None, &generation).body).expect("JSON");
    assert_eq!(body["tools"][0]["function"]["description"], "Pick one.");
    // The rest is sent as given: nothing is normalized.
    assert_eq!(
        body["tools"][0]["function"]["parameters"],
        serde_json::json!({
            "type": "object",
            "title": "t",
            "properties": {"data": {"type": "object", "description": "kept", "properties": {"name": {"type": "string"}}, "required": ["name"]}},
            "required": ["data"]
        })
    );
    // Without a root description the function has none.
    let plain = parse(&[
        ("model", text("m")),
        ("prompt", text("Decide.")),
        ("schema", text(SCHEMA)),
    ])
    .expect("valid parameters");
    let body: Json =
        serde_json::from_slice(&build_request(2, Some(1), None, &plain).body).expect("JSON");
    assert!(
        body["tools"][0]["function"].get("description").is_none(),
        "{body}"
    );
}

#[test]
fn reasoning_effort_is_sent_as_given() {
    let generation = parse(&[
        ("model", text("m")),
        ("prompt", text("p")),
        ("reasoning_effort", text("high")),
    ])
    .expect("valid parameters");
    let body: Json =
        serde_json::from_slice(&build_request(2, Some(1), None, &generation).body).expect("JSON");
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
    let request = build_request(0, Some(1), None, &generation);
    assert_eq!(
        String::from_utf8(request.body).expect("UTF-8"),
        r#"{"messages":[{"content":[{"text":"Read all visible text in this image. Return only the text.","type":"text"},{"image_url":{"url":"data:image/png;base64,iVBORw0KGgphYg=="},"type":"image_url"}],"role":"user"}],"model":"m","reasoning_effort":"low"}"#,
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
    // GIF is not one of the original's image media types.
    assert_eq!(image_media_type(b"GIF89a.."), None);
    assert_eq!(data_url(b"GIF89a.."), None);
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
            "strict, which generate does not take",
        ),
        (
            vec![
                model.clone(),
                prompt.clone(),
                ("schemaname", text("dispatch")),
            ],
            "a schema name without a schema",
        ),
        (
            vec![model.clone(), prompt.clone(), ("stream", text("true"))],
            "stream as text",
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
        outcome(&generation, 0, ok(completion("hello there", "stop"))),
        Returned {
            name: RESULT_EVENT,
            data: r#"{"content":"hello there","request_bytes":0,"tool_calls":[]}"#.into()
        }
    );
    // A streamed reply is read as one (the backend decides, decision D101).
    assert_eq!(
        outcome(&generation, 0, ok(reply("hello there"))),
        Returned {
            name: RESULT_EVENT,
            data: r#"{"content":"hello there","request_bytes":0,"tool_calls":[]}"#.into()
        }
    );
}

/// The result of a reply whose calls are `calls`, `(id, name, args)`, and
/// whose content is `content`.
fn calls_result(content: &str, calls: &[(&str, &str, Json)]) -> Returned {
    let calls: Vec<Json> = calls
        .iter()
        .map(|(id, name, args)| serde_json::json!({ "id": id, "name": name, "args": args }))
        .collect();
    Returned {
        name: RESULT_EVENT,
        data: scxml_event_schema::canonical_json(
            &serde_json::json!({ "content": content, "request_bytes": 0, "tool_calls": calls }),
        ),
    }
}

/// [`FAILED_EVENT`] with `code` and the rejection `{key, message}`.
fn rejected(code: FailureCode, key: &str, message: &str) -> Returned {
    Returned::failure(&Failure {
        code,
        rejection: Some(Rejection {
            key: key.to_owned(),
            message: message.to_owned(),
        }),
    })
}

#[test]
fn a_tool_reply_returns_every_call_its_schema_admits() {
    let generation = parse(&[
        ("model", text("m")),
        ("prompt", text("p")),
        ("schema", text(SCHEMA)),
    ])
    .expect("valid");
    let arguments = r#"{"score":41,"answer":"yes"}"#;
    let answered = calls_result(
        "",
        &[(
            "call-1",
            DEFAULT_SCHEMA_NAME,
            serde_json::json!({"answer": "yes", "score": 41}),
        )],
    );
    assert_eq!(
        outcome(
            &generation,
            0,
            ok(tool_completion(DEFAULT_SCHEMA_NAME, arguments))
        ),
        answered
    );
    // A streamed call's argument fragments are joined in order.
    assert_eq!(
        outcome(
            &generation,
            0,
            ok(tool_reply(DEFAULT_SCHEMA_NAME, arguments))
        ),
        answered
    );
    // Every call is checked against the one function it was offered before
    // anything is returned: arguments its schema does not admit, or a call
    // of a function the request did not offer (the original's processing:
    // "expected tool 'dispatch', got: ..."), are a schema mismatch.
    assert_eq!(
        outcome(
            &generation,
            0,
            ok(tool_completion(DEFAULT_SCHEMA_NAME, r#"{"answer":"yes"}"#))
        ),
        rejected(
            FailureCode::SchemaMismatch,
            "schema-mismatch:",
            "(root): missing properties 'score'"
        )
    );
    assert_eq!(
        outcome(
            &generation,
            0,
            ok(tool_reply(DEFAULT_SCHEMA_NAME, r#"{"answer":"yes"}"#))
        ),
        rejected(
            FailureCode::SchemaMismatch,
            "schema-mismatch:",
            "(root): missing properties 'score'"
        )
    );
    let other = rejected(
        FailureCode::SchemaMismatch,
        "schema-mismatch:name",
        "the call names a function the request did not offer: expected 'result', got 'other'",
    );
    assert_eq!(
        outcome(&generation, 0, ok(tool_reply("other", arguments))),
        other
    );
    assert_eq!(
        outcome(&generation, 0, ok(tool_completion("other", arguments))),
        other
    );
}

/// Arguments that are not a JSON object are not JSON, with the decode
/// error's position and never the text.
#[test]
fn a_calls_arguments_that_are_not_a_json_object_are_not_json() {
    let generation = parse(&[
        ("model", text("m")),
        ("prompt", text("p")),
        ("schema", text(SCHEMA)),
    ])
    .expect("valid");
    for (arguments, message) in [
        (
            "not json",
            "the arguments are not JSON: expected ident at line 1 column 2",
        ),
        (
            "",
            "the arguments are not JSON: EOF while parsing a value at line 1 column 0",
        ),
        ("[]", "the arguments are an array, not a JSON object"),
    ] {
        assert_eq!(
            outcome(
                &generation,
                0,
                ok(tool_completion(DEFAULT_SCHEMA_NAME, arguments))
            ),
            rejected(FailureCode::NotJson, "not-json", message),
            "{arguments}"
        );
    }
    assert_eq!(
        outcome(
            &generation,
            0,
            ok(tool_reply(DEFAULT_SCHEMA_NAME, "not json"))
        ),
        rejected(
            FailureCode::NotJson,
            "not-json",
            "the arguments are not JSON: expected ident at line 1 column 2"
        )
    );
}

#[test]
fn a_calls_undeclared_top_level_fields_are_dropped_and_nested_ones_still_fail() {
    // A dispatch-shaped schema: only `events` at the root, each event
    // closed (`additionalProperties: false`, `data` required).
    let schema = r#"{"type":"object","additionalProperties":false,"required":["events"],"properties":{"events":{"type":"array","items":{"type":"object","additionalProperties":false,"required":["event","data"],"properties":{"event":{"enum":["bot.say"]},"data":{"type":"object","additionalProperties":false,"required":["text"],"properties":{"text":{"type":"string"}}},"reason":{"type":"string"}}}}}}"#;
    let generation = parse(&[
        ("model", text("m")),
        ("prompt", text("p")),
        ("schema", text(schema)),
        ("schemaname", text("dispatch")),
    ])
    .expect("valid");
    // A stray `reason` beside `events` is not the tool's: it is dropped
    // before the check and not returned (the original reads only
    // `args.events`).
    let event = serde_json::json!({"event": "bot.say", "data": {"text": "hi"}, "reason": "asked"});
    let stray = serde_json::json!({"events": [event], "reason": "why"}).to_string();
    let answered = calls_result(
        "",
        &[("call-1", "dispatch", serde_json::json!({"events": [event]}))],
    );
    assert_eq!(
        outcome(&generation, 0, ok(tool_completion("dispatch", &stray))),
        answered
    );
    assert_eq!(
        outcome(&generation, 0, ok(tool_reply("dispatch", &stray))),
        answered
    );
    // Inside an event the schema stays strict: an unoffered field, or an
    // event missing its required `data`, is a schema mismatch.
    for (arguments, key, message) in [
        (
            r#"{"events":[{"event":"bot.say","data":{"text":"hi"},"lifetime":"persistent"}]}"#,
            "schema-mismatch:/events/0",
            "/events/0: additional properties 'lifetime' not allowed",
        ),
        (
            r#"{"events":[{"event":"bot.say","data":{"text":"hi","lifetime":"persistent"}}]}"#,
            "schema-mismatch:/events/0/data",
            "/events/0/data: additional properties 'lifetime' not allowed",
        ),
        (
            r#"{"events":[{"event":"bot.say","text":"hi"}],"reason":"why"}"#,
            "schema-mismatch:/events/0",
            "/events/0: missing properties 'data'",
        ),
    ] {
        assert_eq!(
            outcome(&generation, 0, ok(tool_completion("dispatch", arguments))),
            rejected(FailureCode::SchemaMismatch, key, message),
            "{arguments}"
        );
    }
}

/// A refused call's rejection names where its arguments first fail, as the
/// original's selection rejection names the event and the failing keys: in
/// the dispatch tool's `anyOf` of one branch per event, the branch of the
/// event the call names, and an event no branch offers at its `event`.
#[test]
fn a_rejection_names_where_the_arguments_first_fail_and_never_a_value() {
    let branch = |event: &str, data: &Json| {
        serde_json::json!({
            "type": "object", "additionalProperties": false, "required": ["event", "data"],
            "properties": { "event": { "const": event }, "data": data },
        })
    };
    let say = serde_json::json!({
        "type": "object", "additionalProperties": false, "required": ["text"],
        "properties": { "text": { "type": "string", "maxLength": 8 } },
    });
    let wait = serde_json::json!({
        "type": "object", "additionalProperties": false, "required": ["seconds"],
        "properties": { "seconds": { "type": "integer", "minimum": 1 }, "code": { "type": "string", "pattern": "^[a-z]+$" } },
    });
    let schema = serde_json::json!({
        "type": "object", "required": ["events"],
        "properties": { "events": { "type": "array", "items": { "anyOf": [branch("bot.say", &say), branch("bot.wait", &wait)] } } },
    })
    .to_string();
    let generation = parse(&[
        ("model", text("m")),
        ("prompt", text("p")),
        ("schema", text(&schema)),
        ("schemaname", text("dispatch")),
    ])
    .expect("valid");
    let secret = "a value the model wrote that no rejection repeats";
    for (arguments, key, message) in [
        (
            serde_json::json!({"events": [{"event": "bot.say", "data": {"text": "hi"}}, {"event": "bot.wait", "data": {"seconds": 0}}]}),
            "schema-mismatch:/events/1/data/seconds",
            "/events/1/data/seconds: value must be at least 1",
        ),
        (
            serde_json::json!({"events": [{"event": "bot.say", "data": {"text": secret}}]}),
            "schema-mismatch:/events/0/data/text",
            "/events/0/data/text: length must be <=8, but got 49",
        ),
        (
            serde_json::json!({"events": [{"event": "bot.wait", "data": {"seconds": 2, "code": secret}}]}),
            "schema-mismatch:/events/0/data/code",
            "/events/0/data/code: value does not match the pattern ^[a-z]+$",
        ),
        (
            serde_json::json!({"events": [{"event": "bot.say", "data": {"text": "hi", "reason": secret}}]}),
            "schema-mismatch:/events/0/data",
            "/events/0/data: additional properties 'reason' not allowed",
        ),
        (
            serde_json::json!({"events": [{"event": "bot.wait"}]}),
            "schema-mismatch:/events/0",
            "/events/0: missing properties 'data'",
        ),
        (
            serde_json::json!({"events": [{"event": "action:speech.say", "data": {"text": secret}}]}),
            "schema-mismatch:/events/0/event",
            r#"/events/0/event: value must be one of "bot.say", "bot.wait""#,
        ),
        (
            serde_json::json!({"events": 7}),
            "schema-mismatch:/events",
            "/events: want array, but got number",
        ),
    ] {
        let returned = outcome(
            &generation,
            0,
            ok(tool_completion("dispatch", &arguments.to_string())),
        );
        assert_eq!(
            returned,
            rejected(FailureCode::SchemaMismatch, key, message),
            "{arguments}"
        );
        assert!(!returned.data.contains(secret), "{}", returned.data);
    }
    // A name the model wrote is cut, and the rejection stays one line.
    let long = format!("other\n{}", "x".repeat(300));
    let Returned { data, .. } = outcome(&generation, 0, ok(tool_completion(&long, "{}")));
    let data: Json = serde_json::from_str(&data).expect("JSON");
    let message = data["rejection"]["message"].as_str().expect("a message");
    assert!(message.chars().count() <= MAX_REJECTION_CHARS, "{message}");
    assert!(!message.contains('\n'), "{message}");
    assert!(message.contains("got 'other x"), "{message}");
}

#[test]
fn a_tool_reply_needs_a_call_and_keeps_its_calls_in_index_order() {
    let generation = parse(&[
        ("model", text("m")),
        ("prompt", text("p")),
        ("schema", text(SCHEMA)),
    ])
    .expect("valid");
    let arguments = r#"{"score":41,"answer":"yes"}"#;
    // JSON content without a tool call is no reply, streamed or not: the
    // original's "did not include a required tool call".
    assert_eq!(
        outcome(&generation, 0, ok(completion(arguments, "stop"))),
        Returned::failed(FailureCode::Malformed)
    );
    assert_eq!(
        outcome(&generation, 0, ok(reply(arguments))),
        Returned::failed(FailureCode::Malformed)
    );
    // Each fragment goes to the call its index names, and every call is
    // returned in index order, with the content beside them.
    let later = stream(&[chunk(
        &serde_json::json!({ "content": "Calling.", "tool_calls": [
                { "index": 1, "id": "call-2", "function": { "name": DEFAULT_SCHEMA_NAME, "arguments": r#"{"answer":"no","score":1}"# } },
                { "index": 0, "id": "call-1", "function": { "name": DEFAULT_SCHEMA_NAME, "arguments": arguments } },
            ] }),
        Some("tool_calls"),
    )]);
    assert_eq!(
        outcome(&generation, 0, ok(later)),
        calls_result(
            "Calling.",
            &[
                (
                    "call-1",
                    DEFAULT_SCHEMA_NAME,
                    serde_json::json!({"answer": "yes", "score": 41})
                ),
                (
                    "call-2",
                    DEFAULT_SCHEMA_NAME,
                    serde_json::json!({"answer": "no", "score": 1})
                ),
            ]
        )
    );
    // Without a schema, a tool call is not the reply's text.
    let plain = parse(&[("model", text("m")), ("prompt", text("p"))]).expect("valid");
    assert_eq!(
        outcome(
            &plain,
            0,
            ok(tool_completion(DEFAULT_SCHEMA_NAME, arguments))
        ),
        Returned::failed(FailureCode::Malformed)
    );
}

#[test]
fn every_failure_has_its_closed_code() {
    let generation = parse(&[("model", text("m")), ("prompt", text("p"))]).expect("valid");
    let failed = |answered| outcome(&generation, 0, answered);
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
        failed(ok(completion("cut", "length"))),
        Returned::failed(FailureCode::Truncated)
    );
    assert_eq!(
        failed(ok(completion("", "content_filter"))),
        Returned::failed(FailureCode::Filtered)
    );
    let refused = serde_json::json!({
        "choices": [{ "index": 0, "finish_reason": "stop", "message": { "content": null, "refusal": "no" } }],
    });
    assert_eq!(
        failed(ok(refused.to_string().into_bytes())),
        Returned::failed(FailureCode::Refusal)
    );
    assert_eq!(
        failed(ok(completion("", "stop"))),
        Returned::failed(FailureCode::Malformed),
        "a completion without content"
    );
    assert_eq!(
        failed(ok(br#"{"error":{"message":"boom"}}"#.to_vec())),
        Returned::failed(FailureCode::Malformed)
    );
    // The streamed failures, as a streamed generation reads them.
    let streamed = parse(&[
        ("model", text("m")),
        ("prompt", text("p")),
        ("stream", Data::Bool(true)),
    ])
    .expect("valid");
    let failed = |answered| outcome(&streamed, 0, answered);
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
    assert_eq!(reply_text_of(body.as_bytes()), Ok("Hello".to_owned()));
    // A multi-byte character split across chunks survives.
    assert_eq!(
        reply_text_of(&reply("héllo wörld")),
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
        reply_text_of(&stream(&[
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
        reply_text_of(&stream(&[opening.clone(), reasoning.clone(), spent])),
        Err(FailureCode::Truncated)
    );
    let stopped = chunk(&serde_json::json!({}), Some("stop"));
    assert_eq!(
        reply_text_of(&stream(&[opening, reasoning, stopped])),
        Err(FailureCode::Malformed)
    );
}

#[test]
fn a_stream_that_is_cut_short_or_malformed_is_never_a_reply() {
    let content = chunk(&serde_json::json!({ "content": "partial" }), None);
    let without_done = format!("data: {content}\n\n").into_bytes();
    assert_eq!(reply_text_of(&without_done), Err(FailureCode::Malformed));
    assert_eq!(
        reply_text_of(b"data: not json\n\ndata: [DONE]\n\n"),
        Err(FailureCode::Malformed)
    );
    assert_eq!(
        reply_text_of(&stream(&[
            serde_json::json!({ "error": { "message": "boom" } })
        ])),
        Err(FailureCode::Malformed)
    );
    assert_eq!(
        reply_text_of(&stream(&[chunk(&serde_json::json!({}), Some("stop"))])),
        Err(FailureCode::Malformed),
        "a stream without content"
    );
    assert_eq!(reply_text_of(&[0xff, 0xfe]), Err(FailureCode::Malformed));
    // A non-streamed completion is not a stream.
    let completion = serde_json::json!({
        "choices": [{ "index": 0, "finish_reason": "stop", "message": { "content": "hi" } }],
    });
    assert_eq!(
        reply_text_of(completion.to_string().as_bytes()),
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
    let rejection = rejected(FailureCode::SchemaMismatch, "schema-mismatch:/a", "/a: m");
    assert!(failed.admits(&Data::JsonObject(rejection.data)));
    assert!(failed.admits(&Data::JsonObject(
        rejected(FailureCode::NotJson, "not-json", "m").data
    )));
    for data in [
        r#"{"content":"t","request_bytes":0,"tool_calls":[]}"#,
        r#"{"content":"","request_bytes":12,"tool_calls":[{"args":{"a":1},"id":"c","name":"dispatch"}]}"#,
        r#"{"content":"t","request_bytes":3,"tool_calls":[],"usage":{"input_tokens":1,"output_tokens":2,"reasoning_tokens":0,"total_tokens":3}}"#,
        r#"{"content":"t","request_bytes":3,"tool_calls":[],"usage":{"input_tokens":1}}"#,
    ] {
        assert!(result.admits(&Data::JsonObject(data.into())), "{data}");
    }
    for data in [
        "{}",
        r#"{"text":"t"}"#,
        r#"{"content":"t","request_bytes":0}"#,
        r#"{"content":"t","tool_calls":[]}"#,
        r#"{"content":"","request_bytes":0,"tool_calls":[{"args":[],"id":"c","name":"dispatch"}]}"#,
        r#"{"content":"t","request_bytes":-1,"tool_calls":[]}"#,
        r#"{"content":"t","request_bytes":0,"tool_calls":[],"usage":{"cached_tokens":1}}"#,
    ] {
        assert!(!result.admits(&Data::JsonObject(data.into())), "{data}");
    }
}

/// The base URL is required (no default backend), the key is not: a
/// keyless backend is asked with no `authorization` header (decision
/// D127).
#[test]
fn the_base_url_is_required_and_the_package_declares_no_origin() {
    let manifest = scxml_validator_core::ServicePackageManifest::from_json(include_bytes!(
        "../package/generate.service.json"
    ))
    .expect("the manifest parses");
    assert!(
        manifest.http_origins.is_empty(),
        "no default backend: the baseurl value supplies the one origin"
    );
    assert_eq!(endpoint(Some(2), Some(1)), Ok((2, Some(1))));
    assert_eq!(endpoint(Some(2), None), Ok((2, None)));
    for api_key in [Some(1), None] {
        assert_eq!(
            endpoint(None, api_key),
            Err(FailureCode::Unconfigured),
            "{api_key:?}"
        );
    }
    let generation = parse(&[("model", text("m")), ("prompt", text("p"))]).expect("valid");
    let keyless = build_request(2, None, None, &generation);
    let keyed = build_request(2, Some(1), None, &generation);
    assert!(
        keyless
            .headers
            .iter()
            .all(|(name, _)| name != "authorization"),
        "{:?}",
        keyless.headers
    );
    assert_eq!(keyless.headers, keyed.headers[1..]);
    assert_eq!((keyless.url, keyless.body), (keyed.url, keyed.body));
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
    assert_eq!(
        manifest.env_slots,
        [API_KEY_SLOT, BASE_URL_SLOT, STREAM_SLOT]
    );
}

#[test]
fn a_streamed_reply_reports_progress_at_most_once_a_second() {
    assert!(super::progress_due(None, 0), "the first read reports");
    assert!(!super::progress_due(
        Some(0),
        super::PROGRESS_INTERVAL_NS - 1
    ));
    assert!(super::progress_due(Some(0), super::PROGRESS_INTERVAL_NS));
    assert!(
        !super::progress_due(Some(5), 3),
        "a clock behind the last report"
    );
    let progress = super::Returned::progress(42);
    assert_eq!(progress.name, super::PROGRESS_EVENT);
    assert_eq!(progress.data, r#"{"received":42}"#);
}

#[test]
fn only_data_events_count_as_a_streamed_reply_s_progress() {
    let body = b": ping\n\n: ping\n\ndata: {\"a\":1}\n\nda";
    let (count, next) = super::data_lines(body, 0);
    assert_eq!(count, 1, "the comments are not progress");
    assert_eq!(&body[next..], b"da", "the incomplete line waits");
    let mut more = body.to_vec();
    more.extend_from_slice(b"ta: [DONE]\n\n");
    assert_eq!(super::data_lines(&more, next), (1, more.len()));
    assert_eq!(super::data_lines(b": ping\n\n", 0), (0, 8));
}

/// A completion that is not streamed is never progress, however its bytes
/// arrive (decision D101): a document arms no inactivity bound for it.
#[test]
fn a_reply_that_is_not_streamed_reports_no_progress() {
    let completion = String::from_utf8(completion("hi", "stop")).expect("UTF-8");
    let mut progress = super::Progress::default();
    let mut body = Vec::new();
    for (second, byte) in completion.bytes().enumerate() {
        body.push(byte);
        let now = u64::try_from(second).expect("small") * super::PROGRESS_INTERVAL_NS;
        assert!(!progress.read(&body, now), "{completion}");
    }
}

/// A streamed reply reports once it shows it is a stream, a keep-alive
/// comment included, and then only for new `data:` events, at most once a
/// second (decision D101).
#[test]
fn a_streamed_reply_reports_its_start_and_then_its_data_events() {
    let second = super::PROGRESS_INTERVAL_NS;
    let mut pings = super::Progress::default();
    assert!(pings.read(b": ping\n\n", 0), "the stream has started");
    assert!(!pings.read(b": ping\n\n: ping\n\n", 2 * second));
    let mut data = super::Progress::default();
    let mut body = b"data: {\"a\":1}\n\n".to_vec();
    assert!(data.read(&body, 0), "the stream has started");
    body.extend_from_slice(b"data: {\"a\":2}\n\n");
    assert!(!data.read(&body, second - 1), "within the second");
    assert!(data.read(&body, second), "the event not yet reported");
    assert!(!data.read(&body, 3 * second), "no new event");
}

#[test]
fn with_a_schema_only_no_reasoning_effort_is_admitted() {
    let with = |effort: Option<&str>| {
        let mut parameters = vec![
            ("model", text("m")),
            ("prompt", text("p")),
            ("schema", text(SCHEMA)),
        ];
        if let Some(effort) = effort {
            parameters.push(("reasoning_effort", text(effort)));
        }
        parse(&parameters).map(|generation| generation.reasoning_effort)
    };
    assert_eq!(with(None), Ok(TOOLS_REASONING_EFFORT));
    assert_eq!(with(Some("none")), Ok("none"));
    assert!(with(Some("low")).is_err(), "tools refuse another effort");
    // Without a schema the default stays.
    let plain = parse(&[("model", text("m")), ("prompt", text("p"))]).expect("valid");
    assert_eq!(plain.reasoning_effort, DEFAULT_REASONING_EFFORT);
}

#[test]
fn a_finish_reason_must_agree_with_the_tool_calls() {
    let generation = parse(&[
        ("model", text("m")),
        ("prompt", text("p")),
        ("schema", text(SCHEMA)),
    ])
    .expect("valid");
    let arguments = r#"{"score":41,"answer":"yes"}"#;
    let mut stopped: Json =
        serde_json::from_slice(&tool_completion(DEFAULT_SCHEMA_NAME, arguments)).expect("JSON");
    stopped["choices"][0]["finish_reason"] = "stop".into();
    assert_eq!(
        outcome(&generation, 0, ok(stopped.to_string().into_bytes())),
        Returned::failed(FailureCode::Malformed),
        "calls without a tool_calls finish reason"
    );
    assert_eq!(
        outcome(&generation, 0, ok(completion("", "tool_calls"))),
        Returned::failed(FailureCode::Malformed),
        "a tool_calls finish reason without calls"
    );
    let mut without_id: Json =
        serde_json::from_slice(&tool_completion(DEFAULT_SCHEMA_NAME, arguments)).expect("JSON");
    without_id["choices"][0]["message"]["tool_calls"][0]["id"] = "".into();
    assert_eq!(
        outcome(&generation, 0, ok(without_id.to_string().into_bytes())),
        Returned::failed(FailureCode::Malformed),
        "a call without an id"
    );
}
