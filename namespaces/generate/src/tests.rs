use std::collections::BTreeMap;

use super::*;

const TEXT_REPLY: &str = r#"{"id":"c","object":"chat.completion","choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"hello there","refusal":null}}]}"#;

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

const SCHEMA: &str = r#"{"type":"object","properties":{"answer":{"type":"string"},"score":{"type":"integer"}},"required":["answer","score"],"additionalProperties":false}"#;

/// A deterministic fake backend: expressions evaluate from a table,
/// assignments and requests are recorded, and the service answers with a
/// scripted response.
struct FakeHost {
    values: BTreeMap<String, Value>,
    slots: BTreeMap<String, u32>,
    assigned: Vec<(String, Data)>,
    requests: Vec<Request>,
    answer: Result<Response, ()>,
    fail_assign: bool,
}

impl Default for FakeHost {
    fn default() -> Self {
        Self {
            values: BTreeMap::new(),
            slots: BTreeMap::from([(API_KEY_SLOT.into(), 1), (BASE_URL_SLOT.into(), 2)]),
            assigned: Vec::new(),
            requests: Vec::new(),
            answer: Ok(Response {
                status: 200,
                body: TEXT_REPLY.into(),
            }),
            fail_assign: false,
        }
    }
}

impl FakeHost {
    fn answering(body: Vec<u8>) -> Self {
        Self {
            answer: Ok(Response { status: 200, body }),
            ..Self::default()
        }
    }

    fn body(&self) -> serde_json::Value {
        assert_eq!(self.requests.len(), 1, "exactly one request");
        serde_json::from_slice(&self.requests[0].body).expect("the body is JSON")
    }
}

impl GenerateHost for FakeHost {
    type Error = &'static str;

    fn evaluate(&mut self, expression: &str) -> Result<Result<Value, ()>, &'static str> {
        if expression == "fatal" {
            return Err("fatal");
        }
        Ok(self.values.get(expression).cloned().ok_or(()))
    }

    fn assign(&mut self, location: &str, value: &Data) -> Result<Result<(), ()>, &'static str> {
        if self.fail_assign {
            return Ok(Err(()));
        }
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

fn child(local_name: &str, text: &str) -> Vec<Content> {
    vec![
        Content::Start {
            namespace_uri: Some(NAMESPACE_URI.into()),
            local_name: local_name.into(),
            has_attributes: false,
        },
        Content::Text(text.into()),
        Content::End,
    ]
}

fn run(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut FakeHost,
) -> Result<Executed, Failure<&'static str>> {
    execute("generate", attributes, content, host)
}

fn base() -> Vec<Attribute> {
    vec![
        attr("location", "reply"),
        attr("model", "m-1"),
        env("apikey", "OPENAI_API_KEY"),
        env("baseurl", "OPENAI_BASE_URL"),
    ]
}

