//! Extension components (SCXML 1.0 section 4.10) under the
//! session host.
//!
//! The machine component lowers every executable element of an extension
//! namespace to `machine-capabilities.execute-extension`; this module is the
//! host side of that capability and stays a thin linker:
//!
//! * **Binding** — [`ExtensionComponents`] binds exact namespace URIs to
//!   authenticated `stateforward:scxml/executable-content-extension` components
//!   (release artifacts, or caller components authenticated by SHA-256 through
//!   [`crate::authenticate_extension_component`]). Components compile lazily
//!   through the session host's process-wide cache.
//! * **Loading** — before data binding and its first macrostep, every session
//!   asks for the extension namespaces its document uses
//!   (`machine-capabilities.load-extensions`). A namespace no binding names is
//!   loaded through the embedder's [`ExtensionResolver`]
//!   ([`ExtensionComponents::with_resolver`]), checked exactly like a binding,
//!   and bound for every later session; one the resolver does not serve, or
//!   fails on, fails the session start closed. The host acquires nothing
//!   itself.
//! * **Lifecycle** — every session (root and each invoked child) gets its own
//!   instance of every bound extension component in its own bounded store (of
//!   one the resolver loads, when its session starts): the host checks the
//!   instance's `namespace-uri` against the binding and calls `initialize`, and
//!   calls `shutdown` exactly once when the session's store is dropped, whether
//!   the session finished, failed, or was cancelled.
//! * **Execution** — an element is routed by its exact namespace URI; an
//!   unbound URI answers `unavailable`, which fails the session closed. For the
//!   call, the session's data model, links, and configuration move into the
//!   extension component store (and always move back), so the extension
//!   component's `data-model-host` and `execution-host` act on the executing
//!   session with the same semantics and bounds as the machine's own
//!   capabilities. Raised events and self-addressed sends are returned to the
//!   machine, which queues them.
//! * **Bounds** — each `execute` call runs under the per-call fuel cap and on
//!   the session host's per-call budget (`call_budget`), so its instructions
//!   are charged to the machine call that executed it. A trap retires the
//!   instance for the rest of the session.
//! * **No I/O** — an extension element is executable content, run
//!   synchronously inside a microstep, so its component imports no WASI
//!   interface and no `outbound-http`, and receives no `env` slot values or
//!   references: this host links none of them into it, so one that imports
//!   any fails to instantiate. Work that waits on the outside world belongs
//!   in an invoked service ([`crate::ServiceComponents`]).
//!
//! * **Session snapshot** — `stateforward:scxml/session-snapshot-host` is
//!   read-only: an extension component whose component imports it makes the
//!   machine pass its executable-content image with each of the extension
//!   component's elements ([`SessionNamespaces::reads_snapshot`]), and
//!   `snapshot` assembles the executing session's canonical document from it
//!   and the lent session state (`session_host::extension_document`), at most
//!   [`MAX_EXTENSION_SNAPSHOTS`] times per `execute` call.
//!
//! Telemetry: the load (`extension-load`, one per `load-extensions` call),
//! instantiate, execute, shutdown, and `session-snapshot` (one per extension component snapshot) boundary spans
//! carry closed labels only (the span name, the closed signal event, and the
//! closed error category); namespace URIs, element names, attributes, and log
//! text never become attributes. Guest telemetry imports are recorded like script components'
//! (`executable-content-extension`).

use std::sync::Arc;

use opentelemetry::trace::TraceContextExt as _;
use opentelemetry::{Context, KeyValue};
use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent};
use wasmtime::component::{HasSelf, Linker};

use crate::bindings::stateforward::scxml::telemetry;
use crate::bindings::stateforward::scxml::types::{
    ComponentError, DataValue as WitDataValue, ErrorCode, ErrorVersion, Event as WitEvent,
    EventType as WitEventType, ExecutableContentExtension, ExtensionOutcome, LogLevel, SendRequest,
    SendTarget, TraceContext as WitTraceContext,
};
use crate::call_budget::CallBudget;
use crate::component::{GuestSignal, component_error, record_guest_signal};
use crate::memory_growth::MeteredLimits;
use crate::session::{MachineDataModel, failure_code, native_event, native_value, wit_value};
use crate::session_host::SessionLinks;
use crate::{
    ArtifactError, ArtifactKind, AuthenticatedComponent, HostError, InvalidLimit, LifecycleLease,
    TelemetryProviders, WasmtimeHostConfig,
};

pub mod extension_bindings {
    // The host drives every extension component call through the async
    // ABI to completion before it returns, so `execute` stays synchronous
    // executable content.
    wasmtime::component::bindgen!({
        path: "../../crates/scxml-wit/wit/scxml",
        world: "executable-content-extension",
        with: {
            "stateforward:scxml/types": crate::bindings::stateforward::scxml::types,
            "stateforward:scxml/telemetry": crate::bindings::stateforward::scxml::telemetry,
        },
        exports: { default: async },
        require_store_data_send: true,
    });
}

use extension_bindings::stateforward::scxml::{
    data_model_host, execution_host, session_snapshot_host,
};

