//! Invoked services (SCXML 6.4) under the session host.
//!
//! An `<invoke>` whose type is neither absent nor the SCXML type names an
//! invoked service: a leaf invocation with no machine, run by the
//! `stateforward:scxml/invoked-service` component the host binds to that
//! exact type URI. This module is the host side and stays a thin linker:
//!
//! * **Binding** — [`ServiceComponents`] binds exact type URIs to
//!   authenticated service components (release artifacts of kind
//!   [`ArtifactKind::ServiceExample`], or caller components authenticated by
//!   SHA-256 through [`crate::authenticate_service_component`]), each with
//!   its [`ServicePackage`]. A session whose document names an unbound type
//!   literally fails its start closed (`machine-capabilities.load-services`);
//!   one given by `typeexpr` fails that `<invoke>` (`error.execution`).
//! * **Lifecycle** — every invocation gets its own instance in its own
//!   bounded store: the host checks `type-uri` against the binding, calls
//!   `start` once, then `send` for each event the invoking session sends to
//!   `#_<invokeid>` (or autoforwards) and `wake` when its wakeup falls due on
//!   the run's shared logical clock, and `cancel` when the invoking state
//!   exits first; then it drops the instance.
//! * **Host calls** — `invoked-service-host` works only inside one of the
//!   instance's export calls. `emit` stamps the event `external` with the
//!   service's type URI as `origin-type`, checks it against the event
//!   bounds, and the run pushes it onto the invoking session's queue
//!   (dropped once that session began cancelling the invocation);
//!   `complete` finishes the service (`done.invoke.<id>` with its data);
//!   `set-wakeup` asks for one `wake`.
//! * **Failure** — a trap (fuel exhaustion included), a failed export call
//!   after `start`, or an invalid `emit` retires the instance: the invoking
//!   session receives `failed` (`error.communication` with the invoke id)
//!   and no `done.invoke`. A failed `start` fails the `<invoke>` instead.
//! * **Bounds** — every export call runs under the per-call fuel cap and on
//!   the session host's per-call budget (`call_budget`), like extension
//!   components. `outbound-http` is deny by default and admits only the
//!   origins the service package declares, or the origin a reference's
//!   value supplies entirely.
//! * **Values and references** — the embedder holds the values of the
//!   variables a run grants ([`ServiceComponents::with_values`]). A service
//!   never receives one: for each `env` slot its `<invoke>` binds
//!   (`service-invocation.env`, `env:<slot>="NAME"` or `env:<slot>expr`)
//!   that its package declares, the host hands `start` an opaque reference
//!   when it holds a value for the variable (a name with no value leaves the
//!   slot absent). Unlike an extension element's per-call references, a
//!   service's references live for the whole invocation: every later export
//!   call of that instance may use them, and only inside its own admitted
//!   `outbound-http` requests; no other instance ever receives them. A slot
//!   its package does not declare, one slot twice, more than
//!   [`crate::MAX_DECLARED_ITEMS`] slots, or a name that is not a variable
//!   name rejects the start (the `<invoke>` raises `error.execution`).
//! * **Event contracts** — a package may declare the events its service
//!   emits and accepts, each with a resolved JSON Schema for the event's
//!   data ([`ServiceEvents`]); [`ServiceComponents::bind`] compiles them once
//!   (`scxml_event_schema::CompiledSchema`). With a contract, every `emit`
//!   must name a declared emitted event whose schema admits its data, or the
//!   emit is invalid and retires the instance (the invoking session receives
//!   `error.communication` with the invoke id); and every `<send>` the
//!   invoking session addresses to the service must name a declared accepted
//!   event whose schema admits its data, or the `<send>` raises
//!   `error.execution` (a delayed one is checked when it is sent, and again
//!   when it falls due, when a mismatch is `error.communication`).
//!   Autoforwarded events are delivered unchecked. A service without a
//!   contract is not checked. A document's own `event:schema` attributes
//!   never take part: they stay metadata only.
//! * **WASI** — nothing WASI is ambient: each service's linker holds exactly
//!   the WASI interfaces its package declares and the run grants
//!   ([`crate::WasiGrants`], `crate::wasi`), and a component importing any
//!   other WASI interface is refused at session start (`load-services`) and
//!   before every instantiation.
//!
//! Telemetry: one boundary span per load, start, send, wake, cancel, and
//! `outbound-http` request (`scxml.host-wasmtime.service-*`) with closed
//! labels only; type URIs, invoke ids, events, payloads, and URLs never
//! become attributes. Guest telemetry imports are recorded under the
//! `invoked-service` module label.

