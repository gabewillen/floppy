//! Parity proof of the two component-records-to-capture conversions (see
//! `docs/session-snapshot.md`, "Document"): the native host conversion in
//! [`super::Gather`] and the real `scxml-snapshot-codec` component must
//! produce byte-identical documents from the same records, and must fail
//! every broken record set with the identical snapshot error code.
//!
//! The codec world does not import the record interfaces (its export only
//! `use`s their types), so wasmtime generates its own copies of the
//! machine, data-model, and io-processor records; the test fills both
//! copies with the same values. `types` is substituted, so events, data
//! values, send requests, and trace carriers are literally the host's
//! generated records. `telemetry` is substituted too, and this test links a
//! quiet host implementation of it.

use super::{Gather, RunFailure, SnapshotErrorCode, SnapshotFailure, data_model};
use crate::bindings::exports::stateforward::scxml::machine_api as host_machine;
use crate::bindings::stateforward::scxml::types as wit;
use crate::codec_bindings as codec;
use crate::data_model::{
    DataModelSnapshot as HostDataModelSnapshot, SystemValues as HostSystemValues,
    Variable as HostVariable, VariableText as HostVariableText, VariableValue as HostVariableValue,
};
use crate::io_bindings::exports::stateforward::scxml::io_processor_api as host_io;
use scxml_snapshot::{Options, TraceContextPolicy};

/// Telemetry the codec observes; the proof only needs it to succeed.
struct CodecTelemetry;

// The type-only interfaces generate host traits only for the world-level
// linker, which the proof does not use: the component imports no function
// of them, so the telemetry interface linker above is the whole contract.
impl crate::bindings::stateforward::scxml::types::Host for CodecTelemetry {}

impl crate::bindings::stateforward::scxml::telemetry::Host for CodecTelemetry {
    fn start(
        &mut self,
        _operation: crate::bindings::stateforward::scxml::telemetry::Operation,
        _parent: Option<wit::TraceContext>,
    ) -> (u64, Option<wit::TraceContext>) {
        (0, None)
    }

    fn end(
        &mut self,
        _span: u64,
        _outcome: crate::bindings::stateforward::scxml::telemetry::Outcome,
    ) {
    }

    fn observe(
        &mut self,
        _operation: crate::bindings::stateforward::scxml::telemetry::Operation,
        _outcome: crate::bindings::stateforward::scxml::telemetry::Outcome,
        _measurement: Option<crate::bindings::stateforward::scxml::telemetry::Measurement>,
        _context: Option<wit::TraceContext>,
    ) {
    }

    fn event(
        &mut self,
        _operation: crate::bindings::stateforward::scxml::telemetry::Operation,
        _event: crate::bindings::stateforward::scxml::telemetry::SignalEvent,
        _context: Option<wit::TraceContext>,
    ) {
    }
}

/// The virtualized codec artifact, built once per test process from the
/// current source into this proof's own artifacts directory (the capability
/// script requires one directory per concurrent run; other suites rewrite
/// the shared release directory with their own component sets).
fn codec_bytes() -> Vec<u8> {
    use std::path::Path;
    use std::process::Command;
    use std::sync::OnceLock;

    static CODEC: OnceLock<Vec<u8>> = OnceLock::new();
    CODEC
        .get_or_init(|| {
            let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
            let workspace = manifest
                .parent()
                .and_then(Path::parent)
                .expect("host crate has a workspace parent");
            let artifacts = crate::prebuilt::release_dir().unwrap_or_else(|| {
                let target_root = workspace.join("target/session-host-proof");
                let artifacts = target_root.join("wasm-artifacts/codec-parity");
                let status = Command::new("sh")
                    .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
                    .arg("--artifacts-dir")
                    .arg(&artifacts)
                    .arg("scxml_snapshot_codec")
                    .current_dir(workspace)
                    .env("CARGO_TARGET_DIR", &target_root)
                    .status()
                    .expect("the capability script runs");
                assert!(status.success(), "the codec artifact build must pass");
                artifacts
            });
            std::fs::read(artifacts.join("scxml_snapshot_codec.virtualized.wasm"))
                .expect("the codec artifact is readable")
        })
        .clone()
}

/// One instantiated codec component with its `session-snapshot-api`.
struct Codec {
    store: wasmtime::Store<CodecTelemetry>,
    instance: codec::SessionSnapshot,
}

