//! SCXML Event I/O Processor component domain logic.
//!
//! The processor validates and translates portable send requests. Actual routing
//! remains a host capability and is not hidden in this component. Delayed-send
//! deadlines and cancellation are delegated to the linked project clock
//! component, which remains authoritative for timer decisions.

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
        world: "io-processor",
        path: "../../crates/scxml-wit/wit/scxml",
        generate_all,
    });

    use exports::stateforward::scxml::io_processor_api::DelayedSend as WitDelayedSend;
    use exports::stateforward::scxml::io_processor_api::{
        DispatchDecision, DueBatch, DueRejection as WitDueRejection, DueRoute as WitDueRoute,
        Guest, PendingSend, RejectedDue,
    };
    use stateforward::scxml::telemetry;
    use stateforward::scxml::types::{
        BaggagePolicy as WitBaggagePolicy, BasichttpSend as WitBasichttpSend, ComponentError,
        DataValue as WitDataValue, ErrorCode, ErrorVersion, Event as WitEvent,
        EventType as WitEventType, NamedValue as WitNamedValue, RoutedEvent,
        SendMessage as WitSendMessage, SendRequest as WitSendRequest, SendTarget as WitSendTarget,
        Sender, TraceContext as WitTraceContext,
    };
    use stateforward::scxml_clock::clock_api;
    use stateforward::scxml_clock::types::{
        BaggagePolicy as ClockBaggagePolicy, ClockError, TraceContext as ClockTraceContext,
    };

    struct Component;

    impl Guest for Component {
        fn type_uri() -> String {
            super::SCXML_EVENT_PROCESSOR_URI.into()
        }

        fn aliases() -> Vec<String> {
            super::SCXML_PROCESSOR_ALIASES
                .iter()
                .map(|alias| (*alias).to_owned())
                .collect()
        }

        fn session_location(session_id: String) -> String {
            super::session_origin(&session_id)
        }

        fn classify_target(target: Option<String>, sender: Sender) -> WitSendTarget {
            // Pure and total: no telemetry (WIT contract).
            let is_live_invocation = |id: &str| is_live_invocation(&sender, id);
            let invocation_of_session = |id: &str| invocation_of_session(&sender, id);
            let address = super::SessionAddress {
                session_id: &sender.session_id,
                parent_session_id: sender.parent_session_id.as_deref(),
                is_live_invocation: &is_live_invocation,
                invocation_of_session: &invocation_of_session,
            };
            match super::classify_target(target.as_deref(), &address) {
                super::SendTarget::Internal => WitSendTarget::Internal,
                super::SendTarget::Local => WitSendTarget::Local,
                super::SendTarget::Parent => WitSendTarget::Parent,
                super::SendTarget::Invocation(id) => WitSendTarget::Invocation(id),
                super::SendTarget::Unreachable => WitSendTarget::Unreachable,
                super::SendTarget::Invalid => WitSendTarget::Invalid,
            }
        }

        fn dispatch(
            mut request: WitSendRequest,
            sender: Sender,
        ) -> Result<DispatchDecision, ComponentError> {
            let mut context = None;
            let result = (|| {
                context = validate_wit_context(request.event.trace_context.take())?;
                request.event.trace_context.clone_from(&context);
                let request = to_native_request(request)?;
                let is_live_invocation = |id: &str| is_live_invocation(&sender, id);
                let invocation_of_session = |id: &str| invocation_of_session(&sender, id);
                let address = super::SessionAddress {
                    session_id: &sender.session_id,
                    parent_session_id: sender.parent_session_id.as_deref(),
                    is_live_invocation: &is_live_invocation,
                    invocation_of_session: &invocation_of_session,
                };
                // One processor decision shared with native hosts.
                match super::ScxmlIoProcessor
                    .dispatch(request, &address)
                    .map_err(processor_error)?
                {
                    super::Dispatch::Scheduled(scheduled) => {
                        let clock_context = context.as_ref().map(to_clock_context);
                        clock_api::schedule(
                            &scheduled.send_id,
                            &scheduled.payload,
                            scheduled.delay_ms,
                            clock_context.as_ref(),
                        )
                        .map_err(clock_error)?;
                        Ok(DispatchDecision::Scheduled)
                    }
                    super::Dispatch::Local(event) => {
                        Ok(DispatchDecision::Local(from_native_event(event)))
                    }
                    super::Dispatch::Parent(event) => {
                        Ok(DispatchDecision::Parent(from_native_event(event)))
                    }
                    super::Dispatch::Invocation { invoke_id, event } => {
                        Ok(DispatchDecision::Invocation(RoutedEvent {
                            invoke_id,
                            event: from_native_event(event),
                        }))
                    }
                }
            })();
            let (outcome, event) = if result.is_ok() {
                (telemetry::Outcome::Ok, telemetry::SignalEvent::SendAccepted)
            } else {
                (
                    telemetry::Outcome::Rejected,
                    telemetry::SignalEvent::SendRejected,
                )
            };
            telemetry::observe(
                telemetry::Operation::Dispatch,
                outcome,
                None,
                context.as_ref(),
            );
            telemetry::event(telemetry::Operation::Dispatch, event, context.as_ref());
            result
        }

        fn route_due(request: WitSendRequest, sender: Sender) -> WitDueRoute {
            // Pure and total: a request that cannot be decoded cannot be
            // dispatched either, exactly as a target nobody answers.
            let send_id = request.send_id.clone();
            let Ok(request) = to_native_request(request) else {
                return WitDueRoute::Undeliverable(send_id);
            };
            let is_live_invocation = |id: &str| is_live_invocation(&sender, id);
            let invocation_of_session = |id: &str| invocation_of_session(&sender, id);
            let address = super::SessionAddress {
                session_id: &sender.session_id,
                parent_session_id: sender.parent_session_id.as_deref(),
                is_live_invocation: &is_live_invocation,
                invocation_of_session: &invocation_of_session,
            };
            match super::ScxmlIoProcessor.route_due(request, &address) {
                super::DueRoute::Local(event) => WitDueRoute::Local(from_native_event(event)),
                super::DueRoute::Parent(event) => WitDueRoute::Parent(from_native_event(event)),
                super::DueRoute::Invocation { invoke_id, event } => {
                    WitDueRoute::Invocation(RoutedEvent {
                        invoke_id,
                        event: from_native_event(event),
                    })
                }
                super::DueRoute::Undeliverable { send_id } => {
                    WitDueRoute::Undeliverable(send_id.map(|id| id.as_str().to_owned()))
                }
            }
        }

        fn schedule_basichttp(mut request: WitBasichttpSend) -> Result<(), ComponentError> {
            let mut context = None;
            let result = (|| {
                context = validate_wit_context(request.trace_context.take())?;
                request.trace_context.clone_from(&context);
                let send = to_native_basichttp(request)?;
                let scheduled = super::ScxmlIoProcessor
                    .plan_basichttp(&send)
                    .map_err(processor_error)?;
                let clock_context = context.as_ref().map(to_clock_context);
                clock_api::schedule(
                    &scheduled.send_id,
                    &scheduled.payload,
                    scheduled.delay_ms,
                    clock_context.as_ref(),
                )
                .map(|_| ())
                .map_err(clock_error)
            })();
            let (outcome, event) = if result.is_ok() {
                (telemetry::Outcome::Ok, telemetry::SignalEvent::SendAccepted)
            } else {
                (
                    telemetry::Outcome::Rejected,
                    telemetry::SignalEvent::SendRejected,
                )
            };
            telemetry::observe(
                telemetry::Operation::ScheduleBasichttp,
                outcome,
                None,
                context.as_ref(),
            );
            telemetry::event(
                telemetry::Operation::ScheduleBasichttp,
                event,
                context.as_ref(),
            );
            result
        }

        fn cancel(send_id: String, context: Option<WitTraceContext>) -> Result<(), ComponentError> {
            let context = match validate_wit_context(context) {
                Ok(context) => context,
                Err(error) => {
                    telemetry::observe(
                        telemetry::Operation::Cancel,
                        telemetry::Outcome::Rejected,
                        None,
                        None,
                    );
                    telemetry::event(
                        telemetry::Operation::Cancel,
                        telemetry::SignalEvent::CancelRejected,
                        None,
                    );
                    return Err(error);
                }
            };
            let clock_context = context.as_ref().map(to_clock_context);
            let result = clock_api::cancel(&send_id, clock_context.as_ref())
                .map(|_| ())
                .map_err(clock_error);
            let (outcome, event) = if result.is_ok() {
                (
                    telemetry::Outcome::Ok,
                    telemetry::SignalEvent::CancelCompleted,
                )
            } else {
                (
                    telemetry::Outcome::Rejected,
                    telemetry::SignalEvent::CancelRejected,
                )
            };
            telemetry::observe(
                telemetry::Operation::Cancel,
                outcome,
                None,
                context.as_ref(),
            );
            telemetry::event(telemetry::Operation::Cancel, event, context.as_ref());
            result
        }

        // Read-only: decodes the clock's read-only timer list; removes,
        // schedules, and cancels nothing and records no telemetry
        // observation.
        fn snapshot() -> Result<Vec<PendingSend>, ComponentError> {
            let clock = clock_api::snapshot();
            super::decode_pending_sends(clock.timers.iter().map(|timer| {
                (
                    timer.id.as_str(),
                    timer.payload.as_slice(),
                    timer.deadline_ms,
                )
            }))
            .map(|pending| {
                pending
                    .into_iter()
                    .map(|send| PendingSend {
                        request: from_native_delayed(send.request),
                        deadline_ms: send.deadline_ms,
                    })
                    .collect()
            })
            // The export takes no argument: a timer that does not decode is
            // the clock or this processor breaking its contract.
            .map_err(|_| {
                error(
                    ErrorCode::Protocol,
                    "a pending delayed send does not decode",
                )
            })
        }

        fn drain_due(max_items: u32) -> Result<DueBatch, ComponentError> {
            let result = clock_api::drain_due(max_items)
                .map_err(clock_error)
                .map(|due| {
                    // The clock drain is destructive, so decoding is per item:
                    // valid requests are delivered and bad items are reported,
                    // never silently dropped with the rest of the batch.
                    let batch = super::decode_due_batch(
                        due.into_iter().map(|item| (item.id, item.payload)),
                    );
                    DueBatch {
                        delivered: batch
                            .delivered
                            .into_iter()
                            .map(from_native_delayed)
                            .collect(),
                        rejected: batch
                            .rejected
                            .into_iter()
                            .map(|(send_id, reason)| RejectedDue {
                                send_id,
                                reason: match reason {
                                    super::DueRejection::InvalidPayload => {
                                        WitDueRejection::InvalidPayload
                                    }
                                    super::DueRejection::IdentityMismatch => {
                                        WitDueRejection::IdentityMismatch
                                    }
                                },
                            })
                            .collect(),
                    }
                });
            let (outcome, event) = match &result {
                Ok(batch) if batch.rejected.is_empty() => {
                    (telemetry::Outcome::Ok, telemetry::SignalEvent::DueDrained)
                }
                Ok(_) => (
                    telemetry::Outcome::Failed,
                    telemetry::SignalEvent::DueDrained,
                ),
                Err(_) => (
                    telemetry::Outcome::Rejected,
                    telemetry::SignalEvent::SendRejected,
                ),
            };
            telemetry::observe(telemetry::Operation::DrainDue, outcome, None, None);
            telemetry::event(telemetry::Operation::DrainDue, event, None);
            result
        }
    }

    fn to_native_request(
        request: WitSendRequest,
    ) -> Result<scxml_types::SendRequest, ComponentError> {
        let send_id = request
            .send_id
            .as_deref()
            .map(scxml_types::SendId::new)
            .transpose()
            .map_err(|_| error(ErrorCode::InvalidInput, "send id is invalid"))?;
        let event_send_id = request
            .event
            .send_id
            .as_deref()
            .map(scxml_types::SendId::new)
            .transpose()
            .map_err(|_| error(ErrorCode::InvalidInput, "event send id is invalid"))?;
        if !scxml_types::send_ids_agree(send_id.as_ref(), event_send_id.as_ref()) {
            return Err(error(
                ErrorCode::InvalidInput,
                "the event carries a send id other than the request's",
            ));
        }
        Ok(scxml_types::SendRequest {
            send_id,
            event: scxml_types::Event {
                name: scxml_types::EventName::new(request.event.name)
                    .map_err(|_| error(ErrorCode::InvalidInput, "event name is invalid"))?,
                event_type: match request.event.event_type {
                    WitEventType::Internal => scxml_types::EventType::Internal,
                    WitEventType::External => scxml_types::EventType::External,
                    WitEventType::Platform => scxml_types::EventType::Platform,
                },
                send_id: event_send_id,
                origin: request.event.origin,
                origin_type: request.event.origin_type,
                invoke_id: request
                    .event
                    .invoke_id
                    .map(scxml_types::InvokeId::new)
                    .transpose()
                    .map_err(|_| error(ErrorCode::InvalidInput, "invoke id is invalid"))?,
                data: to_native_value(request.event.data)?,
                raw: request.event.raw.clone(),
                trace_context: request
                    .event
                    .trace_context
                    .map(|context| {
                        scxml_types::TraceContext::new(
                            context.traceparent,
                            context.tracestate,
                            context.baggage,
                            match context.baggage_policy {
                                WitBaggagePolicy::Drop => scxml_types::BaggagePolicy::Drop,
                                WitBaggagePolicy::Propagate => {
                                    scxml_types::BaggagePolicy::Propagate
                                }
                            },
                        )
                        .map_err(|_| error(ErrorCode::InvalidInput, "trace context is invalid"))
                    })
                    .transpose()?,
            },
            target: request.target,
            processor_type: request.processor_type,
            delay_ms: request.delay_ms,
            message: match request.message {
                WitSendMessage::Empty => scxml_types::SendMessage::Empty,
                WitSendMessage::Content => scxml_types::SendMessage::Content,
                WitSendMessage::Params(fields) => {
                    if fields.len() > scxml_types::MAX_SEND_MESSAGE_PARAMS {
                        return Err(error(
                            ErrorCode::PayloadTooLarge,
                            "send message has too many fields",
                        ));
                    }
                    scxml_types::SendMessage::Params(
                        fields
                            .into_iter()
                            .map(|field| {
                                if field.name.is_empty()
                                    || field.name.len() > scxml_types::MAX_NAME_BYTES
                                {
                                    return Err(error(
                                        ErrorCode::InvalidInput,
                                        "send message field name is invalid",
                                    ));
                                }
                                Ok(scxml_types::NamedValue {
                                    name: field.name,
                                    value: to_native_value(field.value)?,
                                })
                            })
                            .collect::<Result<_, _>>()?,
                    )
                }
            },
        })
    }

    fn validate_wit_context(
        context: Option<WitTraceContext>,
    ) -> Result<Option<WitTraceContext>, ComponentError> {
        context
            .map(|context| {
                scxml_types::TraceContext::new(
                    context.traceparent,
                    context.tracestate,
                    context.baggage,
                    match context.baggage_policy {
                        WitBaggagePolicy::Drop => scxml_types::BaggagePolicy::Drop,
                        WitBaggagePolicy::Propagate => scxml_types::BaggagePolicy::Propagate,
                    },
                )
                .map_err(|_| error(ErrorCode::InvalidInput, "trace context is invalid"))
                .map(|validated| WitTraceContext {
                    traceparent: validated.traceparent,
                    tracestate: validated.tracestate,
                    baggage: validated.baggage,
                    baggage_policy: match validated.baggage_policy {
                        scxml_types::BaggagePolicy::Drop => WitBaggagePolicy::Drop,
                        scxml_types::BaggagePolicy::Propagate => WitBaggagePolicy::Propagate,
                    },
                })
            })
            .transpose()
    }

    fn to_clock_context(context: &WitTraceContext) -> ClockTraceContext {
        ClockTraceContext {
            traceparent: context.traceparent.clone(),
            tracestate: context.tracestate.clone(),
            baggage: context.baggage.clone(),
            baggage_policy: match context.baggage_policy {
                WitBaggagePolicy::Drop => ClockBaggagePolicy::Drop,
                WitBaggagePolicy::Propagate => ClockBaggagePolicy::Propagate,
            },
        }
    }

    fn to_native_basichttp(
        request: WitBasichttpSend,
    ) -> Result<scxml_types::BasicHttpSend, ComponentError> {
        if request.params.len() > scxml_types::MAX_BASICHTTP_PARAMS {
            return Err(error(
                ErrorCode::PayloadTooLarge,
                "basichttp send has too many fields",
            ));
        }
        Ok(scxml_types::BasicHttpSend {
            send_id: scxml_types::SendId::new(request.send_id)
                .map_err(|_| error(ErrorCode::InvalidInput, "send id is invalid"))?,
            event_name: request.event_name,
            target: request.target,
            params: request
                .params
                .into_iter()
                .map(|field| {
                    Ok(scxml_types::NamedValue {
                        name: field.name,
                        value: to_native_value(field.value)?,
                    })
                })
                .collect::<Result<_, ComponentError>>()?,
            content: request.content.map(to_native_value).transpose()?,
            delay_ms: request.delay_ms,
            trace_context: request
                .trace_context
                .map(|context| {
                    scxml_types::TraceContext::new(
                        context.traceparent,
                        context.tracestate,
                        context.baggage,
                        match context.baggage_policy {
                            WitBaggagePolicy::Drop => scxml_types::BaggagePolicy::Drop,
                            WitBaggagePolicy::Propagate => scxml_types::BaggagePolicy::Propagate,
                        },
                    )
                    .map_err(|_| error(ErrorCode::InvalidInput, "trace context is invalid"))
                })
                .transpose()?,
        })
    }

    fn from_native_delayed(send: scxml_types::DelayedSend) -> WitDelayedSend {
        match send {
            scxml_types::DelayedSend::Scxml(request) => {
                WitDelayedSend::Scxml(from_native_request(request))
            }
            scxml_types::DelayedSend::BasicHttp(send) => {
                WitDelayedSend::Basichttp(WitBasichttpSend {
                    send_id: send.send_id.as_str().to_owned(),
                    event_name: send.event_name,
                    target: send.target,
                    params: send
                        .params
                        .into_iter()
                        .map(|field| WitNamedValue {
                            name: field.name,
                            value: from_native_value(field.value),
                        })
                        .collect(),
                    content: send.content.map(from_native_value),
                    delay_ms: send.delay_ms,
                    trace_context: send.trace_context.map(from_native_context),
                })
            }
        }
    }

    fn from_native_context(context: scxml_types::TraceContext) -> WitTraceContext {
        WitTraceContext {
            traceparent: context.traceparent,
            tracestate: context.tracestate,
            baggage: context.baggage,
            baggage_policy: match context.baggage_policy {
                scxml_types::BaggagePolicy::Drop => WitBaggagePolicy::Drop,
                scxml_types::BaggagePolicy::Propagate => WitBaggagePolicy::Propagate,
            },
        }
    }

    fn from_native_request(request: scxml_types::SendRequest) -> WitSendRequest {
        WitSendRequest {
            send_id: request.send_id.map(|id| id.as_str().to_owned()),
            event: from_native_event(request.event),
            target: request.target,
            processor_type: request.processor_type,
            delay_ms: request.delay_ms,
            message: match request.message {
                scxml_types::SendMessage::Empty => WitSendMessage::Empty,
                scxml_types::SendMessage::Content => WitSendMessage::Content,
                scxml_types::SendMessage::Params(fields) => WitSendMessage::Params(
                    fields
                        .into_iter()
                        .map(|field| WitNamedValue {
                            name: field.name,
                            value: from_native_value(field.value),
                        })
                        .collect(),
                ),
            },
        }
    }

    /// Whether `invoke_id` names one of the sender's live invocations.
    fn is_live_invocation(sender: &Sender, invoke_id: &str) -> bool {
        sender
            .invocations
            .iter()
            .any(|live| live.invoke_id == invoke_id)
    }

    /// The invoke id of the sender's live invocation whose session id is
    /// `session_id` (an invoked service has none, so no session address
    /// names it).
    fn invocation_of_session(sender: &Sender, session_id: &str) -> Option<String> {
        sender
            .invocations
            .iter()
            .find(|live| live.session_id.as_deref() == Some(session_id))
            .map(|live| live.invoke_id.clone())
    }

    fn from_native_event(event: scxml_types::Event) -> WitEvent {
        WitEvent {
            name: event.name.as_str().to_owned(),
            event_type: match event.event_type {
                scxml_types::EventType::Internal => WitEventType::Internal,
                scxml_types::EventType::External => WitEventType::External,
                scxml_types::EventType::Platform => WitEventType::Platform,
            },
            send_id: event.send_id.map(|id| id.as_str().to_owned()),
            origin: event.origin,
            origin_type: event.origin_type,
            invoke_id: event.invoke_id.map(|id| id.as_str().to_owned()),
            data: from_native_value(event.data),
            raw: event.raw,
            trace_context: event.trace_context.map(|context| WitTraceContext {
                traceparent: context.traceparent,
                tracestate: context.tracestate,
                baggage: context.baggage,
                baggage_policy: match context.baggage_policy {
                    scxml_types::BaggagePolicy::Drop => WitBaggagePolicy::Drop,
                    scxml_types::BaggagePolicy::Propagate => WitBaggagePolicy::Propagate,
                },
            }),
        }
    }

    fn to_native_value(value: WitDataValue) -> Result<scxml_types::DataValue, ComponentError> {
        match value {
            WitDataValue::Absent => Ok(scxml_types::DataValue::Absent),
            WitDataValue::Undefined => Ok(scxml_types::DataValue::Undefined),
            WitDataValue::Null => Ok(scxml_types::DataValue::Null),
            WitDataValue::Boolean(value) => Ok(scxml_types::DataValue::Bool(value)),
            WitDataValue::Number(value) => scxml_types::FiniteNumber::new(value)
                .map(scxml_types::DataValue::Number)
                .map_err(|_| error(ErrorCode::InvalidInput, "event number must be finite")),
            WitDataValue::Text(value) => Ok(scxml_types::DataValue::String(value)),
            WitDataValue::JsonArray(value) => Ok(scxml_types::DataValue::JsonArray(value)),
            WitDataValue::JsonObject(value) => Ok(scxml_types::DataValue::JsonObject(value)),
            WitDataValue::Xml(value) => Ok(scxml_types::DataValue::Xml(value)),
        }
    }

    fn from_native_value(value: scxml_types::DataValue) -> WitDataValue {
        match value {
            scxml_types::DataValue::Absent => WitDataValue::Absent,
            scxml_types::DataValue::Undefined => WitDataValue::Undefined,
            scxml_types::DataValue::Null => WitDataValue::Null,
            scxml_types::DataValue::Bool(value) => WitDataValue::Boolean(value),
            scxml_types::DataValue::Number(value) => WitDataValue::Number(value.get()),
            scxml_types::DataValue::String(value) => WitDataValue::Text(value),
            scxml_types::DataValue::JsonArray(value) => WitDataValue::JsonArray(value),
            scxml_types::DataValue::JsonObject(value) => WitDataValue::JsonObject(value),
            scxml_types::DataValue::Xml(value) => WitDataValue::Xml(value),
        }
    }

    /// Maps one shared processor decision failure to its WIT error code.
    fn processor_error(cause: super::ProcessorError) -> ComponentError {
        let (code, detail) = match cause {
            super::ProcessorError::WrongProcessorType => (
                ErrorCode::Protocol,
                "request selected a different I/O processor",
            ),
            super::ProcessorError::MissingSendId => (
                ErrorCode::InvalidInput,
                "delayed delivery requires a send id",
            ),
            super::ProcessorError::MismatchedSendId => (
                ErrorCode::InvalidInput,
                "the event carries a send id other than the request's",
            ),
            super::ProcessorError::PayloadTooLarge => (
                ErrorCode::PayloadTooLarge,
                "send request exceeds the byte bound",
            ),
            super::ProcessorError::InvalidTarget => (
                ErrorCode::InvalidInput,
                "send target is invalid for this processor",
            ),
            super::ProcessorError::NotDelayed => (
                ErrorCode::InvalidInput,
                "a scheduled basichttp send requires a delay",
            ),
            super::ProcessorError::InvalidSend => {
                (ErrorCode::InvalidInput, "the delayed send is not canonical")
            }
            super::ProcessorError::UnreachableTarget => {
                (ErrorCode::Unavailable, "send target session is unreachable")
            }
        };
        error(code, detail)
    }

    fn clock_error(clock_failure: ClockError) -> ComponentError {
        match clock_failure {
            ClockError::CapacityExhausted => {
                error(ErrorCode::CapacityExhausted, "clock capacity is exhausted")
            }
            ClockError::Overflow | ClockError::InvalidDuration => {
                error(ErrorCode::InvalidInput, "clock deadline is invalid")
            }
            ClockError::DuplicateId => {
                error(ErrorCode::DuplicateId, "send id is already scheduled")
            }
            // The clock reports `invalid-id` only for a malformed id (empty
            // or over its byte bound); an absent id is not an error.
            ClockError::InvalidId => error(ErrorCode::InvalidInput, "send id is invalid"),
            ClockError::PayloadTooLarge => error(
                ErrorCode::PayloadTooLarge,
                "scheduled payload exceeds the byte bound",
            ),
            ClockError::InvalidLimit => error(
                ErrorCode::Protocol,
                "clock contract returned an invalid result",
            ),
            ClockError::InvalidContext => {
                error(ErrorCode::InvalidInput, "trace context is invalid")
            }
        }
    }

    fn error(code: ErrorCode, detail: &'static str) -> ComponentError {
        ComponentError {
            version: ErrorVersion::V1,
            code,
            detail: (detail.len() <= 160).then(|| detail.into()),
        }
    }

    export!(Component);
}

