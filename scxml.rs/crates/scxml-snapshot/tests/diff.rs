//! The diff reports every field a document carries: whenever two documents
//! are not `identical`, at least one change is reported, and the reverse.

use std::fmt::Write as _;

use proptest::prelude::*;
use scxml_snapshot::{
    Capture, Change, Color, ComponentIdentity, ComponentRole, DataModelCapture, DiffDocument,
    DiffOptions, Document, Event, EventType, HistoryValue, InvocationCapture, InvokedBy,
    MachineCapture, Options, PendingSend, Section, SessionCapture, SessionList, Status, System,
    TraceContext, TraceContextPolicy, TransitionCapture, TransitionType, Value, Variable,
    VariableValue, decode, diff, diff_documents, encode, render_diff,
};
use sha2::Digest as _;

fn digest(fill: char) -> String {
    std::iter::repeat_n(fill, 64).collect()
}

fn event(name: &str, carrier: bool) -> Event {
    Event {
        name: name.into(),
        event_type: EventType::External,
        origin: None,
        origintype: None,
        sendid: None,
        invokeid: None,
        data: Value::Absent,
        raw: None,
        trace_context: carrier.then(|| TraceContext {
            traceparent: "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".into(),
            tracestate: None,
            baggage: None,
            baggage_policy: scxml_snapshot::BaggagePolicy::Drop,
        }),
    }
}

/// Every knob a generated document varies.
#[derive(Clone, Debug)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag is one independent generator knob"
)]
struct Knobs {
    now_ms: u64,
    exclude: u8,
    preserve: bool,
    machine_digest: bool,
    root_status: u8,
    root_name: bool,
    ir: bool,
    configuration: bool,
    history: u8,
    bound: u8,
    send_counter: u64,
    x: u8,
    swap_variables: bool,
    external: usize,
    carrier: bool,
    pending: usize,
    cond: bool,
    exit: bool,
    second_transition: bool,
    child: bool,
    child_id: bool,
    child_model: bool,
    child_finished: bool,
    autoforward: bool,
    invoke_state: bool,
}

const SECTIONS: [Section; 7] = [
    Section::System,
    Section::DataModel,
    Section::Queues,
    Section::PendingSends,
    Section::Transitions,
    Section::Previews,
    Section::Invocations,
];

fn knobs() -> impl Strategy<Value = Knobs> {
    (
        (
            0_u64..3,
            // Sparse exclusions: most documents exclude nothing.
            prop_oneof![3 => Just(0_u8), 1 => any::<u8>()],
            any::<bool>(),
            prop::bool::weighted(0.2),
            0_u8..3,
            prop::bool::weighted(0.2),
            prop::bool::weighted(0.2),
            any::<bool>(),
            0_u8..3,
            0_u8..4,
            0_u64..2,
            (0_u8..2, prop::bool::weighted(0.2)),
        ),
        (
            0_usize..3,
            any::<bool>(),
            0_usize..3,
            any::<bool>(),
            any::<bool>(),
            prop::bool::weighted(0.8),
            prop::bool::weighted(0.8),
            prop::bool::weighted(0.2),
            prop::bool::weighted(0.2),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
        ),
    )
        .prop_map(
            |(
                (
                    now_ms,
                    exclude,
                    preserve,
                    machine_digest,
                    root_status,
                    root_name,
                    ir,
                    configuration,
                    history,
                    bound,
                    send_counter,
                    (x, swap_variables),
                ),
                (
                    external,
                    carrier,
                    pending,
                    cond,
                    exit,
                    second_transition,
                    child,
                    child_id,
                    child_model,
                    child_finished,
                    autoforward,
                    invoke_state,
                ),
            )| Knobs {
                now_ms,
                exclude,
                preserve,
                machine_digest,
                root_status,
                root_name,
                ir,
                configuration,
                history,
                bound,
                send_counter,
                x,
                swap_variables,
                external,
                carrier,
                pending,
                cond,
                exit,
                second_transition,
                child,
                child_id,
                child_model,
                child_finished,
                autoforward,
                invoke_state,
            },
        )
}