/// Default instruction fuel cap of one extension component call (`initialize`,
/// `execute`, or `shutdown`); every call also runs on the session host's
/// per-call budget.
///
/// Rationale for one billion: an element may link a whole validator, and
/// one call then compiles the bundled W3C SCXML 1.0 XSD set before it
/// validates anything. Measured with `<scx:validate>` (`namespaces/scx`: the scxml.rs
/// validator in one component, release build, `wasm32-wasip2`), one call
/// takes about 166 million instructions for a two-state document, 191
/// million for a 40-state document, and 214 million for a 1.2 KiB
/// model-written draft; natively about 85% of that time is the XSD set's
/// compilation (`xsd_schema::process_loaded_schemas`), which `xsd-schema`
/// can neither serialize nor share across documents (its name table interns
/// each instance's names). One billion covers such a call several times
/// over; it stays a bound, not a target, and an exhausted call still
/// retires the instance.
pub const DEFAULT_EXTENSION_CALL_FUEL: u64 = 1_000_000_000;
/// Maximum namespaces one session host binds.
pub const MAX_EXTENSION_COMPONENTS: usize = 64;
/// Maximum bytes of one bound namespace URI (the package-manifest field
/// bound).
pub const MAX_NAMESPACE_URI_BYTES: usize = 4_096;
/// Maximum bytes of one bound namespace schema (the compiler's schema bound).
pub const MAX_NAMESPACE_SCHEMA_BYTES: usize = 4 * 1024 * 1024;
/// Maximum `session-snapshot-host.snapshot` calls of one `execute` call. Each
/// call costs the host work linear in the session's state for a few guest
/// instructions, so the count is bounded on every build (an extension component
/// cannot amplify its fuel into unmetered host work).
pub const MAX_EXTENSION_SNAPSHOTS: u32 = 64;
/// The import name of `session-snapshot-host`: an extension component whose
/// component imports it reads the machine's executable-content image.
const SESSION_SNAPSHOT_HOST_IMPORT: &str = "stateforward:scxml/session-snapshot-host@0.2.0";

/// The exact SCXML namespace, which no extension component may claim.
const SCXML_NAMESPACE: &str = "http://www.w3.org/2005/07/scxml";
/// The built-in XML namespace, which no extension component may claim.
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

/// One extension package manifest and its schema bytes (the
/// compiler's `extension-package` and `schema-resource`).
///
/// The session host never interprets it: it forwards it to the compiler
/// component whenever a session compiles an invoked child document, so a
/// child may use every namespace its parent's host binds. The compiler
/// verifies the manifest's identity and the schema's SHA-256.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionPackage {
    /// Exact namespace URI; the binding key.
    pub namespace_uri: String,
    /// Package version.
    pub package_version: String,
    /// Extension component artifact identifier.
    pub extension_artifact: String,
    /// Schema artifact identifier (the schema's system id).
    pub schema_artifact: String,
    /// Lowercase SHA-256 of `schema`.
    pub schema_sha256: String,
    /// WIT world the extension component implements.
    pub wit_world: String,
    /// WIT version.
    pub wit_version: String,
    /// The schema bytes (at most [`MAX_NAMESPACE_SCHEMA_BYTES`]).
    pub schema: Vec<u8>,
}

/// What an [`ExtensionResolver`] answers for one namespace: the package to
/// bind and its extension component, authenticated by the resolver's own trust
/// anchor (for example the digest a release's `SHA256SUMS` names, through
/// [`crate::authenticate_extension_component`]).
#[derive(Clone, Debug)]
pub struct ResolvedExtension {
    /// The package; its `namespace_uri` must equal the requested URI byte for
    /// byte.
    pub package: ExtensionPackage,
    /// The authenticated extension component.
    pub component: AuthenticatedComponent,
}

/// Why an [`ExtensionResolver`] could not load a namespace it serves. The
/// host never records the message; it fails the session start with
/// `unavailable`, and the embedder reports the message itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionResolveError {
    /// What failed, for the embedder's own report.
    pub message: String,
}

impl std::fmt::Display for ExtensionResolveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ExtensionResolveError {}

/// Loads the extension namespaces a host does not bind, when a session's
/// document uses them (`machine-capabilities.load-extensions`, called once per
/// session before data binding). The embedder supplies it
/// ([`ExtensionComponents::with_resolver`]); the host performs no acquisition
/// of its own. A resolver owns its I/O, bounds, and trust model: it must
/// bound its time and bytes and authenticate what it returns.
///
/// It runs on the session thread and blocks the session start. Whatever it
/// returns is bound for the rest of the host's lifetime, so a later session
/// (an invoked child, for example) finds the namespace bound and does not ask
/// again.
pub trait ExtensionResolver: std::fmt::Debug + Send + Sync {
    /// Resolves `namespace_uri`: `Ok(None)` when this resolver does not serve
    /// it (the session start then fails closed with `unavailable`).
    ///
    /// # Errors
    ///
    /// [`ExtensionResolveError`] when it serves the URI but cannot load it
    /// (acquisition, bound, or authentication failure); the session start
    /// fails closed.
    fn resolve(
        &self,
        namespace_uri: &str,
    ) -> Result<Option<ResolvedExtension>, ExtensionResolveError>;
}

