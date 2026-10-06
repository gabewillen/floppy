use super::*;
use proptest::prelude::*;
use scxml_telemetry::{Recorder, install_recorder};
use scxml_types::FiniteNumber;

const TARGET: &str = "http://loopback.invalid/sessions/s1/basichttp";

fn send(event: Option<&str>) -> OutboundSend {
    OutboundSend {
        event_name: event.map(str::to_owned),
        target: Some(TARGET.into()),
        params: Vec::new(),
        content: None,
    }
}

fn number(value: f64) -> DataValue {
    DataValue::Number(FiniteNumber::new(value).expect("test number is finite"))
}

fn body(request: &HttpRequest) -> &str {
    std::str::from_utf8(&request.body).expect("encoded body is ASCII")
}

/// Records every request and answers with a fixed response.
struct Recording {
    requests: Vec<HttpRequest>,
    answer: Result<HttpResponse, TransportFailure>,
}

impl HttpTransport for Recording {
    fn post(&mut self, request: &HttpRequest) -> Result<HttpResponse, TransportFailure> {
        self.requests.push(request.clone());
        self.answer.clone()
    }
}

fn answering(status: u16) -> Recording {
    Recording {
        requests: Vec::new(),
        answer: Ok(HttpResponse {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }),
    }
}

#[test]
fn type_uri_matches_the_scxml_processor_classification() {
    assert_eq!(
        BasicHttpIoProcessor.type_uri(),
        scxml_ioprocessor_scxml::BASICHTTP_EVENT_PROCESSOR_URI
    );
    assert_eq!(
        scxml_ioprocessor_scxml::classify_processor_type(BASICHTTP_EVENT_PROCESSOR_URI),
        scxml_ioprocessor_scxml::ProcessorKind::BasicHttp
    );
}

#[test]
fn documented_wit_bounds_match_enforced_constants() {
    let docs = include_str!("../../../crates/scxml-wit/wit/basichttp/basichttp.wit")
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("///"))
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ");
    for phrase in [
        format!("name 1..={MAX_HEADER_NAME_BYTES} bytes of RFC 9110 token characters"),
        format!("value at most {MAX_HEADER_VALUE_BYTES} bytes"),
        format!("`method` 1..={MAX_METHOD_BYTES} bytes"),
        format!("`uri` 1..={MAX_URI_BYTES} bytes"),
        format!("at most {MAX_HEADERS} headers"),
        format!("`body` at most {MAX_BODY_BYTES} bytes"),
        format!("Parameter name: 1..={MAX_NAME_BYTES} bytes"),
        format!("(at most {MAX_PARAMS})"),
        format!("(at most {MAX_URI_BYTES} bytes)"),
        format!("body (at most {MAX_RAW_BYTES} bytes)"),
    ] {
        assert!(
            docs.contains(&phrase),
            "basichttp.wit must document the enforced bound: {phrase:?}"
        );
    }
}

#[test]
fn event_name_and_namelist_become_ordered_post_parameters() {
    // Official cases 518 and 534: `_scxmleventname` first, then namelist.
    let mut outbound = send(Some("test"));
    outbound.params.push(Param {
        name: "Var1".into(),
        value: number(2.0),
    });
    let request = BasicHttpIoProcessor
        .encode(&outbound)
        .expect("valid send encodes");
    assert_eq!(request.method, "POST");
    assert_eq!(request.uri, TARGET);
    assert_eq!(
        request.headers,
        vec![HttpHeader {
            name: "Content-Type".into(),
            value: FORM_MEDIA_TYPE.into(),
        }]
    );
    assert_eq!(body(&request), "_scxmleventname=test&Var1=2");
}

