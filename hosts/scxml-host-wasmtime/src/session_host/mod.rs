//! The Wasmtime session host: runs one root SCXML session, and the child
//! sessions it invokes, entirely through the shipped components.
//!
//! # Architecture
//!
//! [`SessionHost::new`] checks the kind and size of every component and
//! compiles the machine, the SCXML I/O processor, and the clock, which every
//! session needs. The compiler, the three data models, and the `BasicHTTP`
//! processor are compiled on first use, so a host whose sessions never
//! invoke a child document or name a data model never compiles them. Every
//! compilation goes through a process-wide cache (`compiled`): one bounded
//! Wasmtime engine per engine configuration and one compiled component per
//! authenticated SHA-256, shared by every live host on any thread and
//! compiled in-process from the authenticated bytes (Cranelift
//! `parallel-compilation`; nothing is read from or written to disk). The
//! cache holds weak references only: compiled code lives as long as some
//! host uses it. [`SessionHost::run`] then gives every session its own
//! stores:
//!
//! * a generic-machine store serving the session's Machine IR through
//!   `ir-source`, whose `machine-capabilities` the host implements;
//! * the data-model component the document names (`ecmascript`, `xpath`, or
//!   `null`), to which every data-model capability is forwarded one-to-one;
//! * an SCXML Event I/O Processor store linked to the session's own clock
//!   store (the capability-graph pattern);
//! * lazily, a `BasicHTTP` sender store whose transport is the in-process
//!   loopback (and, for a live session with a [`LiveHttp`] network, the
//!   embedder's [`OutboundHttp`] transport for every target outside the
//!   run's own address space), and one run-wide `BasicHTTP` receiver and
//!   compiler store.
//!
//! Every session owns its machine, data model, SCXML Event I/O Processor,
//! and clock, an internal event queue (in the machine), and an external
//! event queue. The pure `scxml-scheduler` crate is the cooperative
//! executor of the root session and its invoked sessions: it decides *which* session processes its next
//! external event, when `done.invoke` is queued, and when delayed sends come
//! due on the shared logical clock. The machine component decides *what* the
//! work means; the processors decide where sends go. The host never parses
//! SCXML or Machine IR, never selects transitions, and never classifies a
//! send target; it links, forwards, and executes component decisions.
//!
//! # Ownership and re-entrancy
//!
//! No session owns another: the run holds every session's machine store
//! (store, bindings, and session resource) in one flat table keyed by its
//! `SessionKey`, and the scheduler's `InvokedSessions` records who invoked
//! whom. A session's links know only the keys of the sessions it invoked,
//! by invoke id. An invoked session is constructed and registered inside
//! its invoking session's `start-invocation` call, which then suspends
//! (WIT `invocation-progress.deferred`); the host's loop starts it, and
//! likewise cancels a session and then drops it with every session below
//! it (deepest first), before it continues the suspended call. So no
//! session runs inside another's call, no store is ever entered
//! re-entrantly, and a chain of any depth takes no native stack per level.
//! Run-wide state (command budget, trace, root logs, every session's
//! external queue while a command runs, the deferred request, first fault,
//! compiler, and `BasicHTTP` receiver) lives in one single-threaded `Rc`
//! whose `RefCell` borrows are released before any component call.
//!
//! # Bounds
//!
//! Resource limits apply to the Wasm components only: the host imposes no
//! bound of its own on external queues, trace records, `<log>` records,
//! loaded documents, scripted events, or invoked sessions. What remains is
//! what the embedder chooses: its [`InvokeLimits`] (none by default; see
//! [`SessionHost::with_invoke_limits`]; an invocation beyond them fails as
//! that `<invoke>`'s `error.execution`), the run's command budget, the
//! configured fuel of one host call (one budget shared by every store the
//! call reaches, see `call_budget`), and the per-store memory, table, and
//! instance limits of each component.
//!
//! # Telemetry
//!
//! Boundary spans only, with closed labels: session-host construction, one
//! run, child start and cancel, child-document compilation, loopback posts,
//! a live session's calls (including `session-receive-http` and
//! `session-communication-error`), the `http` feature's
//! `basichttp-network-post`, `basichttp-listener-request`,
//! `basichttp-listener-accept`, and `basichttp-listener-connection`, and one
//! `session-snapshot` span per requested checkpoint snapshot (its
//! only measurement is the document size). Each component compilation (a
//! cache miss; a hit emits nothing) is one `component-compile` span with
//! the closed `scxml.outcome` label on the global tracer provider (see
//! `compiled`). Session ids, invoke ids, URIs,
//! events, payloads, documents, and snapshot error codes never become
//! attributes; commands and fuel are measurements.
//!
//! # Trace propagation
//!
//! The run span `session-run` is the child of the caller's current
//! context, and every session's machine batch span without a trigger
//! carrier is its child. Each event carries the W3C carrier of the batch
//! span that emitted it (see the WIT `trace-context`), and the host keeps
//! that carrier on every route: the SCXML processor's immediate and
//! delayed deliveries, the external queues between sessions, autoforward,
//! and the `BasicHTTP` loopback, which carries the sender's validated
//! carrier beside the request (the wire encoding has no trace headers, so
//! `_event.raw` never depends on telemetry). `done.invoke` carries the
//! child's finishing batch carrier ([`scxml_scheduler`] `Done` with the
//! call result's `done-trace-context`). Carriers enter from outside through
//! [`ScriptedEvent::trace_context`] and, for the external client, the
//! request's W3C `traceparent` and `tracestate` headers (an invalid
//! `traceparent` is ignored, as W3C Trace Context requires; inbound HTTP
//! baggage is dropped).