/// One bound namespace: its package and extension component.
#[derive(Clone, Debug)]
pub(crate) struct Binding {
    pub(crate) package: ExtensionPackage,
    pub(crate) component: AuthenticatedComponent,
}

/// Extension components one session host links, keyed by exact namespace URI.
#[derive(Clone, Debug)]
pub struct ExtensionComponents {
    bindings: Vec<Binding>,
    call_fuel: u64,
    resolver: Option<Arc<dyn ExtensionResolver>>,
}

impl Default for ExtensionComponents {
    fn default() -> Self {
        Self::new()
    }
}

/// What [`ExtensionComponents`] hands the session host.
pub(crate) struct ExtensionParts {
    pub(crate) bindings: Vec<Binding>,
    pub(crate) call_fuel: u64,
    pub(crate) resolver: Option<Arc<dyn ExtensionResolver>>,
}

impl ExtensionComponents {
    /// No bindings, no resolver, and
    /// [`DEFAULT_EXTENSION_CALL_FUEL`] per extension component call.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bindings: Vec::new(),
            call_fuel: DEFAULT_EXTENSION_CALL_FUEL,
            resolver: None,
        }
    }

    /// Loads every namespace a session's document uses that no [`Self::bind`]
    /// binds through `resolver` (see [`ExtensionResolver`]). Without one (the
    /// default) such a document fails its session start closed.
    #[must_use]
    pub fn with_resolver(mut self, resolver: Arc<dyn ExtensionResolver>) -> Self {
        self.resolver = Some(resolver);
        self
    }

    /// Binds `component` to `package.namespace_uri`.
    ///
    /// The URI is compared byte-for-byte with each element's namespace (no
    /// prefix, case, or trailing-slash rewriting). The component must be a
    /// release namespace artifact ([`ArtifactKind::ExtensionExample`]) or a
    /// caller component
    /// ([`ArtifactKind::ExtensionComponent`]); its world and `namespace-uri`
    /// are checked when a session instantiates it.
    ///
    /// # Errors
    ///
    /// [`HostError::Artifact`] for another artifact kind
    /// ([`ArtifactError::ArtifactKindMismatch`]), an absolute-URI violation,
    /// the SCXML or XML namespace, a URI over [`MAX_NAMESPACE_URI_BYTES`], a
    /// URI bound twice, more than [`MAX_EXTENSION_COMPONENTS`] bindings, or a
    /// schema over [`MAX_NAMESPACE_SCHEMA_BYTES`]
    /// ([`ArtifactError::NamespaceBinding`]).
    pub fn bind(
        mut self,
        package: ExtensionPackage,
        component: &AuthenticatedComponent,
    ) -> Result<Self, HostError> {
        check_binding(
            &package,
            component,
            self.bindings
                .iter()
                .map(|bound| bound.package.namespace_uri.as_str()),
        )?;
        self.bindings.push(Binding {
            package,
            component: component.clone(),
        });
        Ok(self)
    }

    /// Sets the instruction fuel cap of one extension component call.
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

    /// Whether no namespace is bound.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    pub(crate) fn into_parts(self) -> ExtensionParts {
        ExtensionParts {
            bindings: self.bindings,
            call_fuel: self.call_fuel,
            resolver: self.resolver,
        }
    }
}

/// The checks every binding passes, whether [`ExtensionComponents::bind`] or a
/// resolver made it: an extension artifact kind, an absolute URI within
/// [`MAX_NAMESPACE_URI_BYTES`] that is neither the SCXML nor the XML
/// namespace, a schema within [`MAX_NAMESPACE_SCHEMA_BYTES`], a URI not in
/// `bound`, and fewer than [`MAX_EXTENSION_COMPONENTS`] bindings.
///
/// Extension components get no WASI interface and no outbound HTTP, by rule:
/// an extension element is executable content, run synchronously inside a
/// microstep, and anything asynchronous, I/O-bound, or long-running belongs
/// in an invoked service, the only component I/O is linked into.
pub(crate) fn check_binding<'a>(
    package: &ExtensionPackage,
    component: &AuthenticatedComponent,
    bound: impl ExactSizeIterator<Item = &'a str>,
) -> Result<(), HostError> {
    let namespace_uri = package.namespace_uri.as_str();
    if !matches!(
        component.kind(),
        ArtifactKind::ExtensionExample | ArtifactKind::ExtensionComponent
    ) {
        return Err(ArtifactError::ArtifactKindMismatch.into());
    }
    let count = bound.len();
    let mut bound = bound;
    if namespace_uri.len() > MAX_NAMESPACE_URI_BYTES
        || scxml_types::NamespaceUri::new(namespace_uri).is_err()
        || matches!(namespace_uri, SCXML_NAMESPACE | XML_NAMESPACE)
        || package.schema.len() > MAX_NAMESPACE_SCHEMA_BYTES
        || count >= MAX_EXTENSION_COMPONENTS
        || bound.any(|uri| uri == namespace_uri)
    {
        return Err(ArtifactError::NamespaceBinding.into());
    }
    Ok(())
}

