//! Example extension namespace component.
//!
//! The component exercises the whole `executable-content-extension` contract
//! with one element per host capability, so hosts can prove extension component
//! linking end to end:
//!
//! * `<example:raise event="E" in="S"/>` raises `E` (default
//!   `example.custom`) through `execution-host.raise`; with `in`, only while
//!   `execution-host.in-state(S)` holds;
//! * `<example:send event="E" target="T"/>` sends `E` through
//!   `execution-host.send` (no `target`: the session's own external queue);
//! * `<example:fail/>` fails with `execution` (SCXML `error.execution`);
//! * `<example:spin/>` never returns: it runs until the host's fuel bound
//!   traps it;
//! * `<example:snapshot location="L" exclude="S..."/>` assigns the
//!   session snapshot document (`session-snapshot-host.snapshot`, the
//!   space-separated sections `S` excluded) to `L` as a string; with
//!   `at="initialize"` it assigns instead the error code label the same
//!   call answered during `initialize` (or `ok`);
//!
//! Every other local name is `not-handled`. The decision is the portable
//! [`ExampleNamespace::execute`], tested natively; the guest executes the
//! [`ExampleAction`] it returns against the WIT imports.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; scripts/verify-component-authored-unsafe.py rejects authored unsafe"
    )
)]

#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]
mod guest {
    wit_bindgen::generate!({
        world: "executable-content-extension",
        path: "../../crates/scxml-wit/wit/scxml",
    });

    use std::cell::Cell;

    use exports::stateforward::scxml::executable_content_extension_api::Guest;
    use stateforward::scxml::session_snapshot_host::{self, Section};
    use stateforward::scxml::telemetry::{self, Operation, Outcome, SignalEvent};
    use stateforward::scxml::types::{
        BaggagePolicy, ComponentError, DataValue, ErrorCode, ErrorVersion, Event, EventType,
        ExecutableContentExtension, ExtensionOutcome, SendMessage, SendRequest, TraceContext,
    };
    use stateforward::scxml::{data_model_host, execution_host};

    use super::{ExampleAction, ExampleNamespace};

    struct Component;

    thread_local! {
        /// What `session-snapshot-host.snapshot` answered in `initialize`.
        static AT_INITIALIZE: Cell<Option<ErrorCode>> = const { Cell::new(None) };
    }