mod compiled;
mod links;
mod live;
mod loopback;
mod network;
mod release;
mod run;
mod snapshot;

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

pub use scxml_scheduler::InvokeLimits;

use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent};

use crate::call_budget::CallBudget;
use compiled::{Compiled, LazyComponent, SharedEngine};
pub(crate) use links::SessionLinks;
pub use live::{
    Applied, CloseReport, HttpReceipt, InputRecord, LiveCallResult, LiveClock, LiveSession,
    LiveStatus, ManualClock, SessionClock, SessionOpen, SessionOutput, WallClock,
};
pub use loopback::{LOOPBACK_ORIGIN, loopback_location};
pub use network::{
    CommunicationFailure, CommunicationSender, LiveHttp, MAX_PUBLIC_URL_BYTES, OutboundHttp,
    PublicUrl, PublicUrlError,
};
pub use release::{MAX_RELEASE_MANIFEST_BYTES, ReleaseComponents, ReleaseError, read_bounded_file};
pub use snapshot::{
    CheckpointSnapshot, CheckpointSnapshotFailure, SnapshotExport, SnapshotFailure, SnapshotPoints,
    SnapshotRequest,
};
pub(crate) use snapshot::{ExtensionImage, ExtensionState, extension_document};

use crate::basichttp_types::{HttpRequest, HttpResponse};
use crate::{
    ArtifactError, ArtifactKind, AuthenticatedComponent, HostError, MachineScripts,
    TelemetryProviders, WasmtimeHostConfig, WasmtimeStoreConfig,
};

/// The authenticated components one session host links. Each must carry
/// the matching [`ArtifactKind`]; the machine must be the generic
/// runtime-IR machine (it imports `ir-source`).
#[derive(Clone, Copy)]
pub struct SessionHostComponents<'a> {
    /// Generic `stateforward:scxml/machine` component.
    pub machine: &'a AuthenticatedComponent,
    /// `stateforward:scxml-compiler/compiler` component.
    pub compiler: &'a AuthenticatedComponent,
    /// ECMAScript data model.
    pub data_model_ecmascript: &'a AuthenticatedComponent,
    /// `XPath` data model.
    pub data_model_xpath: &'a AuthenticatedComponent,
    /// Null data model.
    pub data_model_null: &'a AuthenticatedComponent,
    /// SCXML Event I/O Processor.
    pub io_scxml: &'a AuthenticatedComponent,
    /// `BasicHTTP` Event I/O Processor.
    pub io_basichttp: &'a AuthenticatedComponent,
    /// Project clock.
    pub clock: &'a AuthenticatedComponent,
}

/// Kind of one document a session loads (`load-document`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentKind {
    /// An external `<data src>` document.
    Data,
    /// An `<invoke src>` child document.
    Invocation,
}

/// One document an embedder resolved: its text, and the media type its
/// source indicated when it indicated one (an HTTP `Content-Type`, or a
/// type the embedder derives from the resource, for example its file-name
/// extension). The machine reads a JSON or XML indication for `<data src>`
/// (SCXML B.2.1). A media type that is not visible ASCII is dropped at the
/// host boundary; one of any length passes it, and the machine component
/// (a wasm build) reads one of at most 256 bytes, a longer one indicating
/// nothing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddedDocument {
    /// The document text exactly as the resource holds it.
    pub text: String,
    /// The indicated media type, if any.
    pub media_type: Option<String>,
}

impl From<String> for EmbeddedDocument {
    /// A document whose source indicated no media type.
    fn from(text: String) -> Self {
        Self {
            text,
            media_type: None,
        }
    }
}

/// The embedder's per-run resources: external documents and scripts.
///
/// The host calls it synchronously from inside a session's capability
/// call; implementations must not call back into the host.
pub trait SessionEmbedder {
    /// Resolves one document exactly as authored by `uri`, with the media
    /// type its source indicated; `None` when it is not resolvable (the
    /// machine raises `error.execution`).
    fn document(&self, kind: DocumentKind, uri: &str) -> Option<EmbeddedDocument>;

    /// The external-script capability of one new session (root or child).
    fn scripts(&self) -> MachineScripts;
}

/// One external event the embedder injects into the root session (a batch
/// run's scripted event, or one [`LiveSession::send`]).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptedEvent {
    /// The event name (a valid SCXML event name, else the run fails with
    /// `event-name-invalid`).
    pub name: String,
    /// The event's `_event.data`; [`scxml_types::DataValue::Absent`] for
    /// none. The machine validates it against the event bounds.
    pub data: scxml_types::DataValue,
    /// The caller's W3C carrier. The root's batch span that processes the
    /// event becomes its child, and its baggage follows its baggage policy
    /// to every event that batch emits. Never changes semantics.
    pub trace_context: Option<scxml_types::TraceContext>,
}

impl ScriptedEvent {
    /// An event named `name` with no data and no carrier.
    #[must_use]
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            data: scxml_types::DataValue::Absent,
            trace_context: None,
        }
    }
}

/// One run of a root session.
pub struct SessionRun<'a> {
    /// The root document's lowered Machine IR (untrusted; the host serves it
    /// unbounded and the wasm machine applies its own byte bound).
    pub machine_ir: &'a [u8],
    /// The root document's data model, as its Machine IR records it (its
    /// `datamodel` attribute, or the default `ecmascript`); any value other
    /// than `ecmascript`, `xpath`, or `null` links none.
    pub data_model: Option<&'a str>,
    /// The root session's `_sessionid` (a valid SCXML name).
    pub session_id: &'a str,
    /// Scripted external events, delivered in order after `start`.
    pub events: &'a [ScriptedEvent],
    /// When present, the harness acts as an external HTTP client instead
    /// of scripting events: it POSTs `request(location)` to the root's
    /// `BasicHTTP` location after `start`.
    pub external_client: Option<&'a dyn Fn(&str) -> HttpRequest>,
    /// Documents and scripts for every session of the run.
    pub embedder: Rc<dyn SessionEmbedder>,
    /// Shared budget of engine commands across every session (non-zero).
    pub max_commands: u64,
    /// Read-only session snapshots to take at checkpoints;
    /// [`SnapshotRequest::none`] takes none and calls nothing.
    pub snapshots: SnapshotRequest,
}

