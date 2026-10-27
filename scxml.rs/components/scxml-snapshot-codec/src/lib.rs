//! Optional session snapshot codec component (WIT world
//! `stateforward:scxml/session-snapshot`).
//!
//! It exports `session-snapshot-api` (`encode` and `diff` of the canonical
//! read-only document `scxml.session-snapshot/v2`) for hosts that do not
//! link the `scxml-snapshot` crate natively, such as the browser host. It
//! is capability-free: it imports only the versioned telemetry facade (plus
//! the types-only interfaces its records come from) and reads nothing but
//! its arguments. The bytes are those of [`scxml_snapshot::encode`] and
//! [`scxml_snapshot::diff`], so both hosts produce identical documents.
//!
//! Failures are `invalid-input` (or `payload-too-large` for `too-large`)
//! with the static detail `snapshot-<code>`; no pointer, name, or payload
//! crosses the boundary. Telemetry is one `observe` per call with the
//! output size as `snapshot-bytes`, plus one `event`.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; scripts/verify-component-authored-unsafe.py rejects authored unsafe"
    )
)]

use scxml_snapshot::{SnapshotError, SnapshotErrorCode};

/// The static `component-error.detail` of a codec failure.
#[must_use]
pub const fn error_detail(code: SnapshotErrorCode) -> &'static str {
    match code {
        SnapshotErrorCode::TooLarge => "snapshot-too-large",
        SnapshotErrorCode::MalformedJson => "snapshot-malformed-json",
        SnapshotErrorCode::UnsupportedVersion => "snapshot-unsupported-version",
        SnapshotErrorCode::SchemaViolation => "snapshot-schema-violation",
        SnapshotErrorCode::NonCanonical => "snapshot-non-canonical",
        SnapshotErrorCode::IntegrityMismatch => "snapshot-integrity-mismatch",
        SnapshotErrorCode::BoundsExceeded => "snapshot-bounds-exceeded",
        SnapshotErrorCode::InvalidName => "snapshot-invalid-name",
        SnapshotErrorCode::Inconsistent => "snapshot-inconsistent",
        SnapshotErrorCode::DepthExceeded => "snapshot-depth-exceeded",
        SnapshotErrorCode::ClockInvariant => "snapshot-clock-invariant",
        SnapshotErrorCode::NotStable => "snapshot-not-stable",
        SnapshotErrorCode::StatusInvalid => "snapshot-status-invalid",
        SnapshotErrorCode::CodecUnavailable => "snapshot-codec-unavailable",
    }
}

/// Whether a failure is reported as `payload-too-large` (else
/// `invalid-input`).
#[must_use]
pub fn is_too_large(error: &SnapshotError) -> bool {
    error.code == SnapshotErrorCode::TooLarge
}

#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]
mod guest {
    wit_bindgen::generate!({
        world: "session-snapshot",
        path: "../../crates/scxml-wit/wit/scxml",
    });

    use exports::stateforward::scxml::session_snapshot_api as api;
    use stateforward::scxml::data_model_api as dm;
    use stateforward::scxml::io_processor_api as io;
    use stateforward::scxml::machine_capability_types as machine;
    use stateforward::scxml::telemetry::{
        self, Measurement, MeasurementKind, Operation, Outcome, SignalEvent,
    };
    use stateforward::scxml::types as wit;

    use scxml_snapshot as snap;

    struct Component;

    impl api::Guest for Component {
        fn encode(
            capture: api::Capture,
            options: api::SnapshotOptions,
        ) -> Result<Vec<u8>, wit::ComponentError> {
            let options = options_from(&options);
            let encoded =
                capture_from(capture).and_then(|capture| snap::encode(&capture, &options));
            finish(Operation::Encode, encoded)
        }

        fn diff(
            before: Vec<u8>,
            after: Vec<u8>,
            include_trace_context: bool,
        ) -> Result<Vec<u8>, wit::ComponentError> {
            let options = snap::DiffOptions {
                include_trace_context,
            };
            finish(Operation::Diff, snap::diff(&before, &after, options))
        }
    }

    /// Records the boundary observation and maps the result.
    fn finish(
        operation: Operation,
        result: Result<Vec<u8>, snap::SnapshotError>,
    ) -> Result<Vec<u8>, wit::ComponentError> {
        match result {
            Ok(bytes) => {
                let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                telemetry::observe(
                    operation,
                    Outcome::Ok,
                    Some(Measurement {
                        kind: MeasurementKind::SnapshotBytes,
                        value: size,
                    }),
                    None,
                );
                telemetry::event(operation, SignalEvent::ArtifactProduced, None);
                Ok(bytes)
            }
            Err(error) => {
                telemetry::observe(operation, Outcome::Rejected, None, None);
                telemetry::event(operation, SignalEvent::InputRejected, None);
                Err(wit::ComponentError {
                    version: wit::ErrorVersion::V1,
                    code: if super::is_too_large(&error) {
                        wit::ErrorCode::PayloadTooLarge
                    } else {
                        wit::ErrorCode::InvalidInput
                    },
                    detail: Some(super::error_detail(error.code).into()),
                })
            }
        }
    }

