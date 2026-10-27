//! Canonical machine-component linking and one-shot execution.

use super::{
    ArtifactError, ArtifactKind, AuthenticatedComponent, ComponentMeasurement, ComponentSource,
    HostError, LifecycleLease, TelemetryProviders, WasmtimeHostConfig, component_engine,
    record_store_limits,
};
use crate::bindings;
use crate::bindings::exports::stateforward::scxml::machine_api::{
    CallResult, SessionFailure, Status,
};
use crate::bindings::stateforward::scxml::ir_source;
use crate::bindings::stateforward::scxml::machine_capability_types::{
    self, DocumentKind, ExtensionExecution, MachineCapabilities, MachineSnapshot, Observation,
    SendDisposition,
};
use crate::bindings::stateforward::scxml::telemetry;
use crate::bindings::stateforward::scxml::types::{
    AssignPlacement, AssignSource, BaggagePolicy, ComponentError, DataInitializer, DataValue,
    ErrorCode, ErrorVersion, Event, ExecutableContentExtension, ForeachResult, InlineBody,
    NamedValue, ProcessorSend, ScriptReference, SendRequest, SendTarget,
    TraceContext as WitTraceContext,
};
use opentelemetry::Context;
use opentelemetry::trace::{TraceContextExt as _, Tracer as _, TracerProvider as _};
use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, TraceContext};
use wasmtime::component::{HasSelf, Linker, Resource};

use crate::call_budget::CallBudget;
use crate::guest_spans::GuestSpanTable;
use crate::memory_growth::MeteredLimits;
use crate::script::{MachineScripts, ScriptHost, ScriptTelemetry};
use crate::session::{
    IR_SOURCE_INTERFACE, MachineCallTrace, MachineDataModel, MachineIrSource, MachineLogRecord,
    MachineSessionTrace, external_event, failure_code, native_event, native_placement,
    native_source, native_value, wit_value,
};
use crate::session_host::SessionLinks;
use std::sync::Arc;

/// Observable result of a representative component start call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentExecution {
    /// Canonical WIT status after `session.start`.
    pub status: ComponentStatus,
    /// Active state identifiers returned by the component.
    pub active_configuration: Vec<String>,
    /// Effect count returned by the component.
    pub effect_count: u32,
    /// Remaining deterministic Wasmtime fuel after shutdown and resource cleanup.
    pub remaining_fuel: u64,
    /// Pending internal queue depth observed through the read-only snapshot.
    pub queue_depth: u64,
}

/// Stable host projection of the canonical WIT machine status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComponentStatus {
    /// Session has not started.
    Ready,
    /// Session is running.
    Running,
    /// Session reached a final state.
    Finished,
    /// Session failed terminally; the component reports the cause.
    Failed,
    /// Session has shut down.
    ShutDown,
}

pub(crate) struct MachineStoreData {
    limits: MeteredLimits,
    providers: TelemetryProviders,
    guest_spans: GuestSpanTable,
    execution_context: Context,
    /// Bytes served through `ir-source` (empty in packaged mode, where the
    /// component does not import it).
    machine_ir: Vec<u8>,
    /// Linked data-model capability; `None` answers every data-model call
    /// with `unavailable`.
    data_model: Option<Box<dyn MachineDataModel>>,
    /// Whether the machine initialized the linked data model (its `start`
    /// called `initialize-data-model`). A document that needs no data model
    /// never initializes one and runs with none linked, so the read-only
    /// snapshot gather treats it as not linked instead of calling an export
    /// that can only answer `unavailable`.
    data_model_initialized: bool,
    /// `<log>` records observed since the last drain.
    logs: Vec<MachineLogRecord>,
    /// External `<script src>` loader and loaded scripts; `None` answers
    /// `load-script` and `run-script` with `unavailable`.
    scripts: Option<ScriptHost>,
    /// `_sessionid` of the session this store runs.
    session_id: String,
    /// Code of the first `load-script` failure, which tells a document the
    /// host rejected at load apart from any other construction failure.
    script_load_failure: Option<ErrorCode>,
    /// Session-host capabilities (I/O processors, clock, invocation, run
    /// budget and trace); `None` for the standalone proof host, which
    /// answers them with `unavailable`.
    session: Option<Box<SessionLinks>>,
    /// This session's extension-component instances; `None` answers
    /// `execute-extension` with `unavailable`. Declared after the data model
    /// and links so extension components shut down after the session's last
    /// call.
    namespaces: Option<crate::extension::SessionNamespaces>,
    /// The active configuration (`update-active-configuration`), kept only
    /// when extension components are linked (their `in-state`).
    configuration: Vec<String>,
    _store_lease: LifecycleLease,
}

/// Session-scoped links installed into one machine store.
pub(crate) struct MachineLinks {
    pub(crate) machine_ir: Vec<u8>,
    pub(crate) data_model: Option<Box<dyn MachineDataModel>>,
}

impl ir_source::Host for MachineStoreData {
    fn machine_ir(&mut self) -> Vec<u8> {
        let span = scxml_telemetry::observe(Module::HostWasmtime, Operation::HostWasmtimeIrSource);
        span.event(SignalEvent::ArtifactProduced);
        self.machine_ir.clone()
    }
}

impl bindings::stateforward::scxml::types::Host for MachineStoreData {}

impl telemetry::Host for MachineStoreData {
    fn start(
        &mut self,
        operation: telemetry::Operation,
        parent: Option<WitTraceContext>,
    ) -> (u64, Option<WitTraceContext>) {
        let Ok(parent) = parent_context(&self.execution_context, parent.as_ref()) else {
            guest_telemetry_refused(&self.execution_context);
            return (0, None);
        };
        let started = self.guest_spans.start(
            &self.providers,
            "scxml-component-guest",
            "machine-component",
            operation_name(operation),
            &parent,
        );
        guest_span_started(&self.execution_context, started)
    }

    fn end(&mut self, span: u64, outcome: telemetry::Outcome) {
        let ended = self.guest_spans.end(span, outcome_name(outcome));
        guest_span_ended(&self.execution_context, &ended);
    }

    fn observe(
        &mut self,
        operation: telemetry::Operation,
        outcome: telemetry::Outcome,
        measurement: Option<telemetry::Measurement>,
        context: Option<WitTraceContext>,
    ) {
        if let Some(measurement) = measurement {
            self.providers.record_component_measurement(
                component_measurement(measurement.kind),
                measurement.value,
            );
        }
        let recorded = self.record_guest_signal(
            operation_name(operation),
            Some(outcome_name(outcome)),
            None,
            context.as_ref(),
        );
        guest_signal_recorded(&self.execution_context, &recorded);
    }

    fn event(
        &mut self,
        operation: telemetry::Operation,
        event: telemetry::SignalEvent,
        context: Option<WitTraceContext>,
    ) {
        let recorded = self.record_guest_signal(
            operation_name(operation),
            None,
            Some(event_name(event)),
            context.as_ref(),
        );
        guest_signal_recorded(&self.execution_context, &recorded);
    }
}

impl MachineStoreData {
    fn record_guest_signal(
        &self,
        operation: &'static str,
        outcome: Option<&'static str>,
        event: Option<&'static str>,
        carrier: Option<&WitTraceContext>,
    ) -> Result<(), HostError> {
        record_guest_signal(
            &self.providers,
            &self.execution_context,
            "machine-component",
            GuestSignal {
                operation,
                outcome,
                event,
            },
            carrier,
        )
    }
}

/// Closed event the host adds to its own execution span when it refuses a
/// guest telemetry import: a malformed carrier, an exhausted span table or
/// provider lease, or an unknown span handle. The WIT telemetry imports have
/// no failure channel, so the refusal is recorded here; the guest's own
/// result is unaffected.
pub(crate) const GUEST_TELEMETRY_REFUSED: &str = "guest-telemetry-refused";

/// Records one refused guest telemetry import on the host execution span.
pub(crate) fn guest_telemetry_refused(execution_context: &Context) {
    execution_context
        .span()
        .add_event(GUEST_TELEMETRY_REFUSED, Vec::new());
}

/// Maps one guest `telemetry.start` result onto the WIT answer: the handle
/// and propagated context, or `(0, none)` when the span was refused.
pub(crate) fn guest_span_started(
    execution_context: &Context,
    started: Result<(u64, TraceContext), crate::guest_spans::GuestSpanError>,
) -> (u64, Option<WitTraceContext>) {
    let answer = started
        .ok()
        .and_then(|(handle, context)| to_wit_trace_context(&context).map(|wit| (handle, wit)));
    if let Some((handle, context)) = answer {
        (handle, Some(context))
    } else {
        guest_telemetry_refused(execution_context);
        (0, None)
    }
}

/// Records a refused guest `telemetry.end` (an unknown or already-ended
/// handle, which closes nothing) on the host execution span.
pub(crate) fn guest_span_ended(
    execution_context: &Context,
    ended: &Result<(), crate::guest_spans::GuestSpanError>,
) {
    if ended.is_err() {
        guest_telemetry_refused(execution_context);
    }
}