/// Everything a session needs to instantiate its extension components.
pub(crate) struct ExtensionRuntime<'a> {
    pub(crate) engine: &'a wasmtime::Engine,
    pub(crate) linker: &'a Linker<ExtensionStoreData>,
    pub(crate) config: &'a WasmtimeHostConfig,
    pub(crate) providers: &'a TelemetryProviders,
    pub(crate) call_budget: &'a Arc<CallBudget>,
    pub(crate) call_fuel: u64,
}

/// The linker every extension component store uses: exactly the
/// `executable-content-extension` world's imports (no WASI interface and no
/// outbound HTTP: a component importing either fails to instantiate).
pub(crate) fn extension_linker(
    engine: &wasmtime::Engine,
) -> Result<Linker<ExtensionStoreData>, HostError> {
    let mut linker = Linker::<ExtensionStoreData>::new(engine);
    extension_bindings::ExecutableContentExtension::add_to_linker::<_, HasSelf<ExtensionStoreData>>(
        &mut linker,
        |state| state,
    )?;
    Ok(linker)
}

/// Lifecycle of one extension component instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Lifecycle {
    /// `initialize` succeeded; `execute` calls are allowed.
    Live,
    /// A call trapped: the instance cannot be entered again.
    Retired,
    /// `shutdown` was called.
    ShutDown,
}

struct ExtensionInstance {
    namespace_uri: String,
    /// Whether the component imports `session-snapshot-host`.
    reads_snapshot: bool,
    store: wasmtime::Store<ExtensionStoreData>,
    guest: extension_bindings::ExecutableContentExtension,
    lifecycle: Lifecycle,
    call_fuel: u64,
}

/// The executing session's state, lent to the extension component store for one
/// `execute` call.
pub(crate) struct Lent {
    pub(crate) data_model: Option<Box<dyn MachineDataModel>>,
    pub(crate) links: Option<Box<SessionLinks>>,
    pub(crate) configuration: Vec<String>,
    /// The machine's executable-content image, for an extension component that
    /// reads the session snapshot.
    pub(crate) image: Option<crate::session_host::ExtensionImage>,
}

/// What one `execute` call hands back to the machine store.
pub(crate) struct Executed {
    pub(crate) outcome: Result<ExtensionOutcome, ComponentError>,
    pub(crate) internal: Vec<WitEvent>,
    pub(crate) external: Vec<WitEvent>,
}

/// One `execute` call in progress inside an extension component store. The
/// session's lent state is not store data (it is the session thread's):
/// it waits in [`LENT`] for the call.
struct ExecutionCall {
    event: Option<scxml_types::Event>,
    internal: Vec<WitEvent>,
    external: Vec<WitEvent>,
    /// `session-snapshot-host.snapshot` calls made so far.
    snapshots: u32,
}

