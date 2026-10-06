//! Bounded composition of the scaffolded Clock, I/O, and Machine components.

use super::{
    ArtifactError, ArtifactKind, AuthenticatedComponent, ComponentMeasurement, HostError,
    LifecycleLease, TelemetryProviders, WasmtimeHostConfig, component_engine, record_store_limits,
};
use crate::call_budget::CallBudget;
use crate::guest_spans::{GuestSpanError, GuestSpanTable};
use crate::memory_growth::MeteredLimits;
use crate::{clock_bindings, io_bindings};
use opentelemetry::Context;
use opentelemetry::trace::{Span as _, TraceContextExt as _, Tracer as _, TracerProvider as _};
use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, TraceContext};
use std::sync::Arc;
use wasmtime::component::{HasSelf, Linker};

const MAX_COMPONENTS: usize = 3;

/// Exact deny-by-default component artifacts participating in the proof graph.
///
/// Bytes are borrowed for bounded validation and compilation only. Every artifact
/// must implement its canonical versioned WIT world and import no interface that
/// the narrow linker does not provide.
#[derive(Clone, Copy)]
pub struct CapabilityGraphComponents<'a> {
    /// Deterministic `stateforward:scxml-clock/clock` component bytes.
    pub clock: &'a AuthenticatedComponent,
    /// Clock-backed `stateforward:scxml/io-processor` component bytes.
    pub io_processor: &'a AuthenticatedComponent,
    /// Generic-engine `stateforward:scxml/machine` component bytes.
    pub machine: &'a AuthenticatedComponent,
    /// How `machine` obtains its Machine IR (runtime `ir-source` bytes for the
    /// generic machine, or packaged).
    pub machine_ir: super::MachineIrSource<'a>,
}

/// Observable output from one representative bounded capability lifecycle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityGraphExecution {
    /// Caller-owned send identifier returned after deterministic due delivery.
    pub delivered_send_id: String,
    /// Machine status after its engine-owned start operation.
    pub machine_status: super::ComponentStatus,
    /// Total remaining deterministic fuel across the three component stores.
    pub remaining_fuel: u64,
}

/// Store state shared by every capability-graph component store (and the
/// standalone `BasicHTTP` processor store in [`crate::basichttp`]).
pub(crate) struct CommonStoreData {
    pub(crate) limits: MeteredLimits,
    pub(crate) providers: TelemetryProviders,
    execution_context: Context,
    guest_spans: GuestSpanTable,
    telemetry_failure: Option<HostError>,
    _store_lease: LifecycleLease,
}

pub(crate) struct ClockStoreData {
    pub(crate) common: CommonStoreData,
}

pub(crate) struct IoStoreData {
    pub(crate) common: CommonStoreData,
    pub(crate) clock: ClockRuntime,
    last_clock_now: u64,
}

pub(crate) struct ClockRuntime {
    pub(crate) store: wasmtime::Store<ClockStoreData>,
    pub(crate) bindings: clock_bindings::Clock,
}

/// Instantiates the exact virtualized components and executes one bounded
/// schedule/delivery lifecycle.
///
/// This host function never parses SCXML or selects transitions. Machine
/// stepping remains inside `scxml-machine`/`scxml-engine`; the host only owns
/// canonical capability forwarding and component resource lifecycles.
///
/// # Errors
///
/// Rejects empty or oversized artifacts, non-canonical imports/exports,
/// invalid host limits, fuel/resource exhaustion, and typed component failures.
/// All stores and resources are dropped before an error is returned.
pub fn execute_capability_graph(
    components: CapabilityGraphComponents<'_>,
    config: &WasmtimeHostConfig,
    providers: &TelemetryProviders,
) -> Result<CapabilityGraphExecution, HostError> {
    let parent = crate::guest_spans::span_parent(&Context::current());
    let boundary = scxml_telemetry::observe(
        Module::HostWasmtime,
        Operation::HostWasmtimeCapabilityGraphExecute,
    );
    let _lease = providers.acquire_span()?;
    let tracer = providers.tracer.tracer("scxml-host-wasmtime");
    let span = tracer.start_with_context("capability-graph-execute", &parent);
    let context = parent.with_span(span);
    let guard = context.clone().attach();
    let result = execute_capability_graph_inner(components, config, providers);
    if result.is_ok() {
        boundary.event(SignalEvent::TransformationCompleted);
        context
            .span()
            .set_attribute(opentelemetry::KeyValue::new("scxml.outcome", "ok"));
    } else {
        boundary.event(SignalEvent::InputRejected);
        boundary.fail(ErrorCategory::Platform);
        context
            .span()
            .set_status(opentelemetry::trace::Status::error("failed"));
        context
            .span()
            .set_attribute(opentelemetry::KeyValue::new("scxml.outcome", "failed"));
    }
    drop(guard);
    context.span().end();
    result
}