/// Records a refused guest point signal on the host execution span.
pub(crate) fn guest_signal_recorded(execution_context: &Context, recorded: &Result<(), HostError>) {
    if recorded.is_err() {
        guest_telemetry_refused(execution_context);
    }
}

/// Resolves the parent context of one guest signal: the guest's carrier, or
/// the host execution context when the guest supplied none.
pub(crate) fn parent_context(
    execution_context: &Context,
    carrier: Option<&WitTraceContext>,
) -> Result<Context, scxml_telemetry::TraceContextError> {
    match carrier {
        Some(carrier) => scxml_telemetry::extract_trace_context(&to_trace_context(carrier)),
        None => Ok(crate::guest_spans::span_parent(execution_context)),
    }
}

/// Closed labels of one guest point signal.
#[derive(Clone, Copy)]
pub(crate) struct GuestSignal {
    pub(crate) operation: &'static str,
    pub(crate) outcome: Option<&'static str>,
    pub(crate) event: Option<&'static str>,
}

/// Records one guest point signal as a closed, attribute-bounded span.
/// `module` is a static component label (`machine-component` or
/// `script-component`); no payload ever becomes an attribute.
pub(crate) fn record_guest_signal(
    providers: &TelemetryProviders,
    execution_context: &Context,
    module: &'static str,
    signal: GuestSignal,
    carrier: Option<&WitTraceContext>,
) -> Result<(), HostError> {
    let _lease = providers.acquire_span()?;
    let parent = parent_context(execution_context, carrier).map_err(HostError::from)?;
    let tracer = providers.tracer.tracer("scxml-component-guest");
    let span = tracer.start_with_context(signal.operation, &parent);
    let context = parent.with_span(span);
    context
        .span()
        .set_attribute(opentelemetry::KeyValue::new("scxml.module", module));
    context.span().set_attribute(opentelemetry::KeyValue::new(
        "scxml.operation",
        signal.operation,
    ));
    if let Some(outcome) = signal.outcome {
        context
            .span()
            .set_attribute(opentelemetry::KeyValue::new("scxml.outcome", outcome));
    }
    if let Some(event) = signal.event {
        context.span().add_event(event, Vec::new());
    }
    context.span().end();
    Ok(())
}

impl MachineStoreData {
    /// Runs one data-model call, or answers `unavailable` when none is linked.
    fn with_data_model<T>(
        &mut self,
        call: impl FnOnce(&mut dyn MachineDataModel) -> Result<T, ComponentError>,
    ) -> Result<T, ComponentError> {
        match self.data_model.as_deref_mut() {
            Some(data_model) => call(data_model),
            None => Err(unavailable()),
        }
    }

    /// The linked data-model component, when the session's data model is
    /// one: every `data-model-api` function is then forwarded to it
    /// one-to-one with WIT values and errors unchanged.
    fn component_data_model(&mut self) -> Option<&mut crate::ComponentDataModel> {
        self.data_model
            .as_deref_mut()
            .and_then(MachineDataModel::as_component)
    }

    /// Forwards one fallible data-model function to the linked component;
    /// `unavailable` when the session links none (or a native provider,
    /// which implements only the original subset).
    fn forward_data_model<T>(
        &mut self,
        body: impl FnOnce(
            &crate::data_model::DataModelGuest,
            &mut wasmtime::Store<crate::data_model::DataModelStoreData>,
        ) -> wasmtime::Result<Result<T, ComponentError>>,
    ) -> Result<T, ComponentError> {
        match self.component_data_model() {
            Some(data_model) => data_model.forward(body),
            None => Err(unavailable()),
        }
    }

    /// Forwards one pure data-model function; `none` without a component.
    fn forward_data_model_pure<T>(
        &mut self,
        body: impl FnOnce(
            &crate::data_model::DataModelGuest,
            &mut wasmtime::Store<crate::data_model::DataModelStoreData>,
        ) -> wasmtime::Result<Option<T>>,
    ) -> Option<T> {
        self.component_data_model()
            .and_then(|data_model| data_model.forward_pure(body))
    }

    /// The linked data-model component's name and, under
    /// [`DataModelRead::Snapshot`](crate::data_model::DataModelRead), its
    /// read-only `data-model-api.snapshot`; `None` when the session links
    /// no data-model component.
    pub(crate) fn data_model_snapshot(
        &mut self,
        read: crate::data_model::DataModelRead,
    ) -> Option<(
        String,
        Option<Result<crate::data_model::DataModelSnapshot, ComponentError>>,
    )> {
        // No data model linked, or the machine never initialized one: both
        // run with no data model, so the session has no data-model snapshot
        // and no data-model role.
        if !self.data_model_initialized {
            return None;
        }
        let data_model = self.component_data_model()?;
        let name = data_model.component_name().to_owned();
        let snapshot = match read {
            crate::data_model::DataModelRead::Snapshot => {
                Some(data_model.forward(|api, store| api.call_snapshot(store)))
            }
            crate::data_model::DataModelRead::NameOnly => None,
        };
        Some((name, snapshot))
    }

    /// The Machine IR bytes this store serves through `ir-source`.
    pub(crate) fn machine_ir(&self) -> &[u8] {
        &self.machine_ir
    }

    /// Mutable session-host links for this session.
    pub(crate) fn links_mut(&mut self) -> Option<&mut SessionLinks> {
        self.session.as_deref_mut()
    }

    /// The code of the first `load-script` failure during construction.
    pub(crate) const fn script_load_failure(&self) -> Option<ErrorCode> {
        self.script_load_failure
    }
}

impl machine_capability_types::HostMachineCapabilities for MachineStoreData {
    fn update_active_configuration(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        exited: Vec<String>,
        entered: Vec<String>,
    ) -> Result<(), ComponentError> {
        if self.namespaces.is_some() {
            self.configuration
                .retain(|state| !exited.iter().any(|gone| gone == state));
            self.configuration.extend(entered.iter().cloned());
        }
        if let Some(Err(failure)) = self
            .data_model
            .as_deref_mut()
            .map(|data_model| data_model.update_active_configuration(&exited, &entered))
        {
            return Err(failure_code(failure));
        }
        Ok(())
    }

    fn evaluate_condition(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        expression: String,
        event: Option<Event>,
    ) -> Result<bool, ComponentError> {
        if self.component_data_model().is_some() {
            return self.forward_data_model(|api, store| {
                api.call_evaluate_condition(store, &expression, event.as_ref())
            });
        }
        let event = event.map(native_event).transpose()?;
        self.with_data_model(|data_model| {
            data_model
                .evaluate_condition(&expression, event.as_ref())
                .map_err(failure_code)
        })
    }

    fn evaluate_value(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        expression: String,
        event: Option<Event>,
    ) -> Result<DataValue, ComponentError> {
        if self.component_data_model().is_some() {
            return self.forward_data_model(|api, store| {
                api.call_evaluate_value(store, &expression, event.as_ref())
            });
        }
        let event = event.map(native_event).transpose()?;
        self.with_data_model(|data_model| {
            data_model
                .evaluate_value(&expression, event.as_ref())
                .map(wit_value)
                .map_err(failure_code)
        })
    }

    fn assign(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        location: String,
        source: AssignSource,
        placement: AssignPlacement,
        event: Option<Event>,
        context: Option<WitTraceContext>,
    ) -> Result<(), ComponentError> {
        if self.component_data_model().is_some() {
            return self.forward_data_model(|api, store| {
                api.call_assign(
                    store,
                    &location,
                    &source,
                    &placement,
                    event.as_ref(),
                    context.as_ref(),
                )
            });
        }
        let source = native_source(source)?;
        let placement = native_placement(placement);
        let event = event.map(native_event).transpose()?;
        self.with_data_model(|data_model| {
            data_model
                .assign(&location, &source, &placement, event.as_ref())
                .map_err(failure_code)
        })
    }

    fn execute(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        script: String,
        event: Option<Event>,
    ) -> Result<(), ComponentError> {
        if self.component_data_model().is_some() {
            return self
                .forward_data_model(|api, store| api.call_execute(store, &script, event.as_ref()));
        }
        let event = event.map(native_event).transpose()?;
        self.with_data_model(|data_model| {
            data_model
                .execute(&script, event.as_ref())
                .map_err(failure_code)
        })
    }

    fn load_script(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        reference: ScriptReference,
    ) -> Result<(), ComponentError> {
        let data_model = self
            .data_model
            .as_deref()
            .and_then(MachineDataModel::name)
            .map(str::to_owned);
        let loaded = match self.scripts.as_mut() {
            Some(scripts) => scripts.load(&reference, data_model.as_deref()),
            None => Err(unavailable()),
        };
        if let Err(error) = &loaded
            && self.script_load_failure.is_none()
        {
            self.script_load_failure = Some(error.code);
        }
        loaded
    }

    fn run_script(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        reference: ScriptReference,
        event: Option<Event>,
    ) -> Result<(), ComponentError> {
        let native = event.clone().map(native_event).transpose()?;
        match self.scripts.as_ref() {
            Some(scripts) => scripts.run(
                &reference,
                native,
                event.as_ref(),
                &mut self.data_model,
                ScriptTelemetry {
                    providers: &self.providers,
                    execution_context: &self.execution_context,
                },
            ),
            None => Err(unavailable()),
        }
    }