use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};
use scxml_types::{BasicHttpSend, DelayedSend, Event, SendRequest};

// Processor-type and invoke-type spelling lives in `scxml-types` so the
// engine's `<send>`/`<invoke>` interpreter classifies exactly as this
// processor does; it is re-exported here unchanged.
pub use scxml_types::{
    BASICHTTP_EVENT_PROCESSOR_URI, BASICHTTP_PROCESSOR_ALIAS, ProcessorKind,
    SCXML_EVENT_PROCESSOR_URI, SCXML_INVOKE_TYPE_URI, SCXML_PROCESSOR_ALIAS,
    classify_processor_type, is_scxml_invoke_type,
};

/// The `_event.origin` URI the SCXML Event I/O Processor stamps on events
/// sent by session `session_id` (SCXML C.1: `#_scxml_` + session id).
#[must_use]
pub fn session_origin(session_id: &str) -> String {
    format!("#_scxml_{session_id}")
}

/// Addressing view of the sending session used by [`classify_target`].
pub struct SessionAddress<'a> {
    /// The sending session's `_sessionid`.
    pub session_id: &'a str,
    /// The invoking parent's session id, for an invoked session.
    pub parent_session_id: Option<&'a str>,
    /// Whether `id` names a live invocation owned by the sending session.
    pub is_live_invocation: &'a dyn Fn(&str) -> bool,
    /// The invoke id of the live invocation whose session id is `id`.
    pub invocation_of_session: &'a dyn Fn(&str) -> Option<String>,
}

