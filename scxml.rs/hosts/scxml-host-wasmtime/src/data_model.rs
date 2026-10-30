//! A data-model component (`stateforward:scxml/data-model`) linked as a
//! machine session's [`MachineDataModel`].
//!
//! The host stays a thin linker: every expression, assignment, and script is
//! evaluated inside the authenticated data-model component, in its own
//! bounded Wasmtime store. The host only forwards calls, answers the
//! component's `execution-host.in-state` import from the configuration the
//! machine's updates built, and maps component errors to closed
//! [`DataModelFailure`] codes.
//!
//! Contract:
//!
//! * `_event` follows SCXML 1.0 section 5.10: a call made with no current
//!   event (for example `<onentry>` executed by `start`) reaches the
//!   component with `none`, which leaves `_event` unbound; the host never
//!   fabricates an event.
//! * `initialize` and each later call run with a fresh instruction-fuel
//!   budget ([`DEFAULT_DATA_MODEL_CALL_FUEL`] unless changed with
//!   [`ComponentDataModel::with_call_fuel`]), so one call is bounded without
//!   coupling unrelated calls. The ECMAScript provider creates one guarded
//!   interpreter realm per instance, at `initialize`, and keeps it for the
//!   whole session (`components/scxml-data-model-ecmascript/src/session.rs`);
//!   the fuel its workloads take is measured where [`crate::DEFAULT_FUEL`]
//!   is sized. Under the session host this budget is only a per-call cap:
//!   every call is also charged to the calling host call's shared budget
//!   ([`crate::DEFAULT_FUEL`]). Memory, table, and instance limits come
//!   from the host's store configuration.
//! * A trap (including fuel exhaustion) leaves the instance unusable: that
//!   call and every later call fail with [`DataModelFailure::Platform`],
//!   which fails the calling machine session.
//! * The component's other `execution-host` imports (`raise`, `send`,
//!   `cancel`) answer `unavailable`: the machine owns those effects through
//!   its own capabilities. `log` is accepted and discarded.
//!
//! Telemetry: closed operation labels only (`scxml.host-wasmtime.data-model`
//! plus the guest's closed WIT operation/outcome/event enums); expressions,
//! values, state ids, and events never become attributes. Guest
//! `telemetry.start` returns the documented "unavailable" id 0.

use std::sync::Arc;

use opentelemetry::Context;
use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent};
use wasmtime::component::{HasSelf, Linker};

use crate::bindings::stateforward::scxml::telemetry;
use crate::bindings::stateforward::scxml::types::{
    BaggagePolicy as WitBaggagePolicy, ComponentError, DataInitializer as WitDataInitializer,
    ErrorCode, ErrorVersion, Event as WitEvent, EventType as WitEventType, LogLevel, SendRequest,
    TraceContext as WitTraceContext,
};
use crate::call_budget::CallBudget;
use crate::component::{GuestSignal, record_guest_signal};
use crate::memory_growth::MeteredLimits;
use crate::session::{
    DataModelFailure, MachineDataModel, native_value, wit_placement, wit_source, wit_value,
};
use crate::{
    ArtifactError, ArtifactKind, AuthenticatedComponent, HostError, InvalidLimit, LifecycleLease,
    TelemetryProviders, WasmtimeHostConfig, component_engine, record_store_limits,
};

mod data_model_bindings {
    wasmtime::component::bindgen!({
        path: "../../crates/scxml-wit/wit/scxml",
        world: "data-model",
        with: {
            "stateforward:scxml/types": crate::bindings::stateforward::scxml::types,
            "stateforward:scxml/telemetry": crate::bindings::stateforward::scxml::telemetry,
        },
    });
}