    // The machine's own `<raise>` is engine-internal; no linked component
    // raises into a machine session.
    fn raise(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        _event: Event,
    ) -> Result<(), ComponentError> {
        Err(unavailable())
    }

    fn send(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        request: SendRequest,
    ) -> Result<SendDisposition, ComponentError> {
        match self.links_mut() {
            Some(links) => links.send(&request),
            None => Err(unavailable()),
        }
    }

    fn cancel(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        send_id: String,
        context: Option<WitTraceContext>,
    ) -> Result<(), ComponentError> {
        match self.links_mut() {
            Some(links) => links.cancel(&send_id, context.as_ref()),
            None => Err(unavailable()),
        }
    }

    fn log(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        label: Option<String>,
        text: String,
        _context: Option<WitTraceContext>,
    ) -> Result<(), ComponentError> {
        // The host records log text of any length: the data model's own
        // value bound (a wasm build) already bounds what `evaluate-log`
        // renders.
        if let Some(links) = self.links_mut() {
            links.log(label, text);
            return Ok(());
        }
        self.logs.push(MachineLogRecord { label, text });
        Ok(())
    }

    // The session's data model, links, and configuration are lent to the
    // extension component for the call and always restored.
    fn execute_extension(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        element: ExecutableContentExtension,
        event: Option<Event>,
        context: Option<WitTraceContext>,
        snapshot: Option<MachineSnapshot>,
    ) -> Result<ExtensionExecution, ComponentError> {
        let Some(namespaces) = self.namespaces.as_mut() else {
            return Err(unavailable());
        };
        let event = event.map(native_event).transpose()?;
        // The machine's executable-content image, with what only this store
        // knows, for an extension component that reads the session snapshot.
        let image = snapshot.map(|machine| crate::session_host::ExtensionImage {
            machine,
            ir_sha256: {
                use sha2::Digest as _;
                crate::artifact::lower_hex(&sha2::Sha256::digest(&self.machine_ir))
            },
            data_model_initialized: self.data_model_initialized,
        });
        let lent = crate::extension::Lent {
            data_model: self.data_model.take(),
            links: self.session.take(),
            configuration: std::mem::take(&mut self.configuration),
            image,
        };
        let (executed, lent) = namespaces.execute(&element, event, context.as_ref(), lent);
        self.data_model = lent.data_model;
        self.session = lent.links;
        self.configuration = lent.configuration;
        let executed = executed?;
        Ok(ExtensionExecution {
            outcome: executed.outcome,
            internal: executed.internal,
            external: executed.external,
        })
    }

    // Under the session host every namespace the document uses gets this
    // session's instance (loaded through the embedder's resolver when no
    // binding names it); the standalone proof host binds none and answers
    // `unavailable`, so such a session never starts.
    fn load_extensions(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        namespace_uris: Vec<String>,
    ) -> Result<(), ComponentError> {
        match self.session.as_deref() {
            Some(links) => links.load_extensions(&mut self.namespaces, &namespace_uris),
            None => Err(unavailable()),
        }
    }

    // Only a bound extension component that imports `session-snapshot-host`
    // reads the machine's executable-content image.
    fn extension_snapshot_request(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        namespace_uri: String,
    ) -> bool {
        self.namespaces
            .as_ref()
            .is_some_and(|namespaces| namespaces.reads_snapshot(&namespace_uri))
    }

    // Under the session host every command is admitted against the run's
    // shared command budget. The standalone proof host imposes none:
    // Wasmtime fuel and the machine's per-call command bound are its bounds.
    fn admit_command(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
    ) -> Result<(), ComponentError> {
        match self.links_mut() {
            Some(links) => links.admit_command(),
            None => Ok(()),
        }
    }

    // Under the session host each observation extends the run trace; the
    // standalone proof host keeps none.
    fn observe(&mut self, _capabilities: Resource<MachineCapabilities>, observation: Observation) {
        if let Some(links) = self.links_mut() {
            links.observe(observation);
        }
    }

    fn data_model_name(&mut self, _capabilities: Resource<MachineCapabilities>) -> Option<String> {
        self.data_model
            .as_deref()
            .and_then(MachineDataModel::name)
            .map(str::to_owned)
    }

    fn initialize_data_model(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        declarations: Vec<String>,
        machine_name: Option<String>,
    ) -> Result<(), ComponentError> {
        if self.data_model.is_none() {
            return Err(unavailable());
        }
        // `_ioprocessors` lists the processors linked to this session: the
        // session host's SCXML and BasicHTTP processors, or none for the
        // standalone proof host.
        let io_processors = match self.session.as_deref_mut() {
            Some(links) => links.io_processors()?,
            None => DataValue::JsonObject("{}".into()),
        };
        let session_id = self.session_id.clone();
        if let Some(data_model) = self.component_data_model() {
            let initialized = data_model.initialize_raw(
                &declarations,
                &session_id,
                machine_name.as_deref(),
                io_processors,
            );
            if initialized.is_ok() {
                self.data_model_initialized = true;
            }
            return initialized;
        }
        let system = crate::DataModelSystemValues {
            session_id,
            machine_name,
            io_processors: native_value(io_processors)?,
        };
        self.with_data_model(|data_model| {
            data_model
                .initialize(&declarations, &system)
                .map_err(failure_code)
        })
        .inspect(|()| self.data_model_initialized = true)
    }

    fn declare(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        id: String,
    ) -> Result<(), ComponentError> {
        self.forward_data_model(|api, store| api.call_declare(store, &id))
    }

    fn initialize_data(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        id: String,
        initializer: DataInitializer,
    ) -> Result<(), ComponentError> {
        self.forward_data_model(|api, store| api.call_initialize_data(store, &id, &initializer))
    }

    fn set_parameter(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        name: String,
        value: DataValue,
    ) -> Result<bool, ComponentError> {
        self.forward_data_model(|api, store| api.call_set_parameter(store, &name, &value))
    }

    fn evaluate_location(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        location: String,
        event: Option<Event>,
    ) -> Result<DataValue, ComponentError> {
        self.forward_data_model(|api, store| {
            api.call_evaluate_location(store, &location, event.as_ref())
        })
    }

    fn evaluate_text(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        expression: String,
        event: Option<Event>,
    ) -> Result<Option<String>, ComponentError> {
        self.forward_data_model(|api, store| {
            api.call_evaluate_text(store, &expression, event.as_ref())
        })
    }

    fn evaluate_log(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        expression: String,
        event: Option<Event>,
    ) -> Result<String, ComponentError> {
        if self.component_data_model().is_some() {
            return self.forward_data_model(|api, store| {
                api.call_evaluate_log(store, &expression, event.as_ref())
            });
        }
        let event = event.map(native_event).transpose()?;
        self.with_data_model(|data_model| {
            data_model
                .evaluate_log(&expression, event.as_ref())
                .map_err(failure_code)
        })
    }

    fn evaluate_form_value(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        source: String,
        location: bool,
        event: Option<Event>,
    ) -> Result<DataValue, ComponentError> {
        self.forward_data_model(|api, store| {
            api.call_evaluate_form_value(store, &source, location, event.as_ref())
        })
    }

    fn foreach_items(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        expression: String,
        event: Option<Event>,
    ) -> Result<ForeachResult, ComponentError> {
        self.forward_data_model(|api, store| {
            api.call_foreach_items(store, &expression, event.as_ref())
        })
    }

    fn release_foreach(&mut self, _capabilities: Resource<MachineCapabilities>) {
        // A trapped data model has nothing left to forget.
        self.forward_data_model_pure(|api, store| api.call_release_foreach(store).map(Some));
    }

    fn namelist_key(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        location: String,
    ) -> Option<String> {
        self.forward_data_model_pure(|api, store| api.call_namelist_key(store, &location))
    }

    fn event_payload(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        entries: Vec<NamedValue>,
    ) -> Option<DataValue> {
        self.forward_data_model_pure(|api, store| api.call_event_payload(store, &entries))
    }

    fn inline_content(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        content: InlineBody,
    ) -> Option<DataValue> {
        self.forward_data_model_pure(|api, store| api.call_inline_content(store, &content))
    }

    fn external_data(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        text: String,
    ) -> Option<DataValue> {
        self.forward_data_model_pure(|api, store| api.call_external_data(store, &text))
    }

    fn event_data_field(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        data: DataValue,
        name: String,
    ) -> Option<DataValue> {
        self.forward_data_model_pure(|api, store| api.call_event_data_field(store, &data, &name))
    }

    fn accepts_event_data(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        data: DataValue,
    ) -> bool {
        self.forward_data_model_pure(|api, store| {
            api.call_accepts_event_data(store, &data).map(Some)
        })
        .unwrap_or(true)
    }

    fn load_document(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        kind: DocumentKind,
        uri: String,
    ) -> Result<machine_capability_types::LoadedDocument, ComponentError> {
        match self.links_mut() {
            Some(links) => links.load_document(kind, &uri),
            None => Err(unavailable()),
        }
    }