fn execute_capability_graph_inner(
    components: CapabilityGraphComponents<'_>,
    config: &WasmtimeHostConfig,
    providers: &TelemetryProviders,
) -> Result<CapabilityGraphExecution, HostError> {
    config.validate()?;
    validate_component(components.clock, ArtifactKind::Clock, config)?;
    validate_component(
        components.io_processor,
        ArtifactKind::IoProcessorScxml,
        config,
    )?;
    validate_component(components.machine, ArtifactKind::Machine, config)?;
    let total_fuel = config
        .store
        .fuel
        .checked_mul(u64::try_from(MAX_COMPONENTS).map_err(|_| HostError::UnboundedExecution)?)
        .ok_or(HostError::UnboundedExecution)?;
    providers.record_component_measurement(ComponentMeasurement::Fuel, total_fuel);

    let engine = component_engine(config)?;
    let clock = instantiate_clock(&engine, components.clock, config, providers)?;
    let mut io = instantiate_io(&engine, components.io_processor, config, providers, clock)?;

    let delivered_send_id = run_delayed_delivery(&mut io)?;
    let machine = super::component::execute_graph_machine(
        components.machine,
        components.machine_ir,
        config,
        providers,
    )?;
    let io_fuel = io.store.get_fuel()?;
    let clock_fuel = io.store.data().clock.store.get_fuel()?;
    let remaining_fuel = io_fuel
        .checked_add(clock_fuel)
        .and_then(|fuel| fuel.checked_add(machine.remaining_fuel))
        .ok_or(HostError::UnboundedExecution)?;
    providers.record_component_measurement(ComponentMeasurement::Fuel, remaining_fuel);
    Ok(CapabilityGraphExecution {
        delivered_send_id,
        machine_status: machine.status,
        remaining_fuel,
    })
}

pub(crate) fn validate_component(
    component: &AuthenticatedComponent,
    kind: ArtifactKind,
    config: &WasmtimeHostConfig,
) -> Result<(), HostError> {
    if component.kind() != kind {
        Err(ArtifactError::ArtifactKindMismatch.into())
    } else if component.bytes().len() > config.component_bytes {
        Err(ArtifactError::ArtifactTooLarge.into())
    } else {
        Ok(())
    }
}

/// The common store data of one component store, its memory growth
/// charged to `call_budget` (the call budget it shares) or, without one,
/// to a budget of its own of the configured fuel per call.
pub(crate) fn common_store_data(
    config: &WasmtimeHostConfig,
    providers: &TelemetryProviders,
    call_budget: Option<&Arc<CallBudget>>,
) -> Result<CommonStoreData, HostError> {
    let limits = wasmtime::StoreLimitsBuilder::new()
        .memory_size(config.store.memory_bytes)
        .table_elements(config.store.table_elements)
        .instances(config.store.instances)
        .tables(config.store.tables)
        .memories(config.store.memories)
        .trap_on_grow_failure(true)
        .build();
    Ok(CommonStoreData {
        limits: MeteredLimits::metered(limits, call_budget, config.store.fuel),
        providers: providers.clone(),
        execution_context: Context::current(),
        guest_spans: GuestSpanTable::new(),
        telemetry_failure: None,
        _store_lease: providers.acquire_store()?,
    })
}

fn instantiate_clock(
    engine: &wasmtime::Engine,
    component: &AuthenticatedComponent,
    config: &WasmtimeHostConfig,
    providers: &TelemetryProviders,
) -> Result<ClockRuntime, HostError> {
    let component = wasmtime::component::Component::from_binary(engine, component.bytes())?;
    let linker = clock_linker(engine)?;
    instantiate_clock_compiled(engine, &component, &linker, config, providers, None)
}

/// Builds the linker every clock component store of `engine` uses.
pub(crate) fn clock_linker(engine: &wasmtime::Engine) -> Result<Linker<ClockStoreData>, HostError> {
    let mut linker = Linker::<ClockStoreData>::new(engine);
    clock_bindings::Clock::add_to_linker::<_, HasSelf<ClockStoreData>>(&mut linker, |state| state)?;
    Ok(linker)
}

/// Builds the linker every SCXML I/O processor store of `engine` uses.
pub(crate) fn io_linker(engine: &wasmtime::Engine) -> Result<Linker<IoStoreData>, HostError> {
    let mut linker = Linker::<IoStoreData>::new(engine);
    io_bindings::IoProcessor::add_to_linker::<_, HasSelf<IoStoreData>>(&mut linker, |state| state)?;
    Ok(linker)
}

/// Instantiates one precompiled clock component in a fresh bounded store.
pub(crate) fn instantiate_clock_compiled(
    engine: &wasmtime::Engine,
    component: &wasmtime::component::Component,
    linker: &Linker<ClockStoreData>,
    config: &WasmtimeHostConfig,
    providers: &TelemetryProviders,
    call_budget: Option<&Arc<CallBudget>>,
) -> Result<ClockRuntime, HostError> {
    let mut store = wasmtime::Store::new(
        engine,
        ClockStoreData {
            common: common_store_data(config, providers, call_budget)?,
        },
    );
    store.limiter(|state| &mut state.common.limits);
    let budget = Arc::clone(store.data().common.limits.budget());
    budget.share(&mut store);
    store.set_fuel(config.store.fuel)?;
    record_store_limits(&config.store);
    let bindings = clock_bindings::Clock::instantiate(&mut store, component, linker)?;
    Ok(ClockRuntime { store, bindings })
}