std::thread_local! {
    /// The session state lent to the `execute` call running on this thread,
    /// innermost last.
    static LENT: std::cell::RefCell<Vec<Lent>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Runs `body` with the session state lent to the `execute` call running on
/// this thread.
///
/// # Errors
///
/// `unavailable` outside an `execute` call (or re-entered from inside one),
/// and `body`'s error.
fn with_lent<T>(
    body: impl FnOnce(&mut Lent) -> Result<T, ComponentError>,
) -> Result<T, ComponentError> {
    let unavailable = || {
        component_error(
            ErrorCode::Unavailable,
            "session capabilities are available only during execution",
        )
    };
    LENT.with(|lent| {
        let mut stack = lent.try_borrow_mut().map_err(|_| unavailable())?;
        let lent = stack.last_mut().ok_or_else(unavailable)?;
        body(lent)
    })
}

/// Store data of one extension component instance.
pub(crate) struct ExtensionStoreData {
    limits: MeteredLimits,
    providers: TelemetryProviders,
    execution_context: Context,
    guest_spans: crate::guest_spans::GuestSpanTable,
    /// Present only during `execute`.
    call: Option<ExecutionCall>,
    _store_lease: LifecycleLease,
}

/// Every extension component instance of one session, in binding order.
pub(crate) struct SessionNamespaces {
    instances: Vec<ExtensionInstance>,
}

impl SessionNamespaces {
    /// No instance yet. Extension components already initialized when a later
    /// [`Self::add`] fails are shut down when the partial set drops.
    pub(crate) const fn empty() -> Self {
        Self {
            instances: Vec::new(),
        }
    }

    /// Whether this session has an instance for exactly `namespace_uri`.
    pub(crate) fn contains(&self, namespace_uri: &str) -> bool {
        self.instances
            .iter()
            .any(|instance| instance.namespace_uri == namespace_uri)
    }

    /// Instantiates one extension component for this session with its
    /// namespace's grants (`runtime.grants`), checks its `namespace-uri`, and
    /// initializes it (one `extension-instantiate` span).
    ///
    /// # Errors
    ///
    /// [`HostError::Component`] with `extension-component-uri-mismatch` or
    /// `extension-component-initialize-failed`, or a Wasmtime instantiation
    /// failure (for example a component that imports anything else).
    pub(crate) fn add(
        &mut self,
        namespace_uri: &str,
        component: &wasmtime::component::Component,
        runtime: &ExtensionRuntime<'_>,
        execution_context: &Context,
    ) -> Result<(), HostError> {
        let span = scxml_telemetry::observe(
            Module::HostWasmtime,
            Operation::HostWasmtimeExtensionInstantiate,
        );
        match ExtensionInstance::instantiate(namespace_uri, component, runtime, execution_context) {
            Ok(instance) => {
                span.event(SignalEvent::ArtifactAccepted);
                self.instances.push(instance);
                Ok(())
            }
            Err(error) => {
                span.event(SignalEvent::InputRejected);
                span.fail(ErrorCategory::Platform);
                Err(error)
            }
        }
    }

    /// Whether the extension component bound to `namespace_uri` imports
    /// `session-snapshot-host` (the machine then passes its executable-content
    /// image).
    pub(crate) fn reads_snapshot(&self, namespace_uri: &str) -> bool {
        self.instances
            .iter()
            .any(|instance| instance.namespace_uri == namespace_uri && instance.reads_snapshot)
    }

    /// Executes `element` through the extension component bound to its exact
    /// namespace URI, lending it `lent` for the call; `lent` is always
    /// returned.
    pub(crate) fn execute(
        &mut self,
        element: &ExecutableContentExtension,
        event: Option<scxml_types::Event>,
        context: Option<&WitTraceContext>,
        lent: Lent,
    ) -> (Result<Executed, ComponentError>, Lent) {
        let Some(instance) = self.instances.iter_mut().find(|instance| {
            element.name.namespace_uri.as_deref() == Some(instance.namespace_uri.as_str())
        }) else {
            return (
                Err(component_error(
                    ErrorCode::Unavailable,
                    "no extension component is bound to this namespace",
                )),
                lent,
            );
        };
        let span = scxml_telemetry::observe(
            Module::HostWasmtime,
            Operation::HostWasmtimeExtensionExecute,
        );
        let (executed, lent) = instance.execute(element, event, context, lent);
        match &executed {
            Ok(Executed {
                outcome: Ok(ExtensionOutcome::Handled),
                ..
            }) => span.event(SignalEvent::TransformationCompleted),
            Ok(Executed {
                outcome: Ok(ExtensionOutcome::NotHandled),
                ..
            }) => span.event(SignalEvent::InputRejected),
            Ok(Executed {
                outcome: Err(_), ..
            }) => {
                span.event(SignalEvent::InputRejected);
                span.fail(ErrorCategory::Execution);
            }
            Err(_) => {
                span.event(SignalEvent::InputRejected);
                span.fail(ErrorCategory::Platform);
            }
        }
        (executed, lent)
    }
}

impl Drop for SessionNamespaces {
    /// Shuts every live extension component down: the session ended (finished,
    /// failed, or cancelled). A failure is recorded on its span only; the
    /// session's outcome is already decided.
    fn drop(&mut self) {
        for instance in &mut self.instances {
            instance.shutdown();
        }
    }
}

impl ExtensionInstance {
    fn instantiate(
        namespace_uri: &str,
        component: &wasmtime::component::Component,
        runtime: &ExtensionRuntime<'_>,
        execution_context: &Context,
    ) -> Result<Self, HostError> {
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
            ExtensionStoreData {
                limits: MeteredLimits::new(limits, Arc::clone(runtime.call_budget)),
                providers: runtime.providers.clone(),
                execution_context: execution_context.clone(),
                guest_spans: crate::guest_spans::GuestSpanTable::new(),
                call: None,
                _store_lease: runtime.providers.acquire_store()?,
            },
        );
        store.limiter(|state| &mut state.limits);
        runtime.call_budget.share(&mut store);
        store.set_fuel(runtime.call_fuel)?;
        let guest = crate::service::block_on(
            extension_bindings::ExecutableContentExtension::instantiate_async(
                &mut store,
                component,
                runtime.linker,
            ),
        )?;
        let api = guest.stateforward_scxml_executable_content_extension_api();
        store.set_fuel(runtime.call_fuel)?;
        if crate::service::block_on(api.call_namespace_uri(&mut store))? != namespace_uri {
            return Err(HostError::Component(
                "extension-component-uri-mismatch".into(),
            ));
        }
        let reads_snapshot = component
            .component_type()
            .imports(runtime.engine)
            .any(|(name, _)| name == SESSION_SNAPSHOT_HOST_IMPORT);
        let mut instance = Self {
            namespace_uri: namespace_uri.to_owned(),
            reads_snapshot,
            store,
            guest,
            // Live for `initialize` itself, so a failed `initialize` is still
            // shut down below (idempotent by contract); a trap retires it.
            lifecycle: Lifecycle::Live,
            call_fuel: runtime.call_fuel,
        };
        instance.store.set_fuel(instance.call_fuel)?;
        let initialized = crate::service::block_on(
            instance
                .guest
                .stateforward_scxml_executable_content_extension_api()
                .call_initialize(&mut instance.store),
        );
        match initialized {
            Ok(Ok(())) => Ok(instance),
            Ok(Err(_)) => {
                // `shutdown` is idempotent by contract and runs on every
                // ending, including a failed start.
                instance.shutdown();
                Err(HostError::Component(
                    "extension-component-initialize-failed".into(),
                ))
            }
            Err(error) => {
                instance.lifecycle = Lifecycle::Retired;
                Err(error.into())
            }
        }
    }

    fn execute(
        &mut self,
        element: &ExecutableContentExtension,
        event: Option<scxml_types::Event>,
        context: Option<&WitTraceContext>,
        lent: Lent,
    ) -> (Result<Executed, ComponentError>, Lent) {
        if self.lifecycle != Lifecycle::Live {
            return (
                Ok(Executed {
                    outcome: Err(component_error(
                        ErrorCode::Execution,
                        "extension component was retired after a trap",
                    )),
                    internal: Vec::new(),
                    external: Vec::new(),
                }),
                lent,
            );
        }
        self.store.data_mut().call = Some(ExecutionCall {
            event,
            internal: Vec::new(),
            external: Vec::new(),
            snapshots: 0,
        });
        LENT.with_borrow_mut(|stack| stack.push(lent));
        let result = self.store.set_fuel(self.call_fuel).and_then(|()| {
            crate::service::block_on(
                self.guest
                    .stateforward_scxml_executable_content_extension_api()
                    .call_execute(&mut self.store, element, context),
            )
        });
        // The session's state always returns to the machine store, whatever
        // the extension component did.
        let lent = LENT.with_borrow_mut(Vec::pop).unwrap_or_else(|| Lent {
            data_model: None,
            links: None,
            configuration: Vec::new(),
            image: None,
        });
        let Some(call) = self.store.data_mut().call.take() else {
            // Unreachable by construction (only this function sets and takes
            // the call); fail closed without losing the session's state.
            return (
                Err(component_error(
                    ErrorCode::Platform,
                    "extension component execution state was lost",
                )),
                lent,
            );
        };
        let outcome = match result {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(error)) => Err(sanitize(&error)),
            // A trap (including fuel exhaustion) is an extension component failure: SCXML
            // raises `error.execution` and the session continues without this
            // instance.
            Err(_) => {
                self.lifecycle = Lifecycle::Retired;
                Err(component_error(
                    ErrorCode::Execution,
                    "extension component trapped or exhausted its fuel",
                ))
            }
        };
        (
            Ok(Executed {
                outcome,
                internal: call.internal,
                external: call.external,
            }),
            lent,
        )
    }

    fn shutdown(&mut self) {
        if self.lifecycle != Lifecycle::Live {
            return;
        }
        self.lifecycle = Lifecycle::ShutDown;
        let span = scxml_telemetry::observe(
            Module::HostWasmtime,
            Operation::HostWasmtimeExtensionShutdown,
        );
        let result = self.store.set_fuel(self.call_fuel).and_then(|()| {
            crate::service::block_on(
                self.guest
                    .stateforward_scxml_executable_content_extension_api()
                    .call_shutdown(&mut self.store),
            )
        });
        if matches!(result, Ok(Ok(()))) {
            span.event(SignalEvent::TransformationCompleted);
        } else {
            span.event(SignalEvent::InputRejected);
            span.fail(ErrorCategory::Execution);
        }
    }
}