/// Where the SCXML Event I/O Processor delivers one `<send>` (SCXML 6.2 and
/// C.1).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SendTarget {
    /// `#_internal`: the sender's internal queue.
    Internal,
    /// Absent, or the sender's own `#_scxml_sessionid`: the sender's
    /// external queue.
    Local,
    /// `#_parent` or the parent's `#_scxml_sessionid`: the invoking session.
    Parent,
    /// A live invocation of the sender (`#_invokeid`, or the invoked
    /// session's `#_scxml_sessionid`), by invoke id.
    Invocation(String),
    /// A well-formed session address no live session answers
    /// (`error.communication`).
    Unreachable,
    /// Any other target form (`error.execution`).
    Invalid,
}

/// Classifies one `<send>` target for the SCXML Event I/O Processor, by
/// the special targets SCXML C.1 defines: "If neither the 'target' nor the
/// 'targetexpr' attribute is specified, the SCXML Processor MUST add the
/// event to the external event queue of the sending session"; `#_internal`
/// is the internal queue; `#_scxml_sessionid` "where sessionid is the id of
/// an SCXML session that is accessible to the Processor" (the sender
/// itself, its invoking parent, and the sessions it invoked that still
/// run); `#_parent`; and `#_invokeid` "where invokeid is the invokeid of an
/// SCXML session that the sending session has created by `<invoke>`". "If
/// the sending SCXML session specifies a session that does not exist or is
/// inaccessible", the target is [`SendTarget::Unreachable`]
/// (`error.communication`); any other form is [`SendTarget::Invalid`].
#[must_use]
pub fn classify_target(target: Option<&str>, sender: &SessionAddress<'_>) -> SendTarget {
    let Some(target) = target else {
        return SendTarget::Local;
    };
    if target == "#_internal" {
        return SendTarget::Internal;
    }
    if let Some(session_id) = target.strip_prefix("#_scxml_") {
        return if session_id == sender.session_id {
            SendTarget::Local
        } else if sender.parent_session_id == Some(session_id) {
            SendTarget::Parent
        } else if let Some(invoke_id) = (sender.invocation_of_session)(session_id) {
            SendTarget::Invocation(invoke_id)
        } else {
            SendTarget::Unreachable
        };
    }
    if target == "#_parent" && sender.parent_session_id.is_some() {
        return SendTarget::Parent;
    }
    match target.strip_prefix("#_") {
        Some(invoke_id) if (sender.is_live_invocation)(invoke_id) => {
            SendTarget::Invocation(invoke_id.to_owned())
        }
        Some(_) => SendTarget::Unreachable,
        None => SendTarget::Invalid,
    }
}