#[test]
fn content_is_the_percent_encoded_body() {
    // Official case 520: spaces travel as %20 and no event name is sent.
    let mut outbound = send(None);
    outbound.content = Some(DataValue::String("this is some content".into()));
    let request = BasicHttpIoProcessor
        .encode(&outbound)
        .expect("content send encodes");
    assert_eq!(body(&request), "this%20is%20some%20content");
    let event = BasicHttpIoProcessor
        .receive(&request)
        .expect("encoded content decodes");
    assert_eq!(event.name.as_str(), DEFAULT_EVENT_NAME);
    assert_eq!(event.data, DataValue::String("this is some content".into()));
    let raw = event.raw.expect("raw message is retained");
    assert!(raw.starts_with(&format!("POST {TARGET} HTTP/1.1\r\n")));
    assert!(raw.ends_with("\r\n\r\nthis%20is%20some%20content"));
    assert_eq!(
        event.origin_type.as_deref(),
        Some(BASICHTTP_EVENT_PROCESSOR_URI)
    );
}

#[test]
fn values_use_their_portable_text_form() {
    let mut outbound = send(None);
    for (name, value) in [
        ("n", number(-0.0)),
        ("f", number(1.5)),
        ("b", DataValue::Bool(true)),
        ("z", DataValue::Null),
        ("s", DataValue::String("a=b&c".into())),
        ("o", DataValue::JsonObject("{\"k\":1}".into())),
    ] {
        outbound.params.push(Param {
            name: name.into(),
            value,
        });
    }
    let request = BasicHttpIoProcessor
        .encode(&outbound)
        .expect("values encode");
    assert_eq!(
        body(&request),
        "n=0&f=1.5&b=true&z=null&s=a%3Db%26c&o=%7B%22k%22%3A1%7D"
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one table covers every send validation failure"
)]
fn invalid_sends_fail_before_anything_is_sent() {
    let cases = [
        (
            OutboundSend {
                target: None,
                ..send(Some("e"))
            },
            SendError::MissingTarget,
            SendErrorKind::Unavailable,
        ),
        (
            OutboundSend {
                target: Some("#_scxml_session".into()),
                ..send(Some("e"))
            },
            SendError::InvalidTarget,
            SendErrorKind::InvalidInput,
        ),
        (
            OutboundSend {
                target: Some("http:///path".into()),
                ..send(Some("e"))
            },
            SendError::InvalidTarget,
            SendErrorKind::InvalidInput,
        ),
        (
            OutboundSend {
                target: Some("http://host/a b".into()),
                ..send(Some("e"))
            },
            SendError::InvalidTarget,
            SendErrorKind::InvalidInput,
        ),
        (
            send(Some("")),
            SendError::InvalidEventName,
            SendErrorKind::InvalidInput,
        ),
        (
            OutboundSend {
                params: vec![Param {
                    name: String::new(),
                    value: DataValue::Null,
                }],
                ..send(Some("e"))
            },
            SendError::InvalidParamName,
            SendErrorKind::InvalidInput,
        ),
        (
            OutboundSend {
                params: vec![Param {
                    name: "p".into(),
                    value: DataValue::Undefined,
                }],
                ..send(Some("e"))
            },
            SendError::InvalidValue,
            SendErrorKind::InvalidInput,
        ),
        (
            OutboundSend {
                params: vec![Param {
                    name: "p".into(),
                    value: DataValue::Null,
                }],
                content: Some(DataValue::String("c".into())),
                ..send(Some("e"))
            },
            SendError::ContentWithParams,
            SendErrorKind::InvalidInput,
        ),
        (
            OutboundSend {
                params: (0..=MAX_PARAMS)
                    .map(|index| Param {
                        name: format!("p{index}"),
                        value: DataValue::Null,
                    })
                    .collect(),
                ..send(Some("e"))
            },
            SendError::PayloadTooLarge,
            SendErrorKind::PayloadTooLarge,
        ),
        (
            OutboundSend {
                content: Some(DataValue::String(" ".repeat(MAX_BODY_BYTES / 3 + 1))),
                ..send(None)
            },
            SendError::PayloadTooLarge,
            SendErrorKind::PayloadTooLarge,
        ),
    ];
    for (outbound, error, kind) in cases {
        let mut transport = answering(200);
        assert_eq!(
            BasicHttpIoProcessor.send(&outbound, &mut transport),
            Err(error)
        );
        assert_eq!(error.kind(), kind);
        assert!(!error.to_string().is_empty() && error.detail().len() <= 160);
        assert!(transport.requests.is_empty(), "{error:?} must not send");
        // `check` reports the same validation failure at the `<send>`,
        // except a missing target, which fails only when dispatched.
        let checked = BasicHttpIoProcessor.check(&outbound);
        if error == SendError::MissingTarget {
            assert_eq!(checked, Ok(()));
        } else {
            assert_eq!(checked, Err(error));
        }
    }
    let valid = send(Some("e"));
    assert_eq!(BasicHttpIoProcessor.check(&valid), Ok(()));
    let untargeted_invalid = OutboundSend {
        target: None,
        ..send(Some(""))
    };
    assert_eq!(
        BasicHttpIoProcessor.check(&untargeted_invalid),
        Err(SendError::InvalidEventName),
        "a missing target still checks every other field"
    );
}