/// Keeps the extension component contract's recoverable codes and the fatal
/// ones an extension component may legitimately report; any other code or a
/// malformed error is a contract violation (`protocol`). Details are replaced
/// by static text: guest details never reach the session.
fn sanitize(error: &ComponentError) -> ComponentError {
    if error.version != ErrorVersion::V1 {
        return component_error(
            ErrorCode::Protocol,
            "extension component broke the extension component error contract",
        );
    }
    match error.code {
        ErrorCode::InvalidInput => component_error(
            ErrorCode::InvalidInput,
            "extension component rejected its input",
        ),
        ErrorCode::Execution => {
            component_error(ErrorCode::Execution, "extension component execution failed")
        }
        ErrorCode::Unavailable => component_error(
            ErrorCode::Unavailable,
            "extension component reported a capability unavailable",
        ),
        ErrorCode::CapacityExhausted => component_error(
            ErrorCode::CapacityExhausted,
            "extension component exhausted a capacity bound",
        ),
        ErrorCode::PayloadTooLarge => component_error(
            ErrorCode::PayloadTooLarge,
            "extension component exceeded a byte bound",
        ),
        ErrorCode::Platform => component_error(
            ErrorCode::Platform,
            "extension component reported a platform failure",
        ),
        ErrorCode::DuplicateId
        | ErrorCode::UnknownId
        | ErrorCode::UnsupportedVersion
        | ErrorCode::Protocol => component_error(
            ErrorCode::Protocol,
            "extension component broke the extension component error contract",
        ),
    }
}

impl ExtensionStoreData {
    fn call(&mut self) -> Result<&mut ExecutionCall, ComponentError> {
        self.call.as_mut().ok_or_else(|| {
            component_error(
                ErrorCode::Unavailable,
                "session capabilities are available only during execution",
            )
        })
    }

    fn queue(&mut self, event: WitEvent, queue: EventQueue) -> Result<(), ComponentError> {
        let call = self.call()?;
        match queue {
            EventQueue::Internal => call.internal.push(event),
            EventQueue::External => call.external.push(event),
        }
        Ok(())
    }
}

/// The session queue an extension component's event joins (SCXML 4.1, 6.2).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EventQueue {
    /// The internal event queue.
    Internal,
    /// The external event queue.
    External,
}