    fn options_from(options: &api::SnapshotOptions) -> snap::Options {
        snap::Options {
            exclude: options
                .exclude
                .iter()
                .map(|section| match section {
                    api::Section::System => snap::Section::System,
                    api::Section::DataModel => snap::Section::DataModel,
                    api::Section::Queues => snap::Section::Queues,
                    api::Section::PendingSends => snap::Section::PendingSends,
                    api::Section::Transitions => snap::Section::Transitions,
                    api::Section::Previews => snap::Section::Previews,
                    api::Section::Invocations => snap::Section::Invocations,
                })
                .collect(),
            trace_context: match options.trace_context {
                api::TraceContextPolicy::Preserve => snap::TraceContextPolicy::Preserve,
                api::TraceContextPolicy::Strip => snap::TraceContextPolicy::Strip,
            },
            point: match options.point {
                api::SnapshotPoint::Checkpoint => snap::SnapshotPoint::Checkpoint,
                api::SnapshotPoint::ExecutableContent => snap::SnapshotPoint::ExecutableContent,
            },
        }
    }

    fn value_from(value: wit::DataValue) -> snap::Value {
        match value {
            wit::DataValue::Absent => snap::Value::Absent,
            wit::DataValue::Undefined => snap::Value::Undefined,
            wit::DataValue::Null => snap::Value::Null,
            wit::DataValue::Boolean(value) => snap::Value::Boolean(value),
            wit::DataValue::Number(value) => snap::Value::number(value),
            wit::DataValue::Text(text) => snap::Value::Text(text),
            wit::DataValue::JsonArray(text) => snap::Value::JsonArray(text),
            wit::DataValue::JsonObject(text) => snap::Value::JsonObject(text),
            wit::DataValue::Xml(text) => snap::Value::Xml(text),
        }
    }

    fn event_from(event: wit::Event) -> snap::Event {
        snap::Event {
            name: event.name,
            event_type: match event.event_type {
                wit::EventType::Internal => snap::EventType::Internal,
                wit::EventType::External => snap::EventType::External,
                wit::EventType::Platform => snap::EventType::Platform,
            },
            origin: event.origin,
            origintype: event.origin_type,
            sendid: event.send_id,
            invokeid: event.invoke_id,
            data: value_from(event.data),
            raw: event.raw,
            trace_context: event.trace_context.map(carrier_from),
        }
    }

    fn carrier_from(carrier: wit::TraceContext) -> snap::TraceContext {
        snap::TraceContext {
            traceparent: carrier.traceparent,
            tracestate: carrier.tracestate,
            baggage: carrier.baggage,
            baggage_policy: match carrier.baggage_policy {
                wit::BaggagePolicy::Drop => snap::BaggagePolicy::Drop,
                wit::BaggagePolicy::Propagate => snap::BaggagePolicy::Propagate,
            },
        }
    }

    fn events_from(events: Vec<wit::Event>) -> Vec<snap::Event> {
        events.into_iter().map(event_from).collect()
    }

    /// A pending send without a send id cannot be cancelled and breaks the
    /// io-processor contract: the capture is `inconsistent`, the same code
    /// the Wasmtime host reports for the same record.
    fn pending_from(pending: io::PendingSend) -> Result<snap::PendingSend, snap::SnapshotError> {
        match pending.request {
            io::DelayedSend::Scxml(request) => {
                let Some(sendid) = request.send_id else {
                    return Err(snap::SnapshotError::new(
                        snap::SnapshotErrorCode::Inconsistent,
                        None,
                    ));
                };
                Ok(snap::PendingSend {
                    sendid,
                    event: Some(event_from(request.event)),
                    message: None,
                    target: request.target,
                    send_type: request.processor_type,
                    delay_ms: request.delay_ms,
                    deadline_ms: pending.deadline_ms,
                })
            }
            io::DelayedSend::Processor(send) => Ok(snap::PendingSend {
                sendid: send.send_id,
                event: None,
                message: Some(snap::ProcessorMessage {
                    event: send.event_name,
                    params: send
                        .params
                        .into_iter()
                        .map(|field| snap::Param {
                            name: field.name,
                            value: value_from(field.value),
                        })
                        .collect(),
                    content: send.content.map(value_from),
                    trace_context: send.trace_context.map(carrier_from),
                }),
                target: send.target,
                send_type: Some(send.processor_type),
                delay_ms: send.delay_ms,
                deadline_ms: pending.deadline_ms,
            }),
        }
    }

    /// `failed` and `shut-down` are never snapshotted: `status-invalid`.
    fn status_from(status: machine::Status) -> Result<snap::Status, snap::SnapshotError> {
        match status {
            machine::Status::Ready => Ok(snap::Status::Ready),
            machine::Status::Running => Ok(snap::Status::Running),
            machine::Status::Finished => Ok(snap::Status::Finished),
            machine::Status::Failed | machine::Status::ShutDown => Err(snap::SnapshotError::new(
                snap::SnapshotErrorCode::StatusInvalid,
                None,
            )),
        }
    }

