//! Optional heavy Wasmtime Component Model host.
//!
//! This native boundary owns OpenTelemetry SDK providers, W3C propagation,
//! Wasmtime engine policy, and per-store resource limits. Guest components use
//! the `scxml-telemetry` API/WIT facade and never link an SDK or exporter.
//!
//! # Features
//!
//! - Default (no features): the Wasmtime-free surface. It provides the
//!   OpenTelemetry providers ([`TelemetryProviders`], [`TelemetryHost`],
//!   [`GuestSpan`]), the host configuration and its validation
//!   ([`WasmtimeHostConfig`], [`WasmtimeStoreConfig`], [`InvalidLimit`]), the
//!   limit constants, and the error types ([`HostError`], [`ArtifactError`]).
//!   [`wasmtime_enabled`] returns `false`. Wasmtime is not compiled.
//! - `wasmtime`: adds the Component Model host itself, which is built on the
//!   optional `wasmtime`, `scxml-scheduler`, `scxml-script`, `scxml-types`,
//!   and `sha2` dependencies. It covers engine and store construction,
//!   authenticated component artifacts, machine sessions (runtime IR and
//!   packaged), component data models, script components, extension
//!   components, invoked services, and the clock / I/O processor
//!   capability graph. [`wasmtime_enabled`]
//!   returns `true`.
//!   CI checks and tests the crate with this feature
//!   (`cargo test -p scxml-host-wasmtime --features wasmtime`).
//!
//! The feature is additive: it only adds items and the
//! `HostError::Wasmtime` and `HostError::Release` variants, and never removes or changes an item the
//! default surface provides. [`HostError`] is `#[non_exhaustive]` so that a
//! crate matching it keeps compiling when another crate in the build graph
//! enables the feature.

#![forbid(unsafe_code)]

use opentelemetry::Context;
#[cfg(feature = "wasmtime")]
use opentelemetry::metrics::MeterProvider as _;
use opentelemetry::trace::{Status, TraceContextExt as _, Tracer as _, TracerProvider as _};
#[cfg(feature = "wasmtime")]
use scxml_telemetry::RuntimeMeasurement;
use scxml_telemetry::{
    ErrorCategory, Module, Operation, SignalEvent, TraceContext, TraceContextError, WasmTraceHost,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[cfg(feature = "wasmtime")]
mod bindings {
    wasmtime::component::bindgen!({
        path: "../../crates/scxml-wit/wit/scxml",
        world: "machine",
    });
}

#[cfg(feature = "wasmtime")]
mod clock_bindings {
    wasmtime::component::bindgen!({
        path: "../../crates/scxml-wit/wit/scxml/deps/clock",
        world: "clock",
    });
}

#[cfg(feature = "wasmtime")]
mod io_bindings {
    wasmtime::component::bindgen!({
        path: "../../crates/scxml-wit/wit/scxml",
        world: "io-processor",
        with: {
            "stateforward:scxml/types": crate::bindings::stateforward::scxml::types,
            "stateforward:scxml/telemetry": crate::bindings::stateforward::scxml::telemetry,
        },
    });
}

#[cfg(feature = "wasmtime")]
mod compiler_bindings {
    wasmtime::component::bindgen!({
        path: "../../crates/scxml-wit/wit/compiler",
        world: "compiler",
    });
}

#[cfg(feature = "wasmtime")]
#[allow(
    missing_docs,
    reason = "generated WIT bindings re-exported as `basichttp_types`; the canonical documentation is basichttp.wit"
)]
mod basichttp_bindings {
    wasmtime::component::bindgen!({
        path: "../../crates/scxml-wit/wit/basichttp",
        world: "basichttp",
    });
}

/// Host-side bindings of the `session-snapshot` codec world. They exist for
/// the byte-parity proof of the two component-records-to-capture
/// conversions (`docs/session-snapshot.md`): the proof feeds the same
/// component records through the native conversion in
/// `session_host::snapshot` and through the real codec component, and
/// asserts byte-equal documents and identical failure codes. Reusing the
/// generated records via `with` is what makes the two paths consume
/// literally the same values.
#[cfg(feature = "wasmtime")]
mod codec_bindings {
    wasmtime::component::bindgen!({
        path: "../../crates/scxml-wit/wit/scxml",
        world: "session-snapshot",
        with: {
            "stateforward:scxml/types": crate::bindings::stateforward::scxml::types,
            "stateforward:scxml/telemetry": crate::bindings::stateforward::scxml::telemetry,
        },
    });
}

