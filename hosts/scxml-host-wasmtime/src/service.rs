//! Invoked services (SCXML 6.4) under the session host.
//!
//! An `<invoke>` whose type is neither absent nor the SCXML type names an
//! invoked service: an external service run by the
//! `stateforward:scxml/invoked-service` component the host binds to that
//! exact type. This module is the host side and stays a thin linker:
//!
//! * **Binding** — [`ServiceComponents`] binds exact type URIs to
//!   authenticated service components (release artifacts of kind
//!   [`ArtifactKind::ServiceExample`], or caller components authenticated by
//!   SHA-256 through [`crate::authenticate_service_component`]), each with
//!   its [`ServicePackage`], and plans each component's own `wasi:*`
//!   imports against the [`crate::WasiOptions`] the run grants it
//!   (`crate::wasi`) when it is bound. An `<invoke>` of a type no service
//!   binds, literal or given by `typeexpr`, fails when it runs
//!   (`error.execution`, SCXML 6.4: invocation is a runtime action).
//! * **Invocation** — every invocation gets its own instance in its own
//!   store, driven by the WASI 0.3 / Component Model async event loop
//!   (`Store::run_concurrent`) on a thread of its own, and its
//!   `invoked-service-api.invoke` runs as a subtask
//!   (`call_concurrent`). The events the invoking session sends to
//!   `#_<invokeid>` (and autoforwards) feed the `events` stream; the events
//!   the service returns on its stream reach the invoking session stamped
//!   with its invoke id; its future resolving `ok` becomes
//!   `done.invoke.<id>`, and resolving `err`, a trap (fuel exhaustion
//!   included), or an invalid returned event becomes the invocation's
//!   `communication-error` message (`error.communication.<id>`). A rejected
//!   `invoke` (it returned an error, trapped, or could not be instantiated)
//!   fails the `<invoke>` instead (`error.execution`).
//! * **Determinism** — the session hands each input to the invocation (its
//!   start, an event, its due timers, an HTTP response) with the session's
//!   logical time, and waits until the invocation's event loop has nothing
//!   left to run, so what the service returns is applied at the same point
//!   of every run. Its `wasi:clocks/monotonic-clock` is the session's
//!   logical clock (marks in nanoseconds since the session opened, 1 ms
//!   resolution): a `wait-until` or `wait-for` falls due in order with every
//!   delayed send of the run.
//! * **Cancel** — SCXML 6.4: when the invoking state exits first, the host
//!   cancels the `invoke` subtask and drops the invocation's store (no
//!   `done.invoke` follows), and the connection of every request still in
//!   flight is closed.
//! * **Outbound HTTP** — `outbound-http.fetch` suspends only the calling
//!   task. The host admits the request in the invocation (the service
//!   package's declared origins, or an origin a reference's value supplies
//!   entirely) and hands it to the session: a batch run performs it at
//!   once through the embedder's transport ([`ServiceComponents::with_outbound_http`]);
//!   a live session performs it on a transport thread and delivers the
//!   response later as a recorded external input (`LiveSession::deliver_http_responses`),
//!   which a replay delivers from its record instead.
//! * **Bounds** — the invocation runs at most the configured fuel
//!   ([`ServiceComponents::with_call_fuel`]) between two inputs, and returns
//!   at most 1024 events of at most 4 MiB in all per input (amplification
//!   guards, every build): past either the invocation fails.
//! * **Values and references** — the embedder holds the values of the
//!   variables a run grants ([`ServiceComponents::with_values`]). A service
//!   never receives one: for each `env` attribute of its `<invoke>`
//!   (`invoke-request.attributes`) that names a slot its package declares,
//!   the host fills `invoke-request.references` with an opaque reference
//!   when it holds a value for the variable granted to the service's type
//!   URI; the references live for the whole invocation, only inside its own
//!   admitted `outbound-http` requests.
//!
//! Telemetry: one boundary span per start, event, due timers, cancel, and
//! `outbound-http` request (`scxml.host-wasmtime.service-*`) with closed
//! labels only; type URIs, invoke ids, events, payloads, and URLs never
//! become attributes. Guest telemetry imports are recorded under the
//! `invoked-service` module label.

use std::collections::{BTreeMap, VecDeque};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::task::{Context as TaskContext, Poll, Wake, Waker};

use opentelemetry::Context;
use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent};
use wasmtime::StoreContextMut;
use wasmtime::component::{
    Accessor, Destination, FutureConsumer, HasData, HasSelf, Linker, Source, StreamConsumer,
    StreamProducer, StreamReader, StreamResult, VecBuffer,
};

use crate::bindings::stateforward::scxml::telemetry;
use crate::bindings::stateforward::scxml::types::{
    ComponentError, DataValue as WitDataValue, ErrorCode, Event as WitEvent,
    EventType as WitEventType, ExtensionAttribute, InvokeRequest, SlotReference,
    TraceContext as WitTraceContext,
};
use crate::component::{GuestSignal, component_error, record_guest_signal};
use crate::outbound_http::ExtensionHttp;
use crate::session::{native_event, native_value};
use crate::{
    ArtifactError, ArtifactKind, AuthenticatedComponent, ExtensionHttpFailure,
    ExtensionHttpRequest, HostError, InvalidLimit, LifecycleLease, TelemetryProviders,
    WasmtimeHostConfig,
};

pub(crate) mod service_bindings {
    wasmtime::component::bindgen!({
        path: "../../crates/scxml-wit/wit/scxml",
        world: "invoked-service",
        with: {
            "stateforward:scxml/types": crate::bindings::stateforward::scxml::types,
            "stateforward:scxml/telemetry": crate::bindings::stateforward::scxml::telemetry,
            "wasi:clocks": wasmtime_wasi::p3::bindings::clocks,
        },
        require_store_data_send: true,
    });
}

pub use service_bindings::stateforward::scxml::outbound_http;
use wasmtime_wasi::p3::bindings::clocks::{monotonic_clock, types as clock_types};

/// Maximum service type URIs one session host binds.
const MAX_BOUND_SERVICES: usize = 64;
/// Maximum events one invocation may return per input. An amplification
/// guard, so it applies on every build: lifting a guest buffer into host
/// memory is not fuel-metered.
const MAX_RETURNED_EVENTS_PER_INPUT: usize = 1_024;
/// Maximum cumulative bytes of the events one invocation may return per
/// input (as [`scxml_types::Event::validate`] counts them).
const MAX_RETURNED_BYTES_PER_INPUT: usize = 4 * 1_024 * 1_024;
/// Maximum requests of one invocation in flight at once; one more is
/// answered `capacity-exhausted`. An amplification guard (every build).
const MAX_REQUESTS_IN_FLIGHT: usize = 16;
/// Nanoseconds per logical millisecond (the logical clock's resolution).
const NANOS_PER_MS: u64 = 1_000_000;

/// The declarative package of one bound service (its
/// `stateforward.scxml.service-package/v1` manifest, already verified by the
/// embedder): the host binds by `service_uri` and grants `http_origins` and
/// `env_slots`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServicePackage {
    /// Exact service type URI; the binding key.
    pub service_uri: String,
    /// Package version.
    pub package_version: String,
    /// Service component artifact identifier.
    pub service_artifact: String,
    /// WIT world the component implements.
    pub wit_world: String,
    /// WIT version.
    pub wit_version: String,
    /// The origins or URL prefixes the component's requests may reach
    /// (canonicalized like an [`crate::HttpAllowlist`]); binding the package
    /// consents to them.
    pub http_origins: Vec<String>,
    /// The `env` slots an `<invoke>` of the service may bind; each bound
    /// slot's reference is valid for the whole invocation.
    pub env_slots: Vec<String>,
}

/// One bound service: its package, its component, and the component's
/// top-level imports.
#[derive(Clone, Debug)]
struct ServiceBinding {
    package: ServicePackage,
    component: AuthenticatedComponent,
    imports: Vec<String>,
}

/// Invoked-service components one session host links, keyed by exact type
/// URI.
#[derive(Clone, Debug)]
pub struct ServiceComponents {
    bindings: Vec<ServiceBinding>,
    http: Option<Arc<dyn ExtensionHttp>>,
    values: crate::ExtensionEnvironment,
    wasi: BTreeMap<String, crate::WasiOptions>,
    call_fuel: u64,
}

impl Default for ServiceComponents {
    fn default() -> Self {
        Self::new()
    }
}

/// What [`ServiceComponents`] hands the session host.
pub(crate) struct ServiceParts {
    pub(crate) bindings: Vec<(
        ServicePackage,
        AuthenticatedComponent,
        crate::wasi::WasiPlan,
    )>,
    pub(crate) http: Option<Arc<dyn ExtensionHttp>>,
    pub(crate) values: crate::ExtensionEnvironment,
    pub(crate) call_fuel: u64,
}