/// `event` as a validated internal event: its name, data, send id, and
/// carrier; no processor provenance.
fn internal_event(event: WitEvent, send_id: Option<String>) -> Result<WitEvent, ComponentError> {
    let event = WitEvent {
        name: event.name,
        event_type: WitEventType::Internal,
        send_id,
        origin: None,
        origin_type: None,
        invoke_id: None,
        data: event.data,
        raw: None,
        trace_context: event.trace_context,
    };
    native_event(event.clone())?;
    Ok(event)
}

impl crate::bindings::stateforward::scxml::types::Host for ExtensionStoreData {}

impl data_model_host::Host for ExtensionStoreData {
    fn evaluate(&mut self, expression: String) -> Result<WitDataValue, ComponentError> {
        let event = self.call()?.event.clone();
        with_lent(|lent| {
            let model = lent.data_model.as_deref_mut().ok_or_else(|| {
                component_error(ErrorCode::Unavailable, "no data model is linked")
            })?;
            model
                .evaluate_value(&expression, event.as_ref())
                .map(wit_value)
                .map_err(failure_code)
        })
    }

    fn assign(&mut self, location: String, value: WitDataValue) -> Result<(), ComponentError> {
        let value = native_value(value)?;
        let event = self.call()?.event.clone();
        with_lent(|lent| {
            let model = lent.data_model.as_deref_mut().ok_or_else(|| {
                component_error(ErrorCode::Unavailable, "no data model is linked")
            })?;
            model
                .assign(
                    &location,
                    &scxml_types::AssignSource::Value(value),
                    &scxml_types::AssignPlacement::ReplaceChildren,
                    event.as_ref(),
                )
                .map_err(failure_code)
        })
    }
}

impl execution_host::Host for ExtensionStoreData {
    fn raise(&mut self, event: WitEvent) -> Result<(), ComponentError> {
        self.call()?;
        let event = internal_event(event, None)?;
        self.queue(event, EventQueue::Internal)
    }

    fn send(&mut self, request: SendRequest) -> Result<(), ComponentError> {
        self.call()?;
        // Only the SCXML Event I/O Processor's sends route here exactly as
        // `<send>` routes them; an extension component sends through no other
        // processor.
        if request.processor_type.as_deref().is_some_and(|value| {
            scxml_types::classify_processor_type(value) != scxml_types::ProcessorKind::Scxml
        }) {
            return Err(component_error(
                ErrorCode::InvalidInput,
                "an extension component sends only through the SCXML Event I/O Processor",
            ));
        }
        let target = with_lent(|lent| {
            let links = lent.links.as_deref_mut().ok_or_else(|| {
                component_error(ErrorCode::Unavailable, "no I/O processor is linked")
            })?;
            Ok(links.classify_target(request.target.as_deref()))
        })?;
        match target {
            SendTarget::Internal => {
                // As `<send>` (SCXML 6.2.1: 'delay' "Must not occur ...
                // when the attribute 'target' has the value \"_internal\"",
                // so the engine raises error.execution): an internal send is
                // never delayed.
                if request.delay_ms > 0 {
                    return Err(component_error(
                        ErrorCode::InvalidInput,
                        "a send to #_internal cannot be delayed",
                    ));
                }
                // The WIT `send-request` precondition the SCXML Event I/O
                // Processor checks for every other target.
                if request
                    .event
                    .send_id
                    .as_ref()
                    .is_some_and(|event_send_id| request.send_id.as_ref() != Some(event_send_id))
                {
                    return Err(component_error(
                        ErrorCode::InvalidInput,
                        "the event's send id differs from the request's",
                    ));
                }
                let event = internal_event(request.event, request.send_id)?;
                self.queue(event, EventQueue::Internal)
            }
            SendTarget::Invalid => Err(component_error(
                ErrorCode::InvalidInput,
                "send target is not a valid target",
            )),
            SendTarget::Unreachable => Err(component_error(
                ErrorCode::Unavailable,
                "no live session answers the send target",
            )),
            SendTarget::Local | SendTarget::Parent | SendTarget::Invocation(_) => {
                let disposition = with_lent(|lent| {
                    let links = lent.links.as_deref_mut().ok_or_else(|| {
                        component_error(ErrorCode::Unavailable, "no I/O processor is linked")
                    })?;
                    links.send(&request)
                })?;
                match disposition {
                    crate::bindings::stateforward::scxml::machine_capability_types::SendDisposition::EnqueueExternal(event) => {
                        self.queue(event, EventQueue::External)
                    }
                    crate::bindings::stateforward::scxml::machine_capability_types::SendDisposition::Routed
                    | crate::bindings::stateforward::scxml::machine_capability_types::SendDisposition::Scheduled => Ok(()),
                    crate::bindings::stateforward::scxml::machine_capability_types::SendDisposition::Undeliverable => {
                        Err(component_error(
                            ErrorCode::Unavailable,
                            "no live session answers the send target",
                        ))
                    }
                }
            }
        }
    }

    fn cancel(&mut self, send_id: String) -> Result<(), ComponentError> {
        self.call()?;
        with_lent(|lent| {
            let links = lent.links.as_deref_mut().ok_or_else(|| {
                component_error(ErrorCode::Unavailable, "no I/O processor is linked")
            })?;
            links.cancel(&send_id, None)
        })
    }