#[cfg(feature = "wasmtime")]
mod call_budget;
#[cfg(feature = "wasmtime")]
pub use call_budget::CallFuel;
#[cfg(feature = "wasmtime")]
mod component;
#[cfg(feature = "wasmtime")]
mod memory_growth;
#[cfg(feature = "wasmtime")]
pub use component::{
    ComponentExecution, ComponentStatus, execute_component_start, execute_machine_session,
    execute_machine_session_with_scripts,
};
#[cfg(feature = "wasmtime")]
pub use memory_growth::MEMORY_GROWTH_COPY_BYTES_PER_FUEL;
#[cfg(feature = "wasmtime")]
mod session;
#[cfg(feature = "wasmtime")]
pub use session::{
    DataModelFailure, MachineCallTrace, MachineDataModel, MachineIrSource, MachineLogRecord,
    MachineSessionTrace,
};
#[cfg(feature = "wasmtime")]
mod data_model;
#[cfg(feature = "wasmtime")]
mod guest_spans;
#[cfg(feature = "wasmtime")]
pub use data_model::{
    ComponentDataModel, DEFAULT_DATA_MODEL_CALL_FUEL, DataModelComponents, DataModelSystemValues,
    MAX_DATA_MODEL_COMPONENTS,
};
#[cfg(feature = "wasmtime")]
mod validator;
#[cfg(feature = "wasmtime")]
pub use validator::{ComponentValidator, validator_types};
#[cfg(feature = "wasmtime")]
mod package_validator;
#[cfg(feature = "wasmtime")]
pub use package_validator::PackageValidators;
#[cfg(feature = "wasmtime")]
mod script;
#[cfg(feature = "wasmtime")]
pub use script::{
    DEFAULT_SCRIPT_RUN_FUEL, MAX_SCRIPT_BUNDLE_ENTRIES, MachineScripts, ResolvedScript,
    ScriptPolicy, ScriptResolveError, ScriptResolver,
};
#[cfg(feature = "wasmtime")]
mod artifact;
#[cfg(feature = "wasmtime")]
pub use artifact::{
    ArtifactKind, AuthenticatedComponent, ComponentSource, TrustedComponentDigest,
    TrustedManifestDigest, authenticate_component, authenticate_data_model_component,
    authenticate_extension_component, authenticate_package_validator_component,
    authenticate_packaged_machine, authenticate_service_component,
};
#[cfg(feature = "wasmtime")]
mod extension;
#[cfg(feature = "wasmtime")]
pub use extension::{
    DEFAULT_EXTENSION_CALL_FUEL, ExtensionComponents, ExtensionPackage, ExtensionResolveError,
    ExtensionResolver, MAX_EXTENSION_COMPONENTS, MAX_NAMESPACE_SCHEMA_BYTES,
    MAX_NAMESPACE_URI_BYTES, ResolvedExtension,
};
#[cfg(feature = "wasmtime")]
mod outbound_http;
#[cfg(feature = "wasmtime")]
pub use outbound_http::{
    EnvVariable, ExtensionEnvironment, ExtensionHttp, ExtensionHttpFailure, ExtensionHttpMethod,
    ExtensionHttpRequest, ExtensionHttpResponse, MAX_DECLARED_ITEMS, MAX_ENVIRONMENT_NAME_BYTES,
    MAX_ENVIRONMENT_VALUE_BYTES, MAX_EXTENSION_HTTP_HEADER_NAME_BYTES,
    MAX_EXTENSION_HTTP_HEADER_VALUE_BYTES, MAX_EXTENSION_HTTP_HEADERS, MAX_EXTENSION_HTTP_PARTS,
    MAX_EXTENSION_HTTP_REQUEST_BODY_BYTES, MAX_EXTENSION_HTTP_RESPONSE_BODY_BYTES,
    MAX_EXTENSION_HTTP_URL_BYTES,
};
#[cfg(feature = "wasmtime")]
mod service;
#[cfg(feature = "wasmtime")]
pub use service::{ServiceComponents, ServicePackage};
#[cfg(feature = "wasmtime")]
mod wasi;
#[cfg(feature = "wasmtime")]
pub use wasi::WasiOptions;
#[cfg(feature = "wasmtime")]
mod basichttp;
#[cfg(feature = "wasmtime")]
pub use basichttp::{
    BasicHttpProcessor, BasicHttpTransport, TransportUnavailable, basichttp_types,
};
#[cfg(feature = "wasmtime")]
mod capability_graph;
#[cfg(feature = "wasmtime")]
mod session_host;
// The unit tests that run components read the run's shared release when the
// environment names one, as the integration tests do.
#[cfg(all(test, feature = "wasmtime"))]
#[path = "../tests/prebuilt/mod.rs"]
mod prebuilt;
#[cfg(feature = "wasmtime")]
pub use capability_graph::{
    CapabilityGraphComponents, CapabilityGraphExecution, execute_capability_graph,
};
#[cfg(feature = "wasmtime")]
pub use scxml_snapshot;
#[cfg(feature = "wasmtime")]
pub use session_host::{
    Applied, CloseReport, HttpReceipt, InputRecord, LiveCallResult, LiveClock, LiveSession,
    LiveStatus, ManualClock, SessionClock, SessionOpen, SessionOutput, WallClock,
};
#[cfg(feature = "wasmtime")]
pub use session_host::{
    CommunicationFailure, CommunicationSender, LiveHttp, MAX_PUBLIC_URL_BYTES, OutboundHttp,
    PublicUrl, PublicUrlError,
};
#[cfg(feature = "wasmtime")]
mod allowlist;
#[cfg(feature = "http")]
mod http_client;
#[cfg(feature = "wasmtime")]
pub use allowlist::{
    AllowlistError, AllowlistReason, HttpAllowlist, MAX_ALLOWLIST_ENTRIES,
    MAX_ALLOWLIST_ENTRY_BYTES, canonical_http_url,
};
#[cfg(feature = "http")]
pub use http_client::{
    DEFAULT_CONNECT_TIMEOUT, DEFAULT_MAX_IN_FLIGHT, DEFAULT_TOTAL_TIMEOUT, ExtensionHttpClient,
    ExtensionHttpConfig, HttpClientConfig, HttpClientError, MAX_IN_FLIGHT, MAX_RESPONSE_HEAD_BYTES,
    NetworkClient,
};
#[cfg(feature = "http")]
mod resolver;
#[cfg(feature = "http")]
pub use resolver::{RESOLVER_CALL_FUEL, ResolverComponent, resolver_api};
#[cfg(feature = "http")]
mod http_listener;
#[cfg(feature = "http")]
pub use http_listener::{
    ACCEPT_POLL, BasicHttpListener, ClaimedHttp, InboundHttp, ListenerConfig, ListenerHandle,
    MAX_CONCURRENT_CONNECTIONS, MAX_REQUEST_BODY_BYTES, MAX_REQUEST_HEAD_BYTES,
    MAX_REQUEST_HEADERS,
};
#[cfg(feature = "wasmtime")]
pub use session_host::{
    CheckpointSnapshot, CheckpointSnapshotFailure, SnapshotExport, SnapshotFailure, SnapshotPoints,
    SnapshotRequest,
};
#[cfg(feature = "wasmtime")]
pub use session_host::{
    DocumentKind, EmbeddedDocument, InvokeLimits, LOOPBACK_ORIGIN, LoadRejection, LogEntry,
    RunFailure, ScriptedEvent, SessionEmbedder, SessionHost, SessionHostComponents,
    SessionHostError, SessionOutcome, SessionRun, Terminal, TraceEntry, TraceObservation,
    loopback_location,
};
#[cfg(feature = "wasmtime")]
pub use session_host::{
    MAX_RELEASE_MANIFEST_BYTES, ReleaseComponents, ReleaseError, read_bounded_file,
};

/// Maximum linear-memory bytes allowed for one memory by default (64 MiB).
pub const DEFAULT_MEMORY_BYTES: usize = 64 * 1024 * 1024;
/// Maximum elements allowed for one Wasm table by default.
pub const DEFAULT_TABLE_ELEMENTS: usize = 10_000;
/// Maximum component/core instances allowed in one store by default.
pub const DEFAULT_INSTANCES: usize = 100;
/// Maximum tables allowed in one store by default.
pub const DEFAULT_TABLES: usize = 100;
/// Maximum linear memories allowed in one store by default.
pub const DEFAULT_MEMORIES: usize = 100;
/// Deterministic instruction fuel of one host call by default.
///
/// In the session host this is one budget per outermost call into any
/// component, shared by every component that call reaches: the machine and
/// every data-model, I/O processor, clock, `BasicHTTP`, compiler, script,
/// and child-machine call it makes are charged to it (see the session host
/// documentation). So the instructions one host call executes, across every
/// component, never exceed it. A session command is one host call even
/// when it defers `<invoke>` starts or cancels to the host's loop: the
/// command, every child start, cancel, and continuation carried out for it,
/// and its own continuation spend one budget. The one exception is
/// compiling a document the embedder served (an `<invoke src>` document):
/// each is compiled once per run on a budget of its own, so the documents a
/// tree invokes cost no call's fuel, and their compiles stay bounded by the
/// fixed set the embedder serves (inline `<content>` is compiled on the
/// call's budget). A nested store's own per-call cap still applies inside
/// it (for example `DEFAULT_DATA_MODEL_CALL_FUEL` per data-model call).
/// Standalone APIs refuel their one store to it per call.
///
/// Logical runaways are bounded before fuel: one machine call runs at most
/// `scxml_engine::MAX_COMMANDS_PER_RUN` (1 024) commands, so an eventless
/// transition loop, a `<raise>` loop, or a 0 ms self-send chain stops with
/// `RunFailure::CommandBound` (under 1 000 000 000 fuel for 1 024 commands
/// of a two-state loop), and the run's command budget bounds the calls.
///
/// Sizing, measured with the shipped components on an Apple M3 Max
/// (`tests/fuel_sizing.rs`, release, which prints every figure here): this
/// default bounds one runaway host call to roughly 0.22 to 0.25 s of Wasm,
/// about 70 to 85 ms per billion fuel (`default_fuel_runaway_call_time`:
/// the heavy nested `<foreach>`/`<script>` document traps inside its start
/// after 0.23 to 0.25 s, and a 128-region parallel machine whose regions
/// toggle eventlessly after 505 commands and 0.22 to 0.25 s; narrower
/// machines, 64 regions among them, reach the 1 024-command bound first).
/// The ECMAScript provider creates its guarded interpreter realm once, at
/// `initialize`, and keeps it for the whole session. The largest single
/// calls of the sizing workloads, each a document `compile` accepts
/// (`default_fuel_sizing_workloads` bisects and prints them): an ECMAScript
/// start about 0.11 billion, which also covers eight non-assignment
/// `<script>`s in one transition and the representative machine's start; a
/// `<foreach>` with an `<assign>` over 60, 90, and 1 000 items in the start
/// 0.11, 0.12, and 0.21 billion; a 64-region parallel state whose regions
/// all transition 10 million (its start and one event); and the start of an
/// ECMAScript root around a 4 000-state inline `<invoke>` child 1.73
/// billion. This admits the ECMAScript workloads with at least 14 times
/// headroom and the 4 000-state child with 1.72 times (the measurement
/// requires 1.5 times of every workload).
pub const DEFAULT_FUEL: u64 = 3_000_000_000;
/// Maximum native stack used while executing Wasm by default (512 KiB).
pub const DEFAULT_WASM_STACK_BYTES: usize = 512 * 1024;