use std::sync::Arc;

use opentelemetry::Context;
use scxml_telemetry::{Module, Operation, SignalEvent};
use wasmtime::component::{HasSelf, Linker};

use crate::bindings::stateforward::scxml::telemetry;
use crate::bindings::stateforward::scxml::types::{
    ComponentError, DataValue as WitDataValue, EnvBinding, ErrorCode, Event as WitEvent,
    EventType as WitEventType, ServiceInvocation, SlotReference, TraceContext as WitTraceContext,
};
use crate::call_budget::CallBudget;
use crate::component::{GuestSignal, component_error, record_guest_signal};
use crate::extension::{ExtensionHttp, outbound_http};
use crate::memory_growth::MeteredLimits;
use crate::session::{native_event, native_value};
use crate::{
    ArtifactError, ArtifactKind, AuthenticatedComponent, HostError, InvalidLimit, LifecycleLease,
    TelemetryProviders, WasmtimeHostConfig,
};

mod service_bindings {
    wasmtime::component::bindgen!({
        path: "../../crates/scxml-wit/wit/scxml",
        world: "invoked-service",
        with: {
            "stateforward:scxml/types": crate::bindings::stateforward::scxml::types,
            "stateforward:scxml/telemetry": crate::bindings::stateforward::scxml::telemetry,
            "stateforward:scxml/outbound-http": crate::extension::outbound_http,
        },
    });
}

use service_bindings::stateforward::scxml::invoked_service_host;

/// Default instruction fuel cap of one service export call (`type-uri`,
/// `start`, `send`, `wake`, or `cancel`); every call also runs on the session
/// host's per-call budget.
pub const DEFAULT_SERVICE_CALL_FUEL: u64 = 100_000_000;
/// Maximum service type URIs one session host binds.
pub const MAX_SERVICE_COMPONENTS: usize = 64;
/// Maximum declared events in each list of a [`ServiceEvents`] (the
/// manifest bound).
pub const MAX_SERVICE_EVENTS: usize = 64;
/// Maximum bytes of one declared event name.
const MAX_SERVICE_EVENT_NAME_BYTES: usize = 256;

/// One declared event of a service's contract: its exact name and its data
/// schema as canonical resolved JSON (`scxml_event_schema::Resolved`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceEventSchema {
    /// Exact event name.
    pub name: String,
    /// The schema as canonical JSON.
    pub schema: String,
}

/// A service package's declared event contract: the events the service
/// emits and accepts. Empty lists declare no contract (nothing is
/// checked); any declared event makes the contract closed in that
/// direction and the other (an event not listed fails the check).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ServiceEvents {
    /// Events the service emits, strictly sorted by name.
    pub emits: Vec<ServiceEventSchema>,
    /// Events the service accepts, strictly sorted by name.
    pub accepts: Vec<ServiceEventSchema>,
}

/// One bound service's compiled event contract.
#[derive(Debug)]
pub(crate) struct ServiceContract {
    emits: std::collections::BTreeMap<String, scxml_event_schema::CompiledSchema>,
    accepts: std::collections::BTreeMap<String, scxml_event_schema::CompiledSchema>,
}