/// Why the root document was rejected at load (SCXML 1.0 section 5.8).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadRejection {
    /// An external script could not be fetched (`unavailable`).
    ScriptUnavailable,
    /// An external script failed authentication (`invalid-input` or
    /// `payload-too-large`).
    ScriptInvalid,
}

/// How the root session ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Terminal {
    /// Still running when no session had work left.
    Running,
    /// Reached a top-level final state.
    Finished,
    /// Rejected at load; no session ran.
    Rejected(LoadRejection),
}

/// One `<log>` record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogEntry {
    /// The `<log>` label, when present.
    pub label: Option<String>,
    /// The rendered log text.
    pub text: String,
}

/// One observation of the run trace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TraceObservation {
    /// One engine command completed.
    Command,
    /// One `<raise>` was applied; the raised event name.
    Raised(String),
    /// One queued event was dequeued and processed by transition selection
    /// (selected or ignored); the event name. Recorded after the processing
    /// command's [`Self::Command`].
    Processed(String),
    /// One `<log>` was emitted.
    Log(LogEntry),
}

/// One trace observation attributed to the session that produced it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceEntry {
    /// The producing session's `_sessionid`.
    pub session: String,
    /// The invoking session's `_sessionid`; `None` for the root.
    pub parent: Option<String>,
    /// What the session did.
    pub observation: TraceObservation,
}

/// Observations of one completed run.
#[derive(Clone, Debug)]
pub struct SessionOutcome {
    /// How the root session ended.
    pub terminal: Terminal,
    /// The root's active leaf states in document order (empty when
    /// rejected).
    pub configuration: Vec<String>,
    /// Engine commands charged to the budget across every session.
    pub commands: u64,
    /// The root session's `<log>` records, in order.
    pub root_logs: Vec<LogEntry>,
    /// Cross-session observations, in the order the host observed them.
    pub trace: Vec<TraceEntry>,
    /// The response the external client received, for an external-client
    /// run.
    pub http_response: Option<HttpResponse>,
    /// The requested snapshots, in checkpoint order.
    pub snapshots: Vec<CheckpointSnapshot>,
    /// The requested snapshots that could not be taken, in checkpoint
    /// order. A snapshot failure never fails or ends the run.
    pub snapshot_failures: Vec<CheckpointSnapshotFailure>,
}

/// Why a run failed.
#[derive(Debug)]
pub enum RunFailure {
    /// The shared command budget was exhausted.
    CommandBudget,
    /// A machine call stopped at its per-call command bound without
    /// reaching a stable configuration.
    CommandBound,
    /// A machine session failed; the closed WIT code label and the
    /// component's bounded static detail.
    Machine {
        /// Closed WIT `error-code` label.
        code: &'static str,
        /// The component's bounded detail, when present.
        detail: Option<String>,
    },
    /// A host or scheduler invariant failed (stable reason code).
    Harness {
        /// Stable reason code.
        code: &'static str,
        /// Human-readable description (no payloads).
        message: String,
    },
    /// Wasmtime, artifact, or telemetry failure of the host itself.
    Host(HostError),
    /// A requested read-only session snapshot failed.
    Snapshot(SnapshotFailure),
}

impl From<scxml_scheduler::Violation> for RunFailure {
    fn from(violation: scxml_scheduler::Violation) -> Self {
        Self::Harness {
            code: violation.code(),
            message: violation.message().into(),
        }
    }
}

impl From<HostError> for RunFailure {
    fn from(error: HostError) -> Self {
        Self::Host(error)
    }
}

/// A failed run and the engine commands it charged before failing.
#[derive(Debug)]
pub struct SessionHostError {
    /// Why the run failed.
    pub failure: RunFailure,
    /// Engine commands charged before the failure.
    pub commands: u64,
}

impl std::fmt::Display for SessionHostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.failure {
            RunFailure::CommandBudget => formatter.write_str("the run exhausted its command budget"),
            RunFailure::CommandBound => formatter.write_str(
                "a machine call stopped at its per-call command bound before a stable configuration",
            ),
            RunFailure::Machine { code, .. } => write!(formatter, "machine session failed: {code}"),
            RunFailure::Harness { code, message } => write!(formatter, "[{code}] {message}"),
            RunFailure::Host(error) => error.fmt(formatter),
            RunFailure::Snapshot(failure) => write!(formatter, "session snapshot: {failure}"),
        }
    }
}

impl std::error::Error for SessionHostError {}

/// Why `load-extensions` could not give a session one of its namespaces.
#[derive(Debug)]
pub(crate) enum LoadFailure {
    /// No binding names the namespace and no resolver serves it, or the
    /// resolver failed: the session start fails with `unavailable`.
    Unavailable,
    /// A resolved binding was rejected, or the session's instance failed.
    Host(HostError),
}

impl From<HostError> for LoadFailure {
    fn from(error: HostError) -> Self {
        Self::Host(error)
    }
}

