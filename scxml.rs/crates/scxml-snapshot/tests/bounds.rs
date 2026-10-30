//! Every bound the decoder enforces holds exactly: a document at the bound
//! has no `bounds-exceeded` issue, and one past it has one at the bound's
//! pointer; a quantity the format does not bound (a count, an identifier
//! length) has none at any size. Documents are edited through the typed
//! model and re-hashed, so each case reaches the bounds pass (shape,
//! canonical form, and digest hold).

use std::fmt::Write as _;

use scxml_snapshot::{
    BaggagePolicy, Capture, ComponentIdentity, ComponentRole, DataModelCapture, Document, Event,
    EventType, HistoryValue, Invocation, InvokedBy, MAX_TYPEOF_BYTES, MAX_VALUE_BYTES,
    MachineCapture, Options, OversizedValue, Param, PendingSend, ProcessorMessage, SessionCapture,
    SnapshotError, SnapshotErrorCode, Status, System, TextValue, TraceContext, TransitionCapture,
    TransitionType, Value, Variable, VariableValue, check, decode, encode,
};
use sha2::Digest as _;

/// Largest JSON integer a document may hold (2^53 − 1).
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

fn digest(fill: char) -> String {
    std::iter::repeat_n(fill, 64).collect()
}

fn event(name: &str) -> Event {
    Event {
        name: name.into(),
        event_type: EventType::External,
        origin: None,
        origintype: None,
        sendid: None,
        invokeid: None,
        data: Value::Absent,
        raw: None,
        trace_context: None,
    }
}

fn base() -> Document {
    let capture = Capture {
        components: [
            ComponentRole::Clock,
            ComponentRole::DataModelEcmascript,
            ComponentRole::IoProcessorScxml,
            ComponentRole::Machine,
        ]
        .into_iter()
        .map(|role| ComponentIdentity {
            role,
            sha256: digest('b'),
        })
        .collect(),
        now_ms: 0,
        sessions: vec![SessionCapture {
            parent: None,
            session_id: "session-1".into(),
            ir_sha256: digest('a'),
            data_model_name: Some("ecmascript".into()),
            machine: MachineCapture {
                status: Status::Running,
                name: Some("main".into()),
                atomic: vec!["s".into()],
                configuration: vec!["s".into()],
                internal: vec![],
                history: vec![],
                bound_states: vec![],
                invocations: vec![],
                invoked_by: None,
                send_counter: 0,
                invoke_counter: 0,
                transitions: vec![TransitionCapture {
                    source: "s".into(),
                    index: 0,
                    events: vec!["go".into()],
                    targets: vec!["s".into()],
                    transition_type: TransitionType::External,
                    cond: None,
                    domain: None,
                    exit: vec!["s".into()],
                    entry: vec!["s".into()],
                    event_schema: None,
                }],
            },
            data_model: Some(DataModelCapture {
                system: System {
                    sessionid: "session-1".into(),
                    name: Some("main".into()),
                    ioprocessors: Value::JsonObject("{}".into()),
                },
                variables: vec![Variable {
                    name: "x".into(),
                    value: VariableValue::Value(Value::Boolean(true)),
                }],
            }),
            pending_sends: vec![],
            external: vec![],
            finished: false,
        }],
    };
    let bytes = encode(&capture, &Options::default()).expect("the base capture encodes");
    decode(&bytes).expect("the base document decodes")
}

/// Canonical bytes of an edited document, with its digest recomputed.
fn rehash(document: &mut Document) -> Vec<u8> {
    let snapshot = serde_json::to_vec(&document.snapshot).expect("serializes");
    let mut hex = String::with_capacity(64);
    for byte in sha2::Sha256::digest(&snapshot) {
        write!(hex, "{byte:02x}").expect("writing to a string cannot fail");
    }
    document.sha256 = hex;
    serde_json::to_vec(document).expect("serializes")
}

