//! Thin Wasmtime linker for the `BasicHTTP` Event I/O Processor component.
//!
//! The component (`stateforward:scxml-basichttp/basichttp`) owns every
//! `BasicHTTP` decision. This module only instantiates it under the host's
//! resource and fuel limits, forwards its `http-transport` import to an
//! embedder-supplied [`BasicHttpTransport`], and maps its telemetry imports
//! onto the host-owned OpenTelemetry providers. It never encodes, decodes,
//! or routes a message itself and opens no network connection: the
//! embedder decides what "sending" means (a real HTTP client, or an
//! in-process loopback in tests).

use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, TraceContext};
use wasmtime::component::{HasSelf, Linker};

use std::sync::Arc;

use crate::basichttp_bindings::Basichttp;
use crate::basichttp_bindings::stateforward::scxml_basichttp::telemetry as guest_telemetry;
use crate::call_budget::CallBudget;
use crate::capability_graph::{
    CommonStoreData, common_store_data, record_scxml_observation, validate_component,
};
use crate::{
    ArtifactKind, AuthenticatedComponent, ComponentMeasurement, HostError, TelemetryProviders,
    WasmtimeHostConfig, component_engine, record_store_limits,
};

/// Canonical WIT records of `stateforward:scxml-basichttp/types@0.2.0`.
pub use crate::basichttp_bindings::stateforward::scxml_basichttp::types as basichttp_types;

use basichttp_types::{
    ComponentError, Delivery, HttpRequest, HttpResponse, OutboundSend, Received,
};

/// The transport returned no response (unreachable address, refused
/// connection, or transport failure).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransportUnavailable;

/// The embedder-supplied `http-transport` capability.
///
/// Called synchronously, at most once per `send`, from inside the
/// component's `send` export. Implementations carry one request and return
/// the recipient's response; they must not call back into the same
/// [`BasicHttpProcessor`] (the component is not re-entrant).
pub trait BasicHttpTransport: 'static {
    /// Performs one request.
    ///
    /// # Errors
    ///
    /// [`TransportUnavailable`] when no response was obtained; the processor
    /// reports it to the sender as `unavailable` (`error.communication`).
    fn post(&mut self, request: HttpRequest) -> Result<HttpResponse, TransportUnavailable>;
}

struct BasicHttpStoreData<T> {
    common: CommonStoreData,
    transport: T,
}

/// One instantiated `BasicHTTP` processor component linked to transport `T`.
///
/// The processor owns its store; every call is bounded by the configured
/// fuel and memory limits and returns the component's typed result, or a
/// [`HostError`] for a trap, fuel exhaustion, or a telemetry failure.
pub struct BasicHttpProcessor<T: BasicHttpTransport> {
    store: wasmtime::Store<BasicHttpStoreData<T>>,
    bindings: Basichttp,
}

impl<T: BasicHttpTransport> BasicHttpProcessor<T> {
    /// Instantiates the authenticated component with `transport` as its
    /// only outbound capability.
    ///
    /// # Errors
    ///
    /// Rejects a component of another kind or over the byte bound, invalid
    /// host limits, and components importing anything beyond the
    /// `basichttp` world.
    pub fn instantiate(
        component: &AuthenticatedComponent,
        config: &WasmtimeHostConfig,
        providers: &TelemetryProviders,
        transport: T,
    ) -> Result<Self, HostError> {
        let span = scxml_telemetry::observe(
            Module::HostWasmtime,
            Operation::HostWasmtimeBasicHttpInstantiate,
        );
        let result = (|| {
            config.validate()?;
            validate_component(component, ArtifactKind::IoProcessorBasicHttp, config)?;
            let engine = component_engine(config)?;
            let component =
                wasmtime::component::Component::from_binary(&engine, component.bytes())?;
            Self::instantiate_compiled(&engine, &component, config, providers, transport, None)
        })();
        complete(&span, &result);
        result
    }

    /// Instantiates one precompiled `BasicHTTP` processor in a fresh bounded
    /// store of `engine` (the session host's process-wide compiled
    /// component).
    pub(crate) fn instantiate_compiled(
        engine: &wasmtime::Engine,
        component: &wasmtime::component::Component,
        config: &WasmtimeHostConfig,
        providers: &TelemetryProviders,
        transport: T,
        call_budget: Option<&Arc<CallBudget>>,
    ) -> Result<Self, HostError> {
        let mut linker = Linker::<BasicHttpStoreData<T>>::new(engine);
        Basichttp::add_to_linker::<_, HasSelf<BasicHttpStoreData<T>>>(&mut linker, |state| state)?;
        let mut store = wasmtime::Store::new(
            engine,
            BasicHttpStoreData {
                common: common_store_data(config, providers, call_budget)?,
                transport,
            },
        );
        store.limiter(|state| &mut state.common.limits);
        let budget = Arc::clone(store.data().common.limits.budget());
        budget.share(&mut store);
        store.set_fuel(config.store.fuel)?;
        record_store_limits(&config.store);
        let bindings = Basichttp::instantiate(&mut store, component, &linker)?;
        Ok(Self { store, bindings })
    }