fn instantiate_io(
    engine: &wasmtime::Engine,
    component: &AuthenticatedComponent,
    config: &WasmtimeHostConfig,
    providers: &TelemetryProviders,
    clock: ClockRuntime,
) -> Result<IoRuntime, HostError> {
    let component = wasmtime::component::Component::from_binary(engine, component.bytes())?;
    let linker = io_linker(engine)?;
    instantiate_io_compiled(engine, &component, &linker, config, providers, clock, None)
}

/// Instantiates one precompiled SCXML I/O processor linked to `clock` in a
/// fresh bounded store.
pub(crate) fn instantiate_io_compiled(
    engine: &wasmtime::Engine,
    component: &wasmtime::component::Component,
    linker: &Linker<IoStoreData>,
    config: &WasmtimeHostConfig,
    providers: &TelemetryProviders,
    mut clock: ClockRuntime,
    call_budget: Option<&Arc<CallBudget>>,
) -> Result<IoRuntime, HostError> {
    let last_clock_now = clock.now()?;
    let mut store = wasmtime::Store::new(
        engine,
        IoStoreData {
            common: common_store_data(config, providers, call_budget)?,
            clock,
            last_clock_now,
        },
    );
    store.limiter(|state| &mut state.common.limits);
    let budget = Arc::clone(store.data().common.limits.budget());
    budget.share(&mut store);
    store.set_fuel(config.store.fuel)?;
    record_store_limits(&config.store);
    let bindings = io_bindings::IoProcessor::instantiate(&mut store, component, linker)?;
    Ok(IoRuntime { store, bindings })
}

pub(crate) struct IoRuntime {
    pub(crate) store: wasmtime::Store<IoStoreData>,
    pub(crate) bindings: io_bindings::IoProcessor,
}

fn run_delayed_delivery(io: &mut IoRuntime) -> Result<String, HostError> {
    use crate::bindings::stateforward::scxml::types::{
        DataValue, Event, EventType, SendMessage, SendRequest, Sender,
    };
    use io_bindings::exports::stateforward::scxml::io_processor_api::{
        DelayedSend, DispatchDecision,
    };

    let send_id = "scheduled-0".to_owned();
    let trace = crate::bindings::stateforward::scxml::types::TraceContext {
        traceparent: "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".into(),
        tracestate: None,
        baggage: None,
        baggage_policy: crate::bindings::stateforward::scxml::types::BaggagePolicy::Drop,
    };
    let request = SendRequest {
        send_id: Some(send_id.clone()),
        event: Event {
            name: "delivered".into(),
            event_type: EventType::External,
            send_id: Some(send_id.clone()),
            origin: None,
            origin_type: None,
            invoke_id: None,
            data: DataValue::Absent,
            raw: None,
            trace_context: Some(trace),
        },
        target: None,
        processor_type: Some("scxml".into()),
        delay_ms: 7,
        message: SendMessage::Empty,
    };
    let sender = Sender {
        session_id: "representative".into(),
        parent_session_id: None,
        invocations: Vec::new(),
    };
    let api = io.bindings.stateforward_scxml_io_processor_api();
    let decision = api
        .call_dispatch(&mut io.store, &request, &sender)?
        .map_err(|error| io_component_failure("I/O dispatch", &error))?;
    if !matches!(decision, DispatchDecision::Scheduled) {
        return Err(HostError::Component(
            "I/O dispatch did not schedule the delayed request".into(),
        ));
    }
    io.store.data_mut().take_failures()?;
    io.store.data_mut().clock.advance(7)?;
    let due = api
        .call_drain_due(&mut io.store, 1)?
        .map_err(|error| io_component_failure("I/O drain", &error))?;
    io.store.data_mut().take_failures()?;
    match (due.delivered.as_slice(), due.rejected.as_slice()) {
        ([DelayedSend::Scxml(delivered)], [])
            if delivered.send_id.as_deref() == Some(send_id.as_str())
                && delivered.event.send_id.as_deref() == Some(send_id.as_str()) =>
        {
            Ok(send_id)
        }
        _ => Err(HostError::Component(
            "I/O delivery did not return the scheduled request".into(),
        )),
    }
}

impl ClockRuntime {
    pub(crate) fn now(&mut self) -> Result<u64, HostError> {
        let result = self
            .bindings
            .stateforward_scxml_clock_clock_api()
            .call_now(&mut self.store)
            .map_err(HostError::from)?;
        self.store.data_mut().common.take_telemetry_failure()?;
        Ok(result)
    }

    pub(crate) fn advance(&mut self, delta_ms: u64) -> Result<(), HostError> {
        let result = self
            .bindings
            .stateforward_scxml_clock_clock_api()
            .call_advance(&mut self.store, delta_ms)?
            .map(|_| ())
            .map_err(|_| HostError::Component("clock advance rejected".into()));
        self.store.data_mut().common.take_telemetry_failure()?;
        result
    }
}