#[allow(
    clippy::too_many_lines,
    reason = "one capture literal driven by every knob"
)]
fn capture(knobs: &Knobs) -> (Capture, Options) {
    let root_id = "root1";
    let root_name = if knobs.root_name { "renamed" } else { "main" };
    let status = match knobs.root_status {
        0 => Status::Ready,
        1 => Status::Running,
        _ => Status::Finished,
    };
    // A ready session has no configuration yet.
    let (atomic, full) = if status == Status::Ready {
        (vec![], vec![])
    } else if knobs.configuration {
        (vec!["s1".to_owned()], vec!["s1".to_owned()])
    } else {
        (
            vec!["s0.a".to_owned()],
            vec!["s0".to_owned(), "s0.a".to_owned()],
        )
    };
    let history = match knobs.history {
        0 => vec![],
        1 => vec![HistoryValue {
            state: "h".into(),
            value: vec!["s0.a".into()],
        }],
        _ => vec![HistoryValue {
            state: "h".into(),
            value: vec!["s0.b".into()],
        }],
    };
    let bound_states = match knobs.bound {
        0 => vec![],
        1 => vec!["s0".to_owned()],
        2 => vec!["s0".to_owned(), "t".to_owned()],
        _ => vec!["t".to_owned(), "s0".to_owned()],
    };
    let mut variables = vec![
        Variable {
            name: "f".into(),
            value: VariableValue::Value(Value::Boolean(true)),
        },
        Variable {
            name: "x".into(),
            value: VariableValue::Value(Value::Number(knobs.x.to_string())),
        },
    ];
    if knobs.swap_variables {
        variables.swap(0, 1);
    }
    let mut transitions = vec![TransitionCapture {
        source: "s0".into(),
        index: 0,
        events: vec!["go".into()],
        targets: vec!["s1".into()],
        transition_type: TransitionType::External,
        cond: knobs.cond.then(|| "x > 1".into()),
        domain: Some("main".into()),
        exit: if knobs.exit {
            vec!["s0.a".into(), "s0".into()]
        } else {
            vec!["s0".into()]
        },
        entry: vec!["s1".into()],
        event_schema: None,
    }];
    if knobs.second_transition {
        transitions.push(TransitionCapture {
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
        });
    }
    let pending_sends: Vec<PendingSend> = (0..knobs.pending)
        .map(|index| PendingSend {
            sendid: format!("sendid_{index}"),
            event: Some(event("timeout", knobs.carrier)),
            message: None,
            target: None,
            send_type: None,
            delay_ms: 100,
            deadline_ms: 300 + index as u64,
        })
        .collect();
    let model = |sessionid: &str, name: &str, variables: Vec<Variable>| DataModelCapture {
        system: System {
            sessionid: sessionid.into(),
            name: Some(name.into()),
            ioprocessors: Value::JsonObject("{}".into()),
        },
        variables,
    };
    let root = SessionCapture {
        parent: None,
        session_id: root_id.into(),
        ir_sha256: digest(if knobs.ir { 'b' } else { 'a' }),
        data_model_name: Some("ecmascript".into()),
        machine: MachineCapture {
            status,
            name: Some(root_name.into()),
            atomic,
            configuration: full,
            internal: vec![],
            history,
            bound_states,
            invocations: if knobs.child {
                vec![InvocationCapture {
                    invoke_id: "i1".into(),
                    state: if knobs.invoke_state { "s1" } else { "s0" }.into(),
                    index: 0,
                    type_uri: "http://www.w3.org/TR/scxml/".into(),
                    autoforward: knobs.autoforward,
                }]
            } else {
                vec![]
            },
            invoked_by: None,
            send_counter: knobs.send_counter,
            invoke_counter: 0,
            transitions,
        },
        data_model: (status != Status::Ready).then(|| model(root_id, root_name, variables)),
        pending_sends,
        external: (0..knobs.external)
            .map(|_| event("go", knobs.carrier))
            .collect(),
        finished: false,
    };
    let mut sessions = vec![root];
    // Excluding `invocations` (bit 6) leaves the invoked sessions uncaptured.
    if knobs.child && knobs.exclude & (1 << 6) == 0 {
        let child_id = if knobs.child_id { "child2" } else { "child1" };
        sessions.push(SessionCapture {
            parent: Some(0),
            session_id: child_id.into(),
            ir_sha256: digest('c'),
            data_model_name: knobs.child_model.then(|| "ecmascript".into()),
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
                    parent_session_id: root_id.into(),
                    invoke_id: "i1".into(),
                }),
                send_counter: 0,
                invoke_counter: 0,
                transitions: vec![],
            },
            data_model: knobs.child_model.then(|| model(child_id, "worker", vec![])),
            pending_sends: vec![],
            external: vec![],
            finished: knobs.child_finished,
        });
    }
    let components = [
        (ComponentRole::Clock, '2'),
        (ComponentRole::DataModelEcmascript, '4'),
        (ComponentRole::IoProcessorScxml, '3'),
        (
            ComponentRole::Machine,
            if knobs.machine_digest { '5' } else { '1' },
        ),
    ]
    .into_iter()
    .map(|(role, fill)| ComponentIdentity {
        role,
        sha256: digest(fill),
    })
    .collect();
    let exclude = SECTIONS
        .iter()
        .enumerate()
        .filter(|(bit, _)| knobs.exclude & (1 << bit) != 0)
        .map(|(_, section)| *section)
        .collect();
    let options = Options {
        exclude,
        trace_context: if knobs.preserve {
            TraceContextPolicy::Preserve
        } else {
            TraceContextPolicy::Strip
        },
        ..Options::default()
    };
    (
        Capture {
            components,
            now_ms: knobs.now_ms,
            sessions,
        },
        options,
    )
}