impl ServiceContract {
    /// The compiled contract of `events`, or `None` when it declares none.
    ///
    /// # Errors
    ///
    /// [`ArtifactError::EventContractInvalid`].
    fn compile(events: &ServiceEvents) -> Result<Option<Arc<Self>>, HostError> {
        if events.emits.is_empty() && events.accepts.is_empty() {
            return Ok(None);
        }
        let compile = |list: &[ServiceEventSchema]| {
            if list.len() > MAX_SERVICE_EVENTS
                || !list
                    .windows(2)
                    .all(|pair| matches!(pair, [first, second] if first.name < second.name))
            {
                return Err(HostError::from(ArtifactError::EventContractInvalid));
            }
            list.iter()
                .map(|event| {
                    if event.name.len() > MAX_SERVICE_EVENT_NAME_BYTES
                        || scxml_types::EventName::new(event.name.clone()).is_err()
                        || event.name.contains('*')
                        || event.name.split('.').any(str::is_empty)
                    {
                        return Err(HostError::from(ArtifactError::EventContractInvalid));
                    }
                    let schema = scxml_event_schema::CompiledSchema::new(&event.schema)
                        .map_err(|_| HostError::from(ArtifactError::EventContractInvalid))?;
                    Ok((event.name.clone(), schema))
                })
                .collect::<Result<std::collections::BTreeMap<_, _>, HostError>>()
        };
        Ok(Some(Arc::new(Self {
            emits: compile(&events.emits)?,
            accepts: compile(&events.accepts)?,
        })))
    }

    /// Whether the service may emit the event `name` carrying `data`.
    pub(crate) fn admits_emit(&self, name: &str, data: &scxml_types::DataValue) -> bool {
        self.emits
            .get(name)
            .is_some_and(|schema| schema.admits(data))
    }

    /// Whether the service accepts the event `name` carrying `data`.
    pub(crate) fn admits_accept(&self, name: &str, data: &scxml_types::DataValue) -> bool {
        self.accepts
            .get(name)
            .is_some_and(|schema| schema.admits(data))
    }
}

/// The declarative package of one bound service (its
/// `stateforward.scxml.service-package/v1` manifest, already verified by the
/// embedder): the host binds by `service_uri`, grants `http_origins`, and
/// links the `wasi_permissions` the run grants.
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
    /// The WASI interfaces its component may import (a canonical list,
    /// [`scxml_types::check_wasi_permissions`]); each must be granted
    /// ([`ServiceComponents::with_wasi_grants`]) or the binding fails.
    pub wasi_permissions: Vec<scxml_types::WasiPermission>,
    /// Its declared event contract, checked at its emit and send
    /// boundaries; empty declares none.
    pub events: ServiceEvents,
}

/// One bound service: its package and component.
#[derive(Clone, Debug)]
pub(crate) struct ServiceBinding {
    pub(crate) package: ServicePackage,
    pub(crate) component: AuthenticatedComponent,
    /// The compiled event contract; `None` when the package declares none.
    pub(crate) contract: Option<Arc<ServiceContract>>,
}

/// Invoked-service components one session host links, keyed by exact type
/// URI.
#[derive(Clone, Debug)]
pub struct ServiceComponents {
    bindings: Vec<ServiceBinding>,
    http: Option<Arc<dyn ExtensionHttp>>,
    values: crate::ExtensionEnvironment,
    grants: crate::WasiGrants,
    call_fuel: u64,
}

impl Default for ServiceComponents {
    fn default() -> Self {
        Self::new()
    }
}

/// What [`ServiceComponents`] hands the session host.
pub(crate) struct ServiceParts {
    pub(crate) bindings: Vec<(ServiceBinding, crate::wasi::WasiPlan)>,
    pub(crate) http: Option<Arc<dyn ExtensionHttp>>,
    pub(crate) values: crate::ExtensionEnvironment,
    pub(crate) call_fuel: u64,
}