    fn log(&mut self, level: LogLevel, _message: String) {
        // The message is extension component payload and never enters
        // telemetry; only the closed level is recorded.
        let level = match level {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        };
        self.execution_context.span().add_event(
            "extension-log",
            vec![KeyValue::new("scxml.log-level", level)],
        );
    }

    fn in_state(&mut self, state_id: String) -> bool {
        self.call.is_some()
            && with_lent(|lent| Ok(lent.configuration.contains(&state_id))).unwrap_or(false)
    }

    fn now_ms(&mut self) -> u64 {
        if self.call.is_none() {
            return 0;
        }
        with_lent(|lent| {
            Ok(lent
                .links
                .as_deref()
                .map_or(0, crate::session_host::SessionLinks::now_ms))
        })
        .unwrap_or(0)
    }
}

impl session_snapshot_host::Host for ExtensionStoreData {
    // Read-only: assembles the canonical document from the machine's
    // executable-content image and the lent session state; nothing is changed,
    // and the document is extension component payload that never enters
    // telemetry.
    fn snapshot(
        &mut self,
        exclude: Vec<session_snapshot_host::Section>,
    ) -> Result<String, ComponentError> {
        let providers = self.providers.clone();
        let call = self.call()?;
        if call.snapshots >= MAX_EXTENSION_SNAPSHOTS {
            return Err(component_error(
                ErrorCode::CapacityExhausted,
                "an `execute` call takes at most 64 session snapshots",
            ));
        }
        call.snapshots = call.snapshots.saturating_add(1);
        let span =
            scxml_telemetry::observe(Module::HostWasmtime, Operation::HostWasmtimeSessionSnapshot);
        let exclude: Vec<scxml_snapshot::Section> = exclude
            .into_iter()
            .map(|section| match section {
                session_snapshot_host::Section::System => scxml_snapshot::Section::System,
                session_snapshot_host::Section::DataModel => scxml_snapshot::Section::DataModel,
                session_snapshot_host::Section::Queues => scxml_snapshot::Section::Queues,
                session_snapshot_host::Section::PendingSends => {
                    scxml_snapshot::Section::PendingSends
                }
                session_snapshot_host::Section::Transitions => scxml_snapshot::Section::Transitions,
            })
            .collect();
        let internal = &call.internal;
        let external = &call.external;
        let result = with_lent(|lent| {
            let (Some(image), Some(links)) = (lent.image.as_ref(), lent.links.as_deref_mut())
            else {
                return Err(component_error(
                    ErrorCode::Unavailable,
                    "the session snapshot is not available for this `execute` call",
                ));
            };
            Ok(crate::session_host::extension_document(
                image,
                &exclude,
                crate::session_host::ExtensionState {
                    data_model: lent.data_model.as_mut(),
                    links,
                    internal,
                    external,
                },
            ))
        });
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                span.event(SignalEvent::InputRejected);
                span.fail(ErrorCategory::Unsupported);
                return Err(error);
            }
        };
        match result {
            Ok(bytes) => {
                span.event(SignalEvent::ArtifactProduced);
                providers.record_component_measurement(
                    crate::ComponentMeasurement::SnapshotBytes,
                    u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                );
                // The codec writes UTF-8 JSON; a failed conversion is a
                // host invariant violation.
                String::from_utf8(bytes).map_err(|_| {
                    component_error(ErrorCode::Platform, "the session snapshot is not UTF-8")
                })
            }
            Err(failure) => {
                let (category, error) = extension_snapshot_error(&failure);
                span.event(SignalEvent::InputRejected);
                span.fail(category);
                Err(error)
            }
        }
    }
}

/// The extension-facing error of one failed executable-content snapshot, with
/// its closed span category; details are static.
fn extension_snapshot_error(
    failure: &crate::session_host::RunFailure,
) -> (ErrorCategory, ComponentError) {
    use crate::session_host::{RunFailure, SnapshotFailure};
    use scxml_snapshot::SnapshotErrorCode;
    match failure {
        RunFailure::Snapshot(SnapshotFailure::Codec(error))
            if matches!(
                error.code,
                SnapshotErrorCode::BoundsExceeded
                    | SnapshotErrorCode::TooLarge
                    | SnapshotErrorCode::DepthExceeded
            ) =>
        {
            (
                ErrorCategory::Validation,
                component_error(
                    ErrorCode::PayloadTooLarge,
                    "the session snapshot exceeds a bound of its format",
                ),
            )
        }
        RunFailure::Snapshot(SnapshotFailure::ServiceInvocation) => (
            ErrorCategory::Unsupported,
            component_error(
                ErrorCode::Unavailable,
                "a session holding an invoked service cannot be snapshotted",
            ),
        ),
        RunFailure::Snapshot(_) => (
            ErrorCategory::Execution,
            component_error(
                ErrorCode::Execution,
                "the session snapshot could not be read",
            ),
        ),
        _ => (
            ErrorCategory::Platform,
            component_error(ErrorCode::Platform, "the host failed to read the session"),
        ),
    }
}

impl telemetry::Host for ExtensionStoreData {
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
            "executable-content-extension",
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
            "executable-content-extension",
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
            "executable-content-extension",
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