    // The standalone proof host links no SCXML Event I/O Processor, so no
    // session answers any target there.
    fn classify_target(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        target: Option<String>,
    ) -> SendTarget {
        match self.links_mut() {
            Some(links) => links.classify_target(target.as_deref()),
            None => SendTarget::Unreachable,
        }
    }

    fn send_processor(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        request: ProcessorSend,
    ) -> Result<SendDisposition, ComponentError> {
        match self.links_mut() {
            Some(links) => links.send_processor(request),
            None => Err(unavailable()),
        }
    }

    fn compile_invocation(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        source_name: String,
        source: String,
    ) -> Result<Vec<u8>, ComponentError> {
        match self.links_mut() {
            Some(links) => links.compile_invocation(source_name, source),
            None => Err(unavailable()),
        }
    }

    fn start_invocation(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        request: machine_capability_types::Invocation,
    ) -> Result<machine_capability_types::InvocationProgress, ComponentError> {
        match self.links_mut() {
            Some(links) => links.start(request),
            None => Err(unavailable()),
        }
    }

    fn cancel_invocation(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        invoke_id: String,
    ) -> Result<machine_capability_types::InvocationProgress, ComponentError> {
        match self.links_mut() {
            Some(links) => links.cancel_invocation(&invoke_id),
            None => Err(unavailable()),
        }
    }

    fn forward_to_invocation(
        &mut self,
        _capabilities: Resource<MachineCapabilities>,
        invoke_id: String,
        event: Event,
    ) -> Result<(), ComponentError> {
        match self.links_mut() {
            Some(links) => {
                links.forward_to_invocation(&invoke_id, event);
                Ok(())
            }
            None => Err(unavailable()),
        }
    }

    fn drop(&mut self, _rep: Resource<MachineCapabilities>) -> wasmtime::Result<()> {
        Ok(())
    }
}

impl machine_capability_types::Host for MachineStoreData {}

/// Links, instantiates, starts, and cleanly shuts down one machine component.
///
/// `component` must have been authenticated by
/// [`crate::authenticate_component`] (the generic machine, linked with
/// [`MachineIrSource::Runtime`]) or [`crate::authenticate_packaged_machine`]
/// (a packaged `component-wasm`, linked with [`MachineIrSource::Packaged`]).
/// Its bytes are borrowed only for compilation. The
/// returned value is an owned projection of the start call result. This function performs no
/// SCXML selection or stepping; those semantics remain inside the component.
/// The authenticated artifact's fixed ceiling
/// ([`crate::MAX_AUTHENTICATED_COMPONENT_BYTES`]) bounds pre-compilation
/// structure. Fuel, memory, tables, memories, and instances are bounded by
/// `config` after compilation. The session resource and store are dropped
/// before return.
///
/// # Errors
///
/// Rejects the wrong authenticated artifact kind, an unbounded host
/// configuration, missing WIT imports/exports, Wasmtime traps (including fuel
/// exhaustion), and typed guest failures. No live store or guest resource is
/// retained on error.
pub fn execute_component_start(
    component: ComponentSource<'_>,
    ir: MachineIrSource<'_>,
    config: &WasmtimeHostConfig,
    providers: &TelemetryProviders,
) -> Result<ComponentExecution, HostError> {
    let parent = crate::guest_spans::span_parent(&Context::current());
    let span = scxml_telemetry::observe(
        Module::HostWasmtime,
        Operation::HostWasmtimeComponentExecute,
    );
    let component = match component.authenticated() {
        Ok(component) => component,
        Err(error) => {
            span.event(SignalEvent::InputRejected);
            span.fail(host_error_category(&error));
            return Err(error);
        }
    };
    let _lease = match providers.acquire_span() {
        Ok(lease) => lease,
        Err(error) => {
            span.event(SignalEvent::InputRejected);
            span.fail(host_error_category(&error));
            return Err(error);
        }
    };
    let tracer = providers.tracer.tracer("scxml-host-wasmtime");
    let execution_span = tracer.start_with_context("component-execute", &parent);
    let execution_context = parent.with_span(execution_span);
    let guard = execution_context.clone().attach();
    let result =
        execute_component_start_inner(component, ir, config, providers, execution_context.clone());
    if result.is_ok() {
        span.event(SignalEvent::TransformationCompleted);
        execution_context
            .span()
            .set_attribute(opentelemetry::KeyValue::new("scxml.outcome", "ok"));
    } else {
        span.event(SignalEvent::InputRejected);
        span.fail(ErrorCategory::Platform);
        execution_context
            .span()
            .set_status(opentelemetry::trace::Status::error("failed"));
        execution_context
            .span()
            .set_attribute(opentelemetry::KeyValue::new("scxml.outcome", "failed"));
    }
    drop(guard);
    execution_context.span().end();
    result
}

fn execute_component_start_inner(
    component: &AuthenticatedComponent,
    ir: MachineIrSource<'_>,
    config: &WasmtimeHostConfig,
    providers: &TelemetryProviders,
    execution_context: Context,
) -> Result<ComponentExecution, HostError> {
    execute_component_start_with_runtime(component, ir, config, providers, execution_context)
}

/// Runs the capability graph's machine link: `start` and
/// shutdown of one authenticated machine under its own `machine-graph-link`
/// span. The machine has no invocation capability; the graph proves the
/// invoked-service component separately.
pub(super) fn execute_graph_machine(
    component: &AuthenticatedComponent,
    ir: MachineIrSource<'_>,
    config: &WasmtimeHostConfig,
    providers: &TelemetryProviders,
) -> Result<ComponentExecution, HostError> {
    let _lease = match providers.acquire_span() {
        Ok(lease) => lease,
        Err(error) => {
            return Err(error);
        }
    };
    let tracer = providers.tracer.tracer("scxml-host-wasmtime");
    let parent = crate::guest_spans::span_parent(&Context::current());
    let execution_span = tracer.start_with_context("machine-graph-link", &parent);
    let execution_context = parent.with_span(execution_span);
    let guard = execution_context.clone().attach();
    let component_span = tracer.start_with_context("component-execute", &execution_context);
    let component_context = execution_context.clone().with_span(component_span);
    let result = execute_component_start_with_runtime(
        component,
        ir,
        config,
        providers,
        component_context.clone(),
    );
    if result.is_ok() {
        component_context
            .span()
            .set_attribute(opentelemetry::KeyValue::new("scxml.outcome", "ok"));
    } else {
        component_context
            .span()
            .set_status(opentelemetry::trace::Status::error("failed"));
        component_context
            .span()
            .set_attribute(opentelemetry::KeyValue::new("scxml.outcome", "failed"));
    }
    component_context.span().end();
    drop(guard);
    execution_context.span().end();
    result
}

/// Validates, compiles, links, and instantiates one authenticated machine
/// component in a fresh bounded store.
///
/// The component's `ir-source` import must match the supply mode: a
/// runtime-mode (generic) machine must import it and a packaged machine must
/// not, so a raw template is never run as if it were packaged and a packaged
/// machine never silently ignores host-supplied IR.
#[allow(
    clippy::too_many_arguments,
    reason = "each session link (IR, data model, scripts, limits, telemetry, invocations) is an explicit capability"
)]
fn instantiate_machine(
    component: &AuthenticatedComponent,
    ir: MachineIrSource<'_>,
    data_model: Option<Box<dyn MachineDataModel>>,
    scripts: MachineScripts,
    config: &WasmtimeHostConfig,
    providers: &TelemetryProviders,
    execution_context: Context,
) -> Result<(wasmtime::Store<MachineStoreData>, bindings::Machine), HostError> {
    config.validate()?;
    if component.kind() != ArtifactKind::Machine {
        return Err(ArtifactError::ArtifactKindMismatch.into());
    }
    let component_bytes = component.bytes();
    if component_bytes.len() > config.component_bytes {
        return Err(ArtifactError::ArtifactTooLarge.into());
    }
    providers.record_component_measurement(ComponentMeasurement::Fuel, config.store.fuel);
    let engine = component_engine(config)?;
    let links = MachineLinks {
        machine_ir: ir.served_bytes(),
        data_model,
    };
    let component = wasmtime::component::Component::from_binary(&engine, component_bytes)?;
    let imports_ir_source = component
        .component_type()
        .get_import(&engine, IR_SOURCE_INTERFACE)
        .is_some();
    if imports_ir_source != ir.requires_ir_source_import() {
        return Err(ArtifactError::IrSourceModeMismatch.into());
    }
    let mut linker = Linker::<MachineStoreData>::new(&engine);
    bindings::Machine::add_to_linker::<_, HasSelf<MachineStoreData>>(&mut linker, |state| state)?;

    let limits = wasmtime::StoreLimitsBuilder::new()
        .memory_size(config.store.memory_bytes)
        .table_elements(config.store.table_elements)
        .instances(config.store.instances)
        .tables(config.store.tables)
        .memories(config.store.memories)
        .trap_on_grow_failure(true)
        .build();
    let limits = MeteredLimits::metered(limits, None, config.store.fuel);
    let budget = Arc::clone(limits.budget());
    let store_lease = providers.acquire_store()?;
    let mut store = wasmtime::Store::new(
        &engine,
        MachineStoreData {
            limits,
            providers: providers.clone(),
            guest_spans: GuestSpanTable::new(),
            execution_context,
            machine_ir: links.machine_ir,
            data_model: links.data_model,
            data_model_initialized: false,
            logs: Vec::new(),
            scripts: Some(ScriptHost::new(
                scripts,
                engine.clone(),
                config.store.clone(),
            )),
            session_id: String::new(),
            script_load_failure: None,
            session: None,
            namespaces: None,
            configuration: Vec::new(),
            _store_lease: store_lease,
        },
    );
    store.limiter(|state| &mut state.limits);
    budget.share(&mut store);
    store.set_fuel(config.store.fuel)?;
    record_store_limits(&config.store);

    let instance = linker.instantiate(&mut store, &component)?;
    let machine = bindings::Machine::new(&mut store, &instance)?;
    install_script_bundle(&mut store, &instance)?;
    Ok((store, machine))
}