impl IoStoreData {
    pub(crate) fn take_failures(&mut self) -> Result<(), HostError> {
        self.common.take_telemetry_failure()?;
        self.clock.store.data_mut().common.take_telemetry_failure()
    }
}

fn io_component_failure(
    boundary: &'static str,
    error: &crate::bindings::stateforward::scxml::types::ComponentError,
) -> HostError {
    use crate::bindings::stateforward::scxml::types::{ErrorCode, ErrorVersion};

    let code = if error.version != ErrorVersion::V1 {
        "unknown-error-version"
    } else if error
        .detail
        .as_ref()
        .is_some_and(|detail| detail.len() > 160)
    {
        "invalid-error-detail"
    } else {
        match error.code {
            ErrorCode::InvalidInput => "invalid-input",
            ErrorCode::DuplicateId => "duplicate-id",
            ErrorCode::UnknownId => "unknown-id",
            ErrorCode::CapacityExhausted => "capacity-exhausted",
            ErrorCode::PayloadTooLarge => "payload-too-large",
            ErrorCode::UnsupportedVersion => "unsupported-version",
            ErrorCode::Protocol => "protocol",
            ErrorCode::Execution => "execution",
            ErrorCode::Unavailable => "unavailable",
            ErrorCode::Platform => "platform",
        }
    };
    HostError::Component(format!("{boundary} failed: {code}"))
}

impl clock_bindings::stateforward::scxml_clock::types::Host for ClockStoreData {}

impl clock_bindings::stateforward::scxml_clock::telemetry::Host for ClockStoreData {
    fn observe(
        &mut self,
        operation: clock_bindings::stateforward::scxml_clock::telemetry::Operation,
        outcome: clock_bindings::stateforward::scxml_clock::telemetry::Outcome,
        measurement: Option<clock_bindings::stateforward::scxml_clock::telemetry::Measurement>,
        context: Option<clock_bindings::stateforward::scxml_clock::types::TraceContext>,
    ) {
        record_clock_signal(
            &mut self.common,
            operation,
            Some(outcome),
            None,
            measurement,
            context.as_ref(),
        );
    }

    fn event(
        &mut self,
        operation: clock_bindings::stateforward::scxml_clock::telemetry::Operation,
        event: clock_bindings::stateforward::scxml_clock::telemetry::SignalEvent,
        context: Option<clock_bindings::stateforward::scxml_clock::types::TraceContext>,
    ) {
        record_clock_signal(
            &mut self.common,
            operation,
            None,
            Some(event),
            None,
            context.as_ref(),
        );
    }
}

impl crate::bindings::stateforward::scxml::types::Host for IoStoreData {}
impl io_bindings::stateforward::scxml_clock::types::Host for IoStoreData {}

impl io_bindings::stateforward::scxml_clock::clock_api::Host for IoStoreData {
    fn now(&mut self) -> u64 {
        let result = self
            .clock
            .bindings
            .stateforward_scxml_clock_clock_api()
            .call_now(&mut self.clock.store);
        record_clock_now_result(&mut self.common, &mut self.last_clock_now, result)
    }

    fn next_deadline(&mut self) -> Option<u64> {
        // Pure clock query: a trap is latched as a host failure (reported
        // after the processor call returns) and answers no deadline.
        match self
            .clock
            .bindings
            .stateforward_scxml_clock_clock_api()
            .call_next_deadline(&mut self.clock.store)
        {
            Ok(deadline) => deadline,
            Err(error) => {
                self.common.latch_telemetry_failure(HostError::from(error));
                None
            }
        }
    }

    fn snapshot(&mut self) -> io_bindings::stateforward::scxml_clock::clock_api::ClockSnapshot {
        // Read-only clock query forwarded to the linked clock: a trap is
        // latched as a host failure (reported after the processor call
        // returns) and answers an empty image.
        match self
            .clock
            .bindings
            .stateforward_scxml_clock_clock_api()
            .call_snapshot(&mut self.clock.store)
        {
            Ok(snapshot) => io_bindings::stateforward::scxml_clock::clock_api::ClockSnapshot {
                now_ms: snapshot.now_ms,
                timers: snapshot
                    .timers
                    .into_iter()
                    .map(
                        |timer| io_bindings::stateforward::scxml_clock::clock_api::Timer {
                            id: timer.id,
                            payload: timer.payload,
                            deadline_ms: timer.deadline_ms,
                        },
                    )
                    .collect(),
            },
            Err(error) => {
                self.common.latch_telemetry_failure(HostError::from(error));
                io_bindings::stateforward::scxml_clock::clock_api::ClockSnapshot {
                    now_ms: 0,
                    timers: Vec::new(),
                }
            }
        }
    }