impl ServiceComponents {
    /// No bindings, no outbound HTTP, no values, no WASI grant, and
    /// [`DEFAULT_SERVICE_CALL_FUEL`] per export call.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bindings: Vec::new(),
            http: None,
            values: crate::ExtensionEnvironment::none(),
            grants: crate::WasiGrants::new(),
            call_fuel: DEFAULT_SERVICE_CALL_FUEL,
        }
    }

    /// The run's WASI grants, by exact service type URI. Every permission a
    /// bound package declares must be granted, and every grant must name a
    /// bound package and a permission it declares; otherwise
    /// [`crate::SessionHost::with_service_components`] fails closed.
    #[must_use]
    pub fn with_wasi_grants(mut self, grants: crate::WasiGrants) -> Self {
        self.grants = grants;
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
    /// declares, or the origin a reference's value supplies entirely.
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
    /// [`MAX_SERVICE_COMPONENTS`] bindings; invalid declared origins or
    /// slots ([`ArtifactError::NamespaceBinding`]); a non-canonical WASI
    /// permission list ([`ArtifactError::WasiPermissionsInvalid`]); or a
    /// malformed event contract ([`ArtifactError::EventContractInvalid`]).
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
            || self.bindings.len() >= MAX_SERVICE_COMPONENTS
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
                .all(|slot| crate::extension::is_env_slot(slot))
        {
            return Err(ArtifactError::NamespaceBinding.into());
        }
        if scxml_types::check_wasi_permissions(&package.wasi_permissions).is_err() {
            return Err(ArtifactError::WasiPermissionsInvalid.into());
        }
        let contract = ServiceContract::compile(&package.events)?;
        self.bindings.push(ServiceBinding {
            package,
            component: component.clone(),
            contract,
        });
        Ok(self)
    }

    /// Sets the instruction fuel cap of one service export call.
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
        if self.grants.uris().any(|uri| {
            !self
                .bindings
                .iter()
                .any(|binding| binding.package.service_uri == uri)
        }) {
            return Err(ArtifactError::WasiGrantUndeclared.into());
        }
        let bindings = self
            .bindings
            .into_iter()
            .map(|binding| {
                let plan = crate::wasi::WasiPlan::new(
                    &binding.package.wasi_permissions,
                    self.grants.for_uri(&binding.package.service_uri),
                )?;
                Ok((binding, plan))
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
/// world's imports and the WASI interfaces `wasi` plans (none when the
/// package declares none).
pub(crate) fn service_linker(
    engine: &wasmtime::Engine,
    wasi: &crate::wasi::WasiPlan,
) -> Result<Linker<ServiceStoreData>, HostError> {
    let mut linker = Linker::<ServiceStoreData>::new(engine);
    service_bindings::InvokedService::add_to_linker::<_, HasSelf<ServiceStoreData>>(
        &mut linker,
        |state| state,
    )?;
    wasi.add_to_linker(&mut linker)?;
    Ok(linker)
}

/// Everything one invocation needs to instantiate its service.
pub(crate) struct ServiceRuntime<'a> {
    pub(crate) engine: &'a wasmtime::Engine,
    pub(crate) linker: &'a Linker<ServiceStoreData>,
    pub(crate) config: &'a WasmtimeHostConfig,
    pub(crate) providers: &'a TelemetryProviders,
    pub(crate) call_budget: &'a Arc<CallBudget>,
    pub(crate) package: &'a ServicePackage,
    /// The package's compiled event contract.
    pub(crate) contract: Option<&'a Arc<ServiceContract>>,
    /// The package's declared-and-granted WASI permissions.
    pub(crate) wasi: &'a crate::wasi::WasiPlan,
    pub(crate) http: Option<&'a Arc<dyn ExtensionHttp>>,
    /// The values `env` slots may reference.
    pub(crate) values: &'a crate::ExtensionEnvironment,
    pub(crate) call_fuel: u64,
}

/// The run's clock as one service call sees it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ServiceClock {
    /// The shared logical clock, in ms.
    pub(crate) now: u64,
    /// The Unix epoch ms of logical time 0.
    pub(crate) start_epoch_ms: u64,
}

/// One export call in progress inside a service store.
struct ServiceCall {
    emitted: Vec<WitEvent>,
    /// `complete`'s data, once called.
    completion: Option<Completion>,
    /// `set-wakeup`'s last request, once called.
    wakeup: Option<Wakeup>,
    /// An `emit` broke the event bounds.
    invalid_emit: bool,
    /// The carrier stamped on emitted events that carry none.
    carrier: Option<WitTraceContext>,
    clock: ServiceClock,
}

/// The data one `complete` call gave.
struct Completion(Option<WitDataValue>);

/// One `set-wakeup` request.
#[derive(Clone, Copy)]
enum Wakeup {
    /// Wake at this absolute logical ms.
    At(u64),
    /// Clear the pending wakeup.
    Clear,
}