fn document(knobs: &Knobs) -> Document {
    let (capture, options) = capture(knobs);
    let bytes = encode(&capture, &options).expect("a generated capture encodes");
    decode(&bytes).expect("a generated document decodes")
}

fn base_knobs() -> Knobs {
    Knobs {
        now_ms: 0,
        exclude: 0,
        preserve: false,
        machine_digest: false,
        root_status: 1,
        root_name: false,
        ir: false,
        configuration: false,
        history: 1,
        bound: 1,
        send_counter: 0,
        x: 1,
        swap_variables: false,
        external: 1,
        carrier: false,
        pending: 1,
        cond: true,
        exit: true,
        second_transition: true,
        child: true,
        child_id: false,
        child_model: true,
        child_finished: false,
        autoforward: false,
        invoke_state: false,
    }
}

/// The canonical bytes of an edited document, with its digest recomputed.
fn rehash(document: &mut Document) -> Document {
    let snapshot = serde_json::to_vec(&document.snapshot).expect("serializes");
    let mut hex = String::with_capacity(64);
    for byte in sha2::Sha256::digest(&snapshot) {
        write!(hex, "{byte:02x}").expect("writing to a string cannot fail");
    }
    document.sha256 = hex;
    decode(&serde_json::to_vec(document).expect("serializes")).expect("the edit stays valid")
}

fn edited(edit: impl Fn(&mut Knobs)) -> (Document, Document) {
    let before = base_knobs();
    let mut after = before.clone();
    edit(&mut after);
    (document(&before), document(&after))
}

/// Diffs, checks the invariant, and returns the root session's diff.
fn changed(before: &Document, after: &Document) -> DiffDocument {
    let diff = diff_documents(before, after, DiffOptions::default());
    assert!(!diff.identical, "the documents differ");
    assert!(diff.has_changes(), "a non-identical diff reports a change");
    diff
}

fn root(diff: &DiffDocument) -> &scxml_snapshot::SessionDiff {
    diff.sessions
        .iter()
        .find(|session| session.path == ["root"])
        .expect("the root changed")
}

fn child(diff: &DiffDocument) -> &scxml_snapshot::SessionDiff {
    diff.sessions
        .iter()
        .find(|session| session.path == ["root", "i1"])
        .expect("the child changed")
}

fn human(diff: &DiffDocument) -> String {
    render_diff(diff, "a", "b", Color::Never)
}