/// The engine and components one session host links, shared by every
/// store of every run.
///
/// The machine, the SCXML I/O processor, and the clock (which every session
/// needs) are compiled by [`SessionHost::new`]; the compiler, the data
/// models, and the `BasicHTTP` processor are compiled on first use. Every
/// compilation goes through the process-wide [`compiled`] cache.
pub(crate) struct HostCore {
    pub(crate) engine: wasmtime::Engine,
    shared: Arc<SharedEngine>,
    pub(crate) config: WasmtimeHostConfig,
    pub(crate) providers: TelemetryProviders,
    machine: Compiled,
    pub(crate) machine_linker: wasmtime::component::Linker<crate::component::MachineStoreData>,
    compiler: LazyComponent,
    data_models: [(&'static str, LazyComponent); 3],
    pub(crate) data_model_linker:
        wasmtime::component::Linker<crate::data_model::DataModelStoreData>,
    io: Compiled,
    pub(crate) io_linker: wasmtime::component::Linker<crate::capability_graph::IoStoreData>,
    clock: Compiled,
    pub(crate) clock_linker: wasmtime::component::Linker<crate::capability_graph::ClockStoreData>,
    basichttp: LazyComponent,
    /// Extension components keyed by exact namespace URI, compiled on first
    /// use; every session instantiates each of them. The embedder's bindings,
    /// then each namespace the resolver loaded, in load order. Single-threaded
    /// like the host: a borrow is never held across a guest call or a
    /// resolver call, and entries are only ever appended.
    namespaces: RefCell<Vec<(crate::ExtensionPackage, Rc<LazyComponent>)>>,
    /// The linker of every extension component store (built when the first
    /// extension component is bound or loaded).
    namespace_linker: OnceCell<wasmtime::component::Linker<crate::extension::ExtensionStoreData>>,
    /// The embedder's resolver of namespaces no binding names (none: such a
    /// document fails its session start).
    extension_resolver: Option<Arc<dyn crate::extension::ExtensionResolver>>,
    /// The run's values elements' `env` slots may reference.
    extension_values: Arc<crate::ExtensionEnvironment>,
    /// The transport admitted extension requests go through (none: refused).
    extension_http: Option<Arc<dyn crate::extension::ExtensionHttp>>,
    /// The embedder's observer of refused requests and missing values.
    extension_observer: Option<Arc<dyn crate::extension::ExtensionObserver>>,
    /// Fuel cap of one extension component call.
    pub(crate) namespace_call_fuel: u64,
    /// Invoked-service components keyed by exact type URI, compiled on first
    /// use; each invocation instantiates its own.
    services: Vec<ServiceEntry>,
    /// The transport admitted service requests go through (none: refused).
    service_http: Option<Arc<dyn crate::extension::ExtensionHttp>>,
    /// The deferred transport of invoked services (`outbound-http.begin`).
    service_deferred_http: Option<Arc<dyn crate::service_http::ServiceHttp>>,
    /// Fuel cap of one service export call.
    service_call_fuel: u64,
    /// The values services' `env` slots may reference.
    service_values: crate::ExtensionEnvironment,
    /// Release digests session snapshots record as provenance.
    pub(crate) digests: snapshot::ComponentDigests,
    /// The fuel budget of one host call, shared by every store of every
    /// session ([`crate::call_budget`]).
    pub(crate) call_budget: Arc<CallBudget>,
    /// Invoke limits of every run.
    pub(crate) invoke_limits: InvokeLimits,
}

/// One bound invoked service: its package, its component (compiled on first
/// use), its declared-and-granted WASI plan, and the linker of its stores
/// (built on first use from that plan).
pub(crate) struct ServiceEntry {
    package: crate::ServicePackage,
    contract: Option<std::sync::Arc<crate::service::ServiceContract>>,
    component: LazyComponent,
    wasi: crate::wasi::WasiPlan,
    linker: OnceCell<wasmtime::component::Linker<crate::service::ServiceStoreData>>,
}

impl ServiceEntry {
    fn linker(
        &self,
        engine: &wasmtime::Engine,
    ) -> Result<&wasmtime::component::Linker<crate::service::ServiceStoreData>, HostError> {
        if let Some(linker) = self.linker.get() {
            return Ok(linker);
        }
        let linker = crate::service::service_linker(engine, &self.wasi)?;
        Ok(self.linker.get_or_init(|| linker))
    }
}

impl HostCore {
    pub(crate) fn machine(&self) -> &wasmtime::component::Component {
        self.machine.component()
    }

    pub(crate) fn io(&self) -> &wasmtime::component::Component {
        self.io.component()
    }

    pub(crate) fn clock(&self) -> &wasmtime::component::Component {
        self.clock.component()
    }

    /// The compiler component, compiled on first use.
    pub(crate) fn compiler(&self) -> Result<&wasmtime::component::Component, HostError> {
        self.compiler.get(&self.shared)
    }

    /// The `BasicHTTP` processor component, compiled on first use.
    pub(crate) fn basichttp(&self) -> Result<&wasmtime::component::Component, HostError> {
        self.basichttp.get(&self.shared)
    }

    /// Every bound extension component with its exact URI, compiled on first
    /// use; empty when none is bound.
    pub(crate) fn extension_components(
        &self,
    ) -> Result<Vec<(crate::ExtensionPackage, wasmtime::component::Component)>, HostError> {
        let bound: Vec<(crate::ExtensionPackage, Rc<LazyComponent>)> = self
            .namespaces
            .borrow()
            .iter()
            .map(|(package, component)| (package.clone(), Rc::clone(component)))
            .collect();
        bound
            .into_iter()
            .map(|(package, component)| Ok((package, component.get(&self.shared)?.clone())))
            .collect()
    }

    /// The package of every bound namespace, for child compilation.
    pub(crate) fn extension_packages(&self) -> Vec<crate::ExtensionPackage> {
        self.namespaces
            .borrow()
            .iter()
            .map(|(package, _)| package.clone())
            .collect()
    }

    fn namespace_linker(
        &self,
    ) -> Result<&wasmtime::component::Linker<crate::extension::ExtensionStoreData>, HostError> {
        if let Some(linker) = self.namespace_linker.get() {
            return Ok(linker);
        }
        let linker = crate::extension::extension_linker(&self.engine)?;
        Ok(self.namespace_linker.get_or_init(|| linker))
    }

    /// The instantiation runtime of one package's extension component: its
    /// own declared slots and origins.
    fn extension_runtime<'a>(
        &'a self,
        linker: &'a wasmtime::component::Linker<crate::extension::ExtensionStoreData>,
        package: &'a crate::ExtensionPackage,
    ) -> crate::extension::ExtensionRuntime<'a> {
        crate::extension::ExtensionRuntime {
            engine: &self.engine,
            linker,
            config: &self.config,
            providers: &self.providers,
            call_budget: &self.call_budget,
            package,
            values: &self.extension_values,
            http: self.extension_http.as_ref(),
            observer: self.extension_observer.as_ref(),
            call_fuel: self.namespace_call_fuel,
        }
    }