fn bound_pointers(edit: &dyn Fn(&mut Document, usize), size: usize) -> Vec<String> {
    let mut document = base();
    edit(&mut document, size);
    let bytes = rehash(&mut document);
    let issues: Vec<SnapshotError> = check(&bytes).expect("an issue list");
    issues
        .into_iter()
        .filter(|issue| issue.code == SnapshotErrorCode::BoundsExceeded)
        .map(|issue| issue.pointer.unwrap_or_default())
        .collect()
}

/// `edit(document, n)` sets the bounded quantity to `n`.
fn assert_bound(name: &str, max: usize, pointer: &str, edit: &dyn Fn(&mut Document, usize)) {
    assert_eq!(
        bound_pointers(edit, max),
        Vec::<String>::new(),
        "{name} at its maximum {max}"
    );
    let over = bound_pointers(edit, max + 1);
    assert_eq!(
        over.first().map(String::as_str),
        Some(pointer),
        "{name} one past its maximum: {over:?}"
    );
}

/// `edit(document, n)` sets an unbounded quantity to `n`: no bound fails.
fn assert_unbounded(name: &str, size: usize, edit: &dyn Fn(&mut Document, usize)) {
    assert_eq!(
        bound_pointers(edit, size),
        Vec::<String>::new(),
        "{name} at {size}"
    );
}

fn ids(prefix: &str, count: usize) -> Vec<String> {
    (0..count).map(|index| format!("{prefix}{index}")).collect()
}

fn root(document: &mut Document) -> &mut scxml_snapshot::Session {
    &mut document.snapshot.sessions[0]
}

fn first_variable(document: &mut Document) -> &mut Variable {
    root(document)
        .data_model
        .as_mut()
        .and_then(|variables| variables.first_mut())
        .expect("the base has a variable")
}

fn external(document: &mut Document) -> &mut Vec<Event> {
    &mut root(document).queues.as_mut().expect("queues").external
}

fn first_event(document: &mut Document) -> &mut Event {
    let queue = external(document);
    if queue.is_empty() {
        queue.push(event("e"));
    }
    &mut queue[0]
}

fn carrier(document: &mut Document) -> &mut TraceContext {
    first_event(document)
        .trace_context
        .get_or_insert_with(|| TraceContext {
            traceparent: "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".into(),
            tracestate: None,
            baggage: None,
            baggage_policy: BaggagePolicy::Drop,
        })
}

fn transition(document: &mut Document) -> &mut scxml_snapshot::Transition {
    &mut root(document).transitions.as_mut().expect("transitions")[0]
}

fn pending(document: &mut Document) -> &mut Vec<PendingSend> {
    root(document).pending_sends.as_mut().expect("pending")
}

fn send(id: &str) -> PendingSend {
    PendingSend {
        sendid: id.into(),
        event: Some(event("timeout")),
        message: None,
        target: None,
        send_type: None,
        delay_ms: 0,
        deadline_ms: 0,
    }
}

#[test]
fn session_ids_names_and_configurations_have_no_bound() {
    assert_unbounded("session_id", 4_096, &|d, n| {
        root(d).session_id = "s".repeat(n);
    });
    assert_unbounded("name", 4_096, &|d, n| {
        root(d).name = Some("n".repeat(n));
    });
    assert_unbounded("configuration.atomic", 10_001, &|d, n| {
        root(d).configuration.atomic = ids("s", n);
    });
    assert_unbounded("configuration.full", 10_001, &|d, n| {
        root(d).configuration.full = ids("s", n);
    });
    assert_unbounded("history", 10_001, &|d, n| {
        root(d).history = ids("h", n)
            .into_iter()
            .map(|state| HistoryValue {
                state,
                value: vec![],
            })
            .collect();
    });
    assert_unbounded("history value", 10_001, &|d, n| {
        root(d).history = vec![HistoryValue {
            state: "h".into(),
            value: ids("s", n),
        }];
    });
    assert_unbounded("bound_states", 10_001, &|d, n| {
        root(d).bound_states = ids("s", n);
    });
}