impl Codec {
    fn new() -> Self {
        let engine = wasmtime::Engine::new(wasmtime::Config::new().wasm_component_model(true))
            .expect("the engine builds");
        let component = wasmtime::component::Component::from_binary(&engine, &codec_bytes())
            .expect("the codec artifact is a valid component");
        let mut linker = wasmtime::component::Linker::<CodecTelemetry>::new(&engine);
        crate::bindings::stateforward::scxml::telemetry::add_to_linker::<
            _,
            wasmtime::component::HasSelf<CodecTelemetry>,
        >(&mut linker, |state| state)
        .expect("telemetry links");
        // The export's `use`d interfaces appear as type-only import
        // instances: they carry no functions and no resources, so they need
        // no linker definition.
        let mut store = wasmtime::Store::new(&engine, CodecTelemetry);
        let instance = codec::SessionSnapshot::instantiate(&mut store, &component, &linker)
            .expect("the codec instantiates");
        Self { store, instance }
    }

    fn encode(
        &mut self,
        capture: &codec::exports::stateforward::scxml::session_snapshot_api::Capture,
        options: &Options,
    ) -> Result<Vec<u8>, SnapshotErrorCode> {
        let codec_options =
            codec::exports::stateforward::scxml::session_snapshot_api::SnapshotOptions {
                exclude: options
                    .exclude
                    .iter()
                    .map(|section| match section {
                        scxml_snapshot::Section::System => {
                            codec::exports::stateforward::scxml::session_snapshot_api::Section::System
                        }
                        scxml_snapshot::Section::DataModel => {
                            codec::exports::stateforward::scxml::session_snapshot_api::Section::DataModel
                        }
                        scxml_snapshot::Section::Queues => {
                            codec::exports::stateforward::scxml::session_snapshot_api::Section::Queues
                        }
                        scxml_snapshot::Section::PendingSends => {
                            codec::exports::stateforward::scxml::session_snapshot_api::Section::PendingSends
                        }
                        scxml_snapshot::Section::Transitions => {
                            codec::exports::stateforward::scxml::session_snapshot_api::Section::Transitions
                        }
                        scxml_snapshot::Section::Previews => {
                            codec::exports::stateforward::scxml::session_snapshot_api::Section::Previews
                        }
                        scxml_snapshot::Section::Invocations => {
                            codec::exports::stateforward::scxml::session_snapshot_api::Section::Invocations
                        }
                    })
                    .collect(),
                trace_context: match options.trace_context {
                    TraceContextPolicy::Preserve => codec::exports::stateforward::scxml::session_snapshot_api::TraceContextPolicy::Preserve,
                    TraceContextPolicy::Strip => codec::exports::stateforward::scxml::session_snapshot_api::TraceContextPolicy::Strip,
                },
                point: match options.point {
                    scxml_snapshot::SnapshotPoint::Checkpoint => codec::exports::stateforward::scxml::session_snapshot_api::SnapshotPoint::Checkpoint,
                    scxml_snapshot::SnapshotPoint::ExecutableContent => codec::exports::stateforward::scxml::session_snapshot_api::SnapshotPoint::ExecutableContent,
                },
            };
        let api = self.instance.stateforward_scxml_session_snapshot_api();
        api.call_encode(&mut self.store, capture, &codec_options)
            .expect("the codec call traps on nothing")
            .map_err(|error| codec_code(&error))
    }
}

/// Every snapshot error code, for decoding the codec's static detail.
const ALL_CODES: [SnapshotErrorCode; 14] = [
    SnapshotErrorCode::TooLarge,
    SnapshotErrorCode::MalformedJson,
    SnapshotErrorCode::UnsupportedVersion,
    SnapshotErrorCode::SchemaViolation,
    SnapshotErrorCode::NonCanonical,
    SnapshotErrorCode::IntegrityMismatch,
    SnapshotErrorCode::BoundsExceeded,
    SnapshotErrorCode::InvalidName,
    SnapshotErrorCode::Inconsistent,
    SnapshotErrorCode::DepthExceeded,
    SnapshotErrorCode::ClockInvariant,
    SnapshotErrorCode::NotStable,
    SnapshotErrorCode::StatusInvalid,
    SnapshotErrorCode::CodecUnavailable,
];