/// Aliases documents may use for this processor's type, in `_ioprocessors`
/// order (WIT `io-processor-api.aliases`).
pub const SCXML_PROCESSOR_ALIASES: [&str; 1] = [SCXML_PROCESSOR_ALIAS];

/// What the host must do with one accepted send (WIT `dispatch-decision`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Dispatch {
    /// A delayed send for the clock.
    Scheduled(ScheduledSend),
    /// Deliver the stamped event to the sender's own external queue.
    Local(Event),
    /// Deliver the stamped event to the sender's invoking parent.
    Parent(Event),
    /// Deliver the stamped event to one live invocation of the sender.
    Invocation {
        /// Invoke id of the addressed child.
        invoke_id: String,
        /// The stamped event.
        event: Event,
    },
}

/// Where one due delayed send is delivered (WIT `due-route`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DueRoute {
    /// The sender's own external queue.
    Local(Event),
    /// The sender's invoking parent.
    Parent(Event),
    /// One live invocation of the sender.
    Invocation {
        /// Invoke id of the addressed child.
        invoke_id: String,
        /// The event to deliver.
        event: Event,
    },
    /// Nobody answers the target any more (for example an invoked session
    /// that finished), so the message cannot be dispatched when its delay
    /// elapses: the sender raises `error.communication` carrying the
    /// send's id (SCXML 6.2: "It MUST dispatch the message only when the
    /// delay interval elapses"; 6.2.4: "If it is unable to dispatch the
    /// message, the Processor MUST place the error error.communication";
    /// C.1). A sender that itself terminated never reaches this: its
    /// pending sends are discarded (6.2, official case 187).
    Undeliverable {
        /// The failed send's id.
        send_id: Option<scxml_types::SendId>,
    },
}

/// One validated delayed send ready for the project clock: the timer id is
/// the caller-owned send id and the payload the canonical request encoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduledSend {
    /// Clock timer id (the request's send id).
    pub send_id: String,
    /// Canonical encoded request, at most
    /// [`scxml_types::MAX_DELAYED_SEND_BYTES`] bytes.
    pub payload: Vec<u8>,
    /// Positive delay in logical milliseconds.
    pub delay_ms: u64,
}

/// Stateless SCXML Event I/O Processor.
#[derive(Default)]
pub struct ScxmlIoProcessor;