/// Maximum authenticated component artifact size: 2 GiB − 1
/// (2,147,483,647 bytes).
///
/// Components ship as GitHub release assets, and GitHub requires "each file
/// included in a release must be under 2 GiB"
/// (<https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases>),
/// so the largest component a release can carry is the largest component
/// accepted. This pre-compilation structural ceiling is independent of
/// Wasmtime store fuel and memory limits, which apply only after
/// compilation. It is a bound, not a reservation: readers check a file's
/// metadata or a response's `Content-Length` against it before reading and
/// grow their buffers with the bytes actually read. It fits a 32-bit
/// `usize`, one past it included.
pub const MAX_AUTHENTICATED_COMPONENT_BYTES: usize = (2 << 30) - 1;
/// Maximum accepted authenticated component artifact size by default: the
/// GitHub-derived [`MAX_AUTHENTICATED_COMPONENT_BYTES`] itself, so every
/// component a release can carry is accepted unless the embedder lowers
/// [`WasmtimeHostConfig::component_bytes`].
pub const DEFAULT_COMPONENT_BYTES: usize = MAX_AUTHENTICATED_COMPONENT_BYTES;

/// Whether `len` bytes is an admissible authenticated component size: not
/// empty and at most [`MAX_AUTHENTICATED_COMPONENT_BYTES`]. The one size
/// predicate every authentication path, manifest size field, and
/// [`WasmtimeHostConfig::validate`] shares.
pub(crate) const fn component_size_admitted(len: usize) -> bool {
    len > 0 && len <= MAX_AUTHENTICATED_COMPONENT_BYTES
}

/// Typed artifact authentication failures raised before Wasmtime compilation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactError {
    /// Raw bytes were supplied without a trusted manifest authentication result.
    Unauthenticated,
    /// The trusted manifest digest was not exactly 64 lowercase hexadecimal bytes.
    InvalidTrustedDigest,
    /// The manifest exceeded its 64 KiB structural limit.
    ManifestTooLarge,
    /// The manifest bytes did not match the caller's trusted digest.
    ManifestDigestMismatch,
    /// The manifest was not canonical `stateforward.wasm-artifacts/v1` TSV.
    ManifestMalformed,
    /// A manifest path was non-canonical or escaped the manifest directory.
    NonCanonicalPath,
    /// The requested artifact was absent or duplicated.
    ArtifactIdentity,
    /// The artifact was empty or exceeded the fixed pre-compilation
    /// structural limit, [`MAX_AUTHENTICATED_COMPONENT_BYTES`].
    ArtifactTooLarge,
    /// The artifact size did not match its authenticated manifest entry.
    ArtifactSizeMismatch,
    /// The artifact digest did not match its authenticated manifest entry.
    ArtifactDigestMismatch,
    /// The authenticated artifact kind was not valid for the requested world.
    ArtifactKindMismatch,
    /// The machine's `ir-source` import did not match the IR supply mode: a
    /// runtime-mode machine must import it and a packaged machine must not.
    IrSourceModeMismatch,
    /// An extension component binding named a URI that is not an absolute URI
    /// within its byte bound, the SCXML or XML namespace, or one already
    /// bound, or exceeded the binding count.
    NamespaceBinding,
    /// The run grants a WASI resource whose interface the service
    /// component does not import, or grants to a URI no package binds.
    WasiGrantUndeclared,
    /// A WASI grant is outside its bounds, or its directory cannot be
    /// opened.
    WasiGrantInvalid,
    /// A service component imports a `wasi:*` interface the host never
    /// links (such as `wasi:cli` or `wasi:http`): Wasmtime's unknown import
    /// (`wasmtime::UnknownImportError`), refused before instantiation.
    UnknownImport,
    /// A service component imports a WASI interface whose runtime resource
    /// (a preopened directory, TCP addresses, or name lookup) the run does
    /// not grant.
    WasiResourceUngranted,
}

impl std::fmt::Display for ArtifactError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unauthenticated => {
                "the artifact bytes were not authenticated against a trusted manifest"
            }
            Self::InvalidTrustedDigest => {
                "the trusted manifest digest is not 64 lowercase hexadecimal characters"
            }
            Self::ManifestTooLarge => "the release manifest exceeds its 64 KiB bound",
            Self::ManifestDigestMismatch => {
                "the release manifest's SHA-256 is not the trusted manifest digest"
            }
            Self::ManifestMalformed => {
                "the release manifest is not canonical `stateforward.wasm-artifacts/v1` TSV"
            }
            Self::NonCanonicalPath => {
                "the artifact path is not the canonical `<name>.virtualized.wasm` file name"
            }
            Self::ArtifactIdentity => {
                "the release manifest does not list this artifact exactly once"
            }
            Self::ArtifactTooLarge => "the artifact exceeds the component byte bound",
            Self::ArtifactSizeMismatch => "the artifact's size differs from its manifest entry",
            Self::ArtifactDigestMismatch => {
                "the artifact's SHA-256 differs from its manifest entry"
            }
            Self::ArtifactKindMismatch => "the artifact kind is not valid for the requested world",
            Self::IrSourceModeMismatch => {
                "the machine's `ir-source` import does not match the IR supply mode"
            }
            Self::NamespaceBinding => {
                "an extension component must bind one distinct absolute namespace URI other than the SCXML and XML namespaces, within the binding bounds"
            }
            Self::WasiGrantUndeclared => {
                "the run grants a WASI resource no bound service component imports"
            }
            Self::WasiGrantInvalid => "a WASI grant is malformed or its directory cannot be opened",
            Self::UnknownImport => {
                "the service component imports a WASI interface this host never links"
            }
            Self::WasiResourceUngranted => {
                "the service component imports a WASI interface whose resource the run does not grant"
            }
        })
    }
}

impl std::error::Error for ArtifactError {}

/// Validated limits applied to every store created by this host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WasmtimeStoreConfig {
    /// Maximum bytes for each linear memory.
    pub memory_bytes: usize,
    /// Maximum elements for each table.
    pub table_elements: usize,
    /// Maximum instances in a store.
    pub instances: usize,
    /// Maximum tables in a store.
    pub tables: usize,
    /// Maximum memories in a store.
    pub memories: usize,
    /// Instruction fuel of one host call, shared by every component the call
    /// reaches in the session host ([`DEFAULT_FUEL`]).
    pub fuel: u64,
}

impl Default for WasmtimeStoreConfig {
    fn default() -> Self {
        Self {
            memory_bytes: DEFAULT_MEMORY_BYTES,
            table_elements: DEFAULT_TABLE_ELEMENTS,
            instances: DEFAULT_INSTANCES,
            tables: DEFAULT_TABLES,
            memories: DEFAULT_MEMORIES,
            fuel: DEFAULT_FUEL,
        }
    }
}