    /// One session's instances of every bound extension component, each
    /// identity-checked and initialized; `None` when none is bound.
    pub(crate) fn session_namespaces(
        &self,
        execution_context: &opentelemetry::Context,
    ) -> Result<Option<crate::extension::SessionNamespaces>, HostError> {
        let bound = self.extension_components()?;
        if bound.is_empty() {
            return Ok(None);
        }
        let linker = self.namespace_linker()?;
        let mut namespaces = crate::extension::SessionNamespaces::empty();
        for (package, component) in &bound {
            namespaces.add(
                &package.namespace_uri,
                component,
                &self.extension_runtime(linker, package),
                execution_context,
            )?;
        }
        Ok(Some(namespaces))
    }

    /// `machine-capabilities.load-extensions` for one session: gives
    /// `namespaces` an instance of the extension component of every URI in
    /// `namespace_uris` it lacks. A namespace no binding names is loaded
    /// through the embedder's resolver, checked like a binding (the package's
    /// URI must equal the requested one byte for byte), and bound for every
    /// later session.
    ///
    /// # Errors
    ///
    /// [`LoadFailure::Unavailable`] when a namespace has no binding and the
    /// resolver does not serve it or fails; [`LoadFailure::Host`] when a
    /// resolved binding is rejected or an instance fails.
    pub(crate) fn load_extensions(
        &self,
        namespaces: &mut Option<crate::extension::SessionNamespaces>,
        namespace_uris: &[String],
        execution_context: &opentelemetry::Context,
    ) -> Result<(), LoadFailure> {
        for namespace_uri in namespace_uris {
            if namespaces
                .as_ref()
                .is_some_and(|loaded| loaded.contains(namespace_uri))
            {
                continue;
            }
            let (package, component) = self.bound_or_resolved(namespace_uri)?;
            let linker = self.namespace_linker()?;
            namespaces
                .get_or_insert_with(crate::extension::SessionNamespaces::empty)
                .add(
                    namespace_uri,
                    &component,
                    &self.extension_runtime(linker, &package),
                    execution_context,
                )?;
        }
        Ok(())
    }

    /// The compiled component bound to exactly `namespace_uri`, loading and
    /// binding it through the resolver when no binding names it.
    fn bound_or_resolved(
        &self,
        namespace_uri: &str,
    ) -> Result<(crate::ExtensionPackage, wasmtime::component::Component), LoadFailure> {
        let bound = self
            .namespaces
            .borrow()
            .iter()
            .find(|(package, _)| package.namespace_uri == namespace_uri)
            .map(|(package, component)| (package.clone(), Rc::clone(component)));
        if let Some((package, component)) = bound {
            return Ok((package, component.get(&self.shared)?.clone()));
        }
        let resolved = self
            .extension_resolver
            .as_ref()
            .ok_or(LoadFailure::Unavailable)?
            .resolve(namespace_uri)
            .map_err(|_| LoadFailure::Unavailable)?
            .ok_or(LoadFailure::Unavailable)?;
        if resolved.package.namespace_uri != namespace_uri {
            return Err(LoadFailure::Host(
                crate::ArtifactError::NamespaceBinding.into(),
            ));
        }
        crate::capability_graph::validate_component(
            &resolved.component,
            resolved.component.kind(),
            &self.config,
        )?;
        let component = Rc::new(LazyComponent::new(&resolved.component));
        {
            let mut namespaces = self.namespaces.borrow_mut();
            crate::extension::check_binding(
                &resolved.package,
                &resolved.component,
                namespaces
                    .iter()
                    .map(|(package, _)| package.namespace_uri.as_str()),
            )?;
            namespaces.push((resolved.package.clone(), Rc::clone(&component)));
        }
        Ok((resolved.package, component.get(&self.shared)?.clone()))
    }

    /// Whether a service component is bound to exactly `type_uri`.
    pub(crate) fn binds_service(&self, type_uri: &str) -> bool {
        self.service(type_uri).is_some()
    }

    fn service(&self, type_uri: &str) -> Option<&ServiceEntry> {
        self.services
            .iter()
            .find(|entry| entry.package.service_uri == type_uri)
    }

    /// Loads the service bound to exactly `type_uri` without instantiating
    /// it: compiles its component, refuses a WASI import its plan does not
    /// link, and pre-instantiates it against its linker (every other import
    /// must be the world's). `Ok(false)` when no service is bound to it.
    ///
    /// # Errors
    ///
    /// A compilation or linker failure, or
    /// [`crate::ArtifactError::WasiImportUndeclared`].
    pub(crate) fn load_service(&self, type_uri: &str) -> Result<bool, HostError> {
        let Some(entry) = self.service(type_uri) else {
            return Ok(false);
        };
        let component = entry.component.get(&self.shared)?;
        entry.wasi.check_imports(&self.engine, component)?;
        entry.linker(&self.engine)?.instantiate_pre(component)?;
        Ok(true)
    }