#[cfg(test)]
pub(crate) use data_model_bindings::exports::stateforward::scxml::data_model_api::Variable;
/// `data-model-api.variable-text`, for the snapshot codec parity tests.
#[cfg(test)]
pub(crate) use data_model_bindings::exports::stateforward::scxml::data_model_api::VariableText;
/// `data-model-api` syntax-check types, for the validator's
/// `expression-syntax` adapter.
pub(crate) use data_model_bindings::exports::stateforward::scxml::data_model_api::{
    DataModelSnapshot, EffectCause as DataModelEffectCause,
    EffectFinding as DataModelEffectFinding, EffectFragment as DataModelEffectFragment,
    EffectKind as DataModelEffectKind, EffectRole as DataModelEffectRole,
    EffectSpan as DataModelEffectSpan, ExternalScripts as DataModelExternalScripts,
    SyntaxIssue as DataModelSyntaxIssue, SyntaxKind as DataModelSyntaxKind, SystemValues,
    VariableValue,
};

/// Whether a snapshot reads the data model's image or only names it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DataModelRead {
    /// Name the linked data model and read its `data-model-api.snapshot`.
    Snapshot,
    /// Name the linked data model only; its export is not called.
    NameOnly,
}

/// Exported `data-model-api` of one data-model component instance.
pub(crate) type DataModelGuest =
    data_model_bindings::exports::stateforward::scxml::data_model_api::Guest;

/// Builds the linker every data-model component store of `engine` uses.
///
/// # Errors
///
/// A Wasmtime linker definition failure.
pub(crate) fn data_model_linker(
    engine: &wasmtime::Engine,
) -> Result<Linker<DataModelStoreData>, HostError> {
    let mut linker = Linker::<DataModelStoreData>::new(engine);
    data_model_bindings::DataModel::add_to_linker::<_, HasSelf<DataModelStoreData>>(
        &mut linker,
        |state| state,
    )?;
    Ok(linker)
}
use data_model_bindings::stateforward::scxml::execution_host;

/// Default instruction fuel for `initialize` and for each data-model call.
pub const DEFAULT_DATA_MODEL_CALL_FUEL: u64 = 1_000_000_000;

/// SCXML system variables bound at `initialize` (SCXML 1.0 section 5.10).
#[derive(Clone, Debug, PartialEq)]
pub struct DataModelSystemValues {
    /// `_sessionid`; a valid SCXML name.
    pub session_id: String,
    /// `_name`: the document's `name` attribute; `None` when it has none
    /// (SCXML 5.10).
    pub machine_name: Option<String>,
    /// `_ioprocessors`, a canonical JSON object value.
    pub io_processors: scxml_types::DataValue,
}

pub(crate) struct DataModelStoreData {
    limits: MeteredLimits,
    /// States `in-state` answers true for: the machine's active
    /// configuration, kept as a set because only membership is observable.
    configuration: std::collections::BTreeSet<String>,
    providers: TelemetryProviders,
    execution_context: Context,
    /// Spans the guest opened through `telemetry.start`.
    guest_spans: crate::guest_spans::GuestSpanTable,
    /// The linked session's clock: its start epoch and its logical time,
    /// which `now-ms` adds (0 unlinked).
    session_epoch_ms: u64,
    session_time: SessionTime,
    _store_lease: LifecycleLease,
}

/// The current time of one session's clock, shared between the session's
/// links (which advance it) and its data model (which reads it through
/// `execution-host.now-ms`).
pub(crate) type SessionTime = Arc<std::sync::atomic::AtomicU64>;

/// One authenticated data-model component instance used as a session's
/// data model. Single owner; calls are synchronous and never re-entrant.
pub struct ComponentDataModel {
    store: wasmtime::Store<DataModelStoreData>,
    bindings: data_model_bindings::DataModel,
    name: String,
    call_fuel: u64,
    /// What the last call spent.
    last_call: crate::CallFuel,
    lost: bool,
}

impl std::fmt::Debug for ComponentDataModel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ComponentDataModel")
            .field("name", &self.name)
            .field("lost", &self.lost)
            .finish_non_exhaustive()
    }
}

fn component_error(code: ErrorCode, detail: &'static str) -> ComponentError {
    ComponentError {
        version: ErrorVersion::V1,
        code,
        detail: Some(detail.into()),
    }
}