/// The exact snapshot code a codec failure carries: its static detail is
/// `snapshot-<code>`, and its WIT code is `payload-too-large` for
/// `too-large` and `invalid-input` for every other code. Anything else is
/// a broken codec contract and fails the proof.
fn codec_code(error: &wit::ComponentError) -> SnapshotErrorCode {
    let detail = error.detail.as_deref().unwrap_or_default();
    let code = detail
        .strip_prefix("snapshot-")
        .and_then(|label| ALL_CODES.into_iter().find(|code| code.as_str() == label))
        .unwrap_or_else(|| panic!("the codec failed with a snapshot detail, got {detail:?}"));
    let expected = if code == SnapshotErrorCode::TooLarge {
        wit::ErrorCode::PayloadTooLarge
    } else {
        wit::ErrorCode::InvalidInput
    };
    assert_eq!(
        error.code, expected,
        "the codec's WIT code for {detail} is {expected:?}"
    );
    code
}

/// The records both paths convert: one representative session.
///
/// Numbers use renderings the display rule must get exactly right (`-0`,
/// `0.1 + 0.2`, `1e21`); events carry carriers (kept under `preserve`,
/// dropped under `strip`); the data model has function and oversized
/// variables; the machine has history, an invocation, and a transition of
/// each kind with cond, domain, and previews.
struct Records {
    /// `types` records, shared by both paths.
    events: Vec<wit::Event>,
    /// The host's own data-model mirror (what [`data_model`] converts).
    host_data_model: HostDataModelSnapshot,
    /// The codec's generated data-model record with the same values.
    codec_data_model: codec::stateforward::scxml::data_model_api::DataModelSnapshot,
    /// The host's generated machine record.
    host_machine: host_machine::MachineSnapshot,
    /// The codec's generated machine record with the same values.
    codec_machine: codec::stateforward::scxml::machine_api::MachineSnapshot,
    /// The child's machine record: `ready`, invoked by the root's `i1`.
    host_child_machine: host_machine::MachineSnapshot,
    /// The codec's child machine record with the same values.
    codec_child_machine: codec::stateforward::scxml::machine_api::MachineSnapshot,
    /// The root's pending delayed sends as the host's generated records.
    host_pending: Vec<host_io::PendingSend>,
    /// The codec's pending-send records with the same values.
    codec_pending: Vec<codec::stateforward::scxml::io_processor_api::PendingSend>,
}

fn sample_trace() -> wit::TraceContext {
    wit::TraceContext {
        traceparent: "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".into(),
        tracestate: Some("rojo=1".into()),
        baggage: Some("user=agentmlx".into()),
        baggage_policy: wit::BaggagePolicy::Propagate,
    }
}

fn sample_event(
    name: &str,
    event_type: wit::EventType,
    send_id: Option<String>,
    data: wit::DataValue,
    raw: Option<String>,
    carrier: Option<wit::TraceContext>,
) -> wit::Event {
    wit::Event {
        name: name.into(),
        event_type,
        origin: Some("http://example.invalid".into()),
        origin_type: Some("scxml".into()),
        send_id,
        invoke_id: None,
        data,
        raw,
        trace_context: carrier,
    }
}