    fn schedule(
        &mut self,
        id: String,
        payload: Vec<u8>,
        delay_ms: u64,
        context: Option<io_bindings::stateforward::scxml_clock::types::TraceContext>,
    ) -> Result<
        io_bindings::stateforward::scxml_clock::types::Scheduled,
        io_bindings::stateforward::scxml_clock::types::ClockError,
    > {
        let context = context.map(clock_context_from_io);
        self.clock
            .bindings
            .stateforward_scxml_clock_clock_api()
            .call_schedule(
                &mut self.clock.store,
                &id,
                &payload,
                delay_ms,
                context.as_ref(),
            )
            .map_err(|_| io_bindings::stateforward::scxml_clock::types::ClockError::InvalidLimit)?
            .map(
                |scheduled| io_bindings::stateforward::scxml_clock::types::Scheduled {
                    deadline_ms: scheduled.deadline_ms,
                    sequence: scheduled.sequence,
                },
            )
            .map_err(clock_error_to_io)
    }

    fn cancel(
        &mut self,
        id: String,
        context: Option<io_bindings::stateforward::scxml_clock::types::TraceContext>,
    ) -> Result<bool, io_bindings::stateforward::scxml_clock::types::ClockError> {
        let context = context.map(clock_context_from_io);
        self.clock
            .bindings
            .stateforward_scxml_clock_clock_api()
            .call_cancel(&mut self.clock.store, &id, context.as_ref())
            .map_err(|_| io_bindings::stateforward::scxml_clock::types::ClockError::InvalidLimit)?
            .map_err(clock_error_to_io)
    }

    fn drain_due(
        &mut self,
        max_items: u32,
    ) -> Result<
        Vec<io_bindings::stateforward::scxml_clock::types::DueItem>,
        io_bindings::stateforward::scxml_clock::types::ClockError,
    > {
        self.clock
            .bindings
            .stateforward_scxml_clock_clock_api()
            .call_drain_due(&mut self.clock.store, max_items)
            .map_err(|_| io_bindings::stateforward::scxml_clock::types::ClockError::InvalidLimit)?
            .map(|items| {
                items
                    .into_iter()
                    .map(
                        |item| io_bindings::stateforward::scxml_clock::types::DueItem {
                            id: item.id,
                            payload: item.payload,
                            deadline_ms: item.deadline_ms,
                            sequence: item.sequence,
                        },
                    )
                    .collect()
            })
            .map_err(clock_error_to_io)
    }

    fn advance(
        &mut self,
        delta_ms: u64,
    ) -> Result<u64, io_bindings::stateforward::scxml_clock::types::ClockError> {
        self.clock
            .bindings
            .stateforward_scxml_clock_clock_api()
            .call_advance(&mut self.clock.store, delta_ms)
            .map_err(|_| io_bindings::stateforward::scxml_clock::types::ClockError::InvalidLimit)?
            .map_err(clock_error_to_io)
    }
}

impl crate::bindings::stateforward::scxml::telemetry::Host for IoStoreData {
    fn start(
        &mut self,
        operation: crate::bindings::stateforward::scxml::telemetry::Operation,
        parent: Option<crate::bindings::stateforward::scxml::types::TraceContext>,
    ) -> (
        u64,
        Option<crate::bindings::stateforward::scxml::types::TraceContext>,
    ) {
        start_guest_span(
            &mut self.common,
            io_operation_name(operation),
            parent.as_ref().map(io_trace_context).as_ref(),
        )
        .and_then(|(handle, context)| {
            context
                .map(trace_context_to_io)
                .transpose()
                .map(|context| (handle, context))
        })
        .unwrap_or_else(|error| {
            self.common.latch_telemetry_failure(error);
            (0, None)
        })
    }

    fn end(
        &mut self,
        span: u64,
        outcome: crate::bindings::stateforward::scxml::telemetry::Outcome,
    ) {
        if let Err(error) = end_guest_span(&mut self.common, span, io_outcome_name(outcome)) {
            self.common.latch_telemetry_failure(error);
        }
    }

    fn observe(
        &mut self,
        operation: crate::bindings::stateforward::scxml::telemetry::Operation,
        outcome: crate::bindings::stateforward::scxml::telemetry::Outcome,
        measurement: Option<crate::bindings::stateforward::scxml::telemetry::Measurement>,
        context: Option<crate::bindings::stateforward::scxml::types::TraceContext>,
    ) {
        record_scxml_measurement(
            &self.common,
            measurement.map(|value| (value.kind, value.value)),
        );
        if let Err(error) = record_scxml_observation(
            &mut self.common,
            io_operation_name(operation),
            Some(io_outcome_name(outcome)),
            None,
            context.as_ref().map(io_trace_context).as_ref(),
        ) {
            self.common.latch_telemetry_failure(error);
        }
    }

    fn event(
        &mut self,
        operation: crate::bindings::stateforward::scxml::telemetry::Operation,
        event: crate::bindings::stateforward::scxml::telemetry::SignalEvent,
        context: Option<crate::bindings::stateforward::scxml::types::TraceContext>,
    ) {
        if let Err(error) = record_scxml_observation(
            &mut self.common,
            io_operation_name(operation),
            None,
            Some(io_event_name(event)),
            context.as_ref().map(io_trace_context).as_ref(),
        ) {
            self.common.latch_telemetry_failure(error);
        }
    }
}