/// Configuration accepted before a Wasmtime engine or store is created.
///
/// Component Model support and deterministic fuel accounting are invariants
/// of every engine and store this crate builds, not options: an engine
/// without the Component Model cannot run any component, and a store without
/// fuel would admit unbounded execution. Only the bounds below are
/// configurable; [`Self::validate`] rejects each unusable value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WasmtimeHostConfig {
    /// Maximum native stack bytes used while executing Wasm.
    pub max_wasm_stack_bytes: usize,
    /// Maximum bytes accepted for one component artifact.
    pub component_bytes: usize,
    /// Per-store resource and fuel limits.
    pub store: WasmtimeStoreConfig,
}

impl Default for WasmtimeHostConfig {
    fn default() -> Self {
        Self {
            max_wasm_stack_bytes: DEFAULT_WASM_STACK_BYTES,
            component_bytes: DEFAULT_COMPONENT_BYTES,
            store: WasmtimeStoreConfig::default(),
        }
    }
}

/// Configuration fields that cannot safely be zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvalidLimit {
    /// Wasm stack byte limit.
    WasmStackBytes,
    /// Component artifact byte limit.
    ComponentBytes,
    /// Linear-memory byte limit.
    MemoryBytes,
    /// Table-element limit.
    TableElements,
    /// Instance count limit.
    Instances,
    /// Table count limit.
    Tables,
    /// Memory count limit.
    Memories,
    /// Store instruction fuel.
    Fuel,
    /// Script byte bound of a `ScriptPolicy` (with the `wasmtime` feature).
    ScriptBytes,
    /// Variables of a `ExtensionEnvironment` (with the `wasmtime` feature):
    /// name or value bounds, a duplicate or illegal name, or a variable granted
    /// to no package, to one package twice, or to an invalid package URI.
    Environment,
}

/// Typed host initialization failures.
///
/// Non-exhaustive: the `wasmtime` feature adds the `Wasmtime` variant, and
/// Cargo features must stay additive for every crate in a build graph.
#[derive(Debug)]
#[non_exhaustive]
pub enum HostError {
    /// The caller supplied an unusable zero limit.
    InvalidLimit(InvalidLimit),
    /// Execution reached, or would exceed, a host-enforced bound: the
    /// per-session event or engine command bound, a guest-span handle
    /// space, or an overflowing fuel total.
    UnboundedExecution,
    /// Component provenance or pre-compilation structure was rejected.
    Artifact(ArtifactError),
    /// Wasmtime rejected engine or store configuration.
    #[cfg(feature = "wasmtime")]
    Wasmtime(wasmtime::Error),
    /// A host-owned OpenTelemetry provider could not shut down cleanly.
    TelemetryShutdown,
    /// Stores or guest spans still depend on the providers.
    TelemetryInUse,
    /// A guest supplied malformed or over-bound distributed trace metadata.
    TraceContext(TraceContextError),
    /// The guest returned a typed component failure.
    Component(String),
    /// A release components directory was not loaded: the file, the
    /// artifact and manifest, and how to build a complete directory.
    #[cfg(feature = "wasmtime")]
    Release(session_host::ReleaseError),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLimit(limit) => write!(formatter, "invalid zero host limit: {limit:?}"),
            Self::UnboundedExecution => {
                formatter.write_str("execution exceeded a host-enforced bound")
            }
            Self::Artifact(error) => write!(formatter, "component artifact rejected: {error}"),
            #[cfg(feature = "wasmtime")]
            Self::Wasmtime(error) => write!(formatter, "Wasmtime host failure: {error}"),
            Self::TelemetryShutdown => {
                formatter.write_str("OpenTelemetry provider shutdown failed")
            }
            Self::TelemetryInUse => {
                formatter.write_str("OpenTelemetry providers still have live stores or spans")
            }
            Self::TraceContext(error) => write!(formatter, "invalid trace context: {error}"),
            Self::Component(message) => write!(formatter, "component execution failed: {message}"),
            #[cfg(feature = "wasmtime")]
            Self::Release(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for HostError {}

impl From<ArtifactError> for HostError {
    fn from(error: ArtifactError) -> Self {
        Self::Artifact(error)
    }
}

impl From<TraceContextError> for HostError {
    fn from(error: TraceContextError) -> Self {
        Self::TraceContext(error)
    }
}

#[cfg(feature = "wasmtime")]
impl From<wasmtime::Error> for HostError {
    fn from(error: wasmtime::Error) -> Self {
        Self::Wasmtime(error)
    }
}

impl WasmtimeHostConfig {
    /// Validates all limits before Wasmtime receives them.
    ///
    /// This check is deterministic, bounded, and has no external side effects.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::InvalidLimit`] naming the first zero limit, or
    /// [`HostError::Artifact`] with [`ArtifactError::ArtifactTooLarge`] when
    /// `component_bytes` exceeds [`MAX_AUTHENTICATED_COMPONENT_BYTES`].
    pub fn validate(&self) -> Result<(), HostError> {
        let values = [
            (self.max_wasm_stack_bytes, InvalidLimit::WasmStackBytes),
            (self.component_bytes, InvalidLimit::ComponentBytes),
            (self.store.memory_bytes, InvalidLimit::MemoryBytes),
            (self.store.table_elements, InvalidLimit::TableElements),
            (self.store.instances, InvalidLimit::Instances),
            (self.store.tables, InvalidLimit::Tables),
            (self.store.memories, InvalidLimit::Memories),
        ];
        if let Some(limit) = values
            .into_iter()
            .find_map(|(value, limit)| (value == 0).then_some(limit))
        {
            Err(HostError::InvalidLimit(limit))
        } else if !component_size_admitted(self.component_bytes) {
            Err(HostError::Artifact(ArtifactError::ArtifactTooLarge))
        } else if self.store.fuel == 0 {
            Err(HostError::InvalidLimit(InvalidLimit::Fuel))
        } else {
            Ok(())
        }
    }
}

/// Host-owned OpenTelemetry trace and metric providers.
///
/// Construction is side-effect free. [`Self::install_global`] explicitly
/// installs cloned provider handles in process-global API slots. The owner must
/// call [`Self::shutdown`] after all stores and guest spans are dropped.
///
/// # Sharing and mutation
///
/// Clones (and every [`TelemetryHost`] from [`Self::guest_host`]) share one
/// provider pair and one lifecycle record behind an `Arc<Mutex<_>>`. The
/// record counts the live store and guest-span leases and holds the shutdown
/// state (running, succeeded, or failed). A lease increments its count when it
/// is acquired and decrements it (saturating) when it drops. Acquisition fails
/// with [`HostError::TelemetryShutdown`] once shutdown has run. [`Self::shutdown`]
/// refuses with [`HostError::TelemetryInUse`] while any lease is live, and
/// otherwise shuts both SDK providers down exactly once. The first result is
/// then sticky. Each operation holds the lock only for that bookkeeping,
/// never across SDK export or guest calls. A poisoned lock is recovered,
/// because every update leaves the record consistent. Any thread may hold a
/// clone.
#[derive(Clone, Debug)]
pub struct TelemetryProviders {
    tracer: opentelemetry_sdk::trace::SdkTracerProvider,
    meter: opentelemetry_sdk::metrics::SdkMeterProvider,
    lifecycle: Arc<Mutex<ProviderLifecycle>>,
}

#[derive(Debug, Default)]
struct ProviderLifecycle {
    active_spans: usize,
    active_stores: usize,
    shutdown: ShutdownState,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ShutdownState {
    #[default]
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug)]
struct LifecycleLease {
    lifecycle: Arc<Mutex<ProviderLifecycle>>,
    kind: LeaseKind,
}

#[derive(Clone, Copy, Debug)]
enum LeaseKind {
    Span,
    #[cfg(feature = "wasmtime")]
    Store,
}

#[cfg(feature = "wasmtime")]
#[derive(Clone, Copy, Debug)]
enum ComponentMeasurement {
    Effects,
    InputBytes,
    SnapshotBytes,
    QueueDepth,
    Fuel,
}

#[cfg(feature = "wasmtime")]
impl ComponentMeasurement {
    const fn instrument(self) -> (&'static str, &'static str) {
        match self {
            Self::Effects => ("scxml.component.effects", "{effect}"),
            Self::InputBytes => ("scxml.component.input.size", "By"),
            Self::SnapshotBytes => ("scxml.component.snapshot.size", "By"),
            Self::QueueDepth => ("scxml.component.queue.depth", "{item}"),
            Self::Fuel => ("scxml.component.fuel", "{fuel}"),
        }
    }
}

impl Drop for LifecycleLease {
    fn drop(&mut self) {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let counter = match self.kind {
            LeaseKind::Span => &mut lifecycle.active_spans,
            #[cfg(feature = "wasmtime")]
            LeaseKind::Store => &mut lifecycle.active_stores,
        };
        *counter = counter.saturating_sub(1);
    }
}

impl Default for TelemetryProviders {
    fn default() -> Self {
        Self::new()
    }
}

/// Sequential [`opentelemetry_sdk::trace::IdGenerator`]: trace `1`, span `1`,
/// span `2`, … . It exists for golden replay surfaces (`TelemetryProviders::
/// deterministic`) where a report under `preserve` must be byte-identical
/// across runs; random IDs would make every carrier run-specific.
#[derive(Debug, Default)]
struct DeterministicIdGenerator {
    next: AtomicU64,
}

impl opentelemetry_sdk::trace::IdGenerator for DeterministicIdGenerator {
    fn new_trace_id(&self) -> opentelemetry::trace::TraceId {
        let n = self.next.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        opentelemetry::trace::TraceId::from(u128::from(n))
    }

    fn new_span_id(&self) -> opentelemetry::trace::SpanId {
        let n = self.next.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        opentelemetry::trace::SpanId::from(n)
    }
}

impl TelemetryProviders {
    /// Creates host-owned SDK providers without exporters or ambient I/O.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tracer: opentelemetry_sdk::trace::SdkTracerProvider::builder().build(),
            meter: opentelemetry_sdk::metrics::SdkMeterProvider::builder().build(),
            lifecycle: Arc::new(Mutex::new(ProviderLifecycle::default())),
        }
    }