#[test]
fn delivery_requires_a_bounded_2xx_response() {
    let outbound = send(Some("e"));
    let mut accepted = answering(204);
    assert_eq!(BasicHttpIoProcessor.send(&outbound, &mut accepted), Ok(()));
    assert_eq!(accepted.requests.len(), 1);
    for (mut transport, error) in [
        (answering(404), SendError::NotAccepted),
        (answering(99), SendError::InvalidResponse),
        (
            Recording {
                requests: Vec::new(),
                answer: Err(TransportFailure),
            },
            SendError::TransportFailed,
        ),
        (
            Recording {
                requests: Vec::new(),
                answer: Ok(HttpResponse {
                    status: 200,
                    headers: vec![HttpHeader {
                        name: "Bad Name".into(),
                        value: String::new(),
                    }],
                    body: Vec::new(),
                }),
            },
            SendError::InvalidResponse,
        ),
    ] {
        assert_eq!(
            BasicHttpIoProcessor.send(&outbound, &mut transport),
            Err(error)
        );
        assert_eq!(error.kind(), SendErrorKind::Unavailable);
    }
}

fn inbound(headers: &[(&str, &str)], body: &str) -> HttpRequest {
    HttpRequest {
        method: POST_METHOD.into(),
        uri: TARGET.into(),
        headers: headers
            .iter()
            .map(|(name, value)| HttpHeader {
                name: (*name).into(),
                value: (*value).into(),
            })
            .collect(),
        body: body.as_bytes().to_vec(),
    }
}

#[test]
fn external_client_message_uses_the_event_name_header() {
    // The W3C manual test 513 client: form parameters plus an
    // `_scxmleventname` header.
    let event = BasicHttpIoProcessor
        .receive(&inbound(
            &[
                ("Content-Type", FORM_MEDIA_TYPE),
                ("_scxmleventname", "test"),
            ],
            "key1=value1&key2=value2",
        ))
        .expect("well-formed message decodes");
    assert_eq!(event.name.as_str(), "test");
    assert_eq!(
        event.data,
        DataValue::JsonObject("{\"key1\":\"value1\",\"key2\":\"value2\"}".into())
    );
    assert_eq!(event.event_type, scxml_types::EventType::External);
}

#[test]
fn first_form_event_name_wins_and_is_not_data() {
    let event = BasicHttpIoProcessor
        .receive(&inbound(
            &[(
                "content-type",
                "Application/X-WWW-Form-URLEncoded; charset=utf-8",
            )],
            "_scxmleventname=first&p=1&_scxmleventname=second&p=2&q=a+b%21",
        ))
        .expect("form decodes");
    assert_eq!(event.name.as_str(), "first");
    assert_eq!(
        event.data,
        DataValue::JsonObject("{\"p\":[\"1\",\"2\"],\"q\":\"a b!\"}".into())
    );
}

#[test]
fn non_form_bodies_are_text_and_empty_messages_have_no_data() {
    let text = BasicHttpIoProcessor
        .receive(&inbound(&[("Content-Type", "text/plain")], "a=b"))
        .expect("text body decodes");
    // Neither JSON nor an XML document: text.
    assert_eq!(text.data, DataValue::String("a=b".into()));
    assert_eq!(text.name.as_str(), DEFAULT_EVENT_NAME);
    let empty = BasicHttpIoProcessor
        .receive(&inbound(&[], ""))
        .expect("empty body decodes");
    assert_eq!(empty.data, DataValue::Absent);
}