    fn code_label(code: ErrorCode) -> &'static str {
        match code {
            ErrorCode::InvalidInput => "invalid-input",
            ErrorCode::Execution => "execution",
            ErrorCode::DuplicateId => "duplicate-id",
            ErrorCode::UnknownId => "unknown-id",
            ErrorCode::CapacityExhausted => "capacity-exhausted",
            ErrorCode::PayloadTooLarge => "payload-too-large",
            ErrorCode::Unavailable => "unavailable",
            ErrorCode::UnsupportedVersion => "unsupported-version",
            ErrorCode::Protocol => "protocol",
            ErrorCode::Platform => "platform",
        }
    }

    impl Guest for Component {
        fn namespace_uri() -> String {
            super::EXAMPLE_NAMESPACE_URI.into()
        }

        fn initialize() -> Result<(), ComponentError> {
            observed(Operation::Initialize, None, || {
                // The snapshot works only during `execute`; the answer
                // here is kept for `<example:snapshot at="initialize">`.
                AT_INITIALIZE.set(session_snapshot_host::snapshot(&[]).err().map(|e| e.code));
                Ok(())
            })
        }

        #[allow(
            clippy::too_many_lines,
            reason = "one dispatch over every example element"
        )]
        fn execute(
            element: ExecutableContentExtension,
            context: Option<TraceContext>,
        ) -> Result<ExtensionOutcome, ComponentError> {
            let context = validate_context(context.as_ref());
            observed_context(Operation::Execute, context, |context| {
                if element.name.namespace_uri.as_deref() != Some(super::EXAMPLE_NAMESPACE_URI) {
                    return Err(error(
                        ErrorCode::Protocol,
                        "element namespace does not match extension component identity",
                    ));
                }
                let native = scxml_types::ExecutableContentExtension {
                    name: scxml_types::QualifiedName {
                        namespace_uri: None,
                        local_name: element.name.local_name,
                    },
                    attributes: element
                        .attributes
                        .into_iter()
                        .map(|attribute| scxml_types::ExtensionAttribute {
                            name: scxml_types::QualifiedName {
                                namespace_uri: attribute
                                    .name
                                    .namespace_uri
                                    .and_then(|uri| scxml_types::NamespaceUri::new(uri).ok()),
                                local_name: attribute.name.local_name,
                            },
                            value: attribute.value,
                        })
                        .collect(),
                    content: Vec::new(),
                };
                let action = ExampleNamespace.execute(&native).map_err(|_| {
                    error(
                        ErrorCode::InvalidInput,
                        "example element attributes are invalid",
                    )
                })?;
                match action {
                    None => Ok(ExtensionOutcome::NotHandled),
                    Some(ExampleAction::Raise { event, when_in }) => {
                        if when_in.is_none_or(|state| execution_host::in_state(&state)) {
                            execution_host::raise(&event_named(
                                event.as_str(),
                                EventType::Internal,
                                context,
                            ))?;
                        }
                        Ok(ExtensionOutcome::Handled)
                    }
                    Some(ExampleAction::Send {
                        event,
                        target,
                        processor_type,
                        delay_ms,
                        send_id,
                        event_send_id,
                    }) => {
                        let mut event = event_named(event.as_str(), EventType::External, context);
                        event.send_id = event_send_id;
                        execution_host::send(&SendRequest {
                            send_id,
                            event,
                            target,
                            processor_type,
                            delay_ms,
                            message: SendMessage::Empty,
                        })?;
                        Ok(ExtensionOutcome::Handled)
                    }
                    Some(ExampleAction::Fail) => {
                        Err(error(ErrorCode::Execution, "example:fail always fails"))
                    }
                    Some(ExampleAction::Spin) => spin(),
                    Some(ExampleAction::Snapshot {
                        location,
                        exclude,
                        at_initialize,
                    }) => {
                        let text = if at_initialize {
                            AT_INITIALIZE.get().map_or("ok", code_label).to_owned()
                        } else {
                            let sections: Vec<Section> = exclude
                                .iter()
                                .map(|name| match name.as_str() {
                                    "system" => Ok(Section::System),
                                    "data_model" => Ok(Section::DataModel),
                                    "queues" => Ok(Section::Queues),
                                    "pending_sends" => Ok(Section::PendingSends),
                                    "transitions" => Ok(Section::Transitions),
                                    _ => Err(error(
                                        ErrorCode::InvalidInput,
                                        "exclude names an unknown section",
                                    )),
                                })
                                .collect::<Result<_, _>>()?;
                            session_snapshot_host::snapshot(&sections)?
                        };
                        data_model_host::assign(&location, &DataValue::Text(text))?;
                        Ok(ExtensionOutcome::Handled)
                    }
                }
            })
        }

        fn shutdown() -> Result<(), ComponentError> {
            observed(Operation::Shutdown, None, || Ok(()))
        }
    }

    /// Runs until the host's fuel bound traps the call.
    fn spin() -> ! {
        let mut counter: u64 = 0;
        loop {
            counter = std::hint::black_box(counter.wrapping_add(1));
        }
    }

    fn event_named(name: &str, event_type: EventType, context: Option<&TraceContext>) -> Event {
        Event {
            name: name.into(),
            event_type,
            send_id: None,
            origin: None,
            origin_type: None,
            invoke_id: None,
            data: DataValue::Absent,
            raw: None,
            trace_context: context.cloned(),
        }
    }

    fn observed<T>(
        operation: Operation,
        context: Option<&TraceContext>,
        body: impl FnOnce() -> Result<T, ComponentError>,
    ) -> Result<T, ComponentError> {
        let result = body();
        let outcome = match &result {
            Ok(_) => Outcome::Ok,
            Err(error) if is_rejected(error.code) => Outcome::Rejected,
            Err(_) => Outcome::Failed,
        };
        telemetry::observe(operation, outcome, None, context);
        telemetry::event(
            operation,
            if result.is_ok() {
                SignalEvent::TransformationCompleted
            } else {
                SignalEvent::InputRejected
            },
            context,
        );
        result
    }

    fn observed_context<T>(
        operation: Operation,
        context: Result<Option<TraceContext>, ComponentError>,
        body: impl FnOnce(Option<&TraceContext>) -> Result<T, ComponentError>,
    ) -> Result<T, ComponentError> {
        match context {
            Ok(context) => observed(operation, context.as_ref(), || body(context.as_ref())),
            Err(error) => observed(operation, None, || Err(error)),
        }
    }

    fn validate_context(
        context: Option<&TraceContext>,
    ) -> Result<Option<TraceContext>, ComponentError> {
        context
            .map(|context| {
                scxml_types::TraceContext::new(
                    context.traceparent.clone(),
                    context.tracestate.clone(),
                    context.baggage.clone(),
                    match context.baggage_policy {
                        BaggagePolicy::Drop => scxml_types::BaggagePolicy::Drop,
                        BaggagePolicy::Propagate => scxml_types::BaggagePolicy::Propagate,
                    },
                )
                .map_err(|_| error(ErrorCode::InvalidInput, "trace context is invalid"))
                .map(|validated| TraceContext {
                    traceparent: validated.traceparent,
                    tracestate: validated.tracestate,
                    baggage: validated.baggage,
                    baggage_policy: match validated.baggage_policy {
                        scxml_types::BaggagePolicy::Drop => BaggagePolicy::Drop,
                        scxml_types::BaggagePolicy::Propagate => BaggagePolicy::Propagate,
                    },
                })
            })
            .transpose()
    }

    fn error(code: ErrorCode, detail: &'static str) -> ComponentError {
        ComponentError {
            version: ErrorVersion::V1,
            code,
            detail: (detail.len() <= 160).then(|| detail.into()),
        }
    }

    fn is_rejected(code: ErrorCode) -> bool {
        matches!(
            code,
            ErrorCode::InvalidInput
                | ErrorCode::DuplicateId
                | ErrorCode::UnknownId
                | ErrorCode::PayloadTooLarge
                | ErrorCode::UnsupportedVersion
                | ErrorCode::Protocol
        )
    }

    export!(Component);
}