#[test]
fn bound_states_alone_are_reported() {
    // A late-binding chart whose only change is a newly
    // bound state.
    let (before, after) = edited(|knobs| knobs.bound = 2);
    let diff = changed(&before, &after);
    let bound = root(&diff).bound_states.as_ref().expect("bound_states");
    assert_eq!(bound.added, ["t"]);
    assert!(bound.removed.is_empty());
    let text = human(&diff);
    assert!(text.contains("= bound states:\n       + t\n"), "{text}");
    assert!(!text.contains("sessions: unchanged"), "{text}");
    let bytes = diff_bytes(&before, &after);
    assert!(
        bytes.contains(r#""bound_states":{"added":["t"],"removed":[]}"#),
        "{bytes}"
    );
}

fn diff_bytes(before: &Document, after: &Document) -> String {
    let bytes = diff(
        &serde_json::to_vec(before).expect("serializes"),
        &serde_json::to_vec(after).expect("serializes"),
        DiffOptions::default(),
    )
    .expect("diffs");
    String::from_utf8(bytes).expect("UTF-8")
}

#[test]
fn a_reordered_list_is_reported_as_reordered() {
    // [s0, t] -> [t, s0]: the same bound states in another order.
    let bound = |bound| {
        document(&Knobs {
            bound,
            ..base_knobs()
        })
    };
    let diff = changed(&bound(2), &bound(3));
    assert_eq!(
        root(&diff).reordered.as_deref(),
        Some(&[SessionList::BoundStates][..])
    );
    assert!(human(&diff).contains("= reordered: bound states"));
    let (before, after) = edited(|knobs| knobs.swap_variables = true);
    let diff = changed(&before, &after);
    assert_eq!(
        root(&diff).reordered.as_deref(),
        Some(&[SessionList::DataModel][..])
    );
}

#[test]
fn session_id_and_invoked_by_are_reported() {
    let (before, after) = edited(|knobs| knobs.child_id = true);
    let diff = changed(&before, &after);
    let id = child(&diff).session_id.as_ref().expect("session_id");
    assert_eq!((id.from.as_str(), id.to.as_str()), ("child1", "child2"));
    assert!(human(&diff).contains("= session id: `child1` -> `child2`"));

    // `invoked_by` follows the parent's session id; the edit is typed.
    let mut after = before.clone();
    after.snapshot.sessions[1].invoked_by = Some(InvokedBy {
        parent_session_id: "root2".into(),
        invoke_id: "i1".into(),
    });
    after.snapshot.sessions[0].session_id = "root2".into();
    if let Some(system) = &mut after.snapshot.sessions[0].system {
        system.sessionid = "root2".into();
    }
    let after = rehash(&mut after);
    let diff = changed(&before, &after);
    let by = child(&diff).invoked_by.as_ref().expect("invoked_by");
    assert_eq!(
        by.to.as_ref().map(|by| by.parent_session_id.as_str()),
        Some("root2")
    );
    assert!(root(&diff).session_id.is_some());
    assert!(human(&diff).contains("= invoked by: `root1` as i1 -> `root2` as i1"));
}

#[test]
fn name_and_datamodel_name_are_reported() {
    let (before, after) = edited(|knobs| knobs.root_name = true);
    let diff = changed(&before, &after);
    let name = root(&diff).name.as_ref().expect("name");
    assert_eq!(
        (name.from.as_deref(), name.to.as_deref()),
        (Some("main"), Some("renamed"))
    );
    assert!(human(&diff).contains("= name: `main` -> `renamed`"));

    let (before, after) = edited(|knobs| knobs.child_model = false);
    let diff = changed(&before, &after);
    let model = child(&diff)
        .datamodel_name
        .as_ref()
        .expect("datamodel_name");
    assert_eq!(model.from.as_deref(), Some("ecmascript"));
    assert_eq!(model.to, None);
    assert!(human(&diff).contains("= datamodel name: ecmascript -> none"));
    assert!(diff_bytes(&before, &after).contains(r#""datamodel_name":{"from":"ecmascript"}"#));
}

#[test]
fn ir_changes_are_reported_and_clear_compatible() {
    let (before, after) = edited(|knobs| knobs.ir = true);
    let diff = changed(&before, &after);
    assert!(!diff.compatible);
    let ir = root(&diff).ir.as_ref().expect("ir");
    assert_eq!(ir.to.sha256, digest('b'));
    assert!(human(&diff).contains("= ir: v"));
}

#[test]
fn atomic_configuration_is_reported() {
    // An atomic change implied by exited/entered is not repeated.
    let (before, after) = edited(|knobs| knobs.configuration = true);
    let diff = changed(&before, &after);
    let configuration = root(&diff).configuration.as_ref().expect("configuration");
    assert_eq!(configuration.exited, ["s0", "s0.a"]);
    assert_eq!(configuration.entered, ["s1"]);
    assert!(configuration.atomic.is_none());

    // A state that stays active but becomes atomic (as across an IR
    // change) is reported under `atomic` alone.
    let mut after = before.clone();
    after.snapshot.sessions[0].configuration.atomic = vec!["s0".into(), "s0.a".into()];
    let after = rehash(&mut after);
    let diff = changed(&before, &after);
    let configuration = root(&diff).configuration.as_ref().expect("configuration");
    assert!(configuration.exited.is_empty() && configuration.entered.is_empty());
    let atomic = configuration.atomic.as_ref().expect("atomic");
    assert!(atomic.exited.is_empty());
    assert_eq!(atomic.entered, ["s0"]);
    assert!(human(&diff).contains("+ atomic s0"));
}

#[test]
fn invocation_metadata_is_reported() {
    let (before, after) = edited(|knobs| {
        knobs.autoforward = true;
        knobs.invoke_state = true;
    });
    let diff = changed(&before, &after);
    let invocations = root(&diff).invocations.as_ref().expect("invocations");
    let change = &invocations.changed.as_ref().expect("changed")[0];
    assert_eq!(change.invoke_id, "i1");
    assert!(!change.from.autoforward && change.to.autoforward);
    assert_eq!(
        (change.from.state.as_str(), change.to.state.as_str()),
        ("s0", "s1")
    );
    assert!(human(&diff).contains(
        "~ i1: http://www.w3.org/TR/scxml/ at s0[0] -> http://www.w3.org/TR/scxml/ at s1[0] autoforward"
    ));

    let (before, after) = edited(|knobs| knobs.child_finished = true);
    let diff = changed(&before, &after);
    let invocations = root(&diff).invocations.as_ref().expect("invocations");
    assert!(
        invocations.changed.as_ref().expect("changed")[0]
            .to
            .finished
    );
}

#[test]
fn transition_cond_and_previews_are_reported() {
    let (before, after) = edited(|knobs| knobs.cond = false);
    let diff = changed(&before, &after);
    let transitions = root(&diff).transitions.as_ref().expect("transitions");
    let change = &transitions.changed.as_ref().expect("changed")[0];
    assert_eq!((change.source.as_str(), change.index), ("s0", 0));
    assert_eq!(change.from.cond.as_deref(), Some("x > 1"));
    assert_eq!(change.to.cond, None);
    assert!(transitions.added.is_empty() && transitions.removed.is_empty());
    let text = human(&diff);
    assert!(
        text.contains(
            "~ s0[0]\n           - s0[0] on go -> [s1] (external) cond `x > 1` domain main"
        ),
        "{text}"
    );

    let (before, after) = edited(|knobs| knobs.exit = false);
    let diff = changed(&before, &after);
    let change = &root(&diff)
        .transitions
        .as_ref()
        .expect("transitions")
        .changed
        .as_ref()
        .expect("changed")[0];
    assert_eq!(
        change.from.exit.as_deref(),
        Some(&["s0.a".to_owned(), "s0".to_owned()][..])
    );
    assert_eq!(change.to.exit.as_deref(), Some(&["s0".to_owned()][..]));
    assert!(human(&diff).contains("exit [s0] entry [s1]"));

    // With previews excluded on both sides, the same capture change is
    // invisible and the documents are identical.
    let excluded = |exit| {
        document(&Knobs {
            exclude: 1 << 5,
            exit,
            ..base_knobs()
        })
    };
    let diff = diff_documents(&excluded(true), &excluded(false), DiffOptions::default());
    assert!(diff.identical && !diff.has_changes());
}

#[test]
fn components_and_exclusions_are_reported() {
    let (before, after) = edited(|knobs| knobs.machine_digest = true);
    let diff = changed(&before, &after);
    assert!(diff.sessions.is_empty());
    let components = diff.components.as_ref().expect("components");
    assert_eq!(components.len(), 1);
    assert_eq!(components[0].role, ComponentRole::Machine);
    assert_eq!(components[0].change, Change::Changed);
    let text = human(&diff);
    assert!(
        text.contains("= components:\n     ~ machine: 111111111111 -> 555555555555\n"),
        "{text}"
    );

    let (before, after) = edited(|knobs| knobs.exclude = 0b11);
    let diff = changed(&before, &after);
    let exclude = diff.exclude.as_ref().expect("exclude");
    assert!(exclude.from.is_empty());
    assert_eq!(exclude.to, [Section::System, Section::DataModel]);
    assert!(human(&diff).contains("= exclude: [] -> [system, data_model]"));
}

#[test]
fn the_schema_names_every_new_key() {
    let schema: serde_json::Value =
        serde_json::from_str(scxml_snapshot::SESSION_SNAPSHOT_DIFF_SCHEMA).expect("JSON");
    for key in ["exclude", "components"] {
        assert!(schema["properties"][key].is_object(), "{key}");
    }
    let session = &schema["$defs"]["sessionDiff"]["properties"];
    for key in [
        "session_id",
        "name",
        "datamodel_name",
        "ir",
        "invoked_by",
        "bound_states",
        "reordered",
    ] {
        assert!(session[key].is_object(), "{key}");
    }
    assert!(session["configuration"]["properties"]["atomic"].is_object());
    assert!(session["invocations"]["properties"]["changed"].is_object());
    assert!(session["transitions"]["properties"]["changed"].is_object());
    assert!(session["transitions"]["properties"]["cancel"].is_null());
}

/// Deterministic: a pinned seed and case count, and no failure-persistence
/// files written into the tree.
fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 512,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x0d1f_f009),
        ..ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(config())]

    /// For any two valid documents, `identical` is false exactly when a
    /// change is reported, and a reported diff renders no "unchanged"
    /// contradiction.
    #[test]
    fn identical_iff_no_change_is_reported(
        before in knobs(),
        after in knobs(),
        include_trace_context in any::<bool>(),
    ) {
        let (before, after) = (document(&before), document(&after));
        let options = DiffOptions { include_trace_context };
        let diff = diff_documents(&before, &after, options);
        prop_assert_eq!(diff.identical, !diff.has_changes(), "{:?}", diff);
        let text = render_diff(&diff, "a", "b", Color::Never);
        prop_assert_eq!(diff.identical, text.contains("identical: no difference"));
        let same = diff_documents(&before, &before, options);
        prop_assert!(same.identical && !same.has_changes());
    }

    /// The same invariant on near documents: `after` takes each knob from
    /// `other` with probability 1/10, so single-field changes (the shape a
    /// skipped field hides behind) are common.
    #[test]
    fn near_documents_report_every_difference(
        before in knobs(),
        other in knobs(),
        mask in prop::collection::vec(prop::bool::weighted(0.1), 25),
        include_trace_context in any::<bool>(),
    ) {
        let after = merge(&before, &other, &mask);
        let (before, after) = (document(&before), document(&after));
        let diff = diff_documents(&before, &after, DiffOptions { include_trace_context });
        prop_assert_eq!(diff.identical, !diff.has_changes(), "{:?}", diff);
    }
}