    /// Creates host-owned SDK providers whose IDs are sequential, so every
    /// span and every derived W3C carrier is byte-reproducible across runs.
    /// For golden replay and determinism proofs only; production callers use
    /// [`TelemetryProviders::new`], whose IDs are random as W3C Trace Context
    /// expects.
    #[must_use]
    pub fn deterministic() -> Self {
        Self {
            tracer: opentelemetry_sdk::trace::SdkTracerProvider::builder()
                .with_id_generator(DeterministicIdGenerator::default())
                .build(),
            meter: opentelemetry_sdk::metrics::SdkMeterProvider::builder().build(),
            lifecycle: Arc::new(Mutex::new(ProviderLifecycle::default())),
        }
    }

    /// Wraps already configured host SDK providers.
    ///
    /// The caller retains shared ownership and may attach exporters/readers
    /// before passing providers here. No provider is installed implicitly.
    #[must_use]
    pub fn from_providers(
        tracer: opentelemetry_sdk::trace::SdkTracerProvider,
        meter: opentelemetry_sdk::metrics::SdkMeterProvider,
    ) -> Self {
        Self {
            tracer,
            meter,
            lifecycle: Arc::new(Mutex::new(ProviderLifecycle::default())),
        }
    }

    /// Creates a guest telemetry adapter backed by this trace provider.
    #[must_use]
    pub fn guest_host(&self) -> TelemetryHost {
        TelemetryHost {
            tracer: self.tracer.clone(),
            lifecycle: Arc::clone(&self.lifecycle),
        }
    }

    /// Installs these providers as the process-global API providers.
    ///
    /// This is initialization-only process mutation. Applications must call it
    /// before worker threads create instruments or spans.
    pub fn install_global(&self) {
        opentelemetry::global::set_tracer_provider(self.tracer.clone());
        opentelemetry::global::set_meter_provider(self.meter.clone());
    }

    /// Flushes and shuts down both providers.
    ///
    /// The method performs exporter/provider side effects configured by the
    /// embedding application and reports a typed failure without retrying.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::TelemetryShutdown`] if either SDK provider rejects
    /// shutdown. Both providers receive one shutdown attempt.
    pub fn shutdown(&self) -> Result<(), HostError> {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match lifecycle.shutdown {
            ShutdownState::Succeeded => {
                return Ok(());
            }
            ShutdownState::Failed => {
                return Err(HostError::TelemetryShutdown);
            }
            ShutdownState::Running => {}
        }
        if lifecycle.active_spans != 0 || lifecycle.active_stores != 0 {
            return Err(HostError::TelemetryInUse);
        }
        let trace_result = self.tracer.shutdown();
        let metric_result = self.meter.shutdown();
        if trace_result.is_err() || metric_result.is_err() {
            lifecycle.shutdown = ShutdownState::Failed;
            Err(HostError::TelemetryShutdown)
        } else {
            lifecycle.shutdown = ShutdownState::Succeeded;
            Ok(())
        }
    }

    /// Flushes configured trace and metric exporters without ending provider ownership.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::TelemetryShutdown`] if either provider cannot flush.
    pub fn force_flush(&self) -> Result<(), HostError> {
        let trace_result = self.tracer.force_flush();
        let metric_result = self.meter.force_flush();
        if trace_result.is_err() || metric_result.is_err() {
            Err(HostError::TelemetryShutdown)
        } else {
            Ok(())
        }
    }

    #[cfg(feature = "wasmtime")]
    fn acquire_store(&self) -> Result<LifecycleLease, HostError> {
        self.acquire(LeaseKind::Store)
    }

    #[cfg(feature = "wasmtime")]
    fn acquire_span(&self) -> Result<LifecycleLease, HostError> {
        self.acquire(LeaseKind::Span)
    }

    #[cfg(feature = "wasmtime")]
    fn acquire(&self, kind: LeaseKind) -> Result<LifecycleLease, HostError> {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if lifecycle.shutdown != ShutdownState::Running {
            return Err(HostError::TelemetryShutdown);
        }
        match kind {
            LeaseKind::Span => lifecycle.active_spans += 1,
            #[cfg(feature = "wasmtime")]
            LeaseKind::Store => lifecycle.active_stores += 1,
        }
        let lease = LifecycleLease {
            lifecycle: Arc::clone(&self.lifecycle),
            kind,
        };
        Ok(lease)
    }

    #[cfg(feature = "wasmtime")]
    fn record_component_measurement(&self, kind: ComponentMeasurement, value: u64) {
        let (instrument, unit) = kind.instrument();
        self.meter
            .meter("scxml-host-wasmtime")
            .u64_histogram(instrument)
            .with_unit(unit)
            .build()
            .record(value, &[]);
    }
}

/// Concrete WIT telemetry host that creates spans through host-owned providers.
///
/// It shares its tracer provider and the provider lifecycle record with the
/// [`TelemetryProviders`] that created it (see "Sharing and mutation" there).
/// Every started [`GuestSpan`] holds a span lease on that record until it is
/// ended, so the owning providers cannot shut down under an open guest span.
#[derive(Clone, Debug)]
pub struct TelemetryHost {
    tracer: opentelemetry_sdk::trace::SdkTracerProvider,
    lifecycle: Arc<Mutex<ProviderLifecycle>>,
}

impl Default for TelemetryHost {
    fn default() -> Self {
        TelemetryProviders::new().guest_host()
    }
}