/// Maps a component's typed error to the closed provider failure.
fn failure(error: &ComponentError) -> DataModelFailure {
    match error.code {
        ErrorCode::InvalidInput | ErrorCode::PayloadTooLarge => DataModelFailure::InvalidInput,
        ErrorCode::Execution => DataModelFailure::Execution,
        ErrorCode::Unavailable => DataModelFailure::Unavailable,
        _ => DataModelFailure::Platform,
    }
}

/// Converts one portable event to its WIT representation.
pub(crate) fn wit_event(event: &scxml_types::Event) -> WitEvent {
    WitEvent {
        name: event.name.as_str().to_owned(),
        event_type: match event.event_type {
            scxml_types::EventType::Internal => WitEventType::Internal,
            scxml_types::EventType::External => WitEventType::External,
            scxml_types::EventType::Platform => WitEventType::Platform,
        },
        send_id: event.send_id.as_ref().map(|id| id.as_str().to_owned()),
        origin: event.origin.clone(),
        origin_type: event.origin_type.clone(),
        invoke_id: event.invoke_id.as_ref().map(|id| id.as_str().to_owned()),
        data: wit_value(event.data.clone()),
        raw: event.raw.clone(),
        trace_context: event.trace_context.as_ref().map(|context| WitTraceContext {
            traceparent: context.traceparent.clone(),
            tracestate: context.tracestate.clone(),
            baggage: context.baggage.clone(),
            baggage_policy: match context.baggage_policy {
                scxml_types::BaggagePolicy::Drop => WitBaggagePolicy::Drop,
                scxml_types::BaggagePolicy::Propagate => WitBaggagePolicy::Propagate,
            },
        }),
    }
}

impl ComponentDataModel {
    /// Instantiates `component` in a bounded store and initializes it
    /// (`data-model-api.initialize`) with the document's `<data>`
    /// `declarations` (document order, unique; each takes the provider's
    /// placeholder value) and `system` values. Values are bound afterwards
    /// with [`Self::initialize_data`].
    ///
    /// # Errors
    ///
    /// [`ArtifactError::ArtifactKindMismatch`] unless `component` is an
    /// authenticated data-model artifact; invalid host limits; compile,
    /// link, or instantiation failures; a trap during `name` or
    /// `initialize`; and [`HostError::Component`] with the closed WIT code
    /// when the component rejects `initialize`. Nothing is returned on
    /// failure.
    pub fn instantiate(
        component: &AuthenticatedComponent,
        declarations: &[String],
        system: &DataModelSystemValues,
        config: &WasmtimeHostConfig,
        providers: &TelemetryProviders,
    ) -> Result<Self, HostError> {
        let span = scxml_telemetry::observe(
            Module::HostWasmtime,
            Operation::HostWasmtimeDataModelInstantiate,
        );
        let result = Self::instantiate_inner(component, declarations, system, config, providers);
        if result.is_ok() {
            span.event(SignalEvent::ArtifactAccepted);
        } else {
            span.event(SignalEvent::InputRejected);
            span.fail(ErrorCategory::Platform);
        }
        result
    }

    fn instantiate_inner(
        component: &AuthenticatedComponent,
        declarations: &[String],
        system: &DataModelSystemValues,
        config: &WasmtimeHostConfig,
        providers: &TelemetryProviders,
    ) -> Result<Self, HostError> {
        if !matches!(
            component.kind(),
            ArtifactKind::DataModelEcmascript
                | ArtifactKind::DataModelNull
                | ArtifactKind::DataModelXpath
        ) {
            return Err(ArtifactError::ArtifactKindMismatch.into());
        }
        if component.bytes().len() > config.component_bytes {
            return Err(ArtifactError::ArtifactTooLarge.into());
        }
        let engine = component_engine(config)?;
        let compiled = wasmtime::component::Component::from_binary(&engine, component.bytes())?;
        let linker = data_model_linker(&engine)?;
        let mut model = Self::from_compiled(
            &engine,
            &compiled,
            &linker,
            config,
            providers,
            Context::current(),
            None,
        )?;
        model
            .initialize_raw(
                declarations,
                &system.session_id,
                system.machine_name.as_deref(),
                wit_value(system.io_processors.clone()),
            )
            .map_err(|error| HostError::Component(error_code_name(error.code).into()))?;
        Ok(model)
    }