    /// The processor's `BasicHTTP` type aliases (`basichttp-api.aliases`).
    ///
    /// # Errors
    ///
    /// A trap, fuel exhaustion, or telemetry failure.
    pub fn aliases(&mut self) -> Result<Vec<String>, HostError> {
        self.bindings
            .stateforward_scxml_basichttp_basichttp_api()
            .call_aliases(&mut self.store)
            .map_err(HostError::from)
            .and_then(|aliases| self.take_failures().map(|()| aliases))
    }

    /// Resets the store's instruction fuel to `fuel` before one call.
    pub(crate) fn refuel(&mut self, fuel: u64) -> Result<(), HostError> {
        self.store.data().common.limits.set_call_fuel(fuel);
        self.store.set_fuel(fuel).map_err(HostError::from)
    }

    /// The processor's type URI.
    ///
    /// # Errors
    ///
    /// A trap, fuel exhaustion, or telemetry failure.
    pub fn type_uri(&mut self) -> Result<String, HostError> {
        self.bindings
            .stateforward_scxml_basichttp_basichttp_api()
            .call_type_uri(&mut self.store)
            .map_err(HostError::from)
            .and_then(|uri| self.take_failures().map(|()| uri))
    }

    /// Encodes and performs one send through the transport.
    ///
    /// Delivery is at most once (`basichttp.wit` `basichttp-api.send`): the
    /// component calls the transport at most once per send and never
    /// retries, and this method never retries either.
    ///
    /// # Errors
    ///
    /// The outer error is a host failure; the inner one the component's
    /// typed send failure (see `basichttp.wit` `error-code`). An inner
    /// `unavailable` after the transport was called (a non-2xx answer or a
    /// transport failure) does not prove the recipient did not process the
    /// message. A host failure (trap, fuel exhaustion, or telemetry
    /// failure) can occur after the transport call, so delivery is unknown
    /// on any outer error: the request may already have been delivered, and
    /// a caller that resends accepts possible duplicate delivery.
    pub fn send(
        &mut self,
        request: &OutboundSend,
    ) -> Result<Result<(), ComponentError>, HostError> {
        let span =
            scxml_telemetry::observe(Module::HostWasmtime, Operation::HostWasmtimeBasicHttpSend);
        let result = self
            .bindings
            .stateforward_scxml_basichttp_basichttp_api()
            .call_send(&mut self.store, request)
            .map_err(HostError::from)
            .and_then(|sent| self.take_failures().map(|()| sent));
        complete(&span, &result);
        result
    }

    /// Validates and encodes one send exactly as [`Self::send`] would
    /// before calling the transport, without posting it
    /// (`basichttp-api.check`; a missing target is not checked). Pure.
    ///
    /// # Errors
    ///
    /// The outer error is a host failure (trap, fuel exhaustion, or
    /// telemetry failure); the inner one the component's `invalid-input` or
    /// `payload-too-large`.
    pub fn check(
        &mut self,
        request: &OutboundSend,
    ) -> Result<Result<(), ComponentError>, HostError> {
        self.bindings
            .stateforward_scxml_basichttp_basichttp_api()
            .call_check(&mut self.store, request)
            .map_err(HostError::from)
            .and_then(|checked| self.take_failures().map(|()| checked))
    }

    /// Validates and decodes one inbound message addressed to a session's
    /// `BasicHTTP` location. Nothing is enqueued: the embedder adds the
    /// event to the addressed session and answers with [`Self::respond`].
    ///
    /// # Errors
    ///
    /// The outer error is a host failure; the inner one the rejection to
    /// pass to [`Self::respond`].
    pub fn receive(
        &mut self,
        request: &HttpRequest,
    ) -> Result<Result<Received, basichttp_types::Rejection>, HostError> {
        let span = scxml_telemetry::observe(
            Module::HostWasmtime,
            Operation::HostWasmtimeBasicHttpReceive,
        );
        let result = self
            .bindings
            .stateforward_scxml_basichttp_basichttp_api()
            .call_receive(&mut self.store, request)
            .map_err(HostError::from)
            .and_then(|received| self.take_failures().map(|()| received));
        complete(&span, &result);
        result
    }