impl ServiceComponents {
    /// No bindings, no outbound HTTP, no values, no WASI resource, and
    /// [`crate::DEFAULT_EXTENSION_CALL_FUEL`] between two inputs.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bindings: Vec::new(),
            http: None,
            values: crate::ExtensionEnvironment::none(),
            wasi: BTreeMap::new(),
            call_fuel: crate::DEFAULT_EXTENSION_CALL_FUEL,
        }
    }

    /// The WASI runtime resources the run grants the service bound to
    /// exactly `uri` (wasmtime CLI's `--dir` and `-S` options), replacing an
    /// earlier grant to it. Every resource a bound component's imports need
    /// must be granted, and every grant must name a bound service and a
    /// resource its component imports; otherwise
    /// [`crate::SessionHost::with_service_components`] fails closed.
    #[must_use]
    pub fn with_wasi(mut self, uri: &str, options: crate::WasiOptions) -> Self {
        self.wasi.insert(uri.to_owned(), options);
        self
    }

    /// The values services' `env` slots may reference (the run's granted
    /// variables). Services receive references only, never a value.
    #[must_use]
    pub fn with_values(mut self, values: crate::ExtensionEnvironment) -> Self {
        self.values = values;
        self
    }

    /// The transport admitted `outbound-http` requests go through. Without
    /// one (the default) every request is refused with `unavailable`; with
    /// one, the host admits only the origins each service's package
    /// declares, or the origin a reference's value supplies entirely. A batch
    /// run performs each request at once; a live session performs it on a
    /// transport thread of its own and delivers the response as an external
    /// input. Cancelling the invocation closes the connection of a request
    /// still in flight when the transport is an
    /// [`crate::ExtensionHttpClient`]; another transport's own timeouts
    /// bound it.
    #[must_use]
    pub fn with_outbound_http(mut self, transport: Arc<dyn ExtensionHttp>) -> Self {
        self.http = Some(transport);
        self
    }

    /// Binds `component` to `package.service_uri`, compared byte for byte
    /// with each `<invoke type>`.
    ///
    /// # Errors
    ///
    /// [`HostError::Artifact`]: a component that is neither
    /// [`ArtifactKind::ServiceExample`] nor [`ArtifactKind::ServiceComponent`]
    /// ([`ArtifactError::ArtifactKindMismatch`]); a URI that is no service
    /// type URI ([`scxml_types::is_service_type_uri`]), bound twice, or past
    /// 64 bindings; or invalid declared origins or slots
    /// ([`ArtifactError::NamespaceBinding`]); and [`HostError::Component`]
    /// for bytes whose imports cannot be read.
    pub fn bind(
        mut self,
        package: ServicePackage,
        component: &AuthenticatedComponent,
    ) -> Result<Self, HostError> {
        if !matches!(
            component.kind(),
            ArtifactKind::ServiceExample | ArtifactKind::ServiceComponent
        ) {
            return Err(ArtifactError::ArtifactKindMismatch.into());
        }
        if !scxml_types::is_service_type_uri(&package.service_uri)
            || self.bindings.len() >= MAX_BOUND_SERVICES
            || self
                .bindings
                .iter()
                .any(|bound| bound.package.service_uri == package.service_uri)
            || package.http_origins.len() > crate::MAX_DECLARED_ITEMS
            || crate::HttpAllowlist::new(&package.http_origins).is_err()
            || package.env_slots.len() > crate::MAX_DECLARED_ITEMS
            || !package
                .env_slots
                .iter()
                .all(|slot| crate::outbound_http::is_env_slot(slot))
        {
            return Err(ArtifactError::NamespaceBinding.into());
        }
        let imports = crate::wasi::component_imports(component.bytes())?;
        self.bindings.push(ServiceBinding {
            package,
            component: component.clone(),
            imports,
        });
        Ok(self)
    }

    /// Sets the instruction fuel an invocation may run between two inputs.
    ///
    /// # Errors
    ///
    /// [`HostError::InvalidLimit`] for zero fuel.
    pub fn with_call_fuel(mut self, fuel: u64) -> Result<Self, HostError> {
        if fuel == 0 {
            return Err(HostError::InvalidLimit(InvalidLimit::Fuel));
        }
        self.call_fuel = fuel;
        Ok(self)
    }

    /// Whether no service is bound.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// The bindings, each with its WASI plan.
    ///
    /// # Errors
    ///
    /// [`ArtifactError::WasiGrantUndeclared`] for a grant to a URI no
    /// binding names, and every [`crate::wasi::WasiPlan::new`] failure.
    pub(crate) fn into_parts(self) -> Result<ServiceParts, HostError> {
        if self.wasi.keys().any(|uri| {
            !self
                .bindings
                .iter()
                .any(|binding| binding.package.service_uri == *uri)
        }) {
            return Err(ArtifactError::WasiGrantUndeclared.into());
        }
        let none = crate::WasiOptions::default();
        let bindings = self
            .bindings
            .into_iter()
            .map(|binding| {
                let options = self.wasi.get(&binding.package.service_uri).unwrap_or(&none);
                let plan = crate::wasi::WasiPlan::new(&binding.imports, options)?;
                Ok((binding.package, binding.component, plan))
            })
            .collect::<Result<Vec<_>, HostError>>()?;
        Ok(ServiceParts {
            bindings,
            http: self.http,
            values: self.values,
            call_fuel: self.call_fuel,
        })
    }
}

/// The linker of one bound service's stores: exactly the `invoked-service`
/// world's imports and the WASI interfaces `wasi` plans.
pub(crate) fn service_linker(
    engine: &wasmtime::Engine,
    wasi: &crate::wasi::WasiPlan,
) -> Result<Linker<ServiceStoreData>, HostError> {
    let mut linker = Linker::<ServiceStoreData>::new(engine);
    crate::bindings::stateforward::scxml::types::add_to_linker::<_, HasSelf<ServiceStoreData>>(
        &mut linker,
        |state| state,
    )?;
    telemetry::add_to_linker::<_, HasSelf<ServiceStoreData>>(&mut linker, |state| state)?;
    outbound_http::add_to_linker::<_, HasSelf<ServiceStoreData>>(&mut linker, |state| state)?;
    clock_types::add_to_linker::<_, LogicalClock>(&mut linker, |state| state)?;
    monotonic_clock::add_to_linker::<_, LogicalClock>(&mut linker, |state| state)?;
    wasi.add_to_linker(&mut linker)?;
    Ok(linker)
}

/// One bound service's component, pre-instantiated against its linker.
pub(crate) type ServicePre = service_bindings::InvokedServicePre<ServiceStoreData>;

/// Pre-instantiates one bound service's component against its linker.
pub(crate) fn service_pre(
    linker: &Linker<ServiceStoreData>,
    component: &wasmtime::component::Component,
) -> Result<ServicePre, HostError> {
    let pre = linker.instantiate_pre(component)?;
    Ok(service_bindings::InvokedServicePre::new(pre)?)
}

/// The run-wide facts one invocation is started with.
pub(crate) struct ServiceRuntime<'a> {
    pub(crate) pre: &'a service_bindings::InvokedServicePre<ServiceStoreData>,
    pub(crate) config: &'a WasmtimeHostConfig,
    pub(crate) providers: &'a TelemetryProviders,
    pub(crate) package: &'a ServicePackage,
    /// The package's planned WASI interfaces.
    pub(crate) wasi: &'a crate::wasi::WasiPlan,
    /// Whether the run offers outbound HTTP to services.
    pub(crate) http_offered: bool,
    /// The values `env` slots may reference.
    pub(crate) values: &'a crate::ExtensionEnvironment,
    pub(crate) call_fuel: u64,
}

// ---------------------------------------------------------------------------
// The invocation's shared state: what the session's inputs feed, and what the
// invocation produces for the session.
// ---------------------------------------------------------------------------

/// One timer the service waits on (`wait-until` or `wait-for`).
struct Timer {
    /// The logical ms it falls due at.
    deadline: u64,
    fired: bool,
    waker: Option<Waker>,
}

/// How a service asked for one request: `fetch` (the whole response) or
/// `send` (its status, then its body streamed).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RequestKind {
    /// `outbound-http.fetch`.
    Fetch,
    /// `outbound-http.send`.
    Send,
}

/// One request the service waits on (`fetch` or `send`).
#[derive(Default)]
struct Pending {
    /// A `fetch`'s response.
    outcome: Option<Result<outbound_http::HttpResponse, ComponentError>>,
    /// Whether a `send`'s status arrived (or its request failed first).
    status_seen: bool,
    /// A `send`'s status, until the call takes it.
    status: Option<Result<u16, ComponentError>>,
    /// A `send`'s body bytes received and not yet read.
    body: Vec<u8>,
    /// A `send`'s body bytes received in all.
    received: usize,
    /// A `send`'s end, once it arrived (or the body passed its bound).
    end: Option<Result<(), ComponentError>>,
    /// The service dropped the body stream: nothing more is kept for it.
    body_closed: bool,
    /// The service took the end, or dropped the future.
    end_closed: bool,
    waker: Option<Waker>,
    body_waker: Option<Waker>,
    end_waker: Option<Waker>,
}

impl Pending {
    /// Whether the transport still answers this request.
    const fn in_flight(&self) -> bool {
        self.outcome.is_none() && self.end.is_none()
    }

    /// Whether nothing of this request is left for the service.
    const fn settled(&self) -> bool {
        self.end.is_some() && self.body_closed && self.end_closed
    }