impl ScxmlIoProcessor {
    /// Returns the canonical processor type URI.
    #[must_use]
    pub const fn type_uri(&self) -> &'static str {
        SCXML_EVENT_PROCESSOR_URI
    }

    /// Validates a send request and returns the event for host routing.
    ///
    /// # Errors
    ///
    /// As [`Self::plan`].
    pub fn prepare(&self, request: SendRequest) -> Result<Event, ProcessorError> {
        let span = observe(Module::IoProcessorComponent, Operation::IoProcessorPrepare);
        let result = self.plan(&request).map(|_| request.event);
        if result.is_ok() {
            span.event(SignalEvent::IoProcessorSendAccepted);
        } else {
            span.fail(ErrorCategory::InvalidInput);
            span.event(SignalEvent::IoProcessorSendRejected);
        }
        result
    }

    /// Validates one send request from `sender`, stamps its event's
    /// `origin` ([`session_origin`]), `origin-type` (the authored processor
    /// type, else [`SCXML_EVENT_PROCESSOR_URI`]), and `raw` (rendered from
    /// the request's message), and either plans a delayed timer or
    /// classifies the target for immediate delivery (WIT
    /// `io-processor-api.dispatch`). The target is classified before the
    /// delay applies.
    ///
    /// # Errors
    ///
    /// As [`Self::plan`], plus [`ProcessorError::InvalidTarget`] for an
    /// `#_internal` or malformed target, [`ProcessorError::UnreachableTarget`]
    /// for a session address nobody answers, and
    /// [`ProcessorError::PayloadTooLarge`] for a raw message over its bound.
    pub fn dispatch(
        &self,
        mut request: SendRequest,
        sender: &SessionAddress<'_>,
    ) -> Result<Dispatch, ProcessorError> {
        let span = observe(Module::IoProcessorComponent, Operation::IoProcessorDispatch);
        let result = (|| {
            Self::check_request(&request)?;
            let target = classify_target(request.target.as_deref(), sender);
            match target {
                SendTarget::Internal | SendTarget::Invalid => {
                    return Err(ProcessorError::InvalidTarget);
                }
                SendTarget::Unreachable => return Err(ProcessorError::UnreachableTarget),
                SendTarget::Local | SendTarget::Parent | SendTarget::Invocation(_) => {}
            }
            request.event.origin = Some(session_origin(sender.session_id));
            request.event.origin_type = Some(
                request
                    .processor_type
                    .clone()
                    .unwrap_or_else(|| SCXML_EVENT_PROCESSOR_URI.to_owned()),
            );
            let fields: Vec<(String, scxml_types::DataValue)> = match &request.message {
                scxml_types::SendMessage::Params(fields) => fields
                    .iter()
                    .map(|field| (field.name.clone(), field.value.clone()))
                    .collect(),
                scxml_types::SendMessage::Empty | scxml_types::SendMessage::Content => Vec::new(),
            };
            let raw_payload = match &request.message {
                scxml_types::SendMessage::Empty => RawPayload::None,
                scxml_types::SendMessage::Params(_) => RawPayload::Params(&fields),
                scxml_types::SendMessage::Content => RawPayload::Content(&request.event.data),
            };
            let raw = raw_message(&request.event.name, raw_payload)?;
            request.event.raw = Some(raw);
            if let Some(scheduled) = self.plan(&request)? {
                return Ok(Dispatch::Scheduled(scheduled));
            }
            Ok(match target {
                SendTarget::Parent => Dispatch::Parent(request.event),
                SendTarget::Invocation(invoke_id) => Dispatch::Invocation {
                    invoke_id,
                    event: request.event,
                },
                _ => Dispatch::Local(request.event),
            })
        })();
        if result.is_ok() {
            span.event(SignalEvent::IoProcessorSendAccepted);
        } else {
            span.fail(ErrorCategory::InvalidInput);
            span.event(SignalEvent::IoProcessorSendRejected);
        }
        result
    }

    /// Routes one due delayed request from `sender` at delivery time (WIT
    /// `io-processor-api.route-due`): the target is classified again, so a
    /// child that finished or was cancelled since scheduling yields
    /// [`DueRoute::Undeliverable`]. Pure.
    #[must_use]
    pub fn route_due(&self, request: SendRequest, sender: &SessionAddress<'_>) -> DueRoute {
        match classify_target(request.target.as_deref(), sender) {
            SendTarget::Local => DueRoute::Local(request.event),
            SendTarget::Parent => DueRoute::Parent(request.event),
            SendTarget::Invocation(invoke_id) => DueRoute::Invocation {
                invoke_id,
                event: request.event,
            },
            SendTarget::Internal | SendTarget::Unreachable | SendTarget::Invalid => {
                DueRoute::Undeliverable {
                    send_id: request.send_id,
                }
            }
        }
    }

    /// Rejects another processor type and disagreeing request and event
    /// send ids ([`scxml_types::send_ids_agree`]).
    fn check_request(request: &SendRequest) -> Result<(), ProcessorError> {
        if request
            .processor_type
            .as_deref()
            .is_some_and(|value| classify_processor_type(value) != ProcessorKind::Scxml)
        {
            return Err(ProcessorError::WrongProcessorType);
        }
        if !scxml_types::send_ids_agree(request.send_id.as_ref(), request.event.send_id.as_ref()) {
            return Err(ProcessorError::MismatchedSendId);
        }
        Ok(())
    }

    /// Validates one send request and plans its delivery: `None` for
    /// immediate delivery, or the clock timer for a delayed one. The Wasm
    /// component schedules the timer through the imported project clock;
    /// this is the one decision every delayed send of the processor takes
    /// before it is scheduled.
    ///
    /// # Errors
    ///
    /// Rejects a different processor type, delayed delivery without a
    /// caller-owned send id, disagreeing request and event send ids, and a
    /// request whose canonical encoding exceeds its byte bound.
    pub fn plan(&self, request: &SendRequest) -> Result<Option<ScheduledSend>, ProcessorError> {
        Self::check_request(request)?;
        if request.delay_ms == 0 {
            return Ok(None);
        }
        let send_id = request
            .send_id
            .as_ref()
            .ok_or(ProcessorError::MissingSendId)?;
        let payload = scxml_types::encode_delayed_send(&DelayedSend::Scxml(request.clone()))
            .map_err(|_| ProcessorError::PayloadTooLarge)?;
        Ok(Some(ScheduledSend {
            send_id: send_id.as_str().to_owned(),
            payload,
            delay_ms: request.delay_ms,
        }))
    }

    /// Plans the clock timer of one delayed `BasicHTTP` send (WIT
    /// `io-processor-api.schedule-basichttp`): the processor holds it on
    /// the clock under its send id until the delay elapses (SCXML 6.2: "It
    /// MUST dispatch the message only when the delay interval elapses."),
    /// so it is cancelled (6.3) and discarded (6.2) like every delayed send
    /// of the session. The `BasicHTTP` processor validated the send.
    ///
    /// # Errors
    ///
    /// [`ProcessorError::NotDelayed`] for a zero delay,
    /// [`ProcessorError::PayloadTooLarge`] when the canonical encoding
    /// exceeds [`scxml_types::MAX_DELAYED_SEND_BYTES`] or a field bound, and
    /// [`ProcessorError::InvalidSend`] for a value the encoding rejects.
    pub fn plan_basichttp(&self, send: &BasicHttpSend) -> Result<ScheduledSend, ProcessorError> {
        if send.delay_ms == 0 {
            return Err(ProcessorError::NotDelayed);
        }
        let payload = scxml_types::encode_delayed_send(&DelayedSend::BasicHttp(send.clone()))
            .map_err(|error| match error {
                scxml_types::SendCodecError::TooLarge => ProcessorError::PayloadTooLarge,
                scxml_types::SendCodecError::Invalid
                | scxml_types::SendCodecError::UnsupportedVersion => ProcessorError::InvalidSend,
            })?;
        Ok(ScheduledSend {
            send_id: send.send_id.as_str().to_owned(),
            payload,
            delay_ms: send.delay_ms,
        })
    }
}

/// Why one drained clock item could not be delivered as a send request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DueRejection {
    /// The payload was not a canonical delayed-send request.
    InvalidPayload,
    /// The payload named a send id different from the clock timer id.
    IdentityMismatch,
}

/// Outcome of decoding one batch drained from the clock.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DueBatch {
    /// Valid sends in clock order.
    pub delivered: Vec<DelayedSend>,
    /// Timer ids of undeliverable items, in clock order, with the reason.
    pub rejected: Vec<(String, DueRejection)>,
}