    /// One new instance of the service bound to exactly `type_uri`, its
    /// `type-uri` checked; `None` when no service is bound to it.
    ///
    /// # Errors
    ///
    /// A compilation, linker, or instantiation failure, or a `type-uri`
    /// that differs from the binding.
    pub(crate) fn instantiate_service(
        &self,
        type_uri: &str,
        execution_context: &opentelemetry::Context,
        responses: Option<&Arc<crate::service_http::ServiceResponses>>,
    ) -> Result<Option<crate::service::ServiceHandle>, HostError> {
        let Some(entry) = self.service(type_uri) else {
            return Ok(None);
        };
        let component = entry.component.get(&self.shared)?;
        let runtime = crate::service::ServiceRuntime {
            engine: &self.engine,
            linker: entry.linker(&self.engine)?,
            config: &self.config,
            providers: &self.providers,
            call_budget: &self.call_budget,
            package: &entry.package,
            contract: entry.contract.as_ref(),
            wasi: &entry.wasi,
            http: self.service_http.as_ref(),
            deferred: self.service_deferred_http.as_ref(),
            responses,
            values: &self.service_values,
            call_fuel: self.service_call_fuel,
        };
        crate::service::ServiceHandle::instantiate(component, &runtime, execution_context).map(Some)
    }

    /// The data model named exactly `name`, compiled on first use; `None`
    /// when the host links no data model of that name.
    pub(crate) fn data_model(
        &self,
        name: &str,
    ) -> Result<Option<&wasmtime::component::Component>, HostError> {
        self.data_models
            .iter()
            .find(|(linked, _)| *linked == name)
            .map(|(_, component)| component.get(&self.shared))
            .transpose()
    }
}

/// A reusable Wasmtime session host: one engine and one compiled component
/// per artifact, shared by every run. Single-threaded (`!Send`); create one
/// per thread. Hosts share compiled components process-wide, so a second
/// host over the same authenticated components (on any thread) compiles
/// nothing while another host holds them.
pub struct SessionHost {
    core: Rc<HostCore>,
}

impl std::fmt::Debug for SessionHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionHost")
            .finish_non_exhaustive()
    }
}

impl SessionHost {
    /// Authenticates the release session components of one
    /// `scripts/verify-wasm-capabilities.sh` artifacts directory
    /// ([`ReleaseComponents::load`]) and links them into one session host.
    ///
    /// `manifest_sha256` is the caller-trusted SHA-256 of that directory's
    /// `manifest.tsv` (delivered through the application's authenticated
    /// release channel); `fuel` is the deterministic store fuel of every
    /// session run. Callers acquire manifest and artifact bytes through
    /// their own explicit capability boundary; this constructor reads only
    /// the named directory, every file through [`read_bounded_file`]: the
    /// manifest at [`MAX_RELEASE_MANIFEST_BYTES`] and every component at
    /// [`crate::MAX_AUTHENTICATED_COMPONENT_BYTES`].
    ///
    /// # Errors
    ///
    /// [`RunFailure::Host`] for an untrusted digest
    /// ([`HostError::Artifact`]), an unreadable, oversized, or
    /// unauthenticated release file ([`HostError::Release`], which names the
    /// artifact and manifest), invalid host limits, or a Wasmtime
    /// compilation or linker failure.
    pub fn from_release_dir(
        dir: &std::path::Path,
        manifest_sha256: &str,
        fuel: u64,
    ) -> Result<Self, SessionHostError> {
        let host_error = |error: HostError| SessionHostError {
            failure: RunFailure::Host(error),
            commands: 0,
        };
        let trusted = crate::TrustedManifestDigest::from_hex(manifest_sha256)
            .map_err(|error| host_error(error.into()))?;
        let release = ReleaseComponents::load(dir, trusted).map_err(host_error)?;
        let config = WasmtimeHostConfig {
            store: WasmtimeStoreConfig {
                fuel,
                ..WasmtimeStoreConfig::default()
            },
            ..WasmtimeHostConfig::default()
        };
        Self::new(
            &release.session_components(),
            &config,
            &TelemetryProviders::new(),
        )
        .map_err(host_error)
    }

    /// Validates every component's kind and size, then compiles (or reuses
    /// the process-wide compilation of) the machine, the SCXML I/O
    /// processor, and the clock. The other components compile on first use
    /// inside [`Self::run`], whose failures then carry their Wasmtime
    /// compilation errors as [`RunFailure::Host`].
    ///
    /// # Errors
    ///
    /// Invalid host limits, a component of the wrong kind or over the byte
    /// bound, a machine that does not import `ir-source`, and Wasmtime
    /// compilation or linker failures.
    pub fn new(
        components: &SessionHostComponents<'_>,
        config: &WasmtimeHostConfig,
        providers: &TelemetryProviders,
    ) -> Result<Self, HostError> {
        let span =
            scxml_telemetry::observe(Module::HostWasmtime, Operation::HostWasmtimeSessionHostNew);
        let result = Self::new_inner(components, config, providers);
        if result.is_ok() {
            span.event(SignalEvent::ArtifactAccepted);
        } else {
            span.event(SignalEvent::InputRejected);
            span.fail(ErrorCategory::Validation);
        }
        result
    }