impl Records {
    #[allow(
        clippy::too_many_lines,
        reason = "one literal block of representative records, split only for a lint"
    )]
    fn sample() -> Self {
        let trace = sample_trace();
        let events = vec![
            sample_event(
                "internal.done",
                wit::EventType::Internal,
                None,
                wit::DataValue::Number(-0.0),
                None,
                Some(trace.clone()),
            ),
            sample_event(
                "go",
                wit::EventType::External,
                Some("sendid_1".into()),
                wit::DataValue::Number(0.1 + 0.2),
                Some("<raw/>".into()),
                Some(trace.clone()),
            ),
            sample_event(
                "platform.tick",
                wit::EventType::Platform,
                None,
                wit::DataValue::Number(1e21),
                None,
                Some(trace),
            ),
        ];
        let system = wit::DataValue::JsonArray("[\"scxml\"]".into());
        let host_data_model = HostDataModelSnapshot {
            system: HostSystemValues {
                session_id: "session".into(),
                machine_name: Some("sample-machine".into()),
                io_processors: system.clone(),
            },
            variables: vec![
                HostVariable {
                    name: "x".into(),
                    value: HostVariableValue::Value(wit::DataValue::Number(2.5)),
                },
                HostVariable {
                    name: "f".into(),
                    value: HostVariableValue::Text(HostVariableText {
                        kind: "function".into(),
                        text: "function () { return 1; }".into(),
                    }),
                },
                HostVariable {
                    name: "big".into(),
                    value: HostVariableValue::Oversized(70_000),
                },
            ],
        };
        let codec_data_model = codec::stateforward::scxml::data_model_api::DataModelSnapshot {
            system: codec::stateforward::scxml::data_model_api::SystemValues {
                session_id: "session".into(),
                machine_name: Some("sample-machine".into()),
                io_processors: system,
            },
            variables: vec![
                codec::stateforward::scxml::data_model_api::Variable {
                    name: "x".into(),
                    value: codec::stateforward::scxml::data_model_api::VariableValue::Value(
                        wit::DataValue::Number(2.5),
                    ),
                },
                codec::stateforward::scxml::data_model_api::Variable {
                    name: "f".into(),
                    value: codec::stateforward::scxml::data_model_api::VariableValue::Text(
                        codec::stateforward::scxml::data_model_api::VariableText {
                            kind: "function".into(),
                            text: "function () { return 1; }".into(),
                        },
                    ),
                },
                codec::stateforward::scxml::data_model_api::Variable {
                    name: "big".into(),
                    value: codec::stateforward::scxml::data_model_api::VariableValue::Oversized(
                        70_000,
                    ),
                },
            ],
        };
        let host_machine = host_machine::MachineSnapshot {
            status: host_machine::Status::Finished,
            name: Some("sample-machine".into()),
            atomic: vec!["s1".into()],
            configuration: vec!["s1".into()],
            internal: Vec::new(),
            history: vec![host_machine::HistoryValue {
                state: "h".into(),
                value: vec!["s0.a".into()],
            }],
            bound_states: vec!["late".into()],
            invocations: vec![host_machine::InvocationSnapshot {
                invoke_id: "i1".into(),
                state: "s1".into(),
                index: 0,
                type_uri: "http://www.w3.org/TR/scxml/".into(),
                autoforward: true,
            }],
            invoked_by: None,
            send_counter: 2,
            invoke_counter: 1,
            transitions: vec![host_machine::TransitionSnapshot {
                source: "s1".into(),
                index: 0,
                events: vec!["go".into(), "error.*".into()],
                targets: vec!["s2".into()],
                type_: host_machine::TransitionType::External,
                cond: Some("x > 1".into()),
                domain: Some("s0".into()),
                exit: vec!["s1".into()],
                entry: vec!["s2".into(), "s2.init".into()],
                event_schema: None,
            }],
        };
        let codec_machine = codec::stateforward::scxml::machine_api::MachineSnapshot {
            status: codec::stateforward::scxml::machine_api::Status::Finished,
            name: Some("sample-machine".into()),
            atomic: host_machine.atomic.clone(),
            configuration: host_machine.configuration.clone(),
            internal: Vec::new(),
            history: vec![codec::stateforward::scxml::machine_api::HistoryValue {
                state: "h".into(),
                value: vec!["s0.a".into()],
            }],
            bound_states: host_machine.bound_states.clone(),
            invocations: vec![
                codec::stateforward::scxml::machine_api::InvocationSnapshot {
                    invoke_id: "i1".into(),
                    state: "s1".into(),
                    index: 0,
                    type_uri: "http://www.w3.org/TR/scxml/".into(),
                    autoforward: true,
                },
            ],
            invoked_by: None,
            send_counter: 2,
            invoke_counter: 1,
            transitions: vec![
                codec::stateforward::scxml::machine_api::TransitionSnapshot {
                    source: "s1".into(),
                    index: 0,
                    events: vec!["go".into(), "error.*".into()],
                    targets: vec!["s2".into()],
                    type_: codec::stateforward::scxml::machine_api::TransitionType::External,
                    cond: Some("x > 1".into()),
                    domain: Some("s0".into()),
                    exit: vec!["s1".into()],
                    entry: vec!["s2".into(), "s2.init".into()],
                    event_schema: None,
                },
            ],
        };
        let host_child_machine = host_machine::MachineSnapshot {
            status: host_machine::Status::Ready,
            name: Some("child-machine".into()),
            atomic: Vec::new(),
            configuration: Vec::new(),
            internal: Vec::new(),
            history: Vec::new(),
            bound_states: Vec::new(),
            invocations: Vec::new(),
            invoked_by: Some(host_machine::InvokedBy {
                parent_session_id: "session".into(),
                invoke_id: "i1".into(),
            }),
            send_counter: 0,
            invoke_counter: 0,
            transitions: Vec::new(),
        };
        let codec_child_machine = codec::stateforward::scxml::machine_api::MachineSnapshot {
            status: codec::stateforward::scxml::machine_api::Status::Ready,
            name: Some("child-machine".into()),
            atomic: host_child_machine.atomic.clone(),
            configuration: host_child_machine.configuration.clone(),
            internal: Vec::new(),
            history: Vec::new(),
            bound_states: Vec::new(),
            invocations: Vec::new(),
            invoked_by: Some(codec::stateforward::scxml::machine_api::InvokedBy {
                parent_session_id: "session".into(),
                invoke_id: "i1".into(),
            }),
            send_counter: 0,
            invoke_counter: 0,
            transitions: Vec::new(),
        };
        let request = wit::SendRequest {
            send_id: Some("tick".into()),
            event: sample_event(
                "tick",
                wit::EventType::External,
                Some("tick".into()),
                wit::DataValue::Number(1.5),
                Some("_scxmleventname=tick".into()),
                Some(sample_trace()),
            ),
            target: None,
            processor_type: Some("http://www.w3.org/TR/scxml/#SCXMLEventProcessor".into()),
            delay_ms: 100,
            message: wit::SendMessage::Empty,
        };
        let post = wit::ProcessorSend {
            processor_type: "basichttp".into(),
            send_id: "post".into(),
            event_name: Some("posted".into()),
            target: Some("http://example.test/sessions/peer/basichttp".into()),
            params: vec![wit::NamedValue {
                name: "count".into(),
                value: wit::DataValue::Number(2.0),
            }],
            content: None,
            delay_ms: 150,
            trace_context: Some(sample_trace()),
        };
        let host_pending = vec![
            host_io::PendingSend {
                request: host_io::DelayedSend::Scxml(request.clone()),
                deadline_ms: 450,
            },
            host_io::PendingSend {
                request: host_io::DelayedSend::Processor(post.clone()),
                deadline_ms: 500,
            },
        ];
        let codec_pending = vec![
            codec::stateforward::scxml::io_processor_api::PendingSend {
                request: codec::stateforward::scxml::io_processor_api::DelayedSend::Scxml(request),
                deadline_ms: 450,
            },
            codec::stateforward::scxml::io_processor_api::PendingSend {
                request: codec::stateforward::scxml::io_processor_api::DelayedSend::Processor(post),
                deadline_ms: 500,
            },
        ];
        Self {
            events,
            host_data_model,
            codec_data_model,
            host_machine,
            codec_machine,
            host_child_machine,
            codec_child_machine,
            host_pending,
            codec_pending,
        }
    }
}

