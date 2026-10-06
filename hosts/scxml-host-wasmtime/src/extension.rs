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
//! * **Values and references** — the embedder holds the values of the
//!   variables a run grants ([`ExtensionComponents::with_values`]). An
//!   extension component never receives one: when an element names a variable
//!   for one of its package's `env-slots` (`env:<slot>="NAME"` or
//!   `env:<slot>expr`, [`scxml_types::ENV_NAMESPACE_URI`]), the host hands the
//!   component an opaque reference for that slot, valid only during that
//!   `execute` call, and substitutes the value only inside the call's
//!   `outbound-http` requests. Each value is granted to exact package URIs
//!   ([`EnvVariable`]); a name with no value, or whose value is not granted
//!   to the element's namespace, leaves the slot absent
//!   ([`ExtensionObserver::value_missing`]). An element's references are its
//!   own: another element, of the same or another namespace, never receives
//!   them. Extension components import no WASI interface at all: this host
//!   links none into them, so one that imports any fails to instantiate.
//! * **Outbound HTTP** — `stateforward:scxml/outbound-http` is deny by default:
//!   without an embedder-supplied transport
//!   ([`ExtensionComponents::with_outbound_http`]) every request is refused
//!   with `unavailable`. With one, this module substitutes the request's
//!   references, checks it against the WIT bounds, canonicalizes its URL, and
//!   admits it only to an origin or URL prefix the element's package declares
//!   (`http-origins`: binding a package consents to them) or to the origin a
//!   reference's value supplies entirely (the run supplied it, for example a
//!   local model service's base URL); every other request is refused
//!   ([`ExtensionObserver::request_refused`]) before the transport sees it,
//!   so a value is never sent anywhere else. The call blocks the `execute`
//!   call; the transport's timeouts bound it (fuel meters instructions, not
//!   wall time).
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
//! instantiate, execute, shutdown, `extension-http-fetch`, and
//! `session-snapshot` (one per extension component snapshot) boundary spans
//! carry closed labels only (the span name, the closed signal event, and the
//! closed error category); namespace URIs, element names, attributes, variable
//! names or values, URLs, headers, bodies, statuses, and log text never become
//! attributes. Guest telemetry imports are recorded like script components'
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
    SendTarget, SlotReference, TraceContext as WitTraceContext,
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
    // `outbound-http.fetch` is an `async func` (WASI 0.3): the host
    // implements it concurrently and drives every extension component call
    // through the async ABI to completion before it returns, so `execute`
    // stays synchronous executable content.
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

pub use extension_bindings::stateforward::scxml::outbound_http;
use extension_bindings::stateforward::scxml::{
    data_model_host, execution_host, session_snapshot_host,
};

/// Default instruction fuel cap of one extension component call (`initialize`,
/// `execute`, or `shutdown`); every call also runs on the session host's
/// per-call budget.
///
/// Rationale for one billion: an element may link a whole validator, and
/// one call then compiles the bundled W3C SCXML 1.0 XSD set before it
/// validates anything. Measured with nemobot's `<sx:validate>` (the scxml.rs
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
/// Maximum bytes of one environment variable name.
pub const MAX_ENVIRONMENT_NAME_BYTES: usize = 1_024;
/// Maximum bytes of one environment variable value.
pub const MAX_ENVIRONMENT_VALUE_BYTES: usize = 65_536;
/// Maximum bytes of one extension component HTTP request URL (`outbound-http`).
pub const MAX_EXTENSION_HTTP_URL_BYTES: usize = 16_384;
/// Maximum header fields of one extension component HTTP request.
pub const MAX_EXTENSION_HTTP_HEADERS: usize = 64;
/// Maximum bytes of one extension component HTTP header name.
pub const MAX_EXTENSION_HTTP_HEADER_NAME_BYTES: usize = 256;
/// Maximum bytes of one extension component HTTP header value.
pub const MAX_EXTENSION_HTTP_HEADER_VALUE_BYTES: usize = 8_192;
/// Maximum bytes of one extension component HTTP request body (network egress;
/// every build).
pub const MAX_EXTENSION_HTTP_REQUEST_BODY_BYTES: usize = 4 * 1024 * 1024;
/// Maximum bytes of one extension component HTTP response body (network
/// ingress; every build).
pub const MAX_EXTENSION_HTTP_RESPONSE_BODY_BYTES: usize = 16 * 1024 * 1024;
/// Maximum `session-snapshot-host.snapshot` calls of one `execute` call. Each
/// call costs the host work linear in the session's state for a few guest
/// instructions, so the count is bounded on every build (an extension component
/// cannot amplify its fuel into unmetered host work).
pub const MAX_EXTENSION_SNAPSHOTS: u32 = 64;
/// The import name of `session-snapshot-host`: an extension component whose
/// component imports it reads the machine's executable-content image.
const SESSION_SNAPSHOT_HOST_IMPORT: &str = "stateforward:scxml/session-snapshot-host@0.2.0";

/// Header names an extension component may not set: the transport owns them.
const TRANSPORT_HEADERS: [&str; 4] = ["host", "content-length", "transfer-encoding", "connection"];

/// The method of one extension component HTTP request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtensionHttpMethod {
    /// `GET` (no body).
    Get,
    /// `POST`.
    Post,
}

/// One checked extension component HTTP request: the method, the canonical
/// WHATWG serialization of an absolute `http`/`https` URL without userinfo, the
/// header fields within the `outbound-http` bounds, and the body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionHttpRequest {
    /// The method.
    pub method: ExtensionHttpMethod,
    /// The canonical target URL.
    pub url: String,
    /// Header fields, in order.
    pub headers: Vec<(String, String)>,
    /// The body (empty for `GET`).
    pub body: Vec<u8>,
}

/// One extension component HTTP response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionHttpResponse {
    /// The final status (100..=599).
    pub status: u16,
    /// The body, at most [`MAX_EXTENSION_HTTP_RESPONSE_BODY_BYTES`].
    pub body: Vec<u8>,
}