/// Decodes one batch of items already removed from the clock.
///
/// The clock's `drain-due` is destructive, so this decode is per item and
/// never fails as a whole: every valid request is delivered in clock order,
/// and every undeliverable item is reported with its timer id and a closed
/// reason instead of being silently dropped or taking valid items with it.
/// Bounded by the caller's drain limit (at most
/// `scxml_clock::MAX_DRAIN_ITEMS` items of at most
/// [`scxml_types::MAX_DELAYED_SEND_BYTES`] each).
#[must_use]
pub fn decode_due_batch(items: impl IntoIterator<Item = (String, Vec<u8>)>) -> DueBatch {
    let mut batch = DueBatch::default();
    for (id, payload) in items {
        match scxml_types::decode_delayed_send(&payload) {
            Ok(send) if send.send_id().map(scxml_types::SendId::as_str) == Some(id.as_str()) => {
                batch.delivered.push(send);
            }
            Ok(_) => batch.rejected.push((id, DueRejection::IdentityMismatch)),
            Err(_) => batch.rejected.push((id, DueRejection::InvalidPayload)),
        }
    }
    batch
}

/// One pending delayed send (a cancelable, SCXML 6.2/6.3) decoded from a
/// live clock timer.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingSend {
    /// The send as scheduled.
    pub request: DelayedSend,
    /// Absolute deadline on the virtual clock, in milliseconds.
    pub deadline_ms: u64,
}

/// Decodes the clock's live timers, given as `(id, payload, deadline_ms)`
/// in clock order, into pending delayed sends in the same order. Read-only.
///
/// Every timer this processor schedules is a canonical delayed-send
/// request whose send id is the timer id, so any other timer is a broken
/// invariant and fails the whole decode: unlike a destructive drain,
/// nothing is lost by failing. Bounded by the clock's live-timer bound.
///
/// # Errors
///
/// [`DueRejection::InvalidPayload`] or [`DueRejection::IdentityMismatch`]
/// for the first timer that does not decode. The component reports either
/// as `protocol`: the caller supplied nothing, so the failure is a broken
/// dependency invariant, not bad input.
pub fn decode_pending_sends<'a>(
    timers: impl IntoIterator<Item = (&'a str, &'a [u8], u64)>,
) -> Result<Vec<PendingSend>, DueRejection> {
    let span = observe(Module::IoProcessorComponent, Operation::IoProcessorSnapshot);
    let result: Result<Vec<PendingSend>, DueRejection> = timers
        .into_iter()
        .map(|(id, payload, deadline_ms)| {
            let request = scxml_types::decode_delayed_send(payload)
                .map_err(|_| DueRejection::InvalidPayload)?;
            if request.send_id().map(scxml_types::SendId::as_str) != Some(id) {
                return Err(DueRejection::IdentityMismatch);
            }
            Ok(PendingSend {
                request,
                deadline_ms,
            })
        })
        .collect();
    if result.is_ok() {
        span.event(SignalEvent::TransformationCompleted);
    } else {
        span.event(SignalEvent::InputRejected);
        span.fail(ErrorCategory::Platform);
    }
    result
}

/// SCXML Event I/O Processor validation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessorError {
    /// A different processor type was requested.
    WrongProcessorType,
    /// Delayed delivery requires a caller-owned cancellation identity.
    MissingSendId,
    /// The delivered event carries a send id other than the request's.
    MismatchedSendId,
    /// The canonical delayed-send encoding or the raw message exceeds its
    /// byte bound.
    PayloadTooLarge,
    /// The target is `#_internal` or not a target this processor accepts
    /// (SCXML 6.2: `error.execution`).
    InvalidTarget,
    /// A `BasicHTTP` send handed to the clock carries no delay.
    NotDelayed,
    /// A `BasicHTTP` send handed to the clock holds a value its canonical
    /// encoding rejects.
    InvalidSend,
    /// The target is a well-formed session address no live session answers
    /// (SCXML 6.2: `error.communication`).
    UnreachableTarget,
}

impl std::fmt::Display for ProcessorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::WrongProcessorType => "request selected a different I/O processor",
            Self::MissingSendId => "delayed delivery requires a caller-owned send id",
            Self::MismatchedSendId => "the event carries a send id other than the request's",
            Self::PayloadTooLarge => "send request exceeds the byte bound",
            Self::InvalidTarget => "send target is invalid for this processor",
            Self::UnreachableTarget => "send target session is unreachable",
            Self::NotDelayed => "a scheduled basichttp send requires a delay",
            Self::InvalidSend => "the delayed send is not canonical",
        })
    }
}

impl std::error::Error for ProcessorError {}

mod raw;

pub use raw::{
    MAX_RAW_MESSAGE_BYTES, RAW_CONTENT_FIELD, RAW_EVENT_NAME_FIELD, RawPayload, field_text,
    raw_message,
};

#[cfg(test)]
mod tests {
    use super::*;
    use scxml_telemetry::{Recorder, install_recorder};
    use scxml_types::{EventName, SendId, SendRequest};

    fn encode(request: &SendRequest) -> Vec<u8> {
        scxml_types::encode_delayed_send(&DelayedSend::Scxml(request.clone()))
            .expect("fixture request encodes")
    }

    fn basichttp(id: &str, delay_ms: u64) -> BasicHttpSend {
        BasicHttpSend {
            send_id: SendId::new(id).expect("test send id is valid"),
            event_name: Some("due".into()),
            target: Some("http://example.test/in".into()),
            params: Vec::new(),
            content: None,
            delay_ms,
            trace_context: None,
        }
    }

    fn delayed(id: &str) -> SendRequest {
        SendRequest {
            send_id: Some(SendId::new(id).expect("test send id is valid")),
            event: Event {
                send_id: Some(SendId::new(id).expect("test send id is valid")),
                ..Event::external(EventName::new("due").expect("test event name is valid"))
            },
            target: None,
            processor_type: Some("scxml".into()),
            delay_ms: 5,
            message: scxml_types::SendMessage::Empty,
        }
    }

    #[test]
    fn documented_wit_bounds_match_enforced_constants() {
        // The wasm clock's bounds `scxml.wit` documents; natively the clock
        // is unbounded.
        const DOCUMENTED_CLOCK_ID_BYTES: usize = 256;
        const DOCUMENTED_CLOCK_TOTAL_PAYLOAD_BYTES: usize = 4 * 1_024 * 1_024;
        const DOCUMENTED_CLOCK_SCHEDULES: usize = 1_024;
        assert_eq!(
            scxml_clock::MAX_ID_BYTES,
            scxml_types::wasm_bound(DOCUMENTED_CLOCK_ID_BYTES)
        );
        assert_eq!(
            scxml_clock::MAX_TOTAL_PAYLOAD_BYTES,
            scxml_types::wasm_bound(DOCUMENTED_CLOCK_TOTAL_PAYLOAD_BYTES)
        );
        assert_eq!(
            scxml_clock::MAX_SCHEDULES,
            scxml_types::wasm_bound(DOCUMENTED_CLOCK_SCHEDULES)
        );
        let docs = include_str!("../../../crates/scxml-wit/wit/scxml/scxml.wit")
            .lines()
            .map(str::trim)
            .filter_map(|line| line.strip_prefix("///"))
            .flat_map(str::split_whitespace)
            .collect::<Vec<_>>()
            .join(" ");
        for phrase in [
            format!("1 <= `max-items` <= {}", scxml_clock::MAX_DRAIN_ITEMS),
            format!("(at most {DOCUMENTED_CLOCK_ID_BYTES} bytes)"),
            format!("empty or over {DOCUMENTED_CLOCK_ID_BYTES} bytes"),
            format!(
                "encoded request is at most {} bytes",
                scxml_types::MAX_DELAYED_SEND_BYTES
            ),
            format!("clock's {DOCUMENTED_CLOCK_TOTAL_PAYLOAD_BYTES}-byte"),
            format!("holds {DOCUMENTED_CLOCK_SCHEDULES} timers"),
        ] {
            assert!(
                docs.contains(&phrase),
                "io-processor-api must document the enforced bound: {phrase:?}"
            );
        }
        assert_eq!(
            scxml_clock::MAX_PAYLOAD_BYTES,
            scxml_types::wasm_bound(scxml_types::MAX_DELAYED_SEND_BYTES),
            "every encodable delayed send must fit one clock payload"
        );
    }