/// Store data of one service instance.
pub(crate) struct ServiceStoreData {
    limits: MeteredLimits,
    providers: TelemetryProviders,
    execution_context: Context,
    guest_spans: crate::guest_spans::GuestSpanTable,
    type_uri: String,
    allow: crate::HttpAllowlist,
    http: Option<Arc<dyn ExtensionHttp>>,
    /// The package's declared `env` slots.
    env_slots: Vec<String>,
    /// The package's compiled event contract, checked on every `emit`.
    contract: Option<Arc<ServiceContract>>,
    /// The run's values, for binding this invocation's slots at `start`.
    values: crate::ExtensionEnvironment,
    /// This invocation's references and the values they stand for, bound
    /// at `start` and valid until the instance is dropped.
    references: Vec<(u32, String)>,
    call: Option<ServiceCall>,
    /// The time of the last call, for `now-ms` outside one.
    last_now_ms: u64,
    /// The instance's WASI context (reachable only through the interfaces
    /// its plan linked).
    wasi: crate::wasi::WasiState,
    _store_lease: LifecycleLease,
}

impl wasmtime_wasi::WasiView for ServiceStoreData {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        self.wasi.view()
    }
}

/// Lifecycle of one service instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Lifecycle {
    /// Started (or starting); `send` and `wake` are allowed.
    Live,
    /// `complete` was called.
    Completed,
    /// It trapped, failed a call, or emitted an invalid event.
    Retired,
    /// `cancel` was called.
    Cancelled,
}

/// What one export call produced, for the run to apply in order: the events
/// it emitted, then how it left the service.
#[derive(Debug)]
pub(crate) struct ServiceStep {
    /// Validated, stamped events for the invoking session, in emit order.
    pub(crate) events: Vec<WitEvent>,
    /// How the call left the service.
    pub(crate) outcome: ServiceOutcome,
}

/// How one export call left the service.
#[derive(Debug)]
pub(crate) enum ServiceOutcome {
    /// It runs on.
    Running,
    /// It called `complete` with this data.
    Completed(Option<WitDataValue>),
    /// It was retired (trap, failed call, or invalid emit).
    Failed,
}

/// One invocation's service instance and its pending wakeup.
pub(crate) struct ServiceHandle {
    store: wasmtime::Store<ServiceStoreData>,
    guest: service_bindings::InvokedService,
    lifecycle: Lifecycle,
    /// The pending wakeup, in absolute logical ms.
    wakeup: Option<u64>,
    call_fuel: u64,
}

impl ServiceHandle {
    /// Instantiates the bound component for one invocation in its own bounded
    /// store and checks its `type-uri` against the binding.
    ///
    /// # Errors
    ///
    /// [`ArtifactError::WasiImportUndeclared`] for a component importing a
    /// WASI interface its plan does not link, [`ArtifactError::WasiGrantInvalid`]
    /// for a granted directory that cannot be opened, a Wasmtime
    /// instantiation failure (for example a component that imports anything
    /// else), a trap in `type-uri`, or [`HostError::Component`]
    /// `service-component-uri-mismatch`.
    pub(crate) fn instantiate(
        component: &wasmtime::component::Component,
        runtime: &ServiceRuntime<'_>,
        execution_context: &Context,
    ) -> Result<Self, HostError> {
        runtime.wasi.check_imports(runtime.engine, component)?;
        let wasi = runtime.wasi.context()?;
        let config = &runtime.config.store;
        let limits = wasmtime::StoreLimitsBuilder::new()
            .memory_size(config.memory_bytes)
            .table_elements(config.table_elements)
            .instances(config.instances)
            .tables(config.tables)
            .memories(config.memories)
            .trap_on_grow_failure(true)
            .build();
        let mut store = wasmtime::Store::new(
            runtime.engine,
            ServiceStoreData {
                limits: MeteredLimits::new(limits, Arc::clone(runtime.call_budget)),
                providers: runtime.providers.clone(),
                execution_context: execution_context.clone(),
                guest_spans: crate::guest_spans::GuestSpanTable::new(),
                type_uri: runtime.package.service_uri.clone(),
                // Validated when the package was bound.
                allow: crate::HttpAllowlist::new(&runtime.package.http_origins)
                    .map_err(|_| HostError::from(ArtifactError::NamespaceBinding))?,
                http: runtime.http.map(Arc::clone),
                env_slots: runtime.package.env_slots.clone(),
                contract: runtime.contract.map(Arc::clone),
                values: runtime.values.clone(),
                references: Vec::new(),
                call: None,
                last_now_ms: 0,
                wasi,
                _store_lease: runtime.providers.acquire_store()?,
            },
        );
        store.limiter(|state| &mut state.limits);
        runtime.call_budget.share(&mut store);
        store.set_fuel(runtime.call_fuel)?;
        let guest =
            service_bindings::InvokedService::instantiate(&mut store, component, runtime.linker)?;
        store.set_fuel(runtime.call_fuel)?;
        let type_uri = guest
            .stateforward_scxml_invoked_service_api()
            .call_type_uri(&mut store)?;
        if type_uri != runtime.package.service_uri {
            return Err(HostError::Component(
                "service-component-uri-mismatch".into(),
            ));
        }
        Ok(Self {
            store,
            guest,
            lifecycle: Lifecycle::Live,
            wakeup: None,
            call_fuel: runtime.call_fuel,
        })
    }