    /// The response the component chooses for one delivery outcome.
    ///
    /// # Errors
    ///
    /// A trap, fuel exhaustion, or telemetry failure.
    pub fn respond(&mut self, delivery: Delivery) -> Result<HttpResponse, HostError> {
        self.bindings
            .stateforward_scxml_basichttp_basichttp_api()
            .call_respond(&mut self.store, delivery)
            .map_err(HostError::from)
            .and_then(|response| self.take_failures().map(|()| response))
    }

    /// Remaining deterministic fuel of the processor store.
    ///
    /// # Errors
    ///
    /// Fuel accounting is unavailable.
    pub fn remaining_fuel(&self) -> Result<u64, HostError> {
        self.store.get_fuel().map_err(HostError::from)
    }

    /// The embedder's transport.
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.store.data_mut().transport
    }

    fn take_failures(&mut self) -> Result<(), HostError> {
        self.store.data_mut().common.take_telemetry_failure()
    }
}

fn complete<V>(span: &scxml_telemetry::FunctionSpan, result: &Result<V, HostError>) {
    if result.is_ok() {
        span.event(SignalEvent::TransformationCompleted);
    } else {
        span.event(SignalEvent::InputRejected);
        span.fail(ErrorCategory::Platform);
    }
}

impl<T: BasicHttpTransport> basichttp_types::Host for BasicHttpStoreData<T> {}

impl<T: BasicHttpTransport>
    crate::basichttp_bindings::stateforward::scxml_basichttp::http_transport::Host
    for BasicHttpStoreData<T>
{
    fn post(&mut self, request: HttpRequest) -> Result<HttpResponse, ()> {
        let span = scxml_telemetry::observe(
            Module::HostWasmtime,
            Operation::HostWasmtimeBasicHttpTransportPost,
        );
        let result = self
            .transport
            .post(request)
            .map_err(|TransportUnavailable| ());
        if result.is_ok() {
            span.event(SignalEvent::TransformationCompleted);
        } else {
            span.event(SignalEvent::InputRejected);
            span.fail(ErrorCategory::Platform);
        }
        result
    }
}

impl<T: BasicHttpTransport> guest_telemetry::Host for BasicHttpStoreData<T> {
    fn observe(
        &mut self,
        operation: guest_telemetry::Operation,
        outcome: guest_telemetry::Outcome,
        measurement: Option<guest_telemetry::Measurement>,
        context: Option<basichttp_types::TraceContext>,
    ) {
        if let Some(measurement) = measurement {
            let guest_telemetry::MeasurementKind::BodyBytes = measurement.kind;
            self.common
                .providers
                .record_component_measurement(ComponentMeasurement::InputBytes, measurement.value);
        }
        record_signal(
            &mut self.common,
            operation,
            Some(outcome_name(outcome)),
            None,
            context.as_ref(),
        );
    }

    fn event(
        &mut self,
        operation: guest_telemetry::Operation,
        event: guest_telemetry::SignalEvent,
        context: Option<basichttp_types::TraceContext>,
    ) {
        record_signal(
            &mut self.common,
            operation,
            None,
            Some(event_name(event)),
            context.as_ref(),
        );
    }
}

fn record_signal(
    common: &mut CommonStoreData,
    operation: guest_telemetry::Operation,
    outcome: Option<&'static str>,
    event: Option<&'static str>,
    context: Option<&basichttp_types::TraceContext>,
) {
    let parent = context.map(|context| TraceContext {
        traceparent: Some(context.traceparent.clone()),
        tracestate: context.tracestate.clone(),
    });
    if let Err(error) = record_scxml_observation(
        common,
        operation_name(operation),
        outcome,
        event,
        parent.as_ref(),
    ) {
        common.latch_telemetry_failure(error);
    }
}

const fn operation_name(operation: guest_telemetry::Operation) -> &'static str {
    match operation {
        guest_telemetry::Operation::TypeUri => "type-uri",
        guest_telemetry::Operation::Send => "send",
        guest_telemetry::Operation::Receive => "receive",
        guest_telemetry::Operation::Respond => "respond",
    }
}

const fn outcome_name(outcome: guest_telemetry::Outcome) -> &'static str {
    match outcome {
        guest_telemetry::Outcome::Ok => "ok",
        guest_telemetry::Outcome::Rejected => "rejected",
        guest_telemetry::Outcome::Failed => "failed",
    }
}

const fn event_name(event: guest_telemetry::SignalEvent) -> &'static str {
    match event {
        guest_telemetry::SignalEvent::TypeUriReturned => "type-uri-returned",
        guest_telemetry::SignalEvent::SendAccepted => "send-accepted",
        guest_telemetry::SignalEvent::SendRejected => "send-rejected",
        guest_telemetry::SignalEvent::MessageAccepted => "message-accepted",
        guest_telemetry::SignalEvent::MessageRejected => "message-rejected",
        guest_telemetry::SignalEvent::ResponseProduced => "response-produced",
    }
}