fn clock_context_from_io(
    context: io_bindings::stateforward::scxml_clock::types::TraceContext,
) -> clock_bindings::stateforward::scxml_clock::types::TraceContext {
    clock_bindings::stateforward::scxml_clock::types::TraceContext {
        traceparent: context.traceparent,
        tracestate: context.tracestate,
        baggage: context.baggage,
        baggage_policy: match context.baggage_policy {
            io_bindings::stateforward::scxml_clock::types::BaggagePolicy::Drop => {
                clock_bindings::stateforward::scxml_clock::types::BaggagePolicy::Drop
            }
            io_bindings::stateforward::scxml_clock::types::BaggagePolicy::Propagate => {
                clock_bindings::stateforward::scxml_clock::types::BaggagePolicy::Propagate
            }
        },
    }
}

fn clock_error_to_io(
    error: clock_bindings::stateforward::scxml_clock::types::ClockError,
) -> io_bindings::stateforward::scxml_clock::types::ClockError {
    use clock_bindings::stateforward::scxml_clock::types::ClockError as From;
    use io_bindings::stateforward::scxml_clock::types::ClockError as To;

    match error {
        From::InvalidId => To::InvalidId,
        From::InvalidDuration => To::InvalidDuration,
        From::PayloadTooLarge => To::PayloadTooLarge,
        From::DuplicateId => To::DuplicateId,
        From::CapacityExhausted => To::CapacityExhausted,
        From::Overflow => To::Overflow,
        From::InvalidLimit => To::InvalidLimit,
        From::InvalidContext => To::InvalidContext,
    }
}

fn record_clock_signal(
    common: &mut CommonStoreData,
    operation: clock_bindings::stateforward::scxml_clock::telemetry::Operation,
    outcome: Option<clock_bindings::stateforward::scxml_clock::telemetry::Outcome>,
    event: Option<clock_bindings::stateforward::scxml_clock::telemetry::SignalEvent>,
    measurement: Option<clock_bindings::stateforward::scxml_clock::telemetry::Measurement>,
    context: Option<&clock_bindings::stateforward::scxml_clock::types::TraceContext>,
) {
    if let Some(measurement) = measurement {
        common.providers.record_component_measurement(
            match measurement.kind {
                clock_bindings::stateforward::scxml_clock::telemetry::MeasurementKind::QueueDepth => {
                    ComponentMeasurement::QueueDepth
                }
            },
            measurement.value,
        );
    }
    if let Err(error) = record_scxml_observation(
        common,
        clock_operation_name(operation),
        outcome.map(clock_outcome_name),
        event.map(clock_event_name),
        context.map(clock_trace_context).as_ref(),
    ) {
        common.latch_telemetry_failure(error);
    }
}

fn io_trace_context(
    context: &crate::bindings::stateforward::scxml::types::TraceContext,
) -> TraceContext {
    TraceContext {
        traceparent: Some(context.traceparent.clone()),
        tracestate: context.tracestate.clone(),
    }
}

fn clock_trace_context(
    context: &clock_bindings::stateforward::scxml_clock::types::TraceContext,
) -> TraceContext {
    TraceContext {
        traceparent: Some(context.traceparent.clone()),
        tracestate: context.tracestate.clone(),
    }
}

fn trace_context_to_io(
    context: TraceContext,
) -> Result<crate::bindings::stateforward::scxml::types::TraceContext, HostError> {
    let traceparent = context
        .traceparent
        .ok_or_else(|| HostError::Component("telemetry propagation unavailable".into()))?;
    let converted = crate::bindings::stateforward::scxml::types::TraceContext {
        traceparent,
        tracestate: context.tracestate,
        baggage: None,
        baggage_policy: crate::bindings::stateforward::scxml::types::BaggagePolicy::Drop,
    };
    Ok(converted)
}

impl CommonStoreData {
    pub(crate) fn latch_telemetry_failure(&mut self, error: HostError) {
        if self.telemetry_failure.is_none() {
            self.telemetry_failure = Some(error);
        }
    }

    pub(crate) fn take_telemetry_failure(&mut self) -> Result<(), HostError> {
        if let Some(error) = self.telemetry_failure.take() {
            Err(error)
        } else {
            Ok(())
        }
    }
}

fn record_clock_now_result(
    common: &mut CommonStoreData,
    last_clock_now: &mut u64,
    result: wasmtime::Result<u64>,
) -> u64 {
    match result {
        Ok(now) => {
            *last_clock_now = now;
            now
        }
        Err(error) => {
            common.latch_telemetry_failure(HostError::from(error));
            *last_clock_now
        }
    }
}

fn start_guest_span(
    common: &mut CommonStoreData,
    operation: &'static str,
    parent: Option<&TraceContext>,
) -> Result<(u64, Option<TraceContext>), HostError> {
    let parent = match parent {
        Some(carrier) => {
            scxml_telemetry::extract_trace_context(carrier).map_err(HostError::from)?
        }
        None => common.execution_context.clone(),
    };
    common
        .guest_spans
        .start(
            &common.providers,
            "scxml-capability-guest",
            "capability-component",
            operation,
            &parent,
        )
        .map(|(handle, propagated)| (handle, Some(propagated)))
        .map_err(GuestSpanError::into_host_error)
}