    /// Takes one part of a `send`'s response, within its bounds: a status
    /// outside 100..=599, a part out of order, or a transport failure ends
    /// the response with `unavailable`, and a body past
    /// [`crate::MAX_EXTENSION_HTTP_RESPONSE_BODY_BYTES`] in all with
    /// `payload-too-large`; nothing after the end is taken.
    fn take_part(&mut self, part: Result<crate::ExtensionHttpPart, ExtensionHttpFailure>) {
        if self.end.is_some() {
            return;
        }
        let failure = match part {
            Ok(crate::ExtensionHttpPart::Status(status)) if !self.status_seen => {
                self.status_seen = true;
                match crate::outbound_http::settle_status(status) {
                    Ok(status) => {
                        self.status = Some(Ok(status));
                        None
                    }
                    Err((error, _)) => {
                        self.status = Some(Err(error.clone()));
                        Some(error)
                    }
                }
            }
            Ok(crate::ExtensionHttpPart::Body(bytes)) if self.status_seen => {
                self.received = self.received.saturating_add(bytes.len());
                if self.received > crate::MAX_EXTENSION_HTTP_RESPONSE_BODY_BYTES {
                    self.body = Vec::new();
                    Some(
                        crate::outbound_http::failure_error(ExtensionHttpFailure::ResponseTooLarge)
                            .0,
                    )
                } else {
                    if !self.body_closed {
                        self.body.extend_from_slice(&bytes);
                    }
                    None
                }
            }
            Ok(crate::ExtensionHttpPart::End) if self.status_seen => {
                self.end = Some(Ok(()));
                None
            }
            Ok(_) => Some(crate::outbound_http::failure_error(ExtensionHttpFailure::Transport).0),
            Err(failure) => Some(crate::outbound_http::failure_error(failure).0),
        };
        if let Some(error) = failure {
            if !self.status_seen {
                self.status_seen = true;
                self.status = Some(Err(error.clone()));
            }
            self.end = Some(Err(error));
        }
        Io::wake(&mut self.waker);
        Io::wake(&mut self.body_waker);
        Io::wake(&mut self.end_waker);
    }
}

/// The state shared by one invocation's host functions, its streams, and
/// its thread.
#[derive(Default)]
struct Io {
    /// The session's logical time, in ms since the session opened.
    now_ms: u64,
    /// Events the invoking session sent, not yet read.
    events: VecDeque<WitEvent>,
    events_waker: Option<Waker>,
    /// Events the service returned since the last report, validated and
    /// stamped.
    returned: Vec<WitEvent>,
    returned_bytes: usize,
    /// A returned event broke its bounds: the invocation fails.
    invalid: bool,
    /// The future's value, once resolved.
    done: Option<Result<Option<WitDataValue>, ComponentError>>,
    /// Whether `invoke` returned `ok`.
    started: bool,
    timers: BTreeMap<u64, Timer>,
    next_timer: u64,
    /// Requests admitted since the last report.
    requests: Vec<(u64, ExtensionHttpRequest, RequestKind)>,
    pending: BTreeMap<u64, Pending>,
    next_request: u64,
    /// The pump refuels the store at its next poll.
    refuel: bool,
    pump_waker: Option<Waker>,
    /// The invoke type, stamped as `origin-type`.
    type_uri: String,
    /// The carrier stamped on returned events that carry none.
    carrier: Option<WitTraceContext>,
}

impl Io {
    fn wake(waker: &mut Option<Waker>) {
        if let Some(waker) = waker.take() {
            waker.wake();
        }
    }

    /// Validates, stamps, and keeps one returned event.
    fn accept(&mut self, event: WitEvent) {
        let event = WitEvent {
            name: event.name,
            event_type: WitEventType::External,
            send_id: event.send_id,
            origin: event.origin,
            origin_type: Some(self.type_uri.clone()),
            // The invoking session stamps the invoke id on delivery.
            invoke_id: None,
            data: event.data,
            raw: event.raw,
            trace_context: event.trace_context.or_else(|| self.carrier.clone()),
        };
        let Ok(native) = native_event(event.clone()) else {
            self.invalid = true;
            return;
        };
        let total = self.returned_bytes.saturating_add(event_bytes(&native));
        if self.returned.len() >= MAX_RETURNED_EVENTS_PER_INPUT
            || total > MAX_RETURNED_BYTES_PER_INPUT
        {
            self.invalid = true;
            return;
        }
        self.returned_bytes = total;
        self.returned.push(event);
    }

    /// The earliest deadline of a timer not yet fired.
    fn next_deadline(&self) -> Option<u64> {
        self.timers
            .values()
            .filter(|timer| !timer.fired)
            .map(|timer| timer.deadline)
            .min()
    }
}

type SharedIo = Arc<Mutex<Io>>;

fn lock(io: &SharedIo) -> std::sync::MutexGuard<'_, Io> {
    io.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The bytes one validated event counts toward
/// [`MAX_RETURNED_BYTES_PER_INPUT`]: the same fields
/// [`scxml_types::Event::validate`] bounds (fixed-width data as its width).
fn event_bytes(event: &scxml_types::Event) -> usize {
    use scxml_types::DataValue;
    let data = match &event.data {
        DataValue::Absent | DataValue::Undefined | DataValue::Null => 0,
        DataValue::Bool(_) => 1,
        DataValue::Number(_) => 8,
        DataValue::String(value)
        | DataValue::JsonArray(value)
        | DataValue::JsonObject(value)
        | DataValue::Xml(value) => value.len(),
    };
    let trace = event.trace_context.as_ref().map_or(0, |context| {
        context
            .traceparent
            .len()
            .saturating_add(context.tracestate.as_ref().map_or(0, String::len))
            .saturating_add(context.baggage.as_ref().map_or(0, String::len))
    });
    [
        event.name.as_str().len(),
        event.send_id.as_ref().map_or(0, |id| id.as_str().len()),
        event.invoke_id.as_ref().map_or(0, |id| id.as_str().len()),
        event.origin.as_ref().map_or(0, String::len),
        event.origin_type.as_ref().map_or(0, String::len),
        data,
        event.raw.as_ref().map_or(0, String::len),
        trace,
    ]
    .into_iter()
    .fold(0, usize::saturating_add)
}

/// The `events` stream the service reads: what the invoking session sent.
struct Inbox(SharedIo);

impl<D> StreamProducer<D> for Inbox {
    type Item = WitEvent;
    type Buffer = VecBuffer<WitEvent>;