/// The native capture [`Gather`] builds from `records`: the root with its
/// pending sends and external queue, then the child unless `invocations` is
/// excluded (excluding it while a child is present is `inconsistent`).
/// A conversion failure is its exact snapshot code.
fn native_capture(
    records: &Records,
    options: &Options,
) -> Result<scxml_snapshot::Capture, SnapshotErrorCode> {
    let gather = Gather { options };
    let code = |failure: RunFailure| match failure {
        RunFailure::Snapshot(SnapshotFailure::Codec(error)) => error.code,
        other => panic!("the native conversion fails only with a snapshot code, got {other:?}"),
    };
    let root = scxml_snapshot::SessionCapture {
        parent: None,
        session_id: "session".into(),
        ir_sha256: "0".repeat(64),
        data_model_name: Some("ecmascript".into()),
        machine: gather.machine(records.host_machine.clone()).map_err(code)?,
        data_model: Some(data_model(records.host_data_model.clone())),
        pending_sends: records
            .host_pending
            .iter()
            .cloned()
            .map(|pending| gather.pending_send(pending))
            .collect::<Result<_, _>>()
            .map_err(code)?,
        external: records
            .events
            .iter()
            .map(|event| gather.event(event))
            .collect(),
        finished: false,
    };
    let mut sessions = vec![root];
    if !options
        .exclude
        .contains(&scxml_snapshot::Section::Invocations)
    {
        sessions.push(scxml_snapshot::SessionCapture {
            parent: Some(0),
            session_id: "session:i1".into(),
            ir_sha256: "b".repeat(64),
            data_model_name: None,
            machine: gather
                .machine(records.host_child_machine.clone())
                .map_err(code)?,
            data_model: None,
            pending_sends: Vec::new(),
            external: Vec::new(),
            finished: false,
        });
    }
    Ok(scxml_snapshot::Capture {
        components: [
            scxml_snapshot::ComponentRole::Clock,
            scxml_snapshot::ComponentRole::DataModelEcmascript,
            scxml_snapshot::ComponentRole::IoProcessorScxml,
            scxml_snapshot::ComponentRole::Machine,
        ]
        .into_iter()
        .map(|role| scxml_snapshot::ComponentIdentity {
            role,
            sha256: "a".repeat(64),
        })
        .collect(),
        now_ms: 350,
        sessions,
    })
}