    fn new_inner(
        components: &SessionHostComponents<'_>,
        config: &WasmtimeHostConfig,
        providers: &TelemetryProviders,
    ) -> Result<Self, HostError> {
        config.validate()?;
        let checks = [
            (components.machine, ArtifactKind::Machine),
            (components.compiler, ArtifactKind::Compiler),
            (
                components.data_model_ecmascript,
                ArtifactKind::DataModelEcmascript,
            ),
            (components.data_model_xpath, ArtifactKind::DataModelXpath),
            (components.data_model_null, ArtifactKind::DataModelNull),
            (components.io_scxml, ArtifactKind::IoProcessorScxml),
            (components.io_basichttp, ArtifactKind::IoProcessorBasicHttp),
            (components.clock, ArtifactKind::Clock),
        ];
        for (component, kind) in checks {
            crate::capability_graph::validate_component(component, kind, config)?;
        }
        let shared = SharedEngine::for_config(config)?;
        let engine = shared.engine().clone();
        let machine = shared.compile(components.machine)?;
        if machine
            .component()
            .component_type()
            .get_import(&engine, crate::session::IR_SOURCE_INTERFACE)
            .is_none()
        {
            return Err(ArtifactError::IrSourceModeMismatch.into());
        }
        let core = HostCore {
            machine_linker: crate::component::machine_linker(&engine)?,
            machine,
            compiler: LazyComponent::new(components.compiler),
            data_models: [
                (
                    "ecmascript",
                    LazyComponent::new(components.data_model_ecmascript),
                ),
                ("xpath", LazyComponent::new(components.data_model_xpath)),
                ("null", LazyComponent::new(components.data_model_null)),
            ],
            data_model_linker: crate::data_model::data_model_linker(&engine)?,
            io: shared.compile(components.io_scxml)?,
            io_linker: crate::capability_graph::io_linker(&engine)?,
            clock: shared.compile(components.clock)?,
            clock_linker: crate::capability_graph::clock_linker(&engine)?,
            basichttp: LazyComponent::new(components.io_basichttp),
            namespaces: RefCell::new(Vec::new()),
            namespace_linker: OnceCell::new(),
            extension_resolver: None,
            extension_values: Arc::new(crate::ExtensionEnvironment::none()),
            extension_http: None,
            extension_observer: None,
            namespace_call_fuel: crate::extension::DEFAULT_EXTENSION_CALL_FUEL,
            services: Vec::new(),
            service_http: None,
            service_deferred_http: None,
            service_call_fuel: crate::service::DEFAULT_SERVICE_CALL_FUEL,
            service_values: crate::ExtensionEnvironment::none(),
            digests: snapshot::ComponentDigests {
                machine: components.machine.sha256(),
                clock: components.clock.sha256(),
                io_scxml: components.io_scxml.sha256(),
                ecmascript: components.data_model_ecmascript.sha256(),
                xpath: components.data_model_xpath.sha256(),
                null: components.data_model_null.sha256(),
            },
            call_budget: CallBudget::new(config.store.fuel),
            invoke_limits: InvokeLimits::default(),
            config: config.clone(),
            providers: providers.clone(),
            engine,
            shared,
        };
        Ok(Self {
            core: Rc::new(core),
        })
    }

    /// Sets the invoke limits of every run: the deepest nested invoke depth
    /// (a session the root session invokes is at depth 1) and the most
    /// invoked sessions alive under the root session at once (the root
    /// session not counted). By default neither is limited: the host
    /// imposes no invoke limit of its own. An invocation beyond the limits
    /// set here cannot start: the invoking session receives
    /// `error.execution` for that `<invoke>`, as for any invocation that
    /// fails to start (SCXML 6.4); nothing else is visible to a document.
    ///
    /// # Errors
    ///
    /// [`HostError::Component`] (`session-host-shared`) when a run of this
    /// host is in progress.
    pub fn with_invoke_limits(mut self, limits: InvokeLimits) -> Result<Self, HostError> {
        let core = Rc::get_mut(&mut self.core)
            .ok_or_else(|| HostError::Component("session-host-shared".into()))?;
        core.invoke_limits = limits;
        Ok(self)
    }

    /// Binds extension components to their exact namespace
    /// URIs for every session this host runs (root and invoked children).
    ///
    /// Each extension component compiles on first use through the process-wide
    /// cache. Every session then gets its own instance of each bound extension
    /// component: the host checks its `namespace-uri` against the binding and
    /// calls `initialize` when the session is constructed (a mismatch or a
    /// failed `initialize` fails the run with [`RunFailure::Host`]), routes
    /// every executable-content extension by its exact namespace URI, and calls
    /// `shutdown` when the session ends. A namespace a session's document uses
    /// that no binding names is loaded through the resolver
    /// [`crate::ExtensionComponents::with_resolver`] supplies when that session
    /// starts; without one, or when it cannot load it, the session start fails
    /// closed. Extension components import no WASI interface (a package that
    /// declares one is refused, [`crate::ArtifactError::WasiExtensionUnsupported`])
    /// and never receive the embedder's values ([`crate::ExtensionComponents::with_values`]),
    /// only per-execution references to the ones their element's `env` slots
    /// name; their requests go through the embedder's transport (none by
    /// default: every `outbound-http` request is refused), admitted only to an
    /// origin their package declares or a reference's value supplies.
    ///
    /// # Errors
    ///
    /// A component over the host's byte bound, a Wasmtime linker failure,
    /// or [`HostError::Component`] (`session-host-shared`) when a run of
    /// this host is in progress.
    pub fn with_extension_components(
        mut self,
        extensions: crate::ExtensionComponents,
    ) -> Result<Self, HostError> {
        let core = Rc::get_mut(&mut self.core)
            .ok_or_else(|| HostError::Component("session-host-shared".into()))?;
        let crate::extension::ExtensionParts {
            bindings,
            values,
            http,
            observer,
            call_fuel,
            resolver,
        } = extensions.into_parts();
        for binding in &bindings {
            crate::capability_graph::validate_component(
                &binding.component,
                binding.component.kind(),
                &core.config,
            )?;
        }
        core.namespace_linker = OnceCell::new();
        core.namespaces = RefCell::new(
            bindings
                .into_iter()
                .map(|binding| {
                    let component = Rc::new(LazyComponent::new(&binding.component));
                    (binding.package, component)
                })
                .collect(),
        );
        core.extension_resolver = resolver;
        core.extension_values = values;
        core.extension_http = http;
        core.extension_observer = observer;
        core.namespace_call_fuel = call_fuel;
        Ok(self)
    }