#[test]
fn integer_bounds() {
    let integer = |name: &str, pointer: &str, set: &dyn Fn(&mut Document, u64)| {
        let at = |value: u64| {
            let mut document = base();
            set(&mut document, value);
            let bytes = rehash(&mut document);
            check(&bytes)
                .expect("an issue list")
                .into_iter()
                .filter(|issue| issue.code == SnapshotErrorCode::BoundsExceeded)
                .filter_map(|issue| issue.pointer)
                .collect::<Vec<_>>()
        };
        assert_eq!(at(MAX_SAFE_INTEGER), Vec::<String>::new(), "{name}");
        assert_eq!(at(MAX_SAFE_INTEGER + 1), vec![pointer.to_owned()], "{name}");
    };
    integer("now_ms", "/snapshot/clock/now_ms", &|d, v| {
        d.snapshot.clock.now_ms = v;
    });
    integer(
        "counters.send",
        "/snapshot/sessions/0/counters/send",
        &|d, v| {
            root(d).counters.send = v;
        },
    );
    integer(
        "counters.invoke",
        "/snapshot/sessions/0/counters/invoke",
        &|d, v| {
            root(d).counters.invoke = v;
        },
    );
    integer(
        "delay_ms",
        "/snapshot/sessions/0/pending_sends/0/delay_ms",
        &|d, v| {
            let mut entry = send("sendid_0");
            entry.delay_ms = v;
            *pending(d) = vec![entry];
        },
    );
    integer(
        "deadline_ms",
        "/snapshot/sessions/0/pending_sends/0/deadline_ms",
        &|d, v| {
            let mut entry = send("sendid_0");
            entry.deadline_ms = v;
            *pending(d) = vec![entry];
        },
    );
    integer(
        "oversized bytes",
        "/snapshot/sessions/0/data_model/0/value/bytes",
        &|d, v| first_variable(d).value = VariableValue::Oversized(OversizedValue::new(v)),
    );
}

#[test]
fn data_model_bounds() {
    assert_unbounded("variables", 257, &|d, n| {
        *root(d).data_model.as_mut().expect("data_model") = ids("v", n)
            .into_iter()
            .map(|name| Variable {
                name,
                value: VariableValue::Value(Value::Null),
            })
            .collect();
    });
    assert_unbounded("variable name", 4_096, &|d, n| {
        first_variable(d).name = "v".repeat(n);
    });
    assert_bound(
        "variable text",
        MAX_VALUE_BYTES,
        "/snapshot/sessions/0/data_model/0/value/text",
        &|d, n| {
            first_variable(d).value =
                VariableValue::Text(TextValue::new("function", "f".repeat(n)));
        },
    );
    assert_bound(
        "variable typeof",
        MAX_TYPEOF_BYTES,
        "/snapshot/sessions/0/data_model/0/value/typeof",
        &|d, n| {
            first_variable(d).value = VariableValue::Text(TextValue::new("t".repeat(n), "x"));
        },
    );
    for make in [Value::Text, Value::Xml] {
        assert_bound(
            "variable value",
            MAX_VALUE_BYTES,
            "/snapshot/sessions/0/data_model/0/value/value",
            &|d, n| first_variable(d).value = VariableValue::Value(make("t".repeat(n))),
        );
    }
    assert_bound(
        "json array value",
        MAX_VALUE_BYTES,
        "/snapshot/sessions/0/data_model/0/value/value",
        &|d, n| {
            // `["aa…"]`, exactly `n` bytes.
            let text = format!("[\"{}\"]", "a".repeat(n - 4));
            first_variable(d).value = VariableValue::Value(Value::JsonArray(text));
        },
    );
    assert_bound(
        "json object value",
        MAX_VALUE_BYTES,
        "/snapshot/sessions/0/data_model/0/value/value",
        &|d, n| {
            let text = format!("{{\"a\":\"{}\"}}", "a".repeat(n - 8));
            first_variable(d).value = VariableValue::Value(Value::JsonObject(text));
        },
    );
}