    fn machine_from(
        snapshot: machine::MachineSnapshot,
    ) -> Result<snap::MachineCapture, snap::SnapshotError> {
        Ok(snap::MachineCapture {
            status: status_from(snapshot.status)?,
            name: snapshot.name,
            atomic: snapshot.atomic,
            configuration: snapshot.configuration,
            internal: events_from(snapshot.internal),
            history: snapshot
                .history
                .into_iter()
                .map(|value| snap::HistoryValue {
                    state: value.state,
                    value: value.value,
                })
                .collect(),
            bound_states: snapshot.bound_states,
            invocations: snapshot
                .invocations
                .into_iter()
                .map(|invocation| snap::InvocationCapture {
                    invoke_id: invocation.invoke_id,
                    state: invocation.state,
                    index: invocation.index,
                    type_uri: invocation.type_uri,
                    autoforward: invocation.autoforward,
                })
                .collect(),
            invoked_by: snapshot.invoked_by.map(|by| snap::InvokedBy {
                parent_session_id: by.parent_session_id,
                invoke_id: by.invoke_id,
            }),
            send_counter: snapshot.send_counter,
            invoke_counter: snapshot.invoke_counter,
            transitions: snapshot
                .transitions
                .into_iter()
                .map(|transition| snap::TransitionCapture {
                    source: transition.source,
                    index: transition.index,
                    events: transition.events,
                    targets: transition.targets,
                    transition_type: match transition.type_ {
                        machine::TransitionType::External => snap::TransitionType::External,
                        machine::TransitionType::Internal => snap::TransitionType::Internal,
                    },
                    cond: transition.cond,
                    domain: transition.domain,
                    exit: transition.exit,
                    entry: transition.entry,
                    event_schema: transition.event_schema.map(|schema| match schema {
                        machine::EventSchema::Schema(text) => snap::EventSchema::Schema {
                            schema: text.schema,
                            sha256: text.sha256,
                        },
                        machine::EventSchema::Schemaexpr(expr) => {
                            snap::EventSchema::Schemaexpr { expr }
                        }
                    }),
                })
                .collect(),
        })
    }

    fn data_model_from(snapshot: dm::DataModelSnapshot) -> snap::DataModelCapture {
        snap::DataModelCapture {
            system: snap::System {
                sessionid: snapshot.system.session_id,
                name: snapshot.system.machine_name,
                ioprocessors: value_from(snapshot.system.io_processors),
            },
            variables: snapshot
                .variables
                .into_iter()
                .map(|variable| snap::Variable {
                    name: variable.name,
                    value: match variable.value {
                        dm::VariableValue::Value(value) => {
                            snap::VariableValue::Value(value_from(value))
                        }
                        dm::VariableValue::Text(text) => {
                            snap::VariableValue::Text(snap::TextValue::new(text.kind, text.text))
                        }
                        dm::VariableValue::Oversized(bytes) => {
                            snap::VariableValue::Oversized(snap::OversizedValue::new(bytes))
                        }
                    },
                })
                .collect(),
        }
    }

    /// Converts one session record; the machine status is checked before
    /// the pending sends, as the Wasmtime host does.
    fn session_from(
        session: api::SessionCapture,
    ) -> Result<snap::SessionCapture, snap::SnapshotError> {
        Ok(snap::SessionCapture {
            parent: session.parent,
            session_id: session.session_id,
            ir_sha256: session.ir_sha256,
            data_model_name: session.data_model_name,
            machine: machine_from(session.machine)?,
            data_model: session.data_model.map(data_model_from),
            pending_sends: session
                .pending_sends
                .into_iter()
                .map(pending_from)
                .collect::<Result<_, _>>()?,
            external: events_from(session.external),
            finished: session.finished,
        })
    }

    /// Converts the capture records; the first broken session record fails
    /// the whole capture with the code the Wasmtime host reports for it.
    fn capture_from(capture: api::Capture) -> Result<snap::Capture, snap::SnapshotError> {
        let sessions = capture
            .sessions
            .into_iter()
            .map(session_from)
            .collect::<Result<_, _>>()?;
        Ok(snap::Capture {
            components: capture
                .components
                .into_iter()
                .map(|component| snap::ComponentIdentity {
                    role: match component.role {
                        api::ComponentRole::Clock => snap::ComponentRole::Clock,
                        api::ComponentRole::DataModelEcmascript => {
                            snap::ComponentRole::DataModelEcmascript
                        }
                        api::ComponentRole::DataModelNull => snap::ComponentRole::DataModelNull,
                        api::ComponentRole::DataModelXpath => snap::ComponentRole::DataModelXpath,
                        api::ComponentRole::IoProcessorScxml => {
                            snap::ComponentRole::IoProcessorScxml
                        }
                        api::ComponentRole::Machine => snap::ComponentRole::Machine,
                    },
                    sha256: component.sha256,
                })
                .collect(),
            now_ms: capture.now_ms,
            sessions,
        })
    }

    export!(Component);
}