    /// `start`: `Ok` with the call's step when the service accepted its
    /// start (it may already have completed or been retired by an invalid
    /// emit), `Err` when it rejected it or trapped (the `<invoke>` fails;
    /// the instance is retired).
    pub(crate) fn start(
        &mut self,
        request: &ServiceInvocation,
        clock: ServiceClock,
    ) -> Result<ServiceStep, ComponentError> {
        let bound = {
            let data = self.store.data();
            bind_references(&request.env, &data.env_slots, data.values.variables())
        };
        let (slots, references) = match bound {
            Ok(bound) => bound,
            Err(rejected) => {
                self.lifecycle = Lifecycle::Retired;
                return Err(rejected);
            }
        };
        self.store.data_mut().references = references;
        let carrier = request.trace_context.clone();
        let (succeeded, step) = self.call(clock, carrier, |guest, store| {
            guest
                .stateforward_scxml_invoked_service_api()
                .call_start(store, request, &slots)
        });
        if succeeded {
            Ok(step)
        } else {
            self.lifecycle = Lifecycle::Retired;
            self.wakeup = None;
            Err(component_error(
                ErrorCode::InvalidInput,
                "the invoked service rejected its start",
            ))
        }
    }

    /// `send`: one event the invoking session sent to the service.
    pub(crate) fn send(&mut self, event: &WitEvent, clock: ServiceClock) -> ServiceStep {
        let carrier = event.trace_context.clone();
        self.call(clock, carrier, |guest, store| {
            guest
                .stateforward_scxml_invoked_service_api()
                .call_send(store, event)
        })
        .1
    }

    /// `wake`: the requested wakeup fell due.
    pub(crate) fn wake(&mut self, clock: ServiceClock) -> ServiceStep {
        self.call(clock, None, |guest, store| {
            guest
                .stateforward_scxml_invoked_service_api()
                .call_wake(store)
        })
        .1
    }

    /// `cancel`, unless the service completed, was retired, or was already
    /// cancelled; whatever it returns, the instance takes no further call.
    pub(crate) fn cancel(&mut self, clock: ServiceClock) {
        if self.lifecycle != Lifecycle::Live {
            return;
        }
        let _ = self.call(clock, None, |guest, store| {
            guest
                .stateforward_scxml_invoked_service_api()
                .call_cancel(store)
        });
        self.lifecycle = Lifecycle::Cancelled;
        self.wakeup = None;
    }

    /// The instance's compiled event contract, when its package declares
    /// one.
    pub(crate) fn contract(&self) -> Option<Arc<ServiceContract>> {
        self.store.data().contract.clone()
    }