    #[test]
    fn one_bad_due_item_does_not_lose_valid_items_in_the_same_batch() {
        let first = delayed("first");
        let third = delayed("third");
        let mismatched = encode(&delayed("other"));
        let items = vec![
            ("first".to_owned(), encode(&first)),
            ("corrupt".to_owned(), vec![0xff, 0x00]),
            ("mismatch".to_owned(), mismatched),
            ("third".to_owned(), encode(&third)),
        ];
        let batch = decode_due_batch(items);
        assert_eq!(
            batch.delivered,
            vec![DelayedSend::Scxml(first), DelayedSend::Scxml(third)]
        );
        assert_eq!(
            batch.rejected,
            vec![
                ("corrupt".to_owned(), DueRejection::InvalidPayload),
                ("mismatch".to_owned(), DueRejection::IdentityMismatch),
            ]
        );
    }

    #[test]
    fn delayed_basichttp_sends_plan_under_their_send_id_and_fail_closed() {
        // SCXML 6.2: "It MUST dispatch the message only when the delay
        // interval elapses." The processor holds the send on the clock.
        let send = basichttp("post", 250);
        let scheduled = ScxmlIoProcessor
            .plan_basichttp(&send)
            .expect("a delayed BasicHTTP send plans");
        assert_eq!(scheduled.send_id, "post");
        assert_eq!(scheduled.delay_ms, 250);
        let batch = decode_due_batch([("post".to_owned(), scheduled.payload.clone())]);
        assert_eq!(batch.delivered, vec![DelayedSend::BasicHttp(send)]);
        let mismatched = decode_due_batch([("other".to_owned(), scheduled.payload)]);
        assert_eq!(
            mismatched.rejected,
            vec![("other".to_owned(), DueRejection::IdentityMismatch)]
        );
        assert_eq!(
            ScxmlIoProcessor.plan_basichttp(&basichttp("now", 0)),
            Err(ProcessorError::NotDelayed)
        );
        let oversized = BasicHttpSend {
            content: Some(scxml_types::DataValue::String(
                "x".repeat(scxml_types::MAX_DELAYED_SEND_BYTES),
            )),
            ..basichttp("big", 1)
        };
        assert_eq!(
            ScxmlIoProcessor.plan_basichttp(&oversized),
            Err(ProcessorError::PayloadTooLarge)
        );
        let unnamed = BasicHttpSend {
            params: vec![scxml_types::NamedValue {
                name: String::new(),
                value: scxml_types::DataValue::Null,
            }],
            ..basichttp("bad", 1)
        };
        assert_eq!(
            ScxmlIoProcessor.plan_basichttp(&unnamed),
            Err(ProcessorError::InvalidSend)
        );
    }

    #[test]
    fn pending_sends_decode_in_clock_order_and_fail_whole_on_a_broken_timer() {
        let first = encode(&delayed("first"));
        let second = ScxmlIoProcessor
            .plan_basichttp(&basichttp("second", 5))
            .expect("a delayed BasicHTTP send plans")
            .payload;
        let pending = decode_pending_sends([
            ("first", first.as_slice(), 105),
            ("second", second.as_slice(), 105),
        ])
        .expect("canonical timers decode");
        assert_eq!(
            pending,
            [
                PendingSend {
                    request: DelayedSend::Scxml(delayed("first")),
                    deadline_ms: 105,
                },
                PendingSend {
                    request: DelayedSend::BasicHttp(basichttp("second", 5)),
                    deadline_ms: 105,
                },
            ]
        );
        assert_eq!(decode_pending_sends([]), Ok(Vec::new()));
        assert_eq!(
            decode_pending_sends([
                ("first", first.as_slice(), 105),
                ("corrupt", [0xff_u8, 0x00].as_slice(), 106),
            ]),
            Err(DueRejection::InvalidPayload)
        );
        assert_eq!(
            decode_pending_sends([("other", first.as_slice(), 105)]),
            Err(DueRejection::IdentityMismatch)
        );
    }

    fn immediate(target: Option<&str>, message: scxml_types::SendMessage) -> SendRequest {
        SendRequest {
            send_id: None,
            event: Event {
                data: scxml_types::DataValue::String("body".into()),
                ..Event::external(EventName::new("ping").expect("test event name is valid"))
            },
            target: target.map(str::to_owned),
            processor_type: None,
            delay_ms: 0,
            message,
        }
    }

    #[test]
    fn dispatch_stamps_the_event_and_classifies_the_target() {
        let children = |id: &str| id == "child";
        let parent = SessionAddress {
            session_id: "root",
            parent_session_id: None,
            is_live_invocation: &children,
            invocation_of_session: &|_| None,
        };
        let fields = scxml_types::SendMessage::Params(vec![
            scxml_types::NamedValue {
                name: "a".into(),
                value: scxml_types::DataValue::String("1".into()),
            },
            scxml_types::NamedValue {
                name: "a".into(),
                value: scxml_types::DataValue::Null,
            },
        ]);
        let Ok(Dispatch::Local(local)) =
            ScxmlIoProcessor.dispatch(immediate(None, fields), &parent)
        else {
            panic!("an absent target is the sender's own external queue");
        };
        assert_eq!(local.origin.as_deref(), Some("#_scxml_root"));
        assert_eq!(
            local.origin_type.as_deref(),
            Some(SCXML_EVENT_PROCESSOR_URI)
        );
        // SCXML 6.2.4: duplicate names are kept in document order.
        assert_eq!(
            local.raw.as_deref(),
            Some("_scxmleventname=ping&a=1&a=null")
        );
        let Ok(Dispatch::Local(content)) = ScxmlIoProcessor.dispatch(
            immediate(Some("#_scxml_root"), scxml_types::SendMessage::Content),
            &parent,
        ) else {
            panic!("the sender's own #_scxml_sessionid is its external queue");
        };
        assert_eq!(
            content.raw.as_deref(),
            Some("_scxmleventname=ping&_scxmlcontent=body")
        );
        assert!(matches!(
            ScxmlIoProcessor.dispatch(
                immediate(Some("#_child"), scxml_types::SendMessage::Empty),
                &parent
            ),
            Ok(Dispatch::Invocation { ref invoke_id, .. }) if invoke_id == "child"
        ));
        assert_eq!(
            ScxmlIoProcessor.dispatch(
                immediate(Some("#_internal"), scxml_types::SendMessage::Empty),
                &parent
            ),
            Err(ProcessorError::InvalidTarget)
        );
        assert_eq!(
            ScxmlIoProcessor.dispatch(
                immediate(
                    Some("http://example.invalid/"),
                    scxml_types::SendMessage::Empty
                ),
                &parent
            ),
            Err(ProcessorError::InvalidTarget)
        );
        assert_eq!(
            ScxmlIoProcessor.dispatch(
                immediate(Some("#_scxml_nobody"), scxml_types::SendMessage::Empty),
                &parent
            ),
            Err(ProcessorError::UnreachableTarget)
        );

        let none = |_: &str| false;
        let child = SessionAddress {
            session_id: "child",
            parent_session_id: Some("root"),
            is_live_invocation: &none,
            invocation_of_session: &|_| None,
        };
        assert!(matches!(
            ScxmlIoProcessor.dispatch(
                immediate(Some("#_parent"), scxml_types::SendMessage::Empty),
                &child
            ),
            Ok(Dispatch::Parent(_))
        ));
        let Ok(Dispatch::Scheduled(scheduled)) = ScxmlIoProcessor.dispatch(delayed("t1"), &child)
        else {
            panic!("a delayed request is scheduled");
        };
        let Ok(DelayedSend::Scxml(decoded)) = scxml_types::decode_delayed_send(&scheduled.payload)
        else {
            panic!("the scheduled payload is a canonical SCXML send");
        };
        assert_eq!(decoded.event.origin.as_deref(), Some("#_scxml_child"));
        assert_eq!(decoded.event.origin_type.as_deref(), Some("scxml"));
        assert_eq!(decoded.event.raw.as_deref(), Some("_scxmleventname=due"));
    }