    /// Binds invoked-service components to their exact type URIs for every
    /// session this host runs (root and invoked children).
    ///
    /// Each component compiles on first use through the process-wide cache.
    /// Every `<invoke>` of a bound type then gets its own instance in its own
    /// bounded store: the host checks its `type-uri` against the binding
    /// (a mismatch fails the run with [`RunFailure::Host`]), calls `start`,
    /// delivers the events the invoking session sends it, wakes it on the
    /// run's logical clock, and calls `cancel` when the invoking state exits
    /// first. An `<invoke>` of a type no service binds, literal or
    /// `typeexpr`, fails when it starts with `error.execution` (SCXML 6.4);
    /// the session start is unaffected. A service component imports no WASI interface
    /// beyond what its package declares and the run grants
    /// ([`crate::ServiceComponents::with_wasi_grants`]): the host links
    /// exactly those, and refuses a component importing any other WASI
    /// interface when a session that names its type literally starts (before
    /// the first macrostep) and before every instantiation. Its requests go
    /// through the embedder's transport (none by default), admitted only to
    /// an origin its package declares.
    ///
    /// # Errors
    ///
    /// A component over the host's byte bound or of the wrong kind; a
    /// declared WASI permission the grants do not cover
    /// ([`crate::ArtifactError::WasiPermissionUngranted`]) or a grant no
    /// bound package declares ([`crate::ArtifactError::WasiGrantUndeclared`]);
    /// or [`HostError::Component`] (`session-host-shared`) when a run of this
    /// host is in progress.
    pub fn with_service_components(
        mut self,
        services: crate::ServiceComponents,
    ) -> Result<Self, HostError> {
        let core = Rc::get_mut(&mut self.core)
            .ok_or_else(|| HostError::Component("session-host-shared".into()))?;
        let crate::service::ServiceParts {
            bindings,
            http,
            deferred,
            values,
            call_fuel,
        } = services.into_parts()?;
        for (binding, _) in &bindings {
            crate::capability_graph::validate_component(
                &binding.component,
                binding.component.kind(),
                &core.config,
            )?;
        }
        core.services = bindings
            .into_iter()
            .map(|(binding, wasi)| ServiceEntry {
                component: LazyComponent::new(&binding.component),
                package: binding.package,
                contract: binding.contract,
                wasi,
                linker: OnceCell::new(),
            })
            .collect();
        core.service_http = http;
        core.service_deferred_http = deferred;
        core.service_call_fuel = call_fuel;
        core.service_values = values;
        Ok(self)
    }

    /// Opens one live root session (see [`LiveSession`] and
    /// `docs/live-session.md`): constructs the root, starts it, and runs
    /// every session to a stable configuration at logical time 0. The session then
    /// stays alive until [`LiveSession::close`] or drop.
    ///
    /// # Errors
    ///
    /// [`SessionHostError`]: a zero budget (`command-budget-invalid`), a root
    /// document rejected at load (`script-unavailable` or
    /// `script-invalid`), or any failure of the start call.
    pub fn open(&self, open: SessionOpen<'_>) -> Result<LiveSession, SessionHostError> {
        live::open(&self.core, open)
    }

    /// Runs one root session and every child it invokes until the root
    /// finishes or no session has work left.
    ///
    /// Scripted runs start the root, deliver `events` in order, then drain
    /// every external queue and delayed send on the shared logical clock
    /// (`scxml_scheduler::Scheduler::run_scripted`); external-client runs
    /// start the root, POST the client's request through the loopback, then
    /// drain (`run_external_client`). It drives the same engine as a live
    /// session ([`Self::open`]) through the same stepwise scheduler calls,
    /// with the logical clock unbounded and no call boundary between the
    /// inputs.
    ///
    /// # Errors
    ///
    /// [`SessionHostError`] with the commands charged so far: a zero
    /// budget or an invalid scripted event ([`RunFailure::Harness`]),
    /// budget exhaustion, a failed machine session, a scheduler invariant,
    /// or a host failure. Every store is dropped before return.
    pub fn run(&self, run: &SessionRun<'_>) -> Result<SessionOutcome, SessionHostError> {
        let span =
            scxml_telemetry::observe(Module::HostWasmtime, Operation::HostWasmtimeSessionRun);
        let result = run::execute(&self.core, run);
        match &result {
            Ok(_) => span.event(SignalEvent::TransformationCompleted),
            Err(error) => {
                span.event(SignalEvent::InputRejected);
                span.fail(match error.failure {
                    RunFailure::CommandBudget | RunFailure::CommandBound => ErrorCategory::Budget,
                    RunFailure::Machine { .. } => ErrorCategory::Execution,
                    RunFailure::Harness { .. } => ErrorCategory::Harness,
                    RunFailure::Host(_) => ErrorCategory::Platform,
                    RunFailure::Snapshot(_) => ErrorCategory::Validation,
                });
            }
        }
        result
    }
}