/// Builds the linker every session-host machine store of `engine` uses.
pub(crate) fn machine_linker(
    engine: &wasmtime::Engine,
) -> Result<Linker<MachineStoreData>, HostError> {
    let mut linker = Linker::<MachineStoreData>::new(engine);
    bindings::Machine::add_to_linker::<_, HasSelf<MachineStoreData>>(&mut linker, |state| state)?;
    Ok(linker)
}

/// Session-scoped links of one session-host machine store.
pub(crate) struct SessionMachine {
    /// Runtime Machine IR served through `ir-source` (already bounded).
    pub(crate) machine_ir: Vec<u8>,
    /// The session's data-model component, when its document names one
    /// the host links.
    pub(crate) data_model: Option<crate::ComponentDataModel>,
    /// The embedder's external-script capability for this session.
    pub(crate) scripts: MachineScripts,
    /// `_sessionid` of the session.
    pub(crate) session_id: String,
    /// The session host's capability links.
    pub(crate) links: Box<SessionLinks>,
    /// The host's per-call fuel budget, shared by the machine store and
    /// every script store it runs.
    pub(crate) call_budget: Arc<CallBudget>,
    /// The session's extension-component instances, when any is bound.
    pub(crate) namespaces: Option<crate::extension::SessionNamespaces>,
}

/// Instantiates the session host's precompiled generic machine in a fresh
/// bounded store linked to `session`. The caller refuels the store before
/// each export call.
pub(crate) fn instantiate_session_machine(
    engine: &wasmtime::Engine,
    component: &wasmtime::component::Component,
    linker: &Linker<MachineStoreData>,
    config: &WasmtimeHostConfig,
    providers: &TelemetryProviders,
    execution_context: Context,
    session: SessionMachine,
) -> Result<(wasmtime::Store<MachineStoreData>, bindings::Machine), HostError> {
    let limits = wasmtime::StoreLimitsBuilder::new()
        .memory_size(config.store.memory_bytes)
        .table_elements(config.store.table_elements)
        .instances(config.store.instances)
        .tables(config.store.tables)
        .memories(config.store.memories)
        .trap_on_grow_failure(true)
        .build();
    let limits = MeteredLimits::new(limits, Arc::clone(&session.call_budget));
    let store_lease = providers.acquire_store()?;
    let mut store = wasmtime::Store::new(
        engine,
        MachineStoreData {
            limits,
            providers: providers.clone(),
            guest_spans: GuestSpanTable::new(),
            execution_context,
            machine_ir: session.machine_ir,
            data_model: session
                .data_model
                .map(|data_model| Box::new(data_model) as Box<dyn MachineDataModel>),
            data_model_initialized: false,
            logs: Vec::new(),
            scripts: Some(
                ScriptHost::new(session.scripts, engine.clone(), config.store.clone())
                    .sharing(&session.call_budget),
            ),
            session_id: session.session_id,
            script_load_failure: None,
            session: Some(session.links),
            namespaces: session.namespaces,
            configuration: Vec::new(),
            _store_lease: store_lease,
        },
    );
    store.limiter(|state| &mut state.limits);
    session.call_budget.share(&mut store);
    store.set_fuel(config.store.fuel)?;
    record_store_limits(&config.store);
    let instance = linker.instantiate(&mut store, component)?;
    let machine = bindings::Machine::new(&mut store, &instance)?;
    Ok((store, machine))
}

/// Reads a packaged machine's `script-bundle` export, when present, into
/// the session's script host before any session is constructed.
fn install_script_bundle(
    store: &mut wasmtime::Store<MachineStoreData>,
    instance: &wasmtime::component::Instance,
) -> Result<(), HostError> {
    let Some(bundle) =
        instance.get_export_index(&mut *store, None, scxml_script::SCRIPT_BUNDLE_INTERFACE)
    else {
        return Ok(());
    };
    let function = instance
        .get_export_index(&mut *store, Some(&bundle), "scripts")
        .ok_or_else(|| HostError::Component("invalid-script-bundle".into()))?;
    let scripts =
        instance.get_typed_func::<(), (Vec<(String, String, Vec<u8>)>,)>(&mut *store, &function)?;
    let (entries,) = scripts.call(&mut *store, ())?;
    store
        .data_mut()
        .scripts
        .as_mut()
        .ok_or_else(|| HostError::Component("invalid-script-bundle".into()))?
        .install_bundle(entries)
}

fn execute_component_start_with_runtime(
    component: &AuthenticatedComponent,
    ir: MachineIrSource<'_>,
    config: &WasmtimeHostConfig,
    providers: &TelemetryProviders,
    execution_context: Context,
) -> Result<ComponentExecution, HostError> {
    let (mut store, machine) = instantiate_machine(
        component,
        ir,
        None,
        MachineScripts::none(),
        config,
        providers,
        execution_context,
    )?;
    let api = machine.stateforward_scxml_machine_api().session();
    let capabilities = Resource::new_own(0);
    "wasmtime-proof".clone_into(&mut store.data_mut().session_id);
    let session = api.call_constructor(&mut store, "wasmtime-proof", capabilities)?;
    let step = api
        .call_start(&mut store, session)?
        .map_err(|failure| session_failure(&failure))?;
    // The execution proof's queue depth comes from the machine's
    // read-only `snapshot` export (`docs/session-snapshot.md`), which
    // charges no command and changes nothing. It follows a stable
    // `start`, so it cannot answer `not-stable`.
    let snapshot = api
        .call_snapshot(
            &mut store,
            session,
            crate::bindings::exports::stateforward::scxml::machine_api::SnapshotOptions {
                transitions: false,
                previews: false,
            },
        )?
        .map_err(|error| component_failure(&error))?;
    let queue_depth = u64::try_from(snapshot.internal.len()).unwrap_or(u64::MAX);
    providers.record_component_measurement(ComponentMeasurement::QueueDepth, queue_depth);
    api.call_shutdown(&mut store, session)?
        .map_err(|error| component_failure(&error))?;
    session.resource_drop(&mut store)?;

    let remaining_fuel = store.get_fuel()?;
    providers.record_component_measurement(ComponentMeasurement::Fuel, remaining_fuel);
    Ok(execution(step, queue_depth, remaining_fuel))
}

