//! Encoder, decoder, diff, and renderer behavior of `scxml.session-snapshot/v2`.

use proptest::prelude::*;
use scxml_snapshot::{
    BaggagePolicy, Capture, Change, Color, ComponentIdentity, ComponentRole, DataModelCapture,
    DiffOptions, Event, EventSchema, EventType, FifoDiff, HistoryValue, InvocationCapture,
    InvokedBy, MAX_INVOKE_DEPTH, MAX_PENDING_SENDS, MAX_VARIABLES, MachineCapture, Options,
    PendingSend, SESSION_SNAPSHOT_DIFF_VERSION, Section, SessionCapture, SnapshotErrorCode,
    SnapshotPoint, Status, System, TextValue, TraceContext, TraceContextPolicy, TransitionCapture,
    TransitionType, Value, Variable, VariableValue, check, decode, diff, diff_documents, encode,
    render_diff, render_show,
};

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
        trace_context: Some(TraceContext {
            traceparent: "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".into(),
            tracestate: None,
            baggage: None,
            baggage_policy: BaggagePolicy::Drop,
        }),
    }
}

fn pending(sendid: &str, deadline_ms: u64) -> PendingSend {
    PendingSend {
        sendid: sendid.into(),
        event: Some(event("timeout")),
        basichttp: None,
        target: None,
        send_type: None,
        delay_ms: 100,
        deadline_ms,
    }
}

fn child_capture() -> SessionCapture {
    SessionCapture {
        parent: Some(0),
        session_id: "child1".into(),
        ir_sha256: digest('c'),
        data_model_name: Some("ecmascript".into()),
        machine: MachineCapture {
            status: Status::Running,
            name: Some("worker".into()),
            atomic: vec!["w".into()],
            configuration: vec!["w".into()],
            internal: vec![],
            history: vec![],
            bound_states: vec![],
            invocations: vec![],
            invoked_by: Some(InvokedBy {
                parent_session_id: "root1".into(),
                invoke_id: "i1".into(),
            }),
            send_counter: 0,
            invoke_counter: 0,
            transitions: vec![],
        },
        data_model: Some(DataModelCapture {
            system: System {
                sessionid: "child1".into(),
                name: Some("worker".into()),
                ioprocessors: Value::JsonObject("{}".into()),
            },
            variables: vec![],
        }),
        pending_sends: vec![],
        external: vec![event("ping")],
        finished: false,
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one literal fixture covering every section"
)]
fn capture() -> Capture {
    let root = SessionCapture {
        parent: None,
        session_id: "root1".into(),
        ir_sha256: digest('a'),
        data_model_name: Some("ecmascript".into()),
        machine: MachineCapture {
            status: Status::Running,
            name: Some("main".into()),
            atomic: vec!["s0.a".into()],
            configuration: vec!["s0".into(), "s0.a".into()],
            internal: vec![],
            history: vec![HistoryValue {
                state: "h".into(),
                value: vec!["s0.a".into()],
            }],
            bound_states: vec!["s0".into()],
            invocations: vec![InvocationCapture {
                invoke_id: "i1".into(),
                state: "s0".into(),
                index: 0,
                type_uri: "http://www.w3.org/TR/scxml/".into(),
                autoforward: true,
            }],
            invoked_by: None,
            send_counter: 1,
            invoke_counter: 1,
            transitions: vec![
                TransitionCapture {
                    source: "s0".into(),
                    index: 1,
                    events: vec!["go".into(), "error.*".into()],
                    targets: vec!["s1".into()],
                    transition_type: TransitionType::External,
                    cond: Some("x > 1".into()),
                    domain: Some("main".into()),
                    exit: vec!["s0.a".into(), "s0".into()],
                    entry: vec!["s1".into()],
                    event_schema: None,
                },
                TransitionCapture {
                    source: "s0.a".into(),
                    index: 0,
                    events: vec!["tick".into()],
                    targets: vec![],
                    transition_type: TransitionType::Internal,
                    cond: None,
                    domain: None,
                    exit: vec![],
                    entry: vec![],
                    event_schema: None,
                },
            ],
        },
        data_model: Some(DataModelCapture {
            system: System {
                sessionid: "root1".into(),
                name: Some("main".into()),
                ioprocessors: Value::JsonObject("{}".into()),
            },
            variables: vec![
                Variable {
                    name: "f".into(),
                    value: VariableValue::Text(TextValue::new(
                        "function",
                        "function () { return 1; }",
                    )),
                },
                Variable {
                    name: "x".into(),
                    value: VariableValue::Value(Value::Number("1.5".into())),
                },
            ],
        }),
        pending_sends: vec![pending("sendid_0", 350)],
        external: vec![event("go")],
        finished: false,
    };
    Capture {
        components: vec![
            ComponentIdentity {
                role: ComponentRole::Machine,
                sha256: digest('1'),
            },
            ComponentIdentity {
                role: ComponentRole::Clock,
                sha256: digest('2'),
            },
            ComponentIdentity {
                role: ComponentRole::IoProcessorScxml,
                sha256: digest('3'),
            },
            ComponentIdentity {
                role: ComponentRole::DataModelEcmascript,
                sha256: digest('4'),
            },
        ],
        now_ms: 250,
        sessions: vec![root, child_capture()],
    }
}