    /// Instantiates one precompiled data-model component in a fresh bounded
    /// store without initializing it: the machine initializes it later
    /// through `initialize-data-model`.
    pub(crate) fn from_compiled(
        engine: &wasmtime::Engine,
        compiled: &wasmtime::component::Component,
        linker: &Linker<DataModelStoreData>,
        config: &WasmtimeHostConfig,
        providers: &TelemetryProviders,
        execution_context: Context,
        call_budget: Option<&Arc<CallBudget>>,
    ) -> Result<Self, HostError> {
        let limits = wasmtime::StoreLimitsBuilder::new()
            .memory_size(config.store.memory_bytes)
            .table_elements(config.store.table_elements)
            .instances(config.store.instances)
            .tables(config.store.tables)
            .memories(config.store.memories)
            .trap_on_grow_failure(true)
            .build();
        let limits = MeteredLimits::metered(limits, call_budget, DEFAULT_DATA_MODEL_CALL_FUEL);
        let budget = Arc::clone(limits.budget());
        let mut store = wasmtime::Store::new(
            engine,
            DataModelStoreData {
                limits,
                configuration: std::collections::BTreeSet::new(),
                providers: providers.clone(),
                execution_context,
                guest_spans: crate::guest_spans::GuestSpanTable::new(),
                session_epoch_ms: 0,
                session_time: SessionTime::default(),
                _store_lease: providers.acquire_store()?,
            },
        );
        store.limiter(|state| &mut state.limits);
        budget.share(&mut store);
        store.set_fuel(DEFAULT_DATA_MODEL_CALL_FUEL)?;
        record_store_limits(&config.store);
        let bindings = data_model_bindings::DataModel::instantiate(&mut store, compiled, linker)?;
        let api = bindings.stateforward_scxml_data_model_api();
        let name = api.call_name(&mut store)?;
        Ok(Self {
            store,
            bindings,
            name,
            call_fuel: DEFAULT_DATA_MODEL_CALL_FUEL,
            last_call: crate::CallFuel::default(),
            lost: false,
        })
    }

    /// `data-model-api.initialize` with `declarations` and the system
    /// values, forwarded unchanged.
    ///
    /// # Errors
    ///
    /// The component's error; `platform` after a trap.
    pub(crate) fn initialize_raw(
        &mut self,
        declarations: &[String],
        session_id: &str,
        machine_name: Option<&str>,
        io_processors: crate::bindings::stateforward::scxml::types::DataValue,
    ) -> Result<(), ComponentError> {
        let system = SystemValues {
            session_id: session_id.to_owned(),
            machine_name: machine_name.map(str::to_owned),
            io_processors,
        };
        self.forward(|api, store| api.call_initialize(store, declarations, &system))
    }

    /// Forwards one fallible `data-model-api` call with a fresh fuel budget,
    /// returning the component's WIT result unchanged. A trap poisons the
    /// instance and answers `platform` now and for every later call.
    pub(crate) fn forward<T>(
        &mut self,
        body: impl FnOnce(
            &DataModelGuest,
            &mut wasmtime::Store<DataModelStoreData>,
        ) -> wasmtime::Result<Result<T, ComponentError>>,
    ) -> Result<T, ComponentError> {
        if self.lost {
            return Err(component_error(
                ErrorCode::Platform,
                "data-model component trapped earlier",
            ));
        }
        let budget = Arc::clone(self.store.data().limits.budget());
        let before = budget.spent();
        let outcome = self.store.set_fuel(self.call_fuel).and_then(|()| {
            body(
                self.bindings.stateforward_scxml_data_model_api(),
                &mut self.store,
            )
        });
        self.last_call = budget.spent().since(before);
        outcome.unwrap_or_else(|_| {
            self.lost = true;
            Err(component_error(
                ErrorCode::Platform,
                "data-model component trapped or exhausted its fuel",
            ))
        })
    }