use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};
use scxml_types::{EventName, ExecutableContentExtension};

/// Exact URI exported by the example namespace component.
pub const EXAMPLE_NAMESPACE_URI: &str = "https://stateforward.org/scxml/example";

/// What one handled example element asks the host capabilities to do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExampleAction {
    /// Raise an internal event, only while `when_in` is active when set.
    Raise {
        /// The event.
        event: EventName,
        /// The state that must be active (`in` attribute).
        when_in: Option<String>,
    },
    /// Send an external event to `target` (the session itself when absent)
    /// through the I/O processor `processor_type` names (the SCXML Event I/O
    /// Processor when absent).
    Send {
        /// The event.
        event: EventName,
        /// The `target` attribute.
        target: Option<String>,
        /// The `type` attribute.
        processor_type: Option<String>,
        /// The `delay` attribute, in whole milliseconds (0 when absent or
        /// not a number).
        delay_ms: u64,
        /// The `id` attribute: the request's send id.
        send_id: Option<String>,
        /// The `eventid` attribute: the event's own send id.
        event_send_id: Option<String>,
    },
    /// Fail with `execution`.
    Fail,
    /// Run until the host's fuel bound traps the call.
    Spin,
    /// Assign the session snapshot document to `location`.
    Snapshot {
        /// The `location` attribute.
        location: String,
        /// The `exclude` attribute's space-separated section names.
        exclude: Vec<String>,
        /// `at="initialize"`: assign what the snapshot answered in
        /// `initialize` instead.
        at_initialize: bool,
    },
}

/// Example extension component; it keeps no state between `execute` calls.
#[derive(Default)]
pub struct ExampleNamespace;