/// `before` with knob `i` taken from `other` when `mask[i]`.
fn merge(before: &Knobs, other: &Knobs, mask: &[bool]) -> Knobs {
    let mut knobs = before.clone();
    let mut bits = mask.iter().copied();
    let mut take = || bits.next().unwrap_or(false);
    macro_rules! pick {
        ($($field:ident),* $(,)?) => {
            $(if take() { knobs.$field = other.$field.clone(); })*
        };
    }
    pick!(
        now_ms,
        exclude,
        preserve,
        machine_digest,
        root_status,
        root_name,
        ir,
        configuration,
        history,
        bound,
        send_counter,
        x,
        swap_variables,
        external,
        carrier,
        pending,
        cond,
        exit,
        second_transition,
        child,
        child_id,
        child_model,
        child_finished,
        autoforward,
        invoke_state,
    );
    knobs
}

/// SCXML 5.10: `_name` is the `name` attribute of `<scxml>`; a document
/// without one has none. A diff between a named and a nameless session
/// reports the name as gained or lost, never as an invented string.
#[test]
fn a_name_that_appears_or_disappears_is_an_optional_change() {
    let before = document(&base_knobs());
    let mut after = before.clone();
    let named = before.snapshot.sessions[0].name.clone();
    assert!(named.is_some(), "the base fixture is named");
    after.snapshot.sessions[0].name = None;
    if let Some(system) = after.snapshot.sessions[0].system.as_mut() {
        system.name = None;
    }
    let after = rehash(&mut after);
    let diff = changed(&before, &after);
    let change = root(&diff).name.as_ref().expect("the name changed");
    assert_eq!(change.from, named);
    assert_eq!(change.to, None);
    let text = human(&diff);
    assert!(text.contains("-> none"), "{text}");
    let bytes = diff_bytes(&before, &after);
    assert!(bytes.contains(r#""name":{"from":"#), "{bytes}");
    assert!(!bytes.contains(r#""to":null"#), "{bytes}");
}