/// The native path end to end: conversion, then [`scxml_snapshot::encode`].
fn native_encode(records: &Records, options: &Options) -> Result<Vec<u8>, SnapshotErrorCode> {
    let capture = native_capture(records, options)?;
    scxml_snapshot::encode(&capture, options).map_err(|error| error.code)
}

/// The codec's `capture` record with the same values as
/// [`native_capture`], unconverted.
fn codec_capture(
    records: &Records,
    options: &Options,
) -> codec::exports::stateforward::scxml::session_snapshot_api::Capture {
    use codec::exports::stateforward::scxml::session_snapshot_api as api;
    let root = api::SessionCapture {
        parent: None,
        session_id: "session".into(),
        ir_sha256: "0".repeat(64),
        data_model_name: Some("ecmascript".into()),
        machine: records.codec_machine.clone(),
        data_model: Some(records.codec_data_model.clone()),
        pending_sends: records.codec_pending.clone(),
        external: records.events.clone(),
        finished: false,
    };
    let mut sessions = vec![root];
    if !options
        .exclude
        .contains(&scxml_snapshot::Section::Invocations)
    {
        sessions.push(api::SessionCapture {
            parent: Some(0),
            session_id: "session:i1".into(),
            ir_sha256: "b".repeat(64),
            data_model_name: None,
            machine: records.codec_child_machine.clone(),
            data_model: None,
            pending_sends: Vec::new(),
            external: Vec::new(),
            finished: false,
        });
    }
    api::Capture {
        components: [
            api::ComponentRole::Clock,
            api::ComponentRole::DataModelEcmascript,
            api::ComponentRole::IoProcessorScxml,
            api::ComponentRole::Machine,
        ]
        .into_iter()
        .map(|role| api::ComponentIdentity {
            role,
            sha256: "a".repeat(64),
        })
        .collect(),
        now_ms: 350,
        sessions,
    }
}

#[test]
fn the_codec_and_the_host_produce_identical_documents() {
    let mut codec = Codec::new();
    let records = Records::sample();
    let options_sets = [
        Options::default(),
        Options {
            exclude: Vec::new(),
            trace_context: TraceContextPolicy::Preserve,
            ..Options::default()
        },
        Options {
            exclude: vec![
                scxml_snapshot::Section::Previews,
                scxml_snapshot::Section::Invocations,
            ],
            trace_context: TraceContextPolicy::Strip,
            ..Options::default()
        },
        Options {
            exclude: vec![
                scxml_snapshot::Section::DataModel,
                scxml_snapshot::Section::System,
            ],
            trace_context: TraceContextPolicy::Preserve,
            ..Options::default()
        },
    ];
    for options in &options_sets {
        let native_bytes = native_encode(&records, options).expect("the native capture encodes");
        let codec_bytes = codec
            .encode(&codec_capture(&records, options), options)
            .expect("the codec capture encodes");
        assert_eq!(
            native_bytes, codec_bytes,
            "the two conversion paths diverge under {options:?}"
        );
    }
}

/// One broken record set: its name, how it breaks the sample records, and
/// the one code both paths must answer.
struct BrokenCase {
    name: &'static str,
    code: SnapshotErrorCode,
    options: Options,
    breaks: fn(&mut Records),
}