impl ExampleNamespace {
    /// Returns the component's exact canonical namespace URI.
    #[must_use]
    pub const fn namespace_uri(&self) -> &'static str {
        EXAMPLE_NAMESPACE_URI
    }

    /// Decides what one example element does; `None` when its local name is
    /// not an example element (`not-handled`).
    ///
    /// # Errors
    ///
    /// Returns a validation message when the `event` attribute is not a valid
    /// SCXML event name.
    pub fn execute(
        &self,
        element: &ExecutableContentExtension,
    ) -> Result<Option<ExampleAction>, String> {
        let span = observe(
            Module::ExtensionExampleComponent,
            Operation::ExtensionExampleExecute,
        );
        let attribute = |name: &str| {
            element
                .attributes
                .iter()
                .find(|attribute| {
                    attribute.name.namespace_uri.is_none() && attribute.name.local_name == name
                })
                .map(|attribute| attribute.value.clone())
        };
        let event = || {
            EventName::new(attribute("event").unwrap_or_else(|| "example.custom".into()))
                .map_err(|error| error.to_string())
        };
        let result = match element.name.local_name.as_str() {
            "raise" => event().map(|event| {
                Some(ExampleAction::Raise {
                    event,
                    when_in: attribute("in"),
                })
            }),
            "send" => event().map(|event| {
                Some(ExampleAction::Send {
                    event,
                    target: attribute("target"),
                    processor_type: attribute("type"),
                    delay_ms: attribute("delay")
                        .and_then(|delay| delay.parse().ok())
                        .unwrap_or(0),
                    send_id: attribute("id"),
                    event_send_id: attribute("eventid"),
                })
            }),
            "fail" => Ok(Some(ExampleAction::Fail)),
            "spin" => Ok(Some(ExampleAction::Spin)),
            "snapshot" => attribute("location")
                .ok_or_else(|| "example:snapshot requires a location".to_owned())
                .map(|location| {
                    Some(ExampleAction::Snapshot {
                        location,
                        exclude: attribute("exclude")
                            .unwrap_or_default()
                            .split_ascii_whitespace()
                            .map(str::to_owned)
                            .collect(),
                        at_initialize: attribute("at").as_deref() == Some("initialize"),
                    })
                }),
            _ => Ok(None),
        };
        if result.is_ok() {
            span.event(SignalEvent::TransformationCompleted);
        } else {
            span.event(SignalEvent::InputRejected);
            span.fail(ErrorCategory::InvalidInput);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scxml_types::{ExtensionAttribute, NamespaceUri, QualifiedName};

    fn element(local_name: &str, attributes: &[(&str, &str)]) -> ExecutableContentExtension {
        ExecutableContentExtension {
            name: QualifiedName {
                namespace_uri: Some(
                    NamespaceUri::new(EXAMPLE_NAMESPACE_URI)
                        .expect("example namespace URI fixture must be valid"),
                ),
                local_name: local_name.into(),
            },
            attributes: attributes
                .iter()
                .map(|(name, value)| ExtensionAttribute {
                    name: QualifiedName {
                        namespace_uri: None,
                        local_name: (*name).into(),
                    },
                    value: (*value).into(),
                })
                .collect(),
            content: Vec::new(),
        }
    }

    fn name(value: &str) -> EventName {
        EventName::new(value).expect("event name fixture must be valid")
    }

    #[test]
    fn raise_and_send_carry_their_event_state_and_target() {
        let namespace = ExampleNamespace;
        assert_eq!(namespace.namespace_uri(), EXAMPLE_NAMESPACE_URI);
        assert_eq!(
            namespace.execute(&element("raise", &[("event", "example.executed")])),
            Ok(Some(ExampleAction::Raise {
                event: name("example.executed"),
                when_in: None,
            }))
        );
        assert_eq!(
            namespace.execute(&element("raise", &[("in", "s0")])),
            Ok(Some(ExampleAction::Raise {
                event: name("example.custom"),
                when_in: Some("s0".into()),
            }))
        );
        assert_eq!(
            namespace.execute(&element(
                "send",
                &[("event", "e"), ("target", "#_internal")]
            )),
            Ok(Some(ExampleAction::Send {
                event: name("e"),
                target: Some("#_internal".into()),
                processor_type: None,
                delay_ms: 0,
                send_id: None,
                event_send_id: None,
            }))
        );
        assert_eq!(
            namespace.execute(&element(
                "send",
                &[
                    ("event", "e"),
                    ("delay", "5"),
                    ("id", "s"),
                    ("eventid", "t")
                ]
            )),
            Ok(Some(ExampleAction::Send {
                event: name("e"),
                target: None,
                processor_type: None,
                delay_ms: 5,
                send_id: Some("s".into()),
                event_send_id: Some("t".into()),
            }))
        );
        assert_eq!(
            namespace.execute(&element("send", &[("event", "e"), ("type", "scxml")])),
            Ok(Some(ExampleAction::Send {
                event: name("e"),
                target: None,
                processor_type: Some("scxml".into()),
                delay_ms: 0,
                send_id: None,
                event_send_id: None,
            }))
        );
    }

    #[test]
    fn fail_spin_invalid_names_and_unknown_elements() {
        let namespace = ExampleNamespace;
        assert_eq!(
            namespace.execute(&element("fail", &[])),
            Ok(Some(ExampleAction::Fail))
        );
        assert_eq!(
            namespace.execute(&element("spin", &[])),
            Ok(Some(ExampleAction::Spin))
        );
        assert_eq!(
            namespace.execute(&element("other", &[("event", "x")])),
            Ok(None)
        );
        assert!(
            namespace
                .execute(&element("raise", &[("event", "")]))
                .is_err()
        );
    }
}