/// One host-owned guest span and its entered OpenTelemetry context.
pub struct GuestSpan {
    context: Option<Context>,
    _lease: Option<LifecycleLease>,
}

impl TelemetryHost {
    /// Validates a guest carrier before it is used as a remote parent.
    ///
    /// # Errors
    ///
    /// Returns [`TraceContextError`] for malformed or oversized W3C fields.
    pub fn extract_parent(carrier: &TraceContext) -> Result<Context, TraceContextError> {
        scxml_telemetry::extract_trace_context(carrier)
    }

    fn acquire_span(&self) -> Result<LifecycleLease, HostError> {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if lifecycle.shutdown != ShutdownState::Running {
            return Err(HostError::TelemetryShutdown);
        }
        lifecycle.active_spans += 1;
        let lease = LifecycleLease {
            lifecycle: Arc::clone(&self.lifecycle),
            kind: LeaseKind::Span,
        };
        Ok(lease)
    }

    /// Starts a guest span only when its parent is valid and the provider can
    /// grant a lifecycle lease.
    ///
    /// The returned carrier is owned and contains only canonical W3C trace
    /// fields. On failure no span is created and no success carrier is
    /// propagated.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::TraceContext`] for an invalid parent or
    /// [`HostError::TelemetryShutdown`] after provider shutdown.
    pub fn try_start(
        &self,
        module: Module,
        operation: Operation,
        parent: Option<&TraceContext>,
    ) -> Result<(GuestSpan, TraceContext), HostError> {
        let parent_context = match parent {
            Some(carrier) => match Self::extract_parent(carrier) {
                Ok(context) => context,
                Err(error) => {
                    return Err(HostError::from(error));
                }
            },
            None => Context::new(),
        };
        let lease = match self.acquire_span() {
            Ok(lease) => lease,
            Err(error) => {
                return Err(error);
            }
        };
        let tracer = self.tracer.tracer("scxml-guest");
        let span = tracer.start_with_context(operation.as_str(), &parent_context);
        let context = parent_context.with_span(span);
        context.span().set_attributes([
            opentelemetry::KeyValue::new("scxml.module", module.as_str()),
            opentelemetry::KeyValue::new("scxml.operation", operation.as_str()),
        ]);
        let carrier = scxml_telemetry::encode_trace_context(&context);
        if carrier.traceparent.is_none() {
            context.span().end();
            return Err(HostError::TelemetryShutdown);
        }
        Ok((
            GuestSpan {
                context: Some(context),
                _lease: Some(lease),
            },
            carrier,
        ))
    }
}

impl WasmTraceHost for TelemetryHost {
    type Span = GuestSpan;

    fn start(
        &self,
        module: Module,
        operation: Operation,
        parent: Option<&TraceContext>,
    ) -> (Self::Span, Option<TraceContext>) {
        match self.try_start(module, operation, parent) {
            Ok((span, carrier)) => (span, Some(carrier)),
            // A malformed carrier or a refused provider lease yields an inert
            // span: the guest continues untraced and its result is unchanged.
            Err(_) => (
                GuestSpan {
                    context: None,
                    _lease: None,
                },
                None,
            ),
        }
    }

    fn end(&self, span: Self::Span, error: Option<ErrorCategory>) {
        if let Some(context) = span.context {
            if let Some(category) = error {
                context.span().set_status(Status::error(category.as_str()));
                context.span().set_attribute(opentelemetry::KeyValue::new(
                    "error.type",
                    category.as_str(),
                ));
            }
            context.span().end();
        }
    }

    fn event(&self, span: &Self::Span, event: SignalEvent) {
        if let Some(context) = span.context.as_ref() {
            context.span().add_event(event.as_str(), Vec::new());
        }
    }
}

/// Creates a Wasmtime engine configured for components and bounded execution.
///
/// The engine always enables the Component Model and fuel accounting; see
/// [`WasmtimeHostConfig`]. Of that configuration only
/// `max_wasm_stack_bytes` reaches the engine (the session host shares one
/// engine per value of it), and Cranelift compiles functions in parallel.
/// Every linear memory is one read-write mapping of exactly its size that
/// moves when it grows, so a live session costs the process few memory
/// mappings and about 8.6 MiB of address space (docs/limits.md, "Bounds
/// applied on every build"); each move's copy is charged to the host call's fuel
/// (`memory_growth`).
///
/// # Errors
///
/// Returns validation failures before engine construction, or Wasmtime engine
/// construction failures. No engine is returned after either failure.
#[cfg(feature = "wasmtime")]
pub fn component_engine(config: &WasmtimeHostConfig) -> Result<wasmtime::Engine, HostError> {
    let span =
        scxml_telemetry::observe(Module::HostWasmtime, Operation::HostWasmtimeComponentEngine);
    if let Err(error) = config.validate() {
        span.event(SignalEvent::InputRejected);
        span.fail(ErrorCategory::Validation);
        return Err(error);
    }
    let mut wasmtime_config = wasmtime::Config::new();
    wasmtime_config.wasm_component_model(true);
    // The Component Model async ABI (WASI 0.3): invoked services export an
    // `async func` and every component that imports `outbound-http` imports
    // its `async func fetch`.
    wasmtime_config.wasm_component_model_async(true);
    wasmtime_config.consume_fuel(true);
    wasmtime_config.max_wasm_stack(config.max_wasm_stack_bytes);
    // Every linear memory is exactly its size, one read-write mapping
    // with no guard region, no reservation to grow into, and no
    // copy-on-write image (its data segments are copied in), and it moves
    // when it grows. One session is several component instances, and
    // SCXML 6.4 sets no depth for invoked sessions, so a live session must
    // cost the process few memory mappings: Linux allows 65,530 by default
    // (`vm.max_map_count`), and a guard or growth reservation after each
    // memory, or an image mapped inside it, makes it three or four
    // mappings that never merge with their neighbors. Compiled code checks
    // bounds explicitly instead.
    wasmtime_config.memory_reservation(0);
    wasmtime_config.memory_reservation_for_growth(0);
    wasmtime_config.memory_guard_size(0);
    wasmtime_config.memory_init_cow(false);
    wasmtime_config.memory_may_move(true);
    let result = wasmtime::Engine::new(&wasmtime_config).map_err(HostError::from);
    if result.is_ok() {
        span.event(SignalEvent::TransformationCompleted);
        scxml_telemetry::record_runtime_measurement(
            RuntimeMeasurement::WasmtimeStackBytes,
            u64::try_from(config.max_wasm_stack_bytes).unwrap_or(u64::MAX),
        );
    } else {
        span.event(SignalEvent::InputRejected);
        span.fail(ErrorCategory::Platform);
    }
    result
}

/// Store data wrapper that gives one clear owner to user data and limits.
#[cfg(feature = "wasmtime")]
pub struct StoreData<T> {
    /// Embedding application data.
    pub application: T,
    limits: memory_growth::MeteredLimits,
}