/// Why a [`ExtensionHttp`] transport answered no response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtensionHttpFailure {
    /// The transport does not admit the target (nothing was sent).
    NotAllowed,
    /// Connecting, TLS, sending, or receiving failed or timed out.
    Transport,
    /// The response body exceeds [`MAX_EXTENSION_HTTP_RESPONSE_BODY_BYTES`].
    ResponseTooLarge,
}

/// The outbound HTTP transport an embedder grants to extension components
/// (`stateforward:scxml/outbound-http`). The host checks every request
/// against the WIT bounds before calling [`ExtensionHttp::fetch`], which runs
/// on the session thread and blocks the `execute` call: the implementation must
/// bound its own time and the response body, follow no redirect, and add
/// no ambient credentials. With the `http` feature, `ExtensionHttpClient`
/// is the allowlisted implementation.
pub trait ExtensionHttp: std::fmt::Debug + Send + Sync {
    /// Performs one request.
    ///
    /// # Errors
    ///
    /// [`ExtensionHttpFailure`] when no response is answered.
    fn fetch(
        &self,
        request: &ExtensionHttpRequest,
    ) -> Result<ExtensionHttpResponse, ExtensionHttpFailure>;
}

/// The exact SCXML namespace, which no extension component may claim.
const SCXML_NAMESPACE: &str = "http://www.w3.org/2005/07/scxml";
/// The built-in XML namespace, which no extension component may claim.
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

/// One variable a run grants: its name, its value, and the exact package URIs
/// (extension namespace URIs or invoked-service type URIs) whose elements or
/// `<invoke>`s may reference it. A slot reference to the variable from any
/// other package is absent, exactly like a variable with no value. Its `Debug`
/// output never prints the value.
#[derive(Clone, Eq, PartialEq)]
pub struct EnvVariable {
    name: String,
    value: String,
    packages: Vec<String>,
}

impl std::fmt::Debug for EnvVariable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EnvVariable")
            .field("name", &self.name)
            .field("packages", &self.packages)
            .finish_non_exhaustive()
    }
}

impl EnvVariable {
    /// `name` holding `value`, granted to exactly `packages` (sorted and
    /// checked by [`ExtensionEnvironment::new`]).
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        value: impl Into<String>,
        packages: impl IntoIterator<Item = String>,
    ) -> Self {
        let mut packages: Vec<String> = packages.into_iter().collect();
        packages.sort();
        Self {
            name: name.into(),
            value: value.into(),
            packages,
        }
    }

    /// The variable's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The exact package URIs granted the variable, sorted.
    #[must_use]
    pub fn packages(&self) -> &[String] {
        &self.packages
    }

    fn is_valid(&self) -> bool {
        !self.name.is_empty()
            && self.name.len() <= MAX_ENVIRONMENT_NAME_BYTES
            && !self.name.contains(['=', '\0'])
            && self.value.len() <= MAX_ENVIRONMENT_VALUE_BYTES
            && !self.value.contains('\0')
            && !self.packages.is_empty()
            && self.packages.windows(2).all(|pair| pair[0] != pair[1])
            && self.packages.iter().all(|uri| {
                uri.len() <= MAX_NAMESPACE_URI_BYTES && scxml_types::NamespaceUri::new(uri).is_ok()
            })
    }
}

/// The values of the variables a run grants extension elements' and invoked
/// services' `env` slots, each scoped to the package URIs it is granted to:
/// exactly what the embedder passes, never the host process environment.
/// Components never receive them, only references
/// ([`ExtensionComponents::with_values`], [`crate::ServiceComponents::with_values`]),
/// and only for a variable granted to the referencing element's or
/// `<invoke>`'s package. [`ExtensionEnvironment::none`] (the default) holds
/// nothing, so every slot is absent. Its `Debug` output names the variables
/// and their packages only, never a value, so formatting it (or a type that
/// holds it, such as [`ExtensionComponents`] or [`crate::ServiceComponents`])
/// never prints a secret.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct ExtensionEnvironment {
    variables: Vec<EnvVariable>,
}

impl std::fmt::Debug for ExtensionEnvironment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExtensionEnvironment")
            .field("variables", &self.variables)
            .finish_non_exhaustive()
    }
}

impl ExtensionEnvironment {
    /// No variables.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Exactly `variables`, sorted by name.
    ///
    /// # Errors
    ///
    /// [`HostError::InvalidLimit`] ([`InvalidLimit::Environment`]) for a
    /// duplicate name, an empty name or one containing `=` or NUL, a name over
    /// [`MAX_ENVIRONMENT_NAME_BYTES`], a value over
    /// [`MAX_ENVIRONMENT_VALUE_BYTES`] or containing NUL, or a variable
    /// granted to no package, to one package twice, or to a package URI that
    /// is not an absolute URI within [`MAX_NAMESPACE_URI_BYTES`].
    pub fn new(variables: impl IntoIterator<Item = EnvVariable>) -> Result<Self, HostError> {
        let mut variables: Vec<EnvVariable> = variables.into_iter().collect();
        variables.sort_by(|left, right| left.name.cmp(&right.name));
        let invalid = variables
            .windows(2)
            .any(|pair| pair[0].name == pair[1].name)
            || !variables.iter().all(EnvVariable::is_valid);
        if invalid {
            return Err(HostError::InvalidLimit(InvalidLimit::Environment));
        }
        Ok(Self { variables })
    }

    /// The variables, sorted by name.
    #[must_use]
    pub fn variables(&self) -> &[EnvVariable] {
        &self.variables
    }

    /// The value held for `name` when it is granted to `package` (exact
    /// URI); `None` when it is unheld or granted only to other packages.
    pub(crate) fn held(&self, package: &str, name: &str) -> Option<&str> {
        self.variables
            .binary_search_by(|held| held.name.as_str().cmp(name))
            .ok()
            .and_then(|index| self.variables.get(index))
            .filter(|held| {
                held.packages
                    .binary_search_by(|granted| granted.as_str().cmp(package))
                    .is_ok()
            })
            .map(|held| held.value.as_str())
    }
}

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
    /// The `env` slots its elements accept (`env:<slot>` attributes); the host
    /// binds references only for these.
    pub env_slots: Vec<String>,
    /// The origins or URL prefixes its extension component's requests may
    /// reach (canonicalized like an [`crate::HttpAllowlist`]); binding the
    /// package consents to them.
    pub http_origins: Vec<String>,
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