/// B.2.8.1: "In some circumstances, the entity that generated the event may
/// have indicated how the data is to be interpreted. In such cases, the
/// Processor SHOULD try to format the data according to those
/// indications." A JSON `Content-Type` makes a JSON body its value, and an
/// XML one makes an XML document an XML value; a body that is not what its
/// type says, and every other type, stays text.
#[test]
fn a_json_or_xml_content_type_formats_the_body() {
    let data = |content_type: &str, body: &str| {
        BasicHttpIoProcessor
            .receive(&inbound(&[("Content-Type", content_type)], body))
            .expect("the message decodes")
            .data
    };
    assert_eq!(
        data("application/json", r#"{"b": [1, "x"], "a": true}"#),
        DataValue::canonical_json_object(r#"{"b":[1,"x"],"a":true}"#).expect("canonical JSON")
    );
    assert_eq!(
        data("Application/JSON; charset=utf-8", " [1, 2] "),
        DataValue::canonical_json_array("[1,2]").expect("canonical JSON")
    );
    assert_eq!(
        data("application/ld+json", "\"s\""),
        DataValue::String("s".into())
    );
    assert_eq!(data("application/json", "12.5"), number(12.5));
    assert_eq!(data("application/json", "null"), DataValue::Null);
    assert_eq!(
        data("application/json", "{not json"),
        DataValue::String("{not json".into())
    );
    for xml_type in [
        "application/xml",
        "text/xml; charset=utf-8",
        "application/atom+xml",
    ] {
        assert_eq!(
            data(xml_type, "<books><book title=\"t\"/></books>"),
            DataValue::Xml("<books><book title=\"t\"/></books>".into()),
            "{xml_type}"
        );
    }
    // Not a well-formed document: two roots, bare text, malformed markup.
    for body in ["<a/><b/>", "text", "<a>"] {
        assert_eq!(
            data("application/xml", body),
            DataValue::String(body.into()),
            "{body}"
        );
    }
    // `text/plain` indicates no structure: B.2.8.1's content rules apply.
    assert_eq!(
        data("text/plain", "{\"a\": 1}"),
        DataValue::canonical_json_object("{\"a\":1}").expect("canonical JSON")
    );
}

/// B.2.8.1: "Otherwise (i.e., if the content does not consist of key-value
/// pairs), if the Processor supports JSON and it can interpret the content
/// as JSON, it MUST create the corresponding ECMAScript object(s) as the
/// value of _event.data. Otherwise, if the Processor can interpret the
/// content as a valid XML document, it MUST create the corresponding DOM
/// structure". Unlabelled content, a non-form body or a form body's bare
/// `<content>` segment, is JSON, then an XML document, then text.
#[test]
fn unlabelled_content_is_json_then_an_xml_document_then_text() {
    let receive = |headers: &[(&str, &str)], body: &str| {
        BasicHttpIoProcessor
            .receive(&inbound(headers, body))
            .expect("the message decodes")
            .data
    };
    assert_eq!(
        receive(&[], r#"{"a": [1, 2]}"#),
        DataValue::canonical_json_object(r#"{"a":[1,2]}"#).expect("canonical JSON")
    );
    assert_eq!(receive(&[], "12"), number(12.0));
    assert_eq!(
        receive(&[], "<a><b/></a>"),
        DataValue::Xml("<a><b/></a>".into())
    );
    // B.2.8.1: "Otherwise, the Processor MUST treat the content as a
    // space-normalized string literal": XML white space (space, tab, CR, LF)
    // is trimmed and collapsed, as XPath 1.0 `normalize-space()` does; other
    // characters such as U+00A0 are content.
    assert_eq!(
        receive(&[], " \tplain \r\n words\u{a0} "),
        DataValue::String("plain words\u{a0}".into())
    );
    // JSON (RFC 8259 §2) and XML (`S`) white space is space, tab, CR, and
    // LF only: a body padded with U+1680 OGHAM SPACE MARK is neither JSON
    // nor an XML document.
    assert_eq!(
        receive(&[], " \t{\"a\": 1}\r\n"),
        DataValue::canonical_json_object(r#"{"a":1}"#).expect("canonical JSON")
    );
    assert_eq!(
        receive(&[], "\r\n<a/> "),
        DataValue::Xml("\r\n<a/> ".into())
    );
    for body in ["\u{1680}{\"a\":1}", "\u{1680}<a/>", "<a/>\u{1680}"] {
        assert_eq!(
            receive(&[], body),
            DataValue::String(body.into()),
            "{body:?}"
        );
        assert_eq!(
            receive(&[("Content-Type", "application/xml")], body),
            DataValue::String(body.into()),
            "{body:?}"
        );
    }
    assert_eq!(
        receive(
            &[("Content-Type", "text/plain")],
            "  this is  some\ncontent "
        ),
        DataValue::String("this is some content".into())
    );
    let form = [("Content-Type", FORM_MEDIA_TYPE)];
    assert_eq!(
        receive(&form, "%7B%22a%22%3A1%7D"),
        DataValue::canonical_json_object(r#"{"a":1}"#).expect("canonical JSON")
    );
    // IRP 520's `<content>this is some content</content>` keeps its text.
    assert_eq!(
        receive(&form, "this%20is%20some%20content"),
        DataValue::String("this is some content".into())
    );
    assert_eq!(
        receive(&form, "%20this%20is%09%20some%0Acontent%20"),
        DataValue::String("this is some content".into())
    );
    // Key-value pairs stay an object of strings, never JSON-read.
    assert_eq!(
        receive(&form, "p=1"),
        DataValue::JsonObject(r#"{"p":"1"}"#.into())
    );
}

#[test]
fn hostile_messages_are_rejected_with_bounded_reasons() {
    let form = [("Content-Type", FORM_MEDIA_TYPE)];
    let mut get = inbound(&[], "");
    get.method = "GET".into();
    let mut bad_method = inbound(&[], "");
    bad_method.method = "PO ST".into();
    let mut long_uri = inbound(&[], "");
    long_uri.uri = format!("http://h/{}", "a".repeat(MAX_URI_BYTES));
    let mut spaced_uri = inbound(&[], "");
    spaced_uri.uri = "http://h/a b".into();
    let mut binary = inbound(&[], "");
    binary.body = vec![0xff, 0xfe];
    let mut huge = inbound(&[], "");
    huge.body = vec![b'a'; MAX_BODY_BYTES + 1];
    let too_many_headers = inbound(&[("X-A", "1"); MAX_HEADERS + 1], "");
    for (request, rejection) in [
        (get, Rejection::MethodNotAllowed),
        (bad_method, Rejection::InvalidMessage),
        (long_uri, Rejection::PayloadTooLarge),
        (spaced_uri, Rejection::InvalidMessage),
        (binary, Rejection::InvalidMessage),
        (huge, Rejection::PayloadTooLarge),
        (too_many_headers, Rejection::PayloadTooLarge),
        (inbound(&[("X-A", "a\r\nb")], ""), Rejection::InvalidMessage),
        (inbound(&[("Bad Name", "v")], ""), Rejection::InvalidMessage),
        (inbound(&form, "a=%zz"), Rejection::InvalidMessage),
        (inbound(&form, "a=%4"), Rejection::InvalidMessage),
        (inbound(&form, "a=%FF"), Rejection::InvalidMessage),
        (inbound(&form, "content&a=1"), Rejection::InvalidMessage),
        (inbound(&form, "one&two"), Rejection::InvalidMessage),
        (
            inbound(&form, "_scxmleventname=bad%20name"),
            Rejection::InvalidMessage,
        ),
        (
            inbound(&form, &"a=1&".repeat(MAX_PARAMS + 2)),
            Rejection::PayloadTooLarge,
        ),
    ] {
        assert_eq!(BasicHttpIoProcessor.receive(&request), Err(rejection));
    }
}

#[test]
fn responses_map_every_delivery_outcome() {
    let status = |delivery| BasicHttpIoProcessor.respond(delivery).status;
    assert_eq!(status(Delivery::Enqueued), 200);
    assert_eq!(status(Delivery::Rejected(Rejection::InvalidMessage)), 400);
    assert_eq!(status(Delivery::Rejected(Rejection::PayloadTooLarge)), 413);
    assert_eq!(status(Delivery::UnknownSession), 404);
    assert_eq!(status(Delivery::QueueFull), 503);
    let not_allowed = BasicHttpIoProcessor.respond(Delivery::Rejected(Rejection::MethodNotAllowed));
    assert_eq!(not_allowed.status, 405);
    assert_eq!(
        not_allowed.headers,
        vec![HttpHeader {
            name: "Allow".into(),
            value: "POST".into(),
        }]
    );
    assert!(validate_response(&not_allowed).is_ok());
}

#[test]
fn control_and_failure_paths_emit_bounded_events() {
    let recorder = Recorder::default();
    let _guard = install_recorder(recorder.clone());
    let mut transport = answering(200);
    BasicHttpIoProcessor
        .send(&send(Some("e")), &mut transport)
        .expect("send succeeds");
    let mut refused = answering(500);
    assert_eq!(
        BasicHttpIoProcessor.send(&send(Some("e")), &mut refused),
        Err(SendError::NotAccepted)
    );
    let observations = recorder.observations();
    assert!(observations.iter().any(|observation| {
        observation.operation == Operation::BasicHttpSend
            && observation.events == vec![SignalEvent::IoProcessorSendAccepted]
    }));
    assert!(observations.iter().any(|observation| {
        observation.operation == Operation::BasicHttpSend
            && observation.error == Some(ErrorCategory::Execution)
            && observation.events == vec![SignalEvent::IoProcessorSendRejected]
    }));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Every encodable parameter set decodes to exactly the same strings,
    /// in order, through the receive side of the same processor.
    #[test]
    fn encoded_parameters_round_trip_through_receive(
        event in "[A-Za-z][A-Za-z0-9._]{0,15}",
        params in proptest::collection::vec(("[a-z]{1,6}", "\\PC{0,24}"), 0..8),
    ) {
        let mut outbound = send(Some(&event));
        outbound.params = params
            .iter()
            .map(|(name, value)| Param {
                name: name.clone(),
                value: DataValue::String(value.clone()),
            })
            .collect();
        let request = BasicHttpIoProcessor.encode(&outbound).expect("bounded send encodes");
        let decoded = BasicHttpIoProcessor.receive(&request).expect("encoded send decodes");
        prop_assert_eq!(decoded.name.as_str(), event.as_str());
        let mut expected = serde_json::Map::new();
        for (name, value) in &params {
            match expected.get_mut(name) {
                None => {
                    expected.insert(name.clone(), serde_json::Value::String(value.clone()));
                }
                Some(serde_json::Value::Array(values)) => {
                    values.push(serde_json::Value::String(value.clone()));
                }
                Some(existing) => {
                    let first = existing.clone();
                    *existing = serde_json::Value::Array(vec![
                        first,
                        serde_json::Value::String(value.clone()),
                    ]);
                }
            }
        }
        let expected = if params.is_empty() {
            DataValue::Absent
        } else {
            DataValue::canonical_json_object(
                &serde_json::to_string(&serde_json::Value::Object(expected)).expect("json"),
            )
            .expect("canonical")
        };
        prop_assert_eq!(decoded.data, expected);
    }

    /// Arbitrary inbound bytes never panic; they decode or are rejected.
    #[test]
    fn arbitrary_messages_never_panic(
        method in "[A-Z]{1,8}",
        body in proptest::collection::vec(any::<u8>(), 0..256),
        form in any::<bool>(),
    ) {
        let request = HttpRequest {
            method,
            uri: TARGET.into(),
            headers: if form {
                vec![HttpHeader { name: "Content-Type".into(), value: FORM_MEDIA_TYPE.into() }]
            } else {
                Vec::new()
            },
            body,
        };
        let _ = BasicHttpIoProcessor.receive(&request);
    }
}