    fn poll_produce<'a>(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        _store: StoreContextMut<'a, D>,
        mut destination: Destination<'a, WitEvent, VecBuffer<WitEvent>>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let mut io = lock(&self.0);
        if !io.events.is_empty() {
            let events: Vec<WitEvent> = io.events.drain(..).collect();
            destination.set_buffer(events.into());
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        if finish {
            return Poll::Ready(Ok(StreamResult::Cancelled));
        }
        io.events_waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

/// The stream of the events the service returns.
struct Outbox(SharedIo);

impl<D> StreamConsumer<D> for Outbox {
    type Item = WitEvent;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut TaskContext<'_>,
        mut store: StoreContextMut<D>,
        mut source: Source<'_, WitEvent>,
        _finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let remaining = source.remaining(&mut store);
        let mut events: Vec<WitEvent> =
            Vec::with_capacity(remaining.clamp(1, MAX_RETURNED_EVENTS_PER_INPUT));
        source.read(&mut store, &mut events)?;
        let mut io = lock(&self.0);
        for event in events {
            io.accept(event);
        }
        if io.invalid {
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

/// The future that resolves when the service is done.
struct Completion(SharedIo);

impl<D> FutureConsumer<D> for Completion {
    type Item = Result<Option<WitDataValue>, ComponentError>;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut TaskContext<'_>,
        mut store: StoreContextMut<D>,
        mut source: Source<'_, Self::Item>,
        _finish: bool,
    ) -> Poll<wasmtime::Result<()>> {
        let mut value: Option<Self::Item> = None;
        source.read(&mut store, &mut value)?;
        let mut io = lock(&self.0);
        if let Some(value) = value {
            let value = match value {
                Ok(Some(data)) if native_value(data.clone()).is_err() => Err(component_error(
                    ErrorCode::InvalidInput,
                    "the service's done data is outside the event data bounds",
                )),
                value => value,
            };
            io.done = Some(value);
        }
        Io::wake(&mut io.pump_waker);
        Poll::Ready(Ok(()))
    }
}

// ---------------------------------------------------------------------------
// Store data and host functions.
// ---------------------------------------------------------------------------

/// Store data of one invocation.
pub(crate) struct ServiceStoreData {
    limits: crate::memory_growth::MeteredLimits,
    providers: TelemetryProviders,
    execution_context: Context,
    guest_spans: crate::guest_spans::GuestSpanTable,
    io: SharedIo,
    allow: crate::HttpAllowlist,
    /// This invocation's references and the values they stand for.
    references: Vec<(u32, String)>,
    /// Whether the run offers outbound HTTP to services.
    http_offered: bool,
    /// The invocation's WASI context (reachable only through the interfaces
    /// its plan linked).
    wasi: crate::wasi::WasiState,
    _store_lease: LifecycleLease,
}

impl wasmtime_wasi::WasiView for ServiceStoreData {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        self.wasi.view()
    }
}

/// The `wasi:clocks/monotonic-clock@0.3.0` the service world imports, on
/// the session's logical clock.
struct LogicalClock;

impl HasData for LogicalClock {
    type Data<'a> = &'a mut ServiceStoreData;
}

impl clock_types::Host for ServiceStoreData {}

impl monotonic_clock::Host for ServiceStoreData {
    fn now(&mut self) -> wasmtime::Result<monotonic_clock::Mark> {
        Ok(lock(&self.io).now_ms.saturating_mul(NANOS_PER_MS))
    }

    fn get_resolution(&mut self) -> wasmtime::Result<clock_types::Duration> {
        Ok(NANOS_PER_MS)
    }
}

/// Waits until the logical clock reaches `deadline` ms: at once when it
/// already has, else when the session's clock reaches it.
async fn logical_wait(io: SharedIo, deadline: u64) {
    let id = {
        let mut state = lock(&io);
        if deadline <= state.now_ms {
            return;
        }
        let id = state.next_timer;
        state.next_timer = id.saturating_add(1);
        state.timers.insert(
            id,
            Timer {
                deadline,
                fired: false,
                waker: None,
            },
        );
        id
    };
    std::future::poll_fn(|cx| {
        let mut state = lock(&io);
        match state.timers.get_mut(&id) {
            Some(timer) if timer.fired => {
                state.timers.remove(&id);
                Poll::Ready(())
            }
            Some(timer) => {
                timer.waker = Some(cx.waker().clone());
                Poll::Pending
            }
            None => Poll::Ready(()),
        }
    })
    .await;
}

impl<U> monotonic_clock::HostWithStore<U> for LogicalClock {
    async fn wait_until(
        accessor: &Accessor<U, Self>,
        when: monotonic_clock::Mark,
    ) -> wasmtime::Result<()> {
        let io = accessor.with(|mut access| Arc::clone(&access.get().io));
        logical_wait(io, when.div_ceil(NANOS_PER_MS)).await;
        Ok(())
    }

    async fn wait_for(
        accessor: &Accessor<U, Self>,
        how_long: clock_types::Duration,
    ) -> wasmtime::Result<()> {
        let io = accessor.with(|mut access| Arc::clone(&access.get().io));
        let deadline = {
            let now = lock(&io).now_ms.saturating_mul(NANOS_PER_MS);
            now.saturating_add(how_long).div_ceil(NANOS_PER_MS)
        };
        logical_wait(io, deadline).await;
        Ok(())
    }
}

impl crate::bindings::stateforward::scxml::types::Host for ServiceStoreData {}

impl outbound_http::Host for ServiceStoreData {}

/// Admits one request of the invocation `accessor` runs as `kind`, under
/// a boundary span of `operation`: its number and the invocation's shared
/// state, or the `outbound-http` error that refuses it.
fn admit<U>(
    accessor: &Accessor<U, HasSelf<ServiceStoreData>>,
    request: outbound_http::HttpRequest,
    kind: RequestKind,
    operation: Operation,
) -> Result<(u64, SharedIo), ComponentError> {
    // The span covers admission; the transport's own span covers the
    // request (`service-http-request`), and the response arrives as an
    // input of the invocation.
    let span = scxml_telemetry::observe(Module::HostWasmtime, operation);
    let admitted = accessor.with(|mut access| {
        let data = access.get();
        crate::outbound_http::admit_request(
            request,
            &data.references,
            &data.allow,
            data.http_offered,
            |_| {},
        )
        .map(|admitted| (admitted, Arc::clone(&data.io)))
    });
    let admitted = match admitted {
        Ok((admitted, io)) => {
            let mut state = lock(&io);
            if state
                .pending
                .values()
                .filter(|pending| pending.in_flight())
                .count()
                >= MAX_REQUESTS_IN_FLIGHT
            {
                Err((
                    component_error(
                        ErrorCode::CapacityExhausted,
                        "the invocation has as many requests in flight as it may",
                    ),
                    ErrorCategory::Budget,
                ))
            } else {
                let id = state.next_request;
                state.next_request = id.saturating_add(1);
                state.requests.push((id, admitted, kind));
                state.pending.insert(id, Pending::default());
                drop(state);
                Ok((id, io))
            }
        }
        Err(rejected) => Err(rejected),
    };
    match admitted {
        Ok(admitted) => {
            span.event(SignalEvent::TransformationCompleted);
            Ok(admitted)
        }
        Err((error, category)) => {
            span.event(SignalEvent::InputRejected);
            span.fail(category);
            Err(error)
        }
    }
}

impl<U> outbound_http::HostWithStore<U> for HasSelf<ServiceStoreData> {
    async fn fetch(
        accessor: &Accessor<U, Self>,
        request: outbound_http::HttpRequest,
    ) -> Result<outbound_http::HttpResponse, ComponentError> {
        let (id, io) = admit(
            accessor,
            request,
            RequestKind::Fetch,
            Operation::HostWasmtimeServiceHttpFetch,
        )?;
        std::future::poll_fn(|cx| {
            let mut state = lock(&io);
            let Some(pending) = state.pending.get_mut(&id) else {
                return Poll::Ready(Err(component_error(
                    ErrorCode::Unavailable,
                    "the HTTP transport failed",
                )));
            };
            if let Some(outcome) = pending.outcome.take() {
                state.pending.remove(&id);
                Poll::Ready(outcome)
            } else {
                pending.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await
    }

    async fn send(
        accessor: &Accessor<U, Self>,
        request: outbound_http::HttpRequest,
    ) -> Result<
        (
            u16,
            StreamReader<u8>,
            wasmtime::component::FutureReader<Result<(), ComponentError>>,
        ),
        ComponentError,
    > {
        let (id, io) = admit(
            accessor,
            request,
            RequestKind::Send,
            Operation::HostWasmtimeServiceHttpSend,
        )?;
        let status = std::future::poll_fn(|cx| {
            let mut state = lock(&io);
            let Some(pending) = state.pending.get_mut(&id) else {
                return Poll::Ready(Err(component_error(
                    ErrorCode::Unavailable,
                    "the HTTP transport failed",
                )));
            };
            match pending.status.take() {
                Some(Ok(status)) => Poll::Ready(Ok(status)),
                Some(Err(error)) => {
                    state.pending.remove(&id);
                    Poll::Ready(Err(error))
                }
                None => {
                    pending.waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        })
        .await?;
        let opened = accessor.with(|mut access| -> wasmtime::Result<_> {
            let body = StreamReader::new(&mut access, Body::new(&io, id))?;
            let end = wasmtime::component::FutureReader::new(&mut access, BodyEnd::new(&io, id))?;
            Ok((body, end))
        });
        opened.map(|(body, end)| (status, body, end)).map_err(|_| {
            lock(&io).pending.remove(&id);
            component_error(ErrorCode::Platform, "the response body could not be opened")
        })
    }
}

/// The body stream of one `send`: the bytes its invocation received, in
/// order, until the end.
struct Body {
    io: SharedIo,
    id: u64,
}

impl Body {
    fn new(io: &SharedIo, id: u64) -> Self {
        Self {
            io: Arc::clone(io),
            id,
        }
    }
}

impl Drop for Body {
    /// A body the service dropped keeps nothing more.
    fn drop(&mut self) {
        let mut state = lock(&self.io);
        if let Some(pending) = state.pending.get_mut(&self.id) {
            pending.body_closed = true;
            pending.body = Vec::new();
            if pending.settled() {
                state.pending.remove(&self.id);
            }
        }
    }
}

impl<D> StreamProducer<D> for Body {
    type Item = u8;
    type Buffer = VecBuffer<u8>;

    fn poll_produce<'a>(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        _store: StoreContextMut<'a, D>,
        mut destination: Destination<'a, u8, VecBuffer<u8>>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let mut state = lock(&self.io);
        let Some(pending) = state.pending.get_mut(&self.id) else {
            return Poll::Ready(Ok(StreamResult::Dropped));
        };
        if !pending.body.is_empty() {
            destination.set_buffer(std::mem::take(&mut pending.body).into());
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        if pending.end.is_some() {
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        if finish {
            return Poll::Ready(Ok(StreamResult::Cancelled));
        }
        pending.body_waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

/// The future of one `send`'s end: `ok` after the body's last byte, or the
/// failure that ended it.
struct BodyEnd {
    io: SharedIo,
    id: u64,
}

impl BodyEnd {
    fn new(io: &SharedIo, id: u64) -> Self {
        Self {
            io: Arc::clone(io),
            id,
        }
    }
}

impl Drop for BodyEnd {
    fn drop(&mut self) {
        let mut state = lock(&self.io);
        if let Some(pending) = state.pending.get_mut(&self.id) {
            pending.end_closed = true;
            if pending.settled() {
                state.pending.remove(&self.id);
            }
        }
    }
}

impl<D> wasmtime::component::FutureProducer<D> for BodyEnd {
    type Item = Result<(), ComponentError>;

    fn poll_produce(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        _store: StoreContextMut<D>,
        finish: bool,
    ) -> Poll<wasmtime::Result<Option<Self::Item>>> {
        let mut state = lock(&self.io);
        let Some(pending) = state.pending.get_mut(&self.id) else {
            return Poll::Ready(Ok(Some(Err(component_error(
                ErrorCode::Unavailable,
                "the HTTP transport failed",
            )))));
        };
        if let Some(end) = pending.end.clone() {
            return Poll::Ready(Ok(Some(end)));
        }
        if finish {
            return Poll::Ready(Ok(None));
        }
        pending.end_waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl telemetry::Host for ServiceStoreData {
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
            "invoked-service",
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
            "invoked-service",
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
            "invoked-service",
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

// ---------------------------------------------------------------------------
// The invocation's thread.
// ---------------------------------------------------------------------------

/// One input the session hands an invocation.
enum Input {
    /// An event sent to `#_<invokeid>` or autoforwarded.
    Event(Box<WitEvent>),
    /// The logical clock moved; every timer due by then fires.
    Timers,
    /// What the transport answered the request `id`: a `fetch`'s response
    /// (or failure), or one part of a `send`'s.
    Answer(u64, crate::ExtensionHttpAnswer),
    /// The request `id` fails at once (no transport).
    Refused(u64, ComponentError),
}

/// One input with the session's logical time it is applied at.
struct Delivery {
    now_ms: u64,
    input: Input,
}

/// What the invocation reports after the start and after each input, once
/// its event loop has nothing left to run (or it ended).
pub(crate) struct Report {
    /// The events it returned, validated and stamped, in order.
    pub(crate) events: Vec<WitEvent>,
    /// The requests it made, admitted, in order.
    pub(crate) requests: Vec<(u64, ExtensionHttpRequest, RequestKind)>,
    /// Its earliest timer, in logical ms.
    pub(crate) next_deadline: Option<u64>,
    /// How it stands.
    pub(crate) state: ReportState,
}

/// How an invocation stands after an input.
#[derive(Debug)]
pub(crate) enum ReportState {
    /// It runs on, waiting for the session.
    Running,
    /// Its future resolved `ok` with this data (`done.invoke`).
    Done(Option<WitDataValue>),
    /// Its future resolved `err`, it trapped, or it returned an invalid
    /// event, after it started (`communication-error`).
    Failed,
    /// Its `invoke` returned an error, trapped, or could not be
    /// instantiated before returning (`error.execution`).
    Rejected,
}

/// A waker that unparks the invocation's thread and notes that it fired.
struct ThreadWaker {
    thread: std::thread::Thread,
    woken: AtomicBool,
}

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.woken.store(true, Ordering::SeqCst);
        self.thread.unpark();
    }
}

/// Polls `future` on this thread until it is ready, parking while it is
/// pending: the executor of calls that complete within the call, such as an
/// extension component's `execute`.
pub(crate) fn block_on<F: std::future::Future>(future: F) -> F::Output {
    let waker = Arc::new(ThreadWaker {
        thread: std::thread::current(),
        woken: AtomicBool::new(false),
    });
    let task_waker = Waker::from(Arc::clone(&waker));
    let mut cx = TaskContext::from_waker(&task_waker);
    let mut future = std::pin::pin!(future);
    loop {
        waker.woken.store(false, Ordering::SeqCst);
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
        if !waker.woken.load(Ordering::SeqCst) {
            std::thread::park();
        }
    }
}

/// Everything the invocation's thread owns.
struct Invocation {
    store: wasmtime::Store<ServiceStoreData>,
    pre: service_bindings::InvokedServicePre<ServiceStoreData>,
    request: InvokeRequest,
    io: SharedIo,
    call_fuel: u64,
}

/// The body of one invocation's thread: instantiate, call `invoke`, then
/// apply each delivery and report once the event loop has nothing left to
/// run, until the service is done or the session cancels it (drops its
/// sender).
#[allow(
    clippy::too_many_lines,
    reason = "one sequence: instantiate, invoke, pump, then the delivery loop"
)]
fn run_invocation(
    invocation: Invocation,
    deliveries: &mpsc::Receiver<Delivery>,
    reports: &mpsc::Sender<Report>,
) {
    let Invocation {
        mut store,
        pre,
        request,
        io,
        call_fuel,
    } = invocation;
    let rejected = |reports: &mpsc::Sender<Report>| {
        let _ = reports.send(Report {
            events: Vec::new(),
            requests: Vec::new(),
            next_deadline: None,
            state: ReportState::Rejected,
        });
    };
    if store.set_fuel(call_fuel).is_err() {
        rejected(reports);
        return;
    }
    let Ok(guest) = block_on(pre.instantiate_async(&mut store)) else {
        rejected(reports);
        return;
    };
    let pump_io = Arc::clone(&io);
    let future = store.run_concurrent(async move |accessor| -> wasmtime::Result<bool> {
        let events = accessor
            .with(|mut access| StreamReader::new(&mut access, Inbox(Arc::clone(&pump_io))))?;
        let invoked = guest
            .stateforward_scxml_invoked_service_api()
            .call_invoke(accessor, request, events)
            .await?;
        let Ok((returned, completion)) = invoked else {
            return Ok(false);
        };
        accessor.with(|mut access| -> wasmtime::Result<()> {
            returned.pipe(&mut access, Outbox(Arc::clone(&pump_io)))?;
            completion.pipe(&mut access, Completion(Arc::clone(&pump_io)))
        })?;
        lock(&pump_io).started = true;
        // Pump: refuel at every input, until the future resolves or a
        // returned event breaks its bounds.
        loop {
            let refuel = std::future::poll_fn(|cx| {
                let mut state = lock(&pump_io);
                if state.done.is_some() || state.invalid {
                    return Poll::Ready(false);
                }
                if std::mem::take(&mut state.refuel) {
                    return Poll::Ready(true);
                }
                state.pump_waker = Some(cx.waker().clone());
                Poll::Pending
            })
            .await;
            if !refuel {
                return Ok(true);
            }
            accessor.with(|mut access| {
                use wasmtime::AsContextMut as _;
                access.data_mut().limits.restart();
                access.as_context_mut().set_fuel(call_fuel)
            })?;
        }
    });
    let waker = Arc::new(ThreadWaker {
        thread: std::thread::current(),
        woken: AtomicBool::new(false),
    });
    let task_waker = Waker::from(Arc::clone(&waker));
    let mut cx = TaskContext::from_waker(&task_waker);
    let mut future = std::pin::pin!(future);
    loop {
        waker.woken.store(false, Ordering::SeqCst);
        let polled = future.as_mut().poll(&mut cx);
        if polled.is_pending() && waker.woken.load(Ordering::SeqCst) {
            // Woken while it ran: more is ready to run.
            continue;
        }
        let mut state = lock(&io);
        let events = std::mem::take(&mut state.returned);
        state.returned_bytes = 0;
        let requests = std::mem::take(&mut state.requests);
        let next_deadline = state.next_deadline();
        let report_state = match polled {
            Poll::Ready(Ok(Ok(true))) => match (state.invalid, state.done.take()) {
                (false, Some(Ok(data))) => ReportState::Done(data),
                _ => ReportState::Failed,
            },
            Poll::Ready(Ok(Err(_)) | Err(_)) if state.started => ReportState::Failed,
            Poll::Ready(Ok(Ok(false) | Err(_)) | Err(_)) => ReportState::Rejected,
            Poll::Pending if state.invalid => ReportState::Failed,
            Poll::Pending => ReportState::Running,
        };
        drop(state);
        let finished = !matches!(report_state, ReportState::Running);
        if reports
            .send(Report {
                events,
                requests,
                next_deadline,
                state: report_state,
            })
            .is_err()
            || finished
        {
            // The session is gone or the invocation ended: drop the
            // future (cancelling every subtask) and the store.
            return;
        }
        // Idle: wait for the next input. A closed channel is the session's
        // cancel (SCXML 6.4).
        let Ok(delivery) = deliveries.recv() else {
            return;
        };
        let mut state = lock(&io);
        state.now_ms = delivery.now_ms;
        state.refuel = true;
        match delivery.input {
            Input::Event(event) => {
                state.events.push_back(*event);
                Io::wake(&mut state.events_waker);
            }
            Input::Timers => {
                let now = state.now_ms;
                for timer in state.timers.values_mut() {
                    if !timer.fired && timer.deadline <= now {
                        timer.fired = true;
                        Io::wake(&mut timer.waker);
                    }
                }
            }
            Input::Answer(id, answer) => {
                if let Some(pending) = state.pending.get_mut(&id) {
                    match answer {
                        crate::ExtensionHttpAnswer::Response(outcome) => {
                            pending.outcome = Some(
                                crate::outbound_http::settle_response(outcome)
                                    .map_err(|(error, _)| error),
                            );
                            Io::wake(&mut pending.waker);
                        }
                        crate::ExtensionHttpAnswer::Part(part) => {
                            pending.take_part(part);
                            if pending.settled() {
                                state.pending.remove(&id);
                            }
                        }
                    }
                }
            }
            Input::Refused(id, error) => {
                if let Some(pending) = state.pending.get_mut(&id) {
                    pending.outcome = Some(Err(error.clone()));
                    if !pending.status_seen {
                        pending.status_seen = true;
                        pending.status = Some(Err(error.clone()));
                    }
                    pending.end = Some(Err(error));
                    Io::wake(&mut pending.waker);
                    Io::wake(&mut pending.body_waker);
                    Io::wake(&mut pending.end_waker);
                }
            }
        }
        Io::wake(&mut state.pump_waker);
    }
}

// ---------------------------------------------------------------------------
// The session's side of one invocation.
// ---------------------------------------------------------------------------

/// The cancellation of one request in flight: the host cancels it with its
/// invocation (SCXML 6.4); the transport registers how to stop the request
/// (close its connection) as soon as it holds one. Every registered action
/// runs once, and one registered after the cancel runs at once.
#[derive(Default)]
pub(crate) struct CancelSignal {
    state: Mutex<CancelState>,
}

#[derive(Default)]
struct CancelState {
    cancelled: bool,
    actions: Vec<Box<dyn FnOnce() + Send>>,
}

impl CancelSignal {
    /// Cancels the request: runs every registered action once.
    pub(crate) fn cancel(&self) {
        let actions = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.cancelled = true;
            std::mem::take(&mut state.actions)
        };
        for action in actions {
            action();
        }
    }

    /// Whether the request was cancelled.
    #[cfg(any(feature = "http", test))]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .cancelled
    }

    /// Registers `action` to stop the request; it runs at once when the
    /// request was already cancelled.
    #[cfg(any(feature = "http", test))]
    pub(crate) fn on_cancel(&self, action: impl FnOnce() + Send + 'static) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.cancelled {
            drop(state);
            action();
        } else {
            state.actions.push(Box::new(action));
        }
    }
}

std::thread_local! {
    /// The cancellation of the request the current transport thread
    /// performs, which [`crate::ExtensionHttpClient`] wires to its
    /// connection.
    static REQUEST_CANCEL: std::cell::RefCell<Option<Arc<CancelSignal>>> =
        const { std::cell::RefCell::new(None) };
}

/// The cancellation of the request this transport thread performs, if any.
#[cfg(feature = "http")]
pub(crate) fn request_cancel() -> Option<Arc<CancelSignal>> {
    REQUEST_CANCEL.with_borrow(Option::clone)
}

/// Makes `cancel` the cancellation of the request this thread performs.
#[cfg(all(feature = "http", test))]
pub(crate) fn set_request_cancel(cancel: Option<Arc<CancelSignal>>) {
    REQUEST_CANCEL.with_borrow_mut(|slot| *slot = cancel);
}

/// One settled request of a live run, waiting for delivery on the session
/// thread.
pub(crate) struct Settled {
    /// The run-unique id of the invocation that made it.
    pub(crate) invocation: u64,
    /// Its id within that invocation.
    pub(crate) request: u64,
    /// What the transport answered: a `fetch`'s response, or one part of
    /// a `send`'s (consecutive body parts waiting for delivery are joined).
    pub(crate) answer: crate::ExtensionHttpAnswer,
}

/// The settled requests of one live run, shared by the session thread and
/// every transport thread.
pub(crate) struct HttpResponses {
    settled: Mutex<VecDeque<Settled>>,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl HttpResponses {
    /// An empty queue that calls `notify` (from the settling thread) each
    /// time a request settles.
    pub(crate) fn new(notify: Option<Arc<dyn Fn() + Send + Sync>>) -> Arc<Self> {
        Arc::new(Self {
            settled: Mutex::new(VecDeque::new()),
            notify,
        })
    }

    /// Takes the earliest settled answer.
    pub(crate) fn take_first(&self) -> Option<Settled> {
        self.settled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
    }

    /// Whether a settled request waits for delivery.
    pub(crate) fn has_settled(&self) -> bool {
        !self
            .settled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }

    /// Queues one answer. Body bytes of a `send` join the request's last
    /// queued answer when that is body bytes too, so a response that
    /// trickles in reaches its invocation as one input per delivery, not
    /// one per transport read (an amplification guard).
    fn push(&self, settled: Settled) {
        let mut queue = self.settled.lock().unwrap_or_else(PoisonError::into_inner);
        let last = queue.iter_mut().rev().find(|queued| {
            queued.invocation == settled.invocation && queued.request == settled.request
        });
        if let (
            Some(Settled {
                answer: crate::ExtensionHttpAnswer::Part(Ok(crate::ExtensionHttpPart::Body(queued))),
                ..
            }),
            crate::ExtensionHttpAnswer::Part(Ok(crate::ExtensionHttpPart::Body(bytes))),
        ) = (last, &settled.answer)
        {
            queued.extend_from_slice(bytes);
        } else {
            queue.push_back(settled);
        }
        drop(queue);
        if let Some(notify) = &self.notify {
            notify();
        }
    }
}

/// How a run carries out the requests its invocations make.
#[derive(Clone)]
pub(crate) enum RequestMode {
    /// A batch run: each request is performed at once, on the session
    /// thread, and its response delivered before anything else runs.
    Immediate(Arc<dyn ExtensionHttp>),
    /// A live session: each request is performed on a transport thread of
    /// its own, and its response joins `responses` for delivery
    /// (`LiveSession::deliver_http_responses`).
    Threaded {
        transport: Arc<dyn ExtensionHttp>,
        responses: Arc<HttpResponses>,
    },
    /// A replaying live session: the responses come from the record.
    Recorded,
    /// No outbound HTTP: services' requests are refused before they leave
    /// the invocation.
    None,
}

/// What one input left of an invocation, for the run to apply in order: the
/// events it returned, then how it stands.
pub(crate) struct ServiceStep {
    /// Validated, stamped events for the invoking session, in order.
    pub(crate) events: Vec<WitEvent>,
    /// How the input left the invocation.
    pub(crate) state: ReportState,
}

/// One invocation, as the session holds it.
pub(crate) struct ServiceHandle {
    deliveries: Option<mpsc::Sender<Delivery>>,
    reports: mpsc::Receiver<Report>,
    /// The invocation's earliest timer, in logical ms.
    next_deadline: Option<u64>,
    /// Whether it ended (done, failed, or rejected).
    ended: bool,
    /// Its requests performed on transport threads, by id, with their
    /// cancellation.
    in_flight: BTreeMap<u64, Arc<CancelSignal>>,
    /// Its requests waiting for a recorded response (replay), by id.
    awaiting_record: std::collections::BTreeSet<u64>,
    /// Its run-unique id, the key its settled requests carry.
    invocation: u64,
}

impl Drop for ServiceHandle {
    /// Dropping the handle cancels the invocation (SCXML 6.4): its thread
    /// drops the `invoke` subtask and the store, and every request still in
    /// flight is stopped.
    fn drop(&mut self) {
        self.deliveries = None;
        for cancel in std::mem::take(&mut self.in_flight).into_values() {
            cancel.cancel();
        }
    }
}

impl ServiceHandle {
    /// Starts one invocation of the service `runtime` binds: its store, its
    /// `invoke-request` with the references its `env` attributes bind, and
    /// its thread, which instantiates the component and calls `invoke`.
    /// Returns the handle and the start's report.
    ///
    /// # Errors
    ///
    /// `invalid-input` for an `env` attribute naming a slot the package
    /// does not declare, a slot twice, or no variable; a host failure (a
    /// granted directory that cannot be opened, a thread that cannot start)
    /// as [`HostError`].
    pub(crate) fn start(
        runtime: &ServiceRuntime<'_>,
        mut request: InvokeRequest,
        now_ms: u64,
        invocation: u64,
        execution_context: &Context,
    ) -> Result<Result<(Self, Report), ComponentError>, HostError> {
        let (slots, references) = match bind_references(
            &request.attributes,
            &runtime.package.service_uri,
            &runtime.package.env_slots,
            runtime.values,
        ) {
            Ok(bound) => bound,
            Err(rejected) => return Ok(Err(rejected)),
        };
        request.references = slots;
        let wasi = runtime.wasi.context()?;
        let store_config = &runtime.config.store;
        let limits = wasmtime::StoreLimitsBuilder::new()
            .memory_size(store_config.memory_bytes)
            .table_elements(store_config.table_elements)
            .instances(store_config.instances)
            .tables(store_config.tables)
            .memories(store_config.memories)
            .trap_on_grow_failure(true)
            .build();
        let io: SharedIo = Arc::new(Mutex::new(Io {
            now_ms,
            type_uri: request.type_uri.clone(),
            carrier: request.trace_context.clone(),
            ..Io::default()
        }));
        let mut store = wasmtime::Store::new(
            runtime.pre.engine(),
            ServiceStoreData {
                limits: crate::memory_growth::MeteredLimits::metered(
                    limits,
                    None,
                    runtime.call_fuel,
                ),
                providers: runtime.providers.clone(),
                execution_context: execution_context.clone(),
                guest_spans: crate::guest_spans::GuestSpanTable::new(),
                io: Arc::clone(&io),
                // Validated when the package was bound.
                allow: crate::HttpAllowlist::new(&runtime.package.http_origins)
                    .map_err(|_| HostError::from(ArtifactError::NamespaceBinding))?,
                references,
                http_offered: runtime.http_offered,
                wasi,
                _store_lease: runtime.providers.acquire_store()?,
            },
        );
        store.limiter(|state| &mut state.limits);
        let invocation_state = Invocation {
            store,
            pre: runtime.pre.clone(),
            request,
            io,
            call_fuel: runtime.call_fuel,
        };
        let (deliveries, delivered) = mpsc::channel();
        let (reported, reports) = mpsc::channel();
        let parent = Context::current();
        std::thread::Builder::new()
            .name("scxml-invoked-service".into())
            .spawn(move || {
                let _parent = parent.attach();
                run_invocation(invocation_state, &delivered, &reported);
            })
            .map_err(|_| HostError::Component("invoked-service-thread-unavailable".into()))?;
        let mut handle = Self {
            deliveries: Some(deliveries),
            reports,
            next_deadline: None,
            ended: false,
            in_flight: BTreeMap::new(),
            awaiting_record: std::collections::BTreeSet::new(),
            invocation,
        };
        let report = handle.receive();
        Ok(Ok((handle, report)))
    }

    /// The invocation's next report; a thread that ended without one failed.
    fn receive(&mut self) -> Report {
        let report = self.reports.recv().unwrap_or(Report {
            events: Vec::new(),
            requests: Vec::new(),
            next_deadline: None,
            state: ReportState::Failed,
        });
        self.next_deadline = report.next_deadline;
        if !matches!(report.state, ReportState::Running) {
            self.ended = true;
            self.next_deadline = None;
            self.deliveries = None;
        }
        report
    }

    /// Hands the invocation one input at `now_ms` and waits for its report;
    /// an ended invocation takes nothing.
    fn deliver(&mut self, now_ms: u64, input: Input) -> Option<Report> {
        if self.ended {
            return None;
        }
        let sender = self.deliveries.as_ref()?;
        if sender.send(Delivery { now_ms, input }).is_err() {
            self.ended = true;
            return Some(Report {
                events: Vec::new(),
                requests: Vec::new(),
                next_deadline: None,
                state: ReportState::Failed,
            });
        }
        Some(self.receive())
    }

    /// An event the invoking session sent to the service.
    pub(crate) fn send(&mut self, event: WitEvent, now_ms: u64, mode: &RequestMode) -> ServiceStep {
        let report = self.deliver(now_ms, Input::Event(Box::new(event)));
        self.settle(report, now_ms, mode)
    }

    /// The service's due timers at `now_ms`.
    pub(crate) fn timers(&mut self, now_ms: u64, mode: &RequestMode) -> ServiceStep {
        let report = self.deliver(now_ms, Input::Timers);
        self.settle(report, now_ms, mode)
    }

    /// What the transport answered the request `request` (a `fetch`'s
    /// response, or one part of a `send`'s), delivered at `now_ms`: from a
    /// live run's transport or a replay's record. `None` when the
    /// invocation ended or holds no such request (the answer is dropped).
    pub(crate) fn respond(
        &mut self,
        request: u64,
        answer: crate::ExtensionHttpAnswer,
        now_ms: u64,
        mode: &RequestMode,
    ) -> Option<ServiceStep> {
        let known =
            self.in_flight.contains_key(&request) || self.awaiting_record.contains(&request);
        if !known || self.ended {
            return None;
        }
        if answer.is_final() {
            self.in_flight.remove(&request);
            self.awaiting_record.remove(&request);
        }
        let report = self.deliver(now_ms, Input::Answer(request, answer));
        Some(self.settle(report, now_ms, mode))
    }

    /// Settles one report: the start's, or an input's. Its requests are
    /// carried out as `mode` says (a batch run performs them now and
    /// delivers each answer before anything else runs).
    pub(crate) fn settle(
        &mut self,
        report: Option<Report>,
        now_ms: u64,
        mode: &RequestMode,
    ) -> ServiceStep {
        let mut events = Vec::new();
        let mut report = report;
        loop {
            let Some(current) = report.take() else {
                return ServiceStep {
                    events,
                    state: ReportState::Running,
                };
            };
            events.extend(current.events);
            if !matches!(current.state, ReportState::Running) {
                return ServiceStep {
                    events,
                    state: current.state,
                };
            }
            if current.requests.is_empty() {
                return ServiceStep {
                    events,
                    state: ReportState::Running,
                };
            }
            match mode {
                RequestMode::Immediate(transport) => {
                    report = self.perform_first(
                        transport.as_ref(),
                        current.requests,
                        now_ms,
                        &mut events,
                    );
                }
                RequestMode::Threaded {
                    transport,
                    responses,
                } => {
                    for (id, request, kind) in current.requests {
                        let cancel = Arc::new(CancelSignal::default());
                        self.in_flight.insert(id, Arc::clone(&cancel));
                        spawn_request(
                            Arc::clone(transport),
                            responses,
                            self.invocation,
                            (id, request, kind),
                            cancel,
                        );
                    }
                    return ServiceStep {
                        events,
                        state: ReportState::Running,
                    };
                }
                RequestMode::Recorded => {
                    self.awaiting_record
                        .extend(current.requests.into_iter().map(|(id, _, _)| id));
                    return ServiceStep {
                        events,
                        state: ReportState::Running,
                    };
                }
                RequestMode::None => {
                    // Unreachable by construction (the invocation refuses
                    // every request without a transport); fail closed.
                    let mut failed = None;
                    for (id, _, _) in current.requests {
                        failed = self.deliver(
                            now_ms,
                            Input::Refused(
                                id,
                                component_error(
                                    ErrorCode::Unavailable,
                                    "the host offers this component no outbound HTTP",
                                ),
                            ),
                        );
                    }
                    report = failed;
                }
            }
        }
    }

    /// A batch run's step: performs the first of `requests` at once and
    /// delivers each of its answers in order, keeping the events the
    /// deliveries before the last returned in `events`; returns the last
    /// delivery's report, whose requests are the ones still waiting, then
    /// the ones the deliveries made.
    fn perform_first(
        &mut self,
        transport: &dyn ExtensionHttp,
        requests: Vec<(u64, ExtensionHttpRequest, RequestKind)>,
        now_ms: u64,
        events: &mut Vec<WitEvent>,
    ) -> Option<Report> {
        let mut waiting: VecDeque<_> = requests.into();
        let (id, request, kind) = waiting.pop_front()?;
        let span = scxml_telemetry::observe(
            Module::HostWasmtime,
            Operation::HostWasmtimeServiceHttpRequest,
        );
        let answers = perform(transport, &request, kind);
        record_request(&span, answers.last());
        let mut answers = answers.into_iter();
        let mut next = answers
            .next()
            .and_then(|answer| self.deliver(now_ms, Input::Answer(id, answer)));
        for answer in answers {
            let Some(delivered) = next.as_mut() else {
                break;
            };
            if !matches!(delivered.state, ReportState::Running) {
                break;
            }
            events.append(&mut delivered.events);
            waiting.extend(std::mem::take(&mut delivered.requests));
            next = self.deliver(now_ms, Input::Answer(id, answer));
        }
        if let Some(delivered) = next.as_mut() {
            waiting.extend(std::mem::take(&mut delivered.requests));
            delivered.requests = waiting.into();
        }
        next
    }

    /// The invocation's earliest timer, in logical ms.
    pub(crate) const fn next_deadline(&self) -> Option<u64> {
        if self.ended { None } else { self.next_deadline }
    }

    /// Takes the timer deadline when it is due at `now`.
    pub(crate) fn take_due(&mut self, now: u64) -> bool {
        match self.next_deadline() {
            Some(deadline) if deadline <= now => {
                self.next_deadline = None;
                true
            }
            _ => false,
        }
    }

    /// Whether the invocation waits for a response of a request in flight
    /// or a recorded one.
    pub(crate) fn awaiting(&self) -> bool {
        !self.ended && (!self.in_flight.is_empty() || !self.awaiting_record.is_empty())
    }

    /// The run-unique id its settled requests carry.
    pub(crate) const fn invocation(&self) -> u64 {
        self.invocation
    }
}

/// Ends one transport request's span with its last answer: completed for
/// a response or a body that ended, rejected for a failure.
fn record_request(span: &scxml_telemetry::FunctionSpan, last: Option<&crate::ExtensionHttpAnswer>) {
    let completed = matches!(
        last,
        Some(
            crate::ExtensionHttpAnswer::Response(Ok(_))
                | crate::ExtensionHttpAnswer::Part(Ok(crate::ExtensionHttpPart::End))
        )
    );
    if completed {
        span.event(SignalEvent::TransformationCompleted);
    } else {
        span.event(SignalEvent::InputRejected);
        span.fail(ErrorCategory::Platform);
    }
}

/// Performs one admitted request through `transport` at once: a `fetch`'s
/// one answer, or every part of a `send`'s, consecutive body parts joined.
fn perform(
    transport: &dyn ExtensionHttp,
    request: &ExtensionHttpRequest,
    kind: RequestKind,
) -> Vec<crate::ExtensionHttpAnswer> {
    match kind {
        RequestKind::Fetch => vec![crate::ExtensionHttpAnswer::Response(
            transport.fetch(request),
        )],
        RequestKind::Send => {
            let mut answers = Vec::new();
            transport.send(request, &mut |part| {
                push_part(&mut answers, part);
            });
            answers
        }
    }
}

/// Appends one part of a `send`'s response to `answers`, joining it to the
/// last one when both are body bytes.
fn push_part(
    answers: &mut Vec<crate::ExtensionHttpAnswer>,
    part: Result<crate::ExtensionHttpPart, ExtensionHttpFailure>,
) {
    if let (
        Some(crate::ExtensionHttpAnswer::Part(Ok(crate::ExtensionHttpPart::Body(last)))),
        Ok(crate::ExtensionHttpPart::Body(bytes)),
    ) = (answers.last_mut(), &part)
    {
        last.extend_from_slice(bytes);
        return;
    }
    answers.push(crate::ExtensionHttpAnswer::Part(part));
}

/// Performs one admitted request on a transport thread of its own and
/// queues each of its answers for delivery as it arrives; `cancel` closes
/// its connection.
fn spawn_request(
    transport: Arc<dyn ExtensionHttp>,
    responses: &Arc<HttpResponses>,
    invocation: u64,
    (request_id, request, kind): (u64, ExtensionHttpRequest, RequestKind),
    cancel: Arc<CancelSignal>,
) {
    let parent = Context::current();
    let queue = Arc::clone(responses);
    let spawned = std::thread::Builder::new()
        .name("scxml-invoked-service-http".into())
        .spawn(move || {
            let _parent = parent.attach();
            let span = scxml_telemetry::observe(
                Module::HostWasmtime,
                Operation::HostWasmtimeServiceHttpRequest,
            );
            REQUEST_CANCEL.with_borrow_mut(|slot| *slot = Some(cancel));
            let last = match kind {
                RequestKind::Fetch => {
                    let answer = crate::ExtensionHttpAnswer::Response(transport.fetch(&request));
                    queue.push(Settled {
                        invocation,
                        request: request_id,
                        answer: answer.clone(),
                    });
                    Some(answer)
                }
                RequestKind::Send => {
                    let mut last = None;
                    transport.send(&request, &mut |part| {
                        let answer = crate::ExtensionHttpAnswer::Part(part);
                        last = Some(answer.clone());
                        queue.push(Settled {
                            invocation,
                            request: request_id,
                            answer,
                        });
                    });
                    last
                }
            };
            REQUEST_CANCEL.with_borrow_mut(|slot| *slot = None);
            record_request(&span, last.as_ref());
        });
    if spawned.is_err() {
        // A thread that could not start sent nothing: the request fails.
        responses.push(Settled {
            invocation,
            request: request_id,
            answer: match kind {
                RequestKind::Fetch => {
                    crate::ExtensionHttpAnswer::Response(Err(ExtensionHttpFailure::Transport))
                }
                RequestKind::Send => {
                    crate::ExtensionHttpAnswer::Part(Err(ExtensionHttpFailure::Transport))
                }
            },
        });
    }
}

/// One invocation's slot references and the values they stand for.
type BoundReferences = (Vec<SlotReference>, Vec<(u32, String)>);

/// The slot references of one invocation: for each `env` attribute (sorted
/// by slot) whose slot `env_slots` declares, a fresh reference when
/// `values` holds the variable for `type_uri` (numbered from 1 per
/// invocation, so none crosses invocations); a variable with no value, or
/// one not granted to `type_uri`, leaves its slot absent.
///
/// # Errors
///
/// `invalid-input` for more than [`crate::MAX_DECLARED_ITEMS`] attributes,
/// one outside the `env` namespace, a slot the package does not declare or
/// bound twice, or a name that is not a variable name.
fn bind_references(
    attributes: &[ExtensionAttribute],
    type_uri: &str,
    env_slots: &[String],
    values: &crate::ExtensionEnvironment,
) -> Result<BoundReferences, ComponentError> {
    let invalid = |detail| Err(component_error(ErrorCode::InvalidInput, detail));
    if attributes.len() > crate::MAX_DECLARED_ITEMS {
        return invalid("the invocation binds more env slots than a package declares");
    }
    let mut slots = Vec::new();
    let mut references = Vec::new();
    for (index, attribute) in attributes.iter().enumerate() {
        if attribute.name.namespace_uri.as_deref() != Some(scxml_types::ENV_NAMESPACE_URI) {
            return invalid("an invoke attribute is not in the env namespace");
        }
        let slot = &attribute.name.local_name;
        if !env_slots.contains(slot) {
            return invalid("an env slot is not declared by the service's package");
        }
        if attributes
            .get(..index)
            .is_some_and(|before| before.iter().any(|seen| seen.name.local_name == *slot))
        {
            return invalid("the invocation binds one env slot twice");
        }
        if !crate::outbound_http::is_env_name(&attribute.value) {
            return invalid("an env slot names no valid variable");
        }
        let Some(value) = values.held(type_uri, &attribute.value) else {
            continue;
        };
        let reference = u32::try_from(references.len().saturating_add(1))
            .map_err(|_| component_error(ErrorCode::InvalidInput, "too many env slots"))?;
        references.push((reference, value.to_owned()));
        slots.push(SlotReference {
            slot: slot.clone(),
            reference,
        });
    }
    Ok((slots, references))
}

#[cfg(test)]
mod tests {
    use super::{CancelSignal, HttpResponses, Pending, Settled, event_bytes};
    use crate::bindings::stateforward::scxml::types::ErrorCode;
    use crate::{
        ExtensionHttpAnswer, ExtensionHttpFailure, ExtensionHttpPart, ExtensionHttpResponse,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn a_returned_event_counts_its_retained_bytes() {
        let mut event = scxml_types::Event::external(
            scxml_types::EventName::new("echo.reply".to_owned()).expect("a valid event name"),
        );
        assert_eq!(event_bytes(&event), "echo.reply".len());
        event.data = scxml_types::DataValue::String("x".repeat(100));
        event.origin_type = Some("urn:echo".to_owned());
        event.raw = Some("raw".to_owned());
        assert_eq!(
            event_bytes(&event),
            "echo.reply".len() + 100 + "urn:echo".len() + "raw".len()
        );
    }

    #[test]
    fn settled_requests_queue_in_order_and_notify() {
        let notified = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&notified);
        let queue = HttpResponses::new(Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::AcqRel);
        })));
        queue.push(Settled {
            invocation: 1,
            request: 0,
            answer: ExtensionHttpAnswer::Response(Ok(ExtensionHttpResponse {
                status: 200,
                body: b"ok".to_vec(),
            })),
        });
        queue.push(Settled {
            invocation: 1,
            request: 1,
            answer: ExtensionHttpAnswer::Response(Err(ExtensionHttpFailure::Transport)),
        });
        assert!(queue.has_settled());
        let settled: Vec<_> = std::iter::from_fn(|| queue.take_first()).collect();
        assert_eq!(settled.len(), 2);
        assert_eq!((settled[0].invocation, settled[0].request), (1, 0));
        assert_eq!(
            settled[1].answer,
            ExtensionHttpAnswer::Response(Err(ExtensionHttpFailure::Transport))
        );
        assert!(!queue.has_settled());
        assert_eq!(notified.load(Ordering::Acquire), 2);
    }

    fn part(request: u64, part: ExtensionHttpPart) -> Settled {
        Settled {
            invocation: 1,
            request,
            answer: ExtensionHttpAnswer::Part(Ok(part)),
        }
    }

    #[test]
    fn queued_body_bytes_of_one_request_join_until_delivered() {
        let queue = HttpResponses::new(None);
        queue.push(part(0, ExtensionHttpPart::Status(200)));
        queue.push(part(0, ExtensionHttpPart::Body(b"ab".to_vec())));
        queue.push(part(1, ExtensionHttpPart::Status(204)));
        queue.push(part(0, ExtensionHttpPart::Body(b"cd".to_vec())));
        queue.push(part(1, ExtensionHttpPart::Body(b"x".to_vec())));
        let answers: Vec<_> = std::iter::from_fn(|| queue.take_first())
            .map(|settled| (settled.request, settled.answer))
            .collect();
        assert_eq!(
            answers,
            [
                (
                    0,
                    ExtensionHttpAnswer::Part(Ok(ExtensionHttpPart::Status(200)))
                ),
                (
                    0,
                    ExtensionHttpAnswer::Part(Ok(ExtensionHttpPart::Body(b"abcd".to_vec())))
                ),
                (
                    1,
                    ExtensionHttpAnswer::Part(Ok(ExtensionHttpPart::Status(204)))
                ),
                (
                    1,
                    ExtensionHttpAnswer::Part(Ok(ExtensionHttpPart::Body(b"x".to_vec())))
                ),
            ]
        );
        // Delivered bytes start a new answer.
        queue.push(part(0, ExtensionHttpPart::Body(b"e".to_vec())));
        queue.push(part(0, ExtensionHttpPart::End));
        queue.push(part(0, ExtensionHttpPart::Body(b"late".to_vec())));
        assert_eq!(
            std::iter::from_fn(|| queue.take_first()).count(),
            3,
            "nothing joins across the end"
        );
        assert!(ExtensionHttpAnswer::Part(Ok(ExtensionHttpPart::End)).is_final());
        assert!(!ExtensionHttpAnswer::Part(Ok(ExtensionHttpPart::Body(Vec::new()))).is_final());
    }

    #[test]
    fn a_streamed_response_keeps_its_order_and_bounds() {
        let mut pending = Pending::default();
        pending.take_part(Ok(ExtensionHttpPart::Body(b"early".to_vec())));
        assert_eq!(
            pending
                .status
                .as_ref()
                .map(|status| status.as_ref().map_err(|error| error.code)),
            Some(Err(ErrorCode::Unavailable)),
            "a body before the status fails the request"
        );
        assert!(!pending.in_flight());

        let mut pending = Pending::default();
        pending.take_part(Ok(ExtensionHttpPart::Status(700)));
        assert!(
            matches!(pending.status, Some(Err(_))),
            "a status outside 100..=599"
        );

        let mut pending = Pending::default();
        pending.take_part(Ok(ExtensionHttpPart::Status(200)));
        pending.take_part(Ok(ExtensionHttpPart::Body(b"ab".to_vec())));
        pending.take_part(Ok(ExtensionHttpPart::Body(b"cd".to_vec())));
        assert!(pending.in_flight());
        pending.take_part(Ok(ExtensionHttpPart::End));
        pending.take_part(Ok(ExtensionHttpPart::Body(b"late".to_vec())));
        assert_eq!(pending.body, b"abcd");
        assert!(matches!(pending.end, Some(Ok(()))));
        assert!(!pending.in_flight());

        let mut pending = Pending::default();
        pending.take_part(Ok(ExtensionHttpPart::Status(200)));
        pending.take_part(Ok(ExtensionHttpPart::Body(vec![
            0;
            crate::MAX_EXTENSION_HTTP_RESPONSE_BODY_BYTES
        ])));
        assert!(pending.end.is_none(), "the bound itself is within it");
        pending.take_part(Ok(ExtensionHttpPart::Body(vec![0])));
        assert_eq!(
            pending
                .end
                .as_ref()
                .map(|end| end.as_ref().map_err(|error| error.code)),
            Some(Err(ErrorCode::PayloadTooLarge))
        );
        assert!(pending.body.is_empty(), "nothing past the bound is kept");

        let mut pending = Pending::default();
        pending.take_part(Ok(ExtensionHttpPart::Status(200)));
        pending.take_part(Err(ExtensionHttpFailure::Transport));
        assert_eq!(
            pending
                .end
                .as_ref()
                .map(|end| end.as_ref().map_err(|error| error.code)),
            Some(Err(ErrorCode::Unavailable))
        );
    }

    #[test]
    fn a_cancel_runs_each_action_once_and_a_late_action_at_once() {
        let cancel = CancelSignal::default();
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&ran);
        cancel.on_cancel(move || {
            counter.fetch_add(1, Ordering::AcqRel);
        });
        assert!(!cancel.is_cancelled());
        assert_eq!(ran.load(Ordering::Acquire), 0);
        cancel.cancel();
        cancel.cancel();
        assert!(cancel.is_cancelled());
        assert_eq!(ran.load(Ordering::Acquire), 1, "an action runs once");
        let counter = Arc::clone(&ran);
        cancel.on_cancel(move || {
            counter.fetch_add(1, Ordering::AcqRel);
        });
        assert_eq!(ran.load(Ordering::Acquire), 2, "a late action runs at once");
    }
}