/// Runs one representative machine session: construct, `start`, then one
/// `send` per event (the machine runs each command to a stable configuration inside the
/// component), then shuts down and drops every resource.
///
/// The host links `data_model` as the session's data-model capability,
/// records `<log>` calls, and serves `ir` according to its mode; it never
/// selects transitions, interprets IR, or resumes the machine. `raise`,
/// `send`, `cancel`, and `execute-extension` capabilities answer `unavailable`.
/// Bounds: the host imposes none of its own on events or `<log>` records;
/// `scxml_engine::MAX_COMMANDS_PER_RUN` engine commands per call inside the
/// machine (a call that stops there fails as
/// [`HostError::UnboundedExecution`]), plus the store fuel and memory
/// limits in `config`.
///
/// # Examples
///
/// Running the generic machine in runtime-IR mode. The component and its
/// release manifest come from `scripts/verify-wasm-capabilities.sh`, so this
/// example is compiled but not run by `cargo test`; the
/// `machine_ir_modes` integration tests execute the same calls.
///
/// ```no_run
/// use scxml_host_wasmtime::{
///     ArtifactKind, ComponentSource, DataModelFailure, HostError, MachineDataModel,
///     MachineIrSource, TelemetryProviders, TrustedManifestDigest, WasmtimeHostConfig, authenticate_component,
///     execute_machine_session,
/// };
/// use scxml_types::{AssignPlacement, AssignSource, DataValue, Event};
///
/// /// A machine without a data model: every call is unavailable.
/// struct NoDataModel;
///
/// impl MachineDataModel for NoDataModel {
///     fn update_active_configuration(
///         &mut self,
///         _: &[String],
///         _: &[String],
///     ) -> Result<(), DataModelFailure> {
///         Ok(())
///     }
///     fn evaluate_condition(&mut self, _: &str, _: Option<&Event>) -> Result<bool, DataModelFailure> {
///         Err(DataModelFailure::Unavailable)
///     }
///     fn evaluate_value(&mut self, _: &str, _: Option<&Event>) -> Result<DataValue, DataModelFailure> {
///         Err(DataModelFailure::Unavailable)
///     }
///     fn assign(
///         &mut self,
///         _: &str,
///         _: &AssignSource,
///         _: &AssignPlacement,
///         _: Option<&Event>,
///     ) -> Result<(), DataModelFailure> {
///         Err(DataModelFailure::Unavailable)
///     }
///     fn execute(&mut self, _: &str, _: Option<&Event>) -> Result<(), DataModelFailure> {
///         Err(DataModelFailure::Unavailable)
///     }
///     fn evaluate_log(&mut self, _: &str, _: Option<&Event>) -> Result<String, DataModelFailure> {
///         Err(DataModelFailure::Unavailable)
///     }
/// }
///
/// let release = std::path::Path::new("target/component-proof/wasm-artifacts/release");
/// let manifest = std::fs::read(release.join("manifest.tsv"))?;
/// let machine = std::fs::read(release.join("scxml_machine.virtualized.wasm"))?;
/// let trusted = TrustedManifestDigest::from_hex("<pinned release manifest SHA-256>")
///     .map_err(HostError::from)?;
/// let component = authenticate_component(
///     &manifest,
///     trusted,
///     ArtifactKind::Machine,
///     "scxml_machine.virtualized.wasm",
///     &machine,
/// )?;
/// let ir = std::fs::read("machine.scxml-ir.json")?;
/// let trace = execute_machine_session(
///     ComponentSource::Authenticated(&component),
///     MachineIrSource::Runtime(&ir),
///     &["go"],
///     Box::new(NoDataModel),
///     &WasmtimeHostConfig::default(),
///     &TelemetryProviders::new(),
/// )?;
/// // One observable call result for `start` and one per sent event, each already
/// // run to a stable configuration inside the machine.
/// assert_eq!(trace.results.len(), 2);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Errors
///
/// Returns [`HostError::Component`] with the closed code when construction
/// yields a failed session (for example hostile IR) or a command fails;
/// artifact, mode-mismatch, limit, and Wasmtime failures (including fuel
/// exhaustion) as their typed variants. No store or resource outlives the
/// call.
pub fn execute_machine_session(
    component: ComponentSource<'_>,
    ir: MachineIrSource<'_>,
    events: &[&str],
    data_model: Box<dyn MachineDataModel>,
    config: &WasmtimeHostConfig,
    providers: &TelemetryProviders,
) -> Result<MachineSessionTrace, HostError> {
    execute_machine_session_with_scripts(
        component,
        ir,
        events,
        data_model,
        MachineScripts::none(),
        config,
        providers,
    )
}

/// [`execute_machine_session`] with an explicit external-script capability.
///
/// `scripts` supplies the runtime `script-loader` and its trust policy
/// ([`MachineScripts::runtime`]); a packaged machine's own `script-bundle`
/// is always read and serves pinned references. The machine decides which
/// scripts load at construction (a load failure yields a failed session,
/// reported as [`HostError::Component`]) and when they run; the host only
/// resolves, authenticates, links them to `data_model`, and runs them.
///
/// # Errors
///
/// As [`execute_machine_session`].
pub fn execute_machine_session_with_scripts(
    component: ComponentSource<'_>,
    ir: MachineIrSource<'_>,
    events: &[&str],
    data_model: Box<dyn MachineDataModel>,
    scripts: MachineScripts,
    config: &WasmtimeHostConfig,
    providers: &TelemetryProviders,
) -> Result<MachineSessionTrace, HostError> {
    let parent = crate::guest_spans::span_parent(&Context::current());
    let span =
        scxml_telemetry::observe(Module::HostWasmtime, Operation::HostWasmtimeMachineSession);
    let result = (|| {
        let component = component.authenticated()?;
        let _lease = providers.acquire_span()?;
        let tracer = providers.tracer.tracer("scxml-host-wasmtime");
        let execution_context =
            parent.with_span(tracer.start_with_context("machine-session", &parent));
        let guard = execution_context.clone().attach();
        let result = run_machine_session(
            component,
            ir,
            events,
            data_model,
            scripts,
            config,
            providers,
            execution_context.clone(),
        );
        if result.is_err() {
            execution_context
                .span()
                .set_status(opentelemetry::trace::Status::error("failed"));
        }
        drop(guard);
        execution_context.span().end();
        result
    })();
    if let Err(error) = result.as_ref() {
        span.event(SignalEvent::InputRejected);
        span.fail(host_error_category(error));
    } else {
        span.event(SignalEvent::TransformationCompleted);
    }
    result
}

#[allow(
    clippy::too_many_arguments,
    reason = "each session link (IR, data model, scripts, limits, telemetry, invocations) is an explicit capability"
)]
fn run_machine_session(
    component: &AuthenticatedComponent,
    ir: MachineIrSource<'_>,
    events: &[&str],
    data_model: Box<dyn MachineDataModel>,
    scripts: MachineScripts,
    config: &WasmtimeHostConfig,
    providers: &TelemetryProviders,
    execution_context: Context,
) -> Result<MachineSessionTrace, HostError> {
    let (mut store, machine) = instantiate_machine(
        component,
        ir,
        Some(data_model),
        scripts,
        config,
        providers,
        execution_context,
    )?;
    let api = machine.stateforward_scxml_machine_api().session();
    "machine-session".clone_into(&mut store.data_mut().session_id);
    let session = api.call_constructor(&mut store, "machine-session", Resource::new_own(0))?;
    if let Some(cause) = api.call_failure(&mut store, session)? {
        session.resource_drop(&mut store)?;
        return Err(component_failure(&cause));
    }
    let mut steps = Vec::with_capacity(events.len().saturating_add(1));
    let first = api
        .call_start(&mut store, session)?
        .map_err(|failure| session_failure(&failure))?;
    steps.push(drain_step(&mut store, first)?);
    for name in events {
        let step = api
            .call_send(&mut store, session, &external_event(name))?
            .map_err(|failure| session_failure(&failure))?;
        steps.push(drain_step(&mut store, step)?);
    }
    api.call_shutdown(&mut store, session)?
        .map_err(|error| component_failure(&error))?;
    session.resource_drop(&mut store)?;
    let remaining_fuel = store.get_fuel()?;
    providers.record_component_measurement(ComponentMeasurement::Fuel, remaining_fuel);
    Ok(MachineSessionTrace {
        results: steps,
        remaining_fuel,
    })
}

/// Projects one command's observable call result and takes the logs it emitted.
///
/// The machine runs every command to a stable configuration inside the component under
/// the engine's per-call command bound; the host never resumes. A call that
/// stopped at that bound (`stable` false while running) fails closed as
/// [`HostError::UnboundedExecution`].
fn drain_step(
    store: &mut wasmtime::Store<MachineStoreData>,
    step: CallResult,
) -> Result<MachineCallTrace, HostError> {
    if !step.stable && step.status == Status::Running {
        Err(HostError::UnboundedExecution)
    } else {
        Ok(MachineCallTrace {
            status: execution(step.clone(), 0, 0).status,
            active_configuration: step.active_configuration,
            effect_count: step.effect_count,
            logs: std::mem::take(&mut store.data_mut().logs),
        })
    }
}

fn execution(step: CallResult, queue_depth: u64, remaining_fuel: u64) -> ComponentExecution {
    ComponentExecution {
        status: match step.status {
            Status::Ready => ComponentStatus::Ready,
            Status::Running => ComponentStatus::Running,
            Status::Finished => ComponentStatus::Finished,
            Status::Failed => ComponentStatus::Failed,
            Status::ShutDown => ComponentStatus::ShutDown,
        },
        active_configuration: step.active_configuration,
        effect_count: step.effect_count,
        remaining_fuel,
        queue_depth,
    }
}

/// Maps a machine `session-failure` to the host error for its closed cause.
///
/// The one-shot proof host shuts the session down after `start`, and its
/// capabilities commit no host-side effects, so there is nothing to reconcile
/// from `committed-effects`; the closed cause is what the caller receives.
fn session_failure(failure: &SessionFailure) -> HostError {
    component_failure(&failure.error)
}

fn component_failure(error: &ComponentError) -> HostError {
    if error.version != ErrorVersion::V1 {
        return HostError::Component("unknown-error-version".into());
    }
    if error
        .detail
        .as_ref()
        .is_some_and(|detail| detail.len() > 160)
    {
        return HostError::Component("invalid-error-detail".into());
    }
    HostError::Component(error_code_label(error.code).into())
}

/// Closed WIT `error-code` label (bounded; never a payload).
pub(crate) const fn error_code_label(code: ErrorCode) -> &'static str {
    match code {
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
}

/// A closed WIT error with static, payload-free detail.
pub(crate) fn component_error(code: ErrorCode, detail: &'static str) -> ComponentError {
    ComponentError {
        version: ErrorVersion::V1,
        code,
        detail: Some(detail.into()),
    }
}