/// What a host reports to its embedder about one execution, for the
/// embedder's own diagnostics (never telemetry). Both calls carry what the
/// embedder supplied or the document named: no value is ever passed.
pub trait ExtensionObserver: std::fmt::Debug + Send + Sync {
    /// A request of `namespace_uri`'s extension component to `origin` was
    /// refused: its package does not declare it and no reference's value
    /// supplied it.
    fn request_refused(&self, namespace_uri: &str, origin: &str);
    /// An element of `namespace_uri` named the variable `name` for `slot`,
    /// and no value is held for it, so the slot is absent.
    fn value_missing(&self, namespace_uri: &str, slot: &str, name: &str);
}

/// Extension components one session host links, keyed by exact namespace URI.
#[derive(Clone, Debug)]
pub struct ExtensionComponents {
    bindings: Vec<Binding>,
    values: ExtensionEnvironment,
    http: Option<Arc<dyn ExtensionHttp>>,
    observer: Option<Arc<dyn ExtensionObserver>>,
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
    pub(crate) values: Arc<ExtensionEnvironment>,
    pub(crate) http: Option<Arc<dyn ExtensionHttp>>,
    pub(crate) observer: Option<Arc<dyn ExtensionObserver>>,
    pub(crate) call_fuel: u64,
    pub(crate) resolver: Option<Arc<dyn ExtensionResolver>>,
}

impl ExtensionComponents {
    /// No bindings, no values, no outbound HTTP, no resolver, and
    /// [`DEFAULT_EXTENSION_CALL_FUEL`] per extension component call.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bindings: Vec::new(),
            values: ExtensionEnvironment::none(),
            http: None,
            observer: None,
            call_fuel: DEFAULT_EXTENSION_CALL_FUEL,
            resolver: None,
        }
    }

    /// The values elements' `env` slots may reference (the run's granted
    /// variables). Extension components receive references only.
    #[must_use]
    pub fn with_values(mut self, values: ExtensionEnvironment) -> Self {
        self.values = values;
        self
    }

    /// The transport admitted `outbound-http` requests go through. Without one
    /// (the default) every request is refused with `unavailable`. The host
    /// admits each request first (the element's declared origins, or a
    /// reference-supplied origin), so the transport sends what it is given.
    #[must_use]
    pub fn with_outbound_http(mut self, transport: Arc<dyn ExtensionHttp>) -> Self {
        self.http = Some(transport);
        self
    }

    /// The embedder's [`ExtensionObserver`].
    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn ExtensionObserver>) -> Self {
        self.observer = Some(observer);
        self
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
            values: Arc::new(self.values),
            http: self.http,
            observer: self.observer,
            call_fuel: self.call_fuel,
            resolver: self.resolver,
        }
    }
}

/// The checks every binding passes, whether [`ExtensionComponents::bind`] or a
/// resolver made it: an extension artifact kind, an absolute URI within
/// [`MAX_NAMESPACE_URI_BYTES`] that is neither the SCXML nor the XML
/// namespace, a schema within [`MAX_NAMESPACE_SCHEMA_BYTES`], at most
/// [`MAX_DECLARED_ITEMS`] valid `env-slots` and `http-origins` (the latter
/// canonicalized like an [`crate::HttpAllowlist`]), a URI not in `bound`,
/// and fewer than [`MAX_EXTENSION_COMPONENTS`] bindings.
///
/// Extension components get no WASI interface, by rule: an extension element
/// is executable content, run synchronously inside a microstep, and
/// anything asynchronous, I/O-bound, or long-running belongs in an invoked
/// service, the only component WASI is linked into.
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
        || package.env_slots.len() > MAX_DECLARED_ITEMS
        || !package.env_slots.iter().all(|slot| is_env_slot(slot))
        || package.http_origins.len() > MAX_DECLARED_ITEMS
        || crate::HttpAllowlist::new(&package.http_origins).is_err()
        || count >= MAX_EXTENSION_COMPONENTS
        || bound.any(|uri| uri == namespace_uri)
    {
        return Err(ArtifactError::NamespaceBinding.into());
    }
    Ok(())
}

/// Maximum entries of an [`ExtensionPackage`]'s `env_slots` and of its
/// `http_origins` (the manifest bound).
pub const MAX_DECLARED_ITEMS: usize = 32;
/// Maximum bytes of one `env` slot name (the manifest bound).
const MAX_ENV_SLOT_BYTES: usize = 64;
/// Maximum parts of one `outbound-http` URL, header value, or body.
pub const MAX_EXTENSION_HTTP_PARTS: usize = 64;
/// The suffix of an `env:<slot>expr` attribute.
const EXPR_SUFFIX: &str = "expr";