/// Creates a Wasmtime store with validated fuel and resource limits.
///
/// The store comes with a call hook and a resource limiter installed. Each
/// outermost call into its Wasm (one with no Wasm of this store below it)
/// runs on at most `config.store.fuel` instructions, even when the
/// embedder refuels the store above that, and may copy at most that many
/// bytes ([`MEMORY_GROWTH_COPY_BYTES_PER_FUEL`] per fuel unit) as its
/// linear memories grow; a growth past that allowance traps as fuel
/// exhaustion (`wasmtime::Trap::OutOfFuel`). Do not replace the call hook
/// (`wasmtime::Store::call_hook`) or the limiter: Wasmtime keeps one of
/// each. Without the call hook no call is held to the fuel cap and the copy
/// allowance never starts again, so every growth after the first allowance
/// is spent traps; without the limiter growth copies are not bounded.
///
/// # Errors
///
/// Returns before exposing a store when configuration validation or Wasmtime
/// fuel setup fails. User data is consumed and dropped on failure.
#[cfg(feature = "wasmtime")]
pub fn component_store<T>(
    engine: &wasmtime::Engine,
    application: T,
    config: &WasmtimeHostConfig,
) -> Result<wasmtime::Store<StoreData<T>>, HostError> {
    let span = scxml_telemetry::observe(Module::HostWasmtime, Operation::HostWasmtimeStore);
    if let Err(error) = config.validate() {
        span.event(SignalEvent::InputRejected);
        span.fail(ErrorCategory::Validation);
        return Err(error);
    }
    let limits = wasmtime::StoreLimitsBuilder::new()
        .memory_size(config.store.memory_bytes)
        .table_elements(config.store.table_elements)
        .instances(config.store.instances)
        .tables(config.store.tables)
        .memories(config.store.memories)
        .trap_on_grow_failure(true)
        .build();
    let limits = memory_growth::MeteredLimits::metered(limits, None, config.store.fuel);
    let budget = std::sync::Arc::clone(limits.budget());
    let mut store = wasmtime::Store::new(
        engine,
        StoreData {
            application,
            limits,
        },
    );
    store.limiter(|data| &mut data.limits);
    budget.share(&mut store);
    if let Err(error) = store.set_fuel(config.store.fuel) {
        span.fail(ErrorCategory::Platform);
        return Err(HostError::from(error));
    }
    record_store_limits(&config.store);
    span.event(SignalEvent::TransformationCompleted);
    Ok(store)
}

#[cfg(feature = "wasmtime")]
fn record_store_limits(config: &WasmtimeStoreConfig) {
    let usize_measurements = [
        (RuntimeMeasurement::WasmtimeMemoryBytes, config.memory_bytes),
        (
            RuntimeMeasurement::WasmtimeTableElements,
            config.table_elements,
        ),
        (RuntimeMeasurement::WasmtimeInstances, config.instances),
        (RuntimeMeasurement::WasmtimeTables, config.tables),
        (RuntimeMeasurement::WasmtimeMemories, config.memories),
    ];
    for (measurement, value) in usize_measurements {
        scxml_telemetry::record_runtime_measurement(
            measurement,
            u64::try_from(value).unwrap_or(u64::MAX),
        );
    }
    scxml_telemetry::record_runtime_measurement(RuntimeMeasurement::WasmtimeFuel, config.fuel);
}