fn unavailable() -> ComponentError {
    ComponentError {
        version: ErrorVersion::V1,
        code: ErrorCode::Unavailable,
        detail: Some("capability is not linked for this session".into()),
    }
}

fn host_error_category(error: &HostError) -> ErrorCategory {
    match error {
        HostError::Release(crate::ReleaseError::Read { .. })
        | HostError::TelemetryShutdown
        | HostError::TelemetryInUse
        | HostError::Wasmtime(_) => ErrorCategory::Platform,
        HostError::Artifact(_)
        | HostError::InvalidLimit(_)
        | HostError::UnboundedExecution
        | HostError::TraceContext(_)
        | HostError::Release(_) => ErrorCategory::Validation,
        HostError::Component(_) => ErrorCategory::Execution,
    }
}

fn to_trace_context(context: &WitTraceContext) -> TraceContext {
    TraceContext {
        traceparent: Some(context.traceparent.clone()),
        tracestate: context.tracestate.clone(),
    }
}

fn to_wit_trace_context(context: &TraceContext) -> Option<WitTraceContext> {
    context
        .traceparent
        .as_ref()
        .map(|traceparent| WitTraceContext {
            traceparent: traceparent.clone(),
            tracestate: context.tracestate.clone(),
            baggage: None,
            baggage_policy: BaggagePolicy::Drop,
        })
}

pub(crate) const fn operation_name(operation: telemetry::Operation) -> &'static str {
    match operation {
        telemetry::Operation::Initialize => "initialize",
        telemetry::Operation::EvaluateCondition => "evaluate-condition",
        telemetry::Operation::EvaluateValue => "evaluate-value",
        telemetry::Operation::Assign => "assign",
        telemetry::Operation::Execute => "execute",
        telemetry::Operation::SystemContext => "system-context",
        telemetry::Operation::Start => "start",
        telemetry::Operation::Send => "send",
        telemetry::Operation::Cancel => "cancel",
        telemetry::Operation::DrainDue => "drain-due",
        telemetry::Operation::TypeUri => "type-uri",
        telemetry::Operation::Resume => "resume",
        telemetry::Operation::Shutdown => "shutdown",
        telemetry::Operation::CheckSyntax => "check-syntax",
        telemetry::Operation::CheckEffects => "check-effects",
        telemetry::Operation::StartInvoked => "start-invoked",
        telemetry::Operation::Deliver => "deliver",
        telemetry::Operation::Exit => "exit",
        telemetry::Operation::Dispatch => "dispatch",
        telemetry::Operation::RouteDue => "route-due",
        telemetry::Operation::Encode => "encode",
        telemetry::Operation::Diff => "diff",
        telemetry::Operation::CommunicationError => "communication-error",
        telemetry::Operation::ScheduleProcessor => "schedule-processor",
    }
}

pub(crate) const fn outcome_name(outcome: telemetry::Outcome) -> &'static str {
    match outcome {
        telemetry::Outcome::Ok => "ok",
        telemetry::Outcome::Rejected => "rejected",
        telemetry::Outcome::Failed => "failed",
    }
}

const fn component_measurement(kind: telemetry::MeasurementKind) -> ComponentMeasurement {
    match kind {
        telemetry::MeasurementKind::Effects => ComponentMeasurement::Effects,
        telemetry::MeasurementKind::InputBytes => ComponentMeasurement::InputBytes,
        telemetry::MeasurementKind::SnapshotBytes => ComponentMeasurement::SnapshotBytes,
        telemetry::MeasurementKind::QueueDepth => ComponentMeasurement::QueueDepth,
        telemetry::MeasurementKind::Fuel => ComponentMeasurement::Fuel,
    }
}