fn end_guest_span(
    common: &mut CommonStoreData,
    handle: u64,
    outcome: &'static str,
) -> Result<(), HostError> {
    common
        .guest_spans
        .end(handle, outcome)
        .map_err(GuestSpanError::into_host_error)
}

pub(crate) fn record_scxml_observation(
    common: &mut CommonStoreData,
    operation: &'static str,
    outcome: Option<&'static str>,
    event: Option<&'static str>,
    parent: Option<&TraceContext>,
) -> Result<(), HostError> {
    let _lease = common.providers.acquire_span()?;
    let parent = match parent {
        Some(carrier) => {
            scxml_telemetry::extract_trace_context(carrier).map_err(HostError::from)?
        }
        None => crate::guest_spans::span_parent(&common.execution_context),
    };
    let tracer = common.providers.tracer.tracer("scxml-capability-guest");
    let mut span = tracer.start_with_context(operation, &parent);
    span.set_attribute(opentelemetry::KeyValue::new(
        "scxml.module",
        "capability-component",
    ));
    span.set_attribute(opentelemetry::KeyValue::new("scxml.operation", operation));
    if let Some(outcome) = outcome {
        span.set_attribute(opentelemetry::KeyValue::new("scxml.outcome", outcome));
    }
    if let Some(event) = event {
        span.add_event(event, Vec::new());
    }
    span.end();
    Ok(())
}

fn record_scxml_measurement(
    common: &CommonStoreData,
    measurement: Option<(
        crate::bindings::stateforward::scxml::telemetry::MeasurementKind,
        u64,
    )>,
) {
    if let Some((kind, value)) = measurement {
        common.providers.record_component_measurement(
            match kind {
                crate::bindings::stateforward::scxml::telemetry::MeasurementKind::Effects => {
                    ComponentMeasurement::Effects
                }
                crate::bindings::stateforward::scxml::telemetry::MeasurementKind::InputBytes => {
                    ComponentMeasurement::InputBytes
                }
                crate::bindings::stateforward::scxml::telemetry::MeasurementKind::SnapshotBytes => {
                    ComponentMeasurement::SnapshotBytes
                }
                crate::bindings::stateforward::scxml::telemetry::MeasurementKind::QueueDepth => {
                    ComponentMeasurement::QueueDepth
                }
                crate::bindings::stateforward::scxml::telemetry::MeasurementKind::Fuel => {
                    ComponentMeasurement::Fuel
                }
            },
            value,
        );
    }
}

const fn clock_operation_name(
    operation: clock_bindings::stateforward::scxml_clock::telemetry::Operation,
) -> &'static str {
    use clock_bindings::stateforward::scxml_clock::telemetry::Operation;
    match operation {
        Operation::Now => "clock-now",
        Operation::Schedule => "clock-schedule",
        Operation::Cancel => "clock-cancel",
        Operation::DrainDue => "clock-drain-due",
        Operation::Advance => "clock-advance",
    }
}

const fn clock_outcome_name(
    outcome: clock_bindings::stateforward::scxml_clock::telemetry::Outcome,
) -> &'static str {
    use clock_bindings::stateforward::scxml_clock::telemetry::Outcome;
    match outcome {
        Outcome::Ok => "ok",
        Outcome::Rejected => "rejected",
    }
}

const fn clock_event_name(
    event: clock_bindings::stateforward::scxml_clock::telemetry::SignalEvent,
) -> &'static str {
    use clock_bindings::stateforward::scxml_clock::telemetry::SignalEvent;
    match event {
        SignalEvent::Scheduled => "scheduled",
        SignalEvent::Cancelled => "cancelled",
        SignalEvent::DueDrained => "due-drained",
        SignalEvent::Advanced => "advanced",
        SignalEvent::Rejected => "rejected",
    }
}

const fn io_operation_name(
    operation: crate::bindings::stateforward::scxml::telemetry::Operation,
) -> &'static str {
    use crate::bindings::stateforward::scxml::telemetry::Operation;
    match operation {
        Operation::Initialize => "initialize",
        Operation::EvaluateCondition => "evaluate-condition",
        Operation::EvaluateValue => "evaluate-value",
        Operation::Assign => "assign",
        Operation::Execute => "execute",
        Operation::SystemContext => "system-context",
        Operation::Start => "start",
        Operation::Send => "send",
        Operation::Cancel => "cancel",
        Operation::DrainDue => "drain-due",
        Operation::TypeUri => "type-uri",
        Operation::Resume => "resume",
        Operation::Shutdown => "shutdown",
        Operation::CheckSyntax => "check-syntax",
        Operation::CheckEffects => "check-effects",
        Operation::StartInvoked => "start-invoked",
        Operation::Deliver => "deliver",
        Operation::Exit => "exit",
        Operation::Dispatch => "dispatch",
        Operation::RouteDue => "route-due",
        Operation::Encode => "encode",
        Operation::Diff => "diff",
        Operation::CommunicationError => "communication-error",
        Operation::ScheduleBasichttp => "schedule-basichttp",
    }
}