fn execution(result: Result<Executed, Failure<&'static str>>) -> &'static str {
    match result {
        Err(Failure::Execution(detail)) => detail,
        other => panic!("expected error.execution, got {other:?}"),
    }
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
fn the_package_binds_the_exact_uri_schema_digest_slots_and_origin() {
    let manifest = scxml_validator_core::ExtensionPackageManifest::from_json(include_bytes!(
        "../package/generate.extension.json"
    ))
    .expect("the manifest is a valid extension package manifest");
    assert_eq!(manifest.namespace_uri, NAMESPACE_URI);
    assert_eq!(
        manifest.extension_artifact,
        "nemobot_generate.virtualized.wasm"
    );
    assert_eq!(manifest.http_origins, ["https://api.openai.com"]);
    assert_eq!(manifest.env_slots, [API_KEY_SLOT, BASE_URL_SLOT]);
    assert!(manifest.wasi_permissions.is_empty(), "no WASI permission");
    manifest
        .verify_schema(&scxml_validator_core::SchemaSource::new(
            "generate.xsd",
            include_bytes!("../package/generate.xsd").to_vec(),
        ))
        .expect("the schema digest, targetNamespace, and WIT identity verify");
}

#[test]
fn a_text_prompt_posts_a_canonical_chat_completion_and_assigns_the_reply() {
    let mut host = FakeHost::default();
    let mut attributes = base();
    attributes.push(attr("system", "Be brief."));
    assert_eq!(
        run(
            &attributes,
            &child("content", "\n  Say hello.\n"),
            &mut host
        ),
        Ok(Executed::Handled)
    );
    let request = &host.requests[0];
    assert_eq!(
        request.url,
        [
            Part::Reference(2),
            Part::Literal("/chat/completions".into())
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
    assert_eq!(
        String::from_utf8(request.body.clone()).expect("UTF-8"),
        r#"{"messages":[{"content":"Be brief.","role":"system"},{"content":"Say hello.","role":"user"}],"model":"m-1"}"#,
        "the body is canonical JSON: compact, members sorted"
    );
    assert_eq!(
        host.assigned,
        [("reply".to_owned(), Data::String("hello there".into()))]
    );
}

#[test]
fn the_same_element_always_encodes_the_same_bytes() {
    let schema = response_schema(SCHEMA, None, true).expect("the schema compiles");
    let messages = [Message {
        role: Role::User,
        content: "q".into(),
    }];
    let first = build_request(7, 3, "m", &messages, Some(&schema));
    let second = build_request(7, 3, "m", &messages, Some(&schema));
    assert_eq!(first, second);
    assert!(scxml_event_schema::is_canonical_json(
        std::str::from_utf8(&first.body).expect("UTF-8")
    ));
    assert_eq!(
        first.url,
        [
            Part::Reference(7),
            Part::Literal("/chat/completions".into())
        ],
        "the baseurl slot's reference is the base URL"
    );
}

#[test]
fn modelexpr_systemexpr_and_a_baseurl_slot_are_used() {
    let mut host = FakeHost::default();
    host.slots.insert(BASE_URL_SLOT.into(), 5);
    host.values
        .insert("m".into(), Value::Text(" nemotron ".into()));
    host.values.insert("s".into(), Value::Text("sys".into()));
    host.values.insert("p".into(), Value::Text("prompt".into()));
    let attributes = [
        attr("location", "reply"),
        attr("modelexpr", "m"),
        attr("systemexpr", "s"),
        attr("expr", "p"),
    ];
    assert_eq!(run(&attributes, &[], &mut host), Ok(Executed::Handled));
    let body = host.body();
    assert_eq!(body["model"], "nemotron");
    assert_eq!(
        body["messages"],
        serde_json::json!([{"role":"system","content":"sys"},{"role":"user","content":"prompt"}])
    );
    assert_eq!(host.requests[0].url[0], Part::Reference(5));
}

#[test]
fn expr_may_supply_a_message_array() {
    let mut host = FakeHost::default();
    host.values.insert(
        "history".into(),
        Value::Json(
            r#"[{"role":"user","content":"hi"},{"role":"assistant","content":"hello"},{"role":"user","content":"again"}]"#
                .into(),
        ),
    );
    let mut attributes = base();
    attributes.push(attr("expr", "history"));
    attributes.push(attr("system", "sys"));
    assert_eq!(run(&attributes, &[], &mut host), Ok(Executed::Handled));
    let roles: Vec<String> = host.body()["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|message| message["role"].as_str().expect("role").to_owned())
        .collect();
    assert_eq!(roles, ["system", "user", "assistant", "user"]);
}

#[test]
fn malformed_message_arrays_are_refused_before_any_request() {
    for json in [
        "[]",
        r#"{"role":"user","content":"x"}"#,
        r#"[{"role":"tool","content":"x"}]"#,
        r#"[{"role":"user","content":1}]"#,
        r#"[{"role":"user","content":"x","name":"n"}]"#,
        r#"[{"role":"user","content":" "}]"#,
    ] {
        let mut host = FakeHost::default();
        host.values
            .insert("history".into(), Value::Json(json.into()));
        let mut attributes = base();
        attributes.push(attr("expr", "history"));
        execution(run(&attributes, &[], &mut host));
        assert!(host.requests.is_empty(), "{json}: nothing is sent");
    }
}

#[test]
fn a_schema_requests_structured_output_and_assigns_the_validated_value() {
    let mut host = FakeHost::answering(reply(r#"{"score":7,"answer":"yes"}"#));
    let mut attributes = base();
    attributes.push(attr("schemaname", "verdict"));
    attributes.push(attr("strict", "true"));
    let mut content = child("content", "Decide.");
    content.extend(child("schema", SCHEMA));
    assert_eq!(run(&attributes, &content, &mut host), Ok(Executed::Handled));
    let body = host.body();
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(body["response_format"]["json_schema"]["name"], "verdict");
    assert_eq!(body["response_format"]["json_schema"]["strict"], true);
    assert_eq!(
        body["response_format"]["json_schema"]["schema"],
        serde_json::from_str::<serde_json::Value>(SCHEMA).expect("schema JSON")
    );
    assert_eq!(
        host.assigned,
        [(
            "reply".to_owned(),
            Data::JsonObject(r#"{"answer":"yes","score":7}"#.into())
        )],
        "the reply is assigned as a canonical JSON object"
    );
}

#[test]
fn schemaexpr_takes_a_json_value_and_defaults_name_and_strictness() {
    let mut host = FakeHost::answering(reply("[1,2]"));
    host.values.insert(
        "s".into(),
        Value::Json(r#"{"type":"array","items":{"type":"integer"}}"#.into()),
    );
    let mut attributes = base();
    attributes.push(attr("schemaexpr", "s"));
    assert_eq!(
        run(&attributes, &child("content", "List."), &mut host),
        Ok(Executed::Handled)
    );
    let body = host.body();
    assert_eq!(
        body["response_format"]["json_schema"]["name"],
        DEFAULT_SCHEMA_NAME
    );
    assert_eq!(body["response_format"]["json_schema"]["strict"], false);
    assert_eq!(
        host.assigned,
        [("reply".to_owned(), Data::JsonArray("[1,2]".into()))]
    );
}

#[test]
fn scalar_structured_replies_are_assigned_as_scalars() {
    for (text, schema, expected) in [
        (
            "42",
            r#"{"type":"integer"}"#,
            Data::Number(FiniteNumber::new(42.0).expect("finite")),
        ),
        ("true", r#"{"type":"boolean"}"#, Data::Bool(true)),
        (r#""x""#, r#"{"type":"string"}"#, Data::String("x".into())),
        ("null", r#"{"type":"null"}"#, Data::Null),
    ] {
        let mut host = FakeHost::answering(reply(text));
        let mut attributes = base();
        attributes.push(attr("schemaexpr", "s"));
        host.values.insert("s".into(), Value::Text(schema.into()));
        assert_eq!(
            run(&attributes, &child("content", "q"), &mut host),
            Ok(Executed::Handled),
            "{text}"
        );
        assert_eq!(host.assigned, [("reply".to_owned(), expected)]);
    }
}

#[test]
fn a_structured_reply_that_mismatches_the_schema_is_error_execution_and_unassigned() {
    for text in [
        r#"{"answer":"yes"}"#,
        r#"{"answer":"yes","score":"7"}"#,
        r#"{"answer":"yes","score":7,"extra":1}"#,
        "not json",
        "```json\n{\"answer\":\"yes\",\"score\":7}\n```",
    ] {
        let mut host = FakeHost::answering(reply(text));
        let mut content = child("content", "Decide.");
        content.extend(child("schema", SCHEMA));
        execution(run(&base(), &content, &mut host));
        assert!(host.assigned.is_empty(), "{text}: nothing is assigned");
    }
}

#[test]
fn invalid_schemas_are_refused_before_any_request() {
    for schema in [
        "not json",
        r#"{"type":"nonsense"}"#,
        r#"{"$ref":"https://example.com/s.json"}"#,
        r#"{"$schema":"https://example.com/unknown"}"#,
    ] {
        let mut host = FakeHost::default();
        let mut content = child("content", "q");
        content.extend(child("schema", schema));
        assert_eq!(
            execution(run(&base(), &content, &mut host)),
            "gen:generate schema is not a self-contained JSON Schema",
            "{schema}"
        );
        assert!(host.requests.is_empty(), "{schema}: nothing is sent");
    }
}

/// Malformed elements and the detail each raises.
fn malformed_cases() -> Vec<(Vec<Attribute>, Vec<Content>, &'static str)> {
    let content = child("content", "q");
    let with = |extra: &[Attribute]| {
        let mut attributes = base();
        attributes.extend_from_slice(extra);
        attributes
    };
    vec![
        (
            vec![attr("model", "m"), env("apikey", "K")],
            content.clone(),
            "requires a non-empty location",
        ),
        (
            vec![attr("location", "r"), env("apikey", "K")],
            content.clone(),
            "exactly one of model or modelexpr",
        ),
        (
            with(&[attr("modelexpr", "m")]),
            content.clone(),
            "exactly one of model or modelexpr",
        ),
        (with(&[]), Vec::new(), "exactly one of expr or gen:content"),
        (
            with(&[attr("expr", "p")]),
            content.clone(),
            "exactly one of expr or gen:content",
        ),
        (
            with(&[attr("system", "a"), attr("systemexpr", "b")]),
            content.clone(),
            "at most one of system or systemexpr",
        ),
        (
            with(&[attr("stream", "true")]),
            content.clone(),
            "does not define",
        ),
        (
            with(&[Attribute {
                namespace_uri: Some("urn:other".into()),
                local_name: "x".into(),
                value: "y".into(),
            }]),
            content.clone(),
            "another namespace",
        ),
        (
            with(&[attr("strict", "1")]),
            content.clone(),
            "strict must be true or false",
        ),
        (
            with(&[attr("schemaname", "bad name")]),
            content.clone(),
            "schemaname must match",
        ),
        (
            with(&[attr("schemaname", "ok")]),
            content.clone(),
            "require a response schema",
        ),
        (
            with(&[]),
            [content.clone(), content.clone()].concat(),
            "at most one gen:content",
        ),
        (
            with(&[]),
            vec![Content::Text("loose".into())],
            "holds only gen:content and gen:schema",
        ),
        (
            with(&[]),
            child("prompt", "q"),
            "holds only gen:content and gen:schema",
        ),
        (
            with(&[]),
            child("content", "   "),
            "prompt must not be empty",
        ),
        (
            vec![attr("location", "r"), attr("model", "m")],
            content.clone(),
            "no API key",
        ),
    ]
}

/// No backend URL is compiled in: without a `baseurl` value the element
/// fails before any request, so the key never reaches a host the component
/// chose.
#[test]
fn an_element_without_a_base_url_sends_nothing() {
    let mut host = FakeHost::default();
    host.slots.remove(BASE_URL_SLOT);
    let detail = execution(run(&base(), &child("content", "q"), &mut host));
    assert!(detail.contains("no base URL"), "{detail}");
    assert!(host.requests.is_empty(), "nothing is sent");
    assert!(host.assigned.is_empty());
}

#[test]
fn malformed_elements_are_error_execution_and_send_nothing() {
    for (attributes, content, expected) in malformed_cases() {
        let mut host = FakeHost::default();
        if !attributes
            .iter()
            .any(|attribute| attribute.local_name == "apikey")
        {
            host.slots.clear();
        }
        let detail = execution(run(&attributes, &content, &mut host));
        assert!(detail.contains(expected), "{expected}: got {detail}");
        assert!(host.requests.is_empty(), "{expected}: nothing is sent");
        assert!(host.assigned.is_empty());
    }
}

#[test]
fn evaluation_failures_are_error_execution_and_host_failures_pass_through() {
    let mut host = FakeHost::default();
    host.values.insert("n".into(), Value::Other);
    let mut attributes = base();
    attributes.push(attr("expr", "missing"));
    assert!(execution(run(&attributes, &[], &mut host)).contains("could not be evaluated"));
    let mut attributes = base();
    attributes.push(attr("expr", "n"));
    assert!(execution(run(&attributes, &[], &mut host)).contains("string or an array"));
    let mut attributes = base();
    attributes.push(attr("expr", "fatal"));
    assert_eq!(
        run(&attributes, &[], &mut host),
        Err(Failure::Host("fatal"))
    );
}

#[test]
fn service_failures_and_malformed_replies_are_error_execution() {
    let cases: Vec<(Result<Response, ()>, &str)> = vec![
        (Err(()), "refused or failed"),
        (
            Ok(Response {
                status: 500,
                body: b"{}".to_vec(),
            }),
            "non-success status",
        ),
        (
            Ok(Response {
                status: 302,
                body: Vec::new(),
            }),
            "non-success status",
        ),
        (
            Ok(Response {
                status: 200,
                body: b"nope".to_vec(),
            }),
            "not a valid chat completion",
        ),
        (
            Ok(Response {
                status: 200,
                body: br#"{"choices":[]}"#.to_vec(),
            }),
            "not a valid chat completion",
        ),
        (
            Ok(Response {
                status: 200,
                body: br#"{"choices":[{"finish_reason":"length","message":{"role":"assistant","content":"tru"}}]}"#.to_vec(),
            }),
            "token limit",
        ),
        (
            Ok(Response {
                status: 200,
                body: br#"{"choices":[{"finish_reason":"content_filter","message":{"role":"assistant","content":null}}]}"#.to_vec(),
            }),
            "content filter",
        ),
        (
            Ok(Response {
                status: 200,
                body: br#"{"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":null,"refusal":"no"}}]}"#.to_vec(),
            }),
            "refused",
        ),
        (
            Ok(Response {
                status: 200,
                body: br#"{"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":null}}]}"#.to_vec(),
            }),
            "no text content",
        ),
    ];
    for (answer, expected) in cases {
        let mut host = FakeHost {
            answer,
            ..FakeHost::default()
        };
        let detail = execution(run(&base(), &child("content", "q"), &mut host));
        assert!(detail.contains(expected), "{expected}: got {detail}");
        assert!(host.assigned.is_empty());
    }
}

#[test]
fn a_failed_assignment_is_error_execution() {
    let mut host = FakeHost {
        fail_assign: true,
        ..FakeHost::default()
    };
    assert!(
        execution(run(&base(), &child("content", "q"), &mut host)).contains("could not assign")
    );
}

#[test]
fn other_local_names_are_not_handled() {
    let mut host = FakeHost::default();
    assert_eq!(
        execute("prompt", &base(), &[], &mut host),
        Ok(Executed::NotHandled)
    );
    assert!(host.requests.is_empty());
}

#[test]
fn failure_details_never_echo_the_prompt_reply_or_model() {
    let mut host = FakeHost::answering(reply("SECRET-REPLY"));
    let mut content = child("content", "SECRET-PROMPT");
    content.extend(child("schema", r#"{"type":"integer"}"#));
    let mut attributes = base();
    attributes[1] = attr("model", "SECRET-MODEL");
    let detail = execution(run(&attributes, &content, &mut host));
    for secret in ["SECRET-REPLY", "SECRET-PROMPT", "SECRET-MODEL"] {
        assert!(!detail.contains(secret), "{detail}");
    }
}