/// An `env-slots` entry: `[a-z][a-z0-9-]*`, at most 64 bytes, never ending in
/// `expr`.
pub(crate) fn is_env_slot(slot: &str) -> bool {
    slot.len() <= MAX_ENV_SLOT_BYTES
        && !slot.ends_with(EXPR_SUFFIX)
        && slot
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && slot
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// A variable name an `env` slot may name: `[A-Za-z_][A-Za-z0-9_]*`.
pub(crate) fn is_env_name(name: &str) -> bool {
    name.len() <= MAX_ENVIRONMENT_NAME_BYTES
        && name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// Everything a session needs to instantiate its extension components.
pub(crate) struct ExtensionRuntime<'a> {
    pub(crate) engine: &'a wasmtime::Engine,
    pub(crate) linker: &'a Linker<ExtensionStoreData>,
    pub(crate) config: &'a WasmtimeHostConfig,
    pub(crate) providers: &'a TelemetryProviders,
    pub(crate) call_budget: &'a Arc<CallBudget>,
    /// The package of the namespace being instantiated.
    pub(crate) package: &'a ExtensionPackage,
    /// The run's values `env` slots may reference.
    pub(crate) values: &'a Arc<ExtensionEnvironment>,
    pub(crate) http: Option<&'a Arc<dyn ExtensionHttp>>,
    pub(crate) observer: Option<&'a Arc<dyn ExtensionObserver>>,
    pub(crate) call_fuel: u64,
}

/// The linker every extension component store uses: exactly the
/// `executable-content-extension` world's imports (no WASI interface; a
/// package declaring one is refused by [`check_binding`]).
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
    /// The slots its package declares.
    env_slots: Vec<String>,
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
    /// The values this call's references stand for, by reference.
    references: Vec<(u32, String)>,
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
    /// The namespace this instance implements.
    namespace_uri: String,
    /// The origins and URL prefixes its package declares.
    allow: crate::HttpAllowlist,
    /// The run's values `env` slots may reference.
    values: Arc<ExtensionEnvironment>,
    /// The outbound HTTP transport, if any.
    http: Option<Arc<dyn ExtensionHttp>>,
    observer: Option<Arc<dyn ExtensionObserver>>,
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
                namespace_uri: namespace_uri.to_owned(),
                // Validated when the package was bound.
                allow: crate::HttpAllowlist::new(&runtime.package.http_origins)
                    .map_err(|_| HostError::from(ArtifactError::NamespaceBinding))?,
                values: Arc::clone(runtime.values),
                http: runtime.http.map(Arc::clone),
                observer: runtime.observer.map(Arc::clone),
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
            env_slots: runtime.package.env_slots.clone(),
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
        let mut lent = lent;
        let bound = {
            let data = self.store.data();
            bind_references(
                element,
                &self.namespace_uri,
                &self.env_slots,
                &data.values,
                &mut lent,
                event.as_ref(),
            )
        };
        let (slots, references) = match bound {
            Ok(bound) => bound,
            Err(failure) => {
                return (
                    Ok(Executed {
                        outcome: Err(failure),
                        internal: Vec::new(),
                        external: Vec::new(),
                    }),
                    lent,
                );
            }
        };
        if let Some(observer) = self.store.data().observer.as_deref() {
            for missing in element_names_without_value(
                element,
                &self.namespace_uri,
                &self.env_slots,
                &self.store.data().values,
            ) {
                observer.value_missing(&self.namespace_uri, &missing.0, &missing.1);
            }
        }
        self.store.data_mut().call = Some(ExecutionCall {
            event,
            internal: Vec::new(),
            external: Vec::new(),
            snapshots: 0,
            references,
        });
        LENT.with_borrow_mut(|stack| stack.push(lent));
        let result = self.store.set_fuel(self.call_fuel).and_then(|()| {
            crate::service::block_on(
                self.guest
                    .stateforward_scxml_executable_content_extension_api()
                    .call_execute(&mut self.store, element, context, &slots),
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

/// Whether `name` is an RFC 9110 token within its byte bound.
fn is_header_name(name: &str) -> bool {
    (1..=MAX_EXTENSION_HTTP_HEADER_NAME_BYTES).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

/// One `outbound-http` request with every reference substituted, and the
/// origin a reference's value supplied entirely, if any.
#[derive(Debug)]
pub(crate) struct Substituted {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    supplied_origin: Option<url::Origin>,
}

/// Substitutes every reference of `request` with the value this call holds
/// for it. The URL's origin is *supplied* when its first part is a reference
/// whose value is an absolute `http`/`https` URL without userinfo and the
/// rest of the URL, if any, begins its path, query, or fragment (`/`, `?`,
/// `#`): the run, not the document or the component, chose that origin.
///
/// # Errors
///
/// `invalid-input` for more than [`MAX_EXTENSION_HTTP_PARTS`] parts in a
/// field, or a reference the call did not receive.
pub(crate) fn substitute(
    request: outbound_http::HttpRequest,
    references: &[(u32, String)],
) -> Result<Substituted, ComponentError> {
    let invalid = |detail| component_error(ErrorCode::InvalidInput, detail);
    let value = |reference: u32| {
        references
            .iter()
            .find(|(held, _)| *held == reference)
            .map(|(_, value)| value.as_str())
            .ok_or_else(|| invalid("the request names a reference this execution did not receive"))
    };
    let text = |parts: &[outbound_http::TextPart]| -> Result<String, ComponentError> {
        if parts.len() > MAX_EXTENSION_HTTP_PARTS {
            return Err(invalid("a request field has too many parts"));
        }
        let mut text = String::new();
        for part in parts {
            match part {
                outbound_http::TextPart::Literal(literal) => text.push_str(literal),
                outbound_http::TextPart::Reference(reference) => text.push_str(value(*reference)?),
            }
        }
        Ok(text)
    };
    let url = text(&request.url)?;
    let supplied_origin = match request.url.first() {
        Some(outbound_http::TextPart::Reference(reference)) => {
            let supplied = value(*reference)?;
            let rest = url.get(supplied.len()..).unwrap_or_default();
            crate::allowlist::parse_http_url(supplied)
                .filter(|_| rest.is_empty() || rest.starts_with(['/', '?', '#']))
                .map(|supplied| supplied.origin())
        }
        _ => None,
    };
    let mut headers = Vec::with_capacity(request.headers.len().min(MAX_EXTENSION_HTTP_HEADERS + 1));
    for header in &request.headers {
        headers.push((header.name.clone(), text(&header.value)?));
    }
    if request.body.len() > MAX_EXTENSION_HTTP_PARTS {
        return Err(invalid("a request field has too many parts"));
    }
    let mut body = Vec::new();
    for part in request.body {
        match part {
            outbound_http::BodyPart::Bytes(bytes) => body.extend_from_slice(&bytes),
            outbound_http::BodyPart::Reference(reference) => {
                body.extend_from_slice(value(reference)?.as_bytes());
            }
        }
    }
    Ok(Substituted {
        method: request.method,
        url,
        headers,
        body,
        supplied_origin,
    })
}

/// Whether the canonical `url` may be requested: its package declares it
/// (`allow`), or a reference's value supplied its whole origin.
pub(crate) fn admitted(
    url: &str,
    allow: &crate::HttpAllowlist,
    supplied_origin: Option<&url::Origin>,
) -> bool {
    crate::allowlist::parse_http_url(url).is_some_and(|url| {
        allow.admits_canonical(&url)
            || supplied_origin.is_some_and(|origin| url.origin() == *origin)
    })
}

/// One execution's slot references and the values they stand for.
type BoundReferences = (Vec<SlotReference>, Vec<(u32, String)>);

/// The slot references of one execution: for each `env` attribute of
/// `element` naming one of `env_slots`, the variable it names (`env:<slot>`)
/// or its expression evaluates to (`env:<slot>expr`), bound to a fresh
/// reference when `values` holds it for `namespace_uri`, in attribute order. References are
/// numbered from 1 for every call, so one never outlives it.
///
/// # Errors
///
/// `invalid-input` for an attribute naming an undeclared slot, or naming one
/// slot twice; `execution` for an expression that fails or does not evaluate
/// to a variable name.
fn bind_references(
    element: &ExecutableContentExtension,
    namespace_uri: &str,
    env_slots: &[String],
    values: &ExtensionEnvironment,
    lent: &mut Lent,
    event: Option<&scxml_types::Event>,
) -> Result<BoundReferences, ComponentError> {
    let mut slots: Vec<SlotReference> = Vec::new();
    let mut references = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for attribute in &element.attributes {
        if attribute.name.namespace_uri.as_deref() != Some(scxml_types::ENV_NAMESPACE_URI) {
            continue;
        }
        let (slot, expr) = env_slot_of(&attribute.name.local_name, env_slots).ok_or_else(|| {
            component_error(
                ErrorCode::InvalidInput,
                "an env attribute names a slot the element's package does not declare",
            )
        })?;
        if seen.contains(&slot) {
            return Err(component_error(
                ErrorCode::InvalidInput,
                "an env attribute names one slot twice",
            ));
        }
        seen.push(slot.clone());
        let name = if expr {
            let model = lent.data_model.as_deref_mut().ok_or_else(|| {
                component_error(
                    ErrorCode::Execution,
                    "no data model evaluates an env slot expression",
                )
            })?;
            match model.evaluate_value(&attribute.value, event) {
                Ok(scxml_types::DataValue::String(name)) => name,
                _ => {
                    return Err(component_error(
                        ErrorCode::Execution,
                        "an env slot expression does not evaluate to a variable name",
                    ));
                }
            }
        } else {
            attribute.value.clone()
        };
        if !is_env_name(&name) {
            return Err(component_error(
                ErrorCode::Execution,
                "an env slot names no valid variable",
            ));
        }
        let Some(value) = values.held(namespace_uri, &name) else {
            continue;
        };
        let reference = u32::try_from(references.len().saturating_add(1))
            .map_err(|_| component_error(ErrorCode::InvalidInput, "too many env slots"))?;
        references.push((reference, value.to_owned()));
        slots.push(SlotReference { slot, reference });
    }
    Ok((slots, references))
}

/// `(slot, name)` of every literal `env:<slot>="NAME"` attribute of `element`
/// whose variable has no value granted to `namespace_uri` (expression names are reported nowhere: they
/// are evaluated per execution).
fn element_names_without_value(
    element: &ExecutableContentExtension,
    namespace_uri: &str,
    env_slots: &[String],
    values: &ExtensionEnvironment,
) -> Vec<(String, String)> {
    element
        .attributes
        .iter()
        .filter(|attribute| {
            attribute.name.namespace_uri.as_deref() == Some(scxml_types::ENV_NAMESPACE_URI)
        })
        .filter_map(|attribute| {
            let (slot, expr) = env_slot_of(&attribute.name.local_name, env_slots)?;
            (!expr && values.held(namespace_uri, &attribute.value).is_none())
                .then(|| (slot, attribute.value.clone()))
        })
        .collect()
}

/// The slot an `env` attribute's local name binds and whether it is the
/// expression form, against the declared `env_slots`.
fn env_slot_of(local_name: &str, env_slots: &[String]) -> Option<(String, bool)> {
    if env_slots.iter().any(|slot| slot == local_name) {
        return Some((local_name.to_owned(), false));
    }
    local_name
        .strip_suffix(EXPR_SUFFIX)
        .filter(|slot| env_slots.iter().any(|declared| declared == slot))
        .map(|slot| (slot.to_owned(), true))
}

/// Checks one substituted `outbound-http` request against its WIT bounds and
/// canonicalizes its URL (the WHATWG serialization that is admitted and
/// requested).
///
/// # Errors
///
/// `invalid-input` for a request outside the bounds.
pub(crate) fn checked_http_request(
    request: Substituted,
) -> Result<ExtensionHttpRequest, ComponentError> {
    let invalid = |detail| Err(component_error(ErrorCode::InvalidInput, detail));
    let method = match request.method.as_str() {
        "GET" => ExtensionHttpMethod::Get,
        "POST" => ExtensionHttpMethod::Post,
        _ => return invalid("the request method is not GET or POST"),
    };
    if request.url.len() > MAX_EXTENSION_HTTP_URL_BYTES {
        return invalid("the request URL exceeds its byte bound");
    }
    let url = match url::Url::parse(&request.url) {
        Ok(url)
            if matches!(url.scheme(), "http" | "https")
                && url.host().is_some()
                && url.username().is_empty()
                && url.password().is_none() =>
        {
            String::from(url)
        }
        _ => return invalid("the request URL is not an absolute http or https URL"),
    };
    if request.headers.len() > MAX_EXTENSION_HTTP_HEADERS {
        return invalid("the request has too many header fields");
    }
    let mut headers = Vec::with_capacity(request.headers.len());
    for header in request.headers {
        if !is_header_name(&header.0)
            || TRANSPORT_HEADERS
                .iter()
                .any(|owned| header.0.eq_ignore_ascii_case(owned))
        {
            return invalid("a request header name is invalid or owned by the transport");
        }
        if header.1.len() > MAX_EXTENSION_HTTP_HEADER_VALUE_BYTES
            || header.1.contains(['\r', '\n', '\0'])
        {
            return invalid("a request header value is invalid");
        }
        headers.push(header);
    }
    if request.body.len() > MAX_EXTENSION_HTTP_REQUEST_BODY_BYTES
        || (method == ExtensionHttpMethod::Get && !request.body.is_empty())
    {
        return invalid("the request body exceeds its bound or accompanies GET");
    }
    Ok(ExtensionHttpRequest {
        method,
        url,
        headers,
        body: request.body,
    })
}

impl ExtensionStoreData {
    fn fetch_granted(
        &mut self,
        request: outbound_http::HttpRequest,
    ) -> Result<outbound_http::HttpResponse, (ComponentError, ErrorCategory)> {
        let references = self
            .call
            .as_ref()
            .map(|call| call.references.as_slice())
            .unwrap_or_default();
        let observer = self.observer.as_deref();
        let namespace_uri = self.namespace_uri.as_str();
        fetch_admitted(
            request,
            references,
            &self.allow,
            self.http.as_deref(),
            |origin| {
                if let Some(observer) = observer {
                    observer.request_refused(namespace_uri, origin);
                }
            },
        )
    }
}

/// Performs one `outbound-http` request for a granted caller (an extension
/// component during `execute`, or an invoked service during an export
/// call): [`admit_request`], then sends it through `transport` and checks
/// the response ([`settle_response`]). The error carries the span's closed
/// category.
pub(crate) fn fetch_admitted(
    request: outbound_http::HttpRequest,
    references: &[(u32, String)],
    allow: &crate::HttpAllowlist,
    transport: Option<&dyn ExtensionHttp>,
    refused: impl FnOnce(&str),
) -> Result<outbound_http::HttpResponse, (ComponentError, ErrorCategory)> {
    let request = admit_request(request, references, allow, transport.is_some(), refused)?;
    let Some(transport) = transport else {
        return Err(no_transport());
    };
    settle_response(transport.fetch(&request))
}

/// The refusal of a caller the host grants no transport.
pub(crate) fn no_transport() -> (ComponentError, ErrorCategory) {
    (
        component_error(
            ErrorCode::Unavailable,
            "the host offers this component no outbound HTTP",
        ),
        ErrorCategory::Unsupported,
    )
}

/// Admits one `outbound-http` request before anything is sent: substitutes
/// `references`, checks the WIT bounds, refuses it when the caller has no
/// transport (`has_transport`), canonicalizes the URL, and admits it only to
/// `allow` or an origin a reference's value supplied entirely (else calls
/// `refused` with the origin and refuses it). Returns the canonical request
/// a transport sends exactly.
pub(crate) fn admit_request(
    request: outbound_http::HttpRequest,
    references: &[(u32, String)],
    allow: &crate::HttpAllowlist,
    has_transport: bool,
    refused: impl FnOnce(&str),
) -> Result<ExtensionHttpRequest, (ComponentError, ErrorCategory)> {
    let substituted =
        substitute(request, references).map_err(|error| (error, ErrorCategory::InvalidInput))?;
    let supplied_origin = substituted.supplied_origin.clone();
    let request =
        checked_http_request(substituted).map_err(|error| (error, ErrorCategory::InvalidInput))?;
    if !has_transport {
        return Err(no_transport());
    }
    if !admitted(&request.url, allow, supplied_origin.as_ref()) {
        if let Some(url) = crate::allowlist::parse_http_url(&request.url) {
            refused(&url.origin().ascii_serialization());
        }
        return Err((
            component_error(
                ErrorCode::Unavailable,
                "the request target is neither declared by the package nor supplied by the run",
            ),
            ErrorCategory::InvalidInput,
        ));
    }
    Ok(request)
}

/// What one admitted request's transport outcome becomes for the caller: a
/// response within the status range and the response-body bound, or the
/// `outbound-http` failure (`unavailable` for a refusal or transport
/// failure, `payload-too-large` for an oversized body).
pub(crate) fn settle_response(
    outcome: Result<ExtensionHttpResponse, ExtensionHttpFailure>,
) -> Result<outbound_http::HttpResponse, (ComponentError, ErrorCategory)> {
    let response = outcome.map_err(|failure| match failure {
        ExtensionHttpFailure::NotAllowed => (
            component_error(
                ErrorCode::Unavailable,
                "the transport refused the request target",
            ),
            ErrorCategory::InvalidInput,
        ),
        ExtensionHttpFailure::Transport => (
            component_error(ErrorCode::Unavailable, "the HTTP transport failed"),
            ErrorCategory::Platform,
        ),
        ExtensionHttpFailure::ResponseTooLarge => (
            component_error(
                ErrorCode::PayloadTooLarge,
                "the response body exceeds its byte bound",
            ),
            ErrorCategory::Platform,
        ),
    })?;
    if !(100..=599).contains(&response.status) {
        return Err((
            component_error(ErrorCode::Unavailable, "the HTTP transport failed"),
            ErrorCategory::Platform,
        ));
    }
    if response.body.len() > MAX_EXTENSION_HTTP_RESPONSE_BODY_BYTES {
        return Err((
            component_error(
                ErrorCode::PayloadTooLarge,
                "the response body exceeds its byte bound",
            ),
            ErrorCategory::Platform,
        ));
    }
    Ok(outbound_http::HttpResponse {
        status: response.status,
        body: response.body,
    })
}

impl outbound_http::Host for ExtensionStoreData {}

/// An extension component lowers `fetch` synchronously (its `execute` is
/// synchronous executable content): the host performs the admitted request
/// before it answers, blocking `execute`.
#[allow(
    clippy::unused_async_trait_impl,
    reason = "the WIT `fetch` is an `async func`; an extension's request completes before it returns"
)]
impl<U> outbound_http::HostWithStore<U> for wasmtime::component::HasSelf<ExtensionStoreData> {
    async fn fetch(
        accessor: &wasmtime::component::Accessor<U, Self>,
        request: outbound_http::HttpRequest,
    ) -> Result<outbound_http::HttpResponse, ComponentError> {
        accessor.with(|mut access| {
            let data = access.get();
            data.call()?;
            let span = scxml_telemetry::observe(
                Module::HostWasmtime,
                Operation::HostWasmtimeExtensionHttpFetch,
            );
            match data.fetch_granted(request) {
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
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_names_variables_and_never_prints_a_value() {
        const VALUE: &str = "sk-debug-must-not-print";
        let values =
            ExtensionEnvironment::new([EnvVariable::new("API_TOKEN", VALUE, ["urn:x".to_owned()])])
                .expect("a valid variable");
        let environment = format!("{values:?}");
        assert!(environment.contains("API_TOKEN"), "{environment}");
        for text in [
            environment,
            format!(
                "{:?}",
                ExtensionComponents::new().with_values(values.clone())
            ),
            format!(
                "{:?}",
                crate::ServiceComponents::new().with_values(values.clone())
            ),
        ] {
            assert!(!text.contains(VALUE), "a value was formatted: {text}");
        }
    }

    fn http_request(
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> outbound_http::HttpRequest {
        outbound_http::HttpRequest {
            method: method.into(),
            url: vec![outbound_http::TextPart::Literal(url.into())],
            headers: headers
                .iter()
                .map(|(name, value)| outbound_http::HttpHeader {
                    name: (*name).into(),
                    value: vec![outbound_http::TextPart::Literal((*value).into())],
                })
                .collect(),
            body: vec![outbound_http::BodyPart::Bytes(body.to_vec())],
        }
    }

    #[allow(
        clippy::needless_pass_by_value,
        reason = "mirrors the host's owned request"
    )]
    fn checked(
        request: outbound_http::HttpRequest,
    ) -> Result<ExtensionHttpRequest, ComponentError> {
        substitute(request, &[]).and_then(checked_http_request)
    }

    fn rejected(request: outbound_http::HttpRequest) -> Option<String> {
        let error = checked(request).err()?;
        assert_eq!(error.code, ErrorCode::InvalidInput);
        error.detail
    }

    /// A request whose URL is `parts` (`Ok` a literal, `Err` a reference)
    /// and whose one header carries reference 1.
    fn parts(url: &[Result<&str, u32>]) -> outbound_http::HttpRequest {
        let part = |part: &Result<&str, u32>| match part {
            Ok(literal) => outbound_http::TextPart::Literal((*literal).into()),
            Err(reference) => outbound_http::TextPart::Reference(*reference),
        };
        outbound_http::HttpRequest {
            method: "POST".into(),
            url: url.iter().map(part).collect(),
            headers: vec![outbound_http::HttpHeader {
                name: "authorization".into(),
                value: vec![
                    outbound_http::TextPart::Literal("Bearer ".into()),
                    outbound_http::TextPart::Reference(1),
                ],
            }],
            body: vec![
                outbound_http::BodyPart::Bytes(b"{\"k\":\"".to_vec()),
                outbound_http::BodyPart::Reference(1),
                outbound_http::BodyPart::Bytes(b"\"}".to_vec()),
            ],
        }
    }

    /// Whether the request `url` (with references 1 = a secret and 2 =
    /// `base`) is admitted under `declared`, and where it would go.
    fn admission(declared: &[&str], base: &str, url: &[Result<&str, u32>]) -> Option<String> {
        let references = [(1, "sk-secret".to_owned()), (2, base.to_owned())];
        let substituted = substitute(parts(url), &references).expect("references resolve");
        let supplied = substituted.supplied_origin.clone();
        let request = checked_http_request(substituted).ok()?;
        let allow = crate::HttpAllowlist::new(declared).expect("valid declarations");
        admitted(&request.url, &allow, supplied.as_ref()).then_some(request.url)
    }

    #[test]
    fn references_are_substituted_inside_the_request_only() {
        let references = [
            (1, "sk-secret".to_owned()),
            (2, "https://api.example/v1".to_owned()),
        ];
        let substituted =
            substitute(parts(&[Err(2), Ok("/responses")]), &references).expect("resolves");
        assert_eq!(substituted.url, "https://api.example/v1/responses");
        assert_eq!(
            substituted.headers,
            [("authorization".to_owned(), "Bearer sk-secret".to_owned())]
        );
        assert_eq!(substituted.body, br#"{"k":"sk-secret"}"#);
        let unknown = substitute(parts(&[Err(3)]), &references).expect_err("an unknown reference");
        assert_eq!(unknown.code, ErrorCode::InvalidInput);
        let too_many: Vec<Result<&str, u32>> = vec![Ok("x"); MAX_EXTENSION_HTTP_PARTS + 1];
        assert!(substitute(parts(&too_many), &references).is_err());
    }

    #[test]
    fn requests_reach_only_declared_or_run_supplied_origins() {
        let declared = ["https://api.example/v1"];
        // Declared: a literal URL under a declared prefix.
        assert_eq!(
            admission(&declared, "", &[Ok("https://api.example/v1/responses")]).as_deref(),
            Some("https://api.example/v1/responses")
        );
        // Not declared, not supplied: refused, so the secret never leaves.
        assert_eq!(
            admission(&declared, "", &[Ok("https://evil.example/x")]),
            None
        );
        assert_eq!(
            admission(&declared, "", &[Ok("https://api.example/v2/x")]),
            None
        );
        // Run-supplied: the whole origin comes from a reference's value.
        for (base, rest, expected) in [
            (
                "http://127.0.0.1:11434/v1",
                "/responses",
                "http://127.0.0.1:11434/v1/responses",
            ),
            (
                "http://127.0.0.1:11434",
                "/v1/responses",
                "http://127.0.0.1:11434/v1/responses",
            ),
            ("https://local.example", "", "https://local.example/"),
            (
                "https://local.example",
                "?q=1",
                "https://local.example/?q=1",
            ),
            (
                "https://bücher.example/v1",
                "/x",
                "https://xn--bcher-kva.example/v1/x",
            ),
        ] {
            assert_eq!(
                admission(&[], base, &[Err(2), Ok(rest)]).as_deref(),
                Some(expected),
                "{base} + {rest}"
            );
        }
        // Not run-supplied: a literal extends or precedes the authority, the
        // value carries userinfo, or the value is no URL at all.
        for (base, url) in [
            ("http://127.0.0.1", vec![Err(2), Ok(":8080/x")]),
            ("https://local.example", vec![Err(2), Ok(".evil.example/x")]),
            ("https://local.example", vec![Err(2), Ok("@evil.example/x")]),
            ("local.example", vec![Ok("https://"), Err(2), Ok("/x")]),
            ("sk", vec![Ok("https://"), Err(2), Ok(".evil.example/x")]),
            ("https://user:pw@local.example", vec![Err(2), Ok("/x")]),
            ("not a url", vec![Err(2), Ok("/x")]),
            ("https://local.example", vec![Ok(""), Err(2), Ok("/x")]),
        ] {
            assert_eq!(admission(&[], base, &url), None, "{base} {url:?}");
        }
        // A secret reference as the authority is no URL: refused.
        assert_eq!(admission(&[], "", &[Err(1), Ok("/x")]), None);
    }

    #[test]
    fn extension_http_requests_are_canonicalized_within_their_bounds() {
        let accepted = checked(http_request(
            "POST",
            "HTTP://Api.Example:80/v1/./x/../responses",
            &[
                ("Authorization", "Bearer k"),
                ("content-type", "application/json"),
            ],
            b"{}",
        ))
        .expect("a request within the bounds is accepted");
        assert_eq!(
            accepted,
            ExtensionHttpRequest {
                method: ExtensionHttpMethod::Post,
                url: "http://api.example/v1/responses".into(),
                headers: vec![
                    ("Authorization".into(), "Bearer k".into()),
                    ("content-type".into(), "application/json".into()),
                ],
                body: b"{}".to_vec(),
            }
        );
        let get = checked(http_request("GET", "https://a.example/", &[], b""))
            .expect("an empty GET is accepted");
        assert_eq!(get.method, ExtensionHttpMethod::Get);
        let long_url = format!(
            "https://a.example/{}",
            "x".repeat(MAX_EXTENSION_HTTP_URL_BYTES)
        );
        let long_value = "v".repeat(MAX_EXTENSION_HTTP_HEADER_VALUE_BYTES + 1);
        let many: Vec<(&str, &str)> = vec![("x", "v"); MAX_EXTENSION_HTTP_HEADERS + 1];
        let body = vec![0; MAX_EXTENSION_HTTP_REQUEST_BODY_BYTES + 1];
        for request in [
            http_request("PUT", "https://a.example/", &[], b""),
            http_request("post", "https://a.example/", &[], b""),
            http_request("POST", &long_url, &[], b""),
            http_request("POST", "/relative", &[], b""),
            http_request("POST", "ftp://a.example/", &[], b""),
            http_request("POST", "https://user:pw@a.example/", &[], b""),
            http_request("POST", "https://user@a.example/", &[], b""),
            http_request("POST", "https://:pw@a.example/", &[], b""),
            http_request("POST", "https://a.example/", &[("Host", "b.example")], b""),
            http_request(
                "POST",
                "https://a.example/",
                &[("Content-Length", "1")],
                b"",
            ),
            http_request("POST", "https://a.example/", &[("bad name", "v")], b""),
            http_request("POST", "https://a.example/", &[("", "v")], b""),
            http_request(
                "POST",
                "https://a.example/",
                &[("x", "a\r\nInjected: 1")],
                b"",
            ),
            http_request("POST", "https://a.example/", &[("x", &long_value)], b""),
            http_request("POST", "https://a.example/", &many, b""),
            http_request("POST", "https://a.example/", &[], &body),
            http_request("GET", "https://a.example/", &[], b"x"),
        ] {
            let url = format!("{:?}", request.url);
            assert!(rejected(request).is_some(), "{url} is refused");
        }
    }

    fn variable(name: &str, value: &str, packages: &[&str]) -> EnvVariable {
        EnvVariable::new(name, value, packages.iter().map(|uri| (*uri).to_owned()))
    }

    #[test]
    fn environments_are_sorted_and_bounded() {
        let environment = ExtensionEnvironment::new([
            variable("B", "2", &["urn:y", "urn:x"]),
            variable("A", "1", &["urn:x"]),
        ])
        .expect("valid variables");
        let names: Vec<&str> = environment
            .variables()
            .iter()
            .map(EnvVariable::name)
            .collect();
        assert_eq!(names, ["A", "B"]);
        assert_eq!(
            environment.variables()[1].packages(),
            ["urn:x".to_owned(), "urn:y".to_owned()],
            "packages are sorted"
        );
        for variables in [
            vec![variable("", "v", &["urn:x"])],
            vec![variable("A=B", "v", &["urn:x"])],
            vec![variable("A", "v\0", &["urn:x"])],
            vec![
                variable("A", "1", &["urn:x"]),
                variable("A", "2", &["urn:y"]),
            ],
            vec![variable(
                "A",
                &"x".repeat(MAX_ENVIRONMENT_VALUE_BYTES + 1),
                &["urn:x"],
            )],
            vec![variable("A", "v", &[])],
            vec![variable("A", "v", &["urn:x", "urn:x"])],
            vec![variable("A", "v", &["not a uri"])],
        ] {
            assert!(matches!(
                ExtensionEnvironment::new(variables),
                Err(HostError::InvalidLimit(InvalidLimit::Environment))
            ));
        }
        assert!(ExtensionEnvironment::none().variables().is_empty());
    }

    #[test]
    fn a_value_is_held_only_for_the_packages_it_is_granted_to() {
        let environment = ExtensionEnvironment::new([
            variable("KEY", "k", &["urn:a", "urn:c"]),
            variable("OTHER", "o", &["urn:b"]),
        ])
        .expect("valid variables");
        assert_eq!(environment.held("urn:a", "KEY"), Some("k"));
        assert_eq!(environment.held("urn:c", "KEY"), Some("k"));
        assert_eq!(environment.held("urn:b", "KEY"), None, "out of scope");
        assert_eq!(environment.held("urn:a", "OTHER"), None, "out of scope");
        assert_eq!(environment.held("urn:a", "MISSING"), None, "unheld");
        assert_eq!(environment.held("urn:A", "KEY"), None, "exact URI only");
    }

    /// The host sets no bound on how many variables an environment passes
    /// (a native resource bound would refuse an inherited environment).
    #[test]
    fn an_environment_passes_any_number_of_variables() {
        let environment = ExtensionEnvironment::new(
            (0..1_000).map(|index| variable(&format!("V{index:04}"), "", &["urn:x"])),
        )
        .expect("an environment of any size is valid");
        assert_eq!(environment.variables().len(), 1_000);
    }
}