fn options(exclude: &[Section], trace_context: TraceContextPolicy) -> Options {
    Options {
        exclude: exclude.to_vec(),
        trace_context,
        point: SnapshotPoint::Checkpoint,
    }
}

fn encoded() -> Vec<u8> {
    encode(&capture(), &Options::default()).expect("the fixture encodes")
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("documents are UTF-8")
}

#[test]
fn encodes_canonical_bytes_that_decode_and_reencode_identically() {
    let bytes = encoded();
    let document = decode(&bytes).expect("the encoder output decodes");
    assert_eq!(serde_json::to_vec(&document).expect("re-encodes"), bytes);
    assert_eq!(check(&bytes).expect("checks"), vec![]);
    let body = text(&bytes);
    assert!(body.starts_with(r#"{"version":"scxml.session-snapshot/v2","sha256":""#));
    assert!(!body.contains("null") && !body.contains(": ") && !body.ends_with('\n'));
    // Components are sorted by role; strip removes every carrier.
    assert!(body.contains(r#""components":[{"role":"clock""#));
    assert!(!body.contains("traceparent"));
    // Derived fields.
    assert!(body.contains(r#""type":"external","cond":"x > 1","has_cond":true,"domain":"main""#));
    assert!(body.contains(r#""type":"internal","has_cond":false,"exit":[],"entry":[]"#));
    assert!(!body.contains(r#""action""#) && !body.contains(r#""cancel""#));
    assert!(body.contains(r#""invocations":[{"invoke_id":"i1","type":"http://www.w3.org/TR/scxml/","state":"s0","index":0,"autoforward":true,"finished":false,"session":{"session_id":"child1","name":"worker""#));
    assert_eq!(document.snapshot.root.name.as_deref(), Some("main"));
    assert_eq!(
        encode(&capture(), &Options::default()).expect("encodes"),
        bytes
    );
}

#[test]
fn preserve_keeps_carriers_and_exclusions_omit_sections() {
    let preserved =
        encode(&capture(), &options(&[], TraceContextPolicy::Preserve)).expect("encodes");
    assert!(text(&preserved).contains(r#""trace_context":{"traceparent":"00-"#));
    decode(&preserved).expect("decodes");

    let all = [
        Section::Previews,
        Section::System,
        Section::Invocations,
        Section::DataModel,
        Section::Queues,
        Section::PendingSends,
        Section::System,
    ];
    let bytes = encode(&capture(), &options(&all, TraceContextPolicy::Strip))
        .expect_err("children excluded but child captures present");
    assert_eq!(bytes.code, SnapshotErrorCode::Inconsistent);
    let mut root_only = capture();
    root_only.sessions.truncate(1);
    let bytes = encode(&root_only, &options(&all, TraceContextPolicy::Strip)).expect("encodes");
    let body = text(&bytes);
    assert!(body.contains(
        r#""options":{"exclude":["system","data_model","queues","pending_sends","previews","invocations"],"trace_context":"strip"}"#
    ));
    for key in [
        "\"system\"",
        "\"data_model\"",
        "\"queues\"",
        "\"pending_sends\"",
        "\"invocations\"",
        "\"exit\"",
    ] {
        assert!(!body.contains(&format!("{key}:")), "{key} was excluded");
    }
    assert!(body.contains(r#""transitions":[{"source":"s0""#));
    decode(&bytes).expect("decodes");
}

#[test]
fn ready_sessions_omit_the_data_model_image() {
    let mut ready = capture();
    ready.sessions.truncate(1);
    let root = &mut ready.sessions[0];
    root.machine.status = Status::Ready;
    root.machine.atomic.clear();
    root.machine.configuration.clear();
    root.machine.history.clear();
    root.machine.bound_states.clear();
    root.machine.invocations.clear();
    root.machine.transitions.clear();
    root.data_model = None;
    let bytes = encode(&ready, &Options::default()).expect("encodes");
    assert!(!text(&bytes).contains("\"system\":"));
    decode(&bytes).expect("decodes");
}

#[test]
fn encoder_rejects_captures_outside_a_checkpoint_or_the_invocation_shape() {
    let mut busy = capture();
    busy.sessions[0].machine.internal.push(event("x"));
    let error = encode(&busy, &Options::default()).expect_err("internal queue non-empty");
    assert_eq!(error.code, SnapshotErrorCode::NotStable);
    assert_eq!(
        error.pointer.as_deref(),
        Some("/snapshot/root/queues/internal")
    );

    // A session its parent never invoked is an orphan.
    let mut stray = capture();
    let mut grandchild = child_capture();
    grandchild.parent = Some(1);
    grandchild.session_id = "grandchild".into();
    stray.sessions.push(grandchild);
    assert_eq!(
        encode(&stray, &Options::default()).map_err(|error| error.pointer),
        Err(Some("/sessions/2/machine".into()))
    );
    // A chain one session deeper than the ceiling.
    assert_eq!(
        encode(&chain(MAX_INVOKE_DEPTH), &Options::default()).map(|_| ()),
        Ok(())
    );
    assert_eq!(
        encode(&chain(MAX_INVOKE_DEPTH + 1), &Options::default()).map_err(|error| error.code),
        Err(SnapshotErrorCode::DepthExceeded)
    );

    let mut orphan = capture();
    orphan.sessions[0].machine.invocations.clear();
    assert_eq!(
        encode(&orphan, &Options::default()).map_err(|error| error.pointer),
        Err(Some("/sessions/1/machine".into()))
    );

    let mut missing_role = capture();
    missing_role.components.pop();
    assert_eq!(
        encode(&missing_role, &Options::default()).map_err(|error| error.code),
        Err(SnapshotErrorCode::Inconsistent)
    );

    let mut bad_name = capture();
    bad_name.sessions[0].session_id = "has space".into();
    assert_eq!(
        encode(&bad_name, &Options::default()).map_err(|error| error.code),
        Err(SnapshotErrorCode::InvalidName)
    );

    let mut float = capture();
    if let Some(data_model) = &mut float.sessions[0].data_model {
        data_model.variables[1].value = VariableValue::Value(Value::Number("1e3".into()));
    }
    assert_eq!(
        encode(&float, &Options::default()).map_err(|error| error.code),
        Err(SnapshotErrorCode::NonCanonical)
    );
}

/// An executable-content document (`options.point = executable-content`) holds
/// one session mid-macrostep: its internal queue may hold events and it may be
/// an invoked session, but it must exclude `invocations` and `previews`; a
/// checkpoint document keeps every checkpoint rule.
#[test]
fn executable_content_documents_hold_one_busy_session_without_children_or_previews() {
    let executable_content = Options {
        exclude: vec![Section::Previews, Section::Invocations],
        trace_context: TraceContextPolicy::Strip,
        point: SnapshotPoint::ExecutableContent,
    };
    let mut busy = capture();
    busy.sessions.truncate(1);
    busy.sessions[0].machine.invocations.clear();
    busy.sessions[0].machine.internal.push(event("x"));
    let mut invoked = busy.clone();
    invoked.sessions[0].machine.internal.clear();
    invoked.sessions[0].machine.invoked_by = Some(InvokedBy {
        parent_session_id: "parent".into(),
        invoke_id: "i9".into(),
    });
    for capture in [&busy, &invoked] {
        let bytes =
            encode(capture, &executable_content).expect("an executable-content document encodes");
        assert!(text(&bytes).contains(r#""point":"executable-content""#));
        let document = decode(&bytes).expect("and decodes");
        assert_eq!(document.options.point, SnapshotPoint::ExecutableContent);
    }
    let busy_document =
        decode(&encode(&busy, &executable_content).expect("encodes")).expect("decodes");
    assert_eq!(
        busy_document
            .snapshot
            .root
            .queues
            .as_ref()
            .map(|queues| queues.internal.len()),
        Some(1)
    );
    let checkpoint = Options {
        point: SnapshotPoint::Checkpoint,
        ..executable_content.clone()
    };
    assert_eq!(
        encode(&busy, &checkpoint).map_err(|error| error.code),
        Err(SnapshotErrorCode::NotStable)
    );
    assert_eq!(
        encode(&invoked, &checkpoint).map_err(|error| error.code),
        Err(SnapshotErrorCode::Inconsistent)
    );
    for exclude in [vec![Section::Previews], vec![Section::Invocations]] {
        let partial = Options {
            exclude,
            ..executable_content.clone()
        };
        assert_eq!(
            encode(&busy, &partial).map_err(|error| error.pointer),
            Err(Some("/options/point".into()))
        );
    }
    // The checkpoint default is never written, so a document that spells
    // it out is not canonical.
    let plain = encode(&capture(), &Options::default()).expect("a checkpoint document");
    assert!(!text(&plain).contains("point"));
}

#[test]
fn children_are_listed_in_ascending_invoke_id_order() {
    let mut two = capture();
    let mut second = child_capture();
    second.session_id = "child0".into();
    second.machine.invoked_by = Some(InvokedBy {
        parent_session_id: "root1".into(),
        invoke_id: "a0".into(),
    });
    if let Some(model) = &mut second.data_model {
        model.system.sessionid = "child0".into();
    }
    two.sessions.push(second);
    two.sessions[0].machine.invocations.push(InvocationCapture {
        invoke_id: "a0".into(),
        state: "s0".into(),
        index: 1,
        type_uri: "http://www.w3.org/TR/scxml/".into(),
        autoforward: false,
    });
    let document = decode(&encode(&two, &Options::default()).expect("encodes")).expect("decodes");
    let ids: Vec<&str> = document
        .snapshot
        .root
        .invocations
        .iter()
        .flatten()
        .map(|invocation| invocation.invoke_id.as_str())
        .collect();
    assert_eq!(ids, ["a0", "i1"]);
}

fn expect_code(bytes: &[u8], code: SnapshotErrorCode) {
    let error = decode(bytes).expect_err("the mutation is rejected");
    assert_eq!(error.code, code, "{}", String::from_utf8_lossy(bytes));
}

#[test]
fn decoder_rejects_every_mutation_with_its_code() {
    let bytes = encoded();
    let body = text(&bytes).to_owned();

    expect_code(b"not json", SnapshotErrorCode::MalformedJson);
    expect_code(
        body.replace("session-snapshot/v2", "session-snapshot/v3")
            .as_bytes(),
        SnapshotErrorCode::UnsupportedVersion,
    );
    expect_code(
        body.replacen(r#""status":"running""#, r#""status":"failed""#, 1)
            .as_bytes(),
        SnapshotErrorCode::SchemaViolation,
    );
    expect_code(
        body.replacen(r#""counters":{"#, r#""extra":1,"counters":{"#, 1)
            .as_bytes(),
        SnapshotErrorCode::SchemaViolation,
    );
    expect_code(
        body.replacen(r#""now_ms":250"#, r#""now_ms":250.0"#, 1)
            .as_bytes(),
        SnapshotErrorCode::SchemaViolation,
    );
    expect_code(
        body.replacen(r#""now_ms":250"#, r#""now_ms":2.5e2"#, 1)
            .as_bytes(),
        SnapshotErrorCode::SchemaViolation,
    );
    // Whitespace, a trailing newline, swapped keys, and a written `null`.
    expect_code(
        body.replacen(':', ": ", 1).as_bytes(),
        SnapshotErrorCode::NonCanonical,
    );
    expect_code(
        format!("{body}\n").as_bytes(),
        SnapshotErrorCode::NonCanonical,
    );
    expect_code(
        body.replacen(r#""send":1,"invoke":1"#, r#""invoke":1,"send":1"#, 1)
            .as_bytes(),
        SnapshotErrorCode::NonCanonical,
    );
    expect_code(
        body.replacen(
            r#""datamodel_name":"#,
            r#""invoked_by":null,"datamodel_name":"#,
            1,
        )
        .as_bytes(),
        SnapshotErrorCode::NonCanonical,
    );
    // Flipped payload byte: canonical, but the digest no longer matches.
    expect_code(
        body.replacen("\"go\"", "\"gp\"", 1).as_bytes(),
        SnapshotErrorCode::IntegrityMismatch,
    );
    let mut oversized = bytes.clone();
    oversized.resize(scxml_snapshot::MAX_DOCUMENT_BYTES + 1, b' ');
    expect_code(&oversized, SnapshotErrorCode::TooLarge);
    assert_eq!(
        check(&oversized).map_err(|error| error.code),
        Err(SnapshotErrorCode::TooLarge)
    );
    assert_eq!(
        check(body.replacen("\"go\"", "\"gp\"", 1).as_bytes())
            .expect("an issue list")
            .iter()
            .map(|issue| issue.code)
            .collect::<Vec<_>>(),
        [SnapshotErrorCode::IntegrityMismatch]
    );
}

#[test]
fn pointers_hold_only_fixed_keys_and_indices() {
    let mut bad = capture();
    bad.sessions[0].external.push(event("bad name"));
    let error = encode(&bad, &Options::default()).expect_err("invalid event name");
    assert_eq!(error.code, SnapshotErrorCode::InvalidName);
    assert_eq!(
        error.pointer.as_deref(),
        Some("/snapshot/root/queues/external/1/name")
    );
    assert!(!error.to_string().contains("bad name"));
}

#[test]
fn bounds_hold_at_the_maximum_and_fail_one_past_it() {
    let at_max = |sends: usize, variables: usize| {
        let mut capture = capture();
        capture.sessions.truncate(1);
        capture.sessions[0].machine.invocations.clear();
        capture.sessions[0].pending_sends = (0..sends)
            .map(|index| pending(&format!("sendid_{index}"), 350))
            .collect();
        if let Some(model) = &mut capture.sessions[0].data_model {
            model.variables = (0..variables)
                .map(|index| Variable {
                    name: format!("v{index:04}"),
                    value: VariableValue::Value(Value::Boolean(true)),
                })
                .collect();
        }
        encode(&capture, &Options::default()).map(|bytes| decode(&bytes).map(drop))
    };
    assert_eq!(at_max(0, 0), Ok(Ok(())));
    assert_eq!(at_max(MAX_PENDING_SENDS, MAX_VARIABLES), Ok(Ok(())));
    assert_eq!(
        at_max(MAX_PENDING_SENDS + 1, 0).map_err(|error| error.code),
        Err(SnapshotErrorCode::BoundsExceeded)
    );
    assert_eq!(
        at_max(0, MAX_VARIABLES + 1).map_err(|error| error.code),
        Err(SnapshotErrorCode::BoundsExceeded)
    );
}

#[test]
fn diff_reports_fifo_moves_keyed_changes_and_session_lifecycle() {
    let before_bytes = encoded();
    let mut later = capture();
    later.now_ms = 350;
    let root = &mut later.sessions[0];
    root.machine.atomic = vec!["s1".into()];
    root.machine.configuration = vec!["s1".into()];
    root.machine.send_counter = 2;
    root.external = vec![event("stop")];
    root.pending_sends = vec![pending("sendid_1", 400)];
    root.machine.invocations.clear();
    root.machine.transitions.remove(0);
    if let Some(model) = &mut root.data_model {
        model.variables[1].value = VariableValue::Value(Value::Number("2".into()));
        model.variables.remove(0);
    }
    later.sessions.truncate(1);
    let after_bytes = encode(&later, &Options::default()).expect("encodes");

    let bytes = diff(&before_bytes, &after_bytes, DiffOptions::default()).expect("diffs");
    let body = text(&bytes);
    assert!(body.starts_with(&format!(
        r#"{{"version":"{SESSION_SNAPSHOT_DIFF_VERSION}","#
    )));
    assert!(body.contains(r#""identical":false,"compatible":true,"clock":{"from_ms":250,"to_ms":350},"skipped":[],"sessions":[{"path":["root"],"change":"changed","configuration":{"exited":["s0","s0.a"],"entered":["s1"]}"#), "{body}");
    assert!(body.contains(r#""data_model":[{"name":"x","change":"changed","from":{"type":"number","value":"1.5"},"to":{"type":"number","value":"2"}},{"name":"f","change":"removed","from":{"type":"text","typeof":"function","text":"#));
    assert!(body.contains(r#""counters":{"send":{"from":1,"to":2}}"#));
    assert!(body.contains(r#""external":{"consumed":1,"appended":[{"name":"stop""#));
    assert!(body.contains(r#""pending_sends":{"added":[{"sendid":"sendid_1""#));
    assert!(body.contains(r#""removed":["sendid_0"]}"#));
    assert!(body.contains(r#""invocations":{"added":[],"removed":["i1"]}"#));
    assert!(body.contains(r#""transitions":{"added":[],"removed":[{"source":"s0","index":1}]}"#));
    assert!(body.ends_with(r#"{"path":["root","i1"],"change":"removed"}]}"#));

    let before = decode(&before_bytes).expect("decodes");
    let after = decode(&after_bytes).expect("decodes");
    let document = diff_documents(&before, &after, DiffOptions::default());
    assert_eq!(serde_json::to_vec(&document).expect("encodes"), bytes);
    assert!(matches!(
        document.sessions[0]
            .queues
            .as_ref()
            .and_then(|queues| queues.external.as_ref()),
        Some(FifoDiff { consumed: 1, .. })
    ));
    assert_eq!(document.sessions[1].change, Change::Removed);

    let same = diff_documents(&before, &before, DiffOptions::default());
    assert!(same.identical && same.sessions.is_empty());
    let human = render_diff(&document, "a.json", "b.json", Color::Never);
    assert!(human.starts_with("snapshot diff: a.json → b.json\n"));
    assert!(human.contains("--> status: unchanged"));
    assert!(human.contains("= data model:\n       ~ x: 1.5 -> 2\n       - f (was function"));
    assert!(human.contains("- session root/i1"));
    assert!(render_diff(&document, "a", "b", Color::Always).contains("\u{1b}[32m+\u{1b}[0m"));
}

#[test]
fn carriers_are_ignored_by_diff_unless_requested() {
    let stripped = encoded();
    let preserved =
        encode(&capture(), &options(&[], TraceContextPolicy::Preserve)).expect("encodes");
    let ignoring = diff_documents(
        &decode(&stripped).expect("decodes"),
        &decode(&preserved).expect("decodes"),
        DiffOptions::default(),
    );
    assert!(ignoring.identical && ignoring.sessions.is_empty());
    let including = diff_documents(
        &decode(&stripped).expect("decodes"),
        &decode(&preserved).expect("decodes"),
        DiffOptions {
            include_trace_context: true,
        },
    );
    assert!(!including.identical);
    assert!(!including.sessions.is_empty());
}

#[test]
fn show_renders_every_section() {
    let document = decode(&encoded()).expect("decodes");
    let human = render_show(&document, Color::Never);
    for expected in [
        "session root `root1` name `main` datamodel ecmascript ir v5 aaaaaaaaaaaa",
        "  --> status: running",
        "   = configuration: atomic [s0.a] full [s0, s0.a]",
        "       h -> [s0.a]",
        "       x = 1.5",
        "       f = function function () { return 1; }",
        "       sendid_0 `timeout` (external) due +100ms (deadline 350ms, delay 100ms)",
        "       s0[1] on go error.* -> [s1] (external) cond `x > 1` domain main",
        "           exit [s0.a, s0] entry [s1]",
        "       i1 http://www.w3.org/TR/scxml/ at s0[0] autoforward",
        "session i1 `child1` name `worker`",
        "   = invoked by: `root1` as i1",
        "       external[0] `ping` (external)",
    ] {
        assert!(human.contains(expected), "missing {expected:?} in\n{human}");
    }
}

/// Deterministic: a pinned seed and case count, and no failure-persistence
/// files written into the tree.
fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 64,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x05a9_5a09),
        ..ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn arbitrary_text_values_round_trip(text in ".{0,64}", flip in 0usize..4096) {
        let mut capture = capture();
        if let Some(model) = &mut capture.sessions[0].data_model {
            model.variables[1].value = VariableValue::Value(Value::Text(text));
        }
        let bytes = encode(&capture, &Options::default()).expect("encodes");
        let document = decode(&bytes).expect("decodes");
        prop_assert_eq!(serde_json::to_vec(&document).expect("re-encodes"), bytes.clone());
        // Any single flipped byte is rejected without panicking.
        let mut mutated = bytes.clone();
        let index = flip % mutated.len();
        mutated[index] ^= 0x01;
        prop_assert!(decode(&mutated).is_err());
        // Only "no panic" is asserted: a flipped byte may or may not leave
        // parseable JSON, so either result of `check` is acceptable.
        let _ = check(&mutated);
    }
}

/// A capture whose sessions each invoke the next: `depth` levels below the
/// root.
fn chain(depth: usize) -> Capture {
    let mut capture = capture();
    capture.sessions.truncate(2);
    for level in 2..=depth {
        let parent_id = capture.sessions[level - 1].session_id.clone();
        let invoke_id = format!("i{level}");
        if let Some(parent) = capture.sessions.get_mut(level - 1) {
            let mut invocation = capture_invocation(&invoke_id);
            invocation.state = "c".into();
            parent.machine.invocations = vec![invocation];
        }
        let mut child = child_capture();
        child.parent = Some(u32::try_from(level - 1).expect("small"));
        child.session_id = format!("child{level}");
        if let Some(model) = &mut child.data_model {
            model.system.sessionid.clone_from(&child.session_id);
        }
        child.machine.invoked_by = Some(InvokedBy {
            parent_session_id: parent_id,
            invoke_id,
        });
        capture.sessions.push(child);
    }
    capture
}

/// A live invocation record of one invoke id.
fn capture_invocation(invoke_id: &str) -> InvocationCapture {
    InvocationCapture {
        invoke_id: invoke_id.into(),
        state: "s0".into(),
        index: 0,
        type_uri: "http://www.w3.org/TR/scxml/".into(),
        autoforward: false,
    }
}

/// SCXML 5.10: "The SCXML processor MUST bind the variable _name at load
/// time to the value of the 'name' attribute of the <scxml> element." A
/// document without that attribute has no `_name` value, so the snapshot
/// omits `name` in the session and in `system` instead of inventing one;
/// the schema does not require it.
#[test]
fn a_session_without_a_name_omits_it_and_round_trips() {
    let mut nameless = capture();
    nameless.sessions[0].machine.name = None;
    if let Some(model) = nameless.sessions[0].data_model.as_mut() {
        model.system.name = None;
    }
    let bytes = encode(&nameless, &Options::default()).expect("a nameless session encodes");
    let body = text(&bytes);
    assert!(
        body.contains(r#""root":{"session_id":"root1","datamodel_name":"#),
        "no `name` after the root session id: {body}"
    );
    assert!(
        body.contains(r#""system":{"sessionid":"root1","ioprocessors":"#),
        "no `_name` in system: {body}"
    );
    let document = decode(&bytes).expect("a nameless session decodes");
    assert_eq!(document.snapshot.root.name, None);
    assert_eq!(check(&bytes).expect("checks"), vec![]);
    assert_eq!(serde_json::to_vec(&document).expect("re-encodes"), bytes);
    let schema: serde_json::Value =
        serde_json::from_str(scxml_snapshot::SESSION_SNAPSHOT_SCHEMA).expect("schema JSON");
    for def in ["session", "system"] {
        let required = schema["$defs"][def]["required"]
            .as_array()
            .expect("required list");
        assert!(
            !required.iter().any(|key| key == "name"),
            "{def} must not require `name`"
        );
    }
}

#[test]
fn transitions_carry_their_event_schema_and_its_digest_is_checked() {
    use sha2::Digest as _;
    use std::fmt::Write as _;
    let schema = r#"{"type":"object"}"#;
    let sha256 =
        sha2::Sha256::digest(schema.as_bytes())
            .iter()
            .fold(String::new(), |mut text, byte| {
                write!(text, "{byte:02x}").expect("writing to a String cannot fail");
                text
            });
    let mut described = capture();
    described.sessions[0].machine.transitions[0].event_schema = Some(EventSchema::Schema {
        schema: schema.into(),
        sha256: sha256.clone(),
    });
    described.sessions[0].machine.transitions[1].event_schema = Some(EventSchema::Schemaexpr {
        expr: "schemas.tick".into(),
    });
    let bytes = encode(&described, &options(&[], TraceContextPolicy::Strip)).expect("encodes");
    let document = decode(&bytes).expect("decodes");
    let transitions = document.snapshot.root.transitions.clone().expect("listed");
    assert_eq!(
        transitions[0].event_schema,
        described.sessions[0].machine.transitions[0].event_schema
    );
    let body = text(&bytes);
    assert!(body.contains(&format!(
        r#""event_schema":{{"kind":"schema","schema":"{{\"type\":\"object\"}}","sha256":"{sha256}"}}"#
    )));
    assert!(body.contains(r#""event_schema":{"kind":"schemaexpr","expr":"schemas.tick"}"#));
    let human = render_show(&document, Color::Never);
    assert!(human.contains("schemaexpr `schemas.tick`"), "{human}");
    assert!(
        human.contains(&format!("schema sha256:{}", &sha256[..12])),
        "{human}"
    );
    // A digest that does not match its schema is inconsistent.
    let mut wrong = described;
    wrong.sessions[0].machine.transitions[0].event_schema = Some(EventSchema::Schema {
        schema: schema.into(),
        sha256: digest('0'),
    });
    let error = encode(&wrong, &options(&[], TraceContextPolicy::Strip))
        .expect_err("the digest is checked");
    assert_eq!(error.code, SnapshotErrorCode::Inconsistent);
    // A transition without one keeps the bytes it had.
    assert!(!text(&encoded()).contains("event_schema"));
}