    #[test]
    fn an_invoked_session_addresses_the_sessions_it_invoked() {
        // An invoked session addresses the sessions it invoked in turn
        // (SCXML 6.2 `#_<invokeid>`), like any invoking session.
        let grandchildren = |id: &str| id == "gc";
        let session_of = |id: &str| (id == "child.1").then(|| "gc".to_owned());
        let invoking_child = SessionAddress {
            session_id: "child",
            parent_session_id: Some("root"),
            is_live_invocation: &grandchildren,
            invocation_of_session: &session_of,
        };
        // SCXML C.1: `#_invokeid` and the invoked session's
        // `#_scxml_sessionid` both address it.
        for target in ["#_gc", "#_scxml_child.1"] {
            assert_eq!(
                classify_target(Some(target), &invoking_child),
                SendTarget::Invocation("gc".into()),
                "{target}"
            );
        }
        assert_eq!(
            classify_target(Some("#_parent"), &invoking_child),
            SendTarget::Parent
        );
        assert_eq!(
            classify_target(Some("#_scxml_root"), &invoking_child),
            SendTarget::Parent
        );
        // Only the forms C.1 defines: an unknown session or invoke id is
        // unreachable (error.communication); a bare id, `#_self`, and
        // `#_scxml_<invokeid>` are none of them.
        assert_eq!(
            classify_target(Some("gc"), &invoking_child),
            SendTarget::Invalid
        );
        for target in ["#_self", "#_scxml_gc", "#_child", "#_nobody"] {
            assert_eq!(
                classify_target(Some(target), &invoking_child),
                SendTarget::Unreachable,
                "{target}"
            );
        }
    }

    #[test]
    fn route_due_reclassifies_at_delivery_time() {
        let live = |id: &str| id == "child";
        let before = SessionAddress {
            session_id: "root",
            parent_session_id: None,
            is_live_invocation: &live,
            invocation_of_session: &|_| None,
        };
        let request = SendRequest {
            target: Some("#_child".into()),
            ..delayed("t1")
        };
        assert!(matches!(
            ScxmlIoProcessor.route_due(request.clone(), &before),
            DueRoute::Invocation { ref invoke_id, .. } if invoke_id == "child"
        ));
        // The child finished before delivery: the send cannot be
        // dispatched, and the sender is told with the send's id.
        let finished = |_: &str| false;
        let after = SessionAddress {
            session_id: "root",
            parent_session_id: None,
            is_live_invocation: &finished,
            invocation_of_session: &|_| None,
        };
        assert_eq!(
            ScxmlIoProcessor.route_due(request, &after),
            DueRoute::Undeliverable {
                send_id: Some(SendId::new("t1").expect("fixture id")),
            }
        );
        assert!(matches!(
            ScxmlIoProcessor.route_due(delayed("t2"), &after),
            DueRoute::Local(_)
        ));
    }

    #[test]
    fn immediate_scxml_event_is_accepted() {
        let request = SendRequest {
            send_id: None,
            event: Event::external(EventName::new("ping").expect("test event name is valid")),
            target: None,
            processor_type: Some("scxml".into()),
            delay_ms: 0,
            message: scxml_types::SendMessage::Empty,
        };
        assert_eq!(
            ScxmlIoProcessor
                .prepare(request)
                .expect("immediate SCXML request is valid")
                .name
                .as_str(),
            "ping"
        );
    }

    #[test]
    fn delayed_scxml_event_requires_a_send_id() {
        let event = Event::external(EventName::new("ping").expect("test event name is valid"));
        let missing = SendRequest {
            send_id: None,
            event: event.clone(),
            target: None,
            processor_type: Some("scxml".into()),
            delay_ms: 1,
            message: scxml_types::SendMessage::Empty,
        };
        assert_eq!(
            ScxmlIoProcessor.prepare(missing),
            Err(ProcessorError::MissingSendId)
        );

        let identified = SendRequest {
            send_id: Some(SendId::new("timer-1").expect("test send id is valid")),
            event: Event {
                send_id: Some(SendId::new("timer-1").expect("test send id is valid")),
                ..event
            },
            target: None,
            processor_type: Some("scxml".into()),
            delay_ms: 1,
            message: scxml_types::SendMessage::Empty,
        };
        assert!(ScxmlIoProcessor.prepare(identified).is_ok());
    }

    #[test]
    fn send_request_rejects_mismatched_event_identity() {
        let request = SendRequest {
            send_id: Some(SendId::new("request-id").expect("test send id is valid")),
            event: Event {
                send_id: Some(SendId::new("event-id").expect("test send id is valid")),
                ..Event::external(EventName::new("ping").expect("test event name is valid"))
            },
            target: None,
            processor_type: Some("scxml".into()),
            delay_ms: 1,
            message: scxml_types::SendMessage::Empty,
        };
        assert_eq!(
            ScxmlIoProcessor.prepare(request.clone()),
            Err(ProcessorError::MismatchedSendId)
        );
        // SCXML C.1: a delayed send without an authored id keeps its
        // generated id on the request while the event carries none.
        let unnamed = SendRequest {
            event: Event::external(EventName::new("ping").expect("test event name is valid")),
            ..request
        };
        assert!(ScxmlIoProcessor.prepare(unnamed).is_ok());
    }

    #[test]
    fn native_control_and_failure_paths_emit_bounded_events() {
        let recorder = Recorder::default();
        let _guard = install_recorder(recorder.clone());
        let event = Event::external(EventName::new("ping").expect("test event name is valid"));

        let accepted = SendRequest {
            send_id: None,
            event: event.clone(),
            target: None,
            processor_type: Some("scxml".into()),
            delay_ms: 0,
            message: scxml_types::SendMessage::Empty,
        };
        ScxmlIoProcessor
            .prepare(accepted)
            .expect("valid send request must be accepted");

        let rejected = SendRequest {
            send_id: None,
            event,
            target: None,
            processor_type: Some("other".into()),
            delay_ms: 0,
            message: scxml_types::SendMessage::Empty,
        };
        assert_eq!(
            ScxmlIoProcessor.prepare(rejected),
            Err(ProcessorError::WrongProcessorType)
        );
        let rendered = ProcessorError::WrongProcessorType.to_string();
        assert!(!rendered.is_empty(), "bounded failure must render");

        let observations = recorder.observations();
        assert!(observations.iter().any(|observation| {
            observation.operation == Operation::IoProcessorPrepare
                && observation.events == vec![SignalEvent::IoProcessorSendAccepted]
        }));
        assert!(observations.iter().any(|observation| {
            observation.operation == Operation::IoProcessorPrepare
                && observation.error == Some(ErrorCategory::InvalidInput)
                && observation.events == vec![SignalEvent::IoProcessorSendRejected]
        }));
        assert!(
            observations
                .iter()
                .all(|observation| observation.operation == Operation::IoProcessorPrepare),
            "only the prepare boundary emits; classification and formatting run inside it"
        );
    }

    /// `dispatch` is its own boundary: it records `IoProcessorDispatch`,
    /// never the `prepare` label.
    #[test]
    fn dispatch_records_its_own_operation_label() {
        let recorder = Recorder::default();
        let _guard = install_recorder(recorder.clone());
        let nobody = |_: &str| false;
        let root = SessionAddress {
            session_id: "root",
            parent_session_id: None,
            is_live_invocation: &nobody,
            invocation_of_session: &|_| None,
        };
        ScxmlIoProcessor
            .dispatch(immediate(None, scxml_types::SendMessage::Empty), &root)
            .expect("a local send is dispatched");
        assert_eq!(
            ScxmlIoProcessor
                .dispatch(
                    immediate(Some("#_internal"), scxml_types::SendMessage::Empty),
                    &root
                )
                .err(),
            Some(ProcessorError::InvalidTarget)
        );
        let observations = recorder.observations();
        assert_eq!(observations.len(), 2, "{observations:?}");
        assert_eq!(
            observations[0].events,
            vec![SignalEvent::IoProcessorSendAccepted]
        );
        assert_eq!(observations[1].error, Some(ErrorCategory::InvalidInput));
        assert!(
            observations
                .iter()
                .all(|observation| observation.operation == Operation::IoProcessorDispatch),
            "{observations:?}"
        );
    }
}