pub(crate) const fn event_name(event: telemetry::SignalEvent) -> &'static str {
    match event {
        telemetry::SignalEvent::TypeUriReturned => "type-uri-returned",
        telemetry::SignalEvent::SendAccepted => "send-accepted",
        telemetry::SignalEvent::SendRejected => "send-rejected",
        telemetry::SignalEvent::CancelCompleted => "cancel-completed",
        telemetry::SignalEvent::CancelRejected => "cancel-rejected",
        telemetry::SignalEvent::DueDrained => "due-drained",
        telemetry::SignalEvent::InputAccepted => "input-accepted",
        telemetry::SignalEvent::InputRejected => "input-rejected",
        telemetry::SignalEvent::TransformationCompleted => "transformation-completed",
        telemetry::SignalEvent::ArtifactAccepted => "artifact-accepted",
        telemetry::SignalEvent::ArtifactProduced => "artifact-produced",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_sdk::metrics::SdkMeterProvider;
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};

    fn test_store_data(
        providers: TelemetryProviders,
        execution_context: Context,
    ) -> MachineStoreData {
        let store_lease = providers
            .acquire_store()
            .expect("active providers grant the test store a lifecycle lease");
        MachineStoreData {
            limits: MeteredLimits::new(
                wasmtime::StoreLimitsBuilder::new().build(),
                CallBudget::new(u64::MAX),
            ),
            providers,
            guest_spans: GuestSpanTable::new(),
            execution_context,
            machine_ir: Vec::new(),
            data_model: None,
            data_model_initialized: false,
            logs: Vec::new(),
            scripts: None,
            session_id: String::new(),
            script_load_failure: None,
            session: None,
            namespaces: None,
            configuration: Vec::new(),
            _store_lease: store_lease,
        }
    }

    #[test]
    fn component_guest_start_rejects_malformed_parent_without_root_fallback() {
        let providers = TelemetryProviders::new();
        let mut data = test_store_data(providers, Context::new());
        let malformed = WitTraceContext {
            traceparent: "00-not-a-trace".into(),
            tracestate: None,
            baggage: None,
            baggage_policy: BaggagePolicy::Drop,
        };

        let (span_id, propagated) =
            telemetry::Host::start(&mut data, telemetry::Operation::Start, Some(malformed));

        assert_eq!(span_id, 0);
        assert!(propagated.is_none());
        assert!(data.guest_spans.is_empty());
    }

    #[test]
    fn component_guest_span_is_direct_execution_child() {
        let exporter = InMemorySpanExporter::default();
        let tracer = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let providers =
            TelemetryProviders::from_providers(tracer, SdkMeterProvider::builder().build());
        let execution_span = providers
            .tracer
            .tracer("scxml-host-wasmtime-test")
            .start_with_context("component-execute", &Context::new());
        let execution_context = Context::new().with_span(execution_span);
        let execution_trace_id = execution_context.span().span_context().trace_id();
        let execution_span_id = execution_context.span().span_context().span_id();
        let mut data = test_store_data(providers.clone(), execution_context.clone());

        let (guest_id, propagated) =
            telemetry::Host::start(&mut data, telemetry::Operation::Start, None);
        assert_ne!(guest_id, 0);
        assert!(propagated.is_some());
        telemetry::Host::end(&mut data, guest_id, telemetry::Outcome::Ok);
        execution_context.span().end();
        providers.force_flush().expect("in-memory spans flush");

        let spans = exporter
            .get_finished_spans()
            .expect("in-memory spans are readable");
        let guest = spans
            .iter()
            .find(|span| span.name == "start")
            .expect("guest start span is exported");
        assert_eq!(guest.span_context.trace_id(), execution_trace_id);
        assert_eq!(guest.parent_span_id, execution_span_id);
    }

    #[test]
    fn component_failure_projects_only_the_closed_code() {
        let failure = ComponentError {
            version: ErrorVersion::V1,
            code: ErrorCode::DuplicateId,
            detail: Some("untrusted identifier must not escape".into()),
        };

        let error = component_failure(&failure).to_string();

        assert!(error.contains("duplicate-id"));
        assert!(!error.contains("untrusted identifier"));
    }

    /// Canonical IR for the two-state example (`ready --finish--> done`)
    /// the lifecycle tests feed through runtime-mode `ir-source`.
    fn example_ir() -> Vec<u8> {
        example_machine()
            .to_json()
            .expect("the example machine is valid canonical IR")
    }

    /// The two-state example machine of [`example_ir`].
    fn example_machine() -> scxml_compiler_core::MachineIr {
        use scxml_compiler_core::{
            Binding, InitialIr, MachineIr, StateIr, StateKind, TransitionIr, TransitionType,
        };
        let state = |document_order, id: &str, kind, transitions| StateIr {
            document_order,
            id: Some(id.into()),
            kind,
            parent: None,
            children: Vec::new(),
            initial: None,
            onentry: Vec::new(),
            onexit: Vec::new(),
            transitions,
            data: Vec::new(),
            invokes: Vec::new(),
            donedata: None,
        };
        MachineIr {
            version: scxml_compiler_core::MACHINE_IR_VERSION,
            name: Some("example".into()),
            data_model: None,
            binding: Binding::Early,
            initial: InitialIr {
                document_order: 0,
                targets: vec![0],
                actions: Vec::new(),
            },
            states: vec![
                state(1, "ready", StateKind::Atomic, vec![0]),
                state(3, "done", StateKind::Final, Vec::new()),
            ],
            transitions: vec![TransitionIr {
                document_order: 2,
                source: 0,
                targets: vec![1],
                events: vec!["finish".into()],
                cond: None,
                transition_type: TransitionType::External,
                actions: Vec::new(),
                event_schema: None,
            }],
            data: Vec::new(),
            scripts: Vec::new(),
            extension_namespaces: Vec::new(),
        }
    }

    /// Builds and authenticates the release machine artifact through the
    /// canonical capability script (shared with the integration proofs).
    fn machine_component() -> AuthenticatedComponent {
        static COMPONENT: std::sync::OnceLock<AuthenticatedComponent> = std::sync::OnceLock::new();
        COMPONENT.get_or_init(build_machine_component).clone()
    }

    fn build_machine_component() -> AuthenticatedComponent {
        use sha2::Digest as _;

        let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("host crate has a workspace parent");
        let release = crate::prebuilt::release_dir().unwrap_or_else(|| {
            let target = workspace.join("target/component-proof");
            let status = std::process::Command::new("sh")
                .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
                .arg("scxml_machine")
                .current_dir(workspace)
                // The Cargo target every host suite builds the release components
                // in, so they compile once; this suite's artifacts stay its own.
                .env(
                    "CARGO_TARGET_DIR",
                    workspace.join("target/session-host-proof"),
                )
                // This suite's own artifact directory: suites sharing the Cargo
                // target never rewrite a manifest another suite is reading.
                .env(
                    "SCXML_WASM_ARTIFACTS_DIR",
                    target.join("wasm-artifacts/component-lifecycle"),
                )
                .status()
                .expect("lifecycle proof can invoke the capability script");
            assert!(status.success(), "release capability proof must pass");
            target.join("wasm-artifacts/component-lifecycle")
        });
        let manifest = std::fs::read(release.join("manifest.tsv"))
            .expect("capability script writes its manifest");
        let digest = crate::TrustedManifestDigest::from_hex(&format!(
            "{:x}",
            sha2::Sha256::digest(&manifest)
        ))
        .expect("manifest digest is canonical");
        let bytes = std::fs::read(release.join("scxml_machine.virtualized.wasm"))
            .expect("machine artifact is readable");
        crate::authenticate_component(
            &manifest,
            digest,
            ArtifactKind::Machine,
            "scxml_machine.virtualized.wasm",
            &bytes,
        )
        .expect("script artifact authenticates")
    }

    #[test]
    fn machine_session_failure_contract_holds_across_the_component_boundary() {
        let component = machine_component();
        let providers = TelemetryProviders::new();
        let config = WasmtimeHostConfig::default();
        let (mut store, machine) = instantiate_machine(
            &component,
            MachineIrSource::Runtime(&example_ir()),
            None,
            MachineScripts::none(),
            &config,
            &providers,
            Context::new(),
        )
        .expect("authenticated machine instantiates");
        let api = machine.stateforward_scxml_machine_api().session();

        // An invalid session id yields a failed session whose cause is kept.
        let invalid = api
            .call_constructor(&mut store, "", Resource::new_own(0))
            .expect("constructor never traps");
        assert_eq!(
            api.call_status(&mut store, invalid).expect("status call"),
            Status::Failed
        );
        let cause = api
            .call_failure(&mut store, invalid)
            .expect("failure call")
            .expect("failed session reports its cause");
        assert_eq!(cause.code, ErrorCode::InvalidInput);
        assert_eq!(
            cause.detail.as_deref(),
            Some("machine name input is invalid"),
            "the typed boundary cause must survive"
        );
        let failure = api
            .call_start(&mut store, invalid)
            .expect("start call")
            .expect_err("failed sessions reject start");
        assert_eq!(failure.error.code, ErrorCode::Protocol);
        assert_eq!(failure.committed_effects, 0);
        assert!(failure.session_failed);
        assert!(
            api.call_configuration(&mut store, invalid)
                .expect("configuration call")
                .is_empty()
        );
        api.call_shutdown(&mut store, invalid)
            .expect("shutdown call")
            .expect("shutdown always succeeds");
        assert_eq!(
            api.call_status(&mut store, invalid).expect("status call"),
            Status::ShutDown
        );
        assert!(
            api.call_failure(&mut store, invalid)
                .expect("failure call")
                .is_none()
        );
        invalid.resource_drop(&mut store).expect("session drops");
    }

    #[test]
    fn machine_session_lifecycle_misuse_is_a_rejection_across_the_component_boundary() {
        let component = machine_component();
        let providers = TelemetryProviders::new();
        let config = WasmtimeHostConfig::default();
        let (mut store, machine) = instantiate_machine(
            &component,
            MachineIrSource::Runtime(&example_ir()),
            None,
            MachineScripts::none(),
            &config,
            &providers,
            Context::new(),
        )
        .expect("authenticated machine instantiates");
        let api = machine.stateforward_scxml_machine_api().session();

        // Lifecycle misuse is a rejection that leaves a valid session usable.
        let session = api
            .call_constructor(&mut store, "lifecycle-proof", Resource::new_own(0))
            .expect("constructor never traps");
        assert_eq!(
            api.call_status(&mut store, session).expect("status call"),
            Status::Ready
        );
        let early = api
            .call_resume(&mut store, session)
            .expect("resume call")
            .expect_err("resume before start is rejected");
        assert_eq!(early.error.code, ErrorCode::Protocol);
        assert!(!early.session_failed);
        let step = api
            .call_start(&mut store, session)
            .expect("start call")
            .expect("valid session starts");
        assert_eq!(step.status, Status::Running);
        let again = api
            .call_start(&mut store, session)
            .expect("start call")
            .expect_err("second start is rejected");
        assert_eq!(again.error.code, ErrorCode::Protocol);
        assert_eq!(again.committed_effects, 0);
        assert!(!again.session_failed);
        assert_eq!(
            api.call_status(&mut store, session).expect("status call"),
            Status::Running,
            "a rejection must not change the session"
        );
        api.call_shutdown(&mut store, session)
            .expect("shutdown call")
            .expect("shutdown succeeds");
        let resumed = api
            .call_resume(&mut store, session)
            .expect("resume call")
            .expect_err("shut-down sessions reject resume");
        assert!(!resumed.session_failed);
        session.resource_drop(&mut store).expect("session drops");
    }

    /// One `ready` session of `ir` in its own store.
    fn ready_session(
        component: &AuthenticatedComponent,
        ir: &[u8],
    ) -> (
        wasmtime::Store<MachineStoreData>,
        bindings::Machine,
        wasmtime::component::ResourceAny,
    ) {
        let (mut store, machine) = instantiate_machine(
            component,
            MachineIrSource::Runtime(ir),
            None,
            MachineScripts::none(),
            &WasmtimeHostConfig::default(),
            &TelemetryProviders::new(),
            Context::new(),
        )
        .expect("authenticated machine instantiates");
        let session = machine
            .stateforward_scxml_machine_api()
            .session()
            .call_constructor(&mut store, "session-proof", Resource::new_own(0))
            .expect("constructor never traps");
        (store, machine, session)
    }

    #[test]
    fn send_rejects_invocation_provenance_and_non_external_types() {
        use crate::bindings::stateforward::scxml::types::EventType;
        let component = machine_component();
        let ir = example_ir();
        let (mut store, machine, session) = ready_session(&component, &ir);
        let api = machine.stateforward_scxml_machine_api().session();
        api.call_start(&mut store, session)
            .expect("start call")
            .expect("the session starts");
        // Only the invocation path (`deliver-from-invocation`) stamps an
        // invoke id, and only the machine raises internal events (SCXML
        // 6.4, 5.10.1): `send` refuses them as forgeries.
        let forged = [
            (EventType::External, Some("kid")),
            (EventType::Platform, None),
            (EventType::Platform, Some("kid")),
            (EventType::Internal, None),
            (EventType::Internal, Some("kid")),
        ];
        for (event_type, invoke_id) in forged {
            let mut event = external_event("finish");
            event.event_type = event_type;
            event.invoke_id = invoke_id.map(str::to_owned);
            let rejected = api
                .call_send(&mut store, session, &event)
                .expect("send call")
                .expect_err("a forged external event is rejected");
            assert_eq!(rejected.error.code, ErrorCode::InvalidInput);
            assert_eq!(rejected.committed_effects, 0);
            assert!(
                !rejected.session_failed,
                "a rejection leaves the session usable"
            );
            assert_eq!(
                api.call_configuration(&mut store, session)
                    .expect("configuration call"),
                vec!["ready".to_owned()]
            );
        }
        // A platform error raised after its macrostep (a late BasicHTTP
        // `error.communication`) enters the internal queue through
        // `communication-error` (SCXML 3.12.2, 6.2.4).
        let late = api
            .call_communication_error(&mut store, session, Some("sendid_0"))
            .expect("communication-error call")
            .expect("the error is raised");
        assert_eq!(late.status, Status::Running);
        assert!(late.stable);
        let step = api
            .call_send(&mut store, session, &external_event("finish"))
            .expect("send call")
            .expect("an external event is accepted");
        assert_eq!(step.status, Status::Finished);
    }
}