    /// Answers `execution-host.now-ms` from the clock of the session this
    /// data model belongs to: its start epoch `epoch_ms` (Unix epoch ms)
    /// plus its logical time `time`.
    pub(crate) fn share_session_time(&mut self, epoch_ms: u64, time: SessionTime) {
        let data = self.store.data_mut();
        data.session_epoch_ms = epoch_ms;
        data.session_time = time;
    }

    /// The component's `name` (`ecmascript`, `xpath`, `null`).
    pub(crate) fn component_name(&self) -> &str {
        &self.name
    }

    /// Forwards one pure `data-model-api` call (`none` answers). A trap
    /// poisons the instance, so the session's next fallible call fails
    /// with `platform`; this call answers `none`.
    pub(crate) fn forward_pure<T>(
        &mut self,
        body: impl FnOnce(
            &DataModelGuest,
            &mut wasmtime::Store<DataModelStoreData>,
        ) -> wasmtime::Result<Option<T>>,
    ) -> Option<T> {
        if self.lost {
            return None;
        }
        let outcome = self.store.set_fuel(self.call_fuel).and_then(|()| {
            body(
                self.bindings.stateforward_scxml_data_model_api(),
                &mut self.store,
            )
        });
        outcome.unwrap_or_else(|_| {
            self.lost = true;
            None
        })
    }

    /// Values one declared `<data>` variable
    /// (`data-model-api.initialize-data`, SCXML 5.2-5.3): an expression is
    /// evaluated with no current event and stored in the same call.
    ///
    /// # Errors
    ///
    /// The closed failure the component returns: `InvalidInput` or
    /// `Execution` for a recoverable initializer failure; `Platform` after a
    /// trap.
    pub fn initialize_data(
        &mut self,
        id: &str,
        initializer: &scxml_types::DataInitializer,
    ) -> Result<(), DataModelFailure> {
        let initializer = match initializer {
            scxml_types::DataInitializer::Expression(expression) => {
                WitDataInitializer::Expression(expression.clone())
            }
            scxml_types::DataInitializer::Value(value) => {
                WitDataInitializer::Value(wit_value(value.clone()))
            }
        };
        self.call(|api, store| api.call_initialize_data(store, id, &initializer))
    }

    /// Sets the instruction fuel of each later data-model call.
    ///
    /// # Errors
    ///
    /// [`HostError::InvalidLimit`] for zero fuel.
    pub fn with_call_fuel(mut self, fuel: u64) -> Result<Self, HostError> {
        if fuel == 0 {
            return Err(HostError::InvalidLimit(InvalidLimit::Fuel));
        }
        self.call_fuel = fuel;
        self.store.data().limits.set_call_fuel(fuel);
        Ok(self)
    }

    /// The fuel the last data-model call spent, by kind: the Wasm it
    /// executed and its memory growth copies. The call, and each allocation
    /// call Wasmtime makes into the component to pass its arguments and
    /// results, run within the per-call fuel.
    #[must_use]
    pub const fn last_call_fuel(&self) -> crate::CallFuel {
        self.last_call
    }

    /// The bytes every linear memory growth of this data model copied so
    /// far, each taken from its call's copy allowance
    /// ([`crate::MEMORY_GROWTH_COPY_BYTES_PER_FUEL`] bytes per fuel unit).
    #[must_use]
    pub fn memory_growth_copied_bytes(&self) -> u64 {
        self.store.data().limits.copied_bytes()
    }

    /// Runs one component call with a fresh fuel budget; a trap poisons the
    /// instance.
    fn call<T>(
        &mut self,
        body: impl FnOnce(
            &data_model_bindings::exports::stateforward::scxml::data_model_api::Guest,
            &mut wasmtime::Store<DataModelStoreData>,
        ) -> wasmtime::Result<Result<T, ComponentError>>,
    ) -> Result<T, DataModelFailure> {
        self.forward(body).map_err(|error| failure(&error))
    }
}