    /// The pending wakeup of a live service, in logical ms.
    pub(crate) const fn next_deadline(&self) -> Option<u64> {
        match self.lifecycle {
            Lifecycle::Live => self.wakeup,
            Lifecycle::Completed | Lifecycle::Retired | Lifecycle::Cancelled => None,
        }
    }

    /// Takes the wakeup when it is due at `now`.
    pub(crate) fn take_due(&mut self, now: u64) -> bool {
        match self.next_deadline() {
            Some(deadline) if deadline <= now => {
                self.wakeup = None;
                true
            }
            _ => false,
        }
    }

    /// Runs one export call with the host capabilities open and settles it:
    /// returns whether the call itself succeeded, and the step the run
    /// applies. A trap, an error, or an invalid emit retires the instance.
    fn call(
        &mut self,
        clock: ServiceClock,
        carrier: Option<WitTraceContext>,
        body: impl FnOnce(
            &service_bindings::InvokedService,
            &mut wasmtime::Store<ServiceStoreData>,
        ) -> wasmtime::Result<Result<(), ComponentError>>,
    ) -> (bool, ServiceStep) {
        if self.lifecycle != Lifecycle::Live {
            return (
                false,
                ServiceStep {
                    events: Vec::new(),
                    outcome: ServiceOutcome::Running,
                },
            );
        }
        self.store.data_mut().call = Some(ServiceCall {
            emitted: Vec::new(),
            completion: None,
            wakeup: None,
            invalid_emit: false,
            carrier,
            clock,
        });
        let result = self
            .store
            .set_fuel(self.call_fuel)
            .and_then(|()| body(&self.guest, &mut self.store));
        let Some(call) = self.store.data_mut().call.take() else {
            // Unreachable by construction (only this function sets and takes
            // the call); fail closed.
            self.lifecycle = Lifecycle::Retired;
            return (
                false,
                ServiceStep {
                    events: Vec::new(),
                    outcome: ServiceOutcome::Failed,
                },
            );
        };
        let succeeded = matches!(result, Ok(Ok(()))) && !call.invalid_emit;
        if !succeeded {
            self.lifecycle = Lifecycle::Retired;
            self.wakeup = None;
            return (
                false,
                ServiceStep {
                    events: call.emitted,
                    outcome: ServiceOutcome::Failed,
                },
            );
        }
        match call.wakeup {
            Some(Wakeup::At(at)) => self.wakeup = Some(at),
            Some(Wakeup::Clear) => self.wakeup = None,
            None => {}
        }
        let outcome = match call.completion {
            Some(Completion(data)) => {
                self.lifecycle = Lifecycle::Completed;
                self.wakeup = None;
                ServiceOutcome::Completed(data)
            }
            None => ServiceOutcome::Running,
        };
        (
            true,
            ServiceStep {
                events: call.emitted,
                outcome,
            },
        )
    }
}

/// One invocation's slot references and the values they stand for.
type BoundReferences = (Vec<SlotReference>, Vec<(u32, String)>);

/// The slot references of one invocation: for each `env` binding (sorted by
/// slot) whose slot `env_slots` declares, a fresh reference when `values`
/// holds the variable (numbered from 1 per invocation, so none crosses
/// instances); a variable with no value leaves its slot absent.
///
/// # Errors
///
/// `invalid-input` for more than [`crate::MAX_DECLARED_ITEMS`] bindings, a
/// slot the package does not declare or bound twice, or a name that is not a
/// variable name.
fn bind_references(
    env: &[EnvBinding],
    env_slots: &[String],
    values: &[(String, String)],
) -> Result<BoundReferences, ComponentError> {
    let invalid = |detail| Err(component_error(ErrorCode::InvalidInput, detail));
    if env.len() > crate::MAX_DECLARED_ITEMS {
        return invalid("the invocation binds more env slots than a package declares");
    }
    let mut slots = Vec::new();
    let mut references = Vec::new();
    for (index, binding) in env.iter().enumerate() {
        if !env_slots.contains(&binding.slot) {
            return invalid("an env slot is not declared by the service's package");
        }
        if env
            .get(..index)
            .is_some_and(|before| before.iter().any(|seen| seen.slot == binding.slot))
        {
            return invalid("the invocation binds one env slot twice");
        }
        if !crate::extension::is_env_name(&binding.name) {
            return invalid("an env slot names no valid variable");
        }
        let Some(value) = crate::extension::held(values, &binding.name) else {
            continue;
        };
        let reference = u32::try_from(references.len().saturating_add(1))
            .map_err(|_| component_error(ErrorCode::InvalidInput, "too many env slots"))?;
        references.push((reference, value.to_owned()));
        slots.push(SlotReference {
            slot: binding.slot.clone(),
            reference,
        });
    }
    Ok((slots, references))
}