/// Reports whether this build compiled the optional `wasmtime` feature (the
/// Component Model host); `false` on the default, Wasmtime-free surface.
#[must_use]
pub const fn wasmtime_enabled() -> bool {
    cfg!(feature = "wasmtime")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "wasmtime")]
    #[test]
    fn component_measurements_have_fixed_instruments_and_units() {
        assert_eq!(
            ComponentMeasurement::Effects.instrument(),
            ("scxml.component.effects", "{effect}")
        );
        assert_eq!(
            ComponentMeasurement::InputBytes.instrument(),
            ("scxml.component.input.size", "By")
        );
        assert_eq!(
            ComponentMeasurement::SnapshotBytes.instrument(),
            ("scxml.component.snapshot.size", "By")
        );
        assert_eq!(
            ComponentMeasurement::QueueDepth.instrument(),
            ("scxml.component.queue.depth", "{item}")
        );
        assert_eq!(
            ComponentMeasurement::Fuel.instrument(),
            ("scxml.component.fuel", "{fuel}")
        );
    }

    #[cfg(feature = "wasmtime")]
    #[test]
    fn component_measurements_export_one_attribute_free_sample_per_call() {
        let exporter = opentelemetry_sdk::metrics::InMemoryMetricExporter::default();
        let reader = opentelemetry_sdk::metrics::PeriodicReader::builder(exporter.clone()).build();
        let meter = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
            .with_reader(reader)
            .build();
        let providers = TelemetryProviders::from_providers(
            opentelemetry_sdk::trace::SdkTracerProvider::builder().build(),
            meter,
        );

        for (kind, value) in [
            (ComponentMeasurement::Effects, 1),
            (ComponentMeasurement::InputBytes, 2),
            (ComponentMeasurement::SnapshotBytes, 3),
            (ComponentMeasurement::QueueDepth, 4),
            (ComponentMeasurement::Fuel, 5),
        ] {
            providers.record_component_measurement(kind, value);
        }
        providers
            .meter
            .force_flush()
            .expect("in-memory metric exporter flushes");

        let metrics = exporter
            .get_finished_metrics()
            .expect("exported metrics are readable");
        let mut samples = metrics
            .iter()
            .flat_map(opentelemetry_sdk::metrics::data::ResourceMetrics::scope_metrics)
            .flat_map(opentelemetry_sdk::metrics::data::ScopeMetrics::metrics)
            .filter(|metric| metric.name().starts_with("scxml.component."))
            .map(|metric| {
                let (count, sum) = match metric.data() {
                    opentelemetry_sdk::metrics::data::AggregatedMetrics::U64(
                        opentelemetry_sdk::metrics::data::MetricData::Histogram(histogram),
                    ) => histogram
                        .data_points()
                        .map(|point| {
                            assert_eq!(point.attributes().count(), 0);
                            (point.count(), point.sum())
                        })
                        .fold((0, 0), |(count, sum), point| {
                            (count + point.0, sum + point.1)
                        }),
                    _ => (0, 0),
                };
                (metric.name().to_string(), count, sum)
            })
            .collect::<Vec<_>>();
        samples.sort();

        assert_eq!(
            samples,
            [
                ("scxml.component.effects".to_owned(), 1, 1),
                ("scxml.component.fuel".to_owned(), 1, 5),
                ("scxml.component.input.size".to_owned(), 1, 2),
                ("scxml.component.queue.depth".to_owned(), 1, 4),
                ("scxml.component.snapshot.size".to_owned(), 1, 3),
            ]
        );
    }

    const VALID_PARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[test]
    fn defaults_are_bounded_and_valid() {
        let config = WasmtimeHostConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn wasmtime_enabled_reports_the_compiled_feature() {
        assert_eq!(wasmtime_enabled(), cfg!(feature = "wasmtime"));
    }

    #[test]
    fn zero_limits_fail_closed() {
        let config = WasmtimeHostConfig {
            store: WasmtimeStoreConfig {
                memory_bytes: 0,
                ..WasmtimeStoreConfig::default()
            },
            ..WasmtimeHostConfig::default()
        };
        assert!(matches!(
            config.validate(),
            Err(HostError::InvalidLimit(InvalidLimit::MemoryBytes))
        ));
    }

    #[test]
    fn component_limit_cannot_exceed_the_precompilation_ceiling() {
        let config = WasmtimeHostConfig {
            component_bytes: MAX_AUTHENTICATED_COMPONENT_BYTES + 1,
            ..WasmtimeHostConfig::default()
        };
        assert!(matches!(
            config.validate(),
            Err(HostError::Artifact(ArtifactError::ArtifactTooLarge))
        ));
    }

    #[test]
    fn zero_fuel_fails_closed() {
        let zero = WasmtimeHostConfig {
            store: WasmtimeStoreConfig {
                fuel: 0,
                ..WasmtimeStoreConfig::default()
            },
            ..WasmtimeHostConfig::default()
        };
        assert!(matches!(
            zero.validate(),
            Err(HostError::InvalidLimit(InvalidLimit::Fuel))
        ));
    }

    #[test]
    fn adapter_accepts_valid_parent_and_closes_span() {
        let host = TelemetryHost::default();
        let parent = TraceContext {
            traceparent: Some(VALID_PARENT.to_owned()),
            tracestate: Some("vendor=value".to_owned()),
        };
        let adapter = scxml_telemetry::WasmSpanAdapter::new(
            &host,
            Module::HostWasmtime,
            Operation::HostWasmtimeTelemetryStart,
            Some(&parent),
        );
        let propagated = adapter.context().expect("adapter returns a carrier");
        assert!(propagated.traceparent.is_some());
        adapter.event(SignalEvent::InputAccepted);
    }

    #[test]
    fn adapter_rejects_malformed_parent_without_panicking() {
        let malformed = TraceContext {
            traceparent: Some("00-not-a-trace".to_owned()),
            tracestate: None,
        };
        assert!(matches!(
            TelemetryHost::extract_parent(&malformed),
            Err(TraceContextError::TraceparentInvalid)
        ));

        let host = TelemetryHost::default();
        assert!(matches!(
            host.try_start(
                Module::HostWasmtime,
                Operation::HostWasmtimeTelemetryStart,
                Some(&malformed),
            ),
            Err(HostError::TraceContext(
                TraceContextError::TraceparentInvalid
            ))
        ));
        let (_span, propagated) = WasmTraceHost::start(
            &host,
            Module::HostWasmtime,
            Operation::HostWasmtimeTelemetryStart,
            Some(&malformed),
        );
        assert!(
            propagated.is_none(),
            "malformed parent carriers must not fall back to a propagated root span"
        );
    }

    #[test]
    fn adapter_refuses_propagation_after_provider_shutdown() {
        let providers = TelemetryProviders::new();
        let host = providers.guest_host();
        providers
            .shutdown()
            .expect("unused providers shut down cleanly");

        assert!(matches!(
            host.try_start(
                Module::HostWasmtime,
                Operation::HostWasmtimeTelemetryStart,
                None,
            ),
            Err(HostError::TelemetryShutdown)
        ));
        let (_span, propagated) = WasmTraceHost::start(
            &host,
            Module::HostWasmtime,
            Operation::HostWasmtimeTelemetryStart,
            None,
        );
        assert!(
            propagated.is_none(),
            "lease refusal must not propagate a successful span context"
        );
    }

    #[test]
    fn configured_providers_export_guest_spans_and_facade_metrics() {
        let span_exporter = opentelemetry_sdk::trace::InMemorySpanExporter::default();
        let tracer = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_simple_exporter(span_exporter.clone())
            .build();
        let metric_exporter = opentelemetry_sdk::metrics::InMemoryMetricExporter::default();
        let reader =
            opentelemetry_sdk::metrics::PeriodicReader::builder(metric_exporter.clone()).build();
        let meter = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
            .with_reader(reader)
            .build();
        let providers = TelemetryProviders::from_providers(tracer, meter);
        providers.install_global();

        let host = providers.guest_host();
        let adapter = scxml_telemetry::WasmSpanAdapter::new(
            &host,
            Module::HostWasmtime,
            Operation::HostWasmtimeTelemetryStart,
            None,
        );
        adapter.event(SignalEvent::InputAccepted);
        drop(adapter);
        scxml_telemetry::record_runtime_measurement(
            scxml_telemetry::RuntimeMeasurement::WasmtimeFuel,
            42,
        );

        providers
            .tracer
            .force_flush()
            .expect("in-memory span exporter flushes");
        providers
            .meter
            .force_flush()
            .expect("in-memory metric exporter flushes");
        let spans = span_exporter
            .get_finished_spans()
            .expect("exported spans are readable");
        assert!(
            spans
                .iter()
                .any(|span| span.name == Operation::HostWasmtimeTelemetryStart.as_str())
        );
        let metrics = metric_exporter
            .get_finished_metrics()
            .expect("exported metrics are readable");
        assert!(metrics.iter().any(|resource| {
            resource.scope_metrics().any(|scope| {
                scope
                    .metrics()
                    .any(|metric| metric.name() == "scxml.runtime.fuel")
            })
        }));
    }

    #[cfg(feature = "wasmtime")]
    #[test]
    fn provider_shutdown_waits_for_spans_and_stores_and_is_idempotent() {
        let providers = TelemetryProviders::new();
        let host = providers.guest_host();
        let adapter = scxml_telemetry::WasmSpanAdapter::new(
            &host,
            Module::HostWasmtime,
            Operation::HostWasmtimeTelemetryStart,
            None,
        );
        assert!(matches!(
            providers.shutdown(),
            Err(HostError::TelemetryInUse)
        ));
        drop(adapter);

        let store = providers
            .acquire_store()
            .expect("active providers grant a bounded store lease");
        assert!(matches!(
            providers.shutdown(),
            Err(HostError::TelemetryInUse)
        ));
        drop(store);

        assert!(providers.shutdown().is_ok());
        assert!(providers.shutdown().is_ok());
    }

    #[cfg(feature = "wasmtime")]
    #[test]
    fn engine_and_store_apply_fuel_and_resource_limits() {
        let config = WasmtimeHostConfig::default();
        let engine = component_engine(&config).expect("bounded engine configuration is valid");
        let store = component_store(&engine, (), &config).expect("bounded store is created");
        assert_eq!(
            store.get_fuel().expect("fuel accounting is enabled"),
            config.store.fuel
        );
    }

    #[cfg(feature = "wasmtime")]
    #[test]
    fn finite_fuel_traps_an_adversarial_infinite_loop() {
        const INFINITE_LOOP_MODULE: &[u8] = &[
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x07, 0x07, 0x01, 0x03, 0x72, 0x75, 0x6e, 0x00, 0x00, 0x0a,
            0x09, 0x01, 0x07, 0x00, 0x03, 0x40, 0x0c, 0x00, 0x0b, 0x0b,
        ];
        let config = WasmtimeHostConfig {
            store: WasmtimeStoreConfig {
                fuel: 64,
                ..WasmtimeStoreConfig::default()
            },
            ..WasmtimeHostConfig::default()
        };
        let engine = component_engine(&config).expect("bounded engine configuration is valid");
        let mut store = component_store(&engine, (), &config).expect("bounded store is created");
        let module = wasmtime::Module::new(&engine, INFINITE_LOOP_MODULE)
            .expect("adversarial infinite-loop module is valid");
        let instance = wasmtime::Instance::new(&mut store, &module, &[])
            .expect("adversarial module instantiates within resource bounds");
        let run = instance
            .get_typed_func::<(), ()>(&mut store, "run")
            .expect("adversarial module exports run");
        assert!(run.call(&mut store, ()).is_err(), "fuel exhaustion traps");
        assert_eq!(
            store.get_fuel().expect("fuel accounting remains enabled"),
            0
        );
    }

    #[cfg(feature = "wasmtime")]
    #[test]
    fn store_rejects_module_memory_above_configured_limit() {
        const TWO_PAGE_MEMORY_MODULE: &[u8] = &[
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x05, 0x03, 0x01, 0x00, 0x02,
        ];
        let config = WasmtimeHostConfig {
            store: WasmtimeStoreConfig {
                memory_bytes: 64 * 1024,
                ..WasmtimeStoreConfig::default()
            },
            ..WasmtimeHostConfig::default()
        };
        let engine = component_engine(&config).expect("bounded engine configuration is valid");
        let mut store = component_store(&engine, (), &config).expect("bounded store is created");
        let module = wasmtime::Module::new(&engine, TWO_PAGE_MEMORY_MODULE)
            .expect("minimal memory module is valid");
        assert!(wasmtime::Instance::new(&mut store, &module, &[]).is_err());
    }
}