impl MachineDataModel for ComponentDataModel {
    fn name(&self) -> Option<&str> {
        Some(self.name.as_str())
    }

    fn as_component(&mut self) -> Option<&mut ComponentDataModel> {
        Some(self)
    }

    fn update_active_configuration(
        &mut self,
        exited: &[String],
        entered: &[String],
    ) -> Result<(), DataModelFailure> {
        let configuration = &mut self.store.data_mut().configuration;
        for state in exited {
            configuration.remove(state);
        }
        configuration.extend(entered.iter().cloned());
        Ok(())
    }

    fn evaluate_condition(
        &mut self,
        expression: &str,
        event: Option<&scxml_types::Event>,
    ) -> Result<bool, DataModelFailure> {
        let event = event.map(wit_event);
        self.call(|api, store| api.call_evaluate_condition(store, expression, event.as_ref()))
    }

    fn evaluate_value(
        &mut self,
        expression: &str,
        event: Option<&scxml_types::Event>,
    ) -> Result<scxml_types::DataValue, DataModelFailure> {
        let event = event.map(wit_event);
        self.call(|api, store| api.call_evaluate_value(store, expression, event.as_ref()))
            .and_then(|value| native_value(value).map_err(|_| DataModelFailure::Platform))
    }

    fn assign(
        &mut self,
        location: &str,
        source: &scxml_types::AssignSource,
        placement: &scxml_types::AssignPlacement,
        event: Option<&scxml_types::Event>,
    ) -> Result<(), DataModelFailure> {
        let source = wit_source(source.clone());
        let placement = wit_placement(placement.clone());
        let event = event.map(wit_event);
        self.call(|api, store| {
            api.call_assign(store, location, &source, &placement, event.as_ref(), None)
        })
    }

    fn execute(
        &mut self,
        script: &str,
        event: Option<&scxml_types::Event>,
    ) -> Result<(), DataModelFailure> {
        let event = event.map(wit_event);
        self.call(|api, store| api.call_execute(store, script, event.as_ref()))
    }

    fn evaluate_log(
        &mut self,
        expression: &str,
        event: Option<&scxml_types::Event>,
    ) -> Result<String, DataModelFailure> {
        let event = event.map(wit_event);
        self.call(|api, store| api.call_evaluate_log(store, expression, event.as_ref()))
    }
}

/// Closed WIT error-code label (bounded; never a payload).
const fn error_code_name(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::InvalidInput => "invalid-input",
        ErrorCode::UnsupportedVersion => "unsupported-version",
        ErrorCode::Unavailable => "unavailable",
        ErrorCode::CapacityExhausted => "capacity-exhausted",
        ErrorCode::DuplicateId => "duplicate-id",
        ErrorCode::UnknownId => "unknown-id",
        ErrorCode::Execution => "execution",
        ErrorCode::Protocol => "protocol",
        ErrorCode::PayloadTooLarge => "payload-too-large",
        ErrorCode::Platform => "platform",
    }
}

impl crate::bindings::stateforward::scxml::types::Host for DataModelStoreData {}

impl execution_host::Host for DataModelStoreData {
    fn raise(&mut self, _event: WitEvent) -> Result<(), ComponentError> {
        Err(component_error(
            ErrorCode::Unavailable,
            "raise is not provided to data models",
        ))
    }

    fn send(&mut self, _request: SendRequest) -> Result<(), ComponentError> {
        Err(component_error(
            ErrorCode::Unavailable,
            "send is not provided to data models",
        ))
    }

    fn cancel(&mut self, _send_id: String) -> Result<(), ComponentError> {
        Err(component_error(
            ErrorCode::Unavailable,
            "cancel is not provided to data models",
        ))
    }

    fn log(&mut self, _level: LogLevel, _message: String) {}

    fn in_state(&mut self, state_id: String) -> bool {
        self.configuration.contains(&state_id)
    }