/// Every failure branch of the two adapters (status mapping of the root
/// and of a child, both non-snapshottable statuses, a pending send without
/// its send id, and the status checked before the pending sends), plus
/// encoder failures both paths must pass through unchanged (an invalid
/// variable name, a child whose parent link is wrong).
fn broken_cases() -> Vec<BrokenCase> {
    use codec::stateforward::scxml::machine_api as codec_machine;
    vec![
        BrokenCase {
            name: "a failed root machine",
            code: SnapshotErrorCode::StatusInvalid,
            options: Options::default(),
            breaks: |records| {
                records.host_machine.status = host_machine::Status::Failed;
                records.codec_machine.status = codec_machine::Status::Failed;
            },
        },
        BrokenCase {
            name: "a shut-down root machine",
            code: SnapshotErrorCode::StatusInvalid,
            options: Options::default(),
            breaks: |records| {
                records.host_machine.status = host_machine::Status::ShutDown;
                records.codec_machine.status = codec_machine::Status::ShutDown;
            },
        },
        BrokenCase {
            name: "a failed child machine",
            code: SnapshotErrorCode::StatusInvalid,
            options: Options::default(),
            breaks: |records| {
                records.host_child_machine.status = host_machine::Status::Failed;
                records.codec_child_machine.status = codec_machine::Status::Failed;
            },
        },
        BrokenCase {
            name: "a failed root machine with a pending send without its id",
            code: SnapshotErrorCode::StatusInvalid,
            options: Options::default(),
            breaks: |records| {
                records.host_machine.status = host_machine::Status::Failed;
                records.codec_machine.status = codec_machine::Status::Failed;
                for pending in &mut records.host_pending {
                    if let host_io::DelayedSend::Scxml(request) = &mut pending.request {
                        request.send_id = None;
                    }
                }
                for pending in &mut records.codec_pending {
                    if let codec::stateforward::scxml::io_processor_api::DelayedSend::Scxml(
                        request,
                    ) = &mut pending.request
                    {
                        request.send_id = None;
                    }
                }
            },
        },
        BrokenCase {
            name: "a pending send without its send id",
            code: SnapshotErrorCode::Inconsistent,
            options: Options::default(),
            breaks: |records| {
                for pending in &mut records.host_pending {
                    if let host_io::DelayedSend::Scxml(request) = &mut pending.request {
                        request.send_id = None;
                        request.event.send_id = None;
                    }
                }
                for pending in &mut records.codec_pending {
                    if let codec::stateforward::scxml::io_processor_api::DelayedSend::Scxml(
                        request,
                    ) = &mut pending.request
                    {
                        request.send_id = None;
                        request.event.send_id = None;
                    }
                }
            },
        },
        BrokenCase {
            name: "a variable named with a reserved `_` prefix",
            code: SnapshotErrorCode::InvalidName,
            options: Options::default(),
            breaks: |records| {
                if let Some(variable) = records.host_data_model.variables.first_mut() {
                    "_x".clone_into(&mut variable.name);
                }
                if let Some(variable) = records.codec_data_model.variables.first_mut() {
                    "_x".clone_into(&mut variable.name);
                }
            },
        },
        BrokenCase {
            name: "a child invoked by an invocation the root does not have",
            code: SnapshotErrorCode::Inconsistent,
            options: Options::default(),
            breaks: |records| {
                if let Some(parent) = records.host_child_machine.invoked_by.as_mut() {
                    "i2".clone_into(&mut parent.invoke_id);
                }
                if let Some(parent) = records.codec_child_machine.invoked_by.as_mut() {
                    "i2".clone_into(&mut parent.invoke_id);
                }
            },
        },
    ]
}

#[test]
fn both_paths_fail_every_broken_capture_with_the_same_code() {
    let mut codec = Codec::new();
    for case in broken_cases() {
        let mut records = Records::sample();
        (case.breaks)(&mut records);
        let native = native_encode(&records, &case.options).map(|_| "a document");
        let codec_code = codec
            .encode(&codec_capture(&records, &case.options), &case.options)
            .map(|_| "a document");
        assert_eq!(
            (native, codec_code),
            (Err(case.code), Err(case.code)),
            "{}: (native, codec) results",
            case.name
        );
    }
}