/// A value longer than the inline bound is written as its size only, and a
/// value that fits is never written that way.
#[test]
fn oversized_values_start_one_past_the_inline_bound() {
    let at = |bytes: usize| {
        let mut document = base();
        first_variable(&mut document).value =
            VariableValue::Oversized(OversizedValue::new(bytes as u64));
        let bytes = rehash(&mut document);
        decode(&bytes)
            .map(drop)
            .map_err(|error| (error.code, error.pointer))
    };
    assert_eq!(at(MAX_VALUE_BYTES + 1), Ok(()));
    assert_eq!(
        at(MAX_VALUE_BYTES),
        Err((
            SnapshotErrorCode::Inconsistent,
            Some("/snapshot/sessions/0/data_model/0/value/bytes".into())
        ))
    );
    let inline = |bytes: usize| {
        let mut document = base();
        first_variable(&mut document).value = VariableValue::Value(Value::Text("t".repeat(bytes)));
        let bytes = rehash(&mut document);
        decode(&bytes).map(drop).map_err(|error| error.code)
    };
    assert_eq!(inline(MAX_VALUE_BYTES), Ok(()));
    assert_eq!(
        inline(MAX_VALUE_BYTES + 1),
        Err(SnapshotErrorCode::BoundsExceeded)
    );
    let mut document = base();
    first_variable(&mut document).value =
        VariableValue::Oversized(OversizedValue::new(MAX_VALUE_BYTES as u64 + 1));
    let bytes = rehash(&mut document);
    assert!(
        String::from_utf8_lossy(&bytes)
            .contains(r#"{"name":"x","value":{"type":"oversized","bytes":65537}}"#)
    );
    let shown = scxml_snapshot::render_show(&document, scxml_snapshot::Color::Never);
    assert!(shown.contains("x = <oversized: 65537 bytes>"), "{shown}");
}

#[test]
fn queues_have_no_entry_bound() {
    for (key, set) in [
        (
            "internal",
            (|d: &mut Document, n: usize| {
                root(d).queues.as_mut().expect("queues").internal = vec![event("e"); n];
            }) as fn(&mut Document, usize),
        ),
        ("external", |d, n| *external(d) = vec![event("e"); n]),
    ] {
        assert_unbounded(key, 10_001, &set);
    }
}

#[test]
fn event_bounds() {
    let at = "/snapshot/sessions/0/queues/external/0";
    assert_unbounded("event name", 4_096, &|d, n| {
        first_event(d).name = "e".repeat(n);
    });
    // Event metadata, raw, and data have no byte bound (SCXML sets none).
    let large = 4 * 65_536;
    assert_eq!(
        bound_pointers(
            &|d, n| {
                let event = first_event(d);
                event.origin = Some("m".repeat(n));
                event.origintype = Some("m".repeat(n));
                event.sendid = Some("m".repeat(n));
                event.invokeid = Some("m".repeat(n));
                event.raw = Some("r".repeat(n));
                event.data = Value::Text("t".repeat(n));
            },
            large
        ),
        Vec::<String>::new()
    );
    assert_bound(
        "traceparent",
        scxml_types::TRACEPARENT_BYTES,
        &format!("{at}/trace_context/traceparent"),
        &|d, n| carrier(d).traceparent = "0".repeat(n),
    );
    assert_bound(
        "tracestate",
        scxml_types::MAX_TRACESTATE_BYTES,
        &format!("{at}/trace_context/tracestate"),
        &|d, n| carrier(d).tracestate = Some("t".repeat(n)),
    );
    assert_bound(
        "baggage",
        scxml_types::MAX_BAGGAGE_BYTES,
        &format!("{at}/trace_context/baggage"),
        &|d, n| carrier(d).baggage = Some("b".repeat(n)),
    );
}

#[test]
fn pending_sends_have_no_count_or_name_bound() {
    assert_unbounded("pending_sends", 1_025, &|d, n| {
        *pending(d) = ids("sendid_", n).iter().map(|id| send(id)).collect();
    });
    let large = 4 * 65_536;
    assert_eq!(
        bound_pointers(
            &|d, n| {
                let mut entry = send("sendid_0");
                entry.target = Some("t".repeat(n));
                entry.send_type = Some("t".repeat(n));
                *pending(d) = vec![entry];
            },
            large
        ),
        Vec::<String>::new(),
        "a pending send's target and type have no byte bound"
    );
    assert_unbounded("processor message param name", 4_096, &|d, n| {
        let mut entry = post("sendid_0");
        if let Some(message) = &mut entry.message {
            message.params = vec![Param {
                name: "p".repeat(n),
                value: Value::Null,
            }];
        }
        *pending(d) = vec![entry];
    });
}

/// A pending `BasicHTTP` send (SCXML C.2).
fn post(id: &str) -> PendingSend {
    PendingSend {
        event: None,
        message: Some(ProcessorMessage {
            event: Some("ready".into()),
            params: vec![Param {
                name: "k".into(),
                value: Value::Text("v".into()),
            }],
            content: None,
            trace_context: None,
        }),
        ..send(id)
    }
}

#[test]
fn a_pending_send_holds_exactly_one_message() {
    let issues = |entry: PendingSend| {
        let mut document = base();
        *pending(&mut document) = vec![entry];
        let bytes = rehash(&mut document);
        check(&bytes)
            .expect("an issue list")
            .into_iter()
            .map(|issue| (issue.code, issue.pointer.unwrap_or_default()))
            .collect::<Vec<_>>()
    };
    assert_eq!(issues(post("sendid_0")), []);
    assert_eq!(issues(send("sendid_0")), []);
    let both = PendingSend {
        event: Some(event("timeout")),
        ..post("sendid_0")
    };
    let neither = PendingSend {
        message: None,
        ..post("sendid_0")
    };
    for entry in [both, neither] {
        assert_eq!(
            issues(entry),
            [(
                SnapshotErrorCode::Inconsistent,
                "/snapshot/sessions/0/pending_sends/0".to_owned()
            )]
        );
    }
}

#[test]
fn transitions_have_no_count_or_length_bound() {
    assert_unbounded("transitions", 10_001, &|d, n| {
        let first = transition(d).clone();
        *root(d).transitions.as_mut().expect("transitions") = (0..n)
            .map(|index| {
                let mut entry = first.clone();
                entry.index = u32::try_from(index).expect("small");
                entry
            })
            .collect();
    });
    assert_unbounded("events per transition", 10_001, &|d, n| {
        transition(d).events = vec!["go".into(); n];
    });
    assert_unbounded("cond", 65_536, &|d, n| {
        let entry = transition(d);
        entry.cond = Some("c".repeat(n));
        entry.has_cond = true;
    });
    assert_unbounded("targets", 10_001, &|d, n| {
        transition(d).targets = ids("s", n);
    });
    assert_unbounded("exit", 10_001, &|d, n| {
        transition(d).exit = Some(ids("s", n));
    });
    assert_unbounded("entry", 10_001, &|d, n| {
        transition(d).entry = Some(ids("s", n));
    });
}

#[test]
fn invocations_have_no_count_bound() {
    let child = |d: &Document, index: usize| {
        let mut session = d.snapshot.sessions[0].clone();
        let invoke_id = format!("i{index}");
        session.session_id = format!("child-{index}");
        session.invoked_by = Some(InvokedBy {
            parent_session_id: d.snapshot.sessions[0].session_id.clone(),
            invoke_id: invoke_id.clone(),
        });
        session.invocations = Some(Vec::new());
        let invocation = Invocation {
            invoke_id,
            invoke_type: "http://www.w3.org/TR/scxml/".into(),
            state: "s".into(),
            index: 0,
            autoforward: false,
            finished: false,
        };
        (invocation, session)
    };
    assert_unbounded("invocations", 256, &|d, n| {
        let (invocations, sessions): (Vec<_>, Vec<_>) = (0..n).map(|index| child(d, index)).unzip();
        root(d).invocations = Some(invocations);
        d.snapshot.sessions.extend(sessions);
    });
    assert_eq!(
        bound_pointers(
            &|d, n| {
                let (mut invocation, session) = child(d, 0);
                invocation.invoke_type = "t".repeat(n);
                root(d).invocations = Some(vec![invocation]);
                d.snapshot.sessions.push(session);
            },
            4 * 65_536
        ),
        Vec::<String>::new(),
        "an invocation type has no byte bound"
    );
}