    fn now_ms(&mut self) -> u64 {
        self.session_epoch_ms
            .saturating_add(self.session_time.load(std::sync::atomic::Ordering::Relaxed))
    }
}

impl telemetry::Host for DataModelStoreData {
    fn start(
        &mut self,
        operation: telemetry::Operation,
        parent: Option<WitTraceContext>,
    ) -> (u64, Option<WitTraceContext>) {
        let Ok(parent) = crate::component::parent_context(&self.execution_context, parent.as_ref())
        else {
            crate::component::guest_telemetry_refused(&self.execution_context);
            return (0, None);
        };
        let started = self.guest_spans.start(
            &self.providers,
            "scxml-component-guest",
            "data-model-component",
            crate::component::operation_name(operation),
            &parent,
        );
        crate::component::guest_span_started(&self.execution_context, started)
    }

    fn end(&mut self, span: u64, outcome: telemetry::Outcome) {
        let ended = self
            .guest_spans
            .end(span, crate::component::outcome_name(outcome));
        crate::component::guest_span_ended(&self.execution_context, &ended);
    }

    fn observe(
        &mut self,
        operation: telemetry::Operation,
        outcome: telemetry::Outcome,
        _measurement: Option<telemetry::Measurement>,
        context: Option<WitTraceContext>,
    ) {
        let recorded = record_guest_signal(
            &self.providers,
            &self.execution_context,
            "data-model-component",
            GuestSignal {
                operation: crate::component::operation_name(operation),
                outcome: Some(crate::component::outcome_name(outcome)),
                event: None,
            },
            context.as_ref(),
        );
        crate::component::guest_signal_recorded(&self.execution_context, &recorded);
    }

    fn event(
        &mut self,
        operation: telemetry::Operation,
        event: telemetry::SignalEvent,
        context: Option<WitTraceContext>,
    ) {
        let recorded = record_guest_signal(
            &self.providers,
            &self.execution_context,
            "data-model-component",
            GuestSignal {
                operation: crate::component::operation_name(operation),
                outcome: None,
                event: Some(crate::component::event_name(event)),
            },
            context.as_ref(),
        );
        crate::component::guest_signal_recorded(&self.execution_context, &recorded);
    }
}

/// Maximum data-model components one session host binds.
pub const MAX_DATA_MODEL_COMPONENTS: usize = 64;

/// The data-model components of data-model packages one session host links,
/// keyed by the data model's name (the `datamodel` token a document writes
/// and its Machine IR records). The built-in `ecmascript`, `null`, and
/// `xpath` are the release's own and are never bound here.
#[derive(Clone, Debug, Default)]
pub struct DataModelComponents {
    bindings: Vec<(String, AuthenticatedComponent)>,
}

impl DataModelComponents {
    /// No bindings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Binds `component` to the data model named exactly `name` (the
    /// package manifest's `datamodel`).
    ///
    /// # Errors
    ///
    /// [`HostError::Artifact`]: a component that is not
    /// [`ArtifactKind::DataModelComponent`]
    /// ([`ArtifactError::ArtifactKindMismatch`]); or a name that is empty,
    /// a built-in data model,
    /// or bound twice, or past [`MAX_DATA_MODEL_COMPONENTS`] bindings
    /// ([`ArtifactError::NamespaceBinding`]).
    pub fn bind(
        mut self,
        name: &str,
        component: &AuthenticatedComponent,
    ) -> Result<Self, HostError> {
        if component.kind() != ArtifactKind::DataModelComponent {
            return Err(ArtifactError::ArtifactKindMismatch.into());
        }
        if name.is_empty()
            || matches!(name, "ecmascript" | "null" | "xpath")
            || self.bindings.len() >= MAX_DATA_MODEL_COMPONENTS
            || self.bindings.iter().any(|(bound, _)| bound == name)
        {
            return Err(ArtifactError::NamespaceBinding.into());
        }
        self.bindings.push((name.to_owned(), component.clone()));
        Ok(self)
    }

    pub(crate) fn into_bindings(self) -> Vec<(String, AuthenticatedComponent)> {
        self.bindings
    }
}