fn unavailable(detail: &'static str) -> ComponentError {
    component_error(ErrorCode::Unavailable, detail)
}

impl ServiceStoreData {
    /// The open export call, unless the service already completed in it.
    fn open_call(&mut self) -> Result<&mut ServiceCall, ComponentError> {
        match self.call.as_mut() {
            Some(call) if call.completion.is_none() => Ok(call),
            Some(_) => Err(unavailable("the invoked service already completed")),
            None => Err(unavailable(
                "invoked-service host calls are available only during an export call",
            )),
        }
    }
}

impl invoked_service_host::Host for ServiceStoreData {
    fn emit(&mut self, event: WitEvent) -> Result<(), ComponentError> {
        let type_uri = self.type_uri.clone();
        let contract = self.contract.clone();
        let call = self.open_call()?;
        let event = WitEvent {
            name: event.name,
            event_type: WitEventType::External,
            send_id: event.send_id,
            origin: event.origin,
            origin_type: Some(type_uri),
            // The invoking session stamps the invoke id on delivery.
            invoke_id: None,
            data: event.data,
            raw: event.raw,
            trace_context: event.trace_context.or_else(|| call.carrier.clone()),
        };
        match native_event(event.clone()) {
            Ok(native) => {
                // A declared contract is closed: the event must be one the
                // package declares, with data its schema admits.
                if let Some(contract) = contract
                    && !contract.admits_emit(native.name.as_str(), &native.data)
                {
                    call.invalid_emit = true;
                    return Err(component_error(
                        ErrorCode::InvalidInput,
                        "the event is not one the service's package declares with data its schema admits",
                    ));
                }
                call.emitted.push(event);
                Ok(())
            }
            Err(error) => {
                call.invalid_emit = true;
                Err(error)
            }
        }
    }

    fn complete(&mut self, data: Option<WitDataValue>) -> Result<(), ComponentError> {
        let call = self.open_call()?;
        if let Some(value) = &data {
            native_value(value.clone())?;
        }
        call.completion = Some(Completion(data));
        Ok(())
    }

    fn set_wakeup(&mut self, delay_ms: u64) -> Result<(), ComponentError> {
        let call = self.open_call()?;
        call.wakeup = Some(if delay_ms == 0 {
            Wakeup::Clear
        } else {
            Wakeup::At(call.clock.now.saturating_add(delay_ms))
        });
        Ok(())
    }

    fn now_ms(&mut self) -> u64 {
        if let Some(call) = &self.call {
            self.last_now_ms = call.clock.start_epoch_ms.saturating_add(call.clock.now);
        }
        self.last_now_ms
    }
}

impl outbound_http::Host for ServiceStoreData {
    fn fetch(
        &mut self,
        request: outbound_http::HttpRequest,
    ) -> Result<outbound_http::HttpResponse, ComponentError> {
        if self.call.is_none() {
            return Err(unavailable(
                "outbound HTTP is available only during an export call",
            ));
        }
        let span = scxml_telemetry::observe(
            Module::HostWasmtime,
            Operation::HostWasmtimeServiceHttpFetch,
        );
        match crate::extension::fetch_admitted(
            request,
            &self.references,
            &self.allow,
            self.http.as_deref(),
            |_| {},
        ) {
            Ok(response) => {
                span.event(SignalEvent::TransformationCompleted);
                Ok(response)
            }
            Err((error, category)) => {
                span.event(SignalEvent::InputRejected);
                span.fail(category);
                Err(error)
            }
        }
    }
}

impl crate::bindings::stateforward::scxml::types::Host for ServiceStoreData {}

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