const fn io_outcome_name(
    outcome: crate::bindings::stateforward::scxml::telemetry::Outcome,
) -> &'static str {
    use crate::bindings::stateforward::scxml::telemetry::Outcome;
    match outcome {
        Outcome::Ok => "ok",
        Outcome::Rejected => "rejected",
        Outcome::Failed => "failed",
    }
}

const fn io_event_name(
    event: crate::bindings::stateforward::scxml::telemetry::SignalEvent,
) -> &'static str {
    use crate::bindings::stateforward::scxml::telemetry::SignalEvent;
    match event {
        SignalEvent::TypeUriReturned => "type-uri-returned",
        SignalEvent::SendAccepted => "send-accepted",
        SignalEvent::SendRejected => "send-rejected",
        SignalEvent::CancelCompleted => "cancel-completed",
        SignalEvent::CancelRejected => "cancel-rejected",
        SignalEvent::DueDrained => "due-drained",
        SignalEvent::InputAccepted => "input-accepted",
        SignalEvent::InputRejected => "input-rejected",
        SignalEvent::TransformationCompleted => "transformation-completed",
        SignalEvent::ArtifactAccepted => "artifact-accepted",
        SignalEvent::ArtifactProduced => "artifact-produced",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_sdk::metrics::SdkMeterProvider;
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};

    fn test_common(providers: &TelemetryProviders, execution_context: Context) -> CommonStoreData {
        let mut common = common_store_data(&WasmtimeHostConfig::default(), providers, None)
            .expect("active providers grant a bounded test store");
        common.execution_context = execution_context;
        common
    }

    fn exporting_providers() -> (TelemetryProviders, InMemorySpanExporter) {
        let exporter = InMemorySpanExporter::default();
        let tracer = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        (
            TelemetryProviders::from_providers(tracer, SdkMeterProvider::builder().build()),
            exporter,
        )
    }

    #[test]
    fn concurrent_guest_spans_have_unique_handles_and_close_once() {
        let (providers, exporter) = exporting_providers();
        let tracer = providers.tracer.tracer("capability-graph-test");
        let parent_span = tracer.start_with_context("execution", &Context::new());
        let parent = Context::new().with_span(parent_span);
        let parent_trace_id = parent.span().span_context().trace_id();
        let parent_span_id = parent.span().span_context().span_id();
        let mut common = test_common(&providers, parent.clone());

        let (first, _) = start_guest_span(&mut common, "send", None)
            .expect("first guest span starts beneath execution");
        let (second, _) = start_guest_span(&mut common, "cancel", None)
            .expect("second guest span starts while first remains active");
        assert_ne!(first, second);
        end_guest_span(&mut common, second, "rejected").expect("second span closes once");
        end_guest_span(&mut common, first, "ok").expect("first span closes once");
        assert!(end_guest_span(&mut common, first, "ok").is_err());
        parent.span().end();
        providers.force_flush().expect("test spans flush");

        let spans = exporter
            .get_finished_spans()
            .expect("test spans are readable");
        for name in ["send", "cancel"] {
            let span = spans
                .iter()
                .find(|span| span.name == name)
                .expect("guest span is exported");
            assert_eq!(span.span_context.trace_id(), parent_trace_id);
            assert_eq!(span.parent_span_id, parent_span_id);
        }
    }

    #[test]
    fn malformed_trace_context_is_latched_without_root_span() {
        let (providers, exporter) = exporting_providers();
        let mut common = test_common(&providers, Context::new());
        let malformed = TraceContext {
            traceparent: Some("00-not-a-trace".into()),
            tracestate: None,
        };

        let result = record_scxml_observation(
            &mut common,
            "send",
            Some("rejected"),
            None,
            Some(&malformed),
        );

        assert!(matches!(result, Err(HostError::TraceContext(_))));
        providers.force_flush().expect("test spans flush");
        assert!(
            exporter
                .get_finished_spans()
                .expect("test spans are readable")
                .iter()
                .all(|span| span.name != "send")
        );
    }

    #[test]
    fn clock_now_failure_preserves_last_observed_time_and_latches_error() {
        let providers = TelemetryProviders::new();
        let mut common = test_common(&providers, Context::new());
        let mut last_observed = 41;

        let returned = record_clock_now_result(
            &mut common,
            &mut last_observed,
            Err(wasmtime::Error::msg("clock trap")),
        );

        assert_eq!(returned, 41);
        assert_eq!(last_observed, 41);
        assert!(matches!(
            common.take_telemetry_failure(),
            Err(HostError::Wasmtime(_))
        ));
    }

    #[test]
    fn capability_graph_failure_does_not_project_guest_detail() {
        use crate::bindings::stateforward::scxml::types::{
            ComponentError, ErrorCode, ErrorVersion,
        };

        let failure = ComponentError {
            version: ErrorVersion::V1,
            code: ErrorCode::UnknownId,
            detail: Some("secret-send-id".into()),
        };
        let error = io_component_failure("I/O cancel", &failure).to_string();

        assert!(error.contains("unknown-id"));
        assert!(!error.contains("secret-send-id"));
    }
}
